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
use std::cmp::Ordering;
use std::rc::Rc;

use sum_tree::{Bias, ContextLessSummary, Item, SeekTarget, SumTree};

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
/// follow CSS `break-spaces` + `overflow-wrap: anywhere`: at the last line
/// break opportunity (after a run of spaces, or where UAX #14 allows one, as
/// between `}` and `[`) when the next piece does not fit, else anywhere.
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
    let allowed: std::collections::HashSet<usize> = unicode_linebreak::linebreaks(text)
        .filter_map(|(byte, _)| bounds.binary_search(&byte).ok())
        .collect();
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
        if chars[ci] == ' ' || allowed.contains(&(ci + 1)) {
            last_break = Some(ci + 1);
        }
        ci += 1;
    }
    rows
}

/// Seek target: the first `n` lines of a per-line tree.
struct Lines(usize);

/// Replace items `from..to` (0-based lines) of a per-line tree with `items`.
fn splice_lines<T, S>(tree: &mut SumTree<T>, from: usize, to: usize, items: Vec<T>) -> Option<T>
where
    T: Item<Summary = S>,
    S: ContextLessSummary,
    for<'a> Lines: SeekTarget<'a, S, S>,
{
    let mut cursor = tree.cursor::<S>(());
    let mut out = cursor.slice(&Lines(from), Bias::Right);
    let old = cursor.item().cloned();
    cursor.seek_forward(&Lines(to), Bias::Right);
    out.extend(items, ());
    out.append(cursor.suffix(), ());
    drop(cursor);
    *tree = out;
    old
}

#[derive(Clone, Debug)]
struct LineHeight {
    height: f64,
    measured: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct HeightSummary {
    lines: usize,
    height: f64,
}

impl ContextLessSummary for HeightSummary {
    fn zero() -> Self {
        HeightSummary::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.lines += other.lines;
        self.height += other.height;
    }
}

impl Item for LineHeight {
    type Summary = HeightSummary;

    fn summary(&self, _: ()) -> HeightSummary {
        HeightSummary {
            lines: 1,
            height: self.height,
        }
    }
}

impl SeekTarget<'_, HeightSummary, HeightSummary> for Lines {
    fn cmp(&self, at: &HeightSummary, _: ()) -> Ordering {
        Ord::cmp(&self.0, &at.lines)
    }
}

struct Height(f64);

impl SeekTarget<'_, HeightSummary, HeightSummary> for Height {
    fn cmp(&self, at: &HeightSummary, _: ()) -> Ordering {
        self.0.partial_cmp(&at.height).unwrap_or(Ordering::Equal)
    }
}

/// Per-line heights (each line's own rows plus any block widgets above it;
/// zero for lines hidden by folds), estimated until measured.
#[derive(Clone, Debug, Default)]
pub struct HeightMap {
    tree: SumTree<LineHeight>,
}

impl HeightMap {
    fn estimated(heights: impl IntoIterator<Item = f64>) -> impl Iterator<Item = LineHeight> {
        heights.into_iter().map(|height| LineHeight {
            height,
            measured: false,
        })
    }

    fn line(&self, n: usize) -> Option<&LineHeight> {
        self.tree
            .find::<HeightSummary, _>((), &Lines(n.checked_sub(1)?), Bias::Right)
            .2
    }

    pub fn new(heights: Vec<f64>) -> HeightMap {
        HeightMap {
            tree: SumTree::from_iter(HeightMap::estimated(heights), ()),
        }
    }

    pub fn len(&self) -> usize {
        self.tree.summary().lines
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Top of 1-based line `n` (relative to the first line).
    pub fn top(&self, n: usize) -> f64 {
        let before = Lines(n.saturating_sub(1));
        self.tree
            .find::<HeightSummary, _>((), &before, Bias::Right)
            .0
            .height
    }

    pub fn height(&self, n: usize) -> f64 {
        self.line(n).map_or(0.0, |l| l.height)
    }

    pub fn total(&self) -> f64 {
        self.tree.summary().height
    }

    /// Set line `n`'s height; returns the change.
    pub fn set(&mut self, n: usize, h: f64, measured: bool) -> f64 {
        let line = LineHeight {
            height: h,
            measured,
        };
        let old = splice_lines(&mut self.tree, n - 1, n, vec![line]).expect("line in range");
        h - old.height
    }

    pub fn is_measured(&self, n: usize) -> bool {
        self.line(n).is_some_and(|l| l.measured)
    }

    /// The 1-based line covering vertical offset `y` (clamped).
    pub fn line_at_height(&self, y: f64) -> usize {
        if self.is_empty() {
            return 1;
        }
        let (above, _, line) =
            self.tree
                .find::<HeightSummary, _>((), &Height(y.max(0.0)), Bias::Right);
        if line.is_some() {
            above.lines + 1
        } else {
            self.len()
        }
    }

    /// Splice heights for a line replacement (1-based `from`, `old`
    /// lines replaced by `new` estimates).
    pub fn splice(&mut self, from: usize, old: usize, new: Vec<f64>) {
        let i = from - 1;
        let new = HeightMap::estimated(new).collect();
        splice_lines(&mut self.tree, i, i + old, new);
    }
}

/// A line's measurement, if it has one.
#[derive(Clone, Debug, Default)]
struct Measured(Option<Rc<MeasuredLine>>);

#[derive(Clone, Copy, Debug, Default)]
struct MeasuredSummary {
    lines: usize,
    max_width: f32,
}

impl ContextLessSummary for MeasuredSummary {
    fn zero() -> Self {
        MeasuredSummary::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.lines += other.lines;
        self.max_width = self.max_width.max(other.max_width);
    }
}

impl Item for Measured {
    type Summary = MeasuredSummary;

    fn summary(&self, _: ()) -> MeasuredSummary {
        MeasuredSummary {
            lines: 1,
            max_width: self.0.as_ref().map_or(0.0, |m| m.width()),
        }
    }
}

impl SeekTarget<'_, MeasuredSummary, MeasuredSummary> for Lines {
    fn cmp(&self, at: &MeasuredSummary, _: ()) -> Ordering {
        Ord::cmp(&self.0, &at.lines)
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
    /// `.cm-line` right padding.
    pub line_padding_right: f32,
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
            line_padding_right: 2.0,
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
/// `HeightOracle.heightForLine`, with its fractional `lineLength`).
pub fn estimate_rows(chars: usize, line_length: f32) -> usize {
    let extra = ((chars as f32 - line_length) / (line_length - 5.).max(1.)).ceil();
    1 + extra.max(0.) as usize
}

/// The layout of one editor: config, height map and measured lines.
pub struct Layout {
    pub config: LayoutConfig,
    pub heights: HeightMap,
    /// Item `n - 1` is line n's measurement.
    measured: SumTree<Measured>,
    /// Fold ranges as `(first line, last line)`, sorted.
    folded: Vec<(usize, usize)>,
    widgets: Vec<BlockWidget>,
}

impl Layout {
    pub fn new(config: LayoutConfig, state: &EditorState) -> Layout {
        let mut l = Layout {
            config,
            heights: HeightMap::default(),
            measured: SumTree::default(),
            folded: Vec::new(),
            widgets: Vec::new(),
        };
        l.rebuild(state, Vec::new());
        l
    }

    /// `HeightOracle.lineLength`: the content width (line padding included) in characters.
    pub(crate) fn line_length(&self) -> f32 {
        let c = &self.config;
        ((c.wrap_width + c.line_padding_left + c.line_padding_right) / c.char_width.max(1.0))
            .max(5.0)
    }

    fn estimate(&self, state: &EditorState, n: usize) -> f64 {
        let lh = self.config.line_height as f64;
        if !self.config.wrap {
            return lh;
        }
        let line = state.doc.line(n);
        let chars = state.doc.slice(line.from, line.to).chars().count();
        estimate_rows(chars, self.line_length()) as f64 * lh
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
        } else if let Some(m) = self.measured(n) {
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
        self.measured = SumTree::default();
        let heights: Vec<f64> = (1..=state.doc.lines())
            .map(|n| self.line_height_total(state, n))
            .collect();
        self.measured =
            SumTree::from_iter(std::iter::repeat_n(Measured::default(), heights.len()), ());
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
        splice_lines(
            &mut self.measured,
            n - 1,
            n,
            vec![Measured(Some(Rc::new(m)))],
        );
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
        let new = (old_to - old_from + 1).saturating_add_signed(delta);
        let blank = std::iter::repeat_n(Measured::default(), new).collect();
        splice_lines(&mut self.measured, old_from - 1, old_to, blank);
    }

    /// Widest measured line, for horizontal scrolling.
    pub fn max_measured_width(&self) -> f32 {
        self.measured.summary().max_width
    }

    pub fn measured(&self, n: usize) -> Option<&MeasuredLine> {
        self.measured
            .find::<MeasuredSummary, _>((), &Lines(n.checked_sub(1)?), Bias::Right)
            .2?
            .0
            .as_deref()
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
        // UAX #14 lets a line break between close and open punctuation.
        let code = "n }[]";
        assert_eq!(wrap_rows(code, &m.char_xs(code), 35.0), vec![0, 3]);
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
        assert_eq!(estimate_rows(10, 20.), 1);
        assert_eq!(estimate_rows(50, 20.), 3);
        // A fractional line length is kept, as CodeMirror's oracle does.
        assert_eq!(estimate_rows(60, 52.3), 2);
        assert_eq!(estimate_rows(100, 52.3), 3);
    }

    /// xorshift64, so the randomized checks need no dependency.
    fn rng(mut x: u64) -> impl FnMut(usize) -> usize {
        move |n| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n.max(1) as u64) as usize
        }
    }

    #[test]
    fn height_map_matches_a_vec_under_random_edits() {
        let mut r = rng(0x9e37_79b9_7f4a_7c15);
        let mut naive = vec![10.0; 40];
        let mut h = HeightMap::new(naive.clone());
        for _ in 0..3000 {
            let len = naive.len();
            if r(3) == 0 && len > 0 {
                let n = r(len) + 1;
                let v = (r(4) * 10) as f64;
                assert_eq!(h.set(n, v, true), v - naive[n - 1]);
                naive[n - 1] = v;
            } else {
                let from = r(len + 1) + 1;
                let old = r(4).min(len + 1 - from);
                let new: Vec<f64> = (0..r(4)).map(|_| (r(3) * 10) as f64).collect();
                naive.splice(from - 1..from - 1 + old, new.clone());
                h.splice(from, old, new);
            }
            assert_eq!(h.len(), naive.len());
            assert_eq!(h.total(), naive.iter().sum::<f64>());
            for n in 1..=naive.len() + 1 {
                assert_eq!(
                    h.top(n),
                    naive[..(n - 1).min(naive.len())].iter().sum::<f64>()
                );
                assert_eq!(h.height(n), naive.get(n - 1).copied().unwrap_or(0.0));
            }
            let y = r(naive.len() * 20 + 1) as f64;
            let mut top = 0.0;
            let want = naive
                .iter()
                .position(|v| {
                    top += v;
                    top > y
                })
                .map_or(naive.len().max(1), |i| i + 1);
            assert_eq!(h.line_at_height(y), want, "y={y} {naive:?}");
        }
    }

    /// `cargo test --release -p recur-editor -- --ignored --nocapture height_map_splice_timing`
    #[test]
    #[ignore]
    fn height_map_splice_timing() {
        let mut h = HeightMap::new(vec![18.0; 1_000_000]);
        let mut r = rng(1);
        let t = std::time::Instant::now();
        for _ in 0..1000 {
            let n = r(h.len()) + 1;
            h.splice(n, 1, vec![18.0, 18.0]);
            std::hint::black_box((h.top(n), h.line_at_height(n as f64 * 9.0)));
        }
        println!("HeightMap 1M lines: {:?} per newline", t.elapsed() / 1000);
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
