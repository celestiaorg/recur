//! switch_mode, mode shape and chat surface tests: the privilege rule,
//! remounting a live turn with the destination tools, Stop and ask_user
//! dropping the remount, the remount cap, dead ends, unseen flags, and the mode
//! shapes and surface prompts.

use std::sync::Arc;
use std::time::Duration;

use recur_agent_tools::names::mcp;
use recur_domain::{
    AgentRole, CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN, CHAT_MODE_REVIEW,
    Settings, UserQuestion,
};
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use crate::ask_user::ASK_USER_PARK_DELAY;
use crate::attention::chat_inbox::chat_inbox_id;
use crate::chat::modes::chat_mode_shape;
use crate::chat::surface::{ChatSurface, peel_chat_surface};
use crate::chat::switch_mode::{
    MAX_MODE_REMOUNTS, format_mode_switch_display, format_mode_switch_resume_prompt,
    is_mode_switch_resume_dead_end, switch_mode_allowed,
};
use crate::chatlife::ChatTurn;
use crate::turn::transcript::is_mode_switch_continue_display;

#[test]
fn the_privilege_rule() {
    for (from, to, ok) in [
        (CHAT_MODE_AGENT, CHAT_MODE_ASK, true),
        (CHAT_MODE_AGENT, CHAT_MODE_PLAN, true),
        (CHAT_MODE_AGENT, CHAT_MODE_REVIEW, true),
        (CHAT_MODE_ASK, CHAT_MODE_PLAN, true),
        (CHAT_MODE_PLAN, CHAT_MODE_REVIEW, true),
        (CHAT_MODE_REVIEW, CHAT_MODE_ASK, true),
        (CHAT_MODE_ASK, CHAT_MODE_AGENT, false),
        (CHAT_MODE_PLAN, CHAT_MODE_AGENT, false),
        (CHAT_MODE_REVIEW, CHAT_MODE_AGENT, false),
        (CHAT_MODE_ASK, CHAT_MODE_LOOP, false),
    ] {
        assert_eq!(switch_mode_allowed(from, to), ok, "{from}→{to}");
    }
}

async fn sent(mode: &str) -> (Fx, String) {
    let fx = Fx::new(Some(capture("sm", "ok").provider()));
    let ws = fx.workspace("switch").await;
    fx.send_mode(&ws.id, "PA", "hi", "sonnet", mode).await;
    (fx, ws.id)
}

#[tokio::test]
async fn the_same_mode_does_not_remount() {
    let (fx, ws) = sent(CHAT_MODE_AGENT).await;
    let got = fx.eng().switch_chat_mode(&ws, "PA", CHAT_MODE_AGENT).await;
    assert!(
        got.contains("Already in agent") && !got.contains("remounting"),
        "{got}"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn an_idle_switch_only_persists() {
    let (fx, ws) = sent(CHAT_MODE_AGENT).await;
    let got = fx.eng().switch_chat_mode(&ws, "PA", CHAT_MODE_PLAN).await;
    assert!(
        got.contains("Switched chat mode from agent to plan"),
        "{got}"
    );
    assert!(!got.contains("remounting"));
    assert_eq!(
        fx.eng().workspace_chat_selection(&ws, "PA").2,
        CHAT_MODE_PLAN
    );
    assert!(fx.te.events.has("chat.mode_changed"));
    fx.te.close().await;
}

#[tokio::test]
async fn an_upgrade_to_agent_is_refused() {
    let (fx, ws) = sent(CHAT_MODE_PLAN).await;
    let got = fx.eng().switch_chat_mode(&ws, "PA", CHAT_MODE_AGENT).await;
    assert!(got.contains("Cannot switch from plan to agent"), "{got}");
    assert_eq!(
        fx.eng().workspace_chat_selection(&ws, "PA").2,
        CHAT_MODE_PLAN
    );
    fx.te.close().await;
}

#[test]
fn the_resume_prompt_and_display() {
    let got = format_mode_switch_resume_prompt(CHAT_MODE_AGENT, CHAT_MODE_PLAN);
    assert!(got.contains("from agent to plan"));
    assert!(got.contains("mcp__recur__submit_plan"));
    assert!(got.contains("Do not ask them to switch"));
    assert_eq!(
        format_mode_switch_display(CHAT_MODE_PLAN),
        "Switched to Plan: continuing"
    );
    let ask = format_mode_switch_resume_prompt(CHAT_MODE_AGENT, CHAT_MODE_ASK);
    assert!(ask.contains("mcp__recur__create_artifact"));
    assert!(ask.contains("do not edit repository files"));
    for to in [CHAT_MODE_ASK, CHAT_MODE_PLAN, CHAT_MODE_REVIEW] {
        let got = format_mode_switch_resume_prompt(CHAT_MODE_AGENT, to);
        assert!(
            got.contains("git worktree") && got.contains("mcp__recur__create_artifact"),
            "{to}"
        );
    }
    assert!(
        format_mode_switch_resume_prompt(CHAT_MODE_ASK, CHAT_MODE_PLAN)
            .contains("never instead of, submit_plan")
    );
    let review = format_mode_switch_resume_prompt(CHAT_MODE_ASK, CHAT_MODE_REVIEW);
    assert!(
        review.contains("never instead of, report_finding") && review.contains("update_finding")
    );
}

fn spawn_send(fx: &Fx, ws: &str, text: &str, mode: &str) -> tokio::task::JoinHandle<String> {
    let (eng, ws, text, mode) = (
        Arc::clone(fx.eng()),
        ws.to_owned(),
        text.to_owned(),
        mode.to_owned(),
    );
    tokio::spawn(async move {
        eng.send_workspace_message(&ws, "PA", &text, "sonnet", None, &[], &mode, "")
            .await
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_switch_remounts_with_the_new_tools() {
    let c = capture("mode-cont", "continued");
    let started = CancellationToken::new();
    let fx = Fx::new(Some(hold_first_runner(&c, started.clone(), Duration::ZERO)));
    let ws = fx.workspace("remount").await;
    let first = spawn_send(&fx, &ws.id, "build this feature", CHAT_MODE_AGENT);
    within_hang(&started, "first turn starts").await;
    let msg = fx
        .eng()
        .switch_chat_mode(&ws.id, "PA", CHAT_MODE_PLAN)
        .await;
    assert!(
        msg.contains("Switched chat mode from agent to plan"),
        "{msg}"
    );
    assert!(msg.contains("remounting"), "{msg}");
    join_hang(first, "first turn after the park").await;
    let specs = wait_coder_turns(&c, 2).await;
    let (first, cont) = (&specs[0], &specs[1]);
    assert!(has_tool(first, "Edit") && !has_tool(first, mcp::SUBMIT_PLAN));
    assert!(has_tool(cont, mcp::SUBMIT_PLAN));
    assert!(!has_tool(cont, "Edit") && !has_tool(cont, "Write"));
    assert!(cont.prompt.contains("from agent to plan"));
    assert!(cont.append_system_prompt.contains("PLAN mode"));
    assert_eq!(cont.display_text, "Switched to Plan: continuing");
    assert!(cont.refresh_standing);
    assert!(cont.skip_record_user);
    assert_eq!(
        fx.eng().workspace_chat_selection(&ws.id, "PA").2,
        CHAT_MODE_PLAN
    );
    fx.te.close().await;
}

async fn stop_after_switch(delay: Duration) {
    let c = capture("slow-cont", "continued");
    let started = CancellationToken::new();
    let fx = Fx::new(Some(hold_first_runner(&c, started.clone(), delay)));
    let ws = fx.workspace("stop-no-remount").await;
    let first = spawn_send(&fx, &ws.id, "build this", CHAT_MODE_AGENT);
    within_hang(&started, "first turn starts").await;
    fx.eng()
        .switch_chat_mode(&ws.id, "PA", CHAT_MODE_PLAN)
        .await;
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    let reply = join_hang(first, "first turn after stop").await;
    assert!(!reply.is_empty() && !reply.contains("continued"), "{reply}");
    tokio::time::sleep(ASK_USER_PARK_DELAY + Duration::from_millis(200)).await;
    assert_eq!(
        c.with_role(AgentRole::Coder).len(),
        1,
        "Stop must not remount"
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_drops_the_remount() {
    stop_after_switch(Duration::ZERO).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_during_a_slow_teardown_drops_the_remount() {
    stop_after_switch(ASK_USER_PARK_DELAY + Duration::from_millis(80)).await;
}

#[tokio::test]
async fn a_park_skips_a_stopped_turn() {
    let fx = Fx::new(None);
    crate::turn::test_support::add_solo_workspace(fx.eng(), "W1");
    let cancel = CancellationToken::new();
    let turn = fx
        .eng()
        .register_chat_turn("W1", "PA", cancel.clone())
        .unwrap();
    assert!(fx.eng().cancel_chat("W1", "PA").await);
    assert!(cancel.is_cancelled());
    assert!(!fx.eng().park_chat_turn_if("W1", "PA", Some(&turn)));
    assert!(!turn.is_parked());
    fx.te.close().await;
}

#[tokio::test]
async fn the_remount_cap_resets_on_a_user_send() {
    let (fx, ws) = sent(CHAT_MODE_AGENT).await;
    let eng = fx.eng();
    let turn = eng
        .register_chat_turn(&ws, "PA", CancellationToken::new())
        .unwrap();
    for (i, to) in [
        CHAT_MODE_PLAN,
        CHAT_MODE_ASK,
        CHAT_MODE_REVIEW,
        CHAT_MODE_PLAN,
    ]
    .into_iter()
    .enumerate()
    {
        let msg = eng.switch_chat_mode(&ws, "PA", to).await;
        if (i as i64) < MAX_MODE_REMOUNTS {
            assert!(msg.contains("remounting"), "switch {i}: {msg}");
        } else {
            assert!(
                !msg.contains("remounting") && msg.contains("Subsequent turns"),
                "{msg}"
            );
        }
    }
    eng.unregister_chat_turn(&ws, "PA", &turn);
    fx.send(&ws, "PA", "go on", "sonnet").await;
    eng.register_chat_turn(&ws, "PA", CancellationToken::new())
        .unwrap();
    let msg = eng.switch_chat_mode(&ws, "PA", CHAT_MODE_ASK).await;
    assert!(
        msg.contains("remounting"),
        "a user send resets the cap: {msg}"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn a_pending_ask_user_owns_the_tab() {
    let c = capture("ask-skip", "ok");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("ask-owns-park").await;
    fx.send_mode(&ws.id, "PA", "hi", "sonnet", CHAT_MODE_AGENT)
        .await;
    let before = c.with_role(AgentRole::Coder).len();
    let eng = fx.eng();
    let turn = eng
        .register_chat_turn(&ws.id, "PA", CancellationToken::new())
        .unwrap();
    eng.ask_user_broker().park(
        UserQuestion {
            workspace_id: ws.id.clone(),
            agent_code: "PA".into(),
            question: "which?".into(),
            ..UserQuestion::default()
        },
        Some(Arc::clone(&turn)),
    );
    turn.mark_parked();
    eng.note_mode_continue(&ws.id, "PA", CHAT_MODE_AGENT, CHAT_MODE_PLAN, &turn);
    eng.maybe_continue_after_mode_switch(&ws.id, "PA", &turn);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(c.with_role(AgentRole::Coder).len(), before);
    fx.te.close().await;
}

#[tokio::test]
async fn a_pending_remount_skips_the_finished_reply_inbox_row() {
    let fx = Fx::new(None);
    let ws = fx.workspace("inbox-remount").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    let turn = eng
        .register_chat_turn(&ws.id, "PA", CancellationToken::new())
        .unwrap();
    eng.note_mode_continue(&ws.id, "PA", CHAT_MODE_AGENT, CHAT_MODE_PLAN, &turn);
    fx.te.store.update_agent(&ws.id, "PA", |a| a.unseen = true);
    assert!(!eng.emit_chat_inbox(&ws.id, "PA", false));
    assert!(!inbox_has(&fx.te, &chat_inbox_id(&ws.id, "PA")));
    fx.te.close().await;
}

#[tokio::test]
async fn a_remount_dead_end_records_its_note_without_a_bubble() {
    let (fx, ws) = sent(CHAT_MODE_AGENT).await;
    let note = "I can't run right now: mcp bind failed";
    fx.eng().persist_chat_failure(
        &ws,
        "PA",
        &format_mode_switch_resume_prompt(CHAT_MODE_AGENT, CHAT_MODE_PLAN),
        &format_mode_switch_display(CHAT_MODE_PLAN),
        note,
        0,
        &recur_agent::TurnResult::default(),
        false,
        "",
    );
    let items = fx.items(&ws, "PA").await;
    assert!(items.iter().any(|it| it.kind == "text" && it.text == note));
    assert!(
        !items
            .iter()
            .any(|it| it.role == "user" && is_mode_switch_continue_display(&it.text))
    );
    fx.te.close().await;
}

#[test]
fn dead_end_replies() {
    assert!(is_mode_switch_resume_dead_end(
        "I'm at capacity right now — every agent slot is busy. Try again in a moment."
    ));
    assert!(!is_mode_switch_resume_dead_end("continued"));
    assert!(!is_mode_switch_resume_dead_end(""));
}

#[tokio::test]
async fn a_model_that_stops_before_the_park_still_remounts() {
    let c = capture("pre-park", "ok");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("pre-park").await;
    fx.send_mode(&ws.id, "PA", "hi", "sonnet", CHAT_MODE_AGENT)
        .await;
    let before = c.with_role(AgentRole::Coder).len();
    let turn = ChatTurn::detached();
    fx.eng()
        .note_mode_continue(&ws.id, "PA", CHAT_MODE_AGENT, CHAT_MODE_PLAN, &turn);
    fx.eng()
        .maybe_continue_after_mode_switch(&ws.id, "PA", &turn);
    assert!(
        eventually(Duration::from_secs(2), || c
            .with_role(AgentRole::Coder)
            .len()
            > before)
        .await,
        "the remount must still run"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn a_dead_end_flags_the_tab_unseen_when_unfocused() {
    let (fx, ws) = sent(CHAT_MODE_AGENT).await;
    let eng = fx.eng();
    eng.set_workspace_chat_focus(&ws, "PA");
    eng.set_app_focused(false);
    eng.persist_chat_failure(
        &ws,
        "PA",
        "wire",
        &format_mode_switch_display(CHAT_MODE_PLAN),
        "I can't run right now: mcp bind failed",
        0,
        &recur_agent::TurnResult::default(),
        false,
        "",
    );
    eng.flag_chat_unseen(&ws, "PA", false);
    assert!(fx.te.store.agent(&ws, "PA").unwrap().unseen);
    assert!(inbox_has(&fx.te, &chat_inbox_id(&ws, "PA")));
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_last_unseen_event_matches_the_store() {
    let fx = Fx::new(None);
    let ws = fx.workspace("unseen-race").await;
    fx.eng().list_workspace_chats(&ws.id);
    for i in 0..50 {
        fx.eng().set_workspace_chat_focus("", "");
        let (e1, e2) = (Arc::clone(fx.eng()), Arc::clone(fx.eng()));
        let (w1, w2) = (ws.id.clone(), ws.id.clone());
        let a = tokio::spawn(async move { e1.flag_chat_unseen(&w1, "PA", false) });
        let b = tokio::spawn(async move { e2.set_workspace_chat_focus(&w2, "PA") });
        a.await.unwrap();
        b.await.unwrap();
        assert!(!fx.te.store.agent(&ws.id, "PA").unwrap().unseen, "iter {i}");
        assert_ne!(
            last_tab_unseen(&fx.te, 0, &ws.id, "PA"),
            Some(true),
            "iter {i}"
        );
    }
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_focus_recheck_publishes_the_clear() {
    let fx = Fx::new(None);
    let ws = fx.workspace("unseen-focus-clear").await;
    fx.eng().list_workspace_chats(&ws.id);
    let row = chat_inbox_id(&ws.id, "PA");
    for i in 0..50 {
        fx.eng().set_workspace_chat_focus("", "");
        fx.eng().flag_chat_unseen(&ws.id, "PA", false);
        assert!(fx.te.store.agent(&ws.id, "PA").unwrap().unseen);
        assert!(inbox_has(&fx.te, &row));
        let att_before = fx.te.events.count("attention.updated");
        let in_before = fx.te.events.count("inbox.updated");
        let (e1, e2) = (Arc::clone(fx.eng()), Arc::clone(fx.eng()));
        let (w1, w2) = (ws.id.clone(), ws.id.clone());
        let a = tokio::spawn(async move { e1.flag_chat_unseen(&w1, "PA", true) });
        let b = tokio::spawn(async move { e2.set_workspace_chat_focus(&w2, "PA") });
        a.await.unwrap();
        b.await.unwrap();
        assert!(!fx.te.store.agent(&ws.id, "PA").unwrap().unseen, "iter {i}");
        assert!(!inbox_has(&fx.te, &row), "iter {i}");
        assert_eq!(
            last_tab_unseen(&fx.te, att_before, &ws.id, "PA"),
            Some(false),
            "iter {i}"
        );
        assert!(fx.te.events.count("inbox.updated") > in_before, "iter {i}");
    }
    fx.te.close().await;
}

#[tokio::test]
async fn a_remount_never_displaces_a_live_turn() {
    let c = capture("disp", "ok");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("no-displace").await;
    let eng = fx.eng();
    fx.send_mode(&ws.id, "PA", "hi", "sonnet", CHAT_MODE_AGENT)
        .await;
    let before = c.with_role(AgentRole::Coder).len();
    let cancel = CancellationToken::new();
    eng.register_chat_turn(&ws.id, "PA", cancel.clone())
        .unwrap();
    let reply = eng
        .send_workspace_message(
            &ws.id,
            "PA",
            &format_mode_switch_resume_prompt(CHAT_MODE_AGENT, CHAT_MODE_PLAN),
            "sonnet",
            None,
            &[],
            CHAT_MODE_PLAN,
            &format_mode_switch_display(CHAT_MODE_PLAN),
        )
        .await;
    assert!(
        !cancel.is_cancelled(),
        "a remount must not cancel the live turn"
    );
    assert_eq!(reply, "");
    assert_eq!(c.with_role(AgentRole::Coder).len(), before);

    let cancel2 = CancellationToken::new();
    eng.register_chat_turn(&ws.id, "PA", cancel2.clone())
        .unwrap();
    eng.resume_after_mode_switch(&ws.id, "PA", CHAT_MODE_AGENT, CHAT_MODE_PLAN, 0)
        .await;
    assert!(!cancel2.is_cancelled());
    assert_eq!(c.with_role(AgentRole::Coder).len(), before);
    fx.te.close().await;
}

#[tokio::test]
async fn the_remount_follows_the_dock_mode() {
    let c = capture("dock", "ok");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("dock-flip").await;
    let eng = fx.eng();
    fx.send_mode(&ws.id, "PA", "hi", "sonnet", CHAT_MODE_AGENT)
        .await;
    eng.remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, CHAT_MODE_ASK, None)
        .await;
    let generation = eng.alloc_continue_gen(&ws.id, "PA");
    eng.resume_after_mode_switch(&ws.id, "PA", CHAT_MODE_AGENT, CHAT_MODE_PLAN, generation)
        .await;
    let got = c.with_role(AgentRole::Coder).pop().unwrap();
    assert!(got.prompt.contains("to ask"), "{}", got.prompt);
    assert!(got.append_system_prompt.contains("ASK mode"));
    assert!(!has_tool(&got, mcp::SUBMIT_PLAN));
    fx.te.close().await;
}

#[tokio::test]
async fn dropped_remounts_do_not_burn_the_budget() {
    let (fx, ws) = sent(CHAT_MODE_AGENT).await;
    let eng = fx.eng();
    let mut turn = eng
        .register_chat_turn(&ws, "PA", CancellationToken::new())
        .unwrap();
    for to in [CHAT_MODE_PLAN, CHAT_MODE_ASK, CHAT_MODE_REVIEW] {
        let msg = eng.switch_chat_mode(&ws, "PA", to).await;
        assert!(msg.contains("remounting"), "{msg}");
        assert!(eng.cancel_chat(&ws, "PA").await);
        eng.maybe_continue_after_mode_switch(&ws, "PA", &turn);
        turn = eng
            .register_chat_turn(&ws, "PA", CancellationToken::new())
            .unwrap();
    }
    let msg = eng.switch_chat_mode(&ws, "PA", CHAT_MODE_PLAN).await;
    assert!(msg.contains("remounting"), "{msg}");
    fx.te.close().await;
}

#[tokio::test]
async fn clearing_a_workspace_drops_pending_remounts() {
    let (fx, ws) = sent(CHAT_MODE_AGENT).await;
    let eng = fx.eng();
    let turn = eng
        .register_chat_turn(&ws, "PA", CancellationToken::new())
        .unwrap();
    eng.note_mode_continue(&ws, "PA", CHAT_MODE_AGENT, CHAT_MODE_PLAN, &turn);
    assert_eq!(eng.bump_mode_remounts(&ws, "PA"), 1);
    eng.clear_mode_continues_for_workspace(&ws);
    assert!(!eng.pending_mode_continue_for(&ws, "PA"));
    assert_eq!(eng.bump_mode_remounts(&ws, "PA"), 1);
    fx.te.close().await;
}

#[tokio::test]
async fn an_older_turn_cannot_steal_a_newer_remount() {
    let fx = Fx::new(None);
    let eng = fx.eng();
    let (old, newer) = (ChatTurn::detached(), ChatTurn::detached());
    eng.note_mode_continue("ws-steal", "PA", CHAT_MODE_AGENT, CHAT_MODE_PLAN, &old);
    eng.note_mode_continue("ws-steal", "PA", CHAT_MODE_AGENT, CHAT_MODE_ASK, &newer);
    let generation = eng.pending_mode_continue_epoch("ws-steal", "PA").unwrap();
    eng.maybe_continue_after_mode_switch("ws-steal", "PA", &old);
    assert!(eng.pending_mode_continue_for("ws-steal", "PA"));
    assert!(eng.continue_gen_live(generation));
    let p = eng
        .take_mode_continue_for("ws-steal", "PA", Some(&newer))
        .expect("the matching turn owns it");
    assert!(Arc::ptr_eq(&p.turn, &newer));
    assert!(eng.continue_gen_live(generation));
    eng.drop_continue_gen(generation);
    fx.te.close().await;
}

#[test]
fn every_shape_allows_artifacts_and_read_only_modes_say_so() {
    let set = Settings::default();
    let artifacts = [
        mcp::CREATE_ARTIFACT,
        mcp::LIST_ARTIFACTS,
        mcp::READ_ARTIFACT,
        mcp::PRESENT_ARTIFACT,
    ];
    for mode in [
        CHAT_MODE_ASK,
        CHAT_MODE_PLAN,
        CHAT_MODE_REVIEW,
        CHAT_MODE_AGENT,
    ] {
        let shape = chat_mode_shape(mode, &set, false);
        for t in artifacts {
            assert!(shape.tools.iter().any(|x| x == t), "{mode} missing {t}");
        }
        if mode == CHAT_MODE_AGENT {
            continue;
        }
        for t in ["Edit", "Write", "NotebookEdit"] {
            assert!(!shape.tools.iter().any(|x| x == t), "{mode} allows {t}");
        }
        let p = &shape.append_prompt;
        assert!(
            p.contains("create_artifact") && p.contains("git worktree"),
            "{mode}"
        );
        let prefer = p.contains("Prefer an HTML artifact");
        let beside = p.contains("in addition to, never instead of");
        if mode == CHAT_MODE_ASK {
            assert!(prefer && !beside);
        } else {
            assert!(!prefer && beside, "{mode}");
        }
    }
}

#[test]
fn the_agent_shape_prefers_child_workspaces_and_children_execute_locally() {
    let set = Settings::default();
    let has = |tools: &[String], t: &str| tools.iter().any(|x| x == t);
    let shape = chat_mode_shape(CHAT_MODE_AGENT, &set, false);
    assert!(shape.append_prompt.contains("beyond a very simple task"));
    assert!(shape.append_prompt.contains(mcp::CREATE_WORKSPACE));
    assert!(shape.append_prompt.contains(mcp::MESSAGE_CHILD));
    let child = chat_mode_shape(CHAT_MODE_AGENT, &set, true);
    assert!(!child.append_prompt.contains("beyond a very simple task"));
    assert!(
        child
            .append_prompt
            .contains("execute the assigned task here")
    );
    assert!(
        child
            .append_prompt
            .contains(&format!("Do not call {}", mcp::ASK_USER))
    );
    assert!(!child.append_prompt.contains(mcp::CREATE_WORKSPACE));
    assert!(has(&child.tools, mcp::RENAME_BRANCH) && has(&child.tools, mcp::SWITCH_MODE));
    assert!(has(&shape.tools, mcp::ASK_USER));
    for t in [
        mcp::ASK_USER,
        mcp::OPEN_PR,
        mcp::CREATE_WORKSPACE,
        mcp::MERGE_CHILD,
        mcp::READ_CHILD,
        mcp::ARCHIVE_CHILD,
        mcp::MESSAGE_CHILD,
    ] {
        assert!(!has(&child.tools, t), "a child must not allow {t}");
    }
    for t in [
        mcp::OPEN_PR,
        mcp::CREATE_WORKSPACE,
        mcp::MERGE_CHILD,
        mcp::READ_CHILD,
        mcp::ARCHIVE_CHILD,
        mcp::MESSAGE_CHILD,
    ] {
        assert!(has(&shape.tools, t), "the parent needs {t}");
    }
    for mode in [CHAT_MODE_ASK, CHAT_MODE_PLAN, CHAT_MODE_REVIEW] {
        let other = chat_mode_shape(mode, &set, false);
        assert!(!other.append_prompt.contains("beyond a very simple task"));
        assert!(!has(&other.tools, mcp::MESSAGE_CHILD));
        assert!(has(&other.tools, mcp::ASK_USER));
        let nested = chat_mode_shape(mode, &set, true);
        assert!(
            !nested
                .append_prompt
                .contains("execute the assigned task here")
        );
        assert!(!has(&nested.tools, mcp::ASK_USER));
    }
}

#[test]
fn peel_chat_surface_cases() {
    let wire = "⟦recur-surface⟧{\"surface\":\"github_pr\",\"prId\":\"gh-1\",\"prNumber\":9}⟦/recur-surface⟧\nplease review";
    let (rest, surf) = peel_chat_surface(wire).expect("a marker");
    assert_eq!(rest, "please review");
    assert_eq!(
        (surf.surface.as_str(), surf.pr_number, surf.pr_id.as_str()),
        ("github_pr", 9, "gh-1")
    );
    assert!(peel_chat_surface("hello").is_none());
}

fn forge_fx() -> (crate::testing::TestEngine, Arc<crate::Engine>) {
    let te = crate::testing::TestEngine::with(crate::testing::TestOptions {
        forges: vec![Arc::new(recur_forge::testing::FakeForge::default())],
        ..crate::testing::TestOptions::default()
    });
    let eng = Arc::clone(&te.engine);
    (te, eng)
}

#[tokio::test]
async fn the_subject_prompt_for_a_forge_pr_host() {
    let (te, eng) = forge_fx();
    let got = eng.subject_append_prompt(&ChatSurface {
        surface: "github_pr".into(),
        pr_number: 9,
        pr_url: "https://github.com/acme/r/pull/9".into(),
        pr_title: "Imported".into(),
        pr_branch: "feat".into(),
        pr_base: "main".into(),
        host: true,
        ..ChatSurface::default()
    });
    for want in [
        "GitHub pull request #9 (https://github.com/acme/r/pull/9)",
        r#"titled "Imported""#,
        "gh pr diff",
        "working copy",
    ] {
        assert!(got.contains(want), "missing {want}: {got}");
    }
    assert!(!got.contains("workspace for that PR"));
    te.close().await;
}

#[tokio::test]
async fn the_workspace_subject_mentions_a_linked_pr() {
    let (te, eng) = forge_fx();
    let got = eng.subject_append_prompt(&ChatSurface {
        surface: "workspace".into(),
        pr_number: 9,
        pr_source: "gh".into(),
        pr_url: "https://github.com/acme/r/pull/9".into(),
        ..ChatSurface::default()
    });
    assert!(
        got.contains("this Recur workspace") && got.contains("different surface"),
        "{got}"
    );
    te.close().await;
}

#[tokio::test]
async fn the_local_pr_subject() {
    let (te, eng) = forge_fx();
    let got = eng.subject_append_prompt(&ChatSurface {
        surface: "local_pr".into(),
        pr_number: 7,
        pr_title: "WIP".into(),
        ..ChatSurface::default()
    });
    assert!(
        got.contains("Local PR #7") && got.contains("not a GitHub"),
        "{got}"
    );
    te.close().await;
}

#[test]
fn every_chat_mode_can_ask_permission() {
    let off = recur_domain::Settings {
        bypass_permissions: false,
        ..Default::default()
    };
    for mode in ["ask", "plan", "review", "agent", ""] {
        let shape = chat_mode_shape(mode, &off, false);
        assert_eq!(
            shape.permission,
            recur_domain::PERMISSION_MODE_ASK,
            "{mode}"
        );
        assert!(shape.mcp, "{mode} needs the MCP endpoint for its prompts");
    }
    let on = recur_domain::Settings {
        bypass_permissions: true,
        ..Default::default()
    };
    for mode in ["ask", "plan", "review", "agent"] {
        assert_eq!(
            chat_mode_shape(mode, &on, false).permission,
            recur_domain::PERMISSION_MODE_BYPASS,
            "{mode}"
        );
    }
}
