//! The backtracking interpreter (regexp2 `executeDefault`), ported with the
//! same downward-growing track/stack/crawl arrays so every opcode's
//! push/pop/peek sequence matches the original.

use std::time::Instant;

use super::syntax::*;
use super::unicode;

/// The match ran past its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timeout;

/// Capture state (regexp2 `Match` builder).
pub(crate) struct Captures {
    pub matchcount: Vec<usize>,
    pub matches: Vec<Vec<i64>>,
    balancing: bool,
}

impl Captures {
    fn new(capcount: usize) -> Self {
        let mut matches = vec![Vec::new(); capcount];
        if let Some(m) = matches.first_mut() {
            *m = vec![0; 2];
        }
        Self {
            matchcount: vec![0; capcount],
            matches,
            balancing: false,
        }
    }

    fn reset(&mut self) {
        self.matchcount.iter_mut().for_each(|c| *c = 0);
        self.balancing = false;
    }

    fn is_matched(&self, cap: i64) -> bool {
        let Ok(cap) = usize::try_from(cap) else {
            return false;
        };
        cap < self.matchcount.len()
            && self.matchcount[cap] > 0
            && self.matches[cap][self.matchcount[cap] * 2 - 1] != (-3 + 1)
    }

    fn match_index(&self, cap: usize) -> i64 {
        let i = self.matches[cap][self.matchcount[cap] * 2 - 2];
        if i >= 0 {
            i
        } else {
            self.matches[cap][(-3 - i) as usize]
        }
    }

    fn match_length(&self, cap: usize) -> i64 {
        let i = self.matches[cap][self.matchcount[cap] * 2 - 1];
        if i >= 0 {
            i
        } else {
            self.matches[cap][(-3 - i) as usize]
        }
    }

    fn add_match(&mut self, c: usize, start: i64, l: i64) {
        if self.matches[c].is_empty() {
            self.matches[c] = vec![0; 2];
        }
        let capcount = self.matchcount[c];
        if capcount * 2 + 2 > self.matches[c].len() {
            let mut grown = vec![0; capcount * 8];
            grown[..capcount * 2].copy_from_slice(&self.matches[c][..capcount * 2]);
            self.matches[c] = grown;
        }
        self.matches[c][capcount * 2] = start;
        self.matches[c][capcount * 2 + 1] = l;
        self.matchcount[c] = capcount + 1;
    }

    fn balance_match(&mut self, c: usize) {
        self.balancing = true;
        let capcount = self.matchcount[c] as i64;
        let mut target = capcount * 2 - 2;
        if self.matches[c][target as usize] < 0 {
            target = -3 - self.matches[c][target as usize];
        }
        target -= 2;
        if target >= 0 && self.matches[c][target as usize] < 0 {
            let (a, b) = (
                self.matches[c][target as usize],
                self.matches[c][target as usize + 1],
            );
            self.add_match(c, a, b);
        } else {
            self.add_match(c, -3 - target, -4 - target);
        }
    }

    fn remove_match(&mut self, c: usize) {
        self.matchcount[c] -= 1;
    }

    /// Compacts balancing-group bookkeeping (regexp2 `Match.tidy`).
    pub(crate) fn tidy(&mut self) {
        if !self.balancing {
            return;
        }
        for cap in 0..self.matchcount.len() {
            let limit = self.matchcount[cap] * 2;
            let arr = &mut self.matches[cap];
            let mut i = 0;
            while i < limit {
                if arr[i] < 0 {
                    break;
                }
                i += 1;
            }
            let mut j = i as i64;
            while i < limit {
                if arr[i] < 0 {
                    j -= 1;
                } else {
                    if i as i64 != j {
                        arr[j as usize] = arr[i];
                    }
                    j += 1;
                }
                i += 1;
            }
            self.matchcount[cap] = (j / 2) as usize;
        }
        self.balancing = false;
    }
}

pub(crate) struct Runner<'a> {
    code: &'a Code,
    text: &'a [char],
    textend: i64,
    textstart: i64,
    textpos: i64,
    track: Vec<i64>,
    trackpos: usize,
    stack: Vec<i64>,
    stackpos: usize,
    crawl: Vec<i64>,
    crawlpos: usize,
    trackcount: usize,
    codepos: usize,
    operator: i32,
    rtl: bool,
    ci: bool,
    pub(crate) caps: Captures,
    deadline: Option<Instant>,
    steps: u32,
}

impl<'a> Runner<'a> {
    pub(crate) fn new(code: &'a Code, text: &'a [char], deadline: Option<Instant>) -> Self {
        let trackcount = code.track_count;
        let tracksize = (trackcount * 8).max(64);
        let stacksize = (trackcount * 8).max(32);
        Self {
            code,
            text,
            textend: text.len() as i64,
            textstart: 0,
            textpos: 0,
            track: vec![0; tracksize],
            trackpos: tracksize,
            stack: vec![0; stacksize],
            stackpos: stacksize,
            crawl: vec![0; 32],
            crawlpos: 32,
            trackcount,
            codepos: 0,
            operator: 0,
            rtl: false,
            ci: false,
            caps: Captures::new(code.capsize),
            deadline,
            steps: 0,
        }
    }

    /// One match attempt starting at `start` (the body of regexp2's scan
    /// loop). Returns whether group 0 matched.
    pub(crate) fn attempt(&mut self, start: usize) -> Result<bool, Timeout> {
        self.textstart = start as i64;
        self.textpos = start as i64;
        self.trackpos = self.track.len();
        self.stackpos = self.stack.len();
        self.crawlpos = self.crawl.len();
        self.caps.reset();
        self.execute()?;
        Ok(self.caps.matchcount[0] > 0)
    }

    fn ensure_storage(&mut self) {
        if self.stackpos < self.trackcount * 4 {
            double(&mut self.stack, &mut self.stackpos);
        }
        if self.trackpos < self.trackcount * 4 {
            double(&mut self.track, &mut self.trackpos);
        }
    }

    fn crawl_push(&mut self, i: i64) {
        if self.crawlpos == 0 {
            double(&mut self.crawl, &mut self.crawlpos);
        }
        self.crawlpos -= 1;
        self.crawl[self.crawlpos] = i;
    }

    fn popcrawl(&mut self) -> i64 {
        let v = self.crawl[self.crawlpos];
        self.crawlpos += 1;
        v
    }

    fn crawlpos(&self) -> i64 {
        (self.crawl.len() - self.crawlpos) as i64
    }

    fn set_operator(&mut self, op: i32) {
        self.ci = op & CI != 0;
        self.rtl = op & RTL != 0;
        self.operator = op & !(RTL | CI);
    }

    fn advance(&mut self, i: usize) {
        self.codepos += i + 1;
        self.set_operator(self.code.codes[self.codepos]);
    }

    fn goto(&mut self, newpos: usize) {
        if newpos <= self.codepos {
            self.ensure_storage();
        }
        self.set_operator(self.code.codes[newpos]);
        self.codepos = newpos;
    }

    fn trackpos_value(&self) -> i64 {
        (self.track.len() - self.trackpos) as i64
    }

    fn trackto(&mut self, newpos: i64) {
        self.trackpos = self.track.len() - newpos as usize;
    }

    fn tpush(&mut self, v: i64) {
        if self.trackpos == 0 {
            double(&mut self.track, &mut self.trackpos);
        }
        self.trackpos -= 1;
        self.track[self.trackpos] = v;
    }

    fn track_push(&mut self) {
        self.tpush(self.codepos as i64);
    }

    fn track_push1(&mut self, a: i64) {
        self.tpush(a);
        self.tpush(self.codepos as i64);
    }

    fn track_push2(&mut self, a: i64, b: i64) {
        self.tpush(a);
        self.tpush(b);
        self.tpush(self.codepos as i64);
    }

    fn track_push3(&mut self, a: i64, b: i64, c: i64) {
        self.tpush(a);
        self.tpush(b);
        self.tpush(c);
        self.tpush(self.codepos as i64);
    }

    fn track_push_neg1(&mut self, a: i64) {
        self.tpush(a);
        self.tpush(-(self.codepos as i64));
    }

    fn track_push_neg2(&mut self, a: i64, b: i64) {
        self.tpush(a);
        self.tpush(b);
        self.tpush(-(self.codepos as i64));
    }

    fn backtrack(&mut self) {
        let mut newpos = self.track[self.trackpos];
        self.trackpos += 1;
        let back = if newpos < 0 {
            newpos = -newpos;
            BACK2
        } else {
            BACK
        };
        let newpos = newpos as usize;
        self.set_operator(self.code.codes[newpos] | back);
        if newpos < self.codepos {
            self.ensure_storage();
        }
        self.codepos = newpos;
    }

    fn track_pop(&mut self) {
        self.trackpos += 1;
    }

    fn track_pop_n(&mut self, n: usize) {
        self.trackpos += n;
    }

    fn track_peek(&self) -> i64 {
        self.track[self.trackpos - 1]
    }

    fn track_peek_n(&self, i: usize) -> i64 {
        self.track[self.trackpos - i - 1]
    }

    fn spush(&mut self, v: i64) {
        if self.stackpos == 0 {
            double(&mut self.stack, &mut self.stackpos);
        }
        self.stackpos -= 1;
        self.stack[self.stackpos] = v;
    }

    fn stack_push2(&mut self, a: i64, b: i64) {
        self.spush(a);
        self.spush(b);
    }

    fn stack_pop(&mut self) {
        self.stackpos += 1;
    }

    fn stack_pop_n(&mut self, n: usize) {
        self.stackpos += n;
    }

    fn stack_peek(&self) -> i64 {
        self.stack[self.stackpos - 1]
    }

    fn stack_peek_n(&self, i: usize) -> i64 {
        self.stack[self.stackpos - i - 1]
    }

    fn operand(&self, i: usize) -> i64 {
        i64::from(self.code.codes[self.codepos + i + 1])
    }

    fn leftchars(&self) -> i64 {
        self.textpos
    }

    fn rightchars(&self) -> i64 {
        self.textend - self.textpos
    }

    fn bump(&self) -> i64 {
        if self.rtl { -1 } else { 1 }
    }

    fn forwardchars(&self) -> i64 {
        if self.rtl {
            self.textpos
        } else {
            self.textend - self.textpos
        }
    }

    fn forwardcharnext(&mut self) -> u32 {
        if self.rtl {
            self.textpos -= 1;
            self.text[self.textpos as usize] as u32
        } else {
            let ch = self.text[self.textpos as usize] as u32;
            self.textpos += 1;
            ch
        }
    }

    fn backwardnext(&mut self) {
        if self.rtl {
            self.textpos += 1;
        } else {
            self.textpos -= 1;
        }
    }

    fn char_at(&self, j: i64) -> char {
        self.text[j as usize]
    }

    fn runematch(&mut self, s: &[u32]) -> bool {
        let mut c = s.len() as i64;
        let mut pos;
        if !self.rtl {
            if self.textend - self.textpos < c {
                return false;
            }
            pos = self.textpos + c;
        } else {
            if self.textpos < c {
                return false;
            }
            pos = self.textpos;
        }
        while c != 0 {
            c -= 1;
            pos -= 1;
            let t = self.text[pos as usize];
            let t = if self.ci { unicode::to_lower(t) } else { t };
            if s[c as usize] != t as u32 {
                return false;
            }
        }
        if !self.rtl {
            pos += s.len() as i64;
        }
        self.textpos = pos;
        true
    }

    fn refmatch(&mut self, index: i64, len: i64) -> bool {
        let mut pos;
        if !self.rtl {
            if self.textend - self.textpos < len {
                return false;
            }
            pos = self.textpos + len;
        } else {
            if self.textpos < len {
                return false;
            }
            pos = self.textpos;
        }
        let mut cmpos = index + len;
        let mut c = len;
        while c != 0 {
            c -= 1;
            cmpos -= 1;
            pos -= 1;
            let (a, b) = (self.text[cmpos as usize], self.text[pos as usize]);
            let equal = if self.ci {
                unicode::to_lower(a) == unicode::to_lower(b)
            } else {
                a == b
            };
            if !equal {
                return false;
            }
        }
        if !self.rtl {
            pos += len;
        }
        self.textpos = pos;
        true
    }

    fn is_boundary(&self, index: i64) -> bool {
        (index > 0 && unicode::is_word_char(self.text[index as usize - 1]))
            != (index < self.textend && unicode::is_word_char(self.text[index as usize]))
    }

    fn capture(&mut self, capnum: usize, mut start: i64, mut end: i64) {
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        self.crawl_push(capnum as i64);
        self.caps.add_match(capnum, start, end - start);
    }

    fn transfer_capture(&mut self, capnum: i64, uncapnum: usize, mut start: i64, mut end: i64) {
        if end < start {
            std::mem::swap(&mut start, &mut end);
        }
        let start2 = self.caps.match_index(uncapnum);
        let end2 = start2 + self.caps.match_length(uncapnum);
        if start >= end2 {
            end = start;
            start = end2;
        } else if end <= start2 {
            start = start2;
        } else {
            if end > end2 {
                end = end2;
            }
            if start2 > start {
                start = start2;
            }
        }
        self.crawl_push(uncapnum as i64);
        self.caps.balance_match(uncapnum);
        if capnum != -1 {
            self.crawl_push(capnum);
            self.caps.add_match(capnum as usize, start, end - start);
        }
    }

    fn uncapture(&mut self) {
        let capnum = self.popcrawl();
        self.caps.remove_match(capnum as usize);
    }

    fn check_timeout(&mut self) -> Result<(), Timeout> {
        self.steps = self.steps.wrapping_add(1);
        if self.steps.is_multiple_of(256)
            && let Some(d) = self.deadline
            && Instant::now() >= d
        {
            return Err(Timeout);
        }
        Ok(())
    }

    fn execute(&mut self) -> Result<(), Timeout> {
        self.goto(0);
        loop {
            self.check_timeout()?;
            if self.step() {
                continue;
            }
            if self.operator == STOP {
                return Ok(());
            }
            self.backtrack();
        }
    }

    /// Runs the current operator. `true` continues at the new code
    /// position; `false` backtracks (or stops, for `Stop`).
    fn step(&mut self) -> bool {
        let op = self.operator;
        match op {
            STOP => false,
            NOTHING => false,
            GOTO => {
                self.goto(self.operand(0) as usize);
                true
            }
            TESTREF => {
                if !self.caps.is_matched(self.operand(0)) {
                    return false;
                }
                self.advance(1);
                true
            }
            LAZYBRANCH => {
                self.track_push1(self.textpos);
                self.advance(1);
                true
            }
            x if x == LAZYBRANCH | BACK => {
                self.track_pop();
                self.textpos = self.track_peek();
                self.goto(self.operand(0) as usize);
                true
            }
            SETMARK => {
                self.spush(self.textpos);
                self.track_push();
                self.advance(0);
                true
            }
            NULLMARK => {
                self.spush(-1);
                self.track_push();
                self.advance(0);
                true
            }
            x if x == SETMARK | BACK || x == NULLMARK | BACK => {
                self.stack_pop();
                false
            }
            GETMARK => {
                self.stack_pop();
                self.track_push1(self.stack_peek());
                self.textpos = self.stack_peek();
                self.advance(0);
                true
            }
            x if x == GETMARK | BACK => {
                self.track_pop();
                self.spush(self.track_peek());
                false
            }
            CAPTUREMARK => {
                if self.operand(1) != -1 && !self.caps.is_matched(self.operand(1)) {
                    return false;
                }
                self.stack_pop();
                if self.operand(1) != -1 {
                    self.transfer_capture(
                        self.operand(0),
                        self.operand(1) as usize,
                        self.stack_peek(),
                        self.textpos,
                    );
                } else {
                    self.capture(self.operand(0) as usize, self.stack_peek(), self.textpos);
                }
                self.track_push1(self.stack_peek());
                self.advance(2);
                true
            }
            x if x == CAPTUREMARK | BACK => {
                self.track_pop();
                self.spush(self.track_peek());
                self.uncapture();
                if self.operand(0) != -1 && self.operand(1) != -1 {
                    self.uncapture();
                }
                false
            }
            BRANCHMARK => {
                self.stack_pop();
                let matched = self.textpos - self.stack_peek();
                if matched != 0 {
                    self.track_push2(self.stack_peek(), self.textpos);
                    self.spush(self.textpos);
                    self.goto(self.operand(0) as usize);
                } else {
                    self.track_push_neg1(self.stack_peek());
                    self.advance(1);
                }
                true
            }
            x if x == BRANCHMARK | BACK => {
                self.track_pop_n(2);
                self.stack_pop();
                self.textpos = self.track_peek_n(1);
                self.track_push_neg1(self.track_peek());
                self.advance(1);
                true
            }
            x if x == BRANCHMARK | BACK2 => {
                self.track_pop();
                self.spush(self.track_peek());
                false
            }
            LAZYBRANCHMARK => {
                self.stack_pop();
                let old_mark = self.stack_peek();
                if self.textpos != old_mark {
                    if old_mark != -1 {
                        self.track_push2(old_mark, self.textpos);
                    } else {
                        self.track_push2(self.textpos, self.textpos);
                    }
                } else {
                    self.track_push_neg2(old_mark, 0);
                }
                self.advance(1);
                true
            }
            x if x == LAZYBRANCHMARK | BACK => {
                self.track_pop_n(2);
                let pos = self.track_peek_n(1);
                self.track_push_neg2(self.track_peek(), 1);
                self.spush(pos);
                self.textpos = pos;
                self.goto(self.operand(0) as usize);
                true
            }
            x if x == LAZYBRANCHMARK | BACK2 => {
                self.track_pop_n(2);
                let old_mark = self.track_peek();
                let needs_pop = self.track_peek_n(1);
                if needs_pop != 0 {
                    self.stack_pop();
                }
                self.spush(old_mark);
                false
            }
            SETCOUNT => {
                self.stack_push2(self.textpos, self.operand(0));
                self.track_push();
                self.advance(1);
                true
            }
            NULLCOUNT => {
                self.stack_push2(-1, self.operand(0));
                self.track_push();
                self.advance(1);
                true
            }
            x if x == SETCOUNT | BACK || x == NULLCOUNT | BACK => {
                self.stack_pop_n(2);
                false
            }
            BRANCHCOUNT => {
                self.stack_pop_n(2);
                let mark = self.stack_peek();
                let count = self.stack_peek_n(1);
                let matched = self.textpos - mark;
                if count >= self.operand(1) || (matched == 0 && count >= 0) {
                    self.track_push_neg2(mark, count);
                    self.advance(2);
                } else {
                    self.track_push1(mark);
                    self.stack_push2(self.textpos, count + 1);
                    self.goto(self.operand(0) as usize);
                }
                true
            }
            x if x == BRANCHCOUNT | BACK => {
                self.track_pop();
                self.stack_pop_n(2);
                if self.stack_peek_n(1) > 0 {
                    self.textpos = self.stack_peek();
                    self.track_push_neg2(self.track_peek(), self.stack_peek_n(1) - 1);
                    self.advance(2);
                    return true;
                }
                self.stack_push2(self.track_peek(), self.stack_peek_n(1) - 1);
                false
            }
            x if x == BRANCHCOUNT | BACK2 => {
                self.track_pop_n(2);
                self.stack_push2(self.track_peek(), self.track_peek_n(1));
                false
            }
            LAZYBRANCHCOUNT => {
                self.stack_pop_n(2);
                let mark = self.stack_peek();
                let count = self.stack_peek_n(1);
                if count < 0 {
                    self.track_push_neg1(mark);
                    self.stack_push2(self.textpos, count + 1);
                    self.goto(self.operand(0) as usize);
                } else {
                    self.track_push3(mark, count, self.textpos);
                    self.advance(2);
                }
                true
            }
            x if x == LAZYBRANCHCOUNT | BACK => {
                self.track_pop_n(3);
                let mark = self.track_peek();
                let textpos = self.track_peek_n(2);
                if self.track_peek_n(1) < self.operand(1) && textpos != mark {
                    self.textpos = textpos;
                    self.stack_push2(textpos, self.track_peek_n(1) + 1);
                    self.track_push_neg1(mark);
                    self.goto(self.operand(0) as usize);
                    return true;
                }
                self.stack_push2(self.track_peek(), self.track_peek_n(1));
                false
            }
            x if x == LAZYBRANCHCOUNT | BACK2 => {
                self.track_pop();
                self.stack_pop_n(2);
                self.stack_push2(self.track_peek(), self.stack_peek_n(1) - 1);
                false
            }
            SETJUMP => {
                self.stack_push2(self.trackpos_value(), self.crawlpos());
                self.track_push();
                self.advance(0);
                true
            }
            x if x == SETJUMP | BACK => {
                self.stack_pop_n(2);
                false
            }
            BACKJUMP => {
                self.stack_pop_n(2);
                self.trackto(self.stack_peek());
                while self.crawlpos() != self.stack_peek_n(1) {
                    self.uncapture();
                }
                false
            }
            FOREJUMP => {
                self.stack_pop_n(2);
                self.trackto(self.stack_peek());
                self.track_push1(self.stack_peek_n(1));
                self.advance(0);
                true
            }
            x if x == FOREJUMP | BACK => {
                self.track_pop();
                while self.crawlpos() != self.track_peek() {
                    self.uncapture();
                }
                false
            }
            BOL => {
                if self.leftchars() > 0 && self.char_at(self.textpos - 1) != '\n' {
                    return false;
                }
                self.advance(0);
                true
            }
            EOL => {
                if self.rightchars() > 0 && self.char_at(self.textpos) != '\n' {
                    return false;
                }
                self.advance(0);
                true
            }
            BOUNDARY | NONBOUNDARY => {
                if self.is_boundary(self.textpos) != (op == BOUNDARY) {
                    return false;
                }
                self.advance(0);
                true
            }
            BEGINNING => {
                if self.leftchars() > 0 {
                    return false;
                }
                self.advance(0);
                true
            }
            START => {
                if self.textpos != self.textstart {
                    return false;
                }
                self.advance(0);
                true
            }
            ENDZ => {
                let rchars = self.rightchars();
                if rchars > 1 || (rchars == 1 && self.char_at(self.textpos) != '\n') {
                    return false;
                }
                self.advance(0);
                true
            }
            END => {
                if self.rightchars() > 0 {
                    return false;
                }
                self.advance(0);
                true
            }
            ONE => {
                if self.forwardchars() < 1 || i64::from(self.forwardcharnext()) != self.operand(0) {
                    return false;
                }
                self.advance(1);
                true
            }
            NOTONE => {
                if self.forwardchars() < 1 || i64::from(self.forwardcharnext()) == self.operand(0) {
                    return false;
                }
                self.advance(1);
                true
            }
            SET => {
                if self.forwardchars() < 1 {
                    return false;
                }
                let ch = self.forwardcharnext();
                if !self.set_has(self.operand(0), ch) {
                    return false;
                }
                self.advance(1);
                true
            }
            MULTI => {
                let code = self.code;
                if !self.runematch(&code.strings[self.operand(0) as usize]) {
                    return false;
                }
                self.advance(1);
                true
            }
            REF => {
                let capnum = self.operand(0);
                if self.caps.is_matched(capnum) {
                    let cap = capnum as usize;
                    let (idx, len) = (self.caps.match_index(cap), self.caps.match_length(cap));
                    if !self.refmatch(idx, len) {
                        return false;
                    }
                } else {
                    return false;
                }
                self.advance(1);
                true
            }
            ONEREP | NOTONEREP | SETREP => {
                let mut c = self.operand(1);
                if self.forwardchars() < c {
                    return false;
                }
                let target = self.operand(0);
                while c > 0 {
                    let ch = self.forwardcharnext();
                    let ok = match op {
                        ONEREP => i64::from(ch) == target,
                        NOTONEREP => i64::from(ch) != target,
                        _ => self.set_has(target, ch),
                    };
                    if !ok {
                        return false;
                    }
                    c -= 1;
                }
                self.advance(2);
                true
            }
            ONELOOP | NOTONELOOP | SETLOOP => {
                let c = self.operand(1).min(self.forwardchars());
                let target = self.operand(0);
                let mut i = c;
                while i > 0 {
                    let ch = self.forwardcharnext();
                    let ok = match op {
                        ONELOOP => i64::from(ch) == target,
                        NOTONELOOP => i64::from(ch) != target,
                        _ => self.set_has(target, ch),
                    };
                    if !ok {
                        self.backwardnext();
                        break;
                    }
                    i -= 1;
                }
                if c > i {
                    self.track_push2(c - i - 1, self.textpos - self.bump());
                }
                self.advance(2);
                true
            }
            x if x == ONELOOP | BACK || x == NOTONELOOP | BACK || x == SETLOOP | BACK => {
                self.track_pop_n(2);
                let i = self.track_peek();
                let pos = self.track_peek_n(1);
                self.textpos = pos;
                if i > 0 {
                    self.track_push2(i - 1, pos - self.bump());
                }
                self.advance(2);
                true
            }
            ONELAZY | NOTONELAZY | SETLAZY => {
                let c = self.operand(1).min(self.forwardchars());
                if c > 0 {
                    self.track_push2(c - 1, self.textpos);
                }
                self.advance(2);
                true
            }
            x if x == ONELAZY | BACK || x == NOTONELAZY | BACK || x == SETLAZY | BACK => {
                self.track_pop_n(2);
                let pos = self.track_peek_n(1);
                self.textpos = pos;
                let ch = self.forwardcharnext();
                let target = self.operand(0);
                let ok = match op & !BACK {
                    ONELAZY => i64::from(ch) == target,
                    NOTONELAZY => i64::from(ch) != target,
                    _ => self.set_has(target, ch),
                };
                if !ok {
                    return false;
                }
                let i = self.track_peek();
                if i > 0 {
                    self.track_push2(i - 1, pos + self.bump());
                }
                self.advance(2);
                true
            }
            _ => false,
        }
    }

    fn set_has(&self, set: i64, ch: u32) -> bool {
        char::from_u32(ch).is_some_and(|c| self.code.sets[set as usize].char_in(c))
    }
}

/// regexp2 `doubleIntSlice`: doubles a downward-growing stack, keeping its
/// contents at the top.
fn double(s: &mut Vec<i64>, pos: &mut usize) {
    let old_len = s.len();
    let mut grown = vec![0; old_len * 2];
    grown[old_len..].copy_from_slice(s);
    *pos += old_len;
    *s = grown;
}
