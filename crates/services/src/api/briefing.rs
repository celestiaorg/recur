use std::collections::BTreeMap;

use async_trait::async_trait;
use recur_domain::{
    BriefingSnapshot, ChatItem, WorkspaceChanges, WorkspaceChatTab, WorkspaceOverview,
};

use super::AppChatSelection;
use crate::Result;

/// BriefingService is the surface for the Inbox and Ask Recur.
/// Copy and routes come from the engine projection so the UI and coordinator
/// cannot disagree.
#[async_trait]
pub trait BriefingService: Send + Sync {
    async fn app_chat_items(&self, code: &str) -> Result<Vec<ChatItem>>;

    async fn app_chat_selection(&self, code: &str) -> Result<AppChatSelection>;

    async fn cancel_app_message(&self, code: &str) -> Result<()>;

    /// CloseAppChat closes a secondary tab (the primary APP tab is not closable).
    /// Returns the remaining open tabs.
    async fn close_app_chat(&self, code: &str) -> Result<Vec<WorkspaceChatTab>>;

    async fn get_briefing(&self) -> Result<BriefingSnapshot>;

    /// ListAppChats returns the open Ask Recur tabs (primary first).
    async fn list_app_chats(&self) -> Result<Vec<WorkspaceChatTab>>;

    async fn mark_app_chat_seen(&self, code: &str) -> Result<()>;

    async fn mark_workspace_viewed(
        &self,
        workspace_id: &str,
        seen_through_activity_at: i64,
    ) -> Result<()>;

    /// Brings back the workspace's unread cue until it is viewed again.
    async fn mark_workspace_unread(&self, workspace_id: &str) -> Result<()>;

    /// NewAppChat opens a fresh Ask Recur tab and returns it.
    async fn new_app_chat(&self) -> Result<WorkspaceChatTab>;

    async fn remember_app_chat_selection(
        &self,
        code: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
    ) -> Result<()>;

    /// `viewing` is the workspace open in the Inbox overview ("" = none).
    async fn send_app_message(
        &self,
        code: &str,
        text: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
        viewing: &str,
    ) -> Result<String>;

    async fn set_app_chat_focus(&self, visible: bool, code: &str) -> Result<()>;

    /// Sends `message` to a workspace's agent (steers a live turn, else starts
    /// one); returns what happened, for a toast.
    async fn message_workspace(&self, workspace_id: &str, message: &str) -> Result<String>;

    /// One workspace's Inbox overview (no git run; see `workspace_changes`).
    async fn workspace_overview(&self, workspace_id: &str) -> Result<WorkspaceOverview>;

    /// A workspace's committed changes against its project's base branch.
    async fn workspace_changes(&self, workspace_id: &str) -> Result<WorkspaceChanges>;

    /// Interrupts the workspace's live chat turns and stops its workflow.
    async fn stop_workspace(&self, workspace_id: &str) -> Result<()>;
}
