//! Port of Go `lifecycle_test.go`, the loop-driven half: Remove and Delete
//! after a real walk reaches pr_open, and Delete refused while a walk runs.
//! The hand-workspace cases live in `workspace::tests`. The Cursor project
//! dir check needs a per-test `$HOME`, which a shared test process cannot
//! set safely; `recur-provider-cursor` tests `ensure_cursor_project_dirs`.

use std::path::Path;
use std::time::Duration;

use recur_domain::{AutonomyLevel, Workspace, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::loops::test_support::*;
use crate::testing::{TestEngine, add_project, git_out, init_repo, wait_for};

/// Drives `id` to pr_open and waits for its walk to deregister (Go
/// `runToPROpen`): Remove/Delete refuse a walk still in the registry.
async fn run_to_pr_open(t: &TestEngine, id: &str) -> Workspace {
    wait_for(Duration::from_secs(5), "loop to reach pr_open", || {
        t.store
            .workspace(id)
            .is_some_and(|l| matches!(l.state, WorkspaceState::PrOpen | WorkspaceState::Failed))
    })
    .await;
    let l = t.store.workspace(id).unwrap();
    assert_eq!(l.state, WorkspaceState::PrOpen, "{}", l.last_error);
    wait_for(Duration::from_secs(5), "walk to exit", || {
        t.engine.running_loop_by_id(id).is_none()
    })
    .await;
    l
}

async fn start(t: &TestEngine, proj: &str, task: &str) -> String {
    t.engine
        .start_workspace(proj, task, "", AutonomyLevel::default())
        .await
        .unwrap()
}

async fn branch_exists(repo: &str, branch: &str) -> bool {
    recur_git::Manager::new(repo, "")
        .commit_exists(branch)
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_workspace_removes_worktree() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "Add a converging feature").await;
    let before = run_to_pr_open(&t, &id).await;
    assert!(!before.worktree_path.is_empty());
    assert!(Path::new(&before.worktree_path).exists());

    let removed = t.engine.remove_workspace(&id).await.unwrap();
    assert!(removed.archived);
    assert_eq!(removed.worktree_path, "");
    assert_eq!(removed.port, 0, "port released");
    wait_for(Duration::from_secs(5), "worktree removed", || {
        !Path::new(&before.worktree_path).exists()
    })
    .await;
    let list = git_out(&repo.path, &["worktree", "list", "--porcelain"]).unwrap();
    assert!(
        !list
            .lines()
            .any(|l| l.strip_prefix("worktree ") == Some(before.worktree_path.as_str())),
        "{list}"
    );
    assert!(
        !t.store.feed(&id).unwrap_or_default().is_empty(),
        "history survives"
    );
    assert!(t.store.pr_by_workspace(&id).is_some(), "the PR survives");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remove_workspace_deletes_branch_when_configured() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0; 8]).provider()));
    let proj = add_project(&t.store, &repo.path);

    let id = start(&t, &proj, "Feature with a unique commit").await;
    let branch = run_to_pr_open(&t, &id).await.branch;
    assert!(!branch.is_empty());
    t.engine.remove_workspace(&id).await.unwrap();
    wait_for(Duration::from_secs(5), "default remove teardown", || {
        !t.engine.is_deleting(&id)
    })
    .await;
    assert!(
        branch_exists(&repo.path, &branch).await,
        "default Remove keeps {branch}"
    );

    let id2 = start(&t, &proj, "Another feature").await;
    let branch2 = run_to_pr_open(&t, &id2).await.branch;
    assert!(!branch2.is_empty() && branch2 != branch);
    let mut p = t.store.project(&proj).unwrap();
    p.delete_branch_on_remove = true;
    t.store.update_project_run_config(p).unwrap();
    t.engine.remove_workspace(&id2).await.unwrap();
    wait_for(Duration::from_secs(5), "open-PR remove teardown", || {
        !t.engine.is_deleting(&id2)
    })
    .await;
    assert!(
        branch_exists(&repo.path, &branch2).await,
        "an open PR keeps {branch2}"
    );

    let id3 = start(&t, &proj, "Merged then remove").await;
    let branch3 = run_to_pr_open(&t, &id3).await.branch;
    assert!(!branch3.is_empty() && branch3 != branch2);
    let pr3 = t.store.pr_by_workspace(&id3).expect("a PR");
    t.store.update_pr(&pr3.id, |p| {
        p.state = "Merged".into();
        p.group = "merged".into();
    });
    t.engine.remove_workspace(&id3).await.unwrap();
    let mut gone = false;
    for _ in 0..250 {
        if !branch_exists(&repo.path, &branch3).await {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone, "merged PR branch {branch3} deleted on remove");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_workspace_removes_everything() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "Add another converging feature").await;
    let settled = run_to_pr_open(&t, &id).await;
    let pr = t.store.pr_by_workspace(&id).expect("a PR");
    assert!(
        !t.store
            .sessions_for_loop(&id)
            .unwrap_or_default()
            .is_empty(),
        "the walk mirrored its turns"
    );
    t.engine.delete_workspace(&id).await.unwrap();
    assert!(t.store.workspace(&id).is_none());
    assert!(t.store.pr(&pr.id).is_none());
    assert!(
        t.store
            .sessions_for_loop(&id)
            .unwrap_or_default()
            .is_empty(),
        "sessions cascade"
    );
    assert!(t.events.has("workspace.deleted"));
    wait_for(Duration::from_secs(5), "worktree removed", || {
        !Path::new(&settled.worktree_path).exists()
    })
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delete_workspace_while_running_errors() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[])));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "A loop that stays running").await;
    wait_for(Duration::from_secs(2), "walk in flight", || {
        t.engine.running_loop_by_id(&id).is_some()
            && t.store
                .workspace(&id)
                .is_some_and(|l| l.state == WorkspaceState::Planning)
    })
    .await;
    assert!(
        t.engine.delete_workspace(&id).await.is_err(),
        "refused while the walk runs"
    );
    gate.cancel();
    t.engine.stop(&id).await;
    t.close().await;
}
