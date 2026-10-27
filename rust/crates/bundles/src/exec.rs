//! Shebang execution for commands, prompt actions, complete scripts,
//! palette search and New Task expanders.

use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use recur_domain::workspace::IssueRef;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::env::{EnvVar, PromptRunContext, RunContext, environ, prompt_environ, resolve_input};
use crate::go::{self, quote};
use crate::grammar::{go_json_field, go_json_kind};
use crate::manifest::{
    Command, EXPAND_TIMEOUT, ExpandAction, INTERPRETER_NATIVE, INTERPRETER_SHEBANG,
    PROMPT_COMPLETE_MAX_HITS, PROMPT_COMPLETE_TIMEOUT, PROMPT_TIMEOUT, PromptAction, SearchAction,
};
use crate::{Error, err};

/// Per-command wall-clock limit when the caller sets no deadline.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const KILL_GRACE: Duration = Duration::from_millis(500);

/// The part of Go's `context.Context` the runners use: an optional
/// deadline plus cancellation.
#[derive(Debug, Clone, Default)]
pub struct RunCtx {
    deadline: Option<Instant>,
    cancel: CancellationToken,
}

impl RunCtx {
    /// No deadline, never cancelled unless [`RunCtx::cancel`] is called.
    pub fn background() -> Self {
        Self::default()
    }

    /// A context cancelled through `token`.
    pub fn with_cancel(token: CancellationToken) -> Self {
        RunCtx {
            deadline: None,
            cancel: token,
        }
    }

    /// A child with a deadline `d` from now (or the parent's, if sooner).
    pub fn with_timeout(&self, d: Duration) -> Self {
        let at = Instant::now() + d;
        RunCtx {
            deadline: Some(self.deadline.map_or(at, |p| p.min(at))),
            cancel: self.cancel.clone(),
        }
    }

    /// Whether a deadline is set.
    pub fn has_deadline(&self) -> bool {
        self.deadline.is_some()
    }

    /// Cancels this context and every child sharing its token.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub(crate) fn or_timeout(&self, d: Duration) -> Self {
        if self.has_deadline() {
            self.clone()
        } else {
            self.with_timeout(d)
        }
    }

    /// Resolves when the context ends, with Go's `ctx.Err()` text.
    pub(crate) async fn done(&self) -> &'static str {
        let deadline = async {
            match self.deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = deadline => "context deadline exceeded",
            _ = self.cancel.cancelled() => "context canceled",
        }
    }

    /// Go `ctx.Err()`: the end reason if the context already ended.
    pub(crate) fn err(&self) -> Option<&'static str> {
        if self.cancel.is_cancelled() {
            return Some("context canceled");
        }
        match self.deadline {
            Some(at) if Instant::now() >= at => Some("context deadline exceeded"),
            _ => None,
        }
    }
}

/// Captured outcome of a shebang command (or a TypeScript macro under
/// `recur bundles test`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RunResult {
    pub stdout: String,
    pub stderr: String,
    #[serde(rename = "exitCode")]
    pub exit_code: i64,
    /// Resolved routing from the command.
    pub output: String,
    /// Effective values: the MacroResult override when set, otherwise the
    /// manifest (same resolution as the app and bundle tests).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(rename = "openInPreview", skip_serializing_if = "Option::is_none")]
    pub open_in_preview: Option<bool>,
}

/// A failed [`run`]. On timeout `partial` holds the output captured so far
/// (Go returns both the result and the error).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RunError {
    pub message: String,
    pub partial: RunResult,
}

impl From<Error> for RunError {
    fn from(e: Error) -> Self {
        RunError {
            message: e.0,
            partial: RunResult::default(),
        }
    }
}

/// Executes a shebang command. TypeScript commands run in the frontend
/// Worker (or under node in `recur bundles test`), never here.
pub async fn run(ctx: &RunCtx, cmd: &Command, mut rc: RunContext) -> Result<RunResult, RunError> {
    if cmd.interpreter == INTERPRETER_NATIVE || crate::is_script_interpreter(&cmd.interpreter) {
        return Err(err!(
            "{} command {} must run in the frontend",
            cmd.interpreter,
            quote(&cmd.id)
        )
        .into());
    }
    let ctx = ctx.or_timeout(DEFAULT_TIMEOUT);
    let input = resolve_input(cmd, &rc);
    rc.input = input.clone();
    if rc.bundle_support.is_empty() {
        rc.bundle_support = cmd.bundle_dir.clone();
    }
    // Always run from a temp copy so the author's script never gets chmod'ed
    // (which would dirty git mode bits for shipped packs).
    let body = shebang_body(&cmd.body, &cmd.script_path, "command", &cmd.id)?;
    let script = TempScript::write("recur-cmd-", &body)?;
    let work_dir = first_nonempty(&[&rc.work_dir, &rc.project_directory])
        .unwrap_or_else(|| go::dir(&script.path));
    let out = execute(
        &ctx,
        &script.path,
        &work_dir,
        &environ(&rc),
        Some(input.into_bytes()),
    )
    .await?;
    let mut res = run_result_from_cmd(cmd, &out.stdout, &out.stderr, -1);
    match out.end {
        End::Cancelled(why) => Err(RunError {
            message: format!("command cancelled or timed out: {why}"),
            partial: res,
        }),
        End::Exited(status) => {
            res.exit_code = exit_code(status);
            Ok(res)
        }
    }
}

fn run_result_from_cmd(cmd: &Command, stdout: &[u8], stderr: &[u8], exit_code: i64) -> RunResult {
    RunResult {
        stdout: String::from_utf8_lossy(stdout).into_owned(),
        stderr: String::from_utf8_lossy(stderr).into_owned(),
        exit_code,
        output: cmd.output.clone(),
        language: cmd.language.clone(),
        open_in_preview: Some(cmd.open_in_preview),
    }
}

fn first_nonempty(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|s| !s.is_empty())
        .map(|s| s.to_string())
}

pub(crate) fn has_shebang(body: &str) -> bool {
    body.starts_with("#!")
}

/// The body to execute: a pointed-to script must carry its own `#!`; inline
/// bodies without one run under bash.
fn shebang_body(body: &str, script_path: &str, kind: &str, id: &str) -> Result<String, Error> {
    if body.is_empty() {
        return Err(err!("{kind} {} has empty body", quote(id)));
    }
    if !script_path.is_empty() && !has_shebang(body) {
        return Err(err!("{kind} {} script missing #! shebang", quote(id)));
    }
    if has_shebang(body) {
        Ok(body.to_string())
    } else {
        Ok(format!("#!/usr/bin/env bash\n{body}"))
    }
}

/// An executable temp copy of a script, removed on drop.
struct TempScript {
    path: String,
}

impl TempScript {
    /// The script is executed right after, so it is written by `recur_exec` (never open for
    /// writing in this process, where a concurrent spawn would inherit the descriptor and make
    /// the exec fail with "text file busy").
    fn write(prefix: &str, body: &str) -> Result<Self, Error> {
        let file = tempfile::Builder::new()
            .prefix(prefix)
            .make(|p| recur_exec::create_executable(p, body, 0o700))
            .map_err(|e| err!("temp script: {}", go::errno_text(&e)))?;
        let path = file
            .into_temp_path()
            .keep()
            .map_err(|e| err!("temp script: {}", go::errno_text(&e.error)))?;
        Ok(TempScript {
            path: go::path_str(&path),
        })
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

enum End {
    Exited(ExitStatus),
    Cancelled(&'static str),
}

struct Outcome {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    end: End,
}

fn exit_code(status: ExitStatus) -> i64 {
    status.code().map_or(-1, i64::from)
}

/// Runs `script` in its own process group. When `ctx` ends first the group
/// gets SIGTERM, then SIGKILL after a short grace.
async fn execute(
    ctx: &RunCtx,
    script: &str,
    work_dir: &str,
    env: &[EnvVar],
    stdin: Option<Vec<u8>>,
) -> Result<Outcome, Error> {
    let mut command = tokio::process::Command::new(script);
    command
        .current_dir(work_dir)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = spawn_retrying(&mut command)
        .map_err(|e| err!("{}", go::path_err("fork/exec", script, &e)))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = pipe.write_all(&input).await;
        });
    }
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let mut out_task = drain(child.stdout.take(), stdout.clone());
    let mut err_task = drain(child.stderr.take(), stderr.clone());
    let pid = child.id();

    let finished = async {
        let status = child.wait().await;
        let _ = (&mut out_task).await;
        let _ = (&mut err_task).await;
        status
    };
    let end = tokio::select! {
        status = finished => End::Exited(status.map_err(|e| err!("{}", go::errno_text(&e)))?),
        why = ctx.done() => {
            kill_group(pid, &mut child).await;
            // Like Go's Wait, collect what the group wrote before it died.
            let _ = tokio::time::timeout(KILL_GRACE, async {
                let _ = (&mut out_task).await;
                let _ = (&mut err_task).await;
            })
            .await;
            End::Cancelled(why)
        }
    };
    let take = |buf: &Arc<Mutex<Vec<u8>>>| buf.lock().map(|b| b.clone()).unwrap_or_default();
    Ok(Outcome {
        stdout: take(&stdout),
        stderr: take(&stderr),
        end,
    })
}

/// Spawns, retrying briefly on ETXTBSY: a concurrent fork can still hold
/// the just-written script open for writing (Go avoids this with
/// `syscall.ForkLock`, which Rust does not expose).
fn spawn_retrying(command: &mut tokio::process::Command) -> io::Result<tokio::process::Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
            other => return other,
        }
    }
}

pub(crate) fn drain<R>(pipe: Option<R>, buf: Arc<Mutex<Vec<u8>>>) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let Some(mut pipe) = pipe else { return };
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut b) = buf.lock() {
                        b.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
    })
}

async fn kill_group(pid: Option<u32>, child: &mut tokio::process::Child) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;
    let Some(pid) = pid else { return };
    let pgid = Pid::from_raw(pid as i32);
    let _ = killpg(pgid, Signal::SIGTERM);
    if tokio::time::timeout(KILL_GRACE, child.wait())
        .await
        .is_err()
    {
        let _ = killpg(pgid, Signal::SIGKILL);
        let _ = child.wait().await;
    }
}

/// Validated outcome of a `[[prompt]]` action.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PromptResult {
    pub prompt: String,
    pub display: String,
    #[serde(skip_serializing_if = "crate::manifest::is_false")]
    pub skip: bool,
    /// Links the workspace the prompt ran in to this issue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueRef>,
}

/// Runs a shebang prompt action and parses stdout JSON
/// `{"prompt":"...","display":"..."}`. TypeScript prompts go through
/// [`crate::compile_prompt_macro`] and the Worker (or [`crate::run_prompt`]).
pub async fn run_prompt_shebang(
    ctx: &RunCtx,
    prompt: &PromptAction,
    mut rc: PromptRunContext,
) -> Result<PromptResult, Error> {
    let id = quote(&prompt.id);
    if crate::is_script_interpreter(&prompt.interpreter) {
        return Err(err!(
            "prompt {id} is {} — use CompiledPromptModule",
            prompt.interpreter
        ));
    }
    if prompt.interpreter != INTERPRETER_SHEBANG && !prompt.interpreter.is_empty() {
        return Err(err!(
            "prompt {id}: unsupported interpreter {}",
            quote(&prompt.interpreter)
        ));
    }
    let timeout = if prompt.timeout_ms > 0 {
        Duration::from_millis(prompt.timeout_ms as u64)
    } else {
        PROMPT_TIMEOUT
    };
    let ctx = ctx.or_timeout(timeout);
    if rc.bundle_support.is_empty() {
        rc.bundle_support = prompt.bundle_dir.clone();
    }
    if rc.prompt_slash.is_empty() {
        rc.prompt_slash = prompt.slash.clone();
    }
    if prompt.slash == "open-draft-pr" {
        rc.pr_draft = true;
    }
    let body = shebang_body(&prompt.body, &prompt.script_path, "prompt", &prompt.id)?;
    let script = TempScript::write("recur-prompt-", &body)?;
    let work_dir =
        first_nonempty(&[&rc.work_dir, &rc.worktree]).unwrap_or_else(|| go::dir(&script.path));
    let out = execute(&ctx, &script.path, &work_dir, &prompt_environ(&rc), None).await?;
    match out.end {
        End::Cancelled(why) => Err(err!("prompt cancelled or timed out: {why}")),
        End::Exited(status) if !status.success() => Err(err!(
            "prompt {id} exited {}: {}",
            exit_code(status),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        End::Exited(_) => {
            parse_prompt_result_json(&String::from_utf8_lossy(&out.stdout), &prompt.name)
        }
    }
}

/// Decodes a JSON object field as Go would into a `string`.
fn json_string(obj: &Map<String, Value>, name: &str) -> Result<String, String> {
    match go_json_field(obj, name) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!(
            "json: cannot unmarshal {} into Go struct field .{name} of type string",
            go_json_kind(other)
        )),
    }
}

fn json_object<'a>(v: &'a Value, what: &str) -> Result<Option<&'a Map<String, Value>>, String> {
    match v {
        Value::Null => Ok(None),
        Value::Object(o) => Ok(Some(o)),
        other => Err(format!(
            "json: cannot unmarshal {} into Go value of type {what}",
            go_json_kind(other)
        )),
    }
}

pub(crate) fn parse_prompt_result_json(
    stdout: &str,
    fallback_name: &str,
) -> Result<PromptResult, Error> {
    let raw = stdout.trim();
    if raw.is_empty() {
        return Err(err!(
            "prompt stdout empty (want JSON {{\"prompt\",\"display\"}})"
        ));
    }
    let bad = |e: String| err!("prompt stdout is not valid JSON: {e}");
    let value: Value = serde_json::from_str(raw).map_err(|e| bad(e.to_string()))?;
    let mut res = PromptResult::default();
    if let Some(obj) = json_object(&value, "struct").map_err(bad)? {
        res.prompt = json_string(obj, "prompt").map_err(bad)?;
        res.display = json_string(obj, "display").map_err(bad)?;
        res.skip = match go_json_field(obj, "skip") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(other) => {
                return Err(bad(format!(
                    "json: cannot unmarshal {} into Go struct field .skip of type bool",
                    go_json_kind(other)
                )));
            }
        };
        if let Some(issue) = go_json_field(obj, "issue")
            && let Some(o) = json_object(issue, "domain.IssueRef").map_err(bad)?
        {
            res.issue = Some(IssueRef {
                source: json_string(o, "source").map_err(bad)?,
                id: json_string(o, "id").map_err(bad)?,
                key: json_string(o, "key").map_err(bad)?,
            });
        }
    }
    // An empty prompt is a no-send success (e.g. /update finished in git
    // with no LLM turn); display still falls back to the action name.
    Ok(PromptResult {
        prompt: res.prompt.trim().to_string(),
        display: crate::normalize_prompt_display(&res.display, fallback_name),
        skip: res.skip,
        issue: res.issue,
    })
}

/// One trailing-arg suggestion from a complete script.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PromptCompleteHit {
    pub insert: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub psub: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub badge: String,
    /// Host-handled, e.g. `settings:integrations`. Empty = insert/run.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub action: String,
}

/// Stdout JSON from a complete script.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PromptCompleteResult {
    pub hits: Vec<PromptCompleteHit>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

/// Runs `prompt.complete_body` as a shebang and parses
/// `{"hits":[...],"message"?}`. TypeScript complete scripts compile with
/// [`crate::compile_complete_macro`].
pub async fn run_prompt_complete_shebang(
    ctx: &RunCtx,
    prompt: &PromptAction,
    mut rc: PromptRunContext,
) -> Result<PromptCompleteResult, Error> {
    let id = quote(&prompt.id);
    if prompt.complete_interpreter != INTERPRETER_SHEBANG && !prompt.complete_interpreter.is_empty()
    {
        return Err(err!(
            "prompt {id} complete is {} — use CompiledCompleteModule",
            prompt.complete_interpreter
        ));
    }
    if prompt.complete_body.trim().is_empty() {
        return Err(err!("prompt {id} has no complete script"));
    }
    let ctx = ctx.or_timeout(PROMPT_COMPLETE_TIMEOUT);
    if rc.bundle_support.is_empty() {
        rc.bundle_support = prompt.bundle_dir.clone();
    }
    if rc.prompt_slash.is_empty() {
        rc.prompt_slash = prompt.slash.clone();
    }
    let mut body = prompt.complete_body.clone();
    if !prompt.complete_path.is_empty() && !has_shebang(&body) {
        return Err(err!("prompt {id} complete script missing #! shebang"));
    }
    if !has_shebang(&body) {
        body = format!("#!/usr/bin/env bash\n{body}");
    }
    let script = TempScript::write("recur-prompt-complete-", &body)?;
    let work_dir =
        first_nonempty(&[&rc.work_dir, &rc.worktree]).unwrap_or_else(|| go::dir(&script.path));
    let out = execute(&ctx, &script.path, &work_dir, &prompt_environ(&rc), None).await?;
    match out.end {
        End::Cancelled(why) => Err(err!("complete cancelled or timed out: {why}")),
        End::Exited(status) if !status.success() => Err(err!(
            "prompt {id} complete exited {}: {}",
            exit_code(status),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        End::Exited(_) => parse_prompt_complete_json(&String::from_utf8_lossy(&out.stdout)),
    }
}

pub(crate) fn parse_prompt_complete_json(stdout: &str) -> Result<PromptCompleteResult, Error> {
    let raw = stdout.trim();
    if raw.is_empty() {
        return Ok(PromptCompleteResult::default());
    }
    let bad = |e: String| err!("complete stdout is not valid JSON: {e}");
    let value: Value = serde_json::from_str(raw).map_err(|e| bad(e.to_string()))?;
    let Some(obj) = json_object(&value, "bundles.PromptCompleteResult").map_err(bad)? else {
        return Ok(PromptCompleteResult::default());
    };
    let mut hits_raw = Vec::new();
    match go_json_field(obj, "hits") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for item in items {
                let mut h = PromptCompleteHit::default();
                if let Some(o) = json_object(item, "bundles.PromptCompleteHit").map_err(bad)? {
                    h.insert = json_string(o, "insert").map_err(bad)?;
                    h.label = json_string(o, "label").map_err(bad)?;
                    h.psub = json_string(o, "psub").map_err(bad)?;
                    h.detail = json_string(o, "detail").map_err(bad)?;
                    h.badge = json_string(o, "badge").map_err(bad)?;
                    h.action = json_string(o, "action").map_err(bad)?;
                }
                hits_raw.push(h);
            }
        }
        Some(other) => {
            return Err(bad(format!(
                "json: cannot unmarshal {} into Go struct field PromptCompleteResult.hits of type []bundles.PromptCompleteHit",
                go_json_kind(other)
            )));
        }
    }
    let message = json_string(obj, "message").map_err(bad)?;
    let mut hits = Vec::with_capacity(hits_raw.len());
    for mut h in hits_raw {
        for f in [
            &mut h.insert,
            &mut h.label,
            &mut h.psub,
            &mut h.detail,
            &mut h.badge,
            &mut h.action,
        ] {
            *f = f.trim().to_string();
        }
        if h.label.is_empty() {
            h.label = h.insert.clone();
        }
        if h.label.is_empty() && h.action.is_empty() {
            continue;
        }
        hits.push(h);
        if hits.len() >= PROMPT_COMPLETE_MAX_HITS {
            break;
        }
    }
    Ok(PromptCompleteResult {
        hits,
        message: message.trim().to_string(),
    })
}

/// Runs a `[[search]]` shebang; stdout is the complete-script shape.
pub async fn run_search_shebang(
    ctx: &RunCtx,
    search: &SearchAction,
    rc: PromptRunContext,
) -> Result<PromptCompleteResult, Error> {
    let prompt = PromptAction {
        id: search.id.clone(),
        name: search.name.clone(),
        complete_interpreter: INTERPRETER_SHEBANG.to_string(),
        complete_body: search.body.clone(),
        complete_path: search.script_path.clone(),
        bundle_dir: search.bundle_dir.clone(),
        ..Default::default()
    };
    run_prompt_complete_shebang(ctx, &prompt, rc).await
}

/// Runs an `[[expand]]` shebang; stdout is the `[[prompt]]` invoke shape.
pub async fn run_expand_shebang(
    ctx: &RunCtx,
    expand: &ExpandAction,
    rc: PromptRunContext,
) -> Result<PromptResult, Error> {
    let ms = if expand.timeout_ms > 0 {
        expand.timeout_ms
    } else {
        EXPAND_TIMEOUT.as_millis() as i64
    };
    let prompt = PromptAction {
        id: expand.id.clone(),
        name: expand.name.clone(),
        interpreter: INTERPRETER_SHEBANG.to_string(),
        body: expand.body.clone(),
        script_path: expand.script_path.clone(),
        bundle_dir: expand.bundle_dir.clone(),
        timeout_ms: ms,
        ..Default::default()
    };
    run_prompt_shebang(ctx, &prompt, rc).await
}

/// One ⌘K section from [`collect_search_groups`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaletteSearchGroup {
    pub name: String,
    pub icon_svg: String,
    pub hits: Vec<PromptCompleteHit>,
}

/// Runs every `[[search]]` concurrently. A failed or timed-out pack
/// contributes an empty hit list; the rest still return, in input order.
pub async fn collect_search_groups(
    ctx: &RunCtx,
    searches: &[SearchAction],
    make_rc: Option<&(dyn Fn(&SearchAction) -> PromptRunContext + Sync)>,
) -> Vec<PaletteSearchGroup> {
    let runs = searches.iter().map(|se| async move {
        let rc = make_rc.map(|f| f(se)).unwrap_or_default();
        let cctx = ctx.with_timeout(se.timeout());
        let hits = run_search_shebang(&cctx, se, rc)
            .await
            .map(|r| r.hits)
            .unwrap_or_default();
        PaletteSearchGroup {
            name: se.name.clone(),
            icon_svg: se.icon_svg.clone(),
            hits,
        }
    });
    futures::future::join_all(runs).await
}

/// Runs the first matching expander that returns a non-skip prompt. No
/// match (or every match skipping) returns the original text. A matched
/// expander that errors is returned so the host can toast.
pub async fn expand_matching(
    text: &str,
    expands: &[ExpandAction],
    make_rc: Option<&(dyn Fn(&ExpandAction) -> PromptRunContext + Sync)>,
) -> Result<PromptResult, Error> {
    let trimmed = text.trim();
    for ex in expands {
        if !ex.matches(trimmed) {
            continue;
        }
        let mut rc = make_rc.map(|f| f(ex)).unwrap_or_default();
        if rc.args.trim().is_empty() {
            rc.args = trimmed.to_string();
        }
        let ctx = RunCtx::background().with_timeout(ex.timeout());
        let res = run_expand_shebang(&ctx, ex, rc).await?;
        if res.skip || res.prompt.trim().is_empty() {
            continue;
        }
        return Ok(res);
    }
    Ok(PromptResult {
        prompt: trimmed.to_string(),
        ..Default::default()
    })
}
