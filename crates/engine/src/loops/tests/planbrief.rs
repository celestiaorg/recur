//! Plan brief tests: the planning turn's tool scope, the plan
//! in every Coder and Reviewer system prompt (never duplicated into the fix
//! prompt), the createPlan harvest over narration, and rune-safe truncation.
//! The argv spill case is P1's; the send-back case is E5's.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::display::PLAN_MAX_PERSIST_CHARS;
use recur_agent::{AgentEvent, Provider, ToolStep, TurnRequest};
use recur_agent_tools::names::mcp;
use recur_domain::{AgentRole, AutonomyLevel, WorkspaceState};

use crate::loops::test_support::*;
use crate::roles::{coder_system_prompt, maestro_plan_tools, plan_section};
use crate::testing::{TestEngine, add_project, init_repo, wait_for};
use crate::text::truncate;

#[test]
fn maestro_plan_tools_scope() {
    let got = maestro_plan_tools();
    for banned in ["Edit", "Write", "Bash"] {
        assert!(!got.iter().any(|t| t == banned), "{banned}");
    }
    for want in [
        "Read",
        "Grep",
        "Glob",
        mcp::LIST_SKILLS,
        mcp::ACTIVATE_SKILL,
        mcp::ESCALATE,
    ] {
        assert!(got.iter().any(|t| t == want), "missing {want}");
    }
}

#[test]
fn plan_section_degrades_to_task_only() {
    assert_eq!(plan_section(""), "");
    assert_eq!(plan_section("   \n\t "), "");
    let task_only = coder_system_prompt("do the thing", "", false);
    assert!(!task_only.contains("investigated the repo"));
    assert!(task_only.contains("do the thing"));
    let with_plan = coder_system_prompt("do the thing", "Step 1: touch foo.go", false);
    assert!(with_plan.contains("Step 1: touch foo.go"));
    assert!(with_plan.contains("Follow it on the first coding pass"));
    assert!(with_plan.contains("take precedence"));
    assert_eq!(
        coder_system_prompt("do the thing", "Step 1: touch foo.go", false),
        with_plan
    );
}

/// Plans with `plan_text` (through createPlan when `via_create_plan`) and records Coder turns.
#[derive(Default)]
struct PlanBriefRunner {
    plan_text: String,
    via_create_plan: bool,
    coder_calls: Mutex<Vec<TurnRequest>>,
    review_calls: Mutex<Vec<TurnRequest>>,
    plan_spec: Mutex<Option<TurnRequest>>,
}

impl PlanBriefRunner {
    fn provider(self: &Arc<Self>) -> Arc<dyn Provider> {
        let me = Arc::clone(self);
        AsyncProvider::wrap(move |req, cx| {
            let me = Arc::clone(&me);
            async move {
                let fail = |e: String| recur_agent::ProviderError::failed(e);
                match req.role {
                    AgentRole::Maestro => {
                        *me.plan_spec.lock() = Some(req.clone());
                        if me.via_create_plan {
                            let args = serde_json::json!({"plan": me.plan_text}).to_string();
                            cx.sink.emit(AgentEvent::Step(ToolStep {
                                name: "createPlan".into(),
                                input: args,
                                ..ToolStep::default()
                            }));
                            cx.sink.emit(AgentEvent::PlanProposal {
                                body: me.plan_text.clone(),
                            });
                            cx.sink.emit(AgentEvent::Text {
                                text: "I've written up the plan.".into(),
                                uuid: String::new(),
                                parent: String::new(),
                            });
                            return Ok(success(&req.agent_id, "I've written up the plan."));
                        }
                        cx.sink.emit(AgentEvent::Text {
                            text: me.plan_text.clone(),
                            uuid: String::new(),
                            parent: String::new(),
                        });
                        Ok(success(&req.agent_id, ""))
                    }
                    AgentRole::Coder => {
                        let n = {
                            let mut c = me.coder_calls.lock();
                            c.push(req.clone());
                            c.len()
                        };
                        apply_edits(&req.worktree, &[("work.txt", &"x\n".repeat(n))])
                            .map_err(|e| fail(e.to_string()))?;
                        if !req.mcp_url.is_empty() {
                            close_open_findings_mcp(&req.mcp_url).await.map_err(fail)?;
                        }
                        Ok(success(&req.agent_id, ""))
                    }
                    AgentRole::Reviewer => {
                        let iter = {
                            let mut r = me.review_calls.lock();
                            r.push(req.clone());
                            r.len()
                        };
                        if iter == 1 {
                            post_finding(&req.mcp_url, "work.txt", 1, "medium", "needs a fix")
                                .await
                                .map_err(fail)?;
                        }
                        Ok(success(&req.agent_id, ""))
                    }
                    _ => Ok(success("", "")),
                }
            }
        })
    }
}

const NEEDLE: &str = "Steps:\n1. Touch work.txt and add the marker line";

fn plan_text(tail: &str) -> String {
    format!(
        "Summary: add a marker to work.txt.\n\nRelevant files:\n- work.txt — target file\n\n{NEEDLE}\n\nEdge cases / gotchas: none identified\n\n{tail}"
    )
}

async fn run_to_pr(
    runner: &Arc<PlanBriefRunner>,
) -> (TestEngine, String, crate::testing::TempRepo) {
    let repo = init_repo();
    let t = TestEngine::new(Some(runner.provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(
            &proj,
            "Add a marker to work.txt",
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
    (t, id, repo)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planner_plan_reaches_coder_exactly_once() {
    let runner = Arc::new(PlanBriefRunner {
        plan_text: plan_text("Definition of done: work.txt exists with the marker."),
        ..PlanBriefRunner::default()
    });
    let (t, id, _repo) = run_to_pr(&runner).await;
    let plan_spec = runner
        .plan_spec
        .lock()
        .clone()
        .expect("the planning turn ran");
    for tool in plan_spec.allowed_tools.unwrap_or_default() {
        assert!(
            !matches!(tool.as_str(), "Edit" | "Write" | "Bash"),
            "{tool}"
        );
    }
    let l = t.store.workspace(&id).unwrap();
    assert!(
        l.definition_of_done.contains(NEEDLE),
        "{}",
        l.definition_of_done
    );
    let calls = runner.coder_calls.lock().clone();
    assert!(calls.len() >= 2, "{}", calls.len());
    assert!(calls[0].system_prompt.contains(NEEDLE));
    assert!(calls[1].system_prompt.contains(NEEDLE));
    assert!(
        !calls[1].prompt.contains(NEEDLE),
        "the plan is system-prompt-only"
    );
    assert!(calls[1].prompt.contains("needs a fix"));
    assert_eq!(
        calls[0].system_prompt, calls[1].system_prompt,
        "stable across iterations"
    );
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planner_create_plan_harvested_over_narration() {
    let runner = Arc::new(PlanBriefRunner {
        plan_text: plan_text("Definition of done: work.txt exists with the marker."),
        via_create_plan: true,
        ..PlanBriefRunner::default()
    });
    let (t, id, _repo) = run_to_pr(&runner).await;
    let l = t.store.workspace(&id).unwrap();
    assert!(
        l.definition_of_done.contains(NEEDLE),
        "{}",
        l.definition_of_done
    );
    assert!(!l.definition_of_done.contains("I've written up the plan"));
    let plan = l.plan.clone().expect("createPlan filed as a Plan");
    assert!(plan.body.contains(NEEDLE));
    assert!(plan.steps.is_empty());
    assert_eq!(plan.source, "loop-planner");
    let calls = runner.coder_calls.lock().clone();
    assert!(!calls.is_empty());
    assert!(calls[0].system_prompt.contains(NEEDLE));
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reviewer_receives_maestro_plan() {
    let dod = "Definition of done: work.txt exists with the marker.";
    let runner = Arc::new(PlanBriefRunner {
        plan_text: plan_text(dod),
        ..PlanBriefRunner::default()
    });
    let (t, _id, _repo) = run_to_pr(&runner).await;
    let reviews = runner.review_calls.lock().clone();
    assert!(reviews.len() >= 2);
    for (i, r) in reviews.iter().enumerate() {
        assert!(r.system_prompt.contains(dod), "review {}", i + 1);
    }
    t.close().await;
}

#[test]
fn truncate_rune_safe() {
    let s = format!("{}—{}", "a".repeat(9), "b".repeat(20));
    for n in 8..=13 {
        let got = truncate(&s, n);
        assert!(got.ends_with('…'), "{n}: {got}");
    }
    let long = "plan step — with an em dash and “curly quotes” → arrow. ".repeat(2000);
    let got = truncate(&long, PLAN_MAX_PERSIST_CHARS);
    assert!(got.len() <= PLAN_MAX_PERSIST_CHARS + 3);
}
