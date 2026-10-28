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
//! The box reads its unrounded edges from layout (`Window::layout_bounds_unsnapped`, vendored
//! gpui-pre) and lays the content out shifted by the difference, so every edge lands where the
//! in-place layout puts it.

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

/// A box's snapped device edges and how far its unrounded edges sit past them (device px).
struct Fractions {
    x0: f32,
    y0: f32,
    left: f32,
    top: f32,
    /// Unrounded width and height, px.
    w: f32,
    h: f32,
}

impl Fractions {
    fn read(layout: LayoutId, bounds: Bounds<Pixels>, window: &mut Window) -> Fractions {
        let s = window.scale_factor();
        let dev = |v: Pixels| (f32::from(v) * s).round();
        let (x0, y0) = (dev(bounds.origin.x), dev(bounds.origin.y));
        let u = window.layout_bounds_unsnapped(layout);
        Fractions {
            x0,
            y0,
            left: f32::from(u.origin.x) * s - x0,
            top: f32::from(u.origin.y) * s - y0,
            w: f32::from(u.size.width),
            h: f32::from(u.size.height),
        }
    }
}

/// `child` laid out and prepainted as its own root over the box `layout` was laid out at
/// (`bounds`), its edges where in-place layout puts them; paint the returned root. For content
/// an element can only build after layout.
pub fn prepaint_over(
    layout: LayoutId,
    bounds: Bounds<Pixels>,
    child: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let s = window.scale_factor();
    let edges = Fractions::read(layout, bounds, window);
    let (x0, y0) = (edges.x0, edges.y0);
    let (sx, sy) = (edges.left + 1., edges.top + 1.);
    let (w, h) = (edges.w, edges.h);
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
    root
}

impl Element for LayoutRoot {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
    ) -> (LayoutId, LayoutId) {
        let mut style = Style::default();
        style.refine(&self.style);
        if style.position != Position::Absolute {
            style.position = Position::Relative;
        }
        let layout = window.request_layout(style, [], cx);
        (layout, layout)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(child) = self.child.take() else {
            return;
        };
        self.root = Some(prepaint_over(*layout, bounds, child, window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutId,
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
}

/// What a [`cached_block`] and its view share: the place the block computed this frame, the
/// place the view last rendered at, and the height the view measured when it last laid out.
#[derive(Default)]
pub struct BlockSlot {
    place: std::cell::Cell<Option<Place>>,
    rendered: std::cell::Cell<Option<Place>>,
    measured: std::cell::Cell<Option<f32>>,
    markers: Rc<RefCell<BottomMarkers>>,
}

/// A cached block's content, as its view renders it.
pub fn block_content(slot: &Rc<BlockSlot>, child: impl IntoElement, window: &Window) -> AnyElement {
    let Some(p) = slot.place.get() else {
        return div().w_full().child(child).into_any_element();
    };
    slot.rendered.set(Some(p));
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

/// The content's unrounded height under its root's top spacer of `sy` device px, read off the
/// markers of the last layout.
fn read_bottom(m: &BottomMarkers, sy: f32, window: &mut Window) -> Option<f32> {
    let (o, at) = (m.origin?, m.at?);
    let y =
        |id: LayoutId, window: &mut Window| f32::from(window.layout_bounds_unsnapped(id).origin.y);
    let dp = 1. / window.scale_factor();
    Some(y(at, window) - y(o, window) - sy * dp)
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
    /// The content's unrounded height, px (`None` before the first layout).
    height: Option<f32>,
}

/// Markers in the content's own layout: one at its root's origin (every reading is relative to
/// it: a bounds read after the root's prepaint lacks the root's offset) and one at the content's
/// bottom.
#[derive(Default)]
struct BottomMarkers {
    origin: Option<LayoutId>,
    at: Option<LayoutId>,
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
    let below = Style {
        position: Position::Absolute,
        inset: gpui_kit::Edges {
            top: gpui_kit::relative(1.).into(),
            left: px(0.).into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let at = zero(window, cx);
    b.0.borrow_mut().at = Some(at);
    let wrap = Style {
        position: Position::Absolute,
        size: gpui_kit::Size {
            width: gpui_kit::relative(1.).into(),
            height: gpui_kit::relative(1.).into(),
        },
        ..Default::default()
    };
    let below = node(below, &[at], window, cx);
    (node(wrap, &[below], window, cx), ())
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

pub struct BlockLayout {
    state: BlockState,
    layout: LayoutId,
}

impl LayoutBlock {
    fn finish(
        &mut self,
        id: Option<&GlobalElementId>,
        state: BlockState,
        root: AnyElement,
        window: &mut Window,
    ) {
        if let Some(id) = id {
            window.with_element_state::<BlockState, _>(id, |_, _| ((), state));
        }
        self.root = Some(root);
    }
}

impl Element for LayoutBlock {
    type RequestLayoutState = BlockLayout;
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
    ) -> (LayoutId, BlockLayout) {
        let state = id.map_or_else(BlockState::default, |id| {
            window.with_element_state::<BlockState, _>(id, |s, _| {
                let s = s.unwrap_or_default();
                (s, s)
            })
        });
        let scale = window.scale_factor();
        let dp = 1. / scale;
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
        let mut kids = vec![node(boxed, &[], window, cx)];
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
        let layout = node(outer, &kids, window, cx);
        (layout, BlockLayout { state, layout })
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        block: &mut BlockLayout,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(child) = self.child.take() else {
            return;
        };
        let s = window.scale_factor();
        let dp = 1. / s;
        let edges = Fractions::read(block.layout, bounds, window);
        let (x0, y0) = (edges.x0, edges.y0);
        let mut state = block.state;
        let (sx, sy) = (edges.left + 1., edges.top + 1.);
        let w = edges.w;
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
                if let Some(height) = slot.measured.take()
                    && state.height.is_none_or(|h| (h - height).abs() > 1e-3)
                {
                    state.height = Some(height);
                    settle = true;
                }
                if settle {
                    settle_frame(window, Some(entity), cx);
                }
                self.finish(id, state, root, window);
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
        // The content's unrounded height, read off the marker at its bottom.
        let read = read_bottom(&self.markers.borrow(), sy, window);
        if let Some(height) = read
            && state.height.is_none_or(|h| (h - height).abs() > 1e-3)
        {
            state.height = Some(height);
            settle_frame(window, None, cx);
        }
        self.finish(id, state, root, window);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut BlockLayout,
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
