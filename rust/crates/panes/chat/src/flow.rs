//! Inline text with atomic chips, as the user bubble and the composer lay them out: a
//! `white-space: pre-wrap; overflow-wrap: anywhere` block whose `inline-flex` chips
//! (`vertical-align: middle`) take part in line breaking and make their line taller.
//! `recur_ui::Text` has no atomic inline boxes, so this lays the lines out with the same
//! metrics (`line_metrics`, UAX #14 breaks, trailing spaces hanging) and paints each line's
//! text runs as nowrap `recur_ui` text and each chip as its own element at its box.
//!
//! Content is one string where every chip is [`ATOM`]; chips are given in order with their
//! margin-box sizes.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, FontFeatures, GlobalElementId,
    Hsla, InspectorElementId, IntoElement, LayoutId, Length, ParentElement, Pixels, SharedString,
    Style, Styled, TextRun, Window, div, px,
};
use recur_ui::exact;
use recur_ui::font_metrics::line_gap;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, LineMetrics, TextStyled, WhiteSpace, line_metrics, text};

/// The placeholder a chip occupies in the content (U+FFFC, line-break class CB).
pub const ATOM: char = '\u{FFFC}';
pub const ATOM_LEN: usize = ATOM.len_utf8();

/// The block's text style.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowStyle {
    pub family: Family,
    pub size: f32,
    pub weight: f32,
    /// Unitless `line-height`.
    pub leading: f32,
    pub color: Hsla,
}

/// One chip: its element and margin-box size. A chip wider than the line shrinks to it
/// (`max-width: 100%`).
pub struct Atom {
    pub el: AnyElement,
    pub width: f32,
    pub height: f32,
    /// What a selection across the atom copies (a chip's label, as WebKit copies its text;
    /// empty for an image).
    pub copy: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlowLine {
    /// Content byte range (no trailing newline).
    pub range: Range<usize>,
    pub top: f32,
    pub height: f32,
    /// Top of the line box to the baseline.
    pub baseline: f32,
    /// Left edge of the line's first byte in paragraph coordinates.
    left: f32,
    para: usize,
}

#[derive(Clone, Debug)]
struct Para {
    range: Range<usize>,
    /// `(byte offset in the paragraph, x)` for every char start and the end.
    edges: Rc<Vec<(usize, f32)>>,
}

impl Para {
    fn x_at(&self, byte: usize) -> f32 {
        match self.edges.binary_search_by_key(&byte, |e| e.0) {
            Ok(i) => self.edges[i].1,
            Err(i) => self.edges.get(i).map_or(0., |e| e.1),
        }
    }

    /// Width of `r` with trailing spaces hanging (pre-wrap).
    fn width(&self, text: &str, r: Range<usize>) -> f32 {
        let end = r.start + text[r.clone()].trim_end_matches([' ', '\t']).len();
        self.x_at(end) - self.x_at(r.start)
    }
}

/// The laid-out lines.
#[derive(Clone, Debug)]
pub struct FlowLayout {
    pub width: f32,
    pub content: String,
    pub lines: Vec<FlowLine>,
    /// Widest line (for `fit-content`).
    pub max_line: f32,
    pub height: f32,
    /// The strut (one line of plain text).
    pub strut: LineMetrics,
    paras: Vec<Para>,
    /// Each atom's byte offset and margin-box size, in order.
    atoms: Vec<(usize, f32, f32)>,
    xh: f32,
}

fn font_extent(window: &Window, s: &FlowStyle) -> (f32, f32, f32, f32) {
    let ts = window.text_system();
    let id = ts.resolve_font(&s.family.font(s.weight, false, FontFeatures::default()));
    (
        ts.ascent(id, px(s.size)).into(),
        -f32::from(ts.descent(id, px(s.size))),
        line_gap(&s.family, s.size),
        ts.x_height(id, px(s.size)).into(),
    )
}

fn shape_width(text: &str, s: &FlowStyle, window: &Window) -> (f32, Vec<(usize, f32)>) {
    let font = s.family.font(s.weight, false, FontFeatures::default());
    let shaped = window.text_system().shape_line(
        text.to_string().into(),
        px(s.size),
        &[TextRun {
            len: text.len(),
            font,
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let mut xs: Vec<(usize, f32)> = shaped
        .runs
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| (g.index, f32::from(g.position.x))))
        .collect();
    xs.sort_by_key(|g| g.0);
    (f32::from(shaped.width), xs)
}

/// One paragraph's char edges, its atoms at their offsets in it.
fn shape_para(
    text: &str,
    atom_w: &dyn Fn(usize) -> f32,
    s: &FlowStyle,
    window: &Window,
) -> Vec<(usize, f32)> {
    let mut edges = Vec::new();
    let mut pen = 0.;
    let mut seg_start = 0;
    let flush = |from: usize, to: usize, pen: &mut f32, edges: &mut Vec<(usize, f32)>| {
        if from >= to {
            return;
        }
        let seg = &text[from..to];
        let (w, xs) = shape_width(seg, s, window);
        let mut gi = 0;
        for (bi, _) in seg.char_indices() {
            while gi < xs.len() && xs[gi].0 < bi {
                gi += 1;
            }
            edges.push((from + bi, *pen + xs.get(gi).map_or(w, |g| g.1)));
        }
        *pen += w;
    };
    for (bi, ch) in text.char_indices() {
        if ch == ATOM {
            flush(seg_start, bi, &mut pen, &mut edges);
            edges.push((bi, pen));
            pen += atom_w(bi);
            seg_start = bi + ATOM_LEN;
        }
    }
    flush(seg_start, text.len(), &mut pen, &mut edges);
    edges.push((text.len(), pen));
    edges
}

/// One line of a paragraph, in the paragraph's coordinates.
#[derive(Clone, Debug)]
struct ParaLine {
    range: Range<usize>,
    above: f32,
    below: f32,
    left: f32,
    width: f32,
}

/// A paragraph laid out at a width: what a keystroke in another paragraph reuses.
struct ParaFlow {
    edges: Rc<Vec<(usize, f32)>>,
    lines: Rc<Vec<ParaLine>>,
}

/// What a [`ParaFlow`] depends on: its text, its atoms' offsets and sizes, the style and the
/// width.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ParaKey {
    text: String,
    atoms: Vec<(usize, u32, u32)>,
    style: [u32; 4],
    family: Family,
    width: u32,
    scale: u32,
}

/// Paragraphs across frames: typing re-lays out the edited paragraph only. Byte-budgeted,
/// least recently used out first.
#[derive(Default)]
struct ParaCache {
    entries: HashMap<ParaKey, (Rc<ParaFlow>, usize, u64)>,
    order: BTreeMap<u64, ParaKey>,
    stamp: u64,
    bytes: usize,
}

const PARA_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

thread_local! {
    static PARAS: RefCell<ParaCache> = RefCell::default();
    /// Paragraphs laid out (cache misses), for frame-cost tests.
    static PARA_LAYOUTS: Cell<usize> = const { Cell::new(0) };
}

/// Paragraphs laid out on this thread so far (a test reads the difference).
pub fn para_layouts() -> usize {
    PARA_LAYOUTS.get()
}

impl ParaCache {
    fn get(&mut self, key: &ParaKey) -> Option<Rc<ParaFlow>> {
        self.stamp += 1;
        let stamp = self.stamp;
        let (p, _, at) = self.entries.get_mut(key)?;
        *at = stamp;
        Some(p.clone())
    }

    fn insert(&mut self, key: ParaKey, p: Rc<ParaFlow>) {
        let bytes = 96
            + key.text.len() * 2
            + p.edges.len() * 16
            + p.lines.len() * std::mem::size_of::<ParaLine>();
        self.stamp += 1;
        if let Some((_, b, _)) = self.entries.insert(key.clone(), (p, bytes, self.stamp)) {
            self.bytes -= b;
        }
        self.order.insert(self.stamp, key);
        self.bytes += bytes;
        while self.bytes > PARA_CACHE_MAX_BYTES && self.entries.len() > 1 {
            let Some((at, k)) = self.order.pop_first() else {
                break;
            };
            match self.entries.get(&k) {
                Some((_, b, used)) if *used == at => {
                    self.bytes -= *b;
                    self.entries.remove(&k);
                }
                Some((_, _, used)) => {
                    let used = *used;
                    self.order.insert(used, k);
                }
                None => {}
            }
        }
    }
}

/// `overflow-wrap: anywhere` lines (an atom stays whole).
fn break_lines(p: &Para, text: &str, max: f32, eps: f32) -> Vec<Range<usize>> {
    recur_ui::text::break_lines(text, max, eps, true, |r| p.width(text, r))
}

/// What a [`FlowLayout`] depends on (the color does not move anything).
#[derive(Clone, PartialEq, Eq, Hash)]
struct FlowKey {
    content: String,
    atoms: Vec<(u32, u32)>,
    family: Family,
    size: u32,
    weight: u32,
    leading: u32,
    width: u32,
    scale: u32,
}

/// Flow layouts across frames and measure passes: the composer and every visible user bubble
/// are measured several times a frame by Taffy, and a frame redraws them whenever anything
/// above them changes. Byte-budgeted, least recently used out first.
#[derive(Default)]
struct FlowCache {
    entries: HashMap<FlowKey, (FlowLayout, usize, u64)>,
    order: BTreeMap<u64, FlowKey>,
    stamp: u64,
    bytes: usize,
}

const FLOW_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

thread_local! {
    static FLOWS: RefCell<FlowCache> = RefCell::default();
}

impl FlowCache {
    fn get(&mut self, key: &FlowKey) -> Option<FlowLayout> {
        self.stamp += 1;
        let stamp = self.stamp;
        let (l, _, at) = self.entries.get_mut(key)?;
        // Lazy LRU: the order map keeps the old stamp until eviction finds it stale.
        *at = stamp;
        Some(l.clone())
    }

    fn insert(&mut self, key: FlowKey, l: &FlowLayout) {
        let bytes = 96
            + key.content.len() * 2
            + l.lines.len() * std::mem::size_of::<FlowLine>()
            + l.paras.iter().map(|p| p.edges.len() * 16).sum::<usize>();
        self.stamp += 1;
        if let Some((_, b, _)) = self
            .entries
            .insert(key.clone(), (l.clone(), bytes, self.stamp))
        {
            self.bytes -= b;
        }
        self.order.insert(self.stamp, key);
        self.bytes += bytes;
        while self.bytes > FLOW_CACHE_MAX_BYTES && self.entries.len() > 1 {
            let Some((at, k)) = self.order.pop_first() else {
                break;
            };
            match self.entries.get(&k) {
                Some((_, b, used)) if *used == at => {
                    self.bytes -= *b;
                    self.entries.remove(&k);
                }
                Some((_, _, used)) => {
                    let used = *used;
                    self.order.insert(used, k);
                }
                None => {}
            }
        }
    }
}

impl FlowLayout {
    /// Lays `content` out at `width` with chip sizes in order (cached across frames).
    pub fn compute(
        content: &str,
        atoms: &[(f32, f32)],
        s: &FlowStyle,
        width: f32,
        window: &Window,
    ) -> FlowLayout {
        let key = FlowKey {
            content: content.to_string(),
            atoms: atoms
                .iter()
                .map(|(w, h)| (w.to_bits(), h.to_bits()))
                .collect(),
            family: s.family.clone(),
            size: s.size.to_bits(),
            weight: s.weight.to_bits(),
            leading: s.leading.to_bits(),
            width: width.to_bits(),
            scale: window.scale_factor().to_bits(),
        };
        if let Some(hit) = FLOWS.with_borrow_mut(|c| c.get(&key)) {
            return hit;
        }
        let l = Self::compute_uncached(content, atoms, s, width, window);
        FLOWS.with_borrow_mut(|c| c.insert(key, &l));
        l
    }

    fn compute_uncached(
        content: &str,
        atoms: &[(f32, f32)],
        s: &FlowStyle,
        width: f32,
        window: &Window,
    ) -> FlowLayout {
        let mut at = Vec::new();
        let mut n = 0;
        for (bi, ch) in content.char_indices() {
            if ch == ATOM {
                let (w, h) = atoms.get(n).copied().unwrap_or((0., 0.));
                at.push((bi, w.min(width.max(0.)), h));
                n += 1;
            }
        }
        let (a, d, g, xh) = font_extent(window, s);
        let strut = line_metrics(a, d, g, s.size, LineHeight::Scale(s.leading));
        let xh = (xh * 64.).round() / 64.;
        let eps = 0.5 / window.scale_factor();
        let mut paras = Vec::new();
        let mut lines = Vec::new();
        let mut top = 0.;
        let mut max_line: f32 = 0.;
        let mut start = 0;
        for piece in content.split('\n') {
            let range = start..start + piece.len();
            let local: Vec<(usize, f32, f32)> = at
                .iter()
                .filter(|a| a.0 >= range.start && a.0 < range.end)
                .map(|&(b, w, h)| (b - range.start, w, h))
                .collect();
            let key = ParaKey {
                text: piece.to_string(),
                atoms: local
                    .iter()
                    .map(|&(b, w, h)| (b, w.to_bits(), h.to_bits()))
                    .collect(),
                style: [
                    s.size.to_bits(),
                    s.weight.to_bits(),
                    s.leading.to_bits(),
                    xh.to_bits(),
                ],
                family: s.family.clone(),
                width: width.to_bits(),
                scale: window.scale_factor().to_bits(),
            };
            let flow = match PARAS.with_borrow_mut(|c| c.get(&key)) {
                Some(f) => f,
                None => {
                    PARA_LAYOUTS.set(PARA_LAYOUTS.get() + 1);
                    let atom_w =
                        |byte: usize| local.iter().find(|a| a.0 == byte).map_or(0., |a| a.1);
                    let edges = Rc::new(shape_para(piece, &atom_w, s, window));
                    let p = Para {
                        range: 0..piece.len(),
                        edges: edges.clone(),
                    };
                    let mut pl = Vec::new();
                    for r in break_lines(&p, piece, width, eps) {
                        let (mut above, mut below) =
                            (strut.baseline, strut.height - strut.baseline);
                        for &(b, _, h) in &local {
                            if b >= r.start && b < r.end {
                                // `vertical-align: middle`: the box's midpoint at baseline −
                                // x-height/2.
                                let mid = xh / 2.;
                                above = above.max(mid + h / 2.);
                                below = below.max(h / 2. - mid);
                            }
                        }
                        pl.push(ParaLine {
                            width: p.width(piece, r.clone()),
                            left: p.x_at(r.start),
                            range: r,
                            above,
                            below,
                        });
                    }
                    let f = Rc::new(ParaFlow {
                        edges,
                        lines: Rc::new(pl),
                    });
                    PARAS.with_borrow_mut(|c| c.insert(key, f.clone()));
                    f
                }
            };
            let pi = paras.len();
            for l in flow.lines.iter() {
                max_line = max_line.max(l.width);
                lines.push(FlowLine {
                    range: range.start + l.range.start..range.start + l.range.end,
                    top,
                    height: l.above + l.below,
                    baseline: l.above,
                    left: l.left,
                    para: pi,
                });
                top += l.above + l.below;
            }
            paras.push(Para {
                range: range.clone(),
                edges: flow.edges.clone(),
            });
            start += piece.len() + 1;
        }
        FlowLayout {
            width,
            content: content.to_string(),
            lines,
            max_line,
            height: top,
            strut,
            paras,
            atoms: at,
            xh,
        }
    }

    fn line_of(&self, byte: usize) -> usize {
        let mut best = 0;
        for (i, l) in self.lines.iter().enumerate() {
            if byte >= l.range.start && byte <= l.range.end {
                best = i;
                if byte < l.range.end {
                    break;
                }
            }
        }
        best
    }

    /// The x of a byte on its line, relative to the block.
    fn x_on(&self, line: &FlowLine, byte: usize) -> f32 {
        let p = &self.paras[line.para];
        p.x_at(byte - p.range.start) - line.left
    }

    /// Caret box at `byte`: `(x, top, height)` of the text run on its line.
    pub fn caret(&self, byte: usize) -> (f32, f32, f32) {
        let Some(l) = self.lines.get(self.line_of(byte)) else {
            return (0., 0., self.strut.height);
        };
        let x = self.x_on(l, byte.min(l.range.end));
        (
            x,
            l.top + l.baseline - self.strut.baseline,
            self.strut.height,
        )
    }

    /// The caret byte nearest `(x, y)` (relative to the block). Atoms are whole.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        let Some(l) = self
            .lines
            .iter()
            .find(|l| y < l.top + l.height)
            .or(self.lines.last())
        else {
            return 0;
        };
        let p = &self.paras[l.para];
        let mut best = l.range.start;
        let mut best_d = f32::MAX;
        for &(b, ex) in p.edges.iter() {
            let abs = p.range.start + b;
            if abs < l.range.start || abs > l.range.end {
                continue;
            }
            let d = (ex - l.left - x).abs();
            if d < best_d {
                best_d = d;
                best = abs;
            }
        }
        best
    }

    /// The caret byte one line up or down from `byte` at the same x.
    pub fn vertical(&self, byte: usize, down: bool) -> Option<usize> {
        let i = self.line_of(byte);
        let j = if down { i + 1 } else { i.checked_sub(1)? };
        let l = self.lines.get(j)?;
        let (x, _, _) = self.caret(byte);
        Some(self.hit(x, l.top + l.height / 2.))
    }

    /// Selection rectangles for a byte range.
    pub fn rects(&self, r: Range<usize>) -> Vec<(f32, f32, f32, f32)> {
        let mut out = Vec::new();
        for l in &self.lines {
            let s = r.start.max(l.range.start);
            let e = r.end.min(l.range.end);
            if s >= e {
                continue;
            }
            let x0 = self.x_on(l, s);
            let x1 = self.x_on(l, e);
            out.push((x0, l.top, x1 - x0, l.height));
        }
        out
    }

    /// Each atom's margin box `(x, y, w, h)`, in order.
    pub fn atom_boxes(&self) -> Vec<(f32, f32, f32, f32)> {
        self.atoms
            .iter()
            .map(|&(b, w, h)| {
                let l = &self.lines[self.line_of(b)];
                let x = self.x_on(l, b);
                let y = l.top + l.baseline - self.xh / 2. - h / 2.;
                (x, y, w, h)
            })
            .collect()
    }

    /// Each line's text runs between atoms: `(byte range, x, y)` with `y` the run's line-box
    /// top.
    pub fn runs(&self) -> Vec<(Range<usize>, f32, f32)> {
        self.runs_in(None)
    }

    /// [`Self::runs`] on the lines that cross `band` (a y range of the block).
    pub fn runs_in(&self, band: Option<&Range<f32>>) -> Vec<(Range<usize>, f32, f32)> {
        let mut out = Vec::new();
        for l in &self.lines {
            if band.is_some_and(|b| l.top >= b.end || l.top + l.height <= b.start) {
                continue;
            }
            let y = l.top + l.baseline - self.strut.baseline;
            let mut s = l.range.start;
            let text = &self.content[l.range.clone()];
            let push = |a: usize, b: usize, out: &mut Vec<_>| {
                if a < b {
                    out.push((a..b, self.x_on(l, a), y));
                }
            };
            for (bi, ch) in text.char_indices() {
                if ch == ATOM {
                    push(s, l.range.start + bi, &mut out);
                    s = l.range.start + bi + ATOM_LEN;
                }
            }
            push(s, l.range.end, &mut out);
        }
        out
    }
}

/// The element.
pub struct Flow {
    id: ElementId,
    content: SharedString,
    atoms: Vec<Atom>,
    style: FlowStyle,
    /// Report the widest line as the width (`max-width` boxes that shrink to their text).
    fit: bool,
    /// A preserved space after the content widens the shrink-wrapped box (`pre-wrap`).
    trailing_space: bool,
    min_height: f32,
    out: Option<Rc<RefCell<Option<FlowLayout>>>>,
    /// The band of the block (its y range) that can show; only lines in it are placed.
    visible: Option<Range<f32>>,
    selectable: Option<u64>,
}

pub fn flow(
    id: impl Into<ElementId>,
    content: impl Into<SharedString>,
    atoms: Vec<Atom>,
    style: FlowStyle,
) -> Flow {
    Flow {
        id: id.into(),
        content: content.into(),
        atoms,
        style,
        fit: false,
        trailing_space: false,
        min_height: 0.,
        out: None,
        visible: None,
        selectable: None,
    }
}

impl Flow {
    pub fn fit_content(mut self) -> Self {
        self.fit = true;
        self
    }

    /// The template whitespace after the last part, which `white-space: pre-wrap` keeps in
    /// the box's max-content width.
    pub fn trailing_space(mut self, on: bool) -> Self {
        self.trailing_space = on;
        self
    }

    /// The layout of the last frame, for hosts that place a caret or hit-test.
    pub fn layout_into(mut self, out: Rc<RefCell<Option<FlowLayout>>>) -> Self {
        self.out = Some(out);
        self
    }

    /// Place only the lines that cross `band` (the block's y range a scroller can show): a long
    /// draft in the composer lays out and paints its visible lines, not all of them.
    pub fn visible(mut self, band: Range<f32>) -> Self {
        self.visible = Some(band);
        self
    }

    /// Selectable in the enclosing `recur_ui::selection` scope, under a stable `key`. A
    /// selection across a chip copies the chip's [`Atom::copy`].
    pub fn selectable(mut self, key: u64) -> Self {
        self.selectable = Some(key);
        self
    }

    /// One empty line still takes a line box (`p { min-height: 1.4em }`).
    pub fn min_height(mut self, h: f32) -> Self {
        self.min_height = h;
        self
    }
}

impl IntoElement for Flow {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// The width the children were placed at in the last frame, in the element's state.
#[derive(Clone, Default)]
struct Placed(Rc<Cell<Option<f32>>>);

pub struct FlowState {
    node: LayoutId,
    layout: Rc<RefCell<Option<FlowLayout>>>,
    placed: Placed,
    placed_at: Option<f32>,
    children: Vec<AnyElement>,
}

impl Flow {
    /// Each text run and chip at its line-box position, as an absolutely positioned child
    /// of the flow's box: the offsets reach layout exactly (`recur_ui::exact`), so they snap
    /// from the box's fractional origin as WebKit's inline boxes do.
    fn place(&mut self, layout: &FlowLayout, window: &Window) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let st = &self.style;
        let at = |x: f32, y: f32, el: AnyElement| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .flex()
                .flex_col()
                .child(exact::vgap(y.max(0.), window))
                .child(
                    div()
                        .flex()
                        .child(exact::hgap(x.max(0.), window))
                        .child(div().flex_none().child(el)),
                )
                .into_any_element()
        };
        let band = self.visible.clone();
        let shows = |y: f32, h: f32| band.as_ref().is_none_or(|b| y < b.end && y + h > b.start);
        for (r, x, y) in layout.runs_in(band.as_ref()) {
            if !shows(y, layout.strut.height) {
                continue;
            }
            let t = text(layout.content[r].to_string())
                .family(st.family.clone())
                .size(st.size)
                .weight(st.weight)
                .color(st.color)
                .line_height(LineHeight::Scale(st.leading))
                .white_space(WhiteSpace::PreWrap)
                .into_any_element();
            out.push(at(x, y, t));
        }
        for (atom, (x, y, w, h)) in self.atoms.iter_mut().zip(layout.atom_boxes()) {
            let el = std::mem::replace(&mut atom.el, gpui_kit::Empty.into_any_element());
            let _ = w;
            if shows(y, h) {
                out.push(at(x, y, el));
            }
        }
        out
    }
}

impl Element for Flow {
    type RequestLayoutState = FlowState;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, FlowState) {
        let layout = self
            .out
            .clone()
            .unwrap_or_else(|| Rc::new(RefCell::new(None)));
        let slot = layout.clone();
        let content = self.content.to_string();
        let sizes: Vec<(f32, f32)> = self.atoms.iter().map(|a| (a.width, a.height)).collect();
        let st = self.style.clone();
        let fit = self.fit;
        let trailing = self.trailing_space;
        let min_h = self.min_height;
        let placed: Placed = match id {
            Some(id) => window.with_element_state(id, |c: Option<Placed>, _| {
                let c = c.unwrap_or_default();
                (c.clone(), c)
            }),
            None => Placed::default(),
        };
        let leaf = exact::measured(Length::Auto, window, move |known, avail, window| {
            let w = known.width.map(f32::from).unwrap_or(match avail.width {
                AvailableSpace::Definite(w) => w.into(),
                // `overflow-wrap: anywhere`: a shrink-wrapped flow's min-content width is
                // its widest unbreakable piece, so a flex parent's cap can narrow it.
                AvailableSpace::MinContent if fit => 0.,
                _ => f32::MAX / 4.,
            });
            let l = FlowLayout::compute(&content, &sizes, &st, w, window);
            let extra = if trailing {
                shape_width(" ", &st, window).0
            } else {
                0.
            };
            let out = (
                if fit { (l.max_line + extra).min(w) } else { w },
                l.height.max(min_h),
            );
            *slot.borrow_mut() = Some(l);
            out
        });
        let mut nodes = vec![leaf];
        // Children are placed from the last frame's line layout; a new width lays the
        // window out again (see `prepaint`).
        let placed_at = placed.0.get();
        let mut children = match placed_at {
            Some(w) => {
                let content = self.content.to_string();
                let sizes: Vec<(f32, f32)> =
                    self.atoms.iter().map(|a| (a.width, a.height)).collect();
                let l = FlowLayout::compute(&content, &sizes, &self.style, w, window);
                self.place(&l, window)
            }
            None => Vec::new(),
        };
        nodes.extend(children.iter_mut().map(|c| c.request_layout(window, cx)));
        let style = Style {
            position: gpui_kit::Position::Relative,
            display: gpui_kit::Display::Flex,
            align_items: Some(gpui_kit::AlignItems::FlexStart),
            ..Default::default()
        };
        let node = window.request_layout(style, nodes, cx);
        (
            node,
            FlowState {
                node,
                layout,
                placed,
                placed_at,
                children,
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut FlowState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let content = self.content.to_string();
        let sizes: Vec<(f32, f32)> = self.atoms.iter().map(|a| (a.width, a.height)).collect();
        // A shrink-wrapped box is as wide as its widest line, so laying out at its width
        // (a device pixel of slack for the snapped edge) breaks the lines as measuring did.
        let slack = if self.fit {
            1. / window.scale_factor()
        } else {
            0.
        };
        let width = f32::from(bounds.size.width) + slack;
        let stale = state
            .layout
            .borrow()
            .as_ref()
            .is_none_or(|l| l.width != width || l.content != content);
        if stale {
            *state.layout.borrow_mut() = Some(FlowLayout::compute(
                &content,
                &sizes,
                &self.style,
                width,
                window,
            ));
        }
        // The owning view lays out again, not the whole window: a transcript mounts flows as
        // it scrolls.
        state.placed.0.set(Some(width));
        if state.placed_at.is_some_and(|w| w != width) {
            cx.notify(window.current_view());
        }
        if let Some(key) = self.selectable {
            let geom = Rc::new(Selectable {
                layout: state.layout.clone(),
                copies: self.atoms.iter().map(|a| a.copy.clone()).collect(),
                bounds,
            });
            recur_ui::selection::register(key, "\n", bounds, geom, window, cx);
        }
        if state.placed_at.is_none() {
            // A new element (a parked shell laid out again) has nothing placed: place the
            // children now, laid out once as their own root, so its first frame is drawn.
            let l = state.layout.borrow().clone();
            let children = l.map(|l| self.place(&l, window)).unwrap_or_default();
            let root = div().relative().size_full().children(children);
            state.children = vec![recur_ui::layout_root::prepaint_over(
                state.node,
                bounds,
                root.into_any_element(),
                window,
                cx,
            )];
            return;
        }
        for c in &mut state.children {
            c.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut FlowState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(r) = self
            .selectable
            .and_then(|key| recur_ui::selection::selected(key, cx))
        {
            let g = Selectable {
                layout: state.layout.clone(),
                copies: self.atoms.iter().map(|a| a.copy.clone()).collect(),
                bounds,
            };
            let color = recur_ui::selection::highlight(cx);
            for b in recur_ui::selection::Geometry::rects(&g, r) {
                window.paint_quad(gpui_kit::fill(b, color));
            }
        }
        for c in &mut state.children {
            c.paint(window, cx);
        }
    }
}

struct Selectable {
    layout: Rc<RefCell<Option<FlowLayout>>>,
    /// Each atom's copy text, in content order.
    copies: Vec<String>,
    bounds: Bounds<Pixels>,
}

/// The selection's text is the content with each atom replaced by its copy text; these map
/// offsets between that text and the layout's content. An offset inside a chip's text maps to
/// the chip's start (`ceil: false`) or end.
impl Selectable {
    fn segments(&self, content: &str) -> Vec<(usize, usize, usize)> {
        // (layout offset of the atom, copy offset of its text, copy length)
        let mut out = Vec::new();
        let mut shift = 0isize;
        let mut k = 0;
        for (b, ch) in content.char_indices() {
            if ch == ATOM {
                let len = self.copies.get(k).map_or(0, String::len);
                out.push((b, (b as isize + shift) as usize, len));
                shift += len as isize - ATOM_LEN as isize;
                k += 1;
            }
        }
        out
    }

    fn to_copy(&self, content: &str, at: usize) -> usize {
        let mut shift = 0isize;
        for (b, _, len) in self.segments(content) {
            if b >= at {
                break;
            }
            shift += len as isize - ATOM_LEN as isize;
        }
        (at as isize + shift).max(0) as usize
    }

    fn to_layout(&self, content: &str, at: usize, ceil: bool) -> usize {
        let mut shift = 0isize;
        for (b, c, len) in self.segments(content) {
            if at <= c {
                break;
            }
            if at < c + len {
                return if ceil { b + ATOM_LEN } else { b };
            }
            shift += len as isize - ATOM_LEN as isize;
        }
        ((at as isize - shift).max(0) as usize).min(content.len())
    }
}

impl recur_ui::selection::Geometry for Selectable {
    fn text(&self) -> String {
        self.layout.borrow().as_ref().map_or_else(String::new, |l| {
            let mut out = String::new();
            let mut k = 0;
            for ch in l.content.chars() {
                if ch == ATOM {
                    out.push_str(self.copies.get(k).map_or("", String::as_str));
                    k += 1;
                } else {
                    out.push(ch);
                }
            }
            out
        })
    }

    fn offset_at(&self, p: gpui_kit::Point<Pixels>) -> usize {
        let o = self.bounds.origin;
        self.layout.borrow().as_ref().map_or(0, |l| {
            if p.y < o.y {
                return 0;
            }
            let at = l.hit(f32::from(p.x - o.x), f32::from(p.y - o.y));
            self.to_copy(&l.content, at)
        })
    }

    fn rects(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let o = self.bounds.origin;
        self.layout.borrow().as_ref().map_or_else(Vec::new, |l| {
            let range = self.to_layout(&l.content, range.start, false)
                ..self.to_layout(&l.content, range.end, true);
            l.rects(range)
                .into_iter()
                .map(|(x, y, w, h)| Bounds {
                    origin: gpui_kit::point(o.x + px(x), o.y + px(y)),
                    size: gpui_kit::size(px(w), px(h)),
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn style() -> FlowStyle {
        FlowStyle {
            family: Family::Ui,
            size: 12.5,
            weight: 400.,
            leading: 1.5,
            color: Hsla::default(),
        }
    }

    #[gpui::test]
    fn chips_make_their_line_taller_and_break_whole(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|window, _| {
            let plain = FlowLayout::compute("hello world", &[], &style(), 400., window);
            assert_eq!(plain.lines.len(), 1);
            let line = plain.lines[0].height;
            let content = format!("see {ATOM} now");
            let l = FlowLayout::compute(&content, &[(60., 26.)], &style(), 400., window);
            assert_eq!(l.lines.len(), 1);
            assert!(l.lines[0].height > line);
            let (_, y, _, h) = l.atom_boxes()[0];
            assert!(y >= 0. && y + h <= l.height + 0.01);
            let narrow = FlowLayout::compute(&content, &[(60., 26.)], &style(), 70., window);
            assert!(narrow.lines.len() >= 2);
            assert!(
                narrow
                    .runs()
                    .iter()
                    .all(|(r, _, _)| !content[r.clone()].contains(ATOM))
            );
            let two = FlowLayout::compute("a\nb", &[], &style(), 400., window);
            assert_eq!(two.lines.len(), 2);
            assert_eq!(two.hit(0., two.lines[1].top + 1.), 2);
            assert_eq!(two.vertical(0, true), Some(2));
        });
    }

    #[test]
    fn lines_tile_the_text_at_any_width() {
        // 6px per char, with a 40px chip at byte 4.
        let text = format!("a b {ATOM}cd e");
        let mut edges = Vec::new();
        let mut x = 0.;
        for (bi, ch) in text.char_indices() {
            edges.push((bi, x));
            x += if ch == ATOM { 40. } else { 6. };
        }
        edges.push((text.len(), x));
        let p = Para {
            range: 0..text.len(),
            edges: Rc::new(edges),
        };
        for max in [-5., 0., 0.5, 3., 7., 30.] {
            let lines = break_lines(&p, &text, max, 0.25);
            let mut at = 0;
            for l in &lines {
                assert!(l.start <= l.end, "{l:?} at max {max}");
                assert_eq!(l.start, at, "lines must tile the text at max {max}");
                at = l.end;
            }
            assert_eq!(at, text.len());
        }
    }

    /// The flow breaks through `recur_ui::text::break_lines`, so a mandatory break inside a
    /// paragraph (a line separator) ends its line, as the bubble's `pre-wrap` text does in
    /// WebKit. The flow's own loop treated every opportunity as optional.
    #[test]
    fn a_line_separator_ends_the_line() {
        let text = "ab\u{2028}cd";
        let mut edges = Vec::new();
        let mut x = 0.;
        for (bi, _) in text.char_indices() {
            edges.push((bi, x));
            x += 6.;
        }
        edges.push((text.len(), x));
        let p = Para {
            range: 0..text.len(),
            edges: Rc::new(edges),
        };
        let lines = break_lines(&p, text, 400., 0.25);
        assert_eq!(lines, [0..5, 5..text.len()]);
    }
}
