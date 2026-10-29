use async_trait::async_trait;
use recur_domain::{ChatAttachment, ChatItem};

use crate::Result;

/// Rust port of Go `app.MaestroService` (`frontend/bindings/recur/internal/app/maestroservice.ts`).
#[async_trait]
pub trait MaestroService: Send + Sync {
    /// CancelMaestroMessage interrupts an in-flight Maestro chat turn. No-op if
    /// nothing is running.
    async fn cancel_maestro_message(&self, workspace_id: &str) -> Result<()>;

    /// GetMaestroChat returns the loop's Maestro transcript (text, reasoning, tool calls)
    /// from the session store — the single source of truth every chat surface reads — so
    /// the UI reloads full history across app launches (PRD §8.5).
    async fn get_maestro_chat(&self, workspace_id: &str) -> Result<Vec<ChatItem>>;

    /// SendMaestroMessage runs a real Maestro chat turn for a loop and returns the
    /// streamed reply (PRD §8.5, §15.1, M7). It runs an INDEPENDENT `claude -p` turn via
    /// the engine's AgentRunner (ClaudeRunner in prod, ScriptedRunner in tests) on the
    /// caller's goroutine — it never blocks a running loop's goroutine. Continuity is via
    /// --resume <maestro_session> (the engine persists the session id per loop); steering
    /// (a plan/escalate MCP tool call) is applied by the engine ONLY within the rails — a
    /// rail can never be crossed via chat (enforced in Go, not the prompt). The turn is
    /// mirrored to the durable session store (GetMaestroChat reads it back).
    async fn send_maestro_message(
        &self,
        workspace_id: &str,
        text: &str,
        attachments: &[ChatAttachment],
    ) -> Result<String>;
}
