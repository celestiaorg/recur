use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::*;
use crate::process::group_gone;

/// Captures emitted events so tests can assert on the stream a client would have seen.
#[derive(Default)]
struct Collector {
    data: Mutex<HashMap<String, Vec<u8>>>,
    exit: Mutex<HashMap<String, i32>>,
    idle: Mutex<HashMap<String, usize>>,
}

impl Collector {
    fn manager(self: &Arc<Self>) -> Manager {
        let c = self.clone();
        Manager::new(move |ev| match ev {
            Event::Data { id, data } => c
                .data
                .lock()
                .entry(id)
                .or_default()
                .extend_from_slice(&data),
            Event::Exit { id, exit_code, .. } => {
                c.exit.lock().insert(id, exit_code);
            }
            Event::Idle { id, .. } => *c.idle.lock().entry(id).or_default() += 1,
            Event::Busy { .. } => {}
        })
    }

    fn output(&self, id: &str) -> String {
        self.data
            .lock()
            .get(id)
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    }

    fn output_len(&self, id: &str) -> usize {
        self.data.lock().get(id).map_or(0, Vec::len)
    }

    fn exited(&self, id: &str) -> Option<i32> {
        self.exit.lock().get(id).copied()
    }

    fn idle_count(&self, id: &str) -> usize {
        self.idle.lock().get(id).copied().unwrap_or(0)
    }
}

/// Polls until `cond` holds or the deadline passes. PTY output is inherently asynchronous.
fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

struct Opened {
    info: Info,
    _dir: tempfile::TempDir,
}

fn open(m: &Manager) -> Opened {
    open_in(m, "ws-1")
}

fn open_in(m: &Manager, ws: &str) -> Opened {
    let dir = tempfile::tempdir().unwrap();
    let info = m
        .open(ws, dir.path().to_str().unwrap(), 80, 24, &[])
        .unwrap();
    Opened { info, _dir: dir }
}

fn silent() -> Manager {
    Manager::new(|_| {})
}

#[test]
fn open_streams_shell_output_to_attached_client() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    m.write(id, "echo recur-marker\n").unwrap();
    wait_for("echoed command output", || {
        c.output(id).contains("recur-marker")
    });
    m.close_all();
}

/// The core detach contract: the shell keeps running while nothing is attached, its output lands
/// in the replay ring, and re-attaching hands that back.
#[test]
fn detach_keeps_shell_running_and_replays_on_reattach() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    wait_for("initial shell output", || !c.output(id).is_empty());
    m.detach(id).unwrap();
    let before = c.output(id);
    m.write(id, "echo while-detached\n").unwrap();
    let mut replay = String::new();
    wait_for("detached output in the replay buffer", || {
        replay = String::from_utf8_lossy(&m.attach(id).unwrap()).into_owned();
        if replay.contains("while-detached") {
            return true;
        }
        m.detach(id).unwrap();
        false
    });
    assert_eq!(c.output(id), before, "term.data was emitted while detached");
    m.close_all();
}

/// Without acks a fast writer must stall rather than queue without bound, and resume the moment
/// the client acks.
#[test]
fn flow_control_stalls_without_acks_and_resumes_after() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    m.write(id, "yes recur-flood\n").unwrap();
    wait_for("output to pass the high-water mark", || {
        c.output_len(id) > HIGH_WATER
    });
    wait_for("the stream to stall", || {
        let a = c.output_len(id);
        std::thread::sleep(Duration::from_millis(150));
        c.output_len(id) == a
    });
    let stalled = c.output_len(id);
    // Bytes are charged at ingest, so the reader can only overshoot by the read that crosses
    // the mark.
    let limit = HIGH_WATER + 2 * READ_CHUNK;
    assert!(
        stalled <= limit,
        "buffered {stalled} bytes, over the {limit} ceiling"
    );
    m.ack(id, stalled).unwrap();
    wait_for("the stream to resume after an ack", || {
        c.output_len(id) > stalled
    });
    m.close_all();
}

/// A read loop parked in flow control must not keep the session alive: detaching releases it.
#[test]
fn detach_releases_a_stalled_read_loop() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = s.info.id.clone();
    m.attach(&id).unwrap();
    m.write(&id, "yes recur-flood\n").unwrap();
    wait_for("output to pass the high-water mark", || {
        c.output_len(&id) > HIGH_WATER
    });
    m.detach(&id).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let m2 = m.clone();
    std::thread::spawn(move || {
        let _ = m2.close(&id);
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("close blocked on a stalled read loop");
    m.close_all();
}

#[test]
fn exit_keeps_tombstone_until_close() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    m.write(id, "exit 3\n").unwrap();
    wait_for("term.exit", || c.exited(id).is_some());
    assert_eq!(c.exited(id), Some(3));
    let got = m.list("ws-1");
    assert!(
        got.len() == 1 && &got[0].id == id && got[0].exited && got[0].exit_code == Some(3),
        "{got:?}"
    );
    m.set_active("ws-1", id);
    assert!(
        !m.list("ws-1")[0].active,
        "exited tombstone must not be active"
    );
    m.read_plain(id, 20).unwrap();
    m.close(id).unwrap();
    assert!(m.get(id).is_err(), "session still registered after close");
    assert!(m.list("ws-1").is_empty());
    m.close_all();
}

#[test]
fn info_json_keeps_clean_exit_code() {
    let b = serde_json::to_string(&Info {
        id: "x".into(),
        exited: true,
        exit_code: Some(0),
        ..Default::default()
    })
    .unwrap();
    assert!(b.contains(r#""exitCode":0"#), "clean exit omitted: {b}");
    let live = serde_json::to_string(&Info {
        id: "y".into(),
        ..Default::default()
    })
    .unwrap();
    assert!(
        !live.contains("exitCode"),
        "live session leaked exitCode: {live}"
    );
    assert!(
        live.contains(r#""workspaceId":"""#) && live.contains(r#""createdAt":"""#),
        "{live}"
    );
}

fn exit_and_wait(m: &Manager, c: &Collector, id: &str) {
    m.write(id, "exit 0\n").unwrap();
    wait_for(&format!("term.exit {id}"), || c.exited(id).is_some());
}

fn exited_count(m: &Manager) -> usize {
    m.list("ws-1").iter().filter(|s| s.exited).count()
}

#[test]
fn prune_exited_tombstones() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let mut keep = Vec::new();
    for _ in 0..MAX_EXITED_TOMBSTONES + 2 {
        let s = open(&m);
        exit_and_wait(&m, &c, &s.info.id);
        keep.push(s);
    }
    wait_for("exited tombstones at cap", || {
        exited_count(&m) == MAX_EXITED_TOMBSTONES
    });
    let live = open(&m);
    let got = m.list("ws-1");
    assert_eq!(got.len(), MAX_EXITED_TOMBSTONES + 1, "cap + live");
    assert_eq!(
        got.iter().filter(|s| s.exited).count(),
        MAX_EXITED_TOMBSTONES
    );
    assert!(
        got.iter().any(|s| s.id == live.info.id),
        "live session was pruned"
    );
    assert!(
        !got.iter().any(|s| s.id == keep[0].info.id),
        "oldest tombstone was not pruned"
    );
    m.close_all();
}

#[test]
fn prune_evicts_oldest_exit_not_oldest_open() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let oldest = open(&m);
    let mut keep = Vec::new();
    for _ in 0..MAX_EXITED_TOMBSTONES {
        let s = open(&m);
        exit_and_wait(&m, &c, &s.info.id);
        keep.push(s);
    }
    exit_and_wait(&m, &c, &oldest.info.id);
    wait_for("exited tombstones at cap", || {
        exited_count(&m) == MAX_EXITED_TOMBSTONES
    });
    let got = m.list("ws-1");
    assert!(
        got.iter().any(|s| s.id == oldest.info.id),
        "just-exited long-lived tab was pruned"
    );
    assert!(
        !got.iter().any(|s| s.id == keep[0].info.id),
        "first-exited later tab was kept"
    );
    m.close_all();
}

#[test]
fn list_returns_workspace_sessions_in_open_order() {
    let m = silent();
    let a = open(&m);
    let b = open(&m);
    let _other = open_in(&m, "ws-2");
    let got = m.list("ws-1");
    assert_eq!(
        got.iter().map(|i| &i.id).collect::<Vec<_>>(),
        vec![&a.info.id, &b.info.id]
    );
    assert_eq!(got[0].workspace_id, "ws-1");
    m.close_all();
}

fn ids(got: &[Info]) -> Vec<String> {
    got.iter().map(|i| i.id.clone()).collect()
}

#[test]
fn reorder_changes_list_order() {
    let m = silent();
    let a = open(&m);
    let b = open(&m);
    let c = open(&m);
    let want = vec![c.info.id.clone(), a.info.id.clone(), b.info.id.clone()];
    m.reorder("ws-1", &want).unwrap();
    assert_eq!(ids(&m.list("ws-1")), want);
    let d = open(&m);
    let got = m.list("ws-1");
    assert_eq!(got.len(), 4);
    assert_eq!(got[3].id, d.info.id);
    m.close(&a.info.id).unwrap();
    assert_eq!(
        ids(&m.list("ws-1")),
        vec![c.info.id.clone(), b.info.id.clone(), d.info.id.clone()]
    );
    m.close_all();
}

#[test]
fn reorder_rejects_non_permutation() {
    let m = silent();
    let a = open(&m);
    let b = open(&m);
    let (a, b) = (a.info.id.clone(), b.info.id.clone());
    assert!(
        m.reorder("ws-1", std::slice::from_ref(&a)).is_err(),
        "incomplete permutation"
    );
    assert!(
        m.reorder("ws-1", &[a.clone(), b.clone(), "nope".into()])
            .is_err(),
        "unknown id"
    );
    assert!(
        m.reorder("ws-1", &[a.clone(), a.clone()]).is_err(),
        "duplicate id"
    );
    assert_eq!(ids(&m.list("ws-1")), vec![a, b]);
    m.close_all();
}

#[test]
fn open_rejects_missing_working_directory() {
    let m = silent();
    assert!(m.open("ws-1", "", 80, 24, &[]).is_err());
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("does-not-exist");
    assert!(
        m.open("ws-1", missing.to_str().unwrap(), 80, 24, &[])
            .is_err()
    );
}

#[test]
fn operations_on_unknown_session_error() {
    let m = silent();
    assert!(m.attach("nope").is_err());
    assert!(m.detach("nope").is_err(), "detach");
    assert!(m.ack("nope", 1).is_err(), "ack");
    assert!(m.write("nope", "x").is_err(), "write");
    assert!(m.resize("nope", 80, 24).is_err(), "resize");
    assert!(m.clear_replay("nope").is_err(), "clear_replay");
    assert!(m.close("nope").is_err(), "close");
    assert!(m.interrupt("nope").is_err(), "interrupt");
    assert!(m.set_title("nope", "x").is_err(), "set_title");
    assert!(m.read_plain("nope", 1).is_err(), "read_plain");
}

#[test]
fn clamp_size_bounds() {
    assert_eq!(clamp_size(120, 40), (120, 40), "passthrough");
    assert_eq!(clamp_size(0, 0), (80, 24), "zero falls back to defaults");
    assert_eq!(clamp_size(-5, -1), (80, 24), "negative falls back");
    assert_eq!(
        clamp_size(99999, 70000),
        (1000, 1000),
        "over-large is capped below u16 wrap"
    );
}

#[test]
fn pty_env_advertises_terminal_capabilities() {
    let vars = [("TERM", "dumb"), ("COLUMNS", "13"), ("PATH", "/bin")]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
    let env = pty_env_from(vars.into_iter());
    let count = |k: &str| {
        env.iter()
            .filter(|kv| kv.as_bytes().starts_with(format!("{k}=").as_bytes()))
            .count()
    };
    assert_eq!(
        (count("TERM"), count("COLORTERM"), count("COLORFGBG")),
        (1, 1, 1)
    );
    assert_eq!(
        count("COLUMNS"),
        0,
        "COLUMNS would fight the real window size"
    );
    assert!(env.iter().any(|kv| kv == "PATH=/bin"));
    for want in [
        "TERM=xterm-256color",
        "COLORTERM=truecolor",
        "TERM_PROGRAM=Recur",
        "COLORFGBG=15;0",
    ] {
        assert!(env.iter().any(|kv| kv == want), "env missing {want}");
    }
}

#[test]
fn merge_env_replaces_matching_keys() {
    let base: Vec<OsString> = ["A=1", "B=2", "=weird", "NOEQ"].map(OsString::from).into();
    let got = merge_env(base, &["B=3".into(), "C=4".into()]);
    assert_eq!(
        got,
        ["A=1", "=weird", "NOEQ", "B=3", "C=4"].map(OsString::from)
    );
}

#[test]
fn shell_env_reaches_the_pty() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let dir = tempfile::tempdir().unwrap();
    let info = m
        .open(
            "ws-1",
            dir.path().to_str().unwrap(),
            80,
            24,
            &["RECUR_TEST_MARK=marked".into()],
        )
        .unwrap();
    m.attach(&info.id).unwrap();
    m.write(
        &info.id,
        "echo \"[$TERM_PROGRAM:$RECUR_TEST_MARK:${COLUMNS:-unset}]\"\n",
    )
    .unwrap();
    wait_for("env echo", || c.output(&info.id).contains("[Recur:marked:"));
    m.close_all();
}

fn wait_foreground_job(m: &Manager, id: &str) -> i32 {
    let s = m.session(id);
    let mut fg = 0;
    wait_for("job to become the foreground group", || {
        fg = s.foreground_pgid().unwrap_or(0);
        fg > 1 && fg != s.pid
    });
    fg
}

#[test]
fn interrupt_kills_foreground_keeps_shell() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    wait_for("initial shell output", || !c.output(id).is_empty());
    m.write(id, "/bin/sleep 60\n").unwrap();
    let fg = wait_foreground_job(&m, id);
    m.interrupt(id).unwrap();
    assert!(
        group_gone(fg),
        "process group {fg} still alive after interrupt"
    );
    m.write(id, "echo after-interrupt\n").unwrap();
    wait_for("shell still accepts commands", || {
        c.output(id).contains("after-interrupt")
    });
    assert!(c.exited(id).is_none(), "interrupt tore down the session");
    m.close_all();
}

#[test]
fn close_kills_job_that_ignores_hangup() {
    for close_all in [false, true] {
        let c = Arc::new(Collector::default());
        let m = c.manager();
        let s = open(&m);
        let id = &s.info.id;
        m.attach(id).unwrap();
        wait_for("initial shell output", || !c.output(id).is_empty());
        m.write(id, "/bin/sh -c \"trap '' HUP; /bin/sleep 60\"\n")
            .unwrap();
        let fg = wait_foreground_job(&m, id);
        if close_all {
            m.close_all();
        } else {
            m.close(id).unwrap();
        }
        wait_for("job group to die after close", || group_gone(fg));
        m.close_all();
    }
}

#[test]
fn close_spares_nohup_background_job() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    // Keep the user's zsh history out of it: a shell killed mid-save leaves a lock that stalls
    // the next shell's startup.
    let zdot = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let env = [format!("ZDOTDIR={}", zdot.path().display())];
    let info = m
        .open("ws-1", dir.path().to_str().unwrap(), 80, 24, &env)
        .unwrap();
    let id = &info.id;
    m.attach(id).unwrap();
    wait_for("initial shell output", || !c.output(id).is_empty());
    m.write(id, "nohup /bin/sleep 60 >/dev/null 2>&1 & echo BG=${!}:\n")
        .unwrap();
    let re = regex::Regex::new(r"BG=(\d+):").unwrap();
    let mut pid = 0;
    wait_for("background pid", || {
        re.captures(&c.output(id))
            .and_then(|m| m[1].parse().ok())
            .inspect(|&p| pid = p)
            .is_some()
    });
    wait_for("nohup to exec sleep", || {
        std::process::Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim().ends_with("sleep"))
    });
    m.close(id).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
    assert!(alive, "nohup job died with the tab");
    m.close_all();
}

#[test]
fn watch_idle_when_command_exits_keeps_shell() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    wait_for("initial shell output", || !c.output(id).is_empty());
    m.write(id, "/bin/sleep 0.25\n").unwrap();
    wait_for("term.idle after sleep", || c.idle_count(id) >= 1);
    assert!(c.exited(id).is_none(), "shell must stay alive");
    m.write(id, "echo still-usable\n").unwrap();
    wait_for("shell still accepts commands", || {
        c.output(id).contains("still-usable")
    });
    m.close_all();
}

#[test]
fn interrupt_on_idle_shell_is_noop() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    wait_for("initial shell output", || !c.output(id).is_empty());
    let start = Instant::now();
    m.interrupt(id).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "interrupt on an idle shell took {:?}",
        start.elapsed()
    );
    m.write(id, "echo still-here\n").unwrap();
    wait_for("shell still accepts commands", || {
        c.output(id).contains("still-here")
    });
    m.close_all();
}

#[test]
fn interrupt_escalates_when_sigint_is_ignored() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    wait_for("initial shell output", || !c.output(id).is_empty());
    m.write(
        id,
        "sh -c 'trap \"\" INT; echo trap-ready; exec sleep 60'\n",
    )
    .unwrap();
    wait_for("trap-ready", || {
        c.output(id).matches("trap-ready").count() >= 2
    });
    let fg = wait_foreground_job(&m, id);
    m.interrupt(id).unwrap();
    assert!(
        group_gone(fg),
        "process group {fg} still alive after interrupt"
    );
    m.write(id, "echo after-escalate\n").unwrap();
    wait_for("shell still accepts commands after SIGTERM", || {
        c.output(id).contains("after-escalate")
    });
    assert!(c.exited(id).is_none(), "interrupt tore down the session");
    m.close_all();
}

#[test]
fn read_plain_strips_replay_and_set_title() {
    let m = silent();
    let s = open(&m);
    let id = &s.info.id;
    m.set_title(id, "dev").unwrap();
    let got = m.list("ws-1");
    assert!(got.len() == 1 && got[0].title == "dev", "{got:?}");
    m.session(id)
        .st
        .lock()
        .replay
        .write(b"ok \x1b[32mpass\x1b[0m\n");
    let plain = m.read_plain(id, 20).unwrap();
    assert!(
        plain.contains("pass") && !plain.contains('\x1b'),
        "{plain:?}"
    );
    m.set_active("ws-1", id);
    assert_eq!(&m.active_id("ws-1"), id);
    assert!(
        m.list("ws-1")[0].active,
        "list should mark the focused tab active"
    );
    m.close_all();
}

#[test]
fn open_with_seeds_replay_and_title() {
    let m = silent();
    let dir = tempfile::tempdir().unwrap();
    let info = m
        .open_with(
            "ws-1",
            dir.path().to_str().unwrap(),
            80,
            24,
            &[],
            OpenOptions {
                title: "dev".into(),
                replay: b"prior-scrollback\r\n".to_vec(),
            },
        )
        .unwrap();
    assert_eq!(info.title, "dev");
    let raw = m.attach(&info.id).unwrap();
    let text = String::from_utf8_lossy(&raw);
    assert!(
        text.contains("prior-scrollback"),
        "replay missing seeded history: {text:?}"
    );
    assert!(
        raw.windows(RESTORE_TRAILER.len())
            .any(|w| w == RESTORE_TRAILER),
        "replay missing restore trailer"
    );
    m.close_all();
}

#[test]
fn refresh_cwd_throttles_failed_lookups() {
    let st = Mutex::new(State::new("", String::new(), Ring::new(0)));
    refresh_cwd(&st, 0);
    let first = st
        .lock()
        .cwd_checked_at
        .expect("stamped after failed lookup");
    refresh_cwd(&st, 0);
    assert_eq!(
        st.lock().cwd_checked_at,
        Some(first),
        "no retry inside the refresh window"
    );
}

#[test]
fn snapshots_follow_list_order_and_skip_tombstones() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let a = open(&m);
    let b = open(&m);
    m.set_title(&b.info.id, "server").unwrap();
    exit_and_wait(&m, &c, &a.info.id);
    let snaps = m.snapshots("ws-1");
    assert_eq!(snaps.len(), 1);
    assert_eq!(snaps[0].id, b.info.id);
    assert_eq!(snaps[0].title, "server");
    assert!(dir_exists(&snaps[0].cwd), "cwd {:?}", snaps[0].cwd);
    m.close_all();
}

#[test]
fn osc7_updates_the_recorded_cwd() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let target = tempfile::tempdir().unwrap();
    let target = target.path().to_str().unwrap().to_string();
    m.write(
        &s.info.id,
        format!("printf '\\033]7;file://localhost{target}\\007'\n"),
    )
    .unwrap();
    wait_for("osc7 cwd", || m.list("ws-1")[0].cwd == target);
    m.close_all();
}

#[test]
fn dirty_hook_fires_for_workspace_changes() {
    let m = silent();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    m.set_on_dirty(move |ws| s2.lock().push(ws.to_string()));
    let s = open(&m);
    assert!(seen.lock().iter().any(|w| w == "ws-1"));
    seen.lock().clear();
    m.set_title(&s.info.id, "x").unwrap();
    assert_eq!(seen.lock().as_slice(), ["ws-1"]);
    m.close_all();
}

#[test]
fn busy_reflects_the_foreground_job() {
    let c = Arc::new(Collector::default());
    let m = c.manager();
    let s = open(&m);
    let id = &s.info.id;
    m.attach(id).unwrap();
    wait_for("initial shell output", || !c.output(id).is_empty());
    assert!(!m.list("ws-1")[0].busy);
    m.write(id, "/bin/sleep 60\n").unwrap();
    wait_for("busy flag", || m.list("ws-1")[0].busy);
    m.close_all();
}

#[test]
fn event_names_match_the_go_bus() {
    let e = Event::Exit {
        id: "a".into(),
        workspace_id: "w".into(),
        exit_code: 0,
    };
    assert_eq!((e.name(), e.session_id()), ("term.exit", "a"));
    assert_eq!(
        Event::Data {
            id: "a".into(),
            data: vec![]
        }
        .name(),
        "term.data"
    );
    assert_eq!(
        Event::Busy {
            id: "a".into(),
            workspace_id: "w".into()
        }
        .name(),
        "term.busy"
    );
    assert_eq!(
        Event::Idle {
            id: "a".into(),
            workspace_id: "w".into()
        }
        .name(),
        "term.idle"
    );
}

#[test]
fn created_at_is_rfc3339_nano() {
    let t = DateTime::parse_from_rfc3339("2026-01-01T00:00:00.120000Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(rfc3339_nano(t), "2026-01-01T00:00:00.12Z");
    let t = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(rfc3339_nano(t), "2026-01-01T00:00:00Z");
}
