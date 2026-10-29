//! PR detail's checks: Fix checks, live durations, rollups, collapsed
//! groups, timeline commit rows, the composer and merged PRs.

use std::sync::Arc;

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Reply};
use serde_json::{Value, json};

use super::harness::*;
use crate::detail::PrDetail;

const CHECKS: &str = "https://github.com/acme/app/pull/482/checks";

/// What `SendWorkspaceMessage` received: (text, display).
type Sent = Arc<Mutex<Vec<(String, String)>>>;

fn sender(opts: MockOptions) -> (MockOptions, Sent) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let s2 = sent.clone();
    let opts = opts
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
        .resolver("WorkspaceChatItems", |_, _, _| Reply::value(json!([])))
        .resolver("SendWorkspaceMessage", move |a, _, _| {
            let text = |i: usize| a.get(i).and_then(Value::as_str).unwrap_or("").to_string();
            s2.lock().push((text(2), text(7)));
            Reply::value(json!("On it."))
        });
    (opts, sent)
}

#[gpui::test]
fn a_blocked_github_pr_with_failing_checks_offers_fix_checks_instead_of_merge(
    cx: &mut TestAppContext,
) {
    let (opts, sent) = sender(MockOptions::default());
    let mut s = boot(
        cx,
        |sd| {
            patch_pr(
                sd,
                "482",
                json!({
                    "mergeStateStatus": "BLOCKED", "checksOk": false, "reviewDecision": "APPROVED",
                    "viewerCanWrite": true, "viewerLogin": "josh",
                    "checks": [
                        {"name": "lint", "passed": false, "status": "fail"},
                        {"name": "build", "passed": true, "status": "pass"},
                    ],
                }),
            );
            add_idle_pr_workspace(sd, "482");
        },
        opts,
    );
    s.open_pr("482");
    assert!(s.has("pr-act:pr-fix-checks=Fix checks"));
    assert!(!s.has("pr-act:pr-merge=Merge"));
    s.click("pr-act:pr-fix-checks=Fix checks");
    assert!(s.has("prtab:Checks:on") || s.has("prtab:Checks:on:lit"));
    assert!(s.until(|_| sent.lock().len() == 1));
    let (text, display) = sent.lock()[0].clone();
    assert!(text.contains("gh run view --log-failed"), "{text}");
    assert_eq!(display, "Fix failing GitHub checks");
}

#[gpui::test]
fn failed_check_rows_offer_fix_which_focuses_that_check(cx: &mut TestAppContext) {
    let (opts, sent) = sender(MockOptions::default());
    let mut s = boot(
        cx,
        |sd| {
            patch_pr(
                sd,
                "482",
                json!({
                    "checksOk": false,
                    "checks": [
                        {"name": "lint", "passed": false, "status": "fail"},
                        {"name": "build", "passed": true, "status": "pass"},
                    ],
                }),
            );
            add_idle_pr_workspace(sd, "482");
        },
        opts,
    );
    s.open_pr("482");
    s.tab("Checks");
    assert!(s.has("checks-fix:Fix:lint"));
    assert!(s.has("checks-fix:Fix checks"));
    s.click("checks-fix:Fix:lint");
    assert!(s.until(|_| sent.lock().len() == 1));
    let (text, display) = sent.lock()[0].clone();
    assert!(text.contains("failing GitHub check lint"), "{text}");
    assert_eq!(display, "Fix failing check: lint");
}

#[gpui::test]
fn in_progress_check_duration_keeps_counting(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        let mut checks = sd.pr_by_id("482").unwrap()["checks"].clone();
        checks.as_array_mut().unwrap().insert(
            0,
            json!({"name": "build", "detail": "45s", "passed": false, "status": "pending", "startedAt": "2026-08-21T12:00:00Z"}),
        );
        patch_pr(sd, "482", json!({ "checks": checks, "checksOk": false }));
    });
    let at = chrono::DateTime::parse_from_rfc3339("2026-08-21T12:00:45Z")
        .unwrap()
        .to_utc();
    s.cx.update(|_, cx| PrDetail::set_clock(at, cx));
    s.open_pr("482");
    s.tab("Checks");
    assert!(s.has("checkrow:build=45s:spin"));
    assert!(s.has(&format!("check-cn-a:build={CHECKS}")));
    assert!(s.has(&format!("checks-h-link=Checks in progress={CHECKS}")));
    s.wait(5000);
    assert!(s.has("checkrow:build=50s:spin"));
}

fn rollup(cx: &mut TestAppContext, checks: Option<Value>, kind: &str, heading: &str, spin: bool) {
    let mut s = plain_with(cx, move |sd| {
        if let Some(c) = checks {
            patch_pr(sd, "482", json!({ "checksOk": false, "checks": c }));
        }
    });
    s.open_pr("482");
    assert!(s.has(&format!("prtab-check={kind}")));
    assert!(s.has(&format!("checks-h-ico={kind}")));
    assert!(
        s.has(&format!("checks-h-link={heading}={CHECKS}")),
        "{heading}"
    );
    if spin {
        assert!(s.has("checkrow:build=:spin"));
    }
}

#[gpui::test]
fn checks_tab_and_card_header_show_pass(cx: &mut TestAppContext) {
    rollup(cx, None, "pass", "All checks passed", false);
}

#[gpui::test]
fn checks_tab_and_card_header_show_fail(cx: &mut TestAppContext) {
    let c = json!([
        {"name": "lint", "passed": false, "status": "fail"},
        {"name": "build", "passed": false, "status": "pending"},
    ]);
    rollup(cx, Some(c), "fail", "Some checks failed", false);
}

#[gpui::test]
fn checks_tab_and_card_header_show_pending(cx: &mut TestAppContext) {
    let c = json!([{"name": "build", "passed": false, "status": "pending"}]);
    rollup(cx, Some(c), "pending", "Checks in progress", true);
}

#[gpui::test]
fn conversation_collapses_successful_checks_the_checks_tab_leaves_them_open(
    cx: &mut TestAppContext,
) {
    let mut s = plain_with(cx, |sd| {
        patch_pr(
            sd,
            "482",
            json!({
                "checksOk": false,
                "checks": [
                    {"name": "lint", "passed": false, "status": "fail"},
                    {"name": "test", "passed": true, "status": "pass"},
                    {"name": "deploy", "passed": false, "status": "skipped"},
                ],
            }),
        )
    });
    s.open_pr("482");
    assert!(s.has("checkrow:lint="));
    assert!(s.has("checks-group-h:successful:collapsed"));
    assert!(s.has("checks-group-h:skipped:collapsed"));
    assert!(!s.has("checkrow:test="));
    assert!(!s.has("checkrow:deploy="));
    s.tab("Checks");
    assert!(s.has("checks-group-h:successful"));
    assert!(s.has("checks-group-h:skipped"));
    assert!(s.has("checkrow:test="));
    assert!(s.has("checkrow:deploy="));
}

#[gpui::test]
fn conversation_commit_shas_stay_aligned_when_some_rows_have_no_checks(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        let mut comments = json!([
            {
                "id": "gh-commit-nocheck", "kind": "action", "author": "loopbot",
                "bodyMd": "Upgrade bridge-sdk version to 0.3.0", "time": "95d",
                "commitSha": "f8dc8a4aaaaaaa", "verified": true,
                "url": "https://github.com/acme/app/commit/f8dc8a4aaaaaaa",
            },
            {
                "id": "gh-commit-pending", "kind": "action", "author": "loopbot",
                "bodyMd": "Merge branch main", "time": "1d", "commitSha": "162ae9daaaaaaa",
                "verified": true, "checkStatus": "pending",
                "url": "https://github.com/acme/app/commit/162ae9daaaaaaa",
            },
        ]);
        let existing = sd.pr_by_id("482").unwrap()["comments"].clone();
        comments
            .as_array_mut()
            .unwrap()
            .extend(existing.as_array().unwrap().iter().cloned());
        patch_pr(sd, "482", json!({ "comments": comments }));
    });
    s.open_pr("482");
    assert!(s.has("tl-commit:gh-commit-nocheck"));
    assert!(s.has("tl-commit:gh-commit-pending"));
    assert!(s.has("tl-ck:empty"));
    assert!(s.has("tl-ck:icon"));
    let a = s.bounds("tl-sha:f8dc8a4").expect("sha");
    let b = s.bounds("tl-sha:162ae9d").expect("sha");
    assert!((f32::from(a.origin.x) - f32::from(b.origin.x)).abs() < 1.);
}

#[gpui::test]
fn posts_a_conversation_comment_without_submitting_a_review(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.click("composer-textarea");
    let body = "Just a note on the approach — not a formal review.";
    s.type_text(body);
    s.click("composer-go:Comment");
    let comments = s.pr("482").comments.unwrap_or_default();
    let c = comments
        .iter()
        .find(|c| c.body_md == body)
        .expect("comment");
    assert!(c.verb != "approved" && c.verb != "changes_requested");
    assert!(s.has("composer-go:Comment"));
    assert!(s.cx.update(|_, cx| crate::ui::ui(cx).read(cx).composer_draft.is_empty()));
}

#[gpui::test]
fn merged_prs_hide_commenting_and_conversation_checks(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        let files = sd.pr_by_id("482").unwrap()["files"].clone();
        patch_pr(
            sd,
            "471",
            json!({
                "viewerCanWrite": true, "viewerLogin": "avery", "authorLogin": "avery",
                "comments": [
                    {"id": "gh-description", "kind": "description", "author": "avery", "bodyMd": "Adds idempotency keys.", "time": "1d"},
                    {"id": "gh-event-merged", "kind": "action", "author": "mycodecrafting", "bodyMd": "merged commit a3f73e2 into main", "commitSha": "a3f73e2abcdef", "time": "5m"},
                    {"id": "gh-event-deleted", "kind": "action", "author": "mycodecrafting", "bodyMd": "deleted the recur/idempotency branch", "time": "5m"},
                ],
                "checks": [{"name": "test", "detail": "12s", "passed": true, "status": "pass"}],
                "files": files,
            }),
        );
    });
    s.open_pr("471");
    assert!(s.has("pr-settled:Pull request successfully merged and closed"));
    assert!(!s.has("composer"));
    assert!(!s.has("checks-box-body"));
    assert!(s.has("tl-event:gh-event-merged:mycodecrafting merged commit a3f73e2 into main"));
    assert!(s.has("tl-sha-a:a3f73e2=https://github.com/acme/app/commit/a3f73e2abcdef"));
    assert!(s.has("tl-event:gh-event-deleted:mycodecrafting deleted the recur/idempotency branch"));
    assert!(!s.has("pr-act:pr-review=Approve"));
    assert!(!s.has("cmt-act:Edit title"));
    s.click("cmt-more:gh-description");
    assert!(!s.has("menuitem:Edit"));
    s.keys("escape");

    s.tab("Files changed");
    let path = s.pr("471").files.unwrap()[0].path.clone();
    assert!(s.has(&format!("cfile:live:{path}")));
    assert!(!s.has(&format!("addc:{path}:right:1")));
    s.tab("Checks");
    assert!(s.has("checkrow:test=12s"));
}

fn scroll(s: &mut S, pane: &str, dy: f32) {
    let b = s
        .bounds(&format!("prpane:{pane}"))
        .or_else(|| s.bounds(&format!("prpane:{pane}:fade-top")))
        .unwrap();
    s.cx.simulate_event(gpui::ScrollWheelEvent {
        position: b.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(dy))),
        ..Default::default()
    });
    s.settle();
}

#[gpui::test]
fn tab_panes_fade_at_the_top_only_after_scroll(cx: &mut TestAppContext) {
    // A short window so every pane scrolls (the spec pads short panes instead).
    let mut s = boot_sized(cx, |_| {}, MockOptions::default(), 1380., 400.);
    s.open_pr("482");
    for (tab, pane) in [
        ("Conversation", "conv"),
        ("Commits", "commits"),
        ("Checks", "checks"),
    ] {
        s.tab(tab);
        assert!(s.has(&format!("prpane:{pane}")), "{pane} starts unfaded");
        scroll(&mut s, pane, -80.);
        assert!(
            s.has(&format!("prpane:{pane}:fade-top")),
            "{pane} fades once scrolled"
        );
        scroll(&mut s, pane, 400.);
        assert!(
            s.has(&format!("prpane:{pane}")),
            "{pane} unfades at the top"
        );
    }
    s.tab("Files changed");
    assert!(s.has("fl-body"));
    assert!(s.has("diffscroll"));
}
