//! The newest-first tape Review → Commits steps through: unpublished
//! work, then real commits.

use recur_domain::Commit;

use crate::commit_meta::commit_sha;

#[derive(Clone, Debug, PartialEq)]
pub enum TapeItem {
    Working,
    Commit(Box<Commit>),
}

pub fn review_tape(dirty: bool, commits: &[Commit]) -> Vec<TapeItem> {
    let mut items = Vec::with_capacity(commits.len() + 1);
    if dirty {
        items.push(TapeItem::Working);
    }
    items.extend(
        commits
            .iter()
            .cloned()
            .map(|c| TapeItem::Commit(Box::new(c))),
    );
    items
}

/// The open scope's index, -1 when none.
pub fn review_tape_index(tape: &[TapeItem], working_open: bool, sha: &str) -> isize {
    let at = if working_open {
        tape.iter().position(|i| *i == TapeItem::Working)
    } else if sha.is_empty() {
        None
    } else {
        tape.iter()
            .position(|i| matches!(i, TapeItem::Commit(c) if commit_sha(c) == sha))
    };
    at.map_or(-1, |i| i as isize)
}

/// `(older, newer)`.
pub fn can_step_on_tape(idx: isize, len: usize) -> (bool, bool) {
    (idx >= 0 && idx < len as isize - 1, idx > 0)
}

/// `dir` -1 is older (down the newest-first list), +1 newer (up).
pub fn step_tape(tape: &[TapeItem], idx: isize, dir: i8) -> Option<&TapeItem> {
    let next = idx + if dir == -1 { 1 } else { -1 };
    if next < 0 {
        return None;
    }
    tape.get(next as usize)
}

/// Accessible name for a tape step control. WIP is not a commit.
pub fn tape_step_label(item: Option<&TapeItem>, dir: i8) -> &'static str {
    match item {
        Some(TapeItem::Working) => "Uncommitted changes",
        _ if dir == -1 => "Previous commit",
        _ => "Next commit",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str) -> Commit {
        Commit {
            hash: hash.into(),
            sha: hash.into(),
            message: hash.into(),
            ..Default::default()
        }
    }

    #[test]
    fn is_just_commits_when_the_tree_is_clean() {
        let (a, b) = (commit("aaa"), commit("bbb"));
        assert_eq!(
            review_tape(false, &[a.clone(), b.clone()]),
            vec![TapeItem::Commit(Box::new(a)), TapeItem::Commit(Box::new(b))]
        );
    }

    #[test]
    fn puts_working_first_when_dirty() {
        let a = commit("aaa");
        assert_eq!(
            review_tape(true, std::slice::from_ref(&a)),
            vec![TapeItem::Working, TapeItem::Commit(Box::new(a))]
        );
    }

    #[test]
    fn steps_through_a_dirty_tape() {
        let (a, b) = (commit("aaa"), commit("bbb"));
        let dirty = review_tape(true, &[a.clone(), b]);
        assert_eq!(review_tape_index(&dirty, true, ""), 0);
        let idx = review_tape_index(&dirty, true, "");
        assert_eq!(
            step_tape(&dirty, idx, -1),
            Some(&TapeItem::Commit(Box::new(a.clone())))
        );
        assert_eq!(can_step_on_tape(idx, dirty.len()), (true, false));
        let idx = review_tape_index(&dirty, false, "aaa");
        assert_eq!(step_tape(&dirty, idx, 1), Some(&TapeItem::Working));
        assert_eq!(can_step_on_tape(idx, dirty.len()), (true, true));
        assert_eq!(step_tape(&dirty, 0, 1), None);
    }

    #[test]
    fn clean_tape_matches_commits_only_stepping() {
        let (a, b) = (commit("aaa"), commit("bbb"));
        let tape = review_tape(false, &[a, b.clone()]);
        assert_eq!(review_tape_index(&tape, false, "aaa"), 0);
        assert_eq!(
            step_tape(&tape, 0, -1),
            Some(&TapeItem::Commit(Box::new(b)))
        );
        assert_eq!(step_tape(&tape, 0, 1), None);
    }

    #[test]
    fn names_the_wip_item_instead_of_calling_it_a_commit() {
        let a = commit("aaa");
        assert_eq!(
            tape_step_label(Some(&TapeItem::Working), 1),
            "Uncommitted changes"
        );
        assert_eq!(
            tape_step_label(Some(&TapeItem::Working), -1),
            "Uncommitted changes"
        );
        assert_eq!(
            tape_step_label(Some(&TapeItem::Commit(Box::new(a.clone()))), -1),
            "Previous commit"
        );
        assert_eq!(
            tape_step_label(Some(&TapeItem::Commit(Box::new(a))), 1),
            "Next commit"
        );
    }
}
