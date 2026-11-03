//! The live session stream: one turn's events onto the bus as `session.*`,
//! tagged with the workspace and agent code so any surface renders any agent's
//! session live.
//!
//! Text and thinking deltas are published as cumulative snapshots with a
//! monotonic `seq` that continues across turns (seeded from the engine), so
//! the UI's last-seen gate never drops a later turn. A final text/thinking
//! block after deltas publishes `replace` + `done`. Nested subagent events
//! keep their own buffers and counters and carry `parentId`.

use std::collections::HashMap;
use std::sync::Arc;

use recur_agent::display::{
    chat_tool_input, chat_tool_result, display_tool_result, is_list_findings_tool,
    summarize_display_input, tool_description,
};
use recur_agent::{AgentEvent, Provider, TokenUsage};

use crate::Engine;
use crate::bus::{ContextUsage, EngineEvent, SessionRoute};

#[derive(Default)]
struct NestedBuf {
    got_delta: bool,
    got_thinking: bool,
    text_buf: String,
    think_buf: String,
    text_seq: i64,
    think_seq: i64,
}

pub struct SessionBroadcast {
    eng: Arc<Engine>,
    route: SessionRoute,
    provider: Arc<dyn Provider>,
    got_delta: bool,
    got_thinking: bool,
    text_buf: String,
    think_buf: String,
    text_seq: i64,
    think_seq: i64,
    /// Tool-use id → display name from the step (Claude results carry no
    /// tool name; list_findings parsing needs the real one).
    step_name: HashMap<String, String>,
    nested: HashMap<String, NestedBuf>,
}

/// The context fill a usage-bearing result reports, when it has any.
pub fn context_usage(u: &TokenUsage) -> Option<ContextUsage> {
    (u.context_used() > 0).then_some(ContextUsage {
        input_tokens: u.input_tokens,
        cache_creation_tokens: u.cache_creation_tokens,
        cache_read_tokens: u.cache_read_tokens,
        context_window: u.context_window,
    })
}

impl SessionBroadcast {
    pub fn new(eng: Arc<Engine>, route: SessionRoute, provider: Arc<dyn Provider>) -> Self {
        let (text_seq, think_seq) = eng.stream_seq_seed(&route.workspace_id, &route.code);
        Self {
            eng,
            route,
            provider,
            got_delta: false,
            got_thinking: false,
            text_buf: String::new(),
            think_buf: String::new(),
            text_seq,
            think_seq,
            step_name: HashMap::new(),
            nested: HashMap::new(),
        }
    }

    pub fn route(&self) -> &SessionRoute {
        &self.route
    }

    fn publish(&self, ev: EngineEvent) {
        self.eng.bus.publish(ev);
    }

    fn next_text_seq(&mut self) -> i64 {
        self.text_seq += 1;
        self.eng.remember_stream_seq(
            &self.route.workspace_id,
            &self.route.code,
            "text",
            self.text_seq,
        );
        self.text_seq
    }

    fn next_think_seq(&mut self) -> i64 {
        self.think_seq += 1;
        self.eng.remember_stream_seq(
            &self.route.workspace_id,
            &self.route.code,
            "thinking",
            self.think_seq,
        );
        self.think_seq
    }

    fn reset_blocks(&mut self) {
        self.text_buf.clear();
        self.think_buf.clear();
        self.got_delta = false;
        self.got_thinking = false;
    }

    /// Publishes the turn's opening user bubble.
    pub fn user(&self, text: &str) {
        self.publish(self.route.user(text));
    }

    pub fn emit(&mut self, ev: &AgentEvent) {
        if ev.nested() {
            self.emit_nested(ev);
            return;
        }
        match ev {
            AgentEvent::TextDelta { text, .. } => {
                // Stream line uuids are per event, not per block: never reset
                // the cumulative buffer on a uuid change.
                self.got_delta = true;
                self.text_buf.push_str(text);
                let mut d = self.route.delta();
                d.text = self.text_buf.clone();
                d.seq = Some(self.next_text_seq());
                d.replace = true;
                self.publish(EngineEvent::SessionDelta(d));
            }
            AgentEvent::Text { text, uuid, .. } => {
                if text.trim().is_empty() {
                    return;
                }
                let mut d = self.route.delta();
                d.text = text.clone();
                d.done = true;
                d.uuid = uuid.clone();
                if self.got_delta {
                    d.seq = Some(self.next_text_seq());
                    d.replace = true;
                    self.text_buf.clear();
                    self.got_delta = false;
                }
                self.publish(EngineEvent::SessionDelta(d));
            }
            AgentEvent::ThinkingDelta { text, .. } => {
                self.got_thinking = true;
                self.think_buf.push_str(text);
                let mut d = self.route.thinking();
                d.text = Some(self.think_buf.clone());
                d.seq = Some(self.next_think_seq());
                d.replace = true;
                self.publish(EngineEvent::SessionThinking(d));
            }
            AgentEvent::ThinkingTokens { tokens, .. } => {
                let mut d = self.route.thinking();
                d.tokens = Some(*tokens);
                self.publish(EngineEvent::SessionThinking(d));
            }
            AgentEvent::Thinking { text, uuid, .. } => {
                if text.trim().is_empty() {
                    return;
                }
                let mut d = self.route.thinking();
                d.text = Some(text.clone());
                d.done = true;
                d.uuid = uuid.clone();
                if self.got_thinking {
                    d.seq = Some(self.next_think_seq());
                    d.replace = true;
                    self.think_buf.clear();
                    self.got_thinking = false;
                }
                self.publish(EngineEvent::SessionThinking(d));
            }
            AgentEvent::Step(s) => {
                // A tool call ends the current text/thinking block.
                self.reset_blocks();
                self.publish_step(s, &s.parent, &s.uuid);
            }
            AgentEvent::ToolResult(r) => self.publish_tool_result(r),
            AgentEvent::Compact {
                status,
                trigger,
                summary,
                pre_tokens,
                ..
            } => {
                let mut c = self.route.compact();
                c.status = status.clone();
                c.trigger = trigger.clone();
                if !summary.is_empty() {
                    c.summary = Some(summary.clone());
                }
                if *pre_tokens > 0 {
                    c.pre_tokens = Some(*pre_tokens);
                }
                self.publish(EngineEvent::SessionCompact(c));
            }
            AgentEvent::User { text, display, .. } => {
                let mut show = display.trim();
                if show.is_empty() {
                    show = text;
                }
                if show.trim().is_empty() {
                    return;
                }
                let show = show.to_owned();
                self.reset_blocks();
                self.publish(self.route.user(show));
            }
            AgentEvent::Fallback {
                from_model,
                to_model,
                retracted_uuids,
                ..
            } => {
                if to_model.is_empty() && retracted_uuids.is_empty() {
                    return;
                }
                self.reset_blocks();
                let mut f = self.route.fallback();
                f.from = from_model.clone();
                f.to = to_model.clone();
                f.from_label = model_label(self.provider.as_ref(), from_model);
                f.to_label = model_label(self.provider.as_ref(), to_model);
                f.retracted = retracted_uuids.clone();
                self.publish(EngineEvent::SessionFallback(f));
            }
            AgentEvent::Usage(u) => {
                // Mid-turn context fill; live only.
                if let Some(c) = context_usage(u) {
                    self.publish(self.route.context_usage(c));
                }
            }
            AgentEvent::TurnEnd(r) => {
                // The provider's own wall time, so the live footer matches
                // the one after a reload.
                if r.duration_ms > 0 {
                    self.publish(self.route.turn_end(r.duration_ms, context_usage(&r.usage)));
                } else if let Some(c) = context_usage(&r.usage) {
                    self.publish(self.route.context_usage(c));
                }
            }
            AgentEvent::Notice { text } => {
                if text.trim().is_empty() {
                    return;
                }
                self.publish(self.route.notice(text.clone()));
            }
            AgentEvent::Init { .. } | AgentEvent::PlanProposal { .. } => {}
        }
    }

    fn publish_step(&mut self, s: &recur_agent::ToolStep, parent: &str, uuid: &str) {
        let d = self.provider.display_tool(&s.name, &s.input);
        if !s.id.is_empty() {
            self.step_name.insert(s.id.clone(), d.name.clone());
        }
        let mut st = self.route.step();
        st.id = s.id.clone();
        st.tool = d.name.clone();
        st.tool_input = s.summary.clone();
        st.description = tool_description(&d.name, &d.input);
        st.input = chat_tool_input(&d.name, &d.input);
        st.parent_id = parent.to_owned();
        st.uuid = uuid.to_owned();
        self.publish(EngineEvent::SessionStep(st));
    }

    fn publish_tool_result(&self, r: &recur_agent::ToolResultEvent) {
        let d = self.provider.display_tool(&r.name, &r.input);
        let mut name = d.name.clone();
        if !is_list_findings_tool(&name)
            && let Some(n) = self.step_name.get(&r.id).filter(|n| !n.is_empty())
        {
            name = n.clone();
        }
        let mut p = self.route.tool_result();
        p.id = r.id.clone();
        p.result = chat_tool_result(
            &name,
            &display_tool_result(self.provider.as_ref(), &r.name, &r.text),
        );
        p.is_error = r.is_error;
        // Codex MCP attaches full args only on completion.
        if !r.input.is_empty() {
            let input = chat_tool_input(&d.name, &d.input);
            if !input.is_empty() && input != "{}" {
                p.input = Some(input);
                p.tool_input = Some(summarize_display_input(
                    self.provider.as_ref(),
                    &r.name,
                    &r.input,
                ));
            }
        }
        self.publish(EngineEvent::SessionToolResult(p));
    }

    /// Nested subagent output, tagged with the parent Task id. A nested
    /// result never closes the parent turn.
    fn emit_nested(&mut self, ev: &AgentEvent) {
        let parent = ev.parent().to_owned();
        match ev {
            AgentEvent::TextDelta { text, .. } => {
                let n = self.nested.entry(parent.clone()).or_default();
                n.got_delta = true;
                n.text_buf.push_str(text);
                n.text_seq += 1;
                let mut d = self.route.delta();
                d.text = n.text_buf.clone();
                d.seq = Some(n.text_seq);
                d.replace = true;
                d.parent_id = Some(parent);
                self.publish(EngineEvent::SessionDelta(d));
            }
            AgentEvent::Text { text, .. } => {
                if text.trim().is_empty() {
                    return;
                }
                let n = self.nested.entry(parent.clone()).or_default();
                let mut d = self.route.delta();
                d.text = text.clone();
                d.done = true;
                d.parent_id = Some(parent);
                if n.got_delta {
                    n.text_seq += 1;
                    d.seq = Some(n.text_seq);
                    d.replace = true;
                    n.text_buf.clear();
                    n.got_delta = false;
                }
                self.publish(EngineEvent::SessionDelta(d));
            }
            AgentEvent::ThinkingDelta { text, .. } => {
                let n = self.nested.entry(parent.clone()).or_default();
                n.got_thinking = true;
                n.think_buf.push_str(text);
                n.think_seq += 1;
                let mut d = self.route.thinking();
                d.text = Some(n.think_buf.clone());
                d.seq = Some(n.think_seq);
                d.replace = true;
                d.parent_id = Some(parent);
                self.publish(EngineEvent::SessionThinking(d));
            }
            AgentEvent::ThinkingTokens { tokens, .. } => {
                let mut d = self.route.thinking();
                d.tokens = Some(*tokens);
                d.parent_id = Some(parent);
                self.publish(EngineEvent::SessionThinking(d));
            }
            AgentEvent::Thinking { text, .. } => {
                if text.trim().is_empty() {
                    return;
                }
                let n = self.nested.entry(parent.clone()).or_default();
                let mut d = self.route.thinking();
                d.text = Some(text.clone());
                d.done = true;
                d.parent_id = Some(parent);
                if n.got_thinking {
                    n.think_seq += 1;
                    d.seq = Some(n.think_seq);
                    d.replace = true;
                    n.think_buf.clear();
                    n.got_thinking = false;
                }
                self.publish(EngineEvent::SessionThinking(d));
            }
            AgentEvent::Step(s) => {
                let n = self.nested.entry(parent.clone()).or_default();
                n.text_buf.clear();
                n.think_buf.clear();
                n.got_delta = false;
                n.got_thinking = false;
                self.publish_step(s, &parent, "");
            }
            AgentEvent::ToolResult(r) => self.publish_tool_result(r),
            _ => {}
        }
    }
}

/// A fallback notice label: the provider's label for a model, empty for no
/// model.
fn model_label(p: &dyn Provider, model: &str) -> String {
    if model.trim().is_empty() {
        return String::new();
    }
    p.model_label(model)
}
