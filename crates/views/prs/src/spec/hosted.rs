//! PR detail in its workspace: the pull request view of the workspace's center is PR detail's
//! four tabs, with the page's header at the top of the conversation and no page chrome.

use std::collections::BTreeMap;

use gpui::TestAppContext;
use recur_services::fake::Overrides;
use recur_view_core::ViewName;

use super::harness::*;

/// The seed's GitHub PR and the workspace it belongs to.
pub const PR: &str = "482";
pub const WS: &str = "LOOP-2231";

/// The workspace on its pull request view.
pub fn open_ws_pr(s: &mut S) {
    s.state
        .update(s.cx, |st, cx| st.open_workspace(WS, false, cx));
    s.settle();
    s.click("obs-pr-toggle");
    assert_eq!(s.view(), ViewName::Detail);
}

/// A window whose center column fits the four tabs (in a narrower one they scroll).
pub fn wide(cx: &mut TestAppContext) -> S<'_> {
    boot_sized(cx, |_| {}, Overrides::default(), 1900., 880.)
}

fn within(s: &mut S, inner: &str, outer: &str) -> bool {
    match (s.bounds(inner), s.bounds(outer)) {
        (Some(i), Some(o)) => {
            i.left() >= o.left()
                && i.right() <= o.right()
                && i.top() >= o.top()
                && i.bottom() <= o.bottom()
        }
        _ => false,
    }
}

#[gpui::test]
fn the_workspace_center_shows_the_four_tabs_and_switches_between_them(cx: &mut TestAppContext) {
    let mut s = wide(cx);
    open_ws_pr(&mut s);
    for name in ["Commits", "Files changed", "Checks"] {
        assert!(s.has(&format!("prtab:{name}:off")), "{name}");
    }
    assert!(s.has("prtab:Overview:on:lit"));
    let (tabs, row) = (
        s.bounds("prtabs").unwrap(),
        s.bounds("obs-center-tabs").unwrap(),
    );
    assert_eq!(tabs, row, "the tabs are the row under the toolbar");
    assert!(within(&mut s, "prpane:conv", "obs-pr"));
    assert!(
        !s.has("prd-agent"),
        "the workspace's chat is beside it already"
    );
    assert!(s.has("obs-left"));

    for (name, pane) in [
        ("Checks", "prpane:checks"),
        ("Commits", "prpane:commits"),
        ("Overview", "prpane:conv"),
    ] {
        s.tab(name);
        assert!(s.has(&format!("prtab:{name}:on:lit")), "{name}");
        assert!(s.until(|s| s.has(pane)), "{pane}");
        assert!(within(&mut s, pane, "obs-pr"));
    }
    s.tab("Files changed");
    assert!(s.has("prtab:Files changed:on:lit"));
    assert!(!s.has("prpane:conv"));
    assert_eq!(s.view(), ViewName::Detail);
}

#[gpui::test]
fn the_overview_leads_with_the_title_people_and_labels(cx: &mut TestAppContext) {
    let mut s = wide(cx);
    open_ws_pr(&mut s);
    let pr = s.pr(PR);
    assert!(s.has("prtab:Overview:on:lit") && !s.has("prtab:Conversation:on:lit"));
    for sel in [
        "prh".to_string(),
        format!("prh-title={}", pr.title),
        "prh-side".to_string(),
    ] {
        assert!(within(&mut s, &sel, "prpane:conv"), "{sel}");
    }
    assert!(
        within(&mut s, "prh-src=GitHub PR", "prpane:conv"),
        "the state, the source and the number lead it"
    );
    let (head, pane) = (s.bounds("prh").unwrap(), s.bounds("prpane:conv").unwrap());
    assert!(
        head.top() > pane.top() && head.left() > pane.left() && head.right() < pane.right(),
        "the conversation's first row, inside its padding, not a band over it"
    );

    s.tab("Checks");
    assert!(!s.has("prh"), "only the conversation has it");
}

#[gpui::test]
fn in_a_workspace_the_header_is_one_compact_line(cx: &mut TestAppContext) {
    // A center too narrow for the page's header: on the page it wraps to three lines.
    let mut s = boot_sized(cx, |_| {}, Overrides::default(), 1280., 880.);
    open_ws_pr(&mut s);
    let (head, title, pill) = (
        s.bounds("prh").unwrap(),
        s.bounds("prh-t").unwrap(),
        s.bounds("prh-side").unwrap(),
    );
    assert!(f32::from(head.size.height) <= 36., "one line: {head:?}");
    assert!(
        title.top() >= head.top() && pill.bottom() <= head.bottom() && pill.left() >= title.right(),
        "the title and the people share that line"
    );
    assert!(
        !s.has("prh-branch"),
        "the workspace's titlebar has the branch"
    );
}

#[gpui::test]
fn the_participants_popout_stays_in_the_window(cx: &mut TestAppContext) {
    // With the right panel hidden the people sit at the window's right edge.
    let mut s = boot_sized(cx, |_| {}, Overrides::default(), 1280., 880.);
    open_ws_pr(&mut s);
    if s.has("obs-side") {
        s.click("obs-side-toggle");
    }
    assert!(!s.has("obs-side"));
    s.hover("pr-people:prh");
    let (pop, window) = (
        s.bounds("pr-people-pop:prh").expect("the popout"),
        s.bounds("obs").expect("the workspace"),
    );
    assert!(pop.right() <= window.right(), "{pop:?} inside {window:?}");
    assert!(
        pop.top() > s.bounds("pr-people:prh").unwrap().bottom(),
        "under the people"
    );
}

#[gpui::test]
fn the_tab_is_kept_per_pull_request_while_the_center_shows_another_view(cx: &mut TestAppContext) {
    let mut s = wide(cx);
    open_ws_pr(&mut s);
    s.tab("Checks");
    s.click("obs-editor-toggle");
    assert!(!s.has("prtab:Checks:on:lit") && !s.has("obs-pr"));
    s.click("obs-pr-toggle");
    assert!(s.has("prtab:Checks:on:lit"));

    s.state
        .update(s.cx, |st, cx| st.go(ViewName::Briefing, None, cx));
    s.settle();
    s.state
        .update(s.cx, |st, cx| st.open_workspace(WS, false, cx));
    s.settle();
    assert!(s.has("prtab:Checks:on:lit"));
}

fn counted(s: &mut S, f: impl FnOnce(&mut S)) -> BTreeMap<&'static str, u32> {
    recur_ui::guard::count_renders(true);
    f(s);
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    counts
}

/// The chat's composer is an isolated view beside the PR view: a key in it renders neither PR
/// detail nor its tab row.
#[gpui::test]
async fn a_key_in_the_chat_renders_nothing_of_the_pull_request(cx: &mut TestAppContext) {
    let mut s = wide(cx);
    open_ws_pr(&mut s);
    let v = s.dock().expect("workspace view");
    s.cx.update(|window, cx| {
        let chat = v.read(cx).chat().clone();
        chat.update(cx, |c, cx| c.focus_composer(window, cx));
    });
    s.settle();
    let key = |s: &mut S| {
        s.cx.simulate_input("a");
        s.cx.run_until_parked();
    };
    key(&mut s);
    let counts = counted(&mut s, |s| (0..3).for_each(|_| key(s)));
    assert!(!counts.is_empty(), "the composer rendered");
    for quiet in [
        "::PrDetail",
        "::PrHead",
        "::PrTitlebarActions",
        "::ItemPane",
        "::MarkdownView",
        "::Observatory",
    ] {
        let n: u32 = counts
            .iter()
            .filter(|(k, _)| k.ends_with(quiet))
            .map(|(_, n)| *n)
            .sum();
        assert_eq!(n, 0, "{quiet} rendered: {counts:?}");
    }
}

// ---- where opening a PR lands ----

fn selected_ws(s: &mut S) -> String {
    s.state
        .read_with(s.cx, |st, _| st.selected_workspace_id.clone())
}

#[gpui::test]
fn a_list_row_of_a_pr_with_a_workspace_opens_that_workspace_on_the_pull_request(
    cx: &mut TestAppContext,
) {
    let mut s = wide(cx);
    s.prs_nav();
    s.seg("All");
    s.click(&format!("prrow:{PR}"));
    assert_eq!(s.view(), ViewName::Detail);
    assert_eq!(selected_ws(&mut s), WS);
    assert!(s.until(|s| s.has("obs-pr")));
    assert!(s.has("prtab:Overview:on:lit"));
}

#[gpui::test]
fn an_inbox_target_opens_the_workspace_even_from_another_center_view(cx: &mut TestAppContext) {
    let mut s = wide(cx);
    s.state
        .update(s.cx, |st, cx| st.open_workspace(WS, false, cx));
    s.settle();
    s.click("obs-editor-toggle");
    assert!(!s.has("obs-pr"));
    s.state
        .update(s.cx, |st, cx| st.go(ViewName::Briefing, None, cx));
    s.settle();
    s.state
        .update(s.cx, |st, cx| st.route_target(&format!("pr:{PR}"), cx));
    s.settle();
    assert_eq!(s.view(), ViewName::Detail);
    assert_eq!(selected_ws(&mut s), WS);
    assert!(s.until(|s| s.has("obs-pr")));
}

#[gpui::test]
fn a_pr_without_a_workspace_opens_as_the_page_it_was(cx: &mut TestAppContext) {
    let mut s = boot_sized(
        cx,
        |sd| patch_pr(sd, PR, serde_json::json!({ "workspaceId": "" })),
        Overrides::default(),
        1900.,
        880.,
    );
    s.prs_nav();
    s.seg("All");
    s.click(&format!("prrow:{PR}"));
    assert_eq!(s.view(), ViewName::PrDetail);
    assert!(s.has("v-prdetail"));
    let (head, tabs, pane) = (
        s.bounds("prh").unwrap(),
        s.bounds("prtabs").unwrap(),
        s.bounds("prpane:conv").unwrap(),
    );
    assert!(head.bottom() <= tabs.top(), "the header is over the tabs");
    assert!(tabs.bottom() <= pane.top());
    assert!(!s.has("obs-pr"));
    assert!(s.has("pr-act:pr-gh=Open in GitHub"));
    assert!(s.has("tb-create-workspace"));
}

#[gpui::test]
fn an_older_pr_of_a_workspace_opens_as_a_page(cx: &mut TestAppContext) {
    let mut s = boot_sized(
        cx,
        |sd| {
            sd.prs.push(pr_row(serde_json::json!({
                "id": "gh-old", "number": 400, "title": "An earlier attempt",
                "workspaceId": WS, "state": "Closed", "group": "merged",
            })));
        },
        Overrides::default(),
        1900.,
        880.,
    );
    s.state.update(s.cx, |st, cx| st.open_pr("gh-old", cx));
    s.settle();
    assert_eq!(s.view(), ViewName::PrDetail);
    assert!(s.has("prh-title=An earlier attempt"));

    s.state.update(s.cx, |st, cx| st.open_pr(PR, cx));
    s.settle();
    assert_eq!(s.view(), ViewName::Detail);
    assert_eq!(selected_ws(&mut s), WS);
    assert!(s.until(|s| s.has("obs-pr")));
    let title = s.pr(PR).title;
    assert!(s.has(&format!("prh-title={title}")));
}

// ---- where the PR page's titlebar buttons live in a workspace ----

fn open_ws(s: &mut S) {
    s.state
        .update(s.cx, |st, cx| st.open_workspace(WS, false, cx));
    s.settle();
}

fn review_kind(s: &mut S) -> Option<recur_view_core::PrReviewKind> {
    s.state.read_with(s.cx, |st, _| st.overlays.pr_review)
}

/// The buttons the page's titlebar has and a workspace's does not: its View PR menu has two of
/// them, its own land chrome the git ones, and the pull request's tab row the review ones.
const PAGE_ONLY: [&str; 6] = [
    "pr-act:pr-gh=Open in GitHub",
    "pr-act:pr-to-draft=Convert to draft",
    "pr-act:pr-push=Push updates",
    "pr-act:pr-pull=Pull updates",
    "pr-act:pr-force-push=Force push",
    "pr-update-chev",
];

#[gpui::test]
fn the_merge_button_is_in_the_workspace_titlebar_whatever_the_center_shows(
    cx: &mut TestAppContext,
) {
    let mut s = wide(cx);
    open_ws(&mut s);
    assert!(!s.has("obs-pr"), "the center is on the editor");
    assert!(s.has("pr-act:pr-merge=Merge"));
    let (view_pr, merge, tabs) = (
        s.bounds("tb-view-pr").unwrap(),
        s.bounds("pr-act:pr-merge=Merge").unwrap(),
        s.bounds("obs-center-tabs").unwrap(),
    );
    assert!(merge.bottom() <= tabs.top(), "in the titlebar");
    assert!(view_pr.right() <= merge.left(), "after View PR");
    for sel in PAGE_ONLY {
        assert!(!s.has(sel), "{sel}");
    }
    assert!(
        !s.has("pr-act:pr-review=Approve"),
        "the review actions wait for the PR view"
    );

    s.click("obs-pr-toggle");
    assert!(s.has("pr-act:pr-merge=Merge"));
    for sel in PAGE_ONLY {
        assert!(!s.has(sel), "{sel}");
    }
}

#[gpui::test]
fn the_titlebar_button_follows_the_pull_request_state(cx: &mut TestAppContext) {
    let mut s = boot_sized(
        cx,
        |sd| {
            patch_pr(
                sd,
                PR,
                serde_json::json!({
                    "mergeStateStatus": "BLOCKED", "checksOk": false, "reviewDecision": "APPROVED",
                    "viewerCanWrite": true, "viewerLogin": "josh",
                    "checks": [{"name": "lint", "passed": false, "status": "fail"}],
                }),
            )
        },
        Overrides::default(),
        1900.,
        880.,
    );
    open_ws(&mut s);
    assert!(s.has("pr-act:pr-fix-checks=Fix checks"));
    assert!(!s.has("pr-act:pr-merge=Merge"));
    // Fix checks runs in the chat beside it and turns the pull request to its checks.
    s.click("pr-act:pr-fix-checks=Fix checks");
    s.click("obs-pr-toggle");
    assert!(s.has("prtab:Checks:on:lit"));

    s.state.update(s.cx, |st, cx| {
        st.update_pr(
            PR,
            |p| {
                p.draft = true;
                p.merge_state_status = "DRAFT".into();
            },
            cx,
        )
    });
    s.settle();
    assert!(s.has("pr-act:pr-ready=Ready for review"));
    assert!(!s.has("pr-act:pr-fix-checks=Fix checks"));

    s.state.update(s.cx, |st, cx| {
        st.update_pr(
            PR,
            |p| {
                p.state = "Merged".into();
                p.group = "merged".into();
            },
            cx,
        )
    });
    s.settle();
    assert!(
        !s.has("pr-act:pr-ready=Ready for review"),
        "a merged PR has no action"
    );
}

#[gpui::test]
fn the_review_actions_are_in_the_tab_row_only_while_the_pull_request_shows(
    cx: &mut TestAppContext,
) {
    let mut s = boot_sized(
        cx,
        |sd| {
            patch_pr(
                sd,
                PR,
                serde_json::json!({
                    "pendingComments": [{
                        "id": "pend-1", "file": "src/auth/oauth.ts", "line": 2, "side": "RIGHT",
                        "body": "tighten this",
                    }],
                }),
            )
        },
        Overrides::default(),
        1900.,
        880.,
    );
    open_ws(&mut s);
    let row_actions = [
        "pr-act:pr-review=Approve",
        "pr-act:pending=1 pending",
        "pr-act:pr-send-back=Send back",
    ];
    for sel in row_actions {
        assert!(!s.has(sel), "{sel} before the PR view shows");
    }
    s.click("obs-pr-toggle");
    for sel in row_actions {
        assert!(within(&mut s, sel, "obs-center-tabs"), "{sel}");
    }
    let (chev, row, tabs, send_back) = (
        s.bounds("pr-review-chev").unwrap(),
        s.bounds("obs-center-tabs").unwrap(),
        s.bounds("prtabs-scroll").unwrap(),
        s.bounds("pr-act:pr-send-back=Send back").unwrap(),
    );
    assert!(f32::from(row.right() - chev.right()) <= 9., "right-aligned");
    assert!(
        tabs.right() <= send_back.left(),
        "the tabs scroll in what the actions leave"
    );

    // They act on the PR the view shows.
    s.click("pr-act:pr-review=Approve");
    assert_eq!(
        review_kind(&mut s),
        Some(recur_view_core::PrReviewKind::Approve)
    );
    assert!(
        s.has("pr-rev-pending"),
        "the modal lists this PR's pending comments"
    );
    s.click("pr-rev-x");
    assert_eq!(review_kind(&mut s), None);
    s.click("pr-review-chev");
    s.click("pr-review-request");
    assert_eq!(
        review_kind(&mut s),
        Some(recur_view_core::PrReviewKind::Request)
    );
    s.click("pr-rev-x");

    s.click("obs-editor-toggle");
    for sel in row_actions {
        assert!(!s.has(sel), "{sel} on the editor");
    }
}

#[gpui::test]
fn the_view_pr_menu_opens_github_and_converts_to_draft(cx: &mut TestAppContext) {
    let mut s = wide(cx);
    open_ws(&mut s);
    s.click("tb-view-pr-chev");
    assert!(s.has("view-pr-github-item"));
    s.click("view-pr-draft-item");
    assert!(!s.has("view-pr-draft-item"), "the menu closes");
    assert!(s.until(|s| s.toasted(recur_ui::widgets::ToastKind::Success, "Converted to draft")));
    assert!(s.pr(PR).draft);
    assert!(s.has("pr-act:pr-ready=Ready for review"));
    assert!(!s.has("pr-act:pr-merge=Merge"));

    s.click("tb-view-pr-chev");
    assert!(s.has("view-pr-github-item"));
    assert!(
        !s.has("view-pr-draft-item"),
        "a draft has nothing to convert"
    );
}

/// In a center narrower than the four tabs, the row scrolls and keeps the active tab in view.
#[gpui::test]
fn a_narrow_center_scrolls_the_active_tab_into_view(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_ws_pr(&mut s);
    let row = s.bounds("prtabs-scroll").unwrap();
    let checks = s.bounds("prtab:Checks:off").unwrap();
    assert!(
        checks.right() > row.right(),
        "the tabs do not fit: {checks:?} in {row:?}"
    );
    let id = PR.to_string();
    s.cx.update(|_, cx| {
        crate::ui::ui(cx).update(cx, |u, cx| u.set_tab(&id, crate::ui::PrTab::Checks, cx))
    });
    s.settle();
    let (tab, row) = (s.bounds("prtab:Checks:on:lit"), s.bounds("prtabs-scroll"));
    assert!(
        within(&mut s, "prtab:Checks:on:lit", "prtabs-scroll"),
        "{tab:?} in {row:?}"
    );
    assert!(s.until(|s| s.has("prpane:checks")));
}
