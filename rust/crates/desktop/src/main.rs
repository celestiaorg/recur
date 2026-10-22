//! The desktop app entry (root `main.go` and `open_boot.go`): own the data dir or hand the
//! launch to the running instance, tee the log, fix PATH, open the store, seed shipped bundles
//! and skills, start the runtime, open IPC, build the services and recover interrupted work
//! before any UI, start the boot work once events reach the desktop, then run the GPUI app and
//! tear down in the Go app's order.
//!
//! The `mock` feature swaps in the e2e mock bridge for UI work without a backend.

mod macro_runtime;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::{App, AsyncApp};
use recur_domain::OpenPathResult;
use recur_platform::open_boot::{
    Claimed, IpcGate, OpenHub, apply_cold_opens, attach_open_ipc, claim_or_handoff, peel_open_args,
    revoke_open_ui, start_open_ipc,
};
use recur_platform::openipc::Response;
use recur_platform::updater::{Host, RecurUpdater, UpdaterEvent, UserAction};
use recur_platform::{cliinstall, dock, notify, running_in_app_bundle};
use recur_services::{AppEvent, AppServices, Services};
use recur_view_core::{LOCAL_STORAGE_FILE, LocalStorage, ViewRegistry};
use recur_view_observatory::lsp::problems;

/// Stamped at build time for release bundles (Go: `-ldflags -X main.version`).
const VERSION: &str = match option_env!("RECUR_VERSION") {
    Some(v) => v,
    None => "",
};
/// The signed release manifest (Go: `main.updateManifestURL`).
const MANIFEST_URL: &str = match option_env!("RECUR_UPDATE_MANIFEST_URL") {
    Some(v) => v,
    None => "",
};

/// The services the app runs on: the engine-backed ones over `store`, recovered (Go
/// `eng.Recover()` before `NewServices`), or the mock bridge under the `mock` feature.
#[cfg(not(feature = "mock"))]
fn backend(
    rt: &tokio::runtime::Runtime,
    store: recur_store::Store,
) -> (Services, Option<Arc<AppServices>>) {
    let svc = rt
        .block_on(AppServices::open(Arc::new(store)))
        .unwrap_or_else(|err| fatal(format!("resolve data dir: {err}")));
    svc.recover();
    (svc.services(), Some(svc))
}

#[cfg(feature = "mock")]
fn backend(
    _rt: &tokio::runtime::Runtime,
    _store: recur_store::Store,
) -> (Services, Option<Arc<AppServices>>) {
    let bridge =
        recur_services::mock::MockBridge::fresh(recur_services::mock::MockOptions::default());
    (bridge.services(), None)
}

/// Every view crate registers its routes, overlays and keymaps here.
fn registry() -> ViewRegistry {
    let mut r = ViewRegistry::default();
    recur_view_chat::register(&mut r);
    recur_view_review::register(&mut r);
    recur_view_observatory::register(&mut r);
    recur_view_workspace::register(&mut r);
    r
}

/// An OpenPath result as the IPC wire reply.
#[derive(Clone)]
struct Opened(OpenPathResult);

impl From<Opened> for Response {
    fn from(o: Opened) -> Response {
        let r = o.0;
        Response {
            path: r.path,
            project_id: r.project_id,
            workspace_id: r.workspace_id,
            file: r.file,
            created: r.created,
            loose: r.loose,
            ..Response::default()
        }
    }
}

/// Work posted to the GPUI main thread from other threads.
type MainJob = Box<dyn FnOnce(&mut App) + Send>;

/// The updater's host: events go to the UI as toasts, quit runs on the main thread.
struct UpdaterHost {
    services: Services,
    main: UnboundedSender<MainJob>,
}

impl Host for UpdaterHost {
    fn emit(&self, event: UpdaterEvent) {
        self.services.emit(AppEvent::Updater(event));
    }

    fn quit(&self) {
        let _ = self.main.unbounded_send(Box::new(|cx: &mut App| cx.quit()));
    }
}

fn fatal(msg: impl std::fmt::Display) -> ! {
    tracing::error!("{msg}");
    eprintln!("{msg}");
    std::process::exit(1);
}

fn init_logging(data_dir: &Path) -> recur_store::applog::AppLogGuard {
    let (writer, guard) = recur_store::applog::tee_app_log(data_dir);
    let fmt = tracing_subscriber::fmt().event_format(recur_store::applog::GoLogFormat);
    let _ = match writer {
        Some(w) => fmt.with_writer(w).try_init(),
        None => fmt.with_writer(std::io::stderr).try_init(),
    };
    guard
}

fn seed_shipped() {
    let seed = |dir: std::io::Result<PathBuf>, src: &dyn recur_bundles::SeedSource, what: &str| {
        let Ok(dir) = dir else { return };
        if let Err(err) = recur_bundles::seed_defaults(&dir.to_string_lossy(), src, ".") {
            tracing::warn!("seed {what}: {err}");
        }
    };
    // User bundles live in ~/.recur/bundles; project and user skills win on name collision.
    seed(
        recur_store::paths::bundles_dir(),
        &recur_bundles::SHIPPED_BUNDLES,
        "default bundles",
    );
    seed(
        recur_store::paths::recur_skills_dir(),
        &recur_bundles::SHIPPED_SKILLS,
        "recur skills",
    );
}

fn main() {
    // The updater re-execs this binary as the swap helper while the instance lock is held.
    recur_platform::updater::handle_helper_mode();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cold_opens, _) = peel_open_args(&args);
    let data_dir = recur_store::paths::data_dir()
        .unwrap_or_else(|err| fatal(format!("resolve data dir: {err}")));

    // Own the data dir (flock + socket) before the store opens; a second owner would
    // re-recover in-flight loops.
    let mut inst = match claim_or_handoff(&data_dir, &cold_opens) {
        Ok(Claimed::Owner(inst)) => inst,
        Ok(Claimed::HandedOff) => return,
        Err(err) => fatal(err),
    };
    let gate = Arc::new(IpcGate::default());
    let hub: Arc<OpenHub<Opened>> = OpenHub::new();

    let _log = init_logging(&data_dir);

    // A Dock-launched app inherits launchd's PATH: probe the login shell before anything
    // looks up claude, gh, git… and before any thread exists (it sets the environment).
    cliinstall::ensure_path();

    let store = recur_store::Store::new().unwrap_or_else(|err| fatal(format!("open store: {err}")));

    // Files dropped last run stay readable (transcript chips, restored tabs).
    recur_code::load_external_allow_file(&data_dir.join("external-allow.json").to_string_lossy());
    seed_shipped();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|err| fatal(format!("start runtime: {err}")));
    let ipc = start_open_ipc(rt.handle(), &mut inst, gate.clone(), hub.clone());

    let (services, real) = backend(&rt, store);
    let open_path = {
        let (services, handle) = (services.clone(), rt.handle().clone());
        move |p: &str| {
            handle
                .block_on(services.workspaces.open_path(p))
                .map(Opened)
                .map_err(|err| err.to_string())
        }
    };
    attach_open_ipc(&gate, hub.clone(), open_path.clone());
    apply_cold_opens(&hub, &cold_opens, open_path);

    let (main_tx, mut main_rx) = unbounded::<MainJob>();
    let updater = {
        let _enter = rt.enter();
        RecurUpdater::install(
            VERSION,
            MANIFEST_URL,
            Arc::new(UpdaterHost {
                services: services.clone(),
                main: main_tx.clone(),
            }),
        )
    }
    .map(Arc::new);

    // Backend events the desktop owns: UI readiness, OS notifications, the dock badge, and
    // the updater's UI-to-backend events.
    let dock_live = Arc::new(AtomicBool::new(false));
    rt.spawn({
        let (services, hub, dock_live, updater) = (
            services.clone(),
            hub.clone(),
            dock_live.clone(),
            updater.clone(),
        );
        async move {
            let mut rx = services.subscribe();
            while let Some(ev) = rx.recv().await {
                match ev {
                    AppEvent::AppReady => hub.ui_ready_now(),
                    AppEvent::Notify(n) => {
                        tokio::task::spawn_blocking(move || {
                            notify::send_os_notification(&n.title, &n.body, &n.target)
                        });
                    }
                    AppEvent::InboxUpdated(_) | AppEvent::AttentionUpdated(_) => {
                        if dock_live.load(Ordering::SeqCst)
                            && let Ok(c) = services.workspaces.attention_counts().await
                        {
                            tokio::task::spawn_blocking(move || dock::sync_badge(c.dock));
                        }
                    }
                    AppEvent::UpdaterCheck => {
                        if let Some(u) = &updater {
                            u.check_now();
                        }
                    }
                    AppEvent::UpdaterUserRestart => {
                        if let Some(u) = &updater {
                            u.user_action(UserAction::Restart);
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    // The git and Cursor bridge checks, scheduled prompts, auto-resume, the PR sweep and the
    // usage poller, once the subscriber above forwards their notifications.
    if let Some(svc) = &real {
        rt.block_on(svc.start());
    }
    // Production .app only: symlink the bundled CLI onto PATH (D-cli-install).
    rt.spawn_blocking(cliinstall::install_bundled_cli);

    let app = gpui_kit::application().with_assets(recur_ui::Assets);
    {
        let main_tx = main_tx.clone();
        app.on_reopen(move |_| {
            let _ = main_tx.unbounded_send(Box::new(|cx: &mut App| bring_forward(cx)));
        });
    }
    let ipc = Rc::new(RefCell::new(Some(ipc)));
    // The runtime outlives the launch callback, which GPUI drops once it has run.
    let rt_handle = rt.handle().clone();
    app.run(move |cx: &mut App| {
        let rt = rt_handle;
        let registry = registry();
        recur_app::init(&registry, cx);
        let storage = LocalStorage::open(data_dir.join(LOCAL_STORAGE_FILE));
        let main = recur_app::MainWindow {
            services: services.clone(),
            storage,
            data_dir: Some(data_dir.clone()),
            registry,
            shell: recur_app::ShellOptions {
                updates: updater.is_some(),
                on_window_active: real.clone().map(|svc| {
                    Rc::new(move |active: bool| svc.set_app_focused(active)) as Rc<dyn Fn(bool)>
                }),
                ..Default::default()
            },
        };
        let (window, state) = recur_app::open_main_window(main, cx)
            .unwrap_or_else(|err| fatal(format!("open window: {err}")));
        let macros = macro_runtime::MacroRuntime::new(rt.clone());
        state.update(cx, |s, _| {
            s.set_drop_hook(Rc::new(|files: &[String]| {
                for f in files {
                    recur_code::allow_external_path(f);
                }
            }));
            s.set_macro_runner(Some(macros.macro_runner()));
        });
        // The runner has no `App`, so it reads a mirror of the editor's problem store.
        let problems: Rc<RefCell<Vec<problems::LspProblem>>> = Rc::default();
        cx.observe_global::<problems::LspProblems>({
            let problems = problems.clone();
            move |cx| *problems.borrow_mut() = problems::peek_problems(cx)
        })
        .detach();
        let diagnostics: macro_runtime::Diagnostics = Rc::new(move |ws| {
            problems::problems_for_workspace(&problems.borrow(), ws)
                .iter()
                .map(problems::problem_args)
                .collect()
        });
        recur_view_chat::prompt_actions::PromptCatalog::set_runners(
            Some(macros.prompt_runner()),
            Some(macros.complete_runner(diagnostics)),
            cx,
        );

        // Jobs from other threads (focus for a handoff, updater quit) run here.
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(job) = main_rx.next().await {
                cx.update(job);
            }
        })
        .detach();

        hub.set_hooks(
            Some({
                let services = services.clone();
                move |o: Opened| services.emit(AppEvent::AppOpen(o.0))
            }),
            Some({
                let main_tx = main_tx.clone();
                move |applied: Box<dyn FnOnce() + Send>| {
                    let _ = main_tx.unbounded_send(Box::new(move |cx: &mut App| {
                        bring_forward(cx);
                        applied();
                    }));
                }
            }),
        );
        hub.mark_app_live();

        // Notification clicks focus the window and route to the inbox target.
        if running_in_app_bundle() {
            if let Err(err) = notify::start() {
                tracing::warn!("notifications: {err}");
            }
            let (services, hub) = (services.clone(), hub.clone());
            notify::on_response(move |res| {
                let Ok(res) = res else { return };
                let hub = hub.clone();
                std::thread::spawn(move || hub.bring_forward());
                if !res.target().is_empty() {
                    services.emit(AppEvent::NotificationOpen(res.target().to_string()));
                }
            });
        }
        // After the run loop is live: notification and badge authorization, then the badge.
        rt.spawn({
            let (services, dock_live) = (services.clone(), dock_live.clone());
            async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                dock_live.store(true, Ordering::SeqCst);
                tokio::task::spawn_blocking(|| {
                    if running_in_app_bundle() && !notify::check_authorization().unwrap_or(false) {
                        let _ = notify::request_authorization();
                    }
                });
                if let Ok(c) = services.workspaces.attention_counts().await {
                    tokio::task::spawn_blocking(move || dock::sync_badge(c.dock));
                }
            }
        });

        // Closing the window: stop focus hooks and IPC admission, wait out an in-flight
        // focus, and flush the shell state before the window goes.
        {
            let (hub, gate, services, handle) =
                (hub.clone(), gate.clone(), services.clone(), rt.clone());
            let _ = window.update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |_, _| {
                    revoke_open_ui(&hub, &gate);
                    hub.wait_focus();
                    let _ = handle.block_on(services.shell.flush());
                    true
                });
            });
        }
        // The last window closing quits, as on macOS with
        // `ApplicationShouldTerminateAfterLastWindowClosed`.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        // Quit: drop the UI, stop accepting IPC and join in-flight opens, then cancel and join
        // every loop and chat, persist terminals and stop language servers and the usage
        // poller before the store closes (Go `stopIPC(); eng.Shutdown()` in OnShutdown). The
        // flock is released when the instance drops, after the run loop.
        let (hub, gate, ipc, real, handle) = (
            hub.clone(),
            gate.clone(),
            ipc.clone(),
            real.clone(),
            rt.clone(),
        );
        cx.on_app_quit(move |_| {
            revoke_open_ui(&hub, &gate);
            if let Some(ipc) = ipc.borrow_mut().take() {
                ipc.stop();
            }
            if let Some(svc) = &real {
                handle.block_on(svc.shutdown());
            }
            async {}
        })
        .detach();
    });
    drop(rt);
    drop(inst);
}

/// Un-minimise, show and focus the main window (`win.UnMinimise(); win.Show(); win.Focus()`).
fn bring_forward(cx: &mut App) {
    cx.activate(true);
    if let Some(w) = cx.windows().first() {
        let _ = w.update(cx, |_, window, _| window.activate_window());
    }
}
