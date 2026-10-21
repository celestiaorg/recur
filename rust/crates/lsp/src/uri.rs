//! Path and `file://` URI conversion (Go `lsp/uri.go`).

use std::path::Path;

/// The symlink-resolved absolute path, or the cleaned path when the target
/// does not exist yet.
pub fn canon_path(abs_path: &str) -> String {
    let abs = clean(abs_path);
    match std::fs::canonicalize(&abs) {
        Ok(real) => real.to_string_lossy().into_owned(),
        Err(_) => abs,
    }
}

/// Go `filepath.Clean` for absolute and relative Unix paths.
pub fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let body = parts.join("/");
    match (rooted, body.is_empty()) {
        (true, _) => format!("/{body}"),
        (false, true) => ".".into(),
        (false, false) => body,
    }
}

/// An absolute filesystem path as a `file://` URI, escaped like Go's
/// `url.URL{Scheme: "file", Path: p}.String()`.
pub fn path_to_uri(abs_path: &str) -> String {
    format!("file://{}", escape_path(&clean(abs_path)))
}

/// Go `shouldEscape(c, encodePath)`.
fn escape_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    for &b in p.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_'
                    | b'.'
                    | b'~'
                    | b'$'
                    | b'&'
                    | b'+'
                    | b','
                    | b'/'
                    | b':'
                    | b';'
                    | b'='
                    | b'@'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A `file://` URI as a filesystem path.
pub fn uri_to_path(uri: &str) -> Result<String, String> {
    let u = url::Url::parse(uri).map_err(|e| format!("parse {uri:?}: {e}"))?;
    if u.scheme() != "file" {
        return Err(format!("not a file URI: {uri}"));
    }
    unescape(u.path()).ok_or_else(|| format!("parse {uri:?}: invalid URL escape"))
}

/// Strict `%XX` unescaping: a malformed escape fails, as in Go.
fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Whether `p` names an existing regular file or symlink to one.
pub(crate) fn is_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| !m.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canon_path_resolves_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let want = std::fs::canonicalize(&link).unwrap();
        assert_eq!(canon_path(&link.to_string_lossy()), want.to_string_lossy());
        let missing = dir.path().join("nope/../x");
        assert_eq!(
            canon_path(&missing.to_string_lossy()),
            clean(&missing.to_string_lossy())
        );
    }

    #[test]
    fn path_to_uri_unix() {
        let u = path_to_uri("/Users/josh/proj");
        assert!(
            u.starts_with("file:///") && u.contains("/Users/josh/proj"),
            "{u}"
        );
    }

    #[test]
    fn clean_like_go() {
        for (p, want) in [
            ("", "."),
            ("/", "/"),
            ("a/../..", ".."),
            ("/a/./b/../c/", "/a/c"),
            ("//x", "/x"),
            ("/..", "/"),
        ] {
            assert_eq!(clean(p), want, "{p}");
        }
    }
}
