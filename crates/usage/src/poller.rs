//! The cached snapshot and its conservative refresh.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::ctx::{Ctx, CtxError};
use crate::cursor::oauth::revoke_cursor_user_api_key;
use crate::cursor::session::{cursor_auth_path, read_cursor_auth_file, remove_cursor_auth_file};
use crate::fetcher::Fetcher;
use crate::http::is_reachability_error;
use crate::types::{
    CursorUsageConnect, PROVIDER_CURSOR, PROVIDER_USAGE_EVENT, ProviderQuota, Snapshot,
    empty_snapshot,
};

pub const POLL_INTERVAL: Duration = Duration::from_secs(3 * 60);
pub const FRESH_WINDOW_MS: i64 = 60_000;
pub const COLLECT_TIMEOUT: Duration = Duration::from_secs(25);

/// Receives `(event name, snapshot)`: the engine bus, or a test fake.
pub type Publisher = Arc<dyn Fn(&'static str, Snapshot) + Send + Sync>;

/// Caches the last snapshot and refreshes on a conservative interval. The
/// ticker starts with the first read so launch does not touch provider
/// keychains until the UI asks.
pub struct Poller {
    fetch: Arc<Fetcher>,
    publish: Option<Publisher>,
    stop: CancellationToken,
    state: Mutex<State>,
}

struct State {
    last: Snapshot,
    has: bool,
    stopped: bool,
    looping: bool,
    refreshing: Option<watch::Receiver<Option<Snapshot>>>,
}

impl Poller {
    pub fn new(fetch: Arc<Fetcher>, publish: Option<Publisher>) -> Arc<Self> {
        Arc::new(Self {
            fetch,
            publish,
            stop: CancellationToken::new(),
            state: Mutex::new(State {
                last: empty_snapshot(),
                has: false,
                stopped: false,
                looping: false,
                refreshing: None,
            }),
        })
    }

    pub fn fetcher(&self) -> &Arc<Fetcher> {
        &self.fetch
    }

    /// Stops the ticker and any in-flight Cursor Connect.
    pub fn stop(&self) {
        {
            let mut st = self.state.lock();
            if st.stopped {
                return;
            }
            st.stopped = true;
        }
        self.stop.cancel();
        self.fetch.cancel_connect();
    }

    fn kick_loop(self: &Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.looping || st.stopped {
                return;
            }
            st.looping = true;
        }
        let p = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval_at(
                tokio::time::Instant::now() + POLL_INTERVAL,
                POLL_INTERVAL,
            );
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = p.stop.cancelled() => return,
                    _ = tick.tick() => {
                        p.refresh().await;
                    }
                }
            }
        });
    }

    /// The last result, refreshed when older than a minute. `force` bypasses
    /// the cache but still joins an in-flight collect.
    pub async fn snapshot(self: &Arc<Self>, force: bool) -> Snapshot {
        self.kick_loop();
        let (last, has) = {
            let st = self.state.lock();
            (st.last.clone(), st.has)
        };
        if !force && has && self.fetch.now_ms() - last.fetched_at < FRESH_WINDOW_MS {
            return last;
        }
        self.refresh().await
    }

    /// Collects now, or waits for the collect already running.
    pub async fn refresh(self: &Arc<Self>) -> Snapshot {
        self.kick_loop();
        let joined = {
            let mut st = self.state.lock();
            match st.refreshing.clone() {
                Some(rx) => Err(rx),
                None => {
                    let (tx, rx) = watch::channel(None);
                    st.refreshing = Some(rx);
                    Ok(tx)
                }
            }
        };
        let tx = match joined {
            Ok(tx) => tx,
            Err(mut rx) => {
                return rx
                    .wait_for(Option::is_some)
                    .await
                    .map(|s| s.clone().unwrap_or_default())
                    .unwrap_or_default();
            }
        };
        // Clears the in-flight call even when collect panics, so later
        // refreshes do not wait forever.
        let guard = RefreshGuard {
            poller: self,
            tx: Some(tx),
        };
        let ctx = Ctx::background().with_timeout(COLLECT_TIMEOUT);
        let next = self.fetch.collect(&ctx).await;
        let merged = {
            let mut st = self.state.lock();
            let merged = merge_stale(&st.last, next);
            st.last = merged.clone();
            st.has = true;
            merged
        };
        guard.finish(merged.clone());
        if let Some(p) = &self.publish {
            p(PROVIDER_USAGE_EVENT, merged.clone());
        }
        merged
    }

    /// Starts the Cursor browser login; the footer shows a connecting row
    /// until it finishes.
    pub fn connect_cursor(self: &Arc<Self>) -> CursorUsageConnect {
        self.kick_loop();
        let p = Arc::clone(self);
        let conn = self.fetch.start_connect(move |res| match res {
            Ok(()) => {
                tokio::spawn(async move {
                    p.refresh().await;
                });
            }
            Err(CtxError::Canceled) => {}
            Err(CtxError::DeadlineExceeded) => {
                p.mark_cursor_connect_failed("Sign-in timed out: try Connect again");
            }
        });
        if conn.pending {
            self.mark_cursor_connecting(true);
        }
        conn
    }

    pub fn cancel_connect(self: &Arc<Self>) {
        self.fetch.cancel_connect();
        self.mark_cursor_connecting(false);
        let p = Arc::clone(self);
        tokio::spawn(async move {
            p.refresh().await;
        });
    }

    pub(crate) fn mark_cursor_connecting(&self, on: bool) {
        let s = {
            let mut st = self.state.lock();
            let mut s = st.last.clone();
            let providers = s.providers.get_or_insert_with(Vec::new);
            let mut found = false;
            for q in providers.iter_mut().filter(|q| q.id == PROVIDER_CURSOR) {
                q.connecting = on;
                if on {
                    q.hint = "Waiting for browser sign-in…".into();
                    q.error.clear();
                } else {
                    q.hint.clear();
                }
                found = true;
            }
            if !found && on {
                providers.push(ProviderQuota {
                    id: PROVIDER_CURSOR.into(),
                    name: "Cursor".into(),
                    connecting: true,
                    hint: "Waiting for browser sign-in…".into(),
                    ..Default::default()
                });
            }
            st.last = s.clone();
            st.has = true;
            s
        };
        if let Some(p) = &self.publish {
            p(PROVIDER_USAGE_EVENT, s);
        }
    }

    pub(crate) fn mark_cursor_connect_failed(&self, msg: &str) {
        let s = {
            let mut st = self.state.lock();
            let mut s = st.last.clone();
            let providers = s.providers.get_or_insert_with(Vec::new);
            let mut found = false;
            for q in providers.iter_mut().filter(|q| q.id == PROVIDER_CURSOR) {
                q.connecting = false;
                q.error = "Cursor sign-in did not finish".into();
                q.hint = msg.into();
                found = true;
            }
            if !found {
                providers.push(ProviderQuota {
                    id: PROVIDER_CURSOR.into(),
                    name: "Cursor".into(),
                    error: "Cursor sign-in did not finish".into(),
                    hint: msg.into(),
                    ..Default::default()
                });
            }
            st.last = s.clone();
            st.has = true;
            s
        };
        if let Some(p) = &self.publish {
            p(PROVIDER_USAGE_EVENT, s);
        }
    }

    /// Revokes the minted dashboard key, deletes the Connect tokens and drops
    /// the Cursor row.
    pub async fn disconnect_cursor(self: &Arc<Self>) -> std::io::Result<()> {
        self.fetch.cancel_connect();
        let Some(dir) = self.fetch.data_dir() else {
            return Ok(());
        };
        let path = cursor_auth_path(&dir);
        if let Ok(tok) = read_cursor_auth_file(&path)
            && !tok.api_key.is_empty()
            && !tok.access.is_empty()
        {
            let _ = revoke_cursor_user_api_key(
                &Ctx::background(),
                &self.fetch,
                &tok.access,
                &tok.api_key,
                tok.api_key_id,
            )
            .await;
        }
        let err = remove_cursor_auth_file(&path);
        self.forget_provider(PROVIDER_CURSOR);
        let p = Arc::clone(self);
        tokio::spawn(async move {
            p.refresh().await;
        });
        err
    }

    pub(crate) fn forget_provider(&self, id: &str) {
        let mut st = self.state.lock();
        if let Some(providers) = &mut st.last.providers {
            providers.retain(|q| q.id != id);
        }
    }

    #[cfg(test)]
    pub(crate) fn last(&self) -> Snapshot {
        self.state.lock().last.clone()
    }

    #[cfg(test)]
    pub(crate) fn set_last(&self, s: Snapshot) {
        let mut st = self.state.lock();
        st.last = s;
        st.has = true;
    }
}

struct RefreshGuard<'a> {
    poller: &'a Poller,
    tx: Option<watch::Sender<Option<Snapshot>>>,
}

impl RefreshGuard<'_> {
    fn finish(mut self, snap: Snapshot) {
        if let Some(tx) = self.tx.take() {
            self.poller.state.lock().refreshing = None;
            let _ = tx.send(Some(snap));
        }
    }
}

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            self.poller.state.lock().refreshing = None;
            let _ = tx.send(Some(Snapshot::default()));
        }
    }
}

/// Keeps last-known windows when a provider was rate limited or unreachable
/// (marking the snapshot stale), and keeps rows with windows for providers
/// not fetched this pass. `fetched_at` stays on the attempt so a 429 cannot
/// freeze the freshness gate.
pub fn merge_stale(prev: &Snapshot, next: Snapshot) -> Snapshot {
    let by_id: std::collections::HashMap<&str, &ProviderQuota> = prev
        .providers
        .iter()
        .flatten()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let mut out = next;
    out.stale = false;
    if let Some(providers) = &mut out.providers {
        for q in providers.iter_mut() {
            let old = by_id.get(q.id.as_str()).filter(|o| !o.windows.is_empty());
            if q.error == "rate_limited" {
                if let Some(old) = old {
                    let mut old = (*old).clone();
                    old.error.clear();
                    *q = old;
                    out.stale = true;
                    continue;
                }
                q.error = "Usage endpoint is rate limited".into();
                q.hint = "Showing nothing until the next successful refresh.".into();
            } else if is_reachability_error(&q.error)
                && q.windows.is_empty()
                && let Some(old) = old
            {
                let mut old = (*old).clone();
                old.error.clear();
                *q = old;
                out.stale = true;
            }
        }
    }
    let have: std::collections::HashSet<String> = out
        .providers
        .iter()
        .flatten()
        .map(|q| q.id.clone())
        .collect();
    let carried: Vec<ProviderQuota> = prev
        .providers
        .iter()
        .flatten()
        .filter(|o| !have.contains(&o.id) && !o.windows.is_empty())
        .cloned()
        .collect();
    if !carried.is_empty() {
        out.providers.get_or_insert_with(Vec::new).extend(carried);
    }
    out
}
