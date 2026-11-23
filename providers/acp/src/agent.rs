//! A running ACP agent: spawned in its own process group with a scrubbed
//! environment, the `initialize` handshake, a stderr tail, and group kill on
//! close. Plus what a `session/new` answer says about models, effort and
//! modes, and the probe behind `models()` / `status()`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::{FlagDimension, FlagOption, ModelChoice};
use recur_domain::AcpAgent;
use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

use crate::launch::Launch;
use crate::rpc::{Conn, RpcError};

/// The initialize (and probe session) budget.
pub const START_BUDGET: Duration = Duration::from_secs(45);
const KILL_GRACE: Duration = Duration::from_secs(2);
const STDERR_LINES: usize = 40;

pub struct Agent {
    pub conn: Conn,
    /// The `initialize` result.
    pub init: Value,
    pid: u32,
    reaped: watch::Receiver<bool>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl Agent {
    /// Spawns the launch in `cwd` and completes `initialize`.
    pub async fn start(launch: &Launch, cwd: &Path) -> Result<Self, String> {
        let bin = &launch.bin;
        let mut cmd = Command::new(bin);
        cmd.args(&launch.args)
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .process_group(0)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for k in &launch.scrub {
            cmd.env_remove(k);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("start {}: {e}", bin.display()))?;
        let pid = child.id().unwrap_or_default();
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(format!("start {}: missing stdio pipe", bin.display()));
        };
        let ring: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let tail = Arc::clone(&ring);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let mut r = tail.lock();
                r.push_back(l);
                if r.len() > STDERR_LINES {
                    r.pop_front();
                }
            }
        });
        let (tx, reaped) = watch::channel(false);
        tokio::spawn(async move {
            let _ = child.wait().await;
            let _ = tx.send(true);
        });
        let mut agent = Self {
            conn: Conn::new(stdout, stdin),
            init: Value::Null,
            pid,
            reaped,
            stderr: ring,
        };
        let params = json!({
            "protocolVersion": 1,
            "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
            "clientInfo": {"name": "recur", "title": "Recur", "version": env!("CARGO_PKG_VERSION")},
        });
        let init = tokio::select! {
            r = tokio::time::timeout(START_BUDGET, agent.conn.call("initialize", params)) => match r {
                Ok(Ok(v)) => Ok(v),
                Ok(Err(e)) => Err(format!("initialize: {e}")),
                Err(_) => Err("initialize: timed out".to_owned()),
            },
            () = agent.exited() => Err("the agent exited during initialize".to_owned()),
        };
        match init {
            Ok(v) => {
                agent.init = v;
                Ok(agent)
            }
            Err(e) => {
                let e = with_tail(e, &agent.stderr_tail());
                agent.close().await;
                Err(e)
            }
        }
    }

    /// A capability under `agentCapabilities`, e.g. `["sessionCapabilities", "resume"]`.
    pub fn cap(&self, path: &[&str]) -> bool {
        let mut v = &self.init["agentCapabilities"];
        for k in path {
            v = &v[*k];
        }
        !v.is_null() && v != &Value::Bool(false)
    }

    pub async fn exited(&self) {
        let mut r = self.reaped.clone();
        let _ = r.wait_for(|done| *done).await;
    }

    pub fn stderr_tail(&self) -> String {
        Vec::from(self.stderr.lock().clone()).join("\n")
    }

    fn signal(&self, sig: Signal) {
        if !*self.reaped.borrow()
            && let Some(pgid) = i32::try_from(self.pid).ok().and_then(Pid::from_raw)
        {
            let _ = kill_process_group(pgid, sig);
        }
    }

    /// SIGTERM to the group, SIGKILL after a grace; returns once reaped.
    pub async fn close(&self) {
        self.signal(Signal::TERM);
        if tokio::time::timeout(KILL_GRACE, self.exited())
            .await
            .is_err()
        {
            self.signal(Signal::KILL);
            self.exited().await;
        }
    }
}

/// A dropped turn future must not leave the agent running.
impl Drop for Agent {
    fn drop(&mut self) {
        self.signal(Signal::KILL);
    }
}

pub fn with_tail(msg: String, tail: &str) -> String {
    if tail.trim().is_empty() {
        msg
    } else {
        format!("{msg}\n{tail}")
    }
}

/// What a `session/new`, `session/resume` or `session/load` answer exposes.
#[derive(Debug, Clone, Default)]
pub struct SessionInfo {
    pub config_options: Vec<Value>,
    /// The unstable session models API (`models.availableModels`).
    pub models: Value,
    /// `modes.availableModes` / `currentModeId`.
    pub modes: Value,
}

impl SessionInfo {
    pub fn from(v: &Value) -> Self {
        Self {
            config_options: v["configOptions"].as_array().cloned().unwrap_or_default(),
            models: v["models"].clone(),
            modes: v["modes"].clone(),
        }
    }

    /// The config option of an ACP category (`model`, `mode`, `thought_level`).
    pub fn option(&self, category: &str) -> Option<&Value> {
        self.config_options
            .iter()
            .find(|o| o["category"].as_str() == Some(category))
    }

    /// `(value, name)` for every model the session offers.
    pub fn model_options(&self) -> Vec<(String, String)> {
        if let Some(o) = self.option("model") {
            return select_options(o);
        }
        self.models["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let id = m["modelId"].as_str()?;
                Some((id.to_owned(), m["name"].as_str().unwrap_or(id).to_owned()))
            })
            .collect()
    }

    /// The thought_level option as Recur's `effort` dimension.
    pub fn effort(&self) -> Option<FlagDimension> {
        let o = self.option("thought_level")?;
        let current = o["currentValue"].as_str().unwrap_or_default();
        let options: Vec<FlagOption> = select_options(o)
            .into_iter()
            .map(|(value, label)| FlagOption {
                default: value == current,
                value,
                label,
            })
            .collect();
        (!options.is_empty()).then(|| FlagDimension {
            id: "effort".into(),
            label: o["name"].as_str().unwrap_or("Effort").to_owned(),
            options,
        })
    }

    pub fn choices(&self, agent: &AcpAgent) -> Vec<ModelChoice> {
        let flags: Vec<FlagDimension> = self.effort().into_iter().collect();
        let options = self.model_options();
        let shared = shared_group(options.iter().map(|(_, name)| name.as_str())).len();
        options
            .into_iter()
            .map(|(value, label)| ModelChoice {
                model_ref: format!("{}/{value}", agent.id),
                label: label[shared..].to_owned(),
                flags: flags.clone(),
                context_window: 0,
            })
            .collect()
    }
}

/// The `Group/` prefix every name shares (OpenCode names models
/// `<provider>/<model>`); empty when any name lacks it or they differ, so
/// mixed providers keep their prefix.
fn shared_group<'a>(mut names: impl Iterator<Item = &'a str>) -> &'a str {
    let Some(first) = names.next() else {
        return "";
    };
    let prefix = first.find('/').map_or("", |i| &first[..=i]);
    if prefix.is_empty() || first.len() == prefix.len() {
        return "";
    }
    if names.all(|n| n.starts_with(prefix) && n.len() > prefix.len()) {
        prefix
    } else {
        ""
    }
}

/// `(value, name)` of a select config option; grouped options are flattened.
pub fn select_options(o: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for opt in o["options"].as_array().into_iter().flatten() {
        if let Some(group) = opt["options"].as_array() {
            out.extend(group.iter().filter_map(option_pair));
        } else if let Some(p) = option_pair(opt) {
            out.push(p);
        }
    }
    out
}

fn option_pair(o: &Value) -> Option<(String, String)> {
    let v = o["value"].as_str()?;
    Some((v.to_owned(), o["name"].as_str().unwrap_or(v).to_owned()))
}

/// The result of one probe.
#[derive(Debug, Clone)]
pub struct Probe {
    pub version: String,
    /// `Err` carries a user-facing reason (login hint on auth failure).
    pub session: Result<SessionInfo, String>,
}

/// `initialize` + `session/new` in `<data_dir>/acp-probe/<id>`, then
/// `session/close` when advertised. `Err` only when the agent did not start.
pub async fn probe(agent: &AcpAgent, launch: &Launch, data_dir: &Path) -> Result<Probe, String> {
    let dir: PathBuf = data_dir.join("acp-probe").join(agent.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let a = Agent::start(launch, &dir).await?;
    let version = a.init["agentInfo"]["version"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let new = json!({"cwd": dir, "mcpServers": []});
    let session = match tokio::time::timeout(START_BUDGET, a.conn.call("session/new", new)).await {
        Ok(Ok(v)) => {
            if a.cap(&["sessionCapabilities", "close"])
                && let Some(sid) = v["sessionId"].as_str()
            {
                let close = a.conn.call("session/close", json!({"sessionId": sid}));
                let _ = tokio::time::timeout(Duration::from_secs(3), close).await;
            }
            Ok(SessionInfo::from(&v))
        }
        Ok(Err(e)) => Err(session_error(agent, &e)),
        Err(_) => Err("session/new timed out".into()),
    };
    a.close().await;
    Ok(Probe { version, session })
}

/// The user-facing wording for a failed session call.
pub fn session_error(agent: &AcpAgent, e: &RpcError) -> String {
    if e.code == crate::rpc::AUTH_REQUIRED {
        format!("{} is not signed in: {}", agent.label, agent.login_hint)
    } else {
        format!("{} session: {e}", agent.label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_provider_prefix_is_dropped() {
        let one = ["OpenCode Zen/Big Pickle", "OpenCode Zen/Beam Free"];
        assert_eq!(shared_group(one.into_iter()), "OpenCode Zen/");
        let mixed = ["OpenCode Zen/Big Pickle", "Anthropic/Claude Sonnet"];
        assert_eq!(shared_group(mixed.into_iter()), "");
        let plain = ["Gemini 3.8 Flash", "Claude Sonnet 4.6 (Thinking)"];
        assert_eq!(shared_group(plain.into_iter()), "");
        assert_eq!(shared_group(["A/"].into_iter()), "");
    }

    #[test]
    fn session_info_reads_options_and_models() {
        let s = SessionInfo::from(&json!({
            "configOptions": [
                {"id": "model", "category": "model", "currentValue": "a/x",
                 "options": [{"value": "a/x", "name": "X"}, {"group": "g", "options": [{"value": "a/y"}]}]},
                {"id": "effort", "name": "Thinking", "category": "thought_level", "currentValue": "high",
                 "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]},
            ]
        }));
        let agent = &recur_domain::ACP_AGENTS[1];
        let c = s.choices(agent);
        assert_eq!(
            c.iter()
                .map(|m| (m.model_ref.as_str(), m.label.as_str()))
                .collect::<Vec<_>>(),
            [("opencode/a/x", "X"), ("opencode/a/y", "a/y")]
        );
        let e = &c[0].flags[0];
        assert_eq!(
            (e.id.as_str(), e.label.as_str(), e.default_value()),
            ("effort", "Thinking", "high")
        );

        let s = SessionInfo::from(&json!({
            "models": {"currentModelId": "m1", "availableModels": [{"modelId": "m1", "name": "Model 1"}]}
        }));
        assert_eq!(s.model_options(), [("m1".to_owned(), "Model 1".to_owned())]);
        assert!(s.effort().is_none());

        let both = SessionInfo::from(&json!({
            "models": {"availableModels": [{"modelId": "g-high"}, {"modelId": "g-low"}]},
            "configOptions": [{"id": "model", "category": "model", "options": [{"value": "g", "name": "G"}]}]
        }));
        assert_eq!(
            both.model_options(),
            [("g".to_owned(), "G".to_owned())],
            "configOptions win"
        );
    }
}
