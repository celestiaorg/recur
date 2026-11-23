//! Agent Client Protocol provider: one generic client for every ACP stdio
//! agent in [`recur_domain::ACP_AGENTS`] (Antigravity `agy acp`, `opencode
//! acp`). The agent is spawned per turn in the worktree (the Recur MCP URL is
//! per agent), speaks ACP v1 JSON-RPC on stdio, and keeps the conversation
//! (`session/resume` or `session/load`).

pub mod agent;
pub mod launch;
pub mod rpc;
pub mod turn;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use recur_agent::{
    Availability, ErrorKind, Flags, HistoryMode, McpTransports, ModelChoice, PermissionRoute,
    Provider, ProviderCaps, ProviderError, ProviderStatus, SteerSupport, ToolDisplay,
    ToolTransport, TurnContext, TurnRequest, TurnResult,
};
use recur_domain::{AcpAgent, PERMISSION_MODE_ASK, Settings};

/// How long `models()` reuses a probe, so picker refreshes do not spawn the
/// agent (and create a session) every time.
const MODELS_TTL: Duration = Duration::from_secs(5 * 60);

/// What every ACP provider shares.
#[derive(Debug, Clone, Default)]
pub struct AcpConfig {
    /// Recur's data dir; probes run in `<data_dir>/acp-probe/<id>`.
    pub data_dir: PathBuf,
    /// Environment variables spawned agents must not inherit (tracker
    /// secrets); turns add `TurnRequest::env_scrub`.
    pub env_scrub: Vec<String>,
}

/// One ACP agent as a Recur provider.
pub struct AcpProvider {
    agent: &'static AcpAgent,
    cfg: AcpConfig,
    /// The last good model probe and when it ran (offline fallback for slot
    /// remaps).
    last_models: Mutex<Option<(Instant, Vec<ModelChoice>)>>,
}

const CAPS: ProviderCaps = ProviderCaps {
    permission: PermissionRoute::Callback,
    steer: SteerSupport::None,
    history: HistoryMode::Native,
    tool_transport: ToolTransport::Mcp,
    native_fork: false,
    images: true,
    can_disable_tools: false,
    max_turns: false,
    partials: true,
    user_mcp: McpTransports {
        http: true,
        stdio: true,
    },
};

impl AcpProvider {
    pub fn new(agent: &'static AcpAgent, cfg: AcpConfig) -> Self {
        Self {
            agent,
            cfg,
            last_models: Mutex::default(),
        }
    }

    pub fn agent(&self) -> &'static AcpAgent {
        self.agent
    }

    /// The binary: `$<env_bin>`, then `<bin>` on `PATH`.
    pub fn locate(&self) -> Result<PathBuf, String> {
        if let Ok(p) = std::env::var(self.agent.env_bin)
            && !p.trim().is_empty()
        {
            let p = PathBuf::from(p.trim());
            return if is_executable(&p) {
                Ok(p)
            } else {
                Err(format!(
                    "{}={} is not an executable",
                    self.agent.env_bin,
                    p.display()
                ))
            };
        }
        std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(self.agent.bin))
                    .find(|p| is_executable(p))
            })
            .ok_or_else(|| format!("{} not found: {}", self.agent.bin, self.agent.install_hint))
    }

    fn scrub(&self, extra: &[String]) -> Vec<String> {
        let mut v = self.cfg.env_scrub.clone();
        v.extend(extra.iter().cloned());
        v
    }

    fn data_dir(&self) -> PathBuf {
        if self.cfg.data_dir.as_os_str().is_empty() {
            std::env::temp_dir()
        } else {
            self.cfg.data_dir.clone()
        }
    }

    async fn probe(&self, bin: &Path) -> Result<agent::Probe, String> {
        let data_dir = self.data_dir();
        let launch = launch::Launch::new(
            self.agent,
            bin,
            &data_dir,
            self.scrub(&[]),
            PERMISSION_MODE_ASK,
        )?;
        let p = agent::probe(self.agent, &launch, &data_dir).await?;
        if let Ok(info) = &p.session {
            *self.last_models.lock() = Some((Instant::now(), info.choices(self.agent)));
        }
        Ok(p)
    }
}

fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[async_trait::async_trait]
impl Provider for AcpProvider {
    fn id(&self) -> &str {
        self.agent.id
    }

    fn label(&self) -> &str {
        self.agent.label
    }

    fn caps(&self) -> &ProviderCaps {
        &CAPS
    }

    fn availability(&self, settings: &Settings) -> Availability {
        if !settings.provider_enabled(self.agent.id) {
            return Availability::Unavailable(format!(
                "{} is turned off in Settings → Providers",
                self.agent.label
            ));
        }
        Availability::Ready
    }

    /// `installed`/`path` from [`AcpProvider::locate`], `version` from
    /// `agentInfo`, `authenticated` when a probe session opens, `model_count`
    /// from its models, `error` the install or login hint.
    async fn status(&self, _settings: &Settings) -> ProviderStatus {
        let bin = match self.locate() {
            Ok(b) => b,
            Err(error) => {
                return ProviderStatus {
                    error,
                    ..Default::default()
                };
            }
        };
        let mut st = ProviderStatus {
            installed: true,
            path: bin.to_string_lossy().into_owned(),
            ..Default::default()
        };
        match self.probe(&bin).await {
            Err(e) => st.error = e,
            Ok(p) => {
                st.version = p.version;
                match p.session {
                    Ok(info) => {
                        st.authenticated = true;
                        st.model_count = i64::try_from(info.model_options().len()).unwrap_or(0);
                    }
                    Err(e) => st.error = e,
                }
            }
        }
        st
    }

    /// The probed list, reused for [`MODELS_TTL`]. Turned off lists nothing.
    async fn models(&self, settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        if !settings.provider_enabled(self.agent.id) {
            return Ok(Vec::new());
        }
        if let Some((at, m)) = self.last_models.lock().as_ref()
            && at.elapsed() < MODELS_TTL
        {
            return Ok(m.clone());
        }
        let unavailable = |e: String| ProviderError::new(ErrorKind::Unavailable, e);
        let bin = self.locate().map_err(unavailable)?;
        let p = self.probe(&bin).await.map_err(unavailable)?;
        p.session
            .map(|info| info.choices(self.agent))
            .map_err(unavailable)
    }

    fn cached_models(&self, _settings: &Settings) -> Vec<ModelChoice> {
        self.last_models
            .lock()
            .as_ref()
            .map(|(_, m)| m.clone())
            .unwrap_or_default()
    }

    fn resolve_model(&self, model: &str, _flags: &Flags) -> String {
        model.to_owned()
    }

    fn context_window(&self, _model: &str, _flags: &Flags) -> i64 {
        0
    }

    fn model_label(&self, model: &str) -> String {
        let want = format!("{}/{model}", self.agent.id);
        self.last_models
            .lock()
            .as_ref()
            .and_then(|(_, m)| m.iter().find(|c| c.model_ref == want))
            .map_or_else(|| model.to_owned(), |c| c.label.clone())
    }

    fn oneshot_flags(&self, _model: &str) -> Flags {
        Flags::new()
    }

    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        let bin = self
            .locate()
            .map_err(|e| ProviderError::new(ErrorKind::Unavailable, e))?;
        let launch = launch::Launch::new(
            self.agent,
            &bin,
            &self.data_dir(),
            self.scrub(&req.env_scrub),
            &req.permission_mode,
        )
        .map_err(|e| ProviderError::new(ErrorKind::Unavailable, e))?;
        turn::run(self.agent, &launch, req, cx).await
    }

    /// Handles are stored as `<agent id>:<ACP session id>`.
    fn resume_handle_fits(&self, handle: &str) -> bool {
        let h = handle.trim();
        h.is_empty()
            || h.strip_prefix(self.agent.id)
                .is_some_and(|rest| rest.starts_with(':'))
    }

    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        turn::display(name, input)
    }
}
