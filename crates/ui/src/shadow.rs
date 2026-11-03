//! CSS `box-shadow` layers on GPUI's shadow primitive. A lone shadow can go straight on
//! `.shadow()`; list it here when it stacks with other layers, so the CSS order holds.

use crate::color::Color;
use gpui_kit::{BoxShadow, Div, IntoElement, ParentElement, Styled, div, point, px};

/// One `box-shadow` layer (CSS px; `blur` is the CSS blur radius).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CssShadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
}

pub const fn css_box_shadow(x: f32, y: f32, blur: f32, spread: f32, color: Color) -> CssShadow {
    CssShadow {
        x,
        y,
        blur,
        spread,
        color,
    }
}

/// `content` with `shadows` behind it (the first listed on top, as in CSS). `radius` is the
/// content's border radius. Size and position the wrapper as the content would be. Use this when
/// the content clips its children (`overflow_hidden`) or has no border to account for.
pub fn with_shadow(content: impl IntoElement, shadows: &[CssShadow], radius: f32) -> Div {
    // A shrinkable column, so a max height set around it still reaches `content`.
    div()
        .relative()
        .flex()
        .flex_col()
        .min_h_0()
        .child(box_shadow(shadows, radius, 0.))
        .child(content)
}

/// `shadows` for the relatively positioned element this is the first child of, whose border is
/// `border` wide and radius `radius`.
pub fn box_shadow(shadows: &[CssShadow], radius: f32, border: f32) -> impl IntoElement {
    let b = px(-border);
    div()
        .absolute()
        .top(b)
        .left(b)
        .right(b)
        .bottom(b)
        .rounded(px(radius))
        .shadow(
            shadows
                .iter()
                .rev()
                .map(|s| BoxShadow {
                    color: s.color.into(),
                    offset: point(px(s.x), px(s.y)),
                    // GPUI's blur radius is the Gaussian's σ, CSS's is twice it.
                    blur_radius: px(s.blur / 2.),
                    spread_radius: px(s.spread),
                    inset: false,
                })
                .collect(),
        )
}
