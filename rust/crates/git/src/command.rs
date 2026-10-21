//! The git subprocess runner (Go `git.Command`).
//!
//! Every git process:
//! - runs with `GIT_OPTIONAL_LOCKS=0`, so a cancelled `git status` / `git diff`
//!   cannot leave `.git/index.lock` behind;
//! - leads its own process group, so cancellation can SIGTERM hook
//!   grandchildren (`git commit` → pre-commit → `git add`) and let them unlink
//!   the locks they hold. The leader is SIGKILLed if it outlives
//!   [`CANCEL_GRACE`];
//! - waits at most `wait_delay` for inherited stdout/stderr to close after a
//!   clean exit. ssh ControlMaster keeps a background ssh holding git's stderr,
//!   which must not turn a successful push or fetch into a failure.
//!
//! Cancellation is by dropping the future (or a [`Command::deadline`] for the
//! Go call sites that built their own `context.WithTimeout`).

use std::ffi::{OsStr, OsString};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use tokio::time::Instant;

use crate::error::{Error, ErrorKind, Result};

/// How long a cancelled git process has after SIGTERM before its group leader
/// is SIGKILLed. Also the default wait for inherited pipes after exit.
pub const CANCEL_GRACE: Duration = Duration::from_secs(2);

const OPTIONAL_LOCKS: &str = "GIT_OPTIONAL_LOCKS";

/// A git subprocess. Build it with [`command`], then run it with
/// [`output`](Self::output), [`combined_output`](Self::combined_output) or
/// [`run`](Self::run).
#[derive(Debug, Clone)]
pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    extra_env: Vec<(OsString, OsString)>,
    dir: Option<PathBuf>,
    stdin: Option<Vec<u8>>,
    deadline: Option<Instant>,
    wait_delay: Duration,
}

/// Returns a git subprocess for `args` (Go `git.Command`).
pub fn command<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new_program("git", args)
}

/// Output of a run that captured stdout and stderr separately.
#[derive(Debug)]
pub(crate) struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub result: Result<()>,
}

enum Pipes {
    /// stdout and stderr each captured.
    Separate,
    /// stdout and stderr share one pipe, interleaved as written.
    Combined,
    /// Both discarded.
    Null,
}

impl Command {
    pub(crate) fn new_program<I, S>(program: &str, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(|a| a.as_ref().to_owned()).collect(),
            extra_env: Vec::new(),
            dir: None,
            stdin: None,
            deadline: None,
            wait_delay: CANCEL_GRACE,
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    /// Adds an environment variable on top of the inherited environment.
    /// `GIT_OPTIONAL_LOCKS` cannot be overridden.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.extra_env
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Working directory (Go `cmd.Dir`). Empty means the current directory.
    pub fn current_dir(mut self, dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        self.dir = (!dir.as_os_str().is_empty()).then(|| dir.to_path_buf());
        self
    }

    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// Cancels the process at `deadline` the way a Go context deadline did:
    /// SIGTERM to the group, then SIGKILL to the leader after [`CANCEL_GRACE`].
    pub fn deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// [`deadline`](Self::deadline) `timeout` from now.
    pub fn timeout(self, timeout: Duration) -> Self {
        self.deadline(Instant::now() + timeout)
    }

    #[cfg(test)]
    pub(crate) fn wait_delay(mut self, d: Duration) -> Self {
        self.wait_delay = d;
        self
    }

    /// The environment the process runs with.
    pub(crate) fn environ(&self) -> Vec<(OsString, OsString)> {
        git_environ(std::env::vars_os(), &self.extra_env)
    }

    /// Runs git and returns stdout. A failure carries stderr
    /// ([`Error::stderr`]), like Go's `ExitError.Stderr`.
    pub async fn output(self) -> Result<Vec<u8>> {
        let ran = Box::pin(self.execute(Pipes::Separate)).await;
        match ran.result {
            Ok(()) => Ok(ran.stdout),
            Err(e) => Err(attach_stderr(e, ran.stderr)),
        }
    }

    /// Runs git and returns stdout and stderr interleaved, even on failure.
    pub async fn combined_output(self) -> (Vec<u8>, Result<()>) {
        let ran = Box::pin(self.execute(Pipes::Combined)).await;
        (ran.stdout, ran.result)
    }

    /// Runs git with stdout and stderr discarded.
    pub async fn run(self) -> Result<()> {
        Box::pin(self.execute(Pipes::Null)).await.result
    }

    /// Runs git capturing stdout and stderr separately.
    pub(crate) async fn capture(self) -> Captured {
        Box::pin(self.execute(Pipes::Separate)).await
    }

    /// Starts git with stdout piped and stderr discarded, for streaming reads.
    pub(crate) fn spawn_stdout(self) -> Result<Running> {
        let mut cmd = self.base_command();
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| self.spawn_error(e))?;
        let stdout = child.stdout.take();
        Ok(Running {
            guard: ChildGuard::new(child),
            stdout,
        })
    }

    fn base_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.args(&self.args)
            .env_clear()
            .envs(self.environ())
            .process_group(0);
        if let Some(dir) = &self.dir {
            cmd.current_dir(dir);
        }
        cmd
    }

    fn spawn_error(&self, e: std::io::Error) -> Error {
        let program = self.program.to_string_lossy();
        if e.kind() == std::io::ErrorKind::NotFound && self.dir.as_ref().is_none_or(|d| d.is_dir())
        {
            return Error::with_kind(
                format!("exec: \"{program}\": executable file not found in $PATH"),
                ErrorKind::NotFound,
            );
        }
        let message = match &self.dir {
            Some(dir) if !dir.is_dir() => {
                format!("chdir {}: {}", dir.display(), crate::text::errno_text(&e))
            }
            _ => format!("fork/exec {program}: {}", crate::text::errno_text(&e)),
        };
        Error::with_kind(message, ErrorKind::Io(e.kind()))
    }

    async fn execute(self, pipes: Pipes) -> Captured {
        let mut cmd = self.base_command();
        cmd.stdin(if self.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut combined_reader = None;
        match pipes {
            Pipes::Separate => {
                cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
            }
            Pipes::Null => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
            Pipes::Combined => match shared_pipe() {
                Ok((reader, out, err)) => {
                    cmd.stdout(out).stderr(err);
                    combined_reader = Some(reader);
                }
                Err(e) => return failed(e.into()),
            },
        }
        let spawned = cmd.spawn();
        // Drop our copies of the shared pipe's write end so EOF arrives.
        drop(cmd);
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => return failed(self.spawn_error(e)),
        };

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut guard = ChildGuard::new(child);

        let mut out = Vec::new();
        let mut err = Vec::new();
        let status = {
            let input = self.stdin.unwrap_or_default();
            let io = async {
                let feed = async {
                    if let Some(mut w) = stdin {
                        let _ = w.write_all(&input).await;
                        let _ = w.shutdown().await;
                    }
                };
                match combined_reader {
                    Some(r) => {
                        tokio::join!(feed, drain(Some(r), &mut out));
                    }
                    None => {
                        tokio::join!(feed, drain(stdout, &mut out), drain(stderr, &mut err));
                    }
                }
            };
            tokio::pin!(io);
            let mut io_done = false;
            let status = guard.wait_until(self.deadline, &mut io, &mut io_done).await;
            if !io_done {
                let _ = tokio::time::timeout(self.wait_delay, &mut io).await;
            }
            status
        };
        let result = match status {
            Ok(st) if st.success() => Ok(()),
            Ok(st) => Err(Error::with_kind(
                exit_text(st),
                ErrorKind::Exit {
                    status: st,
                    stderr: Vec::new(),
                },
            )),
            Err(e) => Err(e.into()),
        };
        Captured {
            stdout: out,
            stderr: err,
            result,
        }
    }
}

fn failed(e: Error) -> Captured {
    Captured {
        stdout: Vec::new(),
        stderr: Vec::new(),
        result: Err(e),
    }
}

fn attach_stderr(e: Error, stderr: Vec<u8>) -> Error {
    match e.kind() {
        ErrorKind::Exit { status, .. } => {
            let status = *status;
            Error::with_kind(e.message().to_string(), ErrorKind::Exit { status, stderr })
        }
        _ => e,
    }
}

/// One pipe whose write end serves as both stdout and stderr.
fn shared_pipe() -> std::io::Result<(tokio::net::unix::pipe::Receiver, Stdio, Stdio)> {
    let (reader, writer) = std::io::pipe()?;
    let writer2 = writer.try_clone()?;
    let reader = tokio::net::unix::pipe::Receiver::from_owned_fd(OwnedFd::from(reader))?;
    Ok((reader, Stdio::from(writer), Stdio::from(writer2)))
}

async fn drain<R: AsyncRead + Unpin>(r: Option<R>, buf: &mut Vec<u8>) {
    let Some(mut r) = r else { return };
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// A git process whose stdout is read incrementally.
pub(crate) struct Running {
    guard: ChildGuard,
    pub stdout: Option<tokio::process::ChildStdout>,
}

impl Running {
    #[cfg(test)]
    pub(crate) fn pid(&self) -> Option<u32> {
        self.guard.child.as_ref().and_then(Child::id)
    }

    /// Waits for exit after the caller is done with stdout.
    pub async fn wait(mut self) -> Result<()> {
        drop(self.stdout.take());
        let mut never = std::future::pending::<()>();
        let mut done = false;
        match self.guard.wait_until(None, &mut never, &mut done).await {
            Ok(st) if st.success() => Ok(()),
            Ok(st) => Err(Error::with_kind(
                exit_text(st),
                ErrorKind::Exit {
                    status: st,
                    stderr: Vec::new(),
                },
            )),
            Err(e) => Err(e.into()),
        }
    }

    /// SIGKILLs the leader and reaps it.
    pub async fn kill(mut self) {
        drop(self.stdout.take());
        if let Some(mut child) = self.guard.child.take() {
            let _ = child.kill().await;
        }
    }
}

/// Owns a running child. Dropping it before the child exits cancels the way
/// Go's `cmd.Cancel` + `WaitDelay` did.
struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// Waits for exit while driving `io`, terminating the group at `deadline`.
    async fn wait_until<F: std::future::Future<Output = ()> + Unpin>(
        &mut self,
        deadline: Option<Instant>,
        io: &mut F,
        io_done: &mut bool,
    ) -> std::io::Result<ExitStatus> {
        let Some(child) = self.child.as_mut() else {
            return Err(std::io::Error::other("process already reaped"));
        };
        let pid = child.id();
        let expire = async {
            match deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(expire);
        let mut expired = false;
        let mut kill_at: Option<Instant> = None;
        let status = loop {
            let grace = async {
                match kill_at {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                st = child.wait() => break st,
                () = &mut *io, if !*io_done => *io_done = true,
                () = &mut expire, if !expired => {
                    expired = true;
                    terminate_group(pid);
                    kill_at = Some(Instant::now() + CANCEL_GRACE);
                }
                () = grace => {
                    kill_at = None;
                    let _ = child.start_kill();
                }
            }
        };
        self.child = None;
        status
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        terminate_group(child.id());
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if tokio::time::timeout(CANCEL_GRACE, child.wait())
                        .await
                        .is_err()
                    {
                        let _ = child.kill().await;
                    }
                });
            }
            Err(_) => {
                let _ = child.start_kill();
            }
        }
    }
}

fn terminate_group(pid: Option<u32>) {
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGTERM);
    }
}

/// The process environment minus any `GIT_OPTIONAL_LOCKS`, then
/// `GIT_OPTIONAL_LOCKS=0`, then `extra` (minus `GIT_OPTIONAL_LOCKS`).
pub(crate) fn git_environ(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    extra: &[(OsString, OsString)],
) -> Vec<(OsString, OsString)> {
    let mut out: Vec<(OsString, OsString)> = base
        .into_iter()
        .filter(|(k, _)| k != OPTIONAL_LOCKS)
        .collect();
    out.push((OPTIONAL_LOCKS.into(), "0".into()));
    out.extend(extra.iter().filter(|(k, _)| k != OPTIONAL_LOCKS).cloned());
    out
}

/// Go `ProcessState.String()` for a failed exit.
pub(crate) fn exit_text(st: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = st.code() {
        return format!("exit status {code}");
    }
    match st.signal() {
        Some(sig) => {
            let name = signal_name(sig).map_or_else(|| format!("signal {sig}"), str::to_string);
            let core = if st.core_dumped() {
                " (core dumped)"
            } else {
                ""
            };
            format!("signal: {name}{core}")
        }
        None => "exit status -1".to_string(),
    }
}

/// Go's `syscall.Signal.String()` names.
fn signal_name(sig: i32) -> Option<&'static str> {
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
        .and_then(|s| s.checked_sub(1))
        .and_then(|i| NAMES.get(i))
        .copied()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant as StdInstant;

    use super::*;
    use crate::testutil::init_dirty_repo;

    #[tokio::test]
    async fn status_map_succeeds_with_existing_index_lock() {
        let repo = init_dirty_repo(0);
        std::fs::write(format!("{}/.git/index.lock", repo.path), b"").expect("lock");
        let st = crate::status::status_map(&repo.path)
            .await
            .unwrap_or_default();
        assert_eq!(
            st.get("a.go").map(String::as_str),
            Some("m"),
            "status with leftover index.lock = {st:?}, want a.go=m (git status took an optional lock)"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn status_map_does_not_take_index_lock() {
        // Enough untracked files that git status's optional index refresh is
        // observable: Recur used to SIGKILL that refresh and leave index.lock.
        let repo = init_dirty_repo(1500);
        let lock = format!("{}/.git/index.lock", repo.path);
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = {
            let (lock, hits, stop) = (lock.clone(), hits.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if std::fs::metadata(&lock).is_ok() {
                        hits.fetch_add(1, Ordering::Relaxed);
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
            })
        };
        let st = crate::status::status_map(&repo.path)
            .await
            .unwrap_or_default();
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.store(true, Ordering::Relaxed);
        let _ = watcher.join();
        assert_eq!(
            st.get("a.go").map(String::as_str),
            Some("m"),
            "status = {st:?}"
        );
        assert_eq!(
            hits.load(Ordering::Relaxed),
            0,
            "git status took index.lock; Recur status polls must set GIT_OPTIONAL_LOCKS=0"
        );
        assert!(std::fs::metadata(&lock).is_err(), "git status left {lock}");
    }

    #[test]
    fn command_disables_optional_locks() {
        let base = vec![
            ("PATH".into(), "/bin".into()),
            (OsString::from(OPTIONAL_LOCKS), OsString::from("1")),
        ];
        let extra = vec![
            (OsString::from(OPTIONAL_LOCKS), OsString::from("1")),
            ("X".into(), "y".into()),
        ];
        let env = git_environ(base, &extra);
        let locks: Vec<_> = env.iter().filter(|(k, _)| k == OPTIONAL_LOCKS).collect();
        assert_eq!(
            locks.len(),
            1,
            "GIT_OPTIONAL_LOCKS set more than once: {env:?}"
        );
        assert_eq!(locks[0].1, "0");
        assert!(env.iter().any(|(k, v)| k == "X" && v == "y"));
        assert!(
            command(["status"])
                .environ()
                .iter()
                .any(|(k, v)| k == OPTIONAL_LOCKS && v == "0")
        );
    }

    /// Cancelling (dropping) a run SIGTERMs git's whole process group, so a
    /// hook grandchild gets the chance to clean up its locks.
    #[tokio::test]
    async fn cancel_sigterms_the_process_group() {
        let repo = init_dirty_repo(0);
        let marker = format!("{}/terminated", repo.path);
        let script =
            format!(r#"!sh -c 'trap "echo term > {marker}; exit 0" TERM; sleep 30 & wait'"#);
        let alias = format!("alias.hang={script}");
        let run = command(["-C", repo.path.as_str(), "-c", alias.as_str(), "hang"]).output();
        let _ = tokio::time::timeout(Duration::from_millis(500), run).await;
        let deadline = StdInstant::now() + Duration::from_secs(5);
        while std::fs::metadata(&marker).is_err() && StdInstant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            std::fs::metadata(&marker).is_ok(),
            "the hook grandchild never saw SIGTERM"
        );
    }

    #[tokio::test]
    async fn deadline_terminates_like_a_context_timeout() {
        let started = StdInstant::now();
        let res = Command::new_program("sh", ["-c", "sleep 30"])
            .timeout(Duration::from_millis(100))
            .output()
            .await;
        let err = res.expect_err("timed-out command succeeded");
        assert_eq!(err.message(), "signal: terminated");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "deadline did not stop the process"
        );
    }

    #[tokio::test]
    async fn starts_in_own_process_group() {
        let repo = init_dirty_repo(0);
        let running = command(["-C", repo.path.as_str(), "status", "--porcelain"])
            .spawn_stdout()
            .expect("spawn");
        let pid = running.pid().expect("pid") as i32;
        let pgid = nix::unistd::getpgid(Some(Pid::from_raw(pid))).expect("getpgid");
        let _ = running.wait().await;
        assert_eq!(
            pgid.as_raw(),
            pid,
            "git must lead its own process group so cancel can signal hooks"
        );
    }

    #[tokio::test]
    async fn pipe_hold_after_clean_exit_is_success() {
        let started = StdInstant::now();
        let out = Command::new_program("sh", ["-c", "sleep 1 & echo done"])
            .wait_delay(Duration::from_millis(50))
            .output()
            .await
            .expect("pipe hold after exit 0 must succeed");
        assert_eq!(String::from_utf8_lossy(&out).trim(), "done");
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "waited for the pipe holder"
        );
    }

    #[tokio::test]
    async fn non_zero_exit_is_kept() {
        let err = Command::new_program("sh", ["-c", "echo fail >&2; exit 1"])
            .output()
            .await
            .expect_err("exit 1 swallowed");
        assert_eq!(err.message(), "exit status 1");
        assert_eq!(String::from_utf8_lossy(err.stderr()).trim(), "fail");
    }

    #[tokio::test]
    async fn pipe_hold_after_failure_is_kept() {
        let err = Command::new_program("sh", ["-c", "sleep 1 & exit 3"])
            .wait_delay(Duration::from_millis(50))
            .output()
            .await
            .expect_err("a failing exit holding pipes must stay an error");
        assert_eq!(err.message(), "exit status 3");
    }

    fn pipe_hold_git(root: &str) -> Command {
        command(["-C", root, "-c", "alias.hold=!sleep 1 & echo done", "hold"])
            .wait_delay(Duration::from_millis(50))
    }

    #[tokio::test]
    async fn every_runner_absorbs_pipe_hold() {
        let repo = init_dirty_repo(0);
        let out = pipe_hold_git(&repo.path).output().await.expect("output");
        assert_eq!(String::from_utf8_lossy(&out).trim(), "done");
        let (out, res) = pipe_hold_git(&repo.path).combined_output().await;
        res.expect("combined_output");
        assert_eq!(String::from_utf8_lossy(&out).trim(), "done");
        let ran = pipe_hold_git(&repo.path).capture().await;
        ran.result.expect("capture");
        assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "done");
        pipe_hold_git(&repo.path).run().await.expect("run");
    }

    #[tokio::test]
    async fn check_ref_format_via_command() {
        crate::worktree::check_ref_format("recur/ok")
            .await
            .expect("recur/ok is valid");
        assert!(
            crate::worktree::check_ref_format("has space")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn missing_binary_reads_like_go() {
        let err = Command::new_program("recur-no-such-binary", ["x"])
            .output()
            .await
            .expect_err("spawned");
        assert!(matches!(err.kind(), ErrorKind::NotFound));
        assert_eq!(
            err.message(),
            "exec: \"recur-no-such-binary\": executable file not found in $PATH"
        );
    }
}
