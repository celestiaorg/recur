//! The HTML kind of `FilePreview.svelte`: agent HTML runs, as the sandboxed iframe let it, in
//! a WKWebView laid over the preview's box. The document is `htmlPreviewSrcdoc` (CSP first:
//! https/wss only, so the app and Recur's localhost servers are unreachable), the data store
//! is non-persistent, the main frame never navigates away and popups never open. The page's
//! `parent.postMessage` reaches the host (scroll, find results, find keys) and the host
//! answers by `message` events, as with the iframe.
//!
//! Native views sit above GPUI's drawing, so the page is masked wherever GPUI paints an overlay
//! (tooltips, menus, dialogs) over it.

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{
    Bounds, Context, EventEmitter, IntoElement, ParentElement, Pixels, Render, Styled, Task,
    Window, canvas, div,
};
#[cfg(not(target_os = "linux"))]
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

#[cfg(target_os = "linux")]
type Laid = Option<(Bounds<Pixels>, f32)>;

/// A `<select>` the page asked to pop up: its box (CSS px in the page), options and choice.
#[cfg(target_os = "linux")]
#[derive(serde::Deserialize)]
struct SelectMenu {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    index: i64,
    options: Vec<SelectOption>,
}

#[cfg(target_os = "linux")]
#[derive(serde::Deserialize)]
struct SelectOption {
    label: String,
    disabled: bool,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct HtmlPreview {
    path: String,
    scroll: SharedScroll,
    attachment: PageAttachment,
    parked: bool,
    #[cfg(target_os = "macos")]
    web: Option<mac::Page>,
    #[cfg(target_os = "linux")]
    gtk: Option<crate::html_gtk::Page>,
    /// The Linux page's latest snapshot, and images to release after the next draw.
    #[cfg(target_os = "linux")]
    frame: Option<std::sync::Arc<gpui_kit::RenderImage>>,
    #[cfg(target_os = "linux")]
    stale: Vec<std::sync::Arc<gpui_kit::RenderImage>>,
    /// The page's laid-out box and scale, to resize it when the box changes.
    #[cfg(target_os = "linux")]
    laid: std::rc::Rc<std::cell::Cell<Laid>>,
    #[cfg(target_os = "linux")]
    cursor: gpui_kit::CursorStyle,
    #[cfg(target_os = "linux")]
    focus: gpui_kit::FocusHandle,
    /// A `<select>`'s open menu: the page answered a press on it (`html_gtk::INPUT_JS`).
    #[cfg(target_os = "linux")]
    select: Option<SelectMenu>,
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
            #[cfg(target_os = "linux")]
            gtk: None,
            #[cfg(target_os = "linux")]
            frame: None,
            #[cfg(target_os = "linux")]
            stale: Vec::new(),
            #[cfg(target_os = "linux")]
            laid: Default::default(),
            #[cfg(target_os = "linux")]
            cursor: gpui_kit::CursorStyle::Arrow,
            #[cfg(target_os = "linux")]
            focus: cx.focus_handle(),
            #[cfg(target_os = "linux")]
            select: None,
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
        #[cfg(target_os = "linux")]
        {
            use crate::html_gtk::{Page, PageEvent};
            let (tx, mut rx) = mpsc::unbounded::<PageEvent>();
            match &self.gtk {
                Some(page) => page.load(&self.srcdoc, tx),
                None => {
                    self.gtk = Page::new(&self.srcdoc, tx);
                    self.laid.set(None);
                }
            }
            self._messages = Some(cx.spawn(async move |this, cx| {
                while let Some(ev) = rx.next().await {
                    if this.update(cx, |p, cx| p.on_page_event(ev, cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    pub fn set_parked(&mut self, parked: bool) {
        self.parked = parked;
        #[cfg(target_os = "linux")]
        if let Some(page) = &self.gtk {
            page.set_active(!parked);
        }
    }

    #[cfg(target_os = "linux")]
    fn on_page_event(&mut self, ev: crate::html_gtk::PageEvent, cx: &mut Context<Self>) {
        use crate::html_gtk::PageEvent;
        match ev {
            PageEvent::Frame {
                width,
                height,
                bgra,
            } => {
                if let Some(buf) = image::RgbaImage::from_raw(width, height, bgra) {
                    let img = std::sync::Arc::new(gpui_kit::RenderImage::new(smallvec::smallvec![
                        image::Frame::new(buf)
                    ]));
                    if let Some(old) = self.frame.replace(img) {
                        self.stale.push(old);
                    }
                    cx.notify();
                }
            }
            PageEvent::Message(msg) => self.on_message(&msg, cx),
            PageEvent::Reply(Value::String(s)) if s.starts_with('\u{1}') => {
                // A copy: the page's selection.
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(s[1..].to_string()));
            }
            PageEvent::Reply(Value::String(cursor)) => {
                let next = match cursor.as_str() {
                    "pointer" => gpui_kit::CursorStyle::PointingHand,
                    "text" => gpui_kit::CursorStyle::IBeam,
                    "grab" => gpui_kit::CursorStyle::OpenHand,
                    "grabbing" => gpui_kit::CursorStyle::ClosedHand,
                    "crosshair" => gpui_kit::CursorStyle::Crosshair,
                    "not-allowed" => gpui_kit::CursorStyle::OperationNotAllowed,
                    "zoom-in" => gpui_kit::CursorStyle::ZoomIn,
                    "zoom-out" => gpui_kit::CursorStyle::ZoomOut,
                    _ => gpui_kit::CursorStyle::Arrow,
                };
                if next != self.cursor {
                    self.cursor = next;
                    cx.notify();
                }
            }
            PageEvent::Reply(v) => {
                if let Some(menu) = v
                    .get("select")
                    .and_then(|m| serde_json::from_value::<SelectMenu>(m.clone()).ok())
                {
                    self.select = Some(menu);
                    cx.notify();
                }
            }
        }
    }

    /// Closes the `<select>` menu, choosing `index` (`None` keeps the value).
    #[cfg(target_os = "linux")]
    fn pick(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        if self.select.take().is_some() {
            let index = index.map_or(-1, |i| i as i64);
            self.send(
                json!({"type": "pick", "index": index}),
                None,
                &Default::default(),
            );
            cx.notify();
        }
    }

    /// The `<select>`'s options in a native menu under it, as WebKit pops one up.
    #[cfg(target_os = "linux")]
    fn select_menu(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        use gpui_kit::prelude::FluentBuilder as _;
        use gpui_kit::{InteractiveElement as _, StatefulInteractiveElement as _, px};
        use recur_ui::guard::Guarded as _;
        use recur_ui::{TextStyled as _, Theme};
        let (menu, (b, _)) = (self.select.as_ref()?, self.laid.get()?);
        let t = Theme::of(cx).tokens;
        let rows = menu.options.iter().enumerate().map(|(i, o)| {
            let on = i as i64 == menu.index;
            let row = div()
                .id(("html-select-option", i))
                .debug_selector(move || format!("html-select-option:{i}"))
                .px(px(10.))
                .py(px(5.))
                .rounded(px(5.))
                .when(on, |d| d.bg(t.pane))
                .child(
                    recur_ui::text(o.label.clone())
                        .size(12.)
                        .color(if o.disabled { t.faint } else { t.text })
                        .nowrap(),
                );
            if o.disabled {
                row
            } else {
                row.cursor_pointer()
                    .hover(move |s| s.bg(t.pane))
                    .on_click(cx.guarded_listener(move |p, _, _, cx| p.pick(Some(i), cx)))
            }
        });
        let panel = recur_ui::style::menu_panel(cx)
            .id("html-select-menu")
            .debug_selector(|| "html-select-menu".into())
            .min_w(px(menu.w.max(80.)))
            .max_h(px(320.))
            .overflow_y_scroll()
            .occlude()
            .children(rows);
        let at = recur_ui::style::anchored_at(
            f32::from(b.left()) + menu.x,
            f32::from(b.top()) + menu.y + menu.h + 2.,
            panel,
        );
        Some(
            gpui_kit::deferred(
                div()
                    .id("html-select-scrim")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.guarded_listener(|p, _, _, cx| p.pick(None, cx)),
                    )
                    .child(at),
            )
            .with_priority(210)
            .into_any_element(),
        )
    }

    fn post(&self, data: Value) {
        #[cfg(target_os = "macos")]
        if let Some(page) = &self.web {
            page.dispatch(&data);
        }
        #[cfg(target_os = "linux")]
        if let Some(page) = &self.gtk {
            page.dispatch(&data);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
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

#[cfg(target_os = "linux")]
impl HtmlPreview {
    /// One event for the page's input shim, at `pos` in window coordinates.
    fn send(&self, mut ev: Value, pos: Option<gpui_kit::Point<Pixels>>, m: &gpui_kit::Modifiers) {
        let (Some(page), Some((b, _))) = (&self.gtk, self.laid.get()) else {
            return;
        };
        if let Some(p) = pos {
            ev["x"] = json!(f32::from(p.x - b.left()));
            ev["y"] = json!(f32::from(p.y - b.top()));
        }
        ev["ctrl"] = json!(m.control);
        ev["shift"] = json!(m.shift);
        ev["alt"] = json!(m.alt);
        ev["meta"] = json!(m.platform);
        page.input(&ev);
    }

    fn render_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        use gpui_kit::{
            InteractiveElement as _, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
            MouseUpEvent, ScrollWheelEvent, StatefulInteractiveElement as _,
        };
        use recur_ui::guard::Guarded as _;
        for img in self.stale.drain(..) {
            let _ = window.drop_image(img);
        }
        if let Some(page) = &self.gtk {
            page.set_active(!self.parked);
        }
        let laid = self.laid.clone();
        let entity = cx.entity().downgrade();
        let frame = self.frame.clone();
        let place = canvas(
            move |b, window, cx| {
                let scale = window.scale_factor();
                if laid.get() != Some((b, scale)) {
                    laid.set(Some((b, scale)));
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |p: &mut HtmlPreview, _| {
                            if let Some(page) = &p.gtk {
                                page.resize(
                                    f32::from(b.size.width),
                                    f32::from(b.size.height),
                                    scale,
                                );
                            }
                        });
                    }
                }
            },
            move |b, _, window, _| {
                if let Some(img) = frame.clone() {
                    let _ = window.paint_image(b, b, gpui_kit::Corners::default(), img, 0, false);
                }
            },
        )
        .size_full();
        let button = |b: MouseButton| match b {
            MouseButton::Right => 2,
            MouseButton::Middle => 1,
            _ => 0,
        };
        let select = self.select_menu(cx);
        div()
            .id("html-preview")
            .relative()
            .debug_selector(|| "html-preview".into())
            .track_focus(&self.focus)
            .key_context("TextEntry")
            .flex_1()
            .min_h_0()
            .size_full()
            .bg(gpui_kit::white())
            .cursor(self.cursor)
            .child(place)
            .children(select)
            .on_hover(cx.guarded_listener(|p, hovered: &bool, _, _| {
                if !*hovered {
                    p.send(json!({"type": "leave"}), None, &Default::default());
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(move |p, e: &MouseDownEvent, window, cx| {
                    window.focus(&p.focus, cx);
                    p.send(
                        json!({"type": "down", "button": button(e.button), "detail": e.click_count}),
                        Some(e.position),
                        &e.modifiers,
                    );
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.guarded_listener(move |p, e: &MouseUpEvent, _, _| {
                    p.send(
                        json!({"type": "up", "button": button(e.button), "detail": e.click_count}),
                        Some(e.position),
                        &e.modifiers,
                    );
                }),
            )
            .on_mouse_move(cx.guarded_listener(|p, e: &MouseMoveEvent, _, _| {
                let buttons = u8::from(e.pressed_button == Some(MouseButton::Left));
                p.send(
                    json!({"type": "move", "buttons": buttons}),
                    Some(e.position),
                    &e.modifiers,
                );
            }))
            .on_scroll_wheel(cx.guarded_listener(|p, e: &ScrollWheelEvent, window, _| {
                let d = e.delta.pixel_delta(window.line_height());
                p.send(
                    json!({"type": "wheel", "dx": -f32::from(d.x), "dy": -f32::from(d.y)}),
                    Some(e.position),
                    &e.modifiers,
                );
            }))
            .on_key_down(cx.guarded_listener(|p, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if p.select.is_some() {
                    if k.key == "escape" {
                        p.pick(None, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                let cmd = k.modifiers.secondary();
                let key = match k.key.as_str() {
                    "space" => " ".to_string(),
                    "enter" => "Enter".into(),
                    "backspace" => "Backspace".into(),
                    "delete" => "Delete".into(),
                    "tab" => "Tab".into(),
                    "escape" => "Escape".into(),
                    "up" => "ArrowUp".into(),
                    "down" => "ArrowDown".into(),
                    "left" => "ArrowLeft".into(),
                    "right" => "ArrowRight".into(),
                    "pageup" => "PageUp".into(),
                    "pagedown" => "PageDown".into(),
                    "home" => "Home".into(),
                    "end" => "End".into(),
                    other => k.key_char.clone().unwrap_or_else(|| other.to_string()),
                };
                let mut ev = json!({"type": "key", "key": key});
                if cmd && k.key == "c" {
                    ev["copy"] = json!(true);
                } else if cmd && k.key == "a" {
                    ev["selectAll"] = json!(true);
                } else if cmd && k.key == "v" {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|c| c.text())
                        .unwrap_or_default();
                    ev["paste"] = json!(text);
                } else if cmd {
                    return;
                }
                p.send(ev, None, &k.modifiers);
                cx.stop_propagation();
            }))
            .into_any_element()
    }
}

impl HtmlPreview {
    #[cfg(target_os = "linux")]
    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        self.render_page(window, cx)
    }

    #[cfg(not(target_os = "linux"))]
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
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
            .into_any_element()
    }
}

impl Render for HtmlPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
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
    use gpui_kit::{Bounds, EntityId, Pixels, Window};
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
    use objc2_app_kit::{NSResponder, NSView};
    use objc2_core_graphics::CGMutablePath;
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use objc2_quartz_core::{CAShapeLayer, CATransaction};
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
        // SAFETY: WKWebView may be subclassed; the class does not implement Drop.
        #[unsafe(super(WKWebView, NSView, NSResponder, NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "RecurNextHtmlPreviewWebView"]
        #[ivars = RefCell<Vec<Rect>>]
        pub struct WebView;

        impl WebView {
            /// Clicks in a hole reach the overlay GPUI drew there.
            #[unsafe(method_id(hitTest:))]
            fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
                // SAFETY: main thread; the superview is retained for the call.
                let superview = unsafe { self.superview() };
                let local = self.convertPoint_fromView(point, superview.as_deref());
                let in_hole = self.ivars().borrow().iter().any(|h| {
                    local.x >= h[0] && local.x < h[2] && local.y >= h[1] && local.y < h[3]
                });
                if in_hole {
                    None
                } else {
                    // SAFETY: NSView's hitTest:, with the point it was given.
                    unsafe { msg_send![super(self), hitTest: point] }
                }
            }
        }
    );

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
        web: Retained<WebView>,
        bridge: Retained<Bridge>,
        attached: Rc<Cell<bool>>,
        painted: Rc<Cell<u64>>,
        mask: Rc<RefCell<Mask>>,
    }

    /// What the paint closure needs.
    #[derive(Clone)]
    pub struct Handle {
        web: Retained<WebView>,
        attached: Rc<Cell<bool>>,
        painted: Rc<Cell<u64>>,
        mask: Rc<RefCell<Mask>>,
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
                let this = WebView::alloc(mtm).set_ivars(RefCell::default());
                let web: Retained<WebView> =
                    msg_send![super(this), initWithFrame: frame, configuration: &*config];
                web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*bridge)));
                web.setHidden(true);
                web
            };
            let page = Page {
                web,
                bridge,
                attached: Rc::new(Cell::new(false)),
                painted: Rc::new(Cell::new(0)),
                mask: Rc::default(),
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
                mask: self.mask.clone(),
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

    /// Lay the page over `b` (window coordinates) and show it; hide it again after the first
    /// draw that neither paints it nor replays a view around it.
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
        let stack: Rc<[EntityId]> = window.view_stack().into();
        track(page.clone(), b, frame, stack, true, window);
    }

    /// At the end of each draw the page is up in: cut what GPUI painted over it out of the page,
    /// since the native view sits above GPUI's drawing. Hide the page after the first draw that
    /// neither paints it nor replays a view around it.
    fn track(
        page: Handle,
        b: Bounds<Pixels>,
        frame: u64,
        stack: Rc<[EntityId]>,
        painted_now: bool,
        window: &mut Window,
    ) {
        window.after_next_draw(move |window, _| {
            if page.painted.get() != frame {
                return;
            }
            if painted_now || stack.iter().any(|v| window.replayed_view(*v)) {
                set_mask(&page, holes(b, &window.overlay_bounds()));
                track(page, b, frame, stack, false, window);
            } else {
                page.web.setHidden(true);
            }
        });
    }

    /// The page's size and the overlays cut out of it, in page points (y down).
    #[derive(Default, PartialEq)]
    pub struct Mask {
        size: (f64, f64),
        holes: Vec<Rect>,
    }

    /// `[left, top, right, bottom]`.
    type Rect = [f64; 4];

    fn holes(b: Bounds<Pixels>, overlays: &[Bounds<Pixels>]) -> Mask {
        let (x, y) = (
            f64::from(f32::from(b.origin.x)),
            f64::from(f32::from(b.origin.y)),
        );
        let (w, h) = (
            f64::from(f32::from(b.size.width)),
            f64::from(f32::from(b.size.height)),
        );
        let holes = overlays
            .iter()
            .filter_map(|o| {
                let left = f64::from(f32::from(o.origin.x)) - x;
                let top = f64::from(f32::from(o.origin.y)) - y;
                let hole = [
                    left.max(0.),
                    top.max(0.),
                    (left + f64::from(f32::from(o.size.width))).min(w),
                    (top + f64::from(f32::from(o.size.height))).min(h),
                ];
                (hole[0] < hole[2] && hole[1] < hole[3]).then_some(hole)
            })
            .collect();
        Mask {
            size: (w, h),
            holes,
        }
    }

    /// `[0, 0, w, h]` less the holes, as disjoint rects.
    fn visible(mask: &Mask) -> Vec<Rect> {
        let mut rects = vec![[0., 0., mask.size.0, mask.size.1]];
        for h in &mask.holes {
            rects = rects
                .into_iter()
                .flat_map(|r| {
                    if h[0] >= r[2] || h[2] <= r[0] || h[1] >= r[3] || h[3] <= r[1] {
                        return vec![r];
                    }
                    let (top, bottom) = (h[1].max(r[1]), h[3].min(r[3]));
                    [
                        [r[0], r[1], r[2], top],
                        [r[0], bottom, r[2], r[3]],
                        [r[0], top, h[0].max(r[0]), bottom],
                        [h[2].min(r[2]), top, r[2], bottom],
                    ]
                    .into_iter()
                    .filter(|p| p[0] < p[2] && p[1] < p[3])
                    .collect()
                })
                .collect();
        }
        rects
    }

    fn set_mask(page: &Handle, mask: Mask) {
        if *page.mask.borrow() == mask {
            return;
        }
        page.web.setWantsLayer(true);
        let Some(layer) = page.web.layer() else {
            return;
        };
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if mask.holes.is_empty() {
            // SAFETY: main thread.
            unsafe { layer.setMask(None) };
        } else {
            let path = CGMutablePath::new();
            for [l, t, r, b] in visible(&mask) {
                let rect = NSRect::new(NSPoint::new(l, t), NSSize::new(r - l, b - t));
                // SAFETY: a null transform is allowed.
                unsafe { CGMutablePath::add_rect(Some(&path), std::ptr::null(), rect) };
            }
            let shape = CAShapeLayer::new();
            shape.setFrame(NSRect::new(
                NSPoint::new(0., 0.),
                NSSize::new(mask.size.0, mask.size.1),
            ));
            shape.setPath(Some(&path));
            // SAFETY: main thread; the shape layer has no superlayer. The WKWebView is flipped,
            // so its layer and the mask are y down, as the rects are.
            unsafe { layer.setMask(Some(&shape)) };
        }
        CATransaction::commit();
        page.web.ivars().replace(mask.holes.clone());
        *page.mask.borrow_mut() = mask;
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use gpui_kit::{point, px, size};

        fn area(rects: &[Rect]) -> f64 {
            rects.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum()
        }

        #[test]
        fn overlapping_overlays_cut_their_union_out_of_the_page() {
            let page = Bounds::new(point(px(100.), px(50.)), size(px(200.), px(100.)));
            let tooltip = Bounds::new(point(px(80.), px(40.)), size(px(60.), px(30.)));
            let menu = Bounds::new(point(px(120.), px(60.)), size(px(40.), px(40.)));
            let away = Bounds::new(point(px(0.), px(0.)), size(px(50.), px(50.)));
            let mask = holes(page, &[tooltip, menu, away]);
            assert_eq!(mask.holes, vec![[0., 0., 40., 20.], [20., 10., 60., 50.]]);
            let shown = visible(&mask);
            // 200x100 less 40x20 and 40x40, which share 20x10.
            assert_eq!(area(&shown), 20_000. - 800. - 1_600. + 200.);
            for (i, a) in shown.iter().enumerate() {
                for b in &shown[i + 1..] {
                    assert!(a[0] >= b[2] || b[0] >= a[2] || a[1] >= b[3] || b[1] >= a[3]);
                }
            }
        }
    }
}
