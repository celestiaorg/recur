//! The dispatch cases E1/E2 left for E3: Go `attention_test.go` (closed or
//! deleted conductors never run, the Docs loop raises unseen),
//! `workspace_id_test.go` (fresh dispatch skips a leftover branch, keeps
//! the issue link) and `workspace_create_test.go` (StartRunOn over a
//! failed create, BeginStartWorkspace slots and rollback). Tab close is
//! E4's; [`close_tab`] stands in for its effect on the record.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use recur_agent::attach::MAX_CHAT_ATTACHMENT_BYTES;
use recur_domain::{
    AutonomyLevel, ChatAttachment, Group, IssueRef, Project, WORKSPACE_SOLO, Workspace,
    WorkspaceState,
};
use tokio_util::sync::CancellationToken;

use crate::attention::chat_inbox::chat_inbox_id;
use crate::codes::{MAESTRO_AGENT_CODE, PAIR_AGENT_CODE};
use crate::loops::lifecycle::ERR_LOOP_OWNER_GONE;
use crate::loops::test_support::*;
use crate::testing::{TestEngine, add_project, add_run_project, git, init_repo, wait_for};
use crate::workspace::create::{CreatingAlloc, PHASE_CREATING, STATUS_CREATING};

fn inbox_row(t: &TestEngine, id: &str) -> Option<recur_domain::InboxItem> {
    t.store
        .inbox()
        .unwrap_or_default()
        .into_iter()
        .find(|i| i.id == id)
}

async fn owned_queued(
    t: &TestEngine,
    name: &str,
    code: &str,
) -> (Workspace, crate::testing::TempRepo) {
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t.engine.create_workspace(&proj, name, "").await.unwrap();
    t.store.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.loop_chat_code = code.into();
        w.state = WorkspaceState::Queued;
    });
    (t.store.workspace(&ws.id).unwrap(), repo)
}

/// Dispatches a stale snapshot under a held slot and expects the owner-gone
/// refusal with no walk registered.
async fn assert_owner_gone(t: &TestEngine, stale: Workspace) {
    let id = stale.id.clone();
    assert!(t.engine.governor().try_acquire_loop());
    let err = t.engine.dispatch(stale).await.unwrap_err();
    assert_eq!(err.0, ERR_LOOP_OWNER_GONE);
    t.engine.governor().release_loop();
    assert!(t.engine.running_loop_by_id(&id).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_does_not_recreate_closed_maestro() {
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let (ws, _repo) = owned_queued(&t, "closed-ma-dispatch", MAESTRO_AGENT_CODE).await;
    t.engine
        .ensure_maestro_agent_with_status(&ws.id, &ws.goal, "idle");
    let stale = t.store.workspace(&ws.id).unwrap();
    assert_eq!(stale.loop_chat_code, MAESTRO_AGENT_CODE);
    close_tab(&t.store, &ws.id, MAESTRO_AGENT_CODE);
    assert!(t.store.agent(&ws.id, MAESTRO_AGENT_CODE).is_none());
    assert_owner_gone(&t, stale).await;
    assert!(
        t.store.agent(&ws.id, MAESTRO_AGENT_CODE).is_none(),
        "dispatch must not recreate a closed MA tab"
    );
    assert_eq!(t.store.workspace(&ws.id).unwrap().loop_chat_code, "");
    assert!(inbox_row(&t, &chat_inbox_id(&ws.id, MAESTRO_AGENT_CODE)).is_none());
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_does_not_run_closed_queued_pair() {
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let (ws, _repo) = owned_queued(&t, "closed-pa-dispatch", PAIR_AGENT_CODE).await;
    t.engine.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "");
    let stale = t.store.workspace(&ws.id).unwrap();
    close_tab(&t.store, &ws.id, PAIR_AGENT_CODE);
    assert_eq!(t.store.workspace(&ws.id).unwrap().loop_chat_code, "");
    assert_owner_gone(&t, stale).await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_does_not_run_deleted_pair_agent() {
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let (ws, _repo) = owned_queued(&t, "restored-pa-dispatch", PAIR_AGENT_CODE).await;
    t.engine.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "");
    t.store
        .update_workspace(&ws.id, |w| w.state = WorkspaceState::Paused);
    t.store.delete_agent(&ws.id, PAIR_AGENT_CODE);
    let stale = t.store.workspace(&ws.id).unwrap();
    assert_eq!(
        stale.loop_chat_code, PAIR_AGENT_CODE,
        "still names the deleted PA"
    );
    assert_owner_gone(&t, stale).await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn docs_loop_finished_raises_unseen() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "Write the README",
            "",
            AutonomyLevel::default(),
            "test.loops.docs",
            "test.policies.fail",
        )
        .await
        .unwrap();
    let row_id = chat_inbox_id(&id, MAESTRO_AGENT_CODE);
    wait_for(Duration::from_secs(15), "docs loop unseen inbox", || {
        t.store
            .agent(&id, MAESTRO_AGENT_CODE)
            .is_some_and(|a| a.unseen)
            && inbox_row(&t, &row_id).is_some_and(|it| {
                it.unread && it.title.contains("Loop") && it.title.contains("finished")
            })
    })
    .await;
    let c = t.engine.attention_counts();
    assert!(c.unseen >= 1, "{c:?}");
    assert!(
        c.by_workspace
            .as_ref()
            .and_then(|m| m.get(&id))
            .is_some_and(|n| *n >= 1)
    );
    assert!(
        c.by_project
            .as_ref()
            .and_then(|m| m.get(&proj))
            .is_some_and(|n| *n >= 1)
    );
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_fresh_loop_does_not_restore_leftover_branch() {
    let repo = init_repo();
    git(&repo.path, &["checkout", "-b", "recur/bubbly-pretzel"]);
    std::fs::write(Path::new(&repo.path).join("stale.txt"), "old\n").unwrap();
    git(&repo.path, &["add", "stale.txt"]);
    git(&repo.path, &["commit", "-m", "stale leftover"]);
    git(&repo.path, &["checkout", "main"]);
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let proj = add_project(&t.store, &repo.path);
    let rec = Workspace {
        id: "bubbly-pretzel".into(),
        project_id: proj,
        goal: "fresh".into(),
        run_started: true,
        branch: "recur/bubbly-pretzel".into(),
        base_branch: "main".into(),
        state: WorkspaceState::Queued,
        status_label: "Queued".into(),
        group: Group::Active,
        ..Workspace::default()
    };
    t.store.add_workspace(rec.clone()).unwrap();
    assert!(t.engine.governor().try_acquire_loop());
    t.engine.dispatch(rec).await.unwrap();
    let ws = t.store.workspace("bubbly-pretzel").unwrap();
    assert_eq!(ws.branch, "recur/bubbly-pretzel-2");
    assert!(
        !Path::new(&ws.worktree_path).join("stale.txt").exists(),
        "fresh dispatch restored the leftover checkout"
    );
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_keeps_issue_linked_during_worktree_add() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let proj = add_project(&t.store, &repo.path);
    let rec = Workspace {
        id: "issue-link".into(),
        project_id: proj,
        goal: "linked".into(),
        run_started: true,
        base_branch: "main".into(),
        state: WorkspaceState::Queued,
        status_label: "Queued".into(),
        group: Group::Active,
        ..Workspace::default()
    };
    t.store.add_workspace(rec.clone()).unwrap();
    let issue = IssueRef {
        source: "linear".into(),
        id: "lin-1".into(),
        key: "ENG-1".into(),
        ..IssueRef::default()
    };
    let linked = issue.clone();
    t.store
        .update_workspace(&rec.id, |w| w.issue = Some(linked));
    assert!(t.engine.governor().try_acquire_loop());
    t.engine.dispatch(rec).await.unwrap();
    assert_eq!(t.store.workspace("issue-link").unwrap().issue, Some(issue));
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_run_on_failed_create_refuses() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_run_project(
        &t.store,
        Project {
            id: "p1".into(),
            name: "p1".into(),
            repo_path: repo.path.clone(),
            base_branch: "main".into(),
            setup_cmd: "exit 1".into(),
            ..Project::default()
        },
    );
    let ws = t
        .engine
        .begin_create_workspace(&proj.id, "will-fail", "main")
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "create failure", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| w.phase == "failed")
    })
    .await;
    assert!(
        t.engine
            .start_run_on_workspace(&ws.id, "do the thing")
            .await
            .is_err()
    );
    t.engine.delete_workspace(&ws.id).await.unwrap();
    t.close().await;
}

#[tokio::test]
async fn start_run_on_workspace_does_not_stamp_over_failed_create() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "ws-race".into(),
            project_id: "p1".into(),
            r#type: WORKSPACE_SOLO.into(),
            phase: "failed".into(),
            last_error: "setup failed".into(),
            status_label: "Couldn't create".into(),
            ..Workspace::default()
        })
        .unwrap();
    // The window after fail_creating's store write and before the in-flight
    // entry drops: the stamp must not overwrite the failure.
    let done = CancellationToken::new();
    done.cancel();
    t.engine.creating.lock().insert(
        "ws-race".into(),
        Arc::new(CreatingAlloc {
            cancel: CancellationToken::new(),
            done,
        }),
    );
    assert!(
        t.engine
            .start_run_on_workspace("ws-race", "do the thing")
            .await
            .is_err()
    );
    t.engine.creating.lock().remove("ws-race");
    let got = t.store.workspace("ws-race").unwrap();
    assert!(!got.run_started);
    assert_ne!(got.state, WorkspaceState::Queued);
    assert_eq!(got.phase, "failed");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn begin_start_workspace_attachment_error_rolls_back() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let before = t.store.workspaces().len();
    let huge = ChatAttachment {
        id: "huge".into(),
        name: "huge.png".into(),
        mime: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD
            .encode(vec![0u8; MAX_CHAT_ATTACHMENT_BYTES + 1]),
    };
    let res = t
        .engine
        .begin_start_workspace_with(
            &proj,
            "look",
            "",
            "",
            AutonomyLevel::default(),
            "",
            "",
            None,
            &[huge],
        )
        .await;
    assert!(res.is_err());
    assert_eq!(t.store.workspaces().len(), before, "rolled back");
    assert!(
        t.engine.governor().try_acquire_loop(),
        "governor slot leaked"
    );
    t.engine.governor().release_loop();
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn begin_start_workspace_with_slot_is_creating() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let (ws, acquired) = t
        .engine
        .queue_start_workspace(
            &proj,
            "ship it",
            "",
            "",
            &AutonomyLevel::Balanced,
            "",
            "",
            None,
        )
        .await
        .unwrap();
    assert!(acquired, "a loop slot");
    assert_eq!(ws.phase, PHASE_CREATING, "slot held; worktree not ready");
    assert_eq!(ws.status_label, STATUS_CREATING);
    t.engine.governor().release_loop();
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn begin_start_workspace_without_slot_waits() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let slots = take_slots(&t);
    let ws = t
        .engine
        .begin_start_workspace(&proj, "ship it", "", AutonomyLevel::Balanced, None)
        .await
        .unwrap();
    assert_eq!(ws.phase, "waiting for a slot");
    release_slots(&t, slots);
    t.close().await;
}
