//! `recur_agent::Provider` for Cursor: the bridge pool, the turn router's
//! Cursor self-heal (Go `runAgent`), the Settings probes, and the catalog.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use recur_agent::{
    Availability, CredentialDraft, ErrorKind, Flags, HistoryMode, McpTransports, ModelChoice,
    PermissionRoute, Provider, ProviderCaps, ProviderError, ProviderStatus, SteerSupport,
    ToolDisplay, ToolTransport, TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{PROVIDER_CURSOR, Settings, provider_label};
use tokio_util::sync::CancellationToken;

use crate::attribution::{attribution_config_path, write_attribution_config};
use crate::catalog::cursor_catalog_models;
use crate::client::Model;
use crate::connect::Error;
use crate::context::resolve_context_window;
use crate::install::{InstallProgress, install};
use crate::locate::{locate, sdk_version};
use crate::pool::{Pool, PoolError};
use crate::projectdirs::remove_cursor_project_dirs_in;
use crate::runner::RunFailure;
use crate::status::{
    CursorStatus, apply_cursor_pin, apply_cursor_probe, has_recur_cursor_auth_file, probe_cursor,
};
use crate::tools::{cursor_tool_result_text, display_tool};

const CAPS: ProviderCaps = ProviderCaps {
    // No prompt tool: the sandbox is off and tools are pre-scoped.
    permission: PermissionRoute::None,
    steer: SteerSupport::LiveOrRestart,
    history: HistoryMode::Native,
    tool_transport: ToolTransport::Mcp,
    native_fork: false,
    images: true,
    // createPlan and the built-ins cannot be turned off: one-shot turns get
    // Read/Glob/Grep instead.
    can_disable_tools: false,
    max_turns: false,
    partials: true,
    user_mcp: McpTransports {
        http: true,
        stdio: true,
    },
};

/// How the provider is wired.
#[derive(Debug, Clone, Default)]
pub struct CursorConfig {
    /// Recur's data dir: `cursor-bridge/`, the attribution config, the
    /// Connect session file, and `logs/cursor-bridges/<key>/`.
    pub data_dir: PathBuf,
    /// The settings at startup; later ones arrive via `settings_changed`.
    pub settings: Settings,
    /// The home holding `~/.cursor/projects` (`None` = `$HOME`).
    pub home: Option<PathBuf>,
}

/// The Cursor provider.
pub struct CursorProvider {
    data_dir: PathBuf,
    home: Option<PathBuf>,
    settings: RwLock<Settings>,
    pool: Arc<Pool>,
    /// The last good ListModels result (offline catalog).
    catalog: Mutex<Option<Vec<Model>>>,
}

impl CursorProvider {
    pub fn new(cfg: CursorConfig) -> Self {
        let logs = if cfg.data_dir.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            cfg.data_dir.join("logs")
        };
        Self {
            pool: Arc::new(Pool::new(cfg.data_dir.clone(), logs, cfg.home.clone())),
            data_dir: cfg.data_dir,
            home: cfg.home,
            settings: RwLock::new(cfg.settings),
            catalog: Mutex::new(None),
        }
    }

    fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    /// The Settings card in Go's `CursorStatus` JSON shape.
    pub async fn cursor_status(&self, settings: &Settings) -> CursorStatus {
        let bin = match locate(&self.data_dir) {
            Ok(b) => b,
            Err(e) => {
                return CursorStatus {
                    error: e,
                    ..Default::default()
                };
            }
        };
        let mut out = CursorStatus {
            bridge_installed: true,
            bridge_path: bin.to_string_lossy().into_owned(),
            bridge_version: sdk_version(&bin),
            ..Default::default()
        };
        if !has_recur_cursor_auth_file(&self.data_dir) || settings.cursor_api_key.is_empty() {
            apply_cursor_pin(&mut out, &self.data_dir, &bin);
            return out;
        }
        match probe_cursor(&bin, &settings.cursor_api_key).await {
            Err(e) => out.error = e,
            Ok(p) => {
                apply_cursor_probe(&mut out, &p);
                *self.catalog.lock() = Some(p.models);
            }
        }
        apply_cursor_pin(&mut out, &self.data_dir, &bin);
        out
    }

    /// Validates a key that has not been saved; empty falls back to the
    /// saved one.
    pub async fn test_cursor_key(&self, settings: &Settings, api_key: &str) -> CursorStatus {
        let key = if api_key.is_empty() {
            settings.cursor_api_key.as_str()
        } else {
            api_key
        };
        if key.is_empty() {
            return CursorStatus {
                error: "Enter an API key first.".into(),
                ..Default::default()
            };
        }
        let bin = match locate(&self.data_dir) {
            Ok(b) => b,
            Err(e) => {
                return CursorStatus {
                    error: e,
                    ..Default::default()
                };
            }
        };
        let mut out = CursorStatus {
            bridge_installed: true,
            bridge_path: bin.to_string_lossy().into_owned(),
            bridge_version: sdk_version(&bin),
            ..Default::default()
        };
        match probe_cursor(&bin, key).await {
            Err(e) => out.error = e,
            Ok(p) => apply_cursor_probe(&mut out, &p),
        }
        apply_cursor_pin(&mut out, &self.data_dir, &bin);
        out
    }

    /// Downloads and unpacks the pinned bridge into the data dir, returning
    /// the launcher. `progress` feeds the `cursor.install` event
    /// ([`InstallProgress::event_payload`]); on failure publish
    /// [`crate::install_error_payload`]. The caller records the launcher in
    /// `Settings.cursor_bridge_path` (display only).
    pub async fn install(
        &self,
        cancel: &CancellationToken,
        progress: &(dyn Fn(InstallProgress) + Send + Sync),
    ) -> Result<PathBuf, String> {
        let bin = install(&self.data_dir, cancel, progress).await?;
        let set = self.settings();
        if let Some(path) = attribution_config_path(&self.data_dir)
            && let Err(e) = write_attribution_config(
                &path,
                set.cursor_attribute_commits,
                set.cursor_attribute_prs,
            )
        {
            tracing::warn!("recur: warning: write cursor attribution config: {e}");
        }
        Ok(bin)
    }

    async fn run_once(
        &self,
        req: &TurnRequest,
        cx: &mut TurnContext,
        set: &Settings,
    ) -> Result<Result<TurnResult, RunFailure>, PoolError> {
        let res = self
            .pool
            .ensure_runner(&cx.cancel, set, &req.worktree)
            .await?;
        let runner = res.runner.clone();
        let out = runner
            .run(req, &cx.cancel, cx.sink.clone(), &mut cx.steer)
            .await;
        drop(res);
        Ok(out)
    }
}

fn unavailable(e: PoolError) -> ProviderError {
    ProviderError::new(ErrorKind::Unavailable, e.0)
}

fn failure(cancel: &CancellationToken, f: RunFailure) -> ProviderError {
    if f.err.is_canceled() || cancel.is_cancelled() {
        return ProviderError::canceled().with_partial(f.partial);
    }
    let kind = match f.err.connect() {
        Some(c) if c.transient() => ErrorKind::Transient,
        Some(c) if c.unauthenticated() => ErrorKind::Auth,
        _ => ErrorKind::Failed,
    };
    ProviderError::new(kind, f.err.to_string()).with_partial(f.partial)
}

/// A failure before any run-stream event could be emitted (CreateAgent or
/// ResumeAgent), as opposed to a mid-Send transport drop.
pub(crate) fn cursor_died_before_turn(err: &Error) -> bool {
    err.to_string().contains("cursor create agent:")
}

#[async_trait::async_trait]
impl Provider for CursorProvider {
    fn id(&self) -> &str {
        PROVIDER_CURSOR
    }

    fn label(&self) -> &str {
        provider_label(PROVIDER_CURSOR)
    }

    fn caps(&self) -> &ProviderCaps {
        &CAPS
    }

    fn availability(&self, settings: &Settings) -> Availability {
        if !settings.provider_enabled(PROVIDER_CURSOR) {
            return Availability::Unavailable(
                "Cursor is turned off in Settings → Providers".into(),
            );
        }
        if !settings.cursor_configured() {
            return Availability::Unavailable(
                "a role is set to a Cursor model but Cursor is not connected (Settings → Providers → Connect)".into(),
            );
        }
        Availability::Ready
    }

    async fn status(&self, settings: &Settings) -> ProviderStatus {
        self.cursor_status(settings).await.into()
    }

    async fn test_credentials(
        &self,
        settings: &Settings,
        draft: &CredentialDraft,
    ) -> ProviderStatus {
        self.test_cursor_key(settings, &draft.api_key).await.into()
    }

    /// Live ListModels through a throwaway bridge. Empty when Cursor is off,
    /// unconnected or not installed (no group); the cache only takes a
    /// successful list.
    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        if !settings.provider_enabled(PROVIDER_CURSOR) || !settings.cursor_configured() {
            return Ok(Vec::new());
        }
        let Ok(bin) = locate(&self.data_dir) else {
            return Ok(Vec::new());
        };
        let models = crate::status::list_models(&bin, &settings.cursor_api_key)
            .await
            .map_err(ProviderError::failed)?;
        let out = cursor_catalog_models(&models);
        *self.catalog.lock() = Some(models);
        Ok(out)
    }

    fn cached_models(&self, settings: &Settings) -> Vec<ModelChoice> {
        if !settings.provider_enabled(PROVIDER_CURSOR) {
            return Vec::new();
        }
        self.catalog
            .lock()
            .as_deref()
            .map(cursor_catalog_models)
            .unwrap_or_default()
    }

    /// Flags travel as ModelSelection params; the id is unchanged.
    fn resolve_model(&self, model: &str, _flags: &Flags) -> String {
        model.to_owned()
    }

    fn context_window(&self, model: &str, flags: &Flags) -> i64 {
        resolve_context_window(model, flags)
    }

    fn model_label(&self, model: &str) -> String {
        model.to_owned()
    }

    /// No default effort: Cursor flags are per model from ListModels.
    fn oneshot_flags(&self, _model: &str) -> Flags {
        Flags::new()
    }

    /// One turn, with the router's Cursor self-heal: a bridge that died
    /// before anything streamed (CreateAgent refused) is replaced and the
    /// turn retried once. A mid-stream drop also replaces the bridge but is
    /// not retried: events may already be mirrored.
    async fn run_turn(
        &self,
        req: TurnRequest,
        mut cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        let set = self.settings();
        let first = self
            .run_once(&req, &mut cx, &set)
            .await
            .map_err(unavailable)?;
        let fail = match first {
            Ok(r) => return Ok(r),
            Err(f) => f,
        };
        if !self.settings().provider_enabled(PROVIDER_CURSOR)
            && !fail.err.is_canceled()
            && !cx.cancel.is_cancelled()
        {
            self.pool.invalidate(&req.worktree).await;
            return Err(ProviderError::new(
                ErrorKind::Unavailable,
                format!("Cursor is turned off in Settings → Providers: {}", fail.err),
            )
            .with_partial(fail.partial));
        }
        if !fail.err.bridge_unreachable() {
            return Err(failure(&cx.cancel, fail));
        }
        self.pool.invalidate(&req.worktree).await;
        if !cursor_died_before_turn(&fail.err) {
            return Err(failure(&cx.cancel, fail));
        }
        // Nothing streamed yet: safe to retry once on a fresh bridge.
        let set = self.settings();
        match self.run_once(&req, &mut cx, &set).await {
            Err(e) => Err(failure(
                &cx.cancel,
                RunFailure {
                    err: fail
                        .err
                        .suffix(format!(" (bridge restart failed: {})", e.0)),
                    partial: fail.partial,
                },
            )),
            Ok(Ok(r)) => Ok(r),
            Ok(Err(f)) => Err(failure(&cx.cancel, f)),
        }
    }

    /// Cursor agent ids are `agent-…`; anything else belongs to another
    /// provider. Empty always fits.
    fn resume_handle_fits(&self, handle: &str) -> bool {
        let h = handle.trim();
        h.is_empty() || h.starts_with("agent-")
    }

    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        display_tool(name, input)
    }

    fn display_tool_result(&self, name: &str, result: &str) -> String {
        cursor_tool_result_text(name, result)
    }

    /// Drops the worktree's bridge before its directory goes away, then its
    /// `~/.cursor/projects/<slug>` tree. The error is the sentence the
    /// workspace feed shows.
    async fn release_worktree(&self, worktree: &Path) -> Result<(), ProviderError> {
        self.pool.invalidate(worktree).await;
        remove_cursor_project_dirs_in(self.home.as_deref(), worktree).map_err(|e| {
            ProviderError::failed(format!("Could not remove Cursor project dirs: {e}"))
        })
    }

    /// Agent slots went to zero: finish a deferred close.
    async fn on_idle(&self) {
        self.pool.close_if_idle_requested(true).await;
    }

    /// Keeps the settings the next turn reads, rewrites the attribution file
    /// warm bridges read every turn, and closes the pool when Cursor is
    /// turned off (deferred while a Cursor turn runs).
    async fn settings_changed(&self, old: &Settings, new: &Settings) {
        *self.settings.write() = new.clone();
        if let Some(path) = attribution_config_path(&self.data_dir)
            && let Err(e) = write_attribution_config(
                &path,
                new.cursor_attribute_commits,
                new.cursor_attribute_prs,
            )
        {
            // Warm bridges keep stale toggles until the next cold start.
            tracing::warn!("recur: warning: write cursor attribution config: {e}");
        }
        if old.provider_enabled(PROVIDER_CURSOR) && !new.provider_enabled(PROVIDER_CURSOR) {
            self.pool.close_bridges().await;
        } else if new.provider_enabled(PROVIDER_CURSOR) {
            self.pool.cancel_close_when_idle().await;
        }
    }

    async fn shutdown(&self) {
        self.pool.close().await;
    }
}

#[cfg(test)]
mod tests;
