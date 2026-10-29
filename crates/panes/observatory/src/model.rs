//! Small shared types of the Observatory state.

use recur_domain::FileContent;

/// The chrome region that last had keyboard focus (`ObservatoryFocusZone`): drives the amber
/// tab underline and where focus returns on A→B→A.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FocusZone {
    #[default]
    Editor,
    Terminal,
    Agent,
    Other,
}

/// The two editor groups of the split.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Main,
    Split,
}

impl Pane {
    /// The TextMate document / tab park key (`parkPane`).
    pub fn key(self) -> &'static str {
        match self {
            Pane::Main => "main",
            Pane::Split => "split",
        }
    }
}

/// Editor vs branch review in the main column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MainSurface {
    #[default]
    Editor,
    Review,
}

impl MainSurface {
    pub fn key(self) -> &'static str {
        match self {
            MainSurface::Editor => "editor",
            MainSurface::Review => "review",
        }
    }
}

/// A tab's buffer kept so switching back does not refetch or remount empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabBuffer {
    pub draft: String,
    pub saved: String,
    pub file: FileContent,
}

/// `isScratchPath`.
pub fn is_scratch_path(path: &str) -> bool {
    path.starts_with("scratch://")
}

/// `parentPath`.
pub fn parent_path(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// `joinPath`: `''` for an invalid name.
pub fn join_path(parent: &str, name: &str) -> String {
    let n = name.trim();
    if n.is_empty() || n.contains('/') || n == "." || n == ".." {
        return String::new();
    }
    if parent.is_empty() {
        n.to_string()
    } else {
        format!("{parent}/{n}")
    }
}

/// The last path segment.
pub fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// True for `path` or anything under it.
pub fn is_under(p: &str, root: &str) -> bool {
    p == root || (p.len() > root.len() && p.starts_with(root) && p.as_bytes()[root.len()] == b'/')
}

/// Rewrites `p` from under `from` to under `to`.
pub fn rewrite_under(p: &str, from: &str, to: &str) -> String {
    if p == from {
        to.to_string()
    } else {
        format!("{to}{}", &p[from.len()..])
    }
}

/// UTF-8 bytes of a buffer (`utf8Bytes`).
pub fn utf8_bytes(text: &str) -> i64 {
    text.len() as i64
}

/// `fileWithBody`: a file record carrying `body`.
pub fn file_with_body(src: &FileContent, body: &str) -> FileContent {
    FileContent {
        content: body.to_string(),
        size: utf8_bytes(body),
        ..src.clone()
    }
}

/// `Number.prototype.toFixed`: ties go to the larger value, unlike Rust's formatter.
pub fn js_to_fixed(x: f64, digits: usize) -> String {
    let scale = 10f64.powi(digits as i32);
    let scaled = x * scale;
    let floor = scaled.floor();
    let n = if scaled - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    };
    format!("{:.*}", digits, n / scale)
}

/// `formatBytes`.
pub fn format_bytes(n: i64) -> String {
    let f = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        let digits = if n < 10 * 1024 { 1 } else { 0 };
        format!("{} KB", js_to_fixed(f / 1024.0, digits))
    } else if n < 1024 * 1024 * 1024 {
        format!("{} MB", js_to_fixed(f / (1024.0 * 1024.0), 1))
    } else {
        format!("{} GB", js_to_fixed(f / (1024.0 * 1024.0 * 1024.0), 2))
    }
}

/// Above this the buffer is view-only.
pub const MAX_EDIT_BYTES: i64 = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers() {
        assert_eq!(parent_path("a/b/c.go"), "a/b");
        assert_eq!(parent_path("c.go"), "");
        assert_eq!(join_path("a", " b.go "), "a/b.go");
        assert_eq!(join_path("", "b.go"), "b.go");
        assert_eq!(join_path("a", "x/y"), "");
        assert_eq!(join_path("a", ".."), "");
        assert!(is_under("a/b", "a"));
        assert!(is_under("a", "a"));
        assert!(!is_under("ab", "a"));
        assert_eq!(rewrite_under("a/b/c", "a/b", "z"), "z/c");
    }

    #[test]
    fn format_bytes_like_the_web() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(20 * 1024), "20 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 / 2), "1.5 MB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024 / 2), "2.50 GB");
        assert_eq!(format_bytes(10752), "11 KB");
        assert_eq!(js_to_fixed(2.5, 0), "3");
    }
}
