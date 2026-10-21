//! The pdf.js host: pdf.js 6.3.289 (the build the web UI ships, vendored under
//! `assets/pdfjs` with the `cmaps`, `standard_fonts` and `wasm` dirs `pdfjsAssetDirs` points
//! it at) in a hidden WKWebView. Pages render to a canvas exactly as `PdfPreview.svelte`
//! renders them; their pixels are drawn natively.
//!
//! The page is served from the `recur-pdf:` scheme: the library, its assets and the open
//! documents' bytes, nothing else. Its CSP allows only same-origin script, workers and fetches
//! (pdf.js runs with eval off and no scripting), the store is non-persistent and the view is
//! never on screen.

use std::sync::Arc;

use base64::Engine as _;
use gpui_kit::RenderImage;
use serde::Deserialize;
use serde_json::{Value, json};
use smallvec::SmallVec;

use crate::webhost::{self, EngineError, Host};

pub static HOST: Host = Host {
    name: "PDF",
    #[cfg(target_os = "macos")]
    make: mac::make,
};

const SCHEME: &str = "recur-pdf";

const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'; img-src 'self' data: blob:; font-src 'self' data: blob:; style-src 'unsafe-inline'";

const INDEX: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"></head><body><script type="module" src="/host.mjs"></script></body></html>"#;

/// `window.recur(op)`: `open` loads a registered document, `render` draws a page at a zoom
/// and device scale into a canvas and returns its RGBA, `cancel` stops a render, `close`
/// destroys a document. Errors resolve as `{ error }`.
const HOST_JS: &str = r#"
import { getDocument, GlobalWorkerOptions, RenderingCancelledException } from '/pdfjs/pdf.min.mjs'

// pdf.js yields between operator chunks on animation frames, which a page outside any window
// never gets; a message-channel turn yields the same way.
{
  const channel = new MessageChannel()
  const queue = new Map()
  let next = 1
  channel.port1.onmessage = () => {
    const due = [...queue.values()]
    queue.clear()
    const now = performance.now()
    for (const cb of due) cb(now)
  }
  window.requestAnimationFrame = (cb) => {
    if (queue.size === 0) channel.port2.postMessage(0)
    queue.set(next, cb)
    return next++
  }
  window.cancelAnimationFrame = (id) => queue.delete(id)
}

GlobalWorkerOptions.workerSrc = new URL('/pdfjs/pdf.worker.min.mjs', location.href).href
const dirs = {
  cMapUrl: new URL('/pdfjs/cmaps/', location.href).href,
  standardFontDataUrl: new URL('/pdfjs/standard_fonts/', location.href).href,
  wasmUrl: new URL('/pdfjs/wasm/', location.href).href,
}
const docs = new Map()
const tasks = new Map()

function base64(bytes) {
  let s = ''
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000))
  return btoa(s)
}

async function open(op) {
  const res = await fetch('/doc/' + op.doc)
  if (!res.ok) throw new Error(`PDF fetch failed (${res.status})`)
  const data = new Uint8Array(await res.arrayBuffer())
  const loading = getDocument({ data, isEvalSupported: false, isOffscreenCanvasSupported: false, ...dirs })
  docs.set(op.doc, loading)
  const pdf = await loading.promise
  const first = await pdf.getPage(1)
  const base = first.getViewport({ scale: 1 })
  return { pages: pdf.numPages, width: base.width, height: base.height }
}

async function render(op) {
  const loading = docs.get(op.doc)
  if (!loading) throw new Error('PDF render failed')
  const pdf = await loading.promise
  const pg = await pdf.getPage(op.page)
  const base = pg.getViewport({ scale: 1 })
  const vp = pg.getViewport({ scale: op.scale })
  const canvas = document.createElement('canvas')
  canvas.width = Math.floor(vp.width * op.dpr)
  canvas.height = Math.floor(vp.height * op.dpr)
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error('PDF render failed')
  const transform = op.dpr === 1 ? undefined : [op.dpr, 0, 0, op.dpr, 0, 0]
  const task = pg.render({ canvas, canvasContext: ctx, viewport: vp, transform })
  tasks.set(op.task, task)
  try {
    await task.promise
  } catch (e) {
    if (e instanceof RenderingCancelledException) return { cancelled: true }
    throw e
  } finally {
    tasks.delete(op.task)
  }
  const { width, height } = canvas
  const rgba = width && height ? base64(new Uint8Array(ctx.getImageData(0, 0, width, height).data.buffer)) : ''
  canvas.width = 0
  canvas.height = 0
  return { width: base.width, height: base.height, pixelWidth: width, pixelHeight: height, rgba }
}

window.recur = async function (op) {
  try {
    if (op.op === 'open') return await open(op)
    if (op.op === 'render') return await render(op)
    if (op.op === 'cancel') {
      tasks.get(op.task)?.cancel()
      return {}
    }
    if (op.op === 'close') {
      const loading = docs.get(op.doc)
      docs.delete(op.doc)
      if (loading) await loading.destroy()
      return {}
    }
    return { error: 'unknown op' }
  } catch (e) {
    const fallback = op.op === 'open' ? 'PDF load failed' : 'PDF render failed'
    return { error: e instanceof Error ? e.message : fallback }
  }
}
"#;

#[cfg(target_os = "macos")]
static ASSETS: include_dir::Dir<'static> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/pdfjs");

thread_local! {
    static DOCS: std::cell::RefCell<std::collections::HashMap<u64, Arc<[u8]>>> = Default::default();
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

/// What the scheme serves at `path`: bytes and content type.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn resource(path: &str) -> Option<(Arc<[u8]>, &'static str)> {
    let mime = |p: &str| match p.rsplit('.').next() {
        Some("mjs" | "js") => "text/javascript",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    };
    match path {
        "/" | "/index.html" => Some((Arc::from(INDEX.as_bytes()), "text/html")),
        "/host.mjs" => Some((Arc::from(HOST_JS.as_bytes()), "text/javascript")),
        _ => {
            if let Some(id) = path.strip_prefix("/doc/") {
                let id = id.parse::<u64>().ok()?;
                return DOCS.with(|d| d.borrow().get(&id).cloned()).map(|b| (b, "application/pdf"));
            }
            #[cfg(target_os = "macos")]
            if let Some(rest) = path.strip_prefix("/pdfjs/") {
                return ASSETS
                    .get_file(rest)
                    .map(|f| (Arc::from(f.contents()), mime(rest)));
            }
            let _ = mime;
            None
        }
    }
}

/// An open document; closing it frees the page's copy.
pub struct Document {
    id: u64,
}

impl Drop for Document {
    fn drop(&mut self) {
        DOCS.with(|d| d.borrow_mut().remove(&self.id));
        drop(webhost::call(&HOST, json!({ "op": "close", "doc": self.id })));
    }
}

/// `open`'s answer: the page count and page 1's size at scale 1 (CSS px).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct Opened {
    pub pages: usize,
    pub width: f32,
    pub height: f32,
}

/// A rendered page: its size at scale 1 and the canvas pixels.
pub struct PageImage {
    pub width: f32,
    pub height: f32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub image: Arc<RenderImage>,
}

fn error(v: &Value) -> Option<String> {
    v.get("error").and_then(Value::as_str).map(str::to_string)
}

fn open_op(bytes: Arc<[u8]>) -> (Document, Value) {
    let id = NEXT.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    DOCS.with(|d| d.borrow_mut().insert(id, bytes));
    (Document { id }, json!({ "op": "open", "doc": id }))
}

fn opened(v: Result<Value, EngineError>) -> Result<Opened, String> {
    let v = v.map_err(|e| e.0)?;
    if let Some(e) = error(&v) {
        return Err(e);
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// Load `bytes`; the document stays open while the returned handle lives.
pub async fn open(bytes: Arc<[u8]>) -> (Document, Result<Opened, String>) {
    let (doc, op) = open_op(bytes);
    let out = webhost::call(&HOST, op)
        .await
        .unwrap_or_else(|_| Err(EngineError("PDF load failed".into())));
    (doc, opened(out))
}

pub fn open_blocking(bytes: Arc<[u8]>) -> (Document, Result<Opened, String>) {
    let (doc, op) = open_op(bytes);
    let out = webhost::call_blocking(&HOST, op);
    (doc, opened(out))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rendered {
    #[serde(default)]
    cancelled: bool,
    #[serde(default)]
    width: f32,
    #[serde(default)]
    height: f32,
    #[serde(default)]
    pixel_width: u32,
    #[serde(default)]
    pixel_height: u32,
    #[serde(default)]
    rgba: String,
}

fn render_op(doc: &Document, page: usize, scale: f32, dpr: f32, task: u64) -> Value {
    json!({ "op": "render", "doc": doc.id, "page": page, "scale": scale, "dpr": dpr, "task": task })
}

/// Canvas RGBA (straight alpha) → a GPUI image (BGRA).
fn page_image(v: Result<Value, EngineError>) -> Result<Option<PageImage>, String> {
    let v = v.map_err(|e| e.0)?;
    if let Some(e) = error(&v) {
        return Err(e);
    }
    let r: Rendered = serde_json::from_value(v).map_err(|e| e.to_string())?;
    if r.cancelled {
        return Ok(None);
    }
    let mut px = base64::engine::general_purpose::STANDARD
        .decode(r.rgba.as_bytes())
        .map_err(|e| e.to_string())?;
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buffer = image::RgbaImage::from_raw(r.pixel_width, r.pixel_height, px).ok_or("PDF render failed")?;
    Ok(Some(PageImage {
        width: r.width,
        height: r.height,
        pixel_width: r.pixel_width,
        pixel_height: r.pixel_height,
        image: Arc::new(RenderImage::new(SmallVec::from_elem(image::Frame::new(buffer), 1))),
    }))
}

/// Render page `page` (1-based) at `scale` for device scale `dpr`, as render task `task`;
/// `Ok(None)` when [`cancel`]led.
pub async fn render(doc: &Document, page: usize, scale: f32, dpr: f32, task: u64) -> Result<Option<PageImage>, String> {
    let out = webhost::call(&HOST, render_op(doc, page, scale, dpr, task))
        .await
        .unwrap_or_else(|_| Err(EngineError("PDF render failed".into())));
    page_image(out)
}

pub fn render_blocking(doc: &Document, page: usize, scale: f32, dpr: f32, task: u64) -> Result<Option<PageImage>, String> {
    page_image(webhost::call_blocking(&HOST, render_op(doc, page, scale, dpr, task)))
}

/// Cancel render task `task` (`task.cancel()`); its render resolves `Ok(None)`.
pub fn cancel(task: u64) {
    drop(webhost::call(&HOST, json!({ "op": "cancel", "task": task })));
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_foundation::{NSData, NSDictionary, NSHTTPURLResponse, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest};
    use objc2_web_kit::{
        WKURLSchemeHandler, WKURLSchemeTask, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
    };

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and the class does not implement
        // Drop.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "RecurNextPdfScheme"]
        struct Scheme;

        unsafe impl NSObjectProtocol for Scheme {}

        unsafe impl WKURLSchemeHandler for Scheme {
            #[unsafe(method(webView:startURLSchemeTask:))]
            fn start(&self, _web: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
                // SAFETY: the task is live until `didFinish`; all calls are on the main thread.
                unsafe {
                    let request: Retained<NSURLRequest> = task.request();
                    let Some(url) = request.URL() else { return };
                    let path = url.path().map(|p| p.to_string()).unwrap_or_default();
                    let found = super::resource(&path);
                    let (status, body, mime) = match &found {
                        Some((bytes, mime)) => (200, &bytes[..], *mime),
                        None => (404, &[][..], "text/plain"),
                    };
                    let keys = [
                        &*NSString::from_str("Content-Type"),
                        &*NSString::from_str("Content-Security-Policy"),
                    ];
                    let values = [&*NSString::from_str(mime), &*NSString::from_str(super::CSP)];
                    let headers = NSDictionary::from_slices(&keys, &values);
                    let Some(response) = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
                        NSHTTPURLResponse::alloc(),
                        &url,
                        status,
                        Some(&NSString::from_str("HTTP/1.1")),
                        Some(&headers),
                    ) else {
                        return;
                    };
                    task.didReceiveResponse(&response);
                    task.didReceiveData(&NSData::with_bytes(body));
                    task.didFinish();
                }
            }

            #[unsafe(method(webView:stopURLSchemeTask:))]
            fn stop(&self, _web: &WKWebView, _task: &ProtocolObject<dyn WKURLSchemeTask>) {}
        }
    );

    pub fn make(mtm: MainThreadMarker) -> Retained<WKWebView> {
        // SAFETY: WebKit objects are created and used on the main thread (`mtm`).
        unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
            let scheme: Retained<Scheme> = msg_send![Scheme::alloc(mtm), init];
            config.setURLSchemeHandler_forURLScheme(
                Some(ProtocolObject::from_ref(&*scheme)),
                &NSString::from_str(super::SCHEME),
            );
            let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(1024., 768.));
            let web = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config);
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(&format!("{}://app/index.html", super::SCHEME))) {
                web.loadRequest(&NSURLRequest::requestWithURL(&url));
            }
            web
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_the_library_its_assets_and_registered_documents_only() {
        let (bytes, mime) = resource("/index.html").unwrap();
        assert!(std::str::from_utf8(&bytes).unwrap().contains("/host.mjs"));
        assert_eq!(mime, "text/html");
        assert_eq!(resource("/host.mjs").unwrap().1, "text/javascript");
        #[cfg(target_os = "macos")]
        {
            assert_eq!(resource("/pdfjs/pdf.worker.min.mjs").unwrap().1, "text/javascript");
            assert_eq!(resource("/pdfjs/wasm/openjpeg.wasm").unwrap().1, "application/wasm");
            assert!(resource("/pdfjs/standard_fonts/FoxitSerif.pfb").is_some());
        }
        assert!(resource("/pdfjs/../Cargo.toml").is_none());
        assert!(resource("/etc/passwd").is_none());
        let (doc, _) = open_op(Arc::from(&b"%PDF-1.7"[..]));
        assert_eq!(&*resource(&format!("/doc/{}", doc.id)).unwrap().0, b"%PDF-1.7");
        let id = doc.id;
        DOCS.with(|d| d.borrow_mut().remove(&id));
        std::mem::forget(doc);
        assert!(resource(&format!("/doc/{id}")).is_none());
    }
}
