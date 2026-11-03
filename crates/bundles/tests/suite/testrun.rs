//! Bundle test runs.

use recur_bundles::*;

use crate::common::*;

#[tokio::test(flavor = "multi_thread")]
async fn run_bundle_tests_type_script_and_shebang() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/bundle.toml"),
        r#"[bundle]
name = "Demo"
id = "demo-bundle"
version = "1.0.0"

[[command]]
id = "upper-ts"
name = "Upper"
interpreter = "typescript"
input = "selection"
output = "replaceSelection"
script = "src/upper.ts"

[[command]]
id = "echo-sh"
name = "Echo"
interpreter = "shebang"
input = "selection"
output = "replaceSelection"
body = '''
#!/bin/sh
printf 'X%sX' "$(cat)"
'''

[[command]]
id = "diagram-ts"
name = "Diagram"
interpreter = "typescript"
input = "none"
output = "newDocument"
script = "src/diagram.ts"

[[command]]
id = "manifest-preview"
name = "Manifest Preview"
interpreter = "typescript"
input = "none"
output = "newDocument"
language = "mermaid"
openInPreview = true
script = "src/plain.ts"
"#,
    );
    must_write(
        &format!("{dir}/src/upper.ts"),
        "\nimport { type MacroContext, type MacroResult } from '@recur/sdk'\nexport default function (ctx: MacroContext): MacroResult {\n  return { text: ctx.input.toUpperCase(), output: \"replaceSelection\" }\n}\n",
    );
    must_write(
        &format!("{dir}/src/diagram.ts"),
        "\nimport { type MacroContext, type MacroResult, Output } from '@recur/sdk'\nexport default function (_ctx: MacroContext): MacroResult {\n  return { text: \"graph TD; A-->B\", output: Output.newDocument, language: \"mermaid\", openInPreview: true }\n}\n",
    );
    must_write(
        &format!("{dir}/src/plain.ts"),
        "\nexport default function () { return \"graph TD; X-->Y\" }\n",
    );
    must_write(
        &format!("{dir}/tests/cases.toml"),
        r#"
[[test]]
name = "uppercases selection"
command = "upper-ts"
[test.context]
input = "Hi"
selectedText = "Hi"
[test.expect]
stdout = "HI"
output = "replaceSelection"

[[test]]
name = "shebang echoes"
command = "Echo"
[test.context]
input = "ab"
[test.expect]
stdout = "XabX"

[[test]]
name = "mermaid preview routing"
command = "diagram-ts"
[test.expect]
stdout = "graph TD; A-->B"
output = "newDocument"
language = "mermaid"
openInPreview = true

[[test]]
name = "manifest language and preview defaults"
command = "manifest-preview"
[test.expect]
stdout = "graph TD; X-->Y"
output = "newDocument"
language = "mermaid"
openInPreview = true
"#,
    );
    let (_g2, cache) = temp_dir();
    let rep = run_bundle_tests(&RunCtx::background(), &dir, &cache)
        .await
        .unwrap();
    assert!(rep.ok(), "{}", format_report(&rep, false));
    assert_eq!(rep.passed, 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn run_bundle_tests_failure_detail() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/bundle.toml"),
        "[bundle]\nname = \"Demo\"\nid = \"demo-bundle\"\nversion = \"1.0.0\"\n[[command]]\nid = \"upper-ts\"\nname = \"Upper\"\ninterpreter = \"typescript\"\nscript = \"src/upper.ts\"\n",
    );
    must_write(
        &format!("{dir}/src/upper.ts"),
        "\nexport default function (ctx) { return ctx.input.toUpperCase() }\n",
    );
    must_write(
        &format!("{dir}/tests/bad.toml"),
        "\n[[test]]\nname = \"wrong expect\"\ncommand = \"upper-ts\"\n[test.context]\ninput = \"a\"\n[test.expect]\nstdout = \"b\"\n",
    );
    let (_g2, cache) = temp_dir();
    let rep = run_bundle_tests(&RunCtx::background(), &dir, &cache)
        .await
        .unwrap();
    assert!(!rep.ok());
    assert_eq!(rep.failed, 1);
    assert_eq!(
        rep.results[0].detail,
        "stdout mismatch:\n got \"A\"\nwant \"b\""
    );
    assert_eq!(
        format_report(&rep, false),
        format!(
            "FAIL\tDemo\twrong expect\n  stdout mismatch:\n got \"A\"\nwant \"b\"\n  ({dir}/tests/bad.toml)\n0 passed, 1 failed\n"
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_bundle_tests_macro_errors_and_timeouts() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/bundle.toml"),
        "[bundle]\nname = \"Demo\"\nid = \"demo\"\n[[command]]\nid = \"boom\"\nname = \"Boom\"\ninterpreter = \"typescript\"\nbody = \"throw new Error('kaboom')\"\n[[command]]\nid = \"bad-out\"\nname = \"BadOut\"\ninterpreter = \"typescript\"\nbody = \"return { text: 'x', output: 'nope' }\"\n[[command]]\nid = \"snip-cmd\"\nname = \"S\"\nbody = \"#!/bin/sh\\nexit 0\\n\"\n[[snippet]]\nid = \"s1\"\nname = \"S1\"\ntabTrigger = \"s\"\nbody = \"hello\"\n",
    );
    must_write(
        &format!("{dir}/tests/t.toml"),
        "[[test]]\nname = \"throws\"\ncommand = \"boom\"\n[test.expect]\nexitCode = 1\nstderr = \"kaboom\"\n[[test]]\nname = \"bad routing\"\ncommand = \"bad-out\"\n[[test]]\nname = \"missing\"\ncommand = \"nope\"\n[[test]]\nname = \"snip ok\"\nsnippet = \"s1\"\n[test.expect]\nstdout = \"hello\"\n[[test]]\nname = \"snip bad\"\nsnippet = \"s1\"\n[test.expect]\nstdout = \"bye\"\n[[test]]\nname = \"snip missing\"\nsnippet = \"zz\"\n[test.expect]\nstdout = \"x\"\n",
    );
    let (_g2, cache) = temp_dir();
    let rep = run_bundle_tests(&RunCtx::background(), &dir, &cache)
        .await
        .unwrap();
    let details: Vec<(&str, bool, &str)> = rep
        .results
        .iter()
        .map(|r| (r.name.as_str(), r.ok, r.detail.as_str()))
        .collect();
    assert_eq!(
        details,
        [
            ("throws", true, ""),
            (
                "bad routing",
                false,
                "macro output: unknown output \"nope\""
            ),
            ("missing", false, "unknown command \"nope\""),
            ("snip ok", true, ""),
            (
                "snip bad",
                false,
                "body mismatch:\n got \"hello\"\nwant \"bye\""
            ),
            ("snip missing", false, "unknown snippet \"zz\""),
        ]
    );
}
