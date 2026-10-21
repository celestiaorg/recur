//! Search: `@codemirror/search` (cursors, queries, find/replace commands,
//! selection-match highlighting, `selectNextOccurrence`) with Recur's
//! find panel (`cm-search-panel.ts`) and go-to-line dialog
//! (`cm-goto-line.ts`).
//!
//! Offsets are UTF-8 bytes where CodeMirror counts UTF-16 units; the chunk
//! sizes and lookback margins below are applied to bytes.

use std::rc::Rc;

use recur_textmate::JsRegex;
use unicode_normalization::UnicodeNormalization;

use crate::change::Change;
use crate::selection::{EditorSelection, SelectionRange};
use crate::state::{
    CharCategory, EditorState, Effect, ScrollStrategy, ScrollTarget, Transaction, TransactionSpec,
};
use crate::text::{Text, find_cluster_break};

/// A match: its range, whether it is exact (normalization can widen it),
/// and the regexp groups (`None` for plain-string queries).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub from: usize,
    pub to: usize,
    pub precise: bool,
    pub groups: Option<Vec<Option<String>>>,
}

fn normalize(s: &str, fold_case: bool) -> Vec<char> {
    let n: String = s.nfkd().collect();
    if fold_case {
        n.to_lowercase().chars().collect()
    } else {
        n.chars().collect()
    }
}

/// `charBefore` / `charAfter`: the grapheme cluster on either side of `i`.
fn char_before(s: &str, i: usize) -> &str {
    &s[find_cluster_break(s, i, false)..i]
}

fn char_after(s: &str, i: usize) -> &str {
    &s[i..find_cluster_break(s, i, true)]
}

fn is_word(state: &EditorState, s: &str) -> bool {
    !s.is_empty() && state.categorize(s) == CharCategory::Word
}

/// The whole-word test shared by both cursor kinds: each end of the match
/// is a word boundary on at least one side.
fn word_boundaries(state: &EditorState, text: &str, from: usize, to: usize) -> bool {
    (!is_word(state, char_before(text, from)) || !is_word(state, char_after(text, from)))
        && (!is_word(state, char_after(text, to)) || !is_word(state, char_before(text, to)))
}

struct Partial {
    from: usize,
    index: usize,
    precise: bool,
}

/// `test(from, to, buffer, buffer_pos)` for whole-word string matches.
type StringTest<'a> = &'a dyn Fn(usize, usize, &str, usize) -> bool;

/// Plain-string cursor (`SearchCursor`): matches after NFKD normalization,
/// optionally case-folded.
pub struct SearchCursor<'a> {
    doc: &'a Text,
    chars: Box<dyn Iterator<Item = (usize, char)> + 'a>,
    range: (usize, usize),
    query: Vec<char>,
    fold_case: bool,
    matches: Vec<Partial>,
    test: Option<StringTest<'a>>,
}

impl<'a> SearchCursor<'a> {
    pub fn new(
        doc: &'a Text,
        query: &str,
        from: usize,
        to: usize,
        fold_case: bool,
    ) -> SearchCursor<'a> {
        let to = to.min(doc.len());
        SearchCursor {
            doc,
            chars: Box::new(doc.chars_from(from).take_while(move |(p, _)| *p < to)),
            range: (from, to),
            query: normalize(query, fold_case),
            fold_case,
            matches: Vec::new(),
            test: None,
        }
    }

    fn with_test(mut self, test: StringTest<'a>) -> Self {
        self.test = Some(test);
        self
    }

    pub fn next_match(&mut self) -> Option<SearchMatch> {
        self.matches.clear();
        self.next_overlapping()
    }

    /// Like `next_match`, but includes matches partially overlapping the
    /// previous one.
    pub fn next_overlapping(&mut self) -> Option<SearchMatch> {
        if self.query.is_empty() {
            return None;
        }
        loop {
            let (start, ch) = self.chars.next()?;
            let end = start + ch.len_utf8();
            let mut buf = [0u8; 4];
            let norm = normalize(ch.encode_utf8(&mut buf), self.fold_case);
            let mut pos = start;
            let mut pos_precise = true;
            for (i, &code) in norm.iter().enumerate() {
                let last = i == norm.len() - 1;
                if let Some(m) = self.match_char(code, pos, pos_precise, end, last, ch) {
                    return Some(m);
                }
                if last {
                    break;
                }
                if pos_precise && i == 0 && ch == code {
                    pos += ch.len_utf8();
                } else {
                    pos_precise = false;
                }
            }
        }
    }

    fn match_char(
        &mut self,
        code: char,
        pos: usize,
        pos_precise: bool,
        end: usize,
        end_precise: bool,
        ch: char,
    ) -> Option<SearchMatch> {
        let mut found = None;
        let last = self.query.len() - 1;
        let query = &self.query;
        self.matches.retain_mut(|p| {
            if query[p.index] != code {
                return false;
            }
            if p.index == last {
                found = Some((p.from, end, end_precise && p.precise));
                return false;
            }
            p.index += 1;
            true
        });
        if self.query[0] == code {
            if self.query.len() == 1 {
                found = Some((pos, end, pos_precise && end_precise));
            } else {
                self.matches.push(Partial {
                    from: pos,
                    index: 1,
                    precise: pos_precise,
                });
            }
        }
        let (from, to, precise) = found?;
        if let Some(test) = self.test {
            let (buffer, buffer_pos) = self.buffer(end, ch);
            if !test(from, to, &buffer, buffer_pos) {
                return None;
            }
        }
        Some(SearchMatch {
            from,
            to,
            precise,
            groups: None,
        })
    }

    /// The chunk CodeMirror's range iterator was on when `ch` (ending at
    /// `end`) was read: the line break, or the line clipped to the range.
    fn buffer(&self, end: usize, ch: char) -> (String, usize) {
        if ch == '\n' {
            return ("\n".into(), end - 1);
        }
        let line = self.doc.line_at(end - 1);
        let from = line.from.max(self.range.0);
        let to = line.to.min(self.range.1);
        (self.doc.slice(from, to), from)
    }
}

/// The empty-match bookkeeping of `RegExpCursor` (`from > value.to`).
fn to_char_end(doc: &Text, pos: usize) -> usize {
    if pos >= doc.len() {
        return pos;
    }
    let mut p = pos;
    while doc.clamp_to_char(p) != p {
        p += 1;
    }
    p
}

/// Byte offset one character past `i` in `s`.
fn next_char(s: &str, i: usize) -> usize {
    i + s[i..].chars().next().map_or(1, char::len_utf8)
}

type RegexTest<'a> = &'a dyn Fn(usize, usize, &str, usize, usize) -> bool;

fn groups_of(text: &str, spans: &[Option<(usize, usize)>]) -> Vec<Option<String>> {
    spans
        .iter()
        .map(|g| g.map(|(s, e)| text[s..e].to_string()))
        .collect()
}

fn exec(re: &JsRegex, text: &str, at: usize) -> Option<Vec<Option<(usize, usize)>>> {
    if at > text.len() {
        return None;
    }
    re.exec_at(text, at).filter(|g| g[0].is_some())
}

/// Regexp cursor (`RegExpCursor`): line by line, unless the pattern can
/// match across lines (`MultilineRegExpCursor` over a growing flattened
/// slice).
pub struct RegExpCursor<'a> {
    doc: &'a Text,
    re: Option<Rc<JsRegex>>,
    to: usize,
    multiline: bool,
    match_pos: usize,
    last_to: Option<usize>,
    cur_line: String,
    cur_line_start: usize,
    flat_from: usize,
    flat: String,
    test: Option<RegexTest<'a>>,
}

const MULTILINE_CHUNK: usize = 5000;

fn needs_multiline(query: &str) -> bool {
    query.contains('\n')
        || query.contains('\r')
        || query.contains("[^")
        || ["\\s", "\\W", "\\D", "\\n", "\\r"]
            .iter()
            .any(|p| query.contains(p))
}

impl<'a> RegExpCursor<'a> {
    pub fn new(
        doc: &'a Text,
        query: &str,
        ignore_case: bool,
        from: usize,
        to: usize,
    ) -> RegExpCursor<'a> {
        let flags = if ignore_case { "gmui" } else { "gmu" };
        let to = to.min(doc.len());
        let mut cursor = RegExpCursor {
            doc,
            re: JsRegex::new(query, flags).map(Rc::new),
            to,
            multiline: needs_multiline(query),
            match_pos: to_char_end(doc, from),
            last_to: None,
            cur_line: String::new(),
            cur_line_start: 0,
            flat_from: from,
            flat: String::new(),
            test: None,
        };
        if cursor.multiline {
            cursor.flat = doc.slice(from, cursor.chunk_end(from + MULTILINE_CHUNK));
        } else {
            cursor.cur_line_start = doc.line_at(from).from;
            cursor.get_line();
        }
        cursor
    }

    fn with_test(mut self, test: RegexTest<'a>) -> Self {
        self.test = Some(test);
        self
    }

    fn get_line(&mut self) {
        let line = self.doc.line_at(self.cur_line_start);
        let end = line.to.min(self.to.max(self.cur_line_start));
        self.cur_line = self.doc.slice(self.cur_line_start, end);
    }

    fn next_line(&mut self) {
        self.cur_line_start += self.cur_line.len() + 1;
        if self.cur_line_start > self.to {
            self.cur_line.clear();
        } else {
            self.get_line();
        }
    }

    fn chunk_end(&self, pos: usize) -> usize {
        if pos >= self.to {
            self.to
        } else {
            self.doc.line_at(pos).to
        }
    }

    pub fn next_match(&mut self) -> Option<SearchMatch> {
        self.re.as_ref()?;
        if self.multiline {
            self.next_multiline()
        } else {
            self.next_in_lines()
        }
    }

    fn accept(&self, from: usize, to: usize, input: &str, index: usize) -> bool {
        self.test
            .is_none_or(|t| t(from, to, input, index, index + (to - from)))
    }

    fn next_in_lines(&mut self) -> Option<SearchMatch> {
        let re = self.re.clone()?;
        let mut off = self.match_pos.saturating_sub(self.cur_line_start);
        loop {
            let found = if self.match_pos <= self.to {
                exec(&re, &self.cur_line, off)
            } else {
                None
            };
            if let Some(spans) = found {
                let (ms, me) = spans[0].unwrap_or_default();
                let from = self.cur_line_start + ms;
                let to = self.cur_line_start + me;
                self.match_pos = to_char_end(self.doc, to + usize::from(from == to));
                let input = self.cur_line.clone();
                if from == self.cur_line_start + self.cur_line.len() {
                    self.next_line();
                }
                if (from < to || self.last_to.is_none_or(|t| from > t))
                    && self.accept(from, to, &input, ms)
                {
                    self.last_to = Some(to);
                    return Some(SearchMatch {
                        from,
                        to,
                        precise: true,
                        groups: Some(groups_of(&input, &spans)),
                    });
                }
                off = self.match_pos.saturating_sub(self.cur_line_start);
            } else if self.cur_line_start + self.cur_line.len() < self.to {
                self.next_line();
                off = 0;
            } else {
                return None;
            }
        }
    }

    fn next_multiline(&mut self) -> Option<SearchMatch> {
        let re = self.re.clone()?;
        loop {
            let off = self.match_pos.saturating_sub(self.flat_from);
            let mut found = exec(&re, &self.flat, off);
            if let Some(spans) = &found
                && let Some((ms, me)) = spans[0]
                && ms == me
                && ms == off
            {
                found = if off < self.flat.len() {
                    exec(&re, &self.flat, next_char(&self.flat, off))
                } else {
                    None
                };
            }
            if let Some(spans) = found {
                let (ms, me) = spans[0].unwrap_or_default();
                let from = self.flat_from + ms;
                let to = self.flat_from + me;
                let flat_to = self.flat_from + self.flat.len();
                if (flat_to >= self.to || me + 10 <= self.flat.len())
                    && self.accept(from, to, &self.flat, ms)
                {
                    self.match_pos = to_char_end(self.doc, to + usize::from(from == to));
                    return Some(SearchMatch {
                        from,
                        to,
                        precise: true,
                        groups: Some(groups_of(&self.flat, &spans)),
                    });
                }
            }
            if self.flat_from + self.flat.len() == self.to {
                return None;
            }
            let end = self.chunk_end(self.flat_from + self.flat.len() * 2);
            self.flat = self.doc.slice(self.flat_from, end);
        }
    }
}

/// `validRegExp`.
pub fn valid_regexp(source: &str) -> bool {
    JsRegex::new(source, "gmu").is_some()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQueryConfig {
    pub search: String,
    pub case_sensitive: bool,
    pub literal: bool,
    pub regexp: bool,
    pub replace: String,
    pub whole_word: bool,
}

/// `SearchQuery`.
#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    pub search: String,
    pub case_sensitive: bool,
    pub literal: bool,
    pub regexp: bool,
    pub replace: String,
    pub valid: bool,
    pub unquoted: String,
    pub whole_word: bool,
}

/// `SearchQuery.eq`: `literal` does not count, like CodeMirror.
impl PartialEq for SearchQuery {
    fn eq(&self, other: &SearchQuery) -> bool {
        self.search == other.search
            && self.replace == other.replace
            && self.case_sensitive == other.case_sensitive
            && self.regexp == other.regexp
            && self.whole_word == other.whole_word
    }
}

const FIND_PREV_CHUNK: usize = 10000;
const HIGHLIGHT_MARGIN: usize = 250;

impl SearchQuery {
    pub fn new(config: SearchQueryConfig) -> SearchQuery {
        let valid = !config.search.is_empty() && (!config.regexp || valid_regexp(&config.search));
        let mut q = SearchQuery {
            valid,
            unquoted: String::new(),
            search: config.search,
            case_sensitive: config.case_sensitive,
            literal: config.literal,
            regexp: config.regexp,
            replace: config.replace,
            whole_word: config.whole_word,
        };
        q.unquoted = q.unquote(&q.search);
        q
    }

    /// `\n`, `\r`, `\t` and `\\` escapes, unless literal.
    pub fn unquote(&self, text: &str) -> String {
        if self.literal {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\'
                && let Some(&n) = chars.peek()
                && matches!(n, 'n' | 'r' | 't' | '\\')
            {
                chars.next();
                out.push(match n {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    _ => '\\',
                });
                continue;
            }
            out.push(c);
        }
        out
    }

    pub fn config(&self) -> SearchQueryConfig {
        SearchQueryConfig {
            search: self.search.clone(),
            case_sensitive: self.case_sensitive,
            literal: self.literal,
            regexp: self.regexp,
            replace: self.replace.clone(),
            whole_word: self.whole_word,
        }
    }

    /// Every match in `from..to` (`getCursor`), in order.
    pub fn for_each_match(
        &self,
        state: &EditorState,
        from: usize,
        to: usize,
        overlapping: bool,
        mut f: impl FnMut(SearchMatch) -> bool,
    ) {
        if self.regexp {
            let test = |_: usize, _: usize, input: &str, s: usize, e: usize| {
                s == e || word_boundaries(state, input, s, e)
            };
            let mut cur =
                RegExpCursor::new(&state.doc, &self.search, !self.case_sensitive, from, to);
            if self.whole_word {
                cur = cur.with_test(&test);
            }
            while let Some(m) = cur.next_match() {
                if !f(m) {
                    return;
                }
            }
        } else {
            let doc = &state.doc;
            let test = |from: usize, to: usize, buf: &str, buf_pos: usize| {
                if buf_pos > from || buf_pos + buf.len() < to {
                    let s = doc.clamp_to_char(from.saturating_sub(2));
                    let e = to_char_end(doc, (to + 2).min(doc.len()));
                    let text = doc.slice(s, e);
                    return word_boundaries(state, &text, from - s, to - s);
                }
                word_boundaries(state, buf, from - buf_pos, to - buf_pos)
            };
            let mut cur = SearchCursor::new(doc, &self.unquoted, from, to, !self.case_sensitive);
            if self.whole_word {
                cur = cur.with_test(&test);
            }
            loop {
                let m = if overlapping {
                    cur.next_overlapping()
                } else {
                    cur.next_match()
                };
                let Some(m) = m else { return };
                if !f(m) {
                    return;
                }
            }
        }
    }

    fn first(&self, state: &EditorState, from: usize, to: usize) -> Option<SearchMatch> {
        let mut found = None;
        self.for_each_match(state, from, to, !self.regexp, |m| {
            found = Some(m);
            false
        });
        found
    }

    fn last(&self, state: &EditorState, from: usize, to: usize) -> Option<SearchMatch> {
        let mut found = None;
        self.for_each_match(state, from, to, !self.regexp, |m| {
            found = Some(m);
            true
        });
        found
    }

    /// The next match after `cur_to`, wrapping to the document start.
    pub fn next_match(
        &self,
        state: &EditorState,
        cur_from: usize,
        cur_to: usize,
    ) -> Option<SearchMatch> {
        let len = state.doc.len();
        if self.regexp {
            return self
                .first(state, cur_to, len)
                .or_else(|| self.first(state, 0, cur_from));
        }
        let found = self
            .first(state, cur_to, len)
            .or_else(|| self.first(state, 0, len.min(cur_from + self.unquoted.len())))?;
        (!(found.from == cur_from && found.to == cur_to)).then_some(found)
    }

    fn prev_match_in_range(
        &self,
        state: &EditorState,
        from: usize,
        to: usize,
    ) -> Option<SearchMatch> {
        if self.regexp {
            let mut size = 1;
            loop {
                let start = from.max(to.saturating_sub(size * FIND_PREV_CHUNK));
                let found = self.last(state, start, to);
                if let Some(r) = &found
                    && (start == from || r.from > start + 10)
                {
                    return found;
                }
                if start == from {
                    return None;
                }
                size += 1;
            }
        }
        let mut pos = to;
        loop {
            let start = from.max(pos.saturating_sub(FIND_PREV_CHUNK + self.unquoted.len()));
            if let Some(r) = self.last(state, start, pos) {
                return Some(r);
            }
            if start == from {
                return None;
            }
            pos = pos.saturating_sub(FIND_PREV_CHUNK);
        }
    }

    /// The previous match before `cur_from`, wrapping to the document end.
    pub fn prev_match(
        &self,
        state: &EditorState,
        cur_from: usize,
        cur_to: usize,
    ) -> Option<SearchMatch> {
        let len = state.doc.len();
        if self.regexp {
            return self
                .prev_match_in_range(state, 0, cur_from)
                .or_else(|| self.prev_match_in_range(state, cur_to, len));
        }
        let found = self.prev_match_in_range(state, 0, cur_from).or_else(|| {
            self.prev_match_in_range(state, cur_to.saturating_sub(self.unquoted.len()), len)
        })?;
        (found.from != cur_from || found.to != cur_to).then_some(found)
    }

    /// The replacement text for a match (`$&`, `$$`, `$n` in regexp mode).
    pub fn replacement(&self, m: &SearchMatch) -> String {
        let text = self.unquote(&self.replace);
        let Some(groups) = m.groups.as_ref().filter(|_| self.regexp) else {
            return text;
        };
        let mut out = String::with_capacity(text.len());
        let b = text.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'$' && i + 1 < b.len() {
                let rest = &text[i + 1..];
                if rest.starts_with('&') {
                    out.push_str(groups[0].as_deref().unwrap_or(""));
                    i += 2;
                    continue;
                }
                if rest.starts_with('$') {
                    out.push('$');
                    i += 2;
                    continue;
                }
                let digits: &str = &rest[..rest.bytes().take_while(u8::is_ascii_digit).count()];
                if !digits.is_empty() {
                    let mut done = false;
                    for l in (1..=digits.len()).rev() {
                        let n: usize = digits[..l].parse().unwrap_or(0);
                        if n > 0 && n < groups.len() {
                            out.push_str(groups[n].as_deref().unwrap_or("undefined"));
                            out.push_str(&digits[l..]);
                            done = true;
                            break;
                        }
                    }
                    if !done {
                        out.push('$');
                        out.push_str(digits);
                    }
                    i += 1 + digits.len();
                    continue;
                }
            }
            let ch = text[i..].chars().next().unwrap_or('$');
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    /// Every match, or `None` past `limit`.
    pub fn match_all(&self, state: &EditorState, limit: usize) -> Option<Vec<SearchMatch>> {
        let mut out = Vec::new();
        let mut over = false;
        self.for_each_match(state, 0, state.doc.len(), false, |m| {
            if out.len() >= limit {
                over = true;
                return false;
            }
            out.push(m);
            true
        });
        (!over).then_some(out)
    }

    /// Matches to highlight in the visible range `from..to`.
    pub fn highlight(
        &self,
        state: &EditorState,
        from: usize,
        to: usize,
        mut add: impl FnMut(usize, usize),
    ) {
        let margin = if self.regexp {
            HIGHLIGHT_MARGIN
        } else {
            self.unquoted.len()
        };
        let len = state.doc.len();
        self.for_each_match(
            state,
            from.saturating_sub(margin),
            (to + margin).min(len),
            false,
            |m| {
                add(m.from, m.to);
                true
            },
        );
    }
}

#[derive(Clone, Debug)]
pub enum SearchEffect {
    SetQuery(Rc<SearchQuery>),
    TogglePanel(bool),
}

/// The search state field: the current query and whether the panel is open.
#[derive(Clone, Debug, Default)]
pub struct SearchState {
    pub query: Rc<SearchQuery>,
    pub panel: bool,
}

impl SearchState {
    pub fn new(query: SearchQuery) -> SearchState {
        SearchState {
            query: Rc::new(query),
            panel: false,
        }
    }

    pub(crate) fn update(&self, tr: &Transaction) -> SearchState {
        let mut next = self.clone();
        for e in &tr.effects {
            match e {
                Effect::Search(SearchEffect::SetQuery(q)) => next.query = q.clone(),
                Effect::Search(SearchEffect::TogglePanel(open)) => next.panel = *open,
                _ => {}
            }
        }
        next
    }
}

/// `defaultQuery`: the selected text (up to 100 bytes, one range) with the
/// fallback's flags.
pub fn default_query(state: &EditorState, fallback: Option<&SearchQuery>) -> SearchQuery {
    let sel = state.selection.main();
    let sel_text = if sel.is_empty() || sel.to > sel.from + 100 {
        String::new()
    } else {
        state.slice_doc(sel.from, sel.to)
    };
    if let Some(f) = fallback
        && sel_text.is_empty()
    {
        return f.clone();
    }
    let literal = fallback.is_some_and(|f| f.literal);
    SearchQuery::new(SearchQueryConfig {
        search: if literal {
            sel_text
        } else {
            sel_text.replace('\n', "\\n")
        },
        case_sensitive: fallback.is_some_and(|f| f.case_sensitive),
        literal,
        regexp: fallback.is_some_and(|f| f.regexp),
        whole_word: fallback.is_some_and(|f| f.whole_word),
        replace: String::new(),
    })
}

/// What a search command did.
pub enum SearchRun {
    Dispatch(TransactionSpec),
    /// Nothing to do (no match, read-only).
    Failed,
    /// The query is unusable: open the panel instead (`searchCommand`).
    OpenPanel,
}

fn search_command(
    state: &EditorState,
    f: impl FnOnce(&SearchQuery) -> Option<TransactionSpec>,
) -> SearchRun {
    if !state.search.query.valid {
        return SearchRun::OpenPanel;
    }
    match f(&state.search.query) {
        Some(spec) => SearchRun::Dispatch(spec),
        None => SearchRun::Failed,
    }
}

const ANNOUNCE_MARGIN: usize = 30;

fn is_break(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_whitespace() || ".,:;?!".contains(c))
}

/// `announceMatch`: "current match. <context> on line N."
pub fn announce_match(state: &EditorState, from: usize, to: usize) -> Effect {
    let line = state.doc.line_at(from);
    let line_end = state.doc.line_at(to).to;
    let mut start = line.from.max(from.saturating_sub(ANNOUNCE_MARGIN));
    start = state.doc.clamp_to_char(start);
    let end = to_char_end(&state.doc, line_end.min(to + ANNOUNCE_MARGIN));
    let text: Vec<char> = state.slice_doc(start, end).chars().collect();
    let mut slice = &text[..];
    if start != line.from {
        for i in 0..ANNOUNCE_MARGIN.min(slice.len().saturating_sub(1)) {
            if !is_break(slice.get(i + 1).copied()) && is_break(slice.get(i).copied()) {
                slice = &slice[i..];
                break;
            }
        }
    }
    if end != line_end {
        let n = slice.len();
        let mut i = n.saturating_sub(1);
        while i > n.saturating_sub(ANNOUNCE_MARGIN) && i > 0 {
            if !is_break(slice.get(i - 1).copied()) && is_break(slice.get(i).copied()) {
                slice = &slice[..i];
                break;
            }
            i -= 1;
        }
    }
    let text: String = slice.iter().collect();
    Effect::Announce(format!("current match. {text} on line {}.", line.number))
}

fn select_match(state: &EditorState, m: SearchMatch) -> TransactionSpec {
    let selection = EditorSelection::single(m.from, m.to);
    TransactionSpec::default()
        .selection(selection.clone())
        .effect(announce_match(state, m.from, m.to))
        .effect(Effect::ScrollIntoView(ScrollTarget::range(
            selection.main(),
        )))
        .user_event("select.search")
}

/// `findNext`. On dispatch the view also selects the find field's text when
/// it has focus (`selectSearchInput`).
pub fn find_next(state: &EditorState) -> SearchRun {
    search_command(state, |q| {
        let to = state.selection.main().to;
        q.next_match(state, to, to).map(|m| select_match(state, m))
    })
}

/// `findPrevious`.
pub fn find_previous(state: &EditorState) -> SearchRun {
    search_command(state, |q| {
        let from = state.selection.main().from;
        q.prev_match(state, from, from)
            .map(|m| select_match(state, m))
    })
}

/// `selectMatches`: select every match (up to 1000).
pub fn select_matches(state: &EditorState) -> SearchRun {
    search_command(state, |q| {
        let ranges = q.match_all(state, 1000).filter(|r| !r.is_empty())?;
        Some(
            TransactionSpec::default()
                .selection(EditorSelection::create(
                    ranges
                        .iter()
                        .map(|r| SelectionRange::range(r.from, r.to))
                        .collect(),
                    0,
                ))
                .user_event("select.search.matches"),
        )
    })
}

/// `selectSelectionMatches` (Mod-Shift-l).
pub fn select_selection_matches(state: &EditorState) -> Option<TransactionSpec> {
    let sel = &state.selection;
    if sel.ranges.len() > 1 || sel.main().is_empty() {
        return None;
    }
    let SelectionRange { from, to, .. } = sel.main();
    let query = state.slice_doc(from, to);
    let mut ranges = Vec::new();
    let mut main = 0;
    let mut cur = SearchCursor::new(&state.doc, &query, 0, state.doc.len(), false);
    while let Some(m) = cur.next_match() {
        if ranges.len() > 1000 {
            return None;
        }
        if m.from == from {
            main = ranges.len();
        }
        ranges.push(SelectionRange::range(m.from, m.to));
    }
    Some(
        TransactionSpec::default()
            .selection(EditorSelection::create(ranges, main))
            .user_event("select.search.matches"),
    )
}

/// `replaceNext`.
pub fn replace_next(state: &EditorState) -> SearchRun {
    search_command(state, |q| {
        if state.read_only() {
            return None;
        }
        let SelectionRange { from, to, .. } = state.selection.main();
        let first = q.next_match(state, from, from)?;
        let mut next = Some(first.clone());
        let mut changes = Vec::new();
        let mut effects = Vec::new();
        if !first.precise {
            next = q.next_match(state, first.from, first.to);
        } else if first.from == from && first.to == to {
            changes.push(Change::new(first.from, first.to, q.replacement(&first)));
            next = q.next_match(state, first.from, first.to);
            effects.push(Effect::Announce(format!(
                "replaced match on line {}.",
                state.doc.line_at(from).number
            )));
        }
        let change_set = state.changes(&changes);
        let mut spec = TransactionSpec::default();
        if let Some(n) = next {
            let selection = EditorSelection::single(n.from, n.to).map(&change_set.desc(), -1);
            effects.push(announce_match(state, n.from, n.to));
            effects.push(Effect::ScrollIntoView(ScrollTarget::range(
                selection.main(),
            )));
            spec = spec.selection(selection);
        }
        Some(
            spec.changes(change_set)
                .effects(effects)
                .user_event("input.replace"),
        )
    })
}

/// `replaceAll`.
pub fn replace_all(state: &EditorState) -> SearchRun {
    search_command(state, |q| {
        if state.read_only() {
            return None;
        }
        let changes: Vec<Change> = q
            .match_all(state, usize::MAX)
            .unwrap_or_default()
            .iter()
            .filter(|m| m.precise)
            .map(|m| Change::new(m.from, m.to, q.replacement(m)))
            .collect();
        if changes.is_empty() {
            return None;
        }
        let announce = format!("replaced {} matches.", changes.len());
        Some(
            TransactionSpec::default()
                .changes(changes)
                .effect(Effect::Announce(announce))
                .user_event("input.replace.all"),
        )
    })
}

/// `openSearchPanel`: `input_focused` says whether the find field already
/// has focus. The view focuses and selects the field afterwards.
pub fn open_search_panel(state: &EditorState, input_focused: bool) -> Option<TransactionSpec> {
    let current = &state.search;
    if current.panel {
        if input_focused {
            return None;
        }
        let query = default_query(state, Some(&current.query));
        return query.valid.then(|| {
            TransactionSpec::default()
                .effect(Effect::Search(SearchEffect::SetQuery(Rc::new(query))))
        });
    }
    Some(
        TransactionSpec::default()
            .effect(Effect::Search(SearchEffect::TogglePanel(true)))
            .effect(Effect::Search(SearchEffect::SetQuery(Rc::new(
                default_query(state, Some(&current.query)),
            )))),
    )
}

/// `closeSearchPanel`; the view refocuses the editor if focus was in the
/// panel.
pub fn close_search_panel(state: &EditorState) -> Option<TransactionSpec> {
    state.search.panel.then(|| {
        TransactionSpec::default().effect(Effect::Search(SearchEffect::TogglePanel(false)))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchFlag {
    CaseSensitive,
    Regexp,
    WholeWord,
}

/// Recur's ⌥⌘C / ⌥⌘R / ⌥⌘W panel toggles.
pub fn toggle_search_flag(state: &EditorState, flag: SearchFlag) -> Option<TransactionSpec> {
    let q = &state.search.query;
    let mut config = q.config();
    match flag {
        SearchFlag::CaseSensitive => config.case_sensitive = !config.case_sensitive,
        SearchFlag::Regexp => config.regexp = !config.regexp,
        SearchFlag::WholeWord => config.whole_word = !config.whole_word,
    }
    let next = SearchQuery::new(config);
    (!next.eq(q)).then(|| {
        TransactionSpec::default().effect(Effect::Search(SearchEffect::SetQuery(Rc::new(next))))
    })
}

/// `selectWord`: select the word around each cursor.
fn select_word(state: &EditorState) -> Option<TransactionSpec> {
    let sel = &state.selection;
    let next = EditorSelection::create(
        sel.ranges
            .iter()
            .map(|r| {
                state
                    .word_at(r.head())
                    .unwrap_or_else(|| SelectionRange::cursor(r.head()))
            })
            .collect(),
        sel.main_index,
    );
    (!next.eq_sel(sel, false)).then(|| TransactionSpec::default().selection(next))
}

fn find_next_occurrence(state: &EditorState, query: &str) -> Option<SearchMatch> {
    let main = state.selection.main();
    let ranges = &state.selection.ranges;
    let last = ranges[ranges.len() - 1];
    let full_word = state
        .word_at(main.head())
        .is_some_and(|w| w.from == main.from && w.to == main.to);
    let len = state.doc.len();
    let mut cycled = false;
    let mut cur = SearchCursor::new(&state.doc, query, last.to, len, false);
    loop {
        match cur.next_match() {
            None => {
                if cycled {
                    return None;
                }
                cur = SearchCursor::new(&state.doc, query, 0, last.from.saturating_sub(1), false);
                cycled = true;
            }
            Some(m) => {
                if cycled && ranges.iter().any(|r| r.from == m.from) {
                    continue;
                }
                if full_word
                    && state
                        .word_at(m.from)
                        .is_none_or(|w| w.from != m.from || w.to != m.to)
                {
                    continue;
                }
                return Some(m);
            }
        }
    }
}

/// `selectNextOccurrence` (⌘D).
pub fn select_next_occurrence(state: &EditorState) -> Option<TransactionSpec> {
    let ranges = &state.selection.ranges;
    if ranges.iter().any(|r| r.is_empty()) {
        return select_word(state);
    }
    let searched = state.slice_doc(ranges[0].from, ranges[0].to);
    if ranges
        .iter()
        .any(|r| state.slice_doc(r.from, r.to) != searched)
    {
        return None;
    }
    let m = find_next_occurrence(state, &searched)?;
    Some(
        TransactionSpec::default()
            .selection(
                state
                    .selection
                    .add_range(SelectionRange::range(m.from, m.to), false),
            )
            .effect(Effect::ScrollIntoView(ScrollTarget::range(
                SelectionRange::cursor(m.to),
            ))),
    )
}

/// A search highlight: `selected` when it equals a selection range
/// (`cm-searchMatch-selected`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchHighlight {
    pub from: usize,
    pub to: usize,
    pub selected: bool,
}

/// The search highlighter: matches in the visible ranges while the panel
/// is open, merging ranges closer than two highlight margins.
pub fn search_highlights(state: &EditorState, visible: &[(usize, usize)]) -> Vec<MatchHighlight> {
    let s = &state.search;
    let mut out = Vec::new();
    if !s.panel || !s.query.valid {
        return out;
    }
    let mut i = 0;
    while i < visible.len() {
        let (from, mut to) = visible[i];
        while i + 1 < visible.len() && to + 2 * HIGHLIGHT_MARGIN > visible[i + 1].0 {
            i += 1;
            to = visible[i].1;
        }
        s.query.highlight(state, from, to, |from, to| {
            let selected = state
                .selection
                .ranges
                .iter()
                .any(|r| r.from == from && r.to == to);
            out.push(MatchHighlight { from, to, selected });
        });
        i += 1;
    }
    out
}

/// `highlightSelectionMatches()` options as Recur configures them.
const SELECTION_MATCH_MIN: usize = 1;
const SELECTION_MATCH_MAX: usize = 100;

/// Ranges matching the (single, non-empty, ≤200 byte) selection in the
/// visible ranges, excluding the selection itself (`cm-selectionMatch`).
/// More than 100 matches shows none.
pub fn selection_match_highlights(
    state: &EditorState,
    visible: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let sel = &state.selection;
    if sel.ranges.len() > 1 {
        return Vec::new();
    }
    let range = sel.main();
    if range.is_empty() {
        return Vec::new();
    }
    let len = range.to - range.from;
    if !(SELECTION_MATCH_MIN..=200).contains(&len) {
        return Vec::new();
    }
    let query = state.slice_doc(range.from, range.to);
    if query.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &(from, to) in visible {
        let mut cur = SearchCursor::new(&state.doc, &query, from, to, false);
        while let Some(m) = cur.next_match() {
            if m.from >= range.to || m.to <= range.from {
                out.push((m.from, m.to));
            }
            if out.len() > SELECTION_MATCH_MAX {
                return Vec::new();
            }
        }
    }
    out
}

/// Recur's find panel cap on counted matches.
pub const MATCH_CAP: usize = 999;

/// The panel's match counter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchCount {
    /// Invalid or empty query: blank.
    Empty,
    /// "No results".
    None,
    /// "cur / total" (`—` when the selection is not on a match, `999+`
    /// when capped).
    Some {
        current: usize,
        total: usize,
        capped: bool,
    },
}

impl SearchCount {
    pub fn label(&self) -> String {
        match self {
            SearchCount::Empty => String::new(),
            SearchCount::None => "No results".into(),
            SearchCount::Some {
                current,
                total,
                capped,
            } => {
                let total = if *capped {
                    format!("{MATCH_CAP}+")
                } else {
                    total.to_string()
                };
                let cur = if *current > 0 {
                    current.to_string()
                } else {
                    "\u{2014}".into()
                };
                format!("{cur} / {total}")
            }
        }
    }
}

/// Recur's find panel (`RecurSearchPanel`): field values, toggle state and
/// the cached match count. The view renders it and forwards edits.
#[derive(Clone, Debug)]
pub struct SearchPanel {
    pub search: String,
    pub replace: String,
    pub case_sensitive: bool,
    pub regexp: bool,
    pub whole_word: bool,
    query: Rc<SearchQuery>,
    total_cache: Option<(String, usize, bool)>,
    pub count: SearchCount,
}

impl SearchPanel {
    pub fn new(state: &EditorState) -> SearchPanel {
        let q = state.search.query.clone();
        let mut panel = SearchPanel {
            search: q.search.clone(),
            replace: q.replace.clone(),
            case_sensitive: q.case_sensitive,
            regexp: q.regexp,
            whole_word: q.whole_word,
            query: q,
            total_cache: None,
            count: SearchCount::Empty,
        };
        panel.refresh_count(state, false);
        panel
    }

    /// Whether the replace row shows (hidden for read-only editors).
    pub fn has_replace(state: &EditorState) -> bool {
        !state.read_only()
    }

    /// `commitQuery`: the effect to dispatch when the fields changed the
    /// query. Recount against the post-dispatch state with
    /// [`SearchPanel::refresh_count`].
    pub fn commit(&mut self) -> Option<TransactionSpec> {
        let query = SearchQuery::new(SearchQueryConfig {
            search: self.search.clone(),
            case_sensitive: self.case_sensitive,
            regexp: self.regexp,
            whole_word: self.whole_word,
            replace: self.replace.clone(),
            literal: false,
        });
        if query.eq(&self.query) {
            return None;
        }
        self.query = Rc::new(query);
        Some(
            TransactionSpec::default()
                .effect(Effect::Search(SearchEffect::SetQuery(self.query.clone()))),
        )
    }

    fn cache_key(&self, state: &EditorState) -> String {
        let q = &self.query;
        format!(
            "{}\0{}\0{}\0{}\0{}",
            q.search,
            u8::from(q.case_sensitive),
            u8::from(q.regexp),
            u8::from(q.whole_word),
            state.doc.len()
        )
    }

    pub fn refresh_count(&mut self, state: &EditorState, recount_total: bool) {
        if !self.query.valid {
            self.total_cache = None;
            self.count = SearchCount::Empty;
            return;
        }
        let key = self.cache_key(state);
        if recount_total || self.total_cache.as_ref().is_none_or(|c| c.0 != key) {
            let mut total = 0;
            let mut capped = false;
            self.query
                .for_each_match(state, 0, state.doc.len(), false, |_| {
                    total += 1;
                    if total >= MATCH_CAP {
                        capped = true;
                        return false;
                    }
                    true
                });
            self.total_cache = Some((key, total, capped));
        }
        let (_, total, capped) = self.total_cache.clone().unwrap_or_default();
        if total == 0 {
            self.count = SearchCount::None;
            return;
        }
        let sel = state.selection.main();
        let mut i = 0;
        let mut current = 0;
        self.query
            .for_each_match(state, 0, state.doc.len(), false, |m| {
                i += 1;
                if m.from == sel.from && m.to == sel.to {
                    current = i;
                    return false;
                }
                m.from <= sel.to && i < MATCH_CAP
            });
        self.count = SearchCount::Some {
            current,
            total,
            capped,
        };
    }

    fn set_query(&mut self, q: Rc<SearchQuery>) {
        self.search = q.search.clone();
        self.replace = q.replace.clone();
        self.case_sensitive = q.case_sensitive;
        self.regexp = q.regexp;
        self.whole_word = q.whole_word;
        self.query = q;
    }

    /// `update(ViewUpdate)` for one transaction.
    pub fn update(&mut self, tr: &Transaction) {
        let mut changed_query = false;
        for e in &tr.effects {
            if let Effect::Search(SearchEffect::SetQuery(q)) = e
                && !q.eq(&self.query)
            {
                self.set_query(q.clone());
                changed_query = true;
            }
        }
        let state = tr.state();
        if changed_query {
            self.refresh_count(state, false);
        }
        if tr.doc_changed() {
            self.refresh_count(state, true);
        } else if tr.selection.is_some() {
            self.refresh_count(state, false);
        }
    }
}

/// Parse a go-to-line target: `N`, `+N` / `-N` relative lines, `N%` of the
/// document, with an optional `:col` (`parseGotoTarget`).
pub fn parse_goto_target(raw: &str, state: &EditorState) -> Option<SelectionRange> {
    let s = raw.trim();
    let b = s.as_bytes();
    let mut i = 0;
    let sign = match b.first() {
        Some(b'+') | Some(b'-') => {
            i = 1;
            Some(b[0])
        }
        _ => None,
    };
    let digits = |i: usize| b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    let n = digits(i);
    let ln = (n > 0).then(|| &s[i..i + n]);
    i += n;
    let mut col = 0usize;
    if b.get(i) == Some(&b':') {
        let n = digits(i + 1);
        if n == 0 {
            return None;
        }
        col = s[i + 1..i + 1 + n].parse().unwrap_or(usize::MAX);
        i += 1 + n;
    }
    let percent = b.get(i) == Some(&b'%');
    if percent {
        i += 1;
    }
    if i != b.len() {
        return None;
    }
    let start_line = state.doc.line_at(state.selection.main().head());
    let lines = state.doc.lines() as f64;
    let mut line = ln.map_or(start_line.number as f64, |l| {
        l.parse::<f64>().unwrap_or(f64::MAX)
    });
    if ln.is_some() && percent {
        let mut pc = line / 100.0;
        if let Some(sign) = sign {
            pc = pc * if sign == b'-' { -1.0 } else { 1.0 } + start_line.number as f64 / lines;
        }
        line = (lines * pc).round();
    } else if ln.is_some()
        && let Some(sign) = sign
    {
        line = line * if sign == b'-' { -1.0 } else { 1.0 } + start_line.number as f64;
    }
    let n = line.min(lines).max(1.0) as usize;
    let doc_line = state.doc.line(n);
    let col = col.min(doc_line.len());
    let col = state.doc.clamp_to_char(doc_line.from + col) - doc_line.from;
    Some(SelectionRange::cursor(doc_line.from + col))
}

/// The go-to-line dialog's initial value: the current line number.
pub fn goto_line_initial(state: &EditorState) -> String {
    state
        .doc
        .line_at(state.selection.main().head())
        .number
        .to_string()
}

/// Submitting the go-to-line dialog: move there and center it. `None`
/// closes the dialog without moving.
pub fn goto_line_submit(state: &EditorState, raw: &str) -> Option<TransactionSpec> {
    let sel = parse_goto_target(raw, state)?;
    Some(
        TransactionSpec::default()
            .selection(EditorSelection::of_range(sel))
            .effect(Effect::ScrollIntoView(ScrollTarget::pos(
                sel.from,
                ScrollStrategy::Center,
                5.0,
            ))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateConfig;

    fn st(doc: &str) -> EditorState {
        EditorState::create(
            doc,
            StateConfig {
                allow_multiple_selections: true,
                ..StateConfig::default()
            },
        )
    }

    fn with_sel(s: &EditorState, anchor: usize, head: usize) -> EditorState {
        s.update(TransactionSpec::default().selection(EditorSelection::single(anchor, head)))
            .state()
            .clone()
    }

    fn with_query(s: &EditorState, config: SearchQueryConfig) -> EditorState {
        s.update(
            TransactionSpec::default().effect(Effect::Search(SearchEffect::SetQuery(Rc::new(
                SearchQuery::new(config),
            )))),
        )
        .state()
        .clone()
    }

    fn all(cur: &mut SearchCursor) -> Vec<(usize, usize)> {
        std::iter::from_fn(|| cur.next_match())
            .map(|m| (m.from, m.to))
            .collect()
    }

    #[test]
    fn string_cursor_matches_and_normalizes() {
        let t = Text::of("one two\nthree two");
        assert_eq!(
            all(&mut SearchCursor::new(&t, "two", 0, t.len(), false)),
            vec![(4, 7), (14, 17)]
        );
        assert_eq!(
            all(&mut SearchCursor::new(&t, "TWO", 0, t.len(), true)),
            vec![(4, 7), (14, 17)]
        );
        assert_eq!(
            all(&mut SearchCursor::new(&t, "TWO", 0, t.len(), false)),
            vec![]
        );
        assert_eq!(
            all(&mut SearchCursor::new(&t, "two", 5, t.len(), false)),
            vec![(14, 17)]
        );
        assert_eq!(
            all(&mut SearchCursor::new(&t, "o\nt", 0, t.len(), false)),
            vec![(6, 9)]
        );
        let t = Text::of("aaaa");
        assert_eq!(
            all(&mut SearchCursor::new(&t, "aa", 0, 4, false)),
            vec![(0, 2), (2, 4)]
        );
        let mut c = SearchCursor::new(&t, "aa", 0, 4, false);
        let overlapping: Vec<_> = std::iter::from_fn(|| c.next_overlapping())
            .map(|m| m.from)
            .collect();
        assert_eq!(overlapping, vec![0, 1, 2]);
        // "ﬁ" (U+FB01) normalizes to "fi"; a match inside it is imprecise.
        let t = Text::of("\u{FB01}x");
        let m = SearchCursor::new(&t, "ix", 0, t.len(), false)
            .next_match()
            .unwrap();
        assert_eq!((m.from, m.to, m.precise), (0, 4, false));
        let t = Text::of("caf\u{E9}");
        assert!(
            SearchCursor::new(&t, "cafe\u{301}", 0, t.len(), false)
                .next_match()
                .is_some()
        );
    }

    #[test]
    fn regexp_cursor_lines_and_multiline() {
        let t = Text::of("foo1 bar22\nbaz333");
        let mut c = RegExpCursor::new(&t, "\\d+", false, 0, t.len());
        let found: Vec<_> = std::iter::from_fn(|| c.next_match())
            .map(|m| (m.from, m.to))
            .collect();
        assert_eq!(found, vec![(3, 4), (8, 10), (14, 17)]);
        let mut c = RegExpCursor::new(&t, "0\\nb|2\\nb", false, 0, t.len());
        assert_eq!(c.next_match().map(|m| (m.from, m.to)), Some((9, 12)));
        let mut c = RegExpCursor::new(&t, "^", false, 0, t.len());
        let found: Vec<_> = std::iter::from_fn(|| c.next_match())
            .map(|m| m.from)
            .collect();
        assert_eq!(found, vec![0, 11]);
        let mut c = RegExpCursor::new(&t, "BAZ", true, 0, t.len());
        assert_eq!(c.next_match().map(|m| m.from), Some(11));
        let mut c = RegExpCursor::new(&t, "x*", false, 0, 3);
        let found: Vec<_> = std::iter::from_fn(|| c.next_match())
            .map(|m| m.from)
            .collect();
        assert_eq!(found, vec![0, 1, 2, 3]);
    }

    #[test]
    fn whole_word_queries() {
        let s = st("cat concat cat_x cat");
        let q = SearchQuery::new(SearchQueryConfig {
            search: "cat".into(),
            whole_word: true,
            ..Default::default()
        });
        assert_eq!(
            q.match_all(&s, 100)
                .unwrap()
                .iter()
                .map(|m| m.from)
                .collect::<Vec<_>>(),
            vec![0, 17]
        );
        let re = SearchQuery::new(SearchQueryConfig {
            search: "c.t".into(),
            regexp: true,
            whole_word: true,
            ..Default::default()
        });
        assert_eq!(
            re.match_all(&s, 100)
                .unwrap()
                .iter()
                .map(|m| m.from)
                .collect::<Vec<_>>(),
            vec![0, 17]
        );
    }

    #[test]
    fn unquote_and_replacement() {
        let q = SearchQuery::new(SearchQueryConfig {
            search: "a\\nb\\\\".into(),
            replace: "x\\ty".into(),
            ..Default::default()
        });
        assert_eq!(q.unquoted, "a\nb\\");
        let m = SearchMatch {
            from: 0,
            to: 0,
            precise: true,
            groups: None,
        };
        assert_eq!(q.replacement(&m), "x\ty");
        let re = SearchQuery::new(SearchQueryConfig {
            search: "(\\w+)-(\\w+)".into(),
            replace: "$2+$1 $& $$ $12 $9".into(),
            regexp: true,
            ..Default::default()
        });
        let m = SearchMatch {
            from: 0,
            to: 7,
            precise: true,
            groups: Some(vec![
                Some("foo-bar".into()),
                Some("foo".into()),
                Some("bar".into()),
            ]),
        };
        assert_eq!(re.replacement(&m), "bar+foo foo-bar $ foo2 $9");
        assert!(
            !SearchQuery::new(SearchQueryConfig {
                search: "(".into(),
                regexp: true,
                ..Default::default()
            })
            .valid
        );
    }

    #[test]
    fn find_next_and_previous_wrap() {
        let s = with_query(
            &st("ab ab ab"),
            SearchQueryConfig {
                search: "ab".into(),
                ..Default::default()
            },
        );
        let s = with_sel(&s, 4, 4);
        let SearchRun::Dispatch(spec) = find_next(&s) else {
            panic!()
        };
        let s = s.update(spec).state().clone();
        assert_eq!((s.selection.main().from, s.selection.main().to), (6, 8));
        let SearchRun::Dispatch(spec) = find_next(&s) else {
            panic!()
        };
        let s = s.update(spec).state().clone();
        assert_eq!(s.selection.main().from, 0);
        let SearchRun::Dispatch(spec) = find_previous(&s) else {
            panic!()
        };
        let s = s.update(spec).state().clone();
        assert_eq!(s.selection.main().from, 6);
        assert!(matches!(find_next(&st("x")), SearchRun::OpenPanel));
    }

    #[test]
    fn replace_next_and_all() {
        let s = with_query(
            &st("a1 a2 a3"),
            SearchQueryConfig {
                search: "a(\\d)".into(),
                replace: "<$1>".into(),
                regexp: true,
                ..Default::default()
            },
        );
        let SearchRun::Dispatch(spec) = replace_next(&s) else {
            panic!()
        };
        let s = s.update(spec).state().clone();
        assert_eq!(s.doc.to_string(), "a1 a2 a3");
        assert_eq!((s.selection.main().from, s.selection.main().to), (0, 2));
        let SearchRun::Dispatch(spec) = replace_next(&s) else {
            panic!()
        };
        let s = s.update(spec).state().clone();
        assert_eq!(s.doc.to_string(), "<1> a2 a3");
        assert_eq!((s.selection.main().from, s.selection.main().to), (4, 6));
        let SearchRun::Dispatch(spec) = replace_all(&s) else {
            panic!()
        };
        assert_eq!(s.update(spec).state().doc.to_string(), "<1> <2> <3>");
    }

    #[test]
    fn select_next_occurrence_and_selection_matches() {
        let s = with_sel(&st("foo bar foo foobar foo"), 1, 1);
        let s = s
            .update(select_next_occurrence(&s).unwrap())
            .state()
            .clone();
        assert_eq!((s.selection.main().from, s.selection.main().to), (0, 3));
        let s = s
            .update(select_next_occurrence(&s).unwrap())
            .state()
            .clone();
        let s = s
            .update(select_next_occurrence(&s).unwrap())
            .state()
            .clone();
        let froms: Vec<_> = s.selection.ranges.iter().map(|r| r.from).collect();
        assert_eq!(froms, vec![0, 8, 19]);
        let s = with_sel(&s, 0, 3);
        let s = s
            .update(select_selection_matches(&s).unwrap())
            .state()
            .clone();
        assert_eq!(s.selection.ranges.len(), 4);
    }

    #[test]
    fn panel_state_and_counts() {
        let s = with_sel(&st("alpha beta alpha\nalpha"), 0, 5);
        let s = s
            .update(open_search_panel(&s, false).unwrap())
            .state()
            .clone();
        assert!(s.search.panel);
        assert_eq!(s.search.query.search, "alpha");
        let mut panel = SearchPanel::new(&s);
        assert_eq!(panel.count.label(), "1 / 3");
        panel.case_sensitive = true;
        let tr = s.update(panel.commit().unwrap());
        panel.update(&tr);
        assert!(tr.state().search.query.case_sensitive);
        panel.search = "zzz".into();
        let tr = tr.state().update(panel.commit().unwrap());
        panel.refresh_count(tr.state(), false);
        assert_eq!(panel.count.label(), "No results");
        let s = tr.state().clone();
        let s = s
            .update(toggle_search_flag(&s, SearchFlag::Regexp).unwrap())
            .state()
            .clone();
        assert!(s.search.query.regexp);
        assert!(close_search_panel(&s).is_some());
        assert_eq!(
            SearchCount::Some {
                current: 0,
                total: 999,
                capped: true
            }
            .label(),
            "\u{2014} / 999+"
        );
    }

    #[test]
    fn highlights() {
        let s = with_sel(&st("ab ab ab"), 0, 2);
        assert_eq!(
            selection_match_highlights(&s, &[(0, 8)]),
            vec![(3, 5), (6, 8)]
        );
        let s = s
            .update(open_search_panel(&s, false).unwrap())
            .state()
            .clone();
        let h = search_highlights(&s, &[(0, 8)]);
        assert_eq!(h.len(), 3);
        assert!(h[0].selected && !h[1].selected);
    }

    #[test]
    fn goto_line_targets() {
        let s = with_sel(&st("l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10"), 9, 9);
        let pos = |raw: &str| parse_goto_target(raw, &s).map(|r| r.head());
        assert_eq!(pos("1"), Some(0));
        assert_eq!(pos("2:1"), Some(4));
        assert_eq!(pos("+2"), Some(15));
        assert_eq!(pos("-1"), Some(6));
        assert_eq!(pos("50%"), Some(12));
        assert_eq!(pos("99"), Some(27));
        assert_eq!(pos("3:99"), Some(8));
        assert_eq!(pos(""), Some(9));
        assert_eq!(pos("x"), None);
        assert_eq!(goto_line_initial(&s), "4");
    }
}
