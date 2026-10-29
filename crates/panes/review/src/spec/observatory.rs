//! The review-surface scenarios of `observatory.spec.ts`. The Observatory host's own halves
//! (the Review changes toggle and ⌥⌘R, the Changes / Commits chips, file tabs, remembered
//! panes) are covered in `recur-view-observatory`'s `spec::observatory`; the surface's events
//! stand in for them here.

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply};
use serde_json::json;

use super::harness::*;
use crate::diff_rows::*;
use crate::review_diff::{FileListMode, ReviewTab};
use crate::surface::SurfaceEvent;
use recur_engine::view::comment_thread::DiffSide;

fn main_go_lines(extra: serde_json::Value) -> serde_json::Value {
    json!({ "files": [ {
        "path": "src/main.go", "status": "m", "diff": {"add": 1, "del": 1},
        "lines": extra,
    } ] })
}

#[gpui::test]
async fn branch_review_shows_files_changed_and_commits_without_a_pr(cx: &mut TestAppContext) {
    let (mut h, host) = surface(cx, MockOptions::default(), |_| {});
    assert!(h.has("review:changes"));
    // Tree mode by default, one changed file, its card in the stack.
    let files = host.read_with(h.cx, |hst, cx| {
        hst.surface.read(cx).files_view().read(cx).file_list_mode()
    });
    assert_eq!(files, FileListMode::Tree);
    assert!(h.has("button:Tree:true"));
    assert!(h.has("fl-row:file:src/main.go"));
    assert!(h.has("cfile:live:src/main.go"));
    h.click("fl-row:file:src/main.go");
    let selected = host.read_with(h.cx, |hst, cx| {
        hst.surface
            .read(cx)
            .files_view()
            .read(cx)
            .stack()
            .selected()
            .to_string()
    });
    assert_eq!(selected, "src/main.go");
    // Hide and show the file list.
    h.click("button:Hide file list");
    assert!(!h.has("filelist"));
    assert!(h.has("cfile:live:src/main.go"));
    h.click("button:Show file list");
    assert!(h.has("filelist"));
    // Commits: the uncommitted row, then the branch's commits newest first.
    h.cx.update(|window, cx| {
        host.update(cx, |hst, cx| {
            hst.set(|p| p.tab = ReviewTab::Commits, window, cx)
        })
    });
    h.settle();
    assert!(h.has("review:commits"));
    assert!(h.has("commit:working"));
    assert!(h.has("commit:Add feature"));
    assert!(h.has("commit:Older work"));
    // Opening a commit switches to its files under Commits.
    h.click("commit:Add feature");
    assert!(host.read_with(h.cx, |hst, _| {
        hst.events
            .borrow()
            .contains(&SurfaceEvent::TabChange(ReviewTab::Commits))
    }));
    assert!(h.has("cback"));
    assert!(h.has("commit-hd"));
    assert!(h.has("commit-hd-chash"));
    assert!(h.has("diff-file:src/main.go"));
    // Changes clears nothing by itself (the chip clears the scope); ← all commits returns.
    h.click("cback");
    assert!(h.has("review:commits"));
    assert!(!h.has("cback"));
    // Back on Changes, the header opens the file in the editor.
    h.cx.update(|window, cx| {
        host.update(cx, |hst, cx| {
            hst.set(|p| p.tab = ReviewTab::Changes, window, cx)
        })
    });
    h.click("df-open:src/main.go");
    assert!(host.read_with(h.cx, |hst, _| {
        hst.events
            .borrow()
            .contains(&SurfaceEvent::OpenFile("src/main.go".into(), None))
    }));
}

#[gpui::test]
async fn commits_tab_uncommitted_row_is_vs_head_not_vs_base(cx: &mut TestAppContext) {
    let opts = MockOptions::default()
        .resolver("DiffVsBase", |_, _, _| {
            Reply::value(main_go_lines(json!([
                {"old": "", "new": "", "type": "hunk", "codeHtml": "@@ -1,1 +1,2 @@"},
                {"old": "1", "new": "1", "type": "ctx", "codeHtml": "package main"},
                {"old": "", "new": "2", "type": "add", "codeHtml": "// edited"},
            ])))
        })
        .resolver("DiffWorking", |_, _, _| {
            Reply::value(json!({ "files": [ {
                "path": "src/scratch.ts", "status": "a", "diff": {"add": 1, "del": 0},
                "lines": [
                    {"old": "", "new": "", "type": "hunk", "codeHtml": "@@ -0,0 +1,1 @@"},
                    {"old": "", "new": "1", "type": "add", "codeHtml": "export {}"},
                ],
            } ] }))
        });
    let (mut h, host) = surface(cx, opts, |_| {});
    assert!(h.has("cfile:live:src/main.go"));
    assert!(!h.has("cfile:live:src/scratch.ts"));
    h.cx.update(|window, cx| {
        host.update(cx, |hst, cx| {
            hst.set(|p| p.tab = ReviewTab::Commits, window, cx)
        })
    });
    h.click("commit:working");
    assert!(h.has("working-hd"));
    assert!(h.has("obs-review-vs"));
    let vs = host.read_with(h.cx, |hst, cx| {
        hst.surface
            .read(cx)
            .files_view()
            .read(cx)
            .props()
            .scope
            .clone()
    });
    assert_eq!(vs, crate::files_changed::FilesScope::Working);
    assert!(h.has("cfile:live:src/scratch.ts"));
    assert!(!h.has("cfile:live:src/main.go"));
    h.hover("hline:src/scratch.ts:add:right:1");
    assert!(h.has("addc:src/scratch.ts:right:1"));
    h.click("cback");
    assert!(h.has("review:commits"));
    h.cx.update(|window, cx| {
        host.update(cx, |hst, cx| {
            hst.set(|p| p.tab = ReviewTab::Changes, window, cx)
        })
    });
    assert!(h.has("cfile:live:src/main.go"));
    assert!(!h.has("working-hd"));
}

#[gpui::test]
async fn clean_worktree_hides_the_uncommitted_commits_row(cx: &mut TestAppContext) {
    let (mut h, _host) = surface(cx, MockOptions::default(), |p| {
        p.working_dirty_count = 0;
        p.tab = ReviewTab::Commits;
    });
    assert!(!h.has("commit:working"));
    assert!(h.has("commit:Add feature"));
}

#[gpui::test]
async fn working_copy_review_includes_the_commits_tab(cx: &mut TestAppContext) {
    let (mut h, host) = surface(cx, MockOptions::default(), |p| {
        p.base_label = String::new();
        p.tab = ReviewTab::Commits;
    });
    assert!(h.has("commit:working"));
    assert!(h.has("commit:Add feature"));
    assert!(h.has("commit:Older work"));
    let hint = host.read_with(h.cx, |hst, cx| {
        hst.surface
            .read(cx)
            .commits_view()
            .read(cx)
            .base_label
            .clone()
    });
    assert!(hint.is_empty(), "Recent commits on this branch: no base");
    h.click("commit:Add feature");
    assert!(h.has("cback"));
    assert!(h.has("commit-hd"));
}

#[gpui::test]
async fn commit_review_stays_open_when_files_are_edited(cx: &mut TestAppContext) {
    let (mut h, host) = surface(cx, MockOptions::default(), |p| p.tab = ReviewTab::Commits);
    h.click("commit:Add feature");
    assert!(h.has("cback"));
    // Same-string workspaceId re-pass and a live refresh must not clear the commit scope.
    h.cx.update(|window, cx| host.update(cx, |hst, cx| hst.set(|p| p.refresh += 1, window, cx)));
    assert!(h.has("cback"));
    assert!(h.has("commit-hd"));
    assert!(!h.has("review:commits"));
    h.click("cback");
    assert!(h.has("review:commits"));
    h.cx.update(|window, cx| host.update(cx, |hst, cx| hst.set(|p| p.refresh += 1, window, cx)));
    assert!(h.has("review:commits"));
    assert!(h.has("commit:Add feature"));
}

#[gpui::test]
async fn collapsing_a_files_changed_card_shrinks_it_to_the_header(cx: &mut TestAppContext) {
    let (mut h, _host) = surface(cx, MockOptions::default(), |_| {});
    assert!(h.has("hline:src/main.go:add:right:2"));
    let expanded = h.bounds("cfile:live:src/main.go").expect("card");
    assert!(f32::from(expanded.size.height) > 80.);
    h.click("df-chev:src/main.go");
    assert!(h.has("df-head:src/main.go:collapsed"));
    assert!(!h.has("hline:src/main.go:add:right:2"));
    let collapsed = h.bounds("cfile:live:src/main.go").expect("card");
    assert!(f32::from(collapsed.size.height) < 64.);
}

fn finding_opts() -> MockOptions {
    let notes = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    let (n1, n2) = (notes.clone(), notes.clone());
    MockOptions::default()
        .resolver("ListFindings", move |_, _, _| {
            Reply::value(serde_json::Value::Array(n1.lock().clone()))
        })
        .resolver("DiffVsBase", |_, _, _| {
            Reply::value(json!({ "files": [
                { "path": "src/main.go", "status": "m", "diff": {"add": 1, "del": 1}, "lines": [
                    {"old": "", "new": "", "type": "hunk", "codeHtml": "@@ -1,2 +1,2 @@"},
                    {"old": "1", "new": "1", "type": "ctx", "codeHtml": "package main"},
                    {"old": "2", "new": "", "type": "del", "codeHtml": "// gone"},
                    {"old": "", "new": "2", "type": "add", "codeHtml": "// edited"},
                ] },
                { "path": "src/util.go", "status": "a", "diff": {"add": 1, "del": 0}, "lines": [
                    {"old": "", "new": "", "type": "hunk", "codeHtml": "@@ -0,0 +1,1 @@"},
                    {"old": "", "new": "1", "type": "add", "codeHtml": "package util"},
                ] },
            ] }))
        })
        .resolver("CreateFinding", move |a, _, _| {
            let f = json!({
                "id": "f-user-1", "workspaceId": a[0], "file": a[1], "line": a[2],
                "endLine": a[4], "message": a[3], "status": "open", "severity": "info",
                "source": "user", "iteration": 1,
            });
            n2.lock().push(f.clone());
            Reply::value(f)
        })
}

#[gpui::test]
async fn review_line_note_becomes_a_finding(cx: &mut TestAppContext) {
    let (mut h, host) = surface(cx, finding_opts(), |p| p.count = 2);
    // Findings are new-side only: no `+` on deleted rows.
    assert!(!h.has("addc:src/main.go:left:2"));
    h.hover("hline:src/main.go:add:right:2");
    h.click("addc:src/main.go:right:2");
    assert!(h.has("finding-note-input"));
    h.type_text("check the timeout");
    h.click("compose-add");
    h.wait(50);
    assert!(h.has("icmt-finding:f-user-1"));
    assert!(h.has("obs-finding-flag:src/main.go"));
    assert!(h.has("fl-row:file:src/util.go"));
    h.click("button:Show files with open findings:false");
    assert!(h.has("fl-row:file:src/main.go"));
    assert!(!h.has("fl-row:file:src/util.go"));
    let only = host.read_with(h.cx, |hst, cx| {
        hst.surface.read(cx).files_view().read(cx).findings_only()
    });
    assert!(only);
}

#[gpui::test]
async fn drag_on_the_comment_control_selects_a_multiline_finding(cx: &mut TestAppContext) {
    let notes = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<serde_json::Value>::new()));
    let (n1, n2) = (notes.clone(), notes.clone());
    let opts = MockOptions::default()
        .resolver("ListFindings", move |_, _, _| {
            Reply::value(serde_json::Value::Array(n1.lock().clone()))
        })
        .resolver("DiffVsBase", |_, _, _| {
            Reply::value(main_go_lines(json!([
                {"old": "", "new": "", "type": "hunk", "codeHtml": "@@ -1,1 +1,4 @@"},
                {"old": "1", "new": "1", "type": "ctx", "codeHtml": "package main"},
                {"old": "", "new": "2", "type": "add", "codeHtml": "func a() {}"},
                {"old": "", "new": "3", "type": "add", "codeHtml": "func b() {}"},
                {"old": "", "new": "4", "type": "add", "codeHtml": "func c() {}"},
            ])))
        })
        .resolver("CreateFinding", move |a, _, _| {
            let f = json!({
                "id": "f-user-span", "workspaceId": a[0], "file": a[1], "line": a[2],
                "message": a[3], "endLine": a[4], "status": "open", "severity": "info",
                "source": "user", "iteration": 1,
            });
            n2.lock().push(f.clone());
            Reply::value(f)
        });
    let (mut h, host) = surface(cx, opts, |_| {});
    h.cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("before".into())));
    h.drag("addc:src/main.go:right:2", "addc:src/main.go:right:4");
    assert!(h.has("finding-note-input"));
    // `.diff-file.selecting { user-select: none }`: the drag selected lines, not text.
    h.cx.simulate_keystrokes("secondary-c");
    let clip =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(clip.as_deref(), Some("before"));
    let df = host
        .read_with(h.cx, |hst, cx| {
            hst.surface
                .read(cx)
                .files_view()
                .read(cx)
                .stack()
                .diff_file("src/main.go")
                .cloned()
        })
        .expect("diff file");
    let placeholder = df.read_with(h.cx, |d, cx| {
        let _ = cx;
        d.compose_placeholder().to_string()
    });
    assert!(placeholder.contains("lines 2"), "{placeholder}");
    let row2 = df
        .read_with(h.cx, |d, cx| d.row_classes(2, DiffSide::Right, cx))
        .expect("row 2");
    let row4 = df
        .read_with(h.cx, |d, cx| d.row_classes(4, DiffSide::Right, cx))
        .expect("row 4");
    assert!(row2.drafting);
    assert!(row4.draft_end);
    h.type_text("this span should stay together");
    h.click("compose-add");
    h.wait(50);
    assert!(h.has("icmt-finding:f-user-span"));
    let created = notes.lock()[0].clone();
    assert_eq!(
        (created["line"].as_i64(), created["endLine"].as_i64()),
        (Some(2), Some(4))
    );
    let df = host
        .read_with(h.cx, |hst, cx| {
            hst.surface
                .read(cx)
                .files_view()
                .read(cx)
                .stack()
                .diff_file("src/main.go")
                .cloned()
        })
        .expect("diff file");
    for (n, noted, end) in [(2, true, false), (3, true, false), (4, true, true)] {
        let f = df
            .read_with(h.cx, |d, cx| d.row_classes(n, DiffSide::Right, cx))
            .expect("row");
        assert_eq!((f.noted, f.note_end), (noted, end), "line {n}");
    }
    let _ = OpenLine { old_n: 0, new_n: 0 };
}

#[gpui::test]
async fn review_changes_survives_scrolling_a_large_mixed_size_diff(cx: &mut TestAppContext) {
    let files = crate::gallery::large_files(300);
    let opts = MockOptions::default().resolver("DiffVsBase", move |_, _, _| {
        Reply::value(json!({ "files": files.clone() }))
    });
    let (mut h, host) = surface(cx, opts, |p| p.count = 300);
    let count = host.read_with(h.cx, |hst, cx| {
        hst.surface
            .read(cx)
            .files_view()
            .read(cx)
            .stack()
            .files()
            .len()
    });
    assert_eq!(count, 300);
    let pane = host.read_with(h.cx, |hst, cx| {
        hst.surface
            .read(cx)
            .files_view()
            .read(cx)
            .stack()
            .pane
            .clone()
    });
    for frac in [0.3, 0.8, 0.95, 0.4, 0.99, 0.2, 0., 0.6] {
        let top = pane.max_scroll() * frac;
        pane.set_scroll_top(top);
        h.settle();
    }
    for i in 0..60 {
        let top = pane.scroll_top() + if i % 4 == 0 { -500. } else { 300. };
        pane.set_scroll_top(top);
        h.settle();
    }
    let live = host.read_with(h.cx, |hst, cx| {
        hst.surface
            .read(cx)
            .files_view()
            .read(cx)
            .stack()
            .hydrated()
            .len()
    });
    assert!(live > 0 && live <= crate::review_window::REVIEW_HYDRATE_MAX);
}

/// A stuck file header sits at the pane top at every scroll step.
#[gpui::test]
async fn a_stuck_file_header_holds_the_pane_top_while_scrolling(cx: &mut TestAppContext) {
    let files = crate::gallery::large_files(40);
    let paths: Vec<String> = files
        .as_array()
        .expect("files")
        .iter()
        .map(|f| f["path"].as_str().expect("path").to_string())
        .collect();
    let opts = MockOptions::default().resolver("DiffVsBase", move |_, _, _| {
        Reply::value(json!({ "files": files.clone() }))
    });
    let (mut h, host) = surface(cx, opts, |p| p.count = 40);
    let pane = host.read_with(h.cx, |hst, cx| {
        hst.surface
            .read(cx)
            .files_view()
            .read(cx)
            .stack()
            .pane
            .clone()
    });
    h.settle();
    let view = h.cx.debug_bounds("diffscroll").expect("pane");
    let mut checked = 0;
    for _ in 0..40 {
        pane.set_scroll_top(pane.scroll_top() + 37.);
        h.cx.update(|window, _| window.refresh());
        h.cx.run_until_parked();
        // The card spanning the pane top, well past its header, has its header at the top.
        for p in &paths {
            let card = format!("cfile:live:{p}");
            let Some(card) = h.cx.debug_bounds(Box::leak(card.into_boxed_str())) else {
                continue;
            };
            if card.top() < view.top() - gpui::px(20.) && card.bottom() > view.top() + gpui::px(80.)
            {
                let head = format!("df-head:{p}:open");
                let head =
                    h.cx.debug_bounds(Box::leak(head.into_boxed_str()))
                        .expect("header");
                assert!(
                    (head.top() - view.top()).abs() < gpui::px(0.5),
                    "{head:?} under {view:?}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 5, "only {checked} frames had a stuck header");
}

#[gpui::test]
async fn a_huge_diff_mounts_only_the_cards_near_the_viewport(cx: &mut TestAppContext) {
    let files = crate::gallery::large_files(2000);
    let last = files.as_array().and_then(|a| a.last()).expect("files")["path"]
        .as_str()
        .expect("path")
        .to_string();
    let opts = MockOptions::default().resolver("DiffVsBase", move |_, _, _| {
        Reply::value(json!({ "files": files.clone() }))
    });
    let (mut h, host) = surface(cx, opts, |p| p.count = 2000);
    let stub = |p: &str| format!("cfile:stub:{p}");
    h.settle();
    let mut mounted = 0;
    for i in 0..2000 {
        let p = format!("src/pkg{}/file{i}.ts", i % 9);
        for sel in [stub(&p), format!("cfile:live:{p}")] {
            let sel: &'static str = Box::leak(sel.into_boxed_str());
            if h.cx.debug_bounds(sel).is_some() {
                mounted += 1;
            }
        }
    }
    assert!(mounted < 200, "{mounted} cards mounted");
    assert!(!h.has(&stub(&last)));
    // The unmounted cards keep their place: selecting the last one scrolls to it.
    host.update(h.cx, |hst, cx| {
        hst.surface.update(cx, |s, cx| {
            s.files_view().update(cx, |f, cx| f.select_path(&last, cx))
        })
    });
    h.settle();
    h.settle();
    let pane = h.bounds("diffscroll").expect("pane");
    let card = h
        .bounds(&stub(&last))
        .or_else(|| h.bounds(&format!("cfile:live:{last}")))
        .expect("the selected card mounts");
    assert!(
        card.top() >= pane.top() - gpui::px(1.) && card.top() < pane.bottom(),
        "{card:?} in {pane:?}"
    );
}

/// A reload that brings back the same diff (a watcher event, a commit draft written while
/// typing) leaves the review, and the Observatory that hosts it, as they were.
#[gpui::test]
fn a_reload_that_finds_the_same_diff_does_not_notify(cx: &mut TestAppContext) {
    let (h, host) = surface(cx, MockOptions::default(), |_| {});
    let files =
        h.cx.update(|_, cx| host.read(cx).surface.read(cx).files_view().clone());
    let n = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = n.clone();
    let _sub =
        h.cx.update(|_, cx| cx.observe(&files, move |_, _| seen.set(seen.get() + 1)));
    h.cx.update(|_, cx| files.update(cx, |f, cx| f.load(cx)));
    h.cx.run_until_parked();
    assert_eq!(n.get(), 0);
}

/// Code selects across diff rows as WebKit selects across `.hline` grid cells, and ⌘C copies
/// the same text (line-number cells included, empty ones adding nothing).
#[gpui::test]
async fn code_selects_across_rows_and_copies_like_webkit(cx: &mut TestAppContext) {
    let opts = MockOptions::default().resolver("DiffVsBase", |_, _, _| {
        Reply::value(main_go_lines(json!([
            {"old": "", "new": "", "type": "hunk", "codeHtml": "@@ -1,1 +1,4 @@"},
            {"old": "1", "new": "1", "type": "ctx", "codeHtml": "package main"},
            {"old": "", "new": "2", "type": "add", "codeHtml": "func a() {}"},
            {"old": "", "new": "3", "type": "add", "codeHtml": "func b() {}"},
            {"old": "", "new": "4", "type": "add", "codeHtml": "func c() {}"},
        ])))
    });
    let (mut h, _host) = surface(cx, opts, |_| {});
    let r2 = h.bounds("hline:src/main.go:add:right:2").expect("row 2");
    let r4 = h.bounds("hline:src/main.go:add:right:4").expect("row 4");
    // From the start of row 2's code (two 46px gutters, 14px padding) to past row 4's end.
    let from = gpui::point(r2.left() + gpui::px(46. + 46. + 14. + 0.5), r2.center().y);
    let to = gpui::point(r4.right() - gpui::px(40.), r4.center().y);
    let m = gpui::Modifiers::none();
    h.cx.simulate_mouse_move(from, None, m);
    h.cx.simulate_mouse_down(from, gpui::MouseButton::Left, m);
    for i in 1..=4 {
        let p = from + (to - from) * (i as f32 / 4.);
        h.cx.simulate_mouse_move(p, Some(gpui::MouseButton::Left), m);
        h.cx.run_until_parked();
    }
    h.cx.simulate_mouse_up(to, gpui::MouseButton::Left, m);
    h.cx.run_until_parked();
    h.keys("secondary-c");
    let copied =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(
        copied.as_deref(),
        Some("func a() {}\n3\nfunc b() {}\n4\nfunc c() {}")
    );
}
