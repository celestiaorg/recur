//! Rules of `_prs.scss` (and the shared sheets) the PR views have in common: the infinite
//! animations, the pane-header `.seg`, and the controls several views draw.

use std::time::Instant;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Div, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Stateful, Styled, Window, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::motion::{self, Easing};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Color, Theme};

pub fn hsla(c: Color) -> Hsla {
    c.into()
}

/// When the views' infinite animations started (every `.spin` and `.pulse` runs in step from
/// the first frame, as the page's animations do from load).
fn epoch() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// `.spin`: `rotate(360deg)` over 1.5s, linear. Reduced motion holds still.
pub fn spin_deg(window: &mut Window, cx: &App) -> f32 {
    if motion::reduced(cx) {
        return 0.;
    }
    if !motion::is_frozen(cx) {
        window.request_animation_frame();
    }
    (motion::elapsed(epoch(), cx).as_secs_f32() / 1.5).fract() * 360.
}

/// `@keyframes pulse` (opacity .5 → 1 → .5, ease-in-out) over `period` seconds.
pub fn pulse_opacity(period: f32, window: &mut Window, cx: &App) -> f32 {
    if motion::reduced(cx) {
        return 1.;
    }
    if !motion::is_frozen(cx) {
        window.request_animation_frame();
    }
    let p = (motion::elapsed(epoch(), cx).as_secs_f32() % period) / period;
    if p < 0.5 {
        0.5 + 0.5 * Easing::EASE_IN_OUT.apply(p * 2.)
    } else {
        1. - 0.5 * Easing::EASE_IN_OUT.apply((p - 0.5) * 2.)
    }
}

/// `.sd.pulse` in a 14px slot: the amber running dot with its 5px glow (2.4s pulse).
pub fn running_dot(window: &mut Window, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    div()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .opacity(pulse_opacity(2.4, window, cx))
        .child(with_shadow(
            div().size(px(6.)).rounded_full().bg(hsla(t.amber)),
            &[css_box_shadow(0., 0., 5., 0., t.amber)],
            3.,
        ))
}

/// `.seg` (`_main.scss`): `--pane`, a 1px `--line` border, radius 6, padding 2.
pub fn seg(cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    recur_ui::style::card(t.pane, t.line, 6.).flex().p(px(2.))
}

/// A `.seg button` in a pane header (`.briefing-filters .seg button`): full height, padding
/// 0 10, 11px weight 500, muted; `.on` fills `--pane-2` with `--text`.
pub fn seg_text_btn(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    cx: &App,
) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    div()
        .id(id)
        .flex()
        .items_center()
        .h_full()
        .px(px(10.))
        .rounded(px(4.))
        .cursor_pointer()
        .text_color(hsla(if on { t.text } else { t.muted }))
        .when(on, |d| d.bg(hsla(t.pane_2)))
        .child(
            text(label.into())
                .size(11.)
                .weight(500.)
                .line_height(LineHeight::Normal)
                .nowrap(),
        )
}

/// An inline `<svg>` `size` px square in a block of `family` text at `font` px and
/// `line_height` (`<span><Icon /></span>`): the icon's bottom on the strut's baseline, the
/// block as tall as the line box.
pub fn baseline_icon(
    el: impl IntoElement,
    size: f32,
    family: Family,
    font: f32,
    line_height: LineHeight,
    window: &Window,
) -> Div {
    let m = recur_ui::text::strut_metrics(&family, font, line_height, window);
    let above = m.baseline.max(size);
    let below = m.height - m.baseline;
    div()
        .flex()
        .flex_col()
        .flex_none()
        .child(recur_ui::exact::vgap(above - size, window))
        .child(div().flex_none().size(px(size)).child(el))
        .child(recur_ui::exact::vgap(below, window))
}

/// `<span class="d">`: a `size` px dot.
pub fn dot(size: f32, color: Color) -> Div {
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(hsla(color))
}
