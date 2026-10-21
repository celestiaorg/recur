//! Menubar actions on the shell: the action listeners behind each native menu item, the
//! `menu.action` switch App.svelte ran (`onMenuAction`), and the updater toasts
//! (`lib/updater.ts`).

use gpui_kit::{Context, Div, InteractiveElement, Stateful, Window};
use recur_platform::updater::UpdaterEvent;
use recur_services::AppEvent;
use recur_ui::widgets::{ToastKind, ToastOptions, push_toast, toast_action};
use recur_view_core::ViewName;
use recur_view_core::menu::terminal_id_from_menu;
use recur_view_core::modes::ws_main_blocked;

use crate::menu::*;
use crate::shell::Shell;

const UPDATER_TOAST: &str = "updater";

impl Shell {
    /// Every menu item's listener. Contextual items get theirs only while enabled, which is
    /// what makes AppKit draw them enabled.
    pub(crate) fn with_menu_actions(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let menu = self.state.read(cx).menu_state.clone();
        let detail_on = {
            let s = self.state.read(cx);
            s.view == ViewName::Detail
                && s.selected_workspace().is_some()
                && !ws_main_blocked(s.selected_workspace())
        };
        macro_rules! emit {
            ($root:expr, $($t:ty),*) => {
                $root$(.on_action(cx.listener(|this, a: &$t, window, cx| {
                    if let Some(name) = menu_action_name(a) {
                        this.emit_menu_action(name, window, cx);
                    }
                })))*
            };
        }
        let mut root = emit!(
            root,
            CheckForUpdates,
            OpenSettings,
            OpenPath,
            NewWorkspace,
            NewBlankWorkspace,
            PrevEditorTheme,
            NextEditorTheme,
            ToggleSidebar,
            GoTo,
            RunCommand,
            KeyboardShortcuts,
            ReloadBundles,
            QuickOpen,
            GoToSymbol,
            PrevWorkspace,
            NextWorkspace
        );
        macro_rules! contextual {
            ($root:expr, $($t:ty => $on:expr),*) => {{
                let mut r = $root;
                $( if $on.unwrap_or(false) {
                    r = r.on_action(cx.listener(|this, a: &$t, window, cx| {
                        if let Some(name) = menu_action_name(a) {
                            this.emit_menu_action(name, window, cx);
                        }
                    }));
                } )*
                r
            }};
        }
        root = contextual!(
            root,
            NewTab => menu.new_tab,
            Save => menu.save,
            OpenIn => Some(detail_on),
            CopyPath => Some(detail_on),
            CloseTab => menu.close_tab,
            ToggleLeftPanel => menu.toggle_left_panel,
            ToggleRightPanel => menu.toggle_right_panel,
            ToggleEditor => menu.toggle_editor,
            ToggleTerminal => menu.toggle_terminal,
            MaximizeTerminal => menu.maximize_terminal,
            RunDev => menu.run_dev,
            StopRun => menu.stop_run,
            FocusAgent => menu.focus_agent,
            FocusEditor => menu.focus_editor,
            FocusTerminal => menu.focus_terminal,
            PrevTab => menu.prev_tab,
            NextTab => menu.next_tab
        );
        root.on_action(cx.listener(|this, a: &PickTheme, window, cx| {
            if !a.action.is_empty() {
                this.emit_menu_action(&a.action, window, cx);
            }
        }))
        .on_action(cx.listener(|_, _: &About, window, cx| {
            drop(window.prompt(
                gpui_kit::PromptLevel::Info,
                recur_platform::identity::APP_NAME,
                Some(recur_platform::identity::APP_DESCRIPTION),
                &["OK"],
                cx,
            ));
        }))
        .on_action(cx.listener(|_, _: &HideApp, _, cx| cx.hide()))
        .on_action(cx.listener(|_, _: &HideOthers, _, cx| cx.hide_other_apps()))
        .on_action(cx.listener(|_, _: &ShowAll, _, cx| cx.unhide_other_apps()))
        .on_action(cx.listener(|_, _: &QuitApp, _, cx| cx.quit()))
        .on_action(cx.listener(|_, _: &Minimize, window, _| window.minimize_window()))
        .on_action(cx.listener(|_, _: &Zoom, window, _| window.zoom_window()))
        .on_action(cx.listener(|_, _: &BringAllToFront, window, cx| {
            cx.activate(true);
            window.activate_window();
        }))
        .on_action(cx.listener(|_, _: &OpenDevTools, window, cx| {
            #[cfg(any(debug_assertions, feature = "devtools"))]
            window.toggle_inspector(cx);
            #[cfg(not(any(debug_assertions, feature = "devtools")))]
            let _ = (window, cx);
        }))
    }

    /// A menu item fired: fan it out as `menu.action` (the shell's own subscription handles
    /// it first, then views).
    fn emit_menu_action(&mut self, name: &str, _: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_string();
        self.state
            .update(cx, |s, cx| s.dispatch_menu_action(name, cx));
    }

    /// App.svelte's `onMenuAction` handler.
    pub(crate) fn menu_action(
        &mut self,
        action: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        if let Some(key) = action.strip_prefix("app-theme:") {
            state.update(cx, |s, cx| s.set_theme(key, cx));
            return;
        }
        if let Some(key) = action.strip_prefix("editor-theme:") {
            let name = state.update(cx, |s, cx| {
                s.set_editor_theme(key, cx);
                s.editor_theme_name(key)
            });
            recur_view_core::AppState::toast_editor_theme(&name, cx);
            return;
        }
        if let Some(key) = action.strip_prefix("terminal-theme:") {
            let id = terminal_id_from_menu(key);
            state.update(cx, |s, cx| s.set_terminal_theme(&id, cx));
            return;
        }
        // The workspace a dock or run action targets: only while one is on screen and its
        // worktree exists.
        let dock_workspace = |s: &recur_view_core::AppState| {
            let id = s.detail_workspace_id().to_string();
            (!id.is_empty() && !ws_main_blocked(s.workspace(&id))).then_some(id)
        };
        match action {
            "open-path" => self.pick_and_open_path(cx),
            "new-loop" => state.update(cx, |s, cx| s.open_new_task("", None, cx)),
            "new-blank-workspace" => crate::new_workspace::create_blank_workspace(&state, cx),
            "palette" => state.update(cx, |s, cx| s.toggle_go_palette(cx)),
            "run-command" => state.update(cx, |s, cx| s.toggle_commands_palette(cx)),
            "open-in" => state.update(cx, |s, cx| {
                if !ws_main_blocked(s.selected_workspace()) {
                    s.toggle_open_in(cx);
                }
            }),
            "copy-path" if !ws_main_blocked(state.read(cx).selected_workspace()) => {
                crate::titlebar::copy_selected_workspace_path(&state, cx);
            }
            "shortcuts" => state.update(cx, |s, cx| s.toggle_shortcuts(cx)),
            "prev-workspace" => state.update(cx, |s, cx| s.cycle_workspace(-1, cx)),
            "next-workspace" => state.update(cx, |s, cx| s.cycle_workspace(1, cx)),
            "prev-editor-theme" | "next-editor-theme" => {
                let delta = if action == "prev-editor-theme" { -1 } else { 1 };
                let (_, name) = state.update(cx, |s, cx| s.cycle_editor_theme(delta, cx));
                recur_view_core::AppState::toast_editor_theme(&name, cx);
            }
            "toggle-sidebar" => state.update(cx, |s, cx| s.toggle_rail(cx)),
            "settings" => state.update(cx, |s, cx| s.go(ViewName::Settings, None, cx)),
            "check-updates" => self.check_for_updates(cx),
            "reload-bundles" => crate::bundles::reload_bundles(&state, cx),
            "toggle-terminal" => state.update(cx, |s, cx| {
                if let Some(id) = dock_workspace(s) {
                    s.toggle_terminal_dock(&id, cx);
                }
            }),
            "maximize-terminal" => state.update(cx, |s, cx| {
                if let Some(id) = dock_workspace(s) {
                    s.toggle_terminal_dock_maximized(&id, cx);
                }
            }),
            "run-dev" => state.update(cx, |s, cx| {
                if let Some(id) = dock_workspace(s) {
                    s.open_run_menu(&id, cx);
                }
            }),
            "stop-run" => state.update(cx, |s, cx| {
                if let Some(id) = dock_workspace(s) {
                    s.open_stop_menu(&id, cx);
                }
            }),
            _ => {}
        }
    }

    /// `checkForUpdates`: a menu check reports every outcome; a staged update offers Restart.
    pub(crate) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if let Some(version) = self.update_ready.clone().filter(|v| !v.is_empty()) {
            self.show_update_ready(&version, cx);
            return;
        }
        self.update_manual = true;
        push_toast(
            ToastKind::Info,
            "Checking for updates…",
            ToastOptions {
                replace: Some(UPDATER_TOAST.into()),
                ttl: Some(std::time::Duration::ZERO),
                ..Default::default()
            },
            cx,
        );
        self.state.read(cx).services().emit(AppEvent::UpdaterCheck);
    }

    fn show_update_ready(&mut self, version: &str, cx: &mut Context<Self>) {
        let services = self.state.read(cx).services().clone();
        push_toast(
            ToastKind::Info,
            format!(
                "{} {version} is ready to install",
                recur_platform::identity::APP_NAME
            ),
            ToastOptions {
                action: Some(toast_action("Restart", move |_, _| {
                    services.emit(AppEvent::UpdaterUserRestart)
                })),
                ttl: Some(std::time::Duration::ZERO),
                replace: Some(UPDATER_TOAST.into()),
                ..Default::default()
            },
            cx,
        );
    }

    /// `wireUpdaterEvents`: background checks stay silent until an update is downloaded.
    pub(crate) fn updater_event(&mut self, ev: UpdaterEvent, cx: &mut Context<Self>) {
        let opts = |ttl: Option<std::time::Duration>| ToastOptions {
            replace: Some(UPDATER_TOAST.into()),
            ttl,
            ..Default::default()
        };
        let app = recur_platform::identity::APP_NAME;
        match ev {
            UpdaterEvent::UpdateAvailable(r) if self.update_manual => push_toast(
                ToastKind::Info,
                format!("Downloading {app} {}…", r.version),
                opts(Some(std::time::Duration::ZERO)),
                cx,
            ),
            UpdaterEvent::NoUpdate => {
                if self.update_manual {
                    push_toast(
                        ToastKind::Success,
                        format!("{app} is up to date"),
                        opts(None),
                        cx,
                    );
                }
                self.update_manual = false;
            }
            UpdaterEvent::Error(e) => {
                if self.update_manual {
                    let msg = if e.message.is_empty() {
                        "unknown error".to_string()
                    } else {
                        e.message
                    };
                    push_toast(
                        ToastKind::Error,
                        format!("Update failed: {msg}"),
                        opts(None),
                        cx,
                    );
                }
                self.update_manual = false;
            }
            UpdaterEvent::UpdateReady(r) => {
                self.update_manual = false;
                self.update_ready = Some(r.version.clone());
                self.show_update_ready(&r.version, cx);
            }
            _ => {}
        }
    }
}
