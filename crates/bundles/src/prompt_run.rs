//! Running `[[prompt]]` actions outside the UI: shebang in-process, TypeScript
//! compiled to CJS and run on a JavaScriptCore Worker of its own. The engine's
//! loop tasks use this; they must not call the UI's workers.

use std::collections::BTreeMap;

use recur_domain::path;
use serde_json::{Value, json};

use crate::env::PromptRunContext;
use crate::exec::{PromptResult, RunCtx, parse_prompt_result_json, run_prompt_shebang};
use crate::manifest::{PROMPT_TIMEOUT, PromptAction};
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

/// The prompt Worker contract without the UI's display normalization:
/// [`parse_prompt_result_json`] applies the fallback name.
const PROMPT_RUN_BOOT: &str = r#"
async function run({ code, ctx }) {
  const raw = await load(code, "prompt")(Object.freeze(Object.assign({}, ctx)));
  if (!raw || typeof raw !== "object") {
    throw new Error("prompt must return { prompt, display? }");
  }
  return {
    prompt: raw.prompt != null ? String(raw.prompt) : "",
    display: raw.display != null ? String(raw.display) : "",
  };
}
"#;

/// Compiles a TypeScript prompt and runs it.
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
    let msg = json!({ "code": compiled.js, "ctx": prompt_ctx(rc) });
    let out = tokio::select! {
        out = crate::js::run_once("prompt", PROMPT_RUN_BOOT, &msg, crate::js::UNTIL_CANCELLED) => out,
        why = ctx.done() => return Err(err!("prompt cancelled or timed out: {why}")),
    };
    match out.result {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.remove("ok");
            }
            parse_prompt_result_json(&v.to_string(), &prompt.name)
        }
        Err(e) => Err(err!("prompt {:?} exited 1: {}{e}", prompt.id, out.logs)),
    }
}

fn prompt_ctx(rc: &PromptRunContext) -> BTreeMap<&'static str, Value> {
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
