//! Replaces launchd's stunted PATH with the user's login-shell PATH.

use std::path::{Path, PathBuf};

use super::shell;

/// Replaces launchd's PATH with the user's login-shell PATH so lookups find
/// claude, gh, Homebrew, nvm and the rest. An app started from the Dock does
/// not inherit zshrc; a terminal launch does. Common directories are still
/// merged in so a missing `~/.local/bin` in the captured PATH does not hide
/// the native Claude installer.
///
/// Call at the top of `main`, before any thread (tokio runtime, IPC server)
/// starts: it mutates the process environment.
pub fn ensure_path() {
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = compute_path(
        &current,
        &shell::capture_login_path(),
        &extra_path_dirs(crate::home_dir().as_deref(), Path::new("/")),
    );
    if merged != current {
        set_path(&merged);
    }
}

#[allow(unsafe_code)]
fn set_path(value: &str) {
    // SAFETY: `ensure_path` is documented to run at the start of `main`,
    // before any other thread exists that could read the environment.
    unsafe { std::env::set_var("PATH", value) };
}

/// The login PATH (when probed) or the current one, with `extra` merged in.
fn compute_path(current: &str, login: &str, extra: &[PathBuf]) -> String {
    let base = if login.is_empty() { current } else { login };
    merge_path(base, extra)
}

fn extra_path_dirs(home: Option<&Path>, root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home {
        dirs.extend(user_path_dirs(home));
    }
    dirs.extend(SYSTEM_PATH_DIRS.iter().map(PathBuf::from));
    if cfg!(target_os = "macos") {
        dirs.extend(read_path_helper_dirs(root));
    }
    dirs
}

/// Home-relative bin dirs shells usually prepend. Missing directories are
/// skipped by `merge_path`.
fn user_path_dirs(home: &Path) -> Vec<PathBuf> {
    let rel: &[&str] = &[
        ".local/bin",       // Claude Code native, pipx, uv, Recur CLI
        "bin",              // classic ~/bin
        ".cargo/bin",       // rustup
        "go/bin",           // go install (default GOPATH)
        ".volta/bin",       // Volta node
        ".asdf/shims",      // asdf
        ".pyenv/shims",     // pyenv
        ".rbenv/shims",     // rbenv
        ".nix-profile/bin", // Nix user profile
        ".claude/local",    // legacy Claude Code npm local
        ".claude/local/node_modules/.bin",
    ];
    let mut dirs: Vec<PathBuf> = rel.iter().map(|r| home.join(r)).collect();
    if cfg!(target_os = "macos") {
        dirs.push(home.join("Library").join("pnpm"));
    }
    dirs
}

const SYSTEM_PATH_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    super::SYSTEM_BIN_DIR,
    "/usr/local/sbin",
    "/opt/local/bin",
    "/opt/local/sbin",
    "/nix/var/nix/profiles/default/bin",
    "/home/linuxbrew/.linuxbrew/bin",
    "/home/linuxbrew/.linuxbrew/sbin",
];

/// Mirrors `/usr/libexec/path_helper`: `/etc/paths`, then `/etc/paths.d` in
/// lexical order. A fallback when the login shell cannot be probed.
fn read_path_helper_dirs(root: &Path) -> Vec<PathBuf> {
    let etc = root.join("etc");
    let mut dirs = read_path_lines(&etc.join("paths"));
    let Ok(rd) = std::fs::read_dir(etc.join("paths.d")) else {
        return dirs;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| !e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    for name in names {
        dirs.extend(read_path_lines(&etc.join("paths.d").join(name)));
    }
    dirs
}

fn read_path_lines(path: &Path) -> Vec<PathBuf> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .collect()
}

/// Prepends `extra` directories that exist and are not already on `path`,
/// preserving `path`'s order for everything else.
fn merge_path(path: &str, extra: &[PathBuf]) -> String {
    let parts: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split(':').collect()
    };
    let mut seen: std::collections::HashSet<String> = parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect();
    let mut prepend: Vec<String> = Vec::new();
    for d in extra {
        let d = d.to_string_lossy();
        let d = d.trim();
        if d.is_empty() || seen.contains(d) || !Path::new(d).is_dir() {
            continue;
        }
        prepend.push(d.to_string());
        seen.insert(d.to_string());
    }
    if prepend.is_empty() {
        return path.to_string();
    }
    prepend.extend(parts.iter().map(|p| p.to_string()));
    prepend.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn merge_prepends_existing() {
        let extra = tempfile::tempdir().expect("tempdir");
        let missing = extra.path().join("nope");
        let got = merge_path("/usr/bin:/bin", &[extra.path().to_path_buf(), missing]);
        assert_eq!(got, format!("{}:/usr/bin:/bin", s(extra.path())));
    }

    #[test]
    fn merge_skips_already_present() {
        let extra = tempfile::tempdir().expect("tempdir");
        let path = format!("{}:/usr/bin", s(extra.path()));
        assert_eq!(merge_path(&path, &[extra.path().to_path_buf()]), path);
    }

    #[test]
    fn merge_dedups_extra() {
        let extra = tempfile::tempdir().expect("tempdir");
        let e = extra.path().to_path_buf();
        assert_eq!(
            merge_path("/bin", &[e.clone(), e]),
            format!("{}:/bin", s(extra.path()))
        );
    }

    #[test]
    fn merge_empty_base() {
        let extra = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            merge_path("", &[extra.path().to_path_buf()]),
            s(extra.path())
        );
    }

    #[test]
    fn extra_dirs_include_common() {
        let home = tempfile::tempdir().expect("tempdir");
        let h = home.path();
        let got = extra_path_dirs(Some(h), Path::new("/"));
        let want = [
            h.join(".local/bin"),
            h.join("bin"),
            h.join(".cargo/bin"),
            h.join("go/bin"),
            PathBuf::from(crate::cliinstall::SYSTEM_BIN_DIR),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/opt/homebrew/sbin"),
            PathBuf::from("/usr/local/sbin"),
            PathBuf::from("/opt/local/bin"),
        ];
        for dir in &want {
            assert!(
                got.contains(dir),
                "extra_path_dirs() = {got:?}, want {dir:?}"
            );
        }
        let idx = |p: &Path| got.iter().position(|g| g == p).expect("present");
        assert!(idx(&h.join(".local/bin")) < idx(Path::new(crate::cliinstall::SYSTEM_BIN_DIR)));
    }

    #[test]
    fn path_helper_dirs() {
        let root = tempfile::tempdir().expect("tempdir");
        let etc = root.path().join("etc");
        std::fs::create_dir_all(etc.join("paths.d")).expect("mkdir");
        std::fs::write(etc.join("paths"), "/usr/local/bin\n/usr/bin\n# comment\n\n").expect("w");
        std::fs::write(etc.join("paths.d/homebrew"), "/opt/homebrew/bin\n").expect("w");
        std::fs::write(etc.join("paths.d/zzz-extra"), "/opt/extra/bin\n").expect("w");
        std::fs::write(etc.join("paths.d/.hidden"), "/opt/hidden\n").expect("w");
        let got = read_path_helper_dirs(root.path());
        let want: Vec<PathBuf> = [
            "/usr/local/bin",
            "/usr/bin",
            "/opt/homebrew/bin",
            "/opt/extra/bin",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got, want);
    }

    fn fake_home() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().expect("tempdir");
        let local = home.path().join(".local/bin");
        (home, local)
    }

    #[test]
    fn adds_local_bin_before_usr_bin() {
        let (home, local) = fake_home();
        std::fs::create_dir_all(&local).expect("mkdir");
        let got = compute_path(
            "/usr/bin:/bin",
            "",
            &extra_path_dirs(Some(home.path()), Path::new("/")),
        );
        let parts: Vec<&str> = got.split(':').collect();
        let idx = parts
            .iter()
            .position(|p| *p == s(&local))
            .expect("local bin present");
        let usr = parts
            .iter()
            .position(|p| *p == "/usr/bin")
            .expect("usr bin present");
        assert!(idx < usr, "{got}");
    }

    #[test]
    fn merged_path_finds_claude() {
        let (home, local) = fake_home();
        std::fs::create_dir_all(&local).expect("mkdir");
        let bin = local.join("claude");
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").expect("write");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let before = std::process::Command::new("claude")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .status();
        assert!(
            before.is_err() || !before.is_ok_and(|s| s.success()),
            "claude found before merge"
        );
        let merged = compute_path(
            "/usr/bin:/bin",
            "",
            &extra_path_dirs(Some(home.path()), Path::new("/")),
        );
        let status = std::process::Command::new("claude")
            .env("PATH", &merged)
            .status()
            .expect("claude runs from merged PATH");
        assert!(status.success());
    }

    #[test]
    fn skips_missing_local_bin() {
        let (home, local) = fake_home();
        let got = compute_path(
            "/usr/bin:/bin",
            "",
            &extra_path_dirs(Some(home.path()), Path::new("/")),
        );
        assert!(!got.split(':').any(|p| p == s(&local)), "{got}");
    }

    #[test]
    fn uses_login_shell_path() {
        let (home, _) = fake_home();
        let got = compute_path(
            "/usr/bin:/bin",
            "/from/shell:/usr/bin",
            &extra_path_dirs(Some(home.path()), Path::new("/")),
        );
        assert!(got.split(':').any(|p| p == "/from/shell"), "{got}");
    }
}
