//! Interactive PTY sessions for the Observatory's terminal dock.
//!
//! The design keeps the transport cheap. Three rules carry the weight.
//!
//!  1. Coalesce. A PTY hands back whatever the kernel has, which under `cat bigfile` is thousands
//!     of small reads a second. Output is buffered and flushed on a ~8ms tick (or early once a
//!     chunk is worth sending), so consumers see ~120 large chunks a second at worst.
//!  2. Flow control. The client acks bytes once its emulator has parsed them; a session with too
//!     many unacked bytes in flight stops reading its PTY, which back-pressures the shell. Blocking
//!     the producer is what a real terminal does, and it keeps memory flat.
//!  3. Detach is not kill. Closing the dock or switching workspaces only detaches the client. The
//!     shell keeps running, output lands in a bounded replay ring, and re-attaching repaints from
//!     that ring. Sessions die when the user closes them or the app exits; the service layer
//!     snapshots tab list, cwd and replay so the next launch can restore them.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use filedescriptor::{FileDescriptor, Pipe};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::signal::Signal;
use parking_lot::{Condvar, Mutex};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};

use crate::cwd::process_cwd;
use crate::osc7::Osc7Parser;
use crate::plain::tail_plain;
use crate::process::{is_shell_foreground, kill_after_grace, signal_group, tree_groups};
use crate::queries::QueryFilter;
use crate::ring::Ring;

/// PTY read buffer. Large enough that a burst costs few syscalls.
pub(crate) const READ_CHUNK: usize = 64 << 10;
/// Bounds added latency for a small write (a keystroke echo) at one tick.
const FLUSH_INTERVAL: Duration = Duration::from_millis(8);
/// Flush early rather than waiting out the tick, so a burst moves in steady chunks.
const MAX_PENDING: usize = 64 << 10;
/// Ceiling on unacked bytes (queued + in flight) before a session stops reading its PTY.
pub(crate) const HIGH_WATER: usize = 256 << 10;
/// Per-session replay ring: enough that a restart restore has useful scrollback.
const REPLAY_CAP: usize = 1 << 20;
const MAX_EXITED_TOMBSTONES: usize = 4;
const CWD_REFRESH_MIN: Duration = Duration::from_secs(2);
const IDLE_POLL: Duration = Duration::from_millis(25);
const IDLE_POLL_MAX: Duration = Duration::from_secs(1);
const INTERRUPT_GRACE: Duration = Duration::from_secs(2);
const INTERRUPT_POLL: Duration = Duration::from_millis(20);

/// Appended after a persisted replay when seeding a new PTY so a snapshot taken inside an
/// alt-screen program (vim, htop) does not leave the restored tab stuck in that screen, or
/// leave its input modes (mouse reports, focus events, bracketed paste, application
/// cursor/keypad) aimed at the fresh shell.
pub const RESTORE_TRAILER: &[u8] =
    b"\x1b[?1049l\x1b[?1000;1002;1003;1004;1005;1006;1015;2004;1l\x1b>\x1b[0m\x1b[?25h\r\n";

/// Session lifecycle and output, published to the services layer (the former `term.*` events).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `term.data`: one coalesced chunk of raw VT output for an attached client.
    Data { id: String, data: Vec<u8> },
    /// `term.exit`: the shell died; the session stays listed as an exited tombstone.
    Exit {
        id: String,
        workspace_id: String,
        exit_code: i32,
    },
    /// `term.busy`: a command took the foreground from the login shell.
    Busy { id: String, workspace_id: String },
    /// `term.idle`: the foreground command returned to the login shell.
    Idle { id: String, workspace_id: String },
}

impl Event {
    /// The app event name.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Data { .. } => "term.data",
            Event::Exit { .. } => "term.exit",
            Event::Busy { .. } => "term.busy",
            Event::Idle { .. } => "term.idle",
        }
    }

    pub fn session_id(&self) -> &str {
        match self {
            Event::Data { id, .. }
            | Event::Exit { id, .. }
            | Event::Busy { id, .. }
            | Event::Idle { id, .. } => id,
        }
    }
}

/// Publishes an event. Injected so this crate stays free of the services and engine layers.
pub type Emitter = Arc<dyn Fn(Event) + Send + Sync>;
type DirtyHook = Arc<dyn Fn(&str) + Send + Sync>;

/// The client-facing view of a session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub id: String,
    pub workspace_id: String,
    pub cwd: String,
    pub shell: String,
    /// Seeds a fresh tab label. Live titles come from the emulator's OSC title handler.
    pub title: String,
    /// RFC 3339 with nanoseconds so same-second opens (restore, rapid ⌘T) still sort.
    pub created_at: String,
    /// True after the shell died but before the user dismissed the tab. Replay stays readable so
    /// `@terminal` / `read_terminal` can still attach the last screen.
    #[serde(default, skip_serializing_if = "is_false")]
    pub exited: bool,
    /// The shell's status when `exited`, including 0 (clean exit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// True when a foreground process other than the login shell owns the tty.
    #[serde(default, skip_serializing_if = "is_false")]
    pub busy: bool,
    /// True for the dock's focused tab, so a restart reselects the same tab.
    #[serde(default, skip_serializing_if = "is_false")]
    pub active: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Seeds for a session opened to restore a previous tab.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    pub title: String,
    pub replay: Vec<u8>,
}

/// One tab's durable state: enough to spawn a new shell in the same directory and repaint the
/// last screen. Process identity is not kept; the PTY dies with the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub replay: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("terminal: no working directory for workspace {0:?}")]
    NoWorkingDirectory(String),
    #[error("terminal: working directory {0:?} is unavailable")]
    CwdUnavailable(String),
    #[error("terminal: start {shell}: {reason}")]
    Start { shell: String, reason: String },
    #[error("terminal: session {0:?} not found")]
    NotFound(String),
    #[error("terminal: write: {0}")]
    Write(std::io::Error),
    #[error("terminal: resize: {0}")]
    Resize(String),
    #[error("reorder terminals: got {got} ids, have {have} sessions")]
    ReorderCount { got: usize, have: usize },
    #[error("reorder terminals: unknown session {0:?}")]
    ReorderUnknown(String),
    #[error("reorder terminals: duplicate session {0:?}")]
    ReorderDuplicate(String),
    #[error("terminal: process group {0} still running after interrupt")]
    StillRunning(i32),
}

/// Owns every live session. One per app; clones share the same sessions.
#[derive(Clone)]
pub struct Manager(Arc<Shared>);

struct Shared {
    reg: Mutex<Registry>,
    emit: Emitter,
    on_dirty: Mutex<Option<DirtyHook>>,
}

#[derive(Default)]
struct Registry {
    sessions: HashMap<String, Arc<Session>>,
    /// workspace id → focused session id
    active: HashMap<String, String>,
    /// workspace id → session ids in tab-strip order
    order: HashMap<String, Vec<String>>,
}

impl Manager {
    pub fn new(emit: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self(Arc::new(Shared {
            reg: Mutex::default(),
            emit: Arc::new(emit),
            on_dirty: Mutex::new(None),
        }))
    }

    /// Registers a hook fired when a workspace's sessions change in a way that should be
    /// persisted (open, close, title, cwd, output). The hook must not block on manager calls.
    pub fn set_on_dirty(&self, hook: impl Fn(&str) + Send + Sync + 'static) {
        *self.0.on_dirty.lock() = Some(Arc::new(hook));
    }

    /// Starts a login shell in `cwd`. `cols`/`rows` seed the PTY size so the first prompt is
    /// already correctly wrapped. `extra_env` (`KEY=VALUE`) is merged over the PTY baseline.
    pub fn open(
        &self,
        workspace_id: &str,
        cwd: &str,
        cols: i32,
        rows: i32,
        extra_env: &[String],
    ) -> Result<Info, Error> {
        self.open_with(
            workspace_id,
            cwd,
            cols,
            rows,
            extra_env,
            OpenOptions::default(),
        )
    }

    /// `open` plus restore seeds (prior title and replay buffer).
    pub fn open_with(
        &self,
        workspace_id: &str,
        cwd: &str,
        cols: i32,
        rows: i32,
        extra_env: &[String],
        opts: OpenOptions,
    ) -> Result<Info, Error> {
        if cwd.is_empty() {
            return Err(Error::NoWorkingDirectory(workspace_id.to_string()));
        }
        if !dir_exists(cwd) {
            return Err(Error::CwdUnavailable(cwd.to_string()));
        }
        let (cols, rows) = clamp_size(cols, rows);
        let shell = login_shell();
        let start_err = |reason: String| Error::Start {
            shell: shell.clone(),
            reason,
        };

        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| start_err(e.to_string()))?;
        // A login shell (-l) is what Terminal.app and iTerm give you, so PATH, version managers
        // and shell functions are present instead of the GUI app's stunted launchd environment.
        let mut cmd = CommandBuilder::new(&shell);
        cmd.arg("-l");
        cmd.cwd(cwd);
        cmd.env_clear();
        for kv in merge_env(pty_env(), extra_env) {
            let (k, v) = split_env(&kv);
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| start_err(e.to_string()))?;
        drop(pair.slave);
        let master = pair.master;
        let raw = master
            .as_raw_fd()
            .ok_or_else(|| start_err("pty has no master fd".into()))?;
        let reader = FileDescriptor::dup(&Raw(raw)).map_err(|e| start_err(e.to_string()))?;
        let writer = FileDescriptor::dup(&Raw(raw)).map_err(|e| start_err(e.to_string()))?;
        let stop = Pipe::new().map_err(|e| start_err(e.to_string()))?;

        let title = match opts.title.trim() {
            "" => basename(&shell),
            t => t.to_string(),
        };
        let mut replay = Ring::new(REPLAY_CAP);
        if !opts.replay.is_empty() {
            replay.write(&opts.replay);
            replay.write(RESTORE_TRAILER);
        }
        let (wake, wake_rx) = sync_channel(1);
        let (idle_kick, idle_rx) = sync_channel(1);
        let weak = Arc::downgrade(&self.0);
        let ws = workspace_id.to_string();
        let session = Arc::new(Session {
            id: uuid::Uuid::now_v7().to_string(),
            workspace_id: workspace_id.to_string(),
            shell: shell.clone(),
            created_at: Utc::now(),
            pid: child.process_id().map_or(0, |p| p as i32),
            master: Mutex::new(Some(master)),
            writer: Mutex::new(Some(writer)),
            emit: self.0.emit.clone(),
            emit_mu: Mutex::new(()),
            st: Mutex::new(State::new(cwd, title, replay)),
            cond: Condvar::new(),
            wake,
            idle_kick,
            stop: Mutex::new(Some(stop.write)),
            done: AtomicBool::new(false),
            on_dirty: Box::new(move || {
                if let Some(m) = weak.upgrade() {
                    m.dirty(&ws);
                }
            }),
        });

        {
            let mut reg = self.0.reg.lock();
            reg.sessions.insert(session.id.clone(), session.clone());
            let order = reg.order.entry(workspace_id.to_string()).or_default();
            if !order.contains(&session.id) {
                order.push(session.id.clone());
            }
        }

        let s = session.clone();
        std::thread::Builder::new()
            .name("term-flush".into())
            .spawn(move || s.flush_loop(wake_rx))
            .ok();
        let s = session.clone();
        std::thread::Builder::new()
            .name("term-idle".into())
            .spawn(move || s.watch_idle(idle_rx))
            .ok();
        let s = session.clone();
        let shared = Arc::downgrade(&self.0);
        std::thread::Builder::new()
            .name("term-read".into())
            .spawn(move || {
                s.read_loop(reader, stop.read);
                s.reap(child, &shared);
            })
            .map_err(|e| start_err(e.to_string()))?;

        self.0.dirty(workspace_id);
        Ok(session.info())
    }

    /// The workspace's sessions in tab-strip order. New tabs append until `reorder` moves them;
    /// sessions missing from the saved order follow by creation time.
    pub fn list(&self, workspace_id: &str) -> Vec<Info> {
        self.0.reg.lock().list(workspace_id)
    }

    /// Persists a new tab-strip order. `ids` must be a permutation of the workspace's current
    /// session ids (including exited tombstones still listed).
    pub fn reorder(&self, workspace_id: &str, ids: &[String]) -> Result<(), Error> {
        {
            let mut reg = self.0.reg.lock();
            let live = reg.list(workspace_id);
            permutation(&live, ids)?;
            if live.iter().map(|i| &i.id).eq(ids.iter()) {
                return Ok(());
            }
            reg.order.insert(workspace_id.to_string(), ids.to_vec());
        }
        self.0.dirty(workspace_id);
        Ok(())
    }

    /// Live (non-exited) tabs in list order, with cwd refreshed from the shell when possible.
    pub fn snapshots(&self, workspace_id: &str) -> Vec<SessionSnapshot> {
        let mut out = Vec::new();
        for info in self.list(workspace_id) {
            if info.exited {
                continue;
            }
            let Ok(s) = self.get(&info.id) else { continue };
            refresh_cwd(&s.st, s.pid);
            let st = s.st.lock();
            let title = if st.title.is_empty() {
                basename(&s.shell)
            } else {
                st.title.clone()
            };
            out.push(SessionSnapshot {
                id: s.id.clone(),
                title,
                cwd: st.cwd.clone(),
                replay: st.replay.snapshot(),
            });
        }
        out
    }

    fn get(&self, id: &str) -> Result<Arc<Session>, Error> {
        self.0
            .reg
            .lock()
            .sessions
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(id.to_string()))
    }

    /// Marks a client live and returns the raw VT replay to repaint from.
    pub fn attach(&self, id: &str) -> Result<Vec<u8>, Error> {
        let s = self.get(id)?;
        let snap = {
            let mut st = s.st.lock();
            st.attached = true;
            // A new client has an empty screen and owes nothing for what the last one saw.
            st.outstanding = 0;
            st.pending.clear();
            st.replay.snapshot()
        };
        s.cond.notify_all();
        Ok(snap)
    }

    /// Stops streaming without touching the shell. Output continues into the replay ring and the
    /// read loop is released from flow control: a detached session must never hold its PTY
    /// blocked waiting for acks that aren't coming.
    pub fn detach(&self, id: &str) -> Result<(), Error> {
        let s = self.get(id)?;
        {
            let mut st = s.st.lock();
            st.attached = false;
            st.outstanding = 0;
            st.pending.clear();
        }
        s.cond.notify_all();
        Ok(())
    }

    /// Reports that the client parsed `n` more bytes, releasing flow control.
    pub fn ack(&self, id: &str, n: usize) -> Result<(), Error> {
        let s = self.get(id)?;
        {
            let mut st = s.st.lock();
            st.outstanding = st.outstanding.saturating_sub(n);
        }
        s.cond.notify_all();
        Ok(())
    }

    /// Forwards keystrokes (or pasted text, mouse reports, terminal replies) to the shell.
    pub fn write(&self, id: &str, data: impl AsRef<[u8]>) -> Result<(), Error> {
        let s = self.get(id)?;
        s.write_pty(data.as_ref())?;
        s.kick_idle_watch();
        Ok(())
    }

    /// Updates the PTY window size, which makes SIGWINCH-aware programs redraw.
    pub fn resize(&self, id: &str, cols: i32, rows: i32) -> Result<(), Error> {
        let s = self.get(id)?;
        let (cols, rows) = clamp_size(cols, rows);
        let master = s.master.lock();
        let master = master
            .as_ref()
            .ok_or_else(|| Error::Resize("pty is closed".into()))?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Resize(e.to_string()))
    }

    /// Records the live OSC / dock tab label so `@terminal` chips and `list_terminals` name a
    /// session the way the human sees it. Blank titles are ignored.
    pub fn set_title(&self, id: &str, title: &str) -> Result<(), Error> {
        let s = self.get(id)?;
        let title = title.trim();
        if title.is_empty() {
            return Ok(());
        }
        s.st.lock().title = title.to_string();
        self.0.dirty(&s.workspace_id);
        Ok(())
    }

    /// Records which session the dock shows for `workspace_id`; `""` clears. Bare `@terminal` and
    /// `read_terminal` with no id resolve to this tab.
    pub fn set_active(&self, workspace_id: &str, id: &str) {
        if workspace_id.is_empty() {
            return;
        }
        {
            let mut reg = self.0.reg.lock();
            if id.is_empty() {
                reg.active.remove(workspace_id);
            } else {
                reg.active.insert(workspace_id.to_string(), id.to_string());
            }
        }
        self.0.dirty(workspace_id);
    }

    /// The focused session for `workspace_id`, or "".
    pub fn active_id(&self, workspace_id: &str) -> String {
        self.0
            .reg
            .lock()
            .active
            .get(workspace_id)
            .cloned()
            .unwrap_or_default()
    }

    /// ANSI-stripped recent output for a session (last `max_lines`).
    pub fn read_plain(&self, id: &str, max_lines: usize) -> Result<String, Error> {
        let s = self.get(id)?;
        let snap = s.st.lock().replay.snapshot();
        Ok(tail_plain(&snap, max_lines))
    }

    /// Drops the replay buffer after the user clears the terminal so re-attaching doesn't bring
    /// back the output they just wiped.
    pub fn clear_replay(&self, id: &str) -> Result<(), Error> {
        let s = self.get(id)?;
        s.st.lock().replay.reset();
        self.0.dirty(&s.workspace_id);
        Ok(())
    }

    /// Stops the session's foreground process group without killing the shell. Writing ^C only
    /// delivers SIGINT when the tty has ISIG set; a dev server in raw mode swallows the byte.
    /// Signalling the foreground group bypasses termios, then escalates to SIGTERM and SIGKILL.
    /// Success means the group is gone; the shell's own group is never killed here. Blocks for up
    /// to ~4.5s, so call it off the UI thread.
    pub fn interrupt(&self, id: &str) -> Result<(), Error> {
        let s = self.get(id)?;
        let res = s.interrupt_foreground();
        s.kick_idle_watch();
        res
    }

    /// Terminates a session's process tree.
    pub fn close(&self, id: &str) -> Result<(), Error> {
        let s = self.get(id)?;
        let groups = s.terminate();
        std::thread::spawn(move || kill_after_grace(&groups));
        self.0.drop_session(&s);
        self.0.dirty(&s.workspace_id);
        Ok(())
    }

    /// Tears down every session. Called on app shutdown so quitting doesn't strand orphaned
    /// shells (and whatever they were running) behind it. Blocks for the kill grace period.
    pub fn close_all(&self) {
        let all: Vec<Arc<Session>> = {
            let mut reg = self.0.reg.lock();
            reg.active.clear();
            reg.order.clear();
            reg.sessions.drain().map(|(_, s)| s).collect()
        };
        let groups: Vec<i32> = all.iter().flat_map(|s| s.terminate()).collect();
        kill_after_grace(&groups);
    }

    #[cfg(test)]
    fn session(&self, id: &str) -> Arc<Session> {
        self.get(id).unwrap()
    }
}

impl Shared {
    fn dirty(&self, workspace_id: &str) {
        let hook = self.on_dirty.lock().clone();
        if let Some(hook) = hook {
            hook(workspace_id);
        }
    }

    fn drop_session(&self, s: &Session) {
        let mut reg = self.reg.lock();
        reg.sessions.remove(&s.id);
        reg.remove(&s.workspace_id, &s.id);
    }

    /// Drops the oldest-exited sessions in a workspace past the cap so a day of `exit`s cannot
    /// hold unbounded rings and fds. Eviction is by exit time, not open time.
    fn prune_tombstones(&self, workspace_id: &str) {
        let mut reg = self.reg.lock();
        let mut dead: Vec<(Instant, String)> = reg
            .sessions
            .values()
            .filter(|s| s.workspace_id == workspace_id)
            .filter_map(|s| {
                let st = s.st.lock();
                st.exited
                    .then(|| (st.exited_at.unwrap_or_else(Instant::now), s.id.clone()))
            })
            .collect();
        if dead.len() <= MAX_EXITED_TOMBSTONES {
            return;
        }
        dead.sort();
        let evict = dead.len() - MAX_EXITED_TOMBSTONES;
        for (_, id) in dead.into_iter().take(evict) {
            reg.sessions.remove(&id);
            reg.remove(workspace_id, &id);
        }
    }
}

impl Registry {
    fn remove(&mut self, workspace_id: &str, id: &str) {
        if self.active.get(workspace_id).is_some_and(|a| a == id) {
            self.active.remove(workspace_id);
        }
        if let Some(ids) = self.order.get_mut(workspace_id) {
            ids.retain(|x| x != id);
            if ids.is_empty() {
                self.order.remove(workspace_id);
            }
        }
    }

    fn list(&self, workspace_id: &str) -> Vec<Info> {
        let active = self
            .active
            .get(workspace_id)
            .map(String::as_str)
            .unwrap_or("");
        let mine: HashMap<&str, &Arc<Session>> = self
            .sessions
            .values()
            .filter(|s| s.workspace_id == workspace_id)
            .map(|s| (s.id.as_str(), s))
            .collect();
        let mut out = Vec::with_capacity(mine.len());
        let mut seen = std::collections::HashSet::new();
        for id in self.order.get(workspace_id).into_iter().flatten() {
            if let Some(s) = mine.get(id.as_str()) {
                seen.insert(id.as_str());
                out.push(s.listed_info(active));
            }
        }
        let mut extra: Vec<&&Arc<Session>> = mine
            .iter()
            .filter(|(id, _)| !seen.contains(*id))
            .map(|(_, s)| s)
            .collect();
        extra.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        out.extend(extra.into_iter().map(|s| s.listed_info(active)));
        out
    }
}

fn permutation(live: &[Info], ids: &[String]) -> Result<(), Error> {
    if ids.len() != live.len() {
        return Err(Error::ReorderCount {
            got: ids.len(),
            have: live.len(),
        });
    }
    let have: std::collections::HashSet<&str> = live.iter().map(|i| i.id.as_str()).collect();
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !have.contains(id.as_str()) {
            return Err(Error::ReorderUnknown(id.clone()));
        }
        if !seen.insert(id.as_str()) {
            return Err(Error::ReorderDuplicate(id.clone()));
        }
    }
    Ok(())
}

/// One live PTY and the shell running inside it.
struct Session {
    id: String,
    workspace_id: String,
    shell: String,
    created_at: DateTime<Utc>,
    pid: i32,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<FileDescriptor>>,
    emit: Emitter,
    /// Serialises "take the pending chunk, then publish it" so two concurrent flushes cannot
    /// deliver VT bytes out of order. Separate from `st` so an in-flight publish never blocks
    /// an ack.
    emit_mu: Mutex<()>,
    st: Mutex<State>,
    /// Signalled when `outstanding` drops, or on detach/close.
    cond: Condvar,
    /// Nudges the flusher when pending is worth sending.
    wake: SyncSender<()>,
    /// Resets the idle watcher's backoff after a write or interrupt.
    idle_kick: SyncSender<()>,
    /// Write end of the reader's stop pipe; dropping it ends the read loop.
    stop: Mutex<Option<FileDescriptor>>,
    done: AtomicBool,
    on_dirty: Box<dyn Fn() + Send + Sync>,
}

struct State {
    cwd: String,
    /// OSC 7 reported the cwd; skip lsof while this holds.
    cwd_from_osc: bool,
    cwd_checked_at: Option<Instant>,
    /// OSC / dock tab label; empty means basename(shell).
    title: String,
    attached: bool,
    /// Bytes read from the PTY that the client has not acked, counted at ingest rather than at
    /// flush so the pending buffer is inside the budget.
    outstanding: usize,
    pending: Vec<u8>,
    replay: Ring,
    closed: bool,
    exited: bool,
    exited_at: Option<Instant>,
    exit_code: i32,
}

impl State {
    fn new(cwd: &str, title: String, replay: Ring) -> Self {
        Self {
            cwd: cwd.to_string(),
            cwd_from_osc: false,
            cwd_checked_at: None,
            title,
            attached: false,
            outstanding: 0,
            pending: Vec::new(),
            replay,
            closed: false,
            exited: false,
            exited_at: None,
            exit_code: 0,
        }
    }
}

impl Session {
    fn info(&self) -> Info {
        let (title, exited, exit_code, cwd) = {
            let st = self.st.lock();
            (st.title.clone(), st.exited, st.exit_code, st.cwd.clone())
        };
        Info {
            id: self.id.clone(),
            workspace_id: self.workspace_id.clone(),
            cwd,
            shell: self.shell.clone(),
            title: if title.is_empty() {
                basename(&self.shell)
            } else {
                title
            },
            created_at: rfc3339_nano(self.created_at),
            exited,
            exit_code: exited.then_some(exit_code),
            busy: !exited && self.is_busy(),
            active: false,
        }
    }

    fn listed_info(&self, active: &str) -> Info {
        let mut info = self.info();
        info.active = self.id == active && !info.exited;
        info
    }

    fn is_busy(&self) -> bool {
        self.pid > 0
            && self
                .foreground_pgid()
                .is_some_and(|fg| !is_shell_foreground(fg, self.pid))
    }

    fn foreground_pgid(&self) -> Option<i32> {
        if self.st.lock().closed {
            return None;
        }
        // Holding the lock pins the fd against a concurrent release in reap/terminate.
        let master = self.master.lock();
        master.as_ref()?.process_group_leader().filter(|&p| p > 1)
    }

    fn is_closed(&self) -> bool {
        self.st.lock().closed
    }

    fn write_pty(&self, data: &[u8]) -> Result<(), Error> {
        let mut writer = self.writer.lock();
        let w = writer
            .as_mut()
            .ok_or_else(|| Error::Write(std::io::ErrorKind::BrokenPipe.into()))?;
        w.write_all(data).map_err(Error::Write)
    }

    fn kick_idle_watch(&self) {
        let _ = self.idle_kick.try_send(());
    }

    fn read_loop(&self, mut reader: FileDescriptor, stop: FileDescriptor) {
        let mut queries = QueryFilter::new();
        let mut osc7 = Osc7Parser::default();
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            let mut fds = [
                PollFd::new(reader.as_fd(), PollFlags::POLLIN),
                PollFd::new(stop.as_fd(), PollFlags::POLLIN),
            ];
            match poll(&mut fds, PollTimeout::NONE) {
                Ok(_) => {}
                Err(nix::errno::Errno::EINTR) => continue,
                Err(_) => break,
            }
            if fds[1].revents().is_some_and(|r| !r.is_empty()) {
                break;
            }
            if fds[0].revents().is_none_or(|r| r.is_empty()) {
                continue;
            }
            match reader.read(&mut buf) {
                // EOF / EIO on a closed PTY is the normal "shell exited" path.
                Ok(0) => break,
                Ok(n) => self.ingest(&buf[..n], &mut queries, &mut osc7),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => break,
            }
            self.await_credit();
        }
        // Release every master fd so the slave side hangs up, as closing the PTY master does.
        drop(reader);
        self.master.lock().take();
        self.writer.lock().take();
        self.done.store(true, Ordering::SeqCst);
        let _ = self.wake.try_send(());
    }

    /// Records output for replay and, when a client is attached, queues it for the next flush.
    /// Colour/CPR probes are answered synchronously on the PTY and stripped first.
    fn ingest(&self, p: &[u8], queries: &mut QueryFilter, osc7: &mut Osc7Parser) {
        let attached = self.st.lock().attached;
        let (forward, replies) = queries.process(p, attached);
        if !replies.is_empty() {
            // Best-effort: a failed write means the PTY is already gone.
            let _ = self.write_pty(&replies);
        }
        if forward.is_empty() {
            return;
        }
        let cwd = osc7.feed(&forward).filter(|c| dir_exists(c));
        let big = {
            let mut st = self.st.lock();
            if let Some(cwd) = cwd {
                st.cwd = cwd;
                st.cwd_from_osc = true;
            }
            st.replay.write(&forward);
            if st.attached {
                st.pending.extend_from_slice(&forward);
                st.outstanding += forward.len();
                Some(st.pending.len() >= MAX_PENDING)
            } else {
                None
            }
        };
        (self.on_dirty)();
        if big == Some(true) {
            let _ = self.wake.try_send(());
        }
    }

    /// Blocks the read loop while the client is too far behind, which back-pressures the PTY.
    fn await_credit(&self) {
        let mut st = self.st.lock();
        while st.attached && !st.closed && st.outstanding > HIGH_WATER {
            self.cond.wait(&mut st);
        }
    }

    fn flush_loop(&self, wake: Receiver<()>) {
        loop {
            if self.done.load(Ordering::SeqCst) {
                // The final flush is reap's job, so it is ordered before term.exit.
                return;
            }
            match wake.recv_timeout(FLUSH_INTERVAL) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {
                    if self.done.load(Ordering::SeqCst) {
                        return;
                    }
                    self.flush();
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Publishes one coalesced chunk. Bytes were charged against the budget at ingest and are
    /// only forgiven when the client acks.
    fn flush(&self) {
        let _guard = self.emit_mu.lock();
        let chunk = {
            let mut st = self.st.lock();
            if !st.attached || st.pending.is_empty() {
                return;
            }
            std::mem::take(&mut st.pending)
        };
        (self.emit)(Event::Data {
            id: self.id.clone(),
            data: chunk,
        });
    }

    /// Runs once the PTY has closed: flushes what the shell wrote on its way out and reports the
    /// exit status. The session stays registered as an exited tombstone until closed.
    fn reap(&self, mut child: Box<dyn Child + Send + Sync>, shared: &Weak<Shared>) {
        self.flush();
        let code = match child.wait() {
            Ok(status) if status.signal().is_none() => status.exit_code() as i32,
            _ => -1,
        };
        {
            let mut st = self.st.lock();
            st.exited = true;
            st.exited_at = Some(Instant::now());
            st.exit_code = code;
            st.closed = true;
        }
        self.cond.notify_all();
        let shared = shared.upgrade();
        // Prune before term.exit so listeners that list on that event see the cap.
        if let Some(m) = &shared {
            m.prune_tombstones(&self.workspace_id);
        }
        (self.emit)(Event::Exit {
            id: self.id.clone(),
            workspace_id: self.workspace_id.clone(),
            exit_code: code,
        });
        if let Some(m) = &shared {
            m.dirty(&self.workspace_id);
        }
    }

    /// SIGHUPs the shell's group and the foreground job's tree, then closes the PTY, returning the
    /// groups the caller must SIGKILL after the grace period. SIGHUP first is what a closing
    /// terminal window sends; SIGKILL is the backstop for anything that ignores it. Background
    /// jobs are left to the shell's own hangup handling, so `nohup cmd &` outlives the tab.
    fn terminate(&self) -> Vec<i32> {
        let fg = self.foreground_pgid().unwrap_or(0);
        {
            let mut st = self.st.lock();
            if st.closed {
                return Vec::new();
            }
            st.closed = true;
        }
        // Release a read loop parked in await_credit.
        self.cond.notify_all();
        let mut groups = Vec::new();
        if self.pid > 0 {
            groups = tree_groups(self.pid, fg);
            for &g in &groups {
                signal_group(g, Signal::SIGHUP);
            }
        }
        // Ending the read loop releases the PTY, which drives reap → term.exit.
        self.stop.lock().take();
        groups
    }

    fn interrupt_foreground(&self) -> Result<(), Error> {
        if self.pid <= 0 {
            return Ok(());
        }
        let Some(target) = self
            .foreground_pgid()
            .filter(|&fg| !is_shell_foreground(fg, self.pid))
        else {
            return Ok(());
        };
        for (sig, wait) in [
            (Signal::SIGINT, INTERRUPT_GRACE),
            (Signal::SIGTERM, INTERRUPT_GRACE),
            (Signal::SIGKILL, Duration::from_millis(500)),
        ] {
            signal_group(target, sig);
            if self.wait_until_group_gone(target, wait) {
                return Ok(());
            }
        }
        Err(Error::StillRunning(target))
    }

    fn wait_until_group_gone(&self, target: i32, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        loop {
            if self.is_closed() || crate::process::group_gone(target) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(INTERRUPT_POLL);
        }
    }

    /// Emits `term.busy` when a command takes the foreground and `term.idle` each time it returns
    /// to the login shell, so Recur can mark a run stopped without killing the tab. The poll backs
    /// off while quiet (up to 1s); a write or interrupt kicks it back to 25ms.
    fn watch_idle(&self, kick: Receiver<()>) {
        let mut saw_cmd = false;
        let mut delay = IDLE_POLL;
        loop {
            if self.is_closed() {
                return;
            }
            match self.foreground_pgid() {
                Some(fg) if self.pid > 0 && !is_shell_foreground(fg, self.pid) => {
                    if !saw_cmd {
                        (self.emit)(Event::Busy {
                            id: self.id.clone(),
                            workspace_id: self.workspace_id.clone(),
                        });
                    }
                    saw_cmd = true;
                }
                Some(_) if saw_cmd => {
                    (self.emit)(Event::Idle {
                        id: self.id.clone(),
                        workspace_id: self.workspace_id.clone(),
                    });
                    saw_cmd = false;
                }
                _ => {}
            }
            if delay < IDLE_POLL_MAX {
                delay = (delay * 2).min(IDLE_POLL_MAX);
            }
            match kick.recv_timeout(delay) {
                Ok(()) => delay = IDLE_POLL,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn refresh_cwd(st: &Mutex<State>, pid: i32) {
    let (from_osc, cwd, checked) = {
        let st = st.lock();
        (st.cwd_from_osc, st.cwd.clone(), st.cwd_checked_at)
    };
    if from_osc && dir_exists(&cwd) {
        return;
    }
    if checked.is_some_and(|c| c.elapsed() < CWD_REFRESH_MIN) {
        return;
    }
    let dir = process_cwd(pid);
    let ok = dir_exists(&dir);
    let mut st = st.lock();
    st.cwd_checked_at = Some(Instant::now());
    if ok {
        st.cwd = dir;
    }
}

struct Raw(RawFd);

impl AsRawFd for Raw {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

pub(crate) fn dir_exists(p: &str) -> bool {
    !p.is_empty() && Path::new(p).is_dir()
}

fn basename(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map_or_else(|| p.to_string(), |n| n.to_string_lossy().into_owned())
}

/// RFC 3339 with fractional seconds with trailing zeros trimmed.
fn rfc3339_nano(t: DateTime<Utc>) -> String {
    let base = t.format("%Y-%m-%dT%H:%M:%S").to_string();
    let frac = format!("{:09}", t.timestamp_subsec_nanos());
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{base}Z")
    } else {
        format!("{base}.{frac}Z")
    }
}

pub(crate) fn clamp_size(cols: i32, rows: i32) -> (u16, u16) {
    let cols = if cols < 2 { 80 } else { cols.min(1000) };
    let rows = if rows < 1 { 24 } else { rows.min(1000) };
    (cols as u16, rows as u16)
}

pub(crate) fn login_shell() -> String {
    if let Some(sh) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        return sh.to_string_lossy().into_owned();
    }
    ["/bin/zsh", "/bin/bash"]
        .into_iter()
        .find(|sh| Path::new(sh).exists())
        .unwrap_or("/bin/sh")
        .to_string()
}

/// The app environment plus the terminal capability advertisement. TERM must be set
/// explicitly: a GUI app launched from Finder has no TERM, and programs fall back to dumb output.
pub(crate) fn pty_env() -> Vec<OsString> {
    pty_env_from(std::env::vars_os())
}

fn pty_env_from(vars: impl Iterator<Item = (OsString, OsString)>) -> Vec<OsString> {
    // COLORFGBG: light-on-dark fallback for libraries that skip the OSC probe entirely.
    const DROP: &[&str] = &[
        "TERM",
        "COLORTERM",
        "TERM_PROGRAM",
        "COLORFGBG",
        "LINES",
        "COLUMNS",
    ];
    let mut out: Vec<OsString> = vars
        .filter(|(k, _)| !DROP.iter().any(|d| OsStr::new(d) == k))
        .map(|(k, v)| {
            let mut kv = k;
            kv.push("=");
            kv.push(v);
            kv
        })
        .collect();
    out.extend(
        [
            "TERM=xterm-256color",
            "COLORTERM=truecolor",
            "TERM_PROGRAM=Recur",
            "COLORFGBG=15;0",
        ]
        .map(OsString::from),
    );
    out
}

/// Overlays `KEY=VALUE` pairs from `extra` onto `base`, replacing matching keys.
pub(crate) fn merge_env(base: Vec<OsString>, extra: &[String]) -> Vec<OsString> {
    if extra.is_empty() {
        return base;
    }
    let replaced: Vec<&[u8]> = extra
        .iter()
        .filter_map(|kv| kv.find('=').filter(|&i| i > 0).map(|i| &kv.as_bytes()[..i]))
        .collect();
    let mut out: Vec<OsString> = base
        .into_iter()
        .filter(|kv| {
            let b = kv.as_bytes();
            !b.iter()
                .position(|&c| c == b'=')
                .filter(|&i| i > 0)
                .is_some_and(|i| replaced.contains(&&b[..i]))
        })
        .collect();
    out.extend(extra.iter().map(OsString::from));
    out
}

fn split_env(kv: &OsStr) -> (OsString, OsString) {
    let b = kv.as_bytes();
    match b.iter().position(|&c| c == b'=') {
        Some(i) => (
            OsString::from_vec(b[..i].to_vec()),
            OsString::from_vec(b[i + 1..].to_vec()),
        ),
        None => (kv.to_os_string(), OsString::new()),
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
