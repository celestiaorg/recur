//! Captures the website screenshots from the desktop app on the typed fakes over the demo story
//! in `dataset.json`: each screen in `src-web/public/screenshots/manifest.json`, in Paper and
//! Charcoal, as a 1440×800 window saved at 2x (2880×1600) under the manifest's file names. It
//! needs a Retina display and moves the window there when another screen is the main one.
//!
//! The theme section gets `themes/app-{key}.png` per app theme and `themes/editor-{id}.png` per
//! editor theme, cropped to the code area, which the page lays over the app shot at the place
//! `themes/manifest.json` gives.
//!
//! `mise run capture:splash` (`cargo run -p recur-desktop --features fake --example capture --
//! [out dir] [themes]`; `themes` captures only the theme set)
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
mod mac {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use gpui_kit::{AnyWindowHandle, App, AppContext as _, AsyncApp, Entity, px, size};
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSFloatingWindowLevel, NSScreen, NSView,
        NSWindow, NSWindowButton,
    };
    use objc2_foundation::NSPoint;
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use recur_services::api::EditorSession;
    use recur_view_core::{
        AppState, LOCAL_STORAGE_FILE, LocalStorage, ViewName, ViewRegistry, prefs::PanelKey,
    };

    const WIDTH: f32 = 1440.;
    const HEIGHT: f32 = 800.;
    const WORKSPACE: &str = "LOOP-2231";
    const PR: &str = "486";

    /// (file suffix, app theme, editor theme).
    const THEMES: [(&str, &str, &str); 2] = [
        ("paper", "paper", "paper"),
        ("charcoal", "default", "recur"),
    ];

    /// The editor themes the website's theme section offers (bundle theme ids; keep in sync
    /// with `src-web/src/lib/themes.ts`).
    const EDITOR_THEMES: [&str; 16] = [
        "recur",
        "paper",
        "cool-glow",
        "made-of-code",
        "dracula",
        "github-dark",
        "github-light",
        "tokyo-night",
        "nord",
        "gruvbox-dark-medium",
        "monokai",
        "material-theme",
        "dark-plus",
        "solarized-dark",
        "solarized-light",
        "aura",
    ];

    #[derive(Clone, Copy)]
    enum View {
        Workspace,
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
        /// The Files side panel.
        files_open: bool,
        review: bool,
        maestro_dock: f32,
    }

    const fn screen(id: &'static str, view: View) -> Screen {
        Screen {
            id,
            view,
            chat: "PA",
            chat_open: false,
            files_open: false,
            review: false,
            maestro_dock: 390.,
        }
    }

    const SCREENS: [Screen; 8] = [
        Screen {
            chat_open: true,
            files_open: true,
            maestro_dock: 440.,
            ..screen("workspace", View::Workspace)
        },
        Screen {
            chat: "PA2",
            review: true,
            maestro_dock: 400.,
            ..screen("changes", View::Workspace)
        },
        Screen {
            chat: "MA",
            chat_open: true,
            maestro_dock: 650.,
            ..screen("loop", View::Workspace)
        },
        screen("pull-request", View::Pr),
        Screen {
            chat_open: true,
            ..screen("inbox", View::Other(ViewName::Inbox))
        },
        screen("issues", View::Other(ViewName::Issues)),
        screen("schedules", View::Other(ViewName::Schedules)),
        // The website's theme section: an app theme shot with each editor theme's code area
        // laid over it.
        Screen {
            chat_open: true,
            ..screen("theme", View::Workspace)
        },
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
            None if std::env::args().nth(2).as_deref() == Some("themes") => {
                std::process::exit(i32::from(capture_themes(&out.join("themes")) > 0))
            }
            None => capture_all(&out),
        }
    }

    /// One capture: a screen with an app and an editor theme, written to `file`.
    struct Job {
        screen: &'static str,
        app: String,
        editor: String,
        file: PathBuf,
    }

    fn job(screen: &'static str, app: &str, editor: &str, file: PathBuf) -> Job {
        Job {
            screen,
            app: app.into(),
            editor: editor.into(),
            file,
        }
    }

    fn run_jobs(jobs: &[Job]) -> usize {
        let exe = std::env::current_exe().expect("current exe");
        let mut failed = 0;
        for j in jobs {
            let spec = format!("{}|{}|{}|{}", j.screen, j.app, j.editor, j.file.display());
            let ok = Command::new(&exe)
                .env("RECUR_CAPTURE", spec)
                .status()
                .is_ok_and(|st| st.success());
            if !ok {
                eprintln!("capture: {} failed", j.file.display());
                failed += 1;
            }
        }
        failed
    }

    fn capture_all(out: &Path) {
        let mut jobs = Vec::new();
        for (suffix, app, editor) in THEMES {
            for s in SCREENS.iter().filter(|s| s.id != "theme") {
                jobs.push(job(
                    s.id,
                    app,
                    editor,
                    out.join(format!("{}-{suffix}.png", s.id)),
                ));
            }
        }
        let failed = run_jobs(&jobs) + capture_themes(&out.join("themes"));
        std::process::exit(i32::from(failed > 0));
    }

    /// `themes/app-{key}.png` per app theme, `themes/editor-{id}.png` per editor theme cropped
    /// to the code area (where editor themes differ), and `themes/manifest.json` placing it.
    fn capture_themes(dir: &Path) -> usize {
        std::fs::create_dir_all(dir).expect("create themes dir");
        let raw = std::env::temp_dir().join(format!("recur-capture-themes-{}", std::process::id()));
        std::fs::create_dir_all(&raw).expect("create raw dir");
        let mut jobs: Vec<Job> = recur_ui::theme::ThemeId::ALL
            .into_iter()
            .map(|t| {
                let editor = if t.is_light() { "paper" } else { "recur" };
                job(
                    "theme",
                    t.key(),
                    editor,
                    dir.join(format!("app-{}.png", t.key())),
                )
            })
            .collect();
        for e in EDITOR_THEMES {
            jobs.push(job("theme", "default", e, raw.join(format!("{e}.png"))));
        }
        // The first editor theme again over a light app theme: the code area must not change.
        let check = raw.join("check.png");
        jobs.push(job("theme", "paper", EDITOR_THEMES[0], check.clone()));
        let failed = run_jobs(&jobs);
        if failed > 0 {
            return failed;
        }
        let load = |p: &Path| image::open(p).expect("read capture").to_rgb8();
        let shots: Vec<_> = EDITOR_THEMES
            .iter()
            .map(|e| load(&raw.join(format!("{e}.png"))))
            .collect();
        let (w, h) = shots[0].dimensions();
        // Columns and rows where editor themes disagree across a tenth of the window: the code
        // area. Animated dots (a running workspace, a waiting chat) differ in a few pixels only.
        let (mut cols, mut rows) = (vec![0u32; w as usize], vec![0u32; h as usize]);
        for shot in &shots[1..] {
            for (x, y, px) in shot.enumerate_pixels() {
                if differs(px, shots[0].get_pixel(x, y)) {
                    cols[x as usize] += 1;
                    rows[y as usize] += 1;
                }
            }
        }
        let span = |counts: &[u32], min: u32| {
            let first = counts
                .iter()
                .position(|&n| n > min)
                .expect("editor themes differ");
            let last = counts
                .iter()
                .rposition(|&n| n > min)
                .expect("editor themes differ");
            (first as u32, last as u32 + 1)
        };
        let ((x0, x1), (y0, y1)) = (span(&cols, h / 10), span(&rows, w / 10));
        // Pixels that follow the app theme (the active line's edge, the gutter's change bars)
        // stay transparent, so the app shot underneath shows them in its own colours.
        let check = load(&check);
        let app_owned = |x: u32, y: u32| differs(check.get_pixel(x, y), shots[0].get_pixel(x, y));
        for (e, shot) in EDITOR_THEMES.iter().zip(&shots) {
            let crop = image::RgbaImage::from_fn(x1 - x0, y1 - y0, |x, y| {
                let [r, g, b] = shot.get_pixel(x0 + x, y0 + y).0;
                let a = if app_owned(x0 + x, y0 + y) { 0 } else { 255 };
                image::Rgba([r, g, b, a])
            });
            crop.save(dir.join(format!("editor-{e}.png")))
                .expect("write editor crop");
        }
        let manifest = serde_json::json!({
            "width": w,
            "height": h,
            "editor": { "x": x0, "y": y0, "width": x1 - x0, "height": y1 - y0 },
        });
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest).expect("manifest JSON") + "\n",
        )
        .expect("write themes manifest");
        let _ = std::fs::remove_dir_all(&raw);
        0
    }

    fn differs(a: &image::Rgb<u8>, b: &image::Rgb<u8>) -> bool {
        a.0.iter().zip(b.0).any(|(p, q)| p.abs_diff(q) > 8)
    }

    fn capture_one(spec: &str) {
        let mut parts = spec.splitn(4, '|');
        let mut next = || {
            parts
                .next()
                .expect("RECUR_CAPTURE is screen|app|editor|file")
        };
        let (id, app_theme, editor_theme, file) = (next(), next(), next(), PathBuf::from(next()));
        let screen = SCREENS.iter().find(|s| s.id == id).expect("known screen");
        let (app_theme, editor_theme) = (app_theme.to_string(), editor_theme.to_string());
        let scratch = std::env::temp_dir().join(format!("recur-capture-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let _rt = rt.enter();
        let services = super::dataset::services(EditorSession {
            open_files: Some(vec!["src/main.go".into()]),
            selected_path: "src/main.go".into(),
            left_kind: "agent".into(),
            right_kind: "files".into(),
            left_open: Some(screen.chat_open),
            right_open: Some(screen.files_open),
            main_surface: if screen.review { "review" } else { "editor" }.into(),
            ..Default::default()
        });

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
                let window: AnyWindowHandle = window.into();
                cx.spawn(async move |cx: &mut AsyncApp| {
                    let themes = (app_theme.as_str(), editor_theme.as_str());
                    let code = match run(cx, window, &state, screen, themes, &file).await {
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
        (app_theme, editor_theme): (&str, &str),
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
            s.set_theme(app_theme, cx);
            s.set_editor_theme(editor_theme, cx);
            for (key, size) in [
                (PanelKey::Rail, 280.),
                (PanelKey::MaestroDock, screen.maestro_dock),
                (PanelKey::ObsSide, 180.),
                (PanelKey::PrFileList, 220.),
            ] {
                s.set_panel(key, size, cx);
            }
            s.pr_agent_dock_open = matches!(screen.view, View::Pr);
        });
        settle(cx, window, 6).await;
        state.update(cx, |s, cx| match screen.view {
            View::Workspace => s.open_workspace_chat(WORKSPACE, screen.chat, cx),
            View::Pr => s.open_pr(PR, cx),
            View::Other(v) => s.go(v, None, cx),
        });
        settle(cx, window, 20).await;
        save(&ns_window, file)
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
            for px in bytes[y * row..y * row + w * 4].chunks_exact(4) {
                // BGRA over an opaque window.
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        paint_active_lights(ns_window, &mut rgb, w, h);
        image::save_buffer(
            file,
            &rgb,
            w as u32,
            h as u32,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("{}: {e}", file.display()))
    }
}
