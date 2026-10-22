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
