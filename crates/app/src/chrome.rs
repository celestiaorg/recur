//! Window chrome behavior: the titlebar double-click action, the drag / no-drag regions, and
//! the Linux window controls.
//!
//! The window owns its titlebar drag (`app_owns_titlebar_drag`), so a press on bare titlebar
//! chrome moves the window and a second press of a double-click runs the system's
//! "Double-click a window's title bar to" action. Controls stop the press first ([`no_drag`]).
//!
//! On Linux the window asks for client-side decorations, so the WM draws no title bar: the
//! titlebar carries [`window_controls`], and gpui-component's `Root` draws the resize border
//! and shadow. Where the WM can't do client-side decorations (X11 without a compositor or
//! `_GTK_FRAME_EXTENTS`), GPUI falls back to server decorations and the controls stay out.

use gpui_kit::{
    AnyElement, App, Decorations, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use recur_ui::{IconName, icon, style};

/// macOS Desktop & Dock → "Double-click a window's title bar to". Observed
/// `AppleActionOnDoubleClick` values: Fill, Maximize, Minimize, None. Unset or unknown zooms,
/// matching AppKit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarClickAction {
    Zoom,
    Minimize,
    None,
}

pub fn parse_titlebar_double_click_action(pref: &str) -> TitlebarClickAction {
    match pref.trim().to_lowercase().as_str() {
        "minimize" => TitlebarClickAction::Minimize,
        "none" => TitlebarClickAction::None,
        _ => TitlebarClickAction::Zoom,
    }
}

#[cfg(target_os = "macos")]
fn current_titlebar_double_click_pref() -> String {
    use objc2_foundation::{NSString, NSUserDefaults};
    NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str("AppleActionOnDoubleClick"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn current_titlebar_double_click_pref() -> String {
    String::new()
}

pub fn titlebar_double_click_action() -> TitlebarClickAction {
    parse_titlebar_double_click_action(&current_titlebar_double_click_pref())
}

/// `applyTitlebarDoubleClick`: AppKit's own titlebar does nothing while fullscreen. Zoom, not
/// maximize, so the min-size constraint stays.
pub fn apply_titlebar_double_click(window: &mut Window) {
    if window.is_fullscreen() {
        return;
    }
    match titlebar_double_click_action() {
        TitlebarClickAction::Minimize => window.minimize_window(),
        TitlebarClickAction::None => {}
        TitlebarClickAction::Zoom => window.zoom_window(),
    }
}

/// A press on draggable titlebar chrome: the second press of a double-click runs the system
/// action; otherwise the window starts moving with the pointer.
pub fn titlebar_mouse_down(event: &MouseDownEvent, zoom: bool, window: &mut Window) {
    if event.click_count == 2 {
        if zoom {
            apply_titlebar_double_click(window);
        }
        return;
    }
    if event.click_count == 1 {
        window.start_window_move();
    }
}

/// `-webkit-app-region: no-drag`: presses on this element neither move the window nor count
/// toward a titlebar double-click.
pub fn no_drag<E: InteractiveElement>(el: E) -> E {
    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// Minimize, maximize and close for a client-decorated window; `None` when the WM draws them.
/// Close quits, as the last window closing does.
pub fn window_controls(window: &Window, cx: &App) -> Option<AnyElement> {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return None;
    }
    let supports = window.window_controls();
    let button = |id: &'static str, name: IconName| {
        no_drag(style::titlebar_icon_button(id, false, cx)).child(icon(name).size(px(14.)))
    };
    Some(
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .when(supports.minimize, |d| {
                d.child(
                    button("tb-minimize", IconName::Minus)
                        .on_click(|_, window, _| window.minimize_window()),
                )
            })
            .when(supports.maximize, |d| {
                d.child(
                    button("tb-maximize", IconName::Square)
                        .on_click(|_, window, _| window.zoom_window()),
                )
            })
            .child(button("tb-close", IconName::X).on_click(|_, _, cx| cx.quit()))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_double_click_action() {
        let cases = [
            ("", TitlebarClickAction::Zoom),
            ("Maximize", TitlebarClickAction::Zoom),
            ("maximize", TitlebarClickAction::Zoom),
            ("Fill", TitlebarClickAction::Zoom),
            ("FILL", TitlebarClickAction::Zoom),
            ("Minimize", TitlebarClickAction::Minimize),
            ("  minimize  ", TitlebarClickAction::Minimize),
            ("None", TitlebarClickAction::None),
            ("none", TitlebarClickAction::None),
            ("bogus", TitlebarClickAction::Zoom),
        ];
        for (input, want) in cases {
            assert_eq!(parse_titlebar_double_click_action(input), want, "{input:?}");
        }
    }
}
