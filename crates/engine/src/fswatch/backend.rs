//! Platform file-event streams (Go `fswatch_darwin.go`,
//! `fswatch_fsnotify.go`) behind one contract: batches of [`FsEvent`]s, the
//! first event after idle at once and bursts coalesced into one batch per
//! latency window.
//!
//! - macOS: one recursive FSEvents stream on the (symlink-resolved) root;
//!   MustScanSubDirs arrives as a rescan.
//! - Elsewhere: inotify is per directory, so every directory is watched,
//!   skipping `.git` internals (only `.git` itself, for HEAD) and
//!   `node_modules`. New directories are added (and files already written
//!   into them reported); moved or removed ones are unwatched. A queue
//!   overflow is a rescan of the root.
//!   simplify: the walk stops at the inotify watch limit and the rest of the
//!   tree relies on the fingerprint poll; raise fs.inotify.max_user_watches
//!   or move to fanotify if huge trees need live refresh.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::FsEvent;

/// A running stream; dropping it (or [`FsStream::stop`]) ends delivery.
pub struct FsStream {
    rx: mpsc::Receiver<Vec<FsEvent>>,
    stop: CancellationToken,
}

impl FsStream {
    /// The next batch (`None` once the stream ended).
    pub async fn recv(&mut self) -> Option<Vec<FsEvent>> {
        self.rx.recv().await
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }
}

impl Drop for FsStream {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// One raw notification from the platform watcher thread.
enum Raw {
    Event(notify::Event),
    Error,
}

/// Arms the platform stream on `root` (already symlink-resolved).
pub fn start_fs_watch(root: &Path, latency: Duration) -> Result<FsStream, String> {
    let (raw_tx, raw_rx) = mpsc::unbounded_channel::<Raw>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let _ = raw_tx.send(match res {
            Ok(ev) => Raw::Event(ev),
            Err(_) => Raw::Error,
        });
    })
    .map_err(|e| e.to_string())?;
    let mut tree = WatchTree {
        root: root.to_path_buf(),
        watched: Vec::new(),
    };
    if cfg!(target_os = "macos") {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| e.to_string())?;
    } else {
        tree.add(&mut watcher, root, None);
    }
    let (out_tx, out_rx) = mpsc::channel(1);
    let stop = CancellationToken::new();
    let rt = tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?;
    rt.spawn(batch(watcher, tree, raw_rx, out_tx, stop.clone(), latency));
    Ok(FsStream { rx: out_rx, stop })
}

/// The directories watched on per-directory backends.
struct WatchTree {
    root: PathBuf,
    watched: Vec<PathBuf>,
}

impl WatchTree {
    /// Watches `dir` and its subdirectories, reporting files it walks past
    /// into `on_file` (a new directory's files raised no event of their own).
    fn add(
        &mut self,
        w: &mut notify::RecommendedWatcher,
        dir: &Path,
        mut on_file: Option<&mut Vec<FsEvent>>,
    ) {
        let git_dir = self.root.join(".git");
        let mut stack = vec![dir.to_path_buf()];
        while let Some(p) = stack.pop() {
            let is_git = p == git_dir;
            if !is_git && super::fs_ignored(&self.root, &p) {
                continue;
            }
            match w.watch(&p, RecursiveMode::NonRecursive) {
                Ok(()) => self.watched.push(p.clone()),
                Err(e) => {
                    if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) {
                        tracing::warn!(
                            "recur: fs watch on {} hit the inotify watch limit; rest of the tree relies on the fingerprint poll",
                            self.root.display()
                        );
                        return;
                    }
                    continue;
                }
            }
            if is_git {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&p) else {
                continue;
            };
            let mut subdirs = Vec::new();
            for entry in rd.flatten() {
                let path = entry.path();
                match entry.file_type() {
                    Ok(t) if t.is_dir() => subdirs.push(path),
                    Ok(_) => {
                        if let Some(files) = on_file.as_deref_mut() {
                            files.push(FsEvent {
                                path,
                                rescan: false,
                            });
                        }
                    }
                    Err(_) => {}
                }
            }
            subdirs.sort();
            stack.extend(subdirs.into_iter().rev());
        }
    }

    /// Drops the watches on `dir` and everything under it (a moved
    /// directory keeps its watches under the old path otherwise).
    fn remove(&mut self, w: &mut notify::RecommendedWatcher, dir: &Path) {
        let (gone, keep): (Vec<PathBuf>, Vec<PathBuf>) = std::mem::take(&mut self.watched)
            .into_iter()
            .partition(|p| p == dir || p.starts_with(dir));
        self.watched = keep;
        for p in gone {
            let _ = w.unwatch(&p);
        }
    }
}

/// Folds one raw event into FsEvents, keeping per-directory watches in step
/// with creates, moves and removes.
fn fold(
    w: &mut notify::RecommendedWatcher,
    tree: &mut WatchTree,
    ev: notify::Event,
    pending: &mut Vec<FsEvent>,
) {
    if ev.flag() == Some(notify::event::Flag::Rescan) {
        pending.push(FsEvent {
            path: tree.root.clone(),
            rescan: true,
        });
        return;
    }
    // notify's inotify backend also watches opens and closes; reads are not
    // changes (Go's fsnotify never reported them), and git's own reads
    // would otherwise refresh forever.
    if matches!(ev.kind, EventKind::Access(_)) {
        return;
    }
    let per_dir = !cfg!(target_os = "macos");
    use notify::event::{ModifyKind, RenameMode};
    let (removed, created): (Vec<&PathBuf>, Vec<&PathBuf>) = match &ev.kind {
        EventKind::Remove(_) => (ev.paths.iter().collect(), Vec::new()),
        EventKind::Create(_) => (Vec::new(), ev.paths.iter().collect()),
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            (ev.paths.iter().collect(), Vec::new())
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            (Vec::new(), ev.paths.iter().collect())
        }
        // notify also sends the From and To halves on their own; the
        // paired event adds nothing to the watch tree.
        _ => (Vec::new(), Vec::new()),
    };
    if per_dir {
        for p in &removed {
            tree.remove(w, p);
        }
        for p in &created {
            if std::fs::metadata(p).is_ok_and(|m| m.is_dir()) {
                tree.add(w, p, Some(pending));
            }
        }
    }
    for p in ev.paths {
        pending.push(FsEvent {
            path: p,
            rescan: false,
        });
    }
}

/// The latency batcher (Go `startFSWatch`'s select loop).
async fn batch(
    mut w: notify::RecommendedWatcher,
    mut tree: WatchTree,
    mut raw: mpsc::UnboundedReceiver<Raw>,
    out: mpsc::Sender<Vec<FsEvent>>,
    stop: CancellationToken,
    latency: Duration,
) {
    let mut pending: Vec<FsEvent> = Vec::new();
    let mut hold: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    let mut sent: Option<tokio::time::Instant> = None;
    loop {
        let can_send = !pending.is_empty() && hold.is_none();
        tokio::select! {
            _ = stop.cancelled() => return,
            r = raw.recv() => {
                let Some(r) = r else { return };
                match r {
                    Raw::Event(ev) => fold(&mut w, &mut tree, ev, &mut pending),
                    Raw::Error => pending.push(FsEvent {
                        path: tree.root.clone(),
                        rescan: true,
                    }),
                }
                let since = sent.map_or(latency, |s| s.elapsed());
                if hold.is_none() && latency > since {
                    hold = Some(Box::pin(tokio::time::sleep(latency - since)));
                }
            }
            _ = async { hold.as_mut().expect("guarded").await }, if hold.is_some() => {
                hold = None;
            }
            permit = out.reserve(), if can_send => {
                let Ok(permit) = permit else { return };
                permit.send(std::mem::take(&mut pending));
                sent = Some(tokio::time::Instant::now());
            }
        }
    }
}
