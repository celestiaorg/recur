//! Clickable links: OSC 8 hyperlinks (xterm.js `OscLinkProvider`, registered first so it wins)
//! and plain-text web URLs (`@xterm/addon-web-links`).

use std::sync::OnceLock;

use alacritty_terminal::term::cell::Hyperlink;

use super::buffer::Buffer;

/// `strictUrlRegex` from the web-links addon, evaluated as ECMAScript.
const STRICT_URL: &str =
    r#"(https?|HTTPS?):[/]{2}[^\s"'!*(){}|\\\^<>`]*[^\s"':,.!?{}|\\\^~\[\]`()<>]"#;
const WINDOW_LIMIT: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// OSC 8 hyperlink. Recur's webview had no confirm() dialog or window.open, so activating
    /// one did nothing; hover decorations still show.
    Hyperlink,
    /// A URL detected in the text; activation opens it in the browser.
    Web,
}

/// A link under the pointer. Coordinates are xterm's 1-based, end-inclusive `IBufferRange`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub kind: LinkKind,
    pub text: String,
    pub start: (i64, i64),
    pub end: (i64, i64),
}

impl Link {
    pub(crate) fn contains(&self, cols: i64, x: i64, y: i64) -> bool {
        let lower = self.start.1 * cols + self.start.0;
        let upper = self.end.1 * cols + self.end.0;
        let current = y * cols + x;
        lower <= current && current <= upper
    }

    /// `getRangeLength` for double-click / right-click word selection over a link.
    pub(crate) fn range_length(&self, cols: i64) -> i64 {
        (self.end.1 - self.start.1) * cols + self.end.0 - self.start.0 + 1
    }
}

fn url_regex() -> Option<&'static regress::Regex> {
    static RE: OnceLock<Option<regress::Regex>> = OnceLock::new();
    RE.get_or_init(|| regress::Regex::new(STRICT_URL).ok())
        .as_ref()
}

/// Every link on buffer row `y` (1-based), providers in xterm's priority order.
pub(crate) fn links_for_row(buf: &Buffer, y: i64) -> Vec<Vec<Link>> {
    vec![osc_links(buf, y), web_links(buf, y)]
}

/// The link at 1-based position (`x`, `y`), first provider wins.
pub(crate) fn link_at(buf: &Buffer, x: i64, y: i64) -> Option<Link> {
    let cols = buf.cols() as i64;
    links_for_row(buf, y)
        .into_iter()
        .find_map(|links| links.into_iter().find(|l| l.contains(cols, x, y)))
}

fn hyperlink(buf: &Buffer, row: usize, col: usize) -> Option<Hyperlink> {
    buf.hyperlink(row, col)
}

fn osc_links(buf: &Buffer, y: i64) -> Vec<Link> {
    let mut out = Vec::new();
    if y < 1 || !buf.has_row(y as usize - 1) {
        return out;
    }
    let row = y as usize - 1;
    let len = buf.trimmed_length(row);
    let mut current: Option<(usize, Hyperlink)> = None;
    for x in 0..len {
        if current.is_none() && !buf.has_content(row, x) {
            continue;
        }
        let here = hyperlink(buf, row, x);
        let mut finish = false;
        match (&current, &here) {
            // A link that starts on the last trimmed cell is never closed, as in xterm.js.
            (None, Some(h)) => {
                current = Some((x, h.clone()));
                continue;
            }
            (Some((_, cur)), Some(h)) => finish = h != cur,
            (Some(_), None) => finish = true,
            (None, None) => {}
        }
        if finish || (current.is_some() && x == len - 1) {
            if let Some((start, link)) = &current {
                let end_x = x + usize::from(!finish && x == len - 1);
                let uri = link.uri().to_string();
                let http =
                    url::Url::parse(&uri).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
                if !uri.is_empty() && http {
                    let (s, e) = osc_range_with_wrap(buf, y, *start, end_x, link);
                    out.push(Link {
                        kind: LinkKind::Hyperlink,
                        text: uri,
                        start: s,
                        end: e,
                    });
                }
            }
            current = here.map(|h| (x, h));
        }
    }
    out
}

fn osc_range_with_wrap(
    buf: &Buffer,
    y: i64,
    start_x: usize,
    end_x: usize,
    link: &Hyperlink,
) -> ((i64, i64), (i64, i64)) {
    let has =
        |row: i64, x: usize| row >= 0 && hyperlink(buf, row as usize, x).as_ref() == Some(link);
    let (mut start_y, mut fsx, mut end_y, mut fex) = (y, start_x, y, end_x);
    while fsx == 0 {
        if start_y < 2 || !buf.is_wrapped(start_y as usize - 1) {
            break;
        }
        let prev = start_y - 2;
        let prev_len = buf.trimmed_length(prev as usize);
        if prev_len == 0 || !has(prev, prev_len - 1) {
            break;
        }
        let mut ps = prev_len - 1;
        while ps > 0 && has(prev, ps - 1) {
            ps -= 1;
        }
        start_y -= 1;
        fsx = ps;
    }
    loop {
        let cur = end_y - 1;
        if cur < 0 || !buf.has_row(cur as usize) || fex != buf.trimmed_length(cur as usize) {
            break;
        }
        let next = end_y as usize;
        if !buf.has_row(next) || !buf.is_wrapped(next) {
            break;
        }
        let next_len = buf.trimmed_length(next);
        if next_len == 0 || !has(next as i64, 0) {
            break;
        }
        let mut ne = 1;
        while ne < next_len && has(next as i64, ne) {
            ne += 1;
        }
        end_y += 1;
        fex = ne;
    }
    ((fsx as i64 + 1, start_y), (fex as i64, end_y))
}

/// The web-links addon's `isUrl`: the match must parse and begin with its own origin.
fn is_url(text: &str) -> bool {
    let Ok(u) = url::Url::parse(text) else {
        return false;
    };
    let host = match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        (None, _) => String::new(),
    };
    let base = match (u.username(), u.password()) {
        (user, Some(pass)) if !user.is_empty() => format!("{}://{user}:{pass}@{host}", u.scheme()),
        (user, _) if !user.is_empty() => format!("{}://{user}@{host}", u.scheme()),
        _ => format!("{}://{host}", u.scheme()),
    };
    text.to_lowercase().starts_with(&base.to_lowercase())
}

fn web_links(buf: &Buffer, y: i64) -> Vec<Link> {
    let Some(re) = url_regex() else {
        return Vec::new();
    };
    let (lines, start_line) = windowed_lines(buf, y - 1);
    let line: Vec<u16> = lines.concat().encode_utf16().collect();
    let mut out = Vec::new();
    for m in re.find_from_ucs2(&line, 0) {
        let text = String::from_utf16_lossy(&line[m.range()]);
        if !is_url(&text) {
            continue;
        }
        let (sy, sx) = map_str_idx(buf, start_line, 0, m.start() as i64);
        let (ey, ex) = map_str_idx(buf, sy, sx, m.range().len() as i64);
        if sy < 0 || sx < 0 || ey < 0 || ex < 0 {
            continue;
        }
        out.push(Link {
            kind: LinkKind::Web,
            text,
            start: (sx + 1, sy + 1),
            end: (ex, ey + 1),
        });
    }
    out
}

fn windowed_lines(buf: &Buffer, line_index: i64) -> (Vec<String>, i64) {
    let mut top = line_index;
    let mut bottom = line_index;
    let mut lines = Vec::new();
    if line_index < 0 || !buf.has_row(line_index as usize) {
        return (lines, top);
    }
    let current = buf.translate(line_index as usize, true, 0, None);
    if buf.is_wrapped(line_index as usize) && !current.starts_with(' ') {
        let mut len = 0;
        let mut above = Vec::new();
        while top > 0 && len < WINDOW_LIMIT {
            top -= 1;
            let content = buf.translate(top as usize, true, 0, None);
            len += content.encode_utf16().count();
            let stop = !buf.is_wrapped(top as usize) || content.contains(' ');
            above.push(content);
            if stop {
                break;
            }
        }
        above.reverse();
        lines.extend(above);
    }
    lines.push(current);
    let mut len = 0;
    while buf.has_row(bottom as usize + 1)
        && buf.is_wrapped(bottom as usize + 1)
        && len < WINDOW_LIMIT
    {
        bottom += 1;
        let content = buf.translate(bottom as usize, true, 0, None);
        len += content.encode_utf16().count();
        let stop = content.contains(' ');
        lines.push(content);
        if stop {
            break;
        }
    }
    (lines, top)
}

fn map_str_idx(
    buf: &Buffer,
    mut line_index: i64,
    row_index: i64,
    mut string_index: i64,
) -> (i64, i64) {
    let mut start = row_index;
    while string_index != 0 {
        if line_index < 0 || !buf.has_row(line_index as usize) {
            return (-1, -1);
        }
        let row = line_index as usize;
        let cols = buf.cols() as i64;
        for i in start..cols {
            let Some(cell) = buf.cell_info(row, i as usize) else {
                break;
            };
            if cell.width != 0 {
                string_index -= (cell.chars.encode_utf16().count() as i64).max(1);
                if i == cols - 1
                    && cell.chars.is_empty()
                    && buf.has_row(row + 1)
                    && buf.is_wrapped(row + 1)
                    && buf.cell_info(row + 1, 0).is_some_and(|c| c.width == 2)
                {
                    string_index += 1;
                }
            }
            if string_index < 0 {
                return (line_index, i);
            }
        }
        line_index += 1;
        start = 0;
    }
    (line_index, start)
}
