//! Helper mode: `Updater::restart` re-executes this binary with sentinel env
//! vars. The helper waits for the parent to exit, swaps the installed app
//! with the staged artifact, relaunches it and exits. Env vars, not flags, so
//! user arguments are never confused with helper plumbing.

use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::UpdaterError;

pub(super) const ENV_MODE: &str = "RECUR_UPDATER_HELPER";
pub(super) const ENV_TARGET: &str = "RECUR_UPDATER_HELPER_TARGET";
pub(super) const ENV_NEW: &str = "RECUR_UPDATER_HELPER_NEW";
pub(super) const ENV_PID: &str = "RECUR_UPDATER_HELPER_PID";
pub(super) const ENV_LOG: &str = "RECUR_UPDATER_HELPER_LOG";
pub(super) const ENV_READY: &str = "RECUR_UPDATER_HELPER_READY";

pub(super) const HELPER_ENV: [&str; 6] =
    [ENV_MODE, ENV_TARGET, ENV_NEW, ENV_PID, ENV_LOG, ENV_READY];

/// Returns immediately unless this process was spawned as the updater
/// helper; in helper mode it performs the swap, relaunches and exits.
///
/// Call first thing in `main`: the parent still holds the instance lock, so
/// the helper must divert before the single-instance claim hands it off.
pub fn handle_helper_mode() {
    if std::env::var(ENV_MODE).ok().as_deref() != Some("1") {
        return;
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let (target, new_path, ready) = (env(ENV_TARGET), env(ENV_NEW), env(ENV_READY));
    let ready = Path::new(&ready);
    if target.is_empty() || new_path.is_empty() {
        let _ = signal_status(ready, "error: missing helper target or staged artifact");
        std::process::exit(2);
    }
    if let Err(e) = std::fs::metadata(&target) {
        let _ = signal_status(ready, &format!("error: target is unavailable: {e}"));
        std::process::exit(10);
    }
    if let Err(e) = std::fs::metadata(&new_path) {
        let _ = signal_status(
            ready,
            &format!("error: staged artifact is unavailable: {e}"),
        );
        std::process::exit(11);
    }
    if signal_status(ready, "ready").is_err() {
        std::process::exit(3);
    }
    let pid = env(ENV_PID).parse().unwrap_or(0);
    let code = run_helper_swap(
        Path::new(&target),
        Path::new(&new_path),
        pid,
        Path::new(&env(ENV_LOG)),
        &wait_for_pid,
        &OsLauncher,
    );
    std::process::exit(code);
}

pub(super) fn signal_status(path: &Path, status: &str) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(status.as_bytes())
}

/// Polls the readiness file the helper writes once it reaches helper mode.
pub(super) async fn wait_for_helper_ready(
    path: &Path,
    timeout: Duration,
) -> Result<(), UpdaterError> {
    let deadline = Instant::now() + timeout;
    loop {
        match std::fs::read_to_string(path) {
            Ok(payload) => {
                let status = payload.trim();
                if status == "ready" {
                    return Ok(());
                }
                if let Some(detail) = status.strip_prefix("error:") {
                    return Err(UpdaterError::HelperNotReady(detail.trim().to_string()));
                }
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                return Err(UpdaterError::HelperNotReady(format!(
                    "read readiness signal: {e}"
                )));
            }
            Err(_) => {}
        }
        if Instant::now() > deadline {
            return Err(UpdaterError::HelperNotReady(String::new()));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The helper invocation: stdio detached and a new session, so it survives
/// the parent's exit and is not signalled with it.
pub(super) fn detached_command(path: &Path) -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[allow(unsafe_code)]
    // SAFETY: setsid is async-signal-safe and touches no parent memory.
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(io::Error::from)
        });
    }
    cmd
}

pub(super) type Waiter = dyn Fn(u32, Duration) -> Result<(), String>;

/// Starts the replaced app.
pub(super) trait Launcher {
    fn launch(&self, path: &Path) -> io::Result<()>;
}

pub(super) struct OsLauncher;

impl Launcher for OsLauncher {
    fn launch(&self, path: &Path) -> io::Result<()> {
        launch_command(path).spawn().map(drop)
    }
}

/// `open -n` for a macOS bundle, else the binary itself; detached and with
/// the helper env scrubbed so the relaunched app (or a recovered original)
/// boots normally instead of re-entering helper mode.
pub(super) fn launch_command(path: &Path) -> Command {
    let mut cmd = if cfg!(target_os = "macos") && path.extension().is_some_and(|e| e == "app") {
        let mut c = Command::new("open");
        c.arg("-n").arg(path);
        c
    } else {
        Command::new(path)
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for k in HELPER_ENV {
        cmd.env_remove(k);
    }
    cmd
}

/// The swap. Returns the helper's exit code:
/// 0 ok, 10 target missing, 11 artifact missing, 12 backup failed (original
/// relaunched), 13 swap failed (backup restored), 14 restore failed, 15
/// launch failed (backup restored), 16 restore after launch failure failed,
/// 17 parent never exited.
pub(super) fn run_helper_swap(
    target: &Path,
    new_path: &Path,
    parent_pid: u32,
    log_path: &Path,
    wait: &Waiter,
    launcher: &dyn Launcher,
) -> i32 {
    let mut lg = HelperLog::open(log_path);
    lg.log(format!(
        "helper start: target={} new={} pid={parent_pid}",
        target.display(),
        new_path.display()
    ));
    if let Err(e) = std::fs::metadata(target) {
        lg.log(format!("stat target failed: {e}"));
        return 10;
    }
    if let Err(e) = std::fs::metadata(new_path) {
        lg.log(format!("stat new failed: {e}"));
        return 11;
    }
    // Never proceed while the parent lives: the relaunch would start a
    // second copy racing on shared state.
    if parent_pid > 0
        && let Err(e) = wait(parent_pid, Duration::from_secs(30))
    {
        lg.log(format!(
            "parent did not exit within timeout: {e} — aborting swap"
        ));
        return 17;
    }

    let backup = with_suffix(target, ".bak");
    let _ = remove_all(&backup);
    lg.log(format!(
        "backing up {} → {}",
        target.display(),
        backup.display()
    ));
    if let Err(e) = copy_any(target, &backup) {
        lg.log(format!("backup failed: {e}"));
        // The original is untouched; relaunch it rather than restore a
        // possibly incomplete backup.
        if let Err(e) = launcher.launch(target) {
            lg.log(format!("relaunch original failed: {e}"));
        }
        return 12;
    }

    // The artifact was created without exec bits; restore the original mode.
    let orig_mode = std::fs::metadata(target)
        .map(|m| m.permissions().mode())
        .ok();

    let mut swapped = false;
    for attempt in 1..=20 {
        match replace_target(target, new_path) {
            Ok(()) => {
                swapped = true;
                lg.log(format!("swap succeeded on attempt {attempt}"));
                break;
            }
            Err(e) => {
                lg.log(format!("replace (attempt {attempt}): {e}"));
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    if swapped
        && let Some(mode) = orig_mode.filter(|m| *m != 0)
        && let Err(e) =
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode & 0o777))
    {
        lg.log(format!("chmod restored mode: {e} (non-fatal)"));
    }
    if !swapped {
        lg.log("all swap attempts exhausted — restoring backup".into());
        if let Err(e) = restore_from_backup(&backup, target, launcher) {
            lg.log(format!("restore failed: {e}"));
            return 14;
        }
        return 13;
    }
    if let Err(e) = launcher.launch(target) {
        lg.log(format!("launch new failed: {e} — restoring backup"));
        if let Err(e) = restore_from_backup(&backup, target, launcher) {
            lg.log(format!("restore failed: {e}"));
            return 16;
        }
        return 15;
    }
    if let Err(e) = remove_all(&backup) {
        lg.log(format!("backup cleanup: {e} (non-fatal)"));
    }
    // The download's `recur-update-*` staging dir is empty now; the prefix
    // guard keeps a caller-supplied path from being deleted.
    if let Some(staging) = new_path.parent()
        && staging
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("recur-update-"))
        && let Err(e) = remove_all(staging)
    {
        lg.log(format!("staging cleanup: {e} (non-fatal)"));
    }
    lg.log("helper done".into());
    0
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// `os.RemoveAll`: files, symlinks or trees; a missing path is success.
pub(super) fn remove_all(p: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(p),
        Ok(_) => std::fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Unix can unlink a running binary (open handles keep the inode) and
/// rename the new one into place; bundles are directories.
fn replace_target(target: &Path, new_path: &Path) -> io::Result<()> {
    remove_all(target)?;
    std::fs::rename(new_path, target)
}

fn restore_from_backup(
    backup: &Path,
    target: &Path,
    launcher: &dyn Launcher,
) -> Result<(), String> {
    remove_all(target).map_err(|e| format!("remove broken target: {e}"))?;
    std::fs::rename(backup, target).map_err(|e| format!("restore: {e}"))?;
    launcher.launch(target).map_err(|e| e.to_string())
}

fn copy_any(src: &Path, dst: &Path) -> io::Result<()> {
    let info = std::fs::metadata(src)?;
    if info.is_dir() {
        copy_tree(src, dst)
    } else {
        copy_file(src, dst, info.permissions().mode())
    }
}

fn copy_file(src: &Path, dst: &Path, mode: u32) -> io::Result<()> {
    let mut input = File::open(src)?;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode & 0o7777)
        .open(dst)?;
    io::copy(&mut input, &mut out)?;
    out.sync_all()
}

fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    let info = std::fs::metadata(src)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(info.permissions().mode() & 0o7777)
        .create(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        let meta = std::fs::symlink_metadata(&from)?;
        if meta.file_type().is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
        } else if meta.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            copy_file(&from, &to, meta.permissions().mode())?;
        }
    }
    Ok(())
}

/// Polls until `pid` is gone or `timeout` elapses.
fn wait_for_pid(pid: u32, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "pid {pid} still alive after {}s",
        timeout.as_secs()
    ))
}

/// Signal 0 succeeds only for a live process we may signal.
fn is_alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
}

/// Logs to stderr and, when it can be created, the helper log file.
struct HelperLog {
    file: Option<File>,
}

impl HelperLog {
    fn open(path: &Path) -> Self {
        let path = if path.as_os_str().is_empty() {
            std::env::temp_dir().join(format!("recur-update-{}.log", std::process::id()))
        } else {
            path.to_path_buf()
        };
        Self {
            file: File::create(path).ok(),
        }
    }

    fn log(&mut self, msg: String) {
        let line = format!(
            "{}: {msg}\n",
            chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        );
        let _ = io::stderr().write_all(line.as_bytes());
        if let Some(f) = &mut self.file {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Creates the parent directory chain for test fixtures and the staging dir.
#[cfg(test)]
fn mkdir_p(p: &Path) {
    std::fs::DirBuilder::new()
        .recursive(true)
        .create(p)
        .expect("mkdir");
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    type Inspect = Box<dyn Fn(&Path) + Send + Sync>;

    #[derive(Default)]
    struct FakeLauncher {
        calls: Mutex<Vec<PathBuf>>,
        errs: Vec<bool>,
        inspect: Option<Inspect>,
    }

    impl Launcher for FakeLauncher {
        fn launch(&self, path: &Path) -> io::Result<()> {
            if let Some(f) = &self.inspect {
                f(path);
            }
            let mut calls = self.calls.lock();
            let idx = calls.len();
            calls.push(path.to_path_buf());
            if self.errs.get(idx).copied().unwrap_or(false) {
                return Err(io::Error::other("nope"));
            }
            Ok(())
        }
    }

    fn instant(_: u32, _: Duration) -> Result<(), String> {
        Ok(())
    }

    fn write(path: &Path, body: &[u8]) {
        mkdir_p(path.parent().expect("parent"));
        std::fs::write(path, body).expect("write");
    }

    fn read(path: &Path) -> Vec<u8> {
        std::fs::read(path).expect("read")
    }

    fn pair(dir: &Path) -> (PathBuf, PathBuf) {
        let target = dir.join("app.bin");
        let new_path = dir.join("app.bin.new");
        write(&target, b"OLD");
        write(&new_path, b"NEW");
        (target, new_path)
    }

    #[test]
    fn happy_path_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (target, new_path) = pair(dir.path());
        let l = FakeLauncher::default();
        assert_eq!(
            run_helper_swap(&target, &new_path, 0, &dir.path().join("log"), &instant, &l),
            0
        );
        assert_eq!(read(&target), b"NEW");
        assert!(!new_path.exists());
        assert!(!with_suffix(&target, ".bak").exists(), "backup cleaned up");
        assert_eq!(*l.calls.lock(), [target]);
        assert!(
            std::fs::read_to_string(dir.path().join("log"))
                .expect("log")
                .contains("helper done")
        );
    }

    #[test]
    fn preserves_executable_bit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (target, new_path) = pair(dir.path());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let l = FakeLauncher::default();
        assert_eq!(
            run_helper_swap(&target, &new_path, 0, &dir.path().join("log"), &instant, &l),
            0
        );
        let mode = std::fs::metadata(&target)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755, "exec bit not restored");
    }

    #[test]
    fn missing_target_or_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let new_path = dir.path().join("new");
        write(&new_path, b"NEW");
        let l = FakeLauncher::default();
        let log = dir.path().join("log");
        assert_eq!(
            run_helper_swap(
                &dir.path().join("missing"),
                &new_path,
                0,
                &log,
                &instant,
                &l
            ),
            10
        );
        let target = dir.path().join("app.bin");
        write(&target, b"OLD");
        assert_eq!(
            run_helper_swap(&target, &dir.path().join("nope"), 0, &log, &instant, &l),
            11
        );
    }

    #[test]
    fn launch_failure_restores_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (target, new_path) = pair(dir.path());
        let l = FakeLauncher {
            errs: vec![true],
            ..FakeLauncher::default()
        };
        assert_eq!(
            run_helper_swap(&target, &new_path, 0, &dir.path().join("log"), &instant, &l),
            15
        );
        assert_eq!(read(&target), b"OLD");
        assert_eq!(l.calls.lock().len(), 2, "new launch then restore launch");
    }

    fn bundle(root: &Path, payload: &str) {
        write(&root.join("Contents/MacOS/exe"), payload.as_bytes());
    }

    #[test]
    fn app_bundle_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (target, new_path) = (dir.path().join("App.app"), dir.path().join("App.app.new"));
        bundle(&target, "old-bin");
        bundle(&new_path, "new-bin");
        let l = FakeLauncher::default();
        assert_eq!(
            run_helper_swap(&target, &new_path, 0, &dir.path().join("log"), &instant, &l),
            0
        );
        assert_eq!(read(&target.join("Contents/MacOS/exe")), b"new-bin");
    }

    #[test]
    fn app_bundle_restored_on_launch_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (target, new_path) = (dir.path().join("App.app"), dir.path().join("App.app.new"));
        bundle(&target, "old-bin");
        bundle(&new_path, "new-bin");
        let l = FakeLauncher {
            errs: vec![true],
            ..FakeLauncher::default()
        };
        assert_eq!(
            run_helper_swap(&target, &new_path, 0, &dir.path().join("log"), &instant, &l),
            15
        );
        assert_eq!(read(&target.join("Contents/MacOS/exe")), b"old-bin");
    }

    #[test]
    fn parent_wait_timeout_aborts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (target, new_path) = pair(dir.path());
        let l = FakeLauncher::default();
        let stuck = |_: u32, _: Duration| Err("parent still alive".to_string());
        assert_eq!(
            run_helper_swap(
                &target,
                &new_path,
                1234,
                &dir.path().join("log"),
                &stuck,
                &l
            ),
            17
        );
        assert_eq!(read(&target), b"OLD");
        assert!(l.calls.lock().is_empty());
    }

    #[test]
    fn replace_failure_restores_and_relaunches_original() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (target, new_path) = pair(dir.path());
        let l = FakeLauncher {
            inspect: Some(Box::new({
                let target = target.clone();
                move |p| {
                    assert_eq!(p, target);
                    assert_eq!(
                        std::fs::read(p).expect("read"),
                        b"OLD",
                        "restored application"
                    );
                }
            })),
            ..FakeLauncher::default()
        };
        // Removing the staged file after validation makes every replace fail.
        let wait = {
            let new_path = new_path.clone();
            move |_: u32, _: Duration| std::fs::remove_file(&new_path).map_err(|e| e.to_string())
        };
        assert_eq!(
            run_helper_swap(&target, &new_path, 1234, &dir.path().join("log"), &wait, &l),
            13
        );
        assert_eq!(l.calls.lock().len(), 1);
    }

    #[test]
    fn backup_failure_relaunches_original() {
        for launch_fails in [false, true] {
            let dir = tempfile::tempdir().expect("tempdir");
            // The name fits, but adding .bak exceeds the 255-byte limit.
            let target = dir.path().join("a".repeat(252));
            let new_path = dir.path().join("new.bin");
            write(&target, b"OLD");
            write(&new_path, b"NEW");
            let waited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let wait = {
                let waited = std::sync::Arc::clone(&waited);
                move |_: u32, _: Duration| {
                    waited.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }
            };
            let l = FakeLauncher {
                errs: vec![launch_fails],
                ..FakeLauncher::default()
            };
            assert_eq!(
                run_helper_swap(&target, &new_path, 1234, &dir.path().join("log"), &wait, &l),
                12
            );
            assert!(
                waited.load(std::sync::atomic::Ordering::SeqCst),
                "waits for the parent first"
            );
            assert_eq!(*l.calls.lock(), std::slice::from_ref(&target));
            assert_eq!(read(&target), b"OLD", "original intact");
        }
    }

    #[test]
    fn launch_command_scrubs_helper_env() {
        let cmd = launch_command(Path::new("/opt/app/recur"));
        let removed: Vec<String> = cmd
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        for k in HELPER_ENV {
            assert!(
                removed.iter().any(|r| r == k),
                "{k} leaked to launched process"
            );
        }
        assert_eq!(cmd.get_program(), "/opt/app/recur");
        if cfg!(target_os = "macos") {
            let app = launch_command(Path::new("/Applications/Recur.app"));
            assert_eq!(app.get_program(), "open");
            let args: Vec<_> = app.get_args().collect();
            assert_eq!(args, ["-n", "/Applications/Recur.app"]);
        }
    }

    #[tokio::test]
    async fn readiness_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ready");
        signal_status(&path, "ready").expect("signal");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "ready");
        wait_for_helper_ready(&path, Duration::from_secs(1))
            .await
            .expect("ready");

        signal_status(&path, "error: staged artifact missing").expect("signal");
        let err = wait_for_helper_ready(&path, Duration::from_secs(1))
            .await
            .expect_err("error status");
        assert!(
            matches!(err, UpdaterError::HelperNotReady(ref d) if d == "staged artifact missing"),
            "{err}"
        );

        let err = wait_for_helper_ready(&dir.path().join("never"), Duration::from_millis(50))
            .await
            .expect_err("timeout");
        assert!(matches!(err, UpdaterError::HelperNotReady(_)));
    }

    #[test]
    fn no_env_returns() {
        if std::env::var(ENV_MODE).is_err() {
            handle_helper_mode();
        }
    }

    #[test]
    fn alive_checks() {
        assert!(is_alive(std::process::id()));
        assert!(wait_for_pid(std::process::id(), Duration::from_millis(150)).is_err());
        let mut child = Command::new("true").spawn().expect("spawn");
        let pid = child.id();
        child.wait().expect("wait");
        assert!(wait_for_pid(pid, Duration::from_secs(2)).is_ok());
    }
}
