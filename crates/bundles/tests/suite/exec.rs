//! Port of `exec_test.go` (the env-mutating case lives in `tests/env_serial`).

use std::collections::BTreeMap;
use std::time::Duration;

use recur_bundles::*;

use crate::common::*;

fn env_get(env: &[EnvVar], key: &str) -> Option<String> {
    env_lookup(env, key).map(|v| v.to_string_lossy().into_owned())
}

#[tokio::test]
async fn run_env_stdin_stdout_and_input() {
    let (_g, work) = temp_dir();
    let cmd = Command {
        id: "echo-env".into(),
        name: "Echo".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        output: OUTPUT_REPLACE_SELECTION.into(),
        input: INPUT_SELECTION.into(),
        body: "#!/bin/sh\nprintf 'in=%s\\n' \"$(cat)\"\nprintf 'clip=%s\\n' \"$RECUR_CLIPBOARD\"\nprintf 'width=%s\\n' \"$RECUR_INPUT_WIDTH\"\nprintf 'scope=%s\\n' \"$RECUR_SCOPE\"\n".into(),
        ..Default::default()
    };
    let rc = RunContext {
        selected_text: "hello".into(),
        clipboard: "paste".into(),
        scope: "source.go".into(),
        prompt_values: BTreeMap::from([("width".to_string(), "72".to_string())]),
        work_dir: work,
        ..Default::default()
    };
    let res = run(&RunCtx::background(), &cmd, rc).await.unwrap();
    assert_eq!(res.exit_code, 0, "stderr={}", res.stderr);
    assert_eq!(
        res.stdout,
        "in=hello\nclip=paste\nwidth=72\nscope=source.go\n"
    );
    assert_eq!(res.output, OUTPUT_REPLACE_SELECTION);
}

#[tokio::test]
async fn run_non_zero_exit_captured() {
    let (_g, work) = temp_dir();
    let cmd = Command {
        id: "fail".into(),
        name: "Fail".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\necho oops >&2\nexit 3\n".into(),
        ..Default::default()
    };
    let res = run(
        &RunCtx::background(),
        &cmd,
        RunContext {
            work_dir: work,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(res.exit_code, 3);
    assert!(res.stderr.contains("oops"));
}

#[tokio::test]
async fn run_timeout_and_cancel() {
    let (_g, work) = temp_dir();
    let cmd = Command {
        id: "sleep".into(),
        name: "Sleep".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\necho started\nsleep 30\n".into(),
        ..Default::default()
    };
    // Long enough for a freshly written script to start under load.
    let ctx = RunCtx::background().with_timeout(Duration::from_secs(2));
    let started = std::time::Instant::now();
    let err = run(
        &ctx,
        &cmd,
        RunContext {
            work_dir: work.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "group was not killed"
    );
    assert_eq!(
        err.message,
        "command cancelled or timed out: context deadline exceeded"
    );
    assert_eq!(err.partial.stdout, "started\n");
    assert_eq!(err.partial.exit_code, -1);

    let cancel = RunCtx::background();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        c2.cancel();
    });
    let err = run(
        &cancel,
        &cmd,
        RunContext {
            work_dir: work,
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.message,
        "command cancelled or timed out: context canceled"
    );
}

#[tokio::test]
async fn run_rejects_typescript_and_missing_shebang() {
    let ts = Command {
        id: "t".into(),
        interpreter: INTERPRETER_TYPESCRIPT.into(),
        ..Default::default()
    };
    let err = run(&RunCtx::background(), &ts, RunContext::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        "typescript command \"t\" must run in the frontend"
    );
    let sh = Command {
        id: "s".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "echo hi".into(),
        script_path: "/x/s.sh".into(),
        ..Default::default()
    };
    let err = run(&RunCtx::background(), &sh, RunContext::default())
        .await
        .unwrap_err();
    assert_eq!(err.message, "command \"s\" script missing #! shebang");
}

#[tokio::test]
async fn run_inline_body_without_shebang_uses_bash() {
    let (_g, work) = temp_dir();
    let cmd = Command {
        id: "bash".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "echo \"${BASH_VERSION:+bash}\"".into(),
        ..Default::default()
    };
    let res = run(
        &RunCtx::background(),
        &cmd,
        RunContext {
            work_dir: work,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(res.stdout, "bash\n");
}

#[test]
fn environ_bundle_settings() {
    let env = environ(&RunContext {
        bundle_settings: BTreeMap::from([("api_token".to_string(), "secret".to_string())]),
        soft_tabs: true,
        tab_size: 4,
        file_path: "/a/b/c.go".into(),
        ..Default::default()
    });
    assert_eq!(
        env_get(&env, "RECUR_SETTING_API_TOKEN").as_deref(),
        Some("secret")
    );
    assert_eq!(
        env_get(&env, "RECUR_BUNDLE_SETTINGS").as_deref(),
        Some(r#"{"api_token":"secret"}"#)
    );
    assert_eq!(env_get(&env, "RECUR_SOFT_TABS").as_deref(), Some("1"));
    assert_eq!(env_get(&env, "RECUR_FILENAME").as_deref(), Some("c.go"));
    assert_eq!(env_get(&env, "RECUR_DIRECTORY").as_deref(), Some("/a/b"));
}
