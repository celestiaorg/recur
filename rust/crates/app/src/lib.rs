//! The GPUI application shell: the main window (`window_state.go`, `window_titlebar*.go`),
//! the native menu (`app_menu.go`), and App.svelte's root view: titlebar, sidebar, routed
//! main area, overlays host, global keys and boot.
//!
//! Views plug in through [`recur_view_core::ViewRegistry`]; see `rust/docs/ARCHITECTURE.md`.

mod actions;
mod bundles;
pub mod chrome;
mod keys;
mod land;
pub mod menu;
mod new_workspace;
mod shell;
mod titlebar;

#[cfg(feature = "gallery")]
pub mod gallery;

use std::path::PathBuf;

use gpui_kit::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, div, point,
    px, size,
};
use recur_platform::window_state;
use recur_services::Services;
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewRegistry};

pub use chrome::{TitlebarClickAction, parse_titlebar_double_click_action};
pub use keys::TEXT_ENTRY_CONTEXTS;
pub use shell::{Shell, ShellOptions};
pub use titlebar::{TITLEBAR_HEIGHT, TRAFFIC_LIGHTS};

/// Open Developer Tools is offered in dev builds and with the `devtools` feature
/// (`inspectorAvailable`).
pub const INSPECTOR_AVAILABLE: bool = cfg!(any(debug_assertions, feature = "devtools"));

/// Where the macOS traffic lights sit: centered in the 52px titlebar (16px tall buttons), at
/// AppKit's unified-toolbar inset, as Wails' hidden-inset titlebar placed them.
pub const TRAFFIC_LIGHT_POSITION: (f32, f32) = (20., 18.);

/// One-time app setup: the design system, menubar accelerators and every view crate's init.
pub fn init(registry: &ViewRegistry, cx: &mut App) {
    gpui_kit::init(cx);
    recur_ui::init(cx);
    menu::bind_menu_keys(cx);
    for init in &registry.inits {
        init(cx);
    }
}

/// The window's root: gpui-base's `Root` over the WebKit-matching tree root.
struct WindowRoot {
    shell: Entity<Shell>,
}

impl Render for WindowRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(recur_ui::style::window_root(self.shell.clone()))
    }
}

/// GPUI window options from the saved geometry (`windowOptionsFromState`).
pub fn window_options(opts: &window_state::WindowOptions, cx: &App) -> WindowOptions {
    let sz = size(px(opts.width as f32), px(opts.height as f32));
    let bounds = match opts.position {
        Some((x, y)) => Bounds {
            origin: point(px(x as f32), px(y as f32)),
            size: sz,
        },
        None => Bounds::centered(None, sz, cx),
    };
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(opts.title.clone().into()),
            appears_transparent: true,
            traffic_light_position: Some(point(
                px(TRAFFIC_LIGHT_POSITION.0),
                px(TRAFFIC_LIGHT_POSITION.1),
            )),
        }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_movable: true,
        app_owns_titlebar_drag: true,
        is_resizable: true,
        is_minimizable: true,
        app_id: Some(recur_platform::identity::LINUX_APP_ID.to_string()),
        window_min_size: Some(size(px(opts.min_width as f32), px(opts.min_height as f32))),
        ..Default::default()
    }
}

/// What the desktop hands [`open_main_window`].
pub struct MainWindow {
    pub services: Services,
    pub storage: LocalStorage,
    /// The data dir `window.json` lives in; `None` keeps geometry unsaved (tests).
    pub data_dir: Option<PathBuf>,
    pub registry: ViewRegistry,
    pub shell: ShellOptions,
}

/// Opens the main window over `services`: the app state, the shell, the native menu, and
/// geometry persistence (`persistWindowGeometry`, on the shell).
pub fn open_main_window(main: MainWindow, cx: &mut App) -> anyhow::Result<(WindowHandle<gpui_kit::base::Root>, Entity<AppState>)> {
    let loaded = main
        .data_dir
        .as_deref()
        .map(window_state::load)
        .unwrap_or_default();
    let opts = window_options(&window_state::WindowOptions::from_state(&loaded), cx);
    let state = AppState::install(main.services.clone(), main.storage, cx);
    let ctx = ViewContext {
        state: state.clone(),
        services: main.services,
    };
    let registry = main.registry;
    let shell_opts = main.shell;
    let data_dir = main.data_dir;
    let mut shell_opts = shell_opts;
    shell_opts.geometry = data_dir.map(|dir| (dir, loaded));
    let handle = cx.open_window(opts, move |window, cx| {
        let shell = cx.new(|cx| Shell::new(ctx, registry, shell_opts, window, cx));
        let focus = shell.read(cx).focus.clone();
        window.focus(&focus, cx);
        let root = cx.new(|_| WindowRoot { shell });
        cx.new(|cx| gpui_kit::base::Root::new(root, window, cx))
    })?;
    Ok((handle, state))
}

