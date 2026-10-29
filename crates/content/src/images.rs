//! Image sources for markdown and the lightbox. `data:` URLs decode here; everything else
//! (`/gh-image?url=…`, `/workspace-file?…`, `https://…`) goes through the host's
//! [`ImageLoader`], the native stand-in for the webview's asset server and network stack.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use futures::future::BoxFuture;
use gpui_kit::{App, Global, RenderImage};
use smallvec::SmallVec;

/// Bytes for an image `src` the content crate cannot resolve itself.
pub trait ImageLoader: Send + Sync {
    fn load(&self, src: &str) -> BoxFuture<'static, Result<Vec<u8>, String>>;
}

/// The host's loader, installed once (`recur_content::set_image_loader`).
#[derive(Clone)]
pub struct Loader(pub Arc<dyn ImageLoader>);

impl Global for Loader {}

pub fn set_image_loader(cx: &mut App, loader: Arc<dyn ImageLoader>) {
    cx.set_global(Loader(loader));
}

/// A decoded image: GPU-ready frames and the natural size in CSS px (one image pixel per
/// CSS px, as `<img>` sizes them).
#[derive(Clone)]
pub struct Decoded {
    pub image: Arc<RenderImage>,
    pub width: f32,
    pub height: f32,
    /// PNG bytes for the clipboard (`copyImage` writes `image/png`).
    pub png: Arc<Vec<u8>>,
}

/// The payload of a `data:` URL.
pub fn data_url_bytes(src: &str) -> Option<Vec<u8>> {
    let rest = src.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    if meta.ends_with(";base64") {
        let clean: String = data.chars().filter(|c| !c.is_whitespace()).collect();
        base64::engine::general_purpose::STANDARD.decode(clean).ok()
    } else {
        Some(percent_decode(data))
    }
}

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Decode raster bytes (PNG, JPEG, GIF, WebP) into frames GPUI paints (BGRA).
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba.clone())
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let mut bgra = rgba;
    for p in bgra.pixels_mut() {
        p.0.swap(0, 2);
    }
    Ok(Decoded {
        image: Arc::new(RenderImage::new(SmallVec::from_elem(
            image::Frame::new(bgra),
            1,
        ))),
        width: w as f32,
        height: h as f32,
        png: Arc::new(png),
    })
}

/// The raster resampled to `w`×`h` device pixels (bilinear, edges clamped, as WebKit draws a
/// scaled `<img>`), so it paints 1:1 instead of through GPUI's texture filter, which blends
/// the edge rows with the atlas padding.
pub fn resized(d: &Decoded, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let img = image::load_from_memory_with_format(&d.png, image::ImageFormat::Png).ok()?;
    let mut out = image::imageops::resize(
        &img.to_rgba8(),
        w.max(1),
        h.max(1),
        image::imageops::FilterType::Triangle,
    );
    for p in out.pixels_mut() {
        p.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(out),
        1,
    ))))
}

/// A device size and the raster resampled to it.
type Scaled = ((u32, u32), Arc<RenderImage>);

/// A raster loading or loaded for a `src`.
#[derive(Clone, Default)]
pub struct Raster {
    pub src: String,
    pub image: Option<Decoded>,
    pub failed: bool,
    /// The raster resampled to its painted device size.
    pub scaled: std::cell::RefCell<Option<Scaled>>,
}

impl Raster {
    /// The image to paint at `w`×`h` device pixels.
    pub fn at(&self, w: u32, h: u32) -> Option<Arc<RenderImage>> {
        let d = self.image.as_ref()?;
        if (d.width as u32, d.height as u32) == (w, h) {
            return Some(d.image.clone());
        }
        let mut c = self.scaled.borrow_mut();
        if let Some((size, img)) = c.as_ref()
            && *size == (w, h)
        {
            return Some(img.clone());
        }
        let img = resized(d, w, h)?;
        *c = Some(((w, h), img.clone()));
        Some(img)
    }
}

/// Start loading `src`: `data:` URLs decode off the UI thread; others ask the loader.
pub fn fetch(src: &str, cx: &App) -> Option<BoxFuture<'static, Result<Decoded, String>>> {
    if src.starts_with("data:") {
        let bytes = data_url_bytes(src)?;
        return Some(Box::pin(async move { decode(&bytes) }));
    }
    let loader = cx.try_global::<Loader>()?.0.clone();
    let fut = loader.load(src);
    Some(Box::pin(async move { decode(&fut.await?) }))
}

/// `workspaceFileURL`: a worktree file without the JSON bridge. `mtime` 0 is omitted. `at` asks
/// for a git revision of an image: `base` (the merge-base), a commit-ish, or `A...B`.
pub fn workspace_file_url(
    project_id: &str,
    workspace_id: &str,
    path: &str,
    mtime: i64,
    at: &str,
) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.append_pair("project", project_id);
    if !workspace_id.is_empty() {
        q.append_pair("workspace", workspace_id);
    }
    q.append_pair("path", path);
    if mtime != 0 {
        q.append_pair("t", &mtime.to_string());
    }
    if !at.is_empty() {
        q.append_pair("at", at);
    }
    format!("/workspace-file?{}", q.finish())
}

/// Decoded images by `src`, shared by every view in the app (the webview's image cache).
#[derive(Default)]
pub struct ImageCache(pub HashMap<String, Decoded>);

impl Global for ImageCache {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_data_urls() {
        assert_eq!(data_url_bytes("data:text/plain,a%20b").unwrap(), b"a b");
        assert_eq!(data_url_bytes("data:;base64,aGk=").unwrap(), b"hi");
    }
}
