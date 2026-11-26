//! Scroll anchoring for the card stack (CSS `overflow-anchor`): what the pane's top edge shows
//! stays put while cards mount, rows measure and heights settle above it. The shift is applied
//! in prepaint, where the new layout is known, so the frame that moved the content is the frame
//! that scrolls with it.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, point, px,
};

use crate::pane::Pane;
use crate::stack::STUB_CARD_H;

/// Where an anchor (a card, a row) is laid out this frame.
pub type LaidOut = Rc<Cell<Option<LayoutId>>>;

/// Something at the pane's top edge in the last frame, and its y in the content then.
pub struct Anchor {
    pub at: LaidOut,
    pub y: f32,
    /// A whole card's height then: one that shrinks (a collapse) keeps its header at the pane's
    /// top edge instead of scrolling past it.
    pub card_h: Option<f32>,
}

/// `child`, with its layout id in `at`.
pub struct Tracked {
    pub child: AnyElement,
    pub at: LaidOut,
}

impl IntoElement for Tracked {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Tracked {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let id = self.child.request_layout(window, cx);
        self.at.set(Some(id));
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
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
        self.child.paint(window, cx);
    }
}

/// The scrolled column: scrolls `pane` by however far the first anchor still laid out moved.
pub struct Anchored {
    pub child: AnyElement,
    pub pane: Pane,
    /// The scroll this frame was built for, before the scroller clamps it to the new content.
    pub scroll_top: f32,
    pub anchors: Vec<Anchor>,
    /// The column's window y, for the next frame's anchors.
    pub col_top: Rc<Cell<f32>>,
}

impl IntoElement for Anchored {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Anchored {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let id = self.child.request_layout(window, cx);
        (id, id)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        col: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let drawn = self.pane.scroll_top();
        for a in &self.anchors {
            let Some(id) = a.at.get() else { continue };
            let b = window.layout_bounds(id);
            let y = f32::from(b.top() - bounds.top());
            let mut top = self.scroll_top + y - a.y;
            let h = f32::from(b.size.height);
            if a.card_h.is_some_and(|was| h < was - 1.) {
                top = top.min((y + h - STUB_CARD_H).max(y));
            }
            self.pane.set_scroll_top(top);
            break;
        }
        let dy = drawn - self.pane.scroll_top();
        window.with_element_offset(point(px(0.), px(dy)), |window| {
            self.col_top
                .set(f32::from(window.layout_bounds(*col).top()));
            self.child.prepaint(window, cx);
        });
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
        self.child.paint(window, cx);
    }
}
