//! PR detail's titlebar actions from `pr-review.spec.ts`: draft state, Approve on your own PR,
//! Update with base and its method menu, merge methods, the optimistic merge and its rollback.

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply};
use recur_ui::widgets::ToastKind;
use recur_view_core::ViewName;
use serde_json::json;

use super::harness::*;

const MERGED: &str = "pr-settled:Pull request successfully merged and closed";

fn writer(over: serde_json::Value) -> serde_json::Value {
    let mut v = json!({ "viewerCanWrite": true, "viewerLogin": "josh" });
    for (k, x) in over.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[gpui::test]
fn pr_list_labels_a_draft_as_draft_not_in_review(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| patch_pr(sd, "482", json!({ "draft": true, "mergeStateStatus": "DRAFT" })));
    s.prs_nav();
    s.seg("All");
    assert!(s.has("pr-checks:482=Draft"));
    let open = s.bounds("briefing-bucket:open").expect("open bucket");
    let row = s.bounds("prrow:482").expect("row");
    assert!(open.contains(&row.center()));
    if let Some(you) = s.bounds("briefing-bucket:you") {
        assert!(!you.contains(&row.center()));
    }
}

#[gpui::test]
fn converts_a_github_pr_to_a_draft_and_back(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.click("pr-act:pr-to-draft=Convert to draft");
    assert!(s.has("statepill=Draft"));
    assert!(!s.has("statepill=In review"));
    assert!(s.has("pr-act:pr-ready=Ready for review"));
    assert!(!s.has("pr-act:pr-review=Approve"));
    assert!(!s.has("pr-act:pr-merge=Merge"));

    s.prs_nav();
    s.seg("All");
    assert!(s.has("pr-checks:482=Draft"));
    s.click("prrow:482");
    assert!(s.has("pr-act:pr-ready=Ready for review"));
    assert!(!s.has("pr-act:pr-review=Approve"));
    assert!(!s.has("pr-act:pr-merge=Merge"));

    s.click("pr-act:pr-ready=Ready for review");
    assert!(!s.has("statepill=Draft"));
    assert!(s.has("statepill=In review"));
    assert!(s.has("pr-act:pr-to-draft=Convert to draft"));
    assert!(s.has("pr-act:pr-merge=Merge"));
}

#[gpui::test]
fn hides_approve_on_your_own_github_pr_but_still_offers_merge(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        patch_pr(
            sd,
            "482",
            json!({ "authorLogin": "avery", "viewerLogin": "avery", "viewerCanWrite": true, "mergeStateStatus": "CLEAN", "draft": false }),
        )
    });
    s.open_pr("482");
    assert!(!s.has("pr-act:pr-review=Approve"));
    assert!(!s.has("pr-act:pr-review=Request changes"));
    assert!(s.has("pr-act:pr-merge=Merge"));
}

#[gpui::test]
fn a_behind_github_pr_offers_update_with_main_instead_of_merge(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| patch_pr(sd, "482", writer(json!({ "mergeStateStatus": "BEHIND", "behindBase": 2 }))));
    s.open_pr("482");
    assert!(s.has("pr-act:pr-update=Update with main"));
    assert!(!s.has("pr-act:pr-merge=Merge"));
    assert!(!s.has("rework"));
}

#[gpui::test]
fn the_pr_update_split_can_switch_from_merge_to_rebase_onto_the_base(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| patch_pr(sd, "482", writer(json!({ "mergeStateStatus": "BEHIND", "behindBase": 2 }))));
    s.open_pr("482");
    s.click("pr-update-chev");
    assert!(s.has("pr-update-rebase"));
    // The menu paints over PR detail's header: the click lands on the item.
    s.click("pr-update-rebase");
    assert!(s.has("pr-act:pr-update=Rebase onto main"));
    assert!(!s.has("pr-update-rebase"));
}

#[gpui::test]
fn a_behind_draft_still_offers_update_with_main_next_to_ready_for_review(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        patch_pr(sd, "482", writer(json!({ "draft": true, "mergeStateStatus": "DRAFT", "behindBase": 2 })))
    });
    s.open_pr("482");
    assert!(s.has("pr-act:pr-ready=Ready for review"));
    assert!(s.has("pr-act:pr-update=Update with main"));
}

#[gpui::test]
fn a_github_pr_that_is_behind_locally_still_offers_merge_when_github_says_clean(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| patch_pr(sd, "482", writer(json!({ "mergeStateStatus": "CLEAN", "behindBase": 2 }))));
    s.open_pr("482");
    assert!(s.has("pr-act:pr-update=Update with main"));
    assert!(s.has("pr-act:pr-merge=Merge"));
}

#[gpui::test]
fn a_behind_github_pr_waiting_for_review_still_offers_update_with_main(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        patch_pr(
            sd,
            "482",
            writer(json!({ "mergeStateStatus": "BLOCKED", "behindBase": 2, "checksOk": true, "reviewDecision": "REVIEW_REQUIRED" })),
        )
    });
    s.open_pr("482");
    assert!(s.has("pr-act:pr-update=Update with main"));
    assert!(!s.has("pr-act:pr-merge=Merge"));
}

#[gpui::test]
fn a_repo_that_allows_squash_and_rebase_hides_merge_commit(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| patch_pr(sd, "482", writer(json!({ "allowedMergeMethods": ["squash", "rebase"] }))));
    s.open_pr("482");
    s.click("pr-merge-chev");
    assert!(s.has("pr-merge-squash"));
    assert!(s.has("pr-merge-rebase"));
    assert!(!s.has("pr-merge-merge"));
}

#[gpui::test]
fn a_repo_that_only_allows_squash_hides_the_other_merge_methods(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| patch_pr(sd, "482", writer(json!({ "allowedMergeMethods": ["squash"] }))));
    s.open_pr("482");
    assert!(s.has("pr-act:pr-merge=Squash and merge"));
    assert!(!s.has("pr-merge-chev"));
    assert!(!s.has("pr-merge-squash"));
}

fn clean(sd: &mut recur_services::mock::Seed) {
    patch_pr(sd, "482", writer(json!({ "mergeStateStatus": "CLEAN", "draft": false })));
}

#[gpui::test]
fn confirming_merge_paints_merged_before_github_returns(cx: &mut TestAppContext) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut s = boot(
        cx,
        clean,
        MockOptions::default()
            .resolver("MergePR", move |_, _, _| {
                let (reply, release) = Reply::deferred();
                let _ = tx.send(release);
                reply
            })
            .resolver("GetPR", |a, seed, _| {
                Reply::value(a.first().and_then(|id| id.as_str()).and_then(|id| seed.pr_by_id(id)).cloned().unwrap_or_default())
            }),
    );
    s.open_pr("482");
    s.click("pr-act:pr-merge=Merge");
    assert!(s.has("pr-act:pr-merge=Confirm merge"));
    s.click("pr-act:pr-merge=Confirm merge");
    assert!(s.has(MERGED));
    assert!(!s.has("pr-act:pr-merge=Merge"));

    s.tab("Commits");
    assert!(s.has("statepill=Merged"));
    assert!(!s.has("pr-act:pr-merge=Merge"));

    let mut row = recur_services::mock::fixtures::seed().pr_by_id("482").cloned().unwrap();
    row["state"] = json!("In review");
    row["group"] = json!("you");
    rx.try_recv().expect("merging").send(Reply::value(row));
    s.settle();
    assert!(s.has("statepill=Merged"));
    assert!(!s.has("pr-act:pr-merge=Merge"));
}

#[gpui::test]
fn a_failed_merge_restores_the_button_and_keeps_updates_from_the_round_trip(cx: &mut TestAppContext) {
    let (tx, rx) = std::sync::mpsc::channel();
    let merging = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let m2 = merging.clone();
    let mut s = boot(
        cx,
        clean,
        MockOptions::default()
            .resolver("MergePR", move |_, _, _| {
                merging.store(true, std::sync::atomic::Ordering::SeqCst);
                let (reply, release) = Reply::deferred();
                let _ = tx.send(release);
                reply
            })
            .resolver("GetPR", move |a, seed, _| {
                let Some(mut pr) = a.first().and_then(|id| id.as_str()).and_then(|id| seed.pr_by_id(id)).cloned() else {
                    return Reply::null();
                };
                if m2.load(std::sync::atomic::Ordering::SeqCst) {
                    let list = pr["comments"].as_array_mut().unwrap();
                    if !list.iter().any(|c| c["id"] == "during-merge") {
                        list.push(json!({
                            "id": "during-merge", "kind": "comment", "author": "sam",
                            "bodyMd": "landed while merging", "bodyHtml": "landed while merging", "time": "now",
                        }));
                    }
                }
                Reply::value(pr)
            }),
    );
    s.open_pr("482");
    s.click("pr-act:pr-merge=Merge");
    s.click("pr-act:pr-merge=Confirm merge");
    assert!(s.has(MERGED));

    s.tab("Commits");
    s.tab("Conversation");
    let has_landed = |s: &mut S| s.pr("482").comments.unwrap_or_default().iter().any(|c| c.id == "during-merge");
    assert!(s.until(has_landed));
    assert!(s.has("cmt:during-merge:sam landed while merging"));

    rx.try_recv().expect("merging").send(Reply::error("protected branch"));
    s.settle();
    assert!(s.until(|s| s.toasted(ToastKind::Error, "protected branch")));
    assert!(s.until(|s| s.has("pr-act:pr-merge=Merge")));
    assert!(!s.has(MERGED));
    assert!(s.has("cmt:during-merge:sam landed while merging"));
    assert_eq!(s.view(), ViewName::PrDetail);
}
