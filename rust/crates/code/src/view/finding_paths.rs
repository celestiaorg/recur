//! `lib/finding-paths.ts`: placing findings on diff files and rows.

use std::collections::HashSet;
use std::sync::LazyLock;

use recur_domain::{DiffLine, Finding};
use regex::Regex;

use super::js_parse_int;

/// The exact candidate, else a unique slash-anchored suffix match. A
/// wrong-file hit is worse than a miss, so an ambiguous suffix is `""`.
pub fn match_finding_path<'a>(path: &str, candidates: &'a [String]) -> &'a str {
    if path.is_empty() {
        return "";
    }
    if let Some(exact) = candidates.iter().find(|c| *c == path) {
        return exact;
    }
    let hits: Vec<&String> = candidates
        .iter()
        .filter(|c| c.ends_with(&format!("/{path}")) || path.ends_with(&format!("/{c}")))
        .collect();
    match hits.as_slice() {
        [one] => one,
        _ => "",
    }
}

/// The findings bucketed under `path` (in `by_path`'s order), falling back
/// to the first non-empty bucket whose key suffix-matches either way.
pub fn findings_for_path<'a>(by_path: &'a [(String, Vec<Finding>)], path: &str) -> &'a [Finding] {
    if path.is_empty() {
        return &[];
    }
    if let Some((_, fs)) = by_path.iter().find(|(p, fs)| p == path && !fs.is_empty()) {
        return fs;
    }
    by_path
        .iter()
        .find(|(p, fs)| {
            !fs.is_empty()
                && (p == path || path.ends_with(&format!("/{p}")) || p.ends_with(&format!("/{path}")))
        })
        .map_or(&[], |(_, fs)| fs.as_slice())
}

/// Right-side (new) line numbers GitHub accepts comments on for this diff.
pub fn right_diff_line_set(lines: &[DiffLine]) -> HashSet<i64> {
    lines
        .iter()
        .filter(|l| l.r#type != "del" && l.r#type != "hunk")
        .map(|l| js_parse_int(&l.new))
        .filter(|&n| n != 0)
        .collect()
}

/// Whether every line in the inclusive range is a right-side diff row.
pub fn right_diff_has_range(lines: &[DiffLine], start: i64, end: i64) -> bool {
    if start <= 0 || end < start {
        return false;
    }
    let right = right_diff_line_set(lines);
    (start..=end).all(|n| right.contains(&n))
}

/// The 1-based new-side line, or 0 for rows without one (deleted, blank).
fn new_line_no(line: &DiffLine) -> i64 {
    if line.r#type == "del" {
        return 0;
    }
    js_parse_int(&line.new)
}

/// Whether the finding's inclusive span covers `line_no` (`end_line` 0 or
/// equal to `line` is one line).
pub fn finding_covers_line(line: i64, end_line: i64, line_no: i64) -> bool {
    if line <= 0 || line_no <= 0 {
        return false;
    }
    let end = if end_line > line { end_line } else { line };
    (line..=end).contains(&line_no)
}

/// Findings that start on this row's new line (where the card sits).
pub fn findings_on_line<'a>(findings: &'a [Finding], line: &DiffLine) -> Vec<&'a Finding> {
    let n = new_line_no(line);
    if n == 0 {
        return Vec::new();
    }
    findings.iter().filter(|f| f.line == n).collect()
}

/// Findings whose span includes this row's new line (range highlight).
pub fn findings_covering_line<'a>(findings: &'a [Finding], line: &DiffLine) -> Vec<&'a Finding> {
    let n = new_line_no(line);
    if n == 0 {
        return Vec::new();
    }
    findings
        .iter()
        .filter(|f| finding_covers_line(f.line, f.end_line, n))
        .collect()
}

/// Findings whose span ends on this row's new line (note-end gutter).
pub fn findings_ending_line<'a>(findings: &'a [Finding], line: &DiffLine) -> Vec<&'a Finding> {
    let n = new_line_no(line);
    if n == 0 {
        return Vec::new();
    }
    findings
        .iter()
        .filter(|f| f.line > 0 && n == if f.end_line > f.line { f.end_line } else { f.line })
        .collect()
}

static FLAG_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"<div class="flag"[^>]*>[\s\S]*?<div class="fm">[\s\S]*?</div>[\s\x{FEFF}]*</div>[\s\x{FEFF}]*</div>"#,
    )
    .expect("flag block pattern")
});

/// `extra_html` without backend-projected `.flag` blocks, so inline finding
/// cards can replace them.
pub fn extra_html_without_finding_flags(html: &str) -> String {
    if !html.contains(r#"class="flag""#) {
        return html.to_string();
    }
    FLAG_BLOCK
        .replace_all(html, "")
        .trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, line: i64, end_line: i64) -> Finding {
        Finding {
            id: id.into(),
            file: "a.ts".into(),
            line,
            end_line,
            status: "open".into(),
            ..Default::default()
        }
    }

    fn row(new: &str, kind: &str) -> DiffLine {
        DiffLine {
            new: new.into(),
            r#type: kind.into(),
            ..Default::default()
        }
    }

    fn ids(v: Vec<&Finding>) -> Vec<&str> {
        v.into_iter().map(|f| f.id.as_str()).collect()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn match_finding_path_rules() {
        let files = s(&["src/a.ts", "lib/b.ts"]);
        assert_eq!(match_finding_path("src/a.ts", &files), "src/a.ts");
        assert_eq!(match_finding_path("a.ts", &files), "src/a.ts");
        assert_eq!(match_finding_path("/abs/repo/lib/b.ts", &files), "lib/b.ts");
        assert_eq!(match_finding_path("api.ts", &s(&["src/openapi.ts"])), "");
        assert_eq!(match_finding_path("in.go", &s(&["src/main.go"])), "");
        assert_eq!(match_finding_path("c.ts", &files), "");
        assert_eq!(match_finding_path("index.ts", &s(&["src/index.ts", "test/index.ts"])), "");
        assert_eq!(match_finding_path("", &files), "");
    }

    #[test]
    fn findings_for_path_rules() {
        let mut one = f("x", 1, 0);
        one.file = "src/a.ts".into();
        let by_path = vec![("src/a.ts".to_string(), vec![one])];
        assert_eq!(findings_for_path(&by_path, "src/a.ts").len(), 1);
        assert_eq!(findings_for_path(&by_path, "a.ts")[0].file, "src/a.ts");
        let by_api = vec![("api.ts".to_string(), vec![f("y", 1, 0)])];
        assert!(findings_for_path(&by_api, "src/openapi.ts").is_empty());
        assert!(findings_for_path(&by_path, "").is_empty());
    }

    #[test]
    fn findings_on_line_rules() {
        let notes = [f("a", 12, 0), f("b", 40, 44)];
        assert_eq!(ids(findings_on_line(&notes, &row("12", "add"))), ["a"]);
        assert_eq!(ids(findings_on_line(&notes, &row("40", "ctx"))), ["b"]);
        assert!(findings_on_line(&notes, &row("12", "del")).is_empty());
        assert!(findings_on_line(&notes, &row("", "ctx")).is_empty());
        assert!(findings_on_line(&notes, &row("0", "")).is_empty());
        assert_eq!(ids(findings_on_line(&notes, &row("12", ""))), ["a"]);
        assert!(findings_on_line(&notes, &row("42", "")).is_empty());
        assert!(findings_on_line(&notes, &row("44", "")).is_empty());
    }

    #[test]
    fn finding_covers_line_rules() {
        assert!(finding_covers_line(12, 0, 12));
        assert!(finding_covers_line(12, 12, 12));
        assert!(!finding_covers_line(12, 0, 13));
        assert!(!finding_covers_line(40, 44, 39));
        assert!(finding_covers_line(40, 44, 40));
        assert!(finding_covers_line(40, 44, 42));
        assert!(finding_covers_line(40, 44, 44));
        assert!(!finding_covers_line(40, 44, 45));
    }

    #[test]
    fn covering_and_ending_lines() {
        let notes = [f("a", 12, 0), f("b", 40, 44)];
        assert_eq!(ids(findings_covering_line(&notes, &row("12", "add"))), ["a"]);
        assert_eq!(ids(findings_covering_line(&notes, &row("40", ""))), ["b"]);
        assert_eq!(ids(findings_covering_line(&notes, &row("42", "ctx"))), ["b"]);
        assert_eq!(ids(findings_covering_line(&notes, &row("44", ""))), ["b"]);
        assert!(findings_covering_line(&notes, &row("45", "")).is_empty());
        assert_eq!(ids(findings_ending_line(&notes, &row("12", ""))), ["a"]);
        assert!(findings_ending_line(&notes, &row("40", "")).is_empty());
        assert_eq!(ids(findings_ending_line(&notes, &row("44", ""))), ["b"]);
        assert!(findings_covering_line(&notes, &row("12", "del")).is_empty());
        assert!(findings_ending_line(&notes, &row("44", "del")).is_empty());
    }

    #[test]
    fn right_diff_has_range_rules() {
        let lines = [
            row("10", "ctx"),
            row("11", "add"),
            DiffLine {
                old: "12".into(),
                r#type: "del".into(),
                ..Default::default()
            },
            row("12", "add"),
        ];
        assert!(right_diff_has_range(&lines, 10, 12));
        assert!(right_diff_has_range(&lines, 11, 11));
        assert!(!right_diff_has_range(&lines, 10, 13));
        assert!(!right_diff_has_range(&lines[2..3], 12, 12));
        assert!(!right_diff_has_range(&[], 1, 1));
    }

    #[test]
    fn strips_projected_flags() {
        let flag = r#"<div class="flag"><svg></svg><div><div class="ft">t</div><div class="fm">msg</div></div></div>"#;
        let other = r#"<div class="icmt">note</div>"#;
        assert_eq!(extra_html_without_finding_flags(&format!("{flag}{other}")), other);
        assert_eq!(extra_html_without_finding_flags(other), other);
        assert_eq!(extra_html_without_finding_flags(""), "");
        let projected = crate::observe::finding_flag_html(&[f("z", 1, 0)]);
        assert_eq!(extra_html_without_finding_flags(&format!(" {projected} ")), "");
    }
}
