//! Live branch sync: the checkout is the source of truth (humans and agents
//! rename or switch branches mid-session), so the cached Workspace.Branch,
//! every live walk on it, and an open Local PR follow. A forge PR's branch
//! stays frozen at its remote head.

use std::time::Duration;

use recur_domain::PrSource;

use crate::Engine;

impl Engine {
    /// Reads the live branch and, when it differs from the cached one,
    /// updates the store, the running walks, and an open Local PR, then
    /// publishes. Returns the branch git ops should use (live when known).
    pub async fn sync_workspace_branch(&self, workspace_id: &str) -> String {
        self.ws_upstream_cache.lock().remove(workspace_id);
        let Some(ws) = self.store.workspace(workspace_id) else {
            return String::new();
        };
        if ws.worktree_path.is_empty() {
            return ws.branch;
        }
        let live = match tokio::time::timeout(
            Duration::from_secs(5),
            recur_git::current_branch(&ws.worktree_path),
        )
        .await
        {
            Ok(Ok(b)) if !b.is_empty() => b,
            _ => return ws.branch,
        };
        if live == ws.branch {
            return live;
        }
        let Some(updated) = self
            .store
            .update_workspace(workspace_id, |w| w.branch = live.clone())
        else {
            return live;
        };
        // The walk's branch lives under its handle's lock.
        for rl in self.running_loops_on(workspace_id) {
            if rl.handle.map_key == workspace_id {
                rl.handle.set_branch(&live);
            }
        }
        if let Some(pr) = self.store.pr_by_workspace(workspace_id)
            && pr.state != "Merged"
            && pr.source == PrSource::Local
            && pr.branch != live
            && self
                .store
                .update_pr(&pr.id, |p| p.branch = live.clone())
                .is_some()
        {
            self.publish_pr_updated(&pr.id);
        }
        self.publish_loop_updated(
            &updated.id,
            updated.state.clone(),
            &updated.phase,
            updated.iteration,
        );
        self.link_workspace_to_matching_prs(&updated).await;
        live
    }
}
