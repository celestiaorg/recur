//! Port of the live-turn cases of Go `workspace_chat_test.go`: a follow-up
//! steers the live turn instead of starting a second run (with the life lock
//! released while the steer waits for its ack), FIFO order between racing
//! sends, Stop between ticket and register, the steer's display and
//! surface, no steer after Stop or after the result, waiting out a turn that
//! cannot steer, fresh_session and remount sends, and subagent notices.

use std::sync::Arc;
use std::time::Duration;

use recur_agent::attach::DISPLAY_HIDDEN_SENTINEL;
use recur_domain::{
    CHAT_MODE_AGENT, CHAT_MODE_PLAN, ContentBlock, Message, PROVIDER_CLAUDE_CODE, Session,
};
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use crate::chat::surface::{CHAT_SURFACE_CLOSE, CHAT_SURFACE_OPEN};
use crate::chat::switch_mode::{format_mode_switch_display, format_mode_switch_resume_prompt};
use crate::session_config::AgentConfig;
use crate::turn::finalize::{CHAT_INJECTED_SENTINEL, CHAT_STOPPED_REPLY};
use crate::turn::transcript::FORK_PREAMBLE_LEAD;

fn spawn_send(fx: &Fx, ws: &str, text: &str, model: &str) -> tokio::task::JoinHandle<String> {
    let (eng, ws, text, model) = (
        Arc::clone(fx.eng()),
        ws.to_owned(),
        text.to_owned(),
        model.to_owned(),
    );
    tokio::spawn(async move {
        eng.send_workspace_message(&ws, "PA", &text, &model, None, &[], "", "")
            .await
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follow_up_steers_the_live_turn() {
    for model in ["cursor/composer-2.5", "claude-code/sonnet", "codex/gpt-5.4"] {
        let r = HoldInject::new();
        let fx = Fx::new(Some(r.provider()));
        let ws = fx.workspace(&format!("steer {model}")).await;
        let first = spawn_send(&fx, &ws.id, "do the thing", model);
        within_hang(&r.started, "first turn starts").await;
        let got = fx
            .send(&ws.id, "PA", "please stop and fix tests", model)
            .await;
        assert_eq!(got, CHAT_INJECTED_SENTINEL, "{model}");
        r.release.cancel();
        assert_ne!(join_hang(first, "first turn").await, CHAT_INJECTED_SENTINEL);
        assert_eq!(r.runs(), 1, "{model}: the follow-up must steer, not run");
        assert_eq!(r.injected_users(), ["please stop and fix tests"], "{model}");
        fx.te.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_waiting_for_its_ack_does_not_hold_the_life_lock() {
    let r = HoldInject::with_ack_wait();
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("inject-life").await;
    let first = spawn_send(&fx, &ws.id, "do the thing", "sonnet");
    within_hang(&r.started, "first turn starts").await;
    let follow = spawn_send(&fx, &ws.id, "please stop and fix tests", "sonnet");
    within_hang(&r.got_inject, "follow-up reaches the steer").await;
    let msg = tokio::time::timeout(
        Duration::from_secs(2),
        fx.eng().switch_chat_mode(&ws.id, "PA", CHAT_MODE_PLAN),
    )
    .await
    .expect("switch_mode deadlocked on the life lock held across the ack");
    assert!(msg.contains("plan"), "{msg}");
    r.ack_wait.as_ref().unwrap().cancel();
    assert_eq!(join_hang(follow, "follow-up").await, CHAT_INJECTED_SENTINEL);
    r.release.cancel();
    join_hang(first, "first turn").await;
    fx.te.close().await;
}

#[tokio::test]
async fn workspace_sessions_are_scoped_by_owner_and_code() {
    let fx = Fx::new(None);
    fx.te
        .store
        .add_workspace(recur_domain::Workspace {
            id: "WS-scoped".into(),
            ..recur_domain::Workspace::default()
        })
        .unwrap();
    for (owner, code) in [
        ("", "CO"),
        ("PA", "CO"),
        ("MA", "CO"),
        ("PA2", "CO"),
        ("PA2", "RV"),
        ("PA3", "CO"),
        ("PA2", "PA"),
        ("PA2", "MA"),
        ("PA2", "empty"),
    ] {
        let cfg = AgentConfig {
            code: code.into(),
            owner_code: owner.into(),
            ..AgentConfig::default()
        };
        let sess = fx
            .te
            .store
            .create_session(Session {
                id: format!("{owner}-{code}"),
                workspace_id: "WS-scoped".into(),
                kind: "coder".into(),
                provider: PROVIDER_CLAUDE_CODE.into(),
                config_json: recur_domain::json::to_string(&cfg).unwrap(),
                ..Session::default()
            })
            .unwrap();
        if code == "empty" {
            continue;
        }
        fx.te
            .store
            .append_message(
                &sess.id,
                Message {
                    role: "assistant".into(),
                    content: Some(vec![ContentBlock::text(&sess.id)]),
                    ..Message::default()
                },
            )
            .unwrap();
    }
    for (owner, code, want) in [
        ("", "", vec!["-CO"]),
        ("PA", "", vec!["-CO", "PA-CO"]),
        ("MA", "", vec!["-CO", "MA-CO"]),
        ("PA2", "", vec!["PA2-CO", "PA2-RV"]),
        ("PA2", "CO", vec!["PA2-CO"]),
        ("PA3", "CO", vec!["PA3-CO"]),
        ("PA2", "QA", vec![]),
    ] {
        let views = fx.eng().workspace_sessions("WS-scoped", owner, code);
        let mut got: Vec<&str> = views.iter().map(|v| v.id.as_str()).collect();
        got.sort_unstable();
        assert_eq!(got, want, "{owner}/{code}");
        for v in &views {
            let items = v.items.as_deref().unwrap_or_default();
            assert_eq!(items.len(), 1, "{}", v.id);
            assert_eq!(items[0].text, v.id);
        }
    }
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_racing_follow_up_does_not_cancel_the_first_send() {
    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("cursor race").await;
    let (first_in, second_in, proceed) = (
        CancellationToken::new(),
        CancellationToken::new(),
        CancellationToken::new(),
    );
    let (a, b, p) = (first_in.clone(), second_in.clone(), proceed.clone());
    fx.eng()
        .set_before_chat_claim_hook(gate_hook(move |call| match call {
            1 => Some((a.clone(), Some(p.clone()))),
            2 => Some((b.clone(), None)),
            _ => None,
        }));
    let first = spawn_send(&fx, &ws.id, "first prompt", "cursor/composer-2.5");
    within_hang(&first_in, "first send enters its prologue").await;
    let second = spawn_send(&fx, &ws.id, "second prompt", "cursor/composer-2.5");
    within_hang(&second_in, "second send takes a ticket").await;
    proceed.cancel();
    within_hang(&r.started, "the turn starts").await;
    assert_eq!(
        join_hang(second, "the quick send").await,
        CHAT_INJECTED_SENTINEL
    );
    r.release.cancel();
    join_hang(first, "the first send").await;
    assert_eq!(r.runs(), 1);
    assert!(r.last_spec().unwrap().prompt.contains("first prompt"));
    assert_eq!(r.injected_users(), ["second prompt"]);
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follow_up_does_not_restart_after_stop() {
    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("cursor stop race").await;
    let (blocked, proceed) = (CancellationToken::new(), CancellationToken::new());
    let (eng, id) = (Arc::downgrade(fx.eng()), ws.id.clone());
    let (b, p) = (blocked.clone(), proceed.clone());
    fx.eng().set_before_chat_claim_hook(move || {
        let live = eng
            .upgrade()
            .is_some_and(|e| e.live_chat_turn(&id, "PA").is_some());
        let (b, p) = (b.clone(), p.clone());
        Box::pin(async move {
            if live {
                b.cancel();
                p.cancelled().await;
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });
    let first = spawn_send(&fx, &ws.id, "do the thing", "cursor/composer-2.5");
    within_hang(&r.started, "first turn starts").await;
    let follow = spawn_send(&fx, &ws.id, "late steer", "cursor/composer-2.5");
    within_hang(&blocked, "follow-up enters its prologue").await;
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    proceed.cancel();
    assert_eq!(join_hang(follow, "follow-up").await, CHAT_STOPPED_REPLY);
    assert_eq!(r.runs(), 1);
    r.release.cancel();
    join_hang(first, "first turn").await;
    fx.te.close().await;
}

async fn steer_once(fx: &Fx, r: &Arc<HoldInject>, text: &str, display: &str) {
    let ws = fx.workspace("steer once").await;
    let first = spawn_send(fx, &ws.id, "do the thing", "cursor/composer-2.5");
    within_hang(&r.started, "first turn starts").await;
    let got = fx
        .eng()
        .send_workspace_message(
            &ws.id,
            "PA",
            text,
            "cursor/composer-2.5",
            None,
            &[],
            "",
            display,
        )
        .await;
    assert_eq!(got, CHAT_INJECTED_SENTINEL);
    r.release.cancel();
    join_hang(first, "first turn").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_carries_the_display_text_and_the_surface() {
    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    steer_once(&fx, &r, "WIRE_PROMPT", "Fix findings").await;
    let injected = r.injected.lock().clone();
    assert_eq!(injected.len(), 1);
    let (p, d) = &injected[0];
    assert_eq!(d, "Fix findings");
    assert!(
        p.contains("WIRE_PROMPT") && p.contains("Recur workspace"),
        "{p}"
    );
    fx.te.close().await;

    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    let wire =
        format!(r#"{CHAT_SURFACE_OPEN}{{"surface":"github_pr","prNumber":9}}{CHAT_SURFACE_CLOSE}"#)
            + "\nreview this";
    steer_once(&fx, &r, &wire, "").await;
    assert_eq!(r.injected_users(), ["review this"]);
    let (p, _) = r.injected.lock()[0].clone();
    assert!(p.contains("GitHub pull request"), "{p}");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_never_carries_the_fork_preamble() {
    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("cursor fork inject").await;
    let first = spawn_send(&fx, &ws.id, "only turn", "cursor/composer-2.5");
    within_hang(&r.started, "first turn starts").await;
    let sess = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    let mut cfg = AgentConfig::parse(&sess.config_json);
    cfg.fork_inject = true;
    fx.te
        .store
        .update_session_config_json(&sess.id, &recur_domain::json::to_string(&cfg).unwrap())
        .unwrap();
    let got = fx
        .send(&ws.id, "PA", "steer later", "cursor/composer-2.5")
        .await;
    assert_eq!(got, CHAT_INJECTED_SENTINEL);
    assert_eq!(r.runs(), 1);
    assert_eq!(r.injected_users(), ["steer later"]);
    assert!(!r.injected.lock()[0].0.contains(FORK_PREAMBLE_LEAD));
    r.release.cancel();
    join_hang(first, "first turn").await;
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_steer_lands_after_stop() {
    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("cursor stop").await;
    let first = spawn_send(&fx, &ws.id, "do the thing", "cursor/composer-2.5");
    within_hang(&r.started, "first turn starts").await;
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    assert!(
        !fx.eng()
            .try_inject_chat(&ws.id, "PA", "late steer", "late steer")
            .await,
        "a discarded steer must not be acknowledged"
    );
    r.release.cancel();
    join_hang(first, "first turn").await;
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_steer_lands_after_the_result() {
    let r = Arc::new(HoldAfterResult::default());
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("cursor result").await;
    let first = spawn_send(&fx, &ws.id, "do the thing", "cursor/composer-2.5");
    within_hang(&r.started, "first turn emits its result").await;
    let follow = spawn_send(&fx, &ws.id, "late steer", "cursor/composer-2.5");
    r.release.cancel();
    join_hang(first, "first turn").await;
    within_hang(&r.second_started, "the replacement turn starts").await;
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().status, "working");
    assert_eq!(
        r.spec_resume(1),
        "agent-done",
        "resume refreshed after the predecessor unregistered"
    );
    r.second_release.cancel();
    assert_ne!(join_hang(follow, "follow-up").await, CHAT_INJECTED_SENTINEL);
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_cannot_steer_is_waited_out_not_swapped() {
    let r = Arc::new(HoldUntil::default());
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("no swap").await;
    let first = spawn_send(&fx, &ws.id, "do the thing", "claude-code/sonnet");
    within_hang(&r.started, "first turn starts").await;
    let follow = spawn_send(&fx, &ws.id, "steer please", "cursor/composer-2.5");
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !follow.is_finished(),
        "the follow-up swap-cancelled the live turn"
    );
    assert_eq!((*r.runs.lock(), *r.cancels.lock()), (1, 0));
    r.release.cancel();
    join_hang(first, "first turn").await;
    assert_ne!(join_hang(follow, "follow-up").await, CHAT_INJECTED_SENTINEL);
    assert_eq!((*r.runs.lock(), *r.cancels.lock()), (2, 0));
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_session_never_steers_the_live_turn() {
    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("fresh live").await;
    let first = spawn_send(&fx, &ws.id, "do the thing", "cursor/composer-2.5");
    within_hang(&r.started, "first turn starts").await;
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let follow = tokio::spawn(async move {
        eng.send_workspace_turn(
            &id,
            "PA",
            "WIRE review",
            "cursor/composer-2.5",
            None,
            &[],
            "review",
            "Re-review",
            true,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !follow.is_finished(),
        "the fresh_session send returned before the live turn ended"
    );
    assert_eq!(r.runs(), 1);
    r.release.cancel();
    join_hang(first, "first turn").await;
    assert_ne!(
        join_hang(follow, "fresh send").await,
        CHAT_INJECTED_SENTINEL
    );
    let specs = r.specs.lock().clone();
    assert_eq!(specs.len(), 2);
    assert!(
        specs[1].resume_session.is_empty(),
        "an isolated send does not resume"
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remount_registers_only_after_its_fifo_claim() {
    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("continue fifo").await;
    let (first_in, cont_in, proceed) = (
        CancellationToken::new(),
        CancellationToken::new(),
        CancellationToken::new(),
    );
    let (a, b, p) = (first_in.clone(), cont_in.clone(), proceed.clone());
    fx.eng()
        .set_before_chat_claim_hook(gate_hook(move |call| match call {
            1 => Some((a.clone(), Some(p.clone()))),
            2 => Some((b.clone(), None)),
            _ => None,
        }));
    let first = spawn_send(&fx, &ws.id, "user prompt", "cursor/composer-2.5");
    within_hang(&first_in, "first send enters its prologue").await;
    let epoch = fx.eng().alloc_continue_gen(&ws.id, "PA");
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let cont = tokio::spawn(async move {
        eng.send_workspace_turn_epoch(
            &id,
            "PA",
            &format_mode_switch_resume_prompt(CHAT_MODE_AGENT, CHAT_MODE_PLAN),
            "cursor/composer-2.5",
            None,
            &[],
            CHAT_MODE_PLAN,
            &format_mode_switch_display(CHAT_MODE_PLAN),
            false,
            epoch,
        )
        .await
    });
    within_hang(&cont_in, "the remount enters its prologue").await;
    for _ in 0..16 {
        assert!(
            fx.eng().live_chat_turn(&ws.id, "PA").is_none(),
            "the remount registered before its FIFO claim"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    proceed.cancel();
    within_hang(&r.started, "the first turn starts").await;
    r.release.cancel();
    join_hang(first, "first send").await;
    join_hang(cont, "the remount").await;
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subagent_notice_queues_as_its_own_turn() {
    let r = HoldInject::new();
    let fx = Fx::new(Some(r.provider()));
    let ws = fx.workspace("notice").await;
    let model = "claude-code/sonnet";
    let first = spawn_send(&fx, &ws.id, "do the thing", model);
    within_hang(&r.started, "first turn starts").await;
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let notice = tokio::spawn(async move {
        eng.send_workspace_message(
            &id,
            "PA",
            "Subagent x has finished.",
            model,
            None,
            &[],
            "",
            &format!("{DISPLAY_HIDDEN_SENTINEL}x finished (ws.x)"),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(r.runs(), 1, "the notice must wait, not start yet");
    r.release.cancel();
    for h in [first, notice] {
        assert_ne!(join_hang(h, "a turn").await, CHAT_INJECTED_SENTINEL);
    }
    assert_eq!(r.runs(), 2, "the notice runs as its own turn");
    assert!(r.injected.lock().is_empty(), "the notice must not steer");
    fx.te.close().await;
}
