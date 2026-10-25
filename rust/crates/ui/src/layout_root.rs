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

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, AnyView, App, Bounds, Element, ElementId, FlexDirection, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, ParentElement, Pixels, Position, Refineable, Style,
    StyleRefinement, Styled, Window, div, px,
};

/// Frames layout roots and blocks asked for to settle (`take_settle_frames`).
static SETTLE_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Asks for another frame. `request_animation_frame` notifies the view being drawn, which also
/// wakes everything observing it; a cached block notifies its own view instead, which redraws
/// that view and dirties its ancestors (so the block is laid out again) and nothing else: a PR
/// conversation's items all observe the PR detail drawing them.
fn settle_frame(window: &mut Window, view: Option<gpui_kit::EntityId>, cx: &mut App) {
    SETTLE_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match view {
        Some(v) => {
            // Dirty from inside the draw: applied to whichever draw comes next.
            cx.notify(v);
            window.on_next_frame(move |_, cx| cx.notify(v));
        }
        None => window.request_animation_frame(),
    }
}

/// How many frames layout roots and blocks have asked for since the last call: each one
/// notifies the view that drew them, so a root that never settles redraws its view forever.
pub fn take_settle_frames() -> u64 {
    SETTLE_FRAMES.swap(0, std::sync::atomic::Ordering::Relaxed)
}

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
    /// The thresholds to test this frame: the whole pixel, then the interval left last frame. A
    /// settled edge only checks its interval's ends, and starts over if it moved past one.
    fn thresholds(self) -> Vec<f32> {
        let n = STEPS as f32;
        let mut t: Vec<f32> = if self.settled() {
            vec![self.lo, self.hi]
        } else {
            (1..STEPS).map(|k| -0.5 + k as f32 / n).collect()
        };
        if !self.settled() && self.hi - self.lo < 1. - 1e-6 {
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
        if !settled {
            settle_frame(window, None, cx);
        }
        // Unrounded edges in device px, and the content's unrounded size in px.
        let (ux0, uy0) = (x0 + edges.left.mid(), y0 + edges.top.mid());
        let (ux1, uy1) = (x1 + edges.right.mid(), y1 + edges.bottom.mid());
        let (w, h) = (((ux1 - ux0) / s).max(0.), ((uy1 - uy0) / s).max(0.));
        // The root starts one device px before the snapped corner, so the shift is positive.
        let (sx, sy) = (edges.left.mid() + 1., edges.top.mid() + 1.);
        let dp = 1. / s;
        // The content box sits at the shift and takes the unrounded size, both as percentages
        // of the root (authored lengths would round to device pixels), and lays out once: no
        // flex container above it to measure it again.
        let (aw, ah) = (w + sx * dp, h + sy * dp);
        let mut content = div().absolute().flex().flex_col().child(child);
        let st = content.style();
        st.inset.left = Some(gpui_kit::relative(sx * dp / aw.max(1e-6)).into());
        st.inset.top = Some(gpui_kit::relative(sy * dp / ah.max(1e-6)).into());
        st.size.width = Some(gpui_kit::relative(w / aw.max(1e-6)).into());
        st.size.height = Some(gpui_kit::relative(h / ah.max(1e-6)).into());
        let mut root = div()
            .size_full()
            .relative()
            .child(content)
            .into_any_element();
        let origin = gpui_kit::point(px(x0 / s - dp), px(y0 / s - dp));
        let available = gpui_kit::size(px(aw), px(ah));
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

/// A block that takes its content's height, the content laid out once, as its own root, at the
/// block's width: for list items whose nesting would have layout lay them out over and over (a PR
/// conversation's comments). Like [`LayoutRoot`] it reads its unrounded left, top and right edges
/// off probe markers and lays the content out shifted by the remainder; markers under the content
/// read its unrounded height, which the block takes (whole device pixels, then an `aspect_ratio`
/// spacer for the rest), so what follows lands exactly where it would in place. A new height
/// lays the block out again on the next frame.
pub struct LayoutBlock {
    id: ElementId,
    estimate: f32,
    child: Option<Content>,
    root: Option<AnyElement>,
    markers: Rc<RefCell<BottomMarkers>>,
}

enum Content {
    Inline(AnyElement),
    Cached(AnyView, Rc<BlockSlot>),
}

/// `child` as a [`LayoutBlock`], `estimate` px tall until its first layout.
pub fn layout_block(
    id: impl Into<ElementId>,
    estimate: f32,
    child: impl IntoElement,
) -> LayoutBlock {
    LayoutBlock {
        id: id.into(),
        estimate,
        child: Some(Content::Inline(child.into_any_element())),
        root: None,
        markers: Rc::default(),
    }
}

/// `view` as a [`LayoutBlock`] whose content is a cached view: a redraw of the page that does
/// not dirty `view` replays it, laid out exactly where the content of a [`layout_block`] would
/// be. The view renders its content through [`block_content`] with the same `slot`.
pub fn cached_block<V: gpui_kit::Render>(
    id: impl Into<ElementId>,
    estimate: f32,
    view: &gpui_kit::Entity<V>,
    slot: &Rc<BlockSlot>,
    cx: &App,
) -> LayoutBlock {
    // The window tracks a cached view it saw accessed (`style::cached_view`).
    let _ = view.read(cx);
    LayoutBlock {
        id: id.into(),
        estimate,
        child: Some(Content::Cached(view.clone().into(), slot.clone())),
        root: None,
        markers: slot.markers.clone(),
    }
}

/// Where a cached block's content sits in its root, and the bottom span to probe.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    /// Device px from the root's corner to the content's.
    sx: f32,
    sy: f32,
    /// The content's unrounded width and the root's snapped width, px.
    w: f32,
    root_w: f32,
    bottom: Span,
}

/// What a [`cached_block`] and its view share: the place the block computed this frame, the
/// place the view last rendered at, and the height the view measured when it last laid out.
#[derive(Default)]
pub struct BlockSlot {
    place: std::cell::Cell<Option<Place>>,
    rendered: std::cell::Cell<Option<Place>>,
    measured: std::cell::Cell<Option<(f32, Span)>>,
    markers: Rc<RefCell<BottomMarkers>>,
}

/// A cached block's content, as its view renders it.
pub fn block_content(slot: &Rc<BlockSlot>, child: impl IntoElement, window: &Window) -> AnyElement {
    let Some(p) = slot.place.get() else {
        return div().w_full().child(child).into_any_element();
    };
    slot.rendered.set(Some(p));
    slot.markers.borrow_mut().thresholds = p.bottom.thresholds();
    let dp = 1. / window.scale_factor();
    let mut content = div()
        .absolute()
        .flex()
        .flex_col()
        .child(child)
        .child(Bottom(slot.markers.clone()));
    let st = content.style();
    st.inset.left = Some(gpui_kit::relative(p.sx * dp / p.root_w.max(1e-6)).into());
    st.inset.top = Some(px(0.).into());
    st.size.width = Some(gpui_kit::relative(p.w.max(0.) / p.root_w.max(1e-6)).into());
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(Origin(slot.markers.clone()))
        .child(div().flex_none().w(px(dp)).aspect_ratio(1. / p.sy))
        .child(div().relative().w_full().child(content))
        .child(Measure(slot.clone()))
        .into_any_element()
}

/// The content's unrounded height under its root's top spacer of `sy` device px, and where its
/// bottom sits past its snapped position, read off the markers of the last layout.
fn read_bottom(m: &BottomMarkers, sy: f32, window: &mut Window) -> Option<(f32, Span)> {
    let (o, at) = (m.origin?, m.at?);
    let s = window.scale_factor();
    let dev = |v: Pixels| (f32::from(v) * s).round();
    let base = dev(window.layout_bounds(o).origin.y);
    let rb = dev(window.layout_bounds(at).origin.y) - base;
    let results: Vec<(f32, bool)> = m
        .under
        .iter()
        .map(|&(t, mk)| (t, dev(window.layout_bounds(mk).origin.y) - base > rb + 0.5))
        .collect();
    let bottom = Span::narrowed(&results);
    Some(((rb + bottom.mid() - sy) / s, bottom))
}

/// GPUI's snap of a device-px coordinate: half pixels toward zero.
fn round_half_toward_zero(v: f32) -> f32 {
    (v.abs() - 0.5).ceil().copysign(v)
}

/// Reads a cached block's height once its view has laid out.
struct Measure(Rc<BlockSlot>);

impl IntoElement for LayoutBlock {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

#[derive(Clone, Copy, Default)]
struct BlockState {
    left: Span,
    top: Span,
    right: Span,
    /// Where the content's bottom sits past its snapped position.
    bottom: Span,
    /// The content's unrounded height, px (`None` before the first layout).
    height: Option<f32>,
}

/// Markers in the content's own layout: one at its root's origin (every reading is relative to
/// it: a bounds read after the root's prepaint lacks the root's offset), one at the content's
/// bottom, then one per threshold under it.
#[derive(Default)]
struct BottomMarkers {
    thresholds: Vec<f32>,
    origin: Option<LayoutId>,
    at: Option<LayoutId>,
    under: Vec<(f32, LayoutId)>,
}

/// Records the content root's origin.
struct Origin(Rc<RefCell<BottomMarkers>>);

/// Places [`BottomMarkers`] at the content's bottom (`top: 100%` of the content box).
struct Bottom(Rc<RefCell<BottomMarkers>>);

macro_rules! marker_element {
    ($t:ty, $req:expr) => {
        impl IntoElement for $t {
            type Element = Self;

            fn into_element(self) -> Self {
                self
            }
        }

        impl Element for $t {
            type RequestLayoutState = ();
            type PrepaintState = ();

            fn id(&self) -> Option<ElementId> {
                None
            }

            fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
                None
            }

            fn request_layout(
                &mut self,
                _: Option<&GlobalElementId>,
                _: Option<&InspectorElementId>,
                window: &mut Window,
                cx: &mut App,
            ) -> (LayoutId, ()) {
                ($req)(self, window, cx)
            }

            fn prepaint(
                &mut self,
                _: Option<&GlobalElementId>,
                _: Option<&InspectorElementId>,
                _: Bounds<Pixels>,
                _: &mut (),
                _: &mut Window,
                _: &mut App,
            ) {
            }

            fn paint(
                &mut self,
                _: Option<&GlobalElementId>,
                _: Option<&InspectorElementId>,
                _: Bounds<Pixels>,
                _: &mut (),
                _: &mut (),
                _: &mut Window,
                _: &mut App,
            ) {
            }
        }
    };
}

fn zero(window: &mut Window, cx: &mut App) -> LayoutId {
    node(
        Style {
            flex_shrink: 0.,
            ..Default::default()
        },
        &[],
        window,
        cx,
    )
}

marker_element!(Origin, |o: &mut Origin,
                         window: &mut Window,
                         cx: &mut App| {
    let m = zero(window, cx);
    o.0.borrow_mut().origin = Some(m);
    (m, ())
});

marker_element!(Bottom, |b: &mut Bottom,
                         window: &mut Window,
                         cx: &mut App| {
    let dp = 1. / window.scale_factor();
    let below = || Style {
        display: gpui_kit::Display::Flex,
        flex_direction: FlexDirection::Column,
        position: Position::Absolute,
        inset: gpui_kit::Edges {
            top: gpui_kit::relative(1.).into(),
            left: px(0.).into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut m = b.0.borrow_mut();
    let thresholds = m.thresholds.clone();
    let at = zero(window, cx);
    let mut kids = vec![node(below(), &[at], window, cx)];
    m.at = Some(at);
    m.under.clear();
    for t in thresholds {
        // The bottom is past `t` when a marker `0.5 - t` under it rounds up.
        let mk = zero(window, cx);
        let gap = spacer((0.5 - t) * dp, true, dp, window, cx);
        kids.push(node(below(), &[gap, mk], window, cx));
        m.under.push((t, mk));
    }
    let wrap = Style {
        position: Position::Absolute,
        size: gpui_kit::Size {
            width: gpui_kit::relative(1.).into(),
            height: gpui_kit::relative(1.).into(),
        },
        ..Default::default()
    };
    (node(wrap, &kids, window, cx), ())
});

impl IntoElement for Measure {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Measure {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let s = Style {
            position: Position::Absolute,
            ..Default::default()
        };
        (node(s, &[], window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let Some(p) = self.0.rendered.get() else {
            return;
        };
        let m = self.0.markers.borrow();
        self.0.measured.set(read_bottom(&m, p.sy, window));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }
}

pub struct BlockProbes {
    state: BlockState,
    left: Vec<(f32, LayoutId)>,
    top: Vec<(f32, LayoutId)>,
    right: Vec<(f32, LayoutId)>,
}

impl LayoutBlock {
    fn finish(
        &mut self,
        id: Option<&GlobalElementId>,
        state: BlockState,
        root: AnyElement,
        view: Option<gpui_kit::EntityId>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let settled = [state.left, state.top, state.right, state.bottom]
            .iter()
            .all(|e| e.settled());
        if !settled {
            settle_frame(window, view, cx);
        }
        if let Some(id) = id {
            window.with_element_state::<BlockState, _>(id, |_, _| ((), state));
        }
        self.root = Some(root);
    }
}

impl Element for LayoutBlock {
    type RequestLayoutState = BlockProbes;
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
    ) -> (LayoutId, BlockProbes) {
        let state = id.map_or_else(BlockState::default, |id| {
            window.with_element_state::<BlockState, _>(id, |s, _| {
                let s = s.unwrap_or_default();
                (s, s)
            })
        });
        let scale = window.scale_factor();
        let dp = 1. / scale;
        let mut probes = Vec::new();
        let mut place = |span: Span, vertical: bool, far: bool, window: &mut Window| {
            span.thresholds()
                .into_iter()
                .map(|t| {
                    let offset = if far { t + 0.5 } else { 0.5 - t };
                    let (wrap, marker) = probe(offset * dp, vertical, far, dp, window, cx);
                    probes.push(wrap);
                    (t, marker)
                })
                .collect::<Vec<_>>()
        };
        let left = place(state.left, false, false, window);
        let top = place(state.top, true, false, window);
        let right = place(state.right, false, true, window);
        self.markers.borrow_mut().thresholds = state.bottom.thresholds();
        // Whole device pixels in the box, the rest in a spacer under it.
        let h = state.height.unwrap_or(self.estimate).max(0.);
        let whole = (h * scale).floor() / scale;
        let mut boxed = Style {
            position: Position::Relative,
            flex_shrink: 0.,
            ..Default::default()
        };
        boxed.size.width = gpui_kit::relative(1.).into();
        boxed.size.height = px(whole).into();
        let mut kids = vec![node(boxed, &probes, window, cx)];
        if h - whole > 1e-4 {
            kids.push(spacer(h - whole, true, dp, window, cx));
        }
        let mut outer = Style {
            display: gpui_kit::Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.,
            ..Default::default()
        };
        outer.size.width = gpui_kit::relative(1.).into();
        (
            node(outer, &kids, window, cx),
            BlockProbes {
                state,
                left,
                top,
                right,
            },
        )
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        probes: &mut BlockProbes,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(child) = self.child.take() else {
            return;
        };
        let s = window.scale_factor();
        let dp = 1. / s;
        let dev = |v: Pixels| (f32::from(v) * s).round();
        let (x0, y0) = (dev(bounds.origin.x), dev(bounds.origin.y));
        let x1 = dev(bounds.origin.x + bounds.size.width);
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
                    (
                        t,
                        if far {
                            v > snapped - 0.5
                        } else {
                            v > snapped + 0.5
                        },
                    )
                })
                .collect();
            Span::narrowed(&results)
        };
        let mut state = probes.state;
        state.left = read(&probes.left, x0, false, false, window);
        state.top = read(&probes.top, y0, false, true, window);
        state.right = read(&probes.right, x1, true, false, window);
        let (sx, sy) = (state.left.mid() + 1., state.top.mid() + 1.);
        let w = ((x1 + state.right.mid()) - (x0 + state.left.mid())) / s;
        let aw = (w + sx * dp).max(1e-6);
        let origin = gpui_kit::point(px(x0 / s - dp), px(y0 / s - dp));
        let child = match child {
            Content::Inline(child) => child,
            Content::Cached(view, slot) => {
                let place = Place {
                    sx,
                    sy,
                    w,
                    root_w: round_half_toward_zero(aw * s) / s,
                    bottom: state.bottom,
                };
                slot.place.set(Some(place));
                let h = state.height.unwrap_or(self.estimate).max(0.);
                let entity = view.entity_id();
                let mut root = view
                    .cached(gpui_kit::StyleRefinement::default().size_full())
                    .into_any_element();
                root.layout_as_root(
                    gpui_kit::size(
                        gpui_kit::AvailableSpace::Definite(px(aw)),
                        gpui_kit::AvailableSpace::Definite(px(sy * dp + h)),
                    ),
                    window,
                    cx,
                );
                root.prepaint_at(origin, window, cx);
                // A view replayed from its cache keeps the place it rendered at: render it
                // again at this one on the next frame.
                let mut settle = slot.rendered.get() != Some(place);
                if let Some((height, bottom)) = slot.measured.take() {
                    state.bottom = bottom;
                    if state.height.is_none_or(|h| (h - height).abs() > 1e-3) {
                        state.height = Some(height);
                        settle = true;
                    }
                }
                if settle {
                    settle_frame(window, Some(entity), cx);
                }
                self.finish(id, state, root, Some(entity), window, cx);
                return;
            }
        };
        let mut content = div()
            .absolute()
            .flex()
            .flex_col()
            .child(child)
            .child(Bottom(self.markers.clone()));
        let st = content.style();
        st.inset.left = Some(gpui_kit::relative(sx * dp / aw).into());
        st.inset.top = Some(px(0.).into());
        st.size.width = Some(gpui_kit::relative(w.max(0.) / aw).into());
        let mut root = div()
            .w_full()
            .flex()
            .flex_col()
            .child(Origin(self.markers.clone()))
            .child(div().flex_none().w(px(dp)).aspect_ratio(1. / sy))
            .child(div().relative().w_full().child(content))
            .into_any_element();
        root.layout_as_root(
            gpui_kit::size(
                gpui_kit::AvailableSpace::Definite(px(aw)),
                gpui_kit::AvailableSpace::MaxContent,
            ),
            window,
            cx,
        );
        root.prepaint_at(origin, window, cx);
        // The content's bottom in its root's device pixels: snapped at the marker on it, its
        // fraction from the markers under it.
        let read = read_bottom(&self.markers.borrow(), sy, window);
        if let Some((height, bottom)) = read {
            state.bottom = bottom;
            if state.height.is_none_or(|h| (h - height).abs() > 1e-3) {
                state.height = Some(height);
                settle_frame(window, None, cx);
            }
        }
        self.finish(id, state, root, None, window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut BlockProbes,
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
    fn a_settled_edge_holds_on_two_markers_and_reopens_when_it_moves() {
        let mut s = Span::default();
        for _ in 0..3 {
            s = frame(s, 0.1);
        }
        assert!(s.settled());
        assert_eq!(s.thresholds().len(), 2);
        assert_eq!(frame(s, 0.1), s);
        let moved = frame(s, 0.3);
        assert!(!moved.settled() && moved.lo < 0.3 && 0.3 <= moved.hi);
        let mut s = moved;
        for _ in 0..3 {
            s = frame(s, 0.3);
        }
        assert!(s.settled() && s.lo < 0.3 && 0.3 <= s.hi);
    }

    #[test]
    fn a_moved_edge_starts_over_from_the_whole_pixel() {
        let mut s = Span::default();
        for _ in 0..3 {
            s = frame(s, 0.1);
        }
        let s = frame(s, -0.3);
        assert!(s.lo < -0.3 && -0.3 <= s.hi && !s.settled());
        let s = frame(s, -0.3);
        assert!(s.lo < -0.3 && -0.3 <= s.hi && s.hi - s.lo <= 1. / STEPS as f32 + 1e-6);
    }
}

#[cfg(test)]
mod block_tests {
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div, px,
    };

    struct Col;

    impl Render for Col {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_none().w(px(1.)).aspect_ratio(1. / 0.3))
                .child(div().flex_none().h(px(10.)))
                .child(super::layout_block("a", 5., div().h(px(40.)).w_full()))
                .child(super::layout_block("b", 5., div().h(px(30.)).w_full()))
                .child(div().h(px(5.)).debug_selector(|| "after".into()))
        }
    }

    #[gpui_kit::test]
    fn blocks_take_their_contents_height_from_a_fractional_offset(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::init(cx);
        });
        let (_view, cx) = cx.add_window_view(|_, _| Col);
        for _ in 0..6 {
            cx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
        }
        // 10px + 0.3px (a fractional offset) + 10px + 40px + 30px, snapped as in place.
        let after = cx.debug_bounds("after").expect("laid out");
        let y = f32::from(after.origin.y);
        assert!((y - 80.5).abs() < 0.01, "after at {y}");
    }

    thread_local! {
        static BODY_RENDERS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    struct Body {
        h: f32,
        slot: std::rc::Rc<super::BlockSlot>,
    }

    impl Render for Body {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            BODY_RENDERS.with(|n| n.set(n.get() + 1));
            let h = self.h;
            super::block_content(
                &self.slot,
                div()
                    .h(px(h))
                    .w_full()
                    .debug_selector(move || format!("body:{h}")),
                window,
            )
        }
    }

    struct CachedCol {
        a: gpui_kit::Entity<Body>,
        b: gpui_kit::Entity<Body>,
    }

    impl Render for CachedCol {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let (sa, sb) = (self.a.read(cx).slot.clone(), self.b.read(cx).slot.clone());
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_none().w(px(1.)).aspect_ratio(1. / 0.3))
                .child(div().flex_none().h(px(10.)))
                .child(super::cached_block("a", 5., &self.a, &sa, cx))
                .child(super::cached_block("b", 5., &self.b, &sb, cx))
                .child(div().h(px(5.)).debug_selector(|| "after".into()))
        }
    }

    #[gpui_kit::test]
    fn cached_blocks_lay_out_as_inline_ones_and_replay_on_a_host_redraw(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::init(cx);
        });
        let (view, cx) = cx.add_window_view(|_, cx| {
            let body = |h, cx: &mut gpui_kit::App| {
                cx.new(|_| Body {
                    h,
                    slot: std::rc::Rc::default(),
                })
            };
            CachedCol {
                a: body(40., cx),
                b: body(30., cx),
            }
        });
        for _ in 0..6 {
            cx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
        }
        cx.run_until_parked();
        let y = f32::from(cx.debug_bounds("after").expect("laid out").origin.y);
        assert!((y - 80.5).abs() < 0.01, "after at {y}");
        let b = cx.debug_bounds("body:30").expect("b laid out");
        assert!((f32::from(b.origin.y) - 50.5).abs() < 0.01, "b at {b:?}");
        let before = BODY_RENDERS.with(|n| n.get());
        cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert_eq!(
            BODY_RENDERS.with(|n| n.get()),
            before,
            "a host redraw re-rendered a block"
        );
        assert!(
            cx.debug_bounds("body:30").is_some(),
            "a replayed block lost its content"
        );
    }
}
