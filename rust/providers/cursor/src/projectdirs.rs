//! `~/.cursor/projects/<slug>` for a worktree (Go `cursor/projectdirs.go`).

use std::path::{Path, PathBuf};

use crate::locate::clean;

/// The directory name under `~/.cursor/projects/` for a workspace path.
/// Byte-compatible with the SDK's sanitizer (`/[^a-zA-Z0-9]/g` → "-",
/// collapsed, edge dashes trimmed): agents are told to look at
/// `~/.cursor/projects/<slug>/terminals` for prior shell output.
pub fn cursor_project_slug(workspace: &str) -> String {
    let workspace = crate::gojson::trim(workspace);
    if workspace.is_empty() {
        return String::new();
    }
    let mut s = String::with_capacity(workspace.len());
    for c in workspace.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    s.trim_matches('-').to_owned()
}

/// `~/.cursor/projects/<slug>`, or `None` when the path cannot be resolved
/// safely.
fn cursor_project_root(home: Option<&Path>, workspace: &str) -> std::io::Result<Option<PathBuf>> {
    let slug = cursor_project_slug(workspace);
    if slug.is_empty() || slug == "." || slug.contains("..") {
        return Ok(None);
    }
    let home = match home {
        Some(h) => h.to_path_buf(),
        None => match std::env::var_os("HOME") {
            Some(h) if !h.is_empty() => PathBuf::from(h),
            _ => return Err(std::io::Error::other("$HOME is not defined")),
        },
    };
    let projects = clean(&home.join(".cursor").join("projects"));
    let root = clean(&projects.join(&slug));
    // Refuse anything that escapes ~/.cursor/projects.
    match root.strip_prefix(&projects) {
        Ok(rel) if rel.as_os_str().is_empty() => Ok(None),
        Ok(_) => Ok(Some(root)),
        Err(_) => Err(std::io::Error::other(format!(
            "cursor project path {:?} escapes projects dir",
            root.display().to_string()
        ))),
    }
}

/// Creates the project dirs agents pass as Shell `working_directory`,
/// especially `terminals/`. Node reports a missing cwd as
/// `Error: spawn /bin/zsh ENOENT`, which Cursor does not catch: it kills the
/// whole bridge and leaves the agent with a stuck active run. Recur-only
/// worktrees never get these dirs from the SDK.
pub fn ensure_cursor_project_dirs(workspace: &Path) -> std::io::Result<()> {
    ensure_cursor_project_dirs_in(None, workspace)
}

/// [`ensure_cursor_project_dirs`] under an explicit home (`None` = `$HOME`).
pub(crate) fn ensure_cursor_project_dirs_in(
    home: Option<&Path>,
    workspace: &Path,
) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let Some(root) = cursor_project_root(home, &workspace.to_string_lossy())? else {
        return Ok(());
    };
    for name in ["terminals", "agent-transcripts"] {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root.join(name))?;
    }
    Ok(())
}

/// Deletes the whole `~/.cursor/projects/<slug>/` tree for a worktree being
/// removed so Cursor metadata does not accumulate. Missing dirs are fine.
pub fn remove_cursor_project_dirs(workspace: &Path) -> std::io::Result<()> {
    remove_cursor_project_dirs_in(None, workspace)
}

pub(crate) fn remove_cursor_project_dirs_in(
    home: Option<&Path>,
    workspace: &Path,
) -> std::io::Result<()> {
    let Some(root) = cursor_project_root(home, &workspace.to_string_lossy())? else {
        return Ok(());
    };
    // Go `os.RemoveAll`: a file or link at the path goes too.
    let res = match std::fs::symlink_metadata(&root) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(&root),
        Ok(_) => std::fs::remove_file(&root),
        Err(e) => Err(e),
    };
    match res {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug() {
        assert_eq!(
            cursor_project_slug("/Users/josh/.recur/worktrees/velvety-croissant"),
            "Users-josh-recur-worktrees-velvety-croissant"
        );
        assert_eq!(cursor_project_slug("  "), "");
        assert_eq!(cursor_project_slug("/a__b/é"), "a-b");
    }

    // Go TestEnsureCursorProjectDirsCreatesTerminals and
    // TestRemoveCursorProjectDirsDeletesSlugTree, against an explicit home.
    #[test]
    fn ensure_and_remove() {
        let home = tempfile::tempdir().unwrap();
        let h = Some(home.path());
        let ws = Path::new("/Users/josh/.recur/worktrees/velvety-croissant");
        ensure_cursor_project_dirs_in(h, ws).unwrap();
        let root = home
            .path()
            .join(".cursor/projects")
            .join(cursor_project_slug(&ws.to_string_lossy()));
        assert!(root.join("terminals").is_dir());
        std::fs::create_dir_all(root.join("sdk-agent-store")).unwrap();
        remove_cursor_project_dirs_in(h, ws).unwrap();
        assert!(!root.exists());
        remove_cursor_project_dirs_in(h, ws).unwrap();
    }
}
