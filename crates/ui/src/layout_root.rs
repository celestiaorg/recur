//! A box whose content lays out as its own layout root.
//!
//! Taffy lays a nested flex child out once per layout pass of its parent, often at a new width
//! each time, so the cost of a subtree grows with how deep it sits rather than with its size: a
//! three-file diff in the PR page measured its text 67,000 times a frame. A [`LayoutRoot`] is a
//! single node sized by its own style in the outer layout; its content lays out once, in
//! prepaint, at the box's final bounds.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, AnyView, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ParentElement, Pixels, Position, Refineable, Style, StyleRefinement,
    Styled, Window, div, px,
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

fn node(style: Style, children: &[LayoutId], window: &mut Window, cx: &mut App) -> LayoutId {
    window.request_layout(style, children.iter().copied(), cx)
}

/// `child` laid out and prepainted as its own root over `bounds`; paint the returned root. For
/// content an element can only build after layout.
pub fn prepaint_over(
    bounds: Bounds<Pixels>,
    child: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut root = div()
        .size_full()
        .flex()
        .flex_col()
        .child(child)
        .into_any_element();
    root.prepaint_as_root(bounds.origin, bounds.size.into(), window, cx);
    root
}

impl Element for LayoutRoot {
    type RequestLayoutState = ();
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
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.refine(&self.style);
        if style.position != Position::Absolute {
            style.position = Position::Relative;
        }
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(child) = self.child.take() else {
            return;
        };
        self.root = Some(prepaint_over(bounds, child, window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
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
/// conversation's comments). A new height lays the block out again on the next frame.
pub struct LayoutBlock {
    id: ElementId,
    estimate: f32,
    child: Option<Content>,
    root: Option<AnyElement>,
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
    }
}

/// What a [`cached_block`] and its view share: the height the view measured when it last laid
/// out.
#[derive(Default)]
pub struct BlockSlot {
    measured: std::cell::Cell<Option<f32>>,
    markers: Rc<RefCell<BottomMarkers>>,
}

/// A cached block's content, as its view renders it.
pub fn block_content(slot: &Rc<BlockSlot>, child: impl IntoElement) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(Origin(slot.markers.clone()))
        .child(child)
        .child(Bottom(slot.markers.clone()))
        .child(Measure(slot.clone()))
        .into_any_element()
}

/// The content's height, read off the markers of the last layout.
fn read_bottom(m: &BottomMarkers, window: &mut Window) -> Option<f32> {
    let (o, at) = (m.origin?, m.at?);
    let y = |id: LayoutId, window: &mut Window| f32::from(window.layout_bounds(id).origin.y);
    Some(y(at, window) - y(o, window))
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
pub struct BlockState {
    /// The content's height, px (`None` before the first layout).
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

/// Places [`BottomMarkers`] at the content's bottom (`top: 100%` of its parent).
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
        let m = self.0.markers.borrow();
        self.0.measured.set(read_bottom(&m, window));
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
    type RequestLayoutState = BlockState;
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
    ) -> (LayoutId, BlockState) {
        let state = id.map_or_else(BlockState::default, |id| {
            window.with_element_state::<BlockState, _>(id, |s, _| {
                let s = s.unwrap_or_default();
                (s, s)
            })
        });
        let mut style = Style {
            flex_shrink: 0.,
            ..Default::default()
        };
        style.size.width = gpui_kit::relative(1.).into();
        style.size.height = px(state.height.unwrap_or(self.estimate).max(0.)).into();
        (node(style, &[], window, cx), state)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut BlockState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(child) = self.child.take() else {
            return;
        };
        let mut state = *state;
        let width = gpui_kit::AvailableSpace::Definite(bounds.size.width);
        let child = match child {
            Content::Inline(child) => child,
            Content::Cached(view, slot) => {
                let h = state.height.unwrap_or(self.estimate).max(0.);
                let entity = view.entity_id();
                let mut root = view
                    .cached(gpui_kit::StyleRefinement::default().size_full())
                    .into_any_element();
                root.layout_as_root(
                    gpui_kit::size(width, gpui_kit::AvailableSpace::Definite(px(h))),
                    window,
                    cx,
                );
                root.prepaint_at(bounds.origin, window, cx);
                if let Some(height) = slot.measured.take()
                    && state.height.is_none_or(|h| (h - height).abs() > 1e-3)
                {
                    state.height = Some(height);
                    settle_frame(window, Some(entity), cx);
                }
                self.finish(id, state, root, window);
                return;
            }
        };
        let mut root = div()
            .w_full()
            .flex()
            .flex_col()
            .child(child)
            .into_any_element();
        let height = f32::from(
            root.layout_as_root(
                gpui_kit::size(width, gpui_kit::AvailableSpace::MaxContent),
                window,
                cx,
            )
            .height,
        );
        root.prepaint_at(bounds.origin, window, cx);
        if state.height.is_none_or(|h| (h - height).abs() > 1e-3) {
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
        _: &mut BlockState,
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
                .child(div().flex_none().h(px(10.)))
                .child(super::layout_block("a", 5., div().h(px(40.)).w_full()))
                .child(super::layout_block("b", 5., div().h(px(30.)).w_full()))
                .child(div().h(px(5.)).debug_selector(|| "after".into()))
        }
    }

    #[gpui_kit::test]
    fn blocks_take_their_contents_height(cx: &mut gpui_kit::TestAppContext) {
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
        let after = cx.debug_bounds("after").expect("laid out");
        let y = f32::from(after.origin.y);
        assert!((y - 80.).abs() < 0.01, "after at {y}");
    }

    thread_local! {
        static BODY_RENDERS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    struct Body {
        h: f32,
        slot: std::rc::Rc<super::BlockSlot>,
    }

    impl Render for Body {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            BODY_RENDERS.with(|n| n.set(n.get() + 1));
            let h = self.h;
            super::block_content(
                &self.slot,
                div()
                    .h(px(h))
                    .w_full()
                    .debug_selector(move || format!("body:{h}")),
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
        assert!((y - 80.).abs() < 0.01, "after at {y}");
        let b = cx.debug_bounds("body:30").expect("b laid out");
        assert!((f32::from(b.origin.y) - 50.).abs() < 0.01, "b at {b:?}");
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

    struct ArmsOnce(std::rc::Rc<std::cell::Cell<u32>>, bool);

    impl Render for ArmsOnce {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            if !std::mem::replace(&mut self.1, true) {
                let ran = self.0.clone();
                window.after_next_draw(move |window, _| {
                    ran.set(ran.get() + 1);
                    window.after_next_draw(move |_, _| ran.set(ran.get() + 1));
                });
            }
            div()
        }
    }

    /// The macOS HTML preview hides its page from `Window::after_next_draw` (vendored
    /// gpui-pre): a callback armed in a draw runs at its end, one armed from that callback after
    /// the next draw, and neither on an idle frame.
    #[gpui_kit::test]
    fn after_next_draw_runs_once_per_draw(cx: &mut gpui_kit::TestAppContext) {
        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let (_, cx) = cx.add_window_view({
            let ran = ran.clone();
            |_, _| ArmsOnce(ran, false)
        });
        assert_eq!(ran.get(), 1, "runs at the end of the draw that armed it");
        cx.update(|window, cx| window.simulate_next_frame(cx));
        assert_eq!(ran.get(), 1, "an idle frame runs nothing");
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        assert_eq!(ran.get(), 2);
    }

    struct Page;

    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui_kit::canvas(
                |b, window, _| window.insert_hitbox(b, gpui_kit::HitboxBehavior::Native),
                |b, _, window, _| window.paint_hole(b),
            )
            .size_full()
        }
    }

    struct PageHost(gpui_kit::Entity<Page>);

    impl Render for PageHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut style = div().size_full();
            let style = style.style().clone();
            div()
                .size_full()
                .bg(gpui_kit::white())
                .child(crate::style::cached_view(&self.0, style))
                .child(
                    gpui_kit::deferred(
                        div()
                            .id("menu")
                            .absolute()
                            .top(px(10.))
                            .left(px(20.))
                            .size(px(30.))
                            .occlude()
                            .bg(gpui_kit::black()),
                    )
                    .with_priority(1),
                )
        }
    }

    /// The macOS HTML preview's page sits behind GPUI's drawing (vendored gpui-pre): a hole
    /// clears the background over it and a native hitbox passes mouse events to it, while a menu
    /// painted later stays over both, also when the cached view around the page replays.
    #[gpui_kit::test]
    fn a_hole_and_native_hitbox_sit_under_overlays(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, point};
        let (host, cx) = cx.add_window_view(|_, cx| PageHost(cx.new(|_| Page)));
        let probe = |cx: &mut gpui_kit::VisualTestContext| {
            cx.update(|window, _| {
                (
                    window.scene_batch_labels(),
                    window.native_hit_test(point(px(5.), px(5.))),
                    window.native_hit_test(point(px(30.), px(20.))),
                )
            })
        };
        let frame = (
            vec![
                "quads (1)".to_string(),
                "holes (1)".into(),
                "quads (1)".into(),
            ],
            true,
            false,
        );
        assert_eq!(
            probe(cx),
            frame,
            "page over the background, menu over the page"
        );
        cx.update(|_, cx| host.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        assert_eq!(
            probe(cx),
            frame,
            "a replayed cached view keeps the hole and hitbox"
        );
    }
}
