//! Port of `symbol_test.go`.

use recur_bundles::*;

use crate::common::*;

#[test]
fn parse_manifest_symbol_only() {
    let (_g, dir) = temp_dir();
    let parsed = parse_manifest_full(
        &dir,
        b"[bundle]\nname = \"Syms\"\nid = \"demo-syms-1\"\n\n[[symbol]]\nid = \"demo.func\"\nscope = \"entity.name.function\"\nkind = \"func\"\n",
    )
    .unwrap();
    assert!(parsed.commands.is_empty());
    assert_eq!(parsed.symbols.len(), 1);
    let s = &parsed.symbols[0];
    assert_eq!(
        (s.id.as_str(), s.scope.as_str(), s.kind.as_str()),
        ("demo.func", "entity.name.function", "func")
    );
    assert!(s.show);
}

#[test]
fn parse_manifest_symbol_show_false() {
    let (_g, dir) = temp_dir();
    let parsed = parse_manifest_full(
        &dir,
        b"[bundle]\nname = \"Syms\"\nid = \"demo-syms-hide\"\n\n[[symbol]]\nid = \"demo.hide\"\nscope = \"entity.name.function.support\"\nshow = false\n",
    )
    .unwrap();
    assert_eq!(parsed.symbols.len(), 1);
    assert!(!parsed.symbols[0].show);
}

#[test]
fn parse_manifest_symbol_rejects() {
    let (_g, dir) = temp_dir();
    for (name, toml, want) in [
        (
            "missing scope",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[symbol]]\nid = \"x\"\n",
            "scope is required",
        ),
        (
            "dup id",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[symbol]]\nid = \"x\"\nscope = \"entity.name.function\"\n[[symbol]]\nid = \"x\"\nscope = \"entity.name.type\"\n",
            "duplicate id",
        ),
    ] {
        let err = parse_manifest_full(&dir, toml.as_bytes()).unwrap_err();
        assert!(err.0.contains(want), "{name}: {err}");
    }
}

#[test]
fn discover_catalog_user_overrides_symbol_by_id() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    for (root, id, scope) in [
        (&def, "s-default", "entity.name.function"),
        (&user, "s-user", "entity.name.function.go"),
    ] {
        must_write(
            &format!("{root}/S/bundle.toml"),
            &format!(
                "[bundle]\nname = \"S\"\nid = \"{id}\"\n\n[[symbol]]\nid = \"demo.func\"\nscope = \"{scope}\"\nkind = \"func\"\n"
            ),
        );
    }
    let cat = discover_catalog(&DiscoverRoots {
        user_dir: user.clone(),
        default_dir: def.clone(),
    })
    .unwrap();
    let s = find_symbol_by_id(&cat.symbols, "demo.func").expect("missing");
    assert_eq!(s.scope, "entity.name.function.go");
    assert_eq!(s.source_root, "user");
}

#[test]
fn shipped_symbol_catalog() {
    let cat = discover_catalog(&DiscoverRoots {
        default_dir: shipped_root(),
        ..Default::default()
    })
    .unwrap();
    for id in [
        "recur.symbols.source.function",
        "recur.symbols.source.hide-calls",
        "recur.symbols.python.hide-decorator",
        "recur.symbols.markdown.heading",
        "recur.symbols.html.id",
        "recur.symbols.yaml.key",
        "recur.symbols.css.selector",
    ] {
        assert!(
            find_symbol_by_id(&cat.symbols, id).is_some(),
            "missing {id}"
        );
    }
    assert!(
        find_symbol_by_id(&cat.symbols, "recur.symbols.source.function")
            .unwrap()
            .show
    );
    assert!(
        !find_symbol_by_id(&cat.symbols, "recur.symbols.source.hide-calls")
            .unwrap()
            .show
    );
    let md = find_symbol_by_id(&cat.symbols, "recur.symbols.markdown.heading").unwrap();
    assert_eq!(md.scope, "markup.heading.markdown");
    assert!(!md.transform.is_empty());
}
