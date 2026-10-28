use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::Instant;

use super::{DEFAULT_BASH_WAIT, MAX_BASH_LINES, MAX_BASH_WAIT, MAX_LINE_CHARS, clip_line};
use crate::gostr::lossy;
use crate::json::{Shape, Ty, field, unmarshal};
use crate::path::{abs, path_err};
use crate::tool::{Call, Tool, ToolError};

const SCHEMA: &str = r#"{"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"number"}},"required":["command"]}"#;

static IN: Shape = Shape::anon(&[field("command", Ty::Str), field("timeout", Ty::Float)]);

const MAX_CAPTURE: usize = (MAX_BASH_LINES + 2) * (MAX_LINE_CHARS + 8);
/// Go `exec.Cmd.WaitDelay`.
const WAIT_DELAY: Duration = Duration::from_secs(2);
const CLIPPED: &str = "<response clipped>";
const DEADLINE: &str = "context deadline exceeded";
const CANCELED: &str = "context canceled";

pub struct BashTool;

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Run a non-interactive bash command in the worktree root. Each call is a fresh shell: cd and env vars do not carry over, and nothing can prompt for input. Optional timeout is in seconds. Destructive commands require permission when Bypass Permissions is off."
    }

    fn schema(&self) -> String {
        SCHEMA.into()
    }

    async fn execute(&self, c: Call) -> Result<String, ToolError> {
        let a =
            unmarshal(&c.args, &IN).map_err(|e| ToolError(format!("invalid Bash args: {e}")))?;
        let command = a.str("command");
        if command.trim().is_empty() {
            return Err("command is required".into());
        }
        if !c.bypass_perms
            && let Some(p) = &c.permitter
        {
            let v = p
                .request_permission(&c.cancel, &c.workspace_id, &c.agent_code, "Bash", &c.args)
                .await?;
            if !v.allow {
                let reason = if v.reason.is_empty() {
                    "denied"
                } else {
                    v.reason.as_str()
                };
                return Err(format!("permission denied: {reason}").into());
            }
        }
        let wait = bash_wait(a.float("timeout"));
        let (out, ctx_err) = run(&c, command, wait).await;
        match ctx_err {
            Some(e) => Err(e.into()),
            None => Ok(out),
        }
    }
}

/// Go `time.Duration(timeout * float64(time.Second))`, capped at ten minutes.
fn bash_wait(timeout: f64) -> Duration {
    if timeout <= 0.0 {
        return DEFAULT_BASH_WAIT;
    }
    let nanos = timeout * 1e9;
    if nanos >= MAX_BASH_WAIT.as_nanos() as f64 {
        return MAX_BASH_WAIT;
    }
    Duration::from_nanos(nanos as u64)
}

/// Runs `bash -lc` in its own process group. Returns the framed output and,
/// when the command failed because the call's deadline or cancellation fired,
/// the context error text the tool reports instead.
async fn run(c: &Call, command: &str, wait: Duration) -> (String, Option<&'static str>) {
    // exec.CommandContext refuses to start once the context is done.
    let already = if c.cancel.is_cancelled() {
        Some(CANCELED)
    } else if wait.is_zero() {
        Some(DEADLINE)
    } else {
        None
    };
    if let Some(e) = already {
        return (truncate_cmd_output("", "", Some(e)), Some(e));
    }
    let bash = match look_path("bash") {
        Ok(p) => p,
        Err(e) => return (truncate_cmd_output("", "", Some(&e)), None),
    };
    let mut cmd = tokio::process::Command::new(&bash);
    cmd.arg("-lc")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(false);
    if !c.worktree.is_empty() {
        cmd.current_dir(&c.worktree);
        if let Ok(pwd) = abs(&c.worktree) {
            cmd.env("PWD", pwd);
        }
    }
    let mut child = match cmd.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            let err = path_err("fork/exec", &bash, &e);
            return (truncate_cmd_output("", "", Some(&err)), None);
        }
    };
    let pid = child.id().map(|p| Pid::from_raw(p as i32));
    let stdout = Arc::new(Mutex::new(CapBuffer::default()));
    let stderr = Arc::new(Mutex::new(CapBuffer::default()));
    let mut out_task = tokio::spawn(drain(child.stdout.take(), stdout.clone()));
    let mut err_task = tokio::spawn(drain(child.stderr.take(), stderr.clone()));

    let deadline = tokio::time::sleep(wait);
    tokio::pin!(deadline);
    let mut ctx_err: Option<&'static str> = None;
    let waited = tokio::select! {
        s = child.wait() => Some(s),
        _ = &mut deadline => {
            ctx_err = Some(DEADLINE);
            None
        }
        _ = c.cancel.cancelled() => {
            ctx_err = Some(CANCELED);
            None
        }
    };
    let mut delay_from: Option<Instant> = None;
    let status = match waited {
        Some(s) => s,
        None => {
            // Go's Cmd.Cancel: SIGKILL the whole group, then give it WaitDelay
            // to exit before killing the process itself.
            if let Some(pid) = pid {
                let _ = killpg(pid, Signal::SIGKILL);
            }
            delay_from = Some(Instant::now());
            match tokio::time::timeout(WAIT_DELAY, child.wait()).await {
                Ok(s) => s,
                Err(_) => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            }
        }
    };

    let mut run_err: Option<String> = match &status {
        Ok(s) if s.success() => ctx_err.map(String::from),
        Ok(s) => Some(exit_status_text(s)),
        Err(e) => Some(e.to_string()),
    };
    // Go WaitDelay: once the process is gone (or the context fired), wait at
    // most WAIT_DELAY for the pipes to drain, then close them.
    let pipes_by = delay_from.unwrap_or_else(Instant::now) + WAIT_DELAY;
    let drained = tokio::time::timeout_at(pipes_by, async {
        let _ = (&mut out_task).await;
        let _ = (&mut err_task).await;
    })
    .await
    .is_ok();
    if !drained {
        out_task.abort();
        err_task.abort();
        if run_err.is_none() {
            run_err = Some("exec: WaitDelay expired before I/O complete".into());
        }
    }
    let out = truncate_cmd_output(
        &stdout.lock().string(),
        &stderr.lock().string(),
        run_err.as_deref(),
    );
    match (&run_err, ctx_err) {
        (Some(_), Some(e)) => (out, Some(e)),
        _ => (out, None),
    }
}

async fn drain<R: AsyncRead + Unpin>(pipe: Option<R>, sink: Arc<Mutex<CapBuffer>>) {
    let Some(mut pipe) = pipe else { return };
    let mut chunk = [0u8; 32 * 1024];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => sink.lock().write(&chunk[..n]),
        }
    }
}

/// Go `exec.LookPath`.
fn look_path(file: &str) -> Result<String, String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = format!("{dir}/{file}");
        if is_executable(&candidate) {
            if !dir.starts_with('/') {
                return Err(format!(
                    "exec: \"{file}\": cannot run executable found relative to current directory"
                ));
            }
            return Ok(candidate);
        }
    }
    Err(format!(
        "exec: \"{file}\": executable file not found in $PATH"
    ))
}

fn is_executable(p: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0)
}

/// Go `os.ProcessState.String()`.
pub(crate) fn exit_status_text(s: &ExitStatus) -> String {
    let mut res = if let Some(code) = s.code() {
        format!("exit status {code}")
    } else if let Some(sig) = s.signal() {
        format!("signal: {}", signal_name(sig))
    } else if let Some(sig) = s.stopped_signal() {
        format!("stop signal: {}", signal_name(sig))
    } else if s.continued() {
        "continued".to_string()
    } else {
        String::new()
    };
    if s.core_dumped() {
        res.push_str(" (core dumped)");
    }
    res
}

/// Go `syscall.Signal.String()` for this platform.
fn signal_name(sig: i32) -> String {
    #[cfg(target_os = "macos")]
    const SIGNALS: [&str; 32] = [
        "",
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
    const SIGNALS: [&str; 32] = [
        "",
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
    match usize::try_from(sig).ok().and_then(|i| SIGNALS.get(i)) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => format!("signal {sig}"),
    }
}

/// Drains a pipe while bounding retained bytes so a runaway command cannot
/// fill RAM. Overflow is discarded; `clip_lines` still applies the line cap.
#[derive(Default)]
struct CapBuffer {
    buf: Vec<u8>,
    clipped: bool,
}

impl CapBuffer {
    fn write(&mut self, p: &[u8]) {
        if self.clipped {
            return;
        }
        let room = MAX_CAPTURE.saturating_sub(self.buf.len());
        if room == 0 {
            self.clipped = true;
            return;
        }
        if p.len() > room {
            self.buf.extend_from_slice(&p[..room]);
            self.clipped = true;
            return;
        }
        self.buf.extend_from_slice(p);
    }

    fn string(&self) -> String {
        let mut s = clip_lines(&lossy(&self.buf));
        if self.clipped && !s.contains(CLIPPED) {
            if !s.is_empty() {
                s.push('\n');
            }
            s.push_str(CLIPPED);
        }
        s
    }
}

fn clip_lines(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in s.split('\n') {
        lines.push(clip_line(line));
        if lines.len() >= MAX_BASH_LINES {
            lines.push(CLIPPED.into());
            break;
        }
    }
    lines.join("\n")
}

fn truncate_cmd_output(stdout: &str, stderr: &str, run_err: Option<&str>) -> String {
    let mut b = String::new();
    b.push_str(stdout);
    if !stderr.is_empty() {
        if !b.is_empty() {
            b.push('\n');
        }
        b.push_str("stderr:\n");
        b.push_str(stderr);
    }
    if let Some(e) = run_err {
        if !b.is_empty() {
            b.push('\n');
        }
        b.push_str("exit: ");
        b.push_str(e);
    }
    if b.is_empty() {
        return "(no output)".into();
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_matches_go_duration_math() {
        assert_eq!(bash_wait(0.0), DEFAULT_BASH_WAIT);
        assert_eq!(bash_wait(-1.0), DEFAULT_BASH_WAIT);
        assert_eq!(bash_wait(0.25), Duration::from_millis(250));
        assert_eq!(bash_wait(1e30), MAX_BASH_WAIT);
    }

    #[test]
    fn exit_text_matches_go() {
        assert_eq!(
            exit_status_text(&ExitStatus::from_raw(3 << 8)),
            "exit status 3"
        );
        assert_eq!(exit_status_text(&ExitStatus::from_raw(9)), "signal: killed");
        assert_eq!(signal_name(64), "signal 64");
    }
}
