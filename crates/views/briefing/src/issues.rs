//! The assigned GitHub and Linear issues, grouped by status as a list or a
//! board, with source / assignee / status / project filters and a sort kept in
//! `issues-view.json` (and for the session in memory, so coming back is instant). Rows open
//! the issue; their action starts or views the issue's workspace; the context menu links the
//! issue to a workspace.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Global, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, anchored, deferred,
    div, point, px,
};
use recur_domain::{IssueRef, Workspace};
use recur_engine::view::artifacts::format_relative_age;
use recur_services::api::{AssignedIssue, IssuesView};
use recur_ui::fonts::weight;
use recur_ui::guard::Guarded;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Select, SelectEvent, SelectOption, TriggerStyle};
use recur_ui::{Color, IconName, Theme, Tokens, icon, rgba};
use recur_view_core::pr::{PrLifecycle, pr_for_workspace, pr_icon_name, pr_lifecycle};
use recur_view_core::pr_list_filter::LIST_SORTS;
use recur_view_core::{NewTaskDraft, ViewContext, ViewName};
use recur_view_core::{ObserveState as _, Topics};

use crate::style::{bucket_head, empty, filter_seg, filters_bar, hsla, seg_count_btn};

/// `GROUPS`: the status groups in list order.
pub const GROUPS: [(&str, &str); 3] = [
    ("progress", "In progress"),
    ("todo", "Todo"),
    ("backlog", "Backlog"),
];
/// `SEGS` (source).
pub const SEGS: [(&str, &str); 3] = [("all", "All"), ("github", "GitHub"), ("linear", "Linear")];
/// `OWNER_SEGS`.
pub const OWNER_SEGS: [(&str, &str); 3] = [
    ("all", "All"),
    ("mine", "Mine"),
    ("unassigned", "Unassigned"),
];
const UNLINKED: &str = "__unlinked";

/// `SORTS`: the PR list's first four, Priority, then Title.
pub fn sorts() -> Vec<(&'static str, &'static str, IconName)> {
    let mut v: Vec<(&'static str, &'static str, IconName)> = LIST_SORTS
        .iter()
        .take(4)
        .map(|(s, l, i)| (s.id(), *l, *i))
        .collect();
    v.push(("priority", "Priority", IconName::SevHigh));
    if let Some((s, l, i)) = LIST_SORTS.get(4) {
        v.push((s.id(), *l, *i));
    }
    v
}

/// The session's last view (`savedView`).
#[derive(Default)]
struct SavedView(Option<IssuesView>);

impl Global for SavedView {}

/// Forgets the session's view (a fresh app, as the parity scenes start).
pub fn reset_saved_view(cx: &mut App) {
    cx.set_global(SavedView(None));
}

/// The filters and layout (`applyView`): unknown values fall back to the defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewState {
    pub source: String,
    pub owner: String,
    pub groups: Vec<String>,
    pub sort: String,
    pub projects: Vec<String>,
    pub layout: String,
}

impl ViewState {
    pub fn from_saved(v: Option<&IssuesView>) -> ViewState {
        let one = |opts: &[&str], x: Option<&String>, d: &str| -> String {
            match x {
                Some(x) if opts.contains(&x.as_str()) => x.clone(),
                _ => d.to_string(),
            }
        };
        let seg_ids: Vec<&str> = SEGS.iter().map(|s| s.0).collect();
        let owner_ids: Vec<&str> = OWNER_SEGS.iter().map(|s| s.0).collect();
        let sort_ids: Vec<&str> = sorts().iter().map(|s| s.0).collect();
        ViewState {
            source: one(&seg_ids, v.map(|v| &v.source), "all"),
            owner: one(&owner_ids, v.map(|v| &v.owner), "all"),
            groups: v.map(|v| v.groups.clone()).unwrap_or_default(),
            sort: one(&sort_ids, v.map(|v| &v.sort), "updated"),
            projects: v.map(|v| v.projects.clone()).unwrap_or_default(),
            layout: if v.is_some_and(|v| v.layout == "board") {
                "board".into()
            } else {
                "list".into()
            },
        }
    }

    pub fn to_saved(&self) -> IssuesView {
        IssuesView {
            layout: self.layout.clone(),
            source: self.source.clone(),
            owner: self.owner.clone(),
            groups: self.groups.clone(),
            sort: self.sort.clone(),
            projects: self.projects.clone(),
        }
    }
}

fn ms(t: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(t)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

fn prio_rank(p: i64) -> i64 {
    if p == 0 { 5 } else { p }
}

/// `compare` for `sort`.
pub fn compare(sort: &str, a: &AssignedIssue, b: &AssignedIssue) -> std::cmp::Ordering {
    match sort {
        "updated-asc" => ms(&a.updated_at).cmp(&ms(&b.updated_at)),
        "newest" => ms(&b.created_at).cmp(&ms(&a.created_at)),
        "oldest" => ms(&a.created_at).cmp(&ms(&b.created_at)),
        "priority" => prio_rank(a.priority)
            .cmp(&prio_rank(b.priority))
            .then(ms(&b.updated_at).cmp(&ms(&a.updated_at))),
        "title" => recur_editor::text::locale_compare(&a.title, &b.title),
        _ => ms(&b.updated_at).cmp(&ms(&a.updated_at)),
    }
}

/// `hex`: a tracker color with its `#`, or `--faint`.
fn hex_color(c: &str, t: &Tokens) -> Color {
    if c.is_empty() {
        return t.faint;
    }
    let s = if c.starts_with('#') {
        c.to_string()
    } else {
        format!("#{c}")
    };
    Color::parse(&s).unwrap_or(t.faint)
}

/// One status column (`columns`).
#[derive(Clone, Debug)]
pub struct Column {
    pub id: String,
    pub label: String,
    pub color: Color,
    pub rank: usize,
    pub pos: f64,
    pub items: Vec<AssignedIssue>,
}

/// `columns`: one per status (merged by name across teams), In progress → Todo → Backlog,
/// furthest Linear workflow position first.
pub fn columns(
    visible: &[AssignedIssue],
    groups: &[String],
    sort: &str,
    t: &Tokens,
) -> Vec<Column> {
    let mut cols: Vec<Column> = Vec::new();
    for i in visible {
        let g = if i.group.is_empty() {
            "todo"
        } else {
            i.group.as_str()
        };
        let rank = GROUPS.iter().position(|x| x.0 == g).unwrap_or(0);
        if !groups.is_empty() && !groups.iter().any(|x| x == GROUPS[rank].0) {
            continue;
        }
        let label = if i.state.is_empty() {
            GROUPS[rank].1.to_string()
        } else {
            i.state.clone()
        };
        let id = format!("{rank}:{label}");
        match cols.iter_mut().find(|c| c.id == id) {
            Some(c) => {
                c.pos = c.pos.min(i.state_position);
                c.items.push(i.clone());
            }
            None => cols.push(Column {
                id,
                label,
                color: if i.source == "github" {
                    t.emerald
                } else {
                    hex_color(&i.state_color, t)
                },
                rank,
                pos: i.state_position,
                items: vec![i.clone()],
            }),
        }
    }
    cols.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then(
                b.pos
                    .partial_cmp(&a.pos)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(recur_editor::text::locale_compare(&a.label, &b.label))
    });
    for c in cols.iter_mut() {
        c.items.sort_by(|a, b| compare(sort, a, b));
    }
    cols
}

/// The open context menu (`menu`).
#[derive(Clone)]
struct Menu {
    x: f32,
    y: f32,
    issue: AssignedIssue,
    linked: Option<Workspace>,
    choices: Vec<Workspace>,
}

fn ws_name(w: &Workspace) -> String {
    if !w.short.is_empty() {
        w.short.clone()
    } else if !w.branch.is_empty() {
        w.branch.clone()
    } else {
        w.id.clone()
    }
}

pub struct Issues {
    vcx: ViewContext,
    issues: Vec<AssignedIssue>,
    loading: bool,
    view: ViewState,
    view_ready: bool,
    seen_stale: Option<u64>,
    menu: Option<Menu>,
    hover_row: Option<String>,
    project_select: Entity<Select>,
    group_select: Entity<Select>,
    sort_select: Entity<Select>,
    scroll: ScrollHandle,
    board_scroll: ScrollHandle,
    row_focus: std::collections::HashMap<String, FocusHandle>,
    _subs: Vec<Subscription>,
}

impl Issues {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = TriggerStyle {
            value_shrink: true,
            ..Default::default()
        };
        let project_select = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Filter by project")
                .placeholder("All projects")
                .multiple()
                .ghost()
                .trigger_style(TriggerStyle {
                    max_width: Some(176.),
                    ..filter.clone()
                })
        });
        let group_select = cx.new(|cx| {
            Select::new(
                GROUPS
                    .iter()
                    .map(|(v, l)| SelectOption::new(*v, *l))
                    .collect(),
                window,
                cx,
            )
            .label("Filter by group")
            .placeholder("Any status")
            .multiple()
            .ghost()
            .trigger_style(filter.clone())
        });
        let sort_select = cx.new(|cx| {
            Select::new(
                sorts()
                    .iter()
                    .map(|(v, l, _)| SelectOption::new(*v, *l))
                    .collect(),
                window,
                cx,
            )
            .label("Sort issues")
            .ghost()
        });
        let subs = vec![
            cx.observe_state(&vcx.state, Topics::ALL, |this, _, cx| this.on_state(cx)),
            // The menu's window keydown (capture): Escape closes it.
            {
                let me = cx.entity().downgrade();
                cx.intercept_keystrokes(move |ev: &gpui_kit::KeystrokeEvent, _, cx| {
                    if ev.keystroke.key == "escape" {
                        let _ = me.update(cx, |v, cx| {
                            if v.menu.is_some() {
                                cx.stop_propagation();
                                v.close_menu(cx);
                            }
                        });
                    }
                })
            },
            cx.guarded_subscribe(&project_select, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Values(ids) = ev {
                    this.view.projects = ids.iter().map(|s| s.to_string()).collect();
                    this.save_view(cx);
                }
            }),
            cx.guarded_subscribe(&group_select, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Values(ids) = ev {
                    this.view.groups = ids.iter().map(|s| s.to_string()).collect();
                    this.save_view(cx);
                }
            }),
            cx.guarded_subscribe(&sort_select, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    this.view.sort = v.to_string();
                    this.save_view(cx);
                }
            }),
        ];
        let mut me = Issues {
            vcx: vcx.clone(),
            issues: Vec::new(),
            loading: true,
            view: ViewState::from_saved(None),
            view_ready: false,
            seen_stale: None,
            menu: None,
            hover_row: None,
            project_select,
            group_select,
            sort_select,
            scroll: ScrollHandle::new(),
            board_scroll: ScrollHandle::new(),
            row_focus: Default::default(),
            _subs: subs,
        };
        let saved = cx.try_global::<SavedView>().and_then(|s| s.0.clone());
        match saved {
            Some(v) => me.apply_view(Some(&v), cx),
            None => {
                let services = vcx.services.clone();
                cx.spawn(async move |this, cx| {
                    let v = services.shell.get_issues_view().await.ok();
                    let _ = this.update(cx, |me, cx| me.apply_view(v.as_ref(), cx));
                })
                .detach();
            }
        }
        me.on_state(cx);
        me
    }

    pub fn view_state(&self) -> &ViewState {
        &self.view
    }

    fn apply_view(&mut self, v: Option<&IssuesView>, cx: &mut Context<Self>) {
        self.view = ViewState::from_saved(v);
        self.view_ready = true;
        let groups: Vec<SharedString> = self.view.groups.iter().map(|s| s.clone().into()).collect();
        let sort = self.view.sort.clone();
        self.group_select
            .update(cx, |s, cx| s.set_values(groups, cx));
        self.sort_select.update(cx, |s, cx| s.set_value(sort, cx));
        cx.set_global(SavedView(Some(self.view.to_saved())));
        cx.notify();
    }

    /// The persist effect: the session cache and `issues-view.json`.
    fn save_view(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if !self.view_ready {
            return;
        }
        let v = self.view.to_saved();
        cx.set_global(SavedView(Some(v.clone())));
        let services = self.vcx.services.clone();
        cx.spawn(async move |_, _| {
            let _ = services.shell.set_issues_view(&v).await;
        })
        .detach();
    }

    /// `issuesStale.subscribe(load)`.
    fn on_state(&mut self, cx: &mut Context<Self>) {
        let stale = self.vcx.state.read(cx).issues_stale;
        if self.seen_stale != Some(stale) {
            self.seen_stale = Some(stale);
            self.load(cx);
        }
        cx.notify();
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let r = services.settings.list_assigned_issues().await;
            let _ = this.update(cx, |v, cx| {
                if let Ok(rows) = r {
                    v.issues = rows;
                }
                v.loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn has(&self, source: &str) -> bool {
        self.issues.iter().any(|i| i.source == source)
    }

    /// `projectOptions`.
    fn project_options(&self, cx: &App) -> Vec<(String, String)> {
        let ids: std::collections::BTreeSet<String> = self
            .issues
            .iter()
            .map(|i| {
                if i.project_id.is_empty() {
                    UNLINKED.to_string()
                } else {
                    i.project_id.clone()
                }
            })
            .collect();
        let mut opts: Vec<(String, String)> = self
            .vcx
            .state
            .read(cx)
            .projects
            .iter()
            .filter(|p| ids.contains(&p.id))
            .map(|p| {
                (
                    p.id.clone(),
                    if p.name.is_empty() {
                        p.id.clone()
                    } else {
                        p.name.clone()
                    },
                )
            })
            .collect();
        if ids.contains(UNLINKED) {
            opts.push((UNLINKED.into(), "Unlinked".into()));
        }
        opts
    }

    fn visible(&self, cx: &App) -> Vec<AssignedIssue> {
        let opts = self.project_options(cx);
        let active: Vec<&String> = self
            .view
            .projects
            .iter()
            .filter(|id| opts.iter().any(|(v, _)| v == *id))
            .collect();
        let seg = if self.has("linear") && self.has("github") {
            self.view.source.as_str()
        } else {
            "all"
        };
        self.issues
            .iter()
            .filter(|i| {
                (seg == "all" || i.source == seg)
                    && (self.view.owner == "all"
                        || (self.view.owner == "mine")
                            == i.assignees.as_ref().is_some_and(|a| !a.is_empty()))
                    && (active.is_empty() || {
                        let pid = if i.project_id.is_empty() {
                            UNLINKED.to_string()
                        } else {
                            i.project_id.clone()
                        };
                        active.contains(&&pid)
                    })
            })
            .cloned()
            .collect()
    }

    fn linked_workspace(&self, i: &AssignedIssue, cx: &App) -> Option<Workspace> {
        self.vcx
            .state
            .read(cx)
            .workspaces
            .iter()
            .find(|w| {
                w.issue
                    .as_ref()
                    .is_some_and(|r| r.source == i.source && r.id == i.id)
            })
            .cloned()
    }

    fn open_issue(&self, i: &AssignedIssue, cx: &mut App) {
        let (source, url) = (i.source.clone(), i.url.clone());
        let services = self.vcx.services.clone();
        cx.spawn(async move |cx| {
            if source == "linear"
                && services
                    .settings
                    .open_linear_in_app(&url)
                    .await
                    .unwrap_or(false)
            {
                return;
            }
            cx.update(|cx| recur_content::links::open_external_url(&url, cx));
        })
        .detach();
    }

    /// `startWorkspace`: New workspace prefilled with the issue's slash prompt.
    fn start_workspace(&self, i: &AssignedIssue, cx: &mut App) {
        let draft = NewTaskDraft {
            text: format!("/{} {}", i.source, i.r#ref),
            linear_key: if i.source == "linear" {
                if i.linear_project.is_empty() {
                    i.linear_team.clone()
                } else {
                    i.linear_project.clone()
                }
            } else {
                String::new()
            },
            linked_project_id: i.project_id.clone(),
            ..Default::default()
        };
        let pid = i.project_id.clone();
        self.vcx
            .state
            .update(cx, |s, cx| s.open_new_task(&pid, Some(draft), cx));
    }

    fn open_menu(
        &mut self,
        ev: &MouseDownEvent,
        i: &AssignedIssue,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let choices: Vec<Workspace> = self
            .vcx
            .state
            .read(cx)
            .workspaces
            .iter()
            .filter(|w| {
                !w.archived
                    && !w.is_main
                    && (i.project_id.is_empty() || w.project_id == i.project_id)
            })
            .cloned()
            .collect();
        let pad: f32 = 8.;
        let h = 360f32.min((choices.len() + 3) as f32 * 44.);
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
        self.menu = Some(Menu {
            x: pad.max(x.min(vw - 300. - pad)),
            y: pad.max(y.min(vh - h - pad)),
            issue: i.clone(),
            linked: self.linked_workspace(i, cx),
            choices,
        });
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    fn row_focus(&mut self, key: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.row_focus
            .entry(key.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    // ---- render pieces ----

    /// `ref`: the source mark and the number.
    fn ref_el(i: &AssignedIssue, t: &Tokens) -> AnyElement {
        let gh = i.source == "github";
        let label = if gh {
            format!("#{}", i.r#ref.rsplit('#').next().unwrap_or(""))
        } else {
            i.r#ref.clone()
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .text_color(hsla(t.faint))
            .child(titled(
                SharedString::from(format!("iss-src-{}", i.url)),
                if gh { "GitHub issue" } else { "Linear issue" },
                div()
                    .id(SharedString::from(format!("iss-src-el-{}", i.url)))
                    .flex()
                    .items_center()
                    .child(
                        icon(if gh {
                            IconName::Github
                        } else {
                            IconName::Linear
                        })
                        .size(px(14.)),
                    ),
            ))
            .child(
                text(label)
                    .mono()
                    .size(12.)
                    .line_height(LineHeight::Scale(1.))
                    .nowrap(),
            )
            .into_any_element()
    }

    /// `sub`: the project, comments and labels.
    fn sub_el(&self, i: &AssignedIssue, wrap: bool, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let linked = self.vcx.state.read(cx).project(&i.project_id).cloned();
        let mut d = div()
            .mt(px(3.))
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .when(wrap, |d| d.flex_wrap().gap_y(px(4.)))
            .when(!wrap, |d| d.overflow_hidden());
        let mono = |s: String, c: Color| text(s).mono().size(10.).color(c).nowrap();
        d = match linked {
            Some(p) => d.child(titled(
                SharedString::from(format!("iss-proj-{}", i.url)),
                "Recur project",
                div()
                    .id(SharedString::from(format!("iss-proj-el-{}", i.url)))
                    .flex_none()
                    .child(mono(if p.name.is_empty() { p.id } else { p.name }, t.muted)),
            )),
            None => d.child(titled(
                SharedString::from(format!("iss-proj-{}", i.url)),
                "Not linked to a Recur project",
                div()
                    .id(SharedString::from(format!("iss-proj-el-{}", i.url)))
                    .flex_none()
                    .child(mono(i.project.clone(), t.faint)),
            )),
        };
        if i.comments > 0 {
            d = d.child(div().flex_none().child(mono(
                format!(
                    "· {} {}",
                    i.comments,
                    if i.comments == 1 {
                        "comment"
                    } else {
                        "comments"
                    }
                ),
                t.faint,
            )));
        }
        for l in i.labels.as_deref().unwrap_or_default() {
            d = d.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(5.))
                    .child(
                        div()
                            .flex_none()
                            .size(px(6.))
                            .rounded_full()
                            .bg(hsla(hex_color(&l.color, &t))),
                    )
                    .child(mono(l.name.clone(), t.faint)),
            );
        }
        d.into_any_element()
    }

    /// `people`: the assignee stack or the unassigned mark.
    fn people_el(i: &AssignedIssue, ring: Color, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let people: Vec<recur_forge::view::PrPerson> = i
            .assignees
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(recur_forge::view::PrPerson::from)
            .collect();
        if people.is_empty() {
            return titled(
                SharedString::from(format!("iss-unassigned-{}", i.url)),
                "Unassigned",
                div()
                    .id(SharedString::from(format!("iss-unassigned-el-{}", i.url)))
                    .flex()
                    .text_color(hsla(t.faint))
                    .child(
                        icon(IconName::CircleUserRound)
                            .size(px(20.))
                            .stroke_width(1.5),
                    ),
            )
            .into_any_element();
        }
        recur_view_widgets::people::people_stack(
            &format!("iss-{}", i.url),
            &people,
            ring,
            |_, _, _| {},
            cx,
        )
    }

    /// `action`: View workspace for a linked issue, else Create workspace.
    fn action_el(&self, i: &AssignedIssue, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let ws = self.linked_workspace(i, cx);
        let (label, tip, ic) = if ws.is_some() {
            (
                "View workspace",
                "View this issue's workspace",
                IconName::Box,
            )
        } else {
            (
                "Create workspace",
                "Create a Recur workspace for this issue (assigns it to you if unassigned)",
                IconName::Plus,
            )
        };
        let issue = i.clone();
        let btn = recur_ui::set::tbtn(
            SharedString::from(format!("iss-start-{}", i.url)),
            false,
            cx,
        )
        .debug_selector({
            let url = i.url.clone();
            move || format!("iss-start:{url}={label}")
        })
        .when(compact, |d| d.h(px(24.)).px(px(8.)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.guarded_listener(move |v, _, _, cx| {
            cx.stop_propagation();
            match &ws {
                Some(w) => {
                    let id = w.id.clone();
                    v.vcx
                        .state
                        .update(cx, |s, cx| s.open_workspace(&id, false, cx));
                }
                None => v.start_workspace(&issue, cx),
            }
        }))
        .child(recur_ui::set::tbtn_icon(ic))
        .child(recur_ui::style::button_label(label, 12., weight::MEDIUM));
        titled(
            SharedString::from(format!("iss-start-t-{}", i.url)),
            tip,
            btn,
        )
        .into_any_element()
    }

    fn prio_color(p: i64, t: &Tokens) -> Color {
        match p {
            1 => t.coral,
            2 => t.amber,
            _ => t.faint,
        }
    }

    /// One `.prrow.iss`.
    fn row(
        &mut self,
        i: &AssignedIssue,
        last: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tint = if i.source == "github" {
            t.emerald
        } else {
            hex_color(&i.state_color, &t)
        };
        let key = i.url.clone();
        let hovered = self.hover_row.as_deref() == Some(key.as_str());
        let focus = self.row_focus(&key, cx);
        let ring = recur_ui::style::is_focus_visible(&focus, window);
        let ico = div().flex_none().w(px(26.)).flex().items_center().child(
            div()
                .size(px(22.))
                .rounded(px(6.))
                .bg(hsla(tint.fade(0.12)))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::CircleDot).size(px(13.)).color(hsla(tint))),
        );
        let main = div()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(Self::ref_el(i, &t))
                    .child(
                        div().min_w_0().child(
                            text(i.title.clone())
                                .id(SharedString::from(format!("iss-title-{key}")))
                                .size(13.)
                                .leading(1.45)
                                .truncate(),
                        ),
                    ),
            )
            .child(self.sub_el(i, false, cx));
        let ring_c = if hovered { t.pane_2 } else { t.pane };
        let people = div()
            .flex_none()
            .w(px(64.))
            .flex()
            .items_center()
            .child(Self::people_el(i, ring_c, cx));
        let status = div()
            .flex_none()
            .w(px(156.))
            .flex()
            .items_center()
            .gap(px(7.))
            .min_w_0()
            .child(with_shadow(
                recur_view_widgets::chrome::dot(7., tint),
                &[css_box_shadow(0., 0., 5., 0., tint)],
                3.5,
            ))
            .child(text(i.state.clone()).mono().size(11.).color(tint).nowrap());
        let prio = div().flex_none().w(px(72.)).flex().justify_end().child(
            text(if i.priority > 0 {
                i.priority_label.clone()
            } else {
                String::new()
            })
            .mono()
            .size(10.5)
            .color(Self::prio_color(i.priority, &t))
            .nowrap(),
        );
        let now = chrono::Utc::now().timestamp_millis();
        let when = div().flex_none().w(px(44.)).flex().justify_end().child(
            text(
                format_relative_age(ms(&i.updated_at), now)
                    .map(|a| a.rel)
                    .unwrap_or_default(),
            )
            .mono()
            .size(10.)
            .tabular()
            .color(t.faint)
            .nowrap(),
        );
        let action = self.action_el(i, false, cx);
        let gap = || div().flex_none().w(px(16.));
        let (oi, ki, mi, hk) = (i.clone(), i.clone(), i.clone(), key.clone());
        div()
            .id(SharedString::from(format!("iss-row-{key}")))
            .debug_selector({
                let title = i.title.clone();
                move || format!("iss-row:{title}")
            })
            .track_focus(&focus)
            .relative()
            .flex()
            .items_center()
            .py(px(12.))
            .px(px(16.))
            .when(!last, |d| d.border_b_1().border_color(hsla(t.line_soft)))
            .when(hovered, |d| d.bg(hsla(t.pane_2)))
            .cursor_pointer()
            .on_hover(cx.guarded_listener(move |v, h: &bool, _, cx| {
                if *h {
                    v.hover_row = Some(hk.clone());
                } else if v.hover_row.as_deref() == Some(hk.as_str()) {
                    v.hover_row = None;
                }
                cx.notify();
            }))
            .on_click(cx.guarded_listener(move |v, _, _, cx| v.open_issue(&oi, cx)))
            .on_key_down(cx.guarded_listener(move |v, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "enter" {
                    cx.stop_propagation();
                    v.open_issue(&ki, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.guarded_listener(move |v, ev: &MouseDownEvent, window, cx| {
                    v.open_menu(ev, &mi, window, cx)
                }),
            )
            .child(ico)
            .child(gap())
            .child(main)
            .child(gap())
            .child(people)
            .child(gap())
            .child(status)
            .child(gap())
            .child(prio)
            .child(gap())
            .child(when)
            .child(gap())
            .child(div().flex_none().child(action))
            .when(ring, |d| d.child(recur_ui::style::focus_ring(4., 0., cx)))
            .into_any_element()
    }

    /// One `.iss-card` on the board.
    fn card(
        &mut self,
        i: &AssignedIssue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tint = if i.source == "github" {
            t.emerald
        } else {
            hex_color(&i.state_color, &t)
        };
        let key = i.url.clone();
        let hovered = self.hover_row.as_deref() == Some(key.as_str());
        let focus = self.row_focus(&key, cx);
        let ring = recur_ui::style::is_focus_visible(&focus, window);
        let main = div()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(Self::ref_el(i, &t))
                    .child(div().flex_1())
                    .child(text(i.state.clone()).mono().size(10.5).color(tint).nowrap()),
            )
            .child(
                div().mt(px(6.)).child(
                    text(i.title.clone())
                        .id(SharedString::from(format!("iss-card-title-{key}")))
                        .size(13.)
                        .leading(1.4)
                        .line_clamp(3),
                ),
            )
            .child(self.sub_el(i, true, cx));
        let mut foot = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w_0()
            .child(Self::people_el(
                i,
                if hovered { t.pane_2 } else { t.pane },
                cx,
            ));
        if i.priority > 0 {
            foot = foot.child(
                text(i.priority_label.clone())
                    .mono()
                    .size(10.5)
                    .color(Self::prio_color(i.priority, &t))
                    .nowrap(),
            );
        }
        let now = chrono::Utc::now().timestamp_millis();
        foot = foot
            .child(
                text(
                    format_relative_age(ms(&i.updated_at), now)
                        .map(|a| a.rel)
                        .unwrap_or_default(),
                )
                .mono()
                .size(10.)
                .tabular()
                .color(t.faint)
                .nowrap(),
            )
            .child(div().flex_1())
            .child(self.action_el(i, true, cx));
        let (oi, ki, mi, hk) = (i.clone(), i.clone(), i.clone(), key.clone());
        recur_ui::style::card(
            if hovered { t.pane_2 } else { t.pane },
            if hovered { t.line } else { t.line_soft },
            8.,
        )
        .id(SharedString::from(format!("iss-card-{key}")))
        .debug_selector({
            let title = i.title.clone();
            move || format!("iss-card:{title}")
        })
        .track_focus(&focus)
        .relative()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(10.))
        .py(px(10.))
        .px(px(12.))
        .cursor_pointer()
        .on_hover(cx.guarded_listener(move |v, h: &bool, _, cx| {
            if *h {
                v.hover_row = Some(hk.clone());
            } else if v.hover_row.as_deref() == Some(hk.as_str()) {
                v.hover_row = None;
            }
            cx.notify();
        }))
        .on_click(cx.guarded_listener(move |v, _, _, cx| v.open_issue(&oi, cx)))
        .on_key_down(cx.guarded_listener(move |v, ev: &KeyDownEvent, _, cx| {
            if ev.keystroke.key == "enter" {
                cx.stop_propagation();
                v.open_issue(&ki, cx);
            }
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.guarded_listener(move |v, ev: &MouseDownEvent, window, cx| {
                v.open_menu(ev, &mi, window, cx)
            }),
        )
        .child(main)
        .child(foot)
        .when(ring, |d| {
            d.child(recur_ui::style::outline(2., -2., 8., 1., t.amber))
        })
        .into_any_element()
    }

    fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.menu.clone()?;
        let t = Theme::of(cx).tokens;
        let prs = self.vcx.state.read(cx).prs.clone();
        let item = |id: String, disabled: bool| {
            div()
                .id(SharedString::from(id))
                .flex()
                .items_start()
                .gap(px(10.))
                .w_full()
                .px(px(10.))
                .py(px(7.))
                .rounded(px(5.))
                .text_color(hsla(t.text))
                .when(disabled, |d| d.opacity(0.4))
                .when(!disabled, |d| {
                    d.cursor_pointer().hover(move |s| s.bg(hsla(t.pane_2)))
                })
        };
        let label = |s: String| {
            div().flex_1().min_w_0().child(
                text(s)
                    .size(12.)
                    .line_height(LineHeight::Px(16.))
                    .truncate(),
            )
        };
        let mut menu = div()
            .id("iss-ctx")
            .debug_selector(|| "iss-ctx".into())
            .min_w(px(220.))
            .max_w(px(300.))
            .max_h(px(360.))
            .p(px(4.))
            .bg(hsla(t.pane))
            .border_1()
            .border_color(hsla(t.line_bright))
            .rounded(px(8.))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        if let Some(w) = &m.linked {
            let id = w.id.clone();
            menu = menu.child(
                item("iss-ctx-view".into(), false)
                    .debug_selector(|| "iss-ctx:View workspace".into())
                    .on_click(cx.guarded_listener(move |v, _, _, cx| {
                        v.close_menu(cx);
                        v.vcx
                            .state
                            .update(cx, |s, cx| s.open_workspace(&id, false, cx));
                    }))
                    .child(label("View workspace".into())),
            );
            let id = w.id.clone();
            let name = ws_name(w);
            menu = menu
                .child(
                    item("iss-ctx-unlink".into(), false)
                        .debug_selector({
                            let name = name.clone();
                            move || format!("iss-ctx:Unlink from {name}")
                        })
                        .on_click(cx.guarded_listener(move |v, _, _, cx| {
                            v.close_menu(cx);
                            v.vcx
                                .state
                                .update(cx, |s, cx| s.unlink_workspace_issue(&id, cx));
                        }))
                        .child(label(format!("Unlink from {name}"))),
                )
                .child(div().h(px(1.)).mx(px(6.)).my(px(4.)).bg(hsla(t.line)));
        }
        menu = menu.child(
            div().pt(px(6.)).px(px(10.)).pb(px(4.)).child(
                text(if m.linked.is_some() {
                    "Move to workspace"
                } else {
                    "Link to workspace"
                })
                .size(10.5)
                .leading(1.45)
                .color(t.faint),
            ),
        );
        if m.choices.is_empty() {
            menu = menu.child(
                item("iss-ctx-none".into(), true)
                    .child(label("No workspaces in this project".into())),
            );
        }
        for w in &m.choices {
            let ws_pr = pr_for_workspace(&prs, &w.id).cloned();
            let life = pr_lifecycle(ws_pr.as_ref())
                .or((w.state.as_str() == "merged").then_some(PrLifecycle::Merged));
            let mark = match life {
                Some(l) => {
                    let c = match l {
                        PrLifecycle::Open => t.emerald,
                        PrLifecycle::Draft => t.cyan,
                        PrLifecycle::Merged => t.indigo,
                        PrLifecycle::Closed => t.coral,
                        PrLifecycle::Queued => t.warn,
                    };
                    let name = if ws_pr.is_some() {
                        pr_icon_name(ws_pr.as_ref())
                    } else {
                        IconName::GitMerge
                    };
                    icon(name).size(px(14.)).color(hsla(c))
                }
                None => icon(IconName::GitBranch).size(px(14.)),
            };
            let current = m.linked.as_ref().is_some_and(|l| l.id == w.id);
            let name = ws_name(w);
            let mut names = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    text(name.clone())
                        .size(12.)
                        .line_height(LineHeight::Px(16.))
                        .truncate(),
                );
            if !w.branch.is_empty() && w.branch != name {
                names = names.child(
                    text(w.branch.clone())
                        .mono()
                        .size(10.5)
                        .line_height(LineHeight::Px(14.))
                        .color(t.faint)
                        .truncate(),
                );
            }
            let (wid, issue) = (w.id.clone(), m.issue.clone());
            menu = menu.child(
                item(format!("iss-ctx-ws-{}", w.id), current)
                    .debug_selector({
                        let name = name.clone();
                        move || format!("iss-ctx-ws:{name}:{current}")
                    })
                    .when(!current, |d| {
                        d.on_click(cx.guarded_listener(move |v, _, _, cx| {
                            v.close_menu(cx);
                            let r = IssueRef {
                                source: issue.source.clone(),
                                id: issue.id.clone(),
                                key: issue.r#ref.clone(),
                            };
                            let wid = wid.clone();
                            v.vcx
                                .state
                                .update(cx, |s, cx| s.link_workspace_issue(&wid, r, cx));
                        }))
                    })
                    .child(div().mt(px(1.)).flex_none().child(mark))
                    .child(names)
                    .when(current, |d| d.child(icon(IconName::Check).size(px(14.)))),
            );
        }
        let menu = with_shadow(
            menu,
            &[css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))],
            8.,
        );
        Some(
            deferred(
                anchored().position(point(px(m.x), px(m.y))).child(
                    div()
                        .on_mouse_down_out(cx.guarded_listener(|v, _, _, cx| v.close_menu(cx)))
                        .child(menu),
                ),
            )
            // `.obs-ctx`'s `z-index: 80`.
            .with_priority(80)
            .into_any_element(),
        )
    }

    fn filters(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let mut bar = filters_bar(cx);
        if self.loading && !self.issues.is_empty() {
            let deg = recur_view_widgets::chrome::spin_deg(window, cx);
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
        let opts = self.project_options(cx);
        if opts.len() > 1 {
            let active: Vec<SharedString> = self
                .view
                .projects
                .iter()
                .filter(|id| opts.iter().any(|(v, _)| v == *id))
                .map(|s| s.clone().into())
                .collect();
            let options = opts
                .into_iter()
                .map(|(v, l)| SelectOption::new(v, l))
                .collect();
            self.project_select.update(cx, |s, cx| {
                s.set_options(options, cx);
                if s.values() != active.as_slice() {
                    s.set_values(active, cx);
                }
            });
            bar = bar.child(self.project_select.clone());
        }
        let sort_icon = sorts()
            .iter()
            .find(|s| s.0 == self.view.sort)
            .map(|s| s.2)
            .unwrap_or(IconName::ClockArrowDown);
        self.sort_select
            .update(cx, |s, cx| s.set_icon(Some(sort_icon), cx));
        bar = bar
            .child(self.group_select.clone())
            .child(self.sort_select.clone());
        let loading = self.loading;
        bar = bar.child(titled(
            "iss-refresh-t",
            "Refresh issues",
            recur_ui::set::tbtn("iss-refresh", loading, cx)
                .debug_selector(|| "iss-refresh".into())
                .when(!loading, |d| {
                    d.on_click(cx.guarded_listener(|v, _, _, cx| {
                        v.vcx.state.update(cx, |s, cx| {
                            s.issues_stale += 1;
                            cx.notify();
                        })
                    }))
                })
                .child(recur_ui::set::tbtn_icon(IconName::RefreshCw)),
        ));
        let mut owner = filter_seg(cx);
        for (id, l) in OWNER_SEGS {
            let on = self.view.owner == id;
            owner = owner.child(
                seg_count_btn(
                    SharedString::from(format!("iss-owner-{id}")),
                    l,
                    None,
                    on,
                    cx,
                )
                .on_click(cx.guarded_listener(move |v, _, _, cx| {
                    v.view.owner = id.to_string();
                    v.save_view(cx);
                })),
            );
        }
        bar = bar.child(owner);
        if self.has("linear") && self.has("github") {
            let mut src = filter_seg(cx);
            for (id, l) in SEGS {
                let on = self.view.source == id;
                src = src.child(
                    seg_count_btn(SharedString::from(format!("iss-src-{id}")), l, None, on, cx)
                        .on_click(cx.guarded_listener(move |v, _, _, cx| {
                            v.view.source = id.to_string();
                            v.save_view(cx);
                        })),
                );
            }
            bar = bar.child(src);
        }
        let mut layout = filter_seg(cx);
        for (id, ic, tip) in [
            ("list", IconName::Rows3, "List"),
            ("board", IconName::Columns3, "Board"),
        ] {
            let on = self.view.layout == id;
            layout = layout.child(titled(
                SharedString::from(format!("iss-layout-t-{id}")),
                tip,
                recur_view_widgets::chrome::seg_text_btn(
                    SharedString::from(format!("iss-layout-{id}")),
                    "",
                    on,
                    cx,
                )
                .debug_selector(move || format!("iss-layout:{id}:{on}"))
                .child(icon(ic).size(px(13.)))
                .on_click(cx.guarded_listener(move |v, _, _, cx| {
                    v.view.layout = id.to_string();
                    v.save_view(cx);
                })),
            ));
        }
        bar.child(layout).into_any_element()
    }
}

impl Issues {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let filters = self.filters(window, cx);
        let visible = self.visible(cx);
        let cols = columns(&visible, &self.view.groups, &self.view.sort, &t);
        let grouped: Vec<Column> = cols
            .iter()
            .filter(|c| !c.items.is_empty())
            .cloned()
            .collect();
        let body: AnyElement = if self.view.layout == "board" && !grouped.is_empty() {
            let mut columns_el: Vec<AnyElement> = Vec::new();
            for c in cols.iter().rev() {
                let mut col_body = div().flex().flex_col().gap(px(8.)).pb(px(16.));
                for i in &c.items {
                    col_body = col_body.child(self.card(i, window, cx));
                }
                columns_el.push(
                    div()
                        .flex_none()
                        .w(px(325.))
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            bucket_head(&c.label, &c.label, c.items.len(), c.color, cx)
                                .h(px(crate::style::HEAD_H + 1.))
                                .border_1()
                                .rounded(px(6.)),
                        )
                        .child(
                            div().flex_1().min_h_0().flex().child(
                                recur_ui::widgets::scroll_area(SharedString::from(format!(
                                    "iss-col-{}",
                                    c.id
                                )))
                                .child(col_body),
                            ),
                        )
                        .into_any_element(),
                );
            }
            div()
                .debug_selector(|| "iss-board".into())
                .flex_1()
                .min_h_0()
                .flex()
                .child(
                    recur_ui::widgets::scroll_area("iss-board")
                        .horizontal()
                        .track(&self.board_scroll)
                        .scroller(|d| d.flex().gap(px(12.)).py(px(12.)).px(px(16.)))
                        .children(columns_el),
                )
                .into_any_element()
        } else {
            let mut overview = div().flex().flex_col().pb(px(32.));
            if self.loading && self.issues.is_empty() {
                for _ in 0..4 {
                    overview = overview.child(
                        recur_ui::style::skeleton_row(cx)
                            .child(
                                recur_ui::style::skeleton(window, cx)
                                    .size(px(32.))
                                    .rounded(px(8.)),
                            )
                            .child(recur_ui::style::skeleton(window, cx).flex_1().h(px(14.))),
                    );
                }
            } else if self.issues.is_empty() {
                let ed = recur_ui::text::Text::from_spans(vec![
                    recur_ui::text::span("Open GitHub issues (via "),
                    recur_ui::set::code("gh auth login", cx),
                    recur_ui::text::span(") and Linear issues (via an API key in Settings) assigned to you, or unassigned in your project repos and Linear teams, show up here."),
                ])
                .id("iss-empty-ed")
                .size(12.5)
                .leading(1.5)
                .color(t.muted)
                .align(recur_ui::text::Align::Center);
                overview = overview.child(empty(
                    Some(IconName::CircleDot),
                    "No open issues",
                    Some(ed.into_any_element()),
                    None,
                    cx,
                ));
            } else if grouped.is_empty() {
                overview = overview.child(empty(
                    None,
                    "Nothing here",
                    Some(crate::style::empty_desc("No issues match this filter.", cx)),
                    None,
                    cx,
                ));
            } else {
                for c in &grouped {
                    let mut section = div().flex().flex_col().child(bucket_head(
                        &c.label,
                        &c.label,
                        c.items.len(),
                        c.color,
                        cx,
                    ));
                    let n = c.items.len();
                    for (ix, i) in c.items.iter().enumerate() {
                        section = section.child(self.row(i, ix + 1 == n, window, cx));
                    }
                    overview = overview.child(section);
                }
            }
            div()
                .debug_selector(|| "briefing-scroll".into())
                .flex_1()
                .min_h_0()
                .flex()
                .child(
                    recur_ui::widgets::scroll_area("iss-scroll")
                        .track(&self.scroll)
                        .child(overview),
                )
                .into_any_element()
        };
        let me = cx.entity().downgrade();
        div()
            .id("v-issues")
            .debug_selector(|| "v-issues".into())
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(t.bg))
            .on_hover(move |h, _, cx| {
                if !*h {
                    let _ = me.update(cx, |v, cx| {
                        v.hover_row = None;
                        cx.notify();
                    });
                }
            })
            .child(filters)
            .child(body)
            .children(self.render_menu(cx))
    }
}

impl Render for Issues {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The Issues route, built on each visit.
pub struct IssuesPage {
    vcx: ViewContext,
    list: Option<Entity<Issues>>,
    _subs: Vec<Subscription>,
}

impl IssuesPage {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs =
            vec![
                cx.observe_state_in(&vcx.state, Topics::ALL, window, |this, _, window, cx| {
                    this.sync(window, cx)
                }),
            ];
        let mut me = IssuesPage {
            vcx: vcx.clone(),
            list: None,
            _subs: subs,
        };
        me.sync(window, cx);
        me
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on = self.vcx.state.read(cx).view == ViewName::Issues;
        if on && self.list.is_none() {
            let vcx = self.vcx.clone();
            self.list = Some(cx.new(|cx| Issues::new(&vcx, window, cx)));
            cx.notify();
        } else if !on && self.list.is_some() {
            self.list = None;
            cx.notify();
        }
    }

    pub fn list(&self) -> Option<&Entity<Issues>> {
        self.list.as_ref()
    }
}

impl IssuesPage {
    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement + use<> {
        div().size_full().flex().children(self.list.clone())
    }
}

impl Render for IssuesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(title: &str, group: &str, state: &str, pos: f64, updated: &str) -> AssignedIssue {
        AssignedIssue {
            source: "linear".into(),
            title: title.into(),
            group: group.into(),
            state: state.into(),
            state_position: pos,
            updated_at: updated.into(),
            url: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn view_state_falls_back_on_unknown_values() {
        let v = IssuesView {
            layout: "board".into(),
            source: "jira".into(),
            owner: "mine".into(),
            groups: vec!["todo".into()],
            sort: "priority".into(),
            projects: vec!["p".into()],
        };
        let s = ViewState::from_saved(Some(&v));
        assert_eq!(s.layout, "board");
        assert_eq!(s.source, "all");
        assert_eq!(s.owner, "mine");
        assert_eq!(s.sort, "priority");
        assert_eq!(ViewState::from_saved(None).layout, "list");
        assert_eq!(ViewState::from_saved(None).sort, "updated");
    }

    #[test]
    fn columns_order_by_group_then_position() {
        let t = recur_ui::ThemeId::from_key("default").tokens();
        let list = vec![
            issue("a", "todo", "Todo", 1., "2026-01-01T00:00:00Z"),
            issue("b", "progress", "In Review", 3., "2026-01-02T00:00:00Z"),
            issue("c", "progress", "In Progress", 2., "2026-01-03T00:00:00Z"),
            issue("d", "", "", 0., "2026-01-04T00:00:00Z"),
            issue("e", "backlog", "Backlog", 0., "2026-01-05T00:00:00Z"),
        ];
        let cols = columns(&list, &[], "updated", &t);
        let labels: Vec<&str> = cols.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["In Review", "In Progress", "Todo", "Backlog"]);
        assert_eq!(cols[2].items.len(), 2, "an empty group counts as todo");
        assert_eq!(cols[2].items[0].title, "d", "most recently updated first");
        let only = columns(&list, &["backlog".into()], "updated", &t);
        assert_eq!(only.len(), 1);
    }

    #[test]
    fn priority_sorts_none_last() {
        let mut a = issue("a", "todo", "Todo", 0., "2026-01-01T00:00:00Z");
        let mut b = issue("b", "todo", "Todo", 0., "2026-01-02T00:00:00Z");
        a.priority = 0;
        b.priority = 3;
        assert_eq!(compare("priority", &a, &b), std::cmp::Ordering::Greater);
        assert_eq!(compare("title", &a, &b), std::cmp::Ordering::Less);
    }
}
