//! Run-stream frames from `SdkAgentService/Send` and `ObserveRun`. Decoding
//! rules: a frame whose fields have the wrong JSON type fails as a whole (the
//! client skips it), `null` leaves zero values, and raw fields keep their
//! verbatim text.

use serde::{Deserialize, Deserializer, Serialize};

use crate::client::ModelSelection;
use crate::wire::{decode_struct, flex_int, null_default, raw};

/// One frame. The bridge encodes a oneof, so exactly one field is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RunStreamEvent {
    #[serde(deserialize_with = "null_default")]
    pub offset: String,
    /// The normalized conversation event Recur drives the feed from.
    #[serde(rename = "sdkMessage")]
    pub sdk_message: Option<SdkMessage>,
    /// The terminal envelope.
    pub result: Option<RunStreamResult>,
    /// Raw onDelta frames (`enableDeltas`); only summary and turn-ended
    /// usage are read. Empty when absent.
    #[serde(rename = "interactionUpdate", deserialize_with = "raw")]
    pub interaction_update: String,
    /// A conversation-step view of the same information; decoded so a step
    /// frame is not mistaken for an unknown event.
    #[serde(deserialize_with = "raw")]
    pub step: String,
    /// Graceful stream completion, after the result frame.
    #[serde(deserialize_with = "raw")]
    pub done: String,
}

/// The `result` envelope around the terminal [`RunResult`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunStreamResult {
    #[serde(deserialize_with = "null_default")]
    pub agent_id: String,
    #[serde(deserialize_with = "null_default")]
    pub run_id: String,
    /// RUN_LIFECYCLE_STATUS_*.
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    pub result: Option<RunResult>,
}

pub const MSG_STATUS: &str = "status";
pub const MSG_THINKING: &str = "thinking";
pub const MSG_ASSISTANT: &str = "assistant";
pub const MSG_USER: &str = "user";
pub const MSG_TOOL_CALL: &str = "tool_call";
pub const MSG_USAGE: &str = "usage";
pub const MSG_SYSTEM: &str = "system";
/// SDKTaskMessage: task/subagent status, not compaction. Summaries arrive as
/// interaction updates.
pub const MSG_TASK: &str = "task";
pub const MSG_REQUEST: &str = "request";

/// tool_call statuses. The bridge sends "running" (not the CLI's
/// "started"), and "error" is documented but not reliably sent: a failed
/// read emits "running" and then nothing. The runner treats any non-running
/// status as terminal.
pub const TOOL_RUNNING: &str = "running";
pub const TOOL_COMPLETED: &str = "completed";
pub const TOOL_ERROR: &str = "error";

pub const STATUS_RUNNING: &str = "RUNNING";
pub const STATUS_FINISHED: &str = "FINISHED";
pub const STATUS_ERROR: &str = "ERROR";

/// The tool_call `name` of every MCP tool; server and tool live in the args.
pub const TOOL_NAME_MCP: &str = "mcp";

/// One normalized conversation event, flattened from the wire's nesting
/// (`{"type":…,"message":{"type":…,"agent_id":…,"message":{…}}}`) with
/// snake_case or camelCase keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SdkMessage {
    pub kind: String,
    pub agent_id: String,
    pub run_id: String,
    /// status: RUNNING | FINISHED | ERROR. tool_call: running | completed.
    pub status: String,
    /// status: on ERROR, the provider's own reason. The proto RunResult has
    /// no error field, so this is the only place the reason crosses the wire.
    pub message: String,
    pub subtype: String,
    /// thinking text; the final thinking frame is empty and carries the
    /// duration.
    pub text: String,
    pub thinking_duration_ms: i64,
    /// assistant / user body.
    pub content: Option<MessageContent>,
    pub call_id: String,
    pub name: String,
    /// Verbatim args JSON; empty when absent.
    pub args: String,
    /// Verbatim result JSON; empty when absent.
    pub result: String,
    pub truncated: Truncation,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Payload {
    #[serde(rename = "type", deserialize_with = "null_default")]
    kind: String,
    #[serde(deserialize_with = "null_default")]
    agent_id: String,
    #[serde(rename = "agentId", deserialize_with = "null_default")]
    agent_id2: String,
    #[serde(deserialize_with = "null_default")]
    run_id: String,
    #[serde(rename = "runId", deserialize_with = "null_default")]
    run_id2: String,
    #[serde(deserialize_with = "null_default")]
    subtype: String,
    #[serde(deserialize_with = "null_default")]
    status: String,
    #[serde(deserialize_with = "null_default")]
    text: String,
    #[serde(deserialize_with = "null_default")]
    thinking_duration_ms: i64,
    #[serde(rename = "thinkingDurationMs", deserialize_with = "null_default")]
    thinking_duration_ms2: i64,
    #[serde(deserialize_with = "null_default")]
    call_id: String,
    #[serde(rename = "callId", deserialize_with = "null_default")]
    call_id2: String,
    /// Raw so a numeric JSON-RPC id cannot fail the whole frame.
    #[serde(deserialize_with = "raw")]
    id: String,
    #[serde(deserialize_with = "null_default")]
    tool_call_id: String,
    #[serde(rename = "toolCallId", deserialize_with = "null_default")]
    tool_call_id2: String,
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(deserialize_with = "raw")]
    args: String,
    #[serde(deserialize_with = "raw")]
    result: String,
    #[serde(deserialize_with = "null_default")]
    truncated: Truncation,
    /// The `{role, content[]}` body, or (on the outer envelope) the nested
    /// payload.
    #[serde(deserialize_with = "raw")]
    message: String,
    usage: Option<TokenUsage>,
}

impl<'de> Deserialize<'de> for SdkMessage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Box::<serde_json::value::RawValue>::deserialize(d)?;
        let outer: Payload =
            decode_struct(raw.get()).map_err(|_| serde::de::Error::custom("not an SDK message"))?;
        // When `message` is itself a typed payload, it is the real event body
        // and the outer object is an envelope; otherwise the outer object
        // already is the payload.
        let inner = (!outer.message.is_empty())
            .then(|| decode_struct::<Payload>(&outer.message).ok())
            .flatten()
            .filter(|p| !p.kind.is_empty());
        let outer_kind = outer.kind.clone();
        let p = inner.unwrap_or(outer);

        let mut m = SdkMessage {
            kind: first_non_empty(&[&p.kind, &outer_kind]),
            agent_id: first_non_empty(&[&p.agent_id, &p.agent_id2]),
            run_id: first_non_empty(&[&p.run_id, &p.run_id2]),
            subtype: p.subtype,
            status: p.status,
            text: p.text,
            thinking_duration_ms: if p.thinking_duration_ms == 0 {
                p.thinking_duration_ms2
            } else {
                p.thinking_duration_ms
            },
            call_id: first_non_empty(&[
                &p.call_id,
                &p.call_id2,
                &json_string(&p.id),
                &p.tool_call_id,
                &p.tool_call_id2,
            ]),
            name: p.name,
            args: p.args,
            result: p.result,
            truncated: p.truncated,
            usage: p.usage,
            ..Default::default()
        };
        // assistant/user carry their body one level deeper; status carries a
        // plain string.
        if !p.message.is_empty() {
            if let Ok(c) = serde_json::from_str::<MessageContent>(&p.message)
                && !c.role.is_empty()
            {
                m.content = Some(c);
            }
            if let Ok(Some(s)) = serde_json::from_str::<Option<String>>(&p.message) {
                m.message = s;
            }
        }
        Ok(m)
    }
}

fn first_non_empty(vals: &[&str]) -> String {
    vals.iter()
        .find(|v| !v.is_empty())
        .map(|v| (*v).to_owned())
        .unwrap_or_default()
}

/// A JSON string's contents; empty for numbers, objects and `null`.
fn json_string(raw: &str) -> String {
    if !raw.starts_with('"') {
        return String::new();
    }
    serde_json::from_str::<String>(raw).unwrap_or_default()
}

/// Which parts of a tool_call frame the SDK clipped for size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Truncation {
    #[serde(deserialize_with = "null_default")]
    pub args: bool,
    #[serde(deserialize_with = "null_default")]
    pub result: bool,
}

impl Truncation {
    pub fn any(&self) -> bool {
        self.args || self.result
    }
}

/// The MCP routing fields of a tool_call's args; the tool's own parameters
/// stay raw in `params`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolCallArgs {
    #[serde(deserialize_with = "null_default")]
    pub provider_identifier: String,
    #[serde(deserialize_with = "null_default")]
    pub tool_name: String,
    /// Verbatim; empty when absent.
    #[serde(rename = "args", deserialize_with = "raw")]
    pub params: String,
}

/// Decodes a tool_call args envelope. Empty input is an empty
/// envelope.
pub fn tool_call_args(args: &str) -> Option<ToolCallArgs> {
    if args.is_empty() {
        return Some(ToolCallArgs::default());
    }
    decode_struct::<ToolCallArgs>(args).ok()
}

impl SdkMessage {
    /// A stable tool name: built-ins report their own ("read", "shell"); MCP
    /// tools all report "mcp", so this rewrites them to Claude Code's
    /// `mcp__<server>__<tool>`.
    pub fn tool_label(&self) -> String {
        if self.name != TOOL_NAME_MCP {
            return self.name.clone();
        }
        match tool_call_args(&self.args) {
            Some(a) if !a.tool_name.is_empty() => {
                if a.provider_identifier.is_empty() {
                    format!("mcp__{}", a.tool_name)
                } else {
                    format!("mcp__{}__{}", a.provider_identifier, a.tool_name)
                }
            }
            _ => self.name.clone(),
        }
    }

    /// Whether a completed tool_call failed: `{"status":…,"value":{"isError":…}}`.
    /// MCP tools can report protocol success with an application-level
    /// isError, so both count.
    pub fn tool_result_is_error(&self) -> bool {
        if self.result.is_empty() {
            return false;
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct V {
            #[serde(rename = "isError", deserialize_with = "null_default")]
            is_error: bool,
        }
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct R {
            #[serde(deserialize_with = "null_default")]
            status: String,
            #[serde(deserialize_with = "null_default")]
            value: V,
        }
        match decode_struct::<R>(&self.result) {
            Ok(r) => r.value.is_error || (!r.status.is_empty() && r.status != "success"),
            Err(_) => false,
        }
    }
}

/// The `{role, content[]}` body of assistant/user messages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct MessageContent {
    #[serde(deserialize_with = "null_default")]
    pub role: String,
    #[serde(deserialize_with = "null_default")]
    pub content: Vec<ContentBlock>,
}

/// One content block; only text blocks matter to Recur.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ContentBlock {
    #[serde(rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub text: String,
}

impl MessageContent {
    /// The text blocks, concatenated.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter(|b| b.kind == "text")
            .map(|b| b.text.as_str())
            .collect()
    }
}

/// Per-run token accounting. No dollar cost: GetUsage is cloud-only. Counts
/// are JSON numbers on streaming usage messages but strings in the terminal
/// result (proto int64).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TokenUsage {
    #[serde(deserialize_with = "flex_int")]
    pub input_tokens: i64,
    #[serde(deserialize_with = "flex_int")]
    pub output_tokens: i64,
    #[serde(deserialize_with = "flex_int")]
    pub cache_read_tokens: i64,
    #[serde(deserialize_with = "flex_int")]
    pub cache_write_tokens: i64,
    #[serde(deserialize_with = "flex_int")]
    pub total_tokens: i64,
    #[serde(deserialize_with = "flex_int", skip_serializing_if = "is_zero")]
    pub reasoning_tokens: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl TokenUsage {
    /// Prompt-side fill: input + cache read + cache write.
    pub fn prompt_tokens(&self) -> i64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

/// `total - sub`, clamped at zero per field. Strips pre-compaction billing
/// from a later cumulative snapshot.
pub fn sub_usage(total: &TokenUsage, sub: &TokenUsage) -> TokenUsage {
    let clamp = |a: i64, b: i64| if a < b { 0 } else { a - b };
    TokenUsage {
        input_tokens: clamp(total.input_tokens, sub.input_tokens),
        output_tokens: clamp(total.output_tokens, sub.output_tokens),
        cache_read_tokens: clamp(total.cache_read_tokens, sub.cache_read_tokens),
        cache_write_tokens: clamp(total.cache_write_tokens, sub.cache_write_tokens),
        total_tokens: clamp(total.total_tokens, sub.total_tokens),
        reasoning_tokens: clamp(total.reasoning_tokens, sub.reasoning_tokens),
    }
}

pub const DELTA_SUMMARY_STARTED: &str = "summary-started";
pub const DELTA_SUMMARY_COMPLETED: &str = "summary-completed";
pub const DELTA_SUMMARY: &str = "summary";
pub const DELTA_TURN_ENDED: &str = "turn-ended";

/// The InteractionUpdate type and its body, from either the SDK's
/// `{type:"summary-started"}` shape (the bridge wraps it as
/// `{type, update}`) or protobuf-JSON oneof wrappers.
pub fn parse_interaction_type(input: &str) -> (String, String) {
    if input.is_empty() {
        return (String::new(), String::new());
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Tagged {
        #[serde(rename = "type", deserialize_with = "null_default")]
        kind: String,
        #[serde(deserialize_with = "raw")]
        update: String,
    }
    if let Ok(t) = decode_struct::<Tagged>(input) {
        if !t.kind.is_empty() {
            // Summary text lives on the inner object, not the wrapper.
            let body = if t.update.is_empty() {
                input.to_owned()
            } else {
                t.update
            };
            return (t.kind, body);
        }
        if !t.update.is_empty() {
            return parse_interaction_type(&t.update);
        }
    }
    #[derive(Deserialize, Default)]
    #[serde(default, rename_all = "camelCase")]
    struct OneOf {
        #[serde(deserialize_with = "raw")]
        summary_started: String,
        #[serde(deserialize_with = "raw")]
        summary_completed: String,
        #[serde(deserialize_with = "raw")]
        summary: String,
        #[serde(deserialize_with = "raw")]
        turn_ended: String,
    }
    let Ok(o) = decode_struct::<OneOf>(input) else {
        return (String::new(), String::new());
    };
    let pick = [
        (DELTA_SUMMARY_STARTED, o.summary_started),
        (DELTA_SUMMARY_COMPLETED, o.summary_completed),
        (DELTA_SUMMARY, o.summary),
        (DELTA_TURN_ENDED, o.turn_ended),
    ];
    pick.into_iter()
        .find(|(_, b)| !b.is_empty())
        .map(|(t, b)| (t.to_owned(), b))
        .unwrap_or_default()
}

/// The terminal outcome of a run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunResult {
    #[serde(deserialize_with = "null_default")]
    pub run_id: String,
    #[serde(deserialize_with = "null_default")]
    pub agent_id: String,
    #[serde(deserialize_with = "null_default")]
    pub request_id: String,
    /// RUN_LIFECYCLE_STATUS_*.
    #[serde(deserialize_with = "null_default")]
    pub status: String,
    #[serde(deserialize_with = "null_default")]
    pub result: String,
    pub error: Option<RunError>,
    pub model: Option<ModelSelection>,
    #[serde(deserialize_with = "flex_int")]
    pub duration_ms: i64,
    pub usage: Option<TokenUsage>,
}

/// The terminal failure detail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RunError {
    #[serde(deserialize_with = "null_default")]
    pub message: String,
    #[serde(deserialize_with = "null_default")]
    pub code: String,
}

pub const RUN_CREATING: &str = "RUN_LIFECYCLE_STATUS_CREATING";
pub const RUN_RUNNING: &str = "RUN_LIFECYCLE_STATUS_RUNNING";
pub const RUN_FINISHED: &str = "RUN_LIFECYCLE_STATUS_FINISHED";
pub const RUN_ERROR: &str = "RUN_LIFECYCLE_STATUS_ERROR";
pub const RUN_CANCELLED: &str = "RUN_LIFECYCLE_STATUS_CANCELLED";
pub const RUN_EXPIRED: &str = "RUN_LIFECYCLE_STATUS_EXPIRED";

/// Whether a lifecycle status is terminal. Non-terminal ones
/// (creating/running/unspecified) block Send with "already has active run".
pub fn run_status_terminal(status: &str) -> bool {
    matches!(
        status,
        RUN_FINISHED | RUN_ERROR | RUN_CANCELLED | RUN_EXPIRED
    )
}

/// Whether the run ended badly. A missing result counts as an error: a
/// stream that ends without one did not complete.
pub fn run_result_is_error(r: Option<&RunResult>) -> bool {
    match r {
        None => true,
        Some(r) => r.error.is_some() || (!r.status.is_empty() && r.status != RUN_FINISHED),
    }
}

impl RunResult {
    pub fn is_error(&self) -> bool {
        run_result_is_error(Some(self))
    }
}

/// Parses one frame; `None` for a frame of the wrong shape (the
/// client skips it).
pub fn decode_frame(raw: &[u8]) -> Option<RunStreamEvent> {
    serde_json::from_slice::<RunStreamEvent>(raw).ok()
}

#[cfg(test)]
mod tests;
