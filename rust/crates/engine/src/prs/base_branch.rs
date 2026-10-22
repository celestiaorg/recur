//! Changing a workspace's merge base (Go `workspace_base.go`): diffs, PRs
//! and update-with-base compare against it. An open PR is retargeted, on
//! its forge first so Recur and the forge agree.

use std::path::Path;
use std::time::Duration;

use recur_domain::{DiffStat, Workspace};

use crate::Engine;
use crate::bus::{EngineEvent, FsChanged};
use crate::core::{EngineError, Result};
use crate::prs::{none_if_empty, touch_pr_updated};
use crate::text::html_escape;

const VALIDATE_TIMEOUT: Duration = Duration::from_secs(15);
const RETARGET_GIT_TIMEOUT: Duration = Duration::from_secs(15);

impl Engine {
    /// Sets the branch this workspace compares against. The worktree is not
    /// rebased (that stays Update with base). The main working copy has no
    /// merge base.
    pub async fn set_workspace_base_branch(&self, workspace_id: &str, base_branch: &str) -> Result<Workspace> {
        let base_branch = base_branch.trim();
        if base_branch.is_empty() {
            return Err("base branch is required".into());
        }
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        if ws.is_main {
            return Err("the main working copy has no merge base — it tracks the checkout as-is".into());
        }
        if ws.base_branch == base_branch {
            return Ok(ws);
        }
        if !ws.branch.is_empty() && base_branch == ws.branch {
            return Err(EngineError(format!(
                "target branch cannot be the workspace's own branch {:?}",
                ws.branch
            )));
        }
        let proj = self
            .store
            .project(&ws.project_id)
            .filter(|p| !p.repo_path.is_empty())
            .ok_or_else(|| EngineError(format!("project for workspace {workspace_id:?} not found")))?;
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let pr = self.store.pr_by_workspace(workspace_id);
        let open_pr = pr
            .filter(|p| p.state != "Merged" && p.group != "merged");
        let validate = async {
            if !mgr.base_branch_exists(base_branch).await {
                return Err(EngineError(format!(
                    "branch {base_branch:?} does not exist (or has no commits)"
                )));
            }
            if let Some(pr) = &open_pr
                && let Some(forge) = self.pr_forge(pr)
            {
                let forge_base = self.forge_pr_base(&proj.repo_path, base_branch).await;
                forge
                    .retarget_base(Path::new(&proj.repo_path), pr.number, &forge_base)
                    .await
                    .map_err(|e| EngineError(format!("retarget {} PR: {e}", forge.label())))?;
            }
            Ok(())
        };
        tokio::time::timeout(VALIDATE_TIMEOUT, validate)
            .await
            .unwrap_or_else(|_| Err(EngineError::new("context deadline exceeded")))?;

        let updated = self
            .store
            .update_workspace(workspace_id, |w| w.base_branch = base_branch.to_owned())
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        if let Some(pr) = &open_pr {
            self.retarget_open_pr(&mgr, &updated, &pr.id, base_branch).await;
        }
        self.publish_loop_updated(&updated.id, updated.state.clone(), &updated.phase, updated.iteration);
        self.bus.publish(EngineEvent::WorkspaceFsChanged(FsChanged {
            workspace_id: workspace_id.to_owned(),
            ..FsChanged::default()
        }));
        self.emit_workspace_event(
            workspace_id,
            "AR",
            "You",
            &format!(
                "Target branch is now <code>{}</code> — diffs and PRs compare against it.",
                html_escape(base_branch)
            ),
        );
        Ok(updated)
    }

    /// Points an open PR at the new base and refreshes its diff, commits
    /// and behind count. Each snapshot field changes only when its git read
    /// succeeded, so a transient failure cannot blank the PR view.
    pub(crate) async fn retarget_open_pr(
        &self,
        mgr: &recur_git::Manager,
        ws: &Workspace,
        pr_id: &str,
        base: &str,
    ) {
        let (mut diff, mut commits) = (None, None);
        if !ws.worktree_path.is_empty() {
            let _ = tokio::time::timeout(RETARGET_GIT_TIMEOUT, async {
                diff = mgr.diff_pr(&ws.worktree_path, base).await.ok();
                commits = mgr.log_commits(&ws.worktree_path, base).await.ok();
            })
            .await;
        }
        let updated = self.store.update_pr(pr_id, |p| {
            p.base = base.to_owned();
            if let Some(d) = diff {
                p.file_count = d.files.len() as i64;
                p.files = none_if_empty(d.files);
                p.diff = DiffStat {
                    add: d.add,
                    del: d.del,
                };
            }
            if let Some(cs) = commits {
                p.commits = none_if_empty(cs);
            }
            touch_pr_updated(p);
        });
        if updated.is_none() {
            return;
        }
        self.refresh_pr_behind(pr_id).await;
        self.publish_pr_updated(pr_id);
    }
}
