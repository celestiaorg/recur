//! Editing commands: `@codemirror/commands` (cursor and selection motion,
//! deletion, line operations, newline/indent, comments) for left-to-right
//! text. A command returns the transaction to dispatch, or `None` when it
//! does not apply (CodeMirror's `false`).
//!
//! Recur has no syntax tree, so the syntax motions jump to the start of
//! the (empty) tree, i.e. position 0, `selectParentSyntax` never applies,
//! and `insertNewlineAndIndent` only explodes between adjacent brackets.

use std::collections::HashMap;

use recur_textmate::BlockComment;

use crate::brackets::match_brackets;
use crate::change::Change;
use crate::indent::{IndentContext, get_indentation};
use crate::selection::{EditorSelection, SelectionRange};
use crate::state::{
    CharCategory, EditorState, Effect, RangeChange, ScrollStrategy, ScrollTarget, TransactionSpec,
    is_js_space,
};
use crate::text::{count_column, find_cluster_break};
use crate::view_queries::{CheckFn, ViewQueries, skip_atomic_ranges};

pub type Command = fn(&dyn ViewQueries) -> Option<TransactionSpec>;

fn leading_space(s: &str) -> &str {
    &s[..s.len() - s.trim_start_matches(is_js_space).len()]
}

fn update_sel(
    sel: &EditorSelection,
    f: impl FnMut(SelectionRange) -> SelectionRange,
) -> EditorSelection {
    EditorSelection::create(sel.ranges.iter().copied().map(f).collect(), sel.main_index)
}

fn set_sel(selection: EditorSelection) -> TransactionSpec {
    TransactionSpec::default()
        .selection(selection)
        .scroll()
        .user_event("select")
}

fn move_sel(
    state: &EditorState,
    how: impl FnMut(SelectionRange) -> SelectionRange,
) -> Option<TransactionSpec> {
    let selection = update_sel(&state.selection, how);
    (!selection.eq_sel(&state.selection, true)).then(|| set_sel(selection))
}

fn range_end(range: SelectionRange, forward: bool) -> SelectionRange {
    SelectionRange::cursor(if forward { range.to } else { range.from })
}

fn cursor_by_char(v: &dyn ViewQueries, forward: bool) -> Option<TransactionSpec> {
    move_sel(v.state(), |r| {
        if r.is_empty() {
            v.move_by_char(r, forward, None)
        } else {
            range_end(r, forward)
        }
    })
}

pub fn cursor_char_left(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_by_char(v, false)
}

pub fn cursor_char_right(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_by_char(v, true)
}

fn by_char_logical(state: &EditorState, range: SelectionRange, forward: bool) -> SelectionRange {
    let mut pos = range.head();
    let line = state.doc.line_at(pos);
    if pos == if forward { line.to } else { line.from } {
        pos = if forward {
            (line.to + 1).min(state.doc.len())
        } else {
            line.from.saturating_sub(1)
        };
    } else {
        let text = state.doc.slice(line.from, line.to);
        pos = line.from + find_cluster_break(&text, pos - line.from, forward);
    }
    SelectionRange::cursor_assoc(pos, if forward { -1 } else { 1 }, None)
}

pub fn cursor_char_forward_logical(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    move_sel(state, |r| {
        if r.is_empty() {
            by_char_logical(state, r, true)
        } else {
            range_end(r, true)
        }
    })
}

pub fn cursor_char_backward_logical(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    move_sel(state, |r| {
        if r.is_empty() {
            by_char_logical(state, r, false)
        } else {
            range_end(r, false)
        }
    })
}

fn cursor_by_group(v: &dyn ViewQueries, forward: bool) -> Option<TransactionSpec> {
    move_sel(v.state(), |r| {
        if r.is_empty() {
            v.move_by_group(r, forward)
        } else {
            range_end(r, forward)
        }
    })
}

pub fn cursor_group_left(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_by_group(v, false)
}

pub fn cursor_group_right(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_by_group(v, true)
}

/// `toGroupStart`: Windows-style group motion predicate.
fn to_group_start<'a>(state: &'a EditorState, start: &str) -> CheckFn<'a> {
    let cat = state.categorize(start);
    let mut initial = cat != CharCategory::Space;
    Box::new(move |next: &str| {
        let next_cat = state.categorize(next);
        if next_cat != CharCategory::Space {
            return initial && next_cat == cat;
        }
        initial = false;
        true
    })
}

pub fn cursor_group_forward_win(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    move_sel(state, |r| {
        if r.is_empty() {
            let mut by = |s: &str| to_group_start(state, s);
            v.move_by_char(r, true, Some(&mut by))
        } else {
            range_end(r, true)
        }
    })
}

/// `moveBySyntax` over Recur's empty syntax tree.
fn move_by_syntax(forward: bool) -> SelectionRange {
    SelectionRange::cursor_assoc(0, if forward { -1 } else { 1 }, None)
}

pub fn cursor_syntax_left(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    move_sel(v.state(), |_| move_by_syntax(false))
}

pub fn cursor_syntax_right(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    move_sel(v.state(), |_| move_by_syntax(true))
}

fn cursor_by_line(v: &dyn ViewQueries, forward: bool) -> Option<TransactionSpec> {
    move_sel(v.state(), |r| {
        if !r.is_empty() {
            return range_end(r, forward);
        }
        let moved = v.move_vertically(r, forward, None);
        if moved.head() != r.head() {
            moved
        } else {
            v.move_to_line_boundary(r, forward, true)
        }
    })
}

pub fn cursor_line_up(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_by_line(v, false)
}

pub fn cursor_line_down(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_by_line(v, true)
}

fn cursor_by_page(v: &dyn ViewQueries, forward: bool) -> Option<TransactionSpec> {
    let state = v.state();
    let height = v.page_height();
    let selection = update_sel(&state.selection, |r| {
        if r.is_empty() {
            v.move_vertically(r, forward, Some(height))
        } else {
            range_end(r, forward)
        }
    });
    if selection.eq_sel(&state.selection, false) {
        return None;
    }
    let mut spec = set_sel(selection.clone());
    if let Some((scroll_top, scroll_bottom)) = v.scroll_box()
        && let Some((top, bottom)) = v.coords_at_pos(state.selection.main().head(), 1)
        && top > scroll_top
        && bottom < scroll_bottom
    {
        spec = spec.effect(Effect::ScrollIntoView(ScrollTarget::pos(
            selection.main().head(),
            ScrollStrategy::Start,
            top - scroll_top,
        )));
    }
    Some(spec)
}

pub fn cursor_page_up(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_by_page(v, false)
}

pub fn cursor_page_down(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_by_page(v, true)
}

fn move_by_line_boundary(
    v: &dyn ViewQueries,
    start: SelectionRange,
    forward: bool,
) -> SelectionRange {
    let line = v.line_block_at(start.head());
    let mut moved = v.move_to_line_boundary(start, forward, true);
    if moved.head() == start.head() && moved.head() != if forward { line.to } else { line.from } {
        moved = v.move_to_line_boundary(start, forward, false);
    }
    if !forward && moved.head() == line.from && !line.is_empty() {
        let text = v
            .state()
            .slice_doc(line.from, (line.from + 100).min(line.to));
        let space = leading_space(&text).len();
        if space > 0 && start.head() != line.from + space {
            moved = SelectionRange::cursor(line.from + space);
        }
    }
    moved
}

pub fn cursor_line_boundary_forward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    move_sel(v.state(), |r| move_by_line_boundary(v, r, true))
}

pub fn cursor_line_boundary_backward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    move_sel(v.state(), |r| move_by_line_boundary(v, r, false))
}

pub fn cursor_line_start(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    move_sel(v.state(), |r| {
        SelectionRange::cursor_assoc(v.line_block_at(r.head()).from, 1, None)
    })
}

pub fn cursor_line_end(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    move_sel(v.state(), |r| {
        SelectionRange::cursor_assoc(v.line_block_at(r.head()).to, -1, None)
    })
}

fn to_matching_bracket(state: &EditorState, extend: bool) -> Option<TransactionSpec> {
    let mut found = false;
    let selection = update_sel(&state.selection, |range| {
        let head = range.head();
        let matching = match_brackets(state, head, -1)
            .or_else(|| match_brackets(state, head, 1))
            .or_else(|| {
                (head > 0)
                    .then(|| match_brackets(state, head - 1, 1))
                    .flatten()
            })
            .or_else(|| {
                (head < state.doc.len())
                    .then(|| match_brackets(state, head + 1, -1))
                    .flatten()
            });
        let Some(m) = matching else { return range };
        let Some(end) = m.end else { return range };
        found = true;
        let to = if m.start.0 == head { end.1 } else { end.0 };
        if extend {
            SelectionRange::range(range.anchor(), to)
        } else {
            SelectionRange::cursor(to)
        }
    });
    found.then(|| set_sel(selection))
}

pub fn cursor_matching_bracket(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    to_matching_bracket(v.state(), false)
}

pub fn select_matching_bracket(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    to_matching_bracket(v.state(), true)
}

fn extend_sel(
    state: &EditorState,
    forward: bool,
    mut how: impl FnMut(SelectionRange) -> SelectionRange,
) -> Option<TransactionSpec> {
    let selection = update_sel(&state.selection, |mut range| {
        if range.is_undirectional() && (range.head() >= range.anchor()) != forward {
            range = SelectionRange::range(range.head(), range.anchor());
        }
        let head = how(range);
        SelectionRange::range_with(range.anchor(), head.head(), head.goal_column, head.assoc())
    });
    (!selection.eq_sel(&state.selection, false)).then(|| set_sel(selection))
}

pub fn select_char_left(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), false, |r| v.move_by_char(r, false, None))
}

pub fn select_char_right(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), true, |r| v.move_by_char(r, true, None))
}

pub fn select_char_forward_logical(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    extend_sel(state, true, |r| by_char_logical(state, r, true))
}

pub fn select_char_backward_logical(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    extend_sel(state, false, |r| by_char_logical(state, r, false))
}

pub fn select_group_left(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), false, |r| v.move_by_group(r, false))
}

pub fn select_group_right(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), true, |r| v.move_by_group(r, true))
}

pub fn select_group_forward_win(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    extend_sel(state, true, |r| {
        let mut by = |s: &str| to_group_start(state, s);
        v.move_by_char(r, true, Some(&mut by))
    })
}

pub fn select_syntax_left(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), false, |_| move_by_syntax(false))
}

pub fn select_syntax_right(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), true, |_| move_by_syntax(true))
}

pub fn select_line_up(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), false, |r| v.move_vertically(r, false, None))
}

pub fn select_line_down(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), true, |r| v.move_vertically(r, true, None))
}

pub fn select_page_up(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let h = v.page_height();
    extend_sel(v.state(), false, |r| v.move_vertically(r, false, Some(h)))
}

pub fn select_page_down(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let h = v.page_height();
    extend_sel(v.state(), true, |r| v.move_vertically(r, true, Some(h)))
}

/// `cursorLineBoundaryLeft` (left-to-right text: backward).
pub fn cursor_line_boundary_left(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_line_boundary_backward(v)
}

/// `cursorLineBoundaryRight`.
pub fn cursor_line_boundary_right(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    cursor_line_boundary_forward(v)
}

/// `selectLineBoundaryLeft`.
pub fn select_line_boundary_left(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    select_line_boundary_backward(v)
}

/// `selectLineBoundaryRight`.
pub fn select_line_boundary_right(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    select_line_boundary_forward(v)
}

pub fn select_line_boundary_forward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), true, |r| move_by_line_boundary(v, r, true))
}

pub fn select_line_boundary_backward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), false, |r| move_by_line_boundary(v, r, false))
}

pub fn select_line_start(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), false, |r| {
        SelectionRange::cursor(v.line_block_at(r.head()).from)
    })
}

pub fn select_line_end(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    extend_sel(v.state(), true, |r| {
        SelectionRange::cursor(v.line_block_at(r.head()).to)
    })
}

pub fn cursor_doc_start(_v: &dyn ViewQueries) -> Option<TransactionSpec> {
    Some(set_sel(EditorSelection::cursor(0)))
}

pub fn cursor_doc_end(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    Some(set_sel(EditorSelection::cursor(v.state().doc.len())))
}

pub fn select_doc_start(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    Some(set_sel(EditorSelection::single(
        v.state().selection.main().anchor(),
        0,
    )))
}

pub fn select_doc_end(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    Some(set_sel(EditorSelection::single(
        state.selection.main().anchor(),
        state.doc.len(),
    )))
}

pub fn select_all(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    Some(
        TransactionSpec::default()
            .selection(EditorSelection::single(0, v.state().doc.len()))
            .user_event("select"),
    )
}

/// A run of selected lines and the ranges in it (`selectedLineBlocks`).
struct LineBlockSel {
    from: usize,
    to: usize,
    ranges: Vec<SelectionRange>,
}

fn selected_line_blocks(state: &EditorState) -> Vec<LineBlockSel> {
    let mut blocks: Vec<LineBlockSel> = Vec::new();
    let mut upto = 0usize;
    for &range in &state.selection.ranges {
        let start_line = state.doc.line_at(range.from);
        let mut end_line = state.doc.line_at(range.to);
        if !range.is_empty() && range.to == end_line.from {
            end_line = state.doc.line_at(range.to - 1);
        }
        if upto >= start_line.number && !blocks.is_empty() {
            let prev = blocks.last_mut().expect("block");
            prev.to = end_line.to;
            prev.ranges.push(range);
        } else {
            blocks.push(LineBlockSel {
                from: start_line.from,
                to: end_line.to,
                ranges: vec![range],
            });
        }
        upto = end_line.number + 1;
    }
    blocks
}

pub fn select_line(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    let ranges = selected_line_blocks(state)
        .iter()
        .map(|b| SelectionRange::range(b.from, (b.to + 1).min(state.doc.len())))
        .collect();
    Some(
        TransactionSpec::default()
            .selection(EditorSelection::create(ranges, 0))
            .user_event("select"),
    )
}

/// `selectParentSyntax`: the empty tree has no parent to grow to.
pub fn select_parent_syntax(_v: &dyn ViewQueries) -> Option<TransactionSpec> {
    None
}

fn add_cursor_vertically(v: &dyn ViewQueries, forward: bool) -> Option<TransactionSpec> {
    let state = v.state();
    let mut ranges = state.selection.ranges.clone();
    for &range in &state.selection.ranges {
        let line = state.doc.line_at(range.head());
        if if forward {
            line.to < state.doc.len()
        } else {
            line.from > 0
        } {
            let mut cur = range;
            loop {
                let next = v.move_vertically(cur, forward, None);
                if next.head() < line.from || next.head() > line.to {
                    if !ranges.iter().any(|r| r.head() == next.head()) {
                        ranges.push(next);
                    }
                    break;
                } else if next.head() == cur.head() {
                    break;
                } else {
                    cur = next;
                }
            }
        }
    }
    if ranges.len() == state.selection.ranges.len() {
        return None;
    }
    let main = ranges.len() - 1;
    Some(set_sel(EditorSelection::create(ranges, main)))
}

pub fn add_cursor_above(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    add_cursor_vertically(v, false)
}

pub fn add_cursor_below(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    add_cursor_vertically(v, true)
}

pub fn simplify_selection(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let cur = &v.state().selection;
    let selection = if cur.ranges.len() > 1 {
        EditorSelection::create(vec![cur.main()], 0)
    } else if !cur.main().is_empty() {
        EditorSelection::cursor(cur.main().head())
    } else {
        return None;
    };
    Some(set_sel(selection))
}

fn skip_atomic(v: &dyn ViewQueries, pos: usize, forward: bool) -> usize {
    let mut pos = pos;
    for (from, to) in v.atomic_ranges() {
        if from < pos && to > pos {
            pos = if forward { to } else { from };
        }
    }
    pos
}

fn delete_by(
    v: &dyn ViewQueries,
    mut by: impl FnMut(SelectionRange) -> usize,
) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let mut event = "delete.selection";
    let changes = state.change_by_range(|range| {
        let (mut from, mut to) = (range.from, range.to);
        if from == to {
            let mut towards = by(range);
            if towards < from {
                event = "delete.backward";
                towards = skip_atomic(v, towards, false);
            } else if towards > from {
                event = "delete.forward";
                towards = skip_atomic(v, towards, true);
            }
            from = from.min(towards);
            to = to.max(towards);
        } else {
            from = skip_atomic(v, from, false);
            to = skip_atomic(v, to, true);
        }
        if from == to {
            RangeChange::range(range)
        } else {
            RangeChange::with(
                vec![Change::delete(from, to)],
                SelectionRange::cursor_assoc(from, if from < range.head() { -1 } else { 1 }, None),
            )
        }
    });
    if changes.changes.is_empty() {
        return None;
    }
    let mut spec = changes.spec().scroll().user_event(event);
    if event == "delete.selection" {
        spec = spec.effect(Effect::Announce("Selection deleted".into()));
    }
    Some(spec)
}

fn is_variation_selector(s: &str) -> bool {
    s.chars().any(|c| ('\u{FE00}'..='\u{FE0F}').contains(&c))
}

fn delete_by_char(
    v: &dyn ViewQueries,
    forward: bool,
    by_indent_unit: bool,
) -> Option<TransactionSpec> {
    let state = v.state();
    delete_by(v, |range| {
        let mut pos = range.from;
        let line = state.doc.line_at(pos);
        let text = state.doc.slice(line.from, line.to);
        let before = &text[..pos - line.from];
        if by_indent_unit
            && !forward
            && pos > line.from
            && pos < line.from + 200
            && before.bytes().all(|b| b == b' ' || b == b'\t')
        {
            if before.ends_with('\t') {
                return pos - 1;
            }
            let unit = state.indent_unit_width();
            let col = count_column(before, state.tab_size(), before.len());
            let drop = if unit == 0 || col.is_multiple_of(unit) {
                unit
            } else {
                col % unit
            };
            let bb = before.as_bytes();
            let mut i = 0;
            while i < drop && i < bb.len() && bb[bb.len() - 1 - i] == b' ' {
                pos -= 1;
                i += 1;
            }
            return pos;
        }
        let mut target = find_cluster_break(&text, pos - line.from, forward) + line.from;
        if target == pos && line.number != if forward { state.doc.lines() } else { 1 } {
            target = if forward { target + 1 } else { target - 1 };
        } else if !forward && is_variation_selector(&text[target - line.from..pos - line.from]) {
            target = find_cluster_break(&text, target - line.from, false) + line.from;
        }
        target
    })
}

/// `deleteCharBackward` (Backspace): also drops a whole indent unit in
/// leading whitespace.
pub fn delete_char_backward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    delete_by_char(v, false, true)
}

pub fn delete_char_backward_strict(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    delete_by_char(v, false, false)
}

pub fn delete_char_forward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    delete_by_char(v, true, false)
}

fn delete_by_group(v: &dyn ViewQueries, forward: bool) -> Option<TransactionSpec> {
    let state = v.state();
    delete_by(v, |range| {
        let mut pos = range.head();
        let line = state.doc.line_at(pos);
        let text = state.doc.slice(line.from, line.to);
        let mut cat: Option<CharCategory> = None;
        loop {
            if pos == if forward { line.to } else { line.from } {
                if pos == range.head() && line.number != if forward { state.doc.lines() } else { 1 }
                {
                    pos = if forward { pos + 1 } else { pos - 1 };
                }
                break;
            }
            let next = find_cluster_break(&text, pos - line.from, forward) + line.from;
            let next_char = &text[pos.min(next) - line.from..pos.max(next) - line.from];
            let next_cat = state.categorize(next_char);
            if cat.is_some_and(|c| c != next_cat) {
                break;
            }
            if next_char != " " || pos != range.head() {
                cat = Some(next_cat);
            }
            pos = next;
        }
        pos
    })
}

pub fn delete_group_backward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    delete_by_group(v, false)
}

pub fn delete_group_forward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    delete_by_group(v, true)
}

pub fn delete_group_forward_win(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    delete_by(v, |range| {
        let mut by = |s: &str| to_group_start(state, s);
        v.move_by_char(range, true, Some(&mut by)).head()
    })
}

pub fn delete_to_line_end(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let len = v.state().doc.len();
    delete_by(v, |range| {
        let end = v.line_block_at(range.head()).to;
        if range.head() < end {
            end
        } else {
            (range.head() + 1).min(len)
        }
    })
}

pub fn delete_to_line_start(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    delete_by(v, |range| {
        let start = v.line_block_at(range.head()).from;
        if range.head() > start {
            start
        } else {
            range.head().saturating_sub(1)
        }
    })
}

pub fn delete_line_boundary_backward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    delete_by(v, |range| {
        let start = v.move_to_line_boundary(range, false, true).head();
        if range.head() > start {
            start
        } else {
            range.head().saturating_sub(1)
        }
    })
}

pub fn delete_line_boundary_forward(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let len = v.state().doc.len();
    delete_by(v, |range| {
        let end = v.move_to_line_boundary(range, true, true).head();
        if range.head() < end {
            end
        } else {
            (range.head() + 1).min(len)
        }
    })
}

pub fn delete_trailing_whitespace(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let mut changes = Vec::new();
    for n in 1..=state.doc.lines() {
        let line = state.doc.line(n);
        let text = state.doc.slice(line.from, line.to);
        let trimmed = text.trim_end_matches(is_js_space).len();
        if trimmed < text.len() {
            changes.push(Change::delete(line.from + trimmed, line.to));
        }
    }
    (!changes.is_empty()).then(|| {
        TransactionSpec::default()
            .changes(changes)
            .user_event("delete")
    })
}

pub fn split_line(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let changes = state.change_by_range(|r| {
        RangeChange::with(
            vec![Change::new(r.from, r.to, "\n")],
            SelectionRange::cursor(r.from),
        )
    });
    Some(changes.spec().scroll().user_event("input"))
}

pub fn transpose_chars(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let changes = state.change_by_range(|range| {
        if !range.is_empty() || range.from == 0 || range.from == state.doc.len() {
            return RangeChange::range(range);
        }
        let pos = range.from;
        let line = state.doc.line_at(pos);
        let text = state.doc.slice(line.from, line.to);
        let from = if pos == line.from {
            pos - 1
        } else {
            find_cluster_break(&text, pos - line.from, false) + line.from
        };
        let to = if pos == line.to {
            pos + 1
        } else {
            find_cluster_break(&text, pos - line.from, true) + line.from
        };
        let insert = format!("{}{}", state.slice_doc(pos, to), state.slice_doc(from, pos));
        RangeChange::with(
            vec![Change::new(from, to, insert)],
            SelectionRange::cursor(to),
        )
    });
    if changes.changes.is_empty() {
        return None;
    }
    Some(changes.spec().scroll().user_event("move.character"))
}

fn move_line(state: &EditorState, forward: bool) -> Option<TransactionSpec> {
    if state.read_only() {
        return None;
    }
    let mut changes = Vec::new();
    let mut ranges = Vec::new();
    for block in selected_line_blocks(state) {
        if if forward {
            block.to == state.doc.len()
        } else {
            block.from == 0
        } {
            continue;
        }
        let next_line = state.doc.line_at(if forward {
            block.to + 1
        } else {
            block.from - 1
        });
        let next_text = state.doc.slice(next_line.from, next_line.to);
        let size = next_line.len() + 1;
        if forward {
            changes.push(Change::delete(block.to, next_line.to));
            changes.push(Change::insert(block.from, format!("{next_text}\n")));
            for r in &block.ranges {
                ranges.push(SelectionRange::range(
                    (r.anchor() + size).min(state.doc.len()),
                    (r.head() + size).min(state.doc.len()),
                ));
            }
        } else {
            changes.push(Change::delete(next_line.from, block.from));
            changes.push(Change::insert(block.to, format!("\n{next_text}")));
            for r in &block.ranges {
                ranges.push(SelectionRange::range(r.anchor() - size, r.head() - size));
            }
        }
    }
    if changes.is_empty() {
        return None;
    }
    Some(
        TransactionSpec::default()
            .changes(changes)
            .scroll()
            .selection(EditorSelection::create(ranges, state.selection.main_index))
            .user_event("move.line"),
    )
}

pub fn move_line_up(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    move_line(v.state(), false)
}

pub fn move_line_down(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    move_line(v.state(), true)
}

fn copy_line(state: &EditorState, forward: bool) -> Option<TransactionSpec> {
    if state.read_only() {
        return None;
    }
    let changes: Vec<Change> = selected_line_blocks(state)
        .iter()
        .map(|b| {
            let text = state.slice_doc(b.from, b.to);
            if forward {
                Change::insert(b.from, format!("{text}\n"))
            } else {
                Change::insert(b.to, format!("\n{text}"))
            }
        })
        .collect();
    let set = state.changes(&changes);
    let selection = state
        .selection
        .map(&set.desc(), if forward { 1 } else { -1 });
    Some(
        TransactionSpec::default()
            .changes(set)
            .selection(selection)
            .scroll()
            .user_event("input.copyline"),
    )
}

pub fn copy_line_up(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    copy_line(v.state(), false)
}

pub fn copy_line_down(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    copy_line(v.state(), true)
}

pub fn delete_line(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let list: Vec<Change> = selected_line_blocks(state)
        .iter()
        .map(|b| {
            let (mut from, mut to) = (b.from, b.to);
            if from > 0 {
                from -= 1;
            } else if to < state.doc.len() {
                to += 1;
            }
            Change::delete(from, to)
        })
        .collect();
    let changes = state.changes(&list);
    let selection = update_sel(&state.selection, |range| {
        let mut dist = None;
        if v.line_wrapping() {
            let block = v.line_block_at(range.head());
            let side = if range.assoc() == 0 { 1 } else { range.assoc() };
            if let Some((_, bottom)) = v.coords_at_pos(range.head(), side) {
                dist = Some(block.bottom() - bottom + v.default_line_height() / 2.0);
            }
        }
        v.move_vertically(range, true, dist)
    })
    .map(&changes.desc(), -1);
    Some(
        TransactionSpec::default()
            .changes(changes)
            .selection(selection)
            .scroll()
            .user_event("delete.line"),
    )
}

pub fn insert_newline(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    Some(
        v.state()
            .replace_selection("\n")
            .spec()
            .scroll()
            .user_event("input"),
    )
}

pub fn insert_newline_keep_indent(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    let changes = state.change_by_range(|r| {
        let line = state.doc.line_at(r.from);
        let text = state.doc.slice(line.from, line.to);
        let indent = leading_space(&text).to_string();
        RangeChange::with(
            vec![Change::new(r.from, r.to, format!("\n{indent}"))],
            SelectionRange::cursor(r.from + indent.len() + 1),
        )
    });
    Some(changes.spec().scroll().user_event("input"))
}

/// `isBetweenBrackets`: only the adjacent `()`, `[]`, `{}` case applies.
fn is_between_brackets(state: &EditorState, pos: usize) -> Option<(usize, usize)> {
    if pos == 0 || pos >= state.doc.len() {
        return None;
    }
    let pair = state.slice_doc(state.doc.clamp_to_char(pos - 1), pos + 1);
    matches!(pair.as_str(), "()" | "[]" | "{}").then_some((pos, pos))
}

fn newline_and_indent(state: &EditorState, at_eof: bool) -> Option<TransactionSpec> {
    if state.read_only() {
        return None;
    }
    let changes = state.change_by_range(|range| {
        let (mut from, mut to) = (range.from, range.to);
        let line = state.doc.line_at(from);
        let explode = if !at_eof && from == to {
            is_between_brackets(state, from)
        } else {
            None
        };
        if at_eof {
            let l = if to <= line.to {
                line
            } else {
                state.doc.line_at(to)
            };
            from = l.to;
            to = l.to;
        }
        let cx = IndentContext {
            simulate_break: Some(from),
            simulate_double_break: explode.is_some(),
            ..IndentContext::new(state)
        };
        let indent = get_indentation(&cx, from).unwrap_or_else(|| {
            let l = state.doc.line_at(from);
            let text = state.doc.slice(l.from, l.to);
            let space = leading_space(&text);
            count_column(space, state.tab_size(), space.len())
        });
        let text = state.doc.slice(line.from, line.to);
        while to < line.to && text[to - line.from..].starts_with(is_js_space) {
            to += text[to - line.from..]
                .chars()
                .next()
                .map_or(1, char::len_utf8);
        }
        if let Some((f, t)) = explode {
            from = f;
            to = t;
        } else if from > line.from
            && from < line.from + 100
            && text[..from - line.from].chars().all(is_js_space)
        {
            from = line.from;
        }
        let mut insert = format!("\n{}", state.indent_string(indent));
        let inner_len = insert.len() - 1;
        if explode.is_some() {
            insert.push('\n');
            insert.push_str(&state.indent_string(cx.line_indent(line.from, -1)));
        }
        RangeChange::with(
            vec![Change::new(from, to, insert)],
            SelectionRange::cursor(from + 1 + inner_len),
        )
    });
    Some(changes.spec().scroll().user_event("input"))
}

/// `insertNewlineAndIndent` (Enter).
pub fn insert_newline_and_indent(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    newline_and_indent(v.state(), false)
}

/// `insertBlankLine` (Mod-Enter).
pub fn insert_blank_line(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    newline_and_indent(v.state(), true)
}

fn change_by_selected_line(
    state: &EditorState,
    mut f: impl FnMut(crate::text::Line, &str, &mut Vec<Change>, SelectionRange),
) -> crate::state::ChangeByRange {
    let mut at_line = 0usize;
    state.change_by_range(|range| {
        let mut changes = Vec::new();
        let mut pos = range.from;
        while pos <= range.to {
            let line = state.doc.line_at(pos);
            if line.number > at_line && (range.is_empty() || range.to > line.from) {
                let text = state.doc.slice(line.from, line.to);
                f(line, &text, &mut changes, range);
                at_line = line.number;
            }
            pos = line.to + 1;
        }
        let set = state.changes(&changes);
        let range =
            SelectionRange::range(set.map_pos(range.anchor(), 1), set.map_pos(range.head(), 1));
        RangeChange::with(changes, range)
    })
}

pub fn indent_selection(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let updated = std::cell::RefCell::new(HashMap::new());
    let changes = change_by_selected_line(state, |line, text, changes, range| {
        let snapshot = updated.borrow().clone();
        let cx = IndentContext {
            override_indentation: Some(&snapshot),
            ..IndentContext::new(state)
        };
        let Some(mut indent) = get_indentation(&cx, line.from) else {
            return;
        };
        if !text.chars().any(|c| !is_js_space(c)) {
            indent = 0;
        }
        let cur = leading_space(text);
        let norm = state.indent_string(indent);
        if cur != norm || range.from < line.from + cur.len() {
            updated.borrow_mut().insert(line.from, indent);
            changes.push(Change::new(line.from, line.from + cur.len(), norm));
        }
    });
    if changes.changes.is_empty() {
        return Some(TransactionSpec::default());
    }
    Some(changes.spec().user_event("indent"))
}

pub fn indent_more(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let unit = state.indent_unit().to_string();
    let changes = change_by_selected_line(state, |line, _, changes, _| {
        changes.push(Change::insert(line.from, unit.clone()));
    });
    Some(changes.spec().user_event("input.indent"))
}

pub fn indent_less(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let changes = change_by_selected_line(state, |line, text, changes, _| {
        let space = leading_space(text);
        if space.is_empty() {
            return;
        }
        let col = count_column(space, state.tab_size(), space.len());
        let insert = state.indent_string(col.saturating_sub(state.indent_unit_width()));
        let keep = space
            .bytes()
            .zip(insert.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        changes.push(Change::new(
            line.from + keep,
            line.from + space.len(),
            &insert[keep..],
        ));
    });
    Some(changes.spec().user_event("delete.dedent"))
}

pub fn insert_tab(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.selection.ranges.iter().any(|r| !r.is_empty()) {
        return indent_more(v);
    }
    Some(
        state
            .replace_selection("\t")
            .spec()
            .scroll()
            .user_event("input"),
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentOption {
    Toggle,
    Comment,
    Uncomment,
}

struct CommentPos {
    pos: usize,
    margin: usize,
}

const SEARCH_MARGIN: usize = 50;

fn find_block_comment(
    state: &EditorState,
    tokens: &BlockComment,
    from: usize,
    to: usize,
) -> Option<(CommentPos, CommentPos)> {
    let doc = &state.doc;
    let before_start = doc.clamp_to_char(from.saturating_sub(SEARCH_MARGIN));
    let text_before = state.slice_doc(before_start, from);
    let after_end = doc.clamp_to_char((to + SEARCH_MARGIN).min(doc.len()));
    let after_end = if after_end < to { to } else { after_end };
    let text_after = state.slice_doc(to, after_end);
    let space_before = text_before.len() - text_before.trim_end_matches(is_js_space).len();
    let space_after = text_after.len() - text_after.trim_start_matches(is_js_space).len();
    let before_off = text_before.len() - space_before;
    let (open, close) = (&tokens.open, &tokens.close);
    if before_off >= open.len()
        && text_before.get(before_off - open.len()..before_off) == Some(open.as_str())
        && text_after.get(space_after..space_after + close.len()) == Some(close.as_str())
    {
        return Some((
            CommentPos {
                pos: from - space_before,
                margin: usize::from(space_before > 0),
            },
            CommentPos {
                pos: to + space_after,
                margin: usize::from(space_after > 0),
            },
        ));
    }
    let (start_text, end_text) = if to - from <= 2 * SEARCH_MARGIN {
        let t = state.slice_doc(from, to);
        (t.clone(), t)
    } else {
        let a = doc.clamp_to_char(from + SEARCH_MARGIN);
        let b = doc.clamp_to_char(to - SEARCH_MARGIN);
        (state.slice_doc(from, a), state.slice_doc(b, to))
    };
    let start_space = start_text.len() - start_text.trim_start_matches(is_js_space).len();
    let end_space = end_text.len() - end_text.trim_end_matches(is_js_space).len();
    let end_off = end_text.len() as isize - end_space as isize - close.len() as isize;
    if end_off >= 0
        && start_text.get(start_space..start_space + open.len()) == Some(open.as_str())
        && end_text.get(end_off as usize..end_off as usize + close.len()) == Some(close.as_str())
    {
        let end_off = end_off as usize;
        let after_open = start_text[start_space + open.len()..].chars().next();
        let before_close = end_text[..end_off].chars().next_back();
        return Some((
            CommentPos {
                pos: from + start_space + open.len(),
                margin: usize::from(after_open.is_some_and(is_js_space)),
            },
            CommentPos {
                pos: to - end_space - close.len(),
                margin: usize::from(before_close.is_some_and(is_js_space)),
            },
        ));
    }
    None
}

fn selected_line_ranges(state: &EditorState) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for r in &state.selection.ranges {
        let from_line = state.doc.line_at(r.from);
        let mut to_line = if r.to <= from_line.to {
            from_line
        } else {
            state.doc.line_at(r.to)
        };
        if to_line.from > from_line.from && to_line.from == r.to {
            to_line = if r.to == from_line.to + 1 {
                from_line
            } else {
                state.doc.line_at(r.to - 1)
            };
        }
        match ranges.last_mut() {
            Some(last) if last.1 > from_line.from => last.1 = to_line.to,
            _ => {
                let text = state.doc.slice(from_line.from, from_line.to);
                ranges.push((from_line.from + leading_space(&text).len(), to_line.to));
            }
        }
    }
    ranges
}

fn change_block_comment(
    option: CommentOption,
    state: &EditorState,
    ranges: &[(usize, usize)],
) -> Option<TransactionSpec> {
    let token = state
        .language()
        .and_then(|l| l.comment_tokens())
        .and_then(|t| t.block.clone())?;
    let comments: Vec<_> = ranges
        .iter()
        .map(|&(f, t)| find_block_comment(state, &token, f, t))
        .collect();
    if option != CommentOption::Uncomment && !comments.iter().all(Option::is_some) {
        let mut changes = Vec::new();
        for (i, &(from, to)) in ranges.iter().enumerate() {
            if comments[i].is_none() {
                changes.push(Change::insert(from, format!("{} ", token.open)));
                changes.push(Change::insert(to, format!(" {}", token.close)));
            }
        }
        return Some(TransactionSpec::default().changes(state.changes(&changes)));
    }
    if option != CommentOption::Comment && comments.iter().any(Option::is_some) {
        let mut changes = Vec::new();
        for (open, close) in comments.iter().flatten() {
            changes.push(Change::delete(
                open.pos - token.open.len(),
                open.pos + open.margin,
            ));
            changes.push(Change::delete(
                close.pos - close.margin,
                close.pos + token.close.len(),
            ));
        }
        return Some(TransactionSpec::default().changes(changes));
    }
    None
}

struct CommentLine {
    from: usize,
    text: String,
    comment: Option<usize>,
    indent: usize,
    empty: bool,
    single: bool,
}

fn change_line_comment(option: CommentOption, state: &EditorState) -> Option<TransactionSpec> {
    let token = state
        .language()
        .and_then(|l| l.comment_tokens())
        .and_then(|t| t.line.clone())?;
    let mut lines: Vec<CommentLine> = Vec::new();
    let mut prev_line: Option<usize> = None;
    for r in &state.selection.ranges {
        let (from, to) = (r.from, r.to);
        let start_i = lines.len();
        let mut min_indent = usize::MAX;
        let mut pos = from;
        while pos <= to {
            let line = state.doc.line_at(pos);
            if prev_line.is_none_or(|p| line.from > p) && (from == to || to > line.from) {
                prev_line = Some(line.from);
                let text = state.doc.slice(line.from, line.to);
                let indent = leading_space(&text).len();
                let empty = indent == text.len();
                let comment = (text.get(indent..indent + token.len()) == Some(token.as_str()))
                    .then_some(indent);
                if indent < text.len() && indent < min_indent {
                    min_indent = indent;
                }
                lines.push(CommentLine {
                    from: line.from,
                    text,
                    comment,
                    indent,
                    empty,
                    single: false,
                });
            }
            pos = line.to + 1;
        }
        if min_indent < usize::MAX {
            for l in &mut lines[start_i..] {
                if l.indent < l.text.len() {
                    l.indent = min_indent;
                }
            }
        }
        if lines.len() == start_i + 1 {
            lines[start_i].single = true;
        }
    }
    if option != CommentOption::Uncomment
        && lines
            .iter()
            .any(|l| l.comment.is_none() && (!l.empty || l.single))
    {
        let changes: Vec<Change> = lines
            .iter()
            .filter(|l| l.single || !l.empty)
            .map(|l| Change::insert(l.from + l.indent, format!("{token} ")))
            .collect();
        let set = state.changes(&changes);
        let selection = state.selection.map(&set.desc(), 1);
        return Some(TransactionSpec::default().changes(set).selection(selection));
    }
    if option != CommentOption::Comment && lines.iter().any(|l| l.comment.is_some()) {
        let changes: Vec<Change> = lines
            .iter()
            .filter_map(|l| {
                let c = l.comment?;
                let from = l.from + c;
                let mut to = from + token.len();
                if l.text.as_bytes().get(to - l.from) == Some(&b' ') {
                    to += 1;
                }
                Some(Change::delete(from, to))
            })
            .collect();
        return Some(TransactionSpec::default().changes(changes));
    }
    None
}

/// `toggleComment` (Mod-/): line comments when the language has them,
/// else block comments around the selected lines.
pub fn toggle_comment(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let tokens = state.language().and_then(|l| l.comment_tokens())?;
    if tokens.line.is_some() {
        change_line_comment(CommentOption::Toggle, state)
    } else if tokens.block.is_some() {
        change_block_comment(CommentOption::Toggle, state, &selected_line_ranges(state))
    } else {
        None
    }
}

/// `toggleBlockComment` (Shift-Alt-a).
pub fn toggle_block_comment(v: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = v.state();
    if state.read_only() {
        return None;
    }
    let ranges: Vec<(usize, usize)> = state
        .selection
        .ranges
        .iter()
        .map(|r| (r.from, r.to))
        .collect();
    change_block_comment(CommentOption::Toggle, state, &ranges)
}

/// Moves that skip atomic ranges use this for the target position.
pub fn skip_atomic_pos(v: &dyn ViewQueries, pos: usize, bias: i32) -> usize {
    skip_atomic_ranges(&v.atomic_ranges(), pos, bias)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use recur_textmate::{LanguageDataFields, language_data};

    use super::*;
    use crate::state::{LanguageConfig, StateConfig};
    use crate::view_queries::MonoView;

    fn view(doc: &str, anchor: usize, head: usize) -> MonoView {
        view_lang(doc, anchor, head, None)
    }

    fn view_lang(
        doc: &str,
        anchor: usize,
        head: usize,
        lang: Option<LanguageDataFields>,
    ) -> MonoView {
        let s = EditorState::create(
            doc,
            StateConfig {
                allow_multiple_selections: true,
                language: lang.map(|f| {
                    Rc::new(LanguageConfig {
                        scope: "source.test".into(),
                        data: language_data(&f),
                        fold_markers: None,
                    })
                }),
                ..StateConfig::default()
            },
        );
        let s = s
            .update(TransactionSpec::default().selection(EditorSelection::single(anchor, head)))
            .state()
            .clone();
        MonoView::new(s)
    }

    fn run(v: &MonoView, cmd: Command) -> (String, Vec<(usize, usize)>) {
        let s = v
            .state
            .update(cmd(v).expect("command applies"))
            .state()
            .clone();
        (
            s.doc.to_string(),
            s.selection
                .ranges
                .iter()
                .map(|r| (r.anchor(), r.head()))
                .collect(),
        )
    }

    #[test]
    fn motions() {
        let v = view("foo bar\n  baz", 0, 0);
        assert_eq!(run(&v, cursor_char_right).1, vec![(1, 1)]);
        assert_eq!(run(&v, cursor_group_right).1, vec![(3, 3)]);
        assert_eq!(run(&v, cursor_line_down).1, vec![(8, 8)]);
        assert_eq!(run(&v, cursor_line_end).1, vec![(7, 7)]);
        assert_eq!(run(&v, select_line_end).1, vec![(0, 7)]);
        assert_eq!(run(&v, cursor_doc_end).1, vec![(13, 13)]);
        let v = view("foo bar\n  baz", 13, 13);
        assert_eq!(run(&v, cursor_line_boundary_backward).1, vec![(10, 10)]);
        assert_eq!(
            run(
                &view("foo bar\n  baz", 10, 10),
                cursor_line_boundary_backward
            )
            .1,
            vec![(8, 8)]
        );
        assert_eq!(run(&v, cursor_group_left).1, vec![(10, 10)]);
        assert_eq!(run(&v, cursor_syntax_left).1, vec![(0, 0)]);
        assert!(cursor_char_left(&view("x", 0, 0)).is_none());
    }

    #[test]
    fn deletion() {
        assert_eq!(run(&view("ab", 2, 2), delete_char_backward).0, "a");
        assert_eq!(run(&view("    x", 4, 4), delete_char_backward).0, "  x");
        assert_eq!(run(&view("foo bar", 7, 7), delete_group_backward).0, "foo ");
        assert_eq!(run(&view("foo bar", 4, 4), delete_group_backward).0, "bar");
        assert_eq!(run(&view("foo bar", 0, 0), delete_to_line_end).0, "");
        assert_eq!(run(&view("a\nb", 1, 1), delete_char_forward).0, "ab");
        assert_eq!(run(&view("ab", 1, 1), transpose_chars).0, "ba");
        assert_eq!(
            run(&view("a  \nb ", 0, 0), delete_trailing_whitespace).0,
            "a\nb"
        );
    }

    #[test]
    fn line_operations() {
        assert_eq!(
            run(&view("a\nb\nc", 2, 2), move_line_up),
            ("b\na\nc".into(), vec![(0, 0)])
        );
        assert_eq!(
            run(&view("a\nb\nc", 2, 2), move_line_down),
            ("a\nc\nb".into(), vec![(4, 4)])
        );
        assert_eq!(run(&view("a\nb", 0, 0), copy_line_down).0, "a\na\nb");
        assert_eq!(run(&view("a\nb\nc", 2, 2), delete_line).0, "a\nc");
        assert_eq!(run(&view("a\nb", 0, 0), select_line).1, vec![(0, 2)]);
        assert_eq!(
            run(&view("ab", 1, 1), split_line),
            ("a\nb".into(), vec![(1, 1)])
        );
    }

    #[test]
    fn newline_indent_and_tabs() {
        assert_eq!(
            run(&view("  x", 3, 3), insert_newline_and_indent),
            ("  x\n  ".into(), vec![(6, 6)])
        );
        assert_eq!(
            run(&view("f(){}", 4, 4), insert_newline_and_indent).0,
            "f(){\n\n}"
        );
        assert_eq!(run(&view("x", 1, 1), insert_tab).0, "x\t");
        assert_eq!(run(&view("x\ny", 0, 3), indent_more).0, "  x\n  y");
        assert_eq!(run(&view("    x", 0, 0), indent_less).0, "  x");
        assert_eq!(
            run(&view("a\nb", 0, 1), add_cursor_below).1,
            vec![(0, 1), (3, 3)]
        );
    }

    #[test]
    fn comments() {
        let js = LanguageDataFields {
            line_comment: Some("//".into()),
            block_comment_start: Some("/*".into()),
            block_comment_end: Some("*/".into()),
            ..Default::default()
        };
        let v = view_lang("  a\n  b", 0, 7, Some(js.clone()));
        let (doc, _) = run(&v, toggle_comment);
        assert_eq!(doc, "  // a\n  // b");
        let v = view_lang(&doc, 0, doc.len(), Some(js.clone()));
        assert_eq!(run(&v, toggle_comment).0, "  a\n  b");
        let css = LanguageDataFields {
            block_comment_start: Some("/*".into()),
            block_comment_end: Some("*/".into()),
            ..Default::default()
        };
        let v = view_lang("a {}", 0, 0, Some(css.clone()));
        let (doc, _) = run(&v, toggle_comment);
        assert_eq!(doc, "/* a {} */");
        assert_eq!(
            run(&view_lang(&doc, 3, 3, Some(css)), toggle_comment).0,
            "a {}"
        );
    }

    #[test]
    fn matching_bracket_and_page_motion() {
        assert_eq!(
            run(&view("(a)", 0, 0), cursor_matching_bracket).1,
            vec![(3, 3)]
        );
        assert_eq!(
            run(&view("(a)", 3, 3), select_matching_bracket).1,
            vec![(3, 0)]
        );
        let doc: String = (0..60).map(|i| format!("{i}\n")).collect();
        let v = view(&doc, 0, 0);
        let (_, sel) = run(&v, cursor_page_down);
        assert!(sel[0].0 > 0);
    }
}
