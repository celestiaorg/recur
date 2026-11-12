//! One `claude -p` turn.
//!
//! The CLI runs in the worktree as the leader of its own session. A single
//! reaper task owns `wait()`; on cancel the whole session gets SIGTERM, then
//! SIGKILL after [`crate::proc::KILL_GRACE`], including tool processes that
//! detached into their own groups.
//!
//! With stream-json input (steerable turns, images, prompts too large for
//! argv) the prompt is the first stdin user message and steers follow as more
//! user messages, each flushed at the next `tool_result` so the cached
//! prompt prefix survives (D49). With `--input-format stream-json` claude
//! stays alive after `result` waiting for another message, so stdin MUST be
//! closed on the result or the turn deadlocks (stdout never closes ↔ stdin
//! never closes). Two exceptions: a plain steer still pending at the result
//! is flushed instead (claude answers it with another result), and live
//! background tasks hold the result, because claude kills them on stdin EOF;
//! it runs a follow-up turn on their completion and emits another result.
//! The hold lasts until the ceiling, restarted by each steer so a user still
//! talking to the agent keeps reaching it.

use std::sync::Arc;
use std::time::Duration;

use recur_agent::{
    AgentEvent, ContextUsageTracker, EventSink, ProviderError, Steer, SteerInbox, TurnContext,
    TurnRequest, TurnResult,
};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::args;
use crate::input::user_message_line;
use crate::proc::{self, kill_session, look_path, new_session, wait_reaped};
use crate::stream::parse_line;

/// Keeps the last 8 KiB of stderr: usage refusals, auth failures and outages
/// land there, and they separate transient failures from genuine ones (D56).
const STDERR_TAIL_MAX: usize = 8 * 1024;

/// The cap on one stream-json line.
const MAX_LINE: usize = 8 * 1024 * 1024;

pub(crate) async fn run(
    bin: &str,
    hold_ceiling: Duration,
    req: TurnRequest,
    cx: TurnContext,
) -> Result<TurnResult, ProviderError> {
    let bin = if bin.is_empty() { "claude" } else { bin };
    let TurnContext {
        cancel,
        sink,
        steer,
        ..
    } = cx;
    let use_stream = args::stream_input(&req, steer.is_some());
    let (argv, _spilled) = args::spill_append_prompt(args::build(&req, use_stream))
        .map_err(|e| ProviderError::failed(format!("claude system prompt file: {}", e)))?;

    let path = if bin.contains('/') {
        bin.into()
    } else {
        look_path(bin).map_err(|e| ProviderError::failed(format!("start claude: {e}")))?
    };
    let mut cmd = Command::new(&path);
    new_session(&mut cmd);
    cmd.args(&argv)
        .stdin(if use_stream {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if !req.worktree.as_os_str().is_empty() {
        if let Err(e) = std::fs::metadata(&req.worktree) {
            return Err(ProviderError::failed(format!(
                "start claude: chdir {}: {}",
                req.worktree.display(),
                e
            )));
        }
        cmd.current_dir(&req.worktree);
    }
    for key in &req.env_scrub {
        cmd.env_remove(key);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| ProviderError::failed(format!("start claude: {}: {e}", path.display())))?;
    let pid = child.id().unwrap_or_default();
    let mut stdin = child.stdin.take();
    let Some(stdout) = child.stdout.take() else {
        return Err(ProviderError::failed("claude stdout pipe: missing"));
    };
    let tail = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let stderr_task = child.stderr.take().map(|mut err| {
        let tail = Arc::clone(&tail);
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut t = tail.lock();
                t.extend_from_slice(&buf[..n]);
                if t.len() > STDERR_TAIL_MAX {
                    let cut = t.len() - STDERR_TAIL_MAX;
                    t.drain(..cut);
                }
            }
        })
    });
    let mut reaped = proc::reap(child, async move {
        if let Some(t) = stderr_task {
            let _ = t.await;
        }
    });
    let killer = {
        let mut reaped = reaped.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = wait_reaped(&mut reaped) => {}
                _ = cancel.cancelled() => kill_session(pid, &mut reaped).await,
            }
        })
    };

    if use_stream
        && let Some(w) = stdin.as_mut()
        && let Err(e) = write_line(w, &user_message_line(&req.prompt, &req.images)).await
    {
        drop(stdin);
        wait_reaped(&mut reaped).await;
        let _ = killer.await;
        return Err(ProviderError::failed(format!("claude seed prompt: {}", e)));
    }

    let (mut result, have_result) = if use_stream {
        let mut turn = Injecting {
            sink: sink.as_ref(),
            stdin,
            steer,
            pending: Vec::new(),
            seen_tool_result: false,
            bg_tasks: 0,
            held: None,
            hold_deadline: None,
            hold_armed: false,
            hold_ceiling,
            tracker: ContextUsageTracker::default(),
            result: TurnResult::default(),
            have_result: false,
        };
        turn.run(&cancel, stdout).await;
        (turn.result, turn.have_result)
    } else {
        run_stdout_only(stdout, sink.as_ref()).await
    };

    wait_reaped(&mut reaped).await;
    let _ = killer.await;
    let wait_err = reaped.borrow().clone().flatten();

    if cancel.is_cancelled() {
        return Err(ProviderError::canceled().with_partial(result));
    }
    result.stderr = String::from_utf8_lossy(&tail.lock()).into_owned();
    if !have_result {
        // No result line: the process error plus the stderr tail is the shape
        // a hard usage-limit refusal takes.
        let detail = result.stderr.trim();
        let msg = match (wait_err, detail.is_empty()) {
            (Some(w), false) => format!("claude exited without a result: {w}: {detail}"),
            (Some(w), true) => format!("claude exited without a result: {w}"),
            (None, false) => format!("claude produced no result line: {detail}"),
            (None, true) => "claude produced no result line".to_owned(),
        };
        return Err(ProviderError::failed(msg).with_partial(result));
    }
    Ok(result)
}

/// The one-shot path: parse stdout, no stdin.
async fn run_stdout_only(stdout: ChildStdout, sink: &dyn EventSink) -> (TurnResult, bool) {
    let mut result = TurnResult::default();
    let mut have_result = false;
    let mut tracker = ContextUsageTracker::default();
    let mut r = BufReader::new(stdout);
    loop {
        let line = match read_line(&mut r).await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                drain_after(&mut r, &e).await;
                break;
            }
        };
        let Ok(evs) = parse_line(&line) else { continue };
        for mut ev in evs {
            // Meter: the last assistant footprint over cumulative result usage.
            tracker.observe(&mut ev);
            if let AgentEvent::TurnEnd(res) = &ev {
                result = res.clone();
                have_result = true;
            }
            sink.emit(ev);
        }
    }
    (result, have_result)
}

/// A steer waiting for its stdin slot. Plain steers keep the steer to ack
/// after the write; conductor steers were acked on receipt so an unflushed
/// one can land on the loop's pending steer.
struct Pending {
    text: String,
    user: Option<AgentEvent>,
    plain: Option<Steer>,
}

struct Injecting<'a> {
    sink: &'a dyn EventSink,
    stdin: Option<ChildStdin>,
    steer: Option<SteerInbox>,
    pending: Vec<Pending>,
    seen_tool_result: bool,
    bg_tasks: usize,
    held: Option<AgentEvent>,
    hold_deadline: Option<Instant>,
    hold_armed: bool,
    hold_ceiling: Duration,
    tracker: ContextUsageTracker,
    result: TurnResult,
    have_result: bool,
}

impl Injecting<'_> {
    async fn run(&mut self, cancel: &CancellationToken, stdout: ChildStdout) {
        let (tx, mut lines) = mpsc::channel::<Vec<u8>>(16);
        let reader_cancel = cancel.clone();
        tokio::spawn(async move {
            let mut r = BufReader::new(stdout);
            loop {
                match read_line(&mut r).await {
                    Ok(Some(line)) => tokio::select! {
                        sent = tx.send(line) => if sent.is_err() { return },
                        _ = reader_cancel.cancelled() => return,
                    },
                    Ok(None) => return,
                    Err(e) => {
                        drop(tx);
                        drain_after(&mut r, &e).await;
                        return;
                    }
                }
            }
        });

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    self.reject_pending();
                    break;
                }
                _ = sleep_until(self.hold_deadline), if self.hold_deadline.is_some() => {
                    self.hold_deadline = None;
                    self.close_stdin();
                }
                s = next_steer(&mut self.steer) => match s {
                    None => self.steer = None,
                    Some(s) => self.on_steer(s).await,
                },
                line = lines.recv() => match line {
                    None => {
                        // Process exited: plain steers that never found a slot
                        // must nack so the engine sends them as a new turn.
                        self.reject_pending();
                        if let Some(held) = self.held.take() {
                            self.sink.emit(held);
                        }
                        break;
                    }
                    Some(line) => self.on_line(&line).await,
                },
            }
        }
        self.close_stdin();
    }

    async fn on_steer(&mut self, s: Steer) {
        if self.stdin.is_none() {
            s.ack(false);
            return;
        }
        if self.hold_deadline.is_some() {
            self.hold_deadline = Some(Instant::now() + self.hold_ceiling);
        }
        let text = s.stdin_text();
        let user = s.user_event(true);
        let plain = if s.is_plain() {
            Some(s)
        } else {
            s.ack(true);
            None
        };
        self.pending.push(Pending { text, user, plain });
        if self.seen_tool_result {
            self.flush_pending().await;
        }
    }

    async fn on_line(&mut self, line: &[u8]) {
        if let Some(n) = background_task_count(line) {
            self.bg_tasks = n;
            if n == 0 && self.held.is_some() {
                self.close_stdin();
            }
        }
        let Ok(evs) = parse_line(line) else { return };
        for mut ev in evs {
            if matches!(ev, AgentEvent::ToolResult(_)) {
                self.seen_tool_result = true;
                self.flush_pending().await;
            }
            self.tracker.observe(&mut ev);
            if let AgentEvent::TurnEnd(res) = &ev {
                if self.pending.iter().any(|p| p.plain.is_some()) {
                    self.flush_pending().await;
                    continue;
                }
                self.result = res.clone();
                self.have_result = true;
                if self.bg_tasks > 0 && self.stdin.is_some() {
                    self.held = Some(ev);
                    if !self.hold_armed {
                        self.hold_armed = true;
                        self.hold_deadline = Some(Instant::now() + self.hold_ceiling);
                    }
                    continue;
                }
                self.held = None;
                self.close_stdin();
            }
            self.sink.emit(ev);
        }
    }

    async fn flush_pending(&mut self) {
        for p in std::mem::take(&mut self.pending) {
            let mut ok = false;
            if !p.text.is_empty()
                && let Some(w) = self.stdin.as_mut()
                && write_line(w, &user_message_line(&p.text, &[]))
                    .await
                    .is_ok()
            {
                if let Some(user) = p.user {
                    self.sink.emit(user);
                }
                ok = true;
            }
            if let Some(s) = p.plain {
                s.ack(ok);
            }
        }
    }

    fn reject_pending(&mut self) {
        for p in std::mem::take(&mut self.pending) {
            if let Some(s) = p.plain {
                s.ack(false);
            }
        }
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }
}

async fn next_steer(steer: &mut Option<SteerInbox>) -> Option<Steer> {
    match steer {
        Some(inbox) => inbox.recv().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

async fn write_line(w: &mut ChildStdin, line: &str) -> std::io::Result<()> {
    w.write_all(line.as_bytes()).await?;
    w.flush().await
}

/// The still-running background tasks in a `system/background_tasks_changed`
/// frame (it carries the full list). Ambient entries (housekeeping,
/// auto-started watchers) are not activity. `None` for any other line,
/// including malformed JSON.
pub(crate) fn background_task_count(line: &[u8]) -> Option<usize> {
    const NEEDLE: &[u8] = b"\"background_tasks_changed\"";
    if !line.windows(NEEDLE.len()).any(|w| w == NEEDLE) {
        return None;
    }
    let v: Value = serde_json::from_slice(line).ok()?;
    let m = v.as_object()?;
    let str_field = |k: &str| match m.get(k) {
        None | Some(Value::Null) => Some(""),
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => None,
    };
    if str_field("type")? != "system" || str_field("subtype")? != "background_tasks_changed" {
        return None;
    }
    let tasks = match m.get("tasks") {
        None | Some(Value::Null) => return Some(0),
        Some(Value::Array(a)) => a,
        Some(_) => return None,
    };
    let mut n = 0;
    for t in tasks {
        let ambient = match t {
            Value::Null => false,
            Value::Object(o) => match o.get("ambient") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return None,
            },
            _ => return None,
        };
        if !ambient {
            n += 1;
        }
    }
    Some(n)
}

#[derive(Debug, thiserror::Error)]
#[error("bufio.Scanner: token too long")]
struct TooLong;

/// One line without its `\n` (and a trailing `\r`); `None` at EOF. A line of
/// [`MAX_LINE`] bytes or more is an error, which ends the stream.
async fn read_line<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return Ok((!line.is_empty()).then(|| trim_cr(line)));
        }
        let (take, done) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (i, true),
            None => (buf.len(), false),
        };
        line.extend_from_slice(&buf[..take]);
        r.consume(if done { take + 1 } else { take });
        if line.len() >= MAX_LINE {
            return Err(std::io::Error::other(TooLong));
        }
        if done {
            return Ok(Some(trim_cr(line)));
        }
    }
}

fn trim_cr(mut line: Vec<u8>) -> Vec<u8> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    line
}

/// After an oversized line a reader that stops leaves the CLI
/// blocked on a full pipe until the turn is cancelled. Parsing stops here
/// too, but the rest of stdout is drained so the process can still exit.
async fn drain_after<R: AsyncBufRead + Unpin>(r: &mut R, err: &std::io::Error) {
    if err.get_ref().is_some_and(|e| e.is::<TooLong>()) {
        let _ = tokio::io::copy(r, &mut tokio::io::sink()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_tasks() {
        let live = br#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash"}]}"#;
        assert_eq!(background_task_count(live), Some(1));
        let empty = br#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#;
        assert_eq!(background_task_count(empty), Some(0));
        let ambient = br#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"w1","ambient":true}]}"#;
        assert_eq!(background_task_count(ambient), Some(0));
        assert_eq!(
            background_task_count(br#"{"type":"system","subtype":"init"}"#),
            None
        );
        let bad = br#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"ambient":"yes"}]}"#;
        assert_eq!(background_task_count(bad), None);
        let other = br#"{"type":"user","subtype":"background_tasks_changed"}"#;
        assert_eq!(background_task_count(other), None);
    }

    #[tokio::test]
    async fn read_line_like_scanner() {
        let data: &[u8] = b"a\r\n\nb";
        let mut r = BufReader::new(data);
        assert_eq!(read_line(&mut r).await.unwrap(), Some(b"a".to_vec()));
        assert_eq!(read_line(&mut r).await.unwrap(), Some(Vec::new()));
        assert_eq!(read_line(&mut r).await.unwrap(), Some(b"b".to_vec()));
        assert_eq!(read_line(&mut r).await.unwrap(), None);
        let big = vec![b'x'; MAX_LINE];
        let mut r = BufReader::new(&big[..]);
        assert!(read_line(&mut r).await.is_err());
    }
}
