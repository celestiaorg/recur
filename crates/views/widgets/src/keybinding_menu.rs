//! The numbered picker when several bundle commands or snippets share a chord or Tab trigger
//! 1–9 pick a row, ↑↓ and ↩ move and run,
//! Esc or a click on the scrim closes.

use recur_ui::guard::Guarded;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, Global, InteractiveElement, IntoElement,
    KeyBinding, KeyDownEvent, MouseButton, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, actions, div, px,
};
use recur_code::collate::locale_compare;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Theme, rgba};
use recur_view_core::AppState;

actions!(keybinding_menu, [Close]);

const CONTEXT: &str = "KeybindingMenu";

pub fn bind(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Close, Some(CONTEXT))]);
}

/// One row (`KeybindingMenuItem`).
#[derive(Clone, Debug, PartialEq)]
pub struct KeybindingMenuItem {
    pub id: String,
    pub label: String,
    pub category: String,
    pub order: i64,
}

pub type OnPick = Rc<dyn Fn(KeybindingMenuItem, &mut Window, &mut App)>;

/// The open menu (`keybindingMenu`); `None` when closed.
#[derive(Default)]
pub struct KeybindingMenuStore {
    pub open: Option<(Vec<KeybindingMenuItem>, OnPick)>,
    pub epoch: u64,
}

impl Global for KeybindingMenuStore {}

/// `openKeybindingMenu`.
pub fn open_keybinding_menu(items: Vec<KeybindingMenuItem>, on_pick: OnPick, cx: &mut App) {
    let s = cx.default_global::<KeybindingMenuStore>();
    s.open = Some((items, on_pick));
    s.epoch += 1;
    if let Some(state) = AppState::try_global(cx) {
        state.update(cx, |s, cx| s.set_keybinding_menu_open(true, cx));
    }
    cx.refresh_windows();
}

/// `closeKeybindingMenu`.
pub fn close_keybinding_menu(cx: &mut App) {
    let s = cx.default_global::<KeybindingMenuStore>();
    if s.open.take().is_none() {
        return;
    }
    s.epoch += 1;
    if let Some(state) = AppState::try_global(cx) {
        state.update(cx, |s, cx| s.set_keybinding_menu_open(false, cx));
    }
    cx.refresh_windows();
}

pub fn keybinding_menu_open(cx: &App) -> bool {
    cx.try_global::<KeybindingMenuStore>()
        .is_some_and(|s| s.open.is_some())
}

/// Sections by category (locale order), rows by `order` then label, in display order.
pub fn sections(items: &[KeybindingMenuItem]) -> Vec<(String, Vec<KeybindingMenuItem>)> {
    let mut cats: Vec<String> = Vec::new();
    for it in items {
        if !cats.contains(&it.category) {
            cats.push(it.category.clone());
        }
    }
    cats.sort_by(|a, b| locale_compare(a, b));
    cats.into_iter()
        .map(|c| {
            let mut rows: Vec<KeybindingMenuItem> =
                items.iter().filter(|i| i.category == c).cloned().collect();
            rows.sort_by(|a, b| {
                a.order
                    .cmp(&b.order)
                    .then_with(|| locale_compare(&a.label, &b.label))
            });
            (c, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

pub struct KeybindingMenu {
    sel: usize,
    epoch: u64,
    focus: FocusHandle,
    _state: Entity<AppState>,
}

impl Focusable for KeybindingMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl KeybindingMenu {
    pub fn new(state: Entity<AppState>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.default_global::<KeybindingMenuStore>();
        cx.observe_global::<KeybindingMenuStore>(|m, cx| {
            let epoch = cx.global::<KeybindingMenuStore>().epoch;
            if epoch != m.epoch {
                m.epoch = epoch;
                m.sel = 0;
            }
            cx.notify();
        })
        .detach();
        KeybindingMenu {
            sel: 0,
            epoch: 0,
            focus: cx.focus_handle(),
            _state: state,
        }
    }

    fn flat(cx: &App) -> Vec<KeybindingMenuItem> {
        let items = cx
            .try_global::<KeybindingMenuStore>()
            .and_then(|s| s.open.as_ref())
            .map(|(i, _)| i.clone())
            .unwrap_or_default();
        sections(&items).into_iter().flat_map(|(_, r)| r).collect()
    }

    fn run_at(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let flat = Self::flat(cx);
        let Some(item) = flat.get(i).cloned() else {
            return;
        };
        let pick = cx
            .try_global::<KeybindingMenuStore>()
            .and_then(|s| s.open.as_ref().map(|(_, p)| p.clone()));
        let Some(pick) = pick else { return };
        close_keybinding_menu(cx);
        pick(item, window, cx);
    }

    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let n = Self::flat(cx).len();
        match k.key.as_str() {
            "down" | "up" => {
                if k.modifiers.platform || k.modifiers.control || k.modifiers.alt {
                    return;
                }
                self.sel = if k.key == "down" {
                    (self.sel + 1).min(n.saturating_sub(1))
                } else {
                    self.sel.saturating_sub(1)
                };
                cx.stop_propagation();
                cx.notify();
            }
            "enter" => {
                cx.stop_propagation();
                self.run_at(self.sel, window, cx);
            }
            d if d.len() == 1 && ("1"..="9").contains(&d) => {
                let i: usize = d.parse().unwrap_or(0);
                if i >= 1 && i <= n.min(9) {
                    cx.stop_propagation();
                    self.run_at(i - 1, window, cx);
                }
            }
            _ => {}
        }
    }
}

impl KeybindingMenu {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if !keybinding_menu_open(cx) {
            return div().into_any_element();
        }
        if !self.focus.is_focused(window) {
            let f = self.focus.clone();
            window.defer(cx, move |window, cx| window.focus(&f, cx));
        }
        let t = Theme::of(cx).tokens;
        let items = cx
            .try_global::<KeybindingMenuStore>()
            .and_then(|s| s.open.as_ref())
            .map(|(i, _)| i.clone())
            .unwrap_or_default();
        let secs = sections(&items);
        let vh = f32::from(window.viewport_size().height);
        let vw = f32::from(window.viewport_size().width);
        let mut panel = recur_ui::style::card(t.pane, t.line_bright, 10.)
            .id("kb-menu")
            .min_w(px(260.))
            .max_w(px(420f32.min(vw * 0.92)))
            .max_h(px(vh * 0.7))
            .overflow_y_scroll()
            .py(px(6.))
            .flex()
            .flex_col();
        let mut idx = 0usize;
        let mut prev_row = false;
        for (si, (title, rows)) in secs.iter().enumerate() {
            let sep = si > 0;
            panel = panel.child(
                div()
                    .when(sep && prev_row, |d| {
                        d.border_t_1().border_color(t.line).mt(px(4.))
                    })
                    .pt(px(8.))
                    .px(px(14.))
                    .pb(px(3.))
                    .child(
                        text(title.to_uppercase())
                            .size(9.)
                            .weight(600.)
                            .tracking(0.12)
                            .color(t.muted)
                            .nowrap(),
                    ),
            );
            for item in rows {
                let i = idx;
                idx += 1;
                let sel = i == self.sel;
                panel = panel.child(
                    div()
                        .id(SharedString::from(format!("kb-row-{}", item.id)))
                        .debug_selector({
                            let id = item.id.clone();
                            move || format!("kb-row:{id}")
                        })
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(14.))
                        .py(px(5.))
                        .min_h(px(26.))
                        .when(sel, |d| d.bg(rgba(255, 255, 255, 0.08)))
                        .on_mouse_move(cx.guarded_listener(move |m, _, _, cx| {
                            if m.sel != i {
                                m.sel = i;
                                cx.notify();
                            }
                        }))
                        .on_click(
                            cx.guarded_listener(move |m, _, window, cx| m.run_at(i, window, cx)),
                        )
                        .child(
                            div().flex_1().min_w_0().child(
                                text(item.label.clone())
                                    .size(13.)
                                    .line_height(LineHeight::Scale(1.3))
                                    .color(t.text),
                            ),
                        )
                        .when(i < 9, |d| {
                            d.child(
                                text(format!("{}", i + 1))
                                    .size(11.)
                                    .color(t.muted)
                                    .tabular()
                                    .nowrap(),
                            )
                        }),
                );
                prev_row = true;
            }
        }
        div()
            .id("kb-menu-bg")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.guarded_listener(|_, _: &Close, _, cx| close_keybinding_menu(cx)))
            .on_key_down(
                cx.guarded_listener(|m, e: &KeyDownEvent, window, cx| m.on_key(e, window, cx)),
            )
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(5, 7, 10, 0.35))
            .on_mouse_down(MouseButton::Left, |_, _, cx| close_keybinding_menu(cx))
            .child(recur_ui::shadow::with_shadow(
                panel.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                &[recur_ui::shadow::css_box_shadow(
                    0.,
                    20.,
                    60.,
                    -16.,
                    rgba(0, 0, 0, 0.7),
                )],
                10.,
            ))
            .into_any_element()
    }
}

impl Render for KeybindingMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, cat: &str, order: i64) -> KeybindingMenuItem {
        KeybindingMenuItem {
            id: id.into(),
            label: id.into(),
            category: cat.into(),
            order,
        }
    }

    #[test]
    fn sections_sort_categories_then_order_then_label() {
        let s = sections(&[
            item("b", "Snippets", 2),
            item("a", "Snippets", 2),
            item("z", "Go", 1),
            item("c", "Snippets", 1),
        ]);
        let flat: Vec<(&str, &str)> = s
            .iter()
            .flat_map(|(c, r)| r.iter().map(move |i| (c.as_str(), i.id.as_str())))
            .collect();
        assert_eq!(
            flat,
            [
                ("Go", "z"),
                ("Snippets", "c"),
                ("Snippets", "a"),
                ("Snippets", "b")
            ]
        );
    }
}
