//! SVG rasterization through CoreGraphics, the painter WebKit uses for inline SVG: the same
//! paths, strokes and anti-aliasing as a browser on macOS.

use core_graphics::base::kCGImageAlphaPremultipliedLast;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::{CGContext, CGLineCap, CGLineJoin};
use core_graphics::geometry::CGAffineTransform;
use resvg::tiny_skia::{PathSegment, Transform};
use resvg::usvg;

/// Draw `tree` into a `w`×`h` premultiplied RGBA bitmap under `ts` (user space → device).
pub(crate) fn render(tree: &usvg::Tree, ts: Transform, w: u32, h: u32) -> Vec<u8> {
    let space = CGColorSpace::create_device_rgb();
    let mut cx = CGContext::create_bitmap_context(
        None,
        w as usize,
        h as usize,
        8,
        w as usize * 4,
        &space,
        kCGImageAlphaPremultipliedLast,
    );
    // CoreGraphics is y-up; flip to SVG's y-down device space.
    cx.translate(0., h as f64);
    cx.scale(1., -1.);
    group(&cx, tree.root(), ts, 1.);
    let stride = cx.bytes_per_row();
    let data = cx.data();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        out.extend_from_slice(&data[row * stride..row * stride + w as usize * 4]);
    }
    out
}

fn group(cx: &CGContext, g: &usvg::Group, ts: Transform, opacity: f32) {
    let opacity = opacity * g.opacity().get();
    for node in g.children() {
        match node {
            usvg::Node::Group(child) => group(cx, child, ts, opacity),
            usvg::Node::Path(p) if p.is_visible() => path(cx, p, ts, opacity),
            _ => {}
        }
    }
}

fn color(paint: &usvg::Paint) -> Option<(f64, f64, f64)> {
    match paint {
        usvg::Paint::Color(c) => Some((
            c.red as f64 / 255.,
            c.green as f64 / 255.,
            c.blue as f64 / 255.,
        )),
        _ => None,
    }
}

fn trace(cx: &CGContext, p: &usvg::Path) {
    cx.begin_path();
    for seg in p.data().segments() {
        match seg {
            PathSegment::MoveTo(a) => cx.move_to_point(a.x as f64, a.y as f64),
            PathSegment::LineTo(a) => cx.add_line_to_point(a.x as f64, a.y as f64),
            PathSegment::QuadTo(a, b) => {
                cx.add_quad_curve_to_point(a.x as f64, a.y as f64, b.x as f64, b.y as f64)
            }
            PathSegment::CubicTo(a, b, c) => cx.add_curve_to_point(
                a.x as f64, a.y as f64, b.x as f64, b.y as f64, c.x as f64, c.y as f64,
            ),
            PathSegment::Close => cx.close_path(),
        }
    }
}

fn path(cx: &CGContext, p: &usvg::Path, ts: Transform, opacity: f32) {
    let m = ts.pre_concat(p.abs_transform());
    cx.save();
    cx.concat_ctm(CGAffineTransform {
        a: m.sx as f64,
        b: m.ky as f64,
        c: m.kx as f64,
        d: m.sy as f64,
        tx: m.tx as f64,
        ty: m.ty as f64,
    });
    if let Some(fill) = p.fill()
        && let Some((r, g, b)) = color(fill.paint())
    {
        trace(cx, p);
        cx.set_rgb_fill_color(r, g, b, (fill.opacity().get() * opacity) as f64);
        match fill.rule() {
            usvg::FillRule::EvenOdd => cx.eo_fill_path(),
            usvg::FillRule::NonZero => cx.fill_path(),
        }
    }
    if let Some(stroke) = p.stroke()
        && let Some((r, g, b)) = color(stroke.paint())
    {
        trace(cx, p);
        cx.set_rgb_stroke_color(r, g, b, (stroke.opacity().get() * opacity) as f64);
        cx.set_line_width(stroke.width().get() as f64);
        cx.set_line_cap(match stroke.linecap() {
            usvg::LineCap::Butt => CGLineCap::CGLineCapButt,
            usvg::LineCap::Round => CGLineCap::CGLineCapRound,
            usvg::LineCap::Square => CGLineCap::CGLineCapSquare,
        });
        cx.set_line_join(match stroke.linejoin() {
            usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => CGLineJoin::CGLineJoinMiter,
            usvg::LineJoin::Round => CGLineJoin::CGLineJoinRound,
            usvg::LineJoin::Bevel => CGLineJoin::CGLineJoinBevel,
        });
        cx.set_miter_limit(stroke.miterlimit().get() as f64);
        if let Some(dash) = stroke.dasharray() {
            let lengths: Vec<f64> = dash.iter().map(|d| *d as f64).collect();
            cx.set_line_dash(stroke.dashoffset() as f64, &lengths);
        }
        cx.stroke_path();
    }
    cx.restore();
}

/// A blurred shadow layer (`lw`×`lh` alpha, CSS px) upscaled to `dw`×`dh` device pixels as
/// WebKit draws `ShadowBlur`'s layer: a black BGRA image used as a clip mask
/// (`clipToImageBuffer`, `CGContextClipToMask`) over the layer's rect at device scale, then
/// filled. Returns the coverage per device pixel.
pub(crate) fn shadow_mask(layer: &[u8], lw: u32, lh: u32, dw: u32, dh: u32, scale: f32) -> Vec<u8> {
    use core_graphics::base::{kCGBitmapByteOrder32Little, kCGImageAlphaPremultipliedFirst};
    use core_graphics::data_provider::CGDataProvider;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::image::CGImage;
    use std::sync::Arc;

    let bgra: Vec<u8> = layer.iter().flat_map(|&a| [0, 0, 0, a]).collect();
    let provider = CGDataProvider::from_buffer(Arc::new(bgra));
    let image = CGImage::new(
        lw as usize,
        lh as usize,
        8,
        32,
        lw as usize * 4,
        &CGColorSpace::create_device_rgb(),
        kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little,
        &provider,
        true,
        0,
    );
    let mut cx = CGContext::create_bitmap_context(
        None,
        dw as usize,
        dh as usize,
        8,
        dw as usize * 4,
        &CGColorSpace::create_device_rgb(),
        kCGImageAlphaPremultipliedLast,
    );
    cx.scale(scale as f64, scale as f64);
    let rect = CGRect::new(
        &CGPoint::new(0., dh as f64 / scale as f64 - lh as f64),
        &CGSize::new(lw as f64, lh as f64),
    );
    cx.clip_to_mask(rect, &image);
    cx.set_rgb_fill_color(0., 0., 0., 1.);
    cx.fill_rect(rect);
    let stride = cx.bytes_per_row();
    let data = cx.data();
    (0..dh as usize)
        .flat_map(|row| (0..dw as usize).map(move |x| (row, x)))
        .map(|(row, x)| data[row * stride + x * 4 + 3])
        .collect()
}
