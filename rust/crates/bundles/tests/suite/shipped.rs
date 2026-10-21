//! Port of `shipped_tests_test.go` and `seed_shipped_test.go`.

use std::collections::HashSet;

use crate::common::*;
use recur_bundles::*;

fn shipped_dirs() -> Vec<(String, String)> {
    let root = shipped_root();
    let mut out = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.file_type().unwrap().is_dir() || name == "sdk" {
            continue;
        }
        out.push((name.clone(), format!("{root}/{name}")));
    }
    out
}

/// Runs tests/*.toml for every shipped pack that has cases.
#[tokio::test(flavor = "multi_thread")]
async fn shipped_bundle_tests() {
    if !node_available() {
        eprintln!("node not on PATH");
        return;
    }
    let (_g, cache) = temp_dir();
    let ctx = RunCtx::background();
    let mut any = false;
    let mut failures = Vec::new();
    for (name, dir) in shipped_dirs() {
        if !std::path::Path::new(&format!("{dir}/bundle.toml")).exists() {
            continue;
        }
        let tests = load_tests(&dir).unwrap_or_else(|e| panic!("LoadTests({name}): {e}"));
        if tests.is_empty() {
            continue;
        }
        any = true;
        let rep = run_bundle_tests(&ctx, &dir, &cache)
            .await
            .unwrap_or_else(|e| panic!("RunBundleTests({name}): {e}"));
        if !rep.ok() {
            failures.push(format!("{name}:\n{}", format_report(&rep, false)));
        }
    }
    assert!(any, "expected at least one shipped bundle with tests/");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Every shipped [[command]] has a tests/*.toml case (by id or name).
#[test]
fn shipped_commands_have_tests() {
    let mut missing = Vec::new();
    for (name, dir) in shipped_dirs() {
        let Ok(data) = std::fs::read(format!("{dir}/bundle.toml")) else {
            continue;
        };
        let (_, cmds, _) =
            parse_manifest(&dir, &data).unwrap_or_else(|e| panic!("ParseManifest({name}): {e}"));
        if cmds.is_empty() {
            continue;
        }
        let tests = load_tests(&dir).unwrap();
        let covered: HashSet<String> = tests.iter().map(|t| t.case.command.clone()).collect();
        for c in cmds {
            if !covered.contains(&c.id) && !covered.contains(&c.name) {
                missing.push(format!(
                    "{name}: {} ({}) has no tests/*.toml case",
                    c.name, c.id
                ));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

/// Every shipped [[snippet]] has a tests/*.toml snippet case.
#[test]
fn shipped_snippets_have_tests() {
    let mut missing = Vec::new();
    for (name, dir) in shipped_dirs() {
        let Ok(data) = std::fs::read(format!("{dir}/bundle.toml")) else {
            continue;
        };
        let parsed = parse_manifest_full(&dir, &data)
            .unwrap_or_else(|e| panic!("ParseManifestFull({name}): {e}"));
        if parsed.snippets.is_empty() {
            continue;
        }
        let tests = load_tests(&dir).unwrap();
        let covered: HashSet<String> = tests
            .iter()
            .filter(|t| !t.case.snippet.is_empty())
            .map(|t| t.case.snippet.clone())
            .collect();
        for s in parsed.snippets {
            if !covered.contains(&s.id) {
                missing.push(format!(
                    "{name}: {} ({}) has no tests/*.toml snippet case",
                    s.name, s.id
                ));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn discover_shipped_bundles() {
    let (cmds, _) = discover(&DiscoverRoots {
        default_dir: shipped_root(),
        ..Default::default()
    })
    .unwrap();
    assert!(
        cmds.len() >= 30,
        "expected many shipped commands, got {}",
        cmds.len()
    );
    let (_g, cache) = temp_dir();
    let mut n = 0;
    for c in &cmds {
        if !is_script_interpreter(&c.interpreter) {
            continue;
        }
        compile_macro(&cache, c).unwrap_or_else(|e| panic!("compile {} ({}): {e}", c.name, c.id));
        n += 1;
    }
    assert!(n >= 20, "expected many typescript macros, compiled {n}");
}

#[test]
fn shipped_svg_grammar() {
    let cat = discover_catalog(&DiscoverRoots {
        default_dir: shipped_root(),
        ..Default::default()
    })
    .unwrap();
    let g =
        find_grammar_by_scope(&cat.grammars, "text.xml.svg").expect("missing text.xml.svg grammar");
    assert_eq!(g.id, "recur.languages.svg");
    assert_eq!(g.language, "svg");
    assert!(
        g.file_types.iter().any(|f| f == "svg"),
        "fileTypes: {:?}",
        g.file_types
    );
    let body = std::fs::read_to_string(&g.abs_path).unwrap();
    assert!(
        body.contains(r#""include": "text.xml""#),
        "svg grammar should include text.xml"
    );
    assert!(
        cat.terminal_themes.len() >= 8,
        "terminal themes: {}",
        cat.terminal_themes.len()
    );
    assert!(find_terminal_theme_by_id(&cat.terminal_themes, "homebrew").is_some());
}
