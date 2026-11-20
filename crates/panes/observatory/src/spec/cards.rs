//! The review's Uncommitted cards as editable diffs: which cards edit, typing and saving,
//! unsaved text across a scroll and with the editor tabs, and the index after an edit.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use gpui::TestAppContext;
use recur_domain::{FileContent, FileDiffResult};
use recur_services::fake::Overrides;
use recur_view_review::review_diff::ReviewTab;
use serde_json::json;

use super::harness::*;

type Shared<T> = Arc<parking_lot::Mutex<T>>;

/// A worktree of text files changed against HEAD: the review lists every file whose disk
/// text differs from HEAD, `+` one per changed line.
#[derive(Clone, Default)]
struct Tree {
    head: Shared<BTreeMap<String, String>>,
    disk: Shared<BTreeMap<String, String>>,
    staged: Shared<Vec<String>>,
    unstaged: Shared<Vec<String>>,
    writes: Calls,
}

fn lines(n: usize, tag: &str) -> String {
    (1..=n).map(|i| format!("line {i} {tag}\n")).collect()
}

impl Tree {
    fn new(files: &[(&str, &str, &str)]) -> Self {
        let t = Tree::default();
        for (p, head, disk) in files {
            t.head.lock().insert((*p).into(), (*head).into());
            t.disk.lock().insert((*p).into(), (*disk).into());
        }
        t
    }

    fn changed(&self) -> Vec<(String, i64)> {
        let (head, disk) = (self.head.lock(), self.disk.lock());
        disk.iter()
            .filter(|(p, d)| head.get(*p) != Some(*d))
            .map(|(p, d)| {
                let h = head.get(p).cloned().unwrap_or_default();
                let add = d
                    .lines()
                    .zip(h.lines().chain(std::iter::repeat("")))
                    .filter(|(a, b)| a != b)
                    .count() as i64;
                (p.clone(), add)
            })
            .collect()
    }

    fn opts(&self) -> Overrides {
        let (t1, t2, t3, t4, t5) = (
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
        );
        let (staged, unstaged) = (self.staged.clone(), self.unstaged.clone());
        Overrides::default()
            .working_status(move |_, _| {
                Ok(t1
                    .changed()
                    .into_iter()
                    .map(|(p, _)| (p, "m".to_string()))
                    .collect())
            })
            .staged_files(move |_, _| Ok(staged.lock().clone()))
            .unstaged_files(move |_, _| Ok(unstaged.lock().clone()))
            .diff_working(move |_, _| {
                let files: Vec<_> = t2
                    .changed()
                    .into_iter()
                    .map(|(p, add)| {
                        let status = if p.ends_with("gone.go") { "d" } else { "m" };
                        json!({"path": p, "status": status, "diff": {"add": add, "del": 0},
                        "lines": [
                            {"old": "", "new": "", "type": "hunk", "codeHtml": "@@ -1 +1 @@"},
                            {"old": "", "new": "1", "type": "add", "codeHtml": "x"},
                        ]})
                    })
                    .collect();
                Ok(serde_json::from_value::<FileDiffResult>(json!({ "files": files })).unwrap())
            })
            .file_at_head(move |_, (_, _, p)| {
                Ok(t3.head.lock().get(&p).cloned().unwrap_or_default())
            })
            .read_file(move |_, (_, wid, p)| {
                let body = t4.disk.lock().get(&p).cloned().unwrap_or_default();
                let mut f = file(&p, "go", &body, !wid.is_empty());
                if p.ends_with(".png") {
                    f = FileContent {
                        mime: "image/png".into(),
                        content: String::new(),
                        ..f
                    };
                }
                if p.starts_with("big") {
                    f = FileContent {
                        content: String::new(),
                        size: 400 << 10,
                        ..f
                    };
                }
                Ok(f)
            })
            .write_file(move |_, (_, _, p, body)| {
                push(&t5.writes, &[&p, &body]);
                t5.disk.lock().insert(p.clone(), body);
                // Git sees the new edit on top of what was staged.
                if t5.staged.lock().contains(&p) {
                    t5.unstaged.lock().push(p);
                }
                Ok(())
            })
    }
}

fn wide<'a>(cx: &'a mut TestAppContext, tree: &Tree) -> H<'a> {
    let mut h = mount(cx, tree.opts(), |_| {}, |_| {});
    h.click("obs-left-toggle");
    h.click("obs-review-toggle");
    assert_eq!(
        h.read(|o, cx| o.review_strip_tab(cx)),
        ReviewTab::Uncommitted
    );
    h
}

fn card_doc(h: &mut H, path: &str) -> Option<String> {
    h.read(|o, cx| o.card_editor_for(path).map(|e| e.read(cx).peek_doc()))
}

fn focus_card(h: &mut H, path: &str) {
    let p = path.to_string();
    h.update(move |o, window, cx| {
        let e = o.card_editor_for(&p).cloned().expect("card editor");
        e.update(cx, |e, cx| e.focus(window, cx));
    });
    h.settle();
}

fn toggle_review(h: &mut H) {
    h.update(|o, _, cx| o.click_review_toggle(cx));
    h.settle();
}

/// Past the editor's doc-change debounce.
fn debounce(h: &mut H) {
    h.wait(recur_editor::view::EMIT_DEBOUNCE_MS + 30);
}

#[gpui::test]
async fn uncommitted_text_cards_are_editable_and_the_rest_stay_read_only(cx: &mut TestAppContext) {
    let tree = Tree::new(&[
        ("src/a.go", "package a\n", "package a\n\nfunc A() {}\n"),
        ("src/gone.go", "package gone\n", ""),
        ("art/logo.png", "png", "png2"),
        ("big.txt", "", "big"),
    ]);
    let mut h = wide(cx, &tree);
    assert!(h.has("df-editor:src/a.go"), "a text card edits");
    assert_eq!(
        card_doc(&mut h, "src/a.go").as_deref(),
        Some("package a\n\nfunc A() {}\n"),
        "the working tree's text, against HEAD"
    );
    for ro in ["src/gone.go", "art/logo.png", "big.txt"] {
        assert!(h.has(&format!("df-head:{ro}:open")), "{ro} has a card");
        assert!(!h.has(&format!("df-editor:{ro}")), "{ro} stays read-only");
    }
    // Changes is the branch against its base: read-only, and the cards' editors go.
    h.update(|o, _, cx| o.set_review_tab(ReviewTab::Changes, cx));
    h.settle();
    assert!(!h.has("df-editor:src/a.go"));
    assert!(h.read(|o, _| o.cards.is_empty()));
}

#[gpui::test]
async fn a_read_only_workspace_keeps_read_only_cards(cx: &mut TestAppContext) {
    let tree = Tree::new(&[("src/a.go", "a\n", "b\n")]);
    let opts = tree
        .opts()
        .read_file(|_, (_, _, p)| Ok(file(&p, "go", "b\n", false)));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-left-toggle");
    h.click("obs-review-toggle");
    assert!(h.has("df-head:src/a.go:open"));
    assert!(!h.has("df-editor:src/a.go"));
}

#[gpui::test]
async fn typing_in_a_card_marks_it_unsaved_and_cmd_s_saves_it(cx: &mut TestAppContext) {
    let tree = Tree::new(&[
        ("src/a.go", "package a\n", "package a\n// one\n"),
        ("src/b.go", "package b\n", "package b\n// two\n"),
    ]);
    let mut h = wide(cx, &tree);
    focus_card(&mut h, "src/a.go");
    h.type_text("x");
    assert!(
        card_doc(&mut h, "src/a.go").is_some_and(|d| d.starts_with('x')),
        "the key reached the card"
    );
    assert!(h.has("df-unsaved:src/a.go"));
    assert!(!h.has("df-unsaved:src/b.go"));
    h.keys("cmd-s");
    h.wait(50);
    let want = "xpackage a\n// one\n";
    assert_eq!(tree.writes.lock().clone(), [strings(&["src/a.go", want])]);
    assert!(!h.has("df-unsaved:src/a.go"), "saved");
    // The git refresh after the save reloads the diff: the card's count follows the file.
    h.wait(crate::obs::REVIEW_REFRESH_MS + 50);
    let add = h.read(|o, cx| {
        let r = o.review_surface.clone().expect("review");
        let f = r.read(cx).files_view().read(cx);
        f.files()
            .iter()
            .find(|f| f.path == "src/a.go")
            .map(|f| f.diff.add)
    });
    assert_eq!(add, Some(2));
    assert_eq!(card_doc(&mut h, "src/a.go").as_deref(), Some(want));
}

#[gpui::test]
async fn a_card_scrolled_away_and_back_keeps_its_unsaved_text(cx: &mut TestAppContext) {
    let files: Vec<(String, String, String)> = (0..14)
        .map(|i| {
            (
                format!("src/f{i:02}.go"),
                lines(40, "head"),
                lines(40, "disk"),
            )
        })
        .collect();
    let refs: Vec<(&str, &str, &str)> = files
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
        .collect();
    let tree = Tree::new(&refs);
    let mut h = wide(cx, &tree);
    focus_card(&mut h, "src/f00.go");
    h.type_text("kept");
    debounce(&mut h);
    let scroll = |h: &mut H, top: f32| {
        h.update(move |o, _, cx| {
            let r = o.review_surface.clone().expect("review");
            let files = r.read(cx).files_view().clone();
            files.update(cx, |f, cx| {
                f.stack().pane.set_scroll_top(top);
                cx.notify();
            });
        });
        h.settle();
    };
    scroll(&mut h, 1e6);
    assert!(
        h.read(|o, _| o.card_editor_for("src/f00.go").is_none()),
        "a far card's editor is released"
    );
    assert!(
        h.read(|o, _| o.stashed_buffers.contains_key("src/f00.go")),
        "its text is kept with the editor's unsaved buffers"
    );
    assert!(
        h.open_files().contains(&"src/f00.go".to_string()),
        "as an open tab, so it persists and asks before it closes"
    );
    scroll(&mut h, 0.);
    let doc = card_doc(&mut h, "src/f00.go").expect("mounted again");
    assert!(doc.starts_with("kept"), "{doc}");
    assert!(h.has("df-unsaved:src/f00.go"));
}

#[gpui::test]
async fn a_card_and_the_editor_tab_of_its_file_share_one_buffer(cx: &mut TestAppContext) {
    let tree = Tree::new(&[("src/a.go", "package a\n", "package a\n// edit\n")]);
    let mut h = mount(cx, tree.opts(), |_| {}, |_| {});
    h.click("obs-left-toggle");
    h.update(|o, window, cx| {
        o.open_path(crate::obs::OpenRequest::new("src/a.go"), window, cx)
            .detach()
    });
    h.settle();
    h.focus_editor();
    h.type_text("tab ");
    debounce(&mut h);
    h.click("obs-review-toggle");
    assert_eq!(
        card_doc(&mut h, "src/a.go").as_deref(),
        Some("tab package a\n// edit\n"),
        "the card opens on the tab's unsaved text"
    );
    assert!(h.has("df-unsaved:src/a.go"));
    focus_card(&mut h, "src/a.go");
    h.keys("end");
    h.type_text(" card");
    debounce(&mut h);
    assert!(
        h.read(|o, _| o.tab_is_dirty("src/a.go")),
        "the tab shows it unsaved"
    );
    assert!(
        h.editor_doc().starts_with("tab package a card"),
        "the tab follows the card: {:?}",
        h.editor_doc()
    );
    // Back in the editor, a key there shows in the card.
    toggle_review(&mut h);
    h.focus_editor();
    h.keys("cmd-end");
    h.type_text("!");
    debounce(&mut h);
    toggle_review(&mut h);
    let card = card_doc(&mut h, "src/a.go").expect("card");
    assert!(card.ends_with("!"), "{card:?}");
}

#[gpui::test]
async fn editing_a_checked_file_leaves_it_partly_staged(cx: &mut TestAppContext) {
    let tree = Tree::new(&[("src/a.go", "package a\n", "package a\n// staged\n")]);
    tree.staged.lock().push("src/a.go".into());
    let mut h = wide(cx, &tree);
    assert!(h.has("fl-check:src/a.go:On"));
    focus_card(&mut h, "src/a.go");
    h.type_text("y");
    h.keys("cmd-s");
    h.wait(crate::obs::REVIEW_REFRESH_MS + 50);
    assert!(h.has("fl-check:src/a.go:Mixed"), "not staged again");
    let staged: HashSet<String> = tree.staged.lock().iter().cloned().collect();
    assert_eq!(staged, ["src/a.go".to_string()].into());
}
