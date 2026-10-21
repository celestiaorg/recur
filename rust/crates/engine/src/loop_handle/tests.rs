//! The loop control plane's contracts (Go `loop.go` send / checkpoint wait /
//! escalation / steer / done-summary rules).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use recur_agent_tools::{EscalationRequest, Evidence};
use tokio_util::sync::CancellationToken;

use super::*;

fn handle() -> Arc<LoopHandle> {
    LoopHandle::new("w", "p", "w", LoopState::default())
}

#[tokio::test]
async fn pause_blocks_until_resume() {
    let h = handle();
    let cancel = CancellationToken::new();
    h.send(Command::Pause);
    assert!(h.is_paused());
    let h2 = Arc::clone(&h);
    let c2 = cancel.clone();
    let waiter = tokio::spawn(async move { h2.wait_resumed(&c2).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!waiter.is_finished(), "paused walk must wait");
    h.send(Command::Resume);
    let go_on = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("resume did not wake the walk")
        .unwrap();
    assert!(go_on);
}

#[tokio::test]
async fn stop_wins_over_pause_and_escalation() {
    let h = handle();
    let cancel = CancellationToken::new();
    h.send(Command::Pause);
    h.lock().escalated = true;
    let h2 = Arc::clone(&h);
    let c2 = cancel.clone();
    let waiter = tokio::spawn(async move { h2.wait_resumed(&c2).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    h.send(Command::Stop);
    let go_on = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("stop did not wake the walk")
        .unwrap();
    assert!(!go_on);
    let s = h.lock();
    assert!(s.stopped && !s.paused && !s.escalated);
}

#[tokio::test]
async fn cancel_wakes_a_paused_walk() {
    let h = handle();
    let cancel = CancellationToken::new();
    h.send(Command::Pause);
    let h2 = Arc::clone(&h);
    let c2 = cancel.clone();
    let waiter = tokio::spawn(async move { h2.wait_resumed(&c2).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    let go_on = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("cancel did not wake the walk")
        .unwrap();
    assert!(!go_on);
}

#[tokio::test]
async fn answer_clears_escalation_and_pause() {
    let h = handle();
    {
        let mut s = h.lock();
        s.escalated = true;
        s.paused = true;
    }
    let cancel = CancellationToken::new();
    let h2 = Arc::clone(&h);
    let c2 = cancel.clone();
    let waiter = tokio::spawn(async move {
        h2.wait_while(&c2, |s| s.escalated && !s.stopped).await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let ans = EscalationAnswer {
        choice: "Keep going".into(),
        resolved: "Keep going".into(),
        ..EscalationAnswer::default()
    };
    h.send(Command::AnswerEscalation(ans.clone()));
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("answer did not wake the walk")
        .unwrap();
    let s = h.lock();
    assert_eq!(s.answer.as_ref(), Some(&ans));
    assert!(!s.escalated && !s.paused);
}

#[test]
fn commands_queue_without_blocking() {
    let h = handle();
    for _ in 0..20 {
        h.send(Command::Resume);
    }
    h.drain_commands();
    h.send(Command::Pause);
    assert!(h.is_paused());
}

#[test]
fn first_escalation_wins() {
    let h = handle();
    let req = |q: &str| EscalationRequest {
        question: q.into(),
        ..EscalationRequest::default()
    };
    h.request_escalation(req("first"));
    h.request_escalation(req("second"));
    assert!(h.has_pending_escalation());
    assert_eq!(h.take_pending_escalation().unwrap().question, "first");
    assert!(h.take_pending_escalation().is_none());
    h.lock().escalated = true;
    h.request_escalation(req("while blocked"));
    assert!(
        !h.has_pending_escalation(),
        "an active escalation ignores new ones"
    );
}

#[test]
fn grants_accumulate_and_drain() {
    let h = handle();
    h.grant_extension(2);
    h.grant_extension(1);
    assert_eq!(h.take_granted_extension(), 3);
    assert_eq!(h.take_granted_extension(), 0);
}

#[tokio::test]
async fn steer_reaches_the_in_flight_specialist() {
    let h = handle();
    h.queue_steer("   ");
    assert_eq!(h.lock().pending_steer, "");
    let (mut inbox, binding) = h.bind_inject("CO");
    assert!(h.holds_done_turn("CO"));
    h.queue_steer(" switch to plan B ");
    assert_eq!(h.lock().pending_steer, "switch to plan B");
    let s = inbox.try_next().expect("live steer");
    assert_eq!(s.prompt, "switch to plan B");
    assert!(!s.is_plain(), "conductor steers get the Maestro prefix");
    // A full buffer drops the live copy; pending_steer still covers new turns.
    h.queue_steer("one");
    h.queue_steer("two");
    assert_eq!(h.lock().pending_steer, "two");
    drop(binding);
    assert!(h.lock().active_inject.is_none());
    assert!(!h.holds_done_turn("CO"));
    h.queue_steer("after");
    assert_eq!(h.lock().pending_steer, "after");
}

#[test]
fn a_stale_binding_does_not_clear_a_newer_one() {
    let h = handle();
    let (_a, first) = h.bind_inject("CO");
    let (_b, _second) = h.bind_inject("WR");
    drop(first);
    let s = h.lock();
    assert!(s.active_inject.is_some(), "newer binding kept");
    assert_eq!(s.done_turn_code, "WR");
}

#[test]
fn done_summaries_and_notes() {
    let h = handle();
    h.note_done_summary("  ", "ignored");
    assert_eq!(h.take_done_summary(), "");
    h.note_done_summary(" Added the flag ", " deviated ");
    h.note_done_summary("Fixed review", "");
    assert_eq!(h.take_done_summary(), "Fixed review");
    assert_eq!(h.take_done_summary(), "");
    assert_eq!(h.wrap_up_done(), "Added the flag\nFixed review");
    h.note_done_notes(" kept ");
    assert_eq!(h.take_done_notes(), "kept");
    assert_eq!(h.take_done_notes(), "");
}

#[test]
fn accumulate_handoff_caps() {
    assert_eq!(accumulate_handoff("", "  a "), "a");
    assert_eq!(accumulate_handoff("a", ""), "a");
    assert_eq!(accumulate_handoff("a", "b"), "a\nb");
    let long = "x".repeat(HANDOFF_MAX_CHARS + 10);
    assert!(accumulate_handoff("", &long).chars().count() <= HANDOFF_MAX_CHARS);
}

#[test]
fn accepts_done_from_holder_or_implementer() {
    let h = handle();
    assert!(!h.accepts_done_summary(""));
    assert!(
        h.accepts_done_summary("CO"),
        "first-party Coder is the default implementer"
    );
    assert!(!h.accepts_done_summary("WR"));
    let (_i, _b) = h.bind_inject("WR");
    assert!(
        h.accepts_done_summary("WR"),
        "the in-flight specialist owns the turn"
    );

    let h = handle();
    h.lock().roles = RoleView {
        loop_roles: BTreeMap::from([("implementer".to_owned(), "test.roles.writer".to_owned())]),
        phase_roles: None,
        catalog_roles: Arc::new(vec![recur_bundles::Role {
            id: "test.roles.writer".into(),
            code: "WR".into(),
            name: "Writer".into(),
            mode: "agent".into(),
            ..Default::default()
        }]),
    };
    assert!(h.accepts_done_summary("WR"), "the bundle implementer slot");
    assert!(!h.accepts_done_summary("CO"));
    h.lock().roles.phase_roles = Some(BTreeMap::from([(
        "implementer".to_owned(),
        "recur.roles.missing".to_owned(),
    )]));
    assert!(
        h.accepts_done_summary("CO"),
        "an unknown phase role falls back to the Coder"
    );
}

#[test]
fn findings_and_evidence_collectors() {
    let h = handle();
    h.begin_review_iteration(3);
    assert_eq!(h.collect_finding(), 3);
    assert_eq!(h.collect_finding(), 3);
    assert_eq!(h.findings_this_iteration(), 2);
    h.begin_review_iteration(4);
    assert_eq!(h.findings_this_iteration(), 0);
    h.collect_evidence(Evidence {
        step: "login".into(),
        screenshot_path: "/s.png".into(),
        passed: true,
    });
    assert_eq!(h.evidence_this_verify().len(), 1);
    h.reset_evidence();
    assert!(h.evidence_this_verify().is_empty());
}

#[test]
fn idle_stop_and_branch_state() {
    let h = handle();
    assert!(!h.wants_idle() && !h.is_stopped());
    h.request_idle();
    assert!(h.wants_idle());
    h.set_branch("feature");
    assert_eq!(h.branch(), "feature");
    {
        let mut s = h.lock();
        s.exit_state = recur_domain::WorkspaceState::PrOpen;
        s.exit_phase = "PR open".into();
    }
    assert_eq!(
        h.exit(),
        (recur_domain::WorkspaceState::PrOpen, "PR open".to_owned())
    );
}
