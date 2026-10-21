//! Recur's MCP HTTP server: hand-rolled JSON-RPC 2.0 over the MCP
//! "streamable HTTP" transport (`POST /<token>/mcp`), request/response only.
//!
//! Each agent turn gets a URL whose token is bound to a workspace and agent,
//! so a tool call is attributed by the server, never by ids in the request
//! body, and the engine can scope tool access per turn. Sequential turns of
//! one agent reuse its token so `--mcp-config` URLs stay stable; unregister
//! deactivates it (404) until the next register. An overlapping register of
//! the same agent mints a distinct token so two allowlists never collide.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::{ALLOW, CONTENT_TYPE, HeaderValue, LOCATION};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use parking_lot::{Mutex, RwLock};
use recur_agent_tools::json::{Shape, Ty, field, unmarshal, unmarshal_ignoring_errors};
use recur_agent_tools::path::{clean, errno_text};
use recur_agent_tools::{Call, Host, NoopHost};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::protocol::*;
use crate::tools::{ToolSpec, all_tools, new_token};

/// Caps one request body (a runaway client); sized for create_artifact
/// base64 images. Longer bodies are cut off, as Go's `io.LimitReader` does.
const MAX_BODY: usize = 32 << 20;
const READ_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

/// Server start/stop failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct McpError(pub String);

/// Which workspace and agent a token is attributed to, and which bare tool
/// names it may see. `allowed: None` = every tool; `Some(empty)` = none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub workspace_id: String,
    pub agent_code: String,
    /// The conductor tab for a loop specialist turn; empty = agent is the tab.
    pub chat_code: String,
    pub allowed: Option<HashSet<String>>,
    /// False after unregister until the next register.
    pub active: bool,
    /// The agent's canonical token (reused); false for an overlapping extra.
    pub stable: bool,
}

impl Binding {
    pub fn allows(&self, name: &str) -> bool {
        self.allowed.as_ref().is_none_or(|a| a.contains(name))
    }
}

#[derive(Default)]
struct Tokens {
    tokens: HashMap<String, Binding>,
    by_agent: HashMap<(String, String), String>,
}

impl Tokens {
    fn drop_token(&mut self, token: &str) {
        if let Some(b) = self.tokens.remove(token)
            && b.stable
        {
            self.by_agent.remove(&(b.workspace_id, b.agent_code));
        }
    }
}

pub(crate) struct Inner {
    host: Arc<dyn Host>,
    tools: Vec<ToolSpec>,
    port: AtomicU16,
    reg: RwLock<Tokens>,
}

struct Running {
    stop: CancellationToken,
    task: JoinHandle<()>,
}

/// Recur's MCP server. The engine owns one; providers embed
/// [`McpServer::url_for`] URLs in each turn's MCP config.
pub struct McpServer {
    inner: Arc<Inner>,
    addr: Option<String>,
    running: Mutex<Option<Running>>,
}

impl McpServer {
    /// A server backed by `host` (the engine seam). `None` uses the no-op
    /// host so the server never lacks one.
    pub fn new(host: Option<Arc<dyn Host>>) -> McpServer {
        McpServer {
            inner: Arc::new(Inner {
                host: host.unwrap_or_else(|| Arc::new(NoopHost)),
                tools: all_tools(),
                port: AtomicU16::new(0),
                reg: RwLock::new(Tokens::default()),
            }),
            addr: None,
            running: Mutex::new(None),
        }
    }

    /// Overrides the listen address (default `127.0.0.1:0`, a free port).
    pub fn with_addr(mut self, addr: impl Into<String>) -> McpServer {
        self.addr = Some(addr.into());
        self
    }

    /// Binds and serves until [`McpServer::close`]. Returns once listening, so
    /// [`McpServer::url_for`] and [`McpServer::port`] work immediately.
    pub async fn start(&self) -> Result<(), McpError> {
        let addr = self.addr.clone().unwrap_or_else(|| "127.0.0.1:0".into());
        let listener = TcpListener::bind(&addr).await.map_err(|e| {
            McpError(format!(
                "mcp listen: listen tcp {addr}: bind: {}",
                errno_text(&e)
            ))
        })?;
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        self.inner.port.store(port, Ordering::SeqCst);
        let stop = CancellationToken::new();
        let task = tokio::spawn(serve(listener, self.inner.clone(), stop.clone()));
        *self.running.lock() = Some(Running { stop, task });
        Ok(())
    }

    /// Stops accepting, closes idle connections and waits up to 3 s for
    /// in-flight requests. A timeout leaves those connections running.
    pub async fn close(&self) -> Result<(), McpError> {
        let Some(run) = self.running.lock().take() else {
            return Ok(());
        };
        run.stop.cancel();
        match tokio::time::timeout(CLOSE_TIMEOUT, run.task).await {
            Ok(_) => Ok(()),
            Err(_) => Err(McpError("context deadline exceeded".into())),
        }
    }

    /// The bound port (0 before start).
    pub fn port(&self) -> u16 {
        self.inner.port.load(Ordering::SeqCst)
    }

    /// `http://127.0.0.1:<port>/<token>/mcp`
    pub fn url_for(&self, token: &str) -> String {
        format!("http://127.0.0.1:{}/{token}/mcp", self.port())
    }

    /// [`McpServer::register_owned`] without a conductor tab.
    pub fn register(
        &self,
        workspace_id: &str,
        agent_code: &str,
        allowed: Option<&[String]>,
    ) -> (String, String) {
        self.register_owned(workspace_id, agent_code, "", allowed)
    }

    /// Returns `(token, url)` for this turn. Reuses the agent's token after
    /// unregister (replacing its allowlist); mints an extra token while the
    /// canonical one is still active. `allowed`: `None` = unrestricted,
    /// `Some(names)` = only those bare names (empty = no Recur tools; see
    /// [`crate::names_from_allowed_tools`]). `chat_code` is the turn's
    /// conductor tab, which rename_session retargets onto.
    pub fn register_owned(
        &self,
        workspace_id: &str,
        agent_code: &str,
        chat_code: &str,
        allowed: Option<&[String]>,
    ) -> (String, String) {
        let mut b = Binding {
            workspace_id: workspace_id.into(),
            agent_code: agent_code.into(),
            chat_code: chat_code.trim().into(),
            allowed: allowed.map(|a| a.iter().filter(|n| !n.is_empty()).cloned().collect()),
            active: true,
            stable: true,
        };
        let key = (workspace_id.to_string(), agent_code.to_string());
        let mut reg = self.inner.reg.write();
        if let Some(tok) = reg.by_agent.get(&key).cloned() {
            if reg.tokens.get(&tok).is_some_and(|e| e.active) {
                b.stable = false;
                let token = new_token();
                reg.tokens.insert(token.clone(), b);
                drop(reg);
                let url = self.url_for(&token);
                return (token, url);
            }
            reg.tokens.insert(tok.clone(), b);
            drop(reg);
            let url = self.url_for(&tok);
            return (tok, url);
        }
        let token = new_token();
        reg.tokens.insert(token.clone(), b);
        reg.by_agent.insert(key, token.clone());
        drop(reg);
        let url = self.url_for(&token);
        (token, url)
    }

    /// A turn ended: the canonical token deactivates (404 until the next
    /// register); an overlapping extra is revoked. Returns the token's
    /// `(workspace, agent)`, `None` for an unknown token.
    pub fn unregister(&self, token: &str) -> Option<(String, String)> {
        let mut reg = self.inner.reg.write();
        let b = reg.tokens.get(token)?.clone();
        if !b.stable {
            reg.drop_token(token);
        } else if let Some(e) = reg.tokens.get_mut(token) {
            e.active = false;
        }
        Some((b.workspace_id, b.agent_code))
    }

    /// Revokes a token so it 404s.
    pub fn drop_token(&self, token: &str) {
        self.inner.reg.write().drop_token(token);
    }

    /// Revokes every token of one agent (chat close), extras included.
    pub fn drop_agent(&self, workspace_id: &str, agent_code: &str) {
        let mut reg = self.inner.reg.write();
        let toks: Vec<String> = reg
            .tokens
            .iter()
            .filter(|(_, b)| b.workspace_id == workspace_id && b.agent_code == agent_code)
            .map(|(t, _)| t.clone())
            .collect();
        for t in toks {
            reg.drop_token(&t);
        }
    }

    /// Revokes every token of a workspace (workspace delete).
    pub fn drop_workspace(&self, workspace_id: &str) {
        let mut reg = self.inner.reg.write();
        let toks: Vec<String> = reg
            .tokens
            .iter()
            .filter(|(_, b)| b.workspace_id == workspace_id)
            .map(|(t, _)| t.clone())
            .collect();
        for t in toks {
            reg.drop_token(&t);
        }
    }

    #[cfg(test)]
    pub(crate) fn lookup(&self, token: &str) -> Option<Binding> {
        self.inner.reg.read().tokens.get(token).cloned()
    }

    #[cfg(test)]
    pub(crate) async fn dispatch(
        &self,
        cancel: &CancellationToken,
        req: &RpcRequest,
        b: &Binding,
    ) -> Option<RpcResponse> {
        self.inner.dispatch(cancel, req, b).await
    }
}

async fn serve(listener: TcpListener, inner: Arc<Inner>, stop: CancellationToken) {
    let graceful = GracefulShutdown::new();
    let mut backoff = Duration::ZERO;
    loop {
        let accepted = tokio::select! {
            _ = stop.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let stream = match accepted {
            Ok((stream, _)) => {
                backoff = Duration::ZERO;
                stream
            }
            Err(_) => {
                // Go's Serve: back off 5 ms doubling to 1 s (e.g. out of fds).
                backoff = (backoff * 2).clamp(Duration::from_millis(5), Duration::from_secs(1));
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(backoff) => continue,
                }
            }
        };
        let inner = inner.clone();
        let svc = hyper::service::service_fn(move |req| {
            let inner = inner.clone();
            async move { Ok::<_, Infallible>(inner.handle(req).await) }
        });
        let conn = hyper::server::conn::http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(READ_HEADER_TIMEOUT)
            .serve_connection(TokioIo::new(stream), svc);
        let conn = graceful.watch(conn);
        tokio::spawn(async move {
            let _ = conn.await;
        });
    }
    drop(listener);
    graceful.shutdown().await;
}

fn text_response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(format!("{body}\n"))));
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r
}

fn json_response(resp: &RpcResponse) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(resp.encode() + "\n")));
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

/// Go `http.Redirect` with 307, as ServeMux answers a non-canonical path.
fn redirect(method: &Method, url: &str) -> Response<Full<Bytes>> {
    let body = if method == Method::GET {
        format!(
            "<a href=\"{}\">Temporary Redirect</a>.\n\n",
            html_escape(url)
        )
    } else {
        String::new()
    };
    let mut r = Response::new(Full::new(Bytes::from(body)));
    *r.status_mut() = StatusCode::TEMPORARY_REDIRECT;
    if let Ok(v) = HeaderValue::from_str(url) {
        r.headers_mut().insert(LOCATION, v);
    }
    if method == Method::GET || method == Method::HEAD {
        r.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
    }
    r
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

/// Go `net/http` `cleanPath`.
fn clean_path(p: &str) -> String {
    if p.is_empty() {
        return "/".into();
    }
    let p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    };
    let mut np = clean(&p);
    if p.ends_with('/') && np != "/" {
        if p.len() == np.len() + 1 && p.starts_with(&np) {
            np = p;
        } else {
            np.push('/');
        }
    }
    np
}

/// The decoded request path (Go `URL.Path`); `None` on a bad escape.
fn unescape_path(raw: &str) -> Option<String> {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(recur_agent_tools::gostr::lossy(&out))
}

/// Go `url.URL.EscapedPath` for a cleaned path.
fn escape_path(p: &str) -> String {
    let mut out = String::new();
    for &c in p.as_bytes() {
        if c.is_ascii_alphanumeric() || b"-_.~$&+,/:;=@".contains(&c) {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// The `<token>` of `/<token>/mcp` (surrounding slashes ignored).
fn token_from_path(p: &str) -> Option<&str> {
    let parts: Vec<&str> = p.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        [token, "mcp"] if !token.is_empty() => Some(token),
        _ => None,
    }
}

static INITIALIZE_PARAMS: Shape =
    Shape::anon(&[field("protocolVersion", "ProtocolVersion", Ty::Str)]);

static CALL_TOOL_PARAMS: Shape = Shape::anon(&[
    field("name", "Name", Ty::Str),
    field("arguments", "Arguments", Ty::Raw),
]);

impl Inner {
    async fn handle(&self, req: Request<Incoming>) -> Response<Full<Bytes>> {
        let method = req.method().clone();
        let Some(path) = unescape_path(req.uri().path()) else {
            return text_response(StatusCode::BAD_REQUEST, "400 Bad Request");
        };
        if method != Method::CONNECT {
            let cleaned = clean_path(&path);
            if cleaned != path {
                let mut url = escape_path(&cleaned);
                if let Some(q) = req.uri().query() {
                    url = format!("{url}?{q}");
                }
                return redirect(&method, &url);
            }
        }
        let Some(token) = token_from_path(&path) else {
            return text_response(StatusCode::NOT_FOUND, "404 page not found");
        };
        let binding = match self.reg.read().tokens.get(token) {
            Some(b) if b.active => b.clone(),
            _ => return text_response(StatusCode::NOT_FOUND, "unknown mcp token"),
        };
        if method != Method::POST {
            // No server-initiated stream or session teardown: Recur's tools are
            // request/response only and clients fall back to plain POST.
            let mut r = text_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
            r.headers_mut()
                .insert(ALLOW, HeaderValue::from_static("POST"));
            return r;
        }
        // The request's lifetime: hyper drops this future when the client goes
        // away, which cancels a pending permission wait.
        let cancel = CancellationToken::new();
        let _guard = cancel.clone().drop_guard();
        let body = match read_body(req.into_body()).await {
            Ok(b) => b,
            Err(e) => return json_response(&new_error("", CODE_PARSE, format!("read body: {e}"))),
        };
        let Ok(rpc) = RpcRequest::parse(&body) else {
            return json_response(&new_error("", CODE_PARSE, "parse error"));
        };
        if rpc.jsonrpc != JSONRPC_VERSION {
            return json_response(&new_error(
                &rpc.id,
                CODE_INVALID_REQUEST,
                "jsonrpc must be \"2.0\"",
            ));
        }
        match self.dispatch(&cancel, &rpc, &binding).await {
            Some(resp) => json_response(&resp),
            None => {
                // JSON-RPC notifications get no body.
                let mut r = Response::new(Full::new(Bytes::new()));
                *r.status_mut() = StatusCode::ACCEPTED;
                r
            }
        }
    }

    /// Routes one JSON-RPC method. `None` = notification (no response).
    async fn dispatch(
        &self,
        cancel: &CancellationToken,
        req: &RpcRequest,
        b: &Binding,
    ) -> Option<RpcResponse> {
        match req.method.as_str() {
            "initialize" => Some(self.on_initialize(req)),
            "notifications/initialized" | "initialized" => None,
            "tools/list" => Some(self.on_list_tools(req, b)),
            "tools/call" => Some(self.on_call_tool(cancel, req, b).await),
            "ping" => Some(new_result(&req.id, RpcResult::Empty)),
            _ if req.is_notification() => None,
            m => Some(new_error(
                &req.id,
                CODE_METHOD_NOT_FOUND,
                format!("method not found: {m}"),
            )),
        }
    }

    fn on_initialize(&self, req: &RpcRequest) -> RpcResponse {
        // Missing or malformed params are tolerated.
        let p = unmarshal_ignoring_errors(&req.params, &INITIALIZE_PARAMS);
        let version = match p.str("protocolVersion") {
            "" => PROTOCOL_VERSION,
            v => v,
        };
        new_result(
            &req.id,
            RpcResult::Initialize(InitializeResult {
                protocol_version: version.into(),
                capabilities: ServerCapabilities {
                    tools: Some(ToolsCapability {
                        list_changed: false,
                    }),
                },
                server_info: Implementation {
                    name: SERVER_NAME.into(),
                    version: SERVER_VERSION.into(),
                },
                instructions: INSTRUCTIONS.into(),
            }),
        )
    }

    fn on_list_tools(&self, req: &RpcRequest, b: &Binding) -> RpcResponse {
        let tools = self
            .tools
            .iter()
            .filter(|t| b.allows(t.name))
            .map(|t| ToolEntry {
                name: t.name.into(),
                description: t.description.into(),
                input_schema: t.schema.clone(),
            })
            .collect();
        new_result(&req.id, RpcResult::ListTools(tools))
    }

    async fn on_call_tool(
        &self,
        cancel: &CancellationToken,
        req: &RpcRequest,
        b: &Binding,
    ) -> RpcResponse {
        let Ok(p) = unmarshal(&req.params, &CALL_TOOL_PARAMS) else {
            return new_error(&req.id, CODE_INVALID_PARAMS, "invalid tools/call params");
        };
        let name = p.str("name");
        let Some(spec) = self.tools.iter().find(|t| t.name == name) else {
            // An unknown tool is a protocol error, not a tool result.
            return new_error(
                &req.id,
                CODE_METHOD_NOT_FOUND,
                format!("unknown tool: {name}"),
            );
        };
        if !b.allows(name) {
            return new_result(
                &req.id,
                RpcResult::CallTool(tool_error(format!(
                    "tool not available on this agent: {name}"
                ))),
            );
        }
        let args = p.str("arguments");
        // Backfill transcript tool_use args before the handler runs (Codex item
        // frames often arrive as {}).
        self.host
            .note_tool_args(&b.workspace_id, &b.agent_code, name, args);
        let result = spec
            .tool
            .execute(Call {
                cancel: cancel.clone(),
                workspace_id: b.workspace_id.clone(),
                agent_code: b.agent_code.clone(),
                chat_code: b.chat_code.clone(),
                args: args.into(),
                host: Some(self.host.clone()),
                ..Default::default()
            })
            .await;
        // Bad arguments are a tool-level error the model can self-correct from.
        let out = match result {
            Ok(text) => tool_text(text),
            Err(e) => tool_error(e.0),
        };
        new_result(&req.id, RpcResult::CallTool(out))
    }
}

/// Reads up to `MAX_BODY` bytes; the rest is left unread like `io.LimitReader`.
async fn read_body(mut body: Incoming) -> Result<String, String> {
    let mut buf: Vec<u8> = Vec::new();
    while buf.len() < MAX_BODY {
        match body.frame().await {
            None => break,
            Some(Err(e)) => return Err(e.to_string()),
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    let room = MAX_BODY - buf.len();
                    buf.extend_from_slice(&data[..data.len().min(room)]);
                }
            }
        }
    }
    Ok(recur_agent_tools::gostr::lossy(&buf))
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
