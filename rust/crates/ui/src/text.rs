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
use smallvec::SmallVec;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
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
    /// `text-underline-offset`: the underline's top this far below the baseline.
    pub underline_offset: Option<f32>,
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
            underline_offset: self.underline_offset.or(base.underline_offset),
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
    /// `text-underline-offset` in px.
    fn underline_offset(mut self, px: f32) -> Self {
        self.text_style_mut().underline_offset = Some(px);
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
    opacity: f32,
    mask: Option<AlphaMask>,
    field: bool,
    links: Vec<(usize, SpanClick)>,
    layout: Rc<RefCell<Option<Laid>>>,
    /// The view that built this element, while layouts are counted (`guard::count_layouts`).
    owner: Option<&'static str>,
    /// Selectable inside a [`crate::selection`] scope: its key, and the separator copied after
    /// its text.
    selectable: Option<(u64, &'static str)>,
}

/// What clicking an inline link span does.
pub type SpanClick = Rc<dyn Fn(&mut Window, &mut App)>;

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
            opacity: 1.,
            mask: None,
            field: false,
            links: Vec::new(),
            layout: Rc::default(),
            owner: crate::guard::layout_owner(),
            selectable: None,
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
    /// Selectable across elements in the enclosing [`crate::selection`] scope. `key` must be
    /// stable and unique in the scope; `sep` follows this text when a copy joins it to more.
    pub fn selectable(mut self, key: u64, sep: &'static str) -> Self {
        self.selectable = Some((key, sep));
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
    /// Text in a text field's inner editor (a placeholder). When the font's content area is
    /// taller than its line (a negative half-leading), WebKit sets the editor's text that much
    /// higher again than block text; measured in WebKit across Inter sizes (within 1/64 px).
    pub fn field(mut self) -> Self {
        self.field = true;
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
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
    pub fn child_span(mut self, s: Span) -> Self {
        self.spans.push(s);
        self
    }
    /// Make span `index` an inline link button (`.lnk` in running text): a pointer over its
    /// boxes, underlined while hovered, `f` on click.
    pub fn on_click_span(
        mut self,
        index: usize,
        f: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.links.push((index, Rc::new(f)));
        self
    }
}

/// The boxes span `index` covers, one per line it is on: its text's horizontal extent and
/// the line box.
fn span_rects(
    laid: &Laid,
    bounds: Bounds<Pixels>,
    align: Align,
    index: usize,
) -> Vec<Bounds<Pixels>> {
    let box_w = f32::from(bounds.size.width);
    let mut out = Vec::new();
    for line in &laid.lines {
        let p = &laid.paras[line.para];
        let start_x = p.line_start(line.range.start);
        let offset = match align {
            Align::Left => 0.,
            Align::Center => (box_w - line.width) / 2.,
            Align::Right => box_w - line.width,
        };
        let left = f32::from(bounds.origin.x) + offset - start_x;
        let top = f32::from(bounds.origin.y) + line.top;
        for (ri, r) in run_slices(
            &laid.key.runs,
            p.range.start + line.range.start..p.range.start + line.range.end,
        ) {
            if laid.key.runs[ri].span != index {
                continue;
            }
            let x0 = left + p.x_at(r.start - p.range.start);
            let x1 = left + p.x_at(r.end - p.range.start);
            out.push(Bounds::from_corners(
                point(px(x0), px(top)),
                point(px(x1), px(top + line.height)),
            ));
        }
    }
    out
}

/// A selectable text's geometry for its scope, read from its current layout.
struct SelectGeometry {
    layout: Rc<RefCell<Option<Laid>>>,
    bounds: Bounds<Pixels>,
    align: Align,
}

impl SelectGeometry {
    /// Each line with its paragraph, global byte range, top, height and left edge.
    fn lines(&self, laid: &Laid) -> Vec<(usize, Range<usize>, f32, f32, f32)> {
        let box_w = f32::from(self.bounds.size.width);
        laid.lines
            .iter()
            .map(|line| {
                let p = &laid.paras[line.para];
                let offset = match self.align {
                    Align::Left => 0.,
                    Align::Center => (box_w - line.width) / 2.,
                    Align::Right => box_w - line.width,
                };
                let left =
                    f32::from(self.bounds.origin.x) + offset - p.line_start(line.range.start);
                (
                    line.para,
                    p.range.start + line.range.start..p.range.start + line.range.end,
                    f32::from(self.bounds.origin.y) + line.top,
                    line.height,
                    left,
                )
            })
            .collect()
    }
}

impl crate::selection::Geometry for SelectGeometry {
    fn text(&self) -> String {
        self.layout
            .borrow()
            .as_ref()
            .map(|l| l.key.content.to_string())
            .unwrap_or_default()
    }

    fn offset_at(&self, pt: gpui_kit::Point<Pixels>) -> usize {
        let slot = self.layout.borrow();
        let Some(laid) = slot.as_ref() else { return 0 };
        let lines = self.lines(laid);
        let y = f32::from(pt.y);
        let Some(line) = lines.iter().find(|l| y < l.2 + l.3).or(lines.last()) else {
            return 0;
        };
        let (para, range, top, _, left) = line.clone();
        if y < top {
            // Above the text: its start, as WebKit places a caret above the first line.
            return range.start;
        }
        let p = &laid.paras[para];
        let x = f32::from(pt.x) - left;
        // The char boundary nearest `x` within the line.
        let mut best = (range.start, f32::INFINITY);
        for &(b, ex) in &p.edges {
            let g = p.range.start + b;
            if g < range.start || g > range.end {
                continue;
            }
            let d = (ex - x).abs();
            if d < best.1 {
                best = (g, d);
            }
        }
        best.0
    }

    fn rects(&self, r: Range<usize>) -> Vec<Bounds<Pixels>> {
        let slot = self.layout.borrow();
        let Some(laid) = slot.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (para, range, top, height, left) in self.lines(laid) {
            let (s, e) = (r.start.max(range.start), r.end.min(range.end));
            if s >= e && !(r.start <= range.start && r.end > range.end) {
                continue;
            }
            let p = &laid.paras[para];
            let x0 = left + p.x_at(s.min(e) - p.range.start);
            let x1 = left + p.x_at(e - p.range.start);
            out.push(Bounds::from_corners(
                point(px(x0), px(top)),
                point(px(x1.max(x0)), px(top + height)),
            ));
        }
        out
    }
}

/// Hitboxes for the link spans of a laid block.
fn link_hitboxes(
    laid: &Laid,
    bounds: Bounds<Pixels>,
    align: Align,
    links: &[(usize, SpanClick)],
    window: &mut Window,
) -> Vec<(usize, gpui_kit::Hitbox)> {
    let mut out = Vec::new();
    for (i, (span, _)) in links.iter().enumerate() {
        for r in span_rects(laid, bounds, align, *span) {
            out.push((i, window.insert_hitbox(r, gpui_kit::HitboxBehavior::Normal)));
        }
    }
    out
}

/// Cursor, hover repaint and click for link hitboxes; the spans hovered now.
fn paint_links(
    links: &[(usize, SpanClick)],
    hits: &[(usize, gpui_kit::Hitbox)],
    window: &mut Window,
) -> Vec<usize> {
    let mut hovered = Vec::new();
    for (i, hb) in hits {
        window.set_cursor_style(gpui_kit::CursorStyle::PointingHand, hb);
        let was = hb.is_hovered(window);
        if was {
            hovered.push(links[*i].0);
        }
        let h = hb.clone();
        window.on_mouse_event(move |_: &gpui_kit::MouseMoveEvent, phase, window, _| {
            if phase == gpui_kit::DispatchPhase::Bubble && h.is_hovered(window) != was {
                window.refresh();
            }
        });
        let h = hb.clone();
        let f = links[*i].1.clone();
        window.on_mouse_event(move |e: &gpui_kit::MouseUpEvent, phase, window, cx| {
            if phase == gpui_kit::DispatchPhase::Bubble
                && e.button == gpui_kit::MouseButton::Left
                && h.is_hovered(window)
            {
                f(window, cx);
                cx.stop_propagation();
            }
        });
    }
    hovered
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
    field: bool,
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
    underline_offset: Option<f32>,
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
    /// Where each inline box opens (byte, x before its left margin): a line that starts there
    /// starts with the box's left margin, border and padding.
    box_starts: Vec<(usize, f32)>,
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
        /// The bundled face of the run's own font, when the glyph is from it (not a fallback).
        face: Option<usize>,
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
    let text_run = run.text_run(text.len());
    // Which bundled face draws the run's own glyphs: the run's family and weight decide. GPUI
    // cannot say reliably (several requested fonts can share an id, and CoreText may shape with
    // another loaded copy of the same face under a new id), so a shaped run is ours when its
    // glyphs are the ones our face maps their characters to (features may substitute some); a
    // fallback font's are not.
    let face = crate::fonts::match_face(&text_run.font.family, text_run.font.weight.0);
    let parsed = face.and_then(|f| ttf_parser::Face::parse(crate::fonts::FACES[f].2, 0).ok());
    let own = |r: &gpui_kit::ShapedRun| {
        parsed.as_ref().is_some_and(|p| {
            r.glyphs.iter().any(|g| {
                text.get(g.index..)
                    .and_then(|t| t.chars().next())
                    .and_then(|c| p.glyph_index(c))
                    .is_some_and(|id| u32::from(id.0) == g.id.0)
            })
        })
    };
    let shaped =
        window
            .text_system()
            .shape_line(text.to_string().into(), px(run.size), &[text_run], None);
    let mut out = Vec::new();
    for r in &shaped.runs {
        let ours = own(r);
        for g in &r.glyphs {
            out.push((
                // Everything downstream slices the text at glyph indices.
                text.floor_char_boundary(g.index),
                f32::from(g.position.x),
                GlyphSrc::Gpui {
                    font_id: r.font_id,
                    face: face.filter(|_| ours),
                    glyph: g.id,
                    is_emoji: g.is_emoji,
                },
            ));
        }
    }
    (out, f32::from(shaped.width))
}

#[derive(Clone)]
struct Line {
    para: usize,
    range: Range<usize>,
    /// Visible width (trailing spaces hang).
    width: f32,
    ellipsis: bool,
    /// Line box top below the text top, its height and its baseline below its top.
    top: f32,
    height: f32,
    baseline: f32,
}

struct Laid {
    key: Resolved,
    paras: Rc<Vec<Para>>,
    /// The strut: the line box of the text's own font.
    metrics: LineMetrics,
    /// Each run's byte range and how far its inline box reaches above and below the baseline.
    extents: Rc<Vec<(Range<usize>, f32, f32)>>,
    ellipsis: Option<(Vec<PaintGlyph>, f32)>,
    lines: Vec<Line>,
    /// The widths the last line breaking holds for (`None`: only its exact width).
    valid_widths: Option<(f32, f32)>,
    /// Line breaks already computed for this shaping, by width and settings (shared with
    /// [`SHAPED`], so a text drawn again next frame breaks no lines).
    breaks: Breaks,
}

/// `(width bits, white-space, anywhere, clamp, truncate, scale bits)`.
type BreakKey = (Option<u32>, WhiteSpace, bool, Option<usize>, bool, u32);

/// One line breaking of a shaping: its key, the widths it holds for, the lines and the
/// ellipsis glyphs.
struct Broken {
    key: BreakKey,
    valid: Option<(f32, f32)>,
    lines: Vec<Line>,
    ellipsis: Option<(Vec<PaintGlyph>, f32)>,
}

/// A shaping's line breaks, most recent last.
type Breaks = Rc<RefCell<Vec<Broken>>>;

/// How many widths a shaping remembers (a flex pass probes min-content, max-content and the
/// final width; a resize adds one).
const BREAKS_PER_TEXT: usize = 8;

/// Shaped text across frames, by resolved content and style. Views rebuild their elements
/// every render, so without it every visible text is shaped and broken again each frame.
/// Byte-budgeted (see `SHAPED_MAX_BYTES`), least recently used out first.
#[derive(Default)]
struct ShapedCache {
    entries: HashMap<u64, ShapedEntry>,
    order: BTreeMap<u64, u64>,
    stamp: u64,
    bytes: usize,
}

struct ShapedEntry {
    key: Resolved,
    scale: u32,
    paras: Rc<Vec<Para>>,
    metrics: LineMetrics,
    extents: Rc<Vec<(Range<usize>, f32, f32)>>,
    breaks: Breaks,
    bytes: usize,
    stamp: u64,
}

/// The shaped-text budget: a long transcript's visible blocks plus the window's chrome come
/// to a few MiB; this keeps every text of the last several screens.
pub const SHAPED_MAX_BYTES: usize = 48 * 1024 * 1024;

thread_local! {
    static SHAPED: RefCell<ShapedCache> = RefCell::default();
}

/// Drops every cached shaping (font ids belong to one app's text system).
pub fn clear_shaped_cache() {
    SHAPED.with_borrow_mut(|c| *c = ShapedCache::default());
}

/// `(entries, bytes)` of the shaped-text cache.
pub fn shaped_cache_stats() -> (usize, usize) {
    SHAPED.with_borrow(|c| (c.entries.len(), c.bytes))
}

fn resolved_hash(k: &Resolved) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    k.content.as_ref().hash(&mut h);
    k.font_size.to_bits().hash(&mut h);
    k.field.hash(&mut h);
    for r in std::iter::once(&k.base).chain(&k.runs) {
        (
            r.len,
            r.span,
            &r.family,
            r.size.to_bits(),
            r.weight.to_bits(),
            r.italic,
        )
            .hash(&mut h);
        (
            r.color.h.to_bits(),
            r.color.s.to_bits(),
            r.color.l.to_bits(),
            r.color.a.to_bits(),
        )
            .hash(&mut h);
    }
    h.finish()
}

fn para_bytes(paras: &[Para]) -> usize {
    paras
        .iter()
        .map(|p| {
            std::mem::size_of::<Para>()
                + p.edges.capacity() * std::mem::size_of::<(usize, f32)>()
                + p.glyphs.capacity() * std::mem::size_of::<PaintGlyph>()
                + p.boxes.capacity() * std::mem::size_of::<(usize, f32, f32, Range<usize>)>()
                + p.box_starts.capacity() * std::mem::size_of::<(usize, f32)>()
        })
        .sum()
}

impl ShapedCache {
    fn get(&mut self, hash: u64, key: &Resolved, scale: u32) -> Option<Laid> {
        let e = self
            .entries
            .get_mut(&hash)
            .filter(|e| e.scale == scale && e.key == *key)?;
        // Lazy LRU: a hit only restamps; eviction requeues entries used since.
        self.stamp += 1;
        e.stamp = self.stamp;
        Some(Laid {
            key: e.key.clone(),
            paras: e.paras.clone(),
            metrics: e.metrics,
            extents: e.extents.clone(),
            ellipsis: None,
            lines: Vec::new(),
            valid_widths: None,
            breaks: e.breaks.clone(),
        })
    }

    fn insert(&mut self, hash: u64, laid: &Laid, scale: u32) {
        if let Some(old) = self.entries.remove(&hash) {
            self.order.remove(&old.stamp);
            self.bytes -= old.bytes;
        }
        let bytes = 128
            + laid.key.content.len()
            + para_bytes(&laid.paras)
            + laid.extents.len() * 32
            // Up to BREAKS_PER_TEXT line sets.
            + BREAKS_PER_TEXT * 4 * std::mem::size_of::<Line>() * laid.paras.len().max(1);
        self.stamp += 1;
        self.order.insert(self.stamp, hash);
        self.entries.insert(
            hash,
            ShapedEntry {
                key: laid.key.clone(),
                scale,
                paras: laid.paras.clone(),
                metrics: laid.metrics,
                extents: laid.extents.clone(),
                breaks: laid.breaks.clone(),
                bytes,
                stamp: self.stamp,
            },
        );
        self.bytes += bytes;
        while self.bytes > SHAPED_MAX_BYTES && self.entries.len() > 1 {
            let Some((at, oldest)) = self.order.pop_first() else {
                break;
            };
            match self.entries.get(&oldest) {
                Some(e) if e.stamp != at => {
                    let used = e.stamp;
                    self.order.insert(used, oldest);
                }
                Some(_) => {
                    if let Some(e) = self.entries.remove(&oldest) {
                        self.bytes -= e.bytes;
                    }
                }
                None => {}
            }
        }
    }
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

/// A text field editor's line (see [`Text::field`]): a negative half-leading lifts the
/// baseline once more.
fn field_metrics(mut m: LineMetrics) -> LineMetrics {
    m.baseline += (m.baseline - m.ascent).min(0.);
    m
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
            underline_offset: None,
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
                underline_offset: s.underline_offset,
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
            field: self.field,
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
            n if n == crate::fonts::system_family().as_ref() => Family::System,
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
    let mut box_starts = Vec::new();
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
            box_starts.push((abs.start - range.start, pen));
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
        box_starts,
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

    /// Left edge of a line starting at `byte`. An inline box opening there brings its left
    /// margin, border and padding onto the line, as in CSS.
    fn line_start(&self, byte: usize) -> f32 {
        if byte == 0 {
            return 0.;
        }
        self.box_starts
            .iter()
            .find(|(b, _)| *b == byte)
            .map_or_else(|| self.x_at(byte), |(_, x)| *x)
    }
}

/// Greedy line breaking at UAX #14 opportunities, as WebKit breaks a block's lines: a line takes
/// opportunities while `width(start..pos)` fits `max` within `eps`, a mandatory break ends it,
/// and a word wider than the line overflows on its own line or, with `anywhere`
/// (`overflow-wrap: anywhere`), splits after the last char that fits (at least one). A
/// negative `max` (a box narrower than its margins) is zero. `width` is called only for those
/// comparisons. The ranges tile `text`. `recur_content`'s line mirror and the chat's flow break
/// with this too, so they match what paints.
pub fn break_lines(
    text: &str,
    max: f32,
    eps: f32,
    anywhere: bool,
    mut width: impl FnMut(Range<usize>) -> f32,
) -> Vec<Range<usize>> {
    let max = max.max(0.);
    let mut fits = |r: Range<usize>| width(r) <= max + eps;
    let opportunities: Vec<(usize, BreakOpportunity)> = linebreaks(text).collect();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut last_fit: Option<usize> = None;
    let mut i = 0;
    while i < opportunities.len() {
        let (pos, kind) = opportunities[i];
        // An overlong split can end exactly at this opportunity: nothing is left before it.
        if pos <= start {
            i += 1;
            continue;
        }
        if fits(start..pos) {
            if kind == BreakOpportunity::Mandatory {
                lines.push(start..pos);
                start = pos;
                last_fit = None;
            } else {
                last_fit = Some(pos);
            }
            i += 1;
            continue;
        }
        if let Some(b) = last_fit.take() {
            lines.push(start..b);
            start = b;
            continue;
        }
        if anywhere {
            let mut cut = start;
            for (bi, ch) in text[start..pos].char_indices() {
                let end = start + bi + ch.len_utf8();
                if !fits(start..end) {
                    break;
                }
                cut = end;
            }
            if cut == start {
                cut = start + text[start..].chars().next().map_or(1, char::len_utf8);
            }
            lines.push(start..cut);
            start = cut;
            continue;
        }
        // An unbreakable word wider than the box overflows on its own line.
        if kind == BreakOpportunity::Mandatory {
            lines.push(start..pos);
            start = pos;
        } else {
            last_fit = Some(pos);
        }
        i += 1;
    }
    if start < text.len() || lines.is_empty() {
        lines.push(start..text.len());
    }
    lines
}

/// A paragraph's lines at `max` ([`break_lines`]). `range` narrows to the widths that break the
/// same way: every width compared against `max` moves one of its ends.
fn break_para(
    para: &Para,
    pi: usize,
    text: &str,
    max: Option<f32>,
    anywhere: bool,
    eps: f32,
    range: &mut (f32, f32),
) -> Vec<Line> {
    let line = |range: Range<usize>, width: f32| Line {
        para: pi,
        range,
        width,
        ellipsis: false,
        top: 0.,
        height: 0.,
        baseline: 0.,
    };
    let Some(max) = max else {
        return vec![line(0..text.len(), para.width(text, 0..text.len()))];
    };
    let limit = max.max(0.);
    break_lines(text, max, eps, anywhere, |r| {
        let w = para.width(text, r);
        if w <= limit + eps {
            range.0 = range.0.max(w - eps);
        } else {
            range.1 = range.1.min(w - eps);
        }
        w
    })
    .into_iter()
    .map(|r| {
        let w = para.width(text, r.clone());
        line(r, w)
    })
    .collect()
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

/// The line box a strut of `family` at `size` and `line_height` makes on its own: where an
/// inline replaced element (an `<svg>` in a `<span>`) sits outside a text run.
pub fn strut_metrics(
    family: &Family,
    size: f32,
    line_height: LineHeight,
    window: &Window,
) -> LineMetrics {
    let ts = window.text_system();
    let id = ts.resolve_font(&family.font(400., false, FontFeatures::default()));
    line_metrics(
        ts.ascent(id, px(size)).into(),
        -f32::from(ts.descent(id, px(size))),
        crate::font_metrics::line_gap(family, size),
        size,
        line_height,
    )
}

/// The content-area extent of an inline box's font: WebKit's rounded ascent and descent.
fn rounded_extent(run: &RunStyle, window: &Window) -> (f32, f32) {
    let (a, d, _) = font_extent(run, run.size, window);
    ((a * 64.).round() / 64., (d * 64.).ceil() / 64.)
}

impl Laid {
    /// Shapes `key` (or takes the shaping from [`SHAPED`]), or lays it out as empty if
    /// shaping panicked.
    fn build(key: Resolved, window: &Window) -> Laid {
        let scale = window.scale_factor().to_bits();
        let hash = resolved_hash(&key) ^ u64::from(scale);
        if let Some(hit) = SHAPED.with_borrow_mut(|c| c.get(hash, &key, scale)) {
            return hit;
        }
        let fallback = key.clone();
        match crate::guard::catch("text shaping", || Self::shape(key, window)) {
            Some(laid) => {
                SHAPED.with_borrow_mut(|c| c.insert(hash, &laid, scale));
                laid
            }
            None => Laid {
                key: fallback,
                paras: Rc::default(),
                metrics: LineMetrics {
                    height: 0.,
                    baseline: 0.,
                    ascent: 0.,
                    descent: 0.,
                },
                extents: Rc::default(),
                ellipsis: None,
                lines: Vec::new(),
                valid_widths: None,
                breaks: Rc::default(),
            },
        }
    }

    fn shape(key: Resolved, window: &Window) -> Laid {
        let field = key.field;
        let metrics_of = |a, d, g, size| {
            let m = line_metrics(a, d, g, size, key.line_height);
            if field { field_metrics(m) } else { m }
        };
        let (a, d, g) = font_extent(&key.base, key.font_size, window);
        let metrics = metrics_of(a, d, g, key.font_size);
        let mut paras = Vec::new();
        let mut start = 0;
        for piece in key.content.split('\n') {
            paras.push(shape_para(&key, start..start + piece.len(), window));
            start += piece.len() + 1;
        }
        let mut extents = Vec::new();
        let mut at = 0;
        for run in &key.runs {
            let (a, d, g) = font_extent(run, run.size, window);
            let m = metrics_of(a, d, g, run.size);
            extents.push((at..at + run.len, m.baseline, m.height - m.baseline));
            at += run.len;
        }
        Laid {
            key,
            paras: Rc::new(paras),
            metrics,
            extents: Rc::new(extents),
            ellipsis: None,
            lines: Vec::new(),
            valid_widths: None,
            breaks: Rc::default(),
        }
    }

    /// The line box over `range` (content bytes): the strut and every inline box on the line,
    /// aligned on the baseline. Returns (height, baseline below the top).
    fn line_box(&self, range: Range<usize>) -> (f32, f32) {
        let m = self.metrics;
        let (mut above, mut below) = (m.baseline, m.height - m.baseline);
        for (r, a, b) in self.extents.iter() {
            if r.start < range.end && range.start < r.end {
                above = above.max(*a);
                below = below.max(*b);
            }
        }
        (above + below, above)
    }

    /// Breaks the lines at `width`, or leaves none if line breaking panicked.
    fn relayout(
        &mut self,
        width: Option<f32>,
        ws: WhiteSpace,
        anywhere: bool,
        clamp: Option<usize>,
        truncate: bool,
        window: &Window,
    ) {
        let key: BreakKey = (
            width.map(f32::to_bits),
            ws,
            anywhere,
            clamp,
            truncate,
            window.scale_factor().to_bits(),
        );
        let scale = window.scale_factor().to_bits();
        let hit = self.breaks.borrow().iter().rev().find_map(|b| {
            let same = b.key == key
                || width.zip(b.valid).is_some_and(|(w, (lo, hi))| {
                    // Another width that breaks the same way (Taffy probes several a frame).
                    let w = w.max(0.);
                    b.key.0.is_some()
                        && (b.key.1, b.key.2, b.key.3, b.key.4, b.key.5)
                            == (ws, anywhere, clamp, truncate, scale)
                        && w >= lo
                        && w < hi
                });
            same.then(|| (b.lines.clone(), b.ellipsis.clone()))
        });
        if let Some((lines, ellipsis)) = hit {
            self.lines = lines;
            self.ellipsis = ellipsis;
            return;
        }
        let done = crate::guard::catch("text line breaking", || {
            self.break_all(width, ws, anywhere, clamp, truncate, window)
        });
        if done.is_none() {
            self.lines.clear();
            self.ellipsis = None;
            return;
        }
        let mut breaks = self.breaks.borrow_mut();
        if breaks.len() >= BREAKS_PER_TEXT {
            breaks.remove(0);
        }
        breaks.push(Broken {
            key,
            valid: self.valid_widths,
            lines: self.lines.clone(),
            ellipsis: self.ellipsis.clone(),
        });
    }

    fn break_all(
        &mut self,
        width: Option<f32>,
        ws: WhiteSpace,
        anywhere: bool,
        clamp: Option<usize>,
        truncate: bool,
        window: &Window,
    ) {
        self.ellipsis = None;
        let eps = 0.5 / window.scale_factor();
        let wrap = if ws == WhiteSpace::NoWrap {
            None
        } else {
            width
        };
        let mut lines = Vec::new();
        let mut range = (f32::NEG_INFINITY, f32::INFINITY);
        for (pi, p) in self.paras.iter().enumerate() {
            let text = &self.key.content[p.range.clone()];
            lines.extend(break_para(p, pi, text, wrap, anywhere, eps, &mut range));
        }
        let mut cut_last = false;
        if let Some(n) = clamp
            && lines.len() > n
        {
            lines.truncate(n);
            cut_last = true;
        }
        // An ellipsis is placed at the exact width; anything else holds across the range.
        self.valid_widths = (width.is_some() && !cut_last && !truncate).then_some(range);
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
                // WebKit drops the spaces before the ellipsis (`out…`, not `out …`).
                let kept = text[line.range.start..end].trim_end_matches([' ', '\t']);
                line.range.end = line.range.start + kept.len();
                line.width = p.width(text, line.range.clone()) + ell_w;
                line.ellipsis = true;
            }
            self.ellipsis = Some(ell);
        }
        let mut top = 0.;
        for line in &mut lines {
            let start = self.paras[line.para].range.start;
            let (h, b) = self.line_box(start + line.range.start..start + line.range.end);
            (line.top, line.height, line.baseline) = (top, h, b);
            top += h;
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

    /// Lines when none wraps: one per paragraph, up to the clamp.
    fn unwrapped_lines(&self, clamp: Option<usize>) -> usize {
        if self.key.content.is_empty() {
            return 0;
        }
        clamp.map_or(self.paras.len(), |c| c.min(self.paras.len()))
    }

    /// What to probe when no line wraps: each line's baseline and its inline boxes' top and
    /// bottom edges (below the box top), and for left-aligned text their left and right edges.
    fn probe_offsets(&self, clamp: Option<usize>, align: Align, window: &Window) -> Vec<Probe> {
        let mut out = Vec::new();
        let mut top = 0.;
        for p in &self.paras[..self.unwrapped_lines(clamp)] {
            let (h, b) = self.line_box(p.range.clone());
            let baseline = top + b;
            out.push(Probe::Y(baseline));
            for (span, x0, x1, _) in &p.boxes {
                let Some((ib, run)) = self.inline_box(*span) else {
                    continue;
                };
                let (a, d) = rounded_extent(run, window);
                out.push(Probe::Y(baseline - a - ib.pad_top - ib.border));
                out.push(Probe::Y(baseline + d + ib.pad_bottom + ib.border));
                if align == Align::Left {
                    out.extend([Probe::X(*x0), Probe::X(*x1)]);
                }
            }
            top += h;
        }
        out
    }

    fn inline_box(&self, span: usize) -> Option<(InlineBox, &RunStyle)> {
        let b = self.key.boxes.get(span).copied().flatten()?;
        Some((b, self.key.runs.iter().find(|r| r.span == span)?))
    }

    fn unwrapped_height(&self, clamp: Option<usize>) -> f32 {
        self.paras[..self.unwrapped_lines(clamp)]
            .iter()
            .map(|p| self.line_box(p.range.clone()).0)
            .sum()
    }

    fn height(&self) -> f32 {
        self.lines.last().map_or(0., |l| l.top + l.height)
    }

    /// Unrounded box size for layout; empty text takes no space.
    fn size(&self) -> (f32, f32) {
        if self.key.content.is_empty() {
            return (0., 0.);
        }
        let w = self.lines.iter().map(|l| l.width).fold(0., f32::max);
        (w, self.height())
    }

    /// Width of the widest line and the total height, unrounded, after a layout at `width`.
    fn measured(&self) -> (f32, f32) {
        // WebKit stores an inline text width as `LayoutUnit::fromFloatCeil`.
        let w = crate::exact::lu_ceil(self.lines.iter().map(|l| l.width).fold(0., f32::max));
        (w, self.height())
    }
}

pub struct TextState {
    layout: Rc<RefCell<Option<Laid>>>,
    /// A single unwrapped line's exact width, which sized the box.
    natural: Option<f32>,
    /// The max-content width layout measured, when it asked (a shrink-to-fit box).
    max_content: Rc<Cell<Option<f32>>>,
    probes: Vec<Probe>,
    /// The box's own layout node, whose unrounded origin the probes are resolved from.
    node: Option<LayoutId>,
    snapped: Snapped,
    /// The box's unrounded left edge.
    left: Option<Pixels>,
}

/// An offset from the text box's top (`Y`) or left (`X`) edge to probe.
#[derive(Clone, Copy, PartialEq)]
enum Probe {
    X(f32),
    Y(f32),
}

impl Probe {
    fn shift_y(self, by: f32) -> Probe {
        match self {
            Probe::Y(o) => Probe::Y(o + by),
            x => x,
        }
    }
}

/// Probed offsets and where GPUI snapped them, in window coordinates.
type Snapped = Vec<(Probe, f32)>;

/// The snapped window coordinate of a probed offset.
fn snapped(at: &[(Probe, f32)], probe: Probe) -> Option<f32> {
    at.iter()
        .find(|(p, _)| match (*p, probe) {
            (Probe::X(a), Probe::X(b)) | (Probe::Y(a), Probe::Y(b)) => (a - b).abs() < 1e-3,
            _ => false,
        })
        .map(|(_, v)| *v)
}

/// Where GPUI would snap each probed offset from the text box's unrounded origin (`node`'s,
/// read from layout): a baseline or an inline box edge lands where WebKit snaps it, though the
/// box's own origin was rounded to the device grid first.
fn resolve_probes(probes: &[Probe], node: Option<LayoutId>, window: &mut Window) -> Snapped {
    let Some(node) = node else {
        return Vec::new();
    };
    let origin = window.layout_bounds_unsnapped(node).origin;
    let scale = window.scale_factor();
    // A hair past, so an offset on a half pixel rounds down and right as WebKit's does (GPUI
    // rounds halves toward zero).
    let nudge = 1. / 1024.;
    let snap = |v: f32| {
        let d = (v + nudge) * scale;
        (d.abs() - 0.5).ceil().copysign(d) / scale
    };
    probes
        .iter()
        .map(|p| {
            let v = match p {
                Probe::X(o) => snap(f32::from(origin.x) + o),
                Probe::Y(o) => snap(f32::from(origin.y) + o),
            };
            (*p, v)
        })
        .collect()
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
    type PrepaintState = Vec<(usize, gpui_kit::Hitbox)>;

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
        if let Some(owner) = self.owner {
            crate::guard::count_layout(owner);
        }
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
        let max_content: Rc<Cell<Option<f32>>> = Rc::default();
        let measured_w = max_content.clone();
        let counted = self.owner.is_some();
        // Taffy keeps one measure result per constraint kind, so nested flex passes that probe
        // several widths would re-break the same text many times in a frame.
        let memo = RefCell::new(Vec::<(Option<f32>, (f32, f32))>::new());
        let mut style = Style::default();
        if truncate {
            // `overflow: hidden` makes the automatic minimum size zero in a flex row.
            style.min_size.width = px(0.).into();
        }
        if ws == WhiteSpace::NoWrap && clamp.is_none() {
            // A single unwrapped line has a fixed intrinsic size. GPUI rounds every authored
            // or measured length to device pixels, which would push later siblings off the
            // fractional positions WebKit gives them. `aspect_ratio` reaches Taffy unrounded,
            // so the box is a column of two invisible leaves, each one device pixel thin on its
            // authored axis: one exactly as wide as the line, one making up its exact height.
            let exact = layout.borrow_mut().as_mut().map(|laid| {
                laid.relayout(None, ws, anywhere, clamp, false, window);
                laid.measured()
            });
            let dp = 1. / window.scale_factor();
            if let Some((w, h)) = exact
                && w > 0.
                && h > 2. * dp
            {
                let leaf =
                    crate::exact::measured(gpui_kit::Length::Auto, window, move |_, _, _| (w, h));
                let probes = layout
                    .borrow()
                    .as_ref()
                    .map(|l| l.probe_offsets(clamp, self.align, window))
                    .unwrap_or_default();
                let node = window.request_layout(style, [leaf], cx);
                return (
                    node,
                    TextState {
                        layout,
                        natural: Some(w),
                        max_content,
                        probes,
                        node: Some(node),
                        snapped: Vec::new(),
                        left: None,
                    },
                );
            }
        }
        let mut leaves: SmallVec<[LayoutId; 6]> = SmallVec::new();
        leaves.push(crate::exact::measured(
            style.min_size.width,
            window,
            move |known, available, window| {
                if counted {
                    crate::guard::count_measure();
                }
                let mut slot = measure_slot.borrow_mut();
                let Some(laid) = slot.as_mut() else {
                    return (0., 0.);
                };
                let width = known.width.map(f32::from).or(match available.width {
                    AvailableSpace::Definite(w) => Some(w.into()),
                    AvailableSpace::MinContent => Some(0.),
                    AvailableSpace::MaxContent => None,
                });
                // Truncated text measures at its full width; the box decides where it cuts.
                let width = if truncate { None } else { width };
                if let Some(&(_, size)) = memo.borrow().iter().find(|(w, _)| *w == width) {
                    measured_w.set(Some(size.0));
                    return size;
                }
                laid.relayout(width, ws, anywhere, clamp, false, window);
                let size = laid.size();
                memo.borrow_mut().push((width, size));
                measured_w.set(Some(size.0));
                size
            },
        ));
        let probes = layout
            .borrow()
            .as_ref()
            .map(|l| l.probe_offsets(clamp, self.align, window))
            .unwrap_or_default();
        let node = window.request_layout(style, leaves, cx);
        (
            node,
            TextState {
                layout,
                natural: None,
                max_content,
                probes,
                node: Some(node),
                snapped: Vec::new(),
                left: None,
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut TextState,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<(usize, gpui_kit::Hitbox)> {
        let mut hits = Vec::new();
        if let Some(laid) = state.layout.borrow_mut().as_mut() {
            // A box as wide as its line can lose up to a device pixel when GPUI snaps its two
            // edges; the line still fits it, as it does in WebKit's unrounded layout.
            let mut width = f32::from(bounds.size.width);
            let dp = 1. / window.scale_factor();
            let natural = state.natural.or(state.max_content.get());
            if let Some(n) = natural
                && n > width
                && n <= width + dp
            {
                width = n;
            }
            laid.relayout(
                Some(width),
                self.white_space,
                self.anywhere,
                self.clamp,
                self.ellipsis,
                window,
            );
            if !self.links.is_empty() {
                hits = link_hitboxes(laid, bounds, self.align, &self.links, window);
            }
        }
        if let Some((key, sep)) = self.selectable {
            crate::selection::register(
                key,
                sep,
                bounds,
                Rc::new(SelectGeometry {
                    layout: state.layout.clone(),
                    bounds,
                    align: self.align,
                }),
                window,
                cx,
            );
        }
        state.snapped = resolve_probes(&state.probes, state.node, window);
        state.left = state
            .node
            .map(|n| window.layout_bounds_unsnapped(n).origin.x);
        hits
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut TextState,
        hits: &mut Vec<(usize, gpui_kit::Hitbox)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let selection = self
            .selectable
            .and_then(|(key, _)| crate::selection::selected(key, cx))
            .map(|r| {
                let g = SelectGeometry {
                    layout: state.layout.clone(),
                    bounds,
                    align: self.align,
                };
                let color = crate::selection::highlight(cx);
                (crate::selection::Geometry::rects(&g, r), color)
            });
        let selection = selection.as_ref().map(|(r, c)| (r.as_slice(), *c));
        let slot = state.layout.borrow();
        let Some(laid) = slot.as_ref() else { return };
        let clip = self.ellipsis || self.clamp.is_some();
        let align = self.align;
        let at = (0., state.snapped.as_slice());
        let hovered = paint_links(&self.links, hits, window);
        // Glyphs are placed from the box's unrounded left edge, as WebKit positions them.
        let glyph_bounds = Bounds {
            origin: point(state.left.unwrap_or(bounds.origin.x), bounds.origin.y),
            size: bounds.size,
        };
        if clip {
            window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
                paint_lines(
                    laid,
                    glyph_bounds,
                    at,
                    align,
                    self.mask.as_ref(),
                    &hovered,
                    selection,
                    window,
                )
            });
        } else {
            paint_lines(
                laid,
                glyph_bounds,
                at,
                align,
                self.mask.as_ref(),
                &hovered,
                selection,
                window,
            );
        }
    }
}

/// Block text stacked vertically (a heading over its description, a label over a hint). The
/// blocks lay out together at exact LayoutUnit offsets and the stack's height rounds once, as
/// WebKit stacks block boxes; separate [`Text`] elements would each round to a device pixel.
pub struct TextStack {
    id: Option<ElementId>,
    blocks: Vec<(f32, Text)>,
    layouts: Rc<RefCell<Vec<Laid>>>,
}

/// A stack of text blocks; each entry is (margin above, block).
pub fn text_stack(blocks: Vec<(f32, Text)>) -> TextStack {
    TextStack {
        id: None,
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
    /// With an id the stack keeps its exact fractional height across frames.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

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

pub struct StackState {
    probes: Vec<Probe>,
    /// The box's own layout node, whose unrounded origin the probes are resolved from.
    node: Option<LayoutId>,
    snapped: Snapped,
}

impl Element for TextStack {
    type RequestLayoutState = StackState;
    type PrepaintState = Vec<Vec<(usize, gpui_kit::Hitbox)>>;

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
    ) -> (LayoutId, StackState) {
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
        let mut leaves: SmallVec<[LayoutId; 6]> = SmallVec::new();
        leaves.push(crate::exact::measured(
            px(0.).into(),
            window,
            move |known, available, window| {
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
                (w, h)
            },
        ));
        let mut offsets = Vec::new();
        let mut top = 0.;
        for ((gap, t), l) in self.blocks.iter().zip(self.layouts.borrow().iter()) {
            top += gap;
            offsets.extend(
                l.probe_offsets(t.clamp, t.align, window)
                    .into_iter()
                    .map(|p| p.shift_y(top)),
            );
            top += l.unwrapped_height(t.clamp);
        }
        let node = window.request_layout(Style::default(), leaves, cx);
        (
            node,
            StackState {
                probes: offsets,
                node: Some(node),
                snapped: Vec::new(),
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut StackState,
        window: &mut Window,
        _: &mut App,
    ) -> Vec<Vec<(usize, gpui_kit::Hitbox)>> {
        let mut layouts = self.layouts.borrow_mut();
        self.relayout(&mut layouts, Some(bounds.size.width.into()), true, window);
        state.snapped = resolve_probes(&state.probes, state.node, window);
        let mut y = f32::from(bounds.origin.y);
        let mut hits = Vec::new();
        for ((gap, t), laid) in self.blocks.iter().zip(layouts.iter()) {
            y += gap;
            let (_, bh) = laid.measured();
            let b = Bounds {
                origin: point(bounds.origin.x, px(y)),
                size: size(bounds.size.width, px(bh)),
            };
            hits.push(if t.links.is_empty() {
                Vec::new()
            } else {
                link_hitboxes(laid, b, t.align, &t.links, window)
            });
            y += bh;
        }
        hits
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut StackState,
        hits: &mut Vec<Vec<(usize, gpui_kit::Hitbox)>>,
        window: &mut Window,
        _: &mut App,
    ) {
        let layouts = self.layouts.borrow();
        let mut y = f32::from(bounds.origin.y);
        for (i, ((gap, t), laid)) in self.blocks.iter().zip(layouts.iter()).enumerate() {
            let hovered = hits
                .get(i)
                .map(|h| paint_links(&t.links, h, window))
                .unwrap_or_default();
            y += gap;
            let (_, h) = laid.measured();
            let b = Bounds {
                origin: point(bounds.origin.x, px(y)),
                size: size(bounds.size.width, px(h)),
            };
            let at = (y - f32::from(bounds.origin.y), state.snapped.as_slice());
            if t.ellipsis || t.clamp.is_some() {
                window.with_content_mask(Some(gpui_kit::ContentMask { bounds: b }), |window| {
                    paint_lines(
                        laid,
                        b,
                        at,
                        t.align,
                        t.mask.as_ref(),
                        &hovered,
                        None,
                        window,
                    )
                });
            } else {
                paint_lines(
                    laid,
                    b,
                    at,
                    t.align,
                    t.mask.as_ref(),
                    &hovered,
                    None,
                    window,
                );
            }
            y += h;
        }
    }
}

/// Paint laid lines in `bounds`. `at` is the block's offset below the box top and the box's
/// snapped probes; what has no probe snaps from the rounded box origin.
#[allow(clippy::too_many_arguments)]
fn paint_lines(
    laid: &Laid,
    bounds: Bounds<Pixels>,
    at: (f32, &[(Probe, f32)]),
    align: Align,
    mask: Option<&AlphaMask>,
    hovered_links: &[usize],
    selection: Option<(&[Bounds<Pixels>], Hsla)>,
    window: &mut Window,
) {
    let scale = window.scale_factor();
    let box_w = f32::from(bounds.size.width);
    // Only what the clip shows paints: a minified file's one paragraph can be thousands of
    // lines and a million glyphs.
    let clip = window.content_mask().bounds;
    let (clip_top, clip_bottom) = (f32::from(clip.origin.y), f32::from(clip.bottom()));
    let (clip_left, clip_right) = (f32::from(clip.origin.x), f32::from(clip.right()));
    // Paragraphs whose glyphs run in byte order (no right-to-left reordering) find a line's
    // glyphs by binary search instead of scanning them all for every line.
    let mut sorted: Vec<Option<bool>> = vec![None; laid.paras.len()];
    for line in &laid.lines {
        let p = &laid.paras[line.para];
        let top = f32::from(bounds.origin.y) + line.top;
        if top > clip_bottom || top + line.height < clip_top {
            continue;
        }
        let base_off = at.0 + line.top + line.baseline;
        let baseline =
            snapped(at.1, Probe::Y(base_off)).unwrap_or_else(|| snap(top + line.baseline, scale));
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
            let Some((b, run)) = laid.inline_box(*span) else {
                continue;
            };
            let (a, d) = rounded_extent(run, window);
            let (above, below) = (a + b.pad_top + b.border, d + b.pad_bottom + b.border);
            let y0 = snapped(at.1, Probe::Y(base_off - above)).unwrap_or(baseline - above);
            let y1 = snapped(at.1, Probe::Y(base_off + below)).unwrap_or(baseline + below);
            let x = |o: f32| {
                let probe = (align == Align::Left).then(|| snapped(at.1, Probe::X(o - start_x)));
                probe.flatten().unwrap_or(left + o)
            };
            let rect = Bounds::from_corners(point(px(x(*x0)), px(y0)), point(px(x(*x1)), px(y1)));
            // A box broken across lines paints each fragment as a slice of the whole box
            // (`box-decoration-break: slice`), cut at the line's edges.
            let fragment = Bounds::from_corners(
                point(
                    if r.start < line.range.start {
                        px(x(start_x))
                    } else {
                        rect.left()
                    },
                    rect.top(),
                ),
                point(
                    if r.end > line.range.end {
                        px(x(start_x + line.width))
                    } else {
                        rect.right()
                    },
                    rect.bottom(),
                ),
            );
            window.with_content_mask(Some(gpui_kit::ContentMask { bounds: fragment }), |window| {
                window.paint_quad(quad(
                    rect,
                    px(b.radius),
                    b.background.unwrap_or_default(),
                    px(b.border),
                    b.border_color,
                    BorderStyle::Solid,
                ));
            });
        }
        // The selection paints over inline box backgrounds and under the glyphs, as WebKit's.
        if let Some((rects, color)) = selection {
            for r in rects {
                let mid = f32::from(r.origin.y) + f32::from(r.size.height) / 2.;
                if (top..top + line.height).contains(&mid) {
                    window.paint_quad(gpui_kit::fill(*r, color));
                }
            }
        }
        let text = &laid.key.content[p.range.clone()];
        let in_order =
            *sorted[line.para].get_or_insert_with(|| p.glyphs.is_sorted_by_key(|g| g.index));
        let (lo, hi) = if in_order {
            (
                p.glyphs.partition_point(|g| g.index < line.range.start),
                p.glyphs.partition_point(|g| g.index < line.range.end),
            )
        } else {
            (0, p.glyphs.len())
        };
        // One layer per line: the scene orders it as one primitive, where each glyph would
        // be one more insert (and one more again whenever a cached view replays it).
        let reach = 2. * line.height;
        let layer = Bounds::from_corners(
            point(px(clip_left), px(top - reach)),
            point(px(clip_right), px(top + line.height + reach)),
        );
        window.paint_layer(layer, |window| {
            for g in p.glyphs[lo..hi]
                .iter()
                .filter(|g| line.range.contains(&g.index))
            {
                let run = &laid.key.runs[g.run];
                let x = left + g.x;
                if x > clip_right {
                    if in_order {
                        break;
                    }
                    continue;
                }
                if x + 2. * run.size < clip_left {
                    continue;
                }
                let mut color = run.color;
                if let Some(mask) = mask {
                    let next = g.index
                        + text
                            .get(g.index..)
                            .and_then(|t| t.chars().next())
                            .map_or(1, char::len_utf8);
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
        });
        for (ri, r) in run_slices(
            &laid.key.runs,
            p.range.start + line.range.start..p.range.start + line.range.end,
        ) {
            let run = &laid.key.runs[ri];
            let underline = run.underline || hovered_links.contains(&run.span);
            if !(underline || run.line_through) {
                continue;
            }
            let x0 = left + p.x_at(r.start - p.range.start);
            let x1 = left + p.x_at(r.end - p.range.start);
            let deco = crate::font_metrics::decorations(&run.family, run.size);
            if underline {
                let y = run.underline_offset.unwrap_or(deco.underline_offset);
                window.paint_underline(
                    point(px(x0), px(baseline + y)),
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
            face,
            glyph,
            is_emoji,
        } => crate::glyph::paint(
            window, font_id, face, glyph, font_size, origin, color, is_emoji,
        ),
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
            underline_offset: None,
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
    fn field_text_counts_a_negative_half_leading_twice() {
        let inter = |size: f32| (size * 1984. / 2048., size * 494. / 2048.);
        let at = |size: f32| {
            let (a, d) = inter(size);
            field_metrics(line_metrics(a, d, 0., size, LineHeight::Normal)).baseline
        };
        // WebKit's placeholder baselines (tools/ui-parity probes): 14.5px sits at 13.5 where
        // block text sits at 13.765625; 11px and 14px (positive half-leading) do not move.
        assert_eq!(at(14.5), 13.484375);
        assert_eq!(at(18.), 16.65625);
        assert_eq!(at(11.), 11.);
        assert_eq!(at(14.), 13.578125);
    }

    #[test]
    fn a_line_starting_at_an_inline_box_takes_its_left_extent() {
        // "ab cd": the box over "cd" opens at x 13 (after "ab "), its glyphs start 6px later.
        let p = Para {
            range: 0..5,
            edges: vec![(0, 0.), (1, 5.), (2, 10.), (3, 19.), (4, 24.), (5, 35.)],
            glyphs: Vec::new(),
            boxes: Vec::new(),
            box_starts: vec![(3, 13.)],
        };
        assert_eq!(p.line_start(3), 13.);
        assert_eq!(p.width("ab cd", 3..5), 22.);
        assert_eq!(p.line_start(4), 24.);
    }

    /// The shared breaker the text element, `recur_content`'s mirror and the chat flow all use:
    /// at every width its lines tile the text, a mandatory break ends a line, a negative width
    /// lays out as zero and `width` is asked only about ranges that start a line.
    #[test]
    fn the_shared_breaker_tiles_the_text_and_honors_mandatory_breaks() {
        let text = "one two\nthreefourfive  six";
        let width = |r: Range<usize>| text[r].trim_end().chars().count() as f32 * 6.;
        for anywhere in [false, true] {
            let mut prev = None;
            for max in [-5., 0., 3., 6., 20., 40., 1000.] {
                let mut asked = Vec::new();
                let lines = break_lines(text, max, 0.25, anywhere, |r| {
                    asked.push(r.clone());
                    width(r)
                });
                let mut at = 0;
                for l in &lines {
                    assert_eq!(l.start, at, "{lines:?} at {max}");
                    assert!(l.start < l.end, "{lines:?} at {max}");
                    at = l.end;
                }
                assert_eq!(at, text.len());
                assert!(
                    lines.iter().any(|l| l.end == 8),
                    "the newline ends a line at {max}"
                );
                assert!(
                    asked
                        .iter()
                        .all(|r| lines.iter().any(|l| l.start == r.start))
                );
                if max == -5. {
                    prev = Some(lines);
                } else if max == 0. {
                    assert_eq!(prev.take(), Some(lines), "a negative width is zero");
                }
            }
        }
        assert_eq!(
            break_lines(text, 1000., 0.25, false, width),
            [0..8, 8..text.len()]
        );
    }

    #[test]
    fn breaking_anywhere_never_inverts_a_range_at_an_inline_box() {
        // "every `id`/`ids` is": the box over "ids" opens at byte 41 and brings 8px of
        // padding, so an empty range there is wider than a near-zero probe width.
        let text = "every id/ids is";
        let mut x = 0.;
        let mut edges = Vec::new();
        let mut box_starts = Vec::new();
        for i in 0..=text.len() {
            if i == 9 {
                box_starts.push((i, x));
                x += 8.;
            }
            edges.push((i, x));
            x += 6.;
        }
        let p = Para {
            range: 0..text.len(),
            edges,
            glyphs: Vec::new(),
            boxes: Vec::new(),
            box_starts,
        };
        for max in [0., 0.5, 3., 7.] {
            let lines = break_para(&p, 0, text, Some(max), true, 0.25, &mut (0., 0.));
            let mut at = 0;
            for l in &lines {
                assert!(l.range.start <= l.range.end, "{:?} at max {max}", l.range);
                assert_eq!(l.range.start, at, "lines must tile the text at max {max}");
                at = l.range.end;
            }
            assert_eq!(at, text.len());
        }
    }

    #[test]
    fn a_line_breaking_holds_across_its_valid_widths() {
        let text = "the quick brown fox jumps over the lazy dog again and again";
        let edges: Vec<(usize, f32)> = (0..=text.len()).map(|i| (i, i as f32 * 7.)).collect();
        let p = Para {
            range: 0..text.len(),
            edges,
            glyphs: Vec::new(),
            boxes: Vec::new(),
            box_starts: Vec::new(),
        };
        let ranges = |l: &[Line]| l.iter().map(|l| l.range.clone()).collect::<Vec<_>>();
        for anywhere in [false, true] {
            for max in [30., 95.5, 140., 201.25, 333.] {
                let mut valid = (f32::NEG_INFINITY, f32::INFINITY);
                let at = break_para(&p, 0, text, Some(max), anywhere, 0.25, &mut valid);
                assert!(valid.0 <= max && max < valid.1, "{valid:?} excludes {max}");
                // Anywhere inside the range the lines are the same (the range is sufficient,
                // not tight).
                for w in [
                    valid.0,
                    (valid.0 + max) / 2.,
                    max,
                    (max + valid.1.min(1e6)) / 2.,
                ] {
                    if w < valid.1 {
                        let l = break_para(&p, 0, text, Some(w), anywhere, 0.25, &mut (0., 0.));
                        assert_eq!(ranges(&l), ranges(&at), "w {w} in {valid:?} (max {max})");
                    }
                }
            }
        }
    }

    #[test]
    fn span_ranges_merge_runs_of_one_span() {
        let ranges = span_ranges(&[run(2, 0), run(3, 0), run(1, 2)]);
        assert_eq!(ranges, vec![(0, 0..5), (2, 5..6)]);
    }
}
