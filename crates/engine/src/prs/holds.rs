//! Local holds over stale forge snapshots: a just-deleted comment, a
//! just-resolved thread and a just-toggled reaction win over a conversation
//! snapshot taken before the forge caught up, for [`HOLD_TTL`].

use std::time::Instant;

use recur_domain::{PrComment, PrReaction};

use crate::Engine;
use crate::prs::HOLD_TTL;
use crate::prs::ops::mark_thread_resolved;

fn hold_key(pr_id: &str, id: &str) -> String {
    format!("{pr_id}\x00{id}")
}

/// The ids held for `pr_id` (expired keys are dropped from `map`).
fn live_holds<V: Clone>(
    map: &mut std::collections::HashMap<String, (V, Instant)>,
    pr_id: &str,
) -> Vec<(String, V)> {
    let now = Instant::now();
    map.retain(|_, (_, at)| now.duration_since(*at) < HOLD_TTL);
    let prefix = hold_key(pr_id, "");
    map.iter()
        .filter_map(|(k, (v, _))| k.strip_prefix(&prefix).map(|id| (id.to_owned(), v.clone())))
        .collect()
}

impl Engine {
    pub(crate) fn remember_dropped_comments(&self, pr_id: &str, ids: &[String]) {
        if pr_id.is_empty() || ids.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut held = self.prs.dropped_comments.lock();
        for id in ids {
            let id = id.trim();
            if !id.is_empty() {
                held.insert(hold_key(pr_id, id), now);
            }
        }
    }

    pub(crate) fn forget_dropped_comments(&self, pr_id: &str, ids: &[String]) {
        let mut held = self.prs.dropped_comments.lock();
        for id in ids {
            held.remove(&hold_key(pr_id, id));
        }
    }

    /// Drops rows the viewer just deleted that a stale snapshot still lists.
    pub(crate) fn drop_remembered_comments(
        &self,
        pr_id: &str,
        cs: Vec<PrComment>,
    ) -> Vec<PrComment> {
        if cs.is_empty() {
            return cs;
        }
        let mut held = self.prs.dropped_comments.lock();
        if held.is_empty() {
            return cs;
        }
        let now = Instant::now();
        cs.into_iter()
            .filter(|c| {
                let key = hold_key(pr_id, &c.id);
                match held.get(&key) {
                    None => true,
                    Some(at) if now.duration_since(*at) >= HOLD_TTL => {
                        held.remove(&key);
                        true
                    }
                    Some(_) => false,
                }
            })
            .collect()
    }

    pub(crate) fn remember_thread_resolved(&self, pr_id: &str, thread_id: &str, resolved: bool) {
        if pr_id.is_empty() || thread_id.is_empty() {
            return;
        }
        self.prs
            .thread_resolved
            .lock()
            .insert(hold_key(pr_id, thread_id), (resolved, Instant::now()));
    }

    pub(crate) fn forget_thread_resolved(&self, pr_id: &str, thread_id: &str) {
        self.prs
            .thread_resolved
            .lock()
            .remove(&hold_key(pr_id, thread_id));
    }

    /// Re-applies the viewer's recent resolve/unresolve over a snapshot.
    pub(crate) fn apply_remembered_thread_resolved(
        &self,
        pr_id: &str,
        mut cs: Vec<PrComment>,
    ) -> Vec<PrComment> {
        if cs.is_empty() {
            return cs;
        }
        let holds = live_holds(&mut self.prs.thread_resolved.lock(), pr_id);
        for (id, resolved) in holds {
            mark_thread_resolved(&mut cs, &id, resolved);
        }
        cs
    }

    pub(crate) fn remember_comment_reactions(
        &self,
        pr_id: &str,
        comment_id: &str,
        rs: &[PrReaction],
    ) {
        if pr_id.is_empty() || comment_id.is_empty() {
            return;
        }
        self.prs
            .comment_reactions
            .lock()
            .insert(hold_key(pr_id, comment_id), (rs.to_vec(), Instant::now()));
    }

    pub(crate) fn forget_comment_reactions(&self, pr_id: &str, comment_id: &str) {
        self.prs
            .comment_reactions
            .lock()
            .remove(&hold_key(pr_id, comment_id));
    }

    /// Re-applies the viewer's recent reaction toggles over a snapshot.
    pub(crate) fn apply_remembered_comment_reactions(
        &self,
        pr_id: &str,
        mut cs: Vec<PrComment>,
    ) -> Vec<PrComment> {
        if cs.is_empty() {
            return cs;
        }
        let holds = live_holds(&mut self.prs.comment_reactions.lock(), pr_id);
        for (id, rs) in holds {
            for c in cs.iter_mut().filter(|c| c.id == id) {
                c.reactions = rs.clone();
            }
        }
        cs
    }
}
