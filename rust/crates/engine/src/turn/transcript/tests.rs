//! Go `workspace_chat_items_test.go` and the flatten / reconstruct cases of
//! `attachments_test.go`.

use std::sync::Arc;

use recur_agent::TurnRequest;
use recur_agent::UserImage;
use recur_agent::attach::{marker, user_content_blocks};
use recur_domain::{AgentRole, COMPACT_DONE, ContentBlock, Message};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::codes::{CHAT_RETRY_DISPLAY_SENTINEL, PAIR_AGENT_CODE};
use crate::roles::CODER_FIX_DISPLAY;
use crate::testing::{TestEngine, add_project, init_repo};
use crate::turn::AgentSpec;
use crate::turn::test_support::{claude, script};

fn user(blocks: Vec<ContentBlock>) -> Message {
    Message {
        role: "user".into(),
        content: Some(blocks),
        ..Message::default()
    }
}

fn assistant(blocks: Vec<ContentBlock>) -> Message {
    Message {
        role: "assistant".into(),
        content: Some(blocks),
        ..Message::default()
    }
}

fn flat(msgs: &[Message], model: &str) -> Vec<ChatItem> {
    let reg = ProviderRegistry::new(vec![claude()]);
    flatten_transcript(&reg, msgs, model, &Default::default())
}

fn img(id: &str) -> ContentBlock {
    ContentBlock::image(
        id,
        &format!("{id}.png"),
        "image/png",
        &format!("workspaces/W/attachments/{id}.png"),
    )
}

#[tokio::test]
async fn workspace_chat_items_reconstruct_the_turn() {
    let lines = vec![
        r#"{"type":"system","subtype":"init","session_id":"ws-tool-1","model":"claude-sonnet","mcp_servers":[]}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Let me look at the diff first.","signature":"sig-abc"}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"gh pr diff 187","description":"Fetch the PR diff"}}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"diff --git a/x b/x"}],"is_error":false}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The PR looks good overall."}]}}"#,
        r#"{"type":"result","subtype":"success","session_id":"ws-tool-1","duration_ms":900,"is_error":false,"num_turns":1,"result":"The PR looks good overall.","total_cost_usd":0.05,"usage":{"input_tokens":200,"output_tokens":50,"cache_creation_input_tokens":0,"cache_read_input_tokens":15000},"modelUsage":{"claude-sonnet":{"contextWindow":200000,"inputTokens":200,"outputTokens":50,"cacheReadInputTokens":15000,"cacheCreationInputTokens":0}}}"#,
    ];
    let te = TestEngine::new(Some(script(lines)));
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te
        .engine
        .create_workspace(&proj, "scratch", "")
        .await
        .unwrap();
    te.engine
        .run_agent(
            &CancellationToken::new(),
            AgentSpec::new(TurnRequest {
                workspace_id: ws.id.clone(),
                agent_id: format!("{}-PA", ws.id),
                role: AgentRole::Coder,
                model: "sonnet".into(),
                prompt: "review pr 187".into(),
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap();
    let items = te.engine.workspace_chat_items(&ws.id, "").await;
    assert_eq!(items.len(), 5, "{items:#?}");
    assert_eq!(
        (
            items[0].role.as_str(),
            items[0].kind.as_str(),
            items[0].text.as_str()
        ),
        ("user", "text", "review pr 187")
    );
    assert!(items[0].ts > 0);
    assert_eq!(
        (items[1].kind.as_str(), items[1].text.as_str()),
        ("thinking", "Let me look at the diff first.")
    );
    let tool = &items[2];
    assert_eq!((tool.kind.as_str(), tool.tool.as_str()), ("tool", "Bash"));
    assert!(!tool.tool_input.is_empty());
    assert_eq!(tool.description, "Fetch the PR diff");
    assert!(tool.input.contains("gh pr diff 187"));
    assert_eq!(tool.result, "diff --git a/x b/x");
    assert!(!tool.result_error && !tool.result_pending);
    assert_eq!(
        (items[3].role.as_str(), items[3].text.as_str()),
        ("assistant", "The PR looks good overall.")
    );
    let turn = &items[4];
    assert_eq!(turn.kind, "turn");
    assert_eq!(turn.duration_ms, 900);
    assert!(!turn.message_id.is_empty());
    assert_eq!(
        (
            turn.input_tokens,
            turn.cache_read_tokens,
            turn.context_window
        ),
        (200, 15000, 1_000_000)
    );
    assert!(te.engine.workspace_chat_items("nope", "").await.is_empty());
    te.close().await;
}

#[test]
fn compact_item() {
    let items = flat(
        &[assistant(vec![
            ContentBlock::compact(COMPACT_DONE, "auto", "older turns folded", 167_219),
            ContentBlock::text("continuing"),
        ])],
        "claude-code/sonnet",
    );
    assert_eq!(items[0].kind, "compact");
    assert_eq!(items[0].tokens, 167_219);
    assert_eq!(items[0].description, "auto");
    assert_eq!(items[0].text, "older turns folded");
}

#[test]
fn steer_with_tool_result_shows_display() {
    let items = flat(
        &[
            assistant(vec![ContentBlock::tool_use(
                "int-bash",
                "toolu_1",
                "Bash",
                r#"{"command":"npm i"}"#,
            )]),
            user(vec![
                ContentBlock::text_display(
                    "The operator is looking at this Recur workspace.\n\nuse pnpm",
                    "use pnpm",
                ),
                ContentBlock::tool_result("int-bash", "ok", false),
            ]),
        ],
        "claude-code/sonnet",
    );
    let u = items
        .iter()
        .find(|i| i.role == "user" && i.kind == "text")
        .unwrap();
    assert_eq!(u.text, "use pnpm");
}

#[test]
fn nests_task_subagent_tools() {
    let items = flat(
        &[
            assistant(vec![
                ContentBlock::tool_use(
                    "int-task",
                    "toolu_task_1",
                    "Task",
                    r#"{"description":"Map UI","subagent_type":"Explore","prompt":"Search frontend"}"#,
                ),
                ContentBlock::tool_use_nested(
                    "int-grep",
                    "t_nested",
                    "toolu_task_1",
                    "Grep",
                    r#"{"pattern":"PRDetail"}"#,
                ),
            ]),
            user(vec![ContentBlock::tool_result(
                "int-grep",
                "src/PR.svelte",
                false,
            )]),
        ],
        "",
    );
    assert!(
        items.iter().all(|i| i.tool != "Grep"),
        "nested rows never leak"
    );
    let task = items.iter().find(|i| i.tool == "Task").unwrap();
    assert_eq!(task.children.len(), 1);
    assert_eq!(task.children[0].tool, "Grep");
    assert!(!task.children[0].result_pending);
    assert!(task.children[0].result.contains("PR.svelte"));
}

#[test]
fn nests_subagent_output_under_agent() {
    let items = flat(
        &[
            assistant(vec![
                ContentBlock::tool_use(
                    "tu-agent",
                    "toolu_agent",
                    "Agent",
                    r#"{"description":"Explore the repo","prompt":"look around"}"#,
                ),
                ContentBlock::tool_use("tu-grep", "toolu_grep", "Grep", r#"{"pattern":"foo"}"#)
                    .with_parent("toolu_agent"),
                ContentBlock::text("found it in foo.go").with_parent("toolu_agent"),
            ]),
            user(vec![
                ContentBlock::tool_result("tu-grep", "foo.go:1: foo", false)
                    .with_parent("toolu_agent"),
                ContentBlock::tool_result("tu-agent", "explored the repo", false),
            ]),
        ],
        "",
    );
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0].kind.as_str(), items[0].tool.as_str()),
        ("tool", "Task")
    );
    assert_eq!(items[0].tool_id, "toolu_agent");
    assert_eq!(items[0].children.len(), 2);
    assert_eq!(items[0].children[0].tool, "Grep");
    assert_eq!(items[0].children[0].result, "foo.go:1: foo");
    assert_eq!(
        (
            items[0].children[1].kind.as_str(),
            items[0].children[1].text.as_str()
        ),
        ("text", "found it in foo.go")
    );
}

#[test]
fn drops_orphan_nested_blocks() {
    let items = flat(
        &[assistant(vec![
            ContentBlock::tool_use("tu-grep", "toolu_grep", "Grep", r#"{"pattern":"foo"}"#)
                .with_parent("toolu_missing"),
            ContentBlock::text("parent agent reply"),
        ])],
        "",
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, "text");
}

#[tokio::test]
async fn workspace_chat_items_nest_subagent_output() {
    let lines = vec![
        r#"{"type":"system","subtype":"init","session_id":"ws-sa-1","model":"claude-sonnet","mcp_servers":[]}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_agent","name":"Agent","input":{"description":"Explore","prompt":"look around","subagent_type":"Explore"}}]}}"#,
        r#"{"type":"assistant","parent_tool_use_id":"toolu_agent","message":{"content":[{"type":"tool_use","id":"toolu_grep","name":"Grep","input":{"pattern":"foo"}}]}}"#,
        r#"{"type":"user","parent_tool_use_id":"toolu_agent","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_grep","content":[{"type":"text","text":"foo.go:1"}],"is_error":false}]}}"#,
        r#"{"type":"assistant","parent_tool_use_id":"toolu_agent","message":{"content":[{"type":"text","text":"Found foo.go"}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_agent","content":[{"type":"text","text":"explored"}],"is_error":false}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The subagent finished."}]}}"#,
        r#"{"type":"result","subtype":"success","session_id":"ws-sa-1","duration_ms":400,"is_error":false,"num_turns":1,"result":"The subagent finished.","total_cost_usd":0.01}"#,
    ];
    let te = TestEngine::new(Some(script(lines)));
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te
        .engine
        .create_workspace(&proj, "scratch", "")
        .await
        .unwrap();
    te.engine
        .run_agent(
            &CancellationToken::new(),
            AgentSpec::new(TurnRequest {
                workspace_id: ws.id.clone(),
                agent_id: format!("{}-PA", ws.id),
                role: AgentRole::Coder,
                model: "sonnet".into(),
                prompt: "explore foo".into(),
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap();
    let items = content_items(&te.engine.workspace_chat_items(&ws.id, "").await);
    assert!(items.iter().all(|i| i.tool != "Grep"));
    assert!(items.iter().all(|i| i.text != "Found foo.go"));
    let agent = items
        .iter()
        .find(|i| i.kind == "tool" && i.tool == "Task")
        .unwrap();
    assert!(agent.children.len() >= 2);
    assert_eq!(agent.children[0].tool, "Grep");
    assert_eq!(agent.children[0].result, "foo.go:1");
    assert!(
        agent
            .children
            .iter()
            .any(|c| c.kind == "text" && c.text == "Found foo.go")
    );
    te.close().await;
}

#[test]
fn fallback_item_uses_model_labels() {
    let items = flat(
        &[assistant(vec![
            ContentBlock::fallback("claude-fable-5", "claude-opus-4-8"),
            ContentBlock::text("continuing"),
        ])],
        "claude-code/fable",
    );
    assert_eq!(
        (
            items[0].kind.as_str(),
            items[0].text.as_str(),
            items[0].description.as_str()
        ),
        ("fallback", "Opus 4.8", "Fable 5")
    );
    assert_eq!(
        (items[1].kind.as_str(), items[1].text.as_str()),
        ("text", "continuing")
    );
}

#[test]
fn finished_transcripts_do_not_spin() {
    let mut m = assistant(vec![ContentBlock::tool_use("i", "p", "Bash", "{}")]);
    m.stream_status = "complete".into();
    assert!(
        !flat(&[m.clone()], "")[0].result_pending,
        "crash left no result"
    );
    m.stream_status = "streaming".into();
    assert!(
        flat(&[m.clone()], "")[0].result_pending,
        "live turn keeps its spinner"
    );
    m.stream_status.clear();
    assert!(flat(&[m], "")[0].result_pending, "legacy rows stay pending");
}

// ---- flatten_user_message / reconstruct (attachments_test.go)

#[test]
fn flatten_restores_markers() {
    let it = flatten_user_message(&user(vec![
        ContentBlock::text("hi "),
        img("x"),
        ContentBlock::text(" there"),
    ]))
    .unwrap();
    assert_eq!(it.text, format!("hi {} there", marker("x")));
    assert_eq!(it.attachments.len(), 1);
    assert_eq!(it.attachments[0].url, "/workspaces/W/attachments/x.png");
}

#[test]
fn flatten_collapses_coder_dispatch() {
    let wire = "You are the Coder agent. A reviewer found issues in pong.\n\nCall mcp__recur__update_finding …";
    let it = flatten_user_message(&user(vec![ContentBlock::text(wire)])).unwrap();
    assert_eq!(it.text, CODER_FIX_DISPLAY);
}

#[test]
fn flatten_prefers_display_text() {
    let it = flatten_user_message(&user(vec![
        ContentBlock::text_display("WIRE before ", "shown before "),
        img("x"),
        ContentBlock::text_display(" WIRE after", " shown after"),
    ]))
    .unwrap();
    assert_eq!(it.text, format!("shown before {} shown after", marker("x")));
    assert_eq!(it.attachments[0].id, "x");
}

#[test]
fn flatten_skips_wire_after_display_text() {
    let m = marker("shot");
    let it = flatten_user_message(&user(vec![
        ContentBlock::text_display(
            &format!("{FORK_PREAMBLE_LEAD}\n<transcript>\nplease help.\n"),
            &format!("please help.\n{m}\ncues from the inbox"),
        ),
        img("shot"),
        ContentBlock::text(
            " I liked the labels\n<tool_use name=\"Read\"><result>roles.go</result></tool_use>",
        ),
    ]))
    .unwrap();
    for leak in [FORK_PREAMBLE_LEAD, "<transcript>", "tool_use", "roles.go"] {
        assert!(!it.text.contains(leak), "{}", it.text);
    }
    assert!(it.text.contains("please help.") && it.text.contains("cues from the inbox"));
    assert_eq!(it.text.matches(&m).count(), 1);
}

fn shot() -> Vec<UserImage> {
    vec![UserImage {
        id: "shot".into(),
        name: "shot.png".into(),
        mime: "image/png".into(),
        path: "artifacts/W/attachments/shot.png".into(),
        ..UserImage::default()
    }]
}

#[test]
fn foreign_attachment_markers_stay_text() {
    let m = marker("shot");
    let composer = format!("please continue {m} thanks");
    let preamble = format!(
        "{FORK_PREAMBLE_LEAD} Continue from the <transcript> below.\n\n<transcript>\n<message role=\"assistant\"><tool_use name=\"Read\"><result><![CDATA[var re = \"⟦attachment:other⟧\"]]></result></tool_use></message>\n</transcript>\n"
    );
    let blocks = user_content_blocks(&format!("{preamble}{composer}"), &composer, &shot());
    let it = flatten_user_message(&user(blocks)).unwrap();
    assert_eq!(it.text, composer);
    assert_eq!(it.attachments.len(), 1);
}

#[test]
fn strips_fork_preamble_without_display() {
    let wire = format!(
        "{FORK_PREAMBLE_LEAD} Continue from the <transcript> below.\n\n<transcript>\n<message role=\"user\"><text>old</text></message>\n</transcript>\nplease continue"
    );
    let it = flatten_user_message(&user(vec![ContentBlock::text(&wire)])).unwrap();
    assert_eq!(it.text, "please continue");
}

#[test]
fn image_first_fork_hides_preamble() {
    let data = tempfile::tempdir().unwrap();
    let p = data.path().join("artifacts/W/attachments/shot.png");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"png").unwrap();
    let m = marker("shot");
    let preamble = format!(
        "{FORK_PREAMBLE_LEAD} Continue from the <transcript> below.\n\n<transcript>\n<message role=\"assistant\"><text>old reply</text></message>\n</transcript>\n"
    );
    for composer in [m.clone(), format!("{m} cues from the inbox")] {
        let blocks = user_content_blocks(&format!("{preamble}{composer}"), &composer, &shot());
        let msg = user(blocks);
        let it = flatten_user_message(&msg).unwrap();
        for leak in [FORK_PREAMBLE_LEAD, "<transcript>", "old reply"] {
            assert!(!it.text.contains(leak), "{}", it.text);
        }
        assert_eq!(it.text, composer);
        let got = reconstruct_chat_prompt(data.path(), &msg).unwrap();
        assert!(got.wire.contains(FORK_PREAMBLE_LEAD) && got.wire.contains("old reply"));
        assert_eq!(got.imgs.len(), 1);
    }
}

#[test]
fn hides_image_first_fork_without_display() {
    let m = marker("shot");
    let preamble = format!(
        "{FORK_PREAMBLE_LEAD} Continue from the <transcript> below.\n\n<transcript>\n<message role=\"assistant\"><text>old reply</text></message>\n</transcript>\n"
    );
    let it = flatten_user_message(&user(vec![
        ContentBlock::text_display(&preamble, ""),
        img("shot"),
        ContentBlock::text_display(" cues from the inbox", " cues from the inbox"),
    ]))
    .unwrap();
    assert_eq!(it.text, format!("{m} cues from the inbox"));
}

#[test]
fn keeps_quoted_fork_lead_after_image() {
    let it = flatten_user_message(&user(vec![
        ContentBlock::text(&format!("note: {FORK_PREAMBLE_LEAD}")),
        img("shot"),
        ContentBlock::text(" more after the screenshot"),
    ]))
    .unwrap();
    assert_eq!(
        it.text,
        format!(
            "note: {FORK_PREAMBLE_LEAD}{} more after the screenshot",
            marker("shot")
        )
    );
}

#[test]
fn skips_retry_and_mode_switch_rows() {
    assert!(
        flatten_user_message(&user(vec![ContentBlock::text_display(
            "do the thing",
            CHAT_RETRY_DISPLAY_SENTINEL
        )]))
        .is_none()
    );
    assert!(
        flatten_user_message(&user(vec![ContentBlock::text_display(
            "Switched to Plan mode from Agent. Continue …",
            "Switched to Plan: continuing"
        )]))
        .is_none()
    );
}

#[test]
fn reconstruct_omits_missing_images() {
    let data = tempfile::tempdir().unwrap();
    let p = reconstruct_chat_prompt(
        data.path(),
        &user(vec![
            ContentBlock::text("look at this"),
            ContentBlock::image(
                "gone",
                "x.png",
                "image/png",
                "workspaces/W/attachments/missing.png",
            ),
            ContentBlock::text(" please"),
        ]),
    )
    .unwrap();
    assert_eq!(p.wire, "look at this please");
    assert!(p.imgs.is_empty());
    assert!(
        reconstruct_chat_prompt(
            data.path(),
            &user(vec![ContentBlock::image(
                "gone",
                "x.png",
                "image/png",
                "workspaces/W/attachments/missing.png"
            )])
        )
        .is_none()
    );
}

#[tokio::test]
async fn last_chat_prompt_does_not_fall_through() {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te
        .engine
        .create_workspace(&proj, "scratch", "")
        .await
        .unwrap();
    te.engine
        .ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "cursor/composer-2.5");
    let sess = te
        .engine
        .ensure_agent_session(&ws.id, "coder", PAIR_AGENT_CODE, "Agent", "cursor")
        .unwrap();
    te.store
        .append_message(&sess.id, user(vec![ContentBlock::text("older prompt")]))
        .unwrap();
    assert_eq!(
        te.engine
            .last_chat_user_prompt(&ws.id, PAIR_AGENT_CODE)
            .unwrap()
            .wire,
        "older prompt"
    );
    te.store
        .append_message(
            &sess.id,
            user(vec![ContentBlock::image(
                "gone",
                "x.png",
                "image/png",
                "workspaces/W/attachments/missing.png",
            )]),
        )
        .unwrap();
    assert!(
        te.engine
            .last_chat_user_prompt(&ws.id, PAIR_AGENT_CODE)
            .is_none(),
        "the newest prompt is refused, not an older one retried"
    );
    assert_eq!(
        te.engine
            .first_chat_user_prompt(&ws.id, PAIR_AGENT_CODE)
            .unwrap()
            .wire,
        "older prompt"
    );
    te.close().await;
}

#[tokio::test]
async fn loop_goal_images_prefer_the_recorded_set() {
    let te = TestEngine::new(None);
    crate::turn::test_support::add_solo_workspace(&te.engine, "W1");
    let png = "iVBORw0KGgo=";
    let (_, imgs) = crate::attachments::prepare_chat_attachments(
        &te.data_dir,
        "W1",
        "x",
        &[recur_domain::ChatAttachment {
            id: "img1".into(),
            mime: "image/png".into(),
            data: png.into(),
            ..Default::default()
        }],
    )
    .unwrap();
    assert_eq!(imgs.len(), 1);
    te.engine.ensure_pair_agent("W1", "PA2", "");
    let sess = te
        .engine
        .ensure_agent_session("W1", "coder", "PA2", "Agent", "")
        .unwrap();
    // No recorded set and no prompt: nothing.
    assert!(te.engine.chat_loop_goal_images("W1", "PA2").is_none());
    assert!(te.engine.loop_goal_images("W1", "PA2").is_empty());
    te.store
        .patch_session_config_merge(&sess.id, r#"{"goalImages":["img1"]}"#)
        .unwrap();
    let got = te.engine.loop_goal_images("W1", "PA2");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "img1");
    te.store
        .patch_session_config_merge(&sess.id, r#"{"goalImages":[]}"#)
        .unwrap();
    assert!(
        te.engine.loop_goal_images("W1", "PA2").is_empty(),
        "an empty recorded set never falls back to a shared paste"
    );
    let _ = Arc::clone(&te.engine);
    te.close().await;
}

#[test]
fn context_window_rules() {
    let p = claude();
    let m = |model: &str, used: i64, stored: i64| Message {
        model_id: model.into(),
        input_tokens: used,
        context_window: stored,
        ..Message::default()
    };
    let none = Default::default();
    assert_eq!(
        context_window_for_message(Some(p.as_ref()), &m("", 0, 0), "", &none),
        0
    );
    assert_eq!(
        context_window_for_message(Some(p.as_ref()), &m("sonnet", 5, 200_000), "", &none),
        1_000_000,
        "the live window wins for a known model"
    );
    assert_eq!(
        context_window_for_message(Some(p.as_ref()), &m("", 5, 123), "", &none),
        123,
        "no model and no context flag: the stored window"
    );
    let ctx = recur_agent::Flags::from([("context".to_owned(), "1m".to_owned())]);
    assert_eq!(
        context_window_for_message(Some(p.as_ref()), &m("", 5, 123), "", &ctx),
        1_000_000
    );
    assert_eq!(
        context_window_for_message(None, &m("x", 5, 77), "", &none),
        77
    );
    // The dock's context flag beats the provider ceiling stamped on the row.
    let k200 = recur_agent::Flags::from([("context".to_owned(), "200k".to_owned())]);
    assert_eq!(
        context_window_for_message(
            Some(p.as_ref()),
            &m("claude-sonnet-4-6", 32_000, 1_000_000),
            "",
            &k200
        ),
        200_000
    );
    assert_eq!(
        context_window_for_message(
            Some(p.as_ref()),
            &m("claude-sonnet-4-6", 32_000, 1_000_000),
            "",
            &ctx
        ),
        1_000_000
    );
    // Always-1M Sonnet 5 through the session model when the row has none.
    let k300 = recur_agent::Flags::from([("context".to_owned(), "300k".to_owned())]);
    assert_eq!(
        context_window_for_message(
            Some(p.as_ref()),
            &m("", 10_000, 200_000),
            "claude-code/sonnet",
            &k300
        ),
        1_000_000
    );
}

// ---- the provider-neutral flatten cases of Go `cursortools_test.go`.

#[test]
fn truncation_survives_the_transcript() {
    let msgs = [
        assistant(vec![ContentBlock::tool_use(
            "t1",
            "call-1",
            "Read",
            r#"{"file_path":"/wt/hello.txt"}"#,
        )]),
        user(vec![ContentBlock::truncated_tool_result(
            "t1", "alpha\n", false, true,
        )]),
    ];
    let items = flat(&msgs, "");
    let tool = items.iter().find(|i| i.kind == "tool").expect("tool item");
    assert!(tool.result_truncated, "truncation lost on reload");
}

#[test]
fn transcript_clears_pending_on_empty_result() {
    for result in ["", "{}", r#"{"status":"success","value":{}}"#, "alpha"] {
        let msgs = [
            assistant(vec![ContentBlock::tool_use("t1", "c", "Bash", "{}")]),
            user(vec![ContentBlock::tool_result("t1", result, false)]),
        ];
        let items = flat(&msgs, "");
        let tool = items.iter().find(|i| i.kind == "tool").expect("tool item");
        assert!(!tool.result_pending, "result {result:?} left the call pending");
    }
}

#[test]
fn transcript_emits_turn_footer_with_duration() {
    let mut m = assistant(vec![ContentBlock::text("done")]);
    m.duration_ms = 8460;
    let items = flat(&[m], "");
    let last = items.last().expect("items");
    assert_eq!((last.kind.as_str(), last.duration_ms), ("turn", 8460));
}

#[test]
fn transcript_footer_only_at_turn_end() {
    let mut done = assistant(vec![ContentBlock::text("done")]);
    done.duration_ms = 1200;
    let msgs = [
        assistant(vec![ContentBlock::tool_use(
            "t1",
            "c1",
            "Bash",
            r#"{"command":"ls"}"#,
        )]),
        user(vec![ContentBlock::tool_result("t1", "ok", false)]),
        done,
    ];
    let footers = flat(&msgs, "").iter().filter(|i| i.kind == "turn").count();
    assert_eq!(footers, 1);
}

#[test]
fn transcript_no_footer_without_duration() {
    let items = flat(&[assistant(vec![ContentBlock::text("done")])], "");
    assert!(items.iter().all(|i| i.kind != "turn"));
}
