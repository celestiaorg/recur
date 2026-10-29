//! Workspace ↔ forge PR linking by head branch: a workspace checked out on a
//! PR's head, or tracking it under another local name (`@{upstream}`), is that
//! PR's workspace. The project base is never a head: a worktree sitting on main
//! is not a PR.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use recur_domain::Workspace;

use crate::Engine;
use crate::prs::pr_settled;
use crate::workspace::create::normalize_pr_branch;

/// Bounds the `@{upstream}` reads of one linking pass.
const LINK_GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Bounds a whole import linking pass.
const LINK_ALL_TIMEOUT: Duration = Duration::from_secs(30);

impl Engine {
    /// The project's base branch as a PR branch name.
    pub(crate) fn project_base_branch(&self, project_id: &str) -> String {
        self.store
            .project(project_id)
            .map(|p| normalize_pr_branch(&p.base_branch))
            .unwrap_or_default()
    }

    /// The remote branch `@{upstream}` points at, cached per workspace
    /// ("" when the worktree is missing or the branch tracks nothing).
    pub(crate) async fn lookup_workspace_upstream_branch(
        &self,
        ws: &Workspace,
        deadline: Instant,
    ) -> String {
        if ws.worktree_path.trim().is_empty() {
            return String::new();
        }
        if let Some(v) = self.ws_upstream_cache.lock().get(&ws.id) {
            return v.clone();
        }
        self.refresh_workspace_upstream_branch(ws, deadline).await
    }

    /// Re-reads `@{upstream}` and overwrites the cache entry in place, so a
    /// tracking ref set outside Recur is seen without a window where the
    /// entry is missing.
    pub(crate) async fn refresh_workspace_upstream_branch(
        &self,
        ws: &Workspace,
        deadline: Instant,
    ) -> String {
        let path = ws.worktree_path.trim();
        if path.is_empty() {
            return String::new();
        }
        let repo = self
            .store
            .project(&ws.project_id)
            .map(|p| p.repo_path)
            .filter(|r| !r.trim().is_empty())
            .unwrap_or_else(|| path.to_owned());
        let mgr = recur_git::Manager::new(&repo, &self.worktree_root);
        let name = tokio::time::timeout_at(deadline, mgr.upstream(path))
            .await
            .ok()
            .flatten()
            .map(|up| normalize_pr_branch(&up.branch))
            .unwrap_or_default();
        self.ws_upstream_cache
            .lock()
            .insert(ws.id.clone(), name.clone());
        name
    }

    /// Live workspaces of a project that may be a PR head, with the local
    /// branch name.
    fn pr_head_candidates(&self, project_id: &str, base: &str) -> Vec<(Workspace, String)> {
        self.store
            .workspaces()
            .into_iter()
            .filter(|w| {
                !w.archived && !w.is_main && !w.is_pr_chat_host() && w.project_id == project_id
            })
            .filter_map(|w| {
                let local = normalize_pr_branch(&w.branch);
                (!local.is_empty() && (base.is_empty() || local != base)).then_some((w, local))
            })
            .collect()
    }

    /// Head name → live workspace on that branch. Local checkout names win;
    /// `@{upstream}` fills in when the local name differs from the remote.
    pub(crate) async fn index_workspaces_by_pr_head(
        &self,
        project_id: &str,
        deadline: Instant,
    ) -> HashMap<String, Workspace> {
        let mut idx = HashMap::new();
        if project_id.is_empty() {
            return idx;
        }
        let base = self.project_base_branch(project_id);
        let live = self.pr_head_candidates(project_id, &base);
        for (w, local) in &live {
            idx.entry(local.clone()).or_insert_with(|| w.clone());
        }
        for (w, local) in &live {
            let up = self.refresh_workspace_upstream_branch(w, deadline).await;
            if up.is_empty() || &up == local || (!base.is_empty() && up == base) {
                continue;
            }
            idx.entry(up).or_insert_with(|| w.clone());
        }
        idx
    }

    /// The live workspace on `branch` (by local name first, then tracking).
    pub(crate) async fn workspace_matching_pr_branch(
        &self,
        project_id: &str,
        branch: &str,
    ) -> Option<Workspace> {
        let branch = normalize_pr_branch(branch);
        if project_id.is_empty() || branch.is_empty() {
            return None;
        }
        // Not every PR whose head is the project default is "this branch".
        let base = self.project_base_branch(project_id);
        if !base.is_empty() && branch == base {
            return None;
        }
        let deadline = Instant::now() + LINK_GIT_TIMEOUT;
        let mut need_up = Vec::new();
        for w in self.store.workspaces() {
            if w.archived || w.is_main || w.is_pr_chat_host() || w.project_id != project_id {
                continue;
            }
            let local = normalize_pr_branch(&w.branch);
            if local.is_empty() {
                continue;
            }
            if local == branch {
                return Some(w);
            }
            need_up.push(w);
        }
        for w in need_up {
            if self.lookup_workspace_upstream_branch(&w, deadline).await == branch {
                return Some(w);
            }
        }
        None
    }

    /// The local branch plus `@{upstream}`, excluding the project base.
    async fn workspace_pr_heads(&self, ws: &Workspace, deadline: Instant) -> Vec<String> {
        if ws.archived || ws.is_main || ws.is_pr_chat_host() {
            return Vec::new();
        }
        let base = self.project_base_branch(&ws.project_id);
        let local = normalize_pr_branch(&ws.branch);
        if local.is_empty() || (!base.is_empty() && local == base) {
            return Vec::new();
        }
        let mut heads = vec![local.clone()];
        let up = self.lookup_workspace_upstream_branch(ws, deadline).await;
        if !up.is_empty() && up != local && (base.is_empty() || up != base) {
            heads.push(up);
        }
        heads
    }

    /// Attaches each open forge PR without a live workspace to a workspace
    /// already on its head, so View workspace shows without a click.
    pub(crate) async fn link_unlinked_prs(&self) {
        let deadline = Instant::now() + LINK_ALL_TIMEOUT;
        {
            let mut indexes: HashMap<String, HashMap<String, Workspace>> = HashMap::new();
            for p in self.store.prs() {
                if self.pr_forge(&p).is_none() || pr_settled(&p) {
                    continue;
                }
                if !self.live_pr_workspace_id(&p.workspace_id).is_empty() {
                    continue;
                }
                let pid = self.pr_project_id(&p);
                if !indexes.contains_key(&pid) {
                    let idx = self.index_workspaces_by_pr_head(&pid, deadline).await;
                    indexes.insert(pid.clone(), idx);
                }
                let Some(w) = indexes
                    .get(&pid)
                    .and_then(|idx| idx.get(&normalize_pr_branch(&p.branch)))
                    .cloned()
                else {
                    continue;
                };
                self.store.update_pr(&p.id, |pr| {
                    pr.workspace_id = w.id.clone();
                    if pr.project_id.trim().is_empty() {
                        pr.project_id = pid.clone();
                    }
                });
                self.archive_pr_chat_hosts(&p.id, &w.id).await;
            }
        }
    }

    /// The reverse of import linking: a workspace whose branch just became
    /// known (allocate, rename) picks up a forge PR already on that head,
    /// including when the local name differs from the tracked remote.
    pub(crate) async fn link_workspace_to_matching_prs(&self, ws: &Workspace) {
        if ws.archived || ws.is_main || ws.is_pr_chat_host() {
            return;
        }
        let heads = self
            .workspace_pr_heads(ws, Instant::now() + LINK_GIT_TIMEOUT)
            .await;
        if ws.project_id.is_empty() || heads.is_empty() {
            return;
        }
        for p in self.store.prs() {
            if self.pr_forge(&p).is_none() || pr_settled(&p) {
                continue;
            }
            if self.pr_project_id(&p) != ws.project_id {
                continue;
            }
            if !self.live_pr_workspace_id(&p.workspace_id).is_empty() {
                continue;
            }
            if !heads.contains(&normalize_pr_branch(&p.branch)) {
                continue;
            }
            self.store.update_pr(&p.id, |pr| {
                pr.workspace_id = ws.id.clone();
                if pr.project_id.trim().is_empty() {
                    pr.project_id = ws.project_id.clone();
                }
            });
            self.publish_pr_updated(&p.id);
            self.archive_pr_chat_hosts(&p.id, &ws.id).await;
        }
    }
}
