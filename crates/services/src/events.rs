//! App events: everything the backend publishes to the UI, and the few
//! events the UI sends back.
//!
//! Each [`AppEvent`] variant is one wire name; [`AppEvent::name`] and
//! [`AppEvent::payload`] give the event's name and data, and
//! [`AppEvent::from_wire`] reads one back. Payload fields that may be absent
//! are `Option`, so a partial event (the service fakes and the specs
//! dispatch those) leaves the view's value alone.
//!
//! [`EventHub`] fans events out to subscribers over unbounded channels: an
//! event is never dropped and each subscriber sees one publisher's events in
//! publish order.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use parking_lot::Mutex;
use recur_domain::{
    AgentStep, Artifact, AttentionCounts, ConvPoint, DiffStat, Escalation, FeedItem, Group,
    OpenPathResult, PendingLineComment, PermissionRequest, Plan, Pr, PrComment, PrParticipant,
    PrReviewer, PrSource, Resource, UserQuestion, UserQuestionAnswer, WorkspaceState,
};
use recur_platform::updater::UpdaterEvent;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::api::Snapshot;

/// Fan-out of [`AppEvent`]s to any number of subscribers. Cloning shares the
/// hub. Publishing never blocks and never drops; a subscriber whose receiver
/// is gone is pruned on the next publish.
#[derive(Clone, Default)]
pub struct EventHub {
    subscribers: Arc<Mutex<Vec<UnboundedSender<AppEvent>>>>,
}

impl EventHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// A receiver for every event published from now on.
    pub fn subscribe(&self) -> UnboundedReceiver<AppEvent> {
        let (tx, rx) = unbounded_channel();
        self.subscribers.lock().push(tx);
        rx
    }

    pub fn publish(&self, event: AppEvent) {
        self.subscribers
            .lock()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }
}

/// Every app event, plus `notify` (the desktop turns it into an OS
/// notification). Variants marked "UI to backend" are emitted by the UI
/// itself; the desktop shell subscribes to them.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// `agent.step`
    AgentStep(AgentStepEvent),
    /// `agent.updated`
    AgentUpdated(AgentUpdated),
    /// `app.focused`: the OS window regained focus.
    AppFocused(AppFocused),
    /// `app.open`: a `recur <path>` open, routed by `OpenPath`.
    AppOpen(OpenPathResult),
    /// `app.ready` (UI to backend): the shell mounted; queued opens may drain.
    AppReady,
    /// `artifact.created`
    ArtifactCreated(ArtifactEvent),
    /// `artifact.presented`
    ArtifactPresented(ArtifactEvent),
    /// `attention.updated`
    AttentionUpdated(AttentionUpdated),
    /// `briefing.updated`
    BriefingUpdated(BriefingUpdated),
    /// `chat.mode_changed`
    ChatModeChanged(ChatModeChanged),
    /// `cursor.install`: Cursor bridge install progress.
    CursorInstall(CursorInstall),
    /// `escalation.raised`
    EscalationRaised(EscalationRaised),
    /// `escalation.resolved`
    EscalationResolved(EscalationResolved),
    /// `files.dropped`: a native OS file drop on the window.
    FilesDropped(FilesDropped),
    /// `findings.updated`
    FindingsUpdated(WorkspaceRef),
    /// `github.branch_rules`
    GithubBranchRules(GithubBranchRules),
    /// `inbox.updated`
    InboxUpdated(InboxUpdated),
    /// `lsp.exit`
    LspExit(LspExit),
    /// `lsp.message`: one JSON-RPC body from a language server.
    LspMessage(LspMessage),
    /// `menu.action`: a native menubar item or accelerator (the action id).
    MenuAction(String),
    /// `menu.state` (UI to backend): menubar enable map.
    MenuState(MenuState),
    /// `menu.themeState` (UI to backend): Appearance radio checks.
    MenuThemeState(MenuThemeState),
    /// `menu.themes` (UI to backend): Appearance theme catalogs.
    MenuThemes(MenuThemes),
    /// `notification.open`: an OS notification was clicked (its inbox target).
    NotificationOpen(String),
    /// `notify`: an OS notification to show (desktop only; the UI ignores it).
    Notify(Notify),
    /// `permission.requested`
    PermissionRequested(PermissionRequest),
    /// `permission.resolved`
    PermissionResolved(PermissionResolved),
    /// `pr.comment`
    PrComment(Arc<PrCommentEvent>),
    /// `pr.removed`
    PrRemoved(PrRef),
    /// `pr.updated`
    PrUpdated(Arc<PrUpdated>),
    /// `provider.usage`
    ProviderUsage(Snapshot),
    /// `prs.synced`
    PrsSynced(PrsSynced),
    /// `question.answered`
    QuestionAnswered(QuestionAnswered),
    /// `question.asked`
    QuestionAsked(QuestionAsked),
    /// `resource.updated`
    ResourceUpdated(Resource),
    /// `session.compact`
    SessionCompact(SessionCompact),
    /// `session.context_usage`
    SessionContextUsage(SessionContextUsage),
    /// `session.delta`
    SessionDelta(SessionDelta),
    /// `session.fallback`
    SessionFallback(SessionFallback),
    /// `session.notice`
    SessionNotice(SessionText),
    /// `session.step`
    SessionStep(SessionStep),
    /// `session.thinking`
    SessionThinking(SessionThinking),
    /// `session.tool_input`
    SessionToolInput(SessionToolInput),
    /// `session.tool_result`
    SessionToolResult(SessionToolResult),
    /// `session.turn_end`
    SessionTurnEnd(SessionTurnEnd),
    /// `session.user`
    SessionUser(SessionText),
    /// `system.theme`: the OS appearance changed (true = dark).
    SystemTheme(bool),
    /// `term.data`, `term.exit`, `term.busy`, `term.idle`.
    Term(recur_term::Event),
    /// `updater.check` (UI to backend): Check for Updates.
    UpdaterCheck,
    /// `updater:*`: updater lifecycle (`update-available`, `no-update`,
    /// `error`, `update-ready`, …).
    Updater(UpdaterEvent),
    /// `updater:user:restart` (UI to backend): restart into the update.
    UpdaterUserRestart,
    /// `window.titlebarDblclick` (UI to backend): zoom/minimize per OS setting.
    WindowTitlebarDblclick,
    /// `workspace.chat_tab`
    WorkspaceChatTab(WorkspaceChatTabEvent),
    /// `workspace.deleted`
    WorkspaceDeleted(WorkspaceRef),
    /// `workspace.event`
    WorkspaceEvent(WorkspaceEvent),
    /// `workspace.file_index`
    WorkspaceFileIndex(WorkspaceFileIndex),
    /// `workspace.fs_changed`
    WorkspaceFsChanged(WorkspaceFsChanged),
    /// `workspace.removed`
    WorkspaceRemoved(WorkspaceRef),
    /// `workspace.updated`
    WorkspaceUpdated(Arc<WorkspaceUpdated>),
}

/// An event name or payload [`AppEvent::from_wire`] cannot read.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("unknown app event {0:?}")]
    UnknownName(String),
    #[error("app event {name}: {source}")]
    Payload {
        name: String,
        source: serde_json::Error,
    },
}

const UPDATER_PREFIX: &str = "updater:";

impl AppEvent {
    /// The event name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::AgentStep(_) => "agent.step",
            Self::AgentUpdated(_) => "agent.updated",
            Self::AppFocused(_) => "app.focused",
            Self::AppOpen(_) => "app.open",
            Self::AppReady => "app.ready",
            Self::ArtifactCreated(_) => "artifact.created",
            Self::ArtifactPresented(_) => "artifact.presented",
            Self::AttentionUpdated(_) => "attention.updated",
            Self::BriefingUpdated(_) => "briefing.updated",
            Self::ChatModeChanged(_) => "chat.mode_changed",
            Self::CursorInstall(_) => "cursor.install",
            Self::EscalationRaised(_) => "escalation.raised",
            Self::EscalationResolved(_) => "escalation.resolved",
            Self::FilesDropped(_) => "files.dropped",
            Self::FindingsUpdated(_) => "findings.updated",
            Self::GithubBranchRules(_) => "github.branch_rules",
            Self::InboxUpdated(_) => "inbox.updated",
            Self::LspExit(_) => "lsp.exit",
            Self::LspMessage(_) => "lsp.message",
            Self::MenuAction(_) => "menu.action",
            Self::MenuState(_) => "menu.state",
            Self::MenuThemeState(_) => "menu.themeState",
            Self::MenuThemes(_) => "menu.themes",
            Self::NotificationOpen(_) => "notification.open",
            Self::Notify(_) => "notify",
            Self::PermissionRequested(_) => "permission.requested",
            Self::PermissionResolved(_) => "permission.resolved",
            Self::PrComment(_) => "pr.comment",
            Self::PrRemoved(_) => "pr.removed",
            Self::PrUpdated(_) => "pr.updated",
            Self::ProviderUsage(_) => "provider.usage",
            Self::PrsSynced(_) => "prs.synced",
            Self::QuestionAnswered(_) => "question.answered",
            Self::QuestionAsked(_) => "question.asked",
            Self::ResourceUpdated(_) => "resource.updated",
            Self::SessionCompact(_) => "session.compact",
            Self::SessionContextUsage(_) => "session.context_usage",
            Self::SessionDelta(_) => "session.delta",
            Self::SessionFallback(_) => "session.fallback",
            Self::SessionNotice(_) => "session.notice",
            Self::SessionStep(_) => "session.step",
            Self::SessionThinking(_) => "session.thinking",
            Self::SessionToolInput(_) => "session.tool_input",
            Self::SessionToolResult(_) => "session.tool_result",
            Self::SessionTurnEnd(_) => "session.turn_end",
            Self::SessionUser(_) => "session.user",
            Self::SystemTheme(_) => "system.theme",
            Self::Term(e) => e.name(),
            Self::UpdaterCheck => "updater.check",
            Self::Updater(e) => e.name(),
            Self::UpdaterUserRestart => "updater:user:restart",
            Self::WindowTitlebarDblclick => "window.titlebarDblclick",
            Self::WorkspaceChatTab(_) => "workspace.chat_tab",
            Self::WorkspaceDeleted(_) => "workspace.deleted",
            Self::WorkspaceEvent(_) => "workspace.event",
            Self::WorkspaceFileIndex(_) => "workspace.file_index",
            Self::WorkspaceFsChanged(_) => "workspace.fs_changed",
            Self::WorkspaceRemoved(_) => "workspace.removed",
            Self::WorkspaceUpdated(_) => "workspace.updated",
        }
    }

    /// The event `data` (`null` for events without one).
    pub fn payload(&self) -> Value {
        fn json<T: Serialize>(v: &T) -> Value {
            serde_json::to_value(v).unwrap_or(Value::Null)
        }
        match self {
            Self::AgentStep(p) => json(p),
            Self::AgentUpdated(p) => json(p),
            Self::AppFocused(p) => json(p),
            Self::AppOpen(p) => json(p),
            Self::AppReady
            | Self::UpdaterCheck
            | Self::UpdaterUserRestart
            | Self::WindowTitlebarDblclick => Value::Null,
            Self::ArtifactCreated(p) | Self::ArtifactPresented(p) => json(p),
            Self::AttentionUpdated(p) => json(p),
            Self::BriefingUpdated(p) => json(p),
            Self::ChatModeChanged(p) => json(p),
            Self::CursorInstall(p) => json(p),
            Self::EscalationRaised(p) => json(p),
            Self::EscalationResolved(p) => json(p),
            Self::FilesDropped(p) => json(p),
            Self::FindingsUpdated(p) | Self::WorkspaceDeleted(p) | Self::WorkspaceRemoved(p) => {
                json(p)
            }
            Self::GithubBranchRules(p) => json(p),
            Self::InboxUpdated(p) => json(p),
            Self::LspExit(p) => json(p),
            Self::LspMessage(p) => json(p),
            Self::MenuAction(s) | Self::NotificationOpen(s) => Value::String(s.clone()),
            Self::MenuState(p) => json(p),
            Self::MenuThemeState(p) => json(p),
            Self::MenuThemes(p) => json(p),
            Self::Notify(p) => json(p),
            Self::PermissionRequested(p) => json(p),
            Self::PermissionResolved(p) => json(p),
            Self::PrComment(p) => json(p),
            Self::PrRemoved(p) => json(p),
            Self::PrUpdated(p) => json(p),
            Self::ProviderUsage(p) => json(p),
            Self::PrsSynced(p) => json(p),
            Self::QuestionAnswered(p) => json(p),
            Self::QuestionAsked(p) => json(p),
            Self::ResourceUpdated(p) => json(p),
            Self::SessionCompact(p) => json(p),
            Self::SessionContextUsage(p) => json(p),
            Self::SessionDelta(p) => json(p),
            Self::SessionFallback(p) => json(p),
            Self::SessionNotice(p) | Self::SessionUser(p) => json(p),
            Self::SessionStep(p) => json(p),
            Self::SessionThinking(p) => json(p),
            Self::SessionToolInput(p) => json(p),
            Self::SessionToolResult(p) => json(p),
            Self::SessionTurnEnd(p) => json(p),
            Self::SystemTheme(dark) => Value::Bool(*dark),
            Self::Term(e) => term_payload(e),
            Self::Updater(e) => e.payload(),
            Self::WorkspaceChatTab(p) => json(p),
            Self::WorkspaceEvent(p) => json(p),
            Self::WorkspaceFileIndex(p) => json(p),
            Self::WorkspaceFsChanged(p) => json(p),
            Self::WorkspaceUpdated(p) => json(p),
        }
    }

    /// Reads an event back: the inverse of [`name`](Self::name) +
    /// [`payload`](Self::payload).
    pub fn from_wire(name: &str, data: Value) -> Result<Self, WireError> {
        fn de<T: DeserializeOwned>(name: &str, data: Value) -> Result<T, WireError> {
            serde_json::from_value(data).map_err(|source| WireError::Payload {
                name: name.to_owned(),
                source,
            })
        }
        let d = data;
        Ok(match name {
            "agent.step" => Self::AgentStep(de(name, d)?),
            "agent.updated" => Self::AgentUpdated(de(name, d)?),
            "app.focused" => Self::AppFocused(de(name, null_object(d))?),
            "app.open" => Self::AppOpen(de(name, d)?),
            "app.ready" => Self::AppReady,
            "artifact.created" => Self::ArtifactCreated(de(name, d)?),
            "artifact.presented" => Self::ArtifactPresented(de(name, d)?),
            "attention.updated" => Self::AttentionUpdated(de(name, d)?),
            "briefing.updated" => Self::BriefingUpdated(de(name, null_object(d))?),
            "chat.mode_changed" => Self::ChatModeChanged(de(name, d)?),
            "cursor.install" => Self::CursorInstall(de(name, d)?),
            "escalation.raised" => Self::EscalationRaised(de(name, d)?),
            "escalation.resolved" => Self::EscalationResolved(de(name, d)?),
            "files.dropped" => Self::FilesDropped(de(name, d)?),
            "findings.updated" => Self::FindingsUpdated(de(name, d)?),
            "github.branch_rules" => Self::GithubBranchRules(de(name, d)?),
            "inbox.updated" => Self::InboxUpdated(de(name, null_object(d))?),
            "lsp.exit" => Self::LspExit(de(name, d)?),
            "lsp.message" => Self::LspMessage(de(name, d)?),
            "menu.action" => Self::MenuAction(menu_action(name, d)?),
            "menu.state" => Self::MenuState(de(name, d)?),
            "menu.themeState" => Self::MenuThemeState(de(name, d)?),
            "menu.themes" => Self::MenuThemes(de(name, d)?),
            "notification.open" => Self::NotificationOpen(de(name, d)?),
            "notify" => Self::Notify(de(name, d)?),
            "permission.requested" => Self::PermissionRequested(de(name, d)?),
            "permission.resolved" => Self::PermissionResolved(de(name, d)?),
            "pr.comment" => Self::PrComment(std::sync::Arc::new(de(name, d)?)),
            "pr.removed" => Self::PrRemoved(de(name, d)?),
            "pr.updated" => Self::PrUpdated(std::sync::Arc::new(de(name, d)?)),
            "provider.usage" => Self::ProviderUsage(de(name, d)?),
            "prs.synced" => Self::PrsSynced(de(name, d)?),
            "question.answered" => Self::QuestionAnswered(de(name, d)?),
            "question.asked" => Self::QuestionAsked(de(name, d)?),
            "resource.updated" => Self::ResourceUpdated(de(name, d)?),
            "session.compact" => Self::SessionCompact(de(name, d)?),
            "session.context_usage" => Self::SessionContextUsage(de(name, d)?),
            "session.delta" => Self::SessionDelta(de(name, d)?),
            "session.fallback" => Self::SessionFallback(de(name, d)?),
            "session.notice" => Self::SessionNotice(de(name, d)?),
            "session.step" => Self::SessionStep(de(name, d)?),
            "session.thinking" => Self::SessionThinking(de(name, d)?),
            "session.tool_input" => Self::SessionToolInput(de(name, d)?),
            "session.tool_result" => Self::SessionToolResult(de(name, d)?),
            "session.turn_end" => Self::SessionTurnEnd(de(name, d)?),
            "session.user" => Self::SessionUser(de(name, d)?),
            "system.theme" => Self::SystemTheme(de(name, d)?),
            "term.data" | "term.exit" | "term.busy" | "term.idle" => {
                Self::Term(term_from_wire(name, d)?)
            }
            "updater.check" => Self::UpdaterCheck,
            "updater:user:restart" => Self::UpdaterUserRestart,
            "window.titlebarDblclick" => Self::WindowTitlebarDblclick,
            "workspace.chat_tab" => Self::WorkspaceChatTab(de(name, d)?),
            "workspace.deleted" => Self::WorkspaceDeleted(de(name, d)?),
            "workspace.event" => Self::WorkspaceEvent(de(name, d)?),
            "workspace.file_index" => Self::WorkspaceFileIndex(de(name, d)?),
            "workspace.fs_changed" => Self::WorkspaceFsChanged(de(name, d)?),
            "workspace.removed" => Self::WorkspaceRemoved(de(name, d)?),
            "workspace.updated" => Self::WorkspaceUpdated(std::sync::Arc::new(de(name, d)?)),
            _ if name.starts_with(UPDATER_PREFIX) => Self::Updater(updater_from_wire(name, d)?),
            _ => return Err(WireError::UnknownName(name.to_owned())),
        })
    }
}

/// Events with an empty map (`{}`) payload also arrive as `null` from callers
/// that emit no data; both read as the empty payload.
fn null_object(d: Value) -> Value {
    if d.is_null() {
        Value::Object(Default::default())
    } else {
        d
    }
}

/// `menu.action` data is the action string, or an `{action}` object.
fn menu_action(name: &str, d: Value) -> Result<String, WireError> {
    match d {
        Value::String(s) => Ok(s),
        Value::Object(mut m) => match m.remove("action") {
            Some(Value::String(s)) => Ok(s),
            _ => Err(WireError::Payload {
                name: name.to_owned(),
                source: serde::de::Error::custom("missing action"),
            }),
        },
        other => serde_json::from_value(other).map_err(|source| WireError::Payload {
            name: name.to_owned(),
            source,
        }),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TermWire {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
}

fn term_payload(e: &recur_term::Event) -> Value {
    let wire = match e {
        recur_term::Event::Data { id, data } => TermWire {
            id: id.clone(),
            workspace_id: None,
            data: Some(base64::engine::general_purpose::STANDARD.encode(data)),
            exit_code: None,
        },
        recur_term::Event::Exit {
            id,
            workspace_id,
            exit_code,
        } => TermWire {
            id: id.clone(),
            workspace_id: Some(workspace_id.clone()),
            data: None,
            exit_code: Some(*exit_code),
        },
        recur_term::Event::Busy { id, workspace_id }
        | recur_term::Event::Idle { id, workspace_id } => TermWire {
            id: id.clone(),
            workspace_id: Some(workspace_id.clone()),
            data: None,
            exit_code: None,
        },
    };
    serde_json::to_value(wire).unwrap_or(Value::Null)
}

fn term_from_wire(name: &str, d: Value) -> Result<recur_term::Event, WireError> {
    let err = |source| WireError::Payload {
        name: name.to_owned(),
        source,
    };
    let w: TermWire = serde_json::from_value(d).map_err(err)?;
    let workspace_id = w.workspace_id.unwrap_or_default();
    Ok(match name {
        "term.data" => recur_term::Event::Data {
            id: w.id,
            data: base64::engine::general_purpose::STANDARD
                .decode(w.data.unwrap_or_default())
                .map_err(|e| err(serde::de::Error::custom(e)))?,
        },
        "term.exit" => recur_term::Event::Exit {
            id: w.id,
            workspace_id,
            exit_code: w.exit_code.unwrap_or_default(),
        },
        "term.busy" => recur_term::Event::Busy {
            id: w.id,
            workspace_id,
        },
        _ => recur_term::Event::Idle {
            id: w.id,
            workspace_id,
        },
    })
}

fn updater_from_wire(name: &str, d: Value) -> Result<UpdaterEvent, WireError> {
    fn de<T: DeserializeOwned>(name: &str, d: Value) -> Result<T, WireError> {
        serde_json::from_value(d).map_err(|source| WireError::Payload {
            name: name.to_owned(),
            source,
        })
    }
    Ok(match &name[UPDATER_PREFIX.len()..] {
        "check-started" => UpdaterEvent::CheckStarted,
        "update-available" => UpdaterEvent::UpdateAvailable(de(name, d)?),
        "no-update" => UpdaterEvent::NoUpdate,
        "download-started" => UpdaterEvent::DownloadStarted(de(name, d)?),
        "download-progress" => UpdaterEvent::DownloadProgress(de(name, d)?),
        "download-complete" => UpdaterEvent::DownloadComplete(de(name, d)?),
        "verifying" => UpdaterEvent::Verifying(de(name, d)?),
        "installing" => UpdaterEvent::Installing(de(name, d)?),
        "update-ready" => UpdaterEvent::UpdateReady(de(name, d)?),
        "error" => UpdaterEvent::Error(de(name, d)?),
        "meta" => UpdaterEvent::Meta(de(name, d)?),
        _ => return Err(WireError::UnknownName(name.to_owned())),
    })
}

/// Decodes a present key as `Some`, `null` included (as the zero value).
/// Absent stays `None` via `#[serde(default)]`. For fields where presence
/// matters, not just the value.
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Some(Option::<T>::deserialize(d)?.unwrap_or_default()))
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// `{workspaceId}`: `workspace.deleted`, `workspace.removed`,
/// `findings.updated`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceRef {
    pub workspace_id: String,
}

/// `{prId}`: `pr.removed`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrRef {
    pub pr_id: String,
}

/// `agent.step`: one line in an agent's inspector stream.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentStepEvent {
    pub workspace_id: String,
    /// `<workspaceId>-<code>`.
    pub agent_id: String,
    pub step: AgentStep,
}

/// `agent.updated`: an agent's status changed. Ask Recur tabs send
/// `surface: "app"`, their code, and an empty workspace id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentUpdated {
    pub workspace_id: String,
    pub agent_id: String,
    /// working / directing / planning / replanning count as running; idle.
    pub status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
}

/// `app.focused`: `{}` or an optional workspace id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppFocused {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `artifact.created` / `artifact.presented`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ArtifactEvent {
    pub workspace_id: String,
    pub artifact: Artifact,
}

/// `attention.updated`: the full badge rollup plus the tab that changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AttentionUpdated {
    #[serde(flatten)]
    pub counts: AttentionCounts,
    pub workspace_id: String,
    pub code: String,
    pub tab_unseen: bool,
    /// Mark all read: every cached chat chip clears.
    #[serde(skip_serializing_if = "is_false")]
    pub clear_all_unseen: bool,
}

/// `briefing.updated`: the Inbox / Ask Recur projection is stale.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BriefingUpdated {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}

/// `chat.mode_changed`: an agent switched its tab's mode.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatModeChanged {
    pub workspace_id: String,
    pub agent_code: String,
    pub mode: String,
    pub from: String,
}

/// `cursor.install`: one progress step, or `{phase: "error", message,
/// percent: -1}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CursorInstall {
    pub phase: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<i64>,
    /// 0..100, or -1 when the total is unknown (and on error).
    pub percent: i64,
}

/// `escalation.raised`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EscalationRaised {
    pub workspace_id: String,
    pub escalation: Escalation,
}

/// `escalation.resolved`. The live-resume path sends only the workspace and
/// answer; the answer path adds the owning chat and whether a sibling
/// escalation keeps the workspace in "needs you".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EscalationResolved {
    pub workspace_id: String,
    pub answer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
}

/// `files.dropped`: absolute paths plus the drop target the OS reported.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FilesDropped {
    pub files: Vec<String>,
    /// The target element's attributes (`data-chat-drop` routes to a tab).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_list: Option<Vec<String>>,
}

/// `github.branch_rules`: a background re-probe changed whether a branch
/// requires a PR.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GithubBranchRules {
    pub project_id: String,
    pub repo: String,
    pub remote: String,
    pub branch: String,
    #[serde(rename = "requirePR")]
    pub require_pr: bool,
}

/// `inbox.updated`: the unread count; views refetch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InboxUpdated {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread: Option<i64>,
}

/// `lsp.exit`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LspExit {
    pub id: String,
}

/// `lsp.message`: one JSON-RPC body (no Content-Length framing).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LspMessage {
    pub id: String,
    pub message: String,
}

/// `menu.state`: partial enable map. An absent key leaves that item alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MenuState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_tab: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_in: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy_path: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_tab: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggle_left_panel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggle_right_panel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggle_editor: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toggle_terminal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximize_terminal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_agent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_editor: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_terminal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_tab: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_tab: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_dev: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_run: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub find: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goto_line: Option<bool>,
}

/// One radio item in a View → Appearance theme menu.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MenuTheme {
    pub key: String,
    pub name: String,
    /// Editor themes only: light / dark.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

/// `menu.themes`: rebuild the Appearance submenus.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MenuThemes {
    pub app: Vec<MenuTheme>,
    pub editor: Vec<MenuTheme>,
    pub terminal: Vec<MenuTheme>,
    pub app_selected: String,
    pub editor_selected: String,
    pub terminal_selected: String,
}

/// `menu.themeState`: update the Appearance radio checks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MenuThemeState {
    pub chrome_dark: bool,
    pub app_selected: String,
    pub editor_selected: String,
    pub terminal_selected: String,
}

/// `notify`: a native OS notification; clicking it emits
/// [`AppEvent::NotificationOpen`] with `target`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Notify {
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub target: String,
}

/// `permission.resolved`: the request left the queue.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PermissionResolved {
    pub id: String,
    pub allow: bool,
    pub reason: String,
}

/// `pr.comment`: a new thread entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrCommentEvent {
    pub pr_id: String,
    pub comment: PrComment,
}

/// `pr.updated`: the fields the PR list merges. An absent one leaves the view's value alone. `pendingOnly` skips the full refetch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrUpdated {
    pub pr_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checks_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_checks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PrSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffStat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind_base: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updating_base: Option<String>,
    /// Present (even `null`, read as empty) replaces the pending drafts.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub pending_comments: Option<Vec<PendingLineComment>>,
    /// Present (even `null`, read as false) replaces the draft flag.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_state_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_requested: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention_reasons: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participants: Option<Vec<PrParticipant>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewers: Option<Vec<PrReviewer>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_merge_methods: Option<Vec<String>>,
    #[serde(skip_serializing_if = "is_false")]
    pub pending_only: bool,
}

/// `prs.synced`: one project's rows from a sweep still in flight.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrsSynced {
    pub project_id: String,
    /// `None` encodes as `null`. Shared: every subscriber gets the event.
    pub prs: Option<Arc<Vec<Pr>>>,
}

/// `question.answered`. A cleared question carries an empty answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QuestionAnswered {
    pub id: String,
    pub answer: UserQuestionAnswer,
}

/// `question.asked`: an `ask_user` prompt parked for the human.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QuestionAsked {
    pub workspace_id: String,
    pub question: UserQuestion,
}

/// Which session a `session.*` event belongs to. Ask Recur sends
/// `surface: "app"`; `ownerCode` is the chat a specialist runs under.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionTag {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub surface: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
    pub code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
}

/// `session.delta`: a cumulative assistant-text snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionDelta {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub text: String,
    /// Monotonic per stream; stale snapshots are ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    /// Set the body instead of appending.
    #[serde(skip_serializing_if = "is_false")]
    pub replace: bool,
    /// The block finished.
    #[serde(skip_serializing_if = "is_false")]
    pub done: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uuid: String,
    /// The parent Task tool id for nested subagent text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

/// `session.thinking`: a cumulative reasoning snapshot, or a token estimate
/// for redacted reasoning.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionThinking {
    #[serde(flatten)]
    pub tag: SessionTag,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    #[serde(skip_serializing_if = "is_false")]
    pub replace: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub done: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

/// `session.step`: a tool call, in the common display vocabulary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionStep {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub id: String,
    pub tool: String,
    pub tool_input: String,
    pub description: String,
    pub input: String,
    /// Empty for a top-level call.
    pub parent_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uuid: String,
}

/// `session.tool_result`: a tool call's result, matched to the step by id.
/// Some providers only attach the full input here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionToolResult {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub id: String,
    pub result: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub input: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool_input: String,
}

/// `session.tool_input`: an args-only refresh (MCP backfill). Does not settle
/// the result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionToolInput {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub id: String,
    pub input: String,
    pub tool_input: String,
}

/// `session.compact`: the provider is (or finished) summarizing context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionCompact {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub status: String,
    pub trigger: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub pre_tokens: i64,
}

/// `session.fallback`: a model declined and another continued, or earlier
/// rows were retracted.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionFallback {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub from: String,
    pub to: String,
    pub from_label: String,
    pub to_label: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retracted: Vec<String>,
}

/// `session.context_usage`: context fill for the composer meter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionContextUsage {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub context_window: i64,
}

/// `session.turn_end`: the provider's own wall time, plus final context usage
/// when the provider reported any.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionTurnEnd {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub duration_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
}

/// `session.user` / `session.notice`: a user bubble (steer) or a Recur status
/// line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SessionText {
    #[serde(flatten)]
    pub tag: SessionTag,
    pub text: String,
}

/// `workspace.chat_tab`: a chat tab's title (provisional or upgraded).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceChatTabEvent {
    pub workspace_id: String,
    pub code: String,
    pub title: String,
}

/// `workspace.event`: one activity-feed item.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceEvent {
    pub workspace_id: String,
    pub event: FeedItem,
}

/// `workspace.file_index`: index readiness (paths stay in the backend).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceFileIndex {
    pub workspace_id: String,
    pub r#gen: u64,
    pub ready: bool,
    pub building: bool,
}

/// `workspace.fs_changed`: the watched worktree changed on disk.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceFsChanged {
    pub workspace_id: String,
    /// Absent on a base-branch change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescan: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// `workspace.updated`: the full loop shape (every field below the
/// id up to `scheduleId`) or the hoist shape (`fromWorkspaceId`, `codeRemap`,
/// `movedCodes`, `chatsChanged`, `focusChatCode`); an absent field leaves the
/// view's value alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkspaceUpdated {
    pub workspace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<WorkspaceState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iteration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convergence: Option<Vec<ConvPoint>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<Group>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_of_done: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffStat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind_base: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_pr_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_chat_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<String>,
    /// Hoist: the source workspace whose chats moved here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_workspace_id: Option<String>,
    /// Hoist: source tab code to destination tab code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_remap: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved_codes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "is_false")]
    pub chats_changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_chat_code: Option<String>,
}
