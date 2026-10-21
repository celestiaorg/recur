//! `system-ui` text on macOS, shaped and drawn through CoreText as WebKit does: the system UI
//! font with the CSS weight on its `wght` axis (GPUI's font lookup cannot reach SF Medium), CTLine
//! shaping with CoreText's own fallback runs, and glyphs drawn with `CTFontDrawGlyphs`.

use core_foundation::attributed_string::CFMutableAttributedString;
use core_foundation::base::{CFRange, CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::base::kCGImageAlphaPremultipliedLast;
use core_graphics::color_space::CGColorSpace;
use core_graphics::context::CGContext;
use core_graphics::geometry::CGPoint;
use core_text::font::{
    CTFont, kCTFontSystemFontType, new_from_descriptor, new_ui_font_for_language,
};
use core_text::line::CTLine;
use std::cell::RefCell;
use std::collections::HashMap;

/// A shaped glyph: CoreText font slot, glyph id, x in px, UTF-8 byte offset.
pub(crate) struct CtGlyph {
    pub font: usize,
    pub glyph: u16,
    pub x: f32,
    pub index: usize,
}

#[derive(Default)]
struct Fonts {
    by_key: HashMap<(u32, u32), usize>,
    fonts: Vec<CTFont>,
    by_name: HashMap<(String, u32), usize>,
}

thread_local! {
    static FONTS: RefCell<Fonts> = RefCell::default();
}

fn system_font(weight: f32, size: f32) -> usize {
    FONTS.with(|f| {
        let mut f = f.borrow_mut();
        let key = (weight.to_bits(), size.to_bits());
        if let Some(&i) = f.by_key.get(&key) {
            return i;
        }
        let base = new_ui_font_for_language(kCTFontSystemFontType, size as f64, None);
        // WebKit sets the CSS weight on SF's `wght` variation axis, and as the CoreText weight
        // trait, which CoreText's fallback cascade follows (a medium ↵ comes from Lucida Grande
        // Bold).
        let wght = CFNumber::from(0x7767_6874i64);
        let variation = CFDictionary::from_CFType_pairs(&[(
            wght.as_CFType(),
            CFNumber::from(weight as f64).as_CFType(),
        )]);
        let traits = CFDictionary::from_CFType_pairs(&[(
            CFString::new("NSCTFontWeightTrait").as_CFType(),
            CFNumber::from(ct_weight(weight)).as_CFType(),
        )]);
        let attrs: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[
            (
                CFString::new("NSCTFontVariationAttribute"),
                variation.as_CFType(),
            ),
            (CFString::new("NSCTFontTraitsAttribute"), traits.as_CFType()),
        ]);
        let font = base
            .copy_descriptor()
            .create_copy_with_attributes(attrs.to_untyped())
            .map(|d| new_from_descriptor(&d, size as f64))
            .unwrap_or(base);
        let i = f.fonts.len();
        f.fonts.push(font);
        f.by_key.insert(key, i);
        i
    })
}

/// A CSS weight as a CoreText weight trait, the steps WebKit maps between.
fn ct_weight(weight: f32) -> f64 {
    match weight {
        w if w < 150. => -0.8,
        w if w < 250. => -0.6,
        w if w < 350. => -0.4,
        w if w < 450. => 0.,
        w if w < 550. => 0.23,
        w if w < 650. => 0.3,
        w if w < 750. => 0.4,
        w if w < 850. => 0.56,
        _ => 0.62,
    }
}

fn intern(font: CTFont, size: f32) -> usize {
    FONTS.with(|f| {
        let mut f = f.borrow_mut();
        let key = (font.postscript_name(), size.to_bits());
        if let Some(&i) = f.by_name.get(&key) {
            return i;
        }
        let i = f.fonts.len();
        f.fonts.push(font);
        f.by_name.insert(key, i);
        i
    })
}

/// Ascent, descent and line gap of the system font at a weight and size.
pub(crate) fn metrics(weight: f32, size: f32) -> (f32, f32, f32) {
    let i = system_font(weight, size);
    FONTS.with(|f| {
        let f = f.borrow();
        let font = &f.fonts[i];
        (
            font.ascent() as f32,
            font.descent() as f32,
            font.leading() as f32,
        )
    })
}

/// Shape `text` in the system font; returns glyphs and the advance width.
pub(crate) fn shape(text: &str, weight: f32, size: f32) -> (Vec<CtGlyph>, f32) {
    let base = system_font(weight, size);
    let font = FONTS.with(|f| f.borrow().fonts[base].clone());
    let mut s = CFMutableAttributedString::new();
    let cf = CFString::new(text);
    s.replace_str(&cf, CFRange::init(0, 0));
    let len = cf.char_len();
    s.set_attribute(
        CFRange::init(0, len),
        CFString::new("NSFont").as_concrete_TypeRef(),
        &font,
    );
    let line = CTLine::new_with_attributed_string(s.as_concrete_TypeRef() as _);
    // UTF-16 index → UTF-8 byte offset.
    let mut utf8_at = Vec::with_capacity(len as usize + 1);
    for (b, ch) in text.char_indices() {
        for _ in 0..ch.len_utf16() {
            utf8_at.push(b);
        }
    }
    utf8_at.push(text.len());
    let mut out = Vec::new();
    for run in line.glyph_runs().iter() {
        let run_font = run
            .attributes()
            .and_then(|a| a.find(CFString::new("NSFont")).map(|v| v.clone()))
            .and_then(|v| v.downcast::<CTFont>())
            .map_or(base, |f| intern(f, size));
        let glyphs = run.glyphs();
        let positions = run.positions();
        let indices = run.string_indices();
        for ((g, p), i) in glyphs.iter().zip(positions.iter()).zip(indices.iter()) {
            out.push(CtGlyph {
                font: run_font,
                glyph: *g,
                x: p.x as f32,
                index: utf8_at.get(*i as usize).copied().unwrap_or(text.len()),
            });
        }
    }
    let width = line.get_typographic_bounds().width as f32;
    (out, width)
}

/// Draw one CoreText glyph into a `w`×`h` device-pixel bitmap with its baseline at `origin`
/// (CSS px from the top-left), as WebKit draws text (font smoothing on). Returns premultiplied
/// RGBA.
pub(crate) fn render(
    font: usize,
    glyph: u16,
    origin: (f32, f32),
    color: (f32, f32, f32),
    w: u32,
    h: u32,
    scale: f32,
) -> Option<Vec<u8>> {
    let font = FONTS.with(|f| f.borrow().fonts.get(font).cloned())?;
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
    cx.scale(scale as f64, scale as f64);
    cx.set_allows_antialiasing(true);
    cx.set_should_antialias(true);
    cx.set_should_smooth_fonts(true);
    cx.set_allows_font_subpixel_positioning(true);
    cx.set_should_subpixel_position_fonts(true);
    cx.set_allows_font_subpixel_quantization(true);
    cx.set_should_subpixel_quantize_fonts(true);
    cx.set_rgb_fill_color(color.0 as f64, color.1 as f64, color.2 as f64, 1.);
    let y = h as f64 / scale as f64 - origin.1 as f64;
    font.draw_glyphs(&[glyph], &[CGPoint::new(origin.0 as f64, y)], cx.clone());
    let stride = cx.bytes_per_row();
    let data = cx.data();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        out.extend_from_slice(&data[row * stride..row * stride + w as usize * 4]);
    }
    Some(out)
}

/// Glyph bounding box in px (x_min, y_min, x_max, y_max), y up.
pub(crate) fn glyph_bounds(font: usize, glyph: u16) -> Option<(f32, f32, f32, f32)> {
    FONTS.with(|f| {
        let f = f.borrow();
        let font = f.fonts.get(font)?;
        let r = font.get_bounding_rects_for_glyphs(
            core_text::font_descriptor::kCTFontOrientationDefault,
            &[glyph],
        );
        Some((
            r.origin.x as f32,
            r.origin.y as f32,
            (r.origin.x + r.size.width) as f32,
            (r.origin.y + r.size.height) as f32,
        ))
    })
}
