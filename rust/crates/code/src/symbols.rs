//! Heuristic definition and reference index over the repo's source files.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::LazyLock;

use recur_domain::{OutlineSymbol, SymbolDef, SymbolRef};
use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::gore;
use crate::langs::is_source_file;
use crate::outline::{filepath_ext, outline, outline_line};
use crate::root::{path_str, rel_slash_str, skip_dir};
use crate::scan::scan_lines;
use crate::walk::{Flow, walk_dir};

static IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| gore::must(r"\b([A-Z][A-Za-z0-9_]{2,}|[a-z][A-Za-z0-9_]{3,})\b"));

static IS_IDENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}_][\p{L}\p{Nd}_]*$").expect("ident pattern"));

/// Bounds [`find_defs`]: a common name in a monorepo would otherwise build a
/// result nobody scrolls through.
const MAX_SYMBOL_DEFS: usize = 200;
/// Bounds the reference sites collected for one symbol.
const MAX_REFS_PER_NAME: usize = 120;

/// Definition sites named exactly `name` under `root`, capped at 200.
pub fn find_defs(cancel: &CancellationToken, root: &Path, name: &str) -> Result<Vec<SymbolDef>> {
    let name = name.trim();
    let mut defs = Vec::new();
    if name.is_empty() {
        return Ok(defs);
    }
    walk_source(cancel, root, |rel, full| {
        let ext = filepath_ext(rel).to_lowercase();
        Ok(scan_file(full, |line_no, line| {
            if let Some(sym) = outline_line(&ext, line)
                && sym.name == name
            {
                defs.push(SymbolDef {
                    name: sym.name,
                    kind: sym.kind.into(),
                    path: rel.to_string(),
                    line: line_no as i64,
                });
                if defs.len() >= MAX_SYMBOL_DEFS {
                    return Flow::SkipAll;
                }
            }
            Flow::Continue
        }))
    })?;
    Ok(defs)
}

/// Reference sites for a set of names, from one walk.
#[derive(Debug, Clone, Default)]
pub struct SymbolIndex {
    /// Name → reference sites, definitions included (marked `def`), in walk
    /// order.
    pub refs: HashMap<String, Vec<SymbolRef>>,
}

/// Walks `root` once and collects reference sites for every name, marking
/// the sites that are also definitions. Every file is read once and each
/// line checked for all names together, so the cost is one walk however many
/// symbols a diff touched (impact asks about all of them).
pub fn index_symbols(
    cancel: &CancellationToken,
    root: &Path,
    names: &[String],
    per_name: usize,
) -> Result<SymbolIndex> {
    let per_name = if per_name == 0 {
        MAX_REFS_PER_NAME
    } else {
        per_name
    };
    let mut idx = SymbolIndex::default();
    let mut wanted = Vec::new();
    let mut seen = HashSet::new();
    for n in names {
        let n = n.trim();
        if n.is_empty() || !IS_IDENT.is_match(n) || !seen.insert(n.to_string()) {
            continue;
        }
        wanted.push(n.to_string());
    }
    if wanted.is_empty() {
        return Ok(idx);
    }
    let quoted: Vec<String> = wanted.iter().map(|n| gore::quote_meta(n)).collect();
    let word = gore::compile(&format!(r"\b(?:{})\b", quoted.join("|"))).map_err(Error::Msg)?;
    let mut saturated = 0;
    walk_source(cancel, root, |rel, full| {
        let ext = filepath_ext(rel).to_lowercase();
        Ok(scan_file(full, |line_no, line| {
            let mut hits: Vec<&str> = Vec::new();
            for m in word.find_iter(line) {
                if !hits.contains(&m.as_str()) {
                    hits.push(m.as_str());
                }
            }
            if hits.is_empty() {
                return Flow::Continue;
            }
            let def = outline_line(&ext, line);
            let text = line.trim();
            for name in hits {
                let refs = idx.refs.entry(name.to_string()).or_default();
                if refs.len() >= per_name {
                    continue;
                }
                refs.push(SymbolRef {
                    path: rel.to_string(),
                    line: line_no as i64,
                    text: text.to_string(),
                    def: def.as_ref().is_some_and(|d| d.name == name),
                });
                if refs.len() == per_name {
                    saturated += 1;
                }
            }
            if saturated >= wanted.len() {
                Flow::SkipAll
            } else {
                Flow::Continue
            }
        }))
    })?;
    Ok(idx)
}

/// Definition and reference sites for `name` under `root`.
pub fn find_refs(cancel: &CancellationToken, root: &Path, name: &str) -> Result<Vec<SymbolRef>> {
    let mut idx = index_symbols(cancel, root, &[name.to_string()], MAX_REFS_PER_NAME)?;
    Ok(idx.refs.remove(name.trim()).unwrap_or_default())
}

/// Feeds each line of a file to `f`; unreadable files are skipped, and
/// `SkipAll` from `f` stops the enclosing walk.
fn scan_file(full: &str, mut f: impl FnMut(usize, &str) -> Flow) -> Flow {
    let Ok(file) = File::open(full) else {
        return Flow::Continue;
    };
    let stop = scan_lines(file, |n, raw| match f(n, &String::from_utf8_lossy(raw)) {
        Flow::SkipAll => ControlFlow::Break(()),
        _ => ControlFlow::Continue(()),
    });
    match stop {
        Ok(Some(())) => Flow::SkipAll,
        _ => Flow::Continue,
    }
}

/// Outline symbols whose lines are in, or within 3 lines of, the changed
/// line sets (path → new-side line numbers).
pub fn changed_symbols(
    root: &Path,
    changed_lines: &HashMap<String, HashSet<i64>>,
) -> Vec<OutlineSymbol> {
    let mut out = Vec::new();
    for rel in sorted_keys(changed_lines) {
        let lines = &changed_lines[rel];
        let Ok(syms) = outline(root, rel) else {
            continue;
        };
        for s in syms {
            if lines.contains(&s.line) || lines.iter().any(|ln| (ln - s.line).abs() <= 3) {
                out.push(s);
            }
        }
    }
    out
}

pub(crate) fn sorted_keys<V>(m: &HashMap<String, V>) -> Vec<&String> {
    let mut keys: Vec<&String> = m.keys().collect();
    keys.sort();
    keys
}

/// Walks source files under `root`, calling `f(rel, abs)`.
fn walk_source(
    cancel: &CancellationToken,
    root: &Path,
    mut f: impl FnMut(&str, &str) -> Result<Flow>,
) -> Result<()> {
    let root = path_str(root);
    walk_dir(&root, |path, d, err| {
        let (Some(d), None) = (d, err) else {
            return Ok(Flow::Continue);
        };
        if cancel.is_cancelled() {
            return Err(Error::Canceled);
        }
        if d.is_dir() {
            return Ok(if skip_dir(&d.name) {
                Flow::SkipDir
            } else {
                Flow::Continue
            });
        }
        if !is_source_file(&d.name) {
            return Ok(Flow::Continue);
        }
        f(&rel_slash_str(&root, path), path)
    })
}

/// Identifier-like tokens in a line of source, deduplicated, for impact.
pub fn extract_idents(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in IDENT_RE.find_iter(line) {
        let m = m.as_str();
        if m.len() < 3 || out.iter().any(|o| o == m) {
            continue;
        }
        out.push(m.to_string());
    }
    out
}
