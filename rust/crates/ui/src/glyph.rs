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

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Key {
        source: Source,
        glyph: u32,
        size: u32,
        frac: u32,
        color: [u8; 4],
        scale: u32,
    }

    impl std::hash::Hash for Key {
        fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
            let (kind, ix) = match self.source {
                Source::Face(i) => (0u64, i as u64),
                Source::Ct(i) => (1, i as u64),
            };
            h.write_u64(kind << 63 | ix << 32 | u64::from(self.glyph));
            h.write_u64(u64::from(self.size) << 32 | u64::from(self.frac));
            h.write_u64(u64::from(u32::from_le_bytes(self.color)) << 32 | u64::from(self.scale));
        }
    }

    /// A multiply-rotate hasher for the raster map: every painted glyph looks one up each
    /// frame, and SipHash showed in the frame time.
    #[derive(Default)]
    struct KeyHasher(u64);

    impl std::hash::Hasher for KeyHasher {
        fn finish(&self) -> u64 {
            self.0
        }
        fn write(&mut self, bytes: &[u8]) {
            for b in bytes {
                self.write_u64(u64::from(*b));
            }
        }
        fn write_u64(&mut self, v: u64) {
            self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
        }
    }

    type KeyMap<V> = HashMap<Key, V, std::hash::BuildHasherDefault<KeyHasher>>;

    struct Raster {
        image: Arc<RenderImage>,
        /// Bitmap top-left relative to the integer device pixel of the origin.
        dx: i32,
        dy: i32,
        w: u32,
        h: u32,
    }

    /// The glyph bitmaps kept, by their pixel bytes: every glyph, size, color and subpixel
    /// offset on screen in the last several screens, far more than one view shows.
    const RASTER_MAX_BYTES: usize = 64 * 1024 * 1024;

    #[derive(Default)]
    struct State {
        faces: HashMap<FontId, Option<usize>>,
        rasters: KeyMap<(Option<Raster>, u64)>,
        order: std::collections::BTreeMap<u64, Key>,
        stamp: u64,
        bytes: usize,
    }

    /// A cached raster: the image and its offset and size in device pixels.
    type Hit = (Arc<RenderImage>, i32, i32, u32, u32);

    impl State {
        fn raster_bytes(r: &Option<Raster>) -> usize {
            64 + r.as_ref().map_or(0, |r| (r.w * r.h * 4) as usize)
        }

        /// The raster for `key`, rendering it on a miss; least recently used bitmaps past the
        /// budget are returned for the caller to drop from the atlas.
        fn get(
            &mut self,
            key: Key,
            render: impl FnOnce() -> Option<Raster>,
        ) -> (Option<Hit>, Vec<Arc<RenderImage>>) {
            self.stamp += 1;
            let stamp = self.stamp;
            let mut evicted = Vec::new();
            match self.rasters.get_mut(&key) {
                // Lazy LRU: a hit only restamps; eviction requeues entries used since.
                Some((_, at)) => *at = stamp,
                None => {
                    let r = render();
                    self.bytes += Self::raster_bytes(&r);
                    self.rasters.insert(key, (r, stamp));
                    self.order.insert(stamp, key);
                    while self.bytes > RASTER_MAX_BYTES && self.rasters.len() > 1 {
                        let Some((at, old)) = self.order.pop_first() else {
                            break;
                        };
                        match self.rasters.get(&old) {
                            Some((_, used)) if *used != at => {
                                let used = *used;
                                self.order.insert(used, old);
                            }
                            Some(_) => {
                                if let Some((r, _)) = self.rasters.remove(&old) {
                                    self.bytes -= Self::raster_bytes(&r);
                                    evicted.extend(r.map(|r| r.image));
                                }
                            }
                            None => {}
                        }
                    }
                }
            }
            let hit = self.rasters[&key]
                .0
                .as_ref()
                .map(|r| (r.image.clone(), r.dx, r.dy, r.w, r.h));
            (hit, evicted)
        }
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
        let (raster, evicted) = STATE.with(|st| {
            st.borrow_mut().get(key, || {
                render(source, glyph, font_size, frac as f32 / 64., color8, scale)
            })
        });
        for image in evicted {
            let _ = window.drop_image(image);
        }
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
