//! Oniguruma scanner with the semantics of `vscode-oniguruma`.
//!
//! Patterns compile with `ONIG_OPTION_CAPTURE_GROUP` and the default
//! (Oniguruma) syntax over UTF-8. A scan returns the leftmost match across all
//! patterns, the lowest pattern index winning ties, exactly like
//! `onig_regset_search(..., ONIG_REGSET_POSITION_LEAD)`. Offsets are byte
//! offsets into the searched string.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use onig::{Regex, RegexOptions, Region, SearchOptions, Syntax};

/// One capture group of a match. Unmatched groups are reported as an empty
/// range at the end of the string, which every consumer treats as "nothing
/// captured" (vscode-oniguruma reports them as zero-length too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureIndex {
    pub start: usize,
    pub end: usize,
}

impl CaptureIndex {
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnigMatch {
    pub index: usize,
    pub capture_indices: Vec<CaptureIndex>,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid regular expression {pattern:?}: {message}")]
pub struct OnigError {
    pub pattern: String,
    pub message: String,
}

static NEXT_STRING_ID: AtomicU32 = AtomicU32::new(1);

/// A searchable string. The id keys the per-regex search cache, the same way
/// vscode-oniguruma caches results per `OnigString`.
pub struct OnigString {
    content: String,
    id: u32,
}

impl OnigString {
    pub fn new(content: String) -> Self {
        Self {
            content,
            id: NEXT_STRING_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

struct LastSearch {
    string_id: u32,
    position: usize,
    result: Option<Vec<CaptureIndex>>,
}

struct CachedRegex {
    regex: Regex,
    has_g_anchor: bool,
    last: Mutex<Option<LastSearch>>,
}

/// Shared across threads (a grammar's scanners serve the UI and background
/// tokenizers); the search scratch is per thread.
pub struct OnigScanner {
    regexes: Vec<CachedRegex>,
    searches: AtomicU64,
}

thread_local! {
    static REGION: RefCell<Region> = RefCell::new(Region::new());
}

fn has_g_anchor(pattern: &str) -> bool {
    let b = pattern.as_bytes();
    (0..b.len().saturating_sub(1)).any(|i| b[i] == b'\\' && b[i + 1] == b'G')
}

impl OnigScanner {
    pub fn new<S: AsRef<str>>(patterns: &[S]) -> Result<Self, OnigError> {
        let mut regexes = Vec::with_capacity(patterns.len());
        for p in patterns {
            let p = p.as_ref();
            let regex = Regex::with_options(
                p,
                RegexOptions::REGEX_OPTION_CAPTURE_GROUP,
                Syntax::default(),
            )
            .map_err(|e| OnigError {
                pattern: p.to_string(),
                message: e.description().to_string(),
            })?;
            regexes.push(CachedRegex {
                has_g_anchor: has_g_anchor(p),
                regex,
                last: Mutex::new(None),
            });
        }
        Ok(Self {
            regexes,
            searches: AtomicU64::new(0),
        })
    }

    fn search_one(&self, i: usize, s: &OnigString, position: usize) -> Option<Vec<CaptureIndex>> {
        let r = &self.regexes[i];
        if !r.has_g_anchor
            && let Some(last) = r.last.lock().unwrap().as_ref()
            && last.string_id == s.id
            && last.position <= position
        {
            match &last.result {
                None => return None,
                Some(caps) if caps[0].start >= position => return Some(caps.clone()),
                Some(_) => {}
            }
        }
        self.searches.fetch_add(1, Ordering::Relaxed);
        let text = s.content();
        let result = REGION.with_borrow_mut(|region| {
            region.clear();
            let found = r.regex.search_with_options(
                text,
                position,
                text.len(),
                SearchOptions::SEARCH_OPTION_NONE,
                Some(region),
            );
            found.map(|_| {
                (0..region.len())
                    .map(|g| match region.pos(g) {
                        Some((start, end)) => CaptureIndex { start, end },
                        None => CaptureIndex {
                            start: text.len(),
                            end: text.len(),
                        },
                    })
                    .collect::<Vec<_>>()
            })
        });
        if !r.has_g_anchor {
            *r.last.lock().unwrap() = Some(LastSearch {
                string_id: s.id,
                position,
                result: result.clone(),
            });
        }
        result
    }

    /// Leftmost match at or after `position` across all patterns.
    pub fn find_next_match(&self, s: &OnigString, position: usize) -> Option<OnigMatch> {
        let mut best: Option<OnigMatch> = None;
        for i in 0..self.regexes.len() {
            let Some(caps) = self.search_one(i, s, position) else {
                continue;
            };
            let location = caps[0].start;
            let better = best
                .as_ref()
                .is_none_or(|b| location < b.capture_indices[0].start);
            if better {
                best = Some(OnigMatch {
                    index: i,
                    capture_indices: caps,
                });
            }
            if location == position {
                break;
            }
        }
        best
    }

    /// Regex executions so far (cache misses). Tests use it to verify caching.
    pub fn search_count(&self) -> u64 {
        self.searches.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(m: &OnigMatch) -> Vec<(usize, usize)> {
        m.capture_indices.iter().map(|c| (c.start, c.end)).collect()
    }

    #[test]
    fn leftmost_match_wins_and_ties_go_to_the_first_pattern() {
        let s = OnigScanner::new(&["c", "b", "bc"]).unwrap();
        let text = OnigString::new("abc".into());
        let m = s.find_next_match(&text, 0).unwrap();
        assert_eq!(m.index, 1);
        assert_eq!(caps(&m), vec![(1, 2)]);
    }

    #[test]
    fn unmatched_groups_are_empty_at_string_end() {
        let s = OnigScanner::new(&["(a)|(b)"]).unwrap();
        let text = OnigString::new("b\n".into());
        let m = s.find_next_match(&text, 0).unwrap();
        assert_eq!(caps(&m), vec![(0, 1), (2, 2), (0, 1)]);
    }

    #[test]
    fn byte_offsets_for_multibyte_text() {
        let s = OnigScanner::new(&["x"]).unwrap();
        let text = OnigString::new("ééx".into());
        let m = s.find_next_match(&text, 0).unwrap();
        assert_eq!(caps(&m), vec![(4, 5)]);
    }

    #[test]
    fn g_anchor_matches_only_at_the_search_position() {
        let s = OnigScanner::new(&["\\Gb"]).unwrap();
        let text = OnigString::new("abb".into());
        assert!(s.find_next_match(&text, 0).is_none());
        assert_eq!(caps(&s.find_next_match(&text, 1).unwrap()), vec![(1, 2)]);
    }

    #[test]
    fn lookbehind_sees_text_before_the_start_position() {
        let s = OnigScanner::new(&["(?<=a)b"]).unwrap();
        let text = OnigString::new("ab".into());
        assert_eq!(caps(&s.find_next_match(&text, 1).unwrap()), vec![(1, 2)]);
    }

    #[test]
    fn cached_results_are_reused_for_later_positions() {
        let s = OnigScanner::new(&["z"]).unwrap();
        let text = OnigString::new("aaaaz".into());
        s.find_next_match(&text, 0).unwrap();
        s.find_next_match(&text, 2).unwrap();
        assert_eq!(s.search_count(), 1);
        assert!(s.find_next_match(&OnigString::new("q".into()), 0).is_none());
        assert_eq!(s.search_count(), 2);
    }

    #[test]
    fn invalid_patterns_report_an_error() {
        assert!(OnigScanner::new(&["("]).is_err());
    }
}
