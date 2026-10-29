//! Run-stream event tests. The fixtures are VERBATIM frames captured from
//! a real cursor-sdk-bridge (v1.0.0, sdk 1.0.26). They pin the wire quirks:
//! the payload nested under `message`, snake_case payload keys with
//! camelCase nested structs, int64s as strings in the result but numbers in
//! usage messages, "running" (not "started") in flight, and every MCP tool
//! reporting `name: "mcp"`.

use super::*;

const FRAME_STATUS_RUNNING: &str = r#"{"sdkMessage":{"type":"status","message":{"type":"status","agent_id":"agent-1","run_id":"run-1","status":"RUNNING"}},"offset":"1"}"#;
const FRAME_ASSISTANT: &str = r#"{"sdkMessage":{"type":"assistant","message":{"type":"assistant","agent_id":"agent-1","run_id":"run-1","message":{"role":"assistant","content":[{"type":"text","text":"OK"}]}}},"offset":"5"}"#;
const FRAME_THINKING: &str = r#"{"sdkMessage":{"type":"thinking","message":{"type":"thinking","agent_id":"agent-1","run_id":"run-1","text":"considering"}},"offset":"2"}"#;
const FRAME_USAGE: &str = r#"{"sdkMessage":{"type":"usage","message":{"type":"usage","agent_id":"agent-1","run_id":"run-1","usage":{"inputTokens":11288,"outputTokens":33,"cacheReadTokens":8352,"cacheWriteTokens":0,"totalTokens":19673}}},"offset":"6"}"#;
const FRAME_TOOL_RUNNING: &str = r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","agent_id":"agent-1","run_id":"run-1","call_id":"tool_abc","name":"mcp","status":"running","args":{"providerIdentifier":"recur","toolName":"magic_number","args":{}}}},"offset":"13"}"#;
const FRAME_TOOL_COMPLETED: &str = r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","agent_id":"agent-1","run_id":"run-1","call_id":"tool_abc","name":"mcp","status":"completed","args":{"providerIdentifier":"recur","toolName":"magic_number","args":{}},"result":{"status":"success","value":{"content":[{"text":{"text":"The magic number is 4271."}}],"isError":false}}}},"offset":"14"}"#;
const FRAME_TOOL_ERRORED: &str = r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","agent_id":"agent-1","run_id":"run-1","call_id":"tool_def","name":"shell","status":"completed","args":{},"result":{"status":"success","value":{"isError":true}}}},"offset":"15"}"#;
const FRAME_RESULT: &str = r#"{"result":{"agentId":"agent-1","runId":"run-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":{"runId":"run-1","agentId":"agent-1","status":"RUN_LIFECYCLE_STATUS_FINISHED","result":"OK","model":{"id":"composer-2.5"},"durationMs":"2391","usage":{"inputTokens":"11288","outputTokens":"33","cacheReadTokens":"8352","totalTokens":"19673"}}},"offset":"8"}"#;
const FRAME_DONE: &str = r#"{"done":{"agentId":"agent-1","runId":"run-1"},"offset":"9"}"#;

fn decode(raw: &str) -> RunStreamEvent {
    decode_frame(raw.as_bytes()).expect("decode")
}

fn msg(raw: &str) -> SdkMessage {
    decode(raw).sdk_message.expect("sdkMessage")
}

#[test]
fn status_frame() {
    let m = msg(FRAME_STATUS_RUNNING);
    assert_eq!(
        (m.kind.as_str(), m.status.as_str()),
        (MSG_STATUS, STATUS_RUNNING)
    );
    // snake_case keys from the nested payload reach the flat struct.
    assert_eq!(
        (m.agent_id.as_str(), m.run_id.as_str()),
        ("agent-1", "run-1")
    );
    assert_eq!(decode(FRAME_STATUS_RUNNING).offset, "1");
}

#[test]
fn task_frame() {
    // SDKTaskMessage is nested like assistant/thinking; it is not compaction.
    let m = msg(
        r#"{"sdkMessage":{"type":"task","message":{"type":"task","agent_id":"agent-1","run_id":"run-1","status":"running","text":"working on the nested call"}},"offset":"12"}"#,
    );
    assert_eq!(m.kind, MSG_TASK);
    assert_eq!(m.text, "working on the nested call");
    assert_eq!(m.status, "running");
}

#[test]
fn assistant_frame() {
    // The body is nested TWO levels down (sdkMessage.message.message).
    let m = msg(FRAME_ASSISTANT);
    assert_eq!(m.kind, MSG_ASSISTANT);
    assert_eq!(m.content.unwrap().text_content(), "OK");
}

#[test]
fn thinking_frame() {
    let m = msg(FRAME_THINKING);
    assert_eq!(
        (m.kind.as_str(), m.text.as_str()),
        (MSG_THINKING, "considering")
    );
}

#[test]
fn usage_frame_numeric_tokens() {
    let u = msg(FRAME_USAGE).usage.unwrap();
    assert_eq!(u.input_tokens, 11288);
    assert_eq!(u.total_tokens, 19673);
}

#[test]
fn tool_call_rewrites_mcp_label() {
    let m = msg(FRAME_TOOL_RUNNING);
    assert_eq!(m.kind, MSG_TOOL_CALL);
    assert_eq!(m.status, TOOL_RUNNING);
    assert_eq!(m.name, TOOL_NAME_MCP);
    // Every MCP tool reports "mcp"; the engine needs the specific tool.
    assert_eq!(m.tool_label(), "mcp__recur__magic_number");
    assert_eq!(m.call_id, "tool_abc");
}

#[test]
fn tool_call_id_fallbacks() {
    for (raw, want) in [
        (
            r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","id":"tool_from_id","name":"read","status":"running"}}}"#,
            "tool_from_id",
        ),
        (
            r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","toolCallId":"tool_from_camel","name":"read","status":"running"}}}"#,
            "tool_from_camel",
        ),
        (
            r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","tool_call_id":"tool_from_snake","name":"read","status":"running"}}}"#,
            "tool_from_snake",
        ),
        // A numeric JSON-RPC id is no call id, and must not fail the frame.
        (
            r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","id":7,"name":"read","status":"running"}}}"#,
            "",
        ),
    ] {
        assert_eq!(msg(raw).call_id, want, "{raw}");
    }
}

#[test]
fn tool_call_prefers_call_id_over_id() {
    let m = msg(
        r#"{"sdkMessage":{"type":"tool_call","message":{"type":"tool_call","call_id":"real","id":"other","name":"read","status":"running"}}}"#,
    );
    assert_eq!(m.call_id, "real");
}

#[test]
fn grok46_call_id_with_newline() {
    // A live grok-4.6 id from Recur's session store: call-… and fc_… joined
    // by a newline. Decode keeps it intact so step correlation still matches.
    let id =
        "call-9812a46f-b258-4ed8-8d09-55441deb1733-5\nfc_acab39d7-9d73-9fd7-9c87-51495bec8eb8_0";
    let raw = serde_json::json!({
        "sdkMessage": {"type": "tool_call", "message": {
            "type": "tool_call", "call_id": id, "name": "glob", "status": "running",
            "args": {"globPattern": "TODO.md"}
        }}
    })
    .to_string();
    let m = msg(&raw);
    assert_eq!(m.call_id, id);
    assert_eq!((m.name.as_str(), m.status.as_str()), ("glob", TOOL_RUNNING));
}

#[test]
fn builtin_tool_keeps_name() {
    assert_eq!(msg(FRAME_TOOL_ERRORED).tool_label(), "shell");
}

#[test]
fn tool_result_is_error() {
    assert!(!msg(FRAME_TOOL_COMPLETED).tool_result_is_error());
    // A protocol-level success carrying an application-level isError.
    assert!(msg(FRAME_TOOL_ERRORED).tool_result_is_error());
    // In flight is not failed.
    assert!(!msg(FRAME_TOOL_RUNNING).tool_result_is_error());
}

#[test]
fn result_frame_string_ints() {
    let r = decode(FRAME_RESULT).result.unwrap().result.unwrap();
    assert_eq!(r.result, "OK");
    assert!(!r.is_error());
    assert_eq!(r.duration_ms, 2391);
    assert_eq!(r.usage.unwrap().input_tokens, 11288);
    assert_eq!(r.model.unwrap().id, "composer-2.5");
}

#[test]
fn done_frame() {
    let ev = decode(FRAME_DONE);
    assert!(!ev.done.is_empty());
    assert!(ev.result.is_none() && ev.sdk_message.is_none());
}

#[test]
fn run_result_is_error_cases() {
    let r = |status: &str, err: bool| RunResult {
        status: status.into(),
        error: err.then(|| RunError {
            message: "boom".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(run_result_is_error(None), "nil is an error");
    assert!(!r(RUN_FINISHED, false).is_error());
    assert!(r(RUN_ERROR, false).is_error());
    assert!(r(RUN_CANCELLED, false).is_error());
    assert!(r(RUN_FINISHED, true).is_error(), "error detail wins");
    assert!(!r("", false).is_error(), "unknown status is not fatal");
}

#[test]
fn unknown_frame_is_ignorable() {
    let ev = decode(
        r#"{"sdkMessage":{"type":"brand_new","message":{"type":"brand_new","agent_id":"a"}},"offset":"3"}"#,
    );
    let m = ev.sdk_message.unwrap();
    assert_eq!(m.kind, "brand_new");
    assert!(m.content.is_none() && m.usage.is_none());
}

#[test]
fn run_status_terminal_cases() {
    for s in [RUN_FINISHED, RUN_ERROR, RUN_CANCELLED, RUN_EXPIRED] {
        assert!(run_status_terminal(s), "{s}");
    }
    for s in [
        RUN_CREATING,
        RUN_RUNNING,
        "",
        "RUN_LIFECYCLE_STATUS_UNSPECIFIED",
    ] {
        assert!(!run_status_terminal(s), "{s}");
    }
}

#[test]
fn interaction_nested_summary_body() {
    let (t, body) = parse_interaction_type(
        r#"{"type":"summary","update":{"type":"summary","summary":"older turns folded"}}"#,
    );
    assert_eq!(t, DELTA_SUMMARY);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["summary"], "older turns folded");
}

#[test]
fn interaction_types() {
    for (raw, want) in [
        (r#"{"type":"summary-started"}"#, DELTA_SUMMARY_STARTED),
        (
            r#"{"update":{"type":"summary-completed"}}"#,
            DELTA_SUMMARY_COMPLETED,
        ),
        (r#"{"summaryStarted":{}}"#, DELTA_SUMMARY_STARTED),
        (r#"{"summaryCompleted":{}}"#, DELTA_SUMMARY_COMPLETED),
        (
            r#"{"summary":{"summary":"older turns folded"}}"#,
            DELTA_SUMMARY,
        ),
        (
            r#"{"turnEnded":{"usage":{"inputTokens":10}}}"#,
            DELTA_TURN_ENDED,
        ),
        (r#"{"type":"token-delta","tokens":1}"#, "token-delta"),
        ("{}", ""),
    ] {
        assert_eq!(parse_interaction_type(raw).0, want, "{raw}");
    }
}

#[test]
fn sub_usage_clamps_at_zero() {
    let u = |i, c, t| TokenUsage {
        input_tokens: i,
        cache_read_tokens: c,
        total_tokens: t,
        ..Default::default()
    };
    let got = sub_usage(&u(100, 50, 160), &u(80, 40, 120));
    assert_eq!(
        (got.input_tokens, got.cache_read_tokens, got.total_tokens),
        (20, 10, 40)
    );
    assert_eq!(sub_usage(&u(10, 0, 0), &u(40, 0, 0)).input_tokens, 0);
}

#[test]
fn frames_with_wrong_types_are_skipped_like_go() {
    // A mistyped field fails the whole frame; the client skips it.
    assert!(decode_frame(br#"{"offset":5}"#).is_none());
    assert!(
        decode_frame(br#"{"sdkMessage":{"type":"usage","usage":{"inputTokens":1.5}}}"#).is_none()
    );
    assert!(decode_frame(br#"{"sdkMessage":{"type":"tool_call","truncated":true}}"#).is_none());
    assert!(decode_frame(b"not json").is_none());
    // Arrays are rejected (serde would decode them positionally).
    assert!(decode_frame(br#"{"sdkMessage":["status","a","r"]}"#).is_none());
    assert!(tool_call_args(r#"["recur","done"]"#).is_none());
    assert_eq!(
        parse_interaction_type("[1]"),
        (String::new(), String::new())
    );
    // null leaves zero values.
    let ev = decode(r#"{"offset":null,"sdkMessage":null,"result":null}"#);
    assert_eq!(ev, RunStreamEvent::default());
    // A flat (older) payload is its own body.
    let m = msg(
        r#"{"sdkMessage":{"type":"status","status":"ERROR","message":"[internal] x","runId":"r"}}"#,
    );
    assert_eq!(
        (m.status.as_str(), m.message.as_str(), m.run_id.as_str()),
        ("ERROR", "[internal] x", "r")
    );
}
