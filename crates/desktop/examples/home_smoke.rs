//! First-run smoke on real services: a throwaway `RECUR_DATA_DIR` and git repo, the desktop's
//! registry in an unfocused window. A fresh data dir must land on Welcome; it then adds the repo
//! as a project through New project, creates a workspace through New workspace and checks it
//! opens, then visits the Inbox, Issues, Schedules (opening and closing the schedule editor)
//! and every project settings section, and quits. A panic aborts with a non-zero status.
//!
//! `cargo run -p recur-desktop --example home_smoke`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, IntoElement,
    Keystroke, Render, Window, WindowBounds, WindowOptions, point, px, size,
};
use recur_app::{Shell, ShellOptions};
use recur_services::AppServices;
use recur_view_core::{
    AppState, LocalStorage, ScheduleEditorRequest, ViewContext, ViewName, ViewRegistry,
};
use recur_view_home::new_project::NewProjectModal;
use recur_view_home::new_task::NewTaskModal;
use recur_view_home::schedule_editor::ScheduleEditor;

fn say(start: Instant, msg: impl AsRef<str>) {
    println!("[{:>6.1}s] {}", start.elapsed().as_secs_f32(), msg.as_ref());
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args([
            "-c",
            "user.name=Smoke",
            "-c",
            "user.email=smoke@example.com",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "git {args:?}");
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
        let root = std::env::temp_dir().join(format!("recur-home-smoke-{}", std::process::id()));
        let status = Command::new(std::env::current_exe().expect("current exe"))
            .env("RECUR_SMOKE_ROOT", &root)
            .env("RECUR_DATA_DIR", root.join("data"))
            .status()
            .expect("run smoke");
        std::process::exit(status.code().unwrap_or(1));
    };
    let start = Instant::now();
    let data = root.join("data");
    let repo = root.join("acme");
    std::fs::create_dir_all(&repo).expect("repo dir");
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "acme\n").expect("readme");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
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
        recur_view_home::register(&mut registry);
        recur_view_briefing::register(&mut registry);
        recur_view_projects::register(&mut registry);
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
        let repo = repo.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let ok = run(start, cx, window, state, &repo).await;
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

/// Frames until `f` holds, for up to `secs`.
async fn until(
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    secs: u64,
    f: impl Fn(&mut AsyncApp) -> bool,
) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f(cx) {
            return true;
        }
        frames(cx, window, 1).await;
    }
    f(cx)
}

async fn run(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    state: Entity<AppState>,
    repo: &Path,
) -> bool {
    frames(cx, window, 12).await;
    let landed = until(cx, window, 10, |cx| {
        state.read_with(cx, |s, _| s.booted && s.view == ViewName::Welcome)
    })
    .await;
    if !landed {
        let v = state.read_with(cx, |s, _| s.view);
        say(
            start,
            format!("a fresh data dir landed on {v:?}, not Welcome"),
        );
        return false;
    }
    say(start, "fresh data dir lands on Welcome");

    // Add the repo as a project.
    state.update(cx, |s, cx| s.open_new_project(false, cx));
    frames(cx, window, 6).await;
    let Some(np) = cx.update(|cx| NewProjectModal::current(cx)) else {
        say(start, "no New project modal");
        return false;
    };
    let path = repo.to_string_lossy().to_string();
    let _ = cx.update_window(window, |_, window, cx| {
        np.update(cx, |m, cx| m.set_repo_path(&path, window, cx))
    });
    frames(cx, window, 6).await;
    if !until(cx, window, 10, |cx| {
        np.read_with(cx, |m, cx| m.can_save(cx))
    })
    .await
    {
        say(start, "New project cannot save");
        return false;
    }
    np.update(cx, |m, cx| m.create(cx));
    if !until(cx, window, 15, |cx| {
        state.read_with(cx, |s, _| !s.projects.is_empty())
    })
    .await
    {
        say(start, "the project was not created");
        return false;
    }
    let project = state.read_with(cx, |s, _| s.projects[0].clone());
    say(
        start,
        format!("project {} at {}", project.name, project.repo_path),
    );
    frames(cx, window, 6).await;

    // New workspace.
    state.update(cx, |s, cx| s.open_new_task("", None, cx));
    frames(cx, window, 6).await;
    let Some(nt) = cx.update(|cx| NewTaskModal::current(cx)) else {
        say(start, "no New workspace modal");
        return false;
    };
    if !until(cx, window, 20, |cx| {
        nt.read_with(cx, |m, cx| m.can_start(cx))
    })
    .await
    {
        say(start, "New workspace cannot start");
        return false;
    }
    say(
        start,
        format!(
            "New workspace ready in {:?} mode",
            nt.read_with(cx, |m, _| m.mode())
        ),
    );
    let _ = cx.update_window(window, |_, window, cx| {
        nt.update(cx, |m, cx| m.submit(window, cx))
    });
    let opened = until(cx, window, 30, |cx| {
        state.read_with(cx, |s, _| {
            s.view == ViewName::Detail
                && s.selected_workspace()
                    .is_some_and(|w| w.project_id == project.id && !w.is_main)
        })
    })
    .await;
    if !opened {
        say(start, "the workspace did not open");
        return false;
    }
    let ws = state.read_with(cx, |s, _| {
        s.selected_workspace()
            .map(|w| (w.id.clone(), w.branch.clone()))
    });
    say(start, format!("workspace opened: {ws:?}"));
    frames(cx, window, 10).await;

    for (v, label) in [
        (ViewName::Briefing, "Inbox"),
        (ViewName::Issues, "Issues"),
        (ViewName::Schedules, "Schedules"),
    ] {
        state.update(cx, |s, cx| s.go(v, None, cx));
        frames(cx, window, 10).await;
        say(start, label);
    }
    state.update(cx, |s, cx| {
        s.set_schedule_editor(Some(ScheduleEditorRequest::default()), cx)
    });
    frames(cx, window, 8).await;
    let open = cx
        .update(|cx| ScheduleEditor::current(cx))
        .is_some_and(|e| e.read_with(cx, |e, _| e.is_open()));
    if !open {
        say(start, "the schedule editor did not open");
        return false;
    }
    say(start, "schedule editor open");
    key(cx, window, "escape");
    frames(cx, window, 6).await;
    let closed = cx
        .update(|cx| ScheduleEditor::current(cx))
        .is_some_and(|e| e.read_with(cx, |e, _| !e.is_open()));
    say(
        start,
        if closed {
            "schedule editor closed"
        } else {
            "schedule editor still open"
        },
    );

    state.update(cx, |s, cx| s.open_project(&project.id, cx));
    frames(cx, window, 8).await;
    if state.read_with(cx, |s, _| s.view) != ViewName::Project {
        say(start, "project settings did not open");
        return false;
    }
    say(start, "project settings");
    for theme in ["paper", "default"] {
        state.update(cx, |s, cx| s.set_theme(theme, cx));
        frames(cx, window, 6).await;
        say(start, format!("theme {theme}"));
    }
    closed
}
