//! The in-window application menu. GPUI draws no menubar off macOS, so a titlebar button opens
//! the menus [`crate::menu::app_menus`] builds, read back from `cx.get_menus()`: same items,
//! same enabled state and theme radios, same actions. Submenus expand in place.

use std::collections::HashSet;

use gpui_kit::{
    AnyElement, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, OwnedMenuItem,
    ParentElement, StatefulInteractiveElement, Styled, Window, deferred, div,
    prelude::FluentBuilder, px,
};
use recur_ui::guard::Guarded;
use recur_ui::text::text;
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
    focus: Option<FocusHandle>,
    /// Focus before the menu opened, restored when it closes.
    restore: Option<FocusHandle>,
    /// A press on the button while open closes through the button, not the click-outside.
    over_button: bool,
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
                    this.open_app_menu(window, cx);
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
                    d.child(titled("tb-menu-tip", "Menu", button))
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

    fn open_app_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let menu = &mut self.titlebar.menu;
        menu.open = true;
        menu.expanded.clear();
        menu.restore = window.focused(cx);
        let focus = menu.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        window.focus(&focus, cx);
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

    fn app_menu_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let menus = cx.get_menus().unwrap_or_default();
        let top: Vec<OwnedMenuItem> = menus.into_iter().map(OwnedMenuItem::Submenu).collect();
        let mut rows = Vec::new();
        self.app_menu_rows(&top, "", 0, &mut rows, cx);
        let inset = f32::from(window.client_inset().unwrap_or_default());
        let max_h = f32::from(window.viewport_size().height) - inset * 2. - 64.;
        let mut panel = style::menu_panel(cx)
            .id("app-menu")
            .debug_selector(|| "app-menu".into())
            .w(px(260.))
            .max_h(px(max_h.max(120.)))
            .occlude()
            .on_mouse_down_out(cx.guarded_listener(|this, _, window, cx| {
                if !this.titlebar.menu.over_button {
                    this.close_app_menu(window, cx);
                }
            }))
            .on_key_down(cx.guarded_listener(|this, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    this.close_app_menu(window, cx);
                    cx.stop_propagation();
                }
            }))
            // The card's background is a child: scroll the rows inside it, not the panel.
            .child(
                div()
                    .flex_auto()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(scroll_area("app-menu-list").child(div().children(rows))),
            );
        if let Some(focus) = &self.titlebar.menu.focus {
            panel = panel.track_focus(focus);
        }
        panel.into_any_element()
    }

    fn app_menu_rows(
        &self,
        items: &[OwnedMenuItem],
        parent: &str,
        depth: usize,
        out: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let t = Theme::of(cx).tokens;
        for item in items {
            let ix = out.len();
            let mark = |name: Option<IconName>| match name {
                Some(name) => icon(name).size(px(14.)).color(t.amber).into_any_element(),
                None => div().flex_none().size(px(14.)).into_any_element(),
            };
            let row = |label: &str, lead: Option<IconName>, disabled: bool| {
                let sel = format!("app-menu-row:{label}");
                div()
                    .id(("app-menu-row", ix))
                    .debug_selector(move || sel)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(6.))
                    .pr(px(10.))
                    .pl(px(10. + 14. * depth as f32))
                    .rounded(px(5.))
                    .text_size(px(12.5))
                    .text_color(if disabled { t.faint } else { t.text })
                    .when(!disabled, |d| d.cursor_pointer().hover(|s| s.bg(t.pane)))
                    .child(mark(lead))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(text(label.to_string()).truncate()),
                    )
            };
            match item {
                OwnedMenuItem::Separator => out.push(
                    div()
                        .h(px(1.))
                        .mx(px(6.))
                        .my(px(4.))
                        .bg(t.line)
                        .into_any_element(),
                ),
                OwnedMenuItem::Submenu(sub) => {
                    let path = format!("{parent}/{}", sub.name);
                    let expanded = self.titlebar.menu.expanded.contains(&path);
                    let chevron = if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    };
                    out.push(
                        row(&sub.name, None, false)
                            .child(icon(chevron).size(px(14.)).color(t.faint))
                            .on_click(cx.guarded_listener({
                                let path = path.clone();
                                move |this, _, _, cx| {
                                    let open = &mut this.titlebar.menu.expanded;
                                    if !open.remove(&path) {
                                        open.insert(path.clone());
                                    }
                                    cx.notify();
                                }
                            }))
                            .into_any_element(),
                    );
                    if expanded {
                        self.app_menu_rows(&sub.items, &path, depth + 1, out, cx);
                    }
                }
                OwnedMenuItem::Action {
                    name,
                    action,
                    checked,
                    disabled,
                    ..
                } => {
                    let mut r = row(name, checked.then_some(IconName::Check), *disabled);
                    if !*disabled {
                        let action = action.boxed_clone();
                        r = r.on_click(cx.guarded_listener(move |this, _, window, cx| {
                            this.close_app_menu(window, cx);
                            window.dispatch_action(action.boxed_clone(), cx);
                        }));
                    }
                    out.push(r.into_any_element());
                }
                OwnedMenuItem::SystemMenu(_) => {}
            }
        }
    }
}
