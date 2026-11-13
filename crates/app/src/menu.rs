//! The native application menu on GPUI menus.
//!
//! Every item dispatches a GPUI action that the shell turns into a `menu.action` string
//! (`check-updates`, `toggle-sidebar`, `app-theme:<key>`, …). Contextual items
//! (New Tab, Save, Close Tab, the panel and focus toggles, Run…) follow the views'
//! `menu.state`: AppKit validates a menu item by whether its action has a handler, and the
//! shell registers a handler only while the item is enabled.
//!
//! Accelerators are native on macOS only. There the menubar owns ⌘ chords (an enabled item
//! wins over the focused pane); elsewhere the focused pane decides
//! first and the shell's key handler covers the chords (`nativeMenuAccelerators`).

use gpui_kit::{
    Action, App, KeyBinding, KeyBindingContextPredicate, Menu, MenuItem, OsAction, SharedString,
    SystemMenuType,
};
use recur_platform::identity::APP_NAME;
use recur_services::events::{MenuState, MenuThemes};
use recur_view_core::menu::{MenuGroup, MenuWorkspace};

gpui_kit::actions!(
    recur,
    [
        About,
        CheckForUpdates,
        OpenSettings,
        HideApp,
        HideOthers,
        ShowAll,
        QuitApp,
        OpenPath,
        NewWorkspace,
        NewBlankWorkspace,
        ImportConductor,
        NewTab,
        Save,
        OpenIn,
        CopyPath,
        CloseTab,
        Find,
        FindNext,
        FindPrevious,
        GoToLine,
        PrevEditorTheme,
        NextEditorTheme,
        ToggleSidebar,
        ToggleLeftPanel,
        ToggleRightPanel,
        ToggleEditor,
        ToggleTerminal,
        MaximizeTerminal,
        RunDev,
        StopRun,
        OpenDevTools,
        GoTo,
        RunCommand,
        KeyboardShortcuts,
        ReloadBundles,
        QuickOpen,
        GoToSymbol,
        GoInbox,
        GoIssues,
        GoPullRequests,
        GoSchedules,
        GoTerminals,
        GoWelcome,
        FocusAgent,
        FocusEditor,
        FocusTerminal,
        PrevTab,
        NextTab,
        PrevWorkspace,
        NextWorkspace,
        Minimize,
        Zoom,
        ZoomIn,
        ZoomOut,
        ActualSize,
        BringAllToFront,
        ToggleAppMenu,
    ]
);

/// An item that carries its `menu.action` string: the View → Appearance radios
/// (`app-theme:` / `editor-theme:` / `terminal-theme:` plus the key) and the Workspaces menu
/// rows (`open-workspace:` / `open-main:` plus the id).
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = recur, no_json)]
pub struct MenuPick {
    pub action: SharedString,
}

/// Whether menu items carry native key equivalents (`nativeMenuAccelerators`).
pub const NATIVE_MENU_ACCELERATORS: bool = cfg!(target_os = "macos");

/// A key context nothing in the window ever has: bindings in it only label menu items.
const MENU_LABEL_ONLY: &str = "RecurMenuLabelOnly";

/// The `menu.action` string a unit menu action stands for.
pub fn menu_action_name(action: &dyn Action) -> Option<&'static str> {
    macro_rules! map {
        ($($t:ty => $name:literal),* $(,)?) => {
            $( if action.as_any().is::<$t>() { return Some($name); } )*
        };
    }
    map!(
        CheckForUpdates => "check-updates",
        OpenSettings => "settings",
        OpenPath => "open-path",
        NewWorkspace => "new-loop",
        NewBlankWorkspace => "new-blank-workspace",
        ImportConductor => "import-conductor",
        NewTab => "new-tab",
        Save => "save",
        OpenIn => "open-in",
        CopyPath => "copy-path",
        CloseTab => "close-tab",
        Find => "find",
        FindNext => "find-next",
        FindPrevious => "find-previous",
        GoToLine => "goto-line",
        PrevEditorTheme => "prev-editor-theme",
        NextEditorTheme => "next-editor-theme",
        ToggleSidebar => "toggle-sidebar",
        ToggleLeftPanel => "toggle-left-panel",
        ToggleRightPanel => "toggle-right-panel",
        ToggleEditor => "toggle-editor",
        ToggleTerminal => "toggle-terminal",
        MaximizeTerminal => "maximize-terminal",
        RunDev => "run-dev",
        StopRun => "stop-run",
        GoTo => "palette",
        RunCommand => "run-command",
        KeyboardShortcuts => "shortcuts",
        ReloadBundles => "reload-bundles",
        QuickOpen => "quick-open",
        GoToSymbol => "go-symbol",
        GoInbox => "view:briefing",
        GoIssues => "view:issues",
        GoPullRequests => "view:pr",
        GoSchedules => "view:schedules",
        GoTerminals => "view:terminals",
        GoWelcome => "view:welcome",
        FocusAgent => "focus-agent",
        FocusEditor => "focus-editor",
        FocusTerminal => "focus-terminal",
        PrevTab => "prev-tab",
        NextTab => "next-tab",
        PrevWorkspace => "prev-workspace",
        NextWorkspace => "next-workspace",
        ZoomIn => "zoom-in",
        ZoomOut => "zoom-out",
        ActualSize => "zoom-reset",
    );
    None
}

/// The menu items' chords. On macOS they are the menubar's accelerators. Elsewhere the shell
/// and the panes match the same chords (Ctrl for ⌘) themselves, so they are bound label-only,
/// for the in-app menu, and a lone Alt opens that menu.
pub fn bind_menu_keys(cx: &mut App) {
    let mac = NATIVE_MENU_ACCELERATORS;
    let mut keys: Vec<(&str, Box<dyn Action>)> = vec![
        ("cmd-,", Box::new(OpenSettings)),
        ("cmd-alt-o", Box::new(OpenPath)),
        ("cmd-n", Box::new(NewWorkspace)),
        ("cmd-t", Box::new(NewTab)),
        ("cmd-s", Box::new(Save)),
        ("cmd-o", Box::new(OpenIn)),
        ("cmd-w", Box::new(CloseTab)),
        ("cmd-f", Box::new(Find)),
        ("cmd-g", Box::new(FindNext)),
        ("cmd-shift-g", Box::new(FindPrevious)),
        ("cmd-alt-g", Box::new(GoToLine)),
        ("cmd-alt-[", Box::new(PrevEditorTheme)),
        ("cmd-alt-]", Box::new(NextEditorTheme)),
        ("cmd-b", Box::new(ToggleSidebar)),
        ("cmd-alt-b", Box::new(ToggleLeftPanel)),
        ("cmd-alt-shift-b", Box::new(ToggleRightPanel)),
        ("cmd-alt-e", Box::new(ToggleEditor)),
        ("ctrl-`", Box::new(ToggleTerminal)),
        // The menu shows the first binding (⌃⇧`).
        ("ctrl-shift-`", Box::new(MaximizeTerminal)),
        ("cmd-r", Box::new(RunDev)),
        ("cmd-shift-r", Box::new(StopRun)),
        ("cmd-k", Box::new(GoTo)),
        ("cmd-shift-p", Box::new(RunCommand)),
        ("cmd-shift-/", Box::new(KeyboardShortcuts)),
        ("cmd-p", Box::new(QuickOpen)),
        ("cmd-shift-o", Box::new(GoToSymbol)),
        ("cmd-alt-1", Box::new(GoInbox)),
        ("cmd-alt-2", Box::new(GoIssues)),
        ("cmd-alt-3", Box::new(GoPullRequests)),
        ("cmd-alt-4", Box::new(GoSchedules)),
        ("cmd-alt-5", Box::new(GoTerminals)),
        ("cmd-1", Box::new(FocusAgent)),
        ("cmd-2", Box::new(FocusEditor)),
        ("cmd-3", Box::new(FocusTerminal)),
        ("cmd-alt-left", Box::new(PrevTab)),
        ("cmd-alt-right", Box::new(NextTab)),
        ("cmd-alt-up", Box::new(PrevWorkspace)),
        ("cmd-alt-down", Box::new(NextWorkspace)),
    ];
    if mac {
        // GPUI reports a shifted punctuation key as its shifted character without Shift, so
        // ⌃⇧` arrives as ⌃~.
        keys.extend([
            ("ctrl-~", Box::new(MaximizeTerminal) as Box<dyn Action>),
            ("cmd-h", Box::new(HideApp)),
            ("cmd-alt-h", Box::new(HideOthers)),
            ("cmd-q", Box::new(QuitApp)),
            ("cmd-alt-i", Box::new(OpenDevTools)),
            ("cmd-m", Box::new(Minimize)),
        ]);
    }
    let label_only = KeyBindingContextPredicate::parse(MENU_LABEL_ONLY)
        .ok()
        .map(std::rc::Rc::new);
    let context = if mac { None } else { label_only.clone() };
    // The zoom chords label the menu only on every platform: the shell's key handler zooms,
    // after a focused PDF preview's own ⌘+ / ⌘-.
    let zoom: [(&str, Box<dyn Action>); 3] = [
        ("cmd-+", Box::new(ZoomIn)),
        ("cmd--", Box::new(ZoomOut)),
        ("cmd-0", Box::new(ActualSize)),
    ];
    let keys = keys
        .into_iter()
        .map(|(k, a)| (k, a, context.clone()))
        .chain(zoom.into_iter().map(|(k, a)| (k, a, label_only.clone())));
    for (key, action, context) in keys {
        let key = if mac {
            key.to_string()
        } else {
            key.replace("cmd-", "ctrl-")
        };
        if let Ok(binding) = KeyBinding::load(&key, action, context, false, None, cx) {
            cx.bind_keys([binding]);
        }
    }
    if !mac {
        cx.bind_keys([KeyBinding::new("alt", ToggleAppMenu, None)]);
        return;
    }
    // The Edit items dispatch the text-input actions; their shortcuts are the inputs' own.
    // These label the menu only.
    for (key, name) in [
        ("cmd-alt-shift-v", "input::Paste"),
        ("backspace", "input::Delete"),
    ] {
        if let Ok(action) = cx.build_action(name, None)
            && let Ok(binding) = KeyBinding::load(key, action, label_only.clone(), false, None, cx)
        {
            cx.bind_keys([binding]);
        }
    }
}

fn enabled(state: Option<bool>) -> bool {
    state.unwrap_or(false)
}

fn item(name: &str, action: impl Action) -> MenuItem {
    MenuItem::action(name.to_string(), action)
}

/// A contextual item: shown disabled until the views' `menu.state` enables it.
fn contextual(name: &str, action: impl Action, on: Option<bool>) -> MenuItem {
    item(name, action).disabled(!enabled(on))
}

/// A greyed-out label row.
fn label(name: &str) -> MenuItem {
    item(
        name,
        MenuPick {
            action: SharedString::default(),
        },
    )
    .disabled(true)
}

fn loading() -> MenuItem {
    label("Loading…")
}

fn pick(name: &str, action: String) -> MenuItem {
    item(
        name,
        MenuPick {
            action: action.into(),
        },
    )
}

fn radio(name: &str, prefix: &str, key: &str, checked: bool) -> MenuItem {
    pick(name, format!("{prefix}:{key}")).checked(checked)
}

/// `buildWorkspacesMenu`: the sidebar tree, so a collapsed rail on a small screen still
/// reaches every workspace. A disabled header per named group, one submenu per project
/// (Working Copy, workspaces, Scheduled).
fn workspaces_menu(groups: &[MenuGroup]) -> Menu {
    let rows = |rows: &[MenuWorkspace]| -> Vec<MenuItem> {
        rows.iter()
            .map(|w| {
                let name = if w.child {
                    format!("↳ {}", w.name)
                } else {
                    w.name.clone()
                };
                pick(&name, format!("open-workspace:{}", w.id))
            })
            .collect()
    };
    let mut items = Vec::new();
    for g in groups.iter().filter(|g| !g.projects.is_empty()) {
        if !items.is_empty() {
            items.push(MenuItem::separator());
        }
        if !g.name.is_empty() {
            items.push(label(&g.name));
        }
        for p in &g.projects {
            let mut sub = vec![pick("Working Copy", format!("open-main:{}", p.id))];
            if !p.workspaces.is_empty() {
                sub.push(MenuItem::separator());
                sub.extend(rows(&p.workspaces));
            }
            if !p.scheduled.is_empty() {
                sub.push(MenuItem::separator());
                sub.push(MenuItem::submenu(
                    Menu::new("Scheduled").items(rows(&p.scheduled)),
                ));
            }
            items.push(MenuItem::submenu(Menu::new(p.name.clone()).items(sub)));
        }
    }
    if items.is_empty() {
        items.push(label("No Projects"));
    }
    Menu::new("Workspaces").items(items)
}

/// `applyMenuThemes`: the Appearance submenus from the catalogs, with "Loading…" until they
/// arrive.
fn appearance(themes: Option<&MenuThemes>) -> Menu {
    let app: Vec<MenuItem> = match themes.filter(|t| !t.app.is_empty()) {
        None => vec![loading()],
        Some(t) => t
            .app
            .iter()
            .map(|e| radio(&e.name, "app-theme", &e.key, e.key == t.app_selected))
            .collect(),
    };
    let (mut dark, mut light) = (Vec::new(), Vec::new());
    if let Some(t) = themes {
        for e in &t.editor {
            let entry = radio(&e.name, "editor-theme", &e.key, e.key == t.editor_selected);
            if e.kind == "light" {
                light.push(entry);
            } else {
                dark.push(entry);
            }
        }
    }
    if dark.is_empty() {
        dark.push(loading());
    }
    if light.is_empty() {
        light.push(loading());
    }
    let terminal: Vec<MenuItem> = match themes.filter(|t| !t.terminal.is_empty()) {
        None => vec![loading()],
        Some(t) => t
            .terminal
            .iter()
            .map(|e| {
                radio(
                    &e.name,
                    "terminal-theme",
                    &e.key,
                    e.key == t.terminal_selected,
                )
            })
            .collect(),
    };
    Menu::new("Appearance").items([
        MenuItem::submenu(Menu::new("App Theme").items(app)),
        MenuItem::submenu(Menu::new("Code Editor Theme").items([
            MenuItem::submenu(Menu::new("Dark").items(dark)),
            MenuItem::submenu(Menu::new("Light").items(light)),
        ])),
        MenuItem::submenu(Menu::new("Terminal Theme").items(terminal)),
        MenuItem::separator(),
        item("Previous Editor Theme", PrevEditorTheme),
        item("Next Editor Theme", NextEditorTheme),
    ])
}

fn edit_item(name: &str, action: &str, os: Option<OsAction>, cx: &App) -> Option<MenuItem> {
    let action = cx.build_action(action, None).ok()?;
    Some(MenuItem::Action {
        name: name.to_string().into(),
        action,
        os_action: os,
        checked: false,
        disabled: false,
    })
}

/// The menubar, built from the current contextual state and theme catalogs (`installAppMenu`).
pub fn app_menus(
    state: &MenuState,
    themes: Option<&MenuThemes>,
    workspaces: &[MenuGroup],
    updates: bool,
    conductor: bool,
    cx: &App,
) -> Vec<Menu> {
    let mac = cfg!(target_os = "macos");
    let mut menus = Vec::new();

    if mac {
        let mut items = vec![item(&format!("About {APP_NAME}"), About)];
        if updates {
            items.push(item("Check for Updates…", CheckForUpdates));
        }
        items.extend([
            MenuItem::separator(),
            item("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            item(&format!("Hide {APP_NAME}"), HideApp),
            item("Hide Others", HideOthers),
            item("Show All", ShowAll),
            MenuItem::separator(),
            item(&format!("Quit {APP_NAME}"), QuitApp),
        ]);
        menus.push(Menu::new(APP_NAME).items(items));
    }

    let mut file = vec![
        item("Open…", OpenPath),
        item("New Workspace", NewWorkspace),
        // No ⇧⌘N accelerator: the menubar would own it and Explorer's ⇧⌘N New Folder would
        // break. The shell owns ⇧⌘N and yields to the Files pane.
        item("New Blank Workspace", NewBlankWorkspace),
        contextual("New Tab", NewTab, state.new_tab),
        contextual("Save", Save, state.save),
        MenuItem::separator(),
    ];
    if conductor {
        file.extend([
            item("Import Conductor Workspaces…", ImportConductor),
            MenuItem::separator(),
        ]);
    }
    file.extend([
        contextual("Open In…", OpenIn, state.open_in),
        contextual("Copy Path", CopyPath, state.copy_path),
        MenuItem::separator(),
        // Contextual close: terminal / agent / file tab. ⌘W is the tab-close chord.
        contextual("Close Tab", CloseTab, state.close_tab),
    ]);
    if !mac {
        // Not the Quit role: it binds Ctrl+Q, the shell's XON. Close the window to quit.
        file.extend([MenuItem::separator(), item("Quit", QuitApp)]);
    }
    menus.push(Menu::new("File").items(file));

    if NATIVE_MENU_ACCELERATORS {
        // The text-input actions route Undo/Cut/Copy/Paste/Select All to the focused field.
        let edit = [
            edit_item("Undo", "input::Undo", Some(OsAction::Undo), cx),
            edit_item("Redo", "input::Redo", Some(OsAction::Redo), cx),
            Some(MenuItem::separator()),
            edit_item("Cut", "input::Cut", Some(OsAction::Cut), cx),
            edit_item("Copy", "input::Copy", Some(OsAction::Copy), cx),
            edit_item("Paste", "input::Paste", Some(OsAction::Paste), cx),
            edit_item("Paste and Match Style", "input::Paste", None, cx),
            edit_item("Delete", "input::Delete", None, cx),
            edit_item(
                "Select All",
                "input::SelectAll",
                Some(OsAction::SelectAll),
                cx,
            ),
            Some(MenuItem::separator()),
            Some(MenuItem::submenu(Menu::new("Find").items([
                contextual("Find…", Find, state.find),
                contextual("Find Next", FindNext, state.find),
                contextual("Find Previous", FindPrevious, state.find),
            ]))),
        ];
        menus.push(Menu::new("Edit").items(edit.into_iter().flatten()));
    }

    let mut view = vec![
        MenuItem::submenu(appearance(themes)),
        MenuItem::separator(),
        item("Toggle Sidebar", ToggleSidebar),
        contextual(
            "Toggle Left Panel",
            ToggleLeftPanel,
            state.toggle_left_panel,
        ),
        contextual(
            "Toggle Right Panel",
            ToggleRightPanel,
            state.toggle_right_panel,
        ),
        contextual("Toggle Editor", ToggleEditor, state.toggle_editor),
        contextual("Toggle Terminal", ToggleTerminal, state.toggle_terminal),
        contextual(
            "Maximize Terminal",
            MaximizeTerminal,
            state.maximize_terminal,
        ),
        contextual("Run…", RunDev, state.run_dev),
        contextual("Stop Run", StopRun, state.stop_run),
        MenuItem::separator(),
        item("Zoom In", ZoomIn),
        item("Zoom Out", ZoomOut),
        item("Actual Size", ActualSize),
    ];
    if !mac {
        view.extend([MenuItem::separator(), item("Settings…", OpenSettings)]);
    }
    if crate::INSPECTOR_AVAILABLE {
        view.extend([
            MenuItem::separator(),
            item("Open Developer Tools", OpenDevTools),
        ]);
    }
    menus.push(Menu::new("View").items(view));

    menus.push(Menu::new("Navigate").items([
        item("Go to…", GoTo),
        item("Run Command…", RunCommand),
        item("Keyboard Shortcuts…", KeyboardShortcuts),
        item("Reload Bundles", ReloadBundles),
        item("Quick Open…", QuickOpen),
        item("Go to Symbol…", GoToSymbol),
        contextual("Go to Line…", GoToLine, state.goto_line),
        MenuItem::separator(),
        // Sidebar destinations, so a collapsed rail on a small screen costs nothing.
        item("Inbox", GoInbox),
        item("Issues", GoIssues),
        item("Pull Requests", GoPullRequests),
        item("Schedules", GoSchedules),
        contextual("Terminals", GoTerminals, state.go_terminals),
        item("Get Started", GoWelcome),
        MenuItem::separator(),
        contextual("Focus Agent Panel", FocusAgent, state.focus_agent),
        contextual("Focus Editor", FocusEditor, state.focus_editor),
        contextual("Focus Terminal", FocusTerminal, state.focus_terminal),
        MenuItem::separator(),
        contextual("Previous Tab", PrevTab, state.prev_tab),
        contextual("Next Tab", NextTab, state.next_tab),
        MenuItem::separator(),
        item("Previous Workspace", PrevWorkspace),
        item("Next Workspace", NextWorkspace),
    ]));
    menus.push(workspaces_menu(workspaces));

    if NATIVE_MENU_ACCELERATORS {
        menus.push(Menu::new("Window").items([
            item("Minimize", Minimize),
            item("Zoom", Zoom),
            MenuItem::separator(),
            item("Bring All to Front", BringAllToFront),
        ]));
    }
    menus
}

static REBUILDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static REBUILD_NANOS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Native menubar rebuilds so far and the time they took (each lays out the whole menubar in
/// AppKit on the UI thread).
pub fn rebuilds() -> (u64, std::time::Duration) {
    use std::sync::atomic::Ordering;
    (
        REBUILDS.load(Ordering::Relaxed),
        std::time::Duration::from_nanos(REBUILD_NANOS.load(Ordering::Relaxed)),
    )
}

pub(crate) fn count_rebuild(took: std::time::Duration) {
    use std::sync::atomic::Ordering;
    REBUILDS.fetch_add(1, Ordering::Relaxed);
    REBUILD_NANOS.fetch_add(took.as_nanos() as u64, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_actions_map_to_menu_action_names() {
        assert_eq!(menu_action_name(&NewWorkspace), Some("new-loop"));
        assert_eq!(menu_action_name(&GoTo), Some("palette"));
        assert_eq!(menu_action_name(&ToggleSidebar), Some("toggle-sidebar"));
        assert_eq!(menu_action_name(&GoPullRequests), Some("view:pr"));
        assert_eq!(menu_action_name(&GoToLine), Some("goto-line"));
        assert_eq!(menu_action_name(&About), None);
    }

    #[gpui::test]
    fn accel_follows_native_menu_accelerators(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            bind_menu_keys(cx);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let save = keymap
                .bindings_for_action(&Save)
                .next()
                .expect("Save chord");
            // Label-only off macOS: the shell and panes own the chord there.
            assert_eq!(save.predicate().is_none(), NATIVE_MENU_ACCELERATORS);
        });
    }
}
