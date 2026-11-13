//! Projects, sidebar groups, and project.toml named runs.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::serde_util::is_false;
use crate::{MergeCommitMessage, MergeMethod, SquashCommitMessage, UpdateBaseMethod, Workspace};

/// A named, collapsible section in the sidebar Projects tree. Projects
/// reference it via `Project::group_id`; empty means ungrouped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectGroup {
    pub id: String,
    pub name: String,
    /// Expanded in the sidebar (shows member projects).
    pub open: bool,
    /// RFC3339.
    pub created_at: String,
    /// RFC3339.
    pub updated_at: String,
}

/// One entry in the new-workspace base-branch picker. Group is a remote name
/// (origin, upstream, …) or the local branch group ("Local").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BranchChoice {
    pub name: String,
    pub group: String,
}

/// A local folder Recur can open, typically a git repo. Non-git folders are
/// allowed as projects with a working copy only; worktree-based workspaces
/// require `is_git`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Letter, lucide:name, or data:image… avatar.
    pub pic: String,
    /// CSS color var or custom hex.
    pub color: String,
    /// Expanded in the sidebar tree.
    pub open: bool,
    pub idle: bool,
    pub repo_path: String,
    pub base_branch: String,
    /// SetupCmd/TeardownCmd/AppURL/Env/RunCommands are portable run-config
    /// from `<repo>/.recur/project.toml` (computed on read; not stored in
    /// Recur's DB). Settings save writes that file. Runs once when a worktree
    /// is created.
    pub setup_cmd: String,
    /// Runs once on remove/delete (after worktree removal prep).
    pub teardown_cmd: String,
    #[serde(rename = "appURL")]
    pub app_url: String,
    /// Named commands (dev, test, …) from project.toml `[[run]]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub run_commands: Vec<RunCommand>,
    /// The Run button default. Empty = first command. Stored in Recur's DB.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pinned_run: String,
    /// The sidebar project group. Empty means ungrouped.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group_id: String,
    /// Left out of the sidebar unless `Settings::show_hidden_projects`.
    #[serde(skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Dotenv-format text injected into setup/teardown/named runs and
    /// Observatory terminals. Values may reference workspace vars
    /// ($RECUR_PORT, $RECUR_WORKSPACE_ID, $RECUR_WORKTREE, $RECUR_ROOT (the
    /// project checkout), $RECUR_PROJECT_ID, $RECUR_BRANCH, $PORT), host vars,
    /// and keys from .env / .env.local / .recur/.env / .recur/.env.local
    /// (later files win).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub env: String,
    /// Newline-separated globs copied from the checkout into each new
    /// workspace before setup_cmd. Patterns without a slash match at any
    /// depth. Gitignored files are included. Portable via project.toml.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub copy_files: String,
    /// Agents write Conventional Commits subjects and PR titles. From
    /// project.toml `conventional_commits`; computed on read.
    #[serde(skip_serializing_if = "is_false")]
    pub conventional_commits: bool,
    /// True when `<repo>/.recur/project.toml` exists. Computed on read.
    #[serde(skip_serializing_if = "is_false")]
    pub has_project_file: bool,
    /// Set when the file exists but cannot be parsed. Computed on read.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project_file_error: String,
    /// On create/update from project settings, writes .recur/project.toml.
    /// Not persisted.
    #[serde(skip_serializing_if = "is_false")]
    pub write_project_file: bool,
    /// Loops open real GitHub PRs instead of Local PRs. The engine falls back
    /// to a Local PR (with a feed note) without gh or an `origin` remote.
    #[serde(rename = "prOnGithub")]
    pub pr_on_github: bool,
    /// Delete the workspace's local branch after Remove tears down the
    /// worktree. Remote branches are never deleted. Machine-local.
    #[serde(skip_serializing_if = "is_false")]
    pub delete_branch_on_remove: bool,
    /// Comma-separated Linear project names or team keys whose assigned
    /// issues belong to this project. Machine-local.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub linear_projects: String,
    /// Overrides `Settings::default_merge_method`. Empty = inherit.
    #[serde(skip_serializing_if = "MergeMethod::is_empty")]
    pub merge_method: MergeMethod,
    /// Overrides `Settings::default_update_base_method`. Empty = inherit.
    #[serde(skip_serializing_if = "UpdateBaseMethod::is_empty")]
    pub update_base_method: UpdateBaseMethod,
    /// Overrides `Settings::default_squash_commit_message`. Empty = inherit.
    #[serde(skip_serializing_if = "SquashCommitMessage::is_empty")]
    pub squash_commit_message: SquashCommitMessage,
    /// Overrides `Settings::default_merge_commit_message`. Empty = inherit.
    #[serde(skip_serializing_if = "MergeCommitMessage::is_empty")]
    pub merge_commit_message: MergeCommitMessage,
    /// Per-project skill overrides over `Settings::disabled_skills`; see
    /// [`crate::effective_skill_enabled`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_skills: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub enabled_skills: Vec<String>,
    /// Per-project MCP overrides over `Settings::mcp_servers`, same rule as
    /// skills. Machine-local.
    #[serde(rename = "disabledMCP", skip_serializing_if = "Vec::is_empty")]
    pub disabled_mcp: Vec<String>,
    #[serde(rename = "enabledMCP", skip_serializing_if = "Vec::is_empty")]
    pub enabled_mcp: Vec<String>,
    /// True when `repo_path` is inside a git work tree. Computed on read.
    pub is_git: bool,
}

/// The settings-page scan of files matching CopyFiles globs. Files is
/// capped; Count is the full match total.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyFilesPreview {
    pub root: String,
    /// `None` serializes as `null`.
    pub files: Option<Vec<String>>,
    pub count: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// The singleton project that hosts homeless files (paths outside any Recur
/// project or git checkout). Its working copy is an empty Recur-managed
/// folder; the real files are absolute-path editor tabs.
pub const FILES_PROJECT_ID: &str = "files";

/// The outcome of opening a file or folder (File → Open, `recur .`,
/// drag-drop). The UI focuses Workspace and optionally jumps to File.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OpenPathResult {
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    /// Workspace-relative, or absolute when Loose.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub file: String,
    /// Directory to highlight in the explorer.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reveal: String,
    #[serde(skip_serializing_if = "is_false")]
    pub created: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub loose: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<Box<Project>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Box<Workspace>>,
}

/// One named project run (dev, test, …) from project.toml `[[run]]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunCommand {
    pub name: String,
    pub cmd: String,
}

/// The named command list the UI and Run button use, from project.toml
/// `[[run]]`. Blank commands are dropped, blank names become "run", and
/// duplicate names get `-2`, `-3`, … suffixes.
pub fn effective_runs(p: &Project) -> Vec<RunCommand> {
    let out: Vec<RunCommand> = p
        .run_commands
        .iter()
        .filter_map(|r| {
            let cmd = r.cmd.trim();
            if cmd.is_empty() {
                return None;
            }
            let name = match r.name.trim() {
                "" => "run",
                name => name,
            };
            Some(RunCommand {
                name: name.to_owned(),
                cmd: cmd.to_owned(),
            })
        })
        .collect();
    uniquify_run_names(out)
}

/// The pinned command when that name still exists, otherwise the first.
pub fn default_run(p: &Project) -> RunCommand {
    let mut runs = effective_runs(p);
    if runs.is_empty() {
        return RunCommand::default();
    }
    let pin = p.pinned_run.trim();
    let i = if pin.is_empty() {
        None
    } else {
        runs.iter().position(|r| r.name == pin)
    };
    runs.swap_remove(i.unwrap_or(0))
}

/// The command verify-in-browser boots: a run named "dev" if present,
/// otherwise [`default_run`].
pub fn verify_dev_cmd(p: &Project) -> String {
    effective_runs(p)
        .into_iter()
        .find(|r| crate::text::eq_fold(&r.name, "dev"))
        .map(|r| r.cmd)
        .unwrap_or_else(|| default_run(p).cmd)
}

/// A stable fingerprint of [`effective_runs`] for project.toml sync.
pub fn runs_key(p: &Project) -> String {
    effective_runs(p)
        .iter()
        .map(|r| format!("{}\n{}\n", r.name, r.cmd))
        .collect()
}

fn uniquify_run_names(runs: Vec<RunCommand>) -> Vec<RunCommand> {
    let mut used: HashMap<String, i64> = HashMap::new();
    runs.into_iter()
        .map(|r| {
            let mut name = r.name.clone();
            let mut n = used.get(&name).copied().unwrap_or(0);
            used.insert(name.clone(), n + 1);
            if n > 0 {
                name = format!("{}-{}", r.name, n + 1);
                while used.get(&name).copied().unwrap_or(0) > 0 {
                    n += 1;
                    name = format!("{}-{}", r.name, n + 1);
                }
                used.insert(name.clone(), 1);
            }
            RunCommand { name, cmd: r.cmd }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(name: &str, cmd: &str) -> RunCommand {
        RunCommand {
            name: name.into(),
            cmd: cmd.into(),
        }
    }

    #[test]
    fn effective_runs_ignores_empty() {
        assert!(effective_runs(&Project::default()).is_empty());
    }

    #[test]
    fn effective_runs_explicit_wins() {
        let got = effective_runs(&Project {
            run_commands: vec![
                run("dev", "pnpm dev"),
                run("test", "go test ./..."),
                run("  ", ""),
                run("", "echo hi"),
            ],
            ..Default::default()
        });
        let names: Vec<_> = got.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["dev", "test", "run"]);
    }

    #[test]
    fn default_run_pinned() {
        let mut p = Project {
            run_commands: vec![run("dev", "a"), run("test", "b")],
            pinned_run: "test".into(),
            ..Default::default()
        };
        assert_eq!(default_run(&p), run("test", "b"));
        p.pinned_run = "missing".into();
        assert_eq!(default_run(&p).name, "dev");
        assert_eq!(default_run(&Project::default()), RunCommand::default());
    }

    #[test]
    fn verify_dev_cmd_prefers_named_dev() {
        let mut p = Project {
            run_commands: vec![run("test", "go test"), run("dev", "pnpm dev")],
            pinned_run: "test".into(),
            ..Default::default()
        };
        assert_eq!(verify_dev_cmd(&p), "pnpm dev");
        p.run_commands = vec![run("test", "go test")];
        assert_eq!(verify_dev_cmd(&p), "go test");
    }

    #[test]
    fn uniquify_run_names_suffixes() {
        let got = effective_runs(&Project {
            run_commands: vec![run("dev", "a"), run("dev", "b")],
            ..Default::default()
        });
        assert_eq!(got[0].name, "dev");
        assert_eq!(got[1].name, "dev-2");

        let got = effective_runs(&Project {
            run_commands: vec![run("dev", "a"), run("dev-2", "b"), run("dev", "c")],
            ..Default::default()
        });
        let names: Vec<_> = got.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["dev", "dev-2", "dev-3"]);
    }

    #[test]
    fn runs_key_fingerprint() {
        let p = Project {
            run_commands: vec![run(" dev ", " a "), run("", "b")],
            ..Default::default()
        };
        assert_eq!(runs_key(&p), "dev\na\nrun\nb\n");
    }
}
