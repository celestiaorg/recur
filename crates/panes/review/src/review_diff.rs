//! The Files changed / Commits layout prefs (the same local storage
//! keys as the PR detail view), rename labels and the no-op refresh check.

use std::rc::Rc;

use recur_domain::FileDiff;
use recur_view_core::LocalStorage;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffLayout {
    #[default]
    Unified,
    Split,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReviewTab {
    #[default]
    Changes,
    Commits,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileListMode {
    Flat,
    #[default]
    Tree,
}

const DIFF_LAYOUT_KEY: &str = "recur.prDiffLayout";
const FILES_VIEW_KEY: &str = "recur.prFilesView";
const REVIEW_SIDEBAR_KEY: &str = "recur.reviewSidebar";
const REVIEW_TAB_KEY: &str = "recur.obsReviewTab";
const FILE_LIST_MODE_KEY: &str = "recur.reviewFileListMode";

pub fn read_diff_layout(s: &LocalStorage) -> DiffLayout {
    match s.get(DIFF_LAYOUT_KEY) {
        Some("split") => DiffLayout::Split,
        _ => DiffLayout::Unified,
    }
}

pub fn write_diff_layout(s: &mut LocalStorage, next: DiffLayout) {
    let v = match next {
        DiffLayout::Unified => "unified",
        DiffLayout::Split => "split",
    };
    s.set(DIFF_LAYOUT_KEY, v);
}

pub fn read_review_sidebar(s: &LocalStorage) -> bool {
    match s.get(REVIEW_SIDEBAR_KEY) {
        Some("on") => return true,
        Some("off") => return false,
        _ => {}
    }
    // migrate: old list/stack pref — list had a sidebar, stack didn't
    match s.get(FILES_VIEW_KEY) {
        Some("list") => true,
        Some("stack") => false,
        _ => true,
    }
}

pub fn write_review_sidebar(s: &mut LocalStorage, open: bool) {
    s.set(REVIEW_SIDEBAR_KEY, if open { "on" } else { "off" });
}

pub fn read_review_tab(s: &LocalStorage) -> ReviewTab {
    match s.get(REVIEW_TAB_KEY) {
        Some("commits") => ReviewTab::Commits,
        _ => ReviewTab::Changes,
    }
}

pub fn write_review_tab(s: &mut LocalStorage, next: ReviewTab) {
    let v = match next {
        ReviewTab::Changes => "changes",
        ReviewTab::Commits => "commits",
    };
    s.set(REVIEW_TAB_KEY, v);
}

pub fn read_file_list_mode(s: &LocalStorage) -> FileListMode {
    match s.get(FILE_LIST_MODE_KEY) {
        Some("flat") => FileListMode::Flat,
        _ => FileListMode::Tree,
    }
}

pub fn write_file_list_mode(s: &mut LocalStorage, next: FileListMode) {
    let v = match next {
        FileListMode::Flat => "flat",
        FileListMode::Tree => "tree",
    };
    s.set(FILE_LIST_MODE_KEY, v);
}

fn dir_of(p: &str) -> &str {
    p.rfind('/').map_or("", |i| &p[..i])
}

fn base_of(p: &str) -> &str {
    p.rfind('/').map_or(p, |i| &p[i + 1..])
}

/// Header/list label: `dir/old.ts → new.ts` when the parent directory is unchanged.
pub fn file_diff_path_label(path: &str, old_path: &str) -> String {
    if old_path.is_empty() || old_path == path {
        return path.to_string();
    }
    if dir_of(old_path) == dir_of(path) {
        return format!("{old_path} → {}", base_of(path));
    }
    format!("{old_path} → {path}")
}

/// Tree-row label: basename, or `old → new` (full old path when the directory changed).
pub fn file_diff_tree_label(path: &str, old_path: &str) -> String {
    let name = base_of(path);
    if old_path.is_empty() || old_path == path {
        return name.to_string();
    }
    if dir_of(old_path) == dir_of(path) {
        return format!("{} → {name}", base_of(old_path));
    }
    format!("{old_path} → {name}")
}

/// [`file_diff_path_label`] for a file.
pub fn path_label(f: &FileDiff) -> String {
    file_diff_path_label(&f.path, &f.old_path)
}

fn file_diff_body_equal(a: &FileDiff, b: &FileDiff) -> bool {
    if a.path != b.path || a.old_path != b.old_path || a.status != b.status {
        return false;
    }
    if a.diff.add != b.diff.add || a.diff.del != b.diff.del {
        return false;
    }
    let al = a.lines.as_deref().unwrap_or_default();
    let bl = b.lines.as_deref().unwrap_or_default();
    al.len() == bl.len()
        && al.iter().zip(bl).all(|(x, y)| {
            x.r#type == y.r#type && x.old == y.old && x.new == y.new && x.code_html == y.code_html
        })
}

/// Keep previous file identities when a live refresh is a no-op. `None` is "keep `prev` as is"
/// (the TS function returned the very same array).
pub fn reuse_file_diffs(prev: &[Rc<FileDiff>], next: Vec<FileDiff>) -> Option<Vec<Rc<FileDiff>>> {
    if next.is_empty() {
        return if prev.is_empty() {
            None
        } else {
            Some(Vec::new())
        };
    }
    if prev.is_empty() {
        return Some(next.into_iter().map(Rc::new).collect());
    }
    let mut changed = prev.len() != next.len();
    let out: Vec<Rc<FileDiff>> = next
        .into_iter()
        .enumerate()
        .map(|(i, f)| match prev.iter().find(|p| p.path == f.path) {
            Some(old) if file_diff_body_equal(old, &f) => {
                if prev.get(i).is_none_or(|p| !Rc::ptr_eq(p, old)) {
                    changed = true;
                }
                old.clone()
            }
            _ => {
                changed = true;
                Rc::new(f)
            }
        })
        .collect();
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_domain::{DiffLine, DiffStat};

    #[test]
    fn defaults_to_unified_changes_tab_tree_file_list_sidebar_on() {
        let s = LocalStorage::in_memory();
        assert_eq!(read_diff_layout(&s), DiffLayout::Unified);
        assert_eq!(read_review_tab(&s), ReviewTab::Changes);
        assert_eq!(read_file_list_mode(&s), FileListMode::Tree);
        assert!(read_review_sidebar(&s));
    }

    #[test]
    fn round_trips_layout_prefs() {
        let mut s = LocalStorage::in_memory();
        write_diff_layout(&mut s, DiffLayout::Split);
        write_review_tab(&mut s, ReviewTab::Commits);
        write_file_list_mode(&mut s, FileListMode::Flat);
        write_review_sidebar(&mut s, false);
        assert_eq!(read_diff_layout(&s), DiffLayout::Split);
        assert_eq!(read_review_tab(&s), ReviewTab::Commits);
        assert_eq!(read_file_list_mode(&s), FileListMode::Flat);
        assert!(!read_review_sidebar(&s));
        write_review_sidebar(&mut s, true);
        assert!(read_review_sidebar(&s));
    }

    #[test]
    fn falls_back_when_stored_values_are_junk() {
        let mut s = LocalStorage::in_memory();
        s.set("recur.obsReviewTab", "nope");
        s.set("recur.reviewFileListMode", "grid");
        s.set("recur.reviewSidebar", "maybe");
        assert_eq!(read_review_tab(&s), ReviewTab::Changes);
        assert_eq!(read_file_list_mode(&s), FileListMode::Tree);
        assert!(read_review_sidebar(&s));
    }

    #[test]
    fn migrates_the_old_list_stack_pref_into_the_sidebar_default() {
        let mut s = LocalStorage::in_memory();
        s.set("recur.prFilesView", "stack");
        assert!(!read_review_sidebar(&s));
        s.set("recur.prFilesView", "list");
        assert!(read_review_sidebar(&s));
    }

    #[test]
    fn prefers_the_sidebar_key_over_the_migrated_list_stack_pref() {
        let mut s = LocalStorage::in_memory();
        s.set("recur.prFilesView", "stack");
        s.set("recur.reviewSidebar", "on");
        assert!(read_review_sidebar(&s));
    }

    fn file(path: &str, add: i64, code: &str) -> FileDiff {
        FileDiff {
            path: path.into(),
            status: "m".into(),
            diff: DiffStat { add, del: 0 },
            lines: Some(vec![DiffLine {
                r#type: "add".into(),
                new: "1".into(),
                code_html: code.into(),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    #[test]
    fn returns_the_previous_array_when_bodies_are_identical() {
        let prev = vec![Rc::new(file("a.ts", 1, "x"))];
        assert!(reuse_file_diffs(&prev, vec![file("a.ts", 1, "x")]).is_none());
    }

    #[test]
    fn keeps_object_identity_per_unchanged_path() {
        let a = Rc::new(file("a.ts", 1, "x"));
        let b = Rc::new(file("b.ts", 1, "y"));
        let prev = vec![a.clone(), b.clone()];
        let got = reuse_file_diffs(&prev, vec![file("a.ts", 1, "x"), file("b.ts", 2, "z")])
            .expect("changed");
        assert!(Rc::ptr_eq(&got[0], &a));
        assert!(!Rc::ptr_eq(&got[1], &b));
        assert_eq!(got[1].diff.add, 2);
    }

    #[test]
    fn keeps_the_previous_empty_array() {
        assert!(reuse_file_diffs(&[], Vec::new()).is_none());
    }

    #[test]
    fn path_label_returns_the_path_when_there_is_no_rename() {
        assert_eq!(file_diff_path_label("a.ts", ""), "a.ts");
        assert_eq!(file_diff_path_label("a.ts", "a.ts"), "a.ts");
    }

    #[test]
    fn path_label_shortens_the_new_side_when_the_directory_is_unchanged() {
        assert_eq!(
            file_diff_path_label(
                "apps/rindexer/migrations/0016.sql",
                "apps/rindexer/migrations/0015.sql"
            ),
            "apps/rindexer/migrations/0015.sql → 0016.sql"
        );
    }

    #[test]
    fn path_label_keeps_both_full_paths_when_the_directory_changes() {
        assert_eq!(file_diff_path_label("b/y.ts", "a/x.ts"), "a/x.ts → b/y.ts");
        assert_eq!(
            file_diff_path_label("apps/archive/a.ts", "apps/a.ts"),
            "apps/a.ts → apps/archive/a.ts"
        );
    }

    #[test]
    fn tree_label_returns_the_basename_when_there_is_no_rename() {
        assert_eq!(file_diff_tree_label("apps/a.ts", ""), "a.ts");
        assert_eq!(file_diff_tree_label("apps/a.ts", "apps/a.ts"), "a.ts");
    }

    #[test]
    fn tree_label_shows_old_to_new_basename_when_the_directory_is_unchanged() {
        assert_eq!(
            file_diff_tree_label(
                "apps/rindexer/migrations/0016.sql",
                "apps/rindexer/migrations/0015.sql"
            ),
            "0015.sql → 0016.sql"
        );
    }

    #[test]
    fn tree_label_keeps_the_old_path_when_the_directory_changes() {
        assert_eq!(
            file_diff_tree_label("apps/archive/a.ts", "apps/a.ts"),
            "apps/a.ts → a.ts"
        );
        assert_eq!(file_diff_tree_label("b/y.ts", "a/x.ts"), "a/x.ts → y.ts");
    }
}
