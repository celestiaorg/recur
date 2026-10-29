//! Chat tab tests: several pair tabs with separate transcripts, close / reopen
//! / delete closed tabs (and the remounts and owned specialists that go with
//! them), the Maestro as a peer tab, saved tab order, and the life-lock edges
//! of tab operations.

use std::time::Duration;

use recur_domain::{
    AgentRole, CHAT_MODE_AGENT, CHAT_MODE_PLAN, ContentBlock, Message, PROVIDER_CLAUDE_CODE,
    Session, WorkspaceChatTab,
};
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use crate::chat::switch_mode::{format_mode_switch_display, format_mode_switch_resume_prompt};
use crate::chat::tabs::apply_chat_tab_order;
use crate::chatlife::ChatTurn;
use crate::loop_registry::LOOP_MAP_SEP;

#[tokio::test]
async fn pair_tabs_keep_separate_transcripts_and_sessions() {
    let c = capture("sess", "Done.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("multi-chat").await;
    let eng = fx.eng();
    assert_eq!(tab_codes(&eng.list_workspace_chats(&ws.id)), ["PA"]);
    fx.send(&ws.id, "PA", "task A", "sonnet").await;
    let tab_b = eng.create_workspace_chat(&ws.id).await;
    assert_eq!(tab_b.code, "PA2");
    fx.send(&ws.id, "PA2", "task B", "opus").await;
    let a = fx.items(&ws.id, "PA").await;
    let b = fx.items(&ws.id, "PA2").await;
    assert!(a.len() >= 2 && a[0].text == "task A", "{a:?}");
    assert!(b.len() >= 2 && b[0].text == "task B", "{b:?}");
    let pa = with_agent(&c, &format!("{}-PA", ws.id));
    let pa2 = with_agent(&c, &format!("{}-PA2", ws.id));
    assert!(pa[0].resume_session.is_empty() && pa2[0].resume_session.is_empty());
    fx.send(&ws.id, "PA", "follow up A", "sonnet").await;
    let pa = with_agent(&c, &format!("{}-PA", ws.id));
    assert!(!pa[1].resume_session.is_empty(), "a follow-up resumes");
    assert_eq!(pa[1].agent_id, format!("{}-PA", ws.id));

    let tabs = eng.list_workspace_chats(&ws.id);
    assert_eq!(tabs.len(), 2);
    assert!(tabs.iter().all(|t| t.title != "New chat"), "{tabs:?}");

    let tabs = eng.close_workspace_chat(&ws.id, "PA2").await;
    assert_eq!(tab_codes(&tabs), ["PA"]);
    assert!(
        fx.items(&ws.id, "PA2").await.len() >= 2,
        "closed history stays"
    );
    assert_eq!(tab_codes(&eng.list_closed_workspace_chats(&ws.id)), ["PA2"]);
    let tabs = eng.reopen_workspace_chat(&ws.id, "PA2").await;
    assert_eq!(tabs.len(), 2);
    assert!(tab_codes(&tabs).contains(&"PA2".to_owned()));
    assert!(fx.items(&ws.id, "PA2").await.len() >= 2);
    assert!(eng.list_closed_workspace_chats(&ws.id).is_empty());
    assert_eq!(
        tab_codes(&eng.close_workspace_chat(&ws.id, "PA2").await),
        ["PA"]
    );

    // Closing the last tab resets to a fresh chat on a new code.
    let tabs = eng.close_workspace_chat(&ws.id, "PA").await;
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].title, "New chat");
    assert!(tabs[0].code != "PA" && tabs[0].code != "PA2", "{tabs:?}");
    assert!(fx.items(&ws.id, &tabs[0].code).await.is_empty());
    fx.te.close().await;
}

async fn closed_second_tab(fx: &Fx, ws: &str) -> WorkspaceChatTab {
    fx.send(ws, "PA", "keep me", "sonnet").await;
    let tab_b = fx.eng().create_workspace_chat(ws).await;
    fx.send(ws, &tab_b.code, "throw away", "sonnet").await;
    assert_eq!(
        tab_codes(&fx.eng().close_workspace_chat(ws, &tab_b.code).await),
        ["PA"]
    );
    tab_b
}

#[tokio::test]
async fn delete_closed_chat_refuses_open_running_and_live_tabs() {
    let fx = Fx::new(Some(capture("del", "Done.").provider()));
    let ws = fx.workspace("delete-closed").await;
    let eng = fx.eng();
    let tab_b = closed_second_tab(&fx, &ws.id).await;
    assert_eq!(
        tab_codes(&eng.list_closed_workspace_chats(&ws.id)),
        std::slice::from_ref(&tab_b.code)
    );
    fx.te.store.update_workspace(&ws.id, |w| {
        w.chat_tab_order = vec!["PA".into(), tab_b.code.clone()];
    });
    let (left, err) = eng.delete_closed_workspace_chat(&ws.id, "PA").await;
    assert!(err.is_err(), "an open tab cannot be deleted");
    assert_eq!(tab_codes(&left), std::slice::from_ref(&tab_b.code));

    let key = format!("{}{LOOP_MAP_SEP}{}", ws.id, tab_b.code);
    let h = walk(&fx.te, &ws.id, &tab_b.code, &key);
    assert!(
        eng.delete_closed_workspace_chat(&ws.id, &tab_b.code)
            .await
            .1
            .is_err()
    );
    eng.remove_loop(&h);

    let live = eng
        .register_chat_turn(&ws.id, &tab_b.code, CancellationToken::new())
        .unwrap();
    assert!(
        eng.delete_closed_workspace_chat(&ws.id, &tab_b.code)
            .await
            .1
            .is_err()
    );
    eng.unregister_chat_turn(&ws.id, &tab_b.code, &live);

    let (left, err) = eng.delete_closed_workspace_chat(&ws.id, &tab_b.code).await;
    assert!(err.is_ok() && left.is_empty(), "{left:?} {err:?}");
    assert!(!fx.ws(&ws.id).chat_tab_order.contains(&tab_b.code));
    assert!(fx.items(&ws.id, &tab_b.code).await.is_empty());
    assert_eq!(
        tab_codes(&eng.reopen_workspace_chat(&ws.id, &tab_b.code).await),
        ["PA"]
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_closed_chat_serializes_with_reopen() {
    let fx = Fx::new(Some(capture("del-race", "Done.").provider()));
    let ws = fx.workspace("delete-race").await;
    let tab_b = closed_second_tab(&fx, &ws.id).await;
    let (e1, e2) = (fx.eng().clone(), fx.eng().clone());
    let (w1, w2, c1, c2) = (
        ws.id.clone(),
        ws.id.clone(),
        tab_b.code.clone(),
        tab_b.code.clone(),
    );
    let del = tokio::spawn(async move { e1.delete_closed_workspace_chat(&w1, &c1).await.1 });
    let reopen = tokio::spawn(async move { e2.reopen_workspace_chat(&w2, &c2).await });
    let del_err = del.await.unwrap();
    reopen.await.unwrap();
    let agent_open = fx.te.store.agent(&ws.id, &tab_b.code).is_some();
    let sess_ok = fx
        .te
        .store
        .session_for_loop_agent(&ws.id, &tab_b.code)
        .is_some();
    assert!(
        !agent_open || sess_ok,
        "reopen left an open tab with no transcript"
    );
    assert!(
        del_err.is_err() || !agent_open,
        "delete succeeded while open"
    );
    assert!(
        del_err.is_ok() || sess_ok,
        "delete failed and the session is gone"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn delete_closed_chat_invalidates_a_scheduled_remount() {
    let c = capture("del-cont", "Done.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("delete-continue").await;
    let tab_b = closed_second_tab(&fx, &ws.id).await;
    let before = coder_turns(&c).len();
    let turn = ChatTurn::detached();
    fx.eng()
        .note_mode_continue(&ws.id, &tab_b.code, CHAT_MODE_AGENT, CHAT_MODE_PLAN, &turn);
    fx.eng()
        .maybe_continue_after_mode_switch(&ws.id, &tab_b.code, &turn);
    let (_, err) = fx
        .eng()
        .delete_closed_workspace_chat(&ws.id, &tab_b.code)
        .await;
    assert!(err.is_ok(), "{err:?}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(fx.te.store.agent(&ws.id, &tab_b.code).is_none());
    assert!(
        fx.te
            .store
            .session_for_loop_agent(&ws.id, &tab_b.code)
            .is_none()
    );
    assert_eq!(coder_turns(&c).len(), before);
    fx.te.close().await;
}

#[tokio::test]
async fn a_remount_generation_is_captured_when_noted() {
    let c = capture("epoch-note", "Done.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("epoch-note").await;
    let tab_b = closed_second_tab(&fx, &ws.id).await;
    let before = coder_turns(&c).len();
    let turn = ChatTurn::detached();
    fx.eng()
        .note_mode_continue(&ws.id, &tab_b.code, CHAT_MODE_AGENT, CHAT_MODE_PLAN, &turn);
    fx.eng().invalidate_continue_gens(&ws.id, &tab_b.code);
    fx.eng()
        .maybe_continue_after_mode_switch(&ws.id, &tab_b.code, &turn);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(fx.te.store.agent(&ws.id, &tab_b.code).is_none());
    assert_eq!(coder_turns(&c).len(), before);
    fx.te.close().await;
}

#[tokio::test]
async fn remove_workspace_invalidates_a_scheduled_remount() {
    let c = capture("rm-cont", "Done.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("remove-continue").await;
    let tab_b = closed_second_tab(&fx, &ws.id).await;
    let before_items = fx.items(&ws.id, &tab_b.code).await.len();
    let before = coder_turns(&c).len();
    let turn = ChatTurn::detached();
    fx.eng()
        .note_mode_continue(&ws.id, &tab_b.code, CHAT_MODE_AGENT, CHAT_MODE_PLAN, &turn);
    fx.eng()
        .maybe_continue_after_mode_switch(&ws.id, &tab_b.code, &turn);
    fx.eng().remove_workspace(&ws.id).await.expect("remove");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(fx.te.store.agent(&ws.id, &tab_b.code).is_none());
    assert_eq!(coder_turns(&c).len(), before);
    assert_eq!(fx.items(&ws.id, &tab_b.code).await.len(), before_items);
    fx.te.close().await;
}

#[tokio::test]
async fn remove_workspace_waits_for_a_chat_life_prologue() {
    let fx = Fx::new(Some(capture("life", "OK.").provider()));
    let ws = fx.workspace("life-wait").await;
    let (mut life, held) = fx.eng().lock_chat_life(&ws.id, "PA").await;
    assert!(held);
    let (eng, id) = (fx.eng().clone(), ws.id.clone());
    let remove = tokio::spawn(async move { eng.remove_workspace(&id).await });
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        !remove.is_finished(),
        "Remove returned while a prologue held the lock"
    );
    assert!(!fx.ws(&ws.id).archived);
    life.unlock();
    tokio::time::timeout(Duration::from_secs(2), remove)
        .await
        .expect("remove finishes")
        .unwrap()
        .expect("remove ok");
    assert!(fx.ws(&ws.id).archived);
    fx.te.close().await;
}

#[tokio::test]
async fn persist_chat_failure_skips_a_committed_teardown() {
    let fx = Fx::new(Some(capture("fail", "OK.").provider()));
    let ws = fx.workspace("fail-archive").await;
    let eng = fx.eng();
    fx.send_mode(&ws.id, "PA", "hi", "sonnet", CHAT_MODE_AGENT)
        .await;
    let before = fx.items(&ws.id, "PA").await.len();
    eng.deleting.lock().insert(ws.id.clone());
    eng.persist_chat_failure(
        &ws.id,
        "PA",
        &format_mode_switch_resume_prompt(CHAT_MODE_AGENT, CHAT_MODE_PLAN),
        &format_mode_switch_display(CHAT_MODE_PLAN),
        "I can't run right now: mcp bind failed",
        0,
        &recur_agent::TurnResult::default(),
        false,
        "",
    );
    assert_ne!(
        fx.items(&ws.id, "PA").await.len(),
        before,
        "a transient tombstone must not drop the note"
    );
    fx.te.store.update_workspace(&ws.id, |w| w.archived = true);
    let after_delete = fx.items(&ws.id, "PA").await.len();
    eng.persist_chat_failure(
        &ws.id,
        "PA",
        &format_mode_switch_resume_prompt(CHAT_MODE_AGENT, "ask"),
        &format_mode_switch_display("ask"),
        "I can't find that workspace anymore — it may have been removed.",
        0,
        &recur_agent::TurnResult::default(),
        false,
        "",
    );
    assert_eq!(fx.items(&ws.id, "PA").await.len(), after_delete);
    eng.deleting.lock().remove(&ws.id);
    fx.te.close().await;
}

#[tokio::test]
async fn dropping_chat_life_reclaims_a_live_remount_generation() {
    let fx = Fx::new(None);
    let eng = fx.eng();
    let turn = ChatTurn::detached();
    eng.note_mode_continue("ws-epoch", "PA", CHAT_MODE_AGENT, CHAT_MODE_PLAN, &turn);
    let generation = eng.pending_mode_continue_epoch("ws-epoch", "PA").unwrap();
    assert!(generation != 0 && eng.continue_gen_live(generation));
    eng.drop_chat_life_for_workspace("ws-epoch");
    assert!(
        !eng.continue_gen_live(generation),
        "teardown reclaims the generation"
    );
    assert!(!eng.workspace_has_live_continue("ws-epoch"));
    fx.te.close().await;
}

#[tokio::test]
async fn delete_closed_chat_drops_owned_specialists() {
    let fx = Fx::new(Some(capture("del-own", "Done.").provider()));
    let ws = fx.workspace("delete-owned").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "keep me", "sonnet").await;
    let tab_b = eng.create_workspace_chat(&ws.id).await;
    fx.send(&ws.id, &tab_b.code, "conductor", "sonnet").await;
    let co = fx
        .te
        .store
        .create_session(Session {
            workspace_id: ws.id.clone(),
            kind: AgentRole::Coder.as_str().into(),
            provider: PROVIDER_CLAUDE_CODE.into(),
            config_json: format!(
                r#"{{"code":"CO","name":"Coder","ownerCode":"{}"}}"#,
                tab_b.code
            ),
            ..Session::default()
        })
        .unwrap();
    fx.te
        .store
        .append_message(
            &co.id,
            Message {
                role: "assistant".into(),
                content: Some(vec![ContentBlock::text("old coder turn")]),
                ..Message::default()
            },
        )
        .unwrap();
    assert_eq!(
        tab_codes(&eng.close_workspace_chat(&ws.id, &tab_b.code).await),
        ["PA"]
    );
    let (left, err) = eng.delete_closed_workspace_chat(&ws.id, &tab_b.code).await;
    assert!(err.is_ok() && left.is_empty());
    assert!(
        fx.te
            .store
            .session_for_chat_agent(&ws.id, &tab_b.code, "CO")
            .is_none()
    );
    let fresh = eng.create_workspace_chat(&ws.id).await;
    assert!(
        fx.te
            .store
            .session_for_chat_agent(&ws.id, &fresh.code, "CO")
            .is_none()
    );
    fx.te.close().await;
}

#[tokio::test]
async fn closing_the_last_pair_tab_on_a_run_seeds_a_fresh_chat() {
    let fx = Fx::new(Some(capture("lt", "OK.").provider()));
    let ws = fx.workspace("loop-tabs").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    fx.te.store.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.loop_chat_code = "PA".into();
    });
    assert_eq!(tab_codes(&eng.list_workspace_chats(&ws.id)), ["PA"]);
    let tabs = eng.close_workspace_chat(&ws.id, "PA").await;
    assert_eq!(tabs.len(), 1);
    assert_ne!(tabs[0].code, "MA");
    assert_eq!(tabs[0].title, "New chat");
    assert_eq!(fx.ws(&ws.id).loop_chat_code, "");
    fx.te.close().await;
}

#[tokio::test]
async fn a_new_tab_keeps_the_maestro_tab() {
    let fx = Fx::new(Some(capture("mp", "OK.").provider()));
    let ws = fx.workspace("maestro-peer").await;
    let eng = fx.eng();
    fx.te.store.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.loop_chat_code = "MA".into();
    });
    assert_eq!(tab_codes(&eng.list_workspace_chats(&ws.id)), ["MA"]);
    let created = eng.create_workspace_chat(&ws.id).await;
    assert!(!created.code.is_empty() && created.code != "MA");
    assert_eq!(
        tab_codes(&eng.list_workspace_chats(&ws.id)),
        ["MA".to_owned(), created.code]
    );
    fx.te.close().await;
}

#[tokio::test]
async fn closing_the_maestro_tab_clears_the_loop_chat() {
    let fx = Fx::new(Some(capture("cm", "OK.").provider()));
    let ws = fx.workspace("close-maestro").await;
    let eng = fx.eng();
    fx.te.store.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.loop_chat_code = "MA".into();
    });
    eng.create_workspace_chat(&ws.id).await;
    let before = tab_codes(&eng.list_workspace_chats(&ws.id));
    assert_eq!(before.len(), 2);
    assert_eq!(before[0], "MA");
    let tabs = tab_codes(&eng.close_workspace_chat(&ws.id, "MA").await);
    assert!(tabs.len() == 1 && tabs[0] != "MA", "{tabs:?}");
    let listed = tab_codes(&eng.list_workspace_chats(&ws.id));
    assert!(listed.len() == 1 && listed[0] != "MA");
    assert_eq!(fx.ws(&ws.id).loop_chat_code, "");

    // The last MA tab on a run seeds a fresh pair tab.
    let other = fx.workspace("close-last-maestro").await;
    fx.te.store.update_workspace(&other.id, |w| {
        w.run_started = true;
        w.loop_chat_code = "MA".into();
    });
    assert_eq!(tab_codes(&eng.list_workspace_chats(&other.id)), ["MA"]);
    let tabs = tab_codes(&eng.close_workspace_chat(&other.id, "MA").await);
    assert!(tabs.len() == 1 && tabs[0] != "MA", "{tabs:?}");
    let listed = tab_codes(&eng.list_workspace_chats(&other.id));
    assert!(listed.len() == 1 && listed[0] != "MA");
    fx.te.close().await;
}

#[test]
fn apply_chat_tab_order_cases() {
    let t = |c: &str| WorkspaceChatTab {
        code: c.into(),
        ..WorkspaceChatTab::default()
    };
    let def = || vec![t("MA"), t("PA"), t("PA2"), t("PA3")];
    let o = |xs: &[&str]| xs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    type Case<'a> = (&'a str, Vec<WorkspaceChatTab>, Vec<String>, Vec<&'a str>);
    let cases: Vec<Case> = vec![
        ("empty order", def(), vec![], vec!["MA", "PA", "PA2", "PA3"]),
        ("single tab", vec![t("PA")], o(&["PA2", "PA"]), vec!["PA"]),
        (
            "saved permutation",
            def(),
            o(&["PA2", "MA", "PA3", "PA"]),
            vec!["PA2", "MA", "PA3", "PA"],
        ),
        (
            "closed codes skipped",
            vec![t("MA"), t("PA"), t("PA3")],
            o(&["PA2", "PA3", "MA", "PA"]),
            vec!["PA3", "MA", "PA"],
        ),
        (
            "new pair tab appends",
            def(),
            o(&["PA2", "MA", "PA"]),
            vec!["PA2", "MA", "PA", "PA3"],
        ),
        (
            "new Loop still leads",
            def(),
            o(&["PA2", "PA", "PA3"]),
            vec!["MA", "PA2", "PA", "PA3"],
        ),
        (
            "duplicates ignored",
            vec![t("PA"), t("PA2")],
            o(&["PA2", "PA2", "PA"]),
            vec!["PA2", "PA"],
        ),
    ];
    for (name, tabs, order, want) in cases {
        assert_eq!(
            tab_codes(&apply_chat_tab_order(tabs, &order)),
            want,
            "{name}"
        );
    }
}
