//! `ScheduledPrompts.svelte`: the schedules list. On the Schedules page (`project` is `None`)
//! it lists every project's schedules grouped by project under the filters bar; embedded in
//! project Settings → Schedules it lists one project's in a card. Rows expand to the
//! workspaces their runs created, which open, multi-select and two-click remove.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, px,
};
use recur_domain::{Project, ScheduledPrompt, Workspace};
use recur_engine::view::artifacts::format_relative_age;
use recur_ui::fonts::weight;
use recur_ui::text::{LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Select, SelectEvent, SelectOption, TriggerStyle, tab_strip_action};
use recur_ui::{Color, IconName, Theme, Tokens, icon};
use recur_view_core::format::{format_compact_count, format_medium_short_in};
use recur_view_core::modes::{
    ChatMode, parked_needs_you_by_workspace, sidebar_unread_title, workspace_can_remove,
    workspace_is_running,
};
use recur_view_core::pr::{PrLifecycle, pr_for_workspace, pr_icon_name, pr_lifecycle};
use recur_view_core::{AppState, ScheduleEditorRequest, ViewContext};

use crate::schedule_expanded::{self, ScheduleExpanded};
use crate::style::{
    bucket_head, empty, empty_desc, filter_seg, filters_bar, hsla, ibrow, ity, seg_count_btn,
};

/// `REMOVE_ARM_MS`.
const REMOVE_ARM: Duration = Duration::from_secs(3);
/// The list's poll.
const REFRESH_EVERY: Duration = Duration::from_secs(5);

/// `stateFilter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateFilter {
    All,
    Active,
    Paused,
}

/// `scheduleLabel`.
pub fn schedule_label(p: &ScheduledPrompt) -> String {
    if p.cadence == "cron" {
        return if p.cron == "@reboot" {
            "When Recur starts".into()
        } else {
            p.cron.clone()
        };
    }
    let day = match p.cadence.as_str() {
        "once" => p.date.as_str(),
        "weekdays" => "Weekdays",
        _ => "Daily",
    };
    format!("{day} at {}", p.time)
}

/// `nextOf`.
pub fn next_of(p: &ScheduledPrompt) -> String {
    if p.paused {
        return String::new();
    }
    if p.cadence == "cron" && p.cron == "@reboot" {
        return "when Recur starts".into();
    }
    if p.next_run_at != 0 {
        format_medium_short_in(p.next_run_at, &p.time_zone)
    } else {
        String::new()
    }
}

/// A status tone (`toneColor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Faint,
    Muted,
    Amber,
    Coral,
    Warn,
    Emerald,
}

impl Tone {
    fn color(self, t: &Tokens) -> Color {
        match self {
            Tone::Faint => t.faint,
            Tone::Muted => t.muted,
            Tone::Amber => t.amber,
            Tone::Coral => t.coral,
            Tone::Warn => t.warn,
            Tone::Emerald => t.emerald,
        }
    }
}

/// `statusOf`: the label, its tone and whether a run is live.
pub fn status_of(p: &ScheduledPrompt) -> (&'static str, Tone, bool) {
    if p.paused {
        return ("Paused", Tone::Faint, false);
    }
    match p.last_run.as_ref().map(|r| r.status.as_str()) {
        Some("running") => ("Running", Tone::Amber, true),
        Some("waiting") => ("Needs you", Tone::Coral, false),
        Some("failed") => ("Failed", Tone::Coral, false),
        Some("interrupted") => ("Interrupted", Tone::Coral, false),
        Some("missed") => ("Missed", Tone::Warn, false),
        Some("skipped") => ("Skipped", Tone::Muted, false),
        Some("stopped") => ("Stopped", Tone::Muted, false),
        Some("finished") => ("Finished", Tone::Emerald, false),
        _ => ("Scheduled", Tone::Muted, false),
    }
}

/// `runSnippet`: the last assistant text without tags, whitespace collapsed.
pub fn run_snippet(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '<' if !in_tag => {
                // `/<[^>]+>/`: a lone `<` with no closing `>` stays.
                in_tag = true;
                out.push('\u{0}');
            }
            '>' if in_tag => {
                in_tag = false;
                if out.ends_with('\u{0}') {
                    // `<>` is no tag.
                    out.pop();
                    out.push_str("<>");
                } else if let Some(i) = out.rfind('\u{0}') {
                    out.truncate(i);
                }
            }
            _ => out.push(c),
        }
    }
    let out = out.replace('\u{0}', "<");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The workspaces a schedule's runs created (`workspacesOf`).
pub fn workspaces_of<'a>(p: &ScheduledPrompt, all: &'a [Workspace]) -> Vec<&'a Workspace> {
    all.iter()
        .filter(|w| !w.archived && w.host_pr_id.trim().is_empty() && w.schedule_id == p.id)
        .collect()
}

fn bulk_key(schedule_id: &str) -> String {
    format!("bulk:{schedule_id}")
}

/// One group of rows (`grouped`).
struct Group {
    id: String,
    name: String,
    color: String,
    items: Vec<ScheduledPrompt>,
}

pub struct ScheduledPrompts {
    vcx: ViewContext,
    project: Option<Project>,
    schedules: Vec<ScheduledPrompt>,
    loading: bool,
    busy: bool,
    error: String,
    deleting: String,
    snippets: HashMap<String, String>,
    snippet_key: Option<Vec<(String, i64)>>,
    snippet_task: Option<Task<()>>,
    selected: BTreeSet<String>,
    project_filter: Vec<String>,
    state_filter: StateFilter,
    refreshing: bool,
    seen_version: u64,
    armed_remove: Option<String>,
    arm_timer: Option<Task<()>>,
    removing: Option<String>,
    remove_risk: HashMap<String, String>,
    risk_gen: HashMap<String, u64>,
    risk_inflight: BTreeSet<String>,
    hover_row: Option<String>,
    hover_run: Option<String>,
    row_focus: HashMap<String, FocusHandle>,
    project_select: Option<Entity<Select>>,
    scroll: ScrollHandle,
    new_focus: FocusHandle,
    _poll: Task<()>,
    _subs: Vec<Subscription>,
}

impl ScheduledPrompts {
    pub fn new(
        vcx: &ViewContext,
        project: Option<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_select = project.is_none().then(|| {
            cx.new(|cx| {
                Select::new(Vec::new(), window, cx)
                    .label("Filter by project")
                    .placeholder("All projects")
                    .multiple()
                    .ghost()
                    .trigger_style(TriggerStyle {
                        value_shrink: true,
                        ..Default::default()
                    })
            })
        });
        let mut subs = vec![
            cx.observe_state(&vcx.state, Topics::ALL, |this, _, cx| this.on_state(cx)),
            cx.observe_global::<ScheduleExpanded>(|this, cx| {
                this.snippets_effect(cx);
                cx.notify();
            }),
        ];
        if let Some(sel) = &project_select {
            subs.push(cx.guarded_subscribe(sel, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Values(ids) = ev {
                    this.project_filter = ids.iter().map(|s| s.to_string()).collect();
                    cx.notify();
                }
            }));
        }
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH_EVERY).await;
                if this.update(cx, |v, cx| v.refresh(cx)).is_err() {
                    return;
                }
            }
        });
        let seen_version = vcx.state.read(cx).schedules_version;
        let mut me = ScheduledPrompts {
            vcx: vcx.clone(),
            project,
            schedules: Vec::new(),
            loading: true,
            busy: false,
            error: String::new(),
            deleting: String::new(),
            snippets: HashMap::new(),
            snippet_key: None,
            snippet_task: None,
            selected: BTreeSet::new(),
            project_filter: Vec::new(),
            state_filter: StateFilter::All,
            refreshing: false,
            seen_version,
            armed_remove: None,
            arm_timer: None,
            removing: None,
            remove_risk: HashMap::new(),
            risk_gen: HashMap::new(),
            risk_inflight: BTreeSet::new(),
            hover_row: None,
            hover_run: None,
            row_focus: HashMap::new(),
            project_select,
            scroll: ScrollHandle::new(),
            new_focus: cx.focus_handle(),
            _poll: poll,
            _subs: subs,
        };
        me.refresh(cx);
        me
    }

    /// The New schedule button's focus (where focus returns when the editor closes).
    pub fn new_focus(&self) -> &FocusHandle {
        &self.new_focus
    }

    fn state(&self) -> &Entity<AppState> {
        &self.vcx.state
    }

    /// Saves from the editor bump `schedulesVersion`; workspaces change the snippets.
    fn on_state(&mut self, cx: &mut Context<Self>) {
        let v = self.state().read(cx).schedules_version;
        if v != self.seen_version {
            self.seen_version = v;
            self.refresh(cx);
        }
        self.snippets_effect(cx);
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        let services = self.vcx.services.clone();
        let id = self
            .project
            .as_ref()
            .map(|p| p.id.clone())
            .unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let r = services.projects.list_scheduled_prompts(&id).await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(rows) => {
                        if v.project.is_none() {
                            let list = rows.clone();
                            v.vcx.state.update(cx, |s, cx| s.set_schedules(list, cx));
                        }
                        v.schedules = rows;
                    }
                    Err(e) => v.error = format!("Error: {}", e.message),
                }
                v.refreshing = false;
                v.loading = false;
                v.snippets_effect(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The `LastAssistantText` snippets of every expanded schedule's workspaces, refetched when
    /// those workspaces or their activity change.
    fn snippets_effect(&mut self, cx: &mut Context<Self>) {
        let mut key: Vec<(String, i64)> = Vec::new();
        {
            let s = self.state().read(cx);
            for p in &self.schedules {
                if !schedule_expanded::is_expanded(&p.id, cx) {
                    continue;
                }
                for w in workspaces_of(p, &s.workspaces) {
                    key.push((w.id.clone(), w.last_activity_at));
                }
            }
        }
        if self.snippet_key.as_ref() == Some(&key) {
            return;
        }
        self.snippet_key = Some(key.clone());
        let services = self.vcx.services.clone();
        self.snippet_task = Some(cx.spawn(async move |this, cx| {
            let mut rows = HashMap::new();
            for (id, _) in key {
                let text = services
                    .workspaces
                    .last_assistant_text(&id)
                    .await
                    .unwrap_or_default();
                rows.insert(id, text);
            }
            let _ = this.update(cx, |v, cx| {
                v.snippets = rows;
                cx.notify();
            });
        }));
    }

    fn visible(&self) -> Vec<ScheduledPrompt> {
        self.schedules
            .iter()
            .filter(|p| {
                (self.project_filter.is_empty() || self.project_filter.contains(&p.project_id))
                    && match self.state_filter {
                        StateFilter::All => true,
                        StateFilter::Paused => p.paused,
                        StateFilter::Active => !p.paused,
                    }
            })
            .cloned()
            .collect()
    }

    fn filtered(&self) -> bool {
        !self.project_filter.is_empty() || self.state_filter != StateFilter::All
    }

    fn grouped(&self, visible: Vec<ScheduledPrompt>, cx: &App) -> Vec<Group> {
        if let Some(p) = &self.project {
            return vec![Group {
                id: p.id.clone(),
                name: p.name.clone(),
                color: String::new(),
                items: visible,
            }];
        }
        let mut groups: Vec<Group> = Vec::new();
        for p in visible {
            match groups.iter_mut().find(|g| g.id == p.project_id) {
                Some(g) => g.items.push(p),
                None => groups.push(Group {
                    id: p.project_id.clone(),
                    name: String::new(),
                    color: String::new(),
                    items: vec![p],
                }),
            }
        }
        let s = self.state().read(cx);
        for g in groups.iter_mut() {
            match s.projects.iter().find(|x| x.id == g.id) {
                Some(pr) => {
                    g.name = pr.name.clone();
                    g.color = pr.color.clone();
                }
                None => g.name = g.id.clone(),
            }
        }
        groups
    }

    // ---- actions ----

    /// `act`: one action at a time, then a refresh; a failure shows above the list. An action
    /// that resolves to a workspace opens it (`upsertWorkspace` + `openWorkspace`).
    fn act(
        &mut self,
        cx: &mut Context<Self>,
        fut: impl std::future::Future<Output = Result<Option<Workspace>, String>> + 'static,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(ws) => {
                        if let Some(ws) = ws {
                            let id = ws.id.clone();
                            v.vcx.state.update(cx, |s, cx| {
                                s.upsert_workspace(ws, cx);
                                s.open_workspace(&id, false, cx);
                            });
                        }
                        v.refresh(cx);
                    }
                    Err(e) => v.error = e,
                }
                v.busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn edit(&mut self, p: Option<ScheduledPrompt>, cx: &mut Context<Self>) {
        self.deleting.clear();
        let project_id = self.project.as_ref().map(|p| p.id.clone());
        self.state().update(cx, |s, cx| {
            s.set_schedule_editor(
                Some(ScheduleEditorRequest {
                    project_id,
                    schedule: p,
                }),
                cx,
            )
        });
    }

    fn open_workspace(&mut self, id: String, cx: &mut Context<Self>) {
        let services = self.vcx.services.clone();
        self.act(cx, async move {
            services
                .workspaces
                .get_workspace(&id)
                .await
                .map(Some)
                .map_err(|e| format!("Error: {}", e.message))
        });
    }

    fn toggle(&mut self, id: &str, runs: usize, cx: &mut Context<Self>) {
        if runs == 0 {
            return;
        }
        if schedule_expanded::is_expanded(id, cx) {
            self.clear_picked_for_schedule(id, cx);
        }
        let services = self.vcx.services.clone();
        schedule_expanded::toggle(id, &services, cx);
    }

    fn picked_of(&self, ids: &[String]) -> Vec<String> {
        ids.iter()
            .filter(|id| self.selected.contains(*id))
            .cloned()
            .collect()
    }

    fn toggle_pick(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.selected.remove(id) {
            self.selected.insert(id.to_string());
        }
        if self
            .armed_remove
            .as_deref()
            .is_some_and(|a| a.starts_with("bulk:"))
        {
            self.clear_armed_remove();
        }
        cx.notify();
    }

    fn toggle_select_all(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let all_on = !ids.is_empty() && ids.iter().all(|id| self.selected.contains(id));
        for id in ids {
            if all_on {
                self.selected.remove(id);
            } else {
                self.selected.insert(id.clone());
            }
        }
        if self
            .armed_remove
            .as_deref()
            .is_some_and(|a| a.starts_with("bulk:"))
        {
            self.clear_armed_remove();
        }
        cx.notify();
    }

    fn clear_picked_for_schedule(&mut self, schedule_id: &str, cx: &App) {
        let ids: BTreeSet<String> = self
            .state()
            .read(cx)
            .workspaces
            .iter()
            .filter(|w| w.schedule_id == schedule_id)
            .map(|w| w.id.clone())
            .collect();
        self.selected.retain(|id| !ids.contains(id));
    }

    fn clear_armed_remove(&mut self) {
        self.arm_timer = None;
        self.armed_remove = None;
    }

    fn restart_arm_timer(&mut self, id: String, cx: &mut Context<Self>) {
        self.arm_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REMOVE_ARM).await;
            let _ = this.update(cx, |v, cx| {
                v.arm_timer = None;
                if v.armed_remove.as_deref() == Some(id.as_str()) {
                    v.armed_remove = None;
                    cx.notify();
                }
            });
        }));
    }

    /// `fetchRemoveRisk`: the discard risk for a confirm title, once per workspace unless
    /// `force` (arming refetches it).
    fn fetch_remove_risk(&mut self, id: &str, force: bool, cx: &mut Context<Self>) {
        if !force && (self.risk_inflight.contains(id) || self.remove_risk.contains_key(id)) {
            return;
        }
        let gen_ = self.risk_gen.get(id).copied().unwrap_or(0) + 1;
        self.risk_gen.insert(id.to_string(), gen_);
        self.risk_inflight.insert(id.to_string());
        let services = self.vcx.services.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let r = services.workspaces.workspace_discard_risk(&id).await;
            let _ = this.update(cx, |v, cx| {
                if v.risk_gen.get(&id) == Some(&gen_) {
                    v.risk_inflight.remove(&id);
                    match r {
                        Ok(risk) => {
                            v.remove_risk.insert(id.clone(), risk);
                        }
                        Err(_) => {
                            v.remove_risk.remove(&id);
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn remove_confirm_title(&self, ids: &[String]) -> String {
        let risk = ids
            .iter()
            .filter_map(|id| self.remove_risk.get(id))
            .find(|r| !r.is_empty());
        match risk {
            Some(r) => format!("Confirm remove — {r}"),
            None => "Confirm remove".into(),
        }
    }

    /// `onRemoveRun`: the first click arms, the next confirms.
    fn on_remove_run(&mut self, id: &str, can: bool, cx: &mut Context<Self>) {
        if !can || self.removing.is_some() {
            return;
        }
        if self.armed_remove.as_deref() != Some(id) {
            self.armed_remove = Some(id.to_string());
            self.restart_arm_timer(id.to_string(), cx);
            self.fetch_remove_risk(id, true, cx);
            cx.notify();
            return;
        }
        self.clear_armed_remove();
        self.removing = Some(id.to_string());
        self.remove_risk.remove(id);
        let id = id.to_string();
        let task = self.state().update(cx, |s, cx| s.remove_workspace(&id, cx));
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |v, cx| {
                v.selected.remove(&id);
                if v.removing.as_deref() == Some(id.as_str()) {
                    v.removing = None;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// `onRemoveBulk`.
    fn on_remove_bulk(&mut self, schedule_id: &str, ids: Vec<String>, cx: &mut Context<Self>) {
        if ids.is_empty() || self.removing.is_some() {
            return;
        }
        let key = bulk_key(schedule_id);
        if self.armed_remove.as_deref() != Some(key.as_str()) {
            self.armed_remove = Some(key.clone());
            self.restart_arm_timer(key, cx);
            for id in &ids {
                self.fetch_remove_risk(id, true, cx);
            }
            cx.notify();
            return;
        }
        self.clear_armed_remove();
        self.removing = Some(key.clone());
        for id in &ids {
            self.remove_risk.remove(id);
        }
        let task = self
            .state()
            .update(cx, |s, cx| s.remove_workspaces(&ids, cx));
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |v, cx| {
                for id in &ids {
                    v.selected.remove(id);
                }
                if v.removing.as_deref() == Some(key.as_str()) {
                    v.removing = None;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_run_workspace(&mut self, id: String, cx: &mut Context<Self>) {
        self.clear_armed_remove();
        self.open_workspace(id, cx);
    }
}

// ---- render ----

/// `.sc-acts .tb-icon-btn`: 24px, faint until hover (`--text` on `--pane-2`, or `hover_fg`
/// for `.sc-del`); disabled at .4. `confirm` is `.sc-confirm` (coral on `--coral-d`).
fn act_btn(
    id: SharedString,
    disabled: bool,
    hover_fg: Option<Color>,
    confirm: bool,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    let (fg, bg) = if confirm {
        (t.coral, Some(t.coral_d))
    } else {
        (t.faint, None)
    };
    let hover_fg = if confirm {
        t.coral
    } else {
        hover_fg.unwrap_or(t.text)
    };
    let hover_bg = bg.unwrap_or(t.pane_2);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(1.))
        .h(px(24.))
        .px(px(6.))
        .rounded(px(6.))
        .text_color(hsla(fg))
        .when_some(bg, |d, b| d.bg(hsla(b)))
        .map(|d| {
            if disabled {
                d.opacity(0.4).cursor_default()
            } else {
                d.cursor_pointer()
                    .hover(move |s| s.bg(hsla(hover_bg)).text_color(hsla(hover_fg)))
            }
        })
}

/// `.ibs > * + *::before`: `·` 8px either side in `--line-bright`.
fn dot_sep(t: &Tokens) -> Span {
    span("·")
        .color(t.line_bright)
        .boxed(recur_ui::text::InlineBox {
            margin_left: 8.,
            margin_right: 8.,
            ..Default::default()
        })
}

/// `.ibs`: mono 10.5px `--faint`, one ellipsized line.
fn ibs(id: String, spans: Vec<Span>, t: &Tokens) -> Text {
    Text::from_spans(spans)
        .id(SharedString::from(id))
        .mono()
        .size(10.5)
        .leading(1.45)
        .color(t.faint)
        .truncate()
}

impl ScheduledPrompts {
    fn embedded(&self) -> bool {
        self.project.is_some()
    }

    fn row_focus(&mut self, id: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.row_focus
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    fn row(
        &mut self,
        p: &ScheduledPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let embedded = self.embedded();
        let mode = ChatMode::from_key(&p.mode);
        let tint = mode.tint(&t);
        let (st_label, tone, live) = status_of(p);
        let tone_c = tone.color(&t);
        let next = next_of(p);
        let runs: Vec<Workspace> = {
            let s = self.state().read(cx);
            workspaces_of(p, &s.workspaces)
                .into_iter()
                .cloned()
                .collect()
        };
        let n_runs = runs.len();
        let open = schedule_expanded::is_expanded(&p.id, cx);
        let hovered = self.hover_row.as_deref() == Some(p.id.as_str());
        let focus = self.row_focus(&p.id, cx);
        let focused = focus.is_focused(window);
        let focus_within = focus.contains_focused(window, cx);
        let ring = n_runs > 0 && recur_ui::style::is_focus_visible(&focus, window);
        let show_acts = hovered || focus_within;
        let busy = self.busy;
        let id = p.id.clone();

        // `.ity` with the mode glyph (or the live dot), the chevron on hover when expandable.
        let glyph: AnyElement = if n_runs > 0 && hovered {
            icon(IconName::ChevronDown)
                .size(px(14.))
                .stroke_width(2.4)
                .rotate(if open { 0. } else { -90. })
                .into_any_element()
        } else if live {
            titled(
                SharedString::from(format!("sc-live-{id}")),
                "Running",
                recur_view_widgets::chrome::running_dot(window, cx),
            )
            .into_any_element()
        } else {
            icon(mode.icon()).size(px(14.)).into_any_element()
        };
        let tile = ity(tint).child(glyph);

        // `.ibt .sc-title`.
        let title_hover = window.use_keyed_state(
            SharedString::from(format!("sc-title-h-{id}")),
            cx,
            |_, _| false,
        );
        let th = title_hover.clone();
        let over = *title_hover.read(cx);
        let edit_p = p.clone();
        let title = div()
            .id(SharedString::from(format!("sc-title-{id}")))
            .debug_selector({
                let n = p.name.clone();
                move || format!("sc-title:{n}")
            })
            .cursor_pointer()
            .on_hover(move |h, _, cx| {
                th.update(cx, |v, cx| {
                    *v = *h;
                    cx.notify();
                })
            })
            .on_click(cx.guarded_listener(move |v, _, _, cx| {
                cx.stop_propagation();
                v.edit(Some(edit_p.clone()), cx);
            }))
            .child(
                text(p.name.clone())
                    .size(13.)
                    .leading(1.45)
                    .color(t.text)
                    .underline(over),
            );

        let line1 = vec![
            span(schedule_label(p)).color(t.muted),
            dot_sep(&t),
            span(p.time_zone.clone()),
        ];
        let mut line2 = vec![span(mode.label()), dot_sep(&t), span(p.base_branch.clone())];
        let meta = if embedded {
            div()
                .min_w_0()
                .child(
                    div()
                        .mt(px(2.))
                        .child(ibs(format!("sc-ibs1-{id}"), line1, &t)),
                )
                .child(
                    div()
                        .mt(px(1.))
                        .child(ibs(format!("sc-ibs2-{id}"), line2, &t)),
                )
        } else {
            let mut lead = vec![
                span("·")
                    .color(t.line_bright)
                    .boxed(recur_ui::text::InlineBox {
                        margin_right: 8.,
                        ..Default::default()
                    }),
            ];
            lead.append(&mut line2);
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .min_w_0()
                .child(div().mt(px(2.)).min_w_0().flex_shrink(1.).child(ibs(
                    format!("sc-ibs1-{id}"),
                    line1,
                    &t,
                )))
                .child(div().mt(px(2.)).min_w_0().flex_shrink(1.).child(ibs(
                    format!("sc-ibs2-{id}"),
                    lead,
                    &t,
                )))
        };
        let main = div()
            .flex_1()
            .min_w_0()
            .child(div().flex().child(title))
            .child(meta);

        // `.sc-side`.
        let detail = p
            .last_run
            .as_ref()
            .map(|r| r.detail.clone())
            .unwrap_or_default();
        let status = div()
            .id(SharedString::from(format!("sc-status-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("sc-status:{id}={st_label}")
            })
            .flex()
            .items_center()
            .gap(px(7.))
            .when(!embedded, |d| d.w(px(104.)))
            .child(
                div()
                    .flex_none()
                    .size(px(6.))
                    .rounded_full()
                    .bg(hsla(tone_c)),
            )
            .child(
                text(st_label)
                    .mono()
                    .size(10.5)
                    .leading(1.45)
                    .color(tone_c)
                    .nowrap(),
            );
        let status: AnyElement = if detail.is_empty() {
            status.into_any_element()
        } else {
            titled(
                SharedString::from(format!("sc-status-t-{id}")),
                detail,
                status,
            )
            .into_any_element()
        };
        let next_label = if next.is_empty() {
            String::new()
        } else {
            format!("Next {next}")
        };
        let mut side = div().flex_none().flex();
        side = if embedded {
            side.flex_col().items_end().gap(px(4.))
        } else {
            side.items_center().gap(px(16.))
        };
        side = side.child(status);
        if !(embedded && next_label.is_empty()) {
            side = side.child(
                div()
                    .flex_none()
                    .when(!embedded, |d| d.w(px(192.)))
                    .min_w_0()
                    .flex()
                    .justify_end()
                    .child(
                        text(next_label)
                            .id(SharedString::from(format!("sc-next-{id}")))
                            .mono()
                            .size(10.)
                            .leading(1.45)
                            .tabular()
                            .color(t.faint)
                            .truncate(),
                    ),
            );
        }
        if n_runs > 0 {
            let rid = id.clone();
            side = side.child(
                div()
                    .debug_selector(move || format!("sc-runs-n:{rid}={n_runs}"))
                    .child(
                        text(format!(
                            "{n_runs} {}",
                            if n_runs == 1 {
                                "workspace"
                            } else {
                                "workspaces"
                            }
                        ))
                        .size(if embedded { 10.5 } else { 11. })
                        .leading(1.45)
                        .color(t.muted)
                        .nowrap(),
                    ),
            );
        }

        // `.sc-acts`.
        let mut acts = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.))
            .when(!show_acts, |d| d.opacity(0.));
        let action =
            |label: &'static str, sid: &str| SharedString::from(format!("sc-act-{label}-{sid}"));
        if let Some(ws) = p
            .last_run
            .as_ref()
            .map(|r| r.workspace_id.clone())
            .filter(|w| !w.is_empty())
        {
            acts = acts.child(titled(
                action("open-t", &id),
                "Open last workspace",
                act_btn(action("open", &id), busy, None, false, cx)
                    .debug_selector({
                        let id = id.clone();
                        move || format!("sc-btn:Open workspace:{id}")
                    })
                    .when(!busy, |d| {
                        d.on_click(cx.guarded_listener(move |v, _, _, cx| {
                            cx.stop_propagation();
                            v.open_workspace(ws.clone(), cx);
                        }))
                    })
                    .child(icon(IconName::Box).size(px(14.))),
            ));
        }
        let run_id = id.clone();
        acts = acts.child(titled(
            action("run-t", &id),
            "Run now",
            act_btn(action("run", &id), busy, None, false, cx)
                .debug_selector({
                    let id = id.clone();
                    move || format!("sc-btn:Run now:{id}")
                })
                .when(!busy, |d| {
                    d.on_click(cx.guarded_listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        let services = v.vcx.services.clone();
                        let id = run_id.clone();
                        v.act(cx, async move {
                            services
                                .projects
                                .run_scheduled_prompt_now(&id)
                                .await
                                .map(|_| None)
                                .map_err(|e| format!("Error: {}", e.message))
                        });
                    }))
                })
                .child(icon(IconName::Zap).size(px(14.))),
        ));
        let paused = p.paused;
        let pause_label = if paused { "Resume" } else { "Pause" };
        let pause_id = id.clone();
        acts = acts.child(titled(
            action("pause-t", &id),
            pause_label,
            act_btn(action("pause", &id), busy, None, false, cx)
                .debug_selector({
                    let id = id.clone();
                    move || format!("sc-btn:{pause_label}:{id}")
                })
                .when(!busy, |d| {
                    d.on_click(cx.guarded_listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        let services = v.vcx.services.clone();
                        let id = pause_id.clone();
                        v.act(cx, async move {
                            services
                                .projects
                                .pause_scheduled_prompt(&id, !paused)
                                .await
                                .map(|_| None)
                                .map_err(|e| format!("Error: {}", e.message))
                        });
                    }))
                })
                .child(
                    icon(if paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .size(px(14.)),
                ),
        ));
        let edit_p = p.clone();
        acts = acts.child(titled(
            action("edit-t", &id),
            "Edit",
            act_btn(action("edit", &id), busy, None, false, cx)
                .debug_selector({
                    let id = id.clone();
                    move || format!("sc-btn:Edit:{id}")
                })
                .when(!busy, |d| {
                    d.on_click(cx.guarded_listener(move |v, _, _, cx| {
                        cx.stop_propagation();
                        v.edit(Some(edit_p.clone()), cx);
                    }))
                })
                .child(icon(IconName::Pencil).size(px(14.))),
        ));
        if self.deleting == p.id {
            let del_id = id.clone();
            acts = acts.child(
                act_btn(action("confirm", &id), busy, None, true, cx)
                    .debug_selector({
                        let id = id.clone();
                        move || format!("sc-btn:Confirm delete:{id}")
                    })
                    .when(!busy, |d| {
                        d.on_click(cx.guarded_listener(move |v, _, _, cx| {
                            cx.stop_propagation();
                            let services = v.vcx.services.clone();
                            let id = del_id.clone();
                            let me = cx.entity().downgrade();
                            v.act(cx, async move {
                                services
                                    .projects
                                    .delete_scheduled_prompt(&id)
                                    .await
                                    .map_err(|e| format!("Error: {}", e.message))?;
                                let _ = me;
                                Ok(None)
                            });
                            v.deleting.clear();
                        }))
                    })
                    .child(
                        text("Delete?")
                            .mono()
                            .size(10.5)
                            .line_height(LineHeight::Normal)
                            .nowrap(),
                    ),
            );
        } else {
            let del_id = id.clone();
            acts = acts.child(titled(
                action("del-t", &id),
                "Delete",
                act_btn(action("del", &id), busy, Some(t.coral), false, cx)
                    .debug_selector({
                        let id = id.clone();
                        move || format!("sc-btn:Delete:{id}")
                    })
                    .when(!busy, |d| {
                        d.on_click(cx.guarded_listener(move |v, _, _, cx| {
                            cx.stop_propagation();
                            v.deleting = del_id.clone();
                            cx.notify();
                        }))
                    })
                    .child(icon(IconName::Trash).size(px(14.))),
            ));
        }

        let name = p.name.clone();
        let tid = id.clone();
        let kid = id.clone();
        let hid = id.clone();
        let article = ibrow(SharedString::from(format!("scrow-{id}")), hovered, cx)
            .debug_selector(move || format!("article:{name}:{}", open && n_runs > 0))
            .relative()
            .when(paused, |d| d.opacity(0.62))
            .map(|d| {
                if n_runs > 0 {
                    d.cursor_pointer().track_focus(&focus)
                } else {
                    d.cursor_default()
                }
            })
            .on_hover(cx.guarded_listener(move |v, h: &bool, _, cx| {
                if *h {
                    v.hover_row = Some(hid.clone());
                } else if v.hover_row.as_deref() == Some(hid.as_str()) {
                    v.hover_row = None;
                }
                cx.notify();
            }))
            .on_click(cx.guarded_listener(move |v, _, _, cx| v.toggle(&tid, n_runs, cx)))
            .on_key_down(cx.guarded_listener(move |v, ev: &KeyDownEvent, _, cx| {
                if !focused {
                    return;
                }
                let k = ev.keystroke.key.as_str();
                if k == "enter" || k == "space" {
                    cx.stop_propagation();
                    v.toggle(&kid, n_runs, cx);
                }
            }))
            .child(tile)
            .child(main)
            .child(side)
            .child(acts)
            .when(ring, |d| {
                d.child(recur_ui::style::outline(2., -2., 0., 0., t.amber))
            });
        let mut block = div().flex().flex_col().child(article);
        if open && n_runs > 0 {
            block = block.child(self.runs(p, runs, window, cx));
        }
        block.into_any_element()
    }

    /// `.sc-runs`: the expanded schedule's workspaces.
    fn runs(
        &mut self,
        p: &ScheduledPrompt,
        runs: Vec<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let can_pick = runs.len() > 1;
        let (removable, running, prs, attention, parked) = {
            let s = self.state().read(cx);
            let removable: Vec<String> = runs
                .iter()
                .filter(|w| workspace_can_remove(Some(w), s.workspaces_running.contains(&w.id)))
                .map(|w| w.id.clone())
                .collect();
            (
                removable,
                s.workspaces_running.clone(),
                s.prs.clone(),
                s.attention.by_workspace.clone(),
                parked_needs_you_by_workspace(
                    &s.workspaces,
                    &s.user_questions,
                    &s.permission_requests,
                ),
            )
        };
        let picked = self.picked_of(&removable);
        let all_on = !removable.is_empty() && removable.iter().all(|id| self.selected.contains(id));
        let bulk_id = bulk_key(&p.id);
        let bulk_armed = self.armed_remove.as_deref() == Some(bulk_id.as_str());
        let name = p.name.clone();
        let mut list = div()
            .debug_selector(move || format!("sc-runs:{name}"))
            .flex()
            .flex_col()
            .bg(hsla(t.text.mix(0.03, recur_ui::color::TRANSPARENT)));
        if can_pick {
            let label = if picked.is_empty() {
                "Select all".to_string()
            } else {
                format!("{} selected", picked.len())
            };
            let aria = if all_on { "Deselect all" } else { "Select all" };
            let disabled = removable.is_empty();
            let all_hover = window.use_keyed_state(
                SharedString::from(format!("sc-all-h-{}", p.id)),
                cx,
                |_, _| false,
            );
            let ah = all_hover.clone();
            let all_over = *all_hover.read(cx) && !disabled;
            let rm_ids = removable.clone();
            let mut bar = div()
                .id(SharedString::from(format!("sc-runs-bar-{}", p.id)))
                .debug_selector(|| "sc-runs-bar".into())
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .py(px(7.))
                .px(px(16.))
                .border_b_1()
                .border_color(hsla(t.line_soft))
                .when(bulk_armed, |d| d.bg(hsla(t.pane_2)))
                .child(
                    div()
                        .id(SharedString::from(format!("sc-runs-all-{}", p.id)))
                        .debug_selector({
                            let l = label.clone();
                            move || format!("sc-runs-all:{aria}={l}")
                        })
                        .min_w_0()
                        .when(disabled, |d| d.opacity(0.4))
                        .when(!disabled, |d| {
                            d.cursor_pointer()
                                .on_hover(move |h, _, cx| {
                                    ah.update(cx, |v, cx| {
                                        *v = *h;
                                        cx.notify();
                                    })
                                })
                                .on_click(cx.guarded_listener(move |v, _, _, cx| {
                                    v.toggle_select_all(&rm_ids, cx)
                                }))
                        })
                        .child(
                            text(label)
                                .size(11.)
                                .leading(1.45)
                                .color(if all_over { t.text } else { t.muted })
                                .nowrap(),
                        ),
                );
            let mut end = div()
                .flex()
                .justify_end()
                .items_center()
                .min_w(px(80.))
                .min_h(px(18.));
            if !picked.is_empty() {
                let title = if bulk_armed {
                    self.remove_confirm_title(&picked)
                } else {
                    "Remove".into()
                };
                let sid = p.id.clone();
                let ids = picked.clone();
                end = end.child(titled(
                    SharedString::from(format!("sc-bulk-rm-t-{}", p.id)),
                    title.clone(),
                    self.rm_button(
                        SharedString::from(format!("sc-bulk-rm-{}", p.id)),
                        bulk_armed,
                        false,
                        &t,
                    )
                    .debug_selector(move || format!("sc-bulk-rm:{title}"))
                    .on_click(cx.guarded_listener(move |v, _, _, cx| {
                        v.on_remove_bulk(&sid, ids.clone(), cx)
                    })),
                ));
            }
            bar = bar.child(end);
            let picked_hover = picked.clone();
            let bid = bulk_id.clone();
            bar = bar.on_hover(cx.guarded_listener(move |v, h: &bool, _, cx| {
                if *h {
                    for id in &picked_hover {
                        v.fetch_remove_risk(id, false, cx);
                    }
                } else if v.armed_remove.as_deref() == Some(bid.as_str()) {
                    v.clear_armed_remove();
                    cx.notify();
                }
            }));
            list = list.child(bar);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let latest = p
            .last_run
            .as_ref()
            .map(|r| r.workspace_id.clone())
            .unwrap_or_default();
        for w in runs {
            let wid = w.id.clone();
            let label = if w.short.is_empty() {
                w.goal.clone()
            } else {
                w.short.clone()
            };
            let age = format_relative_age(w.last_activity_at, now);
            let snippet = run_snippet(self.snippets.get(&w.id).map(String::as_str).unwrap_or(""));
            let has_diff = w.diff.add != 0 || w.diff.del != 0;
            let ws_pr = pr_for_workspace(&prs, &w.id).cloned();
            let life = pr_lifecycle(ws_pr.as_ref())
                .or((w.state.as_str() == "merged").then_some(PrLifecycle::Merged));
            let is_running = running.contains(&w.id);
            let can_rm = workspace_can_remove(Some(&w), is_running);
            let rm_armed = self.armed_remove.as_deref() == Some(w.id.as_str());
            let unseen = attention.get(&w.id).copied().unwrap_or(0).max(0) as usize;
            let parked_n = parked.get(&w.id).copied().unwrap_or(0);
            let unread = unseen + parked_n;
            let is_picked = self.selected.contains(&w.id);
            let hovered = self.hover_run.as_deref() == Some(w.id.as_str());
            let lit = hovered || rm_armed;

            let kind = |window: &mut Window, cx: &App| -> AnyElement {
                if workspace_is_running(&w, &running) {
                    titled(
                        SharedString::from(format!("sc-run-sd-{}", w.id)),
                        "Agent running",
                        recur_view_widgets::chrome::running_dot(window, cx),
                    )
                    .into_any_element()
                } else if let Some(l) = life {
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
                    icon(name).size(px(14.)).color(hsla(c)).into_any_element()
                } else {
                    icon(IconName::GitBranch).size(px(14.)).into_any_element()
                }
            };
            let ico_slot = |el: AnyElement| {
                div()
                    .flex_none()
                    .size(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(hsla(t.faint))
                    .child(el)
            };

            let mut row = div()
                .id(SharedString::from(format!("sc-run-{wid}")))
                .debug_selector({
                    let wid = wid.clone();
                    move || format!("sc-run:{wid}")
                })
                .w_full()
                .flex()
                .items_center()
                .py(px(7.))
                .px(px(16.))
                .border_b_1()
                .border_color(hsla(t.line_soft))
                .when(can_pick, |d| d.gap(px(12.)))
                .map(|d| {
                    if lit {
                        d.bg(hsla(t.pane_2))
                    } else if is_picked {
                        d.bg(hsla(t.text.mix(0.05, recur_ui::color::TRANSPARENT)))
                    } else {
                        d
                    }
                })
                .on_hover(cx.guarded_listener({
                    let wid = wid.clone();
                    move |v, h: &bool, _, cx| {
                        if *h {
                            v.hover_run = Some(wid.clone());
                            if can_rm {
                                v.fetch_remove_risk(&wid, false, cx);
                            }
                        } else {
                            if v.hover_run.as_deref() == Some(wid.as_str()) {
                                v.hover_run = None;
                            }
                            if v.armed_remove.as_deref() == Some(wid.as_str()) {
                                v.clear_armed_remove();
                            }
                        }
                        cx.notify();
                    }
                }));
            if can_pick {
                let k = kind(window, cx);
                if can_rm {
                    let pick_hover = window.use_keyed_state(
                        SharedString::from(format!("sc-pick-h-{wid}")),
                        cx,
                        |_, _| false,
                    );
                    let ph = pick_hover.clone();
                    let show_check = is_picked || *pick_hover.read(cx);
                    let aria = format!("{} {label}", if is_picked { "Deselect" } else { "Select" });
                    let pid = wid.clone();
                    row = row.child(
                        div().w(px(24.)).flex_none().flex().child(
                            div()
                                .id(SharedString::from(format!("sc-pick-{wid}")))
                                .debug_selector(move || format!("sc-pick:{aria}:{is_picked}"))
                                .cursor_pointer()
                                .text_color(hsla(if show_check { t.text } else { t.faint }))
                                .on_hover(move |h, _, cx| {
                                    ph.update(cx, |v, cx| {
                                        *v = *h;
                                        cx.notify();
                                    })
                                })
                                .on_click(
                                    cx.guarded_listener(move |v, _, _, cx| v.toggle_pick(&pid, cx)),
                                )
                                .child(ico_slot(if show_check {
                                    icon(IconName::SquareCheck).size(px(14.)).into_any_element()
                                } else {
                                    k
                                })),
                        ),
                    );
                } else {
                    row = row.child(div().w(px(24.)).flex_none().flex().child(ico_slot(k)));
                }
            }

            let mut title_spans = div().flex().items_center().gap(px(8.)).min_w_0().child(
                div().min_w_0().child(
                    text(label.clone())
                        .size(13.)
                        .weight(weight::SEMIBOLD)
                        .leading(1.45)
                        .color(t.text)
                        .truncate(),
                ),
            );
            if w.id == latest {
                title_spans = title_spans.child(
                    text("latest")
                        .mono()
                        .size(10.5)
                        .leading(1.45)
                        .color(t.faint)
                        .nowrap(),
                );
            }
            if unread > 0 {
                title_spans = title_spans.child(titled(
                    SharedString::from(format!("sc-unread-{wid}")),
                    sidebar_unread_title(unseen, parked_n),
                    crate::style::unread_chip(unread, cx),
                ));
            }
            let mut main = div().flex_1().min_w_0().child(title_spans);
            if !snippet.is_empty() {
                let sel = format!("sc-run-now:{wid}={snippet}");
                main = main.child(
                    div().debug_selector(move || sel).mt(px(1.)).child(
                        text(snippet)
                            .id(SharedString::from(format!("sc-run-now-{wid}")))
                            .size(12.)
                            .leading(1.45)
                            .color(t.muted)
                            .truncate(),
                    ),
                );
            }
            let diff = div().flex_none().flex().justify_end().when(has_diff, |d| {
                d.child(
                    Text::from_spans(vec![
                        span(format!("+{}", format_compact_count(w.diff.add as f64)))
                            .color(t.git_a),
                        span(" "),
                        span(format!("−{}", format_compact_count(w.diff.del as f64)))
                            .color(t.git_d),
                    ])
                    .mono()
                    .size(10.5)
                    .leading(1.45)
                    .nowrap(),
                )
            });
            let oid = wid.clone();
            let mut open_btn = div()
                .id(SharedString::from(format!("sc-run-open-{wid}")))
                .debug_selector({
                    let l = label.clone();
                    move || format!("sc-run-open:{l}")
                })
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(12.))
                .cursor_pointer()
                .on_click(
                    cx.guarded_listener(move |v, ev: &gpui_kit::ClickEvent, _, cx| {
                        let m = ev.modifiers();
                        if can_pick && can_rm && (m.platform || m.control) {
                            v.toggle_pick(&oid, cx);
                            return;
                        }
                        v.open_run_workspace(oid.clone(), cx);
                    }),
                );
            if !can_pick {
                let k = kind(window, cx);
                open_btn = open_btn.child(div().w(px(24.)).flex_none().flex().child(ico_slot(k)));
            }
            open_btn = open_btn.child(main).child(diff);
            row = row.child(open_btn);

            let rm_title = if can_rm {
                if rm_armed {
                    self.remove_confirm_title(std::slice::from_ref(&w.id))
                } else {
                    "Remove".into()
                }
            } else {
                "Stop the workspace first".into()
            };
            let rid = wid.clone();
            let when = age
                .as_ref()
                .map(|a| a.rel.clone())
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| w.elapsed_label.clone());
            let full = age.map(|a| a.full).unwrap_or_default();
            let end: AnyElement =
                if lit {
                    let rt = rm_title.clone();
                    titled(
                        SharedString::from(format!("sc-run-rm-t-{wid}")),
                        rm_title,
                        self.rm_button(
                            SharedString::from(format!("sc-run-rm-{wid}")),
                            rm_armed,
                            !can_rm,
                            &t,
                        )
                        .debug_selector(move || format!("sc-run-rm:{rt}"))
                        .when(can_rm, |d| {
                            d.on_click(cx.guarded_listener(move |v, _, _, cx| {
                                v.on_remove_run(&rid, can_rm, cx)
                            }))
                        }),
                    )
                    .into_any_element()
                } else {
                    let tx = text(when)
                        .id(SharedString::from(format!("sc-run-when-{wid}")))
                        .mono()
                        .size(10.5)
                        .leading(1.45)
                        .tabular()
                        .color(t.faint)
                        .nowrap();
                    if full.is_empty() {
                        tx.into_any_element()
                    } else {
                        titled(SharedString::from(format!("sc-run-when-t-{wid}")), full, tx)
                            .into_any_element()
                    }
                };
            row = row.child(
                div()
                    .flex_none()
                    .flex()
                    .justify_end()
                    .items_center()
                    .min_w(px(80.))
                    .min_h(px(18.))
                    .child(end),
            );
            list = list.child(row);
        }
        list.into_any_element()
    }

    /// `.sc-run-rm`: 18px, trash 12px; armed coral with dark ink; `.off` at .4.
    fn rm_button(
        &self,
        id: SharedString,
        armed: bool,
        off: bool,
        t: &Tokens,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = *t;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .size(px(18.))
            .rounded(px(4.))
            .map(|d| {
                if armed {
                    d.bg(hsla(t.coral))
                        .text_color(hsla(recur_ui::hex(0x0a0c10)))
                } else {
                    d.text_color(hsla(t.faint))
                }
            })
            .when(off, |d| d.opacity(0.4).cursor_default())
            .when(!off && !armed, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.bg(hsla(t.pane_2)).text_color(hsla(t.text)))
            })
            .when(!off && armed, |d| d.cursor_pointer())
            .child(icon(IconName::Trash).size(px(12.)))
    }

    /// The loading skeleton, the filtered-out state or the first-run state.
    fn empty_state(
        &self,
        visible: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        if self.loading {
            return Some(
                recur_ui::style::skeleton_row(cx)
                    .child(
                        recur_ui::style::skeleton(window, cx)
                            .size(px(30.))
                            .rounded(px(8.)),
                    )
                    .child(recur_ui::style::skeleton(window, cx).flex_1().h(px(14.)))
                    .into_any_element(),
            );
        }
        if self.filtered() && visible == 0 {
            let clear = crate::style::clear_button("sc-clear", cx).on_click(cx.guarded_listener(
                |v, _, _, cx| {
                    v.project_filter.clear();
                    v.state_filter = StateFilter::All;
                    if let Some(sel) = &v.project_select {
                        sel.update(cx, |s, cx| s.set_values(Vec::new(), cx));
                    }
                    cx.notify();
                },
            ));
            return Some(
                empty(
                    Some(IconName::Filter),
                    "Nothing matches these filters",
                    Some(clear.into_any_element()),
                    None,
                    cx,
                )
                .into_any_element(),
            );
        }
        if self.schedules.is_empty() {
            let no_projects = self.project.is_none() && self.state().read(cx).projects.is_empty();
            let disabled = self.busy || no_projects;
            let ring = recur_ui::style::is_focus_visible(&self.new_focus, window);
            let btn = recur_ui::set::tbtn("sc-empty-new", disabled, cx)
                .debug_selector(|| "sc-new".into())
                .relative()
                .track_focus(&self.new_focus)
                .when(!disabled, |d| {
                    d.on_click(cx.guarded_listener(|v, _, _, cx| v.edit(None, cx)))
                })
                .child(recur_ui::set::tbtn_icon(IconName::Plus))
                .child(recur_ui::style::button_label(
                    "New schedule",
                    12.,
                    weight::MEDIUM,
                ))
                .when(ring, |d| d.child(recur_ui::style::focus_ring(6., 1., cx)));
            return Some(
                empty(
                    Some(IconName::Clock),
                    "No schedules yet",
                    Some(empty_desc(
                        "Run a prompt in a fresh workspace on a cron schedule or once at a set time. Recur must be open and your Mac awake; missed runs wait for you.",
                        cx,
                    )),
                    Some(btn.into_any_element()),
                    cx,
                )
                .into_any_element(),
            );
        }
        let _ = t;
        None
    }

    fn list_error(&self, cx: &App) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        (!self.error.is_empty()).then(|| {
            div()
                .debug_selector(|| "sc-listerr".into())
                .py(px(8.))
                .px(px(16.))
                .border_b_1()
                .border_color(hsla(t.line_soft))
                .child(
                    text(self.error.clone())
                        .id("sc-listerr")
                        .size(12.)
                        .leading(1.45)
                        .color(t.coral),
                )
                .into_any_element()
        })
    }

    fn filters(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let projects: Vec<SelectOption> = self
            .state()
            .read(cx)
            .projects
            .iter()
            .map(|p| SelectOption::new(p.id.clone(), p.name.clone()))
            .collect();
        let none = projects.is_empty();
        let disabled = self.busy || none;
        let mut bar = filters_bar(cx).child(titled(
            "sc-new-t",
            "New schedule",
            tab_strip_action("sc-new", false, disabled, cx)
                .debug_selector(|| "sc-new-bar".into())
                .when(!disabled, |d| {
                    d.text_color(hsla(t.muted))
                        .on_click(cx.guarded_listener(|v, _, _, cx| v.edit(None, cx)))
                })
                .child(icon(IconName::Plus).size(px(14.))),
        ));
        bar = bar.child(div().flex_1());
        if projects.len() > 1
            && let Some(sel) = &self.project_select
        {
            sel.update(cx, |s, cx| s.set_options(projects, cx));
            bar = bar.child(sel.clone());
        }
        let active = self.schedules.iter().filter(|s| !s.paused).count();
        let paused = self.schedules.len() - active;
        let f = self.state_filter;
        let mut seg = filter_seg(cx).child(
            seg_count_btn("sc-seg-all", "All", None, f == StateFilter::All, cx).on_click(
                cx.guarded_listener(|v, _, _, cx| {
                    v.state_filter = StateFilter::All;
                    cx.notify();
                }),
            ),
        );
        for (label, which, n) in [
            ("Active", StateFilter::Active, active),
            ("Paused", StateFilter::Paused, paused),
        ] {
            seg = seg.child(
                seg_count_btn(
                    SharedString::from(format!("sc-seg-{label}")),
                    label,
                    (n > 0).then_some(n),
                    f == which,
                    cx,
                )
                .on_click(cx.guarded_listener(move |v, _, _, cx| {
                    v.state_filter = if v.state_filter == which {
                        StateFilter::All
                    } else {
                        which
                    };
                    cx.notify();
                })),
            );
        }
        bar.child(seg).into_any_element()
    }
}

impl ScheduledPrompts {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        self.snippets_effect(cx);
        let visible = self.visible();
        let n_visible = visible.len();
        let empty_el = self.empty_state(n_visible, window, cx);
        if self.embedded() {
            let mut card = recur_ui::style::card(t.pane, t.line, 9.)
                .debug_selector(|| "sc-embedded".into())
                .overflow_hidden()
                .flex()
                .flex_col();
            if let Some(e) = empty_el {
                card = card.child(e);
            }
            for p in &visible {
                card = card.child(self.row(p, window, cx));
            }
            if !self.schedules.is_empty() {
                let disabled = self.busy || self.loading;
                let ring = recur_ui::style::is_focus_visible(&self.new_focus, window);
                card = card.child(
                    div().py(px(10.)).px(px(16.)).flex().child(
                        recur_ui::set::tbtn("sc-foot-new", disabled, cx)
                            .debug_selector(|| "sc-new".into())
                            .relative()
                            .track_focus(&self.new_focus)
                            .when(!disabled, |d| {
                                d.on_click(cx.guarded_listener(|v, _, _, cx| v.edit(None, cx)))
                            })
                            .child(recur_ui::set::tbtn_icon(IconName::Plus))
                            .child(recur_ui::style::button_label(
                                "New schedule",
                                12.,
                                weight::MEDIUM,
                            ))
                            .when(ring, |d| d.child(recur_ui::style::focus_ring(6., 1., cx))),
                    ),
                );
            }
            if let Some(e) = self.list_error(cx) {
                card = card.child(e);
            }
            return card.into_any_element();
        }
        let filters = self.filters(cx);
        let mut overview = div().flex().flex_col().pb(px(32.));
        if let Some(e) = self.list_error(cx) {
            overview = overview.child(e);
        }
        if let Some(e) = empty_el {
            overview = overview.child(e);
        }
        for g in self.grouped(visible, cx) {
            let dot = if g.color.is_empty() {
                t.muted
            } else {
                Theme::of(cx).resolve_css_color(&g.color).unwrap_or(t.muted)
            };
            let mut section = div().flex().flex_col().child(bucket_head(
                &g.name,
                &g.name,
                g.items.len(),
                dot,
                cx,
            ));
            for p in &g.items {
                section = section.child(self.row(p, window, cx));
            }
            overview = overview.child(section);
        }
        div()
            .id("v-schedules")
            .debug_selector(|| "v-schedules".into())
            .size_full()
            .flex()
            .bg(hsla(t.bg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(filters)
                    .child(
                        div()
                            .debug_selector(|| "briefing-scroll".into())
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(
                                recur_ui::widgets::scroll_area("sc-scroll")
                                    .track(&self.scroll)
                                    .child(overview),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ScheduledPrompts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The Schedules route: App.svelte mounts the list only while `$view === 'schedules'`, so
/// each visit starts a fresh list (and its poll).
pub struct SchedulesPage {
    vcx: ViewContext,
    list: Option<Entity<ScheduledPrompts>>,
    _subs: Vec<Subscription>,
}

impl SchedulesPage {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs =
            vec![
                cx.observe_state_in(&vcx.state, Topics::ALL, window, |this, _, window, cx| {
                    this.sync(window, cx)
                }),
            ];
        let mut me = SchedulesPage {
            vcx: vcx.clone(),
            list: None,
            _subs: subs,
        };
        me.sync(window, cx);
        me
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on = self.vcx.state.read(cx).view == recur_view_core::ViewName::Schedules;
        if on && self.list.is_none() {
            let vcx = self.vcx.clone();
            self.list = Some(cx.new(|cx| ScheduledPrompts::new(&vcx, None, window, cx)));
            cx.notify();
        } else if !on && self.list.is_some() {
            self.list = None;
            cx.notify();
        }
    }

    pub fn list(&self) -> Option<&Entity<ScheduledPrompts>> {
        self.list.as_ref()
    }
}

impl SchedulesPage {
    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement + use<> {
        div().size_full().flex().children(self.list.clone())
    }
}

impl Render for SchedulesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(cadence: &str, cron: &str) -> ScheduledPrompt {
        ScheduledPrompt {
            cadence: cadence.into(),
            cron: cron.into(),
            date: "2026-11-02".into(),
            time: "09:00".into(),
            time_zone: "UTC".into(),
            ..Default::default()
        }
    }

    #[test]
    fn labels_follow_the_cadence() {
        assert_eq!(schedule_label(&sp("cron", "0 9 * * *")), "0 9 * * *");
        assert_eq!(schedule_label(&sp("cron", "@reboot")), "When Recur starts");
        assert_eq!(schedule_label(&sp("once", "")), "2026-11-02 at 09:00");
        assert_eq!(schedule_label(&sp("weekdays", "")), "Weekdays at 09:00");
        assert_eq!(schedule_label(&sp("daily", "")), "Daily at 09:00");
    }

    #[test]
    fn next_and_status() {
        let mut p = sp("cron", "@reboot");
        assert_eq!(next_of(&p), "when Recur starts");
        p.paused = true;
        assert_eq!(next_of(&p), "");
        assert_eq!(status_of(&p), ("Paused", Tone::Faint, false));
        p.paused = false;
        assert_eq!(status_of(&p).0, "Scheduled");
        p.last_run = Some(recur_domain::ScheduledPromptRun {
            status: "running".into(),
            ..Default::default()
        });
        assert_eq!(status_of(&p), ("Running", Tone::Amber, true));
        let mut q = sp("cron", "0 9 * * *");
        q.next_run_at = 1_792_595_040_000;
        assert_eq!(next_of(&q), "Oct 21, 2026 at 3:04 PM");
    }

    #[test]
    fn snippets_drop_tags_and_collapse_space() {
        assert_eq!(
            run_snippet("<b>All</b>  tests\n passed"),
            "All tests passed"
        );
        assert_eq!(run_snippet("a < b"), "a < b");
        assert_eq!(run_snippet(""), "");
    }
}
