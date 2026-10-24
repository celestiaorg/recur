//! The composer dock (`.chat-dock`): queued follow-ups, the input box with its `@` / `/` /
//! prompt-argument menu, the contenteditable composer as a native editor (flow layout,
//! caret, selection, IME), the toolbar slot, the context meter, and Stop / Send.

use recur_ui::guard::Guarded;
use std::cell::Cell;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Bounds, Context, ElementInputHandler, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, fill, point, px, size,
};
use recur_engine::view::artifacts::artifact_icon;
use recur_ui::fonts::Family;
use recur_ui::shadow::with_shadow;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{IconName, Theme, Tokens, icon, motion, rgba};

use crate::chat::{AgentChat, MentionHit, PromptArgHit, SlashHit};
use crate::composer::{COMPOSER_PAD_PX, composer_max_height, composer_min_height};
use crate::flow::{ATOM, FlowStyle, flow};
use crate::style::{INK, hsla, label};

/// The composer's geometry from the last frame: the content box (scrolled) and its height.
#[derive(Default)]
pub(crate) struct ComposerGeom {
    pub content: Cell<Option<Bounds<Pixels>>>,
}

fn mention_icon(name: IconName, color: recur_ui::Color) -> AnyElement {
    div()
        .flex_none()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(name).size(px(14.)).color(hsla(color)))
        .into_any_element()
}

impl AgentChat {
    /// The composer text style: 12.5 / 1.4 (14 / 1.55 in the New-workspace modal).
    fn composer_style(&self, t: &Tokens) -> FlowStyle {
        let (size, leading) = if self.props.composer_only {
            (14., 1.55)
        } else {
            (12.5, 1.4)
        };
        FlowStyle {
            family: Family::Ui,
            size,
            weight: 400.,
            leading,
            color: t.text.into(),
        }
    }

    /// `.chat-dock`.
    pub(crate) fn dock(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.props.readonly {
            return None;
        }
        let t = Theme::of(cx).tokens;
        let composer_only = self.props.composer_only;
        let mut dock = div().flex_none().when(!composer_only, |d| {
            let (x, b) = if self.props.compact {
                (10., 10.)
            } else {
                (16., 12.)
            };
            d.px(px(x)).pb(px(b)).bg(hsla(t.bg))
        });
        let queued: Vec<_> = self.queued_messages().into_iter().cloned().collect();
        if !composer_only && !queued.is_empty() {
            let rows: Vec<AnyElement> = queued
                .iter()
                .enumerate()
                .map(|(qi, m)| self.queued_bubble(m, qi, window, cx))
                .collect();
            dock = dock.child(
                div()
                    .max_w(px(780.))
                    .mx_auto()
                    .mb(px(8.))
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(8.))
                    .children(rows),
            );
        }
        let menu = self.mention_menu(window, cx);
        let composer = self.composer(window, cx);
        let mut chat_in = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(6.))
            .w_full()
            .when(!composer_only, |d| {
                d.max_w(px(780.))
                    .mx_auto()
                    .pt(px(8.))
                    .pr(px(10.))
                    .pb(px(8.))
                    .pl(px(14.))
            })
            .child(composer);
        if !composer_only {
            chat_in = chat_in.child(self.in_row(window, cx));
        }
        if let Some(menu) = menu {
            chat_in = chat_in.child(menu);
        }
        let chat_in = if composer_only {
            chat_in.into_any_element()
        } else {
            // `.chat.drop-file .chat-in`: amber border and a 1px amber ring.
            let drop = self.file_drop_over;
            let mut shadow = vec![];
            if drop {
                shadow.push(recur_ui::shadow::css_box_shadow(0., 0., 0., 1., t.amber));
            }
            shadow.push(recur_ui::shadow::css_box_shadow(
                0.,
                10.,
                28.,
                -12.,
                rgba(0, 0, 0, 0.55),
            ));
            let radius = if self.props.compact { 10. } else { 12. };
            with_shadow(
                recur_ui::style::card(t.pane, if drop { t.amber } else { t.line_bright }, radius)
                    .child(chat_in),
                &shadow,
                radius,
            )
            .max_w(px(780.))
            .mx_auto()
            .w_full()
            .into_any_element()
        };
        Some(dock.child(chat_in).into_any_element())
    }

    /// `.chat-in-row`: the toolbar, the context meter, Stop and Send.
    fn in_row(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        // `.chat-in .send { transition: background .12s ease, color .12s ease }`.
        let ease = |id: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            motion::hover(
                id,
                std::time::Duration::from_millis(120),
                motion::Easing::EASE,
                window,
                cx,
            )
        };
        let meter = self.meter.clone();
        let usage = self.props.context_usage.clone();
        meter.update(cx, |m, cx| m.set_usage(usage, cx));
        let mut actions = div()
            .ml_auto()
            .flex()
            .items_center()
            .gap(px(6.))
            .flex_none()
            .child(meter);
        if self.hooks.stop && (self.props.sending || self.props.can_stop) {
            let h = ease("chat-stop-hover", window, cx);
            actions = actions.child(recur_ui::tooltip::titled(
                "chat-stop-tip",
                "Stop (⌘⇧⌫)",
                div()
                    .id("chat-stop")
                    .debug_selector(|| "chat-stop".into())
                    .size(px(32.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(hsla(t.coral.mix(h.k(), t.text)))
                    .text_color(hsla(INK.mix(h.k(), t.bg)))
                    .cursor_pointer()
                    .on_hover(h.listener())
                    .child(icon(IconName::OctagonX).size(px(15.)))
                    .on_click(cx.guarded_listener(|this, _, _, cx| this.stop(cx))),
            ));
        }
        let can_send = self.can_send();
        let p = &self.props;
        let tip = if p.sending && p.can_steer {
            if p.steer_follow_up {
                "Steer (⏎) · Queue (⌘⏎)"
            } else {
                "Queue (⏎) · Steer (⌘⏎)"
            }
        } else if p.sending {
            "Queue follow-up (⏎)"
        } else if p.can_flush_queue {
            "Send queued (⏎)"
        } else {
            "Send"
        };
        let h = ease("chat-send-hover", window, cx);
        let send = div()
            .id("chat-send")
            .debug_selector(|| {
                if can_send {
                    "chat-send".into()
                } else {
                    "chat-send-disabled".into()
                }
            })
            .relative()
            .size(px(32.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .when(can_send, |d| {
                d.bg(hsla(t.amber.mix(h.k(), t.text)))
                    .text_color(hsla(t.on_amber.mix(h.k(), t.bg)))
                    .cursor_pointer()
                    .on_hover(h.listener())
                    .on_click(
                        cx.guarded_listener(|this, ev: &gpui_kit::ClickEvent, window, cx| {
                            let m = ev.modifiers();
                            this.send(m.platform || m.control, window, cx);
                        }),
                    )
            })
            .when(!can_send, |d| {
                d.bg(hsla(t.pane_2))
                    .text_color(hsla(t.faint))
                    .border_1()
                    .border_color(hsla(t.line))
            })
            .child(icon(IconName::Send).size(px(15.)));
        actions = actions.child(recur_ui::tooltip::titled("chat-send-tip", tip, send));
        let tools = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .children(self.toolbar.clone());
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(tools)
            .child(actions)
            .into_any_element()
    }

    /// The composer box (`.chat-composer`).
    fn composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let style = self.composer_style(&t);
        let composer_only = self.props.composer_only;
        let (pad_x, pad_top, pad_bottom) = if composer_only {
            (18., 18., 12.)
        } else {
            (2., 8., 8.)
        };
        let mut atoms = Vec::new();
        for i in 0..self.doc.chips.len() {
            match self.composer_atom(i, window, cx) {
                Some(a) => atoms.push(a),
                None => atoms.push(crate::flow::Atom {
                    el: div().into_any_element(),
                    width: 0.,
                    height: 0.,
                }),
            }
        }
        let content_h = self
            .composer_layout
            .borrow()
            .as_ref()
            .map_or(style.size * style.leading, |l| {
                l.height.max(style.size * style.leading)
            });
        let height = if composer_only {
            None
        } else {
            Some((content_h + COMPOSER_PAD_PX).clamp(composer_min_height(), composer_max_height()))
        };
        let focused = self.focus.is_focused(window);
        let entity = cx.entity();
        let geom = self.composer_geom.clone();
        let layout = self.composer_layout.clone();
        let doc = self.doc.clone();
        let marked = self.composer_marked.clone();
        let blink_on = focused && !motion::is_frozen(cx) && {
            let e = self.caret_blink_epoch.elapsed().as_millis();
            (e / 500).is_multiple_of(2)
        };
        if focused && !motion::is_frozen(cx) {
            window.request_animation_frame();
        }
        let focus = self.focus.clone();
        let sel_color = t.amber.mix(0.3, recur_ui::color::TRANSPARENT);
        let caret_color: Hsla = t.text.into();
        let overlay = canvas(
            move |b, _, _| geom.content.set(Some(b)),
            move |b, _, window, cx| {
                window.handle_input(&focus, ElementInputHandler::new(b, entity.clone()), cx);
                let Some(l) = layout.borrow().clone() else {
                    return;
                };
                let sel = doc.selection();
                if !sel.is_empty() {
                    for (x, y, w, h) in l.rects(sel.clone()) {
                        window.paint_quad(fill(
                            Bounds::new(b.origin + point(px(x), px(y)), size(px(w), px(h))),
                            hsla(sel_color),
                        ));
                    }
                }
                if let Some(m) = &marked {
                    for (x, y, w, h) in l.rects(m.clone()) {
                        window.paint_quad(fill(
                            Bounds::new(
                                b.origin + point(px(x), px(y + h - 1.)),
                                size(px(w), px(1.)),
                            ),
                            caret_color,
                        ));
                    }
                }
                if blink_on && sel.is_empty() {
                    let (x, y, h) = l.caret(doc.caret);
                    window.paint_quad(fill(
                        Bounds::new(b.origin + point(px(x), px(y)), size(px(1.), px(h))),
                        caret_color,
                    ));
                }
            },
        )
        .absolute()
        .inset_0();
        let body = div()
            .relative()
            .w_full()
            .min_h(px(style.size * style.leading))
            .child(
                flow(
                    SharedString::from(format!("composer-{}", self.props.reset_key)),
                    self.doc.text.clone(),
                    atoms,
                    style.clone(),
                )
                .layout_into(self.composer_layout.clone())
                .min_height(style.size * style.leading),
            )
            .child(overlay);
        let empty = self.composer_empty() && self.doc.text.chars().all(|c| c != ATOM);
        let placeholder = empty.then(|| {
            div()
                .absolute()
                .top(px(pad_top))
                .left(px(pad_x))
                .right(px(pad_x))
                .child(
                    text(self.props.placeholder.clone())
                        .id(SharedString::from("composer-ph"))
                        .size(style.size)
                        .line_height(LineHeight::Scale(style.leading))
                        .color(t.faint),
                )
        });
        let mut el = div()
            .id(SharedString::from(format!(
                "composer-box-{}",
                self.props.reset_key
            )))
            .debug_selector(|| "chat-composer".into())
            .key_context("ChatComposer")
            .track_focus(&self.focus)
            .relative()
            .w_full()
            .flex_none()
            .when_some(height, |d, h| d.h(px(h)))
            .when(composer_only, |d| d.min_h(px(148.)).max_h(px(240.)))
            .overflow_y_scroll()
            .track_scroll(&self.composer_scroll)
            .pl(px(pad_x))
            .pr(px(pad_x))
            .pt(px(pad_top))
            .pb(px(pad_bottom))
            .cursor(gpui_kit::CursorStyle::IBeam)
            .text_color(hsla(t.text))
            .on_key_down(
                cx.guarded_listener(|this, ev: &gpui_kit::KeyDownEvent, window, cx| {
                    this.on_composer_key(ev, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.composer_press(ev, window, cx)
                }),
            )
            .on_mouse_move(cx.guarded_listener(|this, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button == Some(MouseButton::Left) {
                    let b = this.composer_byte_at(ev.position);
                    this.composer_mouse_drag(b, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.guarded_listener(|this, _, _, cx| {
                    this.selecting = false;
                    this.sync_mention_from_caret(cx);
                }),
            )
            .child(body);
        if let Some(p) = placeholder {
            el = el.child(p);
        }
        el.into_any_element()
    }

    pub(crate) fn composer_byte_at(&self, p: Point<Pixels>) -> usize {
        let Some(b) = self.composer_geom.content.get() else {
            return self.doc.text.len();
        };
        let Some(l) = self.composer_layout.borrow().clone() else {
            return self.doc.text.len();
        };
        l.hit(f32::from(p.x - b.origin.x), f32::from(p.y - b.origin.y))
    }

    fn composer_press(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.composer_byte_at(ev.position);
        match ev.click_count {
            2 => {
                self.doc.move_to(b, false);
                self.doc.move_left(false, true);
                let s = self.doc.caret;
                self.doc.move_right(true, true);
                self.doc.anchor = s;
                self.focus_composer(window, cx);
            }
            n if n >= 3 => {
                let s = self.doc.line_start(b);
                let e = self.doc.line_end(b);
                self.doc.anchor = s;
                self.doc.caret = e;
                self.focus_composer(window, cx);
            }
            _ => self.composer_mouse_down(b, ev.modifiers.shift, window, cx),
        }
        self.notify_dock(cx);
    }

    // ---- the menu ----

    fn menu_row(
        &self,
        i: usize,
        on: bool,
        disabled: bool,
        kids: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = Theme::of(cx).tokens;
        div()
            .id(SharedString::from(format!("menu-row-{i}")))
            .debug_selector(|| format!("menu-row{}:{i}", if on { "-on" } else { "" }))
            .flex()
            .items_center()
            .gap(px(8.))
            .w_full()
            .px(px(8.))
            .py(px(7.))
            .rounded(px(7.))
            .text_size(px(12.))
            .text_color(hsla(t.text))
            .when(on, |d| d.bg(hsla(t.pane_2)))
            .when(disabled, |d| d.opacity(0.5))
            .when(!disabled, |d| d.cursor_pointer())
            .children(kids)
            .on_mouse_move(
                cx.guarded_listener(move |this, ev: &MouseMoveEvent, _, cx| {
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                    if this.hover_gate.allows(x, y) && this.mention_sel != i {
                        this.mention_sel = i;
                        this.notify_dock(cx);
                    }
                }),
            )
            .on_hover(cx.guarded_listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.hover_gate.armed && this.mention_sel != i {
                    this.mention_sel = i;
                    this.notify_dock(cx);
                }
            }))
    }

    fn name_el(s: String, titled: bool) -> AnyElement {
        let sel = format!("mention:{s}");
        let tx = text(s).weight(500.).line_height(LineHeight::Normal);
        if titled {
            div()
                .debug_selector(|| sel)
                .flex_initial()
                .min_w_0()
                .overflow_hidden()
                .child(tx.truncate())
                .into_any_element()
        } else {
            div()
                .debug_selector(|| sel)
                .flex_none()
                .child(tx.nowrap())
                .into_any_element()
        }
    }

    fn path_el(s: String, titled: bool, t: &Tokens) -> AnyElement {
        let tx = text(s)
            .size(11.)
            .color(t.muted)
            .line_height(LineHeight::Normal);
        if titled {
            div()
                .flex_none()
                .ml_auto()
                .pl(px(8.))
                .child(tx.nowrap())
                .into_any_element()
        } else {
            div()
                .ml_auto()
                .min_w_0()
                .overflow_hidden()
                .pl(px(8.))
                .child(tx.truncate())
                .into_any_element()
        }
    }

    fn badge(s: impl Into<SharedString>, t: &Tokens) -> AnyElement {
        let s = s.into();
        let sel = format!("badge:{s}");
        div()
            .debug_selector(|| sel)
            .flex_none()
            .ml(px(6.))
            .px(px(5.))
            .py(px(1.))
            .rounded(px(3.))
            .bg(hsla(t.pane_2))
            .child(
                // A flex item of the row `<button>`: `line-height: normal`.
                label(s, Family::Ui, 10., 600.)
                    .tracking(0.02)
                    .line_height(LineHeight::Normal)
                    .color(t.muted),
            )
            .into_any_element()
    }

    fn empty_row(s: impl Into<SharedString>, t: &Tokens) -> AnyElement {
        div()
            .px(px(8.))
            .py(px(10.))
            .child(text(s).size(12.).color(t.muted))
            .into_any_element()
    }

    /// `.chat-mention`: files / terminals / artifacts, prompt args, or prompts and skills.
    fn mention_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        let rows: Vec<AnyElement> = if let Some(m) = self.mention.clone() {
            let ws_or_proj = !self.props.project_id.is_empty();
            let has_live =
                !self.live_terminals(cx).is_empty() || !self.live_artifacts(cx).is_empty();
            if !(ws_or_proj || !self.mention_hits.is_empty() || self.file_index_loading || has_live)
            {
                return None;
            }
            if self.file_index_loading && self.mention_hits.is_empty() {
                vec![Self::empty_row("Loading files…", &t)]
            } else if self.mention_hits.is_empty() {
                vec![Self::empty_row(
                    if m.query.trim().is_empty() {
                        "Type to filter files"
                    } else {
                        "No matching files, terminals, or artifacts"
                    },
                    &t,
                )]
            } else {
                let hits = self.mention_hits.clone();
                hits.iter()
                    .enumerate()
                    .map(|(i, h)| {
                        let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
                        let dir =
                            |p: &str| p.rfind('/').map(|i| p[..i].to_string()).unwrap_or_default();
                        let kids = match h {
                            MentionHit::Terminal {
                                title,
                                active,
                                exited,
                                ..
                            } => vec![
                                mention_icon(IconName::Terminal, t.muted),
                                Self::name_el(title.clone(), true),
                                Self::path_el(
                                    if *exited {
                                        "Exited terminal"
                                    } else if *active {
                                        "Focused terminal"
                                    } else {
                                        "Terminal"
                                    }
                                    .into(),
                                    true,
                                    &t,
                                ),
                            ],
                            MentionHit::Artifact { title, kind, .. } => vec![
                                mention_icon(
                                    IconName::from_key(artifact_icon(kind))
                                        .unwrap_or(IconName::Grid),
                                    t.muted,
                                ),
                                Self::name_el(title.clone(), true),
                                Self::path_el(
                                    if kind.is_empty() {
                                        "Artifact".into()
                                    } else {
                                        kind.clone()
                                    },
                                    true,
                                    &t,
                                ),
                            ],
                            MentionHit::File {
                                item, dir: true, ..
                            } => vec![
                                div()
                                    .flex_none()
                                    .flex()
                                    .debug_selector(|| "mention-folder".into())
                                    .child(mention_icon(IconName::Folder, t.muted))
                                    .into_any_element(),
                                Self::name_el(base(item), false),
                                Self::path_el(dir(item), false, &t),
                            ],
                            MentionHit::File { item, .. } => vec![
                                div()
                                    .flex_none()
                                    .size(px(14.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(recur_ui::file_icons::file_icon(base(item)))
                                    .into_any_element(),
                                Self::name_el(base(item), false),
                                Self::path_el(dir(item), false, &t),
                            ],
                        };
                        self.menu_row(i, i == self.mention_sel, false, kids, cx)
                            .on_click(cx.guarded_listener(move |this, _, window, cx| {
                                this.pick_mention_hit(i, window, cx)
                            }))
                            .into_any_element()
                    })
                    .collect()
            }
        } else if self.prompt_args.is_some() && !self.prompt_arg_hits(cx).is_empty() {
            let hits = self.prompt_arg_hits(cx);
            let can_invoke = self.prompt_arg_can_invoke(cx);
            let icon_svg = self
                .prompt_arg_info(cx)
                .map(|i| i.icon_svg.trim().to_string())
                .unwrap_or_default();
            let complete_message = self.complete_message.clone();
            hits.iter()
                .enumerate()
                .map(|(i, hit)| {
                    let cyan = t.cyan;
                    let pico = |name: IconName| mention_icon(name, cyan);
                    match hit {
                        PromptArgHit::Suggest(h) => {
                            let ico = if icon_svg.is_empty() {
                                pico(IconName::Hash)
                            } else {
                                div()
                                    .flex_none()
                                    .size(px(14.))
                                    .text_color(hsla(cyan))
                                    .child(recur_ui::svg_markup(icon_svg.clone()).size(px(14.)))
                                    .into_any_element()
                            };
                            let mut kids = vec![ico, Self::name_el(h.label.clone(), false)];
                            if !h.detail.is_empty() {
                                kids.push(Self::path_el(h.detail.clone(), false, &t));
                            }
                            if !h.badge.is_empty() {
                                kids.push(Self::badge(h.badge.clone(), &t));
                            }
                            let hit = h.clone();
                            self.menu_row(i, i == self.mention_sel, false, kids, cx)
                                .on_click(cx.guarded_listener(move |this, _, window, cx| {
                                    this.pick_prompt_complete_hit(&hit, true, window, cx)
                                }))
                                .into_any_element()
                        }
                        PromptArgHit::Run {
                            slash,
                            args,
                            description,
                            ..
                        } => {
                            let name = if args.is_empty() {
                                format!("/{slash}")
                            } else {
                                format!("/{slash} {args}")
                            };
                            let path = if complete_message.is_empty() {
                                description.clone()
                            } else {
                                complete_message.clone()
                            };
                            let kids = vec![
                                pico(IconName::Prompt),
                                Self::name_el(name, false),
                                Self::path_el(path, false, &t),
                                Self::badge("Prompt", &t),
                            ];
                            let (s, a) = (slash.clone(), args.clone());
                            self.menu_row(i, i == self.mention_sel, !can_invoke, kids, cx)
                                .when(can_invoke, |d| {
                                    d.on_click(cx.guarded_listener(move |this, _, window, cx| {
                                        this.apply_prompt_args(&s, &a, true, window, cx)
                                    }))
                                })
                                .into_any_element()
                        }
                    }
                })
                .collect()
        } else if let Some(sm) = self.skill_mention.clone() {
            let loading = self.skill_index_loading || self.prompt_index_loading;
            let any = !self.props.project_id.is_empty()
                || !self.skill_index.is_empty()
                || loading
                || !self.prompt_index.is_empty();
            if !any {
                return None;
            }
            let hits = self.slash_hits(cx);
            if loading && hits.is_empty() {
                vec![Self::empty_row("Loading…", &t)]
            } else if hits.is_empty() {
                vec![Self::empty_row(
                    if sm.query.trim().is_empty() {
                        "Type to filter"
                    } else {
                        "No matching prompts or skills"
                    },
                    &t,
                )]
            } else {
                let busy = self.props.prompt_busy || self.props.sending;
                hits.iter()
                    .enumerate()
                    .map(|(i, h)| match h {
                        SlashHit::Prompt {
                            slash,
                            name,
                            description,
                            args,
                            mode,
                            ..
                        } => {
                            let lp = mode == "loop";
                            let ico = mention_icon(
                                if lp {
                                    recur_view_core::modes::ChatMode::Loop.icon()
                                } else {
                                    IconName::Prompt
                                },
                                if lp { t.amber } else { t.cyan },
                            );
                            let mut kids = vec![ico, Self::name_el(format!("/{slash}"), false)];
                            if !args.is_empty() {
                                kids.push(
                                    div()
                                        .flex()
                                        .flex_none()
                                        .flex_wrap()
                                        .gap(px(4.))
                                        .items_center()
                                        .max_w(gpui_kit::relative(0.42))
                                        .children(args.iter().map(|a| {
                                            recur_ui::style::card(t.pane_2, t.line, 4.)
                                                .px(px(6.))
                                                .py(px(1.))
                                                .child(
                                                    label(a.clone(), Family::Ui, 10., 600.)
                                                        .tracking(0.02)
                                                        .line_height(LineHeight::Normal)
                                                        .color(t.muted),
                                                )
                                        }))
                                        .into_any_element(),
                                );
                            }
                            kids.push(Self::path_el(
                                if description.is_empty() {
                                    name.clone()
                                } else {
                                    description.clone()
                                },
                                false,
                                &t,
                            ));
                            kids.push(Self::badge(if lp { "Loop" } else { "Prompt" }, &t));
                            let s = slash.clone();
                            self.menu_row(i, i == self.mention_sel, busy, kids, cx)
                                .when(!busy, |d| {
                                    d.on_click(cx.guarded_listener(move |this, _, window, cx| {
                                        this.pick_prompt(&s, window, cx)
                                    }))
                                })
                                .into_any_element()
                        }
                        SlashHit::Skill(s) => {
                            let kids = vec![
                                mention_icon(IconName::Slash, t.amber),
                                Self::name_el(s.name.clone(), false),
                                Self::path_el(
                                    if !s.description.is_empty() {
                                        s.description.clone()
                                    } else if s.scope == "user" {
                                        "user".into()
                                    } else {
                                        "workspace".into()
                                    },
                                    false,
                                    &t,
                                ),
                            ];
                            let name = s.name.clone();
                            self.menu_row(i, i == self.mention_sel, false, kids, cx)
                                .on_click(cx.guarded_listener(move |this, _, window, cx| {
                                    this.pick_skill(&name, window, cx)
                                }))
                                .into_any_element()
                        }
                    })
                    .collect()
            }
        } else {
            return None;
        };
        if self.mention_sel < rows.len() {
            self.menu_scroll.scroll_to_item(self.mention_sel);
        }
        // `overflow: auto` with WebKit's classic scrollbar inside the border.
        let panel = recur_ui::style::card(t.pane, t.line_bright, 10.)
            .id("chat-mention")
            .debug_selector(|| "chat-mention".into())
            .max_h(px(220.))
            .flex()
            .flex_col()
            .child(
                recur_ui::widgets::scroll_area("chat-mention-list")
                    .track(&self.menu_scroll)
                    .scroller(|d| d.p(px(4.)).flex().flex_col())
                    .children(rows),
            );
        let panel = with_shadow(panel, &recur_ui::style::SUGGEST_SHADOW, 10.);
        let el = if self.props.composer_only {
            let top = self
                .composer_layout
                .borrow()
                .as_ref()
                .map(|l| {
                    // `composerCaretClientRect`: a collapsed range's rect is the text box (the
                    // font's content area) centered in the 1.55 line, not the line box.
                    let (_, y, h) = l.caret(self.doc.caret);
                    let m = recur_ui::text::strut_metrics(
                        &recur_ui::fonts::Family::Ui,
                        14.,
                        recur_ui::text::LineHeight::Normal,
                        window,
                    );
                    let content = m.ascent + m.descent;
                    18. + y + (h - content) / 2. + content + 4.
                })
                .unwrap_or(18. + 1.55 * 14. + 4.);
            let vh = f32::from(window.viewport_size().height);
            div()
                .absolute()
                .left(px(10.))
                .right(px(10.))
                .top(px(top.round().max(0.)))
                .max_h(px(220f32.min(0.78 * vh - 80.)))
                .child(panel)
        } else {
            div()
                .absolute()
                .left(px(8.))
                .right(px(8.))
                .bottom(gpui_kit::relative(1.))
                .pb(px(4.))
                .child(panel)
        };
        // `z-index: 40`: above the composer, whose border GPUI paints after its children.
        Some(gpui_kit::deferred(el.occlude()).into_any_element())
    }
}
