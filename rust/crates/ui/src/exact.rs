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
//! - [`measured`]: a custom measured element whose height reaches layout unrounded.
//! - [`lu`]: a length as WebKit stores it.

use gpui_kit::{
    App, AvailableSpace, Div, GlobalElementId, LayoutId, Length, Pixels, Size, Style, Styled,
    Window, div, px, size,
};
use smallvec::SmallVec;
use std::cell::Cell;
use std::rc::Rc;

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

/// The exact height a measured box took in the last frame (see [`measured`]).
pub type ExactCell = Rc<Cell<Option<f32>>>;

/// The element's [`ExactCell`], kept in its element state. `None` without an id.
pub fn exact_cell(id: Option<&GlobalElementId>, window: &mut Window) -> Option<ExactCell> {
    Some(
        window.with_element_state(id?, |cell: Option<ExactCell>, _| {
            let cell = cell.unwrap_or_default();
            (cell.clone(), cell)
        }),
    )
}

/// `v` ceiled to a LayoutUnit (1/64 px), as WebKit stores an inline width
/// (`LayoutUnit::fromFloatCeil`); a value already on that grid stays.
fn lu_ceil(v: f32) -> f32 {
    let lu = ((v * 64.) - 1e-3).ceil() / 64.;
    if lu.is_finite() { lu } else { v }
}

/// `v` floored to the device grid.
pub fn floor_dp(v: f32, scale: f32) -> f32 {
    ((v * scale) + 1e-3).floor() / scale
}

/// The leaves of a custom measured box whose exact size reaches layout. Its width stays as
/// measured (ceiled to a LayoutUnit). GPUI ceils a measured height to device pixels, so the
/// measured leaf reports its height floored to the device grid
/// and a spacer (a 1dp-wide `aspect_ratio` leaf) adds the remainder. The remainder is known
/// only before layout: it is the height from `exact` (the last frame, see [`settle`]) or
/// `guess` until then. Put the leaves in a block (`Style::default()`) container.
///
/// `measure` returns the unrounded `(width, height)` for the known size and available space.
pub fn measured(
    min_width: Length,
    exact: Option<&ExactCell>,
    guess: f32,
    window: &mut Window,
    cx: &mut App,
    measure: impl Fn(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window) -> (f32, f32) + 'static,
) -> SmallVec<[LayoutId; 6]> {
    let mut leaf_style = Style::default();
    leaf_style.min_size.width = min_width;
    let leaf = window.request_measured_layout_exact_width(
        leaf_style,
        move |known, available, window, _| {
            let (w, h) = measure(known, available, window);
            size(px(lu_ceil(w)), px(floor_dp(h, window.scale_factor())))
        },
    );
    let mut leaves = SmallVec::new();
    leaves.push(leaf);
    let scale = window.scale_factor();
    let h = exact.and_then(|c| c.get()).unwrap_or(guess);
    let rest = h - floor_dp(h, scale);
    if rest > 1e-4 {
        let mut spacer = Style::default();
        spacer.size.width = px(1. / scale).into();
        spacer.aspect_ratio = Some(1. / scale / rest);
        leaves.push(window.request_layout(spacer, [], cx));
    }
    leaves
}

/// After the final layout of a [`measured`] box: remember its exact height, and lay the box's
/// view out again when it changed. Notifying the view, not refreshing the window, keeps
/// other cached views (`AnyView::cached`) from re-rendering every time any box settles.
pub fn settle(exact: Option<&ExactCell>, height: f32, window: &mut Window, cx: &mut App) {
    if let Some(c) = exact
        && c.get().is_none_or(|h| (h - height).abs() > 1e-3)
    {
        c.set(Some(height));
        cx.notify(window.current_view());
    }
}
