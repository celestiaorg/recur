use std::fs;
use std::io::Read;
use std::process::Stdio;
use std::time::Duration;

use recur_domain::path;
use recur_domain::{Check, TestResult};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::gostr::truncate_fields;
use crate::rg::look_path;

/// The test, typecheck and lint commands available in `root`, as check rows.
/// Missing toolchains are skipped, not failed. `cancel` stops every command;
/// without one the run is bounded by 90 s.
pub async fn run_checks(
    cancel: Option<CancellationToken>,
    root: &str,
    test_result: Option<&TestResult>,
) -> Vec<Check> {
    let ctx = Ctx::new(cancel, Duration::from_secs(90));
    let mut checks = agent_test_check(test_result);
    let has = |name: &str| has_file(root, name);
    if has("go.mod") {
        checks.push(
            ctx.run(
                root,
                "Tests",
                "go",
                &["test", "./...", "-count=1", "-timeout=45s"],
            )
            .await,
        );
        checks.push(
            ctx.run(root, "Typecheck (go)", "go", &["vet", "./..."])
                .await,
        );
    }
    if has("package.json") {
        if has_script(root, "test") {
            checks.push(
                ctx.run(root, "Tests (npm)", "npm", &["test", "--", "--runInBand"])
                    .await,
            );
        }
        if has_node_bin(root, "tsc") || has_dev_dep(root, "typescript") {
            checks.push(
                ctx.run(
                    root,
                    "Typecheck (tsc)",
                    "npx",
                    &["--no-install", "tsc", "--noEmit"],
                )
                .await,
            );
        }
        if has_script(root, "lint") {
            checks.push(ctx.run(root, "Lint", "npm", &["run", "lint"]).await);
        }
    }
    if has("Cargo.toml") {
        checks.push(
            ctx.run(root, "Typecheck (cargo)", "cargo", &["check", "--quiet"])
                .await,
        );
        checks.push(
            ctx.run(
                root,
                "Tests (cargo)",
                "cargo",
                &["test", "--quiet", "--", "--test-threads=1"],
            )
            .await,
        );
    }
    if has("pyproject.toml") || has("setup.py") || has("requirements.txt") {
        if has_binary("pytest") {
            checks.push(
                ctx.run(root, "Tests (pytest)", "pytest", &["-q", "--tb=no"])
                    .await,
            );
        } else if has_binary("python") || has_binary("python3") {
            let bin = if has_binary("python3") {
                "python3"
            } else {
                "python"
            };
            checks.push(
                ctx.run(
                    root,
                    "Compile (python)",
                    bin,
                    &["-m", "compileall", "-q", "."],
                )
                .await,
            );
        }
    }
    if has("Gemfile") && has_binary("bundle") {
        checks.push(
            ctx.run(
                root,
                "Tests (rspec)",
                "bundle",
                &["exec", "rspec", "--format", "progress"],
            )
            .await,
        );
    }
    if (has("pom.xml") || has("build.gradle") || has("build.gradle.kts"))
        && has_binary("mvn")
        && has("pom.xml")
    {
        checks.push(
            ctx.run(
                root,
                "Compile (maven)",
                "mvn",
                &["-q", "-DskipTests", "compile"],
            )
            .await,
        );
    }
    if has("Package.swift") {
        checks.push(ctx.run(root, "Build (swift)", "swift", &["build"]).await);
    }
    checks
}

/// PR-open checks: the agent's test result plus a quick typecheck, never a
/// full suite (so the loop cannot hang opening a PR). Without `cancel` the
/// run is bounded by 25 s.
pub async fn run_light_checks(
    cancel: Option<CancellationToken>,
    root: &str,
    test_result: Option<&TestResult>,
) -> Vec<Check> {
    let ctx = Ctx::new(cancel, Duration::from_secs(25));
    let mut checks = agent_test_check(test_result);
    if has_file(root, "go.mod") {
        checks.push(
            ctx.run(root, "Typecheck (go)", "go", &["vet", "./..."])
                .await,
        );
    }
    if has_file(root, "Cargo.toml") {
        checks.push(
            ctx.run(root, "Typecheck (cargo)", "cargo", &["check", "--quiet"])
                .await,
        );
    }
    if has_file(root, "package.json")
        && (has_node_bin(root, "tsc") || has_dev_dep(root, "typescript"))
    {
        checks.push(
            ctx.run(
                root,
                "Typecheck (tsc)",
                "npx",
                &["--no-install", "tsc", "--noEmit"],
            )
            .await,
        );
    }
    checks
}

/// The run's cancellation: the caller's token, or one that fires after the
/// default bound.
pub(super) struct Ctx {
    cancel: CancellationToken,
}

impl Ctx {
    pub(super) fn new(cancel: Option<CancellationToken>, bound: Duration) -> Self {
        let cancel = cancel.unwrap_or_else(|| {
            let token = CancellationToken::new();
            let fire = token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(bound).await;
                fire.cancel();
            });
            token
        });
        Self { cancel }
    }

    /// Runs one command with a 50 s cap and turns its outcome into a check.
    pub(super) async fn run(&self, root: &str, name: &str, bin: &str, args: &[&str]) -> Check {
        let (out, err) = run_combined(&self.cancel, root, bin, args).await;
        let out = out.trim();
        let check = |detail: String, passed: bool| Check {
            name: name.into(),
            detail,
            passed,
            ..Default::default()
        };
        if let Some(err) = err {
            // A missing binary skips rather than failing the PR.
            if is_missing_binary(&err, out) {
                return check("skipped (tool unavailable)".into(), true);
            }
            let detail = match first_line(out) {
                "" => err,
                l => l.to_string(),
            };
            return check(truncate_fields(&detail, 140), false);
        }
        let n = count_pass(out);
        check(
            if n > 0 {
                format!("{n} passed")
            } else {
                "ok".into()
            },
            true,
        )
    }
}

/// Runs `bin args` in `root` with stdout and stderr in one buffer. The error
/// is the exit text (`exit status 1`, `signal: killed`, …).
async fn run_combined(
    cancel: &CancellationToken,
    root: &str,
    bin: &str,
    args: &[&str],
) -> (String, Option<String>) {
    let Some(program) = (if bin.contains('/') {
        Some(bin.to_string())
    } else {
        look_path(bin)
    }) else {
        return (
            String::new(),
            Some(format!(
                "exec: {:?}: executable file not found in $PATH",
                bin
            )),
        );
    };
    let (mut reader, writer) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return (String::new(), Some(e.to_string())),
    };
    let stderr = match writer.try_clone() {
        Ok(w) => w,
        Err(e) => return (String::new(), Some(e.to_string())),
    };
    let spawned = Command::new(&program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            return (String::new(), Some(format!("{program}: {e}")));
        }
    };
    let collect = tokio::task::spawn_blocking(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let status = tokio::select! {
        s = child.wait() => s,
        () = cancel.cancelled() => {
            let _ = child.kill().await;
            child.wait().await
        }
        () = tokio::time::sleep(Duration::from_secs(50)) => {
            let _ = child.kill().await;
            child.wait().await
        }
    };
    let out = collect.await.unwrap_or_default();
    let err = match status {
        Ok(s) if s.success() => None,
        Ok(s) => Some(exit_text(s)),
        Err(e) => Some(e.to_string()),
    };
    (out, err)
}

fn exit_text(s: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (s.code(), s.signal()) {
        (Some(code), _) => format!("exit status {code}"),
        (None, Some(9)) => "signal: killed".into(),
        (None, Some(sig)) => format!("signal: {}", signal_name(sig)),
        (None, None) => s.to_string(),
    }
}

/// Signal names for the signals a check can die of.
fn signal_name(sig: i32) -> String {
    match sig {
        1 => "hangup".into(),
        2 => "interrupt".into(),
        3 => "quit".into(),
        6 => "abort trap".into(),
        11 => "segmentation fault".into(),
        13 => "broken pipe".into(),
        15 => "terminated".into(),
        n => format!("signal {n}"),
    }
}

fn agent_test_check(test_result: Option<&TestResult>) -> Vec<Check> {
    let Some(tr) = test_result else {
        return Vec::new();
    };
    let detail = match tr.summary.trim() {
        "" if tr.passed => "Tester reported pass",
        "" => "Tester reported fail",
        s => s,
    };
    vec![Check {
        name: "Tests (agent)".into(),
        detail: truncate_fields(detail, 120),
        passed: tr.passed,
        ..Default::default()
    }]
}

fn is_missing_binary(err: &str, out: &str) -> bool {
    let msg = format!("{err}{out}");
    msg.contains("executable file not found") || (msg.contains("not found") && msg.contains("npx"))
}

fn has_file(root: &str, name: &str) -> bool {
    fs::metadata(path::join(&[root, name])).is_ok()
}

fn has_binary(name: &str) -> bool {
    look_path(name).is_some()
}

/// Whether package.json mentions `"name"` anywhere (for both scripts and dev
/// dependencies).
fn has_script(root: &str, name: &str) -> bool {
    fs::read_to_string(path::join(&[root, "package.json"]))
        .is_ok_and(|b| b.contains(&format!("\"{name}\"")))
}

fn has_dev_dep(root: &str, name: &str) -> bool {
    has_script(root, name)
}

fn has_node_bin(root: &str, name: &str) -> bool {
    fs::metadata(path::join(&[root, "node_modules", ".bin", name])).is_ok()
}

fn first_line(s: &str) -> &str {
    let s = s.trim();
    s.split_once('\n').map_or(s, |(l, _)| l.trim())
}

fn count_pass(out: &str) -> usize {
    out.split('\n')
        .filter(|l| l.starts_with("ok\t") || l.contains(" PASS "))
        .count()
}
