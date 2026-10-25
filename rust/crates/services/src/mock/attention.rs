//! Chat attention (D-attention) as the mock bridge tracks it per page: which
//! chat tab is focused, whether a dialog occludes it, which tabs finished a
//! turn unseen, plus the wrappers `installBridge` puts around the focus,
//! occlusion, cancel, inbox and attention-count resolvers so e2e can idle a
//! chat tab the way the engine would.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Value, json};

use super::frontend::peel_chat_surface;
use super::js;
use super::{Reply, Resolver, Seed, Table};
use crate::events::{AgentUpdated, AppEvent, AttentionUpdated, InboxUpdated, WorkspaceUpdated};

/// `MockChatAttention`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatAttention {
    pub occluded: bool,
    pub focus_ws: String,
    pub focus_code: String,
    /// `workspaceId::code` of every unseen tab.
    pub unseen: BTreeSet<String>,
}

/// The attention view plus what a call queued for after its response.
#[derive(Default)]
pub(crate) struct State {
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

/// `parseChatInboxID`: the workspace and tab of an `ib-chat-…` inbox row.
pub(crate) fn parse_chat_inbox_id(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix("ib-chat-")?;
    let i = rest.rfind('-')?;
    if i == 0 {
        return None;
    }
    Some((rest[..i].to_owned(), rest[i + 1..].to_owned()))
}

fn upsert_chat_inbox(seed: &mut Seed, workspace_id: &str, code: &str) {
    let id = chat_inbox_id(workspace_id, code);
    let ws = seed.loop_by_id(workspace_id);
    let mut sub = ws
        .map(|w| js::get_str(w, "short"))
        .filter(|s| !s.is_empty())
        .unwrap_or(workspace_id)
        .to_owned();
    let host_pr_id = ws.map(|w| js::get_str(w, "hostPrId").trim()).unwrap_or("");
    if !host_pr_id.is_empty() {
        let pr = seed
            .pr_by_id(host_pr_id)
            .or_else(|| seed.prs().find(|p| js::get_str(p, "id") == host_pr_id));
        if let Some(pr) = pr {
            let title = js::get_str(pr, "title");
            let number = js::get(pr, "number").map_or("undefined".into(), js::string);
            let shown = if title.is_empty() {
                sub.clone()
            } else {
                title.to_owned()
            };
            sub = format!("#{number} · {shown}");
        }
    }
    let row = json!({
        "id": id,
        "group": "ready",
        "unread": true,
        "kind": "chat",
        "color": "var(--cyan)",
        "title": format!("<b>{}</b> finished a reply", if code == "MA" { "Loop" } else { "Chat" }),
        "sub": sub,
        "time": "now",
        "action": "View",
        "actionClass": "go",
        "target": format!("chat:{workspace_id}:{code}"),
        "workspaceId": workspace_id,
    });
    match seed.inbox.iter().position(|it| js::get_str(it, "id") == id) {
        Some(i) => seed.inbox[i] = row,
        None => seed.inbox.insert(0, row),
    }
}

fn remove_chat_inbox(seed: &mut Seed, workspace_id: &str, code: &str) -> bool {
    let id = chat_inbox_id(workspace_id, code);
    let before = seed.inbox.len();
    seed.inbox.retain(|it| js::get_str(it, "id") != id);
    seed.inbox.len() != before
}

/// `attentionRollup`: counts as the engine's `AttentionCounts` (no app tab).
pub(crate) fn attention_rollup(seed: &Seed, unseen: &BTreeSet<String>) -> Value {
    let mut by_workspace: BTreeMap<String, i64> = BTreeMap::new();
    let mut by_project: BTreeMap<String, i64> = BTreeMap::new();
    for key in unseen {
        let ws_id = key.find("::").map_or(key.as_str(), |i| &key[..i]);
        *by_workspace.entry(ws_id.to_owned()).or_default() += 1;
        let ws = seed.loop_by_id(ws_id);
        if ws.is_some_and(|w| js::truthy(js::get(w, "hostPrId"))) {
            continue;
        }
        if let Some(proj) = ws
            .map(|w| js::get_str(w, "projectId"))
            .filter(|p| !p.is_empty())
        {
            *by_project.entry(proj.to_owned()).or_default() += 1;
        }
    }
    let n = unseen.len() as i64;
    let unread_kind = |chat: bool| {
        seed.inbox
            .iter()
            .filter(|it| {
                js::truthy(js::get(it, "unread")) && (js::get_str(it, "kind") == "chat") == chat
            })
            .count() as i64
    };
    let chat_unread = unread_kind(true);
    let other_unread = unread_kind(false);
    json!({
        "unseen": n,
        "inboxUnread": other_unread + chat_unread,
        "dock": n + other_unread,
        "byWorkspace": by_workspace,
        "byProject": by_project,
    })
}

fn attention_updated(
    seed: &Seed,
    view: &ChatAttention,
    workspace_id: &str,
    code: &str,
    tab_unseen: bool,
    clear_all_unseen: bool,
) -> AppEvent {
    let counts = attention_rollup(seed, &view.unseen);
    AppEvent::AttentionUpdated(AttentionUpdated {
        counts: serde_json::from_value(counts).unwrap_or_default(),
        workspace_id: workspace_id.to_owned(),
        code: code.to_owned(),
        tab_unseen,
        clear_all_unseen,
    })
}

/// `simulateChatTurnIdle`.
pub(crate) fn turn_idle(
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

/// What a successful call queued, in dispatch order.
pub(crate) fn take_pending(seed: &Seed, st: &mut State) -> Vec<AppEvent> {
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

fn inner(inner: &Option<Resolver>, args: &[Value], seed: &mut Seed, table: &Table) -> Reply {
    match inner {
        Some(f) => f(args, seed, table),
        None => Reply::null(),
    }
}

const RUNNING_STATES: &[&str] = &["queued", "planning", "coding", "reviewing", "verifying"];

/// Wraps the effective resolvers, as `installBridge` does after merging the
/// test's overrides.
pub(crate) fn install(table: &mut Table, attn: &Arc<Mutex<State>>) {
    let wrap = |table: &mut Table, name: &str, f: Box<dyn Fn(Option<Resolver>) -> Resolver>| {
        let prev = table.get(name).cloned();
        table.resolvers.insert(name.to_owned(), f(prev));
    };

    let a = attn.clone();
    wrap(
        table,
        "SetWorkspaceChatFocus",
        Box::new(move |prev| {
            let a = a.clone();
            Arc::new(move |args, seed, t| {
                {
                    let mut st = a.lock();
                    st.view.focus_ws = js::str_arg(args, 0);
                    st.view.focus_code = js::str_arg(args, 1);
                    mark_focused_seen(seed, &mut st);
                }
                inner(&prev, args, seed, t)
            })
        }),
    );

    wrap(
        table,
        "SendWorkspaceMessage",
        Box::new(|prev| {
            Arc::new(move |args, seed, t| {
                let mut next = args.to_vec();
                if let Some(Value::String(text)) = next.get(2) {
                    next[2] = Value::String(peel_chat_surface(text));
                }
                inner(&prev, &next, seed, t)
            })
        }),
    );

    let a = attn.clone();
    wrap(
        table,
        "CancelWorkspaceMessage",
        Box::new(move |prev| {
            let a = a.clone();
            Arc::new(move |args, seed, t| {
                let ws_id = js::str_arg(args, 0);
                let hit = seed.loop_by_id(&ws_id).and_then(|ws| {
                    let loop_code = js::or(js::get(ws, "loopChatCode"), &json!("MA")).clone();
                    let state = js::get_str(ws, "state");
                    (Some(&loop_code) == args.get(1) && RUNNING_STATES.contains(&state))
                        .then(|| js::get_str(ws, "id").to_owned())
                });
                if let Some(id) = hit {
                    let paused = |v: &mut Value| {
                        js::set(v, "state", "paused".into());
                        js::set(v, "phase", "idle".into());
                    };
                    if let Some(ws) = seed.loop_by_id_mut(&ws_id) {
                        paused(ws);
                    }
                    if let Some(row) = seed.find_loop(|l| js::get_str(l, "id") == id) {
                        paused(seed.obj_mut(row));
                    }
                    a.lock()
                        .pending_events
                        .push(AppEvent::WorkspaceUpdated(std::sync::Arc::new(
                            WorkspaceUpdated {
                                workspace_id: id,
                                state: Some("paused".into()),
                                phase: Some("idle".into()),
                                ..Default::default()
                            },
                        )));
                }
                inner(&prev, args, seed, t)
            })
        }),
    );

    let a = attn.clone();
    wrap(
        table,
        "SetChatUIOccluded",
        Box::new(move |prev| {
            let a = a.clone();
            Arc::new(move |args, seed, t| {
                {
                    let mut st = a.lock();
                    let was = st.view.occluded;
                    st.view.occluded = js::truthy(args.first());
                    if !st.view.occluded && was {
                        mark_focused_seen(seed, &mut st);
                    }
                }
                inner(&prev, args, seed, t)
            })
        }),
    );

    let a = attn.clone();
    wrap(
        table,
        "MarkInboxRead",
        Box::new(move |prev| {
            let a = a.clone();
            Arc::new(move |args, seed, t| {
                if let Some((ws, code)) = parse_chat_inbox_id(&js::str_arg(args, 0)) {
                    let mut st = a.lock();
                    st.view.unseen.remove(&chat_tab_key(&ws, &code));
                    st.pending_publish = Some(Publish {
                        workspace_id: ws,
                        code,
                        clear_all_unseen: false,
                    });
                }
                inner(&prev, args, seed, t)
            })
        }),
    );

    let a = attn.clone();
    wrap(
        table,
        "MarkAllInboxRead",
        Box::new(move |prev| {
            let a = a.clone();
            Arc::new(move |args, seed, t| {
                {
                    let mut st = a.lock();
                    st.view.unseen.clear();
                    st.pending_publish = Some(Publish {
                        workspace_id: String::new(),
                        code: String::new(),
                        clear_all_unseen: true,
                    });
                }
                inner(&prev, args, seed, t)
            })
        }),
    );

    let a = attn.clone();
    wrap(
        table,
        "AttentionCounts",
        Box::new(move |prev| {
            let a = a.clone();
            Arc::new(move |args, seed, t| {
                let base = inner(&prev, args, seed, t).embedded();
                let base = if base.is_null() { json!({}) } else { base };
                let live = attention_rollup(seed, &a.lock().view.unseen);
                let mut out = js::spread(&[&base, &live]);
                for key in ["byWorkspace", "byProject"] {
                    let merged = js::spread(&[
                        js::nullish(js::get(&base, key), &json!({})),
                        js::nullish(js::get(&live, key), &json!({})),
                    ]);
                    js::set(&mut out, key, merged);
                }
                Reply::Value(out)
            })
        }),
    );
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
