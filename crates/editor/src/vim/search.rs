//! Vim search: query parsing (`parseQuery`, `translateRegex`), match
//! highlighting, `findNext`, and the `/` `?` `*` `#` commands
//! (`processSearch`).

use std::rc::Rc;
use std::time::Duration;

use recur_textmate::JsRegex;

use super::cm::{Cm, ScrollInfo};
use super::helpers::*;
use super::prompt::{Desc, Prefix, PromptKind, PromptOptions};
use super::state::with_global;
use super::types::*;
use super::{clear_input_state, dispatch};

/// A JavaScript `RegExp` as vim keeps it: `source` (escaped like
/// `RegExp.prototype.source`) and `flags`.
#[derive(Clone)]
pub struct VimRegex {
    pub source: String,
    pub flags: String,
    pub re: Rc<JsRegex>,
}

impl std::fmt::Debug for VimRegex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display())
    }
}

impl PartialEq for VimRegex {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.flags == other.flags
    }
}

impl VimRegex {
    /// `new RegExp(pattern, flags)`; `None` where JavaScript would throw.
    pub fn new(pattern: &str, flags: &str) -> Option<VimRegex> {
        let re = JsRegex::new(pattern, flags)?;
        let mut sorted: Vec<char> = flags.chars().collect();
        sorted.sort_unstable_by_key(|c| "dgimsuvy".find(*c).unwrap_or(9));
        Some(VimRegex {
            source: regexp_source(pattern),
            flags: sorted.into_iter().collect(),
            re: Rc::new(re),
        })
    }

    /// `String(regexp)`.
    pub fn display(&self) -> String {
        format!("/{}/{}", self.source, self.flags)
    }

    pub fn ignore_case(&self) -> bool {
        self.flags.contains('i')
    }

    /// `regexp.test(text)`.
    pub fn test(&self, text: &str) -> bool {
        self.re.test(text)
    }
}

/// `RegExp.prototype.source`: unescaped `/` and line breaks escaped.
fn regexp_source(pattern: &str) -> String {
    if pattern.is_empty() {
        return "(?:)".into();
    }
    let mut out = String::with_capacity(pattern.len());
    let mut escaped = false;
    let mut in_class = false;
    for c in pattern.chars() {
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                out.push(c);
                escaped = true;
            }
            '[' => {
                in_class = true;
                out.push(c);
            }
            ']' => {
                in_class = false;
                out.push(c);
            }
            '/' if !in_class => out.push_str("\\/"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

/// `getSearchCursor`'s source fixup: braces that are not quantifiers are
/// escaped for the unicode-mode cursor.
pub fn cm6_source(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            out.push(c);
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '{' {
            // {(?:\d+(?:,\d*)?|,\d+)}
            let mut j = i + 1;
            let digits = |j: &mut usize| {
                let s = *j;
                while *j < chars.len() && chars[*j].is_ascii_digit() {
                    *j += 1;
                }
                *j > s
            };
            let ok = if digits(&mut j) {
                if j < chars.len() && chars[j] == ',' {
                    j += 1;
                    digits(&mut j);
                }
                true
            } else if j < chars.len() && chars[j] == ',' {
                j += 1;
                digits(&mut j)
            } else {
                false
            };
            if ok && j < chars.len() && chars[j] == '}' {
                out.extend(&chars[i..=j]);
                i = j + 1;
                continue;
            }
            out.push_str("\\{");
            i += 1;
            continue;
        }
        if c == '}' {
            out.push_str("\\}");
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `findUnescapedSeparators`.
pub fn find_unescaped_separators(s: &str, sep: char) -> Vec<usize> {
    let mut escape_next = false;
    let mut out = Vec::new();
    for (i, c) in s.chars().enumerate() {
        if !escape_next && c == sep {
            out.push(i);
        }
        escape_next = !escape_next && c == '\\';
    }
    out
}

/// `splitBySeparator`: `None` when the string does not start with it.
pub fn split_by_separator(s: &str, sep: char) -> Option<Vec<String>> {
    let slashes = find_unescaped_separators(s, sep);
    if slashes.is_empty() {
        return Some(Vec::new());
    }
    if slashes[0] != 0 {
        return None;
    }
    let chars: Vec<char> = s.chars().collect();
    let mut tokens = Vec::new();
    for (k, &start) in slashes.iter().enumerate() {
        let end = slashes.get(k + 1).copied().unwrap_or(chars.len());
        tokens.push(chars[start + 1..end].iter().collect());
    }
    Some(tokens)
}

/// `translateRegex`: vim regexp syntax to JavaScript.
pub fn translate_regex(s: &str) -> String {
    let modes = |c: char| -> Option<&'static str> {
        Some(match c {
            'V' => "|(){+?*.[$^",
            'M' => "|(){+?*.[",
            'm' => "|(){+?",
            'v' => "<>",
            _ => return None,
        })
    };
    let escapes = |c: char| -> Option<&'static str> {
        Some(match c {
            '>' => "(?<=[\\w])(?=[^\\w]|$)",
            '<' => "(?<=[^\\w]|^)(?=[\\w])",
            _ => return None,
        })
    };
    let mut specials = "|(){+?";
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let next = chars[i + 1];
            i += 2;
            if next == '}' || specials.contains(next) {
                out.push(next);
            } else if let Some(m) = modes(next) {
                specials = m;
            } else if let Some(e) = escapes(next) {
                out.push_str(e);
            } else {
                out.push('\\');
                out.push(next);
            }
            continue;
        }
        if "[|(){+*?.$^<>".contains(c) {
            if specials.contains(c) {
                match escapes(c) {
                    Some(e) => out.push_str(e),
                    None => {
                        out.push('\\');
                        out.push(c);
                    }
                }
            } else {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    if let Some(i) = out.find("\\zs") {
        out = format!("(?<={}){}", &out[..i], &out[i + 3..]);
    }
    if let Some(i) = out.find("\\ze") {
        out = format!("{}(?={})", &out[..i], &out[i + 3..]);
    }
    out
}

/// `translateRegexReplace`.
pub fn translate_regex_replace(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let at =
        |i: i64| -> Option<char> { (i >= 0).then(|| chars.get(i as usize).copied()).flatten() };
    let mut escape_next = false;
    let mut out = String::new();
    let mut i: i64 = -1;
    while i < chars.len() as i64 {
        let c = at(i);
        let n = at(i + 1);
        let pair = (c, n);
        let unescape = match pair {
            (Some('\\'), Some('n')) => Some('\n'),
            (Some('\\'), Some('r')) => Some('\r'),
            (Some('\\'), Some('t')) => Some('\t'),
            _ => None,
        };
        if let Some(u) = unescape {
            out.push(u);
            i += 1;
        } else if escape_next {
            if let Some(c) = c {
                out.push(c);
            }
            escape_next = false;
        } else if c == Some('\\') {
            escape_next = true;
            if n.is_some_and(|n| n.is_ascii_digit() || n == '$') {
                out.push('$');
            } else if n != Some('/') && n != Some('\\') {
                out.push('\\');
            }
        } else {
            if c == Some('$') {
                out.push('$');
            }
            if let Some(c) = c {
                out.push(c);
            }
            if n == Some('/') {
                out.push('\\');
            }
        }
        i += 1;
    }
    out
}

/// `unescapeRegexReplace` (pcre mode).
pub fn unescape_regex_replace(s: &str) -> String {
    let pairs = [
        ("\\/", "/"),
        ("\\\\", "\\"),
        ("\\n", "\n"),
        ("\\r", "\r"),
        ("\\t", "\t"),
        ("\\&", "&"),
    ];
    let mut out = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(i) = rest.find('\\') {
            out.push_str(&rest[..i]);
            rest = &rest[i..];
        } else {
            out.push_str(rest);
            break;
        }
        if let Some((from, to)) = pairs.iter().find(|(from, _)| rest.starts_with(from)) {
            out.push_str(to);
            rest = &rest[from.len()..];
        } else {
            out.push('\\');
            rest = &rest[1..];
        }
    }
    out
}

/// `parseQuery`; `Err(())` where `new RegExp` would throw.
pub fn parse_query(
    query: &str,
    ignore_case: bool,
    smart_case: bool,
) -> Result<Option<VimRegex>, ()> {
    with_global(|g| g.registers.get(Some("/")).set_text(query, false, false));
    let slashes = find_unescaped_separators(query, '/');
    let chars: Vec<char> = query.chars().collect();
    let (mut part, force_ignore) = if slashes.is_empty() {
        (query.to_string(), false)
    } else {
        let p: String = chars[..slashes[0]].iter().collect();
        let flags: String = chars[slashes[0]..].iter().collect();
        (p, flags.contains('i'))
    };
    if part.is_empty() {
        return Ok(None);
    }
    if !with_global(|g| g.pcre()) {
        part = translate_regex(&part);
    }
    let mut ignore = ignore_case;
    if smart_case {
        ignore = !part.chars().any(|c| c.is_ascii_uppercase());
    }
    let flags = if ignore || force_ignore { "im" } else { "m" };
    VimRegex::new(&part, flags).map(Some).ok_or(())
}

/// `updateSearchQuery`.
pub fn update_search_query(
    cm: &mut Cm,
    raw: &str,
    ignore_case: bool,
    smart_case: bool,
) -> Result<Option<VimRegex>, ()> {
    if raw.is_empty() {
        return Ok(None);
    }
    let Some(query) = parse_query(raw, ignore_case, smart_case)? else {
        return Ok(None);
    };
    highlight_search_matches(cm, &query);
    let same = with_global(|g| g.query.as_ref() == Some(&query));
    if !same {
        with_global(|g| g.query = Some(query.clone()));
    }
    Ok(Some(query))
}

/// `highlightSearchMatches`: install the overlay after 50ms.
pub fn highlight_search_matches(cm: &mut Cm, query: &VimRegex) {
    let q = query.clone();
    cm.vim().search_state.highlight_pending = true;
    let task = cm.cx.spawn_in(cm.window, async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        this.update_in(cx, |view, window, cx| {
            if view.vim.is_none() {
                return;
            }
            let mut cm = Cm::new(view, window, cx);
            cm.vim().search_state.highlight_pending = false;
            let current = cm.vim().search_state.overlay.clone();
            if current.as_ref() != Some(&q) {
                if current.is_some() {
                    cm.remove_overlay();
                }
                cm.add_overlay(&q.source, &q.flags);
                cm.vim().search_state.overlay = Some(q.clone());
            }
        })
        .ok();
    });
    if let Some(v) = cm.view.vim.as_mut() {
        v.highlight_timer = Some(task);
    }
}

/// `clearSearchHighlight`.
pub fn clear_search_highlight(cm: &mut Cm) {
    if cm.vim().search_state.highlight_pending {
        cm.vim().search_state.highlight_pending = false;
        if let Some(v) = cm.view.vim.as_mut() {
            v.highlight_timer = None;
        }
    }
    cm.remove_overlay();
    cm.vim().search_state.overlay = None;
}

/// `findNext`.
pub fn find_next(cm: &mut Cm, prev: bool, query: &VimRegex, repeat: i64) -> Option<Pos> {
    cm.op_begin();
    let result = (|| {
        let pos = cm.get_cursor();
        let mut cursor = cm.get_search_cursor(query, pos);
        for i in 0..repeat.max(1) {
            let mut found = cm.cursor_find(&mut cursor, prev);
            if i == 0 && found && cursor.from == Some(pos) {
                let last_end = if prev { cursor.from } else { cursor.to };
                found = cm.cursor_find(&mut cursor, prev);
                if found
                    && Cm::cursor_empty_match(&cursor)
                    && cursor.from == last_end
                    && let Some(le) = last_end
                    && cm.line_len(le.line) == le.ch
                {
                    found = cm.cursor_find(&mut cursor, prev);
                }
            }
            if !found {
                let start = if prev {
                    Pos::new(cm.last_line(), INF)
                } else {
                    Pos::new(cm.first_line(), 0)
                };
                cursor = cm.get_search_cursor(query, start);
                if !cm.cursor_find(&mut cursor, prev) {
                    return None;
                }
            }
        }
        cursor.from
    })();
    cm.op_end();
    result
}

/// `findNextFromAndToInclusive`.
pub fn find_next_from_and_to_inclusive(
    cm: &mut Cm,
    prev: bool,
    query: &VimRegex,
    repeat: i64,
) -> Option<(Pos, Pos)> {
    cm.op_begin();
    let result = (|| {
        let pos = cm.get_cursor();
        let mut cursor = cm.get_search_cursor(query, pos);
        let found = cm.cursor_find(&mut cursor, !prev);
        if !cm.vim().visual_mode && found && cursor.from == Some(pos) {
            cm.cursor_find(&mut cursor, !prev);
        }
        for _ in 0..repeat.max(1) {
            if !cm.cursor_find(&mut cursor, prev) {
                let start = if prev {
                    Pos::new(cm.last_line(), INF)
                } else {
                    Pos::new(cm.first_line(), 0)
                };
                cursor = cm.get_search_cursor(query, start);
                if !cm.cursor_find(&mut cursor, prev) {
                    return None;
                }
            }
        }
        Some((cursor.from?, cursor.to?))
    })();
    cm.op_end();
    result
}

/// `processSearch`.
pub(crate) fn process_search(cm: &mut Cm, args: SearchArgs) {
    with_global(|g| g.is_reversed = !args.forward);
    match args.query_src {
        QuerySrc::Prompt => {
            let playing = with_global(|g| g.macro_mode_state.is_playing);
            if playing {
                let query = with_global(|g| {
                    if g.macro_mode_state.replay_search_queries.is_empty() {
                        None
                    } else {
                        Some(g.macro_mode_state.replay_search_queries.remove(0))
                    }
                });
                handle_query(
                    cm,
                    &query.unwrap_or_default(),
                    true,
                    false,
                    args.to_jumplist,
                );
            } else {
                let original_query = with_global(|g| g.query.clone());
                let original_scroll = cm.get_scroll_info();
                super::prompt::show_prompt(
                    cm,
                    PromptOptions {
                        prefix: Prefix::Text(if args.forward { "/".into() } else { "?".into() }),
                        desc: Some(Desc::Pcre),
                        value: None,
                        kind: PromptKind::Search {
                            forward: args.forward,
                            to_jumplist: args.to_jumplist,
                            original_query,
                            original_scroll,
                            last_query: String::new(),
                        },
                    },
                );
            }
        }
        QuerySrc::WordUnderCursor => {
            let mut word = super::motions::expand_word_under_cursor(
                cm, false, false, false, true, false, None,
            );
            let mut is_keyword = true;
            if word.is_none() {
                word = super::motions::expand_word_under_cursor(
                    cm, false, false, false, false, false, None,
                );
                is_keyword = false;
            }
            let Some(word) = word else {
                show_confirm(cm, "No word under cursor");
                clear_input_state(cm);
                return;
            };
            let line = cm.line_chars(word.start.line);
            let text: String = line[(word.start.ch.max(0) as usize).min(line.len())
                ..(word.end.ch.max(0) as usize).min(line.len())]
                .iter()
                .collect();
            let query = if is_keyword && args.whole_word_only {
                format!("\\b{text}\\b")
            } else {
                escape_regex(&text)
            };
            let c = cm.get_cursor();
            with_global(|g| g.jump_list.cached_cursor = Some(c));
            cm.set_cursor(word.start);
            handle_query(cm, &query, true, false, args.to_jumplist);
        }
    }
}

/// `processSearch`'s `handleQuery`.
pub(crate) fn handle_query(
    cm: &mut Cm,
    query: &str,
    ignore_case: bool,
    smart_case: bool,
    to_jumplist: bool,
) {
    with_global(|g| {
        g.search_history.push_input(query);
        g.search_history.reset();
    });
    if update_search_query(cm, query, ignore_case, smart_case).is_err() {
        show_confirm(cm, &format!("Invalid regex: {query}"));
        clear_input_state(cm);
        return;
    }
    dispatch::process_motion(
        cm,
        Motion::FindNext,
        &MotionArgs {
            forward: Some(true),
            to_jumplist,
            ..Default::default()
        },
    );
}

/// Search prompt `onClose`.
pub(crate) fn search_prompt_close(cm: &mut Cm, query: &str, to_jumplist: bool, scroll: ScrollInfo) {
    cm.scroll_to(Some(scroll.left), Some(scroll.top));
    handle_query(cm, query, true, true, to_jumplist);
    if with_global(|g| g.macro_mode_state.is_recording) {
        super::insert::log_search_query(query);
    }
}

/// Search prompt `onChange` (incremental search).
pub(crate) fn search_prompt_change(cm: &mut Cm, query: &str, forward: bool, scroll: ScrollInfo) {
    let parsed = update_search_query(cm, query, true, true).ok().flatten();
    match parsed {
        Some(q) => {
            let p = find_next(cm, !forward, &q, 1);
            cm.scroll_into_view(p);
        }
        None => {
            clear_search_highlight(cm);
            cm.scroll_to(Some(scroll.left), Some(scroll.top));
        }
    }
}

/// Search prompt Escape / Ctrl-C / empty Backspace.
pub(crate) fn search_prompt_cancel(
    cm: &mut Cm,
    query: &str,
    original: Option<&VimRegex>,
    scroll: ScrollInfo,
) {
    with_global(|g| {
        g.search_history.push_input(query);
        g.search_history.reset();
    });
    let source = original.map(|q| q.source.clone()).unwrap_or_default();
    let _ = update_search_query(cm, &source, false, false);
    clear_search_highlight(cm);
    cm.scroll_to(Some(scroll.left), Some(scroll.top));
    clear_input_state(cm);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_vim_regex() {
        assert_eq!(translate_regex("a\\(b\\)"), "a(b)");
        assert_eq!(translate_regex("a(b)"), "a\\(b\\)");
        assert_eq!(
            translate_regex("\\<foo\\>"),
            "(?<=[^\\w]|^)(?=[\\w])foo(?<=[\\w])(?=[^\\w]|$)"
        );
        assert_eq!(translate_regex("\\vfoo(bar)"), "foo(bar)");
        assert_eq!(translate_regex("foo\\zsbar"), "(?<=foo)bar");
        assert_eq!(translate_regex_replace("\\1-$x\\n"), "$1-$$x\n");
        assert_eq!(unescape_regex_replace("a\\/b\\&"), "a/b&");
        assert_eq!(
            split_by_separator("/a/b/g", '/'),
            Some(vec!["a".into(), "b".into(), "g".into()])
        );
        assert_eq!(split_by_separator("a/b", '/'), None);
        assert_eq!(cm6_source("a{2}b{x}"), "a{2}b\\{x\\}");
        assert_eq!(regexp_source("a/b"), "a\\/b");
    }

    #[test]
    fn parses_queries() {
        crate::vim::state::reset_global();
        let q = parse_query("Foo", true, true).unwrap().unwrap();
        assert_eq!(q.display(), "/Foo/m");
        let q = parse_query("foo/i", false, false).unwrap().unwrap();
        assert_eq!(q.display(), "/foo/im");
        assert_eq!(parse_query("", true, true), Ok(None));
    }
}
