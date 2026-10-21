//! Go `streamjson_test.go` and the parser half of `context_usage_test.go`.

use super::*;

fn parse(line: &str) -> Vec<AgentEvent> {
    parse_line(line.as_bytes()).unwrap_or_else(|e| panic!("parse {line}: {e}"))
}

fn one(line: &str) -> AgentEvent {
    let evs = parse(line);
    assert_eq!(evs.len(), 1, "want one event from {line}, got {evs:?}");
    evs.into_iter().next().unwrap()
}

fn kinds(evs: &[AgentEvent]) -> Vec<&'static str> {
    evs.iter().map(AgentEvent::kind).collect()
}

fn step(ev: &AgentEvent) -> &ToolStep {
    match ev {
        AgentEvent::Step(s) => s,
        other => panic!("want step, got {other:?}"),
    }
}

fn tool_result(ev: &AgentEvent) -> &ToolResultEvent {
    match ev {
        AgentEvent::ToolResult(r) => r,
        other => panic!("want tool_result, got {other:?}"),
    }
}

fn result(ev: &AgentEvent) -> &TurnResult {
    match ev {
        AgentEvent::TurnEnd(r) => r,
        other => panic!("want result, got {other:?}"),
    }
}

fn text_of(ev: &AgentEvent) -> &str {
    match ev {
        AgentEvent::Text { text, .. }
        | AgentEvent::TextDelta { text, .. }
        | AgentEvent::Thinking { text, .. }
        | AgentEvent::ThinkingDelta { text, .. }
        | AgentEvent::Notice { text } => text,
        other => panic!("no text on {other:?}"),
    }
}

/// (from, to, retracted, uuid) of a fallback event.
fn fallback_of(ev: &AgentEvent) -> (&str, &str, &[String], &str) {
    match ev {
        AgentEvent::Fallback {
            from_model,
            to_model,
            retracted_uuids,
            uuid,
        } => (from_model, to_model, retracted_uuids, uuid),
        other => panic!("want fallback, got {other:?}"),
    }
}

fn fallbacks(evs: &[AgentEvent]) -> Vec<&AgentEvent> {
    evs.iter().filter(|e| e.kind() == "fallback").collect()
}

#[test]
fn system_init() {
    let ev = one(
        r#"{"type":"system","subtype":"init","session_id":"sess-abc","uuid":"u1","model":"claude-sonnet-4-6","tools":["Read","Edit"],"mcp_servers":[{"name":"recur","status":"connected"}],"cwd":"/tmp/wt","permissionMode":"dontAsk"}"#,
    );
    let AgentEvent::Init {
        session_id,
        model,
        mcp_servers,
    } = ev
    else {
        panic!("want init, got {ev:?}")
    };
    assert_eq!(
        (session_id.as_str(), model.as_str()),
        ("sess-abc", "claude-sonnet-4-6")
    );
    assert_eq!(
        mcp_servers,
        [McpServerStatus {
            name: "recur".into(),
            status: "connected".into()
        }]
    );
}

#[test]
fn assistant_tool_use_and_text() {
    let evs = parse(
        r#"{"type":"assistant","uuid":"u2","session_id":"sess-abc","message":{"role":"assistant","content":[{"type":"text","text":"I'll edit the file."},{"type":"tool_use","id":"toolu_1","name":"Edit","input":{"file_path":"src/app.go","old_string":"a","new_string":"b"}}]}}"#,
    );
    assert_eq!(kinds(&evs), ["text", "step"]);
    assert_eq!(text_of(&evs[0]), "I'll edit the file.");
    let s = step(&evs[1]);
    assert_eq!((s.name.as_str(), s.id.as_str()), ("Edit", "toolu_1"));
    assert_eq!(s.summary, "src/app.go", "Edit input summary");
    assert_eq!(
        s.input,
        r#"{"file_path":"src/app.go","old_string":"a","new_string":"b"}"#
    );
    assert_eq!(s.uuid, "u2");
    assert_eq!(evs[0].uuid(), "u2");
}

#[test]
fn assistant_thinking_block() {
    let evs = parse(
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"weigh options","signature":"sig-1"},{"type":"text","text":"Here's the plan."}]}}"#,
    );
    assert_eq!(kinds(&evs), ["thinking", "text"]);
    let AgentEvent::Thinking {
        text, signature, ..
    } = &evs[0]
    else {
        unreachable!()
    };
    assert_eq!(
        (text.as_str(), signature.as_str()),
        ("weigh options", "sig-1")
    );
}

#[test]
fn redacted_thinking_still_emits() {
    // claude 2.1.198 without --thinking-display: the text is withheld.
    let ev = one(
        r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"","signature":"EtoCC"}]}}"#,
    );
    assert_eq!(
        ev,
        AgentEvent::Thinking {
            text: String::new(),
            signature: "EtoCC".into(),
            uuid: String::new(),
            parent: String::new()
        }
    );
}

#[test]
fn system_thinking_tokens() {
    let tokens = |line: &str| match one(line) {
        AgentEvent::ThinkingTokens { tokens, uuid, .. } => (tokens, uuid),
        other => panic!("want thinking_tokens, got {other:?}"),
    };
    assert_eq!(
        tokens(
            r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":88,"estimated_tokens_delta":38}"#
        ),
        (88, String::new())
    );
    // uuid:l() is minted per line, not the assistant frame id.
    assert_eq!(
        tokens(
            r#"{"type":"system","subtype":"thinking_tokens","uuid":"evt-tick","estimated_tokens":88}"#
        ),
        (88, String::new())
    );
    assert!(
        parse(r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":0}"#).is_empty()
    );
    assert!(
        parse(r#"{"type":"system","subtype":"thinking_tokens","estimated_tokens":"x"}"#).is_empty()
    );
}

#[test]
fn empty_thinking_delta_with_estimate_is_tokens() {
    let ev = one(
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"","estimated_tokens":50}}}"#,
    );
    assert!(
        matches!(ev, AgentEvent::ThinkingTokens { tokens: 50, .. }),
        "{ev:?}"
    );
    // estimated_tokens:null (the --thinking-display summarized shape) is text.
    let ev = one(
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"This","estimated_tokens":null}}}"#,
    );
    assert_eq!(
        ev,
        AgentEvent::ThinkingDelta {
            text: "This".into(),
            parent: String::new()
        }
    );
}

#[test]
fn stream_event_thinking_delta() {
    let ev = one(
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
    );
    assert_eq!(ev.kind(), "thinking_delta");
    assert_eq!(text_of(&ev), "hmm");
}

#[test]
fn stream_event_deltas_drop_envelope_uuid() {
    let ev = one(
        r#"{"type":"stream_event","uuid":"evt-1","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Act"}}}"#,
    );
    assert_eq!(
        (ev.kind(), text_of(&ev), ev.uuid()),
        ("thinking_delta", "Act", "")
    );
    let ev = one(
        r#"{"type":"stream_event","uuid":"evt-2","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}}"#,
    );
    assert_eq!(
        (ev.kind(), text_of(&ev), ev.uuid()),
        ("text_delta", "Hi", "")
    );
}

#[test]
fn assistant_bash_summary() {
    let ev = one(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"go test ./..."}}]}}"#,
    );
    let s = step(&ev);
    assert_eq!(
        (s.name.as_str(), s.summary.as_str()),
        ("Bash", "go test ./...")
    );
}

#[test]
fn user_tool_result_array_content() {
    let ev = one(
        r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"ok"}],"is_error":false}]}}"#,
    );
    let r = tool_result(&ev);
    assert_eq!(
        (r.id.as_str(), r.is_error, r.text.as_str()),
        ("toolu_1", false, "ok")
    );
}

#[test]
fn mcp_done_summarizes_the_summary_field() {
    let ev = one(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t9","name":"mcp__recur__done","input":{"summary":"Created a full-featured terminal Tetris game (tetris.py) using Python's curses module, with all 7 tetromino types."}}]}}"#,
    );
    let s = step(&ev);
    assert_eq!(s.name, "mcp__recur__done");
    assert!(
        !s.summary.contains(r#"{"summary""#),
        "want plain summary, got {}",
        s.summary
    );
    assert!(s.summary.contains("full-featured terminal Tetris"));
}

#[test]
fn user_string_content_has_no_events() {
    assert!(parse(r#"{"type":"user","message":{"role":"user","content":"hello"}}"#).is_empty());
    assert!(parse(r#"{"type":"user","message":{"content":null}}"#).is_empty());
    assert!(parse(r#"{"type":"user","message":{}}"#).is_empty());
}

#[test]
fn result_success() {
    let ev = one(
        r#"{"type":"result","subtype":"success","session_id":"sess-abc","duration_ms":4200,"duration_api_ms":3900,"is_error":false,"num_turns":3,"result":"Done editing.","total_cost_usd":0.0123,"stop_reason":"end_turn"}"#,
    );
    let r = result(&ev);
    assert_eq!((r.subtype.as_str(), r.is_error), ("success", false));
    assert_eq!(
        (r.session_id.as_str(), r.num_turns, r.duration_ms),
        ("sess-abc", 3, 4200)
    );
    assert_eq!(r.text, "Done editing.");
}

#[test]
fn non_terminal_results_are_not_turn_ends() {
    for (name, line, want) in [
        (
            "tool_deferred 1ms",
            r#"{"type":"result","subtype":"success","session_id":"s","duration_ms":1,"is_error":false,"num_turns":1,"result":"","stop_reason":"tool_deferred","deferred_tool_use":{"id":"toolu_bash"}}"#,
            false,
        ),
        (
            "queued follow-up",
            r#"{"type":"result","subtype":"success","session_id":"s","duration_ms":1,"is_error":false,"num_turns":1,"result":"","queued_turn_count":1}"#,
            false,
        ),
        (
            "background_requested",
            r#"{"type":"result","subtype":"success","session_id":"s","duration_ms":12,"is_error":false,"num_turns":1,"result":"","terminal_reason":"background_requested"}"#,
            false,
        ),
        (
            "resumed task-notification replay",
            r#"{"type":"result","subtype":"success","session_id":"s","duration_ms":31,"is_error":false,"num_turns":0,"result":"","stop_reason":null,"origin":{"kind":"task-notification"},"queued_turn_count":0}"#,
            false,
        ),
        (
            "deferred null still terminal",
            r#"{"type":"result","subtype":"success","session_id":"s","duration_ms":5,"is_error":false,"num_turns":1,"result":"ok","deferred_tool_use":null}"#,
            true,
        ),
        (
            "task-notification follow-up turn",
            r#"{"type":"result","subtype":"success","session_id":"s","duration_ms":4200,"is_error":false,"num_turns":1,"result":"done","stop_reason":"end_turn","origin":{"kind":"task-notification"}}"#,
            true,
        ),
        (
            "tiny terminal still counts",
            r#"{"type":"result","subtype":"success","session_id":"s","duration_ms":1,"is_error":false,"num_turns":1,"result":"ok"}"#,
            true,
        ),
        (
            "real completed",
            r#"{"type":"result","subtype":"success","session_id":"s","duration_ms":62095,"is_error":false,"num_turns":2,"result":"done","stop_reason":"end_turn"}"#,
            true,
        ),
    ] {
        let evs = parse(line);
        let got = evs.len() == 1 && evs[0].kind() == "result";
        assert_eq!(got, want, "{name}: {evs:?}");
    }
}

#[test]
fn result_usage_and_context_window() {
    let ev = one(
        r#"{"type":"result","subtype":"success","session_id":"sess-u","duration_ms":100,"is_error":false,"num_turns":1,"result":"ok","total_cost_usd":0.01,"usage":{"input_tokens":120,"output_tokens":40,"cache_creation_input_tokens":500,"cache_read_input_tokens":18000},"modelUsage":{"claude-sonnet-4-6":{"inputTokens":120,"outputTokens":40,"cacheCreationInputTokens":500,"cacheReadInputTokens":18000,"contextWindow":200000},"claude-haiku":{"contextWindow":100000}}}"#,
    );
    let u = &result(&ev).usage;
    assert_eq!(
        (
            u.input_tokens,
            u.output_tokens,
            u.cache_creation_tokens,
            u.cache_read_tokens
        ),
        (120, 40, 500, 18_000)
    );
    assert_eq!(u.context_window, 200_000);
    assert_eq!(u.context_used(), 18_620);
}

#[test]
fn system_compact_boundary() {
    let ev = one(
        r#"{"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto","preTokens":167219}}"#,
    );
    assert_eq!(ev, compact(COMPACT_DONE, "auto".into(), 167_219));
    let ev = one(
        r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"manual","pre_tokens":18875}}"#,
    );
    assert_eq!(ev, compact(COMPACT_DONE, "manual".into(), 18_875));
    let ev = one(r#"{"type":"system","subtype":"compact_started"}"#);
    assert_eq!(ev, compact(COMPACT_STARTED, String::new(), 0));
    let ev = one(
        r#"{"type":"system","subtype":"compact_boundary","compactMetadata":{"trigger":"auto","pre_tokens":9},"compact_metadata":{"trigger":"x","pre_tokens":7}}"#,
    );
    assert_eq!(ev, compact(COMPACT_DONE, "auto".into(), 9));
    assert!(
        parse(r#"{"type":"system","subtype":"compact_boundary","compactMetadata":5}"#).is_empty()
    );
}

#[test]
fn system_status_compacting() {
    let ev = one(r#"{"type":"system","subtype":"status","status":"compacting"}"#);
    assert_eq!(ev, compact(COMPACT_STARTED, "manual".into(), 0));
    assert_eq!(
        one(r#"{"type":"system","subtype":"status","status":"COMPACTING"}"#).kind(),
        "compact"
    );
    assert!(parse(r#"{"type":"system","subtype":"status","status":"idle"}"#).is_empty());
}

#[test]
fn result_error_max_turns_has_no_text() {
    let ev = one(
        r#"{"type":"result","subtype":"error_max_turns","session_id":"sess-xyz","duration_ms":9000,"is_error":true,"num_turns":12,"total_cost_usd":0.5}"#,
    );
    let r = result(&ev);
    assert!(r.is_error && r.subtype == "error_max_turns");
    assert_eq!(r.text, "");
}

#[test]
fn result_error_keeps_the_error_message() {
    let ev = one(
        r#"{"type":"result","subtype":"error_during_execution","is_error":false,"error":"You've hit your org's monthly spend limit","result":"r"}"#,
    );
    let r = result(&ev);
    assert!(r.is_error, "non-success subtypes are errors");
    assert_eq!(r.text, "You've hit your org's monthly spend limit");
    let ev = one(r#"{"type":"result","subtype":"error_during_execution","result":"from result"}"#);
    assert_eq!(result(&ev).text, "from result");
}

#[test]
fn assistant_fallback_content_block() {
    let evs = parse(
        r#"{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}},{"type":"text","text":"continuing on Opus."}]}}"#,
    );
    assert_eq!(kinds(&evs), ["fallback", "text"]);
    let (from, to, _, _) = fallback_of(&evs[0]);
    assert_eq!((from, to), ("claude-fable-5", "claude-opus-4-8"));
    assert_eq!(text_of(&evs[1]), "continuing on Opus.");
}

#[test]
fn ordinary_message_model_is_not_a_fallback() {
    let evs = parse(
        r#"{"type":"assistant","uuid":"asst-1","message":{"role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"hello"}]}}"#,
    );
    assert_eq!(kinds(&evs), ["text"]);
    assert_eq!(text_of(&evs[0]), "hello");
}

#[test]
fn supersedes_merges_content_block_fallback() {
    let evs = parse(
        r#"{"type":"assistant","uuid":"opus-frame","supersedes":["fable-frame"],"message":{"model":"claude-opus-4-8","content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}},{"type":"text","text":"replacement"}]}}"#,
    );
    let fb = fallbacks(&evs);
    assert_eq!(fb.len(), 1);
    let (from, to, retracted, uuid) = fallback_of(fb[0]);
    assert_eq!((from, to), ("claude-fable-5", "claude-opus-4-8"));
    assert_eq!(retracted, ["fable-frame"]);
    assert_eq!(uuid, "opus-frame");
}

#[test]
fn supersedes_retracts_before_replacement_content() {
    let evs = parse(
        r#"{"type":"assistant","uuid":"opus-frame","supersedes":["fable-frame"],"message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"replacement"}],"usage":{"iterations":[{"type":"message","model":"claude-fable-5"},{"type":"fallback_message","model":"claude-opus-4-8"}]}}}"#,
    );
    assert_eq!(kinds(&evs), ["fallback", "text"]);
    let (from, to, retracted, _) = fallback_of(&evs[0]);
    assert_eq!((from, to), ("claude-fable-5", "claude-opus-4-8"));
    assert_eq!(retracted, ["fable-frame"]);
    assert_eq!(text_of(&evs[1]), "replacement");
}

#[test]
fn supersedes_alone_overlays_the_serving_model() {
    let evs = parse(
        r#"{"type":"assistant","uuid":"u","supersedes":[" a ","",null],"message":{"model":" claude-opus-4-8 ","content":[]}}"#,
    );
    let (from, to, retracted, uuid) = fallback_of(&evs[0]);
    assert_eq!((from, to, uuid), ("", "claude-opus-4-8", "u"));
    assert_eq!(retracted, ["a"]);
}

#[test]
fn last_fallback_iteration_is_the_serving_model() {
    let evs = parse(
        r#"{"type":"assistant","uuid":"asst-1","message":{"content":[{"type":"text","text":"ok"}],"usage":{"iterations":[{"type":"message","model":"claude-fable-5"},{"type":"fallback_message","model":"claude-opus-4-8"},{"type":"fallback_message","model":"claude-opus-4-7"}]}}}"#,
    );
    let fb = fallbacks(&evs);
    let (from, to, _, uuid) = fallback_of(fb[0]);
    assert_eq!(
        (from, to, uuid),
        ("claude-fable-5", "claude-opus-4-7", "asst-1")
    );
}

#[test]
fn model_refusal_fallback_retracts_frames() {
    let ev = one(
        r#"{"type":"system","subtype":"model_refusal_fallback","uuid":"sys-1","original_model":"claude-fable-5","fallback_model":"claude-opus-4-8","retracted_message_uuids":["fable-frame"]}"#,
    );
    let (from, to, retracted, uuid) = fallback_of(&ev);
    assert_eq!(
        (from, to, uuid),
        ("claude-fable-5", "claude-opus-4-8", "sys-1")
    );
    assert_eq!(retracted, ["fable-frame"]);
    let ev = one(
        r#"{"type":"system","subtype":"model_refusal_fallback","originalModel":"claude-fable-5","fallbackModel":"claude-opus-4-8","retractedMessageUuids":["a"]}"#,
    );
    let (from, to, retracted, _) = fallback_of(&ev);
    assert_eq!(
        (from, to, retracted.len()),
        ("claude-fable-5", "claude-opus-4-8", 1)
    );
    assert!(
        parse(r#"{"type":"system","subtype":"model_refusal_fallback","original_model":"x"}"#)
            .is_empty()
    );
}

#[test]
fn stream_event_fallback_start() {
    let ev = one(
        r#"{"type":"stream_event","uuid":"e1","event":{"type":"content_block_start","content_block":{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}}}}"#,
    );
    let (from, to, _, uuid) = fallback_of(&ev);
    assert_eq!(
        (from, to, uuid),
        ("claude-fable-5", "claude-opus-4-8", "e1")
    );
    let ev = one(
        r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"fallback","from":"a","to":" b "}}}"#,
    );
    assert_eq!(fallback_of(&ev).1, "b");
    assert!(parse(r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"fallback","to":{"model":""}}}}"#).is_empty());
}

#[test]
fn sticky_fallback_from_usage_iterations() {
    let evs = parse(
        r#"{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"served by Opus."}],"usage":{"input_tokens":10,"output_tokens":4,"iterations":[{"type":"message","model":"claude-fable-5"},{"type":"fallback_message","model":"claude-opus-4-8"}]}}}"#,
    );
    assert_eq!(
        kinds(&evs),
        ["usage", "fallback", "text"],
        "fallback after usage, before text"
    );
    let (from, to, _, _) = fallback_of(&evs[1]);
    assert_eq!((from, to), ("claude-fable-5", "claude-opus-4-8"));
    assert_eq!(text_of(&evs[2]), "served by Opus.");
}

#[test]
fn fallback_block_wins_over_iterations() {
    let evs = parse(
        r#"{"type":"assistant","message":{"content":[{"type":"fallback","from":{"model":"claude-fable-5"},"to":{"model":"claude-opus-4-8"}},{"type":"text","text":"ok"}],"usage":{"iterations":[{"type":"fallback_message","model":"claude-opus-4-8"}]}}}"#,
    );
    assert_eq!(fallbacks(&evs).len(), 1, "{evs:?}");
}

#[test]
fn api_retry_surfaces_a_notice() {
    let ev = one(
        r#"{"type":"system","subtype":"api_retry","attempt":3,"max_retries":10,"retry_delay_ms":2000,"error_status":null,"error":"unknown"}"#,
    );
    assert_eq!(text_of(&ev), "API request failed (unknown), retrying 3/10…");
    let ev = one(
        r#"{"type":"system","subtype":"api_retry","attempt":1,"max_retries":10,"error_status":529,"error":"server_error"}"#,
    );
    assert_eq!(
        text_of(&ev),
        "API request failed (server_error 529), retrying 1/10…"
    );
    let ev = one(r#"{"type":"system","subtype":"api_retry","attempt":2,"max_retries":5}"#);
    assert_eq!(text_of(&ev), "API request failed (error), retrying 2/5…");
    assert!(parse(r#"{"type":"system","subtype":"api_retry","attempt":0}"#).is_empty());
}

#[test]
fn unknown_and_blank_lines_are_ignored() {
    for line in [
        r#"{"type":"stream_event","event":{}}"#,
        r#"{"type":"totally_unknown"}"#,
        r#"{"type":"system","subtype":"plugin_install"}"#,
        "",
        "   ",
        "\r\n",
        "null",
    ] {
        assert!(parse(line).is_empty(), "{line:?}");
    }
}

#[test]
fn malformed_json_errors() {
    for line in [
        "{not json",
        "[1]",
        r#""str""#,
        r#"{"type":5}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":5}]}}"#,
    ] {
        assert!(parse_line(line.as_bytes()).is_err(), "{line}");
    }
    let err = parse_line(br#"{"type":"assistant","uuid":5}"#).unwrap_err();
    assert!(
        err.to_string().starts_with("stream-json assistant: "),
        "{err}"
    );
}

#[test]
fn stream_event_text_delta() {
    let ev = one(
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}}"#,
    );
    assert_eq!((ev.kind(), text_of(&ev)), ("text_delta", "Hel"));
    assert!(parse(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":""}}}"#).is_empty());
}

#[test]
fn stream_event_unknown_delta_ignored() {
    assert!(parse(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{}"}}}"#).is_empty());
}

#[test]
fn nested_tools_and_text_keep_parent_and_nested_result_is_dropped() {
    let parent = "toolu_task_1";
    let evs = parse(&format!(
        r#"{{"type":"assistant","parent_tool_use_id":"{parent}","message":{{"content":[{{"type":"text","text":"I'll grep the UI."}},{{"type":"tool_use","id":"t_nested","name":"Grep","input":{{"pattern":"PRDetail"}}}}]}}}}"#
    ));
    assert_eq!(kinds(&evs), ["text", "step"]);
    assert_eq!(
        (text_of(&evs[0]), evs[0].parent()),
        ("I'll grep the UI.", parent)
    );
    let s = step(&evs[1]);
    assert_eq!(
        (s.name.as_str(), s.id.as_str(), s.parent.as_str()),
        ("Grep", "t_nested", parent)
    );

    let ev = one(&format!(
        r#"{{"type":"user","parent_tool_use_id":"{parent}","message":{{"content":[{{"type":"tool_result","tool_use_id":"t_nested","content":[{{"type":"text","text":"src/PR.svelte"}}],"is_error":false}}]}}}}"#
    ));
    let r = tool_result(&ev);
    assert_eq!((r.id.as_str(), r.parent.as_str()), ("t_nested", parent));

    let ev = one(&format!(
        r#"{{"type":"stream_event","parent_tool_use_id":"{parent}","event":{{"type":"content_block_delta","delta":{{"type":"text_delta","text":"inside"}}}}}}"#
    ));
    assert_eq!(
        (ev.kind(), text_of(&ev), ev.parent()),
        ("text_delta", "inside", parent)
    );

    let ev = one(&format!(
        r#"{{"type":"system","subtype":"thinking_tokens","parent_tool_use_id":"{parent}","estimated_tokens":12}}"#
    ));
    assert!(
        matches!(ev, AgentEvent::ThinkingTokens { tokens: 12, ref parent, .. } if parent == "toolu_task_1")
    );

    for line in [
        format!(
            r#"{{"type":"result","parent_tool_use_id":"{parent}","subtype":"success","session_id":"sub","is_error":false,"result":"subagent done","total_cost_usd":0.1}}"#
        ),
        format!(
            r#"{{"type":"system","subtype":"init","parent_tool_use_id":"{parent}","session_id":"sub","model":"sonnet","mcp_servers":[]}}"#
        ),
        format!(
            r#"{{"type":"assistant","parent_tool_use_id":"{parent}","message":{{"usage":{{"input_tokens":5}},"content":[{{"type":"fallback","to":{{"model":"m"}}}}]}}}}"#
        ),
    ] {
        assert!(parse(&line).is_empty(), "nested line leaked: {line}");
    }
}

#[test]
fn parent_task_without_parent_id_is_kept() {
    let ev = one(
        r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"toolu_task_1","name":"Task","input":{"description":"Map PR detail UI structure","subagent_type":"Explore","prompt":"Search the Svelte frontend."}}]}}"#,
    );
    let s = step(&ev);
    assert_eq!(
        (s.name.as_str(), s.id.as_str(), s.parent.as_str()),
        ("Task", "toolu_task_1", "")
    );
    let ev = one(
        r#"{"type":"assistant","parent_tool_use_id":"  ","message":{"content":[{"type":"text","text":"x"}]}}"#,
    );
    assert_eq!(ev.parent(), "");
    let ev = one(
        r#"{"type":"assistant","parent_tool_use_id":7,"message":{"content":[{"type":"text","text":"x"}]}}"#,
    );
    assert_eq!(ev.parent(), "");
}

#[test]
fn async_task_launch_ack_is_not_a_tool_result() {
    let ack = r"Async agent launched successfully.\n(This tool result is internal metadata – never quote or paste any part of it, including the agentId below, into a user-facing reply.)\n\nagentId: a2a1c3c855409bb10\n";
    let line = format!(
        r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_task_1","content":[{{"type":"text","text":"{ack}"}}],"is_error":false}}]}}}}"#
    );
    assert!(parse(&line).is_empty());
}

#[test]
fn tool_result_mentioning_the_ack_is_kept() {
    let text =
        "see Async agent launched successfully plus agentId and internal metadata in comments";
    let line = format!(
        r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t_read","content":[{{"type":"text","text":"{text}"}}],"is_error":false}}]}}}}"#
    );
    assert_eq!(tool_result(&one(&line)).text, text);
}

#[test]
fn tool_result_text_shapes() {
    let text = |content: &str| {
        let line = format!(
            r#"{{"type":"user","uuid":"u9","message":{{"content":[{{"type":"tool_result","tool_use_id":"t","content":{content},"is_error":true}}]}}}}"#
        );
        let ev = one(&line);
        let r = tool_result(&ev);
        assert!(r.is_error);
        assert_eq!(r.uuid, "u9");
        r.text.clone()
    };
    assert_eq!(text(r#""plain""#), "plain");
    assert_eq!(
        text(
            r#"[{"type":"text","text":"a"},{"type":"text","text":"  "},null,{"type":"image"},{"type":"text","text":"b"}]"#
        ),
        "a\nb"
    );
    assert_eq!(text("null"), "");
    assert_eq!(text(r#" {"k": 1} "#), r#"{"k": 1}"#);
    assert_eq!(text(r#"[{"text":5}]"#), r#"[{"text":5}]"#);
    let ev =
        one(r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t"}]}}"#);
    assert_eq!(tool_result(&ev).text, "");
}

#[test]
fn null_input_keeps_its_raw_text() {
    let ev = one(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Read","input":null}]}}"#,
    );
    let s = step(&ev);
    assert_eq!((s.input.as_str(), s.summary.as_str()), ("null", "null"));
    let ev = one(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Read"}]}}"#,
    );
    let s = step(&ev);
    assert_eq!((s.input.as_str(), s.summary.as_str()), ("", ""));
}

#[test]
fn invalid_utf8_becomes_replacement_runes() {
    let mut line =
        br#"{"type":"assistant","message":{"content":[{"type":"text","text":"a"#.to_vec();
    line.extend_from_slice(b"\xe2\x80");
    line.extend_from_slice(br#"b"}]}}"#);
    let ev = parse_line(&line).unwrap();
    assert_eq!(text_of(&ev[0]), "a\u{FFFD}\u{FFFD}b");
}

/// Go `TestAssistantUsageEmitsKindUsage`: each assistant.message.usage is a
/// per-inference footprint, not session billing.
#[test]
fn assistant_usage_emits_usage() {
    let evs = parse(
        r#"{"type":"assistant","message":{"role":"assistant","model":"claude-sonnet-5","usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":5,"cache_read_input_tokens":80000},"content":[{"type":"text","text":"hi"}]}}"#,
    );
    assert_eq!(kinds(&evs), ["usage", "text"]);
    let AgentEvent::Usage(u) = &evs[0] else {
        unreachable!()
    };
    assert_eq!(
        (u.input_tokens, u.cache_read_tokens, u.cache_creation_tokens),
        (100, 80_000, 5)
    );
    assert_eq!(u.context_used(), 80_105);
    assert!(
        parse(r#"{"type":"assistant","message":{"usage":{"output_tokens":20},"content":[]}}"#)
            .is_empty(),
        "output-only usage does not move the meter"
    );
}
