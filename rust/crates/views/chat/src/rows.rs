//! The step rows (`AgentChat.svelte`'s `stepRow` and the snippets it renders): tool cards with
//! their per-tool bodies, thinking and compaction cards, fallback and notice rows, folded
//! narration, and the flat skill / mode / rename / child / PR / artifact rows.

use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px, relative,
};
use recur_agent::context_usage::format_tokens;
use recur_agent::display::tasks::{
    BodyOpts, ClaudeTaskView, claude_task_chip_status, claude_task_has_body,
    claude_task_result_is_ack, format_task_id, pretty_task_status, task_status_chip_class,
};
use recur_agent::display::web::{compact_url, web_result_is_json};
use recur_agent_tools::display::briefing_tools::{
    list_workspace_briefs_summary, workspace_context_summary,
};
use recur_agent_tools::display::open_pr_tool::open_pr_label;
use recur_agent_tools::display::plan_tools::prose_preview;
use recur_content::Sheet;
use recur_content::links::external_http_url;
use recur_content::render::style::sh_color;
use recur_engine::view::artifacts::{artifact_can_open, artifact_icon, artifact_kind_tally, artifact_when};
use recur_engine::view::chat::{AgentMsg, is_task_tool, step_row_key};
use recur_engine::view::subagent::is_subagent_launch_ack;
use recur_issues::linear::tools::{LinearViewKind, linear_count_label, linear_item_url};
use recur_textmate::{DiffKind, highlight_line_spans, lang_from_path};
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::{Color, IconName, Theme, Tokens, icon, motion};

use crate::chat::{AgentChat, ChatEvent, OpenArtifact};
use crate::findings_ui::{
    finding_body_needed, finding_loc_label, finding_severity_tally, finding_title,
};
use crate::style::{
    ERR, GREEN, Pill, amber_a, coral_a, finding_sev, hsla, ico, label, sev_pill, status_pill,
};
use crate::tools::{
    StepIcon, ToolCx, ToolView, base_name, diff_counts, lint_loc_suffix, lint_sev_class,
    plan_step_mark, pretty_result, pretty_tool_name, thinking_preview,
};

/// `.tc-wait-dot`'s pulse.
const WAIT_PULSE: Duration = Duration::from_millis(1200);

/// What a card child is, for the `.a + .b` borders the stylesheet draws between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Pre,
    TaskPrompt,
    Grepq,
    Wait,
    WebPrompt,
    WebMsg,
    PlanSummary,
    PlanBody,
    PlanSteps,
    CanvasPlan,
    PlanAck,
    FindingEmpty,
    LintPaths,
    TaskChips,
    Other,
}

impl Part {
    fn is_pre(self) -> bool {
        matches!(self, Part::Pre | Part::TaskPrompt | Part::Grepq)
    }
}

/// The border the stylesheet puts on `b` after `a`: `(line color?, soft?)`.
fn joint(a: Part, b: Part) -> Option<bool> {
    use Part::*;
    let line = Some(false);
    match (a, b) {
        (x, y) if x.is_pre() && y.is_pre() => line,
        (TaskChips, Wait) | (TaskChips, Pre) => line,
        (Wait, TaskPrompt) => line,
        (Grepq, Wait) => line,
        (Wait, WebPrompt) | (WebPrompt, WebMsg) | (WebPrompt, Pre) | (Wait, WebMsg) => line,
        (PlanSummary, PlanSteps) | (PlanSummary, PlanBody) | (PlanBody, PlanSteps) => line,
        (CanvasPlan, PlanAck) | (Wait, PlanAck) => line,
        (FindingEmpty, LintPaths) => Some(true),
        (Wait, LintPaths) => line,
        _ => None,
    }
}

/// A card's children with the sibling borders applied.
pub(crate) struct Parts(pub Vec<(Part, AnyElement)>);

impl Parts {
    pub fn new() -> Self {
        Parts(Vec::new())
    }

    pub fn push(&mut self, p: Part, el: impl IntoElement) {
        self.0.push((p, el.into_any_element()));
    }

    pub fn into_children(self, t: &Tokens) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let mut prev: Option<Part> = None;
        for (p, el) in self.0 {
            let el = match prev.and_then(|a| joint(a, p)) {
                Some(soft) => {
                    let mut d = div().border_t_1().border_color(hsla(if soft { t.line_soft } else { t.line }));
                    if prev == Some(Part::TaskChips) {
                        d = d.mt(px(6.));
                    }
                    if p == Part::LintPaths && soft {
                        d = d.pt(px(0.));
                    }
                    d.child(el).into_any_element()
                }
                None => el,
            };
            out.push(el);
            prev = Some(p);
        }
        out
    }
}

/// The thought / compaction canvas (`--text: var(--muted)`).
pub(crate) fn muted_sheet() -> Sheet {
    Sheet {
        muted: true,
        ..Sheet::chat()
    }
}

/// `.tc-pre`: mono 11 / 1.55, pre-wrap, 320px scroll.
pub(crate) fn tc_pre(id: SharedString, body: Text, max_h: f32) -> Div {
    div().child(
        div()
            .id(id)
            .max_h(px(max_h))
            .overflow_y_scroll()
            .px(px(10.))
            .py(px(8.))
            .child(body),
    )
}

pub(crate) fn pre_text(id: impl Into<SharedString>, s: impl Into<SharedString>, color: Color) -> Text {
    text(s)
        .id(gpui_kit::ElementId::Name(id.into()))
        .mono()
        .size(11.)
        .line_height(LineHeight::Scale(1.55))
        .color(color)
        .pre_wrap()
        .break_anywhere()
}

impl AgentChat {
    /// `.tc-wait`: a pulsing amber dot and the in-flight copy.
    pub(crate) fn tc_wait(&self, s: impl Into<SharedString>, window: &mut Window, cx: &Context<Self>) -> Div {
        let t = Theme::of(cx).tokens;
        let (opacity, scale) = if motion::reduced(cx) {
            (0.8, 1.)
        } else {
            let e = motion::elapsed(self.anim_epoch, cx).as_secs_f32();
            let ph = (e % WAIT_PULSE.as_secs_f32()) / WAIT_PULSE.as_secs_f32();
            // ease-in-out between the 0/100% (.35, .85) and 50% (1, 1) keyframes.
            let half = if ph < 0.5 { ph * 2. } else { (1. - ph) * 2. };
            let k = recur_ui::motion::Easing::EASE_IN_OUT.apply(half);
            if !motion::is_frozen(cx) {
                window.request_animation_frame();
            }
            (0.35 + 0.65 * k, 0.85 + 0.15 * k)
        };
        let d = 7. * scale;
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(2.))
            .py(px(9.))
            .text_size(px(12.5))
            .text_color(hsla(t.muted))
            .child(
                div().flex_none().size(px(7.)).flex().items_center().justify_center().child(
                    div()
                        .size(px(d))
                        .rounded_full()
                        .bg(hsla(t.amber.fade(opacity))),
                ),
            )
            .child(text(s))
    }

    /// A settled markdown block, cached per key.
    pub(crate) fn md(
        &mut self,
        key: impl Into<String>,
        body: &str,
        sheet: Option<Sheet>,
        demote_images: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = key.into();
        self.markdown_seen.insert(key.clone());
        let entry = self.markdown.entry(key).or_insert_with(|| {
            cx.new(|cx| {
                let mut v = recur_content::MarkdownView::new(
                    body.to_string(),
                    recur_content::MarkdownClass::ChatCanvas,
                    demote_images,
                    cx,
                );
                if let Some(s) = sheet.clone() {
                    v.set_sheet(s, cx);
                }
                v
            })
        });
        let v = entry.clone();
        v.update(cx, |v, cx| {
            if v.text().as_ref() != body {
                v.set_text(body.to_string(), cx);
            }
        });
        v.into_any_element()
    }

    /// A live markdown block (`StreamingMarkdown`), cached per key.
    pub(crate) fn stream(
        &mut self,
        key: impl Into<String>,
        body: &str,
        sheet: Option<Sheet>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = key.into();
        self.markdown_seen.insert(key.clone());
        let active = self.props.shell_active && self.props.presentation_active;
        let entry = self.streams.entry(key).or_insert_with(|| {
            cx.new(|cx| {
                let mut s = recur_content::StreamingMarkdown::new(cx);
                if let Some(sh) = sheet.clone() {
                    s.set_sheet(sh, cx);
                }
                s
            })
        });
        let v = entry.clone();
        v.update(cx, |s, cx| s.set(body.to_string(), active, false, cx));
        v.into_any_element()
    }

    // ---- step rows ----

    /// `stepRow`.
    pub(crate) fn step_row(
        &mut self,
        m: &AgentMsg,
        key: &str,
        first_checkpoint: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let data = self.tool_data.clone();
        let v = ToolView::new(m, data.cx());
        if v.is_skill_activation() {
            return self.skill_row(&v, cx);
        }
        if v.is_list_skills() {
            return self.list_skills_row(&v, cx);
        }
        if v.is_switch_mode() {
            return self.switch_mode_row(&v, cx);
        }
        if v.is_rename_tool() {
            return self.rename_tool_row(&v, cx);
        }
        if v.is_child_tool() {
            return self.child_tool_row(&v, cx);
        }
        if v.is_open_pr() {
            return self.open_pr_row(&v, key, cx);
        }
        if v.is_reenter_phase() {
            return self.reenter_phase_row(&v, cx);
        }
        if v.is_create_artifact() {
            return self.create_artifact_row(&v, cx);
        }
        if v.is_present_artifact() {
            return self.present_artifact_row(&v, cx);
        }
        let m = m.clone();
        if !m.tool.is_empty() {
            return self.tool_card(&m, key, window, cx);
        }
        if m.compact {
            return self.compact_card(&m, key, cx);
        }
        if m.fallback {
            return self.fallback_card(&m, cx);
        }
        if m.thinking {
            return self.thinking_card(&m, key, first_checkpoint, cx);
        }
        if m.note {
            return self.notice_card(&m, key, cx);
        }
        self.narration_row(&m, key, cx)
    }

    /// `.skillrow`: a flat one-liner.
    fn skillrow(
        &self,
        icon_name: IconName,
        name: AnyElement,
        ellipsis: bool,
        tail: Vec<AnyElement>,
        tip: String,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let name_box = div()
            .when(ellipsis, |d| d.flex_1().min_w_0().overflow_hidden())
            .when(!ellipsis, |d| d.flex_none())
            .child(name);
        let row = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .pt(px(3.))
            .pb(px(3.))
            .pl(px(2.))
            .pr(px(6.))
            .text_size(px(12.))
            .min_w_0()
            .child(ico(icon_name, 14., t.muted))
            .child(name_box)
            .children(tail);
        if tip.is_empty() {
            row.into_any_element()
        } else {
            recur_ui::tooltip::titled(
                SharedString::from(format!("skillrow-{}", tip.len())),
                tip,
                row,
            )
            .into_any_element()
        }
    }

    fn skill_name(&self, spans: Vec<Span>, ellipsis: bool, cx: &Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tx = Text::from_spans(spans).weight(600.).color(t.text);
        if ellipsis { tx.truncate() } else { tx.nowrap() }.into_any_element()
    }

    fn skill_args(&self, s: impl Into<SharedString>, cx: &Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        div()
            .flex_shrink_0()
            .min_w_0()
            .max_w(relative(0.4))
            .child(
                Pill {
                    fg: t.amber,
                    bg: amber_a(0.08),
                    border: amber_a(0.22),
                    ..Pill::chip(&t)
                }
                .render_truncated(s),
            )
            .into_any_element()
    }

    fn skill_tag(&self, s: impl Into<SharedString>, cx: &Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        div()
            .flex_none()
            .child(label(s, Family::Mono, 10.5, 400.).color(t.faint))
            .into_any_element()
    }

    fn skill_row(&self, v: &ToolView<'_>, cx: &Context<Self>) -> AnyElement {
        let m = v.m;
        let name = v.skill_activation_name();
        let args = v
            .input()
            .and_then(|i| i.get("args"))
            .and_then(|a| a.as_str())
            .unwrap_or("")
            .to_string();
        let tag = if m.result_error {
            "Failed"
        } else if m.result_pending {
            "Activating…"
        } else {
            "Activated"
        };
        let mut tail = Vec::new();
        if !args.is_empty() {
            tail.push(self.skill_args(args, cx));
        }
        tail.push(self.skill_tag(tag, cx));
        let tip = if m.result_error {
            if m.result.is_empty() { "failed".to_string() } else { m.result.clone() }
        } else if name.is_empty() {
            "Skill".to_string()
        } else {
            name.clone()
        };
        let label = if name.is_empty() { "Unknown skill".to_string() } else { name };
        self.skillrow(
            IconName::Sparkles,
            self.skill_name(vec![span(label)], false, cx),
            false,
            tail,
            tip,
            cx,
        )
    }

    fn list_skills_row(&self, v: &ToolView<'_>, cx: &Context<Self>) -> AnyElement {
        let m = v.m;
        let name = if m.result_error {
            "List skills"
        } else if m.result_pending {
            "Listing skills"
        } else {
            "Listed skills"
        };
        let tail = if m.result_error { vec![self.skill_tag("failed", cx)] } else { Vec::new() };
        let tip = if m.result_error {
            if m.result.is_empty() { "failed".into() } else { m.result.clone() }
        } else {
            String::new()
        };
        self.skillrow(IconName::Sparkles, self.skill_name(vec![span(name)], false, cx), false, tail, tip, cx)
    }

    fn switch_mode_row(&self, v: &ToolView<'_>, cx: &Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let m = v.m;
        let tr = v.switch_mode_transition();
        let to_key = tr.as_ref().map(|t| t.1.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| v.switch_mode_target());
        let from_key = tr.as_ref().map(|t| t.0.clone()).unwrap_or_default();
        let failed = v.switch_mode_failed();
        let mode = |k: &str| recur_view_core::modes::ChatMode::from_key(k);
        let tint = |k: &str| span(mode(k).label()).weight(700.).color(mode(k).tint(&t));
        let spans = if failed {
            vec![span("Mode switch failed")]
        } else if m.result_pending {
            vec![span("Switching mode")]
        } else if !from_key.is_empty() && !to_key.is_empty() {
            vec![
                span("Switched "),
                tint(&from_key),
                span(" "),
                span("→").weight(500.).color(t.faint).boxed(recur_ui::text::InlineBox {
                    margin_left: 0.28 * 12.,
                    margin_right: 0.28 * 12.,
                    ..Default::default()
                }),
                span(" "),
                tint(&to_key),
            ]
        } else if !to_key.is_empty() {
            vec![span("Switched to "), tint(&to_key)]
        } else {
            vec![span("Switched mode")]
        };
        let tail = if failed { vec![self.skill_tag("failed", cx)] } else { Vec::new() };
        let tip = if failed {
            if m.result.is_empty() { "failed".into() } else { m.result.clone() }
        } else {
            String::new()
        };
        self.skillrow(IconName::Switch3, self.skill_name(spans, false, cx), false, tail, tip, cx)
    }

    fn rename_tool_row(&self, v: &ToolView<'_>, cx: &Context<Self>) -> AnyElement {
        let m = v.m;
        let title = v.rename_tool_value();
        let noun = v.rename_tool_noun();
        let failed = v.rename_tool_failed();
        let bold = |s: &str| span(s.to_string()).weight(700.);
        let spans = if failed {
            vec![span("Rename failed")]
        } else if m.result_pending {
            if title.is_empty() {
                vec![span(format!("Renaming {noun}"))]
            } else {
                vec![span(format!("Renaming {noun} to ")), bold(&title)]
            }
        } else if !title.is_empty() {
            vec![span(format!("Renamed {noun} to ")), bold(&title)]
        } else {
            vec![span(format!("Renamed {noun}"))]
        };
        let tail = if failed { vec![self.skill_tag("failed", cx)] } else { Vec::new() };
        let tip = if failed {
            if m.result.is_empty() { "failed".into() } else { m.result.clone() }
        } else {
            title.clone()
        };
        let icon = if v.is_rename_branch() { IconName::GitBranch } else { IconName::ALargeSmall };
        self.skillrow(icon, self.skill_name(spans, true, cx), true, tail, tip, cx)
    }

    fn reenter_phase_row(&self, v: &ToolView<'_>, cx: &Context<Self>) -> AnyElement {
        let m = v.m;
        let phase = v.reenter_phase_id();
        let failed = m.result_error
            || recur_agent_tools::display::reenter_phase_tool::reenter_phase_failed(&m.result, m.result_error);
        let name = if failed {
            "Re-enter failed"
        } else if m.result_pending {
            "Re-entering"
        } else {
            "Re-entered"
        };
        let mut tail = Vec::new();
        if !phase.is_empty() {
            tail.push(self.skill_args(phase.clone(), cx));
        }
        if failed {
            tail.push(self.skill_tag("failed", cx));
        }
        let tip = if failed {
            if m.result.is_empty() { "failed".into() } else { m.result.clone() }
        } else {
            phase
        };
        self.skillrow(IconName::RefreshCw, self.skill_name(vec![span(name)], false, cx), false, tail, tip, cx)
    }

    fn child_tool_row(&self, v: &ToolView<'_>, cx: &Context<Self>) -> AnyElement {
        let m = v.m;
        let failed = v.child_tool_failed();
        let note = v.child_tool_note();
        let mut tail = Vec::new();
        if !note.is_empty() {
            tail.push(self.skill_args(note, cx));
        }
        if failed {
            tail.push(self.skill_tag("refused", cx));
        }
        let icon = if v.is_merge_child() {
            IconName::GitMerge
        } else if v.is_archive_child() {
            IconName::Inbox
        } else if v.is_message_child() {
            IconName::Message
        } else {
            IconName::Eye
        };
        self.skillrow(
            icon,
            self.skill_name(vec![span(v.child_tool_label())], true, cx),
            true,
            tail,
            m.result.clone(),
            cx,
        )
    }

    fn open_pr_row(&self, v: &ToolView<'_>, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let m = v.m;
        let card = v.open_pr_card();
        let pr_id = v.open_pr_id();
        let failed = card.failed;
        let mut tail = Vec::new();
        if !card.title.is_empty() && !failed {
            tail.push(self.skill_args(card.title.clone(), cx));
        }
        if !pr_id.is_empty() && card.number > 0 && !failed && !card.pending {
            let id = pr_id.clone();
            tail.push(
                div()
                    .id(SharedString::from(format!("open-pr-{key}")))
                    .flex_none()
                    .cursor_pointer()
                    .text_color(hsla(t.cyan))
                    .hover(|s| s.text_color(hsla(t.text)))
                    .child(recur_ui::style::button_label("View PR", 13.333, 400.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |s, cx| s.open_pr(&id, cx));
                    }))
                    .into_any_element(),
            );
        }
        if card.promoted && !failed {
            tail.push(self.skill_tag("promoted", cx));
        }
        if failed {
            tail.push(self.skill_tag("failed", cx));
        }
        let tip = if failed {
            if m.result.is_empty() { "failed".into() } else { m.result.clone() }
        } else {
            card.title.clone()
        };
        let icon = if card.draft { IconName::GitPullRequestDraft } else { IconName::GitPullRequest };
        self.skillrow(icon, self.skill_name(vec![span(open_pr_label(&card))], true, cx), true, tail, tip, cx)
    }

    fn create_artifact_row(&self, v: &ToolView<'_>, cx: &Context<Self>) -> AnyElement {
        let m = v.m;
        let title = v.create_artifact_title();
        let kind = v.create_artifact_kind();
        let failed = v.create_artifact_failed();
        let bold = |s: &str| span(s.to_string()).weight(700.);
        let spans = if failed {
            vec![span("Create failed")]
        } else if m.result_pending {
            if title.is_empty() { vec![span("Creating artifact")] } else { vec![span("Creating "), bold(&title)] }
        } else if !title.is_empty() {
            vec![span("Created "), bold(&title)]
        } else {
            vec![span("Created artifact")]
        };
        let mut tail = Vec::new();
        if !kind.is_empty() && !failed {
            tail.push(self.skill_tag(kind.clone(), cx));
        }
        if failed {
            tail.push(self.skill_tag("failed", cx));
        }
        let tip = if failed {
            if m.result.is_empty() { "failed".into() } else { m.result.clone() }
        } else {
            title
        };
        let icon = IconName::from_key(artifact_icon(&kind)).unwrap_or(IconName::Grid);
        self.skillrow(icon, self.skill_name(spans, true, cx), true, tail, tip, cx)
    }

    fn present_artifact_row(&self, v: &ToolView<'_>, cx: &Context<Self>) -> AnyElement {
        let m = v.m;
        let title = v.present_artifact_title();
        let failed = v.present_artifact_failed();
        let bold = |s: &str| span(s.to_string()).weight(700.);
        let spans = if failed {
            vec![span("Present failed")]
        } else if m.result_pending {
            if title.is_empty() { vec![span("Presenting artifact")] } else { vec![span("Presenting "), bold(&title)] }
        } else if !title.is_empty() {
            vec![span("Presented "), bold(&title)]
        } else {
            vec![span("Presented artifact")]
        };
        let tail = if failed { vec![self.skill_tag("failed", cx)] } else { Vec::new() };
        let tip = if failed {
            if m.result.is_empty() { "failed".into() } else { m.result.clone() }
        } else {
            title
        };
        self.skillrow(IconName::Maximize, self.skill_name(spans, true, cx), true, tail, tip, cx)
    }

    // ---- the tool card ----

    /// `.tc-head`'s icon slot: the step icon, swapping to `+` / `−` on row hover.
    fn tc_ico(&self, group: &SharedString, step: StepIcon, color: Color, expandable: bool, open: bool) -> AnyElement {
        let mut slot = div()
            .relative()
            .flex_none()
            .size(px(14.))
            .top(px(1.))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(expandable, |d| d.group_hover(group.clone(), |s| s.invisible()))
                    .child(icon(step.icon()).size(px(14.)).color(hsla(color))),
            );
        if expandable {
            slot = slot.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .child(
                        text(if open { "−" } else { "+" })
                            .size(13.)
                            .weight(600.)
                            .line_height(LineHeight::Scale(1.))
                            .color(color)
                            .nowrap(),
                    ),
            );
        }
        slot.into_any_element()
    }

    /// `.tc-head` with its click and Enter handling.
    fn tc_head(
        &self,
        id: SharedString,
        expandable: bool,
        children: Vec<AnyElement>,
        on_toggle: Option<std::rc::Rc<dyn Fn(&mut Self, &mut Context<Self>)>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let group = SharedString::from(format!("{id}-g"));
        let mut head = div()
            .id(id.clone())
            .group(group)
            .flex()
            .items_baseline()
            .gap(px(7.))
            .w_full()
            .min_w_0()
            .pt(px(3.))
            .pb(px(3.))
            .pl(px(2.))
            .pr(px(6.))
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(hsla(t.text))
            .children(children);
        if expandable {
            let toggle = on_toggle.clone();
            head = head
                .cursor_pointer()
                .hover(|s| s.bg(hsla(t.pane)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(f) = &toggle {
                        f(this, cx);
                    }
                }));
        }
        head.into_any_element()
    }

    fn tc_label(&self, s: impl Into<SharedString>, color: Color) -> AnyElement {
        div()
            .flex_none()
            .max_w(relative(0.55))
            .overflow_hidden()
            .child(text(s).weight(600.).color(color).truncate())
            .into_any_element()
    }

    fn tc_meta(&self, s: impl Into<SharedString>, color: Color) -> AnyElement {
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(text(s).mono().size(11.).color(color).truncate())
            .into_any_element()
    }

    /// `toolCard`.
    pub(crate) fn tool_card(&mut self, m: &AgentMsg, key: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let data = self.tool_data.clone();
        let v = ToolView::new(m, data.cx());
        let expandable = v.expandable();
        let live_nested = has_live_children(m);
        let open = expandable && self.row_is_open(key, live_nested || v.is_briefing());
        let claude = v.is_claude_task();
        let claude_status = if claude {
            claude_task_chip_status(&m.tool, &v.claude_task_args(), &m.result)
        } else {
            String::new()
        };
        let claude_id = if claude { v.claude_task_row_id() } else { String::new() };
        let id = SharedString::from(format!("tc-{}-{key}", self.props.reset_key));
        let group = SharedString::from(format!("{id}-g"));
        let mut kids: Vec<AnyElement> = vec![
            self.tc_ico(&group, v.icon(), t.muted, expandable, open),
            self.tc_label(v.label(), t.text),
        ];
        let finding_row = v.is_report_finding() || v.is_update_finding();
        if finding_row {
            let sev = v.finding_severity();
            if !sev.is_empty() {
                kids.push(div().flex_none().self_center().child(finding_sev(&sev, 14., &t)).into_any_element());
            }
        }
        if v.is_update_finding() {
            let st = v.finding_status();
            if !st.is_empty() {
                kids.push(status_pill(&st, &t).render(st).self_center().into_any_element());
            }
        }
        if v.is_update_plan_step() {
            let st = v.update_plan_step_status();
            if !st.is_empty() {
                kids.push(status_pill(&st, &t).render(st).self_center().into_any_element());
            }
        }
        if !claude_status.is_empty() {
            let cls = task_status_chip_class(&claude_status).to_string();
            kids.push(
                status_pill(&cls, &t)
                    .render(pretty_task_status(&claude_status))
                    .self_center()
                    .into_any_element(),
            );
        }
        if !claude_id.is_empty() {
            kids.push(Pill { tabular: true, ..Pill::cyan(&t) }.render(claude_id).self_center().into_any_element());
        }
        if v.is_subagent() || is_task_tool(&m.tool) {
            let sa = v.subagent();
            if !sa.subagent_type.is_empty() {
                kids.push(Pill::chip(&t).render(sa.subagent_type.clone()).into_any_element());
            }
            if !sa.model.is_empty() {
                kids.push(Pill::chip(&t).render(sa.model.clone()).into_any_element());
            }
            if v.task_background() {
                kids.push(
                    Pill {
                        fg: t.amber,
                        border: amber_a(0.35),
                        bg: amber_a(0.08),
                        ..Pill::chip(&t)
                    }
                    .render("background")
                    .into_any_element(),
                );
            }
        }
        let file = v.file_of();
        if !file.is_empty() && self.hooks.open_file {
            let open_line = if finding_row { v.finding_line() } else { 0 };
            let loc = if finding_row { v.finding_loc_suffix() } else { String::new() };
            let shown = format!(
                "{}{}",
                base_name(&file),
                if !loc.is_empty() {
                    loc
                } else if open_line > 0 {
                    format!(":{open_line}")
                } else {
                    String::new()
                }
            );
            let path = file.clone();
            kids.push(
                recur_ui::tooltip::titled(
                    SharedString::from(format!("{id}-file-tip")),
                    "Open in editor",
                    Pill::cyan(&t)
                        .render(shown)
                        .id(SharedString::from(format!("{id}-file")))
                        .self_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla(crate::style::cyan_a(0.16))))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.emit(ChatEvent::OpenFile {
                                path: path.clone(),
                                line: (open_line > 0).then_some(open_line),
                                end_line: None,
                                start_col: None,
                                end_col: None,
                            });
                        })),
                )
                .into_any_element(),
            );
        }
        if m.tool == "WebFetch" {
            let href = v.fetch_url();
            if !href.is_empty() {
                kids.push(self.url_pill(&href, &id, cx));
            }
        }
        if v.is_linear() {
            let href = linear_item_url(v.linear_view().as_ref());
            if !href.is_empty() {
                kids.push(self.url_pill(&href, &id, cx));
            }
        }
        if v.has_edit_diff() {
            let (a, d) = diff_counts(&v.edit_diff());
            kids.push(
                div()
                    .flex_none()
                    .self_center()
                    .child(
                        Text::from_spans(vec![
                            span(format!("+{a}")).color(GREEN),
                            span(" "),
                            span(format!("−{d}")).color(ERR),
                        ])
                        .mono()
                        .size(10.5)
                        .nowrap(),
                    )
                    .into_any_element(),
            );
        }
        let meta = v.meta();
        if !meta.is_empty() && meta != file && v.shows_meta(open) {
            kids.push(self.tc_meta(meta, t.faint));
        }
        if m.result_error {
            kids.push(
                Pill {
                    fg: ERR,
                    bg: coral_a(0.1),
                    border: coral_a(0.3),
                    size: 10.,
                    weight: 700.,
                    tracking: 0.04,
                    uppercase: true,
                    line: 15.,
                    pad_x: 5.,
                    family_mono: false,
                    ..Pill::chip(&t)
                }
                .render("failed")
                .self_center()
                .into_any_element(),
            );
        }
        if m.result_truncated {
            kids.push(
                recur_ui::tooltip::titled(
                    SharedString::from(format!("{id}-trunc")),
                    "The provider truncated this result because it was too large",
                    Pill {
                        size: 10.,
                        weight: 700.,
                        tracking: 0.04,
                        uppercase: true,
                        line: 15.,
                        pad_x: 5.,
                        family_mono: false,
                        ..Pill::chip(&t)
                    }
                    .render("truncated")
                    .self_center(),
                )
                .into_any_element(),
            );
        }
        if m.tool == "Bash" {
            let copied = self.copied_row.as_deref() == Some(key);
            let cmd = v.bash_command();
            let k = key.to_string();
            kids.push(
                recur_ui::tooltip::titled(
                    SharedString::from(format!("{id}-copy-tip")),
                    "Copy command",
                    div()
                        .id(SharedString::from(format!("{id}-copy")))
                        .flex_none()
                        .self_center()
                        .size(px(20.))
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(!copied, |d| d.invisible().group_hover(SharedString::from(format!("{id}-card")), |s| s.visible()))
                        .text_color(hsla(if copied { GREEN } else { t.muted }))
                        .hover(|s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                        .child(if copied {
                            icon(IconName::Check).size(px(13.)).stroke_width(2.4)
                        } else {
                            icon(IconName::Copy).size(px(13.))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.copy_text(cmd.clone(), cx);
                            this.mark_copied(Some(k.clone()), None, cx);
                        })),
                )
                .into_any_element(),
            );
        }
        let k = key.to_string();
        let toggle: std::rc::Rc<dyn Fn(&mut Self, &mut Context<Self>)> =
            std::rc::Rc::new(move |this: &mut Self, cx: &mut Context<Self>| this.toggle_row(&k, open, cx));
        let head = self.tc_head(id.clone(), expandable, kids, Some(toggle), cx);
        let mut card = div()
            .group(SharedString::from(format!("{id}-card")))
            .rounded(px(7.))
            .child(head);
        if open {
            let body = self.tool_body(m, key, window, cx);
            let border = if m.result_error { coral_a(0.35) } else { t.line };
            card = card.child(
                div().mt(px(3.)).mb(px(6.)).min_w_0().child(
                    recur_ui::style::card(t.pane, border, 6.)
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .children(body),
                ),
            );
        }
        card.into_any_element()
    }

    /// The `.tc-card` children for an open tool card.
    fn tool_body(&mut self, m: &AgentMsg, key: &str, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = Theme::of(cx).tokens;
        let data = self.tool_data.clone();
        let v = ToolView::new(m, data.cx());
        let mut p = Parts::new();
        let rk = format!("{}:{key}", self.props.reset_key);
        let out_color = |err: bool| if err { ERR } else { t.muted };
        let pre = |p: &mut Parts, part: Part, idk: &str, s: String, color: Color| {
            p.push(part, tc_pre(SharedString::from(format!("{rk}-{idk}")), pre_text(format!("{rk}-{idk}-t"), s, color), 320.));
        };
        if m.tool == "TodoWrite" && !v.todo_items().is_empty() {
            p.push(Part::Other, self.todos(&v.todo_items(), cx));
        } else if v.is_claude_task() {
            self.claude_task_body(&v, &rk, &mut p, window, cx);
        } else if is_task_tool(&m.tool) {
            let sa = v.subagent();
            let launched = !m.result_error && is_subagent_launch_ack(&m.result);
            if m.result_pending || launched {
                p.push(
                    Part::Wait,
                    self.tc_wait(if launched { "Running in the background".to_string() } else { v.pending_label() }, window, cx),
                );
                if !sa.prompt.is_empty() {
                    pre(&mut p, Part::TaskPrompt, "prompt", sa.prompt.clone(), t.text);
                }
            } else {
                if !sa.prompt.is_empty() {
                    pre(&mut p, Part::TaskPrompt, "prompt", sa.prompt.clone(), t.text);
                }
                if !m.result.is_empty() {
                    pre(&mut p, Part::Pre, "out", m.result.clone(), out_color(m.result_error));
                } else if sa.prompt.is_empty() {
                    pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
                }
            }
        } else if v.is_send_message() {
            let sm = recur_engine::view::subagent::parse_send_message_args(v.input().and_then(|i| i.as_object()));
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            }
            if !sm.message.is_empty() {
                pre(&mut p, Part::TaskPrompt, "msg", sm.message.clone(), t.text);
            }
            if !m.result.is_empty() && !is_subagent_launch_ack(&m.result) {
                pre(&mut p, Part::Pre, "out", m.result.clone(), out_color(m.result_error));
            }
        } else if v.is_report_finding() || v.is_update_finding() {
            let msg = v.finding_message();
            let reason = v.finding_status_reason();
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            }
            if !msg.is_empty() {
                let md = self.md(format!("{rk}-fmsg"), &msg, None, false, cx);
                p.push(Part::Other, finding_msg(md, 320.));
            } else if !m.result_pending && reason.is_empty() {
                pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            }
            if !reason.is_empty() {
                p.push(Part::Other, labeled_note("Reason", &reason, 12., 1.4, (0., 10., 10., 10.), &t));
            }
            if m.result_error && !m.result.is_empty() {
                pre(&mut p, Part::Pre, "err", m.result.clone(), ERR);
            }
        } else if v.is_done() {
            let summary = v.done_summary();
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            }
            if !summary.is_empty() {
                let md = self.md(format!("{rk}-done"), &summary, None, false, cx);
                p.push(Part::Other, finding_msg(md, 320.));
            } else if !m.result_pending {
                pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            }
            if m.result_error && !m.result.is_empty() {
                pre(&mut p, Part::Pre, "err", m.result.clone(), ERR);
            }
        } else if v.is_ask_user() {
            let q = v.ask_user_question();
            let ctx = v.ask_user_context();
            if m.result_pending && q.is_empty() {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            }
            if !q.is_empty() {
                let mut body = div().flex().flex_col().text_size(px(12.5)).line_height(relative(1.55)).text_color(hsla(t.text));
                body = body.child(div().when(!ctx.is_empty(), |d| d.mb(px(12.5 * 0.55))).child(text(q).id(SharedString::from(format!("{rk}-askq")))));
                if !ctx.is_empty() {
                    body = body.child(text(ctx).id(SharedString::from(format!("{rk}-askc"))).size(12.).color(t.faint));
                }
                p.push(Part::Other, div().px(px(10.)).py(px(8.)).child(body));
            }
            if !m.result_pending && !m.result.is_empty() && !m.result.contains("Question delivered") {
                pre(&mut p, Part::Pre, "out", m.result.clone(), out_color(m.result_error));
            } else if m.result_error && !m.result.is_empty() {
                pre(&mut p, Part::Pre, "err", m.result.clone(), ERR);
            }
        } else if v.is_list_findings() {
            let listed = v.list_findings_result();
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            } else if let Some(listed) = listed {
                if listed.findings.is_empty() {
                    let st = if listed.status.is_empty() { "open".to_string() } else { listed.status.clone() };
                    p.push(Part::FindingEmpty, finding_empty(format!("No {st} findings."), &t));
                } else {
                    let el = self.listed_findings(&listed, &rk, cx);
                    p.push(Part::Other, el);
                }
            } else if !m.result.is_empty() {
                pre(&mut p, Part::Pre, "out", pretty_result(&m.result), out_color(m.result_error));
            } else {
                pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            }
        } else if v.is_list_artifacts() {
            let listed = v.list_artifacts_result();
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            } else if let Some(listed) = listed {
                if listed.count == 0 && listed.artifacts.is_empty() {
                    p.push(Part::FindingEmpty, finding_empty("No artifacts yet.", &t));
                } else {
                    let el = self.listed_artifacts(&listed, &rk, cx);
                    p.push(Part::Other, el);
                }
            } else if !m.result.is_empty() {
                pre(&mut p, Part::Pre, "out", pretty_result(&m.result), out_color(m.result_error));
            } else {
                pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            }
        } else if v.is_list_workspace_briefs() || v.is_get_workspace_context() || v.is_present_workspace() {
            self.briefing_body(&v, &rk, &mut p, window, cx);
        } else if m.tool == "ToolSearch" {
            let query = v.tool_search_query();
            let max = v.tool_search_max();
            if !query.is_empty() {
                let mut kids = vec![text(format!("\"{query}\"")).mono().size(11.).color(sh_color("sh-str").expect("sh-str")).break_anywhere().into_any_element()];
                if max > 0 {
                    kids.push(grep_flag(format!("max {max}"), &t));
                }
                p.push(Part::Grepq, grepq(kids));
            } else {
                pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            }
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            } else if !m.result.is_empty() {
                pre(&mut p, Part::Pre, "out", m.result.clone(), out_color(m.result_error));
            }
        } else if v.is_read_lints() {
            self.lint_body(&v, &rk, &mut p, window, cx);
        } else if v.is_submit_plan() {
            let summary = v.submit_plan_summary();
            let body = v.submit_plan_body();
            let steps = v.submit_plan_steps();
            let atts = v.submit_plan_att_ids();
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            }
            if !summary.is_empty() {
                p.push(
                    Part::PlanSummary,
                    div().px(px(10.)).py(px(8.)).child(
                        text(summary.clone())
                            .id(SharedString::from(format!("{rk}-psum")))
                            .size(12.5)
                            .line_height(LineHeight::Scale(1.55))
                            .color(t.text)
                            .pre_wrap()
                            .break_anywhere(),
                    ),
                );
            }
            if !body.is_empty() {
                let md = self.md(format!("{rk}-pbody"), &body, None, false, cx);
                let vh = f32::from(window.viewport_size().height);
                p.push(
                    Part::PlanBody,
                    div()
                        .id(SharedString::from(format!("{rk}-pbody-s")))
                        .max_h(px((0.7 * vh).min(720.)))
                        .overflow_y_scroll()
                        .px(px(10.))
                        .py(px(8.))
                        .child(md),
                );
            }
            if !steps.is_empty() {
                p.push(Part::PlanSteps, plan_steps(&steps, &rk, &t));
            } else if summary.is_empty() && body.is_empty() && !m.result_pending {
                pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            }
            if !atts.is_empty() {
                p.push(
                    Part::Other,
                    div().flex().flex_wrap().gap(px(4.)).pt(px(6.)).pb(px(8.)).px(px(10.)).children(atts.into_iter().map(|a| {
                        recur_ui::style::card(t.pane_2, t.line, 4.)
                            .px(px(6.))
                            .py(px(2.))
                            .child(label(a, Family::Mono, 10.5, 400.).color(t.muted))
                    })),
                );
            }
            if m.result_error && !m.result.is_empty() {
                pre(&mut p, Part::Pre, "err", m.result.clone(), ERR);
            }
        } else if v.is_update_plan_step() {
            let id = v.update_plan_step_id();
            let title = v.update_plan_step_title();
            let note = v.update_plan_step_note();
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            }
            if !id.is_empty() || !title.is_empty() || !note.is_empty() {
                let mut col = div().px(px(10.)).py(px(8.)).flex().flex_col().gap(px(6.));
                let mut row = div().flex().flex_wrap().items_baseline().gap_x(px(8.)).gap_y(px(6.)).text_size(px(12.5)).line_height(relative(1.45));
                if !id.is_empty() {
                    row = row.child(label(id, Family::Mono, 11., 400.).color(t.cyan));
                }
                if !title.is_empty() {
                    row = row.child(div().min_w_0().child(text(title).id(SharedString::from(format!("{rk}-pst"))).color(t.text).break_anywhere()));
                }
                col = col.child(row);
                if !note.is_empty() {
                    col = col.child(labeled_note("Note", &note, 12., 1.4, (0., 0., 0., 0.), &t));
                }
                p.push(Part::Other, col);
            } else if !m.result_pending {
                pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            }
            if m.result_error && !m.result.is_empty() {
                pre(&mut p, Part::Pre, "err", m.result.clone(), ERR);
            }
        } else if v.is_edit_plan() {
            let body = v.edit_plan_text();
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            }
            if !body.is_empty() {
                let el = self.canvas_plan(&rk, "eplan", &body, window, cx);
                p.push(Part::CanvasPlan, el);
            } else if !m.result_pending && m.result.is_empty() {
                pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            }
            if !m.result.is_empty() {
                p.push(
                    Part::PlanAck,
                    div().px(px(10.)).py(px(8.)).child(
                        text(m.result.clone())
                            .id(SharedString::from(format!("{rk}-pack")))
                            .size(12.)
                            .line_height(LineHeight::Scale(1.45))
                            .color(if m.result_error { ERR } else { t.muted }),
                    ),
                );
            }
        } else if v.is_create_plan() && !v.plan_body().is_empty() {
            let el = self.canvas_plan(&rk, "cplan", &v.plan_body(), window, cx);
            p.push(Part::CanvasPlan, el);
        } else if m.tool == "Bash" {
            let mut spans = vec![span("$ ").color(t.faint)];
            for (tok, cls) in crate::shhl::highlight_bash(&v.bash_command()) {
                let s = span(tok);
                spans.push(match sh_color(cls) {
                    Some(c) => s.color(c),
                    None => s,
                });
            }
            if !m.result.is_empty() {
                spans.push(span("\n\n"));
                spans.push(span(m.result.clone()).color(if m.result_error { ERR } else { t.muted }));
            }
            let body = Text::from_spans(spans)
                .id(gpui_kit::ElementId::Name(format!("{rk}-bash").into()))
                .mono()
                .size(11.)
                .line_height(LineHeight::Scale(1.55))
                .color(t.text)
                .pre_wrap()
                .break_anywhere();
            p.push(Part::Pre, tc_pre(SharedString::from(format!("{rk}-bash-s")), body, 320.));
        } else if (m.tool == "Grep" || m.tool == "Glob")
            && v.input().and_then(|i| i.get("pattern")).is_some_and(|p| p.is_string())
        {
            let input = v.input().cloned().unwrap_or_default();
            let pat = input.get("pattern").and_then(|p| p.as_str()).unwrap_or("").to_string();
            let mut kids = vec![text(format!("\"{pat}\"")).mono().size(11.).color(sh_color("sh-str").expect("sh-str")).break_anywhere().into_any_element()];
            if let Some(path) = input.get("path").and_then(|p| p.as_str()).filter(|p| !p.is_empty()) {
                kids.push(text(format!("in {}", v.rel(path))).mono().size(11.).color(t.muted).into_any_element());
            }
            for f in v.grep_flags() {
                kids.push(grep_flag(f, &t));
            }
            p.push(Part::Grepq, grepq(kids));
            if !m.result.is_empty() {
                pre(&mut p, Part::Pre, "out", m.result.clone(), out_color(m.result_error));
            }
        } else if m.tool == "WebFetch" || m.tool == "WebSearch" {
            let prompt = if m.tool == "WebFetch" { v.fetch_prompt() } else { String::new() };
            let body = recur_engine::view::js::trim(&m.result).to_string();
            if m.result_pending {
                p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            }
            if !prompt.is_empty() {
                p.push(Part::WebPrompt, labeled_note("Asked", &prompt, 12., 1.45, (8., 10., 8., 10.), &t));
            }
            if !body.is_empty() {
                if m.result_error || web_result_is_json(&body) {
                    pre(&mut p, Part::Pre, "out", pretty_result(&m.result), out_color(m.result_error));
                } else {
                    let md = self.md(format!("{rk}-web"), &m.result, None, true, cx);
                    p.push(
                        Part::WebMsg,
                        div().id(SharedString::from(format!("{rk}-web-s"))).px(px(10.)).py(px(8.)).max_h(px(360.)).overflow_y_scroll().child(md),
                    );
                }
            } else if m.result_error {
                pre(&mut p, Part::Pre, "err", "failed".into(), ERR);
            }
        } else if v.has_edit_diff() {
            let lang = lang_from_path(&v.file_of());
            let rows: Vec<AnyElement> = v
                .edit_diff()
                .iter()
                .map(|r| {
                    let (mark, mark_color, bg) = match r.kind {
                        DiffKind::Del => ("−", ERR, Some(coral_a(0.09))),
                        DiffKind::Add => ("+", GREEN, Some(crate::style::green_a(0.09))),
                        _ => ("", recur_ui::color::TRANSPARENT, None),
                    };
                    cv_row(
                        div().flex_none().w(px(18.)).flex().justify_center().child(label(mark, Family::Mono, 11., 400.).line_height(LineHeight::Scale(1.55)).color(mark_color)),
                        code_line(&r.text, lang, &t),
                        bg,
                    )
                })
                .collect();
            p.push(Part::Other, cv(SharedString::from(format!("{rk}-cv")), rows));
        } else if v.has_write_body() {
            let lang = lang_from_path(&v.file_of());
            let wb = v.write_body();
            let body = wb.strip_suffix('\n').unwrap_or(&wb).to_string();
            let rows: Vec<AnyElement> = body
                .split('\n')
                .enumerate()
                .map(|(li, line)| {
                    cv_row(
                        div().flex_none().w(px(34.)).pr(px(10.)).flex().justify_end().child(label((li + 1).to_string(), Family::Mono, 11., 400.).line_height(LineHeight::Scale(1.55)).color(t.faint)),
                        code_line(line, lang, &t),
                        None,
                    )
                })
                .collect();
            p.push(Part::Other, cv(SharedString::from(format!("{rk}-cv")), rows));
        } else if m.result_pending {
            p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
        } else if m.tool == "Read" {
            if !m.result.is_empty() {
                pre(&mut p, Part::Pre, "out", m.result.clone(), t.muted);
            } else {
                pre(&mut p, Part::Pre, "in", v.pretty_input(), t.text);
            }
        } else if m.tool == "Edit" || m.tool == "Write" || m.tool == "Delete" {
            if !m.result.is_empty() {
                pre(&mut p, Part::Pre, "out", pretty_result(&m.result), out_color(m.result_error));
            } else if m.result_error {
                pre(&mut p, Part::Pre, "err", "failed".into(), ERR);
            }
        } else if v.is_linear() {
            self.linear_body(&v, &rk, &mut p, window, cx);
        } else {
            pre(&mut p, Part::Pre, "cmd", v.pretty_input(), t.text);
            if !m.result.is_empty() {
                pre(&mut p, Part::Pre, "out", pretty_result(&m.result), out_color(m.result_error));
            }
        }
        if !m.children.is_empty() {
            let kids: Vec<AnyElement> = m
                .children
                .clone()
                .iter()
                .enumerate()
                .map(|(ci, c)| {
                    let _ = step_row_key(&c.tool_id, ci);
                    self.nested_child(c, &format!("{key}:{ci}"), window, cx)
                })
                .collect();
            p.push(
                Part::Other,
                div()
                    .mt(px(4.))
                    .mb(px(8.))
                    .mx(px(6.))
                    .py(px(2.))
                    .pl(px(8.))
                    .border_l_2()
                    .border_color(hsla(t.line))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(kids),
            );
        }
        p.into_children(&t)
    }

    /// `presentWorkspaceCard` inside `.tc-briefs`.
    pub(crate) fn present_workspace_card(
        &mut self,
        card: &recur_agent_tools::display::briefing_tools::PresentWorkspaceView,
        idk: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut col = div().flex().flex_col().gap(px(8.));
        if let Some(brief) = &card.brief {
            col = col.child(crate::cards::brief::brief_card(
                SharedString::from(idk.to_string()),
                brief,
                &card.target,
                &self.state,
                self.anim_epoch,
                window,
                cx,
            ));
        }
        col.into_any_element()
    }

    /// `.chat-canvas.tc-plan` inside a tool body.
    fn canvas_plan(&mut self, rk: &str, k: &str, body: &str, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let md = self.md(format!("{rk}-{k}"), body, None, false, cx);
        let vh = f32::from(window.viewport_size().height);
        div()
            .id(SharedString::from(format!("{rk}-{k}-s")))
            .max_h(px((0.7 * vh).min(720.)))
            .overflow_y_scroll()
            .px(px(10.))
            .py(px(8.))
            .child(md)
            .into_any_element()
    }

    /// `nestedChild`: subagent output inside the parent card.
    fn nested_child(&mut self, m: &AgentMsg, key: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        if !m.tool.is_empty() {
            return self.tool_card(m, key, window, cx);
        }
        if m.thinking {
            return self.thinking_card(m, key, false, cx);
        }
        if m.compact {
            return self.compact_card(m, key, cx);
        }
        if m.body.is_empty() {
            return div().into_any_element();
        }
        let rk = format!("{}:{key}", self.props.reset_key);
        let body = if m.streaming {
            self.stream(format!("{rk}-nst"), &m.body, None, cx)
        } else {
            self.md(format!("{rk}-nmd"), &m.body, None, false, cx)
        };
        div()
            .px(px(6.))
            .py(px(4.))
            .text_size(px(12.5))
            .line_height(relative(1.5))
            .text_color(hsla(if m.streaming { t.text } else { t.muted }))
            .child(body)
            .into_any_element()
    }

    fn todos(&self, items: &[recur_agent::display::todos::Todo], cx: &Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        div()
            .py(px(2.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(items.iter().map(|td| todo_row(&td.content, &td.status, None, &[], "", false, &t)))
            .into_any_element()
    }

    fn claude_task_body(&mut self, v: &ToolView<'_>, rk: &str, p: &mut Parts, window: &mut Window, cx: &mut Context<Self>) {
        let t = Theme::of(cx).tokens;
        let m = v.m;
        let args = v.claude_task_args();
        if m.result_pending {
            p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
        }
        let pre = |p: &mut Parts, idk: &str, s: String, color: Color| {
            p.push(Part::Pre, tc_pre(SharedString::from(format!("{rk}-{idk}")), pre_text(format!("{rk}-{idk}-t"), s, color), 320.));
        };
        match m.tool.as_str() {
            "TaskList" => {
                if !m.result_pending && !m.result_error {
                    match v.claude_task_list_items() {
                        Some(items) if !items.is_empty() => {
                            p.push(
                                Part::Other,
                                div().py(px(2.)).flex().flex_col().gap(px(2.)).children(items.iter().map(|td| claude_task_item(td, &t))),
                            );
                        }
                        None => pre(p, "out", m.result.clone(), t.muted),
                        _ => p.push(Part::FindingEmpty, finding_empty("No tasks.", &t)),
                    }
                }
            }
            "TaskGet" => {
                if let Some(got) = v.claude_task_get_view() {
                    if !got.description.is_empty() {
                        let md = self.md(format!("{rk}-tget"), &got.description, None, false, cx);
                        p.push(Part::Other, finding_msg(md, 320.));
                    }
                    if !got.owner.is_empty() || !got.blocked_by.is_empty() || !got.blocks.is_empty() {
                        p.push(Part::Other, task_relations(&got.owner, &got.blocked_by, &got.blocks, &t));
                    }
                } else if !m.result_pending && !m.result_error && !m.result.is_empty() {
                    pre(p, "out", m.result.clone(), t.muted);
                } else if !m.result_pending && !args.description.is_empty() {
                    let md = self.md(format!("{rk}-tdesc"), &args.description, None, false, cx);
                    p.push(Part::Other, finding_msg(md, 320.));
                }
            }
            "TaskCreate" => {
                let md = self.md(format!("{rk}-tcreate"), &args.description, None, false, cx);
                p.push(Part::Other, finding_msg(md, 320.));
            }
            "TaskUpdate" => {
                if !args.owner.is_empty() || !args.add_blocks.is_empty() || !args.add_blocked_by.is_empty() {
                    p.push(Part::Other, task_relations(&args.owner, &args.add_blocked_by, &args.add_blocks, &t));
                }
                if !args.description.is_empty() {
                    let md = self.md(format!("{rk}-tupd"), &args.description, None, false, cx);
                    p.push(Part::Other, finding_msg(md, 320.));
                }
            }
            _ => {}
        }
        if m.result_error && !m.result.is_empty() {
            pre(p, "err", m.result.clone(), ERR);
        } else if !m.result.is_empty()
            && !m.tool.is_empty()
            && !claude_task_result_is_ack(&m.tool, &m.result)
            && m.tool != "TaskList"
            && m.tool != "TaskGet"
        {
            pre(p, "res", m.result.clone(), t.muted);
        }
    }

    fn linear_body(&mut self, v: &ToolView<'_>, rk: &str, p: &mut Parts, window: &mut Window, cx: &mut Context<Self>) {
        let t = Theme::of(cx).tokens;
        let m = v.m;
        let view = v.linear_view();
        let input_body = v.linear_input_body();
        let item = view.as_ref().filter(|v| v.kind == LinearViewKind::Item).and_then(|v| v.items.first().cloned());
        let body = item
            .as_ref()
            .map(|i| i.body.clone())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| {
                if view.as_ref().is_some_and(|v| v.kind == LinearViewKind::List) {
                    String::new()
                } else {
                    input_body.clone()
                }
            });
        if m.result_pending {
            p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
        }
        if let Some(lv) = view.as_ref().filter(|v| v.kind == LinearViewKind::List) {
            if lv.items.is_empty() {
                let noun = if lv.noun.is_empty() { "results" } else { &lv.noun };
                p.push(Part::FindingEmpty, finding_empty(format!("No {noun}."), &t));
            } else {
                let mut head = findings_head(&t).child(findings_count(linear_count_label(lv), &t));
                if lv.has_next_page {
                    head = head.child(label("more", Family::Ui, 11., 400.).color(t.faint));
                }
                let rows: Vec<AnyElement> = lv
                    .items
                    .iter()
                    .enumerate()
                    .map(|(ii, it)| {
                        let mut row = div().flex().flex_wrap().items_center().gap_x(px(8.)).gap_y(px(6.)).px(px(10.)).py(px(8.));
                        if !it.color.is_empty() {
                            row = row.child(lin_dot(&it.color, &t));
                        }
                        let mut main = div().flex_1().min_w(px(12. * 13.)).min_w_0().flex().flex_col().gap(px(2.)).child(
                            div().min_w_0().overflow_hidden().child(text(it.title.clone()).size(12.5).line_height(LineHeight::Scale(1.35)).color(t.text).truncate()),
                        );
                        if !it.summary.is_empty() {
                            main = main.child(div().min_w_0().overflow_hidden().child(text(it.summary.clone()).size(11.5).line_height(LineHeight::Scale(1.4)).color(t.muted).truncate()));
                        }
                        row = row.child(main);
                        if !it.identifier.is_empty() {
                            row = row.child(Pill { tabular: true, ..Pill::cyan(&t) }.render(it.identifier.clone()));
                        }
                        for chip in &it.chips {
                            row = row.child(Pill::chip(&t).render(chip.clone()));
                        }
                        if !it.url.is_empty() {
                            row = row.child(self.url_pill(&it.url, &format!("{rk}-lin-{ii}"), cx));
                        }
                        finding_row_wrap(row.into_any_element(), ii + 1 == lv.items.len(), &t)
                    })
                    .collect();
                p.push(Part::Other, div().flex().flex_col().child(head).child(findings_list(SharedString::from(format!("{rk}-lin")), rows, 360.)));
            }
        } else if let Some(item) = item {
            if !item.title.is_empty() {
                let mut head = findings_head(&t);
                if !item.color.is_empty() {
                    head = head.child(lin_dot(&item.color, &t));
                }
                head = head.child(findings_count(item.title.clone(), &t));
                if !item.identifier.is_empty() && item.identifier != item.title {
                    head = head.child(Pill { tabular: true, ..Pill::cyan(&t) }.render(item.identifier.clone()));
                }
                p.push(Part::Other, head);
            }
            if !item.chips.is_empty() {
                p.push(
                    Part::TaskChips,
                    div().flex().flex_wrap().gap(px(5.)).pt(px(8.)).px(px(10.)).children(item.chips.iter().map(|c| Pill::chip(&t).render(c.clone()))),
                );
            }
            if !body.is_empty() {
                let md = self.md(format!("{rk}-linb"), &body, None, false, cx);
                p.push(Part::Other, finding_msg(md, 320.));
            }
        } else if !body.is_empty() {
            let md = self.md(format!("{rk}-linb"), &body, None, false, cx);
            p.push(Part::Other, finding_msg(md, 320.));
        }
        if view.is_none() && m.result_error && !m.result.is_empty() {
            p.push(Part::Pre, tc_pre(SharedString::from(format!("{rk}-linerr")), pre_text(format!("{rk}-linerr-t"), m.result.clone(), ERR), 320.));
        } else if view.is_none() && !m.result.is_empty() && !m.result_pending {
            p.push(Part::Pre, tc_pre(SharedString::from(format!("{rk}-linout")), pre_text(format!("{rk}-linout-t"), pretty_result(&m.result), t.muted), 320.));
        }
    }

    /// `.tc-url`: a compact link chip; absolute http(s) URLs open in the browser.
    fn url_pill(&self, href: &str, idk: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let pill = Pill {
            fg: t.cyan,
            bg: t.cyan.mix(0.12, recur_ui::color::TRANSPARENT),
            border: t.cyan.mix(0.28, recur_ui::color::TRANSPARENT),
            ..Pill::chip(&t)
        };
        let el = div()
            .flex_none()
            .self_center()
            .min_w_0()
            .max_w(relative(0.52))
            .child(pill.render_truncated(compact_url(href)));
        let tip = recur_ui::tooltip::titled(SharedString::from(format!("{idk}-url-tip")), href.to_string(), el);
        if external_http_url(href).is_none() {
            return tip.into_any_element();
        }
        let href = href.to_string();
        div()
            .id(SharedString::from(format!("{idk}-url")))
            .flex_none()
            .self_center()
            .cursor_pointer()
            .child(tip)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                recur_content::links::open_external_url(&href, cx);
            })
            .into_any_element()
    }

    fn briefing_body(&mut self, v: &ToolView<'_>, rk: &str, p: &mut Parts, window: &mut Window, cx: &mut Context<Self>) {
        let t = Theme::of(cx).tokens;
        let m = v.m;
        let pre = |p: &mut Parts, idk: &str, s: String, color: Color| {
            p.push(Part::Pre, tc_pre(SharedString::from(format!("{rk}-{idk}")), pre_text(format!("{rk}-{idk}-t"), s, color), 320.));
        };
        let out_color = if m.result_error { ERR } else { t.muted };
        if m.result_pending {
            p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
        } else if v.is_list_workspace_briefs() {
            if let Some(listed) = v.list_workspace_briefs_result() {
                p.push(Part::FindingEmpty, finding_empty(list_workspace_briefs_summary(&listed), &t));
            } else if !m.result.is_empty() {
                pre(p, "out", pretty_result(&m.result), out_color);
            } else {
                pre(p, "cmd", v.pretty_input(), t.text);
            }
        } else if v.is_get_workspace_context() {
            if let Some(ctx) = v.workspace_context_result() {
                p.push(Part::FindingEmpty, finding_empty(workspace_context_summary(&ctx), &t));
            } else if !m.result.is_empty() {
                pre(p, "out", pretty_result(&m.result), out_color);
            } else {
                pre(p, "cmd", v.pretty_input(), t.text);
            }
        } else {
            let card = v.present_workspace_result();
            if m.result_error || card.as_ref().is_some_and(|c| !c.error.is_empty()) {
                let msg = card
                    .as_ref()
                    .map(|c| c.error.clone())
                    .filter(|e| !e.is_empty())
                    .or_else(|| (!m.result.is_empty()).then(|| m.result.clone()))
                    .unwrap_or_else(|| "failed".into());
                pre(p, "err", msg, ERR);
            } else if let Some(card) = card {
                let el = self.present_workspace_card(&card, &format!("{rk}-brief"), window, cx);
                p.push(Part::Other, el);
            } else if !m.result.is_empty() {
                pre(p, "out", pretty_result(&m.result), out_color);
            } else {
                pre(p, "cmd", v.pretty_input(), t.text);
            }
        }
    }

    fn lint_body(&mut self, v: &ToolView<'_>, rk: &str, p: &mut Parts, window: &mut Window, cx: &mut Context<Self>) {
        let t = Theme::of(cx).tokens;
        let m = v.m;
        let lint = v.read_lints_result();
        let paths = v.read_lints_paths();
        let open_file = self.hooks.open_file;
        let path_rows = |this: &mut Self, list: Vec<String>, cx: &mut Context<Self>| -> AnyElement {
            let t = Theme::of(cx).tokens;
            let rows: Vec<AnyElement> = list
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    let rel = v.rel(path).to_string();
                    let mut row = div().flex().flex_wrap().items_baseline().gap_x(px(8.)).gap_y(px(6.)).min_w_0();
                    if open_file {
                        row = row.child(this.file_button(base_name(path).to_string(), path.clone(), None, &format!("{rk}-lp-{i}"), cx));
                    }
                    row.child(div().min_w_0().child(text(rel).id(SharedString::from(format!("{rk}-lpr-{i}"))).mono().size(11.).color(t.faint).break_anywhere()))
                        .into_any_element()
                })
                .collect();
            div()
                .id(SharedString::from(format!("{rk}-lpaths")))
                .max_h(px(240.))
                .overflow_y_scroll()
                .pt(px(6.))
                .px(px(10.))
                .pb(px(10.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(rows)
                .into_any_element()
        };
        if m.result_pending {
            p.push(Part::Wait, self.tc_wait(v.pending_label(), window, cx));
            if !paths.is_empty() {
                let el = path_rows(self, paths, cx);
                p.push(Part::LintPaths, el);
            }
        } else if let Some(lint) = lint {
            if lint.total_diagnostics == 0 {
                let msg = if lint.total_files > 0 {
                    format!(
                        "No linter issues in {} file{}.",
                        lint.total_files,
                        if lint.total_files == 1 { "" } else { "s" }
                    )
                } else {
                    "No linter issues.".to_string()
                };
                p.push(Part::FindingEmpty, finding_empty(msg, &t));
                if !lint.files.is_empty() {
                    let list: Vec<String> = lint.files.iter().map(|f| f.path.clone()).filter(|p| !p.is_empty()).collect();
                    let el = path_rows(self, list, cx);
                    p.push(Part::LintPaths, el);
                }
            } else {
                let mut rows = Vec::new();
                let total: usize = lint.files.iter().map(|f| f.diagnostics.len()).sum();
                let mut n = 0;
                for (fi, f) in lint.files.iter().enumerate() {
                    for (di, d) in f.diagnostics.iter().enumerate() {
                        n += 1;
                        let loc = lint_loc_suffix(d);
                        let mut head = div().flex().flex_wrap().items_center().gap_x(px(8.)).gap_y(px(6.)).pt(px(8.)).px(px(10.));
                        if !d.severity.is_empty() {
                            head = head.child(sev_pill(lint_sev_class(&d.severity), &t).render(d.severity.clone()));
                        }
                        if !f.path.is_empty() && open_file {
                            head = head.child(self.file_button(
                                format!("{}{loc}", base_name(&f.path)),
                                f.path.clone(),
                                (d.line > 0).then_some(d.line),
                                &format!("{rk}-lf-{fi}-{di}"),
                                cx,
                            ));
                        } else if !f.path.is_empty() {
                            head = head.child(label(format!("{}{loc}", v.rel(&f.path)), Family::Mono, 11., 400.).color(t.cyan));
                        }
                        if !d.source.is_empty() {
                            head = head.child(Pill::chip(&t).render(d.source.clone()));
                        }
                        let mut row = div().child(head);
                        if !d.message.is_empty() {
                            row = row.child(
                                div().pt(px(6.)).px(px(10.)).pb(px(10.)).child(
                                    text(d.message.clone())
                                        .id(SharedString::from(format!("{rk}-lm-{fi}-{di}")))
                                        .size(12.5)
                                        .line_height(LineHeight::Scale(1.55))
                                        .color(t.text),
                                ),
                            );
                        }
                        rows.push(finding_row_wrap(row.into_any_element(), n == total, &t));
                    }
                }
                p.push(Part::Other, findings_list(SharedString::from(format!("{rk}-lint")), rows, 360.));
            }
        } else if !m.result.is_empty() {
            p.push(
                Part::Pre,
                tc_pre(SharedString::from(format!("{rk}-lo")), pre_text(format!("{rk}-lo-t"), pretty_result(&m.result), if m.result_error { ERR } else { t.muted }), 320.),
            );
        } else if !paths.is_empty() {
            let el = path_rows(self, paths, cx);
            p.push(Part::LintPaths, el);
        } else {
            p.push(Part::Pre, tc_pre(SharedString::from(format!("{rk}-lc")), pre_text(format!("{rk}-lc-t"), v.pretty_input(), t.text), 320.));
        }
    }

    /// A `button.tc-file` that opens a path.
    pub(crate) fn file_button(
        &self,
        shown: String,
        path: String,
        line: Option<i64>,
        idk: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        recur_ui::tooltip::titled(
            SharedString::from(format!("{idk}-tip")),
            "Open in editor",
            Pill::cyan(&t)
                .render(shown)
                .id(SharedString::from(idk.to_string()))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(crate::style::cyan_a(0.16))))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ChatEvent::OpenFile {
                        path: path.clone(),
                        line,
                        end_line: None,
                        start_col: None,
                        end_col: None,
                    });
                })),
        )
        .into_any_element()
    }

    /// The `list_findings` accordion.
    fn listed_findings(&mut self, listed: &crate::findings_ui::ListedFindings, rk: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tally = finding_severity_tally(listed.findings.iter().map(|f| f.severity.as_str()));
        let filter = if listed.status.is_empty() { "open".to_string() } else { listed.status.clone() };
        let mut head = findings_head(&t).child(findings_count(
            format!("{} {}", listed.count, if listed.count == 1 { "finding" } else { "findings" }),
            &t,
        ));
        if filter != "open" {
            head = head.child(label(filter.clone(), Family::Mono, 10., 600.).tracking(0.06).uppercase().color(t.faint));
        }
        if listed.count as usize > listed.findings.len() {
            head = head.child(label(format!("showing {}", listed.findings.len()), Family::Ui, 11., 400.).color(t.faint));
        }
        if tally.len() > 1 || tally.first().is_some_and(|x| x.severity != "info") {
            head = head.child(
                div().ml_auto().flex().items_center().gap(px(8.)).children(tally.iter().map(|x| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(finding_sev(&x.severity, 12., &t))
                        .child(label(x.count.to_string(), Family::Mono, 10.5, 400.).tabular().color(t.muted))
                })),
            );
        }
        let n = listed.findings.len();
        let rows: Vec<AnyElement> = listed
            .findings
            .iter()
            .enumerate()
            .map(|(fi, f)| {
                let loc = crate::findings_ui::finding_loc_suffix(f.line, f.end_line);
                let loc_label = finding_loc_label(&f.file, f.line, f.end_line);
                let title = finding_title(if f.message.is_empty() { &f.summary } else { &f.message });
                let accord = if f.id.is_empty() { format!("listed-{fi}-{}-{}", f.file, f.line) } else { f.id.clone() };
                let open = self.finding_accord_open.get(&accord).copied().unwrap_or(false);
                let note_count = f.notes.len();
                let show_status = !f.status.is_empty() && (filter == "all" || f.status != filter);
                let mut trail = div().flex_none().flex().items_center().gap(px(6.)).self_center();
                if note_count > 0 {
                    trail = trail.child(recur_ui::tooltip::titled(
                        SharedString::from(format!("{rk}-fn-{fi}")),
                        if note_count == 1 { "1 note".to_string() } else { format!("{note_count} notes") },
                        div().flex().items_center().child(icon(IconName::NotebookPen).size(px(12.)).color(hsla(t.muted))),
                    ));
                }
                if show_status {
                    trail = trail.child(status_pill(&f.status, &t).render(f.status.clone()));
                }
                let rel = format!("{}{loc}", self.props.root_path.is_empty().then(|| f.file.clone()).unwrap_or_else(|| crate::tools::rel_path(&self.props.root_path, &f.file).to_string()));
                let head = self.accord_head(&accord, open, &f.severity, &title, &loc_label, Some(rel), trail.into_any_element(), &format!("{rk}-fh-{fi}"), cx);
                let mut row = div().child(head);
                if open {
                    let mut detail = accord_detail(&t);
                    if !f.file.is_empty() {
                        let shown = if loc_label.is_empty() { format!("{}{loc}", base_name(&f.file)) } else { loc_label.clone() };
                        let el = if self.hooks.open_file {
                            let line = (f.line > 0).then_some(f.line);
                            let end = (f.end_line > 0 && f.line > 0 && f.end_line > f.line).then_some(f.end_line);
                            self.loc_button(shown, f.file.clone(), line, end, &format!("{rk}-fd-{fi}"), cx)
                        } else {
                            label(format!("{}{loc}", crate::tools::rel_path(&self.props.root_path, &f.file)), Family::Mono, 11., 400.).color(t.cyan).into_any_element()
                        };
                        detail = detail.child(div().line_height(relative(1.2)).child(el));
                    }
                    if !f.message.is_empty() && finding_body_needed(&f.message) {
                        let md = self.md(format!("{rk}-fm-{fi}"), &f.message, None, false, cx);
                        detail = detail.child(md);
                    }
                    if !f.status_reason.is_empty() {
                        detail = detail.child(labeled_note("Reason", &f.status_reason, 12., 1.4, (0., 0., 0., 0.), &t));
                    }
                    if note_count > 0 {
                        detail = detail.child(div().flex().flex_col().gap(px(2.)).children(f.notes.iter().map(|n| {
                            div().relative().pt(px(5.)).pb(px(5.)).pl(px(8.)).pr(px(28.)).rounded(px(6.)).child(labeled_note("Note", &n.body, 12., 1.45, (0., 0., 0., 0.), &t))
                        })));
                    }
                    row = row.child(detail);
                }
                finding_row_wrap(row.into_any_element(), fi + 1 == n, &t)
            })
            .collect();
        div().flex().flex_col().child(head).child(findings_list(SharedString::from(format!("{rk}-fl")), rows, 360.)).into_any_element()
    }

    /// The accordion row head shared by `list_findings` and the turn digest.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn accord_head(
        &self,
        accord: &str,
        open: bool,
        severity: &str,
        title: &str,
        loc_label: &str,
        loc_tip: Option<String>,
        trail: AnyElement,
        idk: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let sev = if severity.is_empty() { "low" } else { severity };
        let mut main = div().min_w_0().flex().flex_col().gap(px(2.)).child(
            div().min_w_0().overflow_hidden().child(text(title.to_string()).size(12.5).line_height(LineHeight::Scale(1.35)).color(t.text).truncate()),
        );
        if !loc_label.is_empty() {
            let line = div().min_w_0().overflow_hidden().child(label(loc_label.to_string(), Family::Mono, 11., 400.).tabular().color(t.cyan).truncate());
            main = main.child(div().flex().items_center().gap(px(6.)).min_w_0().child(match loc_tip {
                Some(tip) => recur_ui::tooltip::titled(SharedString::from(format!("{idk}-loc")), tip, line).into_any_element(),
                None => line.into_any_element(),
            }));
        }
        let a = accord.to_string();
        let open_bg = t.pane_2.mix(0.55, recur_ui::color::TRANSPARENT);
        div()
            .id(SharedString::from(idk.to_string()))
            .flex()
            .items_start()
            .gap_x(px(8.))
            .w_full()
            .px(px(10.))
            .py(px(8.))
            .cursor_pointer()
            .when(open, |d| d.bg(hsla(open_bg)))
            .hover(|s| s.bg(hsla(t.pane_2)))
            .child(div().flex_none().w(px(14.)).mt(px(2.)).flex().justify_center().child(finding_sev(sev, 14., &t)))
            .child(div().flex_1().min_w_0().child(main))
            .child(trail)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_finding_accord(&a, cx)))
            .into_any_element()
    }

    /// A `button.tc-file` opening a line span.
    pub(crate) fn loc_button(&self, shown: String, path: String, line: Option<i64>, end: Option<i64>, idk: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        recur_ui::tooltip::titled(
            SharedString::from(format!("{idk}-tip")),
            "Open in editor",
            Pill::cyan(&t)
                .render(shown)
                .id(SharedString::from(idk.to_string()))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(crate::style::cyan_a(0.16))))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(ChatEvent::OpenFile {
                        path: path.clone(),
                        line,
                        end_line: end,
                        start_col: None,
                        end_col: None,
                    });
                })),
        )
        .into_any_element()
    }

    fn listed_artifacts(&mut self, listed: &recur_engine::view::artifacts::ListedArtifacts, rk: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tally = artifact_kind_tally(listed.artifacts.iter().map(|a| a.kind.as_str()));
        let mut head = findings_head(&t).child(findings_count(
            format!("{} {}", listed.count, if listed.count == 1 { "artifact" } else { "artifacts" }),
            &t,
        ));
        if listed.count as usize > listed.artifacts.len() {
            head = head.child(label(format!("showing {}", listed.artifacts.len()), Family::Ui, 11., 400.).color(t.faint));
        }
        if tally.len() > 1 {
            head = head.child(div().ml_auto().flex().items_center().gap(px(8.)).children(tally.iter().map(|x| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(ico(IconName::from_key(artifact_icon(&x.kind)).unwrap_or(IconName::Grid), 12., t.muted))
                    .child(label(x.count.to_string(), Family::Mono, 10.5, 400.).tabular().color(t.muted))
            })));
        }
        let now = crate::chat::now_ms();
        let n = listed.artifacts.len();
        let rows: Vec<AnyElement> = listed
            .artifacts
            .iter()
            .enumerate()
            .map(|(ai, a)| {
                let when = artifact_when(&a.created_at, now);
                let can_open = self.hooks.open_artifact && artifact_can_open(&a.id, &a.path);
                let title = if !a.title.is_empty() { a.title.clone() } else if !a.id.is_empty() { a.id.clone() } else { "Untitled".into() };
                let mut main = div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).child(
                    div().min_w_0().overflow_hidden().child(text(title.clone()).size(12.5).line_height(LineHeight::Scale(1.35)).color(t.text).truncate()),
                );
                if !a.created_by.is_empty() || when.is_some() {
                    let mut sub = div().flex().items_center().gap(px(6.)).min_w_0();
                    if !a.created_by.is_empty() {
                        sub = sub.child(label(a.created_by.clone(), Family::Mono, 10.5, 600.).tracking(0.02).color(t.muted));
                    }
                    if let Some(w) = &when {
                        sub = sub.child(recur_ui::tooltip::titled(
                            SharedString::from(format!("{rk}-aw-{ai}")),
                            w.full.clone(),
                            label(w.rel.clone(), Family::Ui, 11., 400.).tabular().color(t.faint),
                        ));
                    }
                    main = main.child(sub);
                }
                let mut row = div()
                    .id(SharedString::from(format!("{rk}-ar-{ai}")))
                    .flex()
                    .items_center()
                    .gap_x(px(8.))
                    .w_full()
                    .px(px(10.))
                    .py(px(8.))
                    .child(ico(IconName::from_key(artifact_icon(&a.kind)).unwrap_or(IconName::Grid), 14., t.muted))
                    .child(main);
                if !a.kind.is_empty() {
                    row = row.child(Pill { fg: t.faint, ..Pill::status(&t, 0.06) }.render(a.kind.clone()).self_center());
                }
                let tip = if can_open {
                    if a.path.is_empty() { title.clone() } else { format!("Open {title}") }
                } else if !a.path.is_empty() {
                    a.path.clone()
                } else {
                    title.clone()
                };
                let el = if can_open {
                    let art = OpenArtifact {
                        id: a.id.clone(),
                        title: a.title.clone(),
                        kind: a.kind.clone(),
                        path: a.path.clone(),
                        created_at: a.created_at.clone(),
                        created_by: a.created_by.clone(),
                    };
                    row.cursor_pointer()
                        .hover(|s| s.bg(hsla(t.pane_2)))
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(ChatEvent::OpenArtifact(art.clone()))))
                        .into_any_element()
                } else {
                    row.into_any_element()
                };
                let el = recur_ui::tooltip::titled(SharedString::from(format!("{rk}-at-{ai}")), tip, el).into_any_element();
                finding_row_wrap(el, ai + 1 == n, &t)
            })
            .collect();
        div().flex().flex_col().child(head).child(findings_list(SharedString::from(format!("{rk}-al")), rows, 360.)).into_any_element()
    }

    // ---- thinking / compaction / fallback / notice / narration ----

    /// `thinkingCard`.
    pub(crate) fn thinking_card(&mut self, m: &AgentMsg, key: &str, first_checkpoint: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let expandable = !m.body.is_empty();
        let open = expandable && self.row_is_open(key, first_checkpoint && m.streaming);
        let id = SharedString::from(format!("th-{}-{key}", self.props.reset_key));
        let group = SharedString::from(format!("{id}-g"));
        let mut kids = vec![
            self.tc_ico(&group, StepIcon::Think, t.faint, expandable, open),
            self.tc_label(if m.streaming { "Thinking" } else { "Thought" }, t.muted),
        ];
        if expandable && !open {
            kids.push(self.tc_meta(thinking_preview(m), t.faint));
        } else if !expandable && m.tokens > 0 {
            kids.push(self.tc_meta(format!("~{} tokens", m.tokens), t.faint));
        }
        let k = key.to_string();
        let head = self.tc_head(id.clone(), expandable, kids, Some(std::rc::Rc::new(move |this: &mut Self, cx: &mut Context<Self>| this.toggle_row(&k, open, cx))), cx);
        let mut card = div().rounded(px(7.)).child(head);
        if open && m.streaming && expandable {
            let rk = format!("{}:{key}", self.props.reset_key);
            let body = self.stream(format!("{rk}-think"), &m.body, Some(muted_sheet()), cx);
            card = card.child(self.live_think(&rk, body, cx));
        } else if open {
            let rk = format!("{}:{key}", self.props.reset_key);
            let md = self.md(format!("{rk}-thought"), &m.body, Some(muted_sheet()), false, cx);
            card = card.child(div().mt(px(3.)).mb(px(6.)).min_w_0().child(div().p(px(2.)).child(md)));
        }
        card.into_any_element()
    }

    /// `.tc-think-live`: ~4 lines pinned to the newest text, faded at the top.
    fn live_think(&mut self, rk: &str, body: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let handle = self
            .think_scrolls
            .entry(rk.to_string())
            .or_insert_with(gpui_kit::ScrollHandle::new)
            .clone();
        let max = handle.max_offset().y;
        if handle.offset().y != -max {
            handle.set_offset(gpui_kit::point(px(0.), -max));
            cx.notify();
        }
        let backdrop = self.backdrop(&t);
        let fade = 1.4 * 13.;
        div()
            .relative()
            .mt(px(2.))
            .mb(px(8.))
            .child(
                div()
                    .id(SharedString::from(format!("{rk}-thinkwin")))
                    .track_scroll(&handle)
                    .overflow_y_scroll()
                    .max_h(px(13. * 1.6 * 4.25))
                    .pt(px(2.))
                    .px(px(2.))
                    .child(body),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(fade))
                    .bg(gpui_kit::linear_gradient(
                        180.,
                        gpui_kit::linear_color_stop(hsla(backdrop), 0.),
                        gpui_kit::linear_color_stop(hsla(backdrop.with_alpha(0.)), 1.),
                    )),
            )
            .into_any_element()
    }

    /// `compactCard`.
    pub(crate) fn compact_card(&mut self, m: &AgentMsg, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let expandable = !m.body.is_empty() && !m.compacting;
        let open = expandable && self.open_rows.get(key).copied().unwrap_or(false);
        let id = SharedString::from(format!("cc-{}-{key}", self.props.reset_key));
        let group = SharedString::from(format!("{id}-g"));
        let mut kids = vec![
            self.tc_ico(&group, StepIcon::Compact, t.amber, expandable, open),
            self.tc_label(
                if m.compacting { "Compacting context" } else { "Context compacted" },
                if m.compacting { t.amber } else { t.muted },
            ),
        ];
        if m.tokens > 0 {
            kids.push(self.tc_meta(format!("was {}", format_tokens(m.tokens)), t.faint));
        }
        let k = key.to_string();
        let head = self.tc_head(id, expandable, kids, Some(std::rc::Rc::new(move |this: &mut Self, cx: &mut Context<Self>| this.toggle_row(&k, open, cx))), cx);
        let mut card = div().rounded(px(7.)).child(head);
        if open {
            let rk = format!("{}:{key}", self.props.reset_key);
            let md = self.md(format!("{rk}-compact"), &m.body, Some(muted_sheet()), false, cx);
            card = card.child(div().mt(px(3.)).mb(px(6.)).min_w_0().child(div().p(px(2.)).child(md)));
        }
        card.into_any_element()
    }

    /// `fallbackCard`.
    pub(crate) fn fallback_card(&mut self, m: &AgentMsg, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let id = SharedString::from(format!("fb-{}-{}", self.props.reset_key, m.body));
        let group = SharedString::from(format!("{id}-g"));
        let mut kids = vec![
            self.tc_ico(&group, StepIcon::Fallback, t.amber, false, false),
            self.tc_label(
                format!("Fell back to {}", if m.body.is_empty() { "another model" } else { &m.body }),
                t.muted,
            ),
        ];
        if !m.description.is_empty() {
            kids.push(self.tc_meta(format!("from {}", m.description), t.faint));
        }
        let head = self.tc_head(id, false, kids, None, cx);
        div().rounded(px(7.)).child(head).into_any_element()
    }

    /// `noticeCard`: the label and the (wrapping) notice.
    pub(crate) fn notice_card(&mut self, m: &AgentMsg, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let wrap = m.note_label == "Commit" || m.note_label == "Handoff";
        let id = SharedString::from(format!("nt-{}-{key}", self.props.reset_key));
        let group = SharedString::from(format!("{id}-g"));
        let label_s = if m.note_label.is_empty() { "Notice".to_string() } else { m.note_label.clone() };
        let meta: AnyElement = if m.note_html {
            crate::html_inline::inline_html(
                SharedString::from(format!("{id}-html")),
                &m.body,
                crate::html_inline::InlineStyle::meta(11., t.muted, &t),
            )
        } else {
            text(m.body.clone()).id(SharedString::from(format!("{id}-t"))).size(11.).color(t.muted).into_any_element()
        };
        let kids = vec![
            div()
                .relative()
                .flex_none()
                .size(px(14.))
                .top(px(1.))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::Activity).size(px(14.)).color(hsla(t.muted)))
                .into_any_element(),
            self.tc_label(label_s, t.muted),
            div().flex_1().min_w_0().child(meta).into_any_element(),
        ];
        let head = self.tc_head(id, false, kids, None, cx);
        div().rounded(px(7.)).when(wrap, |d| d.my(px(2.))).child(head).into_any_element()
    }

    /// `narrationRow`: an inner aside folded into the step group.
    pub(crate) fn narration_row(&mut self, m: &AgentMsg, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let expandable = !m.body.is_empty();
        let open = expandable && self.row_is_open(key, false);
        let id = SharedString::from(format!("nr-{}-{key}", self.props.reset_key));
        let kids = vec![self.tc_label(prose_preview(&m.body), t.text)];
        let k = key.to_string();
        let head = self.tc_head(id, expandable, kids, Some(std::rc::Rc::new(move |this: &mut Self, cx: &mut Context<Self>| this.toggle_row(&k, open, cx))), cx);
        let mut card = div().rounded(px(7.)).child(head);
        if open {
            let rk = format!("{}:{key}", self.props.reset_key);
            let md = self.md(format!("{rk}-narr"), &m.body, None, false, cx);
            card = card.child(div().mt(px(3.)).mb(px(6.)).min_w_0().child(md));
        }
        card.into_any_element()
    }
}

/// `hasLiveChildren`.
pub(crate) fn has_live_children(m: &AgentMsg) -> bool {
    m.children
        .iter()
        .any(|c| c.streaming || c.result_pending || has_live_children(c))
}

/// `.tc-finding-msg`: padded, scrolling markdown.
fn finding_msg(md: AnyElement, max_h: f32) -> AnyElement {
    div()
        .id(gpui_kit::ElementId::Name(format!("fm-{max_h}").into()))
        .px(px(10.))
        .py(px(8.))
        .max_h(px(max_h))
        .overflow_y_scroll()
        .child(md)
        .into_any_element()
}

/// `.tc-finding-empty`.
fn finding_empty(s: impl Into<SharedString>, t: &Tokens) -> AnyElement {
    div()
        .px(px(10.))
        .py(px(12.))
        .text_size(px(12.5))
        .text_color(hsla(t.faint))
        .child(text(s))
        .into_any_element()
}

/// `.tc-finding-reason` / `.tc-plan-update-note` / `.tc-web-prompt`: a mono label then text.
pub(crate) fn labeled_note(
    tag: &str,
    body: &str,
    size: f32,
    leading: f32,
    margin: (f32, f32, f32, f32),
    t: &Tokens,
) -> AnyElement {
    div()
        .mt(px(margin.0))
        .mr(px(margin.1))
        .mb(px(margin.2))
        .ml(px(margin.3))
        .child(
            Text::from_spans(vec![
                span(tag.to_string())
                    .mono()
                    .size(10.)
                    .weight(600.)
                    .color(t.faint)
                    .uppercase()
                    .tracking(0.04)
                    .boxed(recur_ui::text::InlineBox {
                        margin_right: 6.,
                        ..Default::default()
                    }),
                span(" "),
                span(body.to_string()),
            ])
            .size(size)
            .line_height(LineHeight::Scale(leading))
            .color(t.muted),
        )
        .into_any_element()
}

/// `.tc-findings-head`.
fn findings_head(t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .flex_wrap()
        .px(px(10.))
        .py(px(8.))
        .border_b_1()
        .border_color(hsla(t.line_soft))
        .bg(hsla(t.pane_2.mix(0.45, recur_ui::color::TRANSPARENT)))
}

fn findings_count(s: impl Into<SharedString>, t: &Tokens) -> impl IntoElement {
    label(s, Family::Ui, 12., 600.).tracking(-0.01).color(t.text)
}

/// `.tc-findings-list`: a 360px scroller (unbounded inside the turn digest).
pub(crate) fn findings_list(id: SharedString, rows: Vec<AnyElement>, max_h: f32) -> AnyElement {
    div().id(id).max_h(px(max_h)).overflow_y_scroll().flex().flex_col().children(rows).into_any_element()
}

/// `.tc-finding-row`: a bottom hairline except the last.
pub(crate) fn finding_row_wrap(el: AnyElement, last: bool, t: &Tokens) -> AnyElement {
    div()
        .when(!last, |d| d.border_b_1().border_color(hsla(t.line_soft)))
        .child(el)
        .into_any_element()
}

/// `.tc-finding-detail`.
pub(crate) fn accord_detail(t: &Tokens) -> Div {
    div()
        .pt(px(8.))
        .pr(px(10.))
        .pb(px(10.))
        .pl(px(32.))
        .border_t_1()
        .border_color(hsla(t.line_soft))
        .bg(hsla(t.bg))
        .flex()
        .flex_col()
        .gap(px(8.))
        .overflow_hidden()
}

fn lin_dot(css: &str, t: &Tokens) -> AnyElement {
    let c = Color::parse(css).unwrap_or(t.muted);
    let ring = t.text.mix(0.14, recur_ui::color::TRANSPARENT);
    div()
        .flex_none()
        .self_center()
        .size(px(8.))
        .rounded_full()
        .border_1()
        .border_color(hsla(ring))
        .child(div().size_full().rounded_full().bg(hsla(c)))
        .into_any_element()
}

fn grep_flag(s: impl Into<SharedString>, t: &Tokens) -> AnyElement {
    Pill {
        size: 10.,
        line: 15.,
        pad_x: 5.,
        radius: 4.,
        ..Pill::chip(t)
    }
    .render(s)
    .into_any_element()
}

/// `.tc-pre.tc-grepq`.
fn grepq(kids: Vec<AnyElement>) -> AnyElement {
    div()
        .px(px(10.))
        .py(px(8.))
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap_x(px(7.))
        .gap_y(px(4.))
        .text_size(px(11.))
        .line_height(relative(1.55))
        .children(kids)
        .into_any_element()
}

/// `.cv`: the mini code viewer.
fn cv(id: SharedString, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .id(id)
        .py(px(6.))
        .max_h(px(360.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

fn cv_row(gutter: impl IntoElement, code: AnyElement, bg: Option<Color>) -> AnyElement {
    div()
        .flex()
        .min_w_0()
        .pl(px(6.))
        .pr(px(10.))
        .when_some(bg, |d, c| d.bg(hsla(c)))
        .child(gutter)
        .child(div().flex_1().min_w_0().child(code))
        .into_any_element()
}

/// One highlighted line (`highlightLine`).
fn code_line(line: &str, lang: &str, t: &Tokens) -> AnyElement {
    let spans: Vec<Span> = highlight_line_spans(line, lang)
        .into_iter()
        .map(|(r, k)| {
            let s = span(line[r].to_string());
            match k.and_then(|k| sh_color(k.class())) {
                Some(c) => s.color(c),
                None => s,
            }
        })
        .collect();
    let spans = if spans.is_empty() { vec![span(" ")] } else { spans };
    Text::from_spans(spans)
        .mono()
        .size(11.)
        .line_height(LineHeight::Scale(1.55))
        .color(t.text)
        .pre_wrap()
        .break_anywhere()
        .into_any_element()
}

/// `.tc-plan-steps`.
fn plan_steps(steps: &[recur_agent_tools::display::plan_tools::PlanStepView], rk: &str, t: &Tokens) -> AnyElement {
    div()
        .id(SharedString::from(format!("{rk}-psteps")))
        .pt(px(6.))
        .pb(px(8.))
        .flex()
        .flex_col()
        .gap(px(5.))
        .max_h(px(360.))
        .overflow_y_scroll()
        .children(steps.iter().map(|s| {
            let done = s.status == "done" || s.status == "completed";
            let skipped = s.status == "skipped";
            let mut title = text(s.title.clone()).id(SharedString::from(format!("{rk}-ps-{}", s.id))).break_anywhere();
            if done {
                title = title.color(t.muted).line_through(true);
            } else if skipped {
                title = title.color(t.faint).italic();
            }
            div()
                .flex()
                .items_baseline()
                .gap(px(7.))
                .px(px(10.))
                .text_size(px(12.))
                .line_height(relative(1.45))
                .text_color(hsla(t.text))
                .child(div().flex_none().child(label(plan_step_mark(&s.status), Family::Ui, 12., 400.).line_height(LineHeight::Scale(1.)).color(t.amber)))
                .child(div().flex_none().child(label(s.id.clone(), Family::Mono, 10.5, 400.).color(t.faint)))
                .child(div().min_w_0().child(title))
        }))
        .into_any_element()
}

/// `.todo-row`: a checklist row (todos and Claude tasks).
fn todo_row(
    content: &str,
    status: &str,
    id: Option<&str>,
    extras: &[String],
    _key: &str,
    blocked: bool,
    t: &Tokens,
) -> AnyElement {
    let done = status == "completed";
    let active = status == "in_progress";
    let _ = blocked;
    let mut bx = div()
        .flex_none()
        .size(px(14.))
        .mt(px(2.))
        .rounded(px(4.))
        .border(px(1.5))
        .border_color(hsla(t.line_bright))
        .flex()
        .items_center()
        .justify_center();
    if done {
        bx = bx
            .bg(hsla(t.emerald))
            .border_color(hsla(t.emerald))
            .child(icon(IconName::Check).size(px(10.)).stroke_width(3.).color(hsla(crate::style::INK_2)));
    } else if active {
        bx = bx.border_color(hsla(t.amber)).child(
            div().absolute().top_0().left_0().bottom_0().w(relative(0.5)).bg(hsla(t.amber)),
        );
    }
    let mut spans = vec![span(content.to_string())];
    for e in extras {
        spans.push(span(" "));
        spans.push(
            span(e.clone())
                .size(11.)
                .weight(400.)
                .color(t.faint)
                .boxed(recur_ui::text::InlineBox {
                    margin_left: 6.,
                    ..Default::default()
                }),
        );
    }
    let mut tx = Text::from_spans(spans).size(12.5).line_height(LineHeight::Scale(1.45));
    tx = if done {
        tx.color(t.muted).line_through(true)
    } else if active {
        tx.color(t.text).weight(500.)
    } else {
        tx.color(t.text)
    };
    let mut row = div()
        .flex()
        .items_start()
        .gap(px(9.))
        .px(px(2.))
        .py(px(3.))
        .child(bx.relative().overflow_hidden());
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        row = row.child(div().flex_none().mt(px(2.)).child(label(format_task_id(id), Family::Mono, 10.5, 400.).tabular().color(t.cyan)));
    }
    row.child(div().min_w_0().child(tx)).into_any_element()
}

fn claude_task_item(td: &ClaudeTaskView, t: &Tokens) -> AnyElement {
    let mut extras = Vec::new();
    if !td.owner.is_empty() {
        extras.push(td.owner.clone());
    }
    if !td.blocked_by.is_empty() {
        extras.push(format!(
            "blocked by {}",
            td.blocked_by.iter().map(|i| format_task_id(i)).collect::<Vec<_>>().join(", ")
        ));
    }
    todo_row(&td.subject, &td.status, Some(&td.id), &extras, "", !td.blocked_by.is_empty(), t)
}

/// `.tc-plan-update` with Owner / Blocked by / Blocks notes.
fn task_relations(owner: &str, blocked_by: &[String], blocks: &[String], t: &Tokens) -> AnyElement {
    let ids = |v: &[String]| v.iter().map(|i| format_task_id(i)).collect::<Vec<_>>().join(", ");
    let mut col = div().px(px(10.)).py(px(8.)).flex().flex_col().gap(px(6.));
    if !owner.is_empty() {
        col = col.child(labeled_note("Owner", owner, 12., 1.4, (0., 0., 0., 0.), t));
    }
    if !blocked_by.is_empty() {
        col = col.child(labeled_note("Blocked by", &ids(blocked_by), 12., 1.4, (0., 0., 0., 0.), t));
    }
    if !blocks.is_empty() {
        col = col.child(labeled_note("Blocks", &ids(blocks), 12., 1.4, (0., 0., 0., 0.), t));
    }
    col.into_any_element()
}
