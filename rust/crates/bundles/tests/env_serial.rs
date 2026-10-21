//! The `prompt_test.go` / `exec_test.go` cases that change the process
//! environment (a fake `gh` on PATH, a stale `RECUR_*` var). They run in
//! their own test binary, serialized by [`ENV_LOCK`], so no other test can
//! observe the mutation.

use std::process::Command as Proc;
use std::sync::Mutex;
use std::time::Duration;

use recur_bundles::*;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets an env var for the life of the returned guard. Callers hold
/// [`ENV_LOCK`], so no other thread in this binary reads the environment.
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

fn lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn temp_dir() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().unwrap();
    let s = d.path().to_string_lossy().into_owned();
    (d, s)
}

fn chat_dir() -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bundles/Chat");
    go::clean(&p.to_string_lossy())
}

fn shebang_ctx() -> RunCtx {
    RunCtx::background().with_timeout(Duration::from_secs(60))
}

fn chat_prompt(script: &str, id: &str, name: &str, slash: &str) -> PromptAction {
    let dir = chat_dir();
    PromptAction {
        id: id.into(),
        name: name.into(),
        slash: slash.into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: std::fs::read_to_string(format!("{dir}/src/{script}")).unwrap(),
        bundle_dir: dir,
        ..Default::default()
    }
}

/// Installs a fake `gh` that prints `payload` and exits like `gh pr
/// checks` (1 on failure, 8 while pending). Returns the PATH guard.
fn install_fake_gh_checks(bin: &str, payload: &str) -> EnvGuard {
    let out = format!("{bin}/checks.json");
    std::fs::write(&out, payload).unwrap();
    let exit = if payload.contains(r#""bucket":"fail""#) || payload.contains(r#""state":"FAILURE""#)
    {
        1
    } else if payload.contains(r#""bucket":"pending""#) || payload.contains(r#""state":"PENDING""#)
    {
        8
    } else {
        0
    };
    write_exe(
        &format!("{bin}/gh"),
        &format!("#!/bin/sh\ncat {}\nexit {exit}\n", go::quote(&out)),
    );
    prepend_path(bin)
}

fn write_exe(path: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn prepend_path(dir: &str) -> EnvGuard {
    let path = std::env::var("PATH").unwrap_or_default();
    set_env("PATH", &format!("{dir}:{path}"))
}

const FAKE_GH_PR: &str = r#"#!/bin/sh
if [ "$1" = "pr" ] && [ "$2" = "view" ]; then
  printf '%s\n' '{"title":"OAuth","url":"https://github.com/acme/r/pull/42","baseRefName":"main","headRefName":"feat"}'
  exit 0
fi
if [ "$1" = "pr" ] && [ "$2" = "diff" ]; then
  if [ "${3:-}" = "--stat" ]; then
    printf '%s\n' ' file.go | 2 +-'
    exit 0
  fi
  printf '%s\n' 'diff --git a/file.go b/file.go'
  printf '%s\n' '+fmt.Println()'
  exit 0
fi
exit 1
"#;

fn git(dir: &str, args: &[&str]) {
    let out = Proc::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
}

fn repo_with_commit() -> (tempfile::TempDir, String) {
    let (g, repo) = temp_dir();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    std::fs::write(format!("{repo}/f.txt"), "x\n").unwrap();
    git(&repo, &["add", "f.txt"]);
    git(&repo, &["commit", "-m", "init"]);
    (g, repo)
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn environ_strips_stale_recur_vars() {
    let _l = lock();
    let _a = set_env("RECUR_CLIPBOARD", "stale");
    let _b = set_env("PORT", "9999");
    let env = environ(&RunContext {
        clipboard: "fresh".into(),
        soft_tabs: true,
        tab_size: 4,
        ..Default::default()
    });
    let clip: Vec<_> = env.iter().filter(|(k, _)| k == "RECUR_CLIPBOARD").collect();
    assert_eq!(clip.len(), 1);
    assert_eq!(clip[0].1, "fresh");
    assert_eq!(env_lookup(&env, "RECUR_SOFT_TABS").unwrap(), "1");
    let penv = prompt_environ(&PromptRunContext::default());
    assert!(
        !penv.iter().any(|(k, _)| k == "PORT"),
        "PORT must be stripped without rc.port"
    );
    let penv = prompt_environ(&PromptRunContext {
        port: 4242,
        ..Default::default()
    });
    assert_eq!(env_lookup(&penv, "PORT").unwrap(), "4242");
}

#[test]
fn fix_checks_prompt_shebang_cases() {
    let _l = lock();
    let rt = rt();
    let prompt = chat_prompt(
        "fix-checks.sh",
        "recur.chat.fix-checks",
        "Fix checks",
        "fix-checks",
    );
    let (_g1, a) = temp_dir();
    let (_g2, b) = temp_dir();
    let rc = PromptRunContext {
        worktree: a,
        work_dir: b,
        bundle_support: chat_dir(),
        pr_number: 12,
        pr_source: "gh".into(),
        ..Default::default()
    };

    let (_gb, bin) = temp_dir();
    {
        let _p = install_fake_gh_checks(
            &bin,
            r#"[{"name":"lint","state":"FAILURE","bucket":"fail","link":"https://example.com/lint","description":"exit 1"},{"name":"build","state":"SUCCESS","bucket":"pass","link":"https://example.com/build","description":""}]"#,
        );
        let res = rt
            .block_on(run_prompt_shebang(
                &shebang_ctx(),
                &prompt,
                PromptRunContext {
                    pr_url: "https://github.com/acme/r/pull/12".into(),
                    pr_title: "OAuth".into(),
                    pr_branch: "feat/oauth".into(),
                    ..rc.clone()
                },
            ))
            .unwrap();
        assert_eq!(res.display, "Fix 1 failing GitHub check");
        for want in [
            "GitHub pull request #12",
            "gh run view --log-failed",
            r#""name": "lint""#,
            "do not force-push",
            "feat/oauth",
        ] {
            assert!(
                res.prompt.contains(want),
                "missing {want:?}: {}",
                res.prompt
            );
        }
        assert!(!res.prompt.contains(r#""name": "build""#));
        let (prefix, _) = res.prompt.split_once("⟦raw:").expect("raw region");
        assert!(
            !prefix.contains("OAuth"),
            "PR title leaked outside raw region"
        );
    }

    let two_failing = r#"[{"name":"lint","state":"FAILURE","bucket":"fail","link":"https://example.com/lint","description":"exit 1"},{"name":"typecheck","state":"FAILURE","bucket":"fail","link":"https://example.com/ts","description":"tsc"}]"#;
    {
        let _p = install_fake_gh_checks(&bin, two_failing);
        let res = rt
            .block_on(run_prompt_shebang(
                &shebang_ctx(),
                &prompt,
                PromptRunContext {
                    args: "lint".into(),
                    ..rc.clone()
                },
            ))
            .unwrap();
        assert_eq!(res.display, "Fix failing check: lint");
        assert!(res.prompt.contains(r#"Focus on the check named "lint""#));
        let (prefix, _) = res.prompt.split_once("⟦raw:").expect("raw region");
        assert!(!prefix.contains("Focus on the check named"));
        assert!(!res.prompt.contains("typecheck"));

        let res = rt
            .block_on(run_prompt_shebang(
                &shebang_ctx(),
                &prompt,
                PromptRunContext {
                    args: "all".into(),
                    ..rc.clone()
                },
            ))
            .unwrap();
        assert_eq!(res.display, "Fix 2 failing GitHub checks");
        assert!(!res.prompt.contains("Focus on the check named"));
        assert!(
            res.prompt.contains(r#""name": "lint""#)
                && res.prompt.contains(r#""name": "typecheck""#)
        );

        let res = rt
            .block_on(run_prompt_shebang(
                &shebang_ctx(),
                &prompt,
                PromptRunContext {
                    args: "all".into(),
                    params: [("check".to_string(), "lint".to_string())].into(),
                    ..rc.clone()
                },
            ))
            .unwrap();
        assert_eq!(res.display, "Fix failing check: lint");
        assert!(!res.prompt.contains("typecheck"));
    }
    {
        let _p = install_fake_gh_checks(
            &bin,
            r#"[{"name":"lint","state":"SUCCESS","bucket":"pass","link":"https://example.com/lint","description":""}]"#,
        );
        let res = rt
            .block_on(run_prompt_shebang(&shebang_ctx(), &prompt, rc.clone()))
            .unwrap();
        assert_eq!(
            (res.prompt.as_str(), res.display.as_str()),
            ("", "All checks passed")
        );
    }
}

#[test]
fn fix_checks_complete_cases() {
    let _l = lock();
    let rt = rt();
    let dir = chat_dir();
    let prompt = PromptAction {
        id: "recur.chat.fix-checks".into(),
        name: "Fix checks".into(),
        slash: "fix-checks".into(),
        complete_interpreter: INTERPRETER_SHEBANG.into(),
        complete_body: std::fs::read_to_string(format!("{dir}/src/complete-checks.sh")).unwrap(),
        bundle_dir: dir.clone(),
        ..Default::default()
    };
    let (_g1, a) = temp_dir();
    let (_g2, b) = temp_dir();
    let rc = PromptRunContext {
        worktree: a,
        work_dir: b,
        bundle_support: dir,
        pr_number: 12,
        pr_source: "gh".into(),
        complete_arg: "check".into(),
        ..Default::default()
    };
    let (_gb, bin) = temp_dir();
    let _p = install_fake_gh_checks(
        &bin,
        r#"[{"name":"lint","state":"FAILURE","bucket":"fail"},{"name":"build","state":"SUCCESS","bucket":"pass"},{"name":"typecheck","state":"FAILURE","bucket":"fail"}]"#,
    );
    let res = rt
        .block_on(run_prompt_complete_shebang(
            &shebang_ctx(),
            &prompt,
            rc.clone(),
        ))
        .unwrap();
    assert_eq!(res.hits.len(), 3, "{:?}", res.hits);
    assert_eq!(
        (res.hits[0].insert.as_str(), res.hits[0].badge.as_str()),
        ("all", "All")
    );
    let names: Vec<&str> = res.hits[1..].iter().map(|h| h.insert.as_str()).collect();
    assert!(names.contains(&"lint") && names.contains(&"typecheck") && !names.contains(&"build"));
    assert!(res.hits[1..].iter().all(|h| h.badge == "Check"));

    let res = rt
        .block_on(run_prompt_complete_shebang(
            &shebang_ctx(),
            &prompt,
            PromptRunContext {
                args: "lint".into(),
                ..rc
            },
        ))
        .unwrap();
    assert_eq!(res.hits.len(), 1);
    assert_eq!(res.hits[0].insert, "lint");
}

#[test]
fn review_prompt_adapts_to_surface() {
    let _l = lock();
    let rt = rt();
    let prompt = chat_prompt("review.sh", "recur.chat.review", "Review", "");
    let (_g, repo) = repo_with_commit();
    let rc = PromptRunContext {
        worktree: repo.clone(),
        work_dir: repo.clone(),
        bundle_support: chat_dir(),
        ..Default::default()
    };
    let ws = rt
        .block_on(run_prompt_shebang(
            &shebang_ctx(),
            &prompt,
            PromptRunContext {
                surface: "workspace".into(),
                base_branch: "main".into(),
                ..rc.clone()
            },
        ))
        .unwrap();
    assert!(ws.prompt.contains("changes in this worktree"));
    assert_eq!(
        ws.display,
        "Please review the changes in this worktree and file any real issues"
    );
    assert!(!ws.prompt.contains("over-engineering only"));

    let (_gb, bin) = temp_dir();
    write_exe(&format!("{bin}/gh"), FAKE_GH_PR);
    let _p = prepend_path(&bin);
    let gh = rt
        .block_on(run_prompt_shebang(
            &shebang_ctx(),
            &prompt,
            PromptRunContext {
                surface: "github_pr".into(),
                pr_number: 42,
                pr_url: "https://github.com/acme/r/pull/42".into(),
                pr_title: "OAuth".into(),
                pr_host: true,
                pr_base: "main".into(),
                pr_branch: "feat".into(),
                ..rc.clone()
            },
        ))
        .unwrap();
    assert_eq!(gh.display, "Please review GitHub PR #42");
    for want in [
        "GitHub pull request #42",
        "gh pr diff",
        "working copy",
        "+fmt.Println()",
        "local drafts",
    ] {
        assert!(gh.prompt.contains(want), "missing {want:?}");
    }
    assert!(!gh.prompt.contains("changes in this worktree"));

    let local = rt
        .block_on(run_prompt_shebang(
            &shebang_ctx(),
            &prompt,
            PromptRunContext {
                surface: "local_pr".into(),
                pr_number: 7,
                pr_title: "WIP".into(),
                base_branch: "main".into(),
                ..rc
            },
        ))
        .unwrap();
    assert_eq!(local.display, "Please review Local PR #7");
    assert!(local.prompt.contains("Local PR #7") && local.prompt.contains("not a GitHub"));
}

#[test]
fn simplify_review_and_audit_prompts() {
    let _l = lock();
    let rt = rt();
    let (_g, repo) = repo_with_commit();
    let rc = PromptRunContext {
        worktree: repo.clone(),
        work_dir: repo.clone(),
        bundle_support: chat_dir(),
        surface: "workspace".into(),
        base_branch: "main".into(),
        ..Default::default()
    };
    let run_kind = |script: &str, id: &str, rc: PromptRunContext| {
        let p = chat_prompt(script, id, id, "");
        rt.block_on(run_prompt_shebang(&shebang_ctx(), &p, rc))
            .unwrap()
    };
    let rev = run_kind(
        "simplify-review.sh",
        "recur.chat.simplify-review",
        rc.clone(),
    );
    assert_eq!(
        rev.display,
        "Simplify review — file over-engineering findings"
    );
    for want in [
        "over-engineering only",
        "mcp__recur__report_finding",
        "The diff's best outcome is getting shorter",
        "Lean already. Ship.",
        "yagni:",
        "end_line: 38",
        "changes in this worktree",
    ] {
        assert!(
            rev.prompt.contains(want),
            "simplify-review missing {want:?}"
        );
    }
    assert!(!rev.prompt.contains("every real bug, risk, or issue"));
    assert!(!rev.prompt.contains("L<line>") && !rev.prompt.contains("`L12-38:"));
    assert!(!rev.prompt.to_lowercase().contains("ponytail"));

    let audit = run_kind("simplify-audit.sh", "recur.chat.simplify-audit", rc.clone());
    assert_eq!(
        audit.display,
        "Simplify audit — file over-engineering findings"
    );
    for want in [
        "Audit this repository for over-engineering",
        "mcp__recur__report_finding",
        "Scan the whole tree instead of a diff",
        "Lean already. Ship.",
        "## Hunt",
    ] {
        assert!(
            audit.prompt.contains(want),
            "simplify-audit missing {want:?}"
        );
    }
    assert!(!audit.prompt.to_lowercase().contains("ponytail"));
    assert!(
        !audit.prompt.contains("Git context is already gathered")
            && !audit.prompt.contains("## git diff")
    );

    let (_gb, bin) = temp_dir();
    write_exe(&format!("{bin}/gh"), FAKE_GH_PR);
    let _p = prepend_path(&bin);
    let host = run_kind(
        "simplify-audit.sh",
        "recur.chat.simplify-audit",
        PromptRunContext {
            surface: "github_pr".into(),
            pr_number: 42,
            pr_url: "https://github.com/acme/r/pull/42".into(),
            pr_title: "OAuth".into(),
            pr_host: true,
            pr_base: "main".into(),
            pr_branch: "feat".into(),
            base_branch: String::new(),
            ..rc
        },
    );
    assert_eq!(
        host.display,
        "Simplify audit isn't available on a GitHub PR — use /simplify-review"
    );
    for want in ["Refused", "/simplify-review", "working copy"] {
        assert!(host.prompt.contains(want), "host prompt missing {want:?}");
    }
    assert!(!host.prompt.contains("Scan the whole tree"));
    assert!(!host.prompt.contains("gh pr diff"));
    assert!(!host.prompt.contains("mcp__recur__report_finding"));
}
