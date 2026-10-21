//! A text element with the CSS text behavior the web UI depends on and GPUI's built-in text
//! lacks: `letter-spacing`, `text-transform: uppercase`, `line-height: normal`, `tabular-nums`,
//! `white-space` (normal / nowrap / pre-wrap), UAX #14 line breaking with
//! `overflow-wrap: anywhere`, `text-overflow: ellipsis`, `-webkit-line-clamp`, and line boxes
//! and baselines rounded the way Blink rounds them.
//!
//! Font family, size, weight, style, color and numeric line height inherit from the
//! surrounding GPUI text style, like CSS inheritance. Letter spacing and transforms do not
//! cascade in GPUI, so set them on the element that shows the text.

use crate::fonts::{Family, tabular_nums};
use gpui_kit::{
    App, AvailableSpace, Bounds, DefiniteLength, Element, ElementId, FontFeatures, FontId,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, SharedString,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, point, px, size,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use unicode_linebreak::{BreakOpportunity, linebreaks};

/// CSS `line-height`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    /// `normal`: the font's rounded ascent + descent + line gap.
    Normal,
    /// A unitless multiplier of the font size (`1.45`).
    Scale(f32),
    /// An absolute length in CSS px.
    Px(f32),
}

/// CSS `letter-spacing`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tracking {
    Em(f32),
    Px(f32),
}

impl Tracking {
    fn px(self, font_size: f32) -> f32 {
        match self {
            Tracking::Em(em) => em * font_size,
            Tracking::Px(p) => p,
        }
    }
}

/// CSS `white-space`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WhiteSpace {
    /// Collapse spaces, wrap at break opportunities.
    #[default]
    Normal,
    /// Collapse spaces, never wrap.
    NoWrap,
    /// Keep spaces and newlines, wrap.
    PreWrap,
}

/// CSS `text-align`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Character-level style. Unset fields inherit from the parent GPUI text style (or, inside a
/// span, from the element).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextStyle {
    pub family: Option<Family>,
    pub size: Option<Pixels>,
    pub weight: Option<f32>,
    pub italic: Option<bool>,
    pub color: Option<Hsla>,
    pub tracking: Option<Tracking>,
    pub uppercase: Option<bool>,
    pub tabular: Option<bool>,
    pub underline: Option<bool>,
    pub line_through: Option<bool>,
}

impl TextStyle {
    fn over(&self, base: &TextStyle) -> TextStyle {
        TextStyle {
            family: self.family.clone().or_else(|| base.family.clone()),
            size: self.size.or(base.size),
            weight: self.weight.or(base.weight),
            italic: self.italic.or(base.italic),
            color: self.color.or(base.color),
            tracking: self.tracking.or(base.tracking),
            uppercase: self.uppercase.or(base.uppercase),
            tabular: self.tabular.or(base.tabular),
            underline: self.underline.or(base.underline),
            line_through: self.line_through.or(base.line_through),
        }
    }
}

/// Builder methods shared by [`Text`] and [`Span`].
pub trait TextStyled: Sized {
    fn text_style_mut(&mut self) -> &mut TextStyle;

    fn family(mut self, family: Family) -> Self {
        self.text_style_mut().family = Some(family);
        self
    }
    /// `font-family: var(--ui)`.
    fn ui(self) -> Self {
        self.family(Family::Ui)
    }
    /// `font-family: var(--mono)`.
    fn mono(self) -> Self {
        self.family(Family::Mono)
    }
    /// `font-family: var(--disp)`.
    fn display(self) -> Self {
        self.family(Family::Display)
    }
    /// `font-family: system-ui, …` (keycaps).
    fn system(self) -> Self {
        self.family(Family::System)
    }
    fn size(mut self, size: f32) -> Self {
        self.text_style_mut().size = Some(px(size));
        self
    }
    fn weight(mut self, weight: f32) -> Self {
        self.text_style_mut().weight = Some(weight);
        self
    }
    fn italic(mut self) -> Self {
        self.text_style_mut().italic = Some(true);
        self
    }
    fn color(mut self, color: impl Into<Hsla>) -> Self {
        self.text_style_mut().color = Some(color.into());
        self
    }
    /// `letter-spacing` in em.
    fn tracking(mut self, em: f32) -> Self {
        self.text_style_mut().tracking = Some(Tracking::Em(em));
        self
    }
    /// `letter-spacing` in px.
    fn tracking_px(mut self, px: f32) -> Self {
        self.text_style_mut().tracking = Some(Tracking::Px(px));
        self
    }
    fn uppercase(mut self) -> Self {
        self.text_style_mut().uppercase = Some(true);
        self
    }
    /// `font-variant-numeric: tabular-nums`.
    fn tabular(mut self) -> Self {
        self.text_style_mut().tabular = Some(true);
        self
    }
    fn underline(mut self, on: bool) -> Self {
        self.text_style_mut().underline = Some(on);
        self
    }
    fn line_through(mut self, on: bool) -> Self {
        self.text_style_mut().line_through = Some(on);
        self
    }
}

/// A styled run inside a [`Text`] (`<span>`, `<b>`, `.kbd-mod`).
#[derive(Clone, Debug)]
pub struct Span {
    pub text: SharedString,
    pub style: TextStyle,
}

pub fn span(text: impl Into<SharedString>) -> Span {
    Span {
        text: text.into(),
        style: TextStyle::default(),
    }
}

impl TextStyled for Span {
    fn text_style_mut(&mut self) -> &mut TextStyle {
        &mut self.style
    }
}

/// Block-level text: one or more spans laid out as CSS inline text in a block box.
pub struct Text {
    id: Option<ElementId>,
    spans: Vec<Span>,
    style: TextStyle,
    line_height: Option<LineHeight>,
    white_space: WhiteSpace,
    anywhere: bool,
    ellipsis: bool,
    clamp: Option<usize>,
    align: Align,
    layout: Rc<RefCell<Option<Laid>>>,
}

/// Text from a single span.
pub fn text(content: impl Into<SharedString>) -> Text {
    Text::from_spans(vec![span(content)])
}

impl Text {
    pub fn from_spans(spans: Vec<Span>) -> Text {
        Text {
            id: None,
            spans,
            style: TextStyle::default(),
            line_height: None,
            white_space: WhiteSpace::Normal,
            anywhere: false,
            ellipsis: false,
            clamp: None,
            align: Align::Left,
            layout: Rc::default(),
        }
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }
    pub fn line_height(mut self, lh: LineHeight) -> Self {
        self.line_height = Some(lh);
        self
    }
    /// `line-height: <number>`.
    pub fn leading(self, scale: f32) -> Self {
        self.line_height(LineHeight::Scale(scale))
    }
    /// `line-height: normal`.
    pub fn leading_normal(self) -> Self {
        self.line_height(LineHeight::Normal)
    }
    pub fn white_space(mut self, ws: WhiteSpace) -> Self {
        self.white_space = ws;
        self
    }
    /// `white-space: nowrap`.
    pub fn nowrap(self) -> Self {
        self.white_space(WhiteSpace::NoWrap)
    }
    /// `white-space: pre-wrap`.
    pub fn pre_wrap(self) -> Self {
        self.white_space(WhiteSpace::PreWrap)
    }
    /// `overflow-wrap: anywhere` (also `word-break: break-word`).
    pub fn break_anywhere(mut self) -> Self {
        self.anywhere = true;
        self
    }
    /// `white-space: nowrap; overflow: hidden; text-overflow: ellipsis`.
    pub fn truncate(mut self) -> Self {
        self.white_space = WhiteSpace::NoWrap;
        self.ellipsis = true;
        self
    }
    /// `-webkit-line-clamp: n` (with an ellipsis on the last kept line).
    pub fn line_clamp(mut self, lines: usize) -> Self {
        self.clamp = Some(lines.max(1));
        self
    }
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
    pub fn child_span(mut self, s: Span) -> Self {
        self.spans.push(s);
        self
    }
}

impl TextStyled for Text {
    fn text_style_mut(&mut self) -> &mut TextStyle {
        &mut self.style
    }
}

/// The resolved inputs that decide a layout; compared to reuse it across phases.
#[derive(Clone, PartialEq)]
struct Resolved {
    content: SharedString,
    runs: Vec<RunStyle>,
    font_size: f32,
    line_height: LineHeight,
    base: RunStyle,
}

#[derive(Clone, PartialEq)]
struct RunStyle {
    len: usize,
    family: Family,
    weight: f32,
    italic: bool,
    features: FontFeatures,
    color: Hsla,
    tracking: f32,
    underline: bool,
    line_through: bool,
}

impl RunStyle {
    fn text_run(&self, len: usize) -> TextRun {
        TextRun {
            len,
            font: self
                .family
                .font(self.weight, self.italic, self.features.clone()),
            color: self.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }
}

/// One shaped paragraph: glyph boxes keyed by byte offset into the paragraph string.
struct Para {
    range: Range<usize>,
    /// Byte offset (paragraph-relative) → left edge in px, including tracking, for every char
    /// boundary plus the end.
    edges: Vec<(usize, f32)>,
    glyphs: Vec<PaintGlyph>,
}

#[derive(Clone)]
struct PaintGlyph {
    /// Byte offset within the paragraph.
    index: usize,
    x: f32,
    font_id: FontId,
    glyph: gpui_kit::GlyphId,
    is_emoji: bool,
    run: usize,
}

struct Line {
    para: usize,
    range: Range<usize>,
    /// Visible width (trailing spaces hang).
    width: f32,
    ellipsis: bool,
}

struct Laid {
    key: Resolved,
    paras: Vec<Para>,
    metrics: LineMetrics,
    ellipsis: Option<(Vec<PaintGlyph>, f32)>,
    wrap_width: Option<f32>,
    lines: Vec<Line>,
}

/// Line box metrics in CSS px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineMetrics {
    /// Height of one line box.
    pub height: f32,
    /// Top of the line box to the baseline.
    pub baseline: f32,
    pub ascent: f32,
    pub descent: f32,
}

/// Blink's line box for one font: ascent and descent rounded to whole device pixels, the
/// half-leading floored to a whole device pixel above the text.
pub fn line_metrics(
    ascent: f32,
    descent: f32,
    line_gap: f32,
    font_size: f32,
    line_height: LineHeight,
    scale: f32,
) -> LineMetrics {
    let a = (ascent * scale).round();
    let d = (descent * scale).round();
    let h = match line_height {
        LineHeight::Normal => a + d + (line_gap * scale).round(),
        LineHeight::Scale(m) => m * font_size * scale,
        LineHeight::Px(p) => p * scale,
    };
    let half_leading = ((h - (a + d)) / 2.).floor();
    LineMetrics {
        height: h / scale,
        baseline: (half_leading + a) / scale,
        ascent: a / scale,
        descent: d / scale,
    }
}

fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

impl Text {
    fn resolve(&self, window: &Window) -> Resolved {
        let inherited = window.text_style();
        let font_size = self
            .style
            .size
            .unwrap_or_else(|| inherited.font_size.to_pixels(window.rem_size()));
        let parent = TextStyle {
            family: Some(Family::Stack(inherited.font_family.clone(), Vec::new())),
            size: Some(font_size),
            weight: Some(inherited.font_weight.0),
            italic: Some(inherited.font_style == gpui_kit::FontStyle::Italic),
            color: Some(inherited.color),
            tracking: Some(Tracking::Px(0.)),
            uppercase: Some(false),
            tabular: Some(false),
            underline: Some(false),
            line_through: Some(false),
        };
        let base_style = self.style.over(&parent);
        let fs = f32::from(font_size);
        let run_of = |s: &TextStyle, len: usize| RunStyle {
            len,
            family: canonical_family(s.family.clone().unwrap_or(Family::Ui)),
            weight: s.weight.unwrap_or(400.),
            italic: s.italic.unwrap_or(false),
            features: if s.tabular.unwrap_or(false) {
                tabular_nums()
            } else {
                FontFeatures::default()
            },
            color: s.color.unwrap_or_default(),
            tracking: s.tracking.unwrap_or(Tracking::Px(0.)).px(fs),
            underline: s.underline.unwrap_or(false),
            line_through: s.line_through.unwrap_or(false),
        };
        let mut content = String::new();
        let mut runs = Vec::new();
        for s in &self.spans {
            let style = s.style.over(&base_style);
            let mut t = s.text.to_string();
            if style.uppercase.unwrap_or(false) {
                t = t.to_uppercase();
            }
            if t.is_empty() {
                continue;
            }
            content.push_str(&t);
            runs.push(run_of(&style, t.len()));
        }
        let (content, runs) = match self.white_space {
            WhiteSpace::PreWrap => (content, runs),
            _ => collapse_spaces(&content, runs),
        };
        let line_height = self.line_height.unwrap_or(match inherited.line_height {
            DefiniteLength::Absolute(a) => LineHeight::Px(a.to_pixels(window.rem_size()).into()),
            DefiniteLength::Fraction(f) => LineHeight::Scale(f),
        });
        Resolved {
            content: content.into(),
            runs,
            font_size: fs,
            line_height,
            base: run_of(&base_style, 0),
        }
    }
}

/// GPUI names the inherited family only; restore the CSS stack for the three app stacks.
fn canonical_family(f: Family) -> Family {
    match f {
        Family::Stack(name, rest) if rest.is_empty() => match name.as_ref() {
            crate::fonts::UI => Family::Ui,
            crate::fonts::MONO => Family::Mono,
            crate::fonts::DISPLAY => Family::Display,
            crate::fonts::SYSTEM => Family::System,
            _ => Family::Stack(name, rest),
        },
        other => other,
    }
}

/// `white-space: normal|nowrap`: collapse runs of whitespace to one space and drop leading and
/// trailing whitespace, keeping the run table in step.
fn collapse_spaces(s: &str, runs: Vec<RunStyle>) -> (String, Vec<RunStyle>) {
    let mut out = String::with_capacity(s.len());
    let mut out_runs: Vec<RunStyle> = Vec::with_capacity(runs.len());
    let mut run_ix = 0;
    let mut run_end = runs.first().map_or(0, |r| r.len);
    let mut prev_space = true;
    for (i, ch) in s.char_indices() {
        while run_ix + 1 < runs.len() && i >= run_end {
            run_ix += 1;
            run_end += runs[run_ix].len;
        }
        let is_space = matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}');
        if is_space && prev_space {
            continue;
        }
        let ch = if is_space { ' ' } else { ch };
        prev_space = is_space;
        let n = ch.len_utf8();
        out.push(ch);
        let len = out_runs.len();
        match out_runs.last_mut() {
            Some(last) if len == run_ix + 1 => last.len += n,
            _ => {
                while out_runs.len() < run_ix {
                    let mut empty = runs[out_runs.len()].clone();
                    empty.len = 0;
                    out_runs.push(empty);
                }
                let mut r = runs[run_ix].clone();
                r.len = n;
                out_runs.push(r);
            }
        }
    }
    if out.ends_with(' ') {
        out.pop();
        if let Some(last) = out_runs.iter_mut().rev().find(|r| r.len > 0) {
            last.len -= 1;
        }
    }
    out_runs.retain(|r| r.len > 0);
    (out, out_runs)
}

fn run_slices(runs: &[RunStyle], range: Range<usize>) -> Vec<(usize, Range<usize>)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, r) in runs.iter().enumerate() {
        let end = start + r.len;
        let s = start.max(range.start);
        let e = end.min(range.end);
        if s < e {
            out.push((i, s..e));
        }
        start = end;
    }
    out
}

fn shape_para(key: &Resolved, range: Range<usize>, window: &Window) -> Para {
    let text = &key.content[range.clone()];
    let slices = run_slices(&key.runs, range.clone());
    let runs: Vec<TextRun> = slices
        .iter()
        .map(|(i, r)| key.runs[*i].text_run(r.len()))
        .collect();
    let shaped = if text.is_empty() {
        None
    } else {
        Some(window.text_system().shape_line(
            text.to_string().into(),
            px(key.font_size),
            &runs,
            None,
        ))
    };
    let run_at = |ix: usize| -> usize {
        slices
            .iter()
            .find(|(_, r)| r.contains(&(ix + range.start)))
            .map_or(0, |(i, _)| *i)
    };
    let mut glyph_x: Vec<(usize, f32)> = Vec::new();
    let mut glyphs = Vec::new();
    if let Some(line) = &shaped {
        for run in &line.runs {
            for g in &run.glyphs {
                glyph_x.push((g.index, f32::from(g.position.x)));
                glyphs.push(PaintGlyph {
                    index: g.index,
                    x: g.position.x.into(),
                    font_id: run.font_id,
                    glyph: g.id,
                    is_emoji: g.is_emoji,
                    run: run_at(g.index),
                });
            }
        }
    }
    let total = shaped.as_ref().map_or(0., |l| f32::from(l.width));
    glyph_x.sort_by_key(|(i, _)| *i);
    // Left edge of each char: the x of its first glyph, or the next glyph's x for chars that
    // produced none (ligature tails). Tracking accumulates after every char.
    let mut edges = Vec::with_capacity(text.len() + 1);
    let mut acc_tracking = 0.;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut gi = 0;
    for &(bi, _) in &chars {
        while gi < glyph_x.len() && glyph_x[gi].0 < bi {
            gi += 1;
        }
        let x = glyph_x.get(gi).map_or(total, |g| g.1);
        edges.push((bi, x + acc_tracking));
        acc_tracking += key.runs.get(run_at(bi)).map_or(0., |r| r.tracking);
    }
    edges.push((text.len(), total + acc_tracking));
    for g in &mut glyphs {
        let ci = chars.partition_point(|c| c.0 < g.index);
        let before: f32 = chars[..ci]
            .iter()
            .map(|(b, _)| key.runs.get(run_at(*b)).map_or(0., |r| r.tracking))
            .sum();
        g.x += before;
    }
    Para {
        range,
        edges,
        glyphs,
    }
}

impl Para {
    fn x_at(&self, byte: usize) -> f32 {
        match self.edges.binary_search_by_key(&byte, |e| e.0) {
            Ok(i) => self.edges[i].1,
            Err(i) => self.edges.get(i).map_or(0., |e| e.1),
        }
    }
    fn width(&self, text: &str, r: Range<usize>) -> f32 {
        let trimmed_end = r.start + text[r.clone()].trim_end_matches([' ', '\t']).len();
        self.x_at(trimmed_end) - self.x_at(r.start)
    }
}

fn break_lines(
    para: &Para,
    pi: usize,
    text: &str,
    max: Option<f32>,
    anywhere: bool,
    eps: f32,
) -> Vec<Line> {
    let mut lines = Vec::new();
    let Some(max) = max else {
        lines.push(Line {
            para: pi,
            range: 0..text.len(),
            width: para.width(text, 0..text.len()),
            ellipsis: false,
        });
        return lines;
    };
    let opportunities: Vec<(usize, BreakOpportunity)> = linebreaks(text).collect();
    let mut start = 0;
    let mut last_fit: Option<usize> = None;
    let mut i = 0;
    while i < opportunities.len() {
        let (pos, kind) = opportunities[i];
        let w = para.width(text, start..pos);
        if w <= max + eps {
            if kind == BreakOpportunity::Mandatory {
                lines.push(Line {
                    para: pi,
                    range: start..pos,
                    width: w,
                    ellipsis: false,
                });
                start = pos;
                last_fit = None;
            } else {
                last_fit = Some(pos);
            }
            i += 1;
            continue;
        }
        if let Some(b) = last_fit.take() {
            lines.push(Line {
                para: pi,
                range: start..b,
                width: para.width(text, start..b),
                ellipsis: false,
            });
            start = b;
            continue;
        }
        if anywhere {
            // Split the overlong word at the last char that still fits.
            let mut cut = start;
            for (bi, ch) in text[start..pos].char_indices() {
                let end = start + bi + ch.len_utf8();
                if para.width(text, start..end) > max + eps {
                    break;
                }
                cut = end;
            }
            if cut == start {
                cut = start + text[start..].chars().next().map_or(1, char::len_utf8);
            }
            lines.push(Line {
                para: pi,
                range: start..cut,
                width: para.width(text, start..cut),
                ellipsis: false,
            });
            start = cut;
            continue;
        }
        // An unbreakable word wider than the box overflows on its own line.
        if kind == BreakOpportunity::Mandatory {
            lines.push(Line {
                para: pi,
                range: start..pos,
                width: w,
                ellipsis: false,
            });
            start = pos;
        } else {
            last_fit = Some(pos);
        }
        i += 1;
    }
    if start < text.len() || lines.is_empty() {
        lines.push(Line {
            para: pi,
            range: start..text.len(),
            width: para.width(text, start..text.len()),
            ellipsis: false,
        });
    }
    lines
}

fn font_ascent_descent(key: &Resolved, window: &Window) -> (f32, f32, f32) {
    let ts = window.text_system();
    let font = key
        .base
        .family
        .font(key.base.weight, key.base.italic, FontFeatures::default());
    let id = ts.resolve_font(&font);
    let size = px(key.font_size);
    let ascent = f32::from(ts.ascent(id, size));
    let descent = -f32::from(ts.descent(id, size));
    let gap = crate::font_metrics::line_gap(&key.base.family, key.font_size);
    (ascent, descent, gap)
}

impl Laid {
    fn build(key: Resolved, window: &Window) -> Laid {
        let (a, d, g) = font_ascent_descent(&key, window);
        let metrics = line_metrics(
            a,
            d,
            g,
            key.font_size,
            key.line_height,
            window.scale_factor(),
        );
        let mut paras = Vec::new();
        let mut start = 0;
        for piece in key.content.split('\n') {
            let r = start..start + piece.len();
            paras.push(shape_para(&key, r, window));
            start += piece.len() + 1;
        }
        Laid {
            key,
            paras,
            metrics,
            ellipsis: None,
            wrap_width: None,
            lines: Vec::new(),
        }
    }

    fn relayout(
        &mut self,
        width: Option<f32>,
        ws: WhiteSpace,
        anywhere: bool,
        clamp: Option<usize>,
        truncate: bool,
        window: &Window,
    ) {
        let eps = 0.5 / window.scale_factor();
        let wrap = if ws == WhiteSpace::NoWrap {
            None
        } else {
            width
        };
        let mut lines = Vec::new();
        for (pi, p) in self.paras.iter().enumerate() {
            let text = &self.key.content[p.range.clone()];
            lines.extend(break_lines(p, pi, text, wrap, anywhere, eps));
        }
        let mut cut_last = false;
        if let Some(n) = clamp
            && lines.len() > n
        {
            lines.truncate(n);
            cut_last = true;
        }
        if let Some(w) = width
            && (cut_last || truncate)
        {
            let ell = self.ellipsis_glyphs(window);
            let ell_w = ell.1;
            let last_ix = lines.len().saturating_sub(1);
            for (li, line) in lines.iter_mut().enumerate() {
                let overflow = line.width > w + eps;
                if !(overflow && truncate || cut_last && li == last_ix) {
                    continue;
                }
                let p = &self.paras[line.para];
                let text = &self.key.content[p.range.clone()];
                let mut end = line.range.start;
                for (bi, ch) in text[line.range.clone()].char_indices() {
                    let e = line.range.start + bi + ch.len_utf8();
                    if p.width(text, line.range.start..e) + ell_w > w + eps {
                        break;
                    }
                    end = e;
                }
                line.range.end = end;
                line.width = p.width(text, line.range.clone()) + ell_w;
                line.ellipsis = true;
            }
            self.ellipsis = Some(ell);
        }
        self.wrap_width = width;
        self.lines = lines;
    }

    fn ellipsis_glyphs(&self, window: &Window) -> (Vec<PaintGlyph>, f32) {
        let run = self.key.base.text_run("…".len());
        let shaped =
            window
                .text_system()
                .shape_line("…".into(), px(self.key.font_size), &[run], None);
        let mut glyphs = Vec::new();
        for r in &shaped.runs {
            for g in &r.glyphs {
                glyphs.push(PaintGlyph {
                    index: 0,
                    x: g.position.x.into(),
                    font_id: r.font_id,
                    glyph: g.id,
                    is_emoji: g.is_emoji,
                    run: usize::MAX,
                });
            }
        }
        (glyphs, f32::from(shaped.width) + self.key.base.tracking)
    }

    fn size(&self, scale: f32) -> gpui_kit::Size<Pixels> {
        if self.key.content.is_empty() {
            return size(px(0.), px(0.));
        }
        let w = self.lines.iter().map(|l| l.width).fold(0., f32::max);
        let h = self.metrics.height * self.lines.len() as f32;
        size(px(snap(w, scale)), px(snap(h, scale)))
    }
}

pub struct TextState {
    layout: Rc<RefCell<Option<Laid>>>,
}

impl IntoElement for Text {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Text {
    type RequestLayoutState = TextState;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        self.id.clone()
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
    ) -> (LayoutId, TextState) {
        let key = self.resolve(window);
        let layout = self.layout.clone();
        {
            let mut slot = layout.borrow_mut();
            if slot.as_ref().is_none_or(|l| l.key != key) {
                *slot = Some(Laid::build(key, window));
            }
        }
        let (ws, anywhere, clamp, truncate) =
            (self.white_space, self.anywhere, self.clamp, self.ellipsis);
        let measure_slot = layout.clone();
        let style = Style {
            min_size: size(px(0.).into(), px(0.).into()),
            ..Style::default()
        };
        let mut style = style;
        if truncate {
            // `overflow: hidden` makes the automatic minimum size zero in a flex row.
            style.min_size.width = px(0.).into();
        }
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, _| {
                let mut slot = measure_slot.borrow_mut();
                let Some(laid) = slot.as_mut() else {
                    return size(px(0.), px(0.));
                };
                let width = known.width.map(f32::from).or(match available.width {
                    AvailableSpace::Definite(w) => Some(w.into()),
                    AvailableSpace::MinContent => Some(0.),
                    AvailableSpace::MaxContent => None,
                });
                // Truncated text measures at its full width; the box decides where it cuts.
                let width = if truncate { None } else { width };
                laid.relayout(width, ws, anywhere, clamp, false, window);
                laid.size(window.scale_factor())
            });
        (layout_id, TextState { layout })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut TextState,
        window: &mut Window,
        _: &mut App,
    ) {
        if let Some(laid) = state.layout.borrow_mut().as_mut() {
            laid.relayout(
                Some(bounds.size.width.into()),
                self.white_space,
                self.anywhere,
                self.clamp,
                self.ellipsis,
                window,
            );
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut TextState,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let slot = state.layout.borrow();
        let Some(laid) = slot.as_ref() else { return };
        let scale = window.scale_factor();
        let box_w = f32::from(bounds.size.width);
        let clip = self.ellipsis || self.clamp.is_some();
        let paint = |window: &mut Window, cx: &mut App| {
            for (li, line) in laid.lines.iter().enumerate() {
                let p = &laid.paras[line.para];
                let top = f32::from(bounds.origin.y) + laid.metrics.height * li as f32;
                let baseline = snap(top + laid.metrics.baseline, scale);
                let start_x = p.x_at(line.range.start);
                let offset = match self.align {
                    Align::Left => 0.,
                    Align::Center => (box_w - line.width) / 2.,
                    Align::Right => box_w - line.width,
                };
                let left = f32::from(bounds.origin.x) + offset;
                for g in p.glyphs.iter().filter(|g| line.range.contains(&g.index)) {
                    let run = &laid.key.runs[g.run];
                    let origin = point(px(left + g.x - start_x), px(baseline));
                    paint_one(window, cx, origin, g, laid.key.font_size, run.color);
                }
                let end_x = left + p.x_at(line.range.end) - start_x;
                for (ri, r) in run_slices(
                    &laid.key.runs,
                    p.range.start + line.range.start..p.range.start + line.range.end,
                ) {
                    let run = &laid.key.runs[ri];
                    if !(run.underline || run.line_through) {
                        continue;
                    }
                    let x0 = left + p.x_at(r.start - p.range.start) - start_x;
                    let x1 = left + p.x_at(r.end - p.range.start) - start_x;
                    let deco = crate::font_metrics::decorations(&run.family, laid.key.font_size);
                    if run.underline {
                        window.paint_underline(
                            point(px(x0), px(baseline + deco.underline_offset)),
                            px(x1 - x0),
                            &UnderlineStyle {
                                thickness: px(deco.underline_thickness),
                                color: Some(run.color),
                                wavy: false,
                            },
                        );
                    }
                    if run.line_through {
                        window.paint_strikethrough(
                            point(px(x0), px(baseline - deco.strike_offset)),
                            px(x1 - x0),
                            &StrikethroughStyle {
                                thickness: px(deco.strike_thickness),
                                color: Some(run.color),
                            },
                        );
                    }
                }
                if line.ellipsis
                    && let Some((glyphs, _)) = &laid.ellipsis
                {
                    for g in glyphs {
                        let origin = point(px(end_x + g.x), px(baseline));
                        paint_one(
                            window,
                            cx,
                            origin,
                            g,
                            laid.key.font_size,
                            laid.key.base.color,
                        );
                    }
                }
            }
        };
        if clip {
            window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
                paint(window, cx)
            });
        } else {
            paint(window, cx);
        }
    }
}

fn paint_one(
    window: &mut Window,
    _: &mut App,
    origin: gpui_kit::Point<Pixels>,
    g: &PaintGlyph,
    font_size: f32,
    color: Hsla,
) {
    // A glyph the atlas cannot rasterize paints nothing, as in the browser.
    let _ = if g.is_emoji {
        window.paint_emoji(origin, g.font_id, g.glyph, px(font_size))
    } else {
        window.paint_glyph(origin, g.font_id, g.glyph, px(font_size), color)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_line_box_rounding() {
        // Inter 13px at 2x: ascent 12.59375 → 25dp, descent 3.1357 → 6dp.
        let m = line_metrics(12.59375, 3.1357422, 0., 13., LineHeight::Normal, 2.);
        assert_eq!(m.height, 15.5);
        assert_eq!(m.baseline, 12.5);
        let m = line_metrics(12.59375, 3.1357422, 0., 13., LineHeight::Scale(1.45), 2.);
        assert!((m.height - 18.85).abs() < 1e-5);
        // half-leading (37.7 − 31) / 2 = 3.35dp floors to 3dp.
        assert_eq!(m.baseline, 14.);
    }

    #[test]
    fn collapses_white_space_across_runs() {
        let run = |len| RunStyle {
            len,
            family: Family::Ui,
            weight: 400.,
            italic: false,
            features: FontFeatures::default(),
            color: Hsla::default(),
            tracking: 0.,
            underline: false,
            line_through: false,
        };
        let (s, runs) = collapse_spaces("  a  b ", vec![run(4), run(3)]);
        assert_eq!(s, "a b");
        assert_eq!(runs.iter().map(|r| r.len).collect::<Vec<_>>(), vec![2, 1]);
    }
}
