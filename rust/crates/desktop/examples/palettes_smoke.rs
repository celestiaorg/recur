//! Palette smoke on real services: a throwaway `RECUR_DATA_DIR` and git repo with a project and
//! a workspace, the desktop's registry in an unfocused window. It opens every palette from its
//! chord (Go to, Run Command, the shortcuts list, Open in, Run, the language picker), types into
//! each, moves the selection, closes it and checks the flag dropped, in both themes, then quits.
//! A panic aborts with a non-zero status.
//!
//! `cargo run -p recur-desktop --example palettes_smoke`

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
use recur_view_core::{AppState, LocalStorage, Overlays, ViewContext, ViewName, ViewRegistry};

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

impl Render for WindowRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        recur_ui::style::window_root(self.0.clone())
    }
}

fn main() {
    // The data dir is read from the environment as the store opens: run again with it set.
    let Some(root) = std::env::var_os("RECUR_SMOKE_ROOT").map(PathBuf::from) else {
        let root =
            std::env::temp_dir().join(format!("recur-palettes-smoke-{}", std::process::id()));
        let status = Command::new(std::env::current_exe().expect("current exe"))
            .env("RECUR_SMOKE_ROOT", &root)
            .env("RECUR_DATA_DIR", root.join("data"))
            .env("RECUR_WORKTREES_DIR", root.join("worktrees"))
            .status()
            .expect("run smoke");
        std::process::exit(status.code().unwrap_or(1));
    };
    let start = Instant::now();
    let data = root.join("data");
    let repo = root.join("acme");
    std::fs::create_dir_all(repo.join("src")).expect("repo dir");
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "acme\n").expect("readme");
    std::fs::write(repo.join("src/main.go"), "package main\n\nfunc main() {}\n").expect("go");
    std::fs::create_dir_all(repo.join(".recur")).expect(".recur");
    std::fs::write(
        repo.join(".recur/project.toml"),
        "[[run]]\nname = \"dev\"\ncmd = \"sleep 30\"\n\n[[run]]\nname = \"test\"\ncmd = \"true\"\n",
    )
    .expect("project.toml");
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
    // A project with run commands and a workspace on it, through the services.
    let repo_path = repo.to_string_lossy().to_string();
    let project = rt
        .block_on(services.projects.add_project(&recur_domain::Project {
            name: "acme".into(),
            repo_path: repo_path.clone(),
            ..Default::default()
        }))
        .expect("add project");
    let ws = rt
        .block_on(services.workspaces.create_workspace(
            &project.id,
            "palette-smoke",
            "main",
            "agent",
            "",
            "",
            None,
        ))
        .expect("create workspace");
    say(
        start,
        format!("project {} workspace {}", project.name, ws.id),
    );

    let app = gpui_kit::application().with_assets(recur_ui::Assets);
    let handle = rt.handle().clone();
    let svc = Arc::new(svc);
    let quit_svc = svc.clone();
    let ws_id = ws.id.clone();
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
        recur_view_palettes::register(&mut registry);
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
        let ws_id = ws_id.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let ok = run(start, cx, window, state, &ws_id).await;
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

fn keys(cx: &mut AsyncApp, window: AnyWindowHandle, keys: &str) {
    for k in keys.split(' ') {
        let ks = Keystroke::parse(k).expect("keystroke");
        let _ = cx.update_window(window, |_, window, cx| window.dispatch_keystroke(ks, cx));
    }
}

fn type_text(cx: &mut AsyncApp, window: AnyWindowHandle, s: &str) {
    for ch in s.chars() {
        let k = if ch == ' ' {
            "space".to_string()
        } else {
            ch.to_string()
        };
        keys(cx, window, &k);
    }
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

/// Opens a palette with `chord`, types `query`, moves down and up, closes it with Escape.
async fn exercise(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    state: &Entity<AppState>,
    name: &str,
    chord: &str,
    query: &str,
    open: fn(&Overlays) -> bool,
) -> bool {
    keys(cx, window, chord);
    if !until(cx, window, 5, |cx| {
        state.read_with(cx, |s, _| open(&s.overlays))
    })
    .await
    {
        let why = state.read_with(cx, |s, _| {
            let id = s.selected_workspace_id.clone();
            (
                recur_view_core::run::workspace_dev_cmd(&id, &s.workspaces, &s.projects),
                recur_view_core::run::workspace_runs(&id, &s.workspaces, &s.projects).len(),
                s.projects
                    .iter()
                    .map(|p| p.run_commands.len())
                    .collect::<Vec<_>>(),
            )
        });
        say(start, format!("{name}: {chord} did not open it {why:?}"));
        return false;
    }
    frames(cx, window, 4).await;
    type_text(cx, window, query);
    frames(cx, window, 4).await;
    keys(cx, window, "down down up");
    frames(cx, window, 3).await;
    keys(cx, window, "cmd-a backspace");
    frames(cx, window, 3).await;
    keys(cx, window, "escape");
    let closed = until(cx, window, 5, |cx| {
        state.read_with(cx, |s, _| !open(&s.overlays))
    })
    .await;
    say(
        start,
        format!(
            "{name}: {}",
            if closed {
                "opened, typed, closed"
            } else {
                "did not close"
            }
        ),
    );
    closed
}

async fn run(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    state: Entity<AppState>,
    ws: &str,
) -> bool {
    frames(cx, window, 12).await;
    if !until(cx, window, 15, |cx| state.read_with(cx, |s, _| s.booted)).await {
        say(start, "the app did not boot");
        return false;
    }
    say(start, "booted");
    let _ = cx.update_window(window, |_, window, _| window.activate_window());
    let mut ok = true;
    for theme in ["default", "paper"] {
        state.update(cx, |s, cx| s.set_theme(theme, cx));
        state.update(cx, |s, cx| s.go(ViewName::Briefing, None, cx));
        frames(cx, window, 6).await;
        say(start, format!("theme {theme}"));
        ok &= exercise(start, cx, window, &state, "Go to", "cmd-k", "inbox", |o| {
            o.palette
        })
        .await;
        ok &= exercise(
            start,
            cx,
            window,
            &state,
            "Run Command",
            "cmd-shift-p",
            "sort",
            |o| o.commands,
        )
        .await;
        ok &= exercise(
            start,
            cx,
            window,
            &state,
            "Shortcuts",
            "cmd-shift-/",
            "tab",
            |o| o.shortcuts,
        )
        .await;
        state.update(cx, |s, cx| s.open_workspace(ws, false, cx));
        let on_detail = until(cx, window, 20, |cx| {
            state.read_with(cx, |s, _| {
                s.view == ViewName::Detail
                    && s.selected_workspace()
                        .is_some_and(|w| !w.worktree_path.is_empty())
            })
        })
        .await;
        if !on_detail {
            say(start, "the workspace did not open");
            return false;
        }
        frames(cx, window, 10).await;
        ok &= exercise(start, cx, window, &state, "Open in", "cmd-o", "fin", |o| {
            o.open_in
        })
        .await;
        ok &= exercise(start, cx, window, &state, "Run", "cmd-r", "te", |o| {
            o.run_menu
        })
        .await;
        state.update(cx, |s, cx| s.open_syntax_mode_picker(cx));
        frames(cx, window, 4).await;
        ok &= state.read_with(cx, |s, _| s.overlays.syntax_mode);
        type_text(cx, window, "go");
        frames(cx, window, 4).await;
        keys(cx, window, "down escape");
        let closed = until(cx, window, 5, |cx| {
            state.read_with(cx, |s, _| !s.overlays.syntax_mode)
        })
        .await;
        say(
            start,
            format!(
                "Language picker: {}",
                if closed {
                    "opened, typed, closed"
                } else {
                    "did not close"
                }
            ),
        );
        ok &= closed;
    }
    ok
}
