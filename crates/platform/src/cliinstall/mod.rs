//! Places the bundled CLI on PATH when the desktop app launches. The binary
//! lives inside the .app (`Contents/MacOS/recur-cli`); only a symlink is
//! written, so an app update is picked up without recopying bytes.

mod path;
mod shell;

pub use path::ensure_path;

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::identity::{APP_DESCRIPTION, APP_NAME, CLI_LINK_NAME, LINUX_APP_ID, LINUX_PACKAGED_DIR};
use crate::pathutil;

/// The CLI inside a macOS app bundle, relative to the bundle root. Named
/// `recur-cli` so it does not collide with the GUI at `Contents/MacOS/recur`.
/// Apple wants helper executables here (or `Contents/Helpers/`), not under
/// Resources.
pub const REL_BUNDLED_CLI: &str = "Contents/MacOS/recur-cli";

const BUNDLED_CLI_NAME: &str = "recur-cli";

#[derive(Debug, thiserror::Error)]
pub enum CliInstallError {
    #[error("cliinstall: empty dest or target")]
    Empty,
    #[error("cliinstall: {0} exists and is not a Recur symlink")]
    NotSymlink(PathBuf),
    #[error("cliinstall: {dest} points at {current} (not Recur)")]
    Foreign { dest: PathBuf, current: PathBuf },
    #[error("cliinstall: no home directory")]
    NoHome,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The CLI next to a GUI executable at `…/Something.app/Contents/MacOS/<name>`
/// (macOS) or in a flat package directory beside `recur-cli` (Linux). `None`
/// when `exe` is not in a bundle or the CLI file is missing.
pub fn bundled_cli(exe: &Path) -> Option<PathBuf> {
    let exe = exe.to_string_lossy();
    let exe = exe.trim();
    if exe.is_empty() {
        return None;
    }
    let exe = Path::new(exe);
    if !cfg!(target_os = "macos") {
        let cli = exe
            .parent()
            .unwrap_or(Path::new("."))
            .join(BUNDLED_CLI_NAME);
        return cli.is_file().then_some(cli);
    }
    // …/App.app/Contents/MacOS/recur → …/App.app
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" {
        return None;
    }
    if !bundle.to_string_lossy().ends_with(".app") {
        return None;
    }
    let cli = bundle.join(REL_BUNDLED_CLI);
    cli.is_file().then_some(cli)
}

/// A `*.dev.app` or a cargo `target/` dir in a workspace checkout: local
/// builds must not steal the user's production symlink.
pub fn is_dev_bundle(exe: &Path) -> bool {
    if exe.to_string_lossy().contains(".dev.app/") {
        return true;
    }
    exe.ancestors().skip(1).any(|dir| {
        dir.file_name().is_some_and(|n| n == "target") && dir.with_file_name("Cargo.toml").exists()
    })
}

/// Whether `p` looks like a Recur-shipped CLI, so only our own stale symlinks
/// are retargeted and Homebrew / `cargo install` binaries are never clobbered.
pub fn is_bundled_cli_path(p: &Path) -> bool {
    if !cfg!(target_os = "macos") && p.file_name().is_some_and(|n| n == BUNDLED_CLI_NAME) {
        return true;
    }
    let p = p.to_string_lossy();
    if p.contains(&format!(".app/{REL_BUNDLED_CLI}")) {
        return true;
    }
    // Stale launch-time links from the first Resources/bin layout.
    p.contains(&format!(".app/Contents/Resources/bin/{CLI_LINK_NAME}"))
}

/// `~/.local/bin`: writable without sudo. Not on stock macOS PATH;
/// `/usr/local/bin` is (via path_helper) and is tried first when writable.
pub fn user_bin_dir() -> Option<PathBuf> {
    crate::home_dir().map(|h| h.join(".local").join("bin"))
}

/// The usual "install shell command" location (VS Code, Sublime).
pub const SYSTEM_BIN_DIR: &str = "/usr/local/bin";

/// PATH locations to link. `/usr/local/bin` comes first when writable; the
/// user bin dir is always listed and created on demand.
pub fn destinations() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if dir_writable(Path::new(SYSTEM_BIN_DIR)) {
        out.push(Path::new(SYSTEM_BIN_DIR).join(CLI_LINK_NAME));
    }
    if let Some(user) = user_bin_dir() {
        out.push(user.join(CLI_LINK_NAME));
    }
    out
}

fn dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    let probe = dir.join(format!(".recur-cli-write-{}-{nanos}", std::process::id()));
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    if created {
        let _ = std::fs::remove_file(&probe);
    }
    created
}

/// Makes `dest` a symlink to `target`. No-op when it already points there.
/// Refuses to replace a regular file or a symlink that is not ours.
pub fn ensure_link(dest: &Path, target: &Path) -> Result<(), CliInstallError> {
    if dest.as_os_str().is_empty() || target.as_os_str().is_empty() {
        return Err(CliInstallError::Empty);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let want = pathutil::abs(target)?;

    match std::fs::symlink_metadata(dest) {
        Ok(st) => {
            if !st.file_type().is_symlink() {
                return Err(CliInstallError::NotSymlink(dest.to_path_buf()));
            }
            let mut current = std::fs::read_link(dest)?;
            if !current.is_absolute() {
                current = dest.parent().unwrap_or(Path::new("")).join(current);
            }
            if let Ok(current_abs) = pathutil::abs(&current) {
                if same_file_path(&current_abs, &want) {
                    return Ok(());
                }
                if !is_bundled_cli_path(&current) && !is_bundled_cli_path(&current_abs) {
                    return Err(CliInstallError::Foreign {
                        dest: dest.to_path_buf(),
                        current,
                    });
                }
            } else if !is_bundled_cli_path(&current) {
                return Err(CliInstallError::Foreign {
                    dest: dest.to_path_buf(),
                    current,
                });
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(&want, &tmp)?;
    if let Err(err) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }
    Ok(())
}

fn same_file_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(fa), Ok(fb)) => fa.dev() == fb.dev() && fa.ino() == fb.ino(),
        _ => false,
    }
}

/// Finds the CLI next to the running GUI exe and symlinks it onto PATH.
/// Missing bundle, dev build, missing CLI or a package install is a no-op.
/// On Linux it also writes the app-menu entry.
pub fn install_from_app(exe: &Path) -> Result<(), CliInstallError> {
    install_to(exe, &destinations(), desktop_data_home().as_deref())
}

/// Production .app only: symlink the bundled CLI onto PATH (D-cli-install).
/// Run off the UI thread; failures are logged.
pub fn install_bundled_cli() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Err(err) = install_from_app(&exe) {
        tracing::warn!("cli install: {err}");
    }
}

fn install_to(
    exe: &Path,
    dests: &[PathBuf],
    data_home: Option<&Path>,
) -> Result<(), CliInstallError> {
    if exe.as_os_str().is_empty()
        || is_dev_bundle(exe)
        || exe.parent() == Some(Path::new(LINUX_PACKAGED_DIR))
    {
        return Ok(());
    }
    let Some(cli) = bundled_cli(exe) else {
        return Ok(());
    };
    let mut last = None;
    let mut linked = 0;
    for dest in dests {
        match ensure_link(dest, &cli) {
            Ok(()) => linked += 1,
            Err(err) => last = Some(err),
        }
    }
    if linked == 0
        && let Some(err) = last
    {
        return Err(err);
    }
    if cfg!(target_os = "linux") {
        return ensure_desktop_entry(data_home.ok_or(CliInstallError::NoHome)?, exe);
    }
    Ok(())
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
fn desktop_data_home() -> Option<PathBuf> {
    match std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => crate::home_dir().map(|h| h.join(".local").join("share")),
    }
}

/// Matches the Linux application id so Wayland docks pair the window with
/// its icon.
fn desktop_entry_name() -> String {
    format!("{LINUX_APP_ID}.desktop")
}

/// Writes the app-menu launcher for a Linux package directory: `Exec` is
/// `exe`, `Icon` the `recur.png` shipped beside it.
fn ensure_desktop_entry(data_home: &Path, exe: &Path) -> Result<(), CliInstallError> {
    let dir = data_home.join("applications");
    let icon = exe.parent().unwrap_or(Path::new("")).join("recur.png");
    let body = format!(
        "[Desktop Entry]\nType=Application\nName={APP_NAME}\nComment={APP_DESCRIPTION}\n\
         Exec=\"{}\"\nIcon={}\nCategories=Development;\nTerminal=false\n",
        quote_exec(&exe.to_string_lossy()),
        icon.display()
    );
    let dest = dir.join(desktop_entry_name());
    if std::fs::read(&dest).is_ok_and(|old| old == body.as_bytes()) {
        return Ok(());
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&dest, body)?;
    Ok(())
}

/// Desktop Entry `Exec` quoting inside double quotes: the spec's quoting
/// level doubles backslashes again, and `%` field codes are escaped.
fn quote_exec(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\\\"),
            '"' => out.push_str(r#"\""#),
            '`' => out.push_str(r"\`"),
            '$' => out.push_str(r"\$"),
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_exec(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        recur_exec::write_executable(path, body, 0o755).expect("write");
    }

    #[test]
    fn bundled_cli_detection() {
        assert_eq!(bundled_cli(Path::new("")), None);
        assert_eq!(bundled_cli(Path::new("/usr/local/bin/recur")), None);

        let root = tempfile::tempdir().expect("tempdir");
        let exe = root.path().join("Recur.app/Contents/MacOS/recur");
        let cli = root.path().join("Recur.app").join(REL_BUNDLED_CLI);
        write_exec(&exe, "gui");
        assert_eq!(bundled_cli(&exe), None, "before the CLI file exists");
        write_exec(&cli, "cli");
        assert_eq!(bundled_cli(&exe), Some(cli));
    }

    #[test]
    fn dev_bundle_detection() {
        assert!(is_dev_bundle(Path::new(
            "/tmp/bin/recur.dev.app/Contents/MacOS/recur"
        )));
        assert!(!is_dev_bundle(Path::new(
            "/Applications/Recur.app/Contents/MacOS/recur"
        )));

        let ws = tempfile::tempdir().expect("tempdir");
        std::fs::write(ws.path().join("Cargo.toml"), "[workspace]").expect("write");
        assert!(is_dev_bundle(&ws.path().join("target/debug/recur")));
        assert!(is_dev_bundle(&ws.path().join(
            "target/release/bundle/osx/Recur.app/Contents/MacOS/recur"
        )));
    }

    #[test]
    fn bundled_cli_paths() {
        assert!(is_bundled_cli_path(Path::new(
            "/Applications/Recur.app/Contents/MacOS/recur-cli"
        )));
        assert!(is_bundled_cli_path(Path::new(&format!(
            "/Applications/Recur.app/Contents/Resources/bin/{CLI_LINK_NAME}"
        ))));
        assert!(!is_bundled_cli_path(Path::new(
            "/Applications/Recur.app/Contents/MacOS/recur"
        )));
        assert!(!is_bundled_cli_path(Path::new("/usr/local/bin/recur")));
        assert!(!is_bundled_cli_path(Path::new("/opt/homebrew/bin/recur")));
    }

    #[test]
    fn ensure_link_create_and_update() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("Recur.app").join(REL_BUNDLED_CLI);
        write_exec(&bundle, "v1");
        let dest = dir.path().join("bin").join(CLI_LINK_NAME);

        ensure_link(&dest, &bundle).expect("create");
        assert_eq!(std::fs::read_link(&dest).expect("readlink"), bundle);
        ensure_link(&dest, &bundle).expect("idempotent");

        let bundle2 = dir.path().join("Recur2.app").join(REL_BUNDLED_CLI);
        write_exec(&bundle2, "v2");
        ensure_link(&dest, &bundle2).expect("retarget");
        assert_eq!(std::fs::read_link(&dest).expect("readlink"), bundle2);
    }

    #[test]
    fn ensure_link_refuses_foreign() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join(CLI_LINK_NAME);
        write_exec(&dest, "go-install");
        let target = dir.path().join("Recur.app").join(REL_BUNDLED_CLI);
        write_exec(&target, "cli");
        let err = ensure_link(&dest, &target).expect_err("regular file");
        assert!(err.to_string().contains("not a Recur symlink"), "{err}");

        std::fs::remove_file(&dest).expect("rm");
        std::os::unix::fs::symlink("/opt/homebrew/bin/other", &dest).expect("symlink");
        ensure_link(&dest, &target).expect_err("foreign symlink");
    }

    #[test]
    fn install_from_app_links_and_skips() {
        let root = tempfile::tempdir().expect("tempdir");
        let exe = root.path().join("Recur.app/Contents/MacOS/recur");
        let cli = root.path().join("Recur.app").join(REL_BUNDLED_CLI);
        write_exec(&exe, "gui");
        write_exec(&cli, "cli");
        let link = root.path().join(".local/bin").join(CLI_LINK_NAME);
        let data = root.path().join(".local/share");

        install_to(&exe, std::slice::from_ref(&link), Some(&data)).expect("install");
        assert_eq!(std::fs::read_link(&link).expect("readlink"), cli);

        let dev = PathBuf::from(exe.to_string_lossy().replace("Recur.app", "recur.dev.app"));
        install_to(&dev, std::slice::from_ref(&link), Some(&data)).expect("dev");
        install_to(
            Path::new("/tmp/not-a-bundle/recur"),
            std::slice::from_ref(&link),
            Some(&data),
        )
        .expect("unbundled");
        install_to(
            &Path::new(LINUX_PACKAGED_DIR).join("recur"),
            std::slice::from_ref(&link),
            Some(&data),
        )
        .expect("packaged");
    }

    #[test]
    fn desktop_entry() {
        let data = tempfile::tempdir().expect("tempdir");
        let exe = Path::new("/opt/my apps/recur-linux-amd64/recur");
        ensure_desktop_entry(data.path(), exe).expect("entry");
        let path = data.path().join("applications").join(desktop_entry_name());
        let body = std::fs::read_to_string(&path).expect("read");
        for want in [
            "Exec=\"/opt/my apps/recur-linux-amd64/recur\"\n",
            "Icon=/opt/my apps/recur-linux-amd64/recur.png\n",
            &format!("Name={APP_NAME}\n"),
        ] {
            assert!(
                body.contains(want),
                "desktop entry missing {want:?}:\n{body}"
            );
        }
        let mtime = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .expect("mtime");
        std::thread::sleep(std::time::Duration::from_millis(20));
        ensure_desktop_entry(data.path(), exe).expect("unchanged");
        assert_eq!(
            std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .expect("mtime"),
            mtime,
            "unchanged entry is not rewritten"
        );
    }

    #[test]
    fn exec_quoting() {
        assert_eq!(quote_exec(r#"/a b/"$x`%\y"#), r#"/a b/\"\$x\`%%\\\\y"#);
    }
}
