//! Send-path cases earlier slices left for E4: `mcp_required_test.go`
//! `TestDeadMCPFailsChatTurn`, `workspace_chat_fresh_session_test.go`, the
//! send cases of `skill_mentions_test.go`, `simplify_test.go` and
//! `user_mcp_test.go`, the footer cases of `chat_errors_test.go`, the
//! run-lock and close cases of `loop_engage_test.go`, the CloseWorkspaceChat
//! cases of `chat_inbox_test.go` / `attention_test.go`, and the remount
//! history case of `harness_test.go` (checked against the transcript the
//! history source serves, since the Responses builder is P3's).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use recur_agent::{AgentEvent, ProviderError, TurnResult};
use recur_domain::{
    AutonomyLevel, BLOCK_IMAGE, CHAT_MODE_AGENT, CHAT_MODE_ASK, CHAT_MODE_LOOP, CHAT_MODE_PLAN,
    ChatAttachment, ContentBlock, LINEAR_MCP_READ_ONLY_URL, LINEAR_MCP_WRITE_URL, Message,
    PROVIDER_CLAUDE_CODE, Session, UserQuestion, WorkspaceState,
};
use tokio_util::sync::CancellationToken;

use super::test_support::*;
use crate::attention::chat_inbox::chat_inbox_id;
use crate::codes::{CHAT_RETRY_DISPLAY_SENTINEL, MAESTRO_AGENT_CODE, PAIR_AGENT_CODE};
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loops::test_support::{
    AsyncProvider, CaptureScript, enable_linear_mcp, linear_trackers, success,
};
use crate::testing::{TestOptions, wait_for};
use crate::turn::finalize::{CHAT_AGENT_ERROR_PREFIX, CHAT_STOPPED_REPLY};
use crate::turn::mirror::requires_display_text;
use crate::turn::native_history_after_id;
use crate::turn::test_support::{duration_error, result_then_error};
use crate::turn::transcript::FORK_PREAMBLE_LEAD;
use crate::view::mentions::attachment_marker;

fn write(path: &std::path::Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// Installs `/re-review` (fresh_session) and `/fix-findings`, both expanding to
/// the wire `WIRE`.
fn install_fresh_session_prompts(fx: &Fx) {
    let root = std::path::Path::new(&fx.eng().bundle_roots().default_dir).join("ChatTest");
    let script = root.join("src").join("act.sh");
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    recur_exec::write_executable(
        &script,
        "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE\",\"display\":\"Act\"}'\n",
        0o755,
    )
    .unwrap();
    write(
        &root.join("bundle.toml"),
        r#"[bundle]
name = "ChatTest"
id = "recur.chattest"
version = "1.0.0"

[[prompt]]
id = "recur.chattest.re-review"
name = "Re-review"
slash = "re-review"
fresh_session = true
interpreter = "shebang"
script = "src/act.sh"

[[prompt]]
id = "recur.chattest.fix-findings"
name = "Fix Findings"
slash = "fix-findings"
interpreter = "shebang"
script = "src/act.sh"
"#,
    );
}

/// A capture runner whose call `n` (1-based) reports session `sess-n`.
fn numbered() -> Arc<CaptureScript> {
    capture_fn(|call| format!("sess-{}", call + 1))
}

fn capture_fn(session: impl Fn(usize) -> String + Send + Sync + 'static) -> Arc<CaptureScript> {
    CaptureScript::new(move |call| crate::turn::test_support::maestro_fixture(&session(call), "ok"))
}

fn turn_message_id(items: &[recur_domain::ChatItem]) -> String {
    items
        .iter()
        .find(|it| it.kind == "turn")
        .map(|it| it.message_id.clone())
        .unwrap_or_default()
}

/// The messages a history-rebuilding provider replays: those after the
/// fresh_session cutoff.
fn after_cutoff(msgs: &[Message], after_id: &str) -> Vec<Message> {
    let start = msgs
        .iter()
        .position(|m| !after_id.is_empty() && m.id == after_id)
        .map_or(0, |i| i + 1);
    msgs[start..].to_vec()
}

fn texts(msgs: &[Message]) -> String {
    msgs.iter()
        .flat_map(|m| m.blocks())
        .filter_map(|b| b.as_text().map(|t| t.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_mcp_fails_the_chat_turn() {
    let ran = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&ran);
    let provider = AsyncProvider::wrap(move |req, _cx| {
        r.fetch_add(1, Ordering::SeqCst);
        async move { Ok(success(&req.agent_id, "")) }
    });
    let fx = Fx::with(
        Some(provider),
        TestOptions {
            mcp_addr: Some("127.0.0.1:-1".into()),
            ..TestOptions::default()
        },
    );
    let id = fx
        .eng()
        .start_workspace(&fx.proj, "chat only", "", AutonomyLevel::default())
        .await
        .unwrap();
    let reply = fx
        .send_mode(&id, "CH", "make me a plan", "", CHAT_MODE_PLAN)
        .await;
    assert!(reply.to_lowercase().contains("mcp"), "{reply}");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the turn ran text-only");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_session_prompt_skips_resume() {
    let c = numbered();
    let fx = Fx::new(Some(c.provider()));
    install_fresh_session_prompts(&fx);
    let ws = fx.workspace("fresh-review").await;
    fx.send(&ws.id, "PA", "hello", "sonnet").await;
    fx.send_mode(&ws.id, "PA", "/re-review", "sonnet", "review")
        .await;
    fx.send_mode(&ws.id, "PA", "/re-review", "sonnet", "review")
        .await;
    fx.send_mode(&ws.id, "PA", "look at auth", "sonnet", "review")
        .await;
    let turns = with_agent(&c, &format!("{}-PA", ws.id));
    assert_eq!(turns.len(), 4);
    let resumes: Vec<&str> = turns.iter().map(|t| t.resume_session.as_str()).collect();
    assert_eq!(resumes, ["", "", "", "sess-3"], "isolate each /re-review");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_session_turn_arg_skips_resume() {
    let c = numbered();
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("fresh-cta").await;
    fx.send(&ws.id, "PA", "hello", "sonnet").await;
    fx.eng()
        .send_workspace_turn(
            &ws.id,
            "PA",
            "WIRE review",
            "sonnet",
            None,
            &[],
            "review",
            "Re-review",
            true,
        )
        .await;
    let turns = with_agent(&c, &format!("{}-PA", ws.id));
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].resume_session, "");
    assert_eq!(turns[1].prompt, "WIRE review");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_session_skips_the_fork_inject() {
    let c = numbered();
    let fx = Fx::new(Some(c.provider()));
    install_fresh_session_prompts(&fx);
    let ws = fx.workspace("fresh-fork").await;
    fx.send(&ws.id, "PA", "first question", "sonnet").await;
    let tip = turn_message_id(&fx.eng().workspace_chat_items(&ws.id, "PA").await);
    assert!(!tip.is_empty(), "first turn footer missing message id");
    let tab = fx.eng().fork_workspace_chat(&ws.id, "PA", &tip).await;
    assert_eq!(tab.code, "PA2");
    fx.send_mode(&ws.id, "PA2", "/re-review", "sonnet", "review")
        .await;
    let specs = with_agent(&c, &format!("{}-PA2", ws.id));
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].resume_session, "");
    assert!(
        !specs[0].prompt.contains(FORK_PREAMBLE_LEAD),
        "{}",
        specs[0].prompt
    );
    assert!(specs[0].prompt.contains("WIRE"));
    fx.send_mode(&ws.id, "PA2", "explain finding 2", "sonnet", "review")
        .await;
    let specs = with_agent(&c, &format!("{}-PA2", ws.id));
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[1].resume_session, "sess-2");
    assert!(!specs[1].prompt.contains(FORK_PREAMBLE_LEAD));
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_after_an_isolated_pre_init_failure_stays_isolated() {
    let c = CaptureScript::new(|call| {
        if call == 1 {
            Vec::new()
        } else {
            crate::turn::test_support::maestro_fixture(&format!("sess-{}", call + 1), "ok")
        }
    });
    let fx = Fx::new(Some(c.provider()));
    install_fresh_session_prompts(&fx);
    let ws = fx.workspace("fresh-retry").await;
    fx.send(&ws.id, "PA", "hello", "sonnet").await;
    fx.send_mode(&ws.id, "PA", "/re-review", "sonnet", "review")
        .await;
    fx.eng()
        .send_workspace_message(
            &ws.id,
            "PA",
            "",
            "sonnet",
            None,
            &[],
            "review",
            CHAT_RETRY_DISPLAY_SENTINEL,
        )
        .await;
    let turns = with_agent(&c, &format!("{}-PA", ws.id));
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[1].resume_session, "");
    assert_eq!(turns[2].resume_session, "", "the retry stays isolated");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_session_cuts_native_history() {
    let c = numbered();
    let fx = Fx::new(Some(c.provider()));
    install_fresh_session_prompts(&fx);
    let ws = fx.workspace("fresh-native").await;
    fx.send(&ws.id, "PA", "hello from last week", "sonnet")
        .await;
    fx.send_mode(&ws.id, "PA", "/re-review", "sonnet", "review")
        .await;
    fx.send_mode(&ws.id, "PA", "look at auth", "sonnet", "review")
        .await;
    let turns = with_agent(&c, &format!("{}-PA", ws.id));
    assert_eq!(turns.len(), 3);
    let cutoff = &turns[1].history_after_id;
    assert!(!cutoff.is_empty(), "isolated /re-review has no cutoff");
    assert_eq!(&turns[2].history_after_id, cutoff);
    let msgs = fx
        .te
        .store
        .session_transcript(&turns[1].recur_session_id)
        .unwrap()
        .unwrap_or_default();
    let replay = texts(&after_cutoff(&msgs, cutoff));
    assert!(!replay.contains("hello from last week"), "{replay}");
    assert!(
        replay.contains("WIRE") && replay.contains("look at auth"),
        "{replay}"
    );
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    assert!(
        items
            .iter()
            .any(|it| it.role == "user" && it.text.contains("hello from last week")),
        "the visible transcript keeps the old turn"
    );
    let tip = turn_message_id(&items);
    let tab = fx.eng().fork_workspace_chat(&ws.id, "PA", &tip).await;
    assert!(!tab.code.is_empty());
    let child = fx
        .te
        .store
        .session_for_loop_agent(&ws.id, &tab.code)
        .expect("fork session");
    assert_eq!(
        native_history_after_id(&child.config_json),
        "",
        "a fork keeps the copied history"
    );
    fx.te.close().await;
}

/// A skill under the worktree's `.agents/skills` (`$HOME` is
/// process-global).
fn write_skill(worktree: &str, name: &str, body: &str) {
    write(
        &std::path::Path::new(worktree)
            .join(".agents")
            .join("skills")
            .join(name)
            .join("SKILL.md"),
        body,
    );
}

const GO_REVIEW: &str = "---\nname: recur-t-go-code-review\ndescription: Review Go changes.\n---\n# Go review\n\nCheck error handling.\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_skill_wrap_keeps_the_composer_as_display_text() {
    let c = capture_same("skill-sess", "Reviewed.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("scratch").await;
    write_skill(&ws.worktree_path, "recur-t-go-code-review", GO_REVIEW);
    let composer = "/recur-t-go-code-review look at auth";
    fx.send(&ws.id, "", composer, "sonnet").await;
    let s0 = coder_turns(&c).remove(0);
    assert!(
        s0.prompt
            .contains(r#"<skill_content name="recur-t-go-code-review">"#),
        "{}",
        s0.prompt
    );
    assert!(requires_display_text(&s0.prompt));
    assert_eq!(s0.display_text, composer);
    let sess = fx
        .te
        .store
        .session_for_loop_agent(&ws.id, PAIR_AGENT_CODE)
        .expect("chat session");
    let msgs = fx
        .te
        .store
        .session_transcript(&sess.id)
        .unwrap()
        .unwrap_or_default();
    let p = msgs[0].blocks()[0].as_text().expect("text block");
    assert_eq!(p.text, s0.prompt, "stored text is the wire");
    assert_eq!(p.display_text, composer);
    let items = fx.eng().workspace_chat_items(&ws.id, "").await;
    assert!(
        items[0].role == "user" && items[0].text == composer,
        "{items:?}"
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adjacent_skills_all_wrap() {
    let bodies = [
        ("recur-t-solidity-security", "SOLIDITY BODY"),
        ("recur-t-smart-contract-audit", "AUDIT BODY"),
        ("recur-t-go-code-review", "GO REVIEW BODY"),
    ];
    for want in [2, 3] {
        let c = capture_same("adj-skill-sess", "Reviewed.");
        let fx = Fx::new(Some(c.provider()));
        let ws = fx.workspace("scratch").await;
        for (name, body) in bodies {
            write_skill(
                &ws.worktree_path,
                name,
                &format!("---\nname: {name}\ndescription: {name}.\n---\n{body}\n"),
            );
        }
        let mut composer = "review vs origin/main".to_owned();
        for (name, _) in &bodies[..want] {
            composer.push_str(&format!(" /{name}"));
        }
        fx.send(&ws.id, "", &composer, "sonnet").await;
        let s0 = coder_turns(&c).remove(0);
        for (i, (name, body)) in bodies.iter().enumerate() {
            let wrapped = s0
                .prompt
                .contains(&format!(r#"<skill_content name="{name}">"#));
            let has_body = s0.prompt.contains(body);
            if i < want {
                assert!(wrapped && has_body, "{want}: missing {name}: {}", s0.prompt);
            } else {
                assert!(!wrapped && !has_body, "{want}: injected {name}");
            }
        }
        let items = fx.eng().workspace_chat_items(&ws.id, "").await;
        assert!(items[0].role == "user" && items[0].text == composer);
        fx.te.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_replays_the_stored_skill_wire() {
    let c = capture_same("skill-retry", "Reviewed.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("scratch").await;
    write_skill(&ws.worktree_path, "recur-t-go-code-review", GO_REVIEW);
    fx.send(&ws.id, "", "/recur-t-go-code-review look at auth", "sonnet")
        .await;
    fx.eng()
        .send_workspace_message(
            &ws.id,
            "",
            "should-not-be-used",
            "sonnet",
            None,
            &[],
            "",
            CHAT_RETRY_DISPLAY_SENTINEL,
        )
        .await;
    let coders = coder_turns(&c);
    assert!(coders.len() >= 2);
    assert_eq!(coders[1].prompt, coders[0].prompt, "the stored wire");
    assert!(
        coders[1]
            .prompt
            .contains("<skill_content name=\"recur-t-go-code-review\">")
    );
    assert!(coders[1].skip_record_user);
    let users = fx
        .items(&ws.id, "")
        .await
        .iter()
        .filter(|it| it.role == "user" && it.kind == "text")
        .count();
    assert_eq!(users, 1);
    fx.te.close().await;
}

fn simplify_setting(fx: &Fx, on: bool) {
    let mut set = fx.te.store.settings();
    set.simplify = Some(on);
    fx.te.store.set_settings(set);
}

async fn dock_simplify(fx: &Fx, ws: &str, code: &str, on: bool) {
    fx.eng()
        .remember_workspace_chat_selection(ws, code, "sonnet", None, CHAT_MODE_AGENT, Some(on))
        .await;
}

fn simplified(req: &recur_agent::TurnRequest) -> bool {
    req.append_system_prompt.contains("SIMPLIFY MODE")
}

#[tokio::test]
async fn simplify_follows_settings_session_and_mode() {
    // Settings off, Agent: no inject.
    let c = capture_same("sm-off", "ok");
    let fx = Fx::new(Some(c.provider()));
    simplify_setting(&fx, false);
    let ws = fx.workspace("scratch").await;
    fx.send_mode(&ws.id, "", "edit the file", "", CHAT_MODE_AGENT)
        .await;
    assert!(!simplified(&coder_turns(&c)[0]));
    fx.te.close().await;

    // Default on, Plan: injected.
    let c = capture_same("sm-plan", "ok");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("scratch").await;
    fx.send_mode(&ws.id, "", "plan a cache", "", CHAT_MODE_PLAN)
        .await;
    assert!(simplified(&coder_turns(&c)[0]));
    fx.te.close().await;

    // The session chip beats Settings, both ways.
    for (setting, chip) in [(true, false), (false, true)] {
        let c = capture_same("sm-sess", "ok");
        let fx = Fx::new(Some(c.provider()));
        simplify_setting(&fx, setting);
        let ws = fx.workspace("scratch").await;
        fx.eng().ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "");
        dock_simplify(&fx, &ws.id, PAIR_AGENT_CODE, chip).await;
        fx.send_mode(&ws.id, "", "edit the file", "", CHAT_MODE_AGENT)
            .await;
        assert_eq!(simplified(&coder_turns(&c)[0]), chip, "settings {setting}");
        fx.te.close().await;
    }
}

#[tokio::test]
async fn simplify_tabs_are_independent() {
    let c = capture_same("sm-tabs", "ok");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("scratch").await;
    fx.eng().ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "");
    dock_simplify(&fx, &ws.id, PAIR_AGENT_CODE, false).await;
    let tab2 = fx.eng().create_workspace_chat(&ws.id).await;
    assert!(!tab2.code.is_empty());
    fx.send_mode(&ws.id, PAIR_AGENT_CODE, "edit a", "", CHAT_MODE_AGENT)
        .await;
    fx.send_mode(&ws.id, &tab2.code, "edit b", "", CHAT_MODE_AGENT)
        .await;
    let specs = coder_turns(&c);
    assert!(!simplified(&specs[0]), "PA chip off");
    assert!(simplified(&specs[1]), "PA2 follows the default");
    fx.te.close().await;
}

#[tokio::test]
async fn a_settings_change_does_not_flip_a_started_chat() {
    let c = capture_same("sm-sticky", "ok");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("scratch").await;
    fx.send_mode(&ws.id, "", "first", "", CHAT_MODE_AGENT).await;
    simplify_setting(&fx, false);
    fx.send_mode(&ws.id, "", "second", "", CHAT_MODE_AGENT)
        .await;
    assert!(
        simplified(&coder_turns(&c)[1]),
        "stamped on at the first send"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn remember_simplify_does_not_resurrect_a_closed_tab() {
    let fx = Fx::new(Some(capture_same("sm-close", "ok").provider()));
    let ws = fx.workspace("scratch").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    let tab2 = eng.create_workspace_chat(&ws.id).await;
    eng.remember_workspace_chat_simplify(&ws.id, &tab2.code, false);
    eng.close_workspace_chat(&ws.id, &tab2.code).await;
    assert!(fx.te.store.agent(&ws.id, &tab2.code).is_none());
    eng.remember_workspace_chat_simplify(&ws.id, &tab2.code, true);
    assert!(fx.te.store.agent(&ws.id, &tab2.code).is_none());
    eng.remember_chat_simplify(&ws.id, &tab2.code, true);
    assert!(
        fx.te.store.agent(&ws.id, &tab2.code).is_none(),
        "the persist after Close must not recreate the row"
    );
    eng.remember_workspace_chat_simplify("no-such-ws", "PA", true);
    assert!(fx.te.store.agent("no-such-ws", "PA").is_none());
    eng.stamp_maestro_simplify_if_unset(&ws.id);
    eng.remember_workspace_chat_simplify(&ws.id, MAESTRO_AGENT_CODE, false);
    assert_eq!(eng.chat_simplify(&ws.id, MAESTRO_AGENT_CODE), Some(false));
    fx.te.close().await;
}

#[tokio::test]
async fn plan_and_agent_chats_attach_linear() {
    for (mode, url) in [
        (CHAT_MODE_PLAN, LINEAR_MCP_READ_ONLY_URL),
        (CHAT_MODE_AGENT, LINEAR_MCP_WRITE_URL),
    ] {
        let c = capture_same("lin", "OK.");
        let fx = Fx::with(
            Some(c.provider()),
            TestOptions {
                trackers: linear_trackers(),
                ..TestOptions::default()
            },
        );
        enable_linear_mcp(&fx.te.store);
        let ws = fx.workspace("lin").await;
        fx.send_mode(&ws.id, "PA", "work on ACME-1", "sonnet", mode)
            .await;
        let s = coder_turns(&c).remove(0);
        assert_eq!(s.user_mcp.len(), 1, "{mode}: {:?}", s.user_mcp);
        assert_eq!(s.user_mcp[0].url, url, "{mode}");
        assert!(has_tool(&s, "mcp__linear__*"), "{mode}");
        fx.te.close().await;
    }
}

#[tokio::test]
async fn a_failed_chat_error_note_owns_the_footer() {
    let fx = Fx::new(Some(duration_error()));
    let ws = fx.workspace("scratch").await;
    let out = fx.send(&ws.id, "PA", "long task", "sonnet").await;
    assert!(out.starts_with(CHAT_AGENT_ERROR_PREFIX), "{out}");
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    let footers: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.kind == "turn")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(footers.len(), 1, "{items:?}");
    let f = &items[footers[0]];
    assert_eq!(f.duration_ms, 1500);
    assert_eq!((f.input_tokens, f.cache_read_tokens), (200, 15000));
    let error_at = items
        .iter()
        .rposition(|it| it.kind == "text" && it.text.starts_with(CHAT_AGENT_ERROR_PREFIX))
        .expect("error note");
    assert_eq!(footers[0], error_at + 1);
    let sess = fx
        .te
        .store
        .session_for_loop_agent(&ws.id, "PA")
        .expect("session");
    let msgs = fx
        .te
        .store
        .session_transcript(&sess.id)
        .unwrap()
        .unwrap_or_default();
    assert!(
        msgs.iter()
            .any(|m| m.input_tokens == 200 && m.duration_ms == 0)
    );
    fx.te.close().await;
}

#[tokio::test]
async fn a_failed_chat_result_does_not_duplicate_the_footer() {
    let fx = Fx::new(Some(result_then_error()));
    let ws = fx.workspace("scratch").await;
    let out = fx.send(&ws.id, "PA", "long task", "sonnet").await;
    assert!(out.starts_with(CHAT_AGENT_ERROR_PREFIX), "{out}");
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    assert_eq!(items.iter().filter(|it| it.kind == "turn").count(), 1);
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_retry_does_not_inherit_the_prior_footer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let n = Arc::clone(&calls);
    let provider = AsyncProvider::wrap(move |_req, cx| {
        let first = n.fetch_add(1, Ordering::SeqCst) == 0;
        async move {
            if first {
                let res = TurnResult {
                    duration_ms: 800,
                    session_id: "thread-1".into(),
                    ..TurnResult::default()
                };
                cx.sink.emit(AgentEvent::Text {
                    text: "first reply".into(),
                    uuid: String::new(),
                    parent: String::new(),
                });
                cx.sink.emit(AgentEvent::TurnEnd(res.clone()));
                return Ok(res);
            }
            cx.cancel.cancelled().await;
            Err(ProviderError::canceled().with_partial(TurnResult {
                duration_ms: 400,
                session_id: "thread-1".into(),
                ..TurnResult::default()
            }))
        }
    });
    let fx = Fx::new(Some(provider));
    let ws = fx.workspace("scratch").await;
    assert_eq!(
        fx.send(&ws.id, "PA", "long task", "sonnet").await,
        "first reply"
    );
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let retry = tokio::spawn(async move {
        eng.send_workspace_message(
            &id,
            "PA",
            "",
            "sonnet",
            None,
            &[],
            "",
            CHAT_RETRY_DISPLAY_SENTINEL,
        )
        .await
    });
    let (st, id) = (Arc::clone(&fx.te.store), ws.id.clone());
    wait_for(Duration::from_secs(3), "chat agent working", || {
        st.agent(&id, "PA").is_some_and(|a| a.status == "working")
    })
    .await;
    assert!(fx.eng().cancel_chat(&ws.id, "PA").await);
    assert_eq!(
        join_hang(retry, "retry after stop").await,
        CHAT_STOPPED_REPLY
    );
    let items = fx.eng().workspace_chat_items(&ws.id, "PA").await;
    let footers: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.kind == "turn")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(footers.len(), 2, "prior turn + this Stop: {items:?}");
    let stop_at = items
        .iter()
        .rposition(|it| it.kind == "text" && it.text == CHAT_STOPPED_REPLY)
        .expect("stop note");
    assert_eq!(*footers.last().unwrap(), stop_at + 1);
    fx.te.close().await;
}

#[tokio::test]
async fn a_settled_loop_tab_is_not_locked_by_a_sibling_loop() {
    let fx = Fx::new(Some(capture_same("settled-sess", "OK.").provider()));
    let ws = fx.workspace("scratch").await;
    fx.eng().stamp_chat_loop(&ws.id, "PA", "", "", "", "", true);
    let owner = fx.eng().create_workspace_chat(&ws.id).await;
    fx.te.store.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.loop_chat_code = owner.code.clone();
        w.state = WorkspaceState::Queued;
    });
    let lock = fx
        .send_mode(&ws.id, &owner.code, "edit something", "", CHAT_MODE_AGENT)
        .await;
    assert!(lock.contains("loop is running"), "{lock}");
    let free = fx
        .send_mode(&ws.id, "PA", "edit something", "", CHAT_MODE_AGENT)
        .await;
    assert!(!free.contains("loop is running"), "{free}");
    fx.te.close().await;
}

fn hold_loop_slots(fx: &Fx) -> usize {
    let mut slots = 0;
    while fx.eng().governor().try_acquire_loop() {
        slots += 1;
    }
    slots
}

fn release_loop_slots(fx: &Fx, slots: usize) {
    for _ in 0..slots {
        fx.eng().governor().release_loop();
    }
}

#[tokio::test]
async fn a_loop_mode_retry_neither_engages_nor_bypasses_the_run_lock() {
    let fx = Fx::new(Some(capture_same("retry-loop", "OK.").provider()));
    let ws = fx.workspace("scratch").await;
    let retry = |fx: &Fx, ws: String| {
        let eng = Arc::clone(fx.eng());
        async move {
            eng.send_workspace_message(
                &ws,
                "PA",
                "ignored",
                "sonnet",
                None,
                &[],
                CHAT_MODE_LOOP,
                CHAT_RETRY_DISPLAY_SENTINEL,
            )
            .await
        }
    };
    fx.send_mode(&ws.id, "PA", "do the thing", "sonnet", CHAT_MODE_AGENT)
        .await;
    retry(&fx, ws.id.clone()).await;
    assert!(
        !fx.ws(&ws.id).has_run(),
        "a loop-mode retry must not engage"
    );
    let slots = hold_loop_slots(&fx);
    let reply = fx
        .send_mode(&ws.id, "PA", "ship dark mode", "", CHAT_MODE_LOOP)
        .await;
    assert!(
        !reply.contains("Follow along") && !reply.contains("I couldn't engage"),
        "{reply}"
    );
    let lock = retry(&fx, ws.id.clone()).await;
    assert!(lock.contains("loop is running"), "{lock}");
    release_loop_slots(&fx, slots);
    fx.te.close().await;
}

#[tokio::test]
async fn closing_a_loop_then_a_new_review_fix_is_fresh() {
    let fx = Fx::new(Some(capture_same("reengage", "OK.").provider()));
    let ws = fx.workspace("scratch").await;
    let slots = hold_loop_slots(&fx);
    let eng = fx.eng();
    let review = |code: String| {
        let eng = Arc::clone(eng);
        let id = ws.id.clone();
        async move {
            eng.send_workspace_message(
                &id,
                &code,
                "review the worktree",
                "",
                None,
                &[],
                CHAT_MODE_LOOP,
                "Review & fix",
            )
            .await
        }
    };
    review("PA".into()).await;
    let co = fx
        .te
        .store
        .create_session(Session {
            workspace_id: ws.id.clone(),
            kind: "coder".into(),
            provider: PROVIDER_CLAUDE_CODE.into(),
            config_json: r#"{"code":"CO","name":"Coder","ownerCode":"PA"}"#.into(),
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
    let tabs = eng.close_workspace_chat(&ws.id, "PA").await;
    assert!(
        tabs.len() == 1 && tabs[0].code != "MA" && tabs[0].code != "PA",
        "{tabs:?}"
    );
    let fresh = tabs[0].code.clone();
    assert_eq!(fx.ws(&ws.id).loop_chat_code, "");
    assert!(
        fx.te
            .store
            .session_for_chat_agent(&ws.id, "PA", "CO")
            .is_some()
    );
    let closed = eng.list_closed_workspace_chats(&ws.id);
    assert_eq!(tab_codes(&closed), ["PA"]);
    assert_eq!(review(fresh.clone()).await, "");
    let items = fx.items(&ws.id, &fresh).await;
    assert!(
        items.len() == 1 && items[0].role == "user" && items[0].text == "Review & fix",
        "{items:?}"
    );
    let old = fx.items(&ws.id, "PA").await;
    assert!(old.len() == 1 && old[0].text == "Review & fix", "{old:?}");
    assert_eq!(fx.ws(&ws.id).loop_chat_code, fresh);
    assert!(
        fx.te
            .store
            .session_for_chat_agent(&ws.id, &fresh, "CO")
            .is_none(),
        "the new tab inherited the old loop's CO session"
    );
    release_loop_slots(&fx, slots);
    fx.te.close().await;
}

fn inbox_has(fx: &Fx, id: &str) -> bool {
    fx.te
        .store
        .inbox()
        .unwrap_or_default()
        .iter()
        .any(|i| i.id == id)
}

#[tokio::test]
async fn close_publishes_attention_without_an_inbox_row() {
    let fx = Fx::new(None);
    let ws = fx.workspace("close-unseen").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    eng.ensure_pair_agent(&ws.id, "PA2", "");
    eng.ask_user_broker().park(
        UserQuestion {
            workspace_id: ws.id.clone(),
            agent_code: "PA2".into(),
            question: "which?".into(),
            ..UserQuestion::default()
        },
        None,
    );
    fx.te.store.update_agent(&ws.id, "PA2", |a| a.unseen = true);
    assert!(!inbox_has(&fx, &chat_inbox_id(&ws.id, "PA2")));
    let before = fx.te.events.count("attention.updated");
    eng.close_workspace_chat(&ws.id, "PA2").await;
    assert_eq!(fx.te.events.count("attention.updated") - before, 1);
    fx.te.close().await;
}

#[tokio::test]
async fn close_clears_the_inbox_row() {
    let fx = Fx::new(None);
    let ws = fx.workspace("close-inbox").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    eng.ensure_pair_agent(&ws.id, "PA2", "");
    eng.update_agent_status(&ws.id, "PA2", "working");
    eng.update_agent_status(&ws.id, "PA2", "idle");
    let id = chat_inbox_id(&ws.id, "PA2");
    assert!(inbox_has(&fx, &id));
    eng.close_workspace_chat(&ws.id, "PA2").await;
    assert!(!inbox_has(&fx, &id), "close left the inbox row");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_finished_racing_close_leaves_no_orphan_row() {
    let fx = Fx::new(None);
    let ws = fx.workspace("loop-close-race").await;
    let eng = fx.eng();
    eng.list_workspace_chats(&ws.id);
    let st = &fx.te.store;
    st.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.loop_chat_code = MAESTRO_AGENT_CODE.into();
    });
    eng.set_workspace_chat_focus("", "");
    for i in 0..50 {
        eng.ensure_maestro_agent_with_status(&ws.id, &ws.goal, "idle");
        st.update_workspace(&ws.id, |w| w.loop_chat_code = MAESTRO_AGENT_CODE.into());
        st.update_agent(&ws.id, MAESTRO_AGENT_CODE, |a| a.unseen = false);
        eng.clear_chat_inbox(&ws.id, MAESTRO_AGENT_CODE, false);
        let h = LoopHandle::new(
            &ws.id,
            &fx.proj,
            &ws.id,
            LoopState {
                chat_code: MAESTRO_AGENT_CODE.into(),
                exit_state: WorkspaceState::Paused,
                exit_phase: "idle".into(),
                ..LoopState::default()
            },
        );
        let start = CancellationToken::new();
        let (e1, s1) = (Arc::clone(eng), start.clone());
        let flag = tokio::spawn(async move {
            s1.cancelled().await;
            e1.maybe_flag_loop_finished(&h).await;
        });
        let (e2, s2, id) = (Arc::clone(eng), start.clone(), ws.id.clone());
        let close = tokio::spawn(async move {
            s2.cancelled().await;
            e2.close_workspace_chat(&id, MAESTRO_AGENT_CODE).await;
        });
        start.cancel();
        flag.await.unwrap();
        close.await.unwrap();
        if st.agent(&ws.id, MAESTRO_AGENT_CODE).is_some() {
            continue;
        }
        assert!(
            !inbox_has(&fx, &chat_inbox_id(&ws.id, MAESTRO_AGENT_CODE)),
            "iter {i}: a closed MA left an inbox row"
        );
    }
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mode_switch_remount_keeps_the_original_request() {
    let c = capture("remount-hist", "OK.");
    let started = CancellationToken::new();
    let fx = Fx::new(Some(hold_first_runner(&c, started.clone(), Duration::ZERO)));
    let ws = fx.workspace("remount-hist").await;
    let png = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0],
    );
    let orig = "plan this screenshot";
    let (eng, id) = (Arc::clone(fx.eng()), ws.id.clone());
    let text = format!("{orig} {}", attachment_marker("shot1"));
    let first = tokio::spawn(async move {
        eng.send_workspace_message(
            &id,
            "PA",
            &text,
            "sonnet",
            None,
            &[ChatAttachment {
                id: "shot1".into(),
                name: "shot.png".into(),
                mime: "image/png".into(),
                data: png,
            }],
            CHAT_MODE_ASK,
            "",
        )
        .await
    });
    within_hang(&started, "first turn starts").await;
    let msg = fx
        .eng()
        .switch_chat_mode(&ws.id, "PA", CHAT_MODE_PLAN)
        .await;
    assert!(msg.contains("remounting"), "{msg}");
    join_hang(first, "first turn after the park").await;
    let cont = wait_coder_turns(&c, 2).await.remove(1);
    assert!(cont.skip_record_user);
    assert!(!cont.recur_session_id.is_empty());
    let msgs = fx
        .te
        .store
        .session_transcript(&cont.recur_session_id)
        .unwrap()
        .unwrap_or_default();
    let replay = after_cutoff(&msgs, &cont.history_after_id);
    let all = format!("{}\n{}", texts(&replay), cont.prompt);
    assert_eq!(all.matches(orig).count(), 1, "{all}");
    assert!(
        replay
            .iter()
            .flat_map(|m| m.blocks())
            .any(|b| b.r#type == BLOCK_IMAGE),
        "the original image is in the replayed history"
    );
    assert!(cont.prompt.contains("from ask to plan"), "{}", cont.prompt);
    fx.te.close().await;
}
