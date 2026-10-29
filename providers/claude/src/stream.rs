//! The `claude -p --output-format stream-json` parser.
//!
//! One NDJSON line becomes zero or more [`AgentEvent`]s. The top-level `type`
//! selects the shape and unknown types are ignored (the union has ~30
//! variants); blank and unknown lines yield nothing. Shapes follow
//! `docs/claude-headless-reference.md` §3:
//!
//! - `system/init`: session_id, model, mcp_servers[]
//! - `assistant`: message.content[] with text, thinking, tool_use and
//!   fallback blocks
//! - `user`: message.content[] with tool_result blocks
//! - `result`: success carries `result`; error subtypes have none
//! - `stream_event`: `--include-partial-messages` deltas
//!
//! Lines carrying `parent_tool_use_id` belong to a Task subagent multiplexed
//! onto the parent stream: their tools, text and thinking render inside the
//! Task card, and their result/init/compact/usage never speak for the parent.
//!
//! Decoding rules: `null` is the zero value, unknown fields are ignored,
//! invalid UTF-8 becomes U+FFFD per byte. Claude Code writes JSON.stringify
//! output, so case-insensitive key matching and last-wins duplicate keys never
//! arise and are not emulated.

use std::borrow::Cow;
use std::collections::HashMap;

use recur_agent::display::{is_subagent_launch_meta, summarize_tool_input};
use recur_agent::{AgentEvent, McpServerStatus, TokenUsage, ToolResultEvent, ToolStep, TurnResult};
use recur_domain::{COMPACT_DONE, COMPACT_STARTED};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

/// A line that is not valid JSON for its type's shape.
#[derive(Debug, thiserror::Error)]
#[error("{what}: {source}")]
pub struct ParseError {
    /// `stream-json`, `stream-json assistant`, ...
    pub what: &'static str,
    #[source]
    pub source: serde_json::Error,
}

/// Parses one NDJSON line. Blank lines, unknown types and ignored subtypes
/// yield no events; a malformed line is an error (callers skip it).
pub fn parse_line(line: &[u8]) -> Result<Vec<AgentEvent>, ParseError> {
    let line = trim_ascii_space(line);
    if line.is_empty() {
        return Ok(Vec::new());
    }
    let line = go_lossy(line);
    let line = line.as_ref();

    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Head {
        #[serde(rename = "type", deserialize_with = "nd")]
        typ: String,
        #[serde(deserialize_with = "nd")]
        subtype: String,
        #[serde(deserialize_with = "raw_some")]
        parent_tool_use_id: Option<Box<RawValue>>,
    }
    let head: Head = decode(line, "stream-json")?;

    let parent = nested_parent_tool_use_id(head.parent_tool_use_id.as_deref());
    if !parent.is_empty() {
        let evs = match head.typ.as_str() {
            "assistant" => parse_assistant(line)?,
            "user" => parse_user(line)?,
            "stream_event" => parse_stream_event(line)?,
            "system" if head.subtype == "thinking_tokens" => thinking_tokens(line),
            _ => Vec::new(),
        };
        return Ok(stamp_nested_parent(evs, &parent));
    }

    match head.typ.as_str() {
        "system" => Ok(match head.subtype.as_str() {
            "init" => return parse_init(line),
            // Headless Claude REDACTS thinking text and reports progress as
            // cumulative token estimates. The envelope uuid is per line, not
            // the assistant frame id used for retraction, so it is dropped.
            "thinking_tokens" => thinking_tokens(line),
            "compact_boundary" | "compact_started" => parse_compact_system(line, &head.subtype),
            "status" => parse_compact_status(line),
            "model_refusal_fallback" => parse_refusal_fallback(line),
            "api_retry" => parse_api_retry(line),
            _ => Vec::new(),
        }),
        "assistant" => parse_assistant(line),
        "user" => parse_user(line),
        "result" => parse_result(line),
        "stream_event" => parse_stream_event(line),
        _ => Ok(Vec::new()),
    }
}

/// Trims the four ASCII spaces a line can carry.
fn trim_ascii_space(mut b: &[u8]) -> &[u8] {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\r' | b'\n');
    while b.first().is_some_and(ws) {
        b = &b[1..];
    }
    while b.last().is_some_and(ws) {
        b = &b[..b.len() - 1];
    }
    b
}

/// Each byte of an invalid UTF-8 sequence becomes its own U+FFFD.
fn go_lossy(mut b: &[u8]) -> Cow<'_, str> {
    if let Ok(s) = std::str::from_utf8(b) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(b.len() + 8);
    loop {
        match std::str::from_utf8(b) {
            Ok(s) => {
                out.push_str(s);
                return Cow::Owned(out);
            }
            Err(e) => {
                let (good, rest) = b.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(good).unwrap_or_default());
                let bad = e.error_len().unwrap_or(rest.len());
                out.extend(std::iter::repeat_n('\u{FFFD}', bad));
                b = &rest[bad..];
            }
        }
    }
}

/// Decodes a struct: a top-level `null` is the zero value.
fn decode<T: DeserializeOwned + Default>(line: &str, what: &'static str) -> Result<T, ParseError> {
    serde_json::from_str::<Option<T>>(line)
        .map(Option::unwrap_or_default)
        .map_err(|source| ParseError { what, source })
}

/// `null` decodes as the zero value.
fn nd<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A present field keeps its raw text, `null` included.
fn raw_some<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Box<RawValue>>, D::Error> {
    Box::<RawValue>::deserialize(d).map(Some)
}

/// A string list whose `null` elements decode as "".
fn nd_strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v: Option<Vec<Option<String>>> = Option::deserialize(d)?;
    Ok(v.unwrap_or_default()
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect())
}

/// A struct list whose `null` elements decode as zero values.
fn nd_list<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<Vec<T>, D::Error> {
    let v: Option<Vec<Option<T>>> = Option::deserialize(d)?;
    Ok(v.unwrap_or_default()
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect())
}

fn raw_text(raw: &Option<Box<RawValue>>) -> &str {
    raw.as_deref().map_or("", RawValue::get)
}

/// The parent Task tool-use id of a multiplexed subagent line; empty for the
/// parent turn.
fn nested_parent_tool_use_id(raw: Option<&RawValue>) -> String {
    match raw.map(RawValue::get) {
        None | Some("null") => String::new(),
        Some(r) => serde_json::from_str::<String>(r)
            .map(|s| s.trim().to_owned())
            .unwrap_or_default(),
    }
}

/// Tags subagent events with the parent Task id. Nested result, init,
/// compact, usage and fallback are dropped so they cannot close or speak for
/// the parent turn.
fn stamp_nested_parent(evs: Vec<AgentEvent>, parent: &str) -> Vec<AgentEvent> {
    evs.into_iter()
        .filter_map(|mut ev| {
            match &mut ev {
                AgentEvent::Step(s) => s.parent = parent.to_owned(),
                AgentEvent::ToolResult(r) => r.parent = parent.to_owned(),
                AgentEvent::Text { parent: p, .. }
                | AgentEvent::Thinking { parent: p, .. }
                | AgentEvent::ThinkingTokens { parent: p, .. }
                | AgentEvent::TextDelta { parent: p, .. }
                | AgentEvent::ThinkingDelta { parent: p, .. } => *p = parent.to_owned(),
                _ => return None,
            }
            Some(ev)
        })
        .collect()
}

/// Stamps the frame uuid onto events that do not carry one yet.
fn stamp_uuid(evs: &mut [AgentEvent], id: &str) {
    let id = id.trim();
    if id.is_empty() {
        return;
    }
    for ev in evs {
        let slot = match ev {
            AgentEvent::Text { uuid, .. }
            | AgentEvent::Thinking { uuid, .. }
            | AgentEvent::ThinkingTokens { uuid, .. }
            | AgentEvent::Compact { uuid, .. }
            | AgentEvent::Fallback { uuid, .. } => uuid,
            AgentEvent::Step(s) => &mut s.uuid,
            AgentEvent::ToolResult(r) => &mut r.uuid,
            _ => continue,
        };
        if slot.is_empty() {
            *slot = id.to_owned();
        }
    }
}

fn thinking_tokens(line: &str) -> Vec<AgentEvent> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        estimated_tokens: i64,
    }
    match decode::<M>(line, "") {
        Ok(m) if m.estimated_tokens > 0 => vec![AgentEvent::ThinkingTokens {
            tokens: m.estimated_tokens,
            uuid: String::new(),
            parent: String::new(),
        }],
        _ => Vec::new(),
    }
}

fn parse_init(line: &str) -> Result<Vec<AgentEvent>, ParseError> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        session_id: String,
        #[serde(deserialize_with = "nd")]
        model: String,
        #[serde(deserialize_with = "nd_list")]
        mcp_servers: Vec<ServerStatus>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct ServerStatus {
        #[serde(deserialize_with = "nd")]
        name: String,
        #[serde(deserialize_with = "nd")]
        status: String,
    }
    let m: M = decode(line, "stream-json init")?;
    Ok(vec![AgentEvent::Init {
        session_id: m.session_id,
        model: m.model,
        mcp_servers: m
            .mcp_servers
            .into_iter()
            .map(|s| McpServerStatus {
                name: s.name,
                status: s.status,
            })
            .collect(),
    }])
}

fn compact(status: &str, trigger: String, pre_tokens: i64) -> AgentEvent {
    AgentEvent::Compact {
        status: status.to_owned(),
        trigger,
        summary: String::new(),
        pre_tokens,
        uuid: String::new(),
        parent: String::new(),
    }
}

/// `compact_boundary` (after a compact) and `compact_started` (before the
/// silent window, when the CLI emits it). Malformed lines are ignored.
fn parse_compact_system(line: &str, subtype: &str) -> Vec<AgentEvent> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Meta {
        #[serde(deserialize_with = "nd")]
        trigger: String,
        #[serde(rename = "preTokens", deserialize_with = "nd")]
        camel: i64,
        #[serde(rename = "pre_tokens", deserialize_with = "nd")]
        snake: i64,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(rename = "compactMetadata")]
        camel: Option<Meta>,
        #[serde(rename = "compact_metadata")]
        snake: Option<Meta>,
    }
    let Ok(m) = decode::<M>(line, "") else {
        return Vec::new();
    };
    let status = if subtype == "compact_started" {
        COMPACT_STARTED
    } else {
        COMPACT_DONE
    };
    let (trigger, tokens) = match (m.camel, m.snake) {
        (Some(meta), _) => {
            let t = if meta.camel <= 0 {
                meta.snake
            } else {
                meta.camel
            };
            (meta.trigger, t)
        }
        (None, Some(meta)) => {
            let t = if meta.snake <= 0 {
                meta.camel
            } else {
                meta.snake
            };
            (meta.trigger, t)
        }
        (None, None) => (String::new(), 0),
    };
    vec![compact(status, trigger, tokens)]
}

/// `system/status{status:"compacting"}` during a manual /compact.
fn parse_compact_status(line: &str) -> Vec<AgentEvent> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        status: String,
    }
    match decode::<M>(line, "") {
        Ok(m) if m.status.eq_ignore_ascii_case("compacting") => {
            vec![compact(COMPACT_STARTED, "manual".to_owned(), 0)]
        }
        _ => Vec::new(),
    }
}

/// `system/api_retry` as a notice, so a turn stuck in request retries does
/// not look like a silent hang.
fn parse_api_retry(line: &str) -> Vec<AgentEvent> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        attempt: i64,
        #[serde(deserialize_with = "nd")]
        max_retries: i64,
        error_status: Option<i64>,
        #[serde(deserialize_with = "nd")]
        error: String,
    }
    let m = match decode::<M>(line, "") {
        Ok(m) if m.attempt > 0 => m,
        _ => return Vec::new(),
    };
    let mut reason = if m.error.is_empty() {
        "error".to_owned()
    } else {
        m.error
    };
    if let Some(status) = m.error_status {
        reason = format!("{reason} {status}");
    }
    vec![AgentEvent::Notice {
        text: format!(
            "API request failed ({reason}), retrying {}/{}…",
            m.attempt, m.max_retries
        ),
    }]
}

/// One entry of an Anthropic `message.content[]`.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Block {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    #[serde(deserialize_with = "nd")]
    text: String,
    #[serde(deserialize_with = "nd")]
    thinking: String,
    #[serde(deserialize_with = "nd")]
    signature: String,
    #[serde(deserialize_with = "nd")]
    id: String,
    #[serde(deserialize_with = "nd")]
    name: String,
    #[serde(deserialize_with = "raw_some")]
    input: Option<Box<RawValue>>,
    #[serde(deserialize_with = "nd")]
    tool_use_id: String,
    #[serde(deserialize_with = "nd")]
    is_error: bool,
    /// A string OR a content-block array.
    #[serde(deserialize_with = "raw_some")]
    content: Option<Box<RawValue>>,
    /// Fallback blocks: `{model: "…"}` objects.
    #[serde(deserialize_with = "raw_some")]
    from: Option<Box<RawValue>>,
    #[serde(deserialize_with = "raw_some")]
    to: Option<Box<RawValue>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AssistantUsage {
    #[serde(deserialize_with = "nd")]
    input_tokens: i64,
    #[serde(deserialize_with = "nd")]
    output_tokens: i64,
    #[serde(deserialize_with = "nd")]
    cache_creation_input_tokens: i64,
    #[serde(deserialize_with = "nd")]
    cache_read_input_tokens: i64,
    #[serde(deserialize_with = "nd_list")]
    iterations: Vec<Iteration>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Iteration {
    #[serde(rename = "type", deserialize_with = "nd")]
    typ: String,
    #[serde(deserialize_with = "nd")]
    model: String,
}

fn fallback(from: String, to: String, retracted: Vec<String>, uuid: String) -> AgentEvent {
    AgentEvent::Fallback {
        from_model: from,
        to_model: to,
        retracted_uuids: retracted,
        uuid,
    }
}

fn parse_assistant(line: &str) -> Result<Vec<AgentEvent>, ParseError> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        uuid: String,
        #[serde(deserialize_with = "nd_strings")]
        supersedes: Vec<String>,
        #[serde(deserialize_with = "nd")]
        message: Msg,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Msg {
        #[serde(deserialize_with = "nd")]
        model: String,
        #[serde(deserialize_with = "nd_list")]
        content: Vec<Block>,
        usage: Option<AssistantUsage>,
    }
    let m: M = decode(line, "stream-json assistant")?;
    let mut out = Vec::new();
    // Per-inference usage drives the composer context meter; result usage is
    // cumulative billing and can exceed the window.
    if let Some(u) = &m.message.usage {
        let partial = TokenUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cache_creation_tokens: u.cache_creation_input_tokens,
            cache_read_tokens: u.cache_read_input_tokens,
            context_window: 0,
        };
        if partial.context_used() > 0 {
            out.push(AgentEvent::Usage(partial));
        }
    }
    let (mut content_from, mut content_to) = (String::new(), String::new());
    for b in &m.message.content {
        match b.typ.as_str() {
            "text" if !b.text.trim().is_empty() => out.push(AgentEvent::Text {
                text: b.text.clone(),
                uuid: String::new(),
                parent: String::new(),
            }),
            // Emitted even when REDACTED (empty text + signature): the mirror
            // persists the block with its running token estimate.
            "thinking" if !b.thinking.trim().is_empty() || !b.signature.is_empty() => {
                out.push(AgentEvent::Thinking {
                    text: b.thinking.clone(),
                    signature: b.signature.clone(),
                    uuid: String::new(),
                    parent: String::new(),
                })
            }
            "tool_use" => {
                let input = raw_text(&b.input);
                out.push(AgentEvent::Step(ToolStep {
                    id: b.id.clone(),
                    name: b.name.clone(),
                    summary: summarize_tool_input(&b.name, input),
                    input: input.to_owned(),
                    ..Default::default()
                }));
            }
            "fallback" => {
                if let Some((from, to)) = fallback_hop(raw_text(&b.from), raw_text(&b.to)) {
                    (content_from, content_to) = (from, to);
                }
            }
            _ => {}
        }
    }
    let (mut from, mut to) = fallback_from_iterations(m.message.usage.as_ref());
    if !content_to.is_empty() {
        (from, to) = (content_from, content_to);
    }
    let supersedes: Vec<String> = m
        .supersedes
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    // message.model only overlays on a superseding replacement frame: every
    // ordinary assistant line carries it, which is not a fallback signal.
    let serving = m.message.model.trim();
    if !serving.is_empty() && !supersedes.is_empty() {
        to = serving.to_owned();
    }
    if !to.is_empty() || !supersedes.is_empty() {
        let at = out
            .iter()
            .take_while(|e| matches!(e, AgentEvent::Usage(_)))
            .count();
        out.insert(at, fallback(from, to, supersedes, String::new()));
    }
    stamp_uuid(&mut out, &m.uuid);
    Ok(out)
}

/// A fallback hop from raw `from`/`to` values; `None` without a target.
fn fallback_hop(from: &str, to: &str) -> Option<(String, String)> {
    let (from, to) = (fallback_model_id(from), fallback_model_id(to));
    (!to.is_empty()).then_some((from, to))
}

fn fallback_model_id(raw: &str) -> String {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Obj {
        #[serde(deserialize_with = "nd")]
        model: String,
    }
    if raw.is_empty() {
        return String::new();
    }
    if let Ok(o) = serde_json::from_str::<Option<Obj>>(raw) {
        let m = o.unwrap_or_default().model;
        if !m.trim().is_empty() {
            return m.trim().to_owned();
        }
    }
    serde_json::from_str::<Option<String>>(raw)
        .map(|s| s.unwrap_or_default().trim().to_owned())
        .unwrap_or_default()
}

/// The sticky fallback from `usage.iterations`: the first non-fallback
/// model, and the last `fallback_message` model.
fn fallback_from_iterations(u: Option<&AssistantUsage>) -> (String, String) {
    let (mut from, mut to) = (String::new(), String::new());
    for it in u.map(|u| u.iterations.as_slice()).unwrap_or_default() {
        let model = it.model.trim();
        if it.typ == "fallback_message" {
            if !model.is_empty() {
                to = model.to_owned();
            }
        } else if from.is_empty() && !model.is_empty() {
            from = model.to_owned();
        }
    }
    (from, to)
}

/// `system/model_refusal_fallback`: Claude Code tombstoned the refusing leg
/// and retried on another model (snake_case and camelCase both appear).
fn parse_refusal_fallback(line: &str) -> Vec<AgentEvent> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        uuid: String,
        #[serde(deserialize_with = "nd")]
        original_model: String,
        #[serde(rename = "originalModel", deserialize_with = "nd")]
        original_camel: String,
        #[serde(deserialize_with = "nd")]
        fallback_model: String,
        #[serde(rename = "fallbackModel", deserialize_with = "nd")]
        fallback_camel: String,
        #[serde(deserialize_with = "nd_strings")]
        retracted_message_uuids: Vec<String>,
        #[serde(rename = "retractedMessageUuids", deserialize_with = "nd_strings")]
        retracted_camel: Vec<String>,
    }
    let Ok(m) = decode::<M>(line, "") else {
        return Vec::new();
    };
    let pick = |a: &str, b: &str| {
        let a = a.trim();
        if a.is_empty() { b.trim() } else { a }.to_owned()
    };
    let from = pick(&m.original_model, &m.original_camel);
    let to = pick(&m.fallback_model, &m.fallback_camel);
    let retracted = if m.retracted_message_uuids.is_empty() {
        m.retracted_camel
    } else {
        m.retracted_message_uuids
    };
    if to.is_empty() && retracted.is_empty() {
        return Vec::new();
    }
    vec![fallback(from, to, retracted, m.uuid.trim().to_owned())]
}

fn parse_user(line: &str) -> Result<Vec<AgentEvent>, ParseError> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        uuid: String,
        #[serde(deserialize_with = "nd")]
        message: Msg,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Msg {
        #[serde(deserialize_with = "raw_some")]
        content: Option<Box<RawValue>>,
    }
    let m: M = decode(line, "stream-json user")?;
    // String content (or anything but a block array) carries no tool_result.
    let Ok(blocks) =
        serde_json::from_str::<Option<Vec<Option<Block>>>>(raw_text(&m.message.content))
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for b in blocks.unwrap_or_default().into_iter().flatten() {
        if b.typ != "tool_result" {
            continue;
        }
        let text = tool_result_text(raw_text(&b.content));
        // A background Task ack: keep the Task card pending until a real
        // result (or turn end) arrives.
        if is_subagent_launch_meta(&text) {
            continue;
        }
        out.push(AgentEvent::ToolResult(ToolResultEvent {
            id: b.tool_use_id,
            is_error: b.is_error,
            text,
            ..Default::default()
        }));
    }
    stamp_uuid(&mut out, &m.uuid);
    Ok(out)
}

/// A tool_result payload (string or text blocks) as one plain string.
fn tool_result_text(raw: &str) -> String {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Part {
        #[serde(rename = "type", deserialize_with = "nd")]
        _typ: String,
        #[serde(deserialize_with = "nd")]
        text: String,
    }
    if raw.is_empty() {
        return String::new();
    }
    if let Ok(s) = serde_json::from_str::<Option<String>>(raw) {
        return s.unwrap_or_default();
    }
    if let Ok(parts) = serde_json::from_str::<Option<Vec<Option<Part>>>>(raw) {
        return parts
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .filter(|p| !p.text.trim().is_empty())
            .map(|p| p.text)
            .collect::<Vec<_>>()
            .join("\n");
    }
    raw.trim().to_owned()
}

fn parse_result(line: &str) -> Result<Vec<AgentEvent>, ParseError> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        subtype: String,
        #[serde(deserialize_with = "nd")]
        session_id: String,
        #[serde(deserialize_with = "nd")]
        is_error: bool,
        #[serde(deserialize_with = "nd")]
        num_turns: i64,
        #[serde(deserialize_with = "nd")]
        duration_ms: i64,
        #[serde(deserialize_with = "nd")]
        result: String,
        #[serde(deserialize_with = "nd")]
        error: String,
        usage: Option<Usage>,
        /// Keyed by model id; camelCase fields unlike `usage`.
        #[serde(rename = "modelUsage", deserialize_with = "nd")]
        model_usage: HashMap<String, Option<ModelUsage>>,
        #[serde(deserialize_with = "nd")]
        stop_reason: String,
        #[serde(deserialize_with = "nd")]
        terminal_reason: String,
        #[serde(deserialize_with = "nd")]
        queued_turn_count: i64,
        #[serde(deserialize_with = "raw_some")]
        deferred_tool_use: Option<Box<RawValue>>,
        #[serde(deserialize_with = "nd")]
        origin: Origin,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Usage {
        #[serde(deserialize_with = "nd")]
        input_tokens: i64,
        #[serde(deserialize_with = "nd")]
        output_tokens: i64,
        #[serde(deserialize_with = "nd")]
        cache_creation_input_tokens: i64,
        #[serde(deserialize_with = "nd")]
        cache_read_input_tokens: i64,
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct ModelUsage {
        #[serde(deserialize_with = "nd")]
        context_window: i64,
        #[serde(deserialize_with = "nd")]
        input_tokens: i64,
        #[serde(deserialize_with = "nd")]
        output_tokens: i64,
        #[serde(deserialize_with = "nd")]
        cache_creation_input_tokens: i64,
        #[serde(deserialize_with = "nd")]
        cache_read_input_tokens: i64,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Origin {
        #[serde(deserialize_with = "nd")]
        kind: String,
    }
    let m: M = decode(line, "stream-json result")?;
    if result_is_non_terminal(
        &m.stop_reason,
        &m.terminal_reason,
        m.queued_turn_count,
        raw_text(&m.deferred_tool_use),
    ) {
        return Ok(Vec::new());
    }
    // A --resume replays a background task killed with the previous process
    // as a model-less result before the prompt's own turn. Treating it as
    // terminal closes the turn in ~30ms and hides the real reply.
    if m.origin.kind == "task-notification" && m.num_turns == 0 && !m.is_error {
        return Ok(Vec::new());
    }
    let mut r = TurnResult {
        session_id: m.session_id,
        subtype: m.subtype,
        is_error: m.is_error,
        num_turns: m.num_turns,
        duration_ms: m.duration_ms,
        ..Default::default()
    };
    if let Some(u) = m.usage {
        r.usage.input_tokens = u.input_tokens;
        r.usage.output_tokens = u.output_tokens;
        r.usage.cache_creation_tokens = u.cache_creation_input_tokens;
        r.usage.cache_read_tokens = u.cache_read_input_tokens;
    }
    for u in m.model_usage.into_values().flatten() {
        r.usage.context_window = r.usage.context_window.max(u.context_window);
    }
    if r.subtype == "success" {
        r.text = m.result;
    } else {
        // Any non-success subtype is an error even without is_error, so the
        // engine never advances on a failed turn. Keep whatever message the
        // error variant carried: without it a spend-limit failure reaches the
        // user as a bare subtype that cannot be classified (D56).
        r.is_error = true;
        r.text = if m.error.is_empty() {
            m.result
        } else {
            m.error
        };
    }
    Ok(vec![AgentEvent::TurnEnd(r)])
}

/// Claude 2.1+ mid-run `type:result` frames. They must not end the turn:
/// they would publish `session.turn_end` and close plain-steer stdin while
/// tools or queued user turns are still live.
fn result_is_non_terminal(
    stop_reason: &str,
    terminal_reason: &str,
    queued: i64,
    deferred: &str,
) -> bool {
    queued > 0
        || stop_reason == "tool_deferred"
        || terminal_reason == "tool_deferred"
        || terminal_reason == "background_requested"
        || (!deferred.is_empty() && deferred != "null")
}

/// `--include-partial-messages` lines: text and thinking deltas, redacted
/// thinking token estimates, and live fallback hops.
fn parse_stream_event(line: &str) -> Result<Vec<AgentEvent>, ParseError> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct M {
        #[serde(deserialize_with = "nd")]
        uuid: String,
        #[serde(deserialize_with = "nd")]
        event: Ev,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Ev {
        #[serde(rename = "type", deserialize_with = "nd")]
        typ: String,
        content_block: Option<StartBlock>,
        #[serde(deserialize_with = "nd")]
        delta: Delta,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct StartBlock {
        #[serde(rename = "type", deserialize_with = "nd")]
        typ: String,
        #[serde(deserialize_with = "raw_some")]
        from: Option<Box<RawValue>>,
        #[serde(deserialize_with = "raw_some")]
        to: Option<Box<RawValue>>,
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Delta {
        #[serde(rename = "type", deserialize_with = "nd")]
        typ: String,
        #[serde(deserialize_with = "nd")]
        text: String,
        #[serde(deserialize_with = "nd")]
        thinking: String,
        #[serde(deserialize_with = "nd")]
        estimated_tokens: i64,
    }
    let m: M = decode(line, "stream-json stream_event")?;
    if m.event.typ == "content_block_start"
        && let Some(b) = m
            .event
            .content_block
            .as_ref()
            .filter(|b| b.typ == "fallback")
    {
        let mut out: Vec<AgentEvent> = fallback_hop(raw_text(&b.from), raw_text(&b.to))
            .map(|(from, to)| fallback(from, to, Vec::new(), String::new()))
            .into_iter()
            .collect();
        stamp_uuid(&mut out, &m.uuid);
        return Ok(out);
    }
    if m.event.typ != "content_block_delta" {
        return Ok(Vec::new());
    }
    // The envelope uuid is minted per NDJSON line, not per assistant frame;
    // forwarding it made the live UI open a new row per chunk.
    let d = m.event.delta;
    Ok(match d.typ.as_str() {
        "text_delta" if !d.text.is_empty() => vec![AgentEvent::TextDelta {
            text: d.text,
            parent: String::new(),
        }],
        "thinking_delta" if !d.thinking.is_empty() => vec![AgentEvent::ThinkingDelta {
            text: d.thinking,
            parent: String::new(),
        }],
        // Redacted thinking: the text is withheld, only an estimate.
        "thinking_delta" if d.estimated_tokens > 0 => vec![AgentEvent::ThinkingTokens {
            tokens: d.estimated_tokens,
            uuid: String::new(),
            parent: String::new(),
        }],
        _ => Vec::new(),
    })
}

#[cfg(test)]
mod tests;
