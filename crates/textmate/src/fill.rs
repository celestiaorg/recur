//! Background tokenization sweep (like VS Code's
//! DefaultBackgroundTokenizer). The host runs [`TmEngine::fill_job`]s off the
//! UI thread and hands each result to [`TmFill::apply`], which fans the
//! changed line range out to the document's listeners.

use std::collections::HashMap;

use crate::engine::{FillChunk, TmEngine};

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

    /// Apply a fill result for `doc_id`, or with `None` (no job could run)
    /// just report whether the sweep is done. Returns true while more work
    /// remains.
    pub fn apply(&mut self, engine: &mut TmEngine, doc_id: &str, chunk: Option<FillChunk>) -> bool {
        let Some(st) = self.fills.get_mut(doc_id) else {
            return false;
        };
        let (changed, done) = match chunk {
            Some(chunk) => {
                let p = engine.apply_fill(doc_id, chunk);
                (p.changed_from.map(|f| (f, p.changed_to)), p.done)
            }
            None => (
                None,
                engine.filled_through(doc_id) >= engine.line_count(doc_id),
            ),
        };
        for (_, listener) in st.listeners.iter_mut() {
            listener(changed, done);
        }
        if done {
            self.fills.remove(doc_id);
        }
        !done
    }
}
