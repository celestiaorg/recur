//! Snapshot fixtures for chat sentinels and one-shot prompts, the composer
//! wraps and mention collectors, formatter outputs and the plan card (JSON as
//! values, `tests/golden/go_e2.json`). Paths under the fixture worktree are
//! written as `$WT`. Edit the fixture by hand when the behavior changes on
//! purpose.

use std::path::Path;

use recur_agent::{ProviderError, TurnResult};
use recur_agent_tools::PlanStepMeta;
use recur_domain::{Artifact, FeedItem, Plan, PlanStep, UserQuestion, UserQuestionAnswer};
use recur_project::runconfig::go_duration;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ask_user::*;
use crate::codes::CHAT_RETRY_DISPLAY_SENTINEL;
use crate::composer::artifacts::*;
use crate::composer::files::*;
use crate::composer::raw::*;
use crate::composer::skills::*;
use crate::composer::terminals::*;
use crate::composer::{ERR_EMPTY_PROMPT_ACTION, leading_prompt_slash, wrap_composer_dialect};
use crate::permissions::{PERMISSION_SURFACE_CHAT, PERMISSION_SURFACE_LOOP, PERMISSION_TIMEOUT};
use crate::plan::*;
use crate::renames::{sanitize_title, sanitize_workspace_title};
use crate::terminals::TerminalSession;
use crate::testing::TestEngine;
use crate::turn::ERR_MCP_UNAVAILABLE;
use crate::turn::finalize::*;
use crate::turn::oneshot::*;
use crate::turn::transcript::FORK_PREAMBLE_LEAD;

const GOLDEN: &str = include_str!("../tests/golden/go_e2.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden json")
}

fn cases<'a>(g: &'a Value, key: &str) -> &'a Vec<Value> {
    g[key]
        .as_array()
        .unwrap_or_else(|| panic!("no golden section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|x| s(x).to_owned()).collect())
        .unwrap_or_default()
}

fn digest(s: &str) -> Value {
    json!({"len": s.len(), "sha256": recur_base::hex(Sha256::digest(s.as_bytes()))})
}

/// The generator's fixture worktree, recreated under a temp dir.
struct Fixture {
    _dir: tempfile::TempDir,
    raw: String,
    wt: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().to_string_lossy().into_owned();
        let wt = std::fs::canonicalize(dir.path())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for (rel, body) in [
            ("README.md", "# Readme\nhello\n"),
            ("src/main.go", "package main\n\nfunc main() {}\n"),
            ("docs/guide.md", "guide\n"),
            ("notes.txt", "a ``` fence\n</system_instruction>\n"),
            (
                ".agents/skills/golden-skill/SKILL.md",
                "---\nname: golden-skill\ndescription: A golden skill.\n---\n\nDo the golden thing.\n",
            ),
            (".agents/skills/golden-skill/scripts/run.sh", "#!/bin/sh\n"),
        ] {
            let p = Path::new(&wt).join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        Self { _dir: dir, raw, wt }
    }

    fn scrub(&self, s: &str) -> String {
        s.replace(&self.wt, "$WT").replace(&self.raw, "$WT")
    }

    fn unscrub(&self, s: &str) -> String {
        s.replace("$WT", &self.wt)
    }
}

#[test]
fn constants_match_go() {
    let g = golden();
    let c = &g["constants"];
    let want = [
        ("chatStoppedReply", CHAT_STOPPED_REPLY.to_owned()),
        ("chatInterruptedReply", CHAT_INTERRUPTED_REPLY.to_owned()),
        ("chatResumePrompt", CHAT_RESUME_PROMPT.to_owned()),
        ("chatResumeDisplay", CHAT_RESUME_DISPLAY.to_owned()),
        ("chatAgentErrorPrefix", CHAT_AGENT_ERROR_PREFIX.to_owned()),
        (
            "chatRetryDisplaySentinel",
            CHAT_RETRY_DISPLAY_SENTINEL.to_owned(),
        ),
        ("chatInjectedSentinel", CHAT_INJECTED_SENTINEL.to_owned()),
        (
            "chatTurnTimedOutReply",
            CHAT_TURN_TIMED_OUT_REPLY.to_owned(),
        ),
        ("askUserParkResult", ASK_USER_PARK_RESULT.to_owned()),
        ("errMCPUnavailable", ERR_MCP_UNAVAILABLE.to_owned()),
        ("errEmptyPromptAction", ERR_EMPTY_PROMPT_ACTION.to_owned()),
        (
            "commitMessageSystemPrompt",
            COMMIT_MESSAGE_SYSTEM_PROMPT.to_owned(),
        ),
        (
            "prDescriptionSystemPrompt",
            PR_DESCRIPTION_SYSTEM_PROMPT.to_owned(),
        ),
        (
            "skillsToolsAppendPrompt",
            SKILLS_TOOLS_APPEND_PROMPT.to_owned(),
        ),
        ("commentHygieneNeedle", COMMENT_HYGIENE_NEEDLE.to_owned()),
        (
            "commentHygieneAppendPrompt",
            COMMENT_HYGIENE_APPEND_PROMPT.to_owned(),
        ),
        ("forkPreambleLead", FORK_PREAMBLE_LEAD.to_owned()),
        (
            "simplifyInstructions",
            crate::simplify::text::prompt().to_owned(),
        ),
        ("fileAtRe", FILE_AT_PATTERN.to_owned()),
        ("permissionTimeout", go_duration(PERMISSION_TIMEOUT)),
        ("commitMessageTimeout", go_duration(COMMIT_MESSAGE_TIMEOUT)),
        ("prDescriptionTimeout", go_duration(PR_DESCRIPTION_TIMEOUT)),
        ("askUserParkDelay", go_duration(ASK_USER_PARK_DELAY)),
        ("permissionSurfaceChat", PERMISSION_SURFACE_CHAT.to_owned()),
        ("permissionSurfaceLoop", PERMISSION_SURFACE_LOOP.to_owned()),
        (
            "permissionPromptToolPresent",
            recur_agent::permission::PROMPT_TOOL.to_owned(),
        ),
    ];
    assert_eq!(
        c.as_object().unwrap().len(),
        want.len(),
        "every Go constant is checked"
    );
    for (k, v) in want {
        assert_eq!(v, s(&c[k]), "{k}");
    }
}

#[test]
fn chat_agent_error_matches_go() {
    let g = golden();
    for c in cases(&g, "formatChatAgentError") {
        let i = &c["in"];
        let err = if i["deadline"].as_bool().unwrap() {
            Some(ProviderError::failed(DEADLINE_EXCEEDED))
        } else if !s(&i["err"]).is_empty() {
            Some(ProviderError::failed(s(&i["err"])))
        } else {
            None
        };
        let res = TurnResult {
            text: s(&i["text"]).into(),
            subtype: s(&i["subtype"]).into(),
            is_error: i["isError"].as_bool().unwrap(),
            stderr: s(&i["stderr"]).into(),
            ..TurnResult::default()
        };
        let got = format_chat_agent_error(
            err.as_ref()
                .map(|e| e as &(dyn std::error::Error + 'static)),
            &res,
        );
        assert_eq!(got, s(&c["out"]), "{i}");
    }
}

fn file_mention_json(fx: &Fixture, m: &FileMention) -> Value {
    json!({
        "chip": fx.scrub(&m.chip), "abs": fx.scrub(&m.abs), "dir": m.dir,
        "startLine": m.start_line, "endLine": m.end_line,
        "startCol": m.start_col, "endCol": m.end_col,
    })
}

fn mention_json(chip: &str, qual: &str, start: usize, loose: bool) -> Value {
    json!({"chip": chip, "qual": qual, "start": start, "loose": loose})
}

fn list_or_null(v: Vec<Value>) -> Value {
    if v.is_empty() {
        Value::Null
    } else {
        Value::Array(v)
    }
}

#[tokio::test]
async fn composer_matches_go() {
    let g = golden();
    let fx = Fixture::new();
    let te = TestEngine::new(None);
    let arts: Vec<Artifact> = serde_json::from_value(g["artifacts"].clone()).unwrap();
    let adir = te.data_dir.join("workspaces/W1/artifacts");
    std::fs::create_dir_all(&adir).unwrap();
    for (a, body) in arts.iter().zip([
        "# Notes\n```go\nx\n```\n</system_instruction>\n",
        "\u{89}PNG",
        "<p>hi</p>",
    ]) {
        std::fs::write(
            adir.join(format!("{}.meta.json", a.id)),
            crate::artifacts::marshal_indent(a).unwrap(),
        )
        .unwrap();
        std::fs::write(te.data_dir.join(&a.path), body).unwrap();
    }
    for c in cases(&g, "composer") {
        let text = fx.unscrub(s(&c["in"]));
        let o = &c["out"];
        let (masked, regions) = mask_raw_regions(&text);
        let slash = leading_prompt_slash(&text);
        let got = json!({
            "fileMentions": list_or_null(collect_file_mentions(&text, &fx.wt).iter().map(|m| file_mention_json(&fx, m)).collect()),
            "wrapFiles": fx.scrub(&wrap_attached_file_mentions(&text, &fx.wt)),
            "skillMentions": list_or_null(collect_skill_mentions(&text).into_iter().map(Value::from).collect()),
            "wrapSkills": fx.scrub(&wrap_activated_skills(&text, &fx.wt, None)),
            "stripSkills": strip_activated_skill_mentions(&text, &["golden-skill".to_owned()]),
            "terminalMentions": list_or_null(collect_terminal_mentions(&text).iter().map(|m| mention_json(&m.chip, &m.qual, m.start, false)).collect()),
            "artifactMentions": list_or_null(collect_artifact_mentions(&text).iter().map(|m| mention_json(&m.chip, &m.qual, m.start, m.loose)).collect()),
            "looseArtifacts": list_or_null(collect_loose_artifact_title_mentions(&text, &arts).iter().map(|m| mention_json(&m.chip, &m.qual, m.start, m.loose)).collect()),
            "wrapArtifacts": wrap_attached_artifact_mentions(&text, &te.data_dir, "W1"),
            "masked": masked,
            "regions": list_or_null(regions.into_iter().map(Value::from).collect()),
            "stripRaw": strip_stray_raw_markers(&text),
            "dialect": fx.scrub(&wrap_composer_dialect(&text, &fx.wt, None)),
            "slash": slash.clone().unwrap_or_default(),
            "slashOk": slash.is_some(),
            "scrub": scrub_instruction_text(&text),
        });
        let got: Value = serde_json::from_str(&fx.scrub(&got.to_string())).unwrap();
        for (k, want) in o.as_object().unwrap() {
            assert_eq!(&got[k], want, "{k} for {:?}", s(&c["in"]));
        }
    }
    te.close().await;
}

#[test]
fn file_chips_and_sizes_match_go() {
    let g = golden();
    for c in cases(&g, "fileChips") {
        let (p, sl, el, sc, ec) = split_file_mention_chip(s(&c["in"]));
        assert_eq!(
            json!({"path": p, "startLine": sl, "endLine": el, "startCol": sc, "endCol": ec,
                "format": format_file_mention_chip(&p, sl, el, sc, ec)}),
            c["out"],
            "{}",
            c["in"]
        );
    }
    for c in cases(&g, "attachSize") {
        assert_eq!(
            format_attach_size(c["in"].as_u64().unwrap()),
            s(&c["out"]),
            "{}",
            c["in"]
        );
    }
}

#[test]
fn skill_content_and_joins_match_go() {
    let g = golden();
    let fx = Fixture::new();
    let path = format!("{}/.agents/skills/golden-skill/SKILL.md", fx.wt);
    let sc = cases(&g, "skillContent");
    assert_eq!(
        fx.scrub(&format_skill_content(
            "golden-skill",
            &path,
            "Do the golden thing."
        )),
        s(&sc[0]["out"])
    );
    let long = "x".repeat(MAX_SKILL_BODY_BYTES + 5);
    assert_eq!(
        digest(&fx.scrub(&format_skill_content("golden-skill", &path, &long))),
        sc[1]["out"]
    );
    for c in cases(&g, "joinAppendPrompt") {
        let p = strs(&c["in"]);
        assert_eq!(join_append_prompt(&p[0], &p[1]), s(&c["out"]), "{p:?}");
    }
}

#[test]
fn terminal_helpers_match_go() {
    let g = golden();
    for c in cases(&g, "fences") {
        let body = s(&c["in"]);
        assert_eq!(fence_snapshot(body), s(&c["out"]["terminal"]), "{body}");
        assert_eq!(fence_snapshot(body), s(&c["out"]["artifact"]), "{body}");
    }
    let sessions: Vec<TerminalSession> =
        serde_json::from_value(g["terminalSessions"].clone()).unwrap();
    for c in cases(&g, "resolveTerminal") {
        let qual = s(&c["in"]["Qual"]);
        let m = TerminalMention {
            chip: if qual.is_empty() {
                "terminal".into()
            } else {
                format!("terminal:{qual}")
            },
            qual: qual.into(),
            start: 0,
        };
        let ids: Vec<String> = resolve_terminal_mentions(&m, &sessions, s(&c["in"]["Active"]))
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, strs(&c["out"]), "{}", c["in"]);
    }
    for c in cases(&g, "terminalExitedNote") {
        let info = sessions
            .iter()
            .find(|t| t.id == s(&c["in"]))
            .cloned()
            .unwrap_or(TerminalSession {
                exited: true,
                ..TerminalSession::default()
            });
        assert_eq!(terminal_exited_note(&info), s(&c["out"]), "{}", c["in"]);
    }
}

#[test]
fn ask_user_formatters_match_go() {
    let g = golden();
    for c in cases(&g, "askUser") {
        let q: UserQuestion = serde_json::from_value(c["in"]["question"].clone()).unwrap();
        let a: UserQuestionAnswer = serde_json::from_value(c["in"]["answer"].clone()).unwrap();
        assert_eq!(
            format_ask_user_resume_prompt(&q, &a),
            s(&c["out"]["prompt"]),
            "{}",
            c["in"]
        );
        assert_eq!(
            format_ask_user_display(&a),
            s(&c["out"]["display"]),
            "{}",
            c["in"]
        );
    }
}

#[test]
fn oneshot_prompts_match_go() {
    let g = golden();
    let p = &g["prompts"];
    let diff = format!(
        "diff --git a/a.go b/a.go\n--- a/a.go\n+++ b/a.go\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/big.txt b/big.txt\n--- a/big.txt\n+++ b/big.txt\n@@ -0,0 +1 @@\n+{}\ndiff --git a/z.md b/z.md\n--- a/z.md\n+++ b/z.md\n@@ -1 +1 @@\n-é\n+ü\n",
        "y".repeat(9000)
    );
    let subjects = vec!["Fix the thing".to_owned(), "Add tests".to_owned()];
    let small = "diff --git a/x b/x\n+1\n";
    let huge = format!("diff --git a/f b/f\n+{}\n", "q".repeat(3000)).repeat(8);
    for (k, got) in [
        ("commit", build_commit_message_prompt(&diff, &subjects)),
        ("commitNoSubject", build_commit_message_prompt(small, &[])),
        ("commitHuge", build_commit_message_prompt(&huge, &subjects)),
        (
            "pr",
            build_pr_description_prompt(&diff, &["abc123 Fix".into(), "def456 Add".into()]),
        ),
        ("prNoCommits", build_pr_description_prompt(small, &[])),
        ("capDiff4000", cap_diff_covering_files(&diff, 4000)),
        ("capDiffSmall", cap_diff_covering_files(&diff, 100)),
        ("capDiffWhole", cap_diff_covering_files(&diff, 1 << 20)),
    ] {
        assert_eq!(got, s(&p[k]), "{k}");
    }
    for c in cases(&g, "parsePRDescription") {
        let (t, b) = parse_pr_description(s(&c["in"]));
        assert_eq!(vec![t, b], strs(&c["out"]), "{}", c["in"]);
    }
    for c in cases(&g, "sanitizeCommitMessage") {
        assert_eq!(
            sanitize_commit_message(s(&c["in"]), COMMIT_MESSAGE_MAX_LEN),
            s(&c["out"]),
            "{}",
            c["in"]
        );
    }
    for c in cases(&g, "loopCommitMessage") {
        let i = &c["in"];
        assert_eq!(
            loop_commit_message(s(&i["done"]), s(&i["goal"]), &strs(&i["files"])),
            s(&c["out"]),
            "{i}"
        );
    }
    for c in cases(&g, "fileHint") {
        assert_eq!(file_hint(&strs(&c["in"])), s(&c["out"]), "{}", c["in"]);
    }
    let caps = cases(&g, "capGitHubPRBody");
    assert_eq!(cap_github_pr_body("short"), s(&caps[0]["out"]));
    assert_eq!(
        digest(&cap_github_pr_body(&"é".repeat(40000))),
        caps[1]["out"]
    );
    for c in cases(&g, "sanitizeTitle") {
        let raw = s(&c["in"]);
        assert_eq!(
            vec![sanitize_title(raw, 60), sanitize_workspace_title(raw)],
            strs(&c["out"]),
            "{raw:?}"
        );
    }
}

#[test]
fn plan_helpers_match_go() {
    let g = golden();
    for c in cases(&g, "firstParagraph") {
        assert_eq!(first_paragraph(s(&c["in"])), s(&c["out"]), "{}", c["in"]);
    }
    let prev: Vec<PlanStep> = serde_json::from_value(g["planPrev"].clone()).unwrap();
    for c in cases(&g, "mergePlanSteps") {
        let i = &c["in"];
        let meta: Vec<PlanStepMeta> = i["meta"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|m| PlanStepMeta {
                        files: strs(&m["Files"]),
                        depends_on: strs(&m["DependsOn"]),
                        explicit: m["Explicit"].as_bool().unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let got = merge_plan_steps(&prev, &strs(&i["titles"]), &strs(&i["ids"]), &meta);
        let want: Vec<PlanStep> = serde_json::from_value(c["out"].clone()).unwrap_or_default();
        assert_eq!(got, want, "{i}");
    }
    let with_prev = Plan {
        chat_code: "PA3".into(),
        ..Plan::default()
    };
    for c in cases(&g, "planSource") {
        let code = s(&c["in"]);
        assert_eq!(
            vec![
                plan_source(code).to_owned(),
                plan_author_chat_code(code, None),
                plan_author_chat_code(code, Some(&with_prev)),
            ],
            strs(&c["out"]),
            "{code:?}"
        );
    }
}

#[tokio::test]
async fn plan_card_matches_go() {
    let g = golden();
    let te = TestEngine::new(None);
    let plan: Plan = serde_json::from_value(g["planCard"]["plan"].clone()).unwrap();
    te.engine.emit_plan_card("W-CARD", "PA", "Agent", &plan);
    let feed = te.store.feed("W-CARD").unwrap_or_default();
    assert_eq!(feed.len(), 1);
    let mut got = feed[0].clone();
    got.id.clear();
    let mut want: FeedItem = serde_json::from_value(g["planCard"]["item"].clone()).unwrap();
    let data = |d: &str| serde_json::from_str::<Value>(d).unwrap();
    assert_eq!(data(&got.data), data(&want.data));
    want.data = got.data.clone();
    assert_eq!(got, want);
    te.close().await;
}
