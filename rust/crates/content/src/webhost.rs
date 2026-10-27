//! Hidden WebKit pages that run the web UI's own libraries (mermaid.js, pdf.js), so their
//! output is WebKit's: WKWebView on macOS, WebKitGTK 6.0 on Linux. Each [`Host`] is one
//! never-shown page; `call` runs `window.recur(op)` in it and returns the JSON it resolves to.
//! An op's `deviceScale` makes the page render at that device scale (macOS; WebKitGTK uses the
//! first monitor's integer scale).

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use serde_json::Value;

/// Whether webview calls should run synchronously (no application run loop to answer the
/// webview: headless parity renders).
pub fn blocking(cx: &gpui_kit::App) -> bool {
    cx.try_global::<BlockingRenders>().is_some()
}

/// Installed by headless hosts (the parity tool) that have no running application loop.
pub struct BlockingRenders;

impl gpui_kit::Global for BlockingRenders {}

/// A failed call (the platform has no webview, or the page did not answer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A hidden page: its name (in errors) and how its webview is made.
pub struct Host {
    pub name: &'static str,
    #[cfg(target_os = "macos")]
    pub make: fn(objc2::MainThreadMarker) -> objc2::rc::Retained<objc2_web_kit::WKWebView>,
    /// Runs on the WebKitGTK thread.
    #[cfg(target_os = "linux")]
    pub make: fn() -> webkit6::WebView,
}

/// The body `call` runs: waits for the page's `recur`, then returns its answer as JSON.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const CALL_JS: &str = "for (let i = 0; i < 400 && typeof window.recur !== 'function'; i++) await new Promise(r => setTimeout(r, 5));\nreturn JSON.stringify(await window.recur(JSON.parse(payload)));";

#[cfg(target_os = "macos")]
pub use mac::{call, call_blocking};

#[cfg(target_os = "linux")]
pub use gtk::{call, call_blocking};

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn unsupported(host: &Host) -> EngineError {
    EngineError(format!(
        "{} needs the system WebKit, which this platform build does not link",
        host.name
    ))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn call(
    host: &'static Host,
    op: Value,
) -> futures::channel::oneshot::Receiver<Result<Value, EngineError>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let _ = op;
    let _ = tx.send(Err(unsupported(host)));
    rx
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn call_blocking(host: &'static Host, op: Value) -> Result<Value, EngineError> {
    let _ = op;
    Err(unsupported(host))
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use block2::RcBlock;
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
    use futures::channel::oneshot;
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSError, NSString};
    use objc2_web_kit::{WKContentWorld, WKWebView};
    use serde_json::Value;

    use super::{EngineError, Host};

    thread_local! {
        static VIEWS: RefCell<HashMap<&'static str, Retained<WKWebView>>> = RefCell::default();
    }

    fn view(host: &'static Host, mtm: MainThreadMarker) -> Retained<WKWebView> {
        VIEWS.with(|v| {
            v.borrow_mut()
                .entry(host.name)
                .or_insert_with(|| (host.make)(mtm))
                .clone()
        })
    }

    fn mtm(host: &Host) -> Result<MainThreadMarker, EngineError> {
        MainThreadMarker::new()
            .ok_or_else(|| EngineError(format!("{} renders on the main thread", host.name)))
    }

    fn loading(web: &WKWebView) -> bool {
        // SAFETY: a property read on the main thread.
        unsafe { web.isLoading() }
    }

    fn set_device_scale(web: &WKWebView, op: &Value) {
        let Some(scale) = op.get("deviceScale").and_then(Value::as_f64) else {
            return;
        };
        let sel = objc2::sel!(_setOverrideDeviceScaleFactor:);
        // SAFETY: a WebKit SPI with a CGFloat argument, called only when the class has it.
        unsafe {
            let responds: bool = objc2::msg_send![web, respondsToSelector: sel];
            if responds {
                let _: () = objc2::msg_send![web, _setOverrideDeviceScaleFactor: scale];
            }
        }
    }

    fn invoke(
        host: &'static Host,
        web: &WKWebView,
        mtm: MainThreadMarker,
        op: &Value,
        done: impl Fn(Result<Value, EngineError>) + 'static,
    ) {
        set_device_scale(web, op);
        let payload = NSString::from_str(&op.to_string());
        let key = NSString::from_str("payload");
        let args: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_retained_objects(
            &[&*key],
            &[Retained::into_super(Retained::into_super(payload))],
        );
        let body = NSString::from_str(super::CALL_JS);
        let block = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
            if !error.is_null() {
                // SAFETY: WebKit passes a valid NSError when it reports one.
                let msg = unsafe { (*error).localizedDescription() }.to_string();
                done(Err(EngineError(msg)));
                return;
            }
            // SAFETY: the body returns a string (`JSON.stringify`), bridged as NSString.
            let text = unsafe { result.cast::<NSString>().as_ref() }.map(|s| s.to_string());
            match text.and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                Some(v) => done(Ok(v)),
                None => done(Err(EngineError(format!("{} returned nothing", host.name)))),
            }
        });
        // SAFETY: main thread; the block lives until WebKit drops it.
        unsafe {
            web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                &body,
                Some(&args),
                None,
                &WKContentWorld::pageWorld(mtm),
                Some(&block),
            );
        }
    }

    /// Run `op` in the page; resolves on the main run loop.
    pub fn call(host: &'static Host, op: Value) -> oneshot::Receiver<Result<Value, EngineError>> {
        let (tx, rx) = oneshot::channel();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let finish = move |r: Result<Value, EngineError>| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(r);
            }
        };
        match mtm(host) {
            Ok(mtm) => invoke(host, &view(host, mtm), mtm, &op, finish),
            Err(e) => finish(Err(e)),
        }
        rx
    }

    /// Run `op` and spin the main run loop until it answers (headless renders and tests,
    /// where no application loop runs).
    pub fn call_blocking(host: &'static Host, op: Value) -> Result<Value, EngineError> {
        let mtm = mtm(host)?;
        let web = view(host, mtm);
        let deadline = Instant::now() + Duration::from_secs(30);
        // SAFETY: a CoreFoundation constant.
        let mode = unsafe { kCFRunLoopDefaultMode };
        while loading(&web) && Instant::now() < deadline {
            CFRunLoop::run_in_mode(mode, Duration::from_millis(5), true);
        }
        let out: Rc<RefCell<Option<Result<Value, EngineError>>>> = Rc::default();
        let slot = out.clone();
        invoke(host, &web, mtm, &op, move |r| *slot.borrow_mut() = Some(r));
        while out.borrow().is_none() {
            if Instant::now() > deadline {
                return Err(EngineError(format!("{} timed out", host.name)));
            }
            CFRunLoop::run_in_mode(mode, Duration::from_millis(5), true);
        }
        out.borrow_mut()
            .take()
            .unwrap_or_else(|| Err(EngineError(format!("{} returned nothing", host.name))))
    }
}

/// GPUI's Linux event loop is not GLib's, so WebKitGTK runs on a thread of its own that
/// initializes GTK and owns the default main context. Calls are queued to it; a page takes
/// calls once its document has loaded.
#[cfg(target_os = "linux")]
mod gtk {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::OnceLock;
    use std::time::Duration;

    use futures::channel::oneshot;
    use serde_json::Value;
    use webkit6::prelude::*;
    use webkit6::{LoadEvent, WebView, gio, glib};

    use super::{EngineError, Host};

    type Done = Box<dyn FnOnce(Result<Value, EngineError>) + Send>;

    struct Page {
        web: WebView,
        /// Calls made before the first load finished; `None` once it has.
        waiting: RefCell<Option<Vec<(Value, Done)>>>,
    }

    thread_local! {
        static PAGES: RefCell<HashMap<&'static str, Rc<Page>>> = RefCell::default();
    }

    /// The WebKitGTK thread's main context, started on first use. GTK needs a display
    /// (`DISPLAY` or `WAYLAND_DISPLAY`); without one every call fails with GTK's reason.
    fn context() -> Result<&'static glib::MainContext, &'static str> {
        static CONTEXT: OnceLock<Result<glib::MainContext, String>> = OnceLock::new();
        CONTEXT
            .get_or_init(|| {
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::Builder::new()
                    .name("recur-webkit".into())
                    .spawn(move || match webkit6::gtk::init() {
                        Ok(()) => {
                            let _ = tx.send(Ok(glib::MainContext::default()));
                            glib::MainLoop::new(None, false).run();
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e.to_string()));
                        }
                    })
                    .map_err(|e| e.to_string())?;
                rx.recv().map_err(|e| e.to_string())?
            })
            .as_ref()
            .map_err(String::as_str)
    }

    fn page(host: &'static Host) -> Rc<Page> {
        PAGES.with(|pages| {
            pages
                .borrow_mut()
                .entry(host.name)
                .or_insert_with(|| {
                    let page = Rc::new(Page {
                        web: (host.make)(),
                        waiting: RefCell::new(Some(Vec::new())),
                    });
                    let weak = Rc::downgrade(&page);
                    page.web.connect_load_changed(move |_, event| {
                        if event != LoadEvent::Finished {
                            return;
                        }
                        let Some(page) = weak.upgrade() else { return };
                        let queued = page.waiting.borrow_mut().take().unwrap_or_default();
                        for (op, done) in queued {
                            invoke(host, &page.web, &op, done);
                        }
                    });
                    page
                })
                .clone()
        })
    }

    fn run(host: &'static Host, op: Value, done: Done) {
        let page = page(host);
        if let Some(queue) = page.waiting.borrow_mut().as_mut() {
            queue.push((op, done));
            return;
        }
        invoke(host, &page.web, &op, done);
    }

    fn invoke(host: &'static Host, web: &WebView, op: &Value, done: Done) {
        let args = glib::VariantDict::new(None);
        args.insert("payload", op.to_string());
        web.call_async_javascript_function(
            super::CALL_JS,
            Some(&args.end()),
            None,
            None,
            None::<&gio::Cancellable>,
            move |result| {
                done(match result {
                    Err(e) => Err(EngineError(e.message().to_string())),
                    Ok(v) => serde_json::from_str::<Value>(&v.to_str())
                        .map_err(|_| EngineError(format!("{} returned nothing", host.name))),
                })
            },
        );
    }

    fn dispatch(host: &'static Host, op: Value, done: Done) {
        match context() {
            Ok(ctx) => ctx.invoke(move || run(host, op, done)),
            Err(e) => done(Err(EngineError(format!(
                "{} needs WebKitGTK: {e}",
                host.name
            )))),
        }
    }

    /// Run `op` in the page; resolves from the WebKitGTK thread.
    pub fn call(host: &'static Host, op: Value) -> oneshot::Receiver<Result<Value, EngineError>> {
        let (tx, rx) = oneshot::channel();
        dispatch(host, op, Box::new(move |r| drop(tx.send(r))));
        rx
    }

    /// Run `op` and wait for its answer (headless renders and tests).
    pub fn call_blocking(host: &'static Host, op: Value) -> Result<Value, EngineError> {
        let (tx, rx) = std::sync::mpsc::channel();
        dispatch(host, op, Box::new(move |r| drop(tx.send(r))));
        rx.recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| Err(EngineError(format!("{} timed out", host.name))))
    }
}
