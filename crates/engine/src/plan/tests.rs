//! Plan tests (the approval-gate run is E3's).

use recur_agent::display::PLAN_MAX_PERSIST_CHARS;
use recur_agent_tools::{CreateArtifactRequest, Host, PlanStepMeta, PlanSubmission};
use recur_domain::{PlanStep, STEP_DONE, STEP_PENDING, WorkspaceState};

use super::*;
use crate::bus::EngineEvent;
use crate::host::EngineHost;
use crate::testing::TestEngine;
use crate::turn::test_support::add_solo_workspace;

fn sub(summary: &str, body: &str, steps: &[&str]) -> PlanSubmission {
    PlanSubmission {
        summary: summary.into(),
        body: body.into(),
        steps: steps.iter().map(|s| (*s).to_owned()).collect(),
        ..PlanSubmission::default()
    }
}

fn fixture() -> (TestEngine, EngineHost) {
    let te = TestEngine::new(None);
    add_solo_workspace(&te.engine, "W1");
    let h = EngineHost::new(std::sync::Arc::downgrade(&te.engine));
    (te, h)
}

fn plan(te: &TestEngine) -> recur_domain::Plan {
    te.store.workspace("W1").unwrap().plan.unwrap()
}

fn carried_plan(te: &TestEngine, pred: impl Fn(&recur_domain::Plan) -> bool) -> bool {
    te.events.by_name("workspace.updated").iter().any(|e| {
        matches!(e, EngineEvent::WorkspaceUpdated(u) if u.workspace_id == "W1" && u.plan.as_ref().is_some_and(&pred))
    })
}

#[tokio::test]
async fn submit_plan_persists_structured_artifact() {
    let (te, h) = fixture();
    let status = h
        .submit_plan(
            "W1",
            "PA",
            sub(
                "Add a dark-mode toggle",
                "Touch Settings.svelte. Persist the preference in localStorage so it survives reload.",
                &["Add the toggle to Settings", "Persist the preference"],
            ),
        )
        .await;
    assert_eq!(
        status,
        "Plan recorded (2 steps). The human reviews and approves it in the Plan panel."
    );
    let p = plan(&te);
    assert_eq!(p.steps.len(), 2);
    assert!(
        carried_plan(&te, |p| p.steps.len() == 2),
        "workspace.updated carries the plan"
    );
    assert_eq!(
        (p.steps[0].id.as_str(), p.steps[0].status.as_str()),
        ("S1", STEP_PENDING)
    );
    assert_eq!(p.source, "plan-chat");
    assert_eq!(p.chat_code, "PA");
    assert!(!p.approved);
    let dod = te.store.workspace("W1").unwrap().definition_of_done;
    assert!(dod.contains("Touch Settings.svelte") && dod.contains("1. Add the toggle to Settings"));
    let card = te
        .events
        .by_name("workspace.event")
        .into_iter()
        .find_map(|e| match e {
            EngineEvent::WorkspaceEvent(w) if w.event.card == "plan" => Some(w.event),
            _ => None,
        })
        .expect("plan card");
    assert_eq!(card.name, "Agent");
    assert_eq!(
        card.text,
        "Filed a structured plan: 2 step(s) — Add a dark-mode toggle"
    );
    assert_eq!(
        card.data,
        r#"{"summary":"Add a dark-mode toggle","steps":[{"id":"S1","title":"Add the toggle to Settings"},{"id":"S2","title":"Persist the preference"}],"source":"plan-chat"}"#
    );

    assert_eq!(
        h.update_plan_step("W1", "PA", "S2", "done", "landed with tests")
            .await,
        "Step S2 (Persist the preference) → done."
    );
    assert_eq!(
        h.update_plan_step("W1", "PA", "S9", "done", "").await,
        "Refused: no plan step with id \"S9\"."
    );
    let p = plan(&te);
    assert_eq!(p.steps[1].status, STEP_DONE);
    assert_eq!(p.steps[1].note, "landed with tests");
    assert!(
        te.store
            .workspace("W1")
            .unwrap()
            .definition_of_done
            .contains("(done)")
    );

    h.submit_plan("W1", "MA", sub("re-plan", "", &["one"]))
        .await;
    assert_eq!(plan(&te).source, "loop-planner");
    assert_eq!(
        plan(&te).chat_code,
        "PA",
        "a Maestro filing keeps the authoring tab"
    );

    assert_eq!(te.engine.approve_plan("W1"), "Plan approved.");
    assert!(plan(&te).approved);
    assert_eq!(
        te.engine.approve_plan("W1"),
        "The plan is already approved."
    );
    assert_eq!(
        te.engine.approve_plan("nope"),
        "Refused: that workspace no longer exists."
    );
    te.close().await;
}

#[tokio::test]
async fn refusals() {
    let (te, h) = fixture();
    assert_eq!(
        h.update_plan_step("W1", "PA", "S1", "done", "").await,
        "Refused: this workspace has no structured plan steps."
    );
    assert_eq!(
        te.engine.approve_plan("W1"),
        "Refused: this workspace has no structured plan to approve."
    );
    assert_eq!(
        h.submit_plan("nope", "PA", sub("x", "", &[])).await,
        "Refused: that workspace no longer exists."
    );
    assert_eq!(
        te.engine.submit_human_plan("W1", " ", "", &[], &[]),
        "Refused: the plan needs a summary."
    );
    assert_eq!(
        te.engine.update_human_plan_step("W1", "S1", "bogus"),
        "Refused: unknown step status \"bogus\"."
    );
    te.close().await;
}

#[tokio::test]
async fn adopts_prose_plan_from_create_plan() {
    let (te, h) = fixture();
    let body = "Summary: touch work.txt.\n\nSteps:\n1. Write the marker\n\nDefinition of done: file exists.";
    assert!(te.engine.adopt_prose_plan("W1", "PA", body));
    let p = plan(&te);
    assert_eq!(p.body, body);
    assert_eq!(p.summary, "Summary: touch work.txt.");
    assert!(p.steps.is_empty());
    assert_eq!(p.source, "plan-chat");
    assert_eq!(te.store.workspace("W1").unwrap().definition_of_done, body);
    assert!(carried_plan(&te, |p| p.body == body));
    h.submit_plan(
        "W1",
        "PA",
        sub("Real structured plan", "", &["Do the thing"]),
    )
    .await;
    assert!(!te.engine.adopt_prose_plan("W1", "PA", "should not replace"));
    let p = plan(&te);
    assert_eq!(
        (p.steps.len(), p.summary.as_str()),
        (1, "Real structured plan")
    );
    te.close().await;
}

#[tokio::test]
async fn submit_plan_resolves_attachments() {
    let (te, h) = fixture();
    let art = te
        .engine
        .create_artifact(
            "W1",
            "PA",
            CreateArtifactRequest {
                title: "Flow".into(),
                kind: "md".into(),
                content: "# flow\n".into(),
            },
        )
        .unwrap();
    let dir = recur_store::paths::attachments_dir(&te.data_dir, "W1").unwrap();
    std::fs::write(dir.join("shot.png"), "png").unwrap();
    let status = h
        .submit_plan(
            "W1",
            "PA",
            PlanSubmission {
                attachment_ids: vec![art.id.clone(), "shot".into(), "missing-id".into()],
                ..sub(
                    "Use the diagram",
                    "See the attached flow.",
                    &["Follow the diagram"],
                )
            },
        )
        .await;
    assert_eq!(
        status,
        "Plan recorded (1 steps, 2 attachments). Unknown attachment ids: missing-id. The human reviews and approves it in the Plan panel."
    );
    let p = plan(&te);
    assert_eq!(p.attachments.len(), 2);
    assert_eq!(p.attachments[1].kind, "png");
    assert_eq!(p.attachments[1].title, "shot.png");
    let dod = te.store.workspace("W1").unwrap().definition_of_done;
    assert!(dod.contains("See the attached flow.") && dod.contains(&art.path));
    te.close().await;
}

#[tokio::test]
async fn resubmit_preserves_step_status() {
    let (te, h) = fixture();
    h.submit_plan(
        "W1",
        "PA",
        sub(
            "Ship the toggle",
            "Original brief.",
            &["Add the toggle", "Persist the preference"],
        ),
    )
    .await;
    h.update_plan_step("W1", "PA", "S1", "done", "landed with tests")
        .await;
    te.store.update_workspace("W1", |l| {
        l.state = WorkspaceState::Coding;
        l.progress = 0.99;
    });
    let steps = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let status = te.engine.submit_human_plan(
        "W1",
        "Ship the toggle",
        "Reworded brief.",
        &steps(&["Add the toggle", "Persist the preference"]),
        &[],
    );
    assert!(!status.starts_with("Refused"), "{status}");
    let p = plan(&te);
    assert_eq!(
        (p.steps[0].id.as_str(), p.steps[0].status.as_str()),
        ("S1", STEP_DONE)
    );
    assert!(!p.steps[0].note.is_empty());
    assert_eq!(
        (p.steps[1].id.as_str(), p.steps[1].status.as_str()),
        ("S2", STEP_PENDING)
    );
    assert_eq!(p.body, "Reworded brief.");
    assert_eq!(p.source, "human");
    assert_eq!(te.store.workspace("W1").unwrap().progress, 0.15 + 0.7 * 0.5);

    te.engine.submit_human_plan(
        "W1",
        "Start over",
        "New brief.",
        &steps(&["Do something else"]),
        &[],
    );
    let p = plan(&te);
    assert_eq!(p.steps.len(), 1);
    assert_eq!(
        (p.steps[0].id.as_str(), p.steps[0].status.as_str()),
        ("S1", STEP_PENDING)
    );

    h.submit_plan(
        "W1",
        "PA",
        sub(
            "Ship the toggle",
            "Brief.",
            &["Add the toggle", "Persist the preference"],
        ),
    )
    .await;
    h.update_plan_step("W1", "PA", "S1", "done", "toggle landed")
        .await;
    te.engine.submit_human_plan(
        "W1",
        "Ship the toggle",
        "Brief.",
        &steps(&["Add migration", "Add the toggle", "Persist the preference"]),
        &[],
    );
    let p = plan(&te);
    assert_eq!(p.steps.len(), 3);
    assert_eq!(
        (
            p.steps[0].id.as_str(),
            p.steps[0].title.as_str(),
            p.steps[0].status.as_str()
        ),
        ("S1", "Add migration", STEP_PENDING)
    );
    assert_eq!(
        (
            p.steps[1].id.as_str(),
            p.steps[1].title.as_str(),
            p.steps[1].status.as_str()
        ),
        ("S2", "Add the toggle", STEP_DONE)
    );
    let dod = te.store.workspace("W1").unwrap().definition_of_done;
    assert!(dod.contains("1. Add migration") && dod.contains("2. Add the toggle (done)"));
    te.close().await;
}

fn step(id: &str, title: &str, status: &str) -> PlanStep {
    PlanStep {
        id: id.into(),
        title: title.into(),
        status: status.into(),
        ..PlanStep::default()
    }
}

fn titles(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn merge_retitle_keeps_status() {
    let prev = vec![
        PlanStep {
            note: "toggle landed".into(),
            ..step("S1", "Add the toggle", STEP_DONE)
        },
        PlanStep {
            note: "landed with tests".into(),
            ..step("S2", "Persist the preferene", STEP_DONE)
        },
        step("S3", "Ship it", STEP_PENDING),
    ];
    let got = merge_plan_steps(
        &prev,
        &titles(&["Add the toggle", "Persist the preference", "Ship it"]),
        &titles(&["S1", "S2", "S3"]),
        &[],
    );
    assert_eq!(got.len(), 3);
    assert_eq!(
        (
            got[1].id.as_str(),
            got[1].title.as_str(),
            got[1].status.as_str(),
            got[1].note.as_str()
        ),
        (
            "S2",
            "Persist the preference",
            STEP_DONE,
            "landed with tests"
        )
    );
    assert_eq!(
        (got[0].status.as_str(), got[2].status.as_str()),
        (STEP_DONE, STEP_PENDING)
    );
    let rewritten = merge_plan_steps(&prev, &titles(&["Do something else"]), &[], &[]);
    assert_eq!(rewritten.len(), 1);
    assert_eq!(
        (rewritten[0].id.as_str(), rewritten[0].status.as_str()),
        ("S1", STEP_PENDING)
    );
}

#[test]
fn merge_remaps_dependencies_on_renumber() {
    let prev = vec![
        PlanStep {
            files: vec!["api.go".into()],
            ..step("S1", "API", STEP_DONE)
        },
        PlanStep {
            files: vec!["ui.go".into()],
            depends_on: vec!["S1".into()],
            ..step("S2", "UI", STEP_PENDING)
        },
        PlanStep {
            depends_on: vec!["S2".into()],
            ..step("S3", "Docs", STEP_PENDING)
        },
    ];
    let got = merge_plan_steps(&prev, &titles(&["UI", "API", "Docs"]), &[], &[]);
    assert_eq!((got[0].id.as_str(), got[0].title.as_str()), ("S1", "UI"));
    assert_eq!(got[0].depends_on, ["S2"]);
    assert_eq!(got[2].depends_on, ["S1"]);
    let got = merge_plan_steps(&prev, &titles(&["API", "Tests", "Docs"]), &[], &[]);
    assert_eq!(got[2].title, "Docs");
    assert!(got[2].depends_on.is_empty());
    assert_eq!(
        (got[1].title.as_str(), got[1].status.as_str()),
        ("Tests", STEP_PENDING)
    );
    assert!(got[1].depends_on.is_empty());
    let got = merge_plan_steps(
        &prev,
        &titles(&["API", "UI", "Docs"]),
        &[],
        &[
            PlanStepMeta {
                explicit: true,
                ..PlanStepMeta::default()
            },
            PlanStepMeta {
                explicit: true,
                depends_on: vec!["S1".into()],
                ..PlanStepMeta::default()
            },
            PlanStepMeta::default(),
        ],
    );
    assert!(got[0].files.is_empty(), "explicit empty files clear");
    assert!(got[1].files.is_empty());
    assert_eq!(got[1].depends_on, ["S1"]);
    assert_eq!(
        got[2].depends_on,
        ["S2"],
        "a bare title keeps its remapped deps"
    );
}

#[tokio::test]
async fn capped_body_keeps_the_checklist_in_the_mirror() {
    let (te, h) = fixture();
    let body = "x".repeat(PLAN_MAX_PERSIST_CHARS + 800);
    let status = h
        .submit_plan(
            "W1",
            "PA",
            sub("Long brief", &body, &["Keep the checklist"]),
        )
        .await;
    assert!(!status.starts_with("Refused"));
    let p = plan(&te);
    assert!(p.body.len() < PLAN_MAX_PERSIST_CHARS);
    let dod = te.store.workspace("W1").unwrap().definition_of_done;
    assert!(dod.contains("Steps:") && dod.contains("1. Keep the checklist"));
    te.close().await;
}

#[test]
fn first_paragraph_strips_markdown() {
    assert_eq!(
        first_paragraph("# Add dark mode\n\nTouch Settings.svelte."),
        "Add dark mode"
    );
    assert_eq!(first_paragraph("##\n\nDo the work."), "Do the work.");
    assert_eq!(
        first_paragraph("- ship the toggle\n\nDetails."),
        "ship the toggle"
    );
    assert_eq!(first_paragraph("* starred"), "starred");
    assert_eq!(first_paragraph("   "), "");
}

#[test]
fn sources_and_authors() {
    assert_eq!(plan_source("PA2"), "plan-chat");
    assert_eq!(plan_source("AR"), "human");
    assert_eq!(plan_source("PL"), "loop-planner");
    let prev = recur_domain::Plan {
        chat_code: " PA3 ".into(),
        ..Default::default()
    };
    assert_eq!(plan_author_chat_code("PA2", Some(&prev)), "PA2");
    assert_eq!(plan_author_chat_code("AR", Some(&prev)), "PA3");
    assert_eq!(plan_author_chat_code("MA", None), "");
}
