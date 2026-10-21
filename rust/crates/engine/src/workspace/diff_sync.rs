//! Sidebar +N/−M and behind-base sync (Go `diff_sync.go`): a 600 ms debounce
//! per workspace for bursty edits, a generation guard so a slow older git
//! never overwrites a fresher result, and a single-flight refresh of every
//! workspace (one trailing rerun, four gits at a time).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use recur_domain::DiffStat;
use tokio_util::sync::CancellationToken;

use crate::Engine;

/// Coalesces bursty edits into one numstat refresh.
pub const DIFF_SYNC_DELAY: Duration = Duration::from_millis(600);
/// Workspace numstat gits run at once during a full refresh.
pub const DIFF_REFRESH_PARALLEL: usize = 4;

/// Go `diffSyncMu` and what it guards.
#[derive(Default)]
pub struct DiffSync {
    /// Pending debounce per workspace: (timer id, cancel before it fires).
    timers: HashMap<String, (u64, CancellationToken)>,
    next_timer: u64,
    /// Bumped per sync start; a stale result skips the store.
    generation: HashMap<String, u64>,
    /// Serializes store write + publish per workspace.
    apply: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

/// Single-flight state of the refresh-all pass.
#[derive(Default)]
pub struct RefreshAll {
    pub busy: bool,
    pub pending: bool,
}

impl Engine {
    /// Debounces a live diff refresh + publish for one workspace.
    pub fn schedule_workspace_diff_sync(&self, workspace_id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mut ds = self.diff_sync.lock();
        if let Some((_, stop)) = ds.timers.remove(workspace_id) {
            stop.cancel();
        }
        ds.next_timer += 1;
        let timer = ds.next_timer;
        let stop = CancellationToken::new();
        ds.timers
            .insert(workspace_id.to_owned(), (timer, stop.clone()));
        drop(ds);
        let e = self.arc();
        let id = workspace_id.to_owned();
        rt.spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(DIFF_SYNC_DELAY) => {}
                _ = stop.cancelled() => return,
            }
            e.sync_workspace_diff(&id, true).await;
            // Clear only our own slot: a newer schedule may have replaced it.
            let mut ds = e.diff_sync.lock();
            if ds.timers.get(&id).is_some_and(|(t, _)| *t == timer) {
                ds.timers.remove(&id);
            }
        });
    }

    /// Kicks off a background refresh of every workspace's diff and returns
    /// at once. Concurrent callers coalesce into one pass plus one trailing
    /// rerun.
    pub fn schedule_refresh_all_workspace_diffs(&self) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        {
            let mut r = self.diff_refresh_all.lock();
            if r.busy {
                r.pending = true;
                return;
            }
            r.busy = true;
        }
        let e = self.arc();
        rt.spawn(async move {
            loop {
                e.refresh_all_workspace_diffs().await;
                let mut r = e.diff_refresh_all.lock();
                if !r.pending {
                    r.busy = false;
                    return;
                }
                r.pending = false;
            }
        });
    }

    /// Whether the refresh-all pass is running or has a rerun queued.
    pub fn refresh_all_workspace_diffs_busy(&self) -> (bool, bool) {
        let r = self.diff_refresh_all.lock();
        (r.busy, r.pending)
    }

    /// Recomputes every workspace's diff (four at a time), publishing the
    /// ones that changed. Rows without an on-disk worktree are left alone.
    pub async fn refresh_all_workspace_diffs(&self) {
        let sem = Arc::new(tokio::sync::Semaphore::new(DIFF_REFRESH_PARALLEL));
        let mut set = tokio::task::JoinSet::new();
        for ws in self.store.workspaces() {
            if ws.is_pr_chat_host() {
                continue;
            }
            let Ok(permit) = Arc::clone(&sem).acquire_owned().await else {
                break;
            };
            let e = self.arc();
            set.spawn(async move {
                e.sync_workspace_diff(&ws.id, true).await;
                drop(permit);
            });
        }
        while set.join_next().await.is_some() {}
    }

    /// Sets Workspace.Diff (and BehindBase) from git versus the base (HEAD
    /// for the main copy), publishing when `publish` and it changed. True
    /// when git produced a stat (even unchanged) or a newer sync superseded
    /// this one, so the caller must not fall back to cumulative counting.
    pub async fn sync_workspace_diff(&self, workspace_id: &str, publish: bool) -> bool {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return false;
        };
        if ws.is_pr_chat_host()
            || ws.worktree_path.is_empty()
            || !recur_git::is_repo(&ws.worktree_path).await
        {
            return false;
        }
        let mut base = ws.base_branch.trim().to_owned();
        if base.is_empty()
            && !ws.is_main
            && let Some(p) = self.store.project(&ws.project_id)
        {
            base = p.base_branch.trim().to_owned();
        }
        let generation = self.begin_diff_sync(workspace_id);

        let measured = tokio::time::timeout(Duration::from_secs(5), async {
            let (add, del) = recur_git::diff_stat_vs_base(&ws.worktree_path, &base).await?;
            let mut behind = 0;
            if !ws.is_main && !base.is_empty() {
                let mgr = recur_git::Manager::new(&ws.worktree_path, "");
                if let Ok(n) = mgr.commits_behind(&ws.worktree_path, &base).await {
                    behind = n;
                }
            }
            Ok::<_, recur_git::Error>((add, del, behind))
        })
        .await;
        let Ok(Ok((add, del, behind))) = measured else {
            return false;
        };

        let apply = self.diff_apply_lock(workspace_id);
        let _held = apply.lock().await;
        if self.diff_sync_stale(workspace_id, generation) {
            return true;
        }
        let Some(cur) = self.store.workspace(workspace_id) else {
            return false;
        };
        if cur.diff.add == add && cur.diff.del == del && cur.behind_base == behind {
            return true;
        }
        let Some(updated) = self.store.update_workspace(workspace_id, |w| {
            w.diff = DiffStat { add, del };
            w.behind_base = behind;
        }) else {
            return true;
        };
        if publish {
            self.publish_loop_updated(
                &updated.id,
                updated.state.clone(),
                &updated.phase,
                updated.iteration,
            );
        }
        true
    }

    fn begin_diff_sync(&self, workspace_id: &str) -> u64 {
        let mut ds = self.diff_sync.lock();
        let g = ds.generation.entry(workspace_id.to_owned()).or_insert(0);
        *g += 1;
        *g
    }

    fn diff_sync_stale(&self, workspace_id: &str, generation: u64) -> bool {
        self.diff_sync.lock().generation.get(workspace_id).copied() != Some(generation)
    }

    fn diff_apply_lock(&self, workspace_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.diff_sync
                .lock()
                .apply
                .entry(workspace_id.to_owned())
                .or_default(),
        )
    }
}
