//! Port of Go `app_chat_test.go`: the Ask Recur turn shape, its status
//! events, tabs and unseen, Stop, selection, and present_workspace.

use recur_agent_tools::names::mcp;
use recur_domain::{APP_AGENT_CODE, AgentRole, Project, SESSION_KIND_APP, Workspace};
use tokio_util::sync::CancellationToken;

use super::APP_CHAT_TOOLS;
use crate::bus::EngineEvent;
use crate::loops::test_support::CaptureScript;
use crate::testing::{BusRecorder, TestEngine};
use crate::turn::test_support::maestro_fixture;

fn scripted(session: &'static str, reply: &'static str) -> std::sync::Arc<CaptureScript> {
    CaptureScript::new(move |_| maestro_fixture(session, reply))
}

#[tokio::test]
async fn send_app_message_uses_the_ask_coordinator_shape() {
    let c = scripted("app-sess-1", "Needs you: LOOP-2218.");
    let te = TestEngine::new(Some(c.provider()));
    let reply = te
        .engine
        .send_app_message("", "What needs me?", "sonnet", None)
        .await;
    assert!(reply.contains("Needs you: LOOP-2218"), "{reply}");
    let spec = c.with_role(AgentRole::Maestro).remove(0);
    assert_eq!(spec.agent_id, APP_AGENT_CODE);
    assert_eq!(
        (spec.route.kind.as_str(), spec.route.code.as_str()),
        (SESSION_KIND_APP, APP_AGENT_CODE)
    );
    assert!(spec.append_system_prompt.contains("Ask Recur"));
    let tools = spec.allowed_tools.clone().unwrap_or_default();
    assert_eq!(tools, APP_CHAT_TOOLS, "the three briefing tools");
    for tl in &tools {
        assert!(
            !["Edit", "Write", mcp::ASK_USER, mcp::ESCALATE,].contains(&tl.as_str()),
            "coordinator must not get {tl}"
        );
    }
    assert!(tools.iter().any(|t| t == mcp::LIST_WORKSPACE_BRIEFS));
    assert!(tools.iter().any(|t| t == mcp::GET_WORKSPACE_CONTEXT));
    assert!(tools.iter().any(|t| t == mcp::PRESENT_WORKSPACE));
    assert_eq!(spec.worktree, te.data_dir.join("app-chat"));
    let sess = te
        .store
        .session_for_app_kind(SESSION_KIND_APP)
        .expect("app session");
    assert!(sess.workspace_id.is_empty());
    assert!(!te.events.has("workspace.presented"));
    te.close().await;
}

#[tokio::test]
async fn an_empty_app_message_is_a_noop() {
    let te = TestEngine::new(None);
    assert_eq!(
        te.engine.send_app_message("", "  ", "sonnet", None).await,
        ""
    );
    assert!(!te.events.has("agent.updated"));
    te.close().await;
}

fn assert_app_status_seq(events: &BusRecorder, code: &str) {
    let want = format!("app-{code}");
    let mut statuses = Vec::new();
    for ev in events.by_name("agent.updated") {
        let EngineEvent::AgentUpdated(d) = ev else {
            continue;
        };
        if d.agent_id != want {
            continue;
        }
        assert!(d.workspace_id.is_empty(), "{code}: {d:?}");
        assert_eq!(d.surface.as_deref(), Some("app"), "{code}");
        assert_eq!(d.code.as_deref(), Some(code));
        statuses.push(d.status);
    }
    assert!(
        statuses.len() >= 2
            && statuses[0] == "working"
            && statuses.last().map(String::as_str) == Some("idle"),
        "{code}: statuses = {statuses:?}"
    );
}

#[tokio::test]
async fn send_app_message_publishes_running_status() {
    let te = TestEngine::new(Some(scripted("app-sess-run", "ok").provider()));
    let reply = te
        .engine
        .send_app_message("", "What needs me?", "sonnet", None)
        .await;
    assert!(reply.contains("ok"), "{reply}");
    assert_app_status_seq(&te.events, APP_AGENT_CODE);
    assert_eq!(te.engine.new_app_chat().code, "AP2");
    te.engine
        .send_app_message("AP2", "and another", "sonnet", None)
        .await;
    assert_app_status_seq(&te.events, "AP2");
    te.close().await;
}

#[tokio::test]
async fn present_workspace_validates_targets() {
    let te = TestEngine::new(None);
    te.store
        .add_project(Project {
            id: "p1".into(),
            name: "Storefront".into(),
            ..Project::default()
        })
        .unwrap();
    te.store
        .add_workspace(Workspace {
            id: "W-live".into(),
            project_id: "p1".into(),
            short: "Auth".into(),
            ..Workspace::default()
        })
        .unwrap();
    let eng = &te.engine;
    for t in ["", "workspace:missing", "chat:W-live"] {
        assert!(eng.present_workspace(t).contains("Refused"), "{t:?}");
    }
    let live = eng.present_workspace("workspace:W-live");
    assert!(
        live.contains(r#""ok":true"#) && live.contains("workspace:W-live"),
        "{live}"
    );
    assert!(live.contains(r#""title":"Auth""#), "{live}");
    let plan = eng.present_workspace("workspace:W-live:plan");
    assert!(plan.contains(r#""ok":true"#), "{plan}");
    assert!(!te.events.has("workspace.presented"));
    te.close().await;
}

#[tokio::test]
async fn app_chat_unseen_and_focus() {
    let te = TestEngine::new(None);
    te.engine.flag_app_chat_unseen("");
    let c = te.store.attention_counts();
    assert_eq!(c.app_unseen, 1);
    assert!(c.dock >= 1, "dock must include app unseen: {c:?}");
    te.engine.set_app_chat_focus(true, "");
    assert_eq!(te.store.attention_counts().app_unseen, 0);
    te.close().await;
}

#[tokio::test]
async fn app_chat_tabs_lifecycle() {
    let te = TestEngine::new(None);
    let eng = &te.engine;
    let codes = |tabs: &[recur_domain::WorkspaceChatTab]| {
        tabs.iter().map(|t| t.code.clone()).collect::<Vec<_>>()
    };
    assert_eq!(codes(&eng.list_app_chats()), [APP_AGENT_CODE]);
    assert_eq!(eng.new_app_chat().code, "AP2");
    assert_eq!(codes(&eng.list_app_chats()), [APP_AGENT_CODE, "AP2"]);
    eng.flag_app_chat_unseen("AP2");
    eng.flag_app_chat_unseen("");
    assert_eq!(te.store.attention_counts().app_unseen, 2);
    eng.mark_app_chat_seen("AP2");
    assert_eq!(te.store.attention_counts().app_unseen, 1);
    eng.flag_app_chat_unseen("AP2");
    assert_eq!(codes(&eng.close_app_chat("AP2")), [APP_AGENT_CODE]);
    assert_eq!(te.store.attention_counts().app_unseen, 1, "APP only");
    let tabs = eng.close_app_chat(APP_AGENT_CODE);
    assert!(
        codes(&tabs) == [APP_AGENT_CODE] && !tabs[0].unseen,
        "closing APP resets it: {tabs:?}"
    );
    assert_eq!(te.store.attention_counts().app_unseen, 0);
    assert_eq!(eng.new_app_chat().code, "AP3", "AP2 is not reused");
    te.close().await;
}

#[tokio::test]
async fn cancel_app_message_interrupts_a_live_turn() {
    let te = TestEngine::new(None);
    let eng = &te.engine;
    for code in ["APP", "AP2"] {
        let cancel = CancellationToken::new();
        let turn = eng
            .register_chat_turn("", code, cancel.clone())
            .unwrap_or_else(|| panic!("register_chat_turn({code})"));
        eng.cancel_app_message(code).await;
        assert!(cancel.is_cancelled(), "{code}");
        eng.unregister_chat_turn("", code, &turn);
    }
    assert!(eng.live_chat_turn("", "PA").is_none() && eng.live_chat_turn("", "MA").is_none());
    te.close().await;
}

#[tokio::test]
async fn app_chat_selection_persists() {
    let te = TestEngine::new(None);
    let f = [("effort".to_owned(), "low".to_owned())]
        .into_iter()
        .collect();
    te.engine
        .remember_app_chat_selection("", "sonnet", Some(&f));
    let (model, flags) = te.engine.app_chat_selection("");
    assert_eq!(model, "sonnet");
    assert_eq!(flags.get("effort").map(String::as_str), Some("low"));
    te.close().await;
}

#[tokio::test]
async fn send_app_message_omits_user_mcp() {
    let c = scripted("app-mcp", "ok");
    let te = TestEngine::new(Some(c.provider()));
    let mut set = te.store.settings();
    set.mcp_servers = vec![recur_domain::McpServer {
        id: "acme".into(),
        enabled: true,
        transport: "http".into(),
        url: "https://example.com/mcp".into(),
        ..Default::default()
    }];
    te.store.set_settings(set);
    te.engine
        .send_app_message("", "What needs me?", "sonnet", None)
        .await;
    let spec = c.with_role(AgentRole::Maestro).remove(0);
    assert!(spec.user_mcp.is_empty(), "{:?}", spec.user_mcp);
    let tools = spec.allowed_tools.clone().unwrap_or_default();
    assert!(!tools.iter().any(|t| t == "mcp__acme__*"));
    assert_eq!(tools.len(), 3);
    te.close().await;
}
