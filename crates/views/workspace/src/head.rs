//! The agent dock's head (`leftHead(lit)`): the chat tab strip with its recents menu of
//! closed chats and the New chat button.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, deferred, div, px,
    relative,
};
use recur_engine::view::artifacts::format_relative_age;
use recur_ui::fonts::Family;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::tab_chip::{ChipIcon, tab_chip};
use recur_ui::widgets::tab_strip::{tab_strip, tab_strip_action};
use recur_ui::{IconName, Theme, icon, rgba};
use recur_view_core::modes::NEW_CHAT_TITLE;

use crate::view::{WorkspaceView, now_ms};

pub struct ChatTabsHead {
    view: WeakEntity<WorkspaceView>,
    /// The clock button's bounds: a press there toggles instead of closing from outside.
    recents_btn: std::rc::Rc<std::cell::Cell<Option<gpui_kit::Bounds<gpui_kit::Pixels>>>>,
    /// The agent dock has keyboard focus (`leftHead(lit)`).
    lit: bool,
}

impl ChatTabsHead {
    pub(crate) fn new(view: WeakEntity<WorkspaceView>) -> Self {
        ChatTabsHead {
            view,
            recents_btn: Default::default(),
            lit: false,
        }
    }

    pub(crate) fn set_lit(&mut self, lit: bool, cx: &mut Context<Self>) {
        if self.lit != lit {
            self.lit = lit;
            cx.notify();
        }
    }
}

fn hsla(c: recur_ui::Color) -> Hsla {
    c.into()
}

impl ChatTabsHead {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let Some(view) = self.view.upgrade() else {
            return div().into_any_element();
        };
        let t = Theme::of(cx).tokens;
        let v = view.read(cx);
        let ws = v.ws_id.clone();
        let active = v.active_code();
        let tabs = v.tabs().to_vec();
        let n = tabs.len();
        let mut strip = tab_strip(
            SharedString::from(format!("wsv-chat-tabs-{ws}")),
            "Agent chats",
        )
        .flush(true)
        .active_key(SharedString::from(active.clone()))
        .scroll_handle(v.strip_scroll.clone())
        .can_start(move |_| n > 1)
        .on_reorder({
            let w = self.view.clone();
            move |order, _, _, cx| {
                let _ = w.update(cx, |v, cx| v.reorder_tabs(order, cx));
            }
        });
        if let Some((left, id)) = v.strip_restore {
            strip = strip.restore_scroll(left, id);
        }
        for tab in &tabs {
            let mode = v.tab_mode_for(tab);
            let busy = v.chat_key_busy(&v.key(&tab.code), cx);
            let unseen = tab.unseen && active != tab.code;
            let label = if tab.title.is_empty() {
                NEW_CHAT_TITLE.to_string()
            } else {
                tab.title.clone()
            };
            let title = format!(
                "{label} · {} mode{}{}",
                mode.label(),
                if busy { " · running" } else { "" },
                if unseen { " · new response" } else { "" }
            );
            let close = format!(
                "Close {}",
                if tab.title.is_empty() {
                    "chat"
                } else {
                    &tab.title
                }
            );
            let (sel, cls) = (tab.code.clone(), tab.code.clone());
            let (w1, w2) = (self.view.clone(), self.view.clone());
            strip = strip.tab(
                SharedString::from(tab.code.clone()),
                tab_chip(SharedString::from(format!("chat-tab-{}", tab.code)), label)
                    .title(title)
                    .icon(ChipIcon::App(mode.icon(), Some(hsla(mode.tint(&t)))))
                    .active(active == tab.code)
                    .lit(self.lit)
                    .busy(busy)
                    .unseen(unseen)
                    .animate_label(true, format!("{ws}:{}", tab.code))
                    .close_label(close)
                    .reorderable(n > 1)
                    .on_click(move |_, _, cx| {
                        let _ = w1.update(cx, |v, cx| v.select_chat_tab(&sel, cx));
                    })
                    .on_close(move |_, _, cx| {
                        let _ = w2.update(cx, |v, cx| v.close_chat_tab(&cls, cx));
                    }),
            );
        }
        if !v.closed_tabs.is_empty() {
            strip = strip.trailing(self.recents(v, cx));
        }
        let w = self.view.clone();
        strip = strip.trailing(titled(
            "wsv-chat-new-t",
            "New chat (⌘T)",
            // `.wsv-chat-new { color: var(--muted) }`.
            tab_strip_action("wsv-chat-new", false, false, cx)
                .debug_selector(|| "wsv-chat-new".into())
                .text_color(hsla(t.muted))
                .child(icon(IconName::Plus).size(px(15.)))
                .on_click(move |_, _, cx| {
                    let _ = w.update(cx, |v, cx| v.new_chat_tab(cx).detach());
                }),
        ));
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_stretch()
            .child(strip)
            .into_any_element()
    }
}

impl Render for ChatTabsHead {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

impl ChatTabsHead {
    /// `.wsv-recents`: the clock button and its menu of closed chats.
    fn recents(&self, v: &WorkspaceView, cx: &gpui_kit::App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let open = v.recents_open;
        let w = self.view.clone();
        let button = titled(
            "wsv-recents-t",
            "Recent closed chats",
            tab_strip_action("wsv-recents-btn", open, false, cx)
                .debug_selector(|| "wsv-recents".into())
                .child(icon(IconName::Clock).size(px(15.)))
                .child({
                    let b = self.recents_btn.clone();
                    gpui_kit::canvas(move |bounds, _, _| b.set(Some(bounds)), |_, _, _, _| {})
                        .absolute()
                        .inset_0()
                })
                .on_click(move |_, _, cx| {
                    let _ = w.update(cx, |v, cx| v.toggle_recents(cx));
                }),
        );
        let mut root = div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .child(button);
        if open {
            let now = now_ms();
            let rows: Vec<AnyElement> =
                v.closed_tabs
                    .iter()
                    .map(|tab| {
                        let title = if tab.title.is_empty() {
                            NEW_CHAT_TITLE.to_string()
                        } else {
                            tab.title.clone()
                        };
                        let mode = v.tab_mode_for(tab);
                        let armed = v.armed_delete.as_deref() == Some(tab.code.as_str());
                        let age = format_relative_age(tab.updated_at, now);
                        let group = SharedString::from(format!("wsv-recents-row-{}", tab.code));
                        let (c1, c2, c3) = (tab.code.clone(), tab.code.clone(), tab.code.clone());
                        let (w1, w2, w3) =
                            (self.view.clone(), self.view.clone(), self.view.clone());
                        let item =
                            div()
                                .id(SharedString::from(format!("wsv-recents-item-{}", tab.code)))
                                .debug_selector({
                                    let c = tab.code.clone();
                                    move || format!("wsv-recents-item:{c}")
                                })
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .flex_1()
                                .min_w_0()
                                .px(px(10.))
                                .py(px(7.))
                                .rounded(px(5.))
                                .cursor_pointer()
                                .child(div().flex().flex_none().items_center().child(
                                    icon(mode.icon()).size(px(14.)).color(hsla(mode.tint(&t))),
                                ))
                                .child(
                                    div().flex_1().min_w_0().child(
                                        text(title.clone())
                                            .size(12.)
                                            .color(t.text)
                                            .line_height(LineHeight::Normal)
                                            .nowrap()
                                            .truncate(),
                                    ),
                                )
                                .on_click(move |_, _, cx| {
                                    let _ = w1.update(cx, |v, cx| v.reopen_closed_chat(&c1, cx));
                                });
                        let item = titled(
                            SharedString::from(format!("wsv-recents-item-t-{}", tab.code)),
                            title.clone(),
                            item,
                        );
                        let del_label = if armed {
                            "Confirm delete"
                        } else {
                            "Delete permanently"
                        };
                        let del = titled(
                            SharedString::from(format!("wsv-recents-del-t-{}", tab.code)),
                            del_label,
                            div()
                                .id(SharedString::from(format!("wsv-recents-del-{}", tab.code)))
                                .debug_selector({
                                    let c = tab.code.clone();
                                    move || format!("wsv-recents-del:{c}")
                                })
                                .absolute()
                                .top_0()
                                .right_0()
                                .size(px(22.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .map(|d| {
                                    if armed {
                                        d.bg(hsla(t.coral))
                                            .text_color(hsla(recur_ui::hex(0x0A0C10)))
                                    } else {
                                        d.text_color(hsla(t.faint))
                                            .invisible()
                                            .group_hover(group.clone(), |s| s.visible())
                                            .hover(|s| s.text_color(hsla(t.coral)))
                                    }
                                })
                                .child(icon(IconName::Trash).size(px(12.)))
                                .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    let _ = w2.update(cx, |v, cx| v.on_delete_closed_chat(&c2, cx));
                                }),
                        );
                        let slot = div()
                            .relative()
                            .flex_none()
                            .min_w(px(22.))
                            .h(px(22.))
                            .mr(px(8.))
                            .flex()
                            .items_center()
                            .justify_end()
                            .when_some(age, |d, age| {
                                d.child(titled(
                                    SharedString::from(format!("wsv-recents-age-{}", tab.code)),
                                    age.full,
                                    div()
                                        .id(SharedString::from(format!(
                                            "wsv-recents-age-el-{}",
                                            tab.code
                                        )))
                                        .pr(px(5.))
                                        .when(armed, |d| d.invisible())
                                        .when(!armed, |d| {
                                            d.group_hover(group.clone(), |s| s.invisible())
                                        })
                                        .child(
                                            text(age.rel)
                                                .family(Family::Mono)
                                                .size(10.)
                                                .color(t.faint)
                                                .tabular()
                                                .line_height(LineHeight::Normal),
                                        ),
                                ))
                            })
                            .child(del);
                        div()
                            .id(SharedString::from(format!("wsv-recents-row-{c3}")))
                            .group(group.clone())
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .hover(|s| s.bg(hsla(t.pane_2)))
                            .when(armed, |d| d.bg(hsla(t.pane_2)))
                            .on_hover(move |hovered, _, cx| {
                                if !*hovered {
                                    let _ = w3.update(cx, |v, cx| {
                                        if v.armed_delete.as_deref() == Some(c3.as_str()) {
                                            v.clear_armed_delete();
                                            v.touch(cx);
                                        }
                                    });
                                }
                            })
                            .child(item)
                            .child(slot)
                            .into_any_element()
                    })
                    .collect();
            let panel = recur_ui::style::card(t.pane, t.line_bright, 8.)
                .min_w(px(200.))
                .max_w(px(280.))
                .p(px(4.))
                .children(rows);
            let wc = self.view.clone();
            let menu = with_shadow(
                panel,
                &[css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))],
                8.,
            )
            .id("wsv-recents-menu")
            .debug_selector(|| "wsv-recents-menu".into())
            .occlude()
            .on_mouse_down_out({
                let btn = self.recents_btn.clone();
                move |e, _, cx| {
                    if btn.get().is_some_and(|b| b.contains(&e.position)) {
                        return;
                    }
                    let _ = wc.update(cx, |v, cx| v.close_recents(cx));
                }
            });
            root = root.child(
                deferred(
                    div()
                        .absolute()
                        .right_0()
                        .top(relative(1.))
                        .mt(px(4.))
                        .child(menu),
                )
                .with_priority(40),
            );
        }
        root.into_any_element()
    }
}
