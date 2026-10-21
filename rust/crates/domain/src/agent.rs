//! Agents, the activity feed, and the durable event log.

use serde::{Deserialize, Serialize};

use crate::DiffStat;
use crate::serde_util::{is_false, is_zero};

string_enum! {
    /// An app-owned role with a scoped tool set (PRD §8.2).
    AgentRole {
        Maestro = "maestro",
        Coder = "coder",
        Reviewer = "reviewer",
        Tester = "tester",
        Qa = "qa",
    }
}

/// A spawned specialist or the Maestro. Inspection is read-only (PRD §8.5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    pub workspace_id: String,
    /// 2-letter avatar: MA/CO/QA/RV/TE/PR.
    pub code: String,
    pub name: String,
    /// Descriptive subtitle.
    pub role: String,
    /// opus/sonnet.
    pub model: String,
    /// directing/working/idle.
    pub status: String,
    /// The provider session id (for resume/inspection).
    pub session_id: String,
    pub color: String,
    pub task: String,
    /// Index of the current (in-progress) step, -1 if none.
    pub cur: i64,
    /// `None` is Go's nil slice (`null`).
    pub steps: Option<Vec<AgentStep>>,
    /// A chat-facing agent (PA*/MA) finished a turn the human has not viewed.
    /// Specialists never set this.
    #[serde(skip_serializing_if = "is_false")]
    pub unseen: bool,
}

/// One line in the "see into an agent" stream, backed by stream-json events.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentStep {
    /// plan/spawn/watch/read/edit/run/note/done/open/click/check/ok/cur.
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffStat>,
    /// run/edit/read/ok/cur tint.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
}

// Feed item kinds, stamped by the engine at the source so the UI reads a
// field instead of screen-scraping its own outcome phrases.
/// Agent narration / free text.
pub const FEED_KIND_NOTE: &str = "note";
/// A tool call (carries ToolUseID).
pub const FEED_KIND_TOOL: &str = "tool";
/// An engine-authored milestone (committed, converged, opened PR…).
pub const FEED_KIND_OUTCOME: &str = "outcome";
/// A logged departure from a default (IsDeviation).
pub const FEED_KIND_DEVIATION: &str = "deviation";

// Typed cockpit cards (FeedItem.Card).
/// Children of one board round and their outcomes.
pub const FEED_CARD_FAN_OUT: &str = "fanout";
/// A filed structured plan: summary + steps.
pub const FEED_CARD_PLAN: &str = "plan";

/// An activity-feed entry (PRD §15.2 loop.event). Deviations render
/// specially.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FeedItem {
    pub id: String,
    pub agent_code: String,
    pub name: String,
    /// May contain inline <code> (seed data is trusted).
    pub text: String,
    pub time: String,
    /// note|tool|outcome|deviation. Empty on legacy rows and seed data, where
    /// the UI falls back to its heuristic.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub click_agent: String,
    pub is_deviation: bool,
    /// For deviations: the default departed from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub rule: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub why: String,
    /// Tool-call correlation, patched when the matching tool_result arrives.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool_use_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool_result: String,
    #[serde(skip_serializing_if = "is_false")]
    pub is_tool_error: bool,
    /// The real event timestamp (unix ms) on rows read back from the events
    /// table. Live-emitted rows may leave it 0 ("now").
    #[serde(skip_serializing_if = "is_zero")]
    pub ts: i64,
    /// The conductor tab this fact belongs to (one chat = one loop).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub chat_code: String,
    /// A typed cockpit card rendered from Data (JSON); Text is the plain
    /// fallback. Re-emitted under the same ID as it progresses.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub card: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub data: String,
}

/// One row of the append-only durable event log (PRD §6.5, §14; D13): the
/// persisted form of an engine event with its raw payload as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StoredEvent {
    pub id: i64,
    pub workspace_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    /// Event name, e.g. "workspace.event" / "workspace.updated".
    pub r#type: String,
    pub payload_json: String,
    /// Unix milliseconds.
    pub ts: i64,
}
