//! The engine event bus (Go `engine/bus.go`): every engine event, typed.
//!
//! Go published `Event{Name, Data any}`; here each [`EngineEvent`] variant
//! carries a payload struct whose JSON is the Go payload byte for byte. Go
//! built most payloads as `map[string]any`, which `json.Marshal` writes with
//! sorted keys, so those structs declare their fields in sorted key order and
//! serialize straight to the writer (never through `serde_json::Value`, whose
//! key order depends on the workspace feature set). Payloads that were Go
//! structs (`resource.updated`, `permission.requested`) keep struct order.
//!
//! Delivery never drops and keeps per-publisher order: channel subscribers
//! get an unbounded queue, callback subscribers run synchronously in the
//! publisher's task (they must only enqueue).

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::RwLock;
use recur_domain::{
    AgentStep, Artifact, AttentionCounts, ConvPoint, DiffStat, Escalation, FeedItem, Group,
    PendingLineComment, PermissionRequest, Plan, Pr, PrComment, PrParticipant, PrReviewer,
    PrSource, Resource, UserQuestion, UserQuestionAnswer, Workspace, WorkspaceState,
};
use serde::Serialize;
use tokio::sync::mpsc;

/// One engine event. [`EngineEvent::name`] is the Go event name the UI
/// listens on; serializing the event writes only its payload.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum EngineEvent {
    /// `workspace.updated`, loop shape (`publishLoopUpdated`). Persisted.
    WorkspaceUpdated(WorkspaceUpdated),
    /// `workspace.updated`, PR chat host hoist shape.
    WorkspaceHoisted(WorkspaceHoisted),
    /// `workspace.event`. Persisted.
    WorkspaceEvent(WorkspaceEvent),
    /// `workspace.deleted`.
    WorkspaceDeleted(WorkspaceRef),
    /// `workspace.removed` (archived).
    WorkspaceRemoved(WorkspaceRef),
    /// `workspace.watched` (`workspaceId` "" = none).
    WorkspaceWatched(WorkspaceRef),
    /// `workspace.fs_changed`.
    WorkspaceFsChanged(FsChanged),
    /// `workspace.chat_tab`.
    WorkspaceChatTab(ChatTab),
    /// `agent.updated`.
    AgentUpdated(AgentUpdated),
    /// `agent.step`.
    AgentStep(AgentStepEvent),
    /// `resource.updated`.
    ResourceUpdated(Resource),
    /// `inbox.updated`.
    InboxUpdated(InboxUpdated),
    /// `attention.updated`.
    AttentionUpdated(AttentionUpdated),
    /// `briefing.updated`.
    BriefingUpdated(BriefingUpdated),
    /// `notify` (OS notification in the desktop app).
    Notify(Notify),
    /// `permission.requested`.
    PermissionRequested(PermissionRequest),
    /// `permission.resolved`.
    PermissionResolved(PermissionResolved),
    /// `question.asked`.
    QuestionAsked(QuestionAsked),
    /// `question.answered`.
    QuestionAnswered(QuestionAnswered),
    /// `escalation.raised`. Persisted.
    EscalationRaised(EscalationRaised),
    /// `escalation.resolved`.
    EscalationResolved(EscalationResolved),
    /// `findings.updated`.
    FindingsUpdated(WorkspaceRef),
    /// `artifact.created`.
    ArtifactCreated(ArtifactEvent),
    /// `artifact.presented`.
    ArtifactPresented(ArtifactEvent),
    /// `chat.mode_changed`.
    ChatModeChanged(ChatModeChanged),
    /// `chat.renamed`.
    ChatRenamed(ChatRenamed),
    /// `app.chat_tab`.
    AppChatTab(AppChatTab),
    /// `app.focused`.
    AppFocused(AppFocused),
    /// `session.user`.
    SessionUser(SessionUser),
    /// `session.delta`.
    SessionDelta(SessionDelta),
    /// `session.thinking`.
    SessionThinking(SessionThinking),
    /// `session.step`.
    SessionStep(SessionStep),
    /// `session.tool_result`.
    SessionToolResult(SessionToolResult),
    /// `session.tool_input`.
    SessionToolInput(SessionToolInput),
    /// `session.compact`.
    SessionCompact(SessionCompact),
    /// `session.fallback`.
    SessionFallback(SessionFallback),
    /// `session.context_usage`.
    SessionContextUsage(SessionContextUsage),
    /// `session.turn_end`.
    SessionTurnEnd(SessionTurnEnd),
    /// `session.notice`.
    SessionNotice(SessionNotice),
    /// `pr.updated`.
    PrUpdated(PrUpdated),
    /// `pr.comment`.
    PrComment(PrCommentEvent),
    /// `pr.removed`.
    PrRemoved(PrRef),
    /// `prs.synced`.
    PrsSynced(PrsSynced),
    /// `github.branch_rules` (name kept; the payload is forge neutral).
    BranchRules(BranchRules),
}

impl EngineEvent {
    /// The Go event name.
    pub fn name(&self) -> &'static str {
        use EngineEvent as E;
        match self {
            E::WorkspaceUpdated(_) | E::WorkspaceHoisted(_) => "workspace.updated",
            E::WorkspaceEvent(_) => "workspace.event",
            E::WorkspaceDeleted(_) => "workspace.deleted",
            E::WorkspaceRemoved(_) => "workspace.removed",
            E::WorkspaceWatched(_) => "workspace.watched",
            E::WorkspaceFsChanged(_) => "workspace.fs_changed",
            E::WorkspaceChatTab(_) => "workspace.chat_tab",
            E::AgentUpdated(_) => "agent.updated",
            E::AgentStep(_) => "agent.step",
            E::ResourceUpdated(_) => "resource.updated",
            E::InboxUpdated(_) => "inbox.updated",
            E::AttentionUpdated(_) => "attention.updated",
            E::BriefingUpdated(_) => "briefing.updated",
            E::Notify(_) => "notify",
            E::PermissionRequested(_) => "permission.requested",
            E::PermissionResolved(_) => "permission.resolved",
            E::QuestionAsked(_) => "question.asked",
            E::QuestionAnswered(_) => "question.answered",
            E::EscalationRaised(_) => "escalation.raised",
            E::EscalationResolved(_) => "escalation.resolved",
            E::FindingsUpdated(_) => "findings.updated",
            E::ArtifactCreated(_) => "artifact.created",
            E::ArtifactPresented(_) => "artifact.presented",
            E::ChatModeChanged(_) => "chat.mode_changed",
            E::ChatRenamed(_) => "chat.renamed",
            E::AppChatTab(_) => "app.chat_tab",
            E::AppFocused(_) => "app.focused",
            E::SessionUser(_) => "session.user",
            E::SessionDelta(_) => "session.delta",
            E::SessionThinking(_) => "session.thinking",
            E::SessionStep(_) => "session.step",
            E::SessionToolResult(_) => "session.tool_result",
            E::SessionToolInput(_) => "session.tool_input",
            E::SessionCompact(_) => "session.compact",
            E::SessionFallback(_) => "session.fallback",
            E::SessionContextUsage(_) => "session.context_usage",
            E::SessionTurnEnd(_) => "session.turn_end",
            E::SessionNotice(_) => "session.notice",
            E::PrUpdated(_) => "pr.updated",
            E::PrComment(_) => "pr.comment",
            E::PrRemoved(_) => "pr.removed",
            E::PrsSynced(_) => "prs.synced",
            E::BranchRules(_) => "github.branch_rules",
        }
    }

    /// The payload as Go `json.Marshal` wrote it (the `events` table format).
    pub fn payload_json(&self) -> String {
        recur_domain::json::to_string(self).unwrap_or_default()
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// `{workspaceId}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WorkspaceRef {
    #[serde(rename = "workspaceId")]
    pub workspace_id: String,
}

impl WorkspaceRef {
    pub fn new(workspace_id: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
        }
    }
}

/// The loop shape of `workspace.updated`: every field the frontend merges
/// into its live list.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceUpdated {
    pub archived: bool,
    pub base_branch: String,
    pub behind_base: i64,
    pub branch: String,
    pub convergence: Option<Vec<ConvPoint>>,
    pub definition_of_done: String,
    pub diff: DiffStat,
    pub goal: String,
    pub group: Group,
    #[serde(rename = "hostPrId")]
    pub host_pr_id: String,
    pub iteration: i64,
    pub last_error: String,
    pub loop_chat_code: String,
    pub phase: String,
    pub plan: Option<Plan>,
    pub port: i64,
    #[serde(with = "recur_domain::json::float")]
    pub progress: f64,
    pub schedule_id: String,
    pub short: String,
    pub state: WorkspaceState,
    pub status_label: String,
    pub workspace_id: String,
    pub worktree_path: String,
}

impl WorkspaceUpdated {
    /// Go `publishLoopUpdated`: state/phase/iteration are the caller's, the
    /// rest comes from the stored row (zero values when it is gone).
    pub fn from_workspace(
        workspace_id: &str,
        state: WorkspaceState,
        phase: &str,
        iteration: i64,
        w: &Workspace,
    ) -> Self {
        Self {
            archived: w.archived,
            base_branch: w.base_branch.clone(),
            behind_base: w.behind_base,
            branch: w.branch.clone(),
            convergence: w.convergence.clone(),
            definition_of_done: w.definition_of_done.clone(),
            diff: w.diff,
            goal: w.goal.clone(),
            group: w.group.clone(),
            host_pr_id: w.host_pr_id.clone(),
            iteration,
            last_error: w.last_error.clone(),
            loop_chat_code: w.loop_chat_code.clone(),
            phase: phase.to_owned(),
            plan: w.plan.clone(),
            port: w.port,
            progress: w.progress,
            schedule_id: w.schedule_id.clone(),
            short: w.short.clone(),
            state,
            status_label: w.status_label.clone(),
            workspace_id: workspace_id.to_owned(),
            worktree_path: w.worktree_path.clone(),
        }
    }
}

/// The hoist shape of `workspace.updated` (PR chat host moved its chats).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHoisted {
    pub chats_changed: bool,
    pub code_remap: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub focus_chat_code: String,
    pub from_workspace_id: String,
    pub moved_codes: Option<Vec<String>>,
    pub workspace_id: String,
}

/// `workspace.event`: one feed item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEvent {
    pub event: FeedItem,
    pub workspace_id: String,
}

/// `workspace.fs_changed`. The watcher always sends `rescan`; a base-branch
/// retarget sends only the workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsChanged {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescan: Option<bool>,
    pub workspace_id: String,
}

/// `workspace.chat_tab`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTab {
    pub code: String,
    pub title: String,
    pub workspace_id: String,
}

/// `agent.updated`. Ask Recur tabs add `code` and `surface: "app"` with an
/// empty `workspaceId`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUpdated {
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    pub workspace_id: String,
}

/// `agent.step`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStepEvent {
    pub agent_id: String,
    pub step: AgentStep,
    pub workspace_id: String,
}

/// `inbox.updated`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct InboxUpdated {
    pub unread: i64,
}

/// `attention.updated`: the full rollup plus the tab that changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttentionUpdated {
    pub app_unseen: i64,
    pub by_project: Option<BTreeMap<String, i64>>,
    pub by_workspace: Option<BTreeMap<String, i64>>,
    #[serde(skip_serializing_if = "is_false")]
    pub clear_all_unseen: bool,
    pub code: String,
    pub dock: i64,
    pub inbox_unread: i64,
    pub tab_unseen: bool,
    pub unseen: i64,
    pub workspace_id: String,
}

impl AttentionUpdated {
    pub fn new(
        c: AttentionCounts,
        workspace_id: &str,
        code: &str,
        tab_unseen: bool,
        clear_all: bool,
    ) -> Self {
        Self {
            app_unseen: c.app_unseen,
            by_project: c.by_project,
            by_workspace: c.by_workspace,
            clear_all_unseen: clear_all,
            code: code.to_owned(),
            dock: c.dock,
            inbox_unread: c.inbox_unread,
            tab_unseen,
            unseen: c.unseen,
            workspace_id: workspace_id.to_owned(),
        }
    }
}

/// `briefing.updated` (`workspaceId` omitted for an app-wide refresh).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefingUpdated {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `notify`: an OS notification; a click routes to `target`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Notify {
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub title: String,
}

/// `permission.resolved`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PermissionResolved {
    pub allow: bool,
    pub id: String,
    pub reason: String,
}

/// `question.asked`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionAsked {
    pub question: UserQuestion,
    pub workspace_id: String,
}

/// `question.answered` (`answer: {}` when cleared).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QuestionAnswered {
    pub answer: UserQuestionAnswer,
    pub id: String,
}

/// `escalation.raised`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EscalationRaised {
    pub escalation: Escalation,
    pub workspace_id: String,
}

/// `escalation.resolved`. The live re-delivery sends `{workspaceId, answer}`;
/// the answer path adds `agentCode` and `open`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EscalationResolved {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_code: Option<String>,
    pub answer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    pub workspace_id: String,
}

/// `artifact.created` / `artifact.presented`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactEvent {
    pub artifact: Artifact,
    pub workspace_id: String,
}

/// `chat.mode_changed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatModeChanged {
    pub agent_code: String,
    pub from: String,
    pub mode: String,
    pub workspace_id: String,
}

/// `chat.renamed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRenamed {
    pub agent_code: String,
    pub title: String,
    pub workspace_id: String,
}

/// `app.chat_tab`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AppChatTab {
    pub code: String,
    pub title: String,
}

/// `app.focused`: `{}`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AppFocused {}

/// The routing fields every `session.*` live event carries (Go
/// `sessionBroadcast.publish`): `surface` ("app" for Ask Recur), the
/// workspace (omitted for app chats), the tab code and its conductor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRoute {
    pub surface: String,
    pub workspace_id: String,
    pub code: String,
    pub owner_code: String,
}

/// `session.user`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUser {
    pub code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `session.delta`: cumulative assistant text with a monotonic `seq`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDelta {
    pub code: String,
    #[serde(skip_serializing_if = "is_false")]
    pub done: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub replace: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uuid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `session.thinking`: cumulative reasoning text, or a token estimate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionThinking {
    pub code: String,
    #[serde(skip_serializing_if = "is_false")]
    pub done: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub replace: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uuid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `session.step`: one tool call. `parentId` is always present ("" for a
/// top-level call).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStep {
    pub code: String,
    pub description: String,
    pub id: String,
    pub input: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    pub parent_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    pub tool: String,
    pub tool_input: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uuid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `session.tool_result`. `input`/`toolInput` ride along when the provider
/// attached full arguments only on completion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionToolResult {
    pub code: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    pub is_error: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    pub result: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `session.tool_input` (MCP argument backfill).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionToolInput {
    pub code: String,
    pub id: String,
    pub input: String,
    pub tool_input: String,
    pub workspace_id: String,
}

/// `session.compact`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompact {
    pub code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_tokens: Option<i64>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    pub trigger: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `session.fallback`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFallback {
    pub code: String,
    pub from: String,
    pub from_label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retracted: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    pub to: String,
    pub to_label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// The context fill a usage-bearing result reports (Go `mergeContextUsage`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextUsage {
    pub input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub context_window: i64,
}

/// `session.context_usage`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContextUsage {
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub code: String,
    pub context_window: i64,
    pub input_tokens: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `session.turn_end`: the provider's wall time, plus the final context fill
/// when the result carried usage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTurnEnd {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    pub duration_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `session.notice`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotice {
    pub code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

impl SessionRoute {
    pub fn user(&self, text: impl Into<String>) -> EngineEvent {
        EngineEvent::SessionUser(SessionUser {
            code: self.code.clone(),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            text: text.into(),
            workspace_id: self.workspace_id.clone(),
        })
    }

    pub fn delta(&self) -> SessionDelta {
        SessionDelta {
            code: self.code.clone(),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            workspace_id: self.workspace_id.clone(),
            ..SessionDelta::default()
        }
    }

    pub fn thinking(&self) -> SessionThinking {
        SessionThinking {
            code: self.code.clone(),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            workspace_id: self.workspace_id.clone(),
            ..SessionThinking::default()
        }
    }

    pub fn step(&self) -> SessionStep {
        SessionStep {
            code: self.code.clone(),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            workspace_id: self.workspace_id.clone(),
            ..SessionStep::default()
        }
    }

    pub fn tool_result(&self) -> SessionToolResult {
        SessionToolResult {
            code: self.code.clone(),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            workspace_id: self.workspace_id.clone(),
            ..SessionToolResult::default()
        }
    }

    pub fn compact(&self) -> SessionCompact {
        SessionCompact {
            code: self.code.clone(),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            workspace_id: self.workspace_id.clone(),
            ..SessionCompact::default()
        }
    }

    pub fn fallback(&self) -> SessionFallback {
        SessionFallback {
            code: self.code.clone(),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            workspace_id: self.workspace_id.clone(),
            ..SessionFallback::default()
        }
    }

    pub fn context_usage(&self, u: ContextUsage) -> EngineEvent {
        EngineEvent::SessionContextUsage(SessionContextUsage {
            cache_creation_tokens: u.cache_creation_tokens,
            cache_read_tokens: u.cache_read_tokens,
            code: self.code.clone(),
            context_window: u.context_window,
            input_tokens: u.input_tokens,
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            workspace_id: self.workspace_id.clone(),
        })
    }

    pub fn turn_end(&self, duration_ms: i64, usage: Option<ContextUsage>) -> EngineEvent {
        EngineEvent::SessionTurnEnd(SessionTurnEnd {
            cache_creation_tokens: usage.map(|u| u.cache_creation_tokens),
            cache_read_tokens: usage.map(|u| u.cache_read_tokens),
            code: self.code.clone(),
            context_window: usage.map(|u| u.context_window),
            duration_ms,
            input_tokens: usage.map(|u| u.input_tokens),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            workspace_id: self.workspace_id.clone(),
        })
    }

    pub fn notice(&self, text: impl Into<String>) -> EngineEvent {
        EngineEvent::SessionNotice(SessionNotice {
            code: self.code.clone(),
            owner_code: self.owner_code.clone(),
            surface: self.surface.clone(),
            text: text.into(),
            workspace_id: self.workspace_id.clone(),
        })
    }
}

/// `pr.updated`: every PR field the list and detail views merge live.
/// `pendingOnly` marks a draft-comment-only change (skip GetPR). Go read the
/// slices off a decoded row, where an empty omitempty slice is nil, so empty
/// ones serialize as `null`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrUpdated {
    pub allowed_merge_methods: Option<Vec<String>>,
    pub attention_reasons: Option<Vec<String>>,
    pub base: String,
    pub behind_base: i64,
    pub branch: String,
    pub checks_ok: bool,
    pub diff: DiffStat,
    pub draft: bool,
    pub file_count: i64,
    pub group: String,
    pub has_checks: bool,
    pub merge_state_status: String,
    pub mergeable: String,
    pub number: i64,
    pub participants: Option<Vec<PrParticipant>>,
    pub pending_comments: Option<Vec<PendingLineComment>>,
    #[serde(skip_serializing_if = "is_false")]
    pub pending_only: bool,
    pub pr_id: String,
    pub project_id: String,
    pub review_decision: String,
    pub review_requested: bool,
    pub reviewers: Option<Vec<PrReviewer>>,
    pub source: PrSource,
    pub state: String,
    pub title: String,
    pub updating_base: String,
    pub when: String,
    pub workspace_id: String,
}

fn nil_if_empty<T: Clone>(v: &[T]) -> Option<Vec<T>> {
    if v.is_empty() { None } else { Some(v.to_vec()) }
}

impl PrUpdated {
    /// Go `emitPRUpdated`.
    pub fn from_pr(pr_id: &str, pr: &Pr, pending_only: bool) -> Self {
        Self {
            allowed_merge_methods: nil_if_empty(&pr.allowed_merge_methods),
            attention_reasons: nil_if_empty(&pr.attention_reasons),
            base: pr.base.clone(),
            behind_base: pr.behind_base,
            branch: pr.branch.clone(),
            checks_ok: pr.checks_ok,
            diff: pr.diff,
            draft: pr.draft,
            file_count: pr.file_count,
            group: pr.group.clone(),
            has_checks: pr.has_checks,
            merge_state_status: pr.merge_state_status.clone(),
            mergeable: pr.mergeable.clone(),
            number: pr.number,
            participants: nil_if_empty(&pr.participants),
            pending_comments: nil_if_empty(&pr.pending_comments),
            pending_only,
            pr_id: pr_id.to_owned(),
            project_id: pr.project_id.clone(),
            review_decision: pr.review_decision.clone(),
            review_requested: pr.review_requested,
            reviewers: nil_if_empty(&pr.reviewers),
            source: pr.source.clone(),
            state: pr.state.clone(),
            title: pr.title.clone(),
            updating_base: pr.updating_base.clone(),
            when: pr.when.clone(),
            workspace_id: pr.workspace_id.clone(),
        }
    }
}

/// `pr.comment`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrCommentEvent {
    pub comment: PrComment,
    pub pr_id: String,
}

/// `pr.removed`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrRef {
    pub pr_id: String,
}

/// `prs.synced`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrsSynced {
    pub project_id: String,
    pub prs: Option<Vec<Pr>>,
}

/// `github.branch_rules`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BranchRules {
    pub branch: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    pub remote: String,
    pub repo: String,
    #[serde(rename = "requirePR")]
    pub require_pr: bool,
}

type Callback = Arc<dyn Fn(&EngineEvent) + Send + Sync>;

enum Subscriber {
    Channel(mpsc::UnboundedSender<Arc<EngineEvent>>),
    Callback(Callback),
}

/// Fans every event out to every subscriber, in publish order.
#[derive(Default)]
pub struct Bus {
    subs: RwLock<Vec<Subscriber>>,
}

impl Bus {
    pub fn new() -> Self {
        Self::default()
    }

    /// An unbounded, ordered stream of every event published from now on.
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<Arc<EngineEvent>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subs.write().push(Subscriber::Channel(tx));
        rx
    }

    /// Runs `f` synchronously for every event, in registration order, on
    /// the publisher's task. `f` must not block or publish.
    pub fn subscribe_fn(&self, f: impl Fn(&EngineEvent) + Send + Sync + 'static) {
        self.subs.write().push(Subscriber::Callback(Arc::new(f)));
    }

    /// Delivers `ev` to every subscriber. A dropped channel receiver is
    /// pruned.
    pub fn publish(&self, ev: EngineEvent) {
        let ev = Arc::new(ev);
        let mut closed = false;
        let callbacks: Vec<Callback> = {
            let subs = self.subs.read();
            let mut out = Vec::new();
            for s in subs.iter() {
                match s {
                    Subscriber::Channel(tx) => {
                        if tx.send(Arc::clone(&ev)).is_err() {
                            closed = true;
                        }
                    }
                    Subscriber::Callback(f) => out.push(Arc::clone(f)),
                }
            }
            out
        };
        for f in callbacks {
            f(&ev);
        }
        if closed {
            self.subs.write().retain(|s| match s {
                Subscriber::Channel(tx) => !tx.is_closed(),
                Subscriber::Callback(_) => true,
            });
        }
    }
}
