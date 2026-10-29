use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use toml::Value;

use crate::PANE_EMBEDS;

const VIEW_CORE: &str = "recur-view-core";
const WIDGETS: &str = "recur-view-widgets";
const GPUI: &[&str] = &["gpui", "gpui-kit", "gpui-pre", "gpui-fast"];
/// The persisted provider, forge and tracker ids (`docs/ARCHITECTURE.md`).
const IDS: &[&str] = &[
    "claude-code",
    "codex",
    "chatgpt",
    "cursor",
    "github",
    "gh",
    "linear",
];
/// Concrete forge and tracker modules.
const CONCRETE: &[&str] = &[
    "recur_forge::github",
    "recur_issues::github",
    "recur_issues::linear",
];

struct Crate {
    dir: PathBuf,
    /// Workspace crates and external crates this one depends on outside `[dev-dependencies]`
    /// (any target, optional ones included).
    deps: BTreeSet<String>,
}

fn rust_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("rust/")
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("Cargo.toml").is_file())
        .collect();
    out.sort();
    out
}

fn dep_names(table: Option<&Value>, into: &mut BTreeSet<String>) {
    let Some(t) = table.and_then(Value::as_table) else {
        return;
    };
    for (key, v) in t {
        let name = v
            .get("package")
            .and_then(Value::as_str)
            .unwrap_or(key.as_str());
        into.insert(name.to_string());
    }
}

/// Every crate of the workspace, by package name.
fn crates() -> BTreeMap<String, Crate> {
    let root = rust_root();
    let mut dirs = Vec::new();
    for group in [
        "crates",
        "crates/views",
        "crates/panes",
        "providers",
        "tools",
    ] {
        dirs.extend(subdirs(&root.join(group)));
    }
    let mut out = BTreeMap::new();
    for dir in dirs {
        let text = fs::read_to_string(dir.join("Cargo.toml")).expect("manifest");
        let m: Value = toml::from_str(&text).expect("manifest parses");
        let name = m["package"]["name"].as_str().expect("name").to_string();
        let mut deps = BTreeSet::new();
        for section in ["dependencies", "build-dependencies"] {
            dep_names(m.get(section), &mut deps);
            if let Some(targets) = m.get("target").and_then(Value::as_table) {
                for t in targets.values() {
                    dep_names(t.get(section), &mut deps);
                }
            }
        }
        out.insert(name, Crate { dir, deps });
    }
    out
}

fn in_group(c: &Crate, group: &str) -> bool {
    c.dir.parent().is_some_and(|p| p.ends_with(group))
}

/// `name` and every workspace crate it reaches outside dev-dependencies.
fn closure(all: &BTreeMap<String, Crate>, name: &str) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![name.to_string()];
    while let Some(n) = stack.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(c) = all.get(&n) {
            stack.extend(c.deps.iter().filter(|d| all.contains_key(*d)).cloned());
        }
    }
    seen
}

#[test]
fn views_depend_on_no_other_view() {
    let all = crates();
    let views: BTreeSet<&str> = all
        .iter()
        .filter(|(_, c)| in_group(c, "crates/views"))
        .map(|(n, _)| n.as_str())
        .collect();
    let panes: BTreeSet<&str> = all
        .iter()
        .filter(|(_, c)| in_group(c, "crates/panes"))
        .map(|(n, _)| n.as_str())
        .collect();
    assert!(views.contains(VIEW_CORE) && views.contains(WIDGETS) && !panes.is_empty());
    let above = ["recur-app", "recur-desktop", "recur-cli"];
    let mut bad = Vec::new();
    for (name, c) in &all {
        let is_view = views.contains(name.as_str());
        let is_pane = panes.contains(name.as_str());
        if !is_view && !is_pane {
            continue;
        }
        let embeds: &[&str] = PANE_EMBEDS
            .iter()
            .find(|(p, _)| p == name)
            .map_or(&[], |(_, e)| e);
        for d in &c.deps {
            let d = d.as_str();
            let ok = if above.contains(&d) {
                false
            } else if !views.contains(d) && !panes.contains(d) {
                true
            } else if name == VIEW_CORE {
                false
            } else if name == WIDGETS {
                d == VIEW_CORE
            } else if is_pane {
                d == VIEW_CORE || d == WIDGETS || embeds.contains(&d)
            } else {
                d == VIEW_CORE || d == WIDGETS || panes.contains(d)
            };
            if !ok {
                bad.push(format!("{name} -> {d}"));
            }
        }
    }
    assert!(bad.is_empty(), "view layering broken:\n{}", bad.join("\n"));
}

#[test]
fn nothing_from_services_down_depends_on_gpui() {
    let all = crates();
    let bad: Vec<String> = closure(&all, "recur-services")
        .into_iter()
        .filter_map(|n| {
            let c = &all[&n];
            let hit: Vec<&str> = GPUI
                .iter()
                .copied()
                .filter(|g| c.deps.contains(*g))
                .collect();
            (!hit.is_empty()).then(|| format!("{n} -> {}", hit.join(", ")))
        })
        .collect();
    assert!(bad.is_empty(), "GPUI below the UI:\n{}", bad.join("\n"));
}

#[test]
fn the_engine_depends_on_no_provider() {
    let all = crates();
    let providers: BTreeSet<&str> = all
        .iter()
        .filter(|(_, c)| in_group(c, "providers"))
        .map(|(n, _)| n.as_str())
        .collect();
    assert!(!providers.is_empty());
    let bad: Vec<String> = closure(&all, "recur-engine")
        .into_iter()
        .filter(|n| providers.contains(n.as_str()))
        .collect();
    assert!(bad.is_empty(), "the engine reaches {bad:?}");
}

fn is_test_file(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name == "test_support.rs"
        || p.components().any(|c| c.as_os_str() == "tests")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).expect("dir").flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") && !is_test_file(&p) {
            out.push(p);
        }
    }
}

/// The lines of `src` outside comments and a trailing `#[cfg(test)]` module.
fn code_lines(src: &str) -> impl Iterator<Item = (usize, &str)> {
    let end = src
        .lines()
        .position(|l| l.trim() == "#[cfg(test)]")
        .unwrap_or(usize::MAX);
    src.lines()
        .enumerate()
        .take(end)
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
}

#[test]
fn the_engine_names_no_concrete_provider_forge_or_tracker() {
    let root = rust_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates/engine/src"), &mut files);
    assert!(!files.is_empty());
    let mut bad = Vec::new();
    for f in &files {
        let src = fs::read_to_string(f).expect("source");
        for (i, line) in code_lines(&src) {
            let id = IDS.iter().find(|id| {
                line.contains(&format!("\"{id}\"")) || line.contains(&format!("\"{id}/"))
            });
            let module = CONCRETE.iter().find(|m| line.contains(*m));
            if let Some(hit) = id.or(module) {
                let rel = f.strip_prefix(&root).unwrap_or(f);
                bad.push(format!("{}:{}: {hit}", rel.display(), i + 1));
            }
        }
    }
    assert!(bad.is_empty(), "the engine names:\n{}", bad.join("\n"));
}

/// `recur-http` builds every client: `reqwest`'s own constructors panic without a CA store.
#[test]
fn http_clients_come_from_recur_http() {
    let root = rust_root();
    let mut files = Vec::new();
    for dir in subdirs(&root.join("crates"))
        .into_iter()
        .chain(subdirs(&root.join("crates/views")))
        .chain(subdirs(&root.join("crates/panes")))
        .chain(subdirs(&root.join("providers")))
    {
        if !dir.ends_with("http") && dir.join("src").is_dir() {
            rust_files(&dir.join("src"), &mut files);
        }
    }
    let mut bad = Vec::new();
    for f in &files {
        let src = fs::read_to_string(f).expect("source");
        for (i, line) in code_lines(&src) {
            if line.contains("Client::new()") || line.contains("Client::builder()") {
                let rel = f.strip_prefix(&root).unwrap_or(f);
                bad.push(format!("{}:{}", rel.display(), i + 1));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "build clients with recur_http:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_id_scan_sees_a_literal() {
    let src = "let a = 1;\nlet p = \"codex\";\n// \"gh\" in a comment\n#[cfg(test)]\nmod t { \"linear\" }";
    let hits: Vec<usize> = code_lines(src)
        .filter(|(_, l)| {
            l.contains("\"codex\"") || l.contains("\"gh\"") || l.contains("\"linear\"")
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits, [1]);
}
