//! App-server notifications to normalized events (Go `consumeTurn`'s
//! notification switch, `handleItemStarted`, `handleItemCompleted` and the
//! file-change, reasoning and token-usage helpers in `engine/codex.go`).
//!
//! Tool calls keep Codex's raw vocabulary (`shell`, `edit`, `write`,
//! `delete`, `mcp__<server>__<tool>`); [`crate::display`] maps them to the
//! common cards at render time.

use std::collections::{BTreeMap, HashMap};

use recur_agent::{AgentEvent, ToolResultEvent, ToolStep};
use serde_json::Value;
use serde_json::value::RawValue;

use crate::approvals::FileItems;
use crate::types::TokenUsage;

/// Per-turn state shared by the notification handler and the result.
#[derive(Debug, Default)]
pub struct TurnState {
    /// Streamed agent text, or the first completed message.
    pub final_text: String,
    /// The latest single-request footprint (context meter).
    pub usage_last: TokenUsage,
    /// Turn tokens: thread-cumulative total minus the baseline.
    pub usage_turn: TokenUsage,
    pub baseline: TokenUsage,
    pub baseline_set: bool,
    pub context_window: i64,
    /// Item id → tool name.
    pub open_tools: HashMap<String, String>,
    /// Item id → MCP arguments (start may be the only copy).
    pub mcp_args: HashMap<String, String>,
}

/// A JSON object with each member's raw text, so arguments and results
/// survive verbatim (Go `json.RawMessage`).
struct Obj(HashMap<String, Box<RawValue>>);

impl Obj {
    fn parse(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok().map(Self)
    }

    fn raw(&self, k: &str) -> Option<&str> {
        self.0.get(k).map(|r| r.get())
    }

    /// The string member; empty when absent, null or another type (Go's
    /// partial decode leaves a mistyped field zero).
    fn str(&self, k: &str) -> String {
        self.strict(k).unwrap_or_default()
    }

    /// `None` when the member is present with a non-string type, which makes
    /// Go's decode of the whole head fail.
    fn strict(&self, k: &str) -> Option<String> {
        match self.raw(k) {
            None | Some("null") => Some(String::new()),
            Some(r) => serde_json::from_str(r).ok(),
        }
    }

    fn value(&self, k: &str) -> Value {
        self.raw(k)
            .and_then(|r| serde_json::from_str(r).ok())
            .unwrap_or(Value::Null)
    }
}

/// The `item` of an `item/*` notification and its `type`/`id`, or `None`
/// where Go's head decode fails.
fn item_of(params: &str, started: bool) -> Option<(String, Obj, String, String)> {
    let p = Obj::parse(params)?;
    let raw = p.raw("item")?.to_owned();
    let item = Obj::parse(&raw)?;
    let typ = item.strict("type")?;
    let id = item.strict("id")?;
    if started {
        item.strict("server")?;
        item.strict("tool")?;
    }
    Some((raw, item, typ, id))
}

fn encode(m: &BTreeMap<&str, &str>) -> String {
    serde_json::to_string(m).unwrap_or_default()
}

/// `item/started`: a tool call begins.
pub fn item_started(
    params: &str,
    st: &mut TurnState,
    file_items: &FileItems,
) -> Option<AgentEvent> {
    let (raw, item, typ, id) = item_of(params, true)?;
    match typ.as_str() {
        "mcpToolCall" => {
            let (server, tool) = (item.str("server"), item.str("tool"));
            let name = format!("mcp__{server}__{tool}");
            st.open_tools.insert(id.clone(), name.clone());
            let input = match item.raw("arguments") {
                Some(args) => {
                    st.mcp_args.insert(id.clone(), args.to_owned());
                    args.to_owned()
                }
                None => "{}".to_owned(),
            };
            Some(AgentEvent::Step(ToolStep {
                id,
                name,
                summary: format!("{server}/{tool}"),
                input,
                ..Default::default()
            }))
        }
        "commandExecution" => {
            let command = item.str("command");
            st.open_tools.insert(id.clone(), "shell".into());
            Some(AgentEvent::Step(ToolStep {
                id,
                name: "shell".into(),
                input: encode(&BTreeMap::from([("command", command.as_str())])),
                summary: command,
                ..Default::default()
            }))
        }
        "fileChange" => {
            // simplify: item/started is stashed as is; Codex has no live
            // patchUpdated, so an empty changes[] leaves the approval card
            // identifier-only.
            if !id.is_empty() {
                file_items.lock().insert(id.clone(), raw.clone());
            }
            let (tool, path, input) = file_change_step(&raw);
            st.open_tools.insert(id.clone(), tool.clone());
            Some(AgentEvent::Step(ToolStep {
                id,
                name: tool,
                summary: path,
                input,
                ..Default::default()
            }))
        }
        _ => None,
    }
}

/// `item/completed`: text, reasoning, and tool results.
pub fn item_completed(params: &str, st: &mut TurnState) -> Option<AgentEvent> {
    let (raw, item, typ, id) = item_of(params, false)?;
    match typ.as_str() {
        "agentMessage" => {
            let mut text = item.str("text");
            if text.is_empty()
                && let Value::Array(content) = item.value("content")
            {
                for c in &content {
                    if c.get("type").and_then(Value::as_str) == Some("text") {
                        text.push_str(c.get("text").and_then(Value::as_str).unwrap_or_default());
                    }
                }
            }
            if text.is_empty() {
                return None;
            }
            // The completed block wins only over nothing; streamed deltas
            // already hold the same text.
            if st.final_text.is_empty() {
                st.final_text.push_str(&text);
            }
            Some(AgentEvent::Text {
                text,
                uuid: String::new(),
                parent: String::new(),
            })
        }
        "reasoning" => {
            let text = reasoning_summary_text(&raw);
            (!text.is_empty()).then(|| AgentEvent::Thinking {
                text,
                signature: String::new(),
                uuid: String::new(),
                parent: String::new(),
            })
        }
        "mcpToolCall" => {
            let name = st
                .open_tools
                .get(&id)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| format!("mcp__{}__{}", item.str("server"), item.str("tool")));
            let mut text = item.raw("result").unwrap_or_default().to_owned();
            let status = item.str("status");
            let mut is_error =
                status.eq_ignore_ascii_case("failed") || status.eq_ignore_ascii_case("error");
            if let Value::Object(err) = item.value("error") {
                is_error = true;
                text = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
            }
            let args = match item.raw("arguments") {
                Some(a) => a.to_owned(),
                None => st.mcp_args.get(&id).cloned().unwrap_or_default(),
            };
            if !args.is_empty() {
                st.mcp_args.insert(id.clone(), args.clone());
            }
            Some(AgentEvent::ToolResult(ToolResultEvent {
                id,
                name,
                input: args,
                text,
                is_error,
                ..Default::default()
            }))
        }
        "commandExecution" => {
            let exit = item.value("exitCode").as_i64();
            Some(AgentEvent::ToolResult(ToolResultEvent {
                id,
                name: "shell".into(),
                text: item.str("aggregatedOutput"),
                is_error: exit.is_some_and(|c| c != 0),
                ..Default::default()
            }))
        }
        "fileChange" => {
            let (mut tool, _, input) = file_change_step(&raw);
            if let Some(open) = st.open_tools.get(&id).filter(|n| !n.is_empty()) {
                tool = open.clone();
            }
            Some(AgentEvent::ToolResult(ToolResultEvent {
                id,
                text: file_change_result_text(&raw, &tool),
                is_error: item.str("status").eq_ignore_ascii_case("failed"),
                name: tool,
                input,
                ..Default::default()
            }))
        }
        _ => None,
    }
}

/// Maps a `fileChange` item onto Recur's write/delete/edit vocabulary:
/// `(tool, path, input JSON)`. Codex puts create bodies in `changes[].diff`
/// (not a diff); updates carry hunk-only unified diffs (`@@` without
/// `---`/`+++`).
pub fn file_change_step(raw: &str) -> (String, String, String) {
    let changes = changes_of(raw);
    let Some(c0) = changes.first() else {
        return ("edit".into(), String::new(), "{}".into());
    };
    let field = |k: &str| c0.get(k).and_then(Value::as_str).unwrap_or_default();
    let path = field("path").to_owned();
    let kind = c0
        .get("kind")
        .and_then(|k| k.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let (tool, input) = match kind.as_str() {
        "add" => (
            "write",
            encode(&BTreeMap::from([
                ("content", field("diff")),
                ("file_path", path.as_str()),
            ])),
        ),
        "delete" => (
            "delete",
            encode(&BTreeMap::from([("file_path", path.as_str())])),
        ),
        _ => ("edit", encode(&BTreeMap::from([("path", path.as_str())]))),
    };
    (tool.into(), path, input)
}

fn changes_of(raw: &str) -> Vec<Value> {
    match serde_json::from_str::<Value>(raw) {
        Ok(v) => match v.get("changes") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

/// The tool-result body: writes and deletes carry their content in the
/// arguments (Claude's shape), so the result is empty; edits concatenate
/// hunk diffs for the diff viewer.
pub fn file_change_result_text(raw: &str, tool: &str) -> String {
    if matches!(tool, "write" | "Write" | "delete" | "Delete") {
        return String::new();
    }
    changes_of(raw)
        .iter()
        .filter_map(|c| c.get("diff").and_then(Value::as_str))
        .collect()
}

/// Readable summary text of a completed reasoning item. App-server sends
/// `summary` as `[]string`; older shapes use `[{text}]`. Parts are trimmed and
/// joined by blank lines.
pub fn reasoning_summary_text(raw: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return String::new();
    };
    let Some(Value::Array(parts)) = v.get("summary") else {
        return String::new();
    };
    let texts: Option<Vec<&str>> = if parts.iter().all(|p| p.is_string() || p.is_null()) {
        Some(
            parts
                .iter()
                .map(|p| p.as_str().unwrap_or_default())
                .collect(),
        )
    } else if parts.iter().all(|p| p.is_object() || p.is_null()) {
        parts
            .iter()
            .map(|p| match p.get("text") {
                None | Some(Value::Null) => Some(""),
                Some(Value::String(s)) => Some(s.as_str()),
                Some(_) => None,
            })
            .collect()
    } else {
        None
    };
    texts
        .unwrap_or_default()
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// One `thread/tokenUsage/updated` snapshot: the last single request (for
/// the meter), the thread-cumulative total, and the model window. An empty
/// `last` falls back to `total`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub last: TokenUsage,
    pub total: TokenUsage,
    pub window: i64,
}

pub fn observe_usage(params: &str) -> UsageSnapshot {
    let v: Value = serde_json::from_str(params).unwrap_or_default();
    let tu = v.get("tokenUsage").cloned().unwrap_or_default();
    let usage = |k: &str| {
        let u = tu.get(k).cloned().unwrap_or_default();
        let n = |f: &str| u.get(f).and_then(Value::as_i64).unwrap_or_default();
        TokenUsage {
            total_tokens: n("totalTokens"),
            input_tokens: n("inputTokens"),
            cached_input_tokens: n("cachedInputTokens"),
            cache_write_input_tokens: n("cacheWriteInputTokens"),
            output_tokens: n("outputTokens"),
            reasoning_output_tokens: n("reasoningOutputTokens"),
        }
    };
    let total = usage("total");
    let mut last = usage("last");
    if usage_empty(&last) {
        last = total;
    }
    UsageSnapshot {
        last,
        total,
        window: tu
            .get("modelContextWindow")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
    }
}

pub fn usage_empty(u: &TokenUsage) -> bool {
    u.total_tokens == 0
        && u.input_tokens == 0
        && u.cached_input_tokens == 0
        && u.cache_write_input_tokens == 0
}

/// Uncached prompt tokens. Codex `inputTokens` is OpenAI-style (cached is a
/// subset); Recur's meter adds cache reads on top of input.
fn fresh_input(u: &TokenUsage) -> i64 {
    let fresh = u.input_tokens - u.cached_input_tokens;
    if fresh < 0 { u.input_tokens } else { fresh }
}

/// A single-request footprint in the meter's shape.
pub fn meter_usage(u: &TokenUsage, window: i64) -> recur_agent::TokenUsage {
    recur_agent::TokenUsage {
        input_tokens: fresh_input(u),
        output_tokens: u.output_tokens,
        cache_read_tokens: u.cached_input_tokens,
        cache_creation_tokens: u.cache_write_input_tokens,
        context_window: window.max(0),
    }
}

/// The turn result's meter: the last request, else the turn total.
pub fn turn_usage(last: &TokenUsage, turn: &TokenUsage, window: i64) -> recur_agent::TokenUsage {
    meter_usage(if usage_empty(last) { turn } else { last }, window)
}

/// `total - sub`, clamped at zero per field.
pub fn sub_usage(total: &TokenUsage, sub: &TokenUsage) -> TokenUsage {
    let c = |a: i64, b: i64| if a < b { 0 } else { a - b };
    TokenUsage {
        total_tokens: c(total.total_tokens, sub.total_tokens),
        input_tokens: c(total.input_tokens, sub.input_tokens),
        cached_input_tokens: c(total.cached_input_tokens, sub.cached_input_tokens),
        cache_write_input_tokens: c(total.cache_write_input_tokens, sub.cache_write_input_tokens),
        output_tokens: c(total.output_tokens, sub.output_tokens),
        reasoning_output_tokens: c(total.reasoning_output_tokens, sub.reasoning_output_tokens),
    }
}

impl TurnState {
    /// Folds a usage snapshot in and returns the live meter event. Turn
    /// tokens use the delta of the thread total so multi-step turns are not
    /// under-reported by counting only the final request.
    pub fn observe_usage(&mut self, params: &str) -> AgentEvent {
        let snap = observe_usage(params);
        if !usage_empty(&snap.last) {
            self.usage_last = snap.last;
        }
        if snap.window > 0 {
            self.context_window = snap.window;
        }
        if !self.baseline_set && !usage_empty(&snap.total) {
            self.baseline = sub_usage(&snap.total, &snap.last);
            self.baseline_set = true;
        }
        self.usage_turn = if self.baseline_set {
            sub_usage(&snap.total, &self.baseline)
        } else {
            snap.last
        };
        AgentEvent::Usage(meter_usage(&self.usage_last, self.context_window))
    }
}

#[cfg(test)]
mod tests {
    use recur_agent::display::summarize_tool_input;

    use super::*;
    use crate::display::canonical_tool;

    fn started(params: &str, st: &mut TurnState) -> Option<AgentEvent> {
        item_started(params, st, &FileItems::default())
    }

    #[test]
    fn file_change_steps() {
        let raw = r#"{"changes":[{"path":"/tmp/a.ts","kind":{"type":"add"},"diff":"export const a = 1;\n"}]}"#;
        let (tool, path, input) = file_change_step(raw);
        assert_eq!((tool.as_str(), path.as_str()), ("write", "/tmp/a.ts"));
        assert_eq!(
            input,
            r#"{"content":"export const a = 1;\n","file_path":"/tmp/a.ts"}"#
        );
        assert_eq!(file_change_result_text(raw, &tool), "");

        let raw = r#"{"changes":[{"path":"/tmp/a.ts","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-a\n+b\n"}]}"#;
        let (tool, path, input) = file_change_step(raw);
        assert_eq!((tool.as_str(), path.as_str()), ("edit", "/tmp/a.ts"));
        assert_eq!(input, r#"{"path":"/tmp/a.ts"}"#);
        assert!(file_change_result_text(raw, &tool).contains("@@"));

        let raw = r#"{"changes":[{"path":"/tmp/gone.ts","kind":{"type":"delete"}}]}"#;
        let (tool, path, input) = file_change_step(raw);
        assert_eq!((tool.as_str(), path.as_str()), ("delete", "/tmp/gone.ts"));
        assert_eq!(
            summarize_tool_input(canonical_tool(&tool), &input),
            "/tmp/gone.ts"
        );
        assert_eq!(file_change_result_text(raw, &tool), "");
        assert_eq!(
            file_change_step("{}"),
            ("edit".into(), String::new(), "{}".into())
        );
    }

    #[test]
    fn reasoning_summaries() {
        for (raw, want) in [
            (
                r#"{"summary":["**Plan**","Next step"]}"#,
                "**Plan**\n\nNext step",
            ),
            (
                r#"{"summary":[{"text":"one"},{"text":"two"}]}"#,
                "one\n\ntwo",
            ),
            (r#"{"summary":[]}"#, ""),
            (r#"{"summary":null}"#, ""),
            ("{}", ""),
            (r#"{"summary":[" a ","",null]}"#, "a"),
        ] {
            assert_eq!(reasoning_summary_text(raw), want, "{raw}");
        }
    }

    /// Go `TestCodexMCPReportFindingToolName`.
    #[test]
    fn mcp_tool_names() {
        let mut st = TurnState::default();
        let ev = started(
            r#"{"item":{"type":"mcpToolCall","id":"i1","server":"recur","tool":"report_finding","arguments":{"file":"store.go","line":10,"message":"nil deref","severity":"high"}}}"#,
            &mut st,
        );
        let Some(AgentEvent::Step(s)) = ev else {
            panic!("{ev:?}")
        };
        assert_eq!(s.name, "mcp__recur__report_finding");
        assert_eq!(s.summary, "recur/report_finding");
        assert_eq!(
            s.input,
            r#"{"file":"store.go","line":10,"message":"nil deref","severity":"high"}"#
        );
        let done = item_completed(
            r#"{"item":{"type":"mcpToolCall","id":"i1","status":"completed","result":{"content":[]}}}"#,
            &mut st,
        );
        let Some(AgentEvent::ToolResult(r)) = done else {
            panic!("{done:?}")
        };
        assert_eq!(r.name, "mcp__recur__report_finding");
        assert_eq!(r.text, r#"{"content":[]}"#);
        assert_eq!(r.input, s.input, "start args survive onto the result");
        assert!(!r.is_error);
        let failed = item_completed(
            r#"{"item":{"type":"mcpToolCall","id":"i2","server":"x","tool":"y","error":{"message":"boom"}}}"#,
            &mut st,
        );
        let Some(AgentEvent::ToolResult(r)) = failed else {
            panic!()
        };
        assert_eq!(
            (r.name.as_str(), r.text.as_str(), r.is_error),
            ("mcp__x__y", "boom", true)
        );
    }

    #[test]
    fn shell_and_messages() {
        let mut st = TurnState::default();
        let Some(AgentEvent::Step(s)) = started(
            r#"{"item":{"type":"commandExecution","id":"c1","command":"go test ./..."}}"#,
            &mut st,
        ) else {
            panic!()
        };
        assert_eq!(
            (s.name.as_str(), s.summary.as_str(), s.input.as_str()),
            ("shell", "go test ./...", r#"{"command":"go test ./..."}"#)
        );
        let Some(AgentEvent::ToolResult(r)) = item_completed(
            r#"{"item":{"type":"commandExecution","id":"c1","aggregatedOutput":"FAIL","exitCode":1}}"#,
            &mut st,
        ) else {
            panic!()
        };
        assert!(r.is_error && r.text == "FAIL" && r.input.is_empty());
        let ev = item_completed(
            r#"{"item":{"type":"agentMessage","id":"m1","content":[{"type":"text","text":"a"},{"type":"other","text":"x"},{"type":"text","text":"b"}]}}"#,
            &mut st,
        );
        assert_eq!(
            ev,
            Some(AgentEvent::Text {
                text: "ab".into(),
                uuid: String::new(),
                parent: String::new()
            })
        );
        assert_eq!(st.final_text, "ab");
        item_completed(
            r#"{"item":{"type":"agentMessage","text":"later"}}"#,
            &mut st,
        );
        assert_eq!(st.final_text, "ab", "first text wins");
        assert!(started(r#"{"item":{"type":"commandExecution","id":5}}"#, &mut st).is_none());
        assert!(started(r#"{"nope":1}"#, &mut st).is_none());
    }

    #[test]
    fn file_change_items_are_stashed() {
        let items = FileItems::default();
        let mut st = TurnState::default();
        let params = r#"{"item":{"id":"fc1","type":"fileChange","changes":[{"path":"/tmp/a.ts","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-a\n+b\n"}]}}"#;
        let Some(AgentEvent::Step(s)) = item_started(params, &mut st, &items) else {
            panic!()
        };
        assert_eq!((s.name.as_str(), s.summary.as_str()), ("edit", "/tmp/a.ts"));
        assert!(items.lock()["fc1"].contains("@@ -1 +1 @@"));
        let Some(AgentEvent::ToolResult(r)) = item_completed(
            r#"{"item":{"id":"fc1","type":"fileChange","status":"failed","changes":[{"path":"/tmp/a.ts","kind":{"type":"update"},"diff":"@@ x"}]}}"#,
            &mut st,
        ) else {
            panic!()
        };
        assert_eq!(
            (r.name.as_str(), r.text.as_str(), r.is_error),
            ("edit", "@@ x", true)
        );
        assert_eq!(r.input, r#"{"path":"/tmp/a.ts"}"#);
    }

    #[test]
    fn sub_usage_clamps() {
        let got = sub_usage(
            &TokenUsage {
                total_tokens: 100,
                input_tokens: 80,
                output_tokens: 20,
                ..Default::default()
            },
            &TokenUsage {
                total_tokens: 40,
                input_tokens: 30,
                output_tokens: 10,
                cached_input_tokens: 5,
                ..Default::default()
            },
        );
        assert_eq!(
            (
                got.total_tokens,
                got.input_tokens,
                got.output_tokens,
                got.cached_input_tokens
            ),
            (60, 50, 10, 0)
        );
    }

    fn sol_last() -> TokenUsage {
        TokenUsage {
            total_tokens: 156_713,
            input_tokens: 155_843,
            cached_input_tokens: 154_240,
            output_tokens: 870,
            ..Default::default()
        }
    }

    /// Go `TestApplyCodexMeterUsageSplitsInclusiveCache`.
    #[test]
    fn meter_splits_inclusive_cache() {
        let m = meter_usage(&sol_last(), 258_400);
        assert_eq!((m.input_tokens, m.cache_read_tokens), (1603, 154_240));
        assert_eq!(m.context_used(), 155_843);
        assert_eq!(m.context_window, 258_400);
    }

    /// Go `TestStampCodexTurnUsagePrefersLastOverCumulative`.
    #[test]
    fn turn_usage_prefers_last() {
        let cost = TokenUsage {
            total_tokens: 4_034_944,
            input_tokens: 4_020_424,
            cached_input_tokens: 3_859_456,
            output_tokens: 14_520,
            ..Default::default()
        };
        let m = turn_usage(&sol_last(), &cost, 258_400);
        assert_eq!(m.context_used(), 155_843);
        assert_eq!((m.input_tokens, m.cache_read_tokens), (1603, 154_240));
        assert_eq!(
            turn_usage(&TokenUsage::default(), &cost, 0).input_tokens,
            4_020_424 - 3_859_456
        );
    }

    /// Go `TestObserveCodexTokenUsageKeepsLastForMeter`.
    #[test]
    fn observe_keeps_last_for_meter() {
        let params = r#"{"tokenUsage":{"total":{"totalTokens":4034944,"inputTokens":4020424,"cachedInputTokens":3859456,"outputTokens":14520},"last":{"totalTokens":156713,"inputTokens":155843,"cachedInputTokens":154240,"outputTokens":870},"modelContextWindow":258400}}"#;
        let snap = observe_usage(params);
        assert_eq!(
            (snap.last.input_tokens, snap.last.cached_input_tokens),
            (155_843, 154_240)
        );
        assert_eq!(snap.total.input_tokens, 4_020_424);
        assert_eq!(snap.window, 258_400);
        let mut st = TurnState::default();
        let AgentEvent::Usage(u) = st.observe_usage(params) else {
            panic!()
        };
        assert_eq!(u.context_used(), 155_843);
        assert_eq!(
            st.usage_turn,
            sol_last(),
            "first snapshot: turn = last request"
        );
        let only_total = r#"{"tokenUsage":{"total":{"totalTokens":10,"inputTokens":7}}}"#;
        assert_eq!(observe_usage(only_total).last.input_tokens, 7);
    }
}
