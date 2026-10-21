use async_trait::async_trait;
use recur_domain::Escalation;

use crate::Result;

/// Rust port of Go `app.EscalationService` (`frontend/bindings/recur/internal/app/escalationservice.ts`).
#[async_trait]
pub trait EscalationService: Send + Sync {
    /// AnswerEscalation delivers the human's decision to the RUNNING loop and resumes it
    /// (PRD §13). It routes through the engine, which persists answer + status=answered,
    /// clears the "needs you" inbox row, emits escalation.resolved, and delivers
    /// Command{CmdAnswerEscalation} to the loop's command channel so it resumes from the
    /// blocked checkpoint (re-dispatching first if the loop was interrupted). For the
    /// seeded LOOP-2218 demo escalation (no live goroutine) the engine still persists
    /// the answer and moves it out of "needs you" so the UI is honest.
    async fn answer_escalation(
        &self,
        workspace_id: &str,
        choice: &str,
        free_text: &str,
        agent_code: &str,
    ) -> Result<Escalation>;

    async fn get_escalation(&self, workspace_id: &str) -> Result<Escalation>;
}
