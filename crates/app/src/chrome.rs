//! Window chrome behavior: the titlebar double-click action, and the drag / no-drag regions.
//!
//! The window owns its titlebar drag (`app_owns_titlebar_drag`), so a press on bare titlebar
//! chrome moves the window and a second press of a double-click runs the system's
//! "Double-click a window's title bar to" action. Controls stop the press first ([`no_drag`]).

use gpui_kit::{InteractiveElement, MouseButton, MouseDownEvent, Window};

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
