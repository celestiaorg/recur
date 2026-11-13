//! PDF pages through `gpui-pdf` (hayro, pure Rust): parse once, rasterize a page at the zoom
//! and device scale. Both are CPU-bound; `PdfPreview` runs them on the background executor.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::RenderImage;

pub use gpui_pdf::{Document, NormRect};

/// A parsed document and every page's size at scale 1 (PDF points, pdf.js's CSS px).
pub struct Opened {
    pub doc: Arc<Document>,
    pub sizes: Vec<(f32, f32)>,
}

pub fn open(bytes: &[u8]) -> Result<Opened, String> {
    let doc = gpui_pdf::parse(Arc::new(bytes.to_vec())).map_err(|e| match e {
        gpui_pdf::LoadError::Locked => "This PDF is password-protected.".to_string(),
        gpui_pdf::LoadError::Other(e) => e,
    })?;
    let sizes = gpui_pdf::page_dims(&doc);
    if sizes.is_empty() {
        return Err("This PDF has no pages.".into());
    }
    Ok(Opened { doc, sizes })
}

/// Page `page` (1-based) at `scale` for device scale `dpr`, BGRA on white.
pub fn render(
    doc: &Document,
    page: usize,
    scale: f32,
    dpr: f32,
) -> Result<Arc<RenderImage>, String> {
    gpui_pdf::render_page(doc, page - 1, scale * dpr)
}

/// The text layer find searches: each page's readable text as a block (0-based page ids).
pub struct FindText {
    pub blocks: Vec<(usize, String)>,
    /// Each page's layer and its search key (`search_key` of the block).
    layers: Vec<(gpui_pdf::PageText, String)>,
}

/// gpui-pdf's `PageText` index: lowercased with whitespace removed.
fn search_key(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Extract every page's text layer (a parse pass per page, cheaper than rendering).
pub fn find_text(doc: &Document) -> FindText {
    let mut out = FindText {
        blocks: Vec::new(),
        layers: Vec::new(),
    };
    for i in 0..doc.pages().len() {
        let Some(layer) = gpui_pdf::extract_page_text(doc, i) else {
            break;
        };
        let text = layer.text();
        out.layers.push((layer, search_key(&text)));
        out.blocks.push((i, text));
    }
    out
}

impl FindText {
    /// Where bytes `range` of page `page`'s block sit, one normalized rect per line. The
    /// layer only locates the nth whitespace- and case-insensitive occurrence, so `n` is how
    /// many occurrences start before the range does.
    pub fn rects(&self, page: usize, range: Range<usize>) -> Vec<NormRect> {
        let (Some((_, text)), Some((layer, key))) = (self.blocks.get(page), self.layers.get(page))
        else {
            return Vec::new();
        };
        let Some(needle) = text.get(range.clone()) else {
            return Vec::new();
        };
        let want = search_key(needle);
        if want.is_empty() {
            return Vec::new();
        }
        let at = search_key(&text[..range.start]).len();
        let n = (0..at)
            .filter(|&i| key.as_bytes()[i..].starts_with(want.as_bytes()))
            .count();
        layer.locate(needle, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDF: &[u8] = include_bytes!("../../../ui/gallery/content/page.pdf");

    #[test]
    fn opens_and_renders_the_parity_page() {
        let o = open(PDF).expect("open");
        assert_eq!(o.sizes, [(420., 300.)]);
        let img = render(&o.doc, 1, 1., 2.).expect("render");
        let size = img.size(0);
        assert_eq!((size.width.0, size.height.0), (840, 600));
        let px = img.as_bytes(0).expect("frame");
        assert!(
            px.as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] != [255, 255, 255]),
            "blank page"
        );
        assert!(render(&o.doc, 2, 1., 1.).is_err());
        assert!(open(b"%PDF-1.7 nope").is_err());
    }

    #[test]
    fn find_maps_matches_to_their_occurrence() {
        let o = open(PDF).expect("open");
        let f = find_text(&o.doc);
        let text = &f.blocks[0].1;
        assert!(text.contains("Quarterly report"), "{text:?}");
        let hits: Vec<usize> = text.match_indices("the").map(|(i, _)| i).collect();
        assert!(hits.len() >= 2, "{text:?}");
        let rect = |i: usize| f.rects(0, i..i + 3)[0];
        assert_ne!(rect(hits[0]), rect(hits[1]));
        let title = f.rects(0, 0..9);
        assert_eq!(title.len(), 1);
        assert!(title[0].y < rect(hits[0]).y, "the title is above the body");
        assert!(f.rects(0, 0..0).is_empty());
        assert!(f.rects(1, 0..3).is_empty());
    }
}
