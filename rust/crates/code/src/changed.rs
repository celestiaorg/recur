use std::collections::{HashMap, HashSet};
use std::path::Path;

use recur_domain::SymbolRef;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::outline::outline;
use crate::symbols::{find_refs, sorted_keys};

/// A symbol whose definition line falls in a diff hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedDef {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: i64,
}

/// Symbols whose definition line is in the changed line set (path → new-side
/// line numbers). Exact line match only: a ±3 proximity inflated false impact.
pub fn changed_defs(root: &Path, changed_lines: &HashMap<String, HashSet<i64>>) -> Vec<ChangedDef> {
    let mut out = Vec::new();
    for rel in sorted_keys(changed_lines) {
        let Ok(syms) = outline(root, rel) else {
            continue;
        };
        for s in syms {
            if changed_lines[rel].contains(&s.line) {
                out.push(ChangedDef {
                    name: s.name,
                    kind: s.kind,
                    path: rel.clone(),
                    line: s.line,
                });
            }
        }
    }
    out
}

/// References to `name`, preferring files under the same top-level
/// directory as `def_path`, capped. This walks the repo for one symbol;
/// callers with many symbols should build a [`crate::SymbolIndex`] once and
/// rank it with [`rank_refs_in_package`].
pub fn find_refs_in_package(
    cancel: &CancellationToken,
    root: &Path,
    name: &str,
    def_path: &str,
    limit: usize,
) -> Result<Vec<SymbolRef>> {
    let all = find_refs(cancel, root, name)?;
    Ok(rank_refs_in_package(&all, def_path, limit))
}

/// Drops definition sites and puts refs under `def_path`'s top-level
/// directory first, then truncates to `limit` (0 means 8).
pub fn rank_refs_in_package(all: &[SymbolRef], def_path: &str, limit: usize) -> Vec<SymbolRef> {
    let limit = if limit == 0 { 8 } else { limit };
    let pkg = top_pkg(def_path);
    let (mut near, mut far) = (Vec::new(), Vec::new());
    for r in all.iter().filter(|r| !r.def) {
        if !pkg.is_empty() && top_pkg(&r.path) == pkg {
            near.push(r.clone());
        } else {
            far.push(r.clone());
        }
    }
    near.extend(far);
    near.truncate(limit);
    near
}

fn top_pkg(path: &str) -> &str {
    path.split_once('/').map_or(path, |(head, _)| head)
}
