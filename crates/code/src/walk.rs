//! A directory walk: lexical order, symlinks not followed, the callback
//! decides whether to descend, skip a directory or stop the whole walk.

use std::fs::{self, FileType, Metadata};
use std::io;
use std::path::{Path, PathBuf};

use recur_domain::path;

/// What a walk callback asks for next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Continue,
    SkipDir,
    SkipAll,
}

/// One visited path.
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    ft: Option<FileType>,
}

impl Entry {
    pub(crate) fn is_dir(&self) -> bool {
        self.ft.is_some_and(|t| t.is_dir())
    }

    pub(crate) fn is_file(&self) -> bool {
        self.ft.is_some_and(|t| t.is_file())
    }

    pub(crate) fn is_symlink(&self) -> bool {
        self.ft.is_some_and(|t| t.is_symlink())
    }

    /// The entry's lstat.
    pub(crate) fn info(&self) -> io::Result<Metadata> {
        fs::symlink_metadata(&self.path)
    }
}

/// Walks `root`. The callback sees each path, its entry, and any lstat/readdir
/// error for it (then the entry may be missing).
pub(crate) fn walk_dir<E>(
    root: &str,
    mut f: impl FnMut(&str, Option<&Entry>, Option<io::Error>) -> Result<Flow, E>,
) -> Result<(), E> {
    let flow = match fs::symlink_metadata(root) {
        Err(e) => f(root, None, Some(e))?,
        Ok(m) => {
            let entry = Entry {
                name: path::base(root),
                path: PathBuf::from(root),
                ft: Some(m.file_type()),
            };
            walk(root, &entry, &mut f)?
        }
    };
    let _ = flow;
    Ok(())
}

fn walk<E>(
    path: &str,
    d: &Entry,
    f: &mut impl FnMut(&str, Option<&Entry>, Option<io::Error>) -> Result<Flow, E>,
) -> Result<Flow, E> {
    let flow = f(path, Some(d), None)?;
    if flow != Flow::Continue || !d.is_dir() {
        if flow == Flow::SkipDir && d.is_dir() {
            return Ok(Flow::Continue);
        }
        return Ok(flow);
    }
    let entries = match read_dir_sorted(&d.path) {
        Ok(entries) => entries,
        Err(e) => {
            let flow = f(path, Some(d), Some(e))?;
            if flow != Flow::Continue {
                if flow == Flow::SkipDir && d.is_dir() {
                    return Ok(Flow::Continue);
                }
                return Ok(flow);
            }
            Vec::new()
        }
    };
    for e in entries {
        let child = path::join(&[path, &e.name]);
        match walk(&child, &e, f)? {
            Flow::Continue => {}
            Flow::SkipDir => break,
            Flow::SkipAll => return Ok(Flow::SkipAll),
        }
    }
    Ok(Flow::Continue)
}

/// Entries sorted by name.
pub(crate) fn read_dir_sorted(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in fs::read_dir(dir)? {
        let e = e?;
        out.push(Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path(),
            ft: e.file_type().ok(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
