use std::collections::{HashMap, HashSet};
use std::path::Path;

use recur_domain::path;
use recur_domain::{FileDiff, ImpactItem};
use tokio_util::sync::CancellationToken;

use super::{parse_line, to_slash};
use crate::{SymbolIndex, changed_defs, index_symbols, rank_refs_in_package, safe_join};

/// Reference sites reported per changed symbol.
const IMPACT_REFS_PER_SYMBOL: usize = 6;
/// Reference sites collected per symbol before ranking by package
/// proximity: enough candidates for the same-package preference, bounded so
/// a very common name cannot blow up memory.
const IMPACT_REF_SCAN: usize = 60;

/// The blast radius of a diff: symbols whose definition line is in the change
/// set, their package-preferring references (capped), and co-located tests.
/// References for every changed symbol come from one repo walk.
pub fn compute_impact(
    cancel: &CancellationToken,
    root: &str,
    files: &[FileDiff],
) -> Vec<ImpactItem> {
    let root_path = Path::new(root);
    let mut changed_lines: HashMap<String, HashSet<i64>> = HashMap::new();
    let mut changed_paths = Vec::new();
    for f in files {
        let path = to_slash(&f.path);
        changed_paths.push(path.clone());
        // Definitions appear on add lines; pure context is not a touched def.
        let lines = f
            .lines
            .iter()
            .flatten()
            .filter(|l| l.r#type == "add")
            .map(|l| parse_line(&l.new))
            .filter(|&n| n > 0)
            .collect();
        changed_lines.insert(path, lines);
    }

    let defs = changed_defs(root_path, &changed_lines);
    let reportable = |name: &str| name.len() >= 3 && !is_noise_ident(name);
    let names: Vec<String> = defs
        .iter()
        .filter(|d| reportable(&d.name))
        .map(|d| d.name.clone())
        .collect();
    let ref_index = if names.is_empty() {
        SymbolIndex::default()
    } else {
        index_symbols(cancel, root_path, &names, IMPACT_REF_SCAN).unwrap_or_default()
    };

    let mut impact = Vec::new();
    let mut seen = HashSet::new();
    for d in &defs {
        // Tiny and noisy names explode false references.
        if !reportable(&d.name) || !seen.insert(format!("sym:{}:{}", d.path, d.name)) {
            continue;
        }
        impact.push(ImpactItem {
            kind: "symbol".into(),
            name: d.name.clone(),
            path: d.path.clone(),
            line: d.line,
            detail: format!("{} definition in diff", d.kind),
            changed: true,
        });
        let refs = ref_index
            .refs
            .get(&d.name)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for r in rank_refs_in_package(refs, &d.path, IMPACT_REFS_PER_SYMBOL) {
            if !seen.insert(format!("ref:{}:{}", r.path, d.name)) {
                continue;
            }
            impact.push(ImpactItem {
                kind: "api".into(),
                name: d.name.clone(),
                path: r.path,
                line: r.line,
                detail: format!("references changed {}", d.kind),
                changed: false,
            });
        }
    }

    for p in &changed_paths {
        for c in test_candidates(p) {
            let c = to_slash(&c);
            if c == *p || seen.contains(&format!("test:{c}")) {
                continue;
            }
            let Ok(full) = safe_join(root_path, &c) else {
                continue;
            };
            if std::fs::metadata(full).is_err() {
                continue;
            }
            seen.insert(format!("test:{c}"));
            impact.push(ImpactItem {
                kind: "test".into(),
                name: path::base(&c),
                path: c,
                line: 0,
                detail: format!("likely covers {p}"),
                changed: false,
            });
        }
    }
    impact.truncate(36);
    impact
}

/// Test files that likely cover `p`, by language convention.
fn test_candidates(p: &str) -> Vec<String> {
    let base = path::base(p);
    let mut stem = base.strip_suffix(path::ext(&base)).unwrap_or(&base);
    for suffix in ["_test", ".test", ".spec"] {
        stem = stem.strip_suffix(suffix).unwrap_or(stem);
    }
    let base_no_ext = p.strip_suffix(path::ext(p)).unwrap_or(p);
    let dir = path::dir(p);
    let mut out: Vec<String> = [
        "_test.go",
        "_test.py",
        "_test.rs",
        ".test.ts",
        ".test.tsx",
        ".test.js",
        ".spec.ts",
        ".spec.tsx",
        ".spec.js",
        "_spec.rb",
        "Test.java",
        "Tests.java",
        "Test.kt",
        "Tests.swift",
    ]
    .iter()
    .map(|s| format!("{base_no_ext}{s}"))
    .collect();
    out.push(format!("{dir}/{stem}_test.go"));
    out.push(format!("{dir}/test_{stem}.py"));
    out.push(format!("{dir}/{stem}_test.py"));
    // Only the first byte is upper-cased; a non-ASCII first rune cannot name a
    // real file after that byte surgery, so it is left out.
    if let Some(first) = stem.chars().next().filter(char::is_ascii) {
        out.push(format!(
            "{dir}/{}{}Test.java",
            first.to_ascii_uppercase(),
            &stem[1..]
        ));
    }
    out
}

fn is_noise_ident(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "new"
            | "get"
            | "set"
            | "update"
            | "delete"
            | "create"
            | "init"
            | "main"
            | "error"
            | "err"
            | "ok"
            | "data"
            | "value"
            | "item"
            | "type"
            | "name"
            | "id"
            | "run"
            | "start"
            | "stop"
            | "close"
            | "open"
            | "read"
            | "write"
            | "string"
            | "int"
            | "bool"
            | "handle"
            | "handler"
            | "config"
            | "options"
            | "result"
    )
}
