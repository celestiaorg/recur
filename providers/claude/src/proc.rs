//! Process plumbing shared by the turn runner and the status probe: PATH
//! lookup, the single reaper, process-group kill, and the error
//! wording for spawn and exit failures, which reaches the UI.

use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process, kill_process_group};
use tokio::process::{Child, Command};
use tokio::sync::watch;

/// How long a SIGTERMed process group gets before SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(2);

/// `None` while running; then `Some(wait error)`, where the error is the
/// wait text (`exit status 1`, `signal: killed`) and `None` is a
/// clean exit.
pub type Reaped = watch::Receiver<Option<Option<String>>>;

/// The sole owner of `wait()`. `after` runs between the exit and the broadcast,
/// so observers see the exit and the captured output together.
pub fn reap(mut child: Child, after: impl Future<Output = ()> + Send + 'static) -> Reaped {
    let (tx, rx) = watch::channel(None);
    tokio::spawn(async move {
        let status = child.wait().await;
        after.await;
        let err = match status {
            Ok(st) => exit_error(st),
            Err(e) => Some(e.to_string()),
        };
        let _ = tx.send(Some(err));
    });
    rx
}

pub async fn wait_reaped(reaped: &mut Reaped) {
    let _ = reaped.wait_for(Option::is_some).await;
}

/// Starts the child as the leader of a new session (and so of a new group, pgid == pid).
/// Every process its tools start stays in that session, even one that moves to its own group
/// or is orphaned to launchd (`nohup … &`), which a group kill misses.
pub fn new_session(cmd: &mut Command) {
    #[allow(unsafe_code)]
    // SAFETY: setsid is async-signal-safe and touches no parent state.
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()?;
            Ok(())
        });
    }
}

/// SIGTERM to the session led by `pid` (see [`new_session`]), then SIGKILL after
/// [`KILL_GRACE`] to the leader's group and whatever is left in the session; returns once
/// the reaper has seen the leader exit. The group SIGKILL is only sent while the leader is
/// unreaped, so a recycled pid is never signalled.
pub async fn kill_session(pid: u32, reaped: &mut Reaped) {
    let deadline = tokio::time::Instant::now() + KILL_GRACE;
    signal_group(pid, Signal::TERM);
    let mut left = signal_session(pid, Some(Signal::TERM)).await;
    if tokio::time::timeout_at(deadline, wait_reaped(reaped))
        .await
        .is_err()
    {
        signal_group(pid, Signal::KILL);
        wait_reaped(reaped).await;
    }
    while left > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        left = signal_session(pid, None).await;
    }
    // A member forking while it dies can leave a new one behind.
    for _ in 0..3 {
        if signal_session(pid, Some(Signal::KILL)).await == 0 {
            break;
        }
    }
}

/// A running turn's leader pid as an empty file named for it, removed on drop. One that
/// outlives its Recur names an orphan for [`reap_stale_turns`].
pub struct PidFile(Option<PathBuf>);

impl PidFile {
    pub fn create(dir: &Path, pid: u32) -> Self {
        if dir.as_os_str().is_empty() || pid == 0 {
            return Self(None);
        }
        let path = dir.join(pid.to_string());
        let ok = std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(&path, ""))
            .is_ok();
        Self(ok.then_some(path))
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// SIGKILLs every session in `dir` that is still a `claude -p` turn, and clears `dir`. A
/// recycled pid is skipped: it must still lead its own session with Recur's turn flag on argv.
pub async fn reap_stale_turns(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let _ = std::fs::remove_file(e.path());
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(p) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
            continue;
        };
        if session_of(p) != Some(p) || !is_turn(pid).await {
            continue;
        }
        for _ in 0..3 {
            if signal_session(pid, Some(Signal::KILL)).await == 0 {
                break;
            }
        }
    }
}

async fn is_turn(pid: u32) -> bool {
    tokio::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .await
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("--disable-slash-commands"))
}

/// Signals every process in session `sid` (`None` only counts); returns how many it found
/// besides the leader.
async fn signal_session(sid: u32, sig: Option<Signal>) -> usize {
    let Some(sid) = i32::try_from(sid).ok().and_then(Pid::from_raw) else {
        return 0;
    };
    let Ok(out) = tokio::process::Command::new("ps")
        .args(["-axo", "pid="])
        .output()
        .await
    else {
        return 0;
    };
    let mut n = 0;
    for pid in String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|p| p.parse().ok().and_then(Pid::from_raw))
    {
        if session_of(pid) == Some(sid) {
            if let Some(sig) = sig {
                let _ = kill_process(pid, sig);
            }
            n += usize::from(pid != sid);
        }
    }
    n
}

/// rustix's getsid assumes a nonzero sid, but Linux kernel threads (listed by `ps`) report 0.
fn session_of(pid: Pid) -> Option<Pid> {
    #[allow(unsafe_code)]
    // SAFETY: getsid only reads the process table.
    let sid = unsafe { libc::getsid(pid.as_raw_nonzero().get()) };
    Pid::from_raw(sid.max(0))
}

pub fn signal_group(pid: u32, sig: Signal) {
    // new_session and process_group(0) both make pgid == pid.
    if let Some(pgid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(pgid, sig);
    }
}

/// A name with a slash is checked as is; otherwise the first executable regular
/// file named `name` on `$PATH` (empty entries mean the current directory).
pub fn look_path(name: &str) -> Result<PathBuf, String> {
    let not_found = || format!("exec: {}: executable file not found in $PATH", quote(name));
    if name.contains('/') {
        return if executable(Path::new(name)) {
            Ok(PathBuf::from(name))
        } else {
            Err(format!(
                "exec: {}: {}",
                quote(name),
                match std::fs::metadata(name) {
                    Ok(_) => "permission denied".to_owned(),
                    Err(e) => e.to_string(),
                }
            ))
        };
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        let cand = dir.join(name);
        if executable(&cand) {
            return Ok(cand);
        }
    }
    Err(not_found())
}

fn executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// The exit text for a failed exit (`exit status 1`, `signal: killed`); `None` for success.
pub fn exit_error(st: ExitStatus) -> Option<String> {
    if st.success() {
        return None;
    }
    if let Some(code) = st.code() {
        return Some(format!("exit status {code}"));
    }
    let sig = st.signal().unwrap_or_default();
    let core = if st.core_dumped() {
        " (core dumped)"
    } else {
        ""
    };
    Some(format!("signal: {}{core}", signal_name(sig)))
}

/// Signal names for the signals a CLI realistically dies of.
fn signal_name(sig: i32) -> String {
    let name = match sig {
        1 => "hangup",
        2 => "interrupt",
        3 => "quit",
        4 => "illegal instruction",
        #[cfg(target_os = "macos")]
        5 => "trace/BPT trap",
        #[cfg(not(target_os = "macos"))]
        5 => "trace/breakpoint trap",
        #[cfg(target_os = "macos")]
        6 => "abort trap",
        #[cfg(not(target_os = "macos"))]
        6 => "aborted",
        #[cfg(target_os = "macos")]
        7 => "EMT trap",
        #[cfg(not(target_os = "macos"))]
        7 => "bus error",
        8 => "floating point exception",
        9 => "killed",
        #[cfg(target_os = "macos")]
        10 => "bus error",
        #[cfg(not(target_os = "macos"))]
        10 => "user defined signal 1",
        11 => "segmentation fault",
        #[cfg(target_os = "macos")]
        12 => "bad system call",
        #[cfg(not(target_os = "macos"))]
        12 => "user defined signal 2",
        13 => "broken pipe",
        14 => "alarm clock",
        15 => "terminated",
        _ => return format!("signal {sig}"),
    };
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_texts() {
        assert_eq!(exit_error(ExitStatus::from_raw(0)), None);
        assert_eq!(
            exit_error(ExitStatus::from_raw(1 << 8)).as_deref(),
            Some("exit status 1")
        );
        assert_eq!(
            exit_error(ExitStatus::from_raw(9)).as_deref(),
            Some("signal: killed")
        );
        assert_eq!(
            exit_error(ExitStatus::from_raw(15)).as_deref(),
            Some("signal: terminated")
        );
    }

    /// A tool job that moved to its own group and was orphaned to launchd (`nohup … &` from a
    /// shell that exited) still dies with the session.
    #[tokio::test]
    async fn kill_session_reaches_orphaned_groups() {
        use tokio::io::AsyncBufReadExt;
        let mut cmd = Command::new("sh");
        new_session(&mut cmd);
        let mut child = cmd
            // perl, not `set -m`: dash turns job control off without a tty.
            .args([
                "-c",
                r#"(perl -e '$|=1; setpgrp; print "$$\n"; exec @ARGV' sleep 30 &); exec sleep 30"#,
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let leader = child.id().unwrap();
        let mut line = String::new();
        tokio::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .await
            .unwrap();
        let orphan = Pid::from_raw(line.trim().parse().unwrap()).unwrap();
        assert_ne!(
            rustix::process::getpgid(Some(orphan)).unwrap(),
            Pid::from_raw(leader as i32).unwrap(),
            "the job must sit outside the leader's group"
        );
        assert_eq!(
            rustix::process::getsid(Some(orphan)).unwrap(),
            Pid::from_raw(leader as i32).unwrap()
        );

        let mut reaped = reap(child, async {});
        kill_session(leader, &mut reaped).await;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while rustix::process::test_kill_process(orphan).is_ok() {
            assert!(std::time::Instant::now() < deadline, "orphan survived");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// A crashed Recur's turn dies with its tool jobs; a live session without the turn flag
    /// (a recycled pid) is left alone.
    #[tokio::test]
    async fn reap_stale_turns_kills_orphans_only() {
        use tokio::io::AsyncBufReadExt;
        let spawn = |flag: &str| {
            let mut cmd = Command::new("sh");
            new_session(&mut cmd);
            cmd.args(["-c", "sleep 30 & echo $!; wait", "sh", flag])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        };
        let mut turn = spawn("--disable-slash-commands");
        let mut other = spawn("--other");
        let mut line = String::new();
        tokio::io::BufReader::new(turn.stdout.take().unwrap())
            .read_line(&mut line)
            .await
            .unwrap();
        let job = Pid::from_raw(line.trim().parse().unwrap()).unwrap();

        let dir = tempfile::tempdir().unwrap();
        for c in [&turn, &other] {
            std::mem::forget(PidFile::create(dir.path(), c.id().unwrap()));
        }
        reap_stale_turns(dir.path()).await;

        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
        let st = tokio::time::timeout(Duration::from_secs(5), turn.wait())
            .await
            .expect("turn survived")
            .unwrap();
        assert_eq!(st.signal(), Some(9));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while rustix::process::test_kill_process(job).is_ok() {
            assert!(std::time::Instant::now() < deadline, "tool job survived");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            other.try_wait().unwrap().is_none(),
            "non-turn session killed"
        );
        signal_group(other.id().unwrap(), Signal::KILL);
    }

    #[test]
    fn look_path_misses() {
        assert_eq!(
            look_path("recur-no-such-binary-xyz").unwrap_err(),
            r#"exec: "recur-no-such-binary-xyz": executable file not found in $PATH"#
        );
        assert!(look_path("sh").is_ok());
        assert!(look_path("/bin/sh").is_ok());
        assert!(look_path("/no/such/x").is_err());
    }
}
