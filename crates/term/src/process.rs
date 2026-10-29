use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, getpgrp};

/// True when the tty's foreground group is the login shell itself (or unknown).
pub(crate) fn is_shell_foreground(fg: i32, shell_pgid: i32) -> bool {
    fg <= 1 || fg == shell_pgid
}

pub(crate) fn signal_group(pgid: i32, sig: Signal) {
    if pgid <= 1 {
        return;
    }
    let _ = killpg(Pid::from_raw(pgid), sig);
}

/// True once no process remains in `pgid` (`kill(-pgid, 0)` reports ESRCH).
pub(crate) fn group_gone(pgid: i32) -> bool {
    if pgid <= 1 {
        return true;
    }
    killpg(Pid::from_raw(pgid), None) == Err(Errno::ESRCH)
}

pub(crate) fn kill_after_grace(groups: &[i32]) {
    if groups.is_empty() {
        return;
    }
    std::thread::sleep(Duration::from_millis(250));
    for &g in groups {
        signal_group(g, Signal::SIGKILL);
    }
}

/// The shell's group (`root`), the foreground group `fg`, and the groups of every descendant of
/// `fg`'s members. Never includes Recur's own group. If `ps` fails it falls back to the root and
/// fg groups.
pub(crate) fn tree_groups(root: i32, fg: i32) -> Vec<i32> {
    let mut groups = vec![root];
    let recur = getpgrp().as_raw();
    if is_shell_foreground(fg, root) || fg == recur {
        return groups;
    }
    groups.push(fg);
    let Ok(out) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid="])
        .output()
    else {
        return groups;
    };
    if !out.status.success() {
        return groups;
    }
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    let mut pgid_of: HashMap<i32, i32> = HashMap::new();
    let mut queue = VecDeque::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let fields: Vec<i32> = line
            .split_whitespace()
            .take(3)
            .filter_map(|f| f.parse().ok())
            .collect();
        if let [pid, ppid, pgid] = fields[..] {
            children.entry(ppid).or_default().push(pid);
            pgid_of.insert(pid, pgid);
            if pgid == fg {
                queue.push_back(pid);
            }
        }
    }
    let mut seen: HashSet<i32> = [root, fg, recur].into_iter().collect();
    while let Some(pid) = queue.pop_front() {
        if let Some(&g) = pgid_of.get(&pid)
            && g > 1
            && seen.insert(g)
        {
            groups.push(g);
        }
        if let Some(kids) = children.get(&pid) {
            queue.extend(kids);
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::time::Instant;

    #[test]
    fn group_gone_tracks_a_real_group() {
        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id() as i32;
        assert!(!group_gone(pgid), "live process group reported gone");
        signal_group(pgid, Signal::SIGKILL);
        let _ = child.wait();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !group_gone(pgid) {
            assert!(Instant::now() < deadline, "process group never disappeared");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn tree_groups_skips_ps_when_the_shell_owns_the_tty() {
        assert_eq!(tree_groups(4242, 4242), vec![4242]);
        assert_eq!(tree_groups(4242, 0), vec![4242]);
    }
}
