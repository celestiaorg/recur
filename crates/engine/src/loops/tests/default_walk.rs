//! Default walk and fixture loop tests: an untouched dock walks the shipped To
//! a PR graph, an unresolvable guard idles with its reason, and the fixture
//! pack's converge / docs / verify loops.

use std::collections::HashSet;
use std::time::Duration;

use recur_bundles::{DEFAULT_LOOP_ID, SECURITY_LOOP_ID};
use recur_domain::{AutonomyLevel, Finding, FindingStatus, Severity, WorkspaceState};

use crate::loops::test_support::*;
use crate::testing::{TestEngine, add_project, git_out, init_repo, wait_for};

fn agent_codes(t: &TestEngine, id: &str) -> HashSet<String> {
    t.store.agents(id).into_iter().map(|a| a.code).collect()
}

fn idle(t: &TestEngine, id: &str) -> bool {
    t.store
        .workspace(id)
        .is_some_and(|l| l.state == WorkspaceState::Paused || l.state == WorkspaceState::Stopped)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engage_without_pick_walks_default_graph() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_shipped_loop_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(&proj, "Default-pick run", "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(
        Duration::from_secs(20),
        "default-pick run to settle",
        || {
            t.store.workspace(&id).is_some_and(|l| {
                matches!(
                    l.state,
                    WorkspaceState::PrOpen | WorkspaceState::Failed | WorkspaceState::Stopped
                )
            })
        },
    )
    .await;
    let ws = t.store.workspace(&id).unwrap();
    assert_eq!(ws.state, WorkspaceState::PrOpen, "phase {}", ws.phase);
    assert_eq!(ws.loop_id, DEFAULT_LOOP_ID);
    assert!(!ws.policy_id.is_empty());
    assert!(
        agent_codes(&t, &id).contains("PL"),
        "the planner runs as the shipped Planner role: {:?}",
        agent_codes(&t, &id)
    );
    t.close().await;
}

#[tokio::test]
async fn unresolvable_guard_idles_with_reason() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[]).provider()));
    let proj = add_project(&t.store, &repo.path);
    let (mut rec, acquired) = t
        .engine
        .queue_start_workspace(
            &proj,
            "bad loop",
            "",
            "",
            &AutonomyLevel::default(),
            "",
            "",
            None,
        )
        .await
        .unwrap();
    assert!(acquired);
    rec.loop_id = "nobody.loops.missing".into();
    let id = rec.id.clone();
    t.engine.dispatch(rec).await.expect("dispatch");
    wait_for(Duration::from_secs(10), "unresolvable loop to idle", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Paused && l.phase == "loop refs missing")
    })
    .await;
    t.close().await;
}

// ---- fixture

#[test]
fn fixture_pack_parses() {
    let dir = tempfile::tempdir().unwrap();
    let parsed = recur_bundles::parse_manifest_full(
        dir.path().to_str().unwrap(),
        crate::testing::TEST_LOOPS_TOML.as_bytes(),
    )
    .expect("parse");
    assert!(recur_bundles::find_loop(&parsed.loops, "test.loops.docs").is_some());
    assert!(recur_bundles::find_role(&parsed.roles, "test.roles.writer").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn converge_start_review_skips_first_coder() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[1, 0]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "Review the current diff",
            "",
            AutonomyLevel::default(),
            "test.loops.converge",
            "",
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(15), "converge loop to idle", || {
        idle(&t, &id)
    })
    .await;
    assert_ne!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::PrOpen
    );
    assert_eq!(runner.counts(), (1, 2), "fix only, two reviews");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn converge_pr_start_review_commits_uncommitted_diff() {
    let repo = init_repo();
    let runner = std::sync::Arc::new(ConvergeRunner {
        findings_per_iter: vec![0],
        commit_text: "Add human.txt".into(),
        ..ConvergeRunner::default()
    });
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "Ship the human diff", "")
        .await
        .expect("create");
    assert!(!ws.worktree_path.is_empty());
    std::fs::write(
        std::path::Path::new(&ws.worktree_path).join("human.txt"),
        "human change\n",
    )
    .unwrap();
    t.engine
        .remember_chat_loop_policy(&ws.id, "PA", "test.loops.converge-pr", "")
        .await;
    let reply = t
        .engine
        .send_loop_chat_message(
            t.store.workspace(&ws.id).unwrap(),
            "PA",
            "review and open a PR",
            &[],
            "Review & fix to a PR",
        )
        .await;
    assert!(!reply.contains("I couldn't engage"), "{reply}");
    wait_for(
        Duration::from_secs(15),
        "converge-pr to reach pr_open",
        || {
            t.store
                .workspace(&ws.id)
                .is_some_and(|l| l.state == WorkspaceState::PrOpen)
        },
    )
    .await;
    assert_eq!(runner.counts(), (0, 1));
    let pr = t.store.pr_by_workspace(&ws.id).expect("a PR");
    let files = pr.files.clone().unwrap_or_default();
    let human = files
        .iter()
        .find(|f| f.path == "human.txt")
        .expect("human.txt in the PR");
    assert!(human.diff.add > 0);
    assert!(!pr.commits.clone().unwrap_or_default().is_empty());
    let log = git_out(&ws.worktree_path, &["log", "--oneline", "main..HEAD"]).unwrap();
    assert!(log.contains("Add human.txt"), "{log}");
    let status = git_out(&ws.worktree_path, &["status", "--porcelain"]).unwrap();
    assert!(status.trim().is_empty(), "{status}");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn converge_bar_uses_all_open_findings() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0, 0]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "leftover medium finding", "")
        .await
        .unwrap();
    t.store.add_finding(Finding {
        id: "old-medium".into(),
        workspace_id: ws.id.clone(),
        iteration: 8,
        file: "engine.go".into(),
        line: 1,
        severity: Severity::Medium,
        message: "queued close race".into(),
        status: FindingStatus::Open.as_str().into(),
        ..Finding::default()
    });
    t.engine
        .remember_chat_loop_policy(&ws.id, "PA", "test.loops.converge", "test.policies.fail")
        .await;
    let reply = t
        .engine
        .send_loop_chat_message(
            t.store.workspace(&ws.id).unwrap(),
            "PA",
            "fix leftover",
            &[],
            "Review & fix",
        )
        .await;
    assert!(!reply.contains("I couldn't engage"), "{reply}");
    wait_for(
        Duration::from_secs(15),
        "converge to idle after the leftover fix",
        || idle(&t, &ws.id),
    )
    .await;
    let (coder, review) = runner.counts();
    assert!(review >= 1 && coder >= 1, "coder={coder} review={review}");
    assert!(review >= 2, "re-review after the fix: {review}");
    assert_eq!(t.store.open_findings_count(&ws.id), 0);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn docs_loop_converges_without_pr() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
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
    wait_for(Duration::from_secs(15), "docs loop to idle", || {
        idle(&t, &id)
    })
    .await;
    assert_ne!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::PrOpen
    );
    let codes = agent_codes(&t, &id);
    assert!(!codes.contains("CO"), "{codes:?}");
    assert!(codes.contains("WR") && codes.contains("ED"), "{codes:?}");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shipped_security_loop_uses_security_reviewer() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_shipped_loop_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "Security look at this diff",
            "",
            AutonomyLevel::default(),
            SECURITY_LOOP_ID,
            "",
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(15), "security loop to idle", || {
        idle(&t, &id)
    })
    .await;
    assert_ne!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::PrOpen
    );
    let codes = agent_codes(&t, &id);
    assert!(!codes.contains("RV"), "{codes:?}");
    assert!(codes.contains("SR"), "{codes:?}");
    t.close().await;
}

#[tokio::test]
async fn engage_missing_loop_refused() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[]).provider()));
    let proj = add_project(&t.store, &repo.path);
    assert!(
        t.engine
            .start_workspace_with(
                &proj,
                "gone loop",
                "",
                AutonomyLevel::default(),
                "test.loops.does-not-exist",
                ""
            )
            .await
            .is_err()
    );
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_missing_loop_idles() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "docs then delete the pack",
            "",
            AutonomyLevel::default(),
            "test.loops.docs",
            "",
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(15), "docs loop to idle", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Paused)
    })
    .await;
    std::fs::remove_dir_all(
        std::path::Path::new(&t.engine.bundle_roots().default_dir).join("TestLoops"),
    )
    .unwrap();
    t.engine.resume(&id).await;
    wait_for(
        Duration::from_secs(10),
        "resume of the deleted loop to idle",
        || {
            t.store
                .workspace(&id)
                .is_some_and(|l| l.state == WorkspaceState::Paused)
        },
    )
    .await;
    t.close().await;
}
