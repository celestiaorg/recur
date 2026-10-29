//! Running `[[prompt]]` actions outside the webview: shebang in-process,
//! TypeScript compiled to CJS and run with node (the Worker twin). The
//! engine's loop tasks use this; they must not call the UI Worker.

use std::collections::BTreeMap;

use recur_domain::path;
use serde_json::{Value, json};

use crate::env::PromptRunContext;
use crate::exec::{PromptResult, RunCtx, parse_prompt_result_json, run_prompt_shebang};
use crate::manifest::{PROMPT_TIMEOUT, PromptAction};
use crate::testrun::{NodeError, run_node};
use crate::{Error, err};

/// Dispatches a `[[prompt]]` by interpreter.
pub async fn run_prompt(
    ctx: &RunCtx,
    cache_dir: &str,
    prompt: &PromptAction,
    rc: PromptRunContext,
) -> Result<PromptResult, Error> {
    if crate::is_script_interpreter(&prompt.interpreter) {
        return run_prompt_type_script(ctx, cache_dir, prompt, &rc).await;
    }
    run_prompt_shebang(ctx, prompt, rc).await
}

/// Mirrors `frontend/src/lib/macro-worker.ts` PROMPT_WORKER_BOOT.
const NODE_PROMPT_BOOT: &str = r#"
const fs = require("fs");
const codePath = process.argv[2];
const ctxPath = process.argv[3];
const fallbackName = process.argv[4] || "";
const code = fs.readFileSync(codePath, "utf8");
const ctx = JSON.parse(fs.readFileSync(ctxPath, "utf8"));
(async () => {
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const runner = new AsyncFunction(
    "ctx",
    "const module = { exports: {} };\n" +
      "const exports = module.exports;\n" +
      String(code) +
      "\n" +
      "const __fn = module.exports.default || module.exports;\n" +
      "if (typeof __fn !== 'function') throw new Error('prompt must export default function (ctx)');\n" +
      "return await __fn(ctx);"
  );
  const frozen = Object.freeze(Object.assign({}, ctx));
  const raw = await runner(frozen);
  if (!raw || typeof raw !== "object") {
    throw new Error("prompt must return { prompt, display? }");
  }
  const prompt = raw.prompt != null ? String(raw.prompt) : "";
  process.stdout.write(JSON.stringify({ prompt: prompt, display: raw.display != null ? String(raw.display) : "" }));
})().catch((err) => {
  const message = err && err.message ? String(err.message) : String(err);
  process.stderr.write(message);
  process.exit(1);
});
"#;

/// Compiles a TypeScript prompt and runs it with node.
pub async fn run_prompt_type_script(
    ctx: &RunCtx,
    cache_dir: &str,
    prompt: &PromptAction,
    rc: &PromptRunContext,
) -> Result<PromptResult, Error> {
    if !crate::is_script_interpreter(&prompt.interpreter) {
        return Err(err!("prompt {:?} is not typescript", prompt.id));
    }
    let compiled = crate::compile_prompt_macro(cache_dir, prompt)?;
    let timeout = if prompt.timeout_ms > 0 {
        std::time::Duration::from_millis(prompt.timeout_ms as u64)
    } else {
        PROMPT_TIMEOUT
    };
    let ctx = ctx.or_timeout(timeout);
    let ctx_json = serde_json::to_string(&prompt_node_ctx(rc)).map_err(|e| err!("{e}"))?;
    let run = run_node(
        &ctx,
        "recur-prompt-ts-",
        &[
            ("prompt.js", compiled.js.as_bytes()),
            ("boot.js", NODE_PROMPT_BOOT.as_bytes()),
            ("ctx.json", ctx_json.as_bytes()),
        ],
        &["boot.js", "prompt.js", "ctx.json"],
        &[&prompt.name],
    )
    .await;
    let failed = |e: String| {
        if let Some(why) = ctx.err() {
            return err!("prompt cancelled or timed out: {why}");
        }
        err!("node: {e} (is node on PATH?)")
    };
    let run = match run {
        Ok(run) => run,
        Err(NodeError::Setup(e)) => return Err(err!("{e}")),
        Err(NodeError::Exec(e)) => return Err(failed(e)),
    };
    if let Some(code) = run.failed_code {
        if let Some(why) = ctx.err() {
            return Err(err!("prompt cancelled or timed out: {why}"));
        }
        return Err(err!(
            "prompt {:?} exited {code}: {}",
            prompt.id,
            String::from_utf8_lossy(&run.stderr)
        ));
    }
    parse_prompt_result_json(&String::from_utf8_lossy(&run.stdout), &prompt.name)
}

fn prompt_node_ctx(rc: &PromptRunContext) -> BTreeMap<&'static str, Value> {
    let file_name = if rc.file_name.is_empty() && !rc.file_path.is_empty() {
        path::base(&rc.file_path)
    } else {
        rc.file_name.clone()
    };
    BTreeMap::from([
        ("workspaceId", json!(rc.workspace_id)),
        ("projectId", json!(rc.project_id)),
        ("worktree", json!(rc.worktree)),
        ("port", json!(rc.port)),
        ("selectedText", json!(rc.selected_text)),
        ("document", json!(rc.document)),
        ("currentLine", json!(rc.current_line)),
        ("lineNumber", json!(rc.line_number)),
        ("column", json!(rc.column)),
        ("currentWord", json!(rc.current_word)),
        ("filepath", json!(rc.file_path)),
        ("filename", json!(file_name)),
        ("scope", json!(rc.scope)),
        ("clipboard", json!(rc.clipboard)),
        (
            "tabSize",
            json!(if rc.tab_size == 0 { 4 } else { rc.tab_size }),
        ),
        ("softTabs", json!(rc.soft_tabs)),
    ])
}
