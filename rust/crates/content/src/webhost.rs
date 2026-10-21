//! Hidden WKWebViews that run the web UI's own libraries (mermaid.js, pdf.js), so their
//! output is WebKit's. Each [`Host`] is one never-shown page per main thread; `call` runs
//! `window.recur(op)` in it and returns the JSON it resolves to. An op's `deviceScale` makes
//! the page render at that device scale.

#[cfg(not(target_os = "macos"))]
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
}

#[cfg(target_os = "macos")]
pub use mac::{call, call_blocking};

#[cfg(not(target_os = "macos"))]
fn unsupported(host: &Host) -> EngineError {
    EngineError(format!(
        "{} needs the system WebKit, which this platform build does not link",
        host.name
    ))
}

#[cfg(not(target_os = "macos"))]
pub fn call(host: &'static Host, op: Value) -> futures::channel::oneshot::Receiver<Result<Value, EngineError>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let _ = op;
    let _ = tx.send(Err(unsupported(host)));
    rx
}

#[cfg(not(target_os = "macos"))]
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
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::MainThreadMarker;
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
        MainThreadMarker::new().ok_or_else(|| EngineError(format!("{} renders on the main thread", host.name)))
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
        let args: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_retained_objects(&[&*key], &[Retained::into_super(Retained::into_super(payload))]);
        let body = NSString::from_str(
            "for (let i = 0; i < 400 && typeof window.recur !== 'function'; i++) await new Promise(r => setTimeout(r, 5));\nreturn JSON.stringify(await window.recur(JSON.parse(payload)));",
        );
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
