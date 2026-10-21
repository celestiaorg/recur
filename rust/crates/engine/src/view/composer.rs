//! The chat composer's text model (the DOM-free half of frontend
//! `lib/composerDom.ts`): wire text ↔ inline chips (`@file`, `/skill`,
//! `@terminal`, `@artifact`, `⟦attachment:id⟧`, `/prompt` arg slots), one
//! line per paragraph. The GPUI composer renders [`Segment`]s; caret, focus
//! and DOM normalization stay in the view.

use std::sync::LazyLock;

use recur_bundles::PromptArgSpec;
use regex::Regex;

use super::js;
use super::mentions::{
    artifact_at_hits, artifact_mention_token, attachment_hits, attachment_marker,
    collect_loose_at_names, dir_file_mention_path, file_at_hits, file_mention_token,
    is_dir_file_mention, repair_file_mention_boundaries, skill_mention_token, skill_slash_hits,
    terminal_at_hits, terminal_mention_token, token_span_overlaps,
};

/// A pasted image draft.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposerAtt {
    pub id: String,
    pub data_url: String,
    pub name: String,
}

/// A Discord-style `/slash` chip with labeled arg slots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptChipSpec {
    pub slash: String,
    pub args: Vec<PromptArgSpec>,
}

/// One inline run of a composer line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    /// `path` is the chip path (`src/a.ts:12-40`, `src/`).
    File {
        path: String,
        dir: bool,
    },
    Skill(String),
    /// Empty `id` is a bare `@terminal`.
    Terminal {
        id: String,
        title: String,
    },
    /// Empty `id` is a bare `@artifact`.
    Artifact {
        id: String,
        title: String,
    },
    Attachment(ComposerAtt),
    Prompt {
        spec: PromptChipSpec,
        values: Vec<String>,
    },
}

pub fn is_dir_chip_path(path: &str) -> bool {
    is_dir_file_mention(path)
}

fn file_chip_wire(path: &str, dir: bool) -> String {
    if dir || is_dir_file_mention(path) {
        file_mention_token(&dir_file_mention_path(path))
    } else {
        file_mention_token(path)
    }
}

impl Segment {
    /// The segment's wire text.
    pub fn wire(&self) -> String {
        match self {
            Segment::Text(t) => t.clone(),
            Segment::File { path, dir } => file_chip_wire(path, *dir),
            Segment::Skill(name) => skill_mention_token(name),
            Segment::Terminal { id, .. } => terminal_mention_token(id),
            Segment::Artifact { id, .. } => artifact_mention_token(id),
            Segment::Attachment(a) => attachment_marker(&a.id),
            Segment::Prompt { spec, values } => encode_prompt_wire(&spec.slash, values),
        }
    }
}

fn prompt_arg_token(v: &str) -> String {
    if v.is_empty() {
        return "\"\"".to_owned();
    }
    if v.chars().any(|c| js::is_space(c) || c == '"') {
        return format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""));
    }
    v.to_owned()
}

/// The wire form: `/slash` plus the trailing arg values (slots with spaces
/// quoted). All-empty slots leave a trailing space.
pub fn encode_prompt_wire(slash: &str, values: &[String]) -> String {
    let s = js::trim(slash.strip_prefix('/').unwrap_or(slash));
    let filled: Vec<&str> = values.iter().map(|v| js::trim(v)).collect();
    if filled.iter().all(|v| v.is_empty()) {
        return format!("/{s} ");
    }
    if filled.len() <= 1 {
        return js::trim_end(&format!(
            "/{s} {}",
            filled.first().copied().unwrap_or_default()
        ))
        .to_owned();
    }
    let joined: Vec<String> = filled.iter().map(|v| prompt_arg_token(v)).collect();
    js::trim_end(&format!("/{s} {}", joined.join(" "))).to_owned()
}

/// Splits on whitespace; `"quoted"` tokens keep their spaces (`\"` and `\\`
/// escapes).
pub fn split_prompt_arg_tokens(raw: &str) -> Vec<String> {
    let s: Vec<char> = js::trim(raw).chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        while i < s.len() && js::is_space(s[i]) {
            i += 1;
        }
        if i >= s.len() {
            break;
        }
        if s[i] == '"' {
            i += 1;
            let mut tok = String::new();
            while i < s.len() {
                if s[i] == '\\' && i + 1 < s.len() {
                    tok.push(s[i + 1]);
                    i += 2;
                    continue;
                }
                if s[i] == '"' {
                    i += 1;
                    break;
                }
                tok.push(s[i]);
                i += 1;
            }
            out.push(tok);
            continue;
        }
        let mut j = i;
        while j < s.len() && !js::is_space(s[j]) {
            j += 1;
        }
        out.push(s[i..j].iter().collect());
        i = j;
    }
    out
}

/// The slot values of `rest`: one slot takes it all; several split on
/// whitespace with the remainder on the last.
pub fn parse_prompt_chip_values(rest: &str, spec: &PromptChipSpec) -> Vec<String> {
    let n = spec.args.len().max(1);
    let raw = js::trim(rest);
    if n <= 1 {
        return vec![raw.to_owned()];
    }
    let parts = split_prompt_arg_tokens(raw);
    (0..spec.args.len())
        .map(|i| {
            if i == n - 1 {
                parts.get(i..).map(|p| p.join(" ")).unwrap_or_default()
            } else {
                parts.get(i).cloned().unwrap_or_default()
            }
        })
        .collect()
}

static PROMPT_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^/([a-z][a-z0-9-]*)(?:[{}]+([\s\S]*))?$",
        js::WS_CLASS
    ))
    .expect("prompt line re")
});

/// A whole line that is a takes-args `/slash` with its values.
pub fn match_prompt_chip_line(
    line: &str,
    specs: &[PromptChipSpec],
) -> Option<(PromptChipSpec, Vec<String>)> {
    if specs.is_empty() {
        return None;
    }
    let c = PROMPT_LINE.captures(js::trim(line))?;
    let spec = specs.iter().find(|s| s.slash == c[1])?;
    if spec.args.is_empty() {
        return None;
    }
    let values = parse_prompt_chip_values(c.get(2).map_or("", |m| m.as_str()), spec);
    Some((spec.clone(), values))
}

/// How [`hydrate_composer`] restores chips.
#[derive(Default)]
pub struct HydrateOpts<'a> {
    /// Recreate `@file`, `@terminal` and `@artifact` chips (tab restore).
    /// Off while typing: only a picker commit inserts chips.
    pub chip_files: bool,
    pub chip_skills: bool,
    pub terminal_title: Option<&'a dyn Fn(&str) -> String>,
    pub artifact_title: Option<&'a dyn Fn(&str) -> String>,
    pub artifact_id_for_title: Option<&'a dyn Fn(&str) -> String>,
    pub prompt_chips: &'a [PromptChipSpec],
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WireKind {
    File(String),
    Skill(String),
    Att(String),
    Terminal(String),
    Artifact(String),
}

#[derive(Debug, Clone)]
struct WireTok {
    index: usize,
    len: usize,
    kind: WireKind,
}

fn wire_tokens(text: &str, artifact_id_for_title: Option<&dyn Fn(&str) -> String>) -> Vec<WireTok> {
    let mut toks: Vec<WireTok> = Vec::new();
    let mut push = |index: usize, end: usize, kind: WireKind| {
        toks.push(WireTok {
            index,
            len: end - index,
            kind,
        })
    };
    for h in artifact_at_hits(text) {
        push(
            h.start,
            h.end,
            WireKind::Artifact(h.body.unwrap_or_default().to_owned()),
        );
    }
    for h in terminal_at_hits(text) {
        push(
            h.start,
            h.end,
            WireKind::Terminal(h.body.unwrap_or_default().to_owned()),
        );
    }
    for h in file_at_hits(text) {
        if let Some(p) = h.body {
            push(h.start, h.end, WireKind::File(p.to_owned()));
        }
    }
    for h in skill_slash_hits(text) {
        if let Some(n) = h.body {
            push(h.start, h.end, WireKind::Skill(n.to_owned()));
        }
    }
    for h in attachment_hits(text) {
        if let Some(id) = h.body {
            push(h.start, h.end, WireKind::Att(id.to_owned()));
        }
    }
    if let Some(lookup) = artifact_id_for_title {
        for hit in collect_loose_at_names(text) {
            let id = lookup(&hit.token);
            if id.is_empty() {
                continue;
            }
            let spans: Vec<(usize, usize)> = toks.iter().map(|t| (t.index, t.len)).collect();
            if token_span_overlaps(hit.index, hit.len, &spans) {
                continue;
            }
            toks.push(WireTok {
                index: hit.index,
                len: hit.len,
                kind: WireKind::Artifact(id),
            });
        }
    }
    toks.sort_by_key(|t| t.index);
    toks
}

fn push_text(out: &mut Vec<Segment>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(Segment::Text(t)) = out.last_mut() {
        t.push_str(s);
    } else {
        out.push(Segment::Text(s.to_owned()));
    }
}

fn fill_line(
    line: &str,
    attachments: &[ComposerAtt],
    opts: &HydrateOpts<'_>,
    seen_att: &mut Vec<String>,
) -> Vec<Segment> {
    let mut out = Vec::new();
    if line.is_empty() {
        return out;
    }
    if let Some((spec, values)) = match_prompt_chip_line(line, opts.prompt_chips) {
        out.push(Segment::Prompt { spec, values });
        return out;
    }
    let title = |f: Option<&dyn Fn(&str) -> String>, id: &str| f.map(|f| f(id)).unwrap_or_default();
    let mut last = 0;
    for tok in wire_tokens(line, opts.artifact_id_for_title) {
        if tok.index < last {
            continue;
        }
        push_text(&mut out, &line[last..tok.index]);
        let raw = &line[tok.index..tok.index + tok.len];
        match tok.kind {
            WireKind::File(path) if opts.chip_files => out.push(Segment::File {
                dir: is_dir_file_mention(&path),
                path,
            }),
            WireKind::Skill(name) if opts.chip_skills => out.push(Segment::Skill(name)),
            WireKind::Terminal(id) if opts.chip_files => out.push(Segment::Terminal {
                title: title(opts.terminal_title, &id),
                id,
            }),
            WireKind::Artifact(id) if opts.chip_files => out.push(Segment::Artifact {
                title: title(opts.artifact_title, &id),
                id,
            }),
            WireKind::Att(id) => {
                match attachments
                    .iter()
                    .find(|a| a.id == id && !a.data_url.is_empty())
                {
                    Some(a) => out.push(Segment::Attachment(a.clone())),
                    None => push_text(&mut out, &attachment_marker(&id)),
                }
                seen_att.push(id);
            }
            _ => push_text(&mut out, raw),
        }
        last = tok.index + tok.len;
    }
    push_text(&mut out, &line[last..]);
    out
}

/// Lines of segments from wire text. Image drafts not present in the text are
/// appended to the last line.
pub fn hydrate_composer(
    text: &str,
    attachments: &[ComposerAtt],
    opts: &HydrateOpts<'_>,
) -> Vec<Vec<Segment>> {
    let text = if opts.chip_files {
        repair_file_mention_boundaries(text)
    } else {
        text.to_owned()
    };
    let mut seen_att = Vec::new();
    let mut lines: Vec<Vec<Segment>> = text
        .split('\n')
        .map(|line| fill_line(line, attachments, opts, &mut seen_att))
        .collect();
    let orphans = attachments
        .iter()
        .filter(|a| !seen_att.contains(&a.id) && !a.data_url.is_empty());
    if let Some(last) = lines.last_mut() {
        last.extend(orphans.cloned().map(Segment::Attachment));
    }
    lines
}

/// Wire text of the composer lines.
pub fn serialize_composer(lines: &[Vec<Segment>]) -> String {
    lines
        .iter()
        .map(|l| l.iter().map(Segment::wire).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_filler(s: &Segment) -> bool {
    matches!(s, Segment::Text(t) if t.chars().all(|c| c == '\u{200B}' || c == '\u{FEFF}'))
}

fn needs_boundary(s: &Segment) -> bool {
    matches!(
        s,
        Segment::File { .. }
            | Segment::Skill(_)
            | Segment::Terminal { .. }
            | Segment::Artifact { .. }
    )
}

/// Restores the whitespace boundary before every file/skill/terminal/artifact
/// chip (and after terminal/artifact chips, whose collectors require a
/// trailing space), so serialization keeps them mentions.
pub fn ensure_chip_boundaries(lines: &mut [Vec<Segment>]) {
    for line in lines.iter_mut() {
        let mut i = 0;
        while i < line.len() {
            if !needs_boundary(&line[i]) {
                i += 1;
                continue;
            }
            let trailing = matches!(line[i], Segment::Terminal { .. } | Segment::Artifact { .. });
            if let Some(p) = (0..i).rev().find(|&p| !is_filler(&line[p])) {
                match &mut line[p] {
                    Segment::Text(t) => {
                        if t.chars().next_back().is_some_and(|c| !js::is_space(c)) {
                            t.push(' ');
                        }
                    }
                    _ => {
                        line.insert(i, Segment::Text(" ".into()));
                        i += 1;
                    }
                }
            }
            if trailing && let Some(n) = (i + 1..line.len()).find(|&n| !is_filler(&line[n])) {
                match &mut line[n] {
                    Segment::Text(t) => {
                        if t.chars().next().is_some_and(|c| !js::is_space(c)) {
                            t.insert(0, ' ');
                        }
                    }
                    _ => line.insert(n, Segment::Text(" ".into())),
                }
            }
            i += 1;
        }
    }
}

static NEWLINE_AFTER_BLANKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"^[[{}]&&[^\n]]*\n", js::WS_CLASS)).expect("blank line re")
});

/// The first prompt chip's args plus any text typed after it, so a note after
/// `/linear ENG-1` reaches the prompt (multi-arg prompts fold it into the
/// last arg). `None` without a prompt chip.
pub fn prompt_chip_args_with_trailing(lines: &[Vec<Segment>]) -> Option<String> {
    let wire = lines.iter().flatten().find_map(|s| match s {
        Segment::Prompt { .. } => Some(s.wire()),
        _ => None,
    })?;
    let slash = lines.iter().flatten().find_map(|s| match s {
        Segment::Prompt { spec, .. } => Some(format!("/{}", spec.slash)),
        _ => None,
    })?;
    let args = wire
        .strip_prefix(&slash)
        .map(|a| js::trim(a).to_owned())
        .unwrap_or_default();
    let full = serialize_composer(lines);
    let rest = full.find(&wire).map_or("", |at| &full[at + wire.len()..]);
    let tail = js::trim(rest);
    if tail.is_empty() {
        return Some(args);
    }
    if args.is_empty() {
        return Some(tail.to_owned());
    }
    let sep = if NEWLINE_AFTER_BLANKS.is_match(rest) {
        "\n"
    } else {
        " "
    };
    Some(format!("{args}{sep}{tail}"))
}

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;
