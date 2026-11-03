//! Last-frame bounds of the regions the Observatory measures (`getBoundingClientRect`): the
//! maximize FLIP boxes, the split and beside drags, the explorer rows' scroller.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{Bounds, IntoElement, Pixels, Styled, canvas};

pub type BoundsCell = Rc<Cell<Bounds<Pixels>>>;

#[derive(Clone, Default)]
pub struct Geometry {
    /// `.obs`.
    pub obs: BoundsCell,
    /// `.obs-main`.
    pub main: BoundsCell,
    /// `.term-dock`.
    pub dock: BoundsCell,
    /// `.term-dock-slot` (the dock's place while it overlays).
    pub slot: BoundsCell,
    /// `.obs-editors` (the split drag's row).
    pub editors: BoundsCell,
    /// `.obs-files` (the explorer scroller).
    pub files: BoundsCell,
    /// The current image's box in the image pane.
    pub image_cur: BoundsCell,
    /// The base image's box in a compare.
    pub image_base: BoundsCell,
    /// The editor status bar, and its `.sp` spacer (zero wide once the items overflow).
    pub status: BoundsCell,
    pub status_sp: BoundsCell,
    /// The review's and the explorer's status bars and spacers, likewise.
    pub review_status: BoundsCell,
    pub review_status_sp: BoundsCell,
    pub files_status: BoundsCell,
    pub files_status_sp: BoundsCell,
}

/// An absolutely positioned probe that records its parent's bounds at prepaint.
pub fn probe(cell: &BoundsCell) -> impl IntoElement {
    let cell = cell.clone();
    canvas(move |b, _, _| cell.set(b), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}
