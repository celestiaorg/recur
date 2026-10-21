//! Reaping orphaned bridges from a crashed or force-quit Recur (Go
//! `cursor/bridge_reap.go`). The bridge runs in its own process group, so it
//! outlives its parent; a clean shutdown alone is not enough.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, getpid, kill_process_group, test_kill_process};

use crate::locate::{BRIDGE_DIR_NAME, abs, clean, launcher_rel_path};

use super::BRIDGE_KILL_GRACE;

/// The process-group leader Recur last started for a log dir.
pub(crate) const BRIDGE_PID_FILE: &str = "cursor-bridge.pid";

/// Kills leftover bridge process groups from a prior Recur that crashed, and
/// orphans of Recur-installed bridges from other data dirs (a killed dogfood
/// instance whose data dir was removed never starts again to reap its own).
///
/// Safety rules (Settings probes start throwaway bridges from the same
/// launcher while the provider bridge may be mid-run):
///
/// - No log dir, no reap: throwaway probes pass none and must not scan.
/// - Only orphan leaders die (parent 1, or a dead parent). A live sibling
///   whose parent is this Recur or another running Recur is left alone.
///   `except_pid` is the process just spawned.
///
/// Best-effort: failures to list or signal are ignored.
pub(crate) fn reap_stale_bridges(bin: &Path, log_dir: &Path, except_pid: i32, note: &dyn Fn(&str)) {
    if log_dir.as_os_str().is_empty() {
        return;
    }
    let variants = bridge_path_variants(bin);
    if variants.is_empty() {
        return;
    }
    let mut seen = HashSet::new();
    let mut kill = |pgid: i32, why: &str| {
        if pgid <= 1 || pgid == except_pid || !seen.insert(pgid) {
            return;
        }
        note(&format!(
            "recur: reaping stale cursor-sdk-bridge pgid={pgid} ({why})"
        ));
        kill_process_group_graceful(pgid);
    };

    if let Some(pgid) = read_bridge_pidfile(log_dir) {
        let ours = bridge_process_still_ours(pgid, &variants);
        let orphan = ours && bridge_is_orphan(pgid, process_ppid(pgid));
        if orphan {
            kill(pgid, "pidfile");
        }
        // Drop a stale claim. A live non-orphan keeps it; the next ready
        // refreshes the owner.
        if !ours || orphan {
            clear_bridge_pidfile(log_dir);
        }
    }

    let owned = Path::new("/")
        .join(BRIDGE_DIR_NAME)
        .join(launcher_rel_path());
    let mut all = variants;
    all.push(owned.to_string_lossy().into_owned());
    for pgid in list_orphan_bridge_leaders(&all) {
        kill(pgid, "orphan scan");
    }
}

/// Cleaned absolute forms of `bin`, symlinks resolved too: macOS temp paths
/// show up as both /var/… and /private/var/… in `ps`.
pub(crate) fn bridge_path_variants(bin: &Path) -> Vec<String> {
    if bin.as_os_str().is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: PathBuf| {
        let p = clean(&p).to_string_lossy().into_owned();
        if !p.is_empty() && p != "." && !out.contains(&p) {
            out.push(p);
        }
    };
    add(bin.to_path_buf());
    if let Ok(a) = abs(bin) {
        add(a.clone());
        if let Ok(resolved) = std::fs::canonicalize(&a) {
            add(resolved);
        }
    }
    out
}

fn pid_path(log_dir: &Path) -> PathBuf {
    log_dir.join(BRIDGE_PID_FILE)
}

pub(crate) fn write_bridge_pidfile(log_dir: &Path, pid: i32) {
    use std::os::unix::fs::OpenOptionsExt;
    if log_dir.as_os_str().is_empty() || pid <= 1 {
        return;
    }
    let _ = std::fs::create_dir_all(log_dir);
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(pid_path(log_dir))
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{pid}\n").as_bytes()));
}

pub(crate) fn clear_bridge_pidfile(log_dir: &Path) {
    if log_dir.as_os_str().is_empty() {
        return;
    }
    let _ = std::fs::remove_file(pid_path(log_dir));
}

pub(crate) fn read_bridge_pidfile(log_dir: &Path) -> Option<i32> {
    let raw = std::fs::read_to_string(pid_path(log_dir)).ok()?;
    raw.trim().parse::<i32>().ok().filter(|p| *p > 1)
}

/// `kill(pid, 0)` succeeds.
pub(crate) fn process_alive(pid: i32) -> bool {
    Pid::from_raw(pid).is_some_and(|p| test_kill_process(p).is_ok())
}

/// Alive and its command line still names our launcher: guards against PID
/// reuse after reboot.
fn bridge_process_still_ours(pid: i32, variants: &[String]) -> bool {
    if !process_alive(pid) {
        return false;
    }
    match process_command(pid) {
        // Alive but unreadable (permissions): trust the pidfile.
        None => true,
        Some(cmd) => bridge_command_matches(&cmd, variants),
    }
}

/// Whether a `ps` command line runs one of our launchers. The Node worker
/// runs `…/cursor-sdk-bridge.js` and must not match.
pub(crate) fn bridge_command_matches(command: &str, variants: &[String]) -> bool {
    if command.is_empty() {
        return false;
    }
    variants
        .iter()
        .any(|bin| command.contains(bin.as_str()) && !command.contains(&format!("{bin}.js")))
}

/// Whether a bridge leader was abandoned by a dead parent (a force quit
/// leaves parent 1). Unknown parents (`< 0`) fail closed.
pub(crate) fn bridge_is_orphan(pid: i32, ppid: i32) -> bool {
    if pid <= 1 || ppid < 0 {
        return false;
    }
    if ppid <= 1 {
        return true;
    }
    if Pid::from_raw(ppid).is_some_and(|p| p == getpid()) {
        return false;
    }
    !process_alive(ppid)
}

/// Process-group leaders (pid == pgid) for our launchers whose parent is
/// gone. Signalling the group reaps the Node child too.
fn list_orphan_bridge_leaders(variants: &[String]) -> Vec<i32> {
    // -ww: unlimited width so deep install paths are not truncated.
    let Ok(out) = Command::new("ps")
        .args(["-wwaxo", "pid=,ppid=,pgid=,command="])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| parse_ps_line(line.trim()))
        .filter(|(pid, ppid, pgid, cmd)| {
            pid == pgid && bridge_command_matches(cmd, variants) && bridge_is_orphan(*pid, *ppid)
        })
        .map(|(_, _, pgid, _)| pgid)
        .collect()
}

/// `pid ppid pgid command…`; the command keeps its spaces
/// (`…/Application Support/…`).
pub(crate) fn parse_ps_line(line: &str) -> Option<(i32, i32, i32, String)> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgid = fields.next()?.parse().ok()?;
    fields.next()?;
    let mut rest = line.trim();
    for _ in 0..3 {
        rest = rest
            .trim_start()
            .trim_start_matches(|c: char| !c.is_whitespace())
            .trim_start();
    }
    let command = rest.trim().to_owned();
    if command.is_empty() {
        return None;
    }
    Some((pid, ppid, pgid, command))
}

fn ps_field(pid: i32, field: &str) -> Option<String> {
    let out = Command::new("ps")
        .args(["-wwp", &pid.to_string(), "-o", field])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

fn process_command(pid: i32) -> Option<String> {
    ps_field(pid, "command=")
}

pub(crate) fn process_ppid(pid: i32) -> i32 {
    ps_field(pid, "ppid=")
        .and_then(|s| s.parse().ok())
        .unwrap_or(-1)
}

/// SIGTERM to a process group, then SIGKILL after the grace period.
pub(crate) fn kill_process_group_graceful(pgid: i32) {
    let Some(pg) = Pid::from_raw(pgid).filter(|_| pgid > 1) else {
        return;
    };
    let _ = kill_process_group(pg, Signal::TERM);
    let deadline = Instant::now() + BRIDGE_KILL_GRACE;
    while Instant::now() < deadline {
        if !process_alive(pgid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = kill_process_group(pg, Signal::KILL);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_matches() {
        let bin =
            "/Users/josh/Library/Application Support/Recur/cursor-bridge/bin/cursor-sdk-bridge";
        let v = vec![bin.to_owned()];
        for (cmd, want) in [
            (format!("sh {bin} --port 0"), true),
            (format!("{bin} --port 0"), true),
            ("/Users/josh/Library/Application Support/Recur/cursor-bridge/bin/node /Users/josh/Library/Application Support/Recur/cursor-bridge/bin/../dist/bin/cursor-sdk-bridge.js --port 0".to_owned(), false),
            ("sh /Users/josh/Library/Application Support/Recur-LOOP-1/cursor-bridge/bin/cursor-sdk-bridge --port 0".to_owned(), false),
            (String::new(), false),
        ] {
            assert_eq!(bridge_command_matches(&cmd, &v), want, "{cmd}");
        }
    }

    #[test]
    fn ps_line_keeps_spaced_path() {
        let line = "1759     1  1759 sh /Users/josh/Library/Application Support/Recur/cursor-bridge/bin/cursor-sdk-bridge --port 0";
        let (pid, ppid, pgid, cmd) = parse_ps_line(line).unwrap();
        assert_eq!((pid, ppid, pgid), (1759, 1, 1759));
        assert!(
            cmd.contains("Application Support/Recur/cursor-bridge"),
            "{cmd}"
        );
        assert!(cmd.starts_with("sh "), "{cmd}");
        assert_eq!(parse_ps_line("1 2 3"), None);
    }

    #[test]
    fn orphan_rules() {
        assert!(bridge_is_orphan(42, 1));
        assert!(!bridge_is_orphan(42, getpid().as_raw_nonzero().get()));
        assert!(!bridge_is_orphan(1, 1));
        assert!(!bridge_is_orphan(42, -1));
    }
}
