//! Hand-driven landing from a workspace at the engine level (Go
//! `app/workspace_pr_test.go` and `app/workspace_base_test.go`, whose
//! services wrappers are SV's).

use recur_domain::{
    MergeMethod, Pr, PrSource, Project, UPDATE_WORKSPACE_CONFLICT, UPDATE_WORKSPACE_NOOP,
    UPDATE_WORKSPACE_UPDATED, Workspace, WorkspaceState,
};
use recur_forge::model::pr_description;
use std::path::Path;

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
    git_out(dir, &["show", spec])
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[tokio::test]
async fn open_pr_and_merge() {
    let (px, repo, ws) = ws_fx("add a feature").await;
    let eng = px.eng();
    assert!(
        eng.open_workspace_pr(&ws.id, false).await.is_err(),
        "nothing committed"
    );
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
    assert!(
        desc.contains("add a feature") && desc.contains("## Changes"),
        "{desc}"
    );
    let land = eng.workspace_land_state(&ws.id).await;
    assert_eq!(
        (land.pr_id.as_str(), land.pr_number),
        (id.as_str(), pr.number)
    );
    assert_eq!(land.pr_source, "local");
    assert_eq!(
        eng.open_workspace_pr(&ws.id, false).await.unwrap(),
        id,
        "reopen is idempotent"
    );
    let inbox = px.te.store.inbox().unwrap_or_default();
    assert!(inbox.iter().any(|i| i.id == format!("ib-pr-{id}")));

    let (merged, ok) = eng.merge_pr(&id, &MergeMethod::default()).await;
    assert!(ok);
    assert_eq!(
        (merged.state.as_str(), merged.group.as_str()),
        ("Merged", "merged")
    );
    assert_eq!(
        show(&repo.path, &format!("{}:feature.txt", ws.base_branch)),
        "the feature"
    );
    assert!(
        !px.te
            .store
            .inbox()
            .unwrap_or_default()
            .iter()
            .any(|i| i.id == format!("ib-pr-{id}"))
    );
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
    assert!(
        eng.merge_workspace(&ws.id, &MergeMethod::default())
            .await
            .is_err(),
        "nothing to merge"
    );
    assert_eq!(eng.workspace_land_state(&ws.id).await.ahead_base, 0);
    write(&ws, "feature.txt", "the feature\n");
    assert!(eng.workspace_land_state(&ws.id).await.dirty);
    assert!(
        eng.merge_workspace(&ws.id, &MergeMethod::default())
            .await
            .is_err(),
        "uncommitted"
    );
    commit(&ws, "solo: add feature");
    assert!(eng.workspace_land_state(&ws.id).await.ahead_base >= 1);
    let merged = eng
        .merge_workspace(&ws.id, &MergeMethod::default())
        .await
        .unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert!(px.te.store.pr_by_workspace(&ws.id).is_none(), "no PR");
    assert_eq!(
        show(&repo.path, &format!("{}:feature.txt", ws.base_branch)),
        "the feature"
    );
    let again = eng
        .merge_workspace(&ws.id, &MergeMethod::default())
        .await
        .unwrap();
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
    assert!(
        eng.merge_workspace(&ws.id, &MergeMethod::default())
            .await
            .is_err()
    );
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
    assert!(
        land.behind_base == 0 && land.ahead_base >= 1 && !land.dirty && !land.unmerged,
        "{land:?}"
    );
    std::fs::write(Path::new(&repo.path).join("extra.txt"), "base moved\n").unwrap();
    git(&repo.path, &["add", "extra.txt"]);
    git(&repo.path, &["commit", "-m", "advance main"]);
    let err = eng
        .merge_workspace(&ws.id, &MergeMethod::default())
        .await
        .unwrap_err();
    assert!(
        err.0
            .contains("commits behind main — update with base first"),
        "{err}"
    );
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

/// Go `initGitRepoWithRemote`: (keep, clone, bare). The clone tracks a bare
/// origin seeded with one commit on main.
fn repo_with_remote() -> (Vec<tempfile::TempDir>, String, String) {
    let dirs: Vec<tempfile::TempDir> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let bare = dirs[0]
        .path()
        .join("upstream.git")
        .to_string_lossy()
        .into_owned();
    let seed = dirs[1].path().to_string_lossy().into_owned();
    git(&seed, &["init", "-b", "main"]);
    git(&seed, &["config", "user.name", "test"]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    std::fs::write(Path::new(&seed).join("README.md"), "hello\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "initial"]);
    git(&seed, &["clone", "--bare", &seed, &bare]);
    let clone = dirs[2].path().join("work").to_string_lossy().into_owned();
    git(&dirs[3].path().to_string_lossy(), &["clone", &bare, &clone]);
    git(&clone, &["config", "user.name", "test"]);
    git(&clone, &["config", "user.email", "test@example.com"]);
    (dirs, clone, bare)
}

fn project(px: &Px, id: &str, repo: &str, base: &str, pr_on_forge: bool) {
    px.te
        .store
        .add_project(Project {
            id: id.into(),
            name: id.into(),
            repo_path: repo.into(),
            base_branch: base.into(),
            pr_on_github: pr_on_forge,
            ..Project::default()
        })
        .unwrap();
}

/// Another clone of `bare` that commits `file` on `branch` and pushes it.
fn push_from_other(
    bare: &str,
    branch: &str,
    file: &str,
    body: &str,
    msg: &str,
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let other = dir.path().join("other").to_string_lossy().into_owned();
    git(&dir.path().to_string_lossy(), &["clone", bare, &other]);
    git(&other, &["config", "user.name", "test"]);
    git(&other, &["config", "user.email", "test@example.com"]);
    if branch != "main" {
        git(&other, &["checkout", branch]);
    }
    std::fs::write(Path::new(&other).join(file), body).unwrap();
    git(&other, &["add", file]);
    git(&other, &["commit", "-m", msg]);
    git(&other, &["push", "origin", "HEAD"]);
    dir
}

/// Pushes go through the real GitHub forge's git path; `gh` is `script`.
fn real_pushes(px: &Px, script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let gh_path = dir.path().join("gh");
    std::fs::write(&gh_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let gh = std::sync::Arc::new(recur_forge::github::GitHub::with_gh(recur_forge::Gh::new(
        gh_path,
    )));
    px.forge.on_push_head(move |(wt, repo, pr, head, force)| {
        let gh = std::sync::Arc::clone(&gh);
        async move { recur_forge::Forge::push_head(gh.as_ref(), &wt, &repo, &pr, &head, force).await }
    });
    dir
}

fn forge_pr(id: &str, number: i64, ws: &Workspace) -> Pr {
    Pr {
        id: id.into(),
        workspace_id: ws.id.clone(),
        group: "you".into(),
        number,
        source: PrSource::GitHub,
        branch: ws.branch.clone(),
        base: "main".into(),
        title: "feat".into(),
        state: "In review".into(),
        ..Pr::default()
    }
}

#[tokio::test]
async fn merge_pushes_remote_base() {
    let px = Px::new();
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "premote", &clone, "origin/main", false);
    let ws = px
        .eng()
        .create_workspace("premote", "land on origin", "")
        .await
        .unwrap();
    assert_eq!(px.eng().workspace_land_state(&ws.id).await.remote, "origin");
    write(&ws, "feature.txt", "from workspace\n");
    commit(&ws, "solo: add feature");
    let merged = px
        .eng()
        .merge_workspace(&ws.id, &MergeMethod::Squash)
        .await
        .unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert_eq!(show(&bare, "HEAD:feature.txt"), "from workspace");
}

fn requires_pr(px: &Px, require: bool) {
    px.forge
        .on_remote_repo(|_| async { Ok("acme/app".to_owned()) });
    px.forge
        .on_branch_requires_pr(move |_| async move { Some(require) });
}

#[tokio::test]
async fn merge_refused_when_forge_requires_pr() {
    let px = Px::new();
    requires_pr(&px, true);
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "pghremote", &clone, "origin/main", true);
    let ws = px
        .eng()
        .create_workspace("pghremote", "land on origin", "")
        .await
        .unwrap();
    assert!(px.eng().workspace_land_state(&ws.id).await.require_pr);
    write(&ws, "feature.txt", "from workspace\n");
    commit(&ws, "solo: add feature");
    let err = px
        .eng()
        .merge_workspace(&ws.id, &MergeMethod::Squash)
        .await
        .unwrap_err();
    assert!(err.0.contains("requires a GitHub pull request"), "{err}");
    assert!(
        git_out(&bare, &["show", "HEAD:feature.txt"]).is_err(),
        "nothing landed"
    );
    assert_ne!(px.ws(&ws.id).state, WorkspaceState::Merged);
}

#[tokio::test]
async fn merge_forge_remote_when_pr_not_required() {
    let px = Px::new();
    requires_pr(&px, false);
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "pghopen", &clone, "origin/main", true);
    let ws = px
        .eng()
        .create_workspace("pghopen", "land on origin", "")
        .await
        .unwrap();
    assert!(!px.eng().workspace_land_state(&ws.id).await.require_pr);
    write(&ws, "feature.txt", "from workspace\n");
    commit(&ws, "solo: add feature");
    let merged = px
        .eng()
        .merge_workspace(&ws.id, &MergeMethod::Squash)
        .await
        .unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert_eq!(show(&bare, "HEAD:feature.txt"), "from workspace");
}

#[tokio::test]
async fn merge_marks_merged_when_push_fails() {
    let px = Px::new();
    let (keep, clone, _bare) = repo_with_remote();
    git(
        &clone,
        &[
            "remote",
            "set-url",
            "origin",
            &keep[3].path().join("missing.git").to_string_lossy(),
        ],
    );
    project(&px, "ppushfail", &clone, "origin/main", false);
    let ws = px
        .eng()
        .create_workspace("ppushfail", "land locally", "")
        .await
        .unwrap();
    write(&ws, "feature.txt", "from workspace\n");
    commit(&ws, "solo: add feature");
    assert!(
        px.eng()
            .merge_workspace(&ws.id, &MergeMethod::Squash)
            .await
            .is_err(),
        "push failure"
    );
    assert_eq!(
        px.ws(&ws.id).state,
        WorkspaceState::Merged,
        "merged before the push"
    );
    let again = px
        .eng()
        .merge_workspace(&ws.id, &MergeMethod::Squash)
        .await
        .unwrap();
    assert_eq!(again.state, WorkspaceState::Merged);
}

#[tokio::test]
async fn merge_pushes_local_base_when_origin_exists() {
    let px = Px::new();
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "plocalbase", &clone, "main", false);
    let ws = px
        .eng()
        .create_workspace("plocalbase", "land and push", "main")
        .await
        .unwrap();
    assert_eq!(px.eng().workspace_land_state(&ws.id).await.remote, "origin");
    write(&ws, "feature.txt", "from workspace\n");
    commit(&ws, "solo: add feature");
    let merged = px
        .eng()
        .merge_workspace(&ws.id, &MergeMethod::Squash)
        .await
        .unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert_eq!(show(&bare, "HEAD:feature.txt"), "from workspace");
}

#[tokio::test]
async fn behind_merge_base_fetches_remote_tip() {
    let px = Px::new();
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "pbehind", &clone, "main", false);
    let ws = px
        .eng()
        .create_workspace("pbehind", "feature", "main")
        .await
        .unwrap();
    write(&ws, "feature.txt", "mine\n");
    commit(&ws, "solo: feature");
    px.add_pr(forge_pr("gh-111", 111, &ws));
    let _other = push_from_other(
        &bare,
        "main",
        "base-moved.txt",
        "ahead\n",
        "advance merge base",
    );
    assert_eq!(
        px.eng().workspace_land_state(&ws.id).await.behind_base,
        0,
        "stale local main"
    );
    px.eng().refresh_pr_behind("gh-111").await;
    assert!(px.pr("gh-111").behind_base >= 1);
    assert!(
        px.eng()
            .fetch_working_copy_upstream(&ws.id)
            .await
            .behind_base
            >= 1
    );
}

#[tokio::test]
async fn pr_refresh_fetches_remote_base_before_commit_log() {
    let px = Px::new();
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "acme", &clone, "main", false);
    let ws = px
        .eng()
        .create_workspace("acme", "feature", "main")
        .await
        .unwrap();
    write(&ws, "feature.txt", "mine\n");
    commit(&ws, "solo: feature");
    let stale = git_out(&clone, &["rev-parse", "origin/main"])
        .unwrap()
        .trim()
        .to_owned();
    let _other = push_from_other(
        &bare,
        "main",
        "base-moved.txt",
        "ahead\n",
        "advance merge base",
    );
    git(&ws.worktree_path, &["fetch", "origin", "main"]);
    git(
        &ws.worktree_path,
        &["merge", "origin/main", "-m", "sync with base"],
    );
    git(&clone, &["update-ref", "refs/remotes/origin/main", &stale]);
    px.add_pr(Pr {
        id: "pr-stale-base".into(),
        workspace_id: ws.id.clone(),
        group: "you".into(),
        source: PrSource::Local,
        branch: ws.branch.clone(),
        base: "origin/main".into(),
        title: "feat".into(),
        state: "In review".into(),
        ..Pr::default()
    });
    // GetPR's order: behind (fetches the tracking tip), then commits.
    px.eng().refresh_pr_behind("pr-stale-base").await;
    px.eng().refresh_pr_commits("pr-stale-base").await;
    let commits = px.pr("pr-stale-base").commits.unwrap_or_default();
    assert!(
        !commits
            .iter()
            .any(|c| c.message.contains("advance merge base")),
        "{commits:?}"
    );
    assert!(
        commits.iter().any(|c| c.message.contains("solo: feature")),
        "{commits:?}"
    );
}

#[tokio::test]
async fn behind_remote_develop_base_without_pr() {
    let px = Px::new();
    let (_keep, clone, bare) = repo_with_remote();
    git(&clone, &["checkout", "-b", "develop"]);
    git(&clone, &["push", "-u", "origin", "develop"]);
    git(&clone, &["checkout", "main"]);
    project(&px, "pdev", &clone, "origin/main", false);
    let ws = px
        .eng()
        .create_workspace("pdev", "off develop", "origin/develop")
        .await
        .unwrap();
    assert_eq!(ws.base_branch, "origin/develop");
    let _dev = push_from_other(
        &bare,
        "develop",
        "develop-ahead.txt",
        "from develop\n",
        "advance develop",
    );
    let _main = push_from_other(
        &bare,
        "main",
        "main-only.txt",
        "from main\n",
        "advance main",
    );
    assert!(
        px.eng()
            .fetch_working_copy_upstream(&ws.id)
            .await
            .behind_base
            >= 1
    );
    let res = px
        .eng()
        .update_workspace_with_base(&ws.id, "")
        .await
        .unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(
        std::fs::read_to_string(Path::new(&ws.worktree_path).join("develop-ahead.txt"))
            .unwrap()
            .trim(),
        "from develop"
    );
    assert!(
        !Path::new(&ws.worktree_path).join("main-only.txt").exists(),
        "used origin/main"
    );
}

#[tokio::test]
async fn merge_with_forge_pr_does_not_land_locally() {
    let (px, repo, ws) = ws_fx("gh land").await;
    write(&ws, "feature.txt", "gh only\n");
    commit(&ws, "solo: add feature");
    px.add_pr(forge_pr("gh-99", 99, &ws));
    let err = px
        .eng()
        .merge_workspace(&ws.id, &MergeMethod::Squash)
        .await
        .unwrap_err();
    assert_eq!(err.0, "this workspace has a GitHub PR — merge from the PR");
    assert!(git_out(&repo.path, &["show", "HEAD:feature.txt"]).is_err());
}

#[tokio::test]
async fn forge_pr_ahead_and_behind_remote() {
    let px = Px::new();
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "acme", &clone, "main", false);
    let ws = px
        .eng()
        .create_workspace("acme", "add a feature", "")
        .await
        .unwrap();
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "solo: add feature");
    git(
        &ws.worktree_path,
        &[
            "push",
            "-u",
            "origin",
            &format!("HEAD:refs/heads/{}", ws.branch),
        ],
    );
    px.add_pr(forge_pr("gh-9", 9, &ws));
    write(&ws, "local.txt", "local\n");
    commit(&ws, "solo: local");
    assert!(px.eng().workspace_land_state(&ws.id).await.ahead_remote >= 1);
    git(
        &ws.worktree_path,
        &["reset", "--hard", &format!("origin/{}", ws.branch)],
    );
    let _other = push_from_other(
        &bare,
        &ws.branch,
        "from-github.txt",
        "remote\n",
        "from github",
    );
    assert!(
        px.eng()
            .fetch_working_copy_upstream(&ws.id)
            .await
            .behind_remote
            >= 1
    );
    let res = px.eng().pull_workspace_pr(&ws.id).await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(res.base, format!("origin/{}", ws.branch));
    assert_eq!(px.eng().workspace_land_state(&ws.id).await.behind_remote, 0);
    assert_eq!(
        std::fs::read_to_string(Path::new(&ws.worktree_path).join("from-github.txt"))
            .unwrap()
            .trim(),
        "remote"
    );
    let noop = px.eng().pull_workspace_pr(&ws.id).await.unwrap();
    assert_eq!(noop.status, UPDATE_WORKSPACE_NOOP);
}

#[tokio::test]
async fn pull_refuses_local_pr() {
    let (px, _repo, ws) = ws_fx("local only").await;
    assert_eq!(
        px.eng().pull_workspace_pr(&ws.id).await.unwrap_err().0,
        "no open pull request for this workspace"
    );
    px.add_pr(Pr {
        id: "7".into(),
        workspace_id: ws.id.clone(),
        source: PrSource::Local,
        state: "In review".into(),
        ..Pr::default()
    });
    assert_eq!(
        px.eng().pull_workspace_pr(&ws.id).await.unwrap_err().0,
        "this is a Local PR — there is no GitHub head to pull"
    );
}

async fn forge_project_ws(px: &Px, id: &str) -> (TempRepo, Workspace) {
    let repo = init_repo();
    project(px, id, &repo.path, "main", true);
    let ws = px
        .eng()
        .create_workspace(id, "add a feature", "")
        .await
        .unwrap();
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "solo: add feature");
    (repo, ws)
}

#[tokio::test]
async fn open_pr_retries_forge_after_local_fallback() {
    let px = Px::new();
    let (_repo, ws) = forge_project_ws(&px, "pgh").await;
    let id = px.eng().open_workspace_pr(&ws.id, false).await.unwrap();
    assert_eq!(
        px.pr(&id).source,
        PrSource::Local,
        "the default fake create fails"
    );
    let again = px
        .eng()
        .open_workspace_pr_with(&ws.id, "Retry title", "Retry body", false)
        .await
        .unwrap();
    assert_eq!(again, id);
    assert_eq!(px.pr(&id).source, PrSource::Local);
    assert_eq!(
        px.forge.calls_to("create_pr").len(),
        2,
        "the retry asked the forge again"
    );

    px.forge.on_create_pr(|_| async { Ok(77) });
    let promoted = px.eng().open_workspace_pr(&ws.id, false).await.unwrap();
    assert_eq!(promoted, id, "the Recur id stays put");
    let pr = px.pr(&id);
    assert_eq!((pr.source.clone(), pr.number), (PrSource::GitHub, 77));
    assert_eq!(pr.title, "Retry title");
}

#[tokio::test]
async fn open_pr_draft_persists_on_local_fallback() {
    let px = Px::new();
    let (_repo, ws) = forge_project_ws(&px, "pgh-draft").await;
    let id = px
        .eng()
        .open_workspace_pr_with(&ws.id, "Draft title", "Still cooking.", true)
        .await
        .unwrap();
    let pr = px.pr(&id);
    assert_eq!(pr.source, PrSource::Local);
    assert!(pr.draft);
    assert!(px.eng().workspace_land_state(&ws.id).await.pr_draft);
}

#[tokio::test]
async fn open_pr_on_forge_when_create_succeeds() {
    let px = Px::new();
    let (_repo, ws) = forge_project_ws(&px, "acme").await;
    px.forge.on_create_pr(|_| async { Ok(42) });
    let id = px
        .eng()
        .open_workspace_pr_with(&ws.id, "Agent title", "Agent body", true)
        .await
        .unwrap();
    assert_eq!(id, "gh-acme-42");
    let pr = px.pr(&id);
    assert_eq!(
        (pr.source.clone(), pr.number, pr.draft),
        (PrSource::GitHub, 42, true)
    );
    assert!(
        pr.comments.unwrap_or_default().is_empty(),
        "the forge owns the description"
    );
    let notes: Vec<String> = px
        .te
        .store
        .feed(&ws.id)
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.text)
        .collect();
    assert!(
        notes.iter().any(|n| n == "Opened GitHub draft PR #42."),
        "{notes:?}"
    );
}

const NO_GH: &str = "#!/bin/sh\nexit 1\n";

#[tokio::test]
async fn force_push_diverged_forge_pr() {
    let px = Px::new();
    let _gh = real_pushes(&px, NO_GH);
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "acme", &clone, "main", false);
    let ws = px
        .eng()
        .create_workspace("acme", "add a feature", "")
        .await
        .unwrap();
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "solo: add feature");
    git(
        &ws.worktree_path,
        &[
            "push",
            "-u",
            "origin",
            &format!("HEAD:refs/heads/{}", ws.branch),
        ],
    );
    px.add_pr(forge_pr("gh-11", 11, &ws));
    write(&ws, "rebased.txt", "after rebase\n");
    commit(&ws, "solo: rebased");
    let _other = push_from_other(
        &bare,
        &ws.branch,
        "stale.txt",
        "pre-rebase\n",
        "stale github tip",
    );
    let land = px.eng().fetch_working_copy_upstream(&ws.id).await;
    assert!(
        land.ahead_remote >= 1 && land.behind_remote >= 1,
        "{land:?}"
    );
    assert!(
        px.eng().open_workspace_pr(&ws.id, false).await.is_err(),
        "a regular push is refused"
    );
    assert!(
        git_out(
            &bare,
            &["cat-file", "-e", &format!("{}:stale.txt", ws.branch)]
        )
        .is_ok()
    );
    px.eng().open_workspace_pr(&ws.id, true).await.unwrap();
    let land = px.eng().fetch_working_copy_upstream(&ws.id).await;
    assert_eq!((land.ahead_remote, land.behind_remote), (0, 0));
    assert_eq!(
        show(&bare, &format!("{}:rebased.txt", ws.branch)),
        "after rebase"
    );
    assert!(
        git_out(
            &bare,
            &["cat-file", "-e", &format!("{}:stale.txt", ws.branch)]
        )
        .is_err()
    );
}

#[tokio::test]
async fn update_with_base_pushes_forge_pr() {
    let px = Px::new();
    let _gh = real_pushes(&px, NO_GH);
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "acme", &clone, "main", false);
    let ws = px
        .eng()
        .create_workspace("acme", "catch up", "")
        .await
        .unwrap();
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "solo: add feature");
    git(
        &ws.worktree_path,
        &[
            "push",
            "-u",
            "origin",
            &format!("HEAD:refs/heads/{}", ws.branch),
        ],
    );
    px.add_pr(forge_pr("gh-14", 14, &ws));
    std::fs::write(Path::new(&clone).join("from-main.txt"), "base moved\n").unwrap();
    git(&clone, &["add", "from-main.txt"]);
    git(&clone, &["commit", "-m", "advance main"]);
    let res = px
        .eng()
        .update_workspace_with_base(&ws.id, "rebase")
        .await
        .unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(res.pushed_pr, 14);
    assert!(res.force_pushed);
    assert_eq!(
        show(&bare, &format!("{}:from-main.txt", ws.branch)),
        "base moved"
    );
    let land = px.eng().fetch_working_copy_upstream(&ws.id).await;
    assert_eq!((land.ahead_remote, land.behind_remote), (0, 0));
}

#[tokio::test]
async fn update_with_base_merge_does_not_force_forge_pr() {
    let px = Px::new();
    let _gh = real_pushes(&px, NO_GH);
    let (_keep, clone, bare) = repo_with_remote();
    project(&px, "acme", &clone, "main", false);
    let ws = px
        .eng()
        .create_workspace("acme", "catch up", "")
        .await
        .unwrap();
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "solo: add feature");
    git(
        &ws.worktree_path,
        &[
            "push",
            "-u",
            "origin",
            &format!("HEAD:refs/heads/{}", ws.branch),
        ],
    );
    px.add_pr(forge_pr("gh-15", 15, &ws));
    let _other = push_from_other(
        &bare,
        &ws.branch,
        "stale.txt",
        "reviewer commit\n",
        "reviewer suggestion",
    );
    std::fs::write(Path::new(&clone).join("from-main.txt"), "base moved\n").unwrap();
    git(&clone, &["add", "from-main.txt"]);
    git(&clone, &["commit", "-m", "advance main"]);
    let res = px
        .eng()
        .update_workspace_with_base(&ws.id, "merge")
        .await
        .unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(
        res.pushed_pr, 0,
        "a regular push must not overwrite a remote-only commit"
    );
    assert!(!res.force_pushed);
    assert!(
        git_out(
            &bare,
            &["cat-file", "-e", &format!("{}:stale.txt", ws.branch)]
        )
        .is_ok()
    );
    assert!(Path::new(&ws.worktree_path).join("from-main.txt").exists());
    let notes: Vec<String> = px
        .te
        .store
        .feed(&ws.id)
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.text)
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("Updated with base but could not push to GitHub PR: ")),
        "{notes:?}"
    );
}

#[tokio::test]
async fn push_cross_repo_pr_pushes_to_fork() {
    let px = Px::new();
    let (keep, clone, bare) = repo_with_remote();
    let root = keep[3].path();
    let origin = root
        .join("celestiaorg")
        .join("recur.git")
        .to_string_lossy()
        .into_owned();
    let fork = root
        .join("auricom")
        .join("recur.git")
        .to_string_lossy()
        .into_owned();
    git(
        &root.to_string_lossy(),
        &["clone", "--bare", &bare, &origin],
    );
    git(&root.to_string_lossy(), &["clone", "--bare", &bare, &fork]);
    git(&clone, &["remote", "set-url", "origin", &origin]);
    project(&px, "acme", &clone, "main", false);
    let ws = px
        .eng()
        .create_workspace("acme", "fork PR", "")
        .await
        .unwrap();
    write(&ws, "feature.txt", "the feature\n");
    commit(&ws, "fork: feature");
    let head = "recur/tab-close-context-menu";
    git(
        &ws.worktree_path,
        &["push", &fork, &format!("HEAD:refs/heads/{head}")],
    );
    git(
        &ws.worktree_path,
        &["push", &origin, "HEAD:refs/pull/9/head"],
    );
    px.add_pr(Pr {
        branch: head.into(),
        is_cross_repository: true,
        ..forge_pr("gh-9", 9, &ws)
    });
    write(&ws, "local.txt", "local\n");
    commit(&ws, "fork: local");
    let land = px.eng().fetch_working_copy_upstream(&ws.id).await;
    assert_eq!(
        (land.ahead_remote, land.behind_remote),
        (1, 0),
        "vs pull/9/head"
    );

    let gh_dir = real_pushes(&px, NO_GH);
    let err = px.eng().open_workspace_pr(&ws.id, false).await.unwrap_err();
    assert!(err.0.contains("fork"), "{err}");
    drop(gh_dir);
    let _gh = real_pushes(
        &px,
        "#!/bin/sh\nif [ \"$1 $2 $3\" = \"pr view 9\" ]; then\n  echo '{\"headRepository\":{\"name\":\"recur\"},\"headRepositoryOwner\":{\"login\":\"auricom\"}}'\n  exit 0\nfi\nexit 1\n",
    );
    px.eng().open_workspace_pr(&ws.id, false).await.unwrap();
    assert_eq!(show(&fork, &format!("{head}:local.txt")), "local");
    assert!(
        git_out(
            &origin,
            &["rev-parse", "--verify", &format!("refs/heads/{head}")]
        )
        .is_err()
    );
    let land = px.eng().workspace_land_state(&ws.id).await;
    assert_eq!((land.ahead_remote, land.behind_remote), (0, 0));
}
