//! Engine-wide control: Pause/Stop semantics on registered walks and on rows
//! without one, restart recovery, Shutdown, ClearAll, DeleteProject,
//! CheckGit, and keep-awake. The loop-driving engine cases (end-to-end,
//! governor queueing, pause/resume to PR) are in `loops::tests`.

use std::sync::Arc;
use std::time::Duration;

use recur_domain::{Checkpoint, Escalation, Group, InboxItem, Workspace, WorkspaceState};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::bus::EngineEvent;
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::RunningLoop;
use crate::testing::{TestEngine, TestOptions};

fn row(t: &TestEngine, id: &str, state: WorkspaceState, f: impl FnOnce(&mut Workspace)) {
    let mut w = Workspace {
        id: id.into(),
        project_id: "p".into(),
        run_started: true,
        state,
        group: Group::Active,
        ..Workspace::default()
    };
    f(&mut w);
    t.store.add_workspace(w).unwrap();
}

fn walk(t: &TestEngine, id: &str) -> (Arc<LoopHandle>, CancellationToken) {
    let h = LoopHandle::new(id, "p", id, LoopState::default());
    let c = CancellationToken::new();
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&h),
        cancel: c.clone(),
    }));
    (h, c)
}

#[tokio::test]
async fn pause_and_stop_reach_running_walks() {
    let t = TestEngine::new(None);
    row(&t, "w", WorkspaceState::Coding, |_| {});
    let (h, cancel) = walk(&t, "w");
    t.engine.pause("w");
    assert!(h.is_paused());
    assert_eq!(
        t.store.workspace("w").unwrap().state,
        WorkspaceState::Coding,
        "pause lands at a checkpoint"
    );
    t.engine.stop("w").await;
    assert!(h.is_stopped() && cancel.is_cancelled());
    assert_eq!(
        t.store.workspace("w").unwrap().state,
        WorkspaceState::Coding,
        "a live walk records its own stop"
    );
}

#[tokio::test]
async fn stop_without_a_walk_persists_unless_terminal() {
    let t = TestEngine::new(None);
    row(&t, "q", WorkspaceState::Queued, |w| {
        w.checkpoint = Some(Checkpoint::default());
    });
    t.engine.stop("q").await;
    let q = t.store.workspace("q").unwrap();
    assert_eq!(q.state, WorkspaceState::Stopped);
    assert_eq!(q.phase, "stopped");
    assert!(q.checkpoint.is_none(), "a stop drops the resume position");
    for term in [
        WorkspaceState::PrOpen,
        WorkspaceState::Merged,
        WorkspaceState::Failed,
        WorkspaceState::Stopped,
    ] {
        let id = format!("done-{term:?}");
        row(&t, &id, term.clone(), |_| {});
        t.engine.stop(&id).await;
        assert_eq!(
            t.store.workspace(&id).unwrap().state,
            term,
            "late stop never downgrades"
        );
    }
    row(&t, "pq", WorkspaceState::Queued, |_| {});
    t.engine.pause("pq");
    assert_eq!(
        t.store.workspace("pq").unwrap().state,
        WorkspaceState::Queued,
        "pausing a queued loop is a no-op"
    );
    row(&t, "pz", WorkspaceState::Coding, |_| {});
    t.engine.pause("pz");
    assert_eq!(
        t.store.workspace("pz").unwrap().state,
        WorkspaceState::Coding,
        "pause without a walk is a no-op"
    );
}

#[tokio::test]
async fn controls_after_shutdown_started_are_persisted() {
    let t = TestEngine::new(None);
    row(&t, "a", WorkspaceState::Coding, |_| {});
    row(&t, "b", WorkspaceState::Reviewing, |_| {});
    let (_ha, _) = walk(&t, "a");
    let (_hb, _) = walk(&t, "b");
    *t.engine.teardown.lock() = true;
    t.engine.pause("a");
    t.engine.stop("b").await;
    assert_eq!(
        t.store.workspace("a").unwrap().state,
        WorkspaceState::Paused
    );
    assert_eq!(t.store.workspace("a").unwrap().phase, "paused");
    assert_eq!(
        t.store.workspace("b").unwrap().state,
        WorkspaceState::Stopped
    );
    assert!(t.engine.is_shutting_down());
    assert!(
        t.engine
            .register_chat_turn("", "APP", CancellationToken::new())
            .is_none(),
        "no chats after quit"
    );
}

#[tokio::test]
async fn recover_reconciles_persisted_live_states() {
    let t = TestEngine::new(None);
    row(&t, "coding", WorkspaceState::Coding, |w| w.iteration = 2);
    row(&t, "sched", WorkspaceState::Queued, |w| {
        w.scheduled_prompt_pending = true
    });
    row(&t, "failed", WorkspaceState::Failed, |w| {
        w.group = Group::Needs;
        w.elapsed_label = "running".into();
    });
    row(&t, "esc-answered", WorkspaceState::Escalated, |_| {});
    row(&t, "esc-open", WorkspaceState::Escalated, |_| {});
    row(&t, "queued", WorkspaceState::Queued, |_| {});
    t.store.add_inbox_item(InboxItem {
        id: "ib-f".into(),
        target: "workspace:failed".into(),
        unread: true,
        ..InboxItem::default()
    });
    t.store.add_escalation(Escalation {
        workspace_id: "esc-answered".into(),
        question: "q".into(),
        status: "answered".into(),
        ..Escalation::default()
    });
    t.store.add_escalation(Escalation {
        workspace_id: "esc-open".into(),
        question: "q".into(),
        status: "open".into(),
        ..Escalation::default()
    });

    t.engine.recover();

    let c = t.store.workspace("coding").unwrap();
    assert_eq!(
        (c.state.clone(), c.phase.as_str()),
        (WorkspaceState::Paused, PHASE_INTERRUPTED)
    );
    assert_eq!(c.group, Group::Needs);
    assert_eq!(c.status_label, "Paused");
    assert_eq!(c.last, "interrupted");
    let cp = c.checkpoint.expect("resume position kept");
    assert_eq!((cp.state, cp.iteration), (WorkspaceState::Coding, 2));

    let s = t.store.workspace("sched").unwrap();
    assert_eq!(s.phase, PHASE_SCHEDULED_INTERRUPTED);

    let f = t.store.workspace("failed").unwrap();
    assert_eq!(f.group, Group::Active);
    assert_eq!(f.elapsed_label, "failed");
    assert!(
        t.store
            .inbox()
            .unwrap_or_default()
            .iter()
            .all(|i| i.id != "ib-f")
    );

    let a = t.store.workspace("esc-answered").unwrap();
    assert_eq!(
        (a.state.clone(), a.phase.as_str()),
        (WorkspaceState::Paused, "answered — waiting to resume")
    );
    assert_eq!(
        t.store.workspace("esc-open").unwrap().state,
        WorkspaceState::Escalated
    );
    assert_eq!(
        t.store.workspace("queued").unwrap().state,
        WorkspaceState::Queued
    );
    assert!(t.events.has("inbox.updated"));
}

#[tokio::test]
async fn shutdown_cancels_and_joins() {
    let t = TestEngine::new(None);
    row(&t, "w", WorkspaceState::Coding, |_| {});
    let (_h, cancel) = walk(&t, "w");
    let turn_cancel = CancellationToken::new();
    t.store
        .add_workspace(Workspace {
            id: "chat".into(),
            ..Workspace::default()
        })
        .unwrap();
    let turn = t
        .engine
        .register_chat_turn("chat", "PA", turn_cancel.clone())
        .unwrap();
    let e = Arc::clone(&t.engine);
    let turn2 = Arc::clone(&turn);
    let tc = turn_cancel.clone();
    tokio::spawn(async move {
        tc.cancelled().await;
        e.unregister_chat_turn("chat", "PA", &turn2);
    });
    let stop = CancellationToken::new();
    let stop2 = stop.clone();
    let ticker = tokio::spawn(async move { stop2.cancelled().await });
    {
        let mut sc = t.engine.schedule.lock().await;
        sc.stop = Some(stop.clone());
        sc.ticker = Some(ticker);
    }
    t.engine.watch_workspace("w");
    let started = std::time::Instant::now();
    t.engine.shutdown().await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "shutdown waited for the full cap"
    );
    assert!(cancel.is_cancelled() && turn_cancel.is_cancelled() && stop.is_cancelled());
    assert!(turn.ended().is_cancelled());
    assert_eq!(t.engine.watched_workspace(), "");
    assert!(t.engine.is_shutting_down());
}

#[tokio::test]
async fn clear_all_and_delete_project() {
    let t = TestEngine::new(None);
    t.store
        .add_project(recur_domain::Project {
            id: "p".into(),
            name: "p".into(),
            ..Default::default()
        })
        .unwrap();
    row(&t, "w", WorkspaceState::Coding, |_| {});
    row(&t, "x", WorkspaceState::Queued, |w| {
        w.project_id = "other".into()
    });
    let (_h, cancel) = walk(&t, "w");
    t.engine.delete_project("p").await.unwrap();
    assert!(cancel.is_cancelled(), "the project's walks are stopped");
    assert!(t.store.workspace("w").is_none());
    assert!(t.store.workspace("x").is_some());
    assert!(t.events.has("resource.updated"));

    let (_hx, cx) = walk(&t, "x");
    let _alloc = t.engine.alloc_continue_gen("x", "PA");
    t.engine.clear_all().unwrap();
    assert!(cx.is_cancelled());
    assert!(t.store.workspaces().is_empty());
    assert!(!t.engine.continue_gen_live(_alloc));
}

#[tokio::test]
async fn check_git_publishes_notify_when_too_old() {
    let t = TestEngine::with(TestOptions {
        git_warning: Some((
            String::new(),
            "Recur needs Git 2.31 or newer (found 2.24).".into(),
        )),
        ..TestOptions::default()
    });
    t.engine.check_git();
    match t.events.by_name("notify").last() {
        Some(EngineEvent::Notify(n)) => {
            assert_eq!(n.title, "Git is too old");
            assert_eq!(n.body, "Recur needs Git 2.31 or newer (found 2.24).");
        }
        other => panic!("{other:?}"),
    }
    match t.events.by_name("resource.updated").last() {
        Some(EngineEvent::ResourceUpdated(r)) => {
            assert_eq!(r.git_warning, "Recur needs Git 2.31 or newer (found 2.24).")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        t.store.resource().git_warning,
        "",
        "the warning is never persisted"
    );
}

#[tokio::test]
async fn check_git_title_when_missing_and_silent_when_current() {
    let t = TestEngine::with(TestOptions {
        git_warning: Some((
            "Git was not found".into(),
            "Recur needs Git 2.31 or newer on PATH. Git was not found.".into(),
        )),
        ..TestOptions::default()
    });
    t.engine.check_git();
    match t.events.by_name("notify").last() {
        Some(EngineEvent::Notify(n)) => assert_eq!(n.title, "Git was not found"),
        other => panic!("{other:?}"),
    }
    let ok = TestEngine::new(None);
    ok.engine.check_git();
    assert!(!ok.events.has("notify"));
}

#[tokio::test]
async fn new_treats_zero_caps_as_unlimited() {
    let t = TestEngine::new(None);
    let mut set = t.store.settings();
    set.max_loops = 0;
    set.max_agents = 0;
    t.store.set_settings(set);
    let fresh = crate::Engine::new(
        Arc::clone(&t.store),
        Arc::new(crate::Bus::new()),
        crate::GitSetup::default(),
        t.engine.bundle_roots().clone(),
        None,
        crate::ProviderRegistry::default(),
        crate::ForgeRegistry::default(),
        crate::TrackerRegistry::default(),
        t.data_dir.clone(),
    );
    for i in 0..=recur_domain::DEFAULT_MAX_LOOPS {
        assert!(
            fresh.governor().try_acquire_loop(),
            "slot {i} should be free when MaxLoops=0"
        );
    }
}

#[tokio::test]
async fn apply_governor_caps_publishes_resource() {
    let t = TestEngine::new(None);
    t.engine.apply_governor_caps(1, 1).await;
    assert!(t.engine.governor().try_acquire_loop());
    assert!(!t.engine.governor().try_acquire_loop());
    t.engine.apply_governor_caps(-1, 3).await;
    assert_eq!(
        t.engine.governor().free_loops(),
        recur_domain::DEFAULT_MAX_LOOPS - 1
    );
    assert!(t.events.has("resource.updated"));
}

#[tokio::test]
async fn publish_resource_rollup() {
    let t = TestEngine::new(None);
    row(&t, "n", WorkspaceState::Paused, |w| w.group = Group::Needs);
    assert!(t.engine.governor().try_acquire_loop());
    t.engine.governor().add_iters(2);
    t.engine.publish_resource();
    match t.events.by_name("resource.updated").last() {
        Some(EngineEvent::ResourceUpdated(r)) => {
            assert_eq!((r.loops_running, r.needs_you, r.iters_today), (1, 1, 2));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn set_workspace_state_rules() {
    let t = TestEngine::new(None);
    row(&t, "w", WorkspaceState::Queued, |w| w.group = Group::Needs);
    t.engine
        .set_workspace_state("w", WorkspaceState::Coding, "iteration 1");
    let w = t.store.workspace("w").unwrap();
    assert_eq!(
        (
            w.status_label.as_str(),
            w.progress,
            w.elapsed_label.as_str()
        ),
        ("Coding", 0.5, "running")
    );
    assert_eq!(w.checkpoint.as_ref().unwrap().state, WorkspaceState::Coding);
    t.store.update_workspace("w", |w| w.retryable = true);
    t.engine
        .set_workspace_state("w", WorkspaceState::Failed, "turn failed");
    let w = t.store.workspace("w").unwrap();
    assert_eq!(w.group, Group::Needs, "a retryable failure needs you");
    assert!(w.checkpoint.is_some(), "a failure keeps its checkpoint");
    assert_eq!(
        (w.last.as_str(), w.elapsed_label.as_str()),
        ("failed", "failed")
    );
    t.engine
        .set_workspace_state("w", WorkspaceState::PrOpen, "PR open");
    let w = t.store.workspace("w").unwrap();
    assert_eq!((w.status_label.as_str(), w.progress), ("Ready", 1.0));
    assert!(w.checkpoint.is_none());
    t.engine
        .set_workspace_state("missing", WorkspaceState::Paused, "x");
}

#[cfg(target_os = "macos")]
mod keep_awake {
    use super::*;

    async fn wait_inactive(e: &crate::Engine) -> bool {
        for _ in 0..60 {
            if !e.keep_awake.is_active() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn acquire_agent_keeps_awake_when_enabled() {
        let t = TestEngine::new(None);
        let mut set = t.store.settings();
        set.prevent_sleep_while_agents_run = true;
        t.store.set_settings(set);
        let permit = t
            .engine
            .acquire_agent(&CancellationToken::new())
            .await
            .expect("acquire");
        assert!(
            t.engine.keep_awake.is_active(),
            "caffeinate while a slot is held"
        );
        drop(permit);
        assert!(
            wait_inactive(&t.engine).await,
            "caffeinate stopped after the last release"
        );
        t.engine.keep_awake.close();
    }

    #[tokio::test]
    async fn sync_keep_awake_respects_setting_off() {
        let t = TestEngine::new(None);
        let permit = t
            .engine
            .acquire_agent(&CancellationToken::new())
            .await
            .unwrap();
        assert!(!t.engine.keep_awake.is_active(), "setting off by default");
        let mut set = t.store.settings();
        set.prevent_sleep_while_agents_run = true;
        t.store.set_settings(set.clone());
        t.engine.sync_keep_awake();
        assert!(t.engine.keep_awake.is_active());
        set.prevent_sleep_while_agents_run = false;
        t.store.set_settings(set);
        t.engine.sync_keep_awake();
        assert!(wait_inactive(&t.engine).await);
        permit.release();
        t.engine.keep_awake.close();
    }

    #[tokio::test]
    async fn keep_awake_close_blocks_late_sync() {
        let t = TestEngine::new(None);
        let mut set = t.store.settings();
        set.prevent_sleep_while_agents_run = true;
        t.store.set_settings(set);
        let _permit = t
            .engine
            .acquire_agent(&CancellationToken::new())
            .await
            .unwrap();
        assert!(t.engine.keep_awake.is_active());
        t.engine.keep_awake.close();
        assert!(wait_inactive(&t.engine).await);
        t.engine.sync_keep_awake();
        assert!(
            !t.engine.keep_awake.is_active(),
            "sync after close must not respawn caffeinate"
        );
    }
}

#[tokio::test]
async fn agent_permit_releases_once() {
    let t = TestEngine::new(None);
    t.engine.apply_governor_caps(0, 1).await;
    let p = t
        .engine
        .acquire_agent(&CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(t.engine.governor().agents_running(), 1);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        t.engine.acquire_agent(&cancel).await.is_none(),
        "cap full and cancelled"
    );
    p.release();
    assert_eq!(t.engine.governor().agents_running(), 0);
}
