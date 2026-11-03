use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_domain::{PermissionDecision, PermissionRequest};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::loop_handle::{LoopHandle, LoopState};
use crate::loop_registry::RunningLoop;
use crate::testing::TestEngine;

type Outcome = Result<PermissionDecision, recur_agent::PermissionAbandoned>;

fn req(tool: &str) -> PermissionRequest {
    PermissionRequest {
        workspace_id: "W1".into(),
        agent_code: "C".into(),
        tool: tool.into(),
        ..PermissionRequest::default()
    }
}

fn ask(
    b: &Arc<PermissionBroker>,
    cancel: &CancellationToken,
    r: PermissionRequest,
) -> tokio::task::JoinHandle<Outcome> {
    let b = Arc::clone(b);
    let cancel = cancel.clone();
    tokio::spawn(async move { b.request(&cancel, r).await })
}

async fn wait_for_pending(b: &PermissionBroker) -> PermissionRequest {
    for _ in 0..2000 {
        if let Some(p) = b.pending().into_iter().next() {
            return p;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("no permission request became pending");
}

#[tokio::test]
async fn blocks_until_answered() {
    let b = Arc::new(PermissionBroker::new());
    let notified: Arc<Mutex<Vec<PermissionRequest>>> = Arc::default();
    let n = Arc::clone(&notified);
    b.set_notify(move |r| n.lock().push(r.clone()));
    let out = ask(&b, &CancellationToken::new(), req("Bash"));
    let r = wait_for_pending(&b).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !out.is_finished(),
        "request returned before it was answered"
    );
    assert!(b.answer(
        &r.id,
        PermissionDecision {
            allow: true,
            ..Default::default()
        }
    ));
    let got = out.await.unwrap().unwrap();
    assert!(got.allow);
    let n = notified.lock();
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].tool, "Bash");
}

#[tokio::test]
async fn answer_is_idempotent() {
    let b = Arc::new(PermissionBroker::new());
    let out = ask(&b, &CancellationToken::new(), req("Bash"));
    let r = wait_for_pending(&b).await;
    assert!(b.answer(
        &r.id,
        PermissionDecision {
            allow: true,
            ..Default::default()
        }
    ));
    out.await.unwrap().unwrap();
    assert!(!b.answer(&r.id, PermissionDecision::default()));
    assert!(!b.answer("perm-nope", PermissionDecision::default()));
}

#[tokio::test]
async fn times_out_as_denial() {
    let b = PermissionBroker::new();
    b.set_timeout(Duration::from_millis(10));
    let d = b
        .request(&CancellationToken::new(), req("Bash"))
        .await
        .expect("timeout is a verdict");
    assert!(!d.allow);
    assert!(d.reason.contains("no answer"), "{}", d.reason);
    assert_eq!(d.reason, "no answer within 10ms: denied automatically");
    assert!(b.pending().is_empty());
}

#[test]
fn default_timeout_reads_like_go() {
    assert_eq!(
        recur_project::runconfig::go_duration(PERMISSION_TIMEOUT),
        "10m0s"
    );
}

#[tokio::test]
async fn releases_on_dead_agent() {
    let b = Arc::new(PermissionBroker::new());
    let resolved: Arc<Mutex<Vec<String>>> = Arc::default();
    let r2 = Arc::clone(&resolved);
    b.set_resolved(move |id, _| r2.lock().push(id.to_owned()));
    let cancel = CancellationToken::new();
    let out = ask(&b, &cancel, req("Bash"));
    let r = wait_for_pending(&b).await;
    cancel.cancel();
    assert!(out.await.unwrap().is_err(), "abandonment");
    assert!(b.pending().is_empty());
    assert_eq!(*resolved.lock(), vec![r.id]);
}

#[tokio::test]
async fn end_turn_releases_waiters() {
    let b = Arc::new(PermissionBroker::new());
    let out = ask(&b, &CancellationToken::new(), req("Bash"));
    wait_for_pending(&b).await;
    b.end_turn("W1", "C");
    let got = out.await.unwrap().unwrap();
    assert!(!got.allow);
    assert_eq!(got.reason, "the turn ended before this was answered");
    assert!(b.pending().is_empty());
}

#[tokio::test]
async fn remembers_for_the_turn() {
    let b = Arc::new(PermissionBroker::new());
    let out = ask(&b, &CancellationToken::new(), req("Bash"));
    let r = wait_for_pending(&b).await;
    b.answer(
        &r.id,
        PermissionDecision {
            allow: true,
            remember: true,
            ..Default::default()
        },
    );
    out.await.unwrap().unwrap();

    let d = b
        .request(&CancellationToken::new(), req("Bash"))
        .await
        .unwrap();
    assert!(d.allow, "remembered tool auto-allows");

    let other = ask(&b, &CancellationToken::new(), req("Write"));
    let got = wait_for_pending(&b).await;
    assert_eq!(got.tool, "Write");
    b.answer(
        &got.id,
        PermissionDecision {
            allow: true,
            ..Default::default()
        },
    );
    other.await.unwrap().unwrap();

    b.end_turn("W1", "C");
    let next = ask(&b, &CancellationToken::new(), req("Bash"));
    assert_eq!(wait_for_pending(&b).await.tool, "Bash");
    b.end_turn("W1", "C");
    next.await.unwrap().unwrap();
}

#[tokio::test]
async fn surface_for() {
    let te = TestEngine::new(None);
    let e = &te.engine;
    te.store
        .add_workspace(recur_domain::Workspace {
            id: "W".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(e.permission_surface_for("W", "PA"), PERMISSION_SURFACE_CHAT);
    let h = LoopHandle::new("W", "proj", "W", LoopState::default());
    assert!(e.register_loop(RunningLoop {
        handle: h,
        cancel: CancellationToken::new(),
    }));
    assert_eq!(e.permission_surface_for("W", "CD"), PERMISSION_SURFACE_LOOP);
    e.register_chat_turn("W", "PA", CancellationToken::new())
        .unwrap();
    assert_eq!(e.permission_surface_for("W", "PA"), PERMISSION_SURFACE_CHAT);
    e.register_chat_turn("W", "MA", CancellationToken::new())
        .unwrap();
    assert_eq!(e.permission_surface_for("W", "MA"), PERMISSION_SURFACE_CHAT);
    te.close().await;
}

#[tokio::test]
async fn loop_request_raises_inbox_and_resolve_clears_it() {
    let te = TestEngine::new(None);
    let e = Arc::clone(&te.engine);
    te.store
        .add_workspace(recur_domain::Workspace {
            id: "W".into(),
            ..Default::default()
        })
        .unwrap();
    let h = LoopHandle::new("W", "proj", "W", LoopState::default());
    e.register_loop(RunningLoop {
        handle: h,
        cancel: CancellationToken::new(),
    });
    let cancel = CancellationToken::new();
    let eng = Arc::clone(&e);
    let task = tokio::spawn(async move {
        eng.request_permission(
            &cancel,
            "W",
            "CO",
            recur_agent_tools::PermissionAsk {
                tool: "Bash".into(),
                input: r#"{"command":"ls"}"#.into(),
                tool_use_id: String::new(),
            },
        )
        .await
    });
    let r = wait_for_pending(e.permission_broker()).await;
    assert_eq!(r.surface, PERMISSION_SURFACE_LOOP);
    assert_eq!(r.summary, "Bash: ls");
    let inbox = te.store.inbox().unwrap_or_default();
    let row = inbox.iter().find(|i| i.id == r.id).expect("inbox row");
    assert_eq!(row.title, "Permission needed: <b>Bash</b>");
    assert_eq!(row.target, "workspace:W");
    assert!(te.events.has("notify"));
    assert!(e.answer_permission(
        &r.id,
        PermissionDecision {
            allow: true,
            ..Default::default()
        }
    ));
    let v = task.await.unwrap().unwrap();
    assert!(v.allow);
    assert!(
        te.store
            .inbox()
            .unwrap_or_default()
            .iter()
            .all(|i| i.id != r.id)
    );
    let resolved = te.events.by_name("permission.resolved");
    assert_eq!(
        resolved[0].payload_json(),
        format!(r#"{{"allow":true,"id":"{}","reason":""}}"#, r.id)
    );
    te.close().await;
}
