//! Native menubar plumbing shared by the shell and views (`lib/menu.ts`): the action names,
//! the contextual items' enabled state, and the Appearance catalogs. The shell builds the
//! menu (`recur-app`); views push enabled state and subscribe to actions here.

use gpui_kit::{Context, SharedString};
use recur_editor::theme::catalog::compare_for_cycle;
use recur_services::events::{MenuState, MenuTheme, MenuThemeState, MenuThemes};
use recur_ui::theme::ThemeId;

use crate::state::{AppState, ShellEvent};

/// The Terminal Theme submenu's "Match App" row (the empty terminal theme id).
pub const MENU_TERMINAL_MATCH: &str = "match-app";

fn terminal_menu_key(id: &str) -> String {
    if id.is_empty() {
        MENU_TERMINAL_MATCH.into()
    } else {
        id.into()
    }
}

/// The terminal theme id behind a `terminal-theme:` menu key.
pub fn terminal_id_from_menu(key: &str) -> String {
    if key == MENU_TERMINAL_MATCH {
        String::new()
    } else {
        key.into()
    }
}

/// `MENU_STATE_IDLE`: every Observatory-scoped item off (leaving a workspace view).
pub fn menu_state_idle() -> MenuState {
    MenuState {
        new_tab: Some(false),
        save: Some(false),
        open_in: Some(false),
        copy_path: Some(false),
        close_tab: Some(false),
        toggle_left_panel: Some(false),
        toggle_right_panel: Some(false),
        toggle_editor: Some(false),
        toggle_terminal: Some(false),
        maximize_terminal: Some(false),
        focus_agent: Some(false),
        focus_editor: Some(false),
        focus_terminal: Some(false),
        prev_tab: Some(false),
        next_tab: Some(false),
        run_dev: Some(false),
        stop_run: Some(false),
    }
}

/// Fold a partial update in: a missing key leaves that item alone (`applyMenuState`).
pub fn merge_menu_state(into: &mut MenuState, patch: &MenuState) -> bool {
    let before = into.clone();
    macro_rules! merge {
        ($($f:ident),*) => { $( if let Some(v) = patch.$f { into.$f = Some(v); } )* };
    }
    merge!(
        new_tab,
        save,
        open_in,
        copy_path,
        close_tab,
        toggle_left_panel,
        toggle_right_panel,
        toggle_editor,
        toggle_terminal,
        maximize_terminal,
        focus_agent,
        focus_editor,
        focus_terminal,
        prev_tab,
        next_tab,
        run_dev,
        stop_run
    );
    *into != before
}

impl AppState {
    /// `setMenuState`: push enable/disable state for contextual menubar items.
    pub fn set_menu_state(&mut self, patch: MenuState, cx: &mut Context<Self>) {
        if merge_menu_state(&mut self.menu_state, &patch) {
            self.changed(crate::topics::Topics::MENU, cx);
        }
    }

    /// Fan a menubar action out (`menu.action`). The shell handles the app-level ones;
    /// views subscribe for what they own.
    pub fn dispatch_menu_action(
        &mut self,
        action: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        cx.emit(ShellEvent::MenuAction(action.into()));
    }

    /// `pushMenuThemes`: the View → Appearance radio menus.
    pub fn menu_themes(&self) -> MenuThemes {
        let mut editor: Vec<_> = self.editor_themes_for_picker();
        editor.sort_by(compare_for_cycle);
        let mut terminal = vec![MenuTheme {
            key: MENU_TERMINAL_MATCH.into(),
            name: "Match App".into(),
            kind: String::new(),
        }];
        terminal.extend(self.terminal_theme_catalog.iter().map(|t| MenuTheme {
            key: t.id.clone(),
            name: t.name.clone(),
            kind: String::new(),
        }));
        MenuThemes {
            app: ThemeId::ALL
                .iter()
                .map(|t| MenuTheme {
                    key: t.key().into(),
                    name: t.name().into(),
                    kind: String::new(),
                })
                .collect(),
            editor: editor
                .into_iter()
                .map(|c| MenuTheme {
                    key: c.key,
                    name: c.name,
                    kind: if c.dark { "dark" } else { "light" }.into(),
                })
                .collect(),
            terminal,
            app_selected: self.appearance.theme.clone(),
            editor_selected: self.editor_theme.clone(),
            terminal_selected: terminal_menu_key(&self.terminal_prefs.theme),
        }
    }

    /// `pushMenuThemeState`: the Appearance radio checks.
    pub fn menu_theme_state(&self) -> MenuThemeState {
        MenuThemeState {
            chrome_dark: !crate::themes::is_light_theme(&self.appearance.theme),
            app_selected: self.appearance.theme.clone(),
            editor_selected: self.editor_theme.clone(),
            terminal_selected: terminal_menu_key(&self.terminal_prefs.theme),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_state_merges() {
        let mut st = menu_state_idle();
        let changed = merge_menu_state(
            &mut st,
            &MenuState {
                open_in: Some(true),
                copy_path: Some(true),
                ..MenuState::default()
            },
        );
        assert!(changed);
        assert_eq!(st.open_in, Some(true));
        assert_eq!(st.save, Some(false));
        assert!(!merge_menu_state(&mut st, &MenuState::default()));
    }

    #[test]
    fn terminal_match_key() {
        assert_eq!(terminal_menu_key(""), "match-app");
        assert_eq!(terminal_id_from_menu("match-app"), "");
        assert_eq!(terminal_id_from_menu("dracula"), "dracula");
    }
}
