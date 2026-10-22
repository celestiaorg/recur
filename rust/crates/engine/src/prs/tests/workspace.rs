//! Hand-driven landing from a workspace at the engine level (Go
//! `app/workspace_pr_test.go` and `app/workspace_base_test.go`, whose
//! services wrappers are SV's).

use std::path::Path;
use recur_domain::{
    MergeMethod, PrSource, UPDATE_WORKSPACE_CONFLICT, UPDATE_WORKSPACE_NOOP,
    UPDATE_WORKSPACE_UPDATED, Workspace, WorkspaceState,
};
use recur_forge::model::pr_description;

use super::support::*;
use crate::testing::{TempRepo, git, git_out, init_repo};

/// Px with project `acme` on a fresh repo and one ready workspace.
async fn ws_fx(name: &str) -> (Px, TempRepo, Workspace) {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let w = px.eng().create_workspace("acme", name, "").await.unwrap();
    (px, repo, w)
}

fn write(ws: &Workspace, rel: &str, body: &str) {
    std::fs::write(Path::new(&ws.worktree_path).join(rel), body).unwrap();
}

fn commit(ws: &Workspace, msg: &str) {
    git(&ws.worktree_path, &["add", "-A"]);
    git(&ws.worktree_path, &["commit", "-m", msg]);
}

fn show(dir: &str, spec: &str) -> String {
    git_out(dir, &["show", spec]).unwrap_or_default().trim().to_owned()
}

#[tokio::test]
async fn open_pr_and_merge() {
    let (px, repo, ws) = ws_fx("add a feature").await;
    let eng = px.eng();
    assert!(eng.open_workspace_pr(&ws.id, false).await.is_err(), "nothing committed");
    write(&ws, "feature.txt", "the feature\n");
    let err = eng.open_workspace_pr(&ws.id, false).await.unwrap_err();
    assert!(err.0.contains("uncommitted"), "{err}");
    commit(&ws, "solo: add feature");
    let id = eng.open_workspace_pr(&ws.id, false).await.unwrap();
    let pr = px.pr(&id);
    assert_eq!(pr.workspace_id, ws.id);
    assert_eq!(pr.source, PrSource::Local);
    assert!(pr.file_count > 0);
    let desc = pr_description(&pr);
    assert!(desc.contains("add a feature") && desc.contains("## Changes"), "{desc}");
    let land = eng.workspace_land_state(&ws.id).await;
    assert_eq!((land.pr_id.as_str(), land.pr_number), (id.as_str(), pr.number));
    assert_eq!(land.pr_source, "local");
    assert_eq!(eng.open_workspace_pr(&ws.id, false).await.unwrap(), id, "reopen is idempotent");
    let inbox = px.te.store.inbox().unwrap_or_default();
    assert!(inbox.iter().any(|i| i.id == format!("ib-pr-{id}")));

    let (merged, ok) = eng.merge_pr(&id, &MergeMethod::default()).await;
    assert!(ok);
    assert_eq!((merged.state.as_str(), merged.group.as_str()), ("Merged", "merged"));
    assert_eq!(show(&repo.path, &format!("{}:feature.txt", ws.base_branch)), "the feature");
    assert!(!px.te.store.inbox().unwrap_or_default().iter().any(|i| i.id == format!("ib-pr-{id}")));
}

#[tokio::test]
async fn open_pr_with_agent_body() {
    let (px, _repo, ws) = ws_fx("scratch name").await;
    let eng = px.eng();
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "solo: add feature");
    let body = "This covers the whole workspace diff, not just this chat.";
    let id = eng
        .open_workspace_pr_with(&ws.id, "Agent-written title", body, false)
        .await
        .unwrap();
    let pr = px.pr(&id);
    assert_eq!(pr.title, "Agent-written title");
    assert_eq!(pr_description(&pr), body);

    write(&ws, "more.txt", "more\n");
    commit(&ws, "solo: more");
    eng.open_workspace_pr(&ws.id, false).await.unwrap();
    let again = px.pr(&id);
    assert_eq!(again.title, "Agent-written title");
    assert_eq!(pr_description(&again), body);
    assert!(again.file_count >= 2);

    let retry = "A later /open-pr on a Local PR must cover the whole workspace.";
    eng.open_workspace_pr_with(&ws.id, "A different title", retry, false)
        .await
        .unwrap();
    let rewritten = px.pr(&id);
    assert_eq!(rewritten.title, "A different title");
    assert_eq!(pr_description(&rewritten), retry);
}

#[tokio::test]
async fn open_pr_fallback_skips_prompt_action_wire() {
    let (px, _repo, ws) = ws_fx("scratch name").await;
    let label = "Review & fix to a PR";
    px.te.store.update_workspace(&ws.id, |w| {
        w.goal = "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR.".into();
        w.goal_display = label.into();
    });
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "solo: add feature");
    let id = px.eng().open_workspace_pr(&ws.id, false).await.unwrap();
    let pr = px.pr(&id);
    assert_eq!(pr.title, label);
    let got = pr_description(&pr);
    assert!(!got.contains("The human already wrote"), "{got}");
    assert!(got.contains("## Changes"), "{got}");
}

#[tokio::test]
async fn merge_local_without_pr() {
    let (px, repo, ws) = ws_fx("add a feature").await;
    let eng = px.eng();
    assert!(eng.merge_workspace(&ws.id, &MergeMethod::default()).await.is_err(), "nothing to merge");
    assert_eq!(eng.workspace_land_state(&ws.id).await.ahead_base, 0);
    write(&ws, "feature.txt", "the feature\n");
    assert!(eng.workspace_land_state(&ws.id).await.dirty);
    assert!(eng.merge_workspace(&ws.id, &MergeMethod::default()).await.is_err(), "uncommitted");
    commit(&ws, "solo: add feature");
    assert!(eng.workspace_land_state(&ws.id).await.ahead_base >= 1);
    let merged = eng.merge_workspace(&ws.id, &MergeMethod::default()).await.unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert!(px.te.store.pr_by_workspace(&ws.id).is_none(), "no PR");
    assert_eq!(show(&repo.path, &format!("{}:feature.txt", ws.base_branch)), "the feature");
    let again = eng.merge_workspace(&ws.id, &MergeMethod::default()).await.unwrap();
    assert_eq!(again.state, WorkspaceState::Merged, "idempotent");
}

#[tokio::test]
async fn land_refused_while_a_run_is_active() {
    let (px, _repo, ws) = ws_fx("looped").await;
    let eng = px.eng();
    px.te.store.update_workspace(&ws.id, |w| {
        w.run_started = true;
        w.state = WorkspaceState::Coding;
    });
    assert!(eng.merge_workspace(&ws.id, &MergeMethod::default()).await.is_err());
    assert!(eng.update_workspace_with_base(&ws.id, "").await.is_err());
    assert!(eng.open_workspace_pr(&ws.id, false).await.is_err());
    assert!(eng.pull_workspace_pr(&ws.id).await.is_err());
    px.te.store.update_workspace(&ws.id, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "idle".into();
    });
    write(&ws, "idle.txt", "done\n");
    commit(&ws, "idle: add file");
    eng.open_workspace_pr(&ws.id, false).await.unwrap();
}

#[tokio::test]
async fn update_with_base() {
    let (px, repo, ws) = ws_fx("catch up").await;
    let eng = px.eng();
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "solo: add feature");
    let land = eng.workspace_land_state(&ws.id).await;
    assert!(land.behind_base == 0 && land.ahead_base >= 1 && !land.dirty && !land.unmerged, "{land:?}");
    std::fs::write(Path::new(&repo.path).join("extra.txt"), "base moved\n").unwrap();
    git(&repo.path, &["add", "extra.txt"]);
    git(&repo.path, &["commit", "-m", "advance main"]);
    let err = eng.merge_workspace(&ws.id, &MergeMethod::default()).await.unwrap_err();
    assert!(err.0.contains("commits behind main — update with base first"), "{err}");
    assert!(eng.workspace_land_state(&ws.id).await.behind_base >= 1);
    let res = eng.update_workspace_with_base(&ws.id, "").await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(show(&ws.worktree_path, "HEAD:extra.txt"), "base moved");
    assert_eq!(eng.workspace_land_state(&ws.id).await.behind_base, 0);
    let noop = eng.update_workspace_with_base(&ws.id, "").await.unwrap();
    assert_eq!(noop.status, UPDATE_WORKSPACE_NOOP);
}

#[tokio::test]
async fn update_with_base_conflict() {
    let (px, repo, ws) = ws_fx("conflict").await;
    let eng = px.eng();
    write(&ws, "README.md", "workspace edit\n");
    commit(&ws, "solo: edit readme");
    std::fs::write(Path::new(&repo.path).join("README.md"), "main edit\n").unwrap();
    git(&repo.path, &["add", "README.md"]);
    git(&repo.path, &["commit", "-m", "advance main with conflict"]);
    let res = eng.update_workspace_with_base(&ws.id, "").await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_CONFLICT);
    assert!(eng.workspace_land_state(&ws.id).await.unmerged);
}
