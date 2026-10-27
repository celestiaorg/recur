//! Exact lengths: boxes at WebKit's fractional LayoutUnit positions.
//!
//! Taffy lays out in unrounded floats and GPUI snaps each box's absolute edges at the end, as
//! WebKit snaps at paint. But GPUI rounds every authored length (sizes, margins, paddings,
//! gaps, insets) to device pixels before layout and ceils every measured size, so one
//! fractional length (a 17.4px line, an 18.75px measured block) pushes every later box off
//! WebKit's position, and the error adds up down a column. `aspect_ratio` is the one input
//! that reaches Taffy unrounded: a leaf one device pixel thin on one axis is exactly as long
//! as asked on the other. Everything here is built from such leaves.
//!
//! - [`vgap`] / [`hgap`]: a fixed gap of an exact length, in place of a fractional margin,
//!   padding or `gap`.
//! - [`measured`]: a custom measured element whose size reaches layout unrounded.
//! - [`lu`]: a length as WebKit stores it.

use gpui_kit::{
    AvailableSpace, Div, LayoutId, Length, Pixels, Size, Style, Styled, Window, div, px, size,
};

/// A length as WebKit stores it: floored to a LayoutUnit (1/64 px).
pub fn lu(v: f32) -> f32 {
    (v * 64.).floor() / 64.
}

/// A vertical gap of exactly `len` CSS px (one device pixel wide).
pub fn vgap(len: f32, window: &Window) -> Div {
    vgap_at(len, window.scale_factor())
}

/// [`vgap`] at a known device scale, for builders without the window.
pub fn vgap_at(len: f32, scale: f32) -> Div {
    let dp = 1. / scale;
    let d = div().flex_none();
    if len <= 1e-4 {
        return d.h(px(0.));
    }
    d.w(px(dp)).aspect_ratio(dp / len)
}

/// A horizontal gap of exactly `len` CSS px (one device pixel tall).
pub fn hgap(len: f32, window: &Window) -> Div {
    let dp = 1. / window.scale_factor();
    let d = div().flex_none();
    if len <= 1e-4 {
        return d.w(px(0.));
    }
    d.h(px(dp)).aspect_ratio(len / dp)
}

/// `v` ceiled to a LayoutUnit (1/64 px), as WebKit stores an inline width
/// (`LayoutUnit::fromFloatCeil`). A value on that grid stays, within the f32 noise of summed
/// advances (WebKit's own sums land exactly on it: a 6px advance is 6, not 6.000002).
pub(crate) fn lu_ceil(v: f32) -> f32 {
    let lu = ((v * 64.) - 0.01).ceil() / 64.;
    if lu.is_finite() { lu } else { v }
}

/// `v` floored to the device grid.
pub fn floor_dp(v: f32, scale: f32) -> f32 {
    ((v * scale) + 1e-3).floor() / scale
}

/// A custom measured leaf whose exact size reaches layout: its width ceiled to a LayoutUnit,
/// its height as measured (the vendored GPUI's `request_measured_layout_exact`; upstream ceils
/// both to device pixels). Box edges still snap after layout.
///
/// `measure` returns the unrounded `(width, height)` for the known size and available space.
pub fn measured(
    min_width: Length,
    window: &mut Window,
    measure: impl Fn(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window) -> (f32, f32) + 'static,
) -> LayoutId {
    let mut leaf_style = Style::default();
    leaf_style.min_size.width = min_width;
    window.request_measured_layout_exact(leaf_style, move |known, available, window, _| {
        let (w, h) = measure(known, available, window);
        size(px(lu_ceil(w)), px(h))
    })
}
