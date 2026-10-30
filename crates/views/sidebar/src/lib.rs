//! The app sidebar: the nav, the project tree with groups,
//! sticky headers, drag reorder and the collapsed-rail flyout, and the footer with provider
//! usage and stats.

mod drag;
pub mod peek;
mod rows;
pub mod tree;
pub mod usage;

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext, Bounds, Context, DispatchPhase, Div, ElementId, Entity,
    FocusHandle, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Subscription, Task, Window, canvas,
    deferred, div, point, px,
};
use recur_domain::{Project, ProjectGroup};
use recur_services::Services;
use recur_ui::fonts::weight;
use recur_ui::motion::{self, Easing};
use recur_ui::shadow::{box_shadow, css_box_shadow, with_shadow};
use recur_ui::style::{self, focus_ring, is_focus_visible};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Axis, Edge, resize_handle};
use recur_ui::{Color, IconName, Theme, icon, rgba};
use recur_view_core::modes::{parked_needs_you_by_workspace, workspace_is_running};
use recur_view_core::pr::{pr_chat_host_running_count, pr_chat_host_unseen_count};
use recur_view_core::prefs::PanelKey;
use recur_view_core::workspace_nav::{
    ScanPeek, ScanStep, project_id_from_main_scan, scan_commit, scan_forget, scan_land,
    scroll_delta_to_reveal, split_scheduled, unique_refs,
};
use recur_view_core::{AppState, ScanKind, ScheduleEditorRequest, ViewContext, ViewName, call};

use crate::drag::Drag;
use crate::peek::{
    DEFAULT_SLACK, PeekFlags, PeekRect, RAIL_PEEK_EDGE_PX, RAIL_PEEK_HIDE, RAIL_PEEK_SHOW,
    pointer_far_from_rail_edge, pointer_in_left_edge, rail_peek_held,
};
use crate::tree::{Collapse, FADE, GROUP_H, GUTTER, Hole, Layout, PROJ_H, Tree};
use crate::usage::UsageMeter;

const REMOVE_ARM: Duration = Duration::from_millis(3000);
const SCROLLBAR_LINGER: Duration = Duration::from_millis(700);
/// `.rail` slide in/out under a collapsed rail.
const SLIDE: Duration = Duration::from_millis(160);
const FADE_IN: Duration = Duration::from_millis(120);

/// A CSS opacity transition: `on` is the target; interrupted runs start from where they were.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Fade {
    on: bool,
    from: f32,
    at: Option<Instant>,
    dur: Duration,
}

impl Fade {
    pub(crate) fn new(on: bool) -> Fade {
        Fade {
            on,
            from: if on { 1. } else { 0. },
            at: None,
            dur: Duration::ZERO,
        }
    }

    pub(crate) fn value(&self, cx: &App) -> f32 {
        let to = if self.on { 1. } else { 0. };
        match self.at {
            Some(at) => {
                self.from + (to - self.from) * motion::eased(at, self.dur, Easing::EASE, cx)
            }
            None => to,
        }
    }

    pub(crate) fn set(&mut self, on: bool, dur: Duration, cx: &App) {
        if on != self.on {
            self.from = self.value(cx);
            self.on = on;
            self.at = Some(recur_ui::motion::now(cx));
            self.dur = dur;
        }
    }

    pub(crate) fn animating(&self, cx: &App) -> bool {
        self.at
            .is_some_and(|at| motion::progress(at, self.dur, cx) < 1.)
    }
}

/// The right-click menu on a project header.
#[derive(Clone, Debug)]
struct MoveMenu {
    project_id: String,
    at: Point<Pixels>,
}

pub struct Sidebar {
    state: Entity<AppState>,
    services: Services,
    main_focus: FocusHandle,
    rail_focus: FocusHandle,
    /// Tab stops for the sidebar's controls, by element key.
    focus: HashMap<SharedString, FocusHandle>,
    usage: Entity<UsageMeter>,
    collapse: Collapse,
    /// Bumped on every store change (keys the tree and row memos).
    pub(crate) state_rev: u64,
    tree_memo: std::cell::RefCell<Option<(u64, Collapse, Rc<Tree>)>>,
    pub(crate) row_ctx_memo: std::cell::RefCell<Option<(u64, bool, Rc<rows::RowCtx>)>>,
    scan: ScanPeek,
    seen_scan_seq: u64,
    seen_selection: (ViewName, String),
    seen_collapsed: bool,
    seen_peek: bool,
    pub(crate) scroll: ScrollHandle,
    seen_scroll: f32,
    scrolling: bool,
    scroll_timer: Option<Task<()>>,
    sb: Fade,
    sec_fade: Fade,
    under: HashMap<String, Fade>,
    carets: HashMap<String, Fade>,
    pub(crate) hovered: Option<SharedString>,
    pub(crate) drag: Option<Drag>,
    pub(crate) flip: HashMap<String, (f32, Instant)>,
    pub(crate) suppress_click: bool,
    renaming: Option<(String, Entity<gpui_kit::base::input::InputState>)>,
    rename_subs: Vec<Subscription>,
    move_menu: Option<MoveMenu>,
    menu_bounds: Option<Bounds<Pixels>>,
    armed: Option<String>,
    arm_seq: u64,
    arm_ready: u64,
    arm_timer: Option<Task<()>>,
    arm_task: Option<Task<()>>,
    remove_echo: bool,
    risk: HashMap<String, String>,
    risk_gen: HashMap<String, u64>,
    risk_inflight: HashSet<String>,
    removing: Option<String>,
    show_timer: Option<Task<()>>,
    hide_timer: Option<Task<()>>,
    edge_hover: bool,
    edge_fade: Fade,
    slide: Fade,
    /// The share of the rail's width the docked slot holds: it eases with the rail as it
    /// docks or undocks, so the content moves with it.
    dock: Fade,
    body_bounds: Option<Bounds<Pixels>>,
    rail_bounds: Option<Bounds<Pixels>>,
    epoch: Instant,
    /// The "Agent running" dots by element id, isolated so their pulse redraws the dot and not
    /// the sidebar. `dots_prev` holds the last render's until this one claims them.
    dots: HashMap<String, Entity<recur_ui::isolated::FieldView>>,
    dots_prev: HashMap<String, Entity<recur_ui::isolated::FieldView>>,
    _subscriptions: Vec<Subscription>,
}

/// The store topics the sidebar reads (the usage meter follows its own).
const SIDEBAR_TOPICS: Topics = Topics::ALL.without(
    Topics::USAGE
        .union(Topics::MENU)
        .union(Topics::RESOURCE)
        .union(Topics::BRIEFING),
);

impl Sidebar {
    /// `main_focus` is the content column, where focus parks when the flyout closes on it.
    pub fn new(
        ctx: ViewContext,
        main_focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = ctx.state.clone();
        let usage = cx.new(|cx| UsageMeter::new(state.clone(), cx));
        let subs = vec![
            cx.observe_state_in(&state, SIDEBAR_TOPICS, window, |this, _, window, cx| {
                this.state_rev += 1;
                this.state_changed(window, cx)
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.on_peek_out(cx);
                }
            }),
        ];
        let collapsed = state.read(cx).rail_collapsed;
        let mut this = Sidebar {
            state,
            services: ctx.services,
            main_focus,
            rail_focus: cx.focus_handle(),
            focus: HashMap::new(),
            usage,
            collapse: Collapse::default(),
            state_rev: 0,
            tree_memo: Default::default(),
            row_ctx_memo: Default::default(),
            scan: ScanPeek::default(),
            seen_scan_seq: 0,
            seen_selection: (ViewName::Briefing, String::new()),
            seen_collapsed: collapsed,
            seen_peek: false,
            scroll: ScrollHandle::new(),
            seen_scroll: 0.,
            scrolling: false,
            scroll_timer: None,
            sb: Fade::new(false),
            sec_fade: Fade::new(false),
            under: HashMap::new(),
            carets: HashMap::new(),
            hovered: None,
            drag: None,
            flip: HashMap::new(),
            suppress_click: false,
            renaming: None,
            rename_subs: Vec::new(),
            move_menu: None,
            menu_bounds: None,
            armed: None,
            arm_seq: 0,
            arm_ready: 0,
            arm_timer: None,
            arm_task: None,
            remove_echo: false,
            risk: HashMap::new(),
            risk_gen: HashMap::new(),
            risk_inflight: HashSet::new(),
            removing: None,
            show_timer: None,
            hide_timer: None,
            edge_hover: false,
            edge_fade: Fade::new(false),
            slide: Fade::new(false),
            dock: Fade::new(!collapsed),
            body_bounds: None,
            rail_bounds: None,
            epoch: recur_ui::motion::now(cx),
            dots: HashMap::new(),
            dots_prev: HashMap::new(),
            _subscriptions: subs,
        };
        this.seed_collapse(cx);
        this
    }

    pub(crate) fn fh(
        &mut self,
        key: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.focus
            .entry(key.into())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// A focusable control: tab stop, keyboard click, and the `:focus-visible` ring.
    pub(crate) fn control(
        &mut self,
        key: impl Into<SharedString>,
        radius: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key: SharedString = key.into();
        let f = self.fh(key.clone(), cx);
        let ring = is_focus_visible(&f, window);
        div()
            .debug_selector(|| key.to_string())
            .id(ElementId::Name(key))
            .track_focus(&f)
            .role(gpui_kit::Role::Button)
            .relative()
            .cursor_pointer()
            .when(ring, |d| d.child(focus_ring(radius, 0., cx)))
    }

    pub(crate) fn focused(&self, key: &str, window: &Window) -> bool {
        self.focus.get(key).is_some_and(|f| f.is_focused(window))
    }

    pub(crate) fn state(&self) -> &Entity<AppState> {
        &self.state
    }

    pub(crate) fn hover_key(&self, key: &str) -> bool {
        self.hovered.as_deref() == Some(key)
    }

    pub(crate) fn set_hover(&mut self, key: &str, on: bool, cx: &mut Context<Self>) {
        if on {
            if !self.hover_key(key) {
                self.hovered = Some(SharedString::from(key.to_string()));
                cx.notify();
            }
        } else if self.hover_key(key) {
            self.hovered = None;
            cx.notify();
        }
    }

    // ---- collapse state ----

    /// Seed per-project and per-group collapse from their persisted `open` flags.
    fn seed_collapse(&mut self, cx: &App) {
        let s = self.state.read(cx);
        for p in &s.projects {
            self.collapse
                .projects
                .entry(p.id.clone())
                .or_insert(!p.open);
        }
        for g in &s.project_groups {
            self.collapse.groups.entry(g.id.clone()).or_insert(!g.open);
        }
    }

    /// The project tree for the stores and folds (memoized per store change: rows, drags
    /// and the header layer read it many times a render).
    pub(crate) fn tree(&self, cx: &App) -> Rc<Tree> {
        if let Some((rev, collapse, tree)) = self.tree_memo.borrow().as_ref()
            && *rev == self.state_rev
            && *collapse == self.collapse
        {
            return tree.clone();
        }
        let tree = Rc::new(Tree::build(self.state.read(cx), &self.collapse));
        *self.tree_memo.borrow_mut() = Some((self.state_rev, self.collapse.clone(), tree.clone()));
        tree
    }

    /// The tree's layout as it renders now (with the drag placeholder).
    pub(crate) fn layout(&self, tree: &Tree) -> Layout {
        let hole = self.drag.as_ref().filter(|d| d.active).map(|d| Hole {
            group: d.group,
            id: &d.id,
            height: d.size.1,
        });
        Layout::of(tree, hole)
    }

    fn state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.seed_collapse(cx);
        let (seq, collapsed, view, selected) = {
            let s = self.state.read(cx);
            (
                s.scan_event.seq,
                s.rail_collapsed,
                s.view,
                s.selected_workspace_id.clone(),
            )
        };
        if seq != self.seen_scan_seq {
            self.seen_scan_seq = seq;
            self.on_scan_event(cx);
        }
        if (view, selected.clone()) != self.seen_selection {
            self.seen_selection = (view, selected.clone());
            if matches!(view, ViewName::Detail | ViewName::Escalation) && !selected.is_empty() {
                self.reveal_scheduled_for(&selected, cx);
                self.scroll_row_into_view(&selected, cx);
            }
        }
        if collapsed != self.seen_collapsed {
            self.seen_collapsed = collapsed;
            if collapsed {
                // The docked rail slides out, pulling the content with it.
                self.slide = Fade::new(true);
                self.slide.set(false, SLIDE, cx);
                self.dock.set(false, SLIDE, cx);
            } else {
                // Pinning a peeked rail docks in place; a hidden one slides in.
                self.dock = Fade::new(self.state.read(cx).rail_peek);
                self.dock.set(true, SLIDE, cx);
                self.body_bounds = None;
                self.edge_hover = false;
                self.edge_fade = Fade::new(false);
                self.clear_peek_timers();
                self.state.update(cx, |s, cx| {
                    s.close_rail_peek(true, cx);
                });
            }
        }
        let peek = self.state.read(cx).rail_peek;
        if peek != self.seen_peek {
            self.seen_peek = peek;
            if !peek && self.rail_focus.contains_focused(window, cx) {
                // `parkFocusFromRail`: keep focus out of the hidden rail.
                self.main_focus.focus(window, cx);
            }
            if collapsed {
                self.slide.set(peek, SLIDE, cx);
            }
        }
        cx.notify();
    }

    // ---- keyboard-cycle scan ----

    fn landing_for(&self, id: &str, cx: &App) -> (String, String) {
        let s = self.state.read(cx);
        let pid = project_id_from_main_scan(id)
            .map(str::to_string)
            .or_else(|| s.workspace(id).map(|w| w.project_id.clone()))
            .unwrap_or_default();
        if pid.is_empty() {
            return (String::new(), String::new());
        }
        let gid = s
            .project(&pid)
            .map(|p| p.group_id.clone())
            .filter(|g| !g.is_empty() && s.project_groups.iter().any(|x| &x.id == g))
            .unwrap_or_default();
        (pid, gid)
    }

    fn apply_scan_diff(
        &mut self,
        expand_projects: &[String],
        expand_groups: &[String],
        collapse_projects: &[String],
        collapse_groups: &[String],
    ) {
        for id in expand_projects {
            self.collapse.projects.insert(id.clone(), false);
        }
        for id in collapse_projects {
            self.collapse.projects.insert(id.clone(), true);
        }
        for id in expand_groups {
            self.collapse.groups.insert(id.clone(), false);
        }
        for id in collapse_groups {
            self.collapse.groups.insert(id.clone(), true);
        }
    }

    fn on_scan_event(&mut self, cx: &mut Context<Self>) {
        let ev = self.state.read(cx).scan_event.clone();
        if ev.kind == ScanKind::Commit {
            let (pid, gid) = self.landing_for(&ev.workspace_id, cx);
            let c = scan_commit(&self.scan, Some((&pid, &gid)));
            self.scan = c.next;
            self.apply_scan_diff(&[], &[], &c.collapse_projects, &c.collapse_groups);
            if !c.persist_projects.is_empty() || !c.persist_groups.is_empty() {
                self.persist_opens(c.persist_projects, c.persist_groups, cx);
            }
            if let Some(main_pid) = project_id_from_main_scan(&ev.workspace_id) {
                let seq = ev.seq;
                if self.state.read(cx).project(main_pid).is_some() {
                    let open = self
                        .state
                        .update(cx, |s, cx| s.open_main_workspace(main_pid, Some(seq), cx));
                    let state = self.state.clone();
                    cx.spawn(async move |_, cx| {
                        open.await;
                        state.update(cx, |s, cx| {
                            if s.scan_event.seq == seq && s.scan_pending {
                                s.set_scan_pending(false, cx);
                            }
                        });
                    })
                    .detach();
                } else {
                    self.state.update(cx, |s, cx| s.set_scan_pending(false, cx));
                }
            }
            return;
        }
        let main_pid = project_id_from_main_scan(&ev.workspace_id).map(str::to_string);
        let ws_id = if main_pid.is_some() {
            None
        } else {
            self.state
                .read(cx)
                .workspace(&ev.workspace_id)
                .map(|w| w.id.clone())
        };
        let (pid, gid) = self.landing_for(&ev.workspace_id, cx);
        if pid.is_empty() {
            return;
        }
        let step = ScanStep {
            project_collapsed: self.collapse.projects.get(&pid).copied().unwrap_or(false),
            group_collapsed: !gid.is_empty()
                && self.collapse.groups.get(&gid).copied().unwrap_or(false),
            project_id: pid,
            group_id: gid,
        };
        let (next, diff) = scan_land(&self.scan, &step);
        self.scan = next;
        self.apply_scan_diff(
            &diff.expand_projects,
            &diff.expand_groups,
            &diff.collapse_projects,
            &diff.collapse_groups,
        );
        if let Some(id) = ws_id {
            self.reveal_scheduled_for(&id, cx);
        }
        self.scroll_row_into_view(&ev.workspace_id, cx);
    }

    fn persist_opens(
        &mut self,
        projects: Vec<String>,
        groups: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let services = self.services.clone();
        cx.spawn(async move |_, cx| {
            for id in projects {
                let Some(p) = state.read_with(cx, |s, _| s.project(&id).cloned()) else {
                    continue;
                };
                if p.open {
                    continue;
                }
                let next = Project { open: true, ..p };
                if call(
                    cx,
                    services.projects.update_project_run_config(&next),
                    "Could not save project view",
                )
                .await
                .is_some()
                {
                    state.update(cx, |s, cx| s.update_project(&id, |x| x.open = true, cx));
                }
            }
            for id in groups {
                let Some(g) = state.read_with(cx, |s, _| {
                    s.project_groups.iter().find(|x| x.id == id).cloned()
                }) else {
                    continue;
                };
                if g.open {
                    continue;
                }
                let next = ProjectGroup { open: true, ..g };
                if call(
                    cx,
                    services.projects.update_project_group(&next),
                    "Could not save group view",
                )
                .await
                .is_some()
                {
                    state.update(cx, |s, cx| set_group_open(s, &id, true, cx));
                }
            }
        })
        .detach();
    }

    /// Opening a scheduled workspace expands the Scheduled subgroup so the row can show.
    fn reveal_scheduled_for(&mut self, workspace_id: &str, cx: &App) {
        let s = self.state.read(cx);
        let Some(w) = s.workspace(workspace_id) else {
            return;
        };
        if w.is_main || w.archived || !w.host_pr_id.trim().is_empty() {
            return;
        }
        let siblings: Vec<&recur_domain::Workspace> = s
            .workspaces
            .iter()
            .filter(|x| {
                x.project_id == w.project_id
                    && !x.archived
                    && !x.is_main
                    && x.host_pr_id.trim().is_empty()
            })
            .collect();
        if !split_scheduled(&siblings).1.iter().any(|x| x.id == w.id) {
            return;
        }
        let pid = w.project_id.clone();
        if self.collapse.scheduled.get(&pid) == Some(&false) {
            return;
        }
        self.collapse.scheduled.insert(pid, false);
    }

    /// Scroll the tree so the row clears the sticky headers and the bottom fade.
    pub(crate) fn scroll_row_into_view(&mut self, workspace_id: &str, cx: &App) {
        if workspace_id.is_empty() {
            return;
        }
        let tree = self.tree(cx);
        let layout = self.layout(&tree);
        let Some(&row_top) = layout.rows.get(workspace_id) else {
            return;
        };
        let view_h = f32::from(self.scroll.bounds().size.height);
        if view_h <= 0. {
            return;
        }
        let pid = project_id_from_main_scan(workspace_id)
            .map(str::to_string)
            .or_else(|| {
                self.state
                    .read(cx)
                    .workspace(workspace_id)
                    .map(|w| w.project_id.clone())
            })
            .unwrap_or_default();
        let in_group = layout.project(&pid).is_some_and(|(_, g)| g);
        let top_inset = PROJ_H + if in_group { GROUP_H } else { 0. };
        let scroll = self.scroll_top();
        let delta = scroll_delta_to_reveal(
            row_top - scroll,
            row_top - scroll + tree::ROW_H,
            0.,
            view_h,
            top_inset,
            FADE,
        );
        if delta == 0. {
            return;
        }
        let max = (layout.content_h - view_h).max(0.);
        self.set_scroll_top((scroll + delta).clamp(0., max));
    }

    pub(crate) fn scroll_top(&self) -> f32 {
        -f32::from(self.scroll.offset().y)
    }

    pub(crate) fn set_scroll_top(&self, y: f32) {
        self.scroll.set_offset(point(px(0.), px(-y)));
    }

    // ---- toggles ----

    pub(crate) fn toggle_project(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(p) = self.state.read(cx).project(id).cloned() else {
            return;
        };
        self.scan = scan_forget(&self.scan, Some(id), None);
        let next = !self.collapse.projects.get(id).copied().unwrap_or(false);
        self.collapse.projects.insert(id.to_string(), next);
        cx.notify();
        let services = self.services.clone();
        let state = self.state.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let saved = call(
                cx,
                services
                    .projects
                    .update_project_run_config(&Project { open: !next, ..p }),
                "Could not save project view",
            )
            .await;
            match saved {
                Some(saved) => state.update(cx, |s, cx| {
                    s.update_project(&id, |x| x.open = saved.open, cx)
                }),
                None => {
                    let _ = this.update(cx, |this, cx| {
                        this.collapse.projects.insert(id, !next);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn toggle_group(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(g) = self
            .state
            .read(cx)
            .project_groups
            .iter()
            .find(|g| g.id == id)
            .cloned()
        else {
            return;
        };
        self.scan = scan_forget(&self.scan, None, Some(id));
        let next = !self.collapse.groups.get(id).copied().unwrap_or(false);
        self.collapse.groups.insert(id.to_string(), next);
        cx.notify();
        let services = self.services.clone();
        let state = self.state.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let saved = call(
                cx,
                services
                    .projects
                    .update_project_group(&ProjectGroup { open: !next, ..g }),
                "Could not save group view",
            )
            .await;
            match saved {
                Some(saved) => state.update(cx, |s, cx| set_group_open(s, &id, saved.open, cx)),
                None => {
                    let _ = this.update(cx, |this, cx| {
                        this.collapse.groups.insert(id, !next);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn toggle_scheduled(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let closed = self
            .collapse
            .scheduled
            .get(project_id)
            .copied()
            .unwrap_or(true);
        self.collapse
            .scheduled
            .insert(project_id.to_string(), !closed);
        cx.notify();
    }

    fn toggle_show_hidden(&mut self, cx: &mut Context<Self>) {
        let Some(cur) = self.state.read(cx).settings.clone() else {
            return;
        };
        let next = recur_domain::Settings {
            show_hidden_projects: !cur.show_hidden_projects,
            ..cur
        };
        let services = self.services.clone();
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            if let Some(saved) = call(
                cx,
                services.settings.set_settings(&next),
                "Could not save settings",
            )
            .await
            {
                state.update(cx, |s, cx| s.set_settings(saved, cx));
            }
        })
        .detach();
    }

    // ---- groups ----

    fn add_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let services = self.services.clone();
        let state = self.state.clone();
        let group = ProjectGroup {
            name: "New group".into(),
            open: true,
            ..ProjectGroup::default()
        };
        cx.spawn_in(window, async move |this, cx| {
            let Some(saved) = call(
                cx,
                services.projects.add_project_group(&group),
                "Could not create group",
            )
            .await
            else {
                return;
            };
            state.update(cx, |s, cx| {
                if !s.project_groups.iter().any(|g| g.id == saved.id) {
                    let mut list = s.project_groups.clone();
                    list.push(saved.clone());
                    s.set_project_groups(list, cx);
                }
            });
            let _ = this.update_in(cx, |this, window, cx| {
                this.collapse.groups.insert(saved.id.clone(), false);
                this.start_rename(&saved.id, &saved.name, window, cx);
            });
        })
        .detach();
    }

    pub(crate) fn start_rename(
        &mut self,
        id: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::base::input::InputEvent;
        let input = style::text_input(&SharedString::default(), window, cx);
        input.update(cx, |s, cx| {
            s.set_value(name.to_string(), window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.rename_subs = vec![cx.guarded_subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(cx),
                _ => {}
            },
        )];
        self.renaming = Some((id.to_string(), input));
        cx.notify();
    }

    pub(crate) fn renaming(&self) -> Option<&(String, Entity<gpui_kit::base::input::InputState>)> {
        self.renaming.as_ref()
    }

    pub(crate) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming = None;
        self.rename_subs.clear();
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some((id, input)) = self.renaming.take() else {
            return;
        };
        self.rename_subs.clear();
        cx.notify();
        let draft = input.read(cx).value().trim().to_string();
        let name = if draft.is_empty() {
            "New group".to_string()
        } else {
            draft
        };
        let Some(g) = self
            .state
            .read(cx)
            .project_groups
            .iter()
            .find(|g| g.id == id)
            .cloned()
        else {
            return;
        };
        if g.name == name {
            return;
        }
        let services = self.services.clone();
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            if let Some(saved) = call(
                cx,
                services
                    .projects
                    .update_project_group(&ProjectGroup { name, ..g }),
                "Could not rename group",
            )
            .await
            {
                state.update(cx, |s, cx| {
                    let list = s
                        .project_groups
                        .iter()
                        .map(|x| {
                            if x.id == id {
                                ProjectGroup {
                                    name: saved.name.clone(),
                                    ..x.clone()
                                }
                            } else {
                                x.clone()
                            }
                        })
                        .collect();
                    s.set_project_groups(list, cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn delete_group(&mut self, id: &str, cx: &mut Context<Self>) {
        let (prev_groups, prev_projects) = {
            let s = self.state.read(cx);
            (s.project_groups.clone(), s.projects.clone())
        };
        self.state.update(cx, |s, cx| {
            let groups = prev_groups.iter().filter(|g| g.id != id).cloned().collect();
            let projects = prev_projects
                .iter()
                .map(|p| {
                    let mut p = p.clone();
                    if p.group_id == id {
                        p.group_id.clear();
                    }
                    p
                })
                .collect();
            s.set_project_groups(groups, cx);
            s.set_projects(projects, cx);
        });
        let services = self.services.clone();
        let state = self.state.clone();
        let id = id.to_string();
        cx.spawn(async move |_, cx| {
            if call(
                cx,
                services.projects.delete_project_group(&id),
                "Could not delete group",
            )
            .await
            .is_none()
            {
                state.update(cx, |s, cx| {
                    s.set_project_groups(prev_groups, cx);
                    s.set_projects(prev_projects, cx);
                });
            }
        })
        .detach();
    }

    // ---- move menu ----

    pub(crate) fn open_move_menu(
        &mut self,
        project_id: &str,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.move_menu = Some(MoveMenu {
            project_id: project_id.to_string(),
            at,
        });
        self.sync_peek_lock(cx);
        cx.notify();
    }

    fn close_move_menu(&mut self, cx: &mut Context<Self>) {
        if self.move_menu.take().is_some() {
            self.menu_bounds = None;
            self.sync_peek_lock(cx);
            cx.notify();
        }
    }

    fn move_project_to_group(&mut self, project_id: &str, group_id: &str, cx: &mut Context<Self>) {
        self.close_move_menu(cx);
        let prev = self.state.read(cx).projects.clone();
        let mut next: Vec<Project> = prev.clone();
        if let Some(p) = next.iter_mut().find(|p| p.id == project_id) {
            p.group_id = group_id.to_string();
        }
        let bucket: Vec<Project> = next
            .iter()
            .filter(|p| p.group_id == group_id)
            .cloned()
            .collect();
        let groups = self.state.read(cx).project_groups.clone();
        let next = drag::rebuild_order(&next, &groups, group_id, &bucket);
        self.state.update(cx, |s, cx| s.set_projects(next, cx));
        let services = self.services.clone();
        let state = self.state.clone();
        let (pid, gid) = (project_id.to_string(), group_id.to_string());
        cx.spawn(async move |_, cx| {
            let Some(saved) = call(
                cx,
                services.projects.set_project_group(&pid, &gid),
                "Could not move project",
            )
            .await
            else {
                state.update(cx, |s, cx| s.set_projects(prev, cx));
                return;
            };
            let ids = state.update(cx, |s, cx| {
                s.update_project(&pid, |p| p.group_id = saved.group_id.clone(), cx);
                s.projects.iter().map(|p| p.id.clone()).collect::<Vec<_>>()
            });
            if call(
                cx,
                services.projects.reorder_projects(&ids),
                "Could not reorder projects",
            )
            .await
            .is_none()
            {
                // Membership may already be persisted: refetch instead of restoring.
                let _ = state.update(cx, |s, cx| s.load_initial(cx)).await;
            }
        })
        .detach();
    }

    // ---- remove (two-click) ----

    fn fetch_risk(&mut self, id: &str, cx: &mut Context<Self>) -> Task<bool> {
        let gen_ = {
            let g = self.risk_gen.entry(id.to_string()).or_default();
            *g += 1;
            *g
        };
        self.risk_inflight.insert(id.to_string());
        let services = self.services.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let res = services.workspaces.workspace_discard_risk(&id).await;
            this.update(cx, |this, cx| {
                if this.risk_gen.get(&id) != Some(&gen_) {
                    return false;
                }
                this.risk_inflight.remove(&id);
                match res {
                    Ok(risk) => {
                        this.risk.insert(id, risk);
                        cx.notify();
                        true
                    }
                    Err(_) => {
                        this.risk.remove(&id);
                        false
                    }
                }
            })
            .unwrap_or(false)
        })
    }

    /// Hover may reuse a cached or in-flight inspect.
    pub(crate) fn prefetch_risk(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.risk_inflight.contains(id) || self.risk.contains_key(id) {
            return;
        }
        self.fetch_risk(id, cx).detach();
    }

    /// Arming always issues a fresh inspect.
    fn arm_remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.arm_seq += 1;
        let token = self.arm_seq;
        self.armed = Some(id.to_string());
        self.arm_ready = 0;
        self.sync_peek_lock(cx);
        let fetch = self.fetch_risk(id, cx);
        let id = id.to_string();
        self.arm_task = Some(cx.spawn(async move |this, cx| {
            let ok = fetch.await;
            let _ = this.update(cx, |this, cx| {
                if token != this.arm_seq || this.armed.as_deref() != Some(&id) {
                    return;
                }
                if !ok {
                    this.clear_armed(cx);
                    return;
                }
                this.arm_ready = token;
                this.restart_arm_timer(&id, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn clear_armed(&mut self, cx: &mut Context<Self>) {
        self.arm_timer = None;
        if self.armed.take().is_some() {
            cx.notify();
        }
        self.arm_ready = 0;
        self.sync_peek_lock(cx);
    }

    fn restart_arm_timer(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.arm_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REMOVE_ARM).await;
            let _ = this.update(cx, |this, cx| {
                this.arm_timer = None;
                if this.armed.as_deref() == Some(&id) {
                    this.armed = None;
                    this.arm_ready = 0;
                    this.sync_peek_lock(cx);
                    cx.notify();
                }
            });
        }));
    }

    fn commit_remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.clear_armed(cx);
        self.removing = Some(id.to_string());
        self.risk.remove(id);
        let task = self.state.update(cx, |s, cx| s.remove_workspace(id, cx));
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |this, cx| {
                if this.removing.as_deref() == Some(&id) {
                    this.removing = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn armed(&self) -> Option<&str> {
        self.armed.as_deref()
    }

    pub(crate) fn remove_title(&self, id: &str, can: bool) -> String {
        if !can {
            return "Stop the workspace first".into();
        }
        if self.armed.as_deref() != Some(id) {
            return "Remove".into();
        }
        match self.risk.get(id).filter(|r| !r.is_empty()) {
            Some(risk) => format!("Confirm remove — {risk}"),
            None => "Confirm remove".into(),
        }
    }

    pub(crate) fn on_remove_pointer_up(&mut self, id: &str, can: bool, cx: &mut Context<Self>) {
        if !can || self.removing.is_some() || self.armed.as_deref() != Some(id) {
            return;
        }
        if self.arm_ready != self.arm_seq {
            return;
        }
        self.remove_echo = true;
        self.commit_remove(id, cx);
    }

    pub(crate) fn on_remove_click(&mut self, id: &str, can: bool, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.remove_echo) {
            return;
        }
        if !can || self.removing.is_some() {
            return;
        }
        if self.armed.as_deref() != Some(id) {
            self.arm_remove(id, cx);
            return;
        }
        if self.arm_ready != self.arm_seq {
            return;
        }
        self.commit_remove(id, cx);
    }

    pub(crate) fn on_view_pr(&mut self, pr_id: &str, cx: &mut Context<Self>) {
        self.clear_armed(cx);
        self.state.update(cx, |s, cx| s.open_pr(pr_id, cx));
    }

    // ---- collapsed-rail flyout ----

    fn peek_locked(&self, cx: &App) -> bool {
        rail_peek_held(
            self.state.read(cx).rail_collapsed,
            self.move_menu.is_some() || self.armed.is_some(),
            self.drag.as_ref().is_some_and(|d| d.active),
        )
    }

    /// Mirror the hold into the shared store; a hold cancels a pending hide.
    pub(crate) fn sync_peek_lock(&mut self, cx: &mut Context<Self>) {
        let locked = self.peek_locked(cx);
        if locked {
            self.hide_timer = None;
        }
        self.state
            .update(cx, |s, cx| s.set_rail_peek_locked(locked, cx));
    }

    fn clear_peek_timers(&mut self) {
        self.show_timer = None;
        self.hide_timer = None;
    }

    fn set_edge_hover(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.edge_hover != on {
            self.edge_hover = on;
            cx.notify();
        }
    }

    fn show_peek_now(&mut self, cx: &mut Context<Self>) {
        self.clear_peek_timers();
        if !self.state.read(cx).rail_collapsed {
            return;
        }
        self.set_edge_hover(false, cx);
        self.state.update(cx, |s, cx| s.open_rail_peek(cx));
    }

    fn hide_peek_now(&mut self, force: bool, cx: &mut Context<Self>) {
        if !force && self.peek_locked(cx) {
            return;
        }
        self.clear_peek_timers();
        self.set_edge_hover(false, cx);
        self.state.update(cx, |s, cx| {
            s.close_rail_peek(force, cx);
        });
    }

    fn schedule_show_peek(&mut self, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        if !s.rail_collapsed || s.rail_peek {
            return;
        }
        self.hide_timer = None;
        if self.show_timer.is_some() {
            return;
        }
        self.show_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RAIL_PEEK_SHOW).await;
            let _ = this.update(cx, |this, cx| {
                this.show_timer = None;
                this.show_peek_now(cx);
            });
        }));
    }

    fn schedule_hide_peek(&mut self, cx: &mut Context<Self>) {
        if !self.state.read(cx).rail_collapsed || self.peek_locked(cx) {
            return;
        }
        self.set_edge_hover(false, cx);
        self.show_timer = None;
        if !self.state.read(cx).rail_peek || self.hide_timer.is_some() {
            return;
        }
        self.hide_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RAIL_PEEK_HIDE).await;
            let _ = this.update(cx, |this, cx| {
                this.hide_timer = None;
                this.hide_peek_now(false, cx);
            });
        }));
    }

    /// Pointer over the flyout itself or the move menu (`isRailPeekSurface`).
    fn on_peek_surface(&self, pos: Point<Pixels>, cx: &App) -> bool {
        let peek = self.state.read(cx).rail_peek;
        (peek && self.rail_bounds.is_some_and(|b| b.contains(&pos)))
            || self.menu_bounds.is_some_and(|b| b.contains(&pos))
    }

    fn on_peek_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let (collapsed, peek) = {
            let s = self.state.read(cx);
            (s.rail_collapsed, s.rail_peek)
        };
        if !collapsed {
            return;
        }
        let Some(body) = self.body_bounds else {
            return;
        };
        let locked = self.peek_locked(cx);
        let flags = PeekFlags {
            peeking: peek,
            show_pending: self.show_timer.is_some(),
            locked,
        };
        let (x, y) = (f32::from(pos.x), f32::from(pos.y));
        if pointer_far_from_rail_edge(x, flags, DEFAULT_SLACK, f32::from(body.left())) {
            return;
        }
        let rect = PeekRect {
            left: f32::from(body.left()),
            top: f32::from(body.top()),
            right: f32::from(body.right()),
            bottom: f32::from(body.bottom()),
        };
        let in_edge = pointer_in_left_edge(x, y, rect, RAIL_PEEK_EDGE_PX);
        self.set_edge_hover(in_edge && !peek, cx);
        if in_edge || self.on_peek_surface(pos, cx) || locked {
            self.schedule_show_peek(cx);
            self.hide_timer = None;
            return;
        }
        self.schedule_hide_peek(cx);
    }

    fn on_peek_down(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let (collapsed, peek) = {
            let s = self.state.read(cx);
            (s.rail_collapsed, s.rail_peek)
        };
        if !collapsed {
            return;
        }
        if self.on_peek_surface(pos, cx) {
            self.show_peek_now(cx);
            return;
        }
        if peek && !self.peek_locked(cx) {
            self.hide_peek_now(true, cx);
        }
    }

    fn on_peek_out(&mut self, cx: &mut Context<Self>) {
        if !self.state.read(cx).rail_collapsed || self.peek_locked(cx) {
            return;
        }
        self.set_edge_hover(false, cx);
        self.show_timer = None;
        if self.state.read(cx).rail_peek {
            self.hide_peek_now(false, cx);
        }
    }

    // ---- render ----

    /// The amber running dot (`.sd.pulse`): a 6px glowing dot in the 14px icon slot.
    /// The pulsing "Agent running" dot, with element id `id`.
    pub(crate) fn running_dot(&mut self, id: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let epoch = self.epoch;
        let dot = self.dots_prev.remove(id).unwrap_or_else(|| {
            recur_ui::isolated::FieldView::new(move |window, cx| running_dot(epoch, window, cx), cx)
        });
        self.dots.insert(id.to_string(), dot.clone());
        div()
            .id(SharedString::from(id.to_string()))
            .size(px(14.))
            .flex_none()
            .child(recur_ui::isolated::FieldView::element(
                &dot,
                div().size(px(14.)).style().clone(),
                cx,
            ))
    }

    fn nav_row(
        &mut self,
        key: &'static str,
        on: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = Theme::of(cx).tokens;
        let group: SharedString = format!("{key}-g").into();
        self.control(key, 4., window, cx)
            .group(group)
            .flex()
            .items_center()
            .gap(px(9.))
            .h(px(30.))
            .mt(px(2.))
            .px(px(9.))
            .w_full()
            .rounded(px(6.))
            .text_color(if on { t.text } else { t.muted })
            .when(on, |d| d.bg(t.pane_2))
            .when(!on, |d| {
                d.hover(move |s| s.bg(t.pane).text_color(Hsla::from(t.text)))
            })
    }

    fn nav_icon(name: IconName, on: bool, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        if on {
            icon(name).size(px(14.)).color(t.amber).into_any_element()
        } else {
            // `.nav svg { opacity: .8 }` over the row's own (hover-following) color.
            div()
                .flex_none()
                .opacity(0.8)
                .child(icon(name).size(px(14.)))
                .into_any_element()
        }
    }

    fn chip(label: impl Into<SharedString>, coral: bool, cx: &App) -> Div {
        let t = Theme::of(cx).tokens;
        let (bg, fg) = if coral {
            (t.coral, recur_ui::color::hex(0x0a0c10))
        } else {
            (t.pane_2, t.faint)
        };
        div()
            .flex_none()
            .rounded(px(10.))
            .px(px(6.))
            .bg(bg)
            .child(style::button_label(label, 10.5, weight::SEMIBOLD).color(fg))
    }

    /// An unread chip (unseen chats plus parked prompts), `9+` past nine.
    pub(crate) fn unread_chip(
        &self,
        key: &str,
        unseen: i64,
        parked: usize,
        cx: &App,
    ) -> Option<AnyElement> {
        let n = unseen.max(0) as usize + parked;
        if n == 0 {
            return None;
        }
        let label = if n > 9 {
            "9+".to_string()
        } else {
            n.to_string()
        };
        let title = recur_view_core::modes::sidebar_unread_title(unseen.max(0) as usize, parked);
        Some(
            titled(
                SharedString::from(format!("{key}-ct")),
                title,
                Self::chip(label, true, cx).debug_selector(|| format!("ct:{key}")),
            )
            .into_any_element(),
        )
    }

    fn render_top(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let (active, unread, pr_count, pr_running, pr_unseen) = {
            let s = self.state.read(cx);
            (
                s.view.nav_key(),
                s.inbox.iter().filter(|i| i.unread).count(),
                s.prs.len(),
                pr_chat_host_running_count(&s.workspaces, &s.workspaces_running),
                pr_chat_host_unseen_count(&s.workspaces, &s.attention.by_workspace),
            )
        };
        let on = |v: ViewName| active == Some(v);
        let go = |v: ViewName, state: Entity<AppState>| {
            move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
                state.update(cx, |s, cx| s.go(v, None, cx));
            }
        };
        let st = self.state.clone();
        let inbox = self
            .nav_row("nav-inbox", on(ViewName::Briefing), window, cx)
            .on_click(go(ViewName::Briefing, st.clone()))
            .child(Self::nav_icon(IconName::Inbox, on(ViewName::Briefing), cx))
            .child(style::button_label("Inbox", 12.5, weight::REGULAR))
            .child(
                div()
                    .ml_auto()
                    .child(Self::chip(unread.to_string(), unread > 0, cx)),
            );
        let issues = self
            .nav_row("nav-issues", on(ViewName::Issues), window, cx)
            .on_click(go(ViewName::Issues, st.clone()))
            .child(Self::nav_icon(
                IconName::CircleDot,
                on(ViewName::Issues),
                cx,
            ))
            .child(style::button_label("Issues", 12.5, weight::REGULAR));
        let pr_icon = if pr_running > 0 {
            titled(
                "nav-pr-sd",
                "Agent running",
                self.running_dot("nav-pr-sd-dot", cx),
            )
            .into_any_element()
        } else {
            Self::nav_icon(IconName::GitPullRequest, on(ViewName::Pr), cx)
        };
        let unseen_label = format!(
            "{pr_unseen} unseen agent response{}",
            if pr_unseen == 1 { "" } else { "s" }
        );
        let prs = self
            .nav_row("nav-pr", on(ViewName::Pr), window, cx)
            .on_click(go(ViewName::Pr, st.clone()))
            .child(pr_icon)
            .child(style::button_label("Pull requests", 12.5, weight::REGULAR))
            .child(
                div().ml_auto().child(
                    Self::chip(pr_count.to_string(), false, cx)
                        .debug_selector(move || format!("ct:nav-pr:{pr_count}")),
                ),
            )
            .when(pr_unseen > 0, |d| {
                d.child(titled(
                    "nav-pr-unseen",
                    unseen_label.clone(),
                    Self::chip(
                        if pr_unseen > 9 {
                            "9+".into()
                        } else {
                            pr_unseen.to_string()
                        },
                        true,
                        cx,
                    )
                    .id("nav-pr-unseen-chip")
                    .aria_label(unseen_label.clone()),
                ))
            });
        let hovered = self.hover_key("nav-schedules");
        let add_focus = self.fh("nav-schedules-add", cx);
        let padd_focus = is_focus_visible(&add_focus, window);
        let state = self.state.clone();
        let add = self
            .control("nav-schedules-add", 4., window, cx)
            .aria_label("New schedule")
            .size(px(20.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.faint)
            .opacity(if hovered || padd_focus { 1. } else { 0. })
            .hover(move |s| s.bg(t.pane_2).text_color(Hsla::from(t.text)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                state.update(cx, |s, cx| {
                    s.set_schedule_editor(Some(ScheduleEditorRequest::default()), cx)
                });
            })
            .child(icon(IconName::Plus).size(px(14.)));
        let schedules =
            self.nav_row("nav-schedules", on(ViewName::Schedules), window, cx)
                .on_hover(cx.guarded_listener(|this, h: &bool, _, cx| {
                    this.set_hover("nav-schedules", *h, cx)
                }))
                .on_click(go(ViewName::Schedules, st))
                .child(Self::nav_icon(IconName::Clock, on(ViewName::Schedules), cx))
                .child(style::button_label("Schedules", 12.5, weight::REGULAR))
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(px(1.))
                        .flex_none()
                        .child(titled("nav-schedules-add-tip", "New schedule", add)),
                );
        div()
            .flex_none()
            .pt(px(6.))
            .px(px(GUTTER))
            .pb(px(6.))
            .child(inbox)
            .child(issues)
            .child(prs)
            .child(schedules)
    }

    fn sec_button(
        &mut self,
        key: &'static str,
        label: &SharedString,
        name: IconName,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = Theme::of(cx).tokens;
        self.control(key, 4., window, cx)
            .aria_label(label.clone())
            .size(px(22.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(t.faint)
            .hover(move |s| s.bg(t.pane).text_color(Hsla::from(t.muted)))
            .child(icon(name).size(px(14.)))
    }

    fn render_sec(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let (show_hidden, hidden_count) = {
            let s = self.state.read(cx);
            (
                s.show_hidden_projects(),
                unique_refs(&s.projects).iter().filter(|p| p.hidden).count(),
            )
        };
        let mut sa = div().ml_auto().flex().gap(px(2.));
        if hidden_count > 0 || show_hidden {
            let label: SharedString = if show_hidden {
                "Hide hidden projects".into()
            } else {
                format!(
                    "Show {hidden_count} hidden project{}",
                    if hidden_count == 1 { "" } else { "s" }
                )
                .into()
            };
            let name = if show_hidden {
                IconName::Eye
            } else {
                IconName::EyeOff
            };
            let b = self
                .sec_button("sec-hidden", &label, name, window, cx)
                .aria_toggled(if show_hidden {
                    gpui_kit::Toggled::True
                } else {
                    gpui_kit::Toggled::False
                })
                .on_click(cx.guarded_listener(|this, _, _, cx| this.toggle_show_hidden(cx)));
            sa = sa.child(titled("sec-hidden-tip", label, b));
        }
        let label: SharedString = "Add group".into();
        let add_group = self
            .sec_button("sec-add-group", &label, IconName::FolderPlus, window, cx)
            .on_click(cx.guarded_listener(|this, _, window, cx| this.add_group(window, cx)));
        sa = sa.child(titled("sec-add-group-tip", label, add_group));
        let label: SharedString = "Add project".into();
        let state = self.state.clone();
        let add_project = self
            .sec_button("sec-add-project", &label, IconName::HousePlus, window, cx)
            .on_click(move |_, _, cx| state.update(cx, |s, cx| s.open_project("", cx)));
        sa = sa.child(titled("sec-add-project-tip", label, add_project));
        div()
            .flex_none()
            .relative()
            .flex()
            .items_center()
            .gap(px(6.))
            .pt(px(13.))
            .px(px(GUTTER + 8.))
            .pb(px(6.))
            .bg(t.rail)
            .child(
                text("Projects")
                    .mono()
                    .size(9.)
                    .color(t.faint)
                    .tracking(0.14)
                    .uppercase()
                    .nowrap(),
            )
            .child(sa)
    }

    /// `.sec::after`: content scrolling away fades under the pinned Projects header.
    fn sec_fade_layer(&self, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let o = self.sec_fade.value(cx);
        div()
            .absolute()
            .left_0()
            .right_0()
            .top_0()
            .h(px(FADE))
            .opacity(o)
            .child(fade_gradient(t.rail))
            .into_any_element()
    }

    fn render_foot(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let (proj_label, needs_you, visible_running, pr_running) = {
            let s = self.state.read(cx);
            let n = s.projects.len();
            let proj_label = match n {
                0 => "no projects".to_string(),
                1 => "1 project".to_string(),
                n => format!("{n} projects"),
            };
            let parked = parked_needs_you_by_workspace(
                &s.workspaces,
                &s.user_questions,
                &s.permission_requests,
            );
            let mut seen = HashSet::new();
            let (mut running, mut needs) = (0usize, 0usize);
            let mut seen_needs = HashSet::new();
            for w in &s.workspaces {
                if w.archived || !w.host_pr_id.trim().is_empty() {
                    continue;
                }
                if seen.insert(w.id.as_str()) && workspace_is_running(w, &s.workspaces_running) {
                    running += 1;
                }
                if parked.get(&w.id).copied().unwrap_or(0) > 0 && seen_needs.insert(w.id.as_str()) {
                    needs += 1;
                }
            }
            (
                proj_label,
                needs,
                running,
                pr_chat_host_running_count(&s.workspaces, &s.workspaces_running),
            )
        };
        let running_label = format!(
            "{visible_running} {} running",
            if visible_running == 1 {
                "workspace"
            } else {
                "workspaces"
            }
        );
        let pr_label = format!(
            "{pr_running} {} running",
            if pr_running == 1 {
                "PR agent"
            } else {
                "PR agents"
            }
        );
        let status = if needs_you > 0 {
            format!(
                "{needs_you} {} you",
                if needs_you == 1 { "needs" } else { "need" }
            )
        } else {
            running_label
        };
        let pr_hosts = needs_you == 0 && visible_running == 0 && pr_running > 0;
        let footer_label = if pr_hosts { pr_label } else { status };

        let foot_btn = |this: &mut Self,
                        key: &'static str,
                        label: &'static str,
                        name: IconName,
                        window: &Window,
                        cx: &mut Context<Self>| {
            let b = this
                .control(key, 4., window, cx)
                .aria_label(label)
                .size(px(26.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.faint)
                .hover(move |s| s.bg(t.pane).text_color(Hsla::from(t.muted)))
                .child(icon(name).size(px(14.)));
            (b, label)
        };
        let state = self.state.clone();
        let (settings, settings_label) = foot_btn(
            self,
            "foot-settings",
            "Settings",
            IconName::Settings,
            window,
            cx,
        );
        let settings = settings
            .on_click(move |_, _, cx| state.update(cx, |s, cx| s.go(ViewName::Settings, None, cx)));
        let state = self.state.clone();
        let (help, help_label) =
            foot_btn(self, "foot-help", "Get started", IconName::Help, window, cx);
        let help = help
            .on_click(move |_, _, cx| state.update(cx, |s, cx| s.go(ViewName::Welcome, None, cx)));
        let bar = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .py(px(3.))
            .px(px(GUTTER))
            .border_t_1()
            .border_color(Hsla::from(t.line))
            .child(titled("foot-settings-tip", settings_label, settings))
            .child(titled("foot-help-tip", help_label, help))
            .child(div().flex_1())
            .child(self.usage.clone());

        let stat = |label: String, color: Color, grow: bool, last: bool| {
            div()
                .flex_none()
                .when(grow, |d| d.flex_1())
                .flex()
                .items_center()
                .min_w_0()
                .overflow_hidden()
                .px(px(12.))
                .when(!last, |d| {
                    d.border_r_1().border_color(Hsla::from(t.line_soft))
                })
                .child(
                    text(label)
                        .mono()
                        .size(10.5)
                        .line_height(LineHeight::Scale(1.))
                        .color(color)
                        .truncate(),
                )
        };
        let state = self.state.clone();
        let second: AnyElement = if needs_you > 0 {
            let aria = format!("{footer_label}. Open Inbox");
            let el = stat(footer_label, t.coral, true, true)
                .id("foot-needs")
                .debug_selector(|| "foot-needs".into())
                .role(gpui_kit::Role::Button)
                .aria_label(aria)
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.go(ViewName::Briefing, Some("needs"), cx))
                });
            titled("foot-needs-tip", "Open Inbox", el).into_any_element()
        } else if pr_hosts {
            let aria = format!("{footer_label}. Open pull requests");
            let el = stat(footer_label, t.amber, true, true)
                .id("foot-prs")
                .debug_selector(|| "foot-prs".into())
                .role(gpui_kit::Role::Button)
                .aria_label(aria)
                .cursor_pointer()
                .on_click(move |_, _, cx| state.update(cx, |s, cx| s.go(ViewName::Pr, None, cx)));
            titled("foot-prs-tip", "Open pull requests", el).into_any_element()
        } else {
            let run = visible_running > 0;
            stat(footer_label, if run { t.amber } else { t.muted }, run, true).into_any_element()
        };
        let stats = div()
            .flex_none()
            .h(px(26.))
            .bg(t.statusbar)
            .border_t_1()
            .border_color(Hsla::from(t.line))
            .flex()
            .items_stretch()
            .min_w_0()
            .child(stat(proj_label, t.muted, false, false))
            .child(second);
        div().flex_none().flex().flex_col().child(bar).child(stats)
    }

    fn render_move_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.move_menu.clone()?;
        let t = Theme::of(cx).tokens;
        let (cur, groups) = {
            let s = self.state.read(cx);
            (
                s.project(&menu.project_id).cloned(),
                unique_refs(&s.project_groups)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
        let hidden = cur.as_ref().is_some_and(|p| p.hidden);
        let cur_gid = cur.as_ref().map(|p| p.group_id.clone()).unwrap_or_default();
        let item = |id: SharedString, label: SharedString| {
            div()
                .debug_selector({
                    let id = id.clone();
                    move || id.to_string()
                })
                .id(id)
                .role(gpui_kit::Role::MenuItem)
                .w_full()
                .py(px(6.))
                .px(px(8.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(t.text)
                .hover(move |s| s.bg(t.pane_2))
                .child(style::button_label(label, 12., weight::REGULAR))
        };
        let pid = menu.project_id.clone();
        let mut panel = style::card(t.pane, t.line, 8.)
            .id("rail-move-menu")
            .debug_selector(|| "rail-move-menu".into())
            .role(gpui_kit::Role::Menu)
            .min_w(px(160.))
            .p(px(4.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .occlude()
            .on_mouse_down_out(cx.guarded_listener(|this, _, _, cx| this.close_move_menu(cx)))
            .child(
                item(
                    "rail-move-hide".into(),
                    if hidden {
                        "Unhide project"
                    } else {
                        "Hide project"
                    }
                    .into(),
                )
                .on_click(cx.guarded_listener(move |this, _, _, cx| {
                    let pid = pid.clone();
                    this.state
                        .update(cx, |s, cx| s.set_project_hidden(&pid, !hidden, cx))
                        .detach();
                    this.close_move_menu(cx);
                })),
            )
            .child(
                div().pt(px(6.)).px(px(8.)).pb(px(4.)).child(
                    text("Move to group")
                        .mono()
                        .size(9.)
                        .color(t.faint)
                        .tracking(0.12)
                        .uppercase()
                        .nowrap(),
                ),
            );
        if !cur_gid.is_empty() {
            let pid = menu.project_id.clone();
            panel = panel.child(
                item("rail-move-ungrouped".into(), "Ungrouped".into()).on_click(
                    cx.guarded_listener(move |this, _, _, cx| {
                        this.move_project_to_group(&pid, "", cx)
                    }),
                ),
            );
        }
        let targets: Vec<ProjectGroup> = groups.into_iter().filter(|g| g.id != cur_gid).collect();
        if targets.is_empty() && cur_gid.is_empty() {
            panel = panel.child(
                div()
                    .pt(px(6.))
                    .px(px(8.))
                    .pb(px(8.))
                    .child(text("No groups yet").size(11.5).color(t.faint)),
            );
        }
        for g in targets {
            let pid = menu.project_id.clone();
            let gid = g.id.clone();
            panel = panel.child(
                item(
                    SharedString::from(format!("rail-move-{}", g.id)),
                    g.name.clone().into(),
                )
                .on_click(cx.guarded_listener(move |this, _, _, cx| {
                    this.move_project_to_group(&pid, &gid, cx)
                })),
            );
        }
        let entity = cx.entity();
        let panel = panel
            .on_key_down(cx.guarded_listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close_move_menu(cx);
                }
            }))
            .child(
                canvas(
                    move |b, _, cx| entity.update(cx, |this, _| this.menu_bounds = Some(b)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        let at = menu.at;
        Some(
            deferred(style::anchored_at(
                f32::from(at.x),
                f32::from(at.y),
                with_shadow(
                    panel,
                    &[css_box_shadow(0., 10., 28., 0., rgba(0, 0, 0, 0.35))],
                    8.,
                ),
            ))
            // `.rail-move-menu`'s `z-index: 400`: over the rail's resize handle.
            .with_priority(400)
            .into_any_element(),
        )
    }

    fn render_rail(
        &mut self,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = Theme::of(cx).tokens;
        let top = self.render_top(window, cx);
        let sec = self.render_sec(window, cx);
        let tree = self.render_tree(window, cx);
        let sec_fade = self.sec_fade_layer(cx);
        let foot = self.render_foot(window, cx);
        let state = self.state.clone();
        let handle = resize_handle(
            "rail-resize",
            Axis::X,
            Edge::End,
            width,
            &self.fh("rail-resize", cx),
        )
        .on_live({
            let state = state.clone();
            move |w, _, cx| state.update(cx, |s, cx| s.set_panel_live(PanelKey::Rail, w, cx))
        })
        .on_commit({
            let state = state.clone();
            move |_, cx| state.update(cx, |s, _| s.persist_panels())
        })
        .on_step(move |w, _, cx| state.update(cx, |s, cx| s.set_panel(PanelKey::Rail, w, cx)));
        div()
            .id("rail")
            .debug_selector(|| "rail".into())
            .track_focus(&self.rail_focus)
            .role(gpui_kit::Role::Complementary)
            .aria_label("Sidebar")
            .relative()
            .flex_none()
            .w(px(width))
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(t.rail)
            .border_r_1()
            .border_color(Hsla::from(t.line))
            .child(top)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(sec)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(tree)
                            .child(sec_fade),
                    ),
            )
            .child(foot)
            .child(handle)
            .children(self.render_move_menu(cx))
    }
}

/// `linear-gradient(to bottom, color, transparent)` filling its parent.
pub(crate) fn fade_gradient(color: Color) -> impl IntoElement {
    style::css_gradient(
        180.,
        (color, 0.),
        (color.with_alpha(0.), 1.),
        0.,
        0.,
        rgba(0, 0, 0, 0.),
    )
}

fn set_group_open(s: &mut AppState, id: &str, open: bool, cx: &mut Context<AppState>) {
    let list = s
        .project_groups
        .iter()
        .map(|g| {
            let mut g = g.clone();
            if g.id == id {
                g.open = open;
            }
            g
        })
        .collect();
    s.set_project_groups(list, cx);
}

impl Sidebar {
    /// The docked slot's share of the rail's width, and whether it is still easing.
    pub fn dock(&self, cx: &App) -> (f32, bool) {
        (self.dock.value(cx), self.dock.animating(cx))
    }

    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        self.dots_prev = std::mem::take(&mut self.dots);
        let (collapsed, peek, width) = {
            let s = self.state.read(cx);
            (s.rail_collapsed, s.rail_peek, s.panels.size(PanelKey::Rail))
        };
        let entity = cx.entity();
        let rail_bounds = canvas(
            {
                let entity = entity.clone();
                move |b, _, cx| entity.update(cx, |this, _| this.rail_bounds = Some(b))
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        if !collapsed {
            if self.dock.animating(cx) {
                window.request_animation_frame();
            }
            // Pinned to the slot's right edge, so the rail slides in as the slot widens.
            return div()
                .size_full()
                .relative()
                .child(
                    self.render_rail(width, window, cx)
                        .absolute()
                        .top_0()
                        .right_0()
                        .child(rail_bounds),
                )
                .into_any_element();
        }

        // Collapsed: the rail overlays the content as a flyout off the left edge.
        self.edge_fade.set(self.edge_hover && !peek, FADE_IN, cx);
        let slide = self.slide.value(cx);
        if self.slide.animating(cx) || self.edge_fade.animating(cx) {
            window.request_animation_frame();
        }
        let edge = self.edge_fade.value(cx) * 0.55;
        let hotzone = div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(RAIL_PEEK_EDGE_PX))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(gpui_kit::relative(0.12))
                    .bottom(gpui_kit::relative(0.12))
                    .w(px(2.))
                    .rounded(px(2.))
                    .bg(t.amber)
                    .opacity(edge),
            );
        let listeners = canvas(
            {
                let entity = entity.clone();
                move |b, _, cx| entity.update(cx, |this, _| this.body_bounds = Some(b))
            },
            move |_, _, window, _| {
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        e.update(cx, |this, cx| this.on_peek_move(ev.position, cx));
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        e.update(cx, |this, cx| this.on_peek_down(ev.position, cx));
                    }
                });
                let e = entity.clone();
                window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        e.update(cx, |this, cx| this.on_peek_out(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let shown = peek || slide > 0.;
        let rail = shown.then(|| {
            // No shadow while the content still follows the rail out.
            let shadow = rgba(0, 0, 0, 0.38 * (slide - self.dock.value(cx)).max(0.));
            self.render_rail(width, window, cx)
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(-width * (1. - slide)))
                .on_hover(cx.guarded_listener(|this, h: &bool, _, cx| {
                    if !*h {
                        this.schedule_hide_peek(cx);
                    }
                }))
                .child(box_shadow(
                    &[css_box_shadow(10., 0., 28., 0., shadow)],
                    0.,
                    1.,
                ))
                .child(rail_bounds)
        });
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(0.))
            .child(listeners)
            .child(hotzone)
            .children(rail)
            .into_any_element()
    }
}

/// The running dot's body: a 6px amber dot pulsing over 2.4s, asking for the next frame
/// while it runs.
fn running_dot(epoch: Instant, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let opacity = if motion::reduced(cx) {
        1.
    } else {
        if !motion::is_frozen(cx) {
            window.request_animation_frame();
        }
        let p = (motion::elapsed(epoch, cx).as_secs_f32() % 2.4) / 2.4;
        if p < 0.5 {
            0.5 + 0.5 * Easing::EASE_IN_OUT.apply(p * 2.)
        } else {
            1. - 0.5 * Easing::EASE_IN_OUT.apply((p - 0.5) * 2.)
        }
    };
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .opacity(opacity)
        .child(with_shadow(
            div().size(px(6.)).rounded_full().bg(t.amber),
            &[css_box_shadow(0., 0., 5., 0., t.amber)],
            3.,
        ))
        .into_any_element()
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
