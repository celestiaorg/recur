//! The engine-backed services: Go `internal/app` over [`recur_engine::Engine`].
//!
//! [`AppServices`] is the composition root (Go `NewServices` plus the
//! provider, forge and tracker wiring `engine.New` did): it registers the
//! shipped agent providers (Claude Code, Codex, Cursor and one compatible
//! provider per configured endpoint), the GitHub forge and the GitHub Issues
//! and Linear trackers under their persisted ids, builds the engine, bridges
//! its bus onto [`AppEvent`]s, and answers every trait in [`crate::api`].
//!
//! Go read `store.DataDir()` and friends from the process environment; here
//! the paths arrive in [`AppConfig`] so services built for tests never touch
//! the user's data.

mod assets;
mod bridge;
mod bundle;
mod code;
mod editor_session;
mod issues;
mod lsp;
mod model_catalog;
mod open_path;
mod projects;
mod prs;
mod rt;
mod settings;
mod shell;
mod simple;
mod terminal_persist;
mod terminals;
mod workspaces;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use recur_agent::Provider;
use recur_bundles::DiscoverRoots;
use recur_domain::{PROVIDER_CURSOR, Settings};
use recur_engine::bus::{EngineEvent, Notify};
use recur_engine::registry::{ForgeRegistry, ProviderRegistry, TrackerRegistry};
use recur_engine::{Engine, GitSetup};
use recur_forge::Forge;
use recur_issues::IssueTracker;
use recur_provider_claude::{ClaudeConfig, ClaudeProvider};
use recur_provider_codex::{CodexConfig, CodexProvider};
use recur_provider_compatible::CompatibleProvider;
use recur_provider_cursor::{CursorConfig, CursorProvider};
use recur_store::Store;
use recur_usage::UsageService;

use crate::events::{AppEvent, EventHub};
use crate::{Error, Services};

pub use code::LooseFiles;
pub use editor_session::merge_editor_session;
pub use shell::{clamp_shell_state, known_shell_view};
pub use terminal_persist::persist_delay;

/// Where the services keep their files. [`AppConfig::from_env`] is the
/// app's: the managed data root and the bundle roots Go resolved from the
/// environment.
#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    /// The managed root (`recur.db`, `workspaces/<id>/…`, view-state JSON).
    pub data_dir: PathBuf,
    /// The git worktrees parent.
    pub worktrees_dir: PathBuf,
    /// Shipped (`<data>/Bundles`) and user (`~/.recur/bundles`) bundles.
    pub bundles: DiscoverRoots,
    /// Compiled TypeScript macros (`<data>/BundleCache`).
    pub bundles_cache_dir: PathBuf,
    /// The home holding `~/.cursor/projects` (`None` = `$HOME`).
    pub home: Option<PathBuf>,
}

impl AppConfig {
    /// The app's paths, from `RECUR_DATA_DIR` / `RECUR_WORKTREES_DIR` and the
    /// home directory, created if needed.
    pub fn from_env() -> std::io::Result<Self> {
        use recur_store::paths;
        Ok(Self {
            data_dir: paths::data_dir()?,
            worktrees_dir: paths::worktrees_dir()?,
            bundles: DiscoverRoots {
                user_dir: paths::user_bundles_dir()?.to_string_lossy().into_owned(),
                default_dir: paths::bundles_dir()?.to_string_lossy().into_owned(),
            },
            bundles_cache_dir: paths::bundles_cache_dir()?,
            home: None,
        })
    }

    /// Every path under one root (tests and throwaway instances).
    pub fn under(root: &Path) -> Self {
        Self {
            data_dir: root.to_path_buf(),
            worktrees_dir: root.join("worktrees"),
            bundles: DiscoverRoots {
                user_dir: root.join("user-bundles").to_string_lossy().into_owned(),
                default_dir: root.join("Bundles").to_string_lossy().into_owned(),
            },
            bundles_cache_dir: root.join("BundleCache"),
            home: Some(root.join("home")),
        }
    }
}

/// Overrides for tests. Production uses the defaults.
#[derive(Default)]
pub struct AppOptions {
    /// Every turn goes to this provider (Go `SetRunner`); `Some(None)` fails
    /// turns closed.
    pub pin: Option<Option<Arc<dyn Provider>>>,
    /// Replaces the GitHub forge.
    pub forges: Option<Vec<Arc<dyn Forge>>>,
    /// Replaces the GitHub Issues and Linear trackers.
    pub trackers: Option<Vec<Arc<dyn IssueTracker>>>,
    /// The worktree root and boot git alert; `None` probes git.
    pub git: Option<GitSetup>,
}

/// The shipped providers, kept typed for the Settings probes the
/// [`Provider`] trait does not carry (Cursor install, the Go status shapes).
struct Providers {
    claude: Arc<ClaudeProvider>,
    cursor: Arc<CursorProvider>,
    codex: Arc<CodexProvider>,
    /// One per Settings compatible endpoint, by id, with the name its picker
    /// label was built from.
    compatible: Mutex<Vec<(String, String, Arc<CompatibleProvider>)>>,
}

/// Every service over one engine.
pub struct AppServices {
    me: std::sync::Weak<AppServices>,
    handle: tokio::runtime::Handle,
    pub(crate) engine: Arc<Engine>,
    pub(crate) hub: EventHub,
    pub(crate) config: AppConfig,
    providers: Providers,
    pub(crate) term: recur_term::Manager,
    pub(crate) term_persist: terminal_persist::Persist,
    pub(crate) restore_mu: tokio::sync::Mutex<()>,
    pub(crate) lsp: recur_lsp::Manager,
    pub(crate) index: recur_code::IndexHub,
    pub(crate) loose: LooseFiles,
    pub(crate) editor: editor_session::Sessions,
    pub(crate) shell: shell::ShellStore,
    pub(crate) catalog: bundle::CatalogCache,
    pub(crate) usage: Arc<UsageService>,
    /// Serializes `OpenPath` (Go `openMu`).
    pub(crate) open_mu: tokio::sync::Mutex<()>,
}

/// A Go `error` as the service error the UI shows.
pub(crate) fn err(e: impl std::fmt::Display) -> Error {
    Error::from_display(e)
}

impl AppServices {
    /// Builds the services over `store` (Go `engine.New` + `NewServices`).
    /// Must run inside the tokio runtime the backend uses: the file index,
    /// terminals and language servers start their tasks on it. Call
    /// [`AppServices::start`] once the UI is listening.
    pub async fn new(store: Arc<Store>, config: AppConfig, opts: AppOptions) -> Arc<Self> {
        let hub = EventHub::new();
        let settings = store.settings();
        let trackers = TrackerRegistry::new(opts.trackers.unwrap_or_else(|| {
            vec![
                Arc::new(recur_issues::github::GitHubIssues::new()) as Arc<dyn IssueTracker>,
                Arc::new(recur_issues::linear::Linear::new()),
            ]
        }));
        let env_scrub = trackers.secret_env();
        let providers = Providers {
            claude: Arc::new(ClaudeProvider::new(ClaudeConfig::default())),
            cursor: Arc::new(CursorProvider::new(CursorConfig {
                data_dir: config.data_dir.clone(),
                settings: settings.clone(),
                home: config.home.clone(),
            })),
            codex: Arc::new(CodexProvider::new(CodexConfig {
                data_dir: config.data_dir.clone(),
                home: None,
                settings: settings.clone(),
                env_scrub,
                seed_auth_from: None,
            })),
            compatible: Mutex::new(Vec::new()),
        };
        providers.sync_compatible(&settings, &config.data_dir);
        let registry = ProviderRegistry::new(providers.all());
        if let Some(pin) = opts.pin {
            registry.pin(pin);
        }
        let forges = ForgeRegistry::new(opts.forges.unwrap_or_else(|| {
            vec![Arc::new(recur_forge::github::GitHub::new()) as Arc<dyn Forge>]
        }));
        let git = match opts.git {
            Some(g) => g,
            None => GitSetup::probe(config.worktrees_dir.to_string_lossy().into_owned()).await,
        };

        // Terminal output rides the same event stream as everything else
        // (Go: the bus), coalesced into ~8ms chunks by the term package.
        let term = {
            let hub = hub.clone();
            recur_term::Manager::new(move |ev| hub.publish(AppEvent::Term(ev)))
        };
        let lsp = {
            let hub = hub.clone();
            recur_lsp::Manager::new(
                Some(Arc::new(move |ev: recur_lsp::LspEvent| {
                    hub.publish(match ev {
                        recur_lsp::LspEvent::Message { id, message } => {
                            AppEvent::LspMessage(crate::events::LspMessage { id, message })
                        }
                        recur_lsp::LspEvent::Exit { id } => {
                            AppEvent::LspExit(crate::events::LspExit { id })
                        }
                    })
                })),
                config.data_dir.to_string_lossy().into_owned(),
            )
        };
        // Status only: search ranks in the backend (SearchFiles); the path
        // corpus never ships to the UI.
        let index = {
            let hub = hub.clone();
            recur_code::IndexHub::new(
                &config.data_dir.to_string_lossy(),
                Some(Arc::new(move |workspace_id: &str, snap| {
                    hub.publish(AppEvent::WorkspaceFileIndex(
                        crate::events::WorkspaceFileIndex {
                            workspace_id: workspace_id.to_owned(),
                            r#gen: snap.generation,
                            ready: snap.ready,
                            building: snap.building,
                        },
                    ))
                })),
            )
        };

        let engine = Engine::new(
            Arc::clone(&store),
            Arc::new(recur_engine::Bus::new()),
            git,
            config.bundles.clone(),
            Some(Arc::new(term.clone()) as Arc<dyn recur_engine::terminals::TerminalSource>),
            registry,
            forges,
            trackers,
            config.data_dir.clone(),
        );

        let svc = Arc::new_cyclic(|weak: &std::sync::Weak<AppServices>| {
            let usage = {
                let (read, save, hub) = (Arc::clone(&store), weak.clone(), hub.clone());
                let data_dir = config.data_dir.clone();
                UsageService::new(
                    Arc::new(move || read.settings()),
                    Arc::new(move |set: Settings| {
                        let save = save.clone();
                        Box::pin(async move {
                            match save.upgrade() {
                                Some(s) => s
                                    .save_settings(set)
                                    .await
                                    .map(|_| ())
                                    .map_err(|e| e.message),
                                None => Ok(()),
                            }
                        })
                    }),
                    Arc::new(move || Some(data_dir.clone())),
                    Some(Arc::new(move |_name: &'static str, snap| {
                        hub.publish(AppEvent::ProviderUsage(snap))
                    })),
                )
            };
            AppServices {
                me: weak.clone(),
                handle: tokio::runtime::Handle::current(),
                engine,
                hub: hub.clone(),
                config,
                providers,
                term_persist: terminal_persist::Persist::new(weak.clone()),
                term,
                restore_mu: tokio::sync::Mutex::new(()),
                lsp,
                index,
                loose: LooseFiles::default(),
                editor: editor_session::Sessions::default(),
                shell: shell::ShellStore::default(),
                catalog: bundle::CatalogCache::default(),
                usage,
                open_mu: tokio::sync::Mutex::new(()),
            }
        });
        {
            let weak = Arc::downgrade(&svc);
            svc.term.set_on_dirty(move |workspace_id| {
                if let Some(s) = weak.upgrade() {
                    s.term_persist.mark_dirty(workspace_id);
                }
            });
        }
        bridge::attach(&svc);
        svc
    }

    /// The app's services (production paths and registrations).
    pub async fn open(store: Arc<Store>) -> std::io::Result<Arc<Self>> {
        Ok(Self::new(store, AppConfig::from_env()?, AppOptions::default()).await)
    }

    /// The handle views hold.
    pub fn services(self: &Arc<Self>) -> Services {
        Services::new(Arc::clone(self), self.hub.clone())
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    pub fn events(&self) -> &EventHub {
        &self.hub
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Restart recovery (Go `eng.Recover()`): interrupted loops park, zombie
    /// chats heal. Run before the UI loads.
    pub fn recover(&self) {
        self.engine.recover();
    }

    /// The boot work `main.go` ran once the bus reached the UI: the git and
    /// Cursor bridge checks, scheduled prompts, auto-resume, the PR sweep and
    /// the provider usage poller.
    pub async fn start(self: &Arc<Self>) {
        self.engine.check_git();
        self.check_cursor_bridge();
        if let Err(e) = self.engine.start_scheduled_prompts().await {
            tracing::warn!("scheduled prompts: {e}");
        }
        let eng = Arc::clone(&self.engine);
        tokio::spawn(async move { eng.auto_resume().await });
        let eng = Arc::clone(&self.engine);
        tokio::spawn(async move { eng.run_pr_sweep().await });
        self.usage.start();
    }

    /// The window lost or regained key status (Go `eng.SetAppFocused`).
    pub fn set_app_focused(&self, focused: bool) {
        self.engine.set_app_focused(focused);
    }

    /// The dock badge count (Go `eng.AttentionCounts().Dock`).
    pub fn dock_count(&self) -> i64 {
        self.engine.attention_counts().dock
    }

    /// App teardown (Go `eng.Shutdown`, the services' `ServiceShutdown`
    /// hooks and `StopProviderUsage`): loops and chats are cancelled and
    /// joined, terminal tabs are persisted and their process groups killed,
    /// language servers stop, the usage poller stops and the last shell
    /// snapshot is flushed.
    pub async fn shutdown(&self) {
        self.engine.shutdown().await;
        self.terminal_shutdown().await;
        self.lsp.close_all();
        self.usage.stop();
        self.shell_flush();
    }

    /// Go `CheckCursorBridge`: a bridge behind the pinned cursor-sdk
    /// announces itself at boot, since nobody opens Settings to find out why
    /// Cursor turns started failing.
    pub fn check_cursor_bridge(&self) {
        let Ok(bin) = recur_provider_cursor::locate(&self.config.data_dir) else {
            return;
        };
        let have = recur_provider_cursor::sdk_version(&bin);
        let want = recur_provider_cursor::PINNED_SDK_VERSION;
        if have.is_empty() || have == want {
            return;
        }
        tracing::info!("cursor: bridge sdk {have} installed, {want} pinned");
        self.engine.bus.publish(EngineEvent::Notify(Notify {
            title: "Cursor bridge update available".into(),
            body: format!(
                "Installed cursor-sdk {have}, Recur expects {want}. Update it in Settings > Providers > Cursor."
            ),
            target: None,
        }));
    }

    pub(crate) fn store(&self) -> &Store {
        &self.engine.store
    }

    pub(crate) fn data_dir(&self) -> &Path {
        &self.config.data_dir
    }
}

impl Providers {
    fn all(&self) -> Vec<Arc<dyn Provider>> {
        let mut out: Vec<Arc<dyn Provider>> = vec![
            self.claude.clone(),
            self.cursor.clone(),
            self.codex.clone(),
        ];
        for (_, _, p) in self.compatible.lock().iter() {
            out.push(p.clone());
        }
        out
    }

    /// Keeps one provider per Settings compatible endpoint. A provider whose
    /// name changed is rebuilt (its picker label is fixed at construction);
    /// the rest keep their instance. Returns whether the set changed.
    fn sync_compatible(&self, set: &Settings, data_dir: &Path) -> bool {
        let mut cur = self.compatible.lock();
        let next: Vec<(String, String, Arc<CompatibleProvider>)> = set
            .compatible_providers
            .iter()
            .map(|c| {
                match cur
                    .iter()
                    .find(|(id, name, _)| *id == c.id && *name == c.name)
                {
                    Some(kept) => kept.clone(),
                    None => (
                        c.id.clone(),
                        c.name.clone(),
                        Arc::new(
                            CompatibleProvider::new(c.clone()).with_data_dir(data_dir.to_owned()),
                        ),
                    ),
                }
            })
            .collect();
        let changed = next.len() != cur.len()
            || next
                .iter()
                .zip(cur.iter())
                .any(|(a, b)| !Arc::ptr_eq(&a.2, &b.2));
        *cur = next;
        changed
    }

    fn cursor_enabled(set: &Settings) -> bool {
        set.provider_enabled(PROVIDER_CURSOR)
    }
}
