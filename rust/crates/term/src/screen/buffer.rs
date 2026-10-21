//! xterm.js's buffer model (`IBuffer` / `IBufferLine`) over alacritty's grid, so selection,
//! search and link detection can be ported line for line.
//!
//! Rows are absolute: row 0 is the oldest scrollback line and `len() - 1` the bottom screen row
//! (alacritty `Line(row - history)`). A row is "wrapped" when it continues the row above it.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::{Grid, Term};

pub(crate) struct Buffer<'a> {
    grid: &'a Grid<Cell>,
    alt: bool,
}

/// One cell as xterm.js reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CellInfo {
    /// The cell's characters; empty for a null cell (never written, or the gap a wide char
    /// left at the end of a wrapped row).
    pub(crate) chars: String,
    /// 2 for a wide char, 0 for its trailing half, 1 otherwise.
    pub(crate) width: usize,
}

impl CellInfo {
    pub(crate) fn code(&self) -> u32 {
        self.chars.chars().next().map_or(0, u32::from)
    }
}

impl<'a> Buffer<'a> {
    pub(crate) fn new<T>(term: &'a Term<T>) -> Self {
        Self {
            grid: term.grid(),
            alt: term.mode().contains(TermMode::ALT_SCREEN),
        }
    }

    /// The alternate screen has no scrollback.
    pub(crate) fn has_scrollback(&self) -> bool {
        !self.alt
    }

    /// Cursor `(x, y)`, screen-relative.
    pub(crate) fn cursor(&self) -> (i64, i64) {
        let p = self.grid.cursor.point;
        (p.column.0 as i64, p.line.0 as i64)
    }

    pub(crate) fn cols(&self) -> usize {
        self.grid.columns()
    }

    pub(crate) fn rows(&self) -> usize {
        self.grid.screen_lines()
    }

    /// Scrollback lines above the screen (xterm `ybase`).
    pub(crate) fn ybase(&self) -> usize {
        self.grid.history_size()
    }

    /// First row shown in the viewport (xterm `ydisp`).
    pub(crate) fn ydisp(&self) -> usize {
        self.ybase() - self.grid.display_offset()
    }

    pub(crate) fn len(&self) -> usize {
        self.ybase() + self.rows()
    }

    pub(crate) fn has_row(&self, row: usize) -> bool {
        row < self.len()
    }

    fn line(&self, row: usize) -> Line {
        Line(row as i32 - self.ybase() as i32)
    }

    fn cell(&self, row: usize, col: usize) -> &'a Cell {
        &self.grid[self.line(row)][Column(col)]
    }

    pub(crate) fn is_wrapped(&self, row: usize) -> bool {
        row > 0
            && self.has_row(row)
            && self
                .cell(row - 1, self.cols() - 1)
                .flags
                .contains(Flags::WRAPLINE)
    }

    pub(crate) fn cell_info(&self, row: usize, col: usize) -> Option<CellInfo> {
        if !self.has_row(row) || col >= self.cols() {
            return None;
        }
        let c = self.cell(row, col);
        if c.flags.contains(Flags::WIDE_CHAR_SPACER) {
            return Some(CellInfo {
                chars: String::new(),
                width: 0,
            });
        }
        if c.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            return Some(CellInfo {
                chars: String::new(),
                width: 1,
            });
        }
        let mut chars = String::new();
        chars.push(c.c);
        if let Some(zw) = c.zerowidth() {
            chars.extend(zw);
        }
        Some(CellInfo {
            chars,
            width: if c.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            },
        })
    }

    pub(crate) fn hyperlink(
        &self,
        row: usize,
        col: usize,
    ) -> Option<alacritty_terminal::term::cell::Hyperlink> {
        (self.has_row(row) && col < self.cols())
            .then(|| self.cell(row, col).hyperlink())
            .flatten()
    }

    pub(crate) fn width_at(&self, row: usize, col: usize) -> usize {
        self.cell_info(row, col).map_or(1, |c| c.width)
    }

    pub(crate) fn has_content(&self, row: usize, col: usize) -> bool {
        let c = self.cell(row, col);
        !c.flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            && (c.c != ' ' || c.zerowidth().is_some())
    }

    /// Columns up to and including the last written cell (xterm `getTrimmedLength`). Alacritty
    /// does not distinguish erased cells from written spaces, so trailing spaces are trimmed.
    pub(crate) fn trimmed_length(&self, row: usize) -> usize {
        if !self.has_row(row) {
            return 0;
        }
        (0..self.cols())
            .rev()
            .find(|&x| self.has_content(row, x))
            .map_or(0, |x| x + self.width_at(row, x).max(1))
    }

    /// xterm `translateBufferLineToString(row, trimRight, startCol, endCol)`.
    pub(crate) fn translate(
        &self,
        row: usize,
        trim_right: bool,
        start: usize,
        end: Option<usize>,
    ) -> String {
        if !self.has_row(row) {
            return String::new();
        }
        let mut end = end.unwrap_or(self.cols()).min(self.cols());
        if trim_right {
            end = end.min(self.trimmed_length(row));
        }
        let mut out = String::new();
        let mut x = start;
        while x < end {
            if let Some(info) = self.cell_info(row, x) {
                if info.chars.is_empty() {
                    out.push(' ');
                } else {
                    out.push_str(&info.chars);
                }
                x += info.width.max(1);
            } else {
                break;
            }
        }
        out
    }

    /// xterm `getWrappedRangeForLine`.
    pub(crate) fn wrapped_range(&self, row: usize) -> (usize, usize) {
        let mut first = row;
        while first > 0 && self.is_wrapped(first) {
            first -= 1;
        }
        let mut last = row;
        while last + 1 < self.len() && self.is_wrapped(last + 1) {
            last += 1;
        }
        (first, last)
    }
}
