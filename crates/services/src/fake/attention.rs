//! Chat attention as the engine tracks it: which chat tab is focused, whether
//! a dialog occludes it, which tabs finished a turn unseen, and the events a
//! call queues for after it answers.

use std::collections::{BTreeMap, BTreeSet};

use recur_domain::{AttentionCounts, InboxItem, WorkspaceState, run_active};

use super::{Fake, Seed};
use crate::events::{AgentUpdated, AppEvent, AttentionUpdated, InboxUpdated, WorkspaceUpdated};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatAttention {
    pub occluded: bool,
    pub focus_ws: String,
    pub focus_code: String,
    /// `workspaceId::code` of every unseen tab.
    pub unseen: BTreeSet<String>,
}

/// The attention view plus what a call queued for after its answer.
#[derive(Default)]
pub(super) struct State {
    pub view: ChatAttention,
    pending_events: Vec<AppEvent>,
    pending_publish: Option<Publish>,
}

struct Publish {
    workspace_id: String,
    code: String,
    clear_all_unseen: bool,
}

fn chat_tab_key(workspace_id: &str, code: &str) -> String {
    format!("{workspace_id}::{code}")
}

fn chat_inbox_id(workspace_id: &str, code: &str) -> String {
    format!("ib-chat-{workspace_id}-{code}")
}

/// The workspace and tab of an `ib-chat-…` inbox row.
pub(super) fn parse_chat_inbox_id(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix("ib-chat-")?;
    let i = rest.rfind('-')?;
    if i == 0 {
        return None;
    }
    Some((rest[..i].to_owned(), rest[i + 1..].to_owned()))
}

fn upsert_chat_inbox(seed: &mut Seed, workspace_id: &str, code: &str) {
    let id = chat_inbox_id(workspace_id, code);
    let ws = seed.workspace(workspace_id);
    let mut sub = ws
        .map(|w| w.short.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(workspace_id)
        .to_owned();
    let host_pr_id = ws.map(|w| w.host_pr_id.trim()).unwrap_or("");
    if let Some(pr) = seed.pr(host_pr_id).filter(|_| !host_pr_id.is_empty()) {
        let shown = if pr.title.is_empty() { &sub } else { &pr.title };
        sub = format!("#{} · {shown}", pr.number);
    }
    let row = InboxItem {
        id: id.clone(),
        group: "ready".into(),
        unread: true,
        kind: "chat".into(),
        color: "var(--cyan)".into(),
        title: format!(
            "<b>{}</b> finished a reply",
            if code == "MA" { "Workflow" } else { "Chat" }
        ),
        sub,
        time: "now".into(),
        action: "View".into(),
        action_class: "go".into(),
        target: format!("chat:{workspace_id}:{code}"),
        workspace_id: workspace_id.to_owned(),
    };
    match seed.inbox.iter().position(|it| it.id == id) {
        Some(i) => seed.inbox[i] = row,
        None => seed.inbox.insert(0, row),
    }
}

fn remove_chat_inbox(seed: &mut Seed, workspace_id: &str, code: &str) {
    let id = chat_inbox_id(workspace_id, code);
    seed.inbox.retain(|it| it.id != id);
}

/// Counts as the engine's `AttentionCounts` (no app tab).
pub(super) fn rollup(seed: &Seed, unseen: &BTreeSet<String>) -> AttentionCounts {
    let mut by_workspace: BTreeMap<String, i64> = BTreeMap::new();
    let mut by_project: BTreeMap<String, i64> = BTreeMap::new();
    for key in unseen {
        let ws_id = key.find("::").map_or(key.as_str(), |i| &key[..i]);
        *by_workspace.entry(ws_id.to_owned()).or_default() += 1;
        let ws = seed.workspace(ws_id);
        if ws.is_some_and(|w| !w.host_pr_id.is_empty()) {
            continue;
        }
        if let Some(proj) = ws.map(|w| &w.project_id).filter(|p| !p.is_empty()) {
            *by_project.entry(proj.clone()).or_default() += 1;
        }
    }
    let n = unseen.len() as i64;
    let unread = |chat: bool| {
        seed.inbox
            .iter()
            .filter(|it| it.unread && (it.kind == "chat") == chat)
            .count() as i64
    };
    let other_unread = unread(false);
    AttentionCounts {
        unseen: n,
        inbox_unread: other_unread + unread(true),
        dock: n + other_unread,
        by_workspace: Some(by_workspace),
        by_project: Some(by_project),
        app_unseen: 0,
    }
}

fn attention_updated(
    seed: &Seed,
    view: &ChatAttention,
    workspace_id: &str,
    code: &str,
    tab_unseen: bool,
    clear_all_unseen: bool,
) -> AppEvent {
    AppEvent::AttentionUpdated(AttentionUpdated {
        counts: rollup(seed, &view.unseen),
        workspace_id: workspace_id.to_owned(),
        code: code.to_owned(),
        tab_unseen,
        clear_all_unseen,
    })
}

pub(super) fn turn_idle(
    seed: &mut Seed,
    st: &mut State,
    workspace_id: &str,
    code: &str,
) -> Vec<AppEvent> {
    let v = &mut st.view;
    let focused = !v.occluded && v.focus_ws == workspace_id && v.focus_code == code;
    let key = chat_tab_key(workspace_id, code);
    if focused {
        v.unseen.remove(&key);
        remove_chat_inbox(seed, workspace_id, code);
    } else {
        v.unseen.insert(key);
        upsert_chat_inbox(seed, workspace_id, code);
    }
    let mut events = vec![AppEvent::AgentUpdated(AgentUpdated {
        workspace_id: workspace_id.to_owned(),
        agent_id: format!("{workspace_id}-{code}"),
        status: "idle".into(),
        ..Default::default()
    })];
    if !focused {
        events.push(attention_updated(
            seed,
            &st.view,
            workspace_id,
            code,
            true,
            false,
        ));
        events.push(AppEvent::InboxUpdated(InboxUpdated::default()));
    }
    events
}

/// What a successful call queued, in publish order.
pub(super) fn take_pending(seed: &Seed, st: &mut State) -> Vec<AppEvent> {
    let mut events = std::mem::take(&mut st.pending_events);
    if let Some(p) = st.pending_publish.take() {
        events.push(attention_updated(
            seed,
            &st.view,
            &p.workspace_id,
            &p.code,
            false,
            p.clear_all_unseen,
        ));
        events.push(AppEvent::InboxUpdated(InboxUpdated::default()));
    }
    events
}

fn mark_focused_seen(seed: &mut Seed, st: &mut State) {
    let v = &mut st.view;
    if v.occluded || v.focus_ws.is_empty() || v.focus_code.is_empty() {
        return;
    }
    if v.unseen.remove(&chat_tab_key(&v.focus_ws, &v.focus_code)) {
        let (ws, code) = (v.focus_ws.clone(), v.focus_code.clone());
        remove_chat_inbox(seed, &ws, &code);
        st.pending_publish = Some(Publish {
            workspace_id: ws,
            code,
            clear_all_unseen: false,
        });
    }
}

/// The bookkeeping that runs before a call, override or not.
impl Fake {
    pub(super) fn focus_chat(&self, workspace_id: &str, code: &str) {
        let mut seed = self.seed.lock();
        let mut st = self.attention.lock();
        st.view.focus_ws = workspace_id.to_owned();
        st.view.focus_code = code.to_owned();
        mark_focused_seen(&mut seed, &mut st);
    }

    pub(super) fn occlude_chat(&self, occluded: bool) {
        let mut seed = self.seed.lock();
        let mut st = self.attention.lock();
        let was = st.view.occluded;
        st.view.occluded = occluded;
        if !occluded && was {
            mark_focused_seen(&mut seed, &mut st);
        }
    }

    /// Cancelling the loop's own tab while it runs pauses the workspace.
    pub(super) fn cancel_chat(&self, workspace_id: &str, code: &str) {
        let mut seed = self.seed.lock();
        let Some(ws) = seed.workspace_mut(workspace_id) else {
            return;
        };
        let loop_code = if ws.loop_chat_code.is_empty() {
            "MA"
        } else {
            &ws.loop_chat_code
        };
        if loop_code != code || !run_active(&ws.state) {
            return;
        }
        ws.state = WorkspaceState::Paused;
        ws.phase = "idle".into();
        self.attention
            .lock()
            .pending_events
            .push(AppEvent::WorkspaceUpdated(std::sync::Arc::new(
                WorkspaceUpdated {
                    workspace_id: ws.id.clone(),
                    state: Some(WorkspaceState::Paused),
                    phase: Some("idle".into()),
                    ..Default::default()
                },
            )));
    }

    pub(super) fn read_chat_inbox(&self, id: &str) {
        if let Some((ws, code)) = parse_chat_inbox_id(id) {
            let mut st = self.attention.lock();
            st.view.unseen.remove(&chat_tab_key(&ws, &code));
            st.pending_publish = Some(Publish {
                workspace_id: ws,
                code,
                clear_all_unseen: false,
            });
        }
    }

    pub(super) fn read_all_chat_inbox(&self) {
        let mut st = self.attention.lock();
        st.view.unseen.clear();
        st.pending_publish = Some(Publish {
            workspace_id: String::new(),
            code: String::new(),
            clear_all_unseen: true,
        });
    }

    pub(super) fn live_attention(&self) -> AttentionCounts {
        rollup(&self.seed.lock(), &self.attention.lock().view.unseen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_inbox_ids() {
        assert_eq!(
            parse_chat_inbox_id("ib-chat-LOOP-2229-PA2"),
            Some(("LOOP-2229".into(), "PA2".into()))
        );
        assert_eq!(parse_chat_inbox_id("ib-chat--PA"), None);
        assert_eq!(parse_chat_inbox_id("ib1"), None);
    }
}
