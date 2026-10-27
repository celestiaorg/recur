//! A running `codex app-server` (Go `codex/server.go`): spawned per turn
//! (and per probe) in its own process group, with Recur's `CODEX_HOME`, a
//! trusted-project entry for the worktree, optional seeded ChatGPT auth, the
//! initialize handshake, a stderr ring, and group kill on close.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::mcp::config_args;
use crate::proc::{self, Reaped, go_errno, kill_group, path_error, wait_reaped};
use crate::rpc::{BoxFuture, Conn, Ctx};

/// The initialize handshake budget.
pub const START_BUDGET: Duration = Duration::from_secs(45);

/// How to launch app-server.
#[derive(Debug, Clone, Default)]
pub struct ServerConfig {
    pub bin: String,
    /// `CODEX_HOME` (Recur-owned).
    pub home: PathBuf,
    /// The process working directory.
    pub worktree: PathBuf,
    /// Optional `CODEX_API_KEY`.
    pub api_key: String,
    /// `-c key=value` entries (MCP, approval, sandbox).
    pub config_overrides: Vec<String>,
    /// An `auth.json` copied into `home` before start (ChatGPT login reuse).
    /// Never the worktree.
    pub seed_auth_from: Option<PathBuf>,
    /// Environment variables the process must not inherit (tracker secrets).
    pub env_scrub: Vec<String>,
}

/// A live app-server and its connection. Call [`Server::close`] when done.
pub struct Server {
    conn: Conn,
    pid: u32,
    reaped: Reaped,
    stderr: Arc<LineRing>,
    closed: AtomicBool,
}

impl Server {
    /// Spawns app-server and completes initialize/initialized within
    /// [`START_BUDGET`] (bounded by `ctx`).
    pub async fn start(cfg: &ServerConfig, ctx: &Ctx) -> Result<Self, String> {
        if cfg.bin.is_empty() {
            return Err("codex server: no binary".into());
        }
        if cfg.home.as_os_str().is_empty() {
            return Err("codex server: CODEX_HOME required".into());
        }
        if let Err(e) = std::fs::create_dir_all(&cfg.home) {
            return Err(format!(
                "codex server: mkdir CODEX_HOME: {}",
                path_error("mkdir", &cfg.home, &e)
            ));
        }
        if let Some(from) = &cfg.seed_auth_from {
            seed_auth(from, &cfg.home.join("auth.json"))?;
        }
        // An isolated CODEX_HOME has no project trust table; without this,
        // sandbox writes in a loop worktree fail with "workspace tool failed
        // to start".
        if !cfg.worktree.as_os_str().is_empty() {
            ensure_trusted_project(&cfg.home, &cfg.worktree)?;
        }

        let mut cmd = Command::new(&cfg.bin);
        cmd.arg("app-server")
            .args(config_args(&cfg.config_overrides))
            .process_group(0)
            .env_clear()
            .envs(launch_env(
                std::env::vars_os(),
                &cfg.home,
                &cfg.api_key,
                &cfg.env_scrub,
            ))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if !cfg.worktree.as_os_str().is_empty() {
            cmd.current_dir(&cfg.worktree);
        }
        let mut child = cmd.spawn().map_err(|e| {
            format!(
                "start codex app-server: fork/exec {}: {}",
                cfg.bin,
                go_errno(&e)
            )
        })?;
        let pid = child.id().unwrap_or_default();
        let (Some(stdin), Some(stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("start codex app-server: missing stdio pipe".into());
        };
        let ring = Arc::new(LineRing::new(80));
        let drain = Arc::clone(&ring);
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let mut partial: Vec<u8> = Vec::new();
            loop {
                match stderr.read(&mut buf).await {
                    Ok(n) if n > 0 => {
                        partial.extend_from_slice(&buf[..n]);
                        while let Some(i) = partial.iter().position(|&b| b == b'\n') {
                            let line: Vec<u8> = partial.drain(..=i).collect();
                            drain.add(String::from_utf8_lossy(&line[..i]).into_owned());
                        }
                    }
                    _ => break,
                }
            }
            if !partial.is_empty() {
                drain.add(String::from_utf8_lossy(&partial).into_owned());
            }
        });
        let reaped = proc::reap(child);
        let conn = Conn::new(stdout, stdin);
        conn.on_server_request(Some(Arc::new(|method: String, params: String| {
            Box::pin(async move { auto_approve(&method, &params) }) as BoxFuture<_>
        })));
        let srv = Self {
            conn,
            pid,
            reaped,
            stderr: ring,
            closed: AtomicBool::new(false),
        };
        if let Err(e) = srv.handshake(&ctx.with_timeout(START_BUDGET)).await {
            srv.close().await;
            return Err(e);
        }
        Ok(srv)
    }

    async fn handshake(&self, ctx: &Ctx) -> Result<(), String> {
        let params = json!({"clientInfo": {"name": "recur", "title": "Recur", "version": "0.1.0"}});
        ctx.run(self.conn.call::<_, Value>("initialize", &params))
            .await
            .map_err(|e| format!("codex initialize: {e}"))?;
        self.conn
            .notify("initialized", &json!({}))
            .await
            .map_err(|e| format!("codex initialized: {e}"))
    }

    pub fn conn(&self) -> &Conn {
        &self.conn
    }

    /// Resolves once the process has been waited, so a mid-turn crash is
    /// seen without waiting on a hung RPC.
    pub async fn reaped(&self) {
        let mut r = self.reaped.clone();
        wait_reaped(&mut r).await;
    }

    pub fn alive(&self) -> bool {
        !*self.reaped.borrow()
    }

    /// Recent stderr lines for error enrichment.
    pub fn stderr_tail(&self) -> String {
        self.stderr.to_string()
    }

    /// Kills the process group and waits for the exit. Idempotent.
    pub async fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut r = self.reaped.clone();
        kill_group(self.pid, &mut r).await;
    }
}

/// Appends a `projects.<cwd>` trust entry to `CODEX_HOME/config.toml` when
/// missing. Never touches the worktree or `~/.codex`.
pub fn ensure_trusted_project(home: &Path, worktree: &Path) -> Result<(), String> {
    use std::io::Write;
    let abs = proc::abs(worktree).map_err(|e| go_errno(&e))?;
    let cfg_path = home.join("config.toml");
    let marker = format!("[projects.{}]", go_quote(&abs.to_string_lossy()));
    if std::fs::read_to_string(&cfg_path).is_ok_and(|raw| raw.contains(&marker)) {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cfg_path)
        .map_err(|e| path_error("open", &cfg_path, &e))?;
    write!(f, "\n{marker}\ntrust_level = \"trusted\"\n")
        .map_err(|e| path_error("write", &cfg_path, &e))
}

/// Copies the machine login into Recur's home, preserving tokens Codex
/// refreshed there: re-seed only when Recur has no auth yet or the source is
/// strictly newer.
pub fn seed_auth(from: &Path, to: &Path) -> Result<(), String> {
    let raw = match std::fs::read(from) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "codex ChatGPT auth: {} not found — run `codex login` or set an API key",
                from.display()
            ));
        }
        Err(e) => return Err(path_error("open", from, &e)),
    };
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| path_error("mkdir", dir, &e))?;
    }
    if raw.is_empty() {
        return Err(format!("codex ChatGPT auth: {} is empty", from.display()));
    }
    if let (Ok(dst), Ok(src)) = (std::fs::metadata(to), std::fs::metadata(from))
        && let (Ok(dst), Ok(src)) = (dst.modified(), src.modified())
        && src <= dst
    {
        return Ok(());
    }
    write_private(to, &raw).map_err(|e| path_error("open", to, &e))
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(data)
}

/// The user's machine login file.
pub fn default_seed_auth_path() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join(".codex/auth.json"))
}

/// The child environment: `base` minus `scrub`, plus `CODEX_HOME` and (when
/// set) `CODEX_API_KEY`. A host `CODEX_API_KEY` survives when Recur sets
/// none, as in Go.
pub fn launch_env(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    home: &Path,
    api_key: &str,
    scrub: &[String],
) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = base
        .into_iter()
        .filter(|(k, _)| k != "CODEX_HOME" && !scrub.iter().any(|s| k == s.as_str()))
        .collect();
    env.push(("CODEX_HOME".into(), home.into()));
    if !api_key.is_empty() {
        env.retain(|(k, _)| k != "CODEX_API_KEY");
        env.push(("CODEX_API_KEY".into(), api_key.into()));
    }
    env
}

/// Accepts MCP elicitations and approvals so automation is never wedged
/// (paired with `approval_policy=never` and MCP `approve`). The runner
/// replaces it with human prompts when permissions are not bypassed.
pub fn auto_approve(method: &str, _params: &str) -> Result<Value, String> {
    Ok(match method {
        "mcpServer/elicitation/request" => json!({"action": "accept", "content": {}}),
        "item/commandExecution/requestApproval" | "commandExecution/requestApproval" => {
            json!({"decision": "acceptForSession"})
        }
        "execCommandApproval" => json!({"decision": "approved"}),
        "applyPatchApproval" | "fileChange/requestApproval" | "item/fileChange/requestApproval" => {
            json!({"decision": "acceptForSession"})
        }
        "item/permissions/requestApproval" => json!({"decision": "accept"}),
        // Never hang the turn on an unknown request.
        _ => json!({}),
    })
}

/// A small stderr ring.
pub struct LineRing {
    lines: Mutex<Vec<String>>,
    max: usize,
}

impl LineRing {
    pub fn new(max: usize) -> Self {
        Self {
            lines: Mutex::default(),
            max,
        }
    }

    pub fn add(&self, line: String) {
        let mut l = self.lines.lock();
        l.push(line);
        if l.len() > self.max {
            let cut = l.len() - self.max;
            l.drain(..cut);
        }
    }
}

impl std::fmt::Display for LineRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.lines.lock().join("\n"))
    }
}

/// Go `strconv.Quote` for a path (what `%q` prints): printable runes as is,
/// the rest escaped.
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            // simplify: above ASCII only controls are escaped; Go's IsPrint
            // also escapes unassigned and format runes, which real paths do
            // not contain.
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_auth_preserves_newer_home_copy() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("machine-auth.json");
        let to = dir.path().join("home/auth.json");
        std::fs::write(&from, br#"{"old":true}"#).unwrap();
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::write(&to, br#"{"refreshed":true}"#).unwrap();
        let past = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&from)
            .unwrap()
            .set_modified(past)
            .unwrap();
        seed_auth(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), br#"{"refreshed":true}"#);
    }

    #[test]
    fn seed_auth_copies_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("machine-auth.json");
        let to = dir.path().join("home/auth.json");
        std::fs::write(&from, br#"{"ok":true}"#).unwrap();
        seed_auth(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), br#"{"ok":true}"#);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&to).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let missing = dir.path().join("nope.json");
        assert_eq!(
            seed_auth(&missing, &to).unwrap_err(),
            format!(
                "codex ChatGPT auth: {} not found — run `codex login` or set an API key",
                missing.display()
            )
        );
        std::fs::write(&from, b"").unwrap();
        assert!(seed_auth(&from, &to).unwrap_err().ends_with(" is empty"));
    }

    #[test]
    fn trusted_project_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        ensure_trusted_project(home.path(), wt.path()).unwrap();
        let raw = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(raw.contains("trust_level = \"trusted\""));
        assert!(raw.contains(&*wt.path().to_string_lossy()));
        assert_eq!(
            raw,
            format!(
                "\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                wt.path().display()
            )
        );
        ensure_trusted_project(home.path(), wt.path()).unwrap();
        let raw = std::fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert_eq!(raw.matches("trust_level").count(), 1);
    }

    #[test]
    fn launch_env_scrubs_and_sets_home() {
        let base = [
            ("PATH", "/bin"),
            ("LINEAR_API_KEY", "secret"),
            ("CODEX_HOME", "/old"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let got = launch_env(
            base,
            Path::new("/tmp/codex-home"),
            "sk-x",
            &["LINEAR_API_KEY".into()],
        );
        let pairs: Vec<String> = got
            .iter()
            .map(|(k, v)| format!("{}={}", k.to_string_lossy(), v.to_string_lossy()))
            .collect();
        assert_eq!(
            pairs,
            [
                "PATH=/bin",
                "CODEX_HOME=/tmp/codex-home",
                "CODEX_API_KEY=sk-x"
            ]
        );
    }

    #[test]
    fn auto_approve_answers() {
        assert_eq!(
            auto_approve("mcpServer/elicitation/request", "").unwrap(),
            json!({"action": "accept", "content": {}})
        );
        assert_eq!(
            auto_approve("execCommandApproval", "").unwrap()["decision"],
            "approved"
        );
        assert_eq!(auto_approve("x/y", "").unwrap(), json!({}));
    }

    #[test]
    fn ring_keeps_last_lines() {
        let r = LineRing::new(2);
        for l in ["a", "b", "c"] {
            r.add(l.into());
        }
        assert_eq!(r.to_string(), "b\nc");
    }

    /// The full lifecycle against a fake app-server: argv overrides, env,
    /// handshake, auto-approved server requests, stderr tail, group kill.
    #[tokio::test]
    async fn start_handshake_and_close() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("log");
        let bin = dir.path().join("codex");
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{log}'
echo "home=$CODEX_HOME key=$CODEX_API_KEY" >> '{log}'
echo 'warming up' >&2
read -r init
echo '{{"id":1,"result":{{}}}}'
read -r initialized
printf '%s\n' "$init" "$initialized" >> '{log}'
echo '{{"method":"mcpServer/elicitation/request","id":"e1","params":{{}}}}'
read -r answer
printf '%s\n' "$answer" >> '{log}'
sleep 30
"#,
            log = log.display()
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = ServerConfig {
            bin: bin.to_string_lossy().into_owned(),
            home: dir.path().join("home"),
            worktree: dir.path().to_path_buf(),
            api_key: "sk-test".into(),
            config_overrides: vec![r#"approval_policy="never""#.into()],
            ..Default::default()
        };
        let srv = Server::start(&cfg, &Ctx::default()).await.unwrap();
        assert!(srv.alive());
        for _ in 0..200 {
            if std::fs::read_to_string(&log).is_ok_and(|l| l.lines().count() >= 7) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let logged = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = logged.lines().collect();
        assert_eq!(
            &lines[..3],
            ["app-server", "-c", r#"approval_policy="never""#]
        );
        assert_eq!(
            lines[3],
            format!("home={} key=sk-test", dir.path().join("home").display())
        );
        assert_eq!(
            lines[4],
            r#"{"method":"initialize","id":1,"params":{"clientInfo":{"name":"recur","title":"Recur","version":"0.1.0"}}}"#
        );
        assert_eq!(lines[5], r#"{"method":"initialized","params":{}}"#);
        assert_eq!(
            lines[6],
            r#"{"id":"e1","result":{"action":"accept","content":{}}}"#
        );
        assert_eq!(srv.stderr_tail(), "warming up");
        assert!(
            std::fs::read_to_string(dir.path().join("home/config.toml"))
                .unwrap()
                .contains("trust_level")
        );
        tokio::time::timeout(Duration::from_secs(5), srv.close())
            .await
            .unwrap();
        assert!(!srv.alive());
    }

    #[tokio::test]
    async fn handshake_failure_reports_and_kills() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("codex");
        std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = ServerConfig {
            bin: bin.to_string_lossy().into_owned(),
            home: dir.path().join("home"),
            ..Default::default()
        };
        let err = Server::start(&cfg, &Ctx::default()).await.err().unwrap();
        // The child can exit before the request is written (Linux: broken pipe) or after.
        assert!(
            err == "codex initialize: codex rpc -32001: EOF"
                || err == "codex initialize: write: broken pipe",
            "{err}"
        );
        let missing = ServerConfig {
            bin: dir.path().join("nope").to_string_lossy().into_owned(),
            home: dir.path().join("home"),
            ..Default::default()
        };
        let err = Server::start(&missing, &Ctx::default())
            .await
            .err()
            .unwrap();
        assert!(
            err.starts_with("start codex app-server: fork/exec "),
            "{err}"
        );
        assert!(err.ends_with(": no such file or directory"), "{err}");
    }
}
