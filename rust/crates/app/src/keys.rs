//! App.svelte's window `keydown` handler. It runs after the focused element (bubble phase),
//! and after any menubar accelerator the native menu handled: on macOS an enabled menu item
//! owns its chord, so these paths cover the rest (disabled items, Linux, chords without an
//! accelerator such as ⇧⌘N).

use gpui_kit::{Context, KeyDownEvent, Keystroke, Window};
use recur_view_core::modes::ws_main_blocked;
use recur_view_core::run::workspace_dev_cmd;
use recur_view_core::{AppState, ViewName};

use crate::shell::Shell;

/// Key contexts of text entry (the web UI's `INPUT` / `TEXTAREA` / `contenteditable`, the
/// terminal's helper textarea). A view's own text widget adds `TextEntry` to its context.
pub const TEXT_ENTRY_CONTEXTS: [&str; 5] = [
    "Input",
    "RecurEditor",
    "Terminal",
    "TerminalFind",
    "TextEntry",
];

/// `isCmdOrCtrl`: ⌘ on macOS, Ctrl elsewhere.
fn cmd_or_ctrl(k: &Keystroke) -> bool {
    k.modifiers.secondary()
}

fn key_is(k: &Keystroke, key: &str) -> bool {
    k.key.eq_ignore_ascii_case(key)
}

/// `isBackquote`: the physical backquote key (Shift turns it into `~`).
fn is_backquote(k: &Keystroke) -> bool {
    k.key == "`" || k.key == "~"
}

impl Shell {
    pub(crate) fn on_key_down(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &ev.keystroke;
        let m = k.modifiers;
        let state = self.state.clone();
        let handled = if cmd_or_ctrl(k) && key_is(k, "k") && !m.shift {
            state.update(cx, |s, cx| s.toggle_go_palette(cx));
            true
        } else if cmd_or_ctrl(k) && m.shift && !m.alt && key_is(k, "p") {
            // ⌘⇧P: Run Command. ⌘P stays Quick Open in the Observatory.
            state.update(cx, |s, cx| s.toggle_commands_palette(cx));
            true
        } else if cmd_or_ctrl(k) && m.shift && !m.alt && (k.key == "/" || k.key == "?") {
            // ⌘⇧/ (⌘?): the shortcuts list. Plain ⌘/ stays the editor's toggle-comment.
            state.update(cx, |s, cx| s.toggle_shortcuts(cx));
            true
        } else if cmd_or_ctrl(k) && !m.alt && key_is(k, "n") {
            // ⌘N: New workspace modal. ⇧⌘N: a blank Agent workspace. ⌥⌘N is Explorer's
            // New File.
            if m.shift {
                crate::new_workspace::create_blank_workspace(&state, cx);
            } else {
                state.update(cx, |s, cx| s.open_new_task("", None, cx));
            }
            true
        } else if cmd_or_ctrl(k) && !m.shift && m.alt && key_is(k, "o") {
            self.pick_and_open_path(cx);
            true
        } else if cmd_or_ctrl(k) && key_is(k, "o") && !m.shift && !m.alt {
            state.update(cx, |s, cx| s.toggle_open_in(cx));
            true
        } else if cmd_or_ctrl(k) && key_is(k, "b") {
            state.update(cx, |s, cx| s.toggle_rail(cx));
            true
        } else if cmd_or_ctrl(k) && !m.shift && !m.alt && k.key == "," {
            state.update(cx, |s, cx| s.go(ViewName::Settings, None, cx));
            true
        } else if cmd_or_ctrl(k) && m.alt && !m.shift && (k.key == "up" || k.key == "down") {
            // ⌥⌘↑ / ⌥⌘↓: previous / next workspace in sidebar order (wraps).
            let delta = if k.key == "down" { 1 } else { -1 };
            state.update(cx, |s, cx| s.cycle_workspace(delta, cx));
            true
        } else if cmd_or_ctrl(k) && m.alt && !m.shift && (k.key == "[" || k.key == "]") {
            // ⌥⌘[ / ⌥⌘]: previous / next code editor theme.
            let delta = if k.key == "]" { 1 } else { -1 };
            let (_, name) = state.update(cx, |s, cx| s.cycle_editor_theme(delta, cx));
            AppState::toast_editor_theme(&name, cx);
            true
        } else if cmd_or_ctrl(k) && !m.alt && key_is(k, "r") {
            // ⌘R: the Run palette. ⌘⇧R stops the only running command or opens the Stop
            // menu. ⌥⌘R belongs to the Observatory.
            self.run_shortcut(m.shift, cx)
        } else if m.control && !m.platform && !m.alt && is_backquote(k) {
            // ⌃` toggles the terminal drawer, ⌃⇧` maximizes it. The dock is per workspace.
            state.update(cx, |s, cx| {
                let id = s.detail_workspace_id().to_string();
                if id.is_empty() || ws_main_blocked(s.workspace(&id)) {
                    return false;
                }
                if m.shift {
                    s.toggle_terminal_dock_maximized(&id, cx);
                } else {
                    s.toggle_terminal_dock(&id, cx);
                }
                true
            })
        } else if k.key == "escape" {
            self.escape_to_back(window, cx)
        } else {
            false
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn run_shortcut(&mut self, stop: bool, cx: &mut Context<Self>) -> bool {
        self.state.update(cx, |s, cx| {
            let id = s.detail_workspace_id().to_string();
            if id.is_empty() || workspace_dev_cmd(&id, &s.workspaces, &s.projects).is_empty() {
                return false;
            }
            if ws_main_blocked(s.workspace(&id)) {
                return true;
            }
            if stop {
                s.open_stop_menu(&id, cx);
            } else {
                s.open_run_menu(&id, cx);
            }
            true
        })
    }

    /// Esc-to-back: skipped while an overlay owns Escape or while typing; closes the rail
    /// flyout first.
    fn escape_to_back(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.state.read(cx).overlays.blocks_escape() {
            return false;
        }
        let typing = window
            .context_stack()
            .iter()
            .any(|c| TEXT_ENTRY_CONTEXTS.iter().any(|t| c.contains(t)));
        if typing {
            return false;
        }
        self.state.update(cx, |s, cx| {
            if s.rail_peek {
                s.close_rail_peek(false, cx);
                return true;
            }
            match s.view {
                ViewName::Detail | ViewName::Settings | ViewName::Welcome => {
                    s.go(ViewName::Briefing, None, cx)
                }
                ViewName::PrDetail => s.go(ViewName::Pr, None, cx),
                ViewName::Project => {
                    let to = if s.projects.is_empty() {
                        ViewName::Welcome
                    } else {
                        ViewName::Briefing
                    };
                    s.go(to, None, cx)
                }
                _ => return false,
            }
            true
        })
    }
}
