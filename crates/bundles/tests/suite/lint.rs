//! Bundle lint.

use recur_bundles::*;

use crate::common::*;

#[test]
fn lint_bundles_key_equivalent() {
    let (_g, root) = temp_dir();
    let write_pack = |name: &str, toml: &str| {
        let dir = format!("{root}/{name}");
        must_write(
            &format!("{dir}/src/x.ts"),
            "import { replaceEdited, type MacroContext, type MacroResult } from '@recur/sdk'\nexport default function (ctx: MacroContext): MacroResult {\n  return replaceEdited(ctx, ctx.input)\n}\n",
        );
        must_write(&format!("{dir}/bundle.toml"), toml);
        dir
    };
    let a = write_pack(
        "A",
        "[bundle]\nname = \"A\"\nid = \"11111111-1111-4111-8111-111111111111\"\n[[command]]\nid = \"11111111-1111-4111-8111-111111111201\"\nname = \"Upper\"\ncategory = \"Text\"\ninterpreter = \"typescript\"\nscript = \"src/x.ts\"\nkeyEquivalent = \"cmd+shift+u\"\n",
    );
    let b = write_pack(
        "B",
        "[bundle]\nname = \"B\"\nid = \"22222222-2222-4222-8222-222222222222\"\n[[command]]\nid = \"22222222-2222-4222-8222-222222222201\"\nname = \"Also Upper\"\ncategory = \"Text\"\ninterpreter = \"typescript\"\nscript = \"src/x.ts\"\nkeyEquivalent = \"shift+cmd+u\"\n[[command]]\nid = \"22222222-2222-4222-8222-222222222202\"\nname = \"Save Clash\"\ncategory = \"Text\"\ninterpreter = \"typescript\"\nscript = \"src/x.ts\"\nkeyEquivalent = \"cmd+s\"\n[[command]]\nid = \"22222222-2222-4222-8222-222222222203\"\nname = \"Bad Chord\"\ncategory = \"Text\"\ninterpreter = \"typescript\"\nscript = \"src/x.ts\"\nkeyEquivalent = \"mod+u\"\n",
    );
    let (_g2, cache) = temp_dir();
    let rep = lint_bundles(&[a.clone(), b.clone()], &cache);
    assert!(!rep.ok());
    let text = format_lint_report(&rep, false);
    let want = format!(
        "error\tB/Save Clash\tkeyEquivalent \"cmd+s\" collides with app shortcut \"Save\" (binding dropped in-app)\n  {b}\n\
         error\tB/Bad Chord\tkeyEquivalent: \"mod+u\": unknown modifier \"mod\"\n  {b}\n\
         info\t\tkeyEquivalent \"shift+cmd+u\" shared by A/Upper, B/Also Upper (disambiguation menu at runtime)\n  {a}\n\
         2 errors, 1 info\n"
    );
    assert_eq!(text, want);
}

#[test]
fn lint_open_in_preview() {
    let (_g, root) = temp_dir();
    let dir = write_bundle(
        &root,
        "Prev",
        r#"
[bundle]
name = "Prev"
id = "33333333-3333-4333-8333-333333333333"
[[command]]
id = "33333333-3333-4333-8333-333333333301"
name = "Bad Output"
output = "replaceInput"
openInPreview = true
language = "html"
interpreter = "shebang"
body = '''
#!/bin/sh
cat
'''
[[command]]
id = "33333333-3333-4333-8333-333333333302"
name = "Bad Lang"
output = "newDocument"
openInPreview = true
language = "diff"
interpreter = "shebang"
body = '''
#!/bin/sh
cat
'''
[[command]]
id = "33333333-3333-4333-8333-333333333303"
name = "OK HTML"
output = "newDocument"
openInPreview = true
language = "html"
interpreter = "shebang"
body = '''
#!/bin/sh
cat
'''
"#,
    );
    let (_g2, cache) = temp_dir();
    let rep = lint_bundles(std::slice::from_ref(&dir), &cache);
    assert!(!rep.ok());
    let errors: Vec<&str> = rep
        .findings
        .iter()
        .filter(|f| f.severity == LintSeverity::Error)
        .map(|f| f.message.as_str())
        .collect();
    assert!(
        errors.contains(&"openInPreview requires output = \"newDocument\""),
        "{errors:?}"
    );
    assert!(
        errors.contains(
            &"openInPreview: language \"diff\" has no Preview chip (use html, htm, md, mdx, markdown, svg, mmd, or mermaid)"
        ),
        "{errors:?}"
    );
    assert!(
        !rep.findings
            .iter()
            .any(|f| f.command == "OK HTML" && f.severity == LintSeverity::Error)
    );
}

#[test]
fn previewable_language_cases() {
    for ok in [
        "html", "HTML", ".md", "markdown", "svg", "htm", "mdx", "mmd", "mermaid", ".mermaid",
    ] {
        assert!(previewable_language(ok), "{ok}");
    }
    for no in ["", "diff", "json", "ts", "txt"] {
        assert!(!previewable_language(no), "{no}");
    }
}

#[test]
fn lint_symbol_transform_oniguruma() {
    let (_g, root) = temp_dir();
    let dir = write_bundle(
        &root,
        "Sym",
        "[bundle]\nname = \"Sym\"\nid = \"33333333-3333-4333-8333-333333333333\"\n[[symbol]]\nid = \"sym.js-group\"\nscope = \"entity.name.function\"\ntransform = '''\ns/(?:foo)bar//\ns/(?=x)y//\n'''\n[[symbol]]\nid = \"sym.oni\"\nscope = \"entity.name.class\"\ntransform = '''\ns/(?x)foo//\n'''\n",
    );
    let (_g2, cache) = temp_dir();
    let rep = lint_bundles(&[dir], &cache);
    let oni: Vec<&str> = rep
        .findings
        .iter()
        .filter(|f| f.message.contains("Oniguruma"))
        .map(|f| f.command.as_str())
        .collect();
    assert_eq!(oni, ["sym.oni"]);
}

#[test]
fn lint_clean_bundle_reports_ok() {
    let (_g, root) = temp_dir();
    let dir = write_bundle(
        &root,
        "Ok",
        "[bundle]\nname = \"Ok\"\nid = \"ok\"\n[[command]]\nid = \"ok.c\"\nname = \"C\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (_g2, cache) = temp_dir();
    let rep = lint_bundles(&[dir], &cache);
    assert_eq!(format_lint_report(&rep, false), "ok\tno issues\n");
    assert_eq!(
        format_lint_report(&rep, true),
        "\x1b[32mok\tno issues\x1b[0m\n"
    );
}

#[test]
fn lint_reports_unparseable_bundle() {
    let (_g, root) = temp_dir();
    let dir = write_bundle(&root, "Bad", "[bundle]\nname = \"Bad\"\n");
    let (_g2, cache) = temp_dir();
    let rep = lint_bundles(std::slice::from_ref(&dir), &cache);
    assert_eq!(
        format_lint_report(&rep, false),
        format!("error\tBad\tbundle.toml: [bundle].id is required\n  {dir}\n1 errors, 0 info\n")
    );
}
