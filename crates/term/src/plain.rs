/// How much recent output `@terminal` wrap and `read_terminal` include when the caller does not
/// ask for a size.
pub const DEFAULT_TAIL_LINES: usize = 200;
/// Caps a `read_terminal` tail so one dump cannot blow the turn.
pub const MAX_TAIL_LINES: usize = 2000;
const MAX_PLAIN_BYTES: usize = 32 << 10;

/// Drops CSI/OSC/DCS and other C0 controls except newline, tab and CR. Use this when CLI stderr
/// (Codex tracing, coloured CLIs) is shown outside a PTY.
pub fn strip_vt(s: &str) -> String {
    strip(s.as_bytes())
}

/// Converts raw VT bytes to readable text and keeps the last `max_lines` lines. Cursor-addressed
/// full-screen programs (vim, htop) will not reconstruct as a screen: this is a stream tail, which
/// is what shell and test output needs.
pub fn tail_plain(raw: &[u8], max_lines: usize) -> String {
    let max_lines = match max_lines {
        0 => DEFAULT_TAIL_LINES,
        n => n.min(MAX_TAIL_LINES),
    };
    let s = strip(raw).replace("\r\n", "\n");
    // CR overwrites the current line (progress bars, spinners). Keep only the last segment so
    // redraws do not explode into hundreds of tail lines.
    let s = collapse_carriage_returns(&s);
    let mut lines: Vec<&str> = s.split('\n').collect();
    // Drop a trailing empty from a final newline so "last N" is real lines.
    if lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.len() > max_lines {
        lines.drain(..lines.len() - max_lines);
    }
    let out = lines.join("\n");
    if out.len() <= MAX_PLAIN_BYTES {
        return out;
    }
    let mut cut = out.len() - MAX_PLAIN_BYTES;
    while !out.is_char_boundary(cut) {
        cut += 1;
    }
    let mut tail = &out[cut..];
    if let Some(i) = tail.find('\n') {
        tail = &tail[i + 1..];
    }
    format!("…\n{tail}")
}

fn collapse_carriage_returns(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    s.split('\n')
        .map(|line| line.rfind('\r').map_or(line, |j| &line[j + 1..]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drops CSI/OSC/DCS and other C0 controls except newline, tab and CR so a replay ring can be
/// attached as chat context. Incomplete sequences at a wrapped-ring start are discarded with the
/// rest of the escape.
fn strip(p: &[u8]) -> String {
    let mut b = String::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        let c = p[i];
        match c {
            0x1b => i = skip_esc(p, i),
            0x9b => i = skip_until_final(p, i + 1),
            0x9d => i = skip_osc(p, i + 1),
            b'\n' | b'\t' | b'\r' => {
                b.push(c as char);
                i += 1;
            }
            _ if c < 0x20 || c == 0x7f => i += 1,
            _ if c < 0x80 => {
                b.push(c as char);
                i += 1;
            }
            _ => match decode_rune(&p[i..]) {
                Some((r, size)) => {
                    b.push(r);
                    i += size;
                }
                None => i += 1,
            },
        }
    }
    b
}

/// One UTF-8 scalar at the start of `p`, or `None`.
pub(crate) fn decode_rune(p: &[u8]) -> Option<(char, usize)> {
    let len = match p.first()? {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let s = std::str::from_utf8(p.get(..len)?).ok()?;
    s.chars().next().map(|r| (r, len))
}

fn skip_esc(p: &[u8], i: usize) -> usize {
    if i + 1 >= p.len() {
        return p.len();
    }
    match p[i + 1] {
        b'[' => skip_until_final(p, i + 2),
        b']' => skip_osc(p, i + 2),
        b'P' | b'X' | b'^' | b'_' => skip_st(p, i + 2),
        b'(' => {
            if i + 2 < p.len() {
                i + 3
            } else {
                p.len()
            }
        }
        _ => i + 2,
    }
}

fn skip_until_final(p: &[u8], mut i: usize) -> usize {
    while i < p.len() {
        if (0x40..=0x7e).contains(&p[i]) {
            return i + 1;
        }
        i += 1;
    }
    i
}

fn skip_osc(p: &[u8], mut i: usize) -> usize {
    while i < p.len() {
        if p[i] == 0x07 {
            return i + 1;
        }
        if p[i] == 0x1b && i + 1 < p.len() && p[i + 1] == b'\\' {
            return i + 2;
        }
        i += 1;
    }
    i
}

fn skip_st(p: &[u8], mut i: usize) -> usize {
    while i < p.len() {
        if p[i] == 0x1b && i + 1 < p.len() && p[i + 1] == b'\\' {
            return i + 2;
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_and_keeps_text() {
        assert_eq!(
            tail_plain(b"hello \x1b[31mred\x1b[0m world", 50),
            "hello red world"
        );
        assert_eq!(
            strip_vt("\x1b[2mdim\x1b[0m \x1b[31mERROR\x1b[0m boom"),
            "dim ERROR boom"
        );
    }

    #[test]
    fn last_lines() {
        let mut b = String::new();
        for i in 1..=10 {
            b.push_str(&"x".repeat(i));
            b.push('\n');
        }
        assert_eq!(
            tail_plain(b.as_bytes(), 3),
            "xxxxxxxx\nxxxxxxxxx\nxxxxxxxxxx"
        );
    }

    #[test]
    fn collapses_cr_overwrite() {
        assert_eq!(tail_plain(b"a\rb\r\nc", 10), "b\nc");
    }

    #[test]
    fn spinner_does_not_evict_error() {
        let mut b = String::new();
        for i in 0..500 {
            b.push_str(&format!("\rBuilding {i}%"));
        }
        b.push_str("\nFATAL: boom\n");
        let got = tail_plain(b.as_bytes(), 200);
        assert!(
            got.contains("FATAL: boom"),
            "spinner evicted the error: {got:?}"
        );
        assert!(
            got.matches("Building").count() <= 1,
            "spinner frames were not collapsed: {got:?}"
        );
    }

    #[test]
    fn drops_osc() {
        assert_eq!(tail_plain(b"pre\x1b]0;title\x07post", 10), "prepost");
    }

    #[test]
    fn empty_input_defaults() {
        assert_eq!(tail_plain(b"", 0), "");
    }

    #[test]
    fn caps_output_bytes() {
        let line = "y".repeat(100);
        let raw: String = (0..1000).map(|_| format!("{line}\n")).collect();
        let got = tail_plain(raw.as_bytes(), MAX_TAIL_LINES);
        assert!(got.starts_with("…\n"));
        assert!(got.len() <= MAX_PLAIN_BYTES + "…\n".len());
        assert!(got["…\n".len()..].lines().all(|l| l == line));
    }

    #[test]
    fn keeps_multibyte_and_drops_invalid_bytes() {
        assert_eq!(strip(b"caf\xc3\xa9 \xff ok"), "café  ok");
    }
}
