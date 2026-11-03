//! The bundle registry.

use recur_bundles::*;

use crate::common::*;

#[test]
fn parse_manifest_rejects_unknown_enums() {
    let (_g, dir) = temp_dir();
    for (name, toml, want) in [
        (
            "input",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[command]]\nid = \"c1\"\nname = \"X\"\ninput = \"documnet\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
            "unknown input",
        ),
        (
            "output",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[command]]\nid = \"c1\"\nname = \"X\"\noutput = \"replacedocument\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
            "unknown output",
        ),
        (
            "trigger",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[command]]\nid = \"c1\"\nname = \"X\"\ntriggers = [\"onSave\"]\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
            "unknown trigger",
        ),
        (
            "showAsHTML",
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[command]]\nid = \"c1\"\nname = \"X\"\noutput = \"showAsHTML\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
            "showAsHTML was removed",
        ),
    ] {
        let err = parse_manifest(&dir, toml.as_bytes()).unwrap_err();
        assert!(err.0.contains(want), "{name}: want {want:?} in {err}");
    }
}

#[test]
fn parse_manifest_rejects_native_and_js() {
    let (_g, dir) = temp_dir();
    for interp in ["native", "js"] {
        let toml = format!(
            "[bundle]\nname = \"T\"\nid = \"b\"\n[[command]]\nid = \"n1\"\nname = \"Upper\"\ninterpreter = \"{interp}\"\n"
        );
        let err = parse_manifest(&dir, toml.as_bytes()).unwrap_err();
        assert!(err.0.contains("not valid"), "{interp}: {err}");
    }
}

#[test]
fn parse_manifest_inline_and_script() {
    let (_g, dir) = temp_dir();
    must_write(&format!("{dir}/bin/hi.sh"), "#!/bin/sh\necho hi\n");
    let data = r#"
[bundle]
name = "Demo"
id = "bundle-demo"
version = "1"

[[command]]
id = "cmd-inline"
name = "Inline"
category = "Text"
body = '''
#!/bin/sh
echo inline
'''

[[command]]
id = "cmd-script"
name = "Script"
category = "Diff"
script = "bin/hi.sh"
"#;
    let (meta, cmds, _) = parse_manifest(&dir, data.as_bytes()).unwrap();
    assert_eq!(meta.name, "Demo");
    assert_eq!(meta.id, "bundle-demo");
    assert_eq!(cmds.len(), 2);
    assert!(cmds[0].body.contains("echo inline"));
    assert!(!cmds[1].script_path.is_empty() && cmds[1].body.contains("echo hi"));
}

#[test]
fn discover_user_overrides_default_by_id() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    write_bundle(
        &def,
        "Text",
        "[bundle]\nname = \"Text\"\nid = \"b-text\"\n[[command]]\nid = \"shared-cmd\"\nname = \"From Default\"\ncategory = \"Text\"\nbody = \"#!/bin/sh\\necho default\\n\"\n",
    );
    write_bundle(
        &user,
        "Text",
        "[bundle]\nname = \"Text\"\nid = \"b-text-user\"\n[[command]]\nid = \"shared-cmd\"\nname = \"From User\"\ncategory = \"Text\"\norder = 1\nbody = \"#!/bin/sh\\necho user\\n\"\n",
    );
    let (cmds, _) = discover(&DiscoverRoots {
        user_dir: user,
        default_dir: def,
    })
    .unwrap();
    let c = find_by_id(&cmds, "shared-cmd").expect("missing shared-cmd");
    assert_eq!(c.name, "From User");
    assert_eq!(c.source_root, "user");
}

#[test]
fn discover_skips_malformed_bundle() {
    let (_g, def) = temp_dir();
    write_bundle(
        &def,
        "Good",
        "[bundle]\nname = \"Good\"\nid = \"good-bundle\"\n[[command]]\nid = \"good-cmd\"\nname = \"Ok\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    write_bundle(
        &def,
        "Bad",
        "[bundle]\nname = \"Bad\"\nid = \"../../../../tmp/evil\"\n[[command]]\nid = \"bad-cmd\"\nname = \"Nope\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (cmds, _, skips) = discover_report(&DiscoverRoots {
        default_dir: def,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].id, "good-cmd");
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0].name, "Bad");
    assert_eq!(skips[0].reason, "bundle.toml: [bundle].id is not a safe id");
}

#[test]
fn discover_skip_reasons() {
    let (_g, def) = temp_dir();
    std::fs::create_dir_all(format!("{def}/Empty")).unwrap();
    let (_, _, skips) = discover_report(&DiscoverRoots {
        default_dir: def,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0].reason, "missing bundle.toml");
    assert_eq!(skips[0].source, "default");
}

#[test]
fn discover_category_grouping_across_bundles() {
    let (_g, def) = temp_dir();
    write_bundle(
        &def,
        "A",
        "[bundle]\nname = \"A\"\nid = \"b-a\"\n[[command]]\nid = \"a1\"\nname = \"Sort\"\ncategory = \"Text\"\norder = 2\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    write_bundle(
        &def,
        "B",
        "[bundle]\nname = \"B\"\nid = \"b-b\"\n[[command]]\nid = \"b1\"\nname = \"Upper\"\ncategory = \"Text\"\norder = 1\nbody = \"#!/bin/sh\\ntrue\\n\"\n[[command]]\nid = \"b2\"\nname = \"Diff Clip\"\ncategory = \"Diff\"\norder = 1\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (cmds, _) = discover(&DiscoverRoots {
        default_dir: def,
        ..Default::default()
    })
    .unwrap();
    let text: Vec<&str> = cmds
        .iter()
        .filter(|c| c.category == "Text")
        .map(|c| c.name.as_str())
        .collect();
    let diff = cmds.iter().filter(|c| c.category == "Diff").count();
    assert_eq!(diff, 1);
    // Text commands sort by order then name: Upper (1) before Sort (2).
    assert_eq!(text, ["Upper", "Sort"]);
}
