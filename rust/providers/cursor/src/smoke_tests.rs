//! Live smoke tests against a REAL cursor-sdk-bridge (Go
//! `cursor/bridge_smoke_test.go`, `run_smoke_test.go`,
//! `engine/cursor_smoke_test.go`). Skipped unless `RECUR_CURSOR_BRIDGE`
//! points at an unpacked bridge; the credentialed ones also need
//! `CURSOR_API_KEY` and cost real Cursor usage:
//!
//! ```sh
//! pip download cursor-sdk --no-deps -d /tmp/cs && unzip -q /tmp/cs/*.whl -d /tmp/cs/w
//! RECUR_CURSOR_BRIDGE=/tmp/cs/w/cursor_sdk/_vendor/bridge CURSOR_API_KEY=… \
//!   cargo test -p recur-provider-cursor smoke -- --nocapture
//! ```
//!
//! The live run tests assert on the MCP server's own wire log, not only the
//! model's answer: an early version passed because the model shelled out to
//! curl instead of using MCP.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::{AgentEvent, TurnRequest};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::bridge::{Bridge, BridgeConfig};
use crate::client::{
    AgentOptions, LocalOptions, McpServer, ModelSelection, SETTING_SOURCES_MINIMAL,
};
use crate::events::{MSG_STATUS, MSG_TOOL_CALL, STATUS_RUNNING, TOOL_RUNNING};
use crate::locate::{ENV_BRIDGE_PATH, resolve_launcher};
use crate::testserver::{TestServer, serve};
use crate::turn::tests::Rec;

fn require_bridge() -> Option<PathBuf> {
    let Some(p) = std::env::var_os(ENV_BRIDGE_PATH) else {
        eprintln!("skipped: {ENV_BRIDGE_PATH} not set");
        return None;
    };
    Some(
        resolve_launcher(&PathBuf::from(&p))
            .unwrap_or_else(|| panic!("not a cursor-sdk-bridge: {p:?}")),
    )
}

fn require_key() -> Option<String> {
    let k = std::env::var("CURSOR_API_KEY").unwrap_or_default();
    if k.is_empty() {
        eprintln!("skipped: CURSOR_API_KEY not set");
        return None;
    }
    Some(k)
}

fn test_model() -> String {
    std::env::var("RECUR_CURSOR_TEST_MODEL")
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "composer-2.5".into())
}

async fn start(bin: PathBuf, key: &str, workspace: PathBuf) -> Arc<Bridge> {
    tokio::time::timeout(
        Duration::from_secs(90),
        Bridge::start(
            BridgeConfig {
                bin,
                api_key: key.to_owned(),
                workspace,
                ..Default::default()
            },
            &CancellationToken::new(),
        ),
    )
    .await
    .expect("bridge start timed out")
    .expect("start bridge")
}

/// Spawn, discovery and the control RPCs; no credentials needed.
#[tokio::test]
async fn bridge_smoke() {
    let Some(bin) = require_bridge() else { return };
    let ws = tempfile::tempdir().unwrap();
    let b = start(bin, "", ws.path().to_path_buf()).await;
    let d = b.discovery();
    assert!(d.port != 0 && !d.url.is_empty(), "{d:?}");
    assert_eq!(
        (d.transport.as_str(), d.protocol.as_str()),
        ("tcp", "connect")
    );
    let cl = b.connect_client().unwrap();
    let tok = CancellationToken::new();
    let v = cl.get_version(&tok).await.expect("GetVersion");
    assert_eq!(
        v.protocol_version, "sdk.v1",
        "the bridge changed its wire contract"
    );
    // The capabilities the runner depends on, by name.
    for want in ["agent.create", "agent.send", "run.cancel", "run.observe"] {
        assert!(
            v.has(want),
            "bridge missing capability {want:?} (has {:?})",
            v.capabilities
        );
    }
    cl.ping(&tok).await.expect("Ping");
    b.close().await;
}

#[tokio::test]
async fn bridge_catalog_smoke() {
    let (Some(bin), Some(key)) = (require_bridge(), require_key()) else {
        return;
    };
    let ws = tempfile::tempdir().unwrap();
    let b = start(bin, &key, ws.path().to_path_buf()).await;
    let models = b
        .connect_client()
        .unwrap()
        .list_models(&CancellationToken::new(), &key)
        .await
        .expect("ListModels");
    assert!(!models.is_empty(), "ListModels returned no models");
    eprintln!(
        "{} models, first = {} ({}) params={} variants={}",
        models.len(),
        models[0].id,
        models[0].display_name,
        models[0].parameters.len(),
        models[0].variants.len()
    );
    b.close().await;
}

/// A minimal streamable-HTTP MCP server shaped like Recur's: POST/JSON, 202
/// for notifications, token in the path.
struct StubMcp {
    url: String,
    methods: Arc<Mutex<Vec<String>>>,
    _srv: TestServer,
}

async fn start_stub_mcp() -> StubMcp {
    let methods = Arc::new(Mutex::new(Vec::new()));
    let m = methods.clone();
    let srv = serve(move |req, resp| {
        let m = m.clone();
        async move {
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let method = body["method"].as_str().unwrap_or("").to_owned();
            m.lock().push(method.clone());
            let result = match method.as_str() {
                "initialize" => json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "recur", "version": "1"},
                }),
                "tools/list" => json!({"tools": [{
                    "name": "magic_number",
                    "description": "Returns the project's magic number.",
                    "inputSchema": {"type": "object", "properties": {}},
                }]}),
                "tools/call" => {
                    json!({"content": [{"type": "text", "text": "The magic number is 4271."}]})
                }
                _ => json!({}),
            };
            if body["id"].is_null() {
                return resp.status(202, "").await;
            }
            resp.json(&json!({"jsonrpc": "2.0", "id": body["id"], "result": result}).to_string())
                .await;
        }
    })
    .await;
    StubMcp {
        url: format!("{}/tok/mcp", srv.url),
        methods,
        _srv: srv,
    }
}

fn has_all(hay: &[String], want: &[&str]) -> bool {
    want.iter().all(|w| hay.iter().any(|h| h == w))
}

/// The load-bearing test for the bridge route: an agent picks up an MCP
/// server passed INLINE (no .cursor/mcp.json anywhere) and calls it.
#[tokio::test]
async fn run_with_inline_mcp_smoke() {
    let (Some(bin), Some(key)) = (require_bridge(), require_key()) else {
        return;
    };
    let mcp = start_stub_mcp().await;
    let work = tempfile::tempdir().unwrap();
    let b = start(bin, &key, work.path().to_path_buf()).await;
    let cl = b.connect_client().unwrap();
    let tok = CancellationToken::new();
    let agent = cl
        .create_agent(
            &tok,
            &AgentOptions {
                model: Some(ModelSelection {
                    id: "composer-2.5".into(),
                    ..Default::default()
                }),
                api_key: key.clone(),
                name: "recur-smoke".into(),
                local: Some(LocalOptions {
                    cwd: vec![work.path().to_string_lossy().into_owned()],
                    // The narrowest ambient config: no stray developer config.
                    setting_sources: SETTING_SOURCES_MINIMAL
                        .iter()
                        .map(|s| (*s).to_owned())
                        .collect(),
                    ..Default::default()
                }),
                mcp_servers: [("recur".to_owned(), McpServer::http(&mcp.url))].into(),
                ..Default::default()
            },
        )
        .await
        .expect("CreateAgent");
    let mut tool_calls = Vec::new();
    let mut saw_running = false;
    let mut last = None;
    tokio::time::timeout(
        Duration::from_secs(300),
        cl.send(
            &tok,
            &agent,
            "Call the magic_number tool and reply with only the number it returns. Do not use the shell.",
            &[],
            |ev| {
                if let Some(r) = ev.result.and_then(|r| r.result) {
                    last = Some(r);
                }
                if let Some(m) = ev.sdk_message {
                    if m.kind == MSG_STATUS && m.status == STATUS_RUNNING {
                        saw_running = true;
                    }
                    if m.kind == MSG_TOOL_CALL && m.status == TOOL_RUNNING {
                        tool_calls.push(m.tool_label());
                    }
                }
            },
        ),
    )
    .await
    .expect("run timed out")
    .expect("Send");
    let _ = cl.close_agent(&tok, &agent).await;
    assert!(saw_running, "never saw a status/RUNNING event");
    let fin = last.expect("run ended without a result");
    assert!(!fin.is_error(), "run failed: {:?}", fin.error);
    assert!(fin.result.contains("4271"), "{}", fin.result);
    let methods = mcp.methods.lock().clone();
    assert!(
        has_all(&methods, &["initialize", "tools/list"]),
        "inline mcpServers did not take effect: {methods:?}"
    );
    assert!(
        methods.iter().any(|m| m == "tools/call"),
        "MCP tool never invoked: {methods:?}"
    );
    assert!(
        tool_calls.iter().any(|t| t == "mcp__recur__magic_number"),
        "{tool_calls:?}"
    );
    b.close().await;
}

async fn live_run(
    bin: PathBuf,
    key: &str,
    worktree: PathBuf,
    req: TurnRequest,
) -> (recur_agent::TurnResult, Vec<AgentEvent>) {
    let b = start(bin, key, worktree).await;
    let runner = Arc::new(crate::runner::CursorRunner::new(b.clone(), key));
    let rec = Rec::default();
    let res = tokio::time::timeout(
        Duration::from_secs(300),
        runner.run(&req, &CancellationToken::new(), rec.sink(), &mut None),
    )
    .await
    .expect("run timed out");
    b.close().await;
    match res {
        Ok(r) => (r, rec.all()),
        Err(f) => panic!("Run: {}", f.err),
    }
}

fn req(ws: &str, worktree: &std::path::Path, prompt: &str) -> TurnRequest {
    TurnRequest {
        workspace_id: ws.into(),
        agent_id: format!("{ws}-CO"),
        role: recur_domain::AgentRole::Coder,
        model: test_model(),
        worktree: worktree.to_path_buf(),
        prompt: prompt.into(),
        ..Default::default()
    }
}

/// Built-in tools reach the feed with canonical names, canonical inputs and
/// non-empty results; one step per call.
#[tokio::test]
async fn tool_rendering_smoke() {
    let (Some(bin), Some(key)) = (require_bridge(), require_key()) else {
        return;
    };
    let wt = tempfile::tempdir().unwrap();
    std::fs::write(wt.path().join("hello.txt"), "alpha\nbravo\ncharlie\n").unwrap();
    let (_, evs) = live_run(
        bin,
        &key,
        wt.path().to_path_buf(),
        req(
            "LOOP-TOOLS",
            wt.path(),
            "Do exactly these, each as its own tool call: read hello.txt; replace 'bravo' with 'BRAVO' in hello.txt; run `echo hi` in the shell. Then reply DONE.",
        ),
    )
    .await;
    let mut steps: HashMap<String, (String, String, String)> = HashMap::new();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut results: HashMap<String, String> = HashMap::new();
    for e in &evs {
        match e {
            AgentEvent::Step(s) => {
                *counts.entry(s.id.clone()).or_default() += 1;
                steps.insert(
                    s.id.clone(),
                    (s.name.clone(), s.input.clone(), s.summary.clone()),
                );
            }
            AgentEvent::ToolResult(r) => {
                results.insert(r.id.clone(), r.text.clone());
            }
            _ => {}
        }
    }
    let mut by_tool = HashMap::new();
    for (id, (raw, input, summary)) in &steps {
        let d = crate::tools::display_tool(raw, input);
        let result =
            crate::tools::cursor_tool_result_text(raw, results.get(id).map_or("", String::as_str));
        eprintln!(
            "raw={raw:<8} → {:<6} | rawArgs={input} | input={} | result={result:.40?}",
            d.name, d.input
        );
        assert!(
            !raw.is_empty() && *raw != d.name,
            "the event lost the provider's raw name"
        );
        let raw_result = results.get(id).cloned().unwrap_or_default();
        assert!(!raw_result.is_empty(), "call {id} ({raw}) got no result");
        assert!(
            raw_result.contains(r#""status""#),
            "lost Cursor's envelope: {raw_result:.80}"
        );
        by_tool.insert(d.name.clone(), (d.input, summary.clone(), result));
    }
    for want in ["Read", "Edit", "Bash"] {
        assert!(
            by_tool.contains_key(want),
            "no {want} step (tools: {:?})",
            by_tool.keys()
        );
    }
    assert!(
        counts.values().all(|n| *n == 1),
        "every call must produce exactly one step: {counts:?}"
    );
    let (input, _, result) = &by_tool["Read"];
    assert!(input.contains("file_path") && !result.is_empty());
    let (input, summary, result) = &by_tool["Bash"];
    assert!(input.contains("command") && !summary.is_empty() && result.contains("hi"));
    let (input, _, result) = &by_tool["Edit"];
    assert!(
        input.contains("file_path") && result.contains("@@"),
        "Edit result = {result:?}"
    );
}

/// A reply long enough for several assistant frames is reassembled into
/// coherent blocks while still streaming deltas.
#[tokio::test]
async fn streaming_transcript_smoke() {
    let (Some(bin), Some(key)) = (require_bridge(), require_key()) else {
        return;
    };
    let wt = tempfile::tempdir().unwrap();
    let (res, evs) = live_run(
        bin,
        &key,
        wt.path().to_path_buf(),
        req(
            "LOOP-STREAM",
            wt.path(),
            "Do not use any tools. Reply with a short guide that contains a fenced bash code block running `go run .`, a bold **Controls:** line, and a closing sentence.",
        ),
    )
    .await;
    let blocks: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let deltas = evs.iter().filter(|e| e.kind() == "text_delta").count();
    let joined = blocks.concat();
    eprintln!("blocks={} deltas={deltas}\n{joined}", blocks.len());
    assert!(!blocks.is_empty(), "no text block stored");
    assert!(deltas > 0, "nothing streamed live");
    for (i, b) in blocks.iter().enumerate() {
        assert!(
            b.matches("```").count() % 2 == 0,
            "block {i} has an unbalanced fence:\n{b}"
        );
    }
    assert!(
        !res.text.is_empty() && res.text.len() >= joined.len() / 2,
        "Result.Text looks like a fragment"
    );
}

/// A todo list reaches the UI as a checklist, not a raw JSON dump.
#[tokio::test]
async fn todo_rendering_smoke() {
    let (Some(bin), Some(key)) = (require_bridge(), require_key()) else {
        return;
    };
    let wt = tempfile::tempdir().unwrap();
    let (_, evs) = live_run(
        bin,
        &key,
        wt.path().to_path_buf(),
        req(
            "LOOP-PLAN",
            wt.path(),
            "Create a todo list of 3 steps for building a CLI app, then update it marking the first completed and the second in progress. Create no files. Reply DONE.",
        ),
    )
    .await;
    let todos: Vec<&recur_agent::ToolStep> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Step(s)
                if crate::tools::display_tool(&s.name, &s.input).name == "TodoWrite" =>
            {
                Some(s)
            }
            _ => None,
        })
        .collect();
    let Some(last) = todos.last() else {
        eprintln!("skipped: the model did not use the todo tool this run");
        return;
    };
    let d = crate::tools::display_tool(&last.name, &last.input);
    let v: Value = serde_json::from_str(&d.input).expect("the checklist shape");
    let items = v["todos"].as_array().expect("todos");
    assert!(!items.is_empty());
    for (i, t) in items.iter().enumerate() {
        assert!(
            t["content"].as_str().is_some_and(|c| !c.is_empty()),
            "todo {i} has no content"
        );
        assert!(
            matches!(
                t["status"].as_str(),
                Some("pending" | "in_progress" | "completed")
            ),
            "todo {i} status {}",
            t["status"]
        );
    }
    assert!(
        !last.summary.is_empty() && !last.summary.contains('{') && !last.summary.contains('"'),
        "{}",
        last.summary
    );
}

/// Failing reads still end with a result, closed at the next model boundary.
#[tokio::test]
async fn failed_tool_call_smoke() {
    let (Some(bin), Some(key)) = (require_bridge(), require_key()) else {
        return;
    };
    let wt = tempfile::tempdir().unwrap();
    let (_, evs) = live_run(
        bin,
        &key,
        wt.path().to_path_buf(),
        req(
            "LOOP-FAILREAD",
            wt.path(),
            "First read go.mod (it may not exist). Then list the directory with a glob. Then run `echo done` in the shell. Then say what you found. Create nothing.",
        ),
    )
    .await;
    let mut open = HashSet::new();
    let mut steps = HashMap::new();
    let mut results = HashMap::new();
    let mut first_unresolved = None;
    for (n, e) in evs.iter().enumerate() {
        match e {
            AgentEvent::Step(s) => {
                steps.insert(s.id.clone(), s.name.clone());
                open.insert(s.id.clone());
            }
            AgentEvent::ToolResult(r) => {
                results.insert(r.id.clone(), (r.text.clone(), r.is_error));
                open.remove(&r.id);
            }
            AgentEvent::Thinking { .. } | AgentEvent::Text { .. }
                if !open.is_empty() && first_unresolved.is_none() =>
            {
                first_unresolved = Some(n);
            }
            _ => {}
        }
    }
    if steps.is_empty() {
        eprintln!("skipped: the model did not attempt any reads this run");
        return;
    }
    assert_eq!(
        first_unresolved, None,
        "a call was still open when the model produced output"
    );
    for (id, tool) in &steps {
        let (text, is_err) = results
            .get(id)
            .unwrap_or_else(|| panic!("{tool} call {id} never got a result"));
        assert!(!text.is_empty(), "{tool} call {id} got an empty result");
        eprintln!("{tool:<6} isErr={is_err:<5} result={text:.70?}");
    }
}

/// The runner end to end with a Recur-shaped MCP server attached.
#[tokio::test]
async fn runner_end_to_end_smoke() {
    let (Some(bin), Some(key)) = (require_bridge(), require_key()) else {
        return;
    };
    let wt = tempfile::tempdir().unwrap();
    let mcp = start_stub_mcp().await;
    let mut r = req(
        "LOOP-SMOKE",
        wt.path(),
        "Report the magic number. Reply with only the number. Do not use the shell.",
    );
    r.model = "composer-2.5".into();
    r.system_prompt =
        "You are Recur's Coder. Always call the magic_number tool before answering.".into();
    r.mcp_url = mcp.url.clone();
    let (res, evs) = live_run(bin, &key, wt.path().to_path_buf(), r).await;
    assert!(
        !res.is_error,
        "run failed: subtype={} stderr={}",
        res.subtype, res.stderr
    );
    assert!(res.text.contains("4271"), "{}", res.text);
    assert!(
        !res.session_id.is_empty(),
        "multi-turn continuity needs the agent id"
    );
    let kinds: Vec<&str> = evs.iter().map(AgentEvent::kind).collect();
    assert!(kinds.contains(&"init"));
    assert!(
        kinds.contains(&"step") && kinds.contains(&"tool_result"),
        "{kinds:?}"
    );
    let tools: Vec<String> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Step(s) => {
                assert_eq!(s.name, "mcp", "events carry Cursor's raw name");
                Some(crate::tools::display_tool(&s.name, &s.input).name)
            }
            _ => None,
        })
        .collect();
    assert!(
        tools.iter().any(|t| t == "mcp__recur__magic_number"),
        "{tools:?}"
    );
    let methods = mcp.methods.lock().clone();
    assert!(
        has_all(&methods, &["initialize", "tools/list", "tools/call"]),
        "MCP handshake incomplete: {methods:?}"
    );
}
