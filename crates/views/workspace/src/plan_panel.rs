//! The plan panel: Details (headline and brief, edited in place), Attachments (the plan's
//! pinned artifacts) and Todos (the living checklist plus the agents' TodoWrite lists). The
//! Observatory's plan pane header owns the tabs ([`crate::plan_head::PlanHead`]); edits ride
//! `SubmitPlan` / `UpdatePlanStep` / `UpdatePlan`.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::HashMap;

use gpui_kit::base::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
    relative,
};
use recur_agent::display::todos::{Todo, parse_todos};
use recur_content::{MarkdownClass, MarkdownView, Sheet};
use recur_domain::{Plan, PlanStep, Workspace};
use recur_engine::view::artifacts::ArtifactRef;
use recur_engine::view::chat::AgentMsg;
use recur_engine::view::loop_cockpit::FeedSess;
use recur_services::Services;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::ToastKind;
use recur_ui::{IconName, Theme, icon};
use recur_view_chat::chat::OpenArtifact;
use recur_view_chat::session_feed::SessionFeed;
use recur_view_core::mini::{MiniChange, MiniLang, MiniProps, MiniScriptEditor};
use recur_view_core::state::WorkspaceSeed;
use recur_view_core::{AppState, ViewContext, call, toast};
use recur_view_observatory::artifacts_panel::{ArtifactsPanel, ArtifactsPanelEvent};

/// `--plan-gutter`.
const GUTTER: f32 = 15.;
/// `--chrome-foot-h`.
const FOOT_H: f32 = 26.;
/// `--chat-fade` on `.plan-page`.
const FADE: f32 = 40.;

const EXECUTE_IN_CHAT: &str = "Implement the approved plan from the Plan panel — follow its steps and satisfy its definition of done.";

/// Details / Attachments / Todos.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlanTab {
    #[default]
    Details,
    Attachments,
    Todos,
}

impl PlanTab {
    pub fn key(self) -> &'static str {
        match self {
            PlanTab::Details => "details",
            PlanTab::Attachments => "attachments",
            PlanTab::Todos => "todos",
        }
    }

    pub fn from_key(k: &str) -> Option<PlanTab> {
        match k {
            "details" => Some(PlanTab::Details),
            "attachments" => Some(PlanTab::Attachments),
            "todos" => Some(PlanTab::Todos),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlanPanelEvent {
    OpenArtifact(OpenArtifact),
    /// Tab, editing or plan state the head shows changed.
    Changed,
}

/// One checklist row while editing (`StepDraft`): existing steps keep their id as the key;
/// new rows are `new-N`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StepDraft {
    pub key: String,
    pub title: String,
    pub status: String,
    pub note: String,
    pub workspace: String,
}

struct StepRow {
    area: Entity<TextareaState>,
    _sub: Subscription,
}

/// `latestTodosFromMessages`: the newest non-empty TodoWrite list in a transcript.
pub fn latest_todos_from_messages(msgs: &[AgentMsg]) -> Vec<Todo> {
    msgs.iter()
        .rev()
        .filter(|m| m.tool == "TodoWrite")
        .map(|m| parse_todos(&m.input))
        .find(|t| !t.is_empty())
        .unwrap_or_default()
}

/// `STEP_CYCLE`: pending → doing → done → skipped → pending.
pub(crate) fn next_step_status(cur: &str) -> &'static str {
    match cur {
        "pending" => "doing",
        "doing" => "done",
        "done" => "skipped",
        "skipped" => "pending",
        _ => "doing",
    }
}

fn step_status_label(status: &str) -> &'static str {
    match status {
        "doing" => "in progress",
        "done" => "done",
        "skipped" => "skipped",
        _ => "pending",
    }
}

fn steps_sig(steps: &[PlanStep]) -> String {
    steps
        .iter()
        .map(|s| {
            format!(
                "{}\0{}\0{}\0{}\0{}",
                s.id, s.title, s.status, s.note, s.workspace
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn drafts_from(steps: &[PlanStep]) -> Vec<StepDraft> {
    steps
        .iter()
        .map(|s| StepDraft {
            key: s.id.clone(),
            title: s.title.clone(),
            status: s.status.clone(),
            note: s.note.clone(),
            workspace: s.workspace.clone(),
        })
        .collect()
}

/// The progress bar label (`stepsBarLabel`).
pub(crate) fn steps_bar_label(done: usize, skipped: usize, doing: usize, total: usize) -> String {
    let mut parts = vec![format!("{done} done")];
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
    }
    if doing > 0 {
        parts.push(format!("{doing} in progress"));
    }
    format!("{} of {total}", parts.join(", "))
}

fn hsla(c: recur_ui::Color) -> Hsla {
    c.into()
}

pub struct PlanPanel {
    state: Entity<AppState>,
    services: Services,
    ws_id: String,
    feed: Entity<SessionFeed>,
    pub(crate) tab: PlanTab,
    pub(crate) editing: bool,
    selected_artifact: String,
    summary: Entity<TextareaState>,
    body: Option<Entity<MiniScriptEditor>>,
    body_draft: String,
    drafts: Vec<StepDraft>,
    rows: HashMap<String, StepRow>,
    last_ws: String,
    last_sig: String,
    step_focus: String,
    step_seq: u64,
    steps_saving: bool,
    steps_again: bool,
    save_after_steps: bool,
    markdown: Entity<MarkdownView>,
    artifacts: Entity<ArtifactsPanel>,
    body_scroll: ScrollHandle,
    todos_scroll: ScrollHandle,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PlanPanelEvent> for PlanPanel {}

#[cfg(test)]
impl PlanPanel {
    /// A checklist row's title field.
    pub(crate) fn test_step_value(&self, key: &str, cx: &gpui_kit::App) -> Option<String> {
        self.rows
            .get(key)
            .map(|r| r.area.read(cx).value().to_string())
    }

    /// The rendered plan body's text.
    /// The body editor while editing (`None` otherwise) and the draft a save submits.
    #[cfg(test)]
    pub(crate) fn test_body_editor(&self) -> (Option<Entity<MiniScriptEditor>>, String) {
        (self.body.clone(), self.body_draft.clone())
    }

    pub(crate) fn test_body_text(&self, cx: &gpui_kit::App) -> String {
        self.markdown
            .read(cx)
            .text_blocks()
            .into_iter()
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl PlanPanel {
    pub(crate) fn new(
        vcx: &ViewContext,
        ws_id: &str,
        feed: Entity<SessionFeed>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let summary = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 3)
                .placeholder("What this change does, and why")
        });
        let markdown = cx.new(|cx| {
            let mut v = MarkdownView::new("", MarkdownClass::ChatCanvas, false, cx);
            v.set_sheet(
                Sheet {
                    size: 12.5,
                    leading: 1.45,
                    ..Sheet::chat()
                },
                cx,
            );
            v
        });
        let artifacts = cx.new(|cx| {
            ArtifactsPanel::new(vcx.services.clone(), ws_id, cx).with_items(
                Vec::new(),
                "No attachments yet — pin artifacts or pasted images when you submit the plan.",
            )
        });
        let subs = vec![
            cx.observe_state(&vcx.state, Topics::ALL, |p, _, cx| {
                p.sync_drafts(cx);
                cx.notify();
            }),
            cx.guarded_observe(&feed, |_, _, cx| cx.notify()),
            cx.guarded_subscribe(&artifacts, |_, _, ev: &ArtifactsPanelEvent, cx| {
                if let ArtifactsPanelEvent::Open(a) = ev {
                    cx.emit(PlanPanelEvent::OpenArtifact(OpenArtifact {
                        id: a.id.clone(),
                        title: a.title.clone(),
                        kind: a.kind.clone(),
                        path: a.path.clone(),
                        ..Default::default()
                    }));
                }
            }),
            cx.guarded_subscribe_in(&summary, window, |p, _, ev: &InputEvent, window, cx| {
                // Enter in the headline moves to the body (Shift+Enter stays a newline).
                if let InputEvent::PressEnter { secondary, shift } = ev
                    && !secondary
                    && !shift
                    && let Some(b) = p.body.clone()
                {
                    b.update(cx, |e, cx| e.focus(window, cx));
                }
            }),
        ];
        let mut p = PlanPanel {
            state: vcx.state.clone(),
            services: vcx.services.clone(),
            ws_id: ws_id.to_string(),
            feed,
            tab: PlanTab::Details,
            editing: false,
            selected_artifact: String::new(),
            summary,
            body: None,
            body_draft: String::new(),
            drafts: Vec::new(),
            rows: HashMap::new(),
            last_ws: String::new(),
            last_sig: String::new(),
            step_focus: String::new(),
            step_seq: 0,
            steps_saving: false,
            steps_again: false,
            save_after_steps: false,
            markdown,
            artifacts,
            body_scroll: ScrollHandle::new(),
            todos_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _subs: subs,
        };
        let steps = p.plan(cx).map(|pl| pl.steps.clone()).unwrap_or_default();
        p.drafts = drafts_from(&steps);
        p.last_ws = p.ws_id.clone();
        p.last_sig = steps_sig(&steps);
        p
    }

    fn ws<'a>(&self, cx: &'a gpui_kit::App) -> Option<&'a Workspace> {
        self.state.read(cx).workspace(&self.ws_id)
    }

    fn plan<'a>(&self, cx: &'a gpui_kit::App) -> Option<&'a Plan> {
        self.ws(cx).and_then(|w| w.plan.as_ref())
    }

    pub(crate) fn set_tab(&mut self, tab: PlanTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            cx.emit(PlanPanelEvent::Changed);
            cx.notify();
        }
    }

    pub(crate) fn set_tab_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(t) = PlanTab::from_key(key) {
            self.set_tab(t, cx);
        }
    }

    pub(crate) fn set_selected_artifact(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected_artifact = id.to_string();
        self.artifacts.update(cx, |a, cx| a.set_selected(id, cx));
    }

    /// Keep the drafts in sync with the plan without clobbering a title being typed or an
    /// unsaved new row.
    fn sync_drafts(&mut self, cx: &mut Context<Self>) {
        let steps = self.plan(cx).map(|p| p.steps.clone()).unwrap_or_default();
        let sig = steps_sig(&steps);
        if self.last_ws == self.ws_id && sig == self.last_sig {
            return;
        }
        if !self.step_focus.is_empty() && self.last_ws == self.ws_id {
            return;
        }
        let keep: Vec<StepDraft> = if self.last_ws == self.ws_id {
            self.drafts
                .iter()
                .filter(|d| {
                    if !d.key.starts_with("new-") {
                        return false;
                    }
                    let t = d.title.trim();
                    t.is_empty() || !steps.iter().any(|s| s.title == t)
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        self.last_ws = self.ws_id.clone();
        self.last_sig = sig;
        let mut next = drafts_from(&steps);
        next.extend(keep);
        self.set_drafts(next, cx);
    }

    fn set_drafts(&mut self, drafts: Vec<StepDraft>, cx: &mut Context<Self>) {
        self.drafts = drafts;
        let live: Vec<String> = self.drafts.iter().map(|d| d.key.clone()).collect();
        self.rows.retain(|k, _| live.contains(k));
        cx.notify();
    }

    /// The textarea of one row, created on first sight and kept in sync with its draft.
    fn row(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        let title = self
            .drafts
            .iter()
            .find(|d| d.key == key)
            .map(|d| d.title.clone())
            .unwrap_or_default();
        if let Some(r) = self.rows.get(key) {
            if r.area.read(cx).value() != title.as_str() && self.step_focus != key {
                let area = r.area.clone();
                area.update(cx, |a, cx| a.set_value(title, window, cx));
            }
            return r.area.clone();
        }
        let area = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 1000)
                .placeholder("What needs to happen")
                .default_value(title)
        });
        style_area(&area, cx);
        let k = key.to_string();
        let sub = cx.guarded_subscribe_in(
            &area,
            window,
            move |p, area, ev: &InputEvent, window, cx| {
                match ev {
                    InputEvent::Change => {
                        let v = area.read(cx).value().to_string();
                        if let Some(d) = p.drafts.iter_mut().find(|d| d.key == k) {
                            d.title = v;
                        }
                    }
                    InputEvent::Focus => p.step_focus = k.clone(),
                    InputEvent::Blur => {
                        if p.step_focus == k {
                            p.step_focus.clear();
                        }
                        p.persist_step_titles(cx);
                    }
                    InputEvent::PressEnter { secondary, shift } => {
                        if !secondary && !shift {
                            // The textarea inserted a newline: Enter adds the next step instead.
                            let v = area.read(cx).value().replace('\n', "");
                            area.update(cx, |a, cx| a.set_value(v.clone(), window, cx));
                            if let Some(d) = p.drafts.iter_mut().find(|d| d.key == k) {
                                d.title = v;
                            }
                            p.add_step(Some(&k.clone()), window, cx);
                            p.persist_step_titles(cx);
                        }
                    }
                }
            },
        );
        self.rows.insert(
            key.to_string(),
            StepRow {
                area: area.clone(),
                _sub: sub,
            },
        );
        area
    }

    fn focus_step(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let area = self.row(key, window, cx);
        area.update(cx, |a, cx| a.focus(window, cx));
    }

    fn new_step_draft(&mut self) -> StepDraft {
        self.step_seq += 1;
        StepDraft {
            key: format!("new-{}", self.step_seq),
            title: String::new(),
            status: "pending".into(),
            note: String::new(),
            workspace: String::new(),
        }
    }

    pub(crate) fn add_step(
        &mut self,
        after: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.plan(cx).is_none() {
            toast(ToastKind::Error, "File a plan first, then add steps.", cx);
            return;
        }
        let row = self.new_step_draft();
        let key = row.key.clone();
        match after.and_then(|a| self.drafts.iter().position(|d| d.key == a)) {
            Some(i) => self.drafts.insert(i + 1, row),
            None => self.drafts.push(row),
        }
        self.focus_step(&key, window, cx);
        cx.notify();
    }

    fn remove_step(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.drafts.iter().position(|d| d.key == key) else {
            return;
        };
        self.drafts.remove(i);
        self.rows.remove(key);
        if let Some(next) = self
            .drafts
            .get(i.min(self.drafts.len().saturating_sub(1)))
            .map(|d| d.key.clone())
        {
            self.focus_step(&next, window, cx);
        } else {
            self.step_focus.clear();
        }
        if !key.starts_with("new-") {
            self.persist_step_titles(cx);
        }
        cx.notify();
    }

    fn steps_for_submit(&self) -> (Vec<String>, Vec<String>) {
        let rows: Vec<&StepDraft> = self
            .drafts
            .iter()
            .filter(|d| !d.title.trim().is_empty())
            .collect();
        (
            rows.iter().map(|d| d.title.trim().to_string()).collect(),
            rows.iter()
                .map(|d| {
                    if d.key.starts_with("new-") {
                        String::new()
                    } else {
                        d.key.clone()
                    }
                })
                .collect(),
        )
    }

    async fn refresh_workspace(
        svc: Services,
        state: Entity<AppState>,
        cx: &mut gpui_kit::AsyncApp,
    ) {
        if let Ok(fresh) = svc.workspaces.list_workspaces().await {
            state.update(cx, |s, cx| s.set_workspaces_from_list(fresh, cx));
        }
    }

    /// `persistStepTitles`: one save at a time; a save requested meanwhile runs after it.
    fn persist_step_titles(&mut self, cx: &mut Context<Self>) {
        if self.steps_saving {
            self.steps_again = true;
            return;
        }
        let Some(plan) = self.plan(cx).cloned() else {
            return;
        };
        let (titles, ids) = self.steps_for_submit();
        let unchanged = titles.len() == plan.steps.len()
            && titles.iter().zip(&plan.steps).all(|(t, s)| *t == s.title);
        if unchanged {
            return;
        }
        self.steps_saving = true;
        let (svc, state, id) = (
            self.services.clone(),
            self.state.clone(),
            self.ws_id.clone(),
        );
        cx.spawn(async move |this, cx| {
            let status = call(
                cx,
                svc.workspaces
                    .submit_plan(&id, &plan.summary, &plan.body, &titles, &ids),
                "Could not update the checklist",
            )
            .await;
            let ok = match status {
                None => false,
                Some(s) if s.starts_with("Refused") => {
                    let _ = this.update(cx, |_, cx| toast(ToastKind::Error, s, cx));
                    false
                }
                Some(_) => {
                    Self::refresh_workspace(svc.clone(), state.clone(), cx).await;
                    if plan.approved {
                        let _ = this.update(cx, |_, cx| {
                            toast(ToastKind::Success, "Checklist updated — approval reset", cx)
                        });
                    }
                    true
                }
            };
            let _ = this.update(cx, |p, cx| {
                if !ok {
                    let steps = p.plan(cx).map(|pl| pl.steps.clone()).unwrap_or_default();
                    p.set_drafts(drafts_from(&steps), cx);
                }
                p.steps_saving = false;
                if std::mem::take(&mut p.steps_again) {
                    p.persist_step_titles(cx);
                }
                if std::mem::take(&mut p.save_after_steps) {
                    p.save_plan(cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn start_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (summary, body) = match self.plan(cx) {
            Some(p) => (p.summary.clone(), p.body.clone()),
            None => (
                String::new(),
                self.ws(cx)
                    .map(|w| w.definition_of_done.clone())
                    .unwrap_or_default(),
            ),
        };
        self.summary
            .update(cx, |a, cx| a.set_value(summary, window, cx));
        style_area(&self.summary, cx);
        self.body_draft = body.clone();
        // `<MiniScriptEditor lang="md" minLines={6} fill>`.
        let state = self.state.clone();
        let editor = cx.new(|cx| {
            let props = MiniProps {
                lang: MiniLang::Md,
                placeholder: "Relevant files, approach, edge cases, definition of done".into(),
                min_lines: 6,
                fill: true,
                ..Default::default()
            };
            MiniScriptEditor::new(&state, &body, props, window, cx)
        });
        let sub = cx.guarded_subscribe(&editor, |p, _, MiniChange(text): &MiniChange, _| {
            p.body_draft = text.clone();
        });
        self._subs.push(sub);
        self.body = Some(editor);
        self.tab = PlanTab::Details;
        self.editing = true;
        let summary = self.summary.clone();
        summary.update(cx, |a, cx| a.focus(window, cx));
        cx.emit(PlanPanelEvent::Changed);
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editing = false;
        self.body = None;
        cx.emit(PlanPanelEvent::Changed);
        cx.notify();
    }

    fn save_plan(&mut self, cx: &mut Context<Self>) {
        if self.steps_saving {
            self.save_after_steps = true;
            return;
        }
        if let Some(b) = self.body.clone() {
            self.body_draft = b.read(cx).value(cx);
        }
        let summary_draft = self.summary.read(cx).value().to_string();
        let body = self.body_draft.trim().to_string();
        let summary = match summary_draft.trim() {
            "" => body
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or_default()
                .to_string(),
            s => s.to_string(),
        };
        let (titles, ids) = self.steps_for_submit();
        if summary.is_empty() {
            toast(ToastKind::Error, "The plan needs a summary.", cx);
            return;
        }
        let has_plan = self.plan(cx).is_some();
        let (svc, state, id) = (
            self.services.clone(),
            self.state.clone(),
            self.ws_id.clone(),
        );
        cx.spawn(async move |this, cx| {
            if has_plan {
                let Some(status) = call(
                    cx,
                    svc.workspaces
                        .submit_plan(&id, &summary, &body, &titles, &ids),
                    "Could not update the plan",
                )
                .await
                else {
                    return;
                };
                if status.starts_with("Refused") {
                    let _ = this.update(cx, |_, cx| toast(ToastKind::Error, status, cx));
                    return;
                }
                Self::refresh_workspace(svc.clone(), state.clone(), cx).await;
                let _ = this.update(cx, |p, cx| {
                    p.cancel_edit(cx);
                    toast(ToastKind::Success, "Plan updated — approval reset", cx);
                });
                return;
            }
            let prose = [summary, body]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            let Some(status) = call(
                cx,
                svc.workspaces.update_plan(&id, &prose),
                "Could not update the plan",
            )
            .await
            else {
                return;
            };
            if status.starts_with("Refused") {
                let _ = this.update(cx, |_, cx| toast(ToastKind::Error, status, cx));
                return;
            }
            let _ = this.update(cx, |p, cx| {
                p.state.update(cx, |s, cx| {
                    if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == id) {
                        w.definition_of_done = prose.clone();
                    }
                    cx.notify();
                });
                p.cancel_edit(cx);
                toast(
                    ToastKind::Success,
                    "Plan updated — logged as a deviation",
                    cx,
                );
            });
        })
        .detach();
    }

    fn cycle_step(&mut self, step_id: &str, cur: &str, cx: &mut Context<Self>) {
        let next = next_step_status(cur);
        let (svc, state, id, step) = (
            self.services.clone(),
            self.state.clone(),
            self.ws_id.clone(),
            step_id.to_string(),
        );
        cx.spawn(async move |this, cx| {
            let Some(status) = call(
                cx,
                svc.workspaces.update_plan_step(&id, &step, next),
                "Could not update the step",
            )
            .await
            else {
                return;
            };
            if status.starts_with("Refused") {
                let _ = this.update(cx, |_, cx| toast(ToastKind::Error, status, cx));
                return;
            }
            Self::refresh_workspace(svc, state, cx).await;
        })
        .detach();
    }

    /// The head's Approve.
    pub fn approve_plan(&mut self, cx: &mut Context<Self>) {
        let (svc, state, id) = (
            self.services.clone(),
            self.state.clone(),
            self.ws_id.clone(),
        );
        cx.spawn(async move |this, cx| {
            let Some(status) = call(
                cx,
                svc.workspaces.approve_plan(&id),
                "Could not approve the plan",
            )
            .await
            else {
                return;
            };
            if status.starts_with("Refused") {
                let _ = this.update(cx, |_, cx| toast(ToastKind::Error, status, cx));
                return;
            }
            Self::refresh_workspace(svc, state, cx).await;
            let _ = this.update(cx, |_, cx| toast(ToastKind::Success, "Plan approved", cx));
        })
        .detach();
    }

    /// The head's Run as loop: engage the autopilot with this plan.
    pub fn run_as_loop(&mut self, cx: &mut Context<Self>) {
        let goal = self
            .plan(cx)
            .map(|p| p.summary.clone())
            .filter(|s| !s.is_empty())
            .or_else(|| self.ws(cx).map(|w| w.definition_of_done.clone()))
            .unwrap_or_default();
        let (loop_id, policy_id) = {
            let s = self.state.read(cx);
            (
                recur_engine::view::loops::workspace_loop_id(self.ws(cx), s.settings.as_ref()),
                recur_engine::view::loops::workspace_policy_id(self.ws(cx), s.settings.as_ref()),
            )
        };
        let (svc, state, id) = (
            self.services.clone(),
            self.state.clone(),
            self.ws_id.clone(),
        );
        cx.spawn(async move |this, cx| {
            let res = match svc
                .workspaces
                .engage_loop_with(&id, &goal, &loop_id, &policy_id)
                .await
            {
                Ok(s) => Ok(s),
                Err(_) => svc.workspaces.engage_loop(&id, &goal).await,
            };
            let status = match res {
                Ok(s) => s,
                Err(err) => {
                    let _ = this.update(cx, |_, cx| {
                        toast(
                            ToastKind::Error,
                            format!(
                                "Could not engage the loop: {}",
                                recur_view_core::bridge_error_message(&err)
                            ),
                            cx,
                        )
                    });
                    return;
                }
            };
            if status.starts_with("Refused") {
                let _ = this.update(cx, |_, cx| toast(ToastKind::Error, status, cx));
                return;
            }
            Self::refresh_workspace(svc, state, cx).await;
            let _ = this.update(cx, |_, cx| toast(ToastKind::Success, status, cx));
        })
        .detach();
    }

    /// The head's Execute in chat: hand the plan to an Agent-mode chat.
    pub fn execute_in_chat(&mut self, cx: &mut Context<Self>) {
        let id = self.ws_id.clone();
        self.state.update(cx, |s, cx| {
            s.set_workspace_seed(
                WorkspaceSeed {
                    id,
                    text: EXECUTE_IN_CHAT.into(),
                    mode: "agent".into(),
                    ..Default::default()
                },
                cx,
            )
        });
    }

    fn on_edit_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.editing {
            return;
        }
        let k = &ev.keystroke;
        if k.key == "escape" {
            cx.stop_propagation();
            self.cancel_edit(cx);
            return;
        }
        if k.key == "enter" && (k.modifiers.platform || k.modifiers.control) {
            cx.stop_propagation();
            self.save_plan(cx);
        }
    }

    /// The Todos badge: open plan steps plus the agents' open todos.
    pub(crate) fn todos_badge(&self, cx: &gpui_kit::App) -> usize {
        let open_steps = self
            .plan(cx)
            .map(|p| {
                p.steps
                    .iter()
                    .filter(|s| s.status != "done" && s.status != "skipped")
                    .count()
            })
            .unwrap_or(0);
        let agent: usize = self
            .feed
            .read(cx)
            .sessions
            .iter()
            .map(|s| {
                latest_todos_from_messages(&s.messages)
                    .iter()
                    .filter(|t| t.status != "completed")
                    .count()
            })
            .sum();
        open_steps + agent
    }

    fn sessions(&self, cx: &gpui_kit::App) -> Vec<FeedSess> {
        self.feed.read(cx).sessions.clone()
    }
}

fn style_area(area: &Entity<TextareaState>, cx: &mut gpui_kit::App) {
    let t = Theme::of(cx).tokens;
    let caret: Hsla = if recur_ui::motion::is_frozen(cx) {
        t.text.with_alpha(0.001).into()
    } else {
        t.text.into()
    };
    area.update(cx, |s, _| {
        s.set_editor_style(gpui_kit::base::input::InputEditorStyle {
            foreground: t.text.into(),
            muted_foreground: t.faint.into(),
            background: Hsla::transparent_black(),
            border: Hsla::transparent_black(),
            selection: t.amber.fade(0.3).into(),
            caret,
            ..Default::default()
        })
    });
}

impl PlanPanel {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let page = match self.tab {
            PlanTab::Details => self.details(window, cx),
            PlanTab::Attachments => {
                let items: Vec<ArtifactRef> = self
                    .plan(cx)
                    .map(|p| {
                        p.attachments
                            .iter()
                            .map(|a| ArtifactRef {
                                id: a.id.clone(),
                                title: a.title.clone(),
                                kind: a.kind.clone(),
                                path: a.path.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.artifacts.update(cx, |a, cx| a.set_items(items, cx));
                self.artifacts.clone().into_any_element()
            }
            PlanTab::Todos => self.todos(window, cx),
        };
        div()
            .id("plan-pane")
            .debug_selector(|| "plan-pane".into())
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(hsla(t.text))
            .child(
                div()
                    .id("plan-page")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(page),
            )
    }
}

impl Render for PlanPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

impl PlanPanel {
    fn foot(&self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let t = Theme::of(cx).tokens;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .h(px(FOOT_H))
            .px(px(GUTTER))
            .border_t_1()
            .border_color(hsla(t.line))
            .bg(hsla(t.pane))
    }

    /// `.plan-edit-add`: a bare text button with an optional icon.
    fn foot_button(
        &self,
        id: &'static str,
        ic: Option<IconName>,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = Theme::of(cx).tokens;
        div()
            .id(id)
            .debug_selector(move || format!("plan-foot:{label}"))
            .flex()
            .items_center()
            .gap(px(5.))
            .h_full()
            .text_color(hsla(t.muted))
            .cursor_pointer()
            .hover(|s| s.text_color(hsla(t.text)))
            .when_some(ic, |d, ic| d.child(icon(ic).size(px(12.))))
            .child(
                text(label)
                    .size(11.)
                    .weight(500.)
                    .line_height(LineHeight::Normal)
                    .nowrap(),
            )
    }

    fn details(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let plan = self.plan(cx).cloned();
        let dod = self
            .ws(cx)
            .map(|w| w.definition_of_done.clone())
            .unwrap_or_default();
        let mut pane = div()
            .id("plan-details")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .capture_key_down(
                cx.guarded_listener(|p, ev: &KeyDownEvent, _, cx| p.on_edit_key(ev, cx)),
            );
        if self.editing {
            let headline = div()
                .flex()
                .flex_none()
                .border_b_1()
                .border_color(hsla(t.line_soft))
                .bg(hsla(t.bg))
                // `grid-template-columns: calc(14px + 2.2ch) minmax(0, 1fr)` at 10.5px.
                .child(div().flex_none().w(px(14. + 2.2 * mono_ch(10.5, window))))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .pt(px(8.))
                        .pr(px(10.))
                        .pb(px(8.))
                        .pl(px(4.))
                        .text_size(px(13.5))
                        .font_weight(gpui_kit::FontWeight(500.))
                        .line_height(relative(1.4))
                        .child(Textarea::new(&self.summary)),
                );
            let body = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .children(self.body.clone());
            pane = pane.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(headline)
                    .child(body),
            );
        } else {
            if let Some(summary) = plan
                .as_ref()
                .map(|p| p.summary.clone())
                .filter(|s| !s.is_empty())
            {
                pane = pane.child(
                    div()
                        .flex_none()
                        .w_full()
                        .px(px(GUTTER))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(hsla(t.line_soft))
                        .bg(hsla(t.bg))
                        .child(
                            text(summary)
                                .id("plan-summary")
                                .size(13.5)
                                .weight(500.)
                                .line_height(LineHeight::Scale(1.4))
                                .color(t.text)
                                .pre_wrap()
                                .break_anywhere(),
                        ),
                );
            }
            let md = match &plan {
                Some(p) if !p.body.is_empty() => Some(p.body.clone()),
                None if !dod.is_empty() => Some(dod),
                _ => None,
            };
            let mut scroll = recur_ui::widgets::scroll_area("plan-body-scroll")
                .track(&self.body_scroll)
                .scroller(|d| d.debug_selector(|| "plan-body-scroll".into()));
            if let Some(md) = md {
                self.markdown.update(cx, |m, cx| m.set_text(md, cx));
                scroll = scroll.child(
                    div()
                        .pt(px(12.))
                        .px(px(GUTTER))
                        .pb(px(14.))
                        .child(self.markdown.clone()),
                );
            } else if plan.is_none() {
                scroll = scroll.child(
                    div().px(px(GUTTER)).pt(px(14.)).child(
                        text("No plan yet — plan it in a Plan-mode chat, or let the loop’s planning turn write one.")
                            .id("plan-empty")
                            .size(12.)
                            .line_height(LineHeight::Scale(1.45))
                            .color(t.faint),
                    ),
                );
            }
            let backdrop = self.backdrop(cx);
            pane = pane.child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child({
                        let sel = recur_ui::selection::state("plan-body-selection", window, cx);
                        recur_ui::selection::scope(&sel, false, scroll, cx)
                    })
                    .children(edge_fades(&self.body_scroll, backdrop)),
            );
        }
        let foot = if self.editing {
            let save_h = recur_ui::motion::hover(
                "plan-foot-save-hover",
                std::time::Duration::ZERO,
                recur_ui::motion::Easing::EASE,
                window,
                cx,
            );
            let emerald_hi = t.emerald.mix_oklab(0.88, recur_ui::hex(0xffffff));
            let fill = t.emerald.mix(1. - save_h.k(), emerald_hi);
            self.foot(cx)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .child(recur_ui::kbd::kbd("⌘↵").size(11.).color(t.faint))
                        .child(
                            text("to save")
                                .size(11.)
                                .color(t.faint)
                                .line_height(LineHeight::Normal)
                                .nowrap(),
                        ),
                )
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h_full()
                        .child(
                            self.foot_button("plan-cancel", None, "Cancel", cx)
                                .on_click(cx.guarded_listener(|p, _, _, cx| p.cancel_edit(cx))),
                        )
                        .child(titled(
                            "plan-save-t",
                            "Save plan (⌘⏎)",
                            div()
                                .id("plan-save")
                                .debug_selector(|| "plan-save".into())
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .h(px(18.))
                                .px(px(7.))
                                .border_1()
                                .border_color(hsla(fill))
                                .rounded(px(4.))
                                .bg(hsla(fill))
                                .cursor_pointer()
                                .on_hover(save_h.listener())
                                .child(
                                    text("Save")
                                        .size(11.)
                                        .weight(550.)
                                        .line_height(LineHeight::Scale(1.))
                                        .color(recur_ui::hex(0x0A0C10)),
                                )
                                .on_click(cx.guarded_listener(|p, _, _, cx| p.save_plan(cx))),
                        )),
                )
        } else {
            self.foot(cx).child(
                self.foot_button("plan-edit", Some(IconName::Pencil), "Edit", cx)
                    .on_click(cx.guarded_listener(|p, _, window, cx| p.start_edit(window, cx))),
            )
        };
        pane.child(foot).into_any_element()
    }

    /// The panel's backdrop (what the scroll fades blend into): the side the plan pane sits on.
    fn backdrop(&self, cx: &gpui_kit::App) -> recur_ui::Color {
        let t = Theme::of(cx).tokens;
        let side = self
            .state
            .read(cx)
            .panels
            .sides
            .get(&recur_view_core::prefs::CodePanelKey::Plan)
            .copied()
            .unwrap_or_else(|| recur_view_core::prefs::CodePanelKey::Plan.default_side());
        if side == recur_view_core::prefs::PanelSide::Left {
            t.bg
        } else {
            t.pane
        }
    }

    fn step_check(&self, d: &StepDraft, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let status = d.status.as_str();
        let (border, bg, fg) = match status {
            "doing" => (t.amber, None, recur_ui::color::TRANSPARENT),
            "done" => (t.emerald, Some(t.emerald), recur_ui::hex(0x0b0e12)),
            "skipped" => (t.line, None, t.faint),
            _ => (t.line_bright, None, recur_ui::color::TRANSPARENT),
        };
        let mut b = div()
            .id(SharedString::from(format!("plan-step-check-{}", d.key)))
            .debug_selector({
                let k = d.key.clone();
                let s = d.status.clone();
                move || format!("plan-step-check:{k}:{s}")
            })
            .relative()
            .flex_none()
            .size(px(16.))
            .mt(px(8.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(hsla(fg));
        b = match bg {
            Some(c) => b.bg(hsla(c)),
            None => b,
        };
        if status == "doing" {
            // `linear-gradient(to right, var(--amber) 50%, transparent 50%)` in the padding box.
            b = b.child(
                div()
                    .absolute()
                    .top(px(1.5))
                    .left(px(1.5))
                    .bottom(px(1.5))
                    .w(px(6.5))
                    .rounded_l(px(2.5))
                    .bg(hsla(t.amber)),
            );
        }
        let group = SharedString::from(format!("plan-step-check-g-{}", d.key));
        let pending = status_is_plain(status) && !d.key.starts_with("new-");
        let hover_border = t.muted;
        b = b.group(group.clone()).child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(4.))
                .border(px(1.5))
                .border_color(hsla(border))
                .when(pending, |d| {
                    d.group_hover(group.clone(), move |s| s.border_color(hsla(hover_border)))
                }),
        );
        match status {
            "done" => {
                b = b.child(
                    icon(IconName::Check)
                        .stroke_width(3.)
                        .size(px(10.))
                        .color(hsla(fg)),
                )
            }
            "skipped" => b = b.child(icon(IconName::Minus).size(px(10.)).color(hsla(fg))),
            _ => {}
        }
        if d.key.starts_with("new-") {
            return b.into_any_element();
        }
        let id = d.key.clone();
        let label = step_status_label(status);
        titled(
            SharedString::from(format!("plan-step-check-t-{}", d.key)),
            format!("{label} — click to change"),
            b.cursor_pointer()
                .on_click(cx.guarded_listener(move |p, _, _, cx| {
                    let cur = p
                        .drafts
                        .iter()
                        .find(|d| d.key == id)
                        .map(|d| d.status.clone())
                        .unwrap_or_default();
                    p.cycle_step(&id, &cur, cx)
                })),
        )
        .into_any_element()
    }

    fn todos(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let plan = self.plan(cx).cloned();
        let steps = plan.as_ref().map(|p| p.steps.clone()).unwrap_or_default();
        let total = steps.len();
        let count = |s: &str| steps.iter().filter(|x| x.status == s).count();
        let (done, skipped, doing) = (count("done"), count("skipped"), count("doing"));
        let mut pane = div()
            .id("plan-todos")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .bg(hsla(t.bg));
        if total > 0 {
            let bar = steps_bar_label(done, skipped, doing, total);
            let share = |n: usize| n as f32 / total as f32;
            let mut track = div()
                .flex_1()
                .flex()
                .h(px(3.))
                .bg(hsla(t.line))
                .rounded(px(99.))
                .overflow_hidden();
            for (n, c) in [(done, t.emerald), (skipped, t.faint), (doing, t.amber)] {
                if n > 0 {
                    track =
                        track.child(div().flex_none().h_full().w(relative(share(n))).bg(hsla(c)));
                }
            }
            pane = pane.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.))
                    .py(px(8.))
                    .px(px(GUTTER))
                    .border_b_1()
                    .border_color(hsla(t.line_soft))
                    .bg(hsla(t.bg))
                    .child(
                        text(format!("{} of {total} done", done + skipped))
                            .size(11.)
                            .color(t.muted)
                            .tabular()
                            .line_height(LineHeight::Normal)
                            .nowrap(),
                    )
                    .child(titled(
                        "plan-prog-t",
                        bar,
                        div().id("plan-prog").flex_1().flex().child(track),
                    )),
            );
        }
        let mut scroll = div()
            .id("plan-todos-scroll")
            .track_scroll(&self.todos_scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .bg(hsla(t.bg));
        if !self.drafts.is_empty() {
            let drafts = self.drafts.clone();
            let mut list = div().flex().flex_col().py(px(4.));
            for d in &drafts {
                let area = self.row(&d.key, window, cx);
                let color = match d.status.as_str() {
                    "done" => t.muted,
                    "skipped" => t.faint,
                    _ => t.text,
                };
                let mut copy = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .debug_selector({
                                let k = d.key.clone();
                                move || format!("plan-step-title:{k}")
                            })
                            .py(px(6.))
                            .text_size(px(13.))
                            .line_height(relative(1.4))
                            .text_color(hsla(color))
                            .when(d.status == "done", |s| s.line_through())
                            .on_key_down({
                                let k = d.key.clone();
                                cx.guarded_listener(move |p, ev: &KeyDownEvent, window, cx| {
                                    let ks = &ev.keystroke;
                                    if ks.key != "backspace"
                                        || ks.modifiers.platform
                                        || ks.modifiers.control
                                        || ks.modifiers.alt
                                    {
                                        return;
                                    }
                                    let empty = p
                                        .rows
                                        .get(&k)
                                        .is_some_and(|r| r.area.read(cx).value().is_empty());
                                    if empty {
                                        cx.stop_propagation();
                                        p.remove_step(&k, window, cx);
                                    }
                                })
                            })
                            .child(Textarea::new(&area)),
                    );
                if !d.note.is_empty() {
                    copy = copy.child(
                        text(d.note.clone())
                            .size(11.)
                            .line_height(LineHeight::Scale(1.4))
                            .color(t.faint)
                            .break_anywhere(),
                    );
                }
                if !d.workspace.is_empty() {
                    let child = self.state.read(cx).workspace(&d.workspace).cloned();
                    let gone = child.as_ref().is_none_or(|w| w.archived);
                    let label = if d.status == "doing" {
                        "Running in child workspace"
                    } else if gone {
                        "Child transcript"
                    } else {
                        "Child workspace"
                    };
                    let tip = if gone {
                        "Show the child workspace transcript in the loop chat"
                    } else {
                        "Open the child workspace working this step"
                    };
                    let target = d.workspace.clone();
                    copy = copy.child(titled(
                        SharedString::from(format!("plan-step-child-t-{}", d.key)),
                        tip,
                        div()
                            .id(SharedString::from(format!("plan-step-child-{}", d.key)))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .py(px(1.))
                            .text_color(hsla(t.muted))
                            .cursor_pointer()
                            .hover(|s| s.text_color(hsla(t.text)))
                            .on_mouse_down(MouseButton::Left, |_, window, _| {
                                window.prevent_default()
                            })
                            .child(
                                icon(if gone {
                                    IconName::GitMerge
                                } else {
                                    IconName::GitBranch
                                })
                                .size(px(11.)),
                            )
                            .child(
                                text(label)
                                    .size(11.)
                                    .line_height(LineHeight::Normal)
                                    .nowrap(),
                            )
                            .on_click(cx.guarded_listener(move |p, _, _, cx| {
                                let t = target.clone();
                                p.state.update(cx, |s, cx| {
                                    if gone {
                                        s.request_child_transcript(&t, cx);
                                    } else {
                                        s.open_workspace(&t, false, cx);
                                    }
                                });
                            })),
                    ));
                }
                let k = d.key.clone();
                let rm = titled(
                    SharedString::from(format!("plan-step-rm-t-{}", d.key)),
                    "Remove step",
                    div()
                        .id(SharedString::from(format!("plan-step-rm-{}", d.key)))
                        .debug_selector({
                            let k = d.key.clone();
                            move || format!("plan-step-rm:{k}")
                        })
                        .flex_none()
                        .size(px(28.))
                        .mt(px(4.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(hsla(t.faint))
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla(t.pane_2)).text_color(hsla(t.text)))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .child(icon(IconName::Trash).size(px(14.)))
                        .on_click(cx.guarded_listener(move |p, _, window, cx| {
                            p.remove_step(&k, window, cx)
                        })),
                );
                list = list.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .py(px(2.))
                        .px(px(GUTTER))
                        .child(self.step_check(d, cx))
                        .child(copy)
                        .child(rm),
                );
            }
            scroll = scroll.child(list);
        } else {
            let msg = if plan.is_some() {
                "No checklist yet — add a step below."
            } else {
                "No checklist yet — file a plan first, then add steps here."
            };
            scroll = scroll.child(
                div().px(px(GUTTER)).pt(px(14.)).child(
                    text(msg)
                        .size(12.)
                        .line_height(LineHeight::Scale(1.45))
                        .color(t.faint),
                ),
            );
        }
        let groups: Vec<(String, String, Vec<Todo>)> = self
            .sessions(cx)
            .into_iter()
            .map(|s| {
                (
                    s.code.clone(),
                    s.name.clone(),
                    latest_todos_from_messages(&s.messages),
                )
            })
            .filter(|(_, _, todos)| !todos.is_empty())
            .collect();
        if !groups.is_empty() {
            let mut sec = div()
                .mt(px(18.))
                .pt(px(14.))
                .px(px(GUTTER))
                .border_t_1()
                .border_color(hsla(t.line))
                .child(
                    div().mb(px(8.)).child(
                        text("From agents")
                            .uppercase()
                            .family(Family::Mono)
                            .size(9.5)
                            .tracking(0.1)
                            .color(t.faint)
                            .line_height(LineHeight::Normal),
                    ),
                );
            for (code, name, todos) in groups {
                let ava = recur_view_chat::style::ag_color(&code, &t).unwrap_or(t.cyan);
                sec = sec.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .pt(px(8.))
                        .pb(px(4.))
                        .child(
                            div()
                                .flex_none()
                                .size(px(20.))
                                .rounded(px(6.))
                                .bg(hsla(ava))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    text(code.clone())
                                        .size(9.)
                                        .weight(700.)
                                        .color(recur_ui::hex(0x0A0C10)),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector({
                                    let n = if name.is_empty() {
                                        code.clone()
                                    } else {
                                        name.clone()
                                    };
                                    move || format!("plan-todo-name:{n}")
                                })
                                .flex_1()
                                .min_w_0()
                                .child(
                                    text(if name.is_empty() { code.clone() } else { name })
                                        .size(12.)
                                        .weight(600.)
                                        .line_height(LineHeight::Normal)
                                        .nowrap()
                                        .truncate(),
                                ),
                        ),
                );
                let mut ul = div().flex().flex_col().gap(px(2.)).py(px(2.)).mb(px(6.));
                for td in todos {
                    let (box_border, box_bg, fg) = match td.status.as_str() {
                        "completed" => (t.emerald, Some(t.emerald), t.muted),
                        "in_progress" => (t.amber, None, t.text),
                        _ => (t.line_bright, None, t.text),
                    };
                    let mut boxed = div()
                        .relative()
                        .flex_none()
                        .size(px(14.))
                        .mt(px(2.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center();
                    if let Some(c) = box_bg {
                        boxed = boxed.bg(hsla(c));
                    }
                    if td.status == "in_progress" {
                        boxed = boxed.child(
                            div()
                                .absolute()
                                .top(px(1.5))
                                .left(px(1.5))
                                .bottom(px(1.5))
                                .w(px(5.5))
                                .rounded_l(px(2.5))
                                .bg(hsla(t.amber)),
                        );
                    }
                    boxed = boxed.child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(4.))
                            .border(px(1.5))
                            .border_color(hsla(box_border)),
                    );
                    if td.status == "completed" {
                        boxed = boxed.child(
                            icon(IconName::Check)
                                .stroke_width(3.)
                                .size(px(10.))
                                .color(hsla(recur_ui::hex(0x0b0e12))),
                        );
                    }
                    let mut label = text(td.content.clone())
                        .size(12.5)
                        .line_height(LineHeight::Scale(1.45))
                        .color(fg);
                    if td.status == "completed" {
                        label = label.line_through(true);
                    }
                    if td.status == "in_progress" {
                        label = label.weight(500.);
                    }
                    ul = ul.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(9.))
                            .py(px(3.))
                            .px(px(2.))
                            .child(boxed)
                            .child(div().flex_1().min_w_0().child(label)),
                    );
                }
                sec = sec.child(ul);
            }
            scroll = scroll.child(sec);
        }
        pane = pane.child(scroll);
        if plan.is_some() {
            pane = pane.child(
                self.foot(cx).child(
                    self.foot_button("plan-add-step", Some(IconName::Plus), "Add step", cx)
                        .on_click(
                            cx.guarded_listener(|p, _, window, cx| p.add_step(None, window, cx)),
                        ),
                ),
            );
        }
        pane.into_any_element()
    }
}

fn status_is_plain(status: &str) -> bool {
    !matches!(status, "doing" | "done" | "skipped")
}

/// The width of `1ch` in the code font at `size`.
fn mono_ch(size: f32, window: &mut Window) -> f32 {
    let (w, _, _) = text("0")
        .family(Family::Mono)
        .size(size)
        .line_height(LineHeight::Normal)
        .measure(None, window);
    w
}

/// `.plan-body-scroll.fade-top` / `.fade-bot`: the mask as a backdrop gradient over the edges
/// that have more content past them.
fn edge_fades(scroll: &ScrollHandle, backdrop: recur_ui::Color) -> Vec<AnyElement> {
    let top = -f32::from(scroll.offset().y);
    let max = f32::from(scroll.max_offset().y).max(0.);
    let slack = 1.;
    let grad = |from: f32, to: f32| {
        recur_ui::style::gradient_fill(
            180.,
            (backdrop.with_alpha(from), 0.),
            (backdrop.with_alpha(to), 1.),
        )
    };
    let mut out = Vec::new();
    if top > slack {
        out.push(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(FADE))
                .child(grad(1., 0.))
                .into_any_element(),
        );
    }
    if max > slack && top < max - slack {
        out.push(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(FADE))
                .child(grad(0., 1.))
                .into_any_element(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_status_cycles_and_unknown_starts_doing() {
        assert_eq!(next_step_status("pending"), "doing");
        assert_eq!(next_step_status("doing"), "done");
        assert_eq!(next_step_status("done"), "skipped");
        assert_eq!(next_step_status("skipped"), "pending");
        assert_eq!(next_step_status(""), "doing");
    }

    #[test]
    fn progress_label_names_only_the_present_buckets() {
        assert_eq!(steps_bar_label(2, 0, 0, 4), "2 done of 4");
        assert_eq!(
            steps_bar_label(1, 1, 1, 5),
            "1 done, 1 skipped, 1 in progress of 5"
        );
    }

    #[test]
    fn latest_todos_skip_empty_lists() {
        let m = |input: &str| AgentMsg {
            tool: "TodoWrite".into(),
            input: input.into(),
            ..Default::default()
        };
        let msgs = vec![
            m(r#"{"todos":[{"content":"first","status":"completed"}]}"#),
            m(r#"{"todos":[]}"#),
        ];
        let got = latest_todos_from_messages(&msgs);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].content, "first");
    }

    #[test]
    fn drafts_follow_the_plan_steps() {
        let steps = vec![PlanStep {
            id: "s1".into(),
            title: "One".into(),
            status: "doing".into(),
            ..Default::default()
        }];
        let d = drafts_from(&steps);
        assert_eq!(d[0].key, "s1");
        assert_eq!(steps_sig(&steps), "s1\0One\0doing\0\0");
    }
}
