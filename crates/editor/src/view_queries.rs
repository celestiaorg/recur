//! What commands ask of the view (CodeMirror `EditorView` methods used by
//! `@codemirror/commands`, folding and vim): visual line blocks, vertical
//! motion, line boundaries and page height. The GPUI view answers from its
//! layout; tests use [`MonoView`], a fixed-width layout without wrapping.

use crate::selection::SelectionRange;
use crate::state::{CharCategory, EditorState};
use crate::text::find_cluster_break;

/// A visual line block: one document line, or several merged by folds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineBlock {
    pub from: usize,
    pub to: usize,
    pub top: f32,
    pub height: f32,
}

impl LineBlock {
    pub fn bottom(&self) -> f32 {
        self.top + self.height
    }

    pub fn len(&self) -> usize {
        self.to - self.from
    }

    pub fn is_empty(&self) -> bool {
        self.to == self.from
    }
}

/// `moveByChar`'s `by`: given the first character, a predicate for the
/// characters that continue the motion.
pub type ByFn<'a> = dyn FnMut(&str) -> CheckFn<'a> + 'a;
pub type CheckFn<'a> = Box<dyn FnMut(&str) -> bool + 'a>;

pub trait ViewQueries {
    fn state(&self) -> &EditorState;
    fn line_block_at(&self, pos: usize) -> LineBlock;
    /// `moveVertically`; `distance` is in pixels (default: one line).
    fn move_vertically(
        &self,
        start: SelectionRange,
        forward: bool,
        distance: Option<f32>,
    ) -> SelectionRange;
    /// `moveToLineBoundary`: the wrap point or line end/start.
    fn move_to_line_boundary(
        &self,
        start: SelectionRange,
        forward: bool,
        include_wrap: bool,
    ) -> SelectionRange;
    /// Height of a page for page up/down (`pageInfo(view).height`).
    fn page_height(&self) -> f32;
    fn default_line_height(&self) -> f32;
    fn line_wrapping(&self) -> bool;
    /// Vertical pixel extent of `pos`'s caret, relative to the document top.
    fn coords_at_pos(&self, pos: usize, side: i32) -> Option<(f32, f32)>;
    /// Whether the scroller scrolls itself (`pageInfo.selfScroll`) and the
    /// visible content box `(top, bottom)` relative to the document top.
    fn scroll_box(&self) -> Option<(f32, f32)>;
    /// Positions covered by atomic ranges (cursor motion skips them).
    fn atomic_ranges(&self) -> Vec<(usize, usize)> {
        Vec::new()
    }

    /// `moveByChar`, skipping atomic ranges.
    fn move_by_char(
        &self,
        start: SelectionRange,
        forward: bool,
        by: Option<&mut ByFn<'_>>,
    ) -> SelectionRange {
        let moved = move_by_char(self.state(), start, forward, by);
        skip_atoms(&self.atomic_ranges(), start, moved)
    }

    /// `moveByGroup`.
    fn move_by_group(&self, start: SelectionRange, forward: bool) -> SelectionRange {
        let state = self.state();
        let mut by = |first: &str| -> Box<dyn FnMut(&str) -> bool + '_> {
            let mut cat = state.categorize(first);
            Box::new(move |next: &str| {
                let next_cat = state.categorize(next);
                if cat == CharCategory::Space {
                    cat = next_cat;
                }
                cat == next_cat
            })
        };
        let moved = move_by_char(state, start, forward, Some(&mut by));
        skip_atoms(&self.atomic_ranges(), start, moved)
    }
}

/// One step of logical (LTR) visual motion within a line: the next cluster
/// boundary, or `None` at the line edge.
fn move_in_line(line_text: &str, pos_in_line: usize, forward: bool) -> Option<(usize, String)> {
    if forward {
        if pos_in_line >= line_text.len() {
            return None;
        }
        let next = find_cluster_break(line_text, pos_in_line, true);
        Some((next, line_text[pos_in_line..next].to_string()))
    } else {
        if pos_in_line == 0 {
            return None;
        }
        let prev = find_cluster_break(line_text, pos_in_line, false);
        Some((prev, line_text[prev..pos_in_line].to_string()))
    }
}

/// CodeMirror's `moveByChar` for left-to-right text.
pub fn move_by_char(
    state: &EditorState,
    start: SelectionRange,
    forward: bool,
    mut by: Option<&mut ByFn<'_>>,
) -> SelectionRange {
    let mut line = state.doc.line_at(start.head());
    let mut text = state.doc.slice(line.from, line.to);
    let mut cur = start;
    let mut check: Option<CheckFn<'_>> = None;
    loop {
        let (next, ch) = match move_in_line(&text, cur.head() - line.from, forward) {
            Some((p, ch)) => (
                SelectionRange::cursor_assoc(line.from + p, if forward { -1 } else { 1 }, None),
                ch,
            ),
            None => {
                if line.number == if forward { state.doc.lines() } else { 1 } {
                    return cur;
                }
                line = state.doc.line(if forward {
                    line.number + 1
                } else {
                    line.number - 1
                });
                text = state.doc.slice(line.from, line.to);
                let pos = if forward { line.from } else { line.to };
                (
                    SelectionRange::cursor_assoc(pos, if forward { 1 } else { -1 }, None),
                    "\n".to_string(),
                )
            }
        };
        match check.as_mut() {
            None => {
                let Some(by) = by.as_mut() else { return next };
                check = Some(by(&ch));
            }
            Some(c) => {
                if !c(&ch) {
                    return cur;
                }
            }
        }
        cur = next;
    }
}

/// `skipAtomicRanges`.
pub fn skip_atomic_ranges(atoms: &[(usize, usize)], mut pos: usize, bias: i32) -> usize {
    loop {
        let mut moved = 0;
        for &(from, to) in atoms {
            if pos > from && pos < to {
                let side = if moved != 0 {
                    moved
                } else if bias != 0 {
                    bias
                } else if pos - from < to - pos {
                    -1
                } else {
                    1
                };
                pos = if side < 0 { from } else { to };
                moved = side;
            }
        }
        if moved == 0 {
            return pos;
        }
    }
}

/// `skipAtoms(view, oldPos, pos)`.
pub fn skip_atoms(
    atoms: &[(usize, usize)],
    old: SelectionRange,
    pos: SelectionRange,
) -> SelectionRange {
    let new_pos = skip_atomic_ranges(atoms, pos.from, if old.head() > pos.from { -1 } else { 1 });
    if new_pos == pos.from {
        pos
    } else {
        SelectionRange::cursor_assoc(new_pos, if new_pos < pos.from { 1 } else { -1 }, None)
    }
}

/// A fixed-width, non-wrapping layout for tests: every character is one
/// column of `char_width`, every line `line_height` tall, folded regions
/// merge their lines into one block.
pub struct MonoView {
    pub state: EditorState,
    pub char_width: f32,
    pub line_height: f32,
    pub viewport_lines: usize,
}

impl MonoView {
    pub fn new(state: EditorState) -> MonoView {
        MonoView {
            state,
            char_width: 7.0,
            line_height: 18.0,
            viewport_lines: 20,
        }
    }

    /// Visual blocks `(from, to)` in document order.
    fn blocks(&self) -> Vec<(usize, usize)> {
        let doc = &self.state.doc;
        let mut out = Vec::new();
        let mut pos = 0;
        loop {
            let line = doc.line_at(pos);
            let (from, mut to) = (line.from, line.to);
            while let Some(t) = self
                .state
                .folds
                .ranges()
                .iter()
                .filter(|(f, t)| *f >= from && *f <= to && *t > to)
                .map(|(_, t)| *t)
                .max()
            {
                to = doc.line_at(t).to;
            }
            out.push((from, to));
            if to >= doc.len() {
                return out;
            }
            pos = to + 1;
        }
    }

    fn block_index(&self, pos: usize) -> usize {
        self.blocks()
            .iter()
            .position(|&(f, t)| pos >= f && pos <= t)
            .unwrap_or(0)
    }

    fn col_x(&self, pos: usize) -> f32 {
        let line = self.state.doc.line_at(pos);
        let text = self.state.doc.slice(line.from, pos);
        crate::text::count_column(&text, self.state.tab_size(), text.len()) as f32 * self.char_width
    }

    fn pos_at_x(&self, block: LineBlock, x: f32) -> usize {
        let line = self.state.doc.line_at(block.from);
        let text = self.state.doc.slice(line.from, line.to);
        let col = (x / self.char_width).round().max(0.0) as usize;
        line.from
            + crate::text::find_column(&text, col, self.state.tab_size(), false)
                .unwrap_or(text.len())
    }

    fn block_by_index(&self, index: usize) -> Option<LineBlock> {
        self.blocks().get(index).map(|&(from, to)| LineBlock {
            from,
            to,
            top: index as f32 * self.line_height,
            height: self.line_height,
        })
    }
}

impl ViewQueries for MonoView {
    fn state(&self) -> &EditorState {
        &self.state
    }

    fn line_block_at(&self, pos: usize) -> LineBlock {
        let index = self.block_index(pos);
        self.block_by_index(index).unwrap_or(LineBlock {
            from: 0,
            to: 0,
            top: 0.0,
            height: self.line_height,
        })
    }

    fn move_vertically(
        &self,
        start: SelectionRange,
        forward: bool,
        distance: Option<f32>,
    ) -> SelectionRange {
        let head = start.head();
        if head == if forward { self.state.doc.len() } else { 0 } {
            return SelectionRange::cursor_assoc(head, start.assoc(), None);
        }
        let goal = start.goal_column.unwrap_or_else(|| self.col_x(head));
        let index = self.block_index(head);
        let lines = (distance.unwrap_or(self.line_height / 2.0) / self.line_height)
            .ceil()
            .max(1.0) as usize;
        let target = if forward {
            index + lines
        } else {
            index.saturating_sub(lines)
        };
        match self.block_by_index(target) {
            Some(b) => SelectionRange::cursor_assoc(self.pos_at_x(b, goal), 0, Some(goal)),
            None => SelectionRange::cursor_assoc(self.state.doc.len(), -1, None),
        }
    }

    fn move_to_line_boundary(
        &self,
        start: SelectionRange,
        forward: bool,
        _include_wrap: bool,
    ) -> SelectionRange {
        let block = self.line_block_at(start.head());
        SelectionRange::cursor_assoc(
            if forward { block.to } else { block.from },
            if forward { -1 } else { 1 },
            None,
        )
    }

    fn page_height(&self) -> f32 {
        (self.viewport_lines as f32 * self.line_height - 5.0).max(self.line_height)
    }

    fn default_line_height(&self) -> f32 {
        self.line_height
    }

    fn line_wrapping(&self) -> bool {
        false
    }

    fn coords_at_pos(&self, pos: usize, _side: i32) -> Option<(f32, f32)> {
        let b = self.line_block_at(pos);
        Some((b.top, b.bottom()))
    }

    fn scroll_box(&self) -> Option<(f32, f32)> {
        None
    }
}
