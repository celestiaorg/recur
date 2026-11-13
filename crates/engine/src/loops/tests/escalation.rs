//! Escalation tests: the round-cap escalation round trip, keep-iterating and
//! extend_rounds, Stop/Shutdown while escalated, answers after a restart (cap,
//! plan notes, agent questions, delayed dispatch), chat-owned rows with no live
//! walk, and the persisted-kind rules.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use recur_agent_tools::{Host, PlanSubmission};
use recur_bundles::{
    FindingsThreshold, LoopDef, LoopPhase, Policy, VERB_ARTIFACT, VERB_CONVERGE, VERB_ENSURE_PR,
};
use recur_domain::{
    Agent, AutonomyLevel, Checkpoint, DEFAULT_MAX_LOOPS, Escalation, EscalationOption, Finding,
    Group, Severity, WORKSPACE_LOOP, Workspace, WorkspaceState,
};
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::bus::EngineEvent;
use crate::guard::ResolvedGuard;
use crate::host::EngineHost;
use crate::lifecycle::PHASE_INTERRUPTED;
use crate::loops::Walk;
use crate::loops::converge::{CapAnswer, classify_cap_answer};
use crate::loops::escalation::*;
use crate::loops::test_support::*;
use crate::publish::terminal;
use crate::testing::{TestEngine, add_project, init_repo, wait_for};

fn state(t: &TestEngine, id: &str) -> WorkspaceState {
    t.store.workspace(id).map(|w| w.state).unwrap_or_default()
}

async fn wait_state(t: &TestEngine, id: &str, s: WorkspaceState, secs: u64, what: &str) {
    wait_for(Duration::from_secs(secs), what, || state(t, id) == s).await;
}

async fn wait_terminal(t: &TestEngine, id: &str, secs: u64, what: &str) {
    wait_for(Duration::from_secs(secs), what, || terminal(&state(t, id))).await;
}

async fn wait_slots_free(eng: &Engine, secs: u64) {
    wait_for(Duration::from_secs(secs), "loop slot released", || {
        eng.governor().loops_running() == 0
    })
    .await;
}

/// Shutdown, bounded by an 8 s hang guard.
async fn shutdown_within(eng: &Engine) {
    tokio::time::timeout(Duration::from_secs(8), eng.shutdown())
        .await
        .expect("Shutdown hung on a loop blocked on an escalation");
}

async fn started(t: &TestEngine, repo: &str, goal: &str) -> String {
    let proj = add_project(&t.store, repo);
    t.engine
        .start_workspace(&proj, goal, "", AutonomyLevel::default())
        .await
        .expect("start")
}

fn never_converges(n: usize) -> Arc<ConvergeRunner> {
    ConvergeRunner::new(&vec![2; n])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn escalation_round_trip_at_cap() {
    let repo = init_repo();
    let t = TestEngine::new(Some(never_converges(8).provider()));
    let id = started(&t, &repo.path, "never converges, must escalate").await;
    wait_state(&t, &id, WorkspaceState::Escalated, 15, "loop to escalate").await;
    let l = t.store.workspace(&id).unwrap();
    assert_eq!(l.group, Group::Needs);
    let esc = t.store.escalation(&id).expect("escalation persisted");
    assert_eq!(esc.status, "open");
    assert!(!esc.question.is_empty() && !esc.options.clone().unwrap_or_default().is_empty());
    assert!(t.events.has("escalation.raised"));
    assert_eq!(
        t.engine.governor().loops_running(),
        1,
        "the blocked walk holds its slot"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state(&t, &id), WorkspaceState::Escalated);

    let answered = t
        .engine
        .answer_escalation(&id, "Stop", "")
        .await
        .expect("answered");
    assert_eq!(answered.status, "answered");
    wait_terminal(&t, &id, 10, "answered loop to settle").await;
    assert_eq!(state(&t, &id), WorkspaceState::Stopped);
    wait_slots_free(&t.engine, 5).await;
    assert!(t.events.has("escalation.resolved"));
    assert_eq!(t.store.escalation(&id).unwrap().status, "answered");
    let (raised, ans) = t.engine.escalation_stats();
    assert!(raised >= 1 && ans >= 1);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn escalation_keep_iterating_then_converges() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[2, 2, 2, 2, 0]).provider()));
    let id = started(&t, &repo.path, "converges after keep-iterating").await;
    wait_state(
        &t,
        &id,
        WorkspaceState::Escalated,
        15,
        "loop to escalate at cap",
    )
    .await;
    assert!(
        t.engine
            .answer_escalation(&id, "Keep iterating", "")
            .await
            .is_some()
    );
    wait_state(&t, &id, WorkspaceState::PrOpen, 15, "loop to converge").await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extend_loop_rounds_unblocks_cap() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[2, 2, 2, 2, 0]).provider()));
    let id = started(&t, &repo.path, "extend_rounds unblocks cap").await;
    wait_state(
        &t,
        &id,
        WorkspaceState::Escalated,
        15,
        "loop to escalate at cap",
    )
    .await;
    let got = t.engine.extend_loop_rounds(&id, "", 0).await;
    assert!(got.contains("Granted"), "{got}");
    assert!(
        t.engine
            .answer_escalation(&id, "Keep iterating", "")
            .await
            .is_none(),
        "the cap question was already answered"
    );
    wait_state(
        &t,
        &id,
        WorkspaceState::PrOpen,
        15,
        "loop to converge after extend_rounds",
    )
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extend_loop_rounds_refuses_agent_escalation() {
    let repo = init_repo();
    let runner = EscalatingRunner::new();
    let t = TestEngine::new(Some(runner.provider()));
    runner.bind(&t.engine);
    let id = started(&t, &repo.path, "agent question is not a round cap").await;
    wait_for(Duration::from_secs(10), "escalation.raised", || {
        t.events.has("escalation.raised")
    })
    .await;
    wait_state(
        &t,
        &id,
        WorkspaceState::Escalated,
        5,
        "loop to reach escalated",
    )
    .await;
    let got = t.engine.extend_loop_rounds(&id, "", 0).await;
    assert!(got.contains("Refused"), "{got}");
    assert_eq!(state(&t, &id), WorkspaceState::Escalated);
    assert!(t.engine.answer_escalation(&id, "Stop", "").await.is_some());
    wait_terminal(&t, &id, 10, "loop to finish").await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_while_escalated() {
    let repo = init_repo();
    let t = TestEngine::new(Some(never_converges(6).provider()));
    let id = started(&t, &repo.path, "stop while escalated").await;
    wait_state(&t, &id, WorkspaceState::Escalated, 15, "loop to escalate").await;
    let start = std::time::Instant::now();
    t.engine.stop(&id).await;
    wait_state(&t, &id, WorkspaceState::Stopped, 5, "blocked loop to stop").await;
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
    wait_slots_free(&t.engine, 5).await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_at_cap_closes_question_for_restart() {
    let repo = init_repo();
    let t = TestEngine::new(Some(never_converges(6).provider()));
    let id = started(&t, &repo.path, "stop at cap then restart").await;
    wait_state(&t, &id, WorkspaceState::Escalated, 15, "loop to escalate").await;
    t.engine.stop(&id).await;
    wait_state(&t, &id, WorkspaceState::Stopped, 5, "blocked loop to stop").await;
    wait_slots_free(&t.engine, 5).await;
    if let Some(esc) = t.store.escalation(&id) {
        assert_ne!(esc.status, "open", "stop left the cap question open");
    }
    assert!(t.events.has("escalation.resolved"));
    assert!(
        t.engine
            .answer_escalation(&id, "", "keep going")
            .await
            .is_none()
    );
    let status = t.engine.restart(&id).await;
    assert!(status.contains("restarting"), "{status}");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_recovered_escalation_closes_question_for_restart() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "recovered cap", "")
        .await
        .unwrap();
    t.store.update_workspace(&ws.id, |l| {
        l.r#type = WORKSPACE_LOOP.into();
        l.run_started = true;
        l.state = WorkspaceState::Escalated;
        l.group = Group::Needs;
        l.phase = "asking a question".into();
    });
    t.store.add_escalation(Escalation {
        id: "esc-recovered".into(),
        workspace_id: ws.id.clone(),
        status: "open".into(),
        question: "round cap".into(),
        ..Escalation::default()
    });
    t.store.add_escalation(Escalation {
        id: "esc-pa".into(),
        workspace_id: ws.id.clone(),
        agent_code: "PA".into(),
        status: "open".into(),
        question: "A cap".into(),
        ..Escalation::default()
    });
    t.engine.recover();
    assert_eq!(state(&t, &ws.id), WorkspaceState::Escalated);
    assert_eq!(t.store.escalation(&ws.id).unwrap().status, "open");
    t.engine.stop(&ws.id).await;
    assert_eq!(state(&t, &ws.id), WorkspaceState::Stopped);
    assert!(!t.store.has_open_escalation(&ws.id));
    assert!(t.events.has("escalation.resolved"));
    assert!(
        t.engine
            .answer_escalation(&ws.id, "", "keep going")
            .await
            .is_none()
    );
    assert!(
        !t.store
            .answer_escalation_for(&ws.id, "PA", "keep going", "")
            .1
    );
    let status = t.engine.restart(&ws.id).await;
    assert!(status.contains("restarting"), "{status}");
    t.close().await;
}

#[tokio::test]
async fn retire_walk_escalation_leaves_sibling_open() {
    let repo = init_repo();
    let t = TestEngine::new(None);
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "retire-sib", "")
        .await
        .unwrap();
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
    t.engine.retire_walk_escalation(&ws.id, "PA");
    assert!(t.events.has("escalation.resolved"));
    assert!(!t.store.answer_escalation_for(&ws.id, "PA", "later", "").1);
    let (got, ok) = t.store.answer_escalation_for(&ws.id, "PA2", "keep", "");
    assert!(ok);
    assert_eq!(got.unwrap().question, "B cap");
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_wakes_escalated_loop() {
    let repo = init_repo();
    let t = TestEngine::new(Some(never_converges(6).provider()));
    let id = started(&t, &repo.path, "shutdown while escalated").await;
    wait_state(&t, &id, WorkspaceState::Escalated, 15, "loop to escalate").await;
    shutdown_within(&t.engine).await;
    assert_eq!(
        state(&t, &id),
        WorkspaceState::Escalated,
        "the question survives quit"
    );
    assert_eq!(t.store.escalation(&id).unwrap().status, "open");

    let eng2 = t.reopen(Some(ConvergeRunner::new(&[0]).provider()));
    eng2.recover();
    eng2.auto_resume().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(state(&t, &id), WorkspaceState::Escalated);
    assert_eq!(
        eng2.governor().loops_running(),
        0,
        "still waiting on the human"
    );
    assert!(eng2.answer_escalation(&id, "Stop", "").await.is_some());
    wait_state(
        &t,
        &id,
        WorkspaceState::Stopped,
        10,
        "post-restart Stop to stop",
    )
    .await;
    wait_slots_free(&eng2, 5).await;
    eng2.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_during_shutdown_persists_stopped() {
    let repo = init_repo();
    let t = TestEngine::new(Some(never_converges(6).provider()));
    let id = started(&t, &repo.path, "stop during shutdown").await;
    wait_state(&t, &id, WorkspaceState::Escalated, 15, "loop to escalate").await;
    *t.engine.teardown.lock() = true;
    t.engine.stop(&id).await;
    assert_eq!(state(&t, &id), WorkspaceState::Stopped);
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_during_shutdown_preserves_pause() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[("p.txt", "x\n")])));
    let id = started(&t, &repo.path, "pause during shutdown").await;
    wait_for(Duration::from_secs(3), "agent running", || {
        t.engine.governor().agents_running() == 1
    })
    .await;
    *t.engine.teardown.lock() = true;
    t.engine.pause(&id);
    let got = t.store.workspace(&id).unwrap();
    assert_eq!(got.state, WorkspaceState::Paused);
    assert_ne!(got.phase, PHASE_INTERRUPTED);
    gate.cancel();
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn escalate_mcp_tool_raises_escalation() {
    let repo = init_repo();
    let runner = EscalatingRunner::new();
    let t = TestEngine::new(Some(runner.provider()));
    runner.bind(&t.engine);
    let id = started(&t, &repo.path, "agent escalates via the tool").await;
    wait_for(Duration::from_secs(10), "escalation.raised", || {
        t.events.has("escalation.raised")
    })
    .await;
    wait_state(
        &t,
        &id,
        WorkspaceState::Escalated,
        5,
        "loop to reach escalated",
    )
    .await;
    let esc = t.store.escalation(&id).expect("persisted");
    assert!(!esc.question.is_empty());
    assert!(t.engine.answer_escalation(&id, "Stop", "").await.is_some());
    wait_terminal(&t, &id, 10, "loop to finish").await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answer_escalation_idempotent_no_resurrect() {
    let repo = init_repo();
    let t = TestEngine::new(Some(never_converges(8).provider()));
    let proj = add_project(&t.store, &repo.path);
    let id = t
        .engine
        .start_workspace(
            &proj,
            "idempotent answer must not resurrect",
            "",
            AutonomyLevel::default(),
        )
        .await
        .unwrap();
    wait_state(&t, &id, WorkspaceState::Escalated, 15, "loop to escalate").await;
    assert_eq!(t.engine.governor().loops_running(), 1);
    let answered = t
        .engine
        .answer_escalation(&id, "Stop", "")
        .await
        .expect("first answer");
    assert_eq!(answered.status, "answered");
    wait_terminal(&t, &id, 10, "answered loop to settle").await;
    wait_slots_free(&t.engine, 5).await;
    assert_eq!(state(&t, &id), WorkspaceState::Stopped);

    let (_, ans_before) = t.engine.escalation_stats();
    assert!(
        t.engine
            .answer_escalation(&id, "Keep iterating", "")
            .await
            .is_none(),
        "a second answer is a no-op"
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(t.engine.governor().loops_running(), 0);
    assert_eq!(state(&t, &id), WorkspaceState::Stopped);
    assert!(t.engine.governor().try_acquire_loop(), "no leaked slot");
    t.engine.governor().release_loop();
    assert_eq!(t.engine.escalation_stats().1, ans_before);

    // Concurrent double answer: exactly one delivers.
    let id2 = t
        .engine
        .start_workspace(
            &proj,
            "concurrent double answer",
            "",
            AutonomyLevel::default(),
        )
        .await
        .unwrap();
    wait_state(
        &t,
        &id2,
        WorkspaceState::Escalated,
        15,
        "second loop to escalate",
    )
    .await;
    let (a, b) = tokio::join!(
        t.engine.answer_escalation(&id2, "Stop", ""),
        t.engine.answer_escalation(&id2, "Stop", "")
    );
    assert_eq!(usize::from(a.is_some()) + usize::from(b.is_some()), 1);
    wait_terminal(&t, &id2, 10, "concurrently answered loop to settle").await;
    wait_slots_free(&t.engine, 5).await;
    t.close().await;
}

fn chat_esc_runner(session: &str) -> Arc<dyn recur_agent::Provider> {
    scripted_with_edits(
        recur_provider_claude::testing::fixture_lines(session, "resume.txt", "ok", 0.05),
        &[("resume.txt", "resumed\n")],
    )
}

fn chat_owned(t: &TestEngine, proj: &str, id: &str, extra: impl FnOnce(&mut Workspace)) {
    let mut rec = Workspace {
        id: id.into(),
        project_id: proj.into(),
        group: Group::Needs,
        goal: "chat-owned escalation after crash".into(),
        state: WorkspaceState::Escalated,
        status_label: "Needs you".into(),
        phase: "asking a question".into(),
        loop_chat_code: "PA".into(),
        ..Workspace::default()
    };
    extra(&mut rec);
    t.store.add_workspace(rec).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answer_chat_owned_escalation_redispatches() {
    let repo = init_repo();
    let t = TestEngine::new(Some(chat_esc_runner("sess-esc-chat")));
    let proj = add_project(&t.store, &repo.path);
    let id = "LOOP-CHAT-ESC";
    chat_owned(&t, &proj, id, |_| {});
    t.store.upsert_agent(
        id,
        Agent {
            id: format!("{id}-PA"),
            workspace_id: id.into(),
            code: "PA".into(),
            name: "Agent".into(),
            ..Agent::default()
        },
    );
    t.store.add_escalation(Escalation {
        id: "esc-pa".into(),
        workspace_id: id.into(),
        agent_code: "PA".into(),
        status: "open".into(),
        question: "A or B?".into(),
        ..Escalation::default()
    });
    let answered = t
        .engine
        .answer_escalation_for(id, "PA", "Stop", "")
        .await
        .expect("answered");
    assert_eq!(answered.question, "A or B?");
    wait_terminal(&t, id, 8, "re-dispatched chat walk to settle").await;
    wait_slots_free(&t.engine, 5).await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answer_chat_owned_escalation_redispatches_answering_chat() {
    let repo = init_repo();
    let t = TestEngine::new(Some(chat_esc_runner("sess-esc-chat-pa2")));
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = "LOOP-CHAT-ESC-PA2";
    chat_owned(&t, &proj, id, |r| {
        r.goal = "owner goal".into();
        r.loop_id = "recur.loops.pr".into();
        r.checkpoint = Some(Checkpoint {
            phase_id: "planning".into(),
            ..Checkpoint::default()
        });
    });
    t.engine
        .stamp_chat_loop(id, "PA2", "test.loops.verify", "", "chat goal", "", true);
    t.store.add_escalation(Escalation {
        id: "esc-pa2".into(),
        workspace_id: id.into(),
        agent_code: "PA2".into(),
        status: "open".into(),
        question: "A or B?".into(),
        ..Escalation::default()
    });
    let answered = t
        .engine
        .answer_escalation_for(id, "PA2", "Stop", "")
        .await
        .expect("answered");
    assert_eq!(answered.question, "A or B?");
    let got = t.store.workspace(id).unwrap();
    assert_eq!(got.loop_chat_code, "PA2");
    assert_eq!(got.goal, "chat goal");
    assert_eq!(got.loop_id, "test.loops.verify");
    assert!(
        got.checkpoint
            .as_ref()
            .is_none_or(|c| c.phase_id != "planning")
    );
    wait_for(
        Duration::from_secs(8),
        "answering-chat walk to settle",
        || {
            t.store
                .workspace(id)
                .is_some_and(|l| terminal(&l.state) || l.phase == "phase missing")
        },
    )
    .await;
    assert_ne!(t.store.workspace(id).unwrap().phase, "phase missing");
    wait_slots_free(&t.engine, 5).await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answer_chat_owned_escalation_defers_when_slots_full() {
    let repo = init_repo();
    let t = TestEngine::new(Some(chat_esc_runner("sess-esc-chat-pa2-slot")));
    t.install_fixture_pack();
    let proj = add_project(&t.store, &repo.path);
    let id = "LOOP-CHAT-ESC-PA2-SLOT";
    chat_owned(&t, &proj, id, |r| {
        r.goal = "owner goal".into();
        r.loop_id = "recur.loops.pr".into();
        r.checkpoint = Some(Checkpoint {
            phase_id: "planning".into(),
            ..Checkpoint::default()
        });
    });
    t.engine
        .stamp_chat_loop(id, "PA2", "test.loops.verify", "", "chat goal", "", true);
    t.store.add_escalation(Escalation {
        id: "esc-pa2-slot".into(),
        workspace_id: id.into(),
        agent_code: "PA2".into(),
        status: "open".into(),
        question: "A or B?".into(),
        ..Escalation::default()
    });
    let mut slots = 0;
    while t.engine.governor().try_acquire_loop() {
        slots += 1;
    }
    let answered = t
        .engine
        .answer_escalation_for(id, "PA2", "Stop", "")
        .await
        .expect("answered");
    assert_eq!(answered.question, "A or B?");
    let got = t.store.workspace(id).unwrap();
    assert_eq!(got.loop_chat_code, "PA2");
    assert_eq!(got.goal, "chat goal");
    assert_eq!(got.loop_id, "test.loops.verify");
    assert!(
        got.checkpoint
            .as_ref()
            .is_none_or(|c| c.phase_id != "planning")
    );
    assert_eq!(got.state, WorkspaceState::Paused, "deferred resume");
    for _ in 0..slots {
        t.engine.governor().release_loop();
    }
    t.engine.resume(id).await;
    let got = t.store.workspace(id).unwrap();
    assert_eq!(got.loop_chat_code, "PA2");
    assert_eq!(got.goal, "chat goal");
    let lp = t
        .engine
        .running_loop_for_hand(id, "PA2")
        .expect("Resume dispatches PA2, not the persisted owner");
    assert_eq!(lp.chat_code(), "PA2");
    t.close().await;
}

#[tokio::test]
async fn answer_chat_owned_escalation_skips_terminal() {
    let repo = init_repo();
    let t = TestEngine::new(None);
    let proj = add_project(&t.store, &repo.path);
    t.store
        .add_workspace(Workspace {
            id: "LOOP-CHAT-ESC-TERM".into(),
            project_id: proj,
            state: WorkspaceState::Stopped,
            loop_chat_code: "PA".into(),
            ..Workspace::default()
        })
        .unwrap();
    t.store.add_escalation(Escalation {
        id: "esc-term".into(),
        workspace_id: "LOOP-CHAT-ESC-TERM".into(),
        agent_code: "PA".into(),
        status: "open".into(),
        question: "stale".into(),
        ..Escalation::default()
    });
    assert!(
        t.engine
            .answer_escalation_for("LOOP-CHAT-ESC-TERM", "PA", "Stop", "")
            .await
            .is_some()
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(t.engine.governor().loops_running(), 0);
    assert_eq!(state(&t, "LOOP-CHAT-ESC-TERM"), WorkspaceState::Stopped);
    t.close().await;
}

#[tokio::test]
async fn raise_escalation_publishes_notify() {
    let t = TestEngine::new(None);
    t.engine
        .raise_escalation("LOOP-X", "", &esc_req(), ESC_KIND_AGENT);
    assert!(t.events.has("notify"));
    assert_eq!(t.store.escalation("LOOP-X").unwrap().kind, ESC_KIND_AGENT);
}

#[tokio::test]
async fn each_loop_chat_question_has_its_own_inbox_row() {
    let t = TestEngine::new(None);
    t.engine
        .raise_escalation("LOOP-X", "PA", &esc_req(), ESC_KIND_AGENT);
    t.engine
        .raise_escalation("LOOP-X", "PA2", &esc_req(), ESC_KIND_AGENT);
    let targets = |t: &TestEngine| {
        let mut v: Vec<String> = t
            .store
            .inbox()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| i.id.starts_with("ib-esc-"))
            .map(|i| i.target)
            .collect();
        v.sort();
        v
    };
    assert_eq!(targets(&t), ["chat:LOOP-X:PA", "chat:LOOP-X:PA2"]);
    assert!(t.engine.retire_walk_escalation("LOOP-X", "PA"));
    assert_eq!(targets(&t), ["chat:LOOP-X:PA2"]);
}

fn opts(texts: &[&str]) -> Option<Vec<EscalationOption>> {
    Some(
        texts
            .iter()
            .map(|t| EscalationOption {
                text: (*t).into(),
                ..EscalationOption::default()
            })
            .collect(),
    )
}

#[test]
fn restore_escalation_answer_preserves_shape() {
    let cap = opts(&["Keep iterating", "Stop"]);
    let cases = [
        (
            "new free text",
            Escalation {
                answer: "use a mutex".into(),
                free_text: "use a mutex".into(),
                options: cap.clone(),
                ..Escalation::default()
            },
            CapAnswer::Guidance,
        ),
        (
            "new keep iterating",
            Escalation {
                answer: "Keep iterating".into(),
                choice: "Keep iterating".into(),
                options: cap.clone(),
                ..Escalation::default()
            },
            CapAnswer::KeepIterating,
        ),
        (
            "old free text",
            Escalation {
                answer: "use a mutex".into(),
                options: cap.clone(),
                ..Escalation::default()
            },
            CapAnswer::Guidance,
        ),
        (
            "old stop",
            Escalation {
                answer: "Stop".into(),
                options: cap,
                ..Escalation::default()
            },
            CapAnswer::Stop,
        ),
    ];
    for (name, esc, want) in cases {
        let ans = restore_escalation_answer(&esc).unwrap_or_else(|| panic!("{name}: no answer"));
        assert_eq!(classify_cap_answer(&ans), want, "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_free_text_cap_guidance() {
    let repo = init_repo();
    let t = TestEngine::new(Some(never_converges(6).provider()));
    let id = started(&t, &repo.path, "free-text cap after restart").await;
    wait_state(&t, &id, WorkspaceState::Escalated, 15, "loop to escalate").await;
    shutdown_within(&t.engine).await;
    let eng2 = t.reopen(Some(ConvergeRunner::new(&[0]).provider()));
    assert!(
        eng2.answer_escalation(&id, "", "try a mutex instead")
            .await
            .is_some()
    );
    wait_for(
        Duration::from_secs(10),
        "free-text guidance to continue",
        || {
            matches!(
                state(&t, &id),
                WorkspaceState::PrOpen | WorkspaceState::Stopped
            )
        },
    )
    .await;
    assert_eq!(
        state(&t, &id),
        WorkspaceState::PrOpen,
        "free text is guidance, not Stop"
    );
    eng2.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_plan_approval_notes() {
    let repo = init_repo();
    let t = TestEngine::new(Some(ConvergeRunner::new(&[0]).provider()));
    let proj = add_project(&t.store, &repo.path);
    let ws = t
        .engine
        .create_workspace(&proj, "plan-notes-restart", "")
        .await
        .unwrap();
    EngineHost::new(Arc::downgrade(&t.engine))
        .submit_plan(
            &ws.id,
            "PA",
            PlanSubmission {
                summary: "Wire the toggle".into(),
                steps: vec!["Edit settings".into()],
                ..PlanSubmission::default()
            },
        )
        .await;
    let queued = t
        .engine
        .start_run_on_workspace(&ws.id, "ship the toggle")
        .await
        .expect("start run");
    assert!(!queued);
    wait_state(
        &t,
        &ws.id,
        WorkspaceState::Escalated,
        5,
        "plan-approval escalation",
    )
    .await;
    shutdown_within(&t.engine).await;

    let notes = "also persist in localStorage";
    let eng2 = t.reopen(Some(ConvergeRunner::new(&[0]).provider()));
    assert!(
        eng2.answer_escalation(&ws.id, "Proceed with my notes", notes)
            .await
            .is_some()
    );
    wait_for(
        Duration::from_secs(10),
        "plan-approval run to settle",
        || {
            matches!(
                state(&t, &ws.id),
                WorkspaceState::PrOpen | WorkspaceState::Stopped
            )
        },
    )
    .await;
    let got = t.store.workspace(&ws.id).unwrap();
    assert_eq!(got.state, WorkspaceState::PrOpen);
    assert!(
        got.definition_of_done.contains(notes),
        "{}",
        got.definition_of_done
    );
    eng2.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn answered_escalation_survives_delayed_dispatch() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[("hold.txt", "x\n")])));
    t.engine.governor().set_caps(1, 8);
    let proj = add_project(&t.store, &repo.path);
    let id_a = t
        .engine
        .start_workspace(&proj, "holder", "", AutonomyLevel::default())
        .await
        .unwrap();
    wait_for(Duration::from_secs(3), "holder running", || {
        t.engine.governor().loops_running() == 1
    })
    .await;
    let rec = Workspace {
        id: "LOOP-DELAY-ANS".into(),
        project_id: proj.clone(),
        goal: "answered while capped".into(),
        state: WorkspaceState::Escalated,
        phase: "asking a question".into(),
        autonomy: AutonomyLevel::Balanced,
        base_branch: "main".into(),
        checkpoint: Some(Checkpoint {
            state: WorkspaceState::Coding,
            iteration: 4,
            ..Checkpoint::default()
        }),
        ..Workspace::default()
    };
    t.store.add_workspace(rec.clone()).unwrap();
    t.store.add_escalation(Escalation {
        workspace_id: rec.id.clone(),
        question: "I've run 4 iteration(s) without converging".into(),
        options: opts(&["Keep iterating", "Stop"]),
        status: "open".into(),
        ..Escalation::default()
    });
    assert!(
        t.engine
            .answer_escalation(&rec.id, "Stop", "")
            .await
            .is_some()
    );
    assert_eq!(state(&t, &rec.id), WorkspaceState::Paused);
    assert!(!t.store.escalation(&rec.id).unwrap().applied);
    gate.cancel();
    wait_state(&t, &id_a, WorkspaceState::PrOpen, 5, "holder to finish").await;
    wait_slots_free(&t.engine, 3).await;
    t.engine.resume(&rec.id).await;
    wait_state(
        &t,
        &rec.id,
        WorkspaceState::Stopped,
        10,
        "delayed Stop to apply",
    )
    .await;
    t.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_agent_escalation_does_not_swallow_next() {
    let repo = init_repo();
    let runner = EscalatingRunner::new();
    let t = TestEngine::new(Some(runner.provider()));
    runner.bind(&t.engine);
    let id = started(&t, &repo.path, "agent esc then another").await;
    wait_for(Duration::from_secs(10), "escalation.raised", || {
        t.events.has("escalation.raised")
    })
    .await;
    wait_state(&t, &id, WorkspaceState::Escalated, 5, "loop to escalate").await;
    shutdown_within(&t.engine).await;

    let runner2 = EscalatingRunner::new();
    let eng2 = t.reopen(Some(runner2.provider()));
    runner2.bind(&eng2);
    assert!(
        eng2.answer_escalation(&id, "Approach A", "")
            .await
            .is_some()
    );
    wait_state(
        &t,
        &id,
        WorkspaceState::Escalated,
        10,
        "second escalate after restart",
    )
    .await;
    let esc = t.store.escalation(&id).unwrap();
    assert_eq!(esc.status, "open", "the next escalate() was not swallowed");
    eng2.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_then_immediate_shutdown_preserves_pause() {
    let repo = init_repo();
    let gate = CancellationToken::new();
    let t = TestEngine::new(Some(blocking_runner(gate.clone(), &[("p.txt", "x\n")])));
    let id = started(&t, &repo.path, "pause then quit").await;
    wait_for(Duration::from_secs(3), "agent running", || {
        t.engine.governor().agents_running() == 1
    })
    .await;
    t.engine.pause(&id);
    tokio::time::timeout(Duration::from_secs(8), t.engine.shutdown())
        .await
        .expect("Shutdown hung after Pause");
    let got = t.store.workspace(&id).unwrap();
    assert_eq!(got.state, WorkspaceState::Paused);
    assert_ne!(got.phase, PHASE_INTERRUPTED);
    let eng2 = t.reopen(Some(ConvergeRunner::new(&[0]).provider()));
    eng2.recover();
    eng2.auto_resume().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        state(&t, &id),
        WorkspaceState::Paused,
        "AutoResume leaves a human pause"
    );
    gate.cancel();
    eng2.shutdown().await;
}

#[test]
fn ensure_pr_door_escalation_rules() {
    assert!(ensure_pr_door_escalation(&Escalation {
        question: "Policy bar blocked the PR: 1 countable finding. How should I proceed?".into(),
        ..Escalation::default()
    }));
    assert!(ensure_pr_door_escalation(&Escalation {
        options: Some(vec![EscalationOption {
            text: "Ship with caveats".into(),
            desc: "Open the PR anyway, noting remaining findings.".into(),
            ..EscalationOption::default()
        }]),
        ..Escalation::default()
    }));
    assert!(!ensure_pr_door_escalation(&Escalation {
        question: "I've run 4 iteration(s) without converging: 1 open finding remain. How should I proceed?".into(),
        options: Some(vec![EscalationOption {
            text: "Ship with caveats".into(),
            desc: "Open the PR noting remaining risk (rails currently allow it).".into(),
            ..EscalationOption::default()
        }]),
        ..Escalation::default()
    }));
}

fn graph(phases: Vec<LoopPhase>, policy: Policy) -> ResolvedGuard {
    ResolvedGuard {
        loop_def: LoopDef {
            phases,
            ..LoopDef::default()
        },
        policy,
        ..ResolvedGuard::default()
    }
}

fn ph(id: &str, uses: &str, next: &str) -> LoopPhase {
    LoopPhase {
        id: id.into(),
        uses: uses.into(),
        next: next.into(),
        ..LoopPhase::default()
    }
}

fn door_opts() -> Option<Vec<EscalationOption>> {
    Some(vec![
        EscalationOption {
            text: "Keep iterating".into(),
            ..EscalationOption::default()
        },
        EscalationOption {
            text: "Change approach".into(),
            ..EscalationOption::default()
        },
        EscalationOption {
            text: "Ship with caveats".into(),
            desc: "Open the PR anyway, noting remaining findings.".into(),
            ..EscalationOption::default()
        },
        EscalationOption {
            text: "Stop".into(),
            ..EscalationOption::default()
        },
    ])
}

fn cap_opts() -> Option<Vec<EscalationOption>> {
    Some(vec![
        EscalationOption {
            text: "Keep iterating".into(),
            ..EscalationOption::default()
        },
        EscalationOption {
            text: "Ship with caveats".into(),
            desc: "Open the PR noting remaining risk (rails currently allow it).".into(),
            ..EscalationOption::default()
        },
        EscalationOption {
            text: "Stop".into(),
            ..EscalationOption::default()
        },
    ])
}

const DOOR_Q: &str = "Policy bar blocked the PR: 1 countable finding: policy bar blocks opening a PR. How should I proceed?";
const CAP_Q: &str =
    "I've run 4 iteration(s) without converging: 1 open finding remain. How should I proceed?";

#[allow(clippy::too_many_arguments)]
fn seed_answered(
    t: &TestEngine,
    id: &str,
    goal: &str,
    cp_phase: &str,
    sev: Severity,
    choice: &str,
    free: &str,
    question: &str,
    options: Option<Vec<EscalationOption>>,
    kind: &str,
    guard: ResolvedGuard,
) -> Walk {
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            goal: goal.into(),
            state: WorkspaceState::Paused,
            autonomy: AutonomyLevel::Balanced,
            checkpoint: Some(Checkpoint {
                phase_id: cp_phase.into(),
                iteration: 4,
                state: WorkspaceState::Escalated,
                ..Checkpoint::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.store.add_finding(Finding {
        id: format!("f-{id}"),
        workspace_id: id.into(),
        severity: sev,
        status: "open".into(),
        message: "still open".into(),
        file: "a.go".into(),
        ..Finding::default()
    });
    t.store.add_escalation(Escalation {
        workspace_id: id.into(),
        question: question.into(),
        options,
        status: "answered".into(),
        choice: choice.into(),
        free_text: free.into(),
        answer: format!("{free} {choice}").trim().to_owned(),
        kind: kind.into(),
        ..Escalation::default()
    });
    let rec = t.store.workspace(id).unwrap();
    let mut l = Walk::new(&t.engine, &rec);
    l.set_guard(guard);
    l
}

#[tokio::test]
async fn apply_answered_cap_ensure_pr_after_restart() {
    let t = TestEngine::new(None);
    let guard = || {
        graph(
            vec![
                ph("converge", VERB_CONVERGE, "pr"),
                ph("pr", VERB_ENSURE_PR, ""),
            ],
            Policy {
                extend_rounds: 2,
                ..Policy::default()
            },
        )
    };
    let cancel = CancellationToken::new();
    let kind = |q: &str| {
        if q.starts_with("Policy bar blocked the PR:") {
            ESC_KIND_ENSURE_PR
        } else {
            ESC_KIND_CAP
        }
    };

    // ship opens PR despite open findings
    let mut l = seed_answered(
        &t,
        "WS-ens-ship",
        "ensure-pr restart",
        "pr",
        Severity::High,
        "Ship with caveats",
        "",
        DOOR_Q,
        door_opts(),
        kind(DOOR_Q),
        guard(),
    );
    assert!(l.apply_answered_cap(&cancel).await, "ship finishes");
    assert_eq!(state(&t, "WS-ens-ship"), WorkspaceState::PrOpen);

    // keep iterating returns inbound
    let mut l = seed_answered(
        &t,
        "WS-ens-ki",
        "ensure-pr restart",
        "pr",
        Severity::High,
        "Keep iterating",
        "",
        DOOR_Q,
        door_opts(),
        kind(DOOR_Q),
        guard(),
    );
    assert!(!l.apply_answered_cap(&cancel).await);
    let ws = t.store.workspace("WS-ens-ki").unwrap();
    assert_eq!(ws.checkpoint.unwrap().phase_id, "converge");
    assert_eq!(l.h.lock().granted_extra, 0, "inbound, not a cap grant");

    // guidance returns inbound
    let mut l = seed_answered(
        &t,
        "WS-ens-guide",
        "ensure-pr restart",
        "pr",
        Severity::High,
        "",
        "fix the timeout first",
        DOOR_Q,
        door_opts(),
        kind(DOOR_Q),
        guard(),
    );
    assert!(!l.apply_answered_cap(&cancel).await);
    assert_eq!(
        t.store
            .workspace("WS-ens-guide")
            .unwrap()
            .checkpoint
            .unwrap()
            .phase_id,
        "converge"
    );

    // round-cap ship still respects the bar
    let mut l = seed_answered(
        &t,
        "WS-cap-ship",
        "ensure-pr restart",
        "pr",
        Severity::High,
        "Ship with caveats",
        "",
        CAP_Q,
        cap_opts(),
        kind(CAP_Q),
        guard(),
    );
    assert!(l.apply_answered_cap(&cancel).await);
    assert_eq!(state(&t, "WS-cap-ship"), WorkspaceState::Stopped);

    // ship follows ensure_pr next
    let next = graph(
        vec![
            ph("converge", VERB_CONVERGE, "pr"),
            ph("pr", VERB_ENSURE_PR, "docs"),
            ph("docs", VERB_ARTIFACT, ""),
        ],
        Policy {
            extend_rounds: 2,
            ..Policy::default()
        },
    );
    let mut l = seed_answered(
        &t,
        "WS-ens-ship-next",
        "ensure-pr restart",
        "pr",
        Severity::High,
        "Ship with caveats",
        "",
        DOOR_Q,
        door_opts(),
        kind(DOOR_Q),
        next,
    );
    assert!(!l.apply_answered_cap(&cancel).await);
    let ws = t.store.workspace("WS-ens-ship-next").unwrap();
    assert_eq!(ws.state, WorkspaceState::PrOpen);
    assert_eq!(ws.checkpoint.unwrap().phase_id, "docs");
    t.close().await;
}

#[tokio::test]
async fn apply_answered_cap_bundle_cap_ship_follows_next() {
    let t = TestEngine::new(None);
    let cancel = CancellationToken::new();
    let mut l = seed_answered(
        &t,
        "WS-docs-cap-next",
        "docs cap ship",
        "converge",
        Severity::Low,
        "Ship with caveats",
        "",
        CAP_Q,
        cap_opts(),
        ESC_KIND_CAP,
        graph(
            vec![
                ph("converge", VERB_CONVERGE, "done"),
                ph("done", VERB_ARTIFACT, ""),
            ],
            Policy::default(),
        ),
    );
    assert!(!l.apply_answered_cap(&cancel).await);
    let ws = t.store.workspace("WS-docs-cap-next").unwrap();
    assert_ne!(ws.state, WorkspaceState::PrOpen);
    assert_eq!(ws.checkpoint.unwrap().phase_id, "done");

    let mut l = seed_answered(
        &t,
        "WS-docs-cap-idle",
        "docs cap ship",
        "converge",
        Severity::Low,
        "Ship with caveats",
        "",
        CAP_Q,
        cap_opts(),
        ESC_KIND_CAP,
        graph(vec![ph("converge", VERB_CONVERGE, "")], Policy::default()),
    );
    assert!(l.apply_answered_cap(&cancel).await);
    assert_eq!(state(&t, "WS-docs-cap-idle"), WorkspaceState::Paused);
    t.close().await;
}

fn seed_kind(t: &TestEngine, id: &str, kind: &str, choice: &str, options: &[&str]) -> Walk {
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            goal: "kind collision".into(),
            state: WorkspaceState::Paused,
            autonomy: AutonomyLevel::Balanced,
            ..Workspace::default()
        })
        .unwrap();
    t.store.add_escalation(Escalation {
        workspace_id: id.into(),
        question: "Which path?".into(),
        options: opts(options),
        status: "answered".into(),
        choice: choice.into(),
        answer: choice.into(),
        kind: kind.into(),
        ..Escalation::default()
    });
    Walk::new(&t.engine, &t.store.workspace(id).unwrap())
}

#[tokio::test]
async fn apply_answered_cap_uses_persisted_kind_not_option_text() {
    let t = TestEngine::new(None);
    let cancel = CancellationToken::new();
    let mut l = seed_kind(
        &t,
        "WS-kind-cap",
        ESC_KIND_AGENT,
        "Refactor first",
        &["Keep iterating", "Refactor first"],
    );
    assert!(!l.apply_answered_cap(&cancel).await);
    assert_ne!(state(&t, "WS-kind-cap"), WorkspaceState::Stopped);
    assert_eq!(l.h.lock().pending_steer, "Refactor first");

    let mut l = seed_kind(
        &t,
        "WS-kind-plan",
        ESC_KIND_AGENT,
        "Keep iterating",
        &["Approve and proceed", "Keep iterating"],
    );
    assert!(!l.apply_answered_cap(&cancel).await);
    assert_eq!(l.h.lock().pending_steer, "Keep iterating");
    assert_eq!(l.h.lock().granted_extra, 0);
    t.close().await;
}

#[tokio::test]
async fn new_loop_does_not_replay_legacy_answered_escalation() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "WS-legacy-applied".into(),
            goal: "already consumed".into(),
            state: WorkspaceState::Paused,
            phase: PHASE_INTERRUPTED.into(),
            autonomy: AutonomyLevel::Balanced,
            ..Workspace::default()
        })
        .unwrap();
    let esc: Escalation = serde_json::from_str(
        r#"{"workspaceId":"WS-legacy-applied","status":"answered","answer":"Keep iterating","options":[{"text":"Keep iterating"},{"text":"Stop"}]}"#,
    )
    .unwrap();
    assert!(esc.applied, "legacy answered JSON decodes as applied");
    t.store.add_escalation(esc);
    let l = Walk::new(&t.engine, &t.store.workspace("WS-legacy-applied").unwrap());
    assert!(l.h.lock().answer.is_none());
}

#[tokio::test]
async fn new_loop_seeds_legacy_stuck_answered_escalation() {
    let t = TestEngine::new(None);
    let mut esc: Escalation = serde_json::from_str(
        r#"{"workspaceId":"WS-legacy-stuck","status":"answered","answer":"Approach A"}"#,
    )
    .unwrap();
    assert!(esc.applied);
    for (id, st, phase) in [
        (
            "WS-legacy-stuck-esc",
            WorkspaceState::Escalated,
            "asking a question",
        ),
        (
            "WS-legacy-stuck-wait",
            WorkspaceState::Paused,
            "answered: waiting to resume",
        ),
    ] {
        t.store
            .add_workspace(Workspace {
                id: id.into(),
                goal: "pending".into(),
                state: st,
                phase: phase.into(),
                autonomy: AutonomyLevel::Balanced,
                ..Workspace::default()
            })
            .unwrap();
        esc.workspace_id = id.into();
        t.store.add_escalation(esc.clone());
        let l = Walk::new(&t.engine, &t.store.workspace(id).unwrap());
        let ans = l.h.lock().answer.clone();
        assert_eq!(ans.map(|a| a.resolved), Some("Approach A".into()), "{id}");
    }
}

#[tokio::test]
async fn apply_answered_cap_agent_steer_stays_pending_until_prompt() {
    let t = TestEngine::new(None);
    let id = "WS-steer-pending";
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            goal: "steer".into(),
            state: WorkspaceState::Paused,
            autonomy: AutonomyLevel::Balanced,
            ..Workspace::default()
        })
        .unwrap();
    t.store.add_escalation(Escalation {
        workspace_id: id.into(),
        question: "Which path?".into(),
        status: "answered".into(),
        choice: "Refactor first".into(),
        answer: "Refactor first".into(),
        kind: ESC_KIND_AGENT.into(),
        ..Escalation::default()
    });
    let mut l = Walk::new(&t.engine, &t.store.workspace(id).unwrap());
    assert!(!l.apply_answered_cap(&CancellationToken::new()).await);
    assert!(!t.store.escalation(id).unwrap().applied);
    let got = l.apply_pending_steer("do the work");
    assert!(got.contains("Refactor first"), "{got}");
    assert!(t.store.escalation(id).unwrap().applied);
}

#[tokio::test]
async fn checkpoint_agent_answer_reaches_next_prompt() {
    let t = TestEngine::new(None);
    let id = "WS-live-agent-answer";
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            goal: "live answer".into(),
            state: WorkspaceState::Coding,
            autonomy: AutonomyLevel::Balanced,
            ..Workspace::default()
        })
        .unwrap();
    let mut l = Walk::new(&t.engine, &t.store.workspace(id).unwrap());
    l.h.request_escalation(esc_req());
    let h = l.h.clone();
    let store = t.store.clone();
    let answer = async move {
        wait_for(Duration::from_secs(5), "agent escalation", || {
            h.lock().escalated
        })
        .await;
        store.answer_escalation_for(id, "", "", "Use approach B");
        h.send(crate::loop_handle::Command::AnswerEscalation(
            crate::loop_handle::EscalationAnswer {
                free_text: "Use approach B".into(),
                ..Default::default()
            },
        ));
    };
    let cancel = CancellationToken::new();
    let (keep_going, ()) = tokio::join!(l.checkpoint(&cancel), answer);
    assert!(keep_going);
    assert!(!t.store.escalation(id).unwrap().applied);
    let got = l.apply_pending_steer("do the work");
    assert!(got.contains("Use approach B"), "{got}");
    assert!(t.store.escalation(id).unwrap().applied);
}

fn seed_cap_order(t: &TestEngine, id: &str, choice: &str) -> Walk {
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            goal: "cap apply order".into(),
            state: WorkspaceState::Coding,
            autonomy: AutonomyLevel::Balanced,
            ..Workspace::default()
        })
        .unwrap();
    t.store.add_escalation(Escalation {
        workspace_id: id.into(),
        question: "How should I proceed?".into(),
        options: opts(&["Keep iterating", "Ship with caveats", "Stop"]),
        status: "answered".into(),
        choice: choice.into(),
        answer: choice.into(),
        kind: ESC_KIND_CAP.into(),
        ..Escalation::default()
    });
    Walk::new(&t.engine, &t.store.workspace(id).unwrap())
}

fn feed_text(ev: &EngineEvent) -> Option<String> {
    match ev {
        EngineEvent::WorkspaceEvent(w) => Some(w.event.text.clone()),
        _ => None,
    }
}

#[tokio::test]
async fn cap_answer_applied_after_durable_outcome() {
    let t = TestEngine::new(None);
    let cancel = CancellationToken::new();

    // stop persists before the feed
    let mut l = seed_cap_order(&t, "WS-cap-stop-order", "Stop");
    let state_at_feed = Arc::new(parking_lot::Mutex::new(WorkspaceState::default()));
    {
        let (st, store) = (Arc::clone(&state_at_feed), Arc::clone(&t.store));
        t.engine.bus.subscribe_fn(move |ev| {
            if feed_text(ev).is_some_and(|x| x.contains("Stopping per your decision")) {
                *st.lock() = store
                    .workspace("WS-cap-stop-order")
                    .map(|w| w.state)
                    .unwrap_or_default();
            }
        });
    }
    let (out, more) = l.escalate_at_cap_apply_msg(&cancel, 4, 1, "", false).await;
    assert_eq!((out, more), (WorkspaceState::Stopped, 0));
    assert_eq!(*state_at_feed.lock(), WorkspaceState::Stopped);
    assert!(t.store.escalation("WS-cap-stop-order").unwrap().applied);

    // keep iterating stays pending after the helper
    let mut l = seed_cap_order(&t, "WS-cap-ki-order", "Keep iterating");
    let applied_at_feed = Arc::new(AtomicBool::new(true));
    {
        let (a, store) = (Arc::clone(&applied_at_feed), Arc::clone(&t.store));
        t.engine.bus.subscribe_fn(move |ev| {
            if feed_text(ev).is_some_and(|x| x.contains("keep iterating")) {
                a.store(
                    store
                        .escalation("WS-cap-ki-order")
                        .is_some_and(|e| e.applied),
                    Ordering::SeqCst,
                );
            }
        });
    }
    let (_, more) = l.escalate_at_cap_apply_msg(&cancel, 4, 1, "", false).await;
    assert!(more > 0);
    assert!(!applied_at_feed.load(Ordering::SeqCst));
    assert!(!t.store.escalation("WS-cap-ki-order").unwrap().applied);

    // ship stays pending after the helper
    let mut l = seed_cap_order(&t, "WS-cap-ship-order", "Ship with caveats");
    let (out, more) = l.escalate_at_cap_apply_msg(&cancel, 4, 1, "", true).await;
    assert_eq!((out, more), (WorkspaceState::PrOpen, 0));
    assert!(!t.store.escalation("WS-cap-ship-order").unwrap().applied);
}

#[tokio::test]
async fn ensure_pr_keep_iterating_survives_shutdown_during_feed() {
    let t = TestEngine::new(None);
    let lv = std::collections::BTreeMap::from([
        ("high".to_owned(), 0),
        ("critical".to_owned(), 0),
        ("medium".to_owned(), 0),
    ]);
    let guard = graph(
        vec![
            ph("converge", VERB_CONVERGE, "pr"),
            ph("pr", VERB_ENSURE_PR, ""),
        ],
        Policy {
            extend_rounds: 2,
            findings_threshold: FindingsThreshold {
                all_zero: false,
                levels: lv,
            },
            ..Policy::default()
        },
    );
    let id = "WS-ens-ki-quit";
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            goal: "ensure-pr quit".into(),
            state: WorkspaceState::Coding,
            autonomy: AutonomyLevel::Balanced,
            checkpoint: Some(Checkpoint {
                phase_id: "pr".into(),
                iteration: 4,
                state: WorkspaceState::Coding,
                ..Checkpoint::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.store.add_finding(Finding {
        id: format!("f-{id}"),
        workspace_id: id.into(),
        severity: Severity::High,
        status: "open".into(),
        message: "still open".into(),
        file: "a.go".into(),
        ..Finding::default()
    });
    t.store.add_escalation(Escalation {
        workspace_id: id.into(),
        question: "Policy bar blocked the PR: 1 countable finding. How should I proceed?".into(),
        options: door_opts(),
        status: "answered".into(),
        choice: "Keep iterating".into(),
        answer: "Keep iterating".into(),
        kind: ESC_KIND_ENSURE_PR.into(),
        ..Escalation::default()
    });
    let mut l = Walk::new(&t.engine, &t.store.workspace(id).unwrap());
    l.set_guard(guard.clone());
    let cancel = CancellationToken::new();
    let violated = Arc::new(AtomicBool::new(false));
    {
        let (c, store, v) = (cancel.clone(), Arc::clone(&t.store), Arc::clone(&violated));
        t.engine.bus.subscribe_fn(move |ev| {
            if feed_text(ev).is_some_and(|x| x.to_lowercase().contains("keep iterating")) {
                if store.escalation(id).is_some_and(|e| e.applied) {
                    v.store(true, Ordering::SeqCst);
                }
                c.cancel();
            }
        });
    }
    *t.engine.teardown.lock() = true;
    let (_, done) = l
        .run_loop_phase(&cancel, &ph("pr", VERB_ENSURE_PR, ""))
        .await;
    assert!(done);
    assert!(
        !violated.load(Ordering::SeqCst),
        "Applied during the keep-iterating feed"
    );
    let ws = t.store.workspace(id).unwrap();
    assert_eq!(ws.checkpoint.as_ref().unwrap().phase_id, "converge");
    assert_ne!(
        ws.state,
        WorkspaceState::Stopped,
        "a shutdown cancel is not a Stop"
    );

    let eng2 = t.reopen(None);
    eng2.recover();
    let mut l2 = Walk::new(&eng2, &t.store.workspace(id).unwrap());
    l2.set_guard(guard);
    assert!(
        l2.h.lock().answer.is_none(),
        "an applied continuation must not re-seed"
    );
    assert!(!l2.apply_answered_cap(&CancellationToken::new()).await);
    assert_eq!(
        t.store.workspace(id).unwrap().checkpoint.unwrap().phase_id,
        "converge"
    );
    eng2.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn round_cap_keep_iterating_survives_cancel_and_second_restart() {
    let t = TestEngine::new(None);
    let guard = graph(
        vec![
            ph("converge", VERB_CONVERGE, "done"),
            ph("done", VERB_ARTIFACT, ""),
        ],
        Policy {
            extend_rounds: 2,
            max_rounds: 4,
            ..Policy::default()
        },
    );
    let id = "WS-cap-ki-quit";
    t.store
        .add_workspace(Workspace {
            id: id.into(),
            goal: "round-cap quit".into(),
            state: WorkspaceState::Coding,
            autonomy: AutonomyLevel::Balanced,
            checkpoint: Some(Checkpoint {
                phase_id: "converge".into(),
                iteration: 4,
                state: WorkspaceState::Coding,
                ..Checkpoint::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    t.store.add_finding(Finding {
        id: format!("f-{id}"),
        workspace_id: id.into(),
        severity: Severity::Low,
        status: "open".into(),
        message: "still open".into(),
        file: "a.go".into(),
        ..Finding::default()
    });
    t.store.add_escalation(Escalation {
        workspace_id: id.into(),
        question: CAP_Q.into(),
        options: opts(&["Keep iterating", "Ship with caveats", "Stop"]),
        status: "answered".into(),
        choice: "Keep iterating".into(),
        answer: "Keep iterating".into(),
        kind: ESC_KIND_CAP.into(),
        ..Escalation::default()
    });
    let mut l = Walk::new(&t.engine, &t.store.workspace(id).unwrap());
    l.set_guard(guard.clone());
    assert!(!l.apply_answered_cap(&CancellationToken::new()).await);
    assert!(!t.store.escalation(id).unwrap().applied);
    assert!(l.h.lock().granted_extra > 0);

    t.engine.apply_governor_caps(DEFAULT_MAX_LOOPS, 1).await;
    let held = t
        .engine
        .acquire_agent(&CancellationToken::new())
        .await
        .expect("occupy the agent slot");
    let cancel = CancellationToken::new();
    let eng = Arc::clone(&t.engine);
    let run = {
        let c = cancel.clone();
        tokio::spawn(async move {
            let out = l.converge(&c).await;
            drop(eng);
            out
        })
    };
    wait_for(
        Duration::from_secs(5),
        "converge blocked on the agent slot",
        || t.engine.governor().agents_waiting() > 0,
    )
    .await;
    assert!(
        !t.store.escalation(id).unwrap().applied,
        "pending while waiting for a slot"
    );
    cancel.cancel();
    let out = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("converge returns after cancel")
        .unwrap();
    assert_eq!(
        out.err().map(|e| e.to_string()),
        Some(crate::loops::CONTEXT_CANCELED.to_owned())
    );
    assert!(!t.store.escalation(id).unwrap().applied);
    drop(held);

    for _ in 0..2 {
        let eng_n = t.reopen(None);
        eng_n.recover();
        let mut ln = Walk::new(&eng_n, &t.store.workspace(id).unwrap());
        ln.set_guard(guard.clone());
        assert!(
            ln.h.lock().answer.is_some(),
            "a pending grant re-seeds Keep iterating"
        );
        assert!(!ln.apply_answered_cap(&CancellationToken::new()).await);
        assert!(!t.store.escalation(id).unwrap().applied);
        assert!(ln.h.lock().granted_extra > 0);
        eng_n.shutdown().await;
    }
}

#[tokio::test]
async fn structured_cap_answer_applied_after_earlier_steer() {
    let t = TestEngine::new(None);
    let id = "WS-docs-steer-then-ship";
    let mut l = seed_answered(
        &t,
        id,
        "docs ship after steer",
        "converge",
        Severity::Low,
        "Ship with caveats",
        "",
        CAP_Q,
        cap_opts(),
        ESC_KIND_CAP,
        graph(
            vec![
                ph("converge", VERB_CONVERGE, "done"),
                ph("done", VERB_ARTIFACT, ""),
            ],
            Policy::default(),
        ),
    );
    l.h.queue_steer("Fix the heading first");
    assert!(!l.apply_answered_cap(&CancellationToken::new()).await);
    assert!(
        t.store.escalation(id).unwrap().applied,
        "structured ship marks Applied"
    );
    assert_eq!(
        t.store.workspace(id).unwrap().checkpoint.unwrap().phase_id,
        "done"
    );
    let eng2 = t.reopen(None);
    eng2.recover();
    let l2 = Walk::new(&eng2, &t.store.workspace(id).unwrap());
    assert!(
        l2.h.lock().answer.is_none(),
        "an applied ship must not re-seed"
    );
    eng2.shutdown().await;
}
