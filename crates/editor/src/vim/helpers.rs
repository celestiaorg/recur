//! vim.js's shared helpers: cursor clipping, selection building, visual
//! mode exit, marks and the jump list.

use super::cm::Cm;
use super::mode_change;
use super::state::{LastSelection, VALID_MARKS, with_global};
use super::types::*;

/// `isWhiteSpaceString` (`/^\s*$/`).
pub fn is_ws_str(s: &str) -> bool {
    s.chars().all(crate::state::is_js_space)
}

pub fn is_ws(c: Option<char>) -> bool {
    c.is_none_or(crate::state::is_js_space)
}

/// `findFirstNonWhiteSpaceCharacter`.
pub fn first_non_ws(text: &str) -> i64 {
    if text.is_empty() {
        return 0;
    }
    text.chars()
        .position(|c| !crate::state::is_js_space(c))
        .unwrap_or(text.chars().count()) as i64
}

/// `CM.isWordChar` (`[\w\p{Alphabetic}\p{Number}_]`).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `clipCursorToContent`: within the document, and before the line break
/// unless inserting or selecting.
pub fn clip_cursor_to_content(cm: &Cm, cur: Pos, _old: Option<Pos>) -> Pos {
    let vim = cm.vim_ref();
    let include_line_break = vim.insert_mode || vim.visual_mode;
    let line = cur.line.max(0).min(cm.last_line());
    let len = cm.line_len(line);
    let max_ch = len - 1 + i64::from(include_line_break);
    let ch = cur.ch.max(0).min(max_ch);
    Pos::new(line, ch)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelMode {
    Char,
    Line,
    Block,
}

/// `makeCmSelection`'s result.
#[derive(Clone, Debug)]
pub struct CmSel {
    pub ranges: Vec<Range>,
    pub primary: usize,
}

/// `makeCmSelection`.
pub fn make_cm_selection(cm: &Cm, sel: Range, mode: SelMode, exclusive: bool) -> CmSel {
    let mut head = sel.head;
    let mut anchor = sel.anchor;
    match mode {
        SelMode::Char => {
            let head_off = if !exclusive && !before(sel.head, sel.anchor) {
                1
            } else {
                0
            };
            let anchor_off = if before(sel.head, sel.anchor) { 1 } else { 0 };
            CmSel {
                ranges: vec![Range {
                    anchor: offset(sel.anchor, 0, anchor_off),
                    head: offset(sel.head, 0, head_off),
                }],
                primary: 0,
            }
        }
        SelMode::Line => {
            if !before(sel.head, sel.anchor) {
                anchor.ch = 0;
                let last = cm.last_line();
                if head.line > last {
                    head.line = last;
                }
                head.ch = cm.line_len(head.line);
            } else {
                head.ch = 0;
                anchor.ch = cm.line_len(anchor.line);
            }
            CmSel {
                ranges: vec![Range { anchor, head }],
                primary: 0,
            }
        }
        SelMode::Block => {
            let top = anchor.line.min(head.line);
            let bottom = anchor.line.max(head.line);
            let (mut from_ch, mut to_ch) = (anchor.ch, head.ch);
            if from_ch < to_ch {
                to_ch += 1;
            } else {
                from_ch += 1;
            }
            let height = bottom - top + 1;
            let primary = if head.line == top {
                0
            } else {
                (height - 1) as usize
            };
            let ranges = (0..height)
                .map(|i| Range {
                    anchor: Pos::new(top + i, from_ch),
                    head: Pos::new(top + i, to_ch),
                })
                .collect();
            CmSel { ranges, primary }
        }
    }
}

/// `updateCmSelection`.
pub fn update_cm_selection(cm: &mut Cm, sel: Option<Range>, mode: Option<SelMode>) {
    let sel = sel.unwrap_or(cm.vim().sel);
    let mode = mode.unwrap_or(if cm.vim().visual_line {
        SelMode::Line
    } else if cm.vim().visual_block {
        SelMode::Block
    } else {
        SelMode::Char
    });
    let s = make_cm_selection(cm, sel, mode, false);
    cm.set_selections(&s.ranges, s.primary);
}

/// `getHead`.
pub fn get_head(cm: &Cm) -> Pos {
    let cur = cm.get_cursor();
    if cm.get_selection().chars().count() == 1 {
        return pmin(cur, cm.get_anchor());
    }
    cur
}

/// `updateLastSelection`.
pub fn update_last_selection(cm: &mut Cm) {
    let anchor = cm.vim().sel.anchor;
    let mut head = cm.vim().sel.head;
    if let Some(text) = cm.vim().last_pasted_text.take() {
        let idx = cm.index_from_pos(anchor) + text.len();
        head = cm.pos_from_index(idx);
    }
    let anchor_mark = cm.set_bookmark(anchor, false);
    let head_mark = cm.set_bookmark(head, false);
    let vim = cm.vim();
    vim.last_selection = Some(LastSelection {
        anchor_mark,
        head_mark,
        anchor,
        head,
        visual_mode: vim.visual_mode,
        visual_line: vim.visual_line,
        visual_block: vim.visual_block,
    });
}

/// `exitVisualMode`.
pub fn exit_visual_mode(cm: &mut Cm, move_head: bool) {
    if move_head {
        let head = cm.vim().sel.head;
        let p = clip_cursor_to_content(cm, head, None);
        cm.set_cursor(p);
    }
    update_last_selection(cm);
    let vim = cm.vim();
    vim.visual_mode = false;
    vim.visual_line = false;
    vim.visual_block = false;
    if !vim.insert_mode {
        mode_change(cm, "normal", None);
    }
}

/// `expandSelection`.
pub fn expand_selection(cm: &Cm, mut start: Pos, mut end: Pos, mv: bool) -> (Pos, Pos) {
    let sel = cm.vim_ref().sel;
    let mut head = if mv { start } else { sel.head };
    let mut anchor = if mv { start } else { sel.anchor };
    if before(end, start) {
        std::mem::swap(&mut start, &mut end);
    }
    if before(head, anchor) {
        head = pmin(start, head);
        anchor = pmax(anchor, end);
    } else {
        anchor = pmin(start, anchor);
        head = pmax(head, end);
        head = offset(head, 0, -1);
        if head.ch == -1 && head.line != 0 {
            head = Pos::new(head.line - 1, cm.line_len(head.line - 1));
        }
    }
    (anchor, head)
}

/// `clipToLine`: drop trailing newlines and whitespace lines.
pub fn clip_to_line(cm: &Cm, cur_start: Pos, cur_end: &mut Pos) {
    let selection = cm.get_range(cur_start, *cur_end);
    let trailing_ws = selection
        .rfind('\n')
        .is_some_and(|i| is_ws_str(&selection[i + 1..]));
    if !trailing_ws {
        return;
    }
    let mut lines: Vec<&str> = selection.split('\n').collect();
    lines.pop();
    let mut line = lines.pop();
    while !lines.is_empty() && line.is_some_and(|l| !l.is_empty() && is_ws_str(l)) {
        cur_end.line -= 1;
        cur_end.ch = 0;
        line = lines.pop();
    }
    if line.is_some_and(|l| !l.is_empty()) {
        cur_end.line -= 1;
        cur_end.ch = cm.line_len(cur_end.line);
    } else {
        cur_end.ch = 0;
    }
}

/// `updateMark`.
pub fn update_mark(cm: &mut Cm, name: &str, pos: Pos) {
    let latin =
        name.chars().count() == 1 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !VALID_MARKS.contains(&name) && !latin {
        return;
    }
    if let Some(old) = cm.vim().marks.get(name) {
        cm.mark_clear(old);
    }
    let id = cm.set_bookmark(pos, false);
    cm.vim().marks.insert(name.to_string(), id);
}

/// `getMarkPos`.
pub fn get_mark_pos(cm: &mut Cm, name: &str) -> Option<Pos> {
    if name == "'" || name == "`" {
        return Some(jump_find(cm, -1).unwrap_or(Pos::new(0, 0)));
    }
    if name == "." {
        return Some(cm.last_edit_end());
    }
    let id = cm.vim().marks.get(name)?;
    cm.mark_find(id)
}

/// `recordJumpPosition`.
pub fn record_jump_position(cm: &mut Cm, old: Pos, new: Pos) {
    if old != new {
        jump_add(cm, old, new);
    }
}

/// `jumpList.add`.
fn jump_add(cm: &mut Cm, old: Pos, new: Pos) {
    let (pointer, cur_mark) = with_global(|g| {
        let jl = &g.jump_list;
        let current = jl.pointer.rem_euclid(super::state::JUMP_SIZE);
        (
            jl.pointer,
            if jl.pointer >= 0 {
                jl.buffer[current as usize]
            } else {
                None
            },
        )
    });
    let _ = pointer;
    let use_next = |cm: &mut Cm, cursor: Pos| {
        let next = with_global(|g| {
            g.jump_list.pointer += 1;
            g.jump_list.pointer.rem_euclid(super::state::JUMP_SIZE)
        });
        if let Some(trash) = with_global(|g| g.jump_list.buffer[next as usize]) {
            cm.mark_clear(trash);
        }
        let id = cm.set_bookmark(cursor, false);
        with_global(|g| g.jump_list.buffer[next as usize] = Some(id));
    };
    match cur_mark {
        Some(m) => {
            if let Some(p) = cm.mark_find(m)
                && p != old
            {
                use_next(cm, old);
            }
        }
        None => use_next(cm, old),
    }
    use_next(cm, new);
    with_global(|g| {
        let jl = &mut g.jump_list;
        jl.head = jl.pointer;
        jl.tail = (jl.pointer - super::state::JUMP_SIZE + 1).max(0);
    });
}

/// `jumpList.move`: the mark `offset` entries away.
pub fn jump_move(cm: &mut Cm, offset_: i64) -> Option<u64> {
    let size = super::state::JUMP_SIZE;
    let mut mark = with_global(|g| {
        let jl = &mut g.jump_list;
        jl.pointer += offset_;
        if jl.pointer > jl.head {
            jl.pointer = jl.head;
        } else if jl.pointer < jl.tail {
            jl.pointer = jl.tail;
        }
        jl.buffer[(size + jl.pointer).rem_euclid(size) as usize]
    });
    if let Some(m) = mark
        && cm.mark_find(m).is_none()
    {
        let inc = if offset_ > 0 { 1 } else { -1 };
        let old = cm.get_cursor();
        loop {
            let (m2, pointer, head, tail) = with_global(|g| {
                let jl = &mut g.jump_list;
                jl.pointer += inc;
                (
                    jl.buffer[(size + jl.pointer).rem_euclid(size) as usize],
                    jl.pointer,
                    jl.head,
                    jl.tail,
                )
            });
            mark = m2;
            if let Some(m) = mark
                && let Some(p) = cm.mark_find(m)
                && p != old
            {
                break;
            }
            if !(pointer < head && pointer > tail) {
                break;
            }
        }
    }
    mark
}

/// `jumpList.find`.
pub fn jump_find(cm: &mut Cm, offset_: i64) -> Option<Pos> {
    let old = with_global(|g| g.jump_list.pointer);
    let mark = jump_move(cm, offset_);
    with_global(|g| g.jump_list.pointer = old);
    mark.and_then(|m| cm.mark_find(m))
}

/// `selectBlock`.
pub fn select_block(cm: &mut Cm, selection_end: &mut Pos) -> Pos {
    let ranges = cm.list_selections();
    let head = cm.clip_pos(*selection_end);
    let is_clipped = *selection_end != head;
    let cur_head = cm.get_cursor();
    let prim = get_index(&ranges, cur_head).max(0) as usize;
    let was_clipped = ranges[prim].head == ranges[prim].anchor;
    let max = ranges.len() - 1;
    let index = if max - prim > prim { max } else { 0 };
    let mut base = ranges[index].anchor;
    let first = base.line.min(head.line);
    let last = base.line.max(head.line);
    let (mut base_ch, mut head_ch) = (base.ch, head.ch);
    let dir = ranges[index].head.ch - base_ch;
    let new_dir = head_ch - base_ch;
    if dir > 0 && new_dir <= 0 {
        base_ch += 1;
        if !is_clipped {
            head_ch -= 1;
        }
    } else if dir < 0 && new_dir >= 0 {
        base_ch -= 1;
        if !was_clipped {
            head_ch += 1;
        }
    } else if dir < 0 && new_dir == -1 {
        base_ch -= 1;
        head_ch += 1;
    }
    let sels: Vec<Range> = (first..=last)
        .map(|line| Range {
            anchor: Pos::new(line, base_ch),
            head: Pos::new(line, head_ch),
        })
        .collect();
    cm.set_selections(&sels, 0);
    selection_end.ch = head_ch;
    base.ch = base_ch;
    base
}

/// `selectForInsert`.
pub fn select_for_insert(cm: &mut Cm, head: Pos, height: i64) {
    let sel: Vec<Range> = (0..height)
        .map(|i| {
            let h = offset(head, i, 0);
            Range { anchor: h, head: h }
        })
        .collect();
    cm.set_selections(&sel, 0);
}

/// `getIndex`.
pub fn get_index(ranges: &[Range], cursor: Pos) -> i64 {
    ranges
        .iter()
        .position(|r| r.anchor == cursor || r.head == cursor)
        .map_or(-1, |i| i as i64)
}

/// `getSelectedAreaRange`.
pub fn get_selected_area_range(cm: &Cm) -> (Pos, Pos) {
    let selections = cm.list_selections();
    let start = selections[0];
    let end = selections[selections.len() - 1];
    let s = if before(start.anchor, start.head) {
        start.anchor
    } else {
        start.head
    };
    let e = if before(end.anchor, end.head) {
        end.head
    } else {
        end.anchor
    };
    (s, e)
}

/// `extendLineToColumn`.
pub fn extend_line_to_column(cm: &mut Cm, line: i64, column: i64) {
    let end = cm.line_len(line);
    let spaces = " ".repeat((column - end).max(0) as usize);
    cm.set_cursor(Pos::new(line, end));
    let c = cm.get_cursor();
    cm.replace_range(&spaces, c, None);
}

/// `escapeRegex`.
pub fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if ".?*+$[]/\\(){}|-".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `showConfirm`.
pub fn show_confirm(cm: &mut Cm, text: &str) {
    super::prompt::show_confirm(cm, text, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_non_ws_and_escape() {
        assert_eq!(first_non_ws("  ab"), 2);
        assert_eq!(first_non_ws("   "), 3);
        assert_eq!(first_non_ws(""), 0);
        assert_eq!(escape_regex("a.b-c"), "a\\.b\\-c");
    }
}
