//! The bundle host tools: `install_bundle` (lint, human approval, copy into
//! the user root) and `reload_bundles` (reload plus the installed ids).

use std::path::{Path, PathBuf};

use recur_agent_tools::names::mcp;
use recur_agent_tools::{HostError, PermissionAsk};
use recur_bundles::{
    INTERPRETER_SHEBANG, ParsedBundle, VERB_RUN, discover_catalog, format_lint_report,
    lint_bundles, parse_manifest_full,
};
use recur_domain::PERMISSION_MODE_DENY;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::bus::{BundlesChanged, EngineEvent};
use crate::loops::run::command_label;

fn host_err(e: impl std::fmt::Display) -> HostError {
    HostError(e.to_string())
}

/// What the bundle contributes, and every piece of it that runs a process.
fn contributions(b: &ParsedBundle) -> Value {
    let ids = |v: Vec<&String>| v.into_iter().cloned().collect::<Vec<_>>();
    let mut shell = Vec::new();
    for c in &b.commands {
        if c.interpreter == INTERPRETER_SHEBANG {
            shell.push(format!("command {}", c.id));
        }
    }
    for p in &b.prompts {
        if p.interpreter == INTERPRETER_SHEBANG {
            shell.push(format!("prompt {}", p.id));
        }
        if p.complete_interpreter == INTERPRETER_SHEBANG {
            shell.push(format!("prompt {} completion", p.id));
        }
    }
    for s in &b.searches {
        if s.interpreter == INTERPRETER_SHEBANG {
            shell.push(format!("search {}", s.id));
        }
    }
    for x in &b.expands {
        if x.interpreter == INTERPRETER_SHEBANG {
            shell.push(format!("expand {}", x.id));
        }
    }
    for lp in &b.loops {
        for ph in lp.phases.iter().filter(|p| p.uses == VERB_RUN) {
            let label = command_label(ph);
            shell.push(format!("workflow {} phase {}: {label}", lp.id, ph.id));
        }
    }
    for ls in &b.language_servers {
        let cmd = std::iter::once(&ls.command)
            .chain(&ls.args)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        shell.push(format!("language server {}: {cmd}", ls.id));
    }
    json!({
        "workflows": ids(b.loops.iter().map(|l| &l.id).collect()),
        "roles": ids(b.roles.iter().map(|r| &r.id).collect()),
        "policies": ids(b.policies.iter().map(|p| &p.id).collect()),
        "prompts": ids(b.prompts.iter().map(|p| &p.id).collect()),
        "commands": ids(b.commands.iter().map(|c| &c.id).collect()),
        "languageServers": ids(b.language_servers.iter().map(|l| &l.id).collect()),
        "shell": shell,
    })
}

/// Copies regular files and directories; symlinks are skipped so an
/// install never reaches outside the source tree.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let ty = e.file_type()?;
        let to = dst.join(e.file_name());
        if ty.is_dir() {
            copy_tree(&e.path(), &to)?;
        } else if ty.is_file() {
            std::fs::copy(e.path(), &to)?;
        }
    }
    Ok(())
}

/// Removes the install stage however the install future ends, including
/// when it is dropped mid-approval (a stopped chat drops the MCP handler).
struct StageDir(PathBuf);

impl Drop for StageDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Replaces `dst` with the staged copy.
fn install_stage(stage: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        std::fs::remove_dir_all(dst)?;
    }
    std::fs::rename(stage, dst)
}

impl Engine {
    /// The bundle directory `path` names inside the workspace's worktree.
    fn worktree_bundle_dir(&self, workspace_id: &str, path: &str) -> Result<PathBuf, HostError> {
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| HostError::new("workspace not found"))?;
        if ws.worktree_path.is_empty() {
            return Err(HostError::new("this workspace has no worktree"));
        }
        let root = std::fs::canonicalize(&ws.worktree_path).map_err(host_err)?;
        let dir = std::fs::canonicalize(root.join(path))
            .map_err(|_| HostError(format!("{path:?} is not a directory in the worktree")))?;
        if !dir.starts_with(&root) || !dir.is_dir() {
            return Err(HostError(format!(
                "{path:?} is not a directory in the worktree"
            )));
        }
        if !dir.join("bundle.toml").is_file() {
            return Err(HostError(format!("{path:?} has no bundle.toml")));
        }
        Ok(dir)
    }

    /// Host `install_bundle`. Every install asks the human, whatever the
    /// permission mode (deny refuses): bundle shell code runs as the user.
    pub async fn install_bundle_from_chat(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        chat: &str,
        path: &str,
    ) -> Result<String, HostError> {
        let src = self.worktree_bundle_dir(workspace_id, path)?;
        let roots = self.bundle_roots();
        if roots.user_dir.trim().is_empty() {
            return Err(HostError::new("no user bundles directory"));
        }
        let dir_name = src.file_name().unwrap_or_default().to_string_lossy();
        if dir_name.starts_with('.') {
            return Err(HostError(format!(
                "bundle directory {dir_name:?} is hidden"
            )));
        }
        let dst = Path::new(&roots.user_dir).join(&*dir_name);
        if !roots.default_dir.is_empty() && dst.starts_with(&roots.default_dir) {
            return Err(HostError::new("shipped bundles cannot be written"));
        }
        if self.settings().permission_mode() == PERMISSION_MODE_DENY {
            return Err(HostError::new(
                "permission denied: installing a bundle needs approval",
            ));
        }
        let overwrite = dst.exists();
        std::fs::create_dir_all(&roots.user_dir).map_err(host_err)?;

        // Lint, approval and install all read one snapshot, so worktree
        // edits made while the human decides are not installed unseen.
        // Discovery skips dot-directories, so the stage stays inert.
        let guard = StageDir(
            dst.with_file_name(format!(".{dir_name}.installing-{}", uuid::Uuid::now_v7())),
        );
        let stage = guard.0.as_path();
        let res = async {
            copy_tree(&src, stage).map_err(|e| HostError(format!("install failed: {e}")))?;
            let stage_str = stage.to_string_lossy().into_owned();
            let cache = self.data_dir().join("BundleCache");
            let rep = lint_bundles(std::slice::from_ref(&stage_str), &cache.to_string_lossy());
            if !rep.ok() {
                let report =
                    format_lint_report(&rep, false).replace(&stage_str, &src.to_string_lossy());
                return Err(HostError(format!(
                    "lint failed, nothing installed:\n{report}"
                )));
            }
            let toml = std::fs::read(stage.join("bundle.toml")).map_err(host_err)?;
            let parsed = parse_manifest_full(&stage_str, &toml).map_err(host_err)?;
            let mut input = json!({
                "name": parsed.meta.name,
                "id": parsed.meta.id,
                "from": path,
                "to": dst.to_string_lossy(),
                "replaces": overwrite,
            });
            if let (Value::Object(m), Value::Object(c)) = (&mut input, contributions(&parsed)) {
                m.extend(c);
            }
            let v = self
                .request_permission(
                    cancel,
                    workspace_id,
                    chat,
                    PermissionAsk {
                        tool: mcp::INSTALL_BUNDLE.into(),
                        input: input.to_string(),
                        tool_use_id: String::new(),
                    },
                )
                .await?;
            if !v.allow {
                let reason = if v.reason.is_empty() {
                    "denied"
                } else {
                    &v.reason
                };
                return Err(HostError(format!("permission denied: {reason}")));
            }
            install_stage(stage, &dst).map_err(|e| HostError(format!("install failed: {e}")))?;
            input["installed"] = true.into();
            Ok(input.to_string())
        }
        .await;
        drop(guard);
        if res.is_ok() {
            self.bus
                .publish(EngineEvent::BundlesChanged(BundlesChanged {}));
        }
        res
    }

    /// Host `reload_bundles`: reloads the app's bundles and lists the
    /// catalog's workflows, roles and policies (what a new bundle can reuse).
    pub fn reload_bundles_from_chat(&self) -> Result<String, HostError> {
        let cat = discover_catalog(self.bundle_roots()).map_err(host_err)?;
        self.bus
            .publish(EngineEvent::BundlesChanged(BundlesChanged {}));
        let row = |id: &str, name: &str, bundle: &str, source: &str| json!({"id": id, "name": name, "bundle": bundle, "source": source});
        Ok(json!({
            "workflows": cat.loops.iter().map(|l| row(&l.id, &l.name, &l.bundle_name, &l.source_root)).collect::<Vec<_>>(),
            "roles": cat.roles.iter().map(|r| {
                let mut v = row(&r.id, &r.name, &r.bundle_name, &r.source_root);
                v["mode"] = r.mode.clone().into();
                v
            }).collect::<Vec<_>>(),
            "policies": cat.policies.iter().map(|p| row(&p.id, &p.name, &p.bundle_name, &p.source_root)).collect::<Vec<_>>(),
            "skipped": cat.skips.iter().map(|s| json!({"dir": s.dir, "reason": s.reason})).collect::<Vec<_>>(),
        })
        .to_string())
    }
}
