//! The Inbox overview pane in the app's own vocabulary: the PR header's state pill and side
//! pills, the board's list cards (header row, ruled rows, status on the right), the PR
//! checks card, the Changes view's file rows and the Inbox notification rows. Ordered by
//! what to do next; empty parts are left out. Talking to the workspace goes through Ask
//! Recur, which knows this workspace is open.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Div, InteractiveElement, IntoElement, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_content::{MarkdownClass, MarkdownView};
use recur_domain::{InboxItem, WorkspaceBrief, WorkspaceOverview};
use recur_forge::view::{checks_box_byline, group_checks};
use recur_ui::file_icons::file_icon;
use recur_ui::guard::Guarded;
use recur_ui::shadow::{box_shadow, css_box_shadow};
use recur_ui::style::{Tbtn, TbtnSize, button_label, tbtn};
use recur_ui::text::{LineHeight, TextStyled, span, text};
use recur_ui::widgets::project_pic::{PicBox, project_pic};
use recur_ui::{Color, IconName, Theme, Tokens, icon, rgba};
use recur_view_chat::cards::permission::PermissionCard;
use recur_view_chat::cards::question::{QuestionCard, QuestionLike};
use recur_view_chat::html_inline::{InlineStyle, inline_html_text};
use recur_view_chat::style::finding_sev;
use recur_view_core::pr::{PrLifecycle, pr_for_workspace, pr_icon_name, pr_lifecycle};
use recur_view_review::style::note_btn;
use recur_view_widgets::findings_ui::{finding_loc_label, finding_severity_tally, finding_title};

use super::{BriefingView, row_icon};
use crate::style::{hsla, ity};

/// Height of a long latest reply before "Show more".
const REPLY_CLAMP: f32 = 280.;
/// Height of a long plan brief before "Show full plan".
const PLAN_BODY_CLAMP: f32 = 320.;
/// Files, plan and finding rows a card lists before its disclosure.
const FILES_SHOWN: usize = 8;
const FINDINGS_SHOWN: usize = 5;
const PLAN_SHOWN: usize = 6;
const PIC: PicBox = PicBox {
    size: 14.,
    radius: 4.,
    font_size: 8.,
};
/// `note_btn`'s line height (11px label).
const NOTE_LH: f32 = 13.;

/// Collapsible overview parts.
#[derive(Clone, Copy, Default)]
pub(super) struct Open {
    pub reply: bool,
    pub steps: bool,
    pub plan: bool,
    pub files: bool,
    pub findings: bool,
    pub plan_body: bool,
}

/// The board's amber running dot with its glow.
fn running_dot(t: &Tokens) -> AnyElement {
    div()
        .relative()
        .size(px(6.))
        .child(box_shadow(
            &[css_box_shadow(0., 0., 5., 0., t.amber)],
            3.,
            0.,
        ))
        .child(div().absolute().inset_0().rounded_full().bg(hsla(t.amber)))
        .into_any_element()
}

/// The PR header's state pill: a dot and mono label on a tinted wash.
fn state_pill(color: Color, label: &str, live: bool, t: &Tokens) -> Div {
    let dot = if live {
        running_dot(t)
    } else {
        div()
            .size(px(6.))
            .rounded_full()
            .bg(hsla(color))
            .into_any_element()
    };
    div()
        .debug_selector(|| "ov-state".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .py(px(3.))
        .px(px(10.))
        .rounded(px(20.))
        .bg(hsla(color.mix(0.14, recur_ui::color::TRANSPARENT)))
        .child(dot)
        .child(
            text(label.to_string())
                .mono()
                .size(11.)
                .weight(500.)
                .color(color)
                .nowrap(),
        )
}

/// A board-style list card's header row: icon, bold title, muted meta.
fn card_head(
    lead: AnyElement,
    title: impl Into<SharedString>,
    meta: Option<String>,
    t: &Tokens,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .px(px(10.))
        .py(px(7.))
        .text_size(px(12.))
        .text_color(hsla(t.muted))
        .child(
            div()
                .flex_none()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .child(lead),
        )
        .child(
            div()
                .min_w_0()
                .child(text(title).size(12.).weight(600.).color(t.text).truncate()),
        )
        .when_some(meta, |d, m| {
            d.child(text(m).size(12.).color(t.muted).nowrap())
        })
}

fn list_card(t: &Tokens) -> Div {
    recur_ui::style::card(t.pane, t.line, 8.)
        .overflow_hidden()
        .flex()
        .flex_col()
}

/// A ruled row inside a list card.
fn card_row(first: bool, t: &Tokens) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .w_full()
        .px(px(10.))
        .py(px(6.))
        .text_size(px(12.))
        .when(!first, |d| d.border_t_1().border_color(hsla(t.line_soft)))
}

/// The transcript's expand/collapse row (`.steps-head`): a faint chevron that turns when
/// open, muted 11.5px text that brightens on hover.
fn disclosure(id: &'static str, label: String, open: bool, t: &Tokens) -> Stateful<Div> {
    let (muted, text_c) = (hsla(t.muted), hsla(t.text));
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .items_center()
        .gap(px(6.))
        .w_full()
        .px(px(10.))
        .py(px(6.))
        .cursor_pointer()
        .text_size(px(11.5))
        .text_color(muted)
        .hover(move |s| s.text_color(text_c))
        .child(
            div().flex_none().size(px(12.)).child(
                icon(IconName::ChevronRight)
                    .size(px(12.))
                    .stroke_width(2.4)
                    .color(hsla(t.faint))
                    .rotate(if open { 90. } else { 0. }),
            ),
        )
        .child(text(label).line_height(LineHeight::Scale(1.45)).nowrap())
}

/// A quiet section label (`Latest reply`, `Steps`): muted 11.5px, medium weight.
fn section_label(label: &str, t: &Tokens) -> AnyElement {
    text(label.to_string())
        .size(11.5)
        .weight(500.)
        .color(t.muted)
        .nowrap()
        .into_any_element()
}

fn age(ms: i64) -> Option<String> {
    recur_engine::view::artifacts::format_relative_age(ms, chrono::Utc::now().timestamp_millis())
        .map(|a| a.rel)
}

fn diff_stat(add: i64, del: i64, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_none()
        .gap(px(5.))
        .when(add > 0, |d| {
            d.child(text(format!("+{add}")).mono().size(10.5).color(t.git_a))
        })
        .when(del > 0, |d| {
            d.child(text(format!("−{del}")).mono().size(10.5).color(t.git_d))
        })
}

/// The PR merge button's emerald at note-button size.
fn merge_btn(id: &'static str, label: &'static str, t: &Tokens) -> Stateful<Div> {
    let ink = rgba(10, 12, 16, 1.);
    let lit = t.emerald.brightness(1.06);
    div()
        .id(id)
        .debug_selector(move || id.into())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(24.))
        .px(px(9.))
        .rounded(px(6.))
        .bg(hsla(t.emerald))
        .text_color(hsla(ink))
        .cursor_pointer()
        .hover(move |s| s.bg(hsla(lit)))
        .child(icon(IconName::GitMerge).size(px(12.)))
        .child(button_label(label, 11.5, 600.))
}

/// One plan step as the chat's plan card draws it: status mark, wrapping title (struck
/// through once settled), the files it touches as small mono chips.
fn plan_step_row(i: usize, p: &recur_domain::PlanStep, t: &Tokens) -> AnyElement {
    let done_like = p.status == "done" || p.status == "skipped";
    let mark: AnyElement = match p.status.as_str() {
        "done" => icon(IconName::CircleCheck)
            .size(px(13.))
            .color(hsla(t.emerald))
            .into_any_element(),
        "skipped" => icon(IconName::CircleSlash)
            .size(px(13.))
            .color(hsla(t.faint))
            .into_any_element(),
        "doing" => running_dot(t),
        _ => div().into_any_element(),
    };
    let mut title = text(p.title.clone()).size(12.).leading(1.45);
    title = if done_like {
        title.color(t.muted).line_through(true)
    } else {
        title.color(t.text)
    };
    let mut body = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(title);
    if !p.files.is_empty() {
        let mut chips = div().flex().flex_wrap().gap(px(4.));
        for f in &p.files {
            let name = f.rsplit('/').next().unwrap_or(f).to_string();
            chips = chips.child(
                div()
                    .bg(hsla(t.pane_2))
                    .rounded(px(3.))
                    .px(px(5.))
                    .py(px(1.))
                    .child(text(name).mono().size(10.5).color(t.faint).nowrap()),
            );
        }
        body = body.child(chips);
    }
    div()
        .flex()
        .items_start()
        .gap(px(8.))
        .px(px(10.))
        .py(px(4.))
        .when(i == 0, |d| d.pt(px(7.)))
        .child(
            div()
                .flex_none()
                .size(px(14.))
                .mt(px(2.))
                .flex()
                .items_center()
                .justify_center()
                .child(mark),
        )
        .child(body)
        .into_any_element()
}

fn route(v: &mut BriefingView, target: &str, cx: &mut Context<BriefingView>) {
    let target = target.to_owned();
    v.vcx.state.update(cx, |s, cx| s.route_target(&target, cx));
}

impl BriefingView {
    pub(super) fn overview_pane(
        &mut self,
        brief: &WorkspaceBrief,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = Theme::of(cx).tokens;
        let ov = self.overviews.get(&brief.workspace_id).cloned();
        let head = self.overview_head(brief, ov.as_ref(), narrow, cx);
        let content: Vec<AnyElement> = match &ov {
            None => vec![text("Loading…").size(12.).color(t.faint).into_any_element()],
            Some(o) => self.overview_blocks(brief, o, window, cx),
        };
        div()
            .debug_selector(|| "briefing-overview".into())
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .bg(hsla(t.bg))
            .child(head)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        recur_ui::widgets::scroll_area("briefing-overview-scroll")
                            .track(&self.ov_scroll)
                            .scroller(|d| {
                                d.flex()
                                    .flex_col()
                                    .gap(px(14.))
                                    .px(px(18.))
                                    .pt(px(16.))
                                    .pb(px(24.))
                            })
                            .children(content.into_iter().map(|e| div().flex_none().child(e))),
                    )
                    .children(recur_ui::style::edge_fades(&self.ov_scroll, t.bg, 40.)),
            )
    }

    /// The pane's head: a toolbar level with the list's filter bar, then the title and the
    /// state pill with the crumb. The toolbar's rows fill from Open workspace leftwards.
    fn overview_head(
        &mut self,
        brief: &WorkspaceBrief,
        ov: Option<&WorkspaceOverview>,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = Theme::of(cx).tokens;
        let running = ov.is_some_and(|o| o.running);
        let (pill_color, pill_label) = if running {
            (t.amber, "Working")
        } else {
            match brief.bucket.as_str() {
                "needs" => (t.coral, "Needs you"),
                "pickup" => (t.indigo, "Pick up"),
                "working" => (t.amber, "Working"),
                _ => (t.muted, "Idle"),
            }
        };

        // `.crumb`: project / [branch] → base, mono 11.5 faint, truncating.
        let project = self.vcx.state.read(cx).project(&brief.project_id).cloned();
        let mut spans = Vec::new();
        if let Some(p) = &project {
            spans.push(span(p.name.clone()).color(t.muted).weight(500.));
        }
        if !brief.branch.is_empty() {
            if !spans.is_empty() {
                spans.push(span(" / "));
            }
            spans.push(span(brief.branch.clone()).color(t.muted).size(10.5).boxed(
                recur_ui::text::InlineBox {
                    border: 1.,
                    border_color: t.line.into(),
                    pad_top: 1.,
                    pad_right: 6.,
                    pad_bottom: 1.,
                    pad_left: 6.,
                    radius: 5.,
                    ..Default::default()
                },
            ));
            if let Some(base) = project
                .as_ref()
                .map(|p| p.base_branch.clone())
                .filter(|b| !b.is_empty())
            {
                spans.push(span("  →  "));
                spans.push(span(base).color(t.muted));
            }
        }
        let crumb = div()
            .flex()
            .flex_1()
            .items_center()
            .gap(px(8.))
            .min_w(px(80.))
            .overflow_hidden()
            .when_some(project.as_ref(), |d, p| {
                d.child(div().flex_none().child(project_pic(
                    p.pic.clone(),
                    p.color.clone(),
                    p.name.clone(),
                    PIC,
                )))
            })
            .child(
                div().flex_initial().min_w_0().overflow_hidden().child(
                    recur_ui::text::Text::from_spans(spans)
                        .mono()
                        .size(11.5)
                        .color(t.faint)
                        .truncate(),
                ),
            );

        let unread = self.is_unread(&brief.workspace_id, cx);
        let mut actions = div()
            .flex()
            .flex_1()
            .flex_row_reverse()
            .flex_wrap()
            .min_w_0()
            .items_center()
            .gap(px(6.));
        actions = actions.child(
            tbtn("ov-open", Tbtn::Default, TbtnSize::TITLEBAR, None, cx)
                .max_w_full()
                .debug_selector(|| "ov-open".into())
                .child(button_label("Open workspace", 11.5, 500.).truncate())
                .on_click(cx.guarded_listener(|v, _, _, cx| v.open_selected(cx))),
        );
        if let Some(pr) = self.brief_pr(brief, cx) {
            let target = format!("pr:{}", pr.id);
            let label = if pr.draft {
                format!("View draft #{}", pr.number)
            } else {
                format!("View PR #{}", pr.number)
            };
            actions = actions.child(recur_ui::tooltip::titled(
                "ov-pr-tip",
                format!("{} · {}", pr.title, pr.state),
                tbtn("ov-pr", Tbtn::Default, TbtnSize::TITLEBAR, None, cx)
                    .max_w_full()
                    .debug_selector(|| "ov-pr".into())
                    .child(icon(pr_icon_name(Some(&pr))).size(px(13.)))
                    .child(button_label(label, 11.5, 500.).truncate())
                    .on_click(cx.guarded_listener(move |v, _, _, cx| route(v, &target, cx))),
            ));
        }
        actions = actions.child(recur_ui::tooltip::titled(
            "ov-read-tip",
            if unread {
                "Mark as read (⇧⌘U)"
            } else {
                "Mark as unread (⇧⌘U)"
            },
            tbtn("ov-read", Tbtn::Default, TbtnSize::TITLEBAR, None, cx)
                .max_w_full()
                .debug_selector(move || {
                    format!("ov-read:{}", if unread { "unread" } else { "read" })
                })
                .child(
                    button_label(if unread { "Mark read" } else { "Mark unread" }, 11.5, 500.)
                        .truncate(),
                )
                .on_click(cx.guarded_listener(|v, _, _, cx| v.toggle_read(cx))),
        ));
        let toolbar = crate::style::filters_bar(cx)
            .h_auto()
            .px(px(14.))
            .py(px(4.))
            .gap(px(12.))
            .when(narrow, |d| {
                d.child(
                    div()
                        .id("ov-close")
                        .debug_selector(|| "ov-close".into())
                        .flex_none()
                        .p(px(3.))
                        .ml(px(-4.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_color(hsla(t.muted))
                        .hover(move |s| s.bg(hsla(t.pane_2)).text_color(hsla(t.text)))
                        .child(icon(IconName::ChevronLeft).size(px(15.)))
                        .on_click(cx.guarded_listener(|v, _, _, cx| v.clear_selection(cx))),
                )
            })
            .child(actions);

        let title = div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .px(px(18.))
            .pt(px(13.))
            .pb(px(12.))
            .border_b_1()
            .border_color(hsla(t.line))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .debug_selector({
                                let t = brief.title.clone();
                                move || format!("ov-title:{t}")
                            })
                            .child(
                                text(brief.title.clone())
                                    .size(16.)
                                    .weight(600.)
                                    .tracking(-0.01)
                                    .leading(1.35)
                                    .color(t.text),
                            ),
                    )
                    .when_some(age(brief.last_activity_at).filter(|_| !running), |d, a| {
                        d.child(text(a).mono().size(10.5).color(t.faint).nowrap())
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(10.))
                    .gap_y(px(6.))
                    .child(state_pill(pill_color, pill_label, running, &t))
                    .child(crumb),
            );
        div()
            .flex()
            .flex_none()
            .flex_col()
            .child(toolbar)
            .child(title)
    }

    fn overview_blocks(
        &mut self,
        brief: &WorkspaceBrief,
        o: &WorkspaceOverview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = Theme::of(cx).tokens;
        let ctx = &o.context;
        let wid = brief.workspace_id.clone();
        let mut out = Vec::new();

        let questions: Vec<_> = self
            .vcx
            .state
            .read(cx)
            .user_questions
            .iter()
            .filter(|q| q.workspace_id == wid)
            .cloned()
            .collect();
        let perms: Vec<_> = self
            .vcx
            .state
            .read(cx)
            .permission_requests
            .iter()
            .filter(|r| r.workspace_id == wid)
            .cloned()
            .collect();
        let plan_waiting = brief
            .reasons
            .as_ref()
            .is_some_and(|r| r.iter().any(|x| x == "plan_approval"));

        // What arrived since the last look, as the Inbox lists notifications. A question's
        // notification repeats its card below, and is stale once the question is answered.
        let notes: Vec<InboxItem> = self
            .seen_notes
            .iter()
            .filter(|n| !n.target.starts_with("question:"))
            .cloned()
            .collect();
        if !notes.is_empty() {
            let mut c = list_card(&t);
            for (i, n) in notes.iter().enumerate() {
                c = c.child(self.note_row(i, n, cx));
            }
            out.push(c.into_any_element());
        }

        for r in perms {
            let state = self.vcx.state.clone();
            let entity = self
                .permission_cards
                .entry(r.id.clone())
                .or_insert_with(|| cx.new(|cx| PermissionCard::new(r.clone(), true, state, cx)))
                .clone();
            if entity.read(cx).request != r {
                entity.update(cx, |c, cx| c.set_request(r, cx));
            }
            out.push(entity.into_any_element());
        }
        for q in questions {
            let state = self.vcx.state.clone();
            let entity = self
                .question_cards
                .entry(q.id.clone())
                .or_insert_with(|| {
                    let like = QuestionLike {
                        id: q.id.clone(),
                        question: q.question.clone(),
                        context: q.context.clone(),
                        options: q.options.clone(),
                    };
                    cx.new(|cx| QuestionCard::new(like, None, false, state, window, cx))
                })
                .clone();
            out.push(entity.into_any_element());
        }
        for (i, a) in ctx
            .open_asks
            .iter()
            .filter(|a| a.kind == "escalation")
            .enumerate()
        {
            let target = format!("workspace:{wid}");
            out.push(
                list_card(&t)
                    .child(
                        card_head(
                            icon(IconName::TriangleAlert)
                                .size(px(14.))
                                .color(hsla(t.coral))
                                .into_any_element(),
                            "Escalated",
                            None,
                            &t,
                        )
                        .border_b_1()
                        .border_color(hsla(t.line))
                        .child(
                            div().ml_auto().child(
                                note_btn(
                                    SharedString::from(format!("ov-escalation-{i}")),
                                    "Review",
                                    true,
                                    false,
                                    t.pane,
                                    NOTE_LH,
                                    cx,
                                )
                                .on_click(
                                    cx.guarded_listener(move |v, _, _, cx| route(v, &target, cx)),
                                ),
                            ),
                        ),
                    )
                    .child(
                        card_row(true, &t).py(px(8.)).child(
                            text(a.summary.clone())
                                .size(12.5)
                                .leading(1.45)
                                .color(t.text),
                        ),
                    )
                    .into_any_element(),
            );
        }
        if plan_waiting {
            out.push(self.plan_card(brief, o, true, cx));
        }

        if !o.open_findings.is_empty() {
            out.push(self.findings_card(&wid, o, cx));
        }

        if let Some(pr) = ctx.pr.as_ref().filter(|_| !o.failing_checks.is_empty()) {
            out.push(self.checks_card(brief, pr, o, cx));
        }

        if !o.running
            && !plan_waiting
            && (o.failing_checks.is_empty() || brief.is_pr_host)
            && let Some(ship) = self.ship_card(brief, cx)
        {
            out.push(ship);
        }

        if o.running {
            out.push(self.live_card(brief, o, cx));
        }

        if !o.last_reply.is_empty() {
            let md = match &self.reply_md {
                Some((src, v)) if *src == o.last_reply => v.clone(),
                _ => {
                    let src = o.last_reply.clone();
                    let v = cx.new(|cx| {
                        MarkdownView::new(src.clone(), MarkdownClass::ChatCanvas, false, cx)
                    });
                    self.reply_md = Some((src, v.clone()));
                    v
                }
            };
            let long = o.last_reply.len() > 900 || o.last_reply.lines().count() > 14;
            let open = self.ov_open.reply || !long;
            let mut b = div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .pt(px(2.))
                .child(section_label("Latest reply", &t))
                .child(
                    div()
                        .when(!open, |d| d.max_h(px(REPLY_CLAMP)).overflow_hidden())
                        .child(md),
                );
            if long {
                b = b.child(
                    disclosure(
                        "ov-reply-more",
                        if open { "Show less" } else { "Show more" }.into(),
                        open,
                        &t,
                    )
                    .px(px(0.))
                    .on_click(cx.guarded_listener(|v, _, _, cx| {
                        v.ov_open.reply = !v.ov_open.reply;
                        cx.notify();
                    })),
                );
            }
            out.push(b.into_any_element());
        }

        if !ctx.plan_steps.is_empty() && !plan_waiting {
            out.push(self.plan_card(brief, o, false, cx));
        }

        if let Some(files) = self.files_card(brief, cx) {
            out.push(files);
        }

        if out.is_empty() {
            let line = match age(brief.last_activity_at) {
                Some(a) => format!("Nothing new since you looked. Last active {a}."),
                None => "Nothing new since you looked.".to_string(),
            };
            out.push(
                div()
                    .py(px(28.))
                    .flex()
                    .justify_center()
                    .child(text(line).size(12.5).color(t.muted))
                    .into_any_element(),
            );
        }
        out
    }

    /// One unread notification, as the Inbox's Elsewhere rows draw it.
    fn note_row(&mut self, i: usize, n: &InboxItem, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let color = Theme::of(cx).resolve_css_color(&n.color).unwrap_or(t.muted);
        let target = n.target.clone();
        card_row(i == 0, &t)
            .id(SharedString::from(format!("ov-note-{i}")))
            .debug_selector(|| format!("ov-note:{}", n.id))
            .gap(px(12.))
            .py(px(8.))
            .cursor_pointer()
            .hover(move |s| s.bg(hsla(t.pane_2)))
            .child(ity(color).when_some(row_icon(&n.kind), |d, ic| d.child(icon(ic).size(px(14.)))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(inline_html_text(
                        SharedString::from(format!("ov-note-t-{i}")),
                        &n.title,
                        InlineStyle {
                            bold_weight: 600.,
                            leading: Some(1.45),
                            ..InlineStyle::meta(12.5, t.text, &t)
                        },
                    ))
                    .when(!n.sub.is_empty(), |d| {
                        d.child(
                            text(n.sub.clone())
                                .mono()
                                .size(10.5)
                                .leading(1.45)
                                .color(t.faint),
                        )
                    }),
            )
            .child(
                text(n.time.clone())
                    .mono()
                    .size(10.)
                    .color(t.faint)
                    .nowrap(),
            )
            .on_click(cx.guarded_listener(move |v, _, _, cx| route(v, &target, cx)))
            .into_any_element()
    }

    /// Open findings as the chat's findings card lists them: a tinted head with the count
    /// and severity tally, then severity icon, title and cyan location per row. Fix runs
    /// the workspace's `/fix-findings` when its agent is idle; a row opens the Findings panel.
    fn findings_card(
        &mut self,
        wid: &str,
        o: &WorkspaceOverview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let all = &o.open_findings;
        let n = all.len();
        let panel = format!("workspace:{wid}:findings");
        let tally = finding_severity_tally(all.iter().map(|f| f.severity.as_str()));
        let mut right = div().ml_auto().flex().items_center().gap(px(8.));
        for x in &tally {
            right = right.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(finding_sev(&x.severity, 12., &t))
                    .child(
                        text(x.count.to_string())
                            .mono()
                            .size(10.5)
                            .tabular()
                            .color(t.muted),
                    ),
            );
        }
        if !o.running {
            let wid = wid.to_owned();
            right = right.child(recur_ui::tooltip::titled(
                "ov-fix-findings-tip",
                format!("Fix open findings ({n})"),
                note_btn("ov-fix-findings", "Fix", false, false, t.pane, NOTE_LH, cx)
                    .debug_selector(|| "ov-fix-findings".into())
                    .on_click(cx.guarded_listener(move |v, _, _, cx| {
                        v.send_to_workspace(&wid, "/fix-findings", cx)
                    })),
            ));
        }
        let mut c = list_card(&t).child(
            card_head(
                icon(IconName::HatGlasses)
                    .size(px(14.))
                    .color(hsla(t.warn))
                    .into_any_element(),
                format!("{n} open finding{}", if n == 1 { "" } else { "s" }),
                None,
                &t,
            )
            .border_b_1()
            .border_color(hsla(t.line))
            .child(right),
        );
        let open = self.ov_open.findings;
        for (i, f) in all
            .iter()
            .take(if open { usize::MAX } else { FINDINGS_SHOWN })
            .enumerate()
        {
            let sev = if f.severity.as_str().is_empty() {
                "low"
            } else {
                f.severity.as_str()
            };
            let loc = finding_loc_label(&f.file, f.line, f.end_line);
            let target = panel.clone();
            c = c.child(
                div()
                    .id(SharedString::from(format!("ov-finding-{i}")))
                    .debug_selector({
                        let m = f.message.clone();
                        move || format!("ov-finding:{m}")
                    })
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .w_full()
                    .px(px(10.))
                    .py(px(8.))
                    .when(i > 0, |d| d.border_t_1().border_color(hsla(t.line_soft)))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hsla(t.pane_2)))
                    .child(
                        div()
                            .flex_none()
                            .w(px(14.))
                            .mt(px(2.))
                            .flex()
                            .justify_center()
                            .child(finding_sev(sev, 14., &t)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                text(finding_title(&f.message))
                                    .size(12.5)
                                    .line_height(LineHeight::Scale(1.35))
                                    .color(t.text)
                                    .truncate(),
                            )
                            .when(!loc.is_empty(), |d| {
                                d.child(
                                    text(loc)
                                        .mono()
                                        .size(11.)
                                        .tabular()
                                        .color(t.cyan)
                                        .truncate(),
                                )
                            }),
                    )
                    .on_click(cx.guarded_listener(move |v, _, _, cx| route(v, &target, cx))),
            );
        }
        if n > FINDINGS_SHOWN {
            c = c.child(
                disclosure(
                    "ov-findings",
                    if open {
                        "Show fewer".into()
                    } else {
                        format!("{} more findings", n - FINDINGS_SHOWN)
                    },
                    open,
                    &t,
                )
                .border_t_1()
                .border_color(hsla(t.line_soft))
                .on_click(cx.guarded_listener(|v, _, _, cx| {
                    v.ov_open.findings = !v.ov_open.findings;
                    cx.notify();
                })),
            );
        }
        c.into_any_element()
    }

    /// The brief's PR: the one the workspace opened, or the PR it hosts a review for.
    fn brief_pr(&self, brief: &WorkspaceBrief, cx: &Context<Self>) -> Option<recur_domain::Pr> {
        let prs = &self.vcx.state.read(cx).prs;
        prs.iter()
            .find(|p| !brief.pr_id.is_empty() && p.id == brief.pr_id)
            .or_else(|| pr_for_workspace(prs, &brief.workspace_id))
            .cloned()
    }

    /// The PR checks card's shape: rollup heading, byline, Fix checks, failed rows.
    fn checks_card(
        &mut self,
        brief: &WorkspaceBrief,
        pr: &recur_domain::PrSummary,
        o: &WorkspaceOverview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let wid = brief.workspace_id.as_str();
        let all = self
            .brief_pr(brief, cx)
            .and_then(|p| p.checks)
            .unwrap_or_else(|| o.failing_checks.clone());
        let byline = checks_box_byline(&group_checks(&all));
        let sent = self.fix_sent.get(wid) == Some(&pr.checks);
        let mut msg = format!("PR #{} has failing checks:", pr.number);
        for ch in &o.failing_checks {
            msg.push_str(&format!("\n- {}", ch.name));
            if !ch.detail.trim().is_empty() {
                msg.push_str(&format!(": {}", ch.detail.trim()));
            }
            if !ch.url.is_empty() {
                msg.push_str(&format!(" ({})", ch.url));
            }
        }
        msg.push_str(
            "\n\nRead the failing logs, fix the cause, then commit and push. Do not skip or disable checks.",
        );
        let (wid, summary) = (wid.to_owned(), pr.checks.clone());
        // A review host's agent is not the PR's author: nothing to fix from here.
        let fix: AnyElement = if brief.is_pr_host {
            div().into_any_element()
        } else if sent {
            text("sent to agent")
                .mono()
                .size(10.)
                .color(t.faint)
                .nowrap()
                .into_any_element()
        } else {
            recur_ui::tooltip::titled(
                "ov-fix-checks-tip",
                "Ask this workspace's agent to fix the failing checks",
                note_btn(
                    "ov-fix-checks",
                    "Fix checks",
                    false,
                    false,
                    t.pane,
                    NOTE_LH,
                    cx,
                )
                .debug_selector(|| "ov-fix-checks".into())
                .on_click(cx.guarded_listener(move |v, _, _, cx| {
                    v.fix_sent.insert(wid.clone(), summary.clone());
                    v.send_to_workspace(&wid, &msg, cx);
                    cx.notify();
                })),
            )
            .into_any_element()
        };
        let mut c = list_card(&t).child(
            card_head(
                icon(IconName::X)
                    .size(px(14.))
                    .color(hsla(t.coral))
                    .into_any_element(),
                "Some checks failed",
                None,
                &t,
            )
            .border_b_1()
            .border_color(hsla(t.line))
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(text(byline).mono().size(10.).color(t.faint).nowrap())
                    .child(fix),
            ),
        );
        for (i, ch) in o.failing_checks.iter().enumerate() {
            c = c.child(
                card_row(i == 0, &t)
                    .py(px(8.))
                    .child(
                        icon(IconName::X)
                            .size(px(14.))
                            .stroke_width(2.4)
                            .color(hsla(t.coral)),
                    )
                    .child(text(ch.name.clone()).size(12.).weight(500.).nowrap())
                    .child(
                        div().flex_1().min_w_0().flex().justify_end().child(
                            text(ch.detail.trim().to_string())
                                .mono()
                                .size(10.)
                                .color(t.faint)
                                .truncate(),
                        ),
                    ),
            );
        }
        c.into_any_element()
    }

    /// The board card for a live workspace: who is on what, the latest step, Stop.
    fn live_card(
        &mut self,
        brief: &WorkspaceBrief,
        o: &WorkspaceOverview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let (who, task) = match o.context.agents.iter().find(|a| !a.task.is_empty()) {
            Some(a) => (a.name.clone(), a.task.clone()),
            None => ("Agent".to_string(), brief.now.clone()),
        };
        let wid = brief.workspace_id.clone();
        let mut c = list_card(&t).child(
            card_head(
                running_dot(&t),
                who,
                Some(task).filter(|s| !s.is_empty()),
                &t,
            )
            .border_b_1()
            .border_color(hsla(t.line))
            .child(
                div().ml_auto().child(
                    note_btn("ov-stop", "Stop", false, false, t.pane, NOTE_LH, cx)
                        .debug_selector(|| "ov-stop".into())
                        .on_click(cx.guarded_listener(move |v, _, _, cx| {
                            let id = wid.clone();
                            v.act(Some("Stopped"), cx, async move |s| {
                                s.briefing.stop_workspace(&id).await
                            });
                        })),
                ),
            ),
        );
        if let Some((last, earlier)) = o.recent_steps.split_last() {
            let open = self.ov_open.steps;
            if open {
                for (i, s) in earlier.iter().enumerate() {
                    c = c.child(
                        card_row(i == 0, &t)
                            .child(text(s.clone()).mono().size(11.).color(t.faint).truncate()),
                    );
                }
            }
            c = c.child(
                card_row(!open || earlier.is_empty(), &t)
                    .child(
                        div().flex_1().min_w_0().child(
                            text(last.clone())
                                .mono()
                                .size(11.)
                                .color(t.muted)
                                .truncate(),
                        ),
                    )
                    .child(text("now").mono().size(10.).color(t.amber).nowrap()),
            );
            if !earlier.is_empty() {
                let n = earlier.len();
                c = c.child(
                    disclosure(
                        "ov-steps",
                        if open {
                            "Hide earlier steps".to_string()
                        } else {
                            format!("{n} earlier step{}", if n == 1 { "" } else { "s" })
                        },
                        open,
                        &t,
                    )
                    .border_t_1()
                    .border_color(hsla(t.line_soft))
                    .on_click(cx.guarded_listener(|v, _, _, cx| {
                        v.ov_open.steps = !v.ov_open.steps;
                        cx.notify();
                    })),
                );
            }
        }
        c.into_any_element()
    }

    /// The plan as a board card; `approval` puts Review / Approve in its header.
    fn plan_card(
        &mut self,
        brief: &WorkspaceBrief,
        o: &WorkspaceOverview,
        approval: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let total = o.context.plan_steps.len();
        let done = o
            .context
            .plan_steps
            .iter()
            .filter(|p| p.status == "done" || p.status == "skipped")
            .count();
        let mut head = card_head(
            icon(IconName::ListTodo)
                .size(px(14.))
                .color(hsla(if approval { t.warn } else { t.muted }))
                .into_any_element(),
            if approval {
                "Plan ready for approval"
            } else {
                "Plan"
            },
            Some(format!("{total} step{}", if total == 1 { "" } else { "s" })),
            &t,
        )
        .border_b_1()
        .border_color(hsla(t.line));
        let mut footer: Option<Div> = None;
        if approval {
            let (id, review) = (
                brief.workspace_id.clone(),
                format!("workspace:{}:plan", brief.workspace_id),
            );
            footer = Some(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(hsla(t.line_soft))
                    .child(
                        note_btn(
                            "ov-review-plan",
                            "Edit in Plan panel",
                            false,
                            false,
                            t.pane,
                            NOTE_LH,
                            cx,
                        )
                        .on_click(cx.guarded_listener(move |v, _, _, cx| route(v, &review, cx))),
                    )
                    .child(
                        note_btn(
                            "ov-approve-plan",
                            "Approve plan",
                            true,
                            false,
                            t.pane,
                            NOTE_LH,
                            cx,
                        )
                        .debug_selector(|| "ov-approve-plan".into())
                        .on_click(cx.guarded_listener(
                            move |v, _, _, cx| {
                                let id = id.clone();
                                v.act(Some("Plan approved"), cx, async move |s| {
                                    s.workspaces.approve_plan(&id).await.map(|_| ())
                                });
                            },
                        )),
                    ),
            );
        } else {
            head = head.child(
                div().ml_auto().child(
                    text(format!("{done} of {total} done"))
                        .size(11.)
                        .color(t.muted)
                        .nowrap(),
                ),
            );
        }
        let mut c = list_card(&t).child(head);
        if approval {
            if let Some(brief_el) = self.plan_brief(o, cx) {
                c = c.child(brief_el);
            }
            c = c.child(
                div()
                    .px(px(10.))
                    .pt(px(10.))
                    .pb(px(2.))
                    .when(!o.plan_summary.is_empty() || !o.plan_body.is_empty(), |d| {
                        d.border_t_1().border_color(hsla(t.line_soft))
                    })
                    .child(section_label("Steps", &t)),
            );
        }
        // Approval lists every step: that is what gets signed off.
        let open = self.ov_open.plan || approval;
        let steps = &o.context.plan_steps;
        for (i, p) in steps
            .iter()
            .enumerate()
            .take(if open { usize::MAX } else { PLAN_SHOWN })
        {
            c = c.child(plan_step_row(i, p, &t));
        }
        if let Some(f) = footer {
            c = c.child(div().h(px(4.))).child(f);
        } else if total <= PLAN_SHOWN {
            c = c.child(div().h(px(6.)));
        } else {
            c = c.child(
                disclosure(
                    "ov-plan",
                    if self.ov_open.plan {
                        "Show fewer".into()
                    } else {
                        format!("{} more steps", total - PLAN_SHOWN)
                    },
                    self.ov_open.plan,
                    &t,
                )
                .border_t_1()
                .border_color(hsla(t.line_soft))
                .on_click(cx.guarded_listener(|v, _, _, cx| {
                    v.ov_open.plan = !v.ov_open.plan;
                    cx.notify();
                })),
            );
        }
        c.into_any_element()
    }

    /// The plan being approved: its headline, then the markdown brief, clamped with a
    /// toggle when long.
    fn plan_brief(&mut self, o: &WorkspaceOverview, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        if o.plan_summary.is_empty() && o.plan_body.is_empty() {
            return None;
        }
        let mut col = div().flex().flex_col().gap(px(8.)).px(px(12.)).py(px(10.));
        if !o.plan_summary.is_empty() {
            col = col.child(
                text(o.plan_summary.clone())
                    .size(12.5)
                    .weight(600.)
                    .leading(1.45)
                    .color(t.text),
            );
        }
        if !o.plan_body.is_empty() {
            let md = match &self.plan_md {
                Some((src, v)) if *src == o.plan_body => v.clone(),
                _ => {
                    let src = o.plan_body.clone();
                    let v = cx.new(|cx| {
                        MarkdownView::new(src.clone(), MarkdownClass::ChatCanvas, false, cx)
                    });
                    self.plan_md = Some((src, v.clone()));
                    v
                }
            };
            let long = o.plan_body.len() > 1200 || o.plan_body.lines().count() > 18;
            let open = self.ov_open.plan_body || !long;
            col = col.child(
                div()
                    .when(!open, |d| d.max_h(px(PLAN_BODY_CLAMP)).overflow_hidden())
                    .child(md),
            );
            if long {
                col = col.child(
                    disclosure(
                        "ov-plan-body",
                        if open {
                            "Show less".into()
                        } else {
                            "Show full plan".into()
                        },
                        open,
                        &t,
                    )
                    .px(px(0.))
                    .on_click(cx.guarded_listener(|v, _, _, cx| {
                        v.ov_open.plan_body = !v.ov_open.plan_body;
                        cx.notify();
                    })),
                );
            }
        }
        Some(col.into_any_element())
    }

    /// The Changes view's file rows in a card; a row opens the workspace's Review.
    fn files_card(&mut self, brief: &WorkspaceBrief, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        let changes = self.changes.get(&brief.workspace_id)?.clone();
        if changes.files.is_empty() {
            return None;
        }
        let n = changes.files.len();
        let (add, del) = changes
            .files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.add, d + f.del));
        let review = format!("workspace:{}:review", brief.workspace_id);
        let r2 = review.clone();
        let mut c = list_card(&t).child(
            card_head(
                icon(IconName::FileDiff)
                    .size(px(14.))
                    .color(hsla(t.muted))
                    .into_any_element(),
                format!("{n} file{}", if n == 1 { "" } else { "s" }),
                Some(format!("vs {}", changes.base_branch)),
                &t,
            )
            .border_b_1()
            .border_color(hsla(t.line))
            .child(
                div()
                    .ml_auto()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(diff_stat(add, del, &t))
                    .child(
                        note_btn("ov-review", "Review", false, false, t.pane, NOTE_LH, cx)
                            .debug_selector(|| "ov-review".into())
                            .on_click(cx.guarded_listener(move |v, _, _, cx| route(v, &r2, cx))),
                    ),
            ),
        );
        let open = self.ov_open.files;
        for (i, f) in changes
            .files
            .iter()
            .take(if open { usize::MAX } else { FILES_SHOWN })
            .enumerate()
        {
            let (dir, name) = match f.path.rsplit_once('/') {
                Some((d, n)) => (format!("{d}/"), n.to_string()),
                None => (String::new(), f.path.clone()),
            };
            let target = review.clone();
            c = c.child(
                card_row(i == 0, &t)
                    .id(SharedString::from(format!("ov-file-{i}")))
                    .gap(px(8.))
                    .py(px(5.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hsla(t.pane_2)))
                    .child(
                        div()
                            .flex_none()
                            .size(px(14.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .child(file_icon(name.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap(px(6.))
                            .child(text(name).size(12.).color(t.text).nowrap())
                            .child(text(dir).mono().size(10.5).color(t.faint).truncate()),
                    )
                    .child(diff_stat(f.add, f.del, &t))
                    .on_click(cx.guarded_listener(move |v, _, _, cx| route(v, &target, cx))),
            );
        }
        if n > FILES_SHOWN {
            c = c.child(
                disclosure(
                    "ov-files",
                    if open {
                        "Show fewer".into()
                    } else {
                        format!("{} more files", n - FILES_SHOWN)
                    },
                    open,
                    &t,
                )
                .border_t_1()
                .border_color(hsla(t.line_soft))
                .on_click(cx.guarded_listener(|v, _, _, cx| {
                    v.ov_open.files = !v.ov_open.files;
                    cx.notify();
                })),
            );
        }
        Some(c.into_any_element())
    }

    /// What finishing looks like for an idle workspace, in the PR's own terms: merge or
    /// ready it, or open one for unshipped changes. A settled PR shows here too.
    fn ship_card(&mut self, brief: &WorkspaceBrief, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        let pr = self.brief_pr(brief, cx);
        let has_changes = brief.diff_add + brief.diff_del > 0
            || self
                .changes
                .get(&brief.workspace_id)
                .is_some_and(|c| !c.files.is_empty());
        let wid = brief.workspace_id.clone();
        let life = pr.as_ref().and_then(|p| pr_lifecycle(Some(p)));
        let pr_lead = |p: &recur_domain::Pr, color: Color| {
            icon(pr_icon_name(Some(p)))
                .size(px(14.))
                .color(hsla(color))
                .into_any_element()
        };
        let (lead, title, meta, action): (AnyElement, String, Option<String>, Option<AnyElement>) =
            match (&pr, life) {
                (Some(p), Some(PrLifecycle::Open | PrLifecycle::Draft)) if brief.is_pr_host => {
                    let mut facts = Vec::new();
                    if !p.author_login.is_empty() {
                        facts.push(format!("by {}", p.author_login));
                    }
                    if p.file_count > 0 {
                        facts.push(format!(
                            "{} file{}",
                            p.file_count,
                            if p.file_count == 1 { "" } else { "s" }
                        ));
                    }
                    let checks = p.checks_summary();
                    if !checks.is_empty() {
                        facts.push(format!("checks {checks}"));
                    }
                    let view = format!("pr:{}", p.id);
                    (
                        pr_lead(p, t.emerald),
                        format!("#{} {}", p.number, p.title),
                        Some(facts.join(" · ")).filter(|s| !s.is_empty()),
                        Some(
                            note_btn("ov-review-pr", "Review", true, false, t.pane, NOTE_LH, cx)
                                .debug_selector(|| "ov-review-pr".into())
                                .on_click(
                                    cx.guarded_listener(move |v, _, _, cx| route(v, &view, cx)),
                                )
                                .into_any_element(),
                        ),
                    )
                }
                (Some(p), Some(PrLifecycle::Open)) => {
                    let checks = p.checks_summary();
                    let blocker = if p.mergeable == "CONFLICTING" {
                        Some("has conflicts")
                    } else if p.review_decision == "CHANGES_REQUESTED" {
                        Some("changes requested")
                    } else if p.review_decision == "REVIEW_REQUIRED" {
                        Some("waiting on review")
                    } else if checks.starts_with("pending") {
                        Some("checks running")
                    } else {
                        None
                    };
                    let pid = p.id.clone();
                    let action = blocker.is_none().then(|| {
                        merge_btn("ov-merge", "Merge", &t)
                            .on_click(cx.guarded_listener(move |v, _, _, cx| {
                                let id = pid.clone();
                                v.act(Some("Merged"), cx, async move |s| {
                                    s.prs.merge_pr(&id, "").await.map(|_| ())
                                });
                            }))
                            .into_any_element()
                    });
                    (
                        pr_lead(p, t.emerald),
                        format!("#{} {}", p.number, p.title),
                        Some(blocker.unwrap_or("ready to merge").to_string()),
                        action,
                    )
                }
                (Some(p), Some(PrLifecycle::Draft)) => {
                    let pid = p.id.clone();
                    (
                        pr_lead(p, t.cyan),
                        format!("#{} {}", p.number, p.title),
                        Some("draft".into()),
                        Some(
                            note_btn(
                                "ov-ready-pr",
                                "Ready for review",
                                true,
                                false,
                                t.pane,
                                NOTE_LH,
                                cx,
                            )
                            .debug_selector(|| "ov-ready-pr".into())
                            .on_click(cx.guarded_listener(move |v, _, _, cx| {
                                let id = pid.clone();
                                v.act(Some("Marked ready for review"), cx, async move |s| {
                                    s.prs.ready_pr(&id).await.map(|_| ())
                                });
                            }))
                            .into_any_element(),
                        ),
                    )
                }
                (Some(p), Some(PrLifecycle::Merged | PrLifecycle::Closed)) => {
                    let merged = life == Some(PrLifecycle::Merged);
                    (
                        pr_lead(p, if merged { t.indigo } else { t.coral }),
                        format!("#{} {}", p.number, p.title),
                        Some(if merged { "merged" } else { "closed" }.into()),
                        None,
                    )
                }
                (None, _) if has_changes => (
                    icon(IconName::GitPullRequest)
                        .size(px(14.))
                        .color(hsla(t.muted))
                        .into_any_element(),
                    "No pull request yet".into(),
                    None,
                    Some(
                        note_btn("ov-open-pr", "Open PR", true, false, t.pane, NOTE_LH, cx)
                            .debug_selector(|| "ov-open-pr".into())
                            .on_click(cx.guarded_listener(move |v, _, _, cx| {
                                v.send_to_workspace(&wid, "/open-pr", cx)
                            }))
                            .into_any_element(),
                    ),
                ),
                _ => return None,
            };
        let head: AnyElement = match pr.map(|p| format!("pr:{}", p.id)) {
            Some(target) => card_head(lead, title, None, &t)
                .id("ov-ship")
                .debug_selector(|| "ov-ship".into())
                .cursor_pointer()
                .hover(move |s| s.bg(hsla(t.pane_2)))
                .on_click(cx.guarded_listener(move |v, _, _, cx| route(v, &target, cx)))
                .into_any_element(),
            None => card_head(lead, title, None, &t).into_any_element(),
        };
        Some(
            list_card(&t)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(div().flex_1().min_w_0().child(head))
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .pl(px(10.))
                                .pr(px(10.))
                                .when_some(meta, |d, m| {
                                    d.child(text(m).mono().size(10.).color(t.faint).nowrap())
                                })
                                .children(action),
                        ),
                )
                .into_any_element(),
        )
    }
}
