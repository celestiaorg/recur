//! `lib/comment-hunk.ts`: the code snippet above a review comment, from GitHub's `diff_hunk`.

use recur_engine::view::js;

use crate::html::js_int;

/// Conversation snippets keep a few lines above the note, like GitHub.
pub const COMMENT_HUNK_CONTEXT: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HunkLine {
    /// `add`, `del` or `ctx`.
    pub r#type: &'static str,
    pub old: String,
    pub new: String,
    pub text: String,
    /// The line the review comment is anchored to.
    pub target: bool,
    /// Inside a GitHub multiline comment's range.
    pub in_range: bool,
}

fn header(line: &str) -> Option<(i64, i64)> {
    let rest = line.strip_prefix("@@")?;
    let rest = rest.trim_start_matches(|c: char| c.is_whitespace());
    if rest.len() == line.len() - 2 {
        return None;
    }
    let rest = rest.strip_prefix('-')?;
    let digits = |s: &str| -> Option<(i64, usize)> {
        let n = s.bytes().take_while(u8::is_ascii_digit).count();
        (n > 0).then(|| (s[..n].parse().unwrap_or(0), n))
    };
    let (old, n) = digits(rest)?;
    let mut rest = &rest[n..];
    if let Some(r) = rest.strip_prefix(',') {
        let (_, m) = digits(r)?;
        rest = &r[m..];
    }
    let trimmed = rest.trim_start_matches(|c: char| c.is_whitespace());
    if trimmed.len() == rest.len() {
        return None;
    }
    let rest = trimmed.strip_prefix('+')?;
    let (new, _) = digits(rest)?;
    Some((old, new))
}

/// `parseDiffHunk`: numbered add / del / ctx rows (the `@@` header is skipped).
pub fn parse_diff_hunk(hunk: &str) -> Vec<HunkLine> {
    let raw = hunk.replace("\r\n", "\n");
    let raw = js::trim(&raw);
    if raw.is_empty() {
        return Vec::new();
    }
    let (mut old_n, mut new_n) = (0i64, 0i64);
    let mut out = Vec::new();
    for line in raw.split('\n') {
        if let Some((o, n)) = header(line) {
            old_n = o;
            new_n = n;
            continue;
        }
        let mut chars = line.chars();
        let ch = chars.next();
        let rest = chars.as_str().to_string();
        match ch {
            Some('+') => {
                out.push(HunkLine {
                    r#type: "add",
                    new: new_n.to_string(),
                    text: rest,
                    ..Default::default()
                });
                new_n += 1;
            }
            Some('-') => {
                out.push(HunkLine {
                    r#type: "del",
                    old: old_n.to_string(),
                    text: rest,
                    ..Default::default()
                });
                old_n += 1;
            }
            Some('\\') => {}
            _ => {
                let text = if ch == Some(' ') {
                    rest
                } else {
                    line.to_string()
                };
                out.push(HunkLine {
                    r#type: "ctx",
                    old: old_n.to_string(),
                    new: new_n.to_string(),
                    text,
                    ..Default::default()
                });
                old_n += 1;
                new_n += 1;
            }
        }
    }
    out
}

/// A context header's range: `12` or `10-15`.
pub fn comment_hunk_line_label(line: i64, start_line: i64) -> String {
    if line <= 0 {
        return String::new();
    }
    if start_line > 0 && start_line < line {
        format!("{start_line}-{line}")
    } else {
        line.to_string()
    }
}

/// `path:line` (or `path:start-end`), with ` (outdated)`.
pub fn comment_hunk_path_label(path: &str, line: i64, start_line: i64, outdated: bool) -> String {
    let p = js::trim(path);
    if p.is_empty() {
        return String::new();
    }
    let loc = comment_hunk_line_label(line, start_line);
    let base = if loc.is_empty() {
        p.to_string()
    } else {
        format!("{p}:{loc}")
    };
    if outdated {
        format!("{base} (outdated)")
    } else {
        base
    }
}

/// The commented row; GitHub's hunk usually ends there. -1 for an empty hunk.
pub fn comment_hunk_target_index(lines: &[HunkLine], line: i64, side: &str) -> isize {
    if lines.is_empty() {
        return -1;
    }
    let side = if side.is_empty() { "RIGHT" } else { side };
    let left = side.eq_ignore_ascii_case("LEFT");
    if line > 0 {
        for (i, hl) in lines.iter().enumerate().rev() {
            let hit = if left {
                hl.r#type != "add" && js_int(&hl.old) == line
            } else {
                hl.r#type != "del" && js_int(&hl.new) == line
            };
            if hit {
                return i as isize;
            }
        }
    }
    lines.len() as isize - 1
}

#[derive(Clone, Debug, Default)]
pub struct ClipOpts<'a> {
    pub line: i64,
    pub side: &'a str,
    pub start_line: i64,
    pub start_side: &'a str,
    pub context: Option<usize>,
}

/// Keep `context` rows above the note (or range); drop the rest of the hunk.
pub fn clip_comment_hunk(lines: &[HunkLine], opts: &ClipOpts) -> Vec<HunkLine> {
    if lines.is_empty() {
        return Vec::new();
    }
    let end = comment_hunk_target_index(lines, opts.line, opts.side);
    if end < 0 {
        return Vec::new();
    }
    let end = end as usize;
    let mut from = end;
    if opts.start_line > 0 && opts.start_line != opts.line {
        let side = if opts.start_side.is_empty() {
            opts.side
        } else {
            opts.start_side
        };
        let idx = comment_hunk_target_index(lines, opts.start_line, side);
        if idx >= 0 {
            from = (idx as usize).min(end);
        }
    }
    let ctx = opts.context.unwrap_or(COMMENT_HUNK_CONTEXT);
    let start = from.saturating_sub(ctx);
    lines[start..=end]
        .iter()
        .enumerate()
        .map(|(i, hl)| {
            let idx = start + i;
            HunkLine {
                target: idx == end || hl.target,
                in_range: idx >= from || hl.in_range,
                ..hl.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(t: &'static str, old: &str, new: &str, text: &str) -> HunkLine {
        HunkLine {
            r#type: t,
            old: old.into(),
            new: new.into(),
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn is_empty_for_a_blank_hunk() {
        assert!(parse_diff_hunk("").is_empty());
        assert!(parse_diff_hunk("   ").is_empty());
    }

    #[test]
    fn numbers_add_and_del_rows_from_the_header() {
        let hunk = [
            "@@ -16,2 +16,2 @@ Enable deposit",
            "-VITE_ENABLE_FORWARDING_DEPOSIT=false",
            "+VITE_ENABLE_FORWARDING_DEPOSIT=true",
        ]
        .join("\n");
        assert_eq!(
            parse_diff_hunk(&hunk),
            vec![
                row("del", "16", "", "VITE_ENABLE_FORWARDING_DEPOSIT=false"),
                row("add", "", "16", "VITE_ENABLE_FORWARDING_DEPOSIT=true"),
            ]
        );
    }

    #[test]
    fn keeps_context_lines_on_both_sides() {
        let hunk = ["@@ -1,3 +1,3 @@", " keep", "-old", "+new"].join("\n");
        assert_eq!(
            parse_diff_hunk(&hunk),
            vec![
                row("ctx", "1", "1", "keep"),
                row("del", "2", "", "old"),
                row("add", "", "2", "new"),
            ]
        );
    }

    fn long() -> String {
        [
            "@@ -4,9 +4,9 @@ provider registry",
            " import { secrets } from './vault'",
            " const providers = []",
            " function register() {",
            "   return providers",
            " }",
            " const unused = 1",
            " const unused2 = 2",
            " const unused3 = 3",
            "-const secret = process.env.OAUTH_CLIENT_SECRET",
            "+const secret = await secrets.get('oauth/client_secret')",
        ]
        .join("\n")
    }

    fn texts(v: &[HunkLine]) -> Vec<&str> {
        v.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn keeps_three_context_rows_plus_the_commented_line() {
        let clipped = clip_comment_hunk(
            &parse_diff_hunk(&long()),
            &ClipOpts {
                line: 12,
                side: "RIGHT",
                ..Default::default()
            },
        );
        assert_eq!(
            texts(&clipped),
            [
                "const unused2 = 2",
                "const unused3 = 3",
                "const secret = process.env.OAUTH_CLIENT_SECRET",
                "const secret = await secrets.get('oauth/client_secret')",
            ]
        );
        let last = clipped.last().expect("last");
        assert!(last.target);
        assert_eq!(last.new, "12");
        assert!(!clipped.iter().any(|l| l.text.contains("function register")));
    }

    #[test]
    fn anchors_a_left_comment_on_the_deleted_line() {
        let clipped = clip_comment_hunk(
            &parse_diff_hunk(&long()),
            &ClipOpts {
                line: 12,
                side: "LEFT",
                context: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(
            texts(&clipped),
            [
                "const unused3 = 3",
                "const secret = process.env.OAUTH_CLIENT_SECRET"
            ]
        );
        let last = clipped.last().expect("last");
        assert_eq!(last.r#type, "del");
        assert!(last.target);
    }

    #[test]
    fn falls_back_to_the_last_hunk_row_when_the_line_is_missing() {
        let clipped = clip_comment_hunk(
            &parse_diff_hunk(&long()),
            &ClipOpts {
                line: 99,
                side: "RIGHT",
                context: Some(0),
                ..Default::default()
            },
        );
        assert_eq!(clipped.len(), 1);
        assert!(clipped[0].target);
        assert!(clipped[0].text.contains("secrets.get"));
    }

    #[test]
    fn keeps_a_multiline_range_plus_context_above_the_start() {
        let clipped = clip_comment_hunk(
            &parse_diff_hunk(&long()),
            &ClipOpts {
                line: 12,
                side: "RIGHT",
                start_line: 10,
                context: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(
            texts(&clipped),
            [
                "const unused = 1",
                "const unused2 = 2",
                "const unused3 = 3",
                "const secret = process.env.OAUTH_CLIENT_SECRET",
                "const secret = await secrets.get('oauth/client_secret')",
            ]
        );
        let in_range: Vec<_> = clipped.iter().filter(|l| l.in_range).cloned().collect();
        assert_eq!(
            texts(&in_range),
            [
                "const unused2 = 2",
                "const unused3 = 3",
                "const secret = process.env.OAUTH_CLIENT_SECRET",
                "const secret = await secrets.get('oauth/client_secret')",
            ]
        );
        assert!(clipped.last().expect("last").target);
    }

    #[test]
    fn path_label_appends_the_line_when_github_gave_one() {
        assert_eq!(comment_hunk_line_label(12, 0), "12");
        assert_eq!(comment_hunk_line_label(12, 10), "10-12");
        assert_eq!(
            comment_hunk_path_label("src/auth/oauth.ts", 12, 0, false),
            "src/auth/oauth.ts:12"
        );
        assert_eq!(
            comment_hunk_path_label("src/auth/oauth.ts", 12, 10, false),
            "src/auth/oauth.ts:10-12"
        );
        assert_eq!(
            comment_hunk_path_label("src/auth/oauth.ts", 0, 0, false),
            "src/auth/oauth.ts"
        );
        assert_eq!(comment_hunk_path_label("", 12, 0, false), "");
        assert_eq!(
            comment_hunk_path_label("b.go", 9, 7, true),
            "b.go:7-9 (outdated)"
        );
    }
}
