//! The macro workers on JavaScriptCore: one long-lived
//! [`JsWorker`] per contract (macro, prompt, complete) runs the CommonJS the backend
//! compiled with Worker globals only (no `require`, `process` or filesystem), so a module
//! sees nothing but the frozen context it is handed. A timeout, or a dropped call, stops the
//! call and gives that worker a fresh context.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt as _;
use recur_bundles::PromptRunContext;
use recur_bundles::js::{COMPLETE_BOOT, JsWorker, MACRO_BOOT, PROMPT_BOOT};
use recur_services::api::{
    PromptCompleteHitDto, PromptCompleteRequestDto, PromptCompleteResultDto, PromptResultDto,
};
use recur_view_chat::prompt_actions::{CompleteModuleRunner, PromptModuleRunner};
use recur_view_core::commands::{MacroContext, MacroResult, MacroRunner};
use serde_json::{Map, Value, json};
use tokio::runtime::Handle;

/// A workspace's LSP problems as `lspProblemArgs` lines.
pub type Diagnostics = Rc<dyn Fn(&str) -> Vec<String>>;

/// One worker contract (`getWorker` / `resetWorker` and friends).
struct Worker {
    /// "macro", "prompt" or "complete": the error wording's subject.
    what: &'static str,
    js: JsWorker,
}

impl Worker {
    fn new(what: &'static str, boot: &str) -> Self {
        Self {
            what,
            js: JsWorker::new(what, boot),
        }
    }

    async fn call(
        self: Arc<Self>,
        msg: Map<String, Value>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let out = self.js.call(&Value::Object(msg), timeout).await;
        for line in out.logs.lines() {
            tracing::debug!("{} worker: {line}", self.what);
        }
        out.result
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
    pub fn complete_runner(self: &Rc<Self>, diagnostics: Diagnostics) -> CompleteModuleRunner {
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
        "conventionalCommits": c.conventional_commits,
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
