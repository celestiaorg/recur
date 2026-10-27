//! Go `cursor/bridge_alive_test.go`, `bridge_log_test.go`,
//! `bridge_reap_test.go` and `bridge_shell_test.go`, against a Python fake
//! bridge.

use std::ffi::OsString;
use std::time::Duration;

use super::*;
use crate::testserver::{wait_for, write_fake_bridge};

fn cfg(bin: &Path, log_dir: &Path) -> BridgeConfig {
    BridgeConfig {
        bin: bin.to_path_buf(),
        log_dir: log_dir.to_path_buf(),
        ..Default::default()
    }
}

#[test]
fn alive() {
    let reaped = CancellationToken::new();
    let b = Bridge::with_reaped_for_test(reaped.clone(), false);
    assert!(b.alive(), "an open reaper means the process still runs");
    reaped.cancel();
    assert!(!b.alive(), "a fired reaper means it exited");
    assert!(
        !Bridge::with_reaped_for_test(CancellationToken::new(), true).alive(),
        "closed is not alive"
    );
    assert!(
        !Bridge::bound("http://x", "t", None).alive(),
        "a bound test bridge has no process"
    );
}

#[tokio::test]
async fn stderr_log_and_tail() {
    let dir = tempfile::tempdir().unwrap();
    let bin = write_fake_bridge(dir.path());
    let b = Bridge::start(cfg(&bin, dir.path()), &CancellationToken::new())
        .await
        .unwrap();
    wait_for(Duration::from_secs(2), "ready line in tail", || {
        b.stderr_tail().contains(READY_PREFIX)
    })
    .await;
    let tail = b.stderr_tail();
    assert!(
        !tail.contains("test-token"),
        "auth token leaked into tail: {tail}"
    );
    assert!(tail.contains(r#""authToken":"[redacted]""#), "{tail}");
    assert!(tail.contains(log::BRIDGE_LOG_FILE), "{tail}");
    let body = std::fs::read_to_string(dir.path().join(log::BRIDGE_LOG_FILE)).unwrap();
    assert!(body.contains("starting cursor-sdk-bridge"), "{body}");
    assert!(!body.contains("test-token"), "{body}");
    assert!(body.contains(READY_PREFIX), "{body}");
    assert_eq!(b.discovery().auth_token, "test-token");
    assert!(b.alive());
    b.close().await;
    assert!(!b.alive());
}

#[tokio::test]
async fn start_rejects_bad_inputs() {
    let tok = CancellationToken::new();
    let err = Bridge::start(BridgeConfig::default(), &tok)
        .await
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        "cursor bridge: no bridge binary configured"
    );
    let dir = tempfile::tempdir().unwrap();
    let bin = write_fake_bridge(dir.path());
    let bad_ws = BridgeConfig {
        bin,
        workspace: dir.path().join("missing"),
        ..Default::default()
    };
    let err = Bridge::start(bad_ws, &tok).await.err().unwrap();
    assert!(err.to_string().contains("is unavailable"), "{err}");
}

#[tokio::test]
async fn exits_before_ready() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("cursor-sdk-bridge");
    recur_exec::write_executable(&bin, "#!/bin/sh\necho nope >&2\n", 0o755).unwrap();
    let err = Bridge::start(cfg(&bin, Path::new("")), &CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        "cursor bridge: exited before reporting ready"
    );
}

#[tokio::test]
async fn bad_discovery_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("cursor-sdk-bridge");
    recur_exec::write_executable(
        &bin,
        "#!/bin/sh\necho 'cursor-sdk-bridge ready {\"schemaVersion\":2,\"transport\":\"tcp\",\"protocol\":\"connect\",\"url\":\"x\"}' >&2\nsleep 5\n",
        0o755,
    )
    .unwrap();
    let err = Bridge::start(cfg(&bin, Path::new("")), &CancellationToken::new())
        .await
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        "cursor bridge: unsupported discovery schemaVersion 2 (want 1)"
    );
}

#[test]
fn validate_discovery_cases() {
    let ok = Discovery {
        schema_version: 1,
        transport: "tcp".into(),
        protocol: "connect".into(),
        url: "http://x".into(),
        ..Default::default()
    };
    assert!(validate_discovery(&ok).is_ok());
    let unix = Discovery {
        transport: "unix".into(),
        ..ok.clone()
    };
    assert_eq!(
        validate_discovery(&unix).unwrap_err().to_string(),
        r#"cursor bridge: unsupported transport "unix"/"connect" (want tcp/connect)"#
    );
    let no_url = Discovery {
        url: String::new(),
        ..ok
    };
    assert_eq!(
        validate_discovery(&no_url).unwrap_err().to_string(),
        "cursor bridge: discovery carried no url"
    );
}

/// Launches `bin` from a helper that exits at once, leaving it reparented to
/// pid 1 (the force-quit leftover shape).
async fn start_orphan_bridge(bin: &Path) -> i32 {
    let tmp = tempfile::tempdir().unwrap();
    let helper = tmp.path().join("spawn_orphan.py");
    let pid_file = tmp.path().join("orphan.pid");
    std::fs::write(
        &helper,
        r#"
import os, subprocess, sys
bin, pid_path = sys.argv[1], sys.argv[2]
p = subprocess.Popen(
    [bin],
    start_new_session=True,
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)
with open(pid_path, "w", encoding="utf-8") as f:
    f.write(str(p.pid))
os._exit(0)
"#,
    )
    .unwrap();
    let st = std::process::Command::new("python3")
        .arg(&helper)
        .arg(bin)
        .arg(&pid_file)
        .status()
        .unwrap();
    assert!(st.success());
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_for(Duration::from_secs(3), "orphan reparented", || {
        reap::process_ppid(pid) == 1
    })
    .await;
    pid
}

struct KillGroup(i32);

impl Drop for KillGroup {
    fn drop(&mut self) {
        if let Some(p) = Pid::from_raw(self.0) {
            let _ = kill_process_group(p, Signal::KILL);
            let _ = rustix::process::kill_process(p, Signal::KILL);
        }
    }
}

#[tokio::test]
async fn start_reaps_orphan_bridge_same_bin() {
    let dir = tempfile::tempdir().unwrap();
    let bin = write_fake_bridge(dir.path());
    let orphan = start_orphan_bridge(&bin).await;
    let _cleanup = KillGroup(orphan);

    let log_dir = dir.path().join("logs");
    let b = Bridge::start(cfg(&bin, &log_dir), &CancellationToken::new())
        .await
        .unwrap();
    wait_for(Duration::from_secs(5), "orphan reaped", || {
        !reap::process_alive(orphan)
    })
    .await;
    let log = std::fs::read_to_string(log_dir.join(log::BRIDGE_LOG_FILE)).unwrap();
    assert!(log.contains("reaping stale cursor-sdk-bridge"), "{log}");
    assert!(
        log_dir.join(reap::BRIDGE_PID_FILE).exists(),
        "pidfile missing after start"
    );
    b.close().await;
}

#[tokio::test]
async fn probe_start_does_not_kill_live_sibling() {
    // Settings probes (no log dir) share the launcher with a provider bridge
    // that may be mid-run.
    let dir = tempfile::tempdir().unwrap();
    let bin = write_fake_bridge(dir.path());
    let provider = Bridge::start(
        cfg(&bin, &dir.path().join("logs")),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let probe = Bridge::start(cfg(&bin, Path::new("")), &CancellationToken::new())
        .await
        .unwrap();
    assert!(
        reap::process_alive(provider.pid()),
        "probe start reaped the live provider bridge"
    );
    assert!(provider.alive());
    probe.close().await;
    provider.close().await;
}

#[tokio::test]
async fn start_does_not_reap_live_non_orphan_same_bin() {
    let dir = tempfile::tempdir().unwrap();
    let bin = write_fake_bridge(dir.path());
    let mut live = tokio::process::Command::new(&bin)
        .process_group(0)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let live_pid = live.id().unwrap() as i32;
    let _cleanup = KillGroup(live_pid);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let b = Bridge::start(
        cfg(&bin, &dir.path().join("logs")),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        reap::process_alive(live_pid),
        "non-orphan same-bin bridge was reaped"
    );
    b.close().await;
    let _ = live.kill().await;
}

#[tokio::test]
async fn start_does_not_reap_other_bin() {
    let dir = tempfile::tempdir().unwrap();
    let bin_a = write_fake_bridge(&dir.path().join("a"));
    let bin_b = write_fake_bridge(&dir.path().join("b"));
    let other = start_orphan_bridge(&bin_b).await;
    let _cleanup = KillGroup(other);
    let b = Bridge::start(
        cfg(&bin_a, &dir.path().join("logs")),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        reap::process_alive(other),
        "a bridge for another launcher was reaped"
    );
    b.close().await;
}

#[tokio::test]
async fn start_reaps_orphan_bridge_from_other_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let bin = write_fake_bridge(&dir.path().join("a"));
    let other_bin = write_fake_bridge(&dir.path().join("Recur-gone/cursor-bridge/bin"));
    let orphan = start_orphan_bridge(&other_bin).await;
    let _cleanup = KillGroup(orphan);
    let b = Bridge::start(
        cfg(&bin, &dir.path().join("logs")),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    wait_for(
        Duration::from_secs(5),
        "orphan from another data dir reaped",
        || !reap::process_alive(orphan),
    )
    .await;
    b.close().await;
}

#[tokio::test]
async fn close_clears_pidfile() {
    let dir = tempfile::tempdir().unwrap();
    let bin = write_fake_bridge(dir.path());
    let log_dir = dir.path().join("logs");
    let b = Bridge::start(cfg(&bin, &log_dir), &CancellationToken::new())
        .await
        .unwrap();
    assert!(log_dir.join(reap::BRIDGE_PID_FILE).exists());
    b.close().await;
    assert!(!log_dir.join(reap::BRIDGE_PID_FILE).exists());
    b.close().await;
}

#[tokio::test]
async fn workspace_bridge_creates_project_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let bin = write_fake_bridge(dir.path());
    let b = Bridge::start(
        BridgeConfig {
            bin,
            workspace: ws.path().to_path_buf(),
            home: Some(home.path().to_path_buf()),
            ..Default::default()
        },
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(b.discovery().workspace_ref, ws.path().to_string_lossy());
    let slug = crate::projectdirs::cursor_project_slug(&ws.path().to_string_lossy());
    assert!(
        home.path()
            .join(".cursor/projects")
            .join(slug)
            .join("terminals")
            .is_dir()
    );
    b.close().await;
}

fn envs(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}

fn get(env: &[(OsString, OsString)], key: &str) -> Option<String> {
    env.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.to_string_lossy().into_owned())
}

#[test]
fn shell_env_keeps_good_shell() {
    let got = ensure_bridge_shell_env(envs(&[("PATH", "/usr/bin"), ("SHELL", "/bin/sh")]));
    assert_eq!(get(&got, "SHELL").as_deref(), Some("/bin/sh"));
}

#[test]
fn shell_env_fills_missing() {
    let got = ensure_bridge_shell_env(envs(&[("PATH", "/usr/bin")]));
    assert!(shell_ok(Path::new(&get(&got, "SHELL").unwrap())));
}

#[test]
fn shell_env_replaces_missing_binary() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-shell");
    let got = ensure_bridge_shell_env(envs(&[("SHELL", &missing.to_string_lossy())]));
    let shell = get(&got, "SHELL").unwrap();
    assert_ne!(Path::new(&shell), missing);
    assert!(shell_ok(Path::new(&shell)));
    assert_eq!(got.iter().filter(|(k, _)| k == "SHELL").count(), 1);
}

#[test]
fn bridge_env_drops_linear_key_and_adds_ours() {
    let cfg = BridgeConfig {
        api_key: "sk".into(),
        attribution_config_path: PathBuf::from("/d/cursor-attribution.json"),
        ..Default::default()
    };
    let env = bridge_env(
        &cfg,
        envs(&[
            ("PATH", "/bin"),
            ("LINEAR_API_KEY", "secret"),
            ("FOO", "bar"),
            ("SHELL", "/bin/sh"),
        ])
        .into_iter(),
    );
    assert_eq!(get(&env, "LINEAR_API_KEY"), None);
    assert_eq!(get(&env, "PATH").as_deref(), Some("/bin"));
    assert_eq!(get(&env, "FOO").as_deref(), Some("bar"));
    assert_eq!(
        get(&env, "CURSOR_SDK_CLIENT_LANGUAGE").as_deref(),
        Some("go")
    );
    assert_eq!(get(&env, "CURSOR_API_KEY").as_deref(), Some("sk"));
    assert_eq!(
        get(&env, ENV_ATTRIBUTION_PATH).as_deref(),
        Some("/d/cursor-attribution.json")
    );
}

#[tokio::test]
async fn read_line_capped_matches_scanner() {
    let input: &[u8] = b"a\r\nb\nlast";
    let mut r = BufReader::new(input);
    let mut lines = Vec::new();
    let mut buf = Vec::new();
    while read_line_capped(&mut r, &mut buf, 1 << 20).await.unwrap() {
        lines.push(String::from_utf8(std::mem::take(&mut buf)).unwrap());
    }
    assert_eq!(lines, ["a", "b", "last"]);
    let long = [b'x'; 40];
    let mut r = BufReader::new(&long[..]);
    assert!(read_line_capped(&mut r, &mut Vec::new(), 16).await.is_err());
}
