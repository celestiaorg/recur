//! The native application menu (`app_menu.go`) on GPUI menus.
//!
//! Every item dispatches a GPUI action that the shell turns into the `menu.action` string the
//! web UI handled (`check-updates`, `toggle-sidebar`, `app-theme:<key>`, …). Contextual items
//! (New Tab, Save, Close Tab, the panel and focus toggles, Run…) follow the frontend's
//! `menu.state`: AppKit validates a menu item by whether its action has a handler, and the
//! shell registers a handler only while the item is enabled.
//!
//! Accelerators are native on macOS only. There the menubar owns ⌘ chords (an enabled item
//! wins over the focused pane, as WKWebView behaved); elsewhere the focused pane decides
//! first and the shell's key handler covers the chords (`nativeMenuAccelerators`).

use gpui_kit::{
    Action, App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, Menu, MenuItem,
    OsAction, SharedString, SystemMenuType,
};
use recur_platform::identity::APP_NAME;
use recur_services::events::{MenuState, MenuThemes};

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
        NewTab,
        Save,
        OpenIn,
        CopyPath,
        CloseTab,
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
        FocusAgent,
        FocusEditor,
        FocusTerminal,
        PrevTab,
        NextTab,
        PrevWorkspace,
        NextWorkspace,
        Minimize,
        Zoom,
        BringAllToFront,
    ]
);

/// A View → Appearance radio item: `app-theme:` / `editor-theme:` / `terminal-theme:` plus
/// the key.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = recur, no_json)]
pub struct PickTheme {
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
        NewTab => "new-tab",
        Save => "save",
        OpenIn => "open-in",
        CopyPath => "copy-path",
        CloseTab => "close-tab",
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
        FocusAgent => "focus-agent",
        FocusEditor => "focus-editor",
        FocusTerminal => "focus-terminal",
        PrevTab => "prev-tab",
        NextTab => "next-tab",
        PrevWorkspace => "prev-workspace",
        NextWorkspace => "next-workspace",
    );
    None
}

/// The menubar accelerators (`accel(item, chord)` in `app_menu.go`), registered on macOS.
pub fn bind_menu_keys(cx: &mut App) {
    if !NATIVE_MENU_ACCELERATORS {
        return;
    }
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-q", QuitApp, None),
        KeyBinding::new("cmd-alt-o", OpenPath, None),
        KeyBinding::new("cmd-n", NewWorkspace, None),
        KeyBinding::new("cmd-t", NewTab, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-o", OpenIn, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("cmd-alt-[", PrevEditorTheme, None),
        KeyBinding::new("cmd-alt-]", NextEditorTheme, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-alt-b", ToggleLeftPanel, None),
        KeyBinding::new("cmd-alt-shift-b", ToggleRightPanel, None),
        KeyBinding::new("cmd-alt-e", ToggleEditor, None),
        KeyBinding::new("ctrl-`", ToggleTerminal, None),
        KeyBinding::new("ctrl-shift-`", MaximizeTerminal, None),
        KeyBinding::new("cmd-r", RunDev, None),
        KeyBinding::new("cmd-shift-r", StopRun, None),
        KeyBinding::new("cmd-alt-i", OpenDevTools, None),
        KeyBinding::new("cmd-k", GoTo, None),
        KeyBinding::new("cmd-shift-p", RunCommand, None),
        KeyBinding::new("cmd-shift-/", KeyboardShortcuts, None),
        KeyBinding::new("cmd-p", QuickOpen, None),
        KeyBinding::new("cmd-shift-o", GoToSymbol, None),
        KeyBinding::new("cmd-1", FocusAgent, None),
        KeyBinding::new("cmd-2", FocusEditor, None),
        KeyBinding::new("cmd-3", FocusTerminal, None),
        KeyBinding::new("cmd-alt-left", PrevTab, None),
        KeyBinding::new("cmd-alt-right", NextTab, None),
        KeyBinding::new("cmd-alt-up", PrevWorkspace, None),
        KeyBinding::new("cmd-alt-down", NextWorkspace, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
    // The Edit items dispatch the text-input actions; their shortcuts are the inputs' own.
    // These label the menu only.
    let label_only = KeyBindingContextPredicate::parse(MENU_LABEL_ONLY)
        .ok()
        .map(std::rc::Rc::new);
    for (key, name) in [
        ("cmd-alt-shift-v", "input::Paste"),
        ("backspace", "input::Delete"),
    ] {
        if let Ok(action) = cx.build_action(name, None)
            && let Ok(binding) = KeyBinding::load(
                key,
                action,
                label_only.clone(),
                false,
                None,
                &DummyKeyboardMapper,
            )
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

/// A contextual item: shown disabled until the frontend's `menu.state` enables it.
fn contextual(name: &str, action: impl Action, on: Option<bool>) -> MenuItem {
    item(name, action).disabled(!enabled(on))
}

fn loading() -> MenuItem {
    item("Loading…", PickTheme {
        action: SharedString::default(),
    })
    .disabled(true)
}

fn radio(name: &str, prefix: &str, key: &str, checked: bool) -> MenuItem {
    item(
        name,
        PickTheme {
            action: format!("{prefix}:{key}").into(),
        },
    )
    .checked(checked)
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
            .map(|e| radio(&e.name, "terminal-theme", &e.key, e.key == t.terminal_selected))
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
pub fn app_menus(state: &MenuState, themes: Option<&MenuThemes>, updates: bool, cx: &App) -> Vec<Menu> {
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
        contextual("Open In…", OpenIn, state.open_in),
        contextual("Copy Path", CopyPath, state.copy_path),
        MenuItem::separator(),
        // Contextual close: terminal / agent / file tab. ⌘W is the tab-close chord.
        contextual("Close Tab", CloseTab, state.close_tab),
    ];
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
            edit_item("Select All", "input::SelectAll", Some(OsAction::SelectAll), cx),
        ];
        menus.push(Menu::new("Edit").items(edit.into_iter().flatten()));
    }

    let mut view = vec![
        MenuItem::submenu(appearance(themes)),
        MenuItem::separator(),
        item("Toggle Sidebar", ToggleSidebar),
        contextual("Toggle Left Panel", ToggleLeftPanel, state.toggle_left_panel),
        contextual("Toggle Right Panel", ToggleRightPanel, state.toggle_right_panel),
        contextual("Toggle Editor", ToggleEditor, state.toggle_editor),
        contextual("Toggle Terminal", ToggleTerminal, state.toggle_terminal),
        contextual("Maximize Terminal", MaximizeTerminal, state.maximize_terminal),
        contextual("Run…", RunDev, state.run_dev),
        contextual("Stop Run", StopRun, state.stop_run),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_actions_map_to_menu_action_names() {
        assert_eq!(menu_action_name(&NewWorkspace), Some("new-loop"));
        assert_eq!(menu_action_name(&GoTo), Some("palette"));
        assert_eq!(menu_action_name(&ToggleSidebar), Some("toggle-sidebar"));
        assert_eq!(menu_action_name(&About), None);
    }
}
