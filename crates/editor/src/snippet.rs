//! Snippets: `@codemirror/autocomplete`'s `snippet()` templates and
//! tab-stop fields, plus Recur's bundle snippet catalog:
//! tab-trigger expansion, `$TM_SELECTED_TEXT` / `$ISO_DATE`, VS Code `$n`
//! rewriting and the completion source.

use std::cell::Cell;
use std::rc::Rc;

use futures::future::LocalBoxFuture;
use recur_textmate::JsRegex;

use crate::change::{Change, ChangeDesc, MapMode};
use crate::keyequiv::{effective_key_equivalent, format_key_equivalent_display};
use crate::selection::{EditorSelection, SelectionRange};
use crate::state::{Dispatch, EditorState, Effect, Transaction, TransactionSpec, is_js_space};
use crate::text::locale_compare;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FieldPos {
    field: usize,
    line: usize,
    from: usize,
    to: usize,
}

/// A field's current extent in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldRange {
    pub field: usize,
    pub from: usize,
    pub to: usize,
}

impl FieldRange {
    fn map(&self, changes: &ChangeDesc) -> Option<FieldRange> {
        Some(FieldRange {
            field: self.field,
            from: changes.map_pos_mode(self.from, -1, MapMode::TrackDel)?,
            to: changes.map_pos_mode(self.to, 1, MapMode::TrackDel)?,
        })
    }
}

/// A parsed `${…}` / `#{…}` template (`Snippet`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    lines: Vec<String>,
    field_positions: Vec<FieldPos>,
}

struct Field {
    seq: Option<u64>,
    name: String,
}

fn placeholder_re() -> &'static JsRegex {
    thread_local! {
        static RE: &'static JsRegex = Box::leak(Box::new(
            JsRegex::new("[#$]\\{(?:(\\d+)(?::([^{}]*))?|((?:\\\\[{}]|[^{}])*))\\}", "").expect("placeholder regexp"),
        ));
    }
    RE.with(|r| *r)
}

impl Snippet {
    pub fn parse(template: &str) -> Snippet {
        let mut fields: Vec<Field> = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        let mut positions: Vec<FieldPos> = Vec::new();
        let normalized = template.replace("\r\n", "\n").replace('\r', "\n");
        for raw_line in normalized.split('\n') {
            let mut line = raw_line.to_string();
            while let Some(g) = placeholder_re().exec_at(&line, 0) {
                let Some((m_start, m_end)) = g[0] else { break };
                let span = |i: usize| {
                    g.get(i)
                        .copied()
                        .flatten()
                        .map(|(s, e)| line[s..e].to_string())
                };
                let digits = span(1);
                let m2 = span(2);
                let m3 = span(3);
                let mut seq = digits
                    .as_deref()
                    .map(|d| d.parse::<u64>().unwrap_or(u64::MAX));
                let raw_name = m2
                    .clone()
                    .filter(|s| !s.is_empty())
                    .or_else(|| m3.clone().filter(|s| !s.is_empty()))
                    .unwrap_or_default();
                if seq == Some(0) {
                    seq = Some(1_000_000_000);
                }
                let name = unescape_braces(&raw_name);
                let mut found: Option<usize> = None;
                for (i, f) in fields.iter().enumerate() {
                    let hit = match seq {
                        Some(s) => f.seq == Some(s),
                        None => !name.is_empty() && f.name == name,
                    };
                    if hit {
                        found = Some(i);
                    }
                }
                let found = match found {
                    Some(i) => i,
                    None => {
                        let mut i = 0;
                        while i < fields.len()
                            && (seq.is_none() || fields[i].seq.is_some_and(|s| Some(s) < seq))
                        {
                            i += 1;
                        }
                        fields.insert(
                            i,
                            Field {
                                seq,
                                name: name.clone(),
                            },
                        );
                        for p in &mut positions {
                            if p.field >= i {
                                p.field += 1;
                            }
                        }
                        i
                    }
                };
                for p in &mut positions {
                    if p.line == lines.len() && p.from > m_start {
                        let snip = if m2.as_deref().is_some_and(|s| !s.is_empty()) {
                            3 + digits.as_deref().map_or(0, str::len)
                        } else {
                            2
                        };
                        p.from -= snip;
                        p.to -= snip;
                    }
                }
                positions.push(FieldPos {
                    field: found,
                    line: lines.len(),
                    from: m_start,
                    to: m_start + name.len(),
                });
                line = format!("{}{}{}", &line[..m_start], raw_name, &line[m_end..]);
            }
            let mut out = String::with_capacity(line.len());
            let bytes = line.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'\\' && matches!(bytes.get(i + 1), Some(b'{') | Some(b'}')) {
                    let index = i;
                    for p in &mut positions {
                        if p.line == lines.len() && p.from > index {
                            p.from -= 1;
                            p.to -= 1;
                        }
                    }
                    out.push(bytes[i + 1] as char);
                    i += 2;
                    continue;
                }
                let ch = line[i..].chars().next().unwrap_or('\0');
                out.push(ch);
                i += ch.len_utf8();
            }
            lines.push(out);
        }
        Snippet {
            lines,
            field_positions: positions,
        }
    }

    /// The text to insert at `pos` (continuation lines get the start line's
    /// indentation plus one indent unit per leading tab) and the fields.
    pub fn instantiate(
        &self,
        state: &EditorState,
        mut pos: usize,
    ) -> (Vec<String>, Vec<FieldRange>) {
        let mut text: Vec<String> = Vec::new();
        let mut line_start = vec![pos];
        let line_obj = state.doc.line_at(pos);
        let line_text = state.doc.slice(line_obj.from, line_obj.to);
        let base_indent: String = line_text.chars().take_while(|c| is_js_space(*c)).collect();
        for line in &self.lines {
            let mut line = line.clone();
            if !text.is_empty() {
                let mut indent = base_indent.clone();
                let tabs = line.bytes().take_while(|b| *b == b'\t').count();
                for _ in 0..tabs {
                    indent.push_str(state.indent_unit());
                }
                line_start.push(pos + indent.len() - tabs);
                line = format!("{indent}{}", &line[tabs..]);
            }
            pos += line.len() + 1;
            text.push(line);
        }
        let ranges = self
            .field_positions
            .iter()
            .map(|p| FieldRange {
                field: p.field,
                from: line_start[p.line] + p.from,
                to: line_start[p.line] + p.to,
            })
            .collect();
        (text, ranges)
    }
}

fn unescape_braces(s: &str) -> String {
    s.replace("\\{", "{").replace("\\}", "}")
}

/// Active tab-stop fields (`ActiveSnippet`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveSnippet {
    pub ranges: Vec<FieldRange>,
    pub active: usize,
}

impl ActiveSnippet {
    fn map(&self, changes: &ChangeDesc) -> Option<ActiveSnippet> {
        let ranges = self
            .ranges
            .iter()
            .map(|r| r.map(changes))
            .collect::<Option<Vec<_>>>()?;
        Some(ActiveSnippet {
            ranges,
            active: self.active,
        })
    }

    fn selection_inside_field(&self, sel: &EditorSelection) -> bool {
        sel.ranges.iter().all(|range| {
            self.ranges
                .iter()
                .any(|r| r.field == self.active && r.from <= range.from && r.to >= range.to)
        })
    }

    /// Field decorations: `cm-snippetField` marks, or a zero-width
    /// `cm-snippetFieldPosition` marker for empty fields.
    pub fn decorations(&self) -> Vec<(usize, usize, bool)> {
        let mut out: Vec<(usize, usize, bool)> = self
            .ranges
            .iter()
            .map(|r| (r.from, r.to, r.from == r.to))
            .collect();
        out.sort_by_key(|&(f, t, _)| (f, t));
        out
    }
}

#[derive(Clone, Debug)]
pub enum SnippetEffect {
    SetActive(Option<Rc<ActiveSnippet>>),
    MoveToField(usize),
}

impl SnippetEffect {
    pub(crate) fn map(&self, changes: &ChangeDesc) -> Option<SnippetEffect> {
        Some(match self {
            SnippetEffect::SetActive(Some(a)) => {
                SnippetEffect::SetActive(a.map(changes).map(Rc::new))
            }
            other => other.clone(),
        })
    }
}

/// The `snippetState` field.
pub(crate) fn update_snippet_state(
    prev: Option<&Rc<ActiveSnippet>>,
    tr: &Transaction,
    _next: &EditorState,
) -> Option<Rc<ActiveSnippet>> {
    let mut value = prev.cloned();
    for e in &tr.effects {
        match e {
            Effect::Snippet(SnippetEffect::SetActive(v)) => return v.clone(),
            Effect::Snippet(SnippetEffect::MoveToField(f)) if value.is_some() => {
                let ranges = value.as_ref().map(|v| v.ranges.clone()).unwrap_or_default();
                return Some(Rc::new(ActiveSnippet { ranges, active: *f }));
            }
            _ => {}
        }
    }
    if let Some(v) = &value
        && tr.doc_changed()
    {
        value = v.map(&tr.changes.desc()).map(Rc::new);
    }
    if let (Some(v), Some(sel)) = (&value, &tr.selection)
        && !v.selection_inside_field(sel)
    {
        value = None;
    }
    value
}

fn field_selection(ranges: &[FieldRange], field: usize) -> EditorSelection {
    EditorSelection::create(
        ranges
            .iter()
            .filter(|r| r.field == field)
            .map(|r| SelectionRange::range(r.from, r.to))
            .collect(),
        0,
    )
}

/// Insert `template` over `from..to` (`snippet(template)(view, completion,
/// from, to)`). `completion` marks the insert as `input.complete`.
pub fn apply_template(
    d: &mut dyn Dispatch,
    template: &str,
    from: usize,
    to: usize,
    completion: bool,
) {
    let snippet = Snippet::parse(template);
    let state = d.state();
    let (text, ranges) = snippet.instantiate(state, from);
    let main = state.selection.main();
    let mut spec = TransactionSpec::default()
        .changes(Change::new(
            from,
            if to == main.from { main.to } else { to },
            text.join("\n"),
        ))
        .scroll();
    if completion {
        spec = spec.user_event("input.complete");
    }
    if !ranges.is_empty() {
        spec = spec.selection(field_selection(&ranges, 0));
    }
    if ranges.iter().any(|r| r.field > 0) {
        spec = spec.effect(Effect::Snippet(SnippetEffect::SetActive(Some(Rc::new(
            ActiveSnippet { ranges, active: 0 },
        )))));
    }
    d.dispatch(spec);
}

fn move_field(state: &EditorState, dir: i32) -> Option<TransactionSpec> {
    let active = state.snippet.as_ref()?;
    if dir < 0 && active.active == 0 {
        return None;
    }
    let next = (active.active as i64 + i64::from(dir)) as usize;
    let last = dir > 0
        && !active
            .ranges
            .iter()
            .any(|r| r.field as i64 == next as i64 + i64::from(dir));
    let value = (!last).then(|| {
        Rc::new(ActiveSnippet {
            ranges: active.ranges.clone(),
            active: next,
        })
    });
    Some(
        TransactionSpec::default()
            .selection(field_selection(&active.ranges, next))
            .effect(Effect::Snippet(SnippetEffect::SetActive(value)))
            .scroll(),
    )
}

/// `nextSnippetField` (Tab).
pub fn next_snippet_field(state: &EditorState) -> Option<TransactionSpec> {
    move_field(state, 1)
}

/// `prevSnippetField` (Shift-Tab).
pub fn prev_snippet_field(state: &EditorState) -> Option<TransactionSpec> {
    move_field(state, -1)
}

/// `clearSnippet` (Escape).
pub fn clear_snippet(state: &EditorState) -> Option<TransactionSpec> {
    state.snippet.as_ref()?;
    Some(TransactionSpec::default().effect(Effect::Snippet(SnippetEffect::SetActive(None))))
}

pub fn has_next_snippet_field(state: &EditorState) -> bool {
    state
        .snippet
        .as_ref()
        .is_some_and(|a| a.ranges.iter().any(|r| r.field == a.active + 1))
}

pub fn has_prev_snippet_field(state: &EditorState) -> bool {
    state.snippet.as_ref().is_some_and(|a| a.active > 0)
}

/// `snippetPointerHandler`: clicking into another field moves to it.
pub fn snippet_mousedown(state: &EditorState, pos: usize) -> Option<TransactionSpec> {
    let active = state.snippet.as_ref()?;
    let m = active
        .ranges
        .iter()
        .find(|r| r.from <= pos && r.to >= pos)?;
    if m.field == active.active {
        return None;
    }
    let value = active.ranges.iter().any(|r| r.field > m.field).then(|| {
        Rc::new(ActiveSnippet {
            ranges: active.ranges.clone(),
            active: m.field,
        })
    });
    Some(
        TransactionSpec::default()
            .selection(field_selection(&active.ranges, m.field))
            .effect(Effect::Snippet(SnippetEffect::SetActive(value)))
            .scroll(),
    )
}

/// A bundle `[[snippet]]` as the editor uses it (`RegisteredSnippet`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegisteredSnippet {
    pub id: String,
    pub label: String,
    pub tab_trigger: String,
    pub scope: Option<String>,
    pub file_glob: Option<String>,
    pub body: String,
    pub order: i64,
    pub bundle_name: Option<String>,
    pub key_equivalent_raw: Option<String>,
    pub key_equivalent: Option<String>,
    pub key_equivalent_display: Option<String>,
}

/// The catalog fields the app's bundle service reports (`SnippetInfo`).
#[derive(Clone, Debug, Default)]
pub struct SnippetInfo {
    pub id: String,
    pub name: String,
    pub tab_trigger: String,
    pub scope: String,
    pub file_glob: String,
    pub body: String,
    pub order: i64,
    pub bundle_name: String,
    pub key_equivalent: String,
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

impl RegisteredSnippet {
    /// `fromInfo`.
    pub fn from_info(s: &SnippetInfo) -> RegisteredSnippet {
        let key_equivalent = effective_key_equivalent(Some(&s.key_equivalent));
        RegisteredSnippet {
            id: s.id.clone(),
            label: s.name.clone(),
            tab_trigger: s.tab_trigger.clone(),
            scope: non_empty(&s.scope),
            file_glob: non_empty(&s.file_glob),
            body: s.body.clone(),
            order: s.order,
            bundle_name: non_empty(s.bundle_name.trim()),
            key_equivalent_raw: non_empty(&s.key_equivalent),
            key_equivalent_display: (!key_equivalent.is_empty())
                .then(|| format_key_equivalent_display(&key_equivalent)),
            key_equivalent: non_empty(&key_equivalent),
        }
    }
}

/// `sortSnippets`: by order, bundle name, then label.
pub fn sort_snippets(list: &mut [RegisteredSnippet]) {
    list.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| {
                locale_compare(
                    a.bundle_name.as_deref().unwrap_or(""),
                    b.bundle_name.as_deref().unwrap_or(""),
                )
            })
            .then_with(|| locale_compare(&a.label, &b.label))
    });
}

/// Where the editor gets bundle snippets. The app keeps the catalog (its
/// 2s cache, scope/glob matching and disabled-snippet settings); the editor
/// only asks for the enabled snippets matching a file.
pub trait SnippetProvider {
    /// Matching enabled snippets, loading on a cache miss.
    fn load(&self, scope: &str, path: &str) -> LocalBoxFuture<'static, Vec<RegisteredSnippet>>;
    /// Matching enabled snippets from the warm cache only (empty on a miss),
    /// for synchronous Tab expansion.
    fn cached(&self, scope: &str, path: &str) -> Vec<RegisteredSnippet>;
}

/// Tab-trigger glyph for menus (`ife⇥`), empty for chord-only snippets.
pub fn snippet_trigger_hint(tab_trigger: &str) -> String {
    let t = tab_trigger.trim();
    if t.is_empty() {
        String::new()
    } else {
        format!("{t}⇥")
    }
}

/// The range a snippet command replaces: the selection, else the word,
/// line or document per the command's input mode, else the caret.
pub fn command_snippet_range(
    sel_from: usize,
    sel_to: usize,
    input_mode: Option<&str>,
    word: Option<(usize, usize)>,
    line: Option<(usize, usize)>,
    doc_len: usize,
) -> (usize, usize) {
    if sel_from != sel_to {
        return (sel_from, sel_to);
    }
    match (input_mode, word, line) {
        (Some("word"), Some(w), _) => w,
        (Some("line"), _, Some(l)) => l,
        (Some("document"), _, _) => (0, doc_len),
        _ => (sel_from, sel_to),
    }
}

/// The tab-trigger word ending at `pos` in `text` (a letter or `_`, then
/// word characters).
pub fn trigger_word_before(text: &str, pos: usize) -> Option<(usize, usize, String)> {
    let b = text.as_bytes();
    let mut i = pos;
    while i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
        i -= 1;
    }
    if i == pos {
        return None;
    }
    let word = &text[i..pos];
    let first = word.as_bytes()[0];
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return None;
    }
    Some((i, pos, word.to_string()))
}

/// Whether text substituted into a template would be parsed as fields
/// (`$n`, `${…}`, braces), so it has to be shielded behind a token.
pub fn snippet_needs_shield(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().any(|&c| c == b'{' || c == b'}')
        || b.windows(2).any(|w| {
            (w[0] == b'$' || w[0] == b'#') && w[1] == b'{' || w[0] == b'$' && w[1].is_ascii_digit()
        })
}

pub fn iso_date(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("%Y-%m-%d").to_string()
}

fn var_res() -> &'static [JsRegex; 4] {
    thread_local! {
        static RES: &'static [JsRegex; 4] = Box::leak(Box::new([
            JsRegex::new("\\$\\{TM_SELECTED_TEXT\\}", "g").expect("regexp"),
            JsRegex::new("\\$TM_SELECTED_TEXT\\b", "g").expect("regexp"),
            JsRegex::new("\\$\\{ISO_DATE\\}", "g").expect("regexp"),
            JsRegex::new("\\$ISO_DATE\\b", "g").expect("regexp"),
        ]));
    }
    RES.with(|r| *r)
}

/// `$TM_SELECTED_TEXT` / `$ISO_DATE` substitution. The replacement goes
/// through `String.prototype.replace` `$` patterns, as in Recur.
pub fn substitute_snippet_vars(
    body: &str,
    selected: &str,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let date = iso_date(now);
    let [sel_braced, sel_bare, date_braced, date_bare] = var_res();
    let out = sel_braced.replace(body, selected);
    let out = sel_bare.replace(&out, selected);
    let out = date_braced.replace(&out, &date);
    date_bare.replace(&out, &date)
}

/// VS Code `$n` / empty `${n}` to CodeMirror `${n}`, copying the first
/// `${n:default}` onto mirrors (`toCodeMirrorSnippet`).
pub fn to_codemirror_snippet(body: &str) -> String {
    thread_local! {
        static RES: &'static (JsRegex, JsRegex) = Box::leak(Box::new((
            JsRegex::new("\\$\\{(\\d+):([^{}]*)\\}", "g").expect("regexp"),
            JsRegex::new("(?<!\\\\)\\$\\{(\\d+)\\}|(?<!\\\\)\\$(?!\\{)(\\d+)", "g").expect("regexp"),
        )));
    }
    let (with_default, mirror) = RES.with(|r| *r);
    let mut defaults: Vec<(u64, String)> = Vec::new();
    let mut at = 0;
    while let Some(g) = with_default.exec_at(body, at) {
        let Some((s, e)) = g[0] else { break };
        let n: u64 = g[1].map_or(0, |(a, b)| body[a..b].parse().unwrap_or(0));
        if !defaults.iter().any(|(k, _)| *k == n) {
            defaults.push((
                n,
                g[2].map_or(String::new(), |(a, b)| body[a..b].to_string()),
            ));
        }
        at = if e > s { e } else { e + 1 };
        if at > body.len() {
            break;
        }
    }
    let mut out = String::with_capacity(body.len());
    let mut last = 0;
    let mut at = 0;
    while at <= body.len() {
        let Some(g) = mirror.exec_at(body, at) else {
            break;
        };
        let Some((s, e)) = g[0] else { break };
        let digits = g[1].or(g[2]).map_or("", |(a, b)| &body[a..b]);
        let n: u64 = digits.parse().unwrap_or(0);
        out.push_str(&body[last..s]);
        match defaults.iter().find(|(k, _)| *k == n) {
            Some((_, d)) => out.push_str(&format!("${{{n}:{d}}}")),
            None => out.push_str(&format!("${{{n}}}")),
        }
        last = e;
        at = if e > s { e } else { e + 1 };
    }
    out.push_str(&body[last..]);
    out
}

thread_local! {
    static TOKEN_SEQ: Cell<u64> = const { Cell::new(0) };
}

/// A unique `RSEL…` shield token (8 base-36 digits).
fn shield_token() -> String {
    let seq = TOKEN_SEQ.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    let mut x = (crate::clock::now_ms() ^ seq.rotate_left(32)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut digits = String::new();
    for _ in 0..8 {
        let d = (x % 36) as u32;
        digits.push(char::from_digit(d, 36).unwrap_or('0'));
        x /= 36;
    }
    format!("RSEL{digits}")
}

/// `applySnippet`: expand a bundle snippet over `from..to`, substituting
/// the selected text literally.
pub fn apply_snippet(
    d: &mut dyn Dispatch,
    sn: &RegisteredSnippet,
    from: usize,
    to: usize,
    selected: Option<&str>,
) -> bool {
    let sel = selected.map_or_else(|| d.state().slice_doc(from, to), str::to_string);
    let shield = snippet_needs_shield(&sel);
    let token = if shield { shield_token() } else { sel.clone() };
    let body = to_codemirror_snippet(&substitute_snippet_vars(
        &sn.body,
        &token,
        chrono::Local::now(),
    ));
    apply_template(d, &body, from, to, false);
    if shield {
        replace_snippet_token(d, &token, &sel);
    }
    true
}

fn replace_snippet_token(d: &mut dyn Dispatch, token: &str, text: &str) {
    let doc = d.state().doc.to_string();
    let changes: Vec<Change> = doc
        .match_indices(token)
        .map(|(i, _)| Change::new(i, i + token.len(), text))
        .collect();
    if !changes.is_empty() {
        d.dispatch(TransactionSpec::default().changes(changes));
    }
}

/// `tryExpandSnippet`: expand the tab trigger before the caret. One match
/// expands; several go to `on_pick_many` with the trigger range. Returns
/// whether Tab was consumed.
pub fn try_expand_snippet(
    d: &mut dyn Dispatch,
    snippets: &[RegisteredSnippet],
    on_pick_many: impl FnOnce(Vec<RegisteredSnippet>, usize, usize),
) -> bool {
    let state = d.state();
    let sel = state.selection.main();
    if sel.from != sel.to {
        return false;
    }
    let line = state.doc.line_at(sel.head());
    let text = state.doc.slice(line.from, line.to);
    let Some((f, t, word)) = trigger_word_before(&text, sel.head() - line.from) else {
        return false;
    };
    let (from, to) = (line.from + f, line.from + t);
    let matches: Vec<RegisteredSnippet> = snippets
        .iter()
        .filter(|s| s.tab_trigger == word)
        .cloned()
        .collect();
    match matches.len() {
        0 => false,
        1 => apply_snippet(d, &matches[0], from, to, Some("")),
        _ => {
            on_pick_many(matches, from, to);
            true
        }
    }
}

/// Tab-trigger snippets whose trigger starts with `prefix`
/// (case-insensitive); all of them for an explicit empty prefix.
pub fn snippets_matching_prefix(
    snippets: &[RegisteredSnippet],
    prefix: &str,
    explicit: bool,
) -> Vec<RegisteredSnippet> {
    let with_trigger = snippets.iter().filter(|s| !s.tab_trigger.is_empty());
    if prefix.is_empty() {
        return if explicit {
            with_trigger.cloned().collect()
        } else {
            Vec::new()
        };
    }
    let lower = prefix.to_lowercase();
    with_trigger
        .filter(|s| s.tab_trigger.to_lowercase().starts_with(&lower))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::state::StateConfig;

    fn st(doc: &str) -> EditorState {
        EditorState::create(
            doc,
            StateConfig {
                allow_multiple_selections: true,
                indent_unit: "\t".into(),
                ..StateConfig::default()
            },
        )
    }

    fn expand(body: &str, selected: &str) -> (String, Vec<String>) {
        let date = chrono::Local
            .with_ymd_and_hms(2026, 8, 17, 9, 0, 0)
            .unwrap();
        let prepared = to_codemirror_snippet(&substitute_snippet_vars(body, selected, date));
        let mut s = st("X");
        apply_template(&mut s, &prepared, 0, 1, false);
        let sels = s
            .selection
            .ranges
            .iter()
            .map(|r| s.slice_doc(r.from, r.to))
            .collect();
        (s.doc.to_string(), sels)
    }

    fn sn(id: &str, trigger: &str, order: i64) -> RegisteredSnippet {
        RegisteredSnippet {
            id: id.into(),
            label: id.into(),
            tab_trigger: trigger.into(),
            body: "x".into(),
            order,
            ..Default::default()
        }
    }

    #[test]
    fn prefix_matching() {
        let list = [sn("ife", "ife", 10), sn("if", "if", 11), sn("wrap", "", 1)];
        let ids = |v: Vec<RegisteredSnippet>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(
            ids(snippets_matching_prefix(&list, "if", false)),
            vec!["ife", "if"]
        );
        assert_eq!(
            ids(snippets_matching_prefix(&list, "IFE", false)),
            vec!["ife"]
        );
        assert!(snippets_matching_prefix(&list, "", false).is_empty());
        assert_eq!(
            ids(snippets_matching_prefix(&list, "", true)),
            vec!["ife", "if"]
        );
        assert_eq!(snippet_trigger_hint("ife"), "ife⇥");
        assert_eq!(snippet_trigger_hint(""), "");
    }

    #[test]
    fn trigger_words() {
        assert_eq!(trigger_word_before("ife", 3), Some((0, 3, "ife".into())));
        assert_eq!(
            trigger_word_before("  fori", 6),
            Some((2, 6, "fori".into()))
        );
        assert_eq!(trigger_word_before("x ife", 5), Some((2, 5, "ife".into())));
        assert_eq!(trigger_word_before("  ", 2), None);
        assert_eq!(trigger_word_before("1ife", 4), None);
    }

    #[test]
    fn variables_and_rewrites() {
        let date = chrono::Local
            .with_ymd_and_hms(2026, 8, 17, 9, 0, 0)
            .unwrap();
        let body = "<${1:p}>${2:$TM_SELECTED_TEXT}</$1>";
        assert_eq!(
            substitute_snippet_vars(body, "hi $x", date),
            "<${1:p}>${2:hi $x}</$1>"
        );
        assert_eq!(
            substitute_snippet_vars(body, "C:\\path", date),
            "<${1:p}>${2:C:\\path}</$1>"
        );
        assert_eq!(substitute_snippet_vars("$ISO_DATE", "", date), "2026-08-17");
        assert_eq!(
            to_codemirror_snippet("console.log(${1})$0"),
            "console.log(${1})${0}"
        );
        assert_eq!(
            to_codemirror_snippet("<${1:div}>x</$1>"),
            "<${1:div}>x</${1:div}>"
        );
        assert_eq!(
            to_codemirror_snippet("for ${1:i} := 0; $1 < ${2:n}; $1++"),
            "for ${1:i} := 0; ${1:i} < ${2:n}; ${1:i}++"
        );
        assert_eq!(to_codemirror_snippet("$0t.Fatal"), "${0}t.Fatal");
        assert_eq!(
            command_snippet_range(3, 3, Some("word"), Some((0, 3)), None, 10),
            (0, 3)
        );
        assert_eq!(
            command_snippet_range(1, 4, Some("word"), Some((0, 8)), None, 10),
            (1, 4)
        );
        assert_eq!(
            command_snippet_range(3, 3, Some("word"), None, None, 10),
            (3, 3)
        );
        assert!(!snippet_needs_shield("$HOME"));
        assert!(!snippet_needs_shield("C:\\path"));
        assert!(snippet_needs_shield("$1"));
        assert!(snippet_needs_shield("${count}"));
    }

    #[test]
    fn mirrors_and_defaults() {
        let (doc, sels) = expand("if ${1:err} != nil {\n\t${2:log.Fatal(${1:err})}\n}", "");
        assert_eq!(sels, vec!["err", "err"]);
        assert_eq!(doc, "if err != nil {\n\tlog.Fatal(err)\n}");
        let (doc, sels) = expand(
            "func ${1:name}(${2:params}) ${3:type} {\n\t${0:// body}\n}",
            "",
        );
        assert_eq!(sels[0], "name");
        assert!(doc.contains("func name(params) type {") && doc.contains("// body"));
        assert_eq!(
            expand("fmt.Println(${1:\"${2:Hello World!}\"})", "").0,
            "fmt.Println(\"Hello World!\")"
        );
        assert_eq!(
            expand("import ${2:package} \"${1:package}\"", "").0,
            "import package \"package\""
        );
        assert_eq!(
            expand("console.log(${1:$TM_SELECTED_TEXT});", "hi").0,
            "console.log(hi);"
        );
        assert_eq!(
            expand("<${1:p}>${2:$TM_SELECTED_TEXT}</$1>", "hi").0,
            "<p>hi</p>"
        );
        assert_eq!(
            expand("<${1:p}>${2:$TM_SELECTED_TEXT}</$1>", "$HOME").0,
            "<p>$HOME</p>"
        );
        assert!(
            expand(
                "def ${1:fname}(${2}):\n\t\"\"\"${3:docstring for ${1:fname}}\"\"\"\n\t${0:pass}",
                ""
            )
            .0
            .contains("\"\"\"docstring for fname\"\"\"")
        );
        let tmp = expand(
            "${1:TMPFILE}=\"$(mktemp -t ${2:untitled})\"\ntrap '${3:rm -f \"$TMPFILE\"}' EXIT INT HUP TERM",
            "",
        )
        .0;
        assert!(tmp.contains("TMPFILE=\"$(mktemp -t untitled)\""));
        assert!(tmp.contains("trap 'rm -f \"$TMPFILE\"' EXIT INT HUP TERM"));
        assert!(
            expand(
                "<div${1: id=\"${2:name}\"}>\n\t${0:$TM_SELECTED_TEXT}\n</div>",
                ""
            )
            .0
            .contains("<div id=\"name\">")
        );
    }

    #[test]
    fn shielded_selection_is_literal() {
        let wrap = RegisteredSnippet {
            body: "<${1:p}>${2:$TM_SELECTED_TEXT}</$1>".into(),
            ..Default::default()
        };
        let mut s = st("X");
        apply_snippet(&mut s, &wrap, 0, 1, Some("${count}"));
        assert_eq!(s.doc.to_string(), "<p>${count}</p>");
        let quote = RegisteredSnippet {
            body: "\"${1:$TM_SELECTED_TEXT}\"".into(),
            ..Default::default()
        };
        let mut s = st("X");
        apply_snippet(&mut s, &quote, 0, 1, Some("$1"));
        assert_eq!(s.doc.to_string(), "\"$1\"");
    }

    #[test]
    fn tab_moves_between_fields_and_indentation_follows_the_line() {
        let mut s = st("  x");
        s.dispatch(TransactionSpec::default().cursor(3));
        apply_template(&mut s, "a(${1:one}, ${2:two})\n\t${0}", 3, 3, false);
        assert_eq!(s.doc.to_string(), "  xa(one, two)\n  \t");
        assert_eq!(
            s.slice_doc(s.selection.main().from, s.selection.main().to),
            "one"
        );
        let spec = next_snippet_field(&s).unwrap();
        s.dispatch(spec);
        assert_eq!(
            s.slice_doc(s.selection.main().from, s.selection.main().to),
            "two"
        );
        assert!(has_prev_snippet_field(&s));
        let spec = next_snippet_field(&s).unwrap();
        s.dispatch(spec);
        assert!(s.snippet.is_none());
        assert_eq!(s.selection.main().head(), s.doc.len());
    }

    #[test]
    fn sorting_and_tab_expansion() {
        let mut list = vec![sn("b", "b", 2), sn("a2", "a", 1), sn("a1", "a", 1)];
        sort_snippets(&mut list);
        assert_eq!(
            list.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["a1", "a2", "b"]
        );
        let mut s = st("ife");
        s.dispatch(TransactionSpec::default().cursor(3));
        let ife = RegisteredSnippet {
            tab_trigger: "ife".into(),
            body: "if err != nil {\n\t$0\n}".into(),
            ..Default::default()
        };
        assert!(try_expand_snippet(&mut s, &[ife], |_, _, _| panic!()));
        assert_eq!(s.doc.to_string(), "if err != nil {\n\t\n}");
    }
}
