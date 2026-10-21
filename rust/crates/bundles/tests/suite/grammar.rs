//! Port of `grammar_test.go`.

use recur_bundles::*;

use crate::common::*;

fn parse(dir: &str) -> Result<ParsedBundle, Error> {
    let data = std::fs::read(format!("{dir}/bundle.toml")).unwrap();
    parse_manifest_full(dir, &data)
}

#[test]
fn parse_manifest_grammar_and_theme() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/syntaxes/demo.tmLanguage.json"),
        r#"{"scopeName":"source.demo","patterns":[]}"#,
    );
    must_write(
        &format!("{dir}/themes/demo.tmTheme.json"),
        r##"{"name":"Demo","tokenColors":[{"settings":{"foreground":"#fff","background":"#000"}}]}"##,
    );
    must_write(
        &format!("{dir}/bundle.toml"),
        r##"[bundle]
name = "DemoLang"
id = "demo-lang-0001"
version = "1.0.0"

[[grammar]]
id = "demo.grammar"
scopeName = "source.demo"
path = "syntaxes/demo.tmLanguage.json"
fileTypes = ["demo"]

[[theme]]
id = "demo"
name = "Demo"
path = "themes/demo.tmTheme.json"
uiTheme = "dark"
order = 42
preview = { bg = "#06071D", fg = "#E0E0E0", fn = "#B683CA", name = "#2BF1DC", string = "#8DFF8E" }
"##,
    );
    let parsed = parse(&dir).unwrap();
    assert_eq!(parsed.grammars.len(), 1);
    assert_eq!(parsed.grammars[0].scope_name, "source.demo");
    assert_eq!(parsed.grammars[0].id, "demo.grammar");
    assert_eq!(parsed.themes.len(), 1);
    let th = &parsed.themes[0];
    assert_eq!(
        (th.id.as_str(), th.name.as_str(), th.order),
        ("demo", "Demo", 42)
    );
    let p = &th.preview;
    assert_eq!(
        [&p.bg, &p.fg, &p.func, &p.name, &p.string],
        ["#06071D", "#E0E0E0", "#B683CA", "#2BF1DC", "#8DFF8E"]
    );
    assert!(parsed.commands.is_empty());
}

#[test]
fn parse_manifest_terminal_theme() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/themes/dracula.json"),
        r##"{
  "background": "#282a36",
  "foreground": "#f8f8f2",
  "cursor": "#f8f8f2",
  "selectionBackground": "rgba(68,71,90,0.6)",
  "red": "#ff5555",
  "green": "#50fa7b"
}"##,
    );
    must_write(
        &format!("{dir}/bundle.toml"),
        "[bundle]\nname = \"Term\"\nid = \"term-pack-1\"\n\n[[terminalTheme]]\nid = \"dracula\"\nname = \"Dracula\"\npath = \"themes/dracula.json\"\nuiTheme = \"dark\"\norder = 10\n",
    );
    let parsed = parse(&dir).unwrap();
    assert_eq!(parsed.terminal_themes.len(), 1);
    let th = &parsed.terminal_themes[0];
    assert_eq!(
        (th.id.as_str(), th.name.as_str(), th.order),
        ("dracula", "Dracula", 10)
    );
    assert_eq!(th.palette.background, "#282a36");
    assert_eq!(th.palette.foreground, "#f8f8f2");
    assert_eq!(th.palette.red, "#ff5555");
    assert_eq!(th.palette.selection_background, "rgba(68,71,90,0.6)");
}

#[test]
fn parse_manifest_terminal_theme_requires_bg_fg() {
    let (_g, dir) = temp_dir();
    must_write(&format!("{dir}/themes/bad.json"), r##"{"red":"#f00"}"##);
    must_write(
        &format!("{dir}/bundle.toml"),
        "[bundle]\nname = \"Term\"\nid = \"term-pack-bad\"\n\n[[terminalTheme]]\nid = \"bad\"\nname = \"Bad\"\npath = \"themes/bad.json\"\n",
    );
    let err = parse(&dir).unwrap_err();
    assert_eq!(
        err.0,
        "terminalTheme \"bad\": background and foreground are required"
    );
}

#[test]
fn discover_catalog_user_overrides_terminal_theme_by_id() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    must_write(
        &format!("{def}/Term/themes/x.json"),
        r##"{"background":"#000000","foreground":"#ffffff"}"##,
    );
    must_write(
        &format!("{def}/Term/bundle.toml"),
        "[bundle]\nname = \"Term\"\nid = \"term-default\"\n\n[[terminalTheme]]\nid = \"x\"\nname = \"Default\"\npath = \"themes/x.json\"\n",
    );
    must_write(
        &format!("{user}/Term/themes/x.json"),
        r##"{"background":"#111111","foreground":"#eeeeee"}"##,
    );
    must_write(
        &format!("{user}/Term/bundle.toml"),
        "[bundle]\nname = \"Term\"\nid = \"term-user\"\n\n[[terminalTheme]]\nid = \"x\"\nname = \"User\"\npath = \"themes/x.json\"\n",
    );
    let cat = discover_catalog(&DiscoverRoots {
        user_dir: user,
        default_dir: def,
    })
    .unwrap();
    let th = find_terminal_theme_by_id(&cat.terminal_themes, "x").expect("missing");
    assert_eq!(th.name, "User");
    assert_eq!(th.source_root, "user");
    assert_eq!(th.palette.background, "#111111");
}

#[test]
fn parse_manifest_rejects_bad_preview_color() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/themes/demo.tmTheme.json"),
        r#"{"name":"Demo"}"#,
    );
    let err = parse_manifest_full(
        &dir,
        b"[bundle]\nname = \"BadPreview\"\nid = \"bad-preview-0001\"\n\n[[theme]]\nid = \"bad-preview-theme\"\nname = \"Bad\"\npath = \"themes/demo.tmTheme.json\"\npreview = { bg = \"not-a-color\" }\n",
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        "theme \"bad-preview-theme\": preview.bg: must be a #hex colour"
    );
}

#[test]
fn parse_manifest_requires_grammar_id() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/syntaxes/demo.tmLanguage.json"),
        r#"{"scopeName":"source.demo","patterns":[]}"#,
    );
    let err = parse_manifest_full(
        &dir,
        b"[bundle]\nname = \"NoID\"\nid = \"noid.bundle\"\n\n[[grammar]]\nscopeName = \"source.demo\"\npath = \"syntaxes/demo.tmLanguage.json\"\n",
    )
    .unwrap_err();
    assert!(err.0.contains("id is required"), "{err}");
}

#[test]
fn parse_manifest_rejects_grammar_path_escape() {
    let (_g, dir) = temp_dir();
    let err = parse_manifest_full(
        &dir,
        b"[bundle]\nname = \"Bad\"\nid = \"bad-gram-0001\"\n\n[[grammar]]\nid = \"bad.grammar\"\nscopeName = \"source.bad\"\npath = \"../outside.tmLanguage.json\"\nfileTypes = [\"bad\"]\n",
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        "grammar \"source.bad\": path \"../outside.tmLanguage.json\" must be relative and must not contain .."
    );
}

#[test]
fn parse_manifest_missing_grammar_file_reports_stat_error() {
    let (_g, dir) = temp_dir();
    let err = parse_manifest_full(
        &dir,
        b"[bundle]\nname = \"G\"\nid = \"g\"\n[[grammar]]\nid = \"g.x\"\nscopeName = \"source.x\"\npath = \"syntaxes/x.tmLanguage.json\"\n",
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        format!(
            "grammar \"source.x\": read path \"syntaxes/x.tmLanguage.json\": stat {dir}/syntaxes/x.tmLanguage.json: no such file or directory"
        )
    );
}

#[test]
fn discover_catalog_user_overrides_grammar_by_id() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    for (root, id, lang) in [
        (&def, "lang-default-1", "default"),
        (&user, "lang-user-1", "user"),
    ] {
        must_write(
            &format!("{root}/Lang/syntaxes/demo.tmLanguage.json"),
            r#"{"scopeName":"source.demo","patterns":[]}"#,
        );
        must_write(
            &format!("{root}/Lang/bundle.toml"),
            &format!(
                "[bundle]\nname = \"Lang\"\nid = \"{id}\"\n\n[[grammar]]\nid = \"demo.grammar\"\nscopeName = \"source.demo\"\npath = \"syntaxes/demo.tmLanguage.json\"\nfileTypes = [\"demo\"]\nlanguage = \"{lang}\"\n"
            ),
        );
    }
    let cat = discover_catalog(&DiscoverRoots {
        user_dir: user.clone(),
        default_dir: def.clone(),
    })
    .unwrap();
    let g = find_grammar_by_id(&cat.grammars, "demo.grammar").expect("missing grammar by id");
    assert_eq!(g.language, "user");
    assert_eq!(g.source_root, "user");
    let by_scope = find_grammar_by_scope(&cat.grammars, "source.demo").expect("missing by scope");
    assert_eq!(by_scope.language, "user");
}

#[test]
fn grammar_theme_convention_helpers() {
    assert!(grammar_convention_ok("syntaxes/go.tmLanguage.json"));
    assert!(theme_convention_ok("themes/cool-glow.tmTheme.json"));
    assert!(!theme_convention_ok("themes/cool-glow-color-theme.json"));
    assert!(terminal_theme_convention_ok("themes/dracula.json"));
    assert!(!terminal_theme_convention_ok("themes/nord.tmTheme.json"));
}
