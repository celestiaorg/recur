//! The review surface on PR #482 in PR detail's Files pane (file list beside the stacked
//! diffs, PR note mode). The PR detail host's own parts (the PR list, tabs, titlebar review
//! actions and the review modal, conversation cards, checks, merge) are specced in
//! `recur-view-prs`.

use gpui::TestAppContext;
use recur_domain::Finding;
use recur_engine::view::comment_thread::DiffSide;
use recur_services::fake::{Overrides, hold};

use super::harness::*;

const OAUTH: &str = "src/auth/oauth.ts";

/// Scroll the stack to its top (Playwright scrolls a clicked control into view).
fn to_top(h: &mut H, host: &gpui::Entity<PrHost>) {
    let pane = host.read_with(h.cx, |hst, _| hst.stack.pane.clone());
    pane.set_scroll_top(0.);
    h.settle();
}

fn thread_count(h: &mut H, root: &str) -> usize {
    let pr = h.pr("482");
    let comments = pr.comments.unwrap_or_default();
    recur_engine::view::comment_thread::thread_replies(&comments, root).len()
}

#[gpui::test]
async fn files_changed_is_a_sidebar_toc_plus_a_scrolling_stack(cx: &mut TestAppContext) {
    let (mut h, host) = pr_files(cx, Overrides::default(), None);
    assert!(h.has(&format!("cfile:live:{OAUTH}")));
    assert!(h.has("cfile:live:src/hooks/useOAuthSession.ts"));
    h.click("fl-row:file:src/checkout/SignInButtons.tsx");
    let sel = host.read_with(h.cx, |hst, _| hst.stack.selected().to_string());
    assert_eq!(sel, "src/checkout/SignInButtons.tsx");
    let card = h
        .bounds("cfile:live:src/checkout/SignInButtons.tsx")
        .expect("card");
    assert!(f32::from(card.origin.y) < 900.);
}

#[gpui::test]
async fn collapsing_a_files_changed_card_shrinks_it_to_the_header(cx: &mut TestAppContext) {
    let (mut h, _host) = pr_files(cx, Overrides::default(), None);
    let card = format!("cfile:live:{OAUTH}");
    assert!(f32::from(h.bounds(&card).expect("card").size.height) > 80.);
    h.click(&format!("df-chev:{OAUTH}"));
    assert!(h.has(&format!("df-head:{OAUTH}:collapsed")));
    assert!(!h.has(&format!("hline:{OAUTH}:add:right:12")));
    assert!(f32::from(h.bounds(&card).expect("card").size.height) < 64.);
}

#[gpui::test]
async fn open_in_workspace_from_a_pr_file_diff(cx: &mut TestAppContext) {
    let (mut h, host) = pr_files(cx, Overrides::default(), None);
    h.click(&format!("df-open:{OAUTH}"));
    let opened = host.read_with(h.cx, |hst, _| hst.opened.borrow().clone());
    assert_eq!(opened, vec![(OAUTH.to_string(), None)]);
}

#[gpui::test]
async fn files_changed_findings_render_as_inline_cards(cx: &mut TestAppContext) {
    let finding = Finding {
        id: "f-race".into(),
        file: "src/hooks/useOAuthSession.ts".into(),
        line: 2,
        message: "token refresh can race across two tabs".into(),
        status: "open".into(),
        severity: "high".into(),
        ..Default::default()
    };
    let (mut h, host) = pr_files(cx, Overrides::default(), Some(vec![finding]));
    assert!(h.has("obs-finding-flag:src/hooks/useOAuthSession.ts"));
    h.click("fl-row:file:src/hooks/useOAuthSession.ts");
    assert!(h.has("icmt-finding:f-race"));
    h.click("open-in-findings:f-race");
    let opened = host.read_with(h.cx, |hst, _| hst.opened_findings.borrow().clone());
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].id, "f-race");
}

#[gpui::test]
async fn flags_files_with_unresolved_line_comments_and_jumps_to_them(cx: &mut TestAppContext) {
    let (mut h, host) = pr_files(cx, Overrides::default(), None);
    assert!(h.has(&format!("obs-comment-flag:{OAUTH}")));
    h.click(&format!("obs-comment-flag:{OAUTH}"));
    let hl = host.read_with(h.cx, |hst, _| hst.highlight);
    assert_eq!(hl, 12);
    let df = host
        .read_with(h.cx, |hst, _| hst.diff_file(OAUTH))
        .expect("diff file");
    // The note covers 11-12 on the right; it ends on 12.
    let r11 = df
        .read_with(h.cx, |d, cx| d.row_classes(11, DiffSide::Right, cx))
        .expect("11");
    let r12 = df
        .read_with(h.cx, |d, cx| d.row_classes(12, DiffSide::Right, cx))
        .expect("12");
    assert!(r11.noted && !r11.note_end);
    assert!(r12.noted && r12.note_end);
    let thread = h
        .bounds(&format!("icmt-thread:{OAUTH}:12"))
        .expect("thread");
    assert!(f32::from(thread.origin.y) >= -24. && f32::from(thread.origin.y) < 900.);
}

#[gpui::test]
async fn shows_a_posted_line_note_on_the_files_diff_and_replies(cx: &mut TestAppContext) {
    let (mut h, _host) = pr_files(cx, Overrides::default(), None);
    assert!(h.has(&format!("icmt-thread:{OAUTH}:12")));
    h.click("cmt-more:gh-inline-1");
    assert!(h.has("menuitem:Quote reply"));
    assert!(h.has("menuitem:Edit"));
    assert!(h.has("menuitem:Delete"));
    h.click("menuitem:Quote reply");
    assert!(h.has("note-input:reply"));
    h.click("note-cancel");
    h.click("cmt-reply-ph:gh-inline-1");
    h.type_text("diff reply from e2e");
    let before = thread_count(&mut h, "gh-inline-1");
    h.click("note-reply");
    assert_eq!(thread_count(&mut h, "gh-inline-1"), before + 1);
    assert!(h.has("cmt-resolve:gh-inline-1"));
    h.click("cmt-resolve:gh-inline-1");
    assert!(h.has("cmt-resolved-bar:gh-inline-1"));
    h.click("cmt-unresolve:gh-inline-1");
    assert!(h.has("cmt-resolve:gh-inline-1"));
}

#[gpui::test]
async fn quote_reply_prefills_the_reply_box(cx: &mut TestAppContext) {
    let (mut h, host) = pr_files(cx, Overrides::default(), None);
    h.click("cmt-more:gh-inline-1");
    h.click("menuitem:Quote reply");
    let df = host
        .read_with(h.cx, |hst, _| hst.diff_file(OAUTH))
        .expect("diff file");
    let _ = df;
    let text = h.cx.update(|_, cx| {
        let hst = host.read(cx);
        let df = hst.diff_file(OAUTH).expect("df");
        let threads = df.read(cx).thread_entities();
        threads
            .first()
            .map(|t| t.read(cx).reply_text(cx))
            .unwrap_or_default()
    });
    assert!(text.starts_with("> hmm"), "{text:?}");
}

#[gpui::test]
async fn keyboard_activating_a_plus_opens_the_composer(cx: &mut TestAppContext) {
    let (mut h, host) = pr_files(cx, Overrides::default(), None);
    to_top(&mut h, &host);
    let focus = host
        .read_with(h.cx, |hst, cx| {
            hst.diff_file(OAUTH)
                .and_then(|df| df.read(cx).addc_focus(DiffSide::Right, 11))
        })
        .expect("+ on line 11");
    h.cx.update(|window, cx| window.focus(&focus, cx));
    assert!(!h.has("finding-note-input"));
    // GPUI turns enter into a click on its key-up; the simulator only sends key-downs.
    h.keys("enter");
    h.cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").expect("enter"),
    });
    assert!(h.has("finding-note-input"));
}

#[gpui::test]
async fn leaves_an_inline_comment_anchored_to_a_diff_line(cx: &mut TestAppContext) {
    let (mut h, host) = pr_files(cx, Overrides::default(), None);
    to_top(&mut h, &host);
    h.click(&format!("addc:{OAUTH}:right:11"));
    assert!(h.has("finding-note-input"));
    let body = "E2E: verify the secrets client initialises before providers.";
    h.type_text(body);
    h.click("compose-add");
    let pending = h.pr("482").pending_comments;
    assert_eq!(pending.iter().filter(|p| p.body == body).count(), 1);
    let pc = pending.iter().find(|p| p.body == body).expect("pending");
    assert!(h.has(&format!("icmt-pending:{}", pc.id)));
}

#[gpui::test]
async fn drag_on_the_comment_control_selects_a_multiline_line_comment(cx: &mut TestAppContext) {
    let (mut h, host) = pr_files(cx, Overrides::default(), None);
    h.drag(
        &format!("addc:{OAUTH}:right:13"),
        &format!("addc:{OAUTH}:right:15"),
    );
    assert!(h.has("finding-note-input"));
    let df = host
        .read_with(h.cx, |hst, _| hst.diff_file(OAUTH))
        .expect("diff file");
    let ph = df.read_with(h.cx, |d, cx| {
        let _ = cx;
        d.compose_placeholder().to_string()
    });
    assert!(ph.contains("lines 13"), "{ph}");
    assert!(
        df.read_with(h.cx, |d, cx| d.row_classes(13, DiffSide::Right, cx))
            .expect("13")
            .drafting
    );
    assert!(
        df.read_with(h.cx, |d, cx| d.row_classes(15, DiffSide::Right, cx))
            .expect("15")
            .draft_end
    );
    h.type_text("E2E: this span should stay together");
    h.click("compose-add");
    let pc = h
        .pr("482")
        .pending_comments
        .into_iter()
        .find(|p| p.body.contains("this span"))
        .expect("pending");
    assert_eq!(
        (pc.start_line, pc.line, pc.side.as_str()),
        (13, 15, "RIGHT")
    );
    let df = host
        .read_with(h.cx, |hst, _| hst.diff_file(OAUTH))
        .expect("diff file");
    assert!(
        df.read_with(h.cx, |d, cx| d.row_classes(13, DiffSide::Right, cx))
            .expect("13")
            .noted
    );
    assert!(
        df.read_with(h.cx, |d, cx| d.row_classes(15, DiffSide::Right, cx))
            .expect("15")
            .note_end
    );
}

#[gpui::test]
async fn a_removed_pending_comment_leaves_the_diff(cx: &mut TestAppContext) {
    let (mut h, host) = pr_files(cx, Overrides::default(), None);
    to_top(&mut h, &host);
    h.click(&format!("addc:{OAUTH}:right:11"));
    h.type_text("remove me");
    h.click("compose-add");
    let pc = h
        .pr("482")
        .pending_comments
        .into_iter()
        .find(|p| p.body == "remove me")
        .expect("pending");
    h.hover(&format!("icmt-pending:{}", pc.id));
    h.click(&format!("pending-remove:{}", pc.id));
    assert!(!h.has(&format!("icmt-pending:{}", pc.id)));
    assert!(
        h.pr("482")
            .pending_comments
            .iter()
            .all(|p| p.body != "remove me")
    );
}

#[gpui::test]
async fn toggles_reactions_on_the_diff_thread(cx: &mut TestAppContext) {
    let (mut h, _host) = pr_files(cx, Overrides::default(), None);
    assert!(h.has("rxn-pill:gh-inline-1:eyes"));
    h.click("rxn-add:gh-inline-1");
    h.click("rxn-pick:+1");
    assert!(h.has("rxn-pill:gh-inline-1:+1"));
    let pr = h.pr("482");
    let c = pr
        .comments
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.id == "gh-inline-1")
        .expect("note");
    assert!(c.reactions.iter().any(|r| r.content == "+1" && r.mine));
    // Clicking a mine pill removes it.
    h.click("rxn-pill:gh-inline-1:+1");
    assert!(!h.has("rxn-pill:gh-inline-1:+1"));
}

#[gpui::test]
async fn deletes_a_line_note_from_the_more_menu(cx: &mut TestAppContext) {
    let (mut h, _host) = pr_files(cx, Overrides::default(), None);
    h.click("cmt-reply-ph:gh-inline-1");
    h.type_text("thread reply from e2e");
    h.click("note-reply");
    let reply = h
        .pr("482")
        .comments
        .unwrap_or_default()
        .into_iter()
        .find(|c| c.body_md == "thread reply from e2e")
        .expect("reply");
    h.click(&format!("cmt-more:{}", reply.id));
    h.click("menuitem:Delete");
    assert!(
        h.pr("482")
            .comments
            .unwrap_or_default()
            .iter()
            .all(|c| c.id != reply.id)
    );
}

#[gpui::test]
async fn closes_the_reply_composer_before_a_slow_github_write_returns(cx: &mut TestAppContext) {
    let (tx, rx) = std::sync::mpsc::channel();
    let opts = Overrides::default().reply_pr_comment(move |_, _| {
        let (answer, release) = hold();
        let _ = tx.send(release);
        answer
    });
    let (mut h, _host) = pr_files(cx, opts, None);
    h.click("cmt-reply-ph:gh-inline-1");
    h.type_text("slow thread reply");
    h.click("note-reply");
    // The composer closed and the optimistic reply shows while the write is in flight.
    assert!(h.has("cmt-reply-ph:gh-inline-1"));
    let comments = h.pr("482").comments.unwrap_or_default();
    assert!(
        comments
            .iter()
            .any(|c| c.body_md == "slow thread reply" && c.id.starts_with("c-pending-"))
    );
    let release = rx.try_recv().expect("in flight");
    release.send(Ok(recur_domain::PrComment {
        id: "gh-inline-slow-reply".into(),
        code: "YO".into(),
        author: "You".into(),
        time: "now".into(),
        body_md: "slow thread reply".into(),
        verb: "commented".into(),
        kind: "comment".into(),
        in_reply_to_id: "gh-inline-1".into(),
        ..Default::default()
    }));
    h.settle();
    let comments = h.pr("482").comments.unwrap_or_default();
    assert!(comments.iter().any(|c| c.id == "gh-inline-slow-reply"));
    assert!(!comments.iter().any(|c| c.id.starts_with("c-pending-")));
}
