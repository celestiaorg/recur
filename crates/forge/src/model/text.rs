//! Small text helpers shared by forges and the engine's PR code.

use recur_domain::{Commit, Pr};

/// The first non-empty line of `s`, trimmed.
pub fn single_line(s: &str) -> String {
    let s = s.trim();
    match s.find('\n') {
        Some(i) => s[..i].trim().to_owned(),
        None => s.to_owned(),
    }
}

/// The first `n` characters of `s`.
pub fn hard_truncate_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => s[..i].to_owned(),
        None => s.to_owned(),
    }
}

/// The first line of each commit message, skipping blanks.
pub fn commit_subjects(commits: &[Commit]) -> Vec<String> {
    commits
        .iter()
        .map(|c| single_line(&c.message))
        .filter(|s| !s.is_empty())
        .collect()
}

/// `* subject` lines oldest-first. Commit slices are newest-first (git log).
pub fn format_commit_bullet_list(commits: &[Commit]) -> String {
    let mut subjects = commit_subjects(commits);
    subjects.reverse();
    subjects
        .iter()
        .map(|s| format!("* {s}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The PR description (the `description` comment's markdown), trimmed.
pub fn pr_description(pr: &Pr) -> String {
    pr.comments
        .iter()
        .flatten()
        .find(|c| c.kind == "description")
        .map(|c| c.body_md.trim().to_owned())
        .unwrap_or_default()
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '"' => out.push_str("&#34;"),
            c => out.push(c),
        }
    }
    out
}

/// The fallback avatar fill for a comment or commit with no photo.
pub fn comment_avatar_css(code: &str) -> &'static str {
    match code {
        "CO" => "background:var(--cyan);color:#0A0C10",
        "PR" => "background:#b08bd8;color:#0A0C10",
        "MA" => "background:var(--amber);color:#0A0C10",
        "RV" => "background:var(--indigo);color:#0A0C10",
        _ => "background:linear-gradient(135deg,#39434f,#1d242c);color:var(--muted)",
    }
}

/// `a` when it has non-space text, else `b`.
pub(crate) fn first_non_empty<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.trim().is_empty() { b } else { a }
}

/// ASCII case-insensitive equality for the values forges send.
pub(crate) fn eq_fold(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_and_truncate() {
        assert_eq!(single_line("  \n"), "");
        assert_eq!(single_line(" first \nsecond"), "first");
        assert_eq!(hard_truncate_chars("héllo", 2), "hé");
        assert_eq!(hard_truncate_chars("hi", 5), "hi");
        assert_eq!(hard_truncate_chars("hi", 0), "");
    }

    #[test]
    fn bullet_list_is_oldest_first() {
        let commits = [
            Commit {
                message: "newest\nbody".into(),
                ..Default::default()
            },
            Commit {
                message: " ".into(),
                ..Default::default()
            },
            Commit {
                message: "oldest".into(),
                ..Default::default()
            },
        ];
        assert_eq!(format_commit_bullet_list(&commits), "* oldest\n* newest");
        assert_eq!(format_commit_bullet_list(&[]), "");
    }

    #[test]
    fn escapes_like_go() {
        assert_eq!(
            html_escape(r#"<a href="x">&'"#),
            "&lt;a href=&#34;x&#34;&gt;&amp;&#39;"
        );
    }
}
