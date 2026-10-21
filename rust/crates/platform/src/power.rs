//! Machine power assertions (macOS `caffeinate`).

use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};

/// Matches other Recur subprocess teardown: SIGTERM, brief wait, then SIGKILL
/// the process group.
const KILL_GRACE: Duration = Duration::from_secs(2);

/// Holds a single macOS `caffeinate` assertion while `set(true)`. On other
/// platforms every method is a no-op. Safe for concurrent use.
///
/// `caffeinate` runs with `-w <recur-pid>` so the assertion self-releases if
/// Recur crashes or is force-quit (any path that skips `close`). `close` marks
/// the keeper terminal so a late `set(true)` during shutdown cannot respawn it.
#[derive(Default)]
pub struct Keeper {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    child: Option<Tracked>,
    closed: bool,
    next_id: u64,
}

#[derive(Clone)]
struct Tracked {
    id: u64,
    pid: u32,
    done: Arc<Done>,
}

/// Set once the waiter thread has reaped the child, so a recycled pid is
/// never mistaken for a live caffeinate.
#[derive(Default)]
struct Done {
    reaped: Mutex<bool>,
    cv: Condvar,
}

impl Done {
    fn is_set(&self) -> bool {
        *self.reaped.lock()
    }

    fn set(&self) {
        *self.reaped.lock() = true;
        self.cv.notify_all();
    }

    /// Waits up to `timeout`; true once reaped.
    fn wait_for(&self, timeout: Duration) -> bool {
        let mut reaped = self.reaped.lock();
        if !*reaped {
            self.cv.wait_for(&mut reaped, timeout);
        }
        *reaped
    }

    fn wait(&self) {
        let mut reaped = self.reaped.lock();
        while !*reaped {
            self.cv.wait(&mut reaped);
        }
    }
}

impl Keeper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts or stops the assertion to match `want`. Idempotent. After
    /// `close`, `set(true)` is a no-op and `set(false)` still drops any
    /// leftover child.
    pub fn set(&self, want: bool) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let mut st = self.state.lock();
        let want = want && !st.closed;
        if want {
            self.start_locked(&mut st);
            return;
        }
        let taken = take_locked(&mut st);
        drop(st);
        kill(taken);
    }

    /// Drops any held assertion. Equivalent to `set(false)`. Does not close.
    pub fn stop(&self) {
        self.set(false);
    }

    /// Permanently disables the keeper and drops any held assertion. Used by
    /// engine shutdown so a late agent release cannot respawn caffeinate as
    /// the process exits.
    pub fn close(&self) {
        let mut st = self.state.lock();
        st.closed = true;
        if !cfg!(target_os = "macos") {
            return;
        }
        let taken = take_locked(&mut st);
        drop(st);
        kill(taken);
    }

    /// Whether a caffeinate child is currently tracked.
    pub fn is_active(&self) -> bool {
        alive_locked(&mut self.state.lock())
    }

    fn start_locked(&self, st: &mut State) {
        if st.closed || alive_locked(st) {
            return;
        }
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        // -w tethers the assertion to this process: if Recur dies without
        // close, caffeinate exits and idle sleep can resume.
        let spawned = Command::new("caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => {
                tracing::warn!("start caffeinate: {err}");
                return;
            }
        };
        st.next_id += 1;
        let tracked = Tracked {
            id: st.next_id,
            pid: child.id(),
            done: Arc::default(),
        };
        st.child = Some(tracked.clone());
        let state = Arc::clone(&self.state);
        std::thread::spawn(move || {
            let _ = child.wait();
            tracked.done.set();
            let mut st = state.lock();
            if st.child.as_ref().is_some_and(|c| c.id == tracked.id) {
                st.child = None;
            }
        });
    }

    #[cfg(test)]
    fn pid(&self) -> Option<u32> {
        self.state.lock().child.as_ref().map(|c| c.pid)
    }
}

fn alive_locked(st: &mut State) -> bool {
    match &st.child {
        Some(c) if !c.done.is_set() => true,
        _ => {
            st.child = None;
            false
        }
    }
}

/// Clears the tracked child and returns it for teardown outside the lock.
fn take_locked(st: &mut State) -> Option<Tracked> {
    if !alive_locked(st) {
        return None;
    }
    st.child.take()
}

/// Signals the process group without holding the keeper lock, and finishes
/// the wait in the background so agent acquire/release is never stalled.
/// SIGKILL is only sent while the child is unreaped, so a recycled pid is
/// never signalled.
fn kill(taken: Option<Tracked>) {
    let Some(t) = taken else { return };
    if t.done.is_set() {
        return;
    }
    // process_group(0) makes pgid == pid.
    signal_group(t.pid, rustix::process::Signal::TERM);
    std::thread::spawn(move || {
        if t.done.wait_for(KILL_GRACE) {
            return;
        }
        if !t.done.is_set() {
            signal_group(t.pid, rustix::process::Signal::KILL);
            t.done.wait();
        }
    });
}

fn signal_group(pid: u32, sig: rustix::process::Signal) {
    if let Some(pgid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pgid, sig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn caffeinate_available() -> bool {
        cfg!(target_os = "macos")
            && std::process::Command::new("sh")
                .args(["-c", "command -v caffeinate"])
                .output()
                .is_ok_and(|o| o.status.success())
    }

    fn pid_alive(pid: u32) -> bool {
        let pid = rustix::process::Pid::from_raw(pid as i32).expect("pid");
        rustix::process::test_kill_process(pid).is_ok()
    }

    #[test]
    fn set_true_false() {
        if !caffeinate_available() {
            return;
        }
        let k = Keeper::new();
        k.set(true);
        assert!(k.is_active(), "expected caffeinate running after set(true)");
        let pid = k.pid().expect("pid");
        assert!(pid_alive(pid), "caffeinate pid {pid} not alive");

        k.set(true);
        assert!(k.is_active());
        assert_eq!(
            k.pid(),
            Some(pid),
            "second set(true) should keep the same process"
        );

        k.set(false);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !k.is_active() && !pid_alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("caffeinate pid {pid} still alive after set(false)");
    }

    #[test]
    fn stop_when_never_started() {
        let k = Keeper::new();
        k.stop();
        assert!(!k.is_active());
    }

    #[test]
    fn close_refuses_restart() {
        if !caffeinate_available() {
            return;
        }
        let k = Keeper::new();
        k.set(true);
        assert!(k.is_active());
        k.close();
        let deadline = Instant::now() + Duration::from_secs(3);
        while k.is_active() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!k.is_active(), "expected inactive after close");
        k.set(true);
        assert!(
            !k.is_active(),
            "set(true) after close must not respawn caffeinate"
        );
    }
}
