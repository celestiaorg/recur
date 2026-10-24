//! `_briefing.scss` and the shared rules its list views draw with: the pane header
//! (`.briefing-filters`), bucket strips (`.briefing-h`), `.empty`, the flattened `.ibrow` and
//! the Clear filters / Show all buttons.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Div, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, Stateful, Styled, div, px,
};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Color, IconName, Theme, icon};

pub fn hsla(c: Color) -> Hsla {
    c.into()
}

/// `--obs-chrome-h`.
pub const CHROME_H: f32 = 40.;
/// `.briefing-h`: 8px padding, a 9px line, 8px padding and the 1px rule.
pub const HEAD_H: f32 = 26.;

/// `.briefing-filters`: the 40px pane header on `--pane` over a `--line` rule.
pub fn filters_bar(cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    div()
        .debug_selector(|| "briefing-filters".into())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(CHROME_H))
        .px(px(12.))
        .bg(hsla(t.pane))
        .border_b_1()
        .border_color(hsla(t.line))
}

/// `.briefing-filters .seg`: 24px tall, its buttons stretched.
pub fn filter_seg(cx: &App) -> Div {
    recur_view_prs::style::seg(cx).flex_none().h(px(24.))
}

/// `.briefing-filters .seg button` with its optional `.cnt` (mono 10px, `--faint`; `--muted`
/// on the selected button).
pub fn seg_count_btn(
    id: impl Into<ElementId>,
    label: &'static str,
    count: Option<usize>,
    on: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    recur_view_prs::style::seg_text_btn(id, label, on, cx)
        .gap(px(4.))
        .debug_selector(move || format!("seg:{label}:{on}"))
        .when_some(count, |d, n| {
            d.child(
                div().ml(px(1.)).child(
                    text(n.to_string())
                        .mono()
                        .size(10.)
                        .line_height(LineHeight::Normal)
                        .color(if on { t.muted } else { t.faint })
                        .nowrap(),
                ),
            )
        })
}

/// `.briefing-h`: the bucket strip with its dot, uppercase label and count. `key` names the
/// spec handle (`briefing-h:{key}`).
pub fn bucket_head(key: &str, label: &str, n: usize, dot: Color, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    let k = key.to_string();
    div()
        .debug_selector(move || format!("briefing-h:{k}"))
        .flex()
        .flex_none()
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
                .bg(hsla(dot))
                .relative()
                .top(px(-1.)),
        )
        .child(
            text(label.to_string())
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

/// `.empty`: a centered icon tile, title, description and action, 7px apart.
pub fn empty(
    ei: Option<IconName>,
    et: &str,
    ed: Option<AnyElement>,
    ea: Option<AnyElement>,
    cx: &App,
) -> Div {
    let t = Theme::of(cx).tokens;
    let mut d = div()
        .debug_selector(|| "empty".into())
        .flex()
        .flex_col()
        .items_center()
        .gap(px(7.))
        .py(px(72.))
        .px(px(20.));
    if let Some(i) = ei {
        d = d.child(
            recur_ui::style::card(t.pane, t.line, 12.)
                .size(px(46.))
                .mb(px(8.))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(i).size(px(22.)).stroke_width(1.8).color(hsla(t.faint))),
        );
    }
    let et = et.to_string();
    let sel = format!("empty:{et}");
    d = d.child(
        div().debug_selector(move || sel).child(
            text(et.clone())
                .size(14.)
                .weight(600.)
                .leading(1.45)
                .color(t.text)
                .align(recur_ui::text::Align::Center)
                .id(SharedString::from(format!("empty-et-{et}"))),
        ),
    );
    if let Some(ed) = ed {
        d = d.child(div().max_w(px(340.)).flex().justify_center().child(ed));
    }
    if let Some(ea) = ea {
        d = d.child(div().mt(px(14.)).child(ea));
    }
    d
}

/// `.empty .ed` text: 12.5px `--muted`, line-height 1.5, centered.
pub fn empty_desc(s: &str, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    text(s.to_string())
        .size(12.5)
        .leading(1.5)
        .color(t.muted)
        .align(recur_ui::text::Align::Center)
        .id(SharedString::from(format!("empty-ed-{s}")))
        .into_any_element()
}

/// `.briefing-clear`: the Clear filters pill.
pub fn clear_button(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    recur_ui::style::card(t.pane_2, t.line_bright, 999.)
        .id(id)
        .debug_selector(|| "briefing-clear".into())
        .flex()
        .items_center()
        .py(px(5.))
        .px(px(12.))
        .cursor_pointer()
        .text_color(hsla(t.text))
        .child(recur_ui::style::button_label("Clear filters", 12., 400.))
}

/// `.briefing-more`: the full-width Show all / Show fewer row.
pub fn more_button(id: impl Into<ElementId>, label: String, cx: &App) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    div()
        .id(id)
        .debug_selector({
            let l = label.clone();
            move || format!("briefing-more={l}")
        })
        .w_full()
        .flex()
        .justify_center()
        .py(px(8.))
        .px(px(12.))
        .border_b_1()
        .border_color(hsla(t.line_soft))
        .text_color(hsla(t.muted))
        .cursor_pointer()
        .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
        .child(recur_ui::style::button_label(label, 12., 400.))
}

/// `.briefing-overview .ibrow`: a flattened notification row, `padding: 7px 16px` over a
/// `--line-soft` rule, `--pane-2` on hover.
pub fn ibrow(id: impl Into<ElementId>, hovered: bool, cx: &App) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(13.))
        .py(px(7.))
        .px(px(16.))
        .border_b_1()
        .border_color(hsla(t.line_soft))
        .when(hovered, |d| d.bg(hsla(t.pane_2)))
}

/// `.briefing-overview .ibrow .ity`: the 24px tile, `color` at 14% under a 14px glyph.
pub fn ity(color: Color) -> Div {
    div()
        .flex_none()
        .size(px(24.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(color.mix(0.14, recur_ui::color::TRANSPARENT)))
        .text_color(hsla(color))
}

/// `.ibrow .udot`: the 7px unread dot (transparent once read).
pub fn udot(unread: bool, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    div()
        .flex_none()
        .size(px(7.))
        .rounded_full()
        .when(unread, |d| d.bg(hsla(t.amber)))
}

/// `.wbrief-unread`: the coral count pill (`9+` past nine).
pub fn unread_chip(n: usize, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    let label = if n > 9 {
        "9+".to_string()
    } else {
        n.to_string()
    };
    div()
        .flex_none()
        .min_w(px(18.))
        .py(px(1.))
        .px(px(6.))
        .rounded(px(9.))
        .bg(hsla(t.coral))
        .flex()
        .justify_center()
        .child(
            text(label)
                .size(10.)
                .weight(600.)
                .leading(1.45)
                .color(recur_ui::hex(0x0a0c10))
                .nowrap(),
        )
}
