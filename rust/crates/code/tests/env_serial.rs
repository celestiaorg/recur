//! The `rg` cases that change the process environment (`$RECUR_RG`). They run
//! in their own test binary, serialized by [`ENV_LOCK`].

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Mutex;

use recur_code::rg::{self, BIN_NAME, ENV_PATH, RELEASE_VERSION, VERSION_FILE_NAME};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    old: Option<std::ffi::OsString>,
}

#[allow(unsafe_code)]
fn set_env(key: &'static str, value: &str) -> EnvGuard {
    let old = std::env::var_os(key);
    // SAFETY: every test in this binary holds ENV_LOCK while it runs.
    unsafe { std::env::set_var(key, value) };
    EnvGuard { key, old }
}

impl Drop for EnvGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: still under ENV_LOCK (guards drop before the lock).
        unsafe {
            match &self.old {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn script(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "#!/bin/sh\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn locate_env_override() {
    let _l = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake-rg");
    script(&fake);
    let _e = set_env(ENV_PATH, fake.to_str().unwrap());
    assert_eq!(rg::locate(Path::new("")).unwrap(), fake);
}

#[test]
fn locate_lists_what_it_tried() {
    let _l = ENV_LOCK.lock().unwrap();
    let data = tempfile::tempdir().unwrap();
    let _e = set_env(ENV_PATH, "/nonexistent/rg");
    let _p = set_env("PATH", "/nonexistent-bin");
    let err = rg::locate(data.path()).unwrap_err().to_string();
    assert_eq!(
        err,
        format!(
            "ripgrep (rg) not found (looked in: /nonexistent/rg, {}/bin/rg, PATH) — set RECUR_RG or install into <dataDir>/bin/rg",
            data.path().display()
        )
    );
}

#[tokio::test]
async fn ensure_reuses_current_managed_without_network() {
    let _l = ENV_LOCK.lock().unwrap();
    let _e = set_env(ENV_PATH, "");
    let data = tempfile::tempdir().unwrap();
    let managed = data.path().join("bin").join(BIN_NAME);
    script(&managed);
    fs::write(
        data.path().join("bin").join(VERSION_FILE_NAME),
        format!("{RELEASE_VERSION}\n"),
    )
    .unwrap();
    assert_eq!(rg::ensure(data.path()).await.unwrap(), managed);
}

#[tokio::test]
async fn ensure_prefers_env_over_stale_managed() {
    let _l = ENV_LOCK.lock().unwrap();
    let data = tempfile::tempdir().unwrap();
    script(&data.path().join("bin").join(BIN_NAME));
    // A stale sidecar would trigger a reinstall if ensure reached it.
    let sidecar = data.path().join("bin").join(VERSION_FILE_NAME);
    fs::write(&sidecar, "0.0.1\n").unwrap();
    let other = tempfile::tempdir().unwrap();
    let over = other.path().join("override-rg");
    script(&over);
    let _e = set_env(ENV_PATH, over.to_str().unwrap());
    assert_eq!(rg::ensure(data.path()).await.unwrap(), over);
    assert_eq!(fs::read_to_string(&sidecar).unwrap(), "0.0.1\n");
}

#[tokio::test]
async fn ensure_without_data_dir_needs_path() {
    let _l = ENV_LOCK.lock().unwrap();
    let _e = set_env(ENV_PATH, "");
    let _p = set_env("PATH", "/nonexistent-bin");
    assert_eq!(
        rg::ensure(Path::new("")).await.unwrap_err().to_string(),
        "rg ensure: no data directory and ripgrep not on PATH"
    );
}
