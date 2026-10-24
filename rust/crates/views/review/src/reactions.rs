//! `CommentReactions.svelte`: a comment's reaction pills and the reaction picker.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Bounds, Context, DispatchPhase, Entity, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Pixels, Render, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, deferred, div, px,
};
use recur_domain::{PrComment, PrReaction, reaction_mine};
use recur_forge::view::{REACTIONS, reaction_icon, reaction_label, visible_reactions};
use recur_ui::guard::Guarded;
use recur_ui::logic::menu_space::{MENU_EDGE_PX, popover_top};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::{Color, IconName, Theme, Tokens, icon, rgba};
use recur_view_core::AppState;

use crate::store::toggle_pr_comment_reaction;
use crate::style::hsla;

const PICK_WIDTH: f32 = 252.;

/// `--rxn` for a reaction (`.rxn-pill[data-rxn]`).
pub fn rxn_color(content: &str, t: &Tokens) -> Color {
    match content {
        "+1" => t.emerald,
        "-1" => t.coral,
        "laugh" => t.warn,
        "hooray" => t.indigo,
        "confused" => t.amber,
        "heart" => t.coral,
        "rocket" => t.cyan,
        "eyes" => t.cyan.mix(0.6, t.emerald),
        _ => t.muted,
    }
}

fn icon_name(key: &str) -> Option<IconName> {
    IconName::from_key(key)
}

pub struct CommentReactions {
    state: Entity<AppState>,
    comment: PrComment,
    can_react: bool,
    open: bool,
    anchor: Option<Bounds<Pixels>>,
    picker: Option<Bounds<Pixels>>,
    trigger: Option<Bounds<Pixels>>,
}

impl CommentReactions {
    pub fn new(state: Entity<AppState>, comment: PrComment, can_react: bool) -> Self {
        CommentReactions {
            state,
            comment,
            can_react,
            open: false,
            anchor: None,
            picker: None,
            trigger: None,
        }
    }

    /// New props (`comment`, `canReact`).
    pub fn set(&mut self, comment: PrComment, can_react: bool, cx: &mut Context<Self>) {
        if self.comment != comment || self.can_react != can_react {
            self.comment = comment;
            self.can_react = can_react;
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.anchor = None;
            self.picker = None;
            cx.notify();
        }
    }

    fn open_picker(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
            return;
        }
        self.anchor = self.trigger;
        self.open = true;
        cx.notify();
    }

    fn react(&mut self, content: &str, cx: &mut Context<Self>) {
        if !self.can_react || self.comment.id.is_empty() {
            return;
        }
        self.close(cx);
        let c = self.comment.clone();
        toggle_pr_comment_reaction(&self.state, &c, content, cx).detach();
    }

    fn pill(&self, r: &PrReaction, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let rx = rxn_color(&r.content, &t);
        let label = reaction_label(&r.content);
        let (bg, border) = if r.mine {
            (rx.mix(0.22, t.pane), rx.mix(0.55, t.line))
        } else {
            (rx.mix(0.12, t.pane_2), rx.mix(0.28, t.line))
        };
        let (hover_bg, hover_border) = (rx.mix(0.18, t.pane_2), rx.mix(0.48, t.line));
        let title = if r.mine {
            format!("Remove {label}")
        } else {
            format!("React with {label}")
        };
        let content = r.content.clone();
        let id = SharedString::from(format!("rxn-{}-{}", self.comment.id, r.content));
        let can = self.can_react;
        let mut pill = div()
            .id(id.clone())
            .debug_selector(|| format!("rxn-pill:{}:{}", self.comment.id, r.content))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .h(px(24.))
            .px(px(7.))
            .rounded(px(999.))
            .border_1()
            .border_color(hsla(border))
            .bg(hsla(bg))
            .text_color(hsla(rx))
            .when_some(reaction_icon(&r.content).and_then(icon_name), |d, name| {
                d.child(icon(name).size(px(13.)))
            })
            .child(
                text(r.count.to_string())
                    .size(12.)
                    .line_height(LineHeight::Px(12.))
                    .tabular()
                    .nowrap(),
            );
        if can {
            pill = pill
                .cursor_pointer()
                .hover(move |s| s.bg(hsla(hover_bg)).border_color(hsla(hover_border)))
                .on_click(cx.guarded_listener(move |c, _, _, cx| c.react(&content, cx)));
        }
        titled(SharedString::from(format!("{id}-t{i}")), title, pill).into_any_element()
    }

    fn picker_el(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.open {
            return None;
        }
        let r = self.anchor?;
        let t = Theme::of(cx).tokens;
        let vw = f32::from(window.viewport_size().width);
        let vh = f32::from(window.viewport_size().height);
        let left = f32::from(r.origin.x).min(MENU_EDGE_PX.max(vw - PICK_WIDTH - MENU_EDGE_PX));
        let top = popover_top(
            Some(f32::from(r.origin.y)),
            f32::from(r.bottom()),
            self.picker.map_or(0., |b| f32::from(b.size.height)),
            vh,
        );
        let mine = self.comment.reactions.clone();
        let mut row = div().flex().gap(px(2.));
        for (content, key, label) in REACTIONS {
            let is_mine = reaction_mine(&mine, content);
            let fg = rxn_color(content, &t);
            let item = div()
                .id(SharedString::from(format!(
                    "rxn-pick-{}-{content}",
                    self.comment.id
                )))
                .debug_selector(|| format!("rxn-pick:{content}"))
                .flex()
                .items_center()
                .justify_center()
                .w(px(30.))
                .h(px(28.))
                .rounded(px(7.))
                .cursor_pointer()
                .text_color(hsla(fg))
                .when(is_mine, |d| d.bg(hsla(t.pane_2)))
                .hover(|s| s.bg(hsla(t.pane_2)))
                .when_some(icon_name(key), |d, n| d.child(icon(n).size(px(16.))))
                .on_click(cx.guarded_listener(move |c, _, _, cx| c.react(content, cx)));
            row = row.child(titled(
                SharedString::from(format!("rxn-pick-t-{}-{content}", self.comment.id)),
                label,
                item,
            ));
        }
        let me = cx.entity().downgrade();
        let probe = canvas(
            move |b, _, cx| {
                if let Some(me) = me.upgrade() {
                    me.update(cx, |c, cx| {
                        if c.picker.map(|p| p.size) != Some(b.size) {
                            c.picker = Some(b);
                            cx.notify();
                        } else {
                            c.picker = Some(b);
                        }
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let card = recur_ui::style::card(t.pane, t.line, 10.)
            .p(px(4.))
            .child(row)
            .child(probe);
        let panel = with_shadow(
            card,
            &[css_box_shadow(0., 8., 24., 0., rgba(0, 0, 0, 0.28))],
            10.,
        );
        Some(
            deferred(recur_ui::style::anchored_at(left, top, panel))
                .with_priority(40)
                .into_any_element(),
        )
    }
}

impl CommentReactions {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let pills = visible_reactions(&self.comment.reactions);
        let show = !self.comment.id.is_empty() && (!pills.is_empty() || self.can_react);
        if !show {
            return div().into_any_element();
        }
        if self.open && self.state.read(cx).app_overlay_open() {
            self.close(cx);
        }
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .mt(px(8.));
        if self.can_react {
            let me = cx.entity().downgrade();
            let trigger_probe = canvas(
                move |b, _, cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |c, _| c.trigger = Some(b));
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0();
            let add = div()
                .id(SharedString::from(format!("rxn-add-{}", self.comment.id)))
                .debug_selector(|| format!("rxn-add:{}", self.comment.id))
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(24.))
                .h(px(24.))
                .rounded(px(999.))
                .border_1()
                .border_color(hsla(t.line))
                .bg(hsla(t.pane_2))
                .text_color(hsla(t.muted))
                .cursor_pointer()
                .hover(|s| s.border_color(hsla(t.line_bright)).text_color(hsla(t.text)))
                .child(icon(IconName::Smile).size(px(14.)))
                .child(trigger_probe)
                .on_click(cx.guarded_listener(|c, _, _, cx| {
                    cx.stop_propagation();
                    c.open_picker(cx)
                }));
            row = row.child(titled(
                SharedString::from(format!("rxn-add-t-{}", self.comment.id)),
                "Add reaction",
                add,
            ));
        }
        for (i, r) in pills.iter().enumerate() {
            row = row.child(self.pill(r, i, cx));
        }
        let open = self.open;
        let picker = self.picker_el(window, cx);
        let me = cx.entity().downgrade();
        let listeners = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                if !open {
                    return;
                }
                let m1 = me.clone();
                window.on_mouse_event(move |e: &MouseDownEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture
                        || e.button != MouseButton::Left && e.button != MouseButton::Right
                    {
                        return;
                    }
                    if let Some(me) = m1.upgrade() {
                        me.update(cx, |c, cx| {
                            if c.picker.is_some_and(|p| p.contains(&e.position)) {
                                return;
                            }
                            c.close(cx);
                        });
                    }
                });
                let m2 = me.clone();
                window.on_mouse_event(move |_: &ScrollWheelEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    if let Some(me) = m2.upgrade() {
                        me.update(cx, |c, cx| c.close(cx));
                    }
                });
                let m3 = me.clone();
                window.on_key_event(move |e: &KeyDownEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || e.keystroke.key != "escape" {
                        return;
                    }
                    if let Some(me) = m3.upgrade() {
                        me.update(cx, |c, cx| {
                            if c.open {
                                cx.stop_propagation();
                                c.close(cx);
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_0();
        div()
            .child(row)
            .child(listeners)
            .children(picker)
            .into_any_element()
    }
}

impl Render for CommentReactions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The visible reactions in picker order.
pub fn pills(c: &PrComment) -> Vec<PrReaction> {
    visible_reactions(&c.reactions)
}
