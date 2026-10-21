//! Explorer mutations: create, mkdir, rename, delete, copy. All targets are
//! sandboxed under the root, and the root itself is refused.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::gopath;
use crate::root::{path_str, safe_join_str};
use crate::walk::read_dir_sorted;

/// Creates an empty file under `root`, with parents. Fails if the path exists.
pub fn create_file(root: &Path, rel: &str) -> Result<()> {
    let full = safe_target(root, rel)?;
    not_existing(&full, rel)?;
    gopath::mkdir_all(&gopath::dir(&full))?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&full)
        .map_err(|e| Error::path("open", &full, e))?;
    Ok(())
}

/// Creates a directory under `root` (and missing parents). A no-op when the
/// directory exists; fails when a file is there.
pub fn mkdir(root: &Path, rel: &str) -> Result<()> {
    let full = safe_target(root, rel)?;
    match fs::symlink_metadata(&full) {
        Ok(st) if st.is_dir() => Ok(()),
        Ok(_) => Err(Error::msg(format!("{rel} already exists"))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => gopath::mkdir_all(&full),
        Err(e) => Err(Error::path("lstat", &full, e)),
    }
}

/// Moves a file or directory within `root`. The destination must not exist.
pub fn rename(root: &Path, from_rel: &str, to_rel: &str) -> Result<()> {
    let from = safe_target(root, from_rel)?;
    let to = safe_target(root, to_rel)?;
    if from == to {
        return Ok(());
    }
    lstat(&from)?;
    not_existing(&to, to_rel)?;
    gopath::mkdir_all(&gopath::dir(&to))?;
    if is_nested_path(&from, &to) {
        return Err(Error::msg("cannot move into itself"));
    }
    fs::rename(&from, &to).map_err(|e| Error::path("rename", &format!("{from} {to}"), e))
}

/// Deletes a file or directory tree under `root`.
pub fn remove(root: &Path, rel: &str) -> Result<()> {
    let full = safe_target(root, rel)?;
    let st = lstat(&full)?;
    let res = if st.is_dir() {
        fs::remove_dir_all(&full)
    } else {
        fs::remove_file(&full)
    };
    match res {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(Error::path("unlinkat", &full, e)),
        _ => Ok(()),
    }
}

/// Duplicates a file or directory tree within `root`. The destination must
/// not exist. Symlinks inside a copied directory are skipped to stay inside
/// the sandbox.
pub fn copy(root: &Path, from_rel: &str, to_rel: &str) -> Result<()> {
    let from = safe_target(root, from_rel)?;
    let to = safe_target(root, to_rel)?;
    if from == to {
        return Err(Error::msg("source and destination are the same"));
    }
    let st = lstat(&from)?;
    not_existing(&to, to_rel)?;
    if st.is_dir() {
        if is_nested_path(&from, &to) {
            return Err(Error::msg("cannot copy into itself"));
        }
        return copy_dir(&from, &to);
    }
    gopath::mkdir_all(&gopath::dir(&to))?;
    copy_file(&from, &to, st.permissions().mode())
}

/// The absolute filesystem path for a repo-relative path.
pub fn abs_path(root: &Path, rel: &str) -> Result<PathBuf> {
    crate::safe_join(root, rel)
}

/// [`crate::safe_join`] that also rejects empty and root targets.
fn safe_target(root: &Path, rel: &str) -> Result<String> {
    let rel = rel.trim();
    let rel = rel.strip_prefix('/').unwrap_or(rel);
    if rel.is_empty() || rel == "." {
        return Err(Error::msg("path required"));
    }
    safe_join_str(&path_str(root), rel)
}

fn lstat(p: &str) -> Result<fs::Metadata> {
    fs::symlink_metadata(p).map_err(|e| Error::path("lstat", p, e))
}

fn not_existing(full: &str, rel: &str) -> Result<()> {
    match fs::symlink_metadata(full) {
        Ok(_) => Err(Error::msg(format!("{rel} already exists"))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::path("lstat", full, e)),
    }
}

fn is_nested_path(parent: &str, child: &str) -> bool {
    child.starts_with(&format!("{parent}/"))
}

fn copy_file(from: &str, to: &str, mode: u32) -> Result<()> {
    let mut input = File::open(from).map_err(|e| Error::path("open", from, e))?;
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode & 0o777)
        .open(to)
        .map_err(|e| Error::path("open", to, e))?;
    if let Err(e) = io::copy(&mut input, &mut out) {
        drop(out);
        let _ = fs::remove_file(to);
        return Err(Error::path("write", to, e));
    }
    Ok(())
}

fn copy_dir(from: &str, to: &str) -> Result<()> {
    gopath::mkdir_all(to)?;
    let entries = read_dir_sorted(Path::new(from)).map_err(|e| Error::path("open", from, e))?;
    for e in entries {
        let src = gopath::join(&[from, &e.name]);
        let dst = gopath::join(&[to, &e.name]);
        let info = e.info().map_err(|err| Error::path("lstat", &src, err))?;
        if e.is_dir() {
            copy_dir(&src, &dst)?;
            continue;
        }
        if info.file_type().is_symlink() {
            continue;
        }
        copy_file(&src, &dst, info.permissions().mode())?;
    }
    Ok(())
}
