//! The HTML preview's real WebKit: on Linux the offscreen WebKitGTK page (Linux needs a
//! display, CI runs this under `xvfb-run`, and skips without one), on macOS the WKWebView behind
//! a GPUI window's drawing.

fn main() {
    // nextest lists a custom harness's tests first; this binary is the one test `webkit`.
    if std::env::args().any(|a| a == "--list") {
        if !std::env::args().any(|a| a == "--ignored") {
            println!("webkit: test");
        }
        return;
    }
    let no_display = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .all(|v| std::env::var_os(v).is_none_or(|s| s.is_empty()));
    if cfg!(target_os = "linux") && no_display {
        println!("webkit: skipped (no display)");
        return;
    }
    #[cfg(target_os = "linux")]
    html_page();
    #[cfg(target_os = "linux")]
    html_hover_and_select();
    #[cfg(target_os = "macos")]
    mac::html_preview();
    println!("webkit: ok");
}

/// The macOS HTML preview in a real window: the WKWebView sits behind GPUI's view, which shows
/// it through a transparent hole and passes mouse events there to it, while a GPUI overlay
/// (with its shadow) draws over the page and keeps its own clicks.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use gpui_kit::{
        AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity,
        InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render, Styled as _,
        Window, WindowBounds, WindowOptions, div, point, prelude::FluentBuilder as _, px, size,
    };
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObjectProtocol as _};
    use objc2::{ClassType as _, MainThreadMarker, msg_send};
    use objc2_app_kit::{
        NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSFloatingWindowLevel, NSView,
    };
    use objc2_foundation::{NSPoint, NSString};
    use objc2_web_kit::WKWebView;
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use recur_content::preview::scroll::PreviewScrollStore;
    use recur_content::{HtmlPreview, HtmlPreviewEvent};

    const W: f32 = 400.;
    const H: f32 = 300.;

    /// Signals come back as find keys, the one page message the preview surfaces as text.
    const DOC: &str = r#"<style>body{margin:0;background:#f00;height:100vh}</style>
<input id=i style="position:absolute;left:10px;top:10px;width:120px;height:20px"
  onfocus="say('focused')" oninput="say('typed:'+this.value)">
<script>function say(k){parent.postMessage({type:'recur-preview-find-key',key:k},'*')}
document.addEventListener('mousedown',function(e){say('down:'+e.clientX+','+e.clientY)})</script>"#;

    /// The overlay, in window points: a box with a shadow over the page.
    const OVERLAY: (f32, f32, f32) = (200., 100., 60.);

    struct Root {
        preview: Option<Entity<HtmlPreview>>,
        overlay_clicks: Rc<Cell<u32>>,
        scrim: bool,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.overlay_clicks.clone();
            let (x, y, side) = OVERLAY;
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(gpui_kit::blue())
                .children(self.preview.clone())
                .child(
                    gpui_kit::deferred(
                        div()
                            .id("overlay")
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .size(px(side))
                            .bg(gpui_kit::black())
                            .shadow_lg()
                            .occlude()
                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                clicks.set(clicks.get() + 1)
                            }),
                    )
                    .with_priority(1),
                )
                .when(self.scrim, |d| {
                    d.child(recur_ui::style::backdrop_blur(
                        8.,
                        recur_ui::color::rgba(0, 0, 0, 0.),
                    ))
                })
        }
    }

    pub fn html_preview() {
        gpui_kit::application()
            .with_assets(recur_ui::Assets)
            .run(|cx: &mut App| {
                recur_ui::init(cx);
                let keys = Rc::new(RefCell::new(Vec::<String>::new()));
                let mut parts = None;
                let window = cx
                    .open_window(
                        WindowOptions {
                            window_bounds: Some(WindowBounds::Windowed(Bounds {
                                origin: point(px(80.), px(80.)),
                                size: size(px(W), px(H)),
                            })),
                            focus: false,
                            ..Default::default()
                        },
                        |window, cx| {
                            let scroll = Rc::new(RefCell::new(PreviewScrollStore::new()));
                            let preview =
                                cx.new(|cx| HtmlPreview::new(DOC, "t.html", scroll, window, cx));
                            let k = keys.clone();
                            cx.subscribe(&preview, move |_, ev: &HtmlPreviewEvent, _| match ev {
                                HtmlPreviewEvent::FindKey(key) => k.borrow_mut().push(key.clone()),
                                HtmlPreviewEvent::Loaded => k.borrow_mut().push("loaded".into()),
                                _ => {}
                            })
                            .detach();
                            let clicks = Rc::new(Cell::new(0));
                            let root = cx.new(|_| Root {
                                preview: Some(preview.clone()),
                                overlay_clicks: clicks.clone(),
                                scrim: false,
                            });
                            parts = Some((root.clone(), preview, clicks));
                            root
                        },
                    )
                    .expect("open window");
                let (root, preview, clicks) = parts.expect("parts");
                let window: AnyWindowHandle = window.into();
                cx.spawn(async move |cx: &mut AsyncApp| {
                    let result = run(window, &root, &preview, &clicks, &keys, cx).await;
                    match result {
                        Ok(()) => {
                            println!("html preview: ok");
                            println!("webkit: ok");
                            std::process::exit(0);
                        }
                        Err(e) => {
                            eprintln!("html preview: FAIL: {e}");
                            std::process::exit(1);
                        }
                    }
                })
                .detach();
            });
    }

    /// Frames the platform draws and presents, as the app's own.
    async fn frames(cx: &mut AsyncApp, window: AnyWindowHandle, ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
            let _ = cx.update_window(window, |_, window, _| window.refresh());
        }
    }

    async fn until(
        cx: &mut AsyncApp,
        window: AnyWindowHandle,
        what: &str,
        mut f: impl FnMut() -> bool,
    ) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f() {
            if Instant::now() > deadline {
                return Err(format!("timed out waiting for {what}"));
            }
            frames(cx, window, 50).await;
        }
        Ok(())
    }

    struct Views {
        gpui: Retained<NSView>,
        superview: Retained<NSView>,
        web: Option<Retained<NSView>>,
    }

    fn views(cx: &mut AsyncApp, window: AnyWindowHandle) -> Views {
        cx.update_window(window, |_, window, _| {
            let handle = window.window_handle().expect("window handle");
            let RawWindowHandle::AppKit(h) = handle.as_raw() else {
                unreachable!()
            };
            // SAFETY: GPUI's NSView, alive while the window is.
            let gpui = unsafe { Retained::retain(h.ns_view.as_ptr().cast::<NSView>()) }
                .expect("GPUI view");
            // SAFETY: main thread.
            let superview = unsafe { gpui.superview() }.expect("superview");
            let subviews = superview.subviews();
            let index = |v: &NSView| subviews.iter().position(|s| &*s == v);
            let web = subviews
                .iter()
                .find(|v| v.isKindOfClass(WKWebView::class()))
                .map(|v| (index(&v), v));
            if let Some((i, _)) = &web {
                assert!(*i < index(&gpui), "the page is behind GPUI's view");
            }
            Views {
                gpui,
                superview,
                web: web.map(|(_, v)| v),
            }
        })
        .expect("window")
    }

    /// `p` in window points (y down) to the content view's coordinates.
    fn in_superview(v: &Views, x: f32, y: f32) -> NSPoint {
        let h = v.superview.frame().size.height;
        let y = f64::from(y);
        NSPoint::new(
            f64::from(x),
            if v.superview.isFlipped() { y } else { h - y },
        )
    }

    fn hit(v: &Views, x: f32, y: f32) -> Option<Retained<NSView>> {
        v.superview.hitTest(in_superview(v, x, y))
    }

    fn click(v: &Views, x: f32, y: f32) {
        let w = v.gpui.window().expect("NSWindow");
        let at = v.superview.convertPoint_toView(in_superview(v, x, y), None);
        for kind in [NSEventType::LeftMouseDown, NSEventType::LeftMouseUp] {
            let ev = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind,
                at,
                NSEventModifierFlags::empty(),
                0.,
                w.windowNumber(),
                None,
                0,
                1,
                1.,
            )
            .expect("mouse event");
            w.sendEvent(&ev);
        }
    }

    fn key(v: &Views, c: &str, code: u16) {
        let w = v.gpui.window().expect("NSWindow");
        let s = NSString::from_str(c);
        for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
            let ev = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
                kind,
                NSPoint::new(0., 0.),
                NSEventModifierFlags::empty(),
                0.,
                w.windowNumber(),
                None,
                &s,
                &s,
                false,
                code,
            )
            .expect("key event");
            w.sendEvent(&ev);
        }
    }

    fn first_responder_in(v: &Views, view: &NSView) -> bool {
        let w = v.gpui.window().expect("NSWindow");
        w.firstResponder().is_some_and(|r| {
            r.downcast_ref::<NSView>()
                .is_some_and(|r| r.isDescendantOf(view))
        })
    }

    fn layer_opaque(v: &Views) -> bool {
        // SAFETY: GPUI's view is layer-backed; `isOpaque` is a plain property read.
        unsafe {
            let layer: *mut AnyObject = msg_send![&*v.gpui, layer];
            msg_send![layer, isOpaque]
        }
    }

    /// The window's own pixels as (r, g, b, a) at window points, or `None` where the OS does not
    /// let the app capture its window.
    fn capture(v: &Views) -> Option<impl Fn(f32, f32) -> [u8; 4]> {
        use core_graphics::geometry::{CGPoint, CGRect, CGSize};
        use core_graphics::window::{
            create_image, kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming,
            kCGWindowListOptionIncludingWindow,
        };
        let w = v.gpui.window()?;
        let null = CGRect::new(
            &CGPoint::new(f64::INFINITY, f64::INFINITY),
            &CGSize::new(0., 0.),
        );
        let image = create_image(
            null,
            kCGWindowListOptionIncludingWindow,
            u32::try_from(w.windowNumber()).ok()?,
            kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution,
        )?;
        if image.bits_per_pixel() != 32 || image.width() == 0 {
            return None;
        }
        let (width, row) = (image.width(), image.bytes_per_row());
        let scale = width as f32 / W;
        // The image has the titlebar above the content.
        let top = image.height() as f32 / scale - H;
        let data = image.data().bytes().to_vec();
        Some(move |x: f32, y: f32| {
            let i = ((y + top) * scale) as usize * row + (x * scale) as usize * 4;
            // BGRA, premultiplied.
            [data[i + 2], data[i + 1], data[i], data[i + 3]]
        })
    }

    async fn run(
        window: AnyWindowHandle,
        root: &Entity<Root>,
        preview: &Entity<HtmlPreview>,
        clicks: &Rc<Cell<u32>>,
        keys: &Rc<RefCell<Vec<String>>>,
        cx: &mut AsyncApp,
    ) -> Result<(), String> {
        // An occluded window draws no frames (a local run under other windows): float it above
        // them without activating the app.
        let w = views(cx, window).gpui.window().ok_or("no NSWindow")?;
        w.setLevel(NSFloatingWindowLevel);
        w.orderFrontRegardless();
        let has = |k: &str| keys.borrow().iter().any(|s| s == k);
        until(cx, window, "the page to load", || has("loaded")).await?;
        frames(cx, window, 500).await;

        let v = views(cx, window);
        let web = v.web.clone().ok_or("no WKWebView next to GPUI's view")?;
        if web.isHidden() {
            return Err("the page is hidden".into());
        }
        if layer_opaque(&v) {
            return Err("GPUI's layer stays opaque over a hole".into());
        }
        let (ox, oy, side) = OVERLAY;
        let on_page = (50., 150.);
        let on_overlay = (ox + side / 2., oy + side / 2.);
        if !hit(&v, on_page.0, on_page.1).is_some_and(|h| h.isDescendantOf(&web)) {
            return Err("a point on the page does not hit the WKWebView".into());
        }
        if hit(&v, on_overlay.0, on_overlay.1).as_deref() != Some(&*v.gpui) {
            return Err("a point on the overlay does not hit GPUI's view".into());
        }
        println!("html preview: the page is behind GPUI and hit-tests through it");

        match capture(&v) {
            None => println!("html preview: pixels skipped (the window cannot be captured)"),
            Some(px) => {
                let page = px(on_page.0, on_page.1);
                let overlay = px(on_overlay.0, on_overlay.1);
                // Just below the overlay box, inside its shadow.
                let shadow = px(on_overlay.0, oy + side + 4.);
                println!("pixels: page {page:?}, overlay {overlay:?}, shadow {shadow:?}");
                if !(page[0] > 230 && page[1] < 30 && page[2] < 30) {
                    return Err(format!("the page does not show: {page:?}"));
                }
                if overlay[..3].iter().any(|c| *c > 30) {
                    return Err(format!(
                        "the overlay does not draw over the page: {overlay:?}"
                    ));
                }
                if !(shadow[0] > 40 && shadow[0] < page[0] - 10 && shadow[1] < 30) {
                    return Err(format!("the shadow is not over the page: {shadow:?}"));
                }
            }
        }

        // AppKit takes a click in an inactive app's window as activation, so the input checks
        // bring the app forward; locally only on request, since that takes focus.
        let input_checks =
            std::env::var_os("CI").is_some() || std::env::var_os("RECUR_WEBKIT_INPUT").is_some();
        if !input_checks {
            println!("html preview: input skipped (set RECUR_WEBKIT_INPUT=1, takes focus)");
        } else {
            input(window, &v, &web, preview, clicks, keys, cx).await?;
            blur(window, root, &v, &web, cx).await?;
        }

        cx.update(|cx| {
            root.update(cx, |r, cx| {
                r.preview = None;
                cx.notify();
            })
        });
        frames(cx, window, 100).await;
        if !web.isHidden() || !layer_opaque(&v) {
            return Err("the page stays up after the preview leaves".into());
        }
        if input_checks && !first_responder_in(&v, &v.gpui) {
            return Err("hiding the page leaves it the keyboard".into());
        }
        println!("html preview: hidden once undrawn");
        Ok(())
    }

    fn content_filters(web: &NSView) -> usize {
        // SAFETY: `contentFilters` is a plain property read.
        unsafe {
            let filters: *mut AnyObject = msg_send![web, contentFilters];
            msg_send![filters, count]
        }
    }

    /// A backdrop blur drawn over the page blurs the page, and goes with the scrim.
    async fn blur(
        window: AnyWindowHandle,
        root: &Entity<Root>,
        v: &Views,
        web: &NSView,
        cx: &mut AsyncApp,
    ) -> Result<(), String> {
        let set = |on: bool, cx: &mut AsyncApp| {
            cx.update(|cx| {
                root.update(cx, |r, cx| {
                    r.scrim = on;
                    cx.notify();
                })
            })
        };
        set(true, cx);
        frames(cx, window, 300).await;
        if content_filters(web) == 0 {
            return Err("a scrim's blur does not blur the page".into());
        }
        if let Some(px) = capture(v) {
            // Down from the input's bottom edge (y 30), into the red page.
            let profile: Vec<_> = [26., 30., 34., 38., 46., 60.]
                .iter()
                .map(|y| px(70., *y))
                .collect();
            println!("pixels: blurred edge {profile:?}");
            if profile[3][1] < 25 {
                return Err(format!("the page is not blurred: {profile:?}"));
            }
        }
        set(false, cx);
        frames(cx, window, 300).await;
        if content_filters(web) != 0 {
            return Err("the page stays blurred after the scrim leaves".into());
        }
        println!("html preview: a scrim's blur blurs the page");
        Ok(())
    }

    /// Clicks and keys on the page reach it through GPUI's view, the overlay keeps its clicks,
    /// and `release_keyboard` gives GPUI the keyboard back.
    async fn input(
        window: AnyWindowHandle,
        v: &Views,
        web: &NSView,
        preview: &Entity<HtmlPreview>,
        clicks: &Rc<Cell<u32>>,
        keys: &Rc<RefCell<Vec<String>>>,
        cx: &mut AsyncApp,
    ) -> Result<(), String> {
        let has = |k: &str| keys.borrow().iter().any(|s| s == k);
        let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        v.gpui
            .window()
            .ok_or("no NSWindow")?
            .makeKeyAndOrderFront(None);
        frames(cx, window, 300).await;

        click(v, 50., 20.);
        until(cx, window, "a click on the page", || has("focused")).await?;
        if !has("down:50,20") {
            return Err(format!(
                "the page got the click elsewhere: {:?}",
                keys.borrow()
            ));
        }
        if !first_responder_in(v, web) {
            return Err("a click on the page does not give it the keyboard".into());
        }
        key(v, "a", 0);
        until(cx, window, "typing in the page", || has("typed:a")).await?;
        println!("html preview: clicks and keys reach the page");

        let before = keys.borrow().len();
        let (ox, oy, side) = OVERLAY;
        click(v, ox + side / 2., oy + side / 2.);
        frames(cx, window, 200).await;
        if clicks.get() != 1 {
            return Err(format!("the overlay got {} clicks", clicks.get()));
        }
        if keys.borrow().len() != before {
            return Err(format!(
                "the page got the overlay's click: {:?}",
                keys.borrow()
            ));
        }
        if !first_responder_in(v, &v.gpui) {
            return Err("a click on GPUI's view does not take the keyboard back".into());
        }
        click(v, 50., 20.);
        frames(cx, window, 200).await;
        if !first_responder_in(v, web) {
            return Err("a second click on the page does not give it the keyboard".into());
        }
        cx.update_window(window, |_, window, cx| {
            preview.read(cx).release_keyboard(window)
        })
        .map_err(|e| e.to_string())?;
        if !first_responder_in(v, &v.gpui) {
            return Err("release_keyboard does not give GPUI the keyboard".into());
        }
        // The page keeps the keyboard for the hide check.
        click(v, 50., 20.);
        frames(cx, window, 200).await;
        println!("html preview: the overlay keeps its clicks");
        Ok(())
    }
}

/// The Linux HTML preview's offscreen page: laid out at the asked size, snapshotted, driven by
/// forwarded input, and kept on its document (no navigation, no popups).
#[cfg(target_os = "linux")]
fn html_page() {
    use recur_content::html_gtk::{Page, PageEvent};
    use serde_json::json;
    use std::time::{Duration, Instant};

    const DOC: &str = r#"<!doctype html><html><body style="margin:0;background:#f00">
<button id=b style="position:absolute;left:10px;top:10px;width:80px;height:30px"
  onclick="document.body.style.background='#00f';parent.postMessage({type:'clicked'})">Go</button>
<input id=i style="position:absolute;left:10px;top:60px;width:120px">
<script>
  window.open('https://example.com');
  setTimeout(() => { location.href = 'https://example.com/away'; parent.postMessage({type:'tried'}) }, 50);
</script></body></html>"#;

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let page = Page::new(DOC, tx).expect("page");
    page.resize(200., 100., 1.);
    page.set_active(true);
    let next = |rx: &mut futures::channel::mpsc::UnboundedReceiver<PageEvent>,
                want: &dyn Fn(&PageEvent) -> bool|
     -> PageEvent {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match rx.try_recv() {
                Ok(ev) if want(&ev) => return ev,
                Ok(_) => {}
                Err(_) => {
                    assert!(Instant::now() < deadline, "timed out");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    };
    let center = |ev: &PageEvent| match ev {
        PageEvent::Frame {
            width,
            height,
            bgra,
        } if (*width, *height) == (200, 100) => {
            let i = ((80 * width + 150) * 4) as usize;
            Some((bgra[i + 2], bgra[i + 1], bgra[i]))
        }
        _ => None,
    };
    next(&mut rx, &|e| {
        center(e).is_some_and(|(r, g, b)| r > 200 && g < 50 && b < 50)
    });
    // The page tried to navigate away and open a popup; the click below still lands in this
    // document, so both were refused.
    next(
        &mut rx,
        &|e| matches!(e, PageEvent::Message(m) if m.contains("tried")),
    );

    page.input(&json!({"type": "down", "x": 50, "y": 25, "button": 0, "detail": 1}));
    page.input(&json!({"type": "up", "x": 50, "y": 25, "button": 0, "detail": 1}));
    next(
        &mut rx,
        &|e| matches!(e, PageEvent::Message(m) if m.contains("clicked")),
    );
    next(&mut rx, &|e| {
        center(e).is_some_and(|(r, _, b)| b > 200 && r < 50)
    });

    page.input(&json!({"type": "down", "x": 40, "y": 70, "button": 0, "detail": 1}));
    page.input(&json!({"type": "up", "x": 40, "y": 70, "button": 0, "detail": 1}));
    for k in ["h", "i", "!"] {
        page.input(&json!({"type": "key", "key": k}));
    }
    page.input(&json!({"type": "key", "key": "Backspace"}));
    page.input(&json!({"type": "key", "key": "a", "ctrl": true, "selectAll": true}));
    page.input(&json!({"type": "key", "key": "c", "ctrl": true, "copy": true}));
    let copied = next(
        &mut rx,
        &|e| matches!(e, PageEvent::Reply(serde_json::Value::String(s)) if s.starts_with('\u{1}')),
    );
    let PageEvent::Reply(serde_json::Value::String(text)) = copied else {
        unreachable!()
    };
    assert_eq!(
        &text[1..],
        "hi",
        "typed into the input, Backspace removed the !"
    );
    println!("html page: ok");
}

/// What untrusted input cannot do on its own, through the shim: `:hover` styles follow the
/// pointer, and a `<select>` answers its options for the host's menu and takes a pick.
#[cfg(target_os = "linux")]
fn html_hover_and_select() {
    use recur_content::html_gtk::{Page, PageEvent};
    use serde_json::{Value, json};
    use std::time::{Duration, Instant};

    const DOC: &str = r#"<!doctype html><html><head><style>
#h{position:absolute;left:0;top:0;width:100px;height:100px;background:#f00}
@media screen{#h:hover{background:#0f0}}
</style></head><body style="margin:0">
<div id=h></div>
<select id=s style="position:absolute;left:120px;top:10px"
  onchange="parent.postMessage({type:'changed',value:this.value})">
  <option value=a>Apple</option><option value=b disabled>Banana</option><option value=c>Cherry</option>
</select></body></html>"#;

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let page = Page::new(DOC, tx).expect("page");
    page.resize(200., 100., 1.);
    page.set_active(true);
    let next = |rx: &mut futures::channel::mpsc::UnboundedReceiver<PageEvent>,
                want: &dyn Fn(&PageEvent) -> bool|
     -> PageEvent {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match rx.try_recv() {
                Ok(ev) if want(&ev) => return ev,
                Ok(_) => {}
                Err(_) => {
                    assert!(Instant::now() < deadline, "timed out");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    };
    let at = |ev: &PageEvent| match ev {
        PageEvent::Frame {
            width,
            height,
            bgra,
        } if (*width, *height) == (200, 100) => {
            let i = ((50 * width + 50) * 4) as usize;
            Some((bgra[i + 2], bgra[i + 1], bgra[i]))
        }
        _ => None,
    };
    next(&mut rx, &|e| {
        at(e).is_some_and(|(r, g, _)| r > 200 && g < 50)
    });
    page.input(&json!({"type": "move", "x": 50, "y": 50}));
    next(&mut rx, &|e| {
        at(e).is_some_and(|(r, g, _)| g > 200 && r < 50)
    });
    page.input(&json!({"type": "leave"}));
    next(&mut rx, &|e| {
        at(e).is_some_and(|(r, g, _)| r > 200 && g < 50)
    });

    page.input(&json!({"type": "down", "x": 130, "y": 20, "button": 0, "detail": 1}));
    let menu = next(
        &mut rx,
        &|e| matches!(e, PageEvent::Reply(v) if v.get("select").is_some()),
    );
    let PageEvent::Reply(v) = menu else {
        unreachable!()
    };
    let options: Vec<(String, bool)> = v["select"]["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|o| {
            (
                o["label"].as_str().unwrap_or("").to_string(),
                o["disabled"] == Value::Bool(true),
            )
        })
        .collect();
    assert_eq!(
        options,
        [
            ("Apple".into(), false),
            ("Banana".into(), true),
            ("Cherry".into(), false)
        ]
    );
    assert_eq!(v["select"]["index"], json!(0));
    page.input(&json!({"type": "up", "x": 130, "y": 20, "button": 0, "detail": 1}));
    page.input(&json!({"type": "pick", "index": 2}));
    next(
        &mut rx,
        &|e| matches!(e, PageEvent::Message(m) if m.contains("changed") && m.contains("\"c\"")),
    );
    println!("html hover and select: ok");
}
