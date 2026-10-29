//! What a keystroke, a store change and a tab switch render on PR detail: never the
//! workspace behind it (the hidden detail route), the sidebar or the overlays.

use std::collections::BTreeMap;

use gpui::TestAppContext;
use recur_view_core::{PrReviewKind, Topics};

use serde_json::json;

use super::harness::{S, patch_pr, plain, plain_with};

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

/// Another workspace's agent event does not redraw the conversation: PR detail does not notify
/// its items for a store change that leaves what it shows as it was. A change to this PR still
/// redraws them.
#[gpui::test]
async fn an_unrelated_store_change_leaves_the_conversation_alone(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let state = s.state.clone();
    let counts = counted(&mut s, |s| {
        s.cx.update(|_, cx| {
            // What `apply_event` does with an agent event.
            state.update(cx, |st, cx| {
                let w = st.workspaces.iter_mut().find(|w| w.id == "LOOP-2224");
                w.expect("workspace").last = "a new event".into();
                st.changed(Topics::WORKSPACES, cx);
            })
        });
        s.cx.run_until_parked();
    });
    assert_quiet("agent event", &CONVERSATION, &counts);

    let counts = counted(&mut s, |s| {
        s.cx.update(|_, cx| {
            state.update(cx, |st, cx| {
                let id = st.selected_pr_id.clone();
                st.update_pr(&id, |p| p.title.push('!'), cx);
            })
        });
        s.cx.run_until_parked();
    });
    assert!(renders("::ItemPane", &counts) > 0, "{counts:?}");
}

/// The keys that empty the comment box and type into it again redraw its buttons (the part
/// drawing the box), never the conversation.
#[gpui::test]
async fn a_key_that_empties_the_comment_box_leaves_the_conversation_alone(cx: &mut TestAppContext) {
    let mut s = open(cx);
    let d =
        s.cx.update(|_, cx| crate::detail::spec_handle::current(cx))
            .expect("pr detail");
    s.cx.update(|window, cx| d.update(cx, |d, cx| d.focus_composer(window, cx)));
    s.settle();
    type_keys(&mut s);
    let counts = counted(&mut s, |s| {
        for k in ["backspace", "b", "backspace", "c"] {
            s.cx.simulate_keystrokes(k);
            s.cx.run_until_parked();
        }
    });
    assert!(
        renders("::Part<recur_view_prs::detail::PrDetail>", &counts) >= 4,
        "{counts:?}"
    );
    assert_quiet("emptying key", &CONVERSATION, &counts);
}

/// A PR with a thousand commits lays out the rows on screen, not every commit, so a PR detail
/// render (a hover, a store sweep, a pending check's spinner) stays cheap. The rows off screen
/// keep their place: scrolled to the end, the last commit shows where the list ends.
#[gpui::test]
async fn a_pr_with_many_commits_lays_out_the_visible_rows_only(cx: &mut TestAppContext) {
    let base = recur_services::mock::fixtures::seed()
        .pr_by_id("482")
        .unwrap()["commits"]
        .clone();
    let base = base.as_array().unwrap();
    let many: Vec<_> = (0..1000)
        .map(|i| {
            let mut c = base[i % base.len()].clone();
            c["hash"] = json!(format!("{i:07x}"));
            c["message"] = json!(format!("commit {i}"));
            c
        })
        .collect();
    let mut s = plain_with(cx, move |sd| {
        patch_pr(sd, "482", json!({ "commits": many }))
    });
    s.open_pr("482");
    s.tab("Commits");
    let a = s.bounds("commit:commit 0").expect("first row");
    let b = s.bounds("commit:commit 8").expect("ninth row");
    assert_eq!(
        f32::from(b.origin.y - a.origin.y),
        8. * 57.625,
        "row height"
    );
    assert!(!s.has("commit:commit 100"), "an off-screen row laid out");

    let d =
        s.cx.update(|_, cx| crate::detail::spec_handle::current(cx))
            .expect("pr detail");
    s.cx.update(|_, cx| {
        d.update(cx, |d, cx| {
            let max = d.commits_scroll.max_offset().y;
            d.commits_scroll.set_offset(gpui::point(gpui::px(0.), -max));
            cx.notify();
        })
    });
    s.settle();
    assert!(!s.has("commit:commit 0"), "a row scrolled away laid out");
    let last = s.bounds("commit:commit 999").expect("last row");
    let pane = s.bounds("prpane:commits:fade-top").expect("pane");
    let gap = f32::from(pane.bottom() - last.bottom());
    assert!(
        gap > 0. && gap < 60.,
        "the last row ends {gap}px above the pane's end"
    );
}
