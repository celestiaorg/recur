//! Boots project dev servers and runs one-shot setup/teardown commands:
//! allocate a free port, start the process in its own group, probe the app
//! URL, and tear the group down. [`boot`] keeps the server for
//! verify-in-browser (PRD §10); [`run_cmd`] is for setup_cmd / teardown_cmd.
//!
//! Go's `context.Context` becomes a [`CancellationToken`] plus an optional
//! deadline, so callers can still tell a cancel from a timeout.

use std::io::Read as _;
use std::os::unix::process::ExitStatusExt as _;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;
use tokio::sync::{OnceCell, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(300);
const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(2);
const PROBE_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounds the combined stdout/stderr tail from [`run_cmd_output`].
pub const CMD_OUTPUT_TAIL_CAP: usize = 4 * 1024;
/// How long to wait for the shell's last bytes to land in the tail after it
/// exits. Descendants that still hold the pipe keep a drain thread instead
/// of having the pipe closed (which would SIGPIPE them).
const CMD_OUTPUT_WAIT_DELAY: Duration = Duration::from_millis(100);

/// Configures a long-lived dev-server boot for verify-in-browser. The zero
/// value uses the defaults. Setup belongs on workspace create, not here.
#[derive(Debug, Clone, Default)]
pub struct BootOptions {
    /// Where to run dev_cmd (the workspace worktree). Empty: the current dir.
    pub work_dir: String,
    /// The persisted workspace port; 0 allocates a fresh one.
    pub port: i64,
    /// The full environment for dev_cmd; `None` is the host environment plus
    /// RECUR_PORT and PORT.
    pub env: Option<Vec<String>>,
    /// Total time to wait for the URL to respond; zero is the default.
    pub probe_timeout: Duration,
    /// Gap between URL probes; zero is the default.
    pub poll_interval: Duration,
}

impl BootOptions {
    fn probe_timeout(&self) -> Duration {
        if self.probe_timeout.is_zero() {
            DEFAULT_PROBE_TIMEOUT
        } else {
            self.probe_timeout
        }
    }

    fn poll_interval(&self) -> Duration {
        if self.poll_interval.is_zero() {
            DEFAULT_POLL_INTERVAL
        } else {
            self.poll_interval
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error("no dev_cmd configured")]
    NoDevCmd,
    #[error("allocate port: {0}")]
    AllocatePort(std::io::Error),
    #[error("start dev server: {0}")]
    Start(std::io::Error),
    #[error("http client: {0}")]
    Client(String),
    #[error("dev server exited before serving{}", .0.as_ref().map(|d| format!(": {d}")).unwrap_or_default())]
    Exited(Option<String>),
    #[error("no HTTP response at {url} within {}", go_duration(*timeout))]
    NoResponse { url: String, timeout: Duration },
    #[error("context canceled")]
    Canceled,
}

/// Why [`run_cmd_output`] failed.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The command could not be started (Go returns the start error as-is).
    #[error("{0}")]
    Start(std::io::Error),
    #[error("{0}")]
    Pipe(std::io::Error),
    /// The shell exited non-zero or was killed by a signal.
    #[error("{}", exit_status_string(*.0))]
    Exit(ExitStatus),
    #[error("{0}")]
    Wait(String),
    #[error("context canceled")]
    Canceled,
    #[error("context deadline exceeded")]
    DeadlineExceeded,
}

impl RunError {
    /// The exit code for a non-zero exit (`None` when killed by a signal or
    /// for other errors), like Go's `ExitError.ExitCode` (-1 there).
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            RunError::Exit(s) => s.code(),
            _ => None,
        }
    }
}

type WaitResult = Result<ExitStatus, String>;

/// A spawned process group with a single reaper: every observer waits on
/// the same broadcast, so only one wait ever runs on the pid.
struct Group {
    pid: i32,
    reaped: watch::Receiver<Option<WaitResult>>,
    stopped: OnceCell<()>,
}

impl Group {
    fn spawn(mut child: tokio::process::Child) -> Arc<Self> {
        let pid = child.id().map_or(0, |p| p as i32);
        let (tx, rx) = watch::channel(None);
        tokio::spawn(async move {
            let r = child.wait().await.map_err(|e| e.to_string());
            let _ = tx.send(Some(r));
        });
        Arc::new(Self {
            pid,
            reaped: rx,
            stopped: OnceCell::new(),
        })
    }

    async fn wait(&self) -> WaitResult {
        let mut rx = self.reaped.clone();
        match rx.wait_for(Option::is_some).await {
            Ok(v) => v
                .clone()
                .unwrap_or_else(|| Err("process wait lost".to_owned())),
            Err(_) => Err("process wait lost".to_owned()),
        }
    }

    fn is_reaped(&self) -> bool {
        self.reaped.borrow().is_some()
    }

    /// Terminates the whole process group: SIGTERM, then SIGKILL after a
    /// short grace, returning once the leader is reaped. Never waits itself;
    /// it observes the reaper's broadcast.
    async fn kill(&self) {
        if self.pid <= 0 {
            return;
        }
        let pg = Pid::from_raw(self.pid);
        let _ = killpg(pg, Signal::SIGTERM);
        if tokio::time::timeout(DEFAULT_KILL_GRACE, self.wait())
            .await
            .is_err()
        {
            let _ = killpg(pg, Signal::SIGKILL);
            let _ = self.wait().await;
        }
    }

    /// Idempotent teardown: concurrent and repeated callers share one kill.
    async fn stop(&self) {
        self.stopped.get_or_init(|| self.kill()).await;
    }
}

/// A running dev server: the resolved `$RECUR_PORT`, the probed app URL,
/// and [`Booted::stop`], which tears the whole process group down.
///
/// The caller must call `stop`; dropping a `Booted` leaves the server
/// running, as in Go.
pub struct Booted {
    pub port: String,
    pub app_url: String,
    group: Arc<Group>,
}

impl Booted {
    /// Tears the dev server's process group down. Idempotent.
    pub async fn stop(&self) {
        self.group.stop().await;
    }
}

impl std::fmt::Debug for Booted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Booted")
            .field("port", &self.port)
            .field("app_url", &self.app_url)
            .field("pid", &self.group.pid)
            .finish()
    }
}

fn apply_env(cmd: &mut tokio::process::Command, env: &[String]) {
    cmd.env_clear();
    for kv in env {
        if let Some((k, v)) = kv.split_once('=')
            && !k.is_empty()
        {
            cmd.env(k, v);
        }
    }
}

fn shell(script: &str, work_dir: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(script)
        .process_group(0)
        .stdin(Stdio::null());
    if !work_dir.is_empty() {
        cmd.current_dir(work_dir);
    }
    cmd
}

/// Boots the dev server on `$RECUR_PORT` (`opts.port`, else a freshly
/// allocated port) and polls the app URL until it responds. The server keeps
/// running after a successful boot, even if `cancel` fires later; its
/// lifetime is owned by [`Booted::stop`]. On any failure everything started
/// is torn down first.
pub async fn boot(
    cancel: &CancellationToken,
    dev_cmd: &str,
    app_url: &str,
    opts: BootOptions,
) -> Result<Booted, BootError> {
    if dev_cmd.trim().is_empty() {
        return Err(BootError::NoDevCmd);
    }
    let port = if opts.port > 0 {
        opts.port
    } else {
        i64::from(free_port().map_err(BootError::AllocatePort)?)
    };
    let port = port.to_string();

    let mut probe_url = substitute_port(app_url, &port);
    if probe_url.trim().is_empty() {
        probe_url = format!("http://localhost:{port}");
    }
    let client = probe_client(&probe_url).map_err(|e| BootError::Client(e.to_string()))?;

    let mut cmd = shell(dev_cmd, &opts.work_dir);
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    match &opts.env {
        Some(env) => apply_env(&mut cmd, env),
        None => {
            cmd.env("RECUR_PORT", &port).env("PORT", &port);
        }
    }
    let child = cmd.spawn().map_err(BootError::Start)?;
    let group = Group::spawn(child);

    let deadline = Instant::now() + opts.probe_timeout();
    let poll = opts.poll_interval();
    let mut ticker = tokio::time::interval_at(Instant::now() + poll, poll);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if probe(&client, &probe_url).await.is_some() {
            return Ok(Booted {
                port,
                app_url: probe_url,
                group,
            });
        }
        tokio::select! {
            _ = cancel.cancelled() => {
                group.stop().await;
                return Err(BootError::Canceled);
            }
            r = group.wait() => {
                group.stop().await;
                let detail = match r {
                    Ok(s) => (!s.success()).then(|| exit_status_string(s)),
                    Err(e) => Some(e),
                };
                return Err(BootError::Exited(detail));
            }
            _ = ticker.tick() => {
                if Instant::now() > deadline {
                    group.stop().await;
                    return Err(BootError::NoResponse { url: probe_url, timeout: opts.probe_timeout() });
                }
            }
        }
    }
}

/// Keeps the last `max` bytes written to it.
struct TailWriter {
    buf: Vec<u8>,
    max: usize,
}

impl TailWriter {
    fn write(&mut self, p: &[u8]) {
        let n = p.len();
        if self.max == 0 {
            return;
        }
        if n >= self.max {
            self.buf.clear();
            self.buf.extend_from_slice(&p[n - self.max..]);
            return;
        }
        let extra = (self.buf.len() + n).saturating_sub(self.max);
        if extra > 0 {
            self.buf.drain(..extra);
        }
        self.buf.extend_from_slice(p);
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.buf).trim().to_owned()
    }
}

fn tail_text(t: &Mutex<TailWriter>) -> String {
    t.lock().unwrap_or_else(|e| e.into_inner()).text()
}

fn ctx_err(cancel: &CancellationToken, deadline: Option<Instant>) -> Option<RunError> {
    if cancel.is_cancelled() {
        return Some(RunError::Canceled);
    }
    if deadline.is_some_and(|d| Instant::now() >= d) {
        return Some(RunError::DeadlineExceeded);
    }
    None
}

async fn ctx_done(cancel: &CancellationToken, deadline: Option<Instant>) -> RunError {
    let timeout = async {
        match deadline {
            Some(d) => tokio::time::sleep_until(d).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        _ = cancel.cancelled() => RunError::Canceled,
        _ = timeout => RunError::DeadlineExceeded,
    }
}

/// Runs a one-shot shell command to completion in its own process group,
/// killing the whole group on cancel or deadline. A non-zero exit is an
/// error. Used for project setup_cmd (worktree create) and teardown_cmd
/// (archive/delete). `env` `None` inherits the host environment.
pub async fn run_cmd(
    cancel: &CancellationToken,
    deadline: Option<Instant>,
    shell_cmd: &str,
    work_dir: &str,
    env: Option<&[String]>,
) -> Result<(), RunError> {
    run_cmd_output(cancel, deadline, shell_cmd, work_dir, env)
        .await
        .1
}

/// [`run_cmd`] plus the last ~4 KiB of combined stdout/stderr. On cancel or
/// deadline it kills the group, then returns the tail with
/// [`RunError::Canceled`] / [`RunError::DeadlineExceeded`], not the wait
/// error.
pub async fn run_cmd_output(
    cancel: &CancellationToken,
    deadline: Option<Instant>,
    shell_cmd: &str,
    work_dir: &str,
    env: Option<&[String]>,
) -> (String, Result<(), RunError>) {
    if let Some(e) = ctx_err(cancel, deadline) {
        return (String::new(), Err(e));
    }
    let mut cmd = shell(shell_cmd, work_dir);
    if let Some(env) = env {
        apply_env(&mut cmd, env);
    }

    // One pipe for stdout and stderr, owned here, so the wait returns when
    // the shell exits (not when descendants close stdout), and draining can
    // continue instead of closing the read end on survivors.
    let (mut reader, writer) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return (String::new(), Err(RunError::Pipe(e))),
    };
    let writer2 = match writer.try_clone() {
        Ok(w) => w,
        Err(e) => return (String::new(), Err(RunError::Pipe(e))),
    };
    cmd.stdout(writer2).stderr(writer);
    let spawned = cmd.spawn();
    // Drop the parent's copies of the write end; children still hold theirs.
    drop(cmd);
    let child = match spawned {
        Ok(c) => c,
        Err(e) => return (String::new(), Err(RunError::Start(e))),
    };

    let tail = Arc::new(Mutex::new(TailWriter {
        buf: Vec::new(),
        max: CMD_OUTPUT_TAIL_CAP,
    }));
    let (copied_tx, copied_rx) = oneshot::channel::<()>();
    {
        let tail = Arc::clone(&tail);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => tail
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .write(&buf[..n]),
                }
            }
            let _ = copied_tx.send(());
        });
    }

    let group = Group::spawn(child);
    tokio::select! {
        r = group.wait() => {
            if let Some(e) = ctx_err(cancel, deadline) {
                return (tail_text(&tail), Err(e));
            }
            // simplify: the drain thread lives until descendants close
            // stdout (same lifetime as writing to /dev/null). Do not close
            // the pipe.
            let _ = tokio::time::timeout(CMD_OUTPUT_WAIT_DELAY, copied_rx).await;
            let res = match r {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => Err(RunError::Exit(s)),
                Err(e) => Err(RunError::Wait(e)),
            };
            (tail_text(&tail), res)
        }
        e = ctx_done(cancel, deadline) => {
            // Like exec.CommandContext: kill the shell at once, then tear the
            // rest of the group down.
            if !group.is_reaped() && group.pid > 0 {
                let _ = kill(Pid::from_raw(group.pid), Signal::SIGKILL);
            }
            group.kill().await;
            (tail_text(&tail), Err(e))
        }
    }
}

/// Asks the OS for an unused TCP port by binding 127.0.0.1:0 and releasing
/// it. The window before the dev server binds it is acceptable for local
/// workspace allocation: a lost race surfaces as a normal boot failure.
pub fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// Replaces `${RECUR_PORT}`, `$RECUR_PORT`, `${PORT}` and `$PORT` in s,
/// left to right without overlaps (Go's `strings.NewReplacer`).
pub fn substitute_port(s: &str, port: &str) -> String {
    const PATTERNS: [&str; 4] = ["${RECUR_PORT}", "$RECUR_PORT", "${PORT}", "$PORT"];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    'outer: while !rest.is_empty() {
        for p in PATTERNS {
            if let Some(r) = rest.strip_prefix(p) {
                out.push_str(port);
                rest = r;
                continue 'outer;
            }
        }
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            out.push(c);
        }
        rest = chars.as_str();
    }
    out
}

/// The probe client: a 2s timeout, redirects followed, and, like Go's
/// `ProxyFromEnvironment`, never a proxy for localhost or loopback hosts.
fn probe_client(url: &str) -> reqwest::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(PROBE_REQUEST_TIMEOUT);
    let loopback = reqwest::Url::parse(url).ok().is_some_and(|u| {
        let host = u.host_str().unwrap_or_default();
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if loopback {
        b = b.no_proxy();
    }
    b.build()
}

/// One GET; any HTTP response (even 4xx/5xx) means the server is up. Only
/// transport errors mean not ready yet.
async fn probe(client: &reqwest::Client, url: &str) -> Option<u16> {
    client
        .get(url)
        .send()
        .await
        .ok()
        .map(|r| r.status().as_u16())
}

/// Go's `ProcessState.String` for a finished process: `exit status N`,
/// or `signal: NAME` (plus ` (core dumped)`).
pub fn exit_status_string(s: ExitStatus) -> String {
    if let Some(code) = s.code() {
        return format!("exit status {code}");
    }
    match s.signal() {
        Some(sig) => {
            let mut out = format!("signal: {}", signal_name(sig));
            if s.core_dumped() {
                out.push_str(" (core dumped)");
            }
            out
        }
        None => s.to_string(),
    }
}

/// Go's `syscall.Signal.String` names.
fn signal_name(sig: i32) -> String {
    #[cfg(target_os = "macos")]
    const NAMES: [&str; 31] = [
        "hangup",
        "interrupt",
        "quit",
        "illegal instruction",
        "trace/BPT trap",
        "abort trap",
        "EMT trap",
        "floating point exception",
        "killed",
        "bus error",
        "segmentation fault",
        "bad system call",
        "broken pipe",
        "alarm clock",
        "terminated",
        "urgent I/O condition",
        "suspended (signal)",
        "suspended",
        "continued",
        "child exited",
        "stopped (tty input)",
        "stopped (tty output)",
        "I/O possible",
        "cputime limit exceeded",
        "filesize limit exceeded",
        "virtual timer expired",
        "profiling timer expired",
        "window size changes",
        "information request",
        "user defined signal 1",
        "user defined signal 2",
    ];
    #[cfg(not(target_os = "macos"))]
    const NAMES: [&str; 31] = [
        "hangup",
        "interrupt",
        "quit",
        "illegal instruction",
        "trace/breakpoint trap",
        "aborted",
        "bus error",
        "floating point exception",
        "killed",
        "user defined signal 1",
        "segmentation fault",
        "user defined signal 2",
        "broken pipe",
        "alarm clock",
        "terminated",
        "stack fault",
        "child exited",
        "continued",
        "stopped (signal)",
        "stopped",
        "stopped (tty input)",
        "stopped (tty output)",
        "urgent I/O condition",
        "CPU time limit exceeded",
        "file size limit exceeded",
        "virtual timer expired",
        "profiling timer expired",
        "window changed",
        "I/O possible",
        "power failure",
        "bad system call",
    ];
    usize::try_from(sig)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| NAMES.get(i))
        .map_or_else(|| format!("signal {sig}"), |n| (*n).to_owned())
}

/// Go's `time.Duration.String`: `1h2m3.5s`, `300ms`, `1.5µs`, `0s`.
pub fn go_duration(d: Duration) -> String {
    let u = d.as_nanos().min(u128::from(u64::MAX)) as u64;
    if u < 1_000_000_000 {
        let (unit, prec) = match u {
            0 => return "0s".to_owned(),
            1..=999 => ("ns", 0),
            1_000..=999_999 => ("µs", 3),
            _ => ("ms", 6),
        };
        let (frac, v) = fmt_frac(u, prec);
        return format!("{v}{frac}{unit}");
    }
    let (frac, mut v) = fmt_frac(u, 9);
    let mut out = format!("{}{frac}s", v % 60);
    v /= 60;
    if v > 0 {
        out = format!("{}m{out}", v % 60);
        v /= 60;
        if v > 0 {
            out = format!("{v}h{out}");
        }
    }
    out
}

/// The fraction of v/10^prec with trailing zeros (and an all-zero fraction)
/// dropped, and the integer part.
fn fmt_frac(mut v: u64, prec: u32) -> (String, u64) {
    let mut digits = Vec::new();
    let mut print = false;
    for _ in 0..prec {
        let digit = v % 10;
        print = print || digit != 0;
        if print {
            digits.push(char::from(b'0' + digit as u8));
        }
        v /= 10;
    }
    if !print {
        return (String::new(), v);
    }
    digits.push('.');
    (digits.into_iter().rev().collect(), v)
}

#[cfg(test)]
mod tests;
