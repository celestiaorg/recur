//! Steer, active-run, options, prompt and runner-contract tests, against
//! an HTTP stand-in for the bridge.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use recur_agent::turn::format_steer_user_message;
use recur_agent::{AgentEvent, Steer, SteerInbox, TurnRequest};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::testserver::{Req, Resp, TestServer, serve};
use crate::turn::tests::{Rec, kinds};

/// Counters and captures shared by a stand-in bridge.
#[derive(Default)]
struct Calls {
    sends: AtomicUsize,
    cancels: AtomicUsize,
    steers: AtomicUsize,
    observes: AtomicUsize,
    send_texts: Mutex<Vec<String>>,
    steer_calls: Mutex<Vec<(String, String)>>,
    cancel_ids: Mutex<Vec<String>>,
}

impl Calls {
    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }
    fn cancels(&self) -> usize {
        self.cancels.load(Ordering::SeqCst)
    }
    fn steers(&self) -> usize {
        self.steers.load(Ordering::SeqCst)
    }
    fn observes(&self) -> usize {
        self.observes.load(Ordering::SeqCst)
    }
}

/// Answers the agent bookkeeping RPCs every test shares; `None` when the
/// request was handled.
async fn common(req: &Req, resp: Resp, agent: &str, runs: &str) -> Option<Resp> {
    if req.ends("/CreateAgent") || req.ends("/ResumeAgent") {
        resp.json(&format!(r#"{{"agentId":"{agent}"}}"#)).await;
        None
    } else if req.ends("/CloseAgent") {
        resp.json("{}").await;
        None
    } else if req.ends("/ListRuns") {
        resp.json(runs).await;
        None
    } else {
        Some(resp)
    }
}

fn status_frame(agent: &str, run: &str) -> String {
    format!(
        r#"{{"sdkMessage":{{"type":"status","message":{{"type":"status","agent_id":"{agent}","run_id":"{run}","status":"RUNNING"}}}},"offset":"1"}}"#
    )
}

fn result_frame(agent: &str, run: &str, status: &str, text: &str) -> String {
    format!(
        r#"{{"result":{{"agentId":"{agent}","runId":"{run}","status":"RUN_LIFECYCLE_STATUS_{status}","result":{{"runId":"{run}","agentId":"{agent}","status":"RUN_LIFECYCLE_STATUS_{status}","result":"{text}"}}}},"offset":"2"}}"#
    )
}

fn turn_req(resume: &str, prompt: &str) -> TurnRequest {
    TurnRequest {
        worktree: std::env::temp_dir(),
        model: "composer-2.5".into(),
        prompt: prompt.into(),
        resume_session: resume.into(),
        ..Default::default()
    }
}

async fn run_with(
    runner: CursorRunner,
    req: TurnRequest,
    cancel: CancellationToken,
    inbox: Option<SteerInbox>,
) -> (Result<recur_agent::TurnResult, RunFailure>, Vec<AgentEvent>) {
    let runner = Arc::new(runner);
    let rec = Rec::default();
    let mut steer = inbox;
    let res = runner.run(&req, &cancel, rec.sink(), &mut steer).await;
    (res, rec.all())
}

fn runner_for(srv: &TestServer, started: Option<DateTime<Utc>>) -> CursorRunner {
    CursorRunner::new(Bridge::bound(&srv.url, "tok", started), "sk-test")
}

/// A steer channel and its sender.
fn inbox() -> (mpsc::Sender<Steer>, SteerInbox) {
    let (tx, rx) = mpsc::channel(4);
    (tx, SteerInbox(rx))
}

async fn send_steer(tx: &mpsc::Sender<Steer>, text: &str) {
    let _ = tx.send(Steer::new(text, None).0).await;
}

/// Waits for `t`, giving up after 5s.
async fn wait(t: &CancellationToken) -> bool {
    tokio::time::timeout(Duration::from_secs(5), t.cancelled())
        .await
        .is_ok()
}

fn ok_result(res: &Result<recur_agent::TurnResult, RunFailure>) -> &recur_agent::TurnResult {
    match res {
        Ok(r) => r,
        Err(f) => panic!("run failed: {} ({:?})", f.err, f.partial),
    }
}

fn user_texts(evs: &[AgentEvent], live: Option<bool>) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match e {
            AgentEvent::User {
                text, live_steer, ..
            } if live.is_none_or(|l| l == *live_steer) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ---- cursor steer

/// D49 on Cursor: a steer mid-run cancels the run (SDK run.cancel), joins
/// every waiting steer, and Sends one follow-up on the same agent. The turn
/// succeeds; no error_cancelled result.
#[tokio::test]
async fn steer_cancels_then_sends() {
    let calls = Arc::new(Calls::default());
    let status_sent = CancellationToken::new();
    let cancelled = CancellationToken::new();
    let (c, ss, cn) = (calls.clone(), status_sent.clone(), cancelled.clone());
    let srv = serve(move |req, resp| {
        let (c, ss, cn) = (c.clone(), ss.clone(), cn.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-steer", r#"{"items":[]}"#).await else {
                return;
            };
            if req.ends("/SteerRun") {
                resp.json(r#"{"outcome":"revert_to_followup"}"#).await;
            } else if req.ends("/CancelRun") {
                c.cancels.fetch_add(1, Ordering::SeqCst);
                cn.cancel();
                resp.json("{}").await;
            } else if req.ends("/Send") {
                let n = c.sends.fetch_add(1, Ordering::SeqCst) + 1;
                c.send_texts.lock().push(
                    req.json()["message"]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                );
                if n == 1 {
                    resp.frame(&status_frame("agent-steer", "run-steer")).await;
                    ss.cancel();
                    tokio::select! { _ = cn.cancelled() => {}, _ = resp.closed.cancelled() => {} }
                    resp.frame(&result_frame("agent-steer", "run-steer", "CANCELLED", ""))
                        .await;
                    resp.end("{}").await;
                    return;
                }
                resp.frame(&result_frame("agent-steer", "run-2", "FINISHED", "steered"))
                    .await;
                resp.end("{}").await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = inbox();
    tokio::spawn(async move {
        if wait(&status_sent).await {
            send_steer(&tx, "first steer").await;
            send_steer(&tx, "second steer").await;
        }
    });
    let (res, evs) = run_with(
        runner_for(&srv, None),
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(rx),
    )
    .await;
    let r = ok_result(&res);
    assert!(!r.is_error && r.subtype == "success", "{r:?}");
    assert_eq!(r.text, "steered");
    assert_eq!(calls.sends(), 2, "in-flight + follow-up");
    assert!(calls.cancels() >= 1, "SDK run.cancel");
    let texts = calls.send_texts.lock().clone();
    assert_eq!(
        texts[1],
        format_steer_user_message("first steer\n\nsecond steer")
    );
    assert!(
        user_texts(&evs, None)
            .iter()
            .any(|t| t.contains("first steer") && t.contains("second steer")),
        "{:?}",
        kinds(&evs)
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd(r) if r.subtype == "error_cancelled")),
        "steer CancelRun must not end the turn"
    );
}

#[tokio::test]
async fn steer_complete_delivered_keeps_run() {
    let calls = Arc::new(Calls::default());
    let status_sent = CancellationToken::new();
    let steered = CancellationToken::new();
    let (c, ss, st) = (calls.clone(), status_sent.clone(), steered.clone());
    let srv = serve(move |req, resp| {
        let (c, ss, st) = (c.clone(), ss.clone(), st.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-delivered", r#"{"items":[]}"#).await
            else {
                return;
            };
            if req.ends("/SteerRun") {
                c.steers.fetch_add(1, Ordering::SeqCst);
                let all = {
                    let mut s = c.steer_calls.lock();
                    s.push((req.str("runId"), req.str("text")));
                    s.iter()
                        .map(|(_, t)| t.clone())
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                if all.contains("first steer") && all.contains("second steer") {
                    st.cancel();
                }
                resp.json(r#"{"outcome":"complete_delivered"}"#).await;
            } else if req.ends("/CancelRun") {
                c.cancels.fetch_add(1, Ordering::SeqCst);
                resp.json("{}").await;
            } else if req.ends("/Send") {
                c.sends.fetch_add(1, Ordering::SeqCst);
                resp.frame(&status_frame("agent-delivered", "run-live"))
                    .await;
                ss.cancel();
                tokio::select! { _ = st.cancelled() => {}, _ = resp.closed.cancelled() => {} }
                resp.frame(&result_frame(
                    "agent-delivered",
                    "run-live",
                    "FINISHED",
                    "kept",
                ))
                .await;
                resp.end("{}").await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = inbox();
    tokio::spawn(async move {
        if wait(&status_sent).await {
            send_steer(&tx, "first steer").await;
            send_steer(&tx, "second steer").await;
        }
    });
    let (res, evs) = run_with(
        runner_for(&srv, None),
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(rx),
    )
    .await;
    let r = ok_result(&res);
    assert!(!r.is_error && r.subtype == "success");
    assert_eq!(r.text, "kept");
    assert_eq!(calls.sends(), 1, "a delivered steer must not follow up");
    assert_eq!(calls.cancels(), 0);
    assert!(calls.steers() >= 1);
    let joined: Vec<String> = calls
        .steer_calls
        .lock()
        .iter()
        .map(|(_, t)| t.clone())
        .collect();
    let joined = joined.join("\n");
    assert!(
        joined.contains("first steer") && joined.contains("second steer"),
        "{joined}"
    );
    assert!(
        user_texts(&evs, Some(true))
            .iter()
            .any(|t| t.contains("first steer"))
    );
}

#[tokio::test]
async fn ctx_cancel_does_not_follow_up_send() {
    let calls = Arc::new(Calls::default());
    let status_sent = CancellationToken::new();
    let (c, ss) = (calls.clone(), status_sent.clone());
    let srv = serve(move |req, resp| {
        let (c, ss) = (c.clone(), ss.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-stop", r#"{"items":[]}"#).await else {
                return;
            };
            if req.ends("/SteerRun") {
                resp.json(r#"{"outcome":"revert_to_followup"}"#).await;
            } else if req.ends("/CancelRun") {
                c.cancels.fetch_add(1, Ordering::SeqCst);
                resp.json("{}").await;
            } else if req.ends("/Send") {
                c.sends.fetch_add(1, Ordering::SeqCst);
                resp.frame(&status_frame("agent-stop", "run-stop")).await;
                // Let the client note the run id before the test cancels.
                tokio::time::sleep(Duration::from_millis(50)).await;
                ss.cancel();
                resp.closed.cancelled().await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        wait(&status_sent).await;
        c2.cancel();
    });
    let (_tx, rx) = inbox();
    let (res, _) = run_with(
        runner_for(&srv, None),
        turn_req("", "do the thing"),
        cancel,
        Some(rx),
    )
    .await;
    let err = res.expect_err("canceled");
    assert!(err.err.is_canceled(), "{}", err.err);
    assert_eq!(calls.sends(), 1, "Stop must not start a follow-up");
    assert!(calls.cancels() >= 1);
}

#[tokio::test]
async fn late_steer_after_finished_does_not_fail() {
    let calls = Arc::new(Calls::default());
    let finished = CancellationToken::new();
    let (c, fin) = (calls.clone(), finished.clone());
    let srv = serve(move |req, resp| {
        let (c, fin) = (c.clone(), fin.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-late", r#"{"items":[]}"#).await else {
                return;
            };
            if req.ends("/SteerRun") {
                resp.json(r#"{"outcome":"revert_to_followup"}"#).await;
            } else if req.ends("/CancelRun") {
                resp.json("{}").await;
            } else if req.ends("/Send") {
                c.sends.fetch_add(1, Ordering::SeqCst);
                resp.frame(&result_frame("agent-late", "run-done", "FINISHED", "done")).await;
                fin.cancel();
                tokio::select! { _ = resp.closed.cancelled() => {}, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = inbox();
    tokio::spawn(async move {
        if wait(&finished).await {
            tokio::time::sleep(Duration::from_millis(50)).await;
            send_steer(&tx, "too late").await;
        }
    });
    let (res, _) = run_with(
        runner_for(&srv, None),
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(rx),
    )
    .await;
    let r = ok_result(&res);
    assert!(
        !r.is_error && r.subtype == "success" && r.text == "done",
        "{r:?}"
    );
    assert_eq!(
        calls.sends(),
        1,
        "a late steer must not follow up after FINISHED"
    );
}

fn hook(
    f: impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    + Send
    + Sync
    + 'static,
) -> Option<Hook> {
    Some(Arc::new(f))
}

#[tokio::test]
async fn handoff_steer_does_not_cancel_follow_up() {
    let calls = Arc::new(Calls::default());
    let status_sent = CancellationToken::new();
    let cancelled = CancellationToken::new();
    let (c, ss, cn) = (calls.clone(), status_sent.clone(), cancelled.clone());
    let srv = serve(move |req, resp| {
        let (c, ss, cn) = (c.clone(), ss.clone(), cn.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-handoff", r#"{"items":[]}"#).await
            else {
                return;
            };
            if req.ends("/SteerRun") {
                resp.json(r#"{"outcome":"revert_to_followup"}"#).await;
            } else if req.ends("/CancelRun") {
                let id = req.str("runId");
                c.cancel_ids.lock().push(id.clone());
                if id == "run-steer" {
                    cn.cancel();
                }
                resp.json("{}").await;
            } else if req.ends("/Send") {
                let n = c.sends.fetch_add(1, Ordering::SeqCst) + 1;
                c.send_texts.lock().push(
                    req.json()["message"]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                );
                if n == 1 {
                    resp.frame(&status_frame("agent-handoff", "run-steer"))
                        .await;
                    ss.cancel();
                    tokio::select! { _ = cn.cancelled() => {}, _ = resp.closed.cancelled() => {} }
                    resp.frame(&result_frame("agent-handoff", "run-steer", "CANCELLED", ""))
                        .await;
                    resp.end("{}").await;
                    return;
                }
                let run = format!("run-{n}");
                resp.frame(&status_frame("agent-handoff", &run)).await;
                tokio::time::sleep(Duration::from_millis(150)).await;
                let hit = c.cancel_ids.lock().contains(&run);
                let status = if hit { "CANCELLED" } else { "FINISHED" };
                resp.frame(&result_frame("agent-handoff", &run, status, "steered"))
                    .await;
                resp.end("{}").await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = mpsc::channel::<Steer>(1);
    let once = Arc::new(AtomicBool::new(false));
    let handoff_tx = tx.clone();
    let runner = runner_for(&srv, None).with_hooks(
        hook(move || {
            let (once, tx) = (once.clone(), handoff_tx.clone());
            Box::pin(async move {
                if !once.swap(true, Ordering::SeqCst) {
                    send_steer(&tx, "handoff steer").await;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
        }),
        None,
    );
    tokio::spawn(async move {
        if wait(&status_sent).await {
            send_steer(&tx, "first steer").await;
        }
    });
    let (res, _) = run_with(
        runner,
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(SteerInbox(rx)),
    )
    .await;
    let r = ok_result(&res);
    assert!(!r.is_error && r.subtype == "success", "{r:?}");
    assert!(calls.sends() >= 2);
    assert!(
        calls
            .send_texts
            .lock()
            .iter()
            .any(|t| t.contains("handoff steer")),
        "handoff steer must ride a follow-up Send: {:?}",
        calls.send_texts.lock()
    );
}

#[tokio::test]
async fn delivered_steer_during_handoff() {
    let calls = Arc::new(Calls::default());
    let status_sent = CancellationToken::new();
    let cancelled = CancellationToken::new();
    let (c, ss, cn) = (calls.clone(), status_sent.clone(), cancelled.clone());
    let srv = serve(move |req, resp| {
        let (c, ss, cn) = (c.clone(), ss.clone(), cn.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-handoff-live", r#"{"items":[]}"#).await
            else {
                return;
            };
            if req.ends("/SteerRun") {
                let n = c.steers.fetch_add(1, Ordering::SeqCst) + 1;
                c.steer_calls
                    .lock()
                    .push((req.str("runId"), req.str("text")));
                let outcome = if n == 1 {
                    "revert_to_followup"
                } else {
                    "complete_delivered"
                };
                resp.json(&format!(r#"{{"outcome":"{outcome}"}}"#)).await;
            } else if req.ends("/CancelRun") {
                c.cancels.fetch_add(1, Ordering::SeqCst);
                let id = req.str("runId");
                c.cancel_ids.lock().push(id.clone());
                if id == "run-steer" {
                    cn.cancel();
                }
                resp.json("{}").await;
            } else if req.ends("/Send") {
                let n = c.sends.fetch_add(1, Ordering::SeqCst) + 1;
                c.send_texts.lock().push(
                    req.json()["message"]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                );
                if n == 1 {
                    resp.frame(&status_frame("agent-handoff-live", "run-steer"))
                        .await;
                    ss.cancel();
                    tokio::select! { _ = cn.cancelled() => {}, _ = resp.closed.cancelled() => {} }
                    resp.frame(&result_frame(
                        "agent-handoff-live",
                        "run-steer",
                        "CANCELLED",
                        "",
                    ))
                    .await;
                    resp.end("{}").await;
                    return;
                }
                resp.frame(&status_frame("agent-handoff-live", "run-2"))
                    .await;
                tokio::time::sleep(Duration::from_millis(150)).await;
                resp.frame(&result_frame(
                    "agent-handoff-live",
                    "run-2",
                    "FINISHED",
                    "steered",
                ))
                .await;
                resp.end("{}").await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = mpsc::channel::<Steer>(1);
    let (rtx, rrx) = mpsc::channel::<()>(2);
    let rrx = Arc::new(tokio::sync::Mutex::new(rrx));
    let once = Arc::new(AtomicBool::new(false));
    let handoff_tx = tx.clone();
    let runner = runner_for(&srv, None).with_hooks(
        hook(move || {
            let (once, tx, rrx) = (once.clone(), handoff_tx.clone(), rrx.clone());
            Box::pin(async move {
                if once.swap(true, Ordering::SeqCst) {
                    return;
                }
                let mut rrx = rrx.lock().await;
                while rrx.try_recv().is_ok() {}
                send_steer(&tx, "handoff steer").await;
                // The listener's post-cancel drain can also pick it up, in
                // which case it rides the follow-up and is never reserved.
                let _ = tokio::time::timeout(Duration::from_secs(2), rrx.recv()).await;
            })
        }),
        hook(move || {
            let rtx = rtx.clone();
            Box::pin(async move {
                let _ = rtx.try_send(());
            })
        }),
    );
    tokio::spawn(async move {
        if wait(&status_sent).await {
            send_steer(&tx, "first steer").await;
        }
    });
    let (res, _) = run_with(
        runner,
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(SteerInbox(rx)),
    )
    .await;
    let r = ok_result(&res);
    assert!(!r.is_error && r.subtype == "success", "{r:?}");
    assert_eq!(calls.sends(), 2);
    assert!(
        !calls.cancel_ids.lock().contains(&"run-2".to_owned()),
        "a delivered handoff steer must keep the follow-up Send"
    );
    let steered = calls
        .steer_calls
        .lock()
        .iter()
        .any(|(_, t)| t.contains("handoff steer"));
    let texts = calls.send_texts.lock().clone();
    let folded = texts.len() >= 2 && texts[1].contains("handoff steer");
    assert_ne!(
        steered, folded,
        "handoff steer delivered exactly once: steer={steered} send={texts:?}"
    );
}

#[tokio::test]
async fn delivered_steer_during_stream_complete() {
    let calls = Arc::new(Calls::default());
    let steer_started = CancellationToken::new();
    let finished_written = CancellationToken::new();
    let (c, ss, fw) = (
        calls.clone(),
        steer_started.clone(),
        finished_written.clone(),
    );
    let srv = serve(move |req, resp| {
        let (c, ss, fw) = (c.clone(), ss.clone(), fw.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-complete", r#"{"items":[]}"#).await else {
                return;
            };
            if req.ends("/SteerRun") {
                ss.cancel();
                fw.cancelled().await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                resp.json(r#"{"outcome":"complete_delivered"}"#).await;
            } else if req.ends("/CancelRun") {
                c.cancels.fetch_add(1, Ordering::SeqCst);
                resp.json("{}").await;
            } else if req.ends("/Send") {
                c.sends.fetch_add(1, Ordering::SeqCst);
                resp.frame(&status_frame("agent-complete", "run-live")).await;
                ss.cancelled().await;
                resp.frame(r#"{"result":{"agentId":"agent-complete","runId":"run-live","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-live","agentId":"agent-complete","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"done","durationMs":"1500"}},"offset":"2"}"#).await;
                resp.end("{}").await;
                fw.cancel();
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = inbox();
    let (steer, ack) = Steer::new("late-but-delivered", None);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = tx.send(steer).await;
    });
    let (res, evs) = run_with(
        runner_for(&srv, None),
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(rx),
    )
    .await;
    let r = ok_result(&res);
    assert!(!r.is_error && r.subtype == "success");
    assert_eq!(calls.sends(), 1);
    assert_eq!(calls.cancels(), 0);
    assert_eq!(ack.await, Ok(true), "a delivered steer acks true");
    let live = evs
        .iter()
        .position(|e| matches!(e, AgentEvent::User { live_steer: true, text, .. } if text.contains("late-but-delivered")))
        .expect("live user row");
    let end = evs
        .iter()
        .position(|e| matches!(e, AgentEvent::TurnEnd(_)))
        .expect("turn end");
    let AgentEvent::TurnEnd(tr) = &evs[end] else {
        unreachable!()
    };
    assert_eq!(tr.duration_ms, 1500);
    assert!(
        live < end,
        "the live user row must precede the turn end: {:?}",
        kinds(&evs)
    );
}

#[tokio::test]
async fn finished_turn_does_not_wait_on_stuck_steer_run() {
    let steer_started = CancellationToken::new();
    let released = CancellationToken::new();
    let (ss, rel) = (steer_started.clone(), released.clone());
    let srv = serve(move |req, resp| {
        let (ss, rel) = (ss.clone(), rel.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-stuck-steer", r#"{"items":[]}"#).await
            else {
                return;
            };
            if req.ends("/SteerRun") {
                ss.cancel();
                tokio::select! { _ = rel.cancelled() => {}, _ = resp.closed.cancelled() => {} }
            } else if req.ends("/CancelRun") {
                resp.json("{}").await;
            } else if req.ends("/Send") {
                resp.frame(&status_frame("agent-stuck-steer", "run-stuck"))
                    .await;
                ss.cancelled().await;
                resp.frame(&result_frame(
                    "agent-stuck-steer",
                    "run-stuck",
                    "FINISHED",
                    "done",
                ))
                .await;
                resp.end("{}").await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = inbox();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        send_steer(&tx, "steer while running").await;
    });
    let started = std::time::Instant::now();
    let (res, _) = run_with(
        runner_for(&srv, None),
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(rx),
    )
    .await;
    released.cancel();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "waited on a stuck SteerRun: {:?}",
        started.elapsed()
    );
    assert_eq!(ok_result(&res).text, "done");
}

#[tokio::test]
async fn steer_during_stream_serializes_events() {
    let calls = Arc::new(Calls::default());
    let status_sent = CancellationToken::new();
    let steered = CancellationToken::new();
    let (c, ss, st) = (calls.clone(), status_sent.clone(), steered.clone());
    let srv = serve(move |req, resp| {
        let (c, ss, st) = (c.clone(), ss.clone(), st.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-race", r#"{"items":[]}"#).await else {
                return;
            };
            if req.ends("/SteerRun") {
                tokio::time::sleep(Duration::from_millis(80)).await;
                st.cancel();
                resp.json(r#"{"outcome":"complete_delivered"}"#).await;
            } else if req.ends("/CancelRun") {
                c.cancels.fetch_add(1, Ordering::SeqCst);
                resp.json("{}").await;
            } else if req.ends("/Send") {
                c.sends.fetch_add(1, Ordering::SeqCst);
                resp.frame(&status_frame("agent-race", "run-live")).await;
                resp.frame(r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"read","status":"running","args":{"path":"a.go"}}},"offset":"2"}"#).await;
                ss.cancel();
                for _ in 0..20 {
                    resp.frame(r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","usage":{"inputTokens":1000,"outputTokens":500,"cacheReadTokens":2000,"cacheWriteTokens":0,"totalTokens":3500}}},"offset":"u"}"#).await;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                tokio::select! { _ = st.cancelled() => {}, _ = resp.closed.cancelled() => {} }
                resp.frame(r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"c1","name":"read","status":"completed","args":{"path":"a.go"},"result":{"status":"success","value":{"content":"ok"}}}},"offset":"3"}"#).await;
                resp.frame(&result_frame("agent-race", "run-live", "FINISHED", "kept")).await;
                resp.end("{}").await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = inbox();
    tokio::spawn(async move {
        if wait(&status_sent).await {
            send_steer(&tx, "steer while streaming").await;
        }
    });
    let (res, evs) = run_with(
        runner_for(&srv, None),
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(rx),
    )
    .await;
    let r = ok_result(&res);
    assert!(!r.is_error && r.subtype == "success");
    assert_eq!((calls.sends(), calls.cancels()), (1, 0));
    let step = evs
        .iter()
        .any(|e| matches!(e, AgentEvent::Step(s) if s.id == "c1"));
    let live = user_texts(&evs, Some(true))
        .iter()
        .any(|t| t.contains("steer while streaming"));
    let result = evs
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolResult(t) if t.id == "c1"));
    assert!(step && live && result, "{:?}", kinds(&evs));
}

#[tokio::test]
async fn live_steer_flushes_streamed_text() {
    let calls = Arc::new(Calls::default());
    let status_sent = CancellationToken::new();
    let steered = CancellationToken::new();
    let (c, ss, st) = (calls.clone(), status_sent.clone(), steered.clone());
    let chunk = |t: &str| {
        format!(
            r#"{{"sdkMessage":{{"type":"assistant","message":{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{t}"}}]}}}}}}}}"#
        )
    };
    let (a, b) = (chunk("chunkA"), chunk("chunkB"));
    let srv = serve(move |req, resp| {
        let (c, ss, st, a, b) = (c.clone(), ss.clone(), st.clone(), a.clone(), b.clone());
        async move {
            let Some(mut resp) = common(&req, resp, "agent-text", r#"{"items":[]}"#).await else {
                return;
            };
            if req.ends("/SteerRun") {
                tokio::time::sleep(Duration::from_millis(40)).await;
                resp.json(r#"{"outcome":"complete_delivered"}"#).await;
                st.cancel();
            } else if req.ends("/CancelRun") {
                c.cancels.fetch_add(1, Ordering::SeqCst);
                resp.json("{}").await;
            } else if req.ends("/Send") {
                c.sends.fetch_add(1, Ordering::SeqCst);
                resp.frame(&status_frame("agent-text", "run-live")).await;
                resp.frame(&a).await;
                ss.cancel();
                tokio::select! { _ = st.cancelled() => {}, _ = resp.closed.cancelled() => {} }
                tokio::time::sleep(Duration::from_millis(50)).await;
                resp.frame(&b).await;
                resp.frame(&result_frame("agent-text", "run-live", "FINISHED", "kept"))
                    .await;
                resp.end("{}").await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await;
    let (tx, rx) = inbox();
    tokio::spawn(async move {
        if wait(&status_sent).await {
            send_steer(&tx, "steer after A").await;
        }
    });
    let (res, evs) = run_with(
        runner_for(&srv, None),
        turn_req("", "do the thing"),
        CancellationToken::new(),
        Some(rx),
    )
    .await;
    let r = ok_result(&res);
    assert!(!r.is_error && r.subtype == "success");
    assert_eq!((calls.sends(), calls.cancels()), (1, 0));
    let mut seq = Vec::new();
    for e in &evs {
        match e {
            AgentEvent::Text { text, .. } => seq.push(format!("text:{text}")),
            AgentEvent::User {
                text,
                live_steer: true,
                ..
            } => seq.push(format!("user:{text}")),
            _ => {}
        }
    }
    assert_eq!(
        seq,
        [
            "text:chunkA".to_owned(),
            format!("user:{}", format_steer_user_message("steer after A")),
            "text:chunkB".to_owned()
        ],
        "pre-steer text must flush at the boundary"
    );
}

// ---- cursor active run

fn conflict_frame() -> String {
    r#"{"error":{"code":"internal","message":"Agent agent-stuck already has active run"}}"#
        .to_owned()
}

fn running_runs(run: &str, created_at: &str) -> String {
    if created_at.is_empty() {
        return format!(
            r#"{{"items":[{{"runId":"{run}","agentId":"agent-stuck","status":"RUN_LIFECYCLE_STATUS_RUNNING"}}]}}"#
        );
    }
    format!(
        r#"{{"items":[{{"runId":"{run}","agentId":"agent-stuck","status":"RUN_LIFECYCLE_STATUS_RUNNING","createdAt":"{created_at}"}}]}}"#
    )
}

fn started_an_hour_ago() -> DateTime<Utc> {
    Utc::now() - chrono::TimeDelta::hours(1)
}

fn minute_after(t: DateTime<Utc>) -> String {
    (t + chrono::TimeDelta::minutes(1))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// A stand-in for the active-run tests. `observe` answers ObserveRun and
/// `send` answers Send attempt `n` (1-based).
async fn stuck_server<O, OF, S, SF>(
    calls: Arc<Calls>,
    runs: String,
    observe: O,
    send: S,
) -> TestServer
where
    O: Fn(Req, Resp) -> OF + Send + Sync + 'static,
    OF: std::future::Future<Output = ()> + Send + 'static,
    S: Fn(usize, Arc<Calls>, Resp) -> SF + Send + Sync + 'static,
    SF: std::future::Future<Output = ()> + Send + 'static,
{
    let observe = Arc::new(observe);
    let send = Arc::new(send);
    serve(move |req, resp| {
        let (c, runs, observe, send) = (calls.clone(), runs.clone(), observe.clone(), send.clone());
        async move {
            let Some(resp) = common(&req, resp, "agent-stuck", &runs).await else {
                return;
            };
            if req.ends("/ObserveRun") {
                c.observes.fetch_add(1, Ordering::SeqCst);
                observe(req, resp).await;
            } else if req.ends("/CancelRun") {
                c.cancels.fetch_add(1, Ordering::SeqCst);
                resp.json("{}").await;
            } else if req.ends("/SteerRun") {
                c.steers.fetch_add(1, Ordering::SeqCst);
                c.steer_calls
                    .lock()
                    .push((req.str("runId"), req.str("text")));
                resp.json(r#"{"outcome":"complete_delivered"}"#).await;
            } else if req.ends("/Send") {
                let n = c.sends.fetch_add(1, Ordering::SeqCst) + 1;
                send(n, c.clone(), resp).await;
            } else {
                panic!("unexpected RPC {}", req.path);
            }
        }
    })
    .await
}

fn finished_stuck(run: &str, text: &str) -> String {
    format!(
        r#"{{"result":{{"agentId":"agent-stuck","runId":"{run}","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{{"runId":"{run}","agentId":"agent-stuck","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"{text}"}}}},"offset":"1"}}"#
    )
}

fn has_notice(evs: &[AgentEvent], text: &str) -> bool {
    evs.iter()
        .any(|e| matches!(e, AgentEvent::Notice { text: t } if t == text))
}

fn has_text(evs: &[AgentEvent], text: &str) -> bool {
    evs.iter()
        .any(|e| matches!(e, AgentEvent::Text { text: t, .. } if t == text))
}

const WAIT_NOTICE: &str = "Waiting for previous run run-live to finish.";
const FALLBACK_NOTICE: &str = "Previous run did not finish; cancelling and retrying.";

/// A same-process live run is drained via ObserveRun (frames discarded),
/// then this turn's prompt is Sent.
#[tokio::test]
async fn reattaches_active_run() {
    let calls = Arc::new(Calls::default());
    let started = started_an_hour_ago();
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-live", &minute_after(started)),
        |_req, mut resp| async move {
            resp.frame(&finished_stuck("run-live", "old-run")).await;
            resp.end("{}").await;
        },
        |n, _c, mut resp| async move {
            if n == 1 {
                resp.end(&conflict_frame()).await;
                return;
            }
            resp.frame(&finished_stuck("run-2", "delivered")).await;
            resp.end("{}").await;
        },
    )
    .await;
    let (res, evs) = run_with(
        runner_for(&srv, Some(started)),
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        None,
    )
    .await;
    let r = ok_result(&res);
    assert_eq!(
        (calls.sends(), calls.observes(), calls.cancels()),
        (2, 1, 0),
        "drain, do not kill"
    );
    assert_eq!(r.text, "delivered", "this turn's Send, not the drained run");
    assert!(has_notice(&evs, WAIT_NOTICE), "{evs:?}");
    assert!(!has_text(&evs, WAIT_NOTICE));
    assert!(!has_notice(&evs, FALLBACK_NOTICE) && !has_text(&evs, FALLBACK_NOTICE));
}

/// ObserveRun ending is not proof the store cleared activeRunId: a second
/// conflict still cancels and retries.
#[tokio::test]
async fn send_after_observe_still_conflicts() {
    let calls = Arc::new(Calls::default());
    let started = started_an_hour_ago();
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-live", &minute_after(started)),
        |_req, mut resp| async move {
            resp.frame(&finished_stuck("run-live", "old-run")).await;
            resp.end("{}").await;
        },
        |n, _c, mut resp| async move {
            if n <= 2 {
                resp.end(&conflict_frame()).await;
                return;
            }
            resp.frame(&finished_stuck("run-3", "recovered")).await;
            resp.end("{}").await;
        },
    )
    .await;
    let (res, evs) = run_with(
        runner_for(&srv, Some(started)),
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        None,
    )
    .await;
    let r = ok_result(&res);
    assert_eq!(
        (calls.observes(), calls.cancels(), calls.sends()),
        (1, 1, 3)
    );
    assert_eq!(r.text, "recovered");
    assert!(has_notice(&evs, FALLBACK_NOTICE));
    assert!(!has_text(&evs, FALLBACK_NOTICE));
}

/// ObserveRun cannot attach: cancel the non-terminal runs, retry Send once.
#[tokio::test]
async fn clears_stale_active_run() {
    let calls = Arc::new(Calls::default());
    let started = started_an_hour_ago();
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-zombie", &minute_after(started)),
        |_req, mut resp| async move {
            resp.end(r#"{"error":{"code":"not_found","message":"Unknown run: run-zombie"}}"#)
                .await;
        },
        |n, _c, mut resp| async move {
            if n == 1 {
                resp.end(&conflict_frame()).await;
                return;
            }
            resp.frame(&finished_stuck("run-2", "recovered")).await;
            resp.end("{}").await;
        },
    )
    .await;
    let (res, _) = run_with(
        runner_for(&srv, Some(started)),
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        None,
    )
    .await;
    assert_eq!(ok_result(&res).text, "recovered");
    assert_eq!(
        (calls.sends(), calls.observes(), calls.cancels()),
        (2, 1, 1)
    );
}

/// A RUNNING row from before this bridge started is a SQLite leftover:
/// cancel + Send, never Observe.
#[tokio::test]
async fn clears_leftover_active_run() {
    let calls = Arc::new(Calls::default());
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-zombie", "2020-01-01T00:00:00Z"),
        |_req, resp| async move { resp.not_found().await },
        |n, _c, mut resp| async move {
            if n == 1 {
                resp.end(&conflict_frame()).await;
                return;
            }
            resp.frame(&finished_stuck("run-2", "recovered")).await;
            resp.end("{}").await;
        },
    )
    .await;
    let (res, _) = run_with(
        runner_for(&srv, Some(started_an_hour_ago())),
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        None,
    )
    .await;
    assert_eq!(ok_result(&res).text, "recovered");
    assert_eq!(
        (calls.observes(), calls.sends(), calls.cancels()),
        (0, 2, 1)
    );
}

/// A missing createdAt fails closed.
#[tokio::test]
async fn skips_observe_without_created_at() {
    let calls = Arc::new(Calls::default());
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-unknown", ""),
        |_req, resp| async move { resp.not_found().await },
        |_n, c, mut resp| async move {
            if c.cancels() == 0 {
                resp.end(&conflict_frame()).await;
                return;
            }
            resp.frame(&finished_stuck("run-2", "recovered")).await;
            resp.end("{}").await;
        },
    )
    .await;
    let (res, _) = run_with(
        runner_for(&srv, Some(started_an_hour_ago())),
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        None,
    )
    .await;
    assert_eq!(ok_result(&res).text, "recovered");
    assert_eq!((calls.observes(), calls.cancels()), (0, 1));
}

/// Progress, then only keepalives without offsets: the idle deadline still
/// fires and recovery falls back to cancel + Send.
#[tokio::test]
async fn observe_idle_timeout() {
    let calls = Arc::new(Calls::default());
    let started = started_an_hour_ago();
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-hung", &minute_after(started)),
        |_req, mut resp| async move {
            resp.frame(&status_frame("agent-stuck", "run-hung")).await;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            loop {
                tokio::select! {
                    _ = resp.closed.cancelled() => return,
                    _ = tokio::time::sleep_until(deadline) => return,
                    _ = tokio::time::sleep(Duration::from_millis(15)) => resp.frame("{}").await,
                }
            }
        },
        |n, _c, mut resp| async move {
            if n == 1 {
                resp.end(&conflict_frame()).await;
                return;
            }
            resp.frame(&finished_stuck("run-2", "recovered")).await;
            resp.end("{}").await;
        },
    )
    .await;
    let mut runner = runner_for(&srv, Some(started));
    runner.observe_idle_timeout = Duration::from_millis(80);
    let (res, _) = run_with(
        runner,
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        None,
    )
    .await;
    assert_eq!(ok_result(&res).text, "recovered");
    assert_eq!(
        (calls.observes(), calls.cancels(), calls.sends()),
        (1, 1, 2)
    );
}

/// Offset frames every 20ms extend a 200ms idle window past what a total
/// budget would allow.
#[tokio::test]
async fn observe_idle_resets_on_offset() {
    let calls = Arc::new(Calls::default());
    let started = started_an_hour_ago();
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-live", &minute_after(started)),
        |_req, mut resp| async move {
            for _ in 0..15 {
                resp.frame(&status_frame("agent-stuck", "run-live")).await;
                tokio::select! {
                    _ = resp.closed.cancelled() => panic!("ObserveRun cancelled while still emitting offset frames"),
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
            resp.frame(&finished_stuck("run-live", "old-run")).await;
            resp.end("{}").await;
        },
        |n, _c, mut resp| async move {
            if n == 1 {
                resp.end(&conflict_frame()).await;
                return;
            }
            resp.frame(&finished_stuck("run-2", "delivered")).await;
            resp.end("{}").await;
        },
    )
    .await;
    let mut runner = runner_for(&srv, Some(started));
    runner.observe_idle_timeout = Duration::from_millis(200);
    let (res, _) = run_with(
        runner,
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        None,
    )
    .await;
    assert_eq!(ok_result(&res).text, "delivered");
    assert_eq!(
        (calls.observes(), calls.cancels(), calls.sends()),
        (1, 0, 2)
    );
}

/// A bridge without run.observe recovers with cancel + Send.
#[tokio::test]
async fn skips_observe_without_capability() {
    let calls = Arc::new(Calls::default());
    let started = started_an_hour_ago();
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-live", &minute_after(started)),
        |_req, resp| async move { resp.not_found().await },
        |_n, c, mut resp| async move {
            if c.cancels() == 0 {
                resp.end(&conflict_frame()).await;
                return;
            }
            resp.frame(&finished_stuck("run-2", "recovered")).await;
            resp.end("{}").await;
        },
    )
    .await;
    let mut runner = runner_for(&srv, Some(started));
    runner.skip_observe = true;
    let (res, _) = run_with(
        runner,
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        None,
    )
    .await;
    assert_eq!(ok_result(&res).text, "recovered");
    assert_eq!((calls.observes(), calls.cancels()), (0, 1));
}

/// Recovery then a live steer: the steer targets the new Send's run, never
/// the leftover.
#[tokio::test]
async fn recover_then_live_steer() {
    let calls = Arc::new(Calls::default());
    let started = started_an_hour_ago();
    let status_sent = CancellationToken::new();
    let ss = status_sent.clone();
    let srv = stuck_server(
        calls.clone(),
        running_runs("run-live", &minute_after(started)),
        |_req, mut resp| async move {
            resp.frame(&finished_stuck("run-live", "old-run")).await;
            resp.end("{}").await;
        },
        move |n, c, mut resp| {
            let ss = ss.clone();
            async move {
                if n == 1 {
                    resp.end(&conflict_frame()).await;
                    return;
                }
                resp.frame(&status_frame("agent-stuck", "run-2")).await;
                ss.cancel();
                // Wait for the SteerRun.
                for _ in 0..250 {
                    if c.steers() > 0 || resp.closed.is_cancelled() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                resp.frame(&finished_stuck("run-2", "delivered")).await;
                resp.end("{}").await;
            }
        },
    )
    .await;
    let (tx, rx) = inbox();
    tokio::spawn(async move {
        if wait(&status_sent).await {
            send_steer(&tx, "after recovery").await;
        }
    });
    let (res, evs) = run_with(
        runner_for(&srv, Some(started)),
        turn_req("agent-stuck", "continue"),
        CancellationToken::new(),
        Some(rx),
    )
    .await;
    let r = ok_result(&res);
    assert!(!r.is_error && r.subtype == "success");
    assert_eq!(
        (calls.sends(), calls.observes(), calls.cancels()),
        (2, 1, 0)
    );
    let ids: Vec<String> = calls
        .steer_calls
        .lock()
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    assert!(
        !ids.contains(&"run-live".to_owned()),
        "recovery must steer the new Send"
    );
    assert_eq!(ids.first().map(String::as_str), Some("run-2"));
    assert!(
        user_texts(&evs, Some(true))
            .iter()
            .any(|t| t.contains("after recovery"))
    );
}

// ---- cursor: options, prompts, contract

fn opts(r: &CursorRunner, req: TurnRequest) -> crate::client::AgentOptions {
    r.agent_options(&req)
}

fn bare() -> CursorRunner {
    CursorRunner::new(Bridge::bound("http://127.0.0.1:9", "", None), "")
}

#[test]
fn agent_options_wire_inline_mcp() {
    let r = CursorRunner::new(Bridge::bound("http://127.0.0.1:9", "", None), "sk-test");
    let o = opts(
        &r,
        TurnRequest {
            agent_id: "LOOP-1-CO".into(),
            model: "composer-2.5".into(),
            flags: [("fast".to_owned(), "true".to_owned())].into(),
            worktree: "/tmp/wt".into(),
            mcp_url: "http://127.0.0.1:5555/tok/mcp".into(),
            ..Default::default()
        },
    );
    assert_eq!(
        o.mcp_servers["recur"].http.as_ref().unwrap().url,
        "http://127.0.0.1:5555/tok/mcp"
    );
    let local = o.local.unwrap();
    assert_eq!(local.cwd, ["/tmp/wt"]);
    assert_eq!(
        local.sandbox,
        Some(crate::client::SandboxOptions { enabled: false })
    );
    let m = o.model.unwrap();
    assert_eq!(m.id, "composer-2.5");
    assert_eq!(m.params.len(), 1);
    assert_eq!(
        (m.params[0].id.as_str(), m.params[0].value.as_str()),
        ("fast", "true")
    );
    assert_eq!(o.api_key, "sk-test");
    assert_eq!(o.name, "LOOP-1-CO");
}

#[test]
fn agent_options_omit_mcp_when_unset() {
    assert!(opts(&bare(), TurnRequest::default()).mcp_servers.is_empty());
    assert!(opts(&bare(), TurnRequest::default()).model.is_none());
}

#[test]
fn agent_options_add_playwright_and_user_mcp() {
    let o = opts(
        &bare(),
        TurnRequest {
            mcp_url: "http://x/tok/mcp".into(),
            playwright: true,
            user_mcp: vec![
                recur_agent::McpAttach {
                    name: "linear".into(),
                    url: "https://mcp.linear.app/mcp/readonly".into(),
                    headers: [("Authorization".to_owned(), "Bearer lin_api_x".to_owned())].into(),
                    ..Default::default()
                },
                recur_agent::McpAttach {
                    name: "fs".into(),
                    command: "npx".into(),
                    args: vec!["-y".into(), "mcp-fs".into()],
                    env: [("FOO".to_owned(), "bar".to_owned())].into(),
                    ..Default::default()
                },
                recur_agent::McpAttach {
                    name: "  ".into(),
                    url: "http://ignored".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    );
    let pw = o.mcp_servers["playwright"].stdio.as_ref().unwrap();
    assert_eq!(pw.command, "npx");
    assert!(
        o.mcp_servers.contains_key("recur"),
        "playwright must not displace recur"
    );
    assert_eq!(
        o.mcp_servers["linear"].http.as_ref().unwrap().headers["Authorization"],
        "Bearer lin_api_x"
    );
    assert_eq!(
        o.mcp_servers["fs"].stdio.as_ref().unwrap().env["FOO"],
        "bar"
    );
    assert_eq!(o.mcp_servers.len(), 4);
    let st = mcp_status_for(&TurnRequest {
        mcp_url: "http://x".into(),
        user_mcp: vec![recur_agent::McpAttach {
            name: "linear".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    assert_eq!(st.len(), 2);
    assert_eq!(
        (st[1].name.as_str(), st[1].status.as_str()),
        ("linear", "connected")
    );
}

#[test]
fn strict_config_opt_in() {
    let loose = opts(&bare(), TurnRequest::default());
    assert!(
        loose.local.unwrap().setting_sources.is_empty(),
        "default must not restrict setting sources"
    );
    let r = bare();
    r.strict_config.store(true, Ordering::SeqCst);
    let local = opts(&r, TurnRequest::default()).local.unwrap();
    assert!(
        !local.setting_sources.is_empty(),
        "strict mode must NAME a layer"
    );
    assert!(
        local
            .setting_sources
            .iter()
            .all(|s| s != crate::client::SETTING_SOURCE_PROJECT
                && s != crate::client::SETTING_SOURCE_USER)
    );
    let b = serde_json::to_string(&local).unwrap();
    assert!(
        b.contains("settingSources") && !b.contains(r#""settingSources":[]"#),
        "{b}"
    );
}

#[test]
fn read_only_role_uses_plan_mode() {
    let mode = |tools: Option<Vec<&str>>| {
        opts(
            &bare(),
            TurnRequest {
                allowed_tools: tools.map(|t| t.into_iter().map(String::from).collect()),
                ..Default::default()
            },
        )
        .mode
    };
    assert_eq!(mode(Some(vec!["Read", "Glob", "Grep"])), "plan");
    assert_eq!(
        mode(Some(vec!["Read", "Bash", "mcp__recur__report_finding"])),
        "plan",
        "bare Bash stays read-only"
    );
    assert_eq!(mode(Some(vec!["Read", "Edit", "Write", "Bash"])), "");
    assert_eq!(mode(Some(vec!["NotebookEdit"])), "");
    assert_eq!(mode(None), "", "no allowlist is unscoped");
    assert_eq!(mode(Some(vec![])), "");
}

#[test]
fn prompt_carries_role_instructions() {
    let got = bare().prompt(&TurnRequest {
        system_prompt: "You are the Reviewer.".into(),
        prompt: "Review the diff.".into(),
        ..Default::default()
    });
    assert_eq!(
        got,
        "<role-instructions>\nYou are the Reviewer.\n</role-instructions>\n\nReview the diff."
    );
}

#[test]
fn prompt_omits_standing_on_resume() {
    let got = bare().prompt(&TurnRequest {
        system_prompt: "You are the Reviewer.".into(),
        append_system_prompt: "This chat is in AGENT mode.".into(),
        prompt: "fix the flake".into(),
        resume_session: "agent-1".into(),
        ..Default::default()
    });
    assert_eq!(got, "fix the flake");
}

#[test]
fn prompt_keeps_turn_subject_on_resume() {
    let got = bare().prompt(&TurnRequest {
        append_system_prompt: "This chat is in AGENT mode.".into(),
        turn_append_prompt: "The operator is looking at GitHub pull request #3.".into(),
        prompt: "what changed?".into(),
        resume_session: "agent-1".into(),
        ..Default::default()
    });
    assert_eq!(
        got,
        "what changed?\n\nThe operator is looking at GitHub pull request #3."
    );
}

#[test]
fn prompt_refresh_standing_on_resume() {
    let got = bare().prompt(&TurnRequest {
        system_prompt: "You are the Coder.".into(),
        append_system_prompt: "This chat is in PLAN mode.".into(),
        prompt: "continue".into(),
        resume_session: "agent-1".into(),
        refresh_standing: true,
        ..Default::default()
    });
    assert!(
        got.contains("You are the Coder.")
            && got.contains("PLAN mode")
            && got.ends_with("continue"),
        "{got}"
    );
}

#[test]
fn prompt_unchanged_without_system_prompt() {
    assert_eq!(
        bare().prompt(&TurnRequest {
            prompt: "Just do it.".into(),
            ..Default::default()
        }),
        "Just do it."
    );
}

#[test]
fn prompt_maps_attachment_markers_to_image_ordinals() {
    let marker = recur_agent::attach::marker("img_msgze3vv_f1gekn");
    let body = format!("what is this image:\n\n{marker}\n\ndo you know?");
    let got = bare().prompt(&TurnRequest {
        prompt: body.clone(),
        images: vec![recur_agent::UserImage {
            id: "img_msgze3vv_f1gekn".into(),
            mime: "image/png".into(),
            data: "AAAA".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    assert!(got.contains(&marker));
    assert!(
        got.contains(&format!("{marker} is image 1 (first attached image)")),
        "{got}"
    );
    assert!(got.contains("NOT files to open or search"));
    assert!(got.ends_with(&body));
    let kept = bare().prompt(&TurnRequest {
        prompt: format!("see {marker}"),
        ..Default::default()
    });
    assert_eq!(kept, format!("see {marker}"));
}

#[test]
fn wire_prompt_subagent_overlay_only_with_create_workspace() {
    let r = bare();
    let req = |tools: &[&str], child: bool| TurnRequest {
        prompt: "hi".into(),
        chat_child: child,
        allowed_tools: Some(tools.iter().map(|t| (*t).to_owned()).collect()),
        ..Default::default()
    };
    let agent = r.wire_prompt(&req(&["Read", MCP_CREATE_WORKSPACE], false), true);
    assert!(agent.contains(CURSOR_SUBAGENT_PROMPT) && agent.contains("<role-instructions>"));
    assert!(!agent.contains(CURSOR_CHILD_SUBAGENT_PROMPT));
    let child = r.wire_prompt(&req(&["Read"], true), true);
    assert!(
        child.contains(CURSOR_CHILD_SUBAGENT_PROMPT)
            && !child.contains("beyond a very simple task")
    );
    let readonly = r.wire_prompt(&req(&["Read"], false), true);
    assert!(
        !readonly.contains(CURSOR_SUBAGENT_PROMPT)
            && !readonly.contains(CURSOR_CHILD_SUBAGENT_PROMPT)
    );
    assert_eq!(
        r.wire_prompt(&req(&[MCP_CREATE_WORKSPACE], false), false),
        "hi"
    );
}

#[tokio::test]
async fn runner_without_bridge_fails_clearly() {
    let r = CursorRunner::new(
        Arc::new(Bridge::with_reaped_for_test(
            CancellationToken::new(),
            false,
        )),
        "",
    );
    let (res, _) = run_with(r, TurnRequest::default(), CancellationToken::new(), None).await;
    assert_eq!(
        res.err().unwrap().err.to_string(),
        "cursor runner: bridge not started"
    );
}

#[test]
fn no_result_message() {
    let hint = " (bridge log: /tmp/bridge.log)";
    let startup = "recur: starting cursor-sdk-bridge bin=/tmp/cursor-sdk-bridge workspace=/tmp/wt\n\
        cursor-sdk-bridge ready {\"authTokenFile\":\"/tmp/auth-token\",\"host\":\"127.0.0.1\",\"pid\":20136,\"port\":54632,\"protocol\":\"connect\"}\n\
        (node:20136) ExperimentalWarning: SQLite is an experimental feature\n\
        cursor-sdk-bridge SIGTERM received, shutting down\n\
        (bridge log: /tmp/bridge.log)";
    let got = cursor_no_result_message(startup, hint);
    assert!(
        !got.contains("authTokenFile") && !got.contains("starting cursor-sdk-bridge"),
        "{got}"
    );
    assert!(
        got.contains("stopped mid-turn") && got.contains("/tmp/bridge.log"),
        "{got}"
    );
    assert_eq!(
        cursor_no_result_message("", hint),
        format!("cursor run ended without a result{hint}")
    );
    let crash = cursor_no_result_message(
        "Error: spawn /bin/zsh ENOENT\n    at ChildProcess._handle.onexit",
        hint,
    );
    assert!(
        crash.contains("Shell spawn ENOENT") && !crash.contains("ChildProcess"),
        "{crash}"
    );
}

#[test]
fn tail_shell_spawn_crash() {
    assert!(bridge_tail_looks_like_shell_spawn_crash(
        "Error: spawn /bin/zsh ENOENT\n    at ChildProcess._handle.onexit"
    ));
    assert!(!bridge_tail_looks_like_shell_spawn_crash(
        "spawn npx ENOENT\nsome later unrelated line\ncode: 'ENOENT'"
    ));
    assert!(!bridge_tail_looks_like_shell_spawn_crash(
        "spawn /bin/zsh\nENOENT elsewhere"
    ));
}

#[tokio::test]
async fn create_agent_failure_is_wrapped_and_resume_falls_back() {
    let resumed = Arc::new(AtomicUsize::new(0));
    let r2 = resumed.clone();
    let srv = serve(move |req, mut resp| {
        let r2 = r2.clone();
        async move {
            if req.ends("/ResumeAgent") {
                r2.fetch_add(1, Ordering::SeqCst);
                resp.status(500, r#"{"code":"not_found","message":"gone"}"#)
                    .await;
            } else if req.ends("/CreateAgent") {
                resp.json(r#"{"agentId":"agent-new"}"#).await;
            } else if req.ends("/CloseAgent") {
                resp.json("{}").await;
            } else if req.ends("/Send") {
                // A fallback agent has no history: standing notes ride again.
                assert!(
                    req.json()["message"]["text"]
                        .as_str()
                        .unwrap()
                        .starts_with("<role-instructions>")
                );
                resp.frame(&result_frame("agent-new", "run-1", "FINISHED", "ok"))
                    .await;
                resp.end("{}").await;
            }
        }
    })
    .await;
    let mut req = turn_req("agent-old", "go");
    req.system_prompt = "role".into();
    let (res, evs) = run_with(runner_for(&srv, None), req, CancellationToken::new(), None).await;
    let r = ok_result(&res);
    assert_eq!(r.session_id, "agent-new");
    assert_eq!(resumed.load(Ordering::SeqCst), 1);
    assert!(
        matches!(&evs[0], AgentEvent::Init { session_id, model, .. } if session_id == "agent-new" && model == "composer-2.5")
    );

    let refused = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let dead = CursorRunner::new(
        Bridge::bound(&format!("http://127.0.0.1:{refused}"), "", None),
        "",
    );
    let (res, _) = run_with(dead, turn_req("", "go"), CancellationToken::new(), None).await;
    let f = res.err().unwrap();
    assert!(
        f.err.to_string().starts_with("cursor create agent: "),
        "{}",
        f.err
    );
    assert!(f.err.bridge_unreachable());
}

#[tokio::test]
async fn auth_and_missing_result_errors() {
    let srv = serve(|req, resp| async move {
        let Some(mut resp) = common(&req, resp, "agent-a", r#"{"items":[]}"#).await else {
            return;
        };
        if req.ends("/Send") {
            resp.end(r#"{"error":{"code":"unauthenticated","message":"bad key"}}"#)
                .await;
        }
    })
    .await;
    let (res, _) = run_with(
        runner_for(&srv, None),
        turn_req("", "go"),
        CancellationToken::new(),
        None,
    )
    .await;
    let f = res.err().unwrap();
    assert_eq!(
        f.err.to_string(),
        "cursor auth failed (reconnect Cursor in Settings): cursor bridge: [unauthenticated] bad key"
    );
    assert!(f.err.connect().unwrap().unauthenticated());

    let srv = serve(|req, resp| async move {
        let Some(mut resp) = common(&req, resp, "agent-b", r#"{"items":[]}"#).await else {
            return;
        };
        if req.ends("/Send") {
            resp.frame(r#"{"sdkMessage":{"type":"assistant","message":{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"half"}]}}}}"#).await;
        }
    })
    .await;
    let (res, evs) = run_with(
        runner_for(&srv, None),
        turn_req("", "go"),
        CancellationToken::new(),
        None,
    )
    .await;
    let f = res.err().unwrap();
    assert_eq!(f.err.to_string(), "cursor run ended without a result");
    assert!(f.partial.is_error && f.partial.text == "half");
    assert!(
        evs.iter()
            .any(|e| matches!(e, AgentEvent::Text { text, .. } if text == "half"))
    );
}

#[test]
fn wrap_attached_images_for_cursor_orders_by_marker() {
    let (m1, m2) = (marker("img_a"), marker("img_b"));
    let input = format!("before {m1} mid {m2} after");
    let img = |id: &str, data: &str| recur_agent::UserImage {
        id: id.into(),
        mime: "image/png".into(),
        data: data.into(),
        ..Default::default()
    };
    let imgs = [img("img_b", "BB"), img("img_a", "AA")];
    let got = wrap_attached_images_for_cursor(&input, &imgs);
    assert!(got.contains("<system_instruction>"));
    assert!(got.contains(&format!("{m1} is image 1 (first attached image)")));
    assert!(got.contains(&format!("{m2} is image 2 (second attached image)")));
    assert!(got.contains(&input), "markers stay in the body");
    assert_eq!(wrap_attached_images_for_cursor(&input, &[]), input);
}
