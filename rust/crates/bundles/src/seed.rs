//! Seeding shipped bundles and skills into the data dir.
//!
//! Each top-level directory is content-hashed; when the hash differs from
//! the destination's `.recur-seed` stamp (or the stamp is missing) the
//! directory is replaced entirely, so upgrades pick up new defaults and
//! removed files disappear. Unchanged bundles are left alone so local tweaks
//! survive until the next shipped change. Top-level loose files are
//! overwritten when their bytes differ. Customize via `~/.recur/bundles`
//! (id wins) instead of editing the seeded copy.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;

use include_dir::{Dir, DirEntry};
use recur_domain::path;
use sha2::{Digest, Sha256};

use crate::{Error, err};

/// Written inside each seeded bundle directory: hex SHA-256 of the shipped tree.
pub const SEED_STAMP_NAME: &str = ".recur-seed";

/// Shipped Recur Bundles (repo-root `bundles/`), seeded into `<DataDir>/Bundles`.
pub static SHIPPED_BUNDLES: Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../../bundles");

/// Shipped Recur-owned Agent Skills (repo-root `recur-skills/`), seeded
/// into `~/.recur/recur-skills`.
pub static SHIPPED_SKILLS: Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../../recur-skills");

/// A read-only tree to seed from (Go `fs.FS`). Paths are slash-separated
/// and relative to the source root; `"."` is the root.
pub trait SeedSource {
    /// Entries of `dir` as `(name, is_dir)`, sorted by name.
    fn read_dir(&self, dir: &str) -> Result<Vec<(String, bool)>, String>;
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String>;
}

impl SeedSource for Dir<'static> {
    fn read_dir(&self, dir: &str) -> Result<Vec<(String, bool)>, String> {
        let d = if dir == "." {
            self
        } else {
            self.get_dir(dir)
                .ok_or_else(|| format!("{dir}: file does not exist"))?
        };
        let mut out: Vec<(String, bool)> = d
            .entries()
            .iter()
            .map(|e| {
                let name = path::base(&e.path().to_string_lossy());
                (name, matches!(e, DirEntry::Dir(_)))
            })
            .collect();
        out.sort();
        Ok(out)
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        self.get_file(path)
            .map(|f| f.contents().to_vec())
            .ok_or_else(|| format!("{path}: file does not exist"))
    }
}

/// A directory on disk (Go `os.DirFS`).
pub struct DirSource(pub String);

impl SeedSource for DirSource {
    fn read_dir(&self, dir: &str) -> Result<Vec<(String, bool)>, String> {
        let full = path::join(&[&self.0, dir]);
        crate::registry::read_dir_sorted(&full).map_err(|e| format!("{dir}: {e}"))
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let full = path::join(&[&self.0, path]);
        fs::read(&full).map_err(|e| format!("{path}: {e}"))
    }
}

/// An in-memory tree keyed by slash path (Go `fstest.MapFS`).
#[derive(Debug, Default, Clone)]
pub struct MapSource(pub BTreeMap<String, Vec<u8>>);

impl SeedSource for MapSource {
    fn read_dir(&self, dir: &str) -> Result<Vec<(String, bool)>, String> {
        let prefix = if dir == "." {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut out: BTreeMap<String, bool> = BTreeMap::new();
        for path in self.0.keys() {
            if let Some(rest) = path.strip_prefix(&prefix) {
                match rest.split_once('/') {
                    Some((child, _)) => {
                        out.insert(child.to_string(), true);
                    }
                    None => {
                        out.entry(rest.to_string()).or_insert(false);
                    }
                }
            }
        }
        if out.is_empty() && dir != "." {
            return Err(format!("{dir}: file does not exist"));
        }
        Ok(out.into_iter().collect())
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| format!("{path}: file does not exist"))
    }
}

/// Copies shipped content from `src` (rooted at `root`, e.g. `"."`) into
/// `dest_dir` with the stamp/sync semantics described on the module.
pub fn seed_defaults(dest_dir: &str, src: &dyn SeedSource, root: &str) -> Result<(), Error> {
    let root = if root.is_empty() { "." } else { root };
    mkdir_all(dest_dir).map_err(|e| err!("{e}"))?;
    let entries = src
        .read_dir(root)
        .map_err(|e| err!("seed defaults: read {root}: {e}"))?;
    for (name, is_dir) in entries {
        if name == SEED_STAMP_NAME || name.starts_with('.') {
            continue;
        }
        let src_path = path_join(root, &name);
        if is_dir {
            seed_bundle_dir(dest_dir, src, &src_path, &name)?;
        } else {
            seed_root_file(dest_dir, src, &src_path, &name)?;
        }
    }
    Ok(())
}

fn seed_bundle_dir(
    dest_dir: &str,
    src: &dyn SeedSource,
    src_path: &str,
    name: &str,
) -> Result<(), Error> {
    let hash = hash_tree(src, src_path).map_err(|e| err!("seed hash {name}: {e}"))?;
    let dest_bundle = path::join(&[dest_dir, name]);
    let stamp_path = path::join(&[&dest_bundle, SEED_STAMP_NAME]);
    if let Ok(cur) = fs::read(&stamp_path)
        && String::from_utf8_lossy(&cur).trim() == hash
    {
        return Ok(());
    }
    remove_all(&dest_bundle).map_err(|e| err!("seed replace {name}: {e}"))?;
    copy_tree(src, src_path, &dest_bundle).map_err(|e| err!("{e}"))?;
    write_file(&stamp_path, format!("{hash}\n").as_bytes(), 0o644)
        .map_err(|e| err!("seed stamp {name}: {e}"))
}

fn seed_root_file(
    dest_dir: &str,
    src: &dyn SeedSource,
    src_path: &str,
    name: &str,
) -> Result<(), Error> {
    let data = src
        .read_file(src_path)
        .map_err(|e| err!("seed read {name}: {e}"))?;
    let target = path::join(&[dest_dir, name]);
    if fs::read(&target).is_ok_and(|cur| cur == data) {
        return Ok(());
    }
    let mode = if name.ends_with(".sh") { 0o755 } else { 0o644 };
    write_file(&target, &data, mode).map_err(|e| err!("seed {name}: {e}"))
}

/// Walks `dir` in lexical order (Go `fs.WalkDir`), calling `visit` with the
/// slash path and whether it is a directory. Skipped directories are pruned.
fn walk(
    src: &dyn SeedSource,
    dir: &str,
    root: &str,
    visit: &mut dyn FnMut(&str, bool) -> Result<(), String>,
) -> Result<(), String> {
    visit(dir, true)?;
    for (name, is_dir) in src.read_dir(dir)? {
        let path = path_join(dir, &name);
        if skip_seed_path(root, &path) {
            continue;
        }
        if is_dir {
            walk(src, &path, root, visit)?;
        } else {
            visit(&path, false)?;
        }
    }
    Ok(())
}

fn hash_tree(src: &dyn SeedSource, dir: &str) -> Result<String, String> {
    let mut h = Sha256::new();
    walk(src, dir, dir, &mut |path, is_dir| {
        if is_dir {
            return Ok(());
        }
        let rel = path::rel(dir, path).ok_or_else(|| format!("{path} is outside {dir}"))?;
        let data = src.read_file(path)?;
        // path\0len\0bytes\0: stable, collision-resistant tree identity.
        h.update(format!("{rel}\0{}\0", data.len()).as_bytes());
        h.update(&data);
        h.update([0u8]);
        Ok(())
    })?;
    Ok(hex::encode(h.finalize()))
}

fn copy_tree(src: &dyn SeedSource, src_dir: &str, dest_dir: &str) -> Result<(), String> {
    walk(src, src_dir, src_dir, &mut |path, is_dir| {
        let rel = path::rel(src_dir, path).ok_or_else(|| format!("{path} is outside {src_dir}"))?;
        if rel == "." {
            return mkdir_all(dest_dir);
        }
        let target = path::join(&[dest_dir, &rel]);
        if is_dir {
            return mkdir_all(&target);
        }
        let data = src.read_file(path)?;
        let mode = if path.ends_with(".sh") || path.contains("/bin/") {
            0o755
        } else {
            0o644
        };
        mkdir_all(&path::dir(&target))?;
        write_file(&target, &data, mode).map_err(|e| format!("seed {rel}: {e}"))
    })
}

fn path_join(root: &str, name: &str) -> String {
    if root == "." {
        name.to_string()
    } else {
        format!("{root}/{name}")
    }
}

/// Authoring-only paths stay out of seeded runtime copies: `.recur-seed`,
/// dotfiles, and the bundle's top-level `tests/` directory.
fn skip_seed_path(bundle_root: &str, path: &str) -> bool {
    let base = path::base(path);
    if base == SEED_STAMP_NAME || base.starts_with('.') {
        return true;
    }
    match path::rel(bundle_root, path) {
        Some(rel) => rel == "tests" || rel.starts_with("tests/"),
        None => false,
    }
}

fn mkdir_all(dir: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))
}

fn remove_all(dir: &str) -> Result<(), String> {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(m) if !m.is_dir() => fs::remove_file(dir).map_err(|e| format!("{dir}: {e}")),
        _ => fs::remove_dir_all(dir).map_err(|e| format!("{dir}: {e}")),
    }
}

/// Go `os.WriteFile`: create/truncate with `mode` for new files.
pub(crate) fn write_file(path: &str, data: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(|e| format!("{path}: {e}"))?;
    f.write_all(data).map_err(|e| format!("{path}: {e}"))
}
