//! `lib/tree-sticky.ts`: stuck folder headers in the file tree; only the innermost one fades the
//! rows beneath it. Positions are viewport y (`getBoundingClientRect().top`); the tree passes
//! what the DOM walk read.

use recur_view_core::workspace_nav::scroll_delta_to_reveal;

pub const TREE_STICKY_SLACK: f32 = 1.5;

/// One open folder header: its depth (`--d`) and its painted top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickyHeader {
    pub depth: f32,
    pub top: f32,
}

fn header_stuck(h: &StickyHeader, tree_top: f32, row_h: f32, scrolled: bool) -> bool {
    scrolled && (h.top - (tree_top + h.depth * row_h)).abs() <= TREE_STICKY_SLACK
}

/// `syncTreeStickyFades`: `(is_stuck, fade_under)` per header. `fade_hits_live_row(i)` says
/// whether header `i`'s fade would cover the selected or a hovered row.
pub fn sync_tree_sticky_fades(
    scroll_top: f32,
    tree_top: f32,
    headers: &[StickyHeader],
    row_h: f32,
    fade_hits_live_row: impl Fn(usize) -> bool,
) -> Vec<(bool, bool)> {
    let scrolled = scroll_top > 1.;
    let mut inner: Option<usize> = None;
    let mut inner_depth = -1.;
    let stuck: Vec<bool> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let s = header_stuck(h, tree_top, row_h, scrolled);
            if s && h.depth >= inner_depth {
                inner = Some(i);
                inner_depth = h.depth;
            }
            s
        })
        .collect();
    stuck
        .iter()
        .enumerate()
        .map(|(i, &s)| (s, inner == Some(i) && !fade_hits_live_row(i)))
        .collect()
}

/// `fadeHitsLiveRow` for one header: any of `rows` (`(top, bottom)` of the selected row and
/// hovered rows, the header itself excluded) inside the fade below the header.
pub fn fade_hits_rows(header_bottom: f32, fade_h: f32, rows: &[(f32, f32)]) -> bool {
    let (a, b) = (header_bottom, header_bottom + fade_h);
    rows.iter().any(|&(top, bottom)| bottom > a && top < b)
}

/// `revealUnderSticky`: the scrollTop that brings a focused row (`top`, `bottom`) out from under
/// the stuck headers above it (`covering`: how many are stuck, excluding the row's own header).
#[allow(clippy::too_many_arguments)]
pub fn reveal_under_sticky(
    scroll_top: f32,
    max_scroll: f32,
    view_top: f32,
    view_bottom: f32,
    top: f32,
    bottom: f32,
    covering: usize,
    row_h: f32,
) -> f32 {
    let delta = scroll_delta_to_reveal(
        top,
        bottom,
        view_top,
        view_bottom,
        covering as f32 * row_h,
        0.,
    );
    if delta == 0. {
        return scroll_top;
    }
    (scroll_top + delta).max(0.).min(max_scroll.max(0.))
}

/// How many headers are stuck at `scroll_top` (for [`reveal_under_sticky`]), skipping `skip`.
pub fn stuck_count(
    scroll_top: f32,
    tree_top: f32,
    headers: &[StickyHeader],
    row_h: f32,
    skip: Option<usize>,
) -> usize {
    let scrolled = scroll_top > 1.;
    headers
        .iter()
        .enumerate()
        .filter(|(i, h)| Some(*i) != skip && header_stuck(h, tree_top, row_h, scrolled))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(depth: f32, top: f32) -> StickyHeader {
        StickyHeader { depth, top }
    }

    #[test]
    fn fades_nothing_at_the_top_of_the_list() {
        assert_eq!(
            sync_tree_sticky_fades(0., 0., &[h(0., 0.)], 24., |_| false),
            vec![(false, false)]
        );
    }

    #[test]
    fn fades_only_the_deepest_stuck_header() {
        let headers = [h(0., 0.), h(1., 24.)];
        let got = sync_tree_sticky_fades(40., 0., &headers, 24., |_| false);
        assert_eq!(got, vec![(true, false), (true, true)]);
        assert_eq!(
            sync_tree_sticky_fades(40., 0., &headers, 24., |_| false),
            got
        );
    }

    #[test]
    fn does_not_fade_over_a_selected_row() {
        let on = [(24., 48.)];
        let got = sync_tree_sticky_fades(40., 0., &[h(0., 0.)], 24., |_| {
            fade_hits_rows(24., 20., &on)
        });
        assert_eq!(got, vec![(true, false)]);
    }

    #[test]
    fn scrolls_a_focused_row_out_from_under_stuck_headers() {
        let headers = [h(0., 0.)];
        let n = stuck_count(40., 0., &headers, 24., None);
        assert_eq!(
            reveal_under_sticky(40., 800., 0., 200., 8., 32., n, 24.),
            24.
        );
    }

    #[test]
    fn does_not_scroll_a_focused_stuck_header() {
        let headers = [h(0., 0.)];
        let n = stuck_count(40., 0., &headers, 24., Some(0));
        assert_eq!(
            reveal_under_sticky(40., 800., 0., 200., 0., 24., n, 24.),
            40.
        );
    }
}
