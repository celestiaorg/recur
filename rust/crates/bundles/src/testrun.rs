//! `recur bundles test`: runs a bundle's `tests/*.toml` through the real
//! engine (TypeScript under node as the Worker twin, shebang via the OS).

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::env::{RunContext, resolve_input};
use crate::exec::{DEFAULT_TIMEOUT, RunCtx, RunResult, run};
use crate::go::{self, quote};
use crate::grammar::{go_json_field, go_json_kind};
use crate::manifest::{Command, INTERPRETER_SHEBANG, parse_manifest_full, validate_output};
use crate::snippet::{Snippet, find_snippet_by_id};
use crate::testcase::{LoadedTest, TestContext, TestExpect, find_command, load_tests};
use crate::{Error, err};

/// One executed `[[test]]` outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaseResult {
    pub bundle: String,
    pub file: String,
    pub name: String,
    pub ok: bool,
    /// Failure explanation when `!ok`.
    pub detail: String,
}

/// Aggregated case results for a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub results: Vec<CaseResult>,
    pub passed: usize,
    pub failed: usize,
}

impl Report {
    /// Whether every case passed.
    pub fn ok(&self) -> bool {
        self.failed == 0
    }
}

/// Loads `bundle_dir`'s manifest plus `tests/*.toml` and executes each case.
/// `cache_dir` holds compiled TypeScript (a temp dir for one-off CLI runs).
pub async fn run_bundle_tests(
    ctx: &RunCtx,
    bundle_dir: &str,
    cache_dir: &str,
) -> Result<Report, Error> {
    let bundle_dir = go::abs(bundle_dir).map_err(|e| err!("{}", go::errno_text(&e)))?;
    let manifest = go::join(&[&bundle_dir, "bundle.toml"]);
    let data = go::read_file(&manifest).map_err(|e| err!("read bundle.toml: {e}"))?;
    let parsed = parse_manifest_full(&bundle_dir, &data)?;
    let tests = load_tests(&bundle_dir)?;
    let mut report = Report::default();
    for lt in tests {
        let cr = if !lt.case.snippet.trim().is_empty() {
            run_one_snippet_test(&parsed.meta.name, &parsed.snippets, &lt)
        } else {
            run_one_test(ctx, &parsed.meta.name, cache_dir, &parsed.commands, &lt).await
        };
        if cr.ok {
            report.passed += 1;
        } else {
            report.failed += 1;
        }
        report.results.push(cr);
    }
    Ok(report)
}

async fn run_one_test(
    ctx: &RunCtx,
    bundle_name: &str,
    cache_dir: &str,
    cmds: &[Command],
    lt: &LoadedTest,
) -> CaseResult {
    let mut cr = CaseResult {
        bundle: bundle_name.to_string(),
        file: lt.file.clone(),
        name: lt.case.name.clone(),
        ..Default::default()
    };
    let Some(cmd) = find_command(cmds, &lt.case.command) else {
        cr.detail = format!("unknown command {}", quote(&lt.case.command));
        return cr;
    };
    let res = match execute_test_command(ctx, cache_dir, cmd, &lt.case.context).await {
        Ok(res) => res,
        Err(e) => {
            cr.detail = e.0;
            return cr;
        }
    };
    if let Some(detail) = check_expect(&lt.case.expect, &res) {
        cr.detail = detail;
        return cr;
    }
    cr.ok = true;
    cr
}

fn run_one_snippet_test(bundle_name: &str, snippets: &[Snippet], lt: &LoadedTest) -> CaseResult {
    let mut cr = CaseResult {
        bundle: bundle_name.to_string(),
        file: lt.file.clone(),
        name: lt.case.name.clone(),
        ..Default::default()
    };
    let Some(sn) = find_snippet_by_id(snippets, &lt.case.snippet) else {
        cr.detail = format!("unknown snippet {}", quote(&lt.case.snippet));
        return cr;
    };
    let got = sn.body.trim();
    let want = lt.case.expect.stdout.trim();
    let skip = lt.case.expect.skip_stdout;
    if want.is_empty() && !skip {
        cr.detail = "snippet tests must set expect.stdout to the expansion body".to_string();
        return cr;
    }
    if !skip && got != want {
        cr.detail = format!("body mismatch:\n got {}\nwant {}", quote(got), quote(want));
        return cr;
    }
    cr.ok = true;
    cr
}

async fn execute_test_command(
    ctx: &RunCtx,
    cache_dir: &str,
    cmd: &Command,
    tc: &TestContext,
) -> Result<RunResult, Error> {
    if crate::is_script_interpreter(&cmd.interpreter) {
        return run_type_script_macro(ctx, cache_dir, cmd, tc).await;
    }
    if cmd.interpreter == INTERPRETER_SHEBANG {
        let rc = run_context_from_test(tc, &cmd.bundle_dir);
        return run(ctx, cmd, rc).await.map_err(|e| Error(e.message));
    }
    Err(err!(
        "interpreter {} cannot be tested",
        quote(&cmd.interpreter)
    ))
}

fn run_context_from_test(tc: &TestContext, bundle_dir: &str) -> RunContext {
    let input = tc.input.clone();
    let or_input = |s: &str| {
        if s.is_empty() {
            input.clone()
        } else {
            s.to_string()
        }
    };
    let parent = go::dir(bundle_dir);
    RunContext {
        selected_text: or_input(&tc.selected_text),
        current_line: tc.current_line.clone(),
        line_number: tc.line_number,
        column_number: tc.column,
        current_word: or_input(&tc.current_word),
        file_path: tc.file_path.clone(),
        file_name: tc.file_name.clone(),
        scope: tc.scope.clone(),
        tab_size: if tc.tab_size == 0 { 4 } else { tc.tab_size },
        soft_tabs: tc.soft_tabs,
        clipboard: tc.clipboard.clone(),
        bundle_support: bundle_dir.to_string(),
        input: input.clone(),
        prompt_values: tc.prompts.clone(),
        project_directory: parent.clone(),
        work_dir: parent,
        ..Default::default()
    }
}

fn check_expect(exp: &TestExpect, res: &RunResult) -> Option<String> {
    let want_exit = exp.exit_code.unwrap_or(0);
    if res.exit_code != want_exit {
        return Some(format!(
            "exitCode = {}, want {want_exit} (stderr={})",
            res.exit_code,
            quote(&res.stderr)
        ));
    }
    if !exp.skip_stdout
        && (!exp.stdout.is_empty() || !res.stdout.is_empty())
        && res.stdout != exp.stdout
    {
        return Some(format!(
            "stdout mismatch:\n got {}\nwant {}",
            quote(&res.stdout),
            quote(&exp.stdout)
        ));
    }
    if !exp.output.is_empty() && res.output != exp.output {
        return Some(format!(
            "output = {}, want {}",
            quote(&res.output),
            quote(&exp.output)
        ));
    }
    if !exp.stderr.is_empty() && res.stderr != exp.stderr {
        return Some(format!(
            "stderr = {}, want {}",
            quote(&res.stderr),
            quote(&exp.stderr)
        ));
    }
    if !exp.language.is_empty() && res.language != exp.language {
        return Some(format!(
            "language = {}, want {}",
            quote(&res.language),
            quote(&exp.language)
        ));
    }
    if let Some(want) = exp.open_in_preview {
        let got = res.open_in_preview.unwrap_or(false);
        if got != want {
            return Some(format!("openInPreview = {got}, want {want}"));
        }
    }
    None
}

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";

/// Human-readable summary. With `color`, status tokens use ANSI (green ok,
/// red FAIL, tinted summary).
pub fn format_report(r: &Report, color: bool) -> String {
    let paint = |code: &str, s: &str| {
        if !color || code.is_empty() {
            s.to_string()
        } else {
            format!("{code}{s}{ANSI_RESET}")
        }
    };
    let mut b = String::new();
    for c in &r.results {
        if c.ok {
            b.push_str(&format!(
                "{}\t{}\t{}\n",
                paint(ANSI_GREEN, "ok"),
                c.bundle,
                c.name
            ));
            continue;
        }
        b.push_str(&format!(
            "{}\t{}\t{}\n",
            paint(&format!("{ANSI_BOLD}{ANSI_RED}"), "FAIL"),
            c.bundle,
            c.name
        ));
        b.push_str(&format!("  {}\n", paint(ANSI_RED, &c.detail)));
        b.push_str(&format!(
            "  {}\n",
            paint(ANSI_DIM, &format!("({})", c.file))
        ));
    }
    let pass = format!("{} passed", r.passed);
    let fail = format!("{} failed", r.failed);
    if r.failed == 0 {
        b.push_str(&format!("{}, {fail}\n", paint(ANSI_GREEN, &pass)));
    } else {
        b.push_str(&format!(
            "{pass}, {}\n",
            paint(&format!("{ANSI_BOLD}{ANSI_RED}"), &fail)
        ));
    }
    b
}

/// Mirrors `frontend/src/lib/macro-worker.ts` WORKER_BOOT so CLI tests
/// exercise the same CJS + default-export contract as the app Worker.
const NODE_MACRO_BOOT: &str = r#"
const fs = require("fs");
const codePath = process.argv[2];
const ctxPath = process.argv[3];
const code = fs.readFileSync(codePath, "utf8");
const ctx = JSON.parse(fs.readFileSync(ctxPath, "utf8"));
(async () => {
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  // In this Go raw string, \n is two chars (backslash + n) so the written JS
  // source has real escape sequences — same shape as macro-worker WORKER_BOOT.
  const runner = new AsyncFunction(
    "ctx",
    "const module = { exports: {} };\n" +
      "const exports = module.exports;\n" +
      String(code) +
      "\n" +
      "const __fn = module.exports.default || module.exports;\n" +
      "if (typeof __fn !== 'function') throw new Error('macro must export default function (ctx)');\n" +
      "return await __fn(ctx);"
  );
  const frozen = Object.freeze(
    Object.assign({}, ctx, {
      prompts: Object.freeze(Object.assign({}, (ctx && ctx.prompts) || {})),
    })
  );
  const raw = await runner(frozen);
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
  process.stdout.write(JSON.stringify({ text, output, language, openInPreview }));
})().catch((err) => {
  const message = err && err.message ? String(err.message) : String(err);
  process.stderr.write(message);
  process.exit(1);
});
"#;

/// Outcome of a node run under a context (Go `exec.CommandContext`).
pub(crate) struct NodeRun {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `Some(code)` when node exited non-zero or was killed (-1); `None` on success.
    pub failed_code: Option<i64>,
}

/// Why [`run_node`] produced no [`NodeRun`].
pub(crate) enum NodeError {
    /// Temp dir or file setup failed (Go returns these unwrapped).
    Setup(String),
    /// node could not be started.
    Exec(String),
}

/// Writes `files` into a fresh temp dir and runs `node <file_args…>
/// <extra_args…>` (file args resolve inside that dir). A context that ends first SIGKILLs node, which
/// then reports exit -1 with whatever output it produced.
pub(crate) async fn run_node(
    ctx: &RunCtx,
    temp_prefix: &str,
    files: &[(&str, &[u8])],
    file_args: &[&str],
    extra_args: &[&str],
) -> Result<NodeRun, NodeError> {
    let dir = tempfile::Builder::new()
        .prefix(temp_prefix)
        .tempdir()
        .map_err(|e| NodeError::Setup(go::errno_text(&e)))?;
    let dir_path = go::path_str(dir.path());
    for (name, body) in files {
        let path = go::join(&[&dir_path, name]);
        crate::seed::write_file(&path, body, 0o644).map_err(NodeError::Setup)?;
    }
    let argv: Vec<String> = file_args
        .iter()
        .map(|a| go::join(&[&dir_path, a]))
        .chain(extra_args.iter().map(|a| a.to_string()))
        .collect();
    let node = which_node().ok_or_else(|| {
        NodeError::Exec("exec: \"node\": executable file not found in $PATH".to_string())
    })?;
    let mut child = tokio::process::Command::new(&node)
        .args(&argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| NodeError::Exec(go::path_err("fork/exec", &go::path_str(&node), &e)))?;
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let out_task = crate::exec::drain(child.stdout.take(), stdout.clone());
    let err_task = crate::exec::drain(child.stderr.take(), stderr.clone());
    let status = tokio::select! {
        status = child.wait() => status.ok(),
        _ = ctx.done() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            None
        }
    };
    let _ = out_task.await;
    let _ = err_task.await;
    let take = |buf: &Arc<Mutex<Vec<u8>>>| buf.lock().map(|b| b.clone()).unwrap_or_default();
    Ok(NodeRun {
        stdout: take(&stdout),
        stderr: take(&stderr),
        failed_code: match status {
            Some(s) if s.success() => None,
            Some(s) => Some(s.code().map_or(-1, i64::from)),
            None => Some(-1),
        },
    })
}

/// `node` resolved on `PATH` like Go's `exec.LookPath`.
fn which_node() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| {
            if dir.as_os_str().is_empty() {
                std::path::PathBuf::from(".").join("node")
            } else {
                dir.join("node")
            }
        })
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Compiles `cmd` and executes it with node (the Worker twin).
async fn run_type_script_macro(
    ctx: &RunCtx,
    cache_dir: &str,
    cmd: &Command,
    tc: &TestContext,
) -> Result<RunResult, Error> {
    let compiled = crate::compile_macro(cache_dir, cmd)?;
    let ctx = ctx.or_timeout(DEFAULT_TIMEOUT);
    let ctx_json = serde_json::to_string(&macro_ctx_from_test(tc, cmd)).map_err(|e| err!("{e}"))?;
    let run = run_node(
        &ctx,
        "recur-macro-test-",
        &[
            ("macro.js", compiled.js.as_bytes()),
            ("boot.js", NODE_MACRO_BOOT.as_bytes()),
            ("ctx.json", ctx_json.as_bytes()),
        ],
        &["boot.js", "macro.js", "ctx.json"],
        &[],
    )
    .await;
    let run = match run {
        Ok(run) => run,
        Err(NodeError::Setup(e)) => return Err(err!("{e}")),
        Err(NodeError::Exec(e)) => {
            if let Some(why) = ctx.err() {
                return Err(err!("typescript macro timed out: {why}"));
            }
            return Err(err!("node: {e} (is node on PATH?)"));
        }
    };
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    if let Some(code) = run.failed_code {
        return Ok(RunResult {
            stdout,
            stderr,
            exit_code: code,
            output: cmd.output.clone(),
            ..Default::default()
        });
    }
    let out = parse_node_macro_out(&stdout).map_err(|e| {
        err!(
            "parse node result: {e} (stdout={} stderr={})",
            quote(&stdout),
            quote(&stderr)
        )
    })?;
    let routing = if out.output.is_empty() {
        cmd.output.clone()
    } else {
        out.output
    };
    validate_output(&routing).map_err(|e| err!("macro output: {e}"))?;
    // Effective values match the app (commands.ts: result ?? cmd).
    let language = if out.language.is_empty() {
        cmd.language.clone()
    } else {
        out.language
    };
    Ok(RunResult {
        stdout: out.text,
        stderr,
        exit_code: 0,
        output: routing,
        language,
        open_in_preview: Some(out.open_in_preview.unwrap_or(cmd.open_in_preview)),
    })
}

struct NodeMacroOut {
    text: String,
    output: String,
    language: String,
    open_in_preview: Option<bool>,
}

fn parse_node_macro_out(stdout: &str) -> Result<NodeMacroOut, String> {
    let v: Value = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let mut out = NodeMacroOut {
        text: String::new(),
        output: String::new(),
        language: String::new(),
        open_in_preview: None,
    };
    let Value::Object(obj) = v else {
        if v.is_null() {
            return Ok(out);
        }
        return Err(format!(
            "json: cannot unmarshal {} into Go value of type bundles.nodeMacroOut",
            go_json_kind(&v)
        ));
    };
    let s = |name: &str| -> Result<String, String> {
        match go_json_field(&obj, name) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(other) => Err(format!(
                "json: cannot unmarshal {} into Go struct field nodeMacroOut.{name} of type string",
                go_json_kind(other)
            )),
        }
    };
    out.text = s("text")?;
    out.output = s("output")?;
    out.language = s("language")?;
    out.open_in_preview = match go_json_field(&obj, "openInPreview") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(other) => {
            return Err(format!(
                "json: cannot unmarshal {} into Go struct field nodeMacroOut.openInPreview of type bool",
                go_json_kind(other)
            ));
        }
    };
    Ok(out)
}

fn macro_ctx_from_test(tc: &TestContext, cmd: &Command) -> BTreeMap<&'static str, Value> {
    let input = if tc.input.is_empty() {
        resolve_input(cmd, &run_context_from_test(tc, &cmd.bundle_dir))
    } else {
        tc.input.clone()
    };
    let or_input = |s: &str| {
        if s.is_empty() {
            input.clone()
        } else {
            s.to_string()
        }
    };
    BTreeMap::from([
        ("input", json!(input)),
        ("selectedText", json!(tc.selected_text)),
        ("document", json!(or_input(&tc.document))),
        ("currentLine", json!(tc.current_line)),
        ("lineNumber", json!(tc.line_number)),
        ("column", json!(tc.column)),
        ("currentWord", json!(or_input(&tc.current_word))),
        ("filepath", json!(tc.file_path)),
        ("filename", json!(tc.file_name)),
        ("scope", json!(tc.scope)),
        ("clipboard", json!(tc.clipboard)),
        (
            "tabSize",
            json!(if tc.tab_size == 0 { 4 } else { tc.tab_size }),
        ),
        ("softTabs", json!(tc.soft_tabs)),
        ("prompts", json!(tc.prompts)),
    ])
}
