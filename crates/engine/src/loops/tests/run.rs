//! The `run` verb: exit 0 advances, a failure with `on_reject` sends the work
//! back to the implementer with the output tail, and retries rerun the command.

use std::path::Path;
use std::time::Duration;

use recur_domain::{AutonomyLevel, WorkspaceState};

use crate::loops::run::output_tail;
use crate::loops::test_support::*;
use crate::testing::{TempRepo, TestEngine, TestOptions, add_project, init_repo, wait_for};

async fn walk(
    runner: &std::sync::Arc<ConvergeRunner>,
    loop_id: &str,
) -> (TempRepo, TestEngine, String) {
    let repo = init_repo();
    let t = TestEngine::with(TestOptions {
        provider: Some(runner.provider()),
        ..TestOptions::default()
    });
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "run a check",
            "",
            AutonomyLevel::default(),
            loop_id,
            "",
        )
        .await
        .unwrap();
    (repo, t, id)
}

async fn settled(t: &TestEngine, id: &str, want: WorkspaceState) {
    wait_for(Duration::from_secs(30), "the walk to settle", || {
        t.store.workspace(id).is_some_and(|l| l.state == want)
    })
    .await;
}

fn runs(t: &TestEngine, id: &str) -> usize {
    let wt = t.store.workspace(id).unwrap().worktree_path;
    std::fs::read_to_string(Path::new(&wt).join(".runs"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_reject_sends_tail_to_the_implementer() {
    let runner = ConvergeRunner::new(&[0, 0]);
    let (_repo, t, id) = walk(&runner, "test.loops.run-reject").await;
    settled(&t, &id, WorkspaceState::PrOpen).await;
    {
        let s = runner.state.lock();
        assert_eq!(s.coder_runs, 2, "the failed check re-ran the build");
        let coder: Vec<_> = s
            .specs
            .iter()
            .filter(|r| r.role == recur_domain::AgentRole::Coder)
            .collect();
        assert!(!coder[0].prompt.contains("Output tail"));
        assert!(
            coder[1].prompt.contains("The check check"),
            "{}",
            coder[1].prompt
        );
        assert!(coder[1].prompt.contains("exited 3"));
        assert!(coder[1].prompt.contains("boom"));
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_reject_into_review_first_converge_runs_the_coder() {
    let runner = ConvergeRunner::new(&[0, 0]);
    let (_repo, t, id) = walk(&runner, "test.loops.run-reject-review").await;
    settled(&t, &id, WorkspaceState::PrOpen).await;
    {
        let s = runner.state.lock();
        assert_eq!(s.coder_runs, 1, "only the rejection ran the coder");
        let coder = s
            .specs
            .iter()
            .find(|r| r.role == recur_domain::AgentRole::Coder)
            .unwrap();
        assert!(coder.prompt.contains("boom"), "{}", coder.prompt);
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_retries_rerun_the_command() {
    let runner = ConvergeRunner::new(&[]);
    let (_repo, t, id) = walk(&runner, "test.loops.run-retry").await;
    settled(&t, &id, WorkspaceState::PrOpen).await;
    assert_eq!(runs(&t, &id), 2, "the retry passed");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_failure_without_on_reject_takes_on_fail() {
    let runner = ConvergeRunner::new(&[]);
    let (_repo, t, id) = walk(&runner, "test.loops.run-fail").await;
    settled(&t, &id, WorkspaceState::Failed).await;
    assert_eq!(runs(&t, &id), 2, "one try plus one retry");
    assert_eq!(runner.counts(), (0, 0));
    t.close().await;
}

#[test]
fn output_tail_keeps_the_last_lines() {
    let out: String = (1..=50).map(|i| format!("{i}\n")).collect();
    let tail = output_tail(&out, "err\n");
    assert!(tail.starts_with("12\n"), "{tail}");
    assert!(tail.ends_with("50\nerr"));
    assert_eq!(output_tail("", ""), "");

    let noise: String = (1..=60).map(|i| format!("Compiling {i}\n")).collect();
    let tail = output_tail("test a ... FAILED\npanicked at x", &noise);
    assert!(
        tail.starts_with("test a ... FAILED\npanicked at x\n"),
        "{tail}"
    );
    assert!(tail.ends_with("Compiling 60"));
    assert_eq!(tail.lines().count(), 40);

    let tail = output_tail(&out, &noise);
    assert!(
        tail.starts_with("21\n") && tail.contains("50\nCompiling 51"),
        "{tail}"
    );
}
