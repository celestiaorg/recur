//! Runs a test body in a child process of the test binary with extra
//! environment, so tests that need a different HOME never mutate this
//! process's environment (`set_var` is unsafe in edition 2024).

use std::ffi::OsStr;
use std::process::Command;

const MARK: &str = "RECUR_TEST_CHILD";

/// Call from `#[test] fn name` as `run(module_path!(), "name", envs, || …)`.
/// In the parent this re-runs exactly that test with `envs` set and asserts
/// it passed; in the child it runs `body`.
pub(crate) fn run<K: AsRef<OsStr>, V: AsRef<OsStr>>(
    module: &str,
    name: &str,
    envs: &[(K, V)],
    body: impl FnOnce(),
) {
    let test = match module.split_once("::") {
        Some((_, rest)) => format!("{rest}::{name}"),
        None => name.to_owned(),
    };
    if std::env::var(MARK).as_deref() == Ok(test.as_str()) {
        body();
        return;
    }
    let exe = std::env::current_exe().expect("test binary path");
    let mut cmd = Command::new(exe);
    cmd.args([test.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(MARK, &test);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn isolated test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "isolated {test} failed:\n{stdout}\n{stderr}"
    );
}
