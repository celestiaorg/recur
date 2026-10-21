//! usvg trees through CoreGraphics: paths with solid and pad-gradient paints, group
//! opacity (an offscreen layer), clip paths and transforms, embedded raster images and nested
//! SVG. Anything else takes resvg's pipeline (see `needs_full_pipeline`).

use core_graphics::base::{CGFloat, kCGImageAlphaPremultipliedLast};
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::{CGContext, CGLineCap, CGLineJoin};
use core_graphics::geometry::{CGAffineTransform, CGPoint, CGRect, CGSize};
use core_graphics::gradient::{CGGradient, CGGradientDrawingOptions};
use resvg::tiny_skia::{PathSegment, Transform};
use resvg::usvg;

fn context(w: u32, h: u32) -> CGContext {
    let space = CGColorSpace::create_device_rgb();
    let cx = CGContext::create_bitmap_context(
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
    cx
}

fn pixels(cx: &mut CGContext, w: u32, h: u32) -> Vec<u8> {
    let stride = cx.bytes_per_row();
    let data = cx.data();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        out.extend_from_slice(&data[row * stride..row * stride + w as usize * 4]);
    }
    out
}

pub(super) fn render(tree: &usvg::Tree, ts: Transform, w: u32, h: u32) -> Vec<u8> {
    let mut cx = context(w, h);
    group(&cx, tree.root(), ts, w, h);
    pixels(&mut cx, w, h)
}

fn affine(m: Transform) -> CGAffineTransform {
    CGAffineTransform {
        a: m.sx as f64,
        b: m.ky as f64,
        c: m.kx as f64,
        d: m.sy as f64,
        tx: m.tx as f64,
        ty: m.ty as f64,
    }
}

fn trace(cx: &CGContext, data: &resvg::tiny_skia::Path) {
    cx.begin_path();
    for seg in data.segments() {
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

fn add_transformed(cx: &CGContext, data: &resvg::tiny_skia::Path, m: Transform) {
    let Some(d) = data.clone().transform(m) else {
        return;
    };
    for seg in d.segments() {
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

/// Clip to the union of a clip path's shapes (in device space, so the CTM stays put).
fn clip_to(cx: &CGContext, clip: &usvg::ClipPath, ts: Transform) {
    fn add(cx: &CGContext, g: &usvg::Group, ts: Transform, any: &mut bool) {
        for n in g.children() {
            match n {
                usvg::Node::Group(c) => add(cx, c, ts.pre_concat(c.transform()), any),
                usvg::Node::Path(p) => {
                    add_transformed(cx, p.data(), ts);
                    *any = true;
                }
                usvg::Node::Text(t) => add(cx, t.flattened(), ts, any),
                usvg::Node::Image(_) => {}
            }
        }
    }
    cx.begin_path();
    let mut any = false;
    add(cx, clip.root(), ts.pre_concat(clip.transform()), &mut any);
    if !any {
        // An empty clip hides everything.
        cx.move_to_point(0., 0.);
        cx.add_line_to_point(0., 0.);
        cx.close_path();
    }
    cx.clip();
}

fn group(cx: &CGContext, g: &usvg::Group, ts: Transform, w: u32, h: u32) {
    let ts = ts.pre_concat(g.transform());
    let opacity = g.opacity().get();
    if opacity < 1. && g.should_isolate() {
        // Group opacity composites the whole subtree once.
        let layer = context(w, h);
        if let Some(c) = g.clip_path() {
            clip_to(&layer, c, ts);
        }
        children(&layer, g, ts, w, h);
        if let Some(img) = layer.create_image() {
            cx.save();
            // The layer is already in device space; undo the y flip for drawing it.
            cx.translate(0., h as f64);
            cx.scale(1., -1.);
            cx.set_alpha(opacity as CGFloat);
            cx.draw_image(
                CGRect::new(&CGPoint::new(0., 0.), &CGSize::new(w as f64, h as f64)),
                &img,
            );
            cx.restore();
        }
        return;
    }
    cx.save();
    if let Some(c) = g.clip_path() {
        clip_to(cx, c, ts);
    }
    children(cx, g, ts, w, h);
    cx.restore();
}

fn children(cx: &CGContext, g: &usvg::Group, ts: Transform, w: u32, h: u32) {
    for n in g.children() {
        match n {
            usvg::Node::Group(c) => group(cx, c, ts, w, h),
            usvg::Node::Path(p) if p.is_visible() => path(cx, p, ts),
            usvg::Node::Text(t) => group(cx, t.flattened(), ts, w, h),
            usvg::Node::Image(i) if i.is_visible() => image(cx, i, ts, w, h),
            _ => {}
        }
    }
}

fn image(cx: &CGContext, i: &usvg::Image, ts: Transform, w: u32, h: u32) {
    match i.kind() {
        usvg::ImageKind::SVG(tree) => group(cx, tree.root(), ts, w, h),
        _ => {
            // Raster images draw through resvg's decoder into a device-size layer.
            let mut pixmap = match resvg::tiny_skia::Pixmap::new(w, h) {
                Some(p) => p,
                None => return,
            };
            resvg::render_node(&usvg::Node::Image(Box::new(i.clone())), ts, &mut pixmap.as_mut());
            let data = pixmap.take();
            let space = CGColorSpace::create_device_rgb();
            let mut layer = CGContext::create_bitmap_context(
                None,
                w as usize,
                h as usize,
                8,
                w as usize * 4,
                &space,
                kCGImageAlphaPremultipliedLast,
            );
            let stride = layer.bytes_per_row();
            {
                let dst = layer.data();
                for row in 0..h as usize {
                    dst[row * stride..row * stride + w as usize * 4]
                        .copy_from_slice(&data[row * w as usize * 4..(row + 1) * w as usize * 4]);
                }
            }
            if let Some(img) = layer.create_image() {
                cx.save();
                cx.translate(0., h as f64);
                cx.scale(1., -1.);
                cx.draw_image(
                    CGRect::new(&CGPoint::new(0., 0.), &CGSize::new(w as f64, h as f64)),
                    &img,
                );
                cx.restore();
            }
        }
    }
}

fn gradient_stops(stops: &[usvg::Stop], opacity: f32) -> (Vec<CGFloat>, Vec<CGFloat>) {
    let mut comps = Vec::new();
    let mut locs = Vec::new();
    for s in stops {
        let c = s.color();
        comps.extend([
            c.red as f64 / 255.,
            c.green as f64 / 255.,
            c.blue as f64 / 255.,
            (s.opacity().get() * opacity) as f64,
        ]);
        locs.push(s.offset().get() as f64);
    }
    (comps, locs)
}

/// Fill (or stroke, already traced as the current path) with `paint`.
fn paint(cx: &CGContext, p: &usvg::Paint, opacity: f32, m: Transform, even_odd: bool, stroke: Option<&usvg::Stroke>) {
    match p {
        usvg::Paint::Color(c) => {
            let (r, g, b) = (c.red as f64 / 255., c.green as f64 / 255., c.blue as f64 / 255.);
            match stroke {
                Some(_) => {
                    cx.set_rgb_stroke_color(r, g, b, opacity as f64);
                    cx.stroke_path();
                }
                None => {
                    cx.set_rgb_fill_color(r, g, b, opacity as f64);
                    if even_odd {
                        cx.eo_fill_path();
                    } else {
                        cx.fill_path();
                    }
                }
            }
        }
        usvg::Paint::LinearGradient(_) | usvg::Paint::RadialGradient(_) => {
            cx.save();
            if stroke.is_some() {
                cx.replace_path_with_stroked_path();
                cx.clip();
            } else if even_odd {
                cx.eo_clip();
            } else {
                cx.clip();
            }
            let _ = m;
            let space = CGColorSpace::create_device_rgb();
            let opts = CGGradientDrawingOptions::CGGradientDrawsBeforeStartLocation
                | CGGradientDrawingOptions::CGGradientDrawsAfterEndLocation;
            match p {
                usvg::Paint::LinearGradient(l) => {
                    let (comps, locs) = gradient_stops(l.stops(), opacity);
                    let grad = CGGradient::create_with_color_components(&space, &comps, &locs, locs.len());
                    cx.concat_ctm(affine(l.transform()));
                    cx.draw_linear_gradient(
                        &grad,
                        CGPoint::new(l.x1() as f64, l.y1() as f64),
                        CGPoint::new(l.x2() as f64, l.y2() as f64),
                        opts,
                    );
                }
                usvg::Paint::RadialGradient(r) => {
                    let (comps, locs) = gradient_stops(r.stops(), opacity);
                    let grad = CGGradient::create_with_color_components(&space, &comps, &locs, locs.len());
                    cx.concat_ctm(affine(r.transform()));
                    cx.draw_radial_gradient(
                        &grad,
                        CGPoint::new(r.fx() as f64, r.fy() as f64),
                        0.,
                        CGPoint::new(r.cx() as f64, r.cy() as f64),
                        r.r().get() as f64,
                        opts,
                    );
                }
                _ => {}
            }
            cx.restore();
        }
        usvg::Paint::Pattern(_) => {}
    }
}

fn path(cx: &CGContext, p: &usvg::Path, ts: Transform) {
    let m = ts;
    let fill = |cx: &CGContext| {
        if let Some(f) = p.fill() {
            trace(cx, p.data());
            paint(cx, f.paint(), f.opacity().get(), m, f.rule() == usvg::FillRule::EvenOdd, None);
        }
    };
    let stroke = |cx: &CGContext| {
        if let Some(s) = p.stroke() {
            trace(cx, p.data());
            cx.set_line_width(s.width().get() as f64);
            cx.set_line_cap(match s.linecap() {
                usvg::LineCap::Butt => CGLineCap::CGLineCapButt,
                usvg::LineCap::Round => CGLineCap::CGLineCapRound,
                usvg::LineCap::Square => CGLineCap::CGLineCapSquare,
            });
            cx.set_line_join(match s.linejoin() {
                usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => CGLineJoin::CGLineJoinMiter,
                usvg::LineJoin::Round => CGLineJoin::CGLineJoinRound,
                usvg::LineJoin::Bevel => CGLineJoin::CGLineJoinBevel,
            });
            cx.set_miter_limit(s.miterlimit().get() as f64);
            if let Some(dash) = s.dasharray() {
                let lengths: Vec<f64> = dash.iter().map(|d| *d as f64).collect();
                cx.set_line_dash(s.dashoffset() as f64, &lengths);
            }
            paint(cx, s.paint(), s.opacity().get(), m, false, Some(s));
        }
    };
    cx.save();
    cx.concat_ctm(affine(m));
    match p.paint_order() {
        usvg::PaintOrder::FillAndStroke => {
            fill(cx);
            stroke(cx);
        }
        usvg::PaintOrder::StrokeAndFill => {
            stroke(cx);
            fill(cx);
        }
    }
    cx.restore();
}
