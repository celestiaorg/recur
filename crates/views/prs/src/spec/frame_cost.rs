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

const BIG: &str = "Vendor the generated client";

/// A commit of `files` new files, `lines` lines each.
fn big_commit(files: usize, lines: usize) -> serde_json::Value {
    let files: Vec<_> = (0..files)
        .map(|i| {
            let mut rows = vec![serde_json::json!({
                "type": "hunk", "codeHtml": format!("@@ -0,0 +1,{lines} @@ new file"),
            })];
            rows.extend((1..=lines).map(|n| {
                serde_json::json!({
                    "new": n.to_string(), "type": "add",
                    "codeHtml": format!("export const v{n} = {n};"),
                })
            }));
            serde_json::json!({
                "path": format!("gen/client/f{i:03}.ts"), "status": "a",
                "diff": { "add": lines, "del": 0 }, "lines": rows,
            })
        })
        .collect();
    serde_json::json!({
        "hash": "b16c0de", "sha": "b16c0de0000000000000000000000000000000000",
        "message": BIG, "authorRole": "Author", "diff": { "add": 0, "del": 0 },
        "when": "1m", "files": files,
    })
}

fn detail(s: &mut S) -> gpui::Entity<crate::detail::PrDetail> {
    s.cx.update(|_, cx| crate::detail::spec_handle::current(cx))
        .expect("pr detail")
}

/// The rows a stack draws, by identity.
fn rows(
    s: &mut S,
    stack: fn(&crate::detail::PrDetail) -> &recur_view_review::stack::FileStack,
) -> Vec<*const recur_domain::FileDiff> {
    let d = detail(s);
    s.cx.update(|_, cx| {
        stack(d.read(cx))
            .files()
            .iter()
            .map(std::rc::Rc::as_ptr)
            .collect()
    })
}

fn redraw(s: &mut S) {
    let d = detail(s);
    s.cx.update(|_, cx| d.update(cx, |_, cx| cx.notify()));
    s.settle();
}

/// A big commit's diff mounts a window of its cards, and a redraw (every scroll step is one)
/// hands those cards the rows they already have instead of a fresh copy of the whole diff.
#[gpui::test]
async fn a_large_commit_redraws_without_copying_its_diff(cx: &mut TestAppContext) {
    let mut s = super::harness::plain_with(cx, |sd| {
        let mut commits = sd.pr_by_id("482").unwrap()["commits"].clone();
        commits
            .as_array_mut()
            .unwrap()
            .insert(0, big_commit(200, 200));
        super::harness::patch_pr(sd, "482", serde_json::json!({ "commits": commits }));
    });
    s.open_pr("482");
    s.tab("Commits");
    s.click(&format!("commit:{BIG}"));
    assert!(s.has("cfile:live:gen/client/f000.ts"));
    assert!(!s.has("cfile:live:gen/client/f199.ts"));
    let before = rows(&mut s, |d| &d.commit_stack);
    assert_eq!(before.len(), 200);
    redraw(&mut s);
    assert_eq!(rows(&mut s, |d| &d.commit_stack), before);
}

/// Files changed hands its cards the same rows on a redraw and on a store change that leaves
/// the PR's files as they were (the 15s poll, a new comment).
#[gpui::test]
async fn files_changed_keeps_its_rows_across_redraws_and_store_changes(cx: &mut TestAppContext) {
    let mut s = open(cx);
    s.tab("Files changed");
    assert!(s.has("cfile:live:src/auth/oauth.ts"));
    let before = rows(&mut s, |d| &d.stack);
    assert_eq!(before.len(), 3);
    redraw(&mut s);
    assert_eq!(rows(&mut s, |d| &d.stack), before);
    let state = s.state.clone();
    s.cx.update(|_, cx| {
        state.update(cx, |st, cx| {
            let id = st.selected_pr_id.clone();
            st.update_pr(&id, |p| p.title.push('!'), cx);
        })
    });
    s.settle();
    assert_eq!(rows(&mut s, |d| &d.stack), before);
}
