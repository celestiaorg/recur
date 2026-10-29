//! The main working copy against its remote (E1) fetch the tips the titlebar
//! compares against, update from the tracking branch, and push. The open forge
//! PR head fetch and the land state are E5's (`prs`).

use std::time::Duration;

use recur_domain::{
    UPDATE_WORKSPACE_AUTOSTASH, UPDATE_WORKSPACE_NOOP, UPDATE_WORKSPACE_UPDATED, UpdateBaseMethod,
    UpdateWorkspaceResult, Workspace, WorkspaceLandState, effective_update_base_method,
    normalize_update_base_method,
};

use crate::Engine;
use crate::core::{EngineError, Result};
use crate::text::html_escape;

const REMOTE_OP_TIMEOUT: Duration = Duration::from_secs(60);

impl Engine {
    /// Refreshes the remote tips the titlebar compares against (the main
    /// copy's tracking branch, a workspace's merge base, an open forge PR
    /// head) and returns the land state. Best-effort, never an error.
    pub async fn fetch_working_copy_upstream(&self, workspace_id: &str) -> WorkspaceLandState {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return self.workspace_land_state(workspace_id).await;
        };
        if ws.worktree_path.is_empty() || !recur_git::is_repo(&ws.worktree_path).await {
            return self.workspace_land_state(workspace_id).await;
        }
        if ws.is_main {
            let mgr = recur_git::Manager::new(&ws.worktree_path, &self.worktree_root);
            let _ = tokio::time::timeout(recur_git::REMOTE_FETCH_TIMEOUT, async {
                if let Some(up) = mgr.upstream(&ws.worktree_path).await {
                    let _ = mgr.fetch_remote_branch(&up.remote, &up.branch).await;
                }
            })
            .await;
            return self.workspace_land_state(workspace_id).await;
        }
        self.fetch_workspace_merge_base(&ws).await;
        self.fetch_open_pr_remote(&ws).await;
        self.workspace_land_state(workspace_id).await
    }

    async fn fetch_workspace_merge_base(&self, ws: &Workspace) {
        let Some(proj) = self.store.project(&ws.project_id) else {
            return;
        };
        if proj.repo_path.is_empty() {
            return;
        }
        let mut base = ws.base_branch.trim().to_owned();
        if base.is_empty() {
            base = proj.base_branch.trim().to_owned();
        }
        if base.is_empty() {
            return;
        }
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let _ = tokio::time::timeout(
            recur_git::REMOTE_FETCH_TIMEOUT,
            mgr.fetch_compare_ref(&base),
        )
        .await;
    }

    /// Fetches the main copy's upstream and merges or rebases it into HEAD
    /// (`--autostash`). On conflict the update is aborted and the checkout
    /// left as it was; an update the user already started is refused. When
    /// autostash cannot reapply dirty edits, status is `autostash`.
    pub async fn update_working_copy_from_remote(
        &self,
        workspace_id: &str,
        method: UpdateBaseMethod,
    ) -> Result<UpdateWorkspaceResult> {
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        if !ws.is_main {
            return Err("update from remote is only for the main working copy".into());
        }
        if ws.worktree_path.is_empty() || !recur_git::is_repo(&ws.worktree_path).await {
            return Err("this folder is not a git checkout".into());
        }
        tokio::time::timeout(
            REMOTE_OP_TIMEOUT,
            self.update_working_copy(workspace_id, ws, method),
        )
        .await
        .unwrap_or_else(|_| Err("context deadline exceeded".into()))
    }

    async fn update_working_copy(
        &self,
        workspace_id: &str,
        ws: Workspace,
        method: UpdateBaseMethod,
    ) -> Result<UpdateWorkspaceResult> {
        let wt = ws.worktree_path.clone();
        let mgr = recur_git::Manager::new(&wt, &self.worktree_root);
        let up = mgr
            .upstream(&wt)
            .await
            .ok_or_else(|| EngineError::new("this branch is not tracking a remote"))?;
        let fetch_err = mgr.fetch_remote_branch(&up.remote, &up.branch).await.err();

        let proj = self.store.project(&ws.project_id).unwrap_or_default();
        let method = if method.is_empty() {
            effective_update_base_method(&proj, &self.settings())
        } else {
            normalize_update_base_method(method.as_str())
        };
        if mgr.update_in_progress(&wt).await {
            return Err(
                "a merge or rebase is already in progress — finish or abort it before updating"
                    .into(),
            );
        }

        let behind = mgr.commits_behind(&wt, &up.ref_name).await.unwrap_or(0);
        if behind == 0 {
            if let Some(e) = fetch_err {
                return Err(EngineError(format!("fetch {}: {e}", up.ref_name)));
            }
            return Ok(UpdateWorkspaceResult {
                status: UPDATE_WORKSPACE_NOOP.into(),
                workspace: ws,
                base: up.ref_name,
                ..UpdateWorkspaceResult::default()
            });
        }

        let stash_before = mgr.stash_tip(&wt).await;
        match mgr
            .begin_update_with_base_autostash(&wt, &up.ref_name, method.as_str())
            .await
        {
            Ok(()) => {
                let mut status = UPDATE_WORKSPACE_UPDATED;
                let mut event = format!(
                    "Updated with latest <code>{}</code> ({}).",
                    html_escape(&up.ref_name),
                    html_escape(method.label())
                );
                if mgr.autostash_left_in_stash(&wt, &stash_before).await {
                    if let Err(e) = mgr.reset_worktree_to_head(&wt).await {
                        return Err(EngineError(format!(
                            "updated from {} but leftover edits are in git stash and the working tree could not be cleaned: {e}",
                            up.ref_name
                        )));
                    }
                    status = UPDATE_WORKSPACE_AUTOSTASH;
                    event = format!(
                        "Updated with latest <code>{}</code> ({}), but uncommitted edits could not be reapplied and were left in git stash.",
                        html_escape(&up.ref_name),
                        html_escape(method.label())
                    );
                }
                self.sync_workspace_diff(workspace_id, true).await;
                self.emit_workspace_event(workspace_id, "AR", "You", &event);
                let ws = self.store.workspace(workspace_id).unwrap_or_default();
                Ok(UpdateWorkspaceResult {
                    status: status.into(),
                    workspace: ws,
                    base: up.ref_name,
                    ..UpdateWorkspaceResult::default()
                })
            }
            Err(err) => {
                if mgr.update_in_progress(&wt).await {
                    if let Err(aerr) = mgr.abort_in_progress_update(&wt).await {
                        return Err(EngineError(format!(
                            "update from {} hit conflicts and could not abort: {aerr}",
                            up.ref_name
                        )));
                    }
                    return Err(EngineError(format!(
                        "incoming changes on {} conflict with your branch — your checkout was left unchanged",
                        up.ref_name
                    )));
                }
                Err(EngineError(format!("update from {}: {err}", up.ref_name)))
            }
        }
    }

    /// Pushes the main copy's HEAD to its tracking remote. Uncommitted files
    /// stay (a working copy is not one body of work); unmerged or behind
    /// checkouts are refused.
    pub async fn push_working_copy(&self, workspace_id: &str) -> Result<Workspace> {
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found")))?;
        if !ws.is_main {
            return Err("push is only for the main working copy".into());
        }
        if ws.worktree_path.is_empty() || !recur_git::is_repo(&ws.worktree_path).await {
            return Err("this folder is not a git checkout".into());
        }
        tokio::time::timeout(
            REMOTE_OP_TIMEOUT,
            self.push_working_copy_inner(workspace_id, &ws.worktree_path),
        )
        .await
        .unwrap_or_else(|_| Err("context deadline exceeded".into()))
    }

    async fn push_working_copy_inner(&self, workspace_id: &str, wt: &str) -> Result<Workspace> {
        let mgr = recur_git::Manager::new(wt, &self.worktree_root);
        let up = mgr
            .upstream(wt)
            .await
            .ok_or_else(|| EngineError::new("this branch is not tracking a remote"))?;
        if recur_git::inspect_discard_risk(wt).await.unmerged {
            return Err("resolve merge conflicts first".into());
        }
        let _ = mgr.fetch_remote_branch(&up.remote, &up.branch).await;
        if let Ok(behind) = mgr.commits_behind(wt, &up.ref_name).await
            && behind > 0
        {
            return Err(EngineError(format!(
                "this branch is {behind} commits behind {} — update first",
                up.ref_name
            )));
        }
        let ahead = mgr.commits_ahead(wt, &up.ref_name).await.unwrap_or(0);
        if ahead == 0 {
            return Err(EngineError(format!(
                "already up to date with {}",
                up.ref_name
            )));
        }
        mgr.push_upstream(wt)
            .await
            .map_err(|e| EngineError(e.to_string()))?;
        self.emit_workspace_event(
            workspace_id,
            "AR",
            "You",
            &format!("Pushed to <code>{}</code>.", html_escape(&up.remote)),
        );
        self.sync_workspace_diff(workspace_id, true).await;
        self.store
            .workspace(workspace_id)
            .ok_or_else(|| EngineError(format!("workspace {workspace_id:?} not found after push")))
    }
}
