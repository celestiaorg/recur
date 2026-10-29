//! Walker, converge and rails tests (the loop halves; merge and send-back
//! are in `prs::tests`) and the loop-driven fixture cases.

use std::time::Duration;

use pretty_assertions::assert_eq;
use recur_bundles::{FindingsThreshold, LoopDef, LoopPhase, Policy, VERB_CONVERGE, VERB_ENSURE_PR};
use recur_domain::{
    AutonomyLevel, Finding, Plan, PlanStep, STEP_PENDING, Severity, Workspace, WorkspaceState,
};
use tokio_util::sync::CancellationToken;

use crate::bus::EngineEvent;
use crate::guard::ResolvedGuard;
use crate::loops::Walk;
use crate::loops::converge::{converge_round_window, seed_fix_findings};
use crate::loops::failure::LoopError;
use crate::loops::test_support::*;
use crate::roles::{qa_runtime_extra, role_spec};
use crate::testing::{TestEngine, add_project, init_repo, wait_for};

fn guard_with(phases: Vec<LoopPhase>) -> ResolvedGuard {
    ResolvedGuard {
        loop_def: LoopDef {
            phases,
            ..LoopDef::default()
        },
        ..ResolvedGuard::default()
    }
}

fn phase(id: &str, uses: &str, next: &str) -> LoopPhase {
    LoopPhase {
        id: id.into(),
        uses: uses.into(),
        next: next.into(),
        ..LoopPhase::default()
    }
}

fn add_ws(t: &TestEngine, id: &str, state: WorkspaceState, phase: &str) {
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            goal: "x".into(),
            state,
            phase: phase.into(),
            ..Workspace::default()
        })
        .expect("add workspace");
}

// ---- walker

#[tokio::test]
async fn inbound_graph_phase() {
    let t = TestEngine::new(None);
    let mut l = Walk::bare(&t.engine, "x");
    let mut ondemand = phase("security", "", "");
    ondemand.dispatch = "on_demand".into();
    l.set_guard(guard_with(vec![
        phase("planning", "", "converge"),
        phase("converge", "", "pr"),
        phase("pr", VERB_ENSURE_PR, ""),
        ondemand,
    ]));
    assert_eq!(l.inbound_graph_phase("pr"), "converge");
    assert_eq!(l.inbound_graph_phase("converge"), "planning");
    assert_eq!(l.inbound_graph_phase("missing"), "");
    assert_eq!(l.ensure_pr_phase_id(), "pr");
}

#[test]
fn seed_fix_findings_cases() {
    let open = vec![Finding {
        id: "f1".into(),
        status: "open".into(),
        ..Finding::default()
    }];
    assert_eq!(seed_fix_findings(5, false, open.clone()).len(), 1);
    assert!(seed_fix_findings(5, true, open.clone()).is_empty());
    assert!(seed_fix_findings(1, false, open).is_empty());
    assert!(seed_fix_findings(5, false, Vec::new()).is_empty());
}

#[test]
fn converge_round_window_cases() {
    for (start, max, want) in [
        (1, 4, 4),
        (3, 4, 6),
        (7, 6, 12),
        (5, 4, 8),
        (0, 4, 4),
        (1, 0, 1),
    ] {
        assert_eq!(converge_round_window(start, max), want, "({start}, {max})");
    }
}

#[tokio::test]
async fn phase_retries() {
    let t = TestEngine::new(None);
    let mut l = Walk::bare(&t.engine, "x");
    l.set_guard(ResolvedGuard {
        policy: Policy {
            retries: 3,
            ..Policy::default()
        },
        ..ResolvedGuard::default()
    });
    let ph = LoopPhase {
        retries: Some(2),
        ..LoopPhase::default()
    };
    assert_eq!(l.phase_retries(&ph), 2);
    assert_eq!(l.phase_retries(&LoopPhase::default()), 3);
}

#[tokio::test]
async fn run_loop_phase_cancelled_writes_stopped() {
    let t = TestEngine::new(None);
    add_ws(&t, "WS-walk-stop", WorkspaceState::Coding, "implementing");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut l = Walk::bare(&t.engine, "WS-walk-stop");
    let (_, done) = l.run_loop_phase(&cancel, &phase("x", "unknown", "")).await;
    assert!(done);
    let ws = t.store.workspace("WS-walk-stop").unwrap();
    assert_eq!(ws.state, WorkspaceState::Stopped);
}

#[tokio::test]
async fn run_loop_phase_cancelled_idles_when_requested() {
    let t = TestEngine::new(None);
    add_ws(&t, "WS-walk-idle", WorkspaceState::Coding, "implementing");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let mut l = Walk::bare(&t.engine, "WS-walk-idle");
    l.h.request_idle();
    let (_, done) = l.run_loop_phase(&cancel, &phase("x", "unknown", "")).await;
    assert!(done);
    let ws = t.store.workspace("WS-walk-idle").unwrap();
    assert_eq!(
        (ws.state, ws.phase.as_str()),
        (WorkspaceState::Paused, "idle")
    );
}

#[tokio::test]
async fn take_on_fail_preserves_turn_error() {
    let t = TestEngine::new(None);
    add_ws(&t, "WS-onfail", WorkspaceState::Coding, "implementing");
    let mut l = Walk::bare(&t.engine, "WS-onfail");
    l.set_guard(ResolvedGuard {
        policy: Policy {
            on_fail: "fail".into(),
            ..Policy::default()
        },
        ..ResolvedGuard::default()
    });
    let err = recur_agent::ProviderError::failed("provider overloaded");
    let te = LoopError::turn(
        "Coder",
        Some(&err),
        &recur_agent::TurnResult {
            is_error: true,
            ..Default::default()
        },
    );
    let cancel = CancellationToken::new();
    // A phase whose only attempt failed lands in take_on_fail.
    let (_, done) = l
        .take_on_fail(&cancel, &phase("converge", "", ""), Some(te))
        .await;
    assert!(done);
    let ws = t.store.workspace("WS-onfail").unwrap();
    assert!(ws.retryable, "take_on_fail(turn error) must be retryable");
    assert!(ws.last_error.contains("overloaded"), "{}", ws.last_error);
    assert_ne!(ws.last_error, crate::loops::walker::ERR_PHASE_FAILED);
}

#[tokio::test]
async fn turn_context_uses_phase_timeout() {
    let t = TestEngine::new(None);
    let mut l = Walk::bare(&t.engine, "x");
    l.set_guard(ResolvedGuard {
        policy: Policy {
            timeout: "4h".into(),
            ..Policy::default()
        },
        ..ResolvedGuard::default()
    });
    l.set_current_phase(&LoopPhase {
        timeout: "50ms".into(),
        ..LoopPhase::default()
    });
    let scope = l.turn_scope(&CancellationToken::new());
    let dl = scope.deadline().expect("phase timeout deadline");
    let remain = dl.saturating_duration_since(tokio::time::Instant::now());
    assert!(remain <= Duration::from_millis(200), "remain {remain:?}");
}

#[tokio::test]
async fn finish_stopped_if_in_flight_writes_stopped() {
    let t = TestEngine::new(None);
    add_ws(&t, "WS-inflight", WorkspaceState::Coding, "iteration 1");
    Walk::bare(&t.engine, "WS-inflight").finish_stopped_if_in_flight();
    assert_eq!(
        t.store.workspace("WS-inflight").unwrap().state,
        WorkspaceState::Stopped
    );
}

#[tokio::test]
async fn finish_stopped_if_in_flight_leaves_pr_open() {
    let t = TestEngine::new(None);
    add_ws(
        &t,
        "WS-propen",
        WorkspaceState::PrOpen,
        "PR open — ready for your review",
    );
    Walk::bare(&t.engine, "WS-propen").finish_stopped_if_in_flight();
    assert_eq!(
        t.store.workspace("WS-propen").unwrap().state,
        WorkspaceState::PrOpen
    );
}

#[tokio::test]
async fn finish_stopped_if_in_flight_uses_walk_exit() {
    let t = TestEngine::new(None);
    add_ws(&t, "WS-walk-exit", WorkspaceState::Coding, "implementing");
    let l = Walk::bare(&t.engine, "WS-walk-exit");
    {
        let mut s = l.h.lock();
        s.exit_state = WorkspaceState::PrOpen;
        s.exit_phase = "PR open — ready for your review".into();
    }
    l.finish_stopped_if_in_flight();
    assert_eq!(l.h.exit().0, WorkspaceState::PrOpen);
    assert_eq!(
        t.store.workspace("WS-walk-exit").unwrap().state,
        WorkspaceState::Coding,
        "a sibling's in-flight row is left alone"
    );
}

#[test]
fn phase_threshold_rejects_looser_override() {
    let lv = |pairs: &[(&str, i64)]| {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), *v))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let g = ResolvedGuard {
        policy: Policy {
            id: "recur.policies.balanced".into(),
            findings_threshold: FindingsThreshold {
                all_zero: false,
                levels: lv(&[("medium", 0), ("high", 0), ("critical", 0)]),
            },
            ..Policy::default()
        },
        ..ResolvedGuard::default()
    };
    let looser = FindingsThreshold {
        all_zero: false,
        levels: lv(&[("high", 0), ("critical", 0)]),
    };
    let th = g.phase_threshold(&LoopPhase {
        id: "pr".into(),
        findings: Some(looser),
        ..LoopPhase::default()
    });
    assert_eq!(th.levels.get("medium"), Some(&0));
}

#[test]
fn qa_runtime_extra_keeps_url() {
    let extra = qa_runtime_extra("http://127.0.0.1:4173", "/tmp/shots");
    assert!(extra.contains("http://127.0.0.1:4173"));
    assert!(extra.contains("/tmp/shots"));
    assert!(extra.contains("attach_evidence"));
}

#[tokio::test]
async fn run_phase_attempt_board_cap_is_terminal() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "WS-board-cap-walk".into(),
            goal: "x".into(),
            state: WorkspaceState::Coding,
            phase: "implementing".into(),
            plan: Some(Plan {
                steps: vec![PlanStep {
                    id: "S1".into(),
                    title: "do".into(),
                    status: STEP_PENDING.into(),
                    ..PlanStep::default()
                }],
                ..Plan::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    let mut l = Walk::bare(&t.engine, "WS-board-cap-walk");
    l.plan_structured = true;
    l.board_max_rounds = 0;
    l.set_guard(ResolvedGuard {
        policy: Policy {
            max_rounds: 4,
            ..Policy::default()
        },
        ..ResolvedGuard::default()
    });
    let (next, done, err) = l
        .run_phase_attempt(
            &CancellationToken::new(),
            &phase("converge", VERB_CONVERGE, "pr"),
        )
        .await;
    assert!(err.is_none(), "{err:?}");
    assert!(
        done && next == WorkspaceState::Paused.as_str(),
        "next={next} done={done}"
    );
    let ws = t.store.workspace("WS-board-cap-walk").unwrap();
    assert_eq!(
        (ws.state.clone(), ws.phase.as_str()),
        (WorkspaceState::Paused, crate::loops::board::BOARD_CAP_PHASE)
    );
    l.finish_stopped_if_in_flight();
    let ws = t.store.workspace("WS-board-cap-walk").unwrap();
    assert_eq!(
        (ws.state, ws.phase.as_str()),
        (WorkspaceState::Paused, crate::loops::board::BOARD_CAP_PHASE)
    );
}

// ---- rails

#[tokio::test]
async fn can_open_pr_policy_bar() {
    let t = TestEngine::new(None);
    let ws = "L-rail3";
    let (ok, reason) = t.engine.can_open_pr(ws);
    assert!(ok, "0 findings: {reason}");
    t.store.add_finding(Finding {
        id: "f1".into(),
        workspace_id: ws.into(),
        iteration: 1,
        file: "a.go".into(),
        severity: Severity::High,
        message: "bug".into(),
        status: "open".into(),
        ..Finding::default()
    });
    let (ok, reason) = t.engine.can_open_pr(ws);
    assert!(!ok);
    assert!(!reason.is_empty());
    t.store.resolve_findings(ws);
    assert!(t.engine.can_open_pr(ws).0);
    t.store.add_finding(Finding {
        id: "f2".into(),
        workspace_id: ws.into(),
        iteration: 2,
        file: "b.go".into(),
        severity: Severity::Low,
        message: "nit".into(),
        status: "open".into(),
        ..Finding::default()
    });
    let (ok, reason) = t.engine.can_open_pr(ws);
    assert!(ok, "leftover low under Balanced: {reason}");
}

// ---- converge

fn workspace_events(t: &TestEngine) -> Vec<recur_domain::FeedItem> {
    t.events
        .all()
        .into_iter()
        .filter_map(|e| match e {
            EngineEvent::WorkspaceEvent(w) => Some(w.event),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn convergence_loop_three_one_zero() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[3, 1, 0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(
            &proj,
            "Build the converging feature",
            "",
            AutonomyLevel::default(),
        )
        .await
        .expect("start");
    wait_for(
        Duration::from_secs(10),
        "loop to converge to pr_open",
        || {
            t.store
                .workspace(&id)
                .is_some_and(|l| l.state == WorkspaceState::PrOpen)
        },
    )
    .await;
    let (coder, review) = runner.counts();
    assert_eq!((coder, review), (3, 3));
    let l = t.store.workspace(&id).unwrap();
    let conv = l.convergence.clone().unwrap_or_default();
    let series: Vec<i64> = conv.iter().map(|c| c.findings).collect();
    assert_eq!(series, vec![3, 1, 0]);
    for (i, c) in conv.iter().enumerate() {
        assert_eq!(c.iteration, i as i64 + 1);
    }
    {
        let s = runner.state.lock();
        for c in &s.saw_coder {
            assert!(!s.saw_reviewer.contains(c), "rail #2: {c}");
        }
    }
    assert_ne!(
        role_spec(&recur_domain::AgentRole::Reviewer).code,
        role_spec(&recur_domain::AgentRole::Coder).code
    );
    assert!(
        workspace_events(&t)
            .iter()
            .any(|it| it.name == "Maestro" && it.text.contains("Converged")),
        "the engine declares convergence"
    );
    assert!(
        t.events.all().iter().any(|e| matches!(e, EngineEvent::WorkspaceUpdated(u) if u.convergence.as_ref().is_some_and(|c| !c.is_empty()))),
        "workspace.updated carries the convergence series"
    );
    assert_eq!(t.store.findings(&id).len(), 4);
    assert_eq!(t.store.open_findings_count(&id), 0);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn convergence_cap_stops_no_infinite_loop() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[2, 2, 2, 2, 2, 2, 2, 2]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    set_default_policy(&t.store, "test.policies.fail");
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(&proj, "never converges", "", AutonomyLevel::default())
        .await
        .expect("start");
    wait_for(
        Duration::from_secs(15),
        "the loop to fail at the cap",
        || {
            t.store
                .workspace(&id)
                .is_some_and(|l| l.state == WorkspaceState::Failed)
        },
    )
    .await;
    let l = t.store.workspace(&id).unwrap();
    assert_ne!(l.state, WorkspaceState::PrOpen);
    assert_eq!(runner.counts(), (4, 4));
    let conv = l.convergence.unwrap_or_default();
    assert_eq!(conv.len(), 4);
    assert!(conv.iter().all(|c| c.findings != 0));
    t.close().await;
}

// ---- PR-backed inbox (loop halves; MergePR / SendBackToLoop are in `prs::tests`)

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbox_derived_from_real_pr() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(
            &proj,
            "Add OAuth provider config",
            "",
            AutonomyLevel::default(),
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "loop to reach pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    let pr = t
        .store
        .pr_by_workspace(&id)
        .expect("a PR for the converged loop");
    let row = t
        .store
        .inbox()
        .unwrap_or_default()
        .into_iter()
        .find(|it| it.id == format!("ib-pr-{}", pr.id))
        .expect("ready inbox row");
    assert_eq!(
        (row.group.as_str(), row.action.as_str(), row.target.clone()),
        ("ready", "Review", format!("pr:{}", pr.id))
    );
    assert!(row.unread);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_bar_iters_count_real() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[1, 0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let base = t.engine.iters_base;
    let id = t
        .engine
        .start_workspace(&proj, "two-iteration task", "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(Duration::from_secs(6), "loop to reach pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    assert_eq!(t.engine.governor().iters(), 2);
    assert_eq!(t.store.resource().iters_today, base + 2);
    assert!(t.events.has("resource.updated"));
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pr_open_publishes_notify() {
    let repo = init_repo();
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(
            &proj,
            "Add a feature that notifies on PR-ready",
            "",
            AutonomyLevel::default(),
        )
        .await
        .unwrap();
    wait_for(Duration::from_secs(10), "loop to reach pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    let notify = t
        .events
        .all()
        .into_iter()
        .find_map(|e| match e {
            EngineEvent::Notify(n) => Some(n),
            _ => None,
        })
        .expect("a notify when the PR opened");
    assert!(notify.target.unwrap_or_default().starts_with("pr:"));
    t.close().await;
}

#[tokio::test]
async fn base_branch_for_reads_store_after_retarget() {
    let t = TestEngine::new(None);
    t.store
        .add_project(recur_domain::Project {
            id: "p".into(),
            name: "p".into(),
            base_branch: "main".into(),
            ..Default::default()
        })
        .unwrap();
    let ws = recur_domain::Workspace {
        id: "w1".into(),
        project_id: "p".into(),
        base_branch: "main".into(),
        branch: "recur/w1".into(),
        ..Default::default()
    };
    t.store.add_workspace(ws.clone()).unwrap();
    let walk = crate::loops::Walk::new(&t.engine, &ws);
    assert_eq!(walk.base_branch(), "main");
    t.store
        .update_workspace("w1", |w| w.base_branch = "develop".into());
    assert_eq!(walk.base_branch(), "develop", "a mid-run retarget wins");
}
