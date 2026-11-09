//! OpenCode CLI harness: Recur's model sources (API-key endpoints) run
//! inside OpenCode's own agent loop.
//!
//! Recur passes a per-turn config through `OPENCODE_CONFIG_CONTENT`: the
//! selected compatible endpoint becomes a custom OpenAI-compatible provider,
//! Recur's MCP host tools become a remote MCP server, and the turn's prompt
//! becomes a primary agent. See `docs/opencode-provider.md`.

mod config;
mod stream;

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use recur_agent::turn::{TurnContext, TurnRequest};
use recur_agent::{
    AgentEvent, Availability, CredentialDraft, ErrorKind, Flags, HistoryMode, McpTransports,
    ModelChoice, PermissionRoute, Provider, ProviderCaps, ProviderError, ProviderStatus,
    SteerSupport, ToolDisplay, ToolTransport, TurnResult,
};
use recur_domain::{PROVIDER_OPENCODE, Settings, base_model_ref, provider_label};
use stream::TurnState;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

pub use config::{chat_base_url, provider_id, split_model, wire_model};

/// Overrides binary resolution (development and tests).
pub const ENV_BIN_PATH: &str = "RECUR_OPENCODE";

/// What an external CLI harness offers: native history and tools over MCP.
/// OpenCode owns approval prompts; Recur translates its allowlist into
/// OpenCode permissions and never bridges prompts back.
pub const CAPS: ProviderCaps = ProviderCaps {
    permission: PermissionRoute::None,
    steer: SteerSupport::None,
    history: HistoryMode::Native,
    tool_transport: ToolTransport::Mcp,
    native_fork: true,
    images: false,
    can_disable_tools: true,
    max_turns: false,
    partials: false,
    user_mcp: McpTransports {
        http: false,
        stdio: false,
    },
};

const STDERR_TAIL: usize = 8192;

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn look_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| is_executable_file(p))
}

fn home_candidates() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut out = Vec::new();
    if let Some(home) = home {
        out.push(home.join(".opencode/bin/opencode"));
        out.push(home.join(".local/bin/opencode"));
        out.push(home.join(".local/share/mise/installs/opencode/latest/opencode"));
    }
    out.push(PathBuf::from("/usr/local/bin/opencode"));
    out.push(PathBuf::from("/opt/homebrew/bin/opencode"));
    out
}

/// Resolves the `opencode` binary: `$RECUR_OPENCODE`, the Settings path,
/// `opencode` on `PATH`, then common install locations.
pub fn locate(settings: &Settings) -> Result<String, String> {
    let mut tried = Vec::new();
    let from_env = std::env::var(ENV_BIN_PATH).ok();
    for candidate in [
        from_env.as_deref().unwrap_or_default(),
        settings.opencode_bin_path.as_str(),
    ] {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        let path = PathBuf::from(candidate);
        if is_executable_file(&path) {
            return Ok(candidate.to_owned());
        }
        tried.push(candidate.to_owned());
    }
    if let Some(path) = look_path("opencode") {
        return Ok(path.to_string_lossy().into_owned());
    }
    for path in home_candidates() {
        if is_executable_file(&path) {
            return Ok(path.to_string_lossy().into_owned());
        }
    }
    if tried.is_empty() {
        tried.push("PATH".into());
    }
    Err(format!(
        "opencode binary not found (looked in: {}): install OpenCode and put `opencode` on PATH",
        tried.join(", ")
    ))
}

/// The first tool name mapped into Recur's cross-provider vocabulary.
fn canonical_tool(name: &str) -> &str {
    match name {
        "bash" => "Bash",
        "read" => "Read",
        "edit" | "write" | "patch" => "Edit",
        "glob" => "Glob",
        "grep" => "Grep",
        "todowrite" => "TodoWrite",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        "task" => "Task",
        "list" => "LS",
        other => other,
    }
}

pub struct OpenCodeProvider {
    settings: RwLock<Settings>,
}

impl OpenCodeProvider {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings: RwLock::new(settings),
        }
    }

    fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    fn source_model(&self, model: &str) -> Option<(String, String)> {
        split_model(model).ok()
    }

    async fn version(path: &Path) -> String {
        let out = tokio::time::timeout(
            Duration::from_secs(4),
            Command::new(path).arg("--version").output(),
        )
        .await;
        let Ok(Ok(out)) = out else {
            return String::new();
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    async fn status_for(&self, settings: &Settings) -> ProviderStatus {
        match locate(settings) {
            Ok(path) => {
                let version = Self::version(Path::new(&path)).await;
                ProviderStatus {
                    installed: true,
                    path,
                    version,
                    authenticated: true,
                    provider: "OpenCode".into(),
                    ..Default::default()
                }
            }
            Err(error) => ProviderStatus {
                error,
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl Provider for OpenCodeProvider {
    fn id(&self) -> &str {
        PROVIDER_OPENCODE
    }

    fn label(&self) -> &str {
        provider_label(PROVIDER_OPENCODE)
    }

    fn caps(&self) -> &ProviderCaps {
        &CAPS
    }

    fn availability(&self, settings: &Settings) -> Availability {
        if !settings.provider_enabled(PROVIDER_OPENCODE) {
            return Availability::Unavailable(
                "OpenCode is turned off in Settings → Providers".into(),
            );
        }
        match locate(settings) {
            Ok(_) => Availability::Ready,
            Err(error) => Availability::Unavailable(error),
        }
    }

    async fn status(&self, settings: &Settings) -> ProviderStatus {
        self.status_for(settings).await
    }

    async fn test_credentials(
        &self,
        settings: &Settings,
        _draft: &CredentialDraft,
    ) -> ProviderStatus {
        self.status_for(settings).await
    }

    async fn models(&self, _settings: &Settings) -> Result<Vec<ModelChoice>, ProviderError> {
        Ok(Vec::new())
    }

    fn cached_models(&self, _settings: &Settings) -> Vec<ModelChoice> {
        Vec::new()
    }

    fn resolve_model(&self, model: &str, _flags: &Flags) -> String {
        model.to_owned()
    }

    fn context_window(&self, model: &str, _flags: &Flags) -> i64 {
        let settings = self.settings();
        let Some((source, id)) = self.source_model(model) else {
            return 0;
        };
        settings
            .compatible_by_id(&source)
            .and_then(|p| p.models.iter().find(|m| m.id == id))
            .map_or(0, |m| m.context_window)
    }

    fn model_label(&self, model: &str) -> String {
        let settings = self.settings();
        let Some((source, id)) = self.source_model(model) else {
            return base_model_ref(model).to_owned();
        };
        let Some(provider) = settings.compatible_by_id(&source) else {
            return base_model_ref(model).to_owned();
        };
        provider
            .models
            .iter()
            .find(|m| m.id == id)
            .map(|m| {
                if m.label.is_empty() {
                    m.id.clone()
                } else {
                    m.label.clone()
                }
            })
            .unwrap_or(id)
    }

    fn oneshot_flags(&self, _model: &str) -> Flags {
        Flags::new()
    }

    async fn run_turn(
        &self,
        req: TurnRequest,
        cx: TurnContext,
    ) -> Result<TurnResult, ProviderError> {
        let settings = self.settings();
        let (source, model_id) = config::split_model(&req.model)
            .map_err(|e| ProviderError::new(ErrorKind::Failed, e))?;
        let Some(provider) = settings.compatible_by_id(&source).cloned() else {
            return Err(ProviderError::new(
                ErrorKind::Unavailable,
                format!(
                    "OpenCode cannot run {source}/…: pick a model from a configured endpoint in Settings → Providers"
                ),
            ));
        };
        let bin = locate(&settings).map_err(|e| ProviderError::new(ErrorKind::Unavailable, e))?;
        let resumed = !req.resume_session.trim().is_empty();
        let cfg = config::opencode_config(&req, &provider, &model_id, &req.system_prompt, resumed);
        let prompt = config::user_prompt(&req, resumed);

        let mut cmd = Command::new(&bin);
        cmd.arg("run")
            .arg("--format")
            .arg("json")
            .arg("--model")
            .arg(config::wire_model(&source, &model_id))
            .arg("--agent")
            .arg("recur")
            .arg("--auto")
            // OpenCode resolves its project from `--dir`, else `$PWD`, not the
            // process cwd: without both, tools ran in the directory Recur was
            // launched from instead of the worktree.
            .arg("--dir")
            .arg(&req.worktree)
            .current_dir(&req.worktree)
            .env("PWD", &req.worktree)
            .env("OPENCODE_CONFIG_CONTENT", cfg.to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        if resumed {
            cmd.arg("--session").arg(req.resume_session.trim());
            if req.fork_session {
                cmd.arg("--fork");
            }
        }
        for key in &req.env_scrub {
            cmd.env_remove(key);
        }

        let mut child = cmd.spawn().map_err(|e| {
            ProviderError::new(
                ErrorKind::Unavailable,
                format!("could not start opencode: {e}"),
            )
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            tokio::spawn(async move {
                let _ = stdin.write_all(prompt.as_bytes()).await;
                let _ = stdin.shutdown().await;
            });
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::failed("opencode produced no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProviderError::failed("opencode produced no stderr"))?;
        let stderr_task = tokio::spawn(read_tail(stderr));

        let mut state = TurnState::default();
        let mut lines = BufReader::new(stdout).lines();
        let canceled = loop {
            tokio::select! {
                _ = cx.cancel.cancelled() => break true,
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        for ev in state.ingest(&line) {
                            cx.sink.emit(ev);
                        }
                    }
                    Ok(None) => break false,
                    Err(e) => {
                        state.error = e.to_string();
                        break false;
                    }
                },
            }
        };
        if canceled {
            let _ = child.start_kill();
            let _ = child.wait().await;
            let stderr = stderr_task.await.unwrap_or_default();
            return Err(ProviderError::canceled().with_partial(state.partial(&stderr)));
        }
        let status = child.wait().await;
        let stderr = stderr_task.await.unwrap_or_default();
        let ok = status.map(|s| s.success()).unwrap_or(false);
        let result = state.finish(ok, &stderr);
        if result.session_id.is_empty() && !ok {
            return Err(ProviderError::failed(
                result
                    .stderr
                    .clone()
                    .if_empty("opencode exited without a result"),
            )
            .with_partial(result));
        }
        cx.sink.emit(AgentEvent::TurnEnd(result.clone()));
        Ok(result)
    }

    fn resume_handle_fits(&self, handle: &str) -> bool {
        let handle = handle.trim();
        handle.is_empty() || handle.starts_with("ses_")
    }

    fn display_tool(&self, name: &str, input: &str) -> ToolDisplay {
        ToolDisplay {
            name: canonical_tool(name).to_owned(),
            input: input.to_owned(),
        }
    }

    async fn settings_changed(&self, _old: &Settings, new: &Settings) {
        *self.settings.write() = new.clone();
    }
}

trait IfEmpty {
    fn if_empty(self, fallback: &str) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
}

async fn read_tail(mut stderr: tokio::process::ChildStderr) -> String {
    let mut buf = Vec::new();
    let _ = stderr.read_to_end(&mut buf).await;
    let start = buf.len().saturating_sub(STDERR_TAIL);
    String::from_utf8_lossy(&buf[start..]).into_owned()
}

#[cfg(test)]
mod tests;
