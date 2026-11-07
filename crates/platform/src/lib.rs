//! OS integration: single-instance lock and open IPC, PATH probing and CLI
//! install, keep-awake, system fonts, notifications, dock badge, updater.
//!
//! Paths are always explicit arguments (the data dir is resolved by
//! `recur-store`). macOS and Linux are supported.

pub mod cliinstall;
pub mod dock;
pub mod identity;
pub mod notify;
pub mod open_boot;
pub mod openipc;
pub mod osfonts;
pub mod power;
pub mod updater;
pub mod window_state;

mod pathutil;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Whether this binary runs from inside a macOS app bundle
/// (`…/Recur.app/Contents/MacOS/recur`). Native notifications and the
/// updater require it: `UNUserNotificationCenter` aborts unbundled binaries.
pub fn running_in_app_bundle() -> bool {
    std::env::current_exe().is_ok_and(|exe| in_app_bundle(&exe))
}

fn in_app_bundle(exe: &Path) -> bool {
    exe.to_string_lossy().contains(".app/Contents/MacOS/")
}

/// Raises the open-file soft limit toward the hard limit, capped at macOS's
/// `OPEN_MAX` (10240). A Dock-launched app inherits launchd's soft limit of
/// 256, which terminals, agents, language servers and git spawns exhaust.
pub fn raise_fd_limit() {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    const OPEN_MAX: u64 = 10240;
    let lim = getrlimit(Resource::Nofile);
    let want = lim.maximum.map_or(OPEN_MAX, |max| max.min(OPEN_MAX));
    if lim.current.is_some_and(|cur| cur < want) {
        let _ = setrlimit(
            Resource::Nofile,
            Rlimit {
                current: Some(want),
                maximum: lim.maximum,
            },
        );
    }
}

/// `os.UserHomeDir` on Unix: `$HOME`, or `None` when unset or empty.
pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// `exec.LookPath`: the first executable regular file named `name` on PATH.
pub(crate) fn look_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
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

/// Result of [`run_with_timeout`].
pub(crate) struct Finished {
    pub success: bool,
    pub output: Vec<u8>,
}

/// Runs `cmd` with stdin closed and returns its stdout (plus stderr when
/// `combined`) once it exits, or `None` when it cannot start or outlives
/// `timeout` (the child is then killed).
pub(crate) fn run_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
    combined: bool,
) -> Option<Finished> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if combined {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .spawn()
        .ok()?;
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|mut r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = r.read_to_end(&mut buf);
            buf
        })
    })
    .collect();
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut output = Vec::new();
                for r in readers {
                    output.extend(r.join().unwrap_or_default());
                }
                return Some(Finished {
                    success: status.success(),
                    output,
                });
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_bundle_detection() {
        assert!(in_app_bundle(Path::new(
            "/Applications/Recur.app/Contents/MacOS/recur"
        )));
        assert!(!in_app_bundle(Path::new("/usr/local/bin/recur")));
        assert!(!in_app_bundle(Path::new("/tmp/Recur.app/recur")));
        assert!(
            !running_in_app_bundle(),
            "the test binary is not in a .app bundle"
        );
    }

    #[test]
    fn raises_fd_limit() {
        use rustix::process::{Resource, getrlimit};
        let lim = getrlimit(Resource::Nofile);
        rustix::process::setrlimit(
            Resource::Nofile,
            rustix::process::Rlimit {
                current: Some(256),
                maximum: lim.maximum,
            },
        )
        .expect("lower soft limit");
        raise_fd_limit();
        let want = lim.maximum.map_or(10240, |max| max.min(10240));
        assert_eq!(getrlimit(Resource::Nofile).current, Some(want));
    }

    #[test]
    fn run_with_timeout_reports_output_and_kills() {
        let done = run_with_timeout(
            Command::new("sh").args(["-c", "echo out; echo err >&2"]),
            Duration::from_secs(5),
            true,
        )
        .expect("finished");
        assert!(done.success);
        assert_eq!(String::from_utf8_lossy(&done.output), "out\nerr\n");

        let start = Instant::now();
        assert!(
            run_with_timeout(
                Command::new("sleep").arg("5"),
                Duration::from_millis(100),
                false
            )
            .is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
