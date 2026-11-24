//! Captures the website screenshots from the desktop app on the typed fakes over the demo story
//! in `dataset.json`: each screen in `src-web/public/screenshots/manifest.json`, in Charcoal, as
//! a 1440×800 window saved at 2x (2880×1600) under the manifest's file names. It needs a Retina
//! display and moves the window there when another screen is the main one.
//!
//! `mise run capture:splash` (`cargo run -p recur-desktop --features fake --example capture --
//! [out dir]`)
//!
//! Each capture runs in a fresh process, as each had a fresh page in the web capture, so pane
//! layout comes from that screen's editor session and nothing carries over. The window floats
//! above others while it runs, since an occluded window draws no frames. Without an out dir it
//! writes to `src-web/public/screenshots`.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("capture: macOS only (it reads the window's own pixels)");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(target_os = "macos")]
mod dataset;
#[cfg(target_os = "macos")]
mod insights;

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use gpui_kit::{
        AnyWindowHandle, App, AppContext as _, AsyncApp, Entity, Keystroke, MouseButton,
        MouseDownEvent, MouseUpEvent, Pixels, PlatformInput, Point, point, px, size,
    };
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSFloatingWindowLevel, NSScreen, NSView,
        NSWindow, NSWindowButton,
    };
    use objc2_foundation::NSPoint;
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use recur_services::api::EditorSession;
    use recur_ui::widgets::ToastStore;
    use recur_view_core::{
        AppState, LOCAL_STORAGE_FILE, LocalStorage, ViewName, ViewRegistry, prefs::PanelKey,
    };
    use recur_view_workspace::WorkspaceDetail;

    const WIDTH: f32 = 1440.;
    const HEIGHT: f32 = 800.;
    const PR: &str = "486";
    const APP_THEME: &str = "default";
    const EDITOR_THEME: &str = "recur";

    #[derive(Clone, Copy)]
    enum View {
        Workspace(&'static str),
        Pr,
        Other(ViewName),
    }

    /// One tour stop, as the web capture's `screens.json` described it.
    struct Screen {
        id: &'static str,
        view: View,
        chat: &'static str,
        /// The agent column left of the editor.
        chat_open: bool,
        /// After the human answered the Inbox question.
        answered: bool,
        surface: &'static str,
        file: &'static str,
        /// Typed into the commit box of the review's Uncommitted tab.
        commit_message: &'static str,
        /// Clicked in order: the chat's collapsed tool calls, an Inbox row.
        clicks: &'static [(f32, f32)],
        file_list: f32,
        maestro_dock: f32,
    }

    const fn screen(id: &'static str, view: View) -> Screen {
        Screen {
            id,
            view,
            chat: "PA",
            chat_open: false,
            answered: false,
            surface: "editor",
            file: "src/main.go",
            commit_message: "",
            clicks: &[],
            file_list: 220.,
            maestro_dock: 390.,
        }
    }

    /// Inbox with another row selected, so its overview shows.
    const fn inbox(id: &'static str, clicks: &'static [(f32, f32)]) -> Screen {
        Screen {
            chat_open: true,
            clicks,
            ..screen(id, View::Other(ViewName::Inbox))
        }
    }

    const SCREENS: [Screen; 20] = [
        // The three centers of one workspace keep the same chat, so the demo can switch between them.
        Screen {
            chat_open: true,
            maestro_dock: 400.,
            ..screen("workspace", View::Workspace("LOOP-2231"))
        },
        Screen {
            chat_open: true,
            surface: "review",
            maestro_dock: 400.,
            ..screen("changes", View::Workspace("LOOP-2231"))
        },
        Screen {
            chat_open: true,
            surface: "pr",
            maestro_dock: 400.,
            ..screen("workspace-pr", View::Workspace("LOOP-2231"))
        },
        screen("pull-requests", View::Other(ViewName::Pr)),
        Screen {
            chat: "MA",
            chat_open: true,
            file: "src/passkey.go",
            maestro_dock: 650.,
            ..screen("loop", View::Workspace("LOOP-2240"))
        },
        Screen {
            surface: "review",
            file: "refunds/handler.go",
            file_list: 290.,
            commit_message: "Make refund retries idempotent",
            ..screen("uncommitted", View::Workspace("payments-idempotency"))
        },
        Screen {
            chat_open: true,
            file: "webhooks/delivery.go",
            clicks: &[(345., 242.)],
            maestro_dock: 470.,
            ..screen("background", View::Workspace(super::dataset::BACKGROUND))
        },
        screen("pull-request", View::Pr),
        inbox("inbox", &[(345., 483.)]),
        Screen {
            answered: true,
            ..inbox("inbox-answered", &[(345., 483.), (792., 177.)])
        },
        inbox("inbox-passkey", &[(345., 483.), (792., 203.)]),
        inbox("inbox-checkout", &[(345., 483.), (792., 256.)]),
        inbox("inbox-dark-mode", &[(345., 483.), (792., 308.)]),
        inbox("inbox-recovery", &[(345., 483.), (792., 361.)]),
        inbox("inbox-session", &[(345., 483.), (792., 439.)]),
        inbox("inbox-webhook", &[(345., 483.), (792., 492.)]),
        inbox("inbox-idempotency", &[(345., 483.), (792., 544.)]),
        screen("issues", View::Other(ViewName::Issues)),
        screen("schedules", View::Other(ViewName::Schedules)),
        screen("insights", View::Other(ViewName::Insights)),
    ];

    fn registry() -> ViewRegistry {
        let mut r = ViewRegistry::default();
        recur_view_chat::register(&mut r);
        recur_view_review::register(&mut r);
        recur_view_observatory::register(&mut r);
        recur_view_workspace::register(&mut r);
        recur_view_prs::register(&mut r);
        recur_view_settings::register(&mut r);
        recur_view_home::register(&mut r);
        recur_view_briefing::register(&mut r);
        recur_view_insights::register(&mut r);
        recur_view_projects::register(&mut r);
        recur_view_palettes::register(&mut r);
        r
    }

    pub fn main() {
        let out = std::env::args()
            .nth(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-web/public/screenshots")
            });
        std::fs::create_dir_all(&out).expect("create out dir");
        match std::env::var("RECUR_CAPTURE").ok() {
            Some(spec) => capture_one(&spec),
            None => capture_all(&out),
        }
    }

    /// Each screen in its own process, so nothing carries over from one to the next.
    fn capture_all(out: &Path) {
        let exe = std::env::current_exe().expect("current exe");
        let mut failed = false;
        for s in &SCREENS {
            let file = out.join(format!("{}.png", s.id));
            let ok = Command::new(&exe)
                .env("RECUR_CAPTURE", format!("{}|{}", s.id, file.display()))
                .status()
                .is_ok_and(|st| st.success());
            if !ok {
                eprintln!("capture: {} failed", file.display());
                failed = true;
            }
        }
        std::process::exit(i32::from(failed));
    }

    fn capture_one(spec: &str) {
        let (id, file) = spec.split_once('|').expect("RECUR_CAPTURE is screen|file");
        let file = PathBuf::from(file);
        let screen = SCREENS.iter().find(|s| s.id == id).expect("known screen");
        let scratch = std::env::temp_dir().join(format!("recur-capture-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let _rt = rt.enter();
        let session = EditorSession {
            open_files: Some(vec![screen.file.into()]),
            selected_path: screen.file.into(),
            left_kind: "agent".into(),
            right_kind: "files".into(),
            left_open: Some(screen.chat_open),
            right_open: Some(false),
            main_surface: screen.surface.into(),
            ..Default::default()
        };
        let services = super::dataset::services(session, screen.answered);

        gpui_kit::application()
            .with_assets(recur_ui::Assets)
            .run(move |cx: &mut App| {
                let registry = registry();
                recur_app::init(&registry, cx);
                let main = recur_app::MainWindow {
                    services,
                    storage: LocalStorage::open(scratch.join(LOCAL_STORAGE_FILE)),
                    data_dir: None,
                    registry,
                    shell: Default::default(),
                };
                let (window, state) = recur_app::open_main_window(main, cx).expect("open window");
                // The website shows the right panel docked.
                state.update(cx, |s, _| s.panels.side_float_by_default = false);
                let window: AnyWindowHandle = window.into();
                cx.spawn(async move |cx: &mut AsyncApp| {
                    let code = match run(cx, window, &state, screen, &file).await {
                        Ok(()) => {
                            println!("{}", file.display());
                            0
                        }
                        Err(err) => {
                            eprintln!("capture {}: {err}", file.display());
                            1
                        }
                    };
                    let _ = std::fs::remove_dir_all(&scratch);
                    cx.update(|cx| cx.quit());
                    std::process::exit(code);
                })
                .detach();
            });
    }

    async fn sleep(cx: &AsyncApp, ms: u64) {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
    }

    /// Lets loads land and the window present a few frames.
    async fn settle(cx: &mut AsyncApp, window: AnyWindowHandle, n: usize) {
        for _ in 0..n {
            sleep(cx, 100).await;
            let _ = cx.update_window(window, |_, window, _| window.refresh());
        }
    }

    #[allow(unsafe_code)]
    fn ns_window(cx: &mut AsyncApp, window: AnyWindowHandle) -> Result<Retained<NSWindow>, String> {
        let view = cx
            .update_window(window, |_, window, _| {
                let handle = window.window_handle().map_err(|e| e.to_string())?;
                let RawWindowHandle::AppKit(h) = handle.as_raw() else {
                    return Err("not an AppKit window".to_string());
                };
                // SAFETY: GPUI's NSView, alive while the window is.
                unsafe { Retained::retain(h.ns_view.as_ptr().cast::<NSView>()) }
                    .ok_or_else(|| "no NSView".to_string())
            })
            .map_err(|e| e.to_string())??;
        view.window().ok_or_else(|| "no NSWindow".to_string())
    }

    async fn run(
        cx: &mut AsyncApp,
        window: AnyWindowHandle,
        state: &Entity<AppState>,
        screen: &Screen,
        file: &Path,
    ) -> Result<(), String> {
        let _ = cx.update_window(window, |_, window, _| {
            window.resize(size(px(WIDTH), px(HEIGHT)))
        });
        let ns_window = ns_window(cx, window)?;
        move_to_sharpest_screen(&ns_window);
        // An accessory app never activates, so the capture leaves the user's focus alone.
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm)
                .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
        ns_window.setLevel(NSFloatingWindowLevel);
        ns_window.orderFrontRegardless();

        state.update(cx, |s, cx| {
            s.set_theme(APP_THEME, cx);
            s.set_editor_theme(EDITOR_THEME, cx);
            for (key, size) in [
                (PanelKey::Rail, 280.),
                (PanelKey::MaestroDock, screen.maestro_dock),
                (PanelKey::ObsSide, 180.),
                (PanelKey::PrFileList, screen.file_list),
                (PanelKey::InboxChat, 344.),
                (PanelKey::InboxList, 300.),
            ] {
                s.set_panel(key, size, cx);
            }
            s.pr_agent_dock_open = matches!(screen.view, View::Pr);
        });
        settle(cx, window, 6).await;
        state.update(cx, |s, cx| match screen.view {
            View::Workspace(id) => s.open_workspace_chat(id, screen.chat, cx),
            View::Pr => s.open_pr(PR, cx),
            View::Other(v) => s.go(v, None, cx),
        });
        settle(cx, window, 20).await;
        if !screen.commit_message.is_empty() {
            commit_message(cx, window, screen.commit_message)?;
            settle(cx, window, 6).await;
        }
        for &(x, y) in screen.clicks {
            click(cx, window, point(px(x), px(y)));
            settle(cx, window, 6).await;
        }
        dismiss_toasts(cx, window);
        settle(cx, window, 6).await;
        save(&ns_window, file)
    }

    /// Types the message into the commit box, then leaves it as a reader would see it.
    fn commit_message(
        cx: &mut AsyncApp,
        window: AnyWindowHandle,
        message: &str,
    ) -> Result<(), String> {
        cx.update_window(window, |_, window, cx| {
            let view = WorkspaceDetail::current(cx)
                .and_then(|d| d.read(cx).bound_view(cx))
                .ok_or("no workspace view")?;
            let obs = view.read(cx).observatory().clone();
            if !obs.update(cx, |o, cx| o.focus_commit_msg(window, cx)) {
                return Err("the commit box took no focus".to_string());
            }
            window.draw(cx).clear(cx);
            for ch in message.chars() {
                window.dispatch_keystroke(
                    Keystroke {
                        key: ch.to_lowercase().to_string(),
                        key_char: Some(ch.to_string()),
                        ..Default::default()
                    },
                    cx,
                );
            }
            window.blur(cx);
            Ok(())
        })
        .map_err(|e| e.to_string())?
    }

    fn click(cx: &mut AsyncApp, window: AnyWindowHandle, position: Point<Pixels>) {
        let _ = cx.update_window(window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
        });
    }

    /// The pending question pops its toast when the app starts.
    fn dismiss_toasts(cx: &mut AsyncApp, window: AnyWindowHandle) {
        let _ = cx.update_window(window, |_, window, cx| {
            let store = ToastStore::global(cx);
            let ids: Vec<_> = store.read(cx).toasts().iter().map(|t| t.id).collect();
            for id in ids {
                ToastStore::dismiss(&store, id, false, window, cx);
            }
        });
    }

    /// Puts the window on the screen with the highest backing scale (a Retina panel beside a 1x
    /// external display), so the capture comes out at 2x where the Mac has one.
    fn move_to_sharpest_screen(ns_window: &NSWindow) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let best = NSScreen::screens(mtm)
            .iter()
            .max_by(|a, b| a.backingScaleFactor().total_cmp(&b.backingScaleFactor()));
        if let Some(screen) = best {
            let area = screen.visibleFrame();
            ns_window.setFrameOrigin(NSPoint::new(
                area.origin.x,
                area.origin.y + area.size.height - ns_window.frame().size.height,
            ));
        }
    }

    /// The window is never key (activating it would take the user's focus), so AppKit draws the
    /// traffic lights inactive; paint them in their active fills over the real button frames.
    // simplify: colors sampled from a key-window capture on macOS 26; resample if a macOS
    // release restyles the buttons.
    fn paint_active_lights(ns_window: &NSWindow, rgb: &mut [u8], w: usize, h: usize) {
        // (button, fill, one-pixel rim)
        const LIGHTS: [(NSWindowButton, [f64; 3], [f64; 3]); 3] = [
            (
                NSWindowButton::CloseButton,
                [236., 103., 101.],
                [226., 52., 49.],
            ),
            (
                NSWindowButton::MiniaturizeButton,
                [242., 202., 68.],
                [234., 181., 6.],
            ),
            (
                NSWindowButton::ZoomButton,
                [101., 196., 102.],
                [48., 174., 50.],
            ),
        ];
        let frame = ns_window.frame();
        let scale = w as f64 / frame.size.width;
        for (kind, fill, rim) in LIGHTS {
            let Some(button) = ns_window.standardWindowButton(kind) else {
                continue;
            };
            let r = button.convertRect_toView(button.bounds(), None);
            let radius = r.size.width.min(r.size.height) / 2. * scale;
            let cx = (r.origin.x + r.size.width / 2.) * scale;
            let cy = (frame.size.height - r.origin.y - r.size.height / 2.) * scale;
            let (x0, x1) = (
                (cx - radius - 1.).max(0.) as usize,
                ((cx + radius + 1.) as usize).min(w),
            );
            let (y0, y1) = (
                (cy - radius - 1.).max(0.) as usize,
                ((cy + radius + 1.) as usize).min(h),
            );
            for y in y0..y1 {
                for x in x0..x1 {
                    let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
                    let a = (radius + 0.5 - d).clamp(0., 1.);
                    let color = if d > radius - 1. { rim } else { fill };
                    let px = &mut rgb[(y * w + x) * 3..][..3];
                    for (c, f) in px.iter_mut().zip(color) {
                        *c = (f64::from(*c) * (1. - a) + f * a).round() as u8;
                    }
                }
            }
        }
    }

    /// Writes the window's content (no frame or shadow) as a 2x PNG.
    fn save(ns_window: &NSWindow, file: &Path) -> Result<(), String> {
        use core_graphics::geometry::{CGPoint, CGRect, CGSize};
        use core_graphics::window::{
            create_image, kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming,
            kCGWindowListOptionIncludingWindow,
        };
        let null = CGRect::new(
            &CGPoint::new(f64::INFINITY, f64::INFINITY),
            &CGSize::new(0., 0.),
        );
        let number = u32::try_from(ns_window.windowNumber()).map_err(|e| e.to_string())?;
        let image = create_image(
            null,
            kCGWindowListOptionIncludingWindow,
            number,
            kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution,
        )
        .ok_or("the OS would not capture the window")?;
        if image.bits_per_pixel() != 32 {
            return Err(format!("{} bits per pixel", image.bits_per_pixel()));
        }
        let (w, h, row) = (image.width(), image.height(), image.bytes_per_row());
        if w != 2 * WIDTH as usize {
            return Err(format!(
                "captured {w}×{h}: the website wants 2x, so run this on a Mac with a Retina display"
            ));
        }
        let data = image.data();
        let bytes = data.bytes();
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for px in bytes[y * row..y * row + w * 4].as_chunks::<4>().0 {
                // BGRA over an opaque window.
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        paint_active_lights(ns_window, &mut rgb, w, h);
        use image::ImageEncoder as _;
        use image::codecs::png::{CompressionType, FilterType, PngEncoder};
        let mut png = Vec::new();
        PngEncoder::new_with_quality(&mut png, CompressionType::Best, FilterType::Adaptive)
            .write_image(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        std::fs::write(file, png).map_err(|e| format!("{}: {e}", file.display()))
    }
}
