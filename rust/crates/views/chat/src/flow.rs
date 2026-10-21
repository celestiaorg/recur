//! Inline text with atomic chips, as the user bubble and the composer lay them out: a
//! `white-space: pre-wrap; overflow-wrap: anywhere` block whose `inline-flex` chips
//! (`vertical-align: middle`) take part in line breaking and make their line taller.
//! `recur_ui::Text` has no atomic inline boxes, so this lays the lines out with the same
//! metrics (`line_metrics`, UAX #14 breaks, trailing spaces hanging) and paints each line's
//! text runs as nowrap `recur_ui` text and each chip as its own element at its box.
//!
//! Content is one string where every chip is [`ATOM`]; chips are given in order with their
//! margin-box sizes.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, FontFeatures, GlobalElementId,
    Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, SharedString, Style, TextRun,
    Window, point, px, size,
};
use recur_ui::font_metrics::line_gap;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, LineMetrics, TextStyled, WhiteSpace, line_metrics, text};
use unicode_linebreak::{BreakOpportunity, linebreaks};

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
    edges: Vec<(usize, f32)>,
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

fn shape_para(
    content: &str,
    range: Range<usize>,
    atom_w: &dyn Fn(usize) -> f32,
    s: &FlowStyle,
    window: &Window,
) -> Para {
    let text = &content[range.clone()];
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
            pen += atom_w(range.start + bi);
            seg_start = bi + ATOM_LEN;
        }
    }
    flush(seg_start, text.len(), &mut pen, &mut edges);
    edges.push((text.len(), pen));
    Para { range, edges }
}

fn break_lines(p: &Para, text: &str, max: f32, eps: f32) -> Vec<Range<usize>> {
    let ops: Vec<(usize, BreakOpportunity)> = linebreaks(text).collect();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut last_fit: Option<usize> = None;
    let mut i = 0;
    while i < ops.len() {
        let (pos, _) = ops[i];
        if p.width(text, start..pos) <= max + eps {
            last_fit = Some(pos);
            i += 1;
            continue;
        }
        if let Some(b) = last_fit.take()
            && b > start
        {
            lines.push(start..b);
            start = b;
            continue;
        }
        // `overflow-wrap: anywhere`: break inside the word (an atom stays whole).
        let mut cut = start;
        for (bi, ch) in text[start..pos].char_indices() {
            let end = start + bi + ch.len_utf8();
            if p.width(text, start..end) > max + eps {
                break;
            }
            cut = end;
        }
        if cut == start {
            cut = start + text[start..].chars().next().map_or(1, char::len_utf8);
        }
        lines.push(start..cut);
        start = cut;
    }
    if start < text.len() || lines.is_empty() {
        lines.push(start..text.len());
    }
    lines
}

impl FlowLayout {
    /// Lays `content` out at `width` with chip sizes in order.
    pub fn compute(
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
        let atom_w = |byte: usize| {
            at.iter()
                .find(|a| a.0 == byte)
                .map_or(0., |a| a.1)
        };
        let (a, d, g, xh) = font_extent(window, s);
        let strut = line_metrics(a, d, g, s.size, LineHeight::Scale(s.leading));
        let xh = (xh * 64.).round() / 64.;
        let eps = 0.5 / window.scale_factor();
        let mut paras = Vec::new();
        let mut start = 0;
        for piece in content.split('\n') {
            paras.push(shape_para(
                content,
                start..start + piece.len(),
                &atom_w,
                s,
                window,
            ));
            start += piece.len() + 1;
        }
        let mut lines = Vec::new();
        let mut top = 0.;
        let mut max_line: f32 = 0.;
        for (pi, p) in paras.iter().enumerate() {
            let text = &content[p.range.clone()];
            for r in break_lines(p, text, width, eps) {
                let abs = p.range.start + r.start..p.range.start + r.end;
                let (mut above, mut below) = (strut.baseline, strut.height - strut.baseline);
                for &(b, _, h) in &at {
                    if b >= abs.start && b < abs.end {
                        // `vertical-align: middle`: the box's midpoint at baseline − x-height/2.
                        let mid = xh / 2.;
                        above = above.max(mid + h / 2.);
                        below = below.max(h / 2. - mid);
                    }
                }
                max_line = max_line.max(p.width(text, r.clone()));
                lines.push(FlowLine {
                    range: abs,
                    top,
                    height: above + below,
                    baseline: above,
                    left: p.x_at(r.start),
                    para: pi,
                });
                top += above + below;
            }
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
        (x, l.top + l.baseline - self.strut.baseline, self.strut.height)
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
        for &(b, ex) in &p.edges {
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
        let mut out = Vec::new();
        for l in &self.lines {
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
    min_height: f32,
    out: Option<Rc<RefCell<Option<FlowLayout>>>>,
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
        min_height: 0.,
        out: None,
    }
}

impl Flow {
    pub fn fit_content(mut self) -> Self {
        self.fit = true;
        self
    }

    /// The layout of the last frame, for hosts that place a caret or hit-test.
    pub fn layout_into(mut self, out: Rc<RefCell<Option<FlowLayout>>>) -> Self {
        self.out = Some(out);
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

pub struct FlowState {
    layout: Rc<RefCell<Option<FlowLayout>>>,
}

pub struct FlowPrepaint {
    children: Vec<AnyElement>,
}

impl Element for Flow {
    type RequestLayoutState = FlowState;
    type PrepaintState = FlowPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
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
        let min_h = self.min_height;
        let id = window.request_measured_layout(Style::default(), move |known, avail, window, _| {
            let w = known.width.map(f32::from).unwrap_or(match avail.width {
                AvailableSpace::Definite(w) => w.into(),
                _ => f32::MAX / 4.,
            });
            let l = FlowLayout::compute(&content, &sizes, &st, w, window);
            let out = size(
                px(if fit { l.max_line.min(w) } else { w }),
                px(l.height.max(min_h)),
            );
            *slot.borrow_mut() = Some(l);
            out
        });
        (id, FlowState { layout })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut FlowState,
        window: &mut Window,
        cx: &mut App,
    ) -> FlowPrepaint {
        let content = self.content.to_string();
        let sizes: Vec<(f32, f32)> = self.atoms.iter().map(|a| (a.width, a.height)).collect();
        let width = if self.fit {
            // A shrink-wrapped box lays out at the width it reported.
            state
                .layout
                .borrow()
                .as_ref()
                .map_or(f32::from(bounds.size.width), |l| l.width)
        } else {
            f32::from(bounds.size.width)
        };
        let stale = state
            .layout
            .borrow()
            .as_ref()
            .is_none_or(|l| l.width != width || l.content != content);
        if stale {
            *state.layout.borrow_mut() =
                Some(FlowLayout::compute(&content, &sizes, &self.style, width, window));
        }
        let layout = state.layout.borrow().clone().expect("flow layout");
        let mut children = Vec::new();
        let st = &self.style;
        for (r, x, y) in layout.runs() {
            let mut t = text(content[r].to_string())
                .family(st.family.clone())
                .size(st.size)
                .weight(st.weight)
                .color(st.color)
                .line_height(LineHeight::Scale(st.leading))
                .white_space(WhiteSpace::PreWrap)
                .into_any_element();
            t.prepaint_as_root(
                bounds.origin + point(px(x), px(y)),
                size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                window,
                cx,
            );
            children.push(t);
        }
        for (atom, (x, y, w, h)) in self.atoms.iter_mut().zip(layout.atom_boxes()) {
            let mut el = std::mem::replace(&mut atom.el, gpui_kit::Empty.into_any_element());
            el.prepaint_as_root(
                bounds.origin + point(px(x), px(y)),
                size(
                    AvailableSpace::Definite(px(w)),
                    AvailableSpace::Definite(px(h)),
                ),
                window,
                cx,
            );
            children.push(el);
        }
        FlowPrepaint { children }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut FlowState,
        p: &mut FlowPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        for c in &mut p.children {
            c.paint(window, cx);
        }
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
            assert!(narrow.runs().iter().all(|(r, _, _)| !content[r.clone()].contains(ATOM)));
            let two = FlowLayout::compute("a\nb", &[], &style(), 400., window);
            assert_eq!(two.lines.len(), 2);
            assert_eq!(two.hit(0., two.lines[1].top + 1.), 2);
            assert_eq!(two.vertical(0, true), Some(2));
        });
    }
}
