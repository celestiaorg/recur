//! Scope and file-glob filtering for commands and snippets.

use recur_domain::path;

use crate::manifest::Command;

/// Whether `cmd` applies to the editor grammar scope and file path.
/// Unscoped commands (empty scope and fileGlob) match everything. Grammar
/// matching is TextMate-style prefix (`source.go` matches
/// `source.go.string`); fileGlob is tried against the base name and the
/// full slash path.
pub fn matches(cmd: &Command, file_scope: &str, file_path: &str) -> bool {
    matches_scope_glob(&cmd.scope, &cmd.file_glob, file_scope, file_path)
}

pub(crate) fn matches_scope_glob(
    scope: &str,
    file_glob: &str,
    file_scope: &str,
    file_path: &str,
) -> bool {
    if scope.is_empty() && file_glob.is_empty() {
        return true;
    }
    if !scope.is_empty() && !scope_prefix_match(scope, file_scope) {
        return false;
    }
    if !file_glob.is_empty() && !glob_match(file_glob, file_path) {
        return false;
    }
    true
}

fn scope_prefix_match(want: &str, have: &str) -> bool {
    let want = want.trim();
    let have = have.trim();
    if want.is_empty() {
        return true;
    }
    if have.is_empty() {
        return false;
    }
    want.split(',')
        .map(str::trim)
        .filter(|alt| !alt.is_empty())
        .any(|alt| have == alt || have.starts_with(&format!("{alt}.")))
}

/// Whether `file_path` matches any pattern. Empty patterns match every
/// path. Patterns follow command fileGlob rules (basename, slash path,
/// `**/`, and one `{a,b}` brace group).
pub fn match_file_glob(patterns: &[String], file_path: &str) -> bool {
    patterns.is_empty() || patterns.iter().any(|p| glob_match(p, file_path))
}

fn glob_match(pattern: &str, file_path: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return true;
    }
    // One `{a,b,c}` brace group (e.g. `*.{js,ts}`); filepath.Match has none.
    let alts = expand_braces(pattern);
    if alts.len() > 1 {
        return alts.iter().any(|p| glob_match(p, file_path));
    }
    let m = |p: &str, s: &str| path::glob_match(p, s).unwrap_or(false);
    let base = path::base(file_path);
    if m(pattern, &base) || m(pattern, file_path) {
        return true;
    }
    if let Some(suf) = pattern.strip_prefix("**/") {
        if m(suf, &base) {
            return true;
        }
        for (i, b) in file_path.bytes().enumerate() {
            if b == b'/' && m(suf, &file_path[i + 1..]) {
                return true;
            }
        }
    }
    false
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let (Some(start), Some(end)) = (pattern.find('{'), pattern.rfind('}')) else {
        return vec![pattern.to_string()];
    };
    if end <= start {
        return vec![pattern.to_string()];
    }
    let prefix = &pattern[..start];
    let suffix = &pattern[end + 1..];
    let out: Vec<String> = pattern[start + 1..end]
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("{prefix}{p}{suffix}"))
        .collect();
    if out.is_empty() {
        vec![pattern.to_string()]
    } else {
        out
    }
}
