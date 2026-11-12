//! The pull request dashboard. Rows grouped into needs-you, workspace
//! responding, open and merged buckets under sticky headers, with the lifecycle segments,
//! status facets, project filter and sort in the pane header, and a GitHub sync when the
//! route opens.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, AsyncApp, Bounds, ClickEvent, Context, Div, Entity,
    FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, Modifiers, ParentElement, Pixels,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Task,
    Window, canvas, div, px,
};
use recur_domain::{Pr, PrSource};
use recur_engine::prs::is_forge_source;
use recur_engine::view::pr_merge_action::{
    MergeActionKind, can_approve_pr_review, can_set_pr_draft, pr_merge_action, show_pr_update,
};
use recur_engine::view::pr_workspace::{pr_agent_is_running, pr_agent_workspace_id};
use recur_forge::view::{RollupKind, pr_attention_labels, pr_checks_rollup, pr_has_check_status};
use recur_services::Services;
use recur_ui::fonts::{Family, weight};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::{Rbtn, Tbtn, TbtnSize, button_label, rbtn, tbtn};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Select, SelectEvent, SelectOption, ToastKind, TriggerStyle};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::pr::{PrLifecycle, pr_icon_name, pr_lifecycle};
use recur_view_core::{AppState, ViewContext, ViewName, call, toast};

use crate::actions::{apply_merged, github_href};
use crate::chrome::{chrome_color, group_color, group_label, status_label};
use crate::style::{dot, hsla, running_dot, seg, seg_text_btn, spin_deg};
use crate::ui::{PrUi, ui};
use recur_view_core::pr_list_filter::{
    GROUP_ORDER, LIST_FACETS, LIST_SEGS, LIST_SORTS, ListSort, list_group, matches_list_filter,
    scoped_prs, sort_list,
};
use recur_view_widgets::people::{people_stack, pr_people};

/// `@container prlist (min-width: 1200px)`: people and why move onto the row.
const WIDE_LIST: f32 = 1200.;
/// `@media (max-width: 1240px)`: the sort trigger drops its label.
const SORT_LABEL_MIN: f32 = 1240.;
/// `11rem`.
const FILTER_MAX: f32 = 176.;
/// `.briefing-h`: 8px padding, a 9px line, 8px padding and the 1px rule.
const HEAD_H: f32 = 26.;

/// The row's grid tracks: `26px minmax(0, 1fr) 9.75rem 5.75rem 4.5rem 1.15rem 2.75rem`.
const STATUS_W: f32 = 156.;
const DIFF_W: f32 = 92.;
const FILES_W: f32 = 72.;
const CI_W: f32 = 18.4;
const WHEN_W: f32 = 44.;
/// The wide grid's extra tracks: `minmax(12rem, 1fr) 5.75rem 10.5rem`.
const MAIN_MIN_WIDE: f32 = 192.;
const PEOPLE_W: f32 = 92.;
const WHY_W: f32 = 168.;
const COL_GAP: f32 = 16.;
/// How long an armed bulk merge or close waits for its confirming click.
const ARM: Duration = Duration::from_secs(3);

/// An action on the selected rows; each runs only on the PRs it applies to.
#[derive(Clone, Copy, PartialEq)]
enum Bulk {
    Open,
    Update,
    Approve,
    Draft,
    Ready,
    Close,
    Merge,
}

impl Bulk {
    const ALL: [Bulk; 7] = [
        Bulk::Open,
        Bulk::Update,
        Bulk::Approve,
        Bulk::Draft,
        Bulk::Ready,
        Bulk::Close,
        Bulk::Merge,
    ];

    fn label(self) -> &'static str {
        match self {
            Bulk::Open => "Open in GitHub",
            Bulk::Update => "Update with base",
            Bulk::Approve => "Approve",
            Bulk::Draft => "Convert to draft",
            Bulk::Ready => "Mark ready",
            Bulk::Close => "Close",
            Bulk::Merge => "Merge",
        }
    }

    fn done(self) -> &'static str {
        match self {
            Bulk::Open => "opened",
            Bulk::Update => "updated with base",
            Bulk::Approve => "approved",
            Bulk::Draft => "converted to draft",
            Bulk::Ready => "marked ready",
            Bulk::Close => "closed",
            Bulk::Merge => "merged",
        }
    }

    fn applies(self, p: &Pr) -> bool {
        let life = pr_lifecycle(Some(p));
        match self {
            Bulk::Open => !github_href(p).is_empty(),
            Bulk::Update => {
                let a = pr_merge_action(Some(p), false);
                a.kind != MergeActionKind::Resolve && show_pr_update(Some(p), &a)
            }
            Bulk::Approve => {
                !p.draft && p.pending_comments.is_empty() && can_approve_pr_review(Some(p))
            }
            Bulk::Draft => life == Some(PrLifecycle::Open) && can_set_pr_draft(Some(p)),
            Bulk::Ready => life == Some(PrLifecycle::Draft) && can_set_pr_draft(Some(p)),
            Bulk::Close => {
                matches!(life, Some(PrLifecycle::Open | PrLifecycle::Draft))
                    && (!is_forge_source(&p.source) || can_set_pr_draft(Some(p)))
            }
            Bulk::Merge => pr_merge_action(Some(p), false).kind == MergeActionKind::Merge,
        }
    }

    /// Runs on one PR and folds the result into the store; false when it failed (and toasted).
    async fn run(
        self,
        p: &Pr,
        services: &Services,
        state: &Entity<AppState>,
        cx: &mut AsyncApp,
    ) -> bool {
        let (id, prs) = (p.id.as_str(), &services.prs);
        let fail = format!("{} failed", self.label());
        let updated = match self {
            Bulk::Open => {
                let href = github_href(p);
                cx.update(|cx| recur_content::links::open_external_url(&href, cx));
                return true;
            }
            Bulk::Update => {
                let Some(r) = call(cx, prs.update_with_base(id, ""), &fail).await else {
                    return false;
                };
                if r.status == "conflict" {
                    let msg = format!("#{} hit conflicts. Open it to resolve them.", p.number);
                    cx.update(|cx| toast(ToastKind::Error, msg, cx));
                }
                return r.status == "updated";
            }
            Bulk::Merge => {
                let Some(m) = call(cx, prs.merge_pr(id, ""), &fail).await else {
                    return false;
                };
                apply_merged(state, m, cx);
                return true;
            }
            Bulk::Approve => call(cx, prs.submit_review(id, "approve", ""), &fail).await,
            Bulk::Close => call(cx, prs.close_pr(id), &fail).await,
            Bulk::Draft | Bulk::Ready => {
                let draft = self == Bulk::Draft;
                call(cx, prs.set_pr_draft(id, draft), &fail)
                    .await
                    .map(|u| Pr { draft, ..u })
            }
        };
        let Some(u) = updated else { return false };
        state.update(cx, |s, cx| s.update_pr(id, |p| *p = u, cx));
        true
    }
}

/// Last frame's geometry, in window coordinates.
#[derive(Default)]
struct Probes {
    content: Option<Bounds<Pixels>>,
    buckets: HashMap<String, Bounds<Pixels>>,
    people: HashMap<String, Bounds<Pixels>>,
}

pub struct PrList {
    vcx: ViewContext,
    ui: Entity<PrUi>,
    project_select: Entity<Select>,
    status_select: Entity<Select>,
    sort_select: Entity<Select>,
    scroll: ScrollHandle,
    probes: Rc<RefCell<Probes>>,
    list_width: f32,
    hovered_row: Option<String>,
    hovered_people: Option<String>,
    row_focus: HashMap<String, FocusHandle>,
    selected: BTreeSet<String>,
    anchor: Option<String>,
    armed: Option<(Bulk, Task<()>)>,
    bulk_busy: bool,
    on_route: bool,
    sync: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

fn filter_style() -> TriggerStyle {
    TriggerStyle {
        max_width: Some(FILTER_MAX),
        value_shrink: true,
        ..Default::default()
    }
}

impl PrList {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ui = ui(cx);
        let facets: Vec<SelectOption> = LIST_FACETS
            .iter()
            .map(|(v, l, g)| SelectOption::new(*v, *l).group(*g))
            .collect();
        let sorts: Vec<SelectOption> = LIST_SORTS
            .iter()
            .map(|(s, l, _)| SelectOption::new(s.id(), *l))
            .collect();
        let (seg_values, sort) = {
            let u = ui.read(cx);
            (
                u.facets
                    .iter()
                    .map(|s| SharedString::from(s.clone()))
                    .collect(),
                u.sort,
            )
        };
        let project_ids: Vec<SharedString> = ui
            .read(cx)
            .project_ids
            .iter()
            .map(|s| SharedString::from(s.clone()))
            .collect();
        let project_select = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Filter by project")
                .placeholder("All projects")
                .multiple()
                .ghost()
                .trigger_style(filter_style())
                .with_values(project_ids)
        });
        let status_select = cx.new(|cx| {
            Select::new(facets, window, cx)
                .label("Filter by status")
                .placeholder("Any status")
                .multiple()
                .ghost()
                .trigger_style(filter_style())
                .with_values(seg_values)
        });
        let sort_select = cx.new(|cx| {
            Select::new(sorts, window, cx)
                .label("Sort pull requests")
                .ghost()
                .trigger_icon(sort_icon(sort))
                .with_value(sort.id())
        });
        let subs = vec![
            cx.guarded_subscribe(
                &project_select,
                |this: &mut Self, _, ev: &SelectEvent, cx| {
                    if let SelectEvent::Values(v) = ev {
                        let ids: Vec<String> = v.iter().map(|s| s.to_string()).collect();
                        this.ui.update(cx, |u, cx| {
                            u.project_ids = ids;
                            cx.notify();
                        });
                    }
                },
            ),
            cx.guarded_subscribe(
                &status_select,
                |this: &mut Self, _, ev: &SelectEvent, cx| {
                    if let SelectEvent::Values(v) = ev {
                        let f: Vec<String> = v.iter().map(|s| s.to_string()).collect();
                        this.ui.update(cx, |u, cx| {
                            u.facets = f;
                            cx.notify();
                        });
                    }
                },
            ),
            cx.guarded_subscribe(
                &sort_select,
                |this: &mut Self, sel, ev: &SelectEvent, cx| {
                    if let SelectEvent::Change(v) = ev
                        && let Some(s) = ListSort::from_id(v)
                    {
                        this.ui.update(cx, |u, cx| {
                            u.sort = s;
                            cx.notify();
                        });
                        sel.update(cx, |sel, cx| {
                            sel.set_value(s.id(), cx);
                            sel.set_icon(Some(sort_icon(s)), cx);
                        });
                    }
                },
            ),
            cx.guarded_observe(&ui, |_, _, cx| cx.notify()),
            cx.observe_state_in(&vcx.state, Topics::ALL, window, |this, _, window, cx| {
                this.sync_route(window, cx);
                cx.notify();
            }),
        ];
        let mut me = PrList {
            vcx: vcx.clone(),
            ui,
            project_select,
            status_select,
            sort_select,
            scroll: ScrollHandle::new(),
            probes: Rc::default(),
            list_width: 0.,
            hovered_row: None,
            hovered_people: None,
            row_focus: HashMap::new(),
            selected: BTreeSet::new(),
            anchor: None,
            armed: None,
            bulk_busy: false,
            on_route: false,
            sync: None,
            _subs: subs,
        };
        me.sync_route(window, cx);
        me
    }

    fn state(&self) -> &Entity<AppState> {
        &self.vcx.state
    }

    /// On the PR route: sync GitHub PRs when due, `prListSyncing` while it
    /// runs; leaving the route drops the flag (and ignores a late result).
    fn sync_route(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let on = self.state().read(cx).view == ViewName::Pr;
        if on == self.on_route {
            return;
        }
        self.on_route = on;
        if !on {
            self.sync = None;
            self.set_syncing(false, cx);
            return;
        }
        if !self.state().read(cx).github_pr_sync_due() {
            return;
        }
        self.set_syncing(true, cx);
        let services = self.vcx.services.clone();
        let state = self.state().clone();
        self.sync = Some(cx.spawn(async move |this, cx| {
            let res = services.prs.sync_github_prs().await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(list) = res {
                    state.update(cx, |s, cx| s.set_prs_from_sync(list, cx));
                }
                this.set_syncing(false, cx);
            });
        }));
    }

    fn set_syncing(&mut self, on: bool, cx: &mut Context<Self>) {
        self.ui.update(cx, |u, cx| {
            if u.syncing != on {
                u.syncing = on;
                cx.notify();
            }
        });
    }

    fn open(&self, id: &str, cx: &mut App) {
        let id = id.to_string();
        self.state().update(cx, |s, cx| s.open_pr(&id, cx));
    }

    /// ⌘-click toggles a row, shift-click selects from the anchor to it in drawn order, and a
    /// plain click opens it.
    fn click_row(&mut self, id: &str, m: Modifiers, cx: &mut Context<Self>) {
        if !m.shift && !m.secondary() {
            self.open(id, cx);
            return;
        }
        if self.bulk_busy {
            return;
        }
        let order: Vec<String> = self
            .grouped(cx)
            .into_iter()
            .flat_map(|(_, v)| v.into_iter().map(|p| p.id))
            .collect();
        let at = |x: &str| order.iter().position(|o| o == x);
        let range = self
            .anchor
            .as_deref()
            .filter(|_| m.shift)
            .and_then(at)
            .zip(at(id));
        if let Some((a, b)) = range {
            self.selected
                .extend(order[a.min(b)..=a.max(b)].iter().cloned());
        } else {
            if !self.selected.remove(id) {
                self.selected.insert(id.to_string());
            }
            self.anchor = Some(id.to_string());
        }
        self.armed = None;
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        self.anchor = None;
        self.armed = None;
        cx.notify();
    }

    /// The first Merge or Close click arms it for `ARM`, the next runs it.
    fn run_bulk(&mut self, act: Bulk, cx: &mut Context<Self>) {
        if self.bulk_busy {
            return;
        }
        let prs: Vec<Pr> = self
            .state()
            .read(cx)
            .prs
            .iter()
            .filter(|p| self.selected.contains(&p.id) && act.applies(p))
            .cloned()
            .collect();
        if prs.is_empty() {
            return;
        }
        if matches!(act, Bulk::Merge | Bulk::Close) && self.armed.as_ref().map(|a| a.0) != Some(act)
        {
            self.armed = Some((
                act,
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(ARM).await;
                    let _ = this.update(cx, |this, cx| {
                        this.armed = None;
                        cx.notify();
                    });
                }),
            ));
            cx.notify();
            return;
        }
        self.armed = None;
        self.bulk_busy = true;
        cx.notify();
        let services = self.vcx.services.clone();
        let state = self.state().clone();
        cx.spawn(async move |this, cx| {
            let mut n = 0;
            for p in &prs {
                n += usize::from(act.run(p, &services, &state, cx).await);
            }
            if n > 0 {
                let done = if act == Bulk::Merge && prs.iter().all(|p| p.merge_queue_enabled) {
                    "added to the merge queue"
                } else {
                    act.done()
                };
                let msg = format!("{n} PR{} {done}", if n == 1 { "" } else { "s" });
                cx.update(|cx| toast(ToastKind::Success, msg, cx));
            }
            let _ = this.update(cx, |this, cx| {
                this.bulk_busy = false;
                this.clear_selection(cx);
            });
        })
        .detach();
    }

    /// `prListScoped`.
    fn scoped(&self, cx: &App) -> Vec<Pr> {
        let s = self.state().read(cx);
        let ids = &self.ui.read(cx).project_ids;
        scoped_prs(&s.prs, ids, |ws| {
            s.workspaces
                .iter()
                .find(|w| w.id == ws && !w.project_id.is_empty())
                .map(|w| w.project_id.as_str())
        })
        .into_iter()
        .cloned()
        .collect()
    }

    /// `grouped`: buckets in `PR_LIST_GROUP_ORDER` then any others, each sorted.
    fn grouped(&self, cx: &App) -> Vec<(String, Vec<Pr>)> {
        let u = self.ui.read(cx);
        let mut by: Vec<(String, Vec<Pr>)> = Vec::new();
        for p in self.scoped(cx) {
            if !matches_list_filter(&p, u.seg, &u.facets) {
                continue;
            }
            let g = list_group(&p);
            match by.iter_mut().find(|(k, _)| *k == g) {
                Some((_, v)) => v.push(p),
                None => by.push((g, vec![p])),
            }
        }
        let mut order: Vec<String> = GROUP_ORDER.iter().map(|s| s.to_string()).collect();
        order.extend(
            by.iter()
                .map(|(g, _)| g.clone())
                .filter(|g| !GROUP_ORDER.contains(&g.as_str())),
        );
        order
            .into_iter()
            .filter_map(|g| {
                let items = by.iter().find(|(k, _)| *k == g).map(|(_, v)| v.clone())?;
                (!items.is_empty()).then(|| (g, sort_list(&items, u.sort)))
            })
            .collect()
    }

    fn project_name(&self, id: &str, cx: &App) -> String {
        let pid = id.trim();
        if pid.is_empty() {
            return String::new();
        }
        self.state()
            .read(cx)
            .project(pid)
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| pid.to_string())
    }

    /// `rowSub`: the workspace (or project), then the branch.
    fn row_sub(&self, p: &Pr, cx: &App) -> String {
        let loc = if p.workspace_id.is_empty() {
            self.project_name(&p.project_id, cx)
        } else {
            p.workspace_id.clone()
        };
        [loc, p.branch.clone()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    fn unseen(&self, p: &Pr, cx: &App) -> i64 {
        let s = self.state().read(cx);
        let id = pr_agent_workspace_id(Some(p), &s.workspaces);
        if id.is_empty() {
            return 0;
        }
        let archived_host = s
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .is_some_and(|w| w.archived && !w.host_pr_id.trim().is_empty());
        if archived_host {
            return 0;
        }
        s.attention.workspace(&id)
    }

    fn focus_for(&mut self, id: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.row_focus
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    fn filters(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let (syncing, cur_seg) = {
            let u = self.ui.read(cx);
            (u.syncing, u.seg)
        };
        let projects: Vec<SelectOption> = self
            .state()
            .read(cx)
            .projects
            .iter()
            .filter(|p| p.is_git && p.id != "files")
            .map(|p| {
                let label = if p.name.is_empty() { &p.id } else { &p.name };
                SelectOption::new(p.id.clone(), label.clone()).pic(p.pic.clone(), p.color.clone())
            })
            .collect();
        let show_projects = projects.len() > 1;
        if show_projects {
            self.project_select
                .update(cx, |s, cx| s.set_options(projects, cx));
        }
        let narrow = f32::from(window.viewport_size().width) <= SORT_LABEL_MIN;
        self.sort_select.update(cx, |s, cx| {
            let mut tr = s.trigger().clone();
            if tr.hide_value != narrow {
                tr.hide_value = narrow;
                s.set_trigger_style(tr, cx);
            }
        });
        let mut bar = div()
            .debug_selector(|| "briefing-filters".into())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(recur_ui::style::CHROME_H))
            .px(px(12.))
            .bg(hsla(t.pane))
            .border_b_1()
            .border_color(hsla(t.line));
        if !self.selected.is_empty() {
            return self.bulk_bar(bar, cx);
        }
        if syncing {
            let deg = spin_deg(window, cx);
            bar = bar.child(
                div()
                    .debug_selector(|| "tb-refresh".into())
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .text_color(hsla(t.amber))
                    .child(icon(IconName::Loader).size(px(12.)).rotate(deg))
                    .child(text("Refreshing").mono().size(11.).nowrap()),
            );
        }
        bar = bar.child(div().flex_1());
        if show_projects {
            bar = bar.child(self.project_select.clone());
        }
        bar = bar
            .child(self.status_select.clone())
            .child(self.sort_select.clone());
        let mut segs = seg(cx).flex_none().h(px(24.));
        for (life, label) in LIST_SEGS {
            let on = cur_seg == life;
            segs = segs.child(
                seg_text_btn(SharedString::from(format!("seg-{label}")), label, on, cx)
                    .debug_selector(move || format!("seg:{label}:{on}"))
                    .on_click(cx.guarded_listener(move |this, _, _, cx| {
                        this.ui.update(cx, |u, cx| {
                            u.seg = life;
                            cx.notify();
                        });
                    })),
            );
        }
        bar.child(segs)
    }

    /// The filters bar's contents while rows are selected: the count, Clear, and the actions.
    fn bulk_bar(&self, bar: Div, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let (n, busy) = (self.selected.len(), self.bulk_busy);
        let counts: Vec<(Bulk, usize)> = {
            let s = self.state().read(cx);
            let picked: Vec<&Pr> = s
                .prs
                .iter()
                .filter(|p| self.selected.contains(&p.id))
                .collect();
            Bulk::ALL
                .iter()
                .map(|&a| (a, picked.iter().filter(|p| a.applies(p)).count()))
                .collect()
        };
        let clear = div()
            .id("pr-bulk-clear")
            .debug_selector(|| "pr-bulk-clear".into())
            .text_color(hsla(t.muted))
            .when(!busy, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.text_color(hsla(t.text)))
                    .on_click(cx.guarded_listener(|this, _, _, cx| this.clear_selection(cx)))
            })
            .child(button_label("Clear", 11.5, weight::MEDIUM));
        let mut acts = div()
            .id("pr-bulk-acts")
            .flex()
            .items_center()
            .gap(px(6.))
            .on_hover(cx.guarded_listener(|this, on: &bool, _, cx| {
                if !*on && this.armed.take().is_some() {
                    cx.notify();
                }
            }));
        for (a, k) in counts {
            let off = busy || k == 0;
            let armed = self.armed.as_ref().is_some_and(|x| x.0 == a);
            let label = if armed {
                format!("Confirm {} {k}", a.label().to_lowercase())
            } else {
                a.label().to_string()
            };
            let sel = format!("pr-bulk-act:{label}{}", if off { ":off" } else { "" });
            let bid = SharedString::from(format!("pr-bulk-{}", a.label()));
            let over = off.then_some(t.pane);
            let mut b =
                match a {
                    Bulk::Merge => rbtn(bid, Rbtn::Merge, over, cx)
                        .h(px(26.))
                        .px(px(9.))
                        .rounded(px(7.))
                        .child(icon(IconName::GitPullRequest).size(px(13.)))
                        .child(button_label(label, 11.5, weight::MEDIUM_PLUS)),
                    Bulk::Close => {
                        let kind = if armed { Tbtn::Armed } else { Tbtn::Danger };
                        tbtn(bid, kind, TbtnSize::TITLEBAR, over, cx)
                            .child(icon(IconName::GitPullRequestClosed).size(px(13.)))
                            .child(button_label(label, 11.5, weight::MEDIUM))
                    }
                    _ => tbtn(bid, Tbtn::Default, TbtnSize::TITLEBAR, over, cx)
                        .child(button_label(label, 11.5, weight::MEDIUM)),
                }
                .debug_selector(move || sel);
            if !off {
                b = b.on_click(cx.guarded_listener(move |this, _, _, cx| this.run_bulk(a, cx)));
            }
            acts = acts.child(b);
        }
        bar.child(
            div()
                .debug_selector(move || format!("pr-bulk:{n}"))
                .child(text(format!("{n} selected")).size(12.).nowrap()),
        )
        .child(clear)
        .child(div().flex_1())
        .child(acts)
    }

    /// One `.prrow`.
    fn row(
        &mut self,
        p: &Pr,
        group: &str,
        last: bool,
        wide: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tint = chrome_color(Some(p), &t);
        let why = pr_attention_labels(p).join(" · ");
        let show_checks = pr_has_check_status(p);
        let checks = pr_checks_rollup(p);
        let running = {
            let s = self.state().read(cx);
            pr_agent_is_running(Some(p), &s.workspaces, &s.workspaces_running)
        };
        let unseen = self.unseen(p, cx);
        let id = p.id.clone();
        let hovered = self.hovered_row.as_deref() == Some(id.as_str());
        let picked = self.selected.contains(&id);
        let focus = self.focus_for(&id, cx);
        let ring = if hovered { t.pane_2 } else { t.pane };

        // `.pr-ico`
        let ico = div()
            .flex_none()
            .size(px(22.))
            .rounded(px(6.))
            .bg(hsla(tint.fade(0.12)))
            .flex()
            .items_center()
            .justify_center()
            .child(if running {
                titled(
                    SharedString::from(format!("prrow-sd-{id}")),
                    "Agent running",
                    running_dot(window, cx)
                        .id(SharedString::from(format!("prrow-sd-el-{id}")))
                        .debug_selector({
                            let id = id.clone();
                            move || format!("prrow-sd:{id}")
                        }),
                )
                .into_any_element()
            } else {
                // `<span style="color:…"><Icon /></span>`: the svg sits on the span's baseline.
                crate::style::baseline_icon(
                    icon(pr_icon_name(Some(p))).size(px(13.)).color(hsla(tint)),
                    13.,
                    Family::Ui,
                    13.,
                    LineHeight::Scale(1.45),
                    window,
                )
                .into_any_element()
            });
        let ico_col = div()
            .flex_none()
            .w(px(26.))
            .flex()
            .items_center()
            .child(ico);

        // `.pr-main`
        let gh = p.source == PrSource::GitHub;
        let num = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .text_color(hsla(t.faint))
            .child(titled(
                SharedString::from(format!("pr-src-{id}")),
                if gh { "GitHub PR" } else { "Local PR" },
                div()
                    .id(SharedString::from(format!("pr-src-el-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        let title = if gh { "GitHub PR" } else { "Local PR" };
                        move || format!("pr-src:{id}={title}")
                    })
                    .flex()
                    .items_center()
                    .child(
                        icon(if gh {
                            IconName::Github
                        } else {
                            IconName::HardDrive
                        })
                        .size(px(14.)),
                    ),
            ))
            .child(
                text(format!("#{}", p.number))
                    .mono()
                    .size(12.)
                    .line_height(LineHeight::Scale(1.))
                    .nowrap(),
            );
        let mut title_row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .text_size(px(13.))
            .child(num)
            .child(
                div().min_w_0().flex_initial().child(
                    text(p.title.clone())
                        .size(13.)
                        .truncate()
                        .id(SharedString::from(format!("pr-t-text-{id}"))),
                ),
            );
        if unseen > 0 {
            let label = if unseen > 9 {
                "9+".to_string()
            } else {
                unseen.to_string()
            };
            let tip = format!(
                "{unseen} unseen agent response{}",
                if unseen == 1 { "" } else { "s" }
            );
            title_row = title_row.child(titled(
                SharedString::from(format!("prrow-ct-{id}")),
                tip,
                count_chip(&id, label, cx),
            ));
        }
        if gh && !p.url.is_empty() {
            let url = p.url.clone();
            title_row = title_row.child(titled(
                SharedString::from(format!("pr-out-{id}")),
                "Open in GitHub",
                div()
                    .id(SharedString::from(format!("pr-out-el-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("pr-out:{id}")
                    })
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_color(hsla(t.faint))
                    .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.text.fade(0.08))))
                    .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation()
                    })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        recur_content::links::open_external_url(&url, cx);
                    })
                    .child(icon(IconName::QueuePopOut).size(px(13.))),
            ));
        }
        let sub = div().mt(px(3.)).flex().items_center().gap(px(8.)).child(
            text(self.row_sub(p, cx))
                .mono()
                .size(10.)
                .color(t.faint)
                .nowrap(),
        );
        let main = div().min_w_0().child(title_row).child(sub);

        // `.pr-people` and `.pr-why`
        let people = pr_people(p);
        let pkey = format!("row-{id}");
        let me = cx.entity().downgrade();
        let people_el = {
            let pkey2 = pkey.clone();
            let probes = self.probes.clone();
            let pk = pkey.clone();
            div()
                .relative()
                .flex_none()
                .child(people_stack(
                    &pkey,
                    &people,
                    ring,
                    move |on, _, cx| {
                        let key = pkey2.clone();
                        let _ = me.update(cx, |this, cx| {
                            let next = on.then_some(key.clone());
                            if *on || this.hovered_people.as_deref() == Some(key.as_str()) {
                                this.hovered_people = next;
                                cx.notify();
                            }
                        });
                    },
                    cx,
                ))
                .child(
                    canvas(
                        move |b, _, _| {
                            probes.borrow_mut().people.insert(pk.clone(), b);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
        };
        let mut people_slot = Some(people_el);
        let why_el = |w: Option<f32>| {
            let d = div().min_w_0();
            let d = match w {
                Some(w) => d.flex_none().w(px(w)),
                None => d.flex_1(),
            };
            d.child(
                text(why.clone())
                    .mono()
                    .size(10.5)
                    .color(t.muted)
                    .truncate()
                    .id(SharedString::from(format!("pr-why-{id}"))),
            )
            .debug_selector({
                let id = id.clone();
                let why = why.clone();
                move || format!("pr-why:{id}={why}")
            })
        };

        // `.pr-checks`
        let status = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .min_w_0()
            .child(with_shadow(
                dot(7., tint),
                &[css_box_shadow(0., 0., 5., 0., tint)],
                3.5,
            ))
            .child(text(status_label(p)).mono().size(11.).color(tint).nowrap())
            .debug_selector({
                let id = id.clone();
                let label = status_label(p);
                move || format!("pr-checks:{id}={label}")
            });
        // `.pr-diff`: `grid-template-columns: 7ch 6ch`, right-aligned.
        let ch = mono_ch(10.5);
        let diff = div()
            .flex()
            .child(
                div().flex_none().flex().justify_end().w(px(7. * ch)).child(
                    text(format!("+{}", p.diff.add))
                        .mono()
                        .size(10.5)
                        .tabular()
                        .color(t.emerald)
                        .nowrap(),
                ),
            )
            .child(exact_w(8.))
            .child(
                div().flex_none().flex().justify_end().w(px(6. * ch)).child(
                    text(format!("−{}", p.diff.del))
                        .mono()
                        .size(10.5)
                        .tabular()
                        .color(t.coral)
                        .nowrap(),
                ),
            );
        let files = div().flex().justify_end().child(
            text(format!(
                "{} {}",
                p.file_count,
                if p.file_count == 1 { "file" } else { "files" }
            ))
            .mono()
            .size(10.5)
            .tabular()
            .color(t.faint)
            .nowrap(),
        );
        let ci = (show_checks && checks != RollupKind::None).then(|| {
            let (name, color, title, cls) = match checks {
                RollupKind::Fail => (IconName::X, t.coral, "Checks failing", "fail"),
                RollupKind::Pending => (IconName::Loader, t.amber, "Checks in progress", "pending"),
                _ => (IconName::Check, t.emerald, "Checks passing", "ok"),
            };
            let deg = if checks == RollupKind::Pending {
                spin_deg(window, cx)
            } else {
                0.
            };
            titled(
                SharedString::from(format!("pr-ci-{id}")),
                title,
                div()
                    .id(SharedString::from(format!("pr-ci-el-{id}")))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("pr-ci-mark:{id}={cls}")
                    })
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icon(name)
                            .size(px(13.))
                            .stroke_width(2.4)
                            .color(hsla(color))
                            .rotate(deg),
                    ),
            )
        });
        let when = div().flex().justify_end().child(
            text(p.when.clone())
                .mono()
                .size(10.)
                .tabular()
                .color(t.faint)
                .nowrap(),
        );

        let cell = |w: f32, el: Div| div().flex_none().w(px(w)).child(el);
        let ci_cell = exact_cell(CI_W, ci.map(IntoElement::into_any_element));
        let gap = || div().flex_none().w(px(COL_GAP));
        let mut line = div().flex().items_center().child(ico_col).child(gap());
        if wide {
            line = line
                .child(div().flex_1().min_w(px(MAIN_MIN_WIDE)).child(main))
                .child(gap())
                .child(cell(
                    PEOPLE_W,
                    div().flex().items_center().children(people_slot.take()),
                ))
                .child(gap())
                .child(why_el(Some(WHY_W)));
        } else {
            line = line.child(div().flex_1().min_w_0().child(main));
        }
        line = line
            .child(gap())
            .child(cell(STATUS_W, status))
            .child(gap())
            .child(cell(DIFF_W, diff))
            .child(gap())
            .child(cell(FILES_W, files))
            .child(gap())
            .child(ci_cell)
            .child(gap())
            .child(cell(WHEN_W, when));
        let mut body = div().flex().flex_col().child(line);
        if !wide {
            let mut under = div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .children(people_slot.take());
            // An empty `.pr-why` is an empty block; an empty text run would take a line.
            under = if why.is_empty() {
                let sel = format!("pr-why:{id}=");
                under.child(div().debug_selector(move || sel))
            } else {
                under.child(why_el(None))
            };
            body = body.child(
                div()
                    .mt(px(4.))
                    .flex()
                    .child(div().flex_none().w(px(26. + COL_GAP)))
                    .child(div().flex_1().min_w_0().child(under)),
            );
        }
        let needs = group == "you";
        let open_id = id.clone();
        let key_id = id.clone();
        let me = cx.entity().downgrade();
        let rid = id.clone();
        let focused = recur_ui::style::is_focus_visible(&focus, window);
        div()
            .id(SharedString::from(format!("prrow-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("prrow:{id}")
            })
            .track_focus(&focus)
            .relative()
            .py(px(12.))
            .px(px(16.))
            .when(!last, |d| d.border_b_1().border_color(hsla(t.line_soft)))
            .map(|d| {
                if picked {
                    d.bg(hsla(t.amber.fade(if hovered { 0.16 } else { 0.11 })))
                } else if hovered {
                    d.bg(hsla(t.pane_2))
                } else {
                    d
                }
            })
            .cursor_pointer()
            .on_hover(move |on, _, cx| {
                let rid = rid.clone();
                let _ = me.update(cx, |this, cx| {
                    if *on {
                        this.hovered_row = Some(rid);
                        cx.notify();
                    } else if this.hovered_row.as_deref() == Some(rid.as_str()) {
                        this.hovered_row = None;
                        cx.notify();
                    }
                });
            })
            .on_click(cx.guarded_listener(move |this, ev: &ClickEvent, _, cx| {
                this.click_row(&open_id, ev.modifiers(), cx)
            }))
            .on_key_down(cx.guarded_listener(move |this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "enter" {
                    cx.stop_propagation();
                    this.open(&key_id, cx);
                }
            }))
            .when(needs || picked, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(hsla(if picked { t.amber } else { t.coral })),
                )
            })
            .child(body)
            .when(focused, |d| {
                d.child(recur_ui::style::focus_ring(4., 0., cx))
            })
            .into_any_element()
    }

    fn header(&self, g: &str, n: usize, cx: &App) -> Div {
        let t = Theme::of(cx).tokens;
        let dot_color = group_color(g, &t).unwrap_or(recur_ui::color::TRANSPARENT);
        let g2 = g.to_string();
        div()
            .debug_selector(move || format!("briefing-h:{g2}"))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(HEAD_H))
            .px(px(16.))
            .bg(hsla(t.text.mix(0.02, t.bg)))
            .border_b_1()
            .border_color(hsla(t.line_soft))
            .child(
                div()
                    .flex_none()
                    .size(px(5.))
                    .rounded_full()
                    .bg(hsla(dot_color))
                    .relative()
                    .top(px(-1.)),
            )
            .child(
                text(group_label(g).to_string())
                    .mono()
                    .size(9.)
                    .line_height(LineHeight::Scale(1.))
                    .tracking(0.13)
                    .uppercase()
                    .color(t.faint)
                    .nowrap(),
            )
            .child(
                text(n.to_string())
                    .mono()
                    .size(9.)
                    .line_height(LineHeight::Scale(1.))
                    .tabular()
                    .color(t.muted)
                    .nowrap(),
            )
    }

    fn empty(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let any_git = self.state().read(cx).projects.iter().any(|p| p.is_git);
        let state = self.state().clone();
        let label = if any_git {
            "Start a loop"
        } else {
            "Get started"
        };
        let btn = recur_ui::style::tbtn(
            "pr-empty-go",
            recur_ui::style::Tbtn::Amber,
            recur_ui::style::TbtnSize::BASE,
            None,
            cx,
        )
        .debug_selector(move || format!("button:{label}"))
        .on_click(move |_, _, cx| {
            state.update(cx, |s, cx| {
                if any_git {
                    s.open_new_task("", None, cx)
                } else {
                    s.go(ViewName::Welcome, None, cx)
                }
            })
        })
        .child(recur_ui::style::button_label(label, 12., 500.));
        empty_state(
            Some(
                icon(IconName::GitPullRequest)
                    .size(px(22.))
                    .stroke_width(1.8)
                    .color(hsla(t.faint)),
            ),
            "No pull requests yet",
            Some(
                "GitHub PRs from projects you’ve added to Recur show up here, including ones you didn’t open. Loops still land here when they converge.",
            ),
            Some(btn.into_any_element()),
            cx,
        )
    }
}

/// `.empty`: a centered icon tile, title, description and action.
pub fn empty_state(
    ei: Option<recur_ui::Icon>,
    et: &str,
    ed: Option<&str>,
    ea: Option<AnyElement>,
    cx: &App,
) -> Div {
    let t = Theme::of(cx).tokens;
    let mut d = div()
        .debug_selector(|| "empty".into())
        .flex()
        .flex_col()
        .items_center()
        .py(px(72.))
        .px(px(20.));
    let mut first = true;
    let mut push = |d: Div, el: AnyElement| {
        let wrapped = div().when(!first, |w| w.mt(px(7.))).child(el);
        first = false;
        d.child(wrapped)
    };
    if let Some(i) = ei {
        d = push(
            d,
            recur_ui::style::card(t.pane, t.line, 12.)
                .size(px(46.))
                .flex()
                .items_center()
                .justify_center()
                .mb(px(8.))
                .child(i)
                .into_any_element(),
        );
    }
    let et = et.to_string();
    d = push(
        d,
        text(et.clone())
            .size(14.)
            .weight(600.)
            .align(recur_ui::text::Align::Center)
            .id(SharedString::from(format!("empty-et-{et}")))
            .into_any_element(),
    );
    if let Some(ed) = ed {
        d = push(
            d,
            div()
                .max_w(px(340.))
                .child(
                    text(ed.to_string())
                        .size(12.5)
                        .leading(1.5)
                        .color(t.muted)
                        .align(recur_ui::text::Align::Center)
                        .id(SharedString::from(format!("empty-ed-{ed}"))),
                )
                .into_any_element(),
        );
    }
    if let Some(ea) = ea {
        d = push(d, div().mt(px(14.)).child(ea).into_any_element());
    }
    d
}

/// `.prrow .ct`: the coral unseen chip.
fn count_chip(id: &str, label: String, cx: &App) -> gpui_kit::Stateful<Div> {
    let t = Theme::of(cx).tokens;
    let id = id.to_string();
    let l2 = label.clone();
    div()
        .id(SharedString::from(format!("prrow-ct-el-{id}")))
        .debug_selector(move || format!("prrow-ct:{id}={l2}"))
        .flex()
        .flex_none()
        .items_center()
        .h(px(16.))
        .px(px(6.))
        .rounded(px(10.))
        .bg(hsla(t.coral))
        .child(
            text(label)
                .size(10.5)
                .weight(600.)
                .line_height(LineHeight::Px(16.))
                .color(recur_ui::hex(0x0a0c10))
                .nowrap(),
        )
}

fn sort_icon(s: ListSort) -> IconName {
    LIST_SORTS
        .iter()
        .find(|x| x.0 == s)
        .map_or(IconName::ClockArrowDown, |x| x.2)
}

/// `1ch` of the mono face at `size` px.
fn mono_ch(size: f32) -> f32 {
    recur_ui::font_metrics::zero_advance(&Family::Mono, size)
}

/// A gap of exactly `w` px between flex items.
fn exact_w(w: f32) -> Div {
    div().flex_none().w(px(w))
}

/// A `w` px track (fractional) with its content centered on it (`display: grid;
/// place-items: center`).
fn exact_cell(w: f32, content: Option<AnyElement>) -> Div {
    div()
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .child(div().flex_none().w(px(w)))
        .when_some(content, |d, c| {
            d.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(c),
            )
        })
}

impl PrList {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let booted = self.state().read(cx).booted;
        let total = self.state().read(cx).prs.len();
        let wide = self.list_width >= WIDE_LIST;
        let grouped = if booted { self.grouped(cx) } else { Vec::new() };
        self.selected
            .retain(|id| grouped.iter().any(|(_, v)| v.iter().any(|p| p.id == *id)));
        let filters = self.filters(window, cx);
        let mut overview = div()
            .debug_selector(|| "prlist".into())
            .relative()
            .flex()
            .flex_col()
            .pb(px(32.));
        let mut heads: Vec<(String, usize)> = Vec::new();
        if !booted {
            for _ in 0..4 {
                overview = overview.child(
                    recur_ui::style::skeleton_row(cx)
                        .child(
                            recur_ui::style::skeleton(window, cx)
                                .flex_none()
                                .size(px(32.))
                                .rounded(px(8.)),
                        )
                        .child(recur_ui::style::skeleton(window, cx).flex_1().h(px(14.))),
                );
            }
        } else if total == 0 {
            overview = overview.child(self.empty(cx));
        } else {
            if grouped.is_empty() {
                overview = overview.child(empty_state(
                    None,
                    "Nothing here",
                    Some("No pull requests match this filter."),
                    None,
                    cx,
                ));
            }
            for (g, items) in grouped {
                let n = items.len();
                let mut bucket = div()
                    .relative()
                    .flex()
                    .flex_col()
                    .debug_selector({
                        let g = g.clone();
                        move || format!("briefing-bucket:{g}")
                    })
                    // The header paints in the layer above the rows; this keeps its place.
                    .child(div().flex_none().h(px(HEAD_H)));
                for (i, p) in items.iter().enumerate() {
                    bucket = bucket.child(self.row(p, &g, i + 1 == n, wide, window, cx));
                }
                let probes = self.probes.clone();
                let key = g.clone();
                bucket = bucket.child(
                    canvas(
                        move |b, _, _| {
                            probes.borrow_mut().buckets.insert(key.clone(), b);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                );
                overview = overview.child(bucket);
                heads.push((g, n));
            }
        }
        // Layers above the rows, in content coordinates: the people popout (inside a row's
        // `z-index: 1`), then the sticky group headers (`z-index: 2`).
        let probes = self.probes.borrow();
        let origin = probes.content.map(|b| f32::from(b.origin.y));
        let scroll_top = -f32::from(self.scroll.offset().y);
        if let (Some(key), Some(o)) = (self.hovered_people.clone(), origin)
            && let Some(b) = probes.people.get(&key).copied()
        {
            let id = key.strip_prefix("row-").unwrap_or(&key).to_string();
            let pr = self.state().read(cx).pr(&id).cloned();
            if let Some(pr) = pr {
                let people = pr_people(&pr);
                if !people.is_empty() {
                    let left = f32::from(b.origin.x)
                        - probes.content.map_or(0., |c| f32::from(c.origin.x));
                    let top = f32::from(b.origin.y) - o + f32::from(b.size.height) + 6.;
                    overview = overview.child(
                        div()
                            .absolute()
                            .left(px(left))
                            .top(px(top))
                            .child(recur_view_widgets::people::popout_for(&key, &people, cx)),
                    );
                }
            }
        }
        if let Some(o) = origin {
            for (g, n) in &heads {
                let Some(b) = probes.buckets.get(g).copied() else {
                    continue;
                };
                let top = f32::from(b.origin.y) - o;
                let bottom = top + f32::from(b.size.height);
                let at = top.max(scroll_top.min(bottom - HEAD_H));
                overview = overview.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(at))
                        .occlude()
                        .child(self.header(g, *n, cx)),
                );
            }
        }
        drop(probes);
        let probes = self.probes.clone();
        let me = cx.entity().downgrade();
        overview = overview.child(
            canvas(
                move |b, _, cx| {
                    let mut p = probes.borrow_mut();
                    let moved = p.content != Some(b);
                    p.content = Some(b);
                    let w = f32::from(b.size.width);
                    let me = me.clone();
                    cx.defer(move |cx| {
                        let _ = me.update(cx, |this, cx| {
                            if (this.list_width - w).abs() > 0.5 || moved {
                                this.list_width = w;
                                cx.notify();
                            }
                        });
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        let me = cx.entity().downgrade();
        div()
            .id("v-pr")
            .debug_selector(|| "v-pr".into())
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(t.bg))
            .on_hover(move |on, _, cx| {
                if !on {
                    let _ = me.update(cx, |this, cx| {
                        this.hovered_row = None;
                        this.hovered_people = None;
                        cx.notify();
                    });
                }
            })
            .on_key_down(cx.guarded_listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" && !this.selected.is_empty() && !this.bulk_busy {
                    cx.stop_propagation();
                    this.clear_selection(cx);
                }
            }))
            .child(filters)
            .child(
                div()
                    .debug_selector(|| "briefing-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        recur_ui::widgets::scroll_area("briefing-scroll")
                            .track(&self.scroll)
                            .child(overview),
                    ),
            )
    }
}

impl Render for PrList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
