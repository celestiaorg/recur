//! `FindingsPanel.svelte`: the reviewer findings master–detail (the dense list above, the
//! inspector with body, notes and Ignore / Mark invalid / Reopen below), its resizable split
//! (`recur.findingsSplitPct`), deep-link selection and the note composer.

use recur_ui::guard::Guarded;
use std::collections::HashMap;

use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};
use recur_content::{MarkdownClass, MarkdownView, Sheet};
use recur_domain::Finding;
use recur_ui::fonts::Family;
use recur_ui::kbd::kbd;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::scroll_area;
use recur_ui::{Color, IconName, Theme, icon};
use recur_view_core::AppState;
use recur_view_review::style::{hsla, note_btn, style_textarea};
use recur_view_widgets::findings_ui::{
    finding_body_needed, finding_matches_ref, finding_sev_icon, finding_title,
};

use crate::obs::FindingsFilter;

const SPLIT_KEY: &str = "recur.findingsSplitPct";
const SPLIT_DEFAULT: i64 = 60;

/// What the panel asks its host to do (`onOpen`, `onSetStatus`, `onAddNote`, `onDeleteNote`).
#[derive(Clone, Debug, PartialEq)]
pub enum FindingsPanelEvent {
    Open(Box<Finding>),
    SetStatus(Box<Finding>, String),
    AddNote(Box<Finding>, String),
    DeleteNote(Box<Finding>, String),
}

pub struct FindingsPanel {
    state: Entity<AppState>,
    findings: Vec<Finding>,
    filter: FindingsFilter,
    busy_id: String,
    /// `expandIds` / `focusNoteId`: a deep link to select (and open the composer for).
    expand_ids: Vec<String>,
    focus_note_id: String,
    pending_scroll: Option<String>,
    selected_id: String,
    drafts: HashMap<String, Entity<TextareaState>>,
    /// Each draft as an isolated field: a key renders it, not the panel.
    draft_views: HashMap<String, recur_ui::isolated::IsolatedArea>,
    compose_for: String,
    split_pct: i64,
    split_resizing: bool,
    list_scroll: ScrollHandle,
    inspector_scroll: ScrollHandle,
    markdown: HashMap<String, Entity<MarkdownView>>,
    hovered_row: Option<String>,
    hovered_comment: Option<String>,
    pending_compose: Option<String>,
    split_bounds: crate::geometry::BoundsCell,
    _subs: Vec<Subscription>,
}

impl EventEmitter<FindingsPanelEvent> for FindingsPanel {}

fn clamp_split(n: f64) -> i64 {
    n.round().clamp(25., 75.) as i64
}

/// `statusByLine`.
pub fn status_by_line(status: &str, updated_by: &str) -> String {
    let by = match updated_by {
        "coder" => " by coder",
        "reviewer" => " by reviewer",
        "engine" => " by the loop",
        "human" => " by you",
        _ => "",
    };
    match status {
        "resolved" => format!("Resolved{by}"),
        "invalid" => format!("Marked invalid{by}"),
        "ignored" => format!("Ignored{by}"),
        _ => String::new(),
    }
}

/// `noteAuthor`.
pub fn note_author(author: &str) -> String {
    match author {
        "human" => "You".into(),
        "coder" => "Coder".into(),
        "engine" => "Loop".into(),
        "" => "Note".into(),
        a => a.into(),
    }
}

/// `locLabel`: base name with the line or range, else the line alone, else `—`.
pub fn loc_label(f: &Finding) -> String {
    let name = f.file.rsplit('/').next().unwrap_or("");
    let suffix = if f.line <= 0 {
        String::new()
    } else if f.end_line > f.line {
        format!(":{}–{}", f.line, f.end_line)
    } else {
        format!(":{}", f.line)
    };
    if !name.is_empty() {
        return format!("{name}{suffix}");
    }
    let bare = suffix.trim_start_matches(':');
    if bare.is_empty() {
        "—".into()
    } else {
        bare.into()
    }
}

fn status_of(f: &Finding) -> &str {
    if f.status.is_empty() {
        "open"
    } else {
        &f.status
    }
}

/// `statusWhen`: compact age and the full stamp.
fn status_when(iso: &str) -> Option<(String, String)> {
    if iso.is_empty() {
        return None;
    }
    let ms = chrono::DateTime::parse_from_rfc3339(iso)
        .ok()?
        .timestamp_millis();
    let now = chrono::Utc::now().timestamp_millis();
    let a = recur_engine::view::artifacts::format_relative_age(ms, now)?;
    Some((a.rel, a.full))
}

impl FindingsPanel {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let split = state
            .read(cx)
            .storage()
            .get(SPLIT_KEY)
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|n| n.is_finite() && (25. ..=75.).contains(n))
            .map(|n| n.round() as i64)
            .unwrap_or(SPLIT_DEFAULT);
        FindingsPanel {
            state,
            findings: Vec::new(),
            filter: FindingsFilter::Open,
            busy_id: String::new(),
            expand_ids: Vec::new(),
            focus_note_id: String::new(),
            pending_scroll: None,
            selected_id: String::new(),
            drafts: HashMap::new(),
            draft_views: HashMap::new(),
            compose_for: String::new(),
            split_pct: split,
            split_resizing: false,
            list_scroll: ScrollHandle::new(),
            inspector_scroll: ScrollHandle::new(),
            markdown: HashMap::new(),
            hovered_row: None,
            hovered_comment: None,
            pending_compose: None,
            split_bounds: Default::default(),
            _subs: Vec::new(),
        }
    }

    /// The host's props (`findings`, `filter`, `busyId`).
    pub fn set_props(
        &mut self,
        findings: &[Finding],
        filter: FindingsFilter,
        busy_id: &str,
        cx: &mut Context<Self>,
    ) {
        let changed = self.findings.as_slice() != findings
            || self.filter != filter
            || self.busy_id != busy_id;
        if !changed {
            return;
        }
        self.findings = findings.to_vec();
        self.filter = filter;
        self.busy_id = busy_id.to_string();
        self.keep_selection();
        cx.notify();
    }

    /// Deep link (`expandIds`, `focusNoteId`): select the row, scroll it in, maybe compose.
    pub fn expand(&mut self, ids: Vec<String>, focus_note: Option<String>, cx: &mut Context<Self>) {
        self.expand_ids = ids;
        self.focus_note_id = focus_note.unwrap_or_default();
        self.keep_selection();
        if let Some(id) = self.expand_ids.iter().find(|s| !s.is_empty()).cloned() {
            self.pending_scroll = Some(id);
        }
        if !self.focus_note_id.is_empty() {
            let id = self.focus_note_id.clone();
            self.pending_scroll = Some(id.clone());
            self.pending_compose = Some(id);
        }
        // The host clears its props after one tick so the same id can deep-link again.
        self.expand_ids.clear();
        self.focus_note_id.clear();
        cx.notify();
    }

    pub fn selected_id(&self) -> &str {
        &self.selected_id
    }

    fn visible(&self) -> Vec<&Finding> {
        match self.filter {
            FindingsFilter::All => self.findings.iter().collect(),
            f => self
                .findings
                .iter()
                .filter(|x| status_of(x) == f.key())
                .collect(),
        }
    }

    fn resolve_id(&self, r: &str) -> String {
        self.findings
            .iter()
            .find(|f| finding_matches_ref(f, r))
            .map_or_else(|| r.to_string(), |f| f.id.clone())
    }

    /// Keep the selection inside the filtered list (a deep link wins over the default).
    fn keep_selection(&mut self) {
        let vis: Vec<String> = self.visible().iter().map(|f| f.id.clone()).collect();
        let jump = self
            .expand_ids
            .iter()
            .find(|s| !s.is_empty())
            .cloned()
            .or_else(|| (!self.focus_note_id.is_empty()).then(|| self.focus_note_id.clone()));
        let jump_row = jump.and_then(|j| {
            self.visible()
                .into_iter()
                .find(|f| finding_matches_ref(f, &j))
                .map(|f| f.id.clone())
        });
        if let Some(id) = jump_row {
            self.selected_id = id;
        } else if !self.selected_id.is_empty() && !vis.contains(&self.selected_id) {
            self.selected_id = vis.first().cloned().unwrap_or_default();
        } else if self.selected_id.is_empty()
            && let Some(first) = vis.first()
        {
            self.selected_id = first.clone();
        }
        if !self.compose_for.is_empty() && !vis.contains(&self.compose_for) {
            self.compose_for.clear();
        }
    }

    fn select(&mut self, f: &Finding, cx: &mut Context<Self>) {
        if !self.compose_for.is_empty() && self.compose_for != f.id {
            let id = self.compose_for.clone();
            self.close_compose_now(&id, cx);
        }
        self.selected_id = f.id.clone();
        if !f.file.is_empty() {
            cx.emit(FindingsPanelEvent::Open(Box::new(f.clone())));
        }
        cx.notify();
    }

    fn draft(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextareaState> {
        if let Some(d) = self.drafts.get(id) {
            return d.clone();
        }
        let area = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 6)
                .placeholder(SharedString::from("Add a note, clarification, or context…"))
        });
        style_textarea(&area, cx);
        let fid = id.to_string();
        self._subs.push(cx.guarded_subscribe_in(
            &area,
            window,
            move |p, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter {
                    secondary: true, ..
                } = ev
                {
                    p.submit_note(&fid, cx)
                }
            },
        ));
        self.draft_views.insert(
            id.to_string(),
            recur_ui::isolated::IsolatedArea::new(&area, (2, 6), cx),
        );
        self.drafts.insert(id.to_string(), area.clone());
        area
    }

    fn open_compose(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.resolve_id(id);
        self.compose_for = key.clone();
        let area = self.draft(&key, window, cx);
        area.update(cx, |a, cx| a.focus(window, cx));
        cx.notify();
    }

    fn close_compose_now(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.compose_for == id {
            self.compose_for.clear();
        }
        if let Some(a) = self.drafts.get(id).cloned()
            && let Some(w) = cx.windows().first().copied()
        {
            let _ = w.update(cx, |_, window, cx| {
                a.update(cx, |a, cx| a.set_value("", window, cx))
            });
        }
        cx.notify();
    }

    fn submit_note(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(f) = self.findings.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let body = self
            .drafts
            .get(id)
            .map(|a| a.read(cx).value().trim().to_string())
            .unwrap_or_default();
        if body.is_empty() || self.busy_id == f.id {
            return;
        }
        cx.emit(FindingsPanelEvent::AddNote(Box::new(f), body));
        self.close_compose_now(id, cx);
    }

    fn markdown(
        &mut self,
        key: String,
        body: &str,
        sheet: Sheet,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let v = self
            .markdown
            .entry(key)
            .or_insert_with(|| {
                cx.new(|cx| {
                    let mut v =
                        MarkdownView::new(body.to_string(), MarkdownClass::ChatCanvas, false, cx);
                    v.set_sheet(sheet.clone(), cx);
                    v
                })
            })
            .clone();
        v.update(cx, |v, cx| {
            if v.text().as_ref() != body {
                v.set_text(body.to_string(), cx);
            }
        });
        v.into_any_element()
    }

    fn persist_split(&mut self, cx: &mut Context<Self>) {
        let v = self.split_pct.to_string();
        self.state
            .update(cx, |s, _| s.storage_mut().set(SPLIT_KEY, v));
    }
}

/// `.finding-body` on `.chat-canvas`: 12.5px / 1.55, muted, paragraphs .55em apart.
fn body_sheet(max_width: f32) -> Sheet {
    let mut s = Sheet::chat();
    s.size = 12.5;
    s.leading = 1.55;
    s.p_margin_bottom = 0.55;
    s.p_last_child_zero = true;
    s.muted = true;
    s.max_width = Some(max_width);
    s
}

/// `.finding-comment-body`: 12.5px / 1.5, muted, paragraphs .4em apart.
fn comment_sheet() -> Sheet {
    let mut s = Sheet::chat();
    s.size = 12.5;
    s.leading = 1.5;
    s.p_margin_bottom = 0.4;
    s.p_last_child_zero = true;
    s.muted = true;
    s
}

fn sev_color(sev: &str, t: &recur_ui::Tokens) -> Color {
    match sev {
        "medium" => t.amber,
        "high" | "critical" => t.coral,
        _ => t.muted,
    }
}

fn status_color(st: &str, t: &recur_ui::Tokens) -> Color {
    match st {
        "open" => t.amber,
        "ignored" | "invalid" => t.muted,
        "resolved" => t.emerald,
        _ => t.faint,
    }
}

/// `.finding-status`: mono 10px uppercase, .03em.
fn status_label(st: &str, t: &recur_ui::Tokens) -> impl IntoElement {
    text(st.to_string())
        .mono()
        .size(10.)
        .uppercase()
        .tracking(0.03)
        .color(status_color(st, t))
        .nowrap()
}

impl FindingsPanel {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        if let Some(id) = self.pending_compose.take() {
            self.open_compose(&id, window, cx);
        }
        let root = div()
            .debug_selector(|| "findings-panel".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .h_full();
        if self.findings.is_empty() {
            return root.child(empty("No findings yet — run /review.", &t));
        }
        let visible: Vec<Finding> = self.visible().into_iter().cloned().collect();
        if visible.is_empty() {
            return root.child(empty(format!("No {} findings.", self.filter.key()), &t));
        }
        if let Some(id) = self.pending_scroll.take() {
            let rid = self.resolve_id(&id);
            if let Some(i) = visible.iter().position(|f| f.id == rid) {
                self.list_scroll.scroll_to_item(i);
            }
        }
        let filter = self.filter;
        let rows: Vec<AnyElement> = visible
            .iter()
            .map(|f| self.row(f, filter, window, cx))
            .collect();
        let pct = self.split_pct as f32 / 100.;
        let list = div()
            .debug_selector(|| "findings-list".into())
            .flex_none()
            .min_h(px(96.))
            .h(gpui_kit::relative(pct))
            .border_b_1()
            .border_color(hsla(t.line))
            .child(
                scroll_area("findings-list")
                    .track(&self.list_scroll)
                    .child(div().flex().flex_col().children(rows)),
            );
        let selected = visible.iter().find(|f| f.id == self.selected_id).cloned();
        let inspector = self.inspector(selected, window, cx);
        root.child(
            div()
                .debug_selector(|| "findings-split".into())
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(crate::geometry::probe(&self.split_bounds))
                .child(list)
                .child(self.split_drag(cx))
                .child(inspector),
        )
    }
}

impl Render for FindingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

fn empty(s: impl Into<SharedString>, t: &recur_ui::Tokens) -> AnyElement {
    div()
        .px(px(12.))
        .py(px(16.))
        .child(text(s.into()).size(12.).color(t.faint))
        .into_any_element()
}

impl FindingsPanel {
    /// The `.hsplit` between list and inspector: a 6px strip at the split line that drags in
    /// percent of the split's height.
    fn split_drag(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let dragging = self.split_resizing;
        let pct = self.split_pct;
        div()
            .id("findings-hsplit")
            .debug_selector(|| "findings-hsplit".into())
            .absolute()
            .left_0()
            .right_0()
            .top(gpui_kit::relative(pct as f32 / 100.))
            .mt(px(-3.))
            .h(px(6.))
            .cursor_row_resize()
            .when(dragging, |d| d.bg(hsla(t.amber.fade(0.55))))
            .hover(move |s| s.bg(hsla(t.amber.fade(0.55))))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.guarded_listener(|p, _, _, cx| {
                    p.split_resizing = true;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_drag((), |_, _, _, cx| cx.new(|_| gpui_kit::Empty))
            .on_drag_move(
                cx.guarded_listener(move |p, ev: &gpui_kit::DragMoveEvent<()>, _, cx| {
                    let split = p.split_bounds.get();
                    let top = f32::from(split.origin.y);
                    let height = f32::from(split.size.height);
                    if height <= 0. {
                        return;
                    }
                    let y = f32::from(ev.event.position.y);
                    p.split_pct = clamp_split(((y - top) / height * 100.) as f64);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.guarded_listener(|p, _, _, cx| {
                    if p.split_resizing {
                        p.split_resizing = false;
                        p.persist_split(cx);
                        cx.notify();
                    }
                }),
            )
            .into_any_element()
    }

    fn row(
        &mut self,
        f: &Finding,
        filter: FindingsFilter,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let status = status_of(f).to_string();
        let sel = f.id == self.selected_id;
        let busy = self.busy_id == f.id;
        let when = status_when(if f.updated_at.is_empty() {
            &f.created_at
        } else {
            &f.updated_at
        });
        let notes = f.notes.len();
        let hovered = self.hovered_row.as_deref() == Some(f.id.as_str());
        let show_status = filter == FindingsFilter::All || status != filter.key();
        let bg = if sel {
            Some(t.amber_d)
        } else if hovered {
            Some(t.pane_2)
        } else {
            None
        };
        let fid = f.id.clone();
        let ff = f.clone();
        let mut trail = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .self_start()
            .mt(px(1.));
        if notes > 0 {
            let label = if notes == 1 {
                "1 note".to_string()
            } else {
                format!("{notes} notes")
            };
            trail = trail.child(titled(
                SharedString::from(format!("fn-t-{}", f.id)),
                label,
                div()
                    .id(SharedString::from(format!("fn-{}", f.id)))
                    .flex()
                    .items_center()
                    .child(
                        icon(IconName::NotebookPen)
                            .size(px(12.))
                            .color(hsla(if sel { t.text } else { t.muted })),
                    ),
            ));
        }
        if let Some((rel, full)) = when {
            trail = trail.child(titled(
                SharedString::from(format!("fa-t-{}", f.id)),
                full,
                div().id(SharedString::from(format!("fa-{}", f.id))).child(
                    text(rel)
                        .mono()
                        .size(10.5)
                        .tabular()
                        .color(t.faint)
                        .nowrap(),
                ),
            ));
        }
        let mut sub = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(titled(
                SharedString::from(format!("fl-t-{}", f.id)),
                f.file.clone(),
                div()
                    .id(SharedString::from(format!("floc-{}", f.id)))
                    .min_w_0()
                    .child(text(loc_label(f)).mono().size(11.).color(t.cyan).truncate()),
            ));
        if show_status {
            sub = sub.child(div().flex_none().child(status_label(&status, &t)));
        }
        let main = div()
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                text(finding_title(&f.message))
                    .size(12.5)
                    .line_height(LineHeight::Scale(1.35))
                    .truncate(),
            )
            .child(sub);
        let row = div()
            .id(SharedString::from(format!("frow-{}", f.id)))
            .debug_selector({
                let id = f.id.clone();
                move || format!("finding-row:{id}")
            })
            .flex()
            .items_center()
            .gap(px(8.))
            .w_full()
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(hsla(t.line_soft))
            .cursor_pointer()
            .text_color(hsla(t.text))
            .when_some(bg, |d, c| d.bg(hsla(c)))
            .on_hover(cx.guarded_listener({
                let fid = fid.clone();
                move |p, on: &bool, _, cx| {
                    if *on {
                        p.hovered_row = Some(fid.clone());
                    } else if p.hovered_row.as_deref() == Some(fid.as_str()) {
                        p.hovered_row = None;
                    }
                    cx.notify();
                }
            }))
            .on_click(cx.guarded_listener(move |p, _, _, cx| p.select(&ff, cx)))
            .child(
                div().w(px(18.)).flex_none().flex().justify_center().child(
                    icon(finding_sev_icon(&f.severity.to_string()))
                        .size(px(14.))
                        .color(hsla(sev_color(&f.severity.to_string(), &t))),
                ),
            )
            .child(main)
            .child(trail);
        let row = if sel {
            div()
                .relative()
                .child(row)
                .child(
                    // `box-shadow: inset 2px 0 0`: inside the border, above the bottom rule.
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom(px(1.))
                        .w(px(2.))
                        .bg(hsla(t.amber)),
                )
                .into_any_element()
        } else {
            row.into_any_element()
        };
        if busy {
            div().opacity(0.7).child(row).into_any_element()
        } else {
            row
        }
    }

    fn act_btn(
        &self,
        id: String,
        label: &'static str,
        name: IconName,
        danger: bool,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = Theme::of(cx).tokens;
        let (fg, border) = if danger {
            (t.coral, t.coral.mix(0.35, t.line))
        } else {
            (t.muted, t.line)
        };
        let d = div()
            .id(SharedString::from(id))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .h(px(26.))
            .px(px(9.))
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(border))
            .bg(hsla(t.bg))
            .text_color(hsla(fg))
            .child(icon(name).size(px(13.)))
            .child(text(label).size(11.5).nowrap());
        if disabled {
            return d.opacity(0.45);
        }
        d.cursor_pointer().hover(move |s| {
            if danger {
                s.bg(hsla(t.coral.fade(0.1))).text_color(hsla(t.coral))
            } else {
                s.text_color(hsla(t.text)).border_color(hsla(t.line_bright))
            }
        })
    }

    fn inspector(
        &mut self,
        selected: Option<Finding>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let busy = selected.as_ref().is_some_and(|f| self.busy_id == f.id);
        let base = div()
            .debug_selector(|| "finding-inspector".into())
            .flex_1()
            .min_h(px(120.))
            .flex()
            .flex_col()
            .bg(hsla(t.pane));
        let Some(f) = selected else {
            return base
                .child(empty("Select a finding.", &t))
                .into_any_element();
        };
        let status = status_of(&f).to_string();
        let noting = self.compose_for == f.id;
        let show_status =
            self.filter == FindingsFilter::All || status != self.filter.key() || status != "open";
        let mut meta = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_1()
            .min_w_0()
            .child({
                // `.tc-sev.<severity>`.
                let sev = f.severity.to_string();
                recur_view_widgets::pill::sev_pill(&sev, &t)
                    .render(sev)
                    .flex_none()
            })
            .child(titled(
                SharedString::from(format!("fi-t-{}", f.id)),
                finding_title(&f.message),
                div().id("finding-inspector-title").min_w_0().child(
                    text(finding_title(&f.message))
                        .size(13.)
                        .weight(600.)
                        .line_height(LineHeight::Scale(1.3))
                        .tracking(-0.01)
                        .truncate(),
                ),
            ));
        if show_status {
            meta = meta.child(div().flex_none().child(status_label(&status, &t)));
        }
        let mut actions = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .items_center()
            .ml_auto()
            .justify_end()
            .flex_none();
        let disabled = self.busy_id == f.id;
        if status == "open" {
            let (f1, f2) = (f.clone(), f.clone());
            actions = actions
                .child(
                    self.act_btn(
                        "fa-ignore".into(),
                        "Ignore",
                        IconName::EyeOff,
                        false,
                        disabled,
                        cx,
                    )
                    .debug_selector(|| "finding-act:ignore".into())
                    .when(!disabled, |d| {
                        d.on_click(cx.guarded_listener(move |_, _, _, cx| {
                            cx.emit(FindingsPanelEvent::SetStatus(
                                Box::new(f1.clone()),
                                "ignored".into(),
                            ))
                        }))
                    }),
                )
                .child(
                    self.act_btn(
                        "fa-invalid".into(),
                        "Mark invalid",
                        IconName::BugOff,
                        true,
                        disabled,
                        cx,
                    )
                    .debug_selector(|| "finding-act:invalid".into())
                    .when(!disabled, |d| {
                        d.on_click(cx.guarded_listener(move |_, _, _, cx| {
                            cx.emit(FindingsPanelEvent::SetStatus(
                                Box::new(f2.clone()),
                                "invalid".into(),
                            ))
                        }))
                    }),
                );
        } else if status == "ignored" || status == "invalid" {
            let f1 = f.clone();
            actions = actions.child(
                self.act_btn(
                    "fa-reopen".into(),
                    "Reopen",
                    IconName::Undo,
                    false,
                    disabled,
                    cx,
                )
                .debug_selector(|| "finding-act:reopen".into())
                .when(!disabled, |d| {
                    d.on_click(cx.guarded_listener(move |_, _, _, cx| {
                        cx.emit(FindingsPanelEvent::SetStatus(
                            Box::new(f1.clone()),
                            "open".into(),
                        ))
                    }))
                }),
            );
        }
        if !noting {
            let id = f.id.clone();
            actions =
                actions.child(
                    self.act_btn(
                        "fa-note".into(),
                        "Add note",
                        IconName::NotebookPen,
                        false,
                        disabled,
                        cx,
                    )
                    .debug_selector(|| "finding-act:note".into())
                    .when(!disabled, |d| {
                        d.on_click(cx.guarded_listener(move |p, _, window, cx| {
                            p.open_compose(&id, window, cx)
                        }))
                    }),
                );
        }
        let head = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .flex_wrap()
            .min_h(px(crate::style::CHROME_H))
            .py(px(4.))
            .px(px(14.))
            .border_b_1()
            .border_color(hsla(t.line_soft))
            .child(meta)
            .child(actions);
        let ch = ch_width(window, 12.5);
        let mut body = div().flex().flex_col().gap(px(10.));
        let loc_open = f.clone();
        let has_file = !f.file.is_empty();
        let loc = div()
            .id("finding-inspector-loc")
            .debug_selector(|| "finding-inspector-loc".into())
            .self_start()
            .child(text(loc_label(&f)).mono().size(12.).color(t.cyan).nowrap())
            .when(has_file, |d| {
                d.cursor_pointer()
                    .hover(move |s| s.text_color(hsla(t.text)))
                    .on_click(cx.guarded_listener(move |_, _, _, cx| {
                        cx.emit(FindingsPanelEvent::Open(Box::new(loc_open.clone())))
                    }))
            })
            .when(!has_file, |d| d.opacity(0.7));
        let loc = if has_file {
            titled("finding-inspector-loc-t", f.file.clone(), loc).into_any_element()
        } else {
            loc.into_any_element()
        };
        body = body.child(loc);
        if finding_body_needed(&f.message) {
            let md = self.markdown(
                format!("body:{}", f.id),
                &f.message,
                body_sheet(65. * ch),
                cx,
            );
            body = body.child(md);
        }
        if !f.status_reason.is_empty() {
            body = body.child(
                recur_ui::text::Text::from_spans(vec![
                    recur_ui::text::span("Reason")
                        .mono()
                        .size(10.5)
                        .weight(600.)
                        .uppercase()
                        .tracking(0.04)
                        .color(t.faint),
                    recur_ui::text::span("\u{a0}").size(12.),
                    recur_ui::text::span(f.status_reason.clone())
                        .size(12.)
                        .color(t.muted),
                ])
                .size(12.)
                .line_height(LineHeight::Scale(1.4)),
            );
        }
        if status != "open" {
            let by = status_by_line(&status, &f.updated_by);
            if !by.is_empty() {
                let when = status_when(&f.updated_at);
                let mut spans = vec![recur_ui::text::span(by)];
                if let Some((rel, _)) = &when {
                    spans.push(recur_ui::text::span(format!(" · {rel}")));
                }
                let line = recur_ui::text::Text::from_spans(spans)
                    .mono()
                    .size(11.)
                    .color(t.faint)
                    .line_height(LineHeight::Scale(1.4));
                body = body.child(match when {
                    Some((_, full)) => {
                        titled("finding-when-t", full, div().id("finding-when").child(line))
                            .into_any_element()
                    }
                    None => line.into_any_element(),
                });
            }
        }
        if !f.notes.is_empty() {
            let mut notes = div().flex().flex_col().gap(px(8.));
            for n in f.notes.clone() {
                notes = notes.child(self.comment(&f, &n, cx));
            }
            body = body.child(notes);
        }
        let scroll = div()
            .id("finding-inspector-scroll")
            .flex_1()
            .min_h_0()
            .child({
                let area = scroll_area("finding-inspector-scroll")
                    .track(&self.inspector_scroll)
                    .child(
                        div()
                            .pt(px(12.))
                            .px(px(14.))
                            .pb(px(if noting { 8. } else { 14. }))
                            .child(body),
                    );
                let sel = recur_ui::selection::state("finding-inspector-selection", window, cx);
                recur_ui::selection::scope(&sel, false, area, cx)
            });
        let scroll = if noting && self.fade_bottom() {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(scroll)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(28.))
                        .child(recur_ui::style::gradient_fill(
                            180.,
                            (t.pane.with_alpha(0.), 0.),
                            (t.pane, 1.),
                        )),
                )
                .into_any_element()
        } else {
            scroll.into_any_element()
        };
        let mut out = base.child(head).child(scroll);
        if noting {
            out = out.child(self.compose_dock(&f, window, cx));
        }
        if busy {
            out.opacity(0.7).into_any_element()
        } else {
            out.into_any_element()
        }
    }

    fn fade_bottom(&self) -> bool {
        let b = self.inspector_scroll.bounds();
        let max = f32::from(self.inspector_scroll.max_offset().y);
        let top = -f32::from(self.inspector_scroll.offset().y);
        let _ = b;
        max > 1. && top < max - 1.
    }

    fn comment(
        &mut self,
        f: &Finding,
        n: &recur_domain::FindingNote,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let key = format!("{}:{}", f.id, n.id);
        let hovered = self.hovered_comment.as_deref() == Some(key.as_str());
        let when = status_when(&n.created_at);
        let disabled = self.busy_id == f.id;
        let mut meta = div().flex().items_center().gap(px(8.)).child(
            text(note_author(&n.author))
                .mono()
                .size(10.5)
                .weight(600.)
                .uppercase()
                .tracking(0.04)
                .color(t.faint)
                .nowrap(),
        );
        if let Some((rel, _)) = &when {
            meta = meta.child(text(rel.clone()).mono().size(11.).color(t.faint).nowrap());
        }
        if !n.id.is_empty() {
            let (ff, nid) = (f.clone(), n.id.clone());
            let del = div()
                .id(SharedString::from(format!("fdel-{key}")))
                .debug_selector({
                    let k = key.clone();
                    move || format!("finding-note-del:{k}")
                })
                .ml_auto()
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(5.))
                .text_color(hsla(t.faint))
                .child(icon(IconName::Trash).size(px(13.)))
                .when(!hovered, |d| d.opacity(0.))
                .when(disabled && hovered, |d| d.opacity(0.4))
                .when(!disabled, |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.text_color(hsla(t.coral)).bg(hsla(t.pane.fade(0.7))))
                        .on_click(cx.guarded_listener(move |_, _, _, cx| {
                            cx.emit(FindingsPanelEvent::DeleteNote(
                                Box::new(ff.clone()),
                                nid.clone(),
                            ))
                        }))
                });
            meta = meta.child(titled(
                SharedString::from(format!("fdel-t-{key}")),
                "Delete note",
                del,
            ));
        }
        let meta = match &when {
            Some((_, full)) => titled(
                SharedString::from(format!("fnm-t-{key}")),
                full.clone(),
                div()
                    .id(SharedString::from(format!("fnm-{key}")))
                    .child(meta),
            )
            .into_any_element(),
            None => meta.into_any_element(),
        };
        let md = self.markdown(format!("note:{key}"), &n.body, comment_sheet(), cx);
        let hk = key.clone();
        div()
            .id(SharedString::from(format!("fcm-{key}")))
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(hsla(if hovered { t.pane_2 } else { t.bg }))
            .on_hover(cx.guarded_listener(move |p, on: &bool, _, cx| {
                if *on {
                    p.hovered_comment = Some(hk.clone());
                } else if p.hovered_comment.as_deref() == Some(hk.as_str()) {
                    p.hovered_comment = None;
                }
                cx.notify();
            }))
            .child(meta)
            .child(md)
            .into_any_element()
    }

    fn compose_dock(
        &mut self,
        f: &Finding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let area = self.draft(&f.id, window, cx);
        let disabled = self.busy_id == f.id;
        let empty_draft = area.read(cx).value().trim().is_empty();
        let (fid1, fid2) = (f.id.clone(), f.id.clone());
        let cancel = note_btn("fnc-cancel", "Cancel", false, disabled, t.bg, 24., cx)
            .when(!disabled, |d| {
                d.on_click(cx.guarded_listener(move |p, _, _, cx| p.close_compose_now(&fid1, cx)))
            });
        let save = note_btn(
            "fnc-save",
            "Save",
            true,
            disabled || empty_draft,
            t.bg,
            24.,
            cx,
        )
        .when(!disabled && !empty_draft, |d| {
            d.on_click(cx.guarded_listener(move |p, _, _, cx| p.submit_note(&fid2, cx)))
        });
        let foot = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .pt(px(2.))
            .px(px(8.))
            .pb(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(kbd("⌘↵").size(11.).color(t.faint))
                    .child(text("to save").size(11.).color(t.faint).nowrap()),
            )
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(cancel)
                    .child(save),
            );
        let field = div()
            .min_h(px(40.))
            .max_h(px(120.))
            .pt(px(8.))
            .px(px(10.))
            .pb(px(4.))
            .text_size(px(12.5))
            .line_height(gpui_kit::relative(1.5))
            .when(disabled, |d| d.opacity(0.55))
            .when_some(self.draft_views.get(&f.id), |d, v| {
                d.child(v.element(18.75, cx))
            });
        div()
            .flex_none()
            .px(px(14.))
            .pb(px(12.))
            .bg(hsla(t.pane))
            .child(
                recur_ui::style::card(t.bg, t.line, 10.)
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(field)
                    .child(foot),
            )
            .on_key_down(cx.guarded_listener({
                let id = f.id.clone();
                move |p, e: &gpui_kit::KeyDownEvent, _, cx| {
                    if e.keystroke.key == "escape" {
                        cx.stop_propagation();
                        p.close_compose_now(&id, cx);
                    }
                }
            }))
            .into_any_element()
    }
}

/// The width of `0` in Inter at `size` (CSS `ch`).
fn ch_width(window: &mut Window, size: f32) -> f32 {
    let font = Family::Ui.font(400., false, Default::default());
    let ts = window.text_system();
    let id = ts.resolve_font(&font);
    ts.advance(id, px(size), '0')
        .map(|s| f32::from(s.width))
        .unwrap_or(size * 0.6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_lines_and_authors() {
        assert_eq!(status_by_line("resolved", "engine"), "Resolved by the loop");
        assert_eq!(status_by_line("invalid", "human"), "Marked invalid by you");
        assert_eq!(status_by_line("ignored", ""), "Ignored");
        assert_eq!(status_by_line("open", "human"), "");
        assert_eq!(note_author("human"), "You");
        assert_eq!(note_author(""), "Note");
        assert_eq!(note_author("reviewer"), "reviewer");
    }

    #[test]
    fn loc_labels() {
        let f = |file: &str, line: i64, end: i64| Finding {
            file: file.into(),
            line,
            end_line: end,
            ..Default::default()
        };
        assert_eq!(loc_label(&f("src/a.go", 3, 0)), "a.go:3");
        assert_eq!(loc_label(&f("src/a.go", 3, 7)), "a.go:3–7");
        assert_eq!(loc_label(&f("", 4, 0)), "4");
        assert_eq!(loc_label(&f("", 0, 0)), "—");
    }
}
