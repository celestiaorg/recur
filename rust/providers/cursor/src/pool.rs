//! Warm bridges, one per worktree (the Cursor half of Go
//! `engine/provider.go`). A single shared bridge with the GUI's cwd "/"
//! leaves agent shells at disk root even with `local.cwd` set, so each
//! worktree gets its own `--workspace` bridge, multiplexing that worktree's
//! agents by agent id.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use recur_domain::{PROVIDER_CURSOR, Settings};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::attribution::{attribution_config_path, ensure_attribution_patch, write_attribution_config};
use crate::bridge::{Bridge, BridgeConfig};
use crate::locate::{abs, clean, locate, owned_bridge_root};
use crate::runner::CursorRunner;

/// Bounds the lazy bridge start.
const BRIDGE_START_TIMEOUT: Duration = Duration::from_secs(90);

/// How many idle bridges stay warm. A new worktree evicts the least recently
/// used idle one; in-use bridges are kept even over the cap.
pub(crate) const MAX_CURSOR_BRIDGES: usize = 8;

pub(crate) struct Entry {
    pub(crate) bridge: Option<Arc<Bridge>>,
    pub(crate) runner: Option<Arc<CursorRunner>>,
    pub(crate) last_used: Instant,
}

impl Entry {
    fn busy(&self) -> bool {
        self.runner
            .as_ref()
            .is_some_and(|r| r.in_use.load(Ordering::SeqCst) > 0)
    }
}

#[derive(Default)]
pub(crate) struct PoolState {
    /// Keyed by cleaned absolute worktree path.
    pub(crate) entries: HashMap<PathBuf, Entry>,
    /// What the live bridges were started with: a key or binary change tears
    /// the pool down.
    key: String,
    path: PathBuf,
    /// Close once in-flight turns finish (Cursor turned off mid-turn).
    pub(crate) close_when_idle: bool,
}

pub(crate) struct Pool {
    pub(crate) state: Mutex<PoolState>,
    data_dir: PathBuf,
    logs_dir: PathBuf,
    /// The home holding `~/.cursor/projects` (`None` = `$HOME`).
    home: Option<PathBuf>,
}

/// An error that keeps the turn from starting, with Go's wording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoolError(pub String);

impl Pool {
    pub(crate) fn new(data_dir: PathBuf, logs_dir: PathBuf, home: Option<PathBuf>) -> Self {
        Self {
            state: Mutex::new(PoolState::default()),
            data_dir,
            logs_dir,
            home,
        }
    }

    /// A runner backed by a live bridge for `worktree`, starting or
    /// restarting that worktree's bridge when needed. The runner is reserved
    /// before the lock drops; dropping the reservation releases it.
    pub(crate) async fn ensure_runner(
        self: &Arc<Self>,
        cancel: &CancellationToken,
        set: &Settings,
        worktree: &Path,
    ) -> Result<Reservation, PoolError> {
        let mut p = self.state.lock().await;

        if !set.provider_enabled(PROVIDER_CURSOR) {
            return Err(PoolError("Cursor is turned off in Settings → Providers".into()));
        }
        if !set.cursor_configured() {
            return Err(PoolError(
                "a role is set to a Cursor model but Cursor is not connected (Settings → Providers → Connect)".into(),
            ));
        }
        let wt = normalize_cursor_worktree(worktree)?;
        let bin = locate(&self.data_dir).map_err(PoolError)?;
        let attr_path = attribution_config_path(&self.data_dir);

        // A key or binary change: the old processes run with the previous
        // credential or binary.
        if (!p.key.is_empty() || !p.path.as_os_str().is_empty()) && (p.key != set.cursor_api_key || p.path != bin) {
            close_all_locked(&mut p).await;
        }

        if let Some(e) = p.entries.get_mut(&wt)
            && e.bridge.as_ref().is_some_and(|b| b.alive())
            && let Some(r) = e.runner.clone()
        {
            r.strict_config.store(set.cursor_strict_config, Ordering::SeqCst);
            e.last_used = Instant::now();
            p.key = set.cursor_api_key.clone();
            p.path = bin;
            return Ok(Reservation::new(self.clone(), r));
        }
        // Dead or missing: close just that slot.
        close_entry_locked(&mut p, &wt).await;
        evict_locked(&mut p, MAX_CURSOR_BRIDGES.saturating_sub(1)).await;

        // Cold start only: patch Recur's own bridge (Install patches too).
        // An unrecognized SDK cannot stub getAgentSkills: fail closed.
        let Some(root) = owned_bridge_root(&self.data_dir, &bin) else {
            return Err(PoolError(
                "cursor bridge is not installed in the Recur data dir — install it from Settings".into(),
            ));
        };
        ensure_attribution_patch(&root)
            .map_err(|e| PoolError(format!("cursor bridge patch failed; reinstall from Settings: {e}")))?;
        if let Some(path) = &attr_path {
            write_attribution_config(path, set.cursor_attribute_commits, set.cursor_attribute_prs)
                .map_err(|e| PoolError(format!("cursor attribution config: {e}")))?;
        }

        // Per-worktree log and pidfile so concurrent bridges do not clobber
        // one pidfile.
        let log_dir = if self.logs_dir.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            let dir = self.logs_dir.clone();
            match std::fs::create_dir_all(&dir) {
                Ok(()) => dir.join("cursor-bridges").join(worktree_log_key(&wt)),
                Err(_) => PathBuf::new(),
            }
        };
        let cfg = BridgeConfig {
            bin: bin.clone(),
            api_key: set.cursor_api_key.clone(),
            workspace: wt.clone(),
            log_dir,
            attribution_config_path: attr_path.unwrap_or_default(),
            home: self.home.clone(),
        };
        let start_tok = cancel.child_token();
        let started = tokio::time::timeout(BRIDGE_START_TIMEOUT, Bridge::start(cfg, &start_tok)).await;
        let b = match started {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => return Err(PoolError(format!("start cursor bridge: {e}"))),
            Err(_) => {
                start_tok.cancel();
                return Err(PoolError("start cursor bridge: context deadline exceeded".into()));
            }
        };

        // Fail fast and by name when the bridge is too old.
        let Some(cl) = b.connect_client() else {
            b.close().await;
            return Err(PoolError("cursor bridge handshake: bridge not started".into()));
        };
        let v = match tokio::time::timeout(BRIDGE_START_TIMEOUT, cl.get_version(cancel)).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                b.close().await;
                return Err(PoolError(format!("cursor bridge handshake: {e}")));
            }
            Err(_) => {
                b.close().await;
                return Err(PoolError("cursor bridge handshake: context deadline exceeded".into()));
            }
        };
        for want in ["agent.create", "agent.send"] {
            if !v.has(want) {
                b.close().await;
                return Err(PoolError(format!(
                    "cursor bridge {} lacks capability {want:?} — update it from Settings",
                    v.bridge_version
                )));
            }
        }

        let mut r = CursorRunner::new(b.clone(), &set.cursor_api_key);
        r.strict_config.store(set.cursor_strict_config, Ordering::SeqCst);
        r.skip_observe = !v.has("run.observe");
        let r = Arc::new(r);
        p.entries.insert(
            wt,
            Entry {
                bridge: Some(b),
                runner: Some(r.clone()),
                last_used: Instant::now(),
            },
        );
        p.key = set.cursor_api_key.clone();
        p.path = bin;
        Ok(Reservation::new(self.clone(), r))
    }

    /// Drops the bridge for `worktree` (or the whole pool for an empty path)
    /// so the next turn starts fresh: used when the process is gone but not
    /// reaped yet, and before a worktree is removed.
    pub(crate) async fn invalidate(&self, worktree: &Path) {
        let mut p = self.state.lock().await;
        if crate::gojson::trim(&worktree.to_string_lossy()).is_empty() {
            close_all_locked(&mut p).await;
            return;
        }
        // The path may already be gone after archive/delete: still drop the
        // cleaned absolute key.
        let wt = normalize_cursor_worktree(worktree).unwrap_or_else(|_| {
            let c = clean(worktree);
            abs(&c).unwrap_or(c)
        });
        close_entry_locked(&mut p, &wt).await;
    }

    /// Stops every bridge.
    pub(crate) async fn close(&self) {
        let mut p = self.state.lock().await;
        p.close_when_idle = false;
        close_all_locked(&mut p).await;
    }

    /// Turning Cursor off mid-turn: keep bridges with a turn in flight until
    /// it finishes.
    pub(crate) async fn close_bridges(&self) {
        let mut p = self.state.lock().await;
        if p.entries.values().any(Entry::busy) {
            p.close_when_idle = true;
            return;
        }
        p.close_when_idle = false;
        close_all_locked(&mut p).await;
    }

    pub(crate) async fn cancel_close_when_idle(&self) {
        self.state.lock().await.close_when_idle = false;
    }

    pub(crate) async fn close_if_idle_requested(&self, idle: bool) {
        let mut p = self.state.lock().await;
        if !p.close_when_idle || !idle {
            return;
        }
        p.close_when_idle = false;
        close_all_locked(&mut p).await;
    }

    /// Trims idle overflow once a runner's last handout returns.
    async fn trim(&self) {
        let mut p = self.state.lock().await;
        evict_locked(&mut p, MAX_CURSOR_BRIDGES).await;
    }
}

/// One `ensure_runner` handout. Dropping it releases the runner; the last
/// release trims idle overflow above the cap.
pub(crate) struct Reservation {
    pool: Arc<Pool>,
    pub(crate) runner: Arc<CursorRunner>,
}

impl Reservation {
    fn new(pool: Arc<Pool>, runner: Arc<CursorRunner>) -> Self {
        runner.in_use.fetch_add(1, Ordering::SeqCst);
        Self { pool, runner }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if release(&self.runner)
            && let Ok(h) = tokio::runtime::Handle::try_current()
        {
            let pool = self.pool.clone();
            h.spawn(async move { pool.trim().await });
        }
    }
}

/// Drops one handout; true when it was the last.
pub(crate) fn release(r: &CursorRunner) -> bool {
    r.in_use
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n > 0).then(|| n - 1))
        .is_ok_and(|prev| prev == 1)
}

/// Requires an existing absolute directory: an empty path used to start a
/// bridge at "/".
pub(crate) fn normalize_cursor_worktree(worktree: &Path) -> Result<PathBuf, PoolError> {
    let s = worktree.to_string_lossy();
    let trimmed = crate::gojson::trim(&s);
    let required = || PoolError("cursor bridge: worktree path is required (refusing to start at /)".into());
    if trimmed.is_empty() {
        return Err(required());
    }
    let a = abs(Path::new(trimmed))
        .map_err(|e| PoolError(format!("cursor bridge: worktree {trimmed:?}: {e}")))?;
    if a == Path::new("/") {
        return Err(required());
    }
    if !std::fs::metadata(&a).is_ok_and(|m| m.is_dir()) {
        return Err(PoolError(format!(
            "cursor bridge: worktree {:?} is unavailable",
            a.display().to_string()
        )));
    }
    Ok(a)
}

/// A short stable directory name for per-bridge logs and pidfiles.
pub(crate) fn worktree_log_key(worktree: &Path) -> String {
    let sum = Sha256::digest(worktree.to_string_lossy().as_bytes());
    sum[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Closes least-recently-used idle entries until at most `max` remain. Busy
/// bridges are skipped so a long turn is never killed by another worktree's
/// cold start.
pub(crate) async fn evict_locked(p: &mut PoolState, max: usize) {
    while p.entries.len() > max {
        let Some(victim) = lru_idle(p) else {
            return;
        };
        close_entry_locked(p, &victim).await;
    }
}

fn lru_idle(p: &PoolState) -> Option<PathBuf> {
    p.entries
        .iter()
        .filter(|(_, e)| !e.busy())
        .min_by_key(|(_, e)| e.last_used)
        .map(|(k, _)| k.clone())
}

async fn close_entry_locked(p: &mut PoolState, worktree: &Path) {
    if let Some(e) = p.entries.remove(worktree)
        && let Some(b) = e.bridge
    {
        b.close().await;
    }
}

async fn close_all_locked(p: &mut PoolState) {
    let keys: Vec<PathBuf> = p.entries.keys().cloned().collect();
    for k in keys {
        close_entry_locked(p, &k).await;
    }
    p.key.clear();
    p.path = PathBuf::new();
}

#[cfg(test)]
mod tests;
