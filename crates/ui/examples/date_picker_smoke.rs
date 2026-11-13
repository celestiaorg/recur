//! The native date picker in a real window (macOS): Space on a date field opens WebKit's
//! 139 by 148 calendar window under the input, Escape closes it. Exits non-zero on failure.
//!
//! `cargo run -p recur-ui --example date_picker_smoke`

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use std::time::Duration;

    use gpui_kit::{
        App, AppContext as _, AsyncApp, Bounds, Context, Entity, IntoElement, Keystroke,
        ParentElement as _, Render, Styled as _, Window, WindowBounds, WindowOptions, div, point,
        px, size,
    };
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use recur_ui::color::hex;
    use recur_ui::logic::date_time::Kind;
    use recur_ui::widgets::{DateTimeField, DateTimeStyle};

    struct Root(Entity<DateTimeField>);
    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p_8().child(self.0.clone())
        }
    }

    /// Picker-sized windows on screen: (x, y, w, h) in AppKit screen points.
    fn pickers() -> Vec<(f64, f64, f64, f64)> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        NSApplication::sharedApplication(mtm)
            .windows()
            .iter()
            .filter(|w| w.isVisible())
            .map(|w| {
                let f = w.frame();
                (f.origin.x, f.origin.y, f.size.width, f.size.height)
            })
            .filter(|f| (f.2, f.3) == (139., 148.))
            .collect()
    }

    gpui_kit::application()
        .with_assets(recur_ui::Assets)
        .run(|cx: &mut App| {
            recur_ui::init(cx);
            let mut field = None;
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: point(px(80.), px(80.)),
                            size: size(px(420.), px(240.)),
                        })),
                        focus: false,
                        ..Default::default()
                    },
                    |_, cx| {
                        let f = cx.new(|cx| {
                            let style = DateTimeStyle {
                                color: hex(0xaaaaaa),
                                size: 12.,
                                tabular: false,
                                pad_x: 6.,
                                height: 24.,
                                dark: true,
                            };
                            let mut f = DateTimeField::new("dt", Kind::Date, style, cx);
                            f.set_value("2026-10-27", cx);
                            f
                        });
                        field = Some(f.clone());
                        cx.new(|_| Root(f))
                    },
                )
                .expect("open window");
            let field = field.expect("field");
            let window: gpui_kit::AnyWindowHandle = window.into();
            cx.spawn(async move |cx: &mut AsyncApp| {
                let frame = async |cx: &mut AsyncApp| {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                    let _ = cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx));
                };
                let key = |cx: &mut AsyncApp, k: &'static str| {
                    let ks = Keystroke::parse(k).expect("keystroke");
                    let _ =
                        cx.update_window(window, |_, window, cx| window.dispatch_keystroke(ks, cx));
                };
                frame(cx).await;
                let month = field.read_with(cx, |f, _| f.focus_handles()[0].clone());
                let _ = cx.update_window(window, |_, window, cx| window.focus(&month, cx));
                frame(cx).await;
                key(cx, "space");
                frame(cx).await;
                let open = pickers();
                println!("picker windows after Space: {open:?}");
                key(cx, "escape");
                frame(cx).await;
                let closed = pickers();
                println!("picker windows after Escape: {closed:?}");
                let ok = open.len() == 1 && closed.is_empty();
                println!("{}", if ok { "PASS" } else { "FAIL" });
                cx.update(|cx| cx.quit());
                if !ok {
                    std::process::exit(1);
                }
            })
            .detach();
        });
}
