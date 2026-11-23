//! One ACP turn over a per-turn agent process.
//!
//! | TurnRequest | ACP |
//! |---|---|
//! | `worktree` | process cwd + session `cwd` |
//! | `resume_session` (`<agent id>:<sid>`) | `session/resume`, else `session/load`, else a new session |
//! | `mcp_url`, `playwright`, `user_mcp` | session `mcpServers` (nothing written to the worktree) |
//! | `model`, flag `effort` | config options `model` / `thought_level` (else `session/set_model`) |
//! | read-only allowlist or reviewer | `plan` mode when offered; edit/delete/move permissions rejected |
//! | `system_prompt`, `append_system_prompt` | `<role-instructions>` prefix on a fresh session or a remount |
//! | `turn_append_prompt` | user-text suffix every turn |
//! | `images` | `image` content blocks, markers kept in the text |
//! | cancel | `session/cancel`, then process-group kill |

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::attach::images_in_marker_order;
use recur_agent::permission::summary;
use recur_agent::{
    AgentEvent, ContextUsageTracker, ErrorKind, EventSink, McpServerStatus, PermissionAsker,
    ProviderError, TokenUsage, ToolDisplay, ToolResultEvent, ToolStep, TurnContext, TurnRequest,
    TurnResult, standing_in_user_prompt,
};
use recur_domain::{
    AcpAgent, AgentRole, PERMISSION_MODE_BYPASS, PERMISSION_MODE_DENY, PermissionRequest,
};
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, SessionInfo, select_options, session_error, with_tail};
use crate::launch::Launch;
use crate::rpc::{AUTH_REQUIRED, BoxFuture, METHOD_NOT_FOUND, RpcError};

/// How long a cancelled prompt may wind down before the group is killed.
const CANCEL_GRACE: Duration = Duration::from_secs(1);

pub async fn run(
    def: &'static AcpAgent,
    launch: &Launch,
    req: TurnRequest,
    cx: TurnContext,
) -> Result<TurnResult, ProviderError> {
    if req.worktree.as_os_str().is_empty() {
        return Err(ProviderError::failed(format!(
            "{}: worktree path is required",
            def.label
        )));
    }
    let a = tokio::select! {
        a = Agent::start(launch, &req.worktree) => a
            .map_err(|e| fail(&cx.cancel, format!("{}: {e}", def.label)))?,
        () = cx.cancel.cancelled() => return Err(ProviderError::canceled()),
    };
    let res = drive(def, &a, &req, cx.cancel, cx.sink, cx.permissions).await;
    a.close().await;
    res
}

fn fail(cancel: &CancellationToken, msg: String) -> ProviderError {
    let kind = if cancel.is_cancelled() {
        ErrorKind::Canceled
    } else {
        ErrorKind::Failed
    };
    ProviderError::new(kind, msg)
}

/// A setup call that gives up on cancel or process exit.
async fn call(
    a: &Agent,
    cancel: &CancellationToken,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    let gone = |m: &str| RpcError {
        code: 0,
        message: m.into(),
    };
    tokio::select! {
        r = a.conn.call(method, params) => r,
        () = cancel.cancelled() => Err(gone("context canceled")),
        () = a.exited() => Err(gone("the agent exited")),
    }
}

async fn drive(
    def: &'static AcpAgent,
    a: &Agent,
    req: &TurnRequest,
    cancel: CancellationToken,
    sink: Arc<dyn EventSink>,
    permissions: Option<Arc<dyn PermissionAsker>>,
) -> Result<TurnResult, ProviderError> {
    let servers = mcp_servers(req);
    let session_params = |sid: Option<&str>| {
        let mut p = json!({"cwd": req.worktree, "mcpServers": servers});
        if let Some(s) = sid {
            p["sessionId"] = s.into();
        }
        p
    };
    let setup_err = |what: &str, e: RpcError| {
        fail(
            &cancel,
            with_tail(format!("{} {what}: {e}", def.label), &a.stderr_tail()),
        )
    };

    let prefix = format!("{}:", def.id);
    let old = req
        .resume_session
        .trim()
        .strip_prefix(&prefix)
        .filter(|s| !s.is_empty());
    let mut opened = None;
    if let Some(old) = old {
        let method = if a.cap(&["sessionCapabilities", "resume"]) {
            Some("session/resume")
        } else if a.cap(&["loadSession"]) {
            // No update handler is installed yet, so the replay is dropped.
            Some("session/load")
        } else {
            None
        };
        if let Some(m) = method {
            match call(a, &cancel, m, session_params(Some(old))).await {
                Ok(v) => opened = Some((old.to_owned(), SessionInfo::from(&v))),
                Err(_) if cancel.is_cancelled() => return Err(ProviderError::canceled()),
                Err(e) => tracing::warn!(
                    "{} {m} {old:?} failed ({e}): starting a new session",
                    def.label
                ),
            }
        }
    }
    let resumed = opened.is_some();
    let (sid, info) = match opened {
        Some(o) => o,
        None => {
            let v = call(a, &cancel, "session/new", session_params(None))
                .await
                .map_err(|e| {
                    if e.code == AUTH_REQUIRED {
                        ProviderError::new(ErrorKind::Auth, session_error(def, &e))
                    } else {
                        setup_err("session/new", e)
                    }
                })?;
            let Some(sid) = v["sessionId"].as_str().filter(|s| !s.is_empty()) else {
                return Err(ProviderError::failed(format!(
                    "{}: session/new returned no sessionId",
                    def.label
                )));
            };
            (sid.to_owned(), SessionInfo::from(&v))
        }
    };

    for (method, params, required) in config_calls(&sid, &info, req) {
        if let Err(e) = call(a, &cancel, method, params).await {
            if required || cancel.is_cancelled() {
                return Err(setup_err(method, e));
            }
            tracing::warn!("{} {method} ignored: {e}", def.label);
        }
    }

    let handle = format!("{}:{sid}", def.id);
    let emitter = Arc::new(Emitter::new(sink));
    emitter.emit(AgentEvent::Init {
        session_id: handle.clone(),
        model: req.model.clone(),
        mcp_servers: servers
            .iter()
            .map(|s| McpServerStatus {
                name: s["name"].as_str().unwrap_or_default().to_owned(),
                status: "connected".into(),
            })
            .collect(),
    });

    let state = Arc::new(Mutex::new(State {
        servers: servers
            .iter()
            .filter_map(|s| s["name"].as_str().map(str::to_owned))
            .collect(),
        ..Default::default()
    }));
    let partials = req.include_partials;
    {
        let (state, emitter, sid) = (Arc::clone(&state), Arc::clone(&emitter), sid.clone());
        a.conn
            .on_notification(Some(Arc::new(move |method: &str, p: &Value| {
                if method != "session/update" || p["sessionId"].as_str().is_some_and(|s| s != sid) {
                    return;
                }
                let mut st = state.lock();
                for ev in st.on_update(&p["update"], partials) {
                    emitter.emit(ev);
                }
            })));
    }
    {
        let gate = Arc::new(Gate {
            state: Arc::clone(&state),
            emitter: Arc::clone(&emitter),
            asker: permissions,
            cancel: cancel.clone(),
            read_only: read_only(req),
            mode: req.permission_mode.clone(),
            workspace_id: req.workspace_id.clone(),
            agent_code: agent_code(&req.workspace_id, &req.agent_id),
        });
        a.conn
            .on_request(Some(Arc::new(move |method: String, params: Value| {
                let gate = Arc::clone(&gate);
                Box::pin(async move {
                    if method != "session/request_permission" {
                        return Err(RpcError {
                            code: METHOD_NOT_FOUND,
                            message: format!("method not found: {method}"),
                        });
                    }
                    Ok(gate.permission(&params).await)
                }) as BoxFuture<_>
            })));
    }

    let blocks = prompt_blocks(req, resumed, a.cap(&["promptCapabilities", "image"]));
    let started = Instant::now();
    let tail_result = |text: String| TurnResult {
        session_id: handle.clone(),
        subtype: "error_during_execution".into(),
        is_error: true,
        text,
        stderr: a.stderr_tail(),
        duration_ms: elapsed_ms(started),
        ..Default::default()
    };
    let prompt = a.conn.call(
        "session/prompt",
        json!({"sessionId": sid, "prompt": blocks}),
    );
    tokio::pin!(prompt);
    let exited = |msg: String| {
        emitter.detach();
        let msg = with_tail(msg, &a.stderr_tail());
        ProviderError::failed(msg.clone()).with_partial(tail_result(msg))
    };
    let out = tokio::select! {
        biased;
        r = &mut prompt => r,
        () = cancel.cancelled() => {
            // No TurnEnd: cancel settlement owns the footer.
            emitter.detach();
            let _ = a.conn.notify("session/cancel", json!({"sessionId": sid})).await;
            let _ = tokio::time::timeout(CANCEL_GRACE, &mut prompt).await;
            return Err(ProviderError::canceled().with_partial(tail_result("context canceled".into())));
        }
        () = a.exited() => match tokio::time::timeout(Duration::from_millis(500), &mut prompt).await {
            Ok(Ok(v)) => Ok(v),
            _ => return Err(exited(format!("{} exited before the turn completed", def.label))),
        },
    };
    let v = match out {
        Ok(v) => v,
        Err(e) if e.code == AUTH_REQUIRED => {
            emitter.detach();
            return Err(ProviderError::new(ErrorKind::Auth, session_error(def, &e)));
        }
        Err(e) if e.code == 0 => {
            return Err(exited(format!(
                "{} exited before the turn completed",
                def.label
            )));
        }
        Err(e) => return Err(exited(format!("{} session/prompt: {e}", def.label))),
    };
    a.conn.on_notification(None);

    let mut st = state.lock();
    for ev in st.flush() {
        emitter.emit(ev);
    }
    let u = &v["usage"];
    let n = |k: &str| u[k].as_i64().unwrap_or_default();
    let mut res = TurnResult {
        session_id: handle,
        subtype: "success".into(),
        text: st.last_text.clone(),
        stderr: a.stderr_tail(),
        duration_ms: elapsed_ms(started),
        usage: TokenUsage {
            input_tokens: n("inputTokens"),
            output_tokens: n("outputTokens"),
            cache_read_tokens: n("cachedReadTokens"),
            cache_creation_tokens: n("cachedWriteTokens"),
            context_window: st.context_window,
        },
        ..Default::default()
    };
    drop(st);
    let failure = match v["stopReason"].as_str().unwrap_or("end_turn") {
        "end_turn" => None,
        "max_tokens" => Some(("error_max_turns", "the model hit its output token limit")),
        "max_turn_requests" => Some(("error_max_turns", "the agent hit its request limit")),
        "refusal" => Some(("error_during_execution", "the model refused to continue")),
        "cancelled" => Some(("error_during_execution", "the agent cancelled the turn")),
        _ => Some(("error_during_execution", "the agent stopped unexpectedly")),
    };
    if let Some((subtype, text)) = failure {
        res.is_error = true;
        res.subtype = subtype.into();
        res.text = text.into();
        // No duration on the frame, so session.turn_end is not published.
        let mut frame = res.clone();
        frame.duration_ms = 0;
        emitter.finish(AgentEvent::TurnEnd(frame));
        return Ok(res);
    }
    emitter.finish(AgentEvent::TurnEnd(res.clone()));
    Ok(res)
}

/// `(method, params, required)` to apply the turn's model, effort and mode.
/// A model that cannot be set fails the turn; effort and mode are best effort.
fn config_calls(
    sid: &str,
    info: &SessionInfo,
    req: &TurnRequest,
) -> Vec<(&'static str, Value, bool)> {
    let mut out = Vec::new();
    let set_option =
        |o: &Value, value: &str| json!({"sessionId": sid, "configId": o["id"], "value": value});
    let model = req.model.trim();
    if !model.is_empty() {
        if let Some(o) = info.option("model") {
            if o["currentValue"].as_str() != Some(model) {
                out.push(("session/set_config_option", set_option(o, model), true));
            }
        } else if info.models.is_object() && info.models["currentModelId"].as_str() != Some(model) {
            out.push((
                "session/set_model",
                json!({"sessionId": sid, "modelId": model}),
                true,
            ));
        }
    }
    let effort = req.flags.get("effort").map_or("", |s| s.trim());
    if let Some(o) = info.option("thought_level")
        && !effort.is_empty()
        && o["currentValue"].as_str() != Some(effort)
    {
        out.push(("session/set_config_option", set_option(o, effort), false));
    }
    let ro = read_only(req);
    if let Some(o) = info.option("mode") {
        let ids: Vec<String> = select_options(o).into_iter().map(|(v, _)| v).collect();
        if let Some(m) = pick_mode(ro, o["currentValue"].as_str().unwrap_or_default(), &ids) {
            out.push(("session/set_config_option", set_option(o, &m), false));
        }
    } else if let Some(modes) = info.modes["availableModes"].as_array() {
        let ids: Vec<String> = modes
            .iter()
            .filter_map(|m| m["id"].as_str().map(str::to_owned))
            .collect();
        let cur = info.modes["currentModeId"].as_str().unwrap_or_default();
        if let Some(m) = pick_mode(ro, cur, &ids) {
            out.push((
                "session/set_mode",
                json!({"sessionId": sid, "modeId": m}),
                false,
            ));
        }
    }
    out
}

/// `plan` for a read-only turn; leaving a resumed `plan` mode for the first
/// other mode on a write turn.
fn pick_mode(read_only: bool, current: &str, ids: &[String]) -> Option<String> {
    if read_only {
        return (current != "plan" && ids.iter().any(|i| i == "plan")).then(|| "plan".into());
    }
    if current == "plan" {
        return ids.iter().find(|i| *i != "plan").cloned();
    }
    None
}

/// Same rule as Codex's read-only sandbox: a non-empty allowlist without edit
/// tools, or the reviewer role.
pub fn read_only(req: &TurnRequest) -> bool {
    let allowed = req.allowed_tools.as_deref().unwrap_or_default();
    let no_edits = !allowed.is_empty()
        && !allowed
            .iter()
            .any(|t| matches!(t.as_str(), "Edit" | "Write" | "NotebookEdit"));
    no_edits
        || req.role == AgentRole::Reviewer
        || req.role.as_str().eq_ignore_ascii_case("reviewer")
}

/// The Recur MCP, Playwright, and Settings → MCP servers in ACP shape.
pub fn mcp_servers(req: &TurnRequest) -> Vec<Value> {
    let pairs = |m: &BTreeMap<String, String>| {
        m.iter()
            .map(|(k, v)| json!({"name": k, "value": v}))
            .collect::<Vec<_>>()
    };
    let mut out = Vec::new();
    if !req.mcp_url.is_empty() {
        out.push(json!({"type": "http", "name": "recur", "url": req.mcp_url, "headers": []}));
    }
    if req.playwright {
        out.push(json!({"name": "playwright", "command": "npx", "args": ["-y", "@playwright/mcp@latest"], "env": []}));
    }
    for u in &req.user_mcp {
        let name = u.name.trim();
        if name.is_empty() {
            continue;
        }
        if !u.url.is_empty() {
            out.push(
                json!({"type": "http", "name": name, "url": u.url, "headers": pairs(&u.headers)}),
            );
        } else if !u.command.is_empty() {
            out.push(
                json!({"name": name, "command": u.command, "args": u.args, "env": pairs(&u.env)}),
            );
        }
    }
    out
}

/// The user text. ACP has no system prompt, so role and append notes ride
/// in-band on a fresh (or remounted) session, fenced like Cursor's.
pub fn prompt_text(req: &TurnRequest, resumed: bool) -> String {
    let body = join(&req.prompt, &req.turn_append_prompt);
    if !standing_in_user_prompt(req, resumed) {
        return body;
    }
    let role = join(&req.system_prompt, &req.append_system_prompt);
    if role.is_empty() {
        return body;
    }
    format!("<role-instructions>\n{role}\n</role-instructions>\n\n{body}")
}

/// The text block (attachment markers kept, so ids stay visible) and the
/// images in marker order.
fn prompt_blocks(req: &TurnRequest, resumed: bool, images: bool) -> Vec<Value> {
    let text = prompt_text(req, resumed);
    let mut out = vec![json!({"type": "text", "text": text})];
    if images {
        out.extend(
            images_in_marker_order(&req.prompt, &req.images)
                .iter()
                .map(|i| json!({"type": "image", "mimeType": i.mime, "data": i.data})),
        );
    } else if !req.images.is_empty() {
        tracing::warn!(
            "acp agent does not accept images; {} dropped",
            req.images.len()
        );
    }
    out
}

fn join(a: &str, b: &str) -> String {
    match (a.trim(), b.trim()) {
        ("", b) => b.to_owned(),
        (a, "") => a.to_owned(),
        (a, b) => format!("{a}\n\n{b}"),
    }
}

/// Wall time floored at 1 ms: a 0 would skip session.turn_end.
fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis())
        .unwrap_or(i64::MAX)
        .max(1)
}

fn agent_code(workspace_id: &str, agent_id: &str) -> String {
    match agent_id.strip_prefix(&format!("{workspace_id}-")) {
        Some(code) if !code.is_empty() => code.to_owned(),
        _ => agent_id.to_owned(),
    }
}

/// Serializes emits and closes the stream at the result (or on detach).
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

    fn detach(&self) {
        self.gate.lock().0 = true;
    }

    fn finish(&self, ev: AgentEvent) {
        self.emit(ev);
        self.detach();
    }
}

#[derive(Default)]
struct Tool {
    /// The agent's own tool name (nonstandard `name`, Antigravity).
    tool: String,
    kind: String,
    title: String,
    input: String,
    content: Value,
    raw_output: Value,
    status: String,
    stepped: bool,
    done: bool,
}

impl Tool {
    fn merge(&mut self, u: &Value) {
        if !self.stepped {
            if let Some(n) = u["name"].as_str() {
                self.tool = n.into();
            }
            if let Some(k) = u["kind"].as_str() {
                self.kind = k.into();
            }
            if let Some(t) = u["title"].as_str() {
                self.title = t.into();
            }
        }
        match &u["rawInput"] {
            Value::Null => {}
            Value::Object(m) if m.is_empty() => {}
            v => self.input = v.to_string(),
        }
        if !u["content"].is_null() {
            self.content = u["content"].clone();
        }
        if !u["rawOutput"].is_null() {
            self.raw_output = u["rawOutput"].clone();
        }
        if let Some(s) = u["status"].as_str() {
            self.status = s.into();
        }
    }

    /// The raw step name: the agent's tool name when it sends one, else the
    /// ACP kind, or the title when the kind says nothing (`other`, MCP calls).
    /// MCP calls become `mcp__<server>__<tool>`.
    fn name(&self, servers: &[String]) -> String {
        let raw = self.raw_name();
        mcp_name(&raw, servers).unwrap_or(raw)
    }

    /// Agents without a tool name (OpenCode) say grep and glob with one
    /// `search` kind: grep is titled with its pattern, glob with its directory.
    /// Web search is an `other` call titled "… Web Search: <query>".
    fn raw_name(&self) -> String {
        if !self.tool.is_empty() {
            return self.tool.clone();
        }
        let input: Value = serde_json::from_str(&self.input).unwrap_or_default();
        if self.kind == "search" {
            let grep = input["pattern"].as_str().is_some_and(|p| p == self.title)
                || input.get("include").is_some();
            return if grep { "grep" } else { "glob" }.into();
        }
        if self.kind == "other"
            && input["query"].is_string()
            && self.title.to_lowercase().contains("web search")
        {
            return "websearch".into();
        }
        match self.kind.as_str() {
            "" | "other" if !self.title.is_empty() => self.title.clone(),
            "" => "tool".into(),
            k => k.into(),
        }
    }

    fn step(&mut self, id: &str, servers: &[String]) -> AgentEvent {
        self.stepped = true;
        AgentEvent::Step(ToolStep {
            id: id.into(),
            name: self.name(servers),
            summary: self.title.clone(),
            input: if self.input.is_empty() && !self.title.is_empty() {
                json!({"title": self.title}).to_string()
            } else {
                self.input.clone()
            },
            ..Default::default()
        })
    }

    fn result_text(&self) -> String {
        let mut parts = Vec::new();
        for c in self.content.as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("content") => {
                    if let Some(t) = c["content"]["text"].as_str() {
                        parts.push(t.to_owned());
                    }
                }
                Some("diff") => {
                    let verb = if c["oldText"].is_null() {
                        "Created"
                    } else {
                        "Edited"
                    };
                    parts.push(format!("{verb} {}", c["path"].as_str().unwrap_or_default()));
                }
                _ => {}
            }
        }
        if !parts.is_empty() {
            return parts.join("\n");
        }
        match &self.raw_output {
            Value::Null => String::new(),
            Value::Object(m) if m.is_empty() => String::new(),
            Value::String(s) => s.clone(),
            v => v["output"]
                .as_str()
                .map_or_else(|| v.to_string(), str::to_owned),
        }
    }
}

/// What one turn's `session/update`s have built up.
#[derive(Default)]
struct State {
    /// The open text (`false`) or thinking (`true`) block.
    open: Option<bool>,
    buf: String,
    last_text: String,
    tools: HashMap<String, Tool>,
    context_window: i64,
    /// This turn's MCP server names.
    servers: Vec<String>,
}

impl State {
    fn flush(&mut self) -> Vec<AgentEvent> {
        let text = std::mem::take(&mut self.buf);
        match self.open.take() {
            _ if text.is_empty() => Vec::new(),
            Some(true) => vec![AgentEvent::Thinking {
                text,
                signature: String::new(),
                uuid: String::new(),
                parent: String::new(),
            }],
            _ => {
                self.last_text.clone_from(&text);
                vec![AgentEvent::Text {
                    text,
                    uuid: String::new(),
                    parent: String::new(),
                }]
            }
        }
    }

    fn on_update(&mut self, u: &Value, partials: bool) -> Vec<AgentEvent> {
        let kind = u["sessionUpdate"].as_str().unwrap_or_default();
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                let thinking = kind == "agent_thought_chunk";
                let Some(chunk) = u["content"]["text"].as_str().filter(|t| !t.is_empty()) else {
                    return Vec::new();
                };
                let mut out = if self.open == Some(thinking) {
                    Vec::new()
                } else {
                    self.flush()
                };
                self.open = Some(thinking);
                self.buf.push_str(chunk);
                if partials {
                    let (text, parent) = (chunk.to_owned(), String::new());
                    out.push(if thinking {
                        AgentEvent::ThinkingDelta { text, parent }
                    } else {
                        AgentEvent::TextDelta { text, parent }
                    });
                }
                out
            }
            "tool_call" | "tool_call_update" => {
                let Some(id) = u["toolCallId"].as_str().filter(|s| !s.is_empty()) else {
                    return Vec::new();
                };
                self.tool_events(id, u, false)
            }
            "usage_update" => {
                let n = |k: &str| u[k].as_i64().unwrap_or_default();
                self.context_window = n("size");
                vec![AgentEvent::Usage(TokenUsage {
                    input_tokens: n("used"),
                    context_window: n("size"),
                    ..Default::default()
                })]
            }
            _ => Vec::new(),
        }
    }

    /// Merges a tool call; the Step goes out once its input is known (status
    /// past pending, or a permission request), the result once on
    /// completed/failed.
    fn tool_events(&mut self, id: &str, u: &Value, asking: bool) -> Vec<AgentEvent> {
        let t = self.tools.entry(id.to_owned()).or_default();
        t.merge(u);
        let mut evs = Vec::new();
        if !t.stepped && (asking || !matches!(t.status.as_str(), "" | "pending")) {
            evs.push(t.step(id, &self.servers));
        }
        if !t.done && matches!(t.status.as_str(), "completed" | "failed") {
            t.done = true;
            evs.push(AgentEvent::ToolResult(ToolResultEvent {
                id: id.into(),
                name: t.name(&self.servers),
                text: t.result_text(),
                is_error: t.status == "failed"
                    || t.raw_output["exit_code"].as_i64().is_some_and(|c| c != 0),
                ..Default::default()
            }));
        }
        if evs.is_empty() {
            return evs;
        }
        let mut out = self.flush();
        out.extend(evs);
        out
    }
}

/// Answers `session/request_permission`.
struct Gate {
    state: Arc<Mutex<State>>,
    emitter: Arc<Emitter>,
    asker: Option<Arc<dyn PermissionAsker>>,
    cancel: CancellationToken,
    read_only: bool,
    mode: String,
    workspace_id: String,
    agent_code: String,
}

impl Gate {
    async fn permission(&self, params: &Value) -> Value {
        let tc = &params["toolCall"];
        let id = tc["toolCallId"].as_str().unwrap_or_default();
        let (name, kind, input) = {
            let mut st = self.state.lock();
            for ev in st.tool_events(id, tc, true) {
                self.emitter.emit(ev);
            }
            let t = &st.tools[id];
            (t.name(&st.servers), t.kind.clone(), t.input.clone())
        };
        let opts = &params["options"];
        // Recur's own tools never prompt (Codex auto-approves them too).
        if name.starts_with("mcp__recur__") {
            return pick(opts, true, false);
        }
        if self.read_only && matches!(kind.as_str(), "edit" | "delete" | "move") {
            return pick(opts, false, false);
        }
        if self.mode == PERMISSION_MODE_DENY {
            return pick(opts, false, false);
        }
        let Some(asker) = self
            .asker
            .as_ref()
            .filter(|_| self.mode != PERMISSION_MODE_BYPASS)
        else {
            return pick(opts, true, true);
        };
        let d = display(&name, &input);
        let ask = asker.ask(PermissionRequest {
            workspace_id: self.workspace_id.clone(),
            agent_code: self.agent_code.clone(),
            tool: d.name.clone(),
            summary: summary(&d.name, &d.input),
            input,
            ..Default::default()
        });
        tokio::select! {
            d = ask => match d {
                Ok(d) => pick(opts, d.allow, d.remember),
                Err(_) => cancelled(),
            },
            () = self.cancel.cancelled() => cancelled(),
        }
    }
}

/// `mcp__<server>__<tool>` for an MCP call the agent names its own way:
/// Antigravity `mcp_<server>_<tool>`, OpenCode `<server>_<tool>`.
fn mcp_name(raw: &str, servers: &[String]) -> Option<String> {
    servers.iter().find_map(|s| {
        if raw.starts_with(&format!("mcp__{s}__")) {
            return Some(raw.to_owned());
        }
        let tool = raw
            .strip_prefix(&format!("mcp_{s}_"))
            .or_else(|| raw.strip_prefix(&format!("{s}_")))
            .filter(|t| !t.is_empty())?;
        Some(format!("mcp__{s}__{tool}"))
    })
}

fn cancelled() -> Value {
    json!({"outcome": {"outcome": "cancelled"}})
}

/// The option of the wanted kind, falling back within allow or reject.
fn pick(options: &Value, allow: bool, remember: bool) -> Value {
    let want: &[&str] = match (allow, remember) {
        (false, _) => &["reject_once", "reject_always"],
        (true, true) => &["allow_always", "allow_once"],
        (true, false) => &["allow_once", "allow_always"],
    };
    let opts = options.as_array().map(Vec::as_slice).unwrap_or_default();
    for w in want {
        if let Some(id) = opts
            .iter()
            .find(|o| o["kind"].as_str() == Some(w))
            .and_then(|o| o["optionId"].as_str())
        {
            return json!({"outcome": {"outcome": "selected", "optionId": id}});
        }
    }
    cancelled()
}

/// An ACP tool call in Recur's common vocabulary.
pub fn display(name: &str, input: &str) -> ToolDisplay {
    let args = match serde_json::from_str::<Value>(input) {
        Ok(Value::Object(m)) => m,
        _ => Default::default(),
    };
    let s = |keys: &[&str]| {
        keys.iter().find_map(|k| {
            args.get(*k)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
        })
    };
    let path = s(&["file_path", "filePath", "path", "Path"]);
    let canon = match name {
        "execute" | "bash" | "shell" | "run_command" => "Bash",
        "read" | "read_file" => "Read",
        "edit_file" => "Edit",
        "edit" if s(&["content"]).is_some() && s(&["old_string", "oldString"]).is_none() => "Write",
        "edit" | "move" => "Edit",
        "write" | "write_file" => "Write",
        "delete" => "Delete",
        "search" | "grep" => "Grep",
        "glob" => "Glob",
        "fetch" => "WebFetch",
        "search_web" | "websearch" => "WebSearch",
        "invoke_subagent" => "Task",
        _ => {
            return ToolDisplay {
                name: name.into(),
                input: input.into(),
            };
        }
    };
    let fields: &[(&str, Option<&str>)] = match canon {
        "Bash" => &[
            ("command", s(&["CommandLine", "command", "cmd", "title"])),
            ("description", s(&["toolSummary", "description"])),
        ],
        "Read" | "Delete" => &[("file_path", path)],
        "Edit" => &[
            ("file_path", path),
            (
                "old_string",
                s(&["old_string", "oldString", "oldText", "Target"]),
            ),
            (
                "new_string",
                s(&["new_string", "newString", "newText", "Replacement"]),
            ),
        ],
        "Write" => &[("file_path", path), ("content", s(&["content", "Content"]))],
        "Grep" => &[("pattern", s(&["pattern", "query"])), ("path", path)],
        "Glob" => &[("pattern", s(&["pattern"]))],
        "WebSearch" => &[("query", s(&["query", "Query"]))],
        "Task" => &[
            ("subagent_type", s(&["TypeName", "subagent_type"])),
            ("description", s(&["Role", "description"])),
            ("prompt", s(&["Prompt", "prompt"])),
        ],
        _ => &[("url", s(&["url"]))],
    };
    let mut out: BTreeMap<&str, Value> = fields
        .iter()
        .filter_map(|(k, v)| v.map(|v| (*k, Value::from(v))))
        .collect();
    let line = |k: &str| args.get(k).and_then(Value::as_i64).filter(|n| *n > 0);
    if canon == "Read"
        && let Some(start) = line("StartLine")
    {
        out.insert("offset", start.into());
        if let Some(end) = line("EndLine").filter(|e| *e >= start) {
            out.insert("limit", (end - start + 1).into());
        }
    }
    ToolDisplay {
        name: canon.into(),
        input: if out.is_empty() {
            input.into()
        } else {
            serde_json::to_string(&out).unwrap_or_else(|_| input.into())
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Antigravity's own tools (as its CLI declares them) in Recur's cards.
    #[test]
    fn antigravity_tools_use_common_cards() {
        let cases = [
            (
                "write_file",
                r#"{"Content":"go 1.22\n","Path":"/w/go.mod"}"#,
                "Write",
                r#"{"content":"go 1.22\n","file_path":"/w/go.mod"}"#,
            ),
            (
                "read_file",
                r#"{"Path":"/w/a.go","StartLine":10,"EndLine":19}"#,
                "Read",
                r#"{"file_path":"/w/a.go","limit":10,"offset":10}"#,
            ),
            (
                "run_command",
                r#"{"CommandLine":"go version","WaitMsBeforeAsync":5000,"toolSummary":"Check Go"}"#,
                "Bash",
                r#"{"command":"go version","description":"Check Go"}"#,
            ),
            (
                "search_web",
                r#"{"query":"tetris rotation","domain":""}"#,
                "WebSearch",
                r#"{"query":"tetris rotation"}"#,
            ),
            (
                "invoke_subagent",
                r#"{"Prompt":"review","Role":"Reviewer","TypeName":"reviewer"}"#,
                "Task",
                r#"{"description":"Reviewer","prompt":"review","subagent_type":"reviewer"}"#,
            ),
        ];
        for (name, input, want_name, want_input) in cases {
            let d = display(name, input);
            assert_eq!(
                (d.name.as_str(), d.input.as_str()),
                (want_name, want_input),
                "{name}"
            );
        }
        let raw = r#"{"Action":"list"}"#;
        assert_eq!(display("manage_task", raw).input, raw);
    }

    #[test]
    fn displays_in_common_vocabulary() {
        let d = display("execute", r#"{"command":"ls","workdir":"/w"}"#);
        assert_eq!(
            (d.name.as_str(), d.input.as_str()),
            ("Bash", r#"{"command":"ls"}"#)
        );
        let d = display("edit", r#"{"content":"bye\n","filePath":"/w/a"}"#);
        assert_eq!(
            (d.name.as_str(), d.input.as_str()),
            ("Write", r#"{"content":"bye\n","file_path":"/w/a"}"#)
        );
        let d = display(
            "edit",
            r#"{"filePath":"/w/a","oldString":"x","newString":"y"}"#,
        );
        assert_eq!(
            (d.name.as_str(), d.input.as_str()),
            (
                "Edit",
                r#"{"file_path":"/w/a","new_string":"y","old_string":"x"}"#
            )
        );
        assert_eq!(display("read", "").name, "Read");
        let d = display(
            "run_command",
            r#"{"CommandLine":"echo hi > hi.txt","WaitMsBeforeAsync":5000,"toolSummary":"s"}"#,
        );
        assert_eq!(
            (d.name.as_str(), d.input.as_str()),
            (
                "Bash",
                r#"{"command":"echo hi > hi.txt","description":"s"}"#
            )
        );
        assert_eq!(
            display("execute", r#"{"title":"ls"}"#).input,
            r#"{"command":"ls"}"#
        );
        let d = display(
            "edit_file",
            r#"{"Path":"/a","Replacement":"bye","Target":"hello"}"#,
        );
        assert_eq!(
            d.input,
            r#"{"file_path":"/a","new_string":"bye","old_string":"hello"}"#
        );
        assert_eq!(
            display("read_file", r#"{"Path":"/a"}"#).input,
            r#"{"file_path":"/a"}"#
        );
        assert_eq!(display("recur_done", "{}").input, "{}");
    }

    #[test]
    fn modes_and_options() {
        let ids = ["build".to_owned(), "plan".to_owned()];
        assert_eq!(pick_mode(true, "build", &ids).as_deref(), Some("plan"));
        assert_eq!(pick_mode(true, "plan", &ids), None);
        assert_eq!(pick_mode(false, "plan", &ids).as_deref(), Some("build"));
        assert_eq!(pick_mode(false, "build", &ids), None);
        assert_eq!(pick_mode(true, "x", &["x".into()]), None);
        let opts = json!([{"optionId": "a", "kind": "allow_once"}, {"optionId": "r", "kind": "reject_once"}]);
        assert_eq!(pick(&opts, true, true)["outcome"]["optionId"], "a");
        assert_eq!(pick(&opts, false, false)["outcome"]["optionId"], "r");
        assert_eq!(pick(&json!([]), true, true), cancelled());
    }

    #[test]
    fn prompt_framing() {
        let req = TurnRequest {
            system_prompt: "You are the Coder.".into(),
            append_system_prompt: "AGENT mode.".into(),
            turn_append_prompt: "Looking at PR #3.".into(),
            prompt: "fix it".into(),
            ..Default::default()
        };
        assert_eq!(
            prompt_text(&req, false),
            "<role-instructions>\nYou are the Coder.\n\nAGENT mode.\n</role-instructions>\n\nfix it\n\nLooking at PR #3."
        );
        assert_eq!(prompt_text(&req, true), "fix it\n\nLooking at PR #3.");
    }

    #[test]
    fn updates_to_events() {
        let mut st = State::default();
        let up = |st: &mut State, v: Value| st.on_update(&v, true);
        up(
            &mut st,
            json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hm"}}),
        );
        let evs = up(
            &mut st,
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Hi"}}),
        );
        assert_eq!(
            evs.iter().map(AgentEvent::kind).collect::<Vec<_>>(),
            ["thinking", "text_delta"]
        );
        assert!(up(&mut st, json!({"sessionUpdate": "tool_call", "toolCallId": "t", "kind": "execute", "title": "bash", "status": "pending", "rawInput": {}})).is_empty());
        let evs = up(
            &mut st,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t", "status": "in_progress", "title": "ls", "rawInput": {"command": "ls"}}),
        );
        assert_eq!(
            evs.iter().map(AgentEvent::kind).collect::<Vec<_>>(),
            ["text", "step"]
        );
        let AgentEvent::Step(s) = &evs[1] else {
            panic!()
        };
        assert_eq!(
            (s.name.as_str(), s.summary.as_str(), s.input.as_str()),
            ("execute", "ls", r#"{"command":"ls"}"#)
        );
        let evs = up(
            &mut st,
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t", "status": "completed", "title": "x", "content": [{"type": "content", "content": {"type": "text", "text": "a.txt\n"}}, {"type": "diff", "path": "/a", "oldText": null, "newText": "x"}]}),
        );
        let AgentEvent::ToolResult(r) = &evs[0] else {
            panic!()
        };
        assert_eq!(
            (r.name.as_str(), r.text.as_str(), r.is_error),
            ("execute", "a.txt\n\nCreated /a", false)
        );
        assert!(up(&mut st, json!({"sessionUpdate": "tool_call_update", "toolCallId": "t", "status": "completed"})).is_empty());
        let evs = up(
            &mut st,
            json!({"sessionUpdate": "tool_call", "toolCallId": "u", "kind": "other", "title": "recur_done", "status": "failed", "rawOutput": {"output": "nope"}}),
        );
        let AgentEvent::ToolResult(r) = &evs[1] else {
            panic!()
        };
        assert_eq!(
            (r.name.as_str(), r.text.as_str(), r.is_error),
            ("recur_done", "nope", true)
        );
        assert_eq!(st.last_text, "Hi");
    }

    /// Antigravity: a `name` field, `in_progress` first, back to `pending`
    /// while asking, no content on completion, `exit_code` in rawOutput.
    #[test]
    fn antigravity_tool_shape() {
        let mut st = State::default();
        let call = json!({"toolCallId": "c", "name": "run_command", "kind": "execute", "title": "echo hi > hi.txt",
            "rawInput": {"CommandLine": "echo hi > hi.txt", "WaitMsBeforeAsync": 5000}});
        let with = |status: &str| {
            let mut v = call.clone();
            v["sessionUpdate"] = "tool_call_update".into();
            v["status"] = status.into();
            v
        };
        let mut steps = 0;
        for status in ["in_progress", "pending", "in_progress"] {
            steps += st
                .on_update(&with(status), false)
                .iter()
                .filter(|e| e.kind() == "step")
                .count();
        }
        steps += st.tool_events("c", &with("pending"), true).len();
        assert_eq!(steps, 1);
        let mut done = with("completed");
        done["rawOutput"] = json!({"output": "boom", "exit_code": 2});
        let evs = st.on_update(&done, false);
        let AgentEvent::ToolResult(r) = &evs[0] else {
            panic!()
        };
        assert_eq!(
            (r.name.as_str(), r.text.as_str(), r.is_error),
            ("run_command", "boom", true)
        );
    }

    /// OpenCode tool calls as captured live: no tool name, only kind and title.
    #[test]
    fn opencode_calls_use_common_cards() {
        let cases = [
            (
                json!({"kind": "search", "title": "private/tmp/w", "rawInput": {"pattern": "*.txt"}}),
                "Glob",
            ),
            (
                json!({"kind": "search", "title": "bye", "rawInput": {"pattern": "bye", "path": "/w", "include": "*.txt"}}),
                "Grep",
            ),
            (
                json!({"kind": "other", "title": "Exa Web Search: opencode acp", "rawInput": {"query": "opencode acp"}}),
                "WebSearch",
            ),
            (
                json!({"kind": "edit", "title": "w/a.txt", "rawInput": {"content": "hello", "filePath": "/w/a.txt"}}),
                "Write",
            ),
            (
                json!({"kind": "execute", "title": "ls", "rawInput": {"command": "ls", "workdir": "/w"}}),
                "Bash",
            ),
            (
                json!({"kind": "fetch", "title": "https://example.com", "rawInput": {"url": "https://example.com"}}),
                "WebFetch",
            ),
        ];
        for (i, (mut call, want)) in cases.into_iter().enumerate() {
            call["sessionUpdate"] = "tool_call".into();
            call["toolCallId"] = format!("t{i}").into();
            call["status"] = "in_progress".into();
            let mut st = State::default();
            let evs = st.on_update(&call, false);
            let AgentEvent::Step(step) = &evs[0] else {
                panic!("{want}")
            };
            assert_eq!(display(&step.name, &step.input).name, want, "{call}");
        }
    }

    /// Shapes seen live: Antigravity names MCP calls `mcp_<server>_<tool>`,
    /// OpenCode titles them `<server>_<tool>` with kind `other`.
    #[test]
    fn mcp_calls_use_the_common_name() {
        let servers = vec!["recur".to_owned(), "playwright".to_owned()];
        assert_eq!(
            mcp_name("mcp_recur_report_finding", &servers).as_deref(),
            Some("mcp__recur__report_finding")
        );
        assert_eq!(
            mcp_name("playwright_browser_click", &servers).as_deref(),
            Some("mcp__playwright__browser_click")
        );
        assert_eq!(
            mcp_name("mcp__recur__done", &servers).as_deref(),
            Some("mcp__recur__done")
        );
        assert_eq!(mcp_name("run_command", &servers), None);
        assert_eq!(mcp_name("recur_", &servers), None);

        let mut st = State {
            servers,
            ..Default::default()
        };
        let evs = st.on_update(
            &json!({"sessionUpdate": "tool_call", "toolCallId": "m", "kind": "other",
                "title": "recur_report_finding", "status": "in_progress", "rawInput": {"title": "t"}}),
            false,
        );
        let AgentEvent::Step(s) = &evs[0] else {
            panic!()
        };
        assert_eq!(s.name, "mcp__recur__report_finding");
    }
}
