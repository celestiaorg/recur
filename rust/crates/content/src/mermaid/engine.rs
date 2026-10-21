//! The mermaid.js host: mermaid 11.16.1 (the build the web UI ships, vendored under
//! `assets/mermaid`) in a hidden WKWebView. Mermaid lays diagrams out from DOM text
//! measurement, so it runs in WebKit itself and measures exactly as the app's webview did; the
//! diagram it returns is drawn natively. The page loads nothing (CSP `default-src 'none'`),
//! keeps no data (non-persistent store) and is never on screen.

use serde_json::Value;

/// The page mermaid runs in: the app's web fonts (the files `frontend/index.html` loads, as
/// recur-ui bundles them) and `body` font, which mermaid's text measurement inherits.
pub fn page() -> String {
    use base64::Engine as _;
    let mut faces = String::new();
    for (family, weight, bytes) in recur_ui::fonts::FACES {
        faces.push_str(&format!(
            "@font-face{{font-family:'{family}';font-style:normal;font-weight:{weight};font-display:block;src:url(data:font/ttf;base64,{}) format('truetype');}}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
    }
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-eval'; style-src 'unsafe-inline'; img-src data:; font-src data:"><style>{faces}*{{box-sizing:border-box}}html,body{{margin:0;height:100%;overflow:hidden}}body{{font-family:'Inter',system-ui,sans-serif;font-size:13px;line-height:1.45}}</style></head><body><div id="m"></div></body></html>"#
    )
}

/// Injected after mermaid: `recur(op)` renders a source or measures a sanitized SVG.
pub const HOST_JS: &str = r#"
(function () {
  function unclip(root) {
    var svg = root.querySelector('svg');
    if (!svg) return;
    svg.setAttribute('overflow', 'visible');
    svg.querySelectorAll('g, foreignObject, .label, .node, .cluster, .edgeLabel, .nodeLabel').forEach(function (n) {
      n.setAttribute('overflow', 'visible');
      if (n.style) n.style.overflow = 'visible';
    });
  }
  // Paint WebKit resolved, inlined so the native rasterizer needs no selector support.
  var PAINT = ['fill', 'fill-opacity', 'fill-rule', 'stroke', 'stroke-width', 'stroke-opacity', 'stroke-dasharray',
    'stroke-dashoffset', 'stroke-linecap', 'stroke-linejoin', 'stroke-miterlimit', 'opacity', 'display', 'visibility'];
  function collapse(s) { return s.replace(/[\n\r\t]/g, ' ').replace(/ +/g, ' '); }
  function measure(html) {
    var host = document.getElementById('m');
    host.innerHTML = html;
    unclip(host);
    var svg = host.querySelector('svg');
    if (!svg) return { error: 'no svg' };
    var vb = svg.viewBox && svg.viewBox.baseVal;
    var w = vb && vb.width > 0 ? vb.width : parseFloat(svg.getAttribute('width')) || 0;
    var h = vb && vb.height > 0 ? vb.height : parseFloat(svg.getAttribute('height')) || 0;
    var ox = vb && vb.width > 0 ? vb.x : 0, oy = vb && vb.height > 0 ? vb.y : 0;
    svg.removeAttribute('style');
    svg.setAttribute('width', String(w));
    svg.setAttribute('height', String(h));
    svg.style.width = w + 'px';
    svg.style.height = h + 'px';
    svg.style.maxWidth = 'none';
    var inv = svg.getScreenCTM().inverse();
    var texts = svg.querySelectorAll('text');
    var runs = [], raster = [];
    for (var ti = 0; ti < texts.length; ti++) {
      var t = texts[ti];
      var m = inv.multiply(t.getScreenCTM());
      if (Math.abs(m.b) > 1e-6 || Math.abs(m.c) > 1e-6 || m.a <= 0 || m.d <= 0 || Math.abs(m.a - m.d) > 1e-6) { raster.push(ti); continue; }
      var leaves = [];
      (function walk(el) {
        var own = false;
        for (var c = el.firstChild; c; c = c.nextSibling) {
          if (c.nodeType === 3 && c.data.length) own = true;
          if (c.nodeType === 1) walk(c);
        }
        if (own) leaves.push(el);
      })(t);
      var ok = true, mine = [];
      for (var li = 0; li < leaves.length && ok; li++) {
        var el = leaves[li];
        var cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden') continue;
        var text = '';
        for (var c = el.firstChild; c; c = c.nextSibling) if (c.nodeType === 3) text += c.data;
        var n = el.getNumberOfChars();
        if (!n) continue;
        var trimmed = collapse(text);
        if (li === 0) trimmed = trimmed.replace(/^ /, '');
        if (li === leaves.length - 1) trimmed = trimmed.replace(/ $/, '');
        if (trimmed.length !== n) { ok = false; break; }
        var p = el.getStartPositionOfChar(0);
        var end = el.getEndPositionOfChar(n - 1);
        var op = 1;
        for (var e = el; e && e !== svg; e = e.parentElement) op *= parseFloat(getComputedStyle(e).opacity);
        mine.push({
          text: trimmed, x: p.x * m.a + m.e, y: p.y * m.d + m.f,
          size: parseFloat(cs.fontSize) * m.a, family: cs.fontFamily, weight: parseFloat(cs.fontWeight) || 400,
          italic: cs.fontStyle === 'italic' || cs.fontStyle === 'oblique', fill: cs.fill,
          fillOpacity: parseFloat(cs.fillOpacity), opacity: op,
          spacing: cs.letterSpacing === 'normal' ? 0 : parseFloat(cs.letterSpacing) || 0,
          decoration: cs.textDecorationLine, advance: (end.x - p.x) * m.a, text_index: ti
        });
      }
      if (ok) runs = runs.concat(mine); else raster.push(ti);
    }
    var els = svg.querySelectorAll('*');
    var styles = Array.prototype.map.call(els, function (el) {
      var cs = getComputedStyle(el);
      return PAINT.map(function (p) { return cs.getPropertyValue(p); });
    });
    for (var ei = 0; ei < els.length; ei++) {
      for (var pi = 0; pi < PAINT.length; pi++) els[ei].style.setProperty(PAINT[pi], styles[ei][pi]);
    }
    return { width: w, height: h, x: ox, y: oy, runs: runs, raster: raster, texts: texts.length, svg: svg.outerHTML };
  }
  window.recur = async function (op) {
    await document.fonts.ready;
    await Promise.all(Array.from(document.fonts).map(function (f) { return f.load().catch(function () {}); }));
    if (op.op === 'render') {
      mermaid.initialize(op.config);
      try {
        var out = await mermaid.render(op.id, op.source);
        return { svg: out.svg };
      } catch (e) {
        var stale = document.getElementById('d' + op.id);
        if (stale) stale.remove();
        return { error: (e && e.message) ? e.message : 'Mermaid render failed' };
      }
    }
    if (op.op === 'measure') return measure(op.svg);
    return { error: 'unknown op' };
  };
})();
"#;

/// A failed call (the platform has no webview, or the page did not answer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(target_os = "macos")]
pub use mac::{call, call_blocking};

#[cfg(not(target_os = "macos"))]
pub fn call(op: Value) -> futures::channel::oneshot::Receiver<Result<Value, EngineError>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let _ = op;
    let _ = tx.send(Err(EngineError(
        "Mermaid needs the system WebKit, which this platform build does not link".into(),
    )));
    rx
}

#[cfg(not(target_os = "macos"))]
pub fn call_blocking(op: Value) -> Result<Value, EngineError> {
    let _ = op;
    Err(EngineError(
        "Mermaid needs the system WebKit, which this platform build does not link".into(),
    ))
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use block2::RcBlock;
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
    use futures::channel::oneshot;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_foundation::{NSDictionary, NSError, NSString};
    use objc2_web_kit::{
        WKContentWorld, WKUserScript, WKUserScriptInjectionTime, WKWebView,
        WKWebViewConfiguration, WKWebsiteDataStore,
    };
    use serde_json::Value;

    use super::{EngineError, HOST_JS};

    const MERMAID_JS: &str = include_str!("../../assets/mermaid/mermaid.min.js");

    thread_local! {
        static VIEW: RefCell<Option<Retained<WKWebView>>> = const { RefCell::new(None) };
    }

    fn view(mtm: MainThreadMarker) -> Retained<WKWebView> {
        VIEW.with(|v| {
            if let Some(w) = v.borrow().as_ref() {
                return w.clone();
            }
            // SAFETY: WebKit objects are created and used on the main thread (`mtm`).
            let web = unsafe {
                let config = WKWebViewConfiguration::new(mtm);
                config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
                let ucc = config.userContentController();
                let source = NSString::from_str(&format!("{MERMAID_JS}\n{HOST_JS}"));
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &source,
                    WKUserScriptInjectionTime::AtDocumentEnd,
                    true,
                );
                ucc.addUserScript(&script);
                let frame = objc2_foundation::NSRect::new(
                    objc2_foundation::NSPoint::new(0., 0.),
                    objc2_foundation::NSSize::new(1024., 768.),
                );
                let web = WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config);
                web.loadHTMLString_baseURL(&NSString::from_str(&super::page()), None);
                web
            };
            *v.borrow_mut() = Some(web.clone());
            web
        })
    }

    fn mtm() -> Result<MainThreadMarker, EngineError> {
        MainThreadMarker::new().ok_or_else(|| EngineError("Mermaid renders on the main thread".into()))
    }

    fn loading(web: &WKWebView) -> bool {
        // SAFETY: a property read on the main thread.
        unsafe { web.isLoading() }
    }

    /// Mermaid's text measurement depends on the device scale (glyph advances round to device
    /// pixels), so the page renders at the scale of the display the diagram goes to.
    fn set_scale(web: &WKWebView, op: &Value) {
        let scale = op.get("scale").and_then(Value::as_f64).unwrap_or(1.);
        let sel = objc2::sel!(_setOverrideDeviceScaleFactor:);
        // SAFETY: a WebKit SPI with a CGFloat argument, called only when the class has it.
        unsafe {
            let responds: bool = objc2::msg_send![web, respondsToSelector: sel];
            if responds {
                let _: () = objc2::msg_send![web, _setOverrideDeviceScaleFactor: scale];
            }
        }
    }

    fn invoke(web: &WKWebView, mtm: MainThreadMarker, op: &Value, done: impl Fn(Result<Value, EngineError>) + 'static) {
        set_scale(web, op);
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
                None => done(Err(EngineError("Mermaid returned nothing".into()))),
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
    pub fn call(op: Value) -> oneshot::Receiver<Result<Value, EngineError>> {
        let (tx, rx) = oneshot::channel();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let finish = move |r: Result<Value, EngineError>| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(r);
            }
        };
        match mtm() {
            Ok(mtm) => {
                let web = view(mtm);
                invoke(&web, mtm, &op, finish);
            }
            Err(e) => finish(Err(e)),
        }
        rx
    }

    /// Run `op` and spin the main run loop until it answers (headless renders and tests,
    /// where no application loop runs).
    pub fn call_blocking(op: Value) -> Result<Value, EngineError> {
        let mtm = mtm()?;
        let web = view(mtm);
        let deadline = Instant::now() + Duration::from_secs(30);
        // SAFETY: a CoreFoundation constant.
        let mode = unsafe { kCFRunLoopDefaultMode };
        while loading(&web) && Instant::now() < deadline {
            CFRunLoop::run_in_mode(mode, Duration::from_millis(5), true);
        }
        let out: Rc<RefCell<Option<Result<Value, EngineError>>>> = Rc::default();
        let slot = out.clone();
        invoke(&web, mtm, &op, move |r| *slot.borrow_mut() = Some(r));
        while out.borrow().is_none() {
            if Instant::now() > deadline {
                return Err(EngineError("Mermaid timed out".into()));
            }
            CFRunLoop::run_in_mode(mode, Duration::from_millis(5), true);
        }
        out.borrow_mut()
            .take()
            .unwrap_or_else(|| Err(EngineError("Mermaid returned nothing".into())))
    }
}
