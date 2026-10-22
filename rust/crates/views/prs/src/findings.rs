//! The PR-detail half of `lib/findings-ui.ts`: where a finding lands as a GitHub review
//! comment, and whether a queued or submitted comment already carries it.

use std::collections::HashSet;

use recur_code::view::finding_paths::{match_finding_path, right_diff_has_range};
use recur_domain::{FileDiff, Finding, PendingLineComment, PrComment};

/// A GitHub pending-comment anchor: the comment line (the span's end) and its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewAnchor {
    pub file: String,
    pub line: i64,
    pub start_line: i64,
}

/// A location to compare with an anchor.
struct Loc<'a> {
    file: &'a str,
    line: i64,
    start_line: i64,
    side: &'a str,
}

fn review_start_line(start: i64, line: i64) -> i64 {
    if start > 0 && start != line { start } else { 0 }
}

fn loc_matches_anchor(loc: &Loc, a: &ReviewAnchor) -> bool {
    let side = if loc.side.is_empty() {
        "RIGHT"
    } else {
        loc.side
    };
    loc.file == a.file
        && loc.line == a.line
        && review_start_line(loc.start_line, loc.line) == a.start_line
        && !side.eq_ignore_ascii_case("LEFT")
}

/// `findingPublicId`: the short id agents use, else the UUID.
pub fn finding_public_id(f: &Finding) -> &str {
    if f.short_id.is_empty() {
        &f.id
    } else {
        &f.short_id
    }
}

/// `findingReviewAnchor`: `None` unless the path resolves to a unique PR file and the
/// RIGHT-side range is in that file's diff.
pub fn finding_review_anchor(f: &Finding, files: &[FileDiff]) -> Option<ReviewAnchor> {
    let raw = f.file.trim();
    let start = f.line;
    if raw.is_empty() || start <= 0 {
        return None;
    }
    let paths: Vec<String> = files.iter().map(|x| x.path.clone()).collect();
    let file = match_finding_path(raw, &paths);
    if file.is_empty() {
        return None;
    }
    let end = if f.end_line > start { f.end_line } else { start };
    let lines = files
        .iter()
        .find(|x| x.path == file)
        .and_then(|x| x.lines.as_deref())
        .unwrap_or_default();
    if !right_diff_has_range(lines, start, end) {
        return None;
    }
    Some(ReviewAnchor {
        file: file.to_string(),
        line: end,
        start_line: if end > start { start } else { 0 },
    })
}

/// `findingCommentBody`: AddPRComment trims the body; GitHub echoes the trimmed text.
pub fn finding_comment_body(message: &str) -> &str {
    message.trim()
}

fn same_comment_body(a: &str, b: &str) -> bool {
    finding_comment_body(a) == finding_comment_body(b)
}

/// True when a pending review comment already queues this finding's GitHub anchor.
pub fn finding_has_pending_comment(
    f: &Finding,
    pending: &[PendingLineComment],
    files: &[FileDiff],
) -> bool {
    let Some(a) = finding_review_anchor(f, files) else {
        return false;
    };
    pending.iter().any(|pc| {
        loc_matches_anchor(
            &Loc {
                file: &pc.file,
                line: pc.line,
                start_line: pc.start_line,
                side: &pc.side,
            },
            &a,
        ) && same_comment_body(&pc.body, &f.message)
    })
}

/// True when a submitted line comment already occupies this finding's GitHub anchor.
pub fn finding_has_submitted_comment(f: &Finding, comments: &[PrComment], files: &[FileDiff]) -> bool {
    let Some(a) = finding_review_anchor(f, files) else {
        return false;
    };
    comments.iter().any(|c| {
        loc_matches_anchor(
            &Loc {
                file: &c.path,
                line: c.line,
                start_line: c.start_line,
                side: &c.side,
            },
            &a,
        ) && same_comment_body(&c.body_md, &f.message)
    })
}

/// True when this session queued the finding and a submitted comment now occupies that anchor.
pub fn finding_ready_to_settle(
    f: &Finding,
    pending: &[PendingLineComment],
    comments: &[PrComment],
    files: &[FileDiff],
    posted: &HashSet<String>,
) -> bool {
    if f.id.is_empty() || !posted.contains(&f.id) {
        return false;
    }
    !finding_has_pending_comment(f, pending, files)
        && finding_has_submitted_comment(f, comments, files)
}

#[cfg(test)]
mod tests {
    use recur_domain::DiffLine;

    use super::*;

    fn add_lines(right: &[i64]) -> Vec<DiffLine> {
        right
            .iter()
            .map(|n| DiffLine {
                new: n.to_string(),
                r#type: "add".into(),
                ..Default::default()
            })
            .collect()
    }

    fn files(path: &str, right: &[i64]) -> Vec<FileDiff> {
        vec![FileDiff {
            path: path.into(),
            lines: Some(add_lines(right)),
            ..Default::default()
        }]
    }

    fn finding(file: &str, line: i64, end: i64, msg: &str) -> Finding {
        Finding {
            file: file.into(),
            line,
            end_line: end,
            message: msg.into(),
            ..Default::default()
        }
    }

    fn anchor(file: &str, line: i64, start_line: i64) -> Option<ReviewAnchor> {
        Some(ReviewAnchor {
            file: file.into(),
            line,
            start_line,
        })
    }

    #[test]
    fn maps_a_span_onto_github_line_and_start_line() {
        let pr = files("a.go", &[10, 11, 12, 13, 14]);
        assert_eq!(
            finding_review_anchor(&finding("a.go", 10, 14, ""), &pr),
            anchor("a.go", 14, 10)
        );
        assert_eq!(
            finding_review_anchor(&finding("a.go", 10, 0, ""), &files("a.go", &[10])),
            anchor("a.go", 10, 0)
        );
    }

    #[test]
    fn resolves_a_suffix_path_and_rejects_out_of_diff_lines() {
        let pr = files("src/hooks/useOAuthSession.ts", &[2, 8]);
        assert_eq!(
            finding_review_anchor(&finding("useOAuthSession.ts", 2, 0, ""), &pr),
            anchor("src/hooks/useOAuthSession.ts", 2, 0)
        );
        assert_eq!(
            finding_review_anchor(&finding("src/hooks/useOAuthSession.ts", 99, 0, ""), &pr),
            None
        );
        assert_eq!(finding_review_anchor(&finding("missing.ts", 2, 0, ""), &pr), None);
    }

    #[test]
    fn rejects_an_ambiguous_suffix() {
        let pr = [
            FileDiff {
                path: "src/index.ts".into(),
                lines: Some(add_lines(&[1])),
                ..Default::default()
            },
            FileDiff {
                path: "test/index.ts".into(),
                lines: Some(add_lines(&[1])),
                ..Default::default()
            },
        ];
        assert_eq!(finding_review_anchor(&finding("index.ts", 1, 0, ""), &pr), None);
        assert_eq!(
            finding_review_anchor(&finding("src/index.ts", 1, 0, ""), &pr),
            anchor("src/index.ts", 1, 0)
        );
    }

    #[test]
    fn rejects_findings_that_cannot_be_a_line_comment() {
        let pr = files("a.go", &[10]);
        assert_eq!(finding_review_anchor(&finding("", 10, 0, ""), &pr), None);
        assert_eq!(finding_review_anchor(&finding("a.go", 0, 0, ""), &pr), None);
        assert_eq!(finding_review_anchor(&finding("  ", 3, 0, ""), &pr), None);
    }

    fn pending(file: &str, line: i64, start: i64, side: &str, body: &str) -> PendingLineComment {
        PendingLineComment {
            file: file.into(),
            line,
            start_line: start,
            side: side.into(),
            body: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn pending_matches_the_resolved_anchor_not_the_raw_path() {
        let pr = files("src/a.ts", &[2]);
        let f = finding("a.ts", 2, 0, "race");
        assert!(finding_has_pending_comment(
            &f,
            &[pending("src/a.ts", 2, 0, "", "race")],
            &pr
        ));
        assert!(!finding_has_pending_comment(
            &f,
            &[pending("a.ts", 2, 0, "", "race")],
            &pr
        ));
        assert!(!finding_has_pending_comment(
            &f,
            &[pending("src/a.ts", 2, 0, "", "other")],
            &pr
        ));
    }

    #[test]
    fn pending_requires_the_right_side_span() {
        let pr = files("src/a.ts", &[10, 11, 12]);
        let f = finding("src/a.ts", 10, 12, "race");
        assert!(finding_has_pending_comment(
            &f,
            &[pending("src/a.ts", 12, 10, "RIGHT", "race")],
            &pr
        ));
        assert!(!finding_has_pending_comment(
            &f,
            &[pending("src/a.ts", 12, 0, "", "race")],
            &pr
        ));
        assert!(!finding_has_pending_comment(
            &f,
            &[pending("src/a.ts", 12, 10, "LEFT", "race")],
            &pr
        ));
    }

    #[test]
    fn pending_trims_bodies() {
        let pr = files("src/a.ts", &[2]);
        let f = finding("src/a.ts", 2, 0, "  race\n");
        assert!(finding_has_pending_comment(
            &f,
            &[pending("src/a.ts", 2, 0, "", "race")],
            &pr
        ));
    }

    fn submitted(path: &str, line: i64, side: &str, body: &str) -> PrComment {
        PrComment {
            path: path.into(),
            line,
            side: side.into(),
            body_md: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn submitted_matches_a_right_side_comment_at_the_anchor() {
        let pr = files("src/a.ts", &[2]);
        let f = finding("a.ts", 2, 0, "race");
        assert!(finding_has_submitted_comment(
            &f,
            &[submitted("src/a.ts", 2, "RIGHT", "race")],
            &pr
        ));
        assert!(!finding_has_submitted_comment(
            &f,
            &[submitted("src/a.ts", 2, "LEFT", "race")],
            &pr
        ));
        assert!(!finding_has_submitted_comment(
            &f,
            &[submitted("src/a.ts", 2, "", "other")],
            &pr
        ));
        let f = finding("src/a.ts", 2, 0, "\trace  ");
        assert!(finding_has_submitted_comment(
            &f,
            &[submitted("src/a.ts", 2, "RIGHT", "race")],
            &pr
        ));
    }

    #[test]
    fn ready_to_settle_requires_this_session_queued_it() {
        let pr = files("src/a.ts", &[2]);
        let f = Finding {
            id: "F-1".into(),
            ..finding("src/a.ts", 2, 0, "race")
        };
        let done = [submitted("src/a.ts", 2, "RIGHT", "race")];
        let set = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<HashSet<_>>();
        assert!(!finding_ready_to_settle(&f, &[], &done, &pr, &set(&[])));
        assert!(!finding_ready_to_settle(&f, &[], &done, &pr, &set(&["F-other"])));
        assert!(finding_ready_to_settle(&f, &[], &done, &pr, &set(&["F-1"])));
        // Waits until the pending draft is gone.
        assert!(!finding_ready_to_settle(
            &f,
            &[pending("src/a.ts", 2, 0, "", "race")],
            &done,
            &pr,
            &set(&["F-1"])
        ));
    }

    #[test]
    fn public_id_prefers_the_short_id() {
        let mut f = Finding {
            id: "uuid".into(),
            ..Default::default()
        };
        assert_eq!(finding_public_id(&f), "uuid");
        f.short_id = "find_1".into();
        assert_eq!(finding_public_id(&f), "find_1");
    }
}
