//! `lib/review-window.ts`: which file cards mount in a long Files changed stack, and which
//! diff rows mount inside a long file.

use std::collections::{HashMap, HashSet};

/// How many full DiffFile bodies may be mounted at once.
pub const REVIEW_HYDRATE_MAX: usize = 8;
/// Extra files around the selected path, even if they are off-screen.
pub const REVIEW_HYDRATE_OVERSCAN: usize = 2;
/// Prefetch band above/below the scrollport, in px.
pub const REVIEW_VIEW_PAD_PX: f32 = 900.;

const HEAD_PX: f32 = 38.;
pub const DIFF_LINE_PX: f32 = 20.;
/// Extra rows above/below the viewport. Wrapping + comments make estimates drift.
pub const DIFF_LINE_OVERSCAN: usize = 60;
/// Skip line windowing when the file is this small.
pub const DIFF_LINE_WINDOW_MIN: usize = 80;
/// Guess when `lines` is missing (add/del stats only).
const STAT_LINE_CAP: i64 = 500;
const FILE_GAP_PX: f32 = 14.;

/// `estimateDiffHeight`. `lines` is `None` when the file has no line list.
pub fn estimate_diff_height(lines: Option<usize>, add: i64, del: i64, collapsed: bool) -> f32 {
    if collapsed {
        return HEAD_PX;
    }
    let n = lines.unwrap_or_else(|| (add + del + 4).min(STAT_LINE_CAP).max(0) as usize);
    HEAD_PX + n.max(1) as f32 * DIFF_LINE_PX + FILE_GAP_PX
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineWin {
    pub start: usize,
    pub end: usize,
}

impl LineWin {
    pub const fn new(start: usize, end: usize) -> Self {
        LineWin { start, end }
    }
}

/// Sum of slot heights in `[start, end)`. Missing (or zero) entries use `fallback`.
pub fn slot_pad(heights: Option<&[Option<f32>]>, start: usize, end: usize, fallback: f32) -> f32 {
    let hi = end.max(start);
    let Some(h) = heights else {
        return (hi - start) as f32 * fallback;
    };
    (start..hi)
        .map(|i| {
            h.get(i)
                .copied()
                .flatten()
                .filter(|v| *v != 0.)
                .unwrap_or(fallback)
        })
        .sum()
}

fn slot_h(heights: Option<&[Option<f32>]>, i: usize, line_px: f32) -> f32 {
    heights
        .and_then(|h| h.get(i).copied().flatten())
        .filter(|v| *v != 0.)
        .unwrap_or(line_px)
}

/// `readSlotHeights`: each slot is a row plus the blocks after it (threads, findings,
/// composer). `nodes` are `(is_row, height)` in document order from slot `start`. Returns
/// whether `into` changed.
pub fn read_slot_heights(nodes: &[(bool, f32)], start: usize, into: &mut Vec<Option<f32>>) -> bool {
    let mut changed = false;
    let mut slot = start;
    let mut i = 0;
    while i < nodes.len() {
        let mut h = nodes[i].1;
        i += 1;
        while i < nodes.len() && !nodes[i].0 {
            h += nodes[i].1;
            i += 1;
        }
        if into.len() <= slot {
            into.resize(slot + 1, None);
        }
        if h > 0. && into[slot] != Some(h) {
            into[slot] = Some(h);
            changed = true;
        }
        slot += 1;
    }
    changed
}

/// Slice of rows overlapping the scrollport (+ overscan). `heights` is sparse.
pub fn line_window(
    count: usize,
    body_top: f32,
    scroll_top: f32,
    view_h: f32,
    line_px: f32,
    overscan: usize,
    heights: Option<&[Option<f32>]>,
) -> LineWin {
    if count == 0 {
        return LineWin::new(0, 0);
    }
    if count <= DIFF_LINE_WINDOW_MIN {
        return LineWin::new(0, count);
    }
    let pad = overscan as f32 * line_px;
    let a = scroll_top - pad;
    let b = scroll_top + view_h + pad;
    let body_bot = body_top + slot_pad(heights, 0, count, line_px);
    if body_bot <= a {
        return LineWin::new(count.saturating_sub(overscan), count);
    }
    if body_top >= b {
        return LineWin::new(0, count.min(overscan));
    }
    let mut y = body_top;
    let mut start = 0;
    for i in 0..count {
        let next = y + slot_h(heights, i, line_px);
        if next > a {
            start = i;
            break;
        }
        y = next;
    }
    y = body_top + slot_pad(heights, 0, start, line_px);
    let mut end = count;
    for i in start..count {
        y += slot_h(heights, i, line_px);
        if y >= b {
            end = i + 1;
            break;
        }
    }
    LineWin::new(start, end.max(start))
}

pub fn cover_index(win: LineWin, i: isize, count: usize) -> LineWin {
    if i < 0 || i as usize >= count {
        return win;
    }
    let i = i as usize;
    LineWin::new(win.start.min(i), win.end.max(i + 1))
}

/// Slice of `overscan` rows around `i`: jumps must not span the whole file.
pub fn window_around(i: isize, count: usize, overscan: usize) -> LineWin {
    if count == 0 {
        return LineWin::new(0, 0);
    }
    if i < 0 || i as usize >= count {
        return LineWin::new(0, count.min(overscan));
    }
    let i = i as usize;
    LineWin::new(i.saturating_sub(overscan), count.min(i + overscan + 1))
}

/// Viewport slice, unless a new highlight jump needs `slot` mounted.
pub fn jump_line_window(
    view: LineWin,
    count: usize,
    highlight_line: i64,
    last_jump: i64,
    slot: isize,
) -> LineWin {
    if highlight_line > 0 && last_jump != highlight_line && slot >= 0 {
        return window_around(slot, count, DIFF_LINE_OVERSCAN);
    }
    view
}

/// Expand `win` to include `i` only when that row already sits near the slice.
pub fn cover_near(win: LineWin, i: isize, count: usize, max_gap: usize) -> LineWin {
    if i < 0 || i as usize >= count {
        return win;
    }
    let gap = max_gap as isize;
    if i + 1 + gap < win.start as isize || i - gap > win.end as isize {
        return win;
    }
    cover_index(win, i, count)
}

/// `applySlotPin`: the scrollTop that keeps the pinned row where it was after a slice change,
/// or `None` when nothing should move (no pin, the row unmounted, or the pane moved since
/// capture — a highlight jump or the user kept scrolling).
pub fn apply_slot_pin(
    scroll_top: f32,
    row_top: Option<f32>,
    pin_y: Option<f32>,
    captured_top: f32,
) -> Option<f32> {
    let (pin_y, row_top) = (pin_y?, row_top?);
    if scroll_top != captured_top {
        return None;
    }
    let drift = row_top - pin_y;
    (drift.abs() > 0.5).then_some(scroll_top + drift)
}

/// `reservedCardHeight`: min-height for a stacked card. Live cards size themselves; collapsed
/// stubs are header-only; expanded stubs keep the last measured body.
pub fn reserved_card_height(
    lines: Option<usize>,
    add: i64,
    del: i64,
    live: bool,
    collapsed: bool,
    measured: Option<f32>,
) -> Option<f32> {
    if live {
        return None;
    }
    if collapsed {
        return Some(HEAD_PX);
    }
    match measured {
        Some(m) if m > 0. => Some(m),
        _ => Some(estimate_diff_height(lines, add, del, false)),
    }
}

/// `pickHydrated`: the selected file and its neighbors, then the scrollport (visible before the
/// prefetch pad). Not distance-sorted, or a stale spy drops the viewport.
pub fn pick_hydrated(
    paths: &[String],
    selected: &str,
    in_view: &[String],
    max: usize,
    overscan: usize,
) -> HashSet<String> {
    let mut out = HashSet::new();
    if paths.is_empty() || max == 0 {
        return out;
    }
    let idx: HashMap<&str, usize> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), i))
        .collect();
    let add = |p: Option<&String>, out: &mut HashSet<String>| {
        if let Some(p) = p
            && out.len() < max
            && idx.contains_key(p.as_str())
        {
            out.insert(p.clone());
        }
    };
    let sel = idx.get(selected).copied().unwrap_or(0);
    for d in 0..=overscan {
        add(sel.checked_sub(d).and_then(|i| paths.get(i)), &mut out);
        add(paths.get(sel + d), &mut out);
    }
    for p in in_view {
        add(Some(p), &mut out);
    }
    if out.is_empty() {
        out.insert(paths[0].clone());
    }
    out
}

/// `recordStubHeight`'s rule: a live, expanded card's height is kept for its stub. Returns
/// whether the cached height changed.
pub fn record_stub_height(
    heights: &mut HashMap<String, f32>,
    path: &str,
    stub: bool,
    collapsed: bool,
    height: f32,
) -> bool {
    if stub || collapsed || height <= 0. {
        return false;
    }
    heights.insert(path.to_string(), height) != Some(height)
}

/// A stacked card's header is pinned once the card top has scrolled past the scrollport.
pub fn header_is_stuck(card_top: f32, card_bottom: f32, scroll_top: f32) -> bool {
    card_top < scroll_top && card_bottom > scroll_top
}

/// One stacked card's vertical extent in scroll-content coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct CardRange {
    pub path: String,
    pub top: f32,
    pub bottom: f32,
}

/// `inViewPaths`: visible cards first, then the prefetch pad, both in document order.
pub fn in_view_paths(ranges: &[CardRange], scroll_top: f32, view_h: f32, pad: f32) -> Vec<String> {
    let (vis_a, vis_b) = (scroll_top, scroll_top + view_h);
    let (a, b) = (scroll_top - pad, scroll_top + view_h + pad);
    let mut visible = Vec::new();
    let mut prefetch = Vec::new();
    for r in ranges {
        if r.bottom > vis_a && r.top < vis_b {
            visible.push(r.path.clone());
        } else if r.bottom > a && r.top < b {
            prefetch.push(r.path.clone());
        }
    }
    visible.extend(prefetch);
    visible
}

pub fn same_path_list(a: &[String], b: &[String]) -> bool {
    a == b
}

pub fn same_path_set(a: &HashSet<String>, b: &HashSet<String>) -> bool {
    a == b
}

#[cfg(test)]
#[path = "review_window_tests.rs"]
mod tests;
