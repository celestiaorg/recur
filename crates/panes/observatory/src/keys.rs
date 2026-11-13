//! The Observatory's window-level keys (workbench focus, panels, review, tabs, split, view
//! mode, reload, bundle key equivalents, ⌘S in Preview) and the native menu's actions. They
//! run after the focused element handled the key (bubble phase); a key an
//! action binding consumed (a menu accelerator) arrives as the menu action instead.

use gpui_kit::{App, Context, KeystrokeEvent, Subscription, Window};
use recur_editor::keyequiv::{chord_from_keystroke, is_reserved_app_chord};
use recur_editor::view_state::next_editor_view_mode;

pub(crate) use crate::editor_host::commands::{ChordHit, ChordIndex};
use crate::model::{FocusZone, MainSurface, Pane, is_scratch_path};
use crate::obs::{Observatory, ObservatoryEvent};

/// Key contexts whose focus counts as typing (`INPUT`, `TEXTAREA`, contenteditable).
const TYPING_CONTEXTS: [&str; 5] = [
    "Input",
    "RecurEditor",
    "Terminal",
    "ChatComposer",
    "TerminalFind",
];

fn in_context(ev: &KeystrokeEvent, names: &[&str]) -> bool {
    ev.context_stack
        .iter()
        .any(|c| names.iter().any(|n| c.contains(n)))
}

impl Observatory {
    /// Observe every keystroke in the window while this shell is live.
    pub(crate) fn observe_keys(cx: &mut Context<Self>) -> Subscription {
        cx.observe_keystrokes(move |o, ev: &KeystrokeEvent, window, cx| {
            if ev.action.is_some() || o.window != Some(window.window_handle()) {
                return;
            }
            if o.props.shell_active {
                o.on_window_key(ev, window, cx);
            }
        })
    }

    fn in_dock(&self, window: &Window, cx: &App) -> bool {
        self.dock
            .as_ref()
            .is_some_and(|(_, d)| d.read(cx).contains_focus(window, cx))
    }

    fn overlay_open(&self, cx: &App) -> bool {
        let o = &self.state.read(cx).overlays;
        o.palette
            || o.commands
            || o.shortcuts
            || o.new_task
            || o.new_project
            || o.syntax_mode
            || recur_view_widgets::keybinding_menu::keybinding_menu_open(cx)
    }

    /// The global editor chords (Quick Open, tabs, reload, symbol, split, panel focus).
    fn on_window_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        let m = k.modifiers;
        let mod_ = m.secondary(cx);
        let lower = k.key.to_lowercase();
        let typing = in_context(ev, &TYPING_CONTEXTS);
        let in_term = self.in_dock(window, cx);
        let blocked = self.props.main_blocked;

        if self.ctx_menu.is_some() && k.key == "escape" {
            self.close_ctx(cx);
            cx.notify();
        }
        // Off macOS the menu's find chords only label it.
        if !cfg!(target_os = "macos")
            && mod_
            && !m.alt
            && self.focus_zone == FocusZone::Agent
            && (lower == "g" || (lower == "f" && !m.shift))
        {
            let dir = match (lower.as_str(), m.shift) {
                ("f", _) => 0,
                (_, true) => -1,
                _ => 1,
            };
            cx.emit(ObservatoryEvent::AgentFind(dir));
            return;
        }
        if mod_ && !m.shift && lower == "s" && self.show_preview() && self.writable() {
            self.save_edit(window, cx);
            return;
        }
        if mod_ && !m.shift && !m.alt && (lower == "1" || lower == "2" || lower == "3") {
            match lower.as_str() {
                "1" => self.focus_agent_panel(window, cx).detach(),
                _ if blocked => {}
                "2" => self.focus_code_panel(window, cx),
                _ => {
                    let token = self.root_token;
                    self.focus_terminal_panel(token, window, cx);
                }
            }
            return;
        }
        if mod_ && !m.shift && !m.alt && lower == "l" {
            if !self.props.can_add_editor_mention {
                return;
            }
            let path = self.focused_path();
            if path.is_empty() || is_scratch_path(&path) {
                return;
            }
            self.add_editor_mention_to_chat(window, cx);
            return;
        }
        if mod_ && m.alt && lower == "b" {
            if m.shift {
                if self.has_right(cx) && !blocked {
                    let open = self.show_side;
                    self.set_show_side(!open, cx);
                }
            } else {
                self.toggle_visible_left_panel(cx);
            }
            return;
        }
        if mod_ && m.alt && !m.shift && lower == "e" {
            if self.props.workspace_id.is_empty() || blocked || self.term_overlaying() {
                return;
            }
            self.toggle_editor_surface(cx);
            return;
        }
        if mod_ && m.alt && !m.shift && lower == "r" {
            if in_context(ev, &["EditorSearch", "PreviewFind", "TerminalFind"]) || in_term {
                return;
            }
            if in_context(ev, &["Input"]) {
                return;
            }
            if self.quick_open.read(cx).is_open() || self.overlay_open(cx) {
                return;
            }
            if self.props.workspace_id.is_empty()
                || blocked
                || self.term_overlaying()
                || !self.is_git(cx)
            {
                return;
            }
            let opening = self.main_surface(cx) != MainSurface::Review;
            let needs_park = if opening {
                self.main_focus.contains_focused(window, cx)
                    || self.tabs_focus.contains_focused(window, cx)
            } else {
                self.review_focus.contains_focused(window, cx)
            };
            self.toggle_review(cx);
            if needs_park {
                self.park_after_review_toggle(opening, window, cx);
            }
            return;
        }
        if mod_ && !m.shift && !m.alt && lower == "p" {
            if blocked {
                return;
            }
            self.open_quick_open(crate::quick_open::QuickOpenMode::File, window, cx);
            return;
        }
        if mod_ && m.shift && !m.alt && lower == "o" {
            if blocked {
                return;
            }
            self.open_quick_open(crate::quick_open::QuickOpenMode::Symbol, window, cx);
            return;
        }
        if mod_ && !m.shift && !m.alt && lower == "t" {
            if in_term {
                return;
            }
            if self.focus_zone == FocusZone::Agent && self.props.agent_tabs {
                cx.emit(ObservatoryEvent::AgentNewTab);
            }
            return;
        }
        if mod_ && !m.shift && !m.alt && lower == "w" {
            if in_term {
                return;
            }
            if self.focus_zone == FocusZone::Editor
                && self.main_surface(cx) == MainSurface::Editor
                && !self.selected_path.is_empty()
            {
                if blocked {
                    return;
                }
                let p = self.selected_path.clone();
                self.close_mru(&p, window, cx);
            } else if self.focus_zone == FocusZone::Agent && self.props.agent_tabs {
                cx.emit(ObservatoryEvent::AgentCloseTab);
            }
            return;
        }
        if mod_ && m.shift && !m.alt && lower == "t" && !self.closed_tabs.is_empty() {
            if blocked {
                return;
            }
            self.reopen_closed_tab(window, cx);
            return;
        }
        if mod_ && m.alt && !m.shift && (lower == "right" || lower == "left") {
            if in_term {
                return;
            }
            let dir = if lower == "right" { 1 } else { -1 };
            self.cycle_tab_in_zone(dir, window, cx);
            return;
        }
        if mod_ && k.key == "\\" {
            if blocked {
                return;
            }
            self.toggle_split(window, cx);
            return;
        }
        if (mod_ || m.control) && m.shift && !m.alt && lower == "v" {
            if in_term {
                return;
            }
            if self.focus_zone != FocusZone::Editor || blocked || self.editor_hidden(cx) {
                return;
            }
            if self.split_open && self.focused_pane == Pane::Split {
                return;
            }
            if in_context(ev, &["Input"]) || self.overlay_open(cx) {
                return;
            }
            let Some(next) = next_editor_view_mode(self.view_mode, &self.available_view_modes())
            else {
                return;
            };
            self.set_view_mode(next, window, cx).detach();
            return;
        }
        if !typing && lower == "f5" {
            if blocked {
                return;
            }
            self.reload_from_disk(window, cx);
            return;
        }
        self.on_chord_key(ev, window, cx);
    }

    /// The focused pane's path (`focusedPane === 'split' && splitOpen && splitPath`).
    pub(crate) fn focused_path(&self) -> String {
        if self.focused_pane == Pane::Split && self.split_open && !self.split_path.is_empty() {
            self.split_path.clone()
        } else {
            self.selected_path.clone()
        }
    }

    /// ⌥⌘←/→ and the menu's previous / next tab: review tabs, file tabs or agent tabs.
    fn cycle_tab_in_zone(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        let blocked = self.props.main_blocked;
        if self.focus_zone == FocusZone::Editor && self.main_surface(cx) == MainSurface::Review {
            if !blocked {
                self.cycle_review_tab(dir, cx);
            }
        } else if self.focus_zone == FocusZone::Editor && self.open_files.len() > 1 {
            if !blocked {
                self.rotate_file_tab(dir, window, cx);
            }
        } else if self.focus_zone == FocusZone::Agent
            && self.props.agent_tab_count > 1
            && self.props.agent_tabs
        {
            cx.emit(ObservatoryEvent::AgentRotateTab(dir));
        }
    }

    /// Bundle keyEquivalent dispatch (editor focus only; app-reserved chords ignored).
    fn on_chord_key(&mut self, ev: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.focused_path();
        if self.focus_zone != FocusZone::Editor || path.is_empty() || self.chord_index.path != path
        {
            return;
        }
        if self.overlay_open(cx) {
            return;
        }
        let k = &ev.keystroke;
        let Some(chord) = chord_from_keystroke(k) else {
            return;
        };
        if is_reserved_app_chord(&chord) {
            return;
        }
        let m = k.modifiers;
        let has_mod = m.platform || m.control || m.alt;
        let key_part = chord.rsplit('+').next().unwrap_or("");
        let is_fn = key_part.len() >= 2
            && key_part.len() <= 3
            && key_part.starts_with('f')
            && key_part[1..].chars().all(|c| c.is_ascii_digit());
        if !has_mod && !is_fn {
            return;
        }
        let matches = self
            .chord_index
            .hits
            .get(&chord)
            .cloned()
            .unwrap_or_default();
        if matches.is_empty() {
            return;
        }
        cx.stop_propagation();
        if matches.len() == 1 {
            let hit = matches.into_iter().next().expect("one hit");
            self.apply_chord_hit(hit, window, cx);
            return;
        }
        let items = matches.iter().map(ChordHit::item).collect();
        let weak = self.weak.clone();
        recur_view_widgets::keybinding_menu::open_keybinding_menu(
            items,
            std::rc::Rc::new(move |item, window, cx| {
                let Some(o) = weak.upgrade() else { return };
                let Some(hit) = matches.iter().find(|h| h.item().id == item.id).cloned() else {
                    return;
                };
                o.update(cx, |o, cx| o.apply_chord_hit(hit, window, cx));
            }),
            cx,
        );
    }

    /// Native menubar actions (`onMenuAction`).
    pub(crate) fn on_menu_action(
        &mut self,
        action: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.props.shell_active
            || self.state.read(cx).view == recur_view_core::ViewName::Terminals
        {
            return;
        }
        let blocked = self.props.main_blocked;
        match action {
            "focus-agent" => self.focus_agent_panel(window, cx).detach(),
            "focus-editor" if !blocked => {
                self.focus_code_panel(window, cx);
            }
            "focus-terminal" if !blocked => {
                let token = self.root_token;
                self.focus_terminal_panel(token, window, cx);
            }
            "toggle-left-panel" => self.toggle_visible_left_panel(cx),
            "toggle-right-panel" if self.has_right(cx) && !blocked => {
                let open = self.show_side;
                self.set_show_side(!open, cx);
            }
            "toggle-editor"
                if !self.props.workspace_id.is_empty() && !blocked && !self.term_overlaying() =>
            {
                self.toggle_editor_surface(cx);
            }
            "quick-open" if !blocked => {
                self.open_quick_open(crate::quick_open::QuickOpenMode::File, window, cx);
            }
            "go-symbol" if !blocked => {
                self.open_quick_open(crate::quick_open::QuickOpenMode::Symbol, window, cx);
            }
            "save" => self.save_edit(window, cx),
            "new-tab" => {
                if self.focus_zone == FocusZone::Terminal && !blocked {
                    self.dock_new_tab(window, cx);
                } else if self.focus_zone == FocusZone::Agent && self.props.agent_tabs {
                    cx.emit(ObservatoryEvent::AgentNewTab);
                }
            }
            "close-tab" => {
                if self.focus_zone == FocusZone::Terminal {
                    self.dock_close_active_tab(window, cx);
                } else if self.focus_zone == FocusZone::Editor
                    && self.main_surface(cx) == MainSurface::Editor
                    && !self.selected_path.is_empty()
                    && !blocked
                {
                    let p = self.selected_path.clone();
                    self.close_mru(&p, window, cx);
                } else if self.focus_zone == FocusZone::Agent && self.props.agent_tabs {
                    cx.emit(ObservatoryEvent::AgentCloseTab);
                }
            }
            "find" | "find-next" | "find-previous" if self.focus_zone == FocusZone::Agent => {
                cx.emit(ObservatoryEvent::AgentFind(match action {
                    "find" => 0,
                    "find-next" => 1,
                    _ => -1,
                }));
            }
            "find" | "find-next" | "find-previous" | "goto-line" => {
                use recur_editor::bindings::Cmd;
                let Some(editor) = self.menu_text_editor(cx) else {
                    return;
                };
                let cmd = match action {
                    "find" => Cmd::OpenSearchPanel,
                    "find-next" => Cmd::FindNext,
                    "find-previous" => Cmd::FindPrevious,
                    _ => Cmd::GotoLine,
                };
                editor.update(cx, |e, cx| e.run_cmd(cmd, window, cx));
                self.sync_find_panel_open(cx);
            }
            "prev-tab" | "next-tab" => {
                let dir = if action == "prev-tab" { -1 } else { 1 };
                if self.focus_zone == FocusZone::Terminal {
                    self.dock_rotate_tab(dir, window, cx);
                } else {
                    self.cycle_tab_in_zone(dir, window, cx);
                }
            }
            _ => {}
        }
    }
}
