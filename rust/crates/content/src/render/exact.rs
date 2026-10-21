//! Exact lengths. GPUI rounds authored lengths to device pixels before layout; WebKit keeps
//! LayoutUnits. A one-device-pixel leaf sized through `aspect_ratio` (which reaches Taffy
//! unrounded) is exactly as long as asked, so boxes after it keep WebKit's fractional
//! positions and snap as WebKit snaps them.

use gpui_kit::{Div, Styled, Window, div, px};

/// A vertical gap of exactly `len` CSS px.
pub fn vgap(len: f32, window: &Window) -> Div {
    let dp = 1. / window.scale_factor();
    let d = div().flex_none();
    if len <= 1e-4 {
        return d.h(px(0.));
    }
    d.w(px(dp)).aspect_ratio(dp / len)
}

/// A horizontal gap of exactly `len` CSS px.
pub fn hgap(len: f32, window: &Window) -> Div {
    let dp = 1. / window.scale_factor();
    let d = div().flex_none();
    if len <= 1e-4 {
        return d.w(px(0.));
    }
    d.h(px(dp)).aspect_ratio(len / dp)
}
