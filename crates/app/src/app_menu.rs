//! The in-window application menu. GPUI draws no menubar off macOS, so a titlebar button (or a
//! lone Alt) opens the menus [`crate::menu::app_menus`] builds, read back from `cx.get_menus()`:
//! same items, same enabled state and theme radios, same actions, with the chords
//! [`crate::menu::bind_menu_keys`] labels them with. Submenus expand in place.

use std::collections::HashSet;

use gpui_kit::{
    Action, AnyElement, App, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    Keymap, MouseMoveEvent, OwnedMenuItem, ParentElement, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, deferred, div, prelude::FluentBuilder, px,
};
use recur_ui::guard::Guarded;
use recur_ui::text::{TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::scroll_area;
use recur_ui::{IconName, Theme, icon, style};

use crate::chrome::no_drag;
use crate::shell::Shell;

/// Whether the titlebar carries the menu button: wherever the OS draws no menubar.
pub const IN_APP_MENU: bool = cfg!(not(target_os = "macos"));

#[derive(Default)]
pub(crate) struct AppMenu {
    open: bool,
    /// Expanded submenus, keyed by their label path.
    expanded: HashSet<String>,
    /// The highlighted row (pointer or arrow keys).
    active: Option<usize>,
    scroll: ScrollHandle,
    focus: Option<FocusHandle>,
    /// Focus before the menu opened, restored when it closes.
    restore: Option<FocusHandle>,
    /// A press on the button while open closes through the button, not the click-outside.
    over_button: bool,
}

enum RowKind {
    Separator,
    Submenu {
        path: String,
        expanded: bool,
    },
    Item {
        action: Box<dyn Action>,
        checked: bool,
        accel: Option<String>,
    },
}

/// One line of the menu with its expanded submenus flattened in.
struct Row {
    label: SharedString,
    depth: usize,
    /// The enclosing submenu's path; empty at the top level.
    parent: String,
    disabled: bool,
    kind: RowKind,
}

impl Row {
    fn selectable(&self) -> bool {
        !self.disabled && !matches!(self.kind, RowKind::Separator)
    }
}

fn rows(menu: &AppMenu, cx: &App) -> Vec<Row> {
    let top: Vec<OwnedMenuItem> = cx
        .get_menus()
        .unwrap_or_default()
        .into_iter()
        .map(OwnedMenuItem::Submenu)
        .collect();
    let keymap = cx.key_bindings();
    let mut out = Vec::new();
    flatten(&top, "", 0, &menu.expanded, &keymap.borrow(), &mut out);
    out
}

fn flatten(
    items: &[OwnedMenuItem],
    parent: &str,
    depth: usize,
    expanded: &HashSet<String>,
    keymap: &Keymap,
    out: &mut Vec<Row>,
) {
    for item in items {
        let row = |label: SharedString, disabled, kind| Row {
            label,
            depth,
            parent: parent.to_string(),
            disabled,
            kind,
        };
        match item {
            OwnedMenuItem::Separator => out.push(row("".into(), true, RowKind::Separator)),
            OwnedMenuItem::Submenu(sub) => {
                let path = format!("{parent}/{}", sub.name);
                let open = expanded.contains(&path);
                out.push(row(
                    sub.name.clone(),
                    false,
                    RowKind::Submenu {
                        path: path.clone(),
                        expanded: open,
                    },
                ));
                if open {
                    flatten(&sub.items, &path, depth + 1, expanded, keymap, out);
                }
            }
            OwnedMenuItem::Action {
                name,
                action,
                checked,
                disabled,
                ..
            } => out.push(row(
                name.clone().into(),
                *disabled,
                RowKind::Item {
                    action: action.boxed_clone(),
                    checked: *checked,
                    accel: accel_label(keymap, action.as_ref()),
                },
            )),
            OwnedMenuItem::SystemMenu(_) => {}
        }
    }
}

/// The action's chord as Linux shows it, e.g. `Ctrl+Shift+P`.
fn accel_label(keymap: &Keymap, action: &dyn Action) -> Option<String> {
    let binding = keymap.bindings_for_action(action).next()?;
    let chords: Vec<String> = binding
        .keystrokes()
        .iter()
        .map(|k| {
            let m = k.modifiers();
            let mut parts: Vec<String> = [
                (m.control, "Ctrl"),
                (m.alt, "Alt"),
                (m.shift, "Shift"),
                (m.platform, "Super"),
            ]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, name)| name.to_string())
            .collect();
            let mut key = k.key().chars();
            parts.push(
                key.next()
                    .map(|c| c.to_uppercase().chain(key).collect())
                    .unwrap_or_default(),
            );
            parts.join("+")
        })
        .collect();
    Some(chords.join(" "))
}

/// The next selectable row from `from` in `dir` (±1), wrapping; from the end when `from` is
/// `None`.
fn step(rows: &[Row], from: Option<usize>, dir: isize) -> Option<usize> {
    let n = rows.len() as isize;
    let mut i = match from {
        Some(i) => i as isize,
        None if dir > 0 => -1,
        None => n,
    };
    for _ in 0..n {
        i = (i + dir).rem_euclid(n);
        if rows[i as usize].selectable() {
            return Some(i as usize);
        }
    }
    None
}

impl Shell {
    pub(crate) fn app_menu_button(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.titlebar.menu.open;
        let button = no_drag(style::rail_toggle("tb-menu", open, cx))
            .debug_selector(|| "tb-menu".into())
            .on_hover(cx.guarded_listener(|this, over: &bool, _, _| {
                this.titlebar.menu.over_button = *over;
            }))
            .on_click(cx.guarded_listener(|this, _, window, cx| {
                if this.titlebar.menu.open {
                    this.close_app_menu(window, cx);
                } else {
                    this.open_app_menu(false, window, cx);
                }
            }))
            .child(icon(IconName::Menu).size(px(16.)));
        div()
            .relative()
            .flex_none()
            .map(|d| {
                if open {
                    d.child(button)
                } else {
                    d.child(titled("tb-menu-tip", "Menu (Alt)", button))
                }
            })
            .when(open, |d| {
                let panel = self.app_menu_panel(window, cx);
                d.child(
                    deferred(style::attached(
                        true,
                        6.,
                        0.,
                        recur_ui::shadow::with_shadow(panel, &style::MENU_SHADOW, 8.),
                    ))
                    .with_priority(210),
                )
            })
            .into_any_element()
    }

    /// A lone Alt: open with the first row highlighted, or close.
    pub(crate) fn toggle_app_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.titlebar.menu.open {
            self.close_app_menu(window, cx);
        } else {
            self.open_app_menu(true, window, cx);
        }
    }

    fn open_app_menu(&mut self, keyboard: bool, window: &mut Window, cx: &mut Context<Self>) {
        let menu = &mut self.titlebar.menu;
        menu.open = true;
        menu.expanded.clear();
        menu.active = None;
        menu.restore = window.focused(cx);
        let focus = menu.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
        if keyboard {
            self.titlebar.menu.active = step(&rows(&self.titlebar.menu, cx), None, 1);
        }
        cx.notify();
    }

    fn close_app_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let menu = &mut self.titlebar.menu;
        if !menu.open {
            return;
        }
        menu.open = false;
        if let Some(prev) = menu.restore.take() {
            window.focus(&prev, cx);
        }
        cx.notify();
    }

    /// A click or Enter on row `ix`: a submenu expands or collapses, an item runs.
    fn activate_app_menu_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let mut rows = rows(&self.titlebar.menu, cx);
        if ix >= rows.len() || !rows[ix].selectable() {
            return;
        }
        match rows.swap_remove(ix).kind {
            RowKind::Submenu { path, .. } => {
                let open = &mut self.titlebar.menu.expanded;
                if !open.remove(&path) {
                    open.insert(path);
                }
                self.titlebar.menu.active = Some(ix);
                cx.notify();
            }
            RowKind::Item { action, .. } => {
                self.close_app_menu(window, cx);
                window.dispatch_action(action, cx);
            }
            RowKind::Separator => {}
        }
    }

    /// Arrows move, Right / Left open and close submenus, Enter / Space pick, Esc / Tab close.
    fn app_menu_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let rows = rows(&self.titlebar.menu, cx);
        let active = self.titlebar.menu.active;
        let current = active.and_then(|i| rows.get(i));
        let mut next = active;
        match e.keystroke.key.as_str() {
            "escape" | "tab" => {
                cx.stop_propagation();
                return self.close_app_menu(window, cx);
            }
            "enter" | "space" => {
                cx.stop_propagation();
                if let Some(ix) = active {
                    self.activate_app_menu_row(ix, window, cx);
                }
                return;
            }
            "down" => next = step(&rows, active, 1),
            "up" => next = step(&rows, active, -1),
            "home" => next = step(&rows, None, 1),
            "end" => next = step(&rows, None, -1),
            "right" => {
                if let Some(Row {
                    kind:
                        RowKind::Submenu {
                            path,
                            expanded: false,
                        },
                    ..
                }) = current
                {
                    self.titlebar.menu.expanded.insert(path.clone());
                }
            }
            "left" => {
                let collapse = match current {
                    Some(Row {
                        kind:
                            RowKind::Submenu {
                                path,
                                expanded: true,
                            },
                        ..
                    }) => Some(path.clone()),
                    Some(row) if !row.parent.is_empty() => Some(row.parent.clone()),
                    _ => None,
                };
                if let Some(path) = collapse {
                    next = rows.iter().position(
                        |r| matches!(&r.kind, RowKind::Submenu { path: p, .. } if *p == path),
                    );
                    self.titlebar.menu.expanded.remove(&path);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        self.titlebar.menu.active = next;
        if let Some(ix) = next {
            self.titlebar.menu.scroll.scroll_to_item(ix);
        }
        cx.notify();
    }

    fn app_menu_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let rows = rows(&self.titlebar.menu, cx);
        let active = self.titlebar.menu.active;
        let mut items = Vec::with_capacity(rows.len());
        for (ix, row) in rows.into_iter().enumerate() {
            if let RowKind::Separator = row.kind {
                items.push(
                    div()
                        .h(px(1.))
                        .mx(px(6.))
                        .my(px(4.))
                        .bg(t.line)
                        .into_any_element(),
                );
                continue;
            }
            let selectable = row.selectable();
            let (lead, trail) = match &row.kind {
                RowKind::Submenu { expanded, .. } => (
                    None,
                    Some(
                        icon(if *expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(14.))
                        .color(t.faint)
                        .into_any_element(),
                    ),
                ),
                RowKind::Item { checked, accel, .. } => (
                    checked.then_some(IconName::Check),
                    accel.clone().map(|a| {
                        text(a)
                            .mono()
                            .size(11.)
                            .color(t.faint)
                            .nowrap()
                            .into_any_element()
                    }),
                ),
                RowKind::Separator => (None, None),
            };
            let label = row.label.clone();
            items.push(
                div()
                    .id(("app-menu-row", ix))
                    .debug_selector(move || format!("app-menu-row:{label}"))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(6.))
                    .pr(px(10.))
                    .pl(px(10. + 14. * row.depth as f32))
                    .rounded(px(5.))
                    .text_size(px(12.5))
                    .text_color(if row.disabled { t.faint } else { t.text })
                    .when(active == Some(ix), |d| d.bg(t.pane))
                    .child(match lead {
                        Some(name) => icon(name).size(px(14.)).color(t.amber).into_any_element(),
                        None => div().flex_none().size(px(14.)).into_any_element(),
                    })
                    .child(div().flex_1().min_w_0().child(text(row.label).truncate()))
                    .children(trail)
                    .when(selectable, |d| {
                        d.cursor_pointer()
                            .on_mouse_move(cx.guarded_listener(
                                move |this, _: &MouseMoveEvent, _, cx| {
                                    if this.titlebar.menu.active != Some(ix) {
                                        this.titlebar.menu.active = Some(ix);
                                        cx.notify();
                                    }
                                },
                            ))
                            .on_click(cx.guarded_listener(move |this, _, window, cx| {
                                this.activate_app_menu_row(ix, window, cx)
                            }))
                    })
                    .into_any_element(),
            );
        }
        let inset = f32::from(window.client_inset().unwrap_or_default());
        let max_h = f32::from(window.viewport_size().height) - inset * 2. - 64.;
        let mut panel = style::menu_panel(cx)
            .id("app-menu")
            .debug_selector(|| "app-menu".into())
            .w(px(300.))
            .max_h(px(max_h.max(120.)))
            .occlude()
            .on_mouse_down_out(cx.guarded_listener(|this, _, window, cx| {
                if !this.titlebar.menu.over_button {
                    this.close_app_menu(window, cx);
                }
            }))
            .on_key_down(cx.guarded_listener(Self::app_menu_key))
            // The card's background is a child: scroll the rows inside it, not the panel.
            .child(
                div().flex_auto().min_h_0().flex().flex_col().child(
                    scroll_area("app-menu-list")
                        .track(&self.titlebar.menu.scroll)
                        .children(items),
                ),
            );
        if let Some(focus) = &self.titlebar.menu.focus {
            panel = panel.track_focus(focus);
        }
        panel.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{PrevTab, RunCommand, ToggleRightPanel};
    use gpui_kit::KeyBinding;

    #[test]
    fn accel_labels_read_like_linux_chords() {
        let keymap = Keymap::new(vec![
            KeyBinding::new("ctrl-alt-shift-b", ToggleRightPanel, None),
            KeyBinding::new("ctrl-alt-left", PrevTab, None),
        ]);
        let label = |a: &dyn Action| accel_label(&keymap, a);
        assert_eq!(
            label(&ToggleRightPanel).as_deref(),
            Some("Ctrl+Alt+Shift+B")
        );
        assert_eq!(label(&PrevTab).as_deref(), Some("Ctrl+Alt+Left"));
        assert_eq!(label(&RunCommand), None);
    }
}
