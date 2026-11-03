//! Chat fork tests: forking a tab at a turn (same workspace
//! or a sibling one, with the dirty tree), the fork-inject preamble and its
//! compaction watermark, dropping foreign resume handles on retry and dock
//! switches, and the one-shot `forkInject` flag.

use std::time::Duration;

use recur_agent::{TurnRequest, TurnResult};
use recur_domain::{
    COMPACT_DONE, ChatAttachment, ChatItem, ContentBlock, Message, PROVIDER_CLAUDE_CODE,
    PROVIDER_CODEX, PROVIDER_CURSOR, Pr, PrSource, SessionContext, Workspace,
};

use super::test_support::*;
use crate::chat::fork::fork_history_preamble;
use crate::codes::CHAT_RETRY_DISPLAY_SENTINEL;
use crate::session_config::AgentConfig;
use crate::testing::{git, wait_for};
use crate::turn::transcript::FORK_PREAMBLE_LEAD;

fn turn_message_id(items: &[ChatItem]) -> String {
    items
        .iter()
        .find(|it| it.kind == "turn")
        .map(|it| it.message_id.clone())
        .unwrap_or_default()
}

fn last_user(items: &[ChatItem]) -> String {
    items
        .iter()
        .rev()
        .find(|it| it.role == "user" && it.kind == "text")
        .map(|it| it.text.clone())
        .unwrap_or_default()
}

fn text_msg(id: &str, role: &str, text: &str) -> Message {
    Message {
        id: id.into(),
        role: role.into(),
        content: Some(vec![ContentBlock::text(text)]),
        ..Message::default()
    }
}

fn blocks_msg(id: &str, role: &str, blocks: Vec<ContentBlock>) -> Message {
    Message {
        id: id.into(),
        role: role.into(),
        content: Some(blocks),
        ..Message::default()
    }
}

fn fork_inject(fx: &Fx, ws: &str, code: &str) -> bool {
    let sess = fx
        .te
        .store
        .session_for_loop_agent(ws, code)
        .expect("session");
    AgentConfig::parse(&sess.config_json).fork_inject
}

fn req(prompt: &str) -> TurnRequest {
    TurnRequest {
        prompt: prompt.into(),
        model: "sonnet".into(),
        ..TurnRequest::default()
    }
}

#[tokio::test]
async fn a_fork_copies_the_transcript_and_isolates_sends() {
    let c = capture("sess", "reply");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("fork-chat").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "first question", "sonnet").await;
    let first_turn = turn_message_id(&eng.workspace_chat_items(&ws.id, "PA").await);
    assert!(!first_turn.is_empty());
    fx.send(&ws.id, "PA", "second question", "sonnet").await;
    assert!(fx.items(&ws.id, "PA").await.len() >= 4);
    let tab = eng.fork_workspace_chat(&ws.id, "PA", &first_turn).await;
    assert_eq!(tab.code, "PA2");
    assert!(!tab.title.is_empty() && tab.title != "New chat");
    let child = fx.items(&ws.id, "PA2").await;
    assert_eq!(child.len(), 2, "{child:?}");
    assert_eq!(child[0].text, "first question");
    assert!(
        eng.fork_workspace_chat(&ws.id, "PA", "no-such-message")
            .await
            .code
            .is_empty()
    );

    fx.send(&ws.id, "PA2", "branch from first", "sonnet").await;
    assert_eq!(
        last_user(&fx.items(&ws.id, "PA2").await),
        "branch from first"
    );
    assert_eq!(last_user(&fx.items(&ws.id, "PA").await), "second question");
    let specs = with_agent(&c, &format!("{}-PA2", ws.id));
    assert_eq!(specs.len(), 1);
    assert!(specs[0].resume_session.is_empty() && !specs[0].fork_session);
    let p = &specs[0].prompt;
    assert!(
        p.contains("<transcript>") && p.contains(r#"<message role="user">"#),
        "{p}"
    );
    assert!(p.contains("first question") && p.contains("branch from first"));
    assert_eq!(specs[0].display_text, "branch from first");
    fx.te.close().await;
}

#[tokio::test]
async fn a_tip_fork_injects_history_once() {
    let c = capture("sess", "reply");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("fork-tip").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "only turn", "sonnet").await;
    let tip = turn_message_id(&eng.workspace_chat_items(&ws.id, "PA").await);
    assert!(!tip.is_empty());
    let parent = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    assert_eq!(parent.provider, PROVIDER_CLAUDE_CODE);
    assert_eq!(
        eng.fork_workspace_chat(&ws.id, "PA", &tip).await.code,
        "PA2"
    );
    assert!(fork_inject(&fx, &ws.id, "PA2"));
    assert!(
        fx.te
            .store
            .agent(&ws.id, "PA2")
            .is_none_or(|a| a.session_id.is_empty())
    );
    fx.send(&ws.id, "PA2", "continue", "sonnet").await;
    let specs = with_agent(&c, &format!("{}-PA2", ws.id));
    assert_eq!(specs.len(), 1);
    assert!(specs[0].resume_session.is_empty() && !specs[0].fork_session);
    assert!(specs[0].prompt.contains("<transcript>") && specs[0].prompt.contains("only turn"));
    assert!(
        !fork_inject(&fx, &ws.id, "PA2"),
        "consumed after the preamble send"
    );
    fx.te.close().await;
}

const CURSOR_AGENT: &str = "agent-6eebccee-4bef-4e7f-a2f7-27e94087de8b";

#[tokio::test]
async fn a_cursor_handle_is_never_snapshotted_for_claude() {
    let c = capture("sess", "reply");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("fork-cursor-to-claude").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "only turn", "sonnet").await;
    let tip = turn_message_id(&eng.workspace_chat_items(&ws.id, "PA").await);
    eng.set_agent_session(&ws.id, "PA", CURSOR_AGENT);
    assert_eq!(
        eng.fork_workspace_chat(&ws.id, "PA", &tip).await.code,
        "PA2"
    );
    assert!(
        fx.te
            .store
            .agent(&ws.id, "PA2")
            .is_none_or(|a| a.session_id != CURSOR_AGENT)
    );
    assert!(fork_inject(&fx, &ws.id, "PA2"));
    fx.send(&ws.id, "PA2", "continue on claude", "sonnet").await;
    let specs = with_agent(&c, &format!("{}-PA2", ws.id));
    assert!(specs[0].resume_session.is_empty() && !specs[0].fork_session);
    assert!(specs[0].prompt.contains("<transcript>") && specs[0].prompt.contains("only turn"));
    assert!(!specs[0].prompt.contains(CURSOR_AGENT));
    fx.te.close().await;
}

#[tokio::test]
async fn retry_after_a_cursor_switch_injects_instead_of_resuming() {
    let c = capture("sess", "reply");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("retry-after-switch").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "do the thing", "sonnet").await;
    const ID: &str = "agent-6a931db4-d388-4fb7-afc3-b97178f2b82b";
    eng.set_agent_session(&ws.id, "PA", ID);
    eng.send_workspace_message(
        &ws.id,
        "PA",
        "",
        "sonnet",
        None,
        &[],
        "",
        CHAT_RETRY_DISPLAY_SENTINEL,
    )
    .await;
    let specs = with_agent(&c, &format!("{}-PA", ws.id));
    let last = specs.last().unwrap();
    assert!(last.resume_session.is_empty(), "{}", last.resume_session);
    assert!(!last.prompt.contains(ID));
    assert!(last.prompt.contains("<transcript>") && last.prompt.contains("do the thing"));
    assert!(
        fx.te
            .store
            .agent(&ws.id, "PA")
            .is_none_or(|a| a.session_id != ID)
    );
    fx.te.close().await;
}

#[tokio::test]
async fn retry_after_a_codex_switch_drops_the_uuid_resume() {
    let c = capture("sess", "reply");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("retry-after-codex").await;
    fx.send(&ws.id, "PA", "do the thing", "sonnet").await;
    fx.eng()
        .set_agent_session(&ws.id, "PA", "7b1c9f20-2c1a-4d3e-9a11-0f6e2b8c4d5a");
    let sess = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    fx.te
        .store
        .patch_session_config_merge(&sess.id, r#"{"forkInject":true}"#)
        .unwrap();
    fx.eng()
        .send_workspace_message(
            &ws.id,
            "PA",
            "",
            "sonnet",
            None,
            &[],
            "",
            CHAT_RETRY_DISPLAY_SENTINEL,
        )
        .await;
    let last = with_agent(&c, &format!("{}-PA", ws.id)).pop().unwrap();
    assert!(last.resume_session.is_empty());
    assert!(last.prompt.contains("<transcript>") && last.prompt.contains("do the thing"));
    fx.te.close().await;
}

#[tokio::test]
async fn a_retry_of_an_injected_wire_rides_stdin_and_consumes_the_flag() {
    let fx = Fx::new(Some(capture_same("sess-x", "ok").provider()));
    let ws = fx.workspace("retry-injected").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "only turn", "sonnet").await;
    let tip = turn_message_id(&eng.workspace_chat_items(&ws.id, "PA").await);
    assert_eq!(
        eng.fork_workspace_chat(&ws.id, "PA", &tip).await.code,
        "PA2"
    );
    let mut first = req("continue");
    assert!(eng.apply_chat_fork(&ws.id, "PA2", &mut first, false));
    assert!(first.prompt.starts_with(FORK_PREAMBLE_LEAD));
    let mut retry = req(&first.prompt);
    assert!(eng.apply_chat_fork(&ws.id, "PA2", &mut retry, false));
    assert_eq!(retry.prompt.matches(FORK_PREAMBLE_LEAD).count(), 1);
    assert!(retry.prompt_on_stdin);
    eng.finish_chat_fork(
        &ws.id,
        "PA2",
        true,
        &TurnResult {
            session_id: "sess-x".into(),
            ..TurnResult::default()
        },
    );
    assert!(!fork_inject(&fx, &ws.id, "PA2"));
    let mut next = TurnRequest {
        resume_session: "sess-x".into(),
        ..req("next")
    };
    assert!(!eng.apply_chat_fork(&ws.id, "PA2", &mut next, false));
    assert_eq!(next.resume_session, "sess-x");
    assert!(!next.prompt.contains(FORK_PREAMBLE_LEAD));
    fx.te.close().await;
}

#[tokio::test]
async fn an_image_send_on_a_fork_hides_the_injected_transcript() {
    let fx = Fx::new(Some(capture("sess", "reply").provider()));
    let ws = fx.workspace("fork-image").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "look at this screenshot", "sonnet")
        .await;
    let parent = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    let st = &fx.te.store;
    st.append_message(
        &parent.id,
        blocks_msg(
            "",
            "assistant",
            vec![ContentBlock::tool_use(
                "t1",
                "",
                "Read",
                r#"{"path":"attachments.go"}"#,
            )],
        ),
    )
    .unwrap();
    st.append_message(
        &parent.id,
        blocks_msg(
            "",
            "user",
            vec![ContentBlock::tool_result(
                "t1",
                r#"var re = "⟦attachment:other⟧""#,
                false,
            )],
        ),
    )
    .unwrap();
    let asst = st
        .append_message(&parent.id, text_msg("", "assistant", "done reading"))
        .unwrap();
    assert_eq!(
        eng.fork_workspace_chat(&ws.id, "PA", &asst.id).await.code,
        "PA2"
    );
    let png = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0],
    );
    let marker = recur_agent::attach::marker("shot1");
    eng.send_workspace_message(
        &ws.id,
        "PA2",
        &format!("this is better but still needs work {marker} cues from the inbox"),
        "sonnet",
        None,
        &[ChatAttachment {
            id: "shot1".into(),
            name: "shot.png".into(),
            mime: "image/png".into(),
            data: png,
        }],
        "",
        "",
    )
    .await;
    let got = last_user(&fx.items(&ws.id, "PA2").await);
    for leak in [
        FORK_PREAMBLE_LEAD,
        "<transcript>",
        "tool_use",
        "attachment:other",
    ] {
        assert!(!got.contains(leak), "bubble leaked {leak:?}: {got}");
    }
    assert!(
        got.contains("this is better but still needs work") && got.contains("cues from the inbox")
    );
    fx.te.close().await;
}

#[tokio::test]
async fn a_forked_cursor_chat_switched_to_claude_injects_history() {
    let c = capture("sess", "reply");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("fork-cursor-then-claude").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "only turn", "cursor/composer-2.5")
        .await;
    let provider = |code: &str| {
        fx.te
            .store
            .session_for_loop_agent(&ws.id, code)
            .unwrap()
            .provider
    };
    assert_eq!(provider("PA"), PROVIDER_CURSOR);
    let tip = turn_message_id(&eng.workspace_chat_items(&ws.id, "PA").await);
    eng.set_agent_session(&ws.id, "PA", CURSOR_AGENT);
    assert_eq!(
        eng.fork_workspace_chat(&ws.id, "PA", &tip).await.code,
        "PA2"
    );
    assert_eq!(provider("PA2"), PROVIDER_CURSOR);
    assert!(
        fx.te
            .store
            .agent(&ws.id, "PA2")
            .is_none_or(|a| a.session_id != CURSOR_AGENT)
    );
    eng.remember_workspace_chat_selection(&ws.id, "PA2", "sonnet", None, "agent", None)
        .await;
    assert_eq!(provider("PA2"), PROVIDER_CLAUDE_CODE);
    fx.send(&ws.id, "PA2", "continue on claude", "sonnet").await;
    let specs = with_agent(&c, &format!("{}-PA2", ws.id));
    assert_eq!(specs.len(), 1);
    assert!(specs[0].resume_session.is_empty() && !specs[0].fork_session);
    assert!(specs[0].prompt.contains("<transcript>") && specs[0].prompt.contains("only turn"));
    fx.te.close().await;
}

#[test]
fn the_preamble_keeps_tool_results_and_drops_thinking() {
    let got = fork_history_preamble(
        &[
            text_msg("", "user", "hello"),
            blocks_msg(
                "",
                "assistant",
                vec![
                    ContentBlock::thinking("look at the tree", "", 0),
                    ContentBlock::tool_use("t1", "", "Bash", r#"{"command":"ls"}"#),
                ],
            ),
            blocks_msg(
                "",
                "user",
                vec![ContentBlock::tool_result("t1", "huge diff", false)],
            ),
            text_msg("", "assistant", "hi back"),
        ],
        None,
        "",
    );
    assert!(got.contains("<transcript>") && got.contains("hello") && got.contains("hi back"));
    assert!(got.contains(r#"<tool_use name="Bash">"#));
    assert!(
        got.contains("<result><![CDATA[huge diff]]></result>"),
        "{got}"
    );
    for leak in [
        "id=",
        "tool_use_id=",
        "<tool_result",
        "<thinking",
        "look at the tree",
    ] {
        assert!(!got.contains(leak), "{leak}: {got}");
    }
}

#[test]
fn the_preamble_cuts_to_the_last_compact_summary() {
    let got = fork_history_preamble(
        &[
            text_msg("", "user", "ancient prompt"),
            blocks_msg(
                "",
                "assistant",
                vec![ContentBlock::tool_use(
                    "t0",
                    "",
                    "Read",
                    r#"{"path":"roles.go"}"#,
                )],
            ),
            blocks_msg(
                "",
                "user",
                vec![ContentBlock::tool_result(
                    "t0",
                    &"old file ".repeat(200),
                    false,
                )],
            ),
            blocks_msg(
                "",
                "assistant",
                vec![
                    ContentBlock::text("done with the old file"),
                    ContentBlock::compact(
                        COMPACT_DONE,
                        "auto",
                        "older turns folded into a short brief",
                        800_000,
                    ),
                ],
            ),
            text_msg("", "user", "keep going"),
            text_msg("", "assistant", "post-compact reply"),
        ],
        None,
        "",
    );
    assert!(got.contains("older turns folded into a short brief"));
    assert!(got.contains("keep going") && got.contains("post-compact reply"));
    for old in ["ancient prompt", "old file", "done with the old file"] {
        assert!(!got.contains(old), "{old}");
    }
}

fn ctx(summary: &str, watermark: &str) -> SessionContext {
    SessionContext {
        summary: summary.into(),
        last_compacted_message_id: watermark.into(),
        ..SessionContext::default()
    }
}

#[test]
fn the_preamble_uses_the_session_context() {
    let got = fork_history_preamble(
        &[
            text_msg("msg-old", "user", "ancient prompt"),
            text_msg("msg-compact", "assistant", "done with the old file"),
            text_msg("msg-live", "user", "keep going"),
            text_msg("msg-asst", "assistant", "post-compact reply"),
        ],
        Some(&ctx("older turns folded into a short brief", "msg-compact")),
        "",
    );
    assert!(got.contains("older turns folded into a short brief"));
    assert!(got.contains("keep going") && got.contains("post-compact reply"));
    assert!(!got.contains("ancient prompt") && !got.contains("done with the old file"));
}

#[test]
fn the_preamble_uses_the_last_compact_in_the_watermark_message() {
    let got = fork_history_preamble(
        &[
            text_msg("msg-old", "user", "ancient prompt"),
            blocks_msg(
                "msg-compact",
                "assistant",
                vec![
                    ContentBlock::text("after first compact"),
                    ContentBlock::compact(COMPACT_DONE, "auto", "first fold", 100_000),
                    ContentBlock::text("covered by the later compact"),
                    ContentBlock::compact(COMPACT_DONE, "auto", "stale second chip", 200_000),
                    ContentBlock::text("live tail in the same turn"),
                ],
            ),
            text_msg("msg-live", "user", "keep going"),
        ],
        Some(&ctx("latest fold covering both", "msg-compact")),
        "",
    );
    assert!(got.contains("latest fold covering both"));
    assert!(got.contains("live tail in the same turn") && got.contains("keep going"));
    for old in [
        "ancient prompt",
        "after first compact",
        "covered by the later compact",
        "first fold",
        "stale second chip",
    ] {
        assert!(!got.contains(old), "{old}");
    }
}

#[test]
fn the_preamble_keeps_the_tail_when_a_later_compact_has_no_summary() {
    let got = fork_history_preamble(
        &[
            text_msg("msg-old", "user", "ancient prompt"),
            blocks_msg(
                "msg-compact",
                "assistant",
                vec![
                    ContentBlock::compact(COMPACT_DONE, "auto", "first fold", 100_000),
                    ContentBlock::text("still live after empty second compact"),
                    ContentBlock::compact(COMPACT_DONE, "auto", "", 200_000),
                ],
            ),
            text_msg("msg-live", "user", "keep going"),
        ],
        Some(&ctx("first fold", "msg-compact")),
        "",
    );
    assert!(got.contains("first fold"));
    assert!(got.contains("still live after empty second compact") && got.contains("keep going"));
    assert!(!got.contains("ancient prompt"));
}

#[test]
fn the_preamble_keeps_history_when_a_compact_has_no_summary() {
    let got = fork_history_preamble(
        &[
            text_msg("", "user", "please read roles.go"),
            blocks_msg(
                "",
                "assistant",
                vec![
                    ContentBlock::compact(COMPACT_DONE, "auto", "", 167_219),
                    ContentBlock::text("ok"),
                ],
            ),
        ],
        None,
        "",
    );
    assert!(got.contains("please read roles.go"));
}

async fn fork_after_compaction(with_context: bool) {
    let c = capture("sess", "reply");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("fork-compact").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "ancient prompt", "cursor/composer-2.5")
        .await;
    let parent = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    let st = &fx.te.store;
    if with_context {
        let compacted = st
            .append_message(
                &parent.id,
                text_msg("", "assistant", "done with the old file"),
            )
            .unwrap();
        st.write_session_context(
            &parent.id,
            "older turns folded into a short brief",
            &compacted.id,
        )
        .unwrap();
    } else {
        st.append_message(
            &parent.id,
            blocks_msg(
                "",
                "assistant",
                vec![
                    ContentBlock::text("done with the old file"),
                    ContentBlock::compact(
                        COMPACT_DONE,
                        "auto",
                        "older turns folded into a short brief",
                        800_000,
                    ),
                ],
            ),
        )
        .unwrap();
    }
    st.append_message(&parent.id, text_msg("", "user", "keep going"))
        .unwrap();
    let post = st
        .append_message(&parent.id, text_msg("", "assistant", "post-compact reply"))
        .unwrap();
    assert_eq!(
        eng.fork_workspace_chat(&ws.id, "PA", &post.id).await.code,
        "PA2"
    );
    eng.remember_workspace_chat_selection(&ws.id, "PA2", "sonnet", None, "agent", None)
        .await;
    fx.send(&ws.id, "PA2", "continue on claude", "sonnet").await;
    let specs = with_agent(&c, &format!("{}-PA2", ws.id));
    assert_eq!(specs.len(), 1);
    let p = &specs[0].prompt;
    assert!(p.contains("older turns folded into a short brief"), "{p}");
    assert!(p.contains("keep going") && p.contains("post-compact reply"));
    assert!(!p.contains("ancient prompt") && !p.contains("done with the old file"));
    fx.te.close().await;
}

#[tokio::test]
async fn a_fork_send_injects_the_compact_summary_not_older_history() {
    fork_after_compaction(false).await;
}

#[tokio::test]
async fn a_fork_send_injects_the_session_context_not_older_history() {
    fork_after_compaction(true).await;
}

#[test]
fn the_preamble_clips_huge_tool_results() {
    let huge = "z".repeat(20 << 10);
    let got = fork_history_preamble(
        &[
            text_msg("", "user", "please read the file"),
            blocks_msg(
                "",
                "assistant",
                vec![ContentBlock::tool_use(
                    "t1",
                    "",
                    "Read",
                    r#"{"path":"big.go"}"#,
                )],
            ),
            blocks_msg(
                "",
                "user",
                vec![ContentBlock::tool_result("t1", &huge, false)],
            ),
            text_msg("", "assistant", "done reading"),
        ],
        None,
        "",
    );
    assert!(got.contains("please read the file") && got.contains("done reading"));
    assert!(!got.contains(&huge));
    assert!(got.contains("…(truncated)…"));
}

#[tokio::test]
async fn a_fork_to_a_new_workspace_copies_the_transcript() {
    let c = capture("sess", "reply");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("fork-to-ws").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "first question", "sonnet").await;
    fx.send(&ws.id, "PA", "second question", "sonnet").await;
    let first_turn = turn_message_id(&eng.workspace_chat_items(&ws.id, "PA").await);
    let dest = eng
        .fork_workspace_chat_to_workspace(&ws.id, "PA", &first_turn)
        .await
        .expect("fork to workspace");
    assert!(!dest.id.is_empty() && dest.id != ws.id);
    assert_eq!(dest.project_id, fx.proj);
    assert_eq!(dest.goal, "first question");
    assert_eq!(last_user(&fx.items(&ws.id, "PA").await), "second question");
    let child = fx.items(&dest.id, "PA").await;
    assert_eq!(child.len(), 2);
    assert_eq!(child[0].text, "first question");
    let before = fx.te.store.workspaces().len();
    assert!(
        eng.fork_workspace_chat_to_workspace(&ws.id, "PA", "no-such-message")
            .await
            .is_err()
    );
    assert_eq!(fx.te.store.workspaces().len(), before);
    let (st, id) = (std::sync::Arc::clone(&fx.te.store), dest.id.clone());
    wait_for(Duration::from_secs(5), "dest worktree", || {
        st.workspace(&id)
            .is_some_and(|w| !w.worktree_path.is_empty() && w.phase == "editing")
    })
    .await;
    fx.send(&dest.id, "PA", "branch elsewhere", "sonnet").await;
    assert_eq!(
        last_user(&fx.items(&dest.id, "PA").await),
        "branch elsewhere"
    );
    assert_eq!(last_user(&fx.items(&ws.id, "PA").await), "second question");
    let specs = with_agent(&c, &format!("{}-PA", dest.id));
    assert_eq!(specs.len(), 1);
    assert!(specs[0].resume_session.is_empty() && !specs[0].fork_session);
    assert!(specs[0].prompt.contains("<transcript>") && specs[0].prompt.contains("first question"));
    fx.te.close().await;
}

#[tokio::test]
async fn a_pr_chat_host_cannot_fork_into_a_new_worktree() {
    let fx = Fx::new(Some(capture_same("host-1", "ok").provider()));
    let st = &fx.te.store;
    st.add_pr(Pr {
        id: "gh-proj-3".into(),
        project_id: fx.proj.clone(),
        number: 3,
        source: PrSource::GitHub,
        title: "Docs".into(),
        branch: "docs".into(),
        base: "main".into(),
        group: "open".into(),
        state: "Open".into(),
        ..Pr::default()
    })
    .unwrap();
    let host = st
        .add_workspace(Workspace {
            id: "proj.pr-3".into(),
            project_id: fx.proj.clone(),
            r#type: recur_domain::WORKSPACE_SOLO.into(),
            host_pr_id: "gh-proj-3".into(),
            worktree_path: fx.repo.path.clone(),
            goal: "Docs".into(),
            ..Workspace::default()
        })
        .unwrap();
    fx.send_mode(&host.id, "PA", "what changed?", "sonnet", "review")
        .await;
    let tip = turn_message_id(&fx.eng().workspace_chat_items(&host.id, "PA").await);
    assert!(!tip.is_empty());
    let before = st.workspaces().len();
    let err = fx
        .eng()
        .fork_workspace_chat_to_workspace(&host.id, "PA", &tip)
        .await
        .expect_err("a host fork is refused");
    assert!(err.0.contains("create a workspace"), "{err}");
    assert_eq!(st.workspaces().len(), before);
    fx.te.close().await;
}

#[tokio::test]
async fn apply_chat_fork_injects_once() {
    let fx = Fx::new(Some(capture_same("sess-once", "ok").provider()));
    let ws = fx.workspace("fork-once").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "only turn", "sonnet").await;
    let tip = turn_message_id(&eng.workspace_chat_items(&ws.id, "PA").await);
    assert_eq!(
        eng.fork_workspace_chat(&ws.id, "PA", &tip).await.code,
        "PA2"
    );
    let mut first = req("first");
    eng.apply_chat_fork(&ws.id, "PA2", &mut first, false);
    assert!(first.prompt.contains("<transcript>") && first.prompt.contains("only turn"));
    assert!(first.prompt_on_stdin);
    assert!(
        fork_inject(&fx, &ws.id, "PA2"),
        "stays set until a session id lands"
    );
    let child = fx.te.store.session_for_loop_agent(&ws.id, "PA2").unwrap();
    fx.te
        .store
        .append_message(&child.id, text_msg("", "user", &first.prompt))
        .unwrap();
    let mut nested = req("again");
    eng.apply_chat_fork(&ws.id, "PA2", &mut nested, false);
    assert_eq!(nested.prompt.matches(FORK_PREAMBLE_LEAD).count(), 1);
    let ok = TurnResult {
        session_id: "sess-ok".into(),
        ..TurnResult::default()
    };
    eng.finish_chat_fork(&ws.id, "PA2", false, &ok);
    assert!(fork_inject(&fx, &ws.id, "PA2"));
    eng.finish_chat_fork(&ws.id, "PA2", true, &ok);
    assert!(!fork_inject(&fx, &ws.id, "PA2"));
    let mut second = req("second");
    eng.apply_chat_fork(&ws.id, "PA2", &mut second, false);
    assert_eq!(second.prompt, "second");
    fx.te.close().await;
}

#[tokio::test]
async fn resume_handles_fit_only_their_provider() {
    let fx = Fx::new(None);
    const CLAUDE: &str = "sess-1";
    const CODEX: &str = "0192f3c0-1111-7222-8333-444455556666";
    for (id, provider, want) in [
        ("", PROVIDER_CLAUDE_CODE, true),
        (CURSOR_AGENT, PROVIDER_CURSOR, true),
        (CURSOR_AGENT, PROVIDER_CLAUDE_CODE, false),
        (CURSOR_AGENT, PROVIDER_CODEX, false),
        (CLAUDE, PROVIDER_CLAUDE_CODE, true),
        (CLAUDE, PROVIDER_CURSOR, false),
        (CLAUDE, PROVIDER_CODEX, true),
        (CODEX, PROVIDER_CODEX, true),
        (CODEX, PROVIDER_CURSOR, false),
        (CLAUDE, "unknown-provider", false),
    ] {
        assert_eq!(
            fx.eng().resume_fits_provider(id, provider),
            want,
            "{id:?} on {provider}"
        );
    }
    fx.te.close().await;
}

#[tokio::test]
async fn a_claude_to_codex_pick_keeps_the_handle_and_marks_inject() {
    let c = capture_same("sess-switch", "ok");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("switch-codex").await;
    let eng = fx.eng();
    fx.send(&ws.id, "PA", "only turn", "sonnet").await;
    eng.set_agent_session(&ws.id, "PA", "sess-claude");
    eng.remember_workspace_chat_selection(&ws.id, "PA", "codex/gpt-5", None, "agent", None)
        .await;
    assert_eq!(
        fx.te.store.agent(&ws.id, "PA").unwrap().session_id,
        "sess-claude"
    );
    let sess = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    assert_eq!(sess.provider, PROVIDER_CODEX);
    assert!(fork_inject(&fx, &ws.id, "PA"));
    fx.send(&ws.id, "PA", "continue on codex", "codex/gpt-5")
        .await;
    let last = with_agent(&c, &format!("{}-PA", ws.id)).pop().unwrap();
    assert!(last.resume_session.is_empty());
    assert!(last.prompt.contains("<transcript>") && last.prompt.contains("only turn"));
    fx.te.close().await;
}

#[tokio::test]
async fn a_fork_to_a_new_workspace_copies_the_dirty_tree() {
    let fx = Fx::new(Some(capture_same("sess-dirty", "ok").provider()));
    let ws = fx.workspace("fork-dirty").await;
    assert!(!ws.worktree_path.is_empty());
    let wt = std::path::Path::new(&ws.worktree_path);
    std::fs::write(wt.join("committed.txt"), "on branch\n").unwrap();
    git(&ws.worktree_path, &["add", "committed.txt"]);
    git(&ws.worktree_path, &["commit", "-m", "branch work"]);
    std::fs::write(wt.join("dirty.txt"), "uncommitted\n").unwrap();
    fx.send(&ws.id, "PA", "edit the tree", "sonnet").await;
    let tip = turn_message_id(&fx.eng().workspace_chat_items(&ws.id, "PA").await);
    let src_base = ws.base_branch.clone();
    let dest = fx
        .eng()
        .fork_workspace_chat_to_workspace(&ws.id, "PA", &tip)
        .await
        .unwrap();
    let (st, id, base) = (
        std::sync::Arc::clone(&fx.te.store),
        dest.id.clone(),
        src_base.clone(),
    );
    wait_for(Duration::from_secs(5), "dest dirty overlay", || {
        let Some(got) = st.workspace(&id) else {
            return false;
        };
        if got.worktree_path.is_empty() || got.phase != "editing" {
            return false;
        }
        if !base.is_empty() && got.base_branch != base {
            return false;
        }
        std::path::Path::new(&got.worktree_path)
            .join("dirty.txt")
            .exists()
    })
    .await;
    let got = fx.ws(&dest.id);
    if !src_base.is_empty() {
        assert_eq!(got.base_branch, src_base);
    }
    let dwt = std::path::Path::new(&got.worktree_path);
    assert_eq!(
        std::fs::read_to_string(dwt.join("committed.txt")).unwrap(),
        "on branch\n"
    );
    assert_eq!(
        std::fs::read_to_string(dwt.join("dirty.txt")).unwrap(),
        "uncommitted\n"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn a_rename_keeps_the_provider_and_sets_no_fork_inject() {
    let fx = Fx::new(Some(capture_same("sess-rename", "ok").provider()));
    let mut set = fx.te.store.settings();
    set.model_agent = "cursor/composer-2.5".into();
    fx.te.store.set_settings(set);
    let ws = fx.workspace("rename-provider").await;
    fx.send(&ws.id, "PA", "first", "claude-code/sonnet").await;
    assert!(
        fx.eng()
            .rename_chat_session(&ws.id, "PA", "Titled")
            .contains("Titled")
    );
    assert_eq!(
        fx.te
            .store
            .session_for_loop_agent(&ws.id, "PA")
            .unwrap()
            .provider,
        PROVIDER_CLAUDE_CODE
    );
    fx.send(&ws.id, "PA", "follow-up", "claude-code/sonnet")
        .await;
    assert!(!fork_inject(&fx, &ws.id, "PA"));
    fx.te.close().await;
}
