//! Scope and file-glob matching.

use recur_bundles::*;

fn cmd(scope: &str, glob: &str) -> Command {
    Command {
        scope: scope.into(),
        file_glob: glob.into(),
        ..Default::default()
    }
}

#[test]
fn matches_unscoped() {
    let c = cmd("", "");
    assert!(matches(&c, "", ""));
    assert!(matches(&c, "source.go", "main.go"));
}

#[test]
fn matches_grammar_prefix() {
    let c = cmd("source.go", "");
    assert!(matches(&c, "source.go", "a.go"));
    assert!(matches(&c, "source.go.string", "a.go"));
    assert!(!matches(&c, "source.python", "a.py"));
    assert!(!matches(&c, "", "a.go"));
}

#[test]
fn matches_file_glob() {
    let c = cmd("", "*.go");
    assert!(matches(&c, "", "pkg/main.go"));
    assert!(!matches(&c, "", "readme.md"));
    assert!(matches(
        &cmd("", "**/internal/*.go"),
        "",
        "foo/internal/x.go"
    ));
    let brace = cmd("", "*.{js,ts,tsx}");
    assert!(matches(&brace, "", "src/app.tsx"));
    assert!(!matches(&brace, "", "src/app.go"));
}

#[test]
fn match_file_glob_any_pattern() {
    assert!(match_file_glob(&[], "anything.go"));
    assert!(match_file_glob(&["*.go".into(), "*.mod".into()], "go.mod"));
    assert!(!match_file_glob(&["*.go".into()], "readme.md"));
}

#[test]
fn matches_scope_and_glob_together() {
    let c = cmd("source.go", "*.go");
    assert!(matches(&c, "source.go", "main.go"));
    assert!(!matches(&c, "source.go", "main.py"));
    assert!(!matches(&c, "source.python", "main.go"));
}

#[test]
fn matches_comma_scope_or() {
    let markup = cmd(
        "text.html, source.svelte, source.vue, source.tsx, source.js.jsx, source.astro",
        "*.{html,htm,svelte,vue,tsx,jsx,astro,mdx,hbs,handlebars}",
    );
    for (name, scope, path, want) in [
        ("svelte", "source.svelte", "App.svelte", true),
        ("html", "text.html.basic", "index.html", true),
        ("vue", "text.html.vue", "App.vue", true),
        ("tsx", "source.tsx", "App.tsx", true),
        ("jsx", "source.js.jsx", "App.jsx", true),
        ("astro", "source.astro", "index.astro", true),
        ("mdx", "text.html.markdown", "page.mdx", true),
        ("handlebars", "text.html.handlebars", "page.hbs", true),
        ("go", "source.go", "main.go", false),
        ("plain ts", "source.ts", "app.ts", false),
        ("plain js", "source.js", "app.js", false),
        ("markdown", "text.html.markdown", "readme.md", false),
        ("tsx glob miss", "source.tsx", "App.go", false),
    ] {
        assert_eq!(matches(&markup, scope, path), want, "{name}");
    }
}
