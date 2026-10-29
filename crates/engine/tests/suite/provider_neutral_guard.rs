//! Static guards over the engine's non-test sources: no direct provider
//! dispatch, no behavior keyed off a provider id, and PR code that goes
//! through `Forge` without naming a forge.

use std::path::Path;

fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            if !p.ends_with("tests") {
                sources(&p, out);
            }
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let is_test = name == "tests.rs"
            || name.ends_with("_tests.rs")
            || name == "test_support.rs"
            || name == "testing.rs";
        if name.ends_with(".rs") && !is_test {
            out.push((
                p.display().to_string(),
                std::fs::read_to_string(&p).unwrap(),
            ));
        }
    }
}

fn engine_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    assert!(out.len() > 10, "walked the wrong directory");
    out
}

#[test]
fn every_turn_goes_through_run_mirrored() {
    for (path, src) in engine_sources() {
        let n = src.matches(".run_turn(").count();
        let allowed = usize::from(path.ends_with("src/turn/mod.rs"));
        assert!(
            n <= allowed,
            "{path} dispatches a turn via .run_turn( — use Engine::run_agent so routing and the mirror apply"
        );
    }
}

#[test]
fn no_provider_id_compares() {
    let compare = regex::Regex::new(
        r#"(==|!=)\s*(recur_domain::)?PROVIDER_[A-Z_]+|PROVIDER_[A-Z_]+\s*(==|!=|=>|\|)|"(cursor|codex|claude-code)"\s*(=>|\|)|(==|!=)\s*"(cursor|codex|claude-code)""#,
    )
    .unwrap();
    for (path, src) in engine_sources() {
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            assert!(
                !compare.is_match(code),
                "{path}:{}: behavior keyed off a provider id: {line}",
                i + 1
            );
        }
    }
}

#[test]
fn no_forge_specifics_outside_the_forge() {
    let leak = regex::Regex::new(
        r#"PrSource::GitHub|(==|!=)\s*"gh"|"gh"\s*(=>|\|)|"[^"]*GitHub[^"]*"|"(refs/)?pull/"#,
    )
    .unwrap();
    // The chat surface marker: its wire values (`github_pr` and aliases) and the
    // Local PR subject that says it is not a forge PR.
    let allowed = |path: &str| path.ends_with("src/chat/surface.rs");
    for (path, src) in engine_sources() {
        if path.contains("/src/view/") {
            continue;
        }
        for (i, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            assert!(
                !leak.is_match(code) || allowed(&path),
                "{path}:{}: forge-specific behavior or copy outside recur-forge: {line}",
                i + 1
            );
        }
    }
}
