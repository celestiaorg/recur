//! Port of the `SendWorkspaceMessage` cases of Go `chat_cancel_test.go`:
//! Stop on a live turn (one footer, the calm stop note), on a Loop-mode
//! tab's walk, on the Maestro, in the ticketed prologue and fork window, and
//! the waits a follow-up does on a stopped or ownerless predecessor.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use recur_domain::WorkspaceState;
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use crate::codes::MAESTRO_AGENT_CODE;
use crate::loops::test_support::ctx_runner;
use crate::testing::wait_for;
use crate::turn::finalize::CHAT_STOPPED_REPLY;

fn spawn_send(fx: &Fx, ws: &str, text: &str) -> tokio::task::JoinHandle<String> {
    let (eng, ws, text) = (Arc::clone(fx.eng()), ws.to_owned(), text.to_owned());
    tokio::spawn(async move {
        eng.send_workspace_message(&ws, "PA", &text, "sonnet", None, &[], "", "")
            .await
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_interrupts_a_live_chat_turn() {
    let fx = Fx::new(Some(ctx_runner()));
    let ws = fx.workspace("scratch").await;
    let done = spawn_send(&fx, &ws.id, "long task");
    let st = Arc::clone(&fx.te.store);
    let id = ws.id.clone();
    wait_for(Duration::from_secs(3), "chat agent working", || {
        st.agent(&id, "PA").is_some_and(|a| a.status == "working")
    })
    .await;
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    assert_eq!(join_hang(done, "send after stop").await, CHAT_STOPPED_REPLY);
    wait_for(Duration::from_secs(2), "chat agent idle", || {
        st.agent(&id, "PA").is_some_and(|a| a.status == "idle")
    })
    .await;
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    assert!(
        items
            .iter()
            .any(|it| it.kind == "text" && it.text == CHAT_STOPPED_REPLY)
    );
    assert_eq!(
        items
            .iter()
            .filter(|it| it.kind == "turn" && it.duration_ms > 0)
            .count(),
        1,
        "{items:?}"
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_idles_a_loop_mode_tabs_walk() {
    let fx = Fx::new(Some(ctx_runner()));
    let ws = fx.workspace("scratch").await;
    let reply = fx
        .eng()
        .send_workspace_message(
            &ws.id,
            "PA",
            "review and fix",
            "",
            None,
            &[],
            "loop",
            "Review & fix",
        )
        .await;
    assert_eq!(reply, "");
    let eng = Arc::clone(fx.eng());
    wait_for(Duration::from_secs(3), "specialist running", || {
        eng.governor().agents_running() >= 1
    })
    .await;
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    let (st, id) = (Arc::clone(&fx.te.store), ws.id.clone());
    wait_for(Duration::from_secs(3), "walk idle", || {
        st.workspace(&id)
            .is_some_and(|w| w.state == WorkspaceState::Paused && w.phase == "idle")
    })
    .await;
    assert_ne!(fx.ws(&ws.id).state, WorkspaceState::Stopped);
    wait_for(Duration::from_secs(3), "walk exited", || {
        !eng.loop_key_running(&id)
    })
    .await;
    wait_for(Duration::from_secs(2), "agent slot released", || {
        eng.governor().agents_running() == 0
    })
    .await;
    fx.te.close().await;
}

#[tokio::test]
async fn stop_idles_a_queued_loop_mode_walk() {
    let fx = Fx::new(Some(capture("q", "OK.").provider()));
    let ws = fx.workspace("scratch").await;
    let mut slots = 0;
    while fx.eng().governor().try_acquire_loop() {
        slots += 1;
    }
    fx.eng()
        .send_workspace_message(
            &ws.id,
            "PA",
            "review and fix",
            "",
            None,
            &[],
            "loop",
            "Review & fix",
        )
        .await;
    assert_eq!(fx.ws(&ws.id).state, WorkspaceState::Queued);
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    let got = fx.ws(&ws.id);
    assert_eq!(
        (got.state, got.phase.as_str()),
        (WorkspaceState::Paused, "idle")
    );
    for _ in 0..slots {
        fx.eng().governor().release_loop();
    }
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_interrupts_a_maestro_turn() {
    let fx = Fx::new(Some(ctx_runner()));
    let ws = fx.workspace("scratch").await;
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let done = tokio::spawn(async move { eng.send_maestro_message(&id, "status?", &[]).await });
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    wait_for(Duration::from_secs(3), "maestro turn registered", || {
        eng.live_chat_turn(&id, MAESTRO_AGENT_CODE).is_some()
    })
    .await;
    assert!(fx.eng().cancel_chat(&ws.id, MAESTRO_AGENT_CODE).await);
    assert_eq!(
        join_hang(done, "maestro after stop").await,
        CHAT_STOPPED_REPLY
    );
    let st = Arc::clone(&fx.te.store);
    wait_for(Duration::from_secs(2), "maestro idle", || {
        st.agent(&id, MAESTRO_AGENT_CODE)
            .is_some_and(|a| a.status == "idle")
    })
    .await;
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_stamps_one_footer_from_the_returned_duration() {
    let fx = Fx::new(Some(duration_cancel_runner()));
    let ws = fx.workspace("scratch").await;
    let done = spawn_send(&fx, &ws.id, "long task");
    let (st, id) = (Arc::clone(&fx.te.store), ws.id.clone());
    wait_for(Duration::from_secs(3), "chat agent working", || {
        st.agent(&id, "PA").is_some_and(|a| a.status == "working")
    })
    .await;
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    assert_eq!(join_hang(done, "send after stop").await, CHAT_STOPPED_REPLY);
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    let footers: Vec<i64> = items
        .iter()
        .filter(|it| it.kind == "turn")
        .map(|it| it.duration_ms)
        .collect();
    assert_eq!(footers, [1500], "{items:?}");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_chat_has_no_turn_footer() {
    let fx = Fx::new(Some(duration_cancel_runner()));
    let ws = fx.workspace("scratch").await;
    let done = spawn_send(&fx, &ws.id, "ask something");
    let (st, id) = (Arc::clone(&fx.te.store), ws.id.clone());
    wait_for(Duration::from_secs(3), "chat agent working", || {
        st.agent(&id, "PA").is_some_and(|a| a.status == "working")
    })
    .await;
    assert!(fx.eng().park_chat_turn(&ws.id, "PA"));
    assert_eq!(join_hang(done, "send after park").await, "working on it");
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    assert!(items.iter().all(|it| it.kind != "turn"), "{items:?}");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_unblocks_a_follow_up_waiting_on_an_ownerless_turn() {
    let fx = Fx::new(Some(capture("leak", "OK.").provider()));
    let ws = fx.workspace("leaked wait").await;
    bare_turn(&fx.te, &ws.id, "PA");
    let done = spawn_send(&fx, &ws.id, "go on");
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !done.is_finished(),
        "the send must wait on the leaked handle's end"
    );
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    let got = tokio::time::timeout(Duration::from_secs(2), done)
        .await
        .expect("still blocked after Stop")
        .unwrap();
    assert_eq!(got, CHAT_STOPPED_REPLY);
    fx.te.close().await;
}

async fn stop_in_window(set_hook: impl FnOnce(&Fx, CancellationToken, CancellationToken)) {
    let runs = Arc::new(AtomicUsize::new(0));
    let fx = Fx::new(Some(count_runner(Arc::clone(&runs))));
    let ws = fx.workspace("ticketed").await;
    let (blocked, proceed) = (CancellationToken::new(), CancellationToken::new());
    set_hook(&fx, blocked.clone(), proceed.clone());
    let done = spawn_send(&fx, &ws.id, "start me");
    within_hang(&blocked, "the send reaches the window").await;
    assert!(
        fx.eng().cancel_chat(&ws.id, "PA").await,
        "Stop must record against a ticketed send"
    );
    proceed.cancel();
    assert_eq!(join_hang(done, "send after stop").await, CHAT_STOPPED_REPLY);
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the turn must not run");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_lands_on_a_send_before_it_registers() {
    stop_in_window(|fx, b, p| {
        fx.eng()
            .set_before_chat_claim_hook(gate_hook(move |_| Some((b.clone(), Some(p.clone())))));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_lands_on_a_send_in_the_fork_window() {
    stop_in_window(|fx, b, p| {
        fx.eng()
            .set_before_apply_chat_fork_hook(gate_hook(move |_| {
                Some((b.clone(), Some(p.clone())))
            }));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_after_stop_waits_for_the_predecessor_to_unregister() {
    let (started, unwind) = (CancellationToken::new(), CancellationToken::new());
    let fx = Fx::new(Some(slow_unwind_runner(started.clone(), unwind.clone())));
    let ws = fx.workspace("stop wait owner").await;
    let first = spawn_send(&fx, &ws.id, "long task");
    within_hang(&started, "first turn starts").await;
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    let second = spawn_send(&fx, &ws.id, "go on");
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !second.is_finished(),
        "the second send returned before the first unregistered"
    );
    unwind.cancel();
    join_hang(first, "first send").await;
    join_hang(second, "second send").await;
    fx.te.close().await;
}
