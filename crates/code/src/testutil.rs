//! Test helpers.

use std::fs;
use std::path::Path;

/// A temp dir whose name does not start with `.`: the search, symbol and
/// metrics walks skip dot-dirs, the walked root included (as in Go).
pub(crate) fn tmp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("recur-code-")
        .tempdir()
        .expect("temp dir")
}

/// Writes `body` to `root/rel`, creating parents.
pub(crate) fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(p, body).expect("write");
}

/// Runs git in `root` with a fixed identity.
pub(crate) fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
