//! `lib/pr-commits.ts`.

use recur_domain::Commit;

/// Keep already-loaded per-commit diffs when a GetPR snapshot still has `files` empty.
pub fn keep_commit_files(
    prev: Option<&Vec<Commit>>,
    next: Option<Vec<Commit>>,
) -> Option<Vec<Commit>> {
    let Some(prev) = prev.filter(|p| !p.is_empty()) else {
        return next;
    };
    next.map(|list| {
        list.into_iter()
            .map(|mut c| {
                if c.files.as_ref().is_some_and(|f| !f.is_empty()) {
                    return c;
                }
                if let Some(old) = prev.iter().find(|p| commit_keys_match(p, &c))
                    && old.files.as_ref().is_some_and(|f| !f.is_empty())
                {
                    c.files = old.files.clone();
                }
                c
            })
            .collect()
    })
}

/// Short and full hashes refer to the same commit when one is a prefix of the other, across
/// `hash` and `sha`.
pub fn commit_keys_match(a: &Commit, b: &Commit) -> bool {
    let (ah, bh) = (a.hash.to_lowercase(), b.hash.to_lowercase());
    let (as_, bs) = (a.sha.to_lowercase(), b.sha.to_lowercase());
    let prefix = |x: &str, y: &str| x.starts_with(y) || y.starts_with(x);
    (!ah.is_empty() && !bh.is_empty() && prefix(&ah, &bh))
        || (!as_.is_empty() && !bs.is_empty() && prefix(&as_, &bs))
        || (!ah.is_empty() && !bs.is_empty() && prefix(&bs, &ah))
        || (!as_.is_empty() && !bh.is_empty() && prefix(&as_, &bh))
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_domain::{DiffStat, FileDiff};

    fn file(path: &str) -> FileDiff {
        FileDiff {
            path: path.into(),
            status: "m".into(),
            diff: DiffStat { add: 1, del: 0 },
            lines: Some(Vec::new()),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_previously_loaded_diffs_when_the_snapshot_is_still_empty() {
        let prev = vec![Commit {
            hash: "abc1234".into(),
            sha: "abc1234ffff".into(),
            files: Some(vec![file("a.ts")]),
            ..Default::default()
        }];
        let next = vec![Commit {
            hash: "abc1234".into(),
            sha: "abc1234ffff".into(),
            message: "fresh subject".into(),
            files: Some(Vec::new()),
            ..Default::default()
        }];
        let got = keep_commit_files(Some(&prev), Some(next)).expect("list");
        assert_eq!(got[0].message, "fresh subject");
        let files = got[0].files.as_ref().expect("files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "a.ts");
    }

    #[test]
    fn does_not_overwrite_a_snapshot_that_already_has_files() {
        let prev = vec![Commit {
            hash: "abc1234".into(),
            files: Some(vec![file("old.ts")]),
            ..Default::default()
        }];
        let next = vec![Commit {
            hash: "abc1234".into(),
            files: Some(vec![file("new.ts")]),
            ..Default::default()
        }];
        let got = keep_commit_files(Some(&prev), Some(next)).expect("list");
        assert_eq!(got[0].files.as_ref().expect("files")[0].path, "new.ts");
    }

    #[test]
    fn matches_short_hash_against_full_sha() {
        let a = Commit {
            hash: "abc1234".into(),
            sha: "abc1234ffff".into(),
            ..Default::default()
        };
        let b = Commit {
            hash: "abc1234".into(),
            ..Default::default()
        };
        assert!(commit_keys_match(&a, &b));
        let short = Commit {
            hash: "abc1234".into(),
            ..Default::default()
        };
        let full = Commit {
            sha: "abc1234ffff".into(),
            ..Default::default()
        };
        assert!(commit_keys_match(&short, &full));
        assert!(commit_keys_match(&full, &short));
    }
}
