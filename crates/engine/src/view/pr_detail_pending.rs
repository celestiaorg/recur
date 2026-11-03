//! When the PR detail tabs show a skeleton instead of an empty state
//! Forge snapshots always include a
//! description comment; Local PRs render immediately.

use recur_domain::Pr;

use crate::prs::is_forge_source;

pub fn has_pr_description(pr: Option<&Pr>) -> bool {
    pr.and_then(|p| p.comments.as_deref())
        .is_some_and(|cs| cs.iter().any(|c| c.kind == "description"))
}

pub fn pr_conv_pending(pr: Option<&Pr>, give_up: bool) -> bool {
    match pr {
        None => !give_up,
        Some(p) if !is_forge_source(&p.source) || give_up => false,
        Some(p) => !has_pr_description(Some(p)),
    }
}

pub fn pr_files_pending(pr: Option<&Pr>, refreshing: bool, give_up: bool) -> bool {
    let Some(p) = pr else {
        return !give_up;
    };
    if !is_forge_source(&p.source) {
        return refreshing && p.files.is_none();
    }
    if p.files.as_ref().is_some_and(|f| !f.is_empty()) || give_up {
        return false;
    }
    p.file_count > 0 || refreshing
}

pub fn pr_commits_pending(pr: Option<&Pr>, refreshing: bool) -> bool {
    let Some(p) = pr else {
        return true;
    };
    if !is_forge_source(&p.source) {
        return refreshing && p.commits.is_none();
    }
    refreshing && p.commits.as_ref().is_none_or(Vec::is_empty)
}

pub fn pr_checks_pending(pr: Option<&Pr>, refreshing: bool) -> bool {
    let Some(p) = pr else {
        return true;
    };
    if !is_forge_source(&p.source) || p.checks.as_ref().is_some_and(|c| !c.is_empty()) {
        return false;
    }
    refreshing || p.has_checks
}

#[cfg(test)]
mod tests {
    use recur_domain::{Check, Commit, FileDiff, PrComment, PrSource};

    use super::*;

    fn gh(f: impl FnOnce(&mut Pr)) -> Pr {
        let mut p = Pr {
            source: PrSource::GitHub,
            ..Pr::default()
        };
        f(&mut p);
        p
    }

    fn local(f: impl FnOnce(&mut Pr)) -> Pr {
        let mut p = Pr {
            source: PrSource::Local,
            ..Pr::default()
        };
        f(&mut p);
        p
    }

    fn kind(k: &str) -> PrComment {
        PrComment {
            kind: k.into(),
            ..PrComment::default()
        }
    }

    #[test]
    fn conversation() {
        assert!(pr_conv_pending(None, false));
        assert!(!pr_conv_pending(
            Some(&local(|p| p.comments = Some(vec![]))),
            false
        ));
        assert!(pr_conv_pending(
            Some(&gh(|p| p.comments = Some(vec![]))),
            false
        ));
        assert!(pr_conv_pending(
            Some(&gh(|p| p.comments = Some(vec![kind("comment")]))),
            false
        ));
        assert!(!pr_conv_pending(
            Some(&gh(|p| p.comments = Some(vec![kind("description")]))),
            false
        ));
        assert!(!pr_conv_pending(Some(&gh(|_| {})), true));
        assert!(!pr_conv_pending(None, true));
    }

    #[test]
    fn files() {
        let one = || Some(vec![FileDiff::default()]);
        assert!(pr_files_pending(
            Some(&gh(|p| {
                p.files = Some(vec![]);
                p.file_count = 4;
            })),
            false,
            false
        ));
        assert!(!pr_files_pending(
            Some(&gh(|p| {
                p.files = one();
                p.file_count = 4;
            })),
            false,
            false
        ));
        assert!(!pr_files_pending(
            Some(&gh(|p| p.files = Some(vec![]))),
            false,
            false
        ));
        assert!(pr_files_pending(
            Some(&gh(|p| p.files = Some(vec![]))),
            true,
            false
        ));
    }

    #[test]
    fn local_rows_without_diff_fields() {
        assert!(pr_files_pending(Some(&local(|_| {})), true, false));
        assert!(pr_commits_pending(Some(&local(|_| {})), true));
        assert!(!pr_files_pending(Some(&local(|_| {})), false, false));
        assert!(!pr_commits_pending(Some(&local(|_| {})), false));
        assert!(!pr_files_pending(
            Some(&local(|p| p.files = Some(vec![]))),
            true,
            false
        ));
        assert!(!pr_commits_pending(
            Some(&local(|p| p.commits = Some(vec![]))),
            true
        ));
    }

    #[test]
    fn commits_and_checks() {
        assert!(pr_commits_pending(
            Some(&gh(|p| p.commits = Some(vec![]))),
            true
        ));
        assert!(!pr_commits_pending(
            Some(&gh(|p| p.commits = Some(vec![]))),
            false
        ));
        assert!(!pr_commits_pending(
            Some(&gh(|p| p.commits = Some(vec![Commit::default()]))),
            true
        ));
        assert!(pr_checks_pending(
            Some(&gh(|p| p.checks = Some(vec![]))),
            true
        ));
        assert!(pr_checks_pending(
            Some(&gh(|p| {
                p.has_checks = true;
                p.checks = Some(vec![]);
            })),
            false
        ));
        assert!(!pr_checks_pending(
            Some(&gh(|p| p.checks = Some(vec![]))),
            false
        ));
        assert!(!pr_checks_pending(
            Some(&gh(|p| p.checks = Some(vec![Check::default()]))),
            true
        ));
    }

    #[test]
    fn description() {
        assert!(has_pr_description(Some(&gh(
            |p| p.comments = Some(vec![kind("description")])
        ))));
        assert!(!has_pr_description(Some(
            &gh(|p| p.comments = Some(vec![]))
        )));
    }
}
