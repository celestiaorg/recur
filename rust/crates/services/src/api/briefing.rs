use std::collections::BTreeMap;

use async_trait::async_trait;
use recur_domain::{BriefingSnapshot, ChatItem, WorkspaceChatTab};

use super::AppChatSelection;
use crate::Result;

/// BriefingService is the Wails-bound surface for the Inbox and Ask Recur.
/// Copy and routes come from the engine projection so the UI and coordinator
/// cannot disagree.
///
/// Rust port of Go `app.BriefingService` (`frontend/bindings/recur/internal/app/briefingservice.ts`).
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

    /// NewAppChat opens a fresh Ask Recur tab and returns it.
    async fn new_app_chat(&self) -> Result<WorkspaceChatTab>;

    async fn remember_app_chat_selection(
        &self,
        code: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
    ) -> Result<()>;

    async fn send_app_message(
        &self,
        code: &str,
        text: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
    ) -> Result<String>;

    async fn set_app_chat_focus(&self, visible: bool, code: &str) -> Result<()>;
}
