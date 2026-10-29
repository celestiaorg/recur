//! The resume half of ask_user: the human's answer runs as a new turn on the
//! asking tab. The full resume prompt is the wire (replays and forks see it);
//! the short answer is the bubble.

use recur_domain::{ChatAttachment, UserQuestion};

use crate::Engine;
use crate::codes::MAESTRO_AGENT_CODE;

impl Engine {
    /// The conductor resumes on MA; a pair tab reuses its dock selection (an
    /// empty one would fall back to the Settings default while the stored
    /// handle belongs to another provider).
    pub(crate) async fn send_ask_user_resume(
        &self,
        q: &UserQuestion,
        prompt: &str,
        display: &str,
        attachments: Vec<ChatAttachment>,
    ) {
        let code = q.agent_code.trim();
        if code.is_empty() || code == MAESTRO_AGENT_CODE {
            self.send_maestro_message(&q.workspace_id, prompt, &attachments)
                .await;
            return;
        }
        let (model, flags, mode) = self.workspace_chat_selection(&q.workspace_id, code);
        self.send_workspace_message(
            &q.workspace_id,
            code,
            prompt,
            &model,
            Some(&flags),
            &attachments,
            &mode,
            display,
        )
        .await;
    }
}
