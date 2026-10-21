//! Port of Go `resume_test.go`: crash recovery resumes at the checkpoint
//! (D55) with the killed Coder session, human pauses stay paused, queued
//! loops start after a restart, transient failures are retryable (D56), and
//! the checkpoint write-through. `TestClassifyAgentError` is recur-agent's
//! (`error::classify`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::{AgentEvent, Provider, ProviderError, TurnResult};
use recur_domain::{Agent, AgentRole, AutonomyLevel, Checkpoint, Group, Workspace, WorkspaceState};
use recur_store::Store;

use crate::lifecycle::PHASE_INTERRUPTED;
use crate::loops::lifecycle::resumable;
use crate::loops::resume::resume_point_for;
use crate::loops::test_support::*;
use crate::publish::status_label;
use crate::testing::{TestEngine, add_project, init_repo, wait_for};
use crate::text::rfc3339;

const SPEND_LIMIT_MSG: &str = "You've hit your org's monthly spend limit · run /usage-credits to ask your admin for a higher limit";

/// The record an app crash leaves behind: an in-flight state with a
/// checkpoint and no task (Go `interruptedLoop`).
fn interrupted_loop(store: &Store, proj: &str, cp: WorkspaceState, iter: i64) -> String {
    let id = format!("LOOP-RESUME-{}", cp.as_str());
    store
        .add_workspace(Workspace {
            id: id.clone(),
            project_id: proj.into(),
            group: Group::Active,
            goal: "Add a resumable feature".into(),
            short: "Add a resumable feature".into(),
            status_label: status_label(&cp),
            phase: format!("iteration {iter}"),
            iteration: iter,
            autonomy: AutonomyLevel::Balanced,
            base_branch: "main".into(),
            agent_codes: Some(vec!["MA".into(), "CO".into(), "RV".into()]),
            elapsed_label: "running".into(),
            checkpoint: Some(Checkpoint {
                state: cp.clone(),
                iteration: iter,
                at: rfc3339(chrono::Local::now()),
                ..Checkpoint::default()
            }),
            state: cp,
            ..Workspace::default()
        })
        .unwrap();
    id
}

async fn wait_for_state(store: &Store, id: &str, what: &str, want: &[WorkspaceState]) -> Workspace {
    wait_for(Duration::from_secs(10), what, || {
        store.workspace(id).is_some_and(|l| want.contains(&l.state))
    })
    .await;
    store.workspace(id).unwrap()
}

/// A walk publishes its settled state before it records failure marks and
/// unwinds (Go `waitLoopExited`).
async fn wait_loop_exited(t: &TestEngine, id: &str) {
    wait_for(Duration::from_secs(10), "walk to exit", || {
        t.engine.running_loop_by_id(id).is_none()
    })
    .await;
}

const SETTLED: &[WorkspaceState] = &[
    WorkspaceState::PrOpen,
    WorkspaceState::Failed,
    WorkspaceState::Stopped,
];

fn coder_agent(id: &str, session: &str) -> Agent {
    Agent {
        id: format!("{id}-CO"),
        workspace_id: id.into(),
        code: "CO".into(),
        name: "Coder".into(),
        session_id: session.into(),
        ..Agent::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_continues_at_checkpoint_iteration() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = interrupted_loop(&t.store, &proj, WorkspaceState::Coding, 3);

    t.engine.recover();
    let parked = t.store.workspace(&id).unwrap();
    assert_eq!(parked.state, WorkspaceState::Paused);
    assert_eq!(parked.phase, PHASE_INTERRUPTED);
    assert_eq!(parked.checkpoint.as_ref().unwrap().iteration, 3);

    t.engine.auto_resume().await;
    let fin = wait_for_state(&t.store, &id, "resumed loop to settle", SETTLED).await;
    assert_eq!(fin.state, WorkspaceState::PrOpen, "{}", fin.last_error);
    assert_eq!(
        runner.counts(),
        (1, 1),
        "iterations 1 and 2 were not replayed"
    );
    assert_eq!(fin.iteration, 3);
    let conv = fin.convergence.unwrap_or_default();
    assert_eq!(conv.len(), 1);
    assert_eq!(conv[0].iteration, 3);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_skips_coder_when_interrupted_after_commit() {
    for cp in [WorkspaceState::Reviewing, WorkspaceState::Verifying] {
        let repo = init_repo();
        let runner = ConvergeRunner::new(&[0]);
        let t = TestEngine::new(Some(runner.provider()));
        let proj = add_project(&t.store, &repo.path);
        let id = interrupted_loop(&t.store, &proj, cp.clone(), 2);
        t.engine.recover();
        t.engine.auto_resume().await;
        let fin = wait_for_state(&t.store, &id, "resumed loop to settle", SETTLED).await;
        assert_eq!(
            fin.state,
            WorkspaceState::PrOpen,
            "{cp:?}: {}",
            fin.last_error
        );
        assert_eq!(runner.counts(), (0, 1), "{cp:?} already committed its code");
        assert_eq!(fin.iteration, 2);
        t.close().await;
    }
}

/// Records the session each coder turn resumes (Go `sessionSpyRunner`).
#[derive(Default)]
struct SessionSpy {
    coder_resumes: Mutex<Vec<String>>,
}

impl SessionSpy {
    fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, _cx| {
            let me = Arc::clone(&me);
            async move {
                if req.role == AgentRole::Coder {
                    me.coder_resumes.lock().push(req.resume_session.clone());
                    apply_edits(&req.worktree, &[("work.txt", "edit\n")])
                        .map_err(|e| ProviderError::failed(e.to_string()))?;
                }
                Ok(success("new-session", ""))
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_restores_coder_session() {
    let repo = init_repo();
    let runner = Arc::new(SessionSpy::default());
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = interrupted_loop(&t.store, &proj, WorkspaceState::Coding, 2);
    t.store
        .upsert_agent(&id, coder_agent(&id, "sess-killed-mid-edit"));
    t.engine.recover();
    t.engine.auto_resume().await;
    wait_for_state(&t.store, &id, "resumed loop to settle", SETTLED).await;
    let got = runner.coder_resumes.lock().clone();
    assert!(!got.is_empty(), "the Coder never ran");
    assert_eq!(got[0], "sess-killed-mid-edit");
    for (i, s) in got[1..].iter().enumerate() {
        assert_eq!(s, "", "coder turn {} starts fresh", i + 2);
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_resume_ignores_human_paused_loops() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    t.store
        .add_workspace(Workspace {
            id: "LOOP-HUMAN-PAUSED".into(),
            project_id: proj,
            goal: "Paused by hand".into(),
            state: WorkspaceState::Paused,
            phase: "paused by you".into(),
            iteration: 2,
            autonomy: AutonomyLevel::Balanced,
            base_branch: "main".into(),
            checkpoint: Some(Checkpoint {
                state: WorkspaceState::Coding,
                iteration: 2,
                ..Checkpoint::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.engine.recover();
    t.engine.auto_resume().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        t.store.workspace("LOOP-HUMAN-PAUSED").unwrap().state,
        WorkspaceState::Paused
    );
    assert_eq!(runner.counts().0, 0);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_survives_store_reopen() {
    let repo = init_repo();
    let runner = Arc::new(SessionSpy::default());
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = interrupted_loop(&t.store, &proj, WorkspaceState::Coding, 4);
    t.store
        .upsert_agent(&id, coder_agent(&id, "sess-before-quit"));

    t.store.close().unwrap();
    let st2 = Arc::new(Store::open(&t.data_dir.join("recur.db")).unwrap());
    let reloaded = st2.workspace(&id).expect("loop after reopen");
    assert_eq!(reloaded.checkpoint.as_ref().unwrap().iteration, 4);

    let eng2 = t.reopen_on(Arc::clone(&st2), Some(runner.provider()));
    eng2.recover();
    eng2.auto_resume().await;
    let fin = wait_for_state(&st2, &id, "reopened loop to settle", SETTLED).await;
    assert_eq!(fin.state, WorkspaceState::PrOpen, "{}", fin.last_error);
    assert_eq!(fin.iteration, 4);
    let got = runner.coder_resumes.lock().clone();
    assert_eq!(got.first().map(String::as_str), Some("sess-before-quit"));
    eng2.shutdown().await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_resume_starts_queued_loops() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    t.store
        .add_workspace(Workspace {
            id: "LOOP-QUEUED".into(),
            project_id: proj,
            goal: "Queued when the app closed".into(),
            state: WorkspaceState::Queued,
            phase: "waiting for a slot".into(),
            autonomy: AutonomyLevel::Balanced,
            base_branch: "main".into(),
            ..Workspace::default()
        })
        .unwrap();
    t.engine.recover();
    t.engine.auto_resume().await;
    let fin = wait_for_state(&t.store, "LOOP-QUEUED", "queued loop to run", SETTLED).await;
    assert_eq!(fin.state, WorkspaceState::PrOpen, "{}", fin.last_error);
    assert_eq!(fin.iteration, 1, "a never-started loop begins at 1");
    t.close().await;
}

/// Fails the first `fail_until` coder turns the way a spend-limit refusal
/// arrives: stderr carries the provider's message (Go `limitRunner`).
fn limit_runner(msg: &'static str, fail_until: usize) -> Arc<dyn Provider> {
    let turns = Arc::new(Mutex::new(0usize));
    AsyncProvider::wrap(move |req, _cx| {
        let turns = Arc::clone(&turns);
        async move {
            if req.role != AgentRole::Coder {
                return Ok(success("", ""));
            }
            let n = {
                let mut t = turns.lock();
                *t += 1;
                *t
            };
            if n <= fail_until {
                return Err(ProviderError::failed(format!(
                    "claude exited without a result: exit status 1: {msg}"
                ))
                .with_partial(TurnResult {
                    subtype: "error_during_execution".into(),
                    is_error: true,
                    stderr: msg.into(),
                    ..TurnResult::default()
                }));
            }
            apply_edits(&req.worktree, &[("work.txt", "edit\n")])
                .map_err(|e| ProviderError::failed(e.to_string()))?;
            Ok(success("s", ""))
        }
    })
}

fn genuine_failure() -> ProviderError {
    ProviderError::failed("the agent could not parse the file").with_partial(TurnResult {
        subtype: "error_during_execution".into(),
        is_error: true,
        ..TurnResult::default()
    })
}

async fn start(t: &TestEngine, proj: &str, task: &str) -> String {
    t.engine
        .start_workspace(proj, task, "", AutonomyLevel::default())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transient_llm_failure_is_retryable() {
    let repo = init_repo();
    let t = TestEngine::new(Some(limit_runner(SPEND_LIMIT_MSG, 1)));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "Task that hits the spend limit").await;
    wait_for_state(
        &t.store,
        &id,
        "fail on the spend limit",
        &[WorkspaceState::Failed],
    )
    .await;
    wait_loop_exited(&t, &id).await;
    let failed = t.store.workspace(&id).unwrap();
    assert!(failed.retryable);
    assert!(
        failed.last_error.contains("monthly spend limit"),
        "{}",
        failed.last_error
    );
    assert!(
        failed.checkpoint.is_some(),
        "Retry re-enters the checkpoint"
    );
    assert_eq!(failed.group, Group::Needs);
    assert!(resumable(&failed));

    t.engine.retry(&id).await;
    let fin = wait_for_state(
        &t.store,
        &id,
        "retried loop to settle",
        &[WorkspaceState::PrOpen, WorkspaceState::Stopped],
    )
    .await;
    assert_eq!(fin.state, WorkspaceState::PrOpen, "{}", fin.last_error);
    assert!(!fin.retryable && fin.last_error.is_empty());
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn genuine_failure_is_not_retryable() {
    let repo = init_repo();
    let t = TestEngine::new(Some(AsyncProvider::wrap(|req, _cx| async move {
        if req.role != AgentRole::Coder {
            return Ok(success("", ""));
        }
        Err(genuine_failure())
    })));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "Task that genuinely fails").await;
    wait_for_state(&t.store, &id, "loop to fail", &[WorkspaceState::Failed]).await;
    wait_loop_exited(&t, &id).await;
    let failed = t.store.workspace(&id).unwrap();
    assert!(!failed.retryable);
    assert!(!resumable(&failed));
    t.engine.retry(&id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::Failed
    );
    t.close().await;
}

/// Fails the planning turn `fail_until` times, then plans (Go
/// `plannerFailRunner`).
#[derive(Default)]
struct PlannerFail {
    planner_turns: Mutex<usize>,
    coder_turns: Mutex<usize>,
    fail_until: usize,
    msg: &'static str,
    genuine: bool,
}

impl PlannerFail {
    fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, cx| {
            let me = Arc::clone(&me);
            async move {
                match req.role {
                    AgentRole::Maestro => {
                        let n = {
                            let mut p = me.planner_turns.lock();
                            *p += 1;
                            *p
                        };
                        if n <= me.fail_until {
                            if me.genuine {
                                return Err(genuine_failure());
                            }
                            return Err(ProviderError::failed(format!(
                                "claude exited without a result: {}",
                                me.msg
                            ))
                            .with_partial(TurnResult {
                                subtype: "error_during_execution".into(),
                                is_error: true,
                                text: me.msg.into(),
                                ..TurnResult::default()
                            }));
                        }
                        cx.sink.emit(AgentEvent::Text {
                            text: "1. Do the thing".into(),
                            uuid: String::new(),
                            parent: String::new(),
                        });
                        Ok(success(&req.agent_id, "1. Do the thing"))
                    }
                    AgentRole::Coder => {
                        *me.coder_turns.lock() += 1;
                        apply_edits(&req.worktree, &[("work.txt", "edit\n")])
                            .map_err(|e| ProviderError::failed(e.to_string()))?;
                        Ok(success("s", ""))
                    }
                    _ => Ok(success("", "")),
                }
            }
        })
    }

    fn counts(&self) -> (usize, usize) {
        (*self.planner_turns.lock(), *self.coder_turns.lock())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planner_transient_failure_is_retryable() {
    let repo = init_repo();
    let runner = Arc::new(PlannerFail {
        msg: "overloaded_error: the model is overloaded",
        fail_until: 1,
        ..PlannerFail::default()
    });
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "Task whose planner is overloaded").await;
    wait_for_state(
        &t.store,
        &id,
        "fail on the overloaded planner",
        &[WorkspaceState::Failed],
    )
    .await;
    wait_loop_exited(&t, &id).await;
    let failed = t.store.workspace(&id).unwrap();
    assert!(failed.retryable);
    assert!(
        failed.last_error.contains("overloaded"),
        "{}",
        failed.last_error
    );
    assert_eq!(
        failed.checkpoint.as_ref().map(|c| c.phase_id.as_str()),
        Some("planning"),
        "Retry re-enters planning"
    );
    let (planner, coder) = runner.counts();
    assert!(planner >= 1);
    assert_eq!(coder, 0, "must not code without a plan");

    t.engine.retry(&id).await;
    let fin = wait_for_state(
        &t.store,
        &id,
        "retried loop to settle",
        &[WorkspaceState::PrOpen, WorkspaceState::Stopped],
    )
    .await;
    assert_eq!(fin.state, WorkspaceState::PrOpen, "{}", fin.last_error);
    assert!(runner.counts().1 > 0);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planner_genuine_failure_still_degrades() {
    let repo = init_repo();
    let runner = Arc::new(PlannerFail {
        genuine: true,
        fail_until: 1,
        ..PlannerFail::default()
    });
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = start(&t, &proj, "Task whose planner genuinely fails").await;
    let fin = wait_for_state(
        &t.store,
        &id,
        "settle after genuine planner failure",
        SETTLED,
    )
    .await;
    assert_eq!(fin.state, WorkspaceState::PrOpen, "{}", fin.last_error);
    assert!(!fin.retryable);
    assert!(
        runner.counts().1 > 0,
        "proceeds to coding with the bare task"
    );
    t.close().await;
}

#[test]
fn resume_point_for_maps_checkpoints() {
    let cp = |state, iteration| Workspace {
        checkpoint: Some(Checkpoint {
            state,
            iteration,
            ..Checkpoint::default()
        }),
        ..Workspace::default()
    };
    assert!(resume_point_for(&Workspace::default(), HashMap::new()).is_none());
    assert!(
        resume_point_for(&cp(WorkspaceState::PrOpen, 1), HashMap::new()).is_none(),
        "terminal checkpoint is not resumable"
    );
    for (rec, state, iter, skip) in [
        (
            cp(WorkspaceState::Planning, 0),
            WorkspaceState::Planning,
            1,
            false,
        ),
        (
            cp(WorkspaceState::Coding, 4),
            WorkspaceState::Coding,
            4,
            false,
        ),
        (
            cp(WorkspaceState::Reviewing, 2),
            WorkspaceState::Coding,
            2,
            true,
        ),
        (
            cp(WorkspaceState::Verifying, 3),
            WorkspaceState::Coding,
            3,
            true,
        ),
    ] {
        let got = resume_point_for(&rec, HashMap::new()).expect("a resume point");
        assert_eq!(
            (got.state, got.iteration, got.skip_coder),
            (state, iter, skip)
        );
    }
}

#[tokio::test]
async fn set_workspace_state_writes_checkpoint() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "LOOP-CP".into(),
            project_id: "p".into(),
            goal: "g".into(),
            state: WorkspaceState::Queued,
            ..Workspace::default()
        })
        .unwrap();
    t.engine.set_workspace_state(
        "LOOP-CP",
        WorkspaceState::Reviewing,
        "reviewing iteration 2",
    );
    let got = t.store.workspace("LOOP-CP").unwrap();
    assert_eq!(got.checkpoint.unwrap().state, WorkspaceState::Reviewing);
    t.engine
        .set_workspace_state("LOOP-CP", WorkspaceState::PrOpen, "done");
    assert!(t.store.workspace("LOOP-CP").unwrap().checkpoint.is_none());
    t.close().await;
}

#[tokio::test]
async fn set_workspace_state_preserves_phase_id() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "LOOP-PHASE".into(),
            project_id: "p".into(),
            goal: "g".into(),
            state: WorkspaceState::Queued,
            checkpoint: Some(Checkpoint {
                phase_id: "converge".into(),
                resume_phase_id: "pr".into(),
                ..Checkpoint::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.engine
        .set_workspace_state("LOOP-PHASE", WorkspaceState::Coding, "iteration 1");
    let cp = t.store.workspace("LOOP-PHASE").unwrap().checkpoint.unwrap();
    assert_eq!(
        (cp.phase_id.as_str(), cp.resume_phase_id.as_str(), cp.state),
        ("converge", "pr", WorkspaceState::Coding)
    );
    t.close().await;
}
