use std::time::Duration;

use recur_agent_tools::{AskUserRequest, EscalationOption as ToolOption};
use recur_domain::{UserQuestion, UserQuestionAnswer, Workspace};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::testing::{TestEngine, add_project, init_repo};

fn add_solo(te: &TestEngine, id: &str) {
    te.store
        .add_workspace(Workspace {
            id: id.into(),
            ..Default::default()
        })
        .unwrap();
}

fn q(ws: &str, question: &str) -> UserQuestion {
    UserQuestion {
        workspace_id: ws.into(),
        agent_code: "PA".into(),
        question: question.into(),
        ..UserQuestion::default()
    }
}

#[tokio::test]
async fn parks_without_blocking() {
    let te = TestEngine::new(None);
    let msg = tokio::time::timeout(
        Duration::from_millis(500),
        std::future::ready(te.engine.ask_user(
            "W1",
            "PA",
            AskUserRequest {
                question: "Which color?".into(),
                options: vec![ToolOption {
                    text: "Blue".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )),
    )
    .await
    .expect("park returns immediately");
    assert!(msg.contains("Question delivered"), "{msg}");
    let pending = te.engine.pending_ask_users();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].question, "Which color?");
    assert_eq!(pending[0].agent_code, "PA");
    assert_eq!(pending[0].asked_ago, "now");
    assert_eq!(pending[0].options[0].text, "Blue");
    let inbox = te.store.inbox().unwrap_or_default();
    let row = inbox
        .iter()
        .find(|i| i.id == format!("ib-ask-{}", pending[0].id))
        .expect("inbox row");
    assert_eq!(row.title, "<b>PA</b> asked a question");
    assert_eq!(row.sub, "W1 · Which color?");
    assert!(te.events.has("question.asked"));
    te.close().await;
}

#[tokio::test]
async fn ask_user_parks_the_asking_turn() {
    let te = TestEngine::new(None);
    add_solo(&te, "W1");
    let tok = CancellationToken::new();
    te.engine
        .register_chat_turn("W1", "PA", tok.clone())
        .unwrap();
    te.engine.ask_user("W1", "PA", AskUserRequest::default());
    tokio::time::timeout(Duration::from_secs(2), tok.cancelled())
        .await
        .expect("asking turn parked");
    assert!(te.engine.chat_turn_was_parked("W1", "PA"));
    te.close().await;
}

#[tokio::test]
async fn survives_unregister_mcp() {
    let te = TestEngine::new(None);
    te.engine
        .ask_user_broker()
        .park(q("W1", "still here?"), None);
    let (token, _) = te
        .engine
        .register_mcp("W1", "PA", "PA", &[], "sonnet")
        .await
        .unwrap();
    te.engine.unregister_mcp(&token);
    assert_eq!(te.engine.pending_ask_users().len(), 1);
    te.close().await;
}

#[tokio::test]
async fn answer_removes_pending() {
    let te = TestEngine::new(None);
    let parked = te.engine.ask_user_broker().park(q("W1", "color?"), None);
    assert!(
        te.engine
            .answer_ask_user(&parked.id, "Blue", "", Vec::new())
    );
    assert!(te.engine.pending_ask_users().is_empty());
    assert!(
        !te.engine
            .answer_ask_user(&parked.id, "Blue", "", Vec::new())
    );
    let answered = te.events.by_name("question.answered");
    assert_eq!(
        answered[0].payload_json(),
        format!(r#"{{"answer":{{"choice":"Blue"}},"id":"{}"}}"#, parked.id)
    );
    te.close().await;
}

#[test]
fn resume_prompt() {
    let got = format_ask_user_resume_prompt(
        &UserQuestion {
            question: "Which color?".into(),
            context: "pick one".into(),
            ..Default::default()
        },
        &UserQuestionAnswer {
            choice: "Blue".into(),
            ..Default::default()
        },
    );
    assert_eq!(
        got,
        "The user answered your earlier ask_user question.\n\nQuestion: Which color?\nContext: pick one\nAnswer: Blue\n\nContinue with this answer. Do not re-ask the same question unless you still need clarification."
    );
    let both = format_ask_user_resume_prompt(
        &UserQuestion::default(),
        &UserQuestionAnswer {
            choice: "A".into(),
            free_text: "because".into(),
        },
    );
    assert!(both.contains("Answer: A\nDetails: because"));
    assert!(
        format_ask_user_resume_prompt(&UserQuestion::default(), &UserQuestionAnswer::default())
            .contains("Answer: (empty)")
    );
    assert_eq!(
        format_ask_user_display(&UserQuestionAnswer {
            choice: "A".into(),
            free_text: "b".into()
        }),
        "A — b"
    );
    assert_eq!(
        format_ask_user_display(&UserQuestionAnswer::default()),
        "(empty)"
    );
}

#[tokio::test]
async fn park_if_skips_newer_turn() {
    let te = TestEngine::new(None);
    add_solo(&te, "W1");
    let old_tok = CancellationToken::new();
    let old = te
        .engine
        .register_chat_turn("W1", "PA", old_tok.clone())
        .unwrap();
    let new_tok = CancellationToken::new();
    let newer = te
        .engine
        .register_chat_turn("W1", "PA", new_tok.clone())
        .unwrap();
    // Registering the newer turn cancelled the old one (swap); answering a
    // parked ask that captured `old` must not touch `newer`.
    assert!(!te.engine.park_chat_turn_if("W1", "PA", Some(&old)));
    assert!(!new_tok.is_cancelled());
    assert!(te.engine.park_chat_turn_if("W1", "PA", Some(&newer)));
    assert!(new_tok.is_cancelled());
    te.close().await;
}

#[tokio::test]
async fn clear_for_workspace() {
    let te = TestEngine::new(None);
    te.engine.ask_user_broker().park(q("W1", "gone?"), None);
    te.engine.ask_user_broker().park(q("W2", "keep?"), None);
    te.engine.clear_ask_users_for_workspace("W1");
    let pending = te.engine.pending_ask_users();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].workspace_id, "W2");
    assert!(
        te.events
            .by_name("question.answered")
            .iter()
            .any(|e| e.payload_json().contains(r#""answer":{}"#))
    );
    te.close().await;
}

#[tokio::test]
async fn resume_clears_reservation() {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te
        .engine
        .create_workspace(&proj, "ask-resume-clear", "")
        .await
        .unwrap();
    let parked = te.engine.ask_user_broker().park(q(&ws.id, "color?"), None);
    let ans = UserQuestionAnswer {
        choice: "Blue".into(),
        ..Default::default()
    };
    let (got, _) = te
        .engine
        .ask_user_broker()
        .answer(&parked.id, &ans)
        .unwrap();
    assert!(te.engine.ask_user_workspace_busy(&ws.id));
    assert!(te.engine.ask_user_pending_for(&ws.id, "PA"));
    te.engine.resume_after_ask_user(got, ans, Vec::new()).await;
    assert!(!te.engine.ask_user_workspace_busy(&ws.id));
    te.close().await;
}
