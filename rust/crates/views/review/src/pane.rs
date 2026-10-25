//! The `.diffscroll` pane a stack of diff files scrolls in, shared with the files so each can
//! window its rows against it (`root.closest('.diffscroll')`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::{AnyElement, Bounds, EntityId, IntoElement, Pixels, ScrollHandle, point, px};

use crate::note_portal::{Draft, Portal};

#[derive(Clone, Default)]
pub struct Pane {
    pub scroll: ScrollHandle,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// How many times the stack in this pane has drawn: a note's slot drawn in another draw
    /// is off screen.
    draws: Rc<Cell<u64>>,
    /// The line notes open in this pane's cards (see [`crate::note_portal`]).
    drafts: Rc<RefCell<Vec<Draft>>>,
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

    /// The stack is drawing again.
    pub fn begin_draw(&self) {
        self.draws.set(self.draws.get() + 1);
    }

    pub fn draws(&self) -> u64 {
        self.draws.get()
    }

    pub fn set_draft(&self, draft: Draft) {
        let mut d = self.drafts.borrow_mut();
        d.retain(|x| x.card != draft.card);
        d.push(draft);
    }

    pub fn clear_draft(&self, card: EntityId) {
        self.drafts.borrow_mut().retain(|x| x.card != card);
    }

    /// The open line notes' text boxes, for a host outside the cached review to mount after it.
    pub fn portals(&self) -> Vec<AnyElement> {
        self.drafts
            .borrow()
            .iter()
            .map(|d| {
                Portal {
                    slot: d.slot.clone(),
                    draws: self.draws.clone(),
                    child: d.view.clone().into_any_element(),
                    shown: None,
                }
                .into_any_element()
            })
            .collect()
    }
}
