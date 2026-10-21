//! `PermissionPrompts.svelte` (the floating tool-permission card for requests no focused chat
//! is showing) and `QuestionPrompts.svelte` (an ask toast for off-tab `ask_user` questions).

use std::collections::HashSet;

use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, IntoElement, KeystrokeEvent,
    ParentElement, Render, Styled, Subscription, Window, div, px,
};
use recur_domain::{PermissionRequest, UserQuestion};
use recur_ui::widgets::toasts::{ToastAsk, ToastStore, toast_action};
use recur_view_core::nav::ViewName;
use recur_view_core::{AppState, ChatRef};

use crate::cards::permission::{PermissionCard, PermissionCardEvent};

/// The request needs the floating surface: not a chat prompt the focused, on-screen chat shows.
fn needs_float(r: &PermissionRequest, focus: Option<&ChatRef>, on_screen: bool) -> bool {
    if r.surface != "chat" {
        return true;
    }
    match focus {
        Some(f) if on_screen => f.workspace_id != r.workspace_id || f.code != r.agent_code,
        _ => true,
    }
}

/// `current` and `queued`: the oldest floating request and how many more wait.
pub fn floating_permission<'a>(
    requests: &'a [PermissionRequest],
    focus: Option<&ChatRef>,
    on_screen: bool,
) -> Option<(&'a PermissionRequest, usize)> {
    let mut it = requests.iter().filter(|r| needs_float(r, focus, on_screen));
    let current = it.next()?;
    Some((current, it.count()))
}

pub struct PermissionPrompts {
    state: Entity<AppState>,
    card: Option<Entity<PermissionCard>>,
    _subs: Vec<Subscription>,
    card_sub: Option<Subscription>,
}

impl PermissionPrompts {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let me = cx.entity().downgrade();
        let subs = vec![
            cx.observe(&state, |this, _, cx| this.sync(cx)),
            cx.intercept_keystrokes(move |ev: &KeystrokeEvent, _, cx| {
                if ev.keystroke.key != "escape" || ev.keystroke.modifiers.modified() {
                    return;
                }
                let Some(me) = me.upgrade() else { return };
                let Some(id) = me.read(cx).card.as_ref().map(|c| c.read(cx).request.id.clone()) else {
                    return;
                };
                let state = me.read(cx).state.clone();
                state.update(cx, |s, cx| s.answer_permission(&id, false, "denied by the human", false, cx));
            }),
        ];
        let mut this = PermissionPrompts {
            state,
            card: None,
            _subs: subs,
            card_sub: None,
        };
        this.sync(cx);
        this
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let found = floating_permission(&s.permission_requests, s.focused_chat.as_ref(), s.agent_chat_on_screen())
            .map(|(r, q)| (r.clone(), q));
        let Some((req, queued)) = found else {
            if self.card.take().is_some() {
                self.card_sub = None;
                cx.notify();
            }
            return;
        };
        let where_label = s
            .workspaces
            .iter()
            .find(|w| w.id == req.workspace_id)
            .map(|w| w.short.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| req.workspace_id.clone());
        match &self.card {
            Some(card) => card.update(cx, |c, cx| {
                c.set_request(req, cx);
                c.set_where(where_label, queued, cx);
            }),
            None => {
                let state = self.state.clone();
                let card = cx.new(|_| PermissionCard::new(req, false, state));
                card.update(cx, |c, cx| c.set_where(where_label, queued, cx));
                self.card_sub = Some(cx.subscribe(&card, |this, card, _: &PermissionCardEvent, cx| {
                    let ws = card.read(cx).request.workspace_id.clone();
                    this.state.update(cx, |s, cx| {
                        s.selected_workspace_id = ws;
                        s.go(ViewName::Detail, None, cx);
                    });
                }));
                self.card = Some(card);
            }
        }
        cx.notify();
    }
}

impl Render for PermissionPrompts {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let Some(card) = self.card.clone() else {
            return div().into_any_element();
        };
        let w = 360f32.min(f32::from(window.viewport_size().width) - 28.);
        div()
            .id("perm-wrap")
            .absolute()
            .right(px(14.))
            .bottom(px(14.))
            .w(px(w))
            .occlude()
            .child(card)
            .into_any_element()
    }
}

/// `truncate(s, n)`: whitespace collapsed, at most `n` chars with a trailing ellipsis.
pub fn truncate(s: &str, n: usize) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.encode_utf16().count() <= n {
        return t;
    }
    let mut out = String::new();
    let mut units = 0;
    for c in t.chars() {
        units += c.len_utf16();
        if units > n.saturating_sub(1) {
            break;
        }
        out.push(c);
    }
    format!("{}…", out.trim_end())
}

/// `askPayload`.
pub fn ask_payload(q: &UserQuestion, state: &AppState) -> ToastAsk {
    let ws = state.workspaces.iter().find(|w| w.id == q.workspace_id);
    let project = ws
        .and_then(|w| state.projects.iter().find(|p| p.id == w.project_id).map(|p| p.name.clone()))
        .filter(|n| !n.is_empty())
        .or_else(|| ws.map(|w| w.project_id.clone()).filter(|p| !p.is_empty()))
        .unwrap_or_else(|| "Project".into());
    let workspace = ws
        .map(|w| if w.short.is_empty() { w.branch.clone() } else { w.short.clone() })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| q.workspace_id.clone());
    ToastAsk {
        project,
        workspace,
        question: truncate(&q.question, 96),
    }
}

/// Renders nothing; toasts each pending question once unless its chat is already in view.
pub struct QuestionPrompts {
    toasted: HashSet<String>,
    _sub: Subscription,
}

impl QuestionPrompts {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let _sub = cx.observe(&state, |this, state, cx| this.sync(&state, cx));
        let mut this = QuestionPrompts {
            toasted: HashSet::new(),
            _sub,
        };
        this.sync(&state, cx);
        this
    }

    fn sync(&mut self, state: &Entity<AppState>, cx: &mut Context<Self>) {
        let s = state.read(cx);
        let on_screen = s.agent_chat_on_screen();
        let focus = s.focused_chat.clone();
        let mut fire = Vec::new();
        for q in &s.user_questions {
            let here = on_screen
                && focus
                    .as_ref()
                    .is_some_and(|f| f.workspace_id == q.workspace_id && f.code == q.agent_code);
            if !self.toasted.insert(q.id.clone()) || here {
                continue;
            }
            fire.push((ask_payload(q, s), q.workspace_id.clone(), q.agent_code.clone()));
        }
        let pending: HashSet<&str> = s.user_questions.iter().map(|q| q.id.as_str()).collect();
        self.toasted.retain(|id| pending.contains(id.as_str()));
        let store = ToastStore::global(cx);
        for (ask, ws, code) in fire {
            let state = state.clone();
            let action = toast_action("Open", move |_, cx| {
                state.update(cx, |s, cx| s.open_workspace_chat(&ws, &code, cx));
            });
            store.update(cx, |t, cx| t.push_ask(ask, action, None, cx));
        }
    }
}

impl Render for QuestionPrompts {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str, surface: &str, ws: &str, code: &str) -> PermissionRequest {
        PermissionRequest {
            id: id.into(),
            surface: surface.into(),
            workspace_id: ws.into(),
            agent_code: code.into(),
            ..Default::default()
        }
    }

    #[test]
    fn floating_skips_the_focused_chat() {
        let reqs = vec![req("a", "chat", "w1", "C1"), req("b", "loop", "w1", "MA"), req("c", "chat", "w2", "C1")];
        let focus = ChatRef { workspace_id: "w1".into(), code: "C1".into() };
        let (cur, queued) = floating_permission(&reqs, Some(&focus), true).unwrap();
        assert_eq!((cur.id.as_str(), queued), ("b", 1));
        let (cur, queued) = floating_permission(&reqs, Some(&focus), false).unwrap();
        assert_eq!((cur.id.as_str(), queued), ("a", 2));
        assert!(floating_permission(&reqs[..1], Some(&focus), true).is_none());
    }

    #[test]
    fn truncates_like_the_toast() {
        assert_eq!(truncate("  a \n b  ", 96), "a b");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("ab  cdef", 4), "ab…");
    }
}
