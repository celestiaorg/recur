//! Remove (archive) and Delete: settle checks, tombstones, the background disk
//! teardown (teardown_cmd, worktree, branch, data dir), cascading to children,
//! discard-risk copy, ClearAll and DeleteProject.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use recur_domain::{Workspace, WorkspaceState};

use crate::Engine;
use crate::attention::chat_inbox::scheduled_pause_settled;
use crate::bus::{EngineEvent, WorkspaceRef};
use crate::core::{EngineError, Result};
use crate::text::html_escape;
use crate::workspace::create::{CreatingAlloc, wait_alloc};

const GIT_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(30);

impl Engine {
    /// Err unless the workspace has no live walk or chat and rests in a
    /// state Archive/Delete accepts (merged, stopped, failed, PR open, or a
    /// finished pause). A workspace that never ran is always settled.
    pub fn check_loop_settled(&self, id: &str, l: &Workspace) -> Result<()> {
        let busy = || EngineError(format!("workspace {id:?} is running: stop it first"));
        if self.loop_key_running(id) || self.workspace_has_live_chat(id) {
            return Err(busy());
        }
        if !l.has_run() {
            return Ok(());
        }
        match l.state {
            WorkspaceState::Merged
            | WorkspaceState::Stopped
            | WorkspaceState::Failed
            | WorkspaceState::PrOpen => Ok(()),
            WorkspaceState::Paused if scheduled_pause_settled(&l.phase) => Ok(()),
            _ => Err(busy()),
        }
    }

    /// Tears a worktree down on disk, best-effort: providers release it
    /// first (Cursor's bridge and project dirs); failures go on the feed
    /// (or the log once the row is gone). The main working copy, PR chat
    /// hosts, and a path equal to the project checkout are never removed.
    pub(crate) async fn remove_loop_worktree(&self, l: &Workspace) {
        if l.worktree_path.is_empty() || l.is_main || l.is_pr_chat_host() {
            return;
        }
        if let Some(proj) = self.store.project(&l.project_id) {
            let wt = clean_path(l.worktree_path.trim());
            let repo = clean_path(proj.repo_path.trim());
            if !wt.is_empty() && !repo.is_empty() && wt == repo {
                return;
            }
        }
        for p in self.providers.lifecycle() {
            if let Err(err) = p.release_worktree(Path::new(&l.worktree_path)).await {
                if self.store.workspace(&l.id).is_some() {
                    self.emit_workspace_event(&l.id, "MA", "Maestro", &html_escape(&err.message));
                } else {
                    tracing::warn!(
                        "recur: releasing provider state for deleted workspace {}: {}",
                        l.id,
                        err.message
                    );
                }
            }
        }
        let Some(proj) = self.store.project(&l.project_id) else {
            return;
        };
        if proj.repo_path.is_empty() {
            return;
        }
        let mgr = self.plain_git_manager(&proj.repo_path);
        let res = tokio::time::timeout(GIT_TEARDOWN_TIMEOUT, mgr.remove(&l.worktree_path))
            .await
            .unwrap_or_else(|_| {
                Err(recur_git::Error::from(std::io::Error::other(
                    "context deadline exceeded",
                )))
            });
        if let Err(err) = res {
            if self.store.workspace(&l.id).is_some() {
                self.emit_workspace_event(
                    &l.id,
                    "MA",
                    "Maestro",
                    &format!(
                        "Could not remove the worktree: {}",
                        html_escape(&err.to_string())
                    ),
                );
            } else {
                tracing::warn!(
                    "recur: removing worktree {} for deleted workspace {}: {err}",
                    l.worktree_path,
                    l.id
                );
            }
        }
    }

    /// Deletes a workspace's checkpoint refs.
    pub async fn prune_checkpoints(&self, repo_path: &str, workspace_id: &str) {
        if repo_path.is_empty() || workspace_id.is_empty() {
            return;
        }
        let prefix = format!("{}{workspace_id}/", recur_git::CHECKPOINT_REF_PREFIX);
        let _ = tokio::time::timeout(
            Duration::from_secs(60),
            recur_git::delete_checkpoints(repo_path, &prefix),
        )
        .await;
    }

    /// Runs teardown_cmd and removes the worktree, branch and data dir of a
    /// workspace whose row is already gone.
    async fn remove_deleted_workspace_disk(&self, mut l: Workspace) {
        if !l.is_main && !l.is_pr_chat_host() {
            let mut path = l.worktree_path.clone();
            if path.is_empty() && !self.worktree_root.is_empty() && !l.id.is_empty() {
                path = format!("{}/{}", self.worktree_root.trim_end_matches('/'), l.id);
            }
            if !path.is_empty() {
                l.worktree_path = path.clone();
                if std::fs::metadata(&path).is_ok_and(|m| m.is_dir())
                    && let Some(proj) = self.store.project(&l.project_id)
                {
                    self.run_teardown_hook(&l, &proj).await;
                }
                self.remove_loop_worktree(&l).await;
            }
            // Recycle a still-at-base branch; a child's branch is always
            // deleted. Remove keeps branches unless the project opts in.
            if !l.branch.is_empty()
                && let Some(proj) = self.store.project(&l.project_id)
                && !proj.repo_path.is_empty()
            {
                let mut base = l.base_branch.clone();
                if base.is_empty() {
                    base = proj.base_branch.clone();
                }
                let mgr = self.plain_git_manager(&proj.repo_path);
                if !l.parent_id.trim().is_empty() && l.branch != base {
                    let _ = tokio::time::timeout(
                        GIT_TEARDOWN_TIMEOUT,
                        mgr.delete_local_branch(&l.branch),
                    )
                    .await;
                } else {
                    let _ = tokio::time::timeout(
                        GIT_TEARDOWN_TIMEOUT,
                        mgr.delete_branch_if_at_base(&l.branch, &base),
                    )
                    .await;
                }
            }
        }
        if let Some(proj) = self.store.project(&l.project_id) {
            self.prune_checkpoints(&proj.repo_path, &l.id).await;
        }
        let _ = std::fs::remove_dir_all(self.data_dir.join("workspaces").join(&l.id));
    }

    /// Archives a settled workspace: history stays, the worktree goes. The
    /// row is archived and the call returns before disk teardown, which runs
    /// in the background (inline once Shutdown began).
    pub async fn remove_workspace(&self, id: &str) -> Result<Workspace> {
        let mut l = self
            .store
            .workspace(id)
            .ok_or_else(|| EngineError(format!("loop {id:?} not found")))?;
        if l.is_main || l.is_pr_chat_host() {
            return Err(
                "the main working copy can't be removed: it tracks the project's real checkout"
                    .into(),
            );
        }
        self.check_loop_settled(id, &l)?;
        // Reserve before waiting on an in-flight create: that allocate can
        // hoist leftover PR chats onto this id after publishing its tree.
        if !self.reserve_deleting(id) {
            return Err(EngineError(format!("workspace {id:?} is being removed")));
        }
        let alloc = self.cancel_creating(id);
        wait_alloc(alloc.as_ref()).await;
        if let Some(r) = self.store.workspace(id) {
            l = r;
        }
        // A queued run may have dispatched while we waited.
        if let Err(e) = self.check_loop_settled(id, &l) {
            self.release_deleting(id);
            return Err(e);
        }
        // The tombstone stops new chat turns; wait out prologues that
        // already passed their liveness check.
        if self.workspace_has_live_chat(id) {
            self.release_deleting(id);
            return Err(EngineError(format!(
                "workspace {id:?} is running: stop it first"
            )));
        }
        self.wait_chat_life_idle(id).await;
        if let Some(r) = self.store.workspace(id) {
            l = r;
        }
        if let Err(e) = self.check_loop_settled(id, &l) {
            self.release_deleting(id);
            return Err(e);
        }
        self.close_chat_child_on_remove(&l);
        let snapshot = l;
        self.clear_ask_users_for_workspace(id);
        self.clear_mode_continues_for_workspace(id);
        self.drop_chat_life_for_workspace(id);
        self.clear_inbox_for_workspace(id);
        let Some(updated) = self.store.update_workspace(id, |lp| {
            lp.archived = true;
            lp.worktree_path.clear();
            lp.port = 0;
        }) else {
            self.release_deleting(id);
            return Err(EngineError(format!("loop {id:?} not found")));
        };
        // Stop painting the worktree before files start vanishing.
        self.unwatch_workspace(id);
        self.publish_loop_updated(id, updated.state.clone(), &updated.phase, updated.iteration);
        // Archived rows drop out of AttentionCounts: push a fresh rollup.
        self.publish_attention(id, "", false);
        self.bus
            .publish(EngineEvent::WorkspaceRemoved(WorkspaceRef::new(id)));
        self.start_removed_workspace_teardown(id, snapshot, None)
            .await;
        Ok(updated)
    }

    async fn start_removed_workspace_teardown(
        &self,
        id: &str,
        l: Workspace,
        alloc: Option<Arc<CreatingAlloc>>,
    ) {
        let shutting_down = {
            let t = self.teardown.lock();
            *t
        };
        if shutting_down {
            wait_alloc(alloc.as_ref()).await;
            self.remove_removed_workspace_disk(&l).await;
            self.release_deleting(id);
            return;
        }
        let e = self.arc();
        let id = id.to_owned();
        self.tasks.teardown.spawn(async move {
            wait_alloc(alloc.as_ref()).await;
            e.remove_removed_workspace_disk(&l).await;
            e.remove_children_of(&id).await;
            e.release_deleting(&id);
        });
    }

    /// Archives a removed parent's live children (fan-out or subagent): a
    /// running child is stopped and given 15 s to settle.
    fn remove_children_of<'a>(
        &'a self,
        parent_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            for c in self.store.workspaces() {
                if c.archived || c.parent_id != parent_id || self.is_shutting_down() {
                    continue;
                }
                self.stop(&c.id).await;
                let prefix = format!("{}\x00", c.id);
                for (key, _) in self.live_chat_turns() {
                    if let Some(code) = key.strip_prefix(&prefix) {
                        self.cancel_chat(&c.id, code).await;
                    }
                }
                let deadline = Instant::now() + Duration::from_secs(15);
                while Instant::now() < deadline {
                    match self.store.workspace(&c.id) {
                        None => break,
                        Some(cur) => {
                            if self.check_loop_settled(&c.id, &cur).is_ok()
                                && !self.workspace_has_live_chat(&c.id)
                            {
                                break;
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                if let Err(err) = self.remove_workspace(&c.id).await {
                    tracing::warn!("remove {parent_id}: child {}: {err}", c.id);
                }
            }
        })
    }

    /// Tears down an archived workspace's tree (and maybe its local branch)
    /// from the pre-archive snapshot.
    async fn remove_removed_workspace_disk(&self, l: &Workspace) {
        if l.is_main || l.is_pr_chat_host() {
            return;
        }
        if l.worktree_path.is_empty() {
            self.cleanup_partial_worktree(l).await;
        } else {
            // Only with the directory present: otherwise the hook would run
            // without a cwd, in the app dir.
            if std::fs::metadata(&l.worktree_path).is_ok_and(|m| m.is_dir())
                && let Some(proj) = self.store.project(&l.project_id)
            {
                self.run_teardown_hook(l, &proj).await;
            }
            self.remove_loop_worktree(l).await;
        }
        self.cleanup_branch_on_remove(l).await;
    }

    /// Recycles a still-at-base branch, or force-deletes the local branch
    /// when the project opts in and no open PR still needs it.
    async fn cleanup_branch_on_remove(&self, l: &Workspace) {
        let branch = l.branch.trim();
        if branch.is_empty() || l.is_main || l.is_pr_chat_host() {
            return;
        }
        let Some(proj) = self.store.project(&l.project_id) else {
            return;
        };
        if proj.repo_path.is_empty() {
            return;
        }
        let mut base = l.base_branch.trim().to_owned();
        if base.is_empty() {
            base = proj.base_branch.trim().to_owned();
        }
        if branch == base {
            return;
        }
        let mgr = self.plain_git_manager(&proj.repo_path);
        let res = if self.should_force_delete_branch_on_remove(l) {
            tokio::time::timeout(GIT_TEARDOWN_TIMEOUT, mgr.delete_local_branch(branch)).await
        } else {
            tokio::time::timeout(
                GIT_TEARDOWN_TIMEOUT,
                mgr.delete_branch_if_at_base(branch, &base),
            )
            .await
        };
        let res = res.unwrap_or_else(|_| {
            Err(recur_git::Error::from(std::io::Error::other(
                "context deadline exceeded",
            )))
        });
        if let Err(err) = res {
            self.emit_workspace_event(
                &l.id,
                "MA",
                "Maestro",
                &format!(
                    "Could not delete branch {}: {}",
                    html_escape(branch),
                    html_escape(&err.to_string())
                ),
            );
        }
    }

    /// The project opts into DeleteBranchOnRemove (a child workspace always
    /// does) and no unmerged PR depends on the branch.
    pub fn should_force_delete_branch_on_remove(&self, l: &Workspace) -> bool {
        if l.parent_id.trim().is_empty() {
            match self.store.project(&l.project_id) {
                Some(p) if p.delete_branch_on_remove => {}
                _ => return false,
            }
        }
        if let Some(pr) = self.store.pr_by_workspace(&l.id)
            && pr.state != "Merged"
            && pr.group != "merged"
        {
            return false;
        }
        true
    }

    /// Drops inbox rows that deep-link into the workspace.
    pub fn clear_inbox_for_workspace(&self, id: &str) {
        if self.store.clear_inbox_for_workspace(id) > 0 {
            self.publish_inbox_updated();
        }
    }

    /// A short phrase when removing the workspace would throw away
    /// uncommitted or unmerged work, or delete a branch with unique commits
    /// ("" when clean). Live children are included: Remove/Delete stops them
    /// and deletes their branches.
    pub async fn workspace_discard_risk(&self, id: &str) -> String {
        let Some(l) = self.store.workspace(id) else {
            return String::new();
        };
        if l.is_main {
            return String::new();
        }
        // One 10 s bound covers the whole inspection; git calls
        // past it report nothing.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut parts = self.workspace_own_discard_risk(&l, deadline).await;
        let mut running = 0;
        for c in self.store.workspaces() {
            if c.archived || c.parent_id != id {
                continue;
            }
            if self.running_loop_by_id(&c.id).is_some()
                || self.workspace_has_live_chat(&c.id)
                || self.chat_resuming(&c.id)
                || c.scheduled_prompt_pending
            {
                running += 1;
            }
            parts.extend(self.workspace_own_discard_risk(&c, deadline).await);
        }
        if running > 0 {
            parts.insert(
                0,
                format!("{running} running child workspace(s) will be stopped"),
            );
        }
        parts.join("; ")
    }

    async fn workspace_own_discard_risk(
        &self,
        l: &Workspace,
        deadline: tokio::time::Instant,
    ) -> Vec<String> {
        let mut parts = Vec::new();
        if !l.worktree_path.is_empty() {
            let s = tokio::time::timeout_at(
                deadline,
                recur_git::inspect_discard_risk(&l.worktree_path),
            )
            .await
            .unwrap_or_default()
            .summary();
            if !s.is_empty() {
                parts.push(s);
            }
        }
        if self.should_force_delete_branch_on_remove(l)
            && let Some(proj) = self.store.project(&l.project_id)
            && !proj.repo_path.is_empty()
        {
            let branch = l.branch.trim();
            let mut base = l.base_branch.trim().to_owned();
            if base.is_empty() {
                base = proj.base_branch.trim().to_owned();
            }
            if !branch.is_empty() && branch != base {
                let mgr = self.plain_git_manager(&proj.repo_path);
                if tokio::time::timeout_at(deadline, mgr.commit_exists(branch))
                    .await
                    .unwrap_or(false)
                {
                    parts.push("branch will be deleted".to_owned());
                }
            }
        }
        parts
    }

    /// Permanently deletes a settled workspace and its cascade. The row goes
    /// and `workspace.deleted` fires before disk teardown runs in the
    /// background.
    pub async fn delete_workspace(&self, id: &str) -> Result<()> {
        let mut l = self
            .store
            .workspace(id)
            .ok_or_else(|| EngineError(format!("loop {id:?} not found")))?;
        self.check_loop_settled(id, &l)?;
        // Hold the kitchen id until teardown finishes; refuse while a hoist
        // covers it.
        if !self.reserve_deleting(id) {
            return Err(EngineError(format!("workspace {id:?} is being removed")));
        }
        self.wait_chat_life_idle(id).await;
        if let Some(r) = self.store.workspace(id) {
            l = r;
        }
        if let Err(e) = self.check_loop_settled(id, &l) {
            self.release_deleting(id);
            return Err(e);
        }
        let alloc = self.cancel_creating(id);
        self.clear_ask_users_for_workspace(id);
        self.clear_mode_continues_for_workspace(id);
        self.drop_chat_life_for_workspace(id);
        if let Err(err) = self.store.delete_workspace(id) {
            if self.store.workspace(id).is_some() {
                self.release_deleting(id);
            }
            return Err(EngineError(err.to_string()));
        }
        self.mcp.drop_workspace(id);
        // After the store delete so a failed delete keeps its watch.
        self.unwatch_workspace(id);
        self.bus
            .publish(EngineEvent::WorkspaceDeleted(WorkspaceRef::new(id)));
        self.publish_inbox_updated();
        self.publish_resource();
        self.start_deleted_workspace_teardown(id, l, alloc).await;
        Ok(())
    }

    async fn start_deleted_workspace_teardown(
        &self,
        id: &str,
        l: Workspace,
        alloc: Option<Arc<CreatingAlloc>>,
    ) {
        let shutting_down = {
            let t = self.teardown.lock();
            *t
        };
        if shutting_down {
            wait_alloc(alloc.as_ref()).await;
            self.remove_deleted_workspace_disk(l).await;
            self.remove_children_of(id).await;
            self.release_deleting(id);
            return;
        }
        let e = self.arc();
        let id = id.to_owned();
        self.tasks.teardown.spawn(async move {
            wait_alloc(alloc.as_ref()).await;
            e.remove_deleted_workspace_disk(l).await;
            e.remove_children_of(&id).await;
            e.release_deleting(&id);
        });
    }

    /// Stops every walk and chat and wipes all data (policy and settings
    /// stay): the clean first-run state.
    pub fn clear_all(&self) -> Result<()> {
        for rl in self.all_running_loops() {
            rl.cancel.cancel();
        }
        for (_, turn) in self.live_chat_turns() {
            turn.cancel_token().cancel();
        }
        self.store
            .clear_all()
            .map_err(|e| EngineError(e.to_string()))?;
        self.chat_life.lock().clear();
        self.invalidate_all_continue_gens();
        self.publish_resource();
        Ok(())
    }

    /// Stops the project's loops, then cascades the delete in the store.
    pub async fn delete_project(&self, id: &str) -> Result<()> {
        for l in self.store.workspaces() {
            if l.project_id == id {
                self.stop(&l.id).await;
            }
        }
        self.store
            .delete_project(id)
            .map_err(|e| EngineError(e.to_string()))?;
        self.publish_resource();
        Ok(())
    }
}

/// Lexically cleans an absolute path for comparing two paths.
fn clean_path(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|l| *l != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    if rooted {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}
