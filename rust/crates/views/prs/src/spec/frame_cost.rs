//! What a keystroke, a store change and a tab switch render on PR detail: never the
//! workspace behind it (the hidden detail route), the sidebar or the overlays.

use std::collections::BTreeMap;

use gpui::TestAppContext;
use recur_view_core::{PrReviewKind, Topics};

use super::harness::{S, plain};

fn renders(name: &str, counts: &BTreeMap<&'static str, u32>) -> u32 {
    counts
        .iter()
        .filter(|(k, _)| k.ends_with(name))
        .map(|(_, n)| *n)
        .sum()
}

fn counted(s: &mut S, f: impl FnOnce(&mut S)) -> BTreeMap<&'static str, u32> {
    recur_ui::guard::count_renders(true);
    f(s);
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    counts
}

/// The workspace behind PR detail: its shell is not laid out while the route is hidden.
const HIDDEN: [&str; 7] = [
    "::WorkspaceView",
    "::Observatory",
    "::ReviewSurface",
    "::FilesChangedView",
    "::ChatTabsHead",
    "::DockToolbar",
    "::TranscriptPane",
];

/// The conversation beside the comment and review boxes: cached blocks a key replays.
const CONVERSATION: [&str; 3] = ["::ItemPane", "::MarkdownView", "::CommentReactions"];

fn assert_quiet(what: &str, quiet: &[&str], counts: &BTreeMap<&'static str, u32>) {
    for q in quiet {
        assert_eq!(renders(q, counts), 0, "{what}: {q} rendered: {counts:?}");
    }
}

/// A PR open beside the workspace it was reached from, its dock showing that workspace's chat.
fn open(cx: &mut TestAppContext) -> S<'_> {
    let mut s = plain(cx);
    s.click("hit:LOOP-2231");
    s.open_pr("482");
    s
}

fn type_keys(s: &mut S) {
    s.cx.simulate_input("a");
    s.cx.run_until_parked();
}

/// What rendered besides the field a key went to.
fn beside_the_field(counts: &BTreeMap<&'static str, u32>) -> BTreeMap<&'static str, u32> {
    counts
        .iter()
        .filter(|(k, _)| !k.ends_with("::FieldView"))
        .map(|(k, n)| (*k, *n))
        .collect()
}

/// A key renders the comment box alone: the box is an isolated view, so neither PR detail
/// (its host) nor the conversation beside it renders.
#[gpui::test]
async fn a_key_in_the_pr_comment_box_renders_the_box_only(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let d =
        s.cx.update(|_, cx| crate::detail::spec_handle::current(cx))
            .expect("pr detail");
    s.cx.update(|window, cx| d.update(cx, |d, cx| d.focus_composer(window, cx)));
    s.settle();
    type_keys(&mut s);
    let counts = counted(&mut s, |s| {
        for _ in 0..3 {
            type_keys(s);
        }
    });
    assert!(renders("::FieldView", &counts) >= 3, "{counts:?}");
    assert_eq!(beside_the_field(&counts), BTreeMap::new(), "comment key");
    assert_quiet("comment key", &HIDDEN, &counts);
    assert_quiet("comment key", &CONVERSATION, &counts);
    assert_quiet(
        "comment key",
        &[
            "::Sidebar",
            "::Layer",
            "::WorkspaceDetail",
            "::PrTitlebarActions",
        ],
        &counts,
    );
}

#[gpui::test]
async fn a_key_in_the_review_modal_renders_its_box_only(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let state = s.state.clone();
    s.cx.update(|_, cx| state.update(cx, |st, cx| st.open_pr_review(PrReviewKind::Approve, cx)));
    s.settle();
    type_keys(&mut s);
    let counts = counted(&mut s, |s| {
        for _ in 0..3 {
            type_keys(s);
        }
    });
    assert!(renders("::FieldView", &counts) >= 3, "{counts:?}");
    assert_eq!(beside_the_field(&counts), BTreeMap::new(), "review key");
    assert_quiet("review key", &HIDDEN, &counts);
    assert_quiet("review key", &CONVERSATION, &counts);
    assert_quiet(
        "review key",
        &[
            "::Sidebar",
            "::WorkspaceDetail",
            "::PrTitlebarActions",
            "::Toasts",
        ],
        &counts,
    );
}

/// A store change that only PR detail's neighbours read (another workspace's row) does not
/// lay out the workspace behind it.
#[gpui::test]
async fn a_store_change_on_pr_detail_leaves_the_hidden_workspace_alone(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let state = s.state.clone();
    let counts = counted(&mut s, |s| {
        s.cx.update(|_, cx| {
            state.update(cx, |st, cx| {
                let mut w = st.workspace("LOOP-2224").cloned().expect("workspace");
                w.last = "a new event".into();
                st.put_workspace(w, cx);
            })
        });
        s.cx.run_until_parked();
        s.cx.update(|_, cx| state.update(cx, |st, cx| st.changed(Topics::WORKSPACES, cx)));
        s.cx.run_until_parked();
    });
    assert_quiet(
        "store change",
        &[
            "::WorkspaceView",
            "::Observatory",
            "::ReviewSurface",
            "::FilesChangedView",
        ],
        &counts,
    );
}

#[gpui::test]
async fn a_pr_tab_switch_does_not_lay_out_the_hidden_workspace(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let at = s
        .bounds("prtab:Files changed:off")
        .expect("files tab")
        .center();
    s.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    s.settle();
    let counts = counted(&mut s, |s| {
        s.cx.simulate_click(at, gpui::Modifiers::none());
        s.cx.run_until_parked();
    });
    assert!(renders("::DiffFile", &counts) > 0, "{counts:?}");
    assert_quiet(
        "tab switch",
        &["::WorkspaceView", "::Observatory", "::ReviewSurface"],
        &counts,
    );
}
