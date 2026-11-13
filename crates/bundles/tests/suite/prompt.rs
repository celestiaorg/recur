//! Bundle `[[prompt]]` actions (cases that need a fake `gh` on PATH live in
//! `tests/env_serial`).

use std::collections::BTreeMap;
use std::process::Command as Proc;
use std::time::Duration;

use recur_bundles::*;

use crate::common::*;

/// Outlives the production prompt budget under full-suite load.
pub fn shebang_ctx() -> RunCtx {
    RunCtx::background().with_timeout(Duration::from_secs(60))
}

fn chat_dir() -> String {
    repo_path("bundles/Chat")
}

fn chat_prompts() -> Vec<PromptAction> {
    let dir = chat_dir();
    let data = std::fs::read(format!("{dir}/bundle.toml")).unwrap();
    parse_manifest(&dir, &data).unwrap().2
}

fn chat_script_prompt(script: &str, id: &str, name: &str, slash: &str) -> PromptAction {
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

/// A git repo on `main` with one commit, plus the run helper.
pub fn git_repo() -> (tempfile::TempDir, String) {
    let (g, repo) = temp_dir();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.name", "test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    (g, repo)
}

pub fn git(dir: &str, args: &[&str]) {
    let out = Proc::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn commit_file(repo: &str, name: &str, body: &str, msg: &str) {
    std::fs::write(format!("{repo}/{name}"), body).unwrap();
    git(repo, &["add", name]);
    git(repo, &["commit", "-m", msg]);
}

fn parse(toml: &str) -> Result<(BundleMeta, Vec<Command>, Vec<PromptAction>), Error> {
    let (_g, dir) = temp_dir();
    parse_manifest(&dir, toml.as_bytes())
}

#[test]
fn parse_manifest_prompt_actions() {
    let (meta, cmds, prompts) = parse(
        r#"
[bundle]
name = "Chat"
id = "bundle-chat"

[[prompt]]
id = "prompt-fix"
name = "Fix Findings"
description = "Fix open findings"
mode = "agent"
timeoutMs = 10000
interpreter = "shebang"
body = '''
#!/bin/sh
printf '{"prompt":"fix it","display":"Fix"}'
'''

[[prompt]]
id = "prompt-explain"
name = "Explain Selection"
interpreter = "typescript"
takesArgs = true
body = '''
import type { PromptContext, PromptResult } from '@recur/sdk'
export default function (ctx: PromptContext): PromptResult {
  return { prompt: ctx.selectedText, display: "Explain" }
}
'''
"#,
    )
    .unwrap();
    assert_eq!(meta.name, "Chat");
    assert!(cmds.is_empty());
    assert_eq!(prompts.len(), 2);
    let fix = &prompts[0];
    assert_eq!(fix.slash, "fix-findings");
    assert_eq!(
        (fix.mode.as_str(), fix.timeout_ms, fix.fresh_session),
        ("agent", 10000, false)
    );
    let explain = &prompts[1];
    assert_eq!(explain.slash, "explain-selection");
    assert_eq!(explain.interpreter, INTERPRETER_TYPESCRIPT);
    assert!(explain.takes_args);
    assert!(!explain.fresh_session);
}

#[test]
fn parse_manifest_fresh_session_and_tab_group() {
    let (_, _, prompts) = parse(
        "[bundle]\nname = \"Chat\"\nid = \"bundle-chat\"\n[[prompt]]\nid = \"prompt-review\"\nname = \"Review\"\nslash = \"review\"\nfresh_session = true\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n[[prompt]]\nid = \"prompt-commit\"\nname = \"Commit\"\nslash = \"commit\"\nmode = \"agent\"\ntab_group = \"ops\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    )
    .unwrap();
    assert!(prompts[0].fresh_session);
    assert_eq!(prompts[1].tab_group, "ops");
}

#[test]
fn parse_manifest_prompt_complete() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/complete.sh"),
        "#!/bin/sh\nprintf '{\"hits\":[]}\\n'\n",
    );
    let (_, _, prompts) = parse_manifest(
        &dir,
        b"\n[bundle]\nname = \"T\"\nid = \"b\"\n\n[[prompt]]\nid = \"p1\"\nname = \"X\"\nslash = \"x\"\ntakesArgs = true\nargPlaceholder = \"id or URL\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\ncomplete = \"complete.sh\"\n",
    )
    .unwrap();
    let p = &prompts[0];
    assert_eq!(p.arg_placeholder, "id or URL");
    assert_eq!(p.args.len(), 1);
    assert_eq!(p.args[0].name, "args");
    assert_eq!(p.complete_interpreter, INTERPRETER_SHEBANG);
    assert!(p.complete_body.contains("hits"));
    assert_eq!(p.complete_path, format!("{dir}/complete.sh"));
}

#[test]
fn parse_manifest_prompt_icon() {
    let (_g, dir) = temp_dir();
    must_write(
        &format!("{dir}/mark.svg"),
        r##"<svg viewBox="0 0 8 8"><path fill="#222326" d="M0 0h8v8z"/></svg>"##,
    );
    let (_, _, prompts) = parse_manifest(
        &dir,
        b"[bundle]\nname = \"T\"\nid = \"b\"\n[[prompt]]\nid = \"p1\"\nname = \"X\"\nslash = \"x\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\nicon = \"mark.svg\"\n",
    )
    .unwrap();
    assert_eq!(
        prompts[0].icon_svg,
        r#"<svg viewBox="0 0 8 8"><path fill="currentColor" d="M0 0h8v8z"/></svg>"#
    );
    must_write(
        &format!("{dir}/bad.svg"),
        "<svg><script>alert(1)</script></svg>",
    );
    let err = parse_manifest(
        &dir,
        b"[bundle]\nname = \"T\"\nid = \"b\"\n[[prompt]]\nid = \"p1\"\nname = \"X\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\nicon = \"bad.svg\"\n",
    )
    .unwrap_err();
    assert_eq!(
        err.0,
        "prompt \"p1\": icon \"bad.svg\": svg is not a static icon"
    );
}

#[test]
fn parse_manifest_prompt_named_and_optional_args() {
    let (_, _, prompts) = parse(
        "[bundle]\nname = \"T\"\nid = \"b\"\n[[prompt]]\nid = \"p1\"\nname = \"X\"\nslash = \"linear\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n\n[[prompt.arg]]\nname = \"issue\"\nplaceholder = \"TEAM-123\"\n\n[[prompt]]\nid = \"p2\"\nname = \"Y\"\nslash = \"fix-checks\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n\n[[prompt.arg]]\nname = \"check\"\nplaceholder = \"all or a failing check\"\noptional = true\n",
    )
    .unwrap();
    let p = &prompts[0];
    assert!(p.takes_args);
    assert_eq!(
        p.args,
        [PromptArgSpec {
            name: "issue".into(),
            placeholder: "TEAM-123".into(),
            optional: false
        }]
    );
    assert_eq!(p.arg_placeholder, "TEAM-123");
    let q = &prompts[1];
    assert!(q.takes_args);
    assert_eq!(q.args[0].name, "check");
    assert!(q.args[0].optional);
}

#[test]
fn shipped_fix_checks_prompt_args() {
    let prompts = chat_prompts();
    let p = find_prompt_by_slash(&prompts, "fix-checks").expect("missing /fix-checks");
    assert!(p.takes_args);
    assert!(!p.complete_body.trim().is_empty());
    assert_eq!(p.args.len(), 1);
    assert_eq!(p.args[0].name, "check");
    assert!(p.args[0].optional);
    assert!(p.icon_svg.contains(r#"fill="currentColor""#));
    assert!(p.fresh_session);
    assert_eq!(p.tab_group, "ops");
}

#[test]
fn shipped_address_review_prompt_args() {
    let prompts = chat_prompts();
    let p = find_prompt_by_slash(&prompts, "address-review").expect("missing /address-review");
    assert_eq!(p.mode, "agent");
    assert!(p.takes_args);
    assert_eq!(p.args.len(), 1);
    assert_eq!(p.args[0].name, "reviewer");
    assert!(p.args[0].optional);
    assert!(p.fresh_session);
}

#[test]
fn parse_manifest_prompt_loop_mode() {
    let (_, _, prompts) = parse(
        "[bundle]\nname = \"Chat\"\nid = \"bundle-chat\"\n[[prompt]]\nid = \"prompt-converge\"\nname = \"Review & fix\"\nslash = \"review-and-fix\"\nmode = \"loop\"\nloop = \"recur.loops.converge\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n[[prompt]]\nid = \"prompt-implied\"\nname = \"Implied Loop\"\nloop = \"recur.loops.docs\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    )
    .unwrap();
    assert_eq!(
        (prompts[0].mode.as_str(), prompts[0].loop_id.as_str()),
        ("loop", "recur.loops.converge")
    );
    assert_eq!(
        (prompts[1].mode.as_str(), prompts[1].loop_id.as_str()),
        ("loop", "recur.loops.docs")
    );
}

#[test]
fn parse_shipped_prompt_complete() {
    let dir = repo_path("bundles/Linear");
    let data = std::fs::read(format!("{dir}/bundle.toml")).unwrap();
    let parsed = parse_manifest_full(&dir, &data).unwrap();
    let p = find_prompt_by_slash(&parsed.prompts, "linear").expect("linear");
    assert!(p.mode.is_empty());
    assert!(p.takes_args);
    assert!(p.arg_placeholder.contains("TEAM-123"));
    assert_eq!(p.interpreter, INTERPRETER_SHEBANG);
    assert!(p.body.trim_start_matches('\u{feff}').starts_with("#!"));
    assert_eq!(p.complete_interpreter, INTERPRETER_SHEBANG);
    assert!(
        p.complete_body
            .trim_start_matches('\u{feff}')
            .starts_with("#!")
    );
    assert_eq!(p.args.len(), 1);
    assert_eq!(p.args[0].name, "issue");
    assert!(p.icon_svg.contains("currentColor") && p.icon_svg.contains("viewBox"));
    assert_eq!(parsed.searches.len(), 1);
    let se = &parsed.searches[0];
    assert_eq!(
        (se.name.as_str(), se.interpreter.as_str()),
        ("Linear", INTERPRETER_SHEBANG)
    );
    assert!(se.icon_svg.contains("viewBox"));
    assert!(se.body.starts_with("#!"));
    assert_eq!(parsed.expands.len(), 1);
    let ex = &parsed.expands[0];
    assert_eq!(ex.name, "Linear issue");
    assert!(ex.match_re.is_some());
    assert!(ex.matches("ENG-123 also tests"));
    assert!(!ex.matches("fix the flaky test"));
    assert!(!ex.matches("2026-09-01"));
    assert!(ex.body.starts_with("#!"));
}

#[test]
fn parse_shipped_github_expand() {
    let dir = repo_path("bundles/GitHub");
    let data = std::fs::read(format!("{dir}/bundle.toml")).unwrap();
    let parsed = parse_manifest_full(&dir, &data).unwrap();
    let ex = &parsed.expands[0];
    assert!(ex.matches("https://github.com/acme/app/issues/42"));
    assert!(ex.matches("acme/app#42 please"));
    assert!(ex.matches("/github"));
    assert!(!ex.matches("acme/app#42x"));
    assert!(!ex.matches("fix the flaky test"));
}

#[tokio::test(flavor = "multi_thread")]
async fn shipped_chat_loop_prompts() {
    let prompts = chat_prompts();
    let want = BTreeMap::from([
        ("review-and-fix", "recur.loops.converge"),
        ("review-to-pr", "recur.loops.converge-pr"),
        ("to-a-pr", "recur.loops.pr"),
        ("write-docs", "recur.loops.docs"),
        ("security-review", "recur.loops.security"),
    ]);
    for (slash, loop_id) in &want {
        let p = find_prompt_by_slash(&prompts, slash).unwrap_or_else(|| panic!("missing /{slash}"));
        assert_eq!((p.mode.as_str(), p.loop_id.as_str()), ("loop", *loop_id));
        let res = run_prompt_shebang(
            &shebang_ctx(),
            p,
            PromptRunContext {
                bundle_support: chat_dir(),
                ..Default::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("/{slash}: {e}"));
        assert!(
            !res.prompt.trim().is_empty() && !res.display.trim().is_empty(),
            "/{slash}: {res:?}"
        );
    }
}

#[test]
fn shipped_chat_fresh_session_and_tab_groups() {
    let prompts = chat_prompts();
    for (slash, fresh, group) in [
        ("fix-findings", true, "ops"),
        ("commit", true, "ops"),
        ("commit-push", true, "ops"),
        ("open-pr", true, "ops"),
        ("open-draft-pr", true, "ops"),
        ("fix-checks", true, "ops"),
        ("address-review", true, "ops"),
        ("fix-problem", true, "ops"),
        ("fix-merge", true, "ops"),
        ("update", true, "ops"),
        ("review", true, "review"),
        ("re-review", true, ""),
        ("simplify-review", true, "simplify-review"),
        ("simplify-audit", true, "simplify-audit"),
        ("review-and-fix", false, ""),
        ("review-to-pr", false, ""),
        ("to-a-pr", false, ""),
        ("write-docs", false, ""),
        ("security-review", false, ""),
    ] {
        let p = find_prompt_by_slash(&prompts, slash).unwrap_or_else(|| panic!("missing /{slash}"));
        assert_eq!(p.fresh_session, fresh, "/{slash} fresh_session");
        assert_eq!(p.tab_group, group, "/{slash} tab_group");
    }
}

#[test]
fn parse_manifest_prompt_validation() {
    let base = "[bundle]\nname = \"T\"\nid = \"b\"\n";
    let body = "body = \"#!/bin/sh\\ntrue\\n\"\n";
    for (name, rest, want) in [
        ("bad slash", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\nslash = \"Bad_Slash\"\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": slash \"Bad_Slash\" must match ^[a-z][a-z0-9-]*$"),
        ("bad mode", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\nmode = \"workflow\"\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": unknown mode \"workflow\" (use ask|plan|agent|review|loop, or omit)"),
        ("loop requires loop mode", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\nmode = \"agent\"\nloop = \"recur.loops.converge\"\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": loop requires mode \"loop\""),
        ("fresh session with loop mode", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\nmode = \"loop\"\nfresh_session = true\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": fresh_session is not valid with mode \"loop\""),
        ("tab group without mode", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\ntab_group = \"ops\"\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": tab_group requires mode"),
        ("bad tab group", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\nmode = \"agent\"\ntab_group = \"Ops\"\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": tab_group \"Ops\" must match ^[a-z][a-z0-9-]*$"),
        ("bad loop id", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\nmode = \"loop\"\nloop = \"../evil\"\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": loop \"../evil\" is not a safe id"),
        ("timeout ceiling", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\ntimeoutMs = 30001\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": timeoutMs must be 0 or 1..30000"),
        ("body and script", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\ninterpreter = \"shebang\"\n{body}script = \"nope.sh\"\n"), "prompt \"p1\": set body or script, not both"),
        ("missing name", format!("[[prompt]]\nid = \"p1\"\ninterpreter = \"shebang\"\n{body}"), "prompt \"p1\": name is required"),
        ("complete without args", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\ninterpreter = \"shebang\"\n{body}complete = \"complete.sh\"\n"), "prompt \"p1\": complete requires takesArgs"),
        ("bad arg name", format!("[[prompt]]\nid = \"p1\"\nname = \"X\"\ninterpreter = \"shebang\"\n{body}[[prompt.arg]]\nname = \"Issue-ID\"\n"), "prompt \"p1\": arg \"Issue-ID\" must match ^[a-z][a-z0-9_]{0,39}$"),
        ("search typescript", "[[search]]\nid = \"s1\"\nname = \"S\"\ninterpreter = \"typescript\"\nbody = \"export function complete() { return { hits: [] } }\"\n".to_string(), "search \"s1\": interpreter must be shebang"),
        ("expand bad match", format!("[[expand]]\nid = \"e1\"\nname = \"E\"\nmatch = \"(\"\ninterpreter = \"shebang\"\n{body}"), "expand \"e1\": match: regex parse error"),
    ] {
        let err = parse(&format!("{base}{rest}")).unwrap_err();
        assert!(err.0.starts_with(want), "{name}: got {err}, want {want}");
    }
}

#[test]
fn discover_prompts_sorted_by_slash() {
    let (_g, def) = temp_dir();
    write_bundle(
        &def,
        "Chat",
        "[bundle]\nname = \"Chat\"\nid = \"b-chat\"\n[[prompt]]\nid = \"p-z\"\nname = \"Zed\"\nslash = \"zed\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n[[prompt]]\nid = \"p-a\"\nname = \"Audit\"\nslash = \"audit\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (_, prompts, _) = discover_report(&DiscoverRoots {
        user_dir: String::new(),
        default_dir: def,
    })
    .unwrap();
    let slashes: Vec<&str> = prompts.iter().map(|p| p.slash.as_str()).collect();
    assert_eq!(slashes, ["audit", "zed"]);
}

#[test]
fn discover_prompt_slash_user_beats_default() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    write_bundle(
        &def,
        "Chat",
        "[bundle]\nname = \"Chat\"\nid = \"b-chat-default\"\n[[prompt]]\nid = \"prompt-audit-default\"\nname = \"Audit\"\nslash = \"audit\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    write_bundle(
        &user,
        "Chat",
        "[bundle]\nname = \"Chat\"\nid = \"b-chat-user\"\n[[prompt]]\nid = \"prompt-audit-user\"\nname = \"Audit User\"\nslash = \"audit\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (_, prompts, skips) = discover_report(&DiscoverRoots {
        user_dir: user,
        default_dir: def.clone(),
    })
    .unwrap();
    let p = find_prompt_by_slash(&prompts, "audit").unwrap();
    assert_eq!(
        (p.id.as_str(), p.source_root.as_str()),
        ("prompt-audit-user", "user")
    );
    assert_eq!(skips.len(), 1);
    assert_eq!(
        skips[0],
        DiscoverSkip {
            dir: format!("{def}/Chat"),
            name: "audit".into(),
            source: "default".into(),
            reason: "slash \"audit\" collides with prompt \"Audit User\" from bundle \"Chat\" (kept user/prompt-audit-user)".into(),
        }
    );
}

#[test]
fn discover_prompt_slash_same_source_tiebreak() {
    let (_g, def) = temp_dir();
    write_bundle(
        &def,
        "A",
        "[bundle]\nname = \"A\"\nid = \"bundle-bbb\"\n[[prompt]]\nid = \"prompt-zzz\"\nname = \"Later\"\nslash = \"shared\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    write_bundle(
        &def,
        "B",
        "[bundle]\nname = \"B\"\nid = \"bundle-aaa\"\n[[prompt]]\nid = \"prompt-aaa\"\nname = \"Earlier\"\nslash = \"shared\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (_, prompts, skips) = discover_report(&DiscoverRoots {
        default_dir: def,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        (prompts[0].id.as_str(), prompts[0].bundle_id.as_str()),
        ("prompt-aaa", "bundle-aaa")
    );
    assert_eq!(skips.len(), 1);
}

#[test]
fn discover_prompt_id_user_override_and_sorting() {
    let (_g1, def) = temp_dir();
    let (_g2, user) = temp_dir();
    write_bundle(
        &def,
        "Chat",
        "[bundle]\nname = \"Chat\"\nid = \"b-chat\"\n[[prompt]]\nid = \"shared-prompt\"\nname = \"From Default\"\nslash = \"fix-findings\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n[[prompt]]\nid = \"p-a\"\nname = \"Audit\"\nslash = \"audit\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    write_bundle(
        &user,
        "Chat",
        "[bundle]\nname = \"Chat\"\nid = \"b-chat-user\"\n[[prompt]]\nid = \"shared-prompt\"\nname = \"From User\"\nslash = \"fix-findings\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (_, prompts) = discover(&DiscoverRoots {
        user_dir: user,
        default_dir: def,
    })
    .unwrap();
    let p = find_prompt_by_id(&prompts, "shared-prompt").unwrap();
    assert_eq!(
        (p.name.as_str(), p.source_root.as_str()),
        ("From User", "user")
    );
    let slashes: Vec<&str> = prompts.iter().map(|p| p.slash.as_str()).collect();
    assert_eq!(slashes, ["audit", "fix-findings"]);
}

#[test]
fn lint_prompt_slash_dupes() {
    let (_g, root) = temp_dir();
    let a = write_bundle(
        &root,
        "A",
        "[bundle]\nname = \"A\"\nid = \"bundle-a\"\n[[prompt]]\nid = \"p-a\"\nname = \"Audit A\"\nslash = \"audit\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let b = write_bundle(
        &root,
        "B",
        "[bundle]\nname = \"B\"\nid = \"bundle-b\"\n[[prompt]]\nid = \"p-b\"\nname = \"Audit B\"\nslash = \"audit\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (_g2, cache) = temp_dir();
    let rep = lint_bundles(&[a.clone(), b], &cache);
    assert_eq!(
        format_lint_report(&rep, false),
        format!(
            "error\tA/Audit A\tslash \"audit\" duplicated by A/Audit A, B/Audit B\n  {a}\n1 errors, 0 info\n"
        )
    );
}

#[test]
fn lint_prompt_same_bundle_slash_dupe() {
    let (_g, root) = temp_dir();
    let dir = write_bundle(
        &root,
        "Chat",
        "[bundle]\nname = \"Chat\"\nid = \"b-chat\"\n[[prompt]]\nid = \"p1\"\nname = \"One\"\nslash = \"same\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n[[prompt]]\nid = \"p2\"\nname = \"Two\"\nslash = \"same\"\ninterpreter = \"shebang\"\nbody = \"#!/bin/sh\\ntrue\\n\"\n",
    );
    let (_g2, cache) = temp_dir();
    let rep = lint_bundles(std::slice::from_ref(&dir), &cache);
    assert!(!rep.ok());
    assert!(
        format_lint_report(&rep, false).contains("slash \"same\" duplicated with prompt \"One\"")
    );
}

#[test]
fn compile_prompt_and_complete_macros() {
    let (_g, cache) = temp_dir();
    let prompt = PromptAction {
        id: "prompt-ts".into(),
        name: "Explain".into(),
        interpreter: INTERPRETER_TYPESCRIPT.into(),
        body: "\nimport type { PromptContext, PromptResult } from '@recur/sdk'\nexport default function (ctx: PromptContext): PromptResult {\n  return { prompt: \"explain \" + ctx.selectedText, display: \"Explain\" }\n}\n".into(),
        ..Default::default()
    };
    let r = compile_prompt_macro(&cache, &prompt).unwrap();
    assert!(r.js.contains("selectedText") && r.js.contains("Explain"));
    let sh = PromptAction {
        id: "prompt-sh".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "true".into(),
        ..Default::default()
    };
    assert!(compile_prompt_macro(&cache, &sh).is_err());
    let complete = PromptAction {
        id: "prompt-complete".into(),
        complete_interpreter: INTERPRETER_TYPESCRIPT.into(),
        complete_body: "import type { PromptCompleteContext, PromptCompleteResult } from '@recur/sdk'\nexport default function (ctx: PromptCompleteContext): PromptCompleteResult {\n  return { hits: [{ insert: ctx.args, label: ctx.args }] }\n}\n".into(),
        ..Default::default()
    };
    let r = compile_complete_macro(&cache, &complete).unwrap();
    assert!(r.js.contains("hits"));
    assert!(
        std::path::Path::new(&format!("{cache}/prompt-complete-complete.{}.js", r.hash)).exists()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_prompt_type_script() {
    let (_g, cache) = temp_dir();
    let prompt = PromptAction {
        id: "p-ts".into(),
        name: "Explain".into(),
        interpreter: INTERPRETER_TYPESCRIPT.into(),
        body: "return { prompt: `explain ${ctx.selectedText} in ${ctx.filename} tab=${ctx.tabSize}`, display: '' }".into(),
        ..Default::default()
    };
    let res = run_prompt(
        &shebang_ctx(),
        &cache,
        &prompt,
        PromptRunContext {
            selected_text: "x".into(),
            file_path: "/a/b.go".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(res.prompt, "explain x in b.go tab=4");
    assert_eq!(res.display, "Explain");
    let bad = PromptAction {
        body: "throw new Error('nope')".into(),
        ..prompt
    };
    let err = run_prompt(&shebang_ctx(), &cache, &bad, PromptRunContext::default())
        .await
        .unwrap_err();
    assert_eq!(err.0, "prompt \"p-ts\" exited 1: nope");
}

#[tokio::test(flavor = "multi_thread")]
async fn shipped_commit_prompts_honor_conventional_commits() {
    let (_g, cache) = temp_dir();
    let prompts = chat_prompts();
    for slash in ["commit", "commit-push"] {
        let p = find_prompt_by_slash(&prompts, slash).unwrap();
        for on in [false, true] {
            let res = run_prompt(
                &shebang_ctx(),
                &cache,
                p,
                PromptRunContext {
                    conventional_commits: on,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(
                res.prompt.contains("Use Conventional Commits"),
                on,
                "/{slash}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_prompt_complete_shebang_hits() {
    let prompt = PromptAction {
        id: "prompt-complete".into(),
        slash: "linear".into(),
        complete_interpreter: INTERPRETER_SHEBANG.into(),
        complete_body: "#!/bin/sh\nprintf '%s\\n' '{\"hits\":[{\"insert\":\"ENG-1\",\"label\":\"ENG-1\",\"detail\":\"Fix login\",\"badge\":\"Issue\"},{\"label\":\"Connect Linear…\",\"action\":\"settings:integrations\"}],\"message\":\"ok\"}'\n".into(),
        ..Default::default()
    };
    let res = run_prompt_complete_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            args: "ENG".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(res.message, "ok");
    assert_eq!(res.hits.len(), 2);
    assert_eq!(
        (res.hits[0].insert.as_str(), res.hits[0].badge.as_str()),
        ("ENG-1", "Issue")
    );
    assert_eq!(
        (res.hits[1].action.as_str(), res.hits[1].label.as_str()),
        ("settings:integrations", "Connect Linear…")
    );

    let empty = PromptAction {
        id: "e".into(),
        complete_interpreter: INTERPRETER_SHEBANG.into(),
        complete_body: "#!/bin/sh\nprintf '\\n'\n".into(),
        ..Default::default()
    };
    let res = run_prompt_complete_shebang(&shebang_ctx(), &empty, PromptRunContext::default())
        .await
        .unwrap();
    assert!(res.hits.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn collect_search_groups_one_failure() {
    let ok = SearchAction {
        id: "ok".into(),
        name: "Good".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\nprintf '%s\\n' '{\"hits\":[{\"insert\":\"ENG-1\",\"label\":\"Fix\",\"psub\":\"ENG-1\"}]}'\n".into(),
        ..Default::default()
    };
    let bad = SearchAction {
        id: "bad".into(),
        name: "Bad".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\nexit 1\n".into(),
        ..Default::default()
    };
    let groups = collect_search_groups(&RunCtx::background(), &[ok, bad], None).await;
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name, "Good");
    assert_eq!(groups[0].hits.len(), 1);
    assert_eq!(groups[0].hits[0].psub, "ENG-1");
    assert_eq!(groups[1].name, "Bad");
    assert!(groups[1].hits.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn expand_matching_cases() {
    let matching = ExpandAction {
        id: "e".into(),
        name: "E".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        match_re: Some(regex::Regex::new(r"(?i)^[A-Z][A-Z0-9]+-\d+").unwrap()),
        body: "#!/bin/sh\nprintf '{\"prompt\":\"Work on %s\",\"display\":\"Linear\"}\\n' \"$RECUR_PROMPT_ARGS\"\n".into(),
        ..Default::default()
    };
    let skip = ExpandAction {
        id: "s".into(),
        name: "S".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        match_re: Some(regex::Regex::new("^ENG-123").unwrap()),
        body: "#!/bin/sh\nprintf '%s\\n' '{\"skip\":true}'\n".into(),
        ..Default::default()
    };
    let prose = expand_matching("fix the flaky test", std::slice::from_ref(&matching), None)
        .await
        .unwrap();
    assert_eq!(prose.prompt, "fix the flaky test");
    let hit = expand_matching("ENG-123 also tests", std::slice::from_ref(&matching), None)
        .await
        .unwrap();
    assert_eq!(hit.prompt, "Work on ENG-123 also tests");
    let skipped = expand_matching("ENG-123", &[skip, matching], None)
        .await
        .unwrap();
    assert_eq!(skipped.prompt, "Work on ENG-123");
    let never = ExpandAction {
        id: "never".into(),
        name: "Never".into(),
        ..Default::default()
    };
    assert!(!never.matches("ENG-123"));
}

#[tokio::test(flavor = "multi_thread")]
async fn run_prompt_shebang_env_and_display() {
    let (_g, work) = temp_dir();
    let echo = PromptAction {
        id: "prompt-echo".into(),
        name: "Echo Prompt".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"wire @src/main.go /skill go-code-review\",\"display\":\"Echo\\nLabel\"}'\n".into(),
        ..Default::default()
    };
    let mut rc = PromptRunContext {
        workspace_id: "WS1".into(),
        project_id: "proj".into(),
        worktree: work,
        port: 4242,
        selected_text: "sel".into(),
        clipboard: "clip".into(),
        ..Default::default()
    };
    let res = run_prompt_shebang(&shebang_ctx(), &echo, rc.clone())
        .await
        .unwrap();
    assert_eq!(res.prompt, "wire @src/main.go /skill go-code-review");
    assert_eq!(res.display, "Echo Label");

    let script = |id: &str, fmt: &str, vars: &str| PromptAction {
        id: id.into(),
        name: id.into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: format!("#!/bin/sh\nprintf '{{\"prompt\":\"{fmt}\",\"display\":\"x\"}}' {vars}\n"),
        ..Default::default()
    };
    let env = script(
        "env",
        "id=%s port=%s clip=%s",
        "\"$RECUR_WORKSPACE_ID\" \"$RECUR_PORT\" \"$RECUR_CLIPBOARD\"",
    );
    assert_eq!(
        run_prompt_shebang(&shebang_ctx(), &env, rc.clone())
            .await
            .unwrap()
            .prompt,
        "id=WS1 port=4242 clip=clip"
    );
    rc.base_branch = "origin/main".into();
    rc.update_base_method = "rebase".into();
    let base = script(
        "base",
        "base=%s method=%s",
        "\"$RECUR_BASE_BRANCH\" \"$RECUR_UPDATE_BASE_METHOD\"",
    );
    assert_eq!(
        run_prompt_shebang(&shebang_ctx(), &base, rc.clone())
            .await
            .unwrap()
            .prompt,
        "base=origin/main method=rebase"
    );
    rc.pr_id = "gh-7".into();
    rc.pr_number = 7;
    rc.pr_source = "gh".into();
    let pr = script(
        "pr",
        "id=%s num=%s src=%s",
        "\"$RECUR_PR_ID\" \"$RECUR_PR_NUMBER\" \"$RECUR_PR_SOURCE\"",
    );
    assert_eq!(
        run_prompt_shebang(&shebang_ctx(), &pr, rc)
            .await
            .unwrap()
            .prompt,
        "id=gh-7 num=7 src=gh"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_prompt_shebang_issue_errors_and_timeout() {
    let (_g, work) = temp_dir();
    let issue = PromptAction {
        id: "p".into(),
        name: "X".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\necho '{\"prompt\":\"go\",\"issue\":{\"source\":\"linear\",\"id\":\"uuid-1\",\"key\":\"ENG-1\"}}'\n".into(),
        ..Default::default()
    };
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &issue,
        PromptRunContext {
            worktree: work.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let i = res.issue.expect("issue");
    assert_eq!(
        (i.source.as_str(), i.id.as_str(), i.key.as_str()),
        ("linear", "uuid-1", "ENG-1")
    );

    let ts = PromptAction {
        id: "p".into(),
        name: "X".into(),
        interpreter: INTERPRETER_TYPESCRIPT.into(),
        body: "x".into(),
        ..Default::default()
    };
    let err = run_prompt_shebang(&shebang_ctx(), &ts, PromptRunContext::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.0,
        "prompt \"p\" is typescript: use CompiledPromptModule"
    );

    let not_json = PromptAction {
        id: "p".into(),
        name: "X".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\necho not-json\n".into(),
        ..Default::default()
    };
    let err = run_prompt_shebang(
        &shebang_ctx(),
        &not_json,
        PromptRunContext {
            work_dir: work.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        err.0.starts_with("prompt stdout is not valid JSON: "),
        "{err}"
    );

    let empty = PromptAction {
        id: "p2".into(),
        name: "X".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\nprintf '{\"prompt\":\"\",\"display\":\"Updated with latest main\"}'\n"
            .into(),
        ..Default::default()
    };
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &empty,
        PromptRunContext {
            work_dir: work.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (res.prompt.as_str(), res.display.as_str()),
        ("", "Updated with latest main")
    );

    let fail = PromptAction {
        id: "f".into(),
        name: "F".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        body: "#!/bin/sh\necho ' bad ' >&2\nexit 4\n".into(),
        ..Default::default()
    };
    let err = run_prompt_shebang(
        &shebang_ctx(),
        &fail,
        PromptRunContext {
            work_dir: work.clone(),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, "prompt \"f\" exited 4: bad");

    let sleep = PromptAction {
        id: "sleep".into(),
        name: "Sleep".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        timeout_ms: 200,
        body: "#!/bin/sh\nsleep 5\n".into(),
        ..Default::default()
    };
    let err = run_prompt_shebang(
        &RunCtx::background(),
        &sleep,
        PromptRunContext {
            work_dir: work,
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.0,
        "prompt cancelled or timed out: context deadline exceeded"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn run_prompt_shebang_sources_via_bundle_support() {
    let (_g, dir) = temp_dir();
    must_write(&format!("{dir}/src/helper.sh"), "HELPER_OK=1\n");
    let prompt = PromptAction {
        id: "src-sib".into(),
        name: "Src".into(),
        interpreter: INTERPRETER_SHEBANG.into(),
        bundle_dir: dir.clone(),
        body: "#!/bin/sh\nset -e\n. \"${RECUR_BUNDLE_SUPPORT}/src/helper.sh\"\nprintf '{\"prompt\":\"ok=%s\",\"display\":\"Src\"}' \"$HELPER_OK\"\n".into(),
        ..Default::default()
    };
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            work_dir: dir.clone(),
            bundle_support: dir,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(res.prompt, "ok=1");
}

fn env_get(env: &[EnvVar], key: &str) -> Option<String> {
    env_lookup(env, key).map(|v| v.to_string_lossy().into_owned())
}

#[test]
fn prompt_environ_caps_large_document() {
    let big = "x".repeat(64 * 1024 + 1024);
    let env = prompt_environ(&PromptRunContext {
        document: big.clone(),
        clipboard: big.clone(),
        ..Default::default()
    });
    for key in ["RECUR_DOCUMENT", "RECUR_CLIPBOARD"] {
        let v = env_get(&env, key).unwrap();
        assert!(v.len() < big.len() && v.contains("truncated"), "{key}");
        assert!(v.starts_with(&"x".repeat(64 * 1024)));
    }
}

#[test]
fn prompt_environ_includes_args_query_and_params() {
    let env = prompt_environ(&PromptRunContext {
        args: "ENG-123 extra".into(),
        prompt_slash: "linear".into(),
        search_query: "ACME-114".into(),
        complete_arg: "issue".into(),
        params: BTreeMap::from([("issue".to_string(), "ENG-123".to_string())]),
        ..Default::default()
    });
    assert_eq!(
        env_get(&env, "RECUR_PROMPT_ARGS").as_deref(),
        Some("ENG-123 extra")
    );
    assert_eq!(
        env_get(&env, "RECUR_PROMPT_SLASH").as_deref(),
        Some("linear")
    );
    assert_eq!(
        env_get(&env, "RECUR_SEARCH_QUERY").as_deref(),
        Some("ACME-114")
    );
    assert_eq!(
        env_get(&env, "RECUR_PROMPT_ARG_ISSUE").as_deref(),
        Some("ENG-123")
    );
    assert_eq!(
        env_get(&env, "RECUR_PROMPT_PARAMS").as_deref(),
        Some(r#"{"issue":"ENG-123"}"#)
    );
    assert_eq!(
        env_get(&env, "RECUR_PROMPT_COMPLETE_ARG").as_deref(),
        Some("issue")
    );
    assert_eq!(env_get(&env, "RECUR_PR_DRAFT").as_deref(), Some("0"));
}

#[test]
fn prompt_environ_bundle_settings() {
    let env = prompt_environ(&PromptRunContext {
        bundle_settings: BTreeMap::from([
            ("api_key".to_string(), "lin_secret".to_string()),
            ("team_key".to_string(), "ENG".to_string()),
        ]),
        ..Default::default()
    });
    assert_eq!(
        env_get(&env, "RECUR_SETTING_API_KEY").as_deref(),
        Some("lin_secret")
    );
    assert_eq!(
        env_get(&env, "RECUR_SETTING_TEAM_KEY").as_deref(),
        Some("ENG")
    );
    assert_eq!(
        env_get(&env, "RECUR_BUNDLE_SETTINGS").as_deref(),
        Some(r#"{"api_key":"lin_secret","team_key":"ENG"}"#)
    );
    let env = prompt_environ(&PromptRunContext::default());
    for (k, _) in &env {
        let k = k.to_string_lossy();
        assert!(
            !k.starts_with("RECUR_SETTING_")
                && k != "RECUR_BUNDLE_SETTINGS"
                && k != "LINEAR_API_KEY",
            "{k}"
        );
    }
}

#[test]
fn params_from_args_cases() {
    let one = PromptAction {
        args: vec![PromptArgSpec {
            name: "issue".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(
        params_from_args(&one, "ENG-1 extra")["issue"],
        "ENG-1 extra"
    );
    let multi = PromptAction {
        args: ["question", "choice_1", "choice_2"]
            .iter()
            .map(|n| PromptArgSpec {
                name: n.to_string(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let got = params_from_args(&multi, r#""Which color?" Red Blue extra"#);
    assert_eq!(got["question"], "Which color?");
    assert_eq!(got["choice_1"], "Red");
    assert_eq!(got["choice_2"], "Blue extra");
    assert!(params_from_args(&PromptAction::default(), "x").is_empty());
}

#[test]
fn normalize_prompt_display_cases() {
    assert_eq!(normalize_prompt_display("", "Fix Findings"), "Fix Findings");
    let got = normalize_prompt_display(&"a".repeat(200), "x");
    assert_eq!(got.chars().count(), PROMPT_DISPLAY_MAX_LEN);
    assert!(got.ends_with('…'));
    assert_eq!(normalize_prompt_display(" a\r\n  b\rc ", ""), "a b c");
}

#[tokio::test(flavor = "multi_thread")]
async fn update_prompt_shebang_clean_and_conflict() {
    let prompt = chat_script_prompt("update.sh", "recur.chat.update", "Update", "");
    let (_g, repo) = git_repo();
    commit_file(&repo, "README.md", "hi\n", "init");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "feature.txt", "feat\n", "feature");
    git(&repo, &["checkout", "main"]);
    commit_file(&repo, "extra.txt", "base moved\n", "advance main");
    git(&repo, &["checkout", "feature"]);
    let rc = PromptRunContext {
        worktree: repo.clone(),
        work_dir: repo.clone(),
        bundle_support: chat_dir(),
        base_branch: "main".into(),
        update_base_method: "merge".into(),
        ..Default::default()
    };
    let bad = PromptRunContext {
        base_branch: "no-such-branch".into(),
        ..rc.clone()
    };
    assert!(
        run_prompt_shebang(&shebang_ctx(), &prompt, bad)
            .await
            .is_err()
    );
    let res = run_prompt_shebang(&shebang_ctx(), &prompt, rc.clone())
        .await
        .unwrap();
    assert_eq!(res.prompt, "");
    assert!(
        res.display.contains("Updated with latest"),
        "{}",
        res.display
    );
    let out = Proc::new("git")
        .args(["-C", &repo, "show", "HEAD:extra.txt"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "base moved");

    git(&repo, &["checkout", "main"]);
    commit_file(&repo, "README.md", "main edit\n", "main readme");
    git(&repo, &["checkout", "feature"]);
    commit_file(&repo, "README.md", "feature edit\n", "feature readme");
    let res = run_prompt_shebang(&shebang_ctx(), &prompt, rc)
        .await
        .unwrap();
    assert!(res.prompt.contains("conflict"), "{}", res.prompt);
    let merge_head = Proc::new("git")
        .args(["-C", &repo, "rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .output()
        .unwrap();
    assert!(merge_head.status.success(), "should leave MERGE_HEAD");
}

#[tokio::test(flavor = "multi_thread")]
async fn re_review_local_pr_embeds_only_the_delta_since_the_last_pass() {
    let prompt = chat_script_prompt(
        "re-review.sh",
        "recur.chat.re-review",
        "Re-review",
        "re-review",
    );
    let (_g, repo) = git_repo();
    commit_file(&repo, "README.md", "hi\n", "init");
    let out = Proc::new("git")
        .args(["-C", &repo, "rev-parse", "HEAD"])
        .output()
        .unwrap();
    let since = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    commit_file(&repo, "fix.txt", "fixed\n", "fix");
    let rc = PromptRunContext {
        worktree: repo.clone(),
        work_dir: repo.clone(),
        bundle_support: chat_dir(),
        base_branch: "main".into(),
        surface: "local_pr".into(),
        pr_number: 7,
        review_since: since,
        review_now: "HEAD".into(),
        ..Default::default()
    };
    let res = run_prompt_shebang(&shebang_ctx(), &prompt, rc)
        .await
        .unwrap();
    assert!(
        res.prompt
            .contains("only what changed since your last review pass"),
        "{}",
        res.prompt
    );
    assert!(
        res.prompt.contains("+fixed") && !res.prompt.contains("+hi"),
        "{}",
        res.prompt
    );
    assert_eq!(
        res.display,
        "Please re-review Local PR #7 since the last pass"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn update_prompt_shebang_dirty_worktree_defers_to_agent() {
    let prompt = chat_script_prompt("update.sh", "recur.chat.update", "Update", "");
    let (_g, repo) = git_repo();
    commit_file(&repo, "README.md", "hi\n", "init");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "feature.txt", "feat\n", "feature");
    git(&repo, &["checkout", "main"]);
    commit_file(&repo, "README.md", "main edit\n", "advance main");
    git(&repo, &["checkout", "feature"]);
    std::fs::write(format!("{repo}/README.md"), "local dirty\n").unwrap();
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            worktree: repo.clone(),
            work_dir: repo.clone(),
            bundle_support: chat_dir(),
            base_branch: "main".into(),
            update_base_method: "merge".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(res.prompt.contains("overwritten"), "{}", res.prompt);
    assert!(res.display.contains("Finish updating"), "{}", res.display);
    let merge_head = Proc::new("git")
        .args(["-C", &repo, "rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .output()
        .unwrap();
    assert!(!merge_head.status.success());
    assert_eq!(
        std::fs::read_to_string(format!("{repo}/README.md"))
            .unwrap()
            .trim(),
        "local dirty"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn open_pr_prompt_shebang_cases() {
    let prompt = chat_script_prompt("open-pr.sh", "recur.chat.open-pr", "Open PR", "");
    let (_g, repo) = git_repo();
    commit_file(&repo, "README.md", "hi\n", "init");
    git(&repo, &["checkout", "-b", "feature"]);
    commit_file(&repo, "feature.txt", "feat\n", "feature");
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            worktree: repo.clone(),
            work_dir: repo.clone(),
            bundle_support: chat_dir(),
            base_branch: "main".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(res.display, "Open a pull request");
    for want in [
        "mcp__recur__open_pr",
        "Do not run `gh pr create` yourself",
        "feature.txt",
        "Current branch: feature",
        "Compare base: main",
    ] {
        assert!(res.prompt.contains(want), "missing {want:?}");
    }
    assert!(!res.prompt.contains("Opened by Recur") && !res.prompt.contains("draft set to true"));

    let (_g1, w1) = temp_dir();
    let (_g2, w2) = temp_dir();
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            worktree: w1,
            work_dir: w2,
            bundle_support: chat_dir(),
            pr_id: "gh-12".into(),
            pr_number: 12,
            pr_source: "gh".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (res.prompt.as_str(), res.display.as_str()),
        ("", "PR #12 is already open")
    );

    let (_g3, local) = git_repo();
    commit_file(&local, "f.txt", "x\n", "init");
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            worktree: local.clone(),
            work_dir: local.clone(),
            bundle_support: chat_dir(),
            pr_id: "7".into(),
            pr_number: 7,
            pr_source: "local".into(),
            base_branch: "main".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!res.prompt.is_empty());
    assert!(!res.display.contains("already open"));
    assert!(!res.prompt.contains("Conventional Commits"));

    let draft = chat_script_prompt(
        "open-pr.sh",
        "recur.chat.open-draft-pr",
        "Open draft PR",
        "open-draft-pr",
    );
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &draft,
        PromptRunContext {
            worktree: local.clone(),
            work_dir: local,
            bundle_support: chat_dir(),
            base_branch: "main".into(),
            conventional_commits: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(res.display, "Open a draft pull request");
    for want in [
        "draft pull request",
        "draft set to true",
        "mcp__recur__open_pr",
        "Conventional Commits",
    ] {
        assert!(res.prompt.contains(want), "missing {want:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fix_checks_without_pr_or_on_host() {
    let prompt = chat_script_prompt(
        "fix-checks.sh",
        "recur.chat.fix-checks",
        "Fix checks",
        "fix-checks",
    );
    let (_g1, a) = temp_dir();
    let (_g2, b) = temp_dir();
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            worktree: a.clone(),
            work_dir: b.clone(),
            bundle_support: chat_dir(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (res.prompt.as_str(), res.display.as_str()),
        ("", "No GitHub pull request on this workspace")
    );
    let res = run_prompt_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            worktree: a,
            work_dir: b,
            bundle_support: chat_dir(),
            pr_number: 12,
            pr_source: "gh".into(),
            pr_host: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (res.prompt.as_str(), res.display.as_str()),
        (
            "",
            "Create a workspace from this pull request to edit files"
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn address_review_without_pr_or_on_host() {
    let prompt = chat_script_prompt(
        "address-review.sh",
        "recur.chat.address-review",
        "Address review",
        "address-review",
    );
    let (_g1, a) = temp_dir();
    let (_g2, b) = temp_dir();
    let rc = PromptRunContext {
        worktree: a,
        work_dir: b,
        bundle_support: chat_dir(),
        ..Default::default()
    };
    for (rc, want) in [
        (rc.clone(), "No GitHub pull request on this workspace"),
        (
            PromptRunContext {
                pr_number: 12,
                pr_source: "gh".into(),
                pr_host: true,
                ..rc.clone()
            },
            "Create a workspace from this pull request to edit files",
        ),
        (
            PromptRunContext {
                pr_number: 12,
                pr_source: "gh".into(),
                ..rc
            },
            "No GitHub pull request URL on this workspace",
        ),
    ] {
        let res = run_prompt_shebang(&shebang_ctx(), &prompt, rc)
            .await
            .unwrap();
        assert_eq!((res.prompt.as_str(), res.display.as_str()), ("", want));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fix_checks_complete_without_pr() {
    let dir = chat_dir();
    let prompt = PromptAction {
        id: "recur.chat.fix-checks".into(),
        name: "Fix checks".into(),
        slash: "fix-checks".into(),
        complete_interpreter: INTERPRETER_SHEBANG.into(),
        complete_body: std::fs::read_to_string(format!("{dir}/src/complete-checks.sh")).unwrap(),
        bundle_dir: dir,
        ..Default::default()
    };
    let (_g1, a) = temp_dir();
    let (_g2, b) = temp_dir();
    let res = run_prompt_complete_shebang(
        &shebang_ctx(),
        &prompt,
        PromptRunContext {
            worktree: a,
            work_dir: b,
            bundle_support: chat_dir(),
            complete_arg: "check".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(res.message, "No GitHub pull request on this workspace");
    assert_eq!(res.hits.len(), 1);
    assert_eq!(res.hits[0].insert, "all");
}
