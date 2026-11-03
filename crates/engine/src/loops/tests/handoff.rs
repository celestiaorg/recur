//! Handoff tests: done notes onto the walk, the Reviewer's handoff briefing and
//! the replan turn, board-pass accumulation, and a resumed board keeping its
//! landed children's notes.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::{Provider, TurnResult};
use recur_agent_tools::Host;
use recur_bundles::DEFAULT_LOOP_ID;
use recur_domain::{
    AgentRole, AutonomyLevel, ChildFanOut, Plan, PlanStep, STEP_DOING, STEP_DONE, STEP_PENDING,
    Workspace, WorkspaceState,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::host::EngineHost;
use crate::loop_handle::{LoopHandle, LoopState, accumulate_handoff};
use crate::loop_registry::RunningLoop;
use crate::loops::Walk;
use crate::loops::handoff::{handoff_section, plan_execution_changed};
use crate::loops::test_support::*;
use crate::roles::maestro_system_prompt;
use crate::testing::{TestEngine, add_project, init_repo, wait_for};

#[tokio::test]
async fn coder_done_stores_handoff_notes() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "L-hn".into(),
            project_id: "P1".into(),
            goal: "t".into(),
            state: WorkspaceState::Coding,
            ..Workspace::default()
        })
        .unwrap();
    let lp = LoopHandle::new("L-hn", "P1", "L-hn", LoopState::default());
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&lp),
        cancel: CancellationToken::new(),
    }));
    EngineHost::new(Arc::downgrade(&t.engine))
        .done("L-hn", "CO", "Add precheck", "  S2 needs splitting  ")
        .unwrap();
    assert_eq!(lp.take_done_summary(), "Add precheck");
    assert_eq!(lp.take_done_notes(), "S2 needs splitting");
    assert_eq!(lp.take_done_notes(), "");
}

#[test]
fn handoff_section_rules() {
    assert_eq!(handoff_section("   "), "");
    let got = handoff_section("Skipped the migration");
    assert!(
        got.contains("Skipped the migration") && got.contains("is a finding"),
        "{got}"
    );
}

#[test]
fn accumulate_handoff_rules() {
    assert_eq!(
        accumulate_handoff("S1: split parser", ""),
        "S1: split parser"
    );
    assert_eq!(accumulate_handoff("S1: a", "S2: b"), "S1: a\nS2: b");
}

#[test]
fn plan_execution_changed_detects_files_and_deps() {
    let step = |id: &str, title: &str, files: &[&str], deps: &[&str]| PlanStep {
        id: id.into(),
        title: title.into(),
        files: files.iter().map(|s| (*s).to_owned()).collect(),
        depends_on: deps.iter().map(|s| (*s).to_owned()).collect(),
        ..PlanStep::default()
    };
    let before = Workspace {
        definition_of_done: "Importer\n\nSteps:\n1. Parser\n2. Writer\n".into(),
        plan: Some(Plan {
            body: "Importer".into(),
            steps: vec![
                step("S1", "Parser", &["a.go"], &[]),
                step("S2", "Writer", &[], &["S1"]),
            ],
            ..Plan::default()
        }),
        ..Workspace::default()
    };
    let mut after = before.clone();
    after.plan = Some(Plan {
        body: "Importer".into(),
        steps: vec![
            step("S1", "Parser", &["parser.go"], &[]),
            step("S2", "Writer", &[], &["S1"]),
        ],
        ..Plan::default()
    });
    assert!(plan_execution_changed(&before, &after), "files-only change");
    after.plan.as_mut().unwrap().steps[0].files = vec!["a.go".into()];
    assert!(!plan_execution_changed(&before, &after));
}

/// Records the Reviewer's handoff briefing and counts replan turns.
#[derive(Default)]
struct HandoffRunner {
    reviewer_brief: Mutex<String>,
    replans: Mutex<usize>,
}

impl HandoffRunner {
    fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, _cx| {
            let me = Arc::clone(&me);
            async move {
                let fail = |e: String| recur_agent::ProviderError::failed(e);
                match req.role {
                    AgentRole::Coder => {
                        apply_edits(&req.worktree, &[("work.txt", "edit\n")])
                            .map_err(|e| fail(e.to_string()))?;
                        close_open_findings_mcp(&req.mcp_url).await.map_err(fail)?;
                        post_mcp_tool(
                            &req.mcp_url,
                            "done",
                            json!({"summary": "Wire the precheck", "notes": "S2 is too big: split the parser from the writer"}),
                        )
                        .await
                        .map_err(fail)?;
                        Ok(success(&req.agent_id, ""))
                    }
                    AgentRole::Maestro => {
                        if !req.system_prompt.contains("Implementer handoff:") {
                            return Ok(TurnResult {
                                subtype: "success".into(),
                                ..TurnResult::default()
                            });
                        }
                        *me.replans.lock() += 1;
                        post_mcp_tool(
                            &req.mcp_url,
                            "submit_plan",
                            json!({"summary": "Precheck", "body": "Split per handoff.",
                                "steps": ["Add the precheck", "Split: parser", "Split: writer"]}),
                        )
                        .await
                        .map_err(fail)?;
                        Ok(TurnResult {
                            subtype: "success".into(),
                            ..TurnResult::default()
                        })
                    }
                    AgentRole::Reviewer => {
                        *me.reviewer_brief.lock() = req.system_prompt.clone();
                        Ok(success(&req.agent_id, ""))
                    }
                    _ => Ok(TurnResult {
                        subtype: "success".into(),
                        ..TurnResult::default()
                    }),
                }
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handoff_briefs_reviewer_and_replans() {
    let repo = init_repo();
    let runner = Arc::new(HandoffRunner::default());
    let t = TestEngine::new(Some(runner.provider()));
    t.install_shipped_loop_pack();
    let proj = add_project(&t.store, &repo.path);
    let (mut rec, acquired) = t
        .engine
        .queue_start_workspace(
            &proj,
            "Add a precheck",
            "",
            "",
            &AutonomyLevel::default(),
            DEFAULT_LOOP_ID,
            "recur.policies.aggressive",
            None,
        )
        .await
        .unwrap();
    assert!(acquired);
    let pending = |id: &str, title: &str| PlanStep {
        id: id.into(),
        title: title.into(),
        status: STEP_PENDING.into(),
        ..PlanStep::default()
    };
    let plan = Plan {
        summary: "Precheck".into(),
        body: "Add the precheck.".into(),
        approved: true,
        source: "human".into(),
        steps: vec![pending("S1", "Add the precheck"), pending("S2", "Wire it")],
        ..Plan::default()
    };
    rec.definition_of_done = plan.text();
    rec.plan = Some(plan);
    let id = rec.id.clone();
    t.engine.dispatch(rec).await.unwrap();
    wait_for(Duration::from_secs(20), "loop to open a PR", || {
        t.store.workspace(&id).is_some_and(|l| {
            matches!(
                l.state,
                WorkspaceState::PrOpen | WorkspaceState::Failed | WorkspaceState::Stopped
            )
        })
    })
    .await;
    let ws = t.store.workspace(&id).unwrap();
    assert_eq!(ws.state, WorkspaceState::PrOpen, "phase {}", ws.phase);
    assert!(ws.handoff.contains("S2 is too big"), "{}", ws.handoff);
    assert!(runner.reviewer_brief.lock().contains("S2 is too big"));
    assert!(*runner.replans.lock() >= 1);
    let plan = ws.plan.clone().expect("plan");
    assert_eq!(plan.steps.len(), 3, "revised to three steps");
    let kids = t
        .store
        .workspaces()
        .into_iter()
        .filter(|w| w.parent_id == id)
        .count();
    assert_eq!(kids, 2, "the replanned steps fan out too");
    for s in &plan.steps {
        assert_eq!(s.status, STEP_DONE, "{}", s.id);
    }
    assert!(maestro_system_prompt(&t.engine.maestro_context(&ws)).contains("S2 is too big"));
    t.close().await;
}

#[tokio::test]
async fn record_handoff_accumulates_within_board_then_resets() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "L-ho".into(),
            project_id: "P1".into(),
            goal: "t".into(),
            state: WorkspaceState::Coding,
            ..Workspace::default()
        })
        .unwrap();
    let mut lp = Walk::bare(&t.engine, "L-ho");
    lp.record_handoff("CO", "Coder", "S1: unfinished parser");
    lp.record_handoff("CO", "Coder", "S2: split writer");
    assert_eq!(
        lp.review_handoff(),
        "S1: unfinished parser\nS2: split writer"
    );
    lp.record_handoff("CO", "Coder", "");
    assert_eq!(
        lp.review_handoff(),
        "S1: unfinished parser\nS2: split writer"
    );
    lp.reset_board_handoff();
    lp.record_handoff("CO", "Coder", "");
    assert_eq!(lp.review_handoff(), "");
    assert_eq!(t.store.workspace("L-ho").unwrap().handoff, "");
}

#[tokio::test]
async fn run_board_resume_keeps_landed_handoff() {
    let repo = init_repo();
    let t = TestEngine::new(None);
    let m = recur_git::Manager::new(&repo.path, t.engine.worktree_root());
    let (p, _) = m.add("WS-ho-parent", "ho-parent", "main").await.unwrap();
    let (s2, _) = m.add("WS-ho-s2", "ho-s2", &p.branch).await.unwrap();
    std::fs::write(std::path::Path::new(&s2.path).join("s2.txt"), "from s2\n").unwrap();
    m.commit(&s2.path, "s2 work", "test", "test@example.com")
        .await
        .unwrap();
    let (parent_id, s1_id, s2_id) = ("WS-ho-parent", "WS-ho-s1", "WS-ho-s2");
    let note = "S1: concern about parser";
    t.store
        .add_workspace(Workspace {
            id: parent_id.into(),
            goal: "parent".into(),
            state: WorkspaceState::Coding,
            plan: Some(Plan {
                steps: vec![
                    PlanStep {
                        id: "S1".into(),
                        title: "parser".into(),
                        status: STEP_DONE.into(),
                        files: vec!["s1.txt".into()],
                        note: note.into(),
                        ..PlanStep::default()
                    },
                    PlanStep {
                        id: "S2".into(),
                        title: "writer".into(),
                        status: STEP_DOING.into(),
                        files: vec!["s2.txt".into()],
                        workspace: s2_id.into(),
                        ..PlanStep::default()
                    },
                ],
                ..Plan::default()
            }),
            worktree_path: p.path.clone(),
            branch: p.branch.clone(),
            handoff: note.into(),
            fan_out_waves: 1,
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_workspace(Workspace {
            id: s1_id.into(),
            parent_id: parent_id.into(),
            goal: "parser".into(),
            state: WorkspaceState::Merged,
            archived: true,
            parent_step_ids: vec!["S1".into()],
            fan_out: Some(ChildFanOut {
                wave: 1,
                outcome: "merged".into(),
                note: "concern about parser".into(),
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_workspace(Workspace {
            id: s2_id.into(),
            parent_id: parent_id.into(),
            goal: "writer".into(),
            state: WorkspaceState::PrOpen,
            branch: s2.branch.clone(),
            worktree_path: s2.path.clone(),
            parent_step_ids: vec!["S2".into()],
            fan_out: Some(ChildFanOut {
                wave: 1,
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    let mut lp = Walk::bare(&t.engine, parent_id);
    lp.git = Some(m);
    lp.worktree = p.path.clone();
    lp.plan_structured = true;
    lp.board_handoff = note.into();
    lp.last_handoff = note.into();
    let (_, stop) = lp
        .run_board(&CancellationToken::new(), 1)
        .await
        .expect("run_board");
    assert!(stop.is_none());
    assert!(
        lp.review_handoff().contains("concern about parser"),
        "{}",
        lp.review_handoff()
    );
    assert_eq!(
        t.store.workspace(parent_id).unwrap().plan.unwrap().steps[1].status,
        STEP_DONE
    );
    t.close().await;
}
