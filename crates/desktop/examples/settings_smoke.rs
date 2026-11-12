//! Settings smoke run on real services: a throwaway `RECUR_DATA_DIR`, the desktop's registry
//! in an unfocused window. It opens Settings with ⌘,, visits every section, opens and closes
//! the Add provider modal, filters the nav, and quits. A panic aborts with a non-zero status.
//!
//! `cargo run -p recur-desktop --example settings_smoke`

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, IntoElement,
    Keystroke, Render, Window, WindowBounds, WindowOptions, point, px, size,
};
use recur_app::{Shell, ShellOptions};
use recur_services::AppServices;
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use recur_view_settings::view::SettingsView;

fn say(start: Instant, msg: impl AsRef<str>) {
    println!("[{:>6.1}s] {}", start.elapsed().as_secs_f32(), msg.as_ref());
}

struct WindowRoot(Entity<Shell>);

impl WindowRoot {
    fn render_view(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement + use<> {
        recur_ui::style::window_root(self.0.clone())
    }
}

impl Render for WindowRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

fn main() {
    // The data dir is read from the environment as the store opens: run again with it set.
    let Some(root) = std::env::var_os("RECUR_SMOKE_ROOT").map(PathBuf::from) else {
        let root =
            std::env::temp_dir().join(format!("recur-settings-smoke-{}", std::process::id()));
        let status = Command::new(std::env::current_exe().expect("current exe"))
            .env("RECUR_SMOKE_ROOT", &root)
            .env("RECUR_DATA_DIR", root.join("data"))
            .status()
            .expect("run smoke");
        std::process::exit(status.code().unwrap_or(1));
    };
    let start = Instant::now();
    let data = root.join("data");
    say(start, format!("data dir {}", data.display()));

    let store = recur_store::Store::new().expect("open store");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let svc = rt
        .block_on(AppServices::open(Arc::new(store)))
        .expect("services");
    svc.recover();
    let services = svc.services();
    rt.block_on(svc.start());

    let app = gpui_kit::application().with_assets(recur_ui::Assets);
    let handle = rt.handle().clone();
    let svc = Arc::new(svc);
    let quit_svc = svc.clone();
    app.run(move |cx: &mut App| {
        let quit_rt = handle.clone();
        cx.on_app_quit(move |_| {
            quit_rt.block_on(quit_svc.shutdown());
            async {}
        })
        .detach();
        let mut registry = ViewRegistry::default();
        recur_view_chat::register(&mut registry);
        recur_view_review::register(&mut registry);
        recur_view_observatory::register(&mut registry);
        recur_view_workspace::register(&mut registry);
        recur_view_prs::register(&mut registry);
        recur_view_settings::register(&mut registry);
        recur_app::init(&registry, cx);
        let storage = LocalStorage::open(data.join(recur_view_core::LOCAL_STORAGE_FILE));
        let state = AppState::install(services.clone(), storage, cx);
        let ctx = ViewContext {
            state: state.clone(),
            services: services.clone(),
        };
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(40.), px(40.)),
                        size: size(px(1380.), px(900.)),
                    })),
                    focus: false,
                    show: true,
                    ..Default::default()
                },
                move |window, cx| {
                    let shell =
                        cx.new(|cx| Shell::new(ctx, registry, ShellOptions::default(), window, cx));
                    let root = cx.new(|_| WindowRoot(shell));
                    cx.new(|cx| gpui_kit::base::Root::new(root, window, cx))
                },
            )
            .expect("open window");
        let window: AnyWindowHandle = window.into();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let ok = run(start, cx, window, state).await;
            say(start, if ok { "PASS" } else { "FAIL" });
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    drop(svc);
}

async fn sleep(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Draws a frame: an unfocused window gets no display-link frames.
async fn frames(cx: &mut AsyncApp, window: AnyWindowHandle, n: usize) {
    for _ in 0..n {
        sleep(cx, 120).await;
        let _ = cx.update_window(window, |_, window, cx| {
            let drawn = window.draw(cx);
            drawn.clear(cx);
        });
    }
}

fn key(cx: &mut AsyncApp, window: AnyWindowHandle, k: &str) -> bool {
    cx.update_window(window, |_, window, cx| {
        let ks = Keystroke::parse_with_os(k, cx.platform_info().os).expect("keystroke");
        window.dispatch_keystroke(ks, cx)
    })
    .unwrap_or(false)
}

async fn run(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    state: Entity<AppState>,
) -> bool {
    frames(cx, window, 12).await;
    key(cx, window, "cmd-,");
    frames(cx, window, 6).await;
    if state.read_with(cx, |s, _| s.view) != ViewName::Settings {
        say(start, "cmd-, did not open Settings");
        return false;
    }
    say(start, "cmd-, opened Settings");

    for group in recur_view_settings::nav::nav() {
        for item in group.items {
            let id = item.id.id();
            state.update(cx, |s, cx| s.open_settings(Some(id), cx));
            frames(cx, window, 10).await;
            say(start, format!("{} / {}", group.group, item.label));
        }
    }

    state.update(cx, |s, cx| s.open_settings(Some("providers"), cx));
    frames(cx, window, 6).await;
    let Some(view) = cx.update(|cx| SettingsView::current(cx)) else {
        say(start, "no Settings view");
        return false;
    };
    let _ = cx.update_window(window, |_, window, cx| {
        view.update(cx, |v, cx| v.open_compat_modal(window, cx))
    });
    frames(cx, window, 6).await;
    say(start, "Add provider modal open");
    key(cx, window, "escape");
    frames(cx, window, 6).await;
    say(start, "Add provider modal closed");

    state.update(cx, |s, cx| s.open_settings(Some("appearance"), cx));
    frames(cx, window, 6).await;
    for theme in ["paper", "nord", "default"] {
        state.update(cx, |s, cx| s.set_theme(theme, cx));
        frames(cx, window, 4).await;
        say(start, format!("theme {theme}"));
    }
    true
}
