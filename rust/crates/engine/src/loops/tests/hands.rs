//! Port of Go `maestro_hands_test.go`: edit_plan, dispatch_specialist and
//! extend_iterations through the Host seam, and every hand scoped to the
//! calling chat's walk (context liveness, grants, steers, extend_rounds,
//! reenter_phase, run_phase, Resume, Restart, resume_loop, answers).

use std::sync::Arc;
use std::time::Duration;

use recur_agent::Provider;
use recur_agent_tools::{Finding, Host, PlanSubmission};
use recur_bundles::{LoopPhase, VERB_CONVERGE};
use recur_domain::{
    Agent, AgentRole, AutonomyLevel, Checkpoint, Escalation, Group, Severity, WORKSPACE_LOOP,
    Workspace, WorkspaceState,
};
use tokio_util::sync::CancellationToken;

use crate::host::EngineHost;
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::{LOOP_MAP_SEP, RunningLoop};
use crate::loops::test_support::*;
use crate::testing::{TestEngine, add_project, init_repo, wait_for};
use crate::turn::test_support::maestro_fixture;

fn ok_lines(session: &str) -> Vec<String> {
    vec![
        format!(
            r#"{{"type":"system","subtype":"init","session_id":"{session}","model":"m","mcp_servers":[]}}"#
        ),
        format!(
            r#"{{"type":"result","subtype":"success","session_id":"{session}","duration_ms":5,"is_error":false,"num_turns":1,"result":"done","total_cost_usd":0.01}}"#
        ),
    ]
}

fn host(t: &TestEngine) -> EngineHost {
    EngineHost::new(Arc::downgrade(&t.engine))
}

/// A created workspace flipped to a (settled) loop record.
async fn loop_ws(
    t: &TestEngine,
    repo: &str,
    name: &str,
    f: impl FnOnce(&mut Workspace),
) -> Workspace {
    let proj = add_project(&t.store, repo);
    let ws = t.engine.create_workspace(&proj, name, "").await.unwrap();
    t.store.update_workspace(&ws.id, |l| {
        l.r#type = WORKSPACE_LOOP.into();
        f(l);
    });
    t.store.workspace(&ws.id).unwrap()
}

/// A registered walk with no task behind it (Go `newHandLoop`).
fn hand_loop(
    t: &TestEngine,
    ws: &str,
    chat: &str,
    key: &str,
    f: impl FnOnce(&mut LoopState),
) -> Arc<LoopHandle> {
    let mut st = LoopState {
        chat_code: chat.into(),
        ..LoopState::default()
    };
    f(&mut st);
    let h = LoopHandle::new(ws, "", key, st);
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&h),
        cancel: CancellationToken::new(),
    }));
    h
}

fn take_slots(t: &TestEngine) -> usize {
    let mut n = 0;
    while t.engine.governor().try_acquire_loop() {
        n += 1;
    }
    n
}

fn release_slots(t: &TestEngine, n: usize) {
    for _ in 0..n {
        t.engine.governor().release_loop();
    }
}

fn capture(
    lines: fn(&str) -> Vec<String>,
    session: &'static str,
) -> (Arc<CaptureScript>, Arc<dyn Provider>) {
    let c = CaptureScript::new(move |_| lines(session));
    let p = c.provider();
    (c, p)
}

fn fixture_ok(session: &str) -> Vec<String> {
    maestro_fixture(session, "OK.")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn maestro_hands() {
    let (runner, provider) = capture(ok_lines, "hand-1");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "hands", |_| {}).await;
    let rep = host(&t);

    // edit_plan persists + deviates
    let status = rep
        .edit_plan(&ws.id, "MA", "Ship only the config change; defer the UI.")
        .await;
    assert!(status.contains("Plan updated"), "{status}");
    assert_eq!(
        t.store.workspace(&ws.id).unwrap().definition_of_done,
        "Ship only the config change; defer the UI."
    );
    assert!(t.events.all().iter().any(|e| matches!(e,
        crate::bus::EngineEvent::WorkspaceEvent(w) if w.event.is_deviation && w.event.text.contains("Plan edited"))));
    assert!(rep.edit_plan("nope", "MA", "x").await.contains("Refused"));

    // dispatch_specialist runs; findings persist without a running loop
    let status = rep
        .dispatch_specialist(&ws.id, "MA", "reviewer", "look at the config handling")
        .await;
    assert!(status.contains("dispatched"), "{status}");
    wait_for(Duration::from_secs(3), "dispatched turn ran", || {
        runner
            .spec_at(0)
            .is_some_and(|s| s.role == AgentRole::Reviewer)
    })
    .await;
    let s0 = runner.spec_at(0).unwrap();
    assert_eq!(s0.worktree.to_string_lossy(), ws.worktree_path);
    let tools = s0.allowed_tools.clone().unwrap_or_default();
    assert!(
        !tools.iter().any(|t| t == "Edit" || t == "Write"),
        "{tools:?}"
    );
    assert!(tools.iter().any(|t| t == "Bash"), "{tools:?}");
    assert_eq!(s0.permission_mode, t.store.settings().permission_mode());
    rep.report_finding(
        &ws.id,
        "RV",
        Finding {
            file: "a.go".into(),
            line: 3,
            severity: Severity::High,
            message: "bug".into(),
            ..Finding::default()
        },
    );
    assert_eq!(t.store.open_findings_count(&ws.id), 1);

    // extend_iterations refuses with the reason when not running
    let status = rep.extend_iterations(&ws.id, "MA", 2).await;
    assert!(
        status.contains("Refused") && status.contains("not running"),
        "{status}"
    );
    t.close().await;
}

#[tokio::test]
async fn edit_plan_clears_approval_and_keeps_summary() {
    let (_runner, provider) = capture(ok_lines, "edit-1");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "edit-plan", |_| {}).await;
    host(&t)
        .submit_plan(
            &ws.id,
            "PA",
            PlanSubmission {
                summary: "Curated headline".into(),
                body: "Original body.".into(),
                steps: vec!["Do the thing".into()],
                ..PlanSubmission::default()
            },
        )
        .await;
    assert_eq!(t.engine.approve_plan(&ws.id), "Plan approved.");
    assert!(
        t.engine
            .edit_plan(&ws.id, "A substantially different plan body.")
            .contains("Plan updated")
    );
    let plan = t.store.workspace(&ws.id).unwrap().plan.expect("plan");
    assert!(!plan.approved);
    assert_eq!(plan.summary, "Curated headline");
    assert_eq!(plan.body, "A substantially different plan body.");
    t.close().await;
}

#[tokio::test]
async fn maestro_hands_scope_to_calling_chat() {
    let (_r, provider) = capture(fixture_ok, "scope");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "scope-chat", |_| {}).await;
    let sibling = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |_| {},
    );
    assert!(
        !t.engine.maestro_context_for_chat(&ws, "PA").live,
        "no inherited liveness"
    );
    assert!(t.engine.maestro_context_for_chat(&ws, "PA2").live);
    assert!(
        t.engine
            .extend_loop_iterations(&ws.id, "PA", 2)
            .contains("not running")
    );
    assert!(
        t.engine
            .extend_loop_iterations(&ws.id, "PA2", 2)
            .contains("Granted")
    );
    assert_eq!(sibling.lock().granted_extra, 2);
    let g = t
        .engine
        .resolve_guard(&ws.loop_id, &ws.policy_id, &ws.autonomy)
        .expect("guard");
    let phase = g.loop_def.reenter_ids()[0].clone();
    assert!(
        t.engine
            .reenter_phase(&ws.id, "PA2", &phase)
            .await
            .contains("idle only")
    );
    let slots = take_slots(&t);
    let s = t.engine.reenter_phase(&ws.id, "PA", &phase).await;
    assert!(s.contains("loop slot"), "{s}");
    release_slots(&t, slots);
    t.engine
        .apply_maestro_steer(&ws.id, "PA", "steer only this chat");
    assert_eq!(
        sibling.lock().pending_steer,
        "",
        "an idle chat never steers a sibling"
    );
    t.engine.remove_loop(&sibling);
    t.close().await;
}

#[tokio::test]
async fn extend_loop_rounds_answers_suffixed_loop() {
    let (_r, provider) = capture(fixture_ok, "cap");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "cap-chat", |l| {
        l.state = WorkspaceState::Escalated;
        l.checkpoint = Some(Checkpoint {
            phase_id: "verify".into(),
            ..Checkpoint::default()
        });
    })
    .await;
    t.store.add_escalation(Escalation {
        id: "esc-cap".into(),
        workspace_id: ws.id.clone(),
        status: "open".into(),
        question: "cap".into(),
        ..Escalation::default()
    });
    let root = hand_loop(&t, &ws.id, "PA", &ws.id, |s| s.escalated = true);
    let cap = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |s| {
            s.escalated = true;
            s.round_cap_esc = true;
            s.current_phase = LoopPhase {
                id: "converge".into(),
                uses: VERB_CONVERGE.into(),
                ..LoopPhase::default()
            };
        },
    );
    let got = t.engine.extend_loop_rounds(&ws.id, "PA2", 0).await;
    assert!(got.contains("Granted"), "{got}");
    assert!(cap.lock().answer.is_some() && !cap.lock().escalated);
    assert!(root.lock().answer.is_none() && root.lock().escalated);
    t.engine.remove_loop(&root);
    t.engine.remove_loop(&cap);
    t.close().await;
}

#[tokio::test]
async fn maestro_context_state_is_chat_local() {
    let (_r, provider) = capture(fixture_ok, "ctx");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "ctx-chat", |l| {
        l.state = WorkspaceState::Coding;
        l.iteration = 9;
    })
    .await;
    let cap = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |s| {
            s.escalated = true;
            s.round_cap_esc = true;
            s.walk_state = WorkspaceState::Escalated;
            s.walk_iter = 4;
        },
    );
    let ctx = t.engine.maestro_context_for_chat(&ws, "PA2");
    assert!(ctx.live && ctx.state == WorkspaceState::Escalated.as_str());
    assert_eq!(ctx.iteration, 4);
    let fresh = hand_loop(
        &t,
        &ws.id,
        "PA3",
        &format!("{}{LOOP_MAP_SEP}PA3", ws.id),
        |s| {
            s.walk_iter = 0;
            s.goal = "chat-local goal".into();
        },
    );
    let zero = t.engine.maestro_context_for_chat(&ws, "PA3");
    assert_eq!(zero.iteration, 0, "this walk's, not the workspace's 9");
    assert_eq!(zero.goal, "chat-local goal");
    let idle = t.engine.maestro_context_for_chat(&ws, "PA");
    assert!(!idle.live && idle.state == WorkspaceState::Coding.as_str());
    t.engine.remove_loop(&cap);
    t.engine.remove_loop(&fresh);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reenter_phase_uses_calling_chat_graph() {
    let (_r, provider) = capture(fixture_ok, "reenter");
    let t = TestEngine::new(Some(provider));
    t.install_fixture_pack();
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "reenter-chat", |l| {
        l.state = WorkspaceState::Paused;
        l.loop_chat_code = "PA".into();
        l.loop_id.clear();
    })
    .await;
    t.engine
        .remember_chat_loop_policy(&ws.id, "PA2", "test.loops.verify", "")
        .await;
    let slots = take_slots(&t);
    let s = t.engine.reenter_phase(&ws.id, "PA2", "verify").await;
    assert!(!s.contains("not on this loop"), "{s}");
    assert!(s.contains("Loop restarting"), "{s}");
    let got = t.store.workspace(&ws.id).unwrap();
    assert_eq!(got.loop_id, "test.loops.verify");
    assert_eq!(got.loop_chat_code, "PA2");
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test]
async fn resume_wakes_suffixed_loop() {
    let (_r, provider) = capture(fixture_ok, "resume");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "resume-chat", "")
        .await
        .unwrap();
    let lp = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |s| s.paused = true,
    );
    t.engine.resume(&ws.id).await;
    assert!(!lp.is_paused(), "Resume wakes a chat-keyed paused walk");
    t.engine.remove_loop(&lp);
    t.close().await;
}

#[tokio::test]
async fn extend_loop_rounds_does_not_answer_sibling_escalation() {
    let (_r, provider) = capture(fixture_ok, "cap-sib");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "cap-sib", |l| {
        l.state = WorkspaceState::Escalated
    })
    .await;
    for (id, code, q) in [("esc-a", "PA", "A cap"), ("esc-b", "PA2", "B cap")] {
        t.store.add_escalation(Escalation {
            id: id.into(),
            workspace_id: ws.id.clone(),
            agent_code: code.into(),
            status: "open".into(),
            question: q.into(),
            ..Escalation::default()
        });
    }
    let root = hand_loop(
        &t,
        &ws.id,
        "PA",
        &format!("{}{LOOP_MAP_SEP}PA", ws.id),
        |s| s.escalated = true,
    );
    let cap = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |s| {
            s.escalated = true;
            s.round_cap_esc = true;
        },
    );
    let got = t.engine.extend_loop_rounds(&ws.id, "PA2", 0).await;
    assert!(got.contains("Granted"), "{got}");
    assert!(cap.lock().answer.is_some() && !cap.lock().escalated);
    assert!(root.lock().answer.is_none() && root.lock().escalated);
    let (esc, ok) = t.store.answer_escalation_for(&ws.id, "PA", "later", "");
    assert!(ok && esc.unwrap().question == "A cap");
    t.engine.remove_loop(&root);
    t.engine.remove_loop(&cap);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_scoped_workspace_restores_goal() {
    let (_r, provider) = capture(fixture_ok, "goal");
    let t = TestEngine::new(Some(provider));
    t.install_fixture_pack();
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "goal-chat", |l| {
        l.state = WorkspaceState::Paused;
        l.loop_chat_code = "PA".into();
        l.goal = "owner goal".into();
        l.goal_display = "Linear ENG-123 · title".into();
        l.checkpoint = Some(Checkpoint {
            phase_id: "planning".into(),
            ..Checkpoint::default()
        });
    })
    .await;
    t.engine.stamp_chat_loop(
        &ws.id,
        "PA2",
        "test.loops.verify",
        "",
        "chat goal",
        "",
        true,
    );
    let rec = t.engine.chat_scoped_workspace(ws.clone(), "PA2");
    assert_eq!(rec.goal, "chat goal");
    assert_eq!(rec.goal_display, "");
    assert!(rec.checkpoint.is_none_or(|c| c.phase_id != "planning"));
    let slots = take_slots(&t);
    let s = t.engine.reenter_phase(&ws.id, "PA2", "verify").await;
    assert!(s.contains("Loop restarting"), "{s}");
    let got = t.store.workspace(&ws.id).unwrap();
    assert_eq!(got.goal, "chat goal");
    assert_eq!(got.goal_display, "");
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test]
async fn reenter_phase_does_not_mutate_queued_run() {
    let (_r, provider) = capture(fixture_ok, "queued-reenter");
    let t = TestEngine::new(Some(provider));
    t.install_fixture_pack();
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "queued-reenter", |l| {
        l.state = WorkspaceState::Queued;
        l.loop_chat_code = "PA".into();
        l.loop_id = "owner.loop".into();
        l.checkpoint = Some(Checkpoint {
            phase_id: "converge".into(),
            ..Checkpoint::default()
        });
        l.goal = "queued owner goal".into();
    })
    .await;
    t.engine
        .remember_chat_loop_policy(&ws.id, "PA2", "test.loops.verify", "")
        .await;
    t.engine.stamp_chat_loop(
        &ws.id,
        "PA2",
        "test.loops.verify",
        "",
        "second chat goal",
        "",
        true,
    );
    let s = t.engine.reenter_phase(&ws.id, "PA2", "verify").await;
    assert!(s.contains("queued"), "{s}");
    let got = t.store.workspace(&ws.id).unwrap();
    assert_eq!(got.loop_chat_code, "PA");
    assert_eq!(got.loop_id, "owner.loop");
    assert_eq!(got.checkpoint.unwrap().phase_id, "converge");
    assert_eq!(got.goal, "queued owner goal");
    t.close().await;
}

fn open_escalations(t: &TestEngine, ws: &str, rows: &[(&str, &str, &str)]) {
    for (id, code, q) in rows {
        t.store.add_escalation(Escalation {
            id: (*id).into(),
            workspace_id: ws.into(),
            agent_code: (*code).into(),
            status: "open".into(),
            question: (*q).into(),
            ..Escalation::default()
        });
    }
}

#[tokio::test]
async fn answer_escalation_delivers_to_owning_chat() {
    let (_r, provider) = capture(fixture_ok, "esc-own");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "esc-own", |l| {
        l.state = WorkspaceState::Escalated;
        l.group = Group::Needs;
    })
    .await;
    t.store.add_escalation(Escalation {
        id: "esc-old".into(),
        workspace_id: ws.id.clone(),
        status: "answered".into(),
        question: "stale".into(),
        answer: "done".into(),
        ..Escalation::default()
    });
    open_escalations(&t, &ws.id, &[("esc-a", "PA", "A"), ("esc-b", "PA2", "B")]);
    let root = hand_loop(
        &t,
        &ws.id,
        "PA",
        &format!("{}{LOOP_MAP_SEP}PA", ws.id),
        |s| s.escalated = true,
    );
    let sib = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |s| s.escalated = true,
    );
    let answered = t
        .engine
        .answer_escalation(&ws.id, "Stop", "")
        .await
        .expect("answered");
    assert_eq!(answered.question, "A");
    assert!(root.lock().answer.is_some() && !root.lock().escalated);
    assert!(sib.lock().answer.is_none() && sib.lock().escalated);
    assert_eq!(
        t.store.workspace(&ws.id).unwrap().group,
        Group::Needs,
        "PA2 still open"
    );
    t.engine.remove_loop(&root);
    t.engine.remove_loop(&sib);
    t.close().await;
}

#[tokio::test]
async fn answer_escalation_for_routes_to_named_chat() {
    let (_r, provider) = capture(fixture_ok, "esc-named");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "esc-named", |l| {
        l.state = WorkspaceState::Escalated;
        l.group = Group::Needs;
    })
    .await;
    open_escalations(&t, &ws.id, &[("esc-a", "PA", "A"), ("esc-b", "PA2", "B")]);
    let root = hand_loop(
        &t,
        &ws.id,
        "PA",
        &format!("{}{LOOP_MAP_SEP}PA", ws.id),
        |s| s.escalated = true,
    );
    let sib = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |s| s.escalated = true,
    );
    let answered = t
        .engine
        .answer_escalation_for(&ws.id, "PA2", "Stop", "")
        .await
        .expect("answered");
    assert_eq!(answered.question, "B");
    assert!(sib.lock().answer.is_some() && !sib.lock().escalated);
    assert!(root.lock().answer.is_none() && root.lock().escalated);
    t.engine.remove_loop(&root);
    t.engine.remove_loop(&sib);
    t.close().await;
}

#[tokio::test]
async fn restart_refuses_when_sibling_live() {
    let (_r, provider) = capture(fixture_ok, "restart-sib");
    let t = TestEngine::new(Some(provider));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "restart-sib", |l| {
        l.state = WorkspaceState::Stopped
    })
    .await;
    let sib = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |_| {},
    );
    let s = t.engine.restart(&ws.id).await;
    assert!(s.contains("already running"), "{s}");
    t.engine.remove_loop(&sib);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_loop_starts_walk_when_sibling_live() {
    // Turns block so the new walk sits on its first phase.
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[])));
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "resume-sib", |l| {
        l.state = WorkspaceState::Stopped;
        l.checkpoint = Some(Checkpoint {
            phase_id: "converge".into(),
            ..Checkpoint::default()
        });
    })
    .await;
    t.store.upsert_agent(
        &ws.id,
        Agent {
            id: format!("{}-PA", ws.id),
            workspace_id: ws.id.clone(),
            code: "PA".into(),
            name: "Agent".into(),
            ..Agent::default()
        },
    );
    let sib = hand_loop(
        &t,
        &ws.id,
        "PA2",
        &format!("{}{LOOP_MAP_SEP}PA2", ws.id),
        |_| {},
    );
    let got = host(&t).resume_loop(&ws.id, "PA").await;
    assert!(got.contains("restarting"), "{got}");
    let lp = t
        .engine
        .running_loop_by_chat(&ws.id, "PA")
        .expect("this chat's walk started");
    wait_for(
        Duration::from_secs(5),
        "the new walk to enter its first phase",
        || !lp.lock().current_phase.id.is_empty(),
    )
    .await;
    assert_eq!(
        lp.lock().current_phase.id,
        "planning",
        "a fresh restart starts at the graph entry, not the sibling checkpoint"
    );
    let pa2 = t
        .engine
        .running_loop_by_chat(&ws.id, "PA2")
        .expect("sibling stays");
    assert!(Arc::ptr_eq(&pa2, &sib));
    t.engine.stop_loop_by_chat(&ws.id, "PA");
    gate.cancel();
    t.engine.remove_loop(&sib);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_run_phase_does_not_adopt_sibling_checkpoint() {
    let (_r, provider) = capture(fixture_ok, "phase-cp");
    let t = TestEngine::new(Some(provider));
    t.install_fixture_pack();
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "phase-cp", |l| {
        l.state = WorkspaceState::Paused;
        l.loop_chat_code = "PA".into();
        l.checkpoint = Some(Checkpoint {
            phase_id: "converge".into(),
            ..Checkpoint::default()
        });
    })
    .await;
    t.engine
        .remember_chat_loop_policy(&ws.id, "PA2", "test.loops.verify", "")
        .await;
    let slots = take_slots(&t);
    let s = t
        .engine
        .run_on_demand_phase(&ws.id, "PA2", "ondemand")
        .await;
    assert!(s.contains("Loop restarting"), "{s}");
    let cp = t
        .store
        .workspace(&ws.id)
        .unwrap()
        .checkpoint
        .expect("checkpoint");
    assert_eq!(cp.phase_id, "ondemand");
    assert_eq!(
        cp.resume_phase_id, "",
        "a sibling's parked phase is not resumed"
    );
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reenter_phase_serializes_idle_owner() {
    let (_r, provider) = capture(fixture_ok, "reenter-lock");
    let t = TestEngine::new(Some(provider));
    t.install_fixture_pack();
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "reenter-lock", |l| {
        l.state = WorkspaceState::Paused;
        l.loop_chat_code = "PA".into();
    })
    .await;
    t.engine
        .remember_chat_loop_policy(&ws.id, "PA2", "test.loops.verify", "")
        .await;
    let g = t
        .engine
        .resolve_guard("", "", &AutonomyLevel::Balanced)
        .unwrap();
    let first = g.loop_def.reenter_ids()[0].clone();
    let slots = take_slots(&t);
    let (a, b) = tokio::join!(
        t.engine.reenter_phase(&ws.id, "PA", &first),
        t.engine.reenter_phase(&ws.id, "PA2", "verify")
    );
    let restarting = [&a, &b]
        .iter()
        .filter(|s| s.contains("Loop restarting"))
        .count();
    assert_eq!(restarting, 1, "a={a} b={b}");
    let got = t.store.workspace(&ws.id).unwrap();
    if a.contains("Loop restarting") {
        assert_eq!(got.loop_chat_code, "PA");
    }
    if b.contains("Loop restarting") {
        assert_eq!(got.loop_chat_code, "PA2");
    }
    release_slots(&t, slots);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reenter_phase_same_chat_does_not_double_dispatch() {
    let gate = CancellationToken::new();
    let runner = Arc::new(CountingRunner {
        gate: Some(gate.clone()),
        ..CountingRunner::default()
    });
    let t = TestEngine::new(Some(runner.provider()));
    t.install_fixture_pack();
    let repo = init_repo();
    let ws = loop_ws(&t, &repo.path, "reenter-same", |l| {
        l.state = WorkspaceState::Paused;
        l.loop_chat_code = "PA".into();
    })
    .await;
    t.store.upsert_agent(
        &ws.id,
        Agent {
            id: format!("{}-PA", ws.id),
            workspace_id: ws.id.clone(),
            code: "PA".into(),
            name: "Agent".into(),
            ..Agent::default()
        },
    );
    let g = t
        .engine
        .resolve_guard("", "", &AutonomyLevel::Balanced)
        .unwrap();
    let first = g.loop_def.reenter_ids()[0].clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    {
        let e = Arc::clone(&entered);
        t.engine.set_promote_hook(move || {
            e.notify_one();
            std::thread::sleep(Duration::from_millis(40));
        });
    }
    let eng = Arc::clone(&t.engine);
    let (wsid, f) = (ws.id.clone(), first.clone());
    let a = tokio::spawn(async move { eng.reenter_phase(&wsid, "PA", &f).await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("the first reenter entered dispatch");
    let b = t.engine.reenter_phase(&ws.id, "PA", &first).await;
    let a = a.await.unwrap();
    let restarting = [&a, &b]
        .iter()
        .filter(|s| s.contains("Loop restarting"))
        .count();
    assert_eq!(restarting, 1, "a={a} b={b}");
    let n = usize::from(t.engine.running_loop_by_id(&ws.id).is_some())
        + usize::from(
            t.engine
                .loop_key_running(&format!("{}{LOOP_MAP_SEP}PA", ws.id)),
        );
    assert_eq!(n, 1, "one live walk for this chat");
    gate.cancel();
    t.close().await;
}
