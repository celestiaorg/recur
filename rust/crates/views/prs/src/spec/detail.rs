//! PR detail's Conversation findings, Files changed filters, Commits and the review modal's
//! typeaheads, from `pr-review.spec.ts`.

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply};
use recur_view_chat::suggest::SuggestKind;
use serde_json::{Value, json};

use super::harness::*;

const RACE: &str = "token refresh can race across two tabs";
const ABORT: &str = "refresh ignores abort";
const SESSION: &str = "src/hooks/useOAuthSession.ts";

fn pending_ids(s: &mut S) -> Vec<String> {
    s.pr("482").pending_comments.iter().map(|p| p.id.clone()).collect()
}

fn pending_bodies(s: &mut S) -> Vec<String> {
    s.pr("482").pending_comments.iter().map(|p| p.body.clone()).collect()
}

#[gpui::test]
fn opens_a_pr_shows_the_diff_and_opens_the_review_modal_from_files(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.tab("Files changed");
    assert!(s.has("cfile:live:src/auth/oauth.ts"));
    assert!(s.has("hline:src/auth/oauth.ts:add:right:12"));
    s.click("pr-act:pr-review=Approve");
    assert!(s.has("pr-rev-modal=Approve pull request"));
    assert!(s.has("sel:Review type=Approve"));
    assert!(s.has("pr-rev-go=Approve"));
}

#[gpui::test]
fn github_only_findings_post_as_pending_review_comments(cx: &mut TestAppContext) {
    let mut abort = race_finding("");
    abort["id"] = json!("f-abort");
    abort["line"] = json!(8);
    abort["message"] = json!(ABORT);
    abort["severity"] = json!("medium");
    let mut race = race_finding("");
    race.as_object_mut().unwrap().remove("workspaceId");
    abort.as_object_mut().unwrap().remove("workspaceId");
    let f = Findings::new(vec![race, abort]);
    let mut s = boot(cx, |sd| patch_pr(sd, "482", json!({ "workspaceId": "" })), f.options(0));
    s.open_pr("482");
    s.tab("Conversation");
    assert!(s.has("card:Reviewer findings"));
    assert!(s.has("button:Post all"));
    assert!(!s.has("open-in-findings:f-race"));

    s.click(&format!("button:Post as GitHub comment:{RACE}"));
    assert!(s.has("pr-act:pending=1 pending"));
    assert!(!s.has(&format!("finding-row:{RACE}")));
    assert!(s.has(&format!("finding-row:{ABORT}")));

    s.tab("Files changed");
    assert!(s.has("fl-head:Post all"));
    s.click(&format!("fl-row:file:{SESSION}"));
    let id = pending_ids(&mut s).pop().expect("pending");
    assert!(s.has(&format!("icmt-pending:{id}")));
    s.click(&format!("pending-remove:{id}"));
    assert!(!s.has(&format!("icmt-pending:{id}")));

    s.tab("Conversation");
    assert!(s.has(&format!("finding-row:{RACE}")));
    s.click("button:Post all");
    assert!(s.has("pr-act:pending=2 pending"));
    assert!(!s.has("card:Reviewer findings"));

    s.tab("Files changed");
    s.click(&format!("fl-row:file:{SESSION}"));
    let bodies = pending_bodies(&mut s);
    assert!(bodies.iter().any(|b| b.contains(RACE)), "{bodies:?}");
    assert!(bodies.iter().any(|b| b.contains(ABORT)), "{bodies:?}");
    for id in pending_ids(&mut s) {
        assert!(s.has(&format!("icmt-pending:{id}")), "{id}");
    }
}

#[gpui::test]
fn a_matching_submitted_comment_does_not_hide_or_auto_resolve_a_finding(cx: &mut TestAppContext) {
    let f = Findings::new(vec![race_finding("WS-1")]);
    let mut s = boot(
        cx,
        |sd| patch_pr(sd, "482", json!({ "comments": comments_with_race_note("gh-inline-pre") })),
        f.options(0),
    );
    s.open_pr("482");
    assert!(s.has("card:Reviewer findings"));
    assert!(s.has(&format!("finding-row:{RACE}")));
    assert_eq!(f.settled(), 0);
}

fn persisted_draft() -> Value {
    json!({
        "pendingComments": [{
            "id": "pend-finding-race", "file": SESSION, "line": 2, "side": "RIGHT",
            "body": RACE, "findingId": "f-race",
        }],
    })
}

#[gpui::test]
fn a_persisted_finding_draft_resolves_after_submit(cx: &mut TestAppContext) {
    let f = Findings::new(vec![race_finding("LOOP-2231")]);
    let mut s = boot(cx, |sd| patch_pr(sd, "482", persisted_draft()), f.options(0));
    s.open_pr("482");
    assert!(s.has("pr-act:pending=1 pending"));
    assert!(!s.has("card:Reviewer findings"));
    s.click("pr-act:pending=1 pending");
    assert!(s.has("pr-rev-modal=Approve pull request"));
    s.click("pr-rev-go=Approve");
    assert!(!s.has("v-pr-review"));
    assert!(s.until(|_| f.settled() == 1));
    assert!(!s.has("card:Reviewer findings"));
}

#[gpui::test]
fn deleting_a_finding_draft_does_not_resolve_from_an_older_matching_comment(cx: &mut TestAppContext) {
    let f = Findings::new(vec![race_finding("LOOP-2231")]);
    let mut s = boot(
        cx,
        |sd| patch_pr(sd, "482", json!({ "comments": comments_with_race_note("gh-inline-pre") })),
        f.options(0),
    );
    s.open_pr("482");
    s.click(&format!("button:Post as GitHub comment:{RACE}"));
    assert!(s.has("pr-act:pending=1 pending"));
    s.tab("Files changed");
    s.click(&format!("fl-row:file:{SESSION}"));
    let id = pending_ids(&mut s).pop().expect("pending");
    s.click(&format!("pending-remove:{id}"));
    s.tab("Conversation");
    assert!(s.has(&format!("finding-row:{RACE}")));
    assert!(s.has(&format!("button:Post as GitHub comment:{RACE}")));
    assert_eq!(f.settled(), 0);
}

#[gpui::test]
fn a_failed_settle_offers_resolve_without_posting_again(cx: &mut TestAppContext) {
    let f = Findings::new(vec![race_finding("LOOP-2231")]);
    let mut s = boot(cx, |sd| patch_pr(sd, "482", persisted_draft()), f.options(1));
    s.open_pr("482");
    assert!(s.has("pr-act:pending=1 pending"));
    s.click("pr-act:pending=1 pending");
    s.click("pr-rev-go=Approve");
    assert!(!s.has("v-pr-review"));
    assert!(s.until(|_| f.settled() == 1));
    assert!(s.has(&format!("finding-row:{RACE}")));
    assert!(!s.has(&format!("button:Post as GitHub comment:{RACE}")));
    s.click(&format!("button:Resolve finding:{RACE}"));
    assert!(s.until(|_| f.settled() == 2));
    assert!(!s.has("card:Reviewer findings"));
}

#[gpui::test]
fn a_posted_finding_still_settles_after_the_pr_merges(cx: &mut TestAppContext) {
    let f = Findings::new(vec![race_finding("LOOP-2231")]);
    let mut s = boot(
        cx,
        |sd| {
            patch_pr(
                sd,
                "482",
                json!({
                    "state": "Merged", "group": "merged",
                    "pendingComments": [{
                        "id": "pend-seed", "file": SESSION, "line": 8, "side": "RIGHT",
                        "body": "seed", "findingId": "f-race",
                    }],
                    "comments": comments_with_race_note("gh-posted"),
                }),
            )
        },
        f.options(0),
    );
    s.open_pr("482");
    assert!(s.until(|_| f.settled() == 1));
    assert!(!s.has("card:Reviewer findings"));
}

#[gpui::test]
fn files_changed_filters_do_not_carry_over_to_another_pr(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        let files = sd.pr_by_id("482").unwrap()["files"].clone();
        patch_pr(sd, "479", json!({ "files": files, "fileCount": 3 }));
    });
    let paths = ["src/auth/oauth.ts", SESSION, "src/checkout/SignInButtons.tsx"];
    let rows = |s: &mut S| paths.iter().filter(|p| s.has(&format!("fl-row:file:{p}"))).count();
    s.open_pr("482");
    s.tab("Files changed");
    s.click("button:Show files with unresolved comments:false");
    assert_eq!(rows(&mut s), 1);
    assert!(s.has("button:Show all files:true"));

    s.prs_nav();
    s.click("prrow:479");
    s.tab("Files changed");
    assert!(!s.has("empty:No files match"));
    assert_eq!(rows(&mut s), 3);
    assert!(!s.has("button:Show all files:true"));
}

const FIRST_COMMIT: &str = "Add tests for Apple + Google providers";
const LAST_COMMIT: &str = "Add Apple & Google OAuth providers";

fn opens_first_commit(s: &mut S) {
    s.open_pr("482");
    s.tab("Commits");
    let first = s.bounds(&format!("commit:{FIRST_COMMIT}")).expect("first");
    let last = s.bounds(&format!("commit:{LAST_COMMIT}")).expect("last");
    assert!(first.origin.y < last.origin.y);
    s.click(&format!("commit:{FIRST_COMMIT}"));
    assert!(s.has("commit-hd"));
}

#[gpui::test]
fn commit_detail_shows_the_per_commit_diff(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    opens_first_commit(&mut s);
    let commit = s.pr("482").commits.unwrap().into_iter().find(|c| c.message == FIRST_COMMIT).unwrap();
    let path = commit.files.unwrap()[0].path.clone();
    assert!(s.has(&format!("cfile:live:{path}")));
}

#[gpui::test]
fn commit_detail_loads_a_lazy_per_commit_diff(cx: &mut TestAppContext) {
    let original = recur_services::mock::fixtures::seed().pr_by_id("482").unwrap()["commits"].clone();
    let mut stripped = original.clone();
    for c in stripped.as_array_mut().unwrap() {
        c["files"] = json!([]);
    }
    let by_hash = original.clone();
    let mut s = boot(
        cx,
        move |sd| patch_pr(sd, "482", json!({ "commits": stripped })),
        MockOptions::default().resolver("GetCommitDiff", move |a, _, _| {
            let r = a.get(1).and_then(Value::as_str).unwrap_or("").to_string();
            let hit = by_hash
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["hash"] == r.as_str() || c["sha"] == r.as_str())
                .cloned()
                .unwrap_or(json!({}));
            Reply::value(hit)
        }),
    );
    opens_first_commit(&mut s);
    let commit = recur_services::mock::fixtures::seed().pr_by_id("482").unwrap()["commits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["message"] == FIRST_COMMIT)
        .cloned()
        .unwrap();
    let path = commit["files"][0]["path"].as_str().unwrap().to_string();
    assert!(s.until(|s| s.has(&format!("cfile:live:{path}"))));
}

fn open_review_modal(s: &mut S) {
    s.open_pr("482");
    s.click("pr-act:pr-review=Approve");
    assert!(s.has("pr-rev-modal=Approve pull request"));
    s.click("pr-rev-body");
}

#[gpui::test]
fn tab_inserts_the_selected_emoji_in_the_review_modal(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_review_modal(&mut s);
    s.type_text(":");
    assert!(s.has("emoji-suggest-row:0"));
    s.keys("tab");
    assert_eq!(s.review_draft(), "👍");
    assert!(!s.has("emoji-suggest-row:0"));
    assert!(s.has("pr-rev-go=Approve"));
    assert!(s.has("v-pr-review"));
}

#[gpui::test]
fn tab_inserts_a_github_user_mention_in_the_review_modal(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_review_modal(&mut s);
    s.type_text("@");
    assert!(s.has("user-mention-row:0"));
    let who = s.suggestions(SuggestKind::Mention);
    assert!(who.iter().any(|l| l == "loopbot"), "{who:?}");
    assert!(who.iter().any(|l| l == "avery"), "{who:?}");
    s.keys("tab");
    assert_eq!(s.review_draft(), "@avery ");
    assert!(!s.has("user-mention-row:0"));
}

#[gpui::test]
fn tab_inserts_a_mentionable_github_user_from_repository_search(cx: &mut TestAppContext) {
    let mut s = boot(
        cx,
        |_| {},
        MockOptions::default().resolver("SearchPRMentionableUsers", |_, _, _| {
            Reply::value(json!([{ "login": "octocat", "name": "The Octocat" }]))
        }),
    );
    open_review_modal(&mut s);
    s.type_text("@octo");
    assert!(s.until(|s| s.suggestions(SuggestKind::Mention).iter().any(|l| l == "octocat")));
    s.keys("tab");
    assert_eq!(s.review_draft(), "@octocat ");
    assert!(!s.has("user-mention-row:0"));
}

#[gpui::test]
fn request_changes_from_the_titlebar_split_on_any_tab(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.tab("Files changed");
    s.click("pr-review-chev");
    s.click("pr-review-request");
    assert!(s.has("pr-rev-modal=Request changes"));
    assert!(s.has("sel:Review type=Request changes"));
    s.click("pr-rev-body");
    let body = "Please add tests for the secrets init path.";
    s.type_text(body);
    s.click("pr-rev-go=Request changes");
    assert!(!s.has("v-pr-review"));
    s.tab("Conversation");
    let comments = s.pr("482").comments.unwrap_or_default();
    assert!(comments.iter().any(|c| c.body_md.contains(body)), "the review body joins the thread");
    assert!(s.has("thread"));
}
