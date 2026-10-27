//! PR detail's Conversation thread from `pr-review.spec.ts`: inline comments reviewed from the
//! titlebar, edits, quote replies, deletes, nested review notes, reactions and review events.

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply};
use recur_ui::widgets::ToastKind;
use serde_json::json;

use super::harness::*;
use crate::detail::spec_handle::reply_text;

const OAUTH: &str = "src/auth/oauth.ts";

fn composer_draft(s: &mut S) -> String {
    s.cx.update(|_, cx| crate::ui::ui(cx).read(cx).composer_draft.clone())
}

fn comment_body(s: &mut S, id: &str) -> Option<String> {
    s.pr("482")
        .comments
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.id == id)
        .map(|c| c.body_md)
}

/// Replace the focused field's text, as `fill` does.
fn fill(s: &mut S, text: &str) {
    s.keys("secondary-a");
    s.type_text(text);
}

#[gpui::test]
fn leaves_an_inline_comment_and_lists_it_pending_in_the_review_modal(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.tab("Files changed");
    s.click(&format!("addc:{OAUTH}:right:11"));
    assert!(s.has("finding-note-input"));
    let body = "E2E: verify the secrets client initialises before providers.";
    s.type_text(body);
    s.click("compose-add");
    let pending = s.pr("482").pending_comments;
    assert_eq!(pending.iter().filter(|p| p.body == body).count(), 1);
    assert!(s.has("pr-act:pending=1 pending"));

    s.click("pr-act:pending=1 pending");
    assert!(s.has("pr-rev-pending"));
    assert!(s.has(&format!("pr-rev-chip-loc={OAUTH}:11")));
    s.click("pr-rev-x");
    assert!(!s.has("v-pr-review"));
    s.tab("Conversation");
    assert!(!s.has("composer-pending"));
    assert!(s.has("composer-go:Comment"));
}

#[gpui::test]
fn a_dragged_multiline_comment_lists_its_span_in_the_review_modal(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.tab("Files changed");
    s.reveal_within(&format!("addc:{OAUTH}:right:13"), "diffscroll");
    s.drag(
        &format!("addc:{OAUTH}:right:13"),
        &format!("addc:{OAUTH}:right:15"),
    );
    assert!(s.has("finding-note-input"));
    s.type_text("E2E: this span should stay together");
    s.click("compose-add");
    s.click("pr-act:pending=1 pending");
    assert!(s.has(&format!("pr-rev-chip-loc={OAUTH}:13-15")));
}

#[gpui::test]
fn long_inline_code_in_a_pr_description_and_comment_does_not_widen_the_pane(
    cx: &mut TestAppContext,
) {
    let token = "localStorage.setItem('eden-onboarding-storage',JSON.stringify({state:{hasCompletedOnboarding:true,hasSkippedOnboarding:false,hasStartedOnboarding:true,currentStep:0,onboardingVersion:\"v1\"},version:0}))";
    let mut s = plain_with(cx, |sd| {
        let mut comments = sd.pr_by_id("482").unwrap()["comments"].clone();
        for c in comments.as_array_mut().unwrap() {
            if c["kind"] == "description" {
                c["bodyMd"] = json!(format!(
                    "1. Bypass onboarding: `{token}`\n2. Navigate to `/mint`."
                ));
            }
            if c["id"] == "c1" {
                c["bodyHtml"] = json!("");
                c["bodyMd"] = json!(format!("Please run `{token}` locally."));
            }
        }
        patch_pr(sd, "482", json!({ "comments": comments }));
    });
    s.open_pr("482");
    let pane = s.bounds("prpane:conv").expect("pane");
    for sel in ["cmt-desc", "cm-md:c1"] {
        let b = s.bounds(sel).unwrap_or_else(|| panic!("{sel}"));
        assert!(f32::from(b.size.width) <= 840. + 1., "{sel} {b:?}");
        assert!(b.right() <= pane.right(), "{sel} {b:?}");
    }
}

#[gpui::test]
fn edits_the_github_pr_title_and_an_owned_comment_and_quote_replies(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.click("cmt-act:Edit title");
    fill(&mut s, "OAuth secrets — retitled");
    s.click("prh-title-save");
    assert!(s.has("prh-title=OAuth secrets — retitled"));

    s.click("cmt-more:gh-description");
    s.click("menuitem:Edit");
    fill(&mut s, "Retitled description for checkout OAuth.");
    s.keys("secondary-enter");
    assert_eq!(
        comment_body(&mut s, "gh-description").as_deref(),
        Some("Retitled description for checkout OAuth.")
    );

    s.click("cmt-more:c1");
    s.click("menuitem:Edit");
    fill(&mut s, "Edited: confirm secrets.init() still runs first.");
    s.keys("secondary-enter");
    assert_eq!(
        comment_body(&mut s, "c1").as_deref(),
        Some("Edited: confirm secrets.init() still runs first.")
    );

    s.click("cmt-more:c1");
    s.click("menuitem:Quote reply");
    assert!(composer_draft(&mut s).starts_with("> Edited: confirm secrets.init"));

    s.click("cmt-more:c2");
    assert!(s.has("menuitem:Delete"));
    s.click("menuitem:Quote reply");
    assert!(composer_draft(&mut s).contains("> Got the review notes"));

    s.click("cmt-more:c1");
    assert!(s.has("menuitem:Delete"));
    s.click("menuitem:Delete");
    assert!(comment_body(&mut s, "c1").is_none());

    s.click("cmt-more:gh-review-99");
    assert!(s.has("menuitem:Edit"));
    assert!(!s.has("menuitem:Delete"));
    s.click("cmt-more:gh-review-99");
    assert!(!s.has("cmt-more-menu"));

    s.click("cmt-more:gh-description");
    assert!(s.has("menuitem:Open in GitHub"));
    s.click("menuitem:Copy link");
    assert!(s.toasted(ToastKind::Info, "Copied link"));
}

#[gpui::test]
fn nests_line_comments_under_the_review_that_submitted_them(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    assert!(s.has("cmt-kid:gh-inline-1"));
    assert!(s.has(&format!("cmt-ctx-path={OAUTH}:11-12")));
    assert!(s.has("cmt-ctx-diff"));
    s.click("cmt-ctx-fill");
    assert!(!s.has("cmt-ctx-diff"));
    s.click("cmt-ctx-toggle:collapsed");
    assert!(s.has("cmt-ctx-diff"));
    assert!(s.has("cmt-reply-ph:gh-inline-1"));
    s.click("cmt-resolve:Resolve conversation");
    assert!(s.has("cmt-resolved-bar:gh-inline-1"));
    assert!(!s.has("cmt:gh-inline-1:Avery Rourke hmm"));
    s.click("cmt-resolve:Unresolve");
    assert!(s.has("cmt-reply-ph:gh-inline-1"));
}

#[gpui::test]
fn replies_to_and_deletes_a_line_note_in_conversation(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.click("cmt-reply-ph:gh-inline-1");
    s.type_text("thread reply from e2e");
    s.click("finding-note-btn-primary:Reply");
    let reply = s
        .pr("482")
        .comments
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.body_md == "thread reply from e2e")
        .expect("reply");
    assert!(s.until(|s| s.has(&format!("cmt-kid-note:{}:thread reply from e2e", reply.id))));

    let id = s
        .pr("482")
        .comments
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.body_md == "thread reply from e2e")
        .unwrap()
        .id;
    s.click(&format!("cmt-more:{id}"));
    s.click("menuitem:Quote reply");
    let quoted = s.cx.update(|_, cx| reply_text(cx));
    assert!(quoted.starts_with("> thread reply from e2e"), "{quoted:?}");
    s.click("finding-note-btn:Cancel");

    s.click("cmt-more:gh-inline-1");
    s.click("menuitem:Delete");
    assert!(comment_body(&mut s, "gh-inline-1").is_none());
    assert!(!s.has("cmt-kid-note:gh-inline-1:hmm"));
}

#[gpui::test]
fn closes_the_conversation_reply_composer_before_a_slow_github_write_returns(
    cx: &mut TestAppContext,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut s = boot(
        cx,
        |_| {},
        MockOptions::default().resolver("ReplyPRComment", move |_, _, _| {
            let (reply, release) = Reply::deferred();
            let _ = tx.send(release);
            reply
        }),
    );
    s.open_pr("482");
    s.click("cmt-reply-ph:gh-inline-1");
    s.type_text("slow thread reply");
    s.click("finding-note-btn-primary:Reply");
    assert!(s.has("cmt-reply-ph:gh-inline-1"));
    assert!(!s.has("finding-note-btn-primary:Saving…"));
    let pending = s
        .pr("482")
        .comments
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.body_md == "slow thread reply")
        .expect("optimistic reply");
    assert!(s.has(&format!("cmt-kid-note:{}:slow thread reply", pending.id)));
    rx.try_recv().expect("in flight").send(Reply::value(json!({
        "id": "gh-inline-slow-reply", "code": "YO", "author": "You", "time": "now", "bodyHtml": "",
        "bodyMd": "slow thread reply", "verb": "commented", "kind": "comment", "inReplyToId": "gh-inline-1",
    })));
    s.settle();
    assert!(s.has("cmt-kid-note:gh-inline-slow-reply:slow thread reply"));
}

#[gpui::test]
fn submits_a_review_with_pending_line_notes_as_one_conversation_unit(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.tab("Files changed");
    s.click(&format!("addc:{OAUTH}:right:11"));
    s.type_text("group me under the review");
    s.click("compose-add");
    s.click("pr-act:pr-review=Approve");
    assert!(s.has("pr-rev-pending"));
    s.click("pr-rev-body");
    s.type_text("review wrap");
    s.click("pr-rev-go=Approve");
    assert!(!s.has("v-pr-review"));

    s.tab("Conversation");
    let comments = s.pr("482").comments.unwrap_or_default();
    let unit = comments
        .iter()
        .find(|c| c.body_md == "review wrap")
        .expect("review");
    let kid = comments
        .iter()
        .find(|c| c.body_md == "group me under the review")
        .expect("line note");
    assert_eq!(kid.parent_id, unit.id);
    assert!(s.has(&format!("cmt-kid:{}", kid.id)));
    assert!(!s.has(&format!(
        "cmt:{}:{} group me under the review",
        kid.id, kid.author
    )));
}

#[gpui::test]
fn shows_and_toggles_reactions_on_conversation_items(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    assert!(s.has("rxn-pill:gh-description:hooray"));
    assert!(s.has("rxn-pill:c1:+1"));
    s.click("rxn-pill:c1:heart");
    assert!(!s.has("rxn-pill:c1:heart"));
    s.click("rxn-add:c1");
    s.click("rxn-pick:rocket");
    assert!(s.has("rxn-pill:c1:rocket"));
    assert!(s.has("rxn-pill:gh-inline-1:eyes"));

    s.tab("Files changed");
    assert!(s.has("rxn-pill:gh-inline-1:eyes"));
    s.reveal_within("rxn-add:gh-inline-1", "diffscroll");
    s.click("rxn-add:gh-inline-1");
    s.click("rxn-pick:+1");
    assert!(s.has("rxn-pill:gh-inline-1:+1"));
}

#[gpui::test]
fn review_events_stand_out_and_have_no_reaction_picker(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        let mut comments = sd.pr_by_id("482").unwrap()["comments"].clone();
        let list = comments.as_array_mut().unwrap();
        list.push(json!({ "id": "gh-event-fp", "kind": "action", "author": "jcstein", "bodyMd": "force-pushed the branch", "time": "29d" }));
        list.push(json!({ "id": "gh-review-action-1", "kind": "action", "author": "Ferret-san", "verb": "approved", "bodyMd": "approved", "time": "28d" }));
        list.push(json!({ "id": "gh-review-empty", "kind": "comment", "author": "ggrimes", "verb": "approved", "time": "1mo" }));
        patch_pr(sd, "482", json!({ "comments": comments }));
    });
    s.open_pr("482");
    assert!(s.has("tl-event:gh-event-fp:jcstein force-pushed the branch"));
    assert!(!s.has("rxn-add:gh-event-fp"));
    assert!(s.has("tl-event:gh-review-action-1:review-ok:Ferret-san approved"));
    assert!(!s.has("rxn-add:gh-review-action-1"));
    assert!(s.has("cmt:gh-review-empty:review-ok:ggrimes "));
    assert!(s.has("cmt-review-badge:ok"));
    assert!(!s.has("rxn-add:gh-review-empty"));
}
