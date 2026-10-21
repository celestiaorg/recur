//! Blurred CSS `box-shadow`s drawn as WebKit draws them. GPUI's shadow shader approximates the
//! Gaussian with four samples; WebKit's `ShadowBlur` runs three box blurs per axis over a layer
//! in CSS pixels, scales it up, and never paints an outer shadow under the box itself. A lone
//! zero-blur shadow (a focus ring) is exact on GPUI's `.shadow()`; list one here when it stacks
//! with blurred layers, so the CSS order holds.

use crate::color::Color;
use gpui_kit::{
    App, Bounds, Corners, Div, Global, IntoElement, ParentElement, Pixels, RenderImage, Styled,
    Window, canvas, div, point, px, size,
};
use resvg::tiny_skia;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::sync::Arc;

/// One `box-shadow` layer (CSS px; `blur` is the CSS blur radius).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CssShadow {
    pub x: f32,
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
}

pub const fn css_box_shadow(x: f32, y: f32, blur: f32, spread: f32, color: Color) -> CssShadow {
    CssShadow {
        x,
        y,
        blur,
        spread,
        color,
    }
}

/// `content` with `shadows` behind it (the first listed on top, as in CSS). `radius` is the
/// content's border radius. Size and position the wrapper as the content would be.
pub fn with_shadow(content: impl IntoElement, shadows: &[CssShadow], radius: f32) -> Div {
    let shadows: SmallVec<[CssShadow; 2]> = shadows.iter().copied().collect();
    div()
        .relative()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    for s in shadows.iter().rev() {
                        paint(bounds, *s, radius, window, cx);
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(content)
}

/// `ShadowBlur`'s box sizes: three passes approximating a Gaussian of σ = blur / 2.
pub fn lobes(blur: f32) -> [(usize, usize); 3] {
    let sd = blur / 2.;
    let d = ((sd * 0.75 * (2. * std::f32::consts::PI).sqrt() * 0.88 + 0.5).floor() as usize).max(2);
    if d % 2 == 1 {
        let l = (d - 1) / 2;
        [(l, l); 3]
    } else {
        let l = d / 2;
        [(l, l - 1), (l - 1, l), (l, l)]
    }
}

/// One box blur pass over `line` (stride `step`), as `ShadowBlur::blurLayerImage` sums: edges
/// extend, and the average is a 15-bit fixed-point multiply.
fn box_pass(data: &mut [u8], start: usize, step: usize, n: usize, (left, right): (usize, usize)) {
    let at = |i: isize| data[start + (i.clamp(0, n as isize - 1) as usize) * step] as u32;
    let count = (left + right + 1) as u32;
    let inv = (1u32 << 15).div_ceil(count);
    let mut sum: u32 = (-(left as isize)..=right as isize).map(at).sum();
    let mut out = Vec::with_capacity(n);
    for i in 0..n as isize {
        out.push(((sum * inv) >> 15) as u8);
        sum = sum + at(i + right as isize + 1) - at(i - left as isize);
    }
    for (i, v) in out.into_iter().enumerate() {
        data[start + i * step] = v;
    }
}

fn blur(mask: &mut [u8], w: usize, h: usize, blur: f32) {
    let lobes = lobes(blur);
    for lobe in lobes {
        for y in 0..h {
            box_pass(mask, y * w, 1, w, lobe);
        }
    }
    for lobe in lobes {
        for x in 0..w {
            box_pass(mask, x, w, h, lobe);
        }
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.).min(h / 2.).max(0.);
    let k = 0.552_284_8 * r;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Coverage mask of a rounded rect on a `w`×`h` grid, anti-aliased or by pixel center.
fn coverage(w: u32, h: u32, rect: (f32, f32, f32, f32), r: f32, aa: bool) -> Option<Vec<u8>> {
    let mut pm = tiny_skia::Pixmap::new(w, h)?;
    if rect.2 > 0. && rect.3 > 0. {
        let path = rounded_rect(rect.0, rect.1, rect.2, rect.3, r)?;
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(0, 0, 0, 255);
        paint.anti_alias = aa;
        pm.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
    }
    Some(pm.pixels().iter().map(|p| p.alpha()).collect())
}

/// The shadow image for a box of `box_w`×`box_h` CSS px, and its offset from the box origin
/// (CSS px). Straight-alpha BGRA in device pixels.
fn render(
    box_w: f32,
    box_h: f32,
    radius: f32,
    s: CssShadow,
    scale: f32,
) -> Option<(Arc<RenderImage>, f32, f32)> {
    let (sw, sh) = (box_w + 2. * s.spread, box_h + 2. * s.spread);
    if sw <= 0. || sh <= 0. {
        return None;
    }
    let sr = if radius > 0. {
        (radius + s.spread).max(0.)
    } else {
        0.
    };
    // The layer, in CSS px: the spread shape with room for the blur on every side. Unblurred,
    // it is the shape itself at device resolution.
    let pad = s.blur.ceil() + 1.;
    let (lw, lh) = ((sw + 2. * pad).ceil() as u32, (sh + 2. * pad).ceil() as u32);
    let (dw, dh) = (
        (lw as f32 * scale).round() as u32,
        (lh as f32 * scale).round() as u32,
    );
    let layer = if s.blur > 0. {
        let mut layer = coverage(lw, lh, (pad, pad, sw, sh), sr, true)?;
        blur(&mut layer, lw as usize, lh as usize, s.blur);
        layer
    } else {
        coverage(
            dw,
            dh,
            (pad * scale, pad * scale, sw * scale, sh * scale),
            sr * scale,
            true,
        )?
    };
    // Scaled to device pixels with bilinear filtering.
    let sample = |x: f32, y: f32| {
        let (x, y) = (x.clamp(0., lw as f32 - 1.), y.clamp(0., lh as f32 - 1.));
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(lw - 1), (y0 + 1).min(lh - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let v = |x: u32, y: u32| layer[(y * lw + x) as usize] as f32;
        let top = v(x0, y0) * (1. - fx) + v(x1, y0) * fx;
        let bottom = v(x0, y1) * (1. - fx) + v(x1, y1) * fx;
        top * (1. - fy) + bottom * fy
    };
    // CSS never paints an outer shadow under the box; WebKit's clip-out is not anti-aliased.
    let (ox, oy) = (s.x - s.spread - pad, s.y - s.spread - pad);
    let hole = coverage(
        dw,
        dh,
        (-ox * scale, -oy * scale, box_w * scale, box_h * scale),
        radius * scale,
        false,
    )?;
    let [r, g, b, a] = s.color.to_rgba8();
    let mut bgra = Vec::with_capacity((dw * dh * 4) as usize);
    for y in 0..dh {
        for x in 0..dw {
            let v = if s.blur > 0. {
                sample(
                    (x as f32 + 0.5) / scale - 0.5,
                    (y as f32 + 0.5) / scale - 0.5,
                )
            } else {
                layer[(y * dw + x) as usize] as f32
            };
            let keep = 1. - hole[(y * dw + x) as usize] as f32 / 255.;
            let alpha = (v / 255. * a as f32 * keep).round() as u8;
            bgra.extend_from_slice(&[b, g, r, alpha]);
        }
    }
    let buffer = image::RgbaImage::from_raw(dw, dh, bgra)?;
    let image = Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(buffer),
        1,
    )));
    Some((image, ox, oy))
}

type Key = (u32, u32, u32, [u32; 4], [u8; 4], u32);

/// Rendered shadows, bounded by bytes: shadows follow box sizes, which animate.
#[derive(Default)]
struct Cache {
    images: HashMap<Key, Option<(Arc<RenderImage>, f32, f32)>>,
    bytes: usize,
}

impl Global for Cache {}

const CACHE_BYTES: usize = 64 << 20;

fn paint(bounds: Bounds<Pixels>, s: CssShadow, radius: f32, window: &mut Window, cx: &mut App) {
    let scale = window.scale_factor();
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let key: Key = (
        w.to_bits(),
        h.to_bits(),
        radius.to_bits(),
        [
            s.x.to_bits(),
            s.y.to_bits(),
            s.blur.to_bits(),
            s.spread.to_bits(),
        ],
        s.color.to_rgba8(),
        scale.to_bits(),
    );
    let cache = cx.default_global::<Cache>();
    let hit = match cache.images.get(&key) {
        Some(hit) => hit.clone(),
        None => {
            let rendered = render(w, h, radius, s, scale);
            let bytes = rendered.as_ref().map_or(0, |(img, ..)| {
                let sz = img.size(0);
                (sz.width.0 * sz.height.0 * 4) as usize
            });
            if cache.bytes + bytes > CACHE_BYTES {
                cache.images.clear();
                cache.bytes = 0;
            }
            cache.bytes += bytes;
            cache.images.insert(key, rendered.clone());
            rendered
        }
    };
    let Some((image, ox, oy)) = hit else { return };
    let sz = image.size(0);
    let at = Bounds {
        origin: point(bounds.origin.x + px(ox), bounds.origin.y + px(oy)),
        size: size(
            px(sz.width.0 as f32 / scale),
            px(sz.height.0 as f32 / scale),
        ),
    };
    let _ = window.paint_image(at, at, Corners::default(), image, 0, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lobes_follow_shadow_blur() {
        // σ = 15: three 25-wide boxes; σ = 30: even 50, offset left/right/centered.
        assert_eq!(lobes(30.), [(12, 12); 3]);
        assert_eq!(lobes(60.), [(25, 24), (24, 25), (25, 25)]);
        assert_eq!(lobes(1.), [(1, 0), (0, 1), (1, 1)]);
    }

    #[test]
    fn box_pass_averages_with_extended_edges() {
        let mut d = vec![0, 0, 255, 0, 0];
        box_pass(&mut d, 0, 1, 5, (1, 1));
        assert_eq!(d, vec![0, 85, 85, 85, 0]);
        let mut d = vec![255, 0, 0];
        box_pass(&mut d, 0, 1, 3, (1, 1));
        assert_eq!(d, vec![170, 85, 0]);
    }

    #[test]
    fn shadow_skips_the_box_and_falls_off() {
        let (img, ox, oy) = render(
            40.,
            20.,
            4.,
            css_box_shadow(
                0.,
                6.,
                12.,
                0.,
                Color {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.5,
                },
            ),
            2.,
        )
        .unwrap();
        assert_eq!((ox, oy), (-13., -7.));
        let frame = &img.as_bytes(0).unwrap();
        let sz = img.size(0);
        let alpha = |x: u32, y: u32| frame[((y * sz.width.0 as u32 + x) * 4 + 3) as usize];
        // Under the box center: nothing. Just below the box: the shadow's strongest band.
        let (cx, cy) = ((13. + 20.) as u32 * 2, (7. + 10.) as u32 * 2);
        assert_eq!(alpha(cx, cy), 0);
        let below = alpha(cx, (7 + 20 + 2) * 2);
        let far = alpha(cx, (7 + 20 + 12) * 2);
        assert!(below > far && below > 40, "{below} {far}");
    }
}
