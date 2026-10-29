//! The fuzzy ranker behind Quick Open, Go to Symbol and searchable selects.

use std::cmp::Ordering;

/// A candidate with its label lowercased and character-class masked (`prepareFuzzy`).
#[derive(Clone, Debug)]
pub struct FuzzyItem<T> {
    pub item: T,
    pub label: String,
    lower: String,
    mask: u32,
}

/// A ranked match.
#[derive(Clone, Debug, PartialEq)]
pub struct FuzzyHit<T> {
    pub item: T,
    pub score: i64,
    pub label: String,
}

/// Bits 0–25: a–z present; bit 26: a digit; bit 27: anything else.
fn char_mask(s: &str) -> u32 {
    let mut m = 0;
    for c in s.encode_utf16() {
        m |= match c {
            97..=122 => 1 << (c - 97),
            48..=57 => 1 << 26,
            _ => 1 << 27,
        };
    }
    m
}

/// `scoreLower` over UTF-16 units, as JavaScript string indexing works.
fn score_lower(t: &str, q: &str) -> Option<i64> {
    if q.is_empty() {
        return Some(0);
    }
    if t == q {
        return Some(10_000);
    }
    let tu: Vec<u16> = t.encode_utf16().collect();
    let qu: Vec<u16> = q.encode_utf16().collect();
    if let Some(idx) = tu.windows(qu.len()).position(|w| w == qu.as_slice()) {
        let slash = tu.iter().rposition(|&c| c == '/' as u16);
        let base = slash.map_or(&tu[..], |s| &tu[s + 1..]);
        let base_bonus = if base.starts_with(&qu) {
            500
        } else if base.windows(qu.len()).any(|w| w == qu.as_slice()) {
            200
        } else {
            0
        };
        return Some(1000 - idx as i64 + base_bonus + (100 - tu.len() as i64).max(0));
    }
    let (mut qi, mut score, mut prev) = (0usize, 0i64, -2i64);
    for (i, &c) in tu.iter().enumerate() {
        if qi >= qu.len() {
            break;
        }
        if c == qu[qi] {
            score += if i as i64 == prev + 1 { 8 } else { 3 };
            if i == 0 || matches!(tu[i - 1], 0x2f | 0x5f | 0x2d) {
                score += 12;
            }
            prev = i as i64;
            qi += 1;
        }
    }
    if qi < qu.len() {
        return None;
    }
    Some(score - (tu.len() / 4) as i64)
}

/// `fuzzyScore`: higher is better, `None` when the query is not a subsequence.
pub fn fuzzy_score(text: &str, query: &str) -> Option<i64> {
    score_lower(&text.to_lowercase(), &query.trim().to_lowercase())
}

/// `prepareFuzzy`.
pub fn prepare_fuzzy<T: Clone>(items: &[T], label_of: impl Fn(&T) -> String) -> Vec<FuzzyItem<T>> {
    items
        .iter()
        .map(|item| {
            let label = label_of(item);
            let lower = label.to_lowercase();
            FuzzyItem {
                item: item.clone(),
                mask: char_mask(&lower),
                label,
                lower,
            }
        })
        .collect()
}

/// CLDR root collation order of ASCII punctuation and symbols (all of which sort before
/// digits and letters).
const PUNCT_ORDER: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

/// The tie-break order of `Intl.Collator()` (CLDR root) for UI labels: whitespace, then
/// punctuation in CLDR order, then digits, then letters case-insensitively; ties go
/// lowercase first.
pub fn collate(a: &str, b: &str) -> Ordering {
    let key = |c: char| -> (u8, u32) {
        if c.is_whitespace() {
            (0, c as u32)
        } else if let Some(i) = PUNCT_ORDER.find(c) {
            (1, i as u32)
        } else if c.is_numeric() {
            (2, c as u32)
        } else if c.is_alphabetic() {
            (3, c.to_lowercase().next().unwrap_or(c) as u32)
        } else {
            (1, 0x100 + c as u32)
        }
    };
    a.chars().map(key).cmp(b.chars().map(key)).then_with(|| {
        a.chars()
            .map(|c| !c.is_lowercase())
            .cmp(b.chars().map(|c| !c.is_lowercase()))
    })
}

/// `fuzzyFilterPrepared`: the best `limit` matches, score descending then label by collator.
pub fn fuzzy_filter_prepared<T: Clone>(
    prepared: &[FuzzyItem<T>],
    query: &str,
    limit: usize,
) -> Vec<FuzzyHit<T>> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<FuzzyHit<T>> = Vec::new();
    if limit == 0 {
        return out;
    }
    let q_mask = char_mask(&q);
    for p in prepared {
        if q_mask & !p.mask != 0 {
            continue;
        }
        let Some(score) = score_lower(&p.lower, &q) else {
            continue;
        };
        if out.len() == limit
            && let Some(worst) = out.last()
            && (score < worst.score
                || (score == worst.score && collate(&p.label, &worst.label) != Ordering::Less))
        {
            continue;
        }
        let at = out.partition_point(|m| {
            m.score > score || (m.score == score && collate(&m.label, &p.label) == Ordering::Less)
        });
        out.insert(
            at,
            FuzzyHit {
                item: p.item.clone(),
                score,
                label: p.label.clone(),
            },
        );
        if out.len() > limit {
            out.pop();
        }
    }
    out
}

/// `fuzzyFilter`.
pub fn fuzzy_filter<T: Clone>(
    items: &[T],
    query: &str,
    label_of: impl Fn(&T) -> String,
    limit: usize,
) -> Vec<FuzzyHit<T>> {
    fuzzy_filter_prepared(&prepare_fuzzy(items, label_of), query, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_like_fuzzy_ts() {
        assert_eq!(fuzzy_score("main", "main"), Some(10_000));
        assert_eq!(fuzzy_score("anything", "  "), Some(0));
        // Substring: 1000 − index + basename bonus + short-path bonus.
        assert_eq!(
            fuzzy_score("src/main.rs", "main"),
            Some(1000 - 4 + 500 + (100 - 11))
        );
        // Subsequence with word-start bonuses.
        let s = fuzzy_score("tab-strip-drag", "tsd").unwrap();
        assert_eq!(s, (3 + 12) + (3 + 12) + (3 + 12) - 14 / 4);
        assert_eq!(fuzzy_score("abc", "acb"), None);
    }

    #[test]
    fn ranks_and_limits() {
        let items = vec!["readme.md", "src/read.rs", "Reader.ts", "tread"];
        let hits = fuzzy_filter(&items, "read", |s| s.to_string(), 3);
        let labels: Vec<&str> = hits.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels.len(), 3);
        // Exact basename prefix scores above a mid-word match.
        assert_ne!(labels[0], "tread");
        assert!(fuzzy_filter(&items, "zzz", |s| s.to_string(), 5).is_empty());
        assert!(fuzzy_filter(&items, "r", |s| s.to_string(), 0).is_empty());
    }

    #[test]
    fn collator_puts_punctuation_before_letters() {
        // `_` sorts before `/` in CLDR, unlike code-point order.
        assert_eq!(collate("a_b", "a/b"), Ordering::Less);
        assert_eq!(collate("a-b", "a.b"), Ordering::Less);
        assert_eq!(collate("a-b", "ab"), Ordering::Less);
        assert_eq!(collate("apple", "Banana"), Ordering::Less);
        assert_eq!(collate("a", "A"), Ordering::Less);
    }
}
