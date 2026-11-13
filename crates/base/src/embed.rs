//! Directory trees embedded at build time. A crate's `build.rs` calls [`generate`] and the
//! crate includes the result:
//!
//! ```ignore
//! // build.rs
//! fn main() { recur_base::embed::generate("assets/icons", "icons.rs"); }
//! // src
//! static ICONS: recur_base::embed::Dir = include!(concat!(env!("OUT_DIR"), "/icons.rs"));
//! ```

use std::fmt::Write as _;
use std::path::Path;

/// A directory: its path from the embedded root (`""` for the root) and its entries, sorted
/// by name.
pub struct Dir {
    pub path: &'static str,
    pub entries: &'static [Entry],
}

pub struct File {
    pub path: &'static str,
    pub contents: &'static [u8],
}

pub enum Entry {
    Dir(Dir),
    File(File),
}

impl Entry {
    pub fn path(&self) -> &'static str {
        match self {
            Entry::Dir(d) => d.path,
            Entry::File(f) => f.path,
        }
    }

    pub fn name(&self) -> &'static str {
        let p = self.path();
        p.rsplit_once('/').map_or(p, |(_, n)| n)
    }
}

impl Dir {
    /// The files directly in this directory.
    pub fn files(&self) -> impl Iterator<Item = &File> {
        self.entries.iter().filter_map(|e| match e {
            Entry::File(f) => Some(f),
            Entry::Dir(_) => None,
        })
    }

    /// The directory at `path` (`/`-separated, from this one).
    pub fn get_dir(&self, path: &str) -> Option<&Dir> {
        path.split('/')
            .filter(|p| !p.is_empty())
            .try_fold(self, |d, part| match d.entry(part)? {
                Entry::Dir(d) => Some(d),
                Entry::File(_) => None,
            })
    }

    /// The file at `path` (`/`-separated, from this one).
    pub fn get_file(&self, path: &str) -> Option<&File> {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        match self.get_dir(dir)?.entry(name)? {
            Entry::File(f) => Some(f),
            Entry::Dir(_) => None,
        }
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        let i = self.entries.binary_search_by(|e| e.name().cmp(name)).ok()?;
        Some(&self.entries[i])
    }
}

/// From a `build.rs`: writes `$OUT_DIR/<out>`, a [`Dir`] expression embedding every file
/// under `dir` (relative to the crate), and reruns the build script when anything in it
/// changes.
pub fn generate(dir: &str, out: &str) {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let root = Path::new(&manifest).join(dir);
    println!("cargo:rerun-if-changed={}", root.display());
    let mut src = String::from("{ use recur_base::embed::*; ");
    write_dir(&mut src, &root, "");
    src.push_str(" }");
    std::fs::write(Path::new(&out_dir).join(out), src).expect("write embedded dir");
}

fn write_dir(src: &mut String, abs: &Path, rel: &str) {
    let mut entries: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(abs)
        .unwrap_or_else(|e| panic!("{}: {e}", abs.display()))
        .map(|e| {
            let e = e.expect("read_dir entry");
            let name = e.file_name().into_string().expect("UTF-8 file name");
            (name, e.path())
        })
        .collect();
    entries.sort();
    let _ = write!(src, "Dir {{ path: {rel:?}, entries: &[");
    for (name, path) in entries {
        let rel = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if std::fs::metadata(&path).expect("metadata").is_dir() {
            src.push_str("Entry::Dir(");
            write_dir(src, &path, &rel);
            src.push_str("),");
        } else {
            let abs = path.to_str().expect("UTF-8 path");
            let _ = write!(
                src,
                "Entry::File(File {{ path: {rel:?}, contents: include_bytes!({abs:?}) }}),"
            );
        }
    }
    src.push_str("] }");
}

#[cfg(test)]
mod tests {
    use super::*;

    static TREE: Dir = Dir {
        path: "",
        entries: &[
            Entry::File(File {
                path: "a.txt",
                contents: b"a",
            }),
            Entry::Dir(Dir {
                path: "d",
                entries: &[Entry::File(File {
                    path: "d/b.svg",
                    contents: b"b",
                })],
            }),
            Entry::File(File {
                path: "z.json",
                contents: b"z",
            }),
        ],
    };

    #[test]
    fn looks_up_files_and_dirs() {
        assert_eq!(TREE.get_file("a.txt").unwrap().contents, b"a");
        assert_eq!(TREE.get_file("d/b.svg").unwrap().contents, b"b");
        assert_eq!(TREE.get_file("z.json").unwrap().contents, b"z");
        assert!(TREE.get_file("d").is_none());
        assert!(TREE.get_file("d/nope").is_none());
        assert_eq!(TREE.get_dir("d").unwrap().path, "d");
        assert_eq!(TREE.get_dir("").unwrap().path, "");
        assert!(TREE.get_dir("a.txt").is_none());
        let names: Vec<_> = TREE.files().map(|f| f.path).collect();
        assert_eq!(names, ["a.txt", "z.json"]);
        assert_eq!(TREE.entries[1].name(), "d");
    }
}
