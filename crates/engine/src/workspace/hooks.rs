//! Project setup/teardown commands (Go `ws_hooks.go`): `setup_cmd` once per
//! new worktree, `teardown_cmd` on remove/delete, with the workspace env and
//! a 10-minute bound. Failures land on the feed.

use std::time::Duration;
use tokio::time::Instant;

use recur_domain::{Project, Workspace};
use recur_project::runconfig::{RunError, run_cmd_output};
use recur_project::wsenv::Vars;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::text::html_escape;

/// The setup/teardown bound.
pub const PROJECT_HOOK_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Which hook runs (setup failures and success go on the feed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    Setup,
    Teardown,
}

impl Engine {
    /// Runs teardown_cmd best-effort; a failure is noted on the feed (or
    /// logged once the workspace is gone) and never blocks remove/delete.
    /// Never falls back to the project checkout: a still-creating workspace
    /// has no worktree, and teardown there would mutate the user's repo.
    pub async fn run_teardown_hook(&self, ws: &Workspace, proj: &Project) {
        if ws.worktree_path.trim().is_empty() {
            return;
        }
        if let Err(e) = self
            .run_project_hook(&proj.teardown_cmd, ws, proj, HookKind::Teardown)
            .await
        {
            if self.store.workspace(&ws.id).is_some() {
                self.emit_workspace_event(
                    &ws.id,
                    "MA",
                    "Maestro",
                    &format!("Teardown command failed: {}", html_escape(&e)),
                );
            } else {
                tracing::warn!(
                    "recur: teardown command for deleted workspace {}: {e}",
                    ws.id
                );
            }
        }
    }

    /// A hook with the standard 10-minute bound.
    pub async fn run_project_hook(
        &self,
        shell_cmd: &str,
        ws: &Workspace,
        proj: &Project,
        kind: HookKind,
    ) -> Result<(), String> {
        self.run_project_hook_with(
            &CancellationToken::new(),
            Some(Instant::now() + PROJECT_HOOK_TIMEOUT),
            shell_cmd,
            ws,
            proj,
            kind,
        )
        .await
    }

    /// Runs a hook in the worktree (the repo when there is none; no cwd when
    /// neither exists) with the workspace env. The error is the head
    /// ("timed out after 10m", "cancelled", or the exit status) plus the
    /// output tail.
    pub async fn run_project_hook_with(
        &self,
        cancel: &CancellationToken,
        deadline: Option<Instant>,
        shell_cmd: &str,
        ws: &Workspace,
        proj: &Project,
        kind: HookKind,
    ) -> Result<(), String> {
        let shell_cmd = shell_cmd.trim();
        if shell_cmd.is_empty() {
            return Ok(());
        }
        let mut dir = if ws.worktree_path.is_empty() {
            proj.repo_path.clone()
        } else {
            ws.worktree_path.clone()
        };
        if !dir.is_empty() && !std::fs::metadata(&dir).is_ok_and(|m| m.is_dir()) {
            dir.clear();
        }
        let env = Vars::from_workspace(ws, proj).environ(None);
        let (tail, res) = run_cmd_output(cancel, deadline, shell_cmd, &dir, Some(&env)).await;
        match res {
            Ok(()) => {
                if kind == HookKind::Setup {
                    self.emit_workspace_event(&ws.id, "MA", "Maestro", "Setup command finished.");
                }
                Ok(())
            }
            Err(e) => {
                let cancelled = matches!(e, RunError::Canceled);
                let head = match e {
                    RunError::DeadlineExceeded => "timed out after 10m".to_owned(),
                    RunError::Canceled => "cancelled".to_owned(),
                    other => other.to_string(),
                };
                let err = if tail.is_empty() {
                    head
                } else {
                    format!("{head}\n{tail}")
                };
                if kind == HookKind::Setup && !cancelled {
                    self.emit_workspace_event(
                        &ws.id,
                        "MA",
                        "Maestro",
                        &format!("Setup command failed: {}", html_escape(&err)),
                    );
                    tracing::warn!("recur: setup command for workspace {} failed: {err}", ws.id);
                }
                Err(err)
            }
        }
    }
}
