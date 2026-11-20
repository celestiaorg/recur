//! In-app self-update: the updater drives check → download → verify →
//! install → restart and reports every step as an [`UpdaterEvent`]; the UI
//! renders them as toasts and answers with [`UserAction`]s.
//!
//! Providers only look up and stream a release; verification, atomic
//! staging, archive extraction, the swap and the restart helper live here.

mod endpoint;
mod extract;
mod helper;
pub mod manifest;
mod recur;
mod semver;
mod verify;

pub use endpoint::{EndpointConfig, EndpointProvider};
pub use helper::handle_helper_mode;
pub use recur::{RecurUpdater, SignedOnly};

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use verify::Hasher;

/// The updater's lifecycle phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Unconfigured,
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Verifying,
    Installing,
    Ready,
    Error,
}

/// Which phase produced an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Check,
    Download,
    Verify,
    Install,
}

/// What a provider's check returns when something newer exists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Release {
    pub version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub channel: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(with = "go_time")]
    pub published_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub artifact: Artifact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    /// Set by the updater after check (providers leave it empty) to route
    /// the download back to the same source.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub provider: String,
}

/// The file to download for the running platform.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Artifact {
    pub filename: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub filetype: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub size: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub platform: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub arch: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// How to authenticate the downloaded bytes. A digest must match; a
/// signature must verify under the configured public key (never one the
/// release supplies).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Verification {
    /// `sha256` or `sha512`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub digest_algo: String,
    #[serde(with = "go_bytes", skip_serializing_if = "Vec::is_empty")]
    pub digest: Vec<u8>,
    /// `ed25519` or `ed25519ph`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub signature_algo: String,
    #[serde(with = "go_bytes", skip_serializing_if = "Vec::is_empty")]
    pub signature: Vec<u8>,
}

/// Payload of [`UpdaterEvent::DownloadProgress`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub written: i64,
    pub total: i64,
    /// Bytes per second since the download began.
    pub rate: f64,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider: String,
}

/// Payload of [`UpdaterEvent::Error`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub stage: Stage,
    pub message: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub provider: String,
}

/// Payload of [`UpdaterEvent::Meta`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub current_version: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub skipped_version: String,
}

/// Platform context for a provider's check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckRequest {
    pub current_version: String,
    pub platform: String,
    pub arch: String,
}

/// Lifecycle events. [`UpdaterEvent::name`] is the event name the UI
/// listens for.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdaterEvent {
    CheckStarted,
    UpdateAvailable(Release),
    NoUpdate,
    DownloadStarted(Release),
    DownloadProgress(Progress),
    /// All bytes are on disk, before verification.
    DownloadComplete(Release),
    Verifying(Release),
    Installing(Release),
    /// Installed and staged; a restart is pending.
    UpdateReady(Release),
    Error(ErrorInfo),
    Meta(Meta),
}

impl UpdaterEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::CheckStarted => "updater:check-started",
            Self::UpdateAvailable(_) => "updater:update-available",
            Self::NoUpdate => "updater:no-update",
            Self::DownloadStarted(_) => "updater:download-started",
            Self::DownloadProgress(_) => "updater:download-progress",
            Self::DownloadComplete(_) => "updater:download-complete",
            Self::Verifying(_) => "updater:verifying",
            Self::Installing(_) => "updater:installing",
            Self::UpdateReady(_) => "updater:update-ready",
            Self::Error(_) => "updater:error",
            Self::Meta(_) => "updater:meta",
        }
    }

    /// The JSON payload (`null` for events without one).
    pub fn payload(&self) -> serde_json::Value {
        let v = match self {
            Self::CheckStarted | Self::NoUpdate => return serde_json::Value::Null,
            Self::UpdateAvailable(r)
            | Self::DownloadStarted(r)
            | Self::DownloadComplete(r)
            | Self::Verifying(r)
            | Self::Installing(r)
            | Self::UpdateReady(r) => serde_json::to_value(r),
            Self::DownloadProgress(p) => serde_json::to_value(p),
            Self::Error(e) => serde_json::to_value(e),
            Self::Meta(m) => serde_json::to_value(m),
        };
        v.unwrap_or_default()
    }
}

/// User actions the UI sends back (`updater:*` user events).
/// They are only honoured while a check-and-install session is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    /// `updater:window:ready`: replay the current state.
    WindowReady,
    /// `updater:user:install`
    Install,
    /// `updater:user:skip`: skip the pending version.
    Skip,
    /// `updater:user:remind`
    Remind,
    /// `updater:user:cancel`
    Cancel,
    /// `updater:user:restart`
    Restart,
}

impl UserAction {
    pub fn from_event_name(name: &str) -> Option<Self> {
        Some(match name {
            "updater:window:ready" => Self::WindowReady,
            "updater:user:install" => Self::Install,
            "updater:user:skip" => Self::Skip,
            "updater:user:remind" => Self::Remind,
            "updater:user:cancel" => Self::Cancel,
            "updater:user:restart" => Self::Restart,
            _ => return None,
        })
    }
}

/// The application side of the updater.
pub trait Host: Send + Sync {
    fn emit(&self, event: UpdaterEvent);
    /// Begin the normal shutdown sequence. Called by restart once the helper
    /// is waiting for this process to exit.
    fn quit(&self);
}

pub type ProviderError = Box<dyn std::error::Error + Send + Sync>;

/// An update source.
pub trait Provider: Send + Sync {
    /// Stable short name for logs and event payloads.
    fn name(&self) -> &str;

    /// The available upgrade for the running platform; `Ok(None)` when the
    /// source confirms the caller is up to date.
    fn check<'a>(
        &'a self,
        req: &'a CheckRequest,
    ) -> BoxFuture<'a, Result<Option<Release>, ProviderError>>;

    /// Streams the artifact bytes to `dst`, reporting `(written, total)`.
    fn download<'a>(
        &'a self,
        rel: &'a Release,
        dst: &'a mut (dyn Write + Send),
        on_progress: &'a mut (dyn FnMut(i64, i64) + Send),
    ) -> BoxFuture<'a, Result<(), ProviderError>>;
}

/// Updater configuration.
pub struct Config {
    /// The running version (release tag form, e.g. `1.2.3`). Required.
    pub current_version: String,
    /// Tried in order: the first release wins; a provider that answers "up
    /// to date" stops the chain (fallback is for "unreachable", not
    /// "disagree"). Required.
    pub providers: Vec<Arc<dyn Provider>>,
    /// The only trust root for signatures. Without it, digest-only releases
    /// still install but signed releases are rejected.
    pub public_key: Vec<u8>,
    /// How long restart waits for the helper to reach helper mode. Zero
    /// means 30 seconds.
    pub helper_ready_timeout: Duration,
    /// Overrides for the check request; empty uses the running OS / arch.
    pub platform: String,
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdaterError {
    #[error("updater: Init already called")]
    AlreadyConfigured,
    #[error("updater: Init has not been called")]
    NotConfigured,
    #[error("updater: no pending release (call Check first)")]
    NoPendingRelease,
    #[error("updater: download already in progress")]
    DownloadInProgress,
    #[error("updater: nothing to restart into (call DownloadAndInstall first)")]
    NotReady,
    /// The helper never acknowledged helper mode; the detail is its report.
    #[error("updater: helper did not signal readiness{}", detail(.0))]
    HelperNotReady(String),
    #[error("{0}")]
    Failed(String),
}

fn detail(d: &str) -> String {
    if d.is_empty() {
        String::new()
    } else {
        format!(": {d}")
    }
}

fn failed(msg: impl std::fmt::Display) -> UpdaterError {
    UpdaterError::Failed(msg.to_string())
}

const HELPER_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// This build's platform and arch as the manifest names them (`darwin`,
/// `amd64`, …).
fn manifest_platform() -> (&'static str, &'static str) {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    };
    (os, arch)
}

#[derive(Default)]
struct Inner {
    cfg: Option<Arc<Config>>,
    state: Option<State>,
    pending: Option<Release>,
    resolved: Option<PathBuf>,
    /// The version `resolved` holds; checks compare against it instead of
    /// the running version so a staged update can be superseded.
    staged_version: String,
    staging_dir: Option<PathBuf>,
    /// Set while [`Updater::restart`] hands the staged path to the helper.
    restarting: bool,
    last_digest: Option<Vec<u8>>,
    skipped: String,
}

type WaitReady =
    Box<dyn Fn(PathBuf, Duration) -> BoxFuture<'static, Result<(), UpdaterError>> + Send + Sync>;

/// Overridable process plumbing for restart (tests substitute benign
/// commands so the test binary is not re-executed into helper mode).
struct RestartHooks {
    self_exe: fn() -> io::Result<PathBuf>,
    command: Box<dyn Fn(&Path) -> std::process::Command + Send + Sync>,
    wait_ready: WaitReady,
}

impl Default for RestartHooks {
    fn default() -> Self {
        Self {
            self_exe: std::env::current_exe,
            command: Box::new(helper::detached_command),
            wait_ready: Box::new(|path, timeout| {
                Box::pin(async move { helper::wait_for_helper_ready(&path, timeout).await })
            }),
        }
    }
}

/// The updater. Construct with [`Updater::new`], configure once with
/// [`Updater::init`], then drive with [`Updater::check_and_install`] and
/// [`Updater::user_action`].
pub struct Updater {
    host: Arc<dyn Host>,
    inner: Mutex<Inner>,
    download_lock: tokio::sync::Mutex<()>,
    /// Whether a check-and-install session (and so its user-action
    /// listeners) is open.
    session: Mutex<bool>,
    hooks: RestartHooks,
    /// The runtime [`Updater::new`] ran on; user actions arrive from the UI
    /// thread and spawn their work here.
    rt: Option<tokio::runtime::Handle>,
}

impl Updater {
    /// Call from within the tokio runtime that should run update work.
    pub fn new(host: Arc<dyn Host>) -> Arc<Self> {
        Arc::new(Self {
            host,
            inner: Mutex::default(),
            download_lock: tokio::sync::Mutex::new(()),
            session: Mutex::new(false),
            hooks: RestartHooks::default(),
            rt: tokio::runtime::Handle::try_current().ok(),
        })
    }

    fn spawn(&self, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        match &self.rt {
            Some(rt) => drop(rt.spawn(fut)),
            None => drop(tokio::spawn(fut)),
        }
    }

    /// Configures the updater once.
    pub fn init(&self, mut cfg: Config) -> Result<(), UpdaterError> {
        let mut inner = self.inner.lock();
        if inner.cfg.is_some() {
            return Err(UpdaterError::AlreadyConfigured);
        }
        if cfg.current_version.is_empty() {
            return Err(failed("updater: Config.CurrentVersion is required"));
        }
        if cfg.providers.is_empty() {
            return Err(failed(
                "updater: Config.Providers must contain at least one Provider",
            ));
        }
        let (os, arch) = manifest_platform();
        if cfg.platform.is_empty() {
            cfg.platform = os.to_string();
        }
        if cfg.arch.is_empty() {
            cfg.arch = arch.to_string();
        }
        inner.cfg = Some(Arc::new(cfg));
        inner.state = Some(State::Idle);
        Ok(())
    }

    pub fn state(&self) -> State {
        self.inner.lock().state.unwrap_or(State::Unconfigured)
    }

    /// The configured running version, or empty before init.
    pub fn current_version(&self) -> String {
        let inner = self.inner.lock();
        inner
            .cfg
            .as_ref()
            .map(|c| c.current_version.clone())
            .unwrap_or_default()
    }

    /// The staged artifact (after a successful install), if any.
    pub fn downloaded_path(&self) -> Option<PathBuf> {
        self.inner.lock().resolved.clone()
    }

    /// Later checks treat `version` as "no update".
    pub fn skip_version(&self, version: &str) {
        self.inner.lock().skipped = version.to_string();
    }

    pub fn skipped_version(&self) -> String {
        self.inner.lock().skipped.clone()
    }

    fn cfg(&self) -> Result<Arc<Config>, UpdaterError> {
        self.inner
            .lock()
            .cfg
            .clone()
            .ok_or(UpdaterError::NotConfigured)
    }

    fn transition(&self, s: State) {
        self.inner.lock().state = Some(s);
    }

    fn emit(&self, ev: UpdaterEvent) {
        self.host.emit(ev);
    }

    fn should_skip(&self, version: &str) -> bool {
        let inner = self.inner.lock();
        !version.is_empty() && !inner.skipped.is_empty() && inner.skipped == version
    }

    /// Walks the provider chain. `Ok(Some)` is a newer release, `Ok(None)`
    /// means up to date, `Err` means every provider failed.
    pub async fn check(&self) -> Result<Option<Release>, UpdaterError> {
        let cfg = self.cfg()?;
        self.transition(State::Checking);
        self.emit(UpdaterEvent::CheckStarted);
        let staged = self.inner.lock().staged_version.clone();
        let req = CheckRequest {
            current_version: if staged.is_empty() {
                cfg.current_version.clone()
            } else {
                staged
            },
            platform: cfg.platform.clone(),
            arch: cfg.arch.clone(),
        };
        let mut failures = Vec::new();
        for p in &cfg.providers {
            let mut rel = match p.check(&req).await {
                Ok(Some(rel)) => rel,
                Ok(None) => return Ok(self.up_to_date()),
                Err(e) => {
                    failures.push(format!("{}: {e}", p.name()));
                    continue;
                }
            };
            if self.should_skip(&rel.version) {
                return Ok(self.up_to_date());
            }
            rel.provider = p.name().to_string();
            {
                let mut inner = self.inner.lock();
                inner.pending = Some(rel.clone());
                inner.state = Some(State::Available);
            }
            self.emit(UpdaterEvent::UpdateAvailable(rel.clone()));
            return Ok(Some(rel));
        }
        let msg = if failures.is_empty() {
            "all providers failed".to_string()
        } else {
            format!("updater: all providers failed: {}", failures.join("; "))
        };
        self.transition(State::Error);
        self.emit(UpdaterEvent::Error(ErrorInfo {
            stage: Stage::Check,
            message: msg.clone(),
            provider: String::new(),
        }));
        Err(failed(msg))
    }

    fn up_to_date(&self) -> Option<Release> {
        {
            let mut inner = self.inner.lock();
            inner.state = Some(if inner.resolved.is_some() {
                State::Ready
            } else {
                State::UpToDate
            });
        }
        self.emit(UpdaterEvent::NoUpdate);
        None
    }

    fn fail_stage(&self, stage: Stage, provider: &str, err: UpdaterError) -> UpdaterError {
        self.transition(State::Error);
        self.emit(UpdaterEvent::Error(ErrorInfo {
            stage,
            message: err.to_string(),
            provider: provider.to_string(),
        }));
        err
    }

    /// Downloads, verifies and stages the pending release.
    pub async fn download_and_install(&self) -> Result<(), UpdaterError> {
        let Ok(_guard) = self.download_lock.try_lock() else {
            return Err(UpdaterError::DownloadInProgress);
        };
        let cfg = self.cfg()?;
        let pending = self
            .inner
            .lock()
            .pending
            .clone()
            .ok_or(UpdaterError::NoPendingRelease)?;
        let provider = cfg
            .providers
            .iter()
            .find(|p| p.name() == pending.provider)
            .cloned()
            .ok_or_else(|| {
                failed(format!(
                    "updater: provider {:?} is no longer registered",
                    pending.provider
                ))
            })?;
        let name = provider.name().to_string();

        let (tmp_path, tmp_dir) = match self.download(provider.as_ref(), &pending).await {
            Ok(v) => v,
            Err(e) => return Err(self.fail_stage(Stage::Download, &name, e)),
        };
        let cleanup = || {
            let _ = std::fs::remove_dir_all(&tmp_dir);
        };

        self.transition(State::Verifying);
        self.emit(UpdaterEvent::Verifying(pending.clone()));
        if let Err(e) = self.verify(&pending, &cfg) {
            cleanup();
            return Err(self.fail_stage(Stage::Verify, &name, e));
        }

        self.transition(State::Installing);
        self.emit(UpdaterEvent::Installing(pending.clone()));
        let final_path = match finalise_download(&tmp_path, &pending.artifact.filename) {
            Ok(p) => p,
            Err(e) => {
                cleanup();
                return Err(self.fail_stage(Stage::Install, &name, e));
            }
        };
        // macOS ships the .app inside a .zip; unpack so the helper swaps a
        // directory for a directory.
        let final_path = match extract::maybe_extract_into(&final_path) {
            Ok((p, _)) => p,
            Err(e) => {
                cleanup();
                return Err(self.fail_stage(Stage::Install, &name, e));
            }
        };

        // Replace any update a previous install staged without a restart.
        let old = {
            let mut inner = self.inner.lock();
            inner.resolved = Some(final_path);
            inner.staged_version = pending.version.clone();
            inner.state = Some(State::Ready);
            inner
                .staging_dir
                .replace(tmp_dir)
                .filter(|_| !inner.restarting)
        };
        // simplify: a restart that fails after this leaves the old dir in the temp dir.
        if let Some(old) = old {
            let _ = std::fs::remove_dir_all(old);
        }
        self.emit(UpdaterEvent::UpdateReady(pending));
        Ok(())
    }

    /// Streams the artifact into a fresh `recur-update-*` temp dir, hashing
    /// as the bytes pass. On error everything it created is removed.
    async fn download(
        &self,
        p: &dyn Provider,
        rel: &Release,
    ) -> Result<(PathBuf, PathBuf), UpdaterError> {
        self.transition(State::Downloading);
        self.emit(UpdaterEvent::DownloadStarted(rel.clone()));

        let dir = extract::mkdir_temp(&std::env::temp_dir(), "recur-update-")
            .map_err(|e| failed(format!("updater: temp dir: {e}")))?;
        match self.download_into(p, rel, &dir).await {
            Ok(path) => Ok((path, dir)),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    async fn download_into(
        &self,
        p: &dyn Provider,
        rel: &Release,
        dir: &Path,
    ) -> Result<PathBuf, UpdaterError> {
        let tmp_path = dir.join(".artifact");
        let file = std::fs::File::create(&tmp_path)
            .map_err(|e| failed(format!("updater: create temp: {e}")))?;
        let hasher = match &rel.verification {
            // Ed25519ph requires SHA-512 whatever the digest algo says.
            Some(v) if v.signature_algo == "ed25519ph" => Some(Hasher::for_algo("sha512")?),
            Some(v) => Some(Hasher::for_algo(&v.digest_algo)?),
            None => None,
        };
        let tracker = Arc::new(Mutex::new(ProgressTracker::new(
            Arc::clone(&self.host),
            p.name().to_string(),
        )));
        let mut sink = HashingSink {
            file,
            hasher,
            tracker: Arc::clone(&tracker),
        };
        let mut on_progress = {
            let tracker = Arc::clone(&tracker);
            move |written: i64, total: i64| tracker.lock().tick(written, total)
        };
        p.download(rel, &mut sink, &mut on_progress)
            .await
            .map_err(|e| failed(e.to_string()))?;
        sink.file
            .sync_all()
            .map_err(|e| failed(format!("updater: fsync: {e}")))?;
        let HashingSink { file, hasher, .. } = sink;
        drop(file);
        // Final tick so subscribers see written == total.
        tracker.lock().emit_now();
        self.emit(UpdaterEvent::DownloadComplete(rel.clone()));
        if let Some(h) = hasher {
            self.inner.lock().last_digest = Some(h.finish());
        }
        Ok(tmp_path)
    }

    /// Checks the digest computed while streaming; the file is not re-read.
    fn verify(&self, rel: &Release, cfg: &Config) -> Result<(), UpdaterError> {
        let Some(v) = &rel.verification else {
            return Ok(());
        };
        let digest = self
            .inner
            .lock()
            .last_digest
            .clone()
            .ok_or_else(|| failed("updater: no digest available; download path bug"))?;
        verify::run_verification(&digest, v, &cfg.public_key)
    }

    /// Opens a session (user actions are honoured from now on) and runs
    /// check, then download-and-install when something newer exists.
    pub async fn check_and_install(&self) -> Result<(), UpdaterError> {
        *self.session.lock() = true;
        if self.check().await?.is_none() {
            return Ok(());
        }
        self.download_and_install().await
    }

    /// Handles a user action from the UI. Ignored without an open session.
    pub fn user_action(self: &Arc<Self>, action: UserAction) {
        if !*self.session.lock() {
            return;
        }
        match action {
            UserAction::Install => {
                let this = Arc::clone(self);
                self.spawn(async move {
                    let _ = this.download_and_install().await;
                });
            }
            UserAction::Cancel | UserAction::Remind => *self.session.lock() = false,
            UserAction::Skip => {
                {
                    let mut inner = self.inner.lock();
                    if inner.cfg.is_some()
                        && let Some(p) = &inner.pending
                    {
                        inner.skipped = p.version.clone();
                    }
                }
                *self.session.lock() = false;
            }
            UserAction::Restart => {
                let this = Arc::clone(self);
                self.spawn(async move {
                    if let Err(e) = this.restart().await {
                        tracing::warn!("updater restart: {e}");
                    }
                });
            }
            UserAction::WindowReady => self.replay_state_snapshot(),
        }
    }

    /// Re-emits the event for the current state, after [`UpdaterEvent::Meta`].
    fn replay_state_snapshot(&self) {
        let (state, pending, current, skipped) = {
            let inner = self.inner.lock();
            (
                inner.state.unwrap_or(State::Unconfigured),
                inner.pending.clone().unwrap_or_default(),
                inner
                    .cfg
                    .as_ref()
                    .map(|c| c.current_version.clone())
                    .unwrap_or_default(),
                inner.skipped.clone(),
            )
        };
        self.emit(UpdaterEvent::Meta(Meta {
            current_version: current,
            skipped_version: skipped,
        }));
        let ev = match state {
            State::Checking => UpdaterEvent::CheckStarted,
            State::Available => UpdaterEvent::UpdateAvailable(pending),
            State::Downloading => UpdaterEvent::DownloadStarted(pending),
            State::Verifying => UpdaterEvent::Verifying(pending),
            State::Installing => UpdaterEvent::Installing(pending),
            State::Ready => UpdaterEvent::UpdateReady(pending),
            State::UpToDate => UpdaterEvent::NoUpdate,
            State::Idle | State::Unconfigured | State::Error => return,
        };
        self.emit(ev);
    }

    /// Spawns this binary in helper mode, waits until it is ready, then asks
    /// the host to quit. The helper swaps the app once this process exits
    /// and relaunches it. If the helper never gets ready it is killed and
    /// this process keeps running the old version.
    pub async fn restart(&self) -> Result<(), UpdaterError> {
        self.inner.lock().restarting = true;
        let res = self.spawn_helper_and_quit().await;
        if res.is_err() {
            self.inner.lock().restarting = false;
        }
        res
    }

    async fn spawn_helper_and_quit(&self) -> Result<(), UpdaterError> {
        let (staged, timeout) = {
            let inner = self.inner.lock();
            let timeout = inner
                .cfg
                .as_ref()
                .map(|c| c.helper_ready_timeout)
                .filter(|t| !t.is_zero())
                .unwrap_or(HELPER_READY_TIMEOUT);
            (inner.resolved.clone(), timeout)
        };
        let staged = staged.ok_or(UpdaterError::NotReady)?;
        let self_exe =
            (self.hooks.self_exe)().map_err(|e| failed(format!("updater: resolve self: {e}")))?;
        let target = bundle_target(&self_exe);
        let pid = std::process::id();
        let tmp = std::env::temp_dir();
        let log_path = tmp.join(format!("recur-update-{pid}.log"));
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let ready_path = tmp.join(format!("recur-update-{pid}-{nanos}.ready"));
        let _ = std::fs::remove_file(&ready_path);
        let _ready_cleanup = RemoveOnDrop(ready_path.clone());

        let mut cmd = (self.hooks.command)(&self_exe);
        cmd.env(helper::ENV_MODE, "1")
            .env(helper::ENV_TARGET, &target)
            .env(helper::ENV_NEW, &staged)
            .env(helper::ENV_PID, pid.to_string())
            .env(helper::ENV_LOG, &log_path)
            .env(helper::ENV_READY, &ready_path);
        let mut child = cmd
            .spawn()
            .map_err(|e| failed(format!("updater: spawn helper: {e}")))?;
        if let Err(e) = (self.hooks.wait_ready)(ready_path, timeout).await {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        // The helper now waits for this pid to exit.
        drop(child);
        self.host.quit();
        Ok(())
    }
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The `.app` bundle containing `exe`, or `exe` itself outside a bundle (and
/// on Linux, where the binary is the swap target).
fn bundle_target(exe: &Path) -> PathBuf {
    if !cfg!(target_os = "macos") {
        return exe.to_path_buf();
    }
    let clean = crate::pathutil::clean(&exe.to_string_lossy());
    let parts: Vec<&str> = clean.split('/').collect();
    match parts.iter().position(|p| p.ends_with(".app")) {
        Some(i) => PathBuf::from(format!("/{}", crate::pathutil::join(&parts[1..=i]))),
        None => exe.to_path_buf(),
    }
}

/// Renames the downloaded temp file to the artifact's base name inside the
/// staging dir, neutralising names that would resolve outside it.
fn finalise_download(tmp_path: &Path, filename: &str) -> Result<PathBuf, UpdaterError> {
    let base = filename.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
    let base = if base.is_empty() || base == "." || base == ".." || base.contains('\\') {
        "recur-update.bin"
    } else {
        base
    };
    let dest = tmp_path.parent().unwrap_or(Path::new(".")).join(base);
    std::fs::rename(tmp_path, &dest).map_err(|e| failed(format!("updater: finalise: {e}")))?;
    Ok(dest)
}

/// The download destination: file, streaming hash and byte counter.
struct HashingSink {
    file: std::fs::File,
    hasher: Option<Hasher>,
    tracker: Arc<Mutex<ProgressTracker>>,
}

impl Write for HashingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        if let Some(h) = &mut self.hasher {
            h.update(&buf[..n]);
        }
        if n > 0 {
            self.tracker.lock().add(n);
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Throttles provider progress to ~10 events a second and computes the rate.
struct ProgressTracker {
    host: Arc<dyn Host>,
    provider: String,
    written: i64,
    total: i64,
    start: Instant,
    last_emit: Instant,
}

impl ProgressTracker {
    fn new(host: Arc<dyn Host>, provider: String) -> Self {
        let now = Instant::now();
        Self {
            host,
            provider,
            written: 0,
            total: 0,
            start: now,
            last_emit: now,
        }
    }

    fn add(&mut self, n: usize) {
        self.written += n as i64;
    }

    fn tick(&mut self, written: i64, total: i64) {
        self.written = self.written.max(written);
        if total > 0 {
            self.total = total;
        }
        if self.last_emit.elapsed() < Duration::from_millis(100) {
            return;
        }
        self.emit_now();
    }

    fn emit_now(&mut self) {
        self.last_emit = Instant::now();
        let elapsed = (self.last_emit - self.start).as_secs_f64();
        let rate = if elapsed > 0.0 {
            self.written as f64 / elapsed
        } else {
            0.0
        };
        self.host.emit(UpdaterEvent::DownloadProgress(Progress {
            written: self.written,
            total: self.total,
            rate,
            provider: self.provider.clone(),
        }));
    }
}

/// `[]byte` JSON: standard base64.
mod go_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(b))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = Option::<String>::deserialize(d)?.unwrap_or_default();
        base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(serde::de::Error::custom)
    }
}

/// `time.Time` JSON: never omitted; the zero time stands for "unset".
mod go_time {
    use chrono::{DateTime, FixedOffset};
    use serde::{Deserialize, Deserializer, Serializer};

    const ZERO: &str = "0001-01-01T00:00:00Z";

    pub(super) fn serialize<S: Serializer>(
        t: &Option<DateTime<FixedOffset>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
            None => s.serialize_str(ZERO),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<DateTime<FixedOffset>>, D::Error> {
        let s = Option::<String>::deserialize(d)?.unwrap_or_default();
        if s.is_empty() || s == ZERO {
            return Ok(None);
        }
        DateTime::parse_from_rfc3339(&s)
            .map(Some)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
