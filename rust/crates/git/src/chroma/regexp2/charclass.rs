//! Character classes (regexp2 `CharSet`): ranges, Unicode categories, an
//! optional subtraction, and negation.

use super::unicode::{self, Table};

pub(crate) const MAX_RUNE: u32 = 0x10FFFF;

/// The pseudo-category for `\s`.
pub(crate) const SPACE_CATEGORY: &str = " ";
/// The pseudo-category for `\w`.
pub(crate) const WORD_CATEGORY: &str = "W";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SingleRange {
    pub first: u32,
    pub last: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Category {
    pub negate: bool,
    pub cat: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CharSet {
    ranges: Vec<SingleRange>,
    categories: Vec<Category>,
    sub: Option<Box<CharSet>>,
    pub(crate) negate: bool,
    anything: bool,
}

impl CharSet {
    /// A set from regexp2's "old string" encoding: alternating range starts
    /// and (exclusive) ends.
    pub(crate) fn from_old_string(set_text: &[u32], negate: bool) -> CharSet {
        let mut c = CharSet::default();
        if set_text.is_empty() {
            return c;
        }
        let mut text = set_text;
        let mut fill_first = false;
        let mut l = text.len();
        if negate {
            if text[0] == 0 {
                text = &text[1..];
            } else {
                l += 1;
                fill_first = true;
            }
        }
        let n = if l.is_multiple_of(2) {
            l / 2
        } else {
            l / 2 + 1
        };
        c.ranges = vec![SingleRange { first: 0, last: 0 }; n];
        let mut first = true;
        if fill_first {
            c.ranges[0] = SingleRange { first: 0, last: 0 };
            first = false;
        }
        let mut i = 0;
        for &r in text {
            if first {
                c.ranges[i] = SingleRange { first: r, last: 0 };
                first = false;
            } else {
                c.ranges[i].last = r - 1;
                i += 1;
                first = true;
            }
        }
        if !first {
            c.ranges[i].last = MAX_RUNE;
        }
        if c.ranges.len() == 1 && c.ranges[0].first == 0 && c.ranges[0].last >= MAX_RUNE {
            c.anything = !negate;
        }
        c
    }

    pub(crate) fn from_categories(negate_set: bool, negate_cat: bool, cats: &[&str]) -> CharSet {
        CharSet {
            negate: negate_set,
            categories: cats
                .iter()
                .map(|c| Category {
                    cat: (*c).to_string(),
                    negate: negate_cat,
                })
                .collect(),
            ..CharSet::default()
        }
    }

    /// `AnyClass`: every rune.
    pub(crate) fn any() -> CharSet {
        CharSet::from_old_string(&[0], false)
    }

    pub(crate) fn word(negate: bool) -> CharSet {
        CharSet::from_categories(negate, false, &[WORD_CATEGORY])
    }

    pub(crate) fn space(negate: bool) -> CharSet {
        CharSet::from_categories(negate, false, &[SPACE_CATEGORY])
    }

    pub(crate) fn digit(negate: bool) -> CharSet {
        CharSet::from_categories(false, negate, &["Nd"])
    }

    pub(crate) fn char_in(&self, ch: char) -> bool {
        let ch32 = ch as u32;
        let mut val = false;
        let n = self.ranges.len();
        if n > 0 {
            if n <= 4 {
                for r in &self.ranges {
                    if ch32 < r.first {
                        break;
                    }
                    if ch32 <= r.last {
                        val = true;
                        break;
                    }
                }
            } else {
                let (mut lo, mut hi) = (0, n);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if self.ranges[mid].first <= ch32 {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                if lo > 0 && ch32 <= self.ranges[lo - 1].last {
                    val = true;
                }
            }
        }
        if !val && !self.categories.is_empty() {
            val = self.char_in_categories(ch);
        }
        if self.negate {
            val = !val;
        }
        if val && let Some(sub) = &self.sub {
            val = !sub.char_in(ch);
        }
        val
    }

    fn char_in_categories(&self, ch: char) -> bool {
        for ct in &self.categories {
            let hit = match ct.cat.as_str() {
                SPACE_CATEGORY => unicode::is_space(ch),
                WORD_CATEGORY => unicode::is_word_char(ch),
                name => unicode::category(name).is_some_and(|t: Table| unicode::in_table(t, ch)),
            };
            if hit {
                return !ct.negate;
            } else if ct.negate {
                return true;
            }
        }
        false
    }

    pub(crate) fn add_digit(&mut self, negate: bool) {
        self.add_categories(&[Category {
            cat: "Nd".to_string(),
            negate,
        }]);
    }

    pub(crate) fn add_char(&mut self, ch: u32) {
        self.add_range(ch, ch);
    }

    pub(crate) fn add_space(&mut self, negate: bool) {
        self.add_categories(&[Category {
            cat: SPACE_CATEGORY.to_string(),
            negate,
        }]);
    }

    pub(crate) fn add_word(&mut self, negate: bool) {
        self.add_categories(&[Category {
            cat: WORD_CATEGORY.to_string(),
            negate,
        }]);
    }

    fn make_anything(&mut self) {
        self.anything = true;
        self.categories.clear();
        self.ranges = vec![SingleRange {
            first: 0,
            last: MAX_RUNE,
        }];
    }

    fn add_categories(&mut self, cats: &[Category]) {
        if self.anything {
            return;
        }
        for ct in cats {
            let mut found = false;
            for ct2 in &self.categories {
                if ct.cat == ct2.cat {
                    if ct.negate != ct2.negate {
                        self.make_anything();
                        return;
                    }
                    found = true;
                    break;
                }
            }
            if !found {
                self.categories.push(ct.clone());
            }
        }
    }

    /// Adds `\p{name}` / `\P{name}`. The caller has validated `name`.
    pub(crate) fn add_category(&mut self, name: &str, negate: bool, case_insensitive: bool) {
        if case_insensitive && (name == "Ll" || name == "Lu" || name == "Lt") {
            // With IgnoreCase {Ll} {Lu} and {Lt} all match each other.
            self.add_categories(&[
                Category {
                    cat: "Ll".to_string(),
                    negate,
                },
                Category {
                    cat: "Lu".to_string(),
                    negate,
                },
                Category {
                    cat: "Lt".to_string(),
                    negate,
                },
            ]);
        }
        self.add_categories(&[Category {
            cat: name.to_string(),
            negate,
        }]);
    }

    /// Adds the case-equivalent versions of every character in the class.
    pub(crate) fn add_case_equivalences(&mut self) {
        if self.anything {
            return;
        }
        let mut i = 0;
        while i < self.ranges.len() {
            let r = self.ranges[i];
            if r.first == r.last {
                for eq in try_find_case_equivalences(r.first) {
                    self.add_char(eq);
                }
            } else {
                for c in r.first..=r.last {
                    for eq in try_find_case_equivalences(c) {
                        self.add_char(eq);
                    }
                }
            }
            i += 1;
        }
    }

    pub(crate) fn add_subtraction(&mut self, sub: CharSet) {
        self.sub = Some(Box::new(sub));
    }

    pub(crate) fn add_range(&mut self, first: u32, last: u32) {
        self.ranges.push(SingleRange { first, last });
        self.canonicalize();
    }

    fn canonicalize(&mut self) {
        if self.ranges.is_empty() {
            return;
        }
        if self.ranges.len() > 1 {
            self.ranges.sort_by_key(|r| r.first);
            let mut j = 0;
            let mut i = 1;
            loop {
                let mut last = self.ranges[j].last;
                let mut done = false;
                loop {
                    if i == self.ranges.len() || last >= MAX_RUNE {
                        done = true;
                        break;
                    }
                    let cur = self.ranges[i];
                    if cur.first > last + 1 {
                        break;
                    }
                    if last < cur.last {
                        last = cur.last;
                    }
                    i += 1;
                }
                self.ranges[j] = SingleRange {
                    first: self.ranges[j].first,
                    last,
                };
                j += 1;
                if done {
                    break;
                }
                if j < i {
                    self.ranges[j] = self.ranges[i];
                }
                i += 1;
            }
            self.ranges.truncate(j);
        }

        // A class that includes every other character becomes its negation.
        if !self.negate && self.sub.is_none() && self.categories.is_empty() {
            if self.ranges.len() == 2 {
                if self.ranges[0].first == 0
                    && self.ranges[1].last >= MAX_RUNE
                    && self.ranges[0].last + 1 < self.ranges[1].first
                {
                    self.ranges = vec![SingleRange {
                        first: self.ranges[0].last + 1,
                        last: self.ranges[1].first - 1,
                    }];
                    self.negate = true;
                }
            } else if self.ranges.len() == 1 {
                match self.ranges[0].first {
                    0 if self.ranges[0].last == MAX_RUNE - 1 => {
                        self.ranges[0] = SingleRange {
                            first: MAX_RUNE,
                            last: MAX_RUNE,
                        };
                        self.negate = true;
                    }
                    1 if self.ranges[0].last >= MAX_RUNE => {
                        self.ranges[0] = SingleRange { first: 0, last: 0 };
                        self.negate = true;
                    }
                    _ => {}
                }
            }
        }

        if !self.negate
            && self.sub.is_none()
            && self.ranges.len() == 1
            && self.ranges[0].first == 0
            && self.ranges[0].last >= MAX_RUNE
        {
            self.make_anything();
        }

        if !self.negate
            && self.sub.is_none()
            && !self.categories.is_empty()
            && self.ranges.len() == 2
            && self.ranges[0].first == 0
            && self.ranges[0].last + 2 == self.ranges[1].first
            && self.ranges[1].last == MAX_RUNE
        {
            let missing = self.ranges[0].last + 1;
            if char::from_u32(missing).is_some_and(|c| self.char_in_categories(c)) {
                self.make_anything();
            } else {
                self.negate = true;
                self.ranges = vec![SingleRange {
                    first: missing,
                    last: missing,
                }];
                self.categories.clear();
            }
        }
    }

    /// Adds lowercase versions of the class's characters (IgnoreCase sets).
    pub(crate) fn add_lowercase(&mut self) {
        if self.anything {
            return;
        }
        let mut to_add = Vec::new();
        for r in &mut self.ranges {
            if r.first == r.last {
                let lower =
                    char::from_u32(r.first).map_or(r.first, |c| unicode::to_lower(c) as u32);
                *r = SingleRange {
                    first: lower,
                    last: lower,
                };
            } else {
                to_add.push(*r);
            }
        }
        for r in to_add {
            self.add_lowercase_range(r.first, r.last);
        }
        self.canonicalize();
    }

    fn add_lowercase_range(&mut self, ch_min: u32, ch_max: u32) {
        let (mut i, mut i_max) = (0, LC_TABLE.len());
        while i < i_max {
            let mid = (i + i_max) / 2;
            if LC_TABLE[mid].1 < ch_min {
                i = mid + 1;
            } else {
                i_max = mid;
            }
        }
        for &(lo, hi, op, data) in &LC_TABLE[i..] {
            if lo > ch_max {
                return;
            }
            let mut min_t = lo.max(ch_min) as i64;
            let mut max_t = hi.min(ch_max) as i64;
            match op {
                LC_SET => {
                    min_t = data;
                    max_t = data;
                }
                LC_ADD => {
                    min_t += data;
                    max_t += data;
                }
                LC_BOR => {
                    min_t |= 1;
                    max_t |= 1;
                }
                _ => {
                    min_t += min_t & 1;
                    max_t += max_t & 1;
                }
            }
            if min_t < i64::from(ch_min) || max_t > i64::from(ch_max) {
                self.add_range(min_t as u32, max_t as u32);
            }
        }
    }
}

/// The other characters in `ch`'s simple case-folding orbit.
fn try_find_case_equivalences(ch: u32) -> Vec<u32> {
    let Some(c) = char::from_u32(ch) else {
        return Vec::new();
    };
    let mut next = unicode::simple_fold(c);
    if next == c {
        return Vec::new();
    }
    let mut equiv = vec![next as u32];
    loop {
        next = unicode::simple_fold(next);
        if next == c {
            return equiv;
        }
        equiv.push(next as u32);
    }
}

/// Whether any character takes part in case conversion.
pub(crate) fn any_participate_in_case_conversion(chars: &[u32]) -> bool {
    chars.iter().any(|&c| participates_in_case_conversion(c))
}

/// Every character outside punctuation, digits, other numbers, controls
/// and separators may take part in case conversion.
fn participates_in_case_conversion(c: u32) -> bool {
    let Some(c) = char::from_u32(c) else {
        return true;
    };
    ![
        "Pe", "Pc", "Cc", "Pd", "Nd", "Pf", "Pi", "Zl", "Ps", "No", "Po", "Zp", "Zs",
    ]
    .iter()
    .any(|k| unicode::category(k).is_some_and(|t| unicode::in_table(t, c)))
}

const LC_SET: u8 = 0;
const LC_ADD: u8 = 1;
const LC_BOR: u8 = 2;
const LC_BAD: u8 = 3;

/// regexp2's lowercase table: intervals on which lowercasing is
/// non-decreasing, with the mapping form for each.
static LC_TABLE: &[(u32, u32, u8, i64)] = &[
    (0x0041, 0x005A, LC_ADD, 32),
    (0x00C0, 0x00DE, LC_ADD, 32),
    (0x0100, 0x012E, LC_BOR, 0),
    (0x0130, 0x0130, LC_SET, 0x0069),
    (0x0132, 0x0136, LC_BOR, 0),
    (0x0139, 0x0147, LC_BAD, 0),
    (0x014A, 0x0176, LC_BOR, 0),
    (0x0178, 0x0178, LC_SET, 0x00FF),
    (0x0179, 0x017D, LC_BAD, 0),
    (0x0181, 0x0181, LC_SET, 0x0253),
    (0x0182, 0x0184, LC_BOR, 0),
    (0x0186, 0x0186, LC_SET, 0x0254),
    (0x0187, 0x0187, LC_SET, 0x0188),
    (0x0189, 0x018A, LC_ADD, 205),
    (0x018B, 0x018B, LC_SET, 0x018C),
    (0x018E, 0x018E, LC_SET, 0x01DD),
    (0x018F, 0x018F, LC_SET, 0x0259),
    (0x0190, 0x0190, LC_SET, 0x025B),
    (0x0191, 0x0191, LC_SET, 0x0192),
    (0x0193, 0x0193, LC_SET, 0x0260),
    (0x0194, 0x0194, LC_SET, 0x0263),
    (0x0196, 0x0196, LC_SET, 0x0269),
    (0x0197, 0x0197, LC_SET, 0x0268),
    (0x0198, 0x0198, LC_SET, 0x0199),
    (0x019C, 0x019C, LC_SET, 0x026F),
    (0x019D, 0x019D, LC_SET, 0x0272),
    (0x019F, 0x019F, LC_SET, 0x0275),
    (0x01A0, 0x01A4, LC_BOR, 0),
    (0x01A7, 0x01A7, LC_SET, 0x01A8),
    (0x01A9, 0x01A9, LC_SET, 0x0283),
    (0x01AC, 0x01AC, LC_SET, 0x01AD),
    (0x01AE, 0x01AE, LC_SET, 0x0288),
    (0x01AF, 0x01AF, LC_SET, 0x01B0),
    (0x01B1, 0x01B2, LC_ADD, 217),
    (0x01B3, 0x01B5, LC_BAD, 0),
    (0x01B7, 0x01B7, LC_SET, 0x0292),
    (0x01B8, 0x01B8, LC_SET, 0x01B9),
    (0x01BC, 0x01BC, LC_SET, 0x01BD),
    (0x01C4, 0x01C5, LC_SET, 0x01C6),
    (0x01C7, 0x01C8, LC_SET, 0x01C9),
    (0x01CA, 0x01CB, LC_SET, 0x01CC),
    (0x01CD, 0x01DB, LC_BAD, 0),
    (0x01DE, 0x01EE, LC_BOR, 0),
    (0x01F1, 0x01F2, LC_SET, 0x01F3),
    (0x01F4, 0x01F4, LC_SET, 0x01F5),
    (0x01FA, 0x0216, LC_BOR, 0),
    (0x0386, 0x0386, LC_SET, 0x03AC),
    (0x0388, 0x038A, LC_ADD, 37),
    (0x038C, 0x038C, LC_SET, 0x03CC),
    (0x038E, 0x038F, LC_ADD, 63),
    (0x0391, 0x03AB, LC_ADD, 32),
    (0x03E2, 0x03EE, LC_BOR, 0),
    (0x0401, 0x040F, LC_ADD, 80),
    (0x0410, 0x042F, LC_ADD, 32),
    (0x0460, 0x0480, LC_BOR, 0),
    (0x0490, 0x04BE, LC_BOR, 0),
    (0x04C1, 0x04C3, LC_BAD, 0),
    (0x04C7, 0x04C7, LC_SET, 0x04C8),
    (0x04CB, 0x04CB, LC_SET, 0x04CC),
    (0x04D0, 0x04EA, LC_BOR, 0),
    (0x04EE, 0x04F4, LC_BOR, 0),
    (0x04F8, 0x04F8, LC_SET, 0x04F9),
    (0x0531, 0x0556, LC_ADD, 48),
    (0x10A0, 0x10C5, LC_ADD, 48),
    (0x1E00, 0x1EF8, LC_BOR, 0),
    (0x1F08, 0x1F0F, LC_ADD, -8),
    (0x1F18, 0x1F1F, LC_ADD, -8),
    (0x1F28, 0x1F2F, LC_ADD, -8),
    (0x1F38, 0x1F3F, LC_ADD, -8),
    (0x1F48, 0x1F4D, LC_ADD, -8),
    (0x1F59, 0x1F59, LC_SET, 0x1F51),
    (0x1F5B, 0x1F5B, LC_SET, 0x1F53),
    (0x1F5D, 0x1F5D, LC_SET, 0x1F55),
    (0x1F5F, 0x1F5F, LC_SET, 0x1F57),
    (0x1F68, 0x1F6F, LC_ADD, -8),
    (0x1F88, 0x1F8F, LC_ADD, -8),
    (0x1F98, 0x1F9F, LC_ADD, -8),
    (0x1FA8, 0x1FAF, LC_ADD, -8),
    (0x1FB8, 0x1FB9, LC_ADD, -8),
    (0x1FBA, 0x1FBB, LC_ADD, -74),
    (0x1FBC, 0x1FBC, LC_SET, 0x1FB3),
    (0x1FC8, 0x1FCB, LC_ADD, -86),
    (0x1FCC, 0x1FCC, LC_SET, 0x1FC3),
    (0x1FD8, 0x1FD9, LC_ADD, -8),
    (0x1FDA, 0x1FDB, LC_ADD, -100),
    (0x1FE8, 0x1FE9, LC_ADD, -8),
    (0x1FEA, 0x1FEB, LC_ADD, -112),
    (0x1FEC, 0x1FEC, LC_SET, 0x1FE5),
    (0x1FF8, 0x1FF9, LC_ADD, -128),
    (0x1FFA, 0x1FFB, LC_ADD, -126),
    (0x1FFC, 0x1FFC, LC_SET, 0x1FF3),
    (0x2160, 0x216F, LC_ADD, 16),
    (0x24B6, 0x24D0, LC_ADD, 26),
    (0xFF21, 0xFF3A, LC_ADD, 32),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_and_negate() {
        let mut s = CharSet::default();
        s.add_range(u32::from('a'), u32::from('c'));
        s.add_range(u32::from('b'), u32::from('f'));
        assert!(s.char_in('e') && !s.char_in('g'));
        let not_newline = CharSet::from_old_string(&[0x0a, 0x0b], true);
        assert!(!not_newline.char_in('\n') && not_newline.char_in('x'));
        assert!(CharSet::any().char_in('\n'));
    }

    #[test]
    fn categories_and_case() {
        let w = CharSet::word(false);
        assert!(w.char_in('é') && !w.char_in(' '));
        assert!(CharSet::word(true).char_in(' '));
        assert!(CharSet::digit(true).char_in('x') && !CharSet::digit(true).char_in('3'));
        let mut k = CharSet::default();
        k.add_char(u32::from('k'));
        k.add_case_equivalences();
        assert!(k.char_in('K') && k.char_in('\u{212A}'));
    }
}
