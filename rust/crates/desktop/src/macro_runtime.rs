//! The macro worker (`lib/macro-worker.ts`) on `node`: one long-lived worker per contract
//! (macro, prompt, complete) runs the CommonJS the backend compiled, inside a `vm` context
//! that exposes Worker-like globals only (no `require`, `process` or filesystem), so a module
//! sees nothing but the frozen context it is handed. Calls are NDJSON over stdio and may
//! overlap; a timeout (or a dropped prompt/complete call, the TS AbortSignal) kills the shared
//! worker and fails everything in flight, so a runaway module cannot starve later runs.

use std::collections::HashMap;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::FutureExt as _;
use parking_lot::Mutex;
use recur_bundles::PromptRunContext;
use recur_services::api::{
    PromptCompleteHitDto, PromptCompleteRequestDto, PromptCompleteResultDto, PromptResultDto,
};
use recur_view_chat::prompt_actions::{CompleteModuleRunner, PromptModuleRunner};
use recur_view_core::commands::{MacroContext, MacroResult, MacroRunner};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::runtime::Handle;
use tokio::sync::oneshot;

/// Shared by every worker: Worker-like globals in a fresh `vm` context, console to stderr
/// (stdout is the reply channel), and the CommonJS wrapper `macro-worker.ts` builds.
const BOOT_PRELUDE: &str = r#"
const readline = require("readline");
const util = require("util");
const vm = require("vm");
const out = process.stdout;
const err = process.stderr;
const send = (m) => out.write(JSON.stringify(m) + "\n");
const log = (...a) => err.write(util.format(...a) + "\n");
const consoleShim = { log, info: log, debug: log, warn: log, error: log, trace: log };
const globals = { console: consoleShim };
for (const n of [
  "setTimeout", "clearTimeout", "setInterval", "clearInterval", "queueMicrotask",
  "structuredClone", "URL", "URLSearchParams", "TextEncoder", "TextDecoder",
  "AbortController", "AbortSignal", "atob", "btoa", "fetch", "Headers", "Request",
  "Response", "Blob", "performance", "crypto",
]) {
  if (n in globalThis) globals[n] = globalThis[n];
}
const sandbox = vm.createContext(globals);
sandbox.self = sandbox;
const load = (code, what) => vm.runInContext(
  "(async function (ctx) {\n" +
  "const module = { exports: {} };\n" +
  "const exports = module.exports;\n" +
  String(code) + "\n" +
  "const __fn = module.exports.default || module.exports;\n" +
  "if (typeof __fn !== 'function') throw new Error('" + what + " must export default function (ctx) { … }');\n" +
  "return await __fn(ctx);\n})",
  sandbox,
);
const message = (e) => (e && e.message ? String(e.message) : String(e));
readline.createInterface({ input: process.stdin }).on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  run(msg).then(
    (r) => send(Object.assign({ id: msg.id, ok: true }, r)),
    (e) => send({ id: msg.id, ok: false, error: message(e) }),
  );
});
"#;

/// `WORKER_BOOT`.
const MACRO_BOOT: &str = r#"
async function run({ code, ctx }) {
  const frozen = Object.freeze(Object.assign({}, ctx, {
    prompts: Object.freeze(Object.assign({}, (ctx && ctx.prompts) || {})),
  }));
  const raw = await load(code, "macro")(frozen);
  let text = "";
  let output = undefined;
  let language = undefined;
  let openInPreview = undefined;
  if (typeof raw === "string") {
    text = raw;
  } else if (raw && typeof raw === "object") {
    text = raw.text != null ? String(raw.text) : "";
    if (raw.output != null) output = String(raw.output);
    if (raw.language != null && String(raw.language).trim()) language = String(raw.language).trim();
    if (typeof raw.openInPreview === "boolean") openInPreview = raw.openInPreview;
  } else if (raw != null) {
    throw new Error("macro must return a string or { text, output?, language?, openInPreview? }");
  }
  return { text, output, language, openInPreview };
}
"#;

/// `PROMPT_WORKER_BOOT`.
const PROMPT_BOOT: &str = r#"
async function run({ code, ctx, fallbackName }) {
  const raw = await load(code, "prompt")(Object.freeze(Object.assign({}, ctx)));
  if (!raw || typeof raw !== "object") {
    throw new Error("prompt must return { prompt, display? }");
  }
  const prompt = raw.prompt != null ? String(raw.prompt) : "";
  let display = raw.display != null ? String(raw.display) : "";
  display = display.replace(/[\r\n]+/g, " ").replace(/\s+/g, " ").trim();
  if (!display) display = String(fallbackName || "").replace(/[\r\n]+/g, " ").replace(/\s+/g, " ").trim();
  if (display.length > 120) display = display.slice(0, 119) + "…";
  return { prompt, display };
}
"#;

/// `COMPLETE_WORKER_BOOT`.
const COMPLETE_BOOT: &str = r#"
async function run({ code, ctx }) {
  const raw = await load(code, "complete")(Object.freeze(Object.assign({}, ctx)));
  const hits = [];
  const list = raw && Array.isArray(raw.hits) ? raw.hits : (Array.isArray(raw) ? raw : []);
  for (const h of list) {
    if (!h || typeof h !== "object") continue;
    const insert = h.insert != null ? String(h.insert) : "";
    const label = h.label != null ? String(h.label) : insert;
    const detail = h.detail != null ? String(h.detail) : "";
    const badge = h.badge != null ? String(h.badge) : "";
    const action = h.action != null ? String(h.action) : "";
    if (!label && !action) continue;
    hits.push({ insert, label, detail, badge, action });
    if (hits.length >= 25) break;
  }
  const message = raw && raw.message != null ? String(raw.message) : "";
  return { hits, message };
}
"#;

/// A workspace's LSP problems as `lspProblemArgs` lines.
pub type Diagnostics = Rc<dyn Fn(&str) -> Vec<String>>;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

struct Proc {
    child: Mutex<Child>,
    stdin: tokio::sync::Mutex<ChildStdin>,
    pending: Pending,
}

/// One worker contract (`getWorker` / `resetWorker` and friends).
struct Worker {
    /// "macro", "prompt" or "complete": the error wording's subject.
    what: &'static str,
    boot: String,
    proc: Mutex<Option<Arc<Proc>>>,
    seq: AtomicU64,
}

impl Worker {
    fn new(what: &'static str, body: &str) -> Self {
        Self {
            what,
            boot: format!("{body}\n{BOOT_PRELUDE}"),
            proc: Mutex::new(None),
            seq: AtomicU64::new(0),
        }
    }

    fn get(self: &Arc<Self>) -> Result<Arc<Proc>, String> {
        let mut slot = self.proc.lock();
        if let Some(p) = slot.as_ref() {
            return Ok(p.clone());
        }
        let mut child = tokio::process::Command::new("node")
            .arg("-e")
            .arg(&self.boot)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("node: {e} (is node on PATH?)"))?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(format!("{} worker error", self.what));
        };
        let proc = Arc::new(Proc {
            child: Mutex::new(child),
            stdin: tokio::sync::Mutex::new(stdin),
            pending: Pending::default(),
        });
        let what = self.what;
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!("{what} worker: {line}");
            }
        });
        let (me, reader) = (Arc::downgrade(self), proc.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let id = reply.get("id").and_then(Value::as_u64).unwrap_or_default();
                let Some(tx) = reader.pending.lock().remove(&id) else {
                    continue;
                };
                let _ = tx.send(if reply.get("ok") == Some(&Value::Bool(true)) {
                    Ok(reply)
                } else {
                    let msg = reply
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Err(if msg.is_empty() {
                        format!("{what} failed")
                    } else {
                        msg.to_owned()
                    })
                });
            }
            // The worker died (`onerror`): fail what is in flight and start fresh next call.
            if let Some(me) = me.upgrade() {
                me.reset_if(&reader, &format!("{what} worker error"));
            }
        });
        *slot = Some(proc.clone());
        Ok(proc)
    }

    /// Kills `proc` if it is still the live worker and fails everything in flight.
    fn reset_if(&self, proc: &Arc<Proc>, err: &str) {
        {
            let mut slot = self.proc.lock();
            if slot.as_ref().is_some_and(|p| Arc::ptr_eq(p, proc)) {
                *slot = None;
            }
        }
        let _ = proc.child.lock().start_kill();
        for (_, tx) in proc.pending.lock().drain() {
            let _ = tx.send(Err(err.to_owned()));
        }
    }

    /// Posts one call and waits for its reply, the timeout, or the worker dying. A call
    /// dropped before it settles (a cancelled prompt or complete) kills the worker.
    async fn call(
        self: Arc<Self>,
        mut msg: Map<String, Value>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let proc = self.get()?;
        let id = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        msg.insert("id".into(), json!(id));
        let (tx, rx) = oneshot::channel();
        proc.pending.lock().insert(id, tx);
        let mut guard = CancelGuard {
            worker: Some((self.clone(), proc.clone())),
            err: "prompt cancelled",
        };
        let mut line = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
        line.push('\n');
        if proc
            .stdin
            .lock()
            .await
            .write_all(line.as_bytes())
            .await
            .is_err()
        {
            guard.worker = None;
            proc.pending.lock().remove(&id);
            self.reset_if(&proc, &format!("{} worker error", self.what));
            return Err(format!("{} worker error", self.what));
        }
        let res = tokio::time::timeout(timeout, rx).await;
        guard.worker = None;
        match res {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(format!("{} worker error", self.what)),
            Err(_) => {
                let err = format!("{} timed out", self.what);
                self.reset_if(&proc, &err);
                Err(err)
            }
        }
    }
}

struct CancelGuard {
    worker: Option<(Arc<Worker>, Arc<Proc>)>,
    err: &'static str,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some((w, p)) = self.worker.take()
            && w.what != "macro"
        {
            w.reset_if(&p, self.err);
        }
    }
}

/// Aborts the backend task when the UI drops the call.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The three workers, living on the backend runtime.
pub struct MacroRuntime {
    handle: Handle,
    macros: Arc<Worker>,
    prompts: Arc<Worker>,
    completes: Arc<Worker>,
}

impl MacroRuntime {
    pub fn new(handle: Handle) -> Rc<Self> {
        Rc::new(Self {
            handle,
            macros: Arc::new(Worker::new("macro", MACRO_BOOT)),
            prompts: Arc::new(Worker::new("prompt", PROMPT_BOOT)),
            completes: Arc::new(Worker::new("complete", COMPLETE_BOOT)),
        })
    }

    fn spawn(
        &self,
        worker: &Arc<Worker>,
        msg: Map<String, Value>,
        timeout: Duration,
    ) -> futures::future::LocalBoxFuture<'static, Result<Value, String>> {
        let task = self.handle.spawn(worker.clone().call(msg, timeout));
        let guard = AbortOnDrop(task.abort_handle());
        async move {
            let res = task.await;
            drop(guard);
            res.unwrap_or_else(|e| Err(e.to_string()))
        }
        .boxed_local()
    }

    /// `runMacroModule` for `AppState::set_macro_runner`.
    pub fn macro_runner(self: &Rc<Self>) -> MacroRunner {
        let me = self.clone();
        Rc::new(move |code, ctx, timeout| {
            let timeout = if timeout.is_zero() {
                Duration::from_secs(10)
            } else {
                timeout
            };
            let msg = obj([("code", json!(code)), ("ctx", macro_ctx(&ctx))]);
            let call = me.spawn(&me.macros, msg, timeout);
            Box::pin(async move { call.await.map(|r| macro_result(&r)) })
        })
    }

    /// `runPromptModule` for `PromptCatalog::set_runners`.
    pub fn prompt_runner(self: &Rc<Self>) -> PromptModuleRunner {
        let me = self.clone();
        Rc::new(move |code, ctx, chat_code, name, timeout| {
            let timeout = if timeout.is_zero() {
                Duration::from_secs(5)
            } else {
                timeout
            };
            let msg = obj([
                ("code", json!(code)),
                ("ctx", prompt_ctx(&ctx, &chat_code)),
                ("fallbackName", json!(name)),
            ]);
            let call = me.spawn(&me.prompts, msg, timeout);
            async move {
                call.await.map(|r| PromptResultDto {
                    prompt: str_of(&r, "prompt"),
                    display: str_of(&r, "display"),
                    ..Default::default()
                })
            }
            .boxed_local()
        })
    }

    /// `runCompleteModule` for `PromptCatalog::set_runners`, with the workspace's LSP
    /// problems (`lspProblemArgs` lines) from `diagnostics`.
    pub fn complete_runner(
        self: &Rc<Self>,
        diagnostics: Diagnostics,
    ) -> CompleteModuleRunner {
        let me = self.clone();
        Rc::new(move |code, req, timeout| {
            let timeout = if timeout.is_zero() {
                Duration::from_secs(4)
            } else {
                timeout
            };
            let ctx = complete_ctx(&req, diagnostics(&req.workspace_id));
            let msg = obj([("code", json!(code)), ("ctx", ctx)]);
            let call = me.spawn(&me.completes, msg, timeout);
            async move { call.await.map(|r| complete_result(&r)) }.boxed_local()
        })
    }
}

fn obj<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// `MacroContext` as the worker receives it.
fn macro_ctx(c: &MacroContext) -> Value {
    json!({
        "input": c.input,
        "selectedText": c.selected_text,
        "document": c.document,
        "currentLine": c.current_line,
        "lineNumber": c.line_number,
        "column": c.column,
        "currentWord": c.current_word,
        "filepath": c.filepath,
        "filename": c.filename,
        "scope": c.scope,
        "clipboard": c.clipboard,
        "tabSize": c.tab_size,
        "softTabs": c.soft_tabs,
        "prompts": c.prompts,
    })
}

fn macro_result(r: &Value) -> MacroResult {
    MacroResult {
        text: str_of(r, "text"),
        output: str_of(r, "output"),
        language: str_of(r, "language"),
        open_in_preview: r.get("openInPreview").and_then(Value::as_bool),
    }
}

/// `PromptContext` as `runPromptAction` hands it to the worker.
fn prompt_ctx(c: &PromptRunContext, chat_code: &str) -> Value {
    json!({
        "workspaceId": c.workspace_id,
        "projectId": c.project_id,
        "worktree": c.worktree,
        "port": c.port,
        "selectedText": c.selected_text,
        "document": c.document,
        "currentLine": c.current_line,
        "lineNumber": c.line_number,
        "column": c.column,
        "currentWord": c.current_word,
        "filepath": c.file_path,
        "filename": c.file_name,
        "scope": c.scope,
        "clipboard": c.clipboard,
        "tabSize": c.tab_size,
        "softTabs": c.soft_tabs,
        "surface": c.surface,
        "args": c.args,
        "params": c.params,
        "chatCode": chat_code,
    })
}

/// `{ args, arg, params, diagnostics }` (`completePromptArgs`).
fn complete_ctx(req: &PromptCompleteRequestDto, diagnostics: Vec<String>) -> Value {
    let mut ctx = obj([
        ("args", json!(req.args)),
        ("diagnostics", json!(diagnostics)),
    ]);
    if !req.arg.is_empty() {
        ctx.insert("arg".into(), json!(req.arg));
    }
    if !req.params.is_empty() {
        ctx.insert("params".into(), json!(req.params));
    }
    Value::Object(ctx)
}

fn complete_result(r: &Value) -> PromptCompleteResultDto {
    let hits = r
        .get("hits")
        .and_then(Value::as_array)
        .map(|hits| {
            hits.iter()
                .map(|h| PromptCompleteHitDto {
                    insert: str_of(h, "insert"),
                    label: str_of(h, "label"),
                    detail: str_of(h, "detail"),
                    badge: str_of(h, "badge"),
                    action: str_of(h, "action"),
                    ..Default::default()
                })
                .collect()
        })
        .unwrap_or_default();
    PromptCompleteResultDto {
        hits: Some(hits),
        message: str_of(r, "message"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt() -> Rc<MacroRuntime> {
        MacroRuntime::new(Handle::current())
    }

    const CJS: &str = "module.exports = ";

    #[tokio::test(flavor = "multi_thread")]
    async fn macro_round_trip_and_routing() {
        let r = rt().macro_runner();
        let ctx = MacroContext {
            selected_text: "abc".into(),
            prompts: [("name".to_owned(), "x".to_owned())].into(),
            ..Default::default()
        };
        let code = format!(
            "{CJS}async (ctx) => ({{ text: ctx.selectedText.toUpperCase() + ctx.prompts.name, output: 'insertText', language: ' rust ', openInPreview: true }})"
        );
        let got = r(code, ctx.clone(), Duration::ZERO).await.unwrap();
        assert_eq!(
            got,
            MacroResult {
                text: "ABCx".into(),
                output: "insertText".into(),
                language: "rust".into(),
                open_in_preview: Some(true),
            }
        );
        let s = r(format!("{CJS}() => 'plain'"), ctx.clone(), Duration::ZERO)
            .await
            .unwrap();
        assert_eq!((s.text.as_str(), s.output.as_str()), ("plain", ""));
        let e = r(format!("{CJS}42"), ctx.clone(), Duration::ZERO)
            .await
            .unwrap_err();
        assert_eq!(e, "macro must export default function (ctx) { … }");
        let e = r(format!("{CJS}() => 7"), ctx, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(e.starts_with("macro must return a string"), "{e}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sandbox_hides_node_and_freezes_ctx() {
        let r = rt().macro_runner();
        let probe = format!(
            "{CJS}(ctx) => {{ console.log('to stderr'); ctx.input = 'changed'; return [typeof require, typeof process, typeof setTimeout, ctx.input, Object.isFrozen(ctx.prompts)].join(',') }}"
        );
        let got = r(probe, MacroContext::default(), Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(got.text, "undefined,undefined,function,,true");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn timeout_kills_the_worker_and_the_next_call_works() {
        let r = rt().macro_runner();
        let hang = format!("{CJS}() => new Promise(() => {{}})");
        let e = r(hang, MacroContext::default(), Duration::from_millis(200))
            .await
            .unwrap_err();
        assert_eq!(e, "macro timed out");
        let ok = r(
            format!("{CJS}() => 'again'"),
            MacroContext::default(),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(ok.text, "again");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn prompt_display_is_normalized() {
        let r = rt().prompt_runner();
        let ctx = PromptRunContext {
            args: "fix it".into(),
            ..Default::default()
        };
        let code = format!(
            "{CJS}(ctx) => ({{ prompt: 'do: ' + ctx.args + ' in ' + ctx.chatCode, display: '  two\\nlines  ' }})"
        );
        let got = r(
            code,
            ctx.clone(),
            "PA2".into(),
            "Name".into(),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(
            (got.prompt.as_str(), got.display.as_str()),
            ("do: fix it in PA2", "two lines")
        );
        let code = format!("{CJS}() => ({{ prompt: 'p' }})");
        let got = r(
            code,
            ctx.clone(),
            "PA".into(),
            "Fallback Name".into(),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(got.display, "Fallback Name");
        let e = r(
            format!("{CJS}() => 'x'"),
            ctx,
            "PA".into(),
            "N".into(),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert_eq!(e, "prompt must return { prompt, display? }");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn complete_hits_and_diagnostics() {
        let r = rt().complete_runner(Rc::new(|ws: &str| vec![format!("a.go:1:1 error: in {ws}")]));
        let req = PromptCompleteRequestDto {
            args: "ma".into(),
            workspace_id: "W1".into(),
            ..Default::default()
        };
        let code = format!(
            "{CJS}(ctx) => ({{ hits: [{{ insert: 'main' }}, {{ label: '', insert: '' }}, {{ insert: 'x', label: ctx.diagnostics[0] }}], message: ctx.args }})"
        );
        let got = r(code, req, Duration::ZERO).await.unwrap();
        let hits = got.hits.unwrap();
        assert_eq!(
            hits.iter()
                .map(|h| (h.insert.as_str(), h.label.as_str()))
                .collect::<Vec<_>>(),
            [("main", "main"), ("x", "a.go:1:1 error: in W1")]
        );
        assert_eq!(got.message, "ma");
    }
}
