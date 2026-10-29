//! The window's root view (`App.svelte`): titlebar, sidebar, the routed main area, the
//! overlays host, global keys, menu actions and boot.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyView, App, AppContext, Context, Entity, ExternalPaths, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, Styled, Subscription, Window,
    WindowAppearance, div,
};
use recur_services::AppEvent;
use recur_services::events::MenuThemes;
use recur_ui::style;
use recur_ui::tooltip::TitleTooltip;
use recur_ui::widgets::Toasts;
use recur_view_core::{AppState, ShellEvent, ViewContext, ViewName, ViewRegistry};
use recur_view_sidebar::Sidebar;

use crate::land::LandChrome;
use crate::titlebar::TitlebarState;

/// What the desktop hands the shell.
#[derive(Clone, Default)]
pub struct ShellOptions {
    /// The updater is installed: show Check for Updates… (release app bundles).
    pub updates: bool,
    /// The OS window became key / lost key (`eng.SetAppFocused`).
    pub on_window_active: Option<Rc<dyn Fn(bool)>>,
    /// Persist geometry to `window.json` in this data dir, starting from the loaded state.
    pub geometry: Option<(
        std::path::PathBuf,
        recur_platform::window_state::WindowState,
    )>,
}

/// A registered main-area view, built.
struct MountedView {
    routes: &'static [ViewName],
    keep_laid_out: bool,
    view: AnyView,
    titlebar_left: Option<AnyView>,
    titlebar_right: Option<AnyView>,
}

pub struct Shell {
    pub(crate) state: Entity<AppState>,
    pub(crate) opts: ShellOptions,
    pub(crate) sidebar: Entity<Sidebar>,
    /// `.main`, where focus parks when the rail flyout closes on it.
    main_focus: FocusHandle,
    views: Vec<MountedView>,
    /// The overlays, toast host and tooltip, each behind a cached boundary, so a redraw passing
    /// through the shell (a key typed in a chat) does not render them.
    layers: Vec<Entity<recur_ui::pane::Layer>>,
    titlebar_pane: Option<Entity<recur_ui::pane::Pane<Shell>>>,
    pub(crate) focus: FocusHandle,
    pub(crate) titlebar: TitlebarState,
    pub(crate) land: LandChrome,
    /// What the native menubar was last built from.
    menu_built: Option<(
        recur_services::events::MenuState,
        MenuThemes,
        Vec<recur_view_core::menu::MenuGroup>,
    )>,
    /// The theme menus and a fingerprint of the catalogs they were sorted from.
    themes_memo: Option<(u64, MenuThemes)>,
    /// The chat-occluded value last reported (`SetChatUIOccluded`).
    occluded: Option<bool>,
    /// An updater check the user asked for reports every outcome.
    pub(crate) update_manual: bool,
    pub(crate) update_ready: Option<String>,
    picking_path: bool,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    /// The mounted main-area view for `route`, if a registered view serves it.
    pub fn route_view(&self, route: ViewName) -> Option<AnyView> {
        self.views
            .iter()
            .find(|m| m.routes.contains(&route))
            .map(|m| m.view.clone())
    }

    /// Builds the shell and every registered view, then boots (`onMount`).
    pub fn new(
        ctx: ViewContext,
        registry: ViewRegistry,
        opts: ShellOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = ctx.state.clone();
        let subscriptions = vec![
            cx.observe_state_in(&state, Topics::ALL, window, |this, _, window, cx| {
                this.state_changed(window, cx)
            }),
            // The shell's own `menu.action` handling runs first: it subscribed before any view.
            cx.guarded_subscribe_in(&state, window, |this, _, ev: &ShellEvent, window, cx| {
                if let ShellEvent::MenuAction(action) = ev {
                    this.menu_action(action, window, cx);
                }
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                let dark = matches!(
                    window.appearance(),
                    WindowAppearance::Dark | WindowAppearance::VibrantDark
                );
                this.state.update(cx, |s, cx| s.set_native_dark(dark, cx));
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                if let Some(f) = &this.opts.on_window_active {
                    f(active);
                }
                if active {
                    this.state.update(cx, |_, cx| cx.notify());
                }
            }),
        ];
        let main_focus = cx.focus_handle();
        let sidebar = {
            let main_focus = main_focus.clone();
            cx.new(|cx| Sidebar::new(ctx.clone(), main_focus, window, cx))
        };
        let views = registry
            .views
            .iter()
            .map(|e| {
                let parts = e.titlebar.as_ref().map(|f| f(&ctx, window, cx));
                let (left, right) = parts.map(|p| (p.left, p.right)).unwrap_or_default();
                MountedView {
                    routes: e.routes,
                    keep_laid_out: e.keep_laid_out,
                    view: (e.build)(&ctx, window, cx),
                    titlebar_left: left,
                    titlebar_right: right,
                }
            })
            .collect();
        let overlays: Vec<AnyView> = registry
            .overlays
            .iter()
            .map(|(_, build)| build(&ctx, window, cx))
            .collect();
        let toasts = cx.new(Toasts::new);
        let tooltip = TitleTooltip::new(cx);
        let layers = overlays
            .iter()
            .cloned()
            .chain([
                AnyView::from(toasts.clone()),
                AnyView::from(tooltip.clone()),
            ])
            .map(|v| recur_ui::pane::Layer::new(v, cx))
            .collect();
        let mut shell = Self {
            state,
            opts,
            sidebar,
            views,
            layers,
            titlebar_pane: None,
            focus: cx.focus_handle(),
            main_focus,
            titlebar: TitlebarState::default(),
            land: LandChrome::default(),
            menu_built: None,
            themes_memo: None,
            occluded: None,
            update_manual: false,
            update_ready: None,
            picking_path: false,
            _subscriptions: subscriptions,
        };
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        shell.state.update(cx, |s, cx| s.set_native_dark(dark, cx));
        if let Some((dir, loaded)) = shell.opts.geometry.clone() {
            shell.persist_window_geometry(dir, loaded, window, cx);
        }
        shell.boot(cx);
        shell
    }

    /// `persistWindowGeometry`: save `window.json` 250ms after a resize or move settles and
    /// immediately on close; a window saved maximised zooms on first show.
    fn persist_window_geometry(
        &mut self,
        dir: std::path::PathBuf,
        loaded: recur_platform::window_state::WindowState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use recur_platform::window_state::{self, GeometryTracker, WindowFrame};
        use std::cell::RefCell;
        let tracker = Rc::new(RefCell::new(GeometryTracker::new(loaded)));
        let frame = |window: &Window| {
            let b = window.window_bounds();
            let r = b.get_bounds();
            WindowFrame {
                width: f32::from(r.size.width).round() as i32,
                height: f32::from(r.size.height).round() as i32,
                x: f32::from(r.origin.x).round() as i32,
                y: f32::from(r.origin.y).round() as i32,
                maximised: matches!(b, gpui_kit::WindowBounds::Maximized(_))
                    || window.is_maximized(),
                minimised: false,
                fullscreen: matches!(b, gpui_kit::WindowBounds::Fullscreen(_))
                    || window.is_fullscreen(),
            }
        };
        if tracker
            .borrow_mut()
            .should_zoom_on_show(window.is_maximized())
        {
            window.zoom_window();
        }
        let pending: Rc<RefCell<Option<gpui_kit::Task<()>>>> = Rc::default();
        {
            let (tracker, pending, dir) = (tracker.clone(), pending.clone(), dir.clone());
            self._subscriptions
                .push(cx.observe_window_bounds(window, move |_, window, cx| {
                    let f = frame(window);
                    let (tracker, dir) = (tracker.clone(), dir.clone());
                    *pending.borrow_mut() = Some(cx.spawn(async move |_, cx| {
                        cx.background_executor()
                            .timer(window_state::SAVE_DEBOUNCE)
                            .await;
                        let st = tracker.borrow_mut().snapshot(&f);
                        window_state::save(&dir, st);
                    }));
                }));
        }
        window.on_window_should_close(cx, move |window, _| {
            pending.borrow_mut().take();
            let st = tracker.borrow_mut().snapshot(&frame(window));
            window_state::save(&dir, st);
            true
        });
    }

    pub fn state(&self) -> &Entity<AppState> {
        &self.state
    }

    /// `onMount`: theme catalogs, the initial data, shell restore, then `app.ready`.
    fn boot(&mut self, cx: &mut Context<Self>) {
        let state = self.state.clone();
        state.update(cx, |s, cx| {
            s.wire_events(cx);
            s.load_theme_catalogs(cx).detach();
        });
        self.wire_backend_events(cx);
        let services = state.read(cx).services().clone();
        let load = state.update(cx, |s, cx| s.load_initial(cx));
        cx.spawn(async move |_, cx| {
            let loaded = load.await;
            // `loadShellState` swallows its own errors: a missing snapshot restores nothing.
            let saved = services.shell.get_shell_state().await.ok();
            state.update(cx, |s, cx| {
                if loaded.is_ok() {
                    s.apply_boot_route(saved.as_ref(), cx);
                }
                s.enable_shell_persist(cx);
            });
            services.emit(AppEvent::AppReady);
        })
        .detach();
    }

    /// The UI-side listeners of backend events the shell owns (`wireUpdaterEvents`,
    /// `menu.action` from the backend).
    fn wire_backend_events(&mut self, cx: &mut Context<Self>) {
        let mut events = self.state.read(cx).services().subscribe();
        cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let alive = this.update(cx, |shell, cx| match ev {
                    AppEvent::Updater(e) => shell.updater_event(e, cx),
                    ev => shell.land_event(&ev, cx),
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Effects App.svelte ran off store changes.
    fn state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let occluded = self.state.read(cx).chat_occluded();
        if self.occluded != Some(occluded) {
            self.occluded = Some(occluded);
            let services = self.state.read(cx).services().clone();
            cx.background_spawn(async move {
                let _ = services.workspaces.set_chat_ui_occluded(occluded).await;
            })
            .detach();
        }
        self.state.update(cx, |s, cx| s.apply_theme_global(cx));
        self.titlebar_state_changed(window, cx);
        self.sync_menus(cx);
        cx.notify();
    }

    /// Rebuild the native menubar when its enabled state or theme catalogs changed
    /// (`applyMenuState`, `applyMenuThemes`, `applyMenuThemeState`).
    pub(crate) fn sync_menus(&mut self, cx: &mut Context<Self>) {
        let (menu_state, themes, workspaces) = {
            let s = self.state.read(cx);
            let mut menu_state = s.menu_state.clone();
            let on = s.view == ViewName::Detail
                && s.selected_workspace().is_some()
                && !recur_view_core::modes::ws_main_blocked(s.selected_workspace());
            menu_state.open_in = Some(on);
            menu_state.copy_path = Some(on);
            // This runs on every store change: sort the theme lists only when they change.
            let mark = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                for t in &s.editor_theme_catalog {
                    (&t.id, &t.name).hash(&mut h);
                }
                for t in &s.terminal_theme_catalog {
                    (&t.id, &t.name).hash(&mut h);
                }
                if let Some(st) = &s.settings {
                    st.disabled_bundle_themes.hash(&mut h);
                }
                h.finish()
            };
            let mut themes = match &self.themes_memo {
                Some((m, t)) if *m == mark => t.clone(),
                _ => {
                    let t = s.menu_themes();
                    self.themes_memo = Some((mark, t.clone()));
                    t
                }
            };
            let sel = s.menu_theme_state();
            themes.app_selected = sel.app_selected;
            themes.editor_selected = sel.editor_selected;
            themes.terminal_selected = sel.terminal_selected;
            let workspaces = recur_view_core::menu::menu_workspace_tree(
                &s.project_groups,
                &s.projects,
                &s.workspaces,
                s.show_hidden_projects(),
            );
            (menu_state, themes, workspaces)
        };
        let key = (menu_state, themes, workspaces);
        if self.menu_built.as_ref() == Some(&key) {
            return;
        }
        let t = std::time::Instant::now();
        let menus = crate::menu::app_menus(&key.0, Some(&key.1), &key.2, self.opts.updates, cx);
        cx.set_menus(menus);
        crate::menu::count_rebuild(t.elapsed());
        self.menu_built = Some(key);
    }

    /// `pickAndOpenPath`: the native file/folder picker, then the same OpenPath as `recur .`.
    pub(crate) fn pick_and_open_path(&mut self, cx: &mut Context<Self>) {
        if self.picking_path {
            return;
        }
        self.picking_path = true;
        let picked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let path = picked
                .await
                .ok()
                .and_then(|r| r.ok())
                .flatten()
                .and_then(|mut p| p.pop());
            if let Some(path) = path {
                let services = state.read_with(cx, |s, _| s.services().clone());
                let p = path.to_string_lossy().to_string();
                if let Some(res) =
                    recur_view_core::call(cx, services.workspaces.open_path(&p), "Could not open")
                        .await
                {
                    state.update(cx, |s, cx| s.apply_open_path_result(res, cx));
                }
            }
            let _ = this.update(cx, |this, _| this.picking_path = false);
        })
        .detach();
    }

    /// `.main`: the routed views on `--bg`.
    fn main_area(&self, view: ViewName, cx: &App) -> gpui_kit::AnyElement {
        let active = self.views.iter().position(|v| v.routes.contains(&view));
        let mut main = div()
            .id("main")
            .track_focus(&self.main_focus)
            .flex_1()
            .flex()
            .flex_col()
            .min_w_0()
            .bg(recur_ui::Theme::of(cx).tokens.bg)
            .relative();
        // Hidden routes paint first: GPUI still hit-tests an invisible subtree, so the routed
        // view must be on top of them.
        let order = self
            .views
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != active)
            .chain(
                self.views
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| Some(*i) == active),
            );
        for (i, v) in order {
            let on = Some(i) == active;
            if on {
                // Its own layout root: the shell's nesting would lay the route out dozens of times
                // a frame (see `recur_ui::layout_root`).
                main = main.child(recur_ui::layout_root::layout_root(
                    "route",
                    gpui_kit::StyleRefinement::default().flex_1().min_h_0(),
                    div().size_full().flex().flex_col().child(v.view.clone()),
                ));
            } else if v.keep_laid_out {
                // `#v-detail.view`: hidden but laid out at the main area's size. Cached: GPUI lays
                // the whole tree out every frame, and a hidden route only changes when notified.
                main = main.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .invisible()
                        .child(
                            v.view
                                .clone()
                                .cached(gpui_kit::StyleRefinement::default().size_full()),
                        ),
                );
            }
        }
        main.into_any_element()
    }

    pub(crate) fn titlebar_parts(&self, view: ViewName) -> (Option<AnyView>, Option<AnyView>) {
        self.views
            .iter()
            .find(|v| v.routes.contains(&view))
            .map(|v| (v.titlebar_left.clone(), v.titlebar_right.clone()))
            .unwrap_or_default()
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Shell {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        // App.svelte's window keydown sees keys with nothing focused (focus on `body`).
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        let (view, rail_collapsed, rail_width) = {
            let s = self.state.read(cx);
            (
                s.view,
                s.rail_collapsed,
                s.panels.size(recur_view_core::prefs::PanelKey::Rail),
            )
        };
        let titlebar_pane = self
            .titlebar_pane
            .get_or_insert_with(|| recur_ui::pane::Pane::of(cx, Shell::render_titlebar))
            .clone();
        let titlebar = recur_ui::style::cached_view(
            &titlebar_pane,
            gpui_kit::StyleRefinement::default()
                .flex_none()
                .w_full()
                .h(gpui_kit::px(crate::titlebar::TITLEBAR_HEIGHT)),
            cx,
        );
        let main = self.main_area(view, cx);
        let state = self.state.clone();
        let root = style::body(cx)
            .id("shell")
            .key_context("RecurShell")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .on_key_down(
                cx.guarded_listener(|this, ev, window, cx| this.on_key_down(ev, window, cx)),
            )
            .on_drop(move |paths: &ExternalPaths, _, cx| {
                let files: Vec<String> = paths
                    .paths()
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                state.update(cx, |s, cx| s.files_dropped(files, cx));
            })
            .child(
                div().flex().flex_col().size_full().child(titlebar).child(
                    div()
                        .id("body")
                        .flex_1()
                        .flex()
                        .min_h_0()
                        .relative()
                        .overflow_hidden()
                        // A collapsed rail is an overlay, so it paints after the content.
                        // Cached at the rail's own box: typing, scrolling and streaming in
                        // the main area never re-render the project tree.
                        .when(!rail_collapsed, |d| {
                            d.child(recur_ui::style::cached_view(
                                &self.sidebar,
                                gpui_kit::StyleRefinement::default()
                                    .flex_none()
                                    .w(gpui_kit::px(rail_width))
                                    .h_full()
                                    .min_h_0(),
                                cx,
                            ))
                        })
                        .child(main)
                        .when(rail_collapsed, |d| d.child(self.sidebar.clone())),
                ),
            )
            // Deferred at `.nt-bg`'s `z-index: 190`: the page's own deferred layers (resize
            // handles, sticky headers) paint and take the pointer under the overlays.
            .children(self.layers.iter().map(|l| {
                gpui_kit::deferred(recur_ui::style::cached_view(
                    l,
                    gpui_kit::StyleRefinement::default().absolute().inset_0(),
                    cx,
                ))
                .with_priority(recur_ui::style::OVERLAY_PRIORITY)
            }));
        self.with_menu_actions(root, cx)
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
