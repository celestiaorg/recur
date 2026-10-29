//! File drags from explorer rows and OS/Finder drops: workspace-relative
//! `@file` chips and image attachments for the composer, shell-quoted paths for the terminal.
//!
//! An explorer row drags a [`WorkspaceFileDrag`] (GPUI's typed drag payload replaces the
//! DataTransfer MIME and module state); OS drops arrive as absolute paths.

use std::sync::LazyLock;

use regex::Regex;

/// The intra-app drag payload of explorer rows: workspace-relative paths (a trailing `/` marks a
/// folder).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceFileDrag {
    pub paths: Vec<String>,
}

impl WorkspaceFileDrag {
    pub fn new(paths: &[String]) -> Self {
        WorkspaceFileDrag {
            paths: normalize_file_drag_paths(paths),
        }
    }
}

/// `normalizeFileDragPaths`: trim, forward slashes, no leading slash, dedupe.
pub fn normalize_file_drag_paths(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in paths {
        let p = raw.trim().replace('\\', "/");
        let p = p.trim_start_matches('/');
        if p.is_empty() || out.iter().any(|o| o == p) {
            continue;
        }
        out.push(p.to_string());
    }
    out
}

static IMAGE_EXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\.(png|jpe?g|gif|webp|bmp|svg|ico|avif|heic|heif)$").expect("image ext")
});
static CHAT_IMAGE_EXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\.(png|jpe?g|gif|webp)$").expect("chat image ext"));
static SHELL_SAFE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_./:@%+=,-]+$").expect("shell safe"));
static DRIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z]:/").expect("drive"));
static SLASH_DRIVE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/[A-Za-z]:/").expect("slash drive"));

fn base_name(path: &str) -> &str {
    let t = path.trim();
    t.rsplit(['/', '\\']).next().unwrap_or(t)
}

/// `isImagePath`.
pub fn is_image_path(path: &str) -> bool {
    IMAGE_EXT.is_match(base_name(path))
}

/// png/jpeg/gif/webp: the set chat paste and drop accept (`isChatImagePath`).
pub fn is_chat_image_path(path: &str) -> bool {
    CHAT_IMAGE_EXT.is_match(base_name(path))
}

/// Whether `@path` survives `FILE_AT_RE` (no whitespace or `@`).
pub fn is_chipable_file_path(path: &str) -> bool {
    let p = path.trim();
    !p.is_empty() && !p.chars().any(|c| c.is_whitespace() || c == '@')
}

/// POSIX single-quoting so zsh/bash see one argv (`quoteForShell`).
pub fn quote_for_shell(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    if SHELL_SAFE.is_match(path) {
        return path.to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `pathFromFileUrl`: `file:///abs/path` (and absolute paths) to a POSIX-ish path; "" for
/// anything else.
pub fn path_from_file_url(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    if s.len() >= 5 && s[..5].eq_ignore_ascii_case("file:") {
        let rest = &s[5..];
        let path = match rest.strip_prefix("//") {
            Some(after) => match after.find('/') {
                Some(i) => &after[i..],
                None => return String::new(),
            },
            None => rest,
        };
        let path = path.split(['?', '#']).next().unwrap_or("");
        let Some(mut p) = percent_decode(path) else {
            return String::new();
        };
        if SLASH_DRIVE.is_match(&p) {
            p.remove(0);
        }
        return p;
    }
    let posix = s.replace('\\', "/");
    if posix.starts_with('/') || DRIVE.is_match(&posix) {
        return posix;
    }
    String::new()
}

/// An absolute filesystem path (Finder drop, `@/Users/…` chip), not worktree-relative.
pub fn is_abs_filesystem_path(path: &str) -> bool {
    let p = path.trim().replace('\\', "/");
    if p.is_empty() || p.starts_with("scratch://") {
        return false;
    }
    p.starts_with('/') || DRIVE.is_match(&p)
}

/// Joins a workspace-relative explorer path onto `root`; absolute paths pass through.
pub fn resolve_explorer_drop_path(rel: &str, root: &str) -> String {
    let p = rel.trim().replace('\\', "/");
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        return String::new();
    }
    if is_abs_filesystem_path(p) {
        return p.to_string();
    }
    let r = root.trim().replace('\\', "/");
    let r = r.trim_end_matches('/');
    if r.is_empty() {
        return p.to_string();
    }
    format!("{r}/{}", p.trim_start_matches('/'))
}

/// What a drop types into a terminal (`terminalDropText`).
pub fn terminal_drop_text(paths: &[String], root: &str) -> String {
    paths
        .iter()
        .map(|p| {
            let target = if root.is_empty() {
                let t = p.trim_end_matches('/');
                if t.is_empty() { p.as_str() } else { t }.to_string()
            } else {
                resolve_explorer_drop_path(p, root)
            };
            quote_for_shell(&target)
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The workspace-relative path when `path` is inside `root` (relative paths pass through).
pub fn relative_to_workspace(path: &str, root: &str) -> Option<String> {
    let p = path.trim().replace('\\', "/");
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        return None;
    }
    if !p.starts_with('/') {
        return Some(p.to_string());
    }
    let r = root.trim().replace('\\', "/");
    let r = r.trim_end_matches('/');
    if r.is_empty() {
        return None;
    }
    let (pl, rl) = (p.to_lowercase(), r.to_lowercase());
    if pl == rl {
        return None;
    }
    if pl.starts_with(&format!("{rl}/")) {
        return Some(p[r.len() + 1..].to_string());
    }
    None
}

/// How an OS drop lands in the composer (`classifyOsDrop`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OsDrop {
    /// `@file` chips: worktree-relative inside the worktree, absolute outside it.
    pub mentions: Vec<String>,
    /// Image blobs (by name) to attach.
    pub images: Vec<String>,
    /// Outside image paths to read and attach.
    pub image_paths: Vec<String>,
    /// Paths that cannot be chips (spaces).
    pub skipped: Vec<String>,
}

/// `classifyOsDrop`: worktree files (images included) stay chips; outside chat images attach;
/// other outside files chip by absolute path. `images` are blob names attached alongside.
pub fn classify_os_drop(paths: &[String], images: &[String], root_path: &str) -> OsDrop {
    let mut out = OsDrop::default();
    let mut mentioned: Vec<String> = Vec::new();
    for raw in paths {
        if let Some(rel) = relative_to_workspace(raw, root_path) {
            if is_chipable_file_path(&rel) {
                out.mentions.push(rel.clone());
            } else {
                out.skipped.push(rel.clone());
            }
            if let Some(base) = rel.rsplit('/').next().filter(|b| !b.is_empty()) {
                mentioned.push(base.to_string());
            }
            continue;
        }
        if is_chat_image_path(raw) {
            let base = base_name(raw);
            if !images.iter().any(|n| n == base) {
                out.image_paths.push(raw.clone());
            }
            continue;
        }
        if !raw.is_empty() && is_chipable_file_path(raw) {
            out.mentions.push(raw.clone());
        } else if !raw.is_empty() {
            out.skipped.push(raw.clone());
        }
    }
    out.images = images
        .iter()
        .filter(|n| !mentioned.contains(n))
        .cloned()
        .collect();
    out
}

/// Chat-tab keys are `workspaceId\tcode`; the drop target id uses a colon (`encodeChatDropId`).
pub fn encode_chat_drop_id(reset_key: &str) -> String {
    reset_key.replace('\t', ":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn normalize_trims_and_dedupes() {
        assert_eq!(
            normalize_file_drag_paths(&v(&[" /src/a.ts ", "\\src\\b.ts", "src/a.ts", ""])),
            v(&["src/a.ts", "src/b.ts"])
        );
    }

    #[test]
    fn file_urls() {
        assert_eq!(path_from_file_url("src/main.go"), "");
        assert_eq!(
            path_from_file_url("file:///Users/josh/src/a.ts"),
            "/Users/josh/src/a.ts"
        );
        assert_eq!(
            path_from_file_url("file:///Users/josh/My%20File.ts"),
            "/Users/josh/My File.ts"
        );
        assert_eq!(
            path_from_file_url("/Users/josh/src/a.ts"),
            "/Users/josh/src/a.ts"
        );
        assert_eq!(path_from_file_url("https://example.com/a.ts"), "");
    }

    #[test]
    fn relative_paths() {
        let root = "/mock/worktrees/loop-1";
        assert_eq!(
            relative_to_workspace("/mock/worktrees/loop-1/src/main.go", root).as_deref(),
            Some("src/main.go")
        );
        assert_eq!(
            relative_to_workspace("src/main.go", root).as_deref(),
            Some("src/main.go")
        );
        assert_eq!(relative_to_workspace("/elsewhere/main.go", root), None);
        assert_eq!(relative_to_workspace(root, root), None);
    }

    #[test]
    fn classify_drops() {
        let got = classify_os_drop(
            &v(&[
                "/mock/wt/src/main.go",
                "/Users/me/Desktop/shot.png",
                "/Users/me/notes.txt",
            ]),
            &v(&["shot.png"]),
            "/mock/wt",
        );
        assert_eq!(got.mentions, v(&["src/main.go", "/Users/me/notes.txt"]));
        assert_eq!(got.images, v(&["shot.png"]));
        assert!(got.image_paths.is_empty());

        let got = classify_os_drop(
            &v(&["/mock/wt/assets/icon.png"]),
            &v(&["icon.png"]),
            "/mock/wt",
        );
        assert_eq!(got.mentions, v(&["assets/icon.png"]));
        assert!(got.images.is_empty() && got.image_paths.is_empty());

        let got = classify_os_drop(
            &v(&["/Users/me/Desktop/shot.png", "/Users/me/notes.txt"]),
            &[],
            "/mock/wt",
        );
        assert_eq!(got.mentions, v(&["/Users/me/notes.txt"]));
        assert_eq!(got.image_paths, v(&["/Users/me/Desktop/shot.png"]));

        let got = classify_os_drop(
            &v(&["/Users/me/Documents/My Report.csv", "/Users/me/notes.txt"]),
            &[],
            "/mock/wt",
        );
        assert_eq!(got.mentions, v(&["/Users/me/notes.txt"]));
        assert_eq!(got.skipped, v(&["/Users/me/Documents/My Report.csv"]));

        let got = classify_os_drop(&v(&["/mock/wt/assets/icon.png"]), &[], "/mock/wt");
        assert_eq!(got.mentions, v(&["assets/icon.png"]));
        assert!(got.image_paths.is_empty());
    }

    #[test]
    fn drop_ids_and_predicates() {
        assert_eq!(encode_chat_drop_id("ws-1\tPA"), "ws-1:PA");
        assert_eq!(encode_chat_drop_id("ws-1:PA"), "ws-1:PA");
        assert!(is_image_path("a/b/c.PNG"));
        assert!(!is_image_path("src/main.go"));
        assert!(is_chat_image_path("/Users/me/Desktop/shot.png"));
        assert!(is_chat_image_path("photo.JPEG"));
        assert!(!is_chat_image_path("icon.ico"));
        assert!(!is_chat_image_path("logo.svg"));
        assert!(is_abs_filesystem_path(
            "/Users/josh/Downloads/export-111.csv"
        ));
        assert!(!is_abs_filesystem_path("src/main.go"));
        assert!(!is_abs_filesystem_path("scratch://1-notes.txt"));
        assert!(is_abs_filesystem_path("C:/Users/me/a.csv"));
        assert!(is_chipable_file_path("src/main.go"));
        assert!(is_chipable_file_path("/Users/me/notes.txt"));
        assert!(!is_chipable_file_path("/Users/me/My Report.csv"));
        assert!(!is_chipable_file_path("foo@bar.ts"));
    }

    #[test]
    fn terminal_text() {
        assert_eq!(quote_for_shell("src/main.go"), "src/main.go");
        assert_eq!(
            terminal_drop_text(&v(&["src/main.go", "README.md"]), ""),
            "src/main.go README.md"
        );
        assert_eq!(
            resolve_explorer_drop_path("src/main.go", "/tmp/wt"),
            "/tmp/wt/src/main.go"
        );
        assert_eq!(resolve_explorer_drop_path("src/", "/tmp/wt"), "/tmp/wt/src");
        assert_eq!(
            terminal_drop_text(&v(&["src/main.go", "src/"]), "/tmp/wt"),
            "/tmp/wt/src/main.go /tmp/wt/src"
        );
        assert_eq!(quote_for_shell("My File.ts"), "'My File.ts'");
        assert_eq!(quote_for_shell("it's.ts"), "'it'\\''s.ts'");
        assert_eq!(quote_for_shell("foo$.sh"), "'foo$.sh'");
        assert_eq!(quote_for_shell("*.go"), "'*.go'");
    }
}
