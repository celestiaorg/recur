//! Port of Go `attention_test.go` and `chat_inbox_test.go`. Cases that need
//! later slices move with them: the ask_user parks (E2 broker),
//! CloseWorkspaceChat races (E4 tabs), dispatch of closed conductors (E3),
//! and the Docs loop end-to-end (E3).

use std::sync::Arc;

use recur_domain::{Agent, InboxItem, Pr, PrSource, Session, Workspace, WorkspaceState};

use super::chat_inbox::{chat_inbox_id, chat_inbox_target, parse_chat_inbox_id};
use crate::codes::{MAESTRO_AGENT_CODE, PAIR_AGENT_CODE};
use crate::loop_handle::{LoopHandle, LoopState};
use crate::testing::TestEngine;

const PROJ: &str = "proj";

fn workspace(t: &TestEngine, id: &str) {
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            project_id: PROJ.into(),
            short: "attention".into(),
            r#type: recur_domain::WORKSPACE_SOLO.into(),
            ..Workspace::default()
        })
        .unwrap();
}

/// Go `ensurePairAgent`.
fn pair(t: &TestEngine, ws: &str, code: &str) {
    if t.store.agent(ws, code).is_some() {
        return;
    }
    t.engine.upsert_agent(
        ws,
        Agent {
            id: format!("{ws}-{code}"),
            workspace_id: ws.into(),
            code: code.into(),
            name: "Agent".into(),
            role: "codes alongside you".into(),
            status: "idle".into(),
            color: "var(--cyan)".into(),
            cur: -1,
            ..Agent::default()
        },
    );
}

/// Go `ensureMaestroAgentWithStatus` (the MA tab row).
fn maestro(t: &TestEngine, ws: &str) {
    t.engine.put_agent(
        ws,
        Agent {
            id: format!("{ws}-MA"),
            workspace_id: ws.into(),
            code: MAESTRO_AGENT_CODE.into(),
            name: "Maestro".into(),
            status: "idle".into(),
            color: "var(--amber)".into(),
            ..Agent::default()
        },
    );
}

fn inbox(t: &TestEngine, id: &str) -> Option<InboxItem> {
    t.store
        .inbox()
        .unwrap_or_default()
        .into_iter()
        .find(|i| i.id == id)
}

fn unseen(t: &TestEngine, ws: &str, code: &str) -> bool {
    t.store.agent(ws, code).is_some_and(|a| a.unseen)
}

fn finish(t: &TestEngine, ws: &str, code: &str) {
    t.engine.update_agent_status(ws, code, "working");
    t.engine.update_agent_status(ws, code, "idle");
}

fn walk(ws: &str, chat: &str, state: WorkspaceState, phase: &str) -> Arc<LoopHandle> {
    LoopHandle::new(
        ws,
        PROJ,
        ws,
        LoopState {
            chat_code: chat.into(),
            exit_state: state,
            exit_phase: phase.into(),
            ..LoopState::default()
        },
    )
}

#[test]
fn parse_chat_inbox_ids() {
    assert_eq!(
        parse_chat_inbox_id(&chat_inbox_id("demo.golden-burrito", "PA2")),
        Some(("demo.golden-burrito".into(), "PA2".into()))
    );
    assert_eq!(
        parse_chat_inbox_id(&chat_inbox_id("LOOP-2218", "MA")),
        Some(("LOOP-2218".into(), "MA".into()))
    );
    assert_eq!(parse_chat_inbox_id("ib-ask-x"), None, "non-chat id");
    assert_eq!(parse_chat_inbox_id("ib-chat-only"), None, "missing code");
    assert_eq!(
        parse_chat_inbox_id("ib-chat-ws-CO"),
        None,
        "specialist code"
    );
}

#[tokio::test]
async fn unseen_chat_attention() {
    let t = TestEngine::new(None);
    let ws = "proj.attention";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    pair(&t, ws, "PA2");

    // Background tab finishes → unseen.
    finish(&t, ws, "PA2");
    assert!(unseen(&t, ws, "PA2"));
    assert_eq!(t.store.unseen_chat_count(), 1);
    assert!(t.events.has("attention.updated"));
    assert!(t.events.has("inbox.updated"));
    let it = inbox(&t, &chat_inbox_id(ws, "PA2")).expect("unseen PA2 row");
    assert!(it.unread && it.kind == "chat");
    let c = t.engine.attention_counts();
    assert_eq!(c.unseen, 1);
    assert_eq!(c.by_workspace.as_ref().and_then(|m| m.get(ws)), Some(&1));
    assert_eq!(c.by_project.as_ref().and_then(|m| m.get(PROJ)), Some(&1));

    // Focus clears unseen.
    t.engine.set_workspace_chat_focus(ws, "PA2");
    assert!(!unseen(&t, ws, "PA2"));
    assert_eq!(t.store.unseen_chat_count(), 0);
    assert!(inbox(&t, &chat_inbox_id(ws, "PA2")).is_none());

    // The focused tab finishing does not raise unseen.
    finish(&t, ws, "PA2");
    assert!(!unseen(&t, ws, "PA2"));

    // A specialist going idle never sets unseen.
    t.store.upsert_agent(
        ws,
        Agent {
            id: format!("{ws}-CO"),
            workspace_id: ws.into(),
            code: "CO".into(),
            status: "working".into(),
            ..Agent::default()
        },
    );
    t.engine.update_agent_status(ws, "CO", "idle");
    assert!(!unseen(&t, ws, "CO"));

    // Dock = unseen + non-chat inbox unread.
    t.store.mark_all_inbox_read();
    t.store.add_inbox_item(InboxItem {
        id: "ib-att".into(),
        group: "needs".into(),
        title: "needs you".into(),
        unread: true,
        time: "now".into(),
        ..InboxItem::default()
    });
    t.engine.set_workspace_chat_focus(ws, "PA2");
    finish(&t, ws, PAIR_AGENT_CODE);
    let c = t.engine.attention_counts();
    assert_eq!((c.inbox_unread, c.unseen, c.dock), (2, 1, 2), "{c:?}");
    let it = inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).expect("PA row");
    assert_eq!(it.target, chat_inbox_target(ws, PAIR_AGENT_CODE));
    assert!(unseen(&t, ws, PAIR_AGENT_CODE));
    assert!(!unseen(&t, ws, "PA2"));

    let before = t.store.inbox().unwrap_or_default().len();
    t.engine.update_agent_status(ws, "PA2", "working");
    t.engine.set_workspace_chat_focus("", "");
    t.engine.update_agent_status(ws, "PA2", "idle");
    assert_eq!(
        t.store.inbox().unwrap_or_default().len(),
        before + 1,
        "unseen chat must create an inbox row"
    );
}

#[tokio::test]
async fn unseen_chat_attention_surface() {
    let t = TestEngine::new(None);
    let ws = "proj.surface";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.engine.set_workspace_chat_focus(ws, PAIR_AGENT_CODE);

    t.engine.set_app_focused(false);
    finish(&t, ws, PAIR_AGENT_CODE);
    assert!(
        unseen(&t, ws, PAIR_AGENT_CODE),
        "unfocused app: selected tab goes unseen"
    );
    assert_eq!(t.engine.attention_counts().dock, 1);

    t.engine.set_app_focused(true);
    assert!(
        !unseen(&t, ws, PAIR_AGENT_CODE),
        "refocus marks the selected tab seen"
    );
    assert_eq!(t.engine.attention_counts().dock, 0);
    assert!(t.events.has("app.focused"));

    t.engine.set_chat_ui_occluded(true);
    finish(&t, ws, PAIR_AGENT_CODE);
    assert!(unseen(&t, ws, PAIR_AGENT_CODE), "occluded chat goes unseen");
    t.engine.set_chat_ui_occluded(false);
    assert!(
        !unseen(&t, ws, PAIR_AGENT_CODE),
        "clearing occlusion marks seen"
    );

    t.engine.set_chat_ui_occluded(true);
    finish(&t, ws, PAIR_AGENT_CODE);
    t.engine.set_workspace_chat_focus(ws, PAIR_AGENT_CODE);
    assert!(
        unseen(&t, ws, PAIR_AGENT_CODE),
        "focus while occluded must not clear"
    );
    t.engine.set_chat_ui_occluded(false);
}

#[tokio::test]
async fn loop_finished_raises_unseen() {
    let t = TestEngine::new(None);
    let ws = "proj.loop-done";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store
        .update_workspace(ws, |w| w.loop_chat_code = PAIR_AGENT_CODE.into());

    t.engine
        .maybe_flag_loop_finished(&walk(ws, PAIR_AGENT_CODE, WorkspaceState::Paused, "idle"))
        .await;
    assert!(unseen(&t, ws, PAIR_AGENT_CODE));
    let it = inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).expect("loop-finished row");
    assert!(it.unread && it.kind == "chat");
    assert!(
        it.title.contains("Loop") && it.title.contains("finished"),
        "{}",
        it.title
    );
    assert!(!it.title.contains("reply"), "{}", it.title);
    let c = t.engine.attention_counts();
    assert_eq!(c.unseen, 1);
    assert!(t.events.has("attention.updated") && t.events.has("inbox.updated"));

    t.engine.set_workspace_chat_focus(ws, PAIR_AGENT_CODE);
    assert!(!unseen(&t, ws, PAIR_AGENT_CODE));
    assert!(inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).is_none());
}

#[tokio::test]
async fn loop_finished_skips_stop_and_idle_cancel() {
    let t = TestEngine::new(None);
    let ws = "proj.loop-stop";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store
        .update_workspace(ws, |w| w.loop_chat_code = PAIR_AGENT_CODE.into());
    t.engine
        .maybe_flag_loop_finished(&walk(
            ws,
            PAIR_AGENT_CODE,
            WorkspaceState::Stopped,
            "stopped",
        ))
        .await;
    assert!(
        !unseen(&t, ws, PAIR_AGENT_CODE),
        "stopped loop must not raise unseen"
    );

    let idle = walk(ws, PAIR_AGENT_CODE, WorkspaceState::Paused, "idle");
    idle.request_idle();
    t.engine.maybe_flag_loop_finished(&idle).await;
    assert!(
        !unseen(&t, ws, PAIR_AGENT_CODE),
        "composer Stop idle must not raise unseen"
    );

    let stopped = walk(
        ws,
        PAIR_AGENT_CODE,
        WorkspaceState::PrOpen,
        "PR open — ready for your review",
    );
    stopped.lock().stopped = true;
    t.engine.maybe_flag_loop_finished(&stopped).await;
    assert!(
        !unseen(&t, ws, PAIR_AGENT_CODE),
        "Stop after settle must not raise unseen"
    );

    t.engine.set_workspace_chat_focus(ws, PAIR_AGENT_CODE);
    t.engine
        .maybe_flag_loop_finished(&walk(
            ws,
            PAIR_AGENT_CODE,
            WorkspaceState::PrOpen,
            "PR open — ready for your review",
        ))
        .await;
    assert!(
        !unseen(&t, ws, PAIR_AGENT_CODE),
        "focused tab must not raise unseen on PR-open"
    );
}

#[tokio::test]
async fn loop_finished_uses_walk_exit_not_workspace() {
    let t = TestEngine::new(None);
    let ws = "proj.loop-sibling";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store.update_workspace(ws, |w| {
        w.state = WorkspaceState::Coding;
        w.phase = "implementing".into();
    });
    t.engine
        .maybe_flag_loop_finished(&walk(
            ws,
            PAIR_AGENT_CODE,
            WorkspaceState::PrOpen,
            "PR open — ready for your review",
        ))
        .await;
    assert!(
        unseen(&t, ws, PAIR_AGENT_CODE),
        "settled walk badges even if a sibling overwrote state"
    );
    let it = inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).unwrap();
    assert!(it.title.contains("Loop") && !it.title.contains("reply"));

    t.engine.set_workspace_chat_focus(ws, PAIR_AGENT_CODE);
    t.store.update_workspace(ws, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
    });
    t.engine.set_workspace_chat_focus("", "");
    t.engine
        .maybe_flag_loop_finished(&walk(
            ws,
            PAIR_AGENT_CODE,
            WorkspaceState::Coding,
            "implementing",
        ))
        .await;
    assert!(
        !unseen(&t, ws, PAIR_AGENT_CODE),
        "in-flight walk must not badge from a sibling's state"
    );
}

#[tokio::test]
async fn loop_finished_does_not_recreate_closed_tab() {
    let t = TestEngine::new(None);
    let ws = "proj.loop-closed";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store.delete_agent(ws, PAIR_AGENT_CODE);
    t.engine
        .maybe_flag_loop_finished(&walk(ws, PAIR_AGENT_CODE, WorkspaceState::Paused, "idle"))
        .await;
    assert!(t.store.agent(ws, PAIR_AGENT_CODE).is_none());
    assert!(inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).is_none());
}

#[tokio::test]
async fn loop_finished_legacy_empty_chat_code_badges_ma() {
    let t = TestEngine::new(None);
    let ws = "proj.legacy-ma-done";
    workspace(&t, ws);
    t.store.update_workspace(ws, |w| {
        w.run_started = true;
        w.loop_chat_code = MAESTRO_AGENT_CODE.into();
    });
    maestro(&t, ws);
    t.store.update_workspace(ws, |w| w.loop_chat_code.clear());
    t.engine
        .maybe_flag_loop_finished(&walk(ws, "", WorkspaceState::Paused, "idle"))
        .await;
    assert!(unseen(&t, ws, MAESTRO_AGENT_CODE));
    let it = inbox(&t, &chat_inbox_id(ws, MAESTRO_AGENT_CODE)).unwrap();
    assert!(it.title.contains("Loop") && !it.title.contains("reply"));
}

#[tokio::test]
async fn loop_finished_legacy_empty_does_not_follow_pa_owner() {
    let t = TestEngine::new(None);
    let ws = "proj.legacy-ma-pa-owner";
    workspace(&t, ws);
    maestro(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store.update_workspace(ws, |w| {
        w.run_started = true;
        w.loop_chat_code = PAIR_AGENT_CODE.into();
    });
    t.engine
        .maybe_flag_loop_finished(&walk(ws, "", WorkspaceState::Paused, "idle"))
        .await;
    assert!(unseen(&t, ws, MAESTRO_AGENT_CODE));
    assert!(
        !unseen(&t, ws, PAIR_AGENT_CODE),
        "legacy MA walk must not badge the later PA owner"
    );
    assert!(inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).is_none());

    t.engine.set_workspace_chat_focus(ws, MAESTRO_AGENT_CODE);
    t.store.delete_agent(ws, MAESTRO_AGENT_CODE);
    t.engine.set_workspace_chat_focus("", "");
    t.engine
        .maybe_flag_loop_finished(&walk(ws, "", WorkspaceState::Paused, "idle"))
        .await;
    assert!(
        t.store.agent(ws, MAESTRO_AGENT_CODE).is_none(),
        "closed legacy MA must not be recreated"
    );
    assert!(!unseen(&t, ws, PAIR_AGENT_CODE));
}

#[tokio::test]
async fn ordinary_reply_on_settled_loop_is_not_loop_finished() {
    let t = TestEngine::new(None);
    let ws = "proj.loop-reply";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store.update_workspace(ws, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
        w.loop_chat_code = PAIR_AGENT_CODE.into();
    });
    t.engine.set_workspace_chat_focus("", "");
    finish(&t, ws, PAIR_AGENT_CODE);
    let it = inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).expect("ordinary reply row");
    assert!(it.title.contains("finished a reply"), "{}", it.title);
}

#[tokio::test]
async fn upsert_does_not_resurrect_closed_ma() {
    let t = TestEngine::new(None);
    let ws = "proj.pa-owns-closed-ma";
    workspace(&t, ws);
    t.engine.upsert_agent(
        ws,
        Agent {
            id: format!("{ws}-MA"),
            workspace_id: ws.into(),
            code: MAESTRO_AGENT_CODE.into(),
            name: "Maestro".into(),
            status: "planning".into(),
            ..Agent::default()
        },
    );
    assert!(
        t.store.agent(ws, MAESTRO_AGENT_CODE).is_none(),
        "a planner upsert never creates MA"
    );
    maestro(&t, ws);
    t.engine.upsert_agent(
        ws,
        Agent {
            id: format!("{ws}-MA"),
            workspace_id: ws.into(),
            code: MAESTRO_AGENT_CODE.into(),
            status: "planning".into(),
            ..Agent::default()
        },
    );
    assert_eq!(
        t.store.agent(ws, MAESTRO_AGENT_CODE).unwrap().status,
        "planning"
    );
    let codes = t
        .store
        .workspace(ws)
        .unwrap()
        .agent_codes
        .unwrap_or_default();
    assert_eq!(codes, vec!["MA".to_owned()]);
}

#[tokio::test]
async fn sync_chat_inbox_heals_from_unseen() {
    let t = TestEngine::new(None);
    let ws = "proj.sync-inbox";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store
        .update_agent(ws, PAIR_AGENT_CODE, |a| a.unseen = true);
    assert!(inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).is_none());
    t.engine.sync_chat_inbox();
    let it = inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).expect("healed row");
    assert!(it.unread && it.kind == "chat");
    assert!(it.title.contains("New chat"), "untitled tab: {}", it.title);

    t.store
        .update_agent(ws, PAIR_AGENT_CODE, |a| a.unseen = false);
    t.engine.sync_chat_inbox();
    assert!(
        inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).is_none(),
        "stale row dropped"
    );
}

#[tokio::test]
async fn sync_chat_inbox_does_not_infer_loop_finished() {
    let t = TestEngine::new(None);
    let ws = "proj.sync-settled";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store.update_workspace(ws, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
        w.loop_chat_code = PAIR_AGENT_CODE.into();
    });
    t.store
        .update_agent(ws, PAIR_AGENT_CODE, |a| a.unseen = true);
    t.engine.sync_chat_inbox();
    let it = inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).unwrap();
    assert!(it.title.contains("finished a reply"), "{}", it.title);
}

#[tokio::test]
async fn chat_inbox_title_and_sub() {
    let t = TestEngine::new(None);
    let ws = "proj.titled";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    t.store
        .create_session(Session {
            workspace_id: ws.into(),
            kind: "provider".into(),
            config_json: r#"{"code":"PA","name":"Agent","title":"Fix the <login> bug please now"}"#
                .into(),
            ..Session::default()
        })
        .unwrap();
    t.store.update_workspace(ws, |w| w.short = "A & B".into());
    assert_eq!(
        t.engine.chat_tab_title(ws, PAIR_AGENT_CODE),
        "Fix the <login> bug please …"
    );
    t.engine.set_workspace_chat_focus("", "");
    finish(&t, ws, PAIR_AGENT_CODE);
    let it = inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).unwrap();
    assert_eq!(
        it.title,
        "<b>Fix the &lt;login&gt; bug please …</b> finished a reply"
    );
    assert_eq!(it.sub, "A &amp; B");
    assert_eq!(it.color, "var(--cyan)");
    assert_eq!(
        (
            it.group.as_str(),
            it.action.as_str(),
            it.action_class.as_str()
        ),
        ("ready", "View", "go")
    );
}

#[tokio::test]
async fn first_user_message_titles_the_row() {
    use super::chat_inbox::first_user_chat_title;
    assert_eq!(
        first_user_chat_title("\n\n  hello there \nsecond"),
        "hello there"
    );
    assert_eq!(
        first_user_chat_title(crate::codes::CHAT_RETRY_DISPLAY_SENTINEL),
        ""
    );
    assert_eq!(first_user_chat_title("   "), "");
    assert_eq!(first_user_chat_title(&"é".repeat(300)).chars().count(), 200);
}

#[tokio::test]
async fn mark_all_chats_seen_clears_inbox() {
    let t = TestEngine::new(None);
    let ws = "proj.mark-all";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    pair(&t, ws, "PA2");
    finish(&t, ws, PAIR_AGENT_CODE);
    finish(&t, ws, "PA2");
    assert_eq!(t.store.unseen_chat_count(), 2);
    let inbox_before = t.events.count("inbox.updated");
    let att_before = t.events.count("attention.updated");
    t.engine.mark_all_chats_seen();
    assert_eq!(t.store.unseen_chat_count(), 0);
    assert!(
        t.store
            .inbox()
            .unwrap_or_default()
            .iter()
            .all(|i| i.kind != "chat")
    );
    assert_eq!(t.events.count("inbox.updated") - inbox_before, 1);
    assert_eq!(t.events.count("attention.updated") - att_before, 1);
    match t.events.by_name("attention.updated").last() {
        Some(crate::bus::EngineEvent::AttentionUpdated(a)) => assert!(a.clear_all_unseen),
        other => panic!("{other:?}"),
    }
    t.engine.mark_all_chats_seen();
    assert_eq!(
        t.events.count("attention.updated") - att_before,
        1,
        "nothing changed, no event"
    );
}

#[tokio::test]
async fn mark_all_chats_seen_drops_orphan_rows() {
    let t = TestEngine::new(None);
    t.store.add_inbox_item(InboxItem {
        id: "ib-chat-ghost-PA2".into(),
        kind: "chat".into(),
        unread: true,
        target: "chat:ghost:PA2".into(),
        ..InboxItem::default()
    });
    t.store.add_inbox_item(InboxItem {
        id: "ib-keep".into(),
        kind: "pause".into(),
        unread: true,
        target: "workspace:x".into(),
        ..InboxItem::default()
    });
    t.engine.mark_all_chats_seen();
    assert!(inbox(&t, "ib-chat-ghost-PA2").is_none());
    assert!(inbox(&t, "ib-keep").is_some());
}

#[tokio::test]
async fn mark_workspace_chat_seen_removes_orphan_row() {
    let t = TestEngine::new(None);
    let ws = "proj.orphan-row";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    let id = chat_inbox_id(ws, PAIR_AGENT_CODE);
    let row = InboxItem {
        id: id.clone(),
        kind: "chat".into(),
        unread: true,
        target: chat_inbox_target(ws, PAIR_AGENT_CODE),
        ..InboxItem::default()
    };
    t.store.add_inbox_item(row.clone());
    t.engine.mark_workspace_chat_seen(ws, PAIR_AGENT_CODE);
    assert!(
        inbox(&t, &id).is_none(),
        "orphan row with Unseen already false"
    );
    t.store.add_inbox_item(row);
    t.store.delete_agent(ws, PAIR_AGENT_CODE);
    t.engine.mark_workspace_chat_seen(ws, "");
    assert!(
        inbox(&t, &id).is_none(),
        "orphan row with the agent deleted"
    );
}

#[tokio::test]
async fn emit_chat_inbox_declines_when_seen() {
    let t = TestEngine::new(None);
    let ws = "proj.emit-seen";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    assert!(!t.engine.emit_chat_inbox(ws, PAIR_AGENT_CODE, false));
    assert!(inbox(&t, &chat_inbox_id(ws, PAIR_AGENT_CODE)).is_none());
    t.store.update_workspace(ws, |w| w.archived = true);
    t.store
        .update_agent(ws, PAIR_AGENT_CODE, |a| a.unseen = true);
    assert!(
        !t.engine.emit_chat_inbox(ws, PAIR_AGENT_CODE, false),
        "archived"
    );
}

#[tokio::test]
async fn pr_chat_host_unseen_lands_in_inbox() {
    let t = TestEngine::new(None);
    let pr = Pr {
        id: "gh-host-inbox".into(),
        project_id: PROJ.into(),
        number: 88,
        source: PrSource::GitHub,
        title: "Imported checkout".into(),
        branch: "feat/in".into(),
        base: "main".into(),
        group: "open".into(),
        state: "Open".into(),
        ..Pr::default()
    };
    t.store.add_pr(pr.clone()).unwrap();
    let host = "proj.host";
    t.store
        .add_workspace(Workspace {
            id: host.into(),
            project_id: PROJ.into(),
            host_pr_id: pr.id.clone(),
            short: "host".into(),
            ..Workspace::default()
        })
        .unwrap();
    pair(&t, host, PAIR_AGENT_CODE);
    finish(&t, host, PAIR_AGENT_CODE);
    let it = inbox(&t, &chat_inbox_id(host, PAIR_AGENT_CODE)).expect("host row");
    assert!(it.unread && it.kind == "chat");
    assert_eq!(it.target, chat_inbox_target(host, PAIR_AGENT_CODE));
    assert!(it.sub.contains("#88"), "{}", it.sub);
    let c = t.engine.attention_counts();
    assert_eq!(c.unseen, 1);
    assert_eq!(c.by_workspace.as_ref().and_then(|m| m.get(host)), Some(&1));
    assert_eq!(
        c.by_project
            .as_ref()
            .and_then(|m| m.get(PROJ))
            .copied()
            .unwrap_or(0),
        0,
        "a host has no sidebar row"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_agent_status_last_event_matches_store() {
    let t = TestEngine::new(None);
    let ws = "proj.status-mark-all-race";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    for i in 0..50 {
        t.engine.set_workspace_chat_focus("", "");
        t.store.update_agent(ws, PAIR_AGENT_CODE, |a| {
            a.unseen = false;
            a.status = "working".into();
        });
        let (e1, e2) = (Arc::clone(&t.engine), Arc::clone(&t.engine));
        let a = tokio::spawn(async move { e1.update_agent_status(ws, PAIR_AGENT_CODE, "idle") });
        let b = tokio::spawn(async move { e2.mark_all_chats_seen() });
        let _ = tokio::join!(a, b);
        let unseen_now = unseen(&t, ws, PAIR_AGENT_CODE);
        let mut last: Option<bool> = None;
        for ev in t.events.by_name("attention.updated") {
            if let crate::bus::EngineEvent::AttentionUpdated(d) = ev {
                if d.clear_all_unseen {
                    last = Some(false);
                } else if d.code == PAIR_AGENT_CODE && d.workspace_id == ws {
                    last = Some(d.tab_unseen);
                }
            }
        }
        assert!(
            unseen_now || last != Some(true),
            "iter {i}: store Unseen=false but last event tabUnseen=true"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_agent_status_focus_last_event_matches_store() {
    let t = TestEngine::new(None);
    let ws = "proj.status-focus-race";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    for i in 0..50 {
        t.engine.set_workspace_chat_focus("", "");
        t.store.update_agent(ws, PAIR_AGENT_CODE, |a| {
            a.unseen = false;
            a.status = "working".into();
        });
        let (e1, e2) = (Arc::clone(&t.engine), Arc::clone(&t.engine));
        let a = tokio::spawn(async move { e1.update_agent_status(ws, PAIR_AGENT_CODE, "idle") });
        let b = tokio::spawn(async move { e2.set_workspace_chat_focus(ws, PAIR_AGENT_CODE) });
        let _ = tokio::join!(a, b);
        assert!(
            !unseen(&t, ws, PAIR_AGENT_CODE),
            "iter {i}: focused tab left unseen"
        );
        let last = t
            .events
            .by_name("attention.updated")
            .into_iter()
            .rev()
            .filter_map(|ev| match ev {
                crate::bus::EngineEvent::AttentionUpdated(d)
                    if d.code == PAIR_AGENT_CODE && d.workspace_id == ws =>
                {
                    Some(d.tab_unseen)
                }
                _ => None,
            })
            .next();
        assert_ne!(
            last,
            Some(true),
            "iter {i}: last tabUnseen=true after mark-seen"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mark_all_chats_seen_last_event_matches_store() {
    let t = TestEngine::new(None);
    let ws = "proj.mark-all-race";
    workspace(&t, ws);
    pair(&t, ws, PAIR_AGENT_CODE);
    for i in 0..50 {
        t.engine.set_workspace_chat_focus("", "");
        t.store
            .update_agent(ws, PAIR_AGENT_CODE, |a| a.unseen = true);
        let (e1, e2) = (Arc::clone(&t.engine), Arc::clone(&t.engine));
        let a = tokio::spawn(async move { e1.flag_chat_unseen(ws, PAIR_AGENT_CODE, false) });
        let b = tokio::spawn(async move { e2.mark_all_chats_seen() });
        let _ = tokio::join!(a, b);
        let unseen_now = unseen(&t, ws, PAIR_AGENT_CODE);
        let mut last: Option<bool> = None;
        for ev in t.events.by_name("attention.updated") {
            if let crate::bus::EngineEvent::AttentionUpdated(d) = ev {
                if d.clear_all_unseen {
                    last = Some(false);
                } else if d.code == PAIR_AGENT_CODE && d.workspace_id == ws {
                    last = Some(d.tab_unseen);
                }
            }
        }
        assert!(
            unseen_now || last != Some(true),
            "iter {i}: store Unseen=false but last event tabUnseen=true"
        );
    }
}
