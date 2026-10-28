//! Port of the mode, dock-selection and tab-title cases of Go
//! `workspace_chat_test.go`: saved tab order, each mode's turn shape, the
//! selection persisting before the turn runs, prompt groups, standing-note
//! refreshes, provider stamping, Review findings, remapping picks off
//! disabled providers, and stored vs derived tab titles.

use std::sync::Arc;

use recur_agent_tools::names::mcp;
use recur_domain::{
    Agent, AgentRole, CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_REVIEW, CompatibleModel,
    CompatibleProvider, ContentBlock, Flags, Message, PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR,
    Session, Severity, Workspace,
};

use super::test_support::*;
use crate::composer::skills::COMMENT_HYGIENE_NEEDLE;
use crate::loops::test_support::CaptureScript;
use crate::session_config::AgentConfig;
use crate::text::truncate;

fn flags(pairs: &[(&str, &str)]) -> Flags {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[tokio::test]
async fn saved_tab_order_persists_and_new_tabs_append() {
    let fx = Fx::new(Some(capture("ord-sess", "OK.").provider()));
    let ws = fx.workspace("reorder-chats").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    assert_eq!(eng.create_workspace_chat(&ws.id).await.code, "PA2");
    assert_eq!(eng.create_workspace_chat(&ws.id).await.code, "PA3");
    let codes = |xs: &[&str]| xs.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let (tabs, err) = eng.reorder_workspace_chats(&ws.id, &codes(&["PA3", "PA", "PA2"]));
    assert!(err.is_ok());
    assert_eq!(tab_codes(&tabs), ["PA3", "PA", "PA2"]);
    assert_eq!(
        tab_codes(&eng.list_workspace_chats(&ws.id)),
        ["PA3", "PA", "PA2"]
    );
    assert_eq!(eng.create_workspace_chat(&ws.id).await.code, "PA4");
    assert_eq!(tab_codes(&eng.list_workspace_chats(&ws.id))[3], "PA4");
    fx.send(&ws.id, "PA3", "keep this tab", "sonnet").await;
    let before = tab_codes(&eng.list_workspace_chats(&ws.id));
    assert!(
        eng.reorder_workspace_chats(&ws.id, &codes(&["PA", "PA2"]))
            .1
            .is_err()
    );
    assert!(
        eng.reorder_workspace_chats(&ws.id, &codes(&["PA", "PA2", "PA3", "PA4", "PA9"]))
            .1
            .is_err()
    );
    assert_eq!(tab_codes(&eng.list_workspace_chats(&ws.id)), before);

    fx.te.store.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.loop_chat_code = "MA".into();
    });
    assert_eq!(tab_codes(&eng.list_workspace_chats(&ws.id))[0], "MA");
    assert!(
        eng.reorder_workspace_chats(&ws.id, &codes(&["PA", "MA", "PA3", "PA2", "PA4"]))
            .1
            .is_ok()
    );
    let got = tab_codes(&eng.list_workspace_chats(&ws.id));
    assert_eq!((got.len(), &got[0][..], &got[1][..]), (5, "PA", "MA"));
    let closed = tab_codes(&eng.close_workspace_chat(&ws.id, "PA3").await);
    assert_eq!(
        (closed.len(), &closed[0][..], &closed[1][..]),
        (4, "PA", "MA")
    );
    let reopened = tab_codes(&eng.reopen_workspace_chat(&ws.id, "PA3").await);
    assert_eq!((reopened.len(), &reopened[2][..]), (5, "PA3"));
    assert!(
        eng.reorder_workspace_chats("no-such-ws", &codes(&["PA"]))
            .1
            .is_err()
    );
    fx.te.close().await;
}

#[tokio::test]
async fn the_dock_mode_shapes_the_turn() {
    let c = capture("mode-sess", "OK.");
    let fx = Fx::new(Some(c.provider()));
    let mut set = fx.te.store.settings();
    set.bypass_permissions = true;
    fx.te.store.set_settings(set);
    let ws = fx.workspace("modes").await;
    let last = || coder_turns(&c).last().cloned().expect("coder turn");

    fx.send_mode(&ws.id, "PA", "what does this repo do?", "sonnet", "ask")
        .await;
    let s = last();
    for t in ["Edit", "Write", "Bash", "NotebookEdit"] {
        assert!(!has_tool(&s, t), "ask must be read-only: {t}");
    }
    assert!(has_tool(&s, "Read") && has_tool(&s, "Bash(git diff:*)"));
    assert_eq!(s.permission_mode, "bypassPermissions");
    assert!(s.system_prompt.is_empty());
    let p = &s.append_system_prompt;
    assert!(p.contains("ASK mode"));
    assert!(!p.contains("SIMPLIFY MODE"));
    assert!(!p.contains(COMMENT_HYGIENE_NEEDLE));
    assert!(p.contains("list_skills"));
    for t in [
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
        mcp::LIST_TERMINALS,
        mcp::READ_TERMINAL,
        mcp::RENAME_SESSION,
        mcp::RENAME_WORKSPACE,
        mcp::CREATE_ARTIFACT,
        mcp::LIST_ARTIFACTS,
        mcp::READ_ARTIFACT,
        mcp::PRESENT_ARTIFACT,
    ] {
        assert!(has_tool(&s, t), "ask needs {t}");
    }
    for t in [
        mcp::RENAME_BRANCH,
        mcp::OPEN_PR,
        mcp::CREATE_WORKSPACE,
        mcp::MESSAGE_CHILD,
    ] {
        assert!(!has_tool(&s, t), "ask must not get {t}");
    }
    assert!(p.contains("create_artifact"));
    assert!(!s.mcp_url.is_empty());
    assert_eq!(fx.eng().workspace_chat_selection(&ws.id, "PA").2, "ask");
    let tabs = fx.eng().list_workspace_chats(&ws.id);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].mode, "ask");

    fx.send(&ws.id, "PA", "and the tests?", "sonnet").await;
    assert!(!has_tool(&last(), "Edit"), "the persisted mode applies");

    fx.send_mode(&ws.id, "PA", "now fix it", "sonnet", "agent")
        .await;
    let s = last();
    for t in [
        "Edit",
        "Write",
        mcp::LIST_FINDINGS,
        mcp::UPDATE_FINDING,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
        mcp::LIST_TERMINALS,
        mcp::READ_TERMINAL,
        mcp::RENAME_SESSION,
        mcp::RENAME_WORKSPACE,
        mcp::RENAME_BRANCH,
        mcp::OPEN_PR,
    ] {
        assert!(has_tool(&s, t), "agent needs {t}");
    }
    assert!(!has_tool(&s, mcp::DONE));
    assert!(!s.mcp_url.is_empty());
    assert_eq!(s.permission_mode, "bypassPermissions");
    let p = &s.append_system_prompt;
    assert!(!p.contains("list_findings") && !p.contains("update_finding"));
    for want in [
        "list_skills",
        "AGENT mode",
        "Do not ask the user to switch to Agent",
        "beyond a very simple task",
        "Prefer acting on reasonable defaults",
        "Never use Claude's AskUserQuestion",
        "remounts this turn immediately",
        "Do not use Cursor's built-in SwitchMode",
        "SIMPLIFY MODE",
        COMMENT_HYGIENE_NEEDLE,
    ] {
        assert!(p.contains(want), "agent append missing {want:?}");
    }
    assert_eq!(fx.eng().workspace_chat_selection(&ws.id, "PA").2, "agent");
    fx.te.close().await;
}

#[tokio::test]
async fn mode_model_and_flags_persist_before_the_turn_runs() {
    let seen: Arc<parking_lot::Mutex<Option<(String, Flags, String)>>> = Arc::default();
    let eng_cell: Arc<parking_lot::Mutex<Option<Arc<crate::Engine>>>> = Arc::default();
    let ws_cell: Arc<parking_lot::Mutex<String>> = Arc::default();
    let (s, e, w) = (
        Arc::clone(&seen),
        Arc::clone(&eng_cell),
        Arc::clone(&ws_cell),
    );
    let c = CaptureScript::new(move |call| {
        if let Some(eng) = e.lock().clone() {
            let sel = eng.workspace_chat_selection(&w.lock(), "PA2");
            s.lock().get_or_insert(sel);
        }
        crate::turn::test_support::maestro_fixture(&format!("mid-{call}"), "OK.")
    });
    let fx = Fx::new(Some(c.provider()));
    *eng_cell.lock() = Some(Arc::clone(fx.eng()));
    let ws = fx.workspace("mid-turn mode").await;
    *ws_cell.lock() = ws.id.clone();
    assert_eq!(fx.eng().create_workspace_chat(&ws.id).await.code, "PA2");
    let f = flags(&[("effort", "high")]);
    fx.eng()
        .send_workspace_message(
            &ws.id,
            "PA2",
            "review please",
            "sonnet",
            Some(&f),
            &[],
            "review",
            "",
        )
        .await;
    let during = seen.lock().clone().expect("selection read during the turn");
    assert_eq!(
        during,
        ("sonnet".to_owned(), f.clone(), "review".to_owned())
    );
    assert_eq!(
        fx.eng().workspace_chat_selection(&ws.id, "PA2"),
        ("sonnet".to_owned(), f, "review".to_owned())
    );
    *eng_cell.lock() = None;
    fx.te.close().await;
}

#[tokio::test]
async fn prompt_groups_persist_with_or_without_a_selection() {
    let fx = Fx::new(Some(capture("grp", "OK.").provider()));
    let ws = fx.workspace("prompt group").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    eng.remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, "agent", None)
        .await;
    eng.remember_workspace_chat_prompt_group(&ws.id, "PA", "ops")
        .await;
    assert_eq!(eng.list_workspace_chats(&ws.id)[0].prompt_group, "ops");

    let bare = fx.workspace("prompt group no selection").await;
    eng.list_workspace_chats(&bare.id);
    assert!(fx.te.store.session_for_loop_agent(&bare.id, "PA").is_none());
    eng.remember_workspace_chat_prompt_group(&bare.id, "PA", "ops")
        .await;
    assert_eq!(eng.list_workspace_chats(&bare.id)[0].prompt_group, "ops");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_selection_and_group_share_one_session() {
    let fx = Fx::new(Some(capture("grp-race", "OK.").provider()));
    let ws = fx.workspace("prompt group race").await;
    fx.eng().list_workspace_chats(&ws.id);
    let (e1, e2) = (Arc::clone(fx.eng()), Arc::clone(fx.eng()));
    let (w1, w2) = (ws.id.clone(), ws.id.clone());
    let a = tokio::spawn(async move {
        e1.remember_workspace_chat_selection(&w1, "PA", "sonnet", None, "agent", None)
            .await
    });
    let b = tokio::spawn(async move {
        e2.remember_workspace_chat_prompt_group(&w2, "PA", "ops")
            .await
    });
    a.await.unwrap();
    b.await.unwrap();
    let sessions = fx
        .te
        .store
        .sessions_for_loop(&ws.id)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| AgentConfig::parse(&s.config_json).code == "PA")
        .count();
    assert_eq!(sessions, 1);
    assert_eq!(fx.eng().list_workspace_chats(&ws.id)[0].prompt_group, "ops");
    assert_eq!(fx.eng().workspace_chat_selection(&ws.id, "PA").2, "agent");
    fx.te.close().await;
}

#[tokio::test]
async fn the_dock_selection_persists_without_a_send() {
    let fx = Fx::new(Some(capture("mem", "OK.").provider()));
    let ws = fx.workspace("remember mode").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    let f = flags(&[("effort", "high"), ("thinking", "on")]);
    eng.remember_workspace_chat_selection(&ws.id, "PA", "sonnet", Some(&f), "review", None)
        .await;
    assert_eq!(eng.workspace_chat_selection(&ws.id, "PA").2, "review");
    assert_eq!(eng.list_workspace_chats(&ws.id)[0].mode, "review");
    let (model, got, _) = eng.workspace_chat_selection(&ws.id, "PA");
    assert_eq!((model.as_str(), got), ("sonnet", f));
    eng.remember_workspace_chat_selection(
        &ws.id,
        "PA",
        "sonnet",
        Some(&flags(&[("effort", "low")])),
        "review",
        None,
    )
    .await;
    assert_eq!(
        eng.workspace_chat_selection(&ws.id, "PA").1,
        flags(&[("effort", "low")])
    );
    assert!(eng.workspace_sessions(&ws.id, "PA", "").is_empty());
    eng.remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, "loop", None)
        .await;
    assert_eq!(eng.workspace_chat_selection(&ws.id, "PA").2, "loop");
    eng.remember_workspace_chat_selection(&ws.id, "RV", "opus", None, "ask", None)
        .await;
    assert!(fx.te.store.agent(&ws.id, "RV").is_none());
    assert_eq!(eng.workspace_chat_selection(&ws.id, "PA").2, "ask");
    eng.remember_workspace_chat_selection("no-such-ws", "PA", "sonnet", None, "agent", None)
        .await;
    assert!(fx.te.store.agent("no-such-ws", "PA").is_none());
    let tab2 = eng.create_workspace_chat(&ws.id).await;
    eng.remember_workspace_chat_selection(&ws.id, &tab2.code, "sonnet", None, "plan", None)
        .await;
    eng.close_workspace_chat(&ws.id, &tab2.code).await;
    assert!(fx.te.store.agent(&ws.id, &tab2.code).is_none());
    eng.remember_workspace_chat_selection(&ws.id, &tab2.code, "sonnet", None, "review", None)
        .await;
    assert!(
        fx.te.store.agent(&ws.id, &tab2.code).is_none(),
        "a Remember must not resurrect a closed tab"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn a_human_dock_mode_change_refreshes_standing_notes() {
    let c = capture("dock", "OK.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("dock mode change").await;
    fx.send_mode(&ws.id, "PA", "edit it", "sonnet", CHAT_MODE_AGENT)
        .await;
    fx.eng()
        .remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, CHAT_MODE_REVIEW, None)
        .await;
    fx.send_mode(&ws.id, "PA", "what changed?", "sonnet", CHAT_MODE_REVIEW)
        .await;
    let specs = coder_turns(&c);
    assert!(!specs[0].refresh_standing);
    let second = specs.last().unwrap();
    assert!(second.refresh_standing);
    assert!(second.append_system_prompt.contains("REVIEW mode"));
    fx.te.close().await;
}

#[tokio::test]
async fn a_failed_turn_does_not_stamp_the_standing_mode() {
    let inner = capture("failstand", "OK.");
    let fx = Fx::new(Some(fail_at(&inner, 1)));
    let ws = fx.workspace("failed standing").await;
    fx.send_mode(&ws.id, "PA", "edit it", "sonnet", CHAT_MODE_AGENT)
        .await;
    fx.eng()
        .remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, CHAT_MODE_REVIEW, None)
        .await;
    fx.send_mode(&ws.id, "PA", "what changed?", "sonnet", CHAT_MODE_REVIEW)
        .await;
    let sess = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    assert_eq!(
        AgentConfig::parse(&sess.config_json).standing_mode,
        CHAT_MODE_AGENT
    );
    fx.send_mode(&ws.id, "PA", "retry review", "sonnet", CHAT_MODE_REVIEW)
        .await;
    let specs = coder_turns(&inner);
    assert!(specs.len() >= 3);
    assert!(specs.last().unwrap().refresh_standing);
    fx.te.close().await;
}

#[tokio::test]
async fn a_legacy_ask_session_refreshes_when_it_becomes_agent() {
    let c = capture("legacy", "OK.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("legacy standing").await;
    fx.send_mode(&ws.id, "PA", "explain this", "sonnet", CHAT_MODE_ASK)
        .await;
    let sess = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    let mut cfg = AgentConfig::parse(&sess.config_json);
    cfg.standing_mode.clear();
    fx.te
        .store
        .update_session_config_json(&sess.id, &serde_json::to_string(&cfg).unwrap())
        .unwrap();
    fx.eng()
        .remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, CHAT_MODE_AGENT, None)
        .await;
    fx.send_mode(&ws.id, "PA", "now edit it", "sonnet", CHAT_MODE_AGENT)
        .await;
    assert!(coder_turns(&c).last().unwrap().refresh_standing);
    fx.te.close().await;
}

#[tokio::test]
async fn a_dock_pick_stamps_the_session_provider() {
    let fx = Fx::new(Some(capture("prov", "OK.").provider()));
    let ws = fx.workspace("remember provider").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    eng.remember_workspace_chat_selection(&ws.id, "PA", "cursor/composer-2.5", None, "agent", None)
        .await;
    let provider = || {
        fx.te
            .store
            .session_for_loop_agent(&ws.id, "PA")
            .unwrap()
            .provider
    };
    assert_eq!(provider(), PROVIDER_CURSOR);
    const CURSOR_AGENT: &str = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b";
    eng.set_agent_session(&ws.id, "PA", CURSOR_AGENT);
    eng.remember_workspace_chat_selection(&ws.id, "PA", "", None, "ask", None)
        .await;
    assert_eq!(
        fx.te.store.agent(&ws.id, "PA").unwrap().session_id,
        CURSOR_AGENT
    );
    assert_eq!(provider(), PROVIDER_CURSOR);
    eng.remember_workspace_chat_selection(&ws.id, "PA", "sonnet", None, "agent", None)
        .await;
    assert_eq!(provider(), PROVIDER_CLAUDE_CODE);
    assert_eq!(
        fx.te.store.agent(&ws.id, "PA").unwrap().session_id,
        CURSOR_AGENT,
        "a pick keeps the resume handle (AutoResume needs it)"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn review_mode_files_findings() {
    let c = capture("rev", "Reviewed.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("review me").await;
    fx.send_mode(&ws.id, "PA", "review my diff", "sonnet", "review")
        .await;
    let s = coder_turns(&c).last().cloned().unwrap();
    for t in [
        "Edit",
        "Write",
        "NotebookEdit",
        mcp::RENAME_BRANCH,
        mcp::OPEN_PR,
    ] {
        assert!(!has_tool(&s, t), "review must not get {t}");
    }
    for t in [
        "Bash",
        mcp::REPORT_FINDING,
        mcp::LIST_FINDINGS,
        mcp::UPDATE_FINDING,
    ] {
        assert!(has_tool(&s, t), "review needs {t}");
    }
    assert!(!s.mcp_url.is_empty());
    assert_eq!(s.permission_mode, fx.te.store.settings().permission_mode());
    let p = &s.append_system_prompt;
    assert!(p.contains("REVIEW mode"));
    assert!(!p.contains("SIMPLIFY MODE") && !p.contains(COMMENT_HYGIENE_NEEDLE));
    fx.eng().report_finding(
        &ws.id,
        recur_agent_tools::Finding {
            file: "main.go".into(),
            line: 3,
            severity: Severity::High,
            message: "nil deref".into(),
            ..recur_agent_tools::Finding::default()
        },
    );
    let fs = fx.te.store.findings(&ws.id);
    assert_eq!(fs.len(), 1);
    assert_eq!(
        (fs[0].file.as_str(), fs[0].status.as_str(), fs[0].iteration),
        ("main.go", "open", 1)
    );
    assert!(fx.te.events.has("findings.updated"));
    fx.te.close().await;
}

fn remap_fixture(id: &str, model: &str) -> Fx {
    let fx = Fx::new(None);
    fx.te
        .store
        .add_workspace(Workspace {
            id: id.into(),
            goal: "remap".into(),
            ..Workspace::default()
        })
        .unwrap();
    fx.te.store.upsert_agent(
        id,
        Agent {
            id: format!("{id}-PA"),
            workspace_id: id.into(),
            code: "PA".into(),
            model: model.into(),
            ..Agent::default()
        },
    );
    fx
}

#[tokio::test]
async fn remap_moves_picks_off_a_disabled_provider() {
    let fx = remap_fixture("WS-remap", "cursor/composer-2.5");
    let mut set = fx.te.store.settings();
    set.disabled_providers = vec![PROVIDER_CURSOR.into()];
    set.cursor_api_key = "sk-x".into();
    fx.eng()
        .remap_chats_off_disabled_providers(&set.normalize_providers());
    assert_eq!(
        fx.te.store.agent("WS-remap", "PA").unwrap().model,
        "claude-code/claude-sonnet-5-5"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn remap_keeps_the_simplify_chip() {
    let fx = remap_fixture("WS-remap-simplify", "cursor/composer-2.5");
    let sess = fx
        .eng()
        .ensure_agent_session(
            "WS-remap-simplify",
            AgentRole::Coder.as_str(),
            "PA",
            "Agent",
            "",
        )
        .unwrap();
    fx.te
        .store
        .patch_session_config_merge(&sess.id, r#"{"model":"cursor/composer-2.5"}"#)
        .unwrap();
    fx.te
        .store
        .patch_session_config_simplify(&sess.id, false)
        .unwrap();
    let mut set = fx.te.store.settings();
    set.disabled_providers = vec![PROVIDER_CURSOR.into()];
    set.cursor_api_key = "sk-x".into();
    fx.eng()
        .remap_chats_off_disabled_providers(&set.normalize_providers());
    let got = fx
        .te
        .store
        .session_for_loop_agent("WS-remap-simplify", "PA")
        .unwrap();
    assert!(
        got.config_json.contains(r#""simplify":false"#),
        "{}",
        got.config_json
    );
    assert!(
        got.config_json
            .contains(r#""model":"claude-code/claude-sonnet-5-5""#),
        "{}",
        got.config_json
    );
    fx.te.close().await;
}

#[tokio::test]
async fn remap_uses_the_settings_fallback_with_claude_off() {
    let fx = remap_fixture("WS-remap-claude-off", "claude-code/sonnet");
    let mut set = fx.te.store.settings();
    set.disabled_providers = vec![PROVIDER_CLAUDE_CODE.into()];
    set.cursor_api_key = "sk-x".into();
    set.model_agent = "cursor/composer-2.5".into();
    fx.eng().remap_chats_off_disabled_providers(&set);
    assert_eq!(
        fx.te
            .store
            .agent("WS-remap-claude-off", "PA")
            .unwrap()
            .model,
        "cursor/composer-2.5"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn remap_leaves_an_empty_pick() {
    let fx = remap_fixture("WS-remap-empty", "");
    let mut set = fx.te.store.settings();
    set.disabled_providers = vec![PROVIDER_CLAUDE_CODE.into()];
    set.cursor_api_key = "sk-x".into();
    set.model_agent = "cursor/composer-2.5".into();
    fx.eng().remap_chats_off_disabled_providers(&set);
    assert_eq!(fx.te.store.agent("WS-remap-empty", "PA").unwrap().model, "");
    fx.te.close().await;
}

#[tokio::test]
async fn remap_off_a_removed_compatible_provider() {
    let fx = remap_fixture("WS-remap-removed", "alpha/m1");
    let mut set = fx.te.store.settings();
    set.disabled_providers = vec![PROVIDER_CLAUDE_CODE.into()];
    set.compatible_providers = vec![CompatibleProvider {
        id: "beta".into(),
        base_url: "https://b.example".into(),
        models: vec![CompatibleModel {
            id: "m2".into(),
            ..CompatibleModel::default()
        }],
        ..CompatibleProvider::default()
    }];
    set.model_agent = "beta/m2".into();
    fx.eng().remap_chats_off_disabled_providers(&set);
    assert_eq!(
        fx.te.store.agent("WS-remap-removed", "PA").unwrap().model,
        "beta/m2"
    );
    fx.te.close().await;
}

fn legacy_session(fx: &Fx, ws: &str, user: &str) -> Session {
    let sess = fx
        .te
        .store
        .create_session(Session {
            workspace_id: ws.into(),
            kind: "coder".into(),
            provider: PROVIDER_CLAUDE_CODE.into(),
            config_json: r#"{"code":"PA","name":"Agent"}"#.into(),
            ..Session::default()
        })
        .unwrap();
    fx.te
        .store
        .append_message(
            &sess.id,
            Message {
                role: "user".into(),
                content: Some(vec![ContentBlock::text(user)]),
                ..Message::default()
            },
        )
        .unwrap();
    sess
}

#[tokio::test]
async fn the_first_send_stores_the_tab_title() {
    let fx = Fx::new(Some(capture_same("title-sess", "ok").provider()));
    let ws = fx.workspace("title-tab").await;
    fx.send(&ws.id, "PA", "add a dark-mode toggle", "sonnet")
        .await;
    let sess = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    assert!(
        sess.config_json
            .contains(r#""title":"add a dark-mode toggle""#)
    );
    assert_eq!(
        fx.eng().list_workspace_chats(&ws.id)[0].title,
        "add a dark-mode toggle"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn untitled_legacy_tabs_read_the_first_prompt_and_keep_it() {
    let fx = Fx::new(Some(capture_same("legacy-follow", "ok").provider()));
    let ws = fx.workspace("legacy-title").await;
    fx.eng().list_workspace_chats(&ws.id);
    let sess = legacy_session(&fx, &ws.id, "review the flaky suite");
    fx.te
        .store
        .append_message(
            &sess.id,
            Message {
                role: "user".into(),
                content: Some(vec![ContentBlock::tool_result(
                    "tu-1",
                    &"diff ".repeat(4000),
                    false,
                )]),
                ..Message::default()
            },
        )
        .unwrap();
    let tabs = fx.eng().list_workspace_chats(&ws.id);
    assert_eq!(tabs[0].title, "review the flaky suite");

    let ws2 = fx.workspace("legacy-follow").await;
    fx.eng().list_workspace_chats(&ws2.id);
    legacy_session(&fx, &ws2.id, "review the flaky suite");
    fx.send(&ws2.id, "PA", "and also fix the flake", "sonnet")
        .await;
    let sess = fx.te.store.session_for_loop_agent(&ws2.id, "PA").unwrap();
    assert!(!sess.config_json.contains("and also fix the flake"));
    assert_eq!(
        fx.eng().list_workspace_chats(&ws2.id)[0].title,
        "review the flaky suite"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn stored_and_derived_titles_cap_alike() {
    let fx = Fx::new(Some(capture_same("cjk-title", "ok").provider()));
    let ws = fx.workspace("cjk-title").await;
    let first = "漢".repeat(20);
    let want = truncate(&first, crate::attention::chat_inbox::CHAT_TAB_TITLE_LIMIT);
    fx.send(&ws.id, "PA", &first, "sonnet").await;
    assert_eq!(fx.eng().list_workspace_chats(&ws.id)[0].title, want);
    let ws2 = fx.workspace("cjk-derived").await;
    fx.eng().list_workspace_chats(&ws2.id);
    legacy_session(&fx, &ws2.id, &first);
    assert_eq!(fx.eng().list_workspace_chats(&ws2.id)[0].title, want);
    fx.te.close().await;
}

#[tokio::test]
async fn workspace_sessions_skip_pair_and_maestro() {
    let fx = Fx::new(Some(capture_same("pair-sess", "done").provider()));
    let ws = fx.workspace("sessions-skip").await;
    fx.send(&ws.id, "PA", "do the pair work", "sonnet").await;
    for (kind, cfg, role, text) in [
        (
            "maestro",
            r#"{"code":"MA","name":"Maestro"}"#,
            "user",
            "steer the loop",
        ),
        (
            "reviewer",
            r#"{"code":"RV","name":"Reviewer"}"#,
            "assistant",
            "looks good",
        ),
    ] {
        let s = fx
            .te
            .store
            .create_session(Session {
                workspace_id: ws.id.clone(),
                kind: kind.into(),
                provider: PROVIDER_CLAUDE_CODE.into(),
                config_json: cfg.into(),
                ..Session::default()
            })
            .unwrap();
        fx.te
            .store
            .append_message(
                &s.id,
                Message {
                    role: role.into(),
                    content: Some(vec![ContentBlock::text(text)]),
                    ..Message::default()
                },
            )
            .unwrap();
    }
    let views = fx.eng().workspace_sessions(&ws.id, "MA", "");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].code, "RV");
    fx.te.close().await;
}
