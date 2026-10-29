//! Snapshot answers for the stream-json parser: every corpus line through
//! `parse_line`, normalized per event kind (`golden/go_stream.json`). Edit the
//! fixture by hand when the behavior changes on purpose.

use recur_agent::{AgentEvent, TokenUsage, TurnResult};
use recur_provider_claude::stream::parse_line;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Row {
    line: String,
    error: bool,
    events: Vec<Value>,
}

fn usage(u: &TokenUsage) -> Value {
    json!({
        "sessionId": "", "subtype": "", "isError": false, "numTurns": 0, "durationMs": 0, "text": "",
        "in": u.input_tokens, "out": u.output_tokens, "cc": u.cache_creation_tokens,
        "cr": u.cache_read_tokens, "cw": u.context_window, "stderr": "",
    })
}

fn result(r: &TurnResult) -> Value {
    json!({
        "sessionId": r.session_id, "subtype": r.subtype, "isError": r.is_error, "numTurns": r.num_turns,
        "durationMs": r.duration_ms, "text": r.text, "in": r.usage.input_tokens,
        "out": r.usage.output_tokens, "cc": r.usage.cache_creation_tokens,
        "cr": r.usage.cache_read_tokens, "cw": r.usage.context_window, "stderr": r.stderr,
    })
}

fn normalize(ev: &AgentEvent) -> Value {
    let kind = ev.kind();
    match ev {
        AgentEvent::Init {
            session_id,
            model,
            mcp_servers,
        } => json!({
            "kind": kind, "sessionId": session_id, "model": model,
            "mcp": mcp_servers.iter().map(|s| [&s.name, &s.status]).collect::<Vec<_>>(),
        }),
        AgentEvent::Text { text, uuid, parent } => {
            json!({"kind": kind, "text": text, "signature": "", "uuid": uuid, "parent": parent})
        }
        AgentEvent::Thinking {
            text,
            signature,
            uuid,
            parent,
        } => {
            json!({"kind": kind, "text": text, "signature": signature, "uuid": uuid, "parent": parent})
        }
        AgentEvent::TextDelta { text, parent } | AgentEvent::ThinkingDelta { text, parent } => {
            json!({"kind": kind, "text": text, "parent": parent})
        }
        AgentEvent::ThinkingTokens {
            tokens,
            uuid,
            parent,
        } => {
            json!({"kind": kind, "tokens": tokens, "uuid": uuid, "parent": parent})
        }
        AgentEvent::Step(s) => json!({
            "kind": kind, "id": s.id, "name": s.name, "summary": s.summary, "input": s.input,
            "parent": s.parent, "uuid": s.uuid,
        }),
        AgentEvent::ToolResult(r) => json!({
            "kind": kind, "id": r.id, "name": r.name, "input": r.input, "text": r.text,
            "isError": r.is_error, "truncated": r.truncated, "parent": r.parent, "uuid": r.uuid,
        }),
        AgentEvent::Usage(u) => json!({"kind": kind, "usage": usage(u)}),
        AgentEvent::Notice { text } => json!({"kind": kind, "text": text}),
        AgentEvent::Compact {
            status,
            trigger,
            summary,
            pre_tokens,
            uuid,
            parent,
        } => json!({
            "kind": kind, "status": status, "trigger": trigger, "summary": summary,
            "preTokens": pre_tokens, "uuid": uuid, "parent": parent,
        }),
        AgentEvent::Fallback {
            from_model,
            to_model,
            retracted_uuids,
            uuid,
        } => json!({
            "kind": kind, "from": from_model, "to": to_model, "retracted": retracted_uuids, "uuid": uuid,
        }),
        AgentEvent::TurnEnd(r) => json!({"kind": kind, "result": result(r)}),
        AgentEvent::User { .. } | AgentEvent::PlanProposal { .. } => {
            panic!("the Claude parser never emits {kind}")
        }
    }
}

#[test]
fn parser_matches_go() {
    let rows: Vec<Row> = serde_json::from_str(include_str!("golden/go_stream.json")).unwrap();
    assert!(rows.len() > 100, "corpus has {} lines", rows.len());
    let mut bad = Vec::new();
    for row in rows {
        let (error, events) = match parse_line(row.line.as_bytes()) {
            Ok(evs) => (false, evs.iter().map(normalize).collect::<Vec<_>>()),
            Err(_) => (true, Vec::new()),
        };
        if error != row.error || events != row.events {
            bad.push(format!(
                "{}\n  rust error={error} {}\n  go   error={} {}",
                row.line,
                Value::from(events),
                row.error,
                Value::from(row.events)
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "{} mismatches:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
