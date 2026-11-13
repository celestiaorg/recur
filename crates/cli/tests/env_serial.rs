//! The CLI tests that change the process environment
//! (`RECUR_DATA_DIR`, `RECUR_BUNDLES_DIR`, `PATH`), `recur config` among them,
//! serialized by a lock in their own test binary.

use std::sync::Mutex;
use std::time::Duration;

use recur_cli::{Io, execute};
use recur_platform::openipc::{self, Handler, Request, Response};
use tokio_util::sync::CancellationToken;

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
        // SAFETY: guards drop while ENV_LOCK is still held.
        unsafe {
            match &self.old {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A short data dir so `open.sock` stays under the macOS 104-byte limit.
fn short_data_dir() -> (tempfile::TempDir, EnvGuard) {
    let dir = tempfile::Builder::new()
        .prefix("cli")
        .tempdir_in("/tmp")
        .unwrap();
    let guard = set_env("RECUR_DATA_DIR", &dir.path().to_string_lossy());
    (dir, guard)
}

/// Serves the open socket with `handle` on a background runtime.
struct Server {
    stop: CancellationToken,
    rt: tokio::runtime::Runtime,
}

impl Server {
    fn start(sock: std::path::PathBuf, handle: Handler) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let stop = CancellationToken::new();
        let s = stop.clone();
        let path = sock.clone();
        rt.spawn(async move {
            let _ = openipc::serve(s, &path, handle).await;
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !sock.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "socket was not created"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        Server { stop, rt }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
        std::thread::sleep(Duration::from_millis(20));
        let _ = &self.rt;
    }
}

fn sock() -> std::path::PathBuf {
    openipc::sock_path(std::path::Path::new(
        &std::env::var("RECUR_DATA_DIR").unwrap(),
    ))
}

#[test]
fn send_or_launch_uses_socket() {
    let _l = lock();
    let (_d, _g) = short_data_dir();
    let (tx, rx) = std::sync::mpsc::channel::<Request>();
    let tx = Mutex::new(tx);
    let _server = Server::start(
        sock(),
        std::sync::Arc::new(move |req: Request| {
            let _ = tx.lock().unwrap().send(req.clone());
            Response {
                ok: true,
                path: req.path,
                workspace_id: "ws-1".into(),
                ..Default::default()
            }
        }),
    );
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().to_string_lossy().into_owned();
    recur_cli::open::send_or_launch(&dir).unwrap();
    let req = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("server got the request");
    assert_eq!(req.path, dir);
}

#[test]
fn send_or_launch_retries_shutdown() {
    let _l = lock();
    let (_d, _g) = short_data_dir();
    let n = std::sync::atomic::AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<Request>();
    let tx = Mutex::new(tx);
    let _server = Server::start(
        sock(),
        std::sync::Arc::new(move |req: Request| {
            if n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 < 3 {
                return Response::shutting_down();
            }
            let _ = tx.lock().unwrap().send(req.clone());
            Response {
                ok: true,
                path: req.path,
                ..Default::default()
            }
        }),
    );
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().to_string_lossy().into_owned();
    recur_cli::open::send_or_launch(&dir).unwrap();
    let req = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("request after shutdown retries");
    assert_eq!(req.path, dir);
}

#[test]
fn send_or_launch_permanent_error() {
    let _l = lock();
    let (_d, _g) = short_data_dir();
    let _server = Server::start(
        sock(),
        std::sync::Arc::new(|_req: Request| Response::failure("not a git repo")),
    );
    let dir = tempfile::tempdir().unwrap();
    let err = recur_cli::open::send_or_launch(&dir.path().to_string_lossy()).unwrap_err();
    assert_eq!(err, "not a git repo");
}

#[test]
fn root_open_arg() {
    let _l = lock();
    let (_d, _g) = short_data_dir();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = Mutex::new(tx);
    let _server = Server::start(
        sock(),
        std::sync::Arc::new(move |req: Request| {
            let _ = tx.lock().unwrap().send(req.path);
            Response {
                ok: true,
                ..Default::default()
            }
        }),
    );
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().to_string_lossy().into_owned();
    let want = recur_cli::open::resolve_open_arg(&dir).unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = execute(
        std::slice::from_ref(&dir),
        &mut Io {
            out: &mut out,
            err: &mut err,
        },
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), want);
}

#[test]
fn look_desktop_binary_rejects_bundled_cli() {
    let _l = lock();
    let dir = tempfile::tempdir().unwrap();
    let bundled = dir.path().join("Recur.app/Contents/MacOS/recur-cli");
    std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
    recur_exec::write_executable(&bundled, "cli", 0o755).unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(&bundled, bin.join("recur")).unwrap();
    let _p = set_env("PATH", &bin.to_string_lossy());
    assert!(recur_cli::open::look_desktop_binary().is_err());
}

#[test]
fn resolve_bundle_name_via_env() {
    let _l = lock();
    let parent = tempfile::tempdir().unwrap();
    let parent_s = parent.path().to_string_lossy().into_owned();
    recur_cli::create_bundle("Named", &parent_s, false).unwrap();
    let _b = set_env("RECUR_BUNDLES_DIR", &parent_s);
    let (_d, _g) = short_data_dir();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = execute(
        &["bundles".into(), "lint".into(), "Named".into()],
        &mut Io {
            out: &mut out,
            err: &mut err,
        },
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    assert_eq!(String::from_utf8_lossy(&out), "ok\tno issues\n");
}

#[test]
fn bundles_dir_flag_beats_env() {
    let _l = lock();
    let flag_dir = tempfile::tempdir().unwrap();
    let env_dir = tempfile::tempdir().unwrap();
    recur_cli::create_bundle("FromFlag", &flag_dir.path().to_string_lossy(), false).unwrap();
    let _b = set_env("RECUR_BUNDLES_DIR", &env_dir.path().to_string_lossy());
    let (_d, _g) = short_data_dir();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = execute(
        &[
            "--bundles-dir".into(),
            flag_dir.path().to_string_lossy().into_owned(),
            "bundles".into(),
            "lint".into(),
            "FromFlag".into(),
        ],
        &mut Io {
            out: &mut out,
            err: &mut err,
        },
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
}

#[test]
fn launch_failure_reports_hint() {
    let _l = lock();
    let (_d, _g) = short_data_dir();
    let _a = set_env("RECUR_APP", "/definitely/missing/recur");
    let dir = tempfile::tempdir().unwrap();
    let err = recur_cli::open::send_or_launch(&dir.path().to_string_lossy()).unwrap_err();
    assert_eq!(
        err,
        "Recur isn't running and could not be launched: /definitely/missing/recur: No such file or directory (os error 2)\nStart Recur (or `mise run dev`) and retry"
    );
}

fn run(args: &[&str]) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = execute(
        &args,
        &mut Io {
            out: &mut out,
            err: &mut err,
        },
    );
    (
        code,
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
    )
}

#[test]
fn config_without_app_edits_store() {
    let _l = lock();
    let (_d, _g) = short_data_dir();
    let (code, out, err) = run(&["config", "list"]);
    assert_eq!(code, 0, "{err}");
    for want in [
        "modelAgent=",
        "maxAgents=",
        "simplify=null",
        "queueFollowUps=false",
    ] {
        assert!(out.lines().any(|l| l.starts_with(want)), "{want} in {out}");
    }
    assert!(!out.contains("cursorApiKey"), "{out}");

    for (key, value, want) in [
        (
            "modelAgent",
            "claude-code/claude-opus-5-5",
            "claude-code/claude-opus-5-5",
        ),
        ("flagsAgent", r#"{"effort":"high"}"#, r#"{"effort":"high"}"#),
        ("bypassPermissions", "true", "true"),
        ("maxAgents", "3", "3"),
    ] {
        let (code, out, err) = run(&["config", "set", key, value]);
        assert_eq!((code, out.as_str()), (0, ""), "set {key}: {err}");
        let (code, out, err) = run(&["config", "get", key]);
        assert_eq!((code, out), (0, format!("{want}\n")), "get {key}: {err}");
    }
    // A fresh process sees the write.
    let st = recur_store::Store::open_without_secrets(&recur_store::paths::db_path(
        std::path::Path::new(&std::env::var("RECUR_DATA_DIR").unwrap()),
    ))
    .unwrap()
    .settings();
    assert_eq!(
        st.flags_agent.get("effort").map(String::as_str),
        Some("high")
    );
    assert!(st.bypass_permissions);
}

#[test]
fn config_rejects_unknown_keys_and_bad_values() {
    let _l = lock();
    let (_d, _g) = short_data_dir();
    let (code, _, err) = run(&["config", "set", "noSuchKey", "1"]);
    assert_eq!((code, err.as_str()), (1, "unknown setting \"noSuchKey\"\n"));
    let (code, _, err) = run(&["config", "get", "noSuchKey"]);
    assert_eq!((code, err.as_str()), (1, "unknown setting \"noSuchKey\"\n"));
    let (code, _, err) = run(&["config", "set", "cursorApiKey", "sk"]);
    assert_eq!(code, 1);
    assert!(err.contains("is a secret"), "{err}");

    let (_, before, _) = run(&["config", "list"]);
    for (key, value) in [("maxAgents", "lots"), ("bypassPermissions", "[1]")] {
        let (code, _, err) = run(&["config", "set", key, value]);
        assert_eq!(code, 1);
        assert!(
            err.starts_with(&format!("invalid value for {key}")),
            "{err}"
        );
    }
    assert_eq!(run(&["config", "list"]).1, before);
}

#[test]
fn config_with_app_running_goes_through_app() {
    let _l = lock();
    let (d, _g) = short_data_dir();
    let _held = openipc::try_lock_data_dir(d.path()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Request>();
    let tx = Mutex::new(tx);
    let _server = Server::start(
        sock(),
        std::sync::Arc::new(move |req: Request| {
            let _ = tx.lock().unwrap().send(req.clone());
            let mut settings = serde_json::Map::new();
            settings.insert("maxAgents".into(), 5.into());
            Response {
                ok: true,
                settings: Some(settings),
                ..Default::default()
            }
        }),
    );
    let (code, out, err) = run(&["config", "set", "maxAgents", "5"]);
    assert_eq!((code, out.as_str()), (0, ""), "{err}");
    let req = rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .settings
        .unwrap();
    assert_eq!((req.key.as_str(), req.value), ("maxAgents", Some(5.into())));
    assert_eq!(run(&["config", "get", "maxAgents"]).1, "5\n");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2))
            .unwrap()
            .settings
            .unwrap()
            .value,
        None
    );
    assert!(
        !d.path().join("recur.db").exists(),
        "the CLI opened the store"
    );
}
