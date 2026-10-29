//! Router, mirror and broadcast (Go `mirror_test.go`, router cases of
//! `provider_test.go`, `mcp_required_test.go` registration cases).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use recur_agent::testing::ScriptedProvider;
use recur_agent::{AgentEvent, ToolResultEvent, ToolStep, TurnRequest, TurnResult};
use recur_domain::{
    AgentRole, BLOCK_TOOL_USE, COMPACT_DONE, Message, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX,
    PROVIDER_CURSOR, Session,
};
use tokio_util::sync::CancellationToken;

use super::mirror::{SessionMirror, requires_display_text, user_bubble_text};
use super::test_support::{claude, maestro_tool_fixture, script, started_loop};
use super::transcript::{FORK_PREAMBLE_LEAD, flatten_transcript, flatten_user_message};
use super::*;
use crate::bus::EngineEvent;
use crate::roles::CODER_FIX_DISPLAY;
use crate::testing::{TestEngine, add_project, init_repo};

fn coder_spec(ws: &str, model: &str, prompt: &str) -> AgentSpec {
    AgentSpec::new(TurnRequest {
        workspace_id: ws.into(),
        agent_id: format!("{ws}-CO"),
        role: AgentRole::Coder,
        model: model.into(),
        prompt: prompt.into(),
        ..TurnRequest::default()
    })
}

async fn engine_with(
    p: Arc<dyn recur_agent::Provider>,
) -> (TestEngine, String, crate::testing::TempRepo) {
    let te = TestEngine::new(Some(p));
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = started_loop(&te.engine, &proj).await;
    (te, ws, repo)
}

fn transcript(te: &TestEngine, sess: &Session) -> Vec<Message> {
    te.store
        .session_transcript(&sess.id)
        .unwrap()
        .unwrap_or_default()
}

fn coder_session(te: &TestEngine, ws: &str) -> Session {
    te.store
        .session_for_workspace_kind(ws, "coder")
        .expect("coder session")
}

#[tokio::test]
async fn wrapper_mirrors_specialist_turn() {
    let (te, ws, _repo) = engine_with(script(maestro_tool_fixture("coder-sess-1"))).await;
    let tok = CancellationToken::new();
    te.engine
        .run_agent(
            &tok,
            coder_spec(&ws, "claude-sonnet-5", "implement the feature"),
        )
        .await
        .unwrap();
    let sess = coder_session(&te, &ws);
    assert_eq!(sess.kind, "coder");
    assert_eq!(sess.provider, PROVIDER_CLAUDE_CODE);
    let msgs = transcript(&te, &sess);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(
        msgs[0].blocks()[0].as_text().unwrap().text,
        "implement the feature"
    );
    te.engine
        .run_agent(
            &tok,
            coder_spec(&ws, "claude-sonnet-5", "implement the feature"),
        )
        .await
        .unwrap();
    assert_eq!(
        coder_session(&te, &ws).id,
        sess.id,
        "one coder conversation"
    );
    te.close().await;
}

#[tokio::test]
async fn maestro_tool_turn_mirrors_losslessly() {
    let (te, ws, _repo) = engine_with(script(maestro_tool_fixture("maestro-sess-mirror"))).await;
    let mut spec = coder_spec(&ws, "claude-opus-4-6", "is auth ok?");
    spec.req.agent_id = format!("{ws}-MA");
    spec.req.role = AgentRole::Maestro;
    let res = te
        .engine
        .run_agent(&CancellationToken::new(), spec)
        .await
        .unwrap();
    assert_eq!(res.text, "Auth looks fine.");
    assert!(res.turn_end_emitted);
    let sess = te
        .store
        .session_for_workspace_kind(&ws, "maestro")
        .expect("maestro session");
    assert_eq!(sess.provider_session_ref, "maestro-sess-mirror");
    let check = |msgs: &[Message]| {
        assert_eq!(msgs.len(), 4);
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
        assert_eq!(msgs[0].blocks()[0].as_text().unwrap().text, "is auth ok?");
        let asst = &msgs[1];
        assert!(!asst.model_id.is_empty(), "assistant model-stamped");
        assert_eq!(asst.blocks().len(), 2);
        let tu = asst.blocks()[1].as_tool_use().unwrap();
        assert_eq!(
            (tu.name.as_str(), tu.provider_tool_use_id.as_str()),
            ("Read", "toolu_read1")
        );
        assert!(!tu.id.is_empty() && tu.id != tu.provider_tool_use_id);
        assert!(tu.input.contains("auth.go"));
        let tr = msgs[2].blocks()[0].as_tool_result().unwrap();
        assert_eq!(tr.result, "package auth");
        assert_eq!(tr.tool_use_id, tu.id, "dual-id replay");
        assert_eq!(
            msgs[3].blocks()[0].as_text().unwrap().text,
            "Auth looks fine."
        );
        assert_eq!(msgs[3].duration_ms, 900);
    };
    check(&transcript(&te, &sess));
    // Durable: a second store handle on the same database reads it back.
    let st2 = recur_store::Store::open(&te.data_dir.join("recur.db")).unwrap();
    check(&st2.session_transcript(&sess.id).unwrap().unwrap());
    te.close().await;
}

#[tokio::test]
async fn non_claude_turn_is_mirrored_under_its_provider() {
    let (te, ws, _repo) = engine_with(script(maestro_tool_fixture("cursor-sess-1"))).await;
    te.engine
        .run_agent(
            &CancellationToken::new(),
            coder_spec(&ws, "cursor/composer-2.5", "implement the feature"),
        )
        .await
        .unwrap();
    let sess = coder_session(&te, &ws);
    assert_eq!(sess.provider, PROVIDER_CURSOR);
    let msgs = transcript(&te, &sess);
    let mut saw_tool = false;
    for m in msgs.iter().filter(|m| m.role == "assistant") {
        assert_eq!(m.provider, PROVIDER_CURSOR);
        assert_eq!(m.model_id, "composer-2.5", "bare id");
        saw_tool |= m.blocks().iter().any(|b| b.r#type == BLOCK_TOOL_USE);
    }
    assert!(saw_tool, "tool calls mirrored");
    te.close().await;
}

#[tokio::test]
async fn cursor_turn_overwrites_claude_session_provider() {
    let (te, ws, _repo) = engine_with(script(maestro_tool_fixture("cursor-sess-2"))).await;
    te.engine
        .ensure_agent_session(&ws, "coder", "CO", "Coder", PROVIDER_CLAUDE_CODE)
        .unwrap();
    assert_eq!(coder_session(&te, &ws).provider, PROVIDER_CLAUDE_CODE);
    te.engine
        .run_agent(
            &CancellationToken::new(),
            coder_spec(&ws, "cursor/composer-2.5", "implement the feature"),
        )
        .await
        .unwrap();
    assert_eq!(coder_session(&te, &ws).provider, PROVIDER_CURSOR);
    te.close().await;
}

fn steps_on(te: &TestEngine, ws: &str) -> usize {
    te.events
        .by_name("session.step")
        .iter()
        .filter(|e| matches!(e, EngineEvent::SessionStep(s) if s.workspace_id == ws))
        .count()
}

fn counting_emit() -> (Emit, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&n);
    (
        Arc::new(move |ev: &AgentEvent| {
            if matches!(ev, AgentEvent::Step(_)) {
                c.fetch_add(1, Ordering::SeqCst);
            }
        }),
        n,
    )
}

#[tokio::test]
async fn repeated_tool_use_yields_one_step() {
    let dup = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_dup1","name":"Read","input":{"file_path":"auth.go"}}]}}"#;
    let lines = vec![
        r#"{"type":"system","subtype":"init","session_id":"claude-dedupe","model":"claude-sonnet-4-6","mcp_servers":[]}"#,
        dup,
        dup,
        dup,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_dup1","is_error":false,"content":"ok"}]}}"#,
        r#"{"type":"result","subtype":"success","session_id":"claude-dedupe","duration_ms":100,"is_error":false,"num_turns":1,"result":"done","total_cost_usd":0.01}"#,
    ];
    let (te, ws, _repo) = engine_with(script(lines)).await;
    let (emit, emitted) = counting_emit();
    let mut spec = coder_spec(&ws, "claude-sonnet-5", "read auth");
    spec.emit = Some(emit);
    te.engine
        .run_agent(&CancellationToken::new(), spec)
        .await
        .unwrap();
    assert_eq!(steps_on(&te, &ws), 1);
    assert_eq!(emitted.load(Ordering::SeqCst), 1);
    let msgs = transcript(&te, &coder_session(&te, &ws));
    let uses: Vec<_> = msgs
        .iter()
        .flat_map(|m| m.blocks().iter().filter_map(|b| b.as_tool_use()))
        .collect();
    let results: Vec<_> = msgs
        .iter()
        .flat_map(|m| m.blocks().iter().filter_map(|b| b.as_tool_result()))
        .collect();
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].provider_tool_use_id, "toolu_dup1");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_use_id, uses[0].id);
    te.close().await;
}

fn step(id: &str, name: &str, input: &str) -> AgentEvent {
    AgentEvent::Step(ToolStep {
        id: id.into(),
        name: name.into(),
        input: input.into(),
        ..Default::default()
    })
}

fn result_ev(id: &str, text: &str) -> AgentEvent {
    AgentEvent::ToolResult(ToolResultEvent {
        id: id.into(),
        text: text.into(),
        ..Default::default()
    })
}

#[tokio::test]
async fn non_live_user_resets_tool_id_dedupe() {
    let p = ScriptedProvider::from_events(vec![
        step("c1", "read", r#"{"path":"a.go"}"#),
        AgentEvent::User {
            text: "Maestro steer (follow this; it supersedes earlier plan guidance):\nstop".into(),
            display: String::new(),
            live_steer: false,
        },
        step("c1", "read", r#"{"path":"b.go"}"#),
        AgentEvent::TurnEnd(TurnResult {
            subtype: "success".into(),
            duration_ms: 10,
            ..Default::default()
        }),
    ]);
    let (te, ws, _repo) = engine_with(Arc::new(p)).await;
    let (emit, emitted) = counting_emit();
    let mut spec = coder_spec(&ws, "cursor/composer-2.5", "do the thing");
    spec.emit = Some(emit);
    te.engine
        .run_agent(&CancellationToken::new(), spec)
        .await
        .unwrap();
    assert_eq!(steps_on(&te, &ws), 2);
    assert_eq!(emitted.load(Ordering::SeqCst), 2);
    let uses = transcript(&te, &coder_session(&te, &ws))
        .iter()
        .flat_map(|m| {
            m.blocks()
                .iter()
                .filter_map(|b| b.as_tool_use())
                .collect::<Vec<_>>()
        })
        .count();
    assert_eq!(uses, 2, "a new provider run resets the tool-id map");
    te.close().await;
}

fn new_session(te: &TestEngine, provider: &str) -> Session {
    te.store
        .create_session(Session {
            kind: "pair".into(),
            provider: provider.into(),
            ..Session::default()
        })
        .unwrap()
}

fn attached_result(te: &TestEngine, sess: &Session, want: &str) -> bool {
    let msgs = transcript(te, sess);
    let (mut internal, mut result_id) = (String::new(), String::new());
    for m in &msgs {
        for b in m.blocks() {
            if let Some(tu) = b.as_tool_use() {
                internal = tu.id;
            }
            if let Some(tr) = b.as_tool_result() {
                result_id = tr.tool_use_id;
            }
        }
    }
    assert!(!internal.is_empty(), "missing tool_use");
    assert_eq!(result_id, internal, "result references the internal id");
    flatten_transcript(&te.engine.providers, &msgs, "", &Default::default())
        .iter()
        .any(|it| it.kind == "tool" && it.result == want && !it.result_error)
}

#[tokio::test]
async fn live_user_preserves_outstanding_tool_ids() {
    for provider in [PROVIDER_CLAUDE_CODE, PROVIDER_CODEX] {
        let te = TestEngine::new(Some(claude()));
        let sess = new_session(&te, provider);
        let mut m =
            SessionMirror::new(Arc::clone(&te.store), &sess.id, "claude-sonnet-5", provider);
        m.event(&step("toolu_steer1", "Read", r#"{"file_path":"a.go"}"#));
        m.event(&AgentEvent::User {
            text: "also check tests".into(),
            display: String::new(),
            live_steer: true,
        });
        m.event(&result_ev("toolu_steer1", "package a"));
        m.finish();
        assert!(attached_result(&te, &sess, "package a"), "{provider}");
        te.close().await;
    }
}

#[tokio::test]
async fn live_cursor_steer_preserves_tool_ids() {
    let cursor = recur_provider_cursor_display();
    let te = TestEngine::new(Some(cursor));
    let sess = new_session(&te, PROVIDER_CURSOR);
    let mut m = SessionMirror::new(
        Arc::clone(&te.store),
        &sess.id,
        "composer-2.5",
        PROVIDER_CURSOR,
    );
    m.event(&step("c1", "read", r#"{"path":"a.go"}"#));
    m.event(&AgentEvent::User {
        text: "also check tests".into(),
        display: String::new(),
        live_steer: true,
    });
    m.event(&result_ev(
        "c1",
        r#"{"status":"success","value":{"content":"package a"}}"#,
    ));
    m.finish();
    assert!(attached_result(&te, &sess, "package a"));
    te.close().await;
}

/// A pinned provider carrying Cursor's id and display rules.
fn recur_provider_cursor_display() -> Arc<dyn recur_agent::Provider> {
    Arc::new(ScriptedProvider::new(Vec::new()).imitating(Arc::new(
        recur_provider_cursor::CursorProvider::new(recur_provider_cursor::CursorConfig::default()),
    )))
}

#[tokio::test]
async fn parallel_tool_uses_each_persist() {
    let lines = vec![
        r#"{"type":"system","subtype":"init","session_id":"claude-parallel","model":"claude-sonnet-4-6","mcp_servers":[]}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_a","name":"Read","input":{"file_path":"a.go"}},{"type":"tool_use","id":"toolu_b","name":"Read","input":{"file_path":"b.go"}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_a","is_error":false,"content":"a"},{"type":"tool_result","tool_use_id":"toolu_b","is_error":false,"content":"b"}]}}"#,
        r#"{"type":"result","subtype":"success","session_id":"claude-parallel","duration_ms":100,"is_error":false,"num_turns":1,"result":"done","total_cost_usd":0.01}"#,
    ];
    let (te, ws, _repo) = engine_with(script(lines)).await;
    te.engine
        .run_agent(
            &CancellationToken::new(),
            coder_spec(&ws, "claude-sonnet-5", "read both"),
        )
        .await
        .unwrap();
    let mut ids: Vec<String> = transcript(&te, &coder_session(&te, &ws))
        .iter()
        .flat_map(|m| {
            m.blocks()
                .iter()
                .filter_map(|b| b.as_tool_use())
                .map(|t| t.provider_tool_use_id)
                .collect::<Vec<_>>()
        })
        .collect();
    ids.sort();
    assert_eq!(ids, ["toolu_a", "toolu_b"]);
    te.close().await;
}

#[test]
fn display_text_requirements() {
    assert!(requires_display_text(
        r#"<skill_content name="x">body</skill_content>"#
    ));
    assert!(requires_display_text(
        "The user has attached these files. Read them before proceeding."
    ));
    assert!(requires_display_text(
        "The user activated skill(s) via /name. Follow each skill's instructions below."
    ));
    assert!(!requires_display_text("just a normal human message"));
    assert!(requires_display_text(&format!(
        "{FORK_PREAMBLE_LEAD} Continue from the <transcript> below."
    )));
}

#[test]
fn bubble_text_prefers_display() {
    assert_eq!(
        user_bubble_text("WIRE long review body", "Review the current diff"),
        "Review the current diff"
    );
    assert_eq!(
        user_bubble_text(
            "You are the Coder agent. A reviewer found issues in pong",
            ""
        ),
        CODER_FIX_DISPLAY
    );
    assert_eq!(
        user_bubble_text("create a cli game of pong", ""),
        "create a cli game of pong"
    );
}

#[tokio::test]
async fn record_user_keeps_wire_and_display() {
    let wire = "<system_instruction>\nThe user activated skill(s) via /name.\n</system_instruction>\n\n<skill_content name=\"demo\">body</skill_content>\n\nplease review";
    assert!(requires_display_text(wire));
    let (te, ws, _repo) = engine_with(script(maestro_tool_fixture("disp-1"))).await;
    let display = "/demo please review";
    let mut spec = coder_spec(&ws, "claude-sonnet-5", wire);
    spec.req.display_text = display.into();
    te.engine
        .run_agent(&CancellationToken::new(), spec)
        .await
        .unwrap();
    let msgs = transcript(&te, &coder_session(&te, &ws));
    let p = msgs[0].blocks()[0].as_text().unwrap();
    assert_eq!(p.text, wire);
    assert_eq!(p.display_text, display);
    assert_eq!(flatten_user_message(&msgs[0]).unwrap().text, display);
    let users = te.events.by_name("session.user");
    assert!(
        users
            .iter()
            .any(|e| matches!(e, EngineEvent::SessionUser(u) if u.text == display)),
        "the live bubble is the display text"
    );
    te.close().await;
}

fn fallback(from: &str, to: &str, retracted: &[&str], uuid: &str) -> AgentEvent {
    AgentEvent::Fallback {
        from_model: from.into(),
        to_model: to.into(),
        retracted_uuids: retracted.iter().map(|s| (*s).to_owned()).collect(),
        uuid: uuid.into(),
    }
}

fn text(t: &str, uuid: &str) -> AgentEvent {
    AgentEvent::Text {
        text: t.into(),
        uuid: uuid.into(),
        parent: String::new(),
    }
}

fn mirror_events(evs: &[AgentEvent]) -> (TestEngine, Vec<Message>) {
    let te = TestEngine::new(Some(claude()));
    let sess = new_session(&te, PROVIDER_CLAUDE_CODE);
    let mut m = SessionMirror::new(
        Arc::clone(&te.store),
        &sess.id,
        "claude-fable-5",
        PROVIDER_CLAUDE_CODE,
    );
    for ev in evs {
        m.event(ev);
    }
    m.finish();
    let msgs = transcript(&te, &sess);
    (te, msgs)
}

fn fallbacks(msgs: &[Message]) -> Vec<recur_domain::FallbackPayload> {
    msgs.iter()
        .flat_map(|m| {
            m.blocks()
                .iter()
                .filter_map(|b| b.as_fallback())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn texts(msgs: &[Message]) -> Vec<String> {
    msgs.iter()
        .flat_map(|m| {
            m.blocks()
                .iter()
                .filter_map(|b| b.as_text().map(|p| p.text))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[tokio::test]
async fn fallback_dedup_after_tool_result() {
    let (te, msgs) = mirror_events(&[
        fallback("claude-fable-5", "claude-opus-4-8", &[], ""),
        text("checking", ""),
        step("t1", "Bash", "{}"),
        result_ev("t1", "ok"),
        fallback("claude-fable-5", "claude-opus-4-8", &[], ""),
        text("done", ""),
    ]);
    let f = fallbacks(&msgs);
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].from.as_str(), f[0].to.as_str()),
        ("claude-fable-5", "claude-opus-4-8")
    );
    te.close().await;
}

#[tokio::test]
async fn fallback_stamps_serving_model() {
    let (te, msgs) = mirror_events(&[
        fallback("claude-fable-5", "claude-opus-4-8", &[], ""),
        text("served by Opus", ""),
    ]);
    let asst = msgs.iter().find(|m| m.role == "assistant").unwrap();
    assert_eq!(asst.model_id, "claude-opus-4-8");
    te.close().await;
}

#[tokio::test]
async fn retracts_refused_assistant() {
    let (te, msgs) = mirror_events(&[
        text("refused fable output", "fable-frame"),
        fallback(
            "claude-fable-5",
            "claude-opus-4-8",
            &["fable-frame"],
            "opus-frame",
        ),
        text("opus replacement", "opus-frame"),
    ]);
    assert_eq!(fallbacks(&msgs).len(), 1);
    let t = texts(&msgs);
    assert!(!t.contains(&"refused fable output".to_owned()));
    assert!(t.contains(&"opus replacement".to_owned()));
    assert!(
        msgs.iter()
            .filter(|m| m.role == "assistant")
            .all(|m| m.model_id != "claude-fable-5")
    );
    te.close().await;
}

fn assert_notice_before_answer(msgs: &[Message]) {
    let asst = msgs.iter().find(|m| m.role == "assistant").unwrap();
    assert_eq!(asst.model_id, "claude-opus-4-8");
    let (mut saw_fallback, mut saw_text) = (false, false);
    for b in asst.blocks() {
        if b.as_fallback().is_some() {
            assert!(!saw_text, "fallback notice before the answer");
            saw_fallback = true;
        }
        if b.as_text().is_some_and(|p| p.text == "opus replacement") {
            saw_text = true;
        }
    }
    assert!(saw_fallback);
}

#[tokio::test]
async fn supersedes_opens_fresh_replacement() {
    let (te, msgs) = mirror_events(&[
        text("refused fable output", "fable-frame"),
        fallback(
            "claude-fable-5",
            "claude-opus-4-8",
            &["fable-frame"],
            "opus-frame",
        ),
        text("opus replacement", "opus-frame"),
        fallback("claude-fable-5", "claude-opus-4-8", &["fable-frame"], ""),
    ]);
    assert_eq!(fallbacks(&msgs).len(), 1);
    assert_notice_before_answer(&msgs);
    let t = texts(&msgs);
    assert!(!t.contains(&"refused fable output".to_owned()));
    assert!(t.contains(&"opus replacement".to_owned()));
    te.close().await;
}

#[tokio::test]
async fn fallback_enriches_incomplete_from() {
    let (te, msgs) = mirror_events(&[
        text("refused fable output", "fable-frame"),
        fallback("", "claude-opus-4-8", &["fable-frame"], "opus-frame"),
        text("opus replacement", "opus-frame"),
        fallback("claude-fable-5", "claude-opus-4-8", &["fable-frame"], ""),
    ]);
    let f = fallbacks(&msgs);
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].from.as_str(), f[0].to.as_str()),
        ("claude-fable-5", "claude-opus-4-8")
    );
    assert_notice_before_answer(&msgs);
    te.close().await;
}

#[tokio::test]
async fn retract_clears_token_only_thinking() {
    let (te, msgs) = mirror_events(&[
        AgentEvent::ThinkingTokens {
            tokens: 120,
            uuid: "fable-frame".into(),
            parent: String::new(),
        },
        fallback(
            "claude-fable-5",
            "claude-opus-4-8",
            &["fable-frame"],
            "opus-frame",
        ),
        AgentEvent::ThinkingTokens {
            tokens: 8,
            uuid: "opus-frame".into(),
            parent: String::new(),
        },
        AgentEvent::Thinking {
            text: String::new(),
            signature: "sig".into(),
            uuid: "opus-frame".into(),
            parent: String::new(),
        },
    ]);
    let tokens: Vec<i64> = msgs
        .iter()
        .flat_map(|m| {
            m.blocks()
                .iter()
                .filter_map(|b| b.as_thinking().map(|t| t.estimated_tokens))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(tokens, [8]);
    te.close().await;
}

#[tokio::test]
async fn compact_writes_session_context() {
    let te = TestEngine::new(Some(claude()));
    let sess = new_session(&te, PROVIDER_CURSOR);
    let compact = |summary: &str, tokens: i64| AgentEvent::Compact {
        status: COMPACT_DONE.into(),
        trigger: if summary.is_empty() {
            String::new()
        } else {
            "auto".into()
        },
        summary: summary.into(),
        pre_tokens: tokens,
        uuid: String::new(),
        parent: String::new(),
    };
    let mut m = SessionMirror::new(
        Arc::clone(&te.store),
        &sess.id,
        "composer-2.5",
        PROVIDER_CURSOR,
    );
    m.event(&text("pre compact reply", ""));
    m.event(&compact("older turns folded", 800_000));
    m.finish();
    let ctx = te.store.active_session_context(&sess.id).expect("context");
    assert_eq!(ctx.summary, "older turns folded");
    assert_eq!(
        te.store.get_session(&sess.id).unwrap().active_context_id,
        ctx.id
    );
    assert!(!ctx.last_compacted_message_id.is_empty());
    let mut m2 = SessionMirror::new(
        Arc::clone(&te.store),
        &sess.id,
        "composer-2.5",
        PROVIDER_CURSOR,
    );
    m2.event(&compact("", 0));
    m2.finish();
    assert_eq!(
        te.store.active_session_context(&sess.id).unwrap().id,
        ctx.id
    );
    te.close().await;
}

// ---- router (provider_test.go)

#[tokio::test]
async fn nil_pin_fails_closed() {
    let te = TestEngine::new(None);
    let e = te
        .engine
        .run_agent(
            &CancellationToken::new(),
            AgentSpec::new(TurnRequest {
                model: "sonnet".into(),
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        "engine: agent runner was explicitly set to nil; no turn can run"
    );
    te.close().await;
}

fn routed(providers: Vec<Arc<dyn recur_agent::Provider>>) -> TestEngine {
    TestEngine::with(crate::testing::TestOptions {
        registered: Some(providers),
        ..Default::default()
    })
}

#[tokio::test]
async fn unknown_provider_fails_without_fallback() {
    let te = routed(vec![claude()]);
    let e = te
        .engine
        .run_agent(
            &CancellationToken::new(),
            AgentSpec::new(TurnRequest {
                model: "nope/x".into(),
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        "unknown agent provider \"nope\" — pick a model from Settings → Models"
    );
    te.close().await;
}

#[tokio::test]
async fn empty_provider_is_claude_code() {
    let te = routed(vec![claude()]);
    assert_eq!(
        te.engine
            .resolve_provider(&parse_model_ref("").provider)
            .unwrap()
            .id(),
        PROVIDER_CLAUDE_CODE
    );
    assert_eq!(
        te.engine
            .resolve_provider(&parse_model_ref("sonnet").provider)
            .unwrap()
            .id(),
        PROVIDER_CLAUDE_CODE
    );
    assert_eq!(
        te.engine
            .resolve_provider(PROVIDER_CLAUDE_CODE)
            .unwrap()
            .id(),
        PROVIDER_CLAUDE_CODE
    );
    te.close().await;
}

#[tokio::test]
async fn disabled_provider_reason_is_the_turn_error() {
    let te = routed(vec![claude()]);
    let set = recur_domain::Settings {
        disabled_providers: vec![PROVIDER_CLAUDE_CODE.into()],
        ..te.store.settings()
    };
    te.store.set_settings(set);
    let e = te
        .engine
        .run_agent(
            &CancellationToken::new(),
            AgentSpec::new(TurnRequest {
                model: "sonnet".into(),
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        e.message,
        "Claude Code is turned off in Settings → Providers"
    );
    te.close().await;
}

#[tokio::test]
async fn injected_provider_beats_routing_and_partials_follow_skip_thinking() {
    let seen: Arc<parking_lot::Mutex<Vec<TurnRequest>>> = Arc::default();
    let s2 = Arc::clone(&seen);
    let p = ScriptedProvider::new(Vec::new())
        .imitating(claude())
        .on_run(move |r| s2.lock().push(r.clone()));
    let te = TestEngine::new(Some(Arc::new(p)));
    let tok = CancellationToken::new();
    te.engine
        .run_agent(
            &tok,
            AgentSpec::new(TurnRequest {
                model: "cursor/composer-2.5".into(),
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap();
    te.engine
        .run_agent(
            &tok,
            AgentSpec::new(TurnRequest {
                model: "claude-code/haiku".into(),
                skip_thinking: true,
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap();
    let seen = seen.lock().clone();
    assert_eq!(seen[0].model, "composer-2.5", "bare id to the provider");
    assert!(seen[0].include_partials);
    assert_eq!(seen[1].model, "haiku");
    assert!(!seen[1].include_partials, "one-shots skip partials");
    te.close().await;
}

#[tokio::test]
async fn run_agent_publishes_user_display() {
    let (te, ws, _repo) =
        engine_with(script(super::test_support::maestro_fixture("s", "ok"))).await;
    let mut spec = coder_spec(&ws, "sonnet", "WIRE");
    spec.req.display_text = "Shown".into();
    te.engine
        .run_agent(&CancellationToken::new(), spec)
        .await
        .unwrap();
    let users: Vec<String> = te
        .events
        .by_name("session.user")
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::SessionUser(u) => Some(u.text),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["Shown"]);
    te.close().await;
}

#[tokio::test]
async fn ensure_mcp_retries_after_failure_and_register_scopes_tools() {
    let te = TestEngine::new(Some(claude()));
    let (token, url) = te
        .engine
        .register_mcp(
            "W1",
            "CO",
            "",
            &["Read".into(), "mcp__recur__done".into()],
            "sonnet",
        )
        .await
        .unwrap();
    assert!(url.starts_with("http://127.0.0.1:") && url.ends_with(&format!("/{token}/mcp")));
    let (again, _) = te
        .engine
        .register_mcp("W1", "CO", "", &[], "sonnet")
        .await
        .unwrap();
    assert_ne!(again, token, "an overlapping turn mints its own token");
    te.engine.unregister_mcp(&token);
    te.engine.unregister_mcp(&again);
    let (reused, _) = te
        .engine
        .register_mcp("W1", "CO", "", &[], "sonnet")
        .await
        .unwrap();
    assert_eq!(reused, token, "sequential turns reuse the stable token");
    te.close().await;
}

/// Go `TestEnsureMCPRetriesAfterFailure`: a bind failure is not remembered,
/// so Retry can work once the condition clears.
#[tokio::test]
async fn ensure_mcp_start_failure_is_retried() {
    // Below the ephemeral range, so a parallel test binding :0 cannot take
    // the port once it is released.
    let start = 20_000 + (std::process::id() % 5_000) as u16;
    let held = (start..30_000)
        .find_map(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok())
        .expect("a free port below the ephemeral range");
    let addr = held.local_addr().unwrap().to_string();
    let te = TestEngine::with(crate::testing::TestOptions {
        provider: Some(claude()),
        mcp_addr: Some(addr.clone()),
        ..Default::default()
    });
    let e = te.engine.ensure_mcp().await.unwrap_err();
    assert!(
        e.0.starts_with(&format!("mcp listen: listen tcp {addr}: bind: ")),
        "{}",
        e.0
    );
    let e = te
        .engine
        .register_mcp("W1", "CO", "", &[], "sonnet")
        .await
        .unwrap_err();
    assert!(e.0.contains("mcp listen"), "{}", e.0);
    drop(held);
    // On macOS a git child spawned by a parallel test can inherit the
    // listener (CLOEXEC is set after socket creation) and hold the port for
    // its short life; each failed attempt is another retry.
    let mut tries = 0;
    while let Err(e) = te.engine.ensure_mcp().await {
        tries += 1;
        assert!(
            e.0.ends_with("address already in use") && tries < 200,
            "{}",
            e.0
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let port = te.engine.mcp().port();
    assert_eq!(addr, format!("127.0.0.1:{port}"));
    te.engine.ensure_mcp().await.unwrap();
    assert_eq!(
        te.engine.mcp().port(),
        port,
        "an already-started server is not rebound"
    );
    te.close().await;
}

#[tokio::test]
async fn in_process_providers_get_a_turn_token() {
    let compat =
        recur_provider_compatible::CompatibleProvider::new(recur_domain::CompatibleProvider {
            id: "local".into(),
            name: "Local".into(),
            base_url: "http://127.0.0.1:1".into(),
            ..Default::default()
        });
    let te = TestEngine::new(Some(Arc::new(
        ScriptedProvider::new(Vec::new()).imitating(Arc::new(compat)),
    )));
    let (token, url) = te
        .engine
        .register_mcp("W1", "PA", "", &[], "local/m")
        .await
        .unwrap();
    assert_eq!(token, "compat\x00W1\x00PA");
    assert!(url.is_empty());
    te.engine.unregister_mcp(&token);
    te.close().await;
}

// ---- broadcast (thinking_broadcast_test.go, context_usage_test.go)

fn bc(te: &TestEngine) -> SessionBroadcast {
    SessionBroadcast::new(
        Arc::clone(&te.engine),
        crate::bus::SessionRoute {
            workspace_id: "ws-1".into(),
            code: "PA".into(),
            ..Default::default()
        },
        claude(),
    )
}

fn thinking_payloads(te: &TestEngine) -> Vec<String> {
    te.events
        .by_name("session.thinking")
        .iter()
        .map(EngineEvent::payload_json)
        .collect()
}

fn think_delta(t: &str) -> AgentEvent {
    AgentEvent::ThinkingDelta {
        text: t.into(),
        parent: String::new(),
    }
}

fn thinking(t: &str, uuid: &str) -> AgentEvent {
    AgentEvent::Thinking {
        text: t.into(),
        signature: String::new(),
        uuid: uuid.into(),
        parent: String::new(),
    }
}

#[tokio::test]
async fn thinking_replaces_after_deltas() {
    let te = TestEngine::new(None);
    let mut b = bc(&te);
    b.emit(&think_delta("half "));
    b.emit(&think_delta("a thought"));
    b.emit(&thinking("half a thought", ""));
    assert_eq!(
        thinking_payloads(&te),
        [
            r#"{"code":"PA","replace":true,"seq":1,"text":"half ","workspaceId":"ws-1"}"#,
            r#"{"code":"PA","replace":true,"seq":2,"text":"half a thought","workspaceId":"ws-1"}"#,
            r#"{"code":"PA","done":true,"replace":true,"seq":3,"text":"half a thought","workspaceId":"ws-1"}"#,
        ]
    );
    // The next block streams afresh.
    b.emit(&thinking("next", ""));
    assert_eq!(
        thinking_payloads(&te)[3],
        r#"{"code":"PA","done":true,"text":"next","workspaceId":"ws-1"}"#
    );
    te.close().await;
}

#[tokio::test]
async fn notice_broadcast() {
    let te = TestEngine::new(None);
    let mut b = bc(&te);
    b.emit(&AgentEvent::Notice {
        text: "Waiting for previous run run-1 to finish.".into(),
    });
    b.emit(&AgentEvent::Notice { text: "   ".into() });
    b.emit(&text("hello", ""));
    let notices = te.events.by_name("session.notice");
    assert_eq!(notices.len(), 1);
    assert_eq!(
        notices[0].payload_json(),
        r#"{"code":"PA","text":"Waiting for previous run run-1 to finish.","workspaceId":"ws-1"}"#
    );
    assert_eq!(
        te.events.count("session.delta"),
        1,
        "notice never folds into delta"
    );
    te.close().await;
}

#[tokio::test]
async fn thinking_without_deltas_streams_once() {
    let te = TestEngine::new(None);
    bc(&te).emit(&thinking("one shot", ""));
    assert_eq!(
        thinking_payloads(&te),
        [r#"{"code":"PA","done":true,"text":"one shot","workspaceId":"ws-1"}"#]
    );
    te.close().await;
}

#[tokio::test]
async fn thinking_seq_monotonic_across_turns() {
    let te = TestEngine::new(None);
    let mut t1 = bc(&te);
    t1.emit(&think_delta("first "));
    t1.emit(&think_delta("turn"));
    t1.emit(&thinking("first turn", ""));
    assert_eq!(te.engine.stream_seq_seed("ws-1", "PA"), (0, 3));
    let mut t2 = bc(&te);
    t2.emit(&think_delta("second"));
    t2.emit(&thinking("second", ""));
    let seqs: Vec<i64> = te
        .events
        .by_name("session.thinking")
        .iter()
        .filter_map(|e| match e {
            EngineEvent::SessionThinking(t) => t.seq,
            _ => None,
        })
        .collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    te.close().await;
}

#[tokio::test]
async fn thinking_ignores_per_event_uuids() {
    let te = TestEngine::new(None);
    let mut b = bc(&te);
    b.emit(&think_delta("Act"));
    b.emit(&AgentEvent::ThinkingTokens {
        tokens: 326,
        uuid: "evt-2".into(),
        parent: String::new(),
    });
    b.emit(&think_delta("ually"));
    b.emit(&thinking("Actually the bigger issue", "asst-frame"));
    assert_eq!(
        thinking_payloads(&te),
        [
            r#"{"code":"PA","replace":true,"seq":1,"text":"Act","workspaceId":"ws-1"}"#,
            r#"{"code":"PA","tokens":326,"workspaceId":"ws-1"}"#,
            r#"{"code":"PA","replace":true,"seq":2,"text":"Actually","workspaceId":"ws-1"}"#,
            r#"{"code":"PA","done":true,"replace":true,"seq":3,"text":"Actually the bigger issue","uuid":"asst-frame","workspaceId":"ws-1"}"#,
        ]
    );
    te.close().await;
}

#[tokio::test]
async fn nested_streams_keep_their_own_buffers() {
    let te = TestEngine::new(None);
    let mut b = bc(&te);
    b.emit(&AgentEvent::TextDelta {
        text: "top".into(),
        parent: String::new(),
    });
    b.emit(&AgentEvent::TextDelta {
        text: "sub".into(),
        parent: "toolu_task".into(),
    });
    b.emit(&AgentEvent::Text {
        text: "sub done".into(),
        uuid: "x".into(),
        parent: "toolu_task".into(),
    });
    b.emit(&step("toolu_n1", "Read", r#"{"file_path":"a.go"}"#));
    let deltas: Vec<String> = te
        .events
        .by_name("session.delta")
        .iter()
        .map(EngineEvent::payload_json)
        .collect();
    assert_eq!(
        deltas,
        [
            r#"{"code":"PA","replace":true,"seq":1,"text":"top","workspaceId":"ws-1"}"#,
            r#"{"code":"PA","parentId":"toolu_task","replace":true,"seq":1,"text":"sub","workspaceId":"ws-1"}"#,
            r#"{"code":"PA","done":true,"parentId":"toolu_task","replace":true,"seq":2,"text":"sub done","workspaceId":"ws-1"}"#,
        ]
    );
    assert_eq!(
        te.events.by_name("session.step")[0].payload_json(),
        r#"{"code":"PA","description":"","id":"toolu_n1","input":"{\"file_path\":\"a.go\"}","parentId":"","tool":"Read","toolInput":"","workspaceId":"ws-1"}"#
    );
    te.close().await;
}

#[tokio::test]
async fn last_assistant_usage_wins_over_cumulative_result() {
    let lines = vec![
        r#"{"type":"system","subtype":"init","session_id":"sess-ctx","model":"claude-sonnet-5","mcp_servers":[]}"#,
        r#"{"type":"assistant","message":{"usage":{"input_tokens":50,"output_tokens":10,"cache_read_input_tokens":10000,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"step1"},{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"a.go"}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#,
        r#"{"type":"assistant","message":{"usage":{"input_tokens":200,"output_tokens":40,"cache_read_input_tokens":90000,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"done"}]}}"#,
        r#"{"type":"result","subtype":"success","session_id":"sess-ctx","duration_ms":800,"is_error":false,"num_turns":2,"result":"done","total_cost_usd":0.12,"usage":{"input_tokens":250,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":100000},"modelUsage":{"claude-sonnet-5":{"contextWindow":1000000,"inputTokens":250,"outputTokens":50,"cacheReadInputTokens":100000,"cacheCreationInputTokens":0}}}"#,
    ];
    let (te, ws, _repo) = engine_with(script(lines)).await;
    let res = te
        .engine
        .run_agent(
            &CancellationToken::new(),
            coder_spec(&ws, "claude-code/sonnet", "go"),
        )
        .await
        .unwrap();
    assert_eq!(
        (res.usage.input_tokens, res.usage.cache_read_tokens),
        (200, 90_000)
    );
    assert_eq!(res.usage.context_used(), 90_200);
    let msgs = transcript(&te, &coder_session(&te, &ws));
    let stamped = msgs.iter().rev().find(|m| m.duration_ms > 0).unwrap();
    assert_eq!(
        (stamped.input_tokens, stamped.cache_read_tokens),
        (200, 90_000)
    );
    let items = flatten_transcript(
        &te.engine.providers,
        &msgs,
        "claude-code/sonnet",
        &Default::default(),
    );
    let turn = items.iter().find(|i| i.kind == "turn").unwrap();
    assert_eq!((turn.input_tokens, turn.cache_read_tokens), (200, 90_000));
    assert_eq!(turn.context_window, 1_000_000, "always-1M Sonnet");
    let end = te.events.by_name("session.turn_end");
    assert_eq!(
        end[0].payload_json(),
        format!(
            r#"{{"cacheCreationTokens":0,"cacheReadTokens":90000,"code":"CO","contextWindow":1000000,"durationMs":800,"inputTokens":200,"workspaceId":"{ws}"}}"#
        )
    );
    te.close().await;
}

/// Go `TestRealPermissionPromptE2E`: a real `claude -p` asks through Recur's
/// MCP prompt tool and the approval lets the tool run. Spends credit, so it
/// is skipped unless `RECUR_E2E=1`.
#[tokio::test]
async fn real_permission_prompt_e2e() {
    if std::env::var("RECUR_E2E").as_deref() != Ok("1") {
        eprintln!("set RECUR_E2E=1 to run the real permission-prompt E2E (spends credit)");
        return;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    if !std::env::split_paths(&path).any(|d| d.join("claude").is_file()) {
        eprintln!("claude not on PATH");
        return;
    }
    let te = routed(vec![claude()]);
    let (token, url) = te
        .engine
        .register_mcp("W-E2E", "CO", "", &["Read".into()], "haiku")
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("approved.txt");
    let asked = Arc::new(AtomicUsize::new(0));
    let stop = CancellationToken::new();
    let human = {
        let eng = Arc::clone(&te.engine);
        let asked = Arc::clone(&asked);
        let stop = stop.clone();
        tokio::spawn(async move {
            while !stop.is_cancelled() {
                for p in eng.pending_permissions() {
                    asked.fetch_add(1, Ordering::SeqCst);
                    eng.answer_permission(
                        &p.id,
                        recur_domain::PermissionDecision {
                            allow: true,
                            ..Default::default()
                        },
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
    };
    let tok = CancellationToken::new();
    let run = te.engine.run_agent(
        &tok,
        AgentSpec::new(TurnRequest {
            workspace_id: "W-E2E".into(),
            agent_id: "W-E2E-CO".into(),
            model: "claude-code/haiku".into(),
            prompt: format!(
                "Use the Write tool to create the file {} containing exactly: ok",
                target.display()
            ),
            worktree: dir.path().to_path_buf(),
            allowed_tools: Some(vec!["Read".into()]),
            permission_mode: recur_domain::Settings::default().permission_mode().into(),
            mcp_url: url,
            max_turns: 4,
            ..TurnRequest::default()
        }),
    );
    let res = tokio::time::timeout(std::time::Duration::from_secs(180), run)
        .await
        .expect("claude run timed out")
        .expect("claude run");
    stop.cancel();
    human.await.unwrap();
    te.engine.unregister_mcp(&token);
    assert!(
        asked.load(Ordering::SeqCst) > 0,
        "claude never called the permission prompt tool: the flag or its tool name has drifted \
         (result: is_error={} subtype={} text={:?})",
        res.is_error,
        res.subtype,
        res.text
    );
    assert!(target.exists(), "approved Write did not happen");
    te.close().await;
}

/// The history in-process providers replay: the stored transcript plus the
/// fresh_session cutoff (`nativeHistoryAfter`) from the session config.
#[tokio::test]
async fn store_history_serves_transcript_and_fresh_session_cutoff() {
    let te = TestEngine::new(Some(claude()));
    super::test_support::add_solo_workspace(&te.engine, "W1");
    te.engine.ensure_pair_agent("W1", "PA", "");
    let sess = te
        .engine
        .ensure_agent_session("W1", "coder", "PA", "Agent", "local")
        .unwrap();
    let first = te
        .store
        .append_message(
            &sess.id,
            recur_domain::Message {
                role: "user".into(),
                content: Some(vec![recur_domain::ContentBlock::text(
                    "hello from last week",
                )]),
                ..recur_domain::Message::default()
            },
        )
        .unwrap();
    let h = StoreHistory {
        eng: Arc::clone(&te.engine),
    };
    use recur_agent::HistorySource;
    assert_eq!(h.history_after(&sess.id).await, "");
    te.store
        .patch_session_config_merge(
            &sess.id,
            &format!(r#"{{"nativeHistoryAfter":" {} "}}"#, first.id),
        )
        .unwrap();
    assert_eq!(h.history_after(&sess.id).await, first.id);
    let msgs = h.transcript(&sess.id).await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].id, first.id);
    assert!(h.transcript("missing").await.is_empty());
    assert_eq!(h.history_after("missing").await, "");
    te.close().await;
}

#[tokio::test]
async fn session_broadcast_compacts_list_findings_when_result_has_no_tool_name() {
    let te = TestEngine::new(None);
    let mut b = bc(&te);
    let findings: Vec<serde_json::Value> = (0..15)
        .map(|i| {
            serde_json::json!({
                "id": format!("f{i}"), "file": format!("src/file_{i}.rs"), "line": i,
                "severity": "medium", "status": "open", "message": "x".repeat(1500),
            })
        })
        .collect();
    let raw = serde_json::json!({"status": "all", "count": 15, "findings": findings}).to_string();
    assert!(raw.len() > recur_agent::display::CHAT_DETAIL_LIMIT);
    b.emit(&AgentEvent::Step(recur_agent::ToolStep {
        id: "t1".into(),
        name: "mcp__recur__list_findings".into(),
        input: r#"{"status":"all"}"#.into(),
        ..Default::default()
    }));
    // Claude-style result: no tool name, the findings JSON as text.
    b.emit(&AgentEvent::ToolResult(recur_agent::ToolResultEvent {
        id: "t1".into(),
        text: raw,
        ..Default::default()
    }));
    let results = te.events.by_name("session.tool_result");
    let payload: serde_json::Value = serde_json::from_str(&results[0].payload_json()).unwrap();
    let result = payload["result"].as_str().unwrap();
    assert!(result.len() <= recur_agent::display::CHAT_DETAIL_LIMIT);
    let v: serde_json::Value = serde_json::from_str(result).expect("still valid JSON");
    assert_eq!(v["count"], 15);
    te.close().await;
}

fn recording() -> (
    Arc<dyn recur_agent::Provider>,
    Arc<parking_lot::Mutex<Vec<TurnRequest>>>,
) {
    let seen: Arc<parking_lot::Mutex<Vec<TurnRequest>>> = Arc::default();
    let s2 = Arc::clone(&seen);
    let p = ScriptedProvider::new(Vec::new())
        .imitating(claude())
        .on_run(move |r| s2.lock().push(r.clone()));
    (Arc::new(p), seen)
}

#[tokio::test]
async fn mirroring_runner_assigns_user_message_id_for_history() {
    let (p, seen) = recording();
    let (te, ws, _repo) = engine_with(p).await;
    let prompt = "a long briefing that must not duplicate";
    te.engine
        .run_agent(
            &CancellationToken::new(),
            coder_spec(&ws, "claude-sonnet-5", prompt),
        )
        .await
        .unwrap();
    let got = seen.lock()[0].clone();
    assert!(!got.user_message_id.is_empty());
    let msgs = te
        .store
        .session_transcript(&got.recur_session_id)
        .unwrap()
        .unwrap_or_default();
    let users: Vec<&Message> = msgs.iter().filter(|m| m.role == "user").collect();
    assert_eq!(users.len(), 1, "the prompt is recorded once");
    assert_eq!(users[0].id, got.user_message_id);
    te.close().await;
}

#[tokio::test]
async fn mirroring_runner_retry_resolves_user_message_id() {
    let (p, seen) = recording();
    let (te, ws, _repo) = engine_with(p).await;
    let base = || coder_spec(&ws, "claude-sonnet-5", "retry this briefing");
    te.engine
        .run_agent(&CancellationToken::new(), base())
        .await
        .unwrap();
    let mut retry = base();
    retry.req.skip_record_user = true;
    retry.req.user_message_id = "a-fresh-retry-id".into();
    te.engine
        .run_agent(&CancellationToken::new(), retry)
        .await
        .unwrap();
    let seen = seen.lock().clone();
    assert!(!seen[0].user_message_id.is_empty());
    assert_eq!(
        seen[1].user_message_id, seen[0].user_message_id,
        "a retry reuses the recorded row"
    );
    te.close().await;
}
