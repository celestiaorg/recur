//! The HTML kind of `FilePreview.svelte`: agent HTML runs, as the sandboxed iframe let it, in
//! a WKWebView laid over the preview's box. The document is `htmlPreviewSrcdoc` (CSP first:
//! https/wss only, so the app and Recur's localhost servers are unreachable), the data store
//! is non-persistent, the main frame never navigates away and popups never open. The page's
//! `parent.postMessage` reaches the host (scroll, find results, find keys) and the host
//! answers by `message` events, as with the iframe.
//!
//! Native views sit above GPUI's drawing, so overlays do not cover the page (see
//! `ui-parity.md`).

#[cfg(target_os = "macos")]
use futures::StreamExt as _;
#[cfg(target_os = "macos")]
use futures::channel::mpsc;
use gpui_kit::{
    Bounds, Context, EventEmitter, IntoElement, ParentElement, Pixels, Render, Styled, Task,
    Window, canvas, div,
};
use recur_ui::Theme;
use serde_json::{Value, json};

use crate::preview::find::FindOptions;
use crate::preview::scroll::PageAttachment;
use crate::preview::view::SharedScroll;
use crate::preview::{
    PREVIEW_IFRAME_FIND, PREVIEW_IFRAME_FIND_KEY, PREVIEW_IFRAME_FIND_RESULT,
    PREVIEW_IFRAME_RESTORE, PREVIEW_IFRAME_SCROLL, html_preview_srcdoc,
};

/// Messages from the page.
#[derive(Clone, Debug, PartialEq)]
pub enum HtmlPreviewEvent {
    FindResult {
        total: usize,
        index: usize,
        valid: bool,
    },
    FindKey(String),
    Loaded,
}

/// `window.parent` for the page: its `postMessage` goes to the host.
pub const BRIDGE_JS: &str = r#"(function(){var post=function(d){try{window.webkit.messageHandlers.recur.postMessage(JSON.stringify(d))}catch(e){}};try{Object.defineProperty(window,'parent',{value:{postMessage:post},configurable:false})}catch(e){}})();"#;

pub struct HtmlPreview {
    path: String,
    scroll: SharedScroll,
    attachment: PageAttachment,
    parked: bool,
    #[cfg(target_os = "macos")]
    web: Option<mac::Page>,
    srcdoc: String,
    _messages: Option<Task<()>>,
}

impl EventEmitter<HtmlPreviewEvent> for HtmlPreview {}

impl HtmlPreview {
    pub fn new(
        html: &str,
        path: &str,
        scroll: SharedScroll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (attachment, _) = PageAttachment::attach(&scroll.borrow(), path, "");
        let mut p = HtmlPreview {
            path: path.to_string(),
            scroll,
            attachment,
            parked: false,
            #[cfg(target_os = "macos")]
            web: None,
            srcdoc: String::new(),
            _messages: None,
        };
        p.set_html(html, window, cx);
        p
    }

    /// New content: a fresh document (`srcdoc` changes reload the iframe).
    pub fn set_html(&mut self, html: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let doc = html_preview_srcdoc(html);
        if doc == self.srcdoc {
            return;
        }
        self.srcdoc = doc;
        #[cfg(target_os = "macos")]
        {
            let (tx, mut rx) = mpsc::unbounded::<String>();
            match &self.web {
                Some(page) => page.load(&self.srcdoc, tx),
                None => self.web = mac::Page::new(&self.srcdoc, tx),
            }
            self._messages = Some(cx.spawn(async move |this, cx| {
                while let Some(msg) = rx.next().await {
                    if this.update(cx, |p, cx| p.on_message(&msg, cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    pub fn set_parked(&mut self, parked: bool) {
        self.parked = parked;
    }

    fn post(&self, data: Value) {
        #[cfg(target_os = "macos")]
        if let Some(page) = &self.web {
            page.dispatch(&data);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = data;
    }

    fn restore(&mut self) {
        let (top, left) = self.attachment.restore(&self.scroll.borrow());
        self.post(json!({ "type": PREVIEW_IFRAME_RESTORE, "top": top, "left": left }));
    }

    fn on_message(&mut self, raw: &str, cx: &mut Context<Self>) {
        if raw == "\u{0}loaded" {
            // `iframe.onload`: restore, and let find re-query.
            self.restore();
            cx.emit(HtmlPreviewEvent::Loaded);
            return;
        }
        let Ok(d) = serde_json::from_str::<Value>(raw) else {
            return;
        };
        let num = |k: &str| d.get(k).and_then(Value::as_f64).unwrap_or(0.);
        match d.get("type").and_then(Value::as_str) {
            Some(t) if t == PREVIEW_IFRAME_SCROLL => {
                let (top, left) = (num("top"), num("left"));
                self.attachment
                    .on_scroll(&mut self.scroll.borrow_mut(), top, left, self.parked);
            }
            Some(t) if t == PREVIEW_IFRAME_FIND_RESULT => cx.emit(HtmlPreviewEvent::FindResult {
                total: num("total") as usize,
                index: num("index") as usize,
                valid: d.get("valid").and_then(Value::as_bool) != Some(false),
            }),
            Some(t) if t == PREVIEW_IFRAME_FIND_KEY => {
                if let Some(k) = d.get("key").and_then(Value::as_str) {
                    cx.emit(HtmlPreviewEvent::FindKey(k.to_string()));
                }
            }
            _ => {}
        }
    }

    pub fn find_query(&mut self, query: &str, opts: FindOptions, _: &mut Context<Self>) {
        self.post(json!({
            "type": PREVIEW_IFRAME_FIND, "op": "query", "search": query,
            "caseSensitive": opts.case_sensitive, "regexp": opts.regexp, "wholeWord": opts.whole_word,
        }));
    }

    pub fn find_step(&mut self, dir: i32, _: &mut Context<Self>) {
        self.post(
            json!({ "type": PREVIEW_IFRAME_FIND, "op": if dir > 0 { "next" } else { "prev" } }),
        );
    }

    pub fn find_clear(&mut self, _: &mut Context<Self>) {
        self.post(json!({ "type": PREVIEW_IFRAME_FIND, "op": "clear" }));
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Render for HtmlPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        #[cfg(target_os = "macos")]
        let page = self.web.as_ref().map(|p| p.handle());
        let place = canvas(
            |_, _, _| {},
            move |b: Bounds<Pixels>, _, window, _| {
                #[cfg(target_os = "macos")]
                if let Some(page) = &page {
                    mac::place(page, b, window);
                }
                #[cfg(not(target_os = "macos"))]
                let _ = (b, window);
            },
        )
        .size_full();
        // `.file-preview-html iframe { background: #fff }` until the page paints.
        let _ = t;
        div()
            .flex_1()
            .min_h_0()
            .size_full()
            .bg(gpui_kit::white())
            .child(place)
    }
}

impl Drop for HtmlPreview {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(p) = self.web.take() {
            p.remove();
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use futures::channel::mpsc::UnboundedSender;
    use gpui_kit::{Bounds, Pixels, Window};
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc2_web_kit::{
        WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
        WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKUserScript,
        WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use serde_json::Value;

    use super::BRIDGE_JS;

    pub struct Ivars {
        tx: RefCell<Option<UnboundedSender<String>>>,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and the class does not implement
        // Drop.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "RecurNextHtmlPreviewBridge"]
        #[ivars = Ivars]
        pub struct Bridge;

        unsafe impl NSObjectProtocol for Bridge {}

        unsafe impl WKScriptMessageHandler for Bridge {
            #[unsafe(method(userContentController:didReceiveScriptMessage:))]
            fn did_receive(&self, _ucc: &WKUserContentController, message: &WKScriptMessage) {
                // SAFETY: the bridge posts strings (`JSON.stringify`).
                let body = unsafe { message.body() };
                if let Some(s) = body.downcast_ref::<NSString>()
                    && let Some(tx) = self.ivars().tx.borrow().as_ref()
                {
                    let _ = tx.unbounded_send(s.to_string());
                }
            }
        }

        unsafe impl WKNavigationDelegate for Bridge {
            #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
            fn decide(
                &self,
                _web: &WKWebView,
                action: &WKNavigationAction,
                handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
            ) {
                // Subframes may load (CSP limits them to https); the preview itself never
                // navigates away from its document.
                // SAFETY: plain property reads on the main thread.
                let main =
                    unsafe { action.targetFrame() }.is_some_and(|f| unsafe { f.isMainFrame() });
                let url = unsafe { action.request().URL() }
                    .and_then(|u| u.absoluteString())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let ok = !main || url == "about:blank";
                handler.call((if ok {
                    WKNavigationActionPolicy::Allow
                } else {
                    WKNavigationActionPolicy::Cancel
                },));
            }

            #[unsafe(method(webView:didFinishNavigation:))]
            fn did_finish(&self, _web: &WKWebView, _nav: Option<&WKNavigation>) {
                if let Some(tx) = self.ivars().tx.borrow().as_ref() {
                    let _ = tx.unbounded_send("\u{0}loaded".into());
                }
            }
        }
    );

    impl Bridge {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(Ivars {
                tx: RefCell::new(None),
            });
            // SAFETY: NSObject's designated initializer.
            unsafe { msg_send![super(this), init] }
        }
    }

    pub struct Page {
        web: Retained<WKWebView>,
        bridge: Retained<Bridge>,
        attached: Rc<Cell<bool>>,
        painted: Rc<Cell<u64>>,
    }

    /// What the paint closure needs.
    #[derive(Clone)]
    pub struct Handle {
        web: Retained<WKWebView>,
        attached: Rc<Cell<bool>>,
        painted: Rc<Cell<u64>>,
    }

    thread_local! {
        static FRAME: Cell<u64> = const { Cell::new(0) };
    }

    impl Page {
        pub fn new(srcdoc: &str, tx: UnboundedSender<String>) -> Option<Page> {
            let mtm = MainThreadMarker::new()?;
            let bridge = Bridge::new(mtm);
            // SAFETY: WebKit objects created and used on the main thread.
            let web = unsafe {
                let config = WKWebViewConfiguration::new(mtm);
                config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
                let ucc = config.userContentController();
                ucc.addScriptMessageHandler_name(
                    ProtocolObject::from_ref(&*bridge),
                    &NSString::from_str("recur"),
                );
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &NSString::from_str(BRIDGE_JS),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                );
                ucc.addUserScript(&script);
                let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(10., 10.));
                let web =
                    WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config);
                web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*bridge)));
                web.setHidden(true);
                web
            };
            let page = Page {
                web,
                bridge,
                attached: Rc::new(Cell::new(false)),
                painted: Rc::new(Cell::new(0)),
            };
            page.load(srcdoc, tx);
            Some(page)
        }

        pub fn load(&self, srcdoc: &str, tx: UnboundedSender<String>) {
            *self.bridge.ivars().tx.borrow_mut() = Some(tx);
            // SAFETY: main thread.
            unsafe {
                self.web
                    .loadHTMLString_baseURL(&NSString::from_str(srcdoc), None);
            }
        }

        /// `iframe.contentWindow.postMessage(data, '*')`.
        pub fn dispatch(&self, data: &Value) {
            let js =
                format!("window.dispatchEvent(new MessageEvent('message', {{ data: {data} }}));");
            // SAFETY: main thread; no completion handler.
            unsafe {
                self.web
                    .evaluateJavaScript_completionHandler(&NSString::from_str(&js), None);
            }
        }

        pub fn handle(&self) -> Handle {
            Handle {
                web: self.web.clone(),
                attached: self.attached.clone(),
                painted: self.painted.clone(),
            }
        }

        pub fn remove(self) {
            *self.bridge.ivars().tx.borrow_mut() = None;
            self.web.removeFromSuperview();
        }
    }

    fn content_view(window: &Window) -> Option<Retained<NSView>> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return None;
        };
        // SAFETY: AppKit hands out the window's content NSView; retain it for our use.
        unsafe { Retained::retain(h.ns_view.as_ptr().cast::<NSView>()) }
    }

    /// Lay the page over `b` (window coordinates) and show it; hide it again on the next frame
    /// that does not paint it.
    pub fn place(page: &Handle, b: Bounds<Pixels>, window: &mut Window) {
        let Some(view) = content_view(window) else {
            return;
        };
        if !page.attached.get() {
            view.addSubview(&page.web);
            page.attached.set(true);
        }
        let h = view.frame().size.height;
        let (x, y, w, bh) = (
            f64::from(f32::from(b.origin.x)),
            f64::from(f32::from(b.origin.y)),
            f64::from(f32::from(b.size.width)),
            f64::from(f32::from(b.size.height)),
        );
        let y = if view.isFlipped() { y } else { h - y - bh };
        page.web
            .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(w, bh)));
        page.web.setHidden(false);
        let frame = FRAME.with(|f| {
            f.set(f.get() + 1);
            f.get()
        });
        page.painted.set(frame);
        let web = page.web.clone();
        let painted = page.painted.clone();
        window.on_next_frame(move |window, _| {
            let check = painted.clone();
            window.on_next_frame(move |_, _| {
                if check.get() == frame {
                    // Not painted since: the preview left the screen.
                    web.setHidden(true);
                }
            });
        });
    }
}
