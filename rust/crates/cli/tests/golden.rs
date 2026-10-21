//! Golden parity with the Go CLI: builds `cmd/recur` with `go build` into
//! a temp dir and runs both binaries over the same invocations from the
//! repo root, comparing stdout, stderr and exit code byte for byte.
//! Skipped when Go is not available.

use std::path::{Path, PathBuf};
use std::process::Command;

struct Outcome {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

fn repo_root() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    std::fs::canonicalize(p).expect("repo root")
}

fn go_cli(dir: &Path) -> Option<PathBuf> {
    let bin = dir.join("recur-go");
    let out = Command::new("go")
        .args(["build", "-o"])
        .arg(&bin)
        .arg("./cmd/recur")
        .current_dir(repo_root())
        .output();
    match out {
        Ok(o) if o.status.success() => Some(bin),
        Ok(o) => {
            eprintln!(
                "skipped: go build ./cmd/recur failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            None
        }
        Err(e) => {
            eprintln!("skipped: go not available ({e})");
            None
        }
    }
}

fn run(bin: &Path, args: &[&str], env: &[(&str, &str)], cwd: &Path) -> Outcome {
    let out = Command::new(bin)
        .args(args)
        .envs(env.iter().copied())
        .env_remove("FORCE_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env("NO_COLOR", "1")
        .current_dir(cwd)
        .output()
        .expect("run cli");
    Outcome {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code(),
    }
}

fn compare(
    go: &Path,
    rust: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    cwd: &Path,
    failures: &mut Vec<String>,
) {
    let a = run(go, args, env, cwd);
    let b = run(rust, args, env, cwd);
    if a.code != b.code || a.stdout != b.stdout || a.stderr != b.stderr {
        failures.push(format!(
            "recur {}\n  go:   code={:?}\n{}{}\n  rust: code={:?}\n{}{}",
            args.join(" "),
            a.code,
            a.stdout,
            a.stderr,
            b.code,
            b.stdout,
            b.stderr
        ));
    }
}

#[test]
fn rust_cli_matches_go_cli() {
    let tmp = tempfile::tempdir().expect("tmp");
    let Some(go) = go_cli(tmp.path()) else {
        return;
    };
    let rust = PathBuf::from(env!("CARGO_BIN_EXE_recur-cli"));
    let root = repo_root();
    let data = tmp.path().join("data");
    let data_s = data.to_string_lossy().into_owned();
    let env = [("RECUR_DATA_DIR", data_s.as_str())];
    let mut failures = Vec::new();

    let invocations: &[&[&str]] = &[
        &["--help"],
        &["-h"],
        &["help"],
        &["help", "help"],
        &["help", "bundles", "create"],
        &["help", "bogus"],
        &["bundles"],
        &["bundles", "bogus"],
        &["bundles", "--help"],
        &["bundles", "test", "--help"],
        &["bundles", "lint", "-h"],
        &["bundles", "create", "--help"],
        &["open", "--help"],
        &["completion"],
        &["completion", "--help"],
        &["completion", "bash", "--help"],
        &["completion", "zsh", "--help"],
        &["completion", "fish", "--help"],
        &["completion", "powershell", "--help"],
        &["completion", "bash"],
        &["completion", "zsh"],
        &["completion", "fish"],
        &["completion", "powershell"],
        &["completion", "bash", "--no-descriptions"],
        &["completion", "zsh", "--no-descriptions"],
        &["completion", "fish", "--no-descriptions"],
        &["completion", "powershell", "--no-descriptions"],
        &["completion", "bash", "extra"],
        &["--version"],
        &["-v"],
        &["-h", "bundles"],
        &["--help", "bundles"],
        &["--", "--help"],
        &["bundlez"],
        &["--bogus"],
        &["-x"],
        &["-xh"],
        &["--bundles-dir"],
        &["--=x"],
        &["./a", "./b"],
        &["open", "a", "b"],
        &["open", "/definitely/not/here"],
        &["bundles", "create"],
        &["bundles", "create", "bad name", "--dir", "/tmp"],
        &["bundles", "create", "X", "--force=maybe"],
        &["bundles", "create", "--dir"],
        &["bundles", "test", "--bogus"],
        &["bundles", "test", "nonexistent"],
        &["bundles", "lint", "nonexistent"],
        &["bundles", "test", "./internal"],
        &["__complete"],
        &["__complete", ""],
        &["__completeNoDesc", ""],
        &["__complete", "b"],
        &["__complete", "bundles", ""],
        &["__complete", "bundles", "test", "--"],
        &["__complete", "bundles", "test", "-"],
        &["__complete", "bundles", "create", "--dir", ""],
        &["__complete", "bundles", "create", "--force", ""],
        &["__complete", "--bundles-dir", ""],
        &["__complete", "--bundles-dir=x", "bundles", ""],
        &["__complete", "help", ""],
        &["__complete", "help", "b"],
        &["__complete", "help", "bundles", ""],
        &["__complete", "completion", ""],
        &["__complete", "completion", "bash", "--"],
        &["__complete", "open", ""],
        &["__complete", "bogus", "x", ""],
        &["__complete", "--help", ""],
        &["__complete", "--nope", ""],
        &["bundles", "lint", "./bundles"],
        &["bundles", "lint", "bundles/Text"],
        &["bundles", "lint", "Text", "Source", "./bundles/Math"],
        &["bundles", "test", "./bundles"],
        &["bundles", "test", "Text"],
    ];
    for args in invocations {
        compare(&go, &rust, args, &env, &root, &mut failures);
    }

    // create → test → lint on a scaffolded pack, in one parent per CLI.
    for (name, args) in [
        (
            "create",
            vec!["bundles", "create", "DemoPack", "--dir", "{P}"],
        ),
        (
            "create again",
            vec!["bundles", "create", "DemoPack", "--dir", "{P}"],
        ),
        (
            "create force",
            vec!["bundles", "create", "DemoPack", "--dir", "{P}", "--force"],
        ),
        (
            "test by name",
            vec!["bundles", "test", "DemoPack", "--bundles-dir", "{P}"],
        ),
        ("lint by path", vec!["bundles", "lint", "{P}/DemoPack"]),
    ] {
        let pa = tmp.path().join("go-packs");
        let pb = tmp.path().join("rust-packs");
        std::fs::create_dir_all(&pa).unwrap();
        std::fs::create_dir_all(&pb).unwrap();
        let sub = |p: &Path| -> Vec<String> {
            args.iter()
                .map(|a| a.replace("{P}", &p.to_string_lossy()))
                .collect()
        };
        let (aa, ab) = (sub(&pa), sub(&pb));
        let a = run(
            &go,
            &aa.iter().map(String::as_str).collect::<Vec<_>>(),
            &env,
            &root,
        );
        let b = run(
            &rust,
            &ab.iter().map(String::as_str).collect::<Vec<_>>(),
            &env,
            &root,
        );
        let norm = |s: &str, p: &Path| s.replace(&*p.to_string_lossy(), "{P}");
        if a.code != b.code
            || norm(&a.stdout, &pa) != norm(&b.stdout, &pb)
            || norm(&a.stderr, &pa) != norm(&b.stderr, &pb)
        {
            failures.push(format!(
                "{name}\n  go:   {:?} {}{}\n  rust: {:?} {}{}",
                a.code, a.stdout, a.stderr, b.code, b.stdout, b.stderr
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
