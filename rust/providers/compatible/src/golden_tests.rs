//! Replays `tests/golden/go_harness.json` and `go_catalog.json`, recorded from
//! the Go harness, responses package and model catalogue (generators next to
//! the fixtures as `.go.txt`).

use std::sync::Arc;

use parking_lot::Mutex;
use recur_agent::{AgentEvent, EventSink, Provider, Steer, SteerInbox, TurnContext, TurnRequest};
use recur_domain::{CompatibleFlagDim, CompatibleProvider as Config, Message};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::tests::serve;
use super::*;
use crate::history::{
    harness_env, harness_history, harness_instructions, next_history_group, repo_rules,
};
use crate::sse::{decode_response, parse_sse};
use crate::types::{Event, InputItem, Usage};

fn golden() -> Value {
    serde_json::from_str(include_str!("../tests/golden/go_harness.json")).unwrap()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}

fn items(v: &Value) -> Vec<InputItem> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|i| InputItem {
                    r#type: s(&i["type"]),
                    role: s(&i["role"]),
                    content: i["content"]
                        .as_array()
                        .map(|c| {
                            c.iter()
                                .map(|p| types::InputContent {
                                    r#type: s(&p["type"]),
                                    text: s(&p["text"]),
                                    image_url: s(&p["image_url"]),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    call_id: s(&i["call_id"]),
                    name: s(&i["name"]),
                    arguments: s(&i["arguments"]),
                    output: s(&i["output"]),
                    id: s(&i["id"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn event_json(e: &Event) -> Value {
    json!({
        "type": e.r#type, "responseId": e.response_id, "delta": e.delta, "itemId": e.item_id,
        "status": e.status, "errorMessage": e.error_message, "outputText": e.output_text,
        "call": e.call.as_ref().map(|c| json!({"id": c.id, "callId": c.call_id, "name": c.name, "arguments": c.arguments})),
        "usage": e.usage.map(|u| json!({"inputTokens": u.input_tokens, "outputTokens": u.output_tokens, "cacheReadTokens": u.cache_read_tokens, "totalTokens": u.total_tokens})),
    })
}

fn events_json(evs: &[Event]) -> Value {
    if evs.is_empty() {
        return Value::Null;
    }
    Value::Array(evs.iter().map(event_json).collect())
}

#[test]
fn coding_prompt_is_byte_identical() {
    assert_eq!(
        prompt::RECUR_CODING_PROMPT,
        golden()["prompt"].as_str().unwrap()
    );
}

#[test]
fn instructions_match_go() {
    for c in golden()["instructions"].as_array().unwrap() {
        let req = TurnRequest {
            system_prompt: s(&c["system"]),
            append_system_prompt: s(&c["append"]),
            turn_append_prompt: s(&c["turn"]),
            ..Default::default()
        };
        assert_eq!(harness_instructions(&req), s(&c["out"]), "{c}");
    }
}

#[test]
fn env_format_matches_go() {
    let g = &golden()["env"];
    let want = format!(
        "## Environment\nWorktree (Bash cwd): {}\nPlatform: {}/{}\nToday's date: {}",
        s(&g["worktree"]),
        s(&g["goos"]),
        s(&g["goarch"]),
        s(&g["date"])
    );
    assert_eq!(want, s(&g["out"]));
    let got = harness_env("/wt");
    let (os, arch) = history::go_platform();
    assert!(
        got.starts_with(&format!(
            "## Environment\nWorktree (Bash cwd): /wt\nPlatform: {os}/{arch}\nToday's date: "
        )),
        "{got}"
    );
    assert_eq!(harness_env(""), "");
}

#[test]
fn repo_rules_match_go() {
    for c in golden()["repoRules"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in c["files"].as_object().unwrap() {
            std::fs::write(dir.path().join(name), body.as_str().unwrap()).unwrap();
        }
        assert_eq!(
            repo_rules(&dir.path().to_string_lossy()),
            s(&c["out"]),
            "{}",
            c["name"]
        );
    }
}

#[test]
fn history_matches_go() {
    let g = golden();
    let root = tempfile::tempdir().unwrap();
    for (rel, data) in g["dataFiles"].as_object().unwrap() {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data.as_str().unwrap())
            .unwrap();
        let p = root.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    for c in g["history"].as_array().unwrap() {
        let msgs: Vec<Message> = serde_json::from_value(c["msgs"].clone()).unwrap_or_default();
        let sp = &c["spec"];
        let images = sp["images"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|i| recur_agent::UserImage {
                        id: s(&i["id"]),
                        name: s(&i["name"]),
                        mime: s(&i["mime"]),
                        data: s(&i["data"]),
                        path: s(&i["path"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let req = TurnRequest {
            prompt: s(&sp["prompt"]),
            display_text: s(&sp["displayText"]),
            user_message_id: s(&sp["userMessageId"]),
            skip_record_user: sp["skipRecordUser"].as_bool().unwrap(),
            history_after_id: s(&sp["historyAfterId"]),
            images,
            ..Default::default()
        };
        let got = recur_domain::json::to_string(&harness_history(&msgs, &req, Some(root.path())))
            .unwrap();
        let got: Value = serde_json::from_str(&got).unwrap();
        assert_eq!(got, c["out"], "{}", c["name"]);
    }
}

#[test]
fn history_groups_match_go() {
    for c in golden()["historyGroups"].as_array().unwrap() {
        assert_eq!(
            next_history_group(&items(&c["items"])) as u64,
            c["n"].as_u64().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn flags_match_go() {
    let g = golden();
    let dims: Vec<CompatibleFlagDim> = serde_json::from_value(g["flagDims"].clone()).unwrap();
    for c in g["flags"].as_array().unwrap() {
        let mut dst: Map<String, Value> =
            serde_json::from_str(c["body"].as_str().unwrap()).unwrap();
        let flags: recur_agent::Flags =
            serde_json::from_value(c["flags"].clone()).unwrap_or_default();
        flags::apply_flags(&mut dst, &dims, &flags).unwrap();
        assert_eq!(Value::Object(dst), c["out"], "{c}");
    }
}

#[test]
fn sse_matches_go() {
    for c in golden()["sse"].as_array().unwrap() {
        let mut evs = Vec::new();
        parse_sse(c["raw"].as_str().unwrap().as_bytes(), &mut |e| evs.push(e));
        assert_eq!(events_json(&evs), c["events"], "{}", c["raw"]);
    }
}

#[test]
fn decode_matches_go() {
    for c in golden()["decode"].as_array().unwrap() {
        let body = c["body"].as_str().unwrap();
        match decode_response(body.as_bytes()) {
            Ok(evs) => {
                assert!(!c["error"].as_bool().unwrap(), "{body}: want error");
                assert_eq!(events_json(&evs), c["events"], "{body}");
            }
            Err(e) => assert!(c["error"].as_bool().unwrap(), "{body}: {e}"),
        }
    }
}

#[test]
fn terminal_errors_match_go() {
    for c in golden()["terminal"].as_array().unwrap() {
        let got = terminal_response_error(&s(&c["status"]), c["sawCompleted"].as_bool().unwrap())
            .err()
            .unwrap_or_default();
        assert_eq!(got, s(&c["err"]), "{c}");
    }
}

#[test]
fn tool_allowed_matches_go() {
    for c in golden()["toolAllowed"].as_array().unwrap() {
        let allowed = if c["allowedNil"].as_bool().unwrap() {
            None
        } else {
            Some(serde_json::from_value(c["allowed"].clone()).unwrap_or_default())
        };
        let req = TurnRequest {
            tools: c["tools"].as_str().map(String::from),
            allowed_tools: allowed,
            ..Default::default()
        };
        assert_eq!(
            tool_allowed_on_turn(c["name"].as_str().unwrap(), &req),
            c["out"].as_bool().unwrap(),
            "{c}"
        );
    }
}

#[test]
fn agent_codes_match_go() {
    for c in golden()["agentCodes"].as_array().unwrap() {
        let req = TurnRequest {
            route: recur_agent::SessionRoute {
                code: s(&c["route"]),
                ..Default::default()
            },
            agent_id: s(&c["agent"]),
            workspace_id: s(&c["workspace"]),
            ..Default::default()
        };
        assert_eq!(harness_agent_code(&req), s(&c["out"]), "{c}");
    }
}

#[test]
fn fresh_input_matches_go() {
    for c in golden()["freshInput"].as_array().unwrap() {
        let u = Usage {
            input_tokens: c["input"].as_i64().unwrap(),
            cache_read_tokens: c["cache"].as_i64().unwrap(),
            ..Default::default()
        };
        assert_eq!(responses_fresh_input(u), c["out"].as_i64().unwrap());
    }
}

/// Go `ParsedEvent` fields the golden records.
fn go_event(ev: &AgentEvent) -> Value {
    let mut m = json!({"kind": ev.kind(), "text": "", "display": "", "tool": "", "toolUseId": "",
        "toolInput": "", "isError": false, "model": "", "sessionId": ""});
    match ev {
        AgentEvent::Init {
            session_id, model, ..
        } => {
            m["model"] = json!(model);
            m["sessionId"] = json!(session_id);
        }
        AgentEvent::Text { text, .. }
        | AgentEvent::TextDelta { text, .. }
        | AgentEvent::Thinking { text, .. }
        | AgentEvent::ThinkingDelta { text, .. } => m["text"] = json!(text),
        AgentEvent::Step(st) => {
            m["tool"] = json!(st.name);
            m["toolUseId"] = json!(st.id);
            m["toolInput"] = json!(st.summary);
            m["toolInputRaw"] = json!(st.input);
        }
        AgentEvent::ToolResult(r) => {
            m["tool"] = json!(r.name);
            m["toolUseId"] = json!(r.id);
            m["text"] = json!(r.text);
            m["isError"] = json!(r.is_error);
        }
        AgentEvent::User { text, display, .. } => {
            m["text"] = json!(text);
            m["display"] = json!(display);
        }
        AgentEvent::TurnEnd(r) => {
            let mut r = r.clone();
            r.duration_ms = 0;
            m["text"] = json!(r.text);
            m["result"] = serde_json::to_value(&r).unwrap();
        }
        _ => {}
    }
    m
}

#[tokio::test]
async fn run_matches_go_wire_and_events() {
    let g = golden();
    let run = &g["run"];
    let (r1, r2) = (s(&run["round1"]), s(&run["round2"]));
    let (base, seen) = serve(move |n, _| {
        if n == 0 {
            (200, "text/event-stream", r1.clone())
        } else {
            (200, "application/json", r2.clone())
        }
    })
    .await;
    let mut cfg: Config = serde_json::from_value(run["provider"].clone()).unwrap();
    cfg.base_url = base;
    cfg.extra_headers.insert(String::new(), "dropped".into());
    let p = CompatibleProvider::new(cfg);

    let req = TurnRequest {
        model: "gpt-5".into(),
        flags: [("effort", "high"), ("verbosity", "low")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
        prompt: "Track the work <now>".into(),
        system_prompt: "role".into(),
        append_system_prompt: "append".into(),
        allowed_tools: Some(
            [
                "Read",
                "TodoWrite",
                "mcp__recur__ask_user",
                "mcp__recur__report_finding",
            ]
            .map(String::from)
            .into(),
        ),
        include_partials: true,
        recur_session_id: "sess-1".into(),
        workspace_id: "W1".into(),
        agent_id: "W1-PA".into(),
        ..Default::default()
    };
    let events: Arc<Mutex<Vec<AgentEvent>>> = Arc::default();
    let log = Arc::clone(&events);
    let sink: Arc<dyn EventSink> = Arc::new(move |ev: AgentEvent| log.lock().push(ev));
    let mut cx = TurnContext::new(CancellationToken::new(), sink);
    cx.tools = Some(Arc::new(HarnessTools::new(None, Arc::default(), &req)));
    let (tx, rx) = tokio::sync::mpsc::channel(2);
    let (plain, plain_ack) = Steer::new("Also check docs", Some("Also check docs (ui)".into()));
    let (blank, blank_ack) = Steer::new("   ", None);
    tx.send(plain).await.unwrap();
    tx.send(blank).await.unwrap();
    cx.steer = Some(SteerInbox(rx));

    let mut res = p.run_turn(req, cx).await.unwrap();
    res.duration_ms = 0;
    assert!(plain_ack.await.unwrap());
    assert!(!blank_ack.await.unwrap());

    let seen = seen.lock().clone();
    let bodies: Vec<String> = seen.iter().map(|r| r.body.clone()).collect();
    let want: Vec<String> = serde_json::from_value(run["bodies"].clone()).unwrap();
    assert_eq!(bodies.len(), want.len());
    for (got, want) in bodies.iter().zip(&want) {
        pretty_assertions::assert_eq!(got, want, "request body bytes");
    }
    for (r, h) in seen.iter().zip(run["headers"].as_array().unwrap()) {
        assert_eq!(r.path, s(&h["path"]));
        assert_eq!(r.header("authorization"), s(&h["authorization"]));
        assert_eq!(r.header("accept"), s(&h["accept"]));
        assert_eq!(r.header("content-type"), s(&h["contentType"]));
        assert_eq!(r.header("x-extra"), s(&h["xExtra"]));
    }
    let got: Vec<Value> = events.lock().iter().map(go_event).collect();
    let want = run["events"].as_array().unwrap();
    assert_eq!(got.len(), want.len(), "{got:#?}");
    for (g, w) in got.iter().zip(want) {
        let mut w = w.clone();
        if let Some(r) = w.get_mut("result") {
            r.as_object_mut().unwrap().retain(|_, v| !v.is_null());
        }
        pretty_assertions::assert_eq!(g, &w);
    }
    assert_eq!(serde_json::to_value(&res).unwrap(), run["result"]);
}

#[test]
fn catalog_matches_go() {
    let g: Value = serde_json::from_str(include_str!("../tests/golden/go_catalog.json")).unwrap();
    for c in g["models"].as_array().unwrap() {
        let p: Config = serde_json::from_value(c["provider"].clone()).unwrap();
        let got = serde_json::to_value(compatible_catalog_models(&p)).unwrap();
        let want = if c["models"].is_null() {
            json!([])
        } else {
            c["models"].clone()
        };
        assert_eq!(got, want, "{}", p.id);
    }
    for (name, want) in g["presets"].as_object().unwrap() {
        let got = serde_json::to_value(compatible_flag_preset(name)).unwrap();
        let want = if want.is_null() {
            json!([])
        } else {
            want.clone()
        };
        assert_eq!(got, want, "{name:?}");
    }
    // The enabled+configured groups services build from these providers.
    let settings: recur_domain::Settings = serde_json::from_value(g["settings"].clone()).unwrap();
    let groups: Vec<Value> = settings
        .compatible_providers
        .iter()
        .filter(|p| settings.provider_enabled(&p.id) && p.configured())
        .map(|p| {
            json!({"provider": p.id, "label": settings.display_label(&p.id), "models": compatible_catalog_models(p)})
        })
        .collect();
    assert_eq!(Value::Array(groups), g["groups"]);
}
