//! Process plumbing shared by the turn runner and the status probe: PATH
//! lookup, the single reaper, process-group kill (risk R12), and Go's error
//! wording for spawn and exit failures, which reaches the UI.

use std::future::Future;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::process::Child;
use tokio::sync::watch;

/// How long a SIGTERMed process group gets before SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(2);

/// `None` while running; then `Some(wait error)`, where the error is Go's
/// `cmd.Wait()` text (`exit status 1`, `signal: killed`) and `None` is a
/// clean exit.
pub type Reaped = watch::Receiver<Option<Option<String>>>;

/// The sole owner of `wait()`. `after` runs between the exit and the
/// broadcast (Go's Wait also waits for its stderr copier), so observers see
/// the exit and the captured output together.
pub fn reap(mut child: Child, after: impl Future<Output = ()> + Send + 'static) -> Reaped {
    let (tx, rx) = watch::channel(None);
    tokio::spawn(async move {
        let status = child.wait().await;
        after.await;
        let err = match status {
            Ok(st) => exit_error(st),
            Err(e) => Some(go_errno(&e)),
        };
        let _ = tx.send(Some(err));
    });
    rx
}

pub async fn wait_reaped(reaped: &mut Reaped) {
    let _ = reaped.wait_for(Option::is_some).await;
}

/// SIGTERM to the whole group, then SIGKILL after [`KILL_GRACE`]; returns once
/// the reaper has seen the exit. SIGKILL is only sent while the child is
/// unreaped, so a recycled pid is never signalled.
pub async fn kill_group(pid: u32, reaped: &mut Reaped) {
    signal_group(pid, Signal::TERM);
    if tokio::time::timeout(KILL_GRACE, wait_reaped(reaped))
        .await
        .is_err()
    {
        signal_group(pid, Signal::KILL);
        wait_reaped(reaped).await;
    }
}

pub fn signal_group(pid: u32, sig: Signal) {
    // process_group(0) makes pgid == pid.
    if let Some(pgid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(pgid, sig);
    }
}

/// Go `exec.LookPath`: a name with a slash is checked as is; otherwise the
/// first executable regular file named `name` on `$PATH` (empty entries mean
/// the current directory).
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
                    Err(e) => go_errno(&e),
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

/// Go's `ProcessState.String()` for a failed exit; `None` for success.
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

/// Go's `syscall.Signal.String()` for the signals a CLI realistically dies of.
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

/// Go's errno text: the OS message without Rust's ` (os error N)` suffix,
/// lower-cased first letter (`no such file or directory`).
pub fn go_errno(e: &io::Error) -> String {
    let s = e.to_string();
    let s = match s.rfind(" (os error ") {
        Some(i) if e.raw_os_error().is_some() => &s[..i],
        _ => &s,
    };
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Go's `cmd.Start` failure text for a resolved binary
/// (`fork/exec /x/claude: no such file or directory`).
pub fn start_error(bin: &str, e: &io::Error) -> String {
    format!("fork/exec {bin}: {}", go_errno(e))
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

    #[test]
    fn errno_text() {
        let e = io::Error::from_raw_os_error(2);
        assert_eq!(go_errno(&e), "no such file or directory");
        assert_eq!(
            start_error("/x/claude", &e),
            "fork/exec /x/claude: no such file or directory"
        );
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
