//! Chat wire-text mention rules: `@file`,
//! `@terminal`, `@artifact`, loose `@Title`, `/skill` and `⟦attachment:id⟧`
//! tokens, their chips and labels, and the composer's caret queries.
//!
//! The UI regexes use JS lookbehind/lookahead (`(?:^|(?<=\s))@…(?=\s|$)`),
//! which the `regex` crate lacks. Each pattern here is the lookaround-free
//! core plus an explicit boundary check; none of the cores can contain a
//! second token start, so skipping a rejected match never hides a later one.
//! Offsets are byte offsets.

use std::sync::LazyLock;

use regex::Regex;

use super::js;

/// `⟦attachment:<id>⟧`, embedded in wire and transcript text where an image
/// was pasted.
pub static ATTACHMENT_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("⟦attachment:([^⟧]+)⟧").expect("attachment re"));

/// The path body of an `@file` mention (no loc suffix). Absolute segments
/// omit `:` so a `:line` span can follow.
fn file_at_path() -> String {
    let ws = js::WS_CLASS;
    format!(
        r"/(?:[^{ws}@:]+(?:/[^{ws}@:]+)*)|(?:[0-9A-Za-z_.-]+/)+[0-9A-Za-z_./+-]+|[0-9A-Za-z_.-]+/|[0-9A-Za-z_.-]+\.[A-Za-z][0-9A-Za-z_.-]*|\.[A-Za-z0-9_][0-9A-Za-z_.-]*"
    )
}

/// The subset [`repair_file_mention_boundaries`] uses: no bare `name.ext`
/// (emails).
fn file_at_path_slash_or_dot() -> String {
    let ws = js::WS_CLASS;
    format!(
        r"/(?:[^{ws}@:]+(?:/[^{ws}@:]+)*)|(?:[0-9A-Za-z_.-]+/)+[0-9A-Za-z_./+-]+|[0-9A-Za-z_.-]+/|\.[A-Za-z0-9_][0-9A-Za-z_.-]*"
    )
}

/// `[:line[:col][-line[:col]]]` after a path.
const FILE_AT_LOC: &str = r":[0-9]+(?::[0-9]+)?(?:-[0-9]+(?::[0-9]+)?)?";

static FILE_AT_CORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!("@((?:{})(?:{FILE_AT_LOC})?)", file_at_path())).expect("file at re")
});

static FILE_AT_REPAIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "@((?:{})(?:{FILE_AT_LOC})?)",
        file_at_path_slash_or_dot()
    ))
    .expect("file repair re")
});

static TERMINAL_AT_CORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!("@terminal(?::([^{}@]+))?", js::WS_CLASS)).expect("terminal re")
});

static ARTIFACT_AT_CORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!("@artifact(?::([^{}@]+))?", js::WS_CLASS)).expect("artifact re")
});

static AT_NAME_CORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("@([^{}@]+)", js::WS_CLASS)).expect("at name re"));

static SKILL_SLASH_CORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("/([a-z][a-z0-9-]*)").expect("skill re"));

static FILE_MENTION_LOC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r":([0-9]+)(?::([0-9]+))?(?:-([0-9]+)(?::([0-9]+))?)?$").expect("loc re")
});

static TRAILING_BLANKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("[ \t]+\n").expect("blanks re"));
static EXTRA_NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new("\n{3,}").expect("nl re"));
static DOUBLE_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new("  +").expect("spaces re"));

/// One token match: the whole span and the captured body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit<'a> {
    pub start: usize,
    pub end: usize,
    /// The capture (path, qualifier, name, id); `None` when the optional
    /// qualifier is absent.
    pub body: Option<&'a str>,
}

fn hits<'a>(re: &Regex, text: &'a str, trailing: impl Fn(&str, usize) -> bool) -> Vec<Hit<'a>> {
    re.captures_iter(text)
        .filter_map(|c| {
            let m = c.get(0)?;
            (js::space_before(text, m.start()) && trailing(text, m.end())).then(|| Hit {
                start: m.start(),
                end: m.end(),
                body: c.get(1).map(|g| g.as_str()),
            })
        })
        .collect()
}

/// `FILE_AT_RE` matches: an `@path[:loc]` at the start or after whitespace.
pub fn file_at_hits(text: &str) -> Vec<Hit<'_>> {
    hits(&FILE_AT_CORE, text, |_, _| true)
}

/// `TERMINAL_AT_RE` matches (`@terminal` / `@terminal:<qual>`).
pub fn terminal_at_hits(text: &str) -> Vec<Hit<'_>> {
    hits(&TERMINAL_AT_CORE, text, js::space_after)
}

/// `ARTIFACT_AT_RE` matches (`@artifact` / `@artifact:<qual>`).
pub fn artifact_at_hits(text: &str) -> Vec<Hit<'_>> {
    hits(&ARTIFACT_AT_CORE, text, js::space_after)
}

/// `AT_NAME_RE` matches: a whitespace-bounded `@token`.
pub fn at_name_hits(text: &str) -> Vec<Hit<'_>> {
    hits(&AT_NAME_CORE, text, js::space_after)
}

/// `SKILL_SLASH_RE` matches: `/skill-name` (lowercase Agent Skills names, so
/// absolute paths are not chewed), not followed by `[\w/-]`.
pub fn skill_slash_hits(text: &str) -> Vec<Hit<'_>> {
    hits(&SKILL_SLASH_CORE, text, |t, i| {
        js::char_at(t, i).is_none_or(|c| !(js::is_word(c) || c == '/' || c == '-'))
    })
}

/// `⟦attachment:id⟧` matches.
pub fn attachment_hits(text: &str) -> Vec<Hit<'_>> {
    ATTACHMENT_MARKER_RE
        .captures_iter(text)
        .filter_map(|c| {
            let m = c.get(0)?;
            Some(Hit {
                start: m.start(),
                end: m.end(),
                body: c.get(1).map(|g| g.as_str()),
            })
        })
        .collect()
}

fn remove_spans(text: &str, spans: &[Hit<'_>], with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for h in spans {
        out.push_str(&text[last..h.start]);
        out.push_str(with);
        last = h.end;
    }
    out.push_str(&text[last..]);
    out
}

fn tidy_lines(s: &str) -> String {
    EXTRA_NEWLINES
        .replace_all(&TRAILING_BLANKS.replace_all(s, "\n"), "\n\n")
        .into_owned()
}

fn chip_qualifier<'a>(chip: &'a str, keyword: &str) -> &'a str {
    let raw = js::trim(chip);
    let raw = raw.strip_prefix('@').unwrap_or(raw);
    if raw == keyword {
        return "";
    }
    raw.strip_prefix(keyword)
        .and_then(|r| r.strip_prefix(':'))
        .unwrap_or_default()
}

fn chip_label(qual: &str, title: &str, fallback: &str) -> String {
    let t = js::trim(title);
    if !t.is_empty() {
        return t.to_owned();
    }
    if qual.is_empty() {
        return fallback.to_owned();
    }
    qual.chars().take(8).collect()
}

fn unique_tokens(found: Vec<Hit<'_>>, keyword: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in found {
        let token = match h.body {
            Some(q) if !q.is_empty() => format!("{keyword}:{q}"),
            _ => keyword.to_owned(),
        };
        if !out.contains(&token) {
            out.push(token);
        }
    }
    out
}

/// The token inserted when the user picks a terminal from the `@` menu.
pub fn terminal_mention_token(id: &str) -> String {
    let q = js::trim(id);
    if q.is_empty() {
        "@terminal".to_owned()
    } else {
        format!("@terminal:{q}")
    }
}

/// A terminal chip's label: the tab title when known, else a short id.
pub fn terminal_mention_label(chip: &str, title: &str) -> String {
    chip_label(parse_terminal_mention_chip(chip), title, "terminal")
}

/// The qualifier after `@terminal:` (session id or title); empty for a bare
/// `@terminal`.
pub fn parse_terminal_mention_chip(chip: &str) -> &str {
    chip_qualifier(chip, "terminal")
}

/// Unique `@terminal` chips in wire text, first-seen order.
pub fn extract_terminal_mentions(text: &str) -> Vec<String> {
    unique_tokens(terminal_at_hits(text), "terminal")
}

/// The token inserted when the user picks an artifact from the `@` menu.
pub fn artifact_mention_token(id: &str) -> String {
    let q = js::trim(id);
    if q.is_empty() {
        "@artifact".to_owned()
    } else {
        format!("@artifact:{q}")
    }
}

/// An artifact chip's label: the title when known, else a short id.
pub fn artifact_mention_label(chip: &str, title: &str) -> String {
    chip_label(parse_artifact_mention_chip(chip), title, "artifact")
}

/// The qualifier after `@artifact:` (id or title); empty for a bare
/// `@artifact`.
pub fn parse_artifact_mention_chip(chip: &str) -> &str {
    chip_qualifier(chip, "artifact")
}

/// Unique `@artifact` chips in wire text, first-seen order.
pub fn extract_artifact_mentions(text: &str) -> Vec<String> {
    unique_tokens(artifact_at_hits(text), "artifact")
}

/// `@name` tokens that are the `@artifact` / `@terminal` keywords, not panel
/// titles.
pub fn reserved_at_name(token: &str) -> bool {
    let t = js::trim(token).to_lowercase();
    t == "artifact" || t.starts_with("artifact:") || t == "terminal" || t.starts_with("terminal:")
}

/// Whether `@token` as a whole is a file mention (path, name.ext, dotfile).
pub fn at_name_is_file_mention(token: &str) -> bool {
    let s = format!("@{token}");
    file_at_hits(&s)
        .first()
        .is_some_and(|h| h.start == 0 && h.end == s.len())
}

/// The id of the one artifact titled `title`. Empty when reserved, file-like,
/// missing, duplicated, or containing whitespace (not an `@Name` token).
pub fn unique_artifact_id_for_title<'a>(
    title: &str,
    artifacts: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let q = js::trim(title).to_lowercase();
    if q.is_empty()
        || reserved_at_name(&q)
        || at_name_is_file_mention(&q)
        || q.chars().any(js::is_space)
    {
        return String::new();
    }
    let hits: Vec<&str> = artifacts
        .into_iter()
        .filter(|(_, t)| js::trim(t).to_lowercase() == q)
        .map(|(id, _)| id)
        .collect();
    match hits.as_slice() {
        [id] => (*id).to_owned(),
        _ => String::new(),
    }
}

/// A loose `@Title` token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtNameHit {
    pub index: usize,
    pub len: usize,
    pub token: String,
}

/// Loose `@Title` tokens that are not `@artifact`, `@terminal` or a file.
pub fn collect_loose_at_names(text: &str) -> Vec<AtNameHit> {
    at_name_hits(text)
        .into_iter()
        .filter_map(|h| {
            let token = h.body?;
            (!reserved_at_name(token) && !at_name_is_file_mention(token)).then(|| AtNameHit {
                index: h.start,
                len: h.end - h.start,
                token: token.to_owned(),
            })
        })
        .collect()
}

/// Whether `[index, index+len)` overlaps any span.
pub fn token_span_overlaps(index: usize, len: usize, spans: &[(usize, usize)]) -> bool {
    spans.iter().any(|&(i, l)| index < i + l && index + len > i)
}

/// A parsed `@file` chip: the workspace path plus an optional 1-based line or
/// column span (0 = unset).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMentionRef {
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub start_col: i64,
    pub end_col: i64,
}

fn positive_int(raw: Option<&str>) -> i64 {
    match raw.and_then(|r| r.parse::<f64>().ok()) {
        Some(n) if n.is_finite() && n > 0.0 => n as i64,
        _ => 0,
    }
}

/// Formats a chip: `path`, `path:12`, `path:12-40`, `path:12:5-12:18` or
/// `path:12:5-13:20`. Column spans always write both `line:col` ends.
pub fn format_file_mention_chip(
    path: &str,
    start_line: i64,
    end_line: i64,
    start_col: i64,
    end_col: i64,
) -> String {
    let p = js::trim(path);
    if p.is_empty() {
        return String::new();
    }
    if start_line <= 0 {
        return p.to_owned();
    }
    let col_start = start_col.max(0);
    let col_end = end_col.max(0);
    if col_start > 0 && col_end > 0 {
        let end_ln = if end_line >= start_line {
            end_line
        } else {
            start_line
        };
        let end = if end_ln == start_line && col_end < col_start {
            col_start
        } else {
            col_end
        };
        return format!("{p}:{start_line}:{col_start}-{end_ln}:{end}");
    }
    if end_line > start_line {
        return format!("{p}:{start_line}-{end_line}");
    }
    format!("{p}:{start_line}")
}

pub fn format_file_mention_ref(r: &FileMentionRef) -> String {
    format_file_mention_chip(&r.path, r.start_line, r.end_line, r.start_col, r.end_col)
}

/// A trailing slash marks a directory mention (`src/` vs a file `src`).
pub fn dir_file_mention_path(path: &str) -> String {
    let p = js::trim(path).trim_end_matches('/');
    if p.is_empty() {
        String::new()
    } else {
        format!("{p}/")
    }
}

pub fn is_dir_file_mention(chip: &str) -> bool {
    let c = js::trim(chip);
    c.strip_prefix('@').unwrap_or(c).ends_with('/')
}

/// Splits `path[:loc]` into the path and its span. An invalid loc degrades
/// to the path (or the line alone).
pub fn parse_file_mention_chip(chip: &str) -> FileMentionRef {
    let raw = js::trim(chip).trim_end_matches('/');
    let whole = || FileMentionRef {
        path: raw.to_owned(),
        ..FileMentionRef::default()
    };
    if raw.is_empty() {
        return FileMentionRef::default();
    }
    let Some(c) = FILE_MENTION_LOC_RE.captures(raw) else {
        return whole();
    };
    let at = c.get(0).map_or(0, |m| m.start());
    let path = &raw[..at];
    if path.is_empty() {
        return whole();
    }
    let path = path.to_owned();
    let start_line = positive_int(c.get(1).map(|m| m.as_str()));
    if start_line == 0 {
        return FileMentionRef {
            path,
            ..FileMentionRef::default()
        };
    }
    let start_col = positive_int(c.get(2).map(|m| m.as_str()));
    let end_line = positive_int(c.get(3).map(|m| m.as_str()));
    let end_col = positive_int(c.get(4).map(|m| m.as_str()));
    let line_only = |path: String| FileMentionRef {
        path,
        start_line,
        ..FileMentionRef::default()
    };
    if start_col > 0 && end_col > 0 {
        let end_ln = if end_line > 0 { end_line } else { start_line };
        if end_ln < start_line || (end_ln == start_line && end_col < start_col) {
            return line_only(path);
        }
        return FileMentionRef {
            path,
            start_line,
            end_line: if end_ln > start_line { end_ln } else { 0 },
            start_col,
            end_col,
        };
    }
    if end_line > start_line {
        return FileMentionRef {
            path,
            start_line,
            end_line,
            ..FileMentionRef::default()
        };
    }
    line_only(path)
}

/// A file chip's label: the basename plus `(12)`, `(12-40)` or
/// `(12:5-12:18)`.
pub fn file_mention_label(chip: &str) -> String {
    let r = parse_file_mention_chip(chip);
    let base = r.path.rsplit('/').next().unwrap_or_default();
    if r.start_line == 0 {
        return if base.is_empty() {
            chip.to_owned()
        } else {
            base.to_owned()
        };
    }
    if r.start_col > 0 && r.end_col > 0 {
        let end_ln = if r.end_line >= r.start_line {
            r.end_line
        } else {
            r.start_line
        };
        return format!(
            "{base} ({}:{}-{end_ln}:{})",
            r.start_line, r.start_col, r.end_col
        );
    }
    if r.end_line > r.start_line {
        return format!("{base} ({}-{})", r.start_line, r.end_line);
    }
    format!("{base} ({})", r.start_line)
}

pub fn attachment_marker(id: &str) -> String {
    format!("⟦attachment:{id}⟧")
}

/// The token inserted when the user picks a skill from the `/` menu.
pub fn skill_mention_token(name: &str) -> String {
    format!("/{}", js::trim(name))
}

/// Text with attachment markers stripped.
pub fn strip_attachment_markers(text: &str) -> String {
    let stripped = ATTACHMENT_MARKER_RE.replace_all(text, "");
    EXTRA_NEWLINES.replace_all(&stripped, "\n\n").into_owned()
}

/// Text with `@path` mentions stripped.
pub fn strip_file_mentions(text: &str) -> String {
    tidy_lines(&remove_spans(text, &file_at_hits(text), ""))
}

pub fn strip_terminal_mentions(text: &str) -> String {
    tidy_lines(&remove_spans(text, &terminal_at_hits(text), ""))
}

pub fn strip_artifact_mentions(text: &str) -> String {
    tidy_lines(&remove_spans(text, &artifact_at_hits(text), ""))
}

/// Text with `/skill-name` tokens stripped.
pub fn strip_skill_mentions(text: &str) -> String {
    let s = tidy_lines(&remove_spans(text, &skill_slash_hits(text), " "));
    DOUBLE_SPACES.replace_all(&s, " ").into_owned()
}

/// Plain composer text with attachment markers and every mention kind
/// removed.
pub fn strip_composer_markers(text: &str) -> String {
    strip_skill_mentions(&strip_artifact_mentions(&strip_terminal_mentions(
        &strip_file_mentions(&strip_attachment_markers(text)),
    )))
}

/// The wire/transcript string: inline markers stay where they are; any draft
/// not yet represented is appended on its own line.
pub fn compose_marked_text<'a>(text: &str, draft_ids: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = text.to_owned();
    for id in draft_ids {
        let marker = attachment_marker(id);
        if out.contains(&marker) {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&marker);
    }
    out
}

/// The attachment ids present as markers, in order.
pub fn extract_attachment_ids(text: &str) -> Vec<String> {
    attachment_hits(text)
        .into_iter()
        .filter_map(|h| h.body.map(str::to_owned))
        .collect()
}

/// Whether two user-bubble bodies are the same send: the caption matches and
/// attachment ids match when both sides carry them.
pub fn same_user_chat_body(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if js::trim(&strip_attachment_markers(a)) != js::trim(&strip_attachment_markers(b)) {
        return false;
    }
    let ids_a = extract_attachment_ids(a);
    let ids_b = extract_attachment_ids(b);
    ids_a.is_empty() || ids_b.is_empty() || ids_a == ids_b
}

/// The token inserted when the user picks a file from the `@` menu.
pub fn file_mention_token(chip: &str) -> String {
    format!("@{}", js::trim(chip))
}

/// Removes the first `@path` mention, eating one adjacent space so "how does
/// @A compare" becomes "how does compare".
pub fn remove_file_mention_token(text: &str, chip: &str) -> String {
    let token = file_mention_token(chip);
    if token == "@" {
        return text.to_owned();
    }
    let Some(idx) = text.find(&token) else {
        return text.to_owned();
    };
    let mut start = idx;
    let mut end = idx + token.len();
    if text[end..].starts_with(' ') {
        end += 1;
    } else if start > 0 && text[..start].ends_with(' ') {
        start -= 1;
    }
    format!("{}{}", &text[..start], &text[end..])
}

/// An active `@` or `/` query at the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionQuery {
    pub start: usize,
    pub end: usize,
    pub query: String,
}

/// An in-progress `@mention`: `@` at the start or after whitespace, with the
/// caret inside the following non-space query.
pub fn mention_query_at(text: &str, caret: usize) -> Option<MentionQuery> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    for (i, c) in text[..caret].char_indices().rev() {
        if js::is_space(c) {
            return None;
        }
        if c == '@' {
            if !js::space_before(text, i) {
                return None;
            }
            let query = &text[i + 1..caret];
            if query.contains('@') {
                return None;
            }
            return Some(MentionQuery {
                start: i,
                end: caret,
                query: query.to_owned(),
            });
        }
    }
    None
}

fn skill_name_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}

/// An in-progress `/skill` query: `/` at the start or after whitespace, the
/// caret inside the following skill-name characters.
pub fn skill_query_at(text: &str, caret: usize) -> Option<MentionQuery> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let head = text[..caret].trim_end_matches(skill_name_char);
    let i = head.len().checked_sub(1)?;
    if !head.ends_with('/') || !js::space_before(text, i) {
        return None;
    }
    Some(MentionQuery {
        start: i,
        end: caret,
        query: text[i + 1..caret].to_owned(),
    })
}

/// Where a picked file chip goes: an in-progress `@` or `/` query at the
/// caret is replaced (unless `strip_query` is false, for a caret flush after
/// a committed chip), with a whitespace boundary before the chip.
pub fn prepare_file_mention_insert(
    text: &str,
    caret: usize,
    strip_query: bool,
) -> (String, String) {
    let mut at = caret.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let q = if strip_query {
        mention_query_at(text, at).or_else(|| skill_query_at(text, at))
    } else {
        None
    };
    let (from, to) = q.map_or((at, at), |q| (q.start, q.end));
    let mut before = text[..from].to_owned();
    let after = text[to..].to_owned();
    if before.chars().next_back().is_some_and(|c| !js::is_space(c)) {
        before.push(' ');
    }
    (before, after)
}

/// Inserts a space before `@file` tokens that lost their whitespace boundary.
/// Only slashy and leading-dot paths are repaired, so emails stay intact.
pub fn repair_file_mention_boundaries(text: &str) -> String {
    if !text.contains('@') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut last = 0;
    for m in FILE_AT_REPAIR.find_iter(text) {
        out.push_str(&text[last..m.start()]);
        if !js::space_before(text, m.start()) {
            out.push(' ');
        }
        out.push_str(m.as_str());
        last = m.end();
    }
    out.push_str(&text[last..]);
    out
}

/// Unique `@path` tokens in wire text, first-seen order.
pub fn extract_file_mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for h in file_at_hits(text) {
        if let Some(p) = h.body
            && !p.is_empty()
            && !out.iter().any(|x| x == p)
        {
            out.push(p.to_owned());
        }
    }
    out
}

#[cfg(test)]
#[path = "mentions_tests.rs"]
mod tests;
