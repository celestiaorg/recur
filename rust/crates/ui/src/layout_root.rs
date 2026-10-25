//! A box whose content lays out as its own layout root.
//!
//! Taffy lays a nested flex child out once per layout pass of its parent, often at a new width
//! each time, so the cost of a subtree grows with how deep it sits rather than with its size: a
//! three-file diff in the PR page measured its text 67,000 times a frame. A [`LayoutRoot`] is a
//! single node sized by its own style in the outer layout; its content lays out once, in
//! prepaint, at the box's final bounds.
//!
//! GPUI snaps each edge to device pixels from its unrounded window position, and a root lays out
//! from an already snapped origin, so content below a fractional offset would round its edges
//! differently from the same content laid out in place (and lose [`TIE_NUDGE`](crate::style::TIE_NUDGE)).
//! The box reads its unrounded edges off probe markers placed a fraction of a device pixel past
//! each edge, and lays the content out shifted by the difference, so every edge lands where the
//! in-place layout puts it. A marker only says whether the edge plus its offset rounds up, so
//! each frame places [`STEPS`] markers across the whole pixel and as many inside the interval the
//! last frame left, narrowing it until it is far finer than the tie nudge.

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, FlexDirection, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, ParentElement, Pixels, Position, Refineable, Style,
    StyleRefinement, Styled, Window, div, px,
};

/// Markers per edge and per pass: each frame narrows an edge's interval this many times.
const STEPS: usize = 24;
/// Narrower than this (device px) an edge is settled.
const SETTLED: f32 = 1e-4;

pub struct LayoutRoot {
    id: ElementId,
    style: StyleRefinement,
    child: Option<AnyElement>,
    root: Option<AnyElement>,
}

/// `child` laid out as its own root in a box sized by `style` alone (like a cached view's).
pub fn layout_root(
    id: impl Into<ElementId>,
    style: StyleRefinement,
    child: impl IntoElement,
) -> LayoutRoot {
    LayoutRoot {
        id: id.into(),
        style,
        child: Some(child.into_any_element()),
        root: None,
    }
}

impl IntoElement for LayoutRoot {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Where an edge's unrounded position sits past its snapped one, in device px: in `(lo, hi]`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    lo: f32,
    hi: f32,
}

impl Default for Span {
    fn default() -> Self {
        Span { lo: -0.5, hi: 0.5 }
    }
}

impl Span {
    /// The thresholds to test this frame: the whole pixel, then the interval left last frame.
    fn thresholds(self) -> Vec<f32> {
        let n = STEPS as f32;
        let mut t: Vec<f32> = (1..STEPS).map(|k| -0.5 + k as f32 / n).collect();
        if self.hi - self.lo < 1. - 1e-6 {
            t.extend((0..=STEPS).map(|j| self.lo + (self.hi - self.lo) * j as f32 / n));
        }
        t.retain(|t| t.abs() < 0.5 - 1e-5);
        t
    }

    /// The interval `results` (threshold, edge past it) leave.
    fn narrowed(results: &[(f32, bool)]) -> Span {
        let mut s = Span::default();
        for &(t, past) in results {
            if past {
                s.lo = s.lo.max(t);
            } else {
                s.hi = s.hi.min(t);
            }
        }
        s
    }

    fn mid(self) -> f32 {
        (self.lo + self.hi) / 2.
    }

    fn settled(self) -> bool {
        self.hi - self.lo < SETTLED
    }
}

#[derive(Clone, Copy, Default)]
struct Edges {
    left: Span,
    top: Span,
    right: Span,
    bottom: Span,
}

/// The markers this frame placed, with the threshold each tests.
pub struct Probes {
    edges: Edges,
    left: Vec<(f32, LayoutId)>,
    top: Vec<(f32, LayoutId)>,
    right: Vec<(f32, LayoutId)>,
    bottom: Vec<(f32, LayoutId)>,
}

fn node(style: Style, children: &[LayoutId], window: &mut Window, cx: &mut App) -> LayoutId {
    window.request_layout(style, children.iter().copied(), cx)
}

/// A spacer `len` px along `vertical` (one device px across), sized by `aspect_ratio` so the
/// length is not rounded to whole device pixels.
fn spacer(len: f32, vertical: bool, dp: f32, window: &mut Window, cx: &mut App) -> LayoutId {
    let mut s = Style {
        flex_shrink: 0.,
        ..Default::default()
    };
    if vertical {
        s.size.width = px(dp).into();
        s.aspect_ratio = Some(dp / len);
    } else {
        s.size.height = px(dp).into();
        s.aspect_ratio = Some(len / dp);
    }
    node(s, &[], window, cx)
}

/// A zero-size marker `offset` px inside the box from one of its edges, along one axis.
fn probe(
    offset: f32,
    vertical: bool,
    far: bool,
    dp: f32,
    window: &mut Window,
    cx: &mut App,
) -> (LayoutId, LayoutId) {
    let m = Style {
        flex_shrink: 0.,
        ..Default::default()
    };
    let marker = node(m, &[], window, cx);
    let gap = spacer(offset, vertical, dp, window, cx);
    let mut s = Style {
        display: gpui_kit::Display::Flex,
        position: Position::Absolute,
        flex_direction: if vertical {
            FlexDirection::Column
        } else {
            FlexDirection::Row
        },
        ..Default::default()
    };
    let kids = if far {
        s.inset.right = px(0.).into();
        s.inset.bottom = px(0.).into();
        [marker, gap]
    } else {
        s.inset.left = px(0.).into();
        s.inset.top = px(0.).into();
        [gap, marker]
    };
    (node(s, &kids, window, cx), marker)
}

impl Element for LayoutRoot {
    type RequestLayoutState = Probes;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Probes) {
        let edges = id.map_or_else(Edges::default, |id| {
            window.with_element_state::<Edges, _>(id, |s, _| {
                let s = s.unwrap_or_default();
                (s, s)
            })
        });
        let dp = 1. / window.scale_factor();
        let mut children = Vec::new();
        let mut place = |span: Span, vertical: bool, far: bool, window: &mut Window| {
            span.thresholds()
                .into_iter()
                .map(|t| {
                    // A near edge past `t` rounds up with a marker `0.5 - t` inside it; a far
                    // edge past `t` stays up with a marker `t + 0.5` inside it.
                    let offset = if far { t + 0.5 } else { 0.5 - t };
                    let (wrap, marker) = probe(offset * dp, vertical, far, dp, window, cx);
                    children.push(wrap);
                    (t, marker)
                })
                .collect::<Vec<_>>()
        };
        let left = place(edges.left, false, false, window);
        let top = place(edges.top, true, false, window);
        let right = place(edges.right, false, true, window);
        let bottom = place(edges.bottom, true, true, window);
        let mut style = Style::default();
        style.refine(&self.style);
        if style.position != Position::Absolute {
            style.position = Position::Relative;
        }
        let layout = window.request_layout(style, children, cx);
        (
            layout,
            Probes {
                edges,
                left,
                top,
                right,
                bottom,
            },
        )
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        probes: &mut Probes,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(child) = self.child.take() else {
            return;
        };
        let s = window.scale_factor();
        let dev = |v: Pixels| (f32::from(v) * s).round();
        let (x0, y0) = (dev(bounds.origin.x), dev(bounds.origin.y));
        let (x1, y1) = (
            dev(bounds.origin.x + bounds.size.width),
            dev(bounds.origin.y + bounds.size.height),
        );
        let read = |list: &[(f32, LayoutId)],
                    snapped: f32,
                    far: bool,
                    vertical: bool,
                    window: &mut Window| {
            let results: Vec<(f32, bool)> = list
                .iter()
                .map(|&(t, marker)| {
                    let o = window.layout_bounds(marker).origin;
                    let v = dev(if vertical { o.y } else { o.x });
                    // A near marker rounds up, and a far one stays up, when the edge is past `t`.
                    let past = if far {
                        v > snapped - 0.5
                    } else {
                        v > snapped + 0.5
                    };
                    (t, past)
                })
                .collect();
            Span::narrowed(&results)
        };
        let edges = Edges {
            left: read(&probes.left, x0, false, false, window),
            top: read(&probes.top, y0, false, true, window),
            right: read(&probes.right, x1, true, false, window),
            bottom: read(&probes.bottom, y1, true, true, window),
        };
        if let Some(id) = id {
            window.with_element_state::<Edges, _>(id, |_, _| ((), edges));
        }
        let settled = [edges.left, edges.top, edges.right, edges.bottom]
            .iter()
            .all(|e| e.settled());
        let moved = [
            (probes.edges.left, edges.left),
            (probes.edges.top, edges.top),
            (probes.edges.right, edges.right),
            (probes.edges.bottom, edges.bottom),
        ]
        .iter()
        .any(|(a, b)| a != b);
        if !settled && moved {
            window.request_animation_frame();
        }
        // Unrounded edges in device px, and the content's unrounded size in px.
        let (ux0, uy0) = (x0 + edges.left.mid(), y0 + edges.top.mid());
        let (ux1, uy1) = (x1 + edges.right.mid(), y1 + edges.bottom.mid());
        let (w, h) = (((ux1 - ux0) / s).max(0.), ((uy1 - uy0) / s).max(0.));
        // The root starts one device px before the snapped corner, so the shift is positive.
        let (sx, sy) = (edges.left.mid() + 1., edges.top.mid() + 1.);
        let dp = 1. / s;
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_none().w(px(dp)).aspect_ratio(1. / sy))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_none().h(px(dp)).aspect_ratio(sx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(child),
                    ),
            )
            .into_any_element();
        let origin = gpui_kit::point(px(x0 / s - dp), px(y0 / s - dp));
        let available = gpui_kit::size(px(w + sx * dp), px(h + sy * dp));
        root.prepaint_as_root(origin, available.into(), window, cx);
        self.root = Some(root);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Probes,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(root) = self.root.as_mut() {
            root.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Thresholds read against a known fraction, as the markers would round.
    fn frame(span: Span, d: f32) -> Span {
        let results: Vec<(f32, bool)> = span.thresholds().into_iter().map(|t| (t, d > t)).collect();
        Span::narrowed(&results)
    }

    #[test]
    fn an_edge_settles_finer_than_the_tie_nudge_in_three_frames() {
        let nudge = 2. * crate::style::TIE_NUDGE;
        for d in [-0.4372_f32, -0.01, nudge, 0.2031, 0.4999] {
            let mut s = Span::default();
            for _ in 0..3 {
                s = frame(s, d);
            }
            assert!(s.lo < d && d <= s.hi, "{d} outside {s:?}");
            assert!(s.hi - s.lo < nudge / 4., "{d}: {s:?}");
        }
    }

    #[test]
    fn a_moved_edge_starts_over_from_the_whole_pixel() {
        let mut s = Span::default();
        for _ in 0..3 {
            s = frame(s, 0.1);
        }
        let s = frame(s, -0.3);
        assert!(s.lo <= -0.3 && -0.3 <= s.hi && s.hi - s.lo <= 1. / STEPS as f32 + 1e-6);
    }
}
