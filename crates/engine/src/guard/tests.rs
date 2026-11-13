//! Policy tests, plus guard resolution against the fixture pack (the refusals
//! `TestEngageMissingLoopRefused` and the walk tests rely on).

use std::collections::{BTreeMap, HashMap};

use recur_bundles::{DEFAULT_LOOP_ID, DEFAULT_POLICY_ID, FindingsThreshold, STEP_LOOP_ID};
use recur_domain::{AutonomyLevel, Finding, Plan, PlanStep, Severity, Workspace};

use super::*;
use crate::testing::TestEngine;

#[test]
fn synthetic_policy_from_autonomy() {
    let p = synthetic_policy("recur.policies.cautious", &AutonomyLevel::Cautious);
    assert_eq!((p.max_rounds, p.plan_approval.as_str()), (2, "always"));
    assert!(p.findings_threshold.all_zero);
    let p = synthetic_policy(DEFAULT_POLICY_ID, &AutonomyLevel::Balanced);
    assert_eq!(p.max_rounds, 4);
    assert_eq!(p.findings_threshold.levels.get("medium"), Some(&0));
    let lp = synthetic_pr_loop();
    assert_eq!(lp.entry, "planning");
    assert!(
        skip_plan_approval(&p, &Workspace::default()),
        "balanced with no steps skips"
    );
    let aggressive = synthetic_policy("", &AutonomyLevel::Aggressive);
    assert_eq!(aggressive.id, "recur.policies.aggressive");
}

#[test]
fn plan_approval_rules() {
    let with_steps = Workspace {
        plan: Some(Plan {
            steps: vec![PlanStep {
                id: "S1".into(),
                ..PlanStep::default()
            }],
            ..Plan::default()
        }),
        ..Workspace::default()
    };
    let balanced = synthetic_policy("", &AutonomyLevel::Balanced);
    assert!(
        !skip_plan_approval(&balanced, &with_steps),
        "if_structured gates a stepped plan"
    );
    let cautious = synthetic_policy("recur.policies.cautious", &AutonomyLevel::Cautious);
    assert!(!skip_plan_approval(&cautious, &Workspace::default()));
    let mut approved = with_steps.clone();
    approved.plan.as_mut().unwrap().approved = true;
    assert!(
        skip_plan_approval(&cautious, &approved),
        "an approved plan never gates"
    );
    let never = synthetic_policy("recur.policies.aggressive", &AutonomyLevel::Aggressive);
    assert!(skip_plan_approval(&never, &with_steps));
}

#[test]
fn findings_bar_counts() {
    let th = FindingsThreshold {
        all_zero: false,
        levels: BTreeMap::from([
            ("critical".to_owned(), 0),
            ("high".to_owned(), 0),
            ("medium".to_owned(), 0),
        ]),
    };
    let counts = |pairs: &[(&str, i64)]| -> HashMap<String, i64> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    };
    assert!(!recur_bundles::findings_block(
        &th,
        &counts(&[("low", 2), ("info", 1)])
    ));
    assert!(recur_bundles::findings_block(
        &th,
        &counts(&[("medium", 1)])
    ));
    assert_eq!(
        countable_findings(&th, &counts(&[("medium", 1), ("low", 9)])),
        1
    );
    let all = FindingsThreshold {
        all_zero: true,
        levels: BTreeMap::new(),
    };
    assert_eq!(
        countable_findings(&all, &counts(&[("low", 2), ("info", 1)])),
        3
    );
}

#[test]
fn phase_overrides() {
    let g = ResolvedGuard {
        policy: synthetic_policy("", &AutonomyLevel::Balanced),
        loop_def: synthetic_pr_loop(),
        from_bundle: false,
    };
    let mut ph = recur_bundles::LoopPhase::default();
    assert_eq!(g.phase_max_rounds(&ph), 4);
    assert_eq!(g.phase_on_max_rounds(&ph), "escalate");
    ph.max_rounds = Some(7);
    ph.on_max_rounds = "fail".into();
    assert_eq!(g.phase_max_rounds(&ph), 7);
    assert_eq!(g.phase_on_max_rounds(&ph), "fail");
    // A stricter phase bar wins; a looser one falls back to the policy.
    ph.findings = Some(FindingsThreshold {
        all_zero: true,
        levels: BTreeMap::new(),
    });
    assert!(g.phase_threshold(&ph).all_zero);
    ph.findings = Some(FindingsThreshold {
        all_zero: false,
        levels: BTreeMap::from([("critical".to_owned(), 3)]),
    });
    assert_eq!(g.phase_threshold(&ph), g.policy.findings_threshold);
    let zero = ResolvedGuard {
        policy: recur_bundles::Policy::default(),
        ..g.clone()
    };
    assert_eq!(
        zero.phase_max_rounds(&recur_bundles::LoopPhase::default()),
        4
    );
}

#[tokio::test]
async fn resolve_guard_defaults_and_refusals() {
    let t = TestEngine::new(None);
    let e = &t.engine;
    let g = e.resolve_guard("", "", &AutonomyLevel::Balanced).unwrap();
    assert_eq!(g.loop_def.id, DEFAULT_LOOP_ID);
    assert_eq!(g.policy.id, DEFAULT_POLICY_ID);
    assert!(!g.from_bundle);
    let g = e.resolve_guard("", "", &AutonomyLevel::Cautious).unwrap();
    assert_eq!(g.policy.id, "recur.policies.cautious");

    assert_eq!(
        e.resolve_guard("test.loops.does-not-exist", "", &AutonomyLevel::Balanced)
            .unwrap_err(),
        "workflow test.loops.does-not-exist not found"
    );
    assert_eq!(
        e.guard_reason("", "test.policies.fail", &AutonomyLevel::Balanced),
        "policy test.policies.fail not found"
    );
    let step = e
        .resolve_guard(STEP_LOOP_ID, "", &AutonomyLevel::Balanced)
        .unwrap();
    assert_eq!(step.loop_def.id, STEP_LOOP_ID);
    assert_eq!(step.loop_def.phases.len(), 2);

    t.install_fixture_pack();
    let g = e
        .resolve_guard(
            "test.loops.docs",
            "test.policies.fail",
            &AutonomyLevel::Balanced,
        )
        .unwrap();
    assert!(g.from_bundle);
    assert_eq!(g.policy.on_max_rounds, "fail");
    assert_eq!(g.loop_def.name, "Docs loop");

    let mut set = t.store.settings();
    set.disabled_bundle_loops = vec!["test.loops.docs".into()];
    set.disabled_bundle_policies = vec!["test.policies.fail".into()];
    t.store.set_settings(set.clone());
    assert_eq!(
        e.guard_reason("test.loops.docs", "", &AutonomyLevel::Balanced),
        "workflow test.loops.docs is disabled"
    );
    assert_eq!(
        e.guard_reason("", "test.policies.fail", &AutonomyLevel::Balanced),
        "policy test.policies.fail is disabled"
    );
    set.disabled_bundle_loops.clear();
    set.disabled_bundle_policies.clear();
    set.disabled_bundle_roles = vec!["test.roles.writer".into()];
    t.store.set_settings(set.clone());
    assert_eq!(
        e.guard_reason("test.loops.docs", "", &AutonomyLevel::Balanced),
        "role test.roles.writer is disabled"
    );
    set.disabled_bundle_roles.clear();
    set.default_loop = "test.loops.converge".into();
    set.default_policy = "test.policies.fail".into();
    t.store.set_settings(set);
    let g = e.resolve_guard("", "", &AutonomyLevel::Balanced).unwrap();
    assert_eq!(
        (g.loop_def.id.as_str(), g.policy.id.as_str()),
        ("test.loops.converge", "test.policies.fail")
    );

    t.install_test_pack(
        "Broken",
        r#"
[bundle]
name = "Broken"
id = "broken"
version = "1.0.0"

[[loop]]
id = "broken.loop"
name = "Broken"
entry = "converge"
conductor = "recur.roles.maestro"

[[loop.phase]]
id = "converge"
uses = "converge"
[loop.phase.roles]
implementer = "nobody.roles.ghost"
reviewer = "recur.roles.reviewer"
"#,
    );
    assert_eq!(
        e.guard_reason("broken.loop", "", &AutonomyLevel::Balanced),
        "role nobody.roles.ghost not found"
    );
}

#[tokio::test]
async fn child_guard_derives_from_parent_converge() {
    let t = TestEngine::new(None);
    t.install_fixture_pack();
    t.store
        .add_workspace(Workspace {
            id: "parent".into(),
            project_id: "p".into(),
            loop_id: "test.loops.docs".into(),
            ..Workspace::default()
        })
        .unwrap();
    let child = Workspace {
        id: "child".into(),
        parent_id: "parent".into(),
        parent_phase_id: "converge".into(),
        ..Workspace::default()
    };
    let g = t.engine.guard_for_workspace(&child).unwrap();
    assert_eq!(g.loop_def.id, STEP_LOOP_ID);
    assert_eq!(g.loop_def.name, "Docs loop step");
    assert_eq!(g.loop_def.entry, "converge");
    let conv = &g.loop_def.phases[0];
    assert_eq!(
        conv.roles.get("implementer").map(String::as_str),
        Some("test.roles.writer")
    );
    assert_eq!(conv.next, "done");
    assert_eq!(g.loop_def.phases[1].uses, recur_bundles::VERB_ARTIFACT);
    assert_eq!(
        g.loop_def.roles.get("conductor").map(String::as_str),
        Some("recur.roles.maestro")
    );

    // A missing parent resolves the child by its own ids.
    let orphan = Workspace {
        parent_id: "gone".into(),
        ..Workspace::default()
    };
    assert_eq!(
        t.engine.guard_for_workspace(&orphan).unwrap().loop_def.id,
        DEFAULT_LOOP_ID
    );
}

#[tokio::test]
async fn findings_bar_on_stored_findings() {
    let t = TestEngine::new(None);
    for (sev, status) in [
        (Severity::Medium, "open"),
        (Severity::Low, "open"),
        (Severity::High, "resolved"),
        (Severity::Other(String::new()), "open"),
    ] {
        t.store.add_finding(Finding {
            workspace_id: "w".into(),
            severity: sev,
            status: status.into(),
            message: "m".into(),
            ..Finding::default()
        });
    }
    let counts = t.engine.open_findings_counts("w");
    assert_eq!(
        counts.get("medium"),
        Some(&2),
        "empty severity counts as medium"
    );
    assert_eq!(counts.get("low"), Some(&1));
    assert_eq!(counts.get("high"), None);
    let balanced = synthetic_policy("", &AutonomyLevel::Balanced);
    assert_eq!(
        t.engine
            .findings_bar_blocks("w", &balanced.findings_threshold),
        Some("2 countable findings".to_owned())
    );
    let aggressive = synthetic_policy("recur.policies.aggressive", &AutonomyLevel::Aggressive);
    assert_eq!(
        t.engine
            .findings_bar_blocks("w", &aggressive.findings_threshold),
        None
    );
}
