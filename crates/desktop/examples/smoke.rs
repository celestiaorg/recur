//! End-to-end smoke run of the desktop app on real services: a throwaway `RECUR_DATA_DIR` and
//! git repo, the engine-backed services and the real `claude` CLI, the shell with every view
//! registered in an unfocused window. It opens the repo, creates a workspace (⇧⌘N), types a
//! chat turn into the composer and sends it (Enter), watches the reply stream, opens the
//! review diff, and quits, logging what it saw.
//!
//! `cargo run -p recur-desktop --example smoke`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, Focusable as _,
    IntoElement, Keystroke, Render, Window, WindowBounds, WindowOptions, point, px, size,
};
use recur_app::{Shell, ShellOptions};
use recur_services::{AppEvent, AppServices, Services};
use recur_view_core::{AppState, LocalStorage, ViewContext, ViewName, ViewRegistry};
use recur_view_workspace::detail::WorkspaceDetail;
use recur_view_workspace::view::WorkspaceView;

const PROMPT: &str = "create a file named hello.txt containing exactly one line: hello from recur. then reply with the single word done.";

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
        let root = std::env::temp_dir().join(format!("recur-smoke-{}", std::process::id()));
        let status = Command::new(std::env::current_exe().expect("current exe"))
            .env("RECUR_SMOKE_ROOT", &root)
            .env("RECUR_DATA_DIR", root.join("data"))
            .status()
            .expect("run smoke");
        std::process::exit(status.code().unwrap_or(1));
    };
    let start = Instant::now();
    let (data, repo) = (root.join("data"), root.join("repo"));
    std::fs::create_dir_all(&repo).expect("temp repo");
    std::fs::write(repo.join("README.md"), "# smoke\n").expect("readme");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    say(start, format!("data dir {}", data.display()));
    say(start, format!("repo {}", repo.display()));

    recur_platform::cliinstall::ensure_path();
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

    // The backend's events, as the UI receives them.
    rt.spawn({
        let services = services.clone();
        async move {
            let mut rx = services.subscribe();
            let mut deltas = 0usize;
            while let Some(ev) = rx.recv().await {
                let line = match &ev {
                    AppEvent::SessionDelta(_) => {
                        deltas += 1;
                        if deltas % 10 != 1 {
                            continue;
                        }
                        format!("session.delta #{deltas}")
                    }
                    AppEvent::SessionTurnEnd(_)
                    | AppEvent::SessionToolInput(_)
                    | AppEvent::SessionToolResult(_)
                    | AppEvent::AgentUpdated(_)
                    | AppEvent::PermissionRequested(_) => {
                        let s = format!("{ev:?}");
                        s.chars().take(160).collect()
                    }
                    _ => continue,
                };
                say(start, format!("event: {line}"));
            }
        }
    });

    let app = gpui_kit::application().with_assets(recur_ui::Assets);
    let (svc_ui, repo_ui) = (services.clone(), repo.clone());
    let handle = rt.handle().clone();
    let svc = Arc::new(svc);
    let quit_svc = svc.clone();
    app.run(move |cx: &mut App| {
        // macOS ends the process from `terminate:` instead of returning from `run`.
        let quit_rt = handle.clone();
        cx.on_app_quit(move |_| {
            say(start, "app quit; shutting the services down");
            quit_rt.block_on(quit_svc.shutdown());
            say(start, "done");
            async {}
        })
        .detach();
        let mut registry = ViewRegistry::default();
        recur_view_chat::register(&mut registry);
        recur_view_review::register(&mut registry);
        recur_view_observatory::register(&mut registry);
        recur_view_workspace::register(&mut registry);
        recur_app::init(&registry, cx);
        let storage = LocalStorage::open(data.join(recur_view_core::LOCAL_STORAGE_FILE));
        let state = AppState::install(svc_ui.clone(), storage, cx);
        let ctx = ViewContext {
            state: state.clone(),
            services: svc_ui.clone(),
        };
        let shell_slot = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = shell_slot.clone();
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
                    *slot.borrow_mut() = Some(shell.clone());
                    let root = cx.new(|_| WindowRoot(shell));
                    cx.new(|cx| gpui_kit::base::Root::new(root, window, cx))
                },
            )
            .expect("open window");
        let window: AnyWindowHandle = window.into();
        let shell = shell_slot.borrow().clone().expect("shell");
        cx.spawn(async move |cx: &mut AsyncApp| {
            run(start, cx, window, shell, state, svc_ui, repo_ui, handle).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    drop(svc);
}

/// Runs `fut` on the tokio runtime and awaits it from the UI thread.
async fn on_rt<T: Send + 'static>(
    rt: &tokio::runtime::Handle,
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    rt.spawn(fut).await.expect("runtime task")
}

async fn sleep(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Draws a frame. An unfocused window behind others gets no display-link frames, and views
/// sync their children while rendering.
fn frame(cx: &mut AsyncApp, window: AnyWindowHandle) {
    let _ = cx.update_window(window, |_, window, cx| {
        let drawn = window.draw(cx);
        drawn.clear(cx);
    });
}

fn key(cx: &mut AsyncApp, window: AnyWindowHandle, k: &str) -> bool {
    cx.update_window(window, |_, window, cx| {
        let ks = Keystroke::parse_with_os(k, cx.platform_info().os).expect("keystroke");
        window.dispatch_keystroke(ks, cx)
    })
    .unwrap_or(false)
}

fn bound_view(cx: &mut AsyncApp, shell: &Entity<Shell>) -> Option<Entity<WorkspaceView>> {
    cx.update(|cx| {
        let detail = shell
            .read(cx)
            .route_view(ViewName::Detail)?
            .downcast::<WorkspaceDetail>()
            .ok()?;
        detail.read(cx).bound_view(cx)
    })
}

#[allow(clippy::too_many_arguments)]
async fn run(
    start: Instant,
    cx: &mut AsyncApp,
    window: AnyWindowHandle,
    shell: Entity<Shell>,
    state: Entity<AppState>,
    services: Services,
    repo: PathBuf,
    rt: tokio::runtime::Handle,
) {
    sleep(cx, 1500).await;

    // 1. Open the temp repo (what `recur .` hands the app).
    let path = repo.to_string_lossy().to_string();
    let svc = services.clone();
    let opened = on_rt(&rt, async move { svc.workspaces.open_path(&path).await }).await;
    let opened = match opened {
        Ok(o) => o,
        Err(err) => return say(start, format!("FAIL open path: {err}")),
    };
    say(
        start,
        format!(
            "opened project {} (working copy {})",
            opened.project_id, opened.workspace_id
        ),
    );
    let project_id = opened.project_id.clone();
    state.update(cx, |s, cx| s.apply_open_path_result(opened, cx));
    for _ in 0..6 {
        sleep(cx, 250).await;
        frame(cx, window);
    }

    // 2. ⇧⌘N: a blank workspace on the project.
    let before: Vec<String> = state.read_with(cx, |s, _| {
        s.workspaces.iter().map(|w| w.id.clone()).collect()
    });
    let handled = key(cx, window, "cmd-shift-n");
    say(start, format!("cmd-shift-n handled: {handled}"));
    let mut ws_id = String::new();
    for _ in 0..240 {
        sleep(cx, 250).await;
        frame(cx, window);
        let found = state.read_with(cx, |s, _| {
            s.workspaces
                .iter()
                .find(|w| !w.is_main && !before.contains(&w.id))
                .map(|w| (w.id.clone(), w.worktree_path.clone(), w.phase.clone()))
        });
        if let Some((id, wt, phase)) = found
            && !wt.is_empty()
            && Path::new(&wt).join("README.md").exists()
        {
            say(
                start,
                format!("workspace {id} created (phase {phase:?}) at {wt}"),
            );
            // ⇧⌘N outside Welcome leaves it in the sidebar; its row opens it.
            state.update(cx, |s, cx| s.open_workspace(&id, false, cx));
            ws_id = id;
            break;
        }
    }
    if ws_id.is_empty() {
        let dump = state.read_with(cx, |s, _| {
            format!(
                "view {:?}, selected {:?}, workspaces {:?}",
                s.view,
                s.selected_workspace_id,
                s.workspaces
                    .iter()
                    .map(|w| (
                        w.id.clone(),
                        w.is_main,
                        w.worktree_path.clone(),
                        w.phase.clone()
                    ))
                    .collect::<Vec<_>>()
            )
        });
        let toasts = cx.update(|cx| {
            let t = recur_ui::widgets::ToastStore::global(cx);
            t.read(cx)
                .toasts()
                .iter()
                .map(|t| t.text.to_string())
                .collect::<Vec<_>>()
        });
        return say(
            start,
            format!("FAIL no workspace was created: {dump}; toasts {toasts:?}"),
        );
    }
    for _ in 0..6 {
        sleep(cx, 250).await;
        frame(cx, window);
    }

    // 3. Type the turn into the composer and press Enter.
    let Some(view) = bound_view(cx, &shell) else {
        return say(start, "FAIL the detail route has no workspace view");
    };
    let chat = view.read_with(cx, |v, _| v.chat().clone());
    let _ = cx.update_window(window, |_, window, cx| {
        chat.update(cx, |c, cx| c.focus_composer(window, cx))
    });
    sleep(cx, 300).await;
    frame(cx, window);
    for ch in PROMPT.chars() {
        let k = if ch == ' ' {
            "space".to_string()
        } else {
            ch.to_string()
        };
        let _ = key(cx, window, &k);
    }
    sleep(cx, 300).await;
    let typed = chat.read_with(cx, |c, _| c.input().to_string());
    say(
        start,
        format!("composer holds {} chars", typed.chars().count()),
    );
    let _ = key(cx, window, "enter");
    frame(cx, window);
    say(start, "sent with Enter");

    // 4. Watch the transcript the chat renders, approving any permission prompt.
    let mut last = (0usize, 0usize, false);
    let mut saw_stream = false;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        sleep(cx, 500).await;
        frame(cx, window);
        let perms: Vec<String> = state.read_with(cx, |s, _| {
            s.permission_requests.iter().map(|p| p.id.clone()).collect()
        });
        for id in perms {
            say(start, format!("approving permission {id}"));
            state.update(cx, |s, cx| s.answer_permission(&id, true, "", false, cx));
        }
        let (rows, body, streaming, ended) = chat.read_with(cx, |c, _| {
            let msgs = &c.props().messages;
            let reply = msgs
                .iter()
                .rev()
                .find(|m| !m.me && m.tool.is_empty() && !m.turn_end);
            (
                msgs.len(),
                reply.map_or(0, |m| m.body.len()),
                msgs.iter().any(|m| m.streaming),
                msgs.iter().any(|m| m.turn_end),
            )
        });
        saw_stream |= streaming;
        if (rows, body, streaming) != last {
            say(
                start,
                format!("transcript: {rows} rows, reply {body} chars, streaming {streaming}"),
            );
            last = (rows, body, streaming);
        }
        if ended {
            let tail = chat.read_with(cx, |c, _| {
                c.props()
                    .messages
                    .iter()
                    .filter(|m| !m.me && m.tool.is_empty() && !m.turn_end)
                    .map(|m| m.body.clone())
                    .collect::<Vec<_>>()
                    .join(" / ")
            });
            say(
                start,
                format!("turn ended; streamed rows seen: {saw_stream}; reply: {tail:?}"),
            );
            break;
        }
        if Instant::now() > deadline {
            return say(start, "FAIL the turn did not end within 5 minutes");
        }
    }
    let wt = state
        .read_with(cx, |s, _| {
            s.workspace(&ws_id).map(|w| w.worktree_path.clone())
        })
        .unwrap_or_default();
    match std::fs::read_to_string(Path::new(&wt).join("hello.txt")) {
        Ok(s) => say(
            start,
            format!("hello.txt in the worktree: {:?}", s.trim_end()),
        ),
        Err(err) => say(start, format!("hello.txt missing: {err}")),
    }

    // 5. Open the diff: the Observatory's Review changes surface.
    let obs = view.read_with(cx, |v, _| v.observatory().clone());
    obs.update(cx, |o, cx| o.click_review_toggle(cx));
    for _ in 0..8 {
        sleep(cx, 250).await;
        frame(cx, window);
    }
    let showing = obs.read_with(cx, |o, cx| o.showing_review(cx));
    let svc = services.clone();
    let id = ws_id.clone();
    let diff = on_rt(
        &rt,
        async move { svc.code.diff_vs_base(&project_id, &id).await },
    )
    .await;
    match diff {
        Ok(d) => say(
            start,
            format!(
                "review surface showing: {showing}; diff files: {:?}",
                d.files
                    .iter()
                    .flatten()
                    .map(|f| f.path.clone())
                    .collect::<Vec<_>>()
            ),
        ),
        Err(err) => say(
            start,
            format!("review surface showing: {showing}; diff error: {err}"),
        ),
    }

    // 6. Quit (⌘Q is the menu's; the app quits through the same call).
    say(start, "quitting");
    let _ = cx.update_window(window, |_, window, cx| {
        let f = shell.read(cx).focus_handle(cx);
        window.focus(&f, cx);
    });
}
