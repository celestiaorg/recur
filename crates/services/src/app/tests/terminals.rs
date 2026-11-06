//! The terminal service and terminal persistence.

use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine as _;
use recur_domain::Workspace;

use super::Fx;
use crate::Services;
use crate::api::{GLOBAL_TERMINALS, TerminalDockChrome};
use crate::app::terminal_persist::{
    PersistedTab, PersistedTerminals, TERMINAL_PERSIST_MODE, persist_delay, snapshot_path,
};

fn same_dir(a: &str, b: &str) -> bool {
    match (Path::new(a).canonicalize(), Path::new(b).canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn decode(replay: &str) -> String {
    let b = base64::engine::general_purpose::STANDARD
        .decode(replay)
        .expect("base64 replay");
    String::from_utf8_lossy(&b).into_owned()
}

/// Detach + attach until `want` is in the replay buffer.
async fn wait_for_replay(s: &Services, id: &str, want: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        s.terminal.detach_terminal(id).await.unwrap();
        if decode(&s.terminal.attach_terminal(id).await.unwrap()).contains(want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {want:?} in the replay buffer");
}

/// Project `p` on a temp checkout and workspace `ws-1` with no worktree.
fn ws1(fx: &Fx) -> String {
    let repo = fx.temp();
    let pid = fx.add_project(&repo);
    add_ws(fx, "ws-1", &pid, "");
    repo
}

fn add_ws(fx: &Fx, id: &str, project: &str, worktree: &str) {
    fx.store()
        .add_workspace(Workspace {
            id: id.into(),
            project_id: project.into(),
            worktree_path: worktree.into(),
            ..Default::default()
        })
        .unwrap();
}

fn tab(title: &str, cwd: &str, replay: &[u8]) -> PersistedTab {
    PersistedTab {
        title: title.into(),
        cwd: cwd.into(),
        replay: replay.to_vec(),
    }
}

fn chrome(open: bool, beside: bool, pct: i64) -> TerminalDockChrome {
    TerminalDockChrome {
        open,
        beside,
        beside_pct: pct,
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_cwd_prefers_the_worktree_then_the_repo() {
    let fx = Fx::new().await;
    let repo = fx.temp();
    let worktree = fx.temp();
    let pid = fx.add_project(&repo);
    add_ws(&fx, "ws-worktree", &pid, &worktree);
    add_ws(&fx, "ws-no-worktree", &pid, "");
    add_ws(
        &fx,
        "ws-stale-worktree",
        &pid,
        &format!("{worktree}/archived-and-deleted"),
    );
    assert!(same_dir(
        &fx.svc.terminal_cwd("ws-worktree").unwrap(),
        &worktree
    ));
    assert!(same_dir(
        &fx.svc.terminal_cwd("ws-no-worktree").unwrap(),
        &repo
    ));
    assert!(
        fx.svc.terminal_cwd("ws-stale-worktree").is_err(),
        "a removed worktree must not fall back to the project checkout"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_cwd_rejects_unknown_workspace() {
    let fx = Fx::new().await;
    let e = fx.svc.terminal_cwd("nope").unwrap_err();
    assert!(e.message.contains("nope"), "{e}");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn global_terminals_start_at_home_with_the_host_env() {
    let fx = Fx::new().await;
    let home = std::env::var("HOME").unwrap();
    assert_eq!(fx.svc.terminal_cwd(GLOBAL_TERMINALS).unwrap(), home);
    assert!(fx.svc.terminal_env(GLOBAL_TERMINALS).is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn global_terminals_persist_across_shutdown() {
    let fx = Fx::new().await;
    ws1(&fx);
    let t = &fx.s.terminal;
    let info = t.open_terminal(GLOBAL_TERMINALS, 80, 24).await.unwrap();
    assert!(same_dir(&info.cwd, &std::env::var("HOME").unwrap()));
    assert!(t.list_terminals("ws-1").await.unwrap().is_empty());
    t.set_terminal_title(&info.id, "scratch").await.unwrap();
    t.set_terminal_dock_chrome(GLOBAL_TERMINALS, &chrome(true, false, 0))
        .await
        .unwrap();
    fx.svc.terminal_shutdown().await;

    let svc2 = fx.reopen().await;
    let t2 = svc2.services().terminal;
    let restored = t2
        .restore_workspace_terminals(GLOBAL_TERMINALS, 80, 24)
        .await
        .unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].title, "scratch");
    assert!(t2.list_terminal_dock_chrome().await.unwrap().is_empty());
    svc2.terminal_shutdown().await;
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_terminal_fails_without_a_resolvable_directory() {
    let fx = Fx::new().await;
    add_ws(&fx, "ws-nowhere", "ghost", "");
    assert!(
        fx.s.terminal
            .open_terminal("ws-nowhere", 80, 24)
            .await
            .is_err()
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn open_attach_and_replay_through_the_bound_service() {
    let fx = Fx::new().await;
    ws1(&fx);
    let t = &fx.s.terminal;
    let info = t.open_terminal("ws-1", 100, 30).await.unwrap();
    let list = t.list_terminals("ws-1").await.unwrap();
    assert_eq!(
        list.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        [info.id.as_str()]
    );
    t.attach_terminal(&info.id).await.unwrap();
    t.write_terminal(&info.id, "echo bound-service-marker\n")
        .await
        .unwrap();
    t.resize_terminal(&info.id, 120, 40).await.unwrap();
    wait_for_replay(&fx.s, &info.id, "bound-service-marker").await;
    t.clear_terminal(&info.id).await.unwrap();
    t.detach_terminal(&info.id).await.unwrap();
    let replay = t.attach_terminal(&info.id).await.unwrap();
    assert!(
        !decode(&replay).contains("bound-service-marker"),
        "cleared output replayed"
    );
    t.close_terminal(&info.id).await.unwrap();
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_persist_round_trip_across_shutdown() {
    let fx = Fx::new().await;
    let repo = ws1(&fx);
    let t = &fx.s.terminal;
    let info = t.open_terminal("ws-1", 80, 24).await.unwrap();
    t.set_terminal_title(&info.id, "dev").await.unwrap();
    t.set_active_terminal("ws-1", &info.id).await.unwrap();
    t.write_terminal(&info.id, "echo persist-marker\n")
        .await
        .unwrap();
    wait_for_replay(&fx.s, &info.id, "persist-marker").await;
    fx.svc.terminal_shutdown().await;

    let svc2 = fx.reopen().await;
    let t2 = svc2.services().terminal;
    let restored = t2
        .restore_workspace_terminals("ws-1", 80, 24)
        .await
        .unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].title, "dev");
    assert!(restored[0].active, "restored tab is active");
    assert!(same_dir(&restored[0].cwd, &repo), "{}", restored[0].cwd);
    let replay = decode(&t2.attach_terminal(&restored[0].id).await.unwrap());
    assert!(replay.contains("persist-marker"), "{replay:?}");
    svc2.terminal_shutdown().await;
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_does_not_duplicate_live_sessions() {
    let fx = Fx::new().await;
    ws1(&fx);
    let t = &fx.s.terminal;
    let info = t.open_terminal("ws-1", 80, 24).await.unwrap();
    let got = t.restore_workspace_terminals("ws-1", 80, 24).await.unwrap();
    assert_eq!(
        got.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        [info.id.as_str()]
    );
    assert_eq!(t.list_terminals("ws-1").await.unwrap().len(), 1);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_last_tab_drops_persisted_snapshot() {
    let fx = Fx::new().await;
    ws1(&fx);
    let t = &fx.s.terminal;
    let info = t.open_terminal("ws-1", 80, 24).await.unwrap();
    fx.svc.term_persist.flush(&fx.svc);
    assert!(fx.svc.read_terminal_snapshot("ws-1").is_some());
    t.close_terminal(&info.id).await.unwrap();
    fx.svc.term_persist.flush(&fx.svc);
    assert!(fx.svc.read_terminal_snapshot("ws-1").is_none());

    let svc2 = fx.reopen().await;
    let got = svc2
        .services()
        .terminal
        .restore_workspace_terminals("ws-1", 80, 24)
        .await
        .unwrap();
    assert!(got.is_empty());
    svc2.terminal_shutdown().await;
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_falls_back_when_saved_cwd_is_gone() {
    let fx = Fx::new().await;
    let repo = ws1(&fx);
    let gone = fx.root.join("deleted-dir").to_string_lossy().into_owned();
    fx.svc
        .write_terminal_snapshot(
            "ws-1",
            &PersistedTerminals {
                tabs: Some(vec![tab("zsh", &gone, b"old-output\r\n")]),
                ..Default::default()
            },
        )
        .unwrap();
    let got =
        fx.s.terminal
            .restore_workspace_terminals("ws-1", 80, 24)
            .await
            .unwrap();
    assert_eq!(got.len(), 1);
    assert!(same_dir(&got[0].cwd, &repo), "{}", got[0].cwd);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_keeps_live_cwd_when_worktree_is_gone() {
    let fx = Fx::new().await;
    let repo = fx.temp();
    let alive = fx.temp();
    let pid = fx.add_project(&repo);
    add_ws(&fx, "ws-1", &pid, &format!("{}/gone", fx.temp()));
    fx.svc
        .write_terminal_snapshot(
            "ws-1",
            &PersistedTerminals {
                tabs: Some(vec![tab("elsewhere", &alive, b"kept\r\n")]),
                ..Default::default()
            },
        )
        .unwrap();
    let got =
        fx.s.terminal
            .restore_workspace_terminals("ws-1", 80, 24)
            .await
            .unwrap();
    assert_eq!(got.len(), 1);
    assert!(same_dir(&got[0].cwd, &alive), "{}", got[0].cwd);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_dock_chrome_round_trip_across_shutdown() {
    let fx = Fx::new().await;
    ws1(&fx);
    let t = &fx.s.terminal;
    t.open_terminal("ws-1", 80, 24).await.unwrap();
    let want = TerminalDockChrome {
        workspace_id: "ws-1".into(),
        open: true,
        maximized: true,
        editor_hidden: true,
        beside: true,
        beside_pct: 40,
    };
    t.set_terminal_dock_chrome("ws-1", &want).await.unwrap();
    fx.svc.terminal_shutdown().await;

    let svc2 = fx.reopen().await;
    let got = svc2
        .services()
        .terminal
        .list_terminal_dock_chrome()
        .await
        .unwrap();
    assert_eq!(got, vec![want]);
    svc2.terminal_shutdown().await;
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_terminal_dock_chrome_closed_drops_maximize() {
    let fx = Fx::new().await;
    add_ws(&fx, "ws-1", "p", "");
    let t = &fx.s.terminal;
    let c = TerminalDockChrome {
        maximized: true,
        editor_hidden: true,
        ..Default::default()
    };
    t.set_terminal_dock_chrome("ws-1", &c).await.unwrap();
    assert!(t.list_terminal_dock_chrome().await.unwrap().is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_terminal_dock_chrome_closed_keeps_beside() {
    let fx = Fx::new().await;
    add_ws(&fx, "ws-1", "p", "");
    let t = &fx.s.terminal;
    t.set_terminal_dock_chrome("ws-1", &chrome(false, true, 40))
        .await
        .unwrap();
    let got = t.list_terminal_dock_chrome().await.unwrap();
    assert_eq!(got.len(), 1);
    assert!(
        !got[0].open && got[0].beside && got[0].beside_pct == 40,
        "{got:?}"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn beside_pct_survives_closed_below_restart() {
    let fx = Fx::new().await;
    add_ws(&fx, "ws-1", "p", "");
    let t = &fx.s.terminal;
    t.set_terminal_dock_chrome("ws-1", &chrome(true, true, 40))
        .await
        .unwrap();
    t.set_terminal_dock_chrome("ws-1", &chrome(false, false, 40))
        .await
        .unwrap();
    let closed_below = |got: &[TerminalDockChrome]| {
        assert_eq!(got.len(), 1);
        assert!(
            got[0].workspace_id == "ws-1"
                && !got[0].open
                && !got[0].beside
                && got[0].beside_pct == 40,
            "{got:?}"
        );
    };
    closed_below(&t.list_terminal_dock_chrome().await.unwrap());
    fx.svc.term_persist.flush(&fx.svc);
    let snap = fx.svc.read_terminal_snapshot("ws-1").expect("snapshot");
    assert!(snap.beside_pct == 40 && !snap.beside && !snap.open);
    fx.svc.terminal_shutdown().await;

    let svc2 = fx.reopen().await;
    closed_below(
        &svc2
            .services()
            .terminal
            .list_terminal_dock_chrome()
            .await
            .unwrap(),
    );
    svc2.terminal_shutdown().await;
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_empty_when_nothing_saved() {
    let fx = Fx::new().await;
    add_ws(&fx, "ws-1", "p", "");
    let got =
        fx.s.terminal
            .restore_workspace_terminals("ws-1", 80, 24)
            .await
            .unwrap();
    assert!(got.is_empty());
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_does_not_erase_snapshot_after_close_all() {
    let fx = Fx::new().await;
    ws1(&fx);
    fx.s.terminal.open_terminal("ws-1", 80, 24).await.unwrap();
    fx.svc.terminal_shutdown().await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let snap = fx.svc.read_terminal_snapshot("ws-1").expect("snapshot");
    assert_eq!(snap.tabs.map(|t| t.len()), Some(1));
    fx.close().await;
}

#[test]
fn persist_delay_caps_trailing_debounce() {
    let now = Instant::now();
    let (debounce, max_age) = (Duration::from_secs(1), Duration::from_secs(5));
    let ago = |ms: u64| Some(now - Duration::from_millis(ms));
    assert_eq!(persist_delay(now, None, debounce, max_age), debounce);
    assert_eq!(
        persist_delay(now, ago(4000), debounce, max_age),
        Duration::from_secs(1)
    );
    assert_eq!(
        persist_delay(now, ago(4500), debounce, max_age),
        Duration::from_millis(500)
    );
    assert_eq!(
        persist_delay(now, ago(5000), debounce, max_age),
        Duration::ZERO
    );
    assert_eq!(
        persist_delay(now, ago(6000), debounce, max_age),
        Duration::ZERO
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn list_terminal_dock_chrome_caches_disk_read() {
    let fx = Fx::new().await;
    add_ws(&fx, "ws-1", "p", "");
    fx.svc
        .write_terminal_snapshot(
            "ws-1",
            &PersistedTerminals {
                open: true,
                maximized: true,
                tabs: Some(vec![tab("zsh", &fx.temp(), &[0; 1024])]),
                ..Default::default()
            },
        )
        .unwrap();
    let t = &fx.s.terminal;
    let open_max = |got: &[TerminalDockChrome]| got.len() == 1 && got[0].open && got[0].maximized;
    assert!(open_max(&t.list_terminal_dock_chrome().await.unwrap()));
    fx.svc.remove_terminal_snapshot("ws-1").unwrap();
    assert!(
        open_max(&t.list_terminal_dock_chrome().await.unwrap()),
        "cached chrome"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn terminal_snapshot_file_mode() {
    use std::os::unix::fs::PermissionsExt as _;
    let fx = Fx::new().await;
    fx.svc
        .write_terminal_snapshot(
            "ws-1",
            &PersistedTerminals {
                tabs: Some(vec![tab("zsh", &fx.temp(), b"")]),
                ..Default::default()
            },
        )
        .unwrap();
    let meta = std::fs::metadata(snapshot_path(&fx.root, "ws-1")).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, TERMINAL_PERSIST_MODE);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_serializes_overlapping_calls() {
    let fx = Fx::new().await;
    let repo = ws1(&fx);
    fx.svc
        .write_terminal_snapshot(
            "ws-1",
            &PersistedTerminals {
                tabs: Some(vec![tab("zsh", &repo, b"hi\r\n")]),
                ..Default::default()
            },
        )
        .unwrap();
    let calls = (0..8).map(|_| fx.s.terminal.restore_workspace_terminals("ws-1", 80, 24));
    for r in futures::future::join_all(calls).await {
        r.unwrap();
    }
    assert_eq!(fx.s.terminal.list_terminals("ws-1").await.unwrap().len(), 1);
    fx.close().await;
}
