//! A browser page: any URL, localhost included, in a WKWebView laid behind GPUI's drawing as
//! the HTML preview's is (`html::mac`). It has its own non-persistent data store, and Recur's
//! own servers stay out of reach: a content rule list blocks every load (documents and
//! subresources, any host) to their ports, and the navigation delegate refuses main-frame
//! navigations to them before the list is consulted.
//!
//! No script message handler is registered. One added later goes in a client
//! `WKContentWorld`, never the page world.

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{
    Bounds, Context, EventEmitter, IntoElement, ParentElement, Pixels, Render, Styled, Task,
    Window, canvas, div,
};

/// The browser runs where WKWebView does. Linux needs the WPE page (`docs/linux-wpe.md`).
pub const AVAILABLE: bool = cfg!(target_os = "macos");

/// Why the last load showed no page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Nothing accepted the connection.
    Unreachable,
    /// A Recur server's port.
    Blocked,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowserState {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub error: Option<LoadError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserEvent {
    Changed,
    /// ⌘L while the page had the keyboard.
    FocusAddress,
}

/// What the page reports, on the main thread.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum Msg {
    Changed,
    Started,
    Failed { code: isize, message: String },
    Blocked,
    FocusAddress,
}

pub struct BrowserPage {
    #[cfg(target_os = "macos")]
    web: Option<mac::Page>,
    state: BrowserState,
    requested: String,
    _messages: Option<Task<()>>,
}

impl EventEmitter<BrowserEvent> for BrowserPage {}

impl BrowserPage {
    /// A page that never reaches `blocked_ports`.
    pub fn new(blocked_ports: &[u16], cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        let messages = cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.next().await {
                if this.update(cx, |p, cx| p.on_message(msg, cx)).is_err() {
                    break;
                }
            }
        });
        #[cfg(not(target_os = "macos"))]
        let _ = (blocked_ports, tx);
        BrowserPage {
            #[cfg(target_os = "macos")]
            web: mac::Page::new(blocked_ports, tx),
            state: BrowserState::default(),
            requested: String::new(),
            _messages: Some(messages),
        }
    }

    pub fn state(&self) -> &BrowserState {
        &self.state
    }

    /// The URL the last [`Self::navigate`] asked for.
    pub fn requested(&self) -> &str {
        &self.requested
    }

    pub fn navigate(&mut self, url: &str, cx: &mut Context<Self>) {
        self.requested = url.to_string();
        self.state.error = None;
        #[cfg(target_os = "macos")]
        if let Some(web) = &self.web {
            web.load(url);
        }
        cx.emit(BrowserEvent::Changed);
        cx.notify();
    }

    /// Loads the last requested URL again (after a failed load the page has none of its own).
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        let url = self.requested.clone();
        if !url.is_empty() {
            self.navigate(&url, cx);
        }
    }

    pub fn back(&mut self, _: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(web) = &self.web {
            web.go(-1);
        }
    }

    pub fn forward(&mut self, _: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(web) = &self.web {
            web.go(1);
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if self.state.error.is_some() || self.state.url.is_empty() {
            self.retry(cx);
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(web) = &self.web {
            web.go(0);
        }
    }

    pub fn stop(&mut self, _: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        if let Some(web) = &self.web {
            web.stop();
        }
    }

    /// Takes the keyboard back from the page so GPUI focus (the address bar) gets keys.
    pub fn release_keyboard(&self) {
        #[cfg(target_os = "macos")]
        if let Some(web) = &self.web {
            web.handle().release_keyboard();
        }
    }

    fn on_message(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::FocusAddress => {
                cx.emit(BrowserEvent::FocusAddress);
                return;
            }
            Msg::Started => self.state.error = None,
            // A navigation replaced by another one, or one the delegate refused.
            Msg::Failed {
                code: -999 | 102, ..
            } => {}
            Msg::Failed { code, message } => {
                self.state.error = Some(match code {
                    c if UNREACHABLE.contains(&c) => LoadError::Unreachable,
                    BLOCKED_BY_CONTENT_RULE => LoadError::Blocked,
                    _ => LoadError::Failed(message),
                });
            }
            Msg::Blocked => self.state.error = Some(LoadError::Blocked),
            Msg::Changed => {}
        }
        #[cfg(target_os = "macos")]
        if let Some(web) = &self.web {
            let error = self.state.error.take();
            self.state = BrowserState {
                error,
                ..web.state()
            };
        }
        cx.emit(BrowserEvent::Changed);
        cx.notify();
    }

    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> gpui_kit::AnyElement {
        #[cfg(target_os = "macos")]
        let page = self.web.as_ref().map(|p| p.handle());
        let place = canvas(
            |b, window, _| window.insert_hitbox(b, gpui_kit::HitboxBehavior::Native),
            move |b: Bounds<Pixels>, _, window, _| {
                #[cfg(target_os = "macos")]
                if let Some(page) = &page {
                    window.paint_hole(b);
                    crate::html::mac::place(page, b, window);
                }
                #[cfg(not(target_os = "macos"))]
                let _ = (b, window);
            },
        )
        .size_full();
        div()
            .flex_1()
            .min_h_0()
            .size_full()
            .bg(gpui_kit::white())
            .child(place)
            .into_any_element()
    }
}

/// `NSURLErrorCannotConnectToHost`, `NSURLErrorCannotFindHost`, `NSURLErrorTimedOut`,
/// `NSURLErrorNetworkConnectionLost`.
const UNREACHABLE: [isize; 4] = [-1004, -1003, -1001, -1005];

/// `WebKitErrorFrameLoadBlockedByContentBlocker`.
const BLOCKED_BY_CONTENT_RULE: isize = 104;

impl Render for BrowserPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

impl Drop for BrowserPage {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(p) = self.web.take() {
            p.remove();
        }
    }
}

/// Address-bar input as a URL to load: an explicit scheme is kept, loopback hosts and
/// `host:port` get `http://`, anything else host-like `https://`. Empty for input that is not
/// a URL.
pub fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return String::new();
    }
    let lower = s.to_ascii_lowercase();
    if has_scheme(s) || lower.starts_with("about:") || lower.starts_with("data:") {
        return s.to_string();
    }
    if s.chars().any(char::is_whitespace) {
        return String::new();
    }
    let auth = lower.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = auth.rsplit_once(':').is_some_and(|(h, p)| {
        !h.is_empty() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())
    });
    let host = match auth.rsplit_once(':') {
        Some((h, _)) if host_port => h,
        _ => auth,
    };
    if host_port || is_loopback_name(host) {
        format!("http://{s}")
    } else {
        format!("https://{s}")
    }
}

fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once("://") else {
        return false;
    };
    let mut b = scheme.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphabetic())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'))
}

fn is_loopback_name(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "0.0.0.0"
        || host == "[::1]"
        || host.strip_prefix("127.").is_some_and(|r| {
            r.split('.').count() == 3 && r.split('.').all(|o| o.parse::<u8>().is_ok())
        })
}

/// The URL names one of `ports`, on any host: a hostname that resolves to loopback reaches
/// Recur's servers as well as `127.0.0.1` does.
pub fn is_blocked(url: &str, ports: &[u16]) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.port_or_known_default())
        .is_some_and(|p| ports.contains(&p))
}

/// The content rule list blocking `ports`: one rule per port, as WebKit's `url-filter` has no
/// alternation. Every resource type, so fetches, images and sockets are blocked too.
pub fn content_rules(ports: &[u16]) -> String {
    let rules: Vec<serde_json::Value> = ports
        .iter()
        .map(|p| {
            serde_json::json!({
                "trigger": { "url-filter": format!("^[a-z][-a-z0-9+.]*://[^/]*:{p}[/?#]") },
                "action": { "type": "block" },
            })
        })
        .collect();
    serde_json::Value::Array(rules).to_string()
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) mod mac {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::rc::Rc;

    use futures::channel::mpsc::UnboundedSender;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{
        DefinedClass, MainThreadMarker, MainThreadOnly, Message as _, define_class, msg_send,
    };
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSResponder, NSView};
    use objc2_foundation::{
        NSError, NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration, NSPoint,
        NSRect, NSSize, NSString, NSURL, NSURLRequest,
    };
    use objc2_web_kit::{
        WKContentRuleList, WKContentRuleListStore, WKNavigation, WKNavigationAction,
        WKNavigationActionPolicy, WKNavigationDelegate, WKUIDelegate, WKWebView,
        WKWebViewConfiguration, WKWebsiteDataStore, WKWindowFeatures,
    };

    use super::{BrowserState, Msg};
    use crate::html::mac::Handle;

    const OBSERVED: [&str; 5] = ["URL", "title", "loading", "canGoBack", "canGoForward"];

    pub(crate) struct Ivars {
        tx: UnboundedSender<Msg>,
        ports: Vec<u16>,
    }

    impl Ivars {
        fn send(&self, msg: Msg) {
            let _ = self.tx.unbounded_send(msg);
        }
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and the class does not implement
        // Drop.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "RecurBrowserDelegate"]
        #[ivars = Ivars]
        pub(crate) struct Delegate;

        impl Delegate {
            #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
            fn observe(
                &self,
                _key: Option<&NSString>,
                _object: Option<&AnyObject>,
                _change: Option<&AnyObject>,
                _context: *mut c_void,
            ) {
                self.ivars().send(Msg::Changed);
            }
        }

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl WKNavigationDelegate for Delegate {
            #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
            fn decide(
                &self,
                _web: &WKWebView,
                action: &WKNavigationAction,
                handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
            ) {
                // SAFETY: plain property reads on the main thread.
                let url = unsafe { action.request().URL() }
                    .and_then(|u| u.absoluteString())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let main =
                    unsafe { action.targetFrame() }.is_none_or(|f| unsafe { f.isMainFrame() });
                let scheme = url.split(':').next().unwrap_or_default().to_ascii_lowercase();
                let blocked = super::is_blocked(&url, &self.ivars().ports);
                let ok = !blocked
                    && matches!(scheme.as_str(), "http" | "https" | "about" | "data" | "blob");
                if blocked && main {
                    self.ivars().send(Msg::Blocked);
                }
                handler.call((if ok {
                    WKNavigationActionPolicy::Allow
                } else {
                    WKNavigationActionPolicy::Cancel
                },));
            }

            #[unsafe(method(webView:didStartProvisionalNavigation:))]
            fn did_start(&self, _web: &WKWebView, _nav: Option<&WKNavigation>) {
                self.ivars().send(Msg::Started);
            }

            #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
            fn did_fail_provisional(
                &self,
                _web: &WKWebView,
                _nav: Option<&WKNavigation>,
                error: &NSError,
            ) {
                self.failed(error);
            }

            #[unsafe(method(webView:didFailNavigation:withError:))]
            fn did_fail(&self, _web: &WKWebView, _nav: Option<&WKNavigation>, error: &NSError) {
                self.failed(error);
            }
        }

        unsafe impl WKUIDelegate for Delegate {
            /// `target=_blank` and `window.open` load here instead of a new window.
            #[unsafe(method(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
            fn create_web_view(
                &self,
                web: &WKWebView,
                _config: &WKWebViewConfiguration,
                action: &WKNavigationAction,
                _features: &WKWindowFeatures,
            ) -> *mut WKWebView {
                // SAFETY: main thread.
                unsafe { web.loadRequest(&action.request()) };
                std::ptr::null_mut()
            }
        }
    );

    impl Delegate {
        fn new(ivars: Ivars, mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ivars);
            // SAFETY: NSObject's designated initializer.
            unsafe { msg_send![super(this), init] }
        }

        fn failed(&self, error: &NSError) {
            self.ivars().send(Msg::Failed {
                code: error.code(),
                message: error.localizedDescription().to_string(),
            });
        }
    }

    define_class!(
        // SAFETY: WKWebView may be subclassed; the class does not implement Drop.
        #[unsafe(super(WKWebView, NSView, NSResponder, NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "RecurBrowserWebView"]
        #[ivars = UnboundedSender<Msg>]
        pub(crate) struct WebView;

        impl WebView {
            /// ⌘L moves to the address bar even while the page has the keyboard: GPUI does
            /// not see keys then.
            #[unsafe(method(performKeyEquivalent:))]
            fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
                let mods = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
                let key = event
                    .charactersIgnoringModifiers()
                    .map(|s| s.to_string().to_lowercase());
                if mods == NSEventModifierFlags::Command && key.as_deref() == Some("l") {
                    let _ = self.ivars().unbounded_send(Msg::FocusAddress);
                    return Bool::YES;
                }
                // SAFETY: forwards to WKWebView's own handling.
                unsafe { msg_send![super(self), performKeyEquivalent: event] }
            }
        }
    );

    pub struct Page {
        view: Handle,
        delegate: Retained<Delegate>,
        /// The rule list is attached: loads may start.
        ready: Rc<Cell<bool>>,
        pending: Rc<RefCell<Option<String>>>,
    }

    impl Page {
        pub fn new(ports: &[u16], tx: UnboundedSender<Msg>) -> Option<Page> {
            let mtm = MainThreadMarker::new()?;
            let delegate = Delegate::new(
                Ivars {
                    tx: tx.clone(),
                    ports: ports.to_vec(),
                },
                mtm,
            );
            // SAFETY: WebKit objects created and used on the main thread.
            let web: Retained<WKWebView> = unsafe {
                let config = WKWebViewConfiguration::new(mtm);
                config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
                let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(10., 10.));
                let this = WebView::alloc(mtm).set_ivars(tx);
                let web: Retained<WebView> =
                    msg_send![super(this), initWithFrame: frame, configuration: &*config];
                let web = Retained::into_super(web);
                web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                web.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                web.setAllowsBackForwardNavigationGestures(true);
                web.setHidden(true);
                for key in OBSERVED {
                    web.addObserver_forKeyPath_options_context(
                        &delegate,
                        &NSString::from_str(key),
                        NSKeyValueObservingOptions::empty(),
                        std::ptr::null_mut(),
                    );
                }
                web
            };
            let page = Page {
                view: Handle::new(web),
                delegate,
                ready: Rc::new(Cell::new(ports.is_empty())),
                pending: Rc::default(),
            };
            if !ports.is_empty() {
                page.attach_rules(ports, mtm);
            }
            Some(page)
        }

        /// Compiles the rule list and attaches it, then starts the load asked for meanwhile.
        /// Until then nothing loads; a list that does not compile leaves the page blank.
        fn attach_rules(&self, ports: &[u16], mtm: MainThreadMarker) {
            // SAFETY: main thread.
            let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(mtm) }) else {
                return;
            };
            let (web, ready, pending) = (
                self.view.web().retain(),
                self.ready.clone(),
                self.pending.clone(),
            );
            let tx = self.delegate.ivars().tx.clone();
            let done =
                block2::RcBlock::new(move |list: *mut WKContentRuleList, error: *mut NSError| {
                    // SAFETY: WebKit hands a valid list or error, on the main thread.
                    let list = unsafe { list.as_ref() };
                    let Some(list) = list else {
                        let message = unsafe { error.as_ref() }
                            .map(|e| e.localizedDescription().to_string())
                            .unwrap_or_default();
                        let _ = tx.unbounded_send(Msg::Failed { code: 0, message });
                        return;
                    };
                    unsafe {
                        web.configuration()
                            .userContentController()
                            .addContentRuleList(list)
                    };
                    ready.set(true);
                    if let Some(url) = pending.borrow_mut().take() {
                        load_now(&web, &url);
                    }
                });
            // SAFETY: main thread; the block is retained by WebKit until it runs.
            unsafe {
                store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                    Some(&NSString::from_str("recur-browser-blocklist")),
                    Some(&NSString::from_str(&super::content_rules(ports))),
                    Some(&done),
                );
            }
        }

        pub fn load(&self, url: &str) {
            if self.ready.get() {
                load_now(self.view.web(), url);
            } else {
                *self.pending.borrow_mut() = Some(url.to_string());
            }
        }

        /// Back (`-1`), forward (`1`) or reload (`0`).
        pub fn go(&self, dir: i32) {
            let web = self.view.web();
            // SAFETY: main thread.
            unsafe {
                match dir {
                    d if d < 0 => drop(web.goBack()),
                    d if d > 0 => drop(web.goForward()),
                    _ => drop(web.reload()),
                }
            }
        }

        pub fn stop(&self) {
            // SAFETY: main thread.
            unsafe { self.view.web().stopLoading() };
        }

        pub fn state(&self) -> BrowserState {
            let web = self.view.web();
            // SAFETY: plain property reads on the main thread.
            unsafe {
                BrowserState {
                    url: web
                        .URL()
                        .and_then(|u| u.absoluteString())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    title: web.title().map(|s| s.to_string()).unwrap_or_default(),
                    loading: web.isLoading(),
                    can_go_back: web.canGoBack(),
                    can_go_forward: web.canGoForward(),
                    error: None,
                }
            }
        }

        pub fn handle(&self) -> Handle {
            self.view.clone()
        }

        pub fn remove(self) {
            let web = self.view.web();
            // SAFETY: main thread; the observers were added in `new`.
            unsafe {
                for key in OBSERVED {
                    web.removeObserver_forKeyPath(&self.delegate, &NSString::from_str(key));
                }
                web.stopLoading();
            }
            self.view.remove();
        }
    }

    fn load_now(web: &WKWebView, url: &str) {
        // SAFETY: main thread.
        unsafe {
            if let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) {
                web.loadRequest(&NSURLRequest::requestWithURL(&u));
            }
        }
    }
}
