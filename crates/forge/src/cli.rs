//! Subprocess runner for the `gh` and `git` command lines.
//!
//! `Combined` interleaves stdout and stderr in one pipe, `Stdout` keeps
//! stdout only. Dropping a run cancels it: `gh` gets SIGKILL; `git` runs in
//! its own process group that gets SIGTERM, then SIGKILL after two seconds.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

/// How long a finished process's inherited pipes may stay open (an ssh
/// ControlMaster holding git's stderr) before the run returns what it read.
const PIPE_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    Combined,
    Stdout,
}

/// How a process ended. `Code(-1)` is a signal; `Spawn` never started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Exit {
    Success,
    Code(i32),
    Spawn(String),
}

pub(crate) struct Outcome {
    pub out: String,
    pub exit: Exit,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.exit == Exit::Success
    }

    /// The exit code, or -1 for a signal or when the process did not run.
    #[cfg(test)]
    pub fn code(&self) -> i32 {
        match &self.exit {
            Exit::Success => 0,
            Exit::Code(c) => *c,
            Exit::Spawn(_) => -1,
        }
    }
}

/// A failed `gh` or `git` run, shown as `<program> <args>: <trimmed output>`.
/// `out` is the untrimmed output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdError {
    pub program: &'static str,
    pub args: String,
    pub out: String,
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.program, self.args, self.out.trim())
    }
}

impl std::error::Error for CmdError {}

pub(crate) struct Spec<'a, S: AsRef<OsStr>> {
    pub program: &'a Path,
    pub dir: &'a Path,
    pub args: &'a [S],
    pub stdin: Option<&'a str>,
    pub capture: Capture,
    pub git: bool,
}

pub(crate) async fn run<S: AsRef<OsStr>>(spec: Spec<'_, S>) -> Outcome {
    match spawn_and_wait(&spec).await {
        Ok(o) => o,
        Err(e) => Outcome {
            out: String::new(),
            exit: Exit::Spawn(e.to_string()),
        },
    }
}

async fn spawn_and_wait<S: AsRef<OsStr>>(spec: &Spec<'_, S>) -> std::io::Result<Outcome> {
    let (mut reader, writer) = std::io::pipe()?;
    let mut cmd = tokio::process::Command::new(spec.program);
    cmd.args(spec.args);
    if !spec.dir.as_os_str().is_empty() {
        cmd.current_dir(spec.dir);
    }
    match spec.capture {
        Capture::Combined => {
            cmd.stdout(writer.try_clone()?);
            cmd.stderr(writer);
        }
        Capture::Stdout => {
            cmd.stdout(writer);
            cmd.stderr(Stdio::null());
        }
    }
    cmd.stdin(if spec.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    if spec.git {
        cmd.env("GIT_OPTIONAL_LOCKS", "0");
        #[cfg(unix)]
        cmd.process_group(0);
    } else {
        cmd.kill_on_drop(true);
    }
    let mut child = cmd.spawn()?;
    // The parent's copies of the pipe's write end must close so the reader
    // sees EOF when the child (and anything it spawned) exits.
    drop(cmd);
    let mut guard = GroupKill::arm(spec.git, child.id());

    let buf = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&buf);
    // A detached thread, not spawn_blocking: a grandchild holding the pipe
    // must not hold up runtime shutdown.
    let (done_tx, drained) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => sink.lock().extend_from_slice(&chunk[..n]),
            }
        }
        let _ = done_tx.send(());
    });
    if let (Some(input), Some(mut stdin)) = (spec.stdin, child.stdin.take()) {
        let input = input.to_owned();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(input.as_bytes()).await;
        });
    }
    let status = child.wait().await?;
    guard.disarm();
    let _ = tokio::time::timeout(PIPE_GRACE, drained).await;
    let out = String::from_utf8_lossy(&buf.lock()).into_owned();
    let exit = if status.success() {
        Exit::Success
    } else {
        Exit::Code(status.code().unwrap_or(-1))
    };
    Ok(Outcome { out, exit })
}

/// SIGTERMs a git process group when its run is dropped mid-flight, then
/// SIGKILLs the leader after the grace period.
struct GroupKill(Option<u32>);

impl GroupKill {
    fn arm(git: bool, pid: Option<u32>) -> Self {
        Self(if git { pid } else { None })
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        let Some(pid) = self.0.take() else {
            return;
        };
        std::thread::spawn(move || {
            let signal = |args: &[&str]| {
                let _ = std::process::Command::new("kill")
                    .args(args)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            };
            signal(&["-TERM", "--", &format!("-{pid}")]);
            std::thread::sleep(PIPE_GRACE);
            signal(&["-KILL", &pid.to_string()]);
        });
    }
}

/// A path with a separator must be an executable file;
/// a bare name is searched on `PATH`.
pub(crate) fn look_path(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return is_executable(program).then(|| program.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
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

/// The `gh` executable and its run helpers.
#[derive(Debug, Clone)]
pub struct Gh {
    program: PathBuf,
}

impl Default for Gh {
    fn default() -> Self {
        Self::new("gh")
    }
}

impl Gh {
    /// `program` is `gh` (resolved on `PATH`) or a path to a gh-compatible
    /// executable.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Whether `gh` resolves to an executable.
    pub fn installed(&self) -> bool {
        look_path(&self.program).is_some()
    }

    /// Combined output and the exit status.
    pub(crate) async fn output<S: AsRef<str>>(&self, dir: &Path, args: &[S]) -> Outcome {
        self.exec(dir, args, None, Capture::Combined).await
    }

    /// Combined output, or an error wrapping argv and output.
    pub async fn run<S: AsRef<str>>(&self, dir: &Path, args: &[S]) -> Result<String, CmdError> {
        let o = self.output(dir, args).await;
        wrap("gh", args, o)
    }

    /// [`Gh::run`] with `stdin` piped in.
    pub async fn run_stdin<S: AsRef<str>>(
        &self,
        dir: &Path,
        stdin: &str,
        args: &[S],
    ) -> Result<String, CmdError> {
        let o = self.exec(dir, args, Some(stdin), Capture::Combined).await;
        wrap("gh", args, o)
    }

    /// Stdout only.
    pub async fn stdout<S: AsRef<str>>(&self, dir: &Path, args: &[S]) -> Result<String, String> {
        let o = self.exec(dir, args, None, Capture::Stdout).await;
        match o.exit {
            Exit::Success => Ok(o.out),
            Exit::Code(c) => Err(format!("exit status {c}")),
            Exit::Spawn(e) => Err(e),
        }
    }

    async fn exec<S: AsRef<str>>(
        &self,
        dir: &Path,
        args: &[S],
        stdin: Option<&str>,
        capture: Capture,
    ) -> Outcome {
        let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
        run(Spec {
            program: &self.program,
            dir,
            args: &args,
            stdin,
            capture,
            git: false,
        })
        .await
    }
}

/// Runs git in its own process group: combined output, or an error wrapping
/// argv and output.
pub async fn git<S: AsRef<str>>(dir: &Path, args: &[S]) -> Result<String, CmdError> {
    let argv: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let o = run(Spec {
        program: Path::new("git"),
        dir,
        args: &argv,
        stdin: None,
        capture: Capture::Combined,
        git: true,
    })
    .await;
    wrap("git", args, o)
}

/// Stdout only, without [`git`]'s process group.
pub async fn plain_stdout(program: &str, args: &[&str]) -> Result<String, String> {
    let o = run(Spec {
        program: Path::new(program),
        dir: Path::new(""),
        args,
        stdin: None,
        capture: Capture::Stdout,
        git: false,
    })
    .await;
    match o.exit {
        Exit::Success => Ok(o.out),
        Exit::Code(c) => Err(format!("exit status {c}")),
        Exit::Spawn(e) => Err(e),
    }
}

fn wrap<S: AsRef<str>>(program: &'static str, args: &[S], o: Outcome) -> Result<String, CmdError> {
    if o.ok() {
        return Ok(o.out);
    }
    Err(CmdError {
        program,
        args: args.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" "),
        out: o.out,
    })
}

/// Writes an executable script (tests use it as a fake `gh`).
#[cfg(test)]
pub(crate) fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    recur_exec::write_executable(&path, body, 0o755).expect("write script");
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn combined_output_keeps_stderr_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let gh = Gh::new(write_script(
            dir.path(),
            "gh",
            "#!/bin/sh\necho out\necho err >&2\nexit 8\n",
        ));
        let o = gh.output(Path::new(""), &["pr", "checks"]).await;
        assert_eq!(o.code(), 8);
        assert!(
            o.out.contains("out") && o.out.contains("err"),
            "{:?}",
            o.out
        );
        let err = gh.run(Path::new(""), &["pr", "checks"]).await.unwrap_err();
        assert_eq!(err.to_string(), "gh pr checks: out\nerr");
    }

    #[tokio::test]
    async fn stdout_capture_drops_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let gh = Gh::new(write_script(
            dir.path(),
            "gh",
            "#!/bin/sh\necho token\necho noise >&2\n",
        ));
        assert_eq!(
            gh.stdout(Path::new(""), &["auth", "token"]).await.unwrap(),
            "token\n"
        );
    }

    #[tokio::test]
    async fn stdin_reaches_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let gh = Gh::new(write_script(dir.path(), "gh", "#!/bin/sh\ncat\n"));
        assert_eq!(
            gh.run_stdin(Path::new(""), "{\"q\":1}", &["api", "graphql"])
                .await
                .unwrap(),
            "{\"q\":1}"
        );
    }

    #[tokio::test]
    async fn missing_program_is_a_failed_run() {
        let gh = Gh::new("/nonexistent/gh");
        assert!(!gh.installed());
        let o = gh.output(Path::new(""), &["auth", "status"]).await;
        assert_eq!(o.code(), -1);
        assert_eq!(
            gh.run(Path::new(""), &["auth", "status"])
                .await
                .unwrap_err()
                .to_string(),
            "gh auth status: "
        );
    }

    #[tokio::test]
    async fn background_child_holding_the_pipe_does_not_hang() {
        let dir = tempfile::tempdir().unwrap();
        let gh = Gh::new(write_script(
            dir.path(),
            "gh",
            "#!/bin/sh\necho done\nsleep 30 &\nexit 0\n",
        ));
        let start = std::time::Instant::now();
        let out = gh.run(Path::new(""), &["x"]).await.unwrap();
        assert_eq!(out, "done\n");
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn look_path_finds_executables_only() {
        let dir = tempfile::tempdir().unwrap();
        let exe = write_script(dir.path(), "tool", "#!/bin/sh\n");
        assert_eq!(look_path(&exe), Some(exe.clone()));
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "x").unwrap();
        assert_eq!(look_path(&plain), None);
        assert_eq!(look_path(dir.path()), None);
    }
}
