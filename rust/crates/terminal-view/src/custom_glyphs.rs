//! xterm.js's custom glyph rasterizer (`@xterm/addon-webgl` `CustomGlyphRasterizer.ts`): box
//! drawing, block elements, shades, powerline, braille, progress, git-branch and legacy computing
//! symbols are drawn to fill the cell exactly instead of coming from the font.
//!
//! Work happens in device pixels relative to the cell's top-left, like the canvas the atlas
//! rasterizes into; the table lives in [`crate::custom_glyph_data`].

use std::cell::RefCell;
use std::collections::HashMap;
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
#[cfg(test)]
fn has(cp: u32) -> bool {
    lookup(cp).is_some()
}

/// Shades use a 2-phase variant so patterns tile seamlessly across odd cell sizes.
fn is_pattern(cp: u32) -> bool {
    lookup(cp).is_some_and(|parts| parts.iter().any(|p| matches!(p.kind, K::Pattern(_))))
}

struct Ctx<'a> {
    window: &'a mut Window,
    origin: Point<Pixels>,
    dpr: f32,
    color: Hsla,
}

impl Ctx<'_> {
    fn pt(&self, x: f32, y: f32) -> Point<Pixels> {
        point(
            self.origin.x + px(x / self.dpr),
            self.origin.y + px(y / self.dpr),
        )
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let b = Bounds::new(self.pt(x, y), size(px(w / self.dpr), px(h / self.dpr)));
        self.window.paint_quad(fill(b, self.color));
    }

    fn clip_cell<R>(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        f: impl for<'b> FnOnce(&mut Ctx<'b>) -> R,
    ) -> R {
        let bounds = Bounds::new(self.pt(x, y), size(px(w / self.dpr), px(h / self.dpr)));
        let origin = self.origin;
        let (dpr, color) = (self.dpr, self.color);
        self.window
            .with_content_mask(Some(ContentMask { bounds }), |window| {
                f(&mut Ctx {
                    window,
                    origin,
                    dpr,
                    color,
                })
            })
    }

    fn draw(&mut self, b: PathBuilder) {
        if let Ok(path) = b.build() {
            self.window.paint_path(path, self.color);
        }
    }
}

/// Paints `cp` into the cell at `origin` if it has a custom definition. Returns false otherwise.
pub(crate) fn paint(
    window: &mut Window,
    cp: u32,
    origin: Point<Pixels>,
    _width: usize,
    m: &CellMetrics,
    color: Hsla,
) -> bool {
    let Some(parts) = lookup(cp) else {
        return false;
    };
    let mut cx = Ctx {
        window,
        origin,
        dpr: m.dpr,
        color,
    };
    let col_x = (f32::from(origin.x) * m.dpr).round();
    let row_y = (f32::from(origin.y) * m.dpr).round();
    let variant = if is_pattern(cp) {
        ((col_x % 2.0) as u8) * 2 + (row_y % 2.0) as u8
    } else {
        0
    };
    for (i, part) in parts.iter().enumerate() {
        let (mut x, mut y, mut w, mut h) = (0.0, 0.0, m.cell_width, m.cell_height);
        if part.char_scale {
            w = m.char_width;
            h = m.char_height;
            x = (m.cell_width - m.char_width) / 2.0;
            y = (m.cell_height - m.char_height) / 2.0;
        }
        draw_part(&mut cx, cp, i, part, x, y, w, h, m, variant);
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn draw_part(
    cx: &mut Ctx,
    cp: u32,
    index: usize,
    part: &P,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    m: &CellMetrics,
    variant: u8,
) {
    match &part.kind {
        K::Octants(boxes) => {
            let (xe, ye) = (w / 8.0, h / 8.0);
            for b in *boxes {
                cx.rect(x + b[0] * xe, y + b[1] * ye, b[2] * xe, b[3] * ye);
            }
        }
        K::Pattern(rows) => draw_pattern(cx, cp, index, rows, part.clip, x, y, w, h, variant),
        K::PathFn(ins) => {
            let (xp, yp) = (0.15, 0.15 / m.cell_height * m.cell_width);
            cx.clip_cell(x, y, w, h, |cx| {
                let dpr = cx.dpr;
                let b = svg_path(
                    ins,
                    xp,
                    yp,
                    |v, axis| translate(v, axis, w, h, x, y, true, 0.0, 0.0, dpr),
                    false,
                    cx.origin,
                    dpr,
                );
                cx.draw(style(b, part.stroke_width));
            });
        }
        K::Path(ins) => {
            let b = svg_path(
                ins,
                0.0,
                0.0,
                |v, axis| if axis == 0 { x + v * w } else { y + v * h },
                true,
                cx.origin,
                cx.dpr,
            );
            cx.draw(style(b, part.stroke_width));
        }
        // xterm draws the "negative" shape in the atlas background colour, which is fully
        // transparent when `allowTransparency` is on (as in Recur's pane): only the fill shows.
        K::PathNegative => cx.rect(x, y, w, h),
        K::Vector {
            d,
            stroke,
            left_padding,
            right_padding,
        } => {
            let css_line = m.font_size / 12.0;
            let (lp, rp) = (
                left_padding * (css_line / 2.0),
                right_padding * (css_line / 2.0),
            );
            cx.clip_cell(x, y, w, h, |cx| {
                let dpr = cx.dpr;
                let b = svg_path(
                    d,
                    0.0,
                    0.0,
                    |v, axis| translate(v, axis, w, h, x, y, false, lp, rp, dpr),
                    false,
                    cx.origin,
                    dpr,
                );
                cx.draw(style(b, stroke.then_some(css_line)));
            });
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
                if pattern & (1 << bit) == 0 {
                    continue;
                }
                let (ccx, ccy) = (x + (dx + 1.0) * xe, y + pad_y + (dy + 1.0) * ye);
                let mut b =
                    PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
                let radii = point(px(r / cx.dpr), px(r / cx.dpr));
                b.move_to(cx.pt(ccx + r, ccy));
                b.arc_to(radii, px(0.0), false, true, cx.pt(ccx - r, ccy));
                b.arc_to(radii, px(0.0), false, true, cx.pt(ccx + r, ccy));
                b.close();
                cx.draw(b);
            }
        }
    }
}

/// Stroke width in CSS pixels (`lineWidth = devicePixelRatio * width` on the device canvas).
fn style(b: PathBuilder, stroke: Option<f32>) -> PathBuilder {
    match stroke {
        Some(w) => b.with_style(PathStyle::Stroke(
            gpui_kit::StrokeOptions::default().with_line_width(w),
        )),
        None => b.with_style(PathStyle::Fill(FillOptions::non_zero())),
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
fn svg_path(
    ins: &[I],
    xp: f32,
    yp: f32,
    map: impl Fn(f32, usize) -> f32,
    full: bool,
    origin: Point<Pixels>,
    dpr: f32,
) -> PathBuilder {
    let mut b = PathBuilder::fill();
    let (mut cur_x, mut cur_y, mut ctl_x, mut ctl_y) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut last = b' ';
    let mut open = false;
    let lin = |a: &Option<[f32; 3]>| a.map(|c| c[0] + c[1] * xp + c[2] * yp);
    let to_pt = |x: f32, y: f32| point(origin.x + px(x / dpr), origin.y + px(y / dpr));
    for i in ins {
        if i.cmd == b'Z' {
            if open {
                b.close();
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
            b.line_to(to_pt(cur_x, cur_y));
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
            if rx == 0.0 || ry == 0.0 {
                b.line_to(to_pt(x2, y2));
            } else {
                b.arc_to(
                    point(px(rx / dpr), px(ry / dpr)),
                    px(a[2]),
                    a[3] != 0.0,
                    a[4] != 0.0,
                    to_pt(x2, y2),
                );
            }
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
                b.move_to(to_pt(v[0], v[1]));
                open = true;
                (cur_x, cur_y, ctl_x, ctl_y) = (v[0], v[1], v[0], v[1]);
            }
            b'L' => {
                b.line_to(to_pt(v[0], v[1]));
                (cur_x, cur_y, ctl_x, ctl_y) = (v[0], v[1], v[0], v[1]);
            }
            b'Q' if v.len() >= 4 => {
                b.curve_to(to_pt(v[2], v[3]), to_pt(v[0], v[1]));
                (ctl_x, ctl_y, cur_x, cur_y) = (v[0], v[1], v[2], v[3]);
            }
            b'T' => {
                let (cpx, cpy) = if last == b'Q' || last == b'T' {
                    (2.0 * cur_x - ctl_x, 2.0 * cur_y - ctl_y)
                } else {
                    (cur_x, cur_y)
                };
                b.curve_to(to_pt(v[0], v[1]), to_pt(cpx, cpy));
                (ctl_x, ctl_y, cur_x, cur_y) = (cpx, cpy, v[0], v[1]);
            }
            b'C' if v.len() >= 6 => {
                b.cubic_bezier_to(to_pt(v[4], v[5]), to_pt(v[0], v[1]), to_pt(v[2], v[3]));
                (ctl_x, ctl_y, cur_x, cur_y) = (v[2], v[3], v[4], v[5]);
            }
            _ => continue,
        }
        last = i.cmd;
    }
    b
}

/// Codepoint, part, cell width, cell height, variant, colour.
type PatternKey = (u32, usize, u32, u32, u8, u32);

thread_local! {
    static PATTERNS: RefCell<HashMap<PatternKey, Arc<RenderImage>>> = RefCell::new(HashMap::new());
}

#[allow(clippy::too_many_arguments)]
fn draw_pattern(
    cx: &mut Ctx,
    cp: u32,
    index: usize,
    rows: &[&[f32]],
    clip: Option<&[I]>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    variant: u8,
) {
    let (iw, ih) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
    let rgba = gpui_kit::Rgba::from(cx.color);
    let color = u32::from_be_bytes([
        (rgba.r * 255.0).round() as u8,
        (rgba.g * 255.0).round() as u8,
        (rgba.b * 255.0).round() as u8,
        (rgba.a * 255.0).round() as u8,
    ]);
    let key = (cp, index, iw, ih, variant, color);
    let polygon: Option<Vec<(f32, f32)>> = clip.map(|ins| {
        ins.iter()
            .filter(|i| (i.cmd == b'M' || i.cmd == b'L') && i.args.len() >= 2)
            .filter_map(|i| Some((i.args[0]?[0] * w, i.args[1]?[0] * h)))
            .collect()
    });
    let image = PATTERNS.with(|cache| {
        cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let (dx, dy) = (u32::from((variant >> 1) & 1), u32::from(variant & 1));
                let (pw, ph) = (rows[0].len() as u32, rows.len() as u32);
                let mut img = image::RgbaImage::new(iw, ih);
                for py in 0..ih {
                    for pxl in 0..iw {
                        let on = rows[((py + dy) % ph) as usize][((pxl + dx) % pw) as usize];
                        let inside = polygon.as_ref().is_none_or(|poly| {
                            point_in_polygon(poly, pxl as f32 + 0.5, py as f32 + 0.5)
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
            .clone()
    });
    let bounds = Bounds::new(cx.pt(x, y), size(px(w / cx.dpr), px(h / cx.dpr)));
    let _ = cx
        .window
        .paint_image(bounds, bounds, Corners::default(), image, 0, false);
}

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
            assert!(has(cp), "missing U+{cp:04X}");
        }
        assert!(!has('a' as u32));
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
