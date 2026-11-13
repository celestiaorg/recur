//! `.wmark`: the Recur wordmark as a `--text` fill through the PNG's alpha
//! (`mask: var(--wmark) center / contain no-repeat` on a 96×24 box).

use std::cell::RefCell;
use std::sync::Arc;
#[cfg(not(target_os = "macos"))]
use std::sync::OnceLock;

use gpui_kit::{
    App, Bounds, Corners, IntoElement, Pixels, RenderImage, Styled, Window, canvas, px,
};
use recur_ui::{Color, Theme};

const PNG: &[u8] = include_bytes!("../assets/recur-wordmark.png");
const BOX: (f32, f32) = (96., 24.);

/// The PNG's alpha, row-major, with its size.
#[cfg(not(target_os = "macos"))]
fn alpha() -> Option<&'static (u32, u32, Vec<u8>)> {
    static A: OnceLock<Option<(u32, u32, Vec<u8>)>> = OnceLock::new();
    A.get_or_init(|| {
        let img = image::load_from_memory_with_format(PNG, image::ImageFormat::Png).ok()?;
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Some((w, h, rgba.pixels().map(|p| p.0[3]).collect()))
    })
    .as_ref()
}

/// Where the image lands in a `scale`d box: `contain`, centered (device px).
fn placement(sw: u32, sh: u32, scale: f32) -> (u32, u32, f32, f32, f32, f32) {
    let (bw, bh) = (BOX.0 * scale, BOX.1 * scale);
    let k = (bw / sw as f32).min(bh / sh as f32);
    let (iw, ih) = (sw as f32 * k, sh as f32 * k);
    (
        bw.round() as u32,
        bh.round() as u32,
        (bw - iw) / 2.,
        (bh - ih) / 2.,
        iw,
        ih,
    )
}

/// The mask's coverage of each device pixel of the box. WebKit draws the mask image through
/// CoreGraphics at its default interpolation, so on macOS the same call resamples it.
#[cfg(target_os = "macos")]
pub fn coverage(scale: f32) -> Option<(u32, u32, Vec<f32>)> {
    use core_graphics::base::kCGImageAlphaPremultipliedLast;
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::{CGContext, CGInterpolationQuality};
    use core_graphics::data_provider::CGDataProvider;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::image::CGImage;

    let img = image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    let (sw, sh) = img.dimensions();
    let mut pm = img.into_raw();
    for p in pm.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
    let space = CGColorSpace::create_device_rgb();
    let src = CGImage::new(
        sw as usize,
        sh as usize,
        8,
        32,
        sw as usize * 4,
        &space,
        kCGImageAlphaPremultipliedLast,
        &CGDataProvider::from_buffer(std::sync::Arc::new(pm)),
        true,
        0,
    );
    let (w, h, ox, oy, iw, ih) = placement(sw, sh, scale);
    let mut cx = CGContext::create_bitmap_context(
        None,
        w as usize,
        h as usize,
        8,
        w as usize * 4,
        &space,
        kCGImageAlphaPremultipliedLast,
    );
    cx.set_interpolation_quality(CGInterpolationQuality::CGInterpolationQualityDefault);
    // CoreGraphics is y-up.
    cx.draw_image(
        CGRect::new(
            &CGPoint::new(ox as f64, (h as f32 - oy - ih) as f64),
            &CGSize::new(iw as f64, ih as f64),
        ),
        &src,
    );
    let stride = cx.bytes_per_row();
    let data = cx.data();
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            out.push(data[y * stride + x * 4 + 3] as f32 / 255.);
        }
    }
    Some((w, h, out))
}

/// The mask's coverage of each device pixel of the box: each pixel the average of the source
/// area it covers.
#[cfg(not(target_os = "macos"))]
pub fn coverage(scale: f32) -> Option<(u32, u32, Vec<f32>)> {
    let (sw, sh, a) = alpha()?;
    let (w, h, ox, oy, iw, _) = placement(*sw, *sh, scale);
    let k = iw / *sw as f32;
    let src = |x: u32, y: u32| a[(y * sw + x) as usize] as f32 / 255.;
    let mut out = Vec::with_capacity((w * h) as usize);
    for dy in 0..h {
        let (y0, y1) = ((dy as f32 - oy) / k, (dy as f32 + 1. - oy) / k);
        for dx in 0..w {
            let (x0, x1) = ((dx as f32 - ox) / k, (dx as f32 + 1. - ox) / k);
            let mut sum = 0.;
            let (ys, ye) = (y0.max(0.), y1.min(*sh as f32));
            let (xs, xe) = (x0.max(0.), x1.min(*sw as f32));
            if ye > ys && xe > xs {
                let mut y = ys;
                while y < ye {
                    let ny = (y.floor() + 1.).min(ye);
                    let mut x = xs;
                    while x < xe {
                        let nx = (x.floor() + 1.).min(xe);
                        sum += src(x as u32, y as u32) * (nx - x) * (ny - y);
                        x = nx;
                    }
                    y = ny;
                }
            }
            out.push(sum / ((x1 - x0) * (y1 - y0)));
        }
    }
    Some((w, h, out))
}

/// The last raster, keyed by scale and color.
type Cached = Option<((u32, [u8; 4]), Arc<RenderImage>)>;

thread_local! {
    static CACHE: RefCell<Cached> = const { RefCell::new(None) };
}

fn raster(scale: f32, color: Color) -> Option<Arc<RenderImage>> {
    let rgba = color.to_rgba8();
    let key = (scale.to_bits(), rgba);
    if let Some(hit) = CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, i)| i.clone())
    }) {
        return Some(hit);
    }
    let (w, h, cov) = coverage(scale)?;
    let mut bgra = Vec::with_capacity(cov.len() * 4);
    for c in cov {
        // Straight alpha, as GPUI's image sprites take it.
        let a = c * rgba[3] as f32 / 255.;
        bgra.extend_from_slice(&[rgba[2], rgba[1], rgba[0], (a * 255.).round() as u8]);
    }
    let frame = image::Frame::new(image::RgbaImage::from_raw(w, h, bgra)?);
    let img = Arc::new(RenderImage::new(smallvec::SmallVec::from_elem(frame, 1)));
    CACHE.with(|c| *c.borrow_mut() = Some((key, img.clone())));
    Some(img)
}

/// The 96×24 wordmark in `--text`.
pub fn wordmark(cx: &App) -> impl IntoElement {
    let color = Theme::of(cx).tokens.text;
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
            if let Some(img) = raster(window.scale_factor(), color) {
                let _ = window.paint_image(bounds, bounds, Corners::default(), img, 0, false);
            }
        },
    )
    .flex_none()
    .w(px(BOX.0))
    .h(px(BOX.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_the_box_and_centers_vertically() {
        let (w, h, cov) = coverage(2.).expect("png");
        assert_eq!((w, h), (192, 48));
        // 720×172 scaled to 96px wide is 22.93px tall: the top and bottom device rows are
        // empty, the middle carries ink.
        let row = |y: u32| {
            cov[(y * w) as usize..((y + 1) * w) as usize]
                .iter()
                .sum::<f32>()
        };
        assert_eq!(row(0), 0.);
        assert_eq!(row(47), 0.);
        assert!(row(24) > 0.);
        assert!(cov.iter().all(|c| (0. ..=1.0001).contains(c)));
        assert!(cov.iter().sum::<f32>() > 100.);
    }
}
