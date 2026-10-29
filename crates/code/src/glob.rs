//! VS Code-style `*` / `**` / `?` globs for repo-relative paths: the
//! translation the file index, copy-files and the UI's glob filters share,
//! and the UI's explorer exclude check (JavaScript regex semantics).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;

/// The anchored regex source for a glob: `**/` is any directory prefix, `**`
/// anything, `*` and `?` stay within one path segment, regex metacharacters
/// are escaped.
pub(crate) fn glob_to_regexp(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut out = String::from("^");
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if chars.get(i + 2) == Some(&'/') {
                out.push_str("(?:.*/)?");
                i += 2;
            } else {
                out.push_str(".*");
                i += 1;
            }
        } else if c == '*' {
            out.push_str("[^/]*");
        } else if c == '?' {
            out.push_str("[^/]");
        } else if r".+^${}()|[]\".contains(c) {
            out.push('\\');
            out.push(c);
        } else {
            out.push(c);
        }
        i += 1;
    }
    out.push('$');
    out
}

/// One explorer exclude pattern compiled as a JavaScript regex.
#[derive(Clone)]
pub struct CompiledGlob(Arc<regress::Regex>);

static CACHE: LazyLock<Mutex<HashMap<String, CompiledGlob>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn compiled(pattern: &str) -> Option<CompiledGlob> {
    let mut cache = CACHE.lock();
    if let Some(re) = cache.get(pattern) {
        return Some(re.clone());
    }
    let re = CompiledGlob(Arc::new(
        regress::Regex::new(&glob_to_regexp(pattern)).ok()?,
    ));
    cache.insert(pattern.to_string(), re.clone());
    Some(re)
}

/// Resolves an exclude list once per list change (the tree tests every node
/// against every pattern on each render). Blank patterns are dropped and
/// leading slashes trimmed.
pub fn compile_excludes(patterns: &[String]) -> Vec<CompiledGlob> {
    patterns
        .iter()
        .filter_map(|pat| {
            let p = pat.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}');
            if p.is_empty() {
                return None;
            }
            compiled(p.trim_start_matches('/'))
        })
        .collect()
}

/// Match results by compiled pattern (its `Arc`, alive for the process in `CACHE`) and path.
/// Views test every changed path against every exclude on each render; the backtracking
/// matcher made that seconds a frame for a diff of a few thousand files.
static MATCHES: LazyLock<Mutex<HashMap<usize, HashMap<String, bool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Paths remembered per pattern before its memo starts over.
const MATCHES_PER_PATTERN: usize = 200_000;

/// Whether `path` matches any compiled exclude.
pub fn is_excluded_by(path: &str, compiled: &[CompiledGlob]) -> bool {
    if compiled.is_empty() {
        return false;
    }
    let rel = path.trim_start_matches('/');
    let mut memo = MATCHES.lock();
    compiled.iter().any(|re| {
        let seen = memo.entry(Arc::as_ptr(&re.0) as usize).or_default();
        if let Some(&hit) = seen.get(rel) {
            return hit;
        }
        let units: Vec<u16> = rel.encode_utf16().collect();
        let hit = re.0.find_from_ucs2(&units, 0).next().is_some();
        if seen.len() >= MATCHES_PER_PATTERN {
            seen.clear();
        }
        seen.insert(rel.to_owned(), hit);
        hit
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(patterns: &[&str], path: &str) -> bool {
        let pats: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        is_excluded_by(path, &compile_excludes(&pats))
    }

    #[test]
    fn translation() {
        assert_eq!(glob_to_regexp("**/*.map"), r"^(?:.*/)?[^/]*\.map$");
        assert_eq!(glob_to_regexp("dist/**"), r"^dist/.*$");
        assert_eq!(glob_to_regexp("a?(b)"), r"^a[^/]\(b\)$");
    }

    #[test]
    fn excludes_like_the_explorer() {
        assert!(ex(&["**/.DS_Store"], "a/b/.DS_Store"));
        assert!(ex(&["**/.DS_Store"], ".DS_Store"));
        assert!(ex(&[" /dist/** "], "dist/x/y.js"));
        assert!(!ex(&["dist/**"], "distribute/x"));
        assert!(ex(&["node_modules"], "/node_modules"));
        assert!(!ex(&["node_modules"], "a/node_modules"));
        assert!(!ex(&["", "  "], "a"));
        assert!(!ex(&[], "a"));
        assert!(ex(&["*.log"], "x.log"));
        assert!(!ex(&["*.log"], "d/x.log"));
    }

    #[test]
    fn remembered_matches_stay_per_pattern() {
        for _ in 0..2 {
            assert!(ex(&["**/*.map"], "a/b.map"));
            assert!(!ex(&["**/*.css"], "a/b.map"));
            assert!(ex(&["**/*.css", "**/*.map"], "/a/b.map"));
        }
    }
}
