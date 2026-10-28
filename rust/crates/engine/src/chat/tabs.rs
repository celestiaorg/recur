//! Chat tab management (Go `workspace_chat.go`): the strip of a
//! workspace's tabs (PA, PA2, … and the Maestro when it is a real chat),
//! create / close / reorder / reopen / delete closed tabs, the dock
//! selection and prompt groups, and remapping picks off disabled providers.

use std::collections::{BTreeMap, HashMap, HashSet};

use recur_agent::Flags;
use recur_domain::{
    AgentRole, CHAT_MODE_LOOP, CHAT_MODE_REVIEW, FeedItem, SessionView, Settings, Workspace,
    WorkspaceChatTab, WorkspaceState,
};

use crate::codes::{
    MAESTRO_AGENT_CODE, PAIR_AGENT_CODE, is_pair_chat_code, normalize_pair_chat_code,
};
use crate::publish::{progress_for, status_label};
use crate::session_config::AgentConfig;
use crate::turn::sessions::pair_chat_code_num;
use crate::turn::transcript::flatten_transcript;
use crate::{Engine, EngineError, Result};

/// The tab code for pair chat number `n` (PA, PA2, …).
pub fn pair_chat_code_from_num(n: i64) -> String {
    if n <= 1 {
        return PAIR_AGENT_CODE.into();
    }
    format!("PA{n}")
}

/// Rearranges tabs to a saved strip order. Unknown and closed codes are
/// skipped; tabs missing from the save keep their default relative
/// positions (an unplaced Maestro still leads, other new tabs append).
pub fn apply_chat_tab_order(
    tabs: Vec<WorkspaceChatTab>,
    order: &[String],
) -> Vec<WorkspaceChatTab> {
    if order.is_empty() || tabs.len() < 2 {
        return tabs;
    }
    let by_code: HashMap<&str, &WorkspaceChatTab> =
        tabs.iter().map(|t| (t.code.as_str(), t)).collect();
    let in_order: HashSet<&str> = order.iter().map(String::as_str).collect();
    let mut out = Vec::with_capacity(tabs.len());
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(t) = by_code.get(MAESTRO_AGENT_CODE)
        && !in_order.contains(MAESTRO_AGENT_CODE)
    {
        out.push((*t).clone());
        seen.insert(MAESTRO_AGENT_CODE.into());
    }
    for code in order {
        let Some(t) = by_code.get(code.as_str()) else {
            continue;
        };
        if seen.contains(code) {
            continue;
        }
        out.push((*t).clone());
        seen.insert(code.clone());
    }
    for t in &tabs {
        if !seen.contains(&t.code) {
            out.push(t.clone());
        }
    }
    out
}

/// `codes` must be a permutation of the open tabs' codes.
pub fn chat_tab_codes_permutation(tabs: &[WorkspaceChatTab], codes: &[String]) -> Result<()> {
    if codes.len() != tabs.len() {
        return Err(EngineError(format!(
            "reorder chats: got {} codes, have {} tabs",
            codes.len(),
            tabs.len()
        )));
    }
    let have: HashSet<&str> = tabs.iter().map(|t| t.code.as_str()).collect();
    let mut seen = HashSet::new();
    for code in codes {
        if !have.contains(code.as_str()) {
            return Err(EngineError(format!("reorder chats: unknown tab {code:?}")));
        }
        if !seen.insert(code.as_str()) {
            return Err(EngineError(format!(
                "reorder chats: duplicate tab {code:?}"
            )));
        }
    }
    Ok(())
}

/// A pick on a provider that is now off: the static fallback when there is
/// one, else Settings' (already remapped) Agent model.
pub fn remap_chat_off_disabled(set: &Settings, r#ref: &str, flags: &Flags) -> (String, Flags) {
    if r#ref.trim().is_empty() {
        return (r#ref.to_owned(), flags.clone());
    }
    let (next, fl) = set.remap_disabled_model(r#ref, flags, false);
    if next != r#ref {
        return (next, fl);
    }
    if !set.model_ref_unavailable(r#ref, false) {
        return (r#ref.to_owned(), flags.clone());
    }
    let agent = set.model_agent.trim();
    if !agent.is_empty() && !set.model_ref_unavailable(agent, false) {
        return (agent.to_owned(), set.agent_flags());
    }
    (r#ref.to_owned(), flags.clone())
}

fn flags_json(flags: &Flags) -> String {
    serde_json::to_string(flags).unwrap_or_else(|_| "{}".into())
}

impl Engine {
    /// Every open chat tab of a workspace. A hand-driven workspace always
    /// has at least PA; loop is a mode on those tabs; the Maestro stays a
    /// peer tab when it is the loop chat. A saved order wins.
    pub fn list_workspace_chats(&self, workspace_id: &str) -> Vec<WorkspaceChatTab> {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return Vec::new();
        };
        apply_chat_tab_order(self.workspace_chat_tabs(&ws), &ws.chat_tab_order)
    }

    /// The default-ordered strip (Maestro first when it is a real tab,
    /// then PA, PA2, …).
    pub(crate) fn workspace_chat_tabs(&self, ws: &Workspace) -> Vec<WorkspaceChatTab> {
        let mut agents = self.pair_chat_agents(&ws.id);
        if agents.is_empty() && !ws.has_run() && !self.chat_life_reserved(&ws.id) {
            self.ensure_pair_agent(&ws.id, PAIR_AGENT_CODE, "");
            agents = self.pair_chat_agents(&ws.id);
        }
        let mut out = Vec::with_capacity(agents.len() + 1);
        if self.include_maestro_chat_tab(ws) {
            out.push(self.maestro_chat_tab(&ws.id));
        }
        for a in agents {
            let mut tab = WorkspaceChatTab {
                code: a.code.clone(),
                title: self.chat_tab_title(&ws.id, &a.code),
                unseen: a.unseen,
                ..WorkspaceChatTab::default()
            };
            if let Some(sess) = self.store.session_for_loop_agent(&ws.id, &a.code) {
                tab.updated_at = sess.updated_at;
                let cfg = AgentConfig::parse(&sess.config_json);
                tab.mode = cfg.mode;
                tab.prompt_group = cfg.prompt_group;
            }
            self.fill_chat_tab_loop(&mut tab, &ws.id, &a.code);
            out.push(tab);
        }
        out
    }

    /// MA belongs on the strip when it is the loop chat, or its agent row
    /// exists and it engaged a loop or has a user message. Closing MA
    /// clears the loop chat so the tab stays gone.
    fn include_maestro_chat_tab(&self, ws: &Workspace) -> bool {
        if ws.loop_chat_code.trim() == MAESTRO_AGENT_CODE {
            return true;
        }
        if self.store.agent(&ws.id, MAESTRO_AGENT_CODE).is_none() {
            return false;
        }
        self.chat_loop_started(&ws.id, MAESTRO_AGENT_CODE)
            || self.chat_has_user_message(&ws.id, MAESTRO_AGENT_CODE)
    }

    fn maestro_chat_tab(&self, workspace_id: &str) -> WorkspaceChatTab {
        let mut tab = WorkspaceChatTab {
            code: MAESTRO_AGENT_CODE.into(),
            title: self.chat_tab_title(workspace_id, MAESTRO_AGENT_CODE),
            mode: CHAT_MODE_LOOP.into(),
            ..WorkspaceChatTab::default()
        };
        if let Some(sess) = self
            .store
            .session_for_loop_agent(workspace_id, MAESTRO_AGENT_CODE)
        {
            tab.updated_at = sess.updated_at;
        }
        if let Some(a) = self.store.agent(workspace_id, MAESTRO_AGENT_CODE) {
            tab.unseen = a.unseen;
        }
        self.fill_chat_tab_loop(&mut tab, workspace_id, MAESTRO_AGENT_CODE);
        tab
    }

    fn fill_chat_tab_loop(&self, tab: &mut WorkspaceChatTab, workspace_id: &str, code: &str) {
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return;
        };
        let cfg = AgentConfig::parse(&sess.config_json);
        tab.loop_id = cfg.loop_id;
        tab.policy_id = cfg.policy_id;
        tab.loop_started = cfg.loop_started;
    }

    /// Persists a new tab order. `codes` must be a permutation of the open
    /// tabs; on a mismatch the current strip comes back with the error.
    pub fn reorder_workspace_chats(
        &self,
        workspace_id: &str,
        codes: &[String],
    ) -> (Vec<WorkspaceChatTab>, Result<()>) {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return (
                Vec::new(),
                Err(EngineError(format!(
                    "reorder chats: unknown workspace {workspace_id:?}"
                ))),
            );
        };
        let tabs = self.workspace_chat_tabs(&ws);
        if let Err(e) = chat_tab_codes_permutation(&tabs, codes) {
            return (apply_chat_tab_order(tabs, &ws.chat_tab_order), Err(e));
        }
        self.store
            .update_workspace(workspace_id, |w| w.chat_tab_order = codes.to_vec());
        (apply_chat_tab_order(tabs, codes), Ok(()))
    }

    /// Opens a new coding-agent tab on the next free PA* code. A hand-driven
    /// workspace always has PA, so the first new chat is PA2; a loop with no
    /// pair tabs starts at PA.
    pub async fn create_workspace_chat(&self, workspace_id: &str) -> WorkspaceChatTab {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return WorkspaceChatTab::default();
        };
        let (_life, held) = self.lock_chat_life(workspace_id, PAIR_AGENT_CODE).await;
        if !held {
            return WorkspaceChatTab::default();
        }
        if !ws.has_run() && self.pair_chat_agents(workspace_id).is_empty() {
            self.ensure_pair_agent(workspace_id, PAIR_AGENT_CODE, "");
        }
        let code = self.new_pair_chat(&ws);
        WorkspaceChatTab {
            code,
            title: "New chat".into(),
            mode: if ws.is_pr_chat_host() {
                CHAT_MODE_REVIEW.into()
            } else {
                String::new()
            },
            ..WorkspaceChatTab::default()
        }
    }

    /// Adds a fresh pair tab, seeded to review mode on PR chat hosts.
    pub(crate) fn new_pair_chat(&self, ws: &Workspace) -> String {
        let code = self.next_pair_chat_code(&ws.id);
        self.ensure_pair_agent(&ws.id, &code, "");
        if ws.is_pr_chat_host() {
            self.remember_pair_chat_selection(&ws.id, &code, "", None, CHAT_MODE_REVIEW, None);
        }
        code
    }

    /// Removes a tab's agent row; the durable history stays reachable from
    /// the closed list. Closing the last pair tab of a hand workspace
    /// replaces it with a fresh chat on a new code (so the closed transcript
    /// is not revived). The tab's walk stops and the loop chat clears, so a
    /// later Review & fix cannot land on a closed conductor; a queued run
    /// parks. Returns the remaining tabs.
    pub async fn close_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Vec<WorkspaceChatTab> {
        let code = normalize_pair_chat_code(code);
        if self.store.workspace(workspace_id).is_none() {
            return Vec::new();
        }
        let (_life, held) = self.lock_chat_life(workspace_id, &code).await;
        if !held {
            return self.list_workspace_chats(workspace_id);
        }
        let Some(ws) = self.store.workspace(workspace_id) else {
            return Vec::new();
        };
        let agents = self.pair_chat_agents(workspace_id);
        let mut cleared = false;
        let mut unseen_gone = false;
        if agents.len() <= 1 && !ws.has_run() {
            for a in &agents {
                if a.unseen {
                    unseen_gone = true;
                }
                if self.clear_chat_inbox(workspace_id, &a.code, false) {
                    cleared = true;
                }
                self.drop_mcp_agent(workspace_id, &a.code);
                self.store.delete_agent(workspace_id, &a.code);
            }
            self.new_pair_chat(&ws);
            if cleared {
                self.publish_inbox_updated();
            }
            if cleared || unseen_gone {
                self.publish_attention(workspace_id, "", false);
            }
            return self.list_workspace_chats(workspace_id);
        }
        if is_pair_chat_code(&code) || code == MAESTRO_AGENT_CODE {
            if self
                .store
                .agent(workspace_id, &code)
                .is_some_and(|a| a.unseen)
            {
                unseen_gone = true;
            }
            if self.clear_chat_inbox(workspace_id, &code, false) {
                cleared = true;
            }
            self.stop_loop_by_chat(workspace_id, &code);
            let mut parked = false;
            let updated = self.store.update_workspace(workspace_id, |w| {
                if w.loop_chat_code.trim() != code {
                    return;
                }
                w.loop_chat_code.clear();
                if w.state == WorkspaceState::Queued {
                    w.state = WorkspaceState::Paused;
                    w.phase = "idle".into();
                    w.status_label = status_label(&WorkspaceState::Paused);
                    w.progress = progress_for(&WorkspaceState::Paused);
                    w.elapsed_label = "paused".into();
                    parked = true;
                }
            });
            if let Some(u) = updated.filter(|_| parked) {
                self.publish_loop_updated(&u.id, u.state.clone(), &u.phase, u.iteration);
                self.publish_resource();
            }
            self.store.delete_agent(workspace_id, &code);
            self.drop_mcp_agent(workspace_id, &code);
        }
        if cleared {
            self.publish_inbox_updated();
        }
        if cleared || unseen_gone {
            self.publish_attention(workspace_id, &code, false);
        }
        let remaining = self.list_workspace_chats(workspace_id);
        if !remaining.is_empty() {
            return remaining;
        }
        self.new_pair_chat(&ws);
        self.list_workspace_chats(workspace_id)
    }

    /// Pair and MA sessions that still have a transcript but whose agent
    /// row was deleted (closed tabs), newest first.
    pub fn list_closed_workspace_chats(&self, workspace_id: &str) -> Vec<WorkspaceChatTab> {
        if self.store.workspace(workspace_id).is_none() {
            return Vec::new();
        }
        let mut open: HashSet<String> = self
            .pair_chat_agents(workspace_id)
            .into_iter()
            .map(|a| a.code)
            .collect();
        if self.store.agent(workspace_id, MAESTRO_AGENT_CODE).is_some() {
            open.insert(MAESTRO_AGENT_CODE.into());
        }
        let mut out: Vec<WorkspaceChatTab> = Vec::new();
        let mut seen = HashSet::new();
        for s in self
            .store
            .sessions_for_loop(workspace_id)
            .unwrap_or_default()
        {
            let cfg = AgentConfig::parse(&s.config_json);
            if (!is_pair_chat_code(&cfg.code) && cfg.code != MAESTRO_AGENT_CODE)
                || open.contains(&cfg.code)
                || seen.contains(&cfg.code)
            {
                continue;
            }
            if !self.store.session_has_messages(&s.id) {
                continue;
            }
            seen.insert(cfg.code.clone());
            out.push(WorkspaceChatTab {
                title: self.chat_tab_title(workspace_id, &cfg.code),
                code: cfg.code,
                updated_at: s.updated_at,
                mode: cfg.mode,
                prompt_group: cfg.prompt_group,
                ..WorkspaceChatTab::default()
            });
        }
        out.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        out
    }

    /// Permanently drops a closed pair/MA chat's session and the specialist
    /// sessions it owned (messages a fork still shares are kept). Open tabs,
    /// in-flight turns and running loops are refused so the UI can restore
    /// its state. Returns the remaining closed chats (never `None`).
    pub async fn delete_closed_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> (Vec<WorkspaceChatTab>, Result<()>) {
        let remaining = || self.list_closed_workspace_chats(workspace_id);
        if self.store.workspace(workspace_id).is_none() {
            return (remaining(), Ok(()));
        }
        let code = if code == MAESTRO_AGENT_CODE {
            code.to_owned()
        } else {
            let c = normalize_pair_chat_code(code);
            if !is_pair_chat_code(&c) {
                return (remaining(), Ok(()));
            }
            c
        };
        let (_life, held) = self.lock_chat_life(workspace_id, &code).await;
        if !held {
            return (remaining(), Ok(()));
        }
        if self.store.agent(workspace_id, &code).is_some() {
            return (remaining(), Err(EngineError::new("chat is still open")));
        }
        if self.live_chat_turn(workspace_id, &code).is_some()
            || self
                .running_loop_record_by_chat(workspace_id, &code)
                .is_some()
        {
            return (remaining(), Err(EngineError::new("chat is still running")));
        }
        self.take_mode_continue(workspace_id, &code);
        self.reset_mode_remounts(workspace_id, &code);
        self.invalidate_continue_gens(workspace_id, &code);
        if let Err(e) = self.store.delete_closed_chat(workspace_id, &code) {
            return (
                remaining(),
                Err(EngineError(format!("delete closed chat: {e}"))),
            );
        }
        (remaining(), Ok(()))
    }

    /// Restores a closed tab so its transcript is found again. Unknown and
    /// open codes are ignored. MA reclaims the loop chat when none is set.
    pub async fn reopen_workspace_chat(
        &self,
        workspace_id: &str,
        code: &str,
    ) -> Vec<WorkspaceChatTab> {
        if code == MAESTRO_AGENT_CODE {
            let (_life, held) = self.lock_chat_life(workspace_id, code).await;
            if !held || self.store.workspace(workspace_id).is_none() {
                return Vec::new();
            }
            if self
                .store
                .session_for_loop_agent(workspace_id, MAESTRO_AGENT_CODE)
                .is_none()
            {
                return self.list_workspace_chats(workspace_id);
            }
            self.store.update_workspace(workspace_id, |w| {
                if w.loop_chat_code.trim().is_empty() {
                    w.loop_chat_code = MAESTRO_AGENT_CODE.into();
                }
            });
            self.ensure_maestro_agent(workspace_id, "");
            return self.list_workspace_chats(workspace_id);
        }
        let code = normalize_pair_chat_code(code);
        let (_life, held) = self.lock_chat_life(workspace_id, &code).await;
        if !held || self.store.workspace(workspace_id).is_none() {
            return Vec::new();
        }
        if !is_pair_chat_code(&code)
            || self
                .store
                .session_for_loop_agent(workspace_id, &code)
                .is_none()
            || self
                .pair_chat_agents(workspace_id)
                .iter()
                .any(|a| a.code == code)
        {
            return self.list_workspace_chats(workspace_id);
        }
        self.ensure_pair_agent(workspace_id, &code, "");
        self.list_workspace_chats(workspace_id)
    }

    /// The next free PA* code over open agents and every prior pair
    /// session, so a "fresh" tab never resumes a closed chat.
    pub fn next_pair_chat_code(&self, workspace_id: &str) -> String {
        let mut max = self
            .pair_chat_agents(workspace_id)
            .iter()
            .map(|a| pair_chat_code_num(&a.code))
            .max()
            .unwrap_or(0);
        for s in self
            .store
            .sessions_for_loop(workspace_id)
            .unwrap_or_default()
        {
            let cfg = AgentConfig::parse(&s.config_json);
            if is_pair_chat_code(&cfg.code) {
                max = max.max(pair_chat_code_num(&cfg.code));
            }
        }
        pair_chat_code_from_num(max + 1)
    }

    /// Stamps a `[[prompt]]` tab_group on a chat so later grouped actions
    /// reuse it instead of the operator's working tab.
    pub async fn remember_workspace_chat_prompt_group(
        &self,
        workspace_id: &str,
        code: &str,
        group: &str,
    ) {
        if self.store.workspace(workspace_id).is_none() {
            return;
        }
        let mut code = normalize_pair_chat_code(code);
        if !is_pair_chat_code(&code) {
            code = PAIR_AGENT_CODE.into();
        }
        let (_life, held) = self.lock_chat_life(workspace_id, &code).await;
        if !held {
            return;
        }
        self.stamp_chat_prompt_group(workspace_id, &code, group);
    }

    /// Creates the durable session when the group stamp wins the race with
    /// the first dock Remember; a missing agent (a just-closed tab) is not
    /// resurrected.
    fn stamp_chat_prompt_group(&self, workspace_id: &str, code: &str, group: &str) {
        let group = group.trim();
        if group.is_empty() || self.store.agent(workspace_id, code).is_none() {
            return;
        }
        let Some(sess) = self
            .store
            .session_for_loop_agent(workspace_id, code)
            .or_else(|| {
                self.ensure_agent_session(
                    workspace_id,
                    AgentRole::Coder.as_str(),
                    code,
                    "Agent",
                    "",
                )
            })
        else {
            return;
        };
        let mut m = BTreeMap::new();
        m.insert("promptGroup", group);
        let Ok(raw) = serde_json::to_string(&m) else {
            return;
        };
        let _ = self.store.patch_session_config_merge(&sess.id, &raw);
    }

    /// Rewrites pair-chat dock picks whose provider is now off.
    pub fn remap_chats_off_disabled_providers(&self, set: &Settings) {
        for ws in self.store.workspaces() {
            for a in self.pair_chat_agents(&ws.id) {
                let (next, _) = remap_chat_off_disabled(set, &a.model, &Flags::new());
                if next != a.model {
                    self.store
                        .update_agent(&ws.id, &a.code, |ag| ag.model = next.clone());
                }
                let Some(sess) = self.store.session_for_loop_agent(&ws.id, &a.code) else {
                    continue;
                };
                let cfg = AgentConfig::parse(&sess.config_json);
                if cfg.model.trim().is_empty() {
                    continue;
                }
                let (next, flags) = remap_chat_off_disabled(set, &cfg.model, &cfg.flags);
                if next == cfg.model {
                    continue;
                }
                let mut patch = BTreeMap::new();
                patch.insert("model", next);
                let Ok(patch) = serde_json::to_string(&patch) else {
                    continue;
                };
                let _ = self
                    .store
                    .patch_session_config_dock(&sess.id, &patch, &flags_json(&flags));
            }
        }
    }

    /// Specialist sessions of one chat (store order, oldest first),
    /// flattened for the Plan panel and the Loop cockpit. Pair and Maestro
    /// transcripts load per open tab instead. Sessions with no transcript
    /// are omitted. An empty `agent_code` loads every specialist the chat
    /// owns; legacy unowned sessions belong to PA and MA.
    pub fn workspace_sessions(
        &self,
        workspace_id: &str,
        owner_code: &str,
        agent_code: &str,
    ) -> Vec<SessionView> {
        let mut out = Vec::new();
        for s in self
            .store
            .sessions_for_loop(workspace_id)
            .unwrap_or_default()
        {
            let cfg = AgentConfig::parse(&s.config_json);
            if cfg.code == MAESTRO_AGENT_CODE || is_pair_chat_code(&cfg.code) {
                continue;
            }
            let owns = cfg.owner_code == owner_code
                || cfg.owner_code.is_empty()
                    && (owner_code == PAIR_AGENT_CODE || owner_code == MAESTRO_AGENT_CODE);
            if !owns || (!agent_code.is_empty() && cfg.code != agent_code) {
                continue;
            }
            let msgs = match self.store.session_transcript(&s.id) {
                Ok(Some(m)) if !m.is_empty() => m,
                _ => continue,
            };
            out.push(SessionView {
                id: s.id.clone(),
                items: Some(flatten_transcript(
                    &self.providers,
                    &msgs,
                    &cfg.model,
                    &cfg.flags,
                )),
                code: cfg.code,
                name: cfg.name,
                kind: s.kind,
                status: s.status,
                owner_code: cfg.owner_code,
                created_at: s.created_at,
                updated_at: s.updated_at,
            });
        }
        out
    }

    /// The loop-level facts (outcomes and deviations from the events table)
    /// with their real timestamps, for the Loop cockpit.
    pub fn workspace_facts(&self, workspace_id: &str) -> Vec<FeedItem> {
        self.loop_fact_rows(workspace_id)
            .into_iter()
            .map(|r| {
                let mut it = r.item;
                it.ts = r.ts;
                it
            })
            .collect()
    }
}
