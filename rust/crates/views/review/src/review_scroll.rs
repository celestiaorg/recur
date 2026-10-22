//! `lib/review-scroll.ts`.

/// `paneScrollTo`: the scrollTop that puts an element (`el_top`, viewport y) in the upper part
/// of the pane (`pane_top`, viewport y) so its thread shows below it.
pub fn pane_scroll_to(
    scroll_top: f32,
    pane_top: f32,
    client_h: f32,
    el_top: f32,
    bias: f32,
) -> f32 {
    (scroll_top + (el_top - pane_top) - client_h * bias).max(0.)
}

/// `paneScrollTo`'s default bias.
pub const PANE_SCROLL_BIAS: f32 = 0.28;

/// What a highlight jump scrolls to (`pickDiffJumpEl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JumpTarget {
    /// The inline thread anchored at the line.
    Thread,
    /// The diff row with that line.
    Row,
    /// The file itself.
    Root,
}

/// Prefer the inline thread, then the highlighted diff row.
pub fn pick_diff_jump(line: i64, has_thread: bool, has_row: bool) -> JumpTarget {
    if line > 0 && has_thread {
        JumpTarget::Thread
    } else if line > 0 && has_row {
        JumpTarget::Row
    } else {
        JumpTarget::Root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_inline_thread_over_the_diff_row() {
        assert_eq!(pick_diff_jump(12, true, true), JumpTarget::Thread);
    }

    #[test]
    fn falls_back_to_the_highlighted_row() {
        assert_eq!(pick_diff_jump(8, false, true), JumpTarget::Row);
        assert_eq!(pick_diff_jump(0, true, true), JumpTarget::Root);
    }

    #[test]
    fn scrolls_the_target_into_the_upper_third() {
        assert_eq!(pane_scroll_to(0., 0., 400., 820., PANE_SCROLL_BIAS), 708.);
        assert_eq!(pane_scroll_to(0., 100., 400., 120., PANE_SCROLL_BIAS), 0.);
    }
}
