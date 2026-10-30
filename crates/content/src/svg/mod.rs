//! SVG as a browser draws an `<img src="data:image/svg+xml…">` or inline diagram: parsed
//! with usvg (no scripts, no external resources: only `data:` images resolve) and rasterized
//! by resvg at the exact device size.

use std::sync::{Arc, OnceLock};

use gpui_kit::RenderImage;
use resvg::{tiny_skia, usvg};
use smallvec::SmallVec;

fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        // WebKit's generic families on macOS.
        db.set_serif_family("Times");
        db.set_sans_serif_family("Helvetica");
        db.set_monospace_family("Courier");
        Arc::new(db)
    })
    .clone()
}

/// Parse options: nothing outside the document loads, as in an `<img>` context.
pub fn options() -> usvg::Options<'static> {
    let mut o = usvg::Options {
        fontdb: fontdb(),
        font_family: "Times".into(),
        ..Default::default()
    };
    o.image_href_resolver.resolve_string = Box::new(|_, _| None);
    o
}

pub fn parse(svg: &str) -> Result<usvg::Tree, String> {
    usvg::Tree::from_str(svg, &options()).map_err(|e| e.to_string())
}

/// Premultiplied RGBA of `tree` on a `w`×`h` device canvas under `ts` (user space → device).
pub fn rasterize(tree: &usvg::Tree, ts: tiny_skia::Transform, w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(tree, ts, &mut pixmap.as_mut());
    Some(pixmap.take())
}

/// Premultiplied RGBA → a GPUI image (BGRA, straight alpha).
pub fn to_render_image(mut rgba: Vec<u8>, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    for p in rgba.chunks_exact_mut(4) {
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
    let buffer = image::RgbaImage::from_raw(w, h, rgba)?;
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(buffer),
        1,
    ))))
}

/// `tree` drawn into `size` CSS px at `scale` (viewBox fitted, as `<img>` sizes it).
pub fn image_at(
    tree: &usvg::Tree,
    width: f32,
    height: f32,
    scale: f32,
) -> Option<Arc<RenderImage>> {
    let w = (width * scale).round() as u32;
    let h = (height * scale).round() as u32;
    let size = tree.size();
    let ts = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    to_render_image(rasterize(tree, ts, w, h)?, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_images_never_load() {
        let t = parse(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10"><image width="10" height="10" xlink:href="/etc/hosts"/><rect width="4" height="4" fill="red"/></svg>"#).unwrap();
        let images = t
            .root()
            .children()
            .iter()
            .filter(|n| matches!(n, usvg::Node::Image(_)))
            .count();
        assert_eq!(images, 0);
        let px = rasterize(&t, tiny_skia::Transform::identity(), 10, 10).unwrap();
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
    }
}
