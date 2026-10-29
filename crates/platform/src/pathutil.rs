//! Lexical path helpers with Go `path/filepath` semantics on Unix. Rust's
//! `Path` keeps `..` and redundant components, which changes the outcome of
//! the prefix and escape checks ported from Go.

use std::path::{Path, PathBuf};

/// `filepath.Clean`: shortest equivalent path, `..` resolved lexically.
pub fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let rooted = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|p| *p != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            p => out.push(p),
        }
    }
    let joined = out.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// `filepath.Join`: joins non-empty elements and cleans the result.
pub fn join(parts: &[&str]) -> String {
    let kept: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    if kept.is_empty() {
        return String::new();
    }
    clean(&kept.join("/"))
}

/// `filepath.Abs`: joins with the working directory when relative, then cleans.
pub fn abs(path: &Path) -> std::io::Result<PathBuf> {
    let s = path.to_string_lossy();
    if s.starts_with('/') {
        return Ok(PathBuf::from(clean(&s)));
    }
    let cwd = std::env::current_dir()?;
    Ok(PathBuf::from(join(&[&cwd.to_string_lossy(), &s])))
}

/// `filepath.Rel`: a lexical path from `base` to `target`, or `None` when
/// one is absolute and the other is not (or `base` climbs past `target`).
pub fn rel(base: &str, target: &str) -> Option<String> {
    let base = clean(base);
    let target = clean(target);
    if base == target {
        return Some(".".to_string());
    }
    let base = if base == "." { String::new() } else { base };
    if base.starts_with('/') != target.starts_with('/') {
        return None;
    }
    let bparts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    let tparts: Vec<&str> = target.split('/').filter(|p| !p.is_empty()).collect();
    let common = bparts
        .iter()
        .zip(&tparts)
        .take_while(|(a, b)| a == b)
        .count();
    if bparts[common..].contains(&"..") {
        return None;
    }
    let mut out: Vec<&str> = vec![".."; bparts.len() - common];
    out.extend(&tparts[common..]);
    Some(if out.is_empty() {
        ".".to_string()
    } else {
        out.join("/")
    })
}

/// Whether `rel` (a `rel()` result) climbs out of its base.
pub fn escapes(rel: &str) -> bool {
    rel == ".." || rel.starts_with("../")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_matches_go() {
        for (input, want) in [
            ("", "."),
            ("a/b/../c", "a/c"),
            ("/../x", "/x"),
            ("../a/..", ".."),
            ("a//b/./", "a/b"),
            ("/", "/"),
            ("..", ".."),
            ("a/../..", ".."),
        ] {
            assert_eq!(clean(input), want, "clean({input:?})");
        }
    }

    #[test]
    fn rel_matches_go() {
        assert_eq!(rel("/a/b", "/a/b/c").as_deref(), Some("c"));
        assert_eq!(rel("/a/b", "/a/x").as_deref(), Some("../x"));
        assert_eq!(rel("/a/b", "/a/b").as_deref(), Some("."));
        assert_eq!(rel("/a", "b"), None);
        assert_eq!(rel("a", "b").as_deref(), Some("../b"));
        assert!(escapes(".."));
        assert!(escapes("../x"));
        assert!(!escapes("..x"));
    }

    #[test]
    fn join_and_abs() {
        assert_eq!(join(&["/a", "", "b/../c"]), "/a/c");
        assert_eq!(join(&["", ""]), "");
        assert_eq!(abs(Path::new("/x/../y")).expect("abs"), PathBuf::from("/y"));
        assert!(abs(Path::new("rel")).expect("abs").is_absolute());
    }
}
