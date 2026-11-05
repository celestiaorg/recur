//! Send back, update with base, base retarget and sibling cleanup.

use std::time::Duration;

use recur_domain::{
    AutonomyLevel, Commit, DiffStat, FileDiff, PendingLineComment, Pr, PrComment, PrSource,
    UPDATE_WORKSPACE_CONFLICT, UPDATE_WORKSPACE_UPDATED, Workspace, WorkspaceState,
};

use super::support::*;
use crate::bus::EngineEvent;
use crate::loops::test_support::ConvergeRunner;
use crate::prs::rework::distill_instruction;
use crate::prs::workspace::format_open_pr_tool_result;
use crate::testing::{TestEngine, add_project, git, init_repo, wait_for};

#[tokio::test]
async fn send_back_is_a_conductor_turn() {
    let t = TestEngine::new(None);
    let ws = Workspace {
        id: "L-sendback".into(),
        project_id: "P".into(),
        goal: "Add OAuth".into(),
        state: WorkspaceState::PrOpen,
        run_started: true,
        loop_id: "recur.loops.pr".into(),
        ..Workspace::default()
    };
    t.store.add_workspace(ws.clone()).unwrap();
    t.store
        .add_pr(Pr {
            id: "pr-sb".into(),
            workspace_id: ws.id.clone(),
            number: 12,
            title: "Add OAuth".into(),
            state: "In review".into(),
            group: "you".into(),
            comments: Some(vec![
                PrComment {
                    author: "ada".into(),
                    body_md: "please rename the config key".into(),
                    ..PrComment::default()
                },
                PrComment {
                    author: "ada".into(),
                    path: "oauth.ts".into(),
                    line: 40,
                    body_md: "client secret read".into(),
                    ..PrComment::default()
                },
            ]),
            pending_comments: vec![PendingLineComment {
                file: "auth.go".into(),
                line: 8,
                body: "nit: unwrap this error".into(),
                ..PendingLineComment::default()
            }],
            ..Pr::default()
        })
        .unwrap();
    let (updated, ok) = t
        .engine
        .send_back_to_loop(
            "pr-sb",
            &["Resolve the client-secret read in oauth.ts".into()],
        )
        .await;
    assert!(ok);
    assert_eq!(updated.state, "In review");
    assert_eq!(
        t.store.workspace(&ws.id).unwrap().state,
        WorkspaceState::PrOpen
    );
    assert!(t.store.pr("pr-sb").unwrap().pending_comments.is_empty());
    let ctx = t.engine.maestro_context(&ws);
    let prompt = crate::roles::maestro_system_prompt(&ctx);
    for want in [
        "please rename the config key",
        "client secret read",
        "unwrap this error",
        "Resolve the client-secret read in oauth.ts",
        "reenter_phase",
    ] {
        assert!(
            prompt.contains(want) || ctx.pr_thread.contains(want),
            "conductor context missing {want:?}: {}",
            ctx.pr_thread
        );
    }
    assert!(t.store.pr("pr-sb").is_some());
}

#[tokio::test]
async fn maestro_send_back_refusals() {
    let t = TestEngine::new(None);
    let eng = &t.engine;
    assert_eq!(
        eng.maestro_send_back("L-x", "  ").await,
        "send_back refused: instruction is empty."
    );
    assert_eq!(
        eng.maestro_send_back("L-x", "fix it").await,
        "send_back refused: this loop has no open PR; use plan to steer while it's still running."
    );
    t.store
        .add_workspace(Workspace {
            id: "L-x".into(),
            run_started: true,
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_pr(Pr {
            id: "pr-x".into(),
            workspace_id: "L-x".into(),
            number: 3,
            state: "In review".into(),
            group: "you".into(),
            ..Pr::default()
        })
        .unwrap();
    assert_eq!(
        eng.maestro_send_back("L-x", "fix it").await,
        "Sent review back: Maestro will answer or call reenter_phase."
    );
}

#[test]
fn distill_instructions() {
    assert_eq!(
        distill_instruction(&[" ".into()]),
        "Address the reviewer's feedback on this PR with minimal, correct edits."
    );
    assert_eq!(
        distill_instruction(&["one".into()]),
        "Address this review comment: one"
    );
    assert_eq!(
        distill_instruction(&["a".into(), "".into(), "b".into()]),
        "Address the following review comments:\n- a\n- b"
    );
}

async fn walked(task: &str) -> (TestEngine, crate::testing::TempRepo, String, Pr) {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0, 0]).provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(&proj, task, "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(Duration::from_secs(10), "pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    wait_for(Duration::from_secs(5), "walk exit", || {
        t.engine.running_loop_by_id(&id).is_none()
    })
    .await;
    let pr = t.store.pr_by_workspace(&id).expect("a PR");
    (t, repo, id, pr)
}

#[tokio::test]
async fn refresh_behind_and_update_with_base() {
    let (t, repo, _id, pr) = walked("Feature before base moves").await;
    git(
        &repo.path,
        &["commit", "--allow-empty", "-m", "base moved ahead"],
    );
    t.engine.refresh_pr_behind(&pr.id).await;
    assert!(t.store.pr(&pr.id).unwrap().behind_base >= 1);
    let res = t.engine.update_pr_with_base(&pr.id, "").await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_UPDATED);
    let updated = t.store.pr(&pr.id).unwrap();
    assert_eq!(updated.behind_base, 0);
    assert_eq!(updated.updating_base, "");
    assert_eq!(
        (updated.group.as_str(), updated.state.as_str()),
        ("you", "In review")
    );

    git(
        &repo.path,
        &["commit", "--allow-empty", "-m", "base moved again"],
    );
    t.engine.refresh_pr_behind(&pr.id).await;
    t.engine
        .update_pr_with_base(&pr.id, "rebase")
        .await
        .unwrap();
    let rebased = t.store.pr(&pr.id).unwrap();
    assert_eq!(rebased.behind_base, 0);
    assert!(
        rebased
            .comments
            .iter()
            .flatten()
            .any(|c| c.body_html.contains("rebase") || c.body_md.contains("rebase")),
        "a rebase note even when the project default is merge"
    );
    t.close().await;
}

#[tokio::test]
async fn update_pr_with_base_conflict() {
    let (t, repo, id, pr) = walked("Feature that will conflict with base").await;
    let lp = t.store.workspace(&id).unwrap();
    let work = std::path::Path::new(&lp.worktree_path).join("work.txt");
    assert!(work.exists(), "the scripted coder wrote work.txt");
    std::fs::write(
        std::path::Path::new(&repo.path).join("work.txt"),
        "base side: conflicting change\n",
    )
    .unwrap();
    git(&repo.path, &["add", "work.txt"]);
    git(&repo.path, &["commit", "-m", "base conflicts with branch"]);
    t.engine.refresh_pr_behind(&pr.id).await;
    assert!(t.store.pr(&pr.id).unwrap().behind_base >= 1);
    let res = t.engine.update_pr_with_base(&pr.id, "").await.unwrap();
    assert_eq!(res.status, UPDATE_WORKSPACE_CONFLICT);
    let got = t.store.pr(&pr.id).unwrap();
    assert_eq!(got.updating_base, "");
    assert_eq!(got.group, "you");
    let mgr = recur_git::Manager::new(&repo.path, "");
    assert!(mgr.update_in_progress(&lp.worktree_path).await);

    std::fs::write(&work, "resolved by hand\n").unwrap();
    git(&lp.worktree_path, &["add", "work.txt"]);
    let res = t.engine.update_pr_with_base(&pr.id, "").await.unwrap();
    assert_eq!(
        res.status, UPDATE_WORKSPACE_CONFLICT,
        "a second click keeps the conflict"
    );
    assert!(
        mgr.update_in_progress(&lp.worktree_path).await,
        "not aborted"
    );
    assert_eq!(
        std::fs::read_to_string(&work).unwrap(),
        "resolved by hand\n"
    );
    t.close().await;
}

#[tokio::test]
async fn update_pr_with_base_refusals() {
    let t = TestEngine::new(None);
    let err = t
        .engine
        .update_pr_with_base("missing", "")
        .await
        .unwrap_err();
    assert_eq!(
        err.0,
        "update-with-base failed: the PR or its loop is unavailable"
    );
    t.store
        .add_pr(Pr {
            id: "busy".into(),
            updating_base: "git".into(),
            ..Pr::default()
        })
        .unwrap();
    let res = t.engine.update_pr_with_base("busy", "").await.unwrap();
    assert_eq!(res.status, "", "already in flight");
}

#[tokio::test]
async fn retarget_open_pr_keeps_snapshot_on_git_error() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    t.store
        .add_project(recur_domain::Project {
            id: "p".into(),
            name: "p".into(),
            repo_path: repo.path.clone(),
            base_branch: "main".into(),
            ..Default::default()
        })
        .unwrap();
    let missing = tempfile::tempdir().unwrap();
    let ws = Workspace {
        id: "w1".into(),
        project_id: "p".into(),
        base_branch: "main".into(),
        branch: "recur/w1".into(),
        worktree_path: missing
            .path()
            .join("missing-worktree")
            .to_string_lossy()
            .into_owned(),
        ..Workspace::default()
    };
    t.store.add_workspace(ws.clone()).unwrap();
    t.store
        .add_pr(Pr {
            id: "pr1".into(),
            workspace_id: "w1".into(),
            base: "main".into(),
            state: "In review".into(),
            created_at: "2026-08-01T12:00:00Z".into(),
            updated_at: "2026-08-01T12:00:00Z".into(),
            files: Some(vec![FileDiff {
                path: "feature.txt".into(),
                status: "a".into(),
                ..FileDiff::default()
            }]),
            file_count: 1,
            diff: DiffStat { add: 3, del: 0 },
            commits: Some(vec![Commit {
                hash: "abc1234".into(),
                message: "solo: add feature".into(),
                ..Commit::default()
            }]),
            ..Pr::default()
        })
        .unwrap();
    let mgr = recur_git::Manager::new(&repo.path, &missing.path().to_string_lossy());
    t.engine.retarget_open_pr(&mgr, &ws, "pr1", "develop").await;
    let got = t.store.pr("pr1").unwrap();
    assert_eq!(got.base, "develop");
    assert_eq!(got.files.unwrap()[0].path, "feature.txt");
    assert_eq!((got.diff.add, got.file_count), (3, 1));
    assert_eq!(got.commits.unwrap()[0].hash, "abc1234");
    assert_ne!(got.updated_at, "2026-08-01T12:00:00Z");
}

#[tokio::test]
async fn drop_unmerged_sibling_prs_keeps_merged_and_keep_id() {
    let t = TestEngine::new(None);
    let add = |id: &str, n: i64, source: PrSource, state: &str, group: &str| {
        t.store
            .add_pr(Pr {
                id: id.into(),
                workspace_id: "caramel".into(),
                number: n,
                source,
                state: state.into(),
                group: group.into(),
                ..Pr::default()
            })
            .unwrap();
    };
    add("4", 4, PrSource::Local, "In review", "you");
    add("gh-old", 100, PrSource::GitHub, "Merged", "merged");
    add("gh-156", 156, PrSource::GitHub, "In review", "you");
    t.engine.drop_unmerged_sibling_prs("caramel", "gh-156");
    assert!(t.store.pr("4").is_none());
    assert!(t.store.pr("gh-156").is_some());
    assert!(t.store.pr("gh-old").is_some());
    let removed = t.events.by_name("pr.removed");
    assert_eq!(removed.len(), 1);
    let EngineEvent::PrRemoved(r) = &removed[0] else {
        panic!("not pr.removed");
    };
    assert_eq!(r.pr_id, "4");
}

#[tokio::test]
async fn open_workspace_pr_after_rename() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "open-after-rename", "")
        .await
        .unwrap();
    std::fs::write(
        std::path::Path::new(&ws.worktree_path).join("feat.txt"),
        "x\n",
    )
    .unwrap();
    git(&ws.worktree_path, &["add", "-A"]);
    git(&ws.worktree_path, &["commit", "-m", "feat"]);
    git(
        &ws.worktree_path,
        &["branch", "-m", "mycodecrafting/screening-cache"],
    );
    let id = t.engine.open_workspace_pr(&ws.id, false).await.unwrap();
    assert_eq!(
        t.store.pr(&id).unwrap().branch,
        "mycodecrafting/screening-cache"
    );
    assert_eq!(
        t.store.workspace(&ws.id).unwrap().branch,
        "mycodecrafting/screening-cache"
    );
}

#[test]
fn open_pr_result_local_fallback_includes_reason() {
    let pr = Pr {
        number: 164,
        source: PrSource::Local,
        ..Pr::default()
    };
    let got = format_open_pr_tool_result(
        &pr,
        "GitHub",
        true,
        false,
        false,
        "gh pr create failed: HTTP 401",
    );
    assert!(got.contains("Opened Local PR #164"), "{got}");
    assert!(got.contains("HTTP 401"), "{got}");
    assert!(!got.contains("Opened GitHub PR"), "{got}");
    let retry = format_open_pr_tool_result(
        &pr,
        "GitHub",
        true,
        true,
        true,
        "git push failed: permission denied",
    );
    assert!(
        retry.contains("already open (Local)") && retry.contains("permission denied"),
        "{retry}"
    );
}

#[test]
fn open_pr_result_keeps_create_error_when_body_is_long() {
    let forge = recur_forge::testing::FakeForge::new();
    let body = "word ".repeat(80);
    let wrapped = format!(
        "gh pr create failed: gh pr create --head feat --base main --title Add feature --body {body}: GraphQL: Resource not accessible by integration"
    );
    let pr = Pr {
        number: 164,
        source: PrSource::Local,
        ..Pr::default()
    };
    let diag = recur_forge::Forge::failure_diag(&forge, &wrapped);
    let got = format_open_pr_tool_result(&pr, "GitHub", true, false, false, &diag);
    assert!(got.contains("Resource not accessible"), "{got}");
    assert!(!got.contains(&body[..40]), "{got}");
}

#[test]
fn open_pr_result_forge_cases() {
    let gh = |draft| Pr {
        number: 9,
        source: PrSource::GitHub,
        draft,
        ..Pr::default()
    };
    assert_eq!(
        format_open_pr_tool_result(&gh(false), "GitHub", true, false, false, ""),
        "Opened GitHub PR #9."
    );
    assert_eq!(
        format_open_pr_tool_result(&gh(true), "GitHub", true, false, false, ""),
        "Opened GitHub draft PR #9."
    );
    assert_eq!(
        format_open_pr_tool_result(&gh(false), "GitHub", true, true, true, ""),
        "Opened GitHub PR #9 (promoted from Local)."
    );
    assert_eq!(
        format_open_pr_tool_result(&gh(false), "GitHub", true, true, false, ""),
        "Pushed new commits to GitHub PR #9 (description unchanged)."
    );
    let local = Pr {
        number: 2,
        source: PrSource::Local,
        ..Pr::default()
    };
    assert_eq!(
        format_open_pr_tool_result(&local, "", false, true, true, ""),
        "PR #2 is already open (Local)."
    );
    assert_eq!(
        format_open_pr_tool_result(&local, "", false, false, false, ""),
        "Opened Local PR #2."
    );
    assert_eq!(
        format_open_pr_tool_result(&local, "GitHub", true, true, true, ""),
        "PR #2 is already open (Local). GitHub create failed; retry Open PR."
    );
    assert_eq!(
        format_open_pr_tool_result(&local, "GitHub", true, false, false, ""),
        "Opened Local PR #2 (GitHub create failed; retry Open PR)."
    );
}

#[tokio::test]
async fn open_pr_host_tool_reports_the_pr() {
    let px = Px::new();
    let repo = init_repo();
    px.add_project("acme", &repo.path);
    let ws = px
        .eng()
        .create_workspace("acme", "agent pr", "")
        .await
        .unwrap();
    std::fs::write(std::path::Path::new(&ws.worktree_path).join("a.txt"), "a\n").unwrap();
    git(&ws.worktree_path, &["add", "-A"]);
    git(&ws.worktree_path, &["commit", "-m", "a"]);
    let host = crate::host::EngineHost::new(std::sync::Arc::downgrade(px.eng()));
    let got =
        recur_agent_tools::Host::open_pr(&host, &ws.id, "PA", "Agent title", "Agent body", false)
            .await;
    assert!(got.starts_with("Opened Local PR #"), "{got}");
    let pr = px.te.store.pr_by_workspace(&ws.id).unwrap();
    assert_eq!(pr.title, "Agent title");
    assert_eq!(recur_forge::model::pr_description(&pr), "Agent body");
    let again = recur_agent_tools::Host::open_pr(&host, &ws.id, "PA", "", "", false).await;
    assert_eq!(again, format!("PR #{} is already open (Local).", pr.number));
    let refused = recur_agent_tools::Host::open_pr(&host, "missing", "PA", "", "", false).await;
    assert_eq!(refused, "open_pr: workspace \"missing\" not found");
}

async fn base_fx(name: &str) -> (Px, crate::testing::TempRepo, Workspace) {
    let px = Px::new();
    let repo = init_repo();
    git(&repo.path, &["branch", "develop"]);
    px.add_project("acme", &repo.path);
    let ws = px
        .eng()
        .create_workspace("acme", name, "main")
        .await
        .unwrap();
    assert_eq!(ws.base_branch, "main");
    (px, repo, ws)
}

#[tokio::test]
async fn set_workspace_base_branch() {
    let (px, _repo, ws) = base_fx("retarget me").await;
    let got = px
        .eng()
        .set_workspace_base_branch(&ws.id, "develop")
        .await
        .unwrap();
    assert_eq!(got.base_branch, "develop");
    assert_eq!(px.ws(&ws.id).base_branch, "develop");
    assert!(px.te.events.all().iter().any(|e| matches!(
        e,
        EngineEvent::WorkspaceFsChanged(f) if f.workspace_id == ws.id && f.rescan.is_none()
    )));
    let notes: Vec<String> = px
        .te
        .store
        .feed(&ws.id)
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.text)
        .collect();
    assert!(notes.contains(
        &"Target branch is now <code>develop</code>: diffs and PRs compare against it.".to_owned()
    ));
    let again = px
        .eng()
        .set_workspace_base_branch(&ws.id, "develop")
        .await
        .unwrap();
    assert_eq!(again.base_branch, "develop", "same branch is a no-op");
}

#[tokio::test]
async fn set_workspace_base_branch_refusals() {
    let (px, repo, ws) = base_fx("refusals").await;
    let eng = px.eng();
    for (id, base) in [
        (ws.id.as_str(), ""),
        (ws.id.as_str(), "  "),
        ("no-such", "develop"),
        (ws.id.as_str(), "does-not-exist"),
        (ws.id.as_str(), ws.branch.as_str()),
        (ws.id.as_str(), "HEAD"),
    ] {
        assert!(
            eng.set_workspace_base_branch(id, base).await.is_err(),
            "{id} {base:?}"
        );
    }
    let main = eng.open_main_workspace("acme").await.unwrap();
    assert!(
        eng.set_workspace_base_branch(&main.id, "develop")
            .await
            .is_err()
    );
    git(&repo.path, &["tag", "v1.0"]);
    assert!(
        eng.set_workspace_base_branch(&ws.id, "v1.0").await.is_err(),
        "a tag"
    );
    let sha = crate::testing::git_out(&repo.path, &["rev-parse", "HEAD"]).unwrap();
    assert!(
        eng.set_workspace_base_branch(&ws.id, sha.trim())
            .await
            .is_err(),
        "a sha"
    );
}

fn commit_feature(ws: &Workspace) {
    std::fs::write(
        std::path::Path::new(&ws.worktree_path).join("feature.txt"),
        "the feature\n",
    )
    .unwrap();
    git(&ws.worktree_path, &["add", "-A"]);
    git(&ws.worktree_path, &["commit", "-m", "solo: add feature"]);
}

#[tokio::test]
async fn set_workspace_base_branch_retargets_open_pr() {
    let (px, _repo, ws) = base_fx("open a pr").await;
    commit_feature(&ws);
    let id = px.eng().open_workspace_pr(&ws.id, false).await.unwrap();
    assert_eq!(px.pr(&id).base, "main");
    px.eng()
        .set_workspace_base_branch(&ws.id, "develop")
        .await
        .unwrap();
    assert_eq!(px.pr(&id).base, "develop");
}

#[tokio::test]
async fn set_workspace_base_branch_retargets_forge_pr_first() {
    let (px, _repo, ws) = base_fx("forge pr").await;
    px.add_pr(Pr {
        id: "gh-acme-5".into(),
        workspace_id: ws.id.clone(),
        number: 5,
        source: PrSource::GitHub,
        base: "main".into(),
        state: "In review".into(),
        ..Pr::default()
    });
    px.eng()
        .set_workspace_base_branch(&ws.id, "develop")
        .await
        .unwrap();
    assert_eq!(px.forge.calls_to("retarget_base")[0][1..], ["5", "develop"]);
    assert_eq!(px.pr("gh-acme-5").base, "develop");
    px.forge
        .on_retarget_base(|_| async { Err(recur_forge::ForgeError::failed("denied")) });
    let err = px
        .eng()
        .set_workspace_base_branch(&ws.id, "main")
        .await
        .unwrap_err();
    assert_eq!(err.0, "retarget GitHub PR: denied");
    assert_eq!(px.ws(&ws.id).base_branch, "develop", "nothing changed");
}

#[tokio::test]
async fn set_workspace_base_branch_leaves_merged_pr() {
    let (px, _repo, ws) = base_fx("merged pr").await;
    commit_feature(&ws);
    let id = px.eng().open_workspace_pr(&ws.id, false).await.unwrap();
    assert!(
        px.eng()
            .merge_pr(&id, &recur_domain::MergeMethod::default())
            .await
            .is_ok()
    );
    px.eng()
        .set_workspace_base_branch(&ws.id, "develop")
        .await
        .unwrap();
    assert_eq!(px.pr(&id).base, "main", "historical");
    assert_eq!(px.ws(&ws.id).base_branch, "develop");
}

#[tokio::test]
async fn set_workspace_base_branch_trims() {
    let (px, _repo, ws) = base_fx("trim").await;
    let got = px
        .eng()
        .set_workspace_base_branch(&ws.id, "  develop  ")
        .await
        .unwrap();
    assert_eq!(got.base_branch, "develop");
}

#[tokio::test]
async fn set_workspace_base_branch_accepts_remote_tracking() {
    let px = Px::new();
    let (_keep, clone) = clone_tracking("main", "main");
    px.add_project("acme", &clone);
    let ws = px
        .eng()
        .create_workspace("acme", "remote base", "main")
        .await
        .unwrap();
    let got = px
        .eng()
        .set_workspace_base_branch(&ws.id, "origin/main")
        .await
        .unwrap();
    assert_eq!(got.base_branch, "origin/main");
}
