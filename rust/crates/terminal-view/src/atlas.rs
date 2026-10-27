//! Glyph rasters drawn the way xterm.js's WebGL renderer draws them (`TextureAtlas._drawToCache`):
//! each glyph with its underline, overline and strikethrough is drawn into a device-pixel canvas
//! through the canvas 2D API, which WebKit implements with CoreText and CoreGraphics, cropped to
//! its bounding box and read back as the straight-alpha texels the atlas texture holds.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use recur_term::screen::{Rgba, UnderlineStyle};

use crate::font::CellMetrics;

/// `TMP_CANVAS_GLYPH_PADDING`; text is drawn twice that far into the canvas.
const PAD: usize = 2;
const PADDING: usize = PAD * 2;
/// WebKit's synthetic oblique angle, degrees.
const SYNTHETIC_OBLIQUE: f64 = 14.0;
/// Rasters kept before the cache starts over (`clearTextureAtlas` when the atlas fills).
const MAX_RASTERS: usize = 16_384;

/// Everything a raster depends on.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct GlyphKey {
    pub font: FontKey,
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub fg: Rgba,
    pub underline: Option<(UnderlineStyle, Rgba)>,
    /// `getUnderlineVariantOffset` for dotted underlines.
    pub variant_offset: u32,
    pub strikethrough: bool,
    pub overline: bool,
    /// Cell width of the text (`wcwidth`).
    pub width: usize,
    /// The text is a custom glyph (`custom_glyphs`), drawn with canvas paths instead of the font.
    pub custom: bool,
}

/// The atlas configuration: font and cell geometry in device pixels.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct FontKey {
    pub family: String,
    pub size: u32,
    pub dpr: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    pub char_w: u32,
    pub char_h: u32,
    pub line_height: u32,
}

impl FontKey {
    pub fn new(family: &str, m: &CellMetrics, line_height: f32) -> Self {
        Self {
            family: family.to_string(),
            size: m.font_size.to_bits(),
            dpr: m.dpr.to_bits(),
            cell_w: m.cell_width as u32,
            cell_h: m.cell_height as u32,
            char_w: m.char_width as u32,
            char_h: m.char_height as u32,
            line_height: line_height.to_bits(),
        }
    }

    fn font_size(&self) -> f32 {
        f32::from_bits(self.size)
    }

    fn dpr(&self) -> f32 {
        f32::from_bits(self.dpr)
    }

    /// Underline and overline width (`max(1, floor(fontSize * dpr / 15))`).
    fn line_width(&self) -> f64 {
        (self.font_size() * self.dpr() / 15.0).floor().max(1.0) as f64
    }
}

/// A rasterized glyph: straight RGBA texels and where they sit relative to the canvas origin.
pub(crate) struct Raster {
    pub texels: Vec<u8>,
    pub left: usize,
    pub top: usize,
    pub w: usize,
    pub h: usize,
}

thread_local! {
    static CACHE: RefCell<HashMap<GlyphKey, Option<Arc<Raster>>>> = RefCell::default();
}

/// Where a key's canvas origin sits relative to its cell's top-left, device pixels. Text is
/// placed by the character box (`char.top`); custom glyphs are drawn in cell coordinates.
pub(crate) fn canvas_origin(key: &GlyphKey, m: &CellMetrics) -> (i64, i64) {
    let top = if key.custom { 0 } else { m.char_top as i64 };
    let pad = padding(key) as i64;
    (-pad, top - pad)
}

/// `isRestrictedPowerlineGlyph`: drawn at the canvas origin and cropped to the cell.
fn restricted(key: &GlyphKey) -> bool {
    let mut chars = key.text.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('\u{E0B0}'..='\u{E0B7}'), None)
    )
}

/// Where the glyph is drawn in the canvas (left and top padding).
fn padding(key: &GlyphKey) -> usize {
    if restricted(key) { 0 } else { PADDING }
}

/// The raster for `key`, drawn on first use. `None` for an empty glyph.
pub(crate) fn raster(key: &GlyphKey) -> Option<Arc<Raster>> {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= MAX_RASTERS && !c.contains_key(key) {
            c.clear();
        }
        c.entry(key.clone())
            .or_insert_with(|| rasterize(key).map(Arc::new))
            .clone()
    })
}

/// WebKit's premultiplied to straight conversion (`getImageData`, texture uploads).
fn unpremultiply(c: u8, a: u8) -> u8 {
    match a {
        0 => 0,
        255 => c,
        a => ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8,
    }
}

/// And back (`putImageData` into the atlas page).
fn premultiply(c: u8, a: u8) -> u8 {
    ((c as u32 * a as u32 + 127) / 255) as u8
}

fn rasterize(key: &GlyphKey) -> Option<Raster> {
    let f = &key.font;
    let utf16 = key.text.encode_utf16().count();
    let allowed = (f.cell_w as usize * utf16.max(2) + PAD * 2).min(4096);
    // The scratch canvas is at least four cells wide, and the left and right edge searches look
    // `PADDING` past the allowed width.
    let width = (allowed + PADDING).max(f.cell_w as usize * 4 + PAD * 2);
    let height = f.cell_h as usize + PAD * 4;
    let px = draw(key, width, height)?;
    let (left, top, right, bottom) = if restricted(key) {
        bounding_box(&px, width, f.cell_w as usize, f.cell_h as usize, 0)?
    } else {
        bounding_box(&px, width, allowed, height, PADDING)?
    };
    let (w, h) = (right - left + 1, bottom - top + 1);
    // getImageData, putImageData into the atlas page, then the straight texImage2D upload.
    let texel = |c: u8, a: u8| unpremultiply(premultiply(unpremultiply(c, a), a), a);
    let mut texels = Vec::with_capacity(w * h * 4);
    for y in top..=bottom {
        for x in left..=right {
            let i = (y * width + x) * 4;
            let a = px[i + 3];
            texels.extend_from_slice(&[
                texel(px[i], a),
                texel(px[i + 1], a),
                texel(px[i + 2], a),
                a,
            ]);
        }
    }
    Some(Raster {
        texels,
        left,
        top,
        w,
        h,
    })
}

/// `_findGlyphBoundingBox`: rows with coverage within the allowed width, columns with coverage
/// up to `padding` past it. `None` for an empty glyph (`checkCompletelyTransparent`). A
/// restricted powerline glyph searches the cell only.
fn bounding_box(
    px: &[u8],
    width: usize,
    allowed: usize,
    height: usize,
    padding: usize,
) -> Option<(usize, usize, usize, usize)> {
    if px.chunks_exact(4).all(|p| p[3] == 0) {
        return None;
    }
    let a = |x: usize, y: usize| px[(y * width + x) * 4 + 3];
    let top = (0..height)
        .find(|&y| (0..allowed).any(|x| a(x, y) != 0))
        .unwrap_or(0);
    let bottom = (0..height)
        .rev()
        .find(|&y| (0..allowed).any(|x| a(x, y) != 0))
        .unwrap_or(height);
    let left = (0..padding + allowed)
        .find(|&x| (0..height).any(|y| a(x, y) != 0))
        .unwrap_or(0);
    let right = (padding..padding + allowed)
        .rev()
        .find(|&x| (0..height).any(|y| a(x, y) != 0))
        .unwrap_or(allowed);
    (right >= left && bottom >= top && bottom < height).then_some((left, top, right, bottom))
}

fn draw(key: &GlyphKey, width: usize, height: usize) -> Option<Vec<u8>> {
    mac::draw(key, width, height)
}

mod mac {
    use super::*;
    use crate::custom_glyphs::{self, Canvas, Clip, Dims, Seg};
    use core_foundation::attributed_string::CFMutableAttributedString;
    use core_foundation::base::{CFRange, TCFType};
    use core_foundation::string::CFString;
    use core_graphics::base::kCGImageAlphaPremultipliedLast;
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::{CGContext, CGLineCap};
    use core_graphics::data_provider::CGDataProvider;
    use core_graphics::font::CGFont;
    use core_graphics::geometry::{CGAffineTransform, CGPoint, CGRect, CGSize};
    use core_text::font::CTFont;
    use core_text::line::CTLine;

    thread_local! {
        static FONTS: RefCell<HashMap<(String, bool, u32), Option<CTFont>>> = RefCell::default();
    }

    /// The first family of the stack the terminal font resolves to, at `weight` and `size`
    /// (device px): a bundled face when the family is one, otherwise the installed font.
    fn font(family: &str, bold: bool, size: f32) -> Option<CTFont> {
        FONTS.with(|f| {
            f.borrow_mut()
                .entry((family.to_string(), bold, size.to_bits()))
                .or_insert_with(|| {
                    let (primary, fallbacks) = crate::font::mono_font_stack(family);
                    std::iter::once(primary)
                        .chain(fallbacks)
                        .find_map(|name| load(&name, bold, size))
                })
                .clone()
        })
    }

    fn load(name: &str, bold: bool, size: f32) -> Option<CTFont> {
        let weight = if bold { 700. } else { 400. };
        if let Some(i) = recur_ui::fonts::match_face(name, weight) {
            let bytes: &'static [u8] = recur_ui::fonts::FACES[i].2;
            let provider = CGDataProvider::from_buffer(Arc::new(bytes));
            let cg = CGFont::from_data_provider(provider).ok()?;
            return Some(core_text::font::new_from_CGFont(&cg, size as f64));
        }
        let font = core_text::font::new_from_name(name, size as f64).ok()?;
        // CoreText substitutes a default font for unknown names.
        if !font.family_name().eq_ignore_ascii_case(name) {
            return None;
        }
        if bold {
            let traits = core_text::font_descriptor::kCTFontBoldTrait;
            return Some(
                font.clone_with_symbolic_traits(traits, traits)
                    .unwrap_or(font),
            );
        }
        Some(font)
    }

    pub(super) fn draw(key: &GlyphKey, width: usize, height: usize) -> Option<Vec<u8>> {
        let f = &key.font;
        let space = CGColorSpace::create_device_rgb();
        let mut cx = CGContext::create_bitmap_context(
            None,
            width,
            height,
            8,
            width * 4,
            &space,
            kCGImageAlphaPremultipliedLast,
        );
        // The canvas is y-down.
        cx.translate(0., height as f64);
        cx.scale(1., -1.);
        cx.set_allows_antialiasing(true);
        cx.set_should_antialias(true);
        cx.set_should_smooth_fonts(true);
        cx.set_allows_font_subpixel_positioning(true);
        cx.set_should_subpixel_position_fonts(true);
        cx.set_allows_font_subpixel_quantization(true);
        cx.set_should_subpixel_quantize_fonts(true);
        cx.set_line_cap(CGLineCap::CGLineCapButt);

        let padding = super::padding(key) as f64;
        let char_h = f.char_h as f64;
        let char_w = f.char_w as f64;
        let cell_w = f.cell_w as f64;
        let rgba = |c: Rgba| {
            (
                c.r() as f64 / 255.,
                c.g() as f64 / 255.,
                c.b() as f64 / 255.,
                c.a() as f64 / 255.,
            )
        };
        let fill = rgba(key.fg);
        let ch_width = key.width as f64;
        // The context line width the strikethrough's offset reads: canvas default 1, or the
        // overline's (it sets the width without saving the state).
        let mut ctx_line_width = 1.0;

        if key.custom {
            cx.set_rgb_fill_color(fill.0, fill.1, fill.2, fill.3);
            cx.set_rgb_stroke_color(fill.0, fill.1, fill.2, fill.3);
            let cp = key.text.chars().next().map_or(0, u32::from);
            let dims = Dims {
                cell_w: f.cell_w as f32,
                cell_h: f.cell_h as f32,
                char_w: f.char_w as f32,
                char_h: f.char_h as f32,
                font_size: f.font_size(),
                dpr: f.dpr(),
            };
            let variant = (key.variant_offset & 3) as u8;
            let p = padding as f32;
            custom_glyphs::draw(&mut Cg { cx: &cx, fill }, cp, p, p, &dims, variant);
        }

        if let Some((style, color)) = key.underline {
            let lw = f.line_width();
            let y_off = if lw as i64 % 2 == 1 { 0.5 } else { 0.0 };
            let (r, g, b, a) = rgba(color);
            cx.save();
            cx.set_line_width(lw);
            cx.set_rgb_stroke_color(r, g, b, a);
            cx.set_rgb_fill_color(r, g, b, a);
            let y_top = (padding + char_h).ceil() - y_off;
            let y_bot = y_top + lw * 2.0;
            let mut next_offset = key.variant_offset as f64;
            for i in 0..key.width {
                let x0 = padding + i as f64 * cell_w;
                let x1 = padding + (i + 1) as f64 * cell_w;
                cx.save();
                cx.begin_path();
                let mut filled = false;
                match style {
                    UnderlineStyle::Double => {
                        cx.move_to_point(x0, y_top);
                        cx.add_line_to_point(x1, y_top);
                        cx.move_to_point(x0, y_bot);
                        cx.add_line_to_point(x1, y_bot);
                    }
                    UnderlineStyle::Curly => {
                        let top = char_h + 1.0;
                        let bot = top + 3.0 * f.dpr() as f64;
                        cx.clip_to_rect(CGRect::new(
                            &CGPoint::new(x0, top),
                            &CGSize::new(cell_w, bot - top),
                        ));
                        let (sx, sy) = (cell_w / 6.0, (bot - top) / 3.0);
                        const POLYS: [[f64; 8]; 3] = [
                            [0.0, 2.0, 1.0, 3.0, 2.4, 3.0, 0.0, 0.6],
                            [5.5, 0.0, 2.5, 3.0, 1.1, 3.0, 4.1, 0.0],
                            [4.0, 0.0, 6.0, 2.0, 6.0, 0.6, 5.4, 0.0],
                        ];
                        for poly in POLYS {
                            cx.begin_path();
                            for p in (0..8).step_by(2) {
                                let (x, y) = (x0 + poly[p] * sx, bot - poly[p + 1] * sy);
                                if p == 0 {
                                    cx.move_to_point(x, y);
                                } else {
                                    cx.add_line_to_point(x, y);
                                }
                            }
                            cx.close_path();
                            cx.fill_path();
                        }
                        filled = true;
                    }
                    UnderlineStyle::Dotted => {
                        let offset_width = if next_offset == 0.0 {
                            0.0
                        } else if next_offset >= lw {
                            lw * 2.0 - next_offset
                        } else {
                            lw - next_offset
                        };
                        let line_start = next_offset < lw;
                        cx.set_line_dash(0.0, &[lw.round(), lw.round()]);
                        if !line_start || offset_width == 0.0 {
                            cx.move_to_point(x0 + offset_width, y_top);
                            cx.add_line_to_point(x1, y_top);
                        } else {
                            cx.move_to_point(x0, y_top);
                            cx.add_line_to_point(x0 + offset_width, y_top);
                            cx.move_to_point(x0 + offset_width + lw, y_top);
                            cx.add_line_to_point(x1, y_top);
                        }
                        next_offset = next_variant_offset(x1 - x0, lw, next_offset);
                    }
                    UnderlineStyle::Dashed => {
                        let w = x1 - x0;
                        let line = (0.6 * w).floor();
                        let gap = (0.3 * w).floor();
                        cx.set_line_dash(0.0, &[line, gap, w - line - gap]);
                        cx.move_to_point(x0, y_top);
                        cx.add_line_to_point(x1, y_top);
                    }
                    UnderlineStyle::Single => {
                        cx.move_to_point(x0, y_top);
                        cx.add_line_to_point(x1, y_top);
                    }
                }
                if !filled {
                    cx.stroke_path();
                }
                cx.restore();
            }
            cx.restore();
        }

        if key.overline {
            let lw = f.line_width();
            let y_off = if lw as i64 % 2 == 1 { 0.5 } else { 0.0 };
            ctx_line_width = lw;
            cx.save();
            cx.set_line_width(lw);
            cx.set_rgb_stroke_color(fill.0, fill.1, fill.2, fill.3);
            cx.begin_path();
            cx.move_to_point(padding, padding + y_off);
            cx.add_line_to_point(padding + char_w * ch_width, padding + y_off);
            cx.stroke_path();
            cx.restore();
        }

        if !key.custom && !key.text.is_empty() {
            let size = f.font_size() * f.dpr();
            let primary = font(&f.family, key.bold, size)?;
            // `textBaseline = 'ideographic'`: the baseline sits the font's descent above y.
            let baseline = padding + char_h - primary.descent();
            draw_text(
                &cx, &primary, &key.text, padding, baseline, key.italic, fill,
            );
        }

        if key.strikethrough {
            let lw = (f.font_size() * f.dpr() / 10.0).floor().max(1.0) as f64;
            let y_off = if ctx_line_width as i64 % 2 == 1 {
                0.5
            } else {
                0.0
            };
            let y = padding + (char_h / 2.0).floor() - y_off;
            cx.save();
            cx.set_line_width(lw);
            cx.set_rgb_stroke_color(fill.0, fill.1, fill.2, fill.3);
            cx.begin_path();
            cx.move_to_point(padding, y);
            cx.add_line_to_point(padding + char_w * ch_width, y);
            cx.stroke_path();
            cx.restore();
        }

        let stride = cx.bytes_per_row();
        let data = cx.data();
        let mut out = Vec::with_capacity(width * height * 4);
        for row in 0..height {
            out.extend_from_slice(&data[row * stride..row * stride + width * 4]);
        }
        Some(out)
    }

    /// The canvas as a CoreGraphics context, as WebKit's canvas 2D draws on macOS.
    struct Cg<'a> {
        cx: &'a CGContext,
        fill: (f64, f64, f64, f64),
    }

    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn CGContextAddArc(
            c: core_graphics::sys::CGContextRef,
            x: f64,
            y: f64,
            radius: f64,
            start: f64,
            end: f64,
            clockwise: i32,
        );
    }

    impl Cg<'_> {
        /// `ctx.arc` / `ctx.ellipse` as WebKit's `Path::addEllipse` builds it: a unit arc under
        /// translate, rotate and scale (the current path is not part of the saved state).
        #[allow(clippy::too_many_arguments)]
        fn ellipse(&self, x: f64, y: f64, rx: f64, ry: f64, phi: f64, a0: f64, a1: f64, ccw: bool) {
            self.cx.save();
            self.cx.translate(x, y);
            self.cx.rotate(phi);
            self.cx.scale(rx, ry);
            // A `CGContextRef`'s address is the context pointer (`ForeignTypeRef::as_ptr`).
            let ptr: *const core_graphics::context::CGContextRef = &**self.cx;
            // SAFETY: the context is a live bitmap context owned by the caller.
            #[allow(unsafe_code)]
            unsafe {
                CGContextAddArc(ptr as _, 0.0, 0.0, 1.0, a0, a1, i32::from(ccw));
            }
            self.cx.restore();
        }

        /// `drawSvgArc`: the SVG endpoint arc as a canvas ellipse.
        #[allow(clippy::too_many_arguments)]
        fn svg_arc(
            &self,
            (x1, y1): (f64, f64),
            rx: f64,
            ry: f64,
            phi: f64,
            large: bool,
            sweep: bool,
            (x2, y2): (f64, f64),
        ) {
            if rx == 0.0 || ry == 0.0 {
                self.cx.add_line_to_point(x2, y2);
                return;
            }
            let (mut rx, mut ry) = (rx.abs(), ry.abs());
            let (cos, sin) = (phi.cos(), phi.sin());
            let (dx, dy) = ((x1 - x2) / 2.0, (y1 - y2) / 2.0);
            let x1p = cos * dx + sin * dy;
            let y1p = -sin * dx + cos * dy;
            let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
            if lambda > 1.0 {
                rx *= lambda.sqrt();
                ry *= lambda.sqrt();
            }
            let (rx2, ry2) = (rx * rx, ry * ry);
            let sq = ((rx2 * ry2 - rx2 * y1p * y1p - ry2 * x1p * x1p)
                / (rx2 * y1p * y1p + ry2 * x1p * x1p))
                .max(0.0);
            let coef = if large == sweep { -1.0 } else { 1.0 } * sq.sqrt();
            let cxp = coef * (rx * y1p / ry);
            let cyp = coef * -(ry * x1p / rx);
            let cx = cos * cxp - sin * cyp + (x1 + x2) / 2.0;
            let cy = sin * cxp + cos * cyp + (y1 + y2) / 2.0;
            let start = ((y1p - cyp) / ry).atan2((x1p - cxp) / rx);
            let mut delta = ((-y1p - cyp) / ry).atan2((-x1p - cxp) / rx) - start;
            if !sweep && delta > 0.0 {
                delta -= std::f64::consts::TAU;
            } else if sweep && delta < 0.0 {
                delta += std::f64::consts::TAU;
            }
            self.ellipse(cx, cy, rx, ry, phi, start, start + delta, !sweep);
        }
    }

    impl Canvas for Cg<'_> {
        fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
            self.cx.fill_rect(rect(x, y, w, h));
        }

        fn draw_path(&mut self, path: &[Seg], stroke: Option<f32>) {
            let cx = self.cx;
            cx.begin_path();
            let (mut cur, mut start) = ((0.0, 0.0), (0.0, 0.0));
            for seg in path {
                match *seg {
                    Seg::Move(x, y) => {
                        cx.move_to_point(x.into(), y.into());
                        (cur, start) = ((x.into(), y.into()), (x.into(), y.into()));
                    }
                    Seg::Line(x, y) => {
                        cx.add_line_to_point(x.into(), y.into());
                        cur = (x.into(), y.into());
                    }
                    Seg::Quad(qx, qy, x, y) => {
                        cx.add_quad_curve_to_point(qx.into(), qy.into(), x.into(), y.into());
                        cur = (x.into(), y.into());
                    }
                    Seg::Cubic(c1x, c1y, c2x, c2y, x, y) => {
                        cx.add_curve_to_point(
                            c1x.into(),
                            c1y.into(),
                            c2x.into(),
                            c2y.into(),
                            x.into(),
                            y.into(),
                        );
                        cur = (x.into(), y.into());
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
                        let end = (x.into(), y.into());
                        let phi = f64::from(phi).to_radians();
                        self.svg_arc(cur, rx.into(), ry.into(), phi, large, sweep, end);
                        cur = end;
                    }
                    Seg::Circle(x, y, r) => {
                        let (x, y, r) = (f64::from(x), f64::from(y), f64::from(r));
                        self.ellipse(x, y, r, r, 0.0, 0.0, std::f64::consts::TAU, false);
                        cur = (x + r, y);
                    }
                    Seg::Close => {
                        cx.close_path();
                        cur = start;
                    }
                }
            }
            match stroke {
                Some(w) => {
                    cx.set_line_width(w.into());
                    cx.stroke_path();
                }
                None => cx.fill_path(),
            }
        }

        fn clipped(&mut self, clip: Clip<'_>, f: &mut dyn FnMut(&mut dyn Canvas)) {
            self.cx.save();
            match clip {
                Clip::Rect(x, y, w, h) => self.cx.clip_to_rect(rect(x, y, w, h)),
                Clip::Polygon(poly) => {
                    self.cx.begin_path();
                    for (i, &(x, y)) in poly.iter().enumerate() {
                        if i == 0 {
                            self.cx.move_to_point(x.into(), y.into());
                        } else {
                            self.cx.add_line_to_point(x.into(), y.into());
                        }
                    }
                    self.cx.clip();
                }
            }
            f(self);
            self.cx.restore();
        }

        fn pattern(
            &mut self,
            _: (u32, usize),
            rows: &[&[f32]],
            x: f32,
            y: f32,
            w: f32,
            h: f32,
            variant: u8,
        ) {
            // The pattern repeats from the canvas origin, translated by `-dx, -dy`.
            let (dx, dy) = (usize::from((variant >> 1) & 1), usize::from(variant & 1));
            let (pw, ph) = (rows[0].len(), rows.len());
            let (x0, y0) = (x.round() as usize, y.round() as usize);
            let (w, h) = (w.round() as usize, h.round() as usize);
            let (r, g, b, a) = self.fill;
            for py in y0..y0 + h {
                for px in x0..x0 + w {
                    let on = f64::from(rows[(py + dy) % ph][(px + dx) % pw]);
                    if on > 0.0 {
                        self.cx.set_rgb_fill_color(r, g, b, a * on);
                        self.cx.fill_rect(rect(px as f32, py as f32, 1.0, 1.0));
                    }
                }
            }
            self.cx.set_rgb_fill_color(r, g, b, a);
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> CGRect {
        CGRect::new(
            &CGPoint::new(x.into(), y.into()),
            &CGSize::new(w.into(), h.into()),
        )
    }

    /// `computeNextVariantOffset`.
    fn next_variant_offset(cell_width: f64, line_width: f64, current: f64) -> f64 {
        (current + cell_width) % (line_width * 2.0)
    }

    /// `fillText(text, x, baseline)` in `font`, with CoreText's fallback runs for characters the
    /// font lacks and WebKit's synthetic oblique for italics.
    fn draw_text(
        cx: &CGContext,
        font: &CTFont,
        text: &str,
        x: f64,
        baseline: f64,
        italic: bool,
        color: (f64, f64, f64, f64),
    ) {
        let mut s = CFMutableAttributedString::new();
        let cf = CFString::new(text);
        s.replace_str(&cf, CFRange::init(0, 0));
        s.set_attribute(
            CFRange::init(0, cf.char_len()),
            CFString::new("NSFont").as_concrete_TypeRef(),
            font,
        );
        let line = CTLine::new_with_attributed_string(s.as_concrete_TypeRef() as _);
        cx.save();
        cx.set_rgb_fill_color(color.0, color.1, color.2, color.3);
        // WebKit's `computeOverallTextMatrix` on the y-down canvas: the font matrix flipped,
        // then skewed for a synthetic oblique.
        cx.translate(x, baseline);
        let skew = if italic {
            SYNTHETIC_OBLIQUE.to_radians().tan()
        } else {
            0.0
        };
        cx.set_text_matrix(&CGAffineTransform::new(1., 0., skew, -1., 0., 0.));
        for run in line.glyph_runs().iter() {
            let run_font = run
                .attributes()
                .and_then(|a| a.find(CFString::new("NSFont")).map(|v| v.clone()))
                .and_then(|v| v.downcast::<CTFont>())
                .unwrap_or_else(|| font.clone());
            let glyphs = run.glyphs();
            let positions: Vec<CGPoint> = run.positions().iter().copied().collect();
            run_font.draw_glyphs(&glyphs, &positions, cx.clone());
        }
        cx.restore();
    }
}
