//! Idle-time background tokenization sweep (`tm-fill.ts`; VS Code's
//! DefaultBackgroundTokenizer). The host calls [`TmFill::run_idle`] from its
//! idle scheduling with a time-remaining probe; each call sweeps in ~1.5ms
//! slices and fans the changed line range out to the document's listeners.

use std::collections::HashMap;
use std::time::Duration;

use crate::engine::TmEngine;

pub const SLICE: Duration = Duration::from_micros(1500);

/// `(changed 1-based line range, done)`.
pub type FillListener = Box<dyn FnMut(Option<(usize, usize)>, bool)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FillSubscription(u64);

#[derive(Default)]
struct FillState {
    listeners: Vec<(u64, FillListener)>,
}

#[derive(Default)]
pub struct TmFill {
    fills: HashMap<String, FillState>,
    next_id: u64,
}

impl TmFill {
    /// Keep `doc_id` sweeping while at least one subscriber holds on.
    pub fn ensure(&mut self, doc_id: &str, listener: FillListener) -> FillSubscription {
        self.next_id += 1;
        let id = self.next_id;
        self.fills
            .entry(doc_id.to_string())
            .or_default()
            .listeners
            .push((id, listener));
        FillSubscription(id)
    }

    /// Drop a subscription; the sweep stops when the last one leaves.
    pub fn unsubscribe(&mut self, doc_id: &str, sub: FillSubscription) {
        if let Some(st) = self.fills.get_mut(doc_id) {
            st.listeners.retain(|(id, _)| *id != sub.0);
            if st.listeners.is_empty() {
                self.fills.remove(doc_id);
            }
        }
    }

    /// Documents with an active sweep.
    pub fn active_docs(&self) -> Vec<String> {
        self.fills.keys().cloned().collect()
    }

    pub fn is_active(&self, doc_id: &str) -> bool {
        self.fills.contains_key(doc_id)
    }

    /// One idle callback for `doc_id`. Returns true while more work remains.
    pub fn run_idle(
        &mut self,
        engine: &mut TmEngine,
        doc_id: &str,
        time_remaining: impl Fn() -> Duration,
    ) -> bool {
        if !self.fills.contains_key(doc_id) {
            return false;
        }
        let mut changed_from: Option<usize> = None;
        let mut changed_to = 0;
        let mut done;
        let mut last_filled: Option<usize> = None;
        loop {
            let p = engine.fill_slice(doc_id, SLICE);
            if let Some(f) = p.changed_from {
                changed_from = Some(changed_from.map_or(f, |c| c.min(f)));
                changed_to = changed_to.max(p.changed_to);
            }
            done = p.done;
            // No progress (grammar still loading): back off to the next slot.
            if done || last_filled == Some(p.filled) {
                break;
            }
            last_filled = Some(p.filled);
            if time_remaining() <= SLICE {
                break;
            }
        }
        let changed = changed_from.map(|f| (f, changed_to));
        if let Some(st) = self.fills.get_mut(doc_id) {
            for (_, listener) in st.listeners.iter_mut() {
                listener(changed, done);
            }
        }
        if done {
            self.fills.remove(doc_id);
        }
        !done && self.fills.contains_key(doc_id)
    }
}
