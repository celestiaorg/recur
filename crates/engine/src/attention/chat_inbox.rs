//! Unseen-chat inbox rows: `ib-chat-<ws>-<code>` rows derived from
//! Agent.Unseen, Mark all read, the boot heal, and the "Loop finished" badge on
//! a conductor tab.

use std::collections::BTreeMap;
use std::sync::Arc;

use recur_domain::{InboxItem, WorkspaceState};

use crate::Engine;
use crate::codes::{MAESTRO_AGENT_CODE, is_chat_attention_code, is_chat_retry_display};
use crate::loop_handle::LoopHandle;
use crate::session_config::AgentConfig;
use crate::text::{hard_truncate_runes, html_escape, truncate};

const CHAT_INBOX_ID_PREFIX: &str = "ib-chat-";

/// The length a chat tab title is truncated to.
pub const CHAT_TAB_TITLE_LIMIT: usize = 28;

pub fn chat_inbox_id(workspace_id: &str, code: &str) -> String {
    format!("{CHAT_INBOX_ID_PREFIX}{workspace_id}-{code}")
}

pub fn chat_inbox_target(workspace_id: &str, code: &str) -> String {
    format!("chat:{workspace_id}:{code}")
}

/// Splits an `ib-chat-*` id into workspace and chat tab code.
pub fn parse_chat_inbox_id(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix(CHAT_INBOX_ID_PREFIX)?;
    let i = rest.rfind('-')?;
    if i == 0 || i == rest.len() - 1 {
        return None;
    }
    let (ws, code) = (&rest[..i], &rest[i + 1..]);
    if !is_chat_attention_code(code) {
        return None;
    }
    Some((ws.to_owned(), code.to_owned()))
}

fn is_chat_inbox_item(it: &InboxItem) -> bool {
    it.kind == "chat" || it.id.starts_with(CHAT_INBOX_ID_PREFIX)
}

/// The first non-blank line of a first user message as a tab title: attachment
/// markers stripped, retry markers ignored, at most 200 runes.
pub fn first_user_chat_title(text: &str) -> String {
    if is_chat_retry_display(text) {
        return String::new();
    }
    let text = recur_agent::attach::strip_markers(text);
    let line = text
        .split('\n')
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.is_empty() || is_chat_retry_display(line) {
        return String::new();
    }
    hard_truncate_runes(line, 200).to_owned()
}

/// Settled outcomes that count as "finished": a PR open, or an idle/artifact
/// pause. `state`/`phase` are the walk's own.
pub fn loop_settled(state: &WorkspaceState, phase: &str) -> bool {
    match state {
        WorkspaceState::PrOpen => true,
        WorkspaceState::Paused => phase == "idle" || phase == "artifact",
        _ => false,
    }
}

/// Walker-finished pauses (idle, artifact, missing graph) that no longer
/// keep a schedule busy.
pub fn scheduled_pause_settled(phase: &str) -> bool {
    loop_settled(&WorkspaceState::Paused, phase)
        || phase == "phase missing"
        || phase == "loop refs missing"
}

impl Engine {
    /// The tab label: the stored title, else the first user message, else
    /// "New chat" (never flattens the transcript).
    pub fn chat_tab_title(&self, workspace_id: &str, code: &str) -> String {
        let Some(sess) = self.store.session_for_loop_agent(workspace_id, code) else {
            return "New chat".into();
        };
        let cfg = AgentConfig::parse(&sess.config_json);
        let t = cfg.title.trim();
        if !t.is_empty() {
            return truncate(t, CHAT_TAB_TITLE_LIMIT);
        }
        let t = first_user_chat_title(&self.store.session_first_user_prompt(&sess.id));
        if !t.is_empty() {
            return truncate(&t, CHAT_TAB_TITLE_LIMIT);
        }
        "New chat".into()
    }

    /// Upserts the unseen inbox row for a chat tab; true only when a row was
    /// kept. Does not publish (callers batch `inbox.updated`). Re-checks
    /// Unseen after the write so a concurrent mark-seen leaves no orphan.
    /// `loop_finished` is the event that made the tab unseen.
    pub fn emit_chat_inbox(&self, workspace_id: &str, code: &str, loop_finished: bool) -> bool {
        let Some(ws) = self.store.workspace(workspace_id) else {
            return false;
        };
        if ws.archived || !is_chat_attention_code(code) {
            return false;
        }
        // A parked ask_user already has a Needs-you row for this tab.
        if self.ask_user_pending_for(workspace_id, code) {
            return false;
        }
        if self.pending_mode_continue_for(workspace_id, code) {
            return false;
        }
        let Some(a) = self.store.agent(workspace_id, code) else {
            return false;
        };
        if !a.unseen {
            return false;
        }
        let mut title = self.chat_tab_title(workspace_id, code);
        if code == MAESTRO_AGENT_CODE {
            title = "Workflow".into();
        }
        let mut done = "finished a reply";
        if loop_finished {
            title = "Workflow".into();
            done = "finished";
        }
        let mut ws_label = workspace_id.to_owned();
        if !ws.short.trim().is_empty() {
            ws_label = ws.short.trim().to_owned();
        }
        if ws.is_pr_chat_host()
            && let Some(pr) = self.store.pr(&ws.host_pr_id)
        {
            let mut title_bit = pr.title.trim().to_owned();
            if title_bit.is_empty() {
                title_bit = ws_label.clone();
            }
            ws_label = format!("#{} · {title_bit}", pr.number);
        }
        let mut color = if code == MAESTRO_AGENT_CODE {
            "var(--amber)".to_owned()
        } else {
            "var(--cyan)".to_owned()
        };
        if !a.color.trim().is_empty() {
            color = a.color.clone();
        }
        self.store.add_inbox_item(InboxItem {
            id: chat_inbox_id(workspace_id, code),
            group: "ready".into(),
            unread: true,
            kind: "chat".into(),
            color,
            title: format!("<b>{}</b> {done}", html_escape(&title)),
            sub: html_escape(&truncate(&ws_label, 64)),
            time: "now".into(),
            action: "View".into(),
            action_class: "go".into(),
            target: chat_inbox_target(workspace_id, code),
            workspace_id: workspace_id.to_owned(),
        });
        if !self
            .store
            .agent(workspace_id, code)
            .is_some_and(|a| a.unseen)
        {
            self.store
                .remove_inbox_item(&chat_inbox_id(workspace_id, code));
            return false;
        }
        true
    }

    /// Drops a tab's unseen row; `notify` publishes `inbox.updated` when one
    /// was removed.
    pub fn clear_chat_inbox(&self, workspace_id: &str, code: &str, notify: bool) -> bool {
        if !self
            .store
            .remove_inbox_item(&chat_inbox_id(workspace_id, code))
        {
            return false;
        }
        if notify {
            self.publish_inbox_updated();
        }
        true
    }

    /// Clears every unseen chat tab and every chat inbox row (orphans
    /// included): one `inbox.updated` and one `attention.updated`.
    pub fn mark_all_chats_seen(&self) {
        let _g = self.chat_unseen_mu.lock();
        let mut changed = false;
        for w in self.store.workspaces() {
            if w.archived {
                continue;
            }
            for a in self.store.agents(&w.id) {
                if !a.unseen || !is_chat_attention_code(&a.code) {
                    continue;
                }
                self.store
                    .update_agent(&w.id, &a.code, |ag| ag.unseen = false);
                changed = true;
            }
        }
        if self.store.remove_chat_inbox_items() > 0 {
            changed = true;
        }
        if !changed {
            return;
        }
        self.publish_inbox_updated();
        self.publish_attention_evt("", "", false, true);
    }

    /// Heals chat inbox rows from Agent.Unseen (D42): every live unseen chat
    /// gets a row, stale rows go. Run at boot.
    pub fn sync_chat_inbox(&self) {
        let mut want: BTreeMap<String, (String, String)> = BTreeMap::new();
        for w in self.store.workspaces() {
            if w.archived {
                continue;
            }
            for a in self.store.agents(&w.id) {
                if a.unseen && is_chat_attention_code(&a.code) {
                    want.insert(
                        chat_inbox_id(&w.id, &a.code),
                        (w.id.clone(), a.code.clone()),
                    );
                }
            }
        }
        let mut changed = false;
        for it in self.store.inbox().unwrap_or_default() {
            if !is_chat_inbox_item(&it) {
                continue;
            }
            if want.remove(&it.id).is_some() {
                continue;
            }
            if self.store.remove_inbox_item(&it.id) {
                changed = true;
            }
        }
        for (ws, code) in want.values() {
            if self.emit_chat_inbox(ws, code, false) {
                changed = true;
            }
        }
        if changed {
            self.publish_inbox_updated();
        }
    }

    /// Badges the conductor tab "Loop finished" when a walk settles
    /// off-screen. Stop, failure and composer Stop do not.
    pub async fn maybe_flag_loop_finished(&self, l: &Arc<LoopHandle>) {
        if l.wants_idle() || l.is_stopped() {
            return;
        }
        let Some(ws) = self.store.workspace(&l.id) else {
            return;
        };
        let (state, phase) = l.exit();
        if ws.archived || !loop_settled(&state, &phase) {
            return;
        }
        let mut code = l.chat_code().trim().to_owned();
        if code.is_empty() {
            // A legacy MA-owned walk, not the workspace's current owner.
            code = MAESTRO_AGENT_CODE.to_owned();
        }
        self.flag_loop_finished(&l.id, &code).await;
    }

    /// Marks the conductor tab unseen while it is still open. Holds the life
    /// lock so a close cannot sample Unseen, drop the row, then lose a
    /// completion that lands before the agent is deleted. A missing agent is
    /// a closed tab: never resurrect it for a badge.
    pub async fn flag_loop_finished(&self, workspace_id: &str, code: &str) {
        let code = code.trim();
        if !is_chat_attention_code(code) {
            return;
        }
        let (_life, held) = self.lock_chat_life(workspace_id, code).await;
        if !held {
            return;
        }
        if self.store.agent(workspace_id, code).is_none() {
            return;
        }
        self.flag_chat_unseen(workspace_id, code, true);
    }
}
