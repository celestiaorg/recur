//! Chat-send helpers shared by the Loop and Agent send paths (Go
//! `workspace_chat.go` and the rename prompts of `modes.go`): first-message
//! titles and rename nudges, provisional titles stamped ahead of a queued
//! send, the standing system-prompt fingerprint, and the dock selection a
//! send persists. Ported with E3 because `send_maestro_message` and loop
//! engage need them; E4's send pipeline uses the same functions.

use std::collections::BTreeMap;

use recur_agent::attach::strip_markers;
use recur_agent::display::hard_truncate_runes;
use recur_domain::{
    AgentRole, CHAT_MODE_AGENT, CHAT_MODE_LOOP, Workspace, normalize_chat_mode, parse_model_ref,
};
use sha2::{Digest, Sha256};

use crate::Engine;
use crate::attention::chat_inbox::first_user_chat_title;
use crate::bus::{ChatTab, EngineEvent};
use crate::codes::{
    MAESTRO_AGENT_CODE, is_chat_retry_display, is_pair_chat_code, normalize_pair_chat_code,
};
use crate::session_config::AgentConfig;
use crate::text::truncate;
use crate::workspace::create::DEFAULT_WORKSPACE_TITLE;

/// Appended on the first PA* message in a workspace so the agent names the
/// workspace, branch and chat via MCP.
pub const FIRST_WORKSPACE_RENAME_PROMPT: &str = "This is the first session in this workspace. After you understand the request, call mcp__recur__rename_workspace (short title, at most 8 words, sentence case), mcp__recur__rename_branch (kebab-case topic, e.g. add-dark-mode), and mcp__recur__rename_session (3–5 word chat title, sentence case). No confirmation.";

/// The Ask/Plan/Review variant: those modes do not allowlist rename_branch.
pub const FIRST_WORKSPACE_RENAME_PROMPT_NO_BRANCH: &str = "This is the first session in this workspace. After you understand the request, call mcp__recur__rename_workspace (short title, at most 8 words, sentence case) and mcp__recur__rename_session (3–5 word chat title, sentence case). No confirmation.";

/// Appended on the first message of a later chat tab.
pub const FIRST_CHAT_RENAME_PROMPT: &str = "This is the first message in this chat. Call mcp__recur__rename_session once with a 3–5 word topic title in sentence case. No confirmation.";

/// The provisional sidebar title's word cap (rename_workspace's "at most 8
/// words").
pub const WORKSPACE_TITLE_WORDS: usize = 8;
/// The provisional title's rune cap.
pub const WORKSPACE_TITLE_LIMIT: usize = 60;

/// Titles stamped from a queued message before its send ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EarlyTitles {
    pub workspace: std::collections::HashMap<String, EarlyWorkspaceTitle>,
    /// `workspace\x00code` → the stamped chat title.
    pub chats: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EarlyWorkspaceTitle {
    pub goal: String,
    pub prev_goal: String,
    pub prev_short: String,
}

/// True until the first message names the workspace or rename_workspace
/// runs (or the human named it at create).
pub fn is_unnamed_workspace(ws: &Workspace) -> bool {
    let g = ws.goal.trim();
    g.is_empty() || g == DEFAULT_WORKSPACE_TITLE
}

/// The first `n` whitespace-separated words of `s`.
pub fn truncate_words(s: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }
    s.split_whitespace().take(n).collect::<Vec<_>>().join(" ")
}

/// The untitled workspace's sidebar label: the first non-empty line, at
/// most [`WORKSPACE_TITLE_WORDS`], rune-capped. "" leaves New Workspace.
pub fn provisional_workspace_title(text: &str) -> String {
    let text = strip_markers(text);
    let Some(line) = text.split('\n').map(str::trim).find(|l| !l.is_empty()) else {
        return String::new();
    };
    let title = hard_truncate_runes(
        &truncate_words(line, WORKSPACE_TITLE_WORDS),
        WORKSPACE_TITLE_LIMIT,
    )
    .to_owned();
    if title.is_empty() || title == DEFAULT_WORKSPACE_TITLE {
        return String::new();
    }
    title
}

/// sha256 hex of a standing system prompt.
pub fn standing_prompt_sha(prompt: &str) -> String {
    hex::encode(Sha256::digest(prompt.as_bytes()))
}

fn early_chat_key(workspace_id: &str, code: &str) -> String {
    format!("{workspace_id}\x00{code}")
}

impl Engine {
    /// Whether this tab already has a user text turn.
    pub fn chat_has_user_message(&self, workspace_id: &str, code: &str) -> bool {
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return false;
        };
        let text = self.store.session_first_user_prompt(&sess.id);
        !text.trim().is_empty() && !is_chat_retry_display(&text)
    }

    /// The message about to be sent is this tab's first.
    pub fn is_first_chat_message(&self, workspace_id: &str, code: &str) -> bool {
        !self.chat_has_user_message(workspace_id, code)
    }

    /// No PA* (or Maestro) chat in the workspace has a user message yet.
    pub fn is_first_workspace_session(&self, workspace_id: &str, code: &str) -> bool {
        if !self.is_first_chat_message(workspace_id, code) {
            return false;
        }
        for a in self.pair_chat_agents(workspace_id) {
            if a.code != code && self.chat_has_user_message(workspace_id, &a.code) {
                return false;
            }
        }
        !self.chat_has_user_message(workspace_id, MAESTRO_AGENT_CODE)
    }

    /// Asks the agent to name the workspace/branch/chat on the first session
    /// of an untitled workspace, or just the chat on a later tab's first
    /// message. "" when neither applies.
    pub fn rename_nudge_prompt(
        &self,
        workspace_id: &str,
        code: &str,
        mode: &str,
        name_from: &str,
    ) -> String {
        if self.is_first_workspace_session(workspace_id, code)
            && let Some(ws) = self.store.workspace(workspace_id)
        {
            let provisional = provisional_workspace_title(name_from);
            let just_named = !provisional.is_empty() && ws.goal == provisional;
            if is_unnamed_workspace(&ws) || just_named {
                let m = normalize_chat_mode(mode);
                if m == CHAT_MODE_AGENT || m == CHAT_MODE_LOOP {
                    return FIRST_WORKSPACE_RENAME_PROMPT.into();
                }
                return FIRST_WORKSPACE_RENAME_PROMPT_NO_BRANCH.into();
            }
        }
        if self.is_first_chat_message(workspace_id, code) {
            return FIRST_CHAT_RENAME_PROMPT.into();
        }
        String::new()
    }

    /// Sets Goal/Short from the first user message while the workspace is
    /// untitled. Explicit names are left alone; rename_workspace overwrites.
    pub fn maybe_apply_provisional_workspace_title(&self, ws: &Workspace, text: &str) -> bool {
        if ws.has_run() || !is_unnamed_workspace(ws) {
            return false;
        }
        let title = provisional_workspace_title(text);
        if title.is_empty() {
            return false;
        }
        let t = title.clone();
        let Some(updated) = self.store.update_workspace(&ws.id, |lp| {
            if lp.has_run() || !is_unnamed_workspace(lp) {
                return;
            }
            lp.short = truncate(&t, 48);
            lp.goal = t;
        }) else {
            return false;
        };
        if updated.goal != title {
            return false;
        }
        self.publish_loop_updated(
            &updated.id,
            updated.state.clone(),
            &updated.phase,
            updated.iteration,
        );
        true
    }

    /// Stamps the first-message workspace and tab titles for a send queued
    /// behind worktree setup, so they do not wait for the turn.
    pub fn apply_provisional_titles(&self, workspace_id: &str, code: &str, text: &str) {
        if is_chat_retry_display(text) {
            return;
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return;
        };
        if self.maybe_apply_provisional_workspace_title(&ws, text) {
            self.early_titles.lock().workspace.insert(
                workspace_id.to_owned(),
                EarlyWorkspaceTitle {
                    goal: provisional_workspace_title(text),
                    prev_goal: ws.goal.clone(),
                    prev_short: ws.short.clone(),
                },
            );
        }
        let title = self.maybe_apply_provisional_chat_title(workspace_id, code, text);
        if !title.is_empty() {
            self.early_titles
                .lock()
                .chats
                .insert(early_chat_key(workspace_id, code), title);
        }
    }

    /// Undoes titles stamped from a queued message when the first real send
    /// would name differently, so the send re-derives them (and still gets
    /// the first-session nudge).
    pub fn drop_early_titles(&self, mut ws: Workspace, code: &str, name_from: &str) -> Workspace {
        let early_ws = self.early_titles.lock().workspace.remove(&ws.id);
        if let Some(t) = early_ws {
            let title = provisional_workspace_title(name_from);
            if !title.is_empty()
                && title != t.goal
                && let Some(updated) = self.store.update_workspace(&ws.id, |lp| {
                    if lp.goal == t.goal {
                        lp.goal = t.prev_goal.clone();
                        lp.short = t.prev_short.clone();
                    }
                })
            {
                ws = updated;
            }
        }
        let early_chat = self
            .early_titles
            .lock()
            .chats
            .remove(&early_chat_key(&ws.id, code));
        if let Some(v) = early_chat {
            let title = first_user_chat_title(name_from);
            if !title.is_empty()
                && title != v
                && let Some(sess) = self.store.session_for_loop_agent(&ws.id, code)
                && AgentConfig::parse(&sess.config_json).title == v
            {
                let _ = self.store.patch_session_config_title(&sess.id, "");
            }
        }
        ws
    }

    /// Writes the first-message tab label (only on the tab's first send;
    /// a rename_session overwrites it). Returns the title stamped.
    pub fn maybe_apply_provisional_chat_title(
        &self,
        workspace_id: &str,
        code: &str,
        text: &str,
    ) -> String {
        if code != MAESTRO_AGENT_CODE && !is_pair_chat_code(code) {
            return String::new();
        }
        if !self.is_first_chat_message(workspace_id, code) {
            return String::new();
        }
        let title = first_user_chat_title(text);
        if title.is_empty() {
            return String::new();
        }
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return String::new();
        };
        if !AgentConfig::parse(&sess.config_json)
            .title
            .trim()
            .is_empty()
        {
            return String::new();
        }
        if self
            .store
            .patch_session_config_title(&sess.id, &title)
            .is_err()
        {
            return String::new();
        }
        self.bus.publish(EngineEvent::WorkspaceChatTab(ChatTab {
            code: code.to_owned(),
            title: title.clone(),
            workspace_id: workspace_id.to_owned(),
        }));
        title
    }

    /// Whether the standing system prompt changed since it was last attached
    /// to the resumed provider session.
    pub fn standing_prompt_changed(
        &self,
        workspace_id: &str,
        code: &str,
        prompt: &str,
        resume: &str,
    ) -> bool {
        if resume.is_empty() {
            return false;
        }
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return true;
        };
        AgentConfig::parse(&sess.config_json)
            .standing_prompt_sha
            .trim()
            != standing_prompt_sha(prompt)
    }

    /// Records the standing system prompt the session now carries.
    pub fn remember_standing_prompt(&self, workspace_id: &str, code: &str, prompt: &str) {
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return;
        };
        let sha = standing_prompt_sha(prompt);
        let mut cfg = AgentConfig::parse(&sess.config_json);
        if cfg.standing_prompt_sha == sha {
            return;
        }
        cfg.standing_prompt_sha = sha;
        let Ok(raw) = recur_domain::json::to_string(&cfg) else {
            return;
        };
        let _ = self.store.update_session_config_json(&sess.id, &raw);
    }

    /// The dock's picks for a pair chat, persisted so a reload restores
    /// them (Go `RememberWorkspaceChatSelection`). Coerced to a pair-chat
    /// tab; a missing agent is not recreated (a Remember racing a close
    /// cannot resurrect the tab). `simplify` `None` leaves the chip unset.
    pub async fn remember_workspace_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
        model: &str,
        flags: Option<&BTreeMap<String, String>>,
        mode: &str,
        simplify: Option<bool>,
    ) {
        if self.store.workspace(workspace_id).is_none() {
            return;
        }
        let mut code = normalize_pair_chat_code(code);
        if !is_pair_chat_code(&code) {
            code = crate::codes::PAIR_AGENT_CODE.to_owned();
        }
        let (_life, held) = self.lock_chat_life(workspace_id, &code).await;
        if !held || self.store.agent(workspace_id, &code).is_none() {
            return;
        }
        self.remember_pair_chat_selection(workspace_id, &code, model, flags, mode, simplify);
    }

    /// Persists the dock's model + flags + mode + simplify onto a pair
    /// chat's session (and agent model) so a reload restores the pickers.
    /// Creates the session when needed. MA and specialist tabs are never
    /// written.
    pub fn remember_pair_chat_selection(
        &self,
        workspace_id: &str,
        code: &str,
        model: &str,
        flags: Option<&BTreeMap<String, String>>,
        mode: &str,
        simplify: Option<bool>,
    ) {
        let code = normalize_pair_chat_code(code);
        let model = model.trim();
        let mode = mode.trim();
        if model.is_empty() && mode.is_empty() {
            return;
        }
        if code == MAESTRO_AGENT_CODE || !is_pair_chat_code(&code) {
            return;
        }
        self.ensure_pair_agent(workspace_id, &code, model);
        if !model.is_empty() {
            self.store
                .update_agent(workspace_id, &code, |a| a.model = model.to_owned());
        }
        let provider = parse_model_ref(model).provider;
        let Some(mut sess) = self
            .store
            .session_for_loop_agent(workspace_id, &code)
            .or_else(|| {
                self.ensure_agent_session(
                    workspace_id,
                    AgentRole::Coder.as_str(),
                    &code,
                    "Agent",
                    &provider,
                )
            })
        else {
            return;
        };
        let old_provider = sess.provider.clone();
        if !model.is_empty() && !provider.is_empty() && sess.provider != provider {
            let _ = self.store.update_session_provider(&sess.id, &provider);
            sess.provider = provider.clone();
        }
        // json_patch so a concurrent simplify json_set is not clobbered. A
        // foreign session id is kept (AutoResume needs it) but marked so a
        // later user send injects instead of resuming it.
        let mut patch = serde_json::Map::new();
        patch.insert("code".into(), code.clone().into());
        patch.insert("name".into(), "Agent".into());
        let mut flags_json = String::new();
        if !model.is_empty() {
            if let Some(a) = self.store.agent(workspace_id, &code)
                && !a.session_id.is_empty()
            {
                let switched =
                    !old_provider.is_empty() && !provider.is_empty() && old_provider != provider;
                if switched || !self.resume_fits_provider(&a.session_id, &provider) {
                    patch.insert("forkInject".into(), true.into());
                }
            }
            patch.insert("model".into(), model.into());
            flags_json = match flags {
                Some(f) => recur_domain::json::to_string(f).unwrap_or_else(|_| "{}".into()),
                None => "{}".into(),
            };
        }
        if !mode.is_empty() {
            patch.insert("mode".into(), normalize_chat_mode(mode).into());
        }
        let raw = sorted_json(patch);
        let _ = self
            .store
            .patch_session_config_dock(&sess.id, &raw, &flags_json);
        match simplify {
            Some(on) => {
                let _ = self.store.patch_session_config_simplify(&sess.id, on);
            }
            // Freeze Simplify on the first dock/send write so a later
            // Settings change does not flip a chat that never touched it.
            None => {
                let _ = self.store.patch_session_config_simplify_if_unset(
                    &sess.id,
                    self.settings().simplify_enabled(),
                );
            }
        }
    }

    /// Whether a stored resume handle belongs to the provider (Go
    /// `resumeFitsProvider`): an empty handle fits; an unknown provider fits
    /// nothing.
    pub fn resume_fits_provider(&self, handle: &str, provider_id: &str) -> bool {
        let handle = handle.trim();
        if handle.is_empty() {
            return true;
        }
        let p = self.providers.get(provider_id).or_else(|| {
            self.providers
                .for_model(&format!("{provider_id}/"))
                .filter(|p| p.id() == provider_id)
        });
        p.is_some_and(|p| p.resume_handle_fits(handle))
    }
}

/// Go `json.Marshal` of a `map[string]any`: keys sorted.
fn sorted_json(m: serde_json::Map<String, serde_json::Value>) -> String {
    let sorted: BTreeMap<String, serde_json::Value> = m.into_iter().collect();
    recur_domain::json::to_string(&sorted).unwrap_or_else(|_| "{}".into())
}
