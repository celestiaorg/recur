//! Offscreen full-page screenshots (the browser_screenshot tool). Each width loads the URL
//! fresh in a [`BrowserPage`](crate::BrowserPage)'s WKWebView, so it has the same protections:
//! its own non-persistent data store, Recur's ports blocked by the content rule list and the
//! navigation delegate. The view sits in the app's window outside its bounds, where WebKit
//! renders it as a visible page and nobody sees it, and the visible browser never resizes.
//!
//! A page-world user script records console errors, uncaught exceptions and failed requests;
//! after load the log is read back with `evaluateJavaScript`. Its content is page-controlled,
//! so the script and the reader both cap it. No script message handler is registered.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::{App, Task};

/// The first viewport height; the page is then grown to its full height, up to [`MAX_HEIGHT`].
pub const VIEWPORT_HEIGHT: u32 = 900;
pub const MAX_HEIGHT: u32 = 8000;
/// Load, settle and snapshot of one width.
pub const WIDTH_TIMEOUT: Duration = Duration::from_secs(25);
#[cfg(target_os = "macos")]
const SETTLE: Duration = Duration::from_millis(600);
/// The page log's JSON is page-controlled: a longer one is dropped unread.
const MAX_LOG_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub path: Option<PathBuf>,
    pub error: String,
}

/// What the page showed and logged while loading at each width.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageShots {
    pub shots: Vec<Shot>,
    pub console_errors: Vec<String>,
    pub exceptions: Vec<String>,
    pub failed_requests: Vec<String>,
}

impl PageShots {
    /// Adds the page log read back after load: `[[kind, text], …]`.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn add_log(&mut self, json: &str) {
        if json.len() > MAX_LOG_BYTES {
            self.console_errors
                .push("(the page's log was too large to read)".into());
            return;
        }
        let entries: Vec<(String, String)> = serde_json::from_str(json).unwrap_or_default();
        for (kind, text) in entries.into_iter().take(LOG_MAX) {
            let text: String = text.chars().take(LOG_LEN).collect();
            match kind.as_str() {
                "console" => self.console_errors.push(text),
                "exception" => self.exceptions.push(text),
                "request" => self.failed_requests.push(text),
                _ => {}
            }
        }
    }
}

const LOG_MAX: usize = 100;
const LOG_LEN: usize = 1000;

/// Runs in the page world at document start, before the page's own scripts.
#[cfg(target_os = "macos")]
const HOOKS_JS: &str = r#"(() => {
  const MAX = 100, LEN = 1000, log = [];
  try { Object.defineProperty(window, '__recurShotLog', { value: log }); } catch (_) { return; }
  const str = (v) => {
    try {
      if (typeof v === 'string') return v;
      if (v instanceof Error) return v.stack ? `${v}\n${v.stack}` : String(v);
      const j = JSON.stringify(v);
      return j === undefined ? String(v) : j;
    } catch (_) {
      try { return String(v); } catch (_) { return '?'; }
    }
  };
  const push = (kind, parts) => {
    if (log.length < MAX) log.push([kind, parts.map(str).join(' ').slice(0, LEN)]);
  };
  const error = console.error;
  console.error = function (...a) { push('console', a); return error.apply(this, a); };
  addEventListener('error', (e) => {
    const t = e.target;
    if (t && t !== window && t.tagName) {
      push('request', [t.tagName.toLowerCase(), t.currentSrc || t.src || t.href || '']);
    } else {
      push('exception', [e.error || e.message, e.filename ? `(${e.filename}:${e.lineno})` : '']);
    }
  }, true);
  addEventListener('unhandledrejection', (e) => push('exception', ['Unhandled rejection:', e.reason]));
  const f = window.fetch;
  if (f) {
    window.fetch = function (...a) {
      const u = a[0] && a[0].url ? a[0].url : String(a[0]);
      return f.apply(this, a).then(
        (r) => { if (!r.ok) push('request', ['fetch', u, r.status]); return r; },
        (err) => { push('request', ['fetch', u, err]); throw err; });
    };
  }
  const open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (m, u) {
    this.addEventListener('loadend', () => {
      if (!this.status || this.status >= 400) push('request', ['xhr', m, String(u), this.status || 'failed']);
    });
    return open.apply(this, arguments);
  };
})();"#;

#[cfg(target_os = "macos")]
const HEIGHT_JS: &str = "Math.max(document.documentElement.scrollHeight, \
document.body ? document.body.scrollHeight : 0)";
#[cfg(target_os = "macos")]
const LOG_JS: &str = "JSON.stringify(window.__recurShotLog || [])";

/// Screenshots `url` at each width into `out_dir`. The URL must be http(s); one naming a
/// `blocked_ports` port is refused.
pub fn capture(
    url: String,
    widths: Vec<u32>,
    blocked_ports: Vec<u16>,
    out_dir: PathBuf,
    cx: &mut App,
) -> Task<Result<PageShots, String>> {
    let scheme = url
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return Task::ready(Err(format!("{url} is not an http(s) URL")));
    }
    if crate::browser::is_blocked(&url, &blocked_ports) {
        return Task::ready(Err("Recur's own servers cannot be opened".into()));
    }
    #[cfg(target_os = "macos")]
    {
        cx.spawn(async move |cx| {
            std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            let exec = cx.background_executor().clone();
            let mut out = PageShots::default();
            for width in widths {
                let path = out_dir.join(format!("{stamp}-{width}.png"));
                let shot = mac::shoot(&url, width, &blocked_ports, exec.clone());
                let timeout = exec.timer(WIDTH_TIMEOUT);
                let res = match futures::future::select(std::pin::pin!(shot), timeout).await {
                    futures::future::Either::Left((r, _)) => r,
                    futures::future::Either::Right(_) => Err(mac::Failure::Shot(format!(
                        "timed out after {}s",
                        WIDTH_TIMEOUT.as_secs()
                    ))),
                };
                let shot = match res {
                    Ok(page) => {
                        out.add_log(&page.log);
                        let saved = save_png(page.rgba, width, page.height, &path, &exec).await;
                        Shot {
                            width,
                            height: page.height,
                            path: saved.is_ok().then(|| path.clone()),
                            error: saved.err().unwrap_or_default(),
                        }
                    }
                    Err(mac::Failure::Load(message)) => {
                        out.failed_requests.push(format!("{url}: {message}"));
                        Shot {
                            width,
                            error: format!("the page did not load: {message}"),
                            ..Shot::default()
                        }
                    }
                    Err(mac::Failure::Shot(error)) => Shot {
                        width,
                        error,
                        ..Shot::default()
                    },
                };
                out.shots.push(shot);
            }
            Ok(out)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (widths, out_dir, cx);
        Task::ready(Err("the browser is not available on this platform".into()))
    }
}

/// Encodes and writes the PNG off the main thread.
#[cfg(target_os = "macos")]
async fn save_png(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    path: &std::path::Path,
    exec: &gpui_kit::BackgroundExecutor,
) -> Result<(), String> {
    let path = path.to_path_buf();
    exec.spawn(async move {
        let img = image::RgbaImage::from_raw(width, height, rgba)
            .ok_or_else(|| "snapshot has the wrong size".to_string())?;
        img.save_with_format(&path, image::ImageFormat::Png)
            .map_err(|e| format!("{}: {e}", path.display()))
    })
    .await
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::cell::Cell;
    use std::time::Duration;

    use block2::RcBlock;
    use futures::channel::{mpsc, oneshot};
    use gpui_kit::BackgroundExecutor;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage,
    };
    use objc2_foundation::{NSError, NSNumber, NSPoint, NSRect, NSSize, NSString};
    use objc2_web_kit::{
        WKSnapshotConfiguration, WKUserScript, WKUserScriptInjectionTime, WKWebView,
    };

    use super::{HEIGHT_JS, HOOKS_JS, LOG_JS, MAX_HEIGHT, SETTLE, VIEWPORT_HEIGHT};
    use crate::browser::Msg;
    use crate::browser::mac::Page;

    pub(super) enum Failure {
        /// The main navigation failed (or was blocked).
        Load(String),
        Shot(String),
    }

    pub(super) struct Loaded {
        pub height: u32,
        pub rgba: Vec<u8>,
        pub log: String,
    }

    /// Takes the page off the window however the shot ends, timeouts included.
    struct Offscreen(Option<Page>);

    impl Drop for Offscreen {
        fn drop(&mut self) {
            if let Some(p) = self.0.take() {
                p.remove();
            }
        }
    }

    const POLL: Duration = Duration::from_millis(100);

    pub(super) async fn shoot(
        url: &str,
        width: u32,
        ports: &[u16],
        exec: BackgroundExecutor,
    ) -> Result<Loaded, Failure> {
        let shot = |e: &str| Failure::Shot(e.to_string());
        let mtm = MainThreadMarker::new().ok_or_else(|| shot("not on the main thread"))?;
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let page = Offscreen(Some(
            Page::new(ports, tx).ok_or_else(|| shot("could not create a web view"))?,
        ));
        let handle = page.0.as_ref().expect("just set").handle();
        let web = handle.web();
        // SAFETY: WebKit objects used on the main thread.
        unsafe {
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(HOOKS_JS),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            );
            web.configuration()
                .userContentController()
                .addUserScript(&script);
        }
        let x = -f64::from(width) - 4000.;
        set_size(web, x, width, VIEWPORT_HEIGHT);
        if let Some(content) = NSApplication::sharedApplication(mtm)
            .windows()
            .iter()
            .filter(|w| w.isVisible())
            .find_map(|w| w.contentView())
        {
            content.addSubview(web);
        }
        web.setHidden(false);
        page.0.as_ref().expect("just set").load(url);

        loop {
            exec.timer(POLL).await;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::Failed {
                        code: -999 | 102, ..
                    } => {}
                    Msg::Failed { message, .. } => return Err(Failure::Load(message)),
                    Msg::Blocked => {
                        return Err(Failure::Load("Recur's own servers cannot be opened".into()));
                    }
                    _ => {}
                }
            }
            // SAFETY: plain property reads on the main thread.
            let loaded = unsafe { !web.isLoading() && web.URL().is_some() };
            if loaded {
                break;
            }
        }
        exec.timer(SETTLE).await;

        let full = eval(web, HEIGHT_JS)
            .await
            .ok()
            .and_then(|v| v.downcast_ref::<NSNumber>().map(|n| n.as_f64()))
            .unwrap_or(f64::from(VIEWPORT_HEIGHT));
        let height = (full.ceil() as u32).clamp(1, MAX_HEIGHT);
        if height != VIEWPORT_HEIGHT {
            set_size(web, x, width, height);
            exec.timer(Duration::from_millis(300)).await;
        }
        let log = eval(web, LOG_JS)
            .await
            .ok()
            .and_then(|v| v.downcast_ref::<NSString>().map(|s| s.to_string()))
            .unwrap_or_default();
        let image = snapshot(web, width, mtm).await.map_err(Failure::Shot)?;
        let rgba =
            rgba(&image, width, height).ok_or_else(|| shot("could not read the snapshot"))?;
        Ok(Loaded { height, rgba, log })
    }

    fn set_size(web: &WKWebView, x: f64, width: u32, height: u32) {
        web.setFrame(NSRect::new(
            NSPoint::new(x, 0.),
            NSSize::new(f64::from(width), f64::from(height)),
        ));
    }

    async fn eval(web: &WKWebView, js: &str) -> Result<Retained<AnyObject>, String> {
        let (tx, rx) = oneshot::channel();
        let tx = Cell::new(Some(tx));
        let done = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            // SAFETY: WebKit hands a valid result or error, on the main thread.
            let res = unsafe { Retained::retain(value) }.ok_or_else(|| {
                unsafe { error.as_ref() }
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_default()
            });
            if let Some(tx) = tx.take() {
                let _ = tx.send(res);
            }
        });
        // SAFETY: main thread; WebKit retains the block until it runs.
        unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&done)) };
        rx.await.map_err(|_| "the page went away".to_string())?
    }

    /// The view's whole frame, rendered by the web process (not waiting on a screen update,
    /// which an occluded window never gets).
    async fn snapshot(
        web: &WKWebView,
        width: u32,
        mtm: MainThreadMarker,
    ) -> Result<Retained<NSImage>, String> {
        let (tx, rx) = oneshot::channel();
        let tx = Cell::new(Some(tx));
        let done = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            // SAFETY: WebKit hands a valid image or error, on the main thread.
            let res = unsafe { Retained::retain(image) }.ok_or_else(|| {
                unsafe { error.as_ref() }
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "the snapshot failed".into())
            });
            if let Some(tx) = tx.take() {
                let _ = tx.send(res);
            }
        });
        // SAFETY: main thread; WebKit retains the block until it runs.
        unsafe {
            let config = WKSnapshotConfiguration::new(mtm);
            config.setAfterScreenUpdates(false);
            config.setSnapshotWidth(Some(&NSNumber::new_f64(f64::from(width))));
            web.takeSnapshotWithConfiguration_completionHandler(Some(&config), &done);
        }
        rx.await.map_err(|_| "the page went away".to_string())?
    }

    /// The image drawn at one pixel per CSS px, as tightly packed RGBA rows.
    fn rgba(image: &NSImage, width: u32, height: u32) -> Option<Vec<u8>> {
        let (w, h) = (width as usize, height as usize);
        // SAFETY: a fresh bitmap this call owns, drawn into on the main thread.
        unsafe {
            let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                w as isize,
                h as isize,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                0,
                0,
            )?;
            let size = NSSize::new(f64::from(width), f64::from(height));
            rep.setSize(size);
            let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
            NSGraphicsContext::saveGraphicsState_class();
            NSGraphicsContext::setCurrentContext(Some(&ctx));
            image.drawInRect(NSRect::new(NSPoint::new(0., 0.), size));
            ctx.flushGraphics();
            NSGraphicsContext::restoreGraphicsState_class();
            let stride = usize::try_from(rep.bytesPerRow()).ok()?;
            let data = rep.bitmapData();
            if data.is_null() || stride < w * 4 {
                return None;
            }
            let bytes = std::slice::from_raw_parts(data, stride * h);
            Some(
                bytes
                    .chunks_exact(stride)
                    .flat_map(|row| &row[..w * 4])
                    .copied()
                    .collect(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_is_sorted_by_kind_and_capped() {
        let mut out = PageShots::default();
        let long = "x".repeat(LOG_LEN + 50);
        let mut entries = vec![
            ("console".to_string(), long.clone()),
            ("exception".into(), "boom".into()),
            ("request".into(), "img /a.png".into()),
            ("other".into(), "dropped".into()),
        ];
        entries.extend((0..LOG_MAX).map(|i| ("console".to_string(), i.to_string())));
        out.add_log(&serde_json::to_string(&entries).unwrap());
        assert_eq!(out.console_errors.len(), LOG_MAX - 3);
        assert_eq!(out.console_errors[0].chars().count(), LOG_LEN);
        assert_eq!(out.exceptions, ["boom"]);
        assert_eq!(out.failed_requests, ["img /a.png"]);
    }

    #[test]
    fn oversized_or_invalid_log_is_not_parsed() {
        let mut out = PageShots::default();
        out.add_log(&"[".repeat(MAX_LOG_BYTES + 1));
        assert_eq!(out.console_errors.len(), 1);
        let mut out = PageShots::default();
        out.add_log("{not json");
        assert_eq!(out, PageShots::default());
    }
}
