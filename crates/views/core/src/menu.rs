//! Native menubar plumbing shared by the shell and views: the action names,
//! the contextual items' enabled state, and the Appearance catalogs. The shell builds the
//! menu (`recur-app`); views push enabled state and subscribe to actions here.

use gpui_kit::{Context, SharedString};
use recur_domain::{Project, ProjectGroup, Workspace};
use recur_editor::theme::catalog::compare_for_cycle;
use recur_services::events::{MenuState, MenuTheme, MenuThemeState, MenuThemes};
use recur_ui::theme::ThemeId;

use crate::nav::ViewName;
use crate::state::{AppState, ShellEvent};
use crate::workspace_nav::{is_nested_child, nest_workspaces, split_scheduled, unique_refs};

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
        find: Some(false),
        goto_line: Some(false),
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
        stop_run,
        find,
        goto_line
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

/// Navigate → sidebar destinations; the first four are also ⌥⌘1–4 in order.
pub const MENU_VIEWS: [ViewName; 5] = [
    ViewName::Briefing,
    ViewName::Issues,
    ViewName::Pr,
    ViewName::Schedules,
    ViewName::Welcome,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuWorkspace {
    pub id: String,
    pub name: String,
    pub child: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuProject {
    pub id: String,
    pub name: String,
    pub workspaces: Vec<MenuWorkspace>,
    pub scheduled: Vec<MenuWorkspace>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuGroup {
    pub name: String,
    pub projects: Vec<MenuProject>,
}

/// `menuWorkspaceTree`: the Workspaces menu in sidebar order, groups then ungrouped projects
/// (name `""`).
pub fn menu_workspace_tree(
    groups: &[ProjectGroup],
    projects: &[Project],
    workspaces: &[Workspace],
    show_hidden: bool,
) -> Vec<MenuGroup> {
    let loops: Vec<&Workspace> = unique_refs(workspaces)
        .into_iter()
        .filter(|w| !w.archived && !w.is_main && w.host_pr_id.trim().is_empty())
        .collect();
    let rows = |list: &[&Workspace]| -> Vec<MenuWorkspace> {
        nest_workspaces(list)
            .into_iter()
            .map(|w| MenuWorkspace {
                id: w.id.clone(),
                name: w.short.clone(),
                child: is_nested_child(w, list),
            })
            .collect()
    };
    let to_project = |p: &Project| {
        let mine: Vec<&Workspace> = loops
            .iter()
            .copied()
            .filter(|w| w.project_id == p.id)
            .collect();
        let (regular, scheduled) = split_scheduled(&mine);
        MenuProject {
            id: p.id.clone(),
            name: p.name.clone(),
            workspaces: rows(&regular),
            scheduled: rows(&scheduled),
        }
    };
    let shown: Vec<&Project> = unique_refs(projects)
        .into_iter()
        .filter(|p| show_hidden || !p.hidden)
        .collect();
    let ug = unique_refs(groups);
    let mut out: Vec<MenuGroup> = ug
        .iter()
        .map(|g| MenuGroup {
            name: g.name.clone(),
            projects: shown
                .iter()
                .filter(|p| p.group_id == g.id)
                .map(|p| to_project(p))
                .collect(),
        })
        .filter(|g| !g.projects.is_empty())
        .collect();
    let ungrouped: Vec<MenuProject> = shown
        .iter()
        .filter(|p| p.group_id.is_empty() || !ug.iter().any(|g| g.id == p.group_id))
        .map(|p| to_project(p))
        .collect();
    if !ungrouped.is_empty() {
        out.push(MenuGroup {
            name: String::new(),
            projects: ungrouped,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_tree_mirrors_sidebar() {
        let groups = [ProjectGroup {
            id: "g1".into(),
            name: "Work".into(),
            ..Default::default()
        }];
        let project = |id: &str, name: &str, group: &str, hidden: bool| Project {
            id: id.into(),
            name: name.into(),
            group_id: group.into(),
            hidden,
            ..Default::default()
        };
        let projects = [
            project("p1", "Alpha", "g1", false),
            project("p2", "Beta", "", false),
            project("p3", "Hidden", "", true),
        ];
        let ws = |id: &str, pid: &str, short: &str, f: &dyn Fn(&mut Workspace)| {
            let mut w = Workspace {
                id: id.into(),
                project_id: pid.into(),
                short: short.into(),
                ..Default::default()
            };
            f(&mut w);
            w
        };
        let workspaces = [
            ws("m1", "p1", "main", &|w| w.is_main = true),
            ws("w1", "p1", "one", &|_| {}),
            ws("w2", "p1", "two", &|w| w.parent_id = "w1".into()),
            ws("w3", "p1", "gone", &|w| w.archived = true),
            ws("w4", "p1", "pr chat", &|w| w.host_pr_id = "pr1".into()),
            ws("s1", "p1", "nightly", &|w| w.schedule_id = "sch".into()),
            ws("w5", "p2", "beta ws", &|_| {}),
        ];
        let row = |id: &str, name: &str, child: bool| MenuWorkspace {
            id: id.into(),
            name: name.into(),
            child,
        };
        assert_eq!(
            menu_workspace_tree(&groups, &projects, &workspaces, false),
            vec![
                MenuGroup {
                    name: "Work".into(),
                    projects: vec![MenuProject {
                        id: "p1".into(),
                        name: "Alpha".into(),
                        workspaces: vec![row("w1", "one", false), row("w2", "two", true)],
                        scheduled: vec![row("s1", "nightly", false)],
                    }],
                },
                MenuGroup {
                    name: String::new(),
                    projects: vec![MenuProject {
                        id: "p2".into(),
                        name: "Beta".into(),
                        workspaces: vec![row("w5", "beta ws", false)],
                        scheduled: vec![],
                    }],
                },
            ]
        );
        let all = menu_workspace_tree(&groups, &projects, &workspaces, true);
        let ids: Vec<&str> = all[1].projects.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["p2", "p3"]);
    }

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
