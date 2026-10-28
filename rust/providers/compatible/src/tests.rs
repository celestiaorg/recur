//! Go `harness_test.go`, `responses/client_test.go` and
//! `app/compatible_service_test.go`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::{
    AgentEvent, EventSink, Provider, Steer, SteerInbox, ToolHost, TurnContext, TurnRequest,
};
use recur_agent_tools::{AskUserRequest, Call, Host, TodoStore, Tool, ToolError};
use recur_domain::{CompatibleModel, CompatibleProvider as Config};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::types::user_text;

/// One recorded request.
#[derive(Debug, Clone, Default)]
pub(crate) struct Seen {
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Seen {
    pub fn header(&self, name: &str) -> String {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

/// A one-request-per-connection HTTP/1.1 server. `answer(n, req)` returns
/// (status, content type, body) for the n-th request (0-based).
pub(crate) async fn serve(
    answer: impl Fn(usize, &Seen) -> (u16, &'static str, String) + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = Arc::clone(&seen);
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            let answer = Arc::clone(&answer);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let req = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                    let mut lines = head.lines();
                    let path = lines
                        .next()
                        .and_then(|l| l.split(' ').nth(1))
                        .unwrap_or_default()
                        .to_owned();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                        .collect();
                    let len: usize = headers
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, v)| v.parse().ok())
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        let body =
                            String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).into_owned();
                        break Seen {
                            path,
                            headers,
                            body,
                        };
                    }
                };
                let n = {
                    let mut l = log.lock();
                    l.push(req.clone());
                    l.len() - 1
                };
                let (status, ct, body) = answer(n, &req);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (base, seen)
}

pub(crate) fn test_config(base: &str) -> Config {
    Config {
        id: "openai".into(),
        name: "OpenAI".into(),
        base_url: base.into(),
        api_key: "sk-test".into(),
        models: vec![CompatibleModel {
            id: "gpt-5".into(),
            label: "GPT-5".into(),
            flags: recur_domain::compatible_flag_preset("openai"),
            ..Default::default()
        }],
        ..Default::default()
    }
}

type Events = Arc<Mutex<Vec<AgentEvent>>>;

fn context(tools: Option<Arc<dyn ToolHost>>) -> (TurnContext, Events, CancellationToken) {
    let events: Events = Arc::default();
    let log = Arc::clone(&events);
    let sink: Arc<dyn EventSink> = Arc::new(move |ev: AgentEvent| log.lock().push(ev));
    let cancel = CancellationToken::new();
    let mut cx = TurnContext::new(cancel.clone(), sink);
    cx.tools = tools;
    (cx, events, cancel)
}

fn harness_tools_for(req: &TurnRequest, host: Option<Arc<dyn Host>>) -> Arc<dyn ToolHost> {
    Arc::new(HarnessTools::new(host, Arc::default(), req))
}

fn call_body(calls: serde_json::Value) -> String {
    json!({"id": "resp-1", "status": "completed", "output": calls}).to_string()
}

fn text_body(id: &str, text: &str) -> String {
    json!({"id": id, "status": "completed", "output": [
        {"type": "message", "content": [{"type": "output_text", "text": text}]}
    ]})
    .to_string()
}

fn sse(frames: &[&str]) -> String {
    frames.iter().map(|f| format!("data: {f}\n\n")).collect()
}

const JSON: &str = "application/json";
const SSE: &str = "text/event-stream";

#[test]
fn ask_mode_omits_edit() {
    let req = TurnRequest::default();
    let tools = HarnessTools::new(None, Arc::default(), &req);
    let allowed: Vec<String> = ["Read", "Glob", "Grep", "mcp__recur__ask_user"]
        .map(String::from)
        .into();
    let names: Vec<String> = tools
        .tools(Some(&allowed))
        .into_iter()
        .map(|t| t.name)
        .collect();
    for banned in ["Edit", "Write", "Bash"] {
        assert!(!names.contains(&banned.to_owned()), "{names:?}");
    }
    assert!(names.contains(&"Read".to_owned()), "{names:?}");
    assert!(
        names.contains(&"mcp__recur__ask_user".to_owned()),
        "{names:?}"
    );
}

#[test]
fn omits_tools_when_disabled() {
    let tools = HarnessTools::new(None, Arc::default(), &TurnRequest::default());
    let all = tools.tools(None);
    assert!(!all.is_empty());
    assert!(!all.iter().any(|t| t.name == "permission_request"));
    let req = TurnRequest {
        tools: Some(String::new()),
        ..Default::default()
    };
    assert!(tools::tools_disabled(&req));
}

#[test]
fn tool_allowed_cases() {
    let req = |tools: Option<&str>, allowed: Option<&[&str]>| TurnRequest {
        tools: tools.map(String::from),
        allowed_tools: allowed.map(|a| a.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    assert!(!tool_allowed_on_turn(
        "Write",
        &req(Some(""), Some(&["Write"]))
    ));
    assert!(!tool_allowed_on_turn("Write", &req(None, Some(&["Read"]))));
    assert!(tool_allowed_on_turn(
        "Read",
        &req(None, Some(&["Read", "Glob"]))
    ));
    assert!(!tool_allowed_on_turn(
        "permission_request",
        &req(None, None)
    ));
}

#[test]
fn fresh_input_subtracts_cache() {
    let u = types::Usage {
        input_tokens: 100,
        cache_read_tokens: 90,
        ..Default::default()
    };
    assert_eq!(responses_fresh_input(u), 10);
}

#[tokio::test]
async fn write_then_text() {
    let wt = tempfile::tempdir().unwrap();
    let (base, _) = serve(|n, req| {
        assert_eq!(req.path, "/v1/responses");
        if n == 0 {
            return (
                200,
                JSON,
                call_body(
                    json!([{"type": "function_call", "id": "fc_1", "call_id": "call_1",
                    "name": "Write", "arguments": r#"{"file_path":"hello.txt","content":"hi"}"#}]),
                ),
            );
        }
        let mut v: serde_json::Value =
            serde_json::from_str(&text_body("resp-2", "Wrote hello.txt")).unwrap();
        v["usage"] = json!({"input_tokens": 10, "output_tokens": 4});
        (200, JSON, v.to_string())
    })
    .await;
    let p = CompatibleProvider::new(test_config(&base));
    let req = TurnRequest {
        model: "gpt-5".into(),
        prompt: "write hello".into(),
        worktree: wt.path().into(),
        allowed_tools: Some(
            ["Read", "Edit", "Write", "Glob", "Grep", "Bash"]
                .map(String::from)
                .into(),
        ),
        ..Default::default()
    };
    let (cx, events, _) = context(Some(harness_tools_for(&req, None)));
    let res = p.run_turn(req, cx).await.unwrap();
    assert_eq!(res.text, "Wrote hello.txt");
    let steps: Vec<String> = events
        .lock()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Step(s) => Some(s.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(steps, ["Write"]);
    assert_eq!(
        std::fs::read_to_string(wt.path().join("hello.txt")).unwrap(),
        "hi"
    );
}

#[tokio::test]
async fn plain_steer_decodes_display() {
    let display = "user text";
    let prompt = format!(
        "The operator is looking at this workspace.\n\n<system_instruction>\nattached\n</system_instruction>\n\n{display}"
    );
    let (base, seen) = serve(|n, _| {
        if n == 0 {
            return (
                200,
                JSON,
                call_body(
                    json!([{"type": "function_call", "id": "fc_1", "call_id": "call_1",
                    "name": "Read", "arguments": r#"{"file_path":"x"}"#}]),
                ),
            );
        }
        (200, JSON, text_body("resp-2", "ok"))
    })
    .await;
    let p = CompatibleProvider::new(test_config(&base));
    let wt = tempfile::tempdir().unwrap();
    let req = TurnRequest {
        model: "gpt-5".into(),
        prompt: "go".into(),
        worktree: wt.path().into(),
        allowed_tools: Some(vec!["Read".into()]),
        ..Default::default()
    };
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let (steer, ack) = Steer::new(prompt.clone(), Some(display.into()));
    tx.send(steer).await.unwrap();
    let (mut cx, events, _) = context(Some(harness_tools_for(&req, None)));
    cx.steer = Some(SteerInbox(rx));
    p.run_turn(req, cx).await.unwrap();
    assert!(ack.await.unwrap(), "delivered steer is acked");
    let users: Vec<(String, String)> = events
        .lock()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::User { text, display, .. } => Some((text.clone(), display.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(users, [(prompt.clone(), display.to_owned())]);
    let second: serde_json::Value = serde_json::from_str(&seen.lock()[1].body).unwrap();
    let last = second["input"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"][0]["text"], prompt);
}

/// A host that answers ask_user like a parked prompt.
#[derive(Default)]
struct AskHost {
    asked: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl Host for AskHost {
    async fn ask_user(&self, _ws: &str, _agent: &str, req: AskUserRequest) -> String {
        self.asked.lock().push(format!("{req:?}"));
        "The question is shown to the user; their answer arrives as your next message.".into()
    }
}

#[tokio::test]
async fn ask_user_halts() {
    let (base, seen) = serve(|_, _| {
        (
            200,
            JSON,
            call_body(
                json!([{"type": "function_call", "id": "fc_ask", "call_id": "call_ask",
                "name": "ask_user", "arguments": r#"{"question":"Which file?"}"#}]),
            ),
        )
    })
    .await;
    let p = CompatibleProvider::new(test_config(&base));
    let host = Arc::new(AskHost::default());
    let req = TurnRequest {
        model: "gpt-5".into(),
        prompt: "ask me".into(),
        allowed_tools: Some(vec!["mcp__recur__ask_user".into(), "Read".into()]),
        workspace_id: "W1".into(),
        agent_id: "W1-PA".into(),
        ..Default::default()
    };
    let (cx, events, _) = context(Some(harness_tools_for(&req, Some(host.clone()))));
    let res = p.run_turn(req, cx).await.unwrap();
    assert!(!res.is_error, "{res:?}");
    assert!(!events.lock().is_empty());
    assert_eq!(host.asked.lock().len(), 1, "ask_user reached the host");
    assert_eq!(seen.lock().len(), 1, "the loop halted after ask_user");
}

async fn run_one(
    answer: (u16, &'static str, String),
    allowed: &[&str],
) -> (Result<TurnResult, ProviderError>, tempfile::TempDir) {
    let body = answer.2.clone();
    let (status, ct) = (answer.0, answer.1);
    let (base, _) = serve(move |_, _| (status, ct, body.clone())).await;
    let p = CompatibleProvider::new(test_config(&base));
    let wt = tempfile::tempdir().unwrap();
    let req = TurnRequest {
        model: "gpt-5".into(),
        prompt: "go".into(),
        worktree: wt.path().into(),
        allowed_tools: Some(allowed.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    let (cx, _, _) = context(Some(harness_tools_for(&req, None)));
    (p.run_turn(req, cx).await, wt)
}

#[tokio::test]
async fn rejects_failed_response() {
    let body = json!({"id": "resp-fail", "status": "failed", "error": {"message": "overloaded"},
        "output": [{"type": "function_call", "call_id": "call_1", "name": "Write",
            "arguments": r#"{"file_path":"x","content":"y"}"#}]});
    let (res, wt) = run_one((200, JSON, body.to_string()), &["Write"]).await;
    let err = res.unwrap_err();
    assert!(err.message.contains("overloaded"), "{err}");
    assert!(
        !wt.path().join("x").exists(),
        "failed response must not execute Write"
    );
}

#[tokio::test]
async fn rejects_incomplete_response() {
    let body = json!({"id": "resp-incomplete", "status": "incomplete",
        "output": [{"type": "function_call", "call_id": "call_1", "name": "Write",
            "arguments": r#"{"file_path":"x","content":"y"}"#}]});
    let (res, wt) = run_one((200, JSON, body.to_string()), &["Write"]).await;
    let err = res.unwrap_err();
    assert!(err.message.contains("incomplete"), "{err}");
    assert!(!wt.path().join("x").exists());
}

#[tokio::test]
async fn rejects_empty_json() {
    let (res, _) = run_one((200, JSON, "{}".into()), &[]).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn rejects_premature_stream() {
    let body = "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"Write\",\"arguments\":\"\"}}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"file_path\\\":\\\"x\\\",\\\"content\\\":\\\"y\\\"}\"}\n\n";
    let (res, wt) = run_one((200, SSE, body.into()), &["Write"]).await;
    let err = res.unwrap_err();
    assert!(err.message.contains("before completion"), "{err}");
    assert!(!wt.path().join("x").exists());
}

async fn run_text(body: String) -> (TurnResult, Vec<AgentEvent>) {
    let (base, _) = serve(move |_, _| (200, SSE, body.clone())).await;
    let p = CompatibleProvider::new(test_config(&base));
    let (cx, events, _) = context(None);
    let res = p
        .run_turn(
            TurnRequest {
                model: "gpt-5".into(),
                prompt: "go".into(),
                ..Default::default()
            },
            cx,
        )
        .await
        .unwrap();
    let evs = events.lock().clone();
    (res, evs)
}

#[tokio::test]
async fn reasoning_done_not_duplicated() {
    let (res, evs) = run_text(sse(&[
        r#"{"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"think"}"#,
        r#"{"type":"response.reasoning_summary_text.done","item_id":"rs_1","text":"think"}"#,
        r#"{"type":"response.output_text.delta","item_id":"msg_1","delta":"hi"}"#,
        r#"{"type":"response.completed","response":{"id":"r1","status":"completed"}}"#,
    ]))
    .await;
    let thinking: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking, ["think"]);
    assert_eq!(res.text, "hi");
}

#[tokio::test]
async fn keeps_every_final_message() {
    let (res, _) = run_text(sse(&[
        r#"{"type":"response.output_item.done","item":{"id":"msg_1","type":"message","content":[{"type":"output_text","text":"hello"}]}}"#,
        r#"{"type":"response.output_item.done","item":{"id":"msg_2","type":"message","content":[{"type":"output_text","text":"world"}]}}"#,
        r#"{"type":"response.completed","response":{"id":"r1","status":"completed"}}"#,
    ]))
    .await;
    assert_eq!(res.text, "helloworld");
}

#[tokio::test]
async fn mixed_stream_and_final_messages() {
    let (res, _) = run_text(sse(&[
        r#"{"type":"response.output_text.delta","item_id":"msg_1","delta":"hello"}"#,
        r#"{"type":"response.output_item.done","item":{"id":"msg_1","type":"message","content":[{"type":"output_text","text":"hello"}]}}"#,
        r#"{"type":"response.output_item.done","item":{"id":"msg_2","type":"message","content":[{"type":"output_text","text":"world"}]}}"#,
        r#"{"type":"response.completed","response":{"id":"r1","status":"completed"}}"#,
    ]))
    .await;
    assert_eq!(res.text, "helloworld");
}

#[tokio::test]
async fn dispatch_enforces_allowlist() {
    let body = call_body(
        json!([{"type": "function_call", "call_id": "call_1", "name": "Write",
        "arguments": r#"{"file_path":"x.txt","content":"nope"}"#}]),
    );
    let (base, _) = serve(move |_, _| (200, JSON, body.clone())).await;
    let p = CompatibleProvider::new(test_config(&base));
    let wt = tempfile::tempdir().unwrap();
    let req = TurnRequest {
        model: "gpt-5".into(),
        prompt: "write".into(),
        worktree: wt.path().into(),
        allowed_tools: Some(vec!["Read".into()]),
        max_turns: 1,
        ..Default::default()
    };
    let (cx, _, _) = context(Some(harness_tools_for(&req, None)));
    let res = p.run_turn(req, cx).await.unwrap();
    assert_eq!(res.subtype, "error_max_turns");
    assert!(res.is_error);
    assert!(
        !wt.path().join("x.txt").exists(),
        "Write must not run when not allowed"
    );
}

#[tokio::test]
async fn stop_during_bash_does_not_write() {
    let wt = tempfile::tempdir().unwrap();
    let body = call_body(json!([
        {"type": "function_call", "call_id": "call_bash", "name": "Bash",
            "arguments": r#"{"command":"touch started; sleep 30"}"#},
        {"type": "function_call", "call_id": "call_write", "name": "Write",
            "arguments": r#"{"file_path":"after.txt","content":"nope"}"#},
    ]));
    let (base, _) = serve(move |_, _| (200, JSON, body.clone())).await;
    let p = CompatibleProvider::new(test_config(&base));
    let req = TurnRequest {
        model: "gpt-5".into(),
        prompt: "go".into(),
        worktree: wt.path().into(),
        allowed_tools: Some(vec!["Bash".into(), "Write".into()]),
        permission_mode: recur_domain::PERMISSION_MODE_BYPASS.into(),
        ..Default::default()
    };
    let (cx, _, cancel) = context(Some(harness_tools_for(&req, None)));
    let started = wt.path().join("started");
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline && !started.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        cancel.cancel();
    });
    let err = p.run_turn(req, cx).await.unwrap_err();
    assert_eq!(err.kind, recur_agent::ErrorKind::Canceled, "{err}");
    assert!(
        !wt.path().join("after.txt").exists(),
        "canceled Bash must not run the queued Write"
    );
}

/// A Read stand-in that blocks until released, counting concurrency.
struct GateTool {
    active: Arc<AtomicUsize>,
    max_seen: Arc<AtomicUsize>,
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: CancellationToken,
}

#[async_trait::async_trait]
impl Tool for GateTool {
    fn name(&self) -> &'static str {
        "Read"
    }
    fn description(&self) -> &'static str {
        "Read"
    }
    fn schema(&self) -> String {
        r#"{"type":"object"}"#.into()
    }
    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let n = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_seen.fetch_max(n, Ordering::SeqCst);
        let _ = self.started.send(());
        tokio::select! {
            _ = c.cancel.cancelled() => {}
            _ = self.release.cancelled() => {}
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok("ok".into())
    }
}

#[tokio::test]
async fn parallel_tools_cap() {
    const N: usize = 16;
    let active = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let release = CancellationToken::new();
    let req = TurnRequest::default();
    let host: Arc<dyn ToolHost> = Arc::new(HarnessTools::with_tools(
        vec![Arc::new(GateTool {
            active: Arc::clone(&active),
            max_seen: Arc::clone(&max_seen),
            started: started_tx,
            release: release.clone(),
        })],
        None,
        Arc::new(TodoStore::default()),
        &req,
    ));
    let calls: Vec<types::FunctionCall> = (0..N)
        .map(|i| types::FunctionCall {
            call_id: format!("c{i}"),
            name: "Read".into(),
            arguments: "{}".into(),
            ..Default::default()
        })
        .collect();
    let sink: Arc<dyn EventSink> = Arc::new(|_: AgentEvent| {});
    let emitter = Arc::new(harness::TurnEmitter::new(sink));
    let cancel = CancellationToken::new();
    let run = tokio::spawn({
        let (req, cancel, host, emitter) =
            (req.clone(), cancel.clone(), host.clone(), emitter.clone());
        async move { harness::exec_calls(&req, &cancel, Some(&host), &calls, &emitter).await }
    });
    for i in 0..HARNESS_PARALLEL_LIMIT {
        tokio::time::timeout(Duration::from_secs(2), started.recv())
            .await
            .unwrap_or_else(|_| {
                panic!("started {i} parallel Reads, want {HARNESS_PARALLEL_LIMIT}")
            });
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), started.recv())
            .await
            .is_err(),
        "a Read beyond the limit started before a slot freed"
    );
    assert_eq!(max_seen.load(Ordering::SeqCst), HARNESS_PARALLEL_LIMIT);
    assert_eq!(active.load(Ordering::SeqCst), HARNESS_PARALLEL_LIMIT);
    release.cancel();
    let results = tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(results.len(), N);
    assert!(results.iter().all(|r| r.text == "ok"));
    assert!(max_seen.load(Ordering::SeqCst) <= HARNESS_PARALLEL_LIMIT);
}

#[tokio::test]
async fn without_a_tool_host_calls_fail_cleanly() {
    let body = call_body(
        json!([{"type": "function_call", "call_id": "c", "name": "Read", "arguments": "{}"}]),
    );
    let (base, _) = serve(move |n, _| {
        if n == 0 {
            (200, JSON, body.clone())
        } else {
            (200, JSON, text_body("r2", "done"))
        }
    })
    .await;
    let p = CompatibleProvider::new(test_config(&base));
    let (cx, events, _) = context(None);
    let res = p
        .run_turn(
            TurnRequest {
                model: "gpt-5".into(),
                prompt: "go".into(),
                ..Default::default()
            },
            cx,
        )
        .await
        .unwrap();
    assert_eq!(res.text, "done");
    assert!(events.lock().iter().any(|e| matches!(e,
        AgentEvent::ToolResult(r) if r.is_error && r.text == "unknown tool: Read")));
}

#[test]
fn availability_wording() {
    let p = CompatibleProvider::new(test_config("https://api.openai.com"));
    let mut s = recur_domain::Settings::default();
    assert_eq!(
        p.availability(&s),
        recur_agent::Availability::Unavailable(
            "unknown agent provider \"openai\" — pick a model from Settings → Models".into()
        )
    );
    s.compatible_providers = vec![test_config("https://api.openai.com")];
    assert_eq!(p.availability(&s), recur_agent::Availability::Ready);
    s.disabled_providers = vec!["openai".into()];
    assert_eq!(
        p.availability(&s),
        recur_agent::Availability::Unavailable(
            "OpenAI is turned off in Settings → Providers".into()
        )
    );
    s.disabled_providers.clear();
    s.compatible_providers[0].models.clear();
    assert_eq!(
        p.availability(&s),
        recur_agent::Availability::Unavailable(
            "OpenAI is not configured (Settings → Providers)".into()
        )
    );
}

#[test]
fn provider_metadata() {
    let mut cfg = test_config("https://x");
    cfg.models[0].context_window = 400_000;
    let p = CompatibleProvider::new(cfg);
    assert_eq!((p.id(), p.label()), ("openai", "OpenAI"));
    assert_eq!(p.context_window("gpt-5", &Default::default()), 400_000);
    assert_eq!(p.context_window("nope", &Default::default()), 0);
    assert_eq!(p.model_label("gpt-5"), "GPT-5");
    assert_eq!(p.model_label("other"), "other");
    assert!(p.resume_handle_fits(" "));
    assert!(!p.resume_handle_fits("resp_1"));
    assert_eq!(p.display_tool("Agent", "{}").name, "Task");
    assert_eq!(p.caps().history, recur_agent::HistoryMode::App);
}

// ---- responses/client_test.go

#[tokio::test]
async fn client_stream_sse() {
    let (base, seen) = serve(|_, req| {
        if req.header("authorization") != "Bearer sk-test" {
            return (401, "text/plain", "unauthorized".into());
        }
        (200, SSE, sse(&[
            r#"{"type":"response.output_text.delta","delta":"ok"}"#,
            r#"{"type":"response.completed","response":{"id":"r1","usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ]))
    })
    .await;
    let c = Client::new(&test_config(&base));
    let mut deltas = String::new();
    c.stream(
        &CancellationToken::new(),
        types::Request {
            model: "gpt-5".into(),
            input: vec![user_text("hi")],
            ..Default::default()
        },
        &mut |e| deltas.push_str(&e.delta),
    )
    .await
    .unwrap();
    assert_eq!(deltas, "ok");
    let s = seen.lock()[0].clone();
    assert_eq!(s.path, "/v1/responses");
    assert_eq!(s.header("accept"), "text/event-stream");
    assert!(
        s.header("accept-encoding").is_empty(),
        "never ask for a compressed stream"
    );
    let body: serde_json::Value = serde_json::from_str(&s.body).unwrap();
    assert_eq!(
        (body["stream"].clone(), body["store"].clone()),
        (json!(true), json!(false))
    );
}

#[tokio::test]
async fn client_stream_json_fallback() {
    let (base, _) = serve(|_, _| (200, JSON, text_body("r2", "json"))).await;
    let c = Client::new(&Config {
        base_url: base,
        ..Default::default()
    });
    let mut text = String::new();
    c.stream(
        &CancellationToken::new(),
        types::Request {
            model: "m".into(),
            ..Default::default()
        },
        &mut |e| text.push_str(&e.delta),
    )
    .await
    .unwrap();
    assert_eq!(text, "json");
}

#[tokio::test]
async fn client_stream_rejects_empty_json() {
    let (base, _) = serve(|_, _| (200, JSON, "{}".into())).await;
    let c = Client::new(&Config {
        base_url: base,
        ..Default::default()
    });
    let err = c
        .stream(
            &CancellationToken::new(),
            types::Request::default(),
            &mut |_| {},
        )
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("responses decode: "), "{err}");
}

#[tokio::test]
async fn client_stream_http_error_text() {
    let (base, _) = serve(|_, _| (401, "text/plain", "nope\n".into())).await;
    let c = Client::new(&Config {
        base_url: base,
        ..Default::default()
    });
    let err = c
        .stream(
            &CancellationToken::new(),
            types::Request::default(),
            &mut |_| {},
        )
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "responses 401 Unauthorized: nope");
}

async fn ping(answer: (u16, &'static str, String)) -> Result<(), String> {
    let (base, _) = serve(move |_, _| answer.clone()).await;
    Client::new(&Config {
        base_url: base,
        ..Default::default()
    })
    .ping("gpt-5")
    .await
}

#[tokio::test]
async fn client_ping_cases() {
    assert!(ping((401, "text/plain", "nope".into())).await.is_err());
    let err = ping((
        200,
        JSON,
        json!({"id": "r-fail", "status": "failed", "error": {"message": "model not found"}})
            .to_string(),
    ))
    .await
    .unwrap_err();
    assert!(err.contains("model not found"), "{err}");
    assert!(
        ping((200, "text/html", "<html>not json</html>".into()))
            .await
            .is_err()
    );
    assert!(ping((200, JSON, "{}".into())).await.is_err());
    assert!(
        ping((
            200,
            JSON,
            json!({"id": "r", "status": "queued", "output": []}).to_string()
        ))
        .await
        .is_err()
    );
    ping((
        200,
        JSON,
        json!({"id": "r-inc", "status": "incomplete", "output": []}).to_string(),
    ))
    .await
    .unwrap();
    let c = Client::new(&Config::default());
    assert_eq!(c.ping(" ").await.unwrap_err(), "model is required");
}

#[test]
fn endpoint_trims_slash() {
    let c = Client::new(&Config {
        base_url: "https://api.openai.com/".into(),
        ..Default::default()
    });
    assert_eq!(c.endpoint(), "https://api.openai.com/v1/responses");
}

// ---- app/compatible_service_test.go

#[tokio::test]
async fn test_compatible_provider_hits_responses() {
    let (base, seen) = serve(|_, _| {
        (
            200,
            JSON,
            json!({"id": "r", "status": "completed", "output": []}).to_string(),
        )
    })
    .await;
    let got = test_compatible_provider(Config {
        id: "openai".into(),
        base_url: base,
        models: vec![CompatibleModel {
            id: "gpt-5".into(),
            ..Default::default()
        }],
        ..Default::default()
    })
    .await;
    assert!(got.ok, "{got:?}");
    assert_eq!(seen.lock()[0].path, "/v1/responses");
}

#[tokio::test]
async fn test_compatible_provider_needs_model() {
    let got = test_compatible_provider(Config {
        id: "openai".into(),
        base_url: "https://api.openai.com".into(),
        ..Default::default()
    })
    .await;
    assert!(!got.ok && !got.error.is_empty(), "{got:?}");
}

#[test]
fn test_result_json() {
    assert_eq!(
        serde_json::to_string(&CompatibleTestResult {
            ok: true,
            error: String::new()
        })
        .unwrap(),
        r#"{"ok":true}"#
    );
}
