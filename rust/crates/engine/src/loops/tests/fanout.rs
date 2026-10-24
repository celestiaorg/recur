//! Port of Go `fanout_test.go`: ready steps and file grouping, child
//! workspaces per step group merged back, failed children not respawned, no
//! wave barrier, planner-filed plans, docs fan-out, the child loop shape,
//! conflicts (resume, sibling merges, resolve failure), preserve failures,
//! the dispatch_mu window, and stopping/resuming children.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use recur_bundles::{
    DEFAULT_LOOP_ID, DOCS_LOOP_ID, LoopDef, LoopPhase, STEP_LOOP_ID, VERB_ARTIFACT, VERB_CONVERGE,
    VERB_ENSURE_PR, VERB_PLAN,
};
use recur_domain::{
    AutonomyLevel, ChildFanOut, FEED_CARD_FAN_OUT, FEED_CARD_PLAN, Plan, PlanStep, STEP_DOING,
    STEP_DONE, STEP_PENDING, Workspace, WorkspaceState,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::attention::chat_inbox::loop_settled;
use crate::guard::child_loop_def;
use crate::lifecycle::PHASE_INTERRUPTED;
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::RunningLoop;
use crate::loops::Walk;
use crate::loops::fanout::*;
use crate::loops::test_support::*;
use crate::testing::{TestEngine, add_project, git, init_repo, wait_for};

fn step(id: &str, title: &str, status: &str, files: &[&str], deps: &[&str]) -> PlanStep {
    PlanStep {
        id: id.into(),
        title: title.into(),
        status: status.into(),
        files: files.iter().map(|s| (*s).to_owned()).collect(),
        depends_on: deps.iter().map(|s| (*s).to_owned()).collect(),
        ..PlanStep::default()
    }
}

fn excl(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn ready_steps_and_grouping() {
    let mut p = Plan {
        steps: vec![
            step("S1", "parser", STEP_PENDING, &["a.go"], &[]),
            step("S2", "writer", STEP_PENDING, &["b.go"], &[]),
            step("S3", "shares a.go", STEP_PENDING, &["a.go", "c.go"], &[]),
            step("S4", "needs S1", STEP_PENDING, &["d.go"], &["S1"]),
            step("S5", "no files", STEP_PENDING, &[], &[]),
            step("S6", "already done", STEP_DONE, &["e.go"], &[]),
            step("S7", "dep done", STEP_PENDING, &["f.go"], &["S6", "S99"]),
            step("S8", "no files either", STEP_PENDING, &[], &[]),
            step("S9", "excluded", STEP_PENDING, &["g.go"], &[]),
        ],
        ..Plan::default()
    };
    let ready = ready_steps(&p, &excl(&["S9"]));
    assert_eq!(step_ids(&ready), "S1, S2, S3, S5, S7, S8");
    let groups: Vec<String> = group_by_files(&ready).iter().map(|g| step_ids(g)).collect();
    assert_eq!(groups.join(" | "), "S1, S3 | S2 | S5, S8 | S7");
    p.steps[0].status = STEP_DOING.into();
    p.steps[0].workspace = "child-1".into();
    p.steps[2].status = STEP_DOING.into();
    p.steps[2].workspace = "child-1".into();
    let (ids, owned) = in_flight_children(&p);
    assert_eq!(ids, vec!["child-1".to_owned()]);
    assert_eq!(step_ids(&owned["child-1"]), "S1, S3");

    p.steps[1].workspace = "child-fail".into();
    let ready = ready_steps(&p, &excl(&["child-fail"]));
    assert!(
        !step_ids(&ready).contains("S2"),
        "a failed child's step stays excluded across remaps"
    );

    let mut writer = step("S3", "writer", STEP_PENDING, &["w.go"], &[]);
    writer.workspace = "child-fail".into();
    let inserted = Plan {
        steps: vec![
            step("S1", "new first", STEP_PENDING, &["z.go"], &[]),
            step("S2", "unrelated", STEP_PENDING, &["b.go"], &[]),
            writer,
        ],
        ..Plan::default()
    };
    assert_eq!(
        step_ids(&ready_steps(&inserted, &excl(&["child-fail"]))),
        "S1, S2",
        "a stale step key must not suppress a new unrelated S2"
    );
}

#[test]
fn child_incomplete_states() {
    let ws = |state: WorkspaceState, phase: &str| Workspace {
        state,
        phase: phase.into(),
        ..Workspace::default()
    };
    assert!(child_incomplete(&ws(WorkspaceState::Failed, "")));
    assert!(child_incomplete(&ws(
        WorkspaceState::Paused,
        "loop refs missing"
    )));
    assert!(child_incomplete(&ws(
        WorkspaceState::Paused,
        "phase missing"
    )));
    assert!(!child_incomplete(&ws(WorkspaceState::Paused, "idle")));
    assert!(!child_incomplete(&ws(WorkspaceState::PrOpen, "")));
}

fn two_step_plan(body: &str, s1: &str, s2: &str) -> Plan {
    Plan {
        summary: "Importer".into(),
        body: body.into(),
        approved: true,
        source: "human".into(),
        steps: vec![
            step("S1", s1, STEP_PENDING, &["parser.go"], &[]),
            step("S2", s2, STEP_PENDING, &["writer.go"], &[]),
        ],
        ..Plan::default()
    }
}

/// Queues a parent row with `plan` and dispatches it (Go's pre-seeded plan
/// tests).
async fn dispatch_with_plan(
    t: &TestEngine,
    proj: &str,
    goal: &str,
    loop_id: &str,
    plan: Plan,
) -> String {
    let (mut rec, acquired) = t
        .engine
        .queue_start_workspace(
            proj,
            goal,
            "",
            "",
            &AutonomyLevel::default(),
            loop_id,
            "recur.policies.aggressive",
            None,
        )
        .await
        .expect("queue");
    assert!(acquired);
    rec.definition_of_done = plan.text();
    rec.plan = Some(plan);
    let id = rec.id.clone();
    t.engine.dispatch(rec).await.expect("dispatch");
    id
}

async fn wait_parent(t: &TestEngine, id: &str, secs: u64) -> Workspace {
    wait_for(Duration::from_secs(secs), "parent to settle", || {
        t.store.workspace(id).is_some_and(|l| {
            matches!(
                l.state,
                WorkspaceState::PrOpen | WorkspaceState::Failed | WorkspaceState::Stopped
            )
        })
    })
    .await;
    t.store.workspace(id).unwrap()
}

fn children(t: &TestEngine, parent: &str) -> Vec<Workspace> {
    t.store
        .workspaces()
        .into_iter()
        .filter(|w| w.parent_id == parent)
        .collect()
}

fn fan_engine(runner: &Arc<FanOutRunner>) -> TestEngine {
    let t = TestEngine::new(Some(runner.provider()));
    t.engine.set_fan_out_poll(Duration::from_millis(50));
    t.install_shipped_loop_pack();
    t
}

fn wave_cards(t: &TestEngine, id: &str) -> Vec<Value> {
    t.store
        .feed(id)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.card == FEED_CARD_FAN_OUT)
        .map(|f| serde_json::from_str(&f.data).expect("wave card json"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fan_out_runs_independent_steps_in_child_workspaces() {
    let repo = init_repo();
    let runner = Arc::new(FanOutRunner::default());
    let t = fan_engine(&runner);
    let proj = add_project(&t.store, &repo.path);
    let id = dispatch_with_plan(
        &t,
        &proj,
        "Build the importer",
        DEFAULT_LOOP_ID,
        two_step_plan(
            "Parser and writer are independent.",
            "Write the parser",
            "Write the writer",
        ),
    )
    .await;
    let parent = wait_parent(&t, &id, 30).await;
    assert_eq!(
        parent.state,
        WorkspaceState::PrOpen,
        "phase {}",
        parent.phase
    );
    let kids = children(&t, &id);
    assert_eq!(kids.len(), 2);
    for c in &kids {
        assert!(
            c.state == WorkspaceState::Merged && c.archived,
            "{} {:?}",
            c.id,
            c.state
        );
        assert!(
            c.parent_step_ids == ["S1"] || c.parent_step_ids == ["S2"],
            "{:?}",
            c.parent_step_ids
        );
        assert_eq!(c.base_branch, parent.branch);
        assert_eq!(c.loop_id, STEP_LOOP_ID);
    }
    let plan = parent.plan.clone().unwrap();
    assert_eq!(plan.steps.len(), 2);
    for s in &plan.steps {
        assert_eq!(s.status, STEP_DONE, "{} note {}", s.id, s.note);
        assert!(
            s.note.contains("no deviations") && !s.workspace.is_empty(),
            "{s:?}"
        );
    }
    let feed = t.store.feed(&id).unwrap_or_default();
    for f in &feed {
        if f.card != FEED_CARD_FAN_OUT {
            assert!(!f.text.contains("child workspace <code>"), "{}", f.text);
        }
    }
    let cards = wave_cards(&t, &id);
    assert_eq!(cards.len(), 1, "one card per wave, re-emitted under its id");
    let w = &cards[0];
    assert_eq!(w["wave"], 1);
    assert_eq!(w["children"].as_array().unwrap().len(), 2);
    assert_eq!(w["done"], true);
    for c in w["children"].as_array().unwrap() {
        assert_eq!(c["state"], "merged");
        assert_eq!(c["steps"].as_array().unwrap().len(), 1);
    }
    let txt = std::fs::read_dir(&parent.worktree_path)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".txt")
        })
        .count();
    assert_eq!(txt, 2, "both children's files merged into the parent");
    {
        let s = runner.state.lock();
        assert!(
            !s.coder_work.contains(&parent.worktree_path),
            "the parent never codes"
        );
        assert_eq!(s.coder_work.len(), 2);
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_child_is_not_respawned() {
    let repo = init_repo();
    let runner = Arc::new(FanOutRunner {
        fail_on: "Write the writer".into(),
        ..FanOutRunner::default()
    });
    let t = fan_engine(&runner);
    let proj = add_project(&t.store, &repo.path);
    let (mut rec, acquired) = t
        .engine
        .queue_start_workspace(
            &proj,
            "Build the importer",
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
    let plan = two_step_plan(
        "Parser and writer are independent.",
        "Write the parser",
        "Write the writer",
    );
    rec.definition_of_done = plan.text();
    rec.plan = Some(plan);
    *runner.parent_id.lock() = rec.id.clone();
    let id = rec.id.clone();
    t.engine.dispatch(rec).await.unwrap();
    let parent = wait_parent(&t, &id, 30).await;
    assert_eq!(
        parent.state,
        WorkspaceState::PrOpen,
        "phase {}",
        parent.phase
    );
    let writer_children = children(&t, &id)
        .iter()
        .filter(|w| w.parent_step_ids == ["S2"])
        .count();
    {
        let s = runner.state.lock();
        let parent_turns = s
            .coder_work
            .iter()
            .filter(|w| **w == parent.worktree_path)
            .count();
        assert_eq!((writer_children, s.failed), (1, 1));
        assert_eq!(parent_turns, 1, "one fallback turn for the failed step");
    }
    for st in parent.plan.unwrap().steps {
        assert_eq!(st.status, STEP_DONE, "{}", st.id);
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fast_child_unblocks_dependent_while_sibling_runs() {
    let repo = init_repo();
    let runner = Arc::new(FanOutRunner {
        slow_on: "Write the slow writer".into(),
        // The writer runs until the dependent starts: a wave barrier would hold that start
        // behind the writer, which then ends at the cap, after it.
        slow_delay: Duration::from_secs(20),
        slow_until: Some("Wire the parser"),
        mark_on: vec!["Write the slow writer", "Wire the parser"],
        ..FanOutRunner::default()
    });
    let t = fan_engine(&runner);
    let proj = add_project(&t.store, &repo.path);
    let (mut rec, acquired) = t
        .engine
        .queue_start_workspace(
            &proj,
            "Build the importer",
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
    let plan = Plan {
        summary: "Importer".into(),
        body: "Parser first, then wiring; the writer is independent.".into(),
        approved: true,
        source: "human".into(),
        steps: vec![
            step("S1", "Write the parser", STEP_PENDING, &["parser.go"], &[]),
            step(
                "S2",
                "Write the slow writer",
                STEP_PENDING,
                &["writer.go"],
                &[],
            ),
            step("S3", "Wire the parser", STEP_PENDING, &["main.go"], &["S1"]),
        ],
        ..Plan::default()
    };
    rec.definition_of_done = plan.text();
    rec.plan = Some(plan);
    *runner.parent_id.lock() = rec.id.clone();
    let id = rec.id.clone();
    t.engine.dispatch(rec).await.unwrap();
    let parent = wait_parent(&t, &id, 40).await;
    assert_eq!(
        parent.state,
        WorkspaceState::PrOpen,
        "phase {}",
        parent.phase
    );
    let (s3_start, s2_end) = {
        let s = runner.state.lock();
        (
            s.started.get("Wire the parser").copied(),
            s.finished.get("Write the slow writer").copied(),
        )
    };
    let (s3_start, s2_end) = (s3_start.expect("S3 start"), s2_end.expect("S2 end"));
    assert!(s3_start < s2_end, "the wave barrier is back");
    assert_eq!(parent.fan_out_waves, 2);
    for w in children(&t, &id) {
        if w.parent_step_ids != ["S3"] {
            continue;
        }
        let fo = w.fan_out.clone().expect("fan-out record");
        assert_eq!((fo.wave, fo.after.clone()), (2, vec!["S1".to_owned()]));
        assert_eq!(fo.outcome, "merged");
    }
    for s in parent.plan.unwrap().steps {
        assert_eq!(s.status, STEP_DONE, "{}", s.id);
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planner_filed_plan_fans_out() {
    let repo = init_repo();
    let runner = Arc::new(FanOutRunner {
        plans: true,
        ..FanOutRunner::default()
    });
    let t = fan_engine(&runner);
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace_with(
            &proj,
            "Build the importer",
            "",
            AutonomyLevel::default(),
            DEFAULT_LOOP_ID,
            "recur.policies.aggressive",
        )
        .await
        .unwrap();
    let parent = wait_parent(&t, &id, 30).await;
    assert_eq!(
        parent.state,
        WorkspaceState::PrOpen,
        "phase {}",
        parent.phase
    );
    let plan = parent.plan.clone().expect("plan");
    assert_eq!(
        (plan.source.as_str(), plan.steps.len()),
        ("loop-planner", 2)
    );
    let mut plan_cards = 0;
    for f in t.store.feed(&id).unwrap_or_default() {
        assert!(!f.text.contains("definition-of-done:"), "{}", f.text);
        if f.card == FEED_CARD_PLAN {
            plan_cards += 1;
            let d: Value = serde_json::from_str(&f.data).unwrap();
            assert_eq!(d["steps"].as_array().unwrap().len(), 2);
            assert_eq!(d["steps"][0]["files"][0], "parser.go");
        }
    }
    assert_eq!(plan_cards, 1);
    assert_eq!(children(&t, &id).len(), 2);
    {
        let s = runner.state.lock();
        assert_eq!(s.planned, 1);
        assert!(!s.coder_work.contains(&parent.worktree_path));
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn docs_loop_fans_out_with_writers() {
    let repo = init_repo();
    let runner = Arc::new(FanOutRunner::default());
    let t = fan_engine(&runner);
    let proj = add_project(&t.store, &repo.path);
    let plan = Plan {
        summary: "Docs".into(),
        body: "Two independent pages.".into(),
        approved: true,
        source: "human".into(),
        steps: vec![
            step(
                "S1",
                "Write install.md",
                STEP_PENDING,
                &["docs/install.md"],
                &[],
            ),
            step(
                "S2",
                "Write usage.md",
                STEP_PENDING,
                &["docs/usage.md"],
                &[],
            ),
        ],
        ..Plan::default()
    };
    let id = dispatch_with_plan(&t, &proj, "Write the docs", DOCS_LOOP_ID, plan).await;
    wait_for(Duration::from_secs(30), "docs parent to settle", || {
        t.store.workspace(&id).is_some_and(|l| {
            loop_settled(&l.state, &l.phase)
                || matches!(l.state, WorkspaceState::Failed | WorkspaceState::Stopped)
        })
    })
    .await;
    let parent = t.store.workspace(&id).unwrap();
    assert!(
        !matches!(
            parent.state,
            WorkspaceState::PrOpen | WorkspaceState::Failed | WorkspaceState::Stopped
        ),
        "{:?} {}",
        parent.state,
        parent.phase
    );
    let kids = children(&t, &id);
    assert_eq!(kids.len(), 2);
    for w in kids {
        assert_eq!(
            (w.parent_phase_id.as_str(), w.loop_id.as_str()),
            ("converge", STEP_LOOP_ID)
        );
        let codes: HashSet<String> = t.store.agents(&w.id).into_iter().map(|a| a.code).collect();
        assert!(codes.contains("WR") && codes.contains("ED"), "{codes:?}");
        assert!(!codes.contains("CO") && !codes.contains("RV"), "{codes:?}");
    }
    for s in parent.plan.unwrap().steps {
        assert_eq!(s.status, STEP_DONE, "{}", s.id);
    }
    t.close().await;
}

#[test]
fn child_loop_def_mirrors_parent_converge() {
    let map = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let parent = LoopDef {
        id: "acme.loops.custom".into(),
        name: "Custom".into(),
        entry: "plan".into(),
        roles: map(&[("conductor", "acme.roles.boss")]),
        phases: vec![
            LoopPhase {
                id: "plan".into(),
                uses: VERB_PLAN.into(),
                next: "work".into(),
                ..LoopPhase::default()
            },
            LoopPhase {
                id: "work".into(),
                uses: VERB_CONVERGE.into(),
                next: "pr".into(),
                gate: "plan_approval".into(),
                start: "review".into(),
                max_rounds: Some(2),
                on_max_rounds: "proceed".into(),
                timeout: "10m".into(),
                on_fail: "skip".into(),
                roles: map(&[
                    ("implementer", "acme.roles.dev"),
                    ("reviewer", "acme.roles.qa"),
                ]),
                prompts: map(&[("review", "/acme-review"), ("fix", "/acme-fix")]),
                ..LoopPhase::default()
            },
            LoopPhase {
                id: "pr".into(),
                uses: VERB_ENSURE_PR.into(),
                ..LoopPhase::default()
            },
        ],
        ..LoopDef::default()
    };
    let ph = parent.phase_by_id("work").unwrap().clone();
    let child = child_loop_def(&parent, &ph);
    assert_eq!(
        (child.id.as_str(), child.entry.as_str(), child.phases.len()),
        (STEP_LOOP_ID, "converge", 2)
    );
    let conv = &child.phases[0];
    assert!(
        conv.uses == VERB_CONVERGE
            && conv.next == "done"
            && conv.start.is_empty()
            && conv.gate.is_empty()
    );
    assert_eq!(conv.roles["implementer"], "acme.roles.dev");
    assert_eq!(conv.roles["reviewer"], "acme.roles.qa");
    assert_eq!(conv.prompts["review"], "/acme-review");
    assert_eq!(conv.prompts["fix"], "/acme-fix");
    assert_eq!(conv.max_rounds, Some(2));
    assert_eq!(
        (
            conv.on_max_rounds.as_str(),
            conv.timeout.as_str(),
            conv.on_fail.as_str()
        ),
        ("proceed", "10m", "skip")
    );
    assert_eq!(child.phases[1].uses, VERB_ARTIFACT);
    assert_eq!(child.roles["conductor"], "acme.roles.boss");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn board_bootstrap_then_fan_out_then_tail() {
    let repo = init_repo();
    let runner = Arc::new(FanOutRunner::default());
    let t = fan_engine(&runner);
    let proj = add_project(&t.store, &repo.path);
    let plan = Plan {
        summary: "csv2json".into(),
        body: "Bootstrap, then parser and writer, then verify.".into(),
        approved: true,
        source: "human".into(),
        steps: vec![
            step("S1", "Create go.mod", STEP_PENDING, &["go.mod"], &[]),
            step(
                "S2",
                "Write parser.go",
                STEP_PENDING,
                &["parser.go"],
                &["S1"],
            ),
            step(
                "S3",
                "Write writer.go",
                STEP_PENDING,
                &["writer.go"],
                &["S1"],
            ),
            step(
                "S4",
                "Verify end to end",
                STEP_PENDING,
                &["parser.go", "writer.go"],
                &["S2", "S3"],
            ),
        ],
        ..Plan::default()
    };
    let id = dispatch_with_plan(&t, &proj, "CSV to JSON", DEFAULT_LOOP_ID, plan).await;
    let parent = wait_parent(&t, &id, 30).await;
    assert_eq!(
        parent.state,
        WorkspaceState::PrOpen,
        "phase {}",
        parent.phase
    );
    let kids = children(&t, &id);
    assert_eq!(kids.len(), 4);
    for w in &kids {
        assert_eq!(w.parent_step_ids.len(), 1, "{:?}", w.parent_step_ids);
    }
    for s in parent.plan.clone().unwrap().steps {
        assert_eq!(s.status, STEP_DONE, "{} {}", s.id, s.note);
    }
    {
        let s = runner.state.lock();
        assert_eq!(
            s.coder_work
                .iter()
                .filter(|w| **w == parent.worktree_path)
                .count(),
            0
        );
        assert_eq!(s.coder_work.len(), 4);
    }
    let waves: HashMap<i64, usize> = wave_cards(&t, &id)
        .iter()
        .map(|w| {
            (
                w["wave"].as_i64().unwrap(),
                w["children"].as_array().unwrap().len(),
            )
        })
        .collect();
    assert_eq!(waves, HashMap::from([(1, 1), (2, 2), (3, 1)]));
    assert_eq!(parent.fan_out_waves, 3);
    t.close().await;
}

#[tokio::test]
async fn fan_out_skips_children_but_not_single_steps() {
    let t = TestEngine::new(None);
    let mut l = Walk::bare(&t.engine, "x");
    l.plan_structured = true;
    l.parent_id = "parent-1".into();
    assert!(!l.fan_out_eligible(), "a child never fans out");
    l.parent_id.clear();
    assert!(l.fan_out_eligible());
    let p = Plan {
        steps: vec![step("S1", "", STEP_PENDING, &["a"], &[])],
        ..Plan::default()
    };
    let g = group_by_files(&ready_steps(&p, &HashSet::new()));
    assert!(g.len() == 1 && g[0].len() == 1);
}

#[tokio::test]
async fn mark_conflicts_resolved_uses_current_workspace_ownership() {
    let t = TestEngine::new(None);
    let (parent_id, child_id) = ("WS-conflict-parent", "WS-conflict-child");
    let mut s2 = step("S2", "conflicted work", STEP_DOING, &["a.go"], &[]);
    s2.workspace = child_id.into();
    t.store
        .add_workspace(Workspace {
            id: parent_id.into(),
            goal: "parent".into(),
            state: WorkspaceState::Coding,
            plan: Some(Plan {
                steps: vec![
                    step("S1", "inserted bootstrap", STEP_PENDING, &["boot.go"], &[]),
                    s2,
                ],
                ..Plan::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_workspace(Workspace {
            id: child_id.into(),
            parent_id: parent_id.into(),
            goal: "conflicted work".into(),
            state: WorkspaceState::PrOpen,
            parent_step_ids: vec!["S1".into()],
            fan_out: Some(ChildFanOut {
                wave: 1,
                outcome: "conflict: a.go".into(),
                note: "landed dirty".into(),
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    Walk::bare(&t.engine, parent_id)
        .mark_conflicts_resolved()
        .await;
    let got = t.store.workspace(parent_id).unwrap().plan.unwrap();
    assert_eq!(got.steps[0].status, STEP_PENDING, "inserted step untouched");
    assert_eq!(got.steps[1].status, STEP_DONE, "the conflicted step closes");
    t.close().await;
}

fn manager_for(t: &TestEngine, repo: &str) -> recur_git::Manager {
    recur_git::Manager::new(repo, t.engine.worktree_root())
}

#[tokio::test]
async fn preserve_child_work_failure_keeps_worktree() {
    let repo = init_repo();
    let hook = Path::new(&repo.path).join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let dirty = Path::new(&repo.path).join("wip.txt");
    std::fs::write(&dirty, "unfinished\n").unwrap();
    let t = TestEngine::new(None);
    let (parent_id, child_id) = ("WS-preserve-parent", "WS-preserve-child");
    let mut s1 = step("S1", "work", STEP_DOING, &[], &[]);
    s1.workspace = child_id.into();
    let plan = Plan {
        steps: vec![s1],
        ..Plan::default()
    };
    t.store
        .add_workspace(Workspace {
            id: parent_id.into(),
            goal: "parent".into(),
            state: WorkspaceState::Coding,
            plan: Some(plan.clone()),
            worktree_path: repo.path.clone(),
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_workspace(Workspace {
            id: child_id.into(),
            parent_id: parent_id.into(),
            goal: "work".into(),
            state: WorkspaceState::Failed,
            worktree_path: repo.path.clone(),
            branch: "recur/child".into(),
            ..Workspace::default()
        })
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut l = Walk::bare(&t.engine, parent_id);
    l.git = Some(recur_git::Manager::new(
        &repo.path,
        &tmp.path().to_string_lossy(),
    ));
    l.worktree = repo.path.clone();
    let (conflicts, merged, _, failed) = l.merge_child(child_id, &plan.steps).await;
    assert!(conflicts.is_empty() && !merged && failed);
    let child = t.store.workspace(child_id).expect("child kept");
    assert!(!child.archived && !child.worktree_path.is_empty());
    assert!(dirty.exists(), "the dirty file survives");
    assert_eq!(
        t.store.workspace(parent_id).unwrap().plan.unwrap().steps[0].status,
        STEP_PENDING
    );
    t.close().await;
}

async fn commit_file(m: &recur_git::Manager, dir: &str, rel: &str, content: &str, msg: &str) {
    let p = Path::new(dir).join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    m.commit(dir, msg, "test", "test@example.com")
        .await
        .unwrap_or_else(|e| panic!("commit {msg}: {e}"));
}

/// Parent + two children branched off it; S2's README merge left in
/// progress over a parent edit.
async fn conflicted_board(
    t: &TestEngine,
    repo: &str,
    prefix: &str,
) -> (recur_git::Manager, [recur_git::Worktree; 3]) {
    let m = manager_for(t, repo);
    let (p, _) = m
        .add(
            &format!("WS-{prefix}-parent"),
            &format!("{prefix}-parent"),
            "main",
        )
        .await
        .unwrap();
    let (s1, _) = m
        .add(
            &format!("WS-{prefix}-s1"),
            &format!("{prefix}-s1"),
            &p.branch,
        )
        .await
        .unwrap();
    let (s2, _) = m
        .add(
            &format!("WS-{prefix}-s2"),
            &format!("{prefix}-s2"),
            &p.branch,
        )
        .await
        .unwrap();
    commit_file(&m, &s1.path, "s1.txt", "from s1\n", "s1 work").await;
    commit_file(&m, &s2.path, "README.md", "s2 says hi\n", "s2 work").await;
    commit_file(&m, &p.path, "README.md", "parent says hi\n", "parent edit").await;
    let conflicts = m
        .merge_into(&p.path, &s2.branch, "Merge S2", "test", "test@example.com")
        .await
        .unwrap();
    assert!(!conflicts.is_empty(), "README conflict left in progress");
    (m, [p, s1, s2])
}

#[tokio::test]
async fn fan_out_resume_prefers_in_progress_sibling_merge() {
    let repo = init_repo();
    let t = TestEngine::new(None);
    let (m, [p, s1, s2]) = conflicted_board(&t, &repo.path, "resume").await;
    let (parent_id, s1_id, s2_id) = ("WS-resume-parent", "WS-resume-s1", "WS-resume-s2");
    let mut st1 = step("S1", "earlier sibling", STEP_DOING, &["s1.txt"], &[]);
    st1.workspace = s1_id.into();
    let mut st2 = step("S2", "conflicted sibling", STEP_DOING, &["README.md"], &[]);
    st2.workspace = s2_id.into();
    t.store
        .add_workspace(Workspace {
            id: parent_id.into(),
            goal: "parent".into(),
            state: WorkspaceState::Coding,
            plan: Some(Plan {
                steps: vec![st1, st2],
                ..Plan::default()
            }),
            worktree_path: p.path.clone(),
            branch: p.branch.clone(),
            ..Workspace::default()
        })
        .unwrap();
    for (id, goal, wt, ids, outcome) in [
        (s1_id, "earlier sibling", &s1, "S1", ""),
        (
            s2_id,
            "conflicted sibling",
            &s2,
            "S2",
            "conflict: README.md",
        ),
    ] {
        t.store
            .add_workspace(Workspace {
                id: id.into(),
                parent_id: parent_id.into(),
                goal: goal.into(),
                state: WorkspaceState::PrOpen,
                branch: wt.branch.clone(),
                worktree_path: wt.path.clone(),
                parent_step_ids: vec![ids.into()],
                fan_out: Some(ChildFanOut {
                    wave: 1,
                    outcome: outcome.into(),
                    ..ChildFanOut::default()
                }),
                ..Workspace::default()
            })
            .unwrap();
    }
    let mut l = Walk::bare(&t.engine, parent_id);
    l.git = Some(m);
    l.worktree = p.path.clone();
    l.plan_structured = true;
    let res = l
        .fan_out_steps(&CancellationToken::new(), &mut HashSet::new())
        .await;
    assert!(!res.conflicts.is_empty(), "the in-progress S2 merge");
    let s1w = t.store.workspace(s1_id).unwrap();
    assert!(
        s1w.fan_out
            .is_none_or(|f| !f.outcome.starts_with("conflict"))
    );
    let s2w = t.store.workspace(s2_id).unwrap();
    assert!(
        s2w.fan_out
            .is_some_and(|f| f.outcome.starts_with("conflict"))
    );
    assert_eq!(
        t.store.workspace(parent_id).unwrap().plan.unwrap().steps[0].status,
        STEP_DOING
    );
    t.close().await;
}

#[tokio::test]
async fn merge_child_does_not_claim_sibling_merge() {
    let repo = init_repo();
    let t = TestEngine::new(None);
    let (m, [p, s1, s2]) = conflicted_board(&t, &repo.path, "claim").await;
    let (parent_id, s1_id, s2_id) = ("WS-claim-parent", "WS-claim-s1", "WS-claim-s2");
    let mut s1_step = step("S1", "earlier sibling", STEP_DOING, &[], &[]);
    s1_step.workspace = s1_id.into();
    let mut s2_step = step("S2", "conflicted", STEP_DOING, &[], &[]);
    s2_step.workspace = s2_id.into();
    t.store
        .add_workspace(Workspace {
            id: parent_id.into(),
            goal: "parent".into(),
            state: WorkspaceState::Coding,
            worktree_path: p.path.clone(),
            plan: Some(Plan {
                steps: vec![s1_step.clone(), s2_step],
                ..Plan::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_workspace(Workspace {
            id: s1_id.into(),
            parent_id: parent_id.into(),
            goal: "earlier sibling".into(),
            state: WorkspaceState::PrOpen,
            branch: s1.branch.clone(),
            worktree_path: s1.path.clone(),
            fan_out: Some(ChildFanOut {
                wave: 1,
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_workspace(Workspace {
            id: s2_id.into(),
            parent_id: parent_id.into(),
            goal: "conflicted".into(),
            state: WorkspaceState::PrOpen,
            branch: s2.branch.clone(),
            fan_out: Some(ChildFanOut {
                wave: 1,
                outcome: "conflict: README.md".into(),
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    let mut l = Walk::bare(&t.engine, parent_id);
    l.git = Some(m);
    l.worktree = p.path.clone();
    let (conflicts, merged, _, failed) = l.merge_child(s1_id, &[s1_step]).await;
    assert!(
        conflicts.is_empty() && !merged && !failed,
        "skip while a sibling merge is in progress"
    );
    let s1w = t.store.workspace(s1_id).unwrap();
    assert!(
        s1w.fan_out
            .is_none_or(|f| !f.outcome.starts_with("conflict"))
    );
    t.close().await;
}

fn docs_parent(t: &TestEngine, proj: &str, id: &str, steps: Vec<PlanStep>) -> (Workspace, Walk) {
    let plan = Plan {
        summary: "Docs".into(),
        approved: true,
        source: "human".into(),
        steps,
        ..Plan::default()
    };
    let ws = Workspace {
        id: id.into(),
        project_id: proj.into(),
        goal: "Write the docs".into(),
        state: WorkspaceState::Coding,
        plan: Some(plan),
        branch: "main".into(),
        loop_id: DOCS_LOOP_ID.into(),
        policy_id: "recur.policies.aggressive".into(),
        autonomy: AutonomyLevel::Aggressive,
        ..Workspace::default()
    };
    t.store.add_workspace(ws.clone()).unwrap();
    let mut w = Walk::bare(&t.engine, id);
    w.project_id = proj.into();
    w.autonomy = AutonomyLevel::Aggressive;
    w.policy_id = "recur.policies.aggressive".into();
    w.h.set_branch("main");
    w.set_current_phase(&LoopPhase {
        id: "converge".into(),
        uses: VERB_CONVERGE.into(),
        ..LoopPhase::default()
    });
    (ws, w)
}

fn install_step(id: &str, title: &str, file: &str) -> PlanStep {
    step(id, title, STEP_PENDING, &[file], &[])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_step_child_drain_during_init_keeps_parent_metadata() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[])));
    t.install_shipped_loop_pack();
    let proj = add_project(&t.store, &repo.path);
    let parent_id = "WS-drain-parent";
    let (parent_ws, parent) = docs_parent(
        &t,
        &proj,
        parent_id,
        vec![install_step("S1", "Write install.md", "docs/install.md")],
    );
    let drains: Arc<parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>> = Arc::default();
    {
        let (eng, drains) = (Arc::downgrade(&t.engine), Arc::clone(&drains));
        let rt = tokio::runtime::Handle::current();
        t.engine.set_child_queue_hook(move || {
            if let Some(e) = eng.upgrade() {
                drains
                    .lock()
                    .push(rt.spawn(async move { e.drain_queued().await }));
            }
            std::thread::sleep(Duration::from_millis(40));
        });
    }
    let steps = parent_ws.plan.clone().unwrap().steps;
    let id = parent
        .start_step_child(&parent_ws, &steps, 1, &[])
        .await
        .expect("start_step_child");
    let handles: Vec<_> = std::mem::take(&mut *drains.lock());
    for h in handles {
        h.await.unwrap();
    }
    let child = t.store.workspace(&id).expect("child");
    assert_eq!(child.parent_id, parent_id);
    assert_eq!(child.parent_phase_id, "converge");
    assert!(child.plan.as_ref().is_some_and(|p| !p.steps.is_empty()));
    let gp = t.store.workspace(parent_id).unwrap().plan.unwrap();
    assert_eq!(
        (gp.steps[0].status.as_str(), gp.steps[0].workspace.as_str()),
        (STEP_DOING, id.as_str())
    );
    let lp = t
        .engine
        .running_loop_by_id(&id)
        .expect("child walk running");
    let guard = Arc::clone(&lp.lock().guard);
    let ph = guard.loop_def.phase_by_id("converge").expect("converge");
    assert_eq!(
        ph.roles.get("implementer").map(String::as_str),
        Some("recur.roles.writer")
    );
    assert_eq!(
        ph.roles.get("reviewer").map(String::as_str),
        Some("recur.roles.editor")
    );
    gate.cancel();
    t.close().await;
}

fn take_all_slots(t: &TestEngine) -> usize {
    let mut n = 0;
    while t.engine.governor().try_acquire_loop() {
        n += 1;
    }
    n
}

#[tokio::test]
async fn stop_in_flight_children_stops_queued_child() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[]).provider()));
    t.install_shipped_loop_pack();
    let proj = add_project(&t.store, &repo.path);
    let mut slots = take_all_slots(&t);
    let parent_id = "WS-stop-queued-parent";
    let (parent_ws, parent) = docs_parent(
        &t,
        &proj,
        parent_id,
        vec![install_step("S1", "Write install.md", "docs/install.md")],
    );
    let steps = parent_ws.plan.clone().unwrap().steps;
    let id = parent
        .start_step_child(&parent_ws, &steps, 1, &[])
        .await
        .unwrap();
    assert_eq!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::Queued
    );
    assert!(t.engine.running_loop_by_id(&id).is_none());
    parent.stop_in_flight_children().await;
    assert_eq!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::Stopped
    );
    if slots > 0 {
        t.engine.governor().release_loop();
        slots -= 1;
    }
    t.engine.drain_queued().await;
    assert!(
        t.engine.running_loop_by_id(&id).is_none(),
        "a stopped child never drains"
    );
    assert_eq!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::Stopped
    );
    for _ in 0..slots {
        t.engine.governor().release_loop();
    }
    t.close().await;
}

#[tokio::test]
async fn stop_children_skipped_during_shutdown() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[]).provider()));
    t.install_shipped_loop_pack();
    let proj = add_project(&t.store, &repo.path);
    let slots = take_all_slots(&t);
    let (parent_ws, parent) = docs_parent(
        &t,
        &proj,
        "WS-shutdown-parent",
        vec![install_step("S1", "Write install.md", "docs/install.md")],
    );
    let steps = parent_ws.plan.clone().unwrap().steps;
    let id = parent
        .start_step_child(&parent_ws, &steps, 1, &[])
        .await
        .unwrap();
    *t.engine.teardown.lock() = true;
    parent.stop_in_flight_children().await;
    assert_eq!(
        t.store.workspace(&id).unwrap().state,
        WorkspaceState::Queued,
        "untouched by a shutdown"
    );
    for _ in 0..slots {
        t.engine.governor().release_loop();
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn board_resume_redispatches_interrupted_children() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[]).provider()));
    t.install_shipped_loop_pack();
    let proj = add_project(&t.store, &repo.path);
    let mut slots = take_all_slots(&t);
    let (parent_ws, parent) = docs_parent(
        &t,
        &proj,
        "WS-resume-parent",
        vec![
            install_step("S1", "Write install.md", "docs/install.md"),
            install_step("S2", "Write usage.md", "docs/usage.md"),
        ],
    );
    let steps = parent_ws.plan.clone().unwrap().steps;
    let interrupted = parent
        .start_step_child(&parent_ws, &steps[..1], 1, &[])
        .await
        .unwrap();
    let paused = parent
        .start_step_child(&parent_ws, &steps[1..], 1, &[])
        .await
        .unwrap();
    t.store.update_workspace(&interrupted, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = PHASE_INTERRUPTED.into();
    });
    t.store.update_workspace(&paused, |w| {
        w.state = WorkspaceState::Paused;
        w.phase = "paused".into();
    });
    if slots > 0 {
        t.engine.governor().release_loop();
        slots -= 1;
    }
    parent
        .resume_interrupted_children(&[interrupted.clone(), paused.clone()])
        .await;
    let c = t.store.workspace(&interrupted).unwrap();
    assert!(
        !(c.state == WorkspaceState::Paused && c.phase == PHASE_INTERRUPTED),
        "re-dispatched"
    );
    let c = t.store.workspace(&paused).unwrap();
    assert_eq!(
        (c.state, c.phase.as_str()),
        (WorkspaceState::Paused, "paused")
    );
    t.engine.stop(&interrupted).await;
    for _ in 0..slots {
        t.engine.governor().release_loop();
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_board_stops_sibling_when_conflict_resolver_fails() {
    let repo = init_repo();
    let t = TestEngine::new(Some(fail_agent_runner()));
    let (m, [p, s1, s2]) = conflicted_board(&t, &repo.path, "fail").await;
    let (parent_id, s1_id, s2_id) = ("WS-fail-parent", "WS-fail-s1", "WS-fail-s2");
    let mut st1 = step("S1", "slow sibling", STEP_DOING, &["s1.txt"], &[]);
    st1.workspace = s1_id.into();
    let mut st2 = step("S2", "conflicted sibling", STEP_DOING, &["README.md"], &[]);
    st2.workspace = s2_id.into();
    t.store
        .add_workspace(Workspace {
            id: parent_id.into(),
            goal: "parent".into(),
            state: WorkspaceState::Coding,
            plan: Some(Plan {
                steps: vec![st1, st2],
                ..Plan::default()
            }),
            worktree_path: p.path.clone(),
            branch: p.branch.clone(),
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_workspace(Workspace {
            id: s1_id.into(),
            parent_id: parent_id.into(),
            goal: "slow sibling".into(),
            state: WorkspaceState::Coding,
            branch: s1.branch.clone(),
            worktree_path: s1.path.clone(),
            parent_step_ids: vec!["S1".into()],
            fan_out: Some(ChildFanOut {
                wave: 1,
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .add_workspace(Workspace {
            id: s2_id.into(),
            parent_id: parent_id.into(),
            goal: "conflicted sibling".into(),
            state: WorkspaceState::PrOpen,
            branch: s2.branch.clone(),
            worktree_path: s2.path.clone(),
            parent_step_ids: vec!["S2".into()],
            fan_out: Some(ChildFanOut {
                wave: 1,
                outcome: "conflict: README.md".into(),
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    let child = LoopHandle::new(s1_id, "", s1_id, LoopState::default());
    let child_cancel = CancellationToken::new();
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&child),
        cancel: child_cancel.clone(),
    }));
    let mut lp = Walk::bare(&t.engine, parent_id);
    lp.git = Some(m);
    lp.worktree = p.path.clone();
    lp.plan_structured = true;
    let out = lp.run_board(&CancellationToken::new(), 1).await;
    assert!(out.is_err(), "the conflict-resolve turn failed: {out:?}");
    assert!(child.is_stopped(), "the slow sibling walk is stopped");
    assert!(
        child_cancel.is_cancelled(),
        "the slow sibling dispatch is cancelled"
    );
    t.engine.remove_loop(&child);
    git(&p.path, &["merge", "--abort"]);
    t.close().await;
}
