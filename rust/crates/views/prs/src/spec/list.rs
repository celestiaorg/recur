//! The PR list halves of `pr-review.spec.ts`.

use gpui::TestAppContext;

use super::harness::*;

#[gpui::test]
fn probe_people_layout(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.prs_nav();
    s.seg("All");
    for sel in [
        "pr-people:row-482",
        "pr-face:avery",
        "pr-why:482=Needs your review",
        "prrow:482",
    ] {
        eprintln!("{sel}: {:?}", s.bounds(sel));
    }
}

#[gpui::test]
fn probe_detail_layout(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_pr("482");
    s.state.update(s.cx, |st, cx| {
        st.pr_agent_dock_open = false;
        cx.notify();
    });
    s.settle();
    for sel in [
        "cmt-reply-ph:gh-inline-1",
        "cmt-resolve:Resolve conversation",
    ] {
        eprintln!("{sel}: {:?}", s.bounds(sel));
    }
    s.click("prtab:Files changed:off");
    for sel in ["cmt-reply-ph:gh-inline-1", "cmt-resolve:gh-inline-1"] {
        eprintln!("files {sel}: {:?}", s.bounds(sel));
    }
}
