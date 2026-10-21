//! Normalized agent events and turn results (Go `ParsedEvent`, `Result`).
//!
//! Events carry the provider's RAW tool vocabulary: a step's name and input
//! and a result's text are verbatim provider data, because the session store
//! is the lossless record of what the provider did. Rendering them in the
//! common cross-provider vocabulary happens at the display boundary
//! ([`crate::Provider::display_tool`]).
//!
//! String fields follow Go's zero values: empty means absent. `parent` is the
//! provider tool-use id of the Task subagent an event belongs to (Claude
//! `parent_tool_use_id`); empty for the parent turn. `uuid` is the provider
//! frame id used for fallback retraction.

use serde::{Deserialize, Serialize};

/// One `system/init` `mcp_servers[]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerStatus {
    pub name: String,
    /// connected | failed | pending.
    pub status: String,
}

/// Prompt-side token footprint for the context meter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TokenUsage {
    #[serde(skip_serializing_if = "is_zero")]
    pub input_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_creation_tokens: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cache_read_tokens: i64,
    /// The model limit in tokens when known; 0 = unknown.
    #[serde(skip_serializing_if = "is_zero")]
    pub context_window: i64,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl TokenUsage {
    /// Fresh input plus cache read/create. Output tokens do not occupy the
    /// window.
    pub fn context_used(&self) -> i64 {
        self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens
    }

    /// Overlays the latest per-inference footprint onto a terminal result.
    /// Claude/Cursor result usage is cumulative billing across the turn and can
    /// exceed the window; the composer meter needs the latest footprint.
    /// `context_window` is left untouched.
    pub fn prefer_context(&mut self, latest: &TokenUsage) {
        if latest.context_used() <= 0 {
            return;
        }
        self.input_tokens = latest.input_tokens;
        self.output_tokens = latest.output_tokens;
        self.cache_creation_tokens = latest.cache_creation_tokens;
        self.cache_read_tokens = latest.cache_read_tokens;
    }
}

/// The final outcome of an agent turn. JSON names match Go `Result`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TurnResult {
    /// The resume handle to store (Claude session, Codex thread, Cursor
    /// agent id, last Responses id).
    pub session_id: String,
    /// success | error_max_turns | error_during_execution | ...
    pub subtype: String,
    pub is_error: bool,
    pub num_turns: i64,
    /// The provider-measured turn wall time; 0 = no real turn end.
    pub duration_ms: i64,
    /// Final assistant text (success only).
    pub text: String,
    /// Set by the engine's mirroring wrapper when this turn published
    /// `session.turn_end` (a `TurnEnd` with `duration_ms > 0`). Providers never
    /// set it.
    #[serde(skip)]
    pub turn_end_emitted: bool,
    /// The latest per-inference footprint, not cumulative billing.
    #[serde(flatten)]
    pub usage: TokenUsage,
    /// Tail of the CLI's stderr: usage refusals, auth failures and outages
    /// land there, which is what separates transient failures from genuine
    /// ones.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stderr: String,
}

/// A tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolStep {
    /// The provider's tool-use id; the mirror mints Recur's own.
    pub id: String,
    pub name: String,
    /// One-line human summary of the input for the feed (provider-computed).
    pub summary: String,
    /// The verbatim input JSON; empty = none.
    pub input: String,
    pub parent: String,
    pub uuid: String,
}

/// A tool result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResultEvent {
    /// The provider tool-use id this answers.
    pub id: String,
    /// The tool name when the provider reports it (Claude does not).
    pub name: String,
    /// Full arguments when the provider only attaches them on completion
    /// (Codex MCP); empty otherwise.
    pub input: String,
    pub text: String,
    pub is_error: bool,
    /// The provider clipped this result for size.
    pub truncated: bool,
    pub parent: String,
    pub uuid: String,
}

/// One normalized event of a running turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    Init {
        session_id: String,
        model: String,
        mcp_servers: Vec<McpServerStatus>,
    },
    /// A complete assistant text block.
    Text {
        text: String,
        uuid: String,
        parent: String,
    },
    /// Partial text (`include_partials`).
    TextDelta {
        text: String,
        parent: String,
    },
    /// A complete thinking block. Redacted thinking has empty text and a
    /// signature.
    Thinking {
        text: String,
        signature: String,
        uuid: String,
        parent: String,
    },
    /// Partial thinking text (live only).
    ThinkingDelta {
        text: String,
        parent: String,
    },
    /// Cumulative thinking-token estimate (headless Claude redacts the text).
    ThinkingTokens {
        tokens: i64,
        uuid: String,
        parent: String,
    },
    Step(ToolStep),
    ToolResult(ToolResultEvent),
    /// Latest context footprint (live meter; not mirrored).
    Usage(TokenUsage),
    /// Recur status (drain, retry): Activity only, not transcript.
    Notice {
        text: String,
    },
    /// Provider context compaction.
    Compact {
        /// started | done.
        status: String,
        /// auto | manual, when reported.
        trigger: String,
        summary: String,
        pre_tokens: i64,
        uuid: String,
        parent: String,
    },
    /// A user row produced inside the turn (delivered steer, follow-up send).
    /// `live_steer = false` means a new provider run started, so tool ids may
    /// repeat.
    User {
        text: String,
        /// UI label when it differs from `text`.
        display: String,
        live_steer: bool,
    },
    /// Model handoff, and/or provider frames to retract.
    Fallback {
        from_model: String,
        to_model: String,
        retracted_uuids: Vec<String>,
        uuid: String,
    },
    /// A plan proposed through a provider-native tool (Cursor createPlan).
    PlanProposal {
        body: String,
    },
    /// A provider result frame. `duration_ms > 0` marks a real turn end.
    TurnEnd(TurnResult),
}

impl AgentEvent {
    /// The Go `ParsedKind` name.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Text { .. } => "text",
            Self::TextDelta { .. } => "text_delta",
            Self::Thinking { .. } => "thinking",
            Self::ThinkingDelta { .. } => "thinking_delta",
            Self::ThinkingTokens { .. } => "thinking_tokens",
            Self::Step(_) => "step",
            Self::ToolResult(_) => "tool_result",
            Self::Usage(_) => "usage",
            Self::Notice { .. } => "notice",
            Self::Compact { .. } => "compact",
            Self::User { .. } => "user",
            Self::Fallback { .. } => "fallback",
            Self::PlanProposal { .. } => "plan_proposal",
            Self::TurnEnd(_) => "result",
        }
    }

    /// The parent Task tool-use id of a multiplexed subagent event; empty for
    /// the parent turn.
    pub fn parent(&self) -> &str {
        match self {
            Self::Text { parent, .. }
            | Self::TextDelta { parent, .. }
            | Self::Thinking { parent, .. }
            | Self::ThinkingDelta { parent, .. }
            | Self::ThinkingTokens { parent, .. }
            | Self::Compact { parent, .. } => parent,
            Self::Step(s) => &s.parent,
            Self::ToolResult(r) => &r.parent,
            _ => "",
        }
    }

    /// Whether this event belongs to a nested Task subagent.
    pub fn nested(&self) -> bool {
        !self.parent().is_empty()
    }

    /// The provider frame id, when the event carries one.
    pub fn uuid(&self) -> &str {
        match self {
            Self::Text { uuid, .. }
            | Self::Thinking { uuid, .. }
            | Self::ThinkingTokens { uuid, .. }
            | Self::Compact { uuid, .. }
            | Self::Fallback { uuid, .. } => uuid,
            Self::Step(s) => &s.uuid,
            Self::ToolResult(r) => &r.uuid,
            _ => "",
        }
    }
}

/// Remembers the latest `Usage` footprint so a following `TurnEnd` stamps the
/// meter with it instead of cumulative result usage (Go
/// `contextUsageTracker`). Providers run every event through `observe` before
/// emitting it.
#[derive(Debug, Clone, Default)]
pub struct ContextUsageTracker {
    last: TokenUsage,
}

impl ContextUsageTracker {
    pub fn observe(&mut self, ev: &mut AgentEvent) {
        match ev {
            AgentEvent::Usage(u) if u.context_used() > 0 => self.last = u.clone(),
            AgentEvent::TurnEnd(r) => r.usage.prefer_context(&self.last),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_json_matches_go_tags() {
        let r = TurnResult {
            session_id: "s".into(),
            subtype: "success".into(),
            num_turns: 2,
            duration_ms: 10,
            text: "ok".into(),
            turn_end_emitted: true,
            usage: TokenUsage {
                input_tokens: 5,
                context_window: 200_000,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            recur_domain::json::to_string(&r).unwrap(),
            r#"{"sessionId":"s","subtype":"success","isError":false,"numTurns":2,"durationMs":10,"text":"ok","inputTokens":5,"contextWindow":200000}"#
        );
    }

    #[test]
    fn tracker_prefers_last_usage() {
        let mut t = ContextUsageTracker::default();
        let usage = |i, c| {
            AgentEvent::Usage(TokenUsage {
                input_tokens: i,
                cache_read_tokens: c,
                ..Default::default()
            })
        };
        t.observe(&mut usage(50, 10_000));
        t.observe(&mut usage(200, 90_000));
        t.observe(&mut usage(0, 0));
        let mut end = AgentEvent::TurnEnd(TurnResult {
            usage: TokenUsage {
                input_tokens: 250,
                cache_read_tokens: 100_000,
                context_window: 1_000_000,
                ..Default::default()
            },
            ..Default::default()
        });
        t.observe(&mut end);
        let AgentEvent::TurnEnd(r) = end else {
            unreachable!()
        };
        assert_eq!(
            (
                r.usage.input_tokens,
                r.usage.cache_read_tokens,
                r.usage.context_window
            ),
            (200, 90_000, 1_000_000)
        );
        assert_eq!(r.usage.context_used(), 90_200);
    }
}
