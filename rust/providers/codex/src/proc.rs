//! Process plumbing: the reaper, process-group kill (risk R12), PATH lookup
//! and Go's wording for OS errors.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use tokio::process::Child;
use tokio::sync::watch;

/// How long a SIGTERMed app-server group gets before SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(2);

/// Flips to true once the process has been waited.
pub type Reaped = watch::Receiver<bool>;

/// The sole owner of `wait()`.
pub fn reap(mut child: Child) -> Reaped {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = child.wait().await;
        let _ = tx.send(true);
    });
    rx
}

pub async fn wait_reaped(reaped: &mut Reaped) {
    let _ = reaped.wait_for(|done| *done).await;
}

/// SIGTERM to the group, SIGKILL after [`KILL_GRACE`]; returns once reaped.
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

fn signal_group(pid: u32, sig: Signal) {
    // process_group(0) makes pgid == pid.
    if let Some(pgid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(pgid, sig);
    }
}

/// Go `exec.LookPath` for a bare name: the first executable regular file on
/// `$PATH` (empty entries mean the current directory).
pub fn look_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| {
            if dir.as_os_str().is_empty() {
                PathBuf::from(".").join(name)
            } else {
                dir.join(name)
            }
        })
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Go `filepath.Abs`: joined onto the working directory, then lexically
/// cleaned.
pub fn abs(p: &Path) -> io::Result<PathBuf> {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abs_cleans() {
        assert_eq!(
            abs(Path::new("/a/./b/../c")).unwrap(),
            PathBuf::from("/a/c")
        );
        assert!(abs(Path::new("x")).unwrap().is_absolute());
    }
}
