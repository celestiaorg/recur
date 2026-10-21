//! The cursor-sdk-bridge process (Go `cursor/bridge.go`): the local
//! Connect/JSON server Cursor ships inside its Python SDK wheel, a bundled
//! Node runtime running the TypeScript SDK.
//!
//! Why the bridge and not `cursor-agent`: MCP servers go inline per agent (no
//! `.cursor/mcp.json` in the worktree), `settingSources` can narrow ambient
//! config, and run events are typed. What it cannot do (backend facts): no
//! custom system prompt (role prompts are prepended to the user message), no
//! dollar cost for local runs, one run per agent at a time.

mod log;
mod reap;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rustix::process::{Pid, Signal, kill_process_group};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::connect::{Client, Error};
use crate::gojson::{go_int, null_default, trim};
use crate::projectdirs::ensure_cursor_project_dirs_in;

#[cfg(test)]
pub(crate) use log::redact_bridge_line;


/// What the bridge prints on STDERR once listening; the rest of the line is
/// the discovery JSON.
pub(crate) const READY_PREFIX: &str = "cursor-sdk-bridge ready ";

/// Bounds the wait for the ready line. The bundled Node runtime cold-starts
/// slowly on first run.
const BRIDGE_START_TIMEOUT: Duration = Duration::from_secs(60);

/// SIGTERM, wait, then SIGKILL the whole process group.
pub(crate) const BRIDGE_KILL_GRACE: Duration = Duration::from_secs(2);

/// Exported so the install-time SDK patch can read attribution toggles.
pub const ENV_ATTRIBUTION_PATH: &str = "RECUR_CURSOR_ATTRIBUTION_PATH";

/// The ready-line payload. Recur asserts schema, transport and protocol so a
/// bridge that changes them fails loudly at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Discovery {
    #[serde(deserialize_with = "go_int")]
    pub schema_version: i64,
    #[serde(deserialize_with = "null_default")]
    pub server_version: String,
    #[serde(deserialize_with = "go_int")]
    pub pid: i64,
    /// "tcp".
    #[serde(deserialize_with = "null_default")]
    pub transport: String,
    /// "connect".
    #[serde(deserialize_with = "null_default")]
    pub protocol: String,
    #[serde(deserialize_with = "null_default")]
    pub host: String,
    #[serde(deserialize_with = "go_int")]
    pub port: i64,
    #[serde(deserialize_with = "null_default")]
    pub url: String,
    #[serde(deserialize_with = "null_default")]
    pub auth_token: String,
    #[serde(deserialize_with = "null_default")]
    pub auth_token_file: String,
    #[serde(deserialize_with = "null_default")]
    pub workspace_ref: String,
    #[serde(deserialize_with = "null_default")]
    pub state_root: String,
}

/// How to start a bridge. Empty paths mean "not set".
#[derive(Debug, Clone, Default)]
pub struct BridgeConfig {
    /// The `cursor-sdk-bridge` launcher (resolved by [`crate::locate`]).
    pub bin: PathBuf,
    /// Exported as CURSOR_API_KEY so runs authenticate without a
    /// `cursor-agent login`.
    pub api_key: String,
    /// The worktree this bridge serves: `--workspace` and the process cwd.
    /// GUI apps start with cwd "/", and the SDK's shell tool falls back to
    /// `process.cwd()` when the model omits `working_directory`.
    pub workspace: PathBuf,
    /// Tees stderr and Connect faults to `cursor-bridge.log` and holds
    /// `cursor-bridge.pid` for reaping. Per-worktree bridges use distinct
    /// dirs. Empty keeps the ring only and skips reaping (throwaway probes).
    pub log_dir: PathBuf,
    /// Exported as RECUR_CURSOR_ATTRIBUTION_PATH.
    pub attribution_config_path: PathBuf,
    /// The home that holds `~/.cursor/projects` (`None` = `$HOME`).
    pub home: Option<PathBuf>,
}

struct Proc {
    pid: i32,
    /// Cancelled once the single reaper task has observed the exit.
    reaped: CancellationToken,
}

/// A running bridge plus the Connect client bound to it. Agents for one
/// worktree are multiplexed over it by agent id.
pub struct Bridge {
    cfg: BridgeConfig,
    disc: Discovery,
    client: Option<Arc<Client>>,
    started_at: Option<DateTime<Utc>>,
    log: Arc<log::BridgeLog>,
    proc: Option<Proc>,
    closed: AtomicBool,
}

impl Bridge {
    /// Spawns the bridge and waits for its ready line. The caller owns the
    /// live process and must [`Bridge::close`] it.
    pub async fn start(cfg: BridgeConfig, cancel: &CancellationToken) -> Result<Arc<Bridge>, Error> {
        if cfg.bin.as_os_str().is_empty() {
            return Err(Error::msg("cursor bridge: no bridge binary configured"));
        }
        if let Err(e) = std::fs::metadata(&cfg.bin) {
            return Err(Error::msg(format!("cursor bridge: {}: {e}", cfg.bin.display())));
        }
        let mut args: Vec<String> = vec!["--port".into(), "0".into()];
        let has_ws = !cfg.workspace.as_os_str().is_empty();
        if has_ws {
            if !std::fs::metadata(&cfg.workspace).is_ok_and(|m| m.is_dir()) {
                return Err(Error::msg(format!(
                    "cursor bridge: workspace {:?} is unavailable",
                    cfg.workspace.display().to_string()
                )));
            }
            args.push("--workspace".into());
            args.push(cfg.workspace.to_string_lossy().into_owned());
        }

        let mut cmd = tokio::process::Command::new(&cfg.bin);
        cmd.args(&args)
            .process_group(0)
            .kill_on_drop(false)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if has_ws {
            cmd.current_dir(&cfg.workspace);
            // A missing ~/.cursor/projects/<slug>/terminals makes Cursor's
            // Shell tool crash the bridge with a misleading spawn ENOENT.
            if let Err(e) = ensure_cursor_project_dirs_in(cfg.home.as_deref(), &cfg.workspace) {
                tracing::warn!(
                    "recur: ensure cursor project dirs for {}: {e}",
                    cfg.workspace.display()
                );
            }
        }
        let env = bridge_env(&cfg, std::env::vars_os());
        cmd.env_clear().envs(env);

        let log = Arc::new(log::BridgeLog::open(&cfg.log_dir));
        log.note(&format!(
            "recur: starting cursor-sdk-bridge bin={} workspace={}",
            cfg.bin.display(),
            cfg.workspace.display()
        ));

        // The process group outlives a crashed Recur: reap leftovers from
        // this launcher (and our pidfile) before spawning another.
        {
            let (bin, dir, log) = (cfg.bin.clone(), cfg.log_dir.clone(), log.clone());
            let _ = tokio::task::spawn_blocking(move || {
                reap::reap_stale_bridges(&bin, &dir, 0, &|l| log.note(l));
            })
            .await;
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                log.close();
                return Err(Error::msg(format!("start cursor bridge: {e}")));
            }
        };
        let pid = child.id().map_or(0, |p| p as i32);
        let stderr = child.stderr.take();

        // The single reaper owning wait(): a cancel path can never race a
        // second wait.
        let reaped = CancellationToken::new();
        {
            let reaped = reaped.clone();
            let log_dir = cfg.log_dir.clone();
            tokio::spawn(async move {
                let _ = child.wait().await;
                reaped.cancel();
                // Exited: drop the pidfile so a later start does not signal a
                // reused pid.
                reap::clear_bridge_pidfile(&log_dir);
            });
        }
        let proc = Proc { pid, reaped };

        let disc = match await_ready(stderr, log.clone(), cancel).await {
            Ok(d) => d,
            Err(e) => {
                kill(&proc).await;
                log.close();
                return Err(e);
            }
        };
        if let Some(p) = log.path() {
            tracing::info!("recur: cursor bridge ready on {} (stderr → {})", disc.url, p.display());
        }

        let mut token = disc.auth_token.clone();
        if token.is_empty() && !disc.auth_token_file.is_empty() {
            match std::fs::read_to_string(&disc.auth_token_file) {
                Ok(raw) => token = trim(&raw).to_owned(),
                Err(e) => {
                    kill(&proc).await;
                    log.close();
                    return Err(Error::msg(format!("cursor bridge: read auth token: {e}")));
                }
            }
        }
        if token.is_empty() {
            kill(&proc).await;
            log.close();
            return Err(Error::msg("cursor bridge: discovery carried no auth token"));
        }

        reap::write_bridge_pidfile(&cfg.log_dir, pid);

        let note_log = log.clone();
        let client = Client::new(&disc.url, &token).with_log(Arc::new(move |l: &str| note_log.note(l)));
        Ok(Arc::new(Bridge {
            cfg,
            disc,
            client: Some(Arc::new(client)),
            started_at: Some(Utc::now()),
            log,
            proc: Some(proc),
            closed: AtomicBool::new(false),
        }))
    }

    /// A bridge bound to an existing Connect endpoint without a process, for
    /// tests that drive the runner against a stand-in server (Go
    /// `BindClientForTest` + `SetStartedAtForTest`).
    pub fn bound(base_url: &str, token: &str, started_at: Option<DateTime<Utc>>) -> Arc<Bridge> {
        Arc::new(Bridge {
            cfg: BridgeConfig::default(),
            disc: Discovery::default(),
            client: Some(Arc::new(Client::new(base_url, token))),
            started_at,
            log: Arc::new(log::BridgeLog::open(Path::new(""))),
            proc: None,
            closed: AtomicBool::new(false),
        })
    }

    pub(crate) fn client(&self) -> Option<&Arc<Client>> {
        self.client.as_ref()
    }

    /// The Connect client for direct sdk.v1 calls.
    pub fn connect_client(&self) -> Option<Arc<Client>> {
        self.client.clone()
    }

    pub fn discovery(&self) -> &Discovery {
        &self.disc
    }

    pub fn workspace(&self) -> &Path {
        &self.cfg.workspace
    }

    /// When the bridge reported ready; `None` for bound test bridges, whose
    /// run timestamps are then unknown.
    pub fn started_at(&self) -> Option<DateTime<Utc>> {
        self.started_at
    }

    /// Whether the process still runs. A dead Node bridge must not be reused,
    /// or every CreateAgent fails with connection refused on a dead port.
    pub fn alive(&self) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        self.proc.as_ref().is_some_and(|p| !p.reaped.is_cancelled())
    }

    /// Recent stderr lines (redacted) for agent failures, plus the log path.
    pub fn stderr_tail(&self) -> String {
        self.log.tail()
    }

    /// The on-disk stderr capture; `None` when logging is off.
    pub fn log_path(&self) -> Option<PathBuf> {
        self.log.path()
    }

    /// Asks the bridge to shut down, then terminates its process group.
    /// Idempotent.
    pub async fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // Lets the bridge flush local agent state before the signal.
        if let Some(c) = &self.client {
            let tok = CancellationToken::new();
            let _ = tokio::time::timeout(Duration::from_secs(3), c.shutdown(&tok)).await;
        }
        if let Some(p) = &self.proc {
            kill(p).await;
        }
        reap::clear_bridge_pidfile(&self.cfg.log_dir);
        self.log.close();
    }

    #[cfg(test)]
    pub(crate) fn pid(&self) -> i32 {
        self.proc.as_ref().map_or(0, |p| p.pid)
    }

    #[cfg(test)]
    pub(crate) fn with_reaped_for_test(reaped: CancellationToken, closed: bool) -> Bridge {
        Bridge {
            cfg: BridgeConfig::default(),
            disc: Discovery::default(),
            client: None,
            started_at: None,
            log: Arc::new(log::BridgeLog::open(Path::new(""))),
            proc: Some(Proc { pid: 0, reaped }),
            closed: AtomicBool::new(closed),
        }
    }
}

/// Terminates the process group (SIGTERM, grace, SIGKILL) and waits for the
/// reaper to see the exit.
async fn kill(p: &Proc) {
    let Some(pg) = Pid::from_raw(p.pid) else {
        return;
    };
    if p.reaped.is_cancelled() {
        return;
    }
    let _ = kill_process_group(pg, Signal::TERM);
    if tokio::time::timeout(BRIDGE_KILL_GRACE, p.reaped.cancelled())
        .await
        .is_err()
    {
        let _ = kill_process_group(pg, Signal::KILL);
        p.reaped.cancelled().await;
    }
}

/// The child environment: the parent's without LINEAR_API_KEY, plus the SDK
/// client tag, the key, the attribution path, and a usable SHELL.
fn bridge_env(
    cfg: &BridgeConfig,
    parent: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut env: Vec<_> = parent.filter(|(k, _)| k != "LINEAR_API_KEY").collect();
    env.push(("CURSOR_SDK_CLIENT_LANGUAGE".into(), "go".into()));
    if !cfg.api_key.is_empty() {
        env.push(("CURSOR_API_KEY".into(), cfg.api_key.clone().into()));
    }
    if !cfg.attribution_config_path.as_os_str().is_empty() {
        env.push((ENV_ATTRIBUTION_PATH.into(), cfg.attribution_config_path.clone().into()));
    }
    ensure_bridge_shell_env(env)
}

/// Pins SHELL to an absolute shell that exists when the inherited one is
/// missing or unusable (Dock-launched macOS apps get a thin environment), so
/// Cursor's shell tool gets a real binary.
pub(crate) fn ensure_bridge_shell_env(
    mut env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let shell = env
        .iter()
        .rev()
        .find(|(k, _)| k == "SHELL")
        .map(|(_, v)| PathBuf::from(v));
    if shell.as_deref().is_some_and(shell_ok) {
        return env;
    }
    if let Some(c) = ["/bin/zsh", "/bin/bash", "/bin/sh"]
        .into_iter()
        .find(|c| shell_ok(Path::new(c)))
    {
        env.retain(|(k, _)| k != "SHELL");
        env.push(("SHELL".into(), c.into()));
    }
    env
}

fn shell_ok(p: &Path) -> bool {
    p.is_absolute() && crate::locate::is_executable_file(p)
}

/// Scans stderr for the ready line. Every line, before and after ready, is
/// recorded (redacted). Scanning continues after ready so a chatty bridge
/// never fills the pipe and wedges itself.
async fn await_ready(
    stderr: Option<impl AsyncRead + Unpin + Send + 'static>,
    log: Arc<log::BridgeLog>,
    cancel: &CancellationToken,
) -> Result<Discovery, Error> {
    let Some(stderr) = stderr else {
        return Err(Error::msg("cursor bridge: exited before reporting ready"));
    };
    let (tx, rx) = oneshot::channel::<Result<Discovery, Error>>();
    tokio::spawn(async move {
        let mut tx = Some(tx);
        let mut r = BufReader::new(stderr);
        let mut buf = Vec::new();
        let scan_err = loop {
            buf.clear();
            match read_line_capped(&mut r, &mut buf, 1 << 20).await {
                Ok(false) => break None,
                Ok(true) => {}
                Err(e) => break Some(e),
            }
            let line = String::from_utf8_lossy(&buf).into_owned();
            log.note(&line);
            if tx.is_none() {
                continue;
            }
            let Some(idx) = line.find(READY_PREFIX) else {
                continue;
            };
            let res = serde_json::from_str::<Discovery>(&line[idx + READY_PREFIX.len()..])
                .map_err(|e| Error::msg(format!("cursor bridge: bad discovery json: {e}")));
            if let Some(tx) = tx.take() {
                let _ = tx.send(res);
            }
        };
        if let Some(tx) = tx.take() {
            let _ = tx.send(Err(Error::msg("cursor bridge: exited before reporting ready")));
        }
        match scan_err {
            Some(e) => log.note(&format!("recur: bridge stderr scan ended: {e}")),
            None => log.note("recur: bridge stderr closed (process exiting)"),
        }
    });
    tokio::select! {
        r = rx => {
            let d = r.unwrap_or_else(|_| Err(Error::msg("cursor bridge: exited before reporting ready")))?;
            validate_discovery(&d)?;
            Ok(d)
        }
        _ = cancel.cancelled() => Err(Error::canceled()),
        _ = tokio::time::sleep(BRIDGE_START_TIMEOUT) => Err(Error::msg(format!(
            "cursor bridge: not ready after {}s", BRIDGE_START_TIMEOUT.as_secs()
        ))),
    }
}

/// One `\n`-terminated line without its terminator (and a trailing `\r`),
/// like `bufio.Scanner`; false at EOF. A line over `max` ends the scan.
async fn read_line_capped(
    r: &mut (impl AsyncBufReadExt + Unpin),
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<bool> {
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(!buf.is_empty());
        }
        let (take, done) = match avail.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (avail.len(), false),
        };
        buf.extend_from_slice(&avail[..take]);
        r.consume(take);
        if buf.len() > max + 1 {
            return Err(std::io::Error::other("bufio.Scanner: token too long"));
        }
        if done {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(true);
        }
    }
}

/// Fails closed on any discovery shape this client was not built against.
fn validate_discovery(d: &Discovery) -> Result<(), Error> {
    if d.schema_version != 1 {
        return Err(Error::msg(format!(
            "cursor bridge: unsupported discovery schemaVersion {} (want 1)",
            d.schema_version
        )));
    }
    if d.transport != "tcp" || d.protocol != "connect" {
        return Err(Error::msg(format!(
            "cursor bridge: unsupported transport {:?}/{:?} (want tcp/connect)",
            d.transport, d.protocol
        )));
    }
    if d.url.is_empty() {
        return Err(Error::msg("cursor bridge: discovery carried no url"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
