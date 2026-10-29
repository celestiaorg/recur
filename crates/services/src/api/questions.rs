use async_trait::async_trait;
use recur_domain::{ChatAttachment, UserQuestion};

use crate::Result;

/// QuestionsService answers parked ask_user prompts. Not durable across
/// restarts; answering resumes the chat with a new turn.
///
/// Rust port of Go `app.QuestionsService` (`frontend/bindings/recur/internal/app/questionsservice.ts`).
#[async_trait]
pub trait QuestionsService: Send + Sync {
    async fn answer_question(
        &self,
        id: &str,
        choice: &str,
        free_text: &str,
        attachments: &[ChatAttachment],
    ) -> Result<()>;

    async fn list_pending_questions(&self) -> Result<Vec<UserQuestion>>;
}
