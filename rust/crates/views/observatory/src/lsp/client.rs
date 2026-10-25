//! The editor's language server client (`lib/lsp.ts` over `@codemirror/lsp-client` 6.2):
//! [`Client`] implements `recur_editor::lsp::LspClient` on `LspService::write_lsp` and the
//! `lsp.message` / `lsp.exit` events. It owns request ids (12s timeout), the `initialize`
//! exchange, the workspace's documents and versions (`RecurWorkspace`), `didOpen` /
//! `didChange` batching through `content_changes_for`, server requests the client answers
//! itself, `window/showMessage` toasts and `publishDiagnostics` into [`LspProblems`].
//!
//! [`attach_lsp`] is `attachLsp`: `EnsureLSP`, one client per session (the TypeScript server
//! first so Vue can pair with it), and the in-flight / deferred close bookkeeping so a stale
//! Ensure never closes a session another open still wants.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::{Rc, Weak};
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::future::{LocalBoxFuture, Shared};
use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::{App, AsyncApp, BackgroundExecutor, Global, Task};
use recur_editor::change::{Change, ChangeSet};
use recur_editor::lsp::{LspClient, LspError, LspFuture, LspJump, LspRequest, content_changes_for};
use recur_editor::text::Text;
use recur_lsp::protocol::{
    ServerRequest, handle_server_request, rewrite_outgoing, tsserver_body, vue_tsserver_response,
};
use recur_lsp::service::EnsureResult;
use recur_services::{AppEvent, Services};
use recur_ui::widgets::{ToastKind, ToastOptions, push_toast};
use recur_view_core::AppState;
use serde_json::{Map, Value, json};

use super::lang::lsp_language_id_for_path;
use super::problems::{LspProblem, LspProblems, ProblemCode, ProblemSeverity, update_problems};

/// `timeout: 12_000`.
pub const REQUEST_TIMEOUT_MS: u64 = 12_000;
/// `autoSyncClients`: secondary servers sync this long after the last edit.
pub const AUTO_SYNC_MS: u64 = 500;
pub const TYPESCRIPT_SERVER: &str = "typescript-language-server";

fn err(message: impl Into<String>) -> LspError {
    LspError {
        code: None,
        message: message.into(),
    }
}

// ---- URIs ----

fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `decodeURIComponent`; `None` on a malformed escape or invalid UTF-8.
fn decode_uri_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `joinRootUri`.
pub fn join_root_uri(root_uri: &str, rel: &str) -> String {
    let root = if root_uri.ends_with('/') {
        root_uri.to_string()
    } else {
        format!("{root_uri}/")
    };
    let parts: Vec<String> = rel.split('/').map(encode_uri_component).collect();
    root + &parts.join("/")
}

/// `uriToRelPath`.
pub fn uri_to_rel_path(root_uri: &str, file_uri: &str) -> String {
    let root = if root_uri.ends_with('/') {
        root_uri.to_string()
    } else {
        format!("{root_uri}/")
    };
    if let Some(rest) = file_uri.strip_prefix(&root) {
        return decode_uri_component(rest).unwrap_or_else(|| rest.to_string());
    }
    if let (Ok(u), Ok(r)) = (url::Url::parse(file_uri), url::Url::parse(root_uri))
        && let Some(rest) = u.path().strip_prefix(r.path())
        && let Some(d) = decode_uri_component(rest.trim_start_matches('/'))
    {
        return d;
    }
    file_uri.to_string()
}

// ---- capabilities ----

/// `mergeCapabilities`.
fn merge_capabilities(base: &Value, add: &Value) -> Value {
    match (base, add) {
        (_, Value::Null) => base.clone(),
        (Value::Object(b), Value::Object(a)) => {
            let mut out = Map::new();
            for (k, v) in b {
                out.insert(
                    k.clone(),
                    a.get(k)
                        .map_or_else(|| v.clone(), |av| merge_capabilities(v, av)),
                );
            }
            for (k, v) in a {
                if !b.contains_key(k) {
                    out.insert(k.clone(), v.clone());
                }
            }
            Value::Object(out)
        }
        _ => add.clone(),
    }
}

/// The client capabilities `initialize` sends: lsp-client's defaults, then `codeActionCaps`
/// and `workspaceRequestCaps`.
pub fn client_capabilities() -> Value {
    let base = json!({
        "general": { "markdown": { "parser": "marked" } },
        "textDocument": {
            "completion": {
                "completionItem": {
                    "snippetSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "insertReplaceSupport": false,
                },
                "completionList": { "itemDefaults": ["commitCharacters", "editRange", "insertTextFormat"] },
                "completionItemKind": { "valueSet": [] },
                "contextSupport": true,
            },
            "hover": { "contentFormat": ["markdown", "plaintext"] },
            "formatting": {},
            "rename": {},
            "signatureHelp": {
                "contextSupport": true,
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true },
                    "activeParameterSupport": true,
                },
            },
            "definition": {},
            "declaration": {},
            "implementation": {},
            "typeDefinition": {},
            "references": {},
            "diagnostic": {},
        },
        "window": { "showMessage": {} },
    });
    let code_actions = json!({
        "textDocument": {
            "codeAction": {
                "codeActionLiteralSupport": {
                    "codeActionKind": { "valueSet": ["quickfix", "quickfix.export", "", "source.organizeImports"] },
                },
                "isPreferredSupport": true,
                "dataSupport": true,
                "resolveSupport": { "properties": ["edit"] },
            },
            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
            "publishDiagnostics": { "versionSupport": true },
        },
    });
    let workspace = json!({ "workspace": { "configuration": true, "workspaceFolders": true } });
    merge_capabilities(&merge_capabilities(&base, &code_actions), &workspace)
}

// ---- the workspace's documents (`RecurWorkspace`) ----

/// One open document (`RecurWorkspaceFile`). `live` is the attached editor's text (its
/// `getView()`), `None` once detached.
#[derive(Clone, Debug)]
pub struct WsFile {
    pub uri: String,
    pub language_id: String,
    pub version: i64,
    pub doc: Text,
    pub live: Option<Text>,
    /// Bumped on every edit of the attached editor (the identity `view.state.doc` had).
    pub edits: u64,
}

/// A change set replacing all of `prev` with `next` (`ChangeSet.of({from: 0, to, insert})`).
fn full_replace(prev: &Text, next: &str) -> ChangeSet {
    ChangeSet::of(&[Change::new(0, prev.len(), next.to_string())], prev.len())
}

enum Init {
    Pending(Vec<oneshot::Sender<Result<(), LspError>>>),
    Ready,
    Failed(LspError),
}

struct ClientState {
    next_id: u64,
    pending: HashMap<u64, oneshot::Sender<Result<Value, LspError>>>,
    caps: Option<Value>,
    /// `supportSync`: -1 before `initialize` answers.
    support_sync: i64,
    init: Init,
    connected: bool,
    /// Messages waiting on `initializing`.
    queued: Vec<String>,
    files: Vec<WsFile>,
    versions: HashMap<String, i64>,
    detached: Vec<(String, Text, ChangeSet)>,
}

/// Where the client's messages go: the session's ordered `WriteLSP` pump, or a test sink.
pub type Sink = Rc<dyn Fn(String)>;

/// One language server session's client (`LSPClient` with `RecurWorkspace`).
pub struct Client {
    pub session_id: String,
    pub workspace_id: String,
    pub root_uri: String,
    pub server_key: String,
    sink: Sink,
    executor: BackgroundExecutor,
    app: Option<AsyncApp>,
    hub: RefCell<Weak<RefCell<HubState>>>,
    st: Rc<RefCell<ClientState>>,
    me: Weak<Client>,
}

impl Client {
    pub fn new(
        session_id: &str,
        workspace_id: &str,
        root_uri: &str,
        server_key: &str,
        sink: Sink,
        executor: BackgroundExecutor,
        app: Option<AsyncApp>,
    ) -> Rc<Client> {
        Rc::new_cyclic(|me| Client {
            me: me.clone(),
            session_id: session_id.into(),
            workspace_id: workspace_id.into(),
            root_uri: root_uri.into(),
            server_key: server_key.into(),
            sink,
            executor,
            app,
            hub: RefCell::new(Weak::new()),
            st: Rc::new(RefCell::new(ClientState {
                next_id: 0,
                pending: HashMap::new(),
                caps: None,
                support_sync: -1,
                init: Init::Pending(Vec::new()),
                connected: false,
                queued: Vec::new(),
                files: Vec::new(),
                versions: HashMap::new(),
                detached: Vec::new(),
            })),
        })
    }

    fn set_hub(&self, hub: &LspHub) {
        *self.hub.borrow_mut() = Rc::downgrade(&hub.0);
    }

    /// `connected`: a transport is attached.
    pub fn connected(&self) -> bool {
        self.st.borrow().connected
    }

    /// `serverCapabilities` (also cleared by `disconnect`).
    pub fn capabilities(&self) -> Option<Value> {
        self.st.borrow().caps.clone()
    }

    fn has_provider(&self, key: &str) -> bool {
        self.st
            .borrow()
            .caps
            .as_ref()
            .and_then(|c| c.get(key))
            .is_some_and(truthy)
    }

    /// `lspHasDocumentSymbol`.
    pub fn has_document_symbol(&self) -> bool {
        self.has_provider("documentSymbolProvider")
    }

    /// `connect`: start `initialize` and resolve [`Self::initializing`] with its answer.
    pub fn connect(self: &Rc<Self>) -> LspFuture<()> {
        self.st.borrow_mut().connected = true;
        let req = self.request_inner(
            "initialize",
            json!({
                "processId": null,
                "clientInfo": { "name": "@codemirror/lsp-client" },
                "rootUri": if self.root_uri.is_empty() { Value::Null } else { json!(self.root_uri) },
                "capabilities": client_capabilities(),
            }),
            true,
        );
        let this = Rc::downgrade(self);
        async move {
            let resp = req.response.await;
            let Some(this) = this.upgrade() else {
                return Err(err("Client dropped"));
            };
            match resp {
                Ok(resp) => {
                    let caps = resp.get("capabilities").cloned().unwrap_or(Value::Null);
                    let sync = caps.get("textDocumentSync");
                    let support = match sync {
                        None | Some(Value::Null) => 0,
                        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
                        Some(o) => o.get("change").and_then(Value::as_i64).unwrap_or(0),
                    };
                    let (waiters, queued) = {
                        let mut st = this.st.borrow_mut();
                        st.caps = Some(caps);
                        st.support_sync = support;
                        let waiters = match std::mem::replace(&mut st.init, Init::Ready) {
                            Init::Pending(w) => w,
                            _ => Vec::new(),
                        };
                        (waiters, std::mem::take(&mut st.queued))
                    };
                    (this.sink)(
                        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})
                            .to_string(),
                    );
                    for m in queued {
                        (this.sink)(m);
                    }
                    for w in waiters {
                        let _ = w.send(Ok(()));
                    }
                    Ok(())
                }
                Err(e) => {
                    let waiters = {
                        let mut st = this.st.borrow_mut();
                        st.queued.clear();
                        match std::mem::replace(&mut st.init, Init::Failed(e.clone())) {
                            Init::Pending(w) => w,
                            _ => Vec::new(),
                        }
                    };
                    for w in waiters {
                        let _ = w.send(Err(e.clone()));
                    }
                    Err(e)
                }
            }
        }
        .boxed_local()
    }

    /// `initializing`.
    pub fn initializing(&self) -> LspFuture<()> {
        let mut st = self.st.borrow_mut();
        match &mut st.init {
            Init::Ready => async { Ok(()) }.boxed_local(),
            Init::Failed(e) => {
                let e = e.clone();
                async move { Err(e) }.boxed_local()
            }
            Init::Pending(w) => {
                let (tx, rx) = oneshot::channel();
                w.push(tx);
                async move { rx.await.unwrap_or_else(|_| Err(err("Client disconnected"))) }
                    .boxed_local()
            }
        }
    }

    /// `disconnect`: capabilities clear and requests wait on a fresh `initializing`.
    pub fn disconnect(&self) {
        let mut st = self.st.borrow_mut();
        st.caps = None;
        st.init = Init::Pending(Vec::new());
        st.queued.clear();
        st.files.clear();
    }

    fn send_after_init(&self, msg: String) {
        let ready = {
            let mut st = self.st.borrow_mut();
            if !st.connected {
                return;
            }
            match st.init {
                Init::Ready => true,
                Init::Pending(_) => {
                    st.queued.push(msg.clone());
                    false
                }
                Init::Failed(_) => false,
            }
        };
        if ready {
            (self.sink)(msg);
        }
    }

    /// `notification`.
    pub fn notification(&self, method: &str, params: Value) {
        self.send_after_init(
            json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string(),
        );
    }

    fn request_inner(&self, method: &str, params: Value, immediate: bool) -> LspRequest {
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut st = self.st.borrow_mut();
            st.next_id += 1;
            let id = st.next_id;
            st.pending.insert(id, tx);
            id
        };
        let msg =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        let init = if immediate {
            (self.sink)(msg);
            None
        } else {
            self.send_after_init(msg);
            Some(self.initializing())
        };
        let timer = self.executor.clone();
        let pending_owner = Rc::downgrade(&self.st);
        let response = async move {
            if let Some(init) = init {
                init.await?;
            }
            let timeout = timer.timer(Duration::from_millis(REQUEST_TIMEOUT_MS));
            futures::select_biased! {
                r = rx.fuse() => r.unwrap_or_else(|_| Err(err("Client disconnected"))),
                _ = timeout.fuse() => {
                    if let Some(c) = pending_owner.upgrade() {
                        c.borrow_mut().pending.remove(&id);
                    }
                    Err(err("Request timed out"))
                }
            }
        }
        .boxed_local();
        LspRequest { id, response }
    }

    /// `receiveMessage`: a response, notification or server request.
    pub fn receive_message(self: &Rc<Self>, raw: &str, cx: &mut App) {
        let Ok(Value::Object(msg)) = serde_json::from_str::<Value>(raw) else {
            return;
        };
        let has_id = msg.contains_key("id");
        let method = msg.get("method").and_then(Value::as_str);
        match (has_id, method) {
            (true, None) => {
                let Some(id) = msg.get("id").and_then(Value::as_u64) else {
                    return;
                };
                let Some(tx) = self.st.borrow_mut().pending.remove(&id) else {
                    tracing::warn!("[lsp] Received a response for non-existent request {id}");
                    return;
                };
                let result = match msg.get("error") {
                    Some(e) if !e.is_null() => Err(LspError {
                        code: e.get("code").and_then(Value::as_i64),
                        message: e
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    }),
                    _ => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(result);
            }
            (false, Some(method)) => {
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                self.handle_notification(method, &params, cx);
            }
            (true, Some(_)) => {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                (self.sink)(
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "Method not implemented" },
                    })
                    .to_string(),
                );
            }
            (false, None) => {}
        }
    }

    fn handle_notification(self: &Rc<Self>, method: &str, params: &Value, cx: &mut App) {
        match method {
            "textDocument/publishDiagnostics" => self.publish_diagnostics(params, cx),
            "window/showMessage" => {
                let kind = params.get("type").and_then(Value::as_i64).unwrap_or(3);
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if message.is_empty() || kind > 3 {
                    return;
                }
                let server = if self.server_key.is_empty() {
                    "LSP"
                } else {
                    &self.server_key
                };
                push_toast(
                    if kind == 1 {
                        ToastKind::Error
                    } else {
                        ToastKind::Info
                    },
                    format!("{server}: {message}"),
                    ToastOptions::default(),
                    cx,
                );
            }
            "window/logMessage" => {
                let message = params.get("message").and_then(Value::as_str).unwrap_or("");
                match params.get("type").and_then(Value::as_i64) {
                    Some(1) => tracing::error!("[lsp] {message}"),
                    Some(2) => tracing::warn!("[lsp] {message}"),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// `problemsCapturer`: diagnostics for the current version into the Problems store.
    fn publish_diagnostics(self: &Rc<Self>, params: &Value, cx: &mut App) {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if uri.is_empty() {
            return;
        }
        if let Some(v) = params.get("version").filter(|v| !v.is_null())
            && let Some(f) = self.file(&uri)
            && v.as_i64() != Some(f.version)
        {
            return;
        }
        let in_session = self.hub_session().is_some();
        let path = if self.root_uri.is_empty() {
            uri.clone()
        } else {
            uri_to_rel_path(&self.root_uri, &uri)
        };
        let n = |v: Option<&Value>| v.and_then(Value::as_i64).unwrap_or(0);
        let problems: Vec<LspProblem> = params
            .get("diagnostics")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|d| {
                let range = d.get("range");
                let start = range.and_then(|r| r.get("start"));
                let end = range.and_then(|r| r.get("end"));
                LspProblem {
                    uri: uri.clone(),
                    path: path.clone(),
                    severity: ProblemSeverity::from_lsp(d.get("severity").and_then(Value::as_i64)),
                    message: match d.get("message") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Null) | None => String::new(),
                        Some(other) => other.to_string(),
                    },
                    line: n(start.and_then(|p| p.get("line"))) + 1,
                    character: n(start.and_then(|p| p.get("character"))),
                    end_line: n(end.and_then(|p| p.get("line"))) + 1,
                    end_character: n(end.and_then(|p| p.get("character"))),
                    source: d.get("source").and_then(Value::as_str).map(str::to_string),
                    code: d.get("code").and_then(ProblemCode::from_json),
                    session_id: in_session.then(|| self.session_id.clone()),
                    workspace_id: in_session.then(|| self.workspace_id.clone()),
                }
            })
            .collect();
        let key = if in_session {
            if let Some(s) = self.hub_session() {
                let mut d = s.diag_uris.borrow_mut();
                if problems.is_empty() {
                    d.remove(&uri);
                } else {
                    d.insert(uri.clone());
                }
            }
            self.session_id.clone()
        } else {
            String::new()
        };
        update_problems(cx, |p| p.set_for_uri(&uri, problems, &key));
    }

    fn hub_session(self: &Rc<Self>) -> Option<Rc<Session>> {
        let hub = self.hub.borrow().upgrade()?;
        let s = hub.borrow().sessions.get(&self.session_id).cloned()?;
        Rc::ptr_eq(&s.client, self).then_some(s)
    }

    // ---- workspace ----

    /// `workspace.getFile`.
    pub fn file(&self, uri: &str) -> Option<WsFile> {
        self.st
            .borrow()
            .files
            .iter()
            .find(|f| f.uri == uri)
            .cloned()
    }

    pub fn files(&self) -> Vec<WsFile> {
        self.st.borrow().files.clone()
    }

    fn next_version(st: &mut ClientState, uri: &str) -> i64 {
        let v = st.versions.entry(uri.to_string()).or_insert(-1);
        *v += 1;
        *v
    }

    fn did_open(&self, f: &WsFile) {
        self.notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": f.uri,
                    "languageId": f.language_id,
                    "text": f.doc.to_string(),
                    "version": f.version,
                },
            }),
        );
    }

    /// `syncFiles`: attached editors whose text moved, then detached replacements.
    pub fn sync_files(&self) -> Vec<(WsFile, Text, ChangeSet)> {
        let mut st = self.st.borrow_mut();
        let mut out = Vec::new();
        for i in 0..st.files.len() {
            let Some(live) = st.files[i].live.clone() else {
                continue;
            };
            if live == st.files[i].doc {
                continue;
            }
            let prev = st.files[i].doc.clone();
            let changes = full_replace(&prev, &live.to_string());
            let uri = st.files[i].uri.clone();
            let v = Self::next_version(&mut st, &uri);
            let f = &mut st.files[i];
            f.doc = live;
            f.version = v;
            out.push((f.clone(), prev, changes));
        }
        for (uri, prev, changes) in std::mem::take(&mut st.detached) {
            if let Some(f) = st.files.iter().find(|f| f.uri == uri) {
                out.push((f.clone(), prev, changes));
            }
        }
        out
    }

    /// `replaceDetachedDoc`.
    pub fn replace_detached_doc(&self, uri: &str, next: &str) {
        let mut st = self.st.borrow_mut();
        let Some(i) = st.files.iter().position(|f| f.uri == uri) else {
            return;
        };
        if st.files[i].live.is_some() {
            return;
        }
        let new_doc = Text::of(next);
        if new_doc == st.files[i].doc {
            return;
        }
        let prev = st.files[i].doc.clone();
        let changes = full_replace(&prev, next);
        st.detached.push((uri.to_string(), prev, changes));
        let v = Self::next_version(&mut st, uri);
        st.files[i].doc = new_doc;
        st.files[i].version = v;
    }

    /// `requestFile`: open a file no editor shows, from unsaved buffers or disk.
    pub async fn request_file(self: &Rc<Self>, uri: &str) -> Option<WsFile> {
        if let Some(f) = self.file(uri) {
            return Some(f);
        }
        let io = self.file_io()?;
        let rel = uri_to_rel_path(&self.root_uri, uri);
        let app = self.app.clone()?;
        let unsaved = app.update(|cx| io.unsaved(&rel, cx));
        let text = match unsaved {
            Some(t) => t,
            None => app.update(|cx| io.read(&rel, cx)).await?,
        };
        if let Some(f) = self.file(uri) {
            return Some(f);
        }
        let mut language_id = lsp_language_id_for_path(&rel, None);
        if language_id.is_empty() {
            language_id = "plaintext".into();
        }
        let f = {
            let mut st = self.st.borrow_mut();
            let version = Self::next_version(&mut st, uri);
            let f = WsFile {
                uri: uri.to_string(),
                language_id,
                version,
                doc: Text::of(&text),
                live: None,
                edits: 0,
            };
            st.files.push(f.clone());
            f
        };
        self.did_open(&f);
        Some(f)
    }

    fn file_io(&self) -> Option<Rc<dyn LspFileIo>> {
        let hub = self.hub.borrow().upgrade()?;
        hub.borrow().file_io.get(&self.workspace_id).cloned()
    }

    /// A secondary server's view of an editor it does not drive (`secondaryDocSync`): the
    /// host reports the editor's whole text.
    pub fn text_changed(&self, uri: &str, text: &str) {
        let mut st = self.st.borrow_mut();
        if let Some(f) = st.files.iter_mut().find(|f| f.uri == uri)
            && f.live.is_some()
        {
            f.live = Some(Text::of(text));
            f.edits += 1;
        }
    }

    /// `RecurWorkspaceFile.attachView`: an editor shows the file again (no flush).
    pub fn attach_view(&self, uri: &str, text: &Text) {
        if let Some(f) = self.st.borrow_mut().files.iter_mut().find(|f| f.uri == uri) {
            f.live = Some(text.clone());
        }
    }

    /// `RecurWorkspaceFile.detachView` (no flush).
    pub fn detach_view(&self, uri: &str) {
        if let Some(f) = self.st.borrow_mut().files.iter_mut().find(|f| f.uri == uri) {
            f.live = None;
        }
    }

    /// A connected, initialized client with these capabilities (tests stub the server).
    pub fn mark_ready(&self, caps: Value) {
        let mut st = self.st.borrow_mut();
        st.connected = true;
        st.support_sync = match caps.get("textDocumentSync") {
            None | Some(Value::Null) => 1,
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
            Some(o) => o.get("change").and_then(Value::as_i64).unwrap_or(1),
        };
        st.caps = Some(caps);
        st.init = Init::Ready;
    }

    /// `snapshotLspDocs`.
    pub fn snapshot_docs(&self) -> HashMap<String, String> {
        self.st
            .borrow()
            .files
            .iter()
            .map(|f| (f.uri.clone(), f.live.as_ref().unwrap_or(&f.doc).to_string()))
            .collect()
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

impl LspClient for Client {
    fn has_capability(&self, name: &str) -> Option<bool> {
        self.st
            .borrow()
            .caps
            .as_ref()
            .map(|c| c.get(name).is_some_and(truthy))
    }

    fn server_capabilities(&self) -> Option<Value> {
        self.capabilities()
    }

    fn request(&self, method: &str, params: Value) -> LspRequest {
        if !self.connected() {
            return LspRequest {
                id: 0,
                response: async { Err(err("Client not connected")) }.boxed_local(),
            };
        }
        self.request_inner(method, params, false)
    }

    fn cancel_request(&self, id: u64) {
        if self.st.borrow().pending.contains_key(&id) {
            self.notification("$/cancelRequest", json!({ "id": id }));
        }
    }

    fn sync(&self) {
        let support = self.st.borrow().support_sync;
        for (file, prev, changes) in self.sync_files() {
            if support != 0 {
                self.notification(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": file.uri, "version": file.version },
                        "contentChanges": content_changes_for(&file.doc, &prev, &changes, support == 2),
                    }),
                );
            }
        }
    }

    fn open_file(&self, uri: &str, language_id: &str, text: &Text) {
        let existing = {
            let mut st = self.st.borrow_mut();
            match st.files.iter_mut().find(|f| f.uri == uri) {
                Some(f) => {
                    f.live = Some(text.clone());
                    true
                }
                None => false,
            }
        };
        if existing {
            self.sync();
            return;
        }
        let f = {
            let mut st = self.st.borrow_mut();
            let version = Self::next_version(&mut st, uri);
            let f = WsFile {
                uri: uri.to_string(),
                language_id: language_id.to_string(),
                version,
                doc: text.clone(),
                live: Some(text.clone()),
                edits: 0,
            };
            st.files.push(f.clone());
            f
        };
        self.did_open(&f);
    }

    fn close_file(&self, uri: &str) {
        if self.file(uri).is_none() {
            return;
        }
        self.sync();
        if let Some(f) = self.st.borrow_mut().files.iter_mut().find(|f| f.uri == uri) {
            f.live = None;
        }
    }

    fn file_changed(&self, uri: &str, _prev: &Text, _changes: &ChangeSet, next: &Text) {
        if let Some(f) = self.st.borrow_mut().files.iter_mut().find(|f| f.uri == uri)
            && f.live.is_some()
        {
            f.live = Some(next.clone());
            f.edits += 1;
        }
    }

    fn display_file(&self, uri: &str, target: LspJump) -> LspFuture<bool> {
        let show = self
            .hub
            .borrow()
            .upgrade()
            .and_then(|h| h.borrow().display.get(&self.workspace_id).cloned());
        let (Some(show), Some(app)) = (show, self.app.clone()) else {
            return async { Ok(false) }.boxed_local();
        };
        let rel = uri_to_rel_path(&self.root_uri, uri);
        async move { Ok(app.update(|cx| show(rel, target, cx)).await) }.boxed_local()
    }

    fn apply_workspace_edit(&self, edit: Value) -> LspFuture<bool> {
        let Some(me) = self.me.upgrade() else {
            return async { Ok(false) }.boxed_local();
        };
        async move { Ok(apply_workspace_edit(&me, &edit, None).await) }.boxed_local()
    }
}

// ---- the host's file access (`setLspFileIO`, `setLspDisplayFile`) ----

/// `LspFileIO`: the Observatory's buffers and disk for files no editor shows.
pub trait LspFileIo {
    /// The unsaved text of an open tab (or the mounted editors).
    fn unsaved(&self, rel: &str, cx: &mut App) -> Option<String>;
    fn read(&self, rel: &str, cx: &mut App) -> LocalBoxFuture<'static, Option<String>>;
    fn write(&self, rel: &str, text: String, cx: &mut App) -> LocalBoxFuture<'static, ()>;
    /// Replace a mounted editor's text (`lsp.fix`); false when no editor shows `rel`.
    fn apply_if_open(&self, rel: &str, text: &str, cx: &mut App) -> bool;
}

/// Open `rel` and move the caret to the jump target (`displayFile`).
pub type DisplayFileFn = Rc<dyn Fn(String, LspJump, &mut App) -> LocalBoxFuture<'static, bool>>;

// ---- sessions ----

/// One server session (`SessionSlot`).
pub struct Session {
    pub id: String,
    pub workspace_id: String,
    pub root_uri: String,
    pub root_path: String,
    pub server_key: String,
    pub vue_plugin_path: String,
    pub client: Rc<Client>,
    /// URIs that have published diagnostics for this session.
    pub diag_uris: RefCell<BTreeSet<String>>,
    /// `attach_lsp` handed this session to an editor.
    pub kept: Cell<bool>,
    _pump: Option<Task<()>>,
}

type Creating = Shared<LocalBoxFuture<'static, Option<Rc<Client>>>>;

#[derive(Default)]
pub struct HubState {
    sessions: HashMap<String, Rc<Session>>,
    ensure_in_flight: HashMap<String, usize>,
    /// simplify: workspace-scoped because the server is unknown until Ensure returns; a
    /// same-workspace Ensure for a different server delays CloseLSP until it resolves.
    /// Upgrade: language→serverKey map, then key pending by workspace+server.
    pending_ensure: HashMap<String, usize>,
    /// Abandoned session id → workspace, held until in-flight Ensures there resolve.
    deferred_close: Vec<(String, String)>,
    creating: HashMap<String, Creating>,
    display: HashMap<String, DisplayFileFn>,
    file_io: HashMap<String, Rc<dyn LspFileIo>>,
    services: Option<Services>,
    events: Option<Task<()>>,
}

/// The app's language server sessions (`lsp.ts` module state).
#[derive(Clone, Default)]
pub struct LspHub(pub Rc<RefCell<HubState>>);

impl Global for LspHub {}

pub fn init(cx: &mut App) {
    cx.default_global::<LspHub>();
}

/// The session info `attach_lsp` creates clients from.
#[derive(Clone, Debug, Default)]
pub struct ClientInfo {
    pub id: String,
    pub workspace_id: String,
    pub root_uri: String,
    pub root_path: String,
    pub server_key: String,
    pub vue_plugin_path: String,
}

/// The attachment of the open file (`LspAttach`).
#[derive(Clone)]
pub struct LspAttach {
    pub client: Rc<Client>,
    /// Every client that syncs the file: the primary first, then the secondaries.
    pub clients: Vec<Rc<Client>>,
    pub file_uri: String,
    pub language_id: String,
    pub session_id: String,
    pub root_uri: String,
    pub server_key: String,
}

impl LspHub {
    pub fn global(cx: &mut App) -> LspHub {
        cx.default_global::<LspHub>().clone()
    }

    /// `wireLspEvents`: route `lsp.message` / `lsp.exit` once.
    pub fn wire(&self, services: &Services, cx: &mut App) {
        if self.0.borrow().events.is_some() {
            return;
        }
        self.0.borrow_mut().services = Some(services.clone());
        let mut rx = services.subscribe();
        let hub = self.clone();
        let task = cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(ev) = rx.recv().await {
                match ev {
                    AppEvent::LspMessage(m) if !m.id.is_empty() => {
                        cx.update(|cx| hub.dispatch(&m.id, &m.message, cx));
                    }
                    AppEvent::LspExit(e) if !e.id.is_empty() => {
                        cx.update(|cx| hub.drop_session(&e.id, cx));
                    }
                    _ => {}
                }
            }
        });
        self.0.borrow_mut().events = Some(task);
    }

    fn services(&self) -> Option<Services> {
        self.0.borrow().services.clone()
    }

    pub fn session(&self, id: &str) -> Option<Rc<Session>> {
        self.0.borrow().sessions.get(id).cloned()
    }

    pub fn sessions(&self) -> Vec<Rc<Session>> {
        self.0.borrow().sessions.values().cloned().collect()
    }

    /// One `lsp.message`: answer what the client answers itself, else deliver it.
    pub fn dispatch(&self, id: &str, raw: &str, cx: &mut App) {
        let Some(s) = self.session(id) else { return };
        match handle_server_request(raw, &s.root_uri) {
            ServerRequest::Pass => s.client.receive_message(raw, cx),
            ServerRequest::Reply(r) => (s.client.sink)(r),
            ServerRequest::Tsserver { reply, call } => {
                (s.client.sink)(reply);
                let Some(call) = call else { return };
                let ts = self.ts_client_for_workspace(&s.workspace_id);
                let vue = s.client.clone();
                cx.spawn(async move |_| {
                    let mut body = Value::Null;
                    if let Some(ts) = ts
                        && ts.initializing().await.is_ok()
                        && let Ok(r) = ts
                            .request(
                                "workspace/executeCommand",
                                json!({
                                    "command": "typescript.tsserverRequest",
                                    "arguments": [call.command, call.payload],
                                }),
                            )
                            .response
                            .await
                    {
                        body = if r.is_object() { tsserver_body(&r) } else { r };
                    }
                    (vue.sink)(vue_tsserver_response(&call.id, Some(&body)));
                })
                .detach();
            }
        }
    }

    fn ts_client_for_workspace(&self, workspace_id: &str) -> Option<Rc<Client>> {
        self.0
            .borrow()
            .sessions
            .values()
            .find(|s| s.workspace_id == workspace_id && s.server_key == TYPESCRIPT_SERVER)
            .map(|s| s.client.clone())
    }

    /// `dropSession`.
    pub fn drop_session(&self, id: &str, cx: &mut App) {
        let slot = {
            let mut st = self.0.borrow_mut();
            st.deferred_close.retain(|(d, _)| d != id);
            st.sessions.remove(id)
        };
        let Some(slot) = slot else { return };
        slot.client.disconnect();
        let uris: Vec<String> = slot.diag_uris.borrow().iter().cloned().collect();
        update_problems(cx, |p| {
            for uri in &uris {
                p.set_for_uri(uri, Vec::new(), &slot.id);
            }
        });
    }

    fn close_lsp(&self, id: &str, cx: &mut App) {
        self.drop_session(id, cx);
        if let Some(services) = self.services() {
            let id = id.to_string();
            cx.background_executor()
                .spawn(async move {
                    let _ = services.lsp.close_lsp(&id).await;
                })
                .detach();
        }
    }

    pub fn set_display_file(&self, workspace_id: &str, f: Option<DisplayFileFn>) {
        if workspace_id.is_empty() {
            return;
        }
        let mut st = self.0.borrow_mut();
        match f {
            Some(f) => st.display.insert(workspace_id.to_string(), f),
            None => st.display.remove(workspace_id),
        };
    }

    pub fn set_file_io(&self, workspace_id: &str, io: Option<Rc<dyn LspFileIo>>) {
        if workspace_id.is_empty() {
            return;
        }
        let mut st = self.0.borrow_mut();
        match io {
            Some(io) => st.file_io.insert(workspace_id.to_string(), io),
            None => st.file_io.remove(workspace_id),
        };
    }

    /// Register a session whose client the caller built (tests, `installLspSessionForTest`).
    pub fn install_session(&self, info: &ClientInfo, client: Rc<Client>, kept: bool) {
        client.set_hub(self);
        self.0.borrow_mut().sessions.insert(
            info.id.clone(),
            Rc::new(Session {
                id: info.id.clone(),
                workspace_id: info.workspace_id.clone(),
                root_uri: info.root_uri.clone(),
                root_path: info.root_path.clone(),
                server_key: info.server_key.clone(),
                vue_plugin_path: info.vue_plugin_path.clone(),
                client,
                diag_uris: RefCell::default(),
                kept: Cell::new(kept),
                _pump: None,
            }),
        );
    }

    /// `createClient`: connect, initialize, and keep it only while an Ensure wants it.
    async fn create_client(self, info: ClientInfo, cx: AsyncApp) -> Option<Rc<Client>> {
        if let Some(s) = self.session(&info.id)
            && s.client.capabilities().is_some()
        {
            return Some(s.client.clone());
        }
        let services = self.services()?;
        let (tx, mut rx) = mpsc::unbounded::<String>();
        let pump = {
            let (id, root, key, vue) = (
                info.id.clone(),
                info.root_uri.clone(),
                info.server_key.clone(),
                info.vue_plugin_path.clone(),
            );
            cx.spawn(async move |_| {
                while let Some(m) = rx.next().await {
                    let m = rewrite_outgoing(&m, &root, &key, &vue);
                    let _ = services.lsp.write_lsp(&id, &m).await;
                }
            })
        };
        let sink: Sink = Rc::new(move |m| {
            let _ = tx.unbounded_send(m);
        });
        let client = Client::new(
            &info.id,
            &info.workspace_id,
            &info.root_uri,
            &info.server_key,
            sink,
            cx.background_executor().clone(),
            Some(cx.clone()),
        );
        client.set_hub(&self);
        self.0.borrow_mut().sessions.insert(
            info.id.clone(),
            Rc::new(Session {
                id: info.id.clone(),
                workspace_id: info.workspace_id.clone(),
                root_uri: info.root_uri.clone(),
                root_path: info.root_path.clone(),
                server_key: info.server_key.clone(),
                vue_plugin_path: info.vue_plugin_path.clone(),
                client: client.clone(),
                diag_uris: RefCell::default(),
                kept: Cell::new(false),
                _pump: Some(pump),
            }),
        );
        let init = client.connect().await;
        let wanted = {
            let st = self.0.borrow();
            st.ensure_in_flight.get(&info.id).copied().unwrap_or(0) > 0
                || st.sessions.get(&info.id).is_some_and(|s| s.kept.get())
        };
        if init.is_err() || client.capabilities().is_none() || !wanted {
            cx.update(|cx| self.close_lsp(&info.id, cx));
            return None;
        }
        Some(client)
    }

    /// `getOrCreateClient`: one creation per session id.
    async fn get_or_create_client(&self, info: ClientInfo, cx: &AsyncApp) -> Option<Rc<Client>> {
        if let Some(s) = self.session(&info.id)
            && s.client.capabilities().is_some()
        {
            return Some(s.client.clone());
        }
        let pending = self.0.borrow().creating.get(&info.id).cloned();
        if let Some(p) = pending {
            return p.await;
        }
        let fut = self
            .clone()
            .create_client(info.clone(), cx.clone())
            .boxed_local()
            .shared();
        self.0
            .borrow_mut()
            .creating
            .insert(info.id.clone(), fut.clone());
        let out = fut.await;
        self.0.borrow_mut().creating.remove(&info.id);
        out
    }

    fn claim_ensure_ids(&self, ids: &[String]) {
        let mut st = self.0.borrow_mut();
        for id in ids {
            *st.ensure_in_flight.entry(id.clone()).or_default() += 1;
        }
    }

    fn keep_ensure_ids(&self, ids: &[String]) {
        let mut st = self.0.borrow_mut();
        for id in ids {
            if let Some(s) = st.sessions.get(id) {
                s.kept.set(true);
            }
            st.deferred_close.retain(|(d, _)| d != id);
        }
    }

    fn pending_unknown(&self, workspace_id: &str) -> bool {
        !workspace_id.is_empty()
            && self
                .0
                .borrow()
                .pending_ensure
                .get(workspace_id)
                .copied()
                .unwrap_or(0)
                > 0
    }

    fn flush_deferred_close(&self, cx: &mut App) {
        let deferred = self.0.borrow().deferred_close.clone();
        for (id, workspace_id) in deferred {
            let (in_flight, slot, creating) = {
                let st = self.0.borrow();
                (
                    st.ensure_in_flight.get(&id).copied().unwrap_or(0),
                    st.sessions.get(&id).cloned(),
                    st.creating.contains_key(&id),
                )
            };
            if in_flight > 0 {
                continue;
            }
            if slot.as_ref().is_some_and(|s| s.kept.get()) {
                self.0.borrow_mut().deferred_close.retain(|(d, _)| *d != id);
                continue;
            }
            if creating && slot.is_none() {
                continue;
            }
            let ws = slot
                .as_ref()
                .map(|s| s.workspace_id.clone())
                .filter(|w| !w.is_empty())
                .unwrap_or(workspace_id);
            if self.pending_unknown(&ws) {
                continue;
            }
            self.close_lsp(&id, cx);
        }
    }

    fn release_ensure_ids(&self, ids: &[String], abandon: bool, workspace_id: &str, cx: &mut App) {
        for id in ids {
            let n = {
                let mut st = self.0.borrow_mut();
                let n = st
                    .ensure_in_flight
                    .get(id)
                    .copied()
                    .unwrap_or(1)
                    .saturating_sub(1);
                if n > 0 {
                    st.ensure_in_flight.insert(id.clone(), n);
                } else {
                    st.ensure_in_flight.remove(id);
                }
                n
            };
            if !abandon || n > 0 {
                continue;
            }
            let (slot, creating) = {
                let st = self.0.borrow();
                (st.sessions.get(id).cloned(), st.creating.contains_key(id))
            };
            if slot.as_ref().is_some_and(|s| s.kept.get()) {
                continue;
            }
            if creating && slot.is_none() {
                continue;
            }
            let ws = slot
                .as_ref()
                .map(|s| s.workspace_id.clone())
                .filter(|w| !w.is_empty())
                .unwrap_or_else(|| workspace_id.to_string());
            if self.pending_unknown(&ws) {
                let mut st = self.0.borrow_mut();
                st.deferred_close.retain(|(d, _)| d != id);
                st.deferred_close.push((id.clone(), ws));
                continue;
            }
            self.close_lsp(id, cx);
        }
    }

    /// `clearLspForWorkspace`.
    pub fn clear_workspace(&self, workspace_id: &str, cx: &mut App) {
        if workspace_id.is_empty() {
            return;
        }
        let ids: Vec<String> = self
            .0
            .borrow()
            .sessions
            .values()
            .filter(|s| s.workspace_id == workspace_id)
            .map(|s| s.id.clone())
            .collect();
        for id in ids {
            self.drop_session(&id, cx);
        }
        self.set_file_io(workspace_id, None);
        self.set_display_file(workspace_id, None);
    }

    /// `clearAllLsp`.
    pub fn clear_all(&self, cx: &mut App) {
        let ids: Vec<String> = self.0.borrow().sessions.keys().cloned().collect();
        for id in ids {
            self.drop_session(&id, cx);
        }
        let mut st = self.0.borrow_mut();
        st.file_io.clear();
        st.display.clear();
        st.pending_ensure.clear();
        st.ensure_in_flight.clear();
        st.deferred_close.clear();
        drop(st);
        update_problems(cx, |p| p.clear());
    }

    /// `clientForUri`.
    pub fn client_for_uri(&self, uri: &str) -> Option<Rc<Client>> {
        let st = self.0.borrow();
        st.sessions
            .values()
            .find(|s| s.client.file(uri).is_some())
            .or_else(|| st.sessions.values().find(|s| uri.starts_with(&s.root_uri)))
            .map(|s| s.client.clone())
    }

    /// `clientForProblem`.
    pub fn client_for_problem(&self, p: &LspProblem) -> Option<Rc<Client>> {
        if let Some(id) = &p.session_id
            && let Some(s) = self.session(id)
        {
            return Some(s.client.clone());
        }
        self.client_for_uri(&p.uri)
    }

    /// `notifyDidSave`.
    pub fn notify_did_save(&self, workspace_id: &str, rel: &str, text: &str) {
        if workspace_id.is_empty() {
            return;
        }
        for s in self.sessions() {
            if s.workspace_id != workspace_id {
                continue;
            }
            let uri = join_root_uri(&s.root_uri, rel);
            if s.client.file(&uri).is_none() {
                continue;
            }
            let caps = s.client.capabilities();
            let sync = caps.as_ref().and_then(|c| c.get("textDocumentSync"));
            let wants = match sync {
                None | Some(Value::Null) => true,
                Some(Value::Number(n)) => n.as_i64() != Some(0),
                Some(o) => o.get("save") != Some(&Value::Bool(false)),
            };
            if !wants {
                continue;
            }
            s.client.sync();
            let include = sync
                .and_then(|o| o.get("save"))
                .filter(|v| v.is_object())
                .and_then(|v| v.get("includeText"))
                .is_some_and(truthy);
            let mut params = json!({ "textDocument": { "uri": uri } });
            if include {
                params["text"] = json!(text);
            }
            s.client.notification("textDocument/didSave", params);
        }
    }
}

fn ensure_session_ids(res: &EnsureResult) -> Vec<String> {
    let mut ids = Vec::new();
    if !res.session.id.is_empty() {
        ids.push(res.session.id.clone());
    }
    for s in &res.extra {
        if !s.id.is_empty() {
            ids.push(s.id.clone());
        }
    }
    ids
}

/// `attachLsp`: ensure the file's servers and connect a client to each (TypeScript first).
/// `still_wanted` re-checks after every await that the open still wants it.
pub async fn attach_lsp(
    hub: LspHub,
    project_id: String,
    workspace_id: String,
    rel_path: String,
    language_id: String,
    still_wanted: Rc<dyn Fn(&App) -> bool>,
    cx: AsyncApp,
) -> Option<LspAttach> {
    let services = hub.services()?;
    *hub.0
        .borrow_mut()
        .pending_ensure
        .entry(workspace_id.clone())
        .or_default() += 1;
    let res = services
        .lsp
        .ensure_lsp(&project_id, &workspace_id, &language_id, &rel_path)
        .await
        .ok();
    let ids = res.as_ref().map(ensure_session_ids).unwrap_or_default();
    hub.claim_ensure_ids(&ids);
    {
        let mut st = hub.0.borrow_mut();
        let n = st
            .pending_ensure
            .get(&workspace_id)
            .copied()
            .unwrap_or(1)
            .saturating_sub(1);
        if n > 0 {
            st.pending_ensure.insert(workspace_id.clone(), n);
        } else {
            st.pending_ensure.remove(&workspace_id);
        }
    }
    cx.update(|cx| hub.flush_deferred_close(cx));
    let wanted = |cx: &AsyncApp| cx.update(|cx| still_wanted(cx));
    let abandon = |hub: &LspHub| {
        cx.update(|cx| hub.release_ensure_ids(&ids, true, &workspace_id, cx));
    };
    if !wanted(&cx) {
        abandon(&hub);
        return None;
    }
    let Some(res) = res.filter(|r| {
        r.session.available
            && !r.session.id.is_empty()
            && !r.file_uri.is_empty()
            && !r.session.root_uri.is_empty()
    }) else {
        abandon(&hub);
        return None;
    };
    let open_lang = [language_id.as_str(), res.language_id.as_str(), "plaintext"]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("plaintext")
        .to_string();
    let infos: Vec<ClientInfo> = std::iter::once(&res.session)
        .chain(res.extra.iter())
        .filter(|s| s.available && !s.id.is_empty() && !s.root_uri.is_empty())
        .map(|s| ClientInfo {
            id: s.id.clone(),
            workspace_id: workspace_id.clone(),
            root_uri: if s.root_uri.is_empty() {
                res.session.root_uri.clone()
            } else {
                s.root_uri.clone()
            },
            root_path: if s.root_path.is_empty() {
                res.session.root_path.clone()
            } else {
                s.root_path.clone()
            },
            server_key: s.server_key.clone(),
            vue_plugin_path: s.vue_plugin_path.clone(),
        })
        .collect();
    if let Some(ts) = infos.iter().find(|i| i.server_key == TYPESCRIPT_SERVER) {
        if !wanted(&cx) {
            abandon(&hub);
            return None;
        }
        hub.get_or_create_client(ts.clone(), &cx).await;
    }
    let mut clients: Vec<Rc<Client>> = Vec::new();
    let mut primary: Option<(Rc<Client>, String, String)> = None;
    for info in &infos {
        if !wanted(&cx) {
            abandon(&hub);
            return None;
        }
        let Some(client) = hub.get_or_create_client(info.clone(), &cx).await else {
            continue;
        };
        if !client.connected() || client.capabilities().is_none() {
            continue;
        }
        if primary.is_none() {
            primary = Some((client.clone(), info.id.clone(), info.server_key.clone()));
        }
        clients.push(client);
    }
    if !wanted(&cx) {
        abandon(&hub);
        return None;
    }
    let Some((client, session_id, server_key)) = primary else {
        abandon(&hub);
        return None;
    };
    hub.keep_ensure_ids(&ids);
    cx.update(|cx| hub.release_ensure_ids(&ids, false, "", cx));
    Some(LspAttach {
        client,
        clients,
        file_uri: res.file_uri.clone(),
        language_id: open_lang,
        session_id,
        root_uri: res.session.root_uri.clone(),
        server_key,
    })
}

// ---- requests the Observatory makes ----

fn symbol_kind_name(kind: Option<i64>) -> &'static str {
    match kind {
        Some(5) => "class",
        Some(6) => "method",
        Some(12) => "function",
        Some(11) | Some(13) => "var",
        Some(14) => "const",
        Some(10) => "type",
        Some(23) => "export",
        _ => "symbol",
    }
}

fn line_of(pos: Option<&Value>) -> i64 {
    pos.and_then(|p| p.get("line"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// `flattenSymbols`: `SymbolInformation` or nested `DocumentSymbol`s, depth first.
pub fn flatten_symbols(nodes: &[Value], out: &mut Vec<recur_domain::OutlineSymbol>) {
    for n in nodes {
        if n.is_null() {
            continue;
        }
        let name = match n.get("name") {
            Some(Value::String(s)) => s.clone(),
            None | Some(Value::Null) => String::new(),
            Some(v) => v.to_string(),
        };
        let kind = symbol_kind_name(n.get("kind").and_then(Value::as_i64)).to_string();
        if let Some(range) = n.get("location").and_then(|l| l.get("range")) {
            out.push(recur_domain::OutlineSymbol {
                name,
                kind,
                line: line_of(range.get("start")) + 1,
                end: line_of(range.get("end")) + 1,
            });
            continue;
        }
        if let Some(range) = n.get("selectionRange").or_else(|| n.get("range")) {
            let end = n
                .get("range")
                .and_then(|r| r.get("end"))
                .or_else(|| range.get("end"));
            out.push(recur_domain::OutlineSymbol {
                name,
                kind,
                line: line_of(range.get("start")) + 1,
                end: line_of(end) + 1,
            });
        }
        if let Some(children) = n.get("children").and_then(Value::as_array)
            && !children.is_empty()
        {
            flatten_symbols(children, out);
        }
    }
}

/// `lspOutline`: `None` when unsupported or failed.
pub async fn lsp_outline(
    client: &Rc<Client>,
    file_uri: &str,
) -> Option<Vec<recur_domain::OutlineSymbol>> {
    if !client.has_document_symbol() {
        return None;
    }
    client.sync();
    let result = client
        .request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": file_uri } }),
        )
        .response
        .await
        .ok()?;
    let nodes = result.as_array()?;
    let mut out = Vec::new();
    flatten_symbols(nodes, &mut out);
    Some(out)
}

/// `lspReferences` at a 0-based position.
pub async fn lsp_references(
    client: &Rc<Client>,
    file_uri: &str,
    root_uri: &str,
    line: i64,
    character: i64,
) -> Option<Vec<recur_domain::SymbolRef>> {
    if !client.has_provider("referencesProvider") {
        return None;
    }
    client.sync();
    let locs = client
        .request(
            "textDocument/references",
            json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line, "character": character },
                "context": { "includeDeclaration": true },
            }),
        )
        .response
        .await
        .ok()?;
    let locs = locs.as_array()?;
    Some(
        locs.iter()
            .map(|loc| {
                let uri = match loc.get("uri") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                    None => "undefined".into(),
                };
                let path = uri_to_rel_path(root_uri, &uri);
                let line = line_of(loc.get("range").and_then(|r| r.get("start"))) + 1;
                recur_domain::SymbolRef {
                    text: format!("{path}:{line}"),
                    path,
                    line,
                    def: false,
                }
            })
            .collect(),
    )
}

/// `CodeActionItem`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodeActionItem {
    pub title: String,
    pub kind: Option<Value>,
    pub edit: Option<Value>,
    pub command: Option<Value>,
    pub data: Option<Value>,
}

fn opt(v: Option<&Value>) -> Option<Value> {
    v.filter(|v| !v.is_null()).cloned()
}

fn severity_num(s: ProblemSeverity) -> i64 {
    match s {
        ProblemSeverity::Error => 1,
        ProblemSeverity::Warning => 2,
        _ => 3,
    }
}

/// `lspCodeActions`: quick fixes for a problem's range.
pub async fn lsp_code_actions(client: &Rc<Client>, p: &LspProblem) -> Vec<CodeActionItem> {
    if !client.has_provider("codeActionProvider") {
        return Vec::new();
    }
    client.sync();
    let range = json!({
        "start": { "line": p.line - 1, "character": p.character },
        "end": { "line": p.end_line - 1, "character": p.end_character },
    });
    let mut diagnostic = json!({
        "range": range,
        "message": p.message,
        "severity": severity_num(p.severity),
    });
    if let Some(s) = &p.source {
        diagnostic["source"] = json!(s);
    }
    if let Some(c) = &p.code {
        diagnostic["code"] = c.to_json();
    }
    let Ok(result) = client
        .request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": p.uri },
                "range": range,
                "context": { "diagnostics": [diagnostic], "only": ["quickfix"] },
            }),
        )
        .response
        .await
    else {
        return Vec::new();
    };
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    let str_of = |v: Option<&Value>| match v {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Null) | None => None,
        Some(v) => Some(v.to_string()),
    };
    items
        .iter()
        .map(|a| {
            if let Some(cmd) = a.get("command").and_then(Value::as_str) {
                CodeActionItem {
                    title: str_of(a.get("title")).unwrap_or_else(|| cmd.to_string()),
                    command: Some(a.clone()),
                    data: opt(a.get("data")),
                    ..Default::default()
                }
            } else {
                CodeActionItem {
                    title: str_of(a.get("title")).unwrap_or_else(|| "Fix".into()),
                    kind: opt(a.get("kind")),
                    edit: opt(a.get("edit")),
                    command: opt(a.get("command")),
                    data: opt(a.get("data")),
                }
            }
        })
        .filter(|a| !a.title.is_empty())
        .collect()
}

/// `resolveCodeAction`.
pub async fn resolve_code_action(client: &Rc<Client>, action: CodeActionItem) -> CodeActionItem {
    if action.edit.is_some() || action.data.is_none() {
        return action;
    }
    let mut params = json!({ "title": action.title });
    for (k, v) in [
        ("kind", &action.kind),
        ("edit", &action.edit),
        ("command", &action.command),
        ("data", &action.data),
    ] {
        if let Some(v) = v {
            params[k] = v.clone();
        }
    }
    let Ok(resolved) = client.request("codeAction/resolve", params).response.await else {
        return action;
    };
    if resolved.is_null() {
        return action;
    }
    CodeActionItem {
        title: resolved
            .get("title")
            .and_then(Value::as_str)
            .map_or(action.title.clone(), str::to_string),
        kind: opt(resolved.get("kind")).or(action.kind),
        edit: opt(resolved.get("edit")).or(action.edit),
        command: opt(resolved.get("command")).or(action.command),
        data: opt(resolved.get("data")).or(action.data),
    }
}

/// Edits with the document version they were computed against.
type TaggedEdits = Vec<(Value, Option<Value>)>;

/// `collectEditChanges`: `changes` then `documentChanges`, each edit tagged with its version.
fn collect_edit_changes(edit: &Value) -> Vec<(String, TaggedEdits)> {
    let mut out: Vec<(String, TaggedEdits)> = Vec::new();
    if let Some(m) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in m {
            let list = edits
                .as_array()
                .map(|a| a.iter().map(|e| (e.clone(), None)).collect())
                .unwrap_or_default();
            out.push((uri.clone(), list));
        }
    }
    if let Some(dcs) = edit.get("documentChanges").and_then(Value::as_array) {
        for dc in dcs {
            let Some(uri) = dc
                .get("textDocument")
                .and_then(|t| t.get("uri"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(edits) = dc.get("edits").and_then(Value::as_array) else {
                continue;
            };
            let ver = dc
                .get("textDocument")
                .and_then(|t| t.get("version"))
                .cloned();
            let tagged = edits.iter().map(|e| (e.clone(), ver.clone()));
            match out.iter_mut().find(|(u, _)| u == uri) {
                Some((_, list)) => list.extend(tagged),
                None => out.push((uri.to_string(), tagged.collect())),
            }
        }
    }
    out
}

/// `posToOffset` as a byte offset: the line clamped into the document, the UTF-16 column
/// clamped to the line.
pub fn pos_to_offset(doc: &Text, line: i64, character: i64) -> usize {
    let n = usize::try_from(line + 1).unwrap_or(1).clamp(1, doc.lines());
    let l = doc.line(n);
    let from16 = doc.byte_to_utf16(l.from);
    let to16 = doc.byte_to_utf16(l.to);
    let ch = usize::try_from(character).unwrap_or(0);
    doc.utf16_to_byte(to16.min(from16 + ch))
}

fn pos_of(v: Option<&Value>) -> (i64, i64) {
    (
        line_of(v),
        v.and_then(|p| p.get("character"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
    )
}

/// `applyEditsToText`: last edit first, offsets in the original text.
pub fn apply_edits_to_text(text: &str, edits: &[Value]) -> String {
    let doc = Text::of(text);
    let mut sorted: Vec<&Value> = edits.iter().collect();
    sorted.sort_by(|a, b| {
        let (la, ca) = pos_of(a.get("range").and_then(|r| r.get("start")));
        let (lb, cb) = pos_of(b.get("range").and_then(|r| r.get("start")));
        lb.cmp(&la).then(cb.cmp(&ca))
    });
    let mut out = text.to_string();
    for e in sorted {
        let range = e.get("range");
        let (sl, sc) = pos_of(range.and_then(|r| r.get("start")));
        let (el, ec) = pos_of(range.and_then(|r| r.get("end")));
        let from = pos_to_offset(&doc, sl, sc).min(out.len());
        let to = pos_to_offset(&doc, el, ec).min(out.len()).max(from);
        let insert = match e.get("newText") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        };
        out.replace_range(from..to, &insert);
    }
    out
}

struct Planned {
    uri: String,
    edits: TaggedEdits,
    rel: String,
    base: String,
    view: bool,
    loaded: bool,
}

fn io_unsaved(io: &Option<Rc<dyn LspFileIo>>, rel: &str, app: &Option<AsyncApp>) -> Option<String> {
    let (io, app) = (io.as_ref()?, app.as_ref()?);
    if rel.is_empty() {
        return None;
    }
    app.update(|cx| io.unsaved(rel, cx))
}

/// `applyWorkspaceEdit`: open editors, unsaved buffers and disk, all or nothing.
pub async fn apply_workspace_edit(
    client: &Rc<Client>,
    edit: &Value,
    bases: Option<HashMap<String, String>>,
) -> bool {
    let changes = collect_edit_changes(edit);
    let root = client.root_uri.clone();
    let io = client.file_io();
    let app = client.app.clone();
    let mut snapshots = bases.unwrap_or_else(|| client.snapshot_docs());
    let mut planned: Vec<Planned> = Vec::new();
    for (uri, edits) in changes {
        if edits.is_empty() {
            continue;
        }
        let file = client.file(&uri);
        let view = file.as_ref().is_some_and(|f| f.live.is_some());
        let rel = if root.is_empty() {
            String::new()
        } else {
            uri_to_rel_path(&root, &uri)
        };
        let mut text = snapshots.get(&uri).cloned();
        if text.is_none() && view {
            text = file
                .as_ref()
                .and_then(|f| f.live.as_ref().map(Text::to_string));
        }
        if text.is_none() {
            text = io_unsaved(&io, &rel, &app);
        }
        if text.is_none() {
            text = file.as_ref().map(|f| f.doc.to_string());
        }
        if let Some(t) = &text {
            snapshots.insert(uri.clone(), t.clone());
        }
        planned.push(Planned {
            uri,
            edits,
            rel,
            loaded: text.is_some(),
            base: text.unwrap_or_default(),
            view,
        });
    }
    for p in planned.iter_mut() {
        if p.loaded {
            continue;
        }
        let mut file = client.file(&p.uri);
        if file.is_none() {
            file = client.request_file(&p.uri).await;
        }
        let view = file.as_ref().is_some_and(|f| f.live.is_some());
        let mut text = file
            .as_ref()
            .and_then(|f| f.live.as_ref().map(Text::to_string));
        if text.is_none() {
            text = io_unsaved(&io, &p.rel, &app);
        }
        if text.is_none() {
            text = file.as_ref().map(|f| f.doc.to_string());
        }
        if text.is_none()
            && let (Some(io), Some(app)) = (io.as_ref(), app.as_ref())
            && !p.rel.is_empty()
        {
            text = app.update(|cx| io.read(&p.rel, cx)).await;
        }
        let Some(text) = text else { return false };
        p.base = text;
        p.view = view;
        p.loaded = true;
    }
    for p in planned.iter_mut() {
        let file = client.file(&p.uri);
        let doc_version = file.as_ref().map(|f| f.version);
        for (_, ver) in &p.edits {
            if let (Some(ver), Some(dv)) = (ver.as_ref().and_then(Value::as_i64), doc_version)
                && ver != -1
                && ver != dv
            {
                return false;
            }
        }
        let live = match file.as_ref() {
            Some(f) if f.live.is_some() => f.live.as_ref().map(Text::to_string),
            _ => io_unsaved(&io, &p.rel, &app).or_else(|| file.as_ref().map(|f| f.doc.to_string())),
        };
        if live.as_ref().is_some_and(|l| *l != p.base) {
            return false;
        }
        p.view = file.as_ref().is_some_and(|f| f.live.is_some()) || p.view;
    }
    for p in &planned {
        if !p.view {
            continue;
        }
        let edits: Vec<Value> = p.edits.iter().map(|(e, _)| e.clone()).collect();
        let next = apply_edits_to_text(&p.base, &edits);
        if let (Some(io), Some(app)) = (io.as_ref(), app.as_ref()) {
            app.update(|cx| io.apply_if_open(&p.rel, &next, cx));
        }
    }
    for p in &planned {
        if p.view {
            continue;
        }
        let edits: Vec<Value> = p.edits.iter().map(|(e, _)| e.clone()).collect();
        let next = apply_edits_to_text(&p.base, &edits);
        if let (Some(io), Some(app)) = (io.as_ref(), app.as_ref())
            && !p.rel.is_empty()
            && !app.update(|cx| io.apply_if_open(&p.rel, &next, cx))
        {
            app.update(|cx| io.write(&p.rel, next.clone(), cx)).await;
        }
        if client.file(&p.uri).is_some_and(|f| f.live.is_some()) {
            continue;
        }
        client.replace_detached_doc(&p.uri, &next);
    }
    client.sync();
    true
}

/// `applyLspQuickFix`: the server's first editable fix for a problem, with a toast.
pub async fn apply_lsp_quick_fix(
    hub: &LspHub,
    p: &LspProblem,
    fallback: Option<Rc<Client>>,
    cx: &mut AsyncApp,
) -> bool {
    let toast = |kind: ToastKind, text: String, cx: &mut AsyncApp| {
        cx.update(|cx| push_toast(kind, text, ToastOptions::default(), cx));
    };
    let Some(client) = hub.client_for_problem(p).or(fallback) else {
        toast(
            ToastKind::Info,
            "No language server for this file".into(),
            cx,
        );
        return false;
    };
    let edits_now = || {
        client
            .file(&p.uri)
            .filter(|f| f.live.is_some())
            .map(|f| f.edits)
    };
    let before = edits_now();
    let actions = lsp_code_actions(&client, p).await;
    if before.is_some() && edits_now() != before {
        toast(
            ToastKind::Info,
            "Document changed; retry the fix".into(),
            cx,
        );
        return false;
    }
    let mut with_edit = actions.iter().find(|a| a.edit.is_some()).cloned();
    if with_edit.is_none()
        && let Some(pending) = actions.iter().find(|a| a.data.is_some()).cloned()
    {
        with_edit = Some(resolve_code_action(&client, pending).await);
    }
    let Some(action) = with_edit.filter(|a| a.edit.is_some()) else {
        toast(
            ToastKind::Info,
            if actions.is_empty() {
                "No quick fixes"
            } else {
                "No applyable quick fix"
            }
            .into(),
            cx,
        );
        return false;
    };
    if before.is_some() && edits_now() != before {
        toast(
            ToastKind::Info,
            "Document changed; retry the fix".into(),
            cx,
        );
        return false;
    }
    let edit = action.edit.clone().unwrap_or(Value::Null);
    if !apply_workspace_edit(&client, &edit, None).await {
        toast(
            ToastKind::Info,
            "Could not apply fix to every file".into(),
            cx,
        );
        return false;
    }
    toast(ToastKind::Success, action.title.clone(), cx);
    true
}

/// `fixProblemWithAI`.
pub fn fix_problem_with_ai(p: &LspProblem, state: &gpui_kit::Entity<AppState>, cx: &mut App) {
    let args = super::problems::problem_args(p);
    let ws = p.workspace_id.clone().unwrap_or_default();
    state.update(cx, |s, cx| {
        s.request_prompt_action(&ws, "fix-problem", &args, cx)
    });
}

/// `paintMergedDiagnostics`: every session's problems for `uri`, on the editor's text, with
/// the Fix / Fix with AI buttons.
pub fn merged_diagnostics(
    uri: &str,
    doc: &Text,
    state: &gpui_kit::Entity<AppState>,
    cx: &App,
) -> Vec<recur_editor::lint::Diagnostic> {
    use recur_editor::lint::{Diagnostic, DiagnosticAction, Severity};
    LspProblems::global(cx)
        .map(|s| s.for_uri(uri))
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let p = Rc::new(p);
            let fix = {
                let p = p.clone();
                DiagnosticAction {
                    name: "Fix".into(),
                    mark_class: None,
                    apply: Rc::new(move |_, _, _, cx: &mut App| {
                        let hub = LspHub::global(cx);
                        let p = p.clone();
                        cx.spawn(async move |cx: &mut AsyncApp| {
                            apply_lsp_quick_fix(&hub, &p, None, cx).await;
                        })
                        .detach();
                    }),
                }
            };
            let mut actions = vec![fix];
            if p.workspace_id.as_deref().is_some_and(|w| !w.is_empty()) {
                let (p, state) = (p.clone(), state.clone());
                actions.push(DiagnosticAction {
                    name: "Fix with AI".into(),
                    mark_class: None,
                    apply: Rc::new(move |_, _, _, cx: &mut App| {
                        fix_problem_with_ai(&p, &state, cx)
                    }),
                });
            }
            Diagnostic {
                from: pos_to_offset(doc, p.line - 1, p.character),
                to: pos_to_offset(doc, p.end_line - 1, p.end_character),
                severity: match p.severity {
                    ProblemSeverity::Error => Severity::Error,
                    ProblemSeverity::Warning => Severity::Warning,
                    ProblemSeverity::Info => Severity::Info,
                    ProblemSeverity::Hint => Severity::Hint,
                },
                message: p.message.clone(),
                source: None,
                mark_class: None,
                actions,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::TestAppContext;
    use parking_lot::Mutex;
    use recur_services::mock::{MockBridge, MockOptions, Reply};

    use crate::lsp::problems::peek_problems;

    use super::*;

    type Sent = Rc<RefCell<Vec<Value>>>;

    fn stub(
        cx: &mut TestAppContext,
        hub: &LspHub,
        workspace_id: &str,
        root_uri: &str,
    ) -> (Rc<Client>, Sent) {
        let sent: Sent = Rc::default();
        let s = sent.clone();
        let client = Client::new(
            &format!("sess-{workspace_id}"),
            workspace_id,
            root_uri,
            "",
            Rc::new(move |m: String| s.borrow_mut().push(serde_json::from_str(&m).unwrap())),
            cx.background_executor.clone(),
            Some(cx.to_async()),
        );
        client.set_hub(hub);
        client.mark_ready(json!({}));
        (client, sent)
    }

    fn methods(sent: &Sent, method: &str) -> Vec<Value> {
        sent.borrow()
            .iter()
            .filter(|m| m.get("method").and_then(Value::as_str) == Some(method))
            .map(|m| m["params"].clone())
            .collect()
    }

    fn range(start: i64, end: i64, text: &str) -> Value {
        json!({
            "range": { "start": { "line": 0, "character": start }, "end": { "line": 0, "character": end } },
            "newText": text,
        })
    }

    struct Io {
        disk: Rc<RefCell<HashMap<String, String>>>,
        gate: RefCell<Option<oneshot::Receiver<()>>>,
        waiting: Rc<Cell<bool>>,
    }

    impl LspFileIo for Io {
        fn unsaved(&self, _: &str, _: &mut App) -> Option<String> {
            None
        }
        fn read(&self, rel: &str, _: &mut App) -> LocalBoxFuture<'static, Option<String>> {
            let gate = if rel == "b.go" {
                self.gate.borrow_mut().take()
            } else {
                None
            };
            let waiting = self.waiting.clone();
            let text = self.disk.borrow().get(rel).cloned();
            async move {
                if let Some(g) = gate {
                    waiting.set(true);
                    let _ = g.await;
                }
                text
            }
            .boxed_local()
        }
        fn write(&self, rel: &str, text: String, _: &mut App) -> LocalBoxFuture<'static, ()> {
            self.disk.borrow_mut().insert(rel.to_string(), text);
            async {}.boxed_local()
        }
        fn apply_if_open(&self, _: &str, _: &str, _: &mut App) -> bool {
            false
        }
    }

    #[gpui::test]
    fn reattach_keeps_the_last_synced_snapshot_so_pending_edits_still_flush(
        cx: &mut TestAppContext,
    ) {
        let hub = LspHub::default();
        let (c, _) = stub(cx, &hub, "ws-a", "file:///tmp/ws");
        let uri = "file:///tmp/ws/a.go";
        c.open_file(uri, "go", &Text::of("old"));
        let old = Text::of("old");
        c.file_changed(uri, &old, &ChangeSet::empty(3), &Text::of("pending"));
        c.detach_view(uri);
        c.attach_view(uri, &Text::of("pending"));
        assert_eq!(c.file(uri).unwrap().doc.to_string(), "old");
        assert_eq!(c.sync_files().len(), 1);
        assert_eq!(c.file(uri).unwrap().doc.to_string(), "pending");
    }

    #[gpui::test]
    fn close_file_flushes_the_live_buffer_before_detaching(cx: &mut TestAppContext) {
        let hub = LspHub::default();
        let (c, _) = stub(cx, &hub, "ws-a", "file:///tmp/ws");
        let uri = "file:///tmp/ws/a.go";
        c.open_file(uri, "go", &Text::of("old"));
        c.file_changed(
            uri,
            &Text::of("old"),
            &ChangeSet::empty(3),
            &Text::of("new"),
        );
        c.close_file(uri);
        let f = c.file(uri).unwrap();
        assert!(f.live.is_none());
        assert_eq!(f.doc.to_string(), "new");
    }

    #[gpui::test]
    async fn apply_aborts_if_a_planned_editor_changes_while_another_file_loads(
        cx: &mut TestAppContext,
    ) {
        let hub = LspHub::default();
        let (c, _) = stub(cx, &hub, "ws-a", "file:///tmp/ws");
        c.open_file("file:///tmp/ws/a.go", "go", &Text::of("aaa"));
        let (release, gate) = oneshot::channel();
        let waiting = Rc::new(Cell::new(false));
        let disk = Rc::new(RefCell::new(HashMap::from([(
            "b.go".to_string(),
            "bbb".to_string(),
        )])));
        hub.set_file_io(
            "ws-a",
            Some(Rc::new(Io {
                disk,
                gate: RefCell::new(Some(gate)),
                waiting: waiting.clone(),
            })),
        );
        let edit = json!({ "changes": {
            "file:///tmp/ws/a.go": [range(0, 3, "AAA")],
            "file:///tmp/ws/b.go": [range(0, 3, "BBB")],
        }});
        let c2 = c.clone();
        let pending = cx.spawn(async move |_| apply_workspace_edit(&c2, &edit, None).await);
        cx.run_until_parked();
        assert!(waiting.get());
        c.file_changed(
            "file:///tmp/ws/a.go",
            &Text::of("aaa"),
            &ChangeSet::empty(3),
            &Text::of("typed"),
        );
        release.send(()).unwrap();
        assert!(!pending.await);
        let f = c.file("file:///tmp/ws/a.go").unwrap();
        assert_eq!(f.live.unwrap().to_string(), "typed");
    }

    #[gpui::test]
    async fn f2_snapshot_rejects_a_rename_if_the_editor_changed_during_the_request(
        cx: &mut TestAppContext,
    ) {
        let hub = LspHub::default();
        let (c, _) = stub(cx, &hub, "ws-a", "file:///tmp/ws");
        let uri = "file:///tmp/ws/a.go";
        c.open_file(uri, "go", &Text::of("aaa"));
        let bases = c.snapshot_docs();
        c.file_changed(
            uri,
            &Text::of("aaa"),
            &ChangeSet::empty(3),
            &Text::of("typed"),
        );
        let edit = json!({ "changes": { uri: [range(0, 3, "AAA")] } });
        assert!(!apply_workspace_edit(&c, &edit, Some(bases)).await);
        assert_eq!(c.file(uri).unwrap().live.unwrap().to_string(), "typed");
    }

    #[gpui::test]
    async fn consecutive_edits_without_a_view_keep_the_server_snapshot(cx: &mut TestAppContext) {
        let hub = LspHub::default();
        let (c, sent) = stub(cx, &hub, "ws-a", "file:///tmp/ws");
        let uri = "file:///tmp/ws/a.go";
        let disk = Rc::new(RefCell::new(HashMap::from([(
            "a.go".to_string(),
            "foo baz".to_string(),
        )])));
        hub.set_file_io(
            "ws-a",
            Some(Rc::new(Io {
                disk: disk.clone(),
                gate: RefCell::new(None),
                waiting: Rc::default(),
            })),
        );
        let e1 = json!({ "changes": { uri: [range(0, 3, "new")] } });
        assert!(apply_workspace_edit(&c, &e1, None).await);
        let f = c.file(uri).unwrap();
        assert_eq!(f.doc.to_string(), "new baz");
        assert_eq!(disk.borrow()["a.go"], "new baz");
        assert_eq!(methods(&sent, "textDocument/didOpen").len(), 1);
        let changes = methods(&sent, "textDocument/didChange");
        assert_eq!(
            changes.last().unwrap()["textDocument"],
            json!({ "uri": uri, "version": f.version })
        );
        assert_eq!(
            changes.last().unwrap()["contentChanges"],
            json!([{ "text": "new baz" }])
        );
        let e2 = json!({ "changes": { uri: [range(4, 7, "qux")] } });
        assert!(apply_workspace_edit(&c, &e2, None).await);
        assert_eq!(c.file(uri).unwrap().doc.to_string(), "new qux");
        assert_eq!(disk.borrow()["a.go"], "new qux");
        assert_eq!(methods(&sent, "textDocument/didOpen").len(), 1);
        let changes = methods(&sent, "textDocument/didChange");
        assert_eq!(
            changes.last().unwrap()["contentChanges"],
            json!([{ "text": "new qux" }])
        );
    }

    type Releases = Arc<Mutex<Vec<recur_services::mock::Release>>>;

    fn ensure_res(id: &str, server_key: &str, file_uri: &str, language_id: &str) -> Value {
        json!({
            "available": true,
            "id": id,
            "serverKey": server_key,
            "rootUri": "file:///tmp/ws",
            "rootPath": "/tmp/ws",
            "fileUri": file_uri,
            "languageId": language_id,
        })
    }

    fn ensure_hub(cx: &mut TestAppContext) -> (LspHub, Releases, Arc<Mutex<Vec<String>>>) {
        let held: Releases = Arc::default();
        let closed: Arc<Mutex<Vec<String>>> = Arc::default();
        let (h, c) = (held.clone(), closed.clone());
        let bridge = MockBridge::fresh(
            MockOptions::default()
                .resolver("EnsureLSP", move |_, _, _| {
                    let (r, release) = Reply::deferred();
                    h.lock().push(release);
                    r
                })
                .resolver("CloseLSP", move |a, _, _| {
                    c.lock().push(a[0].as_str().unwrap_or("").to_string());
                    Reply::null()
                }),
        );
        let services = bridge.services();
        let hub = LspHub::default();
        cx.update(|cx| hub.wire(&services, cx));
        (hub, held, closed)
    }

    fn attach(
        cx: &mut TestAppContext,
        hub: &LspHub,
        rel: &str,
        lang: &str,
        wanted: Rc<Cell<bool>>,
    ) -> gpui::Task<Option<LspAttach>> {
        let (hub, rel, lang) = (hub.clone(), rel.to_string(), lang.to_string());
        let app = cx.to_async();
        cx.spawn(async move |_| {
            attach_lsp(
                hub,
                "p".into(),
                "ws-a".into(),
                rel,
                lang,
                Rc::new(move |_: &App| wanted.get()),
                app,
            )
            .await
        })
    }

    #[gpui::test]
    async fn stale_ensure_closes_the_session_when_nothing_else_is_pending(cx: &mut TestAppContext) {
        let (hub, held, closed) = ensure_hub(cx);
        let p = attach(cx, &hub, "a.go", "go", Rc::new(Cell::new(false)));
        cx.run_until_parked();
        held.lock().remove(0).send(Reply::value(ensure_res(
            "gopls-1",
            "gopls",
            "file:///tmp/ws/a.go",
            "go",
        )));
        assert!(p.await.is_none());
        cx.run_until_parked();
        assert_eq!(*closed.lock(), vec!["gopls-1".to_string()]);
    }

    async fn two_pending(
        cx: &mut TestAppContext,
        a: (&str, &str, Value),
        b: (&str, &str, Value),
        closes_after_b: &[&str],
    ) {
        let (hub, held, closed) = ensure_hub(cx);
        let want_a = Rc::new(Cell::new(true));
        let pa = attach(cx, &hub, a.0, a.1, want_a.clone());
        cx.run_until_parked();
        let pb = attach(cx, &hub, b.0, b.1, Rc::new(Cell::new(false)));
        cx.run_until_parked();
        want_a.set(false);
        held.lock().remove(0).send(Reply::value(a.2));
        assert!(pa.await.is_none());
        cx.run_until_parked();
        assert!(closed.lock().is_empty());
        held.lock().remove(0).send(Reply::value(b.2));
        assert!(pb.await.is_none());
        cx.run_until_parked();
        let mut got = closed.lock().clone();
        got.sort();
        let mut want: Vec<String> = closes_after_b.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(got, want);
    }

    #[gpui::test]
    async fn stale_ensure_does_not_close_while_another_ensure_for_the_same_language_is_pending(
        cx: &mut TestAppContext,
    ) {
        let res = ensure_res("shared-gopls", "gopls", "file:///tmp/ws/a.go", "go");
        two_pending(
            cx,
            ("a.go", "go", res.clone()),
            ("b.go", "go", res),
            &["shared-gopls"],
        )
        .await;
    }

    #[gpui::test]
    async fn stale_ensure_does_not_close_while_another_language_sharing_the_server_is_pending(
        cx: &mut TestAppContext,
    ) {
        two_pending(
            cx,
            (
                "a.ts",
                "typescript",
                ensure_res(
                    "shared-ts-server",
                    TYPESCRIPT_SERVER,
                    "file:///tmp/ws/a.ts",
                    "typescript",
                ),
            ),
            (
                "b.tsx",
                "typescriptreact",
                ensure_res(
                    "shared-ts-server",
                    TYPESCRIPT_SERVER,
                    "file:///tmp/ws/b.tsx",
                    "typescriptreact",
                ),
            ),
            &["shared-ts-server"],
        )
        .await;
    }

    #[gpui::test]
    async fn stale_ensure_closes_once_an_unrelated_pending_ensure_in_the_workspace_finishes(
        cx: &mut TestAppContext,
    ) {
        two_pending(
            cx,
            (
                "a.go",
                "go",
                ensure_res("gopls-1", "gopls", "file:///tmp/ws/a.go", "go"),
            ),
            (
                "a.py",
                "python",
                ensure_res("pyright-1", "basedpyright", "file:///tmp/ws/a.py", "python"),
            ),
            &["gopls-1", "pyright-1"],
        )
        .await;
    }

    #[gpui::test]
    fn notify_did_save_only_notifies_the_workspace_that_saved_the_file(cx: &mut TestAppContext) {
        let hub = LspHub::default();
        let (a, sent_a) = stub(cx, &hub, "ws-a", "file:///tmp/a");
        let (b, sent_b) = stub(cx, &hub, "ws-b", "file:///tmp/b");
        a.open_file(
            "file:///tmp/a/src/foo.ts",
            "typescript",
            &Text::of("from-a"),
        );
        b.open_file(
            "file:///tmp/b/src/foo.ts",
            "typescript",
            &Text::of("from-b"),
        );
        let caps = json!({ "textDocumentSync": { "save": { "includeText": true } } });
        a.mark_ready(caps.clone());
        b.mark_ready(caps);
        for (ws, root, c) in [("ws-a", "file:///tmp/a", &a), ("ws-b", "file:///tmp/b", &b)] {
            hub.install_session(
                &ClientInfo {
                    id: format!("sess-{ws}"),
                    workspace_id: ws.into(),
                    root_uri: root.into(),
                    ..Default::default()
                },
                c.clone(),
                true,
            );
        }
        sent_a.borrow_mut().clear();
        sent_b.borrow_mut().clear();
        hub.notify_did_save("ws-a", "src/foo.ts", "saved-in-a");
        assert_eq!(
            methods(&sent_a, "textDocument/didSave"),
            vec![
                json!({ "textDocument": { "uri": "file:///tmp/a/src/foo.ts" }, "text": "saved-in-a" })
            ]
        );
        assert!(sent_b.borrow().is_empty());
    }

    #[gpui::test]
    fn initialize_merges_capabilities_then_sends_initialized_and_queued_messages(
        cx: &mut TestAppContext,
    ) {
        let sent: Sent = Rc::default();
        let s = sent.clone();
        let c = Client::new(
            "s1",
            "ws-a",
            "file:///tmp/ws",
            "gopls",
            Rc::new(move |m: String| s.borrow_mut().push(serde_json::from_str(&m).unwrap())),
            cx.background_executor.clone(),
            None,
        );
        let init = cx.spawn({
            let c = c.clone();
            async move |_| c.connect().await
        });
        cx.run_until_parked();
        let first = sent.borrow()[0].clone();
        assert_eq!(first["method"], "initialize");
        let caps = &first["params"]["capabilities"];
        assert_eq!(
            caps["textDocument"]["publishDiagnostics"]["versionSupport"],
            true
        );
        assert_eq!(
            caps["textDocument"]["hover"]["contentFormat"],
            json!(["markdown", "plaintext"])
        );
        assert_eq!(caps["workspace"]["workspaceFolders"], true);
        assert_eq!(first["params"]["rootUri"], "file:///tmp/ws");
        c.open_file("file:///tmp/ws/a.go", "go", &Text::of("x"));
        assert_eq!(sent.borrow().len(), 1);
        let id = first["id"].as_u64().unwrap();
        cx.update(|cx| {
            c.receive_message(
                &json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": {"textDocumentSync": 2}}})
                    .to_string(),
                cx,
            )
        });
        cx.run_until_parked();
        let methods: Vec<String> = sent.borrow()[1..]
            .iter()
            .map(|m| m["method"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(methods, vec!["initialized", "textDocument/didOpen"]);
        drop(init);
        assert_eq!(c.has_capability("hoverProvider"), Some(false));
        cx.update(|cx| {
            c.receive_message(
                &json!({"jsonrpc": "2.0", "id": 7, "method": "custom/thing", "params": {}})
                    .to_string(),
                cx,
            )
        });
        let last = sent.borrow().last().cloned().unwrap();
        assert_eq!(last["error"]["code"], -32601);
    }

    #[gpui::test]
    fn publish_diagnostics_fills_problems_for_the_current_version(cx: &mut TestAppContext) {
        let hub = LspHub::default();
        let (c, _) = stub(cx, &hub, "ws-a", "file:///tmp/ws");
        hub.install_session(
            &ClientInfo {
                id: "sess-ws-a".into(),
                workspace_id: "ws-a".into(),
                root_uri: "file:///tmp/ws".into(),
                ..Default::default()
            },
            c.clone(),
            true,
        );
        c.open_file("file:///tmp/ws/src/a%20b.go", "go", &Text::of("x"));
        let msg = |version: i64| {
            json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
                "uri": "file:///tmp/ws/src/a%20b.go",
                "version": version,
                "diagnostics": [{
                    "range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 9}},
                    "severity": 2, "message": "unused", "source": "go", "code": "U1000",
                }],
            }})
            .to_string()
        };
        cx.update(|cx| c.receive_message(&msg(5), cx));
        assert!(cx.update(|cx| peek_problems(cx)).is_empty());
        cx.update(|cx| c.receive_message(&msg(0), cx));
        let probs = cx.update(|cx| peek_problems(cx));
        assert_eq!(probs.len(), 1);
        assert_eq!(probs[0].path, "src/a b.go");
        assert_eq!((probs[0].line, probs[0].character), (3, 4));
        assert_eq!(probs[0].severity, ProblemSeverity::Warning);
        assert_eq!(probs[0].session_id.as_deref(), Some("sess-ws-a"));
        cx.update(|cx| hub.drop_session("sess-ws-a", cx));
        assert!(cx.update(|cx| peek_problems(cx)).is_empty());
    }

    #[test]
    fn uris_round_trip_like_the_browser() {
        assert_eq!(
            join_root_uri("file:///tmp/ws", "src/a b#.go"),
            "file:///tmp/ws/src/a%20b%23.go"
        );
        assert_eq!(
            uri_to_rel_path("file:///tmp/ws/", "file:///tmp/ws/src/a%20b.go"),
            "src/a b.go"
        );
        assert_eq!(
            uri_to_rel_path("file:///tmp/ws", "file:///other/x.go"),
            "file:///other/x.go"
        );
    }

    #[test]
    fn symbols_flatten_depth_first() {
        let mut out = Vec::new();
        flatten_symbols(
            &[
                json!({
                    "name": "Server", "kind": 5,
                    "range": {"start": {"line": 0}, "end": {"line": 9}},
                    "selectionRange": {"start": {"line": 1}, "end": {"line": 1}},
                    "children": [{"name": "run", "kind": 6, "range": {"start": {"line": 3}, "end": {"line": 5}}}],
                }),
                json!({"name": "x", "kind": 14, "location": {"range": {"start": {"line": 11}, "end": {"line": 11}}}}),
            ],
            &mut out,
        );
        let got: Vec<(String, String, i64, i64)> = out
            .into_iter()
            .map(|s| (s.name, s.kind, s.line, s.end))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Server".into(), "class".into(), 2, 10),
                ("run".into(), "method".into(), 4, 6),
                ("x".into(), "const".into(), 12, 12),
            ]
        );
    }

    #[test]
    fn edits_apply_last_first_in_utf16_columns() {
        let out = apply_edits_to_text(
            "héllo world\nnext",
            &[
                range(0, 5, "HELLO"),
                json!({
                    "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 4}},
                    "newText": "NEXT",
                }),
            ],
        );
        assert_eq!(out, "HELLO world\nNEXT");
    }
}
