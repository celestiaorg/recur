//! gitignore checks for explorer paths.

use std::collections::HashSet;

use crate::command::command;
use crate::text::go_string;

/// Which of `paths` git ignores in `dir` (gitignore, `info/exclude` and the
/// user's global excludes). Tracked files that match a pattern are not
/// ignored, as with `git check-ignore`. Best-effort: `None` when `dir` is not
/// a work tree, git fails, or there are no paths.
///
/// Paths go NUL-delimited (`check-ignore -z --stdin`) so names with spaces,
/// newlines or non-ASCII bytes round-trip without C-quoting.
pub async fn ignored_set(dir: &str, paths: &[String]) -> Option<HashSet<String>> {
    if dir.is_empty() || paths.is_empty() {
        return None;
    }
    let mut input = Vec::new();
    for p in paths.iter().filter(|p| !p.is_empty() && *p != ".") {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    if input.is_empty() {
        return None;
    }
    let out = command(["-C", dir, "check-ignore", "-z", "--stdin"])
        .stdin(input)
        .output()
        .await
        .ok()?;
    parse_check_ignore(&out)
}

/// Parses `git check-ignore -z --stdin` stdout (NUL-delimited raw paths).
pub fn parse_check_ignore(out: &[u8]) -> Option<HashSet<String>> {
    let set: HashSet<String> = out
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(go_string)
        .collect();
    (!set.is_empty()).then_some(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_check_ignore_splits_nul() {
        let got =
            parse_check_ignore(b"node_modules\x00.env\x00weird name.txt\x00").unwrap_or_default();
        for p in ["node_modules", ".env", "weird name.txt"] {
            assert!(
                got.contains(p),
                "parse_check_ignore(...) missing {p:?}: {got:?}"
            );
        }
        assert_eq!(parse_check_ignore(b""), None);
    }
}
