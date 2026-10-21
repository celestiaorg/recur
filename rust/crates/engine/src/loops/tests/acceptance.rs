//! Port of Go `acceptance_test.go` (`TestHeadlineE2E`), the loop half: a
//! typed task plans, converges code↔review [2,0], verifies in a browser with
//! evidence, opens a Local PR, and the record survives a store reopen. The
//! inline comment, send-back and merge steps go through E5's PR API; the
//! feed reconstruction check is E4's (`deferred::reconstruct_feed`). Also
//! the env-gated real-agent run (Go `real_e2e_test.go`), same split.

use std::sync::Arc;
use std::time::Duration;

use recur_domain::{AgentRole, AutonomyLevel, Group, PrSource, WorkspaceState};
use recur_forge::model::single_line;
use recur_store::Store;

use crate::bus::EngineEvent;
use crate::loops::test_support::*;
use crate::roles::role_spec;
use crate::testing::{TestEngine, git, git_out, init_repo, wait_for};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn headline_e2e() {
    let repo = init_repo();
    let runner = Arc::new(ConvergeRunner {
        findings_per_iter: vec![2, 0],
        evidence_always: vec![
            StepEvidence {
                step: "Booted the app and opened /",
                shot: "/shots/01.png",
                passed: true,
            },
            StepEvidence {
                step: "Exercised the new feature end to end",
                shot: "/shots/02.png",
                passed: true,
            },
            StepEvidence {
                step: "Reloaded — state persisted",
                shot: "/shots/03.png",
                passed: true,
            },
        ],
        file: "feature.txt",
        file_body: "the implemented feature\n",
        message: "tighten this",
        ..ConvergeRunner::default()
    });
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let Some(proj) = verify_dev_project(&t.store, &repo.path) else {
        eprintln!("python3 not available; verify needs a bootable dev server");
        return;
    };

    // 1. Fresh project, typed task, the verify graph.
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "Add a verifiable, reviewable feature",
            "",
            AutonomyLevel::default(),
            "test.loops.verify",
            "",
        )
        .await
        .unwrap();
    assert!(!id.is_empty());

    // 2-4. Converge to 0 findings, verify, open a PR.
    wait_for(
        Duration::from_secs(30),
        "converge, verify, and open a PR",
        || {
            t.store
                .workspace(&id)
                .is_some_and(|l| l.state == WorkspaceState::PrOpen)
        },
    )
    .await;
    wait_for(Duration::from_secs(10), "walk to exit", || {
        t.engine.running_loop_by_id(&id).is_none()
    })
    .await;
    let l = t.store.workspace(&id).unwrap();
    assert_eq!(l.group, Group::Ready);
    let series: Vec<i64> = l
        .convergence
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|c| c.findings)
        .collect();
    assert_eq!(series, [2, 0]);

    let s = runner.state.lock();
    assert!(
        s.coder_runs >= 2 && s.review_runs >= 2,
        "{} {}",
        s.coder_runs,
        s.review_runs
    );
    for c in &s.saw_coder {
        assert!(!s.saw_reviewer.contains(c), "rail #2: shared agent id {c}");
    }
    assert!(s.qa_runs >= 1);
    assert!(s.qa_playwright);
    for banned in ["Edit", "Write", "NotebookEdit"] {
        assert!(!s.qa_tools.iter().any(|x| x == banned), "QA has {banned}");
    }
    drop(s);
    assert_ne!(
        role_spec(&AgentRole::Reviewer).code,
        role_spec(&AgentRole::Coder).code
    );
    let saw_converged = t.events.all().into_iter().any(|e| {
        matches!(e, EngineEvent::WorkspaceEvent(w) if w.event.name == "Maestro" && w.event.text.contains("Converged"))
    });
    assert!(saw_converged, "the engine declares convergence");
    assert_eq!(t.store.open_findings_count(&id), 0);
    assert_eq!(t.store.findings(&id).len(), 2);

    assert_eq!(l.verify_state, "passed");
    assert_eq!(l.verify.len(), 3);
    for v in &l.verify {
        assert!(v.passed && !v.screenshot.is_empty());
    }
    let saw_verifying = t.events.by_name("workspace.updated").iter().any(
        |e| matches!(e, EngineEvent::WorkspaceUpdated(u) if u.state == WorkspaceState::Verifying),
    );
    assert!(saw_verifying);

    let pr = t.store.pr_by_workspace(&id).expect("a PR");
    assert_eq!(pr.source, PrSource::Local);
    assert_eq!((pr.state.as_str(), pr.group.as_str()), ("In review", "you"));
    assert!(!pr.branch.is_empty());
    assert_eq!(pr.base, "main");
    let files = pr.files.clone().unwrap_or_default();
    assert!(pr.file_count > 0 && !files.is_empty() && pr.diff.add > 0);
    let commits = pr.commits.clone().unwrap_or_default();
    assert!(!commits.is_empty());
    assert!(!single_line(&commits[0].message).is_empty());
    let checks = pr.checks.clone().unwrap_or_default();
    assert!(
        checks
            .iter()
            .any(|c| c.name == "Independent review" && c.passed),
        "{checks:?}"
    );
    assert!(
        checks
            .iter()
            .any(|c| c.name.to_lowercase().contains("verif") && c.passed),
        "{checks:?}"
    );

    // 7. Survives an app reload.
    t.store.close().unwrap();
    let st2 = Store::open(&t.data_dir.join("recur.db")).unwrap();
    let l2 = st2.workspace(&id).expect("loop after reload");
    assert_eq!(l2.state, WorkspaceState::PrOpen);
    assert_eq!(l2.convergence.unwrap_or_default().len(), 2);
    assert_eq!(l2.verify_state, "passed");
    assert_eq!(l2.verify.len(), 3);
    let pr2 = st2.pr_by_workspace(&id).expect("PR after reload");
    assert_eq!(pr2.state, "In review");
    assert_eq!(pr2.file_count, pr.file_count);
    assert!(!pr2.files.unwrap_or_default().is_empty());
    assert_eq!(st2.findings(&id).len(), 2);
    st2.close().unwrap();
    t.close().await;
}

/// Go `TestRealHeadlineE2E`, the loop half: real `claude -p` agents, the
/// real MCP and Playwright driving a real dev server, real git. Gated on
/// `RECUR_E2E=1` because it spends credit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real agents: RECUR_E2E=1 cargo test -- --ignored real_headline_e2e"]
async fn real_headline_e2e() {
    if std::env::var("RECUR_E2E").as_deref() != Ok("1") {
        eprintln!("set RECUR_E2E=1 to run the real-agent headline E2E (spends credit)");
        return;
    }
    for bin in ["claude", "python3", "npx", "git"] {
        let found = std::process::Command::new("which")
            .arg(bin)
            .output()
            .is_ok_and(|o| o.status.success());
        if !found {
            eprintln!("{bin} not on PATH");
            return;
        }
    }
    let repo = init_repo();
    std::fs::write(
        std::path::Path::new(&repo.path).join("index.html"),
        "<!doctype html>\n<html>\n<head><title>demo</title></head>\n<body>\n</body>\n</html>\n",
    )
    .unwrap();
    git(&repo.path, &["add", "."]);
    git(&repo.path, &["commit", "-m", "add index.html"]);

    let t = TestEngine::new(Some(Arc::new(recur_provider_claude::ClaudeProvider::new(
        recur_provider_claude::ClaudeConfig::default(),
    ))));
    let proj = verify_dev_project(&t.store, &repo.path).expect("python3");
    t.install_fixture_pack();
    let mut p = t.store.policy();
    p.autonomy = AutonomyLevel::Cautious;
    t.store.set_policy(p);

    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "In index.html, add an <h1> element with the exact text 'Hello Recur' inside <body>. Change nothing else.",
            "",
            AutonomyLevel::default(),
            "test.loops.verify",
            "recur.policies.cautious",
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(12 * 60), "real loop to settle", || {
        t.store.workspace(&id).is_some_and(|l| {
            matches!(
                l.state,
                WorkspaceState::PrOpen
                    | WorkspaceState::Failed
                    | WorkspaceState::Stopped
                    | WorkspaceState::Escalated
            )
        })
    })
    .await;
    let l = t.store.workspace(&id).unwrap();
    assert_eq!(
        l.state,
        WorkspaceState::PrOpen,
        "phase={} last={}",
        l.phase,
        l.last
    );
    assert_eq!(l.verify_state, "passed", "{} steps", l.verify.len());
    assert!(!l.verify.is_empty());
    let pr = t.store.pr_by_workspace(&id).expect("a PR");
    assert!(pr.diff.add > 0);
    assert_eq!(t.store.open_findings_count(&id), 0);
    let out = git_out(
        &l.worktree_path,
        &["grep", "Hello Recur", "--", "index.html"],
    );
    assert!(
        out.as_deref().is_ok_and(|o| o.contains("Hello Recur")),
        "{out:?}"
    );
    t.close().await;
}
