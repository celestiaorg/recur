//! Text diffing: `@codemirror/merge`'s `diff` / `presentableDiff` (Myers
//! with half-match splitting, scan limits and timeouts) and Recur's
//! line-oriented diff.
//!
//! The algorithm runs on UTF-16 code units like the JavaScript original,
//! so hunk boundaries and the imprecise fallbacks land in the same places;
//! [`changes_to_bytes`] maps results back to document offsets.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::text::Text;

/// A changed range: `from_a..to_a` in A became `from_b..to_b` in B.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiffChange {
    pub from_a: usize,
    pub to_a: usize,
    pub from_b: usize,
    pub to_b: usize,
}

impl DiffChange {
    pub fn new(from_a: usize, to_a: usize, from_b: usize, to_b: usize) -> DiffChange {
        DiffChange {
            from_a,
            to_a,
            from_b,
            to_b,
        }
    }

    fn offset(&self, off_a: isize, off_b: isize) -> DiffChange {
        DiffChange {
            from_a: self.from_a.wrapping_add_signed(off_a),
            to_a: self.to_a.wrapping_add_signed(off_a),
            from_b: self.from_b.wrapping_add_signed(off_b),
            to_b: self.to_b.wrapping_add_signed(off_b),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffConfig {
    pub scan_limit: Option<usize>,
    pub timeout: Option<Duration>,
}

/// `defaultDiffConfig`.
pub const DEFAULT_DIFF_CONFIG: DiffConfig = DiffConfig {
    scan_limit: Some(500),
    timeout: None,
};

thread_local! {
    static CRUDE: Cell<bool> = const { Cell::new(false) };
}

/// `diffIsPrecise`: whether the last diff run stayed exact.
pub fn diff_is_precise() -> bool {
    !CRUDE.with(Cell::get)
}

struct Frontier {
    vec: Vec<isize>,
    len: usize,
    start: isize,
    end: isize,
}

impl Frontier {
    fn new() -> Frontier {
        Frontier {
            vec: Vec::new(),
            len: 0,
            start: 0,
            end: 0,
        }
    }

    fn reset(&mut self, off: usize) {
        self.len = off << 1;
        self.vec.clear();
        self.vec.resize(self.len.max(off + 2), -1);
        self.vec[off + 1] = 0;
        self.start = 0;
        self.end = 0;
    }

    #[allow(clippy::too_many_arguments)]
    fn advance(
        &mut self,
        depth: isize,
        len_x: isize,
        len_y: isize,
        v_off: isize,
        other: Option<&Frontier>,
        from_back: bool,
        matches: &dyn Fn(isize, isize) -> bool,
    ) -> Option<(isize, isize)> {
        let mut k = -depth + self.start;
        while k <= depth - self.end {
            let off = (v_off + k) as usize;
            let get = |i: usize| self.vec.get(i).copied().unwrap_or(-1);
            let mut x = if k == -depth || (k != depth && get(off - 1) < get(off + 1)) {
                get(off + 1)
            } else {
                get(off - 1) + 1
            };
            let mut y = x - k;
            while x < len_x && y < len_y && matches(x, y) {
                x += 1;
                y += 1;
            }
            if off >= self.vec.len() {
                self.vec.resize(off + 1, -1);
            }
            self.vec[off] = x;
            if x > len_x {
                self.end += 2;
            } else if y > len_y {
                self.start += 2;
            } else if let Some(other) = other {
                let off_other = v_off + (len_x - len_y) - k;
                if off_other >= 0
                    && (off_other as usize) < self.len
                    && other.vec.get(off_other as usize).copied().unwrap_or(-1) != -1
                {
                    let ov = other.vec[off_other as usize];
                    if !from_back {
                        let x_other = len_x - ov;
                        if x >= x_other {
                            return Some((x, y));
                        }
                    } else if ov >= len_x - x {
                        return Some((ov, v_off + ov - off_other));
                    }
                }
            }
            k += 2;
        }
        None
    }
}

fn is_surrogate1(c: u16) -> bool {
    (0xD800..=0xDBFF).contains(&c)
}

fn is_surrogate2(c: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&c)
}

/// `validIndex`: not between the halves of a surrogate pair.
fn valid_index(s: &[u16], index: usize) -> bool {
    index == 0 || index >= s.len() || !is_surrogate1(s[index - 1]) || !is_surrogate2(s[index])
}

fn index_of(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= hay.len()).then_some(from);
    }
    if needle.len() > hay.len() {
        return None;
    }
    let first = needle[0];
    let last = hay.len() - needle.len();
    let mut i = from;
    while i <= last {
        if hay[i] == first && hay[i..i + needle.len()] == *needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn chunk_size(len_a: usize, len_b: usize) -> usize {
    let max = len_a.min(len_b);
    let mut size = 1;
    while size < max {
        size <<= 1;
    }
    size
}

fn common_prefix(
    a: &[u16],
    from_a: usize,
    to_a: usize,
    b: &[u16],
    from_b: usize,
    to_b: usize,
) -> usize {
    if from_a == to_a || from_a == to_b || a.get(from_a) != b.get(from_b) {
        return 0;
    }
    let mut chunk = chunk_size(to_a - from_a, to_b.saturating_sub(from_b));
    let (mut pa, mut pb) = (from_a, from_b);
    loop {
        let (end_a, end_b) = (pa + chunk, pb + chunk);
        if end_a > to_a || end_b > to_b || a[pa..end_a] != b[pb..end_b] {
            if chunk == 1 {
                return pa - from_a - usize::from(!valid_index(a, pa));
            }
            chunk >>= 1;
        } else if end_a == to_a || end_b == to_b {
            return end_a - from_a;
        } else {
            pa = end_a;
            pb = end_b;
        }
    }
}

fn common_suffix(
    a: &[u16],
    from_a: usize,
    to_a: usize,
    b: &[u16],
    from_b: usize,
    to_b: usize,
) -> usize {
    if from_a == to_a || from_b == to_b || a[to_a - 1] != b[to_b - 1] {
        return 0;
    }
    let mut chunk = chunk_size(to_a - from_a, to_b - from_b);
    let (mut pa, mut pb) = (to_a, to_b);
    loop {
        let sa = pa as isize - chunk as isize;
        let sb = pb as isize - chunk as isize;
        if sa < from_a as isize || sb < from_b as isize || a[sa as usize..pa] != b[sb as usize..pb]
        {
            if chunk == 1 {
                return to_a - pa - usize::from(!valid_index(a, pa));
            }
            chunk >>= 1;
        } else if sa as usize == from_a || sb as usize == from_b {
            return to_a - sa as usize;
        } else {
            pa = sa as usize;
            pb = sb as usize;
        }
    }
}

struct Differ<'a> {
    a: &'a [u16],
    b: &'a [u16],
    scan_limit: usize,
    deadline: Option<Instant>,
    f1: Frontier,
    f2: Frontier,
}

const NO_LIMIT: usize = 1_000_000_000;

impl Differ<'_> {
    fn timed_out(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() > d)
    }

    fn find_diff(
        &mut self,
        mut from_a: usize,
        mut to_a: usize,
        mut from_b: usize,
        mut to_b: usize,
    ) -> Vec<DiffChange> {
        let (a, b) = (self.a, self.b);
        let prefix = common_prefix(a, from_a, to_a, b, from_b, to_b);
        let suffix = common_suffix(a, from_a + prefix, to_a, b, from_b + prefix, to_b);
        from_a += prefix;
        to_a -= suffix;
        from_b += prefix;
        to_b -= suffix;
        let len_a = to_a - from_a;
        let len_b = to_b - from_b;
        if len_a == 0 || len_b == 0 {
            return vec![DiffChange::new(from_a, to_a, from_b, to_b)];
        }
        if len_a > len_b {
            if let Some(found) = index_of(&a[from_a..to_a], &b[from_b..to_b], 0) {
                return vec![
                    DiffChange::new(from_a, from_a + found, from_b, from_b),
                    DiffChange::new(from_a + found + len_b, to_a, to_b, to_b),
                ];
            }
        } else if len_b > len_a
            && let Some(found) = index_of(&b[from_b..to_b], &a[from_a..to_a], 0)
        {
            return vec![
                DiffChange::new(from_a, from_a, from_b, from_b + found),
                DiffChange::new(to_a, to_a, from_b + found + len_a, to_b),
            ];
        }
        if len_a == 1 || len_b == 1 {
            return vec![DiffChange::new(from_a, to_a, from_b, to_b)];
        }
        if let Some((shared_a, shared_b, shared_len)) = half_match(a, from_a, to_a, b, from_b, to_b)
        {
            let mut out = self.find_diff(from_a, shared_a, from_b, shared_b);
            out.extend(self.find_diff(shared_a + shared_len, to_a, shared_b + shared_len, to_b));
            return out;
        }
        self.find_snake(from_a, to_a, from_b, to_b)
    }

    fn find_snake(
        &mut self,
        from_a: usize,
        to_a: usize,
        from_b: usize,
        to_b: usize,
    ) -> Vec<DiffChange> {
        let (a, b) = (self.a, self.b);
        let len_a = to_a - from_a;
        let len_b = to_b - from_b;
        if (self.scan_limit < NO_LIMIT && len_a.min(len_b) > self.scan_limit * 16)
            || self.timed_out()
        {
            if len_a.min(len_b) > self.scan_limit.saturating_mul(64) {
                return vec![DiffChange::new(from_a, to_a, from_b, to_b)];
            }
            return self.crude_match(from_a, to_a, from_b, to_b);
        }
        let off = (len_a + len_b).div_ceil(2);
        self.f1.reset(off);
        self.f2.reset(off);
        let match1 = |x: isize, y: isize| a[from_a + x as usize] == b[from_b + y as usize];
        let match2 = |x: isize, y: isize| a[to_a - x as usize - 1] == b[to_b - y as usize - 1];
        let odd = (len_a as isize - len_b as isize) % 2 != 0;
        for depth in 0..off as isize {
            if depth as usize > self.scan_limit || (depth & 63 == 0 && self.timed_out()) {
                return self.crude_match(from_a, to_a, from_b, to_b);
            }
            let done = {
                let (f1, f2) = (&mut self.f1, &self.f2);
                f1.advance(
                    depth,
                    len_a as isize,
                    len_b as isize,
                    off as isize,
                    odd.then_some(f2),
                    false,
                    &match1,
                )
            }
            .or_else(|| {
                let (f2, f1) = (&mut self.f2, &self.f1);
                f2.advance(
                    depth,
                    len_a as isize,
                    len_b as isize,
                    off as isize,
                    (!odd).then_some(f1),
                    true,
                    &match2,
                )
            });
            if let Some((x, y)) = done {
                return self.bisect(
                    from_a,
                    to_a,
                    from_a + x as usize,
                    from_b,
                    to_b,
                    from_b + y as usize,
                );
            }
        }
        vec![DiffChange::new(from_a, to_a, from_b, to_b)]
    }

    fn bisect(
        &mut self,
        from_a: usize,
        to_a: usize,
        mut split_a: usize,
        from_b: usize,
        to_b: usize,
        mut split_b: usize,
    ) -> Vec<DiffChange> {
        let mut stop = false;
        if !valid_index(self.a, split_a) {
            split_a += 1;
            if split_a == to_a {
                stop = true;
            }
        }
        if !valid_index(self.b, split_b) {
            split_b += 1;
            if split_b == to_b {
                stop = true;
            }
        }
        if stop {
            return vec![DiffChange::new(from_a, to_a, from_b, to_b)];
        }
        let mut out = self.find_diff(from_a, split_a, from_b, split_b);
        out.extend(self.find_diff(split_a, to_a, split_b, to_b));
        out
    }

    fn crude_match(
        &mut self,
        from_a: usize,
        to_a: usize,
        from_b: usize,
        to_b: usize,
    ) -> Vec<DiffChange> {
        CRUDE.with(|c| c.set(true));
        let (a, b) = (self.a, self.b);
        let len_a = to_a - from_a;
        let len_b = to_b - from_b;
        let result = if len_a < len_b {
            find_match(b, from_b, to_b, a, from_a, to_a, len_a / 6, 50).map(|(x, y, l)| (y, x, l))
        } else {
            find_match(a, from_a, to_a, b, from_b, to_b, len_b / 6, 50)
        };
        let Some((shared_a, shared_b, shared_len)) = result else {
            return vec![DiffChange::new(from_a, to_a, from_b, to_b)];
        };
        let mut out = self.find_diff(from_a, shared_a, from_b, shared_b);
        out.extend(self.find_diff(shared_a + shared_len, to_a, shared_b + shared_len, to_b));
        out
    }
}

/// `a` assumed longer than `b`.
#[allow(clippy::too_many_arguments)]
fn find_match(
    a: &[u16],
    from_a: usize,
    to_a: usize,
    b: &[u16],
    from_b: usize,
    to_b: usize,
    mut size: usize,
    divide_to: isize,
) -> Option<(usize, usize, usize)> {
    let range_b = &b[from_b..to_b];
    let mut best: Option<(usize, usize, usize)> = None;
    loop {
        if best.is_some() || (size as isize) < divide_to {
            return best;
        }
        let mut start = from_a + size;
        loop {
            if !valid_index(a, start) {
                start += 1;
            }
            let mut end = start + size;
            if !valid_index(a, end) {
                if end == start + 1 {
                    end += 1;
                } else {
                    end -= 1;
                }
            }
            if end >= to_a {
                break;
            }
            let seed = &a[start..end];
            let mut found: isize = -1;
            while let Some(f) = index_of(range_b, seed, (found + 1) as usize) {
                found = f as isize;
                let prefix_after = common_prefix(a, end, to_a, b, from_b + f + seed.len(), to_b);
                let suffix_before = common_suffix(a, from_a, start, b, from_b, from_b + f);
                let length = seed.len() + prefix_after + suffix_before;
                if best.is_none_or(|bst| bst.2 < length) {
                    best = Some((start - suffix_before, from_b + f - suffix_before, length));
                }
            }
            start = end;
        }
        if divide_to < 0 {
            return best;
        }
        size >>= 1;
    }
}

fn half_match(
    a: &[u16],
    from_a: usize,
    to_a: usize,
    b: &[u16],
    from_b: usize,
    to_b: usize,
) -> Option<(usize, usize, usize)> {
    let len_a = to_a - from_a;
    let len_b = to_b - from_b;
    if len_a < len_b {
        return half_match(b, from_b, to_b, a, from_a, to_a).map(|(x, y, l)| (y, x, l));
    }
    if len_a < 4 || len_b * 2 < len_a {
        return None;
    }
    find_match(a, from_a, to_a, b, from_b, to_b, len_a / 4, -1)
}

fn merge_adjacent(changes: &mut Vec<DiffChange>, min_gap: usize) {
    let mut i = 1;
    while i < changes.len() {
        let prev = changes[i - 1];
        let cur = changes[i];
        if prev.to_a + min_gap > cur.from_a && prev.to_b + min_gap > cur.from_b {
            changes[i - 1] = DiffChange::new(prev.from_a, cur.to_a, prev.from_b, cur.to_b);
            changes.remove(i);
        } else {
            i += 1;
        }
    }
}

fn normalize(a: &[u16], b: &[u16], mut changes: Vec<DiffChange>) -> Vec<DiffChange> {
    loop {
        merge_adjacent(&mut changes, 1);
        let mut moved = false;
        for i in 0..changes.len() {
            let mut ch = changes[i];
            let pre = common_prefix(a, ch.from_a, ch.to_a, b, ch.from_b, ch.to_b);
            if pre > 0 {
                ch = DiffChange::new(ch.from_a + pre, ch.to_a, ch.from_b + pre, ch.to_b);
                changes[i] = ch;
            }
            let post = common_suffix(a, ch.from_a, ch.to_a, b, ch.from_b, ch.to_b);
            if post > 0 {
                ch = DiffChange::new(ch.from_a, ch.to_a - post, ch.from_b, ch.to_b - post);
                changes[i] = ch;
            }
            let len_a = ch.to_a - ch.from_a;
            let len_b = ch.to_b - ch.from_b;
            if len_a > 0 && len_b > 0 {
                continue;
            }
            let before_len = ch.from_a - if i > 0 { changes[i - 1].to_a } else { 0 };
            let after_len = if i + 1 < changes.len() {
                changes[i + 1].from_a
            } else {
                a.len()
            } - ch.to_a;
            if before_len == 0 || after_len == 0 {
                continue;
            }
            let text = if len_a > 0 {
                &a[ch.from_a..ch.to_a]
            } else {
                &b[ch.from_b..ch.to_b]
            };
            if before_len <= text.len()
                && a[ch.from_a - before_len..ch.from_a] == text[text.len() - before_len..]
            {
                changes[i] = ch.offset(-(before_len as isize), -(before_len as isize));
                moved = true;
            } else if after_len <= text.len()
                && a[ch.to_a..ch.to_a + after_len] == text[..after_len]
            {
                changes[i] = ch.offset(after_len as isize, after_len as isize);
                moved = true;
            }
        }
        if !moved {
            return changes;
        }
    }
}

fn ascii_word_char(code: u16) -> bool {
    (49..58).contains(&code) || (65..91).contains(&code) || (97..123).contains(&code)
}

fn is_word_char(s: &[u16]) -> bool {
    char::decode_utf16(s.iter().copied())
        .next()
        .and_then(Result::ok)
        .is_some_and(|c| c.is_alphabetic() || c.is_numeric())
}

fn word_char_after(s: &[u16], pos: usize) -> usize {
    if pos == s.len() {
        return 0;
    }
    let next = s[pos];
    if next < 192 {
        return usize::from(ascii_word_char(next));
    }
    if !is_surrogate1(next) || pos == s.len() - 1 {
        return usize::from(is_word_char(&s[pos..pos + 1]));
    }
    if is_word_char(&s[pos..pos + 2]) { 2 } else { 0 }
}

fn word_char_before(s: &[u16], pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let prev = s[pos - 1];
    if prev < 192 {
        return usize::from(ascii_word_char(prev));
    }
    if !is_surrogate2(prev) || pos == 1 {
        return usize::from(is_word_char(&s[pos - 1..pos]));
    }
    if is_word_char(&s[pos - 2..pos]) { 2 } else { 0 }
}

const MAX_SCAN: usize = 8;

fn find_word_boundary_after(s: &[u16], pos: usize, max: usize) -> usize {
    if pos == s.len() || word_char_before(s, pos) == 0 {
        return pos;
    }
    let end = pos + max;
    let mut cur = pos;
    for _ in 0..MAX_SCAN {
        let size = word_char_after(s, cur);
        if size == 0 || cur + size > end {
            return cur;
        }
        cur += size;
    }
    pos
}

fn find_word_boundary_before(s: &[u16], pos: usize, max: usize) -> usize {
    if pos == 0 || word_char_after(s, pos) == 0 {
        return pos;
    }
    let end = pos as isize - max as isize;
    let mut cur = pos;
    for _ in 0..MAX_SCAN {
        let size = word_char_before(s, cur);
        if size == 0 || (cur as isize - size as isize) < end {
            return cur;
        }
        cur -= size;
    }
    pos
}

fn find_line_break_before(s: &[u16], mut pos: usize, stop: usize) -> Option<usize> {
    while pos != stop {
        if pos > 0 && s[pos - 1] == 10 {
            return Some(pos);
        }
        pos -= 1;
    }
    None
}

fn find_line_break_after(s: &[u16], mut pos: usize, stop: usize) -> Option<usize> {
    while pos != stop {
        if s.get(pos) == Some(&10) {
            return Some(pos);
        }
        pos += 1;
    }
    None
}

fn make_presentable(mut changes: Vec<DiffChange>, a: &[u16], b: &[u16]) -> Vec<DiffChange> {
    let mut pos_a = 0;
    let mut i = 0;
    while i < changes.len() {
        let mut change = changes[i];
        let len_a = change.to_a - change.from_a;
        let len_b = change.to_b - change.from_b;
        if (len_a > 0 && len_b > 0) || len_a > 3 || len_b > 3 {
            let next_change_a = if i == changes.len() - 1 {
                a.len()
            } else {
                changes[i + 1].from_a
            };
            let max_scan_before = change.from_a - pos_a;
            let max_scan_after = next_change_a - change.to_a;
            let mut bound_before = find_word_boundary_before(a, change.from_a, max_scan_before);
            let mut bound_after = find_word_boundary_after(a, change.to_a, max_scan_after);
            let mut len_before = change.from_a - bound_before;
            let mut len_after = bound_after - change.to_a;
            if (len_a == 0 || len_b == 0) && len_before > 0 && len_after > 0 {
                let change_len = len_a.max(len_b);
                let (change_text, change_from, change_to) = if len_a > 0 {
                    (a, change.from_a, change.to_a)
                } else {
                    (b, change.from_b, change.to_b)
                };
                if change_len > len_before
                    && a[bound_before..change.from_a]
                        == change_text[change_to - len_before..change_to]
                {
                    change = DiffChange::new(
                        bound_before,
                        bound_before + len_a,
                        change.from_b - len_before,
                        change.to_b - len_before,
                    );
                    changes[i] = change;
                    bound_before = change.from_a;
                    bound_after =
                        find_word_boundary_after(a, change.to_a, next_change_a - change.to_a);
                } else if change_len > len_after
                    && a[change.to_a..bound_after]
                        == change_text[change_from..change_from + len_after]
                {
                    change = DiffChange::new(
                        bound_after - len_a,
                        bound_after,
                        change.from_b + len_after,
                        change.to_b + len_after,
                    );
                    changes[i] = change;
                    bound_after = change.to_a;
                    bound_before =
                        find_word_boundary_before(a, change.from_a, change.from_a - pos_a);
                }
                len_before = change.from_a - bound_before;
                len_after = bound_after - change.to_a;
            }
            if len_before > 0 || len_after > 0 {
                change = DiffChange::new(
                    change.from_a - len_before,
                    change.to_a + len_after,
                    change.from_b - len_before,
                    change.to_b + len_after,
                );
                changes[i] = change;
            } else if len_a == 0 {
                let first = find_line_break_after(b, change.from_b, change.to_b);
                let last =
                    first.and_then(|_| find_line_break_before(b, change.to_b, change.from_b));
                if let Some(f) = first
                    && f - change.from_b <= max_scan_after
                    && change.to_b + (f - change.from_b) <= b.len()
                    && b[change.from_b..f] == b[change.to_b..change.to_b + (f - change.from_b)]
                {
                    let len = (f - change.from_b) as isize;
                    change = change.offset(len, len);
                    changes[i] = change;
                } else if let Some(l) = last
                    && change.to_b - l <= max_scan_before
                    && change.from_b >= change.to_b - l
                    && b[change.from_b - (change.to_b - l)..change.from_b] == b[l..change.to_b]
                {
                    let len = (change.to_b - l) as isize;
                    change = change.offset(-len, -len);
                    changes[i] = change;
                }
            } else if len_b == 0 {
                let first = find_line_break_after(a, change.from_a, change.to_a);
                let last =
                    first.and_then(|_| find_line_break_before(a, change.to_a, change.from_a));
                if let Some(f) = first
                    && f - change.from_a <= max_scan_after
                    && change.to_a + (f - change.from_a) <= a.len()
                    && a[change.from_a..f] == a[change.to_a..change.to_a + (f - change.from_a)]
                {
                    let len = (f - change.from_a) as isize;
                    change = change.offset(len, len);
                    changes[i] = change;
                } else if let Some(l) = last
                    && change.to_a - l <= max_scan_before
                    && change.from_a >= change.to_a - l
                    && a[change.from_a - (change.to_a - l)..change.from_a] == a[l..change.to_a]
                {
                    let len = (change.to_a - l) as isize;
                    change = change.offset(-len, -len);
                    changes[i] = change;
                }
            }
        }
        pos_a = change.to_a;
        i += 1;
    }
    merge_adjacent(&mut changes, 3);
    changes
}

/// `diff` on UTF-16 code units.
pub fn diff_units(a: &[u16], b: &[u16], conf: DiffConfig) -> Vec<DiffChange> {
    CRUDE.with(|c| c.set(false));
    let mut d = Differ {
        a,
        b,
        scan_limit: conf.scan_limit.map_or(NO_LIMIT, |s| s >> 1),
        deadline: conf
            .timeout
            .filter(|t| !t.is_zero())
            .map(|t| Instant::now() + t),
        f1: Frontier::new(),
        f2: Frontier::new(),
    };
    if a == b {
        return Vec::new();
    }
    let raw = d.find_diff(0, a.len(), 0, b.len());
    normalize(a, b, raw)
}

/// `presentableDiff` on UTF-16 code units.
pub fn presentable_diff_units(a: &[u16], b: &[u16], conf: DiffConfig) -> Vec<DiffChange> {
    make_presentable(diff_units(a, b, conf), a, b)
}

/// UTF-16 code units of `s`.
pub fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Map UTF-16 positions (relative to `base_a` / `base_b` in their texts,
/// themselves UTF-16 offsets) to byte offsets.
pub fn changes_to_bytes(
    changes: &[DiffChange],
    a: &Text,
    base_a: usize,
    b: &Text,
    base_b: usize,
) -> Vec<DiffChange> {
    changes
        .iter()
        .map(|c| DiffChange {
            from_a: a.utf16_to_byte(base_a + c.from_a),
            to_a: a.utf16_to_byte(base_a + c.to_a),
            from_b: b.utf16_to_byte(base_b + c.from_b),
            to_b: b.utf16_to_byte(base_b + c.to_b),
        })
        .collect()
}

const MAX_UNIQUE_LINES: usize = 0xF8FF - 0xE000 + 1;
const LINE_DIFF_SCAN_LIMIT: usize = 4000;
const LINE_DIFF_TIMEOUT: Duration = Duration::from_millis(80);
const WORD_DIFF_SCAN_LIMIT: usize = 500;
const WORD_DIFF_TIMEOUT: Duration = Duration::from_millis(40);
const WORD_DIFF_MAX_CHARS: usize = 20_000;

/// Lines of `s` and each line's start offset.
struct LineSplit<'a> {
    lines: Vec<&'a [u16]>,
    starts: Vec<usize>,
}

fn split_lines(s: &[u16]) -> LineSplit<'_> {
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    let mut start = 0;
    for (i, &c) in s.iter().enumerate() {
        if c == 10 {
            lines.push(&s[start..i]);
            starts.push(start);
            start = i + 1;
        }
    }
    lines.push(&s[start..]);
    starts.push(start);
    LineSplit { lines, starts }
}

fn line_index_to_offset(split: &LineSplit, line: usize, doc_len: usize) -> usize {
    split.starts.get(line).copied().unwrap_or(doc_len)
}

/// One private-use code unit per distinct line, or `None` past 6400
/// distinct lines.
fn encode_line_alphabet<'a>(a: &LineSplit<'a>, b: &LineSplit<'a>) -> Option<(Vec<u16>, Vec<u16>)> {
    fn encode<'a>(lines: &[&'a [u16]], map: &mut HashMap<&'a [u16], u16>) -> Option<Vec<u16>> {
        let mut out = Vec::with_capacity(lines.len());
        for &line in lines {
            let next = map.len();
            let ch = match map.get(line) {
                Some(&c) => c,
                None => {
                    if next >= MAX_UNIQUE_LINES {
                        return None;
                    }
                    let c = 0xE000 + next as u16;
                    map.insert(line, c);
                    c
                }
            };
            out.push(ch);
        }
        Some(out)
    }
    let mut map = HashMap::new();
    let ea = encode(&a.lines, &mut map)?;
    let eb = encode(&b.lines, &mut map)?;
    Some((ea, eb))
}

/// A line-index hunk (0-based, exclusive ends), as git would report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineHunk {
    pub from_a: usize,
    pub to_a: usize,
    pub from_b: usize,
    pub to_b: usize,
}

/// `lineHunks`: line Myers without word refinement. Empty when the files
/// have too many distinct lines to encode.
pub fn line_hunks(a: &str, b: &str) -> Vec<LineHunk> {
    if a == b {
        return Vec::new();
    }
    let (ua, ub) = (units(a), units(b));
    let (sa, sb) = (split_lines(&ua), split_lines(&ub));
    let Some((ea, eb)) = encode_line_alphabet(&sa, &sb) else {
        return Vec::new();
    };
    let conf = DiffConfig {
        scan_limit: Some(LINE_DIFF_SCAN_LIMIT),
        timeout: Some(LINE_DIFF_TIMEOUT),
    };
    diff_units(&ea, &eb, conf)
        .into_iter()
        .map(|c| LineHunk {
            from_a: c.from_a,
            to_a: c.to_a,
            from_b: c.from_b,
            to_b: c.to_b,
        })
        .collect()
}

fn refine_for_highlights(a: &[u16], b: &[u16], line_changes: &[DiffChange]) -> Vec<DiffChange> {
    let conf = DiffConfig {
        scan_limit: Some(WORD_DIFF_SCAN_LIMIT),
        timeout: Some(WORD_DIFF_TIMEOUT),
    };
    let mut out = Vec::new();
    for ch in line_changes {
        if ch.from_a == ch.to_a || ch.from_b == ch.to_b {
            out.push(*ch);
            continue;
        }
        let sa = &a[ch.from_a..ch.to_a];
        let sb = &b[ch.from_b..ch.to_b];
        if sa.len() + sb.len() > WORD_DIFF_MAX_CHARS {
            out.push(*ch);
            continue;
        }
        let inner = presentable_diff_units(sa, sb, conf);
        if inner.is_empty() {
            out.push(*ch);
            continue;
        }
        out.extend(
            inner
                .iter()
                .map(|ic| ic.offset(ch.from_a as isize, ch.from_b as isize)),
        );
    }
    out
}

/// `lineOrientedDiff`, the merge view's `diffConfig.override`: line Myers
/// for hunk bounds, then word refinement inside replacements. UTF-16 units.
pub fn line_oriented_diff_units(a: &[u16], b: &[u16]) -> Vec<DiffChange> {
    if a == b {
        return Vec::new();
    }
    let (sa, sb) = (split_lines(a), split_lines(b));
    let conf = DiffConfig {
        scan_limit: Some(LINE_DIFF_SCAN_LIMIT),
        timeout: Some(LINE_DIFF_TIMEOUT),
    };
    let Some((ea, eb)) = encode_line_alphabet(&sa, &sb) else {
        return diff_units(a, b, conf);
    };
    let line_changes: Vec<DiffChange> = diff_units(&ea, &eb, conf)
        .iter()
        .map(|c| DiffChange {
            from_a: line_index_to_offset(&sa, c.from_a, a.len()),
            to_a: line_index_to_offset(&sa, c.to_a, a.len()),
            from_b: line_index_to_offset(&sb, c.from_b, b.len()),
            to_b: line_index_to_offset(&sb, c.to_b, b.len()),
        })
        .collect();
    refine_for_highlights(a, b, &line_changes)
}

/// `presentableDiff(a, b, { override: lineOrientedDiff })`: the override's
/// changes, polished for presentation.
pub fn merge_presentable_diff_units(a: &[u16], b: &[u16]) -> Vec<DiffChange> {
    make_presentable(line_oriented_diff_units(a, b), a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(a: &str, b: &str) -> Vec<(usize, usize, usize, usize)> {
        diff_units(&units(a), &units(b), DiffConfig::default())
            .into_iter()
            .map(|c| (c.from_a, c.to_a, c.from_b, c.to_b))
            .collect()
    }

    fn pd(a: &str, b: &str) -> Vec<(usize, usize, usize, usize)> {
        presentable_diff_units(&units(a), &units(b), DiffConfig::default())
            .into_iter()
            .map(|c| (c.from_a, c.to_a, c.from_b, c.to_b))
            .collect()
    }

    #[test]
    fn basic_diffs() {
        assert_eq!(d("abc", "abc"), vec![]);
        assert_eq!(d("abc", "abxc"), vec![(2, 2, 2, 3)]);
        assert_eq!(d("abcd", "ad"), vec![(1, 3, 1, 1)]);
        assert_eq!(
            d("hello world", "hello brave new world"),
            vec![(6, 6, 6, 16)]
        );
        assert_eq!(d("one two three", "one 2 three"), vec![(4, 7, 4, 5)]);
        assert_eq!(d("abcabc", "xbcxbc"), vec![(0, 1, 0, 1), (3, 4, 3, 4)]);
    }

    #[test]
    fn presentable_expands_to_words() {
        assert_eq!(pd("one two three", "one too three"), vec![(4, 7, 4, 7)]);
        assert_eq!(pd("foo bar", "foo baz"), vec![(4, 7, 4, 7)]);
    }

    #[test]
    fn line_hunks_match_git() {
        let a = "a\nb\nc\nd\n";
        let b = "a\nB\nc\nd\ne\n";
        assert_eq!(
            line_hunks(a, b),
            vec![
                LineHunk {
                    from_a: 1,
                    to_a: 2,
                    from_b: 1,
                    to_b: 2
                },
                LineHunk {
                    from_a: 4,
                    to_a: 4,
                    from_b: 4,
                    to_b: 5
                }
            ]
        );
    }

    #[test]
    fn line_oriented_refines_words() {
        let a = units("keep\nlet x = 1;\nkeep\n");
        let b = units("keep\nlet y = 1;\nkeep\n");
        let r = line_oriented_diff_units(&a, &b);
        assert_eq!(r, vec![DiffChange::new(9, 10, 9, 10)]);
    }

    #[test]
    fn surrogates_are_not_split() {
        let r = d("a😀b", "a😁b");
        assert_eq!(r, vec![(1, 3, 1, 3)]);
    }
}
