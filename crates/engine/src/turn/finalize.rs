//! The user-visible outcome of a chat turn: Stop, interrupt and timeout notes,
//! provider-aware error replies, and the durable failure row that keeps a
//! failed turn visible after a resync.

use std::error::Error;
use std::time::Instant;

use recur_agent::{ProviderError, TurnResult};
use recur_domain::{APP_AGENT_CODE, ContentBlock, Message, STREAM_ERROR, Session};

use super::transcript::{
    flatten_user_message, is_mode_switch_continue_display, user_message_is_chat_prompt,
};
use crate::Engine;
use crate::codes::is_chat_retry_display;
use crate::text::truncate;

/// The note when the user interrupts a turn.
pub const CHAT_STOPPED_REPLY: &str = "Stopped by user";

/// Persisted when Recover heals a turn the process did not finish; distinct
/// from Stop so AutoResume continues the work.
pub const CHAT_INTERRUPTED_REPLY: &str = "Interrupted: Recur closed before this turn finished.";

/// The AutoResume continuation turn: the wire tells the resumed session to
/// pick up, the display is the bubble.
pub const CHAT_RESUME_PROMPT: &str =
    "You were interrupted because Recur restarted. Continue from where you left off.";
pub const CHAT_RESUME_DISPLAY: &str = "Recur restarted: continuing";

/// The stable prefix of synthesized failure replies (the UI appends the
/// note even after streamed text when it sees this).
pub const CHAT_AGENT_ERROR_PREFIX: &str = "Agent error: ";

/// The reply when a follow-up was steered into a live turn instead of
/// starting one (word joiner + "inject"; `crate::view::chat::CHAT_INJECTED_REPLY`).
pub const CHAT_INJECTED_SENTINEL: &str = "\u{2060}inject";

/// The note when a turn's wedged-turn deadline fires.
pub const CHAT_TURN_TIMED_OUT_REPLY: &str = "Agent error: this turn hit Recur's time limit — send again to continue (use Stop if it looked stuck).";

/// The deadline error text; an error ending in it is a timeout.
pub const DEADLINE_EXCEEDED: &str = "context deadline exceeded";

/// Whether `err` (or anything it wraps) is a turn cancellation.
pub fn is_canceled(err: Option<&(dyn Error + 'static)>) -> bool {
    let mut cur = err;
    while let Some(e) = cur {
        if e.downcast_ref::<ProviderError>()
            .is_some_and(ProviderError::is_canceled)
        {
            return true;
        }
        cur = e.source();
    }
    false
}

/// Whether `err` (or anything it wraps) is a deadline expiry.
pub fn is_deadline_exceeded(err: Option<&(dyn Error + 'static)>) -> bool {
    let mut cur = err;
    while let Some(e) = cur {
        if e.to_string().ends_with(DEADLINE_EXCEEDED) {
            return true;
        }
        cur = e.source();
    }
    false
}

/// The chat reply for a failed turn, in the provider's own words where it
/// has them.
pub fn format_chat_agent_error(err: Option<&(dyn Error + 'static)>, res: &TurnResult) -> String {
    if is_deadline_exceeded(err) {
        return CHAT_TURN_TIMED_OUT_REPLY.to_owned();
    }
    let detail = recur_agent::error::failure_reason(err, res);
    let detail = detail.trim();
    if detail.is_empty() || detail == "the agent turn failed" {
        return "I hit an error trying to respond just now. Try again.".to_owned();
    }
    format!("{CHAT_AGENT_ERROR_PREFIX}{}", truncate(detail, 400))
}

/// The wall time to stamp on a synthesized Stop or error note: the
/// provider's duration, else time since `started`. 0 when this run already
/// published `session.turn_end` (one footer per turn).
pub fn chat_failure_duration(res: &TurnResult, started: Option<Instant>) -> i64 {
    if res.turn_end_emitted {
        return 0;
    }
    let mut dur = res.duration_ms;
    if dur <= 0
        && let Some(s) = started
    {
        dur = i64::try_from(s.elapsed().as_millis()).unwrap_or(i64::MAX);
    }
    dur
}

/// Everything [`Engine::finalize_chat_reply`] weighs. `prompt` / `display`
/// match the turn's wire and UI label so a pre-mirror failure stores the
/// same shape.
pub struct ChatOutcome<'a> {
    pub workspace_id: &'a str,
    pub code: &'a str,
    pub prompt: &'a str,
    pub display: &'a str,
    /// The assistant text streamed this turn.
    pub streamed: &'a str,
    pub res: &'a TurnResult,
    pub err: Option<&'a (dyn Error + 'static)>,
    /// The reply when the turn succeeded with no text.
    pub empty_fallback: &'a str,
    pub started: Option<Instant>,
    /// Retry / remount: the user row stays off the transcript.
    pub skip_user: bool,
    /// The preassigned id of the turn's user row (its checkpoint key).
    pub user_msg_id: &'a str,
}

impl Engine {
    /// The reply shown after a chat turn. A failure always synthesizes and
    /// persists a provider-aware note (even after partial text); a user
    /// interrupt is a calm "Stopped by user" with how long it ran; a parked
    /// ask_user turn is quiet; a wedged deadline is a timeout; a turn
    /// cancelled by shutdown keeps its streamed text for Recover.
    pub fn finalize_chat_reply(&self, o: ChatOutcome<'_>) -> String {
        let streamed = o.streamed.trim();
        if is_canceled(o.err) {
            if self.chat_turn_was_parked(o.workspace_id, o.code) {
                return streamed.to_owned();
            }
            if self.chat_turn_was_timed_out(o.workspace_id, o.code) {
                self.persist_chat_failure(
                    o.workspace_id,
                    o.code,
                    o.prompt,
                    o.display,
                    CHAT_TURN_TIMED_OUT_REPLY,
                    0,
                    &TurnResult::default(),
                    o.skip_user,
                    o.user_msg_id,
                );
                return CHAT_TURN_TIMED_OUT_REPLY.to_owned();
            }
            if self.is_shutting_down() {
                return streamed.to_owned();
            }
            self.persist_chat_failure(
                o.workspace_id,
                o.code,
                o.prompt,
                o.display,
                CHAT_STOPPED_REPLY,
                chat_failure_duration(o.res, o.started),
                o.res,
                o.skip_user,
                o.user_msg_id,
            );
            return CHAT_STOPPED_REPLY.to_owned();
        }
        if o.err.is_some() || o.res.is_error {
            let note = format_chat_agent_error(o.err, o.res);
            self.persist_chat_failure(
                o.workspace_id,
                o.code,
                o.prompt,
                o.display,
                &note,
                chat_failure_duration(o.res, o.started),
                o.res,
                o.skip_user,
                o.user_msg_id,
            );
            if !o.workspace_id.is_empty() {
                self.touch_workspace_activity(o.workspace_id, "Chat turn failed", self.now());
            }
            return note;
        }
        let mut out = streamed.to_owned();
        if out.is_empty() {
            out = o.res.text.trim().to_owned();
        }
        if out.is_empty() {
            out = o.empty_fallback.to_owned();
        }
        if !o.workspace_id.is_empty() {
            self.touch_workspace_activity(o.workspace_id, "Chat turn finished", self.now());
        }
        out
    }

    /// Records a failed chat turn durably. When the failure came before the
    /// mirror attached, the session and the user row may not exist yet:
    /// both are ensured (except for Retry, remounts and skip-user resumes).
    /// `duration_ms` > 0 stamps the footer (with the result's token fields)
    /// on the note.
    #[allow(clippy::too_many_arguments)]
    pub fn persist_chat_failure(
        &self,
        workspace_id: &str,
        code: &str,
        prompt: &str,
        display: &str,
        err_note: &str,
        duration_ms: i64,
        usage: &TurnResult,
        skip_user: bool,
        user_msg_id: &str,
    ) {
        let err_note = err_note.trim();
        if err_note.is_empty() {
            return;
        }
        if workspace_id.is_empty() && code != APP_AGENT_CODE {
            return;
        }
        if !workspace_id.is_empty() && self.workspace_teardown_committed(workspace_id) {
            return;
        }
        let Some(sess) = self.chat_session_for_persist(workspace_id, code) else {
            return;
        };
        let prompt = prompt.trim();
        let mut lookup = display.trim().to_owned();
        if !skip_user
            && !is_chat_retry_display(&lookup)
            && !is_mode_switch_continue_display(&lookup)
        {
            if lookup.is_empty() {
                lookup = prompt.to_owned();
            }
            if !prompt.is_empty()
                && !self.session_has_persisted_user(&sess.id, user_msg_id, &lookup)
            {
                let blocks = recur_agent::attach::user_content_blocks(prompt, display, &[]);
                if !blocks.is_empty() {
                    let _ = self.store.append_message(
                        &sess.id,
                        Message {
                            id: user_msg_id.to_owned(),
                            role: "user".into(),
                            content: Some(blocks),
                            ..Message::default()
                        },
                    );
                }
            }
        }
        let Ok(msg) = self.store.append_message(
            &sess.id,
            Message {
                role: "assistant".into(),
                stream_status: STREAM_ERROR.into(),
                content: Some(vec![ContentBlock::text(err_note)]),
                ..Message::default()
            },
        ) else {
            return;
        };
        if msg.id.is_empty() || duration_ms <= 0 {
            return;
        }
        let _ = self.store.complete_message(
            &msg.id,
            STREAM_ERROR,
            &Message {
                duration_ms,
                input_tokens: usage.usage.input_tokens,
                output_tokens: usage.usage.output_tokens,
                cache_creation_tokens: usage.usage.cache_creation_tokens,
                cache_read_tokens: usage.usage.cache_read_tokens,
                context_window: usage.usage.context_window,
                ..Message::default()
            },
        );
    }

    /// The tab's session, created when a pre-mirror failure left none.
    fn chat_session_for_persist(&self, workspace_id: &str, code: &str) -> Option<Session> {
        if workspace_id.is_empty() {
            return self
                .store
                .session_for_app_kind(recur_domain::SESSION_KIND_APP);
        }
        if let Some(s) = self.store.session_for_loop_agent(workspace_id, code) {
            return Some(s);
        }
        let (kind, name, provider) = self.chat_session_meta(workspace_id, code);
        self.ensure_agent_session(workspace_id, &kind, code, &name, &provider)
    }

    /// Whether the failure note should skip synthesizing a user row. With a
    /// preassigned id only that exact row counts (an older prompt with the
    /// same text must not hide a new turn); without one, the newest chat
    /// prompt's text is compared.
    fn session_has_persisted_user(&self, session_id: &str, user_msg_id: &str, text: &str) -> bool {
        let msgs = self
            .store
            .session_transcript(session_id)
            .ok()
            .flatten()
            .unwrap_or_default();
        if !user_msg_id.is_empty() {
            return msgs.iter().any(|m| m.id == user_msg_id);
        }
        let want = text.trim();
        msgs.iter()
            .rev()
            .filter(|m| m.role == "user" && user_message_is_chat_prompt(m))
            .find_map(flatten_user_message)
            .is_some_and(|it| it.text.trim() == want)
    }
}

#[cfg(test)]
#[path = "finalize_tests.rs"]
mod tests;
