//! Recur's managed directories.
//!
//! The managed root is `<UserConfigDir>/Recur` and the worktree parent
//! `~/.recur/worktrees`, so it opens existing data. `RECUR_DATA_DIR` and
//! `RECUR_WORKTREES_DIR` override them.

use std::io;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use recur_project::home::{user_config_dir, user_home_dir};

/// Overrides the managed root entirely.
pub const DATA_DIR_ENV: &str = "RECUR_DATA_DIR";
/// Overrides the git-worktrees parent.
pub const WORKTREES_DIR_ENV: &str = "RECUR_WORKTREES_DIR";
/// The managed root's name under `UserConfigDir`.
pub const APP_DIR_NAME: &str = "Recur";
/// The worktrees parent's name under `~/.recur`.
pub const WORKTREES_DIR_NAME: &str = "worktrees";

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

fn mkdir_all(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(dir)
}

fn home() -> io::Result<PathBuf> {
    user_home_dir().map(PathBuf::from).map_err(io::Error::other)
}

/// Recur's managed root, created if needed: `<UserConfigDir>/Recur`
/// (`~/Library/Application Support/Recur` on macOS), or
/// `$RECUR_DATA_DIR`. It holds recur.db, per-workspace data under
/// `workspaces/<id>/`, the view-state JSON files and caches. Git worktrees
/// live separately under [`worktrees_dir`].
pub fn data_dir() -> io::Result<PathBuf> {
    let root = match env(DATA_DIR_ENV) {
        r if !r.is_empty() => PathBuf::from(r),
        _ => PathBuf::from(user_config_dir().map_err(io::Error::other)?).join(APP_DIR_NAME),
    };
    mkdir_all(&root)?;
    Ok(root)
}

/// The SQLite database under the managed root.
pub fn db_path(root: &Path) -> PathBuf {
    root.join("recur.db")
}

/// The unix socket the desktop app listens on so `recur .` can hand a path
/// to the running instance.
pub fn open_sock_path() -> io::Result<PathBuf> {
    Ok(data_dir()?.join("open.sock"))
}

/// The empty working-copy root of the singleton "Files" project that hosts
/// homeless (non-project) editor tabs, created if needed.
pub fn open_files_dir() -> io::Result<PathBuf> {
    let dir = data_dir()?.join("open-files");
    mkdir_all(&dir)?;
    Ok(dir)
}

/// The per-workspace git worktree parent, created if needed:
///
/// 1. `$RECUR_WORKTREES_DIR` when set
/// 2. `$RECUR_DATA_DIR/worktrees` when a data-dir override is active (keeps
///    tests and alternate instances self-contained)
/// 3. `~/.recur/worktrees`; checkouts live at
///    `<parent>/<project-slug>/<workspace-slug>`
///
/// On first use of the default, leftover worktrees under the legacy
/// `<UserConfigDir>/Recur/worktrees` are moved across (best-effort).
pub fn worktrees_dir() -> io::Result<PathBuf> {
    let v = env(WORKTREES_DIR_ENV);
    if !v.is_empty() {
        let dir = PathBuf::from(v);
        mkdir_all(&dir)?;
        return Ok(dir);
    }
    let data = env(DATA_DIR_ENV);
    if !data.is_empty() {
        let dir = PathBuf::from(data).join("worktrees");
        mkdir_all(&dir)?;
        return Ok(dir);
    }
    let dir = home()?.join(".recur").join(WORKTREES_DIR_NAME);
    mkdir_all(&dir)?;
    if let Ok(cfg) = user_config_dir() {
        migrate_legacy_worktrees(
            &PathBuf::from(cfg).join(APP_DIR_NAME).join("worktrees"),
            &dir,
        );
    }
    Ok(dir)
}

/// Moves entries from the pre-split legacy worktrees dir into new_root when
/// new_root is empty. Best-effort: failures leave the legacy tree alone.
pub(crate) fn migrate_legacy_worktrees(legacy: &Path, new_root: &Path) {
    if legacy == new_root {
        return;
    }
    let Ok(entries) = std::fs::read_dir(legacy) else {
        return;
    };
    let entries: Vec<_> = entries.filter_map(Result::ok).collect();
    if entries.is_empty() {
        return;
    }
    let empty = std::fs::read_dir(new_root).is_ok_and(|mut d| d.next().is_none());
    if !empty {
        return; // already populated: don't merge or clobber
    }
    for e in entries {
        let name = e.file_name();
        // A cross-device rename fails; leave the legacy tree in place.
        if std::fs::rename(legacy.join(&name), new_root.join(&name)).is_err() {
            return;
        }
    }
    let _ = std::fs::remove_dir(legacy); // only if empty
}

/// The per-workspace managed data directory `<root>/workspaces/<id>/`,
/// created if needed. Agent artifacts, chat attachments and QA screenshots
/// live here, not inside the git worktree.
pub fn workspace_dir(root: &Path, workspace_id: &str) -> io::Result<PathBuf> {
    if workspace_id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid argument",
        ));
    }
    let dir = root.join("workspaces").join(workspace_id);
    mkdir_all(&dir)?;
    Ok(dir)
}

fn workspace_subdir(root: &Path, workspace_id: &str, name: &str) -> io::Result<PathBuf> {
    let dir = workspace_dir(root, workspace_id)?.join(name);
    mkdir_all(&dir)?;
    Ok(dir)
}

/// `workspaces/<id>/artifacts/`, created if needed.
pub fn artifacts_dir(root: &Path, workspace_id: &str) -> io::Result<PathBuf> {
    workspace_subdir(root, workspace_id, "artifacts")
}

/// `workspaces/<id>/attachments/` (chat pastes), created if needed.
pub fn attachments_dir(root: &Path, workspace_id: &str) -> io::Result<PathBuf> {
    workspace_subdir(root, workspace_id, "attachments")
}

/// `workspaces/<id>/screenshots/` (QA evidence), created if needed.
pub fn screenshots_dir(root: &Path, workspace_id: &str) -> io::Result<PathBuf> {
    workspace_subdir(root, workspace_id, "screenshots")
}

/// The provider cwd for the app-scoped Ask Recur coordinator. Not a git
/// worktree: the chat is read-only on Recur state and must not inherit a
/// project checkout.
pub fn app_chat_dir() -> io::Result<PathBuf> {
    let dir = data_dir()?.join("app-chat");
    mkdir_all(&dir)?;
    Ok(dir)
}

/// The seeded Recur-owned skills root (`~/.recur/recur-skills`), created if
/// needed. Shipped skills are synced here on boot.
pub fn recur_skills_dir() -> io::Result<PathBuf> {
    let dir = home()?.join(".recur").join("recur-skills");
    mkdir_all(&dir)?;
    Ok(dir)
}

/// The app log directory `<root>/logs/`, created if needed.
pub fn logs_dir(root: &Path) -> io::Result<PathBuf> {
    let dir = root.join("logs");
    mkdir_all(&dir)?;
    Ok(dir)
}

/// The default (read-only shipped) bundles root `<data>/Bundles/`.
pub fn bundles_dir() -> io::Result<PathBuf> {
    let dir = data_dir()?.join("Bundles");
    mkdir_all(&dir)?;
    Ok(dir)
}

/// The user-writable bundles root (`~/.recur/bundles`). User bundles
/// override defaults by command uuid.
pub fn user_bundles_dir() -> io::Result<PathBuf> {
    let dir = home()?.join(".recur").join("bundles");
    mkdir_all(&dir)?;
    Ok(dir)
}

/// The compiled TypeScript/JS macro cache `<data>/BundleCache/`.
pub fn bundles_cache_dir() -> io::Result<PathBuf> {
    let dir = data_dir()?.join("BundleCache");
    mkdir_all(&dir)?;
    Ok(dir)
}

/// `~/.codex/auth.json`, where `codex login` stores credentials; "" when the
/// home directory is unknown. Here because the settings seed is its first user;
/// the Codex provider crate should call this one rather than redefine it.
pub fn codex_default_seed_auth_path() -> String {
    match user_home_dir() {
        Ok(home) => recur_project::home::join(&home, ".codex/auth.json"),
        Err(_) => String::new(),
    }
}
