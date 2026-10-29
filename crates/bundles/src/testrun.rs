//! `recur bundles test`: runs a bundle's `tests/*.toml` through the real
//! engine (TypeScript on the app's JavaScriptCore Worker, shebang via the OS).

use std::collections::BTreeMap;

use recur_domain::path;
use serde_json::{Value, json};

use crate::env::{RunContext, resolve_input};
use crate::exec::{DEFAULT_TIMEOUT, RunCtx, RunResult, run};
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
    let bundle_dir = path::abs(bundle_dir).map_err(|e| err!("{e}"))?;
    let manifest = path::join(&[&bundle_dir, "bundle.toml"]);
    let data = std::fs::read(&manifest).map_err(|e| err!("read {manifest}: {e}"))?;
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
        cr.detail = format!("unknown command {:?}", lt.case.command);
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
        cr.detail = format!("unknown snippet {:?}", lt.case.snippet);
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
        cr.detail = format!("body mismatch:\n got {:?}\nwant {:?}", got, want);
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
    Err(err!("interpreter {:?} cannot be tested", cmd.interpreter))
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
    let parent = path::dir(bundle_dir);
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
            "exitCode = {}, want {want_exit} (stderr={:?})",
            res.exit_code, res.stderr
        ));
    }
    if !exp.skip_stdout
        && (!exp.stdout.is_empty() || !res.stdout.is_empty())
        && res.stdout != exp.stdout
    {
        return Some(format!(
            "stdout mismatch:\n got {:?}\nwant {:?}",
            res.stdout, exp.stdout
        ));
    }
    if !exp.output.is_empty() && res.output != exp.output {
        return Some(format!("output = {:?}, want {:?}", res.output, exp.output));
    }
    if !exp.stderr.is_empty() && res.stderr != exp.stderr {
        return Some(format!("stderr = {:?}, want {:?}", res.stderr, exp.stderr));
    }
    if !exp.language.is_empty() && res.language != exp.language {
        return Some(format!(
            "language = {:?}, want {:?}",
            res.language, exp.language
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

/// Compiles `cmd` and runs it on the app's macro Worker contract. A throw is exit 1 with
/// the message on stderr; a context that ends first is exit -1.
async fn run_type_script_macro(
    ctx: &RunCtx,
    cache_dir: &str,
    cmd: &Command,
    tc: &TestContext,
) -> Result<RunResult, Error> {
    let compiled = crate::compile_macro(cache_dir, cmd)?;
    let ctx = ctx.or_timeout(DEFAULT_TIMEOUT);
    let msg = json!({ "code": compiled.js, "ctx": macro_ctx_from_test(tc, cmd) });
    let failed = |exit_code, stderr| RunResult {
        stderr,
        exit_code,
        output: cmd.output.clone(),
        ..Default::default()
    };
    let out = tokio::select! {
        out = crate::js::run_once("macro", crate::js::MACRO_BOOT, &msg, crate::js::UNTIL_CANCELLED) => out,
        _ = ctx.done() => return Ok(failed(-1, String::new())),
    };
    let raw = match out.result {
        Ok(raw) => raw,
        Err(e) => return Ok(failed(1, format!("{}{e}", out.logs))),
    };
    let s = |k: &str| {
        raw.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let routing = match s("output") {
        o if o.is_empty() => cmd.output.clone(),
        o => o,
    };
    validate_output(&routing).map_err(|e| err!("macro output: {e}"))?;
    // Effective values match the app (commands.ts: result ?? cmd).
    let language = match s("language") {
        l if l.is_empty() => cmd.language.clone(),
        l => l,
    };
    let open_in_preview = raw.get("openInPreview").and_then(Value::as_bool);
    Ok(RunResult {
        stdout: s("text"),
        stderr: out.logs,
        exit_code: 0,
        output: routing,
        language,
        open_in_preview: Some(open_in_preview.unwrap_or(cmd.open_in_preview)),
    })
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
