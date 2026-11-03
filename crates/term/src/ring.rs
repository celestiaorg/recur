/// Fixed-capacity circular byte buffer holding the tail of a session's raw VT output. It backs
/// replay-on-attach: the dock can be closed (or the user can switch workspaces) while a shell
/// keeps running, and re-attaching repaints the screen instead of showing an empty terminal.
///
/// It stores raw VT bytes, not decoded text, so replay carries cursor moves, colours and
/// alt-screen switches. A wrapped-around buffer can begin mid-escape-sequence; the emulator's
/// parser resyncs after discarding the partial sequence, which at this capacity costs at most a
/// few glyphs off the top of the scrollback.
pub(crate) struct Ring {
    buf: Vec<u8>,
    w: usize,
    full: bool,
}

impl Ring {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0; capacity],
            w: 0,
            full: false,
        }
    }

    /// Appends `p`, overwriting the oldest bytes once capacity is reached.
    pub(crate) fn write(&mut self, p: &[u8]) {
        let n = self.buf.len();
        if n == 0 {
            return;
        }
        // A write larger than the whole buffer can only leave its own last n bytes.
        if p.len() >= n {
            self.buf.copy_from_slice(&p[p.len() - n..]);
            self.w = 0;
            self.full = true;
            return;
        }
        let first = (n - self.w).min(p.len());
        self.buf[self.w..self.w + first].copy_from_slice(&p[..first]);
        if first < p.len() {
            self.buf[..p.len() - first].copy_from_slice(&p[first..]);
            self.full = true;
        }
        self.w += p.len();
        if self.w >= n {
            self.w -= n;
            self.full = true;
        }
    }

    /// The buffered bytes in write order, oldest first.
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        if !self.full {
            return self.buf[..self.w].to_vec();
        }
        let mut out = Vec::with_capacity(self.buf.len());
        out.extend_from_slice(&self.buf[self.w..]);
        out.extend_from_slice(&self.buf[..self.w]);
        out
    }

    /// Drops everything buffered, so a later re-attach doesn't replay output the user wiped.
    pub(crate) fn reset(&mut self) {
        self.w = 0;
        self.full = false;
    }
}

#[cfg(test)]
mod tests {
    use super::Ring;

    #[test]
    fn snapshot() {
        let cases: &[(&str, usize, &[&str], &str)] = &[
            ("empty", 8, &[], ""),
            ("under capacity", 8, &["abc"], "abc"),
            ("exactly capacity", 4, &["abcd"], "abcd"),
            ("wraps and drops oldest", 4, &["abcd", "ef"], "cdef"),
            ("many small writes", 4, &["a", "b", "c", "d", "e"], "bcde"),
            (
                "single write larger than capacity",
                4,
                &["abcdefgh"],
                "efgh",
            ),
            ("oversized write after wrap", 4, &["ab", "cdefghij"], "ghij"),
        ];
        for (name, cap, writes, want) in cases {
            let mut r = Ring::new(*cap);
            for w in *writes {
                r.write(w.as_bytes());
            }
            assert_eq!(String::from_utf8(r.snapshot()).unwrap(), *want, "{name}");
        }
    }

    #[test]
    fn reset_drops_history() {
        let mut r = Ring::new(8);
        r.write(b"abcdefghij");
        r.reset();
        assert!(r.snapshot().is_empty());
        r.write(b"xy");
        assert_eq!(r.snapshot(), b"xy");
    }

    #[test]
    fn zero_capacity() {
        let mut r = Ring::new(0);
        r.write(b"abc");
        assert!(r.snapshot().is_empty());
    }

    #[test]
    fn snapshot_is_a_copy() {
        let mut r = Ring::new(4);
        r.write(b"ab");
        let snap = r.snapshot();
        r.write(b"cd");
        assert_eq!(snap, b"ab");
    }
}
