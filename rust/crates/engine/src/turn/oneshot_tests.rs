//! Go `commit_message_test.go`, plus the one-shot request shape.

use std::time::Duration;

use recur_agent::TurnResult;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::testing::TestEngine;
use crate::turn::test_support::{Capture, FnProvider};

fn text_result(text: &'static str) -> std::sync::Arc<dyn recur_agent::Provider> {
    FnProvider::wrap(move |_| {
        Ok(TurnResult {
            subtype: "success".into(),
            text: text.into(),
            ..Default::default()
        })
    })
}

#[tokio::test]
async fn generate_pr_description() {
    let te = TestEngine::new(Some(text_result(
        "Fix the overlay\n\nKeep the sidebar from covering the composer on short viewports.",
    )));
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy();
    let tok = CancellationToken::new();
    let (title, body) = te
        .engine
        .generate_pr_description(
            &tok,
            "ws",
            &wt,
            "diff --git a/x b/x",
            &["fix overlay".into()],
        )
        .await
        .unwrap();
    assert_eq!(title, "Fix the overlay");
    assert_eq!(
        body,
        "Keep the sidebar from covering the composer on short viewports."
    );
    assert!(
        te.engine
            .generate_pr_description(&tok, "ws", &wt, "  \n", &[])
            .await
            .is_err()
    );
    te.close().await;
}

#[tokio::test]
async fn commit_message_turn_shape() {
    let cap = Capture::empty();
    let te = TestEngine::new(Some(cap.clone()));
    let tok = CancellationToken::new();
    let err = te
        .engine
        .generate_commit_message(
            &tok,
            "ws",
            "/tmp",
            "diff --git a/x b/x\n+y",
            &["feat: prior".into()],
        )
        .await
        .unwrap_err();
    assert_eq!(err.0, "empty commit message from model");
    let req = &cap.requests()[0];
    assert_eq!(req.system_prompt, COMMIT_MESSAGE_SYSTEM_PROMPT);
    assert_eq!(req.tools.as_deref(), Some(""));
    assert_eq!(req.max_turns, 1);
    assert!(req.skip_thinking && !req.include_partials);
    assert_eq!(req.permission_mode, "dontAsk");
    assert_eq!(req.flags.get("effort").map(String::as_str), Some("low"));
    assert!(req.allowed_tools.is_none(), "Claude can disable tools");
    assert!(req.user_mcp.is_empty(), "one-shots get no Settings → MCP");
    assert!(
        te.store.session_for_workspace_kind("ws", "").is_none(),
        "no role: no mirror"
    );
    assert_eq!(
        te.engine
            .generate_commit_message(&tok, "ws", "/tmp", " ", &[])
            .await
            .unwrap_err()
            .0,
        "stage changes to generate a commit message"
    );
    te.close().await;
}

#[tokio::test]
async fn oneshot_deadline_reads_context_deadline_exceeded() {
    let te = TestEngine::new(Some(FnProvider::wrap(|_| {
        Err(recur_agent::ProviderError::canceled())
    })));
    // A parent cancel stays a cancel.
    let tok = CancellationToken::new();
    tok.cancel();
    let e = te
        .engine
        .run_oneshot(
            &tok,
            "ws",
            "/tmp",
            "sys",
            "p".into(),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    assert!(e.is_canceled());
    te.close().await;
}

#[test]
fn sanitize_commit_message_cases() {
    for (input, want) in [
        ("", ""),
        ("  fix: add sparkle button  ", "fix: add sparkle button"),
        ("\"feat: scm AI message\"", "feat: scm AI message"),
        ("```\nfix: fences\n```", "fix: fences"),
        (
            "```text\nfeat: tagged fence\n\nBody line.\n```",
            "feat: tagged fence\n\nBody line.",
        ),
        (
            "fix: subject\n\nExplain why in a short body.",
            "fix: subject\n\nExplain why in a short body.",
        ),
    ] {
        assert_eq!(
            sanitize_commit_message(input, COMMIT_MESSAGE_MAX_LEN),
            want,
            "{input:?}"
        );
    }
}

#[test]
fn commit_prompt_caps_diff() {
    let big = "x".repeat(COMMIT_MESSAGE_DIFF_CAP + 100);
    let prompt = build_commit_message_prompt(&big, &["feat: prior".into()]);
    for part in [
        "Recent commit subjects",
        "feat: prior",
        "Staged diff:",
        "[diff truncated]",
    ] {
        assert!(prompt.contains(part), "missing {part}");
    }
    assert!(prompt.len() <= COMMIT_MESSAGE_DIFF_CAP + 400);
}

#[test]
fn pr_prompt_covers_every_file() {
    let lock = format!(
        "diff --git a/package-lock.json b/package-lock.json\n{}",
        "x".repeat(COMMIT_MESSAGE_DIFF_CAP)
    );
    let src = "diff --git a/src/fix.go b/src/fix.go\n+fixed the overlay\n";
    let prompt = build_pr_description_prompt(&format!("{lock}\n{src}"), &["squash: misc".into()]);
    for part in ["package-lock.json", "src/fix.go", "+fixed the overlay"] {
        assert!(prompt.contains(part), "missing {part}");
    }
    assert!(prompt.contains("[truncated]"));
}

#[test]
fn pr_reply_parsing() {
    assert_eq!(
        parse_pr_description("```markdown\nTitle\n\nBody\n```"),
        ("Title".into(), "Body".into())
    );
    assert_eq!(
        parse_pr_description("\"Quoted\""),
        ("Quoted".into(), String::new())
    );
    assert_eq!(parse_pr_description("  "), (String::new(), String::new()));
    let long = "t".repeat(100);
    assert_eq!(parse_pr_description(&long).0.chars().count(), 80);
    let body = "b".repeat(GITHUB_PR_BODY_MAX + 5);
    let (_, b) = parse_pr_description(&format!("T\n{body}"));
    assert!(b.ends_with("\n\n…(truncated)…"));
}

#[test]
fn loop_commit_message_cases() {
    let linear_goal = format!(
        "Work on Linear issue ACME-114 in this workspace. Implement what the issue asks; do not expand scope.\n\nIdentifier: ACME-114\nTitle: PR-C: client-side acme.dev API prechecks and SVG validation\n\n## Description\nCatch predictable API rejections inside Apollo before spending a doomed round-trip.\n{}",
        "more ticket prose. ".repeat(80)
    );
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    type Case<'a> = (
        &'a str,
        String,
        String,
        Vec<String>,
        Vec<&'a str>,
        Vec<&'a str>,
    );
    let cases: Vec<Case> = vec![
        (
            "done summary wins",
            "Add holdings precheck before sell_token".into(),
            "ACME-114 API prechecks".into(),
            s(&["apollo/src/api.ts"]),
            vec!["Add holdings precheck before sell_token"],
            vec!["api.ts", "ACME-114 API prechecks", "recur(", "iteration"],
        ),
        (
            "done is first line only",
            "Wire SVG validation\n\nAlso touched three other files and restated the ticket.".into(),
            linear_goal.clone(),
            vec![],
            vec!["Wire SVG validation"],
            vec!["Also touched", "Description", "recur(", "iteration"],
        ),
        (
            "linear dump becomes title plus files",
            String::new(),
            linear_goal.clone(),
            s(&["apollo/src/api.ts", "apollo/src/svg.ts"]),
            vec!["ACME-114:", "api.ts", "svg.ts"],
            vec![
                "Work on Linear",
                "Description",
                "doomed round-trip",
                "recur(",
                "iteration",
            ],
        ),
        (
            "short ticket plus files",
            String::new(),
            "ACME-114 API prechecks".into(),
            s(&["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts"]),
            vec!["ACME-114 API prechecks (a.ts, b.ts, c.ts +1)"],
            vec!["recur(", "iteration"],
        ),
        (
            "long hint drops instead of chopping the headline",
            String::new(),
            "The human already wrote this change. Review the current worktree, then fix it.".into(),
            s(&[
                "ChatTab.tsx",
                "Composer.tsx",
                "mission-agent-chat.test.tsx",
                "mission-home.test.tsx",
            ]),
            vec!["The human already wrote this change"],
            vec!["ChatTab.tsx"],
        ),
        (
            "plain short goal",
            String::new(),
            "add dark mode".into(),
            vec![],
            vec!["add dark mode"],
            vec!["recur(", "iteration"],
        ),
        (
            "empty falls back",
            String::new(),
            String::new(),
            vec![],
            vec!["changes"],
            vec!["recur(", "iteration"],
        ),
    ];
    for (name, done, goal, files, want, not) in cases {
        let got = loop_commit_message(&done, &goal, &files);
        assert!(got.len() <= 80, "{name}: {got}");
        assert!(!got.contains('\n'), "{name}: {got}");
        for p in want {
            assert!(got.contains(p), "{name}: {got} missing {p}");
        }
        for p in not {
            assert!(!got.contains(p), "{name}: {got} has {p}");
        }
    }
}
