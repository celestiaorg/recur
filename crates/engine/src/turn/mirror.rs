//! The session mirror: projects one turn's normalized events onto the session
//! store as a lossless transcript.
//!
//! Message boundaries follow Anthropic's conversation shape: assistant
//! thinking, text and tool_use blocks accrue into an assistant message;
//! tool_result blocks (which arrive on "user" stream lines) accrue into a
//! following user message. The mirror is used from the turn's event sink,
//! one event at a time.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use recur_agent::{AgentEvent, UserImage};
use recur_domain::{
    COMPACT_DONE, ContentBlock, Message, STREAM_COMPLETE, STREAM_PENDING, STREAM_STREAMING,
};
use recur_store::Store;

use super::transcript::user_message_is_chat_prompt;

pub struct SessionMirror {
    store: Arc<Store>,
    session: String,
    model: String,
    provider: String,
    cur_msg: String,
    cur_role: String,
    /// Provider tool-use id → the internal id minted for it, so a later
    /// tool_result references the internal id.
    tool_ids: HashMap<String, String>,
    /// The running redacted-thinking estimate per parent ("" = parent turn).
    think_tokens: HashMap<String, i64>,
    /// The provider frame that last raised the parent-turn estimate.
    think_token_uuid: String,
    last_fallback_from: String,
    last_fallback_to: String,
    last_fallback_msg: String,
    /// Provider frame uuid → Recur messages written under it (refusal
    /// fallback retraction).
    uuid_msgs: HashMap<String, Vec<String>>,
}

impl SessionMirror {
    pub fn new(store: Arc<Store>, session_id: &str, model: &str, provider: &str) -> Self {
        Self {
            store,
            session: session_id.to_owned(),
            model: model.to_owned(),
            provider: provider.to_owned(),
            cur_msg: String::new(),
            cur_role: String::new(),
            tool_ids: HashMap::new(),
            think_tokens: HashMap::new(),
            think_token_uuid: String::new(),
            last_fallback_from: String::new(),
            last_fallback_to: String::new(),
            last_fallback_msg: String::new(),
            uuid_msgs: HashMap::new(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session
    }

    /// Appends the human's prompt as a completed user row before the turn
    /// streams: `text` is the full wire, `display` the optional bubble text,
    /// images become blocks at their markers. Returns the row id ("" when
    /// there was nothing to record; the trimmed `id` when the write failed).
    pub fn record_user(&self, id: &str, text: &str, display: &str, imgs: &[UserImage]) -> String {
        let blocks = recur_agent::attach::user_content_blocks(text, display, imgs);
        if blocks.is_empty() {
            return String::new();
        }
        match self.store.append_message(
            &self.session,
            Message {
                id: id.to_owned(),
                role: "user".into(),
                content: Some(blocks),
                ..Message::default()
            },
        ) {
            Ok(m) => m.id,
            Err(_) => id.trim().to_owned(),
        }
    }

    /// Projects one event onto message/content rows.
    pub fn event(&mut self, ev: &AgentEvent) {
        match ev {
            AgentEvent::Init { session_id, .. } => {
                if !session_id.is_empty() {
                    let _ = self.store.update_session_ref(&self.session, session_id);
                }
            }
            AgentEvent::Text { text, uuid, parent } => {
                self.ensure("assistant");
                self.note_provider_uuid(uuid);
                self.append(ContentBlock::text(text).with_parent(parent));
            }
            AgentEvent::ThinkingTokens {
                tokens,
                uuid,
                parent,
            } => {
                let id = uuid.trim();
                if !id.is_empty() && parent.is_empty() && id != self.think_token_uuid {
                    self.think_tokens.insert(String::new(), 0);
                    self.think_token_uuid = id.to_owned();
                }
                let cur = self.think_tokens.entry(parent.clone()).or_insert(0);
                if *tokens > *cur {
                    *cur = *tokens;
                }
            }
            AgentEvent::Thinking {
                text,
                signature,
                uuid,
                parent,
            } => {
                let tok = self.think_tokens.remove(parent).unwrap_or(0);
                self.ensure("assistant");
                self.note_provider_uuid(uuid);
                self.append(ContentBlock::thinking(text, signature, tok).with_parent(parent));
                if parent.is_empty() {
                    self.think_token_uuid.clear();
                }
            }
            AgentEvent::Step(s) => {
                // A re-emitted tool_use with a known id would mint an orphan
                // row; blank ids cannot be correlated and are never deduped.
                if !s.id.is_empty() && self.tool_ids.contains_key(&s.id) {
                    return;
                }
                self.ensure("assistant");
                self.note_provider_uuid(&s.uuid);
                let internal = uuid::Uuid::now_v7().to_string();
                if !s.id.is_empty() {
                    self.tool_ids.insert(s.id.clone(), internal.clone());
                }
                // Stored verbatim: the provider's own name and arguments; the
                // common vocabulary is derived at read time.
                self.append(ContentBlock::tool_use_nested(
                    &internal, &s.id, &s.parent, &s.name, &s.input,
                ));
            }
            AgentEvent::ToolResult(r) => {
                self.ensure("user");
                self.note_provider_uuid(&r.uuid);
                let internal = self
                    .tool_ids
                    .get(&r.id)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| r.id.clone());
                // Codex MCP attaches full args only on completion: patch the
                // earlier tool_use so reloads show them.
                if !r.input.is_empty() && r.input != "{}" && !internal.is_empty() {
                    let _ = self.store.patch_tool_use_input(&internal, &r.input);
                }
                self.append(ContentBlock::truncated_tool_result(
                    &internal,
                    &r.text,
                    r.is_error,
                    r.truncated,
                ));
            }
            AgentEvent::Compact {
                status,
                trigger,
                summary,
                pre_tokens,
                uuid,
                parent,
            } => {
                // started is live-only; the done marker persists.
                if status != COMPACT_DONE {
                    return;
                }
                self.ensure("assistant");
                self.note_provider_uuid(uuid);
                self.append(
                    ContentBlock::compact(status, trigger, summary, *pre_tokens)
                        .with_parent(parent),
                );
                if !summary.trim().is_empty() && !self.cur_msg.is_empty() {
                    let _ = self
                        .store
                        .write_session_context(&self.session, summary, &self.cur_msg);
                }
            }
            AgentEvent::User {
                text,
                display,
                live_steer,
            } => {
                if text.trim().is_empty() {
                    return;
                }
                self.close_current(&Message::default());
                // A follow-up that started a new provider run may reuse call
                // ids; a live delivered steer keeps outstanding tools running.
                if !live_steer {
                    self.tool_ids.clear();
                }
                self.ensure("user");
                self.append(ContentBlock::text_display(text, display));
            }
            AgentEvent::Fallback {
                from_model,
                to_model,
                retracted_uuids,
                uuid,
            } => {
                if !retracted_uuids.is_empty() {
                    self.drop_provider_uuids(retracted_uuids);
                }
                let (from, to) = (from_model.trim(), to_model.trim());
                if to.is_empty() {
                    return;
                }
                if to == self.last_fallback_to {
                    if !from.is_empty()
                        && self.last_fallback_from.is_empty()
                        && !self.last_fallback_msg.is_empty()
                    {
                        let _ = self
                            .store
                            .patch_fallback_from(&self.last_fallback_msg, from);
                        self.last_fallback_from = from.to_owned();
                    }
                    return;
                }
                self.last_fallback_from = from.to_owned();
                self.last_fallback_to = to.to_owned();
                // The serving model is stamped before the row opens.
                self.model = to.to_owned();
                if !self.cur_msg.is_empty() && self.cur_role == "assistant" {
                    self.close_current(&Message::default());
                }
                self.ensure("assistant");
                self.note_provider_uuid(uuid);
                self.append(ContentBlock::fallback(from, to));
                self.last_fallback_msg = self.cur_msg.clone();
            }
            AgentEvent::TurnEnd(r) => {
                let usage = Message {
                    duration_ms: r.duration_ms,
                    input_tokens: r.usage.input_tokens,
                    output_tokens: r.usage.output_tokens,
                    cache_creation_tokens: r.usage.cache_creation_tokens,
                    cache_read_tokens: r.usage.cache_read_tokens,
                    context_window: r.usage.context_window,
                    ..Message::default()
                };
                self.close_current(&usage);
            }
            AgentEvent::TextDelta { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::Usage(_)
            | AgentEvent::Notice { .. }
            | AgentEvent::PlanProposal { .. } => {}
        }
    }

    /// Closes any open message and returns the session to idle (called after
    /// every turn so an error mid-stream leaves nothing streaming).
    pub fn finish(&mut self) {
        self.close_current(&Message::default());
        let _ = self.store.set_session_status(&self.session, "idle");
    }

    fn append(&self, b: ContentBlock) {
        let _ = self.store.append_content_block(&self.cur_msg, b);
    }

    fn ensure(&mut self, role: &str) {
        if !self.cur_msg.is_empty() && self.cur_role == role {
            return;
        }
        self.close_current(&Message::default());
        let model = if role == "assistant" {
            self.model.clone()
        } else {
            String::new()
        };
        match self.store.create_message(
            &self.session,
            Message {
                role: role.to_owned(),
                model_id: model,
                provider: self.provider.clone(),
                stream_status: STREAM_STREAMING.into(),
                ..Message::default()
            },
        ) {
            Ok(id) => {
                self.cur_msg = id;
                self.cur_role = role.to_owned();
                let _ = self.store.set_session_status(&self.session, "streaming");
            }
            Err(_) => {
                self.cur_msg.clear();
                self.cur_role.clear();
            }
        }
    }

    fn close_current(&mut self, usage: &Message) {
        if self.cur_msg.is_empty() {
            return;
        }
        let _ = self
            .store
            .complete_message(&self.cur_msg, STREAM_COMPLETE, usage);
        self.cur_msg.clear();
        self.cur_role.clear();
    }

    fn note_provider_uuid(&mut self, id: &str) {
        let id = id.trim();
        if id.is_empty() || self.cur_msg.is_empty() {
            return;
        }
        let ids = self.uuid_msgs.entry(id.to_owned()).or_default();
        if !ids.iter().any(|m| m == &self.cur_msg) {
            ids.push(self.cur_msg.clone());
        }
    }

    fn drop_provider_uuids(&mut self, uuids: &[String]) {
        if uuids.is_empty() {
            return;
        }
        if uuids.iter().any(|u| u.trim() == self.think_token_uuid) {
            self.think_tokens.remove("");
            self.think_token_uuid.clear();
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut ids: Vec<String> = Vec::new();
        for u in uuids {
            for mid in self.uuid_msgs.get(u.trim()).into_iter().flatten() {
                if mid.is_empty() || !seen.insert(mid.clone()) {
                    continue;
                }
                ids.push(mid.clone());
            }
        }
        if ids.is_empty() {
            return;
        }
        if !self.cur_msg.is_empty() && seen.contains(&self.cur_msg) {
            self.cur_msg.clear();
            self.cur_role.clear();
        }
        let _ = self.store.drop_session_messages(&self.session, &ids);
        self.uuid_msgs.retain(|_, mids| {
            mids.retain(|m| !seen.contains(m));
            !mids.is_empty()
        });
    }
}

/// `want` when it is a completed chat-prompt row of the session, else the
/// newest such row (Retry mints a fresh id for checkpointing and never
/// writes it back; harness history still needs the recorded row).
pub fn resolve_session_user_message_id(store: &Store, session_id: &str, want: &str) -> String {
    if session_id.is_empty() {
        return want.to_owned();
    }
    let Ok(msgs) = store.session_transcript(session_id) else {
        return want.to_owned();
    };
    let mut last = String::new();
    for m in msgs.unwrap_or_default() {
        if m.role != "user" || !user_message_is_chat_prompt(&m) {
            continue;
        }
        if m.stream_status == STREAM_PENDING || m.stream_status == STREAM_STREAMING {
            continue;
        }
        if !want.is_empty() && m.id == want {
            return want.to_owned();
        }
        last = m.id;
    }
    if !last.is_empty() {
        return last;
    }
    want.to_owned()
}

/// Whether the wire carries host wrap markers that must not show alone in
/// the bubble: callers set a display text whenever this holds.
pub fn requires_display_text(prompt: &str) -> bool {
    prompt.contains("<skill_content")
        || prompt.contains("The user has attached these files")
        || prompt.contains("The user activated skill(s) via /name")
        || prompt.contains(super::transcript::FORK_PREAMBLE_LEAD)
}

/// The bubble body for a turn's user row: the display text, else a known
/// Coder dispatch label, else the wire.
pub fn user_bubble_text(prompt: &str, display: &str) -> String {
    let d = display.trim();
    if !d.is_empty() {
        return d.to_owned();
    }
    let label = crate::roles::coder_dispatch_display(prompt);
    if !label.is_empty() {
        return label.to_owned();
    }
    prompt.trim().to_owned()
}
