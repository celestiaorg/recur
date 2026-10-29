//! Chat concurrency kernel tests (the send-path cases that drive
//! `SendWorkspaceMessage` move with E4), plus the slot, life-lock and
//! remount-generation contracts they rely on.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use recur_domain::{WORKSPACE_SOLO, Workspace, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::RunningLoop;
use crate::testing::TestEngine;

fn solo(t: &TestEngine, id: &str) {
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            project_id: "p1".into(),
            r#type: WORKSPACE_SOLO.into(),
            short: "hand".into(),
            ..Workspace::default()
        })
        .unwrap();
}

fn bare_turn(t: &TestEngine, ws: &str) -> Arc<super::ChatTurn> {
    t.engine
        .register_chat_turn(ws, "PA", CancellationToken::new())
        .expect("register")
}

#[tokio::test]
async fn try_inject_refuses_after_stop() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let turn = bare_turn(&t, "w");
    assert!(turn.arm_chat_inject().is_some());
    turn.mark_stopped();
    assert!(
        !turn.try_inject("steer", "steer").await,
        "try_inject after Stop must be false"
    );
}

#[tokio::test]
async fn try_inject_refuses_after_park() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let turn = bare_turn(&t, "w");
    assert!(turn.arm_chat_inject().is_some());
    turn.mark_parked();
    assert!(
        !turn.try_inject("steer", "steer").await,
        "try_inject after park must be false"
    );
}

#[tokio::test]
async fn try_inject_without_arm_is_false() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let turn = bare_turn(&t, "w");
    assert!(!turn.try_inject("steer", "steer").await);
    assert!(!t.engine.try_inject_chat("w", "PA", "x", "x").await);
    assert!(
        !t.engine.try_inject_chat("w", "PA2", "x", "x").await,
        "no turn on PA2"
    );
}

#[tokio::test]
async fn try_inject_delivers_with_ack() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let turn = bare_turn(&t, "w");
    let mut inbox = turn.arm_chat_inject().expect("arm");
    assert!(turn.arm_chat_inject().is_none(), "armed twice");
    let reader = tokio::spawn(async move {
        let s = inbox.recv().await.expect("steer");
        let parts = (s.prompt.clone(), s.display.clone());
        s.ack(true);
        parts
    });
    assert!(
        !t.engine.try_inject_chat("w", "", "x", "x").await,
        "an empty code names no tab"
    );
    assert!(
        t.engine
            .try_inject_chat("w", "PA", "  do it  ", " Do it ")
            .await
    );
    let (prompt, display) = reader.await.unwrap();
    assert_eq!(prompt, "do it");
    assert_eq!(display.as_deref(), Some("Do it"));
}

#[tokio::test]
async fn try_inject_blocks_until_stop() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let turn = bare_turn(&t, "w");
    let _inbox = turn.arm_chat_inject().expect("arm");
    let t2 = Arc::clone(&turn);
    let waiter = tokio::spawn(async move { t2.try_inject("steer", "steer").await });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!waiter.is_finished(), "must block until claimed or stopped");
    turn.stop_chat_inject();
    let ok = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("still blocked after stop_chat_inject")
        .unwrap();
    assert!(!ok, "succeeded after inject was disabled");
}

#[tokio::test]
async fn try_inject_unblocks_after_send_when_ack_never_arrives() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let turn = bare_turn(&t, "w");
    let mut inbox = turn.arm_chat_inject().expect("arm");
    let (got_tx, got_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let s = inbox.recv().await;
        let _ = got_tx.send(());
        // Hold the steer unacked.
        tokio::time::sleep(Duration::from_secs(5)).await;
        drop(s);
    });
    let t2 = Arc::clone(&turn);
    let waiter = tokio::spawn(async move { t2.try_inject("steer", "steer").await });
    tokio::time::timeout(Duration::from_secs(2), got_rx)
        .await
        .expect("inject never received")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!waiter.is_finished(), "returned before ack or stop");
    turn.stop_chat_inject();
    let ok = tokio::time::timeout(Duration::from_secs(2), waiter)
        .await
        .expect("still blocked after stop with no ack")
        .unwrap();
    assert!(!ok, "succeeded without an ack");
}

#[tokio::test]
async fn ended_exposed_when_stopped_and_when_healthy() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let turn = bare_turn(&t, "w");
    assert!(
        t.engine.chat_turn_ended("w", "PA").is_some(),
        "healthy live turn must expose ended"
    );
    turn.mark_stopped();
    assert!(
        t.engine.chat_turn_ended("w", "PA").is_some(),
        "stopped turn must expose ended so a post-Stop send waits"
    );
    assert!(t.engine.chat_turn_ended("w", "PA2").is_none());
}

#[tokio::test]
async fn cancel_chat_noop_when_idle() {
    let t = TestEngine::new(None);
    assert!(!t.engine.cancel_chat("missing", "PA").await);
}

#[tokio::test]
async fn cancel_chat_leaves_owned_ended_open() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let cancel = CancellationToken::new();
    let turn = t
        .engine
        .register_chat_turn("w", "PA", cancel.clone())
        .unwrap();
    assert!(t.engine.cancel_chat("w", "PA").await);
    assert!(cancel.is_cancelled());
    assert!(turn.is_stopped());
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !turn.ended().is_cancelled(),
        "owned turn ended closed before unregister"
    );
    t.engine.unregister_chat_turn("w", "PA", &turn);
    assert!(
        turn.ended().is_cancelled(),
        "ended should fire on unregister"
    );
    assert!(!t.engine.workspace_has_live_chat("w"));
}

#[tokio::test]
async fn cancel_chat_closes_unowned_handle() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let turn = bare_turn(&t, "w");
    turn.drop_owner();
    let ended = turn.ended();
    assert!(t.engine.cancel_chat("w", "PA").await);
    assert!(
        ended.is_cancelled(),
        "no owning send: Stop fires ended itself"
    );
    assert!(
        t.engine.live_chat_turn("w", "PA").is_none(),
        "handle dropped"
    );
}

#[tokio::test]
async fn cancel_chat_bumps_stop_gen_for_ticketed_send() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let slot = t.engine.chat_tab_slot("w", "PA");
    let (seq, stop_gen) = slot.begin();
    slot.claim(seq).await;
    assert!(
        t.engine.cancel_chat("w", "PA").await,
        "ticketed prologue must record Stop"
    );
    assert_ne!(
        slot.stop_gen(),
        stop_gen,
        "the send sees Stop and returns without running"
    );
    slot.finish(slot.lock());
    assert!(
        !t.engine.cancel_chat("w", "PA").await,
        "nothing left once the ticket finished"
    );
}

#[tokio::test]
async fn slot_claims_in_begin_order() {
    let t = TestEngine::new(None);
    let slot = t.engine.chat_tab_slot("w", "PA");
    let (s1, _) = slot.begin();
    let (s2, _) = slot.begin();
    let (s3, _) = slot.begin();
    let order = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    for seq in [s3, s1, s2] {
        let slot = Arc::clone(&slot);
        let order = Arc::clone(&order);
        tasks.push(tokio::spawn(async move {
            slot.claim(seq).await;
            order.lock().push(seq);
            tokio::time::sleep(Duration::from_millis(5)).await;
            slot.finish(slot.lock());
        }));
    }
    for task in tasks {
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("FIFO deadlocked")
            .unwrap();
    }
    assert_eq!(*order.lock(), vec![s1, s2, s3]);
}

#[tokio::test]
async fn cancel_chat_idles_live_walk() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let handle = LoopHandle::new(
        "w",
        "p1",
        "w",
        LoopState {
            chat_code: "PA".into(),
            ..LoopState::default()
        },
    );
    let cancel = CancellationToken::new();
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&handle),
        cancel: cancel.clone(),
    }));
    assert!(t.engine.cancel_chat("w", "PA").await, "walk should idle");
    assert!(
        handle.wants_idle(),
        "composer Stop parks idle, not StateStopped"
    );
    assert!(!handle.is_stopped());
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn cancel_chat_idles_queued_loop() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "w".into(),
            project_id: "p1".into(),
            run_started: true,
            loop_chat_code: "PA".into(),
            state: WorkspaceState::Queued,
            phase: "waiting for a slot".into(),
            ..Workspace::default()
        })
        .unwrap();
    assert!(
        t.engine.cancel_chat("w", "PA").await,
        "queued walk should idle"
    );
    let got = t.store.workspace("w").unwrap();
    assert_eq!(got.state, WorkspaceState::Paused);
    assert_eq!(got.phase, "idle");
    assert!(
        !t.engine.cancel_chat("w", "PA2").await,
        "PA2 does not own the run"
    );
}

#[tokio::test]
async fn register_refuses_deleting_archived_and_removed() {
    let t = TestEngine::new(None);
    solo(&t, "ws-del");
    assert!(t.engine.reserve_deleting("ws-del"));
    let cancelled = Arc::new(AtomicBool::new(false));
    let c = CancellationToken::new();
    assert!(
        t.engine
            .register_chat_turn("ws-del", "PA", c.clone())
            .is_none()
    );
    cancelled.store(c.is_cancelled(), Ordering::SeqCst);
    assert!(
        !cancelled.load(Ordering::SeqCst),
        "pre-swap refuse must not cancel"
    );
    assert!(!t.engine.workspace_has_live_chat("ws-del"));
    t.engine.release_deleting("ws-del");
    assert!(
        t.engine
            .register_chat_turn("ws-del", "PA", CancellationToken::new())
            .is_some()
    );

    t.store
        .add_workspace(Workspace {
            id: "ws-arch".into(),
            project_id: "p1".into(),
            archived: true,
            ..Workspace::default()
        })
        .unwrap();
    assert!(
        t.engine
            .register_chat_turn("ws-arch", "PA", CancellationToken::new())
            .is_none()
    );

    solo(&t, "ws-gone");
    t.engine.remove_workspace("ws-gone").await.expect("remove");
    assert!(
        t.engine
            .register_chat_turn("ws-gone", "PA", CancellationToken::new())
            .is_none()
    );
    assert!(!t.engine.workspace_has_live_chat("ws-gone"));
    // App-scoped chats have no workspace.
    assert!(
        t.engine
            .register_chat_turn("", "APP", CancellationToken::new())
            .is_some()
    );
}

#[tokio::test]
async fn register_swaps_out_prior_turn_and_if_idle_refuses() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let c1 = CancellationToken::new();
    let first = t.engine.register_chat_turn("w", "PA", c1.clone()).unwrap();
    let _inbox = first.arm_chat_inject();
    assert!(
        t.engine
            .register_chat_turn_if_idle("w", "PA", CancellationToken::new())
            .is_none()
    );
    let c2 = CancellationToken::new();
    let second = t.engine.register_chat_turn("w", "PA", c2.clone()).unwrap();
    assert!(c1.is_cancelled(), "a tab never runs two agents");
    assert!(
        !first.try_inject("x", "x").await,
        "swapped-out turn stops steering"
    );
    // A stale unregister must not drop the newer turn.
    t.engine.unregister_chat_turn("w", "PA", &first);
    assert!(Arc::ptr_eq(
        &t.engine.live_chat_turn("w", "PA").unwrap(),
        &second
    ));
    t.engine.unregister_chat_turn("w", "PA", &second);
    assert!(
        t.engine
            .register_chat_turn_if_idle("w", "PA", CancellationToken::new())
            .is_some()
    );
}

#[tokio::test]
async fn park_only_parks_the_expected_live_turn() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let c = CancellationToken::new();
    let turn = t.engine.register_chat_turn("w", "PA", c.clone()).unwrap();
    let other = super::ChatTurn::new(CancellationToken::new(), true);
    assert!(!t.engine.park_chat_turn_if("w", "PA", Some(&other)));
    assert!(!t.engine.park_chat_turn_if("w", "PA", None));
    assert!(!c.is_cancelled());
    assert!(t.engine.park_chat_turn("w", "PA"));
    assert!(c.is_cancelled());
    assert!(t.engine.chat_turn_was_parked("w", "PA"));
    assert!(!t.engine.chat_turn_was_timed_out("w", "PA"));

    let c2 = CancellationToken::new();
    let stopped = t.engine.register_chat_turn("w", "PA2", c2.clone()).unwrap();
    stopped.mark_stopped();
    assert!(
        !t.engine.park_chat_turn_if("w", "PA2", Some(&stopped)),
        "park must not override a user Stop"
    );
    let _ = turn;
}

#[tokio::test]
async fn armed_deadline_times_out_the_turn() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let c = CancellationToken::new();
    let turn = t.engine.register_chat_turn("w", "PA", c.clone()).unwrap();
    turn.arm(Duration::from_millis(20));
    tokio::time::timeout(Duration::from_secs(2), c.cancelled())
        .await
        .expect("deadline never fired");
    assert!(t.engine.chat_turn_was_timed_out("w", "PA"));
    assert!(!turn.try_inject("x", "x").await);

    let c2 = CancellationToken::new();
    let t2 = t.engine.register_chat_turn("w", "PA2", c2.clone()).unwrap();
    t2.arm(Duration::from_millis(30));
    t2.disarm();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!c2.is_cancelled(), "disarmed deadline fired");
}

#[tokio::test]
async fn live_turn_lookup_rules() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let _pa = t
        .engine
        .register_chat_turn("w", "PA", CancellationToken::new())
        .unwrap();
    assert!(t.engine.live_chat_turn("w", " PA ").is_some());
    assert!(t.engine.live_chat_turn("w", "").is_none(), "empty code");
    assert!(
        t.engine.live_chat_turn("", "PA").is_none(),
        "empty workspace needs an app code"
    );
    let _app = t
        .engine
        .register_chat_turn("", "AP2", CancellationToken::new())
        .unwrap();
    assert!(t.engine.live_chat_turn("", "AP2").is_some());
}

#[tokio::test]
async fn life_lock_serializes_and_refuses_reserved() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let (mut g1, held) = t.engine.lock_chat_life("w", "").await;
    assert!(held);
    assert!(t.engine.chat_life_busy("w"));
    let e = Arc::clone(&t.engine);
    let second = tokio::spawn(async move {
        let (_g, held) = e.lock_chat_life("w", "PA").await;
        held
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!second.is_finished(), "\"\" and PA share one lock");
    g1.unlock();
    g1.unlock();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("second lock never acquired")
            .unwrap()
    );
    assert!(!t.engine.chat_life_busy("w"), "refs drop to zero");
    assert!(
        t.engine.chat_life.lock().is_empty(),
        "idle slots are retired"
    );

    assert!(t.engine.reserve_deleting("w"));
    let (_g, held) = t.engine.lock_chat_life("w", "PA").await;
    assert!(!held, "a deleting workspace refuses its life lock");
    assert!(!t.engine.reserve_deleting("w"), "one reservation at a time");
    t.engine.release_deleting("w");

    t.engine.hoisting.lock().insert("w".into());
    assert!(
        !t.engine.reserve_deleting("w"),
        "a hoist blocks the reservation"
    );
    let (_g, held) = t.engine.lock_chat_life("w", "PA").await;
    assert!(!held, "a hoisting workspace refuses its life lock");
}

#[tokio::test]
async fn wait_chat_life_idle_returns_after_unlock() {
    let t = TestEngine::new(None);
    solo(&t, "w");
    let (mut g, _) = t.engine.lock_chat_life("w", "MA").await;
    let e = Arc::clone(&t.engine);
    let waiter = tokio::spawn(async move { e.wait_chat_life_idle("w").await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!waiter.is_finished());
    g.unlock();
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("waiter stuck")
        .unwrap();
}

#[tokio::test]
async fn continue_generations() {
    let t = TestEngine::new(None);
    let a = t.engine.alloc_continue_gen("w", "");
    let b = t.engine.alloc_continue_gen("w", "PA2");
    let c = t.engine.alloc_continue_gen("x", "PA");
    assert!(a != b && b != c);
    assert!(t.engine.continue_gen_live(a));
    assert!(!t.engine.continue_gen_live(0));
    t.engine.drop_continue_gen(a);
    assert!(!t.engine.continue_gen_live(a));
    let d = t.engine.alloc_continue_gen("w", "PA");
    t.engine.invalidate_continue_gens("w", "PA2");
    assert!(!t.engine.continue_gen_live(b));
    assert!(t.engine.continue_gen_live(d));
    t.engine.drop_chat_life_for_workspace("w");
    assert!(!t.engine.continue_gen_live(d));
    assert!(t.engine.continue_gen_live(c));
}
