//! xterm.js's custom glyph rasterizer (`@xterm/addon-webgl` `CustomGlyphRasterizer.ts`): box
//! drawing, block elements, shades, powerline, braille, progress, git-branch and legacy computing
//! symbols are drawn to fill the cell exactly instead of coming from the font.
//!
//! Work happens in device pixels relative to the cell's top-left; the table lives in
//! [`crate::custom_glyph_data`].

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::{
    Bounds, ContentMask, Corners, FillOptions, Hsla, PathBuilder, PathStyle, Pixels, Point,
    RenderImage, Window, fill, point, px, size,
};

use crate::custom_glyph_data::GLYPHS;
use crate::font::CellMetrics;

/// One path instruction: command letter and its arguments, each `a + b·xp + c·yp` (None for an
/// empty or unparsable argument, which the canvas code skips).
pub(crate) struct I {
    pub(crate) cmd: u8,
    pub(crate) args: &'static [Option<[f32; 3]>],
}

pub(crate) enum K {
    Octants(&'static [[f32; 4]]),
    Pattern(&'static [&'static [f32]]),
    PathFn(&'static [I]),
    Path(&'static [I]),
    /// A fill with a shape cut out in the background colour.
    PathNegative,
    Vector {
        d: &'static [I],
        stroke: bool,
        left_padding: f32,
        right_padding: f32,
    },
    Braille(u8),
}

pub(crate) struct P {
    pub(crate) kind: K,
    pub(crate) stroke_width: Option<f32>,
    pub(crate) clip: Option<&'static [I]>,
    pub(crate) char_scale: bool,
}

fn lookup(cp: u32) -> Option<&'static [P]> {
    GLYPHS
        .binary_search_by_key(&cp, |(c, _)| *c)
        .ok()
        .map(|i| GLYPHS[i].1)
}

/// Whether `cp` has a custom definition.
pub(crate) fn has(cp: u32) -> bool {
    lookup(cp).is_some()
}

/// Shades use a 2-phase variant so patterns tile seamlessly across odd cell sizes.
fn is_pattern(cp: u32) -> bool {
    lookup(cp).is_some_and(|parts| parts.iter().any(|p| matches!(p.kind, K::Pattern(_))))
}

/// A path segment in device pixels on the y-down canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    /// Control point, end.
    Quad(f32, f32, f32, f32),
    /// Two control points, end.
    Cubic(f32, f32, f32, f32, f32, f32),
    /// An SVG arc from the current point (`drawSvgArc`); `phi` in degrees.
    Arc {
        rx: f32,
        ry: f32,
        phi: f32,
        large: bool,
        sweep: bool,
        x: f32,
        y: f32,
    },
    /// `arc(x, y, r, 0, 2π)`.
    Circle(f32, f32, f32),
    Close,
}

pub(crate) enum Clip<'a> {
    Rect(f32, f32, f32, f32),
    /// A clip path's `M` / `L` points.
    Polygon(&'a [(f32, f32)]),
}

/// The canvas 2D calls the rasterizer makes, in device pixels, filled in the glyph colour.
pub(crate) trait Canvas {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32);
    /// Fills the path (non-zero), or strokes it at `stroke` device pixels.
    fn draw_path(&mut self, path: &[Seg], stroke: Option<f32>);
    /// Runs `f` inside a `save()`, `clip()`, `restore()`.
    fn clipped(&mut self, clip: Clip<'_>, f: &mut dyn FnMut(&mut dyn Canvas));
    /// `drawPatternChar`: `rows` repeated from the canvas origin, shifted left and up by the
    /// variant's bits, filling the rectangle. `key` identifies the pattern for caching.
    #[allow(clippy::too_many_arguments)]
    fn pattern(
        &mut self,
        key: (u32, usize),
        rows: &[&[f32]],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        variant: u8,
    );
}

/// The cell geometry the rasterizer reads, in device pixels.
#[derive(Clone, Copy)]
pub(crate) struct Dims {
    pub cell_w: f32,
    pub cell_h: f32,
    pub char_w: f32,
    pub char_h: f32,
    /// CSS pixels.
    pub font_size: f32,
    pub dpr: f32,
}

impl From<&CellMetrics> for Dims {
    fn from(m: &CellMetrics) -> Self {
        Self {
            cell_w: m.cell_width,
            cell_h: m.cell_height,
            char_w: m.char_width,
            char_h: m.char_height,
            font_size: m.font_size,
            dpr: m.dpr,
        }
    }
}

/// `CellColorResolver`'s variant for shade patterns at a cell's device position: bit 1 shifts
/// the pattern a pixel left, bit 0 a pixel up, so odd cell sizes tile seamlessly.
pub(crate) fn pattern_variant(cp: u32, x: f32, y: f32) -> u8 {
    if is_pattern(cp) {
        ((x.round() % 2.0) as u8) * 2 + (y.round() % 2.0) as u8
    } else {
        0
    }
}

/// `tryDrawCustomGlyph`: draws `cp` with its cell's top-left at `(x0, y0)`. False when `cp` has
/// no custom definition.
pub(crate) fn draw(
    canvas: &mut dyn Canvas,
    cp: u32,
    x0: f32,
    y0: f32,
    d: &Dims,
    variant: u8,
) -> bool {
    let Some(parts) = lookup(cp) else {
        return false;
    };
    for (i, part) in parts.iter().enumerate() {
        let (mut x, mut y, mut w, mut h) = (x0, y0, d.cell_w, d.cell_h);
        if part.char_scale {
            w = d.char_w;
            h = d.char_h;
            x += (d.cell_w - d.char_w) / 2.0;
            y += (d.cell_h - d.char_h) / 2.0;
        }
        match part.clip {
            Some(ins) => {
                let poly: Vec<(f32, f32)> = ins
                    .iter()
                    .filter(|i| (i.cmd == b'M' || i.cmd == b'L') && i.args.len() >= 2)
                    .filter_map(|i| Some((x + i.args[0]?[0] * w, y + i.args[1]?[0] * h)))
                    .collect();
                canvas.clipped(Clip::Polygon(&poly), &mut |c| {
                    draw_part(c, cp, i, part, x, y, w, h, d, variant)
                });
            }
            None => draw_part(canvas, cp, i, part, x, y, w, h, d, variant),
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn draw_part(
    cx: &mut dyn Canvas,
    cp: u32,
    index: usize,
    part: &P,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    d: &Dims,
    variant: u8,
) {
    let dpr = d.dpr;
    match &part.kind {
        K::Octants(boxes) => {
            let (xe, ye) = (w / 8.0, h / 8.0);
            for b in *boxes {
                cx.fill_rect(x + b[0] * xe, y + b[1] * ye, b[2] * xe, b[3] * ye);
            }
        }
        K::Pattern(rows) => cx.pattern((cp, index), rows, x, y, w, h, variant),
        K::PathFn(ins) => {
            let (xp, yp) = (0.15, 0.15 / d.cell_h * d.cell_w);
            let path = svg_path(
                ins,
                xp,
                yp,
                |v, axis| translate(v, axis, w, h, x, y, true, 0.0, 0.0, dpr),
                false,
            );
            let stroke = part.stroke_width.map(|s| s * dpr);
            cx.clipped(Clip::Rect(x, y, w, h), &mut |c| c.draw_path(&path, stroke));
        }
        K::Path(ins) => {
            let path = svg_path(
                ins,
                0.0,
                0.0,
                |v, axis| if axis == 0 { x + v * w } else { y + v * h },
                true,
            );
            cx.draw_path(&path, part.stroke_width.map(|s| s * dpr));
        }
        // xterm draws the "negative" shape in the atlas background colour, which is fully
        // transparent when `allowTransparency` is on (as in Recur's pane): only the fill shows.
        K::PathNegative => cx.fill_rect(x, y, w, h),
        K::Vector {
            d: ins,
            stroke,
            left_padding,
            right_padding,
        } => {
            let css_line = d.font_size / 12.0;
            let (lp, rp) = (
                left_padding * (css_line / 2.0),
                right_padding * (css_line / 2.0),
            );
            let path = svg_path(
                ins,
                0.0,
                0.0,
                |v, axis| translate(v, axis, w, h, x, y, false, lp, rp, dpr),
                false,
            );
            let stroke = stroke.then_some(css_line * dpr);
            cx.clipped(Clip::Rect(x, y, w, h), &mut |c| c.draw_path(&path, stroke));
        }
        K::Braille(pattern) => {
            const DOTS: [(f32, f32); 8] = [
                (1.0, 0.0),
                (1.0, 2.0),
                (1.0, 4.0),
                (5.0, 0.0),
                (5.0, 2.0),
                (5.0, 4.0),
                (1.0, 6.0),
                (5.0, 6.0),
            ];
            let xe = w / 8.0;
            let pad_y = h * 0.1;
            let ye = h * 0.8 / 8.0;
            let r = xe.min(ye);
            for (bit, (dx, dy)) in DOTS.iter().enumerate() {
                if pattern & (1 << bit) != 0 {
                    let (ccx, ccy) = (x + (dx + 1.0) * xe, y + pad_y + (dy + 1.0) * ye);
                    cx.draw_path(&[Seg::Circle(ccx, ccy, r)], None);
                }
            }
        }
    }
}

/// `translateArgs` for one coordinate. `axis` 0 is x.
#[allow(clippy::too_many_arguments)]
fn translate(
    v: f32,
    axis: usize,
    w: f32,
    h: f32,
    x: f32,
    y: f32,
    do_clamp: bool,
    lp: f32,
    rp: f32,
    dpr: f32,
) -> f32 {
    if axis == 0 {
        let mut r = v * (w - lp * dpr - rp * dpr);
        if do_clamp && r != 0.0 {
            r = (r + 0.5 + 0.5).floor() - 0.5;
        }
        r + x + lp * dpr
    } else {
        let mut r = v * h;
        if do_clamp && r != 0.0 {
            r = (r + 0.5 + 0.5).floor() - 0.5;
        }
        r + y
    }
}

/// Builds a canvas path from instructions. `full` enables the commands only the PATH type
/// understands (H, V, A); the other types skip them, as the canvas code logs and continues.
fn svg_path(ins: &[I], xp: f32, yp: f32, map: impl Fn(f32, usize) -> f32, full: bool) -> Vec<Seg> {
    let mut b = Vec::new();
    let (mut cur_x, mut cur_y, mut ctl_x, mut ctl_y) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut last = b' ';
    let mut open = false;
    let lin = |a: &Option<[f32; 3]>| a.map(|c| c[0] + c[1] * xp + c[2] * yp);
    for i in ins {
        if i.cmd == b'Z' {
            if open {
                b.push(Seg::Close);
                open = false;
            }
            last = b'Z';
            continue;
        }
        if full && (i.cmd == b'V' || i.cmd == b'H') {
            let Some(v) = i.args.first().and_then(lin) else {
                continue;
            };
            if i.cmd == b'V' {
                cur_y = map(v, 1);
            } else {
                cur_x = map(v, 0);
            }
            b.push(Seg::Line(cur_x, cur_y));
            ctl_x = cur_x;
            ctl_y = cur_y;
            last = i.cmd;
            continue;
        }
        if i.args.len() < 2 || i.args[0].is_none() || i.args[1].is_none() {
            continue;
        }
        if full && i.cmd == b'A' {
            let a: Vec<f32> = i.args.iter().map(|a| lin(a).unwrap_or(f32::NAN)).collect();
            if a.len() < 7 {
                continue;
            }
            let (x2, y2) = (map(a[5], 0), map(a[6], 1));
            let rx = (map(a[0], 0) - map(0.0, 0)).abs();
            let ry = (map(a[1], 1) - map(0.0, 1)).abs();
            b.push(Seg::Arc {
                rx,
                ry,
                phi: a[2],
                large: a[3] != 0.0,
                sweep: a[4] != 0.0,
                x: x2,
                y: y2,
            });
            cur_x = x2;
            cur_y = y2;
            continue;
        }
        let v: Vec<f32> = i
            .args
            .iter()
            .enumerate()
            .map(|(j, a)| map(lin(a).unwrap_or(f32::NAN), j % 2))
            .collect();
        match i.cmd {
            b'M' => {
                b.push(Seg::Move(v[0], v[1]));
                open = true;
                (cur_x, cur_y, ctl_x, ctl_y) = (v[0], v[1], v[0], v[1]);
            }
            b'L' => {
                b.push(Seg::Line(v[0], v[1]));
                (cur_x, cur_y, ctl_x, ctl_y) = (v[0], v[1], v[0], v[1]);
            }
            b'Q' if v.len() >= 4 => {
                b.push(Seg::Quad(v[0], v[1], v[2], v[3]));
                (ctl_x, ctl_y, cur_x, cur_y) = (v[0], v[1], v[2], v[3]);
            }
            b'T' => {
                let (cpx, cpy) = if last == b'Q' || last == b'T' {
                    (2.0 * cur_x - ctl_x, 2.0 * cur_y - ctl_y)
                } else {
                    (cur_x, cur_y)
                };
                b.push(Seg::Quad(cpx, cpy, v[0], v[1]));
                (ctl_x, ctl_y, cur_x, cur_y) = (cpx, cpy, v[0], v[1]);
            }
            b'C' if v.len() >= 6 => {
                b.push(Seg::Cubic(v[0], v[1], v[2], v[3], v[4], v[5]));
                (ctl_x, ctl_y, cur_x, cur_y) = (v[2], v[3], v[4], v[5]);
            }
            _ => continue,
        }
        last = i.cmd;
    }
    b
}

/// Paints `cp` into the cell at `origin`. Returns false when `cp` has no custom definition.
/// Shades paint as GPUI primitives (their pixels follow the cell's position); every other glyph
/// is rasterized once per cell size and colour and painted as an image, since GPUI tessellates
/// and rasterizes a path again on every frame.
pub(crate) fn paint(
    window: &mut Window,
    images: &mut GlyphImages,
    cp: u32,
    origin: Point<Pixels>,
    _width: usize,
    m: &CellMetrics,
    color: Hsla,
) -> bool {
    if !has(cp) {
        return false;
    }
    let d = Dims::from(m);
    if is_pattern(cp) {
        let variant = pattern_variant(cp, f32::from(origin.x) * m.dpr, f32::from(origin.y) * m.dpr);
        return draw(
            &mut Gpui {
                window,
                origin,
                dpr: m.dpr,
                color,
                polygon: None,
                patterns: &mut images.patterns,
            },
            cp,
            0.0,
            0.0,
            &d,
            variant,
        );
    }
    // Strokes on the cell's edges reach half their width past it.
    let pad = (2.0 * m.dpr).ceil();
    let w = (d.cell_w.ceil() + 2.0 * pad) as u32;
    let h = (d.cell_h.ceil() + 2.0 * pad) as u32;
    let rgba = gpui_kit::Rgba::from(color);
    let c = [rgba.r, rgba.g, rgba.b, rgba.a].map(|v| (v * 255.0).round() as u8);
    let dims = [d.cell_w, d.cell_h, d.char_w, d.char_h, d.font_size, d.dpr].map(f32::to_bits);
    let image = images
        .rasters
        .entry((cp, dims, u32::from_be_bytes(c)))
        .or_insert_with(|| raster(cp, &d, pad, (w, h), c))
        .clone();
    if let Some(image) = image {
        let dev = |v: f32| px(v / m.dpr);
        let bounds = Bounds::new(
            point(origin.x - dev(pad), origin.y - dev(pad)),
            size(dev(w as f32), dev(h as f32)),
        );
        let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
    }
    true
}

/// Images a terminal keeps before it starts over: a glyph in each colour and cell size it drew.
/// Truecolor output (gradients, animated graphs) is what fills it.
const IMAGES_MAX: usize = 1024;

/// Codepoint, cell geometry bits, colour.
type RasterKey = (u32, [u32; 6], u32);

/// One terminal's custom glyph images: rasters by glyph, cell geometry and colour, and shade
/// patterns. GPUI's sprite atlas keeps an image until it is dropped, and a cached view replays
/// the sprites it last painted, so each terminal returns only its own: when it passes
/// [`IMAGES_MAX`] at the start of a paint (which replaces everything it drew) and when it closes.
#[derive(Default)]
pub(crate) struct GlyphImages {
    rasters: HashMap<RasterKey, Option<Arc<RenderImage>>>,
    patterns: HashMap<PatternKey, Arc<RenderImage>>,
}

impl GlyphImages {
    /// Starts over past [`IMAGES_MAX`], returning the images to the atlas. Call before painting.
    pub(crate) fn trim(&mut self, window: &mut Window) {
        if self.rasters.len() + self.patterns.len() < IMAGES_MAX {
            return;
        }
        for image in self.drain() {
            crate::element::count_work(|w| w.dropped += 1);
            let _ = window.drop_image(image);
        }
    }

    pub(crate) fn drain(&mut self) -> Vec<Arc<RenderImage>> {
        let rasters = self.rasters.drain().filter_map(|(_, v)| v);
        rasters
            .chain(self.patterns.drain().map(|(_, v)| v))
            .collect()
    }
}

/// `cp` drawn at `pad` device pixels into a `w`×`h` canvas, in colour `c`.
fn raster(cp: u32, d: &Dims, pad: f32, (w, h): (u32, u32), c: [u8; 4]) -> Option<Arc<RenderImage>> {
    crate::element::count_work(|w| w.rasterized += 1);
    let mut svg = Svg::default();
    draw(&mut svg, cp, pad, pad, d, 0);
    let markup = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{}</svg>"#,
        svg.body
    );
    let mut pixels =
        recur_ui::icon::rasterize(&markup, (w, h), (0., 0.), (w as f32, h as f32), 0.)?;
    // Drawn in black: the alpha is the coverage, painted in the exact colour (BGRA).
    for p in pixels.chunks_exact_mut(4) {
        let cov = u32::from(p[3]);
        p[0] = c[2];
        p[1] = c[1];
        p[2] = c[0];
        p[3] = ((cov * u32::from(c[3]) + 127) / 255) as u8;
    }
    let img = image::RgbaImage::from_raw(w, h, pixels)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(img)])))
}

/// The canvas as SVG markup in device pixels, filled and stroked in black.
#[derive(Default)]
struct Svg {
    body: String,
    clips: usize,
}

impl Canvas for Svg {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
        let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
        let _ = write!(
            self.body,
            r#"<rect x="{x}" y="{y}" width="{w}" height="{h}"/>"#
        );
    }

    fn draw_path(&mut self, path: &[Seg], stroke: Option<f32>) {
        let mut d = String::new();
        for seg in path {
            let _ = match *seg {
                Seg::Move(x, y) => write!(d, "M{x} {y}"),
                Seg::Line(x, y) => write!(d, "L{x} {y}"),
                Seg::Quad(cx, cy, x, y) => write!(d, "Q{cx} {cy} {x} {y}"),
                Seg::Cubic(c1x, c1y, c2x, c2y, x, y) => {
                    write!(d, "C{c1x} {c1y} {c2x} {c2y} {x} {y}")
                }
                Seg::Arc { rx, ry, x, y, .. } if rx == 0.0 || ry == 0.0 => write!(d, "L{x} {y}"),
                Seg::Arc {
                    rx,
                    ry,
                    phi,
                    large,
                    sweep,
                    x,
                    y,
                } => write!(
                    d,
                    "A{} {} {phi} {} {} {x} {y}",
                    rx.abs(),
                    ry.abs(),
                    u8::from(large),
                    u8::from(sweep)
                ),
                Seg::Circle(cx, cy, r) => write!(
                    d,
                    "M{} {cy}A{r} {r} 0 0 1 {} {cy}A{r} {r} 0 0 1 {} {cy}Z",
                    cx + r,
                    cx - r,
                    cx + r
                ),
                Seg::Close => write!(d, "Z"),
            };
        }
        let _ = match stroke {
            Some(sw) => write!(
                self.body,
                r##"<path d="{d}" fill="none" stroke="#000" stroke-width="{sw}"/>"##
            ),
            None => write!(self.body, r#"<path d="{d}"/>"#),
        };
    }

    fn clipped(&mut self, clip: Clip<'_>, f: &mut dyn FnMut(&mut dyn Canvas)) {
        let id = self.clips;
        self.clips += 1;
        let _ = match clip {
            Clip::Rect(x, y, w, h) => write!(
                self.body,
                r#"<clipPath id="c{id}"><rect x="{x}" y="{y}" width="{w}" height="{h}"/></clipPath>"#
            ),
            Clip::Polygon(poly) => {
                let points: Vec<String> = poly.iter().map(|(x, y)| format!("{x},{y}")).collect();
                write!(
                    self.body,
                    r#"<clipPath id="c{id}"><polygon points="{}"/></clipPath>"#,
                    points.join(" ")
                )
            }
        };
        let _ = write!(self.body, r#"<g clip-path="url(#c{id})">"#);
        f(self);
        self.body.push_str("</g>");
    }

    /// Shades never reach here: they paint through [`Gpui`].
    fn pattern(&mut self, _: (u32, usize), _: &[&[f32]], _: f32, _: f32, _: f32, _: f32, _: u8) {}
}

/// The canvas as GPUI quads, paths and images, relative to a cell origin. GPUI clips to
/// rectangles only, so a clip polygon applies to patterns (the only parts that carry one).
struct Gpui<'a> {
    window: &'a mut Window,
    origin: Point<Pixels>,
    dpr: f32,
    color: Hsla,
    polygon: Option<Vec<(f32, f32)>>,
    patterns: &'a mut HashMap<PatternKey, Arc<RenderImage>>,
}

impl Gpui<'_> {
    fn pt(&self, x: f32, y: f32) -> Point<Pixels> {
        point(
            self.origin.x + px(x / self.dpr),
            self.origin.y + px(y / self.dpr),
        )
    }

    fn bounds(&self, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(self.pt(x, y), size(px(w / self.dpr), px(h / self.dpr)))
    }
}

impl Canvas for Gpui<'_> {
    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let b = self.bounds(x, y, w, h);
        self.window.paint_quad(fill(b, self.color));
    }

    fn draw_path(&mut self, path: &[Seg], stroke: Option<f32>) {
        let mut b = PathBuilder::fill();
        for seg in path {
            match *seg {
                Seg::Move(x, y) => {
                    b.move_to(self.pt(x, y));
                }
                Seg::Line(x, y) => {
                    b.line_to(self.pt(x, y));
                }
                Seg::Quad(cx, cy, x, y) => {
                    b.curve_to(self.pt(x, y), self.pt(cx, cy));
                }
                Seg::Cubic(c1x, c1y, c2x, c2y, x, y) => {
                    b.cubic_bezier_to(self.pt(x, y), self.pt(c1x, c1y), self.pt(c2x, c2y));
                }
                Seg::Arc {
                    rx,
                    ry,
                    phi,
                    large,
                    sweep,
                    x,
                    y,
                } => {
                    if rx == 0.0 || ry == 0.0 {
                        b.line_to(self.pt(x, y));
                    } else {
                        let radii = point(px(rx.abs() / self.dpr), px(ry.abs() / self.dpr));
                        b.arc_to(radii, px(phi), large, sweep, self.pt(x, y));
                    }
                }
                Seg::Circle(cx, cy, r) => {
                    let radii = point(px(r / self.dpr), px(r / self.dpr));
                    b.move_to(self.pt(cx + r, cy));
                    b.arc_to(radii, px(0.0), false, true, self.pt(cx - r, cy));
                    b.arc_to(radii, px(0.0), false, true, self.pt(cx + r, cy));
                    b.close();
                }
                Seg::Close => b.close(),
            }
        }
        let b = match stroke {
            Some(w) => b.with_style(PathStyle::Stroke(
                gpui_kit::StrokeOptions::default().with_line_width(w / self.dpr),
            )),
            None => b.with_style(PathStyle::Fill(FillOptions::non_zero())),
        };
        if let Ok(path) = b.build() {
            crate::element::count_work(|w| w.paths += 1);
            self.window.paint_path(path, self.color);
        }
    }

    fn clipped(&mut self, clip: Clip<'_>, f: &mut dyn FnMut(&mut dyn Canvas)) {
        match clip {
            Clip::Rect(x, y, w, h) => {
                let bounds = self.bounds(x, y, w, h);
                let (origin, dpr, color) = (self.origin, self.dpr, self.color);
                let polygon = self.polygon.clone();
                let patterns = &mut *self.patterns;
                self.window
                    .with_content_mask(Some(ContentMask { bounds }), |window| {
                        f(&mut Gpui {
                            window,
                            origin,
                            dpr,
                            color,
                            polygon,
                            patterns,
                        })
                    });
            }
            Clip::Polygon(poly) => {
                let outer = self.polygon.replace(poly.to_vec());
                f(self);
                self.polygon = outer;
            }
        }
    }

    fn pattern(
        &mut self,
        key: (u32, usize),
        rows: &[&[f32]],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        variant: u8,
    ) {
        let (iw, ih) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
        let rgba = gpui_kit::Rgba::from(self.color);
        let color = u32::from_be_bytes([
            (rgba.r * 255.0).round() as u8,
            (rgba.g * 255.0).round() as u8,
            (rgba.b * 255.0).round() as u8,
            (rgba.a * 255.0).round() as u8,
        ]);
        let polygon = self.polygon.as_deref();
        let image = self
            .patterns
            .entry((key.0, key.1, iw, ih, variant, color))
            .or_insert_with(|| {
                let (dx, dy) = (u32::from((variant >> 1) & 1), u32::from(variant & 1));
                let (pw, ph) = (rows[0].len() as u32, rows.len() as u32);
                let mut img = image::RgbaImage::new(iw, ih);
                for py in 0..ih {
                    for pxl in 0..iw {
                        let on = rows[((py + dy) % ph) as usize][((pxl + dx) % pw) as usize];
                        let inside = polygon.is_none_or(|poly| {
                            point_in_polygon(poly, x + pxl as f32 + 0.5, y + py as f32 + 0.5)
                        });
                        let a = if inside {
                            (on * rgba.a * 255.0).round() as u8
                        } else {
                            0
                        };
                        // RenderImage frames are BGRA.
                        img.put_pixel(
                            pxl,
                            py,
                            image::Rgba([
                                (rgba.b * 255.0).round() as u8,
                                (rgba.g * 255.0).round() as u8,
                                (rgba.r * 255.0).round() as u8,
                                a,
                            ]),
                        );
                    }
                }
                Arc::new(RenderImage::new(vec![image::Frame::new(img)]))
            })
            .clone();
        let bounds = self.bounds(x, y, w, h);
        let _ = self
            .window
            .paint_image(bounds, bounds, Corners::default(), image, 0, false);
    }
}

/// Codepoint, part, cell width, cell height, variant, colour.
type PatternKey = (u32, usize, u32, u32, u8, u32);

fn point_in_polygon(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[(i + n - 1) % n];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_covers_box_drawing() {
        assert!(GLYPHS.windows(2).all(|w| w[0].0 < w[1].0));
        for cp in [
            0x2500, 0x253C, 0x2550, 0x256D, 0x2588, 0x2591, 0x2592, 0x2593, 0xE0B0, 0xE0B6, 0x2800,
            0x28FF,
        ] {
            assert!(lookup(cp).is_some(), "missing U+{cp:04X}");
        }
        assert!(lookup('a' as u32).is_none());
        assert!(is_pattern(0x2592) && !is_pattern(0x2500));
    }

    #[test]
    fn half_pixel_rounding_matches_translate_args() {
        // 0.5 of a 14px cell rounds to the pixel centre 7.5 (Math.round(7 + .5) - .5).
        assert_eq!(
            translate(0.5, 0, 14.0, 36.0, 0.0, 0.0, true, 0.0, 0.0, 2.0),
            7.5
        );
        assert_eq!(
            translate(0.0, 0, 14.0, 36.0, 3.0, 0.0, true, 0.0, 0.0, 2.0),
            3.0
        );
        assert_eq!(
            translate(0.5, 1, 14.0, 36.0, 0.0, 10.0, true, 0.0, 0.0, 2.0),
            28.5
        );
        assert_eq!(
            translate(0.5, 0, 14.0, 36.0, 0.0, 0.0, false, 1.0, 1.0, 2.0),
            0.5 * (14.0 - 4.0) + 2.0
        );
    }

    #[test]
    fn clip_polygon_containment() {
        let tri = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        assert!(point_in_polygon(&tri, 1.0, 1.0));
        assert!(!point_in_polygon(&tri, 9.0, 9.0));
    }
}
