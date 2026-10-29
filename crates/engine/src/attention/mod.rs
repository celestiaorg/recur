//! Unseen-chat attention (Go `engine.go` focus/attention publishers and
//! `flagChatUnseen`, D-attention): a chat tab that finishes a turn while the
//! human is not really looking at it (selected tab + OS window focused + no
//! modal covering the chat) gets an Unseen badge and an inbox row.
//!
//! Lock order: `watch` → `chat_focus`; `chat_unseen_mu` → `chat_focus`.
//! `set_app_focused` must not call `watched_workspace` (it would invert the
//! first pair).

pub mod chat_inbox;

use recur_domain::AttentionCounts;

use crate::Engine;
use crate::bus::{AgentUpdated, AppFocused, AttentionUpdated, EngineEvent};
use crate::codes::{PAIR_AGENT_CODE, is_app_chat_code, is_chat_attention_code};

/// The chat tab the human is viewing and whether the surface is visible.
#[derive(Debug, Clone)]
pub struct ChatFocus {
    pub workspace_id: String,
    pub code: String,
    /// The OS key window (default true until the window loses focus).
    pub app_focused: bool,
    /// A modal or palette covers the chat surface.
    pub ui_occludes_chat: bool,
}

impl ChatFocus {
    pub fn new() -> Self {
        Self {
            workspace_id: String::new(),
            code: String::new(),
            app_focused: true,
            ui_occludes_chat: false,
        }
    }

    pub(crate) fn attentive(&self) -> bool {
        self.app_focused && !self.ui_occludes_chat
    }
}

impl Default for ChatFocus {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Sets an agent's status and publishes `agent.updated`. A chat tab
    /// going idle from working/directing raises Unseen (and its inbox row)
    /// unless the tab is really visible or a mode remount continues it.
    pub fn update_agent_status(&self, workspace_id: &str, code: &str, status: &str) {
        let chat_idle = status == "idle" && is_chat_attention_code(code);
        let _unseen = chat_idle.then(|| self.chat_unseen_mu.lock());
        let mut became_unseen = false;
        let (pending, focused) = if chat_idle {
            (
                self.pending_mode_continue_for(workspace_id, code),
                self.is_chat_focused(workspace_id, code),
            )
        } else {
            (false, false)
        };
        self.store.update_agent(workspace_id, code, |a| {
            let prev = std::mem::replace(&mut a.status, status.to_owned());
            if chat_idle && (prev == "working" || prev == "directing") {
                if pending || focused {
                    a.unseen = false;
                } else {
                    a.unseen = true;
                    became_unseen = true;
                }
            }
        });
        self.bus.publish(EngineEvent::AgentUpdated(AgentUpdated {
            agent_id: format!("{workspace_id}-{code}"),
            code: None,
            status: status.to_owned(),
            surface: None,
            workspace_id: workspace_id.to_owned(),
        }));
        if became_unseen {
            if self.emit_chat_inbox(workspace_id, code, false) {
                self.publish_inbox_updated();
            }
            self.publish_attention(workspace_id, code, true);
        } else if chat_idle && self.is_chat_focused(workspace_id, code) {
            self.clear_chat_inbox(workspace_id, code, true);
        }
    }

    /// The tab is selected and the human can actually see it.
    pub fn is_chat_focused(&self, workspace_id: &str, code: &str) -> bool {
        let f = self.chat_focus.lock();
        f.attentive() && f.workspace_id == workspace_id && f.code == code
    }

    /// Records OS window focus. Regaining it with a visible selected tab
    /// marks that tab seen, and asks listeners to reindex the watched
    /// workspace (`app.focused`).
    pub fn set_app_focused(&self, focused: bool) {
        let (mark_app, mark_seen, ws, code, was) = {
            let mut f = self.chat_focus.lock();
            let was = f.app_focused;
            f.app_focused = focused;
            let regained = focused && !was && f.attentive();
            (
                regained && is_app_chat_code(&f.code) && f.workspace_id.is_empty(),
                regained && !f.workspace_id.is_empty() && !f.code.is_empty(),
                f.workspace_id.clone(),
                f.code.clone(),
                was,
            )
        };
        if mark_app {
            self.mark_app_chat_seen(&code);
        } else if mark_seen {
            self.mark_workspace_chat_seen(&ws, &code);
        }
        if focused && !was {
            self.bus.publish(EngineEvent::AppFocused(AppFocused {}));
        }
    }

    /// Records whether a modal or dialog covers the chat. Clearing it with
    /// the app focused marks the selected tab seen.
    pub fn set_chat_ui_occluded(&self, occluded: bool) {
        let (mark_app, mark_seen, ws, code) = {
            let mut f = self.chat_focus.lock();
            let was = f.ui_occludes_chat;
            f.ui_occludes_chat = occluded;
            let cleared = !occluded && was && f.attentive();
            (
                cleared && is_app_chat_code(&f.code) && f.workspace_id.is_empty(),
                cleared && !f.workspace_id.is_empty() && !f.code.is_empty(),
                f.workspace_id.clone(),
                f.code.clone(),
            )
        };
        if mark_app {
            self.mark_app_chat_seen(&code);
        } else if mark_seen {
            self.mark_workspace_chat_seen(&ws, &code);
        }
    }

    /// Records which tab the human views and clears its badge when visible.
    /// An empty id or code clears focus (for that workspace only, when one is
    /// given and it is the focused one).
    pub fn set_workspace_chat_focus(&self, workspace_id: &str, code: &str) {
        let code = code.trim();
        let attentive = {
            let mut f = self.chat_focus.lock();
            if workspace_id.is_empty() || code.is_empty() {
                if workspace_id.is_empty() || f.workspace_id == workspace_id {
                    f.workspace_id.clear();
                    f.code.clear();
                }
                return;
            }
            f.workspace_id = workspace_id.to_owned();
            f.code = code.to_owned();
            f.attentive()
        };
        if attentive {
            self.mark_workspace_chat_seen(workspace_id, code);
        }
    }

    /// Drops focus on a different workspace (switching the watched one).
    pub(crate) fn reset_chat_focus_unless(&self, workspace_id: &str) {
        let mut f = self.chat_focus.lock();
        if f.workspace_id != workspace_id {
            f.workspace_id.clear();
            f.code.clear();
        }
    }

    /// Clears a tab's Unseen flag and its inbox row (the row goes even when
    /// the flag was already clear: desync, closed tab, emit/focus race).
    pub fn mark_workspace_chat_seen(&self, workspace_id: &str, code: &str) {
        let mut code = code.trim().to_owned();
        if code.is_empty() {
            code = PAIR_AGENT_CODE.to_owned();
        }
        if !is_chat_attention_code(&code) {
            return;
        }
        let _g = self.chat_unseen_mu.lock();
        let mut changed = false;
        self.store.update_agent(workspace_id, &code, |a| {
            if a.unseen {
                a.unseen = false;
                changed = true;
            }
        });
        let removed = self.clear_chat_inbox(workspace_id, &code, true);
        if changed || removed {
            self.publish_attention(workspace_id, &code, false);
        }
    }

    /// The unseen + inbox rollup for badges and the dock.
    pub fn attention_counts(&self) -> AttentionCounts {
        self.store.attention_counts()
    }

    /// `attention.updated` with full counts plus the tab that changed.
    pub fn publish_attention(&self, workspace_id: &str, code: &str, unseen: bool) {
        self.publish_attention_evt(workspace_id, code, unseen, false);
    }

    /// `publish_attention` plus `clearAllUnseen` (Mark all read).
    pub fn publish_attention_evt(
        &self,
        workspace_id: &str,
        code: &str,
        unseen: bool,
        clear_all: bool,
    ) {
        let c = self.store.attention_counts();
        self.bus
            .publish(EngineEvent::AttentionUpdated(AttentionUpdated::new(
                c,
                workspace_id,
                code,
                unseen,
                clear_all,
            )));
    }

    /// Raises Unseen on a tab the human is not looking at (and its inbox
    /// row), re-checking focus after the write so a concurrent mark-seen
    /// wins (Go `flagChatUnseen`).
    pub fn flag_chat_unseen(&self, workspace_id: &str, code: &str, loop_finished: bool) {
        if !is_chat_attention_code(code) || self.is_chat_focused(workspace_id, code) {
            return;
        }
        let _g = self.chat_unseen_mu.lock();
        self.store
            .update_agent(workspace_id, code, |a| a.unseen = true);
        let kept = self.emit_chat_inbox(workspace_id, code, loop_finished);
        if self.is_chat_focused(workspace_id, code) {
            let had_unseen = self
                .store
                .agent(workspace_id, code)
                .is_some_and(|a| a.unseen);
            self.store
                .update_agent(workspace_id, code, |a| a.unseen = false);
            let cleared = self.clear_chat_inbox(workspace_id, code, false);
            if cleared {
                self.publish_inbox_updated();
            }
            if cleared || had_unseen {
                self.publish_attention(workspace_id, code, false);
            }
            return;
        }
        if !self
            .store
            .agent(workspace_id, code)
            .is_some_and(|a| a.unseen)
        {
            self.clear_chat_inbox(workspace_id, code, false);
            return;
        }
        if kept {
            self.publish_inbox_updated();
        }
        self.publish_attention(workspace_id, code, true);
    }
}

#[cfg(test)]
mod tests;
