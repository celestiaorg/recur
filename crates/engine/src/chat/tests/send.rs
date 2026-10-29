//! Port of the send and first-message cases of Go `workspace_chat_test.go`:
//! the coding-agent turn shape, flags and resume, the rename nudges and
//! provisional titles (incl. titles stamped ahead of a queued send).

use recur_agent_tools::names::mcp;
use recur_domain::{AgentRole, CHAT_MODE_AGENT, ChatAttachment, Flags};

use super::test_support::*;
use crate::chat_shared::FIRST_WORKSPACE_RENAME_PROMPT;
use crate::workspace::create::DEFAULT_WORKSPACE_TITLE;

#[tokio::test]
async fn send_workspace_message_runs_a_coding_turn() {
    let c = capture_same("ws-sess-1", "Edited the file.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("scratch").await;
    let reply = fx.send(&ws.id, "", "edit the file", "opus").await;
    assert!(reply.contains("Edited the file"), "reply {reply:?}");

    let s0 = c
        .with_role(AgentRole::Coder)
        .into_iter()
        .next()
        .expect("coder turn");
    assert!(
        s0.system_prompt.is_empty(),
        "must not override the provider prompt"
    );
    assert_eq!(s0.model, "opus");
    assert_eq!(s0.worktree.to_string_lossy(), ws.worktree_path);
    for t in [
        "Edit",
        "Write",
        mcp::LIST_FINDINGS,
        mcp::UPDATE_FINDING,
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
        mcp::LIST_TERMINALS,
        mcp::READ_TERMINAL,
        mcp::OPEN_PR,
        mcp::CREATE_WORKSPACE,
        mcp::MESSAGE_CHILD,
    ] {
        assert!(
            has_tool(&s0, t),
            "agent chat needs {t}: {:?}",
            s0.allowed_tools
        );
    }
    assert!(!s0.mcp_url.is_empty(), "agent chat wires the Recur MCP");
    for t in [mcp::ESCALATE, mcp::REPORT_FINDING, mcp::DONE] {
        assert!(!has_tool(&s0, t), "agent chat must not carry {t}");
    }
    let chat = fx.items(&ws.id, "").await;
    assert_eq!(chat.len(), 2, "{chat:?}");
    assert_eq!(
        (chat[0].role.as_str(), chat[0].text.as_str()),
        ("user", "edit the file")
    );
    assert_eq!(chat[1].role, "assistant");
    assert!(
        fx.te
            .store
            .last_assistant_text(&ws.id)
            .contains("Edited the file")
    );

    let flags: Flags = [("effort", "max"), ("context", "1m")]
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    fx.eng()
        .send_workspace_message(
            &ws.id,
            "",
            "with flags",
            "claude-code/sonnet",
            Some(&flags),
            &[],
            "",
            "",
        )
        .await;
    let turns = coder_turns(&c);
    let s1 = turns.last().unwrap();
    assert_eq!(s1.flags.get("effort").map(String::as_str), Some("max"));
    assert_eq!(s1.flags.get("context").map(String::as_str), Some("1m"));
    assert_eq!(
        s0.mcp_url, s1.mcp_url,
        "a follow-up reuses the agent's MCP URL"
    );
    assert!(
        !s1.refresh_standing,
        "an ordinary follow-up must not refresh standing notes"
    );
    let (model, sel_flags, _) = fx.eng().workspace_chat_selection(&ws.id, "");
    assert_eq!(model, "claude-code/sonnet");
    assert_eq!(sel_flags, flags);
    fx.te.close().await;
}

#[tokio::test]
async fn first_workspace_session_nudges_all_three_renames() {
    let c = capture("nudge", "OK.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("").await;
    assert_eq!(ws.goal, DEFAULT_WORKSPACE_TITLE);
    fx.send(&ws.id, "", "add a dark-mode toggle", "").await;
    assert_eq!(fx.ws(&ws.id).goal, "add a dark-mode toggle");
    let specs = with_agent(&c, &format!("{}-PA", ws.id));
    for want in [
        "first session in this workspace",
        "mcp__recur__rename_workspace",
        "mcp__recur__rename_branch",
        "mcp__recur__rename_session",
    ] {
        assert!(
            specs[0].append_system_prompt.contains(want),
            "missing {want}"
        );
    }
    fx.send(&ws.id, "", "and persist the preference", "").await;
    let specs = with_agent(&c, &format!("{}-PA", ws.id));
    assert!(!specs[1].append_system_prompt.contains("first session"));
    assert!(
        !specs[1]
            .append_system_prompt
            .contains("first message in this chat")
    );
    assert_eq!(fx.ws(&ws.id).goal, "add a dark-mode toggle");

    let labeled = fx.workspace("keep this name").await;
    fx.send(&labeled.id, "", "something else entirely", "")
        .await;
    assert_eq!(fx.ws(&labeled.id).goal, "keep this name");
    let named = with_agent(&c, &format!("{}-PA", labeled.id));
    let p = &named[0].append_system_prompt;
    assert!(!p.contains("rename_workspace") && !p.contains("first session in this workspace"));
    assert!(p.contains("mcp__recur__rename_session"));
    fx.te.close().await;
}

#[tokio::test]
async fn provisional_title_from_first_message_until_rename() {
    let c = capture_same("prov", "OK.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("").await;
    fx.send(
        &ws.id,
        "",
        "one two three four five six seven eight nine ten",
        "",
    )
    .await;
    assert_eq!(
        fx.ws(&ws.id).goal,
        "one two three four five six seven eight"
    );
    fx.eng().rename_workspace(&ws.id, "Dark mode");
    assert_eq!(fx.ws(&ws.id).goal, "Dark mode");
    fx.te.close().await;
}

#[tokio::test]
async fn provisional_titles_land_before_a_queued_send() {
    let fx = Fx::new(Some(capture("t", "OK.").provider()));
    let ws = fx.workspace("").await;
    fx.eng().list_workspace_chats(&ws.id);
    fx.eng()
        .remember_workspace_chat_selection(&ws.id, "PA", "", None, CHAT_MODE_AGENT, None)
        .await;
    let msg = "add a dark-mode toggle";
    fx.eng().apply_provisional_titles(&ws.id, "PA", msg);
    assert_eq!(fx.ws(&ws.id).goal, msg);
    let tabs = fx.eng().list_workspace_chats(&ws.id);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].title, msg);
    assert_eq!(
        fx.eng()
            .rename_nudge_prompt(&ws.id, "PA", CHAT_MODE_AGENT, msg),
        FIRST_WORKSPACE_RENAME_PROMPT
    );
    fx.te.close().await;
}

#[tokio::test]
async fn early_titles_from_a_removed_queued_message_are_rederived() {
    let c = capture_same("early", "OK.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("").await;
    fx.eng().list_workspace_chats(&ws.id);
    fx.eng()
        .remember_workspace_chat_selection(&ws.id, "PA", "", None, CHAT_MODE_AGENT, None)
        .await;
    fx.eng()
        .apply_provisional_titles(&ws.id, "PA", "queued then removed");
    fx.send(&ws.id, "PA", "the real first message", "").await;
    assert_eq!(fx.ws(&ws.id).goal, "the real first message");
    let tabs = fx.eng().list_workspace_chats(&ws.id);
    assert_eq!(tab_codes(&tabs), ["PA"]);
    assert_eq!(tabs[0].title, "the real first message");
    let specs = with_agent(&c, &format!("{}-PA", ws.id));
    assert!(
        specs[0]
            .append_system_prompt
            .contains("mcp__recur__rename_workspace")
    );
    fx.te.close().await;
}

#[tokio::test]
async fn provisional_title_strips_a_leading_attachment() {
    let fx = Fx::new(Some(capture_same("attach-title", "OK.").provider()));
    let ws = fx.workspace("").await;
    let png = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0],
    );
    let marker = recur_agent::attach::marker("img_mu1fovfk_9oabcd");
    fx.eng()
        .send_workspace_message(
            &ws.id,
            "",
            &format!("{marker} the sidebar chip leaked"),
            "",
            None,
            &[ChatAttachment {
                id: "img_mu1fovfk_9oabcd".into(),
                name: "shot.png".into(),
                mime: "image/png".into(),
                data: png,
            }],
            "",
            "",
        )
        .await;
    let got = fx.ws(&ws.id);
    assert!(!got.goal.contains("⟦attachment:") && !got.short.contains("⟦attachment:"));
    assert_eq!(got.goal, "the sidebar chip leaked");
    let tabs = fx.eng().list_workspace_chats(&ws.id);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].title, "the sidebar chip leaked");
    fx.te.close().await;
}

#[tokio::test]
async fn a_later_tabs_first_message_nudges_only_the_chat_rename() {
    let c = capture("tab", "OK.");
    let fx = Fx::new(Some(c.provider()));
    let ws = fx.workspace("named workspace").await;
    fx.send(&ws.id, "PA", "task A", "").await;
    let tab_b = fx.eng().create_workspace_chat(&ws.id).await;
    fx.send(&ws.id, &tab_b.code, "task B", "").await;
    let pa = with_agent(&c, &format!("{}-PA", ws.id));
    let p = &pa[0].append_system_prompt;
    assert!(!p.contains("rename_workspace") && !p.contains("first session in this workspace"));
    let pa2 = with_agent(&c, &format!("{}-{}", ws.id, tab_b.code));
    let got = &pa2[0].append_system_prompt;
    assert!(
        got.contains("first message in this chat") && got.contains("mcp__recur__rename_session")
    );
    assert!(!got.contains("rename_workspace") && !got.contains("first session in this workspace"));
    fx.te.close().await;
}

#[tokio::test]
async fn renamed_chat_and_workspace_show_on_the_strip() {
    let fx = Fx::new(Some(capture("rn", "OK.").provider()));
    let ws = fx.workspace("").await;
    fx.eng().rename_workspace(&ws.id, "Dark mode toggle");
    fx.eng().list_workspace_chats(&ws.id);
    let again = fx.eng().rename_chat_session(&ws.id, "PA", "Auth refresh");
    assert!(again.contains("Auth refresh"), "{again}");
    let tabs = fx.eng().list_workspace_chats(&ws.id);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].title, "Auth refresh");
    fx.te.close().await;
}

#[tokio::test]
async fn planner_rename_retitles_the_loop_tab_on_the_strip() {
    let fx = Fx::new(Some(capture("pl", "OK.").provider()));
    let ws = fx.workspace("scratch").await;
    fx.eng().list_workspace_chats(&ws.id);
    let tab2 = fx.eng().create_workspace_chat(&ws.id).await;
    assert_eq!(tab2.code, "PA2");
    let _one = walk(&fx.te, &ws.id, "PA", &ws.id);
    let msg = fx.eng().rename_chat_session(&ws.id, "PL", "Dark mode");
    assert!(msg.contains("Dark mode"), "{msg}");
    let titles: Vec<(String, String)> = fx
        .eng()
        .list_workspace_chats(&ws.id)
        .into_iter()
        .map(|t| (t.code, t.title))
        .collect();
    assert!(
        titles.contains(&("PA".into(), "Dark mode".into())),
        "{titles:?}"
    );
    assert!(
        !titles.contains(&("PA2".into(), "Dark mode".into())),
        "{titles:?}"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn listing_and_creating_tabs_keeps_agents_idle() {
    let fx = Fx::new(Some(capture("idle", "OK.").provider()));
    let ws = fx.workspace("idle-tabs").await;
    fx.eng().list_workspace_chats(&ws.id);
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().status, "idle");
    let tab = fx.eng().create_workspace_chat(&ws.id).await;
    assert_eq!(fx.te.store.agent(&ws.id, &tab.code).unwrap().status, "idle");
    fx.te.close().await;
}
