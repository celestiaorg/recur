//! `TerminalDock.svelte`: the Observatory's terminal drawer. One tab per live shell of the
//! workspace (adopted from `ListTerminals`, else restored, else spawned), every pane mounted so
//! background shells keep streaming, the find bar, run / stop of the workspace's dev command,
//! tab drag reorder, and the dock's own chords. Shells outlive the dock: unmounting parks the
//! panes (`term-park`) and the next dock adopts them.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    Global, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, px,
};
use recur_services::{AppEvent, Services};
use recur_terminal_view::{
    FindBarEvent, TerminalBackend, TerminalEvent, TerminalFindBar, TerminalOptions,
    TerminalPrefs as FontPrefs, TerminalThemeInfo, TerminalView, ThemeContext,
    resolve_terminal_font,
};
use recur_ui::text::{TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Axis, Edge, resize_handle, tab_chip, tab_strip, tab_strip_action};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::chat::WorkspaceTerminalTab;
use recur_view_core::prefs::PanelKey;
use recur_view_core::run::{run_label, workspace_run, workspace_runs};
use recur_view_core::{AppState, call};
use recur_view_review::style::hsla;

/// Session calls a pane makes, through `TerminalService` (the desktop's PTY manager behind it).
pub struct ServicesBackend(pub Services);

impl TerminalBackend for ServicesBackend {
    fn attach(&self, id: &str) -> Result<Vec<u8>, String> {
        let b64 = futures::executor::block_on(self.0.terminal.attach_terminal(id))
            .map_err(|e| e.message)?;
        base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| e.to_string())
    }

    fn detach(&self, id: &str) {
        let _ = futures::executor::block_on(self.0.terminal.detach_terminal(id));
    }

    fn ack(&self, id: &str, n: usize) {
        let _ = futures::executor::block_on(self.0.terminal.ack_terminal(id, n as i64));
    }

    fn write(&self, id: &str, data: &[u8]) {
        let s = String::from_utf8_lossy(data);
        let _ = futures::executor::block_on(self.0.terminal.write_terminal(id, &s));
    }

    fn resize(&self, id: &str, cols: u16, rows: u16) {
        let _ = futures::executor::block_on(self.0.terminal.resize_terminal(
            id,
            i64::from(cols),
            i64::from(rows),
        ));
    }

    fn clear_replay(&self, id: &str) {
        let _ = futures::executor::block_on(self.0.terminal.clear_terminal(id));
    }
}

/// `term.data` / `term.exit` to the panes (`wireTerminalEvents`), wired once per app.
#[derive(Default)]
pub(crate) struct TermEvents(Option<Task<()>>);

impl Global for TermEvents {}

pub(crate) fn init(cx: &mut App) {
    cx.default_global::<TermEvents>();
}

pub(crate) fn wire(services: &Services, cx: &mut App) {
    if cx.default_global::<TermEvents>().0.is_some() {
        return;
    }
    let mut rx = services.subscribe();
    let task = cx.spawn(async move |cx| {
        while let Some(ev) = rx.recv().await {
            if let AppEvent::Term(e) = ev {
                cx.update(|cx| recur_terminal_view::route_event(&e, cx));
            }
        }
    });
    cx.default_global::<TermEvents>().0 = Some(task);
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub exited: bool,
    pub busy: bool,
}

/// What the dock tells the Observatory (`onTabsChange`).
#[derive(Clone, Debug, PartialEq)]
pub enum DockEvent {
    TabsChanged(usize),
}

/// The props `<TerminalDock>` took.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DockProps {
    pub lit: bool,
    pub overlay: bool,
    pub beside: bool,
    pub gpu: bool,
}

pub struct TerminalDock {
    state: Entity<AppState>,
    services: Services,
    backend: Arc<dyn TerminalBackend>,
    workspace_id: String,
    pub props: DockProps,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active_id: String,
    loading: bool,
    pub(crate) panes: HashMap<String, Entity<TerminalView>>,
    pane_subs: HashMap<String, Subscription>,
    pub(crate) find: Entity<TerminalFindBar>,
    pub(crate) find_open: bool,
    searched_id: String,
    pub(crate) focus: FocusHandle,
    resize_focus: FocusHandle,
    body: crate::geometry::BoundsCell,
    /// `.obs-main`, for the drag ceiling (keep a quarter of it, up to 160px, for the editor).
    main_bounds: crate::geometry::BoundsCell,
    run_task: Option<Task<()>>,
    reorder_task: Option<Task<()>>,
    released: bool,
    seen_workspace: Option<recur_domain::Workspace>,
    /// The terminal theme context and what it was built from (`theme_key`).
    theme_memo: Option<(u64, ThemeContext)>,
    _subs: Vec<Subscription>,
}

/// The store topics the dock reads: its workspace row, the run store, prefs and chrome.
const DOCK_TOPICS: Topics = Topics::ALL.without(
    Topics::PRS
        .union(Topics::ATTENTION)
        .union(Topics::AGENTS)
        .union(Topics::INBOX)
        .union(Topics::BRIEFING)
        .union(Topics::USAGE)
        .union(Topics::RESOURCE)
        .union(Topics::MENU),
);

impl EventEmitter<DockEvent> for TerminalDock {}

impl Focusable for TerminalDock {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// What [`theme_context`] reads: the picked theme and the catalog's themes.
fn theme_key(state: &AppState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    (&state.terminal_prefs.theme, state.bundles_epoch).hash(&mut h);
    for t in &state.terminal_theme_catalog {
        (&t.id, &t.name).hash(&mut h);
    }
    h.finish()
}

fn theme_context(state: &AppState) -> ThemeContext {
    let catalog: Vec<TerminalThemeInfo> = state
        .terminal_theme_catalog
        .iter()
        .filter_map(|t| {
            serde_json::to_value(t)
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
        })
        .collect();
    ThemeContext {
        terminal_theme: state.terminal_prefs.theme.clone(),
        catalog,
    }
}

fn font_for(state: &AppState, cx: &App) -> recur_terminal_view::TerminalFont {
    let editor = recur_view_core::editor_env::editor_prefs(cx);
    let p = &state.terminal_prefs;
    let prefs = FontPrefs {
        font_face: p.font_face.clone(),
        font_size: u32::from(p.font_size),
        line_height: p.line_height as f32,
        theme: p.theme.clone(),
    }
    .normalized();
    resolve_terminal_font(&prefs, &editor.font_face, editor.font_size)
}

impl TerminalDock {
    pub fn new(
        state: Entity<AppState>,
        workspace_id: &str,
        props: DockProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let services = state.read(cx).services().clone();
        wire(&services, cx);
        let find = cx.new(|cx| TerminalFindBar::new(window, cx));
        let mut subs = vec![
            cx.guarded_subscribe_in(&find, window, |d, _, ev: &FindBarEvent, window, cx| {
                d.on_find_bar(ev, window, cx)
            }),
            cx.observe_state_changes_in(&state, DOCK_TOPICS, window, |d, topics, window, cx| {
                // Workspace rows change with every agent event: follow only this one's.
                if topics == Topics::WORKSPACES {
                    let ws = d.state.read(cx).workspace(&d.workspace_id).cloned();
                    if ws == d.seen_workspace {
                        return;
                    }
                    d.seen_workspace = ws;
                }
                d.state_changed(window, cx)
            }),
        ];
        subs.push(cx.on_release(|d, cx| d.park_panes(cx)));
        let backend: Arc<dyn TerminalBackend> = Arc::new(ServicesBackend(services.clone()));
        let mut d = TerminalDock {
            state,
            services,
            backend,
            workspace_id: workspace_id.to_string(),
            props,
            tabs: Vec::new(),
            active_id: String::new(),
            loading: true,
            panes: HashMap::new(),
            pane_subs: HashMap::new(),
            find,
            find_open: false,
            searched_id: String::new(),
            focus: cx.focus_handle(),
            resize_focus: cx.focus_handle(),
            body: Default::default(),
            main_bounds: Default::default(),
            run_task: None,
            reorder_task: None,
            released: false,
            seen_workspace: None,
            theme_memo: None,
            _subs: subs,
        };
        d.mount(window, cx);
        d
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// The Observatory's `.obs-main` bounds cell (the resize ceiling).
    pub fn set_main_bounds(&mut self, cell: crate::geometry::BoundsCell) {
        self.main_bounds = cell;
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_id(&self) -> &str {
        &self.active_id
    }

    pub fn pane(&self, id: &str) -> Option<&Entity<TerminalView>> {
        self.panes.get(id)
    }

    pub fn set_props(&mut self, props: DockProps, cx: &mut Context<Self>) {
        if self.props == props {
            return;
        }
        let frosted_changed = props.overlay != self.props.overlay;
        self.props = props;
        if frosted_changed {
            let frosted = self.frosted(cx);
            for p in self.panes.values() {
                p.update(cx, |v, cx| v.set_frosted(frosted, cx));
            }
        }
        cx.notify();
    }

    fn frosted(&self, cx: &App) -> bool {
        self.props.overlay && theme_context(self.state.read(cx)).frosts(Theme::of(cx).id)
    }

    /// Keys are inside the dock (a pane, the find bar, the strip).
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx) || self.focus.is_focused(window)
    }

    /// `focus()`: the active shell, else the dock chrome while a pane mounts.
    pub fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.panes.get(&self.active_id) {
            p.update(cx, |v, cx| v.focus(window, cx));
            return;
        }
        window.focus(&self.focus, cx);
    }

    fn emit_tabs(&mut self, cx: &mut Context<Self>) {
        cx.emit(DockEvent::TabsChanged(self.tabs.len()));
        let list: Vec<WorkspaceTerminalTab> = self
            .tabs
            .iter()
            .map(|t| WorkspaceTerminalTab {
                id: t.id.clone(),
                title: t.title.clone(),
                exited: t.exited,
                active: t.id == self.active_id,
            })
            .collect();
        let ws = self.workspace_id.clone();
        self.state
            .update(cx, |s, cx| s.set_workspace_terminals(&ws, list, cx));
        cx.notify();
    }

    /// `guessSize`: the PTY's first size from the body box (cells approximated).
    fn guess_size(&self) -> (i64, i64) {
        let b = self.body.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        let cols = if w > 0. {
            ((w - 16.) / 8.).floor().max(20.) as i64
        } else {
            80
        };
        let rows = if h > 0. {
            ((h - 8.) / 17.).floor().max(4.) as i64
        } else {
            24
        };
        (cols, rows)
    }

    fn set_active_remote(&self, id: &str, cx: &mut Context<Self>) {
        let svc = self.services.clone();
        let (ws, id) = (self.workspace_id.clone(), id.to_string());
        cx.background_spawn(async move {
            let _ = svc.terminal.set_active_terminal(&ws, &id).await;
        })
        .detach();
    }

    /// `onMount`: adopt live shells, else restore the last tabs, else spawn one.
    fn mount(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let svc = self.services.clone();
        let ws = self.workspace_id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let existing = call(
                cx,
                svc.terminal.list_terminals(&ws),
                "Failed to list terminals",
            )
            .await;
            let usable: Vec<_> = existing
                .unwrap_or_default()
                .into_iter()
                .filter(|t| !t.id.is_empty())
                .collect();
            if !usable.is_empty() {
                let _ = this.update_in(cx, |d, window, cx| d.adopt(usable, window, cx));
            } else {
                cx.background_executor()
                    .timer(Duration::from_millis(0))
                    .await;
                let (cols, rows) = this
                    .read_with(cx, |d, _| d.guess_size())
                    .unwrap_or((80, 24));
                let restored = call(
                    cx,
                    svc.terminal.restore_workspace_terminals(&ws, cols, rows),
                    "Failed to restore terminals",
                )
                .await;
                let restored: Vec<_> = restored
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|t| !t.id.is_empty())
                    .collect();
                if !restored.is_empty() {
                    let _ = this.update_in(cx, |d, window, cx| d.adopt(restored, window, cx));
                } else if let Ok(t) =
                    this.update_in(cx, |d, window, cx| d.spawn(None, false, window, cx))
                {
                    t.await;
                }
            }
            let _ = this.update_in(cx, |d, window, cx| {
                d.claim_run_tabs(cx);
                d.loading = false;
                d.state_changed(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Dedicated run tabs survive restarts by title: re-claim them.
    fn claim_run_tabs(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let names: Vec<String> = workspace_runs(&self.workspace_id, &s.workspaces, &s.projects)
            .into_iter()
            .map(|r| r.name)
            .collect();
        let mut by: HashMap<String, Vec<Tab>> = HashMap::new();
        for t in &self.tabs {
            if names.contains(&t.title) {
                by.entry(t.title.clone()).or_default().push(t.clone());
            }
        }
        let ws = self.workspace_id.clone();
        self.state.update(cx, |s, cx| {
            for (name, list) in by {
                if list.len() != 1 {
                    continue;
                }
                let t = &list[0];
                s.run.set_run_tab(&ws, &name, &t.id);
                if t.busy && !t.exited {
                    s.set_run_session(&ws, &name, &t.id, cx);
                }
            }
            cx.notify();
        });
    }

    fn adopt(&mut self, list: Vec<recur_term::Info>, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs = list
            .iter()
            .map(|t| Tab {
                id: t.id.clone(),
                title: if t.title.is_empty() {
                    "zsh".into()
                } else {
                    t.title.clone()
                },
                exited: t.exited,
                busy: t.busy,
            })
            .collect();
        let focused = list
            .iter()
            .find(|t| t.active && !t.exited)
            .map(|t| t.id.clone());
        let live = self.tabs.iter().find(|t| !t.exited).map(|t| t.id.clone());
        self.active_id = focused
            .or(live)
            .or_else(|| self.tabs.first().map(|t| t.id.clone()))
            .unwrap_or_default();
        let id = self.active_id.clone();
        self.set_active_remote(&id, cx);
        self.ensure_panes(window, cx);
        self.emit_tabs(cx);
    }

    /// The terminal theme context, rebuilt only when its theme or catalog changed.
    fn theme(&mut self, cx: &App) -> ThemeContext {
        let s = self.state.read(cx);
        let key = theme_key(s);
        match &self.theme_memo {
            Some((k, t)) if *k == key => t.clone(),
            _ => {
                let t = theme_context(s);
                self.theme_memo = Some((key, t.clone()));
                t
            }
        }
    }

    fn pane_options(&mut self, id: &str, cx: &App) -> TerminalOptions {
        let theme = self.theme(cx);
        let s = self.state.read(cx);
        TerminalOptions {
            session_id: id.to_string(),
            workspace_id: self.workspace_id.clone(),
            active: id == self.active_id,
            frosted: self.frosted(cx),
            font: font_for(s, cx),
            theme,
            workspace_root: s
                .workspace(&self.workspace_id)
                .map(|w| w.worktree_path.trim_end_matches('/').to_string())
                .unwrap_or_default(),
            dock_focus: Some(self.focus.clone()),
        }
    }

    /// Mount a `TerminalView` per tab (a parked one comes back as it was).
    fn ensure_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.tabs.iter().map(|t| t.id.clone()).collect();
        for id in ids {
            if self.panes.contains_key(&id) {
                continue;
            }
            let view = match recur_terminal_view::take_parked(&id, cx) {
                Some(v) => {
                    let opts = self.pane_options(&id, cx);
                    v.update(cx, |v, cx| {
                        v.set_dock_focus(opts.dock_focus.clone());
                        v.set_font(opts.font.clone(), cx);
                        v.set_theme(opts.theme.clone(), cx);
                        v.set_frosted(opts.frosted, cx);
                        v.set_workspace_root(opts.workspace_root.clone());
                        v.set_active(opts.active, window, cx);
                    });
                    v
                }
                None => {
                    let opts = self.pane_options(&id, cx);
                    let backend = self.backend.clone();
                    cx.new(|cx| TerminalView::new(opts, backend, window, cx))
                }
            };
            let sid = id.clone();
            let sub = cx.guarded_subscribe_in(
                &view,
                window,
                move |d, _, ev: &TerminalEvent, window, cx| d.on_pane_event(&sid, ev, window, cx),
            );
            self.pane_subs.insert(id.clone(), sub);
            self.panes.insert(id, view);
        }
        let active = self.active_id.clone();
        for (id, p) in &self.panes {
            let on = *id == active;
            p.update(cx, |v, cx| v.set_active(on, window, cx));
        }
    }

    /// The dock unmounts: park live panes so the next dock adopts them.
    fn park_panes(&mut self, cx: &mut App) {
        if self.released {
            return;
        }
        self.released = true;
        self.pane_subs.clear();
        for (id, v) in self.panes.drain() {
            if recur_terminal_view::should_dispose(&id, cx) {
                v.update(cx, |v, cx| v.dispose(cx));
            } else {
                recur_terminal_view::park(v, cx);
            }
        }
    }

    fn drop_pane(&mut self, id: &str, cx: &mut Context<Self>) {
        recur_terminal_view::discard_parked(id, cx);
        self.pane_subs.remove(id);
        if let Some(v) = self.panes.remove(id) {
            v.update(cx, |v, cx| v.dispose(cx));
        }
    }

    /// `spawn`: a new shell tab (sized from the dock body).
    pub fn spawn(
        &mut self,
        title: Option<String>,
        take_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<String>> {
        if take_focus {
            window.focus(&self.focus, cx);
        }
        let (cols, rows) = self.guess_size();
        let svc = self.services.clone();
        let ws = self.workspace_id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let info = call(
                cx,
                svc.terminal.open_terminal(&ws, cols, rows),
                "Failed to open terminal",
            )
            .await?;
            this.update_in(cx, |d, window, cx| {
                let t = title.filter(|t| !t.is_empty()).unwrap_or_else(|| {
                    if info.title.is_empty() {
                        "zsh".into()
                    } else {
                        info.title.clone()
                    }
                });
                d.tabs.push(Tab {
                    id: info.id.clone(),
                    title: t,
                    exited: false,
                    busy: false,
                });
                d.active_id = info.id.clone();
                d.set_active_remote(&info.id, cx);
                d.ensure_panes(window, cx);
                d.emit_tabs(cx);
                info.id.clone()
            })
            .ok()
        })
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.spawn(None, true, window, cx).detach();
    }

    /// `selectTab`.
    pub fn select_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.active_id = id.to_string();
        self.ensure_panes(window, cx);
        if let Some(p) = self.panes.get(id) {
            p.update(cx, |v, cx| v.focus(window, cx));
        }
        self.set_active_remote(id, cx);
        self.emit_tabs(cx);
        self.refresh_find(window, cx);
    }

    /// `rotateTab` (⌥⌘←/→).
    pub fn rotate_tab(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len();
        if n < 2 {
            return;
        }
        let i = self.tabs.iter().position(|t| t.id == self.active_id);
        let next = match i {
            None => 0,
            Some(i) => (i as i32 + dir).rem_euclid(n as i32) as usize,
        };
        let id = self.tabs[next].id.clone();
        self.select_tab(&id, window, cx);
    }

    pub fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.active_id.is_empty() {
            let id = self.active_id.clone();
            self.close_tab(&id, window, cx);
        }
    }

    /// `closeTab`: kill the shell; closing the last tab closes the drawer.
    pub fn close_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_pane(id, cx);
        let was_active = self.active_id == id;
        let idx = self.tabs.iter().position(|t| t.id == id).unwrap_or(0);
        let ws = self.workspace_id.clone();
        self.state.update(cx, |s, cx| {
            let name = s.run.run_name_for_session(&ws, id);
            if !name.is_empty() {
                s.run.clear_run_session(&ws, &name, Some(id));
            }
            s.run.clear_run_tab_by_session_id(&ws, id);
            cx.notify();
        });
        if self.tabs.len() > 1 {
            window.focus(&self.focus, cx);
        }
        self.tabs.retain(|t| t.id != id);
        if was_active {
            match self
                .tabs
                .get(idx.min(self.tabs.len().saturating_sub(1)))
                .map(|t| t.id.clone())
            {
                Some(next) if !self.tabs.is_empty() => self.select_tab(&next, window, cx),
                _ => {
                    self.active_id.clear();
                    self.set_active_remote("", cx);
                }
            }
        }
        self.emit_tabs(cx);
        let svc = self.services.clone();
        let id = id.to_string();
        let empty = self.tabs.is_empty();
        cx.spawn(async move |this, cx| {
            call(
                cx,
                svc.terminal.close_terminal(&id),
                "Failed to close terminal",
            )
            .await;
            if empty {
                let _ = this.update(cx, |d, cx| {
                    let ws = d.workspace_id.clone();
                    d.state
                        .update(cx, |s, cx| s.set_terminal_dock_open(&ws, false, cx));
                });
            }
        })
        .detach();
    }

    fn on_pane_event(
        &mut self,
        id: &str,
        ev: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            TerminalEvent::Title(title) => {
                let ws = self.workspace_id.clone();
                if !self
                    .state
                    .read(cx)
                    .run
                    .run_name_for_session(&ws, id)
                    .is_empty()
                {
                    return;
                }
                let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else {
                    return;
                };
                if title.is_empty() || tab.title == *title {
                    return;
                }
                tab.title = title.clone();
                let (svc, id, title) = (self.services.clone(), id.to_string(), title.clone());
                cx.background_spawn(async move {
                    let _ = svc.terminal.set_terminal_title(&id, &title).await;
                })
                .detach();
                self.emit_tabs(cx);
            }
            TerminalEvent::Exit(code) => self.on_exit(id, *code, cx),
            TerminalEvent::FindResults {
                index,
                total,
                valid,
            } => {
                let (i, n, v) = (*index, *total, *valid);
                self.find.update(cx, |f, cx| f.set_results(i, n, v, cx));
                let _ = window;
            }
        }
    }

    /// `onExit`: the tab stays (its last screen is worth reading) until the backend drops it.
    fn on_exit(&mut self, id: &str, code: i32, cx: &mut Context<Self>) {
        let ws = self.workspace_id.clone();
        self.state.update(cx, |s, cx| {
            let name = s.run.run_name_for_session(&ws, id);
            s.run.clear_run_session(&ws, &name, Some(id));
            cx.notify();
        });
        for t in &mut self.tabs {
            if t.id == id {
                t.title = format!("{} — exited ({code})", t.title);
                t.exited = true;
            }
        }
        self.emit_tabs(cx);
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let Ok(list) = svc.terminal.list_terminals(&ws).await else {
                return;
            };
            let keep: Vec<String> = list.iter().map(|t| t.id.clone()).collect();
            let _ = this.update(cx, |d, cx| {
                let gone: Vec<String> = d
                    .tabs
                    .iter()
                    .filter(|t| !keep.contains(&t.id))
                    .map(|t| t.id.clone())
                    .collect();
                if gone.is_empty() {
                    return;
                }
                for g in &gone {
                    if let Some(v) = recur_terminal_view::take_parked(g, cx) {
                        v.update(cx, |v, cx| v.dispose(cx));
                    }
                }
                let dropped_active = !d.active_id.is_empty() && !keep.contains(&d.active_id);
                d.tabs.retain(|t| keep.contains(&t.id));
                if dropped_active {
                    let live = d
                        .tabs
                        .iter()
                        .find(|t| !t.exited)
                        .or(d.tabs.first())
                        .map(|t| t.id.clone());
                    d.active_id = live.unwrap_or_default();
                    let id = d.active_id.clone();
                    d.set_active_remote(&id, cx);
                }
                d.emit_tabs(cx);
            });
        })
        .detach();
    }

    /// Run requests and focus requests from elsewhere (titlebar, palette, chat chips).
    fn state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace_id.clone();
        if !self.loading
            && self
                .state
                .read(cx)
                .run
                .request
                .as_ref()
                .is_some_and(|r| r.workspace_id == ws)
            && let Some(req) = self.state.update(cx, |s, _| s.take_run_dev_request(&ws))
        {
            self.spawn_and_run_dev(req.cmd, req.name, window, cx);
        }
        let focus = self.state.read(cx).terminal_focus().cloned();
        if let Some(f) = focus
            && f.workspace_id == ws
            && !f.id.is_empty()
            && self.tabs.iter().any(|t| t.id == f.id)
        {
            self.state.update(cx, |s, cx| {
                s.take_terminal_focus(&ws);
                s.set_terminal_dock_open(&ws, true, cx);
            });
            self.select_tab(&f.id, window, cx);
        }
        let opts_font = font_for(self.state.read(cx), cx);
        let theme = self.theme(cx);
        for p in self.panes.values() {
            p.update(cx, |v, cx| {
                v.set_font(opts_font.clone(), cx);
                v.set_theme(theme.clone(), cx);
            });
        }
        cx.notify();
    }

    /// `spawnAndRunDev`: type the command into the run's own login shell (reused while alive).
    fn spawn_and_run_dev(
        &mut self,
        cmd: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = self.workspace_id.clone();
        let prev_id = self.state.read(cx).run.run_tab_id(&ws, &name);
        let prev = self.tabs.iter().find(|t| t.id == prev_id).cloned();
        let svc = self.services.clone();
        let state = self.state.clone();
        self.run_task = Some(cx.spawn_in(window, async move |this, cx| {
            let id = match prev {
                Some(p) if !p.exited => {
                    let _ = this.update_in(cx, |d, window, cx| d.select_tab(&p.id, window, cx));
                    p.id
                }
                other => {
                    if let Some(p) = other {
                        let _ =
                            this.update_in(cx, |d, window, cx| d.drop_run_tab(&p.id, window, cx));
                    }
                    let Ok(t) = this.update_in(cx, |d, window, cx| {
                        d.spawn(Some(name.clone()), true, window, cx)
                    }) else {
                        return;
                    };
                    let Some(id) = t.await else { return };
                    state.update(cx, |s, cx| {
                        s.run.set_run_tab(&ws, &name, &id);
                        cx.notify();
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(450))
                        .await;
                    let alive = this
                        .read_with(cx, |d, _| d.tabs.iter().any(|t| t.id == id && !t.exited))
                        .unwrap_or(false);
                    if !alive {
                        return;
                    }
                    id
                }
            };
            state.update(cx, |s, _| s.mark_run_starting(&id));
            loop {
                let busy = state.read_with(cx, |s, _| s.run.stop_in_flight(&id));
                if !busy {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(25))
                    .await;
            }
            if let Err(err) = svc.terminal.interrupt_terminal(&id).await {
                let msg = recur_view_core::bridge_error_message(&err);
                cx.update(|_, cx| {
                    recur_view_core::toast(
                        recur_ui::widgets::ToastKind::Error,
                        format!("Could not stop the running process: {msg}"),
                        cx,
                    )
                })
                .ok();
                finish_started(&state, &id, cx).await;
                return;
            }
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            state.update(cx, |s, cx| s.set_run_session(&ws, &name, &id, cx));
            let line = if cmd.ends_with('\n') {
                cmd.clone()
            } else {
                format!("{cmd}\n")
            };
            if call(
                cx,
                svc.terminal.write_terminal(&id, &line),
                "Failed to start run",
            )
            .await
            .is_none()
            {
                state.update(cx, |s, cx| {
                    s.run.clear_run_session(&ws, &name, Some(&id));
                    cx.notify();
                });
                finish_started(&state, &id, cx).await;
                return;
            }
            state.update(cx, |s, cx| s.arm_run_busy_watch(&id, cx));
            finish_started(&state, &id, cx).await;
            let _ = this.update_in(cx, |d, window, cx| {
                if let Some(p) = d.panes.get(&id) {
                    p.update(cx, |v, cx| v.focus(window, cx));
                }
            });
        }));
    }

    /// `dropRunTab`: an exited run tab before its replacement spawns.
    fn drop_run_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace_id.clone();
        self.state.update(cx, |s, cx| {
            s.run.clear_run_tab_by_session_id(&ws, id);
            cx.notify();
        });
        self.drop_pane(id, cx);
        let idx = self.tabs.iter().position(|t| t.id == id).unwrap_or(0);
        self.tabs.retain(|t| t.id != id);
        if self.active_id == id {
            let next = self
                .tabs
                .get(idx.saturating_sub(1))
                .or(self.tabs.first())
                .map(|t| t.id.clone())
                .unwrap_or_default();
            self.active_id = next.clone();
            self.set_active_remote(&next, cx);
            self.ensure_panes(window, cx);
        }
        self.emit_tabs(cx);
        let svc = self.services.clone();
        let id = id.to_string();
        cx.spawn(async move |_, cx| {
            call(
                cx,
                svc.terminal.close_terminal(&id),
                "Failed to close terminal",
            )
            .await;
        })
        .detach();
    }

    /// The dock's run button: run the active tab's command, or stop it.
    fn run_or_stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name, running) = self.dock_run(cx);
        let ws = self.workspace_id.clone();
        if running {
            self.state.update(cx, |s, cx| {
                s.request_stop_run(&ws, &name, cx);
            });
            return;
        }
        let s = self.state.read(cx);
        let run = workspace_runs(&ws, &s.workspaces, &s.projects)
            .into_iter()
            .find(|r| r.name == name)
            .or_else(|| workspace_run(&ws, &s.workspaces, &s.projects));
        let Some(run) = run else { return };
        self.spawn_and_run_dev(run.cmd, run.name, window, cx);
    }

    /// `dockRun`: the active tab's run (else the pinned one) and whether it runs.
    fn dock_run(&self, cx: &App) -> (String, bool) {
        let s = self.state.read(cx);
        let from_tab = s
            .run
            .run_name_for_session(&self.workspace_id, &self.active_id);
        let name = if !from_tab.is_empty() {
            from_tab
        } else {
            workspace_run(&self.workspace_id, &s.workspaces, &s.projects)
                .map(|r| r.name)
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "dev".into())
        };
        let running = s.run.is_run_running(&self.workspace_id, &name);
        (name, running)
    }

    // ---- find ----

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        self.find.update(cx, |f, cx| f.focus_and_select(window, cx));
        self.refresh_find(window, cx);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        self.find_open = false;
        if let Some(p) = self.panes.get(&self.searched_id) {
            p.update(cx, |v, cx| v.clear_search(cx));
        }
        self.searched_id.clear();
        self.find.update(cx, |f, _| f.reset_counts());
        if let Some(p) = self.panes.get(&self.active_id) {
            p.update(cx, |v, cx| v.focus(window, cx));
        }
        cx.notify();
    }

    fn step_find(&mut self, dir: i32, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        let q = self.find.read(cx).query(cx);
        let Some(p) = self.panes.get(&self.active_id).cloned() else {
            return;
        };
        if q.is_empty() {
            return;
        }
        let opts = self.find.read(cx).options();
        p.update(cx, |v, cx| {
            if dir > 0 {
                v.find_next(&q, opts, cx);
            } else {
                v.find_previous(&q, opts, cx);
            }
        });
    }

    /// The incremental search `$effect`: query, options or active tab changed.
    fn refresh_find(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        if self.searched_id != self.active_id
            && let Some(p) = self.panes.get(&self.searched_id)
        {
            p.update(cx, |v, cx| v.clear_search(cx));
        }
        self.searched_id = self.active_id.clone();
        let q = self.find.read(cx).query(cx);
        let Some(p) = self.panes.get(&self.active_id).cloned() else {
            self.find.update(cx, |f, _| f.reset_counts());
            return;
        };
        if q.is_empty() {
            self.find.update(cx, |f, _| f.reset_counts());
            p.update(cx, |v, cx| v.clear_search(cx));
            return;
        }
        let mut opts = self.find.read(cx).options();
        opts.incremental = true;
        p.update(cx, |v, cx| {
            v.find_next(&q, opts, cx);
        });
    }

    fn on_find_bar(&mut self, ev: &FindBarEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            FindBarEvent::Changed => self.refresh_find(window, cx),
            FindBarEvent::Next => self.step_find(1, cx),
            FindBarEvent::Previous => self.step_find(-1, cx),
            FindBarEvent::Close => self.close_find(window, cx),
        }
    }

    /// `onDockKey`: the dock's chords while focus is inside it (capture phase).
    fn on_dock_key(
        &mut self,
        e: &gpui_kit::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let in_find = self.find.read(cx).contains_focus(window, cx);
        let Some(action) = dock_key(
            &e.keystroke,
            cfg!(target_os = "macos"),
            self.find_open,
            in_find,
            !self.active_id.is_empty(),
        ) else {
            return;
        };
        cx.stop_propagation();
        match action {
            DockKey::CloseFind => self.close_find(window, cx),
            DockKey::OpenFind => self.open_find(window, cx),
            DockKey::StepFind(dir) => self.step_find(dir, cx),
            DockKey::Copy => {
                if let Some(p) = self.panes.get(&self.active_id) {
                    let sel = p.read(cx).selection_text();
                    if !sel.is_empty() {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(sel));
                    }
                }
            }
            DockKey::Paste => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|c| c.text())
                    .unwrap_or_default();
                if !text.is_empty()
                    && let Some(p) = self.panes.get(&self.active_id).cloned()
                {
                    p.update(cx, |v, cx| v.paste(&text, cx));
                }
            }
            DockKey::ToggleCase | DockKey::ToggleRegexp | DockKey::ToggleWord => {
                self.find.update(cx, |f, cx| match action {
                    DockKey::ToggleCase => f.toggle_case(cx),
                    DockKey::ToggleRegexp => f.toggle_regexp(cx),
                    _ => f.toggle_whole_word(cx),
                });
                self.refresh_find(window, cx);
            }
            DockKey::Rotate(dir) => self.rotate_tab(dir, window, cx),
            DockKey::NewTab => self.spawn(None, true, window, cx).detach(),
            DockKey::CloseTab => self.close_active_tab(window, cx),
        }
    }

    fn reorder(
        &mut self,
        order: Vec<SharedString>,
        before: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.tabs.clone();
        let next: Vec<Tab> = order
            .iter()
            .filter_map(|k| snapshot.iter().find(|t| t.id == k.as_ref()).cloned())
            .collect();
        if next.len() != snapshot.len() {
            return;
        }
        self.tabs = next;
        self.emit_tabs(cx);
        let svc = self.services.clone();
        let ws = self.workspace_id.clone();
        let ids: Vec<String> = order.iter().map(|s| s.to_string()).collect();
        let _ = before;
        self.reorder_task = Some(cx.spawn(async move |this, cx| {
            let ok = call(
                cx,
                svc.terminal.reorder_terminals(&ws, &ids),
                "Could not reorder terminals",
            )
            .await;
            if ok.is_none() {
                let _ = this.update(cx, |d, cx| {
                    d.tabs = snapshot;
                    d.emit_tabs(cx);
                });
            }
        }));
    }
}

async fn finish_started(state: &Entity<AppState>, id: &str, cx: &mut gpui_kit::AsyncWindowContext) {
    cx.background_executor()
        .timer(Duration::from_millis(80))
        .await;
    state.update(cx, |s, _| s.mark_run_started(id));
}

impl TerminalDock {
    fn render_view(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let app = Theme::of(cx).id;
        let ctx = theme_context(self.state.read(cx));
        let term_bg: recur_ui::Color = {
            let c = ctx.surface_bg(app);
            recur_ui::rgba(c.r(), c.g(), c.b(), f32::from(c.a()) / 255.)
        };
        let frosts = ctx.frosts(app);
        let ws = self.workspace_id.clone();
        let maximized = self.state.read(cx).dock.is_maximized(&ws);
        let s = self.state.read(cx);
        let has_runs =
            !recur_view_core::run::workspace_dev_cmd(&ws, &s.workspaces, &s.projects).is_empty();
        let (run_name, running) = self.dock_run(cx);
        let lit = self.props.lit;
        let n = self.tabs.len();
        let mut strip = tab_strip(
            SharedString::from(format!("term-tabs-{ws}")),
            "Terminal sessions",
        )
        .chrome(true)
        .active_key(SharedString::from(self.active_id.clone()))
        .can_start(move |_| n > 1)
        .on_reorder({
            let me = cx.entity().downgrade();
            move |order, before, _, cx| {
                if let Some(d) = me.upgrade() {
                    d.update(cx, |d, cx| d.reorder(order, before, cx));
                }
            }
        });
        for tab in self.tabs.clone() {
            let (id1, id2) = (tab.id.clone(), tab.id.clone());
            let chip = tab_chip(
                SharedString::from(format!("term-tab-{}", tab.id)),
                tab.title.clone(),
            )
            .active(self.active_id == tab.id)
            .lit(lit)
            .muted(tab.exited)
            .reorderable(n > 1)
            .close_label(format!("Close {}", tab.title))
            .on_click(cx.guarded_listener(move |d, _, window, cx| d.select_tab(&id1, window, cx)))
            .on_close(cx.guarded_listener(move |d, _, window, cx| d.close_tab(&id2, window, cx)))
            .on_key_down({
                let id = tab.id.clone();
                let me = cx.entity().downgrade();
                move |e: &gpui_kit::KeyDownEvent, window: &mut Window, cx: &mut App| {
                    if e.keystroke.key != "backspace" && e.keystroke.key != "delete" {
                        return false;
                    }
                    if let Some(d) = me.upgrade() {
                        d.update(cx, |d, cx| d.close_tab(&id, window, cx));
                    }
                    true
                }
            });
            strip = strip.tab(SharedString::from(tab.id.clone()), chip);
        }
        if has_runs {
            let title = if running {
                format!("{} (⌘⇧R)", run_label("Stop", &run_name))
            } else {
                format!("{} (⌘R)", run_label("Run", &run_name))
            };
            strip = strip.trailing(titled(
                "term-run-t",
                title,
                tab_strip_action("term-run", running, false, cx)
                    .debug_selector(|| "term-act:run".into())
                    .child(
                        icon(if running {
                            IconName::SquareFill
                        } else {
                            IconName::Play
                        })
                        .size(px(15.)),
                    )
                    .on_click(cx.guarded_listener(|d, _, window, cx| d.run_or_stop(window, cx))),
            ));
        }
        strip = strip
            .trailing(titled(
                "term-new-t",
                "New Tab (⌘T)",
                tab_strip_action("term-new", false, false, cx)
                    .debug_selector(|| "term-act:new".into())
                    .child(icon(IconName::Plus).size(px(15.)))
                    .on_click(cx.guarded_listener(|d, _, window, cx| d.new_tab(window, cx))),
            ))
            .trailing(titled(
                "term-clear-t",
                "Clear terminal",
                tab_strip_action("term-clear", false, self.active_id.is_empty(), cx)
                    .debug_selector(|| "term-act:clear".into())
                    .child(icon(IconName::Ban).size(px(15.)))
                    .when(!self.active_id.is_empty(), |b| {
                        b.on_click(cx.guarded_listener(|d, _, _, cx| {
                            if let Some(p) = d.panes.get(&d.active_id).cloned() {
                                p.update(cx, |v, cx| v.clear(cx));
                            }
                        }))
                    }),
            ))
            .trailing(titled(
                "term-max-t",
                if maximized {
                    "Restore terminal size (⌃⇧`)"
                } else {
                    "Maximize terminal (⌃⇧`)"
                },
                tab_strip_action("term-max", maximized, false, cx)
                    .debug_selector(|| "term-act:maximize".into())
                    .child(
                        icon(if maximized {
                            IconName::Minimize
                        } else {
                            IconName::Maximize
                        })
                        .size(px(15.)),
                    )
                    .on_click(cx.guarded_listener(move |d, _, _, cx| {
                        let ws = d.workspace_id.clone();
                        d.state.update(cx, |s, cx| {
                            s.set_terminal_dock_maximized(&ws, !maximized, cx)
                        });
                    })),
            ))
            .trailing(titled(
                "term-hide-t",
                "Hide terminal (⌃`)",
                tab_strip_action("term-hide", false, false, cx)
                    .debug_selector(|| "term-act:hide".into())
                    .child(icon(IconName::PanelBottom).size(px(15.)))
                    .on_click(cx.guarded_listener(|d, _, _, cx| {
                        let ws = d.workspace_id.clone();
                        d.state
                            .update(cx, |s, cx| s.set_terminal_dock_open(&ws, false, cx));
                    })),
            ));
        let strip_el: AnyElement = if self.props.overlay {
            div().bg(hsla(t.pane)).child(strip).into_any_element()
        } else {
            strip.into_any_element()
        };
        let mut panes = div()
            .debug_selector(|| "term-dock-panes".into())
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(crate::geometry::probe(&self.body));
        if self.loading {
            panes = panes.child(term_empty("Starting shell…", &t));
        } else if self.tabs.is_empty() {
            panes = panes.child(term_empty("No terminals. Press + to start one.", &t));
        }
        for tab in &self.tabs {
            let Some(p) = self.panes.get(&tab.id) else {
                continue;
            };
            let active = tab.id == self.active_id;
            let view = p.clone();
            let el = div()
                .id(SharedString::from(format!("term-pane-{}", tab.id)))
                .debug_selector({
                    let id = tab.id.clone();
                    move || format!("term-pane:{id}")
                })
                .absolute()
                .inset_0()
                .when(!active, |d| d.invisible())
                .on_drop(
                    move |drag: &recur_view_core::file_drag::WorkspaceFileDrag, window, cx| {
                        let paths = drag.paths.clone();
                        view.update(cx, |v, cx| v.drop_paths(&paths, window, cx));
                    },
                )
                .child(p.clone());
            panes = panes.child(el);
        }
        let mut body = div().flex_1().min_h_0().overflow_hidden().flex().flex_col();
        if self.find_open {
            body = body.child(
                div()
                    .flex_none()
                    .border_b_1()
                    .border_color(hsla(t.line))
                    .bg(hsla(t.pane))
                    .child(self.find.clone()),
            );
        }
        body = body.child(panes);
        let overlay = self.props.overlay;
        let bg = if overlay {
            if frosts {
                term_bg.mix(0.72, recur_ui::color::TRANSPARENT)
            } else {
                term_bg
            }
        } else {
            term_bg
        };
        let beside = self.props.beside;
        let mut root =
            div()
                .id(SharedString::from(format!("term-dock-{ws}")))
                .debug_selector(|| "term-dock".into())
                .track_focus(&self.focus)
                .relative()
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .bg(hsla(bg))
                .capture_key_down(cx.guarded_listener(
                    |d, e: &gpui_kit::KeyDownEvent, window, cx| d.on_dock_key(e, window, cx),
                ));
        if beside {
            root = root.min_w_0().border_l_1().border_color(hsla(t.line));
        } else if !overlay {
            root = root.border_t_1().border_color(hsla(t.line));
        }
        if !beside && !overlay {
            let (state, state2, state3) =
                (self.state.clone(), self.state.clone(), self.state.clone());
            let h = self.state.read(cx).panels.size(PanelKey::TerminalDock);
            let main = self.main_bounds.clone();
            root = root.child(
                resize_handle(
                    "term-dock-resize",
                    Axis::Y,
                    Edge::Start,
                    h,
                    &self.resize_focus,
                )
                .max(move |_, _| {
                    let mh = f32::from(main.get().size.height);
                    if mh <= 0. {
                        return f32::INFINITY;
                    }
                    let floor = (mh * 0.25).round().min(160.);
                    h.max(mh - floor)
                })
                .on_live(move |size, _, cx| {
                    state.update(cx, |s, cx| {
                        s.set_panel_live(PanelKey::TerminalDock, size, cx)
                    });
                })
                .on_commit(move |_, cx| {
                    state2.update(cx, |s, cx| {
                        s.persist_panels();
                        cx.notify();
                    });
                })
                .on_step(move |h, _, cx| {
                    state3.update(cx, |s, cx| s.set_panel(PanelKey::TerminalDock, h, cx));
                }),
            );
        }
        root.child(strip_el).child(body)
    }
}

impl Render for TerminalDock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// What a key does in the focused dock (`onDockKeydown`). macOS uses ⌘ chords; elsewhere
/// bare Ctrl chords belong to the shell (readline, interrupt) and the dock takes Ctrl+Shift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DockKey {
    CloseFind,
    OpenFind,
    StepFind(i32),
    Copy,
    Paste,
    ToggleCase,
    ToggleRegexp,
    ToggleWord,
    Rotate(i32),
    NewTab,
    CloseTab,
}

pub(crate) fn dock_key(
    k: &gpui_kit::Keystroke,
    mac: bool,
    find_open: bool,
    in_find: bool,
    has_active: bool,
) -> Option<DockKey> {
    let m = k.modifiers;
    let modk = if mac { m.platform } else { m.control };
    let key = k.key.as_str();
    if key == "escape" && find_open {
        return Some(DockKey::CloseFind);
    }
    let dock_chord = modk && !m.alt && if mac { !m.shift } else { m.shift };
    if dock_chord && key == "f" {
        return Some(DockKey::OpenFind);
    }
    if find_open && modk && !m.alt {
        let dir = match (mac, m.shift, key) {
            (true, false, "g") => 1,
            (true, true, "g") => -1,
            (false, true, "g") => 1,
            (false, true, "h") => -1,
            _ => 0,
        };
        if dir != 0 {
            return Some(DockKey::StepFind(dir));
        }
    }
    if !mac && dock_chord && !in_find && key == "c" {
        return Some(DockKey::Copy);
    }
    if !mac && dock_chord && !in_find && key == "v" {
        return Some(DockKey::Paste);
    }
    if in_find && modk && m.alt && !m.shift {
        return match key {
            "c" => Some(DockKey::ToggleCase),
            "r" => Some(DockKey::ToggleRegexp),
            "w" => Some(DockKey::ToggleWord),
            _ => None,
        };
    }
    if !modk {
        return None;
    }
    if m.alt && !m.shift && (key == "left" || key == "right") {
        return Some(DockKey::Rotate(if key == "left" { -1 } else { 1 }));
    }
    if !dock_chord {
        return None;
    }
    match key {
        "t" => Some(DockKey::NewTab),
        "w" if has_active => Some(DockKey::CloseTab),
        _ => None,
    }
}

fn term_empty(s: &'static str, t: &recur_ui::Tokens) -> AnyElement {
    div()
        .px(px(12.))
        .py(px(10.))
        .child(text(s).size(11.).color(t.faint))
        .into_any_element()
}

impl crate::obs::Observatory {
    /// The dock for this workspace while it is open (`{#key workspaceId}`): a workspace switch
    /// rebuilds it so it adopts that workspace's shells.
    pub(crate) fn ensure_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.props.workspace_id.clone();
        if ws.is_empty() || !self.term_open(cx) {
            if self.dock.take().is_some() {
                self.term_tab_count = 0;
            }
            return;
        }
        let props = DockProps {
            lit: self.focus_zone == crate::model::FocusZone::Terminal,
            overlay: self.term_overlaying(),
            beside: self.term_beside_layout(cx),
            gpu: self.props.shell_active,
        };
        match &self.dock {
            Some((id, d)) if *id == ws => {
                let d = d.clone();
                d.update(cx, |d, cx| d.set_props(props, cx));
            }
            _ => {
                let state = self.state.clone();
                let main = self.bounds.main.clone();
                let d = cx.new(|cx| {
                    let mut d = TerminalDock::new(state, &ws, props, window, cx);
                    d.set_main_bounds(main);
                    d
                });
                let sub = cx.guarded_subscribe(&d, |o, _, ev: &DockEvent, cx| {
                    let DockEvent::TabsChanged(n) = ev;
                    o.term_tab_count = *n;
                    o.sync_side_effects(cx);
                });
                let focus = d.read(cx).focus.clone();
                self.dock_subs = vec![sub];
                self.dock_subs.extend(self.watch_zone(&focus, window, cx));
                self.dock = Some((ws, d));
            }
        }
    }

    pub(crate) fn dock_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_dock(window, cx);
        if let Some((_, d)) = self.dock.clone() {
            d.update(cx, |d, cx| d.new_tab(window, cx));
        }
    }

    pub(crate) fn dock_close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, d)) = self.dock.clone() {
            d.update(cx, |d, cx| d.close_active_tab(window, cx));
        }
    }

    pub(crate) fn dock_rotate_tab(
        &mut self,
        dir: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, d)) = self.dock.clone() {
            d.update(cx, |d, cx| d.rotate_tab(dir, window, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DockKey, dock_key};

    fn k(s: &str) -> gpui_kit::Keystroke {
        gpui_kit::Keystroke::parse(s).expect("keystroke")
    }

    fn on(s: &str, mac: bool) -> Option<DockKey> {
        dock_key(&k(s), mac, false, false, true)
    }

    #[test]
    fn mac_uses_command_chords() {
        assert_eq!(on("cmd-t", true), Some(DockKey::NewTab));
        assert_eq!(on("cmd-w", true), Some(DockKey::CloseTab));
        assert_eq!(on("cmd-f", true), Some(DockKey::OpenFind));
        assert_eq!(on("cmd-alt-left", true), Some(DockKey::Rotate(-1)));
        assert_eq!(on("cmd-alt-right", true), Some(DockKey::Rotate(1)));
        // Copy / paste ride the app's own Edit menu on macOS.
        assert_eq!(on("cmd-c", true), None);
        assert_eq!(on("ctrl-t", true), None);
        // ⌘W with no tab does nothing.
        assert_eq!(dock_key(&k("cmd-w"), true, false, false, false), None);
    }

    #[test]
    fn off_macos_bare_ctrl_chords_stay_with_the_shell() {
        // Ctrl+T / Ctrl+W / Ctrl+F are readline keys; Ctrl+C is the interrupt.
        for s in ["ctrl-t", "ctrl-w", "ctrl-f", "ctrl-c", "ctrl-v"] {
            assert_eq!(on(s, false), None, "{s}");
        }
        assert_eq!(on("ctrl-shift-t", false), Some(DockKey::NewTab));
        assert_eq!(on("ctrl-shift-w", false), Some(DockKey::CloseTab));
        assert_eq!(on("ctrl-shift-f", false), Some(DockKey::OpenFind));
        assert_eq!(on("ctrl-shift-c", false), Some(DockKey::Copy));
        assert_eq!(on("ctrl-shift-v", false), Some(DockKey::Paste));
        // In the find field, the chord copies the query, not the screen.
        assert_eq!(dock_key(&k("ctrl-shift-c"), false, true, true, true), None);
    }

    #[test]
    fn find_keys_step_toggle_and_close() {
        assert_eq!(
            dock_key(&k("escape"), true, true, true, true),
            Some(DockKey::CloseFind)
        );
        assert_eq!(dock_key(&k("escape"), true, false, false, true), None);
        assert_eq!(
            dock_key(&k("cmd-g"), true, true, true, true),
            Some(DockKey::StepFind(1))
        );
        assert_eq!(
            dock_key(&k("cmd-shift-g"), true, true, true, true),
            Some(DockKey::StepFind(-1))
        );
        assert_eq!(
            dock_key(&k("ctrl-shift-g"), false, true, true, true),
            Some(DockKey::StepFind(1))
        );
        assert_eq!(
            dock_key(&k("ctrl-shift-h"), false, true, true, true),
            Some(DockKey::StepFind(-1))
        );
        assert_eq!(
            dock_key(&k("cmd-alt-c"), true, true, true, true),
            Some(DockKey::ToggleCase)
        );
        assert_eq!(
            dock_key(&k("cmd-alt-r"), true, true, true, true),
            Some(DockKey::ToggleRegexp)
        );
        assert_eq!(
            dock_key(&k("cmd-alt-w"), true, true, true, true),
            Some(DockKey::ToggleWord)
        );
    }
}
