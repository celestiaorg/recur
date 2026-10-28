//! Incremental rule-stack + token checkpoints (`cm-tm-stacks.ts`).
//!
//! Line N+1 needs the stack after line N. After an edit, stale stacks are
//! kept so re-tokenizing can stop as soon as a line ends in the same state
//! it did before (usually 1-3 lines, not the whole viewport).

use std::cmp::Ordering;
use std::rc::Rc;
use std::time::Instant;

use sum_tree::{Bias, ContextLessSummary, Item, SeekTarget, SumTree};

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

pub struct TmStackCache<S: Clone = StateStack> {
    /// Item `n - 1` holds 1-based line n's stack and tokens.
    lines: SumTree<Slot<S>>,
    filled_through: usize,
}

impl<S: Clone> Default for TmStackCache<S> {
    fn default() -> Self {
        TmStackCache {
            lines: SumTree::default(),
            filled_through: 0,
        }
    }
}

#[derive(Clone)]
struct Slot<S> {
    after: Option<S>,
    tokens: Option<LineTokens>,
}

impl<S> Default for Slot<S> {
    fn default() -> Self {
        Slot {
            after: None,
            tokens: None,
        }
    }
}

/// Lines, and lines missing a stack or tokens.
#[derive(Clone, Copy, Debug, Default)]
struct SlotSummary {
    lines: usize,
    gaps: usize,
}

impl ContextLessSummary for SlotSummary {
    fn zero() -> Self {
        SlotSummary::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.lines += other.lines;
        self.gaps += other.gaps;
    }
}

impl<S: Clone> Item for Slot<S> {
    type Summary = SlotSummary;

    fn summary(&self, _: ()) -> SlotSummary {
        SlotSummary {
            lines: 1,
            gaps: usize::from(self.after.is_none() || self.tokens.is_none()),
        }
    }
}

struct Lines(usize);
struct Gaps(usize);

impl SeekTarget<'_, SlotSummary, SlotSummary> for Lines {
    fn cmp(&self, at: &SlotSummary, _: ()) -> Ordering {
        self.0.cmp(&at.lines)
    }
}

impl SeekTarget<'_, SlotSummary, SlotSummary> for Gaps {
    fn cmp(&self, at: &SlotSummary, _: ()) -> Ordering {
        self.0.cmp(&at.gaps)
    }
}

impl<S: Clone> TmStackCache<S> {
    fn slot(&self, line: usize) -> Option<&Slot<S>> {
        let index = line.checked_sub(1)?;
        self.lines
            .find::<SlotSummary, _>((), &Lines(index), Bias::Right)
            .2
    }

    /// Replace items `from..to` (0-based) with `items(old)`.
    fn replace(
        &mut self,
        from: usize,
        to: usize,
        items: impl FnOnce(SumTree<Slot<S>>) -> Vec<Slot<S>>,
    ) {
        let mut cursor = self.lines.cursor::<SlotSummary>(());
        let mut out = cursor.slice(&Lines(from), Bias::Right);
        let old = cursor.slice(&Lines(to), Bias::Right);
        out.extend(items(old), ());
        out.append(cursor.suffix(), ());
        drop(cursor);
        self.lines = out;
    }

    pub fn filled(&self) -> usize {
        self.filled_through
    }

    /// Line to start tokenizing from, and the stack entering it (`None` =
    /// the initial state).
    pub fn resume_at(&self, from_line: usize) -> (usize, Option<S>) {
        let before = (from_line.max(1) - 1).min(self.filled_through);
        if before >= 1
            && let Some(stack) = self.stack_at(before)
        {
            return (before + 1, Some(stack));
        }
        (1, None)
    }

    pub fn remember(&mut self, line: usize, stack: S, tokens: Option<LineTokens>) {
        if line < 1 {
            return;
        }
        let len = self.lines.summary().lines;
        if line > len {
            self.lines
                .extend(std::iter::repeat_with(Slot::default).take(line - len), ());
        }
        self.replace(line - 1, line, |old| {
            let old = old.first().cloned().unwrap_or_default();
            vec![Slot {
                after: Some(stack),
                tokens: tokens.or(old.tokens),
            }]
        });
        if line == self.filled_through + 1 {
            self.filled_through = line;
        }
    }

    pub fn stack_at(&self, line: usize) -> Option<S> {
        self.slot(line)?.after.clone()
    }

    pub fn tokens_at(&self, line: usize) -> Option<LineTokens> {
        self.slot(line)?.tokens.clone()
    }

    pub fn has_tokens(&self, from_line: usize, to_line: usize) -> bool {
        if from_line > to_line {
            return true;
        }
        if from_line == 0 {
            return false;
        }
        let mut cursor = self.lines.cursor::<SlotSummary>(());
        cursor.seek(&Lines(from_line - 1), Bias::Right);
        for _ in from_line..=to_line {
            if cursor.item().is_none_or(|s| s.tokens.is_none()) {
                return false;
            }
            cursor.next();
        }
        true
    }

    /// Line `n` ended in its previous state, so later cached lines are valid
    /// again while they still have both a stack and tokens.
    pub fn mark_stable(&mut self, n: usize) {
        let (before, _, _) = self
            .lines
            .find::<SlotSummary, _>((), &Lines(n), Bias::Right);
        let (gap, _, _) = self
            .lines
            .find::<SlotSummary, _>((), &Gaps(before.gaps), Bias::Right);
        self.filled_through = self.filled_through.max(gap.lines.max(n));
    }

    /// 1-based splice matching a document line replace. Same-length replaces
    /// keep stale stacks (for early-stop) but drop their tokens.
    pub fn splice(&mut self, from_line: usize, delete_count: usize, insert_count: usize) {
        let i = from_line.max(1) - 1;
        if delete_count != insert_count {
            self.replace(i, i + delete_count, |_| {
                std::iter::repeat_with(Slot::default)
                    .take(insert_count)
                    .collect()
            });
        } else {
            self.replace(i, i + insert_count, |old| {
                old.iter()
                    .map(|s| Slot {
                        after: s.after.clone(),
                        tokens: None,
                    })
                    .collect()
            });
        }
        self.filled_through = self.filled_through.min(i);
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

    /// The `Vec` layout the cache had before its sum tree, as a model.
    #[derive(Default)]
    struct Naive {
        after: Vec<Option<usize>>,
        tokens: Vec<Option<u32>>,
        filled: usize,
    }

    impl Naive {
        fn stable_end(&self, n: usize) -> usize {
            let mut end = n;
            while self.after.get(end).is_some_and(Option::is_some)
                && self.tokens.get(end).is_some_and(Option::is_some)
            {
                end += 1;
            }
            end
        }
    }

    fn splice_vec<T>(v: &mut Vec<Option<T>>, i: usize, del: usize, ins: usize) {
        let i = i.min(v.len());
        let end = (i + del).min(v.len());
        v.splice(i..end, std::iter::repeat_with(|| None).take(ins));
    }

    #[test]
    fn matches_a_vec_under_random_edits() {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut r = move |n: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % n.max(1) as u64) as usize
        };
        let mut cache = TmStackCache::default();
        let mut naive = Naive::default();
        for _ in 0..5000 {
            let len = naive.after.len().max(naive.tokens.len());
            match r(4) {
                0 | 1 => {
                    let line = r(len + 3) + 1;
                    let tok = (r(2) == 0).then_some(line as u32);
                    cache.remember(line, Fake(line), tok.map(|t| toks(&[t])));
                    let i = line - 1;
                    if naive.after.len() <= i {
                        naive.after.resize(i + 1, None);
                    }
                    naive.after[i] = Some(line);
                    if let Some(t) = tok {
                        if naive.tokens.len() <= i {
                            naive.tokens.resize(i + 1, None);
                        }
                        naive.tokens[i] = Some(t);
                    }
                    if line == naive.filled + 1 {
                        naive.filled = line;
                    }
                }
                2 => {
                    let from = r(len + 2) + 1;
                    let (del, ins) = (r(4), r(4));
                    cache.splice(from, del, ins);
                    if del != ins {
                        splice_vec(&mut naive.after, from - 1, del, ins);
                        splice_vec(&mut naive.tokens, from - 1, del, ins);
                    } else {
                        for t in naive.tokens.iter_mut().skip(from - 1).take(ins) {
                            *t = None;
                        }
                    }
                    naive.filled = naive.filled.min(from - 1);
                }
                _ => {
                    let n = r(len + 2);
                    cache.mark_stable(n);
                    naive.filled = naive.filled.max(naive.stable_end(n));
                }
            }
            assert_eq!(cache.filled(), naive.filled);
            for n in 0..=len + 2 {
                let i = n.wrapping_sub(1);
                assert_eq!(
                    cache.stack_at(n),
                    naive.after.get(i).copied().flatten().map(Fake)
                );
                assert_eq!(
                    cache.tokens_at(n).map(|t| t[0]),
                    naive.tokens.get(i).copied().flatten()
                );
            }
            let (a, b) = (r(len + 2), r(len + 2));
            let want = (a..=b).all(|n| {
                naive
                    .tokens
                    .get(n.wrapping_sub(1))
                    .is_some_and(Option::is_some)
            });
            assert_eq!(cache.has_tokens(a, b), want);
        }
    }

    /// `cargo test --release -p recur-textmate -- --ignored --nocapture splice_timing`
    #[test]
    #[ignore]
    fn splice_timing() {
        let mut cache = TmStackCache::default();
        for n in 1..=1_000_000 {
            cache.remember(n, Fake(n), Some(toks(&[0])));
        }
        let t = std::time::Instant::now();
        for k in 0..1000 {
            let n = 1 + k * 997 % 999_000;
            cache.splice(n, 1, 2);
            cache.remember(n, Fake(n), Some(toks(&[0])));
            cache.remember(n + 1, Fake(n), Some(toks(&[0])));
            cache.mark_stable(n + 1);
        }
        println!(
            "TmStackCache 1M lines: {:?} per newline",
            t.elapsed() / 1000
        );
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
