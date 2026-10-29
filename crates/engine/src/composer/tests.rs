//! Go `raw_regions_test.go` and `composer_test.go`.

use std::path::Path;

use tokio_util::sync::CancellationToken;

use super::*;
use crate::testing::TestEngine;

fn write(p: &Path, body: &str) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, body).unwrap();
}

#[test]
fn dialect_skips_raw_regions() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("README.md"), "hi");
    let r = root.path().to_string_lossy();
    let input =
        "please read @README.md\n\n⟦raw⟧\nsee @README.md and /go-code-review in the diff\n⟦/raw⟧\n";
    let got = wrap_composer_dialect(input, &r, None);
    assert!(!got.contains("⟦raw⟧") && !got.contains("⟦/raw⟧") && !got.contains("⟦rawph:"));
    assert!(got.contains("<system_instruction>"));
    assert_eq!(got.matches(" as @README.md").count(), 1);
    assert!(got.contains("see @README.md and /go-code-review in the diff"));
    assert!(got.contains("please read @README.md"));
}

#[test]
fn dialect_nonce_ignores_quoted_bare_markers() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("app.go"), "x");
    let r = root.path().to_string_lossy();
    let input = "intro @app.go\n⟦raw:a1b2⟧\ndocs say use `⟦raw⟧`…`⟦/raw⟧` around blobs\nand also @app.go /go-code-review inside the diff\n⟦/raw:a1b2⟧\n";
    let got = wrap_composer_dialect(input, &r, None);
    assert!(
        !got.contains("⟦raw:a1b2⟧") && !got.contains("⟦/raw:a1b2⟧") && !got.contains("⟦rawph:")
    );
    assert_eq!(got.matches(" as @app.go").count(), 1);
    assert!(got.contains("docs say use `⟦raw⟧`…`⟦/raw⟧` around blobs"));
    assert!(got.contains("and also @app.go /go-code-review inside the diff"));
}

#[test]
fn dialect_strips_stray_markers() {
    assert_eq!(
        wrap_composer_dialect("hello ⟦/raw⟧ world ⟦raw:dead⟧", "", None),
        "hello  world "
    );
}

#[test]
fn dialect_plain_round_trips() {
    let root = tempfile::tempdir().unwrap();
    let plain = "hello with no mentions";
    assert_eq!(
        wrap_composer_dialect(plain, &root.path().to_string_lossy(), None),
        plain
    );
}

#[test]
fn dialect_does_not_wrap_bare_artifact() {
    let root = tempfile::tempdir().unwrap();
    let input = "check @artifact please";
    assert_eq!(
        wrap_composer_dialect(input, &root.path().to_string_lossy(), None),
        input
    );
}

#[test]
fn unclosed_raw_makes_the_rest_raw() {
    let (masked, regions) = raw::mask_raw_regions("a ⟦raw⟧ @x /y");
    assert_eq!(masked, "a ⟦rawph:0⟧");
    assert_eq!(regions, [" @x /y"]);
    assert_eq!(raw::unmask_raw_regions(&masked, &regions), "a  @x /y");
}

/// A shebang `[[prompt]]` pack under the engine's default bundles root.
fn write_prompt_bundle(te: &TestEngine, slash: &str, body: &str, id: &str) {
    let dir = Path::new(&te.engine.bundle_roots().default_dir).join("ChatTest");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    recur_exec::write_executable(dir.join("src").join("act.sh"), body, 0o755).unwrap();
    write(
        &dir.join("bundle.toml"),
        &format!(
            "[bundle]\nname = \"ChatTest\"\nid = \"recur.chattest\"\nversion = \"1.0.0\"\n\n[[prompt]]\nid = \"{id}\"\nname = \"Act\"\nslash = \"{slash}\"\ninterpreter = \"shebang\"\nscript = \"src/act.sh\"\n"
        ),
    );
}

fn opts(worktree: &str, ignore_disabled: bool) -> ExpandOpts {
    ExpandOpts {
        worktree: worktree.into(),
        ignore_disabled,
        ..ExpandOpts::default()
    }
}

#[tokio::test]
async fn expand_wraps_mentions() {
    let te = TestEngine::new(None);
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("README.md"), "hi");
    let got = te
        .engine
        .expand_composer(
            &CancellationToken::new(),
            "please read @README.md",
            opts(&root.path().to_string_lossy(), false),
        )
        .await
        .unwrap();
    assert!(got.prompt.contains("<system_instruction>"));
    assert!(!got.ran_action);
    assert_eq!(
        got.display, "please read @README.md",
        "wrapped wire keeps the composer"
    );
    te.close().await;
}

#[tokio::test]
async fn expand_runs_shebang_and_wraps() {
    let te = TestEngine::new(None);
    write_prompt_bundle(
        &te,
        "act",
        "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"wire @README.md\",\"display\":\"Act\"}'\n",
        "recur.chattest.act",
    );
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("README.md"), "hi");
    let got = te
        .engine
        .expand_composer(
            &CancellationToken::new(),
            "/act",
            opts(&root.path().to_string_lossy(), true),
        )
        .await
        .unwrap();
    assert!(got.ran_action);
    assert_eq!(got.display, "Act");
    assert!(got.prompt.contains("<system_instruction>"));
    assert!(got.prompt.contains("wire @README.md"));
    te.close().await;
}

#[tokio::test]
async fn expand_empty_action_errors() {
    let te = TestEngine::new(None);
    write_prompt_bundle(
        &te,
        "empty",
        "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"\",\"display\":\"Done\"}'\n",
        "recur.chattest.empty",
    );
    let e = te
        .engine
        .expand_composer(&CancellationToken::new(), "/empty", opts("", true))
        .await
        .unwrap_err();
    assert!(e.is_empty_prompt_action());
    assert!(e.result.ran_action);
    assert_eq!(e.result.display, "Done");
    te.close().await;
}

#[tokio::test]
async fn expand_honors_disabled_unless_ignored() {
    let te = TestEngine::new(None);
    write_prompt_bundle(
        &te,
        "off",
        "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE\",\"display\":\"Act\"}'\n",
        "recur.chattest.off",
    );
    let mut set = te.store.settings();
    set.disabled_bundle_prompts = vec!["recur.chattest.off".into()];
    te.store.set_settings(set);
    let tok = CancellationToken::new();
    let skipped = te
        .engine
        .expand_composer(&tok, "/off", opts("", false))
        .await
        .unwrap();
    assert!(!skipped.ran_action);
    assert_eq!(skipped.prompt, "/off");
    let ran = te
        .engine
        .expand_composer(&tok, "/off", opts("", true))
        .await
        .unwrap();
    assert!(ran.ran_action);
    assert_eq!(ran.prompt, "WIRE");
    te.close().await;
}

#[tokio::test]
async fn expand_trailing_text_does_not_fire() {
    let te = TestEngine::new(None);
    write_prompt_bundle(
        &te,
        "act",
        "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE\",\"display\":\"Act\"}'\n",
        "recur.chattest.act",
    );
    let got = te
        .engine
        .expand_composer(&CancellationToken::new(), "/act now", opts("", true))
        .await
        .unwrap();
    assert!(!got.ran_action);
    assert_eq!(got.prompt, "/act now");
    te.close().await;
}

#[test]
fn leading_slash() {
    assert_eq!(
        leading_prompt_slash("  /review  ").as_deref(),
        Some("review")
    );
    assert_eq!(leading_prompt_slash("/review extra"), None);
    assert_eq!(leading_prompt_slash("see /review"), None);
}

#[tokio::test]
async fn loop_mission_path_loads() {
    let te = TestEngine::new(None);
    let pack = Path::new(&te.engine.bundle_roots().default_dir).join("X");
    write(
        &pack.join("bundle.toml"),
        r#"[bundle]
name = "X"
id = "x.loop"
version = "1.0.0"

[[role]]
id = "x.roles.ma"
name = "Maestro"
code = "MA"
mode = "loop"
prompt = "hello"

[[loop]]
id = "x.loops.one"
name = "One"
conductor = "x.roles.ma"
prompt = "prompts/mission.md"
entry = "done"

[[loop.phase]]
id = "done"
uses = "artifact"
"#,
    );
    write(&pack.join("prompts").join("mission.md"), "Ship it.");
    let cat = te.engine.discover_loop_catalog().unwrap();
    let lp = recur_bundles::find_loop(&cat.loops, "x.loops.one").expect("loop");
    assert_eq!(
        lp.prompts.get("mission").map(|s| s.trim()),
        Some("Ship it.")
    );
    te.close().await;
}

#[tokio::test]
async fn expand_reports_fresh_session() {
    let te = TestEngine::new(None);
    write_prompt_bundle(
        &te,
        "fix-findings",
        "#!/bin/sh\nprintf '%s\\n' '{\"prompt\":\"WIRE\",\"display\":\"Act\"}'\n",
        "recur.chattest.fix-findings",
    );
    let toml = Path::new(&te.engine.bundle_roots().default_dir).join("ChatTest/bundle.toml");
    let mut body = std::fs::read_to_string(&toml).unwrap();
    body.push_str("\n[[prompt]]\nid = \"recur.chattest.re-review\"\nname = \"Re-review\"\nslash = \"re-review\"\nfresh_session = true\ninterpreter = \"shebang\"\nscript = \"src/act.sh\"\n");
    write(&toml, &body);
    let tok = CancellationToken::new();
    let got = te
        .engine
        .expand_composer(&tok, "/re-review", opts("", true))
        .await
        .unwrap();
    assert!(got.fresh_session);
    let fix = te
        .engine
        .expand_composer(&tok, "/fix-findings", opts("", true))
        .await
        .unwrap();
    assert!(!fix.fresh_session);
    te.close().await;
}
