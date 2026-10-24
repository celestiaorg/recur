//! `WorkspaceBriefCard.svelte`: one workspace's briefing as a dense table row (the Inbox list)
//! or as the card a `present_workspace` pin shows in a transcript. Clicking opens the
//! primary target and marks the workspace's notifications read.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Entity, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_domain::{InboxItem, WorkspaceBrief};
use recur_engine::view::artifacts::format_relative_age;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::{Color, IconName, Theme, Tokens, icon, motion};
use recur_view_core::AppState;

use crate::style::{INK, hsla, label};

/// `REASON_META`: the reason tile's icon and color.
fn reason_meta(reason: &str, t: &Tokens) -> (IconName, Color) {
    match reason {
        "permission" => (IconName::Shield, t.coral),
        "question" => (IconName::MessageQuestion, t.coral),
        "escalation" => (IconName::TriangleAlert, t.coral),
        "retryable_failure" => (IconName::RefreshCw, t.coral),
        "failure" => (IconName::OctagonX, t.coral),
        "plan_approval" => (IconName::ListTodo, t.warn),
        "findings" => (IconName::HatGlasses, t.warn),
        "pr_attention" => (IconName::GitPullRequest, t.emerald),
        "unseen_chat" => (IconName::MessageCode, t.cyan),
        "active_agent" => (IconName::Bot, t.amber),
        "unfinished_plan" => (IconName::ListTodo, t.indigo),
        "dirty_idle" => (IconName::FileDiff, t.indigo),
        "recent_chat" => (IconName::MessageSquare, t.indigo),
        _ => (IconName::Box, t.muted),
    }
}

/// `.ity`: the 24px reason tile.
fn tile(
    brief: &WorkspaceBrief,
    row: bool,
    epoch: std::time::Instant,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let reason = brief
        .reasons
        .as_ref()
        .and_then(|r| r.first())
        .map(String::as_str)
        .unwrap_or("");
    let (name, color) = reason_meta(reason, &t);
    let live = reason == "active_agent";
    // `.wbrief-row .ity.live svg { animation: wbrief-live 2.4s ease-in-out infinite }`.
    let alpha = if row && live && !motion::reduced(cx) {
        let p = (motion::elapsed(epoch, cx).as_secs_f32() % 2.4) / 2.4;
        let half = if p < 0.5 { p * 2. } else { (1. - p) * 2. };
        if !motion::is_frozen(cx) {
            window.request_animation_frame();
        }
        1. - 0.6 * motion::Easing::EASE_IN_OUT.apply(half)
    } else {
        1.
    };
    div()
        .flex_none()
        .size(px(24.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(color.mix(0.12, recur_ui::color::TRANSPARENT)))
        .child(icon(name).size(px(14.)).color(hsla(color.fade(alpha))))
        .into_any_element()
}

fn open(state: &Entity<AppState>, brief: &WorkspaceBrief, open_target: &str, cx: &mut App) {
    let target = recur_engine::view::js::trim(if brief.primary.target.is_empty() {
        open_target
    } else {
        &brief.primary.target
    })
    .to_string();
    if target.is_empty() {
        return;
    }
    let ws = brief.workspace_id.clone();
    state.update(cx, |s, cx| {
        s.mark_workspace_inbox_read(&ws, cx);
        s.route_target(&target, cx);
    });
}

fn title_row(brief: &WorkspaceBrief, wrap: bool, extra: Vec<Span>, t: &Tokens) -> Text {
    let mut spans = vec![span(brief.title.clone())];
    if brief.pr_number > 0 {
        spans.push(
            span(format!("#{}", brief.pr_number))
                .mono()
                .size(11.)
                .weight(400.)
                .color(t.muted)
                .boxed(recur_ui::text::InlineBox {
                    margin_left: 4.,
                    ..Default::default()
                }),
        );
    }
    spans.extend(extra);
    let tx = Text::from_spans(spans).size(13.).weight(600.).color(t.text);
    if wrap {
        tx.break_anywhere()
    } else {
        tx.truncate()
    }
}

/// The card variant (`present_workspace` in a transcript).
pub fn brief_card(
    id: SharedString,
    brief: &WorkspaceBrief,
    open_target: &str,
    state: &Entity<AppState>,
    epoch: std::time::Instant,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    // `.wbrief { transition: border-color .12s, background .12s }`.
    let h = motion::hover(
        SharedString::from(format!("{id}-hover")),
        std::time::Duration::from_millis(120),
        motion::Easing::EASE,
        window,
        cx,
    );
    let mut main = div()
        .flex_1()
        .min_w_0()
        .child(title_row(brief, true, Vec::new(), &t).id(SharedString::from(format!("{id}-t"))));
    if !brief.project_name.is_empty() || !brief.branch.is_empty() {
        let mut parts: Vec<Span> = Vec::new();
        if !brief.project_name.is_empty() {
            parts.push(span(brief.project_name.clone()));
        }
        if !brief.branch.is_empty() {
            if !parts.is_empty() {
                parts.push(span("·").boxed(recur_ui::text::InlineBox {
                    margin_left: 7.,
                    margin_right: 7.,
                    ..Default::default()
                }));
            }
            parts.push(span(brief.branch.clone()));
        }
        main = main.child(
            div().mt(px(2.)).min_w_0().child(
                Text::from_spans(parts)
                    .id(SharedString::from(format!("{id}-m")))
                    .mono()
                    .size(10.5)
                    .weight(400.)
                    .line_height(LineHeight::Scale(1.35))
                    .color(t.faint)
                    .break_anywhere(),
            ),
        );
    }
    let next = if brief.now.is_empty() {
        &brief.next
    } else {
        &brief.now
    };
    if !next.is_empty() {
        main = main.child(
            div().mt(px(4.)).child(
                text(next.clone())
                    .id(SharedString::from(format!("{id}-n")))
                    .size(13.)
                    .line_height(LineHeight::Scale(1.35))
                    .color(t.text)
                    .break_anywhere(),
            ),
        );
    }
    let (state, b, target) = (state.clone(), brief.clone(), open_target.to_string());
    div()
        .id(id)
        .relative()
        .flex()
        .gap(px(10.))
        .px(px(12.))
        .py(px(10.))
        .rounded(px(10.))
        .border_1()
        .border_color(hsla(t.line_bright.mix(h.k(), t.line)))
        .bg(hsla(t.pane_2.mix(h.k(), t.pane)))
        .cursor_pointer()
        .on_hover(h.listener())
        .child(tile(brief, false, epoch, window, cx))
        .child(main)
        .on_click(move |_, _, cx| open(&state, &b, &target, cx))
        .into_any_element()
}

/// The compact table row (Inbox list, tool-result lists).
#[allow(clippy::too_many_arguments)]
pub fn brief_row(
    id: SharedString,
    brief: &WorkspaceBrief,
    notes: &[InboxItem],
    open_target: &str,
    state: &Entity<AppState>,
    epoch: std::time::Instant,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    // `.wbrief-row { transition: background .1s }`.
    let h = motion::hover(
        SharedString::from(format!("{id}-hover")),
        std::time::Duration::from_millis(100),
        motion::Easing::EASE,
        window,
        cx,
    );
    let unread = notes.len();
    let note_titles: Vec<String> = notes
        .iter()
        .map(|n| strip_tags(&n.title))
        .filter(|s| !s.is_empty())
        .collect();
    let mut extra = Vec::new();
    if !brief.branch.is_empty() {
        extra.push(
            span(brief.branch.clone())
                .mono()
                .size(10.5)
                .weight(400.)
                .color(t.faint)
                .boxed(recur_ui::text::InlineBox {
                    margin_left: 8.,
                    ..Default::default()
                }),
        );
    }
    let mut title = div().flex().items_baseline().min_w_0().child(
        div()
            .flex_initial()
            .min_w_0()
            .overflow_hidden()
            .child(title_row(brief, false, extra, &t)),
    );
    if unread > 0 {
        let tip = if note_titles.is_empty() {
            format!(
                "{unread} unread notification{}",
                if unread == 1 { "" } else { "s" }
            )
        } else {
            note_titles.join(" · ")
        };
        let sel = format!("wbrief-unread:{}={unread}:{tip}", brief.title);
        title = title.child(recur_ui::tooltip::titled(
            SharedString::from(format!("{id}-u")),
            tip,
            div()
                .debug_selector(move || sel)
                .flex_none()
                .self_center()
                .ml(px(8.))
                .min_w(px(18.))
                .py(px(1.))
                .px(px(6.))
                .rounded(px(9.))
                .bg(hsla(t.coral))
                .flex()
                .justify_center()
                .child(label(unread.to_string(), Family::Ui, 10., 600.).color(INK)),
        ));
    }
    let now = if brief.now.is_empty() {
        &brief.next
    } else {
        &brief.now
    };
    let main = div().min_w_0().child(title).child(
        div()
            .mt(px(1.))
            .min_w_0()
            .overflow_hidden()
            .child(text(now.clone()).size(12.).color(t.muted).truncate()),
    );
    let fact = |s: String| {
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(6.))
            .child(label(s, Family::Mono, 10.5, 400.).color(t.faint))
    };
    let plan = if brief.plan_total > 0 {
        format!("{}/{}", brief.plan_done, brief.plan_total)
    } else {
        String::new()
    };
    let diff = (brief.diff_add + brief.diff_del > 0).then(|| {
        Text::from_spans(vec![
            span(format!("+{}", brief.diff_add)).color(t.git_a),
            span(" "),
            span(format!("−{}", brief.diff_del)).color(t.git_d),
        ])
        .mono()
        .size(10.5)
        .color(t.faint)
        .nowrap()
    });
    let age = format_relative_age(brief.last_activity_at, crate::chat::now_ms());
    let has_unread = unread > 0 || brief.changed_since_viewed;
    let (state, b, target) = (state.clone(), brief.clone(), open_target.to_string());
    // The root font size (`rem`), not the body's 13px.
    let rem = 16.;
    div()
        .id(id.clone())
        .debug_selector({
            let t = brief.title.clone();
            move || format!("wbrief-row:{t}")
        })
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .py(px(7.))
        .border_b_1()
        .border_color(hsla(t.line_soft))
        .cursor_pointer()
        .bg(hsla(t.pane_2.mix(h.k(), recur_ui::color::TRANSPARENT)))
        .on_hover(h.listener())
        .child(
            div()
                .flex_none()
                .size(px(7.))
                .rounded_full()
                .when(has_unread, |d| d.bg(hsla(t.amber))),
        )
        .child(
            div()
                .flex_none()
                .w(px(28.))
                .child(tile(brief, true, epoch, window, cx)),
        )
        .child(div().flex_1().min_w_0().child(main))
        .child(
            div()
                .flex_shrink_0()
                .min_w(px(4. * rem))
                .w(px(6.5 * rem))
                .overflow_hidden()
                .child(
                    text(brief.project_name.clone())
                        .size(11.5)
                        .color(t.muted)
                        .truncate(),
                ),
        )
        .child(div().flex_none().w(px(3. * rem)).child(if plan.is_empty() {
            div()
        } else {
            fact(plan)
        }))
        .child(
            div()
                .flex_none()
                .w(px(6. * rem))
                .flex()
                .justify_end()
                .when_some(diff, |d, x| d.child(x)),
        )
        .child(
            // `text-align: right` in a 2.5rem track: a wider age starts at the track and
            // overflows to the right, as a too-wide line box does.
            div().flex_none().w(px(2.5 * rem)).flex().child(
                div()
                    .flex_none()
                    .min_w(px(2.5 * rem))
                    .flex()
                    .justify_end()
                    .when_some(age, |d, a| {
                        d.child(recur_ui::tooltip::titled(
                            SharedString::from(format!("{id}-age")),
                            a.full,
                            label(a.rel, Family::Mono, 10.5, 400.).color(t.faint),
                        ))
                    }),
            ),
        )
        .on_click(move |_, _, cx| open(&state, &b, &target, cx))
        .into_any_element()
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out
}
