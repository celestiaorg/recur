//! Simplify mode (Go `engine/simplify.go`): the per-session composer chip,
//! frozen on a chat's first write so a later Settings change does not flip
//! it, and inherited into the loops a chat starts. The rails text is
//! [`text`].

pub mod text;

use crate::Engine;
use crate::codes::{MAESTRO_AGENT_CODE, is_pair_chat_code, normalize_pair_chat_code};
use crate::session_config::AgentConfig;

impl Engine {
    /// The rails for a workspace chat turn: on for Agent/Plan modes when the
    /// tab's chip (else Settings) is on.
    pub fn simplify_chat_append(&self, workspace_id: &str, code: &str, chat_mode: &str) -> String {
        if !text::applies_to_chat(chat_mode) || !self.effective_chat_simplify(workspace_id, code) {
            return String::new();
        }
        text::prompt().to_owned()
    }

    /// The rails for a loop Coder turn: follows the loop's (MA) session, not
    /// the Coder's own.
    pub fn simplify_coder_append(&self, workspace_id: &str) -> String {
        if !self.effective_chat_simplify(workspace_id, MAESTRO_AGENT_CODE) {
            return String::new();
        }
        text::prompt().to_owned()
    }

    /// The session's explicit chip (`None` = never set).
    pub fn chat_simplify(&self, workspace_id: &str, code: &str) -> Option<bool> {
        let code = normalize_pair_chat_code(code);
        let sess = self.store.session_for_loop_agent(workspace_id, &code)?;
        AgentConfig::parse(&sess.config_json).simplify
    }

    /// The inject gate: an explicit session flag wins, else Settings.
    pub fn effective_chat_simplify(&self, workspace_id: &str, code: &str) -> bool {
        self.chat_simplify(workspace_id, code)
            .unwrap_or_else(|| self.settings().simplify_enabled())
    }

    /// A loop's starting chip: the pair chat that filed the plan, else the
    /// newest pair chat that chose, else Settings.
    pub fn inherit_simplify(&self, workspace_id: &str) -> bool {
        if let Some(plan) = self.store.workspace(workspace_id).and_then(|w| w.plan) {
            let code = plan.chat_code.trim();
            if is_pair_chat_code(code)
                && let Some(s) = self.chat_simplify(workspace_id, code)
            {
                return s;
            }
        }
        let mut best = 0i64;
        let mut found: Option<bool> = None;
        for a in self.pair_chat_agents(workspace_id) {
            let Some(sess) = self.store.session_for_loop_agent(workspace_id, &a.code) else {
                continue;
            };
            let Some(s) = AgentConfig::parse(&sess.config_json).simplify else {
                continue;
            };
            if found.is_none() || sess.updated_at >= best {
                best = sess.updated_at;
                found = Some(s);
            }
        }
        found.unwrap_or_else(|| self.settings().simplify_enabled())
    }

    /// Freezes the chip on the loop (MA) session at run start, if unset
    /// (atomic, so a concurrent chip toggle is not overwritten).
    pub fn stamp_maestro_simplify_if_unset(&self, workspace_id: &str) {
        let sess = match self
            .store
            .session_for_loop_agent(workspace_id, MAESTRO_AGENT_CODE)
        {
            Some(s) => s,
            None => {
                let Some(s) = self.ensure_agent_session(
                    workspace_id,
                    recur_domain::AgentRole::Maestro.as_str(),
                    MAESTRO_AGENT_CODE,
                    "Maestro",
                    "",
                ) else {
                    return;
                };
                s
            }
        };
        let _ = self
            .store
            .patch_session_config_simplify_if_unset(&sess.id, self.inherit_simplify(workspace_id));
    }

    /// Persists the composer chip for a tab (PA* or MA) without a turn. A
    /// missing pair-chat agent is not recreated (a Remember racing a close
    /// cannot resurrect the tab), and no session is created (that raced the
    /// first send).
    pub fn remember_workspace_chat_simplify(&self, workspace_id: &str, code: &str, on: bool) {
        if self.store.workspace(workspace_id).is_none() {
            return;
        }
        let code = normalize_pair_chat_code(code);
        if code == MAESTRO_AGENT_CODE {
            self.remember_chat_simplify(workspace_id, &code, on);
            return;
        }
        if !is_pair_chat_code(&code) || self.store.agent(workspace_id, &code).is_none() {
            return;
        }
        self.remember_chat_simplify(workspace_id, &code, on);
    }

    pub(crate) fn remember_chat_simplify(&self, workspace_id: &str, code: &str, on: bool) {
        let code = normalize_pair_chat_code(code);
        if code != MAESTRO_AGENT_CODE && !is_pair_chat_code(&code) {
            return;
        }
        if is_pair_chat_code(&code) && self.store.agent(workspace_id, &code).is_none() {
            return;
        }
        if let Some(sess) = self.store.session_for_loop_agent(workspace_id, &code) {
            let _ = self.store.patch_session_config_simplify(&sess.id, on);
        }
    }
}

#[cfg(test)]
mod tests;
