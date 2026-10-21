use std::path::Path;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::*;

/// Serves HTTP on $RECUR_PORT without `python3 -m http.server`'s getfqdn
/// reverse lookup, which stalls for 30s+ on macOS CI runners.
const DEV_SERVER_CMD: &str = r#"python3 -c 'import http.server as h, socketserver as s, sys; s.TCPServer.allow_reuse_address = True; s.TCPServer(("", int(sys.argv[1])), h.SimpleHTTPRequestHandler).serve_forever()' $RECUR_PORT"#;

fn has_python() -> bool {
    std::process::Command::new("python3")
        .arg("-c")
        .arg("")
        .status()
        .is_ok_and(|s| s.success())
}

async fn answers(url: &str, timeout: Duration) -> bool {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
        .unwrap();
    probe(&client, url).await.is_some()
}

async fn wait_freed(url: &str) -> bool {
    for _ in 0..40 {
        if !answers(url, Duration::from_millis(500)).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn opts(port: i64, probe: Duration) -> BootOptions {
    BootOptions {
        port,
        probe_timeout: probe,
        poll_interval: Duration::from_millis(150),
        ..Default::default()
    }
}

/// Boots a long-lived dev server, confirms it serves on $RECUR_PORT while
/// up, then tears the whole group down via stop (twice) and confirms the
/// port is freed.
#[tokio::test]
async fn boot_happy_path_and_teardown() {
    if !has_python() {
        eprintln!("python3 not available; skipping");
        return;
    }
    let cancel = CancellationToken::new();
    let booted = boot(
        &cancel,
        DEV_SERVER_CMD,
        "http://localhost:$RECUR_PORT",
        opts(0, Duration::from_secs(15)),
    )
    .await
    .unwrap();
    assert!(!booted.port.is_empty());
    assert!(answers(&booted.app_url, Duration::from_secs(2)).await);

    tokio::time::timeout(Duration::from_secs(10), async {
        booted.stop().await;
        booted.stop().await;
    })
    .await
    .expect("stop hung");
    assert!(wait_freed(&booted.app_url).await, "dev server leaked");
}

/// A successful boot returns a server owned by stop, not by the token
/// passed to boot: cancelling afterwards must not kill it.
#[tokio::test]
async fn boot_survives_cancel_until_stop() {
    if !has_python() {
        return;
    }
    let cancel = CancellationToken::new();
    let booted = boot(
        &cancel,
        DEV_SERVER_CMD,
        "http://localhost:$RECUR_PORT",
        opts(0, Duration::from_secs(15)),
    )
    .await
    .unwrap();
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        answers(&booted.app_url, Duration::from_secs(2)).await,
        "server died after cancel"
    );
    booted.stop().await;
    assert!(wait_freed(&booted.app_url).await, "dev server leaked");
}

#[tokio::test]
async fn boot_uses_provided_port() {
    if !has_python() {
        return;
    }
    let port = free_port().unwrap();
    let booted = boot(
        &CancellationToken::new(),
        DEV_SERVER_CMD,
        "http://localhost:$RECUR_PORT",
        opts(i64::from(port), Duration::from_secs(15)),
    )
    .await
    .unwrap();
    assert_eq!(booted.port, port.to_string());
    booted.stop().await;
}

/// A dev command that never opens a port fails within the probe budget and
/// tears down.
#[tokio::test]
async fn boot_fails_fast_when_never_serves() {
    let start = std::time::Instant::now();
    let err = boot(
        &CancellationToken::new(),
        "sleep 30",
        "http://localhost:$RECUR_PORT",
        opts(0, Duration::from_secs(2)),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, BootError::NoResponse { .. }), "{err}");
    assert!(err.to_string().ends_with("within 2s"), "{err}");
    assert!(start.elapsed() < Duration::from_secs(8));
}

#[tokio::test]
async fn boot_reports_early_exit() {
    let err = boot(
        &CancellationToken::new(),
        "exit 3",
        "",
        opts(0, Duration::from_secs(5)),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "dev server exited before serving: exit status 3"
    );
    let err = boot(
        &CancellationToken::new(),
        "true",
        "",
        opts(0, Duration::from_secs(5)),
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "dev server exited before serving");
}

#[tokio::test]
async fn boot_cancel_during_probe() {
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c.cancel();
    });
    let err = boot(&cancel, "sleep 30", "", opts(0, Duration::from_secs(15)))
        .await
        .unwrap_err();
    assert!(matches!(err, BootError::Canceled), "{err}");
}

#[tokio::test]
async fn boot_empty_dev_cmd() {
    let err = boot(&CancellationToken::new(), "  ", "", BootOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "no dev_cmd configured");
}

#[tokio::test]
async fn run_cmd_fails_on_non_zero() {
    let dir = tempfile::tempdir().unwrap();
    let err = run_cmd(
        &CancellationToken::new(),
        None,
        "exit 3",
        &dir.path().to_string_lossy(),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err.exit_code(), Some(3));
    assert_eq!(err.to_string(), "exit status 3");
}

fn kill_pid_file(path: &Path) {
    if let Ok(b) = std::fs::read_to_string(path)
        && let Ok(pid) = b.trim().parse::<i32>()
        && pid > 0
    {
        let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
    }
}

async fn run_in(dir: &Path, script: &str) -> (String, Result<(), RunError>) {
    run_cmd_output(
        &CancellationToken::new(),
        None,
        script,
        &dir.to_string_lossy(),
        None,
    )
    .await
}

#[tokio::test]
async fn run_cmd_output_captures_stdout_and_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let (tail, err) = run_in(dir.path(), "echo out; echo boom >&2; exit 23").await;
    let err = err.unwrap_err();
    assert!(matches!(err, RunError::Exit(_)), "{err}");
    assert_eq!(err.exit_code(), Some(23));
    assert!(tail.contains("out") && tail.contains("boom"), "{tail:?}");
}

#[tokio::test]
async fn run_cmd_output_caps_tail() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = r#"echo HEADMARK; i=0; while [ "$i" -lt 400 ]; do printf '%s\n' 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'; i=$((i+1)); done; echo TAILMARK"#;
    let (tail, err) = run_in(dir.path(), cmd).await;
    err.unwrap();
    assert!(!tail.contains("HEADMARK"));
    assert!(tail.contains("TAILMARK"));
    assert!(tail.len() <= CMD_OUTPUT_TAIL_CAP);
}

#[tokio::test]
async fn run_cmd_output_returns_while_background_child_holds_pipes() {
    let dir = tempfile::tempdir().unwrap();
    let pid_path = dir.path().join("pid");
    let start = std::time::Instant::now();
    let script = format!(
        "sleep 60 & echo $! >{}; echo boom >&2; exit 23",
        pid_path.display()
    );
    let (tail, err) = run_in(dir.path(), &script).await;
    kill_pid_file(&pid_path);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "hung on background pipes"
    );
    assert_eq!(err.unwrap_err().exit_code(), Some(23));
    assert!(tail.contains("boom"), "{tail:?}");
}

#[tokio::test]
async fn run_cmd_output_success_with_background_child() {
    let dir = tempfile::tempdir().unwrap();
    let pid_path = dir.path().join("pid");
    let start = std::time::Instant::now();
    let script = format!("sleep 60 & echo $! >{}; echo ok", pid_path.display());
    let (tail, err) = run_in(dir.path(), &script).await;
    kill_pid_file(&pid_path);
    assert!(start.elapsed() < Duration::from_secs(5));
    err.unwrap();
    assert!(tail.contains("ok"), "{tail:?}");
}

/// The child sleeps past the 100ms wait delay, then writes to its inherited
/// stdout. Closing that pipe would SIGPIPE echo (status 141); a live drain
/// keeps it 0.
#[tokio::test]
async fn run_cmd_output_background_child_write_does_not_sigpipe() {
    let dir = tempfile::tempdir().unwrap();
    let pid_path = dir.path().join("pid");
    let status_path = dir.path().join("status");
    let script = format!(
        "(sleep 0.3; echo child-log; echo $? >{:?}) & echo $! >{:?}; echo ok",
        status_path.display().to_string(),
        pid_path.display().to_string()
    );
    let start = std::time::Instant::now();
    let (tail, err) = run_in(dir.path(), &script).await;
    assert!(start.elapsed() < Duration::from_secs(5));
    err.unwrap();
    assert!(tail.contains("ok"), "{tail:?}");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut status = String::new();
    while std::time::Instant::now() < deadline {
        if let Ok(b) = std::fs::read_to_string(&status_path) {
            status = b.trim().to_owned();
            if !status.is_empty() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    kill_pid_file(&pid_path);
    assert_eq!(status, "0", "SIGPIPE is 141");
}

#[tokio::test]
async fn run_cmd_output_timeout_returns_deadline_exceeded() {
    let dir = tempfile::tempdir().unwrap();
    let deadline = Instant::now() + Duration::from_millis(200);
    let (tail, err) = run_cmd_output(
        &CancellationToken::new(),
        Some(deadline),
        "echo before; sleep 30",
        &dir.path().to_string_lossy(),
        None,
    )
    .await;
    let err = err.unwrap_err();
    assert!(matches!(err, RunError::DeadlineExceeded), "{err}");
    assert!(!err.to_string().contains("signal: killed"));
    assert!(tail.contains("before"), "{tail:?}");
}

#[tokio::test]
async fn run_cmd_output_cancel_returns_canceled() {
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c.cancel();
    });
    let (_, err) = run_cmd_output(&cancel, None, "sleep 30", "", None).await;
    assert!(matches!(err.unwrap_err(), RunError::Canceled));
    let (_, err) = run_cmd_output(&cancel, None, "true", "", None).await;
    assert!(matches!(err.unwrap_err(), RunError::Canceled));
}

#[tokio::test]
async fn run_cmd_output_uses_given_env_and_dir() {
    let dir = tempfile::tempdir().unwrap();
    let env = vec![
        "FOO=bar".to_owned(),
        "PATH=/usr/bin:/bin".to_owned(),
        "FOO=baz".to_owned(),
    ];
    let (tail, err) = run_cmd_output(
        &CancellationToken::new(),
        None,
        "echo \"$FOO $HOME\"; pwd",
        &dir.path().to_string_lossy(),
        Some(&env),
    )
    .await;
    err.unwrap();
    let real = std::fs::canonicalize(dir.path()).unwrap();
    assert!(tail.starts_with("baz \n"), "{tail:?}");
    assert!(tail.ends_with(&*real.to_string_lossy()), "{tail:?}");
}

#[tokio::test]
async fn run_cmd_output_bad_dir_is_start_error() {
    let (_, err) = run_cmd_output(
        &CancellationToken::new(),
        None,
        "true",
        "/no/such/dir",
        None,
    )
    .await;
    assert!(matches!(err.unwrap_err(), RunError::Start(_)));
}

#[test]
fn substitute_port_replaces_all_forms() {
    assert_eq!(
        substitute_port(
            "http://h:${RECUR_PORT}/$RECUR_PORT/${PORT}/$PORT/$PORTX",
            "9"
        ),
        "http://h:9/9/9/9/9X"
    );
    assert_eq!(substitute_port("é$RECUR_PORTé", "1"), "é1é");
    assert_eq!(substitute_port("", "1"), "");
}

#[test]
fn tail_writer_keeps_last_bytes() {
    let mut w = TailWriter {
        buf: Vec::new(),
        max: 4,
    };
    w.write(b"ab");
    w.write(b"cde");
    assert_eq!(w.buf, b"bcde");
    w.write(b"123456");
    assert_eq!(w.buf, b"3456");
    let mut z = TailWriter {
        buf: Vec::new(),
        max: 0,
    };
    z.write(b"x");
    assert!(z.buf.is_empty());
}

#[test]
fn go_duration_formats() {
    for (d, want) in [
        (Duration::ZERO, "0s"),
        (Duration::from_nanos(5), "5ns"),
        (Duration::from_nanos(1_500), "1.5µs"),
        (Duration::from_millis(300), "300ms"),
        (Duration::from_micros(1_500), "1.5ms"),
        (Duration::from_secs(2), "2s"),
        (Duration::from_secs(15), "15s"),
        (Duration::from_millis(3_500), "3.5s"),
        (Duration::from_secs(90), "1m30s"),
        (Duration::from_secs(3_600), "1h0m0s"),
        (
            Duration::from_secs(3_661) + Duration::from_millis(500),
            "1h1m1.5s",
        ),
    ] {
        assert_eq!(go_duration(d), want, "{d:?}");
    }
}

#[test]
fn exit_status_strings() {
    assert_eq!(
        exit_status_string(ExitStatus::from_raw(3 << 8)),
        "exit status 3"
    );
    assert_eq!(
        exit_status_string(ExitStatus::from_raw(9)),
        "signal: killed"
    );
    assert_eq!(
        exit_status_string(ExitStatus::from_raw(15)),
        "signal: terminated"
    );
    assert_eq!(
        exit_status_string(ExitStatus::from_raw(64)),
        "signal: signal 64"
    );
}
