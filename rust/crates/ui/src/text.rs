//! A text element with the CSS text behavior the web UI depends on and GPUI's built-in text
//! lacks: `letter-spacing`, `text-transform: uppercase`, `line-height: normal`, `tabular-nums`,
//! `white-space` (normal / nowrap / pre-wrap), UAX #14 line breaking with
//! `overflow-wrap: anywhere`, `text-overflow: ellipsis`, `-webkit-line-clamp`, inline boxes
//! (padded, bordered `<span>`s), and line boxes and baselines rounded the way Blink rounds
//! them.
//!
//! Font family, size, weight, style, color and numeric line height inherit from the
//! surrounding GPUI text style, like CSS inheritance. Letter spacing and transforms do not
//! cascade in GPUI, so set them on the element that shows the text.

use crate::fonts::{Family, tabular_nums};
use gpui_kit::{
    App, AvailableSpace, BorderStyle, Bounds, DefiniteLength, Element, ElementId, FontFeatures,
    FontId, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, SharedString,
    StrikethroughStyle, Style, TextRun, UnderlineStyle, Window, point, px, quad, size,
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

/// The box of an inline `<span>` with padding, border, margin or background. Horizontal
/// extents take space in the line; vertical ones paint around the font's content area
/// without changing the line box, as in CSS.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InlineBox {
    pub margin_left: f32,
    pub margin_right: f32,
    pub border: f32,
    pub border_color: Hsla,
    pub pad_top: f32,
    pub pad_right: f32,
    pub pad_bottom: f32,
    pub pad_left: f32,
    pub background: Option<Hsla>,
    pub radius: f32,
}

impl InlineBox {
    fn left(&self) -> f32 {
        self.margin_left + self.border + self.pad_left
    }
    fn right(&self) -> f32 {
        self.margin_right + self.border + self.pad_right
    }
}

/// A styled run inside a [`Text`] (`<span>`, `<b>`, `.kbd-mod`, `<code>`).
#[derive(Clone, Debug)]
pub struct Span {
    pub text: SharedString,
    pub style: TextStyle,
    pub inline_box: Option<InlineBox>,
}

pub fn span(text: impl Into<SharedString>) -> Span {
    Span {
        text: text.into(),
        style: TextStyle::default(),
        inline_box: None,
    }
}

impl Span {
    /// Give the span an inline box (padding, border, background).
    pub fn boxed(mut self, b: InlineBox) -> Self {
        self.inline_box = Some(b);
        self
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
    exact_height: bool,
    opacity: f32,
    mask: Option<AlphaMask>,
    layout: Rc<RefCell<Option<Laid>>>,
}

/// A horizontal alpha mask over the window (CSS px → alpha multiplier), like a CSS
/// `mask-image` gradient. Applied per glyph at the glyph's center.
pub type AlphaMask = Rc<dyn Fn(f32) -> f32>;

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
            exact_height: false,
            opacity: 1.,
            mask: None,
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
    /// Fade glyphs by a horizontal mask (see [`AlphaMask`]).
    pub fn mask(mut self, mask: AlphaMask) -> Self {
        self.mask = Some(mask);
        self
    }
    /// CSS `opacity` on the text's own box. Glyphs never overlap, so this is exact.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }
    /// For a nowrap line stacked in a column: keep its fractional line-box height exact (and
    /// round its width) instead of the default exact width. See `ui-parity.md`.
    pub fn exact_height(mut self) -> Self {
        self.exact_height = true;
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
    boxes: Vec<Option<InlineBox>>,
    font_size: f32,
    line_height: LineHeight,
    base: RunStyle,
}

#[derive(Clone, PartialEq)]
struct RunStyle {
    len: usize,
    span: usize,
    family: Family,
    size: f32,
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

/// One shaped paragraph: char edges and glyphs in paragraph coordinates.
struct Para {
    range: Range<usize>,
    /// Byte offset (paragraph-relative) → left edge in px, including tracking and inline box
    /// extents, for every char boundary plus the end.
    edges: Vec<(usize, f32)>,
    glyphs: Vec<PaintGlyph>,
    /// Inline box spans: (span index, border-box left, border-box right, byte range).
    boxes: Vec<(usize, f32, f32, Range<usize>)>,
}

#[derive(Clone)]
struct PaintGlyph {
    /// Byte offset within the paragraph.
    index: usize,
    x: f32,
    src: GlyphSrc,
    run: usize,
}

/// Which painter owns a glyph.
#[derive(Clone, Copy)]
enum GlyphSrc {
    Gpui {
        font_id: FontId,
        glyph: gpui_kit::GlyphId,
        is_emoji: bool,
    },
    /// CoreText-shaped `system-ui` text (macOS).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Ct { font: usize, glyph: u16 },
}

/// One shaped run: (byte index, x, glyph) in run coordinates, and its advance width.
fn shape_run(window: &Window, run: &RunStyle, text: &str) -> (Vec<(usize, f32, GlyphSrc)>, f32) {
    #[cfg(target_os = "macos")]
    if run.family == Family::System {
        let (glyphs, width) = crate::ct_text::shape(text, run.weight, run.size);
        let out = glyphs
            .into_iter()
            .map(|g| {
                (
                    g.index,
                    g.x,
                    GlyphSrc::Ct {
                        font: g.font,
                        glyph: g.glyph,
                    },
                )
            })
            .collect();
        return (out, width);
    }
    let shaped = window.text_system().shape_line(
        text.to_string().into(),
        px(run.size),
        &[run.text_run(text.len())],
        None,
    );
    let mut out = Vec::new();
    for r in &shaped.runs {
        for g in &r.glyphs {
            out.push((
                g.index,
                f32::from(g.position.x),
                GlyphSrc::Gpui {
                    font_id: r.font_id,
                    glyph: g.id,
                    is_emoji: g.is_emoji,
                },
            ));
        }
    }
    (out, f32::from(shaped.width))
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

/// WebKit's line box for one font, as measured in the reference engine (see
/// `rust/docs/ui-parity.md`): ascent rounds and descent ceils to LayoutUnits (1/64 px); `normal` line
/// height is the rounded ascent + descent + line gap; a numeric line height truncates to a
/// LayoutUnit; the half-leading above the text floors to a LayoutUnit.
pub fn line_metrics(
    ascent: f32,
    descent: f32,
    line_gap: f32,
    font_size: f32,
    line_height: LineHeight,
) -> LineMetrics {
    let layout_unit = |v: f32| (v * 64.).floor() / 64.;
    let a = (ascent * 64.).round() / 64.;
    let d = (descent * 64.).ceil() / 64.;
    let h = match line_height {
        LineHeight::Normal => ascent.round() + descent.round() + line_gap.round(),
        LineHeight::Scale(m) => layout_unit(m * font_size),
        LineHeight::Px(p) => layout_unit(p),
    };
    let half_leading = layout_unit((h - (a + d)) / 2.);
    LineMetrics {
        height: h,
        baseline: half_leading + a,
        ascent: a,
        descent: d,
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
        let run_of = |s: &TextStyle, len: usize, span: usize| {
            let size = f32::from(s.size.unwrap_or(font_size));
            RunStyle {
                len,
                span,
                family: canonical_family(s.family.clone().unwrap_or(Family::Ui)),
                size,
                weight: s.weight.unwrap_or(400.),
                italic: s.italic.unwrap_or(false),
                features: if s.tabular.unwrap_or(false) {
                    tabular_nums()
                } else {
                    FontFeatures::default()
                },
                color: s.color.unwrap_or_default().opacity(self.opacity),
                tracking: s.tracking.unwrap_or(Tracking::Px(0.)).px(size),
                underline: s.underline.unwrap_or(false),
                line_through: s.line_through.unwrap_or(false),
            }
        };
        let mut content = String::new();
        let mut runs = Vec::new();
        for (i, s) in self.spans.iter().enumerate() {
            let style = s.style.over(&base_style);
            let mut t = s.text.to_string();
            if style.uppercase.unwrap_or(false) {
                t = t.to_uppercase();
            }
            if t.is_empty() {
                continue;
            }
            content.push_str(&t);
            runs.push(run_of(&style, t.len(), i));
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
            boxes: self.spans.iter().map(|s| s.inline_box).collect(),
            font_size: f32::from(font_size),
            line_height,
            base: run_of(&base_style, 0, usize::MAX),
        }
    }
}

/// GPUI names the inherited family only; restore the CSS stack for the app stacks.
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

/// Runs overlapping `range`, as (run index, absolute byte range).
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

/// Absolute byte range of each span's text in the content.
fn span_ranges(runs: &[RunStyle]) -> Vec<(usize, Range<usize>)> {
    let mut out: Vec<(usize, Range<usize>)> = Vec::new();
    let mut start = 0;
    for r in runs {
        let end = start + r.len;
        match out.last_mut() {
            Some((span, range)) if *span == r.span => range.end = end,
            _ => out.push((r.span, start..end)),
        }
        start = end;
    }
    out
}

fn shape_para(key: &Resolved, range: Range<usize>, window: &Window) -> Para {
    let spans = span_ranges(&key.runs);
    let mut pen = 0.;
    let mut edges = Vec::new();
    let mut glyphs = Vec::new();
    let mut boxes = Vec::new();
    let mut open_box: Option<(usize, f32)> = None;
    for (ri, abs) in run_slices(&key.runs, range.clone()) {
        let run = &key.runs[ri];
        let span_range = spans
            .iter()
            .find(|(s, _)| *s == run.span)
            .map(|(_, r)| r.clone());
        let ib = key.boxes.get(run.span).copied().flatten();
        if let (Some(b), Some(sr)) = (ib, &span_range)
            && abs.start == sr.start.max(range.start)
        {
            open_box = Some((run.span, pen + b.margin_left));
            pen += b.left();
        }
        let text = &key.content[abs.clone()];
        let (shaped, total) = shape_run(window, run, text);
        let mut glyph_x: Vec<(usize, f32)> = shaped.iter().map(|g| (g.0, g.1)).collect();
        glyph_x.sort_by_key(|(i, _)| *i);
        let chars: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let mut gi = 0;
        for (ci, &bi) in chars.iter().enumerate() {
            while gi < glyph_x.len() && glyph_x[gi].0 < bi {
                gi += 1;
            }
            let x = glyph_x.get(gi).map_or(total, |g| g.1);
            edges.push((
                abs.start - range.start + bi,
                pen + x + run.tracking * ci as f32,
            ));
        }
        for (index, x, src) in shaped {
            let ci = chars.partition_point(|&c| c < index);
            glyphs.push(PaintGlyph {
                index: abs.start - range.start + index,
                x: pen + x + run.tracking * ci as f32,
                src,
                run: ri,
            });
        }
        pen += total + run.tracking * chars.len() as f32;
        if let (Some(b), Some(sr)) = (ib, &span_range)
            && abs.end == sr.end.min(range.end)
            && let Some((span, x0)) = open_box.take()
        {
            pen += b.right();
            let local = sr.start.max(range.start) - range.start..abs.end - range.start;
            boxes.push((span, x0, pen - b.margin_right, local));
        }
    }
    edges.push((range.len(), pen));
    Para {
        range,
        edges,
        glyphs,
        boxes,
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
        let end = if trimmed_end == text.len() {
            // The paragraph end also carries a closing inline box's right extent.
            self.edges.last().map_or(0., |e| e.1)
        } else {
            self.x_at(trimmed_end)
        };
        end - self.line_start(r.start)
    }

    /// Left edge of a line starting at `byte`: the paragraph start keeps a leading inline
    /// box's left extent.
    fn line_start(&self, byte: usize) -> f32 {
        if byte == 0 { 0. } else { self.x_at(byte) }
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
    let line = |range: Range<usize>, width: f32| Line {
        para: pi,
        range,
        width,
        ellipsis: false,
    };
    let Some(max) = max else {
        return vec![line(0..text.len(), para.width(text, 0..text.len()))];
    };
    let opportunities: Vec<(usize, BreakOpportunity)> = linebreaks(text).collect();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut last_fit: Option<usize> = None;
    let mut i = 0;
    while i < opportunities.len() {
        let (pos, kind) = opportunities[i];
        let w = para.width(text, start..pos);
        if w <= max + eps {
            if kind == BreakOpportunity::Mandatory {
                lines.push(line(start..pos, w));
                start = pos;
                last_fit = None;
            } else {
                last_fit = Some(pos);
            }
            i += 1;
            continue;
        }
        if let Some(b) = last_fit.take() {
            lines.push(line(start..b, para.width(text, start..b)));
            start = b;
            continue;
        }
        if anywhere {
            // Split the overlong word after the last char that still fits.
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
            lines.push(line(start..cut, para.width(text, start..cut)));
            start = cut;
            continue;
        }
        // An unbreakable word wider than the box overflows on its own line.
        if kind == BreakOpportunity::Mandatory {
            lines.push(line(start..pos, w));
            start = pos;
        } else {
            last_fit = Some(pos);
        }
        i += 1;
    }
    if start < text.len() || lines.is_empty() {
        lines.push(line(start..text.len(), para.width(text, start..text.len())));
    }
    lines
}

/// Unrounded ascent, descent and line gap of a run's primary font.
fn font_extent(run: &RunStyle, size: f32, window: &Window) -> (f32, f32, f32) {
    #[cfg(target_os = "macos")]
    if run.family == Family::System {
        return crate::ct_text::metrics(run.weight, size);
    }
    let ts = window.text_system();
    let id = ts.resolve_font(
        &run.family
            .font(run.weight, run.italic, FontFeatures::default()),
    );
    (
        ts.ascent(id, px(size)).into(),
        -f32::from(ts.descent(id, px(size))),
        crate::font_metrics::line_gap(&run.family, size),
    )
}

/// The content-area extent of an inline box's font: WebKit's rounded ascent and descent.
fn rounded_extent(run: &RunStyle, window: &Window) -> (f32, f32) {
    let (a, d, _) = font_extent(run, run.size, window);
    ((a * 64.).round() / 64., (d * 64.).ceil() / 64.)
}

impl Laid {
    fn build(key: Resolved, window: &Window) -> Laid {
        let (a, d, g) = font_extent(&key.base, key.font_size, window);
        let metrics = line_metrics(a, d, g, key.font_size, key.line_height);
        let mut paras = Vec::new();
        let mut start = 0;
        for piece in key.content.split('\n') {
            paras.push(shape_para(&key, start..start + piece.len(), window));
            start += piece.len() + 1;
        }
        Laid {
            key,
            paras,
            metrics,
            ellipsis: None,
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
        self.lines = lines;
    }

    fn ellipsis_glyphs(&self, window: &Window) -> (Vec<PaintGlyph>, f32) {
        let base = &self.key.base;
        let mut run = base.clone();
        run.size = self.key.font_size;
        let (shaped, width) = shape_run(window, &run, "…");
        let glyphs = shaped
            .into_iter()
            .map(|(_, x, src)| PaintGlyph {
                index: 0,
                x,
                src,
                run: usize::MAX,
            })
            .collect();
        (glyphs, width + base.tracking)
    }

    fn size(&self, scale: f32) -> gpui_kit::Size<Pixels> {
        if self.key.content.is_empty() {
            return size(px(0.), px(0.));
        }
        let w = self.lines.iter().map(|l| l.width).fold(0., f32::max);
        let h = self.metrics.height * self.lines.len() as f32;
        size(px(snap(w, scale)), px(snap(h, scale)))
    }

    /// Width of the widest line and the total height, unrounded, after a layout at `width`.
    fn measured(&self) -> (f32, f32) {
        // WebKit stores an inline text width as `LayoutUnit::fromFloatCeil`.
        let w = (self.lines.iter().map(|l| l.width).fold(0., f32::max) * 64.).ceil() / 64.;
        (w, self.metrics.height * self.lines.len() as f32)
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

impl Text {
    /// Lay the text out against `width` (or unconstrained) and return its unrounded
    /// `(width, height)` in CSS px and the line metrics, as the element would.
    pub fn measure(&self, width: Option<f32>, window: &Window) -> (f32, f32, LineMetrics) {
        let mut laid = Laid::build(self.resolve(window), window);
        laid.relayout(
            width,
            self.white_space,
            self.anywhere,
            self.clamp,
            self.ellipsis,
            window,
        );
        let (w, h) = laid.measured();
        (w, h, laid.metrics)
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
        cx: &mut App,
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
        let mut style = Style::default();
        if truncate {
            // `overflow: hidden` makes the automatic minimum size zero in a flex row.
            style.min_size.width = px(0.).into();
        }
        if ws == WhiteSpace::NoWrap && clamp.is_none() {
            // A single unwrapped line has a fixed intrinsic size. GPUI rounds every authored
            // or measured length to device pixels, which would push later siblings off the
            // fractional positions WebKit gives them; `aspect_ratio` reaches Taffy unrounded,
            // so one axis is authored (rounded) and the other derived exactly from it.
            let exact = layout.borrow_mut().as_mut().map(|laid| {
                laid.relayout(None, ws, anywhere, clamp, false, window);
                laid.measured()
            });
            if let Some((w, h)) = exact
                && w > 0.
                && h > 0.
            {
                let scale = window.scale_factor();
                if self.exact_height {
                    let ws = snap(w, scale).max(1. / scale);
                    style.size.width = px(ws).into();
                    style.aspect_ratio = Some(ws / h);
                } else {
                    let hs = snap(h, scale).max(1. / scale);
                    style.size.height = px(hs).into();
                    style.aspect_ratio = Some(w / hs);
                }
                return (window.request_layout(style, [], cx), TextState { layout });
            }
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
        _: &mut App,
    ) {
        let slot = state.layout.borrow();
        let Some(laid) = slot.as_ref() else { return };
        let clip = self.ellipsis || self.clamp.is_some();
        let align = self.align;
        if clip {
            window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
                paint_lines(laid, bounds, align, self.mask.as_ref(), window)
            });
        } else {
            paint_lines(laid, bounds, align, self.mask.as_ref(), window);
        }
    }
}

/// Block text stacked vertically (a heading over its description, a label over a hint). The
/// blocks lay out together at exact LayoutUnit offsets and the stack's height rounds once, as
/// WebKit stacks block boxes; separate [`Text`] elements would each round to a device pixel.
pub struct TextStack {
    blocks: Vec<(f32, Text)>,
    layouts: Rc<RefCell<Vec<Laid>>>,
}

/// A stack of text blocks; each entry is (margin above, block).
pub fn text_stack(blocks: Vec<(f32, Text)>) -> TextStack {
    TextStack {
        blocks,
        layouts: Rc::default(),
    }
}

impl IntoElement for TextStack {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl TextStack {
    fn relayout(
        &self,
        layouts: &mut [Laid],
        width: Option<f32>,
        final_pass: bool,
        window: &Window,
    ) {
        for ((_, t), laid) in self.blocks.iter().zip(layouts.iter_mut()) {
            let w = if t.ellipsis && !final_pass {
                None
            } else {
                width
            };
            laid.relayout(
                w,
                t.white_space,
                t.anywhere,
                t.clamp,
                t.ellipsis && final_pass,
                window,
            );
        }
    }
}

impl Element for TextStack {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
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
    ) -> (LayoutId, ()) {
        let built: Vec<Laid> = self
            .blocks
            .iter()
            .map(|(_, t)| Laid::build(t.resolve(window), window))
            .collect();
        *self.layouts.borrow_mut() = built;
        let layouts = self.layouts.clone();
        let blocks: Vec<(f32, WhiteSpace, bool, Option<usize>, bool)> = self
            .blocks
            .iter()
            .map(|(g, t)| (*g, t.white_space, t.anywhere, t.clamp, t.ellipsis))
            .collect();
        let id =
            window.request_measured_layout(Style::default(), move |known, available, window, _| {
                let mut layouts = layouts.borrow_mut();
                let width = known.width.map(f32::from).or(match available.width {
                    AvailableSpace::Definite(w) => Some(w.into()),
                    AvailableSpace::MinContent => Some(0.),
                    AvailableSpace::MaxContent => None,
                });
                let (mut w, mut h): (f32, f32) = (0., 0.);
                for ((gap, ws, anywhere, clamp, truncate), laid) in
                    blocks.iter().zip(layouts.iter_mut())
                {
                    laid.relayout(
                        if *truncate { None } else { width },
                        *ws,
                        *anywhere,
                        *clamp,
                        false,
                        window,
                    );
                    let (lw, lh) = laid.measured();
                    h += gap + lh;
                    w = w.max(lw);
                }
                let scale = window.scale_factor();
                size(px(snap(w, scale)), px(snap(h, scale)))
            });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let mut layouts = self.layouts.borrow_mut();
        self.relayout(&mut layouts, Some(bounds.size.width.into()), true, window);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let layouts = self.layouts.borrow();
        let mut y = f32::from(bounds.origin.y);
        for ((gap, t), laid) in self.blocks.iter().zip(layouts.iter()) {
            y += gap;
            let (_, h) = laid.measured();
            let b = Bounds {
                origin: point(bounds.origin.x, px(y)),
                size: size(bounds.size.width, px(h)),
            };
            if t.ellipsis || t.clamp.is_some() {
                window.with_content_mask(Some(gpui_kit::ContentMask { bounds: b }), |window| {
                    paint_lines(laid, b, t.align, t.mask.as_ref(), window)
                });
            } else {
                paint_lines(laid, b, t.align, t.mask.as_ref(), window);
            }
            y += h;
        }
    }
}

fn paint_lines(
    laid: &Laid,
    bounds: Bounds<Pixels>,
    align: Align,
    mask: Option<&AlphaMask>,
    window: &mut Window,
) {
    let scale = window.scale_factor();
    let box_w = f32::from(bounds.size.width);
    for (li, line) in laid.lines.iter().enumerate() {
        let p = &laid.paras[line.para];
        let top = f32::from(bounds.origin.y) + laid.metrics.height * li as f32;
        let baseline = snap(top + laid.metrics.baseline, scale);
        let start_x = p.line_start(line.range.start);
        let offset = match align {
            Align::Left => 0.,
            Align::Center => (box_w - line.width) / 2.,
            Align::Right => box_w - line.width,
        };
        let left = f32::from(bounds.origin.x) + offset - start_x;
        // Inline box backgrounds and borders paint under the text.
        for (span, x0, x1, r) in &p.boxes {
            if r.end <= line.range.start || r.start >= line.range.end.max(line.range.start + 1) {
                continue;
            }
            let Some(b) = laid.key.boxes.get(*span).copied().flatten() else {
                continue;
            };
            let Some(run) = laid.key.runs.iter().find(|r| r.span == *span) else {
                continue;
            };
            let (a, d) = rounded_extent(run, window);
            let y0 = baseline - a - b.pad_top - b.border;
            let y1 = baseline + d + b.pad_bottom + b.border;
            let rect =
                Bounds::from_corners(point(px(left + x0), px(y0)), point(px(left + x1), px(y1)));
            window.paint_quad(quad(
                rect,
                px(b.radius),
                b.background.unwrap_or_default(),
                px(b.border),
                b.border_color,
                BorderStyle::Solid,
            ));
        }
        let text = &laid.key.content[p.range.clone()];
        for g in p.glyphs.iter().filter(|g| line.range.contains(&g.index)) {
            let run = &laid.key.runs[g.run];
            let mut color = run.color;
            if let Some(mask) = mask {
                let next = g.index + text[g.index..].chars().next().map_or(1, char::len_utf8);
                let center = left + (p.x_at(g.index) + p.x_at(next)) / 2.;
                color = color.opacity(mask(center));
            }
            paint_one(
                window,
                point(px(left + g.x), px(baseline)),
                g,
                run.size,
                color,
            );
        }
        for (ri, r) in run_slices(
            &laid.key.runs,
            p.range.start + line.range.start..p.range.start + line.range.end,
        ) {
            let run = &laid.key.runs[ri];
            if !(run.underline || run.line_through) {
                continue;
            }
            let x0 = left + p.x_at(r.start - p.range.start);
            let x1 = left + p.x_at(r.end - p.range.start);
            let deco = crate::font_metrics::decorations(&run.family, run.size);
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
            let end_x = left + p.x_at(line.range.end);
            for g in glyphs {
                let origin = point(px(end_x + g.x), px(baseline));
                paint_one(window, origin, g, laid.key.font_size, laid.key.base.color);
            }
        }
    }
}

fn paint_one(
    window: &mut Window,
    origin: gpui_kit::Point<Pixels>,
    g: &PaintGlyph,
    font_size: f32,
    color: Hsla,
) {
    match g.src {
        GlyphSrc::Gpui {
            font_id,
            glyph,
            is_emoji,
        } => crate::glyph::paint(window, font_id, glyph, font_size, origin, color, is_emoji),
        #[cfg(target_os = "macos")]
        GlyphSrc::Ct { font, glyph } => {
            crate::glyph::paint_ct(window, font, glyph, font_size, origin, color)
        }
        #[cfg(not(target_os = "macos"))]
        GlyphSrc::Ct { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webkit_line_box_metrics() {
        // Values WebKit reports for these samples (tools/ui-parity text/calibration).
        let inter = |size: f32| (size * 1984. / 2048., size * 494. / 2048.);
        let check = |(a, d): (f32, f32), size: f32, lh: LineHeight, height: f32, baseline: f32| {
            let m = line_metrics(a, d, 0., size, lh);
            assert_eq!((m.height, m.baseline), (height, baseline), "{size} {lh:?}");
        };
        check(inter(13.), 13., LineHeight::Normal, 16., 12.71875);
        check(
            inter(13.),
            13.,
            LineHeight::Scale(1.45),
            18.84375,
            14.140625,
        );
        check(inter(12.), 12., LineHeight::Normal, 15., 11.859375);
        check(inter(12.), 12., LineHeight::Scale(1.4), 16.796875, 12.75);
        check(inter(11.), 11., LineHeight::Scale(1.), 11., 9.5);
        // JetBrains Mono: ascent 1020, descent 300 per 1000.
        check(
            (12. * 1.02, 12. * 0.3),
            12.,
            LineHeight::Normal,
            16.,
            12.3125,
        );
        check(
            (9. * 1.02, 9. * 0.3),
            9.,
            LineHeight::Scale(1.),
            9.,
            7.734375,
        );
        check(
            (10.5 * 1.02, 10.5 * 0.3),
            10.5,
            LineHeight::Scale(1.45),
            15.21875,
            11.375,
        );
    }

    fn run(len: usize, span: usize) -> RunStyle {
        RunStyle {
            len,
            span,
            family: Family::Ui,
            size: 13.,
            weight: 400.,
            italic: false,
            features: FontFeatures::default(),
            color: Hsla::default(),
            tracking: 0.,
            underline: false,
            line_through: false,
        }
    }

    #[test]
    fn collapses_white_space_across_runs() {
        let (s, runs) = collapse_spaces("  a  b ", vec![run(4, 0), run(3, 1)]);
        assert_eq!(s, "a b");
        assert_eq!(runs.iter().map(|r| r.len).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(runs.iter().map(|r| r.span).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn span_ranges_merge_runs_of_one_span() {
        let ranges = span_ranges(&[run(2, 0), run(3, 0), run(1, 2)]);
        assert_eq!(ranges, vec![(0, 0..5), (2, 5..6)]);
    }
}
