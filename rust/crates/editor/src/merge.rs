//! The Diff view's unified merge state: `@codemirror/merge`'s
//! `unifiedMergeView` as `MergeEditor.svelte` configures it (line-oriented
//! diff, word highlights, change gutter, no accept/reject controls,
//! unchanged regions collapsed with margin 3 / min 6 lines), plus
//! `cm-merge-full-line-marks.ts` and the revert chip of
//! `cm-merge-revert.ts`.
//!
//! Deleted text is shown without syntax colors, as in Recur: the merge
//! package only highlights deletions with a Lezer language, which Recur's
//! TextMate editor does not have.

use std::rc::Rc;

use crate::change::{Change, ChangeDesc, ChangeSet};
use crate::diff::{
    DiffChange, changes_to_bytes, diff_is_precise, merge_presentable_diff_units, units,
};
use crate::keyequiv::format_key_equivalent_display;
use crate::state::{EditorState, Effect, Transaction, TransactionSpec};
use crate::text::Text;

/// A changed region, whole lines on both sides (`Chunk`). `changes` are
/// relative to `from_a` / `from_b`; `to_*` may point one past the document
/// end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub changes: Rc<[DiffChange]>,
    pub from_a: usize,
    pub to_a: usize,
    pub from_b: usize,
    pub to_b: usize,
    pub precise: bool,
}

impl Chunk {
    pub fn end_a(&self) -> usize {
        self.from_a.max(self.to_a.saturating_sub(1))
    }

    pub fn end_b(&self) -> usize {
        self.from_b.max(self.to_b.saturating_sub(1))
    }

    fn offset(&self, off_a: isize, off_b: isize) -> Chunk {
        if off_a == 0 && off_b == 0 {
            return self.clone();
        }
        Chunk {
            changes: self.changes.clone(),
            from_a: self.from_a.wrapping_add_signed(off_a),
            to_a: self.to_a.wrapping_add_signed(off_a),
            from_b: self.from_b.wrapping_add_signed(off_b),
            to_b: self.to_b.wrapping_add_signed(off_b),
            precise: self.precise,
        }
    }
}

/// Diff `a[from_a..to_a]` against `b[from_b..to_b]` with Recur's merge diff;
/// changes in absolute byte offsets.
fn presentable(
    a: &Text,
    from_a: usize,
    to_a: usize,
    b: &Text,
    from_b: usize,
    to_b: usize,
) -> (Vec<DiffChange>, bool) {
    let sa = units(&a.slice(from_a, to_a));
    let sb = units(&b.slice(from_b, to_b));
    let changes = merge_presentable_diff_units(&sa, &sb);
    let precise = diff_is_precise();
    (
        changes_to_bytes(
            &changes,
            a,
            a.byte_to_utf16(from_a),
            b,
            b.byte_to_utf16(from_b),
        ),
        precise,
    )
}

fn from_line(from_a: usize, from_b: usize, a: &Text, b: &Text) -> (usize, usize) {
    let (la, lb) = (a.line_at(from_a), b.line_at(from_b));
    if la.to == from_a && lb.to == from_b && from_a < a.len() && from_b < b.len() {
        (from_a + 1, from_b + 1)
    } else {
        (la.from, lb.from)
    }
}

fn to_line(to_a: usize, to_b: usize, a: &Text, b: &Text) -> (usize, usize) {
    let (la, lb) = (a.line_at(to_a), b.line_at(to_b));
    if la.from == to_a && lb.from == to_b {
        (to_a, to_b)
    } else {
        (la.to + 1, lb.to + 1)
    }
}

/// `toChunks` for absolute changes.
fn to_chunks(changes: &[DiffChange], a: &Text, b: &Text, precise: bool) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < changes.len() {
        let change = changes[i];
        let (from_a, from_b) = from_line(change.from_a, change.from_b, a, b);
        let (mut to_a, mut to_b) = to_line(change.to_a, change.to_b, a, b);
        let rel = |c: DiffChange| {
            DiffChange::new(
                c.from_a - from_a,
                c.to_a - from_a,
                c.from_b - from_b,
                c.to_b - from_b,
            )
        };
        let mut chunk = vec![rel(change)];
        while i + 1 < changes.len() {
            let next = changes[i + 1];
            let (next_a, next_b) = from_line(next.from_a, next.from_b, a, b);
            if next_a > to_a + 1 && next_b > to_b + 1 {
                break;
            }
            chunk.push(rel(next));
            (to_a, to_b) = to_line(next.to_a, next.to_b, a, b);
            i += 1;
        }
        chunks.push(Chunk {
            changes: chunk.into(),
            from_a,
            to_a: from_a.max(to_a),
            from_b,
            to_b: from_b.max(to_b),
            precise,
        });
        i += 1;
    }
    chunks
}

/// `Chunk.build`.
pub fn build_chunks(a: &Text, b: &Text) -> Vec<Chunk> {
    let (changes, precise) = presentable(a, 0, a.len(), b, 0, b.len());
    to_chunks(&changes, a, b, precise)
}

const UPDATE_MARGIN: usize = 1000;

fn find_pos(chunks: &[Chunk], pos: usize, is_a: bool, start: bool) -> (usize, usize) {
    let (mut lo, mut hi) = (0, chunks.len());
    loop {
        if lo == hi {
            let (ref_a, ref_b) = if lo > 0 {
                (chunks[lo - 1].to_a, chunks[lo - 1].to_b)
            } else {
                (0, 0)
            };
            let off = pos as isize - if is_a { ref_a } else { ref_b } as isize;
            return (
                ref_a.wrapping_add_signed(off),
                ref_b.wrapping_add_signed(off),
            );
        }
        let mid = (lo + hi) >> 1;
        let c = &chunks[mid];
        let (from, to) = if is_a {
            (c.from_a, c.to_a)
        } else {
            (c.from_b, c.to_b)
        };
        if from > pos {
            hi = mid;
        } else if to <= pos {
            lo = mid + 1;
        } else {
            return if start {
                (c.from_a, c.from_b)
            } else {
                (c.to_a, c.to_b)
            };
        }
    }
}

struct UpdateRange {
    from_a: usize,
    to_a: usize,
    from_b: usize,
    to_b: usize,
    diff_a: isize,
    diff_b: isize,
}

fn find_ranges_for_change(
    chunks: &[Chunk],
    changes: &ChangeSet,
    is_a: bool,
    other_len: usize,
) -> Vec<UpdateRange> {
    let mut ranges: Vec<UpdateRange> = Vec::new();
    let len = changes.len();
    changes.iter_changed_ranges(false, |c_from_a, c_to_a, c_from_b, c_to_b| {
        let (mut from_a, mut to_a) = (0, if is_a { len } else { other_len });
        let (mut from_b, mut to_b) = (0, if is_a { other_len } else { len });
        if c_from_a > UPDATE_MARGIN {
            (from_a, from_b) = find_pos(chunks, c_from_a - UPDATE_MARGIN, is_a, true);
        }
        if c_to_a + UPDATE_MARGIN < len {
            (to_a, to_b) = find_pos(chunks, c_to_a + UPDATE_MARGIN, is_a, false);
        }
        let len_diff = (c_to_b - c_from_b) as isize - (c_to_a - c_from_a) as isize;
        let (diff_a, diff_b) = if is_a { (len_diff, 0) } else { (0, len_diff) };
        match ranges.last_mut() {
            Some(last) if last.to_a >= from_a => {
                last.to_a = to_a;
                last.to_b = to_b;
                last.diff_a += diff_a;
                last.diff_b += diff_b;
            }
            _ => ranges.push(UpdateRange {
                from_a,
                to_a,
                from_b,
                to_b,
                diff_a,
                diff_b,
            }),
        }
    });
    ranges
}

fn update_chunks(ranges: &[UpdateRange], chunks: &[Chunk], a: &Text, b: &Text) -> Vec<Chunk> {
    if ranges.is_empty() {
        return chunks.to_vec();
    }
    let mut result = Vec::new();
    let (mut off_a, mut off_b) = (0isize, 0isize);
    let mut chunk_i = 0;
    let mut i = 0;
    loop {
        let range = ranges.get(i);
        let from_a = range.map_or(a.len(), |r| r.from_a.wrapping_add_signed(off_a));
        let from_b = range.map_or(b.len(), |r| r.from_b.wrapping_add_signed(off_b));
        while chunk_i < chunks.len() {
            let next = &chunks[chunk_i];
            if range.is_some()
                && (next.to_a.wrapping_add_signed(off_a) > from_a
                    || next.to_b.wrapping_add_signed(off_b) > from_b)
            {
                break;
            }
            result.push(next.offset(off_a, off_b));
            chunk_i += 1;
        }
        let Some(range) = range else { break };
        let to_a = range
            .to_a
            .wrapping_add_signed(off_a + range.diff_a)
            .min(a.len());
        let to_b = range
            .to_b
            .wrapping_add_signed(off_b + range.diff_b)
            .min(b.len());
        let (changes, precise) = presentable(a, from_a, to_a, b, from_b, to_b);
        result.extend(to_chunks(&changes, a, b, precise));
        off_a += range.diff_a;
        off_b += range.diff_b;
        while chunk_i < chunks.len() {
            let next = &chunks[chunk_i];
            if next.from_a.wrapping_add_signed(off_a) > to_a
                && next.from_b.wrapping_add_signed(off_b) > to_b
            {
                break;
            }
            chunk_i += 1;
        }
        i += 1;
    }
    result
}

/// `collapseUnchanged({ margin: 3, minSize: 6 })`.
pub const COLLAPSE_MARGIN: usize = 3;
pub const COLLAPSE_MIN_SIZE: usize = 6;

/// A collapsed run of unchanged lines (`cm-collapsedLines`, "N unchanged
/// lines"); clicking it expands it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Collapsed {
    pub from: usize,
    pub to: usize,
    pub lines: usize,
}

fn build_collapsed(doc: &Text, chunks: &[Chunk]) -> Vec<Collapsed> {
    let mut out = Vec::new();
    let mut prev_line = 1;
    let mut i = 0;
    loop {
        let chunk = chunks.get(i);
        let collapse_from = if i > 0 {
            prev_line + COLLAPSE_MARGIN
        } else {
            1
        };
        let collapse_to = match chunk {
            Some(c) => doc.line_at(c.from_b).number as isize - 1 - COLLAPSE_MARGIN as isize,
            None => doc.lines() as isize,
        };
        let lines = collapse_to - collapse_from as isize + 1;
        if lines >= COLLAPSE_MIN_SIZE as isize {
            out.push(Collapsed {
                from: doc.line(collapse_from).from,
                to: doc.line(collapse_to as usize).to,
                lines: lines as usize,
            });
        }
        let Some(c) = chunk else { break };
        prev_line = doc.line_at(doc.len().min(c.to_b)).number;
        i += 1;
    }
    out
}

#[derive(Clone, Debug)]
pub enum MergeEffect {
    /// `uncollapseUnchanged`: expand the collapsed region starting here.
    Uncollapse(usize),
}

impl MergeEffect {
    pub(crate) fn map(&self, changes: &ChangeDesc) -> MergeEffect {
        match self {
            MergeEffect::Uncollapse(p) => MergeEffect::Uncollapse(changes.map_pos(*p, -1)),
        }
    }
}

/// The unified merge state: the original (base) document, the chunks and
/// the collapsed unchanged regions.
#[derive(Clone, Debug)]
pub struct MergeState {
    pub original: Text,
    pub chunks: Rc<[Chunk]>,
    pub collapsed: Rc<[Collapsed]>,
}

impl MergeState {
    pub fn new(original: &str, doc: &Text) -> MergeState {
        let original = Text::of(original);
        let chunks = build_chunks(&original, doc);
        let collapsed = build_collapsed(doc, &chunks);
        MergeState {
            original,
            chunks: chunks.into(),
            collapsed: collapsed.into(),
        }
    }

    /// The chunk covering `pos` on the B side (`chunkAtPos`).
    pub fn chunk_at(&self, pos: usize) -> Option<&Chunk> {
        self.chunks
            .iter()
            .find(|c| c.from_b <= pos && pos <= c.end_b())
    }
}

pub(crate) fn update_merge_state(
    prev: Option<&Rc<MergeState>>,
    tr: &Transaction,
    doc: &Text,
) -> Option<Rc<MergeState>> {
    let prev = prev?;
    let uncollapse: Vec<usize> = tr
        .effects
        .iter()
        .filter_map(|e| match e {
            Effect::Merge(MergeEffect::Uncollapse(p)) => Some(*p),
            _ => None,
        })
        .collect();
    if !tr.doc_changed() && uncollapse.is_empty() {
        return Some(prev.clone());
    }
    let mut next = (**prev).clone();
    let changes = tr.changes.desc();
    let mut collapsed: Vec<Collapsed> = prev
        .collapsed
        .iter()
        .filter_map(|c| {
            let from = changes.map_pos(c.from, -1);
            let to = changes.map_pos(c.to, 1);
            (from <= to).then_some(Collapsed {
                from,
                to,
                lines: c.lines,
            })
        })
        .collect();
    collapsed.retain(|c| !uncollapse.contains(&c.from));
    if tr.doc_changed() {
        let ranges = find_ranges_for_change(&prev.chunks, &tr.changes, false, prev.original.len());
        next.chunks = update_chunks(&ranges, &prev.chunks, &prev.original, doc).into();
        if !collapsed.is_empty() {
            let touching: Vec<usize> = collapsed
                .iter()
                .filter(|c| {
                    next.chunks
                        .iter()
                        .any(|ch| c.to >= ch.from_b && c.from <= ch.to_b)
                })
                .map(|c| c.from)
                .collect();
            collapsed.retain(|c| !touching.contains(&c.from));
        }
    }
    next.collapsed = collapsed.into();
    Some(Rc::new(next))
}

/// `rejectChunk`: restore the chunk at `pos` (default: the cursor) to the
/// original text. `None` when there is no chunk there.
pub fn reject_chunk(state: &EditorState, pos: Option<usize>) -> Option<TransactionSpec> {
    let merge = state.merge.as_ref()?;
    let at = pos.unwrap_or_else(|| state.selection.main().head());
    let chunk = merge.chunk_at(at)?;
    let orig = &merge.original;
    let mut insert = orig.slice(
        chunk.from_a,
        chunk
            .from_a
            .max(chunk.to_a.saturating_sub(1))
            .min(orig.len()),
    );
    if chunk.from_a != chunk.to_a && chunk.to_b <= state.doc.len() {
        insert.push('\n');
    }
    Some(
        TransactionSpec::default()
            .changes(Change::new(
                chunk.from_b,
                state.doc.len().min(chunk.to_b),
                insert,
            ))
            .user_event("revert"),
    )
}

/// Line decorations for a chunk's B side: `cm-changedLine` + gutter mark
/// on each line start, `cm-changedText` word marks, and `cm-fullLineChange`
/// on lines the word marks cover entirely.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkDecorations {
    pub changed_lines: Vec<usize>,
    pub changed_text: Vec<(usize, usize)>,
    pub full_line: Vec<usize>,
}

/// `buildChunkDeco` (B side, highlightChanges) plus full-line detection.
pub fn chunk_decorations(doc: &Text, chunk: &Chunk) -> ChunkDecorations {
    let mut out = ChunkDecorations::default();
    let (from, to) = (chunk.from_b, chunk.to_b);
    if from == to {
        return out;
    }
    let mut change_i = 0;
    let mut pos = from;
    let last = to.saturating_sub(1).min(doc.len());
    loop {
        out.changed_lines.push(pos);
        let line = doc.line_at(pos);
        let line_end = line.to.min(last);
        while change_i < chunk.changes.len() {
            let c = &chunk.changes[change_i];
            let next_from = from + c.from_b;
            let next_to = from + c.to_b;
            let ch_from = pos.max(next_from);
            let ch_to = line_end.min(next_to);
            if ch_from < ch_to {
                out.changed_text.push((ch_from, ch_to));
            }
            if next_to < line_end {
                change_i += 1;
            } else {
                break;
            }
        }
        if line_fully_covered(chunk.from_b, &chunk.changes, line.from, line.to) {
            out.full_line.push(line.from);
        }
        if line.to >= last || line.to >= doc.len() {
            break;
        }
        pos = line.to + 1;
    }
    out
}

/// `lineFullyCoveredByChanges`.
pub fn line_fully_covered(
    chunk_from_b: usize,
    changes: &[DiffChange],
    line_from: usize,
    line_to: usize,
) -> bool {
    if line_to <= line_from {
        return false;
    }
    let mut intervals: Vec<(usize, usize)> = changes
        .iter()
        .filter_map(|c| {
            let a = (chunk_from_b + c.from_b).max(line_from);
            let b = (chunk_from_b + c.to_b).min(line_to);
            (a < b).then_some((a, b))
        })
        .collect();
    intervals.sort_unstable();
    let mut at = line_from;
    for (a, b) in intervals {
        if a > at {
            return false;
        }
        if b > at {
            at = b;
        }
        if at >= line_to {
            return true;
        }
    }
    at >= line_to
}

/// A run of deleted text in a deleted-chunk line; `changed` runs get
/// `cm-deletedText`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletedSpan {
    pub text: String,
    pub changed: bool,
}

/// The deleted-chunk widget's lines (`deletionWidget`): the original text
/// of the chunk, split on newlines, with changed runs marked. Empty when
/// nothing was deleted (the widget is an empty block then).
pub fn deleted_chunk_lines(original: &Text, chunk: &Chunk) -> Vec<Vec<DeletedSpan>> {
    if chunk.from_a >= chunk.to_a {
        return Vec::new();
    }
    let text = original.slice(chunk.from_a, chunk.end_a());
    let mut lines: Vec<Vec<DeletedSpan>> = vec![Vec::new()];
    let changes = &chunk.changes;
    let mut change_i = 0;
    let mut inside = false;
    let mut at = 0;
    let to = text.len();
    while at < to {
        if text.as_bytes()[at] == b'\n' {
            lines.push(Vec::new());
            at += 1;
            continue;
        }
        let mut next_stop = to;
        let mut flip = false;
        if let Some(nl) = text[at..].find('\n') {
            next_stop = at + nl;
        }
        if change_i < changes.len() {
            let bound = if inside {
                changes[change_i].to_a
            } else {
                changes[change_i].from_a
            };
            if bound <= next_stop {
                next_stop = bound;
                if inside {
                    change_i += 1;
                }
                flip = true;
            }
        }
        if next_stop > at {
            if let Some(line) = lines.last_mut() {
                line.push(DeletedSpan {
                    text: text[at..next_stop].to_string(),
                    changed: inside,
                });
            }
            at = next_stop;
        }
        if flip {
            inside = !inside;
        }
    }
    lines
}

/// Hide the revert chip this long after the pointer leaves a hunk.
pub const REVERT_HIDE_MS: u64 = 140;
/// Gap between the hunk's bottom and the chip.
pub const REVERT_OUTSIDE_GAP: f32 = 4.0;
/// Chip inset from the right edge.
pub const REVERT_RIGHT_INSET: f32 = 8.0;
/// Chip width before it has been measured.
pub const REVERT_BUTTON_FALLBACK_W: f32 = 88.0;
/// `revertHunkCommand`'s chord.
pub const REVERT_HUNK_CHORD: &str = "opt+cmd+z";

pub fn revert_hunk_key_display() -> String {
    format_key_equivalent_display(REVERT_HUNK_CHORD)
}

/// The chip's title and accessible label.
pub fn revert_hunk_title() -> String {
    format!(
        "Revert this hunk to the workspace base ({})",
        revert_hunk_key_display()
    )
}

/// `hunkButtonLeft`: the chip's content x, against the nearer of the
/// content's right edge and the visible scroller edge.
pub fn hunk_button_left(
    content_right: f32,
    scroll_left: f32,
    client_width: f32,
    width: f32,
) -> f32 {
    content_right.min(scroll_left + client_width) - width - REVERT_RIGHT_INSET
}

/// The revert chip's hover state (`mergeRevertHoverPlugin`): which hunk it
/// advertises, from the pointer if it is on one, else the caret.
#[derive(Clone, Debug, Default)]
pub struct RevertHover {
    /// The chunk the chip shows for (its `from_b`), when visible.
    pub from_b: Option<usize>,
    pub hover_from_b: Option<usize>,
    /// Deadline of the pending hide, if any.
    pub hide_at: Option<u64>,
    last_pointer: Option<(f32, f32)>,
}

/// What the view should do with the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipAction {
    Keep,
    /// Show (or move) the chip for the chunk starting at this B position.
    Place(usize),
    Hide,
}

impl RevertHover {
    /// `revertAimChunk`.
    pub fn target(&self, state: &EditorState) -> Option<usize> {
        let merge = state.merge.as_ref()?;
        if let Some(h) = self.hover_from_b
            && let Some(c) = merge.chunk_at(h)
        {
            return Some(c.from_b);
        }
        merge
            .chunk_at(state.selection.main().head())
            .map(|c| c.from_b)
    }

    /// `update(ViewUpdate)` after `tr`; `layout_changed` for viewport or
    /// geometry changes.
    pub fn update(&mut self, tr: &Transaction, layout_changed: bool) -> ChipAction {
        let state = tr.state();
        if state.read_only() {
            return self.force_hide();
        }
        if tr.doc_changed() {
            self.from_b = self.from_b.map(|p| tr.changes.map_pos(p, -1));
            self.hover_from_b = self.hover_from_b.map(|p| tr.changes.map_pos(p, -1));
        }
        if tr.selection.is_some() {
            self.hover_from_b = None;
        }
        if tr.doc_changed() || tr.selection.is_some() || layout_changed {
            return self.sync(state, layout_changed || tr.doc_changed());
        }
        ChipAction::Keep
    }

    /// Initial placement, and the resync after a hide delay.
    pub fn sync(&mut self, state: &EditorState, force_place: bool) -> ChipAction {
        let Some(target) = self.target(state) else {
            return self.force_hide();
        };
        let same = Some(target) == self.from_b;
        self.from_b = Some(target);
        if !same || force_place {
            ChipAction::Place(target)
        } else {
            ChipAction::Keep
        }
    }

    /// Pointer moved to `(x, y)` over chunk `chunk_from_b` (`None` off any
    /// hunk). Moves without travel (scroll/layout) are ignored.
    pub fn pointer(
        &mut self,
        state: &EditorState,
        x: f32,
        y: f32,
        chunk_from_b: Option<usize>,
        now: u64,
    ) -> ChipAction {
        if state.read_only() || self.last_pointer == Some((x, y)) {
            return ChipAction::Keep;
        }
        self.last_pointer = Some((x, y));
        let Some(ch) = chunk_from_b else {
            self.hover_from_b = None;
            self.hide_at = Some(now + REVERT_HIDE_MS);
            return ChipAction::Keep;
        };
        self.hide_at = None;
        self.hover_from_b = Some(ch);
        if Some(ch) == self.from_b {
            return ChipAction::Keep;
        }
        self.from_b = Some(ch);
        ChipAction::Place(ch)
    }

    /// Pointer left the scroller or the chip.
    pub fn leave(&mut self, now: u64) {
        self.hover_from_b = None;
        self.hide_at = Some(now + REVERT_HIDE_MS);
    }

    /// Pointer entered the chip.
    pub fn enter_chip(&mut self) {
        self.hide_at = None;
    }

    /// The hide timer fired.
    pub fn hide_due(&mut self, state: &EditorState, now: u64) -> ChipAction {
        if self.hide_at.is_none_or(|t| t > now) {
            return ChipAction::Keep;
        }
        self.hide_at = None;
        self.sync(state, false)
    }

    pub fn force_hide(&mut self) -> ChipAction {
        self.hide_at = None;
        self.from_b = None;
        ChipAction::Hide
    }

    /// Clicking the chip (or ⌥⌘Z): revert the advertised hunk.
    pub fn revert(&mut self, state: &EditorState) -> Option<TransactionSpec> {
        if state.read_only() {
            return None;
        }
        let spec = reject_chunk(state, self.from_b);
        self.hover_from_b = None;
        spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateConfig;

    fn merge_state(original: &str, doc: &str) -> EditorState {
        let mut s = EditorState::create(doc, StateConfig::default());
        s.merge = Some(Rc::new(MergeState::new(original, &s.doc)));
        s
    }

    #[test]
    fn chunks_cover_whole_lines() {
        let s = merge_state("a\nb\nc\n", "a\nB\nc\n");
        let m = s.merge.as_ref().unwrap();
        assert_eq!(m.chunks.len(), 1);
        let c = &m.chunks[0];
        assert_eq!((c.from_a, c.to_a, c.from_b, c.to_b), (2, 4, 2, 4));
        assert_eq!(&*c.changes, &[DiffChange::new(0, 1, 0, 1)]);
        let d = chunk_decorations(&s.doc, c);
        assert_eq!(d.changed_lines, vec![2]);
        assert_eq!(d.changed_text, vec![(2, 3)]);
        assert_eq!(d.full_line, vec![2]);
        assert_eq!(
            deleted_chunk_lines(&m.original, c),
            vec![vec![DeletedSpan {
                text: "b".into(),
                changed: true
            }]]
        );
    }

    #[test]
    fn pure_insertions_and_deletions() {
        let s = merge_state("a\nc\n", "a\nb\nc\n");
        let c = &s.merge.as_ref().unwrap().chunks[0];
        assert_eq!((c.from_a, c.to_a, c.from_b, c.to_b), (2, 2, 2, 4));
        assert!(deleted_chunk_lines(&s.merge.as_ref().unwrap().original, c).is_empty());
        let s = merge_state("a\nb\nc\n", "a\nc\n");
        let c = &s.merge.as_ref().unwrap().chunks[0];
        assert_eq!((c.from_a, c.to_a, c.from_b, c.to_b), (2, 4, 2, 2));
    }

    #[test]
    fn edits_update_chunks_and_reject_restores() {
        let s = merge_state("one\ntwo\nthree\n", "one\ntwo\nthree\n");
        assert!(s.merge.as_ref().unwrap().chunks.is_empty());
        let s = s
            .update(TransactionSpec::default().changes(Change::new(4, 7, "TWO")))
            .state()
            .clone();
        let m = s.merge.as_ref().unwrap();
        assert_eq!(m.chunks.len(), 1);
        assert_eq!((m.chunks[0].from_b, m.chunks[0].to_b), (4, 8));
        let s = s
            .update(TransactionSpec::default().cursor(5))
            .state()
            .clone();
        let s = s.update(reject_chunk(&s, None).unwrap()).state().clone();
        assert_eq!(s.doc.to_string(), "one\ntwo\nthree\n");
        assert!(s.merge.as_ref().unwrap().chunks.is_empty());
    }

    #[test]
    fn collapses_long_unchanged_runs() {
        let orig: String = (1..=20).map(|i| format!("l{i}\n")).collect();
        let doc = orig.replace("l10\n", "L10\n");
        let s = merge_state(&orig, &doc);
        let m = s.merge.as_ref().unwrap();
        let lines: Vec<usize> = m.collapsed.iter().map(|c| c.lines).collect();
        assert_eq!(lines, vec![6, 8]);
        let first = m.collapsed[0];
        assert_eq!(s.slice_doc(first.from, first.to), "l1\nl2\nl3\nl4\nl5\nl6");
        let s = s
            .update(
                TransactionSpec::default()
                    .effect(Effect::Merge(MergeEffect::Uncollapse(first.from))),
            )
            .state()
            .clone();
        assert_eq!(s.merge.as_ref().unwrap().collapsed.len(), 1);
    }

    #[test]
    fn revert_chip_follows_pointer_then_caret() {
        let s = merge_state("a\nb\nc\nd\ne\n", "a\nB\nc\nd\nE\n");
        let mut hover = RevertHover::default();
        assert_eq!(hover.sync(&s, true), ChipAction::Hide);
        assert_eq!(
            hover.pointer(&s, 1.0, 1.0, Some(8), 0),
            ChipAction::Place(8)
        );
        assert_eq!(hover.pointer(&s, 1.0, 1.0, None, 10), ChipAction::Keep);
        assert_eq!(hover.pointer(&s, 2.0, 2.0, None, 10), ChipAction::Keep);
        assert_eq!(hover.hide_due(&s, 200), ChipAction::Hide);
        let tr = s.update(TransactionSpec::default().cursor(2));
        assert_eq!(hover.update(&tr, false), ChipAction::Place(2));
        assert_eq!(revert_hunk_key_display(), "⌥⌘Z");
        assert_eq!(hunk_button_left(500.0, 0.0, 400.0, 88.0), 304.0);
    }
}
