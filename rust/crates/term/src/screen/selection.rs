//! xterm.js's `SelectionModel` and `SelectionService`, over [`Buffer`].
//!
//! Coordinates are `[col, row]` in absolute buffer rows. Mouse positions arrive already
//! converted with xterm's `getCoords(..., isSelection = true)` rounding (see
//! [`crate::input::selection_coords`]).

use super::buffer::Buffer;

/// xterm.js `wordSeparator` default.
pub const WORD_SEPARATOR: &str = " ()[]{}',\"`";
const DRAG_SCROLL_MAX_THRESHOLD: f64 = 50.0;
const DRAG_SCROLL_MAX_SPEED: f64 = 15.0;
/// Interval between drag-scroll steps while the mouse is held outside the grid.
pub const DRAG_SCROLL_INTERVAL_MS: u64 = 50;
/// A click shorter than this with Alt held moves the prompt cursor.
pub const ALT_CLICK_MOVE_CURSOR_MS: u64 = 500;

pub(crate) type Pos = [i64; 2];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    #[default]
    Normal,
    Word,
    Line,
    Column,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Model {
    pub(crate) select_all: bool,
    pub(crate) start_length: i64,
    pub(crate) start: Option<Pos>,
    pub(crate) end: Option<Pos>,
}

impl Model {
    pub(crate) fn clear(&mut self) {
        *self = Model::default();
    }

    pub(crate) fn reversed(&self) -> bool {
        match (self.start, self.end) {
            (Some(s), Some(e)) => s[1] > e[1] || (s[1] == e[1] && s[0] > e[0]),
            _ => false,
        }
    }

    pub(crate) fn final_start(&self) -> Option<Pos> {
        if self.select_all {
            return Some([0, 0]);
        }
        match (self.start, self.end) {
            (Some(_), Some(e)) if self.reversed() => Some(e),
            (s, _) => s,
        }
    }

    pub(crate) fn final_end(&self, cols: i64, ybase: i64, rows: i64) -> Option<Pos> {
        if self.select_all {
            return Some([cols, ybase + rows - 1]);
        }
        let start = self.start?;
        let Some(end) = self.end.filter(|_| !self.reversed()) else {
            let spl = start[0] + self.start_length;
            if spl > cols {
                if spl % cols == 0 {
                    return Some([cols, start[1] + spl / cols - 1]);
                }
                return Some([spl % cols, start[1] + spl / cols]);
            }
            return Some([spl, start[1]]);
        };
        if self.start_length != 0 && end[1] == start[1] {
            let spl = start[0] + self.start_length;
            if spl > cols {
                return Some([spl % cols, start[1] + spl / cols]);
            }
            return Some([spl.max(end[0]), end[1]]);
        }
        Some(end)
    }

    /// Shifts the selection after `amount` rows were trimmed off the top of the scrollback.
    /// Returns whether it needs a redraw.
    pub(crate) fn handle_trim(&mut self, amount: i64) -> bool {
        if let Some(s) = &mut self.start {
            s[1] -= amount;
        }
        if let Some(e) = &mut self.end {
            e[1] -= amount;
        }
        if self.end.is_some_and(|e| e[1] < 0) {
            self.clear();
            return true;
        }
        if self.start.is_some_and(|s| s[1] < 0) {
            self.start = Some([0, 0]);
            return true;
        }
        false
    }
}

/// Result of a mouse-up in the selection service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseUp {
    /// Nothing for the PTY.
    None,
    /// Alt-click moved the prompt cursor: send these arrow keys.
    MoveCursor(String),
}

#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub(crate) model: Model,
    pub(crate) mode: SelectionMode,
    dragging: bool,
    drag_scroll_amount: i64,
}

impl Selection {
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// True while a mouse-down selection drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub(crate) fn has_selection(&self, buf: &Buffer) -> bool {
        match (self.model.final_start(), self.final_end(buf)) {
            (Some(s), Some(e)) => s != e,
            _ => false,
        }
    }

    pub(crate) fn final_end(&self, buf: &Buffer) -> Option<Pos> {
        self.model
            .final_end(buf.cols() as i64, buf.ybase() as i64, buf.rows() as i64)
    }

    pub(crate) fn range(&self, buf: &Buffer) -> Option<(Pos, Pos)> {
        Some((self.model.final_start()?, self.final_end(buf)?))
    }

    pub(crate) fn clear(&mut self) {
        self.model.clear();
        self.dragging = false;
    }

    pub(crate) fn text(&self, buf: &Buffer) -> String {
        let Some((start, end)) = self.range(buf) else {
            return String::new();
        };
        let mut result: Vec<String> = Vec::new();
        let row = |r: i64| r.max(0) as usize;
        if self.mode == SelectionMode::Column {
            if start[0] == end[0] {
                return String::new();
            }
            let (a, b) = if start[0] < end[0] {
                (start[0], end[0])
            } else {
                (end[0], start[0])
            };
            for i in start[1]..=end[1] {
                result.push(buf.translate(row(i), true, a as usize, Some(b as usize)));
            }
        } else {
            let start_row_end = (start[1] == end[1]).then_some(end[0].max(0) as usize);
            result.push(buf.translate(
                row(start[1]),
                true,
                start[0].max(0) as usize,
                start_row_end,
            ));
            for i in start[1] + 1..end[1] {
                let text = buf.translate(row(i), true, 0, None);
                match result.last_mut() {
                    Some(last) if buf.is_wrapped(row(i)) => last.push_str(&text),
                    _ => result.push(text),
                }
            }
            if start[1] != end[1] {
                let text = buf.translate(row(end[1]), true, 0, Some(end[0].max(0) as usize));
                match result.last_mut() {
                    Some(last) if buf.is_wrapped(row(end[1])) => last.push_str(&text),
                    _ => result.push(text),
                }
            }
        }
        result
            .iter()
            .map(|l| l.replace('\u{a0}', " "))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn is_cell_selected(&self, buf: &Buffer, x: i64, y: i64) -> bool {
        let Some((start, end)) = self.range(buf) else {
            return false;
        };
        if start == end {
            return false;
        }
        let ydisp = buf.ydisp() as i64;
        let (vs, ve) = (start[1] - ydisp, end[1] - ydisp);
        let (cs, ce) = (vs.max(0), ve.min(buf.rows() as i64 - 1));
        if cs >= buf.rows() as i64 || ce < 0 {
            return false;
        }
        let y = y - ydisp;
        if self.mode == SelectionMode::Column {
            if start[0] <= end[0] {
                return x >= start[0] && y >= cs && x < end[0] && y <= ce;
            }
            return x < start[0] && y >= cs && x >= end[0] && y <= ce;
        }
        (y > vs && y < ve)
            || (vs == ve && y == vs && x >= start[0] && x < end[0])
            || (vs < ve && y == ve && x < end[0])
            || (vs < ve && y == vs && x >= start[0])
    }

    fn coords_in_selection(&self, buf: &Buffer, c: Pos) -> bool {
        let Some((start, end)) = self.range(buf) else {
            return false;
        };
        (c[1] > start[1] && c[1] < end[1])
            || (start[1] == end[1] && c[1] == start[1] && c[0] >= start[0] && c[0] < end[0])
            || (start[1] < end[1] && c[1] == end[1] && c[0] < end[0])
            || (start[1] < end[1] && c[1] == start[1] && c[0] >= start[0])
    }

    pub(crate) fn select_all(&mut self) {
        self.model.select_all = true;
    }

    /// `setSelection(col, row, length)` (used by search).
    pub(crate) fn set(&mut self, col: i64, row: i64, length: i64) {
        self.clear();
        self.model.start = Some([col, row]);
        self.model.start_length = length;
    }

    /// Mouse-down on the grid with the primary button. `click_count` is the DOM `detail`.
    pub(crate) fn mouse_down(
        &mut self,
        buf: &Buffer,
        coords: Pos,
        click_count: usize,
        shift: bool,
        column: bool,
        link: Option<(Pos, i64)>,
    ) {
        self.drag_scroll_amount = 0;
        if shift {
            if self.model.start.is_some() {
                self.model.end = Some(coords);
            }
        } else {
            match click_count {
                1 => self.single_click(buf, coords, column),
                2 if self.select_word_at_cursor(buf, coords, true, link) => {
                    self.mode = SelectionMode::Word
                }
                3 => {
                    self.mode = SelectionMode::Line;
                    self.select_line_at(buf, coords[1]);
                }
                _ => {}
            }
        }
        self.dragging = true;
    }

    fn single_click(&mut self, buf: &Buffer, coords: Pos, column: bool) {
        self.model.start_length = 0;
        self.model.select_all = false;
        self.mode = if column {
            SelectionMode::Column
        } else {
            SelectionMode::Normal
        };
        self.model.start = Some(coords);
        self.model.end = None;
        let (x, y) = (coords[0], coords[1]);
        if y < 0 || !buf.has_row(y as usize) || x as usize >= buf.cols() {
            return;
        }
        // A click on the right half of a wide char starts after it.
        if buf.width_at(y as usize, x as usize) == 0
            && let Some(s) = &mut self.model.start
        {
            s[0] += 1;
        }
    }

    /// Mouse move while the button is held. `scroll_amount` is from [`drag_scroll_amount`].
    pub(crate) fn mouse_move(&mut self, buf: &Buffer, coords: Pos, scroll_amount: i64) -> bool {
        let Some(start) = self.model.start else {
            return false;
        };
        let prev = self.model.end;
        let mut end = coords;
        let cols = buf.cols() as i64;
        match self.mode {
            SelectionMode::Line => end[0] = if end[1] < start[1] { 0 } else { cols },
            SelectionMode::Word => {
                self.model.end = Some(end);
                self.select_to_word_at(buf, end);
                end = self.model.end.unwrap_or(end);
            }
            _ => {}
        }
        self.drag_scroll_amount = scroll_amount;
        if self.mode != SelectionMode::Column {
            if scroll_amount > 0 {
                end[0] = cols;
            } else if scroll_amount < 0 {
                end[0] = 0;
            }
        }
        if end[1] >= 0
            && (end[1] as usize) < buf.len()
            && end[0] < cols
            && end[0] >= 0
            && buf.width_at(end[1] as usize, end[0] as usize) == 0
        {
            end[0] += 1;
        }
        self.model.end = Some(end);
        prev != Some(end)
    }

    /// One drag-scroll tick. Returns the lines to scroll the viewport by.
    pub(crate) fn drag_scroll(&self) -> i64 {
        if self.model.start.is_none() || self.model.end.is_none() || self.drag_scroll_amount == 0 {
            return 0;
        }
        self.drag_scroll_amount
    }

    /// After the viewport scrolled by a drag tick, pin the end to the viewport edge.
    pub(crate) fn after_drag_scroll(&mut self, buf: &Buffer) {
        let Some(end) = &mut self.model.end else {
            return;
        };
        let cols = buf.cols() as i64;
        if self.drag_scroll_amount > 0 {
            if self.mode != SelectionMode::Column {
                end[0] = cols;
            }
            end[1] = (buf.ydisp() as i64 + buf.rows() as i64 - 1).min(buf.len() as i64 - 1);
        } else {
            if self.mode != SelectionMode::Column {
                end[0] = 0;
            }
            end[1] = buf.ydisp() as i64;
        }
    }

    pub(crate) fn mouse_up(
        &mut self,
        buf: &Buffer,
        elapsed_ms: u64,
        alt: bool,
        report_coords: Option<Pos>,
        app_cursor: bool,
    ) -> MouseUp {
        self.dragging = false;
        if self.text(buf).chars().count() <= 1
            && elapsed_ms < ALT_CLICK_MOVE_CURSOR_MS
            && alt
            && buf.ybase() == buf.ydisp()
            && let Some(c) = report_coords
        {
            return MouseUp::MoveCursor(super::move_to_cell::sequence(buf, c[0], c[1], app_cursor));
        }
        MouseUp::None
    }

    pub(crate) fn right_click_select(
        &mut self,
        buf: &Buffer,
        coords: Pos,
        link: Option<(Pos, i64)>,
    ) -> bool {
        if self.coords_in_selection(buf, coords) {
            return false;
        }
        self.select_word_at_cursor(buf, coords, false, link)
    }

    fn select_word_at_cursor(
        &mut self,
        buf: &Buffer,
        coords: Pos,
        allow_ws: bool,
        link: Option<(Pos, i64)>,
    ) -> bool {
        if let Some((start, len)) = link {
            self.model.start = Some(start);
            self.model.start_length = len;
            self.model.end = None;
            return true;
        }
        self.select_word_at(buf, coords, allow_ws);
        self.model.end = None;
        true
    }

    fn select_word_at(&mut self, buf: &Buffer, mut coords: Pos, allow_ws: bool) {
        if let Some((mut start, length)) = word_at(buf, coords, allow_ws, true, true) {
            while start < 0 {
                start += buf.cols() as i64;
                coords[1] -= 1;
            }
            self.model.start = Some([start, coords[1]]);
            self.model.start_length = length;
        }
    }

    fn select_to_word_at(&mut self, buf: &Buffer, coords: Pos) {
        let Some((mut start, mut length)) = word_at(buf, coords, true, true, true) else {
            return;
        };
        let cols = buf.cols() as i64;
        let mut end_row = coords[1];
        while start < 0 {
            start += cols;
            end_row -= 1;
        }
        if !self.model.reversed() {
            while start + length > cols {
                length -= cols;
                end_row += 1;
            }
        }
        self.model.end = Some([
            if self.model.reversed() {
                start
            } else {
                start + length
            },
            end_row,
        ]);
    }

    fn select_line_at(&mut self, buf: &Buffer, row: i64) {
        let (first, last) = buf.wrapped_range(row.max(0) as usize);
        let cols = buf.cols() as i64;
        self.model.start = Some([0, first as i64]);
        self.model.end = None;
        // getRangeLength({x:0,y:first},{x:cols-1,y:last})
        self.model.start_length = (last as i64 - first as i64) * cols + cols;
    }
}

/// xterm `_getMouseEventScrollAmount`: `offset` is the pointer's y relative to the grid top,
/// `height` the grid's height.
pub fn drag_scroll_amount(offset: f64, height: f64) -> i64 {
    if (0.0..=height).contains(&offset) {
        return 0;
    }
    let mut offset = if offset > height {
        offset - height
    } else {
        offset
    };
    offset = offset.clamp(-DRAG_SCROLL_MAX_THRESHOLD, DRAG_SCROLL_MAX_THRESHOLD)
        / DRAG_SCROLL_MAX_THRESHOLD;
    (offset / offset.abs() + super::theme::js_round(offset * (DRAG_SCROLL_MAX_SPEED - 1.0))) as i64
}

fn is_separator(buf: &Buffer, row: usize, col: usize) -> bool {
    match buf.cell_info(row, col) {
        Some(c) if c.width == 0 => false,
        Some(c) => {
            let ch = if c.chars.is_empty() {
                ""
            } else {
                c.chars.as_str()
            };
            // `"".indexOf` of an empty needle is 0 in JS: null cells count as separators.
            ch.is_empty() || WORD_SEPARATOR.contains(ch)
        }
        None => false,
    }
}

fn line_chars(buf: &Buffer, row: usize) -> Vec<char> {
    buf.translate(row, false, 0, None).chars().collect()
}

/// Character index for viewport column `x`, compensating for wide chars and multi-char cells.
fn viewport_col_to_char_index(buf: &Buffer, row: usize, x: i64) -> i64 {
    let mut idx = x;
    let mut i = 0;
    while x >= i {
        if let Some(c) = buf.cell_info(row, i as usize) {
            let len = c.chars.chars().count() as i64;
            if c.width == 0 {
                idx -= 1;
            } else if len > 1 && x != i {
                idx += len - 1;
            }
        }
        i += 1;
    }
    idx
}

/// xterm `_getWordAt`: start column (may be negative when following wrapped rows above) and
/// length of the word under `coords`.
pub(crate) fn word_at(
    buf: &Buffer,
    coords: Pos,
    allow_ws: bool,
    above: bool,
    below: bool,
) -> Option<(i64, i64)> {
    let cols = buf.cols() as i64;
    if coords[0] >= cols || coords[1] < 0 || !buf.has_row(coords[1] as usize) {
        return None;
    }
    let row = coords[1] as usize;
    let line = line_chars(buf, row);
    let char_at =
        |i: i64| -> Option<char> { (i >= 0).then(|| line.get(i as usize).copied()).flatten() };
    let mut start_index = viewport_col_to_char_index(buf, row, coords[0]);
    let mut end_index = start_index;
    let char_offset = coords[0] - start_index;
    let (mut left_wide, mut right_wide, mut left_long, mut right_long) = (0i64, 0i64, 0i64, 0i64);

    if char_at(start_index) == Some(' ') {
        while start_index > 0 && char_at(start_index - 1) == Some(' ') {
            start_index -= 1;
        }
        while (end_index as usize) < line.len() && char_at(end_index + 1) == Some(' ') {
            end_index += 1;
        }
    } else {
        let mut start_col = coords[0];
        let mut end_col = coords[0];
        if buf.width_at(row, start_col as usize) == 0 {
            left_wide += 1;
            start_col -= 1;
        }
        if buf.width_at(row, end_col as usize) == 2 {
            right_wide += 1;
            end_col += 1;
        }
        let len = buf
            .cell_info(row, end_col as usize)
            .map_or(0, |c| c.chars.chars().count()) as i64;
        if len > 1 {
            right_long += len - 1;
            end_index += len - 1;
        }
        while start_col > 0 && start_index > 0 && !is_separator(buf, row, start_col as usize - 1) {
            let c = buf.cell_info(row, start_col as usize - 1);
            let len = c.as_ref().map_or(0, |c| c.chars.chars().count()) as i64;
            if c.as_ref().is_some_and(|c| c.width == 0) {
                left_wide += 1;
                start_col -= 1;
            } else if len > 1 {
                left_long += len - 1;
                start_index -= len - 1;
            }
            start_index -= 1;
            start_col -= 1;
        }
        while end_col + 1 < cols
            && end_index + 1 < line.len() as i64
            && !is_separator(buf, row, end_col as usize + 1)
        {
            let c = buf.cell_info(row, end_col as usize + 1);
            let len = c.as_ref().map_or(0, |c| c.chars.chars().count()) as i64;
            if c.as_ref().is_some_and(|c| c.width == 2) {
                right_wide += 1;
                end_col += 1;
            } else if len > 1 {
                right_long += len - 1;
                end_index += len - 1;
            }
            end_index += 1;
            end_col += 1;
        }
    }
    end_index += 1;

    let mut start = start_index + char_offset - left_wide + left_long;
    let mut length =
        cols.min(end_index - start_index + left_wide + right_wide - left_long - right_long);

    if !allow_ws {
        let slice: String = line
            .get(start_index.max(0) as usize..(end_index.max(0) as usize).min(line.len()))
            .map(|s| s.iter().collect())
            .unwrap_or_default();
        if slice.trim().is_empty() {
            return None;
        }
    }
    let code0 = |r: usize, c: usize| buf.cell_info(r, c).map_or(0, |c| c.code());
    if above
        && start == 0
        && code0(row, 0) != 32
        && row > 0
        && buf.is_wrapped(row)
        && code0(row - 1, cols as usize - 1) != 32
        && let Some((prev_start, _)) = word_at(buf, [cols - 1, coords[1] - 1], false, true, false)
    {
        let offset = cols - prev_start;
        start -= offset;
        length += offset;
    }
    if below
        && start + length == cols
        && code0(row, cols as usize - 1) != 32
        && buf.has_row(row + 1)
        && buf.is_wrapped(row + 1)
        && code0(row + 1, 0) != 32
        && let Some((_, next_len)) = word_at(buf, [0, coords[1] + 1], false, false, true)
    {
        length += next_len;
    }
    Some((start, length))
}
