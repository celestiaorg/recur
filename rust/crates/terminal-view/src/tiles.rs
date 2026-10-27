//! The grid composited the way xterm.js's WebGL renderer and WebKit produce it, one row at a time.
//!
//! The renderer clears its canvas to the palette background, fills background rectangles, blends
//! every glyph's atlas texels with `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` in cell order (alpha blends
//! the same way, so glyph edges leave the canvas partly transparent), draws the cursor, and the
//! page composites that canvas as premultiplied over the `.xterm` background behind it. Glyph
//! edges therefore carry `a(1 - a)` of that background on top of the blend, which no straight
//! blend of a glyph layer can produce over arbitrary colours. So the grid is composited here into
//! row tiles with the exact 8-bit math and painted as images. A tile covers one row's pixels and
//! depends on its row and the two around it (glyph rasters spill a few pixels into neighbours);
//! tiles are cached by content, so scrolling and unchanged rows reuse them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use gpui_kit::{Bounds, Corners, Pixels, Point, RenderImage, Window, point, px, size};
use recur_term::screen::{CursorShape, Frame, Glyph, Rgba, UnderlineStyle};
use smallvec::SmallVec;

use crate::atlas::{self, FontKey, GlyphKey, Raster};
use crate::custom_glyphs;
use crate::font::CellMetrics;

/// Tile bytes kept for reuse.
const BUDGET: usize = 96 << 20;

struct Tile {
    image: Arc<RenderImage>,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
struct Cache {
    tiles: HashMap<u64, Tile>,
    bytes: usize,
    frame: u64,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

/// One glyph placed on the grid.
struct Placed {
    key: GlyphKey,
    col: usize,
    row: usize,
    clip_left: bool,
}

/// A glyph raster at its grid position (device pixels), clipped left of `clip_left`.
struct Positioned {
    raster: Arc<Raster>,
    left: i64,
    top: i64,
    clip_left: Option<i64>,
}

/// A device-pixel rectangle on the grid with its colour.
#[derive(Hash)]
struct Fill {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    color: Rgba,
}

/// The atlas key for a frame glyph.
pub(crate) fn glyph_key(g: &Glyph, font: &FontKey, m: &CellMetrics) -> GlyphKey {
    let dotted = g
        .underline
        .is_some_and(|u| u.style == UnderlineStyle::Dotted);
    // CellColorResolver: `x * cellWidth % (round(lineWidth) * 2)` for a dotted underline, and
    // when that is 0, a shade pattern's phase.
    let mut variant_offset = 0;
    if dotted {
        let period = (m.underline_width().round() * 2.0) as u32;
        variant_offset = (g.col as u32 * m.cell_width as u32) % period;
    }
    if variant_offset == 0 {
        variant_offset = u32::from(custom_glyphs::pattern_variant(
            g.codepoint,
            g.col as f32 * m.cell_width,
            g.row as f32 * m.cell_height,
        ));
    }
    GlyphKey {
        font: font.clone(),
        text: g.text.clone(),
        bold: g.bold,
        italic: g.italic,
        fg: g.fg,
        underline: g.underline.map(|u| (u.style, u.color)),
        variant_offset,
        strikethrough: g.strikethrough,
        overline: g.overline,
        width: g.width,
        custom: custom_glyphs::has(g.codepoint),
    }
}

/// `RectangleRenderer.renderCursor` for the non-block shapes, device pixels.
fn cursor_fills(frame: &Frame, m: &CellMetrics) -> Vec<Fill> {
    let Some(c) = frame.cursor else {
        return Vec::new();
    };
    let (cw, ch, dpr) = (
        m.cell_width as i64,
        m.cell_height as i64,
        m.dpr.round() as i64,
    );
    let (x, y) = (c.col as i64 * cw, c.row as i64 * ch);
    let w = c.width as i64 * cw;
    let r = |x, y, w, h| Fill {
        x,
        y,
        w,
        h,
        color: c.color,
    };
    match c.shape {
        CursorShape::Bar => vec![r(x, y, dpr, ch)],
        CursorShape::Underline => vec![r(x, y + ch - dpr, w, dpr)],
        CursorShape::Outline => vec![
            r(x, y, dpr, ch),
            r(x, y + ch - dpr, w, dpr),
            r(x, y, w, dpr),
            r(x + w - dpr, y, dpr, ch),
        ],
    }
}

/// Paints the grid's cells (backgrounds, glyphs and decorations, cursor) at `origin`.
/// `backdrop` is the `.xterm` background behind the canvas. Custom glyphs are painted on top
/// by the caller.
pub(crate) fn paint(
    window: &mut Window,
    origin: Point<Pixels>,
    frame: &Frame,
    m: &CellMetrics,
    font: &FontKey,
    backdrop: Rgba,
) {
    let (cw, ch) = (m.cell_width as i64, m.cell_height as i64);
    let width = frame.cols as i64 * cw;
    let mut rows: Vec<(Vec<Fill>, Vec<Placed>)> =
        (0..frame.rows).map(|_| (Vec::new(), Vec::new())).collect();
    for r in &frame.rects {
        rows[r.row].0.push(Fill {
            x: r.col as i64 * cw,
            y: r.row as i64 * ch,
            w: r.len as i64 * cw,
            h: ch,
            color: r.color,
        });
    }
    for g in &frame.glyphs {
        rows[g.row].1.push(Placed {
            key: glyph_key(g, font, m),
            col: g.col,
            row: g.row,
            clip_left: g.clip_left,
        });
    }
    let cursor = cursor_fills(frame, m);
    let cursor_row = frame.cursor.map(|c| c.row);
    let dpr = m.dpr;

    let mut drop = Vec::new();
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.frame += 1;
        let now = cache.frame;
        for row in 0..frame.rows {
            let band = row.saturating_sub(1)..(row + 2).min(frame.rows);
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (row, frame.rows, width, frame.background, backdrop, font).hash(&mut h);
            for r in band.clone() {
                r.hash(&mut h);
                rows[r].0.hash(&mut h);
                for p in &rows[r].1 {
                    (&p.key, p.col, p.clip_left).hash(&mut h);
                }
            }
            if cursor_row == Some(row) {
                cursor.hash(&mut h);
            }
            let id = h.finish();
            if !cache.tiles.contains_key(&id) {
                let glyphs = band.flat_map(|r| rows[r].1.iter());
                let glyphs = glyphs.filter_map(|p| {
                    let raster = atlas::raster(&p.key)?;
                    let (ox, oy) = atlas::canvas_origin(&p.key, m);
                    let cell_x = p.col as i64 * cw;
                    Some(Positioned {
                        left: cell_x + ox + raster.left as i64,
                        top: p.row as i64 * ch + oy + raster.top as i64,
                        clip_left: p.clip_left.then_some(cell_x),
                        raster,
                    })
                });
                let pixels = compose(
                    row as i64 * ch,
                    width,
                    ch,
                    frame.background,
                    backdrop,
                    rows[row].0.iter(),
                    glyphs,
                    cursor.iter().filter(|_| cursor_row == Some(row)),
                );
                let image = image::RgbaImage::from_raw(width as u32, ch as u32, pixels)
                    .unwrap_or_else(|| image::RgbaImage::new(width as u32, ch as u32));
                let image = Arc::new(RenderImage::new(SmallVec::from_elem(
                    image::Frame::new(image),
                    1,
                )));
                let bytes = (width * ch * 4) as usize;
                cache.bytes += bytes;
                cache.tiles.insert(
                    id,
                    Tile {
                        image,
                        bytes,
                        used: now,
                    },
                );
            }
            let tile = cache.tiles.get_mut(&id).map(|t| {
                t.used = now;
                t.image.clone()
            });
            if let Some(image) = tile {
                let bounds = Bounds::new(
                    point(origin.x, origin.y + px((row as i64 * ch) as f32 / dpr)),
                    size(px(width as f32 / dpr), px(ch as f32 / dpr)),
                );
                let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
            }
        }
        // Evict the least recently painted tiles past the budget (never this frame's).
        if cache.bytes > BUDGET {
            let mut old: Vec<(u64, u64)> = cache
                .tiles
                .iter()
                .filter(|(_, t)| t.used < now)
                .map(|(id, t)| (t.used, *id))
                .collect();
            old.sort_unstable();
            for (_, id) in old {
                if cache.bytes <= BUDGET {
                    break;
                }
                if let Some(t) = cache.tiles.remove(&id) {
                    cache.bytes -= t.bytes;
                    drop.push(t.image);
                }
            }
        }
    });
    for image in drop {
        let _ = window.drop_image(image);
    }
}

/// One row's pixels: rows `y0..y0 + height` of the grid canvas.
#[allow(clippy::too_many_arguments)]
fn compose<'a>(
    y0: i64,
    width: i64,
    height: i64,
    background: Rgba,
    backdrop: Rgba,
    rects: impl Iterator<Item = &'a Fill>,
    glyphs: impl Iterator<Item = Positioned>,
    cursor: impl Iterator<Item = &'a Fill>,
) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    // The WebGL drawing buffer: 8-bit colour and alpha, cleared to transparent black.
    let mut buf = vec![0u8; w * h * 4];
    let blend = |buf: &mut [u8], x: i64, y: i64, c: [u8; 3], a: u8| {
        let i = ((y - y0) as usize * w + x as usize) * 4;
        let (sa, da) = (a as f32 / 255.0, 1.0 - a as f32 / 255.0);
        for k in 0..3 {
            buf[i + k] = (c[k] as f32 * sa + buf[i + k] as f32 * da).round() as u8;
        }
        buf[i + 3] = (a as f32 * sa + buf[i + 3] as f32 * da).round() as u8;
    };
    let fill = |buf: &mut [u8], f: &Fill| {
        let (x1, x2) = (f.x.max(0), (f.x + f.w).min(width));
        let (y1, y2) = (f.y.max(y0), (f.y + f.h).min(y0 + height));
        let c = [f.color.r(), f.color.g(), f.color.b()];
        for y in y1..y2 {
            for x in x1..x2 {
                blend(buf, x, y, c, f.color.a());
            }
        }
    };
    // The viewport-clear rectangle, then the background runs.
    fill(
        &mut buf,
        &Fill {
            x: 0,
            y: y0,
            w: width,
            h: height,
            color: background,
        },
    );
    for r in rects {
        fill(&mut buf, r);
    }
    for g in glyphs {
        blend_raster(
            &mut buf,
            &g.raster,
            g.left,
            g.top,
            y0,
            width,
            height,
            g.clip_left,
            &blend,
        );
    }
    for c in cursor {
        fill(&mut buf, c);
    }
    // The page composites the canvas as premultiplied over the backdrop; GPUI takes straight
    // BGRA.
    let pa = backdrop.a() as f32 / 255.0;
    let p = [
        backdrop.r() as f32 * pa,
        backdrop.g() as f32 * pa,
        backdrop.b() as f32 * pa,
    ];
    let mut out = Vec::with_capacity(buf.len());
    for px in buf.chunks_exact(4) {
        let t = 1.0 - px[3] as f32 / 255.0;
        let a = (px[3] as f32 + t * pa * 255.0).round().min(255.0);
        let c = |k: usize| {
            let v = (px[k] as f32 + t * p[k]).round().min(255.0);
            if a >= 255.0 || a == 0.0 {
                v as u8
            } else {
                (v * 255.0 / a).round().min(255.0) as u8
            }
        };
        out.extend_from_slice(&[c(2), c(1), c(0), a as u8]);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn blend_raster(
    buf: &mut [u8],
    raster: &Raster,
    left: i64,
    top: i64,
    y0: i64,
    width: i64,
    height: i64,
    clip_left: Option<i64>,
    blend: &impl Fn(&mut [u8], i64, i64, [u8; 3], u8),
) {
    let min_x = clip_left.unwrap_or(0).max(0);
    for ry in 0..raster.h as i64 {
        let y = top + ry;
        if y < y0 || y >= y0 + height {
            continue;
        }
        for rx in 0..raster.w as i64 {
            let x = left + rx;
            if x < min_x || x >= width {
                continue;
            }
            let i = (ry as usize * raster.w + rx as usize) * 4;
            let t = &raster.texels[i..i + 4];
            if t[3] != 0 {
                blend(buf, x, y, [t[0], t[1], t[2]], t[3]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(w: usize, texels: &[[u8; 4]]) -> Arc<Raster> {
        Arc::new(Raster {
            texels: texels.concat(),
            left: 0,
            top: 0,
            w,
            h: texels.len() / w,
        })
    }

    fn at(raster: Arc<Raster>, left: i64, clip_left: Option<i64>) -> Positioned {
        Positioned {
            raster,
            left,
            top: 0,
            clip_left,
        }
    }

    const BG: Rgba = Rgba::rgb(20, 20, 20);

    /// BGRA pixels back to RGB.
    fn rgb(px: &[u8]) -> Vec<[u8; 3]> {
        px.chunks_exact(4).map(|p| [p[2], p[1], p[0]]).collect()
    }

    #[test]
    fn glyph_edges_carry_the_backdrop_through_the_canvas_alpha() {
        let glyph = raster(1, &[[230, 228, 255, 128]]);
        let px = compose(
            0,
            1,
            1,
            BG,
            BG,
            std::iter::empty(),
            [at(glyph, 0, None)].into_iter(),
            std::iter::empty(),
        );
        // Blend: 230·a + 20·(1 - a) = 125; canvas alpha 128·a + 255·(1 - a) = 191; the page
        // adds (1 - 191/255) · 20.
        assert_eq!(rgb(&px), vec![[130, 129, 143]]);
        assert_eq!(px[3], 255);
    }

    #[test]
    fn rects_glyphs_and_cursor_draw_in_renderer_order() {
        let fill = Fill {
            x: 1,
            y: 0,
            w: 1,
            h: 1,
            color: Rgba::rgb(0, 0, 200),
        };
        let cursor = Fill {
            x: 2,
            y: 0,
            w: 1,
            h: 1,
            color: Rgba::rgb(255, 0, 0),
        };
        let a = raster(3, &[[255, 255, 255, 255]; 3]);
        let b = raster(1, &[[0, 255, 0, 255]]);
        let px = compose(
            0,
            3,
            1,
            BG,
            BG,
            [&fill].into_iter(),
            [at(a, 0, None), at(b, 1, None)].into_iter(),
            [&cursor].into_iter(),
        );
        assert_eq!(
            rgb(&px),
            vec![[255, 255, 255], [0, 255, 0], [255, 0, 0]],
            "later glyphs over earlier ones, the cursor over both"
        );
    }

    #[test]
    fn overhang_left_of_a_background_change_is_clipped() {
        let wide = raster(2, &[[255, 255, 255, 255]; 2]);
        let px = compose(
            0,
            2,
            1,
            BG,
            BG,
            std::iter::empty(),
            [at(wide, 0, Some(1))].into_iter(),
            std::iter::empty(),
        );
        assert_eq!(rgb(&px), vec![[20, 20, 20], [255, 255, 255]]);
    }

    #[test]
    fn transparent_cells_stay_translucent() {
        let clear = Rgba::new(0, 0, 0, 0);
        let glyph = raster(2, &[[200, 100, 50, 255], [200, 100, 50, 0]]);
        let px = compose(
            0,
            2,
            1,
            clear,
            clear,
            std::iter::empty(),
            [at(glyph, 0, None)].into_iter(),
            std::iter::empty(),
        );
        assert_eq!(&px[..4], &[50, 100, 200, 255]);
        assert_eq!(px[7], 0);
    }

    fn key(text: &str, overline: bool, strikethrough: bool, size: f32) -> GlyphKey {
        // JetBrains Mono at 2x.
        let m = CellMetrics::compute(size * 0.6, size * 1.02, size * 0.3, size, 1.15, 2.0);
        GlyphKey {
            font: FontKey::new("JetBrains Mono", &m, 1.15),
            text: text.into(),
            bold: false,
            italic: false,
            fg: Rgba::rgb(255, 255, 255),
            underline: None,
            variant_offset: 0,
            strikethrough,
            overline,
            width: 1,
            custom: false,
        }
    }

    #[test]
    fn overline_strokes_the_top_row_of_the_character_box() {
        let k = key(" ", true, false, 12.0);
        let r = atlas::raster(&k).expect("an overlined blank is not empty");
        // One device pixel (12px · 2 / 15 floors to 1) at the canvas padding, the character
        // width long.
        assert_eq!((r.left, r.top, r.w, r.h), (4, 4, 14, 1));
        assert!(r.texels.chunks_exact(4).all(|t| t[3] == 255));
        assert!(atlas::raster(&key(" ", false, false, 12.0)).is_none());
    }

    #[test]
    fn overline_width_moves_the_strikethrough_like_the_canvas_state_does() {
        // At 16px the overline is 2 device pixels wide and leaves the context line width even,
        // so the strikethrough is not shifted half a pixel and straddles a pixel boundary.
        let rows = |k: &GlyphKey| {
            let r = atlas::raster(k).unwrap();
            (0..r.h)
                .filter(|y| (0..r.w).any(|x| r.texels[(y * r.w + x) * 4 + 3] != 0))
                .map(|y| (y + r.top, r.texels[(y * r.w + r.w / 2) * 4 + 3]))
                .collect::<Vec<_>>()
        };
        let strike = rows(&key(" ", false, true, 16.0));
        assert_eq!(strike.len(), 3);
        assert!(strike.iter().all(|(_, a)| *a == 255));
        let both = rows(&key(" ", true, true, 16.0));
        let struck: Vec<_> = both.iter().filter(|(y, _)| *y > 6).collect();
        assert_eq!(struck.len(), 4);
        assert!(struck.first().is_some_and(|(_, a)| *a < 255));
    }
}
