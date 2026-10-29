//! TypeScript compilation.

use recur_bundles::*;
use recur_domain::path;

use crate::common::*;

/// esbuild's location path for `<id>.ts`: relative to the working directory.
fn loc(id: &str) -> String {
    path::rel(&path::getwd().unwrap(), &format!("/{id}.ts")).unwrap()
}

fn ts(id: &str, body: &str) -> Command {
    Command {
        id: id.into(),
        interpreter: INTERPRETER_TYPESCRIPT.into(),
        body: body.into(),
        ..Default::default()
    }
}

#[test]
fn compile_macro_type_script_and_cache() {
    let (_g, cache) = temp_dir();
    let cmd = ts(
        "ts-upper",
        "\nimport { Output, type MacroContext, type MacroResult } from '@recur/sdk'\nexport default function (ctx: MacroContext): MacroResult {\n  return { text: ctx.input.toUpperCase(), output: Output.replaceSelection }\n}\n",
    );
    let r1 = compile_macro(&cache, &cmd).unwrap();
    assert!(!r1.cache_hit);
    assert!(r1.js.contains("toUpperCase"), "{}", r1.js);
    assert!(
        r1.js.contains("replaceSelection") && r1.js.contains("Object.freeze"),
        "{}",
        r1.js
    );
    let r2 = compile_macro(&cache, &cmd).unwrap();
    assert!(r2.cache_hit);
    assert_eq!(r1.js, r2.js);
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
}

#[test]
fn compile_macro_wraps_bare_body() {
    let (_g, cache) = temp_dir();
    let r = compile_macro(
        &cache,
        &ts("ts-bare", r#"return replaceEdited(ctx, ctx.input + "!")"#),
    )
    .unwrap();
    assert!(r.js.contains("replaceEdited"), "{}", r.js);
}

#[test]
fn compile_macro_rejects_other_imports() {
    let (_g, cache) = temp_dir();
    let err = compile_macro(
        &cache,
        &ts(
            "ts-bad",
            "\nimport fs from \"fs\"\nexport default (ctx: { input: string }) => { void fs; return ctx.input }\n",
        ),
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        format!(
            "compile \"ts-bad\": {}:2:15: import \"fs\" is not allowed (only \"@recur/sdk\")",
            loc("ts-bad")
        )
    );
}

#[test]
fn compile_macro_rejects_require_dynamic_import_and_missing_export() {
    let (_g, cache) = temp_dir();
    let err = compile_macro(
        &cache,
        &ts(
            "ts-req",
            "export default async (ctx: any) => { const a = require(\"os\"); const b = await import('path'); return ctx.input }\n",
        ),
    )
    .unwrap_err();
    let l = loc("ts-req");
    assert_eq!(
        err.0,
        format!(
            "compile \"ts-req\": {l}:1:55: import \"os\" is not allowed (only \"@recur/sdk\"); {l}:1:85: import \"path\" is not allowed (only \"@recur/sdk\")"
        )
    );
    let err = compile_macro(
        &cache,
        &ts(
            "ts-missing",
            "import { nope } from '@recur/sdk'\nexport default (ctx: any) => nope(ctx)\n",
        ),
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        format!(
            "compile \"ts-missing\": {}:1:9: No matching export in \"recur-sdk:@recur/sdk\" for import \"nope\"",
            loc("ts-missing")
        )
    );
    let err = compile_macro(
        &cache,
        &ts(
            "ts-tla",
            "await Promise.resolve(1)\nexport default () => 'x'\n",
        ),
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        format!(
            "compile \"ts-tla\": {}:1:0: Top-level await is not available in the configured target environment (\"es2020\")",
            loc("ts-tla")
        )
    );
}

#[test]
fn compile_macro_sdk_covers_output_constants() {
    for v in [
        OUTPUT_DISCARD,
        OUTPUT_REPLACE_SELECTION,
        OUTPUT_REPLACE_DOCUMENT,
        OUTPUT_REPLACE_INPUT,
        OUTPUT_INSERT_TEXT,
        OUTPUT_INSERT_SNIPPET,
        OUTPUT_NEW_DOCUMENT,
        OUTPUT_SHOW_AS_TOOLTIP,
        OUTPUT_COPY_TO_CLIPBOARD,
        OUTPUT_SHOW_AS_TOAST,
    ] {
        assert!(
            MACRO_SDK_SOURCE.contains(&format!("\"{v}\"")),
            "missing {v}"
        );
    }
}

fn normalize_sdk(s: &str) -> String {
    s.split('\n')
        .filter(|l| !l.trim().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[test]
fn macro_sdk_matches_authoring_copy() {
    let author = std::fs::read_to_string(repo_path("bundles/sdk/index.ts")).unwrap();
    assert_eq!(normalize_sdk(&author), normalize_sdk(MACRO_SDK_SOURCE));
}

#[test]
fn compile_macro_rejects_shebang() {
    let (_g, cache) = temp_dir();
    let cmd = Command {
        id: "x".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "true".into(),
        ..Default::default()
    };
    assert_eq!(
        compile_macro(&cache, &cmd).unwrap_err().0,
        "command \"x\": interpreter \"shebang\" is not typescript"
    );
}

#[test]
fn compile_macro_invalidates_on_edit() {
    let (_g, cache) = temp_dir();
    let mut cmd = ts(
        "ts-edit",
        "\nimport type { MacroContext } from '@recur/sdk'\nexport default (ctx: MacroContext) => ctx.input\n",
    );
    let r1 = compile_macro(&cache, &cmd).unwrap();
    cmd.body = "\nimport type { MacroContext } from '@recur/sdk'\nexport default (ctx: MacroContext) => ctx.input + \"x\"\n".into();
    let r2 = compile_macro(&cache, &cmd).unwrap();
    assert!(!r2.cache_hit);
    assert_ne!(r1.hash, r2.hash);
    let n = std::fs::read_dir(&cache)
        .unwrap()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("ts-edit.") && n.ends_with(".js")
        })
        .count();
    assert_eq!(n, 1);
}

#[test]
fn compile_hash_matches_go() {
    // A stable cache key so cache files stay addressable by the same name.
    let (_g, cache) = temp_dir();
    let r = compile_macro(&cache, &ts("h", "export default () => 'x'\n")).unwrap();
    assert_eq!(r.hash.len(), 32);
    use sha2::Digest;
    let key = format!("8\0typescript\0{MACRO_SDK_SOURCE}\0export default () => 'x'\n");
    assert_eq!(
        r.hash,
        hex::encode(&sha2::Sha256::digest(key.as_bytes())[..16])
    );
}
