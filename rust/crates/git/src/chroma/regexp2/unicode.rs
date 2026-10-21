//! Go `unicode` package lookups over the generated Unicode 15.0 tables.

use super::unicode_tables::{CATEGORIES, CATEGORY_ALIASES, SIMPLE_FOLD, TO_LOWER, TO_UPPER};

pub(crate) type Table = &'static [(char, char)];

pub(crate) fn in_table(t: Table, c: char) -> bool {
    t.binary_search_by(|&(lo, hi)| {
        if hi < c {
            std::cmp::Ordering::Less
        } else if lo > c {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    })
    .is_ok()
}

/// A general category (`L`, `Lu`, …) or one of Go's long aliases (`Letter`).
pub(crate) fn category(name: &str) -> Option<Table> {
    let lookup = |n: &str| {
        CATEGORIES
            .binary_search_by(|(k, _)| k.cmp(&n))
            .ok()
            .map(|i| CATEGORIES[i].1)
    };
    lookup(name).or_else(|| {
        let i = CATEGORY_ALIASES
            .binary_search_by(|(k, _)| k.cmp(&name))
            .ok()?;
        lookup(CATEGORY_ALIASES[i].1)
    })
}

fn cat(name: &str) -> Table {
    category(name).unwrap_or(&[])
}

fn mapped(t: &'static [(char, char)], c: char) -> Option<char> {
    t.binary_search_by(|(k, _)| k.cmp(&c)).ok().map(|i| t[i].1)
}

/// Go `unicode.ToLower`.
pub(crate) fn to_lower(c: char) -> char {
    mapped(TO_LOWER, c).unwrap_or(c)
}

/// Go `unicode.ToUpper`.
pub(crate) fn to_upper(c: char) -> char {
    mapped(TO_UPPER, c).unwrap_or(c)
}

/// Go `unicode.SimpleFold`: the next rune in `c`'s case-folding orbit.
pub(crate) fn simple_fold(c: char) -> char {
    mapped(SIMPLE_FOLD, c).unwrap_or(c)
}

pub(crate) fn is_lower(c: char) -> bool {
    in_table(cat("Ll"), c)
}

pub(crate) fn is_upper(c: char) -> bool {
    in_table(cat("Lu"), c)
}

pub(crate) fn is_letter(c: char) -> bool {
    in_table(cat("L"), c)
}

pub(crate) fn is_number(c: char) -> bool {
    in_table(cat("N"), c)
}

/// Go `unicode.IsSpace` (identical to the White_Space property).
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace()
}

/// Go `unicode.IsPrint`: letters, marks, numbers, punctuation, symbols and
/// the ASCII space.
pub(crate) fn is_print(c: char) -> bool {
    c == ' '
        || ["L", "M", "N", "P", "S"]
            .iter()
            .any(|k| in_table(cat(k), c))
}

/// regexp2 `IsWordChar`: L, Mn, Nd, Pc, plus ZWJ and ZWNJ.
pub(crate) fn is_word_char(c: char) -> bool {
    ["L", "Mn", "Nd", "Pc"].iter().any(|k| in_table(cat(k), c))
        || c == '\u{200D}'
        || c == '\u{200C}'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_case_mapping() {
        assert_eq!(to_lower('A'), 'a');
        assert_eq!(to_lower('\u{130}'), 'i');
        assert_eq!(to_upper('a'), 'A');
        assert_eq!(to_upper('\u{1C6}'), '\u{1C4}');
        assert_eq!(simple_fold('A'), 'a');
        assert_eq!(simple_fold('a'), 'A');
        assert_eq!(simple_fold('k'), '\u{212A}');
        assert_eq!(simple_fold('\u{212A}'), 'K');
        assert_eq!(simple_fold('1'), '1');
    }

    #[test]
    fn categories_and_classes() {
        assert!(is_word_char('_') && is_word_char('é') && is_word_char('\u{200D}'));
        assert!(!is_word_char('-'));
        assert!(is_letter('ж') && !is_letter('1'));
        assert!(is_number('Ⅻ') && is_number('7'));
        assert!(
            category("Letter").is_some() && category("Lu").is_some() && category("Nope").is_none()
        );
        assert!(is_print('a') && is_print(' ') && !is_print('\n'));
    }
}
