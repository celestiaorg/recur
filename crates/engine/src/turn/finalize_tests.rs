//! Chat error tests plus the ask_user finalize cases. The
//! SendWorkspaceMessage-driven footer cases run the same turns through
//! `run_agent` + `finalize_chat_reply` here; `chat::tests` re-runs them through
//! the send path.

use std::error::Error;
use std::sync::Arc;

use recur_agent::error::CODEX_LOGIN_EXPIRED;
use recur_agent::testing::ScriptedProvider;
use recur_agent::{ProviderError, TurnRequest, TurnResult};
use recur_domain::{AgentRole, ContentBlock, Message, PROVIDER_CLAUDE_CODE};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::codes::{CHAT_RETRY_DISPLAY_SENTINEL, PAIR_AGENT_CODE};
use crate::testing::{TestEngine, add_project, init_repo};
use crate::turn::AgentSpec;
use crate::turn::checkpoint::turn_user_message_id;
use crate::turn::test_support::{claude, duration_error, result_then_error};
use crate::turn::transcript::{content_items, user_message_is_chat_prompt};

fn err(msg: &str) -> ProviderError {
    ProviderError::failed(msg)
}

fn fmt(e: Option<&ProviderError>, res: &TurnResult) -> String {
    format_chat_agent_error(e.map(|e| e as &(dyn Error + 'static)), res)
}

#[test]
fn agent_error_includes_detail() {
    assert_eq!(
        fmt(
            Some(&err("cursor run: invalid_argument: bad image")),
            &TurnResult::default()
        ),
        "Agent error: cursor run: invalid_argument: bad image"
    );
    assert_eq!(
        fmt(None, &TurnResult::default()),
        "I hit an error trying to respond just now. Try again."
    );
}

#[test]
fn agent_error_deadline_exceeded() {
    assert_eq!(
        fmt(Some(&err(DEADLINE_EXCEEDED)), &TurnResult::default()),
        CHAT_TURN_TIMED_OUT_REPLY
    );
    assert_eq!(
        fmt(
            Some(&err(&format!(
                "generate commit message: {DEADLINE_EXCEEDED}"
            ))),
            &TurnResult::default()
        ),
        CHAT_TURN_TIMED_OUT_REPLY
    );
}

#[test]
fn agent_error_prefers_provider_stderr() {
    let res = TurnResult {
        stderr: "You've hit your org's monthly spend limit".into(),
        ..Default::default()
    };
    assert_eq!(
        fmt(Some(&err("cursor run: boom")), &res),
        "Agent error: You've hit your org's monthly spend limit"
    );
}

#[test]
fn agent_error_codex_login_expired() {
    let stderr = "\x1b[2m2026-09-10T15:43:08.630243Z\x1b[0m \x1b[31mERROR\x1b[0m \x1b[2mcodex_login::auth::manager\x1b[0m \x1b[2m-\x1b[0m Failed to refresh token: 401 Unauthorized {\n  \"error\": \"invalid_grant\"\n}";
    let want = format!("Agent error: {CODEX_LOGIN_EXPIRED}");
    let res = TurnResult {
        stderr: stderr.into(),
        ..Default::default()
    };
    assert_eq!(fmt(Some(&err("codex app-server exited")), &res), want);
    assert_eq!(
        fmt(
            Some(&err(&format!("codex thread: boom\n{stderr}"))),
            &TurnResult::default()
        ),
        want
    );
    let res = TurnResult {
        stderr: "ERROR codex_login::auth::manager - 401 Unauthorized".into(),
        ..Default::default()
    };
    assert_eq!(fmt(Some(&err("codex app-server exited")), &res), want);
}

#[test]
fn agent_error_tidies_tracing_dump() {
    let res = TurnResult {
        stderr: "\x1b[31mERROR\x1b[0m workspace::tools - workspace tool failed to start {\n  \"code\": 1\n}".into(),
        ..Default::default()
    };
    assert_eq!(
        fmt(Some(&err("codex run: boom")), &res),
        "Agent error: workspace tool failed to start"
    );
}

#[test]
fn agent_error_keeps_provider_json() {
    let stderr = r#"API Error: 400 {"error":{"message":"model does not support images"}}"#;
    let res = TurnResult {
        stderr: stderr.into(),
        ..Default::default()
    };
    assert_eq!(
        fmt(Some(&err("codex run: boom")), &res),
        format!("Agent error: {stderr}")
    );
}

#[test]
fn agent_error_prefers_runner_err_over_assistant_text() {
    let res = TurnResult {
        is_error: true,
        text: "I'll check the failure output and load any relevant Recur skills.".into(),
        ..Default::default()
    };
    assert_eq!(
        fmt(
            Some(&err(
                "cursor run ended without a result: bridge crashed spawning a shell"
            )),
            &res
        ),
        "Agent error: cursor run ended without a result: bridge crashed spawning a shell"
    );
}

#[test]
fn agent_error_hides_cursor_bridge_startup_dump() {
    let got = fmt(
        Some(&err(
            "cursor run ended without a result: the Cursor bridge stopped mid-turn; retry (bridge log: /tmp/bridge.log)",
        )),
        &TurnResult::default(),
    );
    assert!(!got.contains("authTokenFile") && !got.contains("starting cursor-sdk-bridge"));
    assert!(got.starts_with(CHAT_AGENT_ERROR_PREFIX) && got.contains("stopped mid-turn"));
}

struct Fixture {
    te: TestEngine,
    ws: String,
    _repo: crate::testing::TempRepo,
}

async fn fixture(p: Option<Arc<dyn recur_agent::Provider>>) -> Fixture {
    let te = TestEngine::new(p.or_else(|| Some(claude())));
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te
        .engine
        .create_workspace(&proj, "scratch", "")
        .await
        .unwrap()
        .id;
    Fixture {
        te,
        ws,
        _repo: repo,
    }
}

impl Fixture {
    fn items(&self) -> Vec<recur_domain::ChatItem> {
        let te = &self.te;
        let sess = te
            .store
            .session_for_loop_agent(&self.ws, PAIR_AGENT_CODE)
            .expect("session");
        let msgs = te
            .store
            .session_transcript(&sess.id)
            .unwrap()
            .unwrap_or_default();
        crate::turn::transcript::flatten_transcript(
            &te.engine.providers,
            &msgs,
            "",
            &Default::default(),
        )
    }

    fn cursor_session(&self) -> recur_domain::Session {
        self.te
            .engine
            .ensure_pair_agent(&self.ws, PAIR_AGENT_CODE, "cursor/composer-2.5");
        self.te
            .engine
            .ensure_agent_session(&self.ws, "coder", PAIR_AGENT_CODE, "Agent", "cursor")
            .unwrap()
    }

    fn finalize(
        &self,
        prompt: &str,
        display: &str,
        streamed: &str,
        res: &TurnResult,
        e: Option<&ProviderError>,
    ) -> String {
        self.te.engine.finalize_chat_reply(ChatOutcome {
            workspace_id: &self.ws,
            code: PAIR_AGENT_CODE,
            prompt,
            display,
            streamed,
            res,
            err: e.map(|e| e as &(dyn Error + 'static)),
            empty_fallback: "Done.",
            started: None,
            skip_user: false,
            user_msg_id: "",
        })
    }
}

fn users(items: &[recur_domain::ChatItem]) -> usize {
    items
        .iter()
        .filter(|i| i.role == "user" && i.kind == "text")
        .count()
}

#[tokio::test]
async fn finalize_persists_synthesized_error() {
    let f = fixture(None).await;
    f.cursor_session();
    let out = f.finalize(
        "hi",
        "",
        "",
        &TurnResult::default(),
        Some(&err("cursor run: boom")),
    );
    assert_eq!(out, "Agent error: cursor run: boom");
    let items = content_items(&f.items());
    assert_eq!(items.last().unwrap().text, out);
    f.te.close().await;
}

#[tokio::test]
async fn finalize_persists_error_after_partial_stream() {
    let f = fixture(None).await;
    let sess = f.cursor_session();
    for (role, t) in [
        ("user", "write a helper"),
        ("assistant", "Sure — I'll start by"),
    ] {
        f.te.store
            .append_message(
                &sess.id,
                Message {
                    role: role.into(),
                    content: Some(vec![ContentBlock::text(t)]),
                    ..Message::default()
                },
            )
            .unwrap();
    }
    let res = TurnResult {
        is_error: true,
        stderr: "rate limit exceeded".into(),
        ..Default::default()
    };
    let out = f.finalize("write a helper", "", "Sure — I'll start by", &res, None);
    assert_eq!(out, "Agent error: rate limit exceeded");
    let items = content_items(&f.items());
    assert!(items.len() >= 3);
    assert_eq!(items.last().unwrap().text, out);
    assert_eq!(users(&items), 1, "the mirrored prompt is not duplicated");
    f.te.close().await;
}

#[tokio::test]
async fn finalize_persists_pre_mirror_failure() {
    let f = fixture(None).await;
    f.te.engine
        .ensure_pair_agent(&f.ws, PAIR_AGENT_CODE, "cursor/composer-2.5");
    let out = f.finalize(
        "paste this image",
        "",
        "",
        &TurnResult::default(),
        Some(&err("cursor: API key not configured")),
    );
    assert_eq!(out, "Agent error: cursor: API key not configured");
    let items = content_items(&f.items());
    assert_eq!(items.len(), 2);
    assert_eq!(
        (items[0].role.as_str(), items[0].text.as_str()),
        ("user", "paste this image")
    );
    assert_eq!(items[1].text, out);
    // The pre-mirror session took the tab's own provider.
    let sess =
        f.te.store
            .session_for_loop_agent(&f.ws, PAIR_AGENT_CODE)
            .unwrap();
    assert_eq!(sess.provider, "cursor");
    f.te.close().await;
}

#[tokio::test]
async fn retry_failure_does_not_duplicate_user() {
    let f = fixture(None).await;
    let sess = f.cursor_session();
    f.te.store
        .append_message(
            &sess.id,
            Message {
                role: "user".into(),
                content: Some(vec![ContentBlock::text("do the thing")]),
                ..Message::default()
            },
        )
        .unwrap();
    f.te.engine.persist_chat_failure(
        &f.ws,
        PAIR_AGENT_CODE,
        "do the thing",
        CHAT_RETRY_DISPLAY_SENTINEL,
        "Agent error: You've hit your individual spend limit",
        0,
        &TurnResult::default(),
        false,
        "",
    );
    let items = content_items(&f.items());
    assert_eq!(users(&items), 1);
    assert_eq!(
        items.last().unwrap().text,
        "Agent error: You've hit your individual spend limit"
    );
    f.te.close().await;
}

#[tokio::test]
async fn wrapped_display_failure_does_not_duplicate() {
    let f = fixture(None).await;
    let sess = f.cursor_session();
    let composer = "/go-code-review look at auth";
    let wire = format!("<skill_content name=\"go-code-review\">body</skill_content>\n{composer}");
    f.te.store
        .append_message(
            &sess.id,
            Message {
                role: "user".into(),
                content: Some(vec![ContentBlock::text_display(&wire, composer)]),
                ..Message::default()
            },
        )
        .unwrap();
    f.te.engine.persist_chat_failure(
        &f.ws,
        PAIR_AGENT_CODE,
        &wire,
        composer,
        "Agent error: boom",
        0,
        &TurnResult::default(),
        false,
        "",
    );
    let items = content_items(&f.items());
    assert_eq!(users(&items), 1);
    assert!(
        items
            .iter()
            .filter(|i| i.role == "user")
            .all(|i| i.text == composer)
    );
    f.te.close().await;
}

#[tokio::test]
async fn resume_failure_records_new_turn() {
    let f = fixture(None).await;
    f.te.engine
        .ensure_pair_agent(&f.ws, PAIR_AGENT_CODE, "sonnet");
    let sess =
        f.te.engine
            .ensure_agent_session(
                &f.ws,
                "coder",
                PAIR_AGENT_CODE,
                "Agent",
                PROVIDER_CLAUDE_CODE,
            )
            .unwrap();
    f.te.store
        .append_message(
            &sess.id,
            Message {
                role: "user".into(),
                content: Some(vec![ContentBlock::text("previous")]),
                ..Message::default()
            },
        )
        .unwrap();
    let want = "11111111-1111-1111-1111-111111111111";
    f.te.engine.persist_chat_failure(
        &f.ws,
        PAIR_AGENT_CODE,
        CHAT_RESUME_PROMPT,
        CHAT_RESUME_DISPLAY,
        "Agent error: boom",
        1500,
        &TurnResult::default(),
        false,
        want,
    );
    let items = f.items();
    assert!(
        items
            .iter()
            .any(|i| i.role == "user" && i.kind == "text" && i.text == CHAT_RESUME_DISPLAY)
    );
    let footer = items
        .iter()
        .rfind(|i| i.kind == "turn")
        .unwrap()
        .message_id
        .clone();
    let msgs = f.te.store.session_transcript(&sess.id).unwrap().unwrap();
    assert_eq!(turn_user_message_id(&msgs, &footer), want);
    let last_user = msgs
        .iter()
        .rfind(|m| m.role == "user" && user_message_is_chat_prompt(m))
        .unwrap();
    assert_eq!(last_user.id, want);

    let want2 = "22222222-2222-2222-2222-222222222222";
    f.te.engine.persist_chat_failure(
        &f.ws,
        PAIR_AGENT_CODE,
        CHAT_RESUME_PROMPT,
        CHAT_RESUME_DISPLAY,
        "Agent error: boom again",
        800,
        &TurnResult::default(),
        false,
        want2,
    );
    let msgs = f.te.store.session_transcript(&sess.id).unwrap().unwrap();
    let last_user = msgs
        .iter()
        .rfind(|m| m.role == "user" && user_message_is_chat_prompt(m))
        .unwrap();
    assert_eq!(
        last_user.id, want2,
        "same resume text still inserts the new key"
    );
    let last_footer = msgs.iter().rfind(|m| m.duration_ms > 0).unwrap();
    assert_eq!(turn_user_message_id(&msgs, &last_footer.id), want2);
    f.te.close().await;
}

/// A chat turn on PA through the router, finalized like the send path.
async fn chat_turn(f: &Fixture, prompt: &str) -> String {
    let tok = CancellationToken::new();
    let out =
        f.te.engine
            .run_agent(
                &tok,
                AgentSpec::new(TurnRequest {
                    workspace_id: f.ws.clone(),
                    agent_id: format!("{}-PA", f.ws),
                    role: AgentRole::Coder,
                    model: "sonnet".into(),
                    prompt: prompt.into(),
                    ..TurnRequest::default()
                }),
            )
            .await;
    let (res, e) = match out {
        Ok(r) => (r, None),
        Err(e) => (*e.partial.clone(), Some(e)),
    };
    f.finalize(prompt, "", "", &res, e.as_ref())
}

#[tokio::test]
async fn failed_chat_error_note_owns_footer() {
    let f = fixture(Some(duration_error())).await;
    let out = chat_turn(&f, "long task").await;
    assert!(out.starts_with(CHAT_AGENT_ERROR_PREFIX), "{out}");
    let items = f.items();
    let footers: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| i.kind == "turn")
        .map(|(n, _)| n)
        .collect();
    assert_eq!(footers.len(), 1);
    let footer = &items[footers[0]];
    assert_eq!(footer.duration_ms, 1500);
    assert_eq!(
        (footer.input_tokens, footer.cache_read_tokens),
        (200, 15000)
    );
    let error_at = items
        .iter()
        .rposition(|i| i.kind == "text" && i.text.starts_with(CHAT_AGENT_ERROR_PREFIX))
        .unwrap();
    assert_eq!(
        footers[0],
        error_at + 1,
        "footer right after the error note"
    );
    let sess =
        f.te.store
            .session_for_loop_agent(&f.ws, PAIR_AGENT_CODE)
            .unwrap();
    let msgs = f.te.store.session_transcript(&sess.id).unwrap().unwrap();
    assert!(
        msgs.iter()
            .any(|m| m.input_tokens == 200 && m.duration_ms == 0),
        "the result's usage is mirrored without a duration"
    );
    f.te.close().await;
}

#[tokio::test]
async fn failed_chat_result_does_not_duplicate_footer() {
    let f = fixture(Some(result_then_error())).await;
    let out = chat_turn(&f, "long task").await;
    assert!(out.starts_with(CHAT_AGENT_ERROR_PREFIX));
    assert_eq!(f.items().iter().filter(|i| i.kind == "turn").count(), 1);
    f.te.close().await;
}

#[tokio::test]
async fn stop_note_gets_its_own_footer() {
    let f = fixture(Some(Arc::new(
        ScriptedProvider::new(Vec::new()).imitating(claude()),
    )))
    .await;
    let res = TurnResult {
        duration_ms: 400,
        ..Default::default()
    };
    let e = ProviderError::canceled().with_partial(res.clone());
    f.te.engine
        .ensure_pair_agent(&f.ws, PAIR_AGENT_CODE, "sonnet");
    let out = f.finalize("long task", "", "", &res, Some(&e));
    assert_eq!(out, CHAT_STOPPED_REPLY);
    let items = f.items();
    let stop_at = items
        .iter()
        .position(|i| i.text == CHAT_STOPPED_REPLY)
        .unwrap();
    assert_eq!(items[stop_at + 1].kind, "turn");
    assert_eq!(items[stop_at + 1].duration_ms, 400);
    f.te.close().await;
}

#[tokio::test]
async fn parked_turn_finalizes_quietly() {
    let te = TestEngine::new(None);
    crate::turn::test_support::add_solo_workspace(&te.engine, "W1");
    let tok = CancellationToken::new();
    te.engine
        .register_chat_turn("W1", "PA", tok.clone())
        .unwrap();
    assert!(te.engine.park_chat_turn("W1", "PA"));
    assert!(tok.is_cancelled());
    assert!(te.engine.chat_turn_was_parked("W1", "PA"));
    let canceled = ProviderError::canceled();
    let out = te.engine.finalize_chat_reply(ChatOutcome {
        workspace_id: "W1",
        code: "PA",
        prompt: "hi",
        display: "",
        streamed: "partial",
        res: &TurnResult::default(),
        err: Some(&canceled),
        empty_fallback: "Done.",
        started: None,
        skip_user: false,
        user_msg_id: "",
    });
    assert_eq!(out, "partial");
    te.close().await;
}

#[tokio::test]
async fn timed_out_finalize_is_not_a_user_stop() {
    let te = TestEngine::new(None);
    crate::turn::test_support::add_solo_workspace(&te.engine, "W1");
    let turn = te
        .engine
        .register_chat_turn("W1", "PA", CancellationToken::new())
        .unwrap();
    turn.arm(std::time::Duration::from_millis(1));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(te.engine.chat_turn_was_timed_out("W1", "PA"));
    let canceled = ProviderError::canceled();
    let out = te.engine.finalize_chat_reply(ChatOutcome {
        workspace_id: "W1",
        code: "PA",
        prompt: "hi",
        display: "",
        streamed: "",
        res: &TurnResult::default(),
        err: Some(&canceled),
        empty_fallback: "Done.",
        started: None,
        skip_user: false,
        user_msg_id: "",
    });
    assert_eq!(out, CHAT_TURN_TIMED_OUT_REPLY);
    te.close().await;
}

#[tokio::test]
async fn success_prefers_streamed_then_result_then_fallback() {
    let f = fixture(None).await;
    let ok = TurnResult {
        text: "from result".into(),
        ..Default::default()
    };
    assert_eq!(f.finalize("p", "", " streamed ", &ok, None), "streamed");
    assert_eq!(f.finalize("p", "", "", &ok, None), "from result");
    assert_eq!(
        f.finalize("p", "", "", &TurnResult::default(), None),
        "Done."
    );
    f.te.close().await;
}

#[test]
fn failure_duration_rules() {
    let emitted = TurnResult {
        duration_ms: 900,
        turn_end_emitted: true,
        ..Default::default()
    };
    assert_eq!(
        chat_failure_duration(&emitted, None),
        0,
        "one footer per turn"
    );
    let res = TurnResult {
        duration_ms: 900,
        ..Default::default()
    };
    assert_eq!(chat_failure_duration(&res, None), 900);
    let started = std::time::Instant::now() - std::time::Duration::from_millis(120);
    assert!(chat_failure_duration(&TurnResult::default(), Some(started)) >= 120);
}
