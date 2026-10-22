//! The `.diffscroll` pane a stack of diff files scrolls in, shared with the files so each can
//! window its rows against it (`root.closest('.diffscroll')`).

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Bounds, Pixels, ScrollHandle, point, px};

#[derive(Clone, Default)]
pub struct Pane {
    pub scroll: ScrollHandle,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Pane {
    pub fn new() -> Self {
        Self::default()
    }

    /// `scrollTop`.
    pub fn scroll_top(&self) -> f32 {
        -f32::from(self.scroll.offset().y)
    }

    pub fn set_scroll_top(&self, top: f32) {
        let max = self.max_scroll();
        let top = top.max(0.).min(max.max(0.));
        self.scroll.set_offset(point(px(0.), px(-top)));
    }

    /// `scrollHeight - clientHeight`.
    pub fn max_scroll(&self) -> f32 {
        f32::from(self.scroll.max_offset().y).max(0.)
    }

    /// The scroller's border box in window coordinates, from the last frame.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.bounds.get()
    }

    pub fn set_bounds(&self, b: Bounds<Pixels>) {
        self.bounds.set(Some(b));
    }

    /// The shared cell, for children that clip against the pane (the ⋯ menu).
    pub fn bounds_cell(&self) -> Rc<Cell<Option<Bounds<Pixels>>>> {
        self.bounds.clone()
    }

    /// `clientHeight`.
    pub fn client_height(&self) -> f32 {
        self.bounds().map_or(0., |b| f32::from(b.size.height))
    }

    /// The viewport y of the pane's top edge.
    pub fn top(&self) -> f32 {
        self.bounds().map_or(0., |b| f32::from(b.origin.y))
    }
}
