//! Shared test helpers.
#![allow(dead_code)]

use std::path::Path;

use recur_domain::path;

/// Repo-root `bundles/` (the shipped reference packs).
pub fn shipped_root() -> String {
    repo_path("bundles")
}

/// A path under the repo root.
pub fn repo_path(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    path::clean(&root.to_string_lossy())
}

/// Writes `body` to `path`, creating parent directories.
pub fn must_write(path: &str, body: &str) {
    let p = Path::new(path);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, body).expect("write");
}

/// Writes `root/name/bundle.toml` and returns the bundle dir.
pub fn write_bundle(root: &str, name: &str, toml: &str) -> String {
    let dir = format!("{root}/{name}");
    must_write(&format!("{dir}/bundle.toml"), toml);
    dir
}

/// A fresh temp dir as a string path (kept alive by the returned guard).
pub fn temp_dir() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().expect("tempdir");
    let s = d.path().to_string_lossy().into_owned();
    (d, s)
}
