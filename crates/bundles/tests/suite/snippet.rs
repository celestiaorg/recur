//! Bundle snippets.

use recur_bundles::*;

use crate::common::*;

#[test]
fn parse_manifest_snippet_only() {
    let (_g, dir) = temp_dir();
    let toml = "[bundle]\nname = \"Snips\"\nid = \"demo-snips-1\"\n\n[[snippet]]\nid = \"demo.ife\"\nname = \"If\"\ntabTrigger = \"ife\"\nscope = \"source.go\"\nbody = '''\nif ${1:condition} {\n\t$0\n}\n'''\n";
    let parsed = parse_manifest_full(&dir, toml.as_bytes()).unwrap();
    assert_eq!(parsed.snippets.len(), 1);
    assert_eq!(parsed.snippets[0].tab_trigger, "ife");
    assert!(parsed.snippets[0].body.contains("${1:condition}"));
}

#[test]
fn parse_manifest_snippet_rejects() {
    let (_g, dir) = temp_dir();
    let err = parse_manifest_full(
        &dir,
        b"[bundle]\nname = \"T\"\nid = \"b\"\n[[snippet]]\nid = \"x\"\nname = \"X\"\nbody = \"hi\"\n",
    )
    .unwrap_err();
    assert_eq!(err.0, "snippet \"x\": set tabTrigger and/or keyEquivalent");
    let err = parse_manifest_full(
        &dir,
        b"[bundle]\nname = \"T\"\nid = \"b\"\n[[snippet]]\nid = \"x\"\nname = \"X\"\ntabTrigger = \"9x\"\nbody = \"hi\"\n",
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        "snippet \"x\": tabTrigger \"9x\" must be a word (letter, then letters/digits/_)"
    );
}

#[test]
fn discover_catalog_user_overrides_snippet_by_id() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    for (root, id, body) in [(&def, "s-default", "default"), (&user, "s-user", "user")] {
        must_write(
            &format!("{root}/S/bundle.toml"),
            &format!(
                "[bundle]\nname = \"S\"\nid = \"{id}\"\n\n[[snippet]]\nid = \"demo.ife\"\nname = \"If\"\ntabTrigger = \"ife\"\nscope = \"source.go\"\nbody = \"{body}\"\n"
            ),
        );
    }
    let cat = discover_catalog(&DiscoverRoots {
        user_dir: user.clone(),
        default_dir: def.clone(),
    })
    .unwrap();
    let s = find_snippet_by_id(&cat.snippets, "demo.ife").expect("missing");
    assert_eq!(s.body, "user");
    assert_eq!(s.source_root, "user");
}

#[test]
fn snippet_matches_scope() {
    let s = Snippet {
        scope: "source.go".into(),
        tab_trigger: "ife".into(),
        ..Default::default()
    };
    assert!(snippet_matches(&s, "source.go", "main.go"));
    assert!(!snippet_matches(&s, "source.python", "main.py"));
}

#[test]
fn shipped_snippet_catalog() {
    let cat = discover_catalog(&DiscoverRoots {
        default_dir: shipped_root(),
        ..Default::default()
    })
    .unwrap();
    for id in [
        "recur.go.ife",
        "recur.javascript.log",
        "recur.python.def",
        "recur.shell.bash",
        "recur.html.wrap",
        "recur.source.quote",
    ] {
        assert!(
            find_snippet_by_id(&cat.snippets, id).is_some(),
            "missing {id}"
        );
    }
}
