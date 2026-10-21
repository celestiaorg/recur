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
    GlyphKey {
        font: font.clone(),
        text: g.text.clone(),
        bold: g.bold,
        italic: g.italic,
        fg: g.fg,
        underline: g.underline.map(|u| (u.style, u.color)),
        // CellColorResolver: `x * cellWidth % (round(lineWidth) * 2)`.
        variant_offset: if dotted {
            let period = (m.underline_width().round() * 2.0) as u32;
            (g.col as u32 * m.cell_width as u32) % period
        } else {
            0
        },
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
                let image = compose(
                    row as i64 * ch,
                    width,
                    ch,
                    frame.background,
                    backdrop,
                    rows[row].0.iter(),
                    glyphs,
                    cursor.iter().filter(|_| cursor_row == Some(row)),
                    m,
                );
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
    glyphs: impl Iterator<Item = &'a Placed>,
    cursor: impl Iterator<Item = &'a Fill>,
    m: &CellMetrics,
) -> Arc<RenderImage> {
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
    let (cw, ch) = (m.cell_width as i64, m.cell_height as i64);
    for p in glyphs {
        let Some(raster) = atlas::raster(&p.key) else {
            continue;
        };
        let (ox, oy) = atlas::canvas_origin(&p.key, m);
        let cell_x = p.col as i64 * cw;
        let left = cell_x + ox + raster.left as i64;
        let top = p.row as i64 * ch + oy + raster.top as i64;
        blend_raster(
            &mut buf,
            &raster,
            left,
            top,
            y0,
            width,
            height,
            p.clip_left.then_some(cell_x),
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
    let image = image::RgbaImage::from_raw(w as u32, h as u32, out)
        .unwrap_or_else(|| image::RgbaImage::new(w as u32, h as u32));
    Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(image),
        1,
    )))
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
