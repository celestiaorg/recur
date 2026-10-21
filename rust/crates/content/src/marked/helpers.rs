//! `helpers.ts`.

use super::rules::rules;
use crate::js::{self, encode_uri};

fn replacement(ch: &str) -> &'static str {
    match ch {
        "&" => "&amp;",
        "<" => "&lt;",
        ">" => "&gt;",
        "\"" => "&quot;",
        _ => "&#39;",
    }
}

/// `escapeHtmlEntities(html, encode)`.
pub fn escape_html_entities(html: &str, encode: bool) -> String {
    let o = &rules().other;
    if encode {
        if o.escape_test.test(html) {
            return o
                .escape_replace
                .replace_with(html, |c| replacement(c.full()).into());
        }
    } else if o.escape_test_no_encode.test(html) {
        return o
            .escape_replace_no_encode
            .replace_with(html, |c| replacement(c.full()).into());
    }
    html.to_string()
}

/// `cleanUrl`: `encodeURI` (which cannot fail on a Rust string) with `%25` decoded back.
pub fn clean_url(href: &str) -> Option<String> {
    Some(rules().other.percent_decode.replace(&encode_uri(href), "%"))
}

/// `splitCells(tableRow, count)`.
pub fn split_cells(table_row: &str, count: Option<usize>) -> Vec<String> {
    let o = &rules().other;
    let bytes = table_row.as_bytes();
    let row = o.find_pipe.replace_with(table_row, |c| {
        let mut escaped = false;
        let mut curr = c.index();
        while curr > 0 {
            curr -= 1;
            if bytes[curr] != b'\\' {
                break;
            }
            escaped = !escaped;
        }
        if escaped { "|".into() } else { " |".into() }
    });
    let mut cells: Vec<String> = o
        .split_pipe
        .split(&row)
        .into_iter()
        .map(|p| p.unwrap_or("").to_string())
        .collect();
    if cells.first().is_some_and(|c| js::trim(c).is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| js::trim(c).is_empty()) {
        cells.pop();
    }
    if let Some(count) = count {
        if cells.len() > count {
            cells.truncate(count);
        } else {
            while cells.len() < count {
                cells.push(String::new());
            }
        }
    }
    cells
        .into_iter()
        .map(|c| o.slash_pipe.replace(js::trim(&c), "|"))
        .collect()
}

/// `rtrim(str, c, invert)`: drop trailing `c`s (or trailing non-`c`s when inverted).
pub fn rtrim(s: &str, c: char, invert: bool) -> String {
    let mut end = s.len();
    for (i, ch) in s.char_indices().rev() {
        let keep_going = if invert { ch != c } else { ch == c };
        if !keep_going {
            break;
        }
        end = i;
    }
    s[..end].to_string()
}

/// `trimTrailingBlankLines`: keep a single trailing blank line, drop more.
pub fn trim_trailing_blank_lines(s: &str) -> String {
    let o = &rules().other;
    let lines: Vec<&str> = s.split('\n').collect();
    let mut end = lines.len() as i64 - 1;
    while end >= 0 && o.blank_line.test(lines[end as usize]) {
        end -= 1;
    }
    if lines.len() as i64 - end <= 2 {
        return s.to_string();
    }
    lines[..(end + 1) as usize].join("\n")
}

/// `findClosingBracket(str, b)`: byte index of the unmatched closer, -1 none, -2 unbalanced.
pub fn find_closing_bracket(s: &str, b: (char, char)) -> i64 {
    if !s.contains(b.1) {
        return -1;
    }
    let mut level = 0;
    let mut chars = s.char_indices();
    while let Some((i, ch)) = chars.next() {
        if ch == '\\' {
            chars.next();
        } else if ch == b.0 {
            level += 1;
        } else if ch == b.1 {
            level -= 1;
            if level < 0 {
                return i as i64;
            }
        }
    }
    if level > 0 {
        return -2;
    }
    -1
}

/// `expandTabs(line, indent)`: tabs to the next multiple of four columns.
pub fn expand_tabs(line: &str, indent: usize) -> String {
    let mut col = indent;
    let mut out = String::with_capacity(line.len());
    for ch in line.chars() {
        if ch == '\t' {
            let added = 4 - (col % 4);
            out.extend(std::iter::repeat_n(' ', added));
            col += added;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_cells_keeps_escaped_pipes() {
        assert_eq!(split_cells("| a | b \\| c |", None), vec!["a", "b | c"]);
        assert_eq!(split_cells("a | b", Some(3)), vec!["a", "b", ""]);
    }

    #[test]
    fn closing_bracket_and_tabs() {
        assert_eq!(find_closing_bracket("a(b)c)", ('(', ')')), 5);
        assert_eq!(find_closing_bracket("a((b)", ('(', ')')), -2);
        assert_eq!(expand_tabs("\tx", 1), "   x");
        assert_eq!(rtrim("ab##", '#', false), "ab");
        assert_eq!(
            escape_html_entities("a & b < \"c\" '", true),
            "a &amp; b &lt; &quot;c&quot; &#39;"
        );
        assert_eq!(escape_html_entities("&amp; &", false), "&amp; &amp;");
    }
}
