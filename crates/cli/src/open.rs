//! `recur .` / `recur open` (`internal/cli/open.go`): hand a path to the
//! running app over the open-IPC socket, launching the app if needed.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use recur_domain::path;
use recur_platform::cliinstall::is_bundled_cli_path;
use recur_platform::identity::{APP_NAME, BUNDLE_ID};
use recur_platform::openipc::{self, IpcError, Request};

use crate::cobra::CliError;
use crate::paths;

const OPEN_RETRY_FOR: Duration = Duration::from_secs(30);
const OPEN_RETRY_WAIT: Duration = Duration::from_millis(250);

/// Why one open call failed.
#[derive(Debug)]
pub enum OpenError {
    /// No live listener (Go `openipc.DialError`).
    Dial(String),
    /// The app asked us to retry (Go `openipc.ErrUnavailable`).
    Unavailable,
    /// Anything else: reached the app and it failed, or timed out.
    Other(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Dial(m) | OpenError::Other(m) => f.write_str(m),
            OpenError::Unavailable => f.write_str("openipc: instance unavailable"),
        }
    }
}

impl OpenError {
    fn retryable(&self) -> bool {
        matches!(self, OpenError::Dial(_) | OpenError::Unavailable)
    }
}

/// `runOpen`: resolve the arg (exit 2 on failure), then send or launch
/// (exit 1 on failure). An empty arg focuses the app.
pub fn run_open(arg: &str) -> Result<(), CliError> {
    let abs = if arg.is_empty() {
        String::new()
    } else {
        resolve_open_arg(arg).map_err(|e| CliError::Exit(2, Some(e)))?
    };
    send_or_launch(&abs).map_err(|e| CliError::Exit(1, Some(e)))
}

pub fn resolve_open_arg(arg: &str) -> Result<String, String> {
    let abs = paths::canon_path(arg)?;
    std::fs::metadata(&abs).map_err(|e| format!("{arg}: {e}"))?;
    Ok(abs)
}

pub fn send_or_launch(abs: &str) -> Result<(), String> {
    let sock = paths::open_sock_path()?;
    let err = match call_open(&sock, abs) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    if !err.retryable() {
        return Err(err.to_string());
    }
    let launch_failed = |e: String| {
        format!(
            "Recur isn't running and could not be launched: {e}\nStart Recur (or `mise run dev`) and retry"
        )
    };
    let mut launched = false;
    if matches!(err, OpenError::Dial(_)) {
        launch_recur(abs).map_err(launch_failed)?;
        launched = true;
    }
    let deadline = Instant::now() + OPEN_RETRY_FOR;
    let mut last = None;
    while Instant::now() < deadline {
        match call_open(&sock, abs) {
            Ok(()) => return Ok(()),
            Err(e) if !e.retryable() => return Err(e.to_string()),
            Err(e) => {
                if !launched && matches!(e, OpenError::Dial(_)) {
                    launch_recur(abs).map_err(launch_failed)?;
                    launched = true;
                }
                last = Some(e);
            }
        }
        std::thread::sleep(OPEN_RETRY_WAIT);
    }
    let last = last.map_or_else(
        || "timed out waiting for Recur".to_string(),
        |e| e.to_string(),
    );
    Err(format!(
        "Recur launched but did not accept the path: {last}"
    ))
}

/// One request over the socket.
// Wiring point: the open-IPC client is recur-platform's `openipc::call`.
pub fn call_open(sock: &str, abs: &str) -> Result<(), OpenError> {
    let req = Request {
        path: abs.to_string(),
    };
    let res = openipc::call(Path::new(sock), &req).map_err(|e| ipc_error(sock, e))?;
    if openipc::retryable(&res) {
        return Err(OpenError::Unavailable);
    }
    if !res.ok {
        if res.error.is_empty() {
            return Err(OpenError::Other("recur: open failed".to_string()));
        }
        return Err(OpenError::Other(res.error));
    }
    Ok(())
}

/// Go's `net` error text for a failed call.
fn ipc_error(sock: &str, e: IpcError) -> OpenError {
    match e {
        IpcError::Dial(err) => OpenError::Dial(match err.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
                format!("dial unix {sock}: i/o timeout")
            }
            _ => format!("dial unix {sock}: connect: {}", err),
        }),
        IpcError::Io(err) => OpenError::Other(match err.kind() {
            io::ErrorKind::UnexpectedEof => "EOF".to_string(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
                format!("read unix ->{sock}: i/o timeout")
            }
            _ => err.to_string(),
        }),
        other => OpenError::Other(other.to_string()),
    }
}

/// Starts the desktop app with `--open <abs>`: `$RECUR_APP`, then the app
/// bundle via `open` (macOS), then a desktop binary next to this CLI or on
/// PATH.
pub fn launch_recur(abs: &str) -> Result<(), String> {
    let extra: Vec<&str> = if abs.is_empty() {
        Vec::new()
    } else {
        vec!["--open", abs]
    };
    if let Ok(app) = std::env::var("RECUR_APP")
        && !app.is_empty()
    {
        return start(&app, &extra);
    }
    if cfg!(target_os = "macos") {
        let mut by_id = vec!["-b", BUNDLE_ID];
        let mut by_name = vec!["-a", APP_NAME];
        if !extra.is_empty() {
            by_id.push("--args");
            by_id.extend(&extra);
            by_name.push("--args");
            by_name.extend(&extra);
        }
        for args in [by_id, by_name] {
            if Command::new("open")
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
            {
                return Ok(());
            }
        }
    }
    if let Ok(bin) = look_desktop_binary() {
        return start(&bin, &extra);
    }
    if cfg!(target_os = "macos") {
        return Err("Recur.app not found".to_string());
    }
    Err("Recur desktop binary not found (set RECUR_APP)".to_string())
}

/// Go `exec.Command(name, args...).Start()`.
fn start(name: &str, args: &[&str]) -> Result<(), String> {
    let path = if name.contains('/') {
        name.to_string()
    } else {
        look_path(name)
            .ok_or_else(|| format!("exec: {:?}: executable file not found in $PATH", name))?
    };
    Command::new(&path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{path}: {e}"))
}

/// Go `exec.LookPath`.
fn look_path(name: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| {
            if dir.as_os_str().is_empty() {
                Path::new(".").join(name)
            } else {
                dir.join(name)
            }
        })
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .map(|p| p.to_string_lossy().into_owned())
}

/// The desktop binary: `recur` next to this executable (a local build), or
/// `recur` on PATH, as long as it is not a CLI binary.
pub fn look_desktop_binary() -> Result<String, String> {
    if let Ok(exe) = std::env::current_exe() {
        let cand = path::join(&[&path::dir(&exe.to_string_lossy()), "recur"]);
        if std::fs::metadata(&cand).is_ok_and(|m| !m.is_dir()) && !is_cli_binary(&cand) {
            return Ok(cand);
        }
    }
    if let Some(p) = look_path("recur")
        && !is_cli_binary(&p)
    {
        return Ok(p);
    }
    Err("no desktop binary".to_string())
}

/// This process, or the app-shipped CLI that launch-time install puts on
/// PATH. Executing either as the desktop app would recurse.
pub fn is_cli_binary(p: &str) -> bool {
    if let Ok(me) = std::env::current_exe()
        && same_file(Path::new(p), &me)
    {
        return true;
    }
    let resolved = std::fs::canonicalize(p)
        .map_or_else(|_| p.to_string(), |r| r.to_string_lossy().into_owned());
    is_bundled_cli_path(Path::new(p)) || is_bundled_cli_path(Path::new(&resolved))
}

fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}
