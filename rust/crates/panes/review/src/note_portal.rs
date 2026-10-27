//! A line note's text box, drawn outside the card stack it sits in.
//!
//! The stack and the review around it are cached views, and GPUI re-renders a notified view's
//! ancestors, a cached one with every cache under it off: a key typed into a text box inside a
//! card would redraw every card on screen. So the card keeps the composer's frame and an empty
//! box at the text box's place ([`Slot`] records where it is and what clips it each time the
//! stack draws), and a host outside the cached views (the Observatory, the PR detail) mounts
//! [`portals`](crate::pane::Pane::portals): the text box laid out at that place in the same
//! frame. A key then redraws the text box and its host.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, AnyView, App, AvailableSpace, Bounds, ContentMask, Element, ElementId, EntityId,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, Position, Style, Window,
    size,
};

/// Where a text box's place sits this frame and what clips it, and the stack draw that saw it.
#[derive(Default)]
pub struct Slot {
    pub bounds: Cell<Option<Bounds<Pixels>>>,
    pub clip: Cell<Option<Bounds<Pixels>>>,
    pub drawn: Cell<u64>,
}

/// An open line note: its place and the view that draws its text box.
#[derive(Clone)]
pub struct Draft {
    pub card: EntityId,
    pub slot: Rc<Slot>,
    pub view: AnyView,
}

/// A text box at its slot. Prepaints after the stack (the host mounts it after the review), so
/// it reads where the slot sits this frame; a slot the stack did not draw this time (its card
/// left the window) draws nothing.
pub struct Portal {
    pub(crate) slot: Rc<Slot>,
    pub(crate) draws: Rc<Cell<u64>>,
    pub(crate) child: AnyElement,
    pub(crate) shown: Option<Bounds<Pixels>>,
}

impl IntoElement for Portal {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Portal {
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
        let style = Style {
            position: Position::Absolute,
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
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
        if self.slot.drawn.get() != self.draws.get() {
            return;
        }
        let (Some(at), Some(clip)) = (self.slot.bounds.get(), self.slot.clip.get()) else {
            return;
        };
        self.shown = Some(clip);
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            self.child.layout_as_root(
                size(
                    AvailableSpace::Definite(at.size.width),
                    AvailableSpace::Definite(at.size.height),
                ),
                window,
                cx,
            );
            self.child.prepaint_at(at.origin, window, cx);
        });
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
        if let Some(clip) = self.shown {
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                self.child.paint(window, cx)
            });
        }
    }
}
