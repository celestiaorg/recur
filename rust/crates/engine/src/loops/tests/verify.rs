//! Port of Go `verify_test.go`: the verify phase boots the dev server, the
//! QA agent files evidence over the real MCP path, a pass advances to
//! pr_open, a fail or a missing run config blocks it, and a graph without
//! verify never runs QA.

use std::sync::Arc;
use std::time::Duration;

use recur_agent_tools::names::mcp;
use recur_domain::{AgentRole, AutonomyLevel, LINEAR_MCP_READ_ONLY_URL, WorkspaceState};
use recur_provider_claude::args::mcp_config_json;

use crate::bus::EngineEvent;
use crate::loops::test_support::*;
use crate::roles::MCP_PLAYWRIGHT_ALL;
use crate::testing::{TestEngine, TestOptions, add_project, init_repo, wait_for};

fn ev(step: &'static str, shot: &'static str, passed: bool) -> StepEvidence {
    StepEvidence { step, shot, passed }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_pass_advances_to_pr_open() {
    let repo = init_repo();
    let runner = Arc::new(ConvergeRunner {
        findings_per_iter: vec![0],
        evidence_per_run: vec![vec![
            ev("Booted the app and opened /checkout", "/shots/01.png", true),
            ev(
                "Signed in with Apple → valid session",
                "/shots/02.png",
                true,
            ),
            ev("Reloaded — session persisted", "/shots/03.png", true),
        ]],
        file_body: "edit\n",
        ..ConvergeRunner::default()
    });
    let t = TestEngine::with(TestOptions {
        provider: Some(runner.provider()),
        trackers: linear_trackers(),
        ..TestOptions::default()
    });
    t.install_fixture_pack();
    enable_linear_mcp(&t.store);
    let Some(proj) = verify_dev_project(&t.store, &repo.path) else {
        eprintln!("python3 not available; verify needs a bootable dev server");
        return;
    };
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "Add a verifiable feature",
            "",
            AutonomyLevel::default(),
            "test.loops.verify",
            "",
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(25), "verify and reach pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;

    let (qa_runs, plw, tools, qa_spec) = {
        let s = runner.state.lock();
        let qa = s.specs.iter().find(|r| r.role == AgentRole::Qa).cloned();
        (s.qa_runs, s.qa_playwright, s.qa_tools.clone(), qa)
    };
    assert_eq!(qa_runs, 1);
    assert!(plw, "QA requests the Playwright MCP");
    let has = |t: &str| tools.iter().any(|x| x == t);
    for want in [
        mcp::ATTACH_EVIDENCE,
        MCP_PLAYWRIGHT_ALL,
        mcp::REPORT_RESULT,
        "Read",
    ] {
        assert!(has(want), "QA tools missing {want}: {tools:?}");
    }
    for banned in ["Edit", "Write", "NotebookEdit"] {
        assert!(!has(banned), "QA must not have {banned}");
    }
    let cfg = mcp_config_json(&qa_spec.unwrap());
    for want in [r#""recur""#, r#""playwright""#, "@playwright/mcp"] {
        assert!(cfg.contains(want), "{cfg}");
    }
    assert!(cfg.contains(LINEAR_MCP_READ_ONLY_URL), "{cfg}");

    let l = t.store.workspace(&id).unwrap();
    assert_eq!(l.verify_state, "passed");
    assert_eq!(l.verify.len(), 3);
    assert!(l.verify.iter().all(|v| v.passed));
    assert_eq!(l.verify[0].screenshot, "/shots/01.png");
    let saw_verifying = t.events.by_name("workspace.updated").iter().any(
        |e| matches!(e, EngineEvent::WorkspaceUpdated(u) if u.state == WorkspaceState::Verifying),
    );
    assert!(saw_verifying, "a verifying state in the pipeline");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_fail_blocks_pr_open() {
    let repo = init_repo();
    let fail_run = vec![
        ev("Opened /checkout", "/shots/a.png", true),
        ev("Sign in with Google returned a 500", "/shots/b.png", false),
    ];
    let runner = Arc::new(ConvergeRunner {
        findings_per_iter: vec![0; 6],
        evidence_per_run: vec![fail_run; 6],
        file_body: "edit\n",
        ..ConvergeRunner::default()
    });
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let Some(proj) = verify_dev_project(&t.store, &repo.path) else {
        eprintln!("python3 not available; verify needs a bootable dev server");
        return;
    };
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "feature that never verifies",
            "",
            AutonomyLevel::default(),
            "test.loops.verify",
            "test.policies.fail",
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(40), "fail without opening a PR", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Failed)
    })
    .await;
    let l = t.store.workspace(&id).unwrap();
    assert_eq!(l.verify_state, "failed");
    assert!(runner.state.lock().qa_runs >= 1);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_rail_off_no_verify_required() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(&proj, "no-verify feature", "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(Duration::from_secs(10), "pr_open without verify", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    assert_eq!(runner.state.lock().qa_runs, 0);
    assert_eq!(t.store.workspace(&id).unwrap().verify_state, "");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_absent_run_config_blocks_pr() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0; 6]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "feature with no run config",
            "",
            AutonomyLevel::default(),
            "test.loops.verify",
            "test.policies.fail",
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(20), "fail with no run config", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::Failed)
    })
    .await;
    assert_eq!(t.store.workspace(&id).unwrap().verify_state, "failed");
    assert_eq!(runner.state.lock().qa_runs, 0, "no dev server to drive");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_on_reject_reenters_converge() {
    let repo = init_repo();
    let runner = Arc::new(ConvergeRunner {
        findings_per_iter: vec![0, 0],
        evidence_per_run: vec![
            vec![ev("sign-in 500", "/shots/a.png", false)],
            vec![ev("sign-in ok", "/shots/b.png", true)],
        ],
        ..ConvergeRunner::default()
    });
    let t = TestEngine::with(TestOptions {
        provider: Some(runner.provider()),
        ..TestOptions::default()
    });
    t.install_fixture_pack();
    let Some(proj) = verify_dev_project(&t.store, &repo.path) else {
        eprintln!("python3 not available; verify needs a bootable dev server");
        return;
    };
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "verify then fix",
            "",
            AutonomyLevel::default(),
            "test.loops.verify-reject",
            "",
        )
        .await
        .unwrap();
    wait_for(
        Duration::from_secs(40),
        "on_reject loop to open a PR",
        || {
            t.store
                .workspace(&id)
                .is_some_and(|l| l.state == WorkspaceState::PrOpen)
        },
    )
    .await;
    assert!(
        runner.state.lock().qa_runs >= 2,
        "on_reject = converge re-verifies"
    );
    t.close().await;
}
