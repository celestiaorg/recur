//! Managed paths the CLI needs (Go `internal/store` paths, `git.CanonPath`).
//!
//! recur-store and recur-git have not landed yet; when they do, these
//! become calls into them (the data dir is `<UserConfigDir>/RecurNext`, or
//! `$RECUR_DATA_DIR`, per rust/docs/ARCHITECTURE.md).

use std::path::Path;

use recur_domain::path;

/// Directory name of the Rust app's managed root under `UserConfigDir`.
pub const DATA_DIR_NAME: &str = "RecurNext";

/// Go `os.UserHomeDir` on Unix.
pub fn user_home_dir() -> Result<String, String> {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => Ok(h),
        _ => Err("$HOME is not defined".to_string()),
    }
}

/// Go `os.UserConfigDir` (macOS and Linux).
pub fn user_config_dir() -> Result<String, String> {
    if cfg!(target_os = "macos") {
        return Ok(format!("{}/Library/Application Support", user_home_dir()?));
    }
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(dir) if !dir.is_empty() => {
            if path::is_abs(&dir) {
                Ok(dir)
            } else {
                Err("path in $XDG_CONFIG_HOME is relative".to_string())
            }
        }
        _ => match std::env::var("HOME") {
            Ok(h) if !h.is_empty() => Ok(format!("{h}/.config")),
            _ => Err("neither $XDG_CONFIG_HOME nor $HOME are defined".to_string()),
        },
    }
}

fn mkdir_all(dir: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))
}

/// The managed root (`$RECUR_DATA_DIR` or `<UserConfigDir>/RecurNext`), created.
pub fn data_dir() -> Result<String, String> {
    let root = match std::env::var("RECUR_DATA_DIR") {
        Ok(r) if !r.is_empty() => r,
        _ => path::join(&[&user_config_dir()?, DATA_DIR_NAME]),
    };
    mkdir_all(&root)?;
    Ok(root)
}

/// The open-IPC socket in the data dir.
pub fn open_sock_path() -> Result<String, String> {
    let root = data_dir()?;
    Ok(recur_platform::openipc::sock_path(Path::new(&root))
        .to_string_lossy()
        .into_owned())
}

/// The shipped bundles root (`<DataDir>/Bundles`), created.
pub fn shipped_bundles_dir() -> Result<String, String> {
    let dir = path::join(&[&data_dir()?, "Bundles"]);
    mkdir_all(&dir)?;
    Ok(dir)
}

/// The user bundles root (`~/.recur/bundles`), created.
pub fn user_bundles_dir() -> Result<String, String> {
    let dir = path::join(&[&user_home_dir()?, ".recur", "bundles"]);
    mkdir_all(&dir)?;
    Ok(dir)
}

/// Go `git.ExpandHome`: a leading `~` or `~/` becomes the home dir.
fn expand_home(p: &str) -> String {
    if (p == "~" || p.starts_with("~/"))
        && let Ok(home) = user_home_dir()
    {
        return path::join(&[&home, &p[1..]]);
    }
    p.to_string()
}

/// Go `git.CanonPath`: absolute, cleaned, symlinks resolved when possible.
pub fn canon_path(p: &str) -> Result<String, String> {
    let p = expand_home(p).trim().to_string();
    if p.is_empty() {
        return Err("empty path".to_string());
    }
    let abs = path::abs(&p).map_err(|e| e.to_string())?;
    match std::fs::canonicalize(&abs) {
        Ok(real) => Ok(real.to_string_lossy().into_owned()),
        Err(_) => Ok(abs),
    }
}
