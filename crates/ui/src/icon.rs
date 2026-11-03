//! SVG icons drawn the way the browser draws inline `<svg>`: rasterized once per device-pixel
//! size and color, straight into the target pixels, with CoreGraphics on macOS (WebKit's SVG
//! painter) and tiny-skia elsewhere. GPUI's own `svg()` supersamples and downsamples on the GPU, which
//! softens edges.
//!
//! App icons, Lucide icons and provider marks all render here.

use crate::assets::{APP_ICONS, LUCIDE_ICONS, PROVIDER_ICONS, lucide_entry, svg_bytes};
use crate::color::Color;
use crate::icon_name::IconName;
use gpui_kit::{
    AbsoluteLength, App, Bounds, Corners, DefiniteLength, Element, ElementId, Global,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, Length, Pixels, Refineable,
    RenderImage, Style, StyleRefinement, Styled, Window, px,
};
use resvg::{tiny_skia, usvg};
use smallvec::SmallVec;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Clone)]
enum Source {
    Asset(String),
    Markup(Arc<str>),
    /// Pixels drawn by a custom rasterizer: (cache key, draw(w, h, scale) → premultiplied RGBA).
    Raster(u64, RasterFn),
}

type RasterFn = Arc<dyn Fn(u32, u32, f32) -> Option<Vec<u8>>>;

/// An SVG icon element. Size it with `Styled` (`.size(px(14.))`); color follows the inherited
/// text color unless set.
pub struct Icon {
    source: Source,
    stroke_width: Option<f32>,
    color: Option<Hsla>,
    rotation: f32,
    polychrome: bool,
    mask: Option<crate::text::AlphaMask>,
    style: StyleRefinement,
}

/// An app icon with its registry stroke width.
pub fn icon(name: IconName) -> Icon {
    Icon::new(Source::Asset(format!("{APP_ICONS}/{}.svg", name.key())))
}

/// Any Lucide name or alias, stroke 2. Unknown names draw nothing.
pub fn lucide_icon(name: &str) -> Icon {
    let path =
        lucide_entry(name).map_or(String::new(), |e| format!("{LUCIDE_ICONS}/{}.svg", e.name));
    Icon::new(Source::Asset(path))
}

/// The brand mark for a provider slug, filled with the current color.
pub fn provider_icon(provider: &str) -> Icon {
    let slug = match provider.trim().to_lowercase().as_str() {
        "cursor" => "cursor",
        "claude-code" | "anthropic" | "claude" => "claude",
        "codex" | "openai" | "chatgpt" => "codex",
        "ollama" => "ollama",
        _ => "generic",
    };
    Icon::new(Source::Asset(format!("{PROVIDER_ICONS}/{slug}.svg")))
}

/// Pack-authored SVG markup (`Icon svg=…`). Keeps its own colors; `currentColor` takes the
/// text color.
pub fn svg_markup(markup: impl Into<Arc<str>>) -> Icon {
    let mut i = Icon::new(Source::Markup(markup.into()));
    i.polychrome = true;
    i
}

/// Pixels from a custom rasterizer (`draw(w, h, scale)` → premultiplied RGBA at device size),
/// kept with their own colors. `key` identifies the drawing for the raster cache.
pub fn raster_icon(key: u64, draw: impl Fn(u32, u32, f32) -> Option<Vec<u8>> + 'static) -> Icon {
    let mut i = Icon::new(Source::Raster(key, Arc::new(draw)));
    i.polychrome = true;
    i
}

impl Icon {
    fn new(source: Source) -> Icon {
        let style = StyleRefinement {
            flex_shrink: Some(0.),
            ..Default::default()
        };
        Icon {
            source,
            stroke_width: None,
            color: None,
            rotation: 0.,
            polychrome: false,
            mask: None,
            style,
        }
        .size(px(16.))
    }

    /// Override the registry stroke width (`<Icon strokeWidth={2.4}>`).
    pub fn stroke_width(mut self, w: f32) -> Self {
        self.stroke_width = Some(w);
        self
    }

    pub fn color(mut self, c: impl Into<Hsla>) -> Self {
        self.color = Some(c.into());
        self
    }

    /// Fade by a horizontal mask (see [`crate::text::AlphaMask`]), sampled at the center.
    pub fn mask(mut self, mask: crate::text::AlphaMask) -> Self {
        self.mask = Some(mask);
        self
    }

    /// CSS `transform: rotate(deg)` about the center.
    pub fn rotate(mut self, degrees: f32) -> Self {
        self.rotation = degrees;
        self
    }
}

impl Styled for Icon {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for Icon {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    source: u64,
    stroke_width: u32,
    w: u32,
    h: u32,
    content: (u32, u32),
    color: [u8; 4],
    rotation: i32,
    polychrome: bool,
}

#[derive(Default)]
struct Cache(HashMap<Key, Option<Arc<RenderImage>>>);

impl Global for Cache {}

fn source_hash(s: &Source) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match s {
        Source::Asset(p) => p.hash(&mut h),
        Source::Markup(m) => m.hash(&mut h),
        Source::Raster(key, _) => key.hash(&mut h),
    }
    h.finish()
}

fn rgba8(c: Hsla) -> [u8; 4] {
    let r = gpui_kit::Rgba::from(c);
    Color {
        r: r.r,
        g: r.g,
        b: r.b,
        a: r.a,
    }
    .to_rgba8()
}

impl Element for Icon {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let scale = window.scale_factor();
        let w = (f32::from(bounds.size.width) * scale).round() as u32;
        let h = (f32::from(bounds.size.height) * scale).round() as u32;
        if w == 0 || h == 0 {
            return;
        }
        // GPUI rounds an authored size to device pixels; WebKit snaps the SVG's origin to a
        // device pixel but scales it to the exact size. Drawn that way, with a device pixel of
        // margin for the overhang.
        let exact = |len: Option<Length>, rounded: u32| match len {
            Some(Length::Definite(DefiniteLength::Absolute(AbsoluteLength::Pixels(p)))) => {
                f32::from(p) * scale
            }
            _ => rounded as f32,
        };
        let content = if matches!(self.source, Source::Raster(..)) {
            (w as f32, h as f32)
        } else {
            (
                exact(self.style.size.width, w),
                exact(self.style.size.height, h),
            )
        };
        let margin = u32::from(content != (w as f32, h as f32));
        let (w, h) = (w + 2 * margin, h + 2 * margin);
        let bounds = bounds.dilate(px(margin as f32 / scale));
        let color = self.color.unwrap_or_else(|| window.text_style().color);
        let key = Key {
            source: source_hash(&self.source),
            stroke_width: self.stroke_width.map_or(0, f32::to_bits),
            w,
            h,
            content: (content.0.to_bits(), content.1.to_bits()),
            color: rgba8(color),
            rotation: self.rotation.round() as i32,
            polychrome: self.polychrome,
        };
        let cache = cx.default_global::<Cache>();
        let image = match cache.0.get(&key) {
            Some(hit) => hit.clone(),
            None => {
                let rendered = self.render(w, h, content, key.color, scale);
                cache.0.insert(key, rendered.clone());
                rendered
            }
        };
        let Some(mut image) = image else { return };
        // A CSS `mask-image` gradient varies across the icon: scale each column's alpha.
        if let Some(mask) = &self.mask
            && let Some(bytes) = image.as_bytes(0)
        {
            let mut bgra = bytes.to_vec();
            let left = f32::from(bounds.left());
            for (i, p) in bgra.chunks_exact_mut(4).enumerate() {
                let x = left + ((i as u32 % w) as f32 + 0.5) / scale;
                p[3] = (p[3] as f32 * mask(x)).round() as u8;
            }
            if let Some(buffer) = image::RgbaImage::from_raw(w, h, bgra) {
                image = Arc::new(RenderImage::new(SmallVec::from_elem(
                    image::Frame::new(buffer),
                    1,
                )));
            }
        }
        let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
    }
}

impl Icon {
    fn svg_text(&self, color: [u8; 4]) -> Option<String> {
        let raw = match &self.source {
            Source::Asset(p) => String::from_utf8(svg_bytes(p)?.to_vec()).ok()?,
            Source::Markup(m) => m.to_string(),
            Source::Raster(..) => return None,
        };
        let mut svg = raw;
        if let Some(sw) = self.stroke_width
            && let Some(start) = svg.find("stroke-width=\"")
        {
            let from = start + "stroke-width=\"".len();
            if let Some(len) = svg[from..].find('"') {
                svg.replace_range(from..from + len, &sw.to_string());
            }
        }
        if self.polychrome {
            let c = Color {
                r: color[0] as f32 / 255.,
                g: color[1] as f32 / 255.,
                b: color[2] as f32 / 255.,
                a: color[3] as f32 / 255.,
            };
            svg = svg.replace("currentColor", &c.to_css());
        }
        Some(svg)
    }

    fn render(
        &self,
        w: u32,
        h: u32,
        content: (f32, f32),
        color: [u8; 4],
        scale: f32,
    ) -> Option<Arc<RenderImage>> {
        let pixels = match &self.source {
            Source::Raster(_, draw) => draw(w, h, scale)?,
            _ => {
                // The margin: zero, or one device pixel around an exact-size icon.
                let at = (
                    ((w as f32 - content.0) / 2.).round(),
                    ((h as f32 - content.1) / 2.).round(),
                );
                rasterize(&self.svg_text(color)?, (w, h), at, content, self.rotation)?
            }
        };
        let mut bgra = pixels;
        if self.polychrome {
            unpremultiply_bgra(&mut bgra);
        } else {
            // Monochrome: coverage is the alpha, the color is exact.
            for p in bgra.chunks_exact_mut(4) {
                let cov = p[3] as u32;
                p[0] = color[2];
                p[1] = color[1];
                p[2] = color[0];
                p[3] = ((cov * color[3] as u32 + 127) / 255) as u8;
            }
        }
        let buffer = image::RgbaImage::from_raw(w, h, bgra)?;
        Some(Arc::new(RenderImage::new(SmallVec::from_elem(
            image::Frame::new(buffer),
            1,
        ))))
    }
}

/// Premultiplied RGBA from a rasterizer → the straight BGRA `RenderImage` holds.
fn unpremultiply_bgra(bgra: &mut [u8]) {
    for p in bgra.chunks_exact_mut(4) {
        let a = p[3];
        let un = |c: u8| {
            if a == 0 {
                0
            } else {
                ((c as f32 * 255. / a as f32).round()).min(255.) as u8
            }
        };
        let (r, g, b) = (un(p[0]), un(p[1]), un(p[2]));
        p[0] = b;
        p[1] = g;
        p[2] = r;
    }
}

/// Paint SVG `markup` whose viewBox is `size` CSS px with its top-left at the exact window
/// position `origin`, as WebKit draws a shape at a fractional position (a list marker's
/// ellipse): rasterized into the device pixels it touches, at its sub-pixel offset in them.
pub fn paint_svg_at(
    markup: &str,
    origin: gpui_kit::Point<f32>,
    size: (f32, f32),
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let (x0, y0) = ((origin.x * scale).floor(), (origin.y * scale).floor());
    let w = ((origin.x + size.0) * scale).ceil() - x0;
    let h = ((origin.y + size.1) * scale).ceil() - y0;
    if w < 1. || h < 1. {
        return;
    }
    let at = (origin.x * scale - x0, origin.y * scale - y0);
    let (w, h) = (w as u32, h as u32);
    let key = Key {
        source: source_hash(&Source::Markup(markup.into())),
        stroke_width: 0,
        w,
        h,
        content: (at.0.to_bits(), at.1.to_bits()),
        color: [0; 4],
        rotation: 0,
        polychrome: true,
    };
    let content = (size.0 * scale, size.1 * scale);
    let cache = cx.default_global::<Cache>();
    let image = cache
        .0
        .entry(key)
        .or_insert_with(|| {
            let mut bgra = rasterize(markup, (w, h), at, content, 0.)?;
            unpremultiply_bgra(&mut bgra);
            let buffer = image::RgbaImage::from_raw(w, h, bgra)?;
            Some(Arc::new(RenderImage::new(SmallVec::from_elem(
                image::Frame::new(buffer),
                1,
            ))))
        })
        .clone();
    let Some(image) = image else { return };
    let bounds = Bounds::new(
        gpui_kit::point(px(x0 / scale), px(y0 / scale)),
        gpui_kit::size(px(w as f32 / scale), px(h as f32 / scale)),
    );
    let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
}

/// Rasterize SVG markup into premultiplied RGBA on a `canvas` of device pixels, scaling the
/// viewBox to fit a `content` box at `at` (device pixels) and centering it there
/// (`preserveAspectRatio="xMidYMid meet"`).
pub fn rasterize(
    svg: &str,
    canvas: (u32, u32),
    at: (f32, f32),
    content: (f32, f32),
    rotation_deg: f32,
) -> Option<Vec<u8>> {
    let (w, h) = canvas;
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let s = (content.0 / size.width()).min(content.1 / size.height());
    let dx = at.0 + (content.0 - size.width() * s) / 2.;
    let dy = at.1 + (content.1 - size.height() * s) / 2.;
    let mut ts = tiny_skia::Transform::from_row(s, 0., 0., s, dx, dy);
    if rotation_deg != 0. {
        ts = ts.post_concat(tiny_skia::Transform::from_rotate_at(
            rotation_deg,
            at.0 + content.0 / 2.,
            at.1 + content.1 / 2.,
        ));
    }
    #[cfg(target_os = "macos")]
    {
        Some(crate::cg_raster::render(&tree, ts, w, h))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
        resvg::render(&tree, ts, &mut pixmap.as_mut());
        Some(pixmap.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_chevron_with_exact_coverage() {
        let svg = std::str::from_utf8(svg_bytes(&format!("{APP_ICONS}/chevron-down.svg")).unwrap())
            .unwrap();
        let px = rasterize(svg, (28, 28), (0., 0.), (28., 28.), 0.).unwrap();
        let covered = px.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(covered > 40 && covered < 300, "{covered}");
        // Rotating 180° flips the chevron to point up: the top row empties, the bottom fills.
        let up = rasterize(svg, (28, 28), (0., 0.), (28., 28.), 180.).unwrap();
        assert_ne!(px, up);
    }

    #[test]
    fn stroke_override_rewrites_the_root_width() {
        let i = icon(IconName::ChevronDown).stroke_width(2.2);
        let svg = i.svg_text([0, 0, 0, 255]).unwrap();
        assert!(svg.contains("stroke-width=\"2.2\""));
    }
}
