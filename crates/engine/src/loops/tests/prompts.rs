//! Loop prompt tests: bundle role prompts never carry the loop's walk recipe,
//! prompt-action slots keep their display label, the goal display is every
//! specialist's user bubble, and Linear raw regions unwrap in phase prompts
//! without losing the label.

use std::time::Duration;

use recur_bundles::{Catalog, LoopDef, LoopPhase, Role, VERB_PLAN, VERB_VERIFY};
use recur_domain::{AgentRole, AutonomyLevel, Workspace, WorkspaceState};
use tokio_util::sync::CancellationToken;

use crate::composer::ExpandOpts;
use crate::guard::ResolvedGuard;
use crate::loops::Walk;
use crate::loops::test_support::*;
use crate::testing::{TestEngine, add_project, init_repo, wait_for};

#[tokio::test]
async fn bundle_system_prompt_omits_walk_recipe() {
    let t = TestEngine::new(None);
    let mission = "The human already wrote this change. Review the current worktree, then fix and re-review until the policy bar passes.";
    let mut l = Walk::bare(&t.engine, "x");
    l.set_guard(ResolvedGuard {
        loop_def: LoopDef {
            prompts: [("mission".to_owned(), mission.to_owned())].into(),
            ..LoopDef::default()
        },
        ..ResolvedGuard::default()
    });
    l.catalog = Catalog {
        roles: vec![Role {
            id: "recur.roles.reviewer".into(),
            prompt: "You are an independent Reviewer. Read the diff. Do not edit.".into(),
            ..Role::default()
        }],
        ..Catalog::default()
    };
    let got = l.bundle_system_prompt("recur.roles.reviewer", "FALLBACK", mission, "");
    assert!(!got.contains("fix and re-review"), "{got}");
    assert!(got.contains("Do not edit"));
    assert!(!got.contains("FALLBACK"));
    let got = l.bundle_system_prompt("recur.roles.reviewer", "FALLBACK", "add dark mode", "");
    assert!(got.contains("Task: add dark mode"), "{got}");
}

async fn idle(t: &TestEngine, id: &str) {
    wait_for(Duration::from_secs(15), "loop to idle", || {
        t.store
            .workspace(id)
            .is_some_and(|l| matches!(l.state, WorkspaceState::Paused | WorkspaceState::Stopped))
    })
    .await;
}

fn first_spec(runner: &ConvergeRunner, role: AgentRole) -> recur_agent::TurnRequest {
    runner
        .state
        .lock()
        .specs
        .iter()
        .find(|s| s.role == role)
        .cloned()
        .unwrap_or_else(|| panic!("no {role:?} spec"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_specialist_prompt_action_uses_display() {
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let body = "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE Review the changes in this worktree and file every real bug\",\"display\":\"Please review the changes in this worktree and file any real issues\"}'\n";
    write_prompt_bundle(&t, "review", body, "recur.chattest.review");
    t.install_test_pack(
        "TestLoops",
        r#"
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[loop]]
id = "test.loops.prompt-display"
name = "Prompt display"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "review"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
[loop.phase.prompts]
review = "/review"
"#,
    );
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "Review the current diff",
            "",
            AutonomyLevel::default(),
            "test.loops.prompt-display",
            "",
        )
        .await
        .unwrap();
    idle(&t, &id).await;
    let review = first_spec(&runner, AgentRole::Reviewer);
    let want = "Please review the changes in this worktree and file any real issues";
    assert_eq!(review.display_text, want);
    assert!(
        review.prompt.contains("WIRE Review the changes"),
        "{}",
        review.prompt
    );
    assert_eq!(first_user_bubble(&t.engine, &id, "RV").await, want);
    t.close().await;
}

#[test]
fn loop_goal_label() {
    for (goal, display, want) in [
        ("add dark mode", "", "add dark mode"),
        (
            "full Linear body",
            "Linear ENG-123 · title",
            "Linear ENG-123 · title",
        ),
        ("task", "  ", "task"),
    ] {
        let ws = Workspace {
            goal: goal.into(),
            goal_display: display.into(),
            ..Workspace::default()
        };
        assert_eq!(ws.goal_label(), want);
    }
}

const GOAL_DISPLAY_PACK: &str = r#"
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[loop]]
id = "test.loops.goal-display"
name = "Goal display"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "implement"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_goal_display_is_user_bubble() {
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_test_pack("TestLoops", GOAL_DISPLAY_PACK);
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let wire = "Linear ENG-123\n\nFull issue body that must reach the model.";
    let label = "Linear ENG-123 · title";
    let (rec, acquired) = t
        .engine
        .queue_start_workspace(
            &proj,
            wire,
            label,
            "",
            &AutonomyLevel::default(),
            "test.loops.goal-display",
            "",
            None,
        )
        .await
        .unwrap();
    assert_eq!(rec.goal, wire);
    assert_eq!(rec.goal_display, label);
    assert_eq!(rec.short, label);
    assert!(acquired);
    let id = rec.id.clone();
    t.engine.dispatch(rec).await.unwrap();
    idle(&t, &id).await;
    let coder = first_spec(&runner, AgentRole::Coder);
    assert_eq!(coder.display_text, label);
    assert!(coder.prompt.contains(wire));
    let review = first_spec(&runner, AgentRole::Reviewer);
    assert!(!review.display_text.contains("Full issue body"));
    assert!(review.display_text.contains(label));
    assert!(review.prompt.contains(wire));
    assert_eq!(t.store.agent(&id, "CO").expect("coder agent").task, label);
    assert_eq!(first_user_bubble(&t.engine, &id, "CO").await, label);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_implement_prose_keeps_prompt_display() {
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    let implement = "Ship the change by editing the worktree.";
    t.install_test_pack(
        "TestLoops",
        &format!(
            r#"
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[loop]]
id = "test.loops.implement-prose"
name = "Implement prose"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "implement"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
[loop.phase.prompts]
implement = "{implement}"
"#
        ),
    );
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let goal = "add dark mode";
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            goal,
            "",
            AutonomyLevel::default(),
            "test.loops.implement-prose",
            "",
        )
        .await
        .unwrap();
    idle(&t, &id).await;
    let coder = first_spec(&runner, AgentRole::Coder);
    assert!(coder.prompt.contains(implement));
    assert_ne!(coder.display_text, goal);
    assert_eq!(first_user_bubble(&t.engine, &id, "CO").await, implement);
    t.close().await;
}

/// The wire `bundles/Linear/src/linear.sh` produces.
fn linear_issue_wire(ident: &str, title: &str, body: &str) -> String {
    [
        &format!("Work on Linear issue {ident} in this workspace. Implement what the issue asks; do not expand scope."),
        "",
        "⟦raw:lin⟧",
        &format!("Identifier: {ident}"),
        &format!("Title: {title}"),
        "",
        "## Description",
        body,
        "⟦/raw:lin⟧",
        "",
    ]
    .join("\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_goal_display_survives_linear_raw_expand() {
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::new(Some(runner.provider()));
    t.install_test_pack(
        "TestLoops",
        r#"
[bundle]
name = "Test Loops"
id = "test.loops"
version = "1.0.0"

[[loop]]
id = "test.loops.raw-label"
name = "Raw label"
entry = "planning"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "planning"
uses = "plan"
role = "recur.roles.planner"
next = "converge"

[[loop.phase]]
id = "converge"
uses = "converge"
start = "implement"
[loop.phase.roles]
implementer = "recur.roles.coder"
reviewer = "recur.roles.reviewer"
"#,
    );
    let wire = linear_issue_wire(
        "ENG-1",
        "Fix the thing",
        "Full issue body that must reach the model.",
    );
    let label = "Linear ENG-1 · Fix the thing";
    let cancel = CancellationToken::new();
    let composer = t
        .engine
        .expand_composer(&cancel, &wire, ExpandOpts::default())
        .await
        .unwrap();
    assert!(!composer.display.is_empty(), "the unwrap sets a display");

    let mut l = Walk::bare(&t.engine, "x");
    l.goal = wire.clone();
    l.goal_display = label.into();
    l.set_current_phase(&LoopPhase {
        id: "planning".into(),
        uses: VERB_PLAN.into(),
        ..LoopPhase::default()
    });
    let (plan_prompt, plan_disp) = l.expand_phase_prompt(&cancel, "plan", &wire).await.unwrap();
    assert_eq!(plan_disp, "", "keep GoalDisplay");
    assert!(!plan_prompt.contains("⟦raw:lin⟧") && !plan_prompt.contains("⟦/raw:lin⟧"));
    assert!(plan_prompt.contains("Full issue body that must reach the model."));
    l.set_current_phase(&LoopPhase {
        id: "verify".into(),
        uses: VERB_VERIFY.into(),
        ..LoopPhase::default()
    });
    let (qa_prompt, qa_disp) = l
        .expand_phase_prompt(&cancel, "verify", &format!("Verify in the browser: {wire}"))
        .await
        .unwrap();
    assert_eq!(qa_disp, "");
    assert!(!qa_prompt.contains("⟦raw:lin⟧") && !qa_prompt.contains("⟦/raw:lin⟧"));
    assert!(qa_prompt.contains("Full issue body that must reach the model."));
    let mut blank = Walk::bare(&t.engine, "y");
    blank.goal = wire.clone();
    let (_, d) = blank
        .expand_phase_prompt(&cancel, "plan", &wire)
        .await
        .unwrap();
    assert_eq!(d, "");

    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let (rec, acquired) = t
        .engine
        .queue_start_workspace(
            &proj,
            &wire,
            label,
            "",
            &AutonomyLevel::default(),
            "test.loops.raw-label",
            "",
            None,
        )
        .await
        .unwrap();
    assert!(acquired);
    let id = rec.id.clone();
    t.engine.dispatch(rec).await.unwrap();
    wait_for(Duration::from_secs(15), "raw-label loop to idle", || {
        t.store.workspace(&id).is_some_and(|ws| {
            matches!(
                ws.state,
                WorkspaceState::Paused | WorkspaceState::Stopped | WorkspaceState::PrOpen
            )
        })
    })
    .await;
    let planner = first_spec(&runner, AgentRole::Maestro);
    assert_eq!(planner.display_text, label);
    assert!(!planner.prompt.contains("⟦raw:lin⟧"));
    assert!(
        planner
            .prompt
            .contains("Full issue body that must reach the model.")
    );
    assert_eq!(first_user_bubble(&t.engine, &id, "MA").await, label);
    t.close().await;
}
