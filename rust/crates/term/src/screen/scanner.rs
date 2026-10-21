//! A lexer over PTY output that finds the control sequences the screen answers or tracks itself
//! (xterm.js's reports, mouse modes, cursor style), and passes every other byte through to the
//! emulator unchanged and in order.

/// A complete CSI sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Csi {
    pub(crate) private: Option<u8>,
    pub(crate) params: Vec<i64>,
    pub(crate) intermediates: Vec<u8>,
    pub(crate) final_byte: u8,
    pub(crate) raw: Vec<u8>,
}

impl Csi {
    pub(crate) fn param(&self, i: usize) -> i64 {
        self.params.get(i).copied().unwrap_or(0)
    }

    fn parse(raw: Vec<u8>) -> Self {
        let body = &raw[2..raw.len() - 1];
        let final_byte = raw[raw.len() - 1];
        let (private, body) = match body.first() {
            Some(&p @ (b'<' | b'=' | b'>' | b'?')) => (Some(p), &body[1..]),
            _ => (None, body),
        };
        let split = body
            .iter()
            .position(|b| (0x20..=0x2f).contains(b))
            .unwrap_or(body.len());
        let (param_bytes, intermediates) = body.split_at(split);
        let params = if param_bytes.is_empty() {
            Vec::new()
        } else {
            param_bytes
                .split(|&b| b == b';')
                .map(|p| {
                    let p = p.split(|&b| b == b':').next().unwrap_or(&[]);
                    p.iter().filter(|b| b.is_ascii_digit()).fold(0i64, |n, b| {
                        (n * 10 + (b - b'0') as i64).min(i64::from(i32::MAX))
                    })
                })
                .collect()
        };
        Csi {
            private,
            params,
            intermediates: intermediates.to_vec(),
            final_byte,
            raw,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Item {
    /// Bytes for the emulator.
    Forward(Vec<u8>),
    Csi(Csi),
    /// `ESC` + final byte with no intermediates (RIS is `ESC c`).
    Esc {
        final_byte: u8,
        raw: Vec<u8>,
    },
    /// `DCS $ q Pt ST` (DECRQSS) payload.
    Decrqss(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    EscInter,
    Csi,
    CsiIgnore,
    Osc,
    OscEsc,
    DcsHeader,
    DcsPass,
    DcsPassEsc,
    Decrqss,
    DecrqssEsc,
    Str,
    StrEsc,
}

const MAX_CSI: usize = 512;
const MAX_DECRQSS: usize = 1024;

pub(crate) struct Scanner {
    state: State,
    hold: Vec<u8>,
    payload: Vec<u8>,
}

impl Scanner {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Ground,
            hold: Vec::new(),
            payload: Vec::new(),
        }
    }

    pub(crate) fn feed(&mut self, input: &[u8]) -> Vec<Item> {
        let mut out = Vec::new();
        let mut run = Vec::new();
        for &b in input {
            self.step(b, &mut run, &mut out);
        }
        flush(&mut run, &mut out);
        out
    }

    fn step(&mut self, b: u8, run: &mut Vec<u8>, out: &mut Vec<Item>) {
        const ESC: u8 = 0x1b;
        let cancel = b == 0x18 || b == 0x1a;
        match self.state {
            State::Ground => {
                if b == ESC {
                    self.begin_escape();
                } else {
                    run.push(b);
                }
            }
            State::Esc => {
                self.hold.push(b);
                match b {
                    b'[' => self.state = State::Csi,
                    b'P' => self.state = State::DcsHeader,
                    b']' => self.pass_held(run, State::Osc),
                    b'X' | b'^' | b'_' => self.pass_held(run, State::Str),
                    0x20..=0x2f => self.state = State::EscInter,
                    ESC => {
                        self.hold.pop();
                        run.append(&mut self.hold);
                        self.begin_escape();
                    }
                    0x00..=0x1f => self.pass_held(run, State::Ground),
                    _ => {
                        flush(run, out);
                        out.push(Item::Esc {
                            final_byte: b,
                            raw: std::mem::take(&mut self.hold),
                        });
                        self.state = State::Ground;
                    }
                }
            }
            State::EscInter => {
                self.hold.push(b);
                if !(0x20..=0x2f).contains(&b) {
                    self.pass_held(run, State::Ground);
                }
            }
            State::Csi => {
                if b == ESC {
                    run.append(&mut self.hold);
                    self.begin_escape();
                    return;
                }
                self.hold.push(b);
                if cancel {
                    self.pass_held(run, State::Ground);
                } else if (0x40..=0x7e).contains(&b) {
                    flush(run, out);
                    out.push(Item::Csi(Csi::parse(std::mem::take(&mut self.hold))));
                    self.state = State::Ground;
                } else if self.hold.len() > MAX_CSI {
                    self.pass_held(run, State::CsiIgnore);
                }
            }
            State::CsiIgnore => {
                if b == ESC {
                    self.begin_escape();
                } else {
                    run.push(b);
                    if cancel || (0x40..=0x7e).contains(&b) {
                        self.state = State::Ground;
                    }
                }
            }
            State::Osc | State::Str => match b {
                ESC => {
                    self.hold.push(b);
                    self.state = if self.state == State::Osc {
                        State::OscEsc
                    } else {
                        State::StrEsc
                    };
                }
                0x07 if self.state == State::Osc => {
                    run.push(b);
                    self.state = State::Ground;
                }
                _ => {
                    run.push(b);
                    if cancel {
                        self.state = State::Ground;
                    }
                }
            },
            State::OscEsc | State::StrEsc | State::DcsPassEsc => {
                if b == b'\\' {
                    self.hold.push(b);
                    self.pass_held(run, State::Ground);
                } else {
                    // ESC ends the string and starts a new escape sequence.
                    self.state = State::Esc;
                    self.step(b, run, out);
                }
            }
            State::DcsHeader => {
                if b == ESC {
                    run.append(&mut self.hold);
                    self.begin_escape();
                    return;
                }
                self.hold.push(b);
                if cancel {
                    self.pass_held(run, State::Ground);
                } else if (0x40..=0x7e).contains(&b) {
                    if self.hold == b"\x1bP$q" {
                        self.payload.clear();
                        self.state = State::Decrqss;
                    } else {
                        self.pass_held(run, State::DcsPass);
                    }
                } else if self.hold.len() > MAX_CSI {
                    self.pass_held(run, State::DcsPass);
                }
            }
            State::DcsPass => match b {
                ESC => {
                    self.hold.push(b);
                    self.state = State::DcsPassEsc;
                }
                _ => {
                    run.push(b);
                    if cancel {
                        self.state = State::Ground;
                    }
                }
            },
            State::Decrqss => match b {
                ESC => self.state = State::DecrqssEsc,
                _ if cancel => {
                    self.hold.clear();
                    self.payload.clear();
                    self.state = State::Ground;
                }
                _ => {
                    self.payload.push(b);
                    if self.payload.len() > MAX_DECRQSS {
                        run.append(&mut self.hold);
                        run.append(&mut self.payload);
                        self.state = State::DcsPass;
                    }
                }
            },
            State::DecrqssEsc => {
                if b == b'\\' {
                    flush(run, out);
                    self.hold.clear();
                    out.push(Item::Decrqss(std::mem::take(&mut self.payload)));
                    self.state = State::Ground;
                } else {
                    // An aborted DCS produces no report.
                    self.hold.clear();
                    self.payload.clear();
                    self.begin_escape();
                    self.step(b, run, out);
                }
            }
        }
    }

    fn begin_escape(&mut self) {
        self.hold.clear();
        self.hold.push(0x1b);
        self.state = State::Esc;
    }

    fn pass_held(&mut self, run: &mut Vec<u8>, next: State) {
        run.append(&mut self.hold);
        self.state = next;
    }
}

fn flush(run: &mut Vec<u8>, out: &mut Vec<Item>) {
    if !run.is_empty() {
        out.push(Item::Forward(std::mem::take(run)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forwarded(items: &[Item]) -> Vec<u8> {
        items
            .iter()
            .flat_map(|i| match i {
                Item::Forward(b) => b.clone(),
                Item::Csi(c) => c.raw.clone(),
                Item::Esc { raw, .. } => raw.clone(),
                Item::Decrqss(_) => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn passes_plain_text_and_osc_untouched() {
        let mut s = Scanner::new();
        let input = b"hi \x1b]0;title\x07 there \x1b]8;;http://x\x1b\\link";
        let items = s.feed(input);
        assert_eq!(forwarded(&items), input);
        assert!(items.iter().all(|i| matches!(i, Item::Forward(_))));
    }

    #[test]
    fn lexes_csi_with_private_marker_and_intermediates() {
        let mut s = Scanner::new();
        let items = s.feed(b"a\x1b[?1000;1006hb\x1b[2 q\x1b[?25$p");
        let csis: Vec<&Csi> = items
            .iter()
            .filter_map(|i| if let Item::Csi(c) = i { Some(c) } else { None })
            .collect();
        assert_eq!(csis.len(), 3);
        assert_eq!(
            (csis[0].private, csis[0].params.clone(), csis[0].final_byte),
            (Some(b'?'), vec![1000, 1006], b'h')
        );
        assert_eq!(
            (
                csis[1].intermediates.clone(),
                csis[1].param(0),
                csis[1].final_byte
            ),
            (b" ".to_vec(), 2, b'q')
        );
        assert_eq!(
            (
                csis[2].private,
                csis[2].intermediates.clone(),
                csis[2].final_byte
            ),
            (Some(b'?'), b"$".to_vec(), b'p')
        );
    }

    #[test]
    fn carries_split_sequences_across_feeds() {
        let mut s = Scanner::new();
        assert_eq!(s.feed(b"x\x1b["), vec![Item::Forward(b"x".to_vec())]);
        let items = s.feed(b">c");
        assert!(
            matches!(&items[0], Item::Csi(c) if c.private == Some(b'>') && c.final_byte == b'c')
        );
    }

    #[test]
    fn extracts_decrqss() {
        let mut s = Scanner::new();
        let items = s.feed(b"\x1bP$q q\x1b\\after");
        assert_eq!(items[0], Item::Decrqss(b" q".to_vec()));
        assert_eq!(items[1], Item::Forward(b"after".to_vec()));
    }

    #[test]
    fn other_dcs_passes_through() {
        let mut s = Scanner::new();
        let input = b"\x1bPq#0;2;0;0;0#0!10~\x1b\\";
        assert_eq!(forwarded(&s.feed(input)), input);
    }

    #[test]
    fn esc_inside_osc_starts_a_new_sequence() {
        let mut s = Scanner::new();
        let items = s.feed(b"\x1b]0;t\x1b[c");
        assert_eq!(items[0], Item::Forward(b"\x1b]0;t".to_vec()));
        assert!(matches!(&items[1], Item::Csi(c) if c.final_byte == b'c'));
    }

    #[test]
    fn reports_ris() {
        let mut s = Scanner::new();
        assert!(matches!(
            &s.feed(b"\x1bc")[0],
            Item::Esc {
                final_byte: b'c',
                ..
            }
        ));
        assert_eq!(forwarded(&s.feed(b"\x1b(B\x1b=")), b"\x1b(B\x1b=");
    }
}
