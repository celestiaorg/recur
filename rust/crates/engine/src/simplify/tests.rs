//! Go `simplify_test.go` (the send and loop-start cases run through E3/E4;
//! the session flag rules they rest on are tested here).

use recur_agent_tools::{Host, PlanSubmission};

use super::*;
use crate::host::EngineHost;
use crate::testing::TestEngine;
use crate::turn::test_support::add_solo_workspace;

fn engine() -> TestEngine {
    let te = TestEngine::new(None);
    add_solo_workspace(&te.engine, "W1");
    te
}

/// A pair tab with its session and an explicit chip (what the dock's
/// Remember writes).
fn pair_with_chip(te: &TestEngine, code: &str, on: bool) {
    te.engine.ensure_pair_agent("W1", code, "");
    let sess = te
        .engine
        .ensure_agent_session("W1", "coder", code, "Agent", "")
        .unwrap();
    te.store
        .patch_session_config_simplify(&sess.id, on)
        .unwrap();
}

#[tokio::test]
async fn chat_append_follows_mode_session_and_settings() {
    let te = engine();
    assert!(
        te.engine
            .simplify_chat_append("W1", "PA", "agent")
            .starts_with("SIMPLIFY MODE")
    );
    assert!(
        te.engine
            .simplify_chat_append("W1", "PA", "plan")
            .starts_with("SIMPLIFY MODE")
    );
    assert_eq!(te.engine.simplify_chat_append("W1", "PA", "ask"), "");
    assert_eq!(te.engine.simplify_chat_append("W1", "PA", "review"), "");
    let mut set = te.store.settings();
    set.simplify = Some(false);
    te.store.set_settings(set);
    assert_eq!(
        te.engine.simplify_chat_append("W1", "PA", "agent"),
        "",
        "settings off"
    );
    pair_with_chip(&te, "PA", true);
    assert!(
        te.engine
            .simplify_chat_append("W1", "PA", "agent")
            .starts_with("SIMPLIFY MODE"),
        "session on beats settings off"
    );
    pair_with_chip(&te, "PA2", false);
    let mut set = te.store.settings();
    set.simplify = Some(true);
    te.store.set_settings(set);
    assert_eq!(
        te.engine.simplify_chat_append("W1", "PA2", "agent"),
        "",
        "session off beats settings on; tabs independent"
    );
    te.close().await;
}

#[tokio::test]
async fn coder_follows_maestro_session() {
    let te = engine();
    assert!(
        te.engine
            .simplify_coder_append("W1")
            .starts_with("SIMPLIFY MODE")
    );
    te.engine.stamp_maestro_simplify_if_unset("W1");
    te.engine
        .remember_workspace_chat_simplify("W1", "MA", false);
    assert_eq!(te.engine.simplify_coder_append("W1"), "");
    te.engine.remember_workspace_chat_simplify("W1", "MA", true);
    assert!(
        te.engine
            .simplify_coder_append("W1")
            .starts_with("SIMPLIFY MODE")
    );
    te.close().await;
}

#[tokio::test]
async fn stamp_if_unset_freezes_and_never_overwrites() {
    let te = engine();
    assert_eq!(te.engine.chat_simplify("W1", "MA"), None);
    te.engine.stamp_maestro_simplify_if_unset("W1");
    assert_eq!(te.engine.chat_simplify("W1", "MA"), Some(true));
    te.engine
        .remember_workspace_chat_simplify("W1", "MA", false);
    te.engine.stamp_maestro_simplify_if_unset("W1");
    assert_eq!(te.engine.chat_simplify("W1", "MA"), Some(false));
    te.close().await;
}

#[tokio::test]
async fn remember_never_creates_sessions_or_tabs() {
    let te = engine();
    te.engine.ensure_pair_agent("W1", "PA", "");
    te.engine
        .remember_workspace_chat_simplify("W1", "PA", false);
    assert!(te.store.session_for_loop_agent("W1", "PA").is_none());
    te.engine
        .remember_workspace_chat_simplify("W1", "MA", false);
    assert!(te.store.session_for_loop_agent("W1", "MA").is_none());
    // A closed tab (agent row gone) is not resurrected.
    pair_with_chip(&te, "PA2", true);
    te.store.delete_agent("W1", "PA2");
    te.engine
        .remember_workspace_chat_simplify("W1", "PA2", false);
    assert!(te.store.agent("W1", "PA2").is_none());
    te.engine.remember_chat_simplify("W1", "PA2", false);
    assert!(te.store.agent("W1", "PA2").is_none());
    assert_eq!(te.engine.chat_simplify("W1", "PA2"), Some(true));
    te.engine
        .remember_workspace_chat_simplify("no-such-ws", "PA", true);
    assert!(te.store.agent("no-such-ws", "PA").is_none());
    te.close().await;
}

#[tokio::test]
async fn inherit_follows_plan_author_then_newest_choice() {
    let te = engine();
    assert!(te.engine.inherit_simplify("W1"), "settings default");
    pair_with_chip(&te, "PA", false);
    pair_with_chip(&te, "PA2", true);
    assert!(
        te.engine.inherit_simplify("W1"),
        "newest chooser wins without a plan"
    );
    EngineHost::new(std::sync::Arc::downgrade(&te.engine))
        .submit_plan(
            "W1",
            "PA",
            PlanSubmission {
                summary: "ship it".into(),
                steps: vec!["one".into()],
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        te.store.workspace("W1").unwrap().plan.unwrap().chat_code,
        "PA"
    );
    assert!(
        !te.engine.inherit_simplify("W1"),
        "the plan author's chip, not the newest tab"
    );
    te.engine.stamp_maestro_simplify_if_unset("W1");
    assert_eq!(te.engine.chat_simplify("W1", "MA"), Some(false));
    te.close().await;
}
