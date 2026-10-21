//! Go `path/filepath` semantics (Unix) for string paths, so joined and
//! cleaned paths compare byte for byte with the Go app's.

use std::path::Path;

/// `filepath.Clean`: lexical shortest equivalent path.
pub(crate) fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|last| *last != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// `filepath.Join`: joins non-empty elements and cleans the result.
pub(crate) fn join(elems: &[&str]) -> String {
    let parts: Vec<&str> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return String::new();
    }
    clean(&parts.join("/"))
}

pub(crate) fn is_abs(p: &str) -> bool {
    p.starts_with('/')
}

/// `filepath.Abs`.
pub(crate) fn abs(p: &str) -> std::io::Result<String> {
    if is_abs(p) {
        return Ok(clean(p));
    }
    let wd = std::env::current_dir()?;
    Ok(join(&[&wd.to_string_lossy(), p]))
}

/// `filepath.Dir`.
pub(crate) fn dir(p: &str) -> String {
    let i = p.rfind('/').map_or(0, |i| i + 1);
    clean(&p[..i])
}

/// `filepath.Base`.
pub(crate) fn base(p: &str) -> String {
    if p.is_empty() {
        return ".".to_string();
    }
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        return "/".to_string();
    }
    match p.rfind('/') {
        Some(i) => p[i + 1..].to_string(),
        None => p.to_string(),
    }
}

/// `filepath.EvalSymlinks` for an absolute or relative path that exists.
pub(crate) fn eval_symlinks(p: &str) -> std::io::Result<String> {
    let resolved = std::fs::canonicalize(Path::new(p))?;
    Ok(resolved.to_string_lossy().into_owned())
}

/// `filepath.Rel`: `targ` relative to `basepath`, both made clean first.
pub(crate) fn rel(basepath: &str, targ: &str) -> Option<String> {
    let base = clean(basepath);
    let targ = clean(targ);
    if targ == base {
        return Some(".".to_string());
    }
    let base = if base == "." { String::new() } else { base };
    if is_abs(&base) != is_abs(&targ) {
        return None;
    }
    let (b, t) = (base.as_bytes(), targ.as_bytes());
    let (bl, tl) = (b.len(), t.len());
    let (mut b0, mut bi, mut t0, mut ti) = (0, 0, 0, 0);
    loop {
        while bi < bl && b[bi] != b'/' {
            bi += 1;
        }
        while ti < tl && t[ti] != b'/' {
            ti += 1;
        }
        if t[t0..ti] != b[b0..bi] {
            break;
        }
        if bi < bl {
            bi += 1;
        }
        if ti < tl {
            ti += 1;
        }
        b0 = bi;
        t0 = ti;
    }
    if &base[b0..bi] == ".." {
        return None;
    }
    if b0 != bl {
        let seps = base[b0..bl].matches('/').count();
        let mut buf = String::from("..");
        for _ in 0..seps {
            buf.push_str("/..");
        }
        if t0 != tl {
            buf.push('/');
            buf.push_str(&targ[t0..]);
        }
        return Some(buf);
    }
    Some(targ[t0..].to_string())
}

/// `path.Clean` (slash paths) is the same algorithm on Unix.
pub(crate) fn clean_slash(p: &str) -> String {
    clean(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_matches_go() {
        for (input, want) in [
            ("", "."),
            ("abc", "abc"),
            ("abc/def", "abc/def"),
            ("a/b/c", "a/b/c"),
            (".", "."),
            ("..", ".."),
            ("../..", "../.."),
            ("../../abc", "../../abc"),
            ("/abc", "/abc"),
            ("/", "/"),
            ("abc/", "abc"),
            ("abc/def/", "abc/def"),
            ("a/b/c/", "a/b/c"),
            ("./", "."),
            ("../", ".."),
            ("../../", "../.."),
            ("/abc/", "/abc"),
            ("abc//def//ghi", "abc/def/ghi"),
            ("//abc", "/abc"),
            ("///abc", "/abc"),
            ("//abc//", "/abc"),
            ("abc//", "abc"),
            ("abc/./def", "abc/def"),
            ("/./abc/def", "/abc/def"),
            ("abc/.", "abc"),
            ("abc/def/ghi/../jkl", "abc/def/jkl"),
            ("abc/def/../ghi/../jkl", "abc/jkl"),
            ("abc/def/..", "abc"),
            ("abc/def/../..", "."),
            ("/abc/def/../..", "/"),
            ("abc/def/../../..", ".."),
            ("/abc/def/../../..", "/"),
            ("abc/def/../../../ghi/jkl/../../../mno", "../../mno"),
            ("/../abc", "/abc"),
            ("abc/./../def", "def"),
            ("abc//./../def", "def"),
            ("abc/../../././../def", "../../def"),
        ] {
            assert_eq!(clean(input), want, "clean({input:?})");
        }
    }

    #[test]
    fn rel_matches_go() {
        for (base, targ, want) in [
            ("a/b", "a/b", Some(".")),
            ("a/b/.", "a/b", Some(".")),
            ("a/b", "a/b/.", Some(".")),
            ("./a/b", "a/b", Some(".")),
            ("a/b", "./a/b", Some(".")),
            ("ab/cd", "ab/cde", Some("../cde")),
            ("ab/cd", "ab/c", Some("../c")),
            ("a/b", "a/b/c/d", Some("c/d")),
            ("a/b", "a/b/../c", Some("../c")),
            ("a/b/../c", "a/b", Some("../b")),
            ("a/b/c", "a/c/d", Some("../../c/d")),
            ("a/b", "c/d", Some("../../c/d")),
            ("a/b/c/d", "a/b", Some("../..")),
            ("a/b/c/d", "a/b/", Some("../..")),
            ("a/b/c/d/", "a/b", Some("../..")),
            ("a/b/c/d/", "a/b/", Some("../..")),
            ("../../a/b", "../../a/b/c/d", Some("c/d")),
            ("/a/b", "/a/b", Some(".")),
            ("/a/b/.", "/a/b", Some(".")),
            ("/a/b", "/a/b/.", Some(".")),
            ("/ab/cd", "/ab/cde", Some("../cde")),
            ("/ab/cd", "/ab/c", Some("../c")),
            ("/a/b", "/a/b/c/d", Some("c/d")),
            ("/a/b", "/a/b/../c", Some("../c")),
            ("/a/b/../c", "/a/b", Some("../b")),
            ("/a/b/c", "/a/c/d", Some("../../c/d")),
            ("/a/b", "/c/d", Some("../../c/d")),
            ("/a/b/c/d", "/a/b", Some("../..")),
            ("/a/b/c/d", "/a/b/", Some("../..")),
            ("/a/b/c/d/", "/a/b", Some("../..")),
            ("/a/b/c/d/", "/a/b/", Some("../..")),
            ("/../../a/b", "/../../a/b/c/d", Some("c/d")),
            (".", "a/b", Some("a/b")),
            (".", "..", Some("..")),
            ("..", ".", None),
            ("..", "a", None),
            ("../..", "..", None),
            ("a", "/a", None),
            ("/a", "a", None),
        ] {
            assert_eq!(rel(base, targ).as_deref(), want, "rel({base:?}, {targ:?})");
        }
    }

    #[test]
    fn dir_and_base_match_go() {
        assert_eq!(dir("/a/b/c"), "/a/b");
        assert_eq!(dir("a/b"), "a");
        assert_eq!(dir("a"), ".");
        assert_eq!(dir("/"), "/");
        assert_eq!(dir(""), ".");
        assert_eq!(base("/a/b/c"), "c");
        assert_eq!(base("/a/b/"), "b");
        assert_eq!(base("/"), "/");
        assert_eq!(base(""), ".");
        assert_eq!(join(&["a", "", "b/../c"]), "a/c");
        assert_eq!(join(&["", ""]), "");
    }
}
