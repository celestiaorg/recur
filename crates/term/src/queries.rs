/// Default charcoal theme colours as 16-bit xterm `rgb:` reports. Matching the UI's `--bg`,
/// `--text` and `--amber` keeps termenv/pterm dark-mode detection consistent with what the user
/// sees even though replies never round-trip the renderer.
pub(crate) const DEFAULT_FG_REPORT: &str = "rgb:e6e6/e4e4/e1e1";
pub(crate) const DEFAULT_BG_REPORT: &str = "rgb:1414/1414/1414";
pub(crate) const DEFAULT_CURSOR_REPORT: &str = "rgb:e0e0/7a7a/5f5f";

/// The exact probes intercepted. Anything else (OSC titles, other CSI) passes through. The last
/// one, CSI 6n, is only answered here while no client is attached; a live screen reports the real
/// cursor.
const KNOWN_QUERIES: &[&[u8]] = &[
    b"\x1b]10;?\x1b\\",
    b"\x1b]10;?\x07",
    b"\x1b]11;?\x1b\\",
    b"\x1b]11;?\x07",
    b"\x1b]12;?\x1b\\",
    b"\x1b]12;?\x07",
    b"\x1b[6n",
];

/// Answers terminal status probes on the PTY side and strips them from the stream forwarded to
/// the emulator.
///
/// Tools built on muesli/termenv (pterm, charmbracelet, …) probe OSC 10/11/12, fenced
/// by CSI 6n, to learn fg/bg colours. A real terminal replies on the same fd within microseconds;
/// answering here keeps the probe synchronous with the reader so the reply cannot land after the
/// tool moved on and show up as typed garbage (`11;rgb:1414/1414/1414`, `1R`) on the prompt.
pub(crate) struct QueryFilter {
    carry: Vec<u8>,
}

impl QueryFilter {
    pub(crate) fn new() -> Self {
        Self { carry: Vec::new() }
    }

    /// Consumes PTY output, returning the bytes to forward and any immediate replies to write
    /// back to the PTY master.
    pub(crate) fn process(&mut self, input: &[u8], attached: bool) -> (Vec<u8>, Vec<u8>) {
        let queries = if attached {
            &KNOWN_QUERIES[..KNOWN_QUERIES.len() - 1]
        } else {
            KNOWN_QUERIES
        };
        let mut forward = Vec::new();
        let mut replies = Vec::new();
        if self.carry.is_empty() && input.is_empty() {
            return (forward, replies);
        }
        let data = if self.carry.is_empty() {
            input.to_vec()
        } else {
            let mut d = std::mem::take(&mut self.carry);
            d.extend_from_slice(input);
            d
        };
        let mut i = 0;
        while i < data.len() {
            if data[i] != 0x1b {
                let j = data[i + 1..]
                    .iter()
                    .position(|&b| b == 0x1b)
                    .map_or(data.len(), |p| i + 1 + p);
                forward.extend_from_slice(&data[i..j]);
                i = j;
                continue;
            }
            match match_terminal_query(&data[i..], queries) {
                Match::Incomplete => {
                    self.carry = data[i..].to_vec();
                    break;
                }
                Match::Query(n) => {
                    replies.extend_from_slice(&reply_for(&data[i..i + n]));
                    i += n;
                }
                Match::None => {
                    forward.push(data[i]);
                    i += 1;
                }
            }
        }
        (forward, replies)
    }
}

fn reply_for(query: &[u8]) -> Vec<u8> {
    // CSI 6n with nothing attached: termenv only uses it as a fence after the OSC probe, so the
    // coordinates don't matter as long as the shape is valid.
    if query == b"\x1b[6n" {
        return b"\x1b[1;1R".to_vec();
    }
    if query.len() < 6 || query[0] != 0x1b || query[1] != b']' || query[2] != b'1' {
        return Vec::new();
    }
    let colour = match query[3] {
        b'0' => DEFAULT_FG_REPORT,
        b'1' => DEFAULT_BG_REPORT,
        b'2' => DEFAULT_CURSOR_REPORT,
        _ => return Vec::new(),
    };
    format!("\x1b]1{};{colour}\x07", query[3] as char).into_bytes()
}

enum Match {
    Query(usize),
    Incomplete,
    None,
}

fn match_terminal_query(data: &[u8], queries: &[&[u8]]) -> Match {
    let mut could_be_prefix = false;
    for q in queries {
        if data.len() >= q.len() && &data[..q.len()] == *q {
            return Match::Query(q.len());
        }
        if data.len() < q.len() && q.starts_with(data) {
            could_be_prefix = true;
        }
    }
    if could_be_prefix {
        Match::Incomplete
    } else {
        Match::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(hay: &[u8], needle: &str) -> bool {
        hay.windows(needle.len()).any(|w| w == needle.as_bytes())
    }

    #[test]
    fn answers_osc11_and_cpr_while_detached() {
        let mut f = QueryFilter::new();
        let (forward, replies) = f.process(b"\x1b]11;?\x1b\\\x1b[6n", false);
        assert!(forward.is_empty(), "probes must be stripped: {forward:?}");
        assert!(contains(
            &replies,
            &format!("\x1b]11;{DEFAULT_BG_REPORT}\x07")
        ));
        assert!(replies.ends_with(b"\x1b[1;1R"));
    }

    #[test]
    fn leaves_cpr_to_an_attached_screen() {
        let mut f = QueryFilter::new();
        let (forward, replies) = f.process(b"\x1b]11;?\x07\x1b[6n", true);
        assert_eq!(forward, b"\x1b[6n");
        assert_eq!(
            replies,
            format!("\x1b]11;{DEFAULT_BG_REPORT}\x07").into_bytes()
        );
    }

    #[test]
    fn answers_bel_terminated_query() {
        let mut f = QueryFilter::new();
        let (forward, replies) = f.process(b"\x1b]10;?\x07", false);
        assert!(forward.is_empty());
        assert_eq!(
            replies,
            format!("\x1b]10;{DEFAULT_FG_REPORT}\x07").into_bytes()
        );
    }

    #[test]
    fn passes_through_osc_titles() {
        let mut f = QueryFilter::new();
        let title = b"\x1b]2;josh@Mac:~/recur\x07";
        let (forward, replies) = f.process(title, false);
        assert!(replies.is_empty());
        assert_eq!(forward, title);
    }

    #[test]
    fn carries_split_probes() {
        let mut f = QueryFilter::new();
        let probe = b"\x1b]11;?\x1b\\";
        let (fwd1, rep1) = f.process(&probe[..3], false);
        assert!(fwd1.is_empty() && rep1.is_empty());
        let (fwd2, rep2) = f.process(&probe[3..], false);
        assert!(fwd2.is_empty());
        assert!(contains(&rep2, &format!("\x1b]11;{DEFAULT_BG_REPORT}\x07")));
    }

    #[test]
    fn preserves_surrounding_output() {
        let mut f = QueryFilter::new();
        let (forward, replies) = f.process(b"hello\x1b]11;?\x07world", false);
        assert_eq!(forward, b"helloworld");
        assert!(contains(&replies, DEFAULT_BG_REPORT));
    }

    #[test]
    fn cursor_colour() {
        let mut f = QueryFilter::new();
        let (_, replies) = f.process(b"\x1b]12;?\x07", false);
        assert_eq!(
            replies,
            format!("\x1b]12;{DEFAULT_CURSOR_REPORT}\x07").into_bytes()
        );
    }
}
