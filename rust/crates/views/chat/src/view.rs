//! `AgentChat`'s root (`<div class="chat">`): the scrolling transcript with its edge fades,
//! the fork menu, the dock, the lightbox and image menu, drops, and the window-level keys
//! the Svelte component listened for.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, DragMoveEvent, ExternalPaths, InteractiveElement, IntoElement,
    KeystrokeEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, canvas, div, linear_color_stop, linear_gradient, px,
};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, text};
use recur_ui::{IconName, Theme, icon, rgba};
use recur_view_core::file_drag::WorkspaceFileDrag;

use crate::chat::{AgentChat, ChatEvent};
use crate::style::hsla;
use crate::transcript_ui::NEAR_BOTTOM_PX;

impl AgentChat {
    /// The window-level keys (`svelte:window onkeydown` and the effects' `keydown`
    /// listeners), plus the keys a focused prompt slot hands the chat.
    pub(crate) fn intercept_keys(cx: &mut Context<Self>) -> Subscription {
        let me = cx.entity().downgrade();
        cx.intercept_keystrokes(move |ev: &KeystrokeEvent, window, cx| {
            let Some(chat) = me.upgrade() else { return };
            let k = &ev.keystroke;
            let m = k.modifiers;
            // A focused prompt slot: Backspace at an empty start, menu keys, ⌘.
            if chat.read(cx).focused_slot(window, cx).is_some() {
                let kd = gpui_kit::KeyDownEvent {
                    keystroke: k.clone(),
                    is_held: false,
                    prefer_character_input: false,
                };
                let handled = chat.update(cx, |c, cx| c.on_slot_key(&kd, window, cx));
                if handled {
                    cx.stop_propagation();
                    return;
                }
            }
            if k.key == "escape" && !m.modified() {
                let c = chat.read(cx);
                if c.fork_menu.is_some() {
                    chat.update(cx, |c, cx| {
                        c.fork_menu = None;
                        cx.notify();
                    });
                    cx.stop_propagation();
                    return;
                }
                if c.meter.read(cx).is_open() {
                    chat.update(cx, |c, cx| c.meter.update(cx, |m, cx| m.close(cx)));
                    cx.stop_propagation();
                    return;
                }
                let menu = c.menu_open() && c.focus.is_focused(window);
                let composing = !c.digest_comment_for.is_empty();
                if !menu
                    && !composing
                    && let Some(req) = c.chat_permissions(cx).first().cloned()
                {
                    let state = c.state.clone();
                    state.update(cx, |s, cx| {
                        s.answer_permission(&req.id, false, "denied by the human", false, cx)
                    });
                    cx.stop_propagation();
                }
                return;
            }
            // ⌘⇧⌫ / Ctrl+Shift+Backspace: stop, only while focus is inside the chat.
            if k.key == "backspace" && m.shift && !m.alt && (m.platform || m.control) {
                let c = chat.read(cx);
                let live =
                    c.props.shell_active && (c.props.sending || c.props.can_stop) && c.hooks.stop;
                let inside = c.focus.contains_focused(window, cx)
                    || c.focus.is_focused(window)
                    || c.focused_slot(window, cx).is_some();
                if live && inside {
                    chat.update(cx, |c, cx| c.stop(cx));
                    cx.stop_propagation();
                }
            }
        })
    }

    /// The scroller's top / bottom fades (`fadeScrollEdges` over `.chat-scroll`'s mask): a
    /// `--bg` gradient over the opaque chat background equals the mask.
    fn fades(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let t = Theme::of(cx).tokens;
        let bg = self.backdrop(&t);
        let fade = if self.props.session_body {
            20.
        } else if self.props.compact {
            32.
        } else {
            40.
        };
        let top = -f32::from(self.scroll.offset().y);
        let max = f32::from(self.scroll.max_offset().y).max(0.);
        let show_top = !self.fade_ready || top > 1.;
        let show_bot = self.fade_ready && max > NEAR_BOTTOM_PX && top < max - NEAR_BOTTOM_PX;
        let grad = |from: f32, to: f32| {
            linear_gradient(
                180.,
                linear_color_stop(hsla(bg.with_alpha(from)), 0.),
                linear_color_stop(hsla(bg.with_alpha(to)), 1.),
            )
        };
        let mut out = Vec::new();
        if show_top {
            out.push(
                div()
                    .debug_selector(|| "fade-top".into())
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(fade))
                    .bg(grad(1., 0.))
                    .into_any_element(),
            );
        }
        if show_bot {
            out.push(
                div()
                    .debug_selector(|| "fade-bot".into())
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(fade))
                    .bg(grad(0., 1.))
                    .into_any_element(),
            );
        }
        out
    }

    /// `.turn-end-fork-menu`: Fork chat / Fork to new workspace.
    fn fork_menu_el(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.fork_menu.clone()?;
        if !self.hooks.fork_to_workspace {
            return None;
        }
        let t = Theme::of(cx).tokens;
        let item = |id: &'static str,
                    ic: IconName,
                    s: &'static str,
                    to_ws: bool,
                    cx: &mut Context<Self>| {
            let mid = menu.message_id.clone();
            div()
                .id(id)
                .debug_selector(move || format!("fork-item:{s}"))
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .px(px(10.))
                .py(px(8.))
                .rounded(px(5.))
                .text_size(px(12.5))
                .text_color(hsla(t.text))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(t.pane)))
                .child(icon(ic).size(px(14.)).color(hsla(t.muted)))
                .child(text(s).line_height(LineHeight::Normal).nowrap())
                .on_click(cx.listener(move |c, _, _, cx| {
                    c.fork_menu = None;
                    cx.emit(if to_ws {
                        ChatEvent::ForkToWorkspace(mid.clone())
                    } else {
                        ChatEvent::Fork(mid.clone())
                    });
                    cx.notify();
                }))
        };
        let panel = recur_ui::style::card(t.pane_2, t.line_bright, 8.)
            .min_w(px(220.))
            .p(px(4.))
            .child(item("fork-chat", IconName::Split, "Fork chat", false, cx))
            .child(item(
                "fork-ws",
                IconName::Plus,
                "Fork to new workspace",
                true,
                cx,
            ));
        let panel = with_shadow(
            panel,
            &[css_box_shadow(0., 10., 28., 0., rgba(0, 0, 0, 0.45))],
            8.,
        )
        .id("fork-menu")
        .occlude()
        .on_mouse_down_out(cx.listener(|c, _, _, cx| {
            c.fork_menu = None;
            cx.notify();
        }));
        Some(
            gpui_kit::deferred(recur_ui::style::anchored_at(menu.left, menu.top, panel))
                .with_priority(80)
                .into_any_element(),
        )
    }
}

impl Render for AgentChat {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        self.tc_shift = crate::rows::tc_icon_shift(window);
        self.scale = window.scale_factor();
        self.snapshot_tool_data(cx);
        self.markdown_seen.clear();
        self.sync_turn_clock(cx);
        let composer_only = self.props.composer_only;
        let session = self.props.session_body;
        let compact = self.props.compact;
        let mut root = div()
            .id(SharedString::from(format!("chat-{}", self.props.reset_key)))
            .debug_selector(|| "chat".into())
            .relative()
            .flex()
            .flex_col()
            .min_h_0()
            .when(!session, |d| d.size_full().bg(hsla(t.bg)))
            .when(session, |d| d.max_h(px(420.)))
            .text_size(px(13.))
            .text_color(hsla(t.text));
        if !composer_only {
            let kids = self.transcript_children(window, cx);
            let entity = cx.entity();
            let probe = canvas(
                move |b, _, cx| {
                    let h = f32::from(b.size.height);
                    let e = entity.clone();
                    cx.defer(move |cx| e.update(cx, |c, cx| c.on_scroll_frame(h, cx)));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0();
            let col = div()
                .flex()
                .flex_col()
                .w_full()
                .when(!session, |d| d.max_w(px(720.)).mx_auto())
                .children(kids);
            let scroller = div()
                .id(SharedString::from(format!(
                    "chat-scroll-{}",
                    self.props.reset_key
                )))
                .debug_selector(|| "chat-scroll".into())
                .track_scroll(&self.scroll)
                .overflow_y_scroll()
                .when(!session && !compact, |d| {
                    d.flex_1().min_h_0().pt(px(26.)).px(px(28.)).pb(px(8.))
                })
                .when(!session && compact, |d| {
                    d.flex_1().min_h_0().pt(px(18.)).px(px(18.)).pb(px(8.))
                })
                .when(session, |d| {
                    d.flex_initial()
                        .min_h_0()
                        .max_h(px(420.))
                        .py(px(8.))
                        .px(px(12.))
                })
                .child(col);
            let main = div()
                .relative()
                .when(!session, |d| d.flex_1().min_h_0())
                .when(session, |d| d.flex_initial().min_h_0())
                .flex()
                .flex_col()
                .child(scroller)
                .child(probe)
                .children(self.fades(cx))
                .children(self.fork_menu_el(cx));
            root = root.child(main);
        }
        if let Some(dock) = self.dock(window, cx) {
            root = root.child(dock);
        }
        if let Some(lb) = &self.lightbox {
            root = root.child(lb.clone());
        }
        if let Some(menu) = &self.img_ctx {
            root = root.child(menu.clone());
        }
        // Drop the cached markdown of rows that left the transcript.
        let seen = std::mem::take(&mut self.markdown_seen);
        self.markdown.retain(|k, _| seen.contains(k));
        self.streams.retain(|k, _| seen.contains(k));
        let perms: Vec<String> = self
            .chat_permissions(cx)
            .into_iter()
            .map(|r| r.id)
            .collect();
        let qs: Vec<String> = self.chat_questions(cx).into_iter().map(|q| q.id).collect();
        self.cards.retain(&perms, &qs);
        if self.props.readonly || self.app_surface() {
            return root;
        }
        root.on_drag_move::<ExternalPaths>(cx.listener(
            |c, ev: &DragMoveEvent<ExternalPaths>, _, cx| {
                let over = ev.bounds.contains(&ev.event.position);
                if over != c.file_drop_over {
                    c.file_drop_over = over;
                    cx.notify();
                }
            },
        ))
        .on_drag_move::<WorkspaceFileDrag>(cx.listener(
            |c, ev: &DragMoveEvent<WorkspaceFileDrag>, _, cx| {
                let over = ev.bounds.contains(&ev.event.position);
                if over != c.file_drop_over {
                    c.file_drop_over = over;
                    cx.notify();
                }
            },
        ))
        .on_drop(cx.listener(|c, paths: &ExternalPaths, window, cx| {
            c.file_drop_over = false;
            let list: Vec<String> = paths
                .0
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            c.apply_os_drop(list, window, cx);
        }))
        .on_drop(cx.listener(|c, drag: &WorkspaceFileDrag, window, cx| {
            let paths = drag.paths.clone();
            c.drop_explorer_paths(&paths, window, cx);
        }))
    }
}
