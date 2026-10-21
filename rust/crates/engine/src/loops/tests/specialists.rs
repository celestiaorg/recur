//! Loop cases moved here from other Go files: `plan_test.go`
//! `TestPlanApprovalGate`, `user_mcp_test.go`
//! `TestLoopSpecialistsAttachLinear`, `mcp_required_test.go`
//! `TestDeadMCPFailsTheLoopInsteadOfConverging`, and the loop-start cases
//! of `simplify_test.go`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use recur_agent::TurnRequest;
use recur_agent_tools::{Host, PlanSubmission};
use recur_domain::{
    AgentRole, AutonomyLevel, CHAT_MODE_AGENT, CHAT_MODE_REVIEW, LINEAR_MCP_READ_ONLY_URL,
    LINEAR_MCP_WRITE_URL, WorkspaceState,
};

use crate::codes::{MAESTRO_AGENT_CODE, PAIR_AGENT_CODE};
use crate::composer::skills::COMMENT_HYGIENE_NEEDLE;
use crate::host::EngineHost;
use crate::loops::test_support::*;
use crate::testing::{TestEngine, TestOptions, add_project, init_repo, wait_for};
use crate::turn::test_support::maestro_fixture;

fn has_tool(req: &TurnRequest, tool: &str) -> bool {
    req.allowed_tools
        .as_ref()
        .is_some_and(|a| a.iter().any(|t| t == tool))
}

fn assert_linear(req: &TurnRequest, url: &str) {
    assert_eq!(req.user_mcp.len(), 1, "{:?} user_mcp", req.role);
    assert_eq!(req.user_mcp[0].url, url, "{:?}", req.role);
    assert!(has_tool(req, "mcp__linear__*"), "{:?}", req.role);
}

/// The dock's Remember with an explicit Simplify chip (Go
/// `rememberDockSimplify`).
async fn remember_dock_simplify(t: &TestEngine, ws: &str, code: &str, on: bool) {
    t.engine
        .remember_workspace_chat_selection(ws, code, "sonnet", None, CHAT_MODE_AGENT, Some(on))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan_approval_gate() {
    let runner = CaptureScript::new(|call| maestro_fixture(&format!("gate-{call}"), "OK."));
    let t = TestEngine::new(Some(runner.provider()));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let host = EngineHost::new(Arc::downgrade(&t.engine));

    let ws = t.engine.create_workspace(&proj, "gated", "").await.unwrap();
    host.submit_plan(
        &ws.id,
        "PA",
        PlanSubmission {
            summary: "Wire the toggle".into(),
            steps: vec!["Edit settings".into(), "Add persistence".into()],
            ..PlanSubmission::default()
        },
    )
    .await;
    let queued = t
        .engine
        .start_run_on_workspace(&ws.id, "ship the toggle")
        .await
        .unwrap();
    assert!(!queued);
    wait_for(Duration::from_secs(5), "plan-approval escalation", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| w.state == WorkspaceState::Escalated)
    })
    .await;
    let esc = t.store.escalation(&ws.id).expect("an escalation");
    assert!(esc.question.contains("plan"), "{}", esc.question);
    let got = t.store.workspace(&ws.id).unwrap();
    assert_eq!(
        got.plan.as_ref().map(|p| p.source.as_str()),
        Some("plan-chat"),
        "the existing plan is adopted, not re-planned"
    );

    assert!(
        t.engine
            .answer_escalation(&ws.id, "Approve and proceed", "")
            .await
            .is_some()
    );
    wait_for(Duration::from_secs(10), "run to settle at pr_open", || {
        t.store
            .workspace(&ws.id)
            .is_some_and(|w| w.state == WorkspaceState::PrOpen)
    })
    .await;
    assert!(
        t.store
            .workspace(&ws.id)
            .unwrap()
            .plan
            .is_some_and(|p| p.approved),
        "answering the gate approves the plan"
    );
    let coder = runner
        .with_role(AgentRole::Coder)
        .into_iter()
        .next()
        .expect("a coder turn");
    assert!(coder.system_prompt.contains("Wire the toggle"));
    assert!(coder.system_prompt.contains("update_plan_step"));
    assert!(coder.append_system_prompt.contains("SIMPLIFY MODE"));
    assert!(coder.append_system_prompt.contains(COMMENT_HYGIENE_NEEDLE));
    assert!(has_tool(&coder, "mcp__recur__update_plan_step"));

    // Aggressive: same setup, no gate.
    let mut pol = t.store.policy();
    pol.autonomy = AutonomyLevel::Aggressive;
    t.store.set_policy(pol);
    let ws2 = t
        .engine
        .create_workspace(&proj, "ungated", "")
        .await
        .unwrap();
    host.submit_plan(
        &ws2.id,
        "PA",
        PlanSubmission {
            summary: "Small fix".into(),
            steps: vec!["one".into()],
            ..PlanSubmission::default()
        },
    )
    .await;
    assert!(
        !t.engine
            .start_run_on_workspace(&ws2.id, "small fix")
            .await
            .unwrap()
    );
    wait_for(Duration::from_secs(10), "aggressive run settles", || {
        t.store
            .workspace(&ws2.id)
            .is_some_and(|w| w.state == WorkspaceState::PrOpen)
    })
    .await;
    assert!(t.store.escalation(&ws2.id).is_none());
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_specialists_attach_linear() {
    let runner = ConvergeRunner::new(&[0]);
    let t = TestEngine::with(TestOptions {
        provider: Some(runner.provider()),
        trackers: linear_trackers(),
        ..TestOptions::default()
    });
    enable_linear_mcp(&t.store);
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(&proj, "ACME-1 fix it", "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(Duration::from_secs(10), "loop to reach pr_open", || {
        t.store
            .workspace(&id)
            .is_some_and(|l| l.state == WorkspaceState::PrOpen)
    })
    .await;
    let specs = runner.state.lock().specs.clone();
    let of = |role: AgentRole| -> Vec<TurnRequest> {
        specs.iter().filter(|s| s.role == role).cloned().collect()
    };
    let planner = of(AgentRole::Maestro);
    assert!(!planner.is_empty(), "the planner ran");
    assert_linear(&planner[0], LINEAR_MCP_READ_ONLY_URL);
    let coders = of(AgentRole::Coder);
    assert!(!coders.is_empty());
    for c in &coders {
        assert_linear(c, LINEAR_MCP_WRITE_URL);
    }
    let reviews = of(AgentRole::Reviewer);
    assert!(!reviews.is_empty());
    for r in &reviews {
        assert_linear(r, LINEAR_MCP_READ_ONLY_URL);
    }
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_mcp_fails_the_loop_instead_of_converging() {
    // Files a blocking finding if it ever gets an endpoint; quiet without.
    let ran = Arc::new(parking_lot::Mutex::new(0usize));
    let r = Arc::clone(&ran);
    let t = TestEngine::with(TestOptions {
        provider: Some(AsyncProvider::wrap(move |req, _cx| {
            let r = Arc::clone(&r);
            async move {
                *r.lock() += 1;
                if req.role == AgentRole::Reviewer && !req.mcp_url.is_empty() {
                    post_finding(&req.mcp_url, "work.txt", 1, "high", "must not ship")
                        .await
                        .map_err(recur_agent::ProviderError::failed)?;
                }
                Ok(success(&req.agent_id, ""))
            }
        })),
        mcp_addr: Some("127.0.0.1:-1".into()),
        ..TestOptions::default()
    });
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(&proj, "Build the feature", "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(Duration::from_secs(10), "a terminal state", || {
        t.store.workspace(&id).is_some_and(|l| {
            matches!(
                l.state,
                WorkspaceState::Failed | WorkspaceState::PrOpen | WorkspaceState::Stopped
            )
        })
    })
    .await;
    let l = t.store.workspace(&id).unwrap();
    assert_ne!(
        l.state,
        WorkspaceState::PrOpen,
        "no MCP read as convergence"
    );
    assert_eq!(l.state, WorkspaceState::Failed);
    assert!(
        l.last_error.to_lowercase().contains("mcp"),
        "{}",
        l.last_error
    );
    assert!(l.retryable, "a bind failure clears; Retry must be offered");
    assert_eq!(l.group, recur_domain::Group::Needs);
    assert_eq!(*ran.lock(), 0, "no turn ran without an MCP endpoint");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simplify_loop_inherits_pair_chat_off() {
    let t = TestEngine::new(Some(
        CaptureScript::new(|_| maestro_fixture("sm-inherit", "ok")).provider(),
    ));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "scratch", "")
        .await
        .unwrap();
    t.engine.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "");
    remember_dock_simplify(&t, &ws.id, PAIR_AGENT_CODE, false).await;
    let slots = take_slots(&t);
    assert!(
        t.engine
            .start_run_on_workspace(&ws.id, "ship it")
            .await
            .unwrap(),
        "queued (slots held)"
    );
    assert_eq!(t.engine.simplify_coder_append(&ws.id), "");
    assert_eq!(
        t.engine.chat_simplify(&ws.id, MAESTRO_AGENT_CODE),
        Some(false)
    );
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn begin_start_workspace_stamps_simplify_chip() {
    let t = TestEngine::new(Some(
        CaptureScript::new(|_| maestro_fixture("sm-begin", "ok")).provider(),
    ));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let slots = take_slots(&t);
    let ws = t
        .engine
        .begin_start_workspace(&proj, "ship it", "", AutonomyLevel::Balanced, Some(false))
        .await
        .unwrap();
    assert_eq!(
        t.engine.chat_simplify(&ws.id, MAESTRO_AGENT_CODE),
        Some(false),
        "the create chip, not the settings default"
    );
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loop_engage_stamps_maestro_before_start() {
    let t = TestEngine::new(Some(
        CaptureScript::new(|_| maestro_fixture("sm-engage", "ok")).provider(),
    ));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "scratch", "")
        .await
        .unwrap();
    t.engine.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "");
    remember_dock_simplify(&t, &ws.id, PAIR_AGENT_CODE, false).await;
    let slots = take_slots(&t);
    let rec = t.store.workspace(&ws.id).unwrap();
    let reply = t
        .engine
        .send_loop_chat_message(rec, PAIR_AGENT_CODE, "ship it", &[], "")
        .await;
    assert!(!reply.contains("I couldn't engage"), "{reply}");
    let got = t.store.workspace(&ws.id).unwrap();
    assert!(got.has_run());
    assert_eq!(got.state, WorkspaceState::Queued);
    assert_eq!(
        t.engine.chat_simplify(&ws.id, MAESTRO_AGENT_CODE),
        Some(false)
    );
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test]
async fn remember_pair_chat_selection_keeps_simplify() {
    let t = TestEngine::new(None);
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "scratch", "")
        .await
        .unwrap();
    t.engine.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "");
    remember_dock_simplify(&t, &ws.id, PAIR_AGENT_CODE, false).await;
    let flags = BTreeMap::from([("effort".to_owned(), "high".to_owned())]);
    t.engine
        .remember_workspace_chat_selection(
            &ws.id,
            PAIR_AGENT_CODE,
            "sonnet",
            Some(&flags),
            CHAT_MODE_REVIEW,
            None,
        )
        .await;
    assert_eq!(t.engine.chat_simplify(&ws.id, PAIR_AGENT_CODE), Some(false));
    assert_eq!(
        t.engine.workspace_chat_selection(&ws.id, PAIR_AGENT_CODE).2,
        CHAT_MODE_REVIEW
    );
    t.close().await;
}
