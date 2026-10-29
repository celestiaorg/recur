//! The loop-flow Host methods (ReportFinding, Done, ListFindings, UpdateFinding,
//! ReportResult, Escalate, AttachEvidence).

use std::sync::Arc;

use recur_agent_tools::{EscalationRequest, Evidence, Finding, Host};
use recur_domain::{Session, Severity, Workspace};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::RunningLoop;
use crate::testing::TestEngine;

fn finding(msg: &str) -> Finding {
    Finding {
        file: "a.go".into(),
        line: 3,
        end_line: 5,
        severity: Severity::High,
        message: msg.into(),
        summary: "  digest  ".into(),
    }
}

fn walk(t: &TestEngine, ws: &str) -> Arc<LoopHandle> {
    let h = LoopHandle::new(ws, "p", ws, LoopState::default());
    assert!(t.engine.register_loop(RunningLoop {
        handle: Arc::clone(&h),
        cancel: CancellationToken::new(),
    }));
    h
}

fn host(t: &TestEngine) -> EngineHost {
    EngineHost::new(Arc::downgrade(&t.engine))
}

#[tokio::test]
async fn report_finding_stamps_the_live_review_iteration() {
    let t = TestEngine::new(None);
    let h = walk(&t, "w");
    h.begin_review_iteration(4);
    let stored = host(&t).report_finding("w", "RV", finding("nil deref"));
    assert_eq!(stored.iteration, 4);
    assert_eq!(stored.status, "open");
    assert_eq!(stored.summary, "digest");
    assert!(!stored.id.is_empty(), "the store assigns the id");
    assert_eq!(h.findings_this_iteration(), 1);
    assert!(t.events.has("findings.updated"));
}

#[tokio::test]
async fn report_finding_without_a_walk_uses_the_workspace_iteration() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "w".into(),
            iteration: 3,
            ..Workspace::default()
        })
        .unwrap();
    assert_eq!(
        host(&t).report_finding("w", "RV", finding("x")).iteration,
        3
    );
    assert_eq!(
        host(&t)
            .report_finding("nowhere", "RV", finding("y"))
            .iteration,
        1
    );
}

#[tokio::test]
async fn coder_done_is_refused_while_findings_are_open() {
    let t = TestEngine::new(None);
    let h = walk(&t, "w");
    let f = host(&t).report_finding("w", "RV", finding("nil deref"));
    t.store
        .add_finding_note(
            "w",
            &f.id,
            "please check",
            recur_domain::FINDING_ACTOR_HUMAN,
        )
        .ok();
    let err = host(&t)
        .done("w", "CO", "done it", "notes kept")
        .expect_err("open findings refuse done");
    let want_head = "cannot finish: 1 open finding(s) remain. Address each, call mcp__recur__update_finding (resolved or invalid with reason), then call done again.\n";
    assert!(err.0.starts_with(want_head), "{}", err.0);
    let line = format!("1. id={} [high] a.go:3-5 — nil deref", f.public_id());
    assert!(err.0.contains(&line), "{}", err.0);
    assert!(
        err.0.contains("   operator note: please check"),
        "{}",
        err.0
    );
    assert!(!err.0.ends_with('\n'));
    assert_eq!(h.take_done_notes(), "notes kept", "refused notes are kept");
    assert_eq!(h.take_done_summary(), "");

    // Close it and done lands on the walk.
    host(&t)
        .update_finding("w", "CO", &f.id, "resolved", "")
        .expect("resolve");
    host(&t).done("w", "CO", "Added the flag", "").unwrap();
    assert_eq!(h.take_done_summary(), "Added the flag");
}

#[tokio::test]
async fn done_from_a_non_implementer_is_ignored() {
    let t = TestEngine::new(None);
    let h = walk(&t, "w");
    host(&t).done("w", "RV", "reviewed", "").unwrap();
    assert_eq!(h.take_done_summary(), "");
    // Two walks and no claim: ambiguous, dropped.
    let t2 = TestEngine::new(None);
    let a = walk(&t2, "w");
    let b = LoopHandle::new("w", "p", "w\x1ePA2", LoopState::default());
    assert!(t2.engine.register_loop(RunningLoop {
        handle: Arc::clone(&b),
        cancel: CancellationToken::new(),
    }));
    host(&t2).done("w", "CO", "who?", "").unwrap();
    assert_eq!(a.take_done_summary(), "");
    assert_eq!(b.take_done_summary(), "");
    let (_inbox, _bind) = b.bind_inject("CO");
    host(&t2).done("w", "CO", "mine", "").unwrap();
    assert_eq!(b.take_done_summary(), "mine", "the claiming walk owns it");
}

#[tokio::test]
async fn list_and_update_findings_attribution() {
    let t = TestEngine::new(None);
    let a = host(&t).report_finding("w", "RV", finding("a"));
    let b = host(&t).report_finding("w", "RV", finding("b"));
    assert_eq!(host(&t).list_findings("w", "CO", "open").unwrap().len(), 2);
    assert!(
        host(&t)
            .list_findings("w", "CO", "resolved")
            .unwrap()
            .is_empty()
    );

    let up = host(&t)
        .update_finding("w", "RV", &a.id, "invalid", "dup")
        .unwrap();
    assert_eq!(up.updated_by, recur_domain::FINDING_ACTOR_REVIEWER);
    let up = host(&t)
        .update_finding("w", "PA", &b.id, "resolved", "")
        .unwrap();
    assert_eq!(up.updated_by, recur_domain::FINDING_ACTOR_CODER);
    assert!(
        host(&t)
            .update_finding("w", "CO", "nope", "resolved", "")
            .is_err()
    );

    // A Review-mode chat is the reviewer.
    t.store
        .add_workspace(Workspace {
            id: "w2".into(),
            ..Workspace::default()
        })
        .unwrap();
    t.store
        .create_session(Session {
            workspace_id: "w2".into(),
            kind: "provider".into(),
            config_json: r#"{"code":"PA2","name":"Agent","mode":"review"}"#.into(),
            ..Session::default()
        })
        .unwrap();
    assert_eq!(
        t.engine.finding_update_actor("w2", "PA2"),
        recur_domain::FINDING_ACTOR_REVIEWER
    );
    assert_eq!(t.engine.chat_selection_mode("w2", "MA"), "loop");
    assert_eq!(
        t.engine.finding_update_actor("w2", "SR"),
        recur_domain::FINDING_ACTOR_REVIEWER
    );
}

#[tokio::test]
async fn report_result_escalate_and_evidence() {
    let t = TestEngine::new(None);
    t.store
        .add_workspace(Workspace {
            id: "w".into(),
            ..Workspace::default()
        })
        .unwrap();
    host(&t).report_result("w", "TE", true, "all green");
    let tr = t.store.workspace("w").unwrap().test_result.unwrap();
    assert!(tr.passed);
    assert_eq!(
        (tr.summary.as_str(), tr.when.as_str()),
        ("all green", "now")
    );

    // No walk: escalations and evidence are dropped.
    host(&t).escalate(
        "w",
        "MA",
        EscalationRequest {
            question: "Which?".into(),
            ..EscalationRequest::default()
        },
    );
    let h = walk(&t, "w");
    assert!(!h.has_pending_escalation());
    host(&t).escalate(
        "w",
        "MA",
        EscalationRequest {
            question: "Which?".into(),
            ..EscalationRequest::default()
        },
    );
    assert_eq!(h.take_pending_escalation().unwrap().question, "Which?");
    host(&t).attach_evidence(
        "w",
        "QA",
        Evidence {
            step: "login".into(),
            screenshot_path: "/s.png".into(),
            passed: false,
        },
    );
    assert_eq!(h.evidence_this_verify().len(), 1);
    t.engine.begin_verify(&h);
    assert!(h.evidence_this_verify().is_empty());
    assert!(t.store.workspace("w").unwrap().test_result.is_none());
}

#[tokio::test]
async fn host_after_engine_drop_is_a_noop() {
    let t = TestEngine::new(None);
    let h = host(&t);
    drop(t);
    assert_eq!(
        h.report_finding("w", "RV", finding("x")),
        recur_domain::Finding::default()
    );
    assert!(h.done("w", "CO", "s", "n").is_ok());
    assert!(h.list_findings("w", "CO", "").unwrap().is_empty());
}
