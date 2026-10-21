//! PR state, grouping and attention derived from list rows and snapshots,
//! plus the comparisons the engine uses to skip no-op updates.

use recur_domain::{
    ATTENTION_ASSIGNED, ATTENTION_CHANGES_REQUESTED, ATTENTION_CHECKS_FAILING,
    ATTENTION_REVIEW_REQUESTED, Commit, Pr, PrComment, PrParticipant,
};

use super::text::eq_fold;
use crate::{CommitAuthor, CommitMeta, PrListRow, PrSnapshot};

/// PR list groups stored on `Pr.group`.
pub const GROUP_YOU: &str = "you";
pub const GROUP_LOOP: &str = "loop";
pub const GROUP_OPEN: &str = "open";
pub const GROUP_MERGED: &str = "merged";

/// Whether any of `actors` is `login` (case-insensitive).
pub fn has_login(actors: &[PrParticipant], login: &str) -> bool {
    let login = login.trim();
    !login.is_empty() && actors.iter().any(|a| eq_fold(a.login.trim(), login))
}

/// Inputs to [`attention_reasons`].
pub struct AttentionInput<'a> {
    pub state: &'a str,
    pub author: &'a str,
    pub viewer: &'a str,
    pub review_decision: &'a str,
    pub review_requests: &'a [PrParticipant],
    pub assignees: &'a [PrParticipant],
    pub checks_ok: bool,
    pub draft: bool,
}

/// Why an open, non-draft PR needs the viewer: review requested, assigned,
/// and for the author, changes requested or checks failing.
pub fn attention_reasons(input: AttentionInput) -> Vec<String> {
    if eq_fold(input.state, "MERGED") || eq_fold(input.state, "CLOSED") || input.draft {
        return Vec::new();
    }
    let viewer = input.viewer.trim();
    if viewer.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if has_login(input.review_requests, viewer) {
        out.push(ATTENTION_REVIEW_REQUESTED.to_owned());
    }
    if has_login(input.assignees, viewer) {
        out.push(ATTENTION_ASSIGNED.to_owned());
    }
    if eq_fold(input.author.trim(), viewer) {
        if eq_fold(input.review_decision, "CHANGES_REQUESTED") {
            out.push(ATTENTION_CHANGES_REQUESTED.to_owned());
        }
        if !input.checks_ok {
            out.push(ATTENTION_CHECKS_FAILING.to_owned());
        }
    }
    out
}

/// Attention reasons for a list row.
pub fn listed_attention_reasons(row: &PrListRow, viewer: &str) -> Vec<String> {
    attention_reasons(AttentionInput {
        state: &row.state,
        author: &row.author_login,
        viewer,
        review_decision: &row.review_decision,
        review_requests: &row.review_requests,
        assignees: &row.assignees,
        checks_ok: row.checks_ok,
        draft: row.is_draft,
    })
}

/// The viewer has an outstanding review request on this row.
pub fn listed_review_requested(row: &PrListRow, viewer: &str) -> bool {
    has_login(&row.review_requests, viewer)
}

/// Display state and list group for a list row.
pub fn listed_state_group(row: &PrListRow, viewer: &str) -> (&'static str, &'static str) {
    match row.state.trim().to_uppercase().as_str() {
        "MERGED" => return ("Merged", GROUP_MERGED),
        "CLOSED" => return ("Closed", GROUP_MERGED),
        _ => {}
    }
    let group = if listed_attention_reasons(row, viewer).is_empty() {
        GROUP_OPEN
    } else {
        GROUP_YOU
    };
    match row.review_decision.trim().to_uppercase().as_str() {
        "APPROVED" => ("Approved", group),
        "CHANGES_REQUESTED" => ("Changes requested", group),
        "REVIEW_REQUIRED" => ("Review required", group),
        _ if group == GROUP_YOU => ("In review", group),
        _ => ("Open", group),
    }
}

/// Keeps the last known mergeability while the forge recomputes it
/// (`UNKNOWN`), so a list sweep does not erase a known conflict.
pub fn listed_merge_field(prev: &str, listed: &str) -> String {
    let v = listed.trim().to_uppercase();
    if (v.is_empty() || v == "UNKNOWN") && !prev.is_empty() {
        return prev.to_owned();
    }
    v
}

/// 1 when the row's merge state says the head is behind its base.
pub fn listed_behind(row: &PrListRow) -> i64 {
    i64::from(eq_fold(row.merge_state_status.trim(), "BEHIND"))
}

/// Merged, closed, or in the merged group.
pub fn pr_settled(p: &Pr) -> bool {
    p.state == "Merged" || p.state == "Closed" || p.group == GROUP_MERGED
}

/// Attention reasons for a snapshot.
pub fn snapshot_attention_reasons(
    snap: &PrSnapshot,
    viewer: &str,
    checks_ok: bool,
    draft: bool,
) -> Vec<String> {
    attention_reasons(AttentionInput {
        state: &snap.state,
        author: &snap.author_login,
        viewer,
        review_decision: &snap.review_decision,
        review_requests: &snap.review_requests,
        assignees: &snap.assignees,
        checks_ok,
        draft,
    })
}

/// Overlays review-request / assignee reasons from a conversation refresh.
/// Without a viewer login it keeps the stored reasons and group so a partial
/// snapshot cannot wipe the dashboard. Returns (reasons, review requested,
/// group).
pub fn conversation_attention(
    pr: &Pr,
    snap: &PrSnapshot,
    state: &str,
    group: &str,
) -> (Vec<String>, bool, String) {
    let mut viewer = snap.viewer_login.trim();
    if viewer.is_empty() {
        viewer = pr.viewer_login.trim();
    }
    if viewer.is_empty() {
        return (
            pr.attention_reasons.clone(),
            pr.review_requested,
            group.to_owned(),
        );
    }
    let reasons = snapshot_attention_reasons(snap, viewer, pr.checks_ok, snap.is_draft || pr.draft);
    let review_requested = has_login(&snap.review_requests, viewer);
    if group == GROUP_LOOP || state == "Merged" || state == "Closed" {
        return (reasons, review_requested, group.to_owned());
    }
    let group = if reasons.is_empty() {
        GROUP_OPEN
    } else {
        GROUP_YOU
    };
    (reasons, review_requested, group.to_owned())
}

/// Display state and group after a conversation refresh. A stored merge is
/// never undone; a reopened PR leaves the merged group.
pub fn conversation_state(pr: &Pr, snap: &PrSnapshot) -> (String, String) {
    if pr.state == "Merged" {
        return (pr.state.clone(), pr.group.clone());
    }
    match snap.state.as_str() {
        "MERGED" => return ("Merged".to_owned(), GROUP_MERGED.to_owned()),
        "CLOSED" => return ("Closed".to_owned(), pr.group.clone()),
        _ => {}
    }
    let group = if pr.group == GROUP_MERGED {
        GROUP_OPEN.to_owned()
    } else {
        pr.group.clone()
    };
    let state = match snap.review_decision.as_str() {
        "APPROVED" => "Approved",
        "CHANGES_REQUESTED" => "Changes requested",
        _ if pr.state == "Approved" || pr.state == "Changes requested" => "In review",
        _ if pr.state == "Closed" => "Open",
        _ => return (pr.state.clone(), group),
    };
    (state.to_owned(), group)
}

/// Thread rows are equal for storage purposes: every field the forge sets,
/// ignoring the relative `time` label and local-only display fields.
pub fn comments_equal(a: &[PrComment], b: &[PrComment]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.id == b.id
                && a.body_html == b.body_html
                && a.author == b.author
                && a.author_login == b.author_login
                && a.kind == b.kind
                && a.verb == b.verb
                && a.body_md == b.body_md
                && a.avatar_url == b.avatar_url
                && a.verified == b.verified
                && a.check_status == b.check_status
                && a.commit_sha == b.commit_sha
                && a.url == b.url
                && a.parent_id == b.parent_id
                && a.review_id == b.review_id
                && a.path == b.path
                && a.line == b.line
                && a.start_line == b.start_line
                && a.side == b.side
                && a.start_side == b.start_side
                && a.diff_hunk == b.diff_hunk
                && a.in_reply_to_id == b.in_reply_to_id
                && a.thread_id == b.thread_id
                && a.resolved == b.resolved
                && a.outdated == b.outdated
                && a.node_id == b.node_id
                && a.reactions == b.reactions
        })
}

/// Commits are equal for storage purposes (ignores files and diff stats).
pub fn commits_equal(a: &[Commit], b: &[Commit]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.hash == b.hash
                && a.sha == b.sha
                && a.author == b.author
                && a.avatar_url == b.avatar_url
                && a.verified == b.verified
                && a.check_status == b.check_status
                && a.when == b.when
                && a.message == b.message
                && a.body == b.body
        })
}

/// Finds a per-commit entry by full sha, or by prefix either way once the
/// sha has at least 7 characters.
pub fn lookup_by_sha<'a, T>(
    map: &'a std::collections::HashMap<String, T>,
    sha: &str,
) -> Option<&'a T> {
    let sha = sha.trim();
    if sha.is_empty() || map.is_empty() {
        return None;
    }
    if let Some(v) = map.get(sha) {
        return Some(v);
    }
    if sha.len() >= 7 {
        return map
            .iter()
            .find(|(oid, _)| oid.starts_with(sha) || sha.starts_with(oid.as_str()))
            .map(|(_, v)| v);
    }
    None
}

/// Stamps forge authors, avatars, signatures and check status onto commits.
pub fn enrich_commits(commits: &[Commit], snap: &PrSnapshot) -> Vec<Commit> {
    enrich_commits_with(commits, &snap.commit_authors, &snap.commit_meta)
}

pub(crate) fn enrich_commits_with(
    commits: &[Commit],
    authors: &std::collections::HashMap<String, CommitAuthor>,
    meta: &std::collections::HashMap<String, CommitMeta>,
) -> Vec<Commit> {
    let mut out = commits.to_vec();
    for c in &mut out {
        let mut key = c.sha.trim().to_owned();
        if key.is_empty() {
            key = c.hash.trim().to_owned();
        }
        if let Some(a) = lookup_by_sha(authors, &key) {
            c.author = a.name.clone();
            c.avatar_url = a.avatar_url.clone();
        }
        if let Some(m) = lookup_by_sha(meta, &key) {
            c.verified = m.verified;
            c.check_status = m.check_status.clone();
            if c.sha.is_empty() {
                c.sha = key.trim().to_owned();
            }
        }
    }
    out
}

/// Replaces commit avatars (those that have one) with the forge's API avatar
/// for the author, when known.
pub fn apply_commit_avatars(
    commits: &mut [Commit],
    avatars: &std::collections::HashMap<String, String>,
) {
    if avatars.is_empty() {
        return;
    }
    for c in commits {
        if c.avatar_url.trim().is_empty() {
            continue;
        }
        if let Some(url) = avatars.get(&c.author.trim().to_lowercase())
            && !url.is_empty()
        {
            c.avatar_url = url.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn snap(state: &str, decision: &str) -> PrSnapshot {
        PrSnapshot {
            state: state.into(),
            review_decision: decision.into(),
            ..Default::default()
        }
    }

    fn pr(state: &str, group: &str) -> Pr {
        Pr {
            state: state.into(),
            group: group.into(),
            ..Default::default()
        }
    }

    #[test]
    fn conversation_state_cases() {
        let p = pr("In review", "you");
        assert_eq!(
            conversation_state(&p, &snap("OPEN", "APPROVED")),
            ("Approved".into(), "you".into())
        );
        assert_eq!(
            conversation_state(&p, &snap("MERGED", "")),
            ("Merged".into(), "merged".into())
        );
        let merged = pr("Merged", "merged");
        assert_eq!(
            conversation_state(&merged, &snap("OPEN", "APPROVED")),
            ("Merged".into(), "merged".into())
        );
        let closed = pr("Closed", GROUP_MERGED);
        assert_eq!(
            conversation_state(&closed, &snap("OPEN", "")),
            ("Open".into(), GROUP_OPEN.into())
        );
        assert_eq!(
            conversation_state(&closed, &snap("OPEN", "APPROVED")),
            ("Approved".into(), GROUP_OPEN.into())
        );
        assert_eq!(
            conversation_state(&pr("Approved", "open"), &snap("OPEN", "")),
            ("In review".into(), "open".into())
        );
    }

    fn row(state: &str, decision: &str) -> PrListRow {
        PrListRow {
            state: state.into(),
            review_decision: decision.into(),
            author_login: "sam".into(),
            checks_ok: true,
            ..Default::default()
        }
    }

    fn who(login: &str) -> PrParticipant {
        PrParticipant {
            login: login.into(),
            ..Default::default()
        }
    }

    #[test]
    fn listed_state_and_attention() {
        assert_eq!(
            listed_state_group(&row("MERGED", ""), "sam"),
            ("Merged", GROUP_MERGED)
        );
        assert_eq!(
            listed_state_group(&row("CLOSED", ""), "sam"),
            ("Closed", GROUP_MERGED)
        );
        assert_eq!(
            listed_state_group(&row("OPEN", "APPROVED"), "x"),
            ("Approved", GROUP_OPEN)
        );
        let mut r = row("OPEN", "");
        r.review_requests = vec![who("Avery")];
        assert_eq!(listed_state_group(&r, "avery"), ("In review", GROUP_YOU));
        assert!(listed_review_requested(&r, "AVERY"));
        assert_eq!(
            listed_attention_reasons(&r, "avery"),
            vec![ATTENTION_REVIEW_REQUESTED]
        );
        r.is_draft = true;
        assert!(listed_attention_reasons(&r, "avery").is_empty());

        let mut mine = row("OPEN", "CHANGES_REQUESTED");
        mine.checks_ok = false;
        mine.assignees = vec![who("sam")];
        assert_eq!(
            listed_attention_reasons(&mine, "sam"),
            vec![
                ATTENTION_ASSIGNED,
                ATTENTION_CHANGES_REQUESTED,
                ATTENTION_CHECKS_FAILING
            ]
        );
        assert!(listed_attention_reasons(&mine, "").is_empty());
        assert_eq!(
            listed_state_group(&mine, "sam"),
            ("Changes requested", GROUP_YOU)
        );
    }

    #[test]
    fn merge_field_keeps_known_value() {
        assert_eq!(listed_merge_field("CONFLICTING", "UNKNOWN"), "CONFLICTING");
        assert_eq!(listed_merge_field("CONFLICTING", ""), "CONFLICTING");
        assert_eq!(listed_merge_field("", "unknown"), "UNKNOWN");
        assert_eq!(listed_merge_field("CONFLICTING", "mergeable"), "MERGEABLE");
        let mut r = row("OPEN", "");
        r.merge_state_status = "behind".into();
        assert_eq!(listed_behind(&r), 1);
    }

    #[test]
    fn conversation_attention_needs_a_viewer() {
        let mut p = pr("In review", "open");
        p.attention_reasons = vec!["assigned".into()];
        let mut s = snap("OPEN", "");
        s.review_requests = vec![who("avery")];
        assert_eq!(
            conversation_attention(&p, &s, "In review", "open"),
            (vec!["assigned".to_owned()], false, "open".to_owned())
        );
        p.viewer_login = "avery".into();
        assert_eq!(
            conversation_attention(&p, &s, "In review", "open"),
            (
                vec![ATTENTION_REVIEW_REQUESTED.to_owned()],
                true,
                GROUP_YOU.to_owned()
            )
        );
        assert_eq!(
            conversation_attention(&p, &s, "In review", GROUP_LOOP).2,
            GROUP_LOOP
        );
    }

    #[test]
    fn comments_equal_detects_outdated() {
        let live = vec![PrComment {
            id: "gh-inline-1".into(),
            path: "b.go".into(),
            line: 9,
            start_line: 7,
            side: "LEFT".into(),
            diff_hunk: "@@ -7,3 +7,3 @@".into(),
            body_md: "stale".into(),
            body_html: "<code>b.go:7-9</code>".into(),
            ..Default::default()
        }];
        let mut stale = live.clone();
        stale[0].outdated = true;
        assert!(comments_equal(&live, &live));
        assert!(!comments_equal(&live, &stale));
        let mut relabeled = live.clone();
        relabeled[0].time = "5m".into();
        assert!(comments_equal(&live, &relabeled));
    }

    #[test]
    fn enrich_commits_by_prefix() {
        let snap = PrSnapshot {
            commit_authors: HashMap::from([(
                "3f1d02babcdef999".to_owned(),
                CommitAuthor {
                    name: "mycodecrafting".into(),
                    avatar_url: "https://github.com/mycodecrafting.png?size=56".into(),
                },
            )]),
            commit_meta: HashMap::from([(
                "3f1d02babcdef999".to_owned(),
                CommitMeta {
                    verified: true,
                    check_status: "pass".into(),
                    url: String::new(),
                },
            )]),
            ..Default::default()
        };
        let got = enrich_commits(
            &[
                Commit {
                    hash: "3f1d02b".into(),
                    author: "Josh".into(),
                    author_role: "Author".into(),
                    ..Default::default()
                },
                Commit {
                    hash: "deadbee".into(),
                    author: "local".into(),
                    ..Default::default()
                },
            ],
            &snap,
        );
        assert_eq!(got[0].author, "mycodecrafting");
        assert!(!got[0].avatar_url.is_empty() && got[0].verified && got[0].check_status == "pass");
        assert_eq!(got[0].sha, "3f1d02b");
        assert_eq!(got[1].author, "local");
        assert!(!got[1].verified && got[1].check_status.is_empty());
    }

    #[test]
    fn commit_avatars_replace_existing_only() {
        let mut commits = vec![
            Commit {
                author: "Avery".into(),
                avatar_url: "https://github.com/avery.png?size=56".into(),
                ..Default::default()
            },
            Commit {
                author: "avery".into(),
                ..Default::default()
            },
        ];
        apply_commit_avatars(
            &mut commits,
            &HashMap::from([(
                "avery".to_owned(),
                "https://avatars.githubusercontent.com/u/9".to_owned(),
            )]),
        );
        assert_eq!(
            commits[0].avatar_url,
            "https://avatars.githubusercontent.com/u/9"
        );
        assert_eq!(commits[1].avatar_url, "");
    }
}
