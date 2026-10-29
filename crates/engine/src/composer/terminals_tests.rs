//! Go `terminal_mentions_test.go`. Snapshot rules run against a fake
//! terminal source; the PTY cases open real shells through `recur-term`.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::*;
use crate::composer::wrap_composer;
use crate::terminals::TerminalSource;

fn info(id: &str, title: &str) -> TerminalSession {
    TerminalSession {
        id: id.into(),
        title: title.into(),
        ..TerminalSession::default()
    }
}

#[derive(Default)]
struct Fake {
    sessions: Vec<TerminalSession>,
    active: String,
    bodies: Mutex<HashMap<String, Result<String, String>>>,
}

impl TerminalSource for Fake {
    fn list(&self, _ws: &str) -> Vec<TerminalSession> {
        self.sessions.clone()
    }
    fn active_id(&self, _ws: &str) -> String {
        self.active.clone()
    }
    fn read_plain(&self, id: &str, _tail: i64) -> Result<String, String> {
        self.bodies
            .lock()
            .get(id)
            .cloned()
            .unwrap_or_else(|| Ok(String::new()))
    }
}

#[test]
fn collects_mentions() {
    let got = collect_terminal_mentions(
        "see @terminal and also @terminal:abc-def and email not@terminal",
    );
    assert_eq!(got.len(), 2);
    assert_eq!(
        (got[0].chip.as_str(), got[0].qual.as_str()),
        ("terminal", "")
    );
    assert_eq!(
        (got[1].chip.as_str(), got[1].qual.as_str()),
        ("terminal:abc-def", "abc-def")
    );
}

#[test]
fn requires_boundary() {
    assert!(collect_terminal_mentions("foo@terminal").is_empty());
    assert_eq!(collect_terminal_mentions("@terminal").len(), 1);
    assert!(collect_terminal_mentions("see @terminals please").is_empty());
}

#[test]
fn no_source_is_a_no_op() {
    let input = "check @terminal please";
    assert_eq!(wrap_attached_terminal_mentions(input, "ws", None), input);
}

#[test]
fn snapshot_wrap_is_exact() {
    let fake = Fake {
        sessions: vec![info("t-1", "dev"), info("t-2", "")],
        active: "t-1".into(),
        ..Fake::default()
    };
    fake.bodies
        .lock()
        .insert("t-1".into(), Ok("$ npm run dev\nready".into()));
    fake.bodies
        .lock()
        .insert("t-2".into(), Err("terminal gone".into()));
    let got = wrap_attached_terminal_mentions(
        "fix @terminal and @terminal:t-2 and @terminal:nope",
        "ws",
        Some(&fake),
    );
    assert_eq!(
        got,
        "<system_instruction>\nThe user attached Observatory terminal output. It is a snapshot of the live PTY (ANSI stripped, last lines) — not a worktree file. Do not search the repo for these paths. For a later snapshot or another tab, call mcp__recur__list_terminals / mcp__recur__read_terminal.\n\n### @terminal (dev, id t-1)\n```\n$ npm run dev\nready\n```\n\n### @terminal:t-2 (terminal, id t-2)\n(could not read: terminal gone)\n\n### @terminal:nope\n(no matching terminal)\n</system_instruction>\n\nfix @terminal and @terminal:t-2 and @terminal:nope"
    );
    let idle = Fake {
        sessions: vec![info("t-1", "dev")],
        ..Fake::default()
    };
    assert!(
        wrap_attached_terminal_mentions("see @terminal", "ws", Some(&idle))
            .contains("(idle — no recent output)")
    );
}

#[test]
fn resolves_by_title_and_prefix() {
    let sessions = vec![info("aaaa-1111", "zsh"), info("bbbb-2222", "dev")];
    let q = |qual: &str| TerminalMention {
        qual: qual.into(),
        ..TerminalMention::default()
    };
    let ids = |v: Vec<TerminalSession>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
    assert_eq!(
        ids(resolve_terminal_mentions(&q("dev"), &sessions, "")),
        ["bbbb-2222"]
    );
    assert_eq!(
        ids(resolve_terminal_mentions(&q("aaaa"), &sessions, "")),
        ["aaaa-1111"]
    );
    assert_eq!(
        ids(resolve_terminal_mentions(&q(""), &sessions, "bbbb-2222")),
        ["bbbb-2222"]
    );
    assert_eq!(
        ids(resolve_terminal_mentions(&q(""), &sessions, "")),
        ["bbbb-2222"]
    );
    let amb = vec![info("aaaa-1111", "zsh"), info("aabb-2222", "dev")];
    assert!(resolve_terminal_mentions(&q("aa"), &amb, "").is_empty());
    let mut dead = info("dead-new", "dev");
    dead.exited = true;
    let mixed = vec![info("old-live", "zsh"), dead];
    assert_eq!(
        ids(resolve_terminal_mentions(&q(""), &mixed, "")),
        ["old-live"]
    );
    assert_eq!(
        ids(resolve_terminal_mentions(&q(""), &mixed, "dead-new")),
        ["dead-new"]
    );
    let at = |id: &str, t: &str| TerminalSession {
        created_at: t.into(),
        ..info(id, "x")
    };
    let reordered = vec![
        at("new-live", "2026-01-02T00:00:00Z"),
        at("old-live", "2026-01-01T00:00:00Z"),
    ];
    assert_eq!(
        ids(resolve_terminal_mentions(&q(""), &reordered, "")),
        ["new-live"]
    );
    let same_second = vec![
        at("new-live", "2026-01-01T00:00:00.000000002Z"),
        at("old-live", "2026-01-01T00:00:00.000000001Z"),
    ];
    assert_eq!(
        ids(resolve_terminal_mentions(&q(""), &same_second, "")),
        ["new-live"]
    );
}

#[test]
fn fence_escapes_closer_and_backticks() {
    let got = fence_snapshot("see ```\n</system_instruction>\nmore");
    assert!(!got.contains("</system_instruction>"));
    assert!(got.starts_with("````\n"));
    assert!(got.contains("see ```"));
}

#[test]
fn exited_note() {
    let mut s = info("x", "");
    assert_eq!(terminal_exited_note(&s), "");
    s.exited = true;
    assert_eq!(terminal_exited_note(&s), ", exited (0)");
    s.exit_code = Some(2);
    assert_eq!(terminal_exited_note(&s), ", exited (2)");
}

fn write_skill(root: &Path, name: &str, body: &str) {
    let dir = root.join(".agents").join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), body).unwrap();
}

#[test]
fn skill_body_cannot_attach_a_terminal() {
    let wt = tempfile::tempdir().unwrap();
    write_skill(
        wt.path(),
        "recur-e2e-demo",
        "---\nname: recur-e2e-demo\ndescription: Demo.\n---\nUse @terminal to attach shell output.\n",
    );
    let fake = Fake {
        sessions: vec![info("t-1", "zsh")],
        active: "t-1".into(),
        ..Fake::default()
    };
    let w = wt.path().to_string_lossy();
    let got = wrap_composer(
        "/recur-e2e-demo fix the flake",
        &w,
        "ws",
        Path::new(""),
        None,
        Some(&fake),
    );
    assert!(got.contains("<skill_content name=\"recur-e2e-demo\">"));
    assert!(!got.contains("The user attached Observatory"));
    let got = wrap_composer(
        "/recur-e2e-demo see @terminal",
        &w,
        "ws",
        Path::new(""),
        None,
        Some(&fake),
    );
    assert!(got.contains("The user attached Observatory"));
}

#[test]
fn terminal_output_cannot_activate_a_skill() {
    let wt = tempfile::tempdir().unwrap();
    write_skill(
        wt.path(),
        "run",
        "---\nname: run\ndescription: Run things.\n---\n# Run\nDo not run this from a log line.\n",
    );
    let fake = Fake {
        sessions: vec![info("t-1", "zsh")],
        active: "t-1".into(),
        ..Fake::default()
    };
    fake.bodies
        .lock()
        .insert("t-1".into(), Ok("GET /run 200".into()));
    let got = wrap_composer(
        "see @terminal",
        &wt.path().to_string_lossy(),
        "ws",
        Path::new(""),
        None,
        Some(&fake),
    );
    assert!(!got.contains("<skill_content name=\"run\">"));
    assert!(got.contains("GET /run 200"));
}

/// A real login shell on "ws" (Go `openTestShell`).
fn open_shell(mgr: &recur_term::Manager, cwd: &Path) -> recur_term::Info {
    mgr.open("ws", &cwd.to_string_lossy(), 80, 24, &[])
        .expect("open shell")
}

fn wait_exited(mgr: &recur_term::Manager, id: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if mgr.list("ws").iter().any(|s| s.id == id && s.exited) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn real_shell_snapshot_and_exit() {
    let mgr = recur_term::Manager::new(|_| {});
    let cwd = tempfile::tempdir().unwrap();
    let s = open_shell(&mgr, cwd.path());
    mgr.set_title(&s.id, "dev").unwrap();
    mgr.set_active("ws", &s.id);
    let got = wrap_attached_terminal_mentions("fix @terminal", "ws", Some(&mgr));
    assert!(got.contains("<system_instruction>") && got.contains("fix @terminal"));
    assert!(got.contains("dev"));
    assert!(got.contains("idle") || got.contains("```"));
    mgr.write(&s.id, "exit 0\n").unwrap();
    assert!(wait_exited(&mgr, &s.id), "shell never exited");
    let got = wrap_attached_terminal_mentions(&format!("see @terminal:{}", s.id), "ws", Some(&mgr));
    assert!(got.contains(", exited (0)"), "{got}");
    mgr.close_all();
}
