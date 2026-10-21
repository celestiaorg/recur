//! Glyph painting. On macOS, glyphs from the bundled faces are rasterized through
//! CoreGraphics with WebKit's text settings (the app's WKWebView draws text this way), so
//! text matches the web UI pixel for pixel. Glyphs from other fonts (system fallbacks) and
//! other platforms use GPUI's glyph atlas.

use gpui_kit::{FontId, GlyphId, Hsla, Pixels, Point, Window, px};

/// Paint one glyph with its baseline origin at `origin` (CSS px; `y` already on a device
/// pixel).
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint(
    window: &mut Window,
    font_id: FontId,
    face: Option<usize>,
    glyph: GlyphId,
    size: f32,
    origin: Point<Pixels>,
    color: Hsla,
    is_emoji: bool,
) {
    if is_emoji {
        let _ = window.paint_emoji(origin, font_id, glyph, px(size));
        return;
    }
    #[cfg(target_os = "macos")]
    if mac::paint(window, font_id, face, glyph, size, origin, color) {
        return;
    }
    // A glyph the atlas cannot rasterize paints nothing, as in the browser.
    let _ = window.paint_glyph(origin, font_id, glyph, px(size), color);
}

/// Paint a glyph shaped by CoreText (`ct_text`) at `origin` (CSS px; `y` on a device pixel).
#[cfg(target_os = "macos")]
pub(crate) fn paint_ct(
    window: &mut Window,
    font: usize,
    glyph: u16,
    size: f32,
    origin: Point<Pixels>,
    color: Hsla,
) {
    mac::paint_source(
        window,
        mac::Source::Ct(font),
        glyph as u32,
        size,
        origin,
        color,
    );
}

#[cfg(target_os = "macos")]
mod mac {
    use crate::fonts::{FACES, match_face};
    use gpui_kit::{
        Bounds, Corners, FontId, GlyphId, Hsla, Pixels, Point, RenderImage, Rgba, Window, point,
        px, size,
    };
    use smallvec::SmallVec;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Where a glyph's outlines come from.
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub(super) enum Source {
        /// A bundled face (index into `FACES`).
        Face(usize),
        /// A CoreText font slot from `ct_text`.
        Ct(usize),
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    struct Key {
        source: Source,
        glyph: u32,
        size: u32,
        frac: u32,
        color: [u8; 4],
        scale: u32,
    }

    struct Raster {
        image: Arc<RenderImage>,
        /// Bitmap top-left relative to the integer device pixel of the origin.
        dx: i32,
        dy: i32,
        w: u32,
        h: u32,
    }

    #[derive(Default)]
    struct State {
        faces: HashMap<FontId, Option<usize>>,
        rasters: HashMap<Key, Option<Raster>>,
    }

    thread_local! {
        static STATE: RefCell<State> = RefCell::default();
    }

    fn face_for(window: &Window, id: FontId, st: &mut State) -> Option<usize> {
        *st.faces.entry(id).or_insert_with(|| {
            let font = window.text_system().get_font_for_id(id)?;
            match_face(&font.family, font.weight.0)
        })
    }

    pub(super) fn paint(
        window: &mut Window,
        font_id: FontId,
        face: Option<usize>,
        glyph: GlyphId,
        font_size: f32,
        origin: Point<Pixels>,
        color: Hsla,
    ) -> bool {
        let face =
            face.or_else(|| STATE.with(|st| face_for(window, font_id, &mut st.borrow_mut())));
        match face {
            Some(face) => paint_source(
                window,
                Source::Face(face),
                glyph.0,
                font_size,
                origin,
                color,
            ),
            None => false,
        }
    }

    pub(super) fn paint_source(
        window: &mut Window,
        source: Source,
        glyph: u32,
        font_size: f32,
        origin: Point<Pixels>,
        color: Hsla,
    ) -> bool {
        let scale = window.scale_factor();
        let rgba = Rgba::from(color);
        let q = |x: f32| (x * 255.).round().clamp(0., 255.) as u8;
        let color8 = [q(rgba.r), q(rgba.g), q(rgba.b), q(rgba.a)];
        let xd = f32::from(origin.x) * scale;
        let yd = (f32::from(origin.y) * scale).round();
        let ix = xd.floor();
        let frac = ((xd - ix) * 64.).round() as u32;
        let key = Key {
            source,
            glyph,
            size: font_size.to_bits(),
            frac,
            color: color8,
            scale: scale.to_bits(),
        };
        let raster = STATE.with(|st| {
            let mut st = st.borrow_mut();
            st.rasters
                .entry(key)
                .or_insert_with(|| {
                    render(source, glyph, font_size, frac as f32 / 64., color8, scale)
                })
                .as_ref()
                .map(|r| (r.image.clone(), r.dx, r.dy, r.w, r.h))
        });
        let Some((image, dx, dy, w, h)) = raster else {
            return false;
        };
        let bounds = Bounds {
            origin: point(px((ix + dx as f32) / scale), px((yd + dy as f32) / scale)),
            size: size(px(w as f32 / scale), px(h as f32 / scale)),
        };
        let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
        true
    }

    fn render(
        source: Source,
        glyph: u32,
        font_size: f32,
        frac: f32,
        color: [u8; 4],
        scale: f32,
    ) -> Option<Raster> {
        // Glyph box in device px, y up.
        let (x0, y0, x1, y1) = match source {
            Source::Face(face) => {
                let parsed = ttf_parser::Face::parse(FACES.get(face)?.2, 0).ok()?;
                let b = parsed.glyph_bounding_box(ttf_parser::GlyphId(glyph as u16))?;
                let k = font_size / parsed.units_per_em() as f32 * scale;
                (
                    b.x_min as f32 * k,
                    b.y_min as f32 * k,
                    b.x_max as f32 * k,
                    b.y_max as f32 * k,
                )
            }
            Source::Ct(font) => {
                let (a, b, c, d) = crate::ct_text::glyph_bounds(font, glyph as u16)?;
                (a * scale, b * scale, c * scale, d * scale)
            }
        };
        // Room for the outline plus CoreGraphics' anti-aliasing and smoothing spread.
        let pad = 2;
        let left = x0.floor() as i32 - pad;
        let right = x1.ceil() as i32 + pad + 1;
        let top = y1.ceil() as i32 + pad;
        let bottom = y0.floor() as i32 - pad;
        let (w, h) = ((right - left) as u32, (top - bottom) as u32);
        // Baseline origin inside the bitmap, CSS px from its top-left.
        let ox = (-left as f32 + frac) / scale;
        let oy = top as f32 / scale;
        let rgb = (
            color[0] as f32 / 255.,
            color[1] as f32 / 255.,
            color[2] as f32 / 255.,
            1.,
        );
        let mut px = match source {
            Source::Face(face) => crate::cg_raster::render_glyph(
                FACES.get(face)?.2,
                glyph as u16,
                font_size,
                (ox, oy),
                rgb,
                crate::cg_raster::TextSmoothing::Auto,
                w,
                h,
                scale,
            )?,
            Source::Ct(font) => crate::ct_text::render(
                font,
                glyph as u16,
                (ox, oy),
                (rgb.0, rgb.1, rgb.2),
                w,
                h,
                scale,
            )?,
        };
        if px.chunks_exact(4).all(|p| p[3] == 0) {
            return None;
        }
        // Coverage comes out as alpha; the color is exact (straight BGRA for the atlas).
        for p in px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            p[0] = color[2];
            p[1] = color[1];
            p[2] = color[0];
            p[3] = ((a * color[3] as u32 + 127) / 255) as u8;
        }
        let buffer = image::RgbaImage::from_raw(w, h, px)?;
        Some(Raster {
            image: Arc::new(RenderImage::new(SmallVec::from_elem(
                image::Frame::new(buffer),
                1,
            ))),
            dx: left,
            dy: -top,
            w,
            h,
        })
    }
}
