//! `SessionCard.svelte`: a specialist row in the Loop cockpit's fan-out card (an in-workspace
//! run, not a child worktree). Live sessions start open and stream inside a read-only chat.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    div, px,
};
use recur_domain::Finding;
use recur_engine::view::chat::AgentMsg;
use recur_engine::view::loop_cockpit::specialist_mode;
use recur_ui::guard::Guarded;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, Tokens, icon};
use recur_view_core::ViewContext;
use recur_view_core::modes::ChatMode;

use crate::cards::plan::running_dot;
use crate::chat::{AgentChat, ChatEvent, ChatHooks, ChatProps};
use crate::html_inline::{InlineStyle, inline_html};
use crate::style::hsla;

/// The props.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionCardProps {
    pub code: String,
    pub name: String,
    pub messages: Vec<AgentMsg>,
    /// Streaming now: render expanded.
    pub live: bool,
    pub presentation_active: bool,
    pub root_path: String,
    pub workspace_id: String,
    pub findings: Vec<Finding>,
    /// The host opens files (`onOpenFile`).
    pub open_file: bool,
    /// The first row of its card (`.fanout-row:first-of-type` has no top rule).
    pub first: bool,
    /// The host chat's inherited font size ([`ChatProps::font_size`]).
    pub font_size: f32,
}

pub struct SessionCard {
    vcx: ViewContext,
    pub(crate) props: SessionCardProps,
    expanded_pref: Option<bool>,
    pub(crate) chat: Option<Entity<AgentChat>>,
    _sub: Option<Subscription>,
}

impl EventEmitter<ChatEvent> for SessionCard {}

/// `lastMeta`: the last prose row (html for engine notices).
pub fn last_meta(messages: &[AgentMsg]) -> Option<(String, bool)> {
    messages.iter().rev().find_map(|m| {
        if m.me
            || !m.tool.is_empty()
            || m.thinking
            || m.compact
            || m.turn_end
            || m.stopped
            || m.error
            || m.checkpoint
        {
            return None;
        }
        let t = recur_engine::view::js::trim(&m.body);
        (!t.is_empty()).then(|| (t.to_string(), m.note_html))
    })
}

/// `.fanout-row`'s step slot: chips, swapped for the chevron on hover of an expandable row.
pub(crate) fn steps_slot(
    group: &SharedString,
    chips: AnyElement,
    expandable: bool,
    expanded: bool,
    t: &Tokens,
) -> AnyElement {
    let mut slot = div().relative().flex().items_center().child(
        div()
            .flex()
            .gap(px(4.))
            .when(expandable, |d| {
                d.group_hover(group.clone(), |s| s.invisible())
            })
            .child(chips),
    );
    if expandable {
        slot = slot.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .flex()
                .items_center()
                .invisible()
                .group_hover(group.clone(), |s| s.visible())
                .child(
                    icon(IconName::ChevronDown)
                        .size(px(14.))
                        .stroke_width(2.4)
                        .rotate(if expanded { 0. } else { -90. })
                        .color(hsla(t.muted)),
                ),
        );
    }
    slot.into_any_element()
}

impl SessionCard {
    pub fn new(vcx: ViewContext, props: SessionCardProps) -> Self {
        SessionCard {
            vcx,
            props,
            expanded_pref: None,
            chat: None,
            _sub: None,
        }
    }

    pub fn set_props(&mut self, props: SessionCardProps, cx: &mut Context<Self>) {
        self.props = props;
        if let Some(chat) = self.chat.clone() {
            let p = self.chat_props();
            chat.update(cx, |c, cx| c.set_props(p, cx));
        }
        cx.notify();
    }

    pub(crate) fn toggle(&mut self, cx: &mut Context<Self>) {
        self.expanded_pref = Some(!self.expanded());
        cx.notify();
    }

    pub(crate) fn expanded(&self) -> bool {
        self.expanded_pref.unwrap_or(self.props.live)
    }

    fn chat_props(&self) -> ChatProps {
        ChatProps {
            messages: self.props.messages.clone(),
            readonly: true,
            sending: self.props.live,
            presentation_active: self.props.presentation_active,
            root_path: self.props.root_path.clone(),
            workspace_id: self.props.workspace_id.clone(),
            findings: self.props.findings.clone(),
            empty_text: "No output yet.".into(),
            session_body: true,
            font_size: self.props.font_size,
            ..ChatProps::default()
        }
    }

    fn ensure_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<AgentChat> {
        if let Some(c) = &self.chat {
            return c.clone();
        }
        let props = self.chat_props();
        let vcx = self.vcx.clone();
        let open_file = self.props.open_file;
        let chat = cx.new(|cx| {
            let mut c = AgentChat::new(&vcx, props, window, cx);
            c.set_hooks(
                ChatHooks {
                    open_file,
                    ..Default::default()
                },
                cx,
            );
            c
        });
        self._sub =
            Some(cx.guarded_subscribe(&chat, |_, _, ev: &ChatEvent, cx| cx.emit(ev.clone())));
        self.chat = Some(chat.clone());
        chat
    }
}

impl SessionCard {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let expanded = self.expanded();
        let live = self.props.live;
        let id = SharedString::from(format!(
            "sess-{}-{}",
            self.props.workspace_id, self.props.code
        ));
        let group = SharedString::from(format!("{id}-g"));
        let mode = ChatMode::from_key(specialist_mode(&self.props.code));
        let chips = recur_ui::tooltip::titled(
            SharedString::from(format!("{id}-mode")),
            mode.label(),
            div()
                .size(px(14.))
                .flex()
                .child(icon(mode.icon()).size(px(14.)).color(hsla(mode.tint(&t)))),
        )
        .into_any_element();
        let name = if self.props.name.is_empty() {
            self.props.code.clone()
        } else {
            self.props.name.clone()
        };
        let mut name_el = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div().flex_none().child(
                    text(name)
                        .color(t.text)
                        .line_height(LineHeight::Normal)
                        .nowrap(),
                ),
            );
        if !live
            && !expanded
            && let Some((meta, html)) = last_meta(&self.props.messages)
        {
            let el: AnyElement = if html {
                let mut st = InlineStyle::meta(11., t.faint, &t);
                st.pre_wrap = true;
                div()
                    .overflow_hidden()
                    .child(inline_html(
                        SharedString::from(format!("{id}-meta")),
                        &meta,
                        st,
                    ))
                    .into_any_element()
            } else {
                text(meta)
                    .size(11.)
                    .color(t.faint)
                    .line_height(LineHeight::Normal)
                    .truncate()
                    .into_any_element()
            };
            let code = self.props.code.clone();
            name_el = name_el.child(
                div()
                    .debug_selector(move || format!("sess-meta:{code}"))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(el),
            );
        }
        let state = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(11.))
            .text_color(hsla(t.muted))
            .when(live, |d| {
                let code = self.props.code.clone();
                d.debug_selector(move || format!("sess-live:{code}")).child(
                    div()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(running_dot(&t)),
                )
            })
            .child(
                text(if live { "working" } else { "finished" })
                    .line_height(LineHeight::Normal)
                    .nowrap(),
            );
        let code = self.props.code.clone();
        let row = div()
            .id(id.clone())
            .debug_selector(|| {
                format!(
                    "sess-head:{code}:{}",
                    if expanded { "open" } else { "closed" }
                )
            })
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .px(px(10.))
            .py(px(5.))
            .text_size(px(12.))
            .when(!self.props.first, |d| {
                d.border_t_1().border_color(hsla(t.line))
            })
            .cursor_pointer()
            .hover(|s| s.bg(hsla(t.pane_2)))
            .child(steps_slot(&group, chips, true, expanded, &t))
            .child(name_el)
            .child(state)
            .on_click(cx.guarded_listener(|c, _, _, cx| c.toggle(cx)));
        let mut out = div().flex().flex_col().child(row);
        if expanded {
            let chat = self.ensure_chat(window, cx);
            out = out.child(
                div()
                    .debug_selector(|| "fanout-transcript".into())
                    .relative()
                    .pt(px(1.))
                    .child(recur_ui::style::dashed_top_border(1., t.line))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        // `.fanout-transcript .fanout-body` drops `.sess-body`'s card.
                        div()
                            .max_h(px(420.))
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .child(chat),
                    ),
            );
        }
        out
    }
}

impl Render for SessionCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_meta_skips_tools_and_footers() {
        let msgs = vec![
            AgentMsg {
                body: "first".into(),
                ..Default::default()
            },
            AgentMsg {
                tool: "Bash".into(),
                body: "x".into(),
                ..Default::default()
            },
            AgentMsg {
                turn_end: true,
                ..Default::default()
            },
            AgentMsg {
                body: "  ".into(),
                ..Default::default()
            },
        ];
        assert_eq!(last_meta(&msgs), Some(("first".into(), false)));
        assert_eq!(last_meta(&[]), None);
    }
}
