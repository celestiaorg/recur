//! The deterministic cross-workspace Now/Next projection. Copy, reason codes
//! and routes are derived in one place so the Overview UI and Ask Recur
//! cannot disagree.

use serde::{Deserialize, Serialize};

use crate::serde_util::{is_false, is_zero};
use crate::{Check, Finding, PlanStep};

// Overview buckets. A workspace appears in at most one.
pub const BRIEF_BUCKET_NEEDS: &str = "needs";
pub const BRIEF_BUCKET_WORKING: &str = "working";
pub const BRIEF_BUCKET_PICKUP: &str = "pickup";

// Reason codes, highest priority first. The primary Now/Next pair uses the
// first matching code; the rest are secondary.
pub const BRIEF_REASON_PERMISSION: &str = "permission";
pub const BRIEF_REASON_QUESTION: &str = "question";
pub const BRIEF_REASON_ESCALATION: &str = "escalation";
pub const BRIEF_REASON_RETRYABLE_FAILURE: &str = "retryable_failure";
pub const BRIEF_REASON_FAILURE: &str = "failure";
pub const BRIEF_REASON_PLAN_APPROVAL: &str = "plan_approval";
pub const BRIEF_REASON_FINDINGS: &str = "findings";
pub const BRIEF_REASON_PR_ATTENTION: &str = "pr_attention";
pub const BRIEF_REASON_UNSEEN_CHAT: &str = "unseen_chat";
pub const BRIEF_REASON_ACTIVE_AGENT: &str = "active_agent";
pub const BRIEF_REASON_UNFINISHED_PLAN: &str = "unfinished_plan";
pub const BRIEF_REASON_DIRTY_IDLE: &str = "dirty_idle";
pub const BRIEF_REASON_RECENT_CHAT: &str = "recent_chat";

// The semantic kind of a BriefAction (navigation, not a duplicated mutation).
pub const BRIEF_ACTION_OPEN: &str = "open";
pub const BRIEF_ACTION_ANSWER: &str = "answer";
pub const BRIEF_ACTION_REVIEW: &str = "review";
pub const BRIEF_ACTION_CONTINUE: &str = "continue";
pub const BRIEF_ACTION_APPROVE: &str = "approve";

/// The durable kind of the app-scoped Ask Recur session (workspace_id NULL).
pub const SESSION_KIND_APP: &str = "app";

/// The session config_json.code / bus agent code for Ask Recur.
pub const APP_AGENT_CODE: &str = "APP";

/// One projection of current work across workspaces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BriefingSnapshot {
    pub generated_at: i64,
    pub counts: BriefingCounts,
    /// `None` serializes as `null`.
    pub workspaces: Option<Vec<WorkspaceBrief>>,
}

/// The Overview section tally. Independent of inbox unread.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BriefingCounts {
    pub needs: i64,
    pub working: i64,
    pub pickup: i64,
}

/// One Overview card: identity, Now/Next, and routes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceBrief {
    pub workspace_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project_name: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub branch: String,
    pub bucket: String,
    /// `None` serializes as `null`.
    pub reasons: Option<Vec<String>>,
    pub now: String,
    pub next: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub last_activity_at: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub last_viewed_at: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub seen_activity_at: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub changed_since_viewed: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub quiet_age: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phase: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plan_summary: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub plan_done: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub plan_total: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub active_agents: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub active_agent_task: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latest_chat_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub latest_chat_title: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub diff_add: i64,
    #[serde(skip_serializing_if = "is_zero")]
    pub diff_del: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub pr_id: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub pr_number: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub is_pr_host: bool,
    pub primary: BriefAction,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub secondary: Vec<BriefAction>,
}

/// A navigation target on an existing surface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BriefAction {
    pub label: String,
    pub target: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// The coordinator's show-in-chat payload for present_workspace. It does not
/// switch the visible shell; the transcript renders a card the human opens.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresentWorkspaceResult {
    pub ok: bool,
    pub target: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brief: Option<WorkspaceBrief>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// The bounded detail returned only to the coordinator
/// (`get_workspace_context`). Not a full transcript dump.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceContext {
    pub brief: WorkspaceBrief,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plan_steps: Vec<PlanStep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AgentSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub open_asks: Vec<OpenAsk>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_chats: Vec<ChatHeader>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conversation: Vec<ConversationLine>,
}

/// The Inbox overview of one workspace: its context plus the live detail
/// (latest reply, recent tool steps, failing checks).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceOverview {
    pub context: WorkspaceContext,
    /// A turn, loop or agent is live.
    pub running: bool,
    pub last_reply: String,
    /// Tool calls of the most recent chat, oldest first.
    pub recent_steps: Vec<String>,
    pub failing_checks: Vec<Check>,
    /// Reviewer findings still open, most severe first.
    pub open_findings: Vec<Finding>,
    /// The plan's headline and full markdown brief (what an approval approves).
    pub plan_summary: String,
    pub plan_body: String,
}

/// A workspace's committed changes against its project's base branch (the
/// Inbox overview fetches them apart: they cost a git run).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceChanges {
    pub base_branch: String,
    pub files: Vec<ChangedFile>,
}

/// One changed file and its line counts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChangedFile {
    pub path: String,
    pub add: i64,
    pub del: i64,
}

/// One live or recent agent on a workspace, for context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSummary {
    pub code: String,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub task: String,
    #[serde(skip_serializing_if = "is_false")]
    pub unseen: bool,
}

/// A parked question, permission, or escalation the human still owes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OpenAsk {
    /// permission | question | escalation.
    pub kind: String,
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent_code: String,
    pub summary: String,
}

/// A session header without transcript (title, mode, recency).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatHeader {
    pub code: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub unseen: bool,
}

/// The PR state the coordinator may cite.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrSummary {
    pub id: String,
    pub number: i64,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub review_decision: String,
    /// `Pr::checks_summary`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub checks: String,
}

/// One human-visible excerpt line (user/assistant text).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationLine {
    pub role: String,
    pub text: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub at: i64,
}
