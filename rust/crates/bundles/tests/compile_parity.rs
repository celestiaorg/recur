//! Proves the swc compile path behaves exactly like Go's esbuild path.
//!
//! Every TypeScript file under repo `bundles/` is compiled twice: by
//! [`recur_bundles`] and by `tests/esbuild_ref` (Go `compile.go`'s esbuild
//! options, wrappers and `@recur/sdk` plugin, same esbuild version). Both
//! outputs run in one node driver with the Worker boot shape across many
//! contexts (each bundle's own test contexts plus synthetic ones), with
//! `Math.random` and `Date` made deterministic. Every result, return value
//! or thrown message, must be identical. Import-rule failures must produce
//! identical messages too.
//!
//! The reference binary is built from `tests/esbuild_ref` with `go` when
//! `RECUR_ESBUILD_REF` does not point at a prebuilt one; without Go the
//! test is skipped:
//!
//! ```sh
//! (cd rust/crates/bundles/tests/esbuild_ref && go build -o /tmp/esbuild_ref .)
//! RECUR_ESBUILD_REF=/tmp/esbuild_ref cargo test -p recur-bundles --test compile_parity -- --nocapture
//! ```

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command as Proc, Stdio};

use recur_bundles::*;
use serde_json::{Value, json};

fn repo_path(rel: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join(rel);
    go::clean(&p.to_string_lossy())
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Command,
    Prompt,
    Complete,
}

impl Kind {
    fn arg(self) -> &'static str {
        match self {
            Kind::Command => "command",
            Kind::Prompt => "prompt",
            Kind::Complete => "complete",
        }
    }
}

struct Entry {
    kind: Kind,
    id: String,
    source: String,
    file: String,
    contexts: Vec<Value>,
}

/// The esbuild reference compiler, built once per test binary.
fn reference_bin() -> Option<String> {
    static BIN: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        if let Ok(bin) = std::env::var("RECUR_ESBUILD_REF") {
            return Some(bin);
        }
        let out_dir =
            std::env::temp_dir().join(format!("recur-esbuild-ref-{}", std::process::id()));
        let bin = out_dir.join("esbuild_ref");
        let built = Proc::new("go")
            .args(["build", "-o"])
            .arg(&bin)
            .arg(".")
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/esbuild_ref"))
            .output();
        match built {
            Ok(o) if o.status.success() => Some(go::path_str(&bin)),
            Ok(o) => {
                eprintln!(
                    "skipped: building esbuild_ref failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                );
                None
            }
            Err(e) => {
                eprintln!("skipped: go not available ({e})");
                None
            }
        }
    })
    .clone()
}

fn reference_compile(bin: &str, kind: Kind, id: &str, source: &str) -> Result<String, String> {
    let mut child = Proc::new(bin)
        .args([kind.arg(), id, &repo_path("internal/bundles/macro_sdk.ts")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn esbuild_ref");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(source.as_bytes())
        .expect("write source");
    let out = child.wait_with_output().expect("esbuild_ref");
    if out.status.success() {
        Ok(String::from_utf8(out.stdout).expect("utf8"))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

const SAMPLE_INPUTS: &[&str] = &[
    "",
    "hello world",
    "Hello World\nsecond Line\n",
    "  indented\n\tTabbed line\n    four spaces\n",
    "{\"b\":1,\"a\":[1,2,{\"c\":null}],\"s\":\"x\\ny\"}",
    "{\n  // comment\n  \"a\": 1,\n}\n",
    "1 2 3\n4.5\n-7\n",
    "ünïcödé 日本語 😀 Ωmega",
    "a,b;c d\te",
    "<div class=\"x\">a &amp; b</div>",
    "foo_bar-baz quxQuux HTTPServer",
    "0x1F 255 0b101 0o17 3.14e2",
    "https://example.com/a b?c=d&e=f#g",
    "line1\r\nline2\r\n",
    "# Heading\n\n- item\n- item 2\n\n1. one\n2. two\n",
    "x = 1\nlonger_name = 2\ny=3\n",
    "'single' \"double\" `back` “smart” ‘quotes’",
    "SGVsbG8gd29ybGQ=",
    "%E6%97%A5%20a+b",
    "key: value\nlist:\n  - b\n  - a\nz: 1\n",
    "(1 + 2) * 3 / 4 - 5 ^ 2",
    "the quick brown fox jumps over the lazy dog the end",
    "b\na\nc\na\nB\n",
];

fn command_contexts(tests: &[LoadedTest], id: &str, name: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for t in tests {
        if t.case.command != id && t.case.command != name {
            continue;
        }
        let c = &t.case.context;
        let input = if c.input.is_empty() {
            c.selected_text.clone()
        } else {
            c.input.clone()
        };
        out.push(json!({
            "input": input,
            "selectedText": c.selected_text,
            "document": if c.document.is_empty() { input.clone() } else { c.document.clone() },
            "currentLine": c.current_line,
            "lineNumber": c.line_number,
            "column": c.column,
            "currentWord": if c.current_word.is_empty() { input.clone() } else { c.current_word.clone() },
            "filepath": c.file_path,
            "filename": c.file_name,
            "scope": c.scope,
            "clipboard": c.clipboard,
            "tabSize": if c.tab_size == 0 { 4 } else { c.tab_size },
            "softTabs": c.soft_tabs,
            "prompts": c.prompts,
        }));
    }
    for (i, s) in SAMPLE_INPUTS.iter().enumerate() {
        for selected in [true, false] {
            out.push(json!({
                "input": s,
                "selectedText": if selected { *s } else { "" },
                "document": s,
                "currentLine": s.lines().next().unwrap_or(""),
                "lineNumber": i + 1,
                "column": i,
                "currentWord": s.split_whitespace().next().unwrap_or(""),
                "filepath": "/work/src/file.md",
                "filename": "file.md",
                "scope": "text.html.markdown",
                "clipboard": SAMPLE_INPUTS[(i + 1) % SAMPLE_INPUTS.len()],
                "tabSize": if selected { 2 } else { 4 },
                "softTabs": selected,
                "prompts": {"width": "20", "prefix": "> ", "column": "40"},
            }));
        }
    }
    out
}

const SURFACES: [&str; 3] = ["workspace", "local_pr", "github_pr"];

fn prompt_contexts() -> Vec<Value> {
    let mut out = Vec::new();
    for (i, s) in SAMPLE_INPUTS.iter().enumerate() {
        out.push(json!({
            "workspaceId": "ws-1",
            "projectId": "p-1",
            "worktree": "/work",
            "port": 3000 + i,
            "selectedText": s,
            "document": s,
            "currentLine": s.lines().next().unwrap_or(""),
            "lineNumber": i,
            "column": 0,
            "currentWord": "",
            "filepath": "/work/main.go",
            "filename": "main.go",
            "scope": "source.go",
            "clipboard": "",
            "tabSize": 4,
            "softTabs": i % 2 == 0,
            "surface": SURFACES[i % 3],
            "args": s,
            "params": {"check": s, "problem": s},
        }));
    }
    out
}

fn complete_contexts() -> Vec<Value> {
    let diagnostics = [
        "src/main.go:12:5 error (gopls): undefined: foo",
        "src/lib.rs:3:1 warning (rust-analyzer): unused import",
        "web/app.ts:40:10 error (ts): Property 'x' does not exist",
    ];
    let mut out = Vec::new();
    for s in SAMPLE_INPUTS {
        for diags in [&diagnostics[..], &[][..]] {
            out.push(json!({
                "args": s,
                "arg": "problem",
                "params": {"problem": s},
                "diagnostics": diags,
            }));
        }
    }
    out
}

/// Runs `(code, ctx)` pairs in node exactly as the Worker boot would and
/// returns one JSON result per pair.
const DRIVER: &str = r#"
const fs = require("fs");
const jobs = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const RealDate = Date;
const FIXED = RealDate.UTC(2026, 9, 20, 12, 34, 56);
class FixedDate extends RealDate {
  constructor(...a) { if (a.length === 0) super(FIXED); else super(...a); }
  static now() { return FIXED; }
}
globalThis.Date = FixedDate;
let seed = 1;
Math.random = () => { seed = (seed * 16807) % 2147483647; return (seed - 1) / 2147483646; };
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
function describe(v) {
  if (typeof v === "undefined") return { type: "undefined" };
  if (typeof v === "function") return { type: "function" };
  if (typeof v === "string") return { type: "string", value: v };
  try { return { type: typeof v, value: JSON.parse(JSON.stringify(v)) }; } catch (e) { return { type: typeof v, value: String(v) }; }
}
(async () => {
  const out = [];
  for (const job of jobs) {
    seed = 1;
    const ctx = job.kind === "command"
      ? Object.freeze(Object.assign({}, job.ctx, { prompts: Object.freeze(Object.assign({}, job.ctx.prompts || {})) }))
      : Object.freeze(Object.assign({}, job.ctx));
    try {
      const runner = new AsyncFunction(
        "ctx",
        "const module = { exports: {} };\n" +
          "const exports = module.exports;\n" +
          String(job.code) +
          "\n" +
          "const __fn = module.exports.default || module.exports;\n" +
          "if (typeof __fn !== 'function') throw new Error('macro must export default function (ctx)');\n" +
          "return await __fn(ctx);"
      );
      out.push({ ok: describe(await runner(ctx)) });
    } catch (err) {
      out.push({ err: err && err.message ? String(err.message) : String(err), name: err && err.name });
    }
  }
  process.stdout.write(JSON.stringify(out));
})();
"#;

fn run_driver(jobs: &[Value]) -> Vec<Value> {
    let dir = tempfile::tempdir().expect("tempdir");
    let jobs_path = dir.path().join("jobs.json");
    let driver_path = dir.path().join("driver.js");
    std::fs::write(&jobs_path, serde_json::to_vec(jobs).expect("jobs")).expect("write jobs");
    std::fs::write(&driver_path, DRIVER).expect("write driver");
    let out = Proc::new("node")
        .arg(&driver_path)
        .arg(&jobs_path)
        .output()
        .expect("node");
    assert!(
        out.status.success(),
        "driver: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("driver output")
}

fn collect_entries() -> Vec<Entry> {
    let root = repo_path("bundles");
    let cat = discover_catalog(&DiscoverRoots {
        default_dir: root.clone(),
        ..Default::default()
    })
    .expect("catalog");
    let mut entries = Vec::new();
    let mut tests_by_dir = std::collections::HashMap::new();
    for c in cat
        .commands
        .iter()
        .filter(|c| is_script_interpreter(&c.interpreter))
    {
        let tests = tests_by_dir
            .entry(c.bundle_dir.clone())
            .or_insert_with(|| load_tests(&c.bundle_dir).expect("tests"));
        entries.push(Entry {
            kind: Kind::Command,
            id: c.id.clone(),
            source: c.body.clone(),
            file: c.script_path.clone(),
            contexts: command_contexts(tests, &c.id, &c.name),
        });
    }
    for p in &cat.prompts {
        if is_script_interpreter(&p.interpreter) {
            entries.push(Entry {
                kind: Kind::Prompt,
                id: p.id.clone(),
                source: p.body.clone(),
                file: p.script_path.clone(),
                contexts: prompt_contexts(),
            });
        }
        if p.complete_interpreter == INTERPRETER_TYPESCRIPT {
            entries.push(Entry {
                kind: Kind::Complete,
                id: p.id.clone(),
                source: p.complete_body.clone(),
                file: p.complete_path.clone(),
                contexts: complete_contexts(),
            });
        }
    }
    // The SDK authoring copy is the module both compilers inline; exercise
    // every export of it through a namespace import.
    let covered: BTreeSet<String> = entries.iter().map(|e| e.file.clone()).collect();
    for file in walk_ts(&root) {
        if covered.contains(&file) {
            continue;
        }
        let source = std::fs::read_to_string(&file).expect("read ts");
        assert_eq!(
            strip_comment_lines(&source),
            strip_comment_lines(MACRO_SDK_SOURCE),
            "{file} is not referenced by a manifest and is not the SDK"
        );
        entries.push(Entry {
            kind: Kind::Command,
            id: "sdk-probe".into(),
            source: SDK_PROBE.into(),
            file,
            contexts: command_contexts(&[], "", ""),
        });
    }
    entries
}

const SDK_PROBE: &str = r#"import * as sdk from "@recur/sdk"
import { Output, replaceEdited, stripJSONC, parseJSONC, type MacroContext } from "@recur/sdk"
export default function (ctx: MacroContext) {
  let parsed: unknown
  try { parsed = parseJSONC(ctx.input) } catch (e) { parsed = "error: " + (e as Error).message }
  return {
    text: JSON.stringify({
      keys: Object.keys(sdk).sort(),
      frozen: Object.isFrozen(Output),
      output: Output,
      edited: replaceEdited(ctx, ctx.input.toUpperCase()),
      stripped: stripJSONC(ctx.input),
      parsed,
      same: sdk.Output === Output,
    }),
  }
}
"#;

fn strip_comment_lines(s: &str) -> String {
    s.split('\n')
        .filter(|l| !l.trim().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn walk_ts(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("read dir")
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = go::path_str(&e.path());
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            out.extend(walk_ts(&p));
        } else if p.ends_with(".ts") {
            out.push(p);
        }
    }
    out
}

fn rust_compile(cache: &str, e: &Entry) -> Result<String, String> {
    let res = match e.kind {
        Kind::Command => compile_macro(
            cache,
            &Command {
                id: e.id.clone(),
                interpreter: INTERPRETER_TYPESCRIPT.into(),
                body: e.source.clone(),
                ..Default::default()
            },
        ),
        Kind::Prompt => compile_prompt_macro(
            cache,
            &PromptAction {
                id: e.id.clone(),
                interpreter: INTERPRETER_TYPESCRIPT.into(),
                body: e.source.clone(),
                ..Default::default()
            },
        ),
        Kind::Complete => compile_complete_macro(
            cache,
            &PromptAction {
                id: e.id.clone(),
                complete_interpreter: INTERPRETER_TYPESCRIPT.into(),
                complete_body: e.source.clone(),
                ..Default::default()
            },
        ),
    };
    res.map(|r| r.js).map_err(|e| e.0)
}

#[test]
fn swc_output_behaves_like_esbuild_for_every_shipped_ts_file() {
    if Proc::new("node").arg("--version").output().is_err() {
        eprintln!("skipped: node not on PATH");
        return;
    }
    let Some(bin) = reference_bin() else {
        return;
    };
    let cache = tempfile::tempdir().expect("cache");
    let cache = go::path_str(cache.path());
    let entries = collect_entries();
    let all_ts = walk_ts(&repo_path("bundles"));
    let files: BTreeSet<&str> = entries.iter().map(|e| e.file.as_str()).collect();
    for f in &all_ts {
        assert!(files.contains(f.as_str()), "{f} not exercised");
    }

    let mut jobs = Vec::new();
    let mut labels = Vec::new();
    for e in &entries {
        let ours = rust_compile(&cache, e).unwrap_or_else(|err| panic!("swc {}: {err}", e.id));
        let theirs = reference_compile(&bin, e.kind, &e.id, &e.source)
            .unwrap_or_else(|err| panic!("esbuild {}: {err}", e.id));
        for (i, ctx) in e.contexts.iter().enumerate() {
            for (side, code) in [("swc", &ours), ("esbuild", &theirs)] {
                jobs.push(json!({"kind": e.kind.arg(), "code": code, "ctx": ctx}));
                labels.push(format!("{} {} ctx#{i} [{side}]", e.kind.arg(), e.id));
            }
        }
    }
    let results = run_driver(&jobs);
    assert_eq!(results.len(), jobs.len());
    let mut mismatches = Vec::new();
    for (i, pair) in results.chunks(2).enumerate() {
        if pair[0] != pair[1] {
            mismatches.push(format!(
                "{}\n  swc:     {}\n  esbuild: {}",
                labels[i * 2],
                pair[0],
                pair[1]
            ));
        }
    }
    eprintln!(
        "compile parity: {} files, {} entries, {} executions compared",
        all_ts.len(),
        entries.len(),
        results.len() / 2
    );
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn import_rule_errors_match_esbuild() {
    let Some(bin) = reference_bin() else {
        return;
    };
    let cache = tempfile::tempdir().expect("cache");
    let cache = go::path_str(cache.path());
    for (id, src) in [
        (
            "fs-import",
            "\nimport fs from \"fs\"\nexport default (ctx: { input: string }) => { void fs; return ctx.input }\n",
        ),
        (
            "req",
            "export default async (ctx: any) => { const a = require(\"os\"); const b = await import('path'); return ctx.input }\n",
        ),
        (
            "missing",
            "import { nope } from '@recur/sdk'\nexport default (ctx: any) => nope(ctx)\n",
        ),
        (
            "default-import",
            "import sdk from '@recur/sdk'\nexport default (ctx: any) => sdk\n",
        ),
        ("side-effect", "import './x'\nexport default () => 1\n"),
        (
            "tla",
            "await Promise.resolve(1)\nexport default () => 'x'\n",
        ),
        (
            "for-await",
            "for await (const x of [1]) {}\nexport default () => 'x'\n",
        ),
        (
            "bare-body-import",
            "const x = require('child_process')\nreturn 'x'",
        ),
    ] {
        let e = Entry {
            kind: Kind::Command,
            id: id.into(),
            source: src.into(),
            file: String::new(),
            contexts: Vec::new(),
        };
        let ours = rust_compile(&cache, &e).expect_err("swc should reject");
        let theirs =
            reference_compile(&bin, Kind::Command, id, src).expect_err("esbuild should reject");
        assert_eq!(ours, format!("compile \"{id}\": {theirs}"), "{id}");
    }
    // Accepted by both: re-exports are never name-checked (they may be
    // types), and type-only imports of anything are erased.
    for (id, src) in [
        (
            "reexport",
            "export { nope } from '@recur/sdk'\nexport default () => 1\n",
        ),
        (
            "type-only",
            "import type { Nope } from 'whatever'\nexport default (x: Nope) => 1\n",
        ),
        ("unused", "import fs from 'fs'\nexport default () => 1\n"),
    ] {
        let e = Entry {
            kind: Kind::Command,
            id: id.into(),
            source: src.into(),
            file: String::new(),
            contexts: Vec::new(),
        };
        rust_compile(&cache, &e).unwrap_or_else(|err| panic!("swc {id}: {err}"));
        reference_compile(&bin, Kind::Command, id, src)
            .unwrap_or_else(|err| panic!("esbuild {id}: {err}"));
    }
}
