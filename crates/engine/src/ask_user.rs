//! The park-and-return `ask_user` broker.
//!
//! Parking returns immediately: the asking chat turn is stopped quietly so
//! the tab goes idle, and the human's answer later starts a resumed turn with
//! the answer as user input (MCP clients time out a blocked POST). The resume
//! send itself is [`Engine::send_ask_user_resume`] in `chat::ask_resume`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent_tools::AskUserRequest;
use recur_domain::{
    ChatAttachment, EscalationOption, Group, InboxItem, UserQuestion, UserQuestionAnswer,
};

use crate::Engine;
use crate::bus::{EngineEvent, QuestionAnswered, QuestionAsked};
use crate::chatlife::ChatTurn;
use crate::text::{html_escape, truncate, unix_nanos};

/// The tool result a parked question returns at once.
pub const ASK_USER_PARK_RESULT: &str = "Question delivered to the user. Stop now and wait — their answer will arrive as a later message. Do not invent an answer or continue working.";

/// Lets the MCP response flush to the agent before the turn is parked.
pub(crate) const ASK_USER_PARK_DELAY: Duration = Duration::from_millis(150);

/// Between answering and starting the resumed turn.
const ASK_USER_RESUME_DELAY: Duration = Duration::from_millis(50);

type Notify = Arc<dyn Fn(&UserQuestion) + Send + Sync>;
type Resolved = Arc<dyn Fn(&str, &UserQuestionAnswer) + Send + Sync>;

struct Pending {
    req: UserQuestion,
    /// The asking turn: park and answer only cancel it while it is live.
    turn: Option<Arc<ChatTurn>>,
}

#[derive(Default)]
struct State {
    pending: HashMap<String, Pending>,
    /// Answered; the resumed turn is not registered yet.
    resuming: HashMap<String, UserQuestion>,
    seq: u64,
}

/// Parked questions and the answer-to-resume gap.
#[derive(Default)]
pub struct AskUserBroker {
    state: Mutex<State>,
    notify: Mutex<Option<Notify>>,
    resolved: Mutex<Option<Resolved>>,
}

impl AskUserBroker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_notify(&self, f: impl Fn(&UserQuestion) + Send + Sync + 'static) {
        *self.notify.lock() = Some(Arc::new(f));
    }

    pub fn set_resolved(&self, f: impl Fn(&str, &UserQuestionAnswer) + Send + Sync + 'static) {
        *self.resolved.lock() = Some(Arc::new(f));
    }

    /// Stores a question (assigning its id) and notifies; never blocks.
    pub fn park(&self, mut req: UserQuestion, turn: Option<Arc<ChatTurn>>) -> UserQuestion {
        {
            let mut st = self.state.lock();
            st.seq += 1;
            req.id = format!("ask-{}-{}", unix_nanos(), st.seq);
            if req.asked_ago.is_empty() {
                req.asked_ago = "now".into();
            }
            st.pending.insert(
                req.id.clone(),
                Pending {
                    req: req.clone(),
                    turn,
                },
            );
        }
        let notify = self.notify.lock().clone();
        if let Some(f) = notify {
            f(&req);
        }
        req
    }

    /// Takes a pending question into the resume gap. `None` when unknown.
    pub fn answer(
        &self,
        id: &str,
        ans: &UserQuestionAnswer,
    ) -> Option<(UserQuestion, Option<Arc<ChatTurn>>)> {
        let p = {
            let mut st = self.state.lock();
            let p = st.pending.remove(id)?;
            st.resuming.insert(p.req.id.clone(), p.req.clone());
            p
        };
        self.publish_resolved(id, ans);
        Some((p.req, p.turn))
    }

    /// Unanswered questions ordered by id.
    pub fn pending(&self) -> Vec<UserQuestion> {
        let mut out: Vec<UserQuestion> = self
            .state
            .lock()
            .pending
            .values()
            .map(|p| p.req.clone())
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Drops a workspace's pending questions and resume gaps; returns the
    /// dropped pending ids, sorted.
    pub fn drop_workspace(&self, workspace_id: &str) -> Vec<String> {
        let workspace_id = workspace_id.trim();
        if workspace_id.is_empty() {
            return Vec::new();
        }
        let mut st = self.state.lock();
        let mut ids: Vec<String> = st
            .pending
            .iter()
            .filter(|(_, p)| p.req.workspace_id == workspace_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            st.pending.remove(id);
        }
        st.resuming.retain(|_, q| q.workspace_id != workspace_id);
        ids.sort();
        ids
    }

    pub fn drop_resume(&self, id: &str) {
        self.state.lock().resuming.remove(id);
    }

    /// A parked question or an answer-resume gap still belongs to this
    /// workspace (hoist and child archive must wait).
    pub fn workspace_busy(&self, workspace_id: &str) -> bool {
        if workspace_id.is_empty() {
            return false;
        }
        let st = self.state.lock();
        st.pending
            .values()
            .any(|p| p.req.workspace_id == workspace_id)
            || st.resuming.values().any(|q| q.workspace_id == workspace_id)
    }

    /// A parked or resuming question on this tab.
    pub fn pending_for(&self, workspace_id: &str, code: &str) -> bool {
        let st = self.state.lock();
        st.pending
            .values()
            .any(|p| p.req.workspace_id == workspace_id && p.req.agent_code == code)
            || st
                .resuming
                .values()
                .any(|q| q.workspace_id == workspace_id && q.agent_code == code)
    }

    fn publish_resolved(&self, id: &str, ans: &UserQuestionAnswer) {
        let f = self.resolved.lock().clone();
        if let Some(f) = f {
            f(id, ans);
        }
    }
}

/// The resumed turn's wire prompt: the full question and answer for
/// provider replay.
pub fn format_ask_user_resume_prompt(q: &UserQuestion, ans: &UserQuestionAnswer) -> String {
    let mut b = String::from("The user answered your earlier ask_user question.\n\n");
    b.push_str("Question: ");
    b.push_str(q.question.trim());
    b.push('\n');
    let ctx = q.context.trim();
    if !ctx.is_empty() {
        b.push_str("Context: ");
        b.push_str(ctx);
        b.push('\n');
    }
    let choice = ans.choice.trim();
    let free = ans.free_text.trim();
    match (choice.is_empty(), free.is_empty()) {
        (false, false) => {
            b.push_str("Answer: ");
            b.push_str(choice);
            b.push_str("\nDetails: ");
            b.push_str(free);
        }
        (false, true) => {
            b.push_str("Answer: ");
            b.push_str(choice);
        }
        (true, false) => {
            b.push_str("Answer: ");
            b.push_str(free);
        }
        (true, true) => b.push_str("Answer: (empty)"),
    }
    b.push_str("\n\nContinue with this answer. Do not re-ask the same question unless you still need clarification.");
    b
}

/// The short chat-bubble label for an answer.
pub fn format_ask_user_display(ans: &UserQuestionAnswer) -> String {
    let choice = ans.choice.trim();
    let free = ans.free_text.trim();
    match (choice.is_empty(), free.is_empty()) {
        (false, false) => format!("{choice} — {free}"),
        (false, true) => choice.to_owned(),
        (true, false) => free.to_owned(),
        (true, true) => "(empty)".to_owned(),
    }
}

impl Engine {
    /// Host `ask_user`: parks the question and returns at once; the asking
    /// turn is then park-stopped (only that turn) after the tool result has
    /// flushed.
    pub fn ask_user(&self, workspace_id: &str, agent_code: &str, req: AskUserRequest) -> String {
        let mut q = UserQuestion {
            workspace_id: workspace_id.to_owned(),
            agent_code: agent_code.to_owned(),
            question: req.question,
            context: req.context,
            ..UserQuestion::default()
        };
        if let Some(a) = self.store.agent(workspace_id, agent_code) {
            q.session_id = a.session_id;
        }
        q.options = req
            .options
            .into_iter()
            .map(|o| EscalationOption {
                text: o.text,
                tag: o.tag,
                desc: o.desc,
            })
            .collect();
        let turn = self.live_chat_turn(workspace_id, agent_code);
        self.ask_users.park(q, turn.clone());
        let eng = self.arc();
        let (ws, code) = (workspace_id.to_owned(), agent_code.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(ASK_USER_PARK_DELAY).await;
            eng.park_chat_turn_if(&ws, &code, turn.as_ref());
        });
        ASK_USER_PARK_RESULT.to_owned()
    }

    /// Delivers the human's answer and starts the resumed turn (quiet-stops
    /// the asking turn only, never a newer send on the tab). False when the
    /// question is gone.
    pub fn answer_ask_user(
        &self,
        id: &str,
        choice: &str,
        free_text: &str,
        attachments: Vec<ChatAttachment>,
    ) -> bool {
        let ans = UserQuestionAnswer {
            choice: choice.to_owned(),
            free_text: free_text.to_owned(),
        };
        let Some((q, turn)) = self.ask_users.answer(id, &ans) else {
            return false;
        };
        self.park_chat_turn_if(&q.workspace_id, &q.agent_code, turn.as_ref());
        let eng = self.arc();
        tokio::spawn(async move {
            tokio::time::sleep(ASK_USER_RESUME_DELAY).await;
            eng.resume_after_ask_user(q, ans, attachments).await;
        });
        true
    }

    /// Runs the resumed turn, then closes the answer-resume gap.
    pub async fn resume_after_ask_user(
        &self,
        q: UserQuestion,
        ans: UserQuestionAnswer,
        attachments: Vec<ChatAttachment>,
    ) {
        let prompt = format_ask_user_resume_prompt(&q, &ans);
        let display = format_ask_user_display(&ans);
        self.send_ask_user_resume(&q, &prompt, &display, attachments)
            .await;
        self.ask_users.drop_resume(&q.id);
        self.maybe_archive_linked_pr_chat_host(&q.workspace_id);
    }

    /// Unanswered ask_user prompts.
    pub fn pending_ask_users(&self) -> Vec<UserQuestion> {
        self.ask_users.pending()
    }

    /// The broker (tests park questions directly).
    pub fn ask_user_broker(&self) -> &AskUserBroker {
        &self.ask_users
    }

    /// A parked question or answer-resume gap still holds the workspace.
    pub fn ask_user_workspace_busy(&self, workspace_id: &str) -> bool {
        self.ask_users.workspace_busy(workspace_id)
    }

    /// A parked or resuming question on this tab.
    pub(crate) fn ask_user_pending_for(&self, workspace_id: &str, code: &str) -> bool {
        self.ask_users.pending_for(workspace_id, code)
    }

    /// Drops a workspace's parked questions and their inbox rows
    /// (delete/archive).
    pub(crate) fn clear_ask_users_for_workspace(&self, workspace_id: &str) {
        let ids = self.ask_users.drop_workspace(workspace_id);
        if ids.is_empty() {
            return;
        }
        for id in ids {
            self.store.remove_inbox_item(&format!("ib-ask-{id}"));
            self.bus
                .publish(EngineEvent::QuestionAnswered(QuestionAnswered {
                    answer: UserQuestionAnswer::default(),
                    id,
                }));
        }
        self.publish_inbox_updated();
    }

    pub(crate) fn publish_ask_user_requested(&self, q: &UserQuestion) {
        let short = self
            .store
            .workspace(&q.workspace_id)
            .map(|w| w.short)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| q.workspace_id.clone());
        self.store.add_inbox_item(InboxItem {
            id: format!("ib-ask-{}", q.id),
            group: Group::Needs.as_str().to_owned(),
            unread: true,
            kind: "pause".into(),
            color: "var(--amber)".into(),
            title: format!("<b>{}</b> asked a question", html_escape(&q.agent_code)),
            sub: format!(
                "{} · {}",
                html_escape(&truncate(&short, 64)),
                html_escape(&truncate(&q.question, 64))
            ),
            time: "now".into(),
            action: "Answer".into(),
            action_class: "urgent".into(),
            target: format!("question:{}", q.id),
            workspace_id: q.workspace_id.clone(),
        });
        self.publish_inbox_updated();
        self.bus.publish(EngineEvent::QuestionAsked(QuestionAsked {
            question: q.clone(),
            workspace_id: q.workspace_id.clone(),
        }));
        self.publish_notify(
            "An agent has a question",
            &truncate(&q.question, 120),
            &format!("question:{}", q.id),
        );
    }

    pub(crate) fn publish_ask_user_resolved(&self, id: &str, ans: &UserQuestionAnswer) {
        self.store.remove_inbox_item(&format!("ib-ask-{id}"));
        self.publish_inbox_updated();
        self.bus
            .publish(EngineEvent::QuestionAnswered(QuestionAnswered {
                answer: ans.clone(),
                id: id.to_owned(),
            }));
    }
}

#[cfg(test)]
mod tests;
