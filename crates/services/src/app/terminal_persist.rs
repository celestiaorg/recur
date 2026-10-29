//! Terminal tab persistence: each workspace's tabs and dock layout as
//! `<data>/workspaces/<id>/terminals.json.gz`, written on a trailing debounce
//! capped at five seconds and flushed on quit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use super::AppServices;
use super::editor_session::{read_gz_json, write_gz_json};
use crate::api::TerminalDockChrome;

pub(crate) const TERMINAL_PERSIST_FILE: &str = "terminals.json.gz";
const TERMINAL_PERSIST_DEBOUNCE: Duration = Duration::from_secs(1);
const TERMINAL_PERSIST_MAX_AGE: Duration = Duration::from_secs(5);
pub(crate) const TERMINAL_PERSIST_MODE: u32 = 0o600;

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// One workspace's snapshot, dock layout included, persisted with the
/// workspace like the editor session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct PersistedTerminals {
    #[serde(skip_serializing_if = "is_false")]
    pub open: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub maximized: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub editor_hidden: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub beside: bool,
    #[serde(skip_serializing_if = "is_zero")]
    pub beside_pct: i64,
    pub active: i64,
    /// `None` is `null`.
    pub tabs: Option<Vec<PersistedTab>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PersistedTab {
    pub title: String,
    pub cwd: String,
    /// Base64 in JSON, omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty", with = "b64")]
    pub replay: Vec<u8>,
}

mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = Option::<String>::deserialize(d)?.unwrap_or_default();
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
}

/// The dock-layout prefix of the snapshot; decoding it skips the tabs so
/// listing layouts does not allocate scrollback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PersistedChrome {
    open: bool,
    maximized: bool,
    editor_hidden: bool,
    beside: bool,
    beside_pct: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Chrome {
    pub open: bool,
    pub maximized: bool,
    pub editor_hidden: bool,
    pub beside: bool,
    pub beside_pct: i64,
}

impl Chrome {
    pub fn idle(&self) -> bool {
        !self.open && !self.maximized && !self.editor_hidden && !self.beside && self.beside_pct == 0
    }

    /// A closed dock is never maximized or covering the editor; the split is
    /// 20–80% (0 = unset).
    pub fn clamped(mut self) -> Self {
        if !self.open {
            self.maximized = false;
            self.editor_hidden = false;
        }
        self.beside_pct = match self.beside_pct {
            n if n <= 0 => 0,
            n => n.clamp(20, 80),
        };
        self
    }

    fn of(c: &PersistedChrome) -> Self {
        Self {
            open: c.open,
            maximized: c.maximized,
            editor_hidden: c.editor_hidden,
            beside: c.beside,
            beside_pct: c.beside_pct,
        }
    }
}

impl From<&TerminalDockChrome> for Chrome {
    fn from(c: &TerminalDockChrome) -> Self {
        Self {
            open: c.open,
            maximized: c.maximized,
            editor_hidden: c.editor_hidden,
            beside: c.beside,
            beside_pct: c.beside_pct,
        }
    }
}

fn snapshot_idle(s: &PersistedTerminals) -> bool {
    s.tabs.as_ref().is_none_or(Vec::is_empty)
        && Chrome {
            open: s.open,
            maximized: s.maximized,
            editor_hidden: s.editor_hidden,
            beside: s.beside,
            beside_pct: s.beside_pct,
        }
        .idle()
}

/// How long until the debounced write: `debounce`, cut short so a write
/// lands within `max_age` of the first unsaved change.
pub fn persist_delay(
    now: Instant,
    first_dirty: Option<Instant>,
    debounce: Duration,
    max_age: Duration,
) -> Duration {
    let Some(first) = first_dirty else {
        return debounce;
    };
    let age = now.saturating_duration_since(first);
    max_age.saturating_sub(age).min(debounce)
}

pub(crate) fn snapshot_path(root: &Path, workspace_id: &str) -> PathBuf {
    root.join("workspaces")
        .join(workspace_id)
        .join(TERMINAL_PERSIST_FILE)
}

#[derive(Default)]
struct Inner {
    dirty: HashSet<String>,
    timer: Option<JoinHandle<()>>,
    first: Option<Instant>,
    closed: bool,
    chrome: HashMap<String, Chrome>,
}

/// The debounce state.
pub(crate) struct Persist {
    me: Weak<AppServices>,
    handle: tokio::runtime::Handle,
    inner: Mutex<Inner>,
}

impl Persist {
    pub(crate) fn new(me: Weak<AppServices>) -> Self {
        Self {
            me,
            handle: tokio::runtime::Handle::current(),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Schedules a write of `workspace_id`'s tabs. Called from the terminal
    /// manager's dirty hook on any thread.
    pub(crate) fn mark_dirty(&self, workspace_id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        let mut st = self.inner.lock();
        if st.closed {
            return;
        }
        let now = Instant::now();
        let first = *st.first.get_or_insert(now);
        st.dirty.insert(workspace_id.to_owned());
        let delay = persist_delay(
            now,
            Some(first),
            TERMINAL_PERSIST_DEBOUNCE,
            TERMINAL_PERSIST_MAX_AGE,
        );
        if let Some(t) = st.timer.take() {
            t.abort();
        }
        let me = self.me.clone();
        st.timer = Some(self.handle.spawn(async move {
            tokio::time::sleep(delay).await;
            tokio::task::spawn_blocking(move || {
                if let Some(s) = me.upgrade() {
                    s.term_persist.flush(&s);
                }
            });
        }));
    }

    pub(crate) fn forget(&self, workspace_id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        let mut st = self.inner.lock();
        st.dirty.remove(workspace_id);
        st.chrome.remove(workspace_id);
    }

    /// Writes every dirty workspace (the debounce timer and shutdown). A
    /// failed write is retried on the next debounce.
    pub(crate) fn flush(&self, s: &AppServices) {
        let ids: Vec<String> = {
            let mut st = self.inner.lock();
            // The timer is either this caller or already finished: drop it.
            st.timer = None;
            st.first = None;
            st.dirty.drain().collect()
        };
        for id in ids {
            if let Err(e) = s.persist_workspace_terminals(&id) {
                tracing::warn!("terminal: persist {id}: {e}");
                self.mark_dirty(&id);
            }
        }
    }

    /// Stops accepting writes, cancelling the timer.
    pub(crate) fn close(&self) {
        let mut st = self.inner.lock();
        st.closed = true;
        if let Some(t) = st.timer.take() {
            t.abort();
        }
    }

    fn closed(&self) -> bool {
        self.inner.lock().closed
    }

    fn cached_chrome(&self, workspace_id: &str) -> Option<Chrome> {
        self.inner.lock().chrome.get(workspace_id).copied()
    }

    fn cache_chrome(&self, workspace_id: &str, c: Chrome, overwrite: bool) -> Chrome {
        let mut st = self.inner.lock();
        if !overwrite && let Some(cached) = st.chrome.get(workspace_id) {
            return *cached;
        }
        st.chrome.insert(workspace_id.to_owned(), c);
        c
    }
}

impl AppServices {
    pub(crate) fn read_terminal_snapshot(&self, workspace_id: &str) -> Option<PersistedTerminals> {
        if workspace_id.is_empty() {
            return None;
        }
        let snap: PersistedTerminals = read_gz_json(&snapshot_path(self.data_dir(), workspace_id))?;
        (!snapshot_idle(&snap)).then_some(snap)
    }

    pub(crate) fn write_terminal_snapshot(
        &self,
        workspace_id: &str,
        snap: &PersistedTerminals,
    ) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = recur_store::paths::workspace_dir(self.data_dir(), workspace_id)?;
        let path = write_gz_json(&dir, ".terminals-", TERMINAL_PERSIST_FILE, snap)?;
        std::fs::set_permissions(
            &path,
            std::fs::Permissions::from_mode(TERMINAL_PERSIST_MODE),
        )
    }

    pub(crate) fn remove_terminal_snapshot(&self, workspace_id: &str) -> std::io::Result<()> {
        match std::fs::remove_file(snapshot_path(self.data_dir(), workspace_id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// The dock layout: cached, else read from the snapshot once.
    pub(crate) fn terminal_chrome(&self, workspace_id: &str) -> Chrome {
        if let Some(c) = self.term_persist.cached_chrome(workspace_id) {
            return c;
        }
        let c = if workspace_id.is_empty() {
            Chrome::default()
        } else {
            read_gz_json::<PersistedChrome>(&snapshot_path(self.data_dir(), workspace_id))
                .map(|c| Chrome::of(&c))
                .unwrap_or_default()
        };
        self.term_persist.cache_chrome(workspace_id, c, false)
    }

    pub(crate) fn set_terminal_chrome(&self, workspace_id: &str, c: Chrome) {
        self.term_persist
            .cache_chrome(workspace_id, c.clamped(), true);
        self.term_persist.mark_dirty(workspace_id);
    }

    fn persist_workspace_terminals(&self, workspace_id: &str) -> std::io::Result<()> {
        if self.store().workspace(workspace_id).is_none() {
            return self.remove_terminal_snapshot(workspace_id);
        }
        let chrome = self.terminal_chrome(workspace_id).clamped();
        let snaps = self.term.snapshots(workspace_id);
        if snaps.is_empty() {
            if self.term_persist.closed() {
                // CloseAll already emptied the roster: keep the snapshot the
                // shutdown flush just wrote.
                return Ok(());
            }
            if chrome.idle() {
                return self.remove_terminal_snapshot(workspace_id);
            }
        }
        let active_id = self.term.active_id(workspace_id);
        let active = snaps.iter().rposition(|s| s.id == active_id).unwrap_or(0) as i64;
        let tabs = snaps
            .into_iter()
            .map(|s| PersistedTab {
                title: s.title,
                cwd: s.cwd,
                replay: s.replay,
            })
            .collect();
        self.write_terminal_snapshot(
            workspace_id,
            &PersistedTerminals {
                open: chrome.open,
                maximized: chrome.maximized,
                editor_hidden: chrome.editor_hidden,
                beside: chrome.beside,
                beside_pct: chrome.beside_pct,
                active,
                tabs: Some(tabs),
            },
        )
    }

    /// Persists every tab, then kills every session's process group so a dev
    /// server cannot outlive the app holding its port.
    pub(crate) async fn terminal_shutdown(&self) {
        self.term_persist.close();
        let me = self.me();
        let _ = tokio::task::spawn_blocking(move || {
            me.term_persist.flush(&me);
            me.term.close_all();
        })
        .await;
    }
}
