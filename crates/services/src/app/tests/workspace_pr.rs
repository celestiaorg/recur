//! Workspace PR services.

use std::sync::Arc;
use std::time::{Duration, Instant};

use recur_bundles::PromptRunContext;
use recur_domain::{
    Finding, Pr, PrSource, Project, Severity, UPDATE_WORKSPACE_CONFLICT, UPDATE_WORKSPACE_NOOP,
    UPDATE_WORKSPACE_UPDATED, Workspace, WorkspaceState,
};
use recur_forge::Forge;
use recur_forge::testing::FakeForge;

use super::{Fx, FxOpts, git, git_out, read, write};

fn description(pr: &Pr) -> String {
    pr.comments
        .iter()
        .flatten()
        .find(|c| c.kind == "description")
        .map(|c| c.body_md.clone())
        .unwrap_or_default()
}

fn project(fx: &Fx, id: &str, repo: &str, base: &str, on_github: bool) -> String {
    fx.store()
        .add_project(Project {
            id: id.into(),
            name: id.into(),
            repo_path: repo.into(),
            base_branch: base.into(),
            pr_on_github: on_github,
            ..Default::default()
        })
        .unwrap()
        .id
}

fn gh_pr(id: &str, ws: &Workspace, number: i64) -> Pr {
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
        ..Default::default()
    }
}

fn shows(repo: &str, spec: &str) -> Option<String> {
    git_out(repo, &["show", spec])
        .ok()
        .map(|s| s.trim().to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_open_pr_and_merge() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "add a feature", "").await;
    let w = &fx.s.workspaces;
    assert!(
        w.open_workspace_pr(&ws.id, false).await.is_err(),
        "empty workspace"
    );
    fx.s.code
        .write_file(&pid, &ws.id, "feature.txt", "the feature\n")
        .await
        .unwrap();
    let e = w.open_workspace_pr(&ws.id, false).await.unwrap_err();
    assert!(e.message.contains("uncommitted"), "{e}");
    assert!(
        fx.s.code
            .commit(&pid, &ws.id, "solo: add feature")
            .await
            .unwrap()
            .committed
    );

    let pr_id = w.open_workspace_pr(&ws.id, false).await.unwrap();
    let pr = fx.s.prs.get_pr(&pr_id).await.unwrap();
    assert!(
        pr.workspace_id == ws.id && pr.source == PrSource::Local && pr.file_count > 0,
        "{pr:?}"
    );
    let d = description(&pr);
    assert!(
        d.contains("add a feature") && d.contains("## Changes"),
        "{d}"
    );
    let land = w.workspace_land_state(&ws.id).await.unwrap();
    assert_eq!(
        (land.pr_id.as_str(), land.pr_number),
        (pr_id.as_str(), pr.number)
    );
    assert_eq!(
        w.open_workspace_pr(&ws.id, false).await.unwrap(),
        pr_id,
        "idempotent"
    );

    let merged = fx.s.prs.merge_pr(&pr_id, "").await.unwrap();
    assert_eq!(
        (merged.state.as_str(), merged.group.as_str()),
        ("Merged", "merged")
    );
    assert_eq!(
        shows(&repo, &format!("{}:feature.txt", ws.base_branch)).as_deref(),
        Some("the feature")
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_open_pr_with_agent_body() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "scratch name", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    const BODY: &str = "This covers the whole workspace diff, not just this chat.";
    let eng = &fx.svc.engine;
    let pr_id = eng
        .open_workspace_pr_with(&ws.id, "Agent-written title", BODY, false)
        .await
        .unwrap();
    let pr = fx.s.prs.get_pr(&pr_id).await.unwrap();
    assert_eq!(pr.title, "Agent-written title");
    assert_eq!(description(&pr), BODY);

    fx.commit_file(&pid, &ws.id, "more.txt", "more\n", "solo: more")
        .await;
    fx.s.workspaces
        .open_workspace_pr(&ws.id, false)
        .await
        .unwrap();
    let again = fx.s.prs.get_pr(&pr_id).await.unwrap();
    assert_eq!(again.title, "Agent-written title");
    assert_eq!(description(&again), BODY);
    assert!(again.file_count >= 2, "{}", again.file_count);

    const RETRY: &str = "A later /open-pr on a Local PR must cover the whole workspace.";
    eng.open_workspace_pr_with(&ws.id, "A different title", RETRY, false)
        .await
        .unwrap();
    let rewritten = fx.s.prs.get_pr(&pr_id).await.unwrap();
    assert_eq!(rewritten.title, "A different title");
    assert_eq!(description(&rewritten), RETRY);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_open_pr_fallback_skips_prompt_action_wire() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "scratch name", "").await;
    let label = "Review & fix to a PR";
    fx.store()
        .update_workspace(&ws.id, |w| {
            w.goal = "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes, then open a PR.".into();
            w.goal_display = label.into();
        })
        .unwrap();
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    let pr_id = fx
        .svc
        .engine
        .open_workspace_pr(&ws.id, false)
        .await
        .unwrap();
    let pr = fx.s.prs.get_pr(&pr_id).await.unwrap();
    assert_eq!(pr.title, label);
    let d = description(&pr);
    assert!(
        !d.contains("The human already wrote") && !d.contains("Do not reopen scope"),
        "{d}"
    );
    assert!(d.contains("## Changes"), "{d}");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_merge_local() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "add a feature", "").await;
    let w = &fx.s.workspaces;
    assert!(
        w.merge_workspace(&ws.id, "").await.is_err(),
        "empty workspace"
    );
    assert_eq!(w.workspace_land_state(&ws.id).await.unwrap().ahead_base, 0);
    fx.s.code
        .write_file(&pid, &ws.id, "feature.txt", "the feature\n")
        .await
        .unwrap();
    assert!(w.workspace_land_state(&ws.id).await.unwrap().dirty);
    assert!(w.merge_workspace(&ws.id, "").await.is_err(), "uncommitted");
    assert!(
        fx.s.code
            .commit(&pid, &ws.id, "solo: add feature")
            .await
            .unwrap()
            .committed
    );
    assert!(w.workspace_land_state(&ws.id).await.unwrap().ahead_base >= 1);

    let merged = w.merge_workspace(&ws.id, "").await.unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert!(fx.store().pr_by_workspace(&ws.id).is_none(), "no PR");
    assert_eq!(
        shows(&repo, &format!("{}:feature.txt", ws.base_branch)).as_deref(),
        Some("the feature")
    );
    let again = w.merge_workspace(&ws.id, "").await.unwrap();
    assert_eq!(again.state, WorkspaceState::Merged);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_merge_refused_has_run() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "looped", "").await;
    fx.store()
        .update_workspace(&ws.id, |w| {
            w.run_started = true;
            w.state = WorkspaceState::Coding;
        })
        .unwrap();
    let w = &fx.s.workspaces;
    assert!(w.merge_workspace(&ws.id, "").await.is_err());
    assert!(w.update_workspace_with_base(&ws.id, "").await.is_err());
    assert!(w.open_workspace_pr(&ws.id, false).await.is_err());
    fx.store()
        .update_workspace(&ws.id, |w| {
            w.state = WorkspaceState::Paused;
            w.phase = "idle".into();
        })
        .unwrap();
    fx.commit_file(&pid, &ws.id, "idle.txt", "done\n", "idle: add file")
        .await;
    w.open_workspace_pr(&ws.id, false).await.unwrap();
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_update_with_base() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "catch up", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    let w = &fx.s.workspaces;
    let land = w.workspace_land_state(&ws.id).await.unwrap();
    assert!(
        land.behind_base == 0 && land.ahead_base >= 1 && !land.dirty && !land.unmerged,
        "{land:?}"
    );
    write(format!("{repo}/extra.txt"), "base moved\n");
    git(&repo, &["add", "extra.txt"]);
    git(&repo, &["commit", "-m", "advance main"]);
    assert!(w.merge_workspace(&ws.id, "").await.is_err(), "behind base");
    assert!(w.workspace_land_state(&ws.id).await.unwrap().behind_base >= 1);

    let res = w.update_workspace_with_base(&ws.id, "").await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(
        shows(&ws.worktree_path, "HEAD:extra.txt").as_deref(),
        Some("base moved")
    );
    assert_eq!(w.workspace_land_state(&ws.id).await.unwrap().behind_base, 0);
    let noop = w.update_workspace_with_base(&ws.id, "").await.unwrap();
    assert_eq!(noop.status, UPDATE_WORKSPACE_NOOP);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_update_with_base_conflict() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "conflict", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "README.md",
        "workspace edit\n",
        "solo: edit readme",
    )
    .await;
    write(format!("{repo}/README.md"), "main edit\n");
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "advance main with conflict"]);
    let w = &fx.s.workspaces;
    let res = w.update_workspace_with_base(&ws.id, "").await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_CONFLICT);
    assert!(w.workspace_land_state(&ws.id).await.unwrap().unmerged);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_remove() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "scratch", "").await;
    let path = ws.worktree_path.clone();
    let removed = fx.s.workspaces.remove_workspace(&ws.id).await.unwrap();
    assert!(removed.archived);
    assert!(removed.worktree_path.is_empty());
    assert!(
        fx.s.workspaces
            .get_workspace(&ws.id)
            .await
            .unwrap()
            .archived
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while std::path::Path::new(&path).exists() {
        assert!(Instant::now() < deadline, "worktree still at {path}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_workspace_pushes_remote_base() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = project(&fx, "premote", &clone, "origin/main", false);
    let ws = fx.solo(&pid, "land on origin", "").await;
    let w = &fx.s.workspaces;
    assert_eq!(
        w.workspace_land_state(&ws.id).await.unwrap().remote,
        "origin"
    );
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "from workspace\n",
        "solo: add feature",
    )
    .await;
    let merged = w.merge_workspace(&ws.id, "squash").await.unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert_eq!(
        shows(&bare, "HEAD:feature.txt").as_deref(),
        Some("from workspace")
    );
    fx.close().await;
}

async fn requires_pr_fixture(require: bool) -> Fx {
    let forge = Arc::new(FakeForge::new());
    forge
        .on_remote_repo(|_| async { Ok("acme/app".to_owned()) })
        .on_branch_requires_pr(move |_| async move { Some(require) });
    Fx::with(FxOpts {
        forges: Some(vec![forge as Arc<dyn Forge>]),
        ..FxOpts::default()
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_workspace_refused_when_github_requires_pr() {
    let fx = requires_pr_fixture(true).await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = project(&fx, "pghremote", &clone, "origin/main", true);
    let ws = fx.solo(&pid, "land on origin", "").await;
    let w = &fx.s.workspaces;
    assert!(w.workspace_land_state(&ws.id).await.unwrap().require_pr);
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "from workspace\n",
        "solo: add feature",
    )
    .await;
    let e = w.merge_workspace(&ws.id, "squash").await.unwrap_err();
    assert!(e.message.contains("requires a GitHub pull request"), "{e}");
    assert!(
        shows(&bare, "HEAD:feature.txt").is_none(),
        "landed on origin"
    );
    assert_ne!(
        fx.store().workspace(&ws.id).unwrap().state,
        WorkspaceState::Merged
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_workspace_github_remote_when_pr_not_required() {
    let fx = requires_pr_fixture(false).await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = project(&fx, "pghopen", &clone, "origin/main", true);
    let ws = fx.solo(&pid, "land on origin", "").await;
    let w = &fx.s.workspaces;
    assert!(!w.workspace_land_state(&ws.id).await.unwrap().require_pr);
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "from workspace\n",
        "solo: add feature",
    )
    .await;
    let merged = w.merge_workspace(&ws.id, "squash").await.unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert_eq!(
        shows(&bare, "HEAD:feature.txt").as_deref(),
        Some("from workspace")
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_workspace_undone_when_push_fails() {
    let fx = Fx::new().await;
    let (clone, _) = fx.init_git_repo_with_remote();
    let missing = format!("{}/missing.git", fx.temp());
    git(&clone, &["remote", "set-url", "origin", &missing]);
    let pid = project(&fx, "ppushfail", &clone, "origin/main", false);
    let ws = fx.solo(&pid, "land locally", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "from workspace\n",
        "solo: add feature",
    )
    .await;
    let w = &fx.s.workspaces;
    assert!(
        w.merge_workspace(&ws.id, "squash").await.is_err(),
        "push fails"
    );
    assert_ne!(
        fx.store().workspace(&ws.id).unwrap().state,
        WorkspaceState::Merged
    );
    assert!(
        w.merge_workspace(&ws.id, "squash").await.is_err(),
        "retry lands again and fails the push again"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_workspace_pushes_local_base_when_origin_exists() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = project(&fx, "plocalbase", &clone, "main", false);
    let ws = fx.solo(&pid, "land and push", "main").await;
    let w = &fx.s.workspaces;
    assert_eq!(
        w.workspace_land_state(&ws.id).await.unwrap().remote,
        "origin"
    );
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "from workspace\n",
        "solo: add feature",
    )
    .await;
    let merged = w.merge_workspace(&ws.id, "squash").await.unwrap();
    assert_eq!(merged.state, WorkspaceState::Merged);
    assert_eq!(
        shows(&bare, "HEAD:feature.txt").as_deref(),
        Some("from workspace")
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn behind_merge_base_fetches_remote_tip() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = project(&fx, "pbehind", &clone, "main", false);
    let ws = fx.solo(&pid, "feature", "main").await;
    fx.commit_file(&pid, &ws.id, "feature.txt", "mine\n", "solo: feature")
        .await;
    fx.store().add_pr(gh_pr("gh-111", &ws, 111)).unwrap();
    let other = fx.clone_other(&bare);
    write(format!("{other}/base-moved.txt"), "ahead\n");
    git(&other, &["add", "base-moved.txt"]);
    git(&other, &["commit", "-m", "advance merge base"]);
    git(&other, &["push", "origin", "main"]);

    let w = &fx.s.workspaces;
    assert_eq!(
        w.workspace_land_state(&ws.id).await.unwrap().behind_base,
        0,
        "stale local main before fetch"
    );
    fx.svc.engine.refresh_pr_behind("gh-111").await;
    assert!(fx.store().pr("gh-111").unwrap().behind_base >= 1);
    assert!(
        w.fetch_working_copy_upstream(&ws.id)
            .await
            .unwrap()
            .behind_base
            >= 1
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn get_pr_fetches_remote_base_before_commit_log() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.solo(&pid, "feature", "main").await;
    fx.commit_file(&pid, &ws.id, "feature.txt", "mine\n", "solo: feature")
        .await;
    let stale = git_out(&clone, &["rev-parse", "origin/main"]).unwrap();
    let other = fx.clone_other(&bare);
    write(format!("{other}/base-moved.txt"), "ahead\n");
    git(&other, &["add", "base-moved.txt"]);
    git(&other, &["commit", "-m", "advance merge base"]);
    git(&other, &["push", "origin", "main"]);
    git(&ws.worktree_path, &["fetch", "origin", "main"]);
    git(
        &ws.worktree_path,
        &["merge", "origin/main", "-m", "sync with base"],
    );
    git(
        &clone,
        &["update-ref", "refs/remotes/origin/main", stale.trim()],
    );
    fx.store()
        .add_pr(Pr {
            id: "pr-stale-base".into(),
            workspace_id: ws.id.clone(),
            group: "you".into(),
            source: PrSource::Local,
            branch: ws.branch.clone(),
            base: "origin/main".into(),
            title: "feat".into(),
            state: "In review".into(),
            ..Default::default()
        })
        .unwrap();
    let pr = fx.s.prs.get_pr("pr-stale-base").await.unwrap();
    let commits = pr.commits.unwrap_or_default();
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
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn behind_remote_develop_base_without_pr() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    git(&clone, &["checkout", "-b", "develop"]);
    git(&clone, &["push", "-u", "origin", "develop"]);
    git(&clone, &["checkout", "main"]);
    let pid = project(&fx, "pdev", &clone, "origin/main", false);
    let ws = fx.solo(&pid, "off develop", "origin/develop").await;
    assert_eq!(ws.base_branch, "origin/develop");
    assert!(fx.store().pr_by_workspace(&ws.id).is_none());

    let other = fx.clone_other(&bare);
    git(&other, &["checkout", "develop"]);
    write(format!("{other}/develop-ahead.txt"), "from develop\n");
    git(&other, &["add", "develop-ahead.txt"]);
    git(&other, &["commit", "-m", "advance develop"]);
    git(&other, &["push", "origin", "develop"]);
    write(format!("{other}/main-only.txt"), "from main\n");
    git(&other, &["checkout", "main"]);
    git(&other, &["add", "main-only.txt"]);
    git(
        &other,
        &["commit", "-m", "advance main, must not be the compare tip"],
    );
    git(&other, &["push", "origin", "main"]);

    let w = &fx.s.workspaces;
    assert!(
        w.fetch_working_copy_upstream(&ws.id)
            .await
            .unwrap()
            .behind_base
            >= 1
    );
    let res = w.update_workspace_with_base(&ws.id, "").await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(
        read(format!("{}/develop-ahead.txt", ws.worktree_path)).trim(),
        "from develop"
    );
    assert!(
        !std::path::Path::new(&format!("{}/main-only.txt", ws.worktree_path)).exists(),
        "update used origin/main instead of origin/develop"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_workspace_github_pr_does_not_land_locally() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = fx.add_project(&repo);
    let ws = fx.solo(&pid, "gh land", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "gh only\n",
        "solo: add feature",
    )
    .await;
    fx.store()
        .add_pr(Pr {
            id: "gh-99".into(),
            workspace_id: ws.id.clone(),
            number: 99,
            source: PrSource::GitHub,
            title: "gh".into(),
            state: "In review".into(),
            group: "you".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(
        fx.s.workspaces
            .merge_workspace(&ws.id, "squash")
            .await
            .is_err()
    );
    assert!(shows(&repo, "HEAD:feature.txt").is_none(), "landed locally");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_github_pr_ahead_and_behind_remote() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.solo(&pid, "add a feature", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    let head = format!("HEAD:refs/heads/{}", ws.branch);
    git(&ws.worktree_path, &["push", "-u", "origin", &head]);
    fx.store().add_pr(gh_pr("gh-9", &ws, 9)).unwrap();
    fx.commit_file(&pid, &ws.id, "local.txt", "local\n", "solo: local")
        .await;
    let w = &fx.s.workspaces;
    assert!(w.workspace_land_state(&ws.id).await.unwrap().ahead_remote >= 1);
    git(
        &ws.worktree_path,
        &["reset", "--hard", &format!("origin/{}", ws.branch)],
    );

    let other = fx.clone_other(&bare);
    git(&other, &["checkout", &ws.branch]);
    write(format!("{other}/from-github.txt"), "remote\n");
    git(&other, &["add", "from-github.txt"]);
    git(&other, &["commit", "-m", "from github"]);
    git(&other, &["push", "origin", "HEAD"]);

    assert!(
        w.fetch_working_copy_upstream(&ws.id)
            .await
            .unwrap()
            .behind_remote
            >= 1
    );
    let res = w.pull_workspace_pr(&ws.id).await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(
        w.workspace_land_state(&ws.id).await.unwrap().behind_remote,
        0
    );
    assert_eq!(
        read(format!("{}/from-github.txt", ws.worktree_path)).trim(),
        "remote"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fill_prompt_git_env_omits_local_pr_id() {
    let fx = Fx::new().await;
    let st = fx.store();
    let pid = project(&fx, "p", "/tmp/x", "main", false);
    let ws = |id: &str| Workspace {
        id: id.into(),
        project_id: pid.clone(),
        goal: "g".into(),
        base_branch: "main".into(),
        ..Default::default()
    };
    st.add_workspace(ws("W-local")).unwrap();
    st.add_pr(Pr {
        id: "7".into(),
        workspace_id: "W-local".into(),
        number: 7,
        source: PrSource::Local,
        state: "In review".into(),
        group: "you".into(),
        ..Default::default()
    })
    .unwrap();
    let mut rc = PromptRunContext::default();
    fx.svc.fill_prompt_git_env(&mut rc, "W-local");
    assert_eq!(rc.pr_id, "", "so /open-pr can retry");
    assert_eq!(rc.pr_number, 7);
    assert_eq!(rc.pr_source, PrSource::Local.as_str());

    st.add_workspace(ws("W-gh")).unwrap();
    st.add_pr(Pr {
        id: "gh-9".into(),
        workspace_id: "W-gh".into(),
        number: 9,
        source: PrSource::GitHub,
        state: "In review".into(),
        group: "you".into(),
        ..Default::default()
    })
    .unwrap();
    let mut gh = PromptRunContext::default();
    fx.svc.fill_prompt_git_env(&mut gh, "W-gh");
    assert_eq!(
        (gh.pr_id.as_str(), gh.pr_number, gh.pr_source.as_str()),
        ("gh-9", 9, PrSource::GitHub.as_str())
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fill_prompt_git_env_pr_chat_host() {
    let fx = Fx::new().await;
    let st = fx.store();
    let pid = project(&fx, "p", "/tmp/clone", "main", false);
    let pr = st
        .add_pr(Pr {
            id: "gh-p-4".into(),
            project_id: pid.clone(),
            number: 4,
            source: PrSource::GitHub,
            title: "Imported".into(),
            url: "https://github.com/acme/r/pull/4".into(),
            base: "main".into(),
            branch: "feat".into(),
            state: "Open".into(),
            group: "open".into(),
            ..Default::default()
        })
        .unwrap();
    st.add_workspace(Workspace {
        id: "W-host".into(),
        project_id: pid,
        goal: "Imported".into(),
        host_pr_id: pr.id.clone(),
        worktree_path: "/tmp/clone".into(),
        base_branch: "main".into(),
        ..Default::default()
    })
    .unwrap();
    let mut rc = PromptRunContext {
        surface: "workspace".into(),
        ..Default::default()
    };
    fx.svc.fill_prompt_git_env(&mut rc, "W-host");
    assert!(rc.pr_host);
    assert_eq!(rc.surface, "github_pr");
    assert_eq!(
        (rc.pr_id.as_str(), rc.pr_number, rc.pr_url.as_str()),
        (pr.id.as_str(), 4, pr.url.as_str())
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_pr_findings_uses_chat_host() {
    let fx = Fx::new().await;
    let st = fx.store();
    let pid = project(&fx, "p", "/tmp/clone", "main", false);
    st.add_pr(Pr {
        id: "gh-p-5".into(),
        project_id: pid.clone(),
        number: 5,
        source: PrSource::GitHub,
        state: "Open".into(),
        group: "open".into(),
        ..Default::default()
    })
    .unwrap();
    st.add_workspace(Workspace {
        id: "W-host-f".into(),
        project_id: pid,
        goal: "PR".into(),
        host_pr_id: "gh-p-5".into(),
        worktree_path: "/tmp/clone".into(),
        ..Default::default()
    })
    .unwrap();
    st.add_finding(Finding {
        id: "f1".into(),
        workspace_id: "W-host-f".into(),
        file: "a.go".into(),
        line: 1,
        severity: Severity::High,
        message: "bug".into(),
        status: "open".into(),
        ..Default::default()
    });
    let got = fx.s.prs.list_pr_findings("gh-p-5").await.unwrap();
    assert_eq!(
        got.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
        ["f1"]
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_open_pr_retries_github_after_local_fallback() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = project(&fx, "pgh", &repo, "main", true);
    let ws = fx.solo(&pid, "add a feature", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    let pr_id =
        fx.s.workspaces
            .open_workspace_pr(&ws.id, false)
            .await
            .unwrap();
    let pr = fx.s.prs.get_pr(&pr_id).await.unwrap();
    assert_eq!(
        pr.source,
        PrSource::Local,
        "local fallback when gh create fails"
    );
    let again = fx
        .svc
        .engine
        .open_workspace_pr_with(&ws.id, "Retry title", "Retry body", false)
        .await
        .unwrap();
    assert_eq!(again, pr_id);
    assert_eq!(
        fx.s.prs.get_pr(&pr_id).await.unwrap().source,
        PrSource::Local
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_open_pr_draft_persists_on_local_fallback() {
    let fx = Fx::new().await;
    let repo = fx.init_git_repo();
    let pid = project(&fx, "pgh-draft", &repo, "main", true);
    let ws = fx.solo(&pid, "add a feature", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    let pr_id = fx
        .svc
        .engine
        .open_workspace_pr_with(&ws.id, "Draft title", "Still cooking.", true)
        .await
        .unwrap();
    let pr = fx.s.prs.get_pr(&pr_id).await.unwrap();
    assert_eq!(pr.source, PrSource::Local);
    assert!(pr.draft, "a later GitHub retry stays a draft");
    assert!(
        fx.s.workspaces
            .workspace_land_state(&ws.id)
            .await
            .unwrap()
            .pr_draft
    );
    fx.close().await;
}

/// One remote-only commit on the workspace branch, pushed from a second clone.
fn diverge_remote(fx: &Fx, bare: &str, ws: &Workspace, file: &str, msg: &str) {
    let other = fx.clone_other(bare);
    git(&other, &["checkout", &ws.branch]);
    write(format!("{other}/{file}"), "remote only\n");
    git(&other, &["add", file]);
    git(&other, &["commit", "-m", msg]);
    git(&other, &["push", "origin", "HEAD"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_force_push_diverged_github_pr() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.solo(&pid, "add a feature", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    let head = format!("HEAD:refs/heads/{}", ws.branch);
    git(&ws.worktree_path, &["push", "-u", "origin", &head]);
    fx.store().add_pr(gh_pr("gh-11", &ws, 11)).unwrap();
    fx.commit_file(
        &pid,
        &ws.id,
        "rebased.txt",
        "after rebase\n",
        "solo: rebased",
    )
    .await;
    diverge_remote(&fx, &bare, &ws, "stale.txt", "stale github tip");

    let w = &fx.s.workspaces;
    let land = w.fetch_working_copy_upstream(&ws.id).await.unwrap();
    assert!(
        land.ahead_remote >= 1 && land.behind_remote >= 1,
        "{land:?}"
    );
    let stale = format!("{}:stale.txt", ws.branch);
    assert!(
        w.open_workspace_pr(&ws.id, false).await.is_err(),
        "regular push refused"
    );
    git_out(&bare, &["cat-file", "-e", &stale]).expect("remote-only stale.txt kept");

    w.open_workspace_pr(&ws.id, true).await.unwrap();
    let land = w.fetch_working_copy_upstream(&ws.id).await.unwrap();
    assert_eq!((land.ahead_remote, land.behind_remote), (0, 0));
    assert_eq!(
        shows(&bare, &format!("{}:rebased.txt", ws.branch)).as_deref(),
        Some("after rebase")
    );
    assert!(
        git_out(&bare, &["cat-file", "-e", &stale]).is_err(),
        "stale.txt survived"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_update_with_base_pushes_github_pr() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.solo(&pid, "catch up", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    let head = format!("HEAD:refs/heads/{}", ws.branch);
    git(&ws.worktree_path, &["push", "-u", "origin", &head]);
    fx.store().add_pr(gh_pr("gh-14", &ws, 14)).unwrap();
    write(format!("{clone}/from-main.txt"), "base moved\n");
    git(&clone, &["add", "from-main.txt"]);
    git(&clone, &["commit", "-m", "advance main"]);

    let w = &fx.s.workspaces;
    let res = w
        .update_workspace_with_base(&ws.id, "rebase")
        .await
        .unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(res.pushed_pr, 14, "auto-push after rebase");
    assert!(res.force_pushed);
    assert_eq!(
        shows(&bare, &format!("{}:from-main.txt", ws.branch)).as_deref(),
        Some("base moved")
    );
    let land = w.fetch_working_copy_upstream(&ws.id).await.unwrap();
    assert_eq!((land.ahead_remote, land.behind_remote), (0, 0));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_update_with_base_merge_does_not_force_github_pr() {
    let fx = Fx::new().await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let pid = fx.add_project(&clone);
    let ws = fx.solo(&pid, "catch up", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "solo: add feature",
    )
    .await;
    let head = format!("HEAD:refs/heads/{}", ws.branch);
    git(&ws.worktree_path, &["push", "-u", "origin", &head]);
    fx.store().add_pr(gh_pr("gh-15", &ws, 15)).unwrap();
    diverge_remote(&fx, &bare, &ws, "stale.txt", "reviewer suggestion");
    write(format!("{clone}/from-main.txt"), "base moved\n");
    git(&clone, &["add", "from-main.txt"]);
    git(&clone, &["commit", "-m", "advance main"]);

    let res =
        fx.s.workspaces
            .update_workspace_with_base(&ws.id, "merge")
            .await
            .unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    assert_eq!(
        res.pushed_pr, 0,
        "regular push must not overwrite a remote-only commit"
    );
    assert!(!res.force_pushed);
    git_out(
        &bare,
        &["cat-file", "-e", &format!("{}:stale.txt", ws.branch)],
    )
    .expect("remote-only stale.txt kept");
    assert!(std::path::Path::new(&format!("{}/from-main.txt", ws.worktree_path)).exists());
    fx.close().await;
}

/// A fork PR (head in auricom/recur): pushing lands on the fork, never
/// creates the branch on origin, and ahead/behind compares against
/// pull/N/head.
#[tokio::test(flavor = "multi_thread")]
async fn workspace_push_cross_repo_pr_pushes_to_fork() {
    let gh_dir = tempfile::tempdir().unwrap();
    let gh = gh_dir.path().join("gh");
    let set_gh = |body: &str| {
        recur_exec::write_executable(&gh, body, 0o755).unwrap();
    };
    set_gh("#!/bin/sh\nexit 1\n");
    let forge = recur_forge::github::GitHub::with_gh(recur_forge::Gh::new(&gh));
    let fx = Fx::with(FxOpts {
        forges: Some(vec![Arc::new(forge) as Arc<dyn Forge>]),
        ..FxOpts::default()
    })
    .await;
    let (clone, bare) = fx.init_git_repo_with_remote();
    let root = fx.temp();
    let origin = format!("{root}/celestiaorg/recur.git");
    let fork = format!("{root}/auricom/recur.git");
    git(&root, &["clone", "--bare", &bare, &origin]);
    git(&root, &["clone", "--bare", &bare, &fork]);
    git(&clone, &["remote", "set-url", "origin", &origin]);

    let pid = fx.add_project(&clone);
    let ws = fx.solo(&pid, "fork PR", "").await;
    fx.commit_file(
        &pid,
        &ws.id,
        "feature.txt",
        "the feature\n",
        "fork: feature",
    )
    .await;
    const HEAD: &str = "recur/tab-close-context-menu";
    git(
        &ws.worktree_path,
        &["push", &fork, &format!("HEAD:refs/heads/{HEAD}")],
    );
    git(
        &ws.worktree_path,
        &["push", &origin, "HEAD:refs/pull/9/head"],
    );
    fx.store()
        .add_pr(Pr {
            branch: HEAD.into(),
            is_cross_repository: true,
            ..gh_pr("gh-9", &ws, 9)
        })
        .unwrap();
    fx.commit_file(&pid, &ws.id, "local.txt", "local\n", "fork: local")
        .await;

    let w = &fx.s.workspaces;
    let land = w.fetch_working_copy_upstream(&ws.id).await.unwrap();
    assert_eq!(
        (land.ahead_remote, land.behind_remote),
        (1, 0),
        "vs pull/9/head"
    );

    let e = w.open_workspace_pr(&ws.id, false).await.unwrap_err();
    assert!(e.message.contains("fork"), "{e}");

    set_gh(
        r#"#!/bin/sh
if [ "$1 $2 $3" = "pr view 9" ]; then
  echo '{"headRepository":{"name":"recur"},"headRepositoryOwner":{"login":"auricom"}}'
  exit 0
fi
exit 1
"#,
    );
    w.open_workspace_pr(&ws.id, false).await.unwrap();
    assert_eq!(
        shows(&fork, &format!("{HEAD}:local.txt")).as_deref(),
        Some("local")
    );
    assert!(
        git_out(
            &origin,
            &["rev-parse", "--verify", &format!("refs/heads/{HEAD}")]
        )
        .is_err(),
        "origin gained the fork branch"
    );
    let land = w.workspace_land_state(&ws.id).await.unwrap();
    assert_eq!((land.ahead_remote, land.behind_remote), (0, 0));
    fx.close().await;
}
