//! The active-worktree watcher: exactly one workspace (the one on screen) is
//! watched, and `workspace.fs_changed` is the single refresh trigger every
//! surface reacts to.
//!
//! The fast path is the platform stream ([`backend`]): one recursive
//! FSEvents stream on macOS, per-directory inotify watches elsewhere,
//! batched 400 ms with the first event after idle delivered at once.
//! Events under `.git/` are noise, except `.git/HEAD` (a checkout), which
//! syncs the branch label at once. A 7 s fingerprint poll (`git status`,
//! HEAD, branch, @{upstream}, base and compare tips) is the backstop for
//! `.git`-only changes and the worktree-deleted detector. Non-git working
//! copies skip the fingerprint; a failed stream start degrades to the poll.

pub mod backend;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::bus::{EngineEvent, FsChanged, WorkspaceRef};

/// The watched workspace and the cancel of its watch task.
#[derive(Default)]
pub struct WatchState {
    pub cancel: Option<CancellationToken>,
    pub id: String,
}

/// One watcher notification: an absolute, symlink-resolved path, or a
/// rescan when the platform dropped events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEvent {
    pub path: PathBuf,
    pub rescan: bool,
}

fn rel_slash(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    Some(if s.is_empty() { ".".into() } else { s })
}

/// Stream noise: everything under `.git`, `.DS_Store`, `node_modules`.
pub fn fs_ignored(root: &Path, path: &Path) -> bool {
    let Some(rel) = rel_slash(root, path) else {
        return false;
    };
    if rel == ".git" || rel.starts_with(".git/") {
        return true;
    }
    if rel.ends_with("/.DS_Store") || rel == ".DS_Store" {
        return true;
    }
    format!("/{rel}/").contains("/node_modules/")
}

/// Whether `path` is the worktree's own `.git/HEAD` (a branch switch).
/// Linked worktrees keep HEAD outside the tree, so this only matches the
/// main working copy.
pub fn fs_is_head_change(root: &Path, path: &Path) -> bool {
    rel_slash(root, path).is_some_and(|r| r == ".git/HEAD")
}

async fn git_out(dir: &str, args: &[&str]) -> Option<Vec<u8>> {
    let (out, res) = recur_git::command(args.iter().copied())
        .current_dir(dir)
        .combined_output()
        .await;
    res.ok().map(|()| out)
}

/// The SHA for `rev`, or "" when missing. git's stderr never enters the
/// fingerprint (a no-upstream worktree would hash error text).
async fn git_rev_parse(dir: &str, rev: &str) -> String {
    git_out(dir, &["rev-parse", "--verify", "--quiet", rev])
        .await
        .map(|o| String::from_utf8_lossy(&o).trim().to_owned())
        .unwrap_or_default()
}

/// Hashes `git status --porcelain -z`, HEAD, the branch, @{upstream}, and
/// the base tip plus its compare ref's tip. Missing optional refs hash as
/// empty. `None` means git failed: keep the previous baseline.
pub async fn worktree_fingerprint(worktree: &str, base: &str) -> Option<[u8; 32]> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let status = git_out(worktree, &["status", "--porcelain", "-z"]).await?;
        let head = git_rev_parse(worktree, "HEAD").await;
        let branch = git_out(worktree, &["branch", "--show-current"])
            .await
            .map(|o| String::from_utf8_lossy(&o).trim().to_owned())
            .unwrap_or_default();
        let upstream = git_rev_parse(worktree, "@{upstream}").await;
        let base = base.trim();
        let (mut base_tip, mut compare_tip) = (String::new(), String::new());
        if !base.is_empty() {
            base_tip = git_rev_parse(worktree, base).await;
            let compare = recur_git::Manager::new(worktree, "")
                .compare_ref(base)
                .await;
            if !compare.is_empty() && compare != base {
                compare_tip = git_rev_parse(worktree, &compare).await;
            }
        }
        let mut h = Sha256::new();
        h.update(&status);
        for part in [&head, &branch, &upstream, &base_tip, &compare_tip] {
            h.update(b"\x00");
            h.update(part.as_bytes());
        }
        Some(h.finalize().into())
    })
    .await
    .ok()
    .flatten()
}

impl Engine {
    /// The workspace armed for watching, or "".
    pub fn watched_workspace(&self) -> String {
        self.watch.lock().id.clone()
    }

    /// Makes `id` the single watched workspace (an empty id stops
    /// watching). Re-watching the same id restarts the stream.
    pub fn watch_workspace(&self, id: &str) {
        let mut w = self.watch.lock();
        if let Some(c) = w.cancel.take() {
            c.cancel();
        }
        w.id = id.to_owned();
        // Published under the watch lock so subscribers see watches in the
        // order they took effect (the file-index hub pins on this).
        self.bus
            .publish(EngineEvent::WorkspaceWatched(WorkspaceRef::new(id)));
        if id.is_empty() {
            // Leaving the Observatory: no chat tab is on screen anymore.
            self.set_workspace_chat_focus("", "");
            return;
        }
        // Switching workspaces drops the previous focus so a background
        // finish there can raise unseen until the UI re-focuses.
        self.reset_chat_focus_unless(id);
        let Some(ws) = self.store.workspace(id) else {
            return;
        };
        if ws.worktree_path.is_empty() {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let cancel = CancellationToken::new();
        w.cancel = Some(cancel.clone());
        drop(w);
        let e = self.arc();
        let id = id.to_owned();
        rt.spawn(async move { e.watch_loop(cancel, id, ws.worktree_path).await });
    }

    /// The merge-base ref the titlebar compares against ("" when gone).
    fn workspace_base_name(&self, id: &str) -> String {
        let Some(ws) = self.store.workspace(id) else {
            return String::new();
        };
        let base = ws.base_branch.trim();
        if !base.is_empty() {
            return base.to_owned();
        }
        self.store
            .project(&ws.project_id)
            .map(|p| p.base_branch.trim().to_owned())
            .unwrap_or_default()
    }

    fn publish_fs_changed(&self, id: &str, rels: Vec<String>, rescan: bool) {
        self.bus.publish(EngineEvent::WorkspaceFsChanged(FsChanged {
            paths: rels,
            rescan: Some(rescan),
            workspace_id: id.to_owned(),
        }));
    }

    async fn watch_loop(self: Arc<Self>, cancel: CancellationToken, id: String, worktree: String) {
        // Watch and filter on the resolved root (/var → /private/var):
        // FSEvents drops modifies on an unresolved path.
        let watch_root =
            std::fs::canonicalize(&worktree).unwrap_or_else(|_| PathBuf::from(&worktree));
        let ignore_root = watch_root.clone();
        let (latency, poll_every) = {
            let h = self.hooks.lock();
            (h.fs_event_latency, h.fs_poll_backstop)
        };
        let mut stream = match backend::start_fs_watch(&watch_root, latency) {
            Ok(s) => Some(s),
            Err(err) => {
                tracing::warn!(
                    "recur: fs watch on {} failed ({err}) — falling back to {:?} fingerprint poll",
                    watch_root.display(),
                    poll_every
                );
                None
            }
        };
        // Classify once so a transient git failure cannot flap the
        // fingerprint through a "not git" state.
        let is_git = recur_git::is_repo(&worktree).await;
        let mut last: Option<[u8; 32]> = None;
        let mut poll =
            tokio::time::interval_at(tokio::time::Instant::now() + poll_every, poll_every);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            let next_batch = async {
                match stream.as_mut() {
                    Some(s) => s.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => return,
                batch = next_batch => {
                    let Some(batch) = batch else {
                        stream = None;
                        continue;
                    };
                    let (mut refresh, mut head_changed, mut rescan) = (false, false, false);
                    let mut rels: Vec<String> = Vec::new();
                    for ev in &batch {
                        if ev.rescan {
                            // Dropped events may include the HEAD rewrite of a
                            // churny checkout: sync the branch too.
                            refresh = true;
                            head_changed = true;
                            rescan = true;
                        } else if fs_is_head_change(&ignore_root, &ev.path) {
                            refresh = true;
                            head_changed = true;
                        } else if !fs_ignored(&ignore_root, &ev.path) {
                            refresh = true;
                            if let Some(rel) = rel_slash(&ignore_root, &ev.path)
                                && rel != "."
                                && !rels.contains(&rel)
                            {
                                rels.push(rel);
                            }
                        }
                    }
                    if head_changed {
                        self.sync_workspace_branch(&id).await;
                    }
                    if refresh {
                        self.schedule_workspace_diff_sync(&id);
                        self.publish_fs_changed(&id, rels, rescan);
                    }
                }
                _ = poll.tick() => {
                    if !std::fs::metadata(&worktree).is_ok_and(|m| m.is_dir()) {
                        return;
                    }
                    if !is_git {
                        continue;
                    }
                    let base = self.workspace_base_name(&id);
                    let Some(fp) = worktree_fingerprint(&worktree, &base).await else {
                        continue;
                    };
                    if last.is_some_and(|l| l != fp) {
                        // Branch renames do not touch the tree; sync first.
                        self.sync_workspace_branch(&id).await;
                        self.schedule_workspace_diff_sync(&id);
                        self.publish_fs_changed(&id, Vec::new(), true);
                    }
                    last = Some(fp);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
