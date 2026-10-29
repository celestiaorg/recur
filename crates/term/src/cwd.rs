use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LSOF_TIMEOUT: Duration = Duration::from_millis(400);

/// The current working directory of `pid`, or "" when it cannot be read. Used when snapshotting
/// tabs so a restore starts in the directory the user `cd`'d to, not the worktree they originally
/// opened the shell in.
pub(crate) fn process_cwd(pid: i32) -> String {
    if pid <= 0 {
        return String::new();
    }
    if cfg!(target_os = "linux")
        && let Ok(p) = std::fs::read_link(format!("/proc/{pid}/cwd"))
    {
        return p.to_string_lossy().into_owned();
    }
    lsof_cwd(pid)
}

fn lsof_cwd(pid: i32) -> String {
    let pid = pid.to_string();
    let Some(out) = output_within(
        Command::new("lsof").args(["-a", "-p", &pid, "-d", "cwd", "-Fn", "-w"]),
        LSOF_TIMEOUT,
    ) else {
        return String::new();
    };
    for line in out.split(|&b| b == b'\n') {
        if line.len() < 2 || line[0] != b'n' {
            continue;
        }
        let path = String::from_utf8_lossy(&line[1..]);
        return match path.find(" (") {
            Some(cut) => path[..cut].to_string(),
            None => path.into_owned(),
        };
    }
    String::new()
}

/// Runs `cmd` and returns its stdout if it exits successfully before `timeout`; the process is
/// killed otherwise (Go's `exec.CommandContext` with a deadline).
pub(crate) fn output_within(cmd: &mut Command, timeout: Duration) -> Option<Vec<u8>> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = reader.join().ok()?;
    status.filter(|s| s.success()).map(|_| out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn same_file(a: &str, b: &std::path::Path) -> bool {
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }

    #[test]
    fn reads_child_directory() {
        let dir = tempfile::tempdir().unwrap();
        let dir = std::fs::canonicalize(dir.path()).unwrap();
        let mut child = Command::new("sleep")
            .arg("30")
            .current_dir(&dir)
            .spawn()
            .unwrap();
        // Each lsof attempt is capped at 400ms; under a loaded test run several in a row can miss.
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut got = String::new();
        while Instant::now() < deadline {
            got = process_cwd(child.id() as i32);
            if same_file(&got, &dir) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(same_file(&got, &dir), "process_cwd = {got:?}, want {dir:?}");
    }
}
