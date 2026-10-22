//! Optimistic resolve / unresolve of a line-note thread (frontend
//! `lib/pr-thread-resolved.ts`): the in-flight holds that stop a stale GetPR
//! flipping the button back, and how the service's answer merges into the
//! PR the UI already shows.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use recur_domain::{Pr, PrComment, PrReaction};
use recur_forge::view::apply_reaction_holds;

use super::comment_thread::{
    apply_thread_resolved, apply_thread_resolved_holds, keep_pending_replies, line_thread_key,
};

/// How long a settled toggle keeps overriding snapshots.
pub const THREAD_RESOLVED_HOLD_TTL: Duration = Duration::from_secs(120);

/// In-flight resolve toggles by thread key.
#[derive(Debug, Default)]
pub struct ThreadResolvedHolds(HashMap<String, (bool, Instant)>);

impl ThreadResolvedHolds {
    /// Starts a toggle on `c`'s thread and paints `comments`. `None` when the
    /// comment has no thread key or the same toggle is already in flight.
    pub fn begin(
        &mut self,
        comments: &mut [PrComment],
        c: &PrComment,
        resolved: bool,
    ) -> Option<String> {
        let key = line_thread_key(Some(c));
        if key.is_empty() || self.holds(&key) == Some(resolved) {
            return None;
        }
        self.0.insert(key.clone(), (resolved, Instant::now()));
        apply_thread_resolved(comments, &key, resolved);
        Some(key)
    }

    /// The live hold on `key`, if any.
    pub fn holds(&self, key: &str) -> Option<bool> {
        self.0
            .get(key)
            .filter(|(_, at)| at.elapsed() < THREAD_RESOLVED_HOLD_TTL)
            .map(|(r, _)| *r)
    }

    /// The write failed: drop the hold and paint the thread back. Returns
    /// false (nothing to do) when a newer toggle replaced this one.
    pub fn fail(&mut self, comments: &mut [PrComment], key: &str, resolved: bool) -> bool {
        if self.holds(key) != Some(resolved) {
            return false;
        }
        self.0.remove(key);
        apply_thread_resolved(comments, key, !resolved);
        true
    }

    /// The live holds as the paint map `apply_thread_resolved_holds` takes.
    pub fn active(&self) -> HashMap<String, bool> {
        self.0
            .iter()
            .filter(|(_, (_, at))| at.elapsed() < THREAD_RESOLVED_HOLD_TTL)
            .map(|(k, (r, _))| (k.clone(), *r))
            .collect()
    }

    pub fn reset(&mut self) {
        self.0.clear();
    }
}

/// The service's updated PR with the UI's pending replies, resolve holds
/// and reaction holds laid over its comments.
pub fn merge_resolved_update(
    prev_comments: &[PrComment],
    mut updated: Pr,
    holds: &ThreadResolvedHolds,
    reaction_holds: &HashMap<String, Vec<PrReaction>>,
) -> Pr {
    let mut comments =
        keep_pending_replies(prev_comments, updated.comments.take().unwrap_or_default());
    apply_thread_resolved_holds(&mut comments, &holds.active());
    apply_reaction_holds(&mut comments, reaction_holds);
    updated.comments = Some(comments);
    updated
}

pub fn thread_resolved_toast(resolved: bool) -> &'static str {
    if resolved {
        "Conversation resolved"
    } else {
        "Conversation reopened"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, thread: &str, resolved: bool) -> PrComment {
        PrComment {
            id: id.into(),
            thread_id: thread.into(),
            path: "a.ts".into(),
            line: 1,
            resolved,
            ..PrComment::default()
        }
    }

    #[test]
    fn begin_paints_and_dedupes() {
        let mut holds = ThreadResolvedHolds::default();
        let mut list = vec![note("gh-inline-1", "PRRT_1", false)];
        let first = list[0].clone();
        assert_eq!(
            holds.begin(&mut list, &first, true).as_deref(),
            Some("PRRT_1")
        );
        assert!(list[0].resolved);
        assert_eq!(
            holds.begin(&mut list, &first, true),
            None,
            "same toggle in flight"
        );
        assert_eq!(holds.begin(&mut list, &PrComment::default(), true), None);
        assert_eq!(holds.holds("PRRT_1"), Some(true));
    }

    #[test]
    fn failure_paints_back_unless_superseded() {
        let mut holds = ThreadResolvedHolds::default();
        let mut list = vec![note("gh-inline-1", "PRRT_1", false)];
        let c = list[0].clone();
        holds.begin(&mut list, &c, true);
        holds.begin(&mut list, &c, false);
        assert!(
            !holds.fail(&mut list, "PRRT_1", true),
            "a newer toggle owns the key"
        );
        assert!(holds.fail(&mut list, "PRRT_1", false));
        assert!(list[0].resolved);
        assert_eq!(holds.holds("PRRT_1"), None);
    }

    #[test]
    fn merge_keeps_holds_and_pending_replies() {
        let mut holds = ThreadResolvedHolds::default();
        let mut prev = vec![note("gh-inline-1", "PRRT_1", false)];
        let c = prev[0].clone();
        holds.begin(&mut prev, &c, true);
        prev.push(PrComment {
            id: "c-pending-1".into(),
            in_reply_to_id: "gh-inline-1".into(),
            body_md: "thanks".into(),
            ..PrComment::default()
        });
        let stale = Pr {
            id: "pr-1".into(),
            comments: Some(vec![note("gh-inline-1", "PRRT_1", false)]),
            ..Pr::default()
        };
        let rx = vec![PrReaction {
            content: "heart".into(),
            count: 1,
            mine: true,
        }];
        let reactions = HashMap::from([("gh-inline-1".to_owned(), rx.clone())]);
        let merged = merge_resolved_update(&prev, stale, &holds, &reactions);
        let cs = merged.comments.unwrap();
        assert_eq!(cs.len(), 2);
        assert!(cs[0].resolved);
        assert_eq!(cs[0].reactions, rx);
        assert_eq!(cs[1].id, "c-pending-1");
        holds.reset();
        assert!(holds.active().is_empty());
        assert_eq!(thread_resolved_toast(false), "Conversation reopened");
    }
}
