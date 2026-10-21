//! Port of `langdata_test.go`.

use recur_bundles::*;

use crate::common::*;

#[test]
fn parse_manifest_language_data_only() {
    let (_g, dir) = temp_dir();
    let toml = r##"[bundle]
name = "LangData"
id = "demo-langdata-1"
version = "1.0.0"

[[languageData]]
id = "demo.langdata.yaml"
scopeName = "source.yaml"
lineComment = "#"
disableIndentCorrections = true
increaseIndentPattern = "^\\s*.*:$"
closeBrackets = ["<"]
"##;
    let parsed = parse_manifest_full(&dir, toml.as_bytes()).unwrap();
    assert!(parsed.commands.is_empty() && parsed.grammars.is_empty());
    assert_eq!(parsed.language_data.len(), 1);
    let d = &parsed.language_data[0];
    assert_eq!(d.id, "demo.langdata.yaml");
    assert_eq!(d.scope_name, "source.yaml");
    assert_eq!(d.line_comment, "#");
    assert!(d.disable_indent_corrections);
    assert_eq!(d.increase_indent_pattern, r"^\s*.*:$");
    assert_eq!(d.close_brackets, ["<"]);
}

#[test]
fn parse_manifest_language_data_rejects() {
    let (_g, dir) = temp_dir();
    for (name, toml, want) in [
        (
            "empty payload",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[languageData]]\nid = \"x\"\nscopeName = \"source.x\"\n",
            "comment tokens, indent",
        ),
        (
            "block unpaired",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[languageData]]\nid = \"x\"\nscopeName = \"source.x\"\nblockCommentStart = \"<!--\"\n",
            "both be set",
        ),
        (
            "closeBrackets not a char",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[languageData]]\nid = \"x\"\nscopeName = \"source.x\"\ncloseBrackets = [\"<>\"]\n",
            "single character",
        ),
        (
            "dup scope",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[languageData]]\nid = \"x\"\nscopeName = \"source.x\"\nlineComment = \"#\"\n[[languageData]]\nid = \"y\"\nscopeName = \"source.x\"\nlineComment = \"//\"\n",
            "duplicate scopeName",
        ),
    ] {
        let err = parse_manifest_full(&dir, toml.as_bytes()).unwrap_err();
        assert!(err.0.contains(want), "{name}: want {want:?} in {err}");
    }
}

#[test]
fn discover_catalog_user_overrides_language_data_by_id() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    must_write(
        &format!("{def}/LD/bundle.toml"),
        "[bundle]\nname = \"LD\"\nid = \"ld-default-1\"\n\n[[languageData]]\nid = \"demo.langdata\"\nscopeName = \"source.demo\"\nlineComment = \"#\"\n",
    );
    must_write(
        &format!("{user}/LD/bundle.toml"),
        "[bundle]\nname = \"LD\"\nid = \"ld-user-1\"\n\n[[languageData]]\nid = \"demo.langdata\"\nscopeName = \"source.demo\"\nlineComment = \"//\"\n",
    );
    let cat = discover_catalog(&DiscoverRoots {
        user_dir: user,
        default_dir: def,
    })
    .unwrap();
    let d = find_language_data_by_id(&cat.language_data, "demo.langdata").expect("missing");
    assert_eq!(d.line_comment, "//");
    assert_eq!(d.source_root, "user");
}

#[test]
fn resolve_language_data_prefix() {
    let all = vec![
        LanguageData {
            id: "src".into(),
            scope_name: "source".into(),
            line_comment: "//".into(),
            ..Default::default()
        },
        LanguageData {
            id: "py".into(),
            scope_name: "source.python".into(),
            line_comment: "#".into(),
            ..Default::default()
        },
    ];
    assert_eq!(
        resolve_language_data(&all, "source.go")
            .unwrap()
            .line_comment,
        "//"
    );
    assert_eq!(
        resolve_language_data(&all, "source.python")
            .unwrap()
            .line_comment,
        "#"
    );
    assert!(resolve_language_data(&all, "text.html.basic").is_none());
}

#[test]
fn shipped_language_data_comments() {
    let cat = discover_catalog(&DiscoverRoots {
        default_dir: shipped_root(),
        ..Default::default()
    })
    .unwrap();
    let check = |scope: &str, line: &str, start: &str, end: &str| {
        let d = resolve_language_data(&cat.language_data, scope)
            .unwrap_or_else(|| panic!("missing {scope}"));
        assert_eq!(
            (
                d.line_comment.as_str(),
                d.block_comment_start.as_str(),
                d.block_comment_end.as_str()
            ),
            (line, start, end),
            "{scope}"
        );
    };
    check("source.yaml", "#", "", "");
    check("source.dotenv", "#", "", "");
    check("text.html.basic", "", "<!--", "-->");
    let html = find_language_data_by_scope(&cat.language_data, "text.html.basic").unwrap();
    assert_eq!(html.close_brackets, ["<"]);
    let md = find_language_data_by_scope(&cat.language_data, "text.html.markdown").unwrap();
    assert_eq!(md.close_brackets.join(""), "<_*");
    check("source.go", "//", "/*", "*/");
    if let Some(d) = resolve_language_data(&cat.language_data, "source.json") {
        assert!(d.line_comment.is_empty() && d.block_comment_start.is_empty());
    }
    let py = resolve_language_data(&cat.language_data, "source.python").unwrap();
    assert!(!py.increase_indent_pattern.is_empty() && py.disable_indent_corrections);
    let mk = find_language_data_by_scope(&cat.language_data, "source.makefile").unwrap();
    assert!(!mk.increase_indent_pattern.is_empty() && mk.disable_indent_corrections);
    for d in &cat.language_data {
        for p in [
            &d.increase_indent_pattern,
            &d.decrease_indent_pattern,
            &d.indent_next_line_pattern,
            &d.zero_indent_pattern,
        ] {
            assert!(
                !p.contains("(?x)") && !p.contains("(?i)"),
                "{}: {p:?}",
                d.id
            );
        }
    }
}
