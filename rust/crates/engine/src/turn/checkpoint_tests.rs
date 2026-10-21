//! Go `checkpoint_test.go`. The send path's start/end snapshots are taken
//! here around `run_agent` the way E4's send does; the AutoResume reuse
//! cases go through `last_open_chat_prompt_id`.

use std::path::Path;

use recur_agent::TurnRequest;
use recur_agent::testing::ScriptedProvider;
use recur_domain::{AgentRole, ChatItem, ContentBlock, Message, Workspace};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::session_config::AgentConfig;
use crate::testing::{TestEngine, add_project, git_out, init_repo};
use crate::turn::AgentSpec;
use crate::turn::test_support::{claude, maestro_fixture};
use crate::turn::transcript::content_items;

fn q(id: &str) -> Message {
    Message {
        id: id.into(),
        role: "user".into(),
        content: Some(vec![ContentBlock::text("question")]),
        ..Message::default()
    }
}

#[tokio::test]
async fn mark_checkpoints_flags_changed_turns() {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let ws = Workspace {
        id: "mark".into(),
        worktree_path: repo.path.clone(),
        ..Workspace::default()
    };
    let idx = tempfile::tempdir().unwrap();
    let idx_s = idx.path().to_string_lossy().into_owned();
    let mut msgs = Vec::new();
    let mut items = Vec::new();
    let mut want = Vec::new();
    for (user, start, end, changed) in [
        ("changed", true, true, true),
        ("unchanged", true, true, false),
        ("missing-end", true, false, true),
        ("missing-start", false, true, true),
    ] {
        for (phase, take) in [("start", start), ("end", end)] {
            if phase == "end" && changed {
                std::fs::write(Path::new(&repo.path).join("edit.txt"), user).unwrap();
            }
            if take {
                recur_git::checkpoint(&repo.path, &idx_s, &checkpoint_ref(&ws.id, user, phase))
                    .await
                    .unwrap();
            }
        }
        msgs.push(q(user));
        msgs.push(Message {
            id: format!("{user}-tool"),
            role: "user".into(),
            content: Some(vec![ContentBlock {
                r#type: recur_domain::BLOCK_TOOL_RESULT.into(),
                ..ContentBlock::default()
            }]),
            ..Message::default()
        });
        for i in 0..2 {
            let footer = format!("{user}-footer{i}");
            msgs.push(Message {
                id: footer.clone(),
                role: "assistant".into(),
                duration_ms: 1,
                ..Message::default()
            });
            items.push(ChatItem {
                kind: "turn".into(),
                message_id: footer,
                ..ChatItem::default()
            });
            want.push(start && end && changed);
        }
    }
    items.push(ChatItem {
        kind: "text".into(),
        message_id: "changed-footer0".into(),
        ..ChatItem::default()
    });
    items.push(ChatItem {
        kind: "turn".into(),
        message_id: "unknown".into(),
        ..ChatItem::default()
    });
    want.extend([false, false]);
    te.engine.mark_checkpoints(&ws, &msgs, &mut items).await;
    let got: Vec<bool> = items.iter().map(|i| i.checkpoint).collect();
    assert_eq!(got, want);
    te.close().await;
}

/// One checkpointed PA turn, as the send path runs it.
async fn turn(te: &TestEngine, ws: &Workspace, prompt: &str) {
    let user_id = uuid::Uuid::now_v7().to_string();
    te.engine.take_checkpoint(ws, &user_id, "start").await;
    te.engine
        .run_agent(
            &CancellationToken::new(),
            AgentSpec::new(TurnRequest {
                workspace_id: ws.id.clone(),
                agent_id: format!("{}-PA", ws.id),
                role: AgentRole::Coder,
                model: "sonnet".into(),
                prompt: prompt.into(),
                user_message_id: user_id.clone(),
                worktree: ws.worktree_path.clone().into(),
                ..TurnRequest::default()
            }),
        )
        .await
        .unwrap();
    te.engine.take_checkpoint(ws, &user_id, "end").await;
}

fn editing(file: &'static str) -> std::sync::Arc<dyn recur_agent::Provider> {
    std::sync::Arc::new(
        recur_provider_claude::testing::NdjsonScript::new(maestro_fixture("sess-ckpt", "ok"))
            .with_edits([(file.to_owned(), "agent\n".to_owned())])
            .imitating(claude()),
    )
}

async fn footers(te: &TestEngine, ws: &str) -> Vec<ChatItem> {
    te.engine
        .workspace_chat_items(ws, "PA")
        .await
        .into_iter()
        .filter(|i| i.kind == "turn")
        .collect()
}

#[tokio::test]
async fn turn_diff_and_reset() {
    let te = TestEngine::new(Some(editing("turn1.txt")));
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te.engine.create_workspace(&proj, "ckpt", "").await.unwrap();
    std::fs::write(Path::new(&ws.worktree_path).join("mine.txt"), "user\n").unwrap();
    turn(&te, &ws, "first").await;
    te.engine.providers().pin(Some(editing("turn2.txt")));
    turn(&te, &ws, "second").await;
    let f = footers(&te, &ws.id).await;
    assert_eq!(f.len(), 2);
    assert!(f[0].checkpoint && f[1].checkpoint);
    let d = te
        .engine
        .workspace_chat_turn_diff(&ws.id, "PA", &f[1].message_id)
        .await
        .unwrap();
    let files = d.files.clone().unwrap_or_default();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "turn2.txt");
    assert_eq!(d.diff.add, 1);
    assert_eq!(d.hash.len(), 7);
    te.engine
        .reset_workspace_chat(&ws.id, "PA", &f[0].message_id)
        .await
        .unwrap();
    let wt = Path::new(&ws.worktree_path);
    assert!(!wt.join("turn2.txt").exists());
    assert!(wt.join("turn1.txt").exists() && wt.join("mine.txt").exists());
    assert_eq!(
        git_out(&ws.worktree_path, &["status", "--porcelain"]).unwrap(),
        "?? mine.txt\n?? turn1.txt\n"
    );
    let items = content_items(&te.engine.workspace_chat_items(&ws.id, "PA").await);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].text, "first");
    let sess = te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    assert!(AgentConfig::parse(&sess.config_json).fork_inject);
    assert_eq!(
        te.store
            .agent(&ws.id, "PA")
            .map(|a| a.session_id)
            .unwrap_or_default(),
        ""
    );
    te.close().await;
}

#[tokio::test]
async fn unedited_turn_hides_checkpoint_and_reset_keeps_manual_edits() {
    let te = TestEngine::new(Some(std::sync::Arc::new(
        recur_provider_claude::testing::NdjsonScript::new(maestro_fixture("sess-ckpt-empty", "ok"))
            .imitating(claude()),
    )));
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te
        .engine
        .create_workspace(&proj, "ckpt-empty", "")
        .await
        .unwrap();
    turn(&te, &ws, "first").await;
    turn(&te, &ws, "typo").await;
    let f = footers(&te, &ws.id).await;
    assert_eq!(f.len(), 2);
    assert!(f.iter().all(|i| !i.checkpoint));
    std::fs::write(Path::new(&ws.worktree_path).join("mine.txt"), "user\n").unwrap();
    te.engine
        .reset_workspace_chat(&ws.id, "PA", &f[0].message_id)
        .await
        .unwrap();
    assert!(Path::new(&ws.worktree_path).join("mine.txt").exists());
    let items = content_items(&te.engine.workspace_chat_items(&ws.id, "PA").await);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].text, "first");
    te.close().await;
}

#[tokio::test]
async fn reset_refusals() {
    let te = TestEngine::new(Some(std::sync::Arc::new(
        ScriptedProvider::new(Vec::new()).imitating(claude()),
    )));
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te.engine.create_workspace(&proj, "r", "").await.unwrap();
    assert_eq!(
        te.engine
            .reset_workspace_chat(&ws.id, "PA", "x")
            .await
            .unwrap_err()
            .0,
        "chat has no session"
    );
    te.engine.ensure_pair_agent_run(&ws.id, "PA", "sonnet");
    assert_eq!(
        te.engine
            .reset_workspace_chat(&ws.id, "PA", "x")
            .await
            .unwrap_err()
            .0,
        "stop the running turn first"
    );
    assert_eq!(
        te.engine
            .workspace_chat_turn_diff("nope", "PA", "x")
            .await
            .unwrap_err()
            .0,
        "workspace not found"
    );
    te.engine
        .ensure_maestro_agent_with_status(&ws.id, "", "idle");
    te.engine
        .ensure_agent_session(&ws.id, "maestro", "MA", "Maestro", "")
        .unwrap();
    let sess = te.store.session_for_loop_agent(&ws.id, "MA").unwrap();
    te.store.append_message(&sess.id, q("u1")).unwrap();
    let footer = te
        .store
        .append_message(
            &sess.id,
            Message {
                role: "assistant".into(),
                duration_ms: 5,
                ..Message::default()
            },
        )
        .unwrap();
    assert_eq!(
        te.engine
            .reset_workspace_chat(&ws.id, "MA", &footer.id)
            .await
            .unwrap_err()
            .0,
        "reset is not available on a running loop's chat"
    );
    te.close().await;
}

#[tokio::test]
async fn review_checkpoint_delta_only_on_the_reviewed_tab() {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te
        .engine
        .create_workspace(&proj, "review-ckpt", "")
        .await
        .unwrap();
    let none = (String::new(), String::new());
    assert_eq!(
        te.engine.review_checkpoint(&ws.id, "PA", "re-review").await,
        none
    );
    assert_eq!(
        te.engine.review_checkpoint(&ws.id, "PA", "review").await,
        none
    );
    assert_eq!(
        te.engine.review_checkpoint(&ws.id, "PA", "re-review").await,
        none,
        "no live provider session"
    );
    assert_eq!(
        te.engine.review_checkpoint(&ws.id, "PA", "other").await,
        none
    );
    te.engine.ensure_pair_agent(&ws.id, "PA", "sonnet");
    te.engine.set_agent_session(&ws.id, "PA", "sess-review");
    std::fs::write(Path::new(&ws.worktree_path).join("fix.txt"), "fixed\n").unwrap();
    let (since, now) = te.engine.review_checkpoint(&ws.id, "PA", "re-review").await;
    assert!(!since.is_empty() && !now.is_empty() && since != now);
    assert_eq!(
        git_out(&ws.worktree_path, &["diff", "--name-only", &since, &now]).unwrap(),
        "fix.txt\n"
    );
    assert_eq!(
        te.engine
            .review_checkpoint(&ws.id, "PA2", "re-review")
            .await
            .0,
        "",
        "another tab is a full pass"
    );
    te.close().await;
}

#[tokio::test]
async fn last_open_prompt_is_the_unfinished_checkpointed_turn() {
    let te = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&te.store, &repo.path);
    let ws = te.engine.create_workspace(&proj, "open", "").await.unwrap();
    te.engine.ensure_pair_agent(&ws.id, "PA", "sonnet");
    let sess = te
        .engine
        .ensure_agent_session(&ws.id, "coder", "PA", "Agent", "claude-code")
        .unwrap();
    let user = te.store.append_message(&sess.id, q("")).unwrap();
    assert_eq!(
        te.engine.last_open_chat_prompt_id(&ws, "PA").await,
        "",
        "no start checkpoint: a distinct turn"
    );
    te.engine.take_checkpoint(&ws, &user.id, "start").await;
    assert_eq!(te.engine.last_open_chat_prompt_id(&ws, "PA").await, user.id);
    // An interim end (graceful quit) still reuses the turn.
    te.engine.take_checkpoint(&ws, &user.id, "end").await;
    assert_eq!(te.engine.last_open_chat_prompt_id(&ws, "PA").await, user.id);
    assert_eq!(last_chat_prompt_id(&te.store, &ws.id, "PA"), user.id);
    // A footer closes it: AutoResume must not steal the finished pair.
    let footer = te
        .store
        .append_message(
            &sess.id,
            Message {
                role: "assistant".into(),
                ..Message::default()
            },
        )
        .unwrap();
    te.store
        .complete_message(
            &footer.id,
            recur_domain::STREAM_COMPLETE,
            &Message {
                duration_ms: 10,
                ..Message::default()
            },
        )
        .unwrap();
    assert_eq!(te.engine.last_open_chat_prompt_id(&ws, "PA").await, "");
    te.close().await;
}
