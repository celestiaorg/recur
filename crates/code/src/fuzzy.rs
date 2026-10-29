//! The UI's fuzzy ranker for Go to Symbol, palettes and
//! mention typeaheads. Scoring runs on UTF-16 code units like JavaScript; the
//! same scorer on bytes is Quick Open's path ranking ([`crate::rank_files`]).

use crate::collate::locale_compare;

/// A ranked candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyHit<T> {
    pub item: T,
    pub score: i64,
    pub label: String,
}

/// A candidate with its label pre-lowercased and masked (see
/// [`prepare_fuzzy`]).
#[derive(Debug, Clone)]
pub struct FuzzyItem<T> {
    pub item: T,
    pub label: String,
    lower: Vec<u16>,
    mask: u32,
}

/// Bits 0-25 = `a`-`z` present, bit 26 = a digit, bit 27 = anything else. A
/// query can only be a subsequence of a text whose mask covers its own, so
/// one AND rejects most of an index before the scan.
fn char_mask(s: &[u16]) -> u32 {
    let mut m = 0;
    for &c in s {
        m |= match c {
            97..=122 => 1 << (c - 97),
            48..=57 => 1 << 26,
            _ => 1 << 27,
        };
    }
    m
}

fn index_of<T: PartialEq>(hay: &[T], needle: &[T]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The shared scorer over already-lowercased text and query units. `None`
/// when the query is not a substring or subsequence.
pub(crate) fn score_lower<T: Copy + PartialEq + From<u8>>(t: &[T], q: &[T]) -> Option<i64> {
    if q.is_empty() {
        return Some(0);
    }
    if t == q {
        return Some(10_000);
    }
    let slash = T::from(b'/');
    if let Some(idx) = index_of(t, q) {
        // Prefer basename and earlier matches.
        let base = match t.iter().rposition(|&c| c == slash) {
            Some(i) => &t[i + 1..],
            None => t,
        };
        let base_bonus = if base.starts_with(q) {
            500
        } else if index_of(base, q).is_some() {
            200
        } else {
            0
        };
        return Some(1000 - idx as i64 + base_bonus + (100 - t.len() as i64).max(0));
    }
    let (under, dash) = (T::from(b'_'), T::from(b'-'));
    let mut qi = 0;
    let mut score = 0i64;
    let mut prev: i64 = -2;
    for (i, &c) in t.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if c == q[qi] {
            score += if i as i64 == prev + 1 { 8 } else { 3 };
            if i == 0 || t[i - 1] == slash || t[i - 1] == under || t[i - 1] == dash {
                score += 12;
            }
            prev = i as i64;
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    Some(score - (t.len() / 4) as i64)
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Higher is better; `None` when there is no match.
pub fn fuzzy_score(text: &str, query: &str) -> Option<i64> {
    score_lower(
        &utf16(&text.to_lowercase()),
        &utf16(&js_trim(query).to_lowercase()),
    )
}

/// Lowercases a candidate list once so repeated queries against it do not
/// pay for it again. Memoize on the source list, not the query.
pub fn prepare_fuzzy<T: Clone>(items: &[T], label_of: impl Fn(&T) -> String) -> Vec<FuzzyItem<T>> {
    items
        .iter()
        .map(|item| {
            let label = label_of(item);
            let lower = utf16(&label.to_lowercase());
            let mask = char_mask(&lower);
            FuzzyItem {
                item: item.clone(),
                label,
                lower,
                mask,
            }
        })
        .collect()
}

/// Ranks `prepared` against `query`, keeping the best `limit`: score
/// descending, then label in `localeCompare` order. A bounded sorted list
/// keeps a 20k-file index O(n log limit), and the mask skips the scan for
/// most non-matches.
pub fn fuzzy_filter_prepared<T: Clone>(
    prepared: &[FuzzyItem<T>],
    query: &str,
    limit: usize,
) -> Vec<FuzzyHit<T>> {
    let q = utf16(&js_trim(query).to_lowercase());
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
        if out.len() == limit {
            let worst = &out[limit - 1];
            if score < worst.score
                || (score == worst.score && locale_compare(&p.label, &worst.label).is_ge())
            {
                continue;
            }
        }
        let at = out.partition_point(|m| {
            m.score > score || (m.score == score && locale_compare(&m.label, &p.label).is_lt())
        });
        out.insert(
            at,
            FuzzyHit {
                item: p.item.clone(),
                score,
                label: p.label.clone(),
            },
        );
        out.truncate(limit);
    }
    out
}

/// [`prepare_fuzzy`] then [`fuzzy_filter_prepared`].
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
    fn scores_like_the_ui() {
        assert_eq!(fuzzy_score("abc", ""), Some(0));
        assert_eq!(fuzzy_score("Main.go", "main.go"), Some(10_000));
        // Substring: 1000 - idx + base bonus + short-path bonus.
        assert_eq!(
            fuzzy_score("src/main.go", "main"),
            Some(1000 - 4 + 500 + 89)
        );
        assert_eq!(
            fuzzy_score("src/xmain.go", "main"),
            Some(1000 - 5 + 200 + 88)
        );
        // Subsequence: run and boundary bonuses minus length / 4.
        assert_eq!(fuzzy_score("a_bc", "abc"), Some(3 + 12 + 3 + 12 + 8 - 1));
        assert_eq!(fuzzy_score("abc", "abd"), None);
        assert_eq!(fuzzy_score("x", " x \u{feff}"), Some(10_000));
    }

    #[test]
    fn filter_orders_by_score_then_locale() {
        let items = ["src_y", "src/x", "Src", "src-z", "other"];
        let hits = fuzzy_filter(&items, "src", |s| s.to_string(), 10);
        let labels: Vec<&str> = hits.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, ["Src", "src_y", "src-z", "src/x"]);
        assert_eq!(hits[0].score, 10_000);
        let top = fuzzy_filter(&items, "src", |s| s.to_string(), 2);
        assert_eq!(top.len(), 2);
        assert!(fuzzy_filter(&items, "src", |s| s.to_string(), 0).is_empty());
    }

    #[test]
    fn mask_rejects_missing_characters() {
        let p = prepare_fuzzy(&["abc"], |s| s.to_string());
        assert!(fuzzy_filter_prepared(&p, "abz", 5).is_empty());
        assert_eq!(fuzzy_filter_prepared(&p, "ac", 5).len(), 1);
    }
}
