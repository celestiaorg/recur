//! Display layout: how document lines become painted rows. Tabs expand to
//! tab stops, special characters become placeholders
//! (`highlightSpecialChars`), folds end in a `…` placeholder, long lines
//! wrap like `white-space: break-spaces; overflow-wrap: anywhere`, and a
//! height map (estimated until measured, like CodeMirror's) places every
//! line, fold and block widget vertically.
//!
//! [`LayoutView`] answers the motion queries commands need
//! ([`ViewQueries`]); text widths come from a [`TextMeasure`] so tests can
//! use fixed advances and the editor uses GPUI's shaper.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::selection::SelectionRange;
use crate::state::EditorState;
use crate::view_queries::{LineBlock, ViewQueries};

/// Widths of text in the editor font.
pub trait TextMeasure {
    /// x of every char boundary of `text` (`chars + 1` entries).
    fn char_xs(&self, text: &str) -> Vec<f32>;
}

/// Fixed-advance measure (tests, and the estimate before shaping).
pub struct MonoMeasure(pub f32);

impl TextMeasure for MonoMeasure {
    fn char_xs(&self, text: &str) -> Vec<f32> {
        let mut xs = Vec::with_capacity(text.len() + 1);
        let mut x = 0.0;
        xs.push(0.0);
        for _ in text.chars() {
            x += self.0;
            xs.push(x);
        }
        xs
    }
}

/// Characters `highlightSpecialChars` replaces by default.
pub fn is_special_char(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x0000..=0x0008 | 0x000a..=0x001f | 0x007f..=0x009f | 0x00ad | 0x061c | 0x200b | 0x200e | 0x200f
        | 0x2028 | 0x2029 | 0x202d | 0x202e | 0x2066 | 0x2067 | 0x2069 | 0xfeff | 0xfff9..=0xfffc)
}

/// The placeholder glyph for a special character: control pictures for
/// C0 controls, a bullet otherwise.
pub fn special_placeholder(c: char) -> char {
    let u = c as u32;
    if u >= 32 {
        '\u{2022}'
    } else if u == 10 {
        '\u{2424}'
    } else {
        char::from_u32(9216 + u).unwrap_or('\u{2022}')
    }
}

/// The title CodeMirror gives a special char placeholder (`\u` + hex).
pub fn special_title(c: char) -> String {
    format!("\\u{:x}", c as u32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegKind {
    Text,
    Tab,
    Special(char),
    /// The fold placeholder (`…`) after a folded line.
    FoldPlaceholder,
}

/// A display segment: line-relative document bytes and display bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seg {
    pub doc_from: usize,
    pub doc_to: usize,
    pub disp_from: usize,
    pub disp_to: usize,
    pub kind: SegKind,
}

/// A document line as displayed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayLine {
    pub text: String,
    pub segs: Vec<Seg>,
    /// Document length of the line (excluding the line break).
    pub doc_len: usize,
}

/// Build a display line. `folded` adds the fold placeholder at the end.
pub fn display_line(text: &str, tab_size: usize, special_chars: bool, folded: bool) -> DisplayLine {
    let mut out = String::with_capacity(text.len());
    let mut segs: Vec<Seg> = Vec::new();
    let mut col = 0usize;
    let mut text_start: Option<(usize, usize)> = None;
    let flush =
        |segs: &mut Vec<Seg>, start: &mut Option<(usize, usize)>, doc_to: usize, disp_to: usize| {
            if let Some((d, p)) = start.take()
                && doc_to > d
            {
                segs.push(Seg {
                    doc_from: d,
                    doc_to,
                    disp_from: p,
                    disp_to,
                    kind: SegKind::Text,
                });
            }
        };
    for (i, c) in text.char_indices() {
        if c == '\t' {
            flush(&mut segs, &mut text_start, i, out.len());
            let n = tab_size.max(1) - col % tab_size.max(1);
            let from = out.len();
            out.extend(std::iter::repeat_n(' ', n));
            segs.push(Seg {
                doc_from: i,
                doc_to: i + 1,
                disp_from: from,
                disp_to: out.len(),
                kind: SegKind::Tab,
            });
            col += n;
        } else if special_chars && is_special_char(c) {
            flush(&mut segs, &mut text_start, i, out.len());
            let from = out.len();
            out.push(special_placeholder(c));
            segs.push(Seg {
                doc_from: i,
                doc_to: i + c.len_utf8(),
                disp_from: from,
                disp_to: out.len(),
                kind: SegKind::Special(c),
            });
            col += 1;
        } else {
            if text_start.is_none() {
                text_start = Some((i, out.len()));
            }
            out.push(c);
            col += 1;
        }
    }
    flush(&mut segs, &mut text_start, text.len(), out.len());
    if folded {
        let from = out.len();
        out.push('\u{2026}');
        segs.push(Seg {
            doc_from: text.len(),
            doc_to: text.len(),
            disp_from: from,
            disp_to: out.len(),
            kind: SegKind::FoldPlaceholder,
        });
    }
    DisplayLine {
        text: out,
        segs,
        doc_len: text.len(),
    }
}

impl DisplayLine {
    /// Display offset of a line-relative document offset. `assoc < 0`
    /// keeps a position at a segment edge on the left segment.
    pub fn disp_of(&self, doc: usize, assoc: i32) -> usize {
        for s in &self.segs {
            if s.kind == SegKind::FoldPlaceholder {
                continue;
            }
            if doc < s.doc_from {
                return s.disp_from;
            }
            if doc == s.doc_from && (assoc >= 0 || s.doc_from == s.doc_to) {
                return s.disp_from;
            }
            if doc <= s.doc_to {
                return match s.kind {
                    SegKind::Text => s.disp_from + (doc - s.doc_from),
                    _ if doc == s.doc_from => s.disp_from,
                    _ => s.disp_to,
                };
            }
        }
        self.text_end()
    }

    /// Display length before the fold placeholder.
    pub fn text_end(&self) -> usize {
        self.segs
            .iter()
            .rev()
            .find(|s| s.kind != SegKind::FoldPlaceholder)
            .map_or(0, |s| s.disp_to)
    }

    /// Document offset for a display offset (atomic segments snap to
    /// their nearer edge).
    pub fn doc_of(&self, disp: usize) -> usize {
        for s in &self.segs {
            if disp < s.disp_to || (disp == s.disp_to && s.kind == SegKind::Text) {
                if disp <= s.disp_from {
                    return s.doc_from;
                }
                return match s.kind {
                    SegKind::Text => s.doc_from + (disp - s.disp_from),
                    SegKind::FoldPlaceholder => s.doc_from,
                    _ if disp - s.disp_from < s.disp_to - disp => s.doc_from,
                    _ => s.doc_to,
                };
            }
        }
        self.doc_len
    }

    /// The fold placeholder's display range, if any.
    pub fn placeholder(&self) -> Option<(usize, usize)> {
        self.segs
            .iter()
            .find(|s| s.kind == SegKind::FoldPlaceholder)
            .map(|s| (s.disp_from, s.disp_to))
    }
}

/// Display offsets where wrapped rows start (always includes 0). Breaks
/// follow CSS `break-spaces` + `overflow-wrap: anywhere`: after a run of
/// spaces when the next word does not fit, else anywhere.
pub fn wrap_rows(text: &str, xs: &[f32], width: f32) -> Vec<usize> {
    let mut rows = vec![0usize];
    if width <= 0.0 || xs.last().copied().unwrap_or(0.0) <= width {
        return rows;
    }
    let bounds: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let chars: Vec<char> = text.chars().collect();
    let mut row_start_ci = 0usize;
    let mut last_break: Option<usize> = None;
    let mut ci = 0usize;
    while ci < chars.len() {
        let end_x = xs[ci + 1] - xs[row_start_ci];
        if end_x > width && ci > row_start_ci {
            let brk = match last_break {
                Some(b) if b > row_start_ci && b <= ci => b,
                _ => ci,
            };
            rows.push(bounds[brk]);
            row_start_ci = brk;
            last_break = None;
            ci = brk;
            continue;
        }
        if chars[ci] == ' ' {
            last_break = Some(ci + 1);
        }
        ci += 1;
    }
    rows
}

/// A Fenwick tree of line heights.
#[derive(Clone, Debug, Default)]
struct Fenwick {
    tree: Vec<f64>,
}

impl Fenwick {
    fn build(values: &[f64]) -> Fenwick {
        let n = values.len();
        let mut tree = vec![0.0; n + 1];
        for (i, v) in values.iter().enumerate() {
            tree[i + 1] += v;
            let j = (i + 1) + ((i + 1) & (!(i + 1) + 1));
            if j <= n {
                let t = tree[i + 1];
                tree[j] += t;
            }
        }
        Fenwick { tree }
    }

    fn add(&mut self, i: usize, delta: f64) {
        let mut k = i + 1;
        while k < self.tree.len() {
            self.tree[k] += delta;
            k += k & (!k + 1);
        }
    }

    /// Sum of `values[..i]`.
    fn prefix(&self, i: usize) -> f64 {
        let mut k = i.min(self.tree.len().saturating_sub(1));
        let mut s = 0.0;
        while k > 0 {
            s += self.tree[k];
            k -= k & (!k + 1);
        }
        s
    }

    /// The largest `i` with `prefix(i) <= y`, and that prefix.
    fn lower(&self, y: f64) -> usize {
        let n = self.tree.len() - 1;
        let mut pos = 0;
        let mut rem = y;
        let mut step = n.next_power_of_two();
        while step > 0 {
            let next = pos + step;
            if next <= n && self.tree[next] <= rem {
                pos = next;
                rem -= self.tree[next];
            }
            step >>= 1;
        }
        pos
    }
}

/// Per-line heights (each line's own rows plus any block widgets above it;
/// zero for lines hidden by folds), estimated until measured.
#[derive(Clone, Debug, Default)]
pub struct HeightMap {
    heights: Vec<f64>,
    measured: Vec<bool>,
    fen: Fenwick,
}

impl HeightMap {
    pub fn new(heights: Vec<f64>) -> HeightMap {
        let n = heights.len();
        HeightMap {
            fen: Fenwick::build(&heights),
            measured: vec![false; n],
            heights,
        }
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// Top of 1-based line `n` (relative to the first line).
    pub fn top(&self, n: usize) -> f64 {
        self.fen.prefix(n.saturating_sub(1))
    }

    pub fn height(&self, n: usize) -> f64 {
        self.heights.get(n.wrapping_sub(1)).copied().unwrap_or(0.0)
    }

    pub fn total(&self) -> f64 {
        self.fen.prefix(self.heights.len())
    }

    /// Set line `n`'s height; returns the change.
    pub fn set(&mut self, n: usize, h: f64, measured: bool) -> f64 {
        let i = n - 1;
        let delta = h - self.heights[i];
        if delta != 0.0 {
            self.heights[i] = h;
            self.fen.add(i, delta);
        }
        self.measured[i] = measured;
        delta
    }

    pub fn is_measured(&self, n: usize) -> bool {
        self.measured
            .get(n.wrapping_sub(1))
            .copied()
            .unwrap_or(false)
    }

    /// The 1-based line covering vertical offset `y` (clamped).
    pub fn line_at_height(&self, y: f64) -> usize {
        if self.heights.is_empty() {
            return 1;
        }
        let i = self.fen.lower(y.max(0.0));
        let mut n = (i + 1).min(self.heights.len());
        while n < self.heights.len() && self.heights[n - 1] == 0.0 {
            n += 1;
        }
        n
    }

    /// Splice heights for a line replacement (1-based `from`, `old`
    /// lines replaced by `new` estimates).
    pub fn splice(&mut self, from: usize, old: usize, new: Vec<f64>) {
        let i = from - 1;
        let n_new = new.len();
        self.heights
            .splice(i..(i + old).min(self.heights.len()), new);
        self.measured.splice(
            i..(i + old).min(self.measured.len()),
            std::iter::repeat_n(false, n_new),
        );
        self.fen = Fenwick::build(&self.heights);
    }
}

/// Inputs to layout that come from prefs and the element's size.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutConfig {
    pub line_height: f32,
    /// Average advance of a character (`defaultCharacterWidth`).
    pub char_width: f32,
    pub tab_size: usize,
    pub special_chars: bool,
    pub wrap: bool,
    /// Width available to a line's text (content width minus `.cm-line`
    /// padding).
    pub wrap_width: f32,
    /// `.cm-content` vertical padding.
    pub padding_top: f32,
    pub padding_bottom: f32,
    /// Visible scroller height.
    pub viewport_height: f32,
    /// `.cm-line` left padding.
    pub line_padding_left: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        LayoutConfig {
            line_height: 18.6,
            char_width: 7.2,
            tab_size: 4,
            special_chars: true,
            wrap: false,
            wrap_width: 800.0,
            padding_top: 8.0,
            padding_bottom: 8.0,
            viewport_height: 600.0,
            line_padding_left: 6.0,
        }
    }
}

/// A block widget above a line (deleted merge chunk) or replacing lines
/// (collapsed unchanged region).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockWidget {
    /// 1-based line the widget sits above.
    pub line: usize,
    pub height: f32,
    /// The widget replaces lines `line..=replace_to` (collapsed region).
    pub replace_to: Option<usize>,
    /// Caller's index for the widget.
    pub id: usize,
}

/// A measured line: its display line, wrap rows and char boundary xs.
#[derive(Clone, Debug)]
pub struct MeasuredLine {
    pub display: DisplayLine,
    pub xs: Vec<f32>,
    /// Display byte offset of each char boundary (aligned with `xs`).
    pub bounds: Vec<usize>,
    pub rows: Vec<usize>,
}

impl MeasuredLine {
    fn new(display: DisplayLine, measure: &dyn TextMeasure, wrap: Option<f32>) -> MeasuredLine {
        let xs = measure.char_xs(&display.text);
        let bounds: Vec<usize> = display
            .text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(display.text.len()))
            .collect();
        let rows = match wrap {
            Some(w) => wrap_rows(&display.text, &xs, w),
            None => vec![0],
        };
        MeasuredLine {
            display,
            xs,
            bounds,
            rows,
        }
    }

    fn ci(&self, disp: usize) -> usize {
        self.bounds
            .partition_point(|b| *b < disp)
            .min(self.xs.len() - 1)
    }

    /// x of a display offset (unwrapped).
    pub fn x_of(&self, disp: usize) -> f32 {
        self.xs[self.ci(disp)]
    }

    /// The wrap row containing a display offset; `assoc < 0` puts a row
    /// boundary on the previous row.
    pub fn row_of(&self, disp: usize, assoc: i32) -> usize {
        let mut r = self.rows.partition_point(|s| *s <= disp).saturating_sub(1);
        if assoc < 0 && r > 0 && self.rows[r] == disp {
            r -= 1;
        }
        r
    }

    /// Row-relative x of a display offset.
    pub fn row_x(&self, disp: usize, row: usize) -> f32 {
        self.x_of(disp) - self.x_of(self.rows[row])
    }

    pub fn row_range(&self, row: usize) -> (usize, usize) {
        let end = self
            .rows
            .get(row + 1)
            .copied()
            .unwrap_or(self.display.text.len());
        (self.rows[row], end)
    }

    /// The display offset nearest `x` in a row.
    pub fn disp_at_x(&self, row: usize, x: f32) -> usize {
        let (from, to) = self.row_range(row);
        let base = self.x_of(from);
        let (cf, ct) = (self.ci(from), self.ci(to));
        let mut best = cf;
        for c in cf..=ct {
            let cx = self.xs[c] - base;
            if cx <= x {
                best = c;
            } else {
                if c > cf && x - (self.xs[c - 1] - base) > cx - x {
                    best = c;
                }
                break;
            }
        }
        let mut d = self.bounds[best];
        if row + 1 < self.rows.len() && d == to && to > from {
            d = self.bounds[self.ci(to).saturating_sub(1).max(cf)];
        }
        d
    }

    pub fn width(&self) -> f32 {
        self.xs.last().copied().unwrap_or(0.0)
    }
}

/// Estimated rows of a line before it is measured (CodeMirror's
/// `HeightOracle.heightForLine`).
pub fn estimate_rows(chars: usize, chars_per_row: usize) -> usize {
    let per = chars_per_row.max(1);
    1 + chars
        .saturating_sub(per)
        .div_ceil(per.saturating_sub(5).max(1))
}

/// The layout of one editor: config, height map and measured lines.
pub struct Layout {
    pub config: LayoutConfig,
    pub heights: HeightMap,
    measured: HashMap<usize, MeasuredLine>,
    /// Fold ranges as `(first line, last line)`, sorted.
    folded: Vec<(usize, usize)>,
    widgets: Vec<BlockWidget>,
}

impl Layout {
    pub fn new(config: LayoutConfig, state: &EditorState) -> Layout {
        let mut l = Layout {
            config,
            heights: HeightMap::default(),
            measured: HashMap::new(),
            folded: Vec::new(),
            widgets: Vec::new(),
        };
        l.rebuild(state, Vec::new());
        l
    }

    fn chars_per_row(&self) -> usize {
        (self.config.wrap_width / self.config.char_width.max(1.0))
            .floor()
            .max(1.0) as usize
    }

    fn estimate(&self, state: &EditorState, n: usize) -> f64 {
        let lh = self.config.line_height as f64;
        if !self.config.wrap {
            return lh;
        }
        let line = state.doc.line(n);
        let chars = state.doc.slice(line.from, line.to).chars().count();
        estimate_rows(chars, self.chars_per_row()) as f64 * lh
    }

    /// Folded line ranges from the fold state.
    fn fold_lines(state: &EditorState) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = state
            .folds
            .ranges()
            .iter()
            .map(|&(from, to)| (state.doc.line_at(from).number, state.doc.line_at(to).number))
            .filter(|(a, b)| b > a)
            .collect();
        out.sort_unstable();
        out
    }

    fn hidden(&self, n: usize) -> bool {
        self.folded.iter().any(|&(a, b)| n > a && n <= b)
            || self
                .widgets
                .iter()
                .any(|w| w.replace_to.is_some_and(|to| n >= w.line && n <= to))
    }

    /// The line `n` displays its fold placeholder.
    pub fn fold_end(&self, n: usize) -> Option<usize> {
        self.folded
            .iter()
            .filter(|(a, _)| *a == n)
            .map(|(_, b)| *b)
            .max()
    }

    fn widget_height(&self, n: usize) -> f64 {
        self.widgets
            .iter()
            .filter(|w| w.line == n)
            .map(|w| w.height as f64)
            .sum()
    }

    fn line_height_total(&self, state: &EditorState, n: usize) -> f64 {
        let own = if self.hidden(n) {
            0.0
        } else if let Some(m) = self.measured.get(&n) {
            m.rows.len() as f64 * self.config.line_height as f64
        } else {
            self.estimate(state, n)
        };
        own + self.widget_height(n)
    }

    /// Rebuild the height map (doc, fold or widget changes; config
    /// changes). Measured lines are dropped.
    pub fn rebuild(&mut self, state: &EditorState, widgets: Vec<BlockWidget>) {
        self.folded = Layout::fold_lines(state);
        self.widgets = widgets;
        self.measured.clear();
        let heights: Vec<f64> = (1..=state.doc.lines())
            .map(|n| self.line_height_total(state, n))
            .collect();
        self.heights = HeightMap::new(heights);
    }

    pub fn widgets(&self) -> &[BlockWidget] {
        &self.widgets
    }

    pub fn is_hidden(&self, n: usize) -> bool {
        self.hidden(n)
    }

    /// Measure line `n` with real text widths, updating its height.
    /// Returns the height change.
    pub fn measure(&mut self, state: &EditorState, n: usize, measure: &dyn TextMeasure) -> f64 {
        let text = state.doc.line_text(n);
        let folded = self.fold_end(n).is_some();
        let display = display_line(
            &text,
            self.config.tab_size,
            self.config.special_chars,
            folded,
        );
        let wrap = self.config.wrap.then_some(self.config.wrap_width);
        let m = MeasuredLine::new(display, measure, wrap);
        let rows = m.rows.len();
        self.measured.insert(n, m);
        let h = if self.hidden(n) {
            0.0
        } else {
            rows as f64 * self.config.line_height as f64
        } + self.widget_height(n);
        self.heights.set(n, h, true)
    }

    /// After lines `old_from..=old_to` were replaced and later lines moved
    /// by `delta`, drop their measurements and shift the rest.
    pub fn forget_measured_from(&mut self, old_from: usize, old_to: usize, delta: isize) {
        let old = std::mem::take(&mut self.measured);
        for (n, m) in old {
            if n < old_from {
                self.measured.insert(n, m);
            } else if n > old_to {
                self.measured.insert(n.wrapping_add_signed(delta), m);
            }
        }
    }

    /// Widest measured line, for horizontal scrolling.
    pub fn max_measured_width(&self) -> f32 {
        self.measured
            .values()
            .map(MeasuredLine::width)
            .fold(0.0, f32::max)
    }

    pub fn measured(&self, n: usize) -> Option<&MeasuredLine> {
        self.measured.get(&n)
    }

    /// Top of line `n`'s text (after any widget above it), in content
    /// coordinates (padding included).
    pub fn line_top(&self, n: usize) -> f64 {
        self.config.padding_top as f64 + self.heights.top(n) + self.widget_height(n)
    }

    /// Top of the block (widgets included) for line `n`.
    pub fn block_top(&self, n: usize) -> f64 {
        self.config.padding_top as f64 + self.heights.top(n)
    }

    /// Full content height (`.cm-content` box).
    pub fn content_height(&self) -> f64 {
        self.config.padding_top as f64 + self.heights.total() + self.config.padding_bottom as f64
    }

    /// The visible line (not hidden) covering content `y`.
    pub fn line_at_y(&self, y: f64) -> usize {
        let mut n = self
            .heights
            .line_at_height(y - self.config.padding_top as f64);
        while n > 1 && self.hidden(n) {
            n -= 1;
        }
        n
    }

    /// The last line of the visual block starting at `n` (fold end).
    pub fn block_end_line(&self, n: usize) -> usize {
        self.fold_end(n).unwrap_or(n)
    }

    /// The first line of the visual block containing line `n`.
    pub fn block_start_line(&self, n: usize) -> usize {
        self.folded
            .iter()
            .filter(|&&(a, b)| n > a && n <= b)
            .map(|&(a, _)| a)
            .min()
            .unwrap_or(n)
    }
}

/// [`ViewQueries`] over a [`Layout`], measuring lines on demand.
pub struct LayoutView<'a> {
    pub state: &'a EditorState,
    pub layout: &'a RefCell<Layout>,
    pub measure: &'a dyn TextMeasure,
    /// Visible scroller box in content coordinates, when the editor
    /// scrolls itself.
    pub scroll: Option<(f32, f32)>,
    pub atomic: Vec<(usize, usize)>,
}

impl LayoutView<'_> {
    fn ensure(&self, n: usize) {
        let has = self.layout.borrow().measured(n).is_some();
        if !has {
            self.layout
                .borrow_mut()
                .measure(self.state, n, self.measure);
        }
    }

    /// Line-relative display info for a position: (line no, display
    /// offset, row).
    fn locate(&self, pos: usize, assoc: i32) -> (usize, usize, usize) {
        let line = self.state.doc.line_at(pos);
        self.ensure(line.number);
        let l = self.layout.borrow();
        let m = l.measured(line.number).expect("measured");
        let disp = m.display.disp_of(pos - line.from, assoc);
        (line.number, disp, m.row_of(disp, assoc))
    }

    /// Row top in content coordinates.
    fn row_top(&self, n: usize, row: usize) -> f32 {
        let l = self.layout.borrow();
        (l.line_top(n) + row as f64 * l.config.line_height as f64) as f32
    }

    /// The x of a position within its row.
    pub fn x_at(&self, pos: usize, assoc: i32) -> f32 {
        let (n, disp, row) = self.locate(pos, assoc);
        let l = self.layout.borrow();
        l.measured(n).expect("measured").row_x(disp, row)
    }

    /// The document position at content `(x, y)` (`posAtCoords`), `x`
    /// relative to the line's text start.
    pub fn pos_at(&self, x: f32, y: f32) -> usize {
        let (n, top) = {
            let l = self.layout.borrow();
            let n = l.line_at_y(y as f64);
            (n, l.line_top(n))
        };
        self.ensure(n);
        let l = self.layout.borrow();
        let m = l.measured(n).expect("measured");
        let lh = l.config.line_height as f64;
        let row = (((y as f64 - top) / lh).floor().max(0.0) as usize).min(m.rows.len() - 1);
        let disp = m.disp_at_x(row, x);
        let line = self.state.doc.line(n);
        if let Some((p, _)) = m.display.placeholder()
            && disp >= p
        {
            let end = l.block_end_line(n);
            return self.state.doc.line(end).to;
        }
        line.from + m.display.doc_of(disp)
    }

    /// Top and bottom of the row a position sits on.
    pub fn row_box(&self, pos: usize, assoc: i32) -> (f32, f32) {
        let (n, _, row) = self.locate(pos, assoc);
        let top = self.row_top(n, row);
        (top, top + self.layout.borrow().config.line_height)
    }
}

impl ViewQueries for LayoutView<'_> {
    fn state(&self) -> &EditorState {
        self.state
    }

    fn line_block_at(&self, pos: usize) -> LineBlock {
        let doc = &self.state.doc;
        let n = doc.line_at(pos).number;
        let l = self.layout.borrow();
        let first = l.block_start_line(n);
        let last = l.block_end_line(first);
        let top = l.line_top(first) as f32;
        let mut height = 0.0;
        for k in first..=last {
            height += l.heights.height(k);
        }
        height -= l
            .widgets
            .iter()
            .filter(|w| w.line == first)
            .map(|w| w.height as f64)
            .sum::<f64>();
        LineBlock {
            from: doc.line(first).from,
            to: doc.line(last).to,
            top,
            height: height as f32,
        }
    }

    fn move_vertically(
        &self,
        start: SelectionRange,
        forward: bool,
        distance: Option<f32>,
    ) -> SelectionRange {
        let head = start.head();
        let doc_len = self.state.doc.len();
        if head == if forward { doc_len } else { 0 } {
            return SelectionRange::cursor_assoc(head, start.assoc(), None);
        }
        let assoc = if start.assoc() == 0 {
            if forward { 1 } else { -1 }
        } else {
            start.assoc()
        };
        let goal = start.goal_column.unwrap_or_else(|| self.x_at(head, assoc));
        let (top, bottom) = self.row_box(head, assoc);
        let lh = self.layout.borrow().config.line_height;
        let dist = distance.unwrap_or(lh / 2.0);
        let y = if forward { bottom + dist } else { top - dist };
        let content_h = self.layout.borrow().content_height() as f32;
        let pad_top = self.layout.borrow().config.padding_top;
        if y < pad_top {
            return SelectionRange::cursor_assoc(0, 1, Some(goal));
        }
        if y > content_h - self.layout.borrow().config.padding_bottom {
            return SelectionRange::cursor_assoc(doc_len, -1, Some(goal));
        }
        let pos = self.pos_at(goal, y);
        let (_, _, row) = self.locate(pos, -1);
        let a = if row > 0 && self.locate(pos, 1).2 != row {
            -1
        } else {
            1
        };
        SelectionRange::cursor_assoc(pos, a, Some(goal))
    }

    fn move_to_line_boundary(
        &self,
        start: SelectionRange,
        forward: bool,
        include_wrap: bool,
    ) -> SelectionRange {
        let head = start.head();
        let block = self.line_block_at(head);
        if include_wrap && self.layout.borrow().config.wrap {
            let (n, disp, row) = self.locate(head, start.assoc());
            let l = self.layout.borrow();
            let m = l.measured(n).expect("measured");
            let (rf, rt) = m.row_range(row);
            let line = self.state.doc.line(n);
            if forward && row + 1 < m.rows.len() {
                let to = line.from + m.display.doc_of(rt);
                let _ = disp;
                return SelectionRange::cursor_assoc(to, -1, None);
            }
            if !forward && row > 0 {
                return SelectionRange::cursor_assoc(line.from + m.display.doc_of(rf), 1, None);
            }
        }
        SelectionRange::cursor_assoc(
            if forward { block.to } else { block.from },
            if forward { -1 } else { 1 },
            None,
        )
    }

    fn page_height(&self) -> f32 {
        let l = self.layout.borrow();
        (l.config.viewport_height - 5.0).max(l.config.line_height)
    }

    fn default_line_height(&self) -> f32 {
        self.layout.borrow().config.line_height
    }

    fn line_wrapping(&self) -> bool {
        self.layout.borrow().config.wrap
    }

    fn coords_at_pos(&self, pos: usize, side: i32) -> Option<(f32, f32)> {
        Some(self.row_box(pos, side))
    }

    fn scroll_box(&self) -> Option<(f32, f32)> {
        self.scroll
    }

    fn atomic_ranges(&self) -> Vec<(usize, usize)> {
        self.atomic.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Effect, StateConfig, TransactionSpec};

    #[test]
    fn display_lines_map_both_ways() {
        let d = display_line("a\tb\u{200b}c", 4, true, false);
        assert_eq!(d.text, "a   b\u{2022}c");
        assert_eq!(d.disp_of(1, 1), 1);
        assert_eq!(d.disp_of(2, 1), 4);
        assert_eq!(d.doc_of(2), 1);
        assert_eq!(d.doc_of(3), 2);
        assert_eq!(d.doc_of(4), 2);
        assert_eq!(d.disp_of(3, 1), 5);
        assert_eq!(d.doc_of(d.text.len()), d.doc_len);
        let f = display_line("x {", 4, true, true);
        assert_eq!(f.placeholder(), Some((3, 6)));
        assert_eq!(f.text_end(), 3);
    }

    #[test]
    fn wraps_at_spaces_else_anywhere() {
        let m = MonoMeasure(10.0);
        let text = "aaa bbb ccc";
        assert_eq!(wrap_rows(text, &m.char_xs(text), 75.0), vec![0, 4]);
        assert_eq!(wrap_rows(text, &m.char_xs(text), 45.0), vec![0, 4, 8]);
        let long = "abcdefghij";
        assert_eq!(wrap_rows(long, &m.char_xs(long), 35.0), vec![0, 3, 6, 9]);
        assert_eq!(wrap_rows("ab", &m.char_xs("ab"), 50.0), vec![0]);
    }

    #[test]
    fn height_map_tracks_lines() {
        let mut h = HeightMap::new(vec![10.0, 20.0, 0.0, 10.0]);
        assert_eq!(h.top(3), 30.0);
        assert_eq!(h.total(), 40.0);
        assert_eq!(h.line_at_height(25.0), 2);
        assert_eq!(h.line_at_height(30.0), 4);
        h.set(2, 10.0, true);
        assert_eq!(h.top(4), 20.0);
        h.splice(2, 2, vec![5.0, 5.0, 5.0]);
        assert_eq!(h.len(), 5);
        assert_eq!(h.total(), 35.0);
        assert_eq!(estimate_rows(10, 20), 1);
        assert_eq!(estimate_rows(50, 20), 3);
    }

    #[test]
    fn layout_view_moves_and_folds() {
        let s = EditorState::create("abc\n\tdef\nghi", StateConfig::default());
        let layout = RefCell::new(Layout::new(
            LayoutConfig {
                line_height: 10.0,
                ..Default::default()
            },
            &s,
        ));
        let m = MonoMeasure(5.0);
        let v = LayoutView {
            state: &s,
            layout: &layout,
            measure: &m,
            scroll: None,
            atomic: Vec::new(),
        };
        let down = v.move_vertically(SelectionRange::cursor(2), true, None);
        assert_eq!(down.head(), 5);
        assert_eq!(v.pos_at(0.0, 8.0 + 25.0), 9);
        assert_eq!(v.line_block_at(5).top, 18.0);
        let s2 = s
            .update(TransactionSpec::default().effect(Effect::Fold { from: 3, to: 8 }))
            .state()
            .clone();
        layout.borrow_mut().rebuild(&s2, Vec::new());
        let v2 = LayoutView {
            state: &s2,
            layout: &layout,
            measure: &m,
            scroll: None,
            atomic: Vec::new(),
        };
        let b = v2.line_block_at(5);
        assert_eq!((b.from, b.to), (0, 8));
        assert_eq!(v2.line_block_at(10).top, 18.0);
    }
}
