//! xterm.js's search addon (`SearchAddon`, `SearchEngine`, `SearchState`,
//! `SearchResultTracker`, highlight decorations) over [`Buffer`].
//!
//! Strings are UTF-16 code units and regexes are ECMAScript (non-unicode mode), so indices,
//! case folding and pattern syntax match the JavaScript implementation.

use super::buffer::Buffer;
use super::selection::Selection;
use super::theme::Rgba;

const NON_WORD_CHARACTERS: &str = " ~!@#$%^&*()+`-=[]{}|\\;:\"',./<>?";
/// The terminal dock's `highlightLimit` (`PREVIEW_FIND_CAP`).
pub const HIGHLIGHT_LIMIT: usize = 999;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decorations {
    pub match_background: Rgba,
    pub active_match_background: Rgba,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
    pub whole_word: bool,
    pub incremental: bool,
    pub decorations: Option<Decorations>,
}

/// `onDidChangeResults`: `index` is -1 when no match is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchResults {
    pub index: i64,
    pub count: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid regular expression: {0}")]
pub struct InvalidRegex(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    pub(crate) col: i64,
    pub(crate) row: i64,
    pub(crate) size: i64,
}

#[derive(Default)]
pub struct Search {
    cached_term: Option<String>,
    last_options: Option<SearchOptions>,
    results: Vec<Hit>,
    /// Bottom-layer decorations: every highlighted result.
    highlights: Vec<Hit>,
    /// Top-layer decoration for the selected result.
    active: Option<Hit>,
    decorations: Option<Decorations>,
}

struct Pos {
    row: i64,
    col: i64,
}

type U16 = Vec<u16>;

fn utf16(s: &str) -> U16 {
    s.encode_utf16().collect()
}

fn lower(s: &[u16]) -> U16 {
    utf16(&String::from_utf16_lossy(s).to_lowercase())
}

fn index_of(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= hay.len()).then_some(from);
    }
    (from..=hay.len().saturating_sub(needle.len()))
        .find(|&i| hay.get(i..i + needle.len()) == Some(needle))
}

fn last_index_of(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    let max = from.min(hay.len().saturating_sub(needle.len()));
    if needle.len() > hay.len() {
        return None;
    }
    (0..=max)
        .rev()
        .find(|&i| hay.get(i..i + needle.len()) == Some(needle))
}

impl Search {
    pub fn cached_term(&self) -> Option<&str> {
        self.cached_term.as_deref()
    }

    pub fn last_options(&self) -> Option<&SearchOptions> {
        self.last_options.as_ref()
    }

    pub(crate) fn highlight_at(
        &self,
        buf: &Buffer,
        x: i64,
        y: i64,
    ) -> (Option<Rgba>, Option<Rgba>) {
        let Some(deco) = self.decorations else {
            return (None, None);
        };
        let cols = buf.cols() as i64;
        let covers = |h: &Hit| {
            let start = h.row * cols + h.col;
            let pos = y * cols + x;
            pos >= start && pos < start + h.size
        };
        let bottom = self
            .highlights
            .iter()
            .any(covers)
            .then_some(deco.match_background);
        let top = self
            .active
            .as_ref()
            .filter(|h| covers(h))
            .map(|_| deco.active_match_background);
        (bottom, top)
    }

    pub(crate) fn handle_trim(&mut self, amount: i64) {
        for h in self
            .highlights
            .iter_mut()
            .chain(self.results.iter_mut())
            .chain(self.active.iter_mut())
        {
            h.row -= amount;
        }
        let gone = |h: &Hit| h.row < 0;
        self.highlights.retain(|h| !gone(h));
        self.results.retain(|h| !gone(h));
        if self.active.as_ref().is_some_and(gone) {
            self.active = None;
        }
    }

    pub(crate) fn clear_cached_term(&mut self) {
        self.cached_term = None;
    }

    pub fn clear_decorations(&mut self, retain_cached_term: bool) {
        self.active = None;
        self.highlights.clear();
        self.results.clear();
        if !retain_cached_term {
            self.cached_term = None;
        }
    }

    /// The addon records the new options before comparing, so an options-only change never
    /// re-highlights; callers clear decorations when the options key changes.
    fn should_update_highlighting(&self, term: &str, opts: &SearchOptions) -> bool {
        opts.decorations.is_some() && self.cached_term.as_deref() != Some(term)
    }

    pub(crate) fn find(
        &mut self,
        buf: &Buffer,
        sel: &mut Selection,
        term: &str,
        opts: &SearchOptions,
        forward: bool,
        no_scroll: bool,
    ) -> Result<(bool, Option<i64>, Option<SearchResults>), InvalidRegex> {
        self.last_options = Some(opts.clone());
        let update = self.should_update_highlighting(term, opts);
        let engine = Engine::new(buf, term, opts)?;
        if update {
            self.highlight_all(buf, &engine, term, opts);
        }
        let mut scroll = None;
        let found = if term.is_empty() {
            sel.clear();
            self.clear_decorations(false);
            false
        } else {
            let hit = if forward {
                engine.find_next_with_selection(buf, sel, self.cached_term.as_deref())
            } else {
                engine.find_previous_with_selection(buf, sel, self.cached_term.as_deref())
            };
            self.active = None;
            match hit {
                None => {
                    sel.clear();
                    false
                }
                Some(h) => {
                    sel.set(h.col, h.row, h.size);
                    if opts.decorations.is_some() {
                        self.active = Some(h.clone());
                    }
                    let ydisp = buf.ydisp() as i64;
                    let rows = buf.rows() as i64;
                    if !no_scroll && (h.row >= ydisp + rows || h.row < ydisp) {
                        scroll = Some(h.row - ydisp - rows / 2);
                    }
                    true
                }
            }
        };
        let results = opts.decorations.map(|_| SearchResults {
            index: self
                .active
                .as_ref()
                .and_then(|a| self.results.iter().position(|r| r == a))
                .map_or(-1, |i| i as i64),
            count: self.results.len(),
        });
        self.cached_term = Some(term.to_string());
        Ok((found, scroll, results))
    }

    fn highlight_all(&mut self, buf: &Buffer, engine: &Engine, term: &str, opts: &SearchOptions) {
        if term.is_empty() {
            self.clear_decorations(false);
            return;
        }
        self.clear_decorations(true);
        let cols = buf.cols() as i64;
        let mut results: Vec<Hit> = Vec::new();
        let mut result = engine.find(buf, 0, 0);
        while let Some(r) = result {
            if results
                .last()
                .is_some_and(|p| p.row == r.row && p.col == r.col)
                || results.len() >= HIGHLIGHT_LIMIT
            {
                break;
            }
            let mut next_col = r.col + r.size;
            let mut next_row = r.row;
            if next_col >= cols {
                next_row += next_col / cols;
                next_col %= cols;
            }
            results.push(r);
            result = engine.find(buf, next_row, next_col);
        }
        self.results = results.clone();
        if opts.decorations.is_some() {
            self.decorations = opts.decorations;
            self.highlights = results;
        }
    }
}

struct Engine {
    term: U16,
    case_sensitive: bool,
    whole_word: bool,
    regex: Option<regress::Regex>,
}

impl Engine {
    fn new(_buf: &Buffer, term: &str, opts: &SearchOptions) -> Result<Self, InvalidRegex> {
        let regex = if opts.regex && !term.is_empty() {
            let flags = if opts.case_sensitive { "" } else { "i" };
            Some(regress::Regex::with_flags(term, flags).map_err(|e| InvalidRegex(e.to_string()))?)
        } else {
            None
        };
        Ok(Self {
            term: utf16(term),
            case_sensitive: opts.case_sensitive,
            whole_word: opts.whole_word,
            regex,
        })
    }

    fn total(buf: &Buffer) -> i64 {
        (buf.ybase() + buf.rows()) as i64
    }

    fn find(&self, buf: &Buffer, start_row: i64, start_col: i64) -> Option<Hit> {
        if self.term.is_empty() {
            return None;
        }
        let mut pos = Pos {
            row: start_row,
            col: start_col,
        };
        if let Some(h) = self.find_in_line(buf, &mut pos, false) {
            return Some(h);
        }
        for y in start_row + 1..Self::total(buf) {
            pos = Pos { row: y, col: 0 };
            if let Some(h) = self.find_in_line(buf, &mut pos, false) {
                return Some(h);
            }
        }
        None
    }

    fn selection_position(buf: &Buffer, sel: &Selection) -> Option<((i64, i64), (i64, i64))> {
        sel.range(buf)
            .filter(|_| sel.has_selection(buf))
            .map(|(s, e)| ((s[0], s[1]), (e[0], e[1])))
    }

    fn find_next_with_selection(
        &self,
        buf: &Buffer,
        sel: &mut Selection,
        cached: Option<&str>,
    ) -> Option<Hit> {
        let prev = Self::selection_position(buf, sel);
        sel.clear();
        let term = String::from_utf16_lossy(&self.term);
        let (mut start_col, mut start_row) = (0, 0);
        if let Some((s, e)) = prev {
            if cached == Some(term.as_str()) {
                (start_col, start_row) = e;
            } else {
                (start_col, start_row) = s;
            }
        }
        let mut pos = Pos {
            row: start_row,
            col: start_col,
        };
        let mut result = self.find_in_line(buf, &mut pos, false);
        if result.is_none() {
            for y in start_row + 1..Self::total(buf) {
                pos = Pos { row: y, col: 0 };
                result = self.find_in_line(buf, &mut pos, false);
                if result.is_some() {
                    break;
                }
            }
        }
        if result.is_none() && start_row != 0 {
            for y in 0..start_row {
                pos = Pos { row: y, col: 0 };
                result = self.find_in_line(buf, &mut pos, false);
                if result.is_some() {
                    break;
                }
            }
        }
        if result.is_none()
            && let Some((s, _)) = prev
        {
            pos = Pos { row: s.1, col: 0 };
            result = self.find_in_line(buf, &mut pos, false);
        }
        result
    }

    fn find_previous_with_selection(
        &self,
        buf: &Buffer,
        sel: &mut Selection,
        cached: Option<&str>,
    ) -> Option<Hit> {
        let prev = Self::selection_position(buf, sel);
        sel.clear();
        let term = String::from_utf16_lossy(&self.term);
        let cols = buf.cols() as i64;
        let bottom = Self::total(buf) - 1;
        let mut start_row = bottom;
        let mut pos = Pos {
            row: start_row,
            col: cols,
        };
        let mut result = None;
        if let Some((s, e)) = prev {
            pos = Pos { row: s.1, col: s.0 };
            start_row = s.1;
            if cached != Some(term.as_str()) {
                // Try to expand the selection to the right first.
                result = self.find_in_line(buf, &mut pos, false);
                if result.is_none() {
                    pos = Pos { row: e.1, col: e.0 };
                    start_row = e.1;
                }
            }
        }
        if result.is_none() {
            result = self.find_in_line(buf, &mut pos, true);
        }
        if result.is_none() {
            pos.col = pos.col.max(cols);
            for y in (0..start_row).rev() {
                pos.row = y;
                result = self.find_in_line(buf, &mut pos, true);
                if result.is_some() {
                    break;
                }
            }
        }
        if result.is_none() && start_row != bottom {
            for y in (start_row..=bottom).rev() {
                pos.row = y;
                result = self.find_in_line(buf, &mut pos, true);
                if result.is_some() {
                    break;
                }
            }
        }
        result
    }

    fn is_whole_word(index: usize, line: &[u16], len: usize) -> bool {
        let sep = |c: Option<&u16>| {
            c.is_some_and(|c| NON_WORD_CHARACTERS.encode_utf16().any(|n| n == *c))
        };
        (index == 0 || sep(line.get(index - 1)))
            && (index + len == line.len() || sep(line.get(index + len)))
    }

    fn find_in_line(&self, buf: &Buffer, pos: &mut Pos, reverse: bool) -> Option<Hit> {
        let row = pos.row;
        let col = pos.col;
        let cols = buf.cols() as i64;
        if row >= 0 && buf.is_wrapped(row as usize) {
            if reverse {
                pos.col += cols;
                return None;
            }
            pos.row -= 1;
            pos.col += cols;
            return self.find_in_line(buf, pos, false);
        }
        if row < 0 || !buf.has_row(row as usize) {
            return None;
        }
        let (string_line, offsets) = line_with_wrap(buf, row as usize, true);
        let offset = cols_to_string_offset(buf, row, col);
        let mut term = self.term.clone();
        let (search_term, search_line) = if self.regex.is_some() || self.case_sensitive {
            (self.term.clone(), string_line.clone())
        } else {
            (lower(&self.term), lower(&string_line))
        };
        let mut result_index: Option<usize> = None;
        if let Some(re) = &self.regex {
            if reverse {
                let slice = &search_line[..offset.min(search_line.len())];
                let mut from = 0;
                while from <= slice.len() {
                    let Some(m) = re.find_from_ucs2(slice, from).next() else {
                        break;
                    };
                    result_index = Some(m.start());
                    term = slice[m.range()].to_vec();
                    from = m.start() + 1;
                }
            } else {
                let start = offset.min(search_line.len());
                let slice = &search_line[start..];
                if let Some(m) = re.find_from_ucs2(slice, 0).next()
                    && !m.range().is_empty()
                {
                    result_index = Some(start + m.start());
                    term = slice[m.range()].to_vec();
                }
            }
        } else if reverse {
            if offset >= search_term.len() {
                result_index =
                    last_index_of(&search_line, &search_term, offset - search_term.len());
            }
        } else {
            result_index = index_of(&search_line, &search_term, offset);
        }
        let index = result_index?;
        if self.whole_word && !Self::is_whole_word(index, &search_line, term.len()) {
            return None;
        }
        let mut start_off = 0;
        while start_off < offsets.len() - 1 && index >= offsets[start_off + 1] {
            start_off += 1;
        }
        let mut end_off = start_off;
        while end_off < offsets.len() - 1 && index + term.len() >= offsets[end_off + 1] {
            end_off += 1;
        }
        let start_col_off = index - offsets[start_off];
        let end_col_off = index + term.len() - offsets[end_off];
        let start_col =
            string_length_to_buffer_size(buf, row + start_off as i64, start_col_off as i64);
        let end_col = string_length_to_buffer_size(buf, row + end_off as i64, end_col_off as i64);
        Some(Hit {
            col: start_col,
            row: row + start_off as i64,
            size: end_col - start_col + cols * (end_off - start_off) as i64,
        })
    }
}

/// `translateBufferLineToStringWithWrap`: the logical line starting at `row` and the UTF-16
/// offsets where each wrapped row begins.
fn line_with_wrap(buf: &Buffer, mut row: usize, trim_right: bool) -> (U16, Vec<usize>) {
    let mut strings: Vec<U16> = Vec::new();
    let mut offsets = vec![0];
    while buf.has_row(row) {
        let wraps = buf.has_row(row + 1) && buf.is_wrapped(row + 1);
        let mut s = utf16(&buf.translate(row, !wraps && trim_right, 0, None));
        if wraps {
            let last = buf.cell_info(row, buf.cols() - 1);
            let last_null = last.is_some_and(|c| c.code() == 0 && c.width == 1);
            if last_null && buf.cell_info(row + 1, 0).is_some_and(|c| c.width == 2) {
                s.pop();
            }
        }
        let len = s.len();
        strings.push(s);
        if !wraps {
            break;
        }
        offsets.push(offsets[offsets.len() - 1] + len);
        row += 1;
    }
    (strings.concat(), offsets)
}

fn string_length_to_buffer_size(buf: &Buffer, row: i64, mut offset: i64) -> i64 {
    if row < 0 || !buf.has_row(row as usize) {
        return 0;
    }
    let row = row as usize;
    let mut i = 0;
    while i < offset {
        let Some(cell) = buf.cell_info(row, i as usize) else {
            break;
        };
        let len = cell.chars.encode_utf16().count() as i64;
        if len > 1 {
            offset -= len - 1;
        }
        if buf
            .cell_info(row, i as usize + 1)
            .is_some_and(|c| c.width == 0)
        {
            offset += 1;
        }
        i += 1;
    }
    offset
}

fn cols_to_string_offset(buf: &Buffer, start_row: i64, mut cols_left: i64) -> usize {
    let cols = buf.cols() as i64;
    let mut line_index = start_row;
    let mut offset = 0usize;
    while cols_left > 0 && line_index >= 0 && buf.has_row(line_index as usize) {
        let r = line_index as usize;
        let mut i = 0;
        while i < cols_left && i < cols {
            let Some(cell) = buf.cell_info(r, i as usize) else {
                break;
            };
            if cell.width != 0 {
                offset += if cell.code() == 0 {
                    1
                } else {
                    cell.chars.encode_utf16().count()
                };
            }
            i += 1;
        }
        line_index += 1;
        if line_index < 0
            || !buf.has_row(line_index as usize)
            || !buf.is_wrapped(line_index as usize)
        {
            break;
        }
        cols_left -= cols;
    }
    offset
}
