//! One Codex turn over a per-turn `codex app-server` (Go `CodexRunner.Run`
//! and `consumeTurn`).
//!
//! | TurnRequest | Codex |
//! |---|---|
//! | `worktree` | `cwd` + process directory |
//! | `model`, flags `effort` / `fast` | thread/turn model, `effort`, `serviceTier` |
//! | flags `context=1m` | `model_context_window` / `model_auto_compact_token_limit` overrides |
//! | `system_prompt` | `developerInstructions` on thread/start and thread/resume, never also in the user text |
//! | `append_system_prompt` | user-text suffix on a fresh thread or a remount |
//! | `turn_append_prompt` | user-text suffix every turn |
//! | `mcp_url`, `playwright`, `user_mcp` | `-c mcp_servers…` overrides (nothing written to the worktree) |
//! | `resume_session` | thread/resume, falling back to a new thread |
//! | steers | `turn/steer` (30 s each) |
//! | cancel | `turn/interrupt` + process-group kill |
//! | read-only allowlist or reviewer | `read-only` sandbox with network (`gh pr view`) |
//! | `bypassPermissions` | `danger-full-access`, approvals `never` |
//!
//! ## Allowlist translation (risk R9)
//!
//! Codex has no per-tool allowlist. The engine's Claude-syntax list is read
//! for one bit: a non-empty list without `Edit`, `Write` or `NotebookEdit`
//! (or the reviewer role) runs in the read-only sandbox; anything else gets
//! `workspace-write` (or `danger-full-access` under bypass). Rule syntax
//! (`Bash(git diff:*)`) and `mcp__*` globs are ignored: Recur MCP tools are
//! auto-approved through the server's `default_tools_approval_mode`, and
//! sandbox exceptions reach the human as approval requests.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::{
    AgentEvent, ContextUsageTracker, ErrorKind, EventSink, Flags, McpServerStatus, ProviderError,
    TurnContext, TurnRequest, TurnResult, standing_in_user_prompt,
};
use recur_domain::{AgentRole, PERMISSION_MODE_BYPASS};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::approvals::{self, FileItems, TurnTarget, approval_policy, routes_to_human};
use crate::client::Client;
use crate::items::{TurnState, item_completed, item_started, turn_usage};
use crate::mcp::{McpServer, config_overrides};
use crate::rpc::{BoxFuture, Ctx, NotifyFn};
use crate::server::{Server, ServerConfig};
use crate::types::{SandboxPolicy, ThreadStartParams, TurnStartParams, TurnSteerParams, UserInput};

/// Codex's default GPT-5.6 window. Larger prompts bill at 2× input / 1.5×
/// output, so the window is raised only when the Context flag says so.
const GPT56_DEFAULT_CONTEXT_WINDOW: i64 = 272_000;

/// Fallback tier when a saved flag still says `fast=true`.
const FAST_SERVICE_TIER: &str = "priority";

const STEER_TIMEOUT: Duration = Duration::from_secs(30);
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(5);

/// The settings a turn runs with.
#[derive(Debug, Clone, Default)]
pub struct RunConfig {
    pub bin: String,
    pub api_key: String,
    /// Seed `CODEX_HOME` from `~/.codex/auth.json` when no key is used.
    pub chatgpt_auth: bool,
    pub home: std::path::PathBuf,
    /// `sandbox_workspace_write.network_access` on write roles.
    pub network_access: bool,
    pub env_scrub: Vec<String>,
    pub seed_auth_from: Option<std::path::PathBuf>,
}

pub async fn run(
    rc: RunConfig,
    req: TurnRequest,
    cx: TurnContext,
) -> Result<TurnResult, ProviderError> {
    if req.worktree.to_string_lossy().trim().is_empty() {
        return Err(ProviderError::failed(
            "codex runner: worktree path is required",
        ));
    }
    let servers = mcp_servers_for(&req);
    let mut overrides = config_overrides(&servers);
    overrides.extend(long_context_overrides(&req.flags));
    overrides.extend(network_overrides(rc.network_access));
    overrides.push(format!(
        r#"approval_policy="{}""#,
        approval_policy(&req.permission_mode)
    ));
    // ChatGPT login wins: a leftover key must not while the toggle is on.
    let api_key = if rc.chatgpt_auth {
        String::new()
    } else {
        rc.api_key.clone()
    };
    let mut env_scrub = rc.env_scrub.clone();
    env_scrub.extend(req.env_scrub.iter().cloned());
    let cfg = ServerConfig {
        bin: rc.bin.clone(),
        home: rc.home.clone(),
        worktree: req.worktree.clone(),
        seed_auth_from: if api_key.is_empty() && rc.chatgpt_auth {
            rc.seed_auth_from.clone()
        } else {
            None
        },
        api_key,
        config_overrides: overrides,
        env_scrub,
    };
    let ctx = Ctx::new(cx.cancel.clone());
    let srv = Server::start(&cfg, &ctx)
        .await
        .map_err(|e| fail(&cx, format!("codex start: {e}")))?;
    let res = drive(&srv, &rc, &req, servers, cx).await;
    srv.close().await;
    res
}

/// Cancellation wraps Go's `context.Canceled` with `%w`, so a cancelled
/// turn's error is a cancel even when it carries a step's wording.
fn fail(cx: &TurnContext, msg: String) -> ProviderError {
    let kind = if cx.cancel.is_cancelled() {
        ErrorKind::Canceled
    } else {
        ErrorKind::Failed
    };
    ProviderError::new(kind, msg)
}

async fn drive(
    srv: &Server,
    rc: &RunConfig,
    req: &TurnRequest,
    servers: Vec<McpServer>,
    cx: TurnContext,
) -> Result<TurnResult, ProviderError> {
    let client = Client::new(srv.conn().clone());
    let file_items = FileItems::default();
    if routes_to_human(&req.permission_mode, cx.permissions.is_some()) {
        let target = TurnTarget {
            workspace_id: req.workspace_id.clone(),
            agent_id: req.agent_id.clone(),
            permission_mode: req.permission_mode.clone(),
        };
        let asker = cx.permissions.clone();
        let cancel = cx.cancel.clone();
        let items = file_items.clone();
        srv.conn()
            .on_server_request(Some(Arc::new(move |method: String, params: String| {
                let (target, asker, cancel, items) =
                    (target.clone(), asker.clone(), cancel.clone(), items.clone());
                Box::pin(async move {
                    Ok(approvals::handle(
                        &method,
                        &params,
                        &target,
                        asker.as_ref(),
                        &cancel,
                        &items,
                    )
                    .await)
                }) as BoxFuture<_>
            })));
    }

    let (sandbox, turn_sandbox) = sandbox_for(req, rc.network_access);
    let thread_params = ThreadStartParams {
        model: req.model.clone(),
        cwd: req.worktree.to_string_lossy().into_owned(),
        approval_policy: approval_policy(&req.permission_mode).into(),
        sandbox: sandbox.into(),
        developer_instructions: req.system_prompt.clone(),
        service_name: "recur".into(),
        ..Default::default()
    };
    let ctx = Ctx::new(cx.cancel.clone());
    let mut resumed = false;
    let thread = if req.resume_session.is_empty() {
        client.thread_start(&ctx, thread_params).await
    } else {
        match client
            .thread_resume(&ctx, &req.resume_session, &thread_params)
            .await
        {
            Ok(t) => {
                resumed = true;
                Ok(t)
            }
            Err(e) => {
                tracing::warn!(
                    "codex thread/resume {:?} failed ({e}) — starting a new thread",
                    req.resume_session
                );
                client.thread_start(&ctx, thread_params).await
            }
        }
    };
    let thread =
        thread.map_err(|e| fail(&cx, format!("codex thread: {e}\n{}", srv.stderr_tail())))?;

    let emitter = Arc::new(Emitter::new(Arc::clone(&cx.sink)));
    emitter.emit(AgentEvent::Init {
        session_id: thread.id.clone(),
        model: req.model.clone(),
        mcp_servers: servers
            .iter()
            .map(|s| McpServerStatus {
                name: s.name.clone(),
                status: "connected".into(),
            })
            .collect(),
    });
    let prompt = user_prompt(req, resumed);
    consume_turn(
        srv,
        &client,
        req,
        &thread.id,
        turn_sandbox,
        prompt,
        cx,
        emitter,
        file_items,
    )
    .await
}

/// The `turn/start` user text. `system_prompt` is already
/// `developerInstructions` (including a Coder's Maestro plan), so it is never
/// repeated here. `append_system_prompt` has no Codex flag, so it rides
/// in-band on a fresh thread and on remounts, not on an ordinary resume.
pub fn user_prompt(req: &TurnRequest, resumed: bool) -> String {
    let prompt = join_append_prompt(&req.prompt, &req.turn_append_prompt);
    if !req.append_system_prompt.is_empty() && standing_in_user_prompt(req, resumed) {
        return join_append_prompt(&prompt, &req.append_system_prompt);
    }
    prompt
}

fn join_append_prompt(base: &str, extra: &str) -> String {
    let (base, extra) = (base.trim(), extra.trim());
    match (base.is_empty(), extra.is_empty()) {
        (true, _) => extra.to_owned(),
        (_, true) => base.to_owned(),
        _ => format!("{base}\n\n{extra}"),
    }
}

/// Serializes every emit and closes the stream at the result: no event after
/// it, even from a notification callback already running when the handler
/// was cleared.
struct Emitter {
    sink: Arc<dyn EventSink>,
    gate: Mutex<(bool, ContextUsageTracker)>,
}

impl Emitter {
    fn new(sink: Arc<dyn EventSink>) -> Self {
        Self {
            sink,
            gate: Mutex::new((false, ContextUsageTracker::default())),
        }
    }

    fn emit(&self, mut ev: AgentEvent) {
        let mut g = self.gate.lock();
        if g.0 {
            return;
        }
        g.1.observe(&mut ev);
        self.sink.emit(ev);
    }

    /// Stops further events without a result, so cancel settlement owns the
    /// turn footer.
    fn detach(&self) {
        self.gate.lock().0 = true;
    }

    fn finish(&self, mut ev: AgentEvent) {
        let mut g = self.gate.lock();
        g.0 = true;
        g.1.observe(&mut ev);
        self.sink.emit(ev);
    }
}

struct TurnDone {
    status: String,
    error: String,
}

#[allow(clippy::too_many_arguments)]
async fn consume_turn(
    srv: &Server,
    client: &Client,
    req: &TurnRequest,
    thread_id: &str,
    turn_sandbox: SandboxPolicy,
    prompt: String,
    cx: TurnContext,
    emitter: Arc<Emitter>,
    file_items: FileItems,
) -> Result<TurnResult, ProviderError> {
    let state = Arc::new(Mutex::new(TurnState::default()));
    let (done_tx, mut done_rx) = mpsc::channel::<TurnDone>(1);
    let conn = client.conn().clone();

    // Registered before turn/start so a fast turn/completed (or an early
    // item/started) is not dropped.
    let partials = req.include_partials;
    let handler: NotifyFn = {
        let (state, emitter) = (Arc::clone(&state), Arc::clone(&emitter));
        Arc::new(move |method: &str, params: &str| {
            let v = || serde_json::from_str::<Value>(params).unwrap_or_default();
            let text_of = |v: &Value, k: &str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let ev = match method {
                "turn/completed" => {
                    let turn = v().get("turn").cloned().unwrap_or_default();
                    let _ = done_tx.try_send(TurnDone {
                        status: text_of(&turn, "status"),
                        error: turn
                            .get("error")
                            .map(|e| text_of(e, "message"))
                            .unwrap_or_default(),
                    });
                    None
                }
                "item/agentMessage/delta" => {
                    let p = v();
                    let mut chunk = text_of(&p, "delta");
                    if chunk.is_empty() {
                        chunk = text_of(&p, "text");
                    }
                    if chunk.is_empty() {
                        return;
                    }
                    state.lock().final_text.push_str(&chunk);
                    partials.then_some(AgentEvent::TextDelta {
                        text: chunk,
                        parent: String::new(),
                    })
                }
                "item/completed" => item_completed(params, &mut state.lock()),
                "item/started" => item_started(params, &mut state.lock(), &file_items),
                // summaryTextDelta is the readable summary (GPT-5.x);
                // textDelta is raw reasoning when a model exposes it.
                "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                    let delta = text_of(&v(), "delta");
                    (partials && !delta.is_empty()).then_some(AgentEvent::ThinkingDelta {
                        text: delta,
                        parent: String::new(),
                    })
                }
                // A boundary between summary sections (index 0 is the first).
                "item/reasoning/summaryPartAdded" => {
                    let idx = v()
                        .get("summaryIndex")
                        .and_then(Value::as_i64)
                        .unwrap_or_default();
                    (partials && idx > 0).then(|| AgentEvent::ThinkingDelta {
                        text: "\n\n".into(),
                        parent: String::new(),
                    })
                }
                "thread/tokenUsage/updated" => Some(state.lock().observe_usage(params)),
                _ => None,
            };
            if let Some(ev) = ev {
                emitter.emit(ev);
            }
        })
    };
    conn.on_notification(Some(handler));

    // turn/completed has no duration; wall time (floored at 1 ms) lets the
    // result publish session.turn_end and the transcript footer that hosts
    // the findings digest.
    let started = Instant::now();
    let ctx = Ctx::new(cx.cancel.clone());
    let turn = client
        .turn_start(
            &ctx,
            &TurnStartParams {
                thread_id: thread_id.into(),
                input: vec![UserInput::text(prompt)],
                model: req.model.clone(),
                cwd: req.worktree.to_string_lossy().into_owned(),
                approval_policy: approval_policy(&req.permission_mode).into(),
                sandbox_policy: Some(turn_sandbox),
                effort: req.flags.get("effort").cloned().unwrap_or_default(),
                service_tier: service_tier_from_flags(&req.flags),
                collaboration: None,
            },
        )
        .await;
    let turn = match turn {
        Ok(t) => t,
        Err(e) => {
            conn.on_notification(None);
            return Err(fail(
                &cx,
                format!("codex turn/start: {e}\n{}", srv.stderr_tail()),
            ));
        }
    };

    // Mid-turn steers (D49). turn_cancel ends the loop when the turn settles.
    let turn_cancel = cx.cancel.child_token();
    let steer_task = cx.steer.map(|mut inbox| {
        let (client, emitter, cancel) = (client.clone(), Arc::clone(&emitter), turn_cancel.clone());
        let thread_id = thread_id.to_owned();
        tokio::spawn(async move {
            loop {
                let s = tokio::select! {
                    _ = cancel.cancelled() => return,
                    s = inbox.recv() => match s {
                        Some(s) => s,
                        None => return,
                    },
                };
                // Conductor steers go in as is: Codex never had the
                // Maestro prefix Claude's stdin path adds.
                let text = if s.is_plain() {
                    s.plain_parts().0
                } else {
                    s.prompt.trim().to_owned()
                };
                if text.is_empty() {
                    s.ack(false);
                    continue;
                }
                let ctx = Ctx::new(cancel.clone()).with_timeout(STEER_TIMEOUT);
                let sent = client
                    .turn_steer(
                        &ctx,
                        &TurnSteerParams {
                            thread_id: thread_id.clone(),
                            input: vec![UserInput::text(text)],
                        },
                    )
                    .await;
                if sent.is_err() {
                    s.ack(false);
                    continue;
                }
                if let Some(user) = s.user_event(true) {
                    emitter.emit(user);
                }
                s.ack(true);
            }
        })
    });
    let settle_steers = async |task: Option<tokio::task::JoinHandle<()>>| {
        turn_cancel.cancel();
        if let Some(t) = task {
            let _ = t.await;
        }
    };

    let tail_result = |text: String, stderr: String| TurnResult {
        session_id: thread_id.into(),
        subtype: "error_during_execution".into(),
        is_error: true,
        text,
        stderr,
        duration_ms: elapsed_ms(started),
        ..Default::default()
    };

    let done = tokio::select! {
        d = done_rx.recv() => d.unwrap_or(TurnDone { status: String::new(), error: String::new() }),
        _ = srv.reaped() => {
            settle_steers(steer_task).await;
            let tail = srv.stderr_tail();
            let mut msg = "codex app-server exited before turn completed".to_owned();
            if !tail.is_empty() {
                msg = format!("{msg}\n{tail}");
            }
            conn.on_notification(None);
            emitter.detach();
            return Err(ProviderError::failed(msg.clone()).with_partial(tail_result(msg, tail)));
        }
        _ = cx.cancel.cancelled() => {
            settle_steers(steer_task).await;
            // No TurnEnd: cancel settlement (user Stop vs ask_user /
            // switch_mode park) owns the footer.
            conn.on_notification(None);
            emitter.detach();
            let (client, thread_id, turn_id) = (client.clone(), thread_id.to_owned(), turn.id.clone());
            tokio::spawn(async move {
                let ctx = Ctx::default().with_timeout(INTERRUPT_TIMEOUT);
                let _ = client.turn_interrupt(&ctx, &thread_id, &turn_id).await;
            });
            let partial = tail_result("context canceled".into(), srv.stderr_tail());
            return Err(ProviderError::canceled().with_partial(partial));
        }
    };
    settle_steers(steer_task).await;

    let mut res = {
        let st = state.lock();
        TurnResult {
            session_id: thread_id.into(),
            subtype: "success".into(),
            text: st.final_text.clone(),
            stderr: srv.stderr_tail(),
            usage: turn_usage(&st.usage_last, &st.usage_turn, st.context_window),
            ..Default::default()
        }
    };
    let status = done.status.to_lowercase();
    if !status.is_empty() && status != "completed" && status != "success" {
        res.is_error = true;
        res.subtype = "error_during_execution".into();
        if !done.error.is_empty() {
            res.text = done.error;
        }
    }
    res.duration_ms = elapsed_ms(started);
    conn.on_notification(None);
    if res.is_error {
        // Persist tokens and context through the result frame, but with no
        // duration, so session.turn_end is not published; settlement stamps
        // the footer on the error note from the returned result.
        let mut frame = res.clone();
        frame.duration_ms = 0;
        emitter.finish(AgentEvent::TurnEnd(frame));
        return Ok(res);
    }
    emitter.finish(AgentEvent::TurnEnd(res.clone()));
    Ok(res)
}

/// Wall time since `started`, floored at 1 ms: a 0 would skip
/// session.turn_end and the transcript footer.
pub fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis())
        .unwrap_or(i64::MAX)
        .max(1)
}

/// The Recur MCP (required), Playwright, and Settings → MCP servers.
pub fn mcp_servers_for(req: &TurnRequest) -> Vec<McpServer> {
    let mut out = Vec::new();
    if !req.mcp_url.is_empty() {
        out.push(McpServer {
            name: "recur".into(),
            url: req.mcp_url.clone(),
            required: true,
            ..Default::default()
        });
    }
    if req.playwright {
        out.push(McpServer {
            name: "playwright".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@playwright/mcp@latest".into()],
            ..Default::default()
        });
    }
    for u in &req.user_mcp {
        let name = u.name.trim();
        if name.is_empty() {
            continue;
        }
        out.push(McpServer {
            name: name.into(),
            url: u.url.clone(),
            command: u.command.clone(),
            args: u.args.clone(),
            env: u.env.clone(),
            http_headers: u.headers.clone(),
            required: false,
        });
    }
    out
}

/// The thread sandbox and turn policy. Ask/Plan/Review chats are read-only
/// through the allowlist even though they are coder turns; read-only still
/// gets network so `gh pr view` / `gh pr diff` work (injected diffs
/// truncate). Bypass on a write role is `danger-full-access`.
pub fn sandbox_for(req: &TurnRequest, network: bool) -> (&'static str, SandboxPolicy) {
    if read_only_allowlist(req.allowed_tools.as_deref()) || role_is_read_only(&req.role) {
        return (
            "read-only",
            SandboxPolicy {
                typ: "readOnly".into(),
                network_access: true,
            },
        );
    }
    if req.permission_mode == PERMISSION_MODE_BYPASS {
        return (
            "danger-full-access",
            SandboxPolicy {
                typ: "dangerFullAccess".into(),
                network_access: false,
            },
        );
    }
    (
        "workspace-write",
        SandboxPolicy {
            typ: "workspaceWrite".into(),
            network_access: network,
        },
    )
}

/// A non-empty allowlist without edit tools (Go `isReadOnlyRole`).
fn read_only_allowlist(allowed: Option<&[String]>) -> bool {
    let allowed = allowed.unwrap_or_default();
    !allowed.is_empty()
        && !allowed
            .iter()
            .any(|t| matches!(t.as_str(), "Edit" | "Write" | "NotebookEdit"))
}

fn role_is_read_only(role: &AgentRole) -> bool {
    *role == AgentRole::Reviewer || role.as_str().eq_ignore_ascii_case("reviewer")
}

/// Lifts the workspace-write sandbox's network block (Settings).
pub fn network_overrides(on: bool) -> Vec<String> {
    if on {
        vec!["sandbox_workspace_write.network_access=true".into()]
    } else {
        Vec::new()
    }
}

/// Raises Codex's window when the Context flag is above the 272K default.
pub fn long_context_overrides(flags: &Flags) -> Vec<String> {
    if parse_context_flag(flags.get("context").map_or("", String::as_str))
        <= GPT56_DEFAULT_CONTEXT_WINDOW
    {
        return Vec::new();
    }
    vec![
        "model_context_window=1000000".into(),
        "model_auto_compact_token_limit=900000".into(),
    ]
}

/// `272k` / `1m` / `1.5M` in tokens; 0 when empty or not positive.
pub fn parse_context_flag(s: &str) -> i64 {
    let s = s.trim().to_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix('m') {
        (n, 1_000_000.0)
    } else if let Some(n) = s.strip_suffix('k') {
        (n, 1_000.0)
    } else {
        (s.as_str(), 1.0)
    };
    match go_parse_float(num.trim()) {
        Some(f) if f > 0.0 => (f * mult) as i64,
        _ => 0,
    }
}

/// Go `strconv.ParseFloat(s, 64)`: decimal and hex (`0x1p-2`) literals with
/// Go's digit-separating underscores, an optional sign, and inf/infinity/nan.
fn go_parse_float(s: &str) -> Option<f64> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    let v = match lower.as_str() {
        "inf" | "infinity" => f64::INFINITY,
        "nan" => f64::NAN,
        _ if lower.starts_with("0x") => parse_hex_float(&lower[2..])?,
        _ => {
            if !underscores_ok(&lower, false) {
                return None;
            }
            lower.replace('_', "").parse::<f64>().ok()?
        }
    };
    Some(if neg { -v } else { v })
}

/// Go's rule: an underscore must sit between two digits (or right after a
/// base prefix).
fn underscores_ok(s: &str, hex: bool) -> bool {
    let b = s.as_bytes();
    let digit = |c: u8| {
        if hex {
            c.is_ascii_hexdigit()
        } else {
            c.is_ascii_digit()
        }
    };
    b.iter().enumerate().all(|(i, &c)| {
        c != b'_'
            || (i + 1 < b.len() && digit(b[i + 1]) && (i == 0 && hex || i > 0 && digit(b[i - 1])))
    })
}

/// The part after `0x`: hex mantissa with an optional point, and a required
/// binary exponent `p±d`.
fn parse_hex_float(s: &str) -> Option<f64> {
    let (mant, exp) = s.split_once('p')?;
    if !underscores_ok(mant, true) || !underscores_ok(exp.trim_start_matches(['+', '-']), false) {
        return None;
    }
    let mant = mant.replace('_', "");
    let exp: i32 = exp.replace('_', "").parse().ok()?;
    let (int, frac) = mant.split_once('.').unwrap_or((&mant, ""));
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let mut v = 0f64;
    for c in int.chars().chain(frac.chars()) {
        v = v * 16.0 + f64::from(c.to_digit(16)?);
    }
    let shift = exp - 4 * i32::try_from(frac.len()).ok()?;
    Some(v * 2f64.powi(shift))
}

/// The `turn/start` service tier: the Fast flag's tier id (legacy
/// `true`/`on` mean `priority`), or a legacy `serviceTier` flag, which wins.
pub fn service_tier_from_flags(flags: &Flags) -> String {
    let get = |k: &str| flags.get(k).map_or("", |v| v.trim());
    let legacy = get("serviceTier");
    if !legacy.is_empty() {
        return match legacy.to_lowercase().as_str() {
            "standard" | "default" | "off" | "false" => String::new(),
            _ => legacy.to_owned(),
        };
    }
    let fast = get("fast");
    match fast.to_lowercase().as_str() {
        "" | "false" | "off" | "0" | "standard" | "default" => String::new(),
        "true" | "on" | "1" => FAST_SERVICE_TIER.into(),
        _ => fast.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|s| (*s).to_owned()).collect())
    }

    fn flags(pairs: &[(&str, &str)]) -> Flags {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn user_text_omits_role() {
        let r = TurnRequest {
            system_prompt: "You are the Maestro — long charter.".into(),
            prompt: "where are we?".into(),
            ..Default::default()
        };
        assert_eq!(user_prompt(&r, false), "where are we?");
        let nudge = TurnRequest {
            system_prompt: "You are the Maestro.".into(),
            append_system_prompt: "Name the chat.".into(),
            prompt: "hi".into(),
            ..Default::default()
        };
        assert_eq!(user_prompt(&nudge, false), "hi\n\nName the chat.");
    }

    #[test]
    fn user_text_omits_plan_and_resume_standing() {
        let plan = "Maestro plan: touch foo.go";
        let first = TurnRequest {
            system_prompt: format!("You are the Coder.\n\n{plan}"),
            append_system_prompt: "This chat is in AGENT mode.".into(),
            prompt: "do the thing".into(),
            ..Default::default()
        };
        let got = user_prompt(&first, false);
        assert!(!got.contains(plan) && got.contains("AGENT mode") && got.contains("do the thing"));
        let resumed = TurnRequest {
            prompt: "and persist it".into(),
            resume_session: "thread-1".into(),
            ..first.clone()
        };
        assert_eq!(user_prompt(&resumed, true), "and persist it");
        let subject = TurnRequest {
            append_system_prompt: "This chat is in AGENT mode.".into(),
            turn_append_prompt: "The operator is looking at GitHub pull request #3.".into(),
            prompt: "what changed?".into(),
            resume_session: "thread-1".into(),
            ..Default::default()
        };
        let got = user_prompt(&subject, true);
        assert_eq!(
            got,
            "what changed?\n\nThe operator is looking at GitHub pull request #3."
        );
        let remount = TurnRequest {
            append_system_prompt: "This chat is in PLAN mode.".into(),
            prompt: "continue".into(),
            resume_session: "thread-1".into(),
            refresh_standing: true,
            ..Default::default()
        };
        assert!(user_prompt(&remount, true).contains("PLAN mode"));
    }

    #[test]
    fn sandbox_follows_allowlist() {
        let coder = |tools: &[&str]| TurnRequest {
            role: AgentRole::Coder,
            allowed_tools: strs(tools),
            ..Default::default()
        };
        let (s, p) = sandbox_for(&coder(&["Read", "Grep", "Bash"]), false);
        assert_eq!(
            (s, p.typ.as_str(), p.network_access),
            ("read-only", "readOnly", true)
        );
        let (s, p) = sandbox_for(&coder(&["Read", "Edit", "Write", "Bash"]), false);
        assert_eq!(
            (s, p.typ.as_str(), p.network_access),
            ("workspace-write", "workspaceWrite", false)
        );
        assert_eq!(
            recur_domain::json::to_string(&p).unwrap(),
            r#"{"type":"workspaceWrite"}"#
        );
        let (_, on) = sandbox_for(&coder(&["Read", "Edit", "Write", "Bash"]), true);
        assert_eq!(
            recur_domain::json::to_string(&on).unwrap(),
            r#"{"type":"workspaceWrite","networkAccess":true}"#
        );
        let reviewer = TurnRequest {
            role: AgentRole::Reviewer,
            ..Default::default()
        };
        let (s, p) = sandbox_for(&reviewer, false);
        assert_eq!(
            (s, p.typ.as_str(), p.network_access),
            ("read-only", "readOnly", true)
        );
        assert_eq!(
            recur_domain::json::to_string(&p).unwrap(),
            r#"{"type":"readOnly","networkAccess":true}"#
        );
    }

    #[test]
    fn bypass_is_danger_full_access() {
        let edit = TurnRequest {
            role: AgentRole::Coder,
            allowed_tools: strs(&["Read", "Edit", "Write", "Bash"]),
            permission_mode: "bypassPermissions".into(),
            ..Default::default()
        };
        let (s, p) = sandbox_for(&edit, true);
        assert_eq!(
            (s, p.typ.as_str(), p.network_access),
            ("danger-full-access", "dangerFullAccess", false)
        );
        let reviewer = TurnRequest {
            role: AgentRole::Reviewer,
            permission_mode: "bypassPermissions".into(),
            ..Default::default()
        };
        let (s, p) = sandbox_for(&reviewer, false);
        assert_eq!((s, p.network_access), ("read-only", true));
    }

    #[test]
    fn overrides() {
        assert!(network_overrides(false).is_empty());
        assert_eq!(
            network_overrides(true),
            ["sandbox_workspace_write.network_access=true"]
        );
        assert!(long_context_overrides(&Flags::new()).is_empty());
        assert!(long_context_overrides(&flags(&[("context", "272k")])).is_empty());
        assert_eq!(
            long_context_overrides(&flags(&[("context", "1m")])),
            [
                "model_context_window=1000000",
                "model_auto_compact_token_limit=900000"
            ]
        );
        for (s, want) in [
            ("", 0),
            ("272k", 272_000),
            ("1M", 1_000_000),
            ("1.5m", 1_500_000),
            ("x", 0),
            ("-1k", 0),
            ("500", 500),
        ] {
            assert_eq!(parse_context_flag(s), want, "{s}");
        }
    }

    #[test]
    fn service_tiers() {
        for (f, want) in [
            (flags(&[]), ""),
            (flags(&[("fast", "false")]), ""),
            (flags(&[("fast", "true")]), "priority"),
            (flags(&[("fast", "on")]), "priority"),
            (flags(&[("fast", "priority")]), "priority"),
            (flags(&[("fast", "turbo")]), "turbo"),
            (flags(&[("serviceTier", "standard")]), ""),
            (flags(&[("serviceTier", "priority")]), "priority"),
            (
                flags(&[("serviceTier", "priority"), ("fast", "false")]),
                "priority",
            ),
        ] {
            assert_eq!(service_tier_from_flags(&f), want, "{f:?}");
        }
    }

    #[test]
    fn elapsed() {
        assert!(elapsed_ms(Instant::now() - Duration::from_millis(1500)) >= 1400);
        assert_eq!(elapsed_ms(Instant::now() + Duration::from_secs(1)), 1);
    }

    #[test]
    fn mcp_servers() {
        let r = TurnRequest {
            mcp_url: "http://127.0.0.1:1/t/mcp".into(),
            playwright: true,
            user_mcp: vec![
                recur_agent::McpAttach {
                    name: " ".into(),
                    ..Default::default()
                },
                recur_agent::McpAttach {
                    name: "linear".into(),
                    url: "https://mcp.linear.app/mcp".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let got = mcp_servers_for(&r);
        let names: Vec<_> = got.iter().map(|s| (s.name.as_str(), s.required)).collect();
        assert_eq!(
            names,
            [("recur", true), ("playwright", false), ("linear", false)]
        );
    }
}
