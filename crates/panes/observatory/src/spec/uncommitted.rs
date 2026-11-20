//! The review's Uncommitted tab as the Observatory hosts it: when it shows and where the
//! review lands, the checkboxes staging through the code service, discard with its confirm, the
//! commit composer, the Review button's count, and the entry points Source Control had.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::TestAppContext;
use recur_domain::{FileDiffResult, WorktreeCommitResult};
use recur_services::fake::Overrides;
use recur_view_review::review_diff::ReviewTab;
use serde_json::json;

use super::harness::*;
use crate::model::MainSurface;

type Shared<T> = Arc<parking_lot::Mutex<T>>;

/// A working tree whose status, index and edits on top of it the spec changes as it goes, with
/// every stage, commit and discard recorded as `[call, args…]`.
struct Repo {
    status: Shared<BTreeMap<String, String>>,
    staged: Shared<Vec<String>>,
    unstaged: Shared<Vec<String>>,
    calls: Calls,
}

impl Repo {
    fn new(files: &[&str]) -> Self {
        Repo {
            status: Arc::new(parking_lot::Mutex::new(statuses(
                &files.iter().map(|f| (*f, "m")).collect::<Vec<_>>(),
            ))),
            staged: Default::default(),
            unstaged: Default::default(),
            calls: Calls::default(),
        }
    }

    fn opts(&self) -> Overrides {
        let (st, st2) = (self.status.clone(), self.status.clone());
        let staged = self.staged.clone();
        let unstaged = self.unstaged.clone();
        let c = |name: &'static str, calls: &Calls| {
            let calls = calls.clone();
            move |args: Vec<String>| {
                let mut v = vec![name.to_owned()];
                v.extend(args);
                calls.lock().push(v);
            }
        };
        let (stage, unstage) = (c("stage", &self.calls), c("unstage", &self.calls));
        let (commit, commit_staged) = (c("commit", &self.calls), c("commit_staged", &self.calls));
        let (discard, discard_all) = (c("discard", &self.calls), c("discard_all", &self.calls));
        let done = || {
            Ok(WorktreeCommitResult {
                hash: "abc1234".into(),
                committed: true,
                ..Default::default()
            })
        };
        Overrides::default()
            .working_status(move |_, _| Ok(st.lock().clone()))
            .staged_files(move |_, _| Ok(staged.lock().clone()))
            .unstaged_files(move |_, _| Ok(unstaged.lock().clone()))
            .diff_working(move |_, _| {
                let files: Vec<_> = st2
                    .lock()
                    .keys()
                    .map(|p| {
                        json!({"path": p, "status": "m", "diff": {"add": 1, "del": 0}, "lines": [
                            {"old": "", "new": "", "type": "hunk", "codeHtml": "@@ -1,0 +1,1 @@"},
                            {"old": "", "new": "1", "type": "add", "codeHtml": "x"},
                        ]})
                    })
                    .collect();
                Ok(serde_json::from_value::<FileDiffResult>(json!({ "files": files })).unwrap())
            })
            .stage(move |_, (_, _, paths)| {
                stage(paths);
                Ok(())
            })
            .unstage(move |_, (_, _, paths)| {
                unstage(paths);
                Ok(())
            })
            .commit(move |_, (_, _, msg)| {
                commit(vec![msg]);
                done()
            })
            .commit_staged(move |_, (_, _, msg)| {
                commit_staged(vec![msg]);
                done()
            })
            .discard_file(move |_, (_, _, path)| {
                discard(vec![path]);
                Ok(())
            })
            .discard_all(move |_, _| {
                discard_all(Vec::new());
                Ok(())
            })
            .generate_commit_message(|_, _| Ok("feat: the generated message\n".into()))
    }

    fn take(&self) -> Vec<Vec<String>> {
        std::mem::take(&mut *self.calls.lock())
    }
}

fn call(v: &[&str]) -> Vec<String> {
    strings(v)
}

/// The Observatory with the chat dock hidden, so the review is wide enough to show its file
/// list beside the diffs.
fn wide<'a>(cx: &'a mut TestAppContext, repo: &Repo) -> H<'a> {
    let mut h = mount(cx, repo.opts(), |_| {}, |_| {});
    h.click("obs-left-toggle");
    h
}

fn refresh(h: &mut H) {
    h.update(|o, _, cx| o.refresh_working_status(cx).detach());
    h.settle();
}

fn tab(h: &mut H) -> ReviewTab {
    h.read(|o, cx| o.review_strip_tab(cx))
}

#[gpui::test]
async fn the_uncommitted_tab_shows_while_the_tree_is_dirty_and_falls_back_to_changes(
    cx: &mut TestAppContext,
) {
    let repo = Repo::new(&["src/a.go", "src/b.go"]);
    let mut h = wide(cx, &repo);
    h.click("obs-review-toggle");
    assert_eq!(
        tab(&mut h),
        ReviewTab::Uncommitted,
        "a dirty tree lands here"
    );
    assert!(h.has("tab-chip:review-uncommitted:on"));
    assert!(h.has("tab-chip:review-changes"));
    assert!(h.has("fl-check:src/a.go:Off"), "nothing is staged on open");
    // The last change is committed: the tab goes and the review shows Changes.
    repo.status.lock().clear();
    refresh(&mut h);
    assert_eq!(tab(&mut h), ReviewTab::Changes);
    assert!(!h.has("tab-chip:review-uncommitted"));
    assert!(h.has("tab-chip:review-changes:on"));
    assert!(!h.has("fl-check:src/a.go:Off"));
    // New edits bring the tab back without taking the review from Changes.
    repo.status.lock().insert("src/a.go".into(), "m".into());
    refresh(&mut h);
    assert!(h.has("tab-chip:review-uncommitted"));
    assert_eq!(tab(&mut h), ReviewTab::Changes);
    h.click("tab-chip:review-uncommitted");
    assert_eq!(tab(&mut h), ReviewTab::Uncommitted);
    assert!(h.has("fl-check:src/a.go:Off"));
}

#[gpui::test]
async fn opening_the_review_lands_on_uncommitted_when_dirty_and_on_changes_when_clean(
    cx: &mut TestAppContext,
) {
    let repo = Repo::new(&["src/a.go"]);
    let mut h = wide(cx, &repo);
    h.update(|o, _, cx| o.set_review_tab(ReviewTab::Commits, cx));
    h.click("obs-node:file:src/main.go");
    h.focus_editor();
    h.keys("secondary-alt-r");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    assert_eq!(tab(&mut h), ReviewTab::Uncommitted, "⌥⌘R on a dirty tree");
    h.keys("secondary-alt-r");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Editor);
    repo.status.lock().clear();
    refresh(&mut h);
    h.click("obs-review-toggle");
    assert_eq!(
        tab(&mut h),
        ReviewTab::Changes,
        "a clean tree opens on Changes"
    );
    assert!(!h.has("tab-chip:review-uncommitted"));
}

#[gpui::test]
async fn checkboxes_stage_and_unstage_files_and_folders(cx: &mut TestAppContext) {
    let repo = Repo::new(&["src/a.go", "src/b.go", "src/c.go"]);
    *repo.staged.lock() = strings(&["src/a.go", "src/b.go"]);
    *repo.unstaged.lock() = strings(&["src/b.go"]);
    let mut h = wide(cx, &repo);
    h.click("obs-review-toggle");
    assert!(h.has("fl-check:src/a.go:On"));
    assert!(h.has("fl-check:src/b.go:Mixed"), "staged with edits on top");
    assert!(h.has("fl-check:src/c.go:Off"));
    assert!(h.has("fl-check:src:Mixed"));
    h.click("fl-check:src/c.go:Off");
    h.click("fl-check:src/a.go:On");
    h.click("fl-check:src/b.go:Mixed");
    h.click("fl-check:src:Mixed");
    assert_eq!(
        repo.take(),
        [
            call(&["stage", "src/c.go"]),
            call(&["unstage", "src/a.go"]),
            call(&["stage", "src/b.go"]),
            call(&["stage", "src/a.go", "src/b.go", "src/c.go"]),
        ]
    );
    // The index moved: the checkboxes follow the next status refresh.
    *repo.staged.lock() = strings(&["src/a.go", "src/b.go", "src/c.go"]);
    repo.unstaged.lock().clear();
    refresh(&mut h);
    assert!(h.has("fl-check:src:On"));
    h.click("fl-check:src:On");
    assert_eq!(
        repo.take(),
        [call(&["unstage", "src/a.go", "src/b.go", "src/c.go"])]
    );
}

#[gpui::test]
async fn commit_takes_the_checked_files_or_everything_when_none_is_checked(
    cx: &mut TestAppContext,
) {
    let repo = Repo::new(&["src/a.go", "src/b.go"]);
    let mut h = wide(cx, &repo);
    assert!(h.update(|o, window, cx| o.focus_commit_msg(window, cx)));
    h.settle();
    assert!(h.has("obs-scm-foot"));
    h.type_text("all of it");
    h.keys("secondary-enter");
    assert_eq!(repo.take(), [call(&["commit", "all of it"])]);
    assert_eq!(h.read(|o, cx| o.inputs.commit_msg(cx)), "");
    *repo.staged.lock() = strings(&["src/a.go"]);
    refresh(&mut h);
    assert!(h.update(|o, window, cx| o.focus_commit_msg(window, cx)));
    h.type_text("just a");
    h.click("scm-commit");
    assert_eq!(repo.take(), [call(&["commit_staged", "just a"])]);
}

#[gpui::test]
async fn generate_writes_the_message_from_the_checked_files(cx: &mut TestAppContext) {
    let repo = Repo::new(&["src/a.go"]);
    let mut h = wide(cx, &repo);
    h.click("obs-review-toggle");
    // Nothing checked: nothing to describe.
    h.click("scm-gen");
    assert_eq!(h.read(|o, cx| o.inputs.commit_msg(cx)), "");
    *repo.staged.lock() = strings(&["src/a.go"]);
    refresh(&mut h);
    h.click("scm-gen");
    assert_eq!(
        h.read(|o, cx| o.inputs.commit_msg(cx)),
        "feat: the generated message"
    );
}

#[gpui::test]
async fn discard_asks_first_per_file_per_folder_and_for_everything(cx: &mut TestAppContext) {
    let repo = Repo::new(&["src/a.go", "src/b.go"]);
    let mut h = wide(cx, &repo);
    h.click("obs-review-toggle");
    let answer = |h: &mut H, needle: &str, button: &str| {
        let prompt = h.cx.pending_prompt().expect("a confirm");
        assert!(
            prompt.0.contains(needle) || prompt.1.contains(needle),
            "{prompt:?}"
        );
        h.cx.simulate_prompt_answer(button);
        h.settle();
    };
    h.hover("fl-row:file:src/a.go");
    h.click("fl-discard:src/a.go");
    answer(&mut h, "“src/a.go”", "Cancel");
    assert!(repo.take().is_empty(), "cancel keeps the changes");
    h.click("fl-discard:src/a.go");
    answer(&mut h, "“src/a.go”", "Discard");
    assert_eq!(repo.take(), [call(&["discard", "src/a.go"])]);
    h.hover("fl-row:dir:src");
    h.click("fl-discard:src");
    answer(&mut h, "2 files in “src”", "Discard");
    assert_eq!(
        repo.take(),
        [
            call(&["discard", "src/a.go"]),
            call(&["discard", "src/b.go"])
        ]
    );
    h.click("df-discard:src/b.go");
    answer(&mut h, "“src/b.go”", "Discard");
    assert_eq!(repo.take(), [call(&["discard", "src/b.go"])]);
    h.click("button:Discard all");
    answer(&mut h, "Discard all 2", "Discard all");
    assert_eq!(repo.take(), [call(&["discard_all"])]);
}

#[gpui::test]
async fn the_review_button_carries_the_uncommitted_count(cx: &mut TestAppContext) {
    let repo = Repo::new(&["src/a.go", "src/b.go", "src/c.go"]);
    let mut h = wide(cx, &repo);
    let badge = |h: &mut H| {
        let (b, t) = (h.bounds("obs-pane-badge:3"), h.bounds("obs-review-toggle"));
        b.zip(t).is_some_and(|(b, t)| t.intersects(&b))
    };
    assert!(badge(&mut h));
    repo.status.lock().clear();
    refresh(&mut h);
    assert!(!badge(&mut h));
    assert!(h.bounds("obs-review-toggle").is_some());
}

#[gpui::test]
async fn what_opened_source_control_opens_the_uncommitted_tab(cx: &mut TestAppContext) {
    let repo = Repo::new(&["src/a.go"]);
    let mut h = wide(cx, &repo);
    assert!(!h.has("panel-toggle:scm"), "no Source Control panel");
    h.click("obs-dirty-link");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    assert_eq!(tab(&mut h), ReviewTab::Uncommitted);
    h.click("tab-chip:review-changes");
    h.click("button:Hide file list");
    let focused = h.update(|o, window, cx| o.focus_commit_msg(window, cx));
    assert!(focused, "the message box takes the keys");
    assert_eq!(tab(&mut h), ReviewTab::Uncommitted);
    assert!(
        h.has("filelist"),
        "the composer's column comes back with it"
    );
    assert!(h.has("obs-scm-foot"));
}

#[gpui::test]
async fn a_conflicted_file_shows_its_bang_in_the_uncommitted_list(cx: &mut TestAppContext) {
    let repo = Repo::new(&["src/a.go", "src/b.go"]);
    repo.status.lock().insert("src/b.go".into(), "!".into());
    let mut h = wide(cx, &repo);
    refresh(&mut h);
    h.click("obs-review-toggle");
    assert!(h.has("fl-st:src/a.go:m"));
    assert!(h.has("fl-st:src/b.go:!"));
}

#[gpui::test]
async fn the_changes_tab_keeps_its_count_across_tab_switches(cx: &mut TestAppContext) {
    let repo = Repo::new(&["src/a.go"]);
    let branch = statuses(&[("src/a.go", "m"), ("src/b.go", "a"), ("src/c.go", "m")]);
    let opts = repo.opts().git_status(move |_, _| Ok(branch.clone()));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-left-toggle");
    refresh(&mut h);
    h.update(|o, _, cx| o.refresh_git_status(cx).detach());
    h.settle();
    h.click("obs-review-toggle");
    let badge = "tab-chip-badge:review-changes:3";
    assert!(h.has(badge), "on Uncommitted");
    h.click("tab-chip:review-changes");
    assert!(h.has(badge), "on Changes");
    if h.has("tab-chip:review-commits") {
        h.click("tab-chip:review-commits");
        assert!(h.has(badge), "on Commits");
    }
}
