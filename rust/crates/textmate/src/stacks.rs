//! Incremental rule-stack + token checkpoints (`cm-tm-stacks.ts`).
//!
//! Line N+1 needs the stack after line N. After an edit, stale stacks are
//! kept so re-tokenizing can stop as soon as a line ends in the same state
//! it did before (usually 1-3 lines, not the whole viewport).

use std::rc::Rc;
use std::time::Instant;

use crate::grammar::StateStack;

/// Line tokens in tokenizeLine2 layout (`[start, metadata, ...]`).
pub type LineTokens = Rc<[u32]>;

/// A rule stack that can tell whether it ended in the same state as another.
pub trait RuleStack: Clone {
    fn same_state(&self, other: &Self) -> bool;
}

impl RuleStack for StateStack {
    fn same_state(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

pub struct TmStackCache<S = StateStack> {
    /// `after[n]` = stack after 1-based line n. Index 0 unused.
    after: Vec<Option<S>>,
    tokens: Vec<Option<LineTokens>>,
    filled_through: usize,
}

impl<S> Default for TmStackCache<S> {
    fn default() -> Self {
        TmStackCache {
            after: vec![None],
            tokens: vec![None],
            filled_through: 0,
        }
    }
}

fn get<T: Clone>(v: &[Option<T>], n: usize) -> Option<T> {
    v.get(n).cloned().flatten()
}

fn set<T>(v: &mut Vec<Option<T>>, n: usize, value: T) {
    if v.len() <= n {
        v.resize_with(n + 1, || None);
    }
    v[n] = Some(value);
}

fn splice_holes<T>(v: &mut Vec<Option<T>>, index: usize, del: usize, ins: usize) {
    if v.len() < index {
        v.resize_with(index, || None);
    }
    let end = (index + del).min(v.len());
    v.splice(index..end, std::iter::repeat_with(|| None).take(ins));
}

impl<S: Clone> TmStackCache<S> {
    pub fn filled(&self) -> usize {
        self.filled_through
    }

    /// Line to start tokenizing from, and the stack entering it (`None` =
    /// the initial state).
    pub fn resume_at(&self, from_line: usize) -> (usize, Option<S>) {
        let before = (from_line.max(1) - 1).min(self.filled_through);
        if before >= 1
            && let Some(stack) = get(&self.after, before)
        {
            return (before + 1, Some(stack));
        }
        (1, None)
    }

    pub fn remember(&mut self, line: usize, stack: S, tokens: Option<LineTokens>) {
        if line < 1 {
            return;
        }
        set(&mut self.after, line, stack);
        if let Some(t) = tokens {
            set(&mut self.tokens, line, t);
        }
        if line == self.filled_through + 1 {
            self.filled_through = line;
        }
    }

    pub fn stack_at(&self, line: usize) -> Option<S> {
        get(&self.after, line)
    }

    pub fn tokens_at(&self, line: usize) -> Option<LineTokens> {
        get(&self.tokens, line)
    }

    pub fn has_tokens(&self, from_line: usize, to_line: usize) -> bool {
        (from_line..=to_line).all(|n| self.tokens.get(n).is_some_and(Option::is_some))
    }

    /// Line `n` ended in its previous state, so later cached lines are valid
    /// again while they still have both a stack and tokens.
    pub fn mark_stable(&mut self, n: usize) {
        let mut end = n;
        while self.after.get(end + 1).is_some_and(Option::is_some)
            && self.tokens.get(end + 1).is_some_and(Option::is_some)
        {
            end += 1;
        }
        self.filled_through = self.filled_through.max(end);
    }

    /// 1-based splice matching a document line replace. Same-length replaces
    /// keep stale stacks (for early-stop) but drop their tokens.
    pub fn splice(&mut self, from_line: usize, delete_count: usize, insert_count: usize) {
        let i = from_line.max(1);
        if delete_count != insert_count {
            splice_holes(&mut self.after, i, delete_count, insert_count);
            splice_holes(&mut self.tokens, i, delete_count, insert_count);
        } else {
            for n in i..i + insert_count {
                if let Some(slot) = self.tokens.get_mut(n) {
                    *slot = None;
                }
            }
        }
        self.filled_through = self.filled_through.min(i - 1);
    }
}

pub struct RangeResult {
    pub tokenized: usize,
    pub stable_at: Option<usize>,
    pub last_line: usize,
}

/// Tokenize `from_line..=to_line`, resuming from the last checkpoint. With
/// `stop_when_stable`, stop after a line whose end stack matches its stale
/// cached stack once the rest of the range still has tokens.
#[allow(clippy::too_many_arguments)]
pub fn tokenize_line_range<S: RuleStack, E>(
    line_text: impl Fn(usize) -> String,
    cache: &mut TmStackCache<S>,
    mut tokenize: impl FnMut(&str, Option<&S>) -> Result<(LineTokens, S), E>,
    from_line: usize,
    to_line: usize,
    stop_when_stable: bool,
    budget: Option<usize>,
    deadline: Option<Instant>,
) -> Result<RangeResult, E> {
    if to_line < 1 || from_line > to_line {
        return Ok(RangeResult {
            tokenized: 0,
            stable_at: None,
            last_line: to_line,
        });
    }
    let (start_line, mut stack) = cache.resume_at(from_line);
    let mut tokenized = 0;
    let mut stable_at = None;
    let mut last_line = start_line - 1;
    let end = match budget {
        Some(b) => to_line.min((start_line + b).saturating_sub(1)),
        None => to_line,
    };
    let mut n = start_line;
    while n <= end {
        let stale = if stop_when_stable {
            cache.stack_at(n)
        } else {
            None
        };
        let (tokens, next) = tokenize(&line_text(n), stack.as_ref())?;
        cache.remember(n, next.clone(), Some(tokens));
        tokenized += 1;
        last_line = n;
        if let Some(stale) = stale
            && next.same_state(&stale)
        {
            cache.mark_stable(n);
            if cache.has_tokens(n + 1, to_line) {
                stable_at = Some(n);
                break;
            }
        }
        stack = Some(next);
        if deadline.is_some_and(|d| Instant::now() > d) {
            break;
        }
        n += 1;
    }
    Ok(RangeResult {
        tokenized,
        stable_at,
        last_line: stable_at.unwrap_or(last_line),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Fake(usize);

    impl RuleStack for Fake {
        fn same_state(&self, other: &Self) -> bool {
            self.0 == other.0
        }
    }

    fn toks(v: &[u32]) -> LineTokens {
        v.into()
    }

    #[test]
    fn starts_from_line_one_with_no_checkpoints() {
        let cache = TmStackCache::<Fake>::default();
        assert_eq!(cache.resume_at(4000).0, 1);
    }

    #[test]
    fn resumes_after_the_last_contiguous_checkpoint() {
        let mut cache = TmStackCache::default();
        for n in 1..=50 {
            cache.remember(n, Fake(n), None);
        }
        assert_eq!(cache.resume_at(4980), (51, Some(Fake(50))));
    }

    #[test]
    fn a_viewport_near_the_end_does_not_restart_after_a_fill() {
        let mut cache = TmStackCache::default();
        for n in 1..=5000 {
            cache.remember(n, Fake(n), None);
        }
        assert_eq!(cache.resume_at(4980).0, 4980);
    }

    #[test]
    fn growing_the_cache_skips_the_filled_prefix() {
        let mut cache = TmStackCache::default();
        for n in 1..=100 {
            cache.remember(n, Fake(n), None);
        }
        assert_eq!(cache.resume_at(90), (90, Some(Fake(89))));
    }

    #[test]
    fn a_splice_keeps_the_stack_entering_the_changed_line() {
        let mut cache = TmStackCache::default();
        for n in 1..=100 {
            cache.remember(n, Fake(n), None);
        }
        cache.splice(40, 1, 1);
        assert_eq!(cache.resume_at(80), (40, Some(Fake(39))));
    }

    #[test]
    fn a_splice_at_line_one_drops_everything() {
        let mut cache = TmStackCache::default();
        cache.remember(1, Fake(1), None);
        cache.splice(1, 1, 1);
        assert_eq!(cache.resume_at(10).0, 1);
        assert_eq!(cache.filled(), 0);
    }

    #[test]
    fn splices_a_whole_document_replace() {
        let mut cache = TmStackCache::default();
        for n in 1..=5 {
            cache.remember(n, Fake(n), Some(toks(&[0, n as u32])));
        }
        cache.splice(1, 5, 200_000);
        assert_eq!(cache.filled(), 0);
    }

    fn fake_tokenize(text: &str, prev: Option<&Fake>) -> Result<(LineTokens, Fake), ()> {
        let d = prev.map_or(0, |p| p.0);
        Ok((toks(&[0, text.len().max(1) as u32]), Fake(d + text.len())))
    }

    #[test]
    fn stops_on_the_edited_line_when_the_end_stack_matches() {
        let texts = ["aa", "bb", "cc", "dd", "ee"];
        let mut cache = TmStackCache::default();
        let text = |n: usize| texts[n - 1].to_string();
        tokenize_line_range(text, &mut cache, fake_tokenize, 1, 5, false, None, None).unwrap();
        assert_eq!(cache.filled(), 5);
        cache.splice(3, 1, 1);
        let r =
            tokenize_line_range(text, &mut cache, fake_tokenize, 3, 5, true, None, None).unwrap();
        assert_eq!(r.tokenized, 1);
        assert_eq!(r.stable_at, Some(3));
        assert_eq!(cache.filled(), 5);
    }

    #[test]
    fn keeps_stale_stacks_on_a_same_length_splice() {
        let mut cache = TmStackCache::default();
        for n in 1..=5 {
            cache.remember(n, Fake(n), Some(toks(&[0, n as u32])));
        }
        cache.splice(3, 1, 1);
        assert_eq!(cache.filled(), 2);
        assert_eq!(cache.stack_at(3), Some(Fake(3)));
        assert_eq!(cache.tokens_at(4).as_deref(), Some(&[0, 4][..]));
    }

    #[test]
    fn retokenizes_every_replaced_line_when_the_first_reconverges() {
        let mut cache = TmStackCache::default();
        let mut texts = vec!["aa", "bb", "cc", "dd", "ee"];
        tokenize_line_range(
            |n| texts[n - 1].to_string(),
            &mut cache,
            fake_tokenize,
            1,
            5,
            true,
            None,
            None,
        )
        .unwrap();
        texts = vec!["aa", "bx", "cc", "dd", "eeee"];
        cache.splice(1, 5, 5);
        tokenize_line_range(
            |n| texts[n - 1].to_string(),
            &mut cache,
            fake_tokenize,
            1,
            5,
            true,
            None,
            None,
        )
        .unwrap();
        assert_eq!(cache.tokens_at(5).as_deref(), Some(&[0, 4][..]));
        assert_eq!(cache.filled(), 5);
    }

    #[test]
    fn shifts_later_checkpoints_when_a_line_is_inserted() {
        let mut cache = TmStackCache::default();
        for n in 1..=4 {
            cache.remember(n, Fake(n), Some(toks(&[0, n as u32])));
        }
        cache.splice(2, 0, 1);
        assert_eq!(cache.filled(), 1);
        assert_eq!(cache.stack_at(2), None);
        assert_eq!(cache.stack_at(3), Some(Fake(2)));
    }
}
