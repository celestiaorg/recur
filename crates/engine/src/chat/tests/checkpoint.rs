//! Send-path checkpoint tests: a turn's start/end snapshots through
//! `send_workspace_message` (diff and reset), and the AutoResume continuation
//! reusing an interrupted turn's pair without stealing a completed one.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use recur_agent::Provider;
use recur_domain::{AgentRole, ContentBlock, Message, PROVIDER_CLAUDE_CODE, Workspace};
use recur_git::CHECKPOINT_REF_PREFIX;

use super::recover::persist_working_chat;
use super::test_support::*;
use crate::codes::PAIR_AGENT_CODE;
use crate::loops::test_support::CaptureScript;
use crate::session_config::AgentConfig;
use crate::testing::{git_out, wait_for};
use crate::turn::checkpoint::{checkpoint_ref, last_chat_prompt_id};
use crate::turn::finalize::CHAT_RESUME_DISPLAY;

/// Writes `name(call)` into the worktree, then replays the capture script.
fn editing(c: &Arc<CaptureScript>, name: fn(usize) -> &'static str) -> Arc<dyn Provider> {
    let counter = Arc::clone(c);
    before_turn(c.provider(), move |req| {
        let call = counter.specs.lock().len();
        let _ = std::fs::write(req.worktree.join(name(call)), "agent\n");
        async {}
    })
}

async fn oid(ws: &Workspace, r: &str) -> String {
    recur_git::list_checkpoints(&ws.worktree_path, r)
        .await
        .unwrap_or_default()
        .get(r)
        .cloned()
        .unwrap_or_default()
}

async fn footer_ids(fx: &Fx, ws: &str) -> Vec<String> {
    fx.eng()
        .workspace_chat_items(ws, PAIR_AGENT_CODE)
        .await
        .into_iter()
        .filter(|i| i.kind == "turn")
        .map(|i| i.message_id)
        .collect()
}

async fn diff_paths(fx: &Fx, ws: &str, footer: &str) -> Vec<String> {
    fx.eng()
        .workspace_chat_turn_diff(ws, PAIR_AGENT_CODE, footer)
        .await
        .unwrap()
        .files
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.path)
        .collect()
}

#[tokio::test]
async fn send_turns_checkpoint_diff_and_reset() {
    let c = capture_same("sess-ckpt", "ok");
    let fx = Fx::new(Some(editing(&c, |call| {
        if call == 0 { "turn1.txt" } else { "turn2.txt" }
    })));
    let ws = fx.workspace("ckpt").await;
    let wt = Path::new(&ws.worktree_path);
    std::fs::write(wt.join("mine.txt"), "user\n").unwrap();
    fx.send(&ws.id, "PA", "first", "sonnet").await;
    fx.send(&ws.id, "PA", "second", "sonnet").await;
    let footers: Vec<_> = fx
        .eng()
        .workspace_chat_items(&ws.id, "PA")
        .await
        .into_iter()
        .filter(|i| i.kind == "turn")
        .collect();
    assert!(
        footers.len() == 2 && footers[0].checkpoint && footers[1].checkpoint,
        "{footers:?}"
    );
    let d = fx
        .eng()
        .workspace_chat_turn_diff(&ws.id, "PA", &footers[1].message_id)
        .await
        .unwrap();
    let files = d.files.clone().unwrap_or_default();
    assert!(
        files.len() == 1 && files[0].path == "turn2.txt",
        "{files:?}"
    );
    assert_eq!((d.diff.add, d.hash.len()), (1, 7));
    fx.eng()
        .reset_workspace_chat(&ws.id, "PA", &footers[0].message_id)
        .await
        .unwrap();
    assert!(!wt.join("turn2.txt").exists());
    assert!(wt.join("turn1.txt").exists() && wt.join("mine.txt").exists());
    assert_eq!(
        git_out(&ws.worktree_path, &["status", "--porcelain"]).unwrap(),
        "?? mine.txt\n?? turn1.txt\n"
    );
    let items = fx.items(&ws.id, "PA").await;
    assert!(items.len() == 2 && items[0].text == "first", "{items:?}");
    let sess = fx.te.store.session_for_loop_agent(&ws.id, "PA").unwrap();
    assert!(AgentConfig::parse(&sess.config_json).fork_inject);
    assert_eq!(fx.te.store.agent(&ws.id, "PA").unwrap().session_id, "");
    fx.te.close().await;
}

/// A session with an unanswered user row and its start snapshot, then
/// `mid.txt` written as the crashed turn's work.
async fn interrupted_turn(fx: &Fx, ws: &Workspace) -> String {
    let eng = fx.eng();
    eng.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "sonnet");
    let sess = eng
        .ensure_agent_session(
            &ws.id,
            AgentRole::Coder.as_str(),
            PAIR_AGENT_CODE,
            "Agent",
            PROVIDER_CLAUDE_CODE,
        )
        .unwrap();
    let user = fx
        .te
        .store
        .append_message(
            &sess.id,
            Message {
                role: "user".into(),
                content: Some(vec![ContentBlock::text("do the work")]),
                ..Message::default()
            },
        )
        .unwrap();
    assert!(!user.id.is_empty());
    eng.take_checkpoint(ws, &user.id, "start").await;
    std::fs::write(Path::new(&ws.worktree_path).join("mid.txt"), "mid\n").unwrap();
    user.id
}

async fn auto_resume(fx: &Fx, c: &Arc<CaptureScript>, skip_user: bool, ws: &str) {
    fx.eng().recover();
    fx.eng().auto_resume().await;
    let cc = Arc::clone(c);
    wait_for(Duration::from_secs(5), "auto-resume chat turn", move || {
        cc.with_role(AgentRole::Coder).last().is_some_and(|s| {
            s.display_text == CHAT_RESUME_DISPLAY && s.skip_record_user == skip_user
        })
    })
    .await;
    let (st, id) = (Arc::clone(&fx.te.store), ws.to_owned());
    wait_for(Duration::from_secs(3), "resumed chat idle", move || {
        st.agent(&id, PAIR_AGENT_CODE)
            .is_some_and(|a| a.status == "idle")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_reuses_the_interrupted_turns_start() {
    let c = capture_same("sess-ckpt-restart", "ok");
    let fx = Fx::new(Some(editing(&c, |_| "post.txt")));
    let ws = fx.workspace("ckpt-restart").await;
    let user = interrupted_turn(&fx, &ws).await;
    let start = checkpoint_ref(&ws.id, &user, "start");
    let start_oid = oid(&ws, &start).await;
    assert!(!start_oid.is_empty());
    persist_working_chat(&fx, &ws.id, PAIR_AGENT_CODE);
    auto_resume(&fx, &c, true, &ws.id).await;
    let refs = recur_git::list_checkpoints(
        &ws.worktree_path,
        &format!("{CHECKPOINT_REF_PREFIX}{}/", ws.id),
    )
    .await
    .unwrap_or_default();
    assert_eq!(refs.get(&start), Some(&start_oid), "the pre-crash start");
    assert!(refs.contains_key(&checkpoint_ref(&ws.id, &user, "end")));
    let items = fx.eng().workspace_chat_items(&ws.id, PAIR_AGENT_CODE).await;
    for it in &items {
        assert!(
            !(it.role == "user"
                && (it.text == CHAT_RESUME_DISPLAY || it.text.contains("Recur restarted"))),
            "resume user bubble: {}",
            it.text
        );
    }
    let footer = footer_ids(&fx, &ws.id).await.pop().expect("a footer");
    let paths = diff_paths(&fx, &ws.id, &footer).await;
    for want in ["mid.txt", "post.txt"] {
        assert!(paths.iter().any(|p| p == want), "{paths:?} lacks {want}");
    }
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_does_not_steal_a_completed_turns_end() {
    let c = capture_same("sess-ckpt-steal", "ok");
    let fx = Fx::new(Some(editing(&c, |call| {
        if call == 0 { "first.txt" } else { "resume.txt" }
    })));
    let ws = fx.workspace("ckpt-steal").await;
    fx.send(&ws.id, PAIR_AGENT_CODE, "first", "sonnet").await;
    let user = last_chat_prompt_id(&fx.te.store, &ws.id, PAIR_AGENT_CODE);
    assert!(!user.is_empty());
    let footer = footer_ids(&fx, &ws.id).await.pop().expect("a footer");
    let end = checkpoint_ref(&ws.id, &user, "end");
    let end_oid = oid(&ws, &end).await;
    assert!(!end_oid.is_empty());
    persist_working_chat(&fx, &ws.id, PAIR_AGENT_CODE);
    auto_resume(&fx, &c, false, &ws.id).await;
    assert_eq!(
        oid(&ws, &end).await,
        end_oid,
        "AutoResume overwrote the end"
    );
    let paths = diff_paths(&fx, &ws.id, &footer).await;
    assert!(!paths.iter().any(|p| p == "resume.txt"), "{paths:?}");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_replaces_an_interim_end() {
    let c = capture_same("sess-ckpt-interim", "ok");
    let fx = Fx::new(Some(editing(&c, |_| "post.txt")));
    let ws = fx.workspace("ckpt-interim").await;
    let user = interrupted_turn(&fx, &ws).await;
    let start = checkpoint_ref(&ws.id, &user, "start");
    let start_oid = oid(&ws, &start).await;
    fx.eng().take_checkpoint(&ws, &user, "end").await;
    let end = checkpoint_ref(&ws.id, &user, "end");
    let end_before = oid(&ws, &end).await;
    assert!(!start_oid.is_empty() && !end_before.is_empty());
    persist_working_chat(&fx, &ws.id, PAIR_AGENT_CODE);
    auto_resume(&fx, &c, true, &ws.id).await;
    assert_eq!(oid(&ws, &start).await, start_oid);
    let end_after = oid(&ws, &end).await;
    assert!(
        !end_after.is_empty() && end_after != end_before,
        "the end is replaced"
    );
    fx.te.close().await;
}
