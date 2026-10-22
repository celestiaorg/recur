//! `lib/commit-meta.ts`.

use recur_domain::Commit;

/// The full SHA when the commit has one, else its short hash.
pub fn commit_sha(c: &Commit) -> String {
    let s = if c.sha.is_empty() { &c.hash } else { &c.sha };
    s.trim().to_string()
}

/// `Coder · iteration 2` for agent commits, the author for human ones.
pub fn commit_byline(c: &Commit) -> String {
    if !c.author_role.is_empty() && c.author_role != "Author" {
        return if c.iter_label.is_empty() {
            c.author_role.clone()
        } else {
            format!("{} · {}", c.author_role, c.iter_label)
        };
    }
    [&c.author, &c.author_role]
        .into_iter()
        .find(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "Author".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_prefers_the_full_sha() {
        let c = Commit {
            hash: "abc1234".into(),
            sha: " abc1234ffff ".into(),
            ..Default::default()
        };
        assert_eq!(commit_sha(&c), "abc1234ffff");
        let c = Commit {
            hash: "abc1234".into(),
            ..Default::default()
        };
        assert_eq!(commit_sha(&c), "abc1234");
    }

    #[test]
    fn byline_names_the_role_or_the_author() {
        let agent = Commit {
            author_role: "Coder".into(),
            iter_label: "iteration 2".into(),
            ..Default::default()
        };
        assert_eq!(commit_byline(&agent), "Coder · iteration 2");
        let human = Commit {
            author: "Ada".into(),
            author_role: "Author".into(),
            ..Default::default()
        };
        assert_eq!(commit_byline(&human), "Ada");
        assert_eq!(commit_byline(&Commit::default()), "Author");
    }
}
