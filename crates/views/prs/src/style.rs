//! Styles the PR views have in common: the infinite
//! animations, the pane-header `.seg`, and the controls several views draw.

use gpui_kit::{Div, Hsla, IntoElement, ParentElement, Styled, Window, div, px};
use recur_ui::Color;
use recur_ui::fonts::Family;
use recur_ui::text::LineHeight;

pub use recur_view_widgets::chrome::{
    dot, pulse_opacity, running_dot, seg, seg_text_btn, spin_deg,
};

pub fn hsla(c: Color) -> Hsla {
    c.into()
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
