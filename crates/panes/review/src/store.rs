//! App-wide review state: in-flight resolve and reaction holds
//! (`lib/pr-thread-resolved.ts`, `lib/comment-reactions.ts`), unsent thread editors
//! (`threadEditorDrafts`) and the finding ids already posted as review drafts
//! (`postedFindingIds`), with the optimistic writes that use them.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui_kit::{App, Entity, Global, Task};
use recur_domain::{
    PendingLineComment, PrComment, PrReaction, normalize_reaction_content, toggle_reactions,
};
use recur_engine::view::comment_thread::{
    ThreadEditorDrafts, apply_thread_resolved_holds, keep_pending_replies,
};
use recur_engine::view::pr_thread_resolved::{
    ThreadResolvedHolds, merge_resolved_update, thread_resolved_toast,
};
use recur_forge::view::{apply_comment_reactions, apply_reaction_holds};
use recur_ui::widgets::ToastKind;
use recur_view_core::{AppState, call, toast};

use crate::findings::drop_finding_ids_removed_from_pending;

/// How long a settled reaction toggle keeps overriding snapshots.
pub const REACTION_HOLD_TTL: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct ReviewStore {
    pub thread_holds: ThreadResolvedHolds,
    reaction_holds: HashMap<String, (Vec<PrReaction>, Instant)>,
    pub drafts: ThreadEditorDrafts,
    pub posted_finding_ids: HashSet<String>,
}

impl Global for ReviewStore {}

impl ReviewStore {
    /// The store, `None` before the first write (every hold and draft empty).
    pub fn get(cx: &App) -> Option<&ReviewStore> {
        cx.try_global::<ReviewStore>()
    }

    pub fn global_mut(cx: &mut App) -> &mut ReviewStore {
        cx.default_global::<ReviewStore>()
    }

    /// The live reaction holds (`commentReactionHold`).
    pub fn reaction_holds(&self) -> HashMap<String, Vec<PrReaction>> {
        self.reaction_holds
            .iter()
            .filter(|(_, (_, at))| at.elapsed() < REACTION_HOLD_TTL)
            .map(|(k, (v, _))| (k.clone(), v.clone()))
            .collect()
    }

    fn reaction_hold(&self, id: &str) -> Option<&Vec<PrReaction>> {
        self.reaction_holds
            .get(id)
            .filter(|(_, at)| at.elapsed() < REACTION_HOLD_TTL)
            .map(|(v, _)| v)
    }

    /// `resetCommentReactionHolds` / `resetThreadResolvedHolds`.
    pub fn reset_holds(&mut self) {
        self.reaction_holds.clear();
        self.thread_holds.reset();
    }
}

fn live_reaction_holds(cx: &App) -> HashMap<String, Vec<PrReaction>> {
    ReviewStore::get(cx)
        .map(ReviewStore::reaction_holds)
        .unwrap_or_default()
}

/// `mergePRThreadComments`: keep optimistic replies and reaction holds over a GetPR snapshot.
pub fn merge_pr_thread_comments(
    prev: &[PrComment],
    next: Vec<PrComment>,
    cx: &App,
) -> Vec<PrComment> {
    let mut out = keep_pending_replies(prev, next);
    apply_reaction_holds(&mut out, &live_reaction_holds(cx));
    out
}

/// Every hold laid over a GetPR snapshot (resolve toggles, then reactions).
pub fn apply_review_holds(comments: &mut [PrComment], cx: &App) {
    if let Some(store) = ReviewStore::get(cx) {
        apply_thread_resolved_holds(comments, &store.thread_holds.active());
        apply_reaction_holds(comments, &store.reaction_holds());
    }
}

/// `patchPRPendingComments`: patch a PR's pending line comments locally so the diff and the
/// conversation update without GetPR; drafts the operator deleted lose their posted marker.
pub fn patch_pr_pending_comments(
    state: &Entity<AppState>,
    pr_id: &str,
    f: impl FnOnce(&[PendingLineComment]) -> Vec<PendingLineComment>,
    cx: &mut App,
) {
    let Some(prev) = state.read(cx).pr(pr_id).map(|p| p.pending_comments.clone()) else {
        return;
    };
    let next = f(&prev);
    let store = ReviewStore::global_mut(cx);
    if let Some(posted) =
        drop_finding_ids_removed_from_pending(&store.posted_finding_ids, &prev, &next)
    {
        store.posted_finding_ids = posted;
    }
    state.update(cx, |s, cx| {
        s.update_pr(pr_id, |p| p.pending_comments = next, cx)
    });
}

fn patch_comments(
    state: &Entity<AppState>,
    pr_id: &str,
    f: impl FnOnce(&mut Vec<PrComment>),
    cx: &mut App,
) {
    state.update(cx, |s, cx| {
        s.update_pr(
            pr_id,
            |p| {
                let mut list = p.comments.take().unwrap_or_default();
                f(&mut list);
                p.comments = Some(list);
            },
            cx,
        )
    });
}

/// `togglePRCommentReaction`: optimistic add/remove. Resolves false when the write did not
/// stick.
pub fn toggle_pr_comment_reaction(
    state: &Entity<AppState>,
    c: &PrComment,
    content: &str,
    cx: &mut App,
) -> Task<bool> {
    let id = recur_engine::view::js::trim(&c.id).to_string();
    let Some(n) = normalize_reaction_content(content) else {
        return Task::ready(false);
    };
    let pr_id = state.read(cx).selected_pr_id.clone();
    if id.is_empty() || pr_id.is_empty() {
        return Task::ready(false);
    }
    let next = toggle_reactions(&c.reactions, n);
    ReviewStore::global_mut(cx)
        .reaction_holds
        .insert(id.clone(), (next.clone(), Instant::now()));
    patch_comments(
        state,
        &pr_id,
        |list| apply_comment_reactions(list, &id, &next),
        cx,
    );
    let services = state.read(cx).services().clone();
    let state = state.clone();
    let before = c.reactions.clone();
    let n = n.to_string();
    cx.spawn(async move |cx| {
        let updated = call(
            cx,
            services.prs.toggle_pr_comment_reaction(&pr_id, &id, &n),
            "Could not update reaction",
        )
        .await;
        cx.update(|cx| {
            if ReviewStore::get(cx).and_then(|s| s.reaction_hold(&id)) != Some(&next) {
                return false;
            }
            let Some(updated) = updated else {
                ReviewStore::global_mut(cx).reaction_holds.remove(&id);
                patch_comments(
                    &state,
                    &pr_id,
                    |list| apply_comment_reactions(list, &id, &before),
                    cx,
                );
                return false;
            };
            let prev = state
                .read(cx)
                .pr(&updated.id)
                .and_then(|p| p.comments.clone())
                .unwrap_or_default();
            let mut updated = updated;
            let comments =
                merge_pr_thread_comments(&prev, updated.comments.take().unwrap_or_default(), cx);
            updated.comments = Some(comments);
            state.update(cx, |s, cx| {
                let uid = updated.id.clone();
                s.update_pr(&uid, |p| *p = updated, cx)
            });
            true
        })
    })
}

/// `setPRThreadResolved`: optimistic resolve / unresolve. Resolves false when the write did not
/// stick.
pub fn set_pr_thread_resolved(
    state: &Entity<AppState>,
    c: &PrComment,
    resolved: bool,
    cx: &mut App,
) -> Task<bool> {
    let pr_id = state.read(cx).selected_pr_id.clone();
    if pr_id.is_empty() {
        return Task::ready(false);
    }
    let mut key = None;
    state.update(cx, |s, cx| {
        let Some(pr) = s.prs.iter_mut().find(|p| p.id == pr_id) else {
            return;
        };
        let mut list = pr.comments.take().unwrap_or_default();
        key = cx
            .default_global::<ReviewStore>()
            .thread_holds
            .begin(&mut list, c, resolved);
        pr.comments = Some(list);
        cx.notify();
    });
    let Some(key) = key else {
        return Task::ready(false);
    };
    let services = state.read(cx).services().clone();
    let state = state.clone();
    cx.spawn(async move |cx| {
        let updated = call(
            cx,
            services.prs.set_pr_thread_resolved(&pr_id, &key, resolved),
            "Could not update conversation",
        )
        .await;
        cx.update(|cx| {
            if ReviewStore::get(cx).and_then(|s| s.thread_holds.holds(&key)) != Some(resolved) {
                return false;
            }
            let Some(updated) = updated else {
                state.update(cx, |s, cx| {
                    if let Some(pr) = s.prs.iter_mut().find(|p| p.id == pr_id) {
                        let mut list = pr.comments.take().unwrap_or_default();
                        cx.default_global::<ReviewStore>()
                            .thread_holds
                            .fail(&mut list, &key, resolved);
                        pr.comments = Some(list);
                        cx.notify();
                    }
                });
                return false;
            };
            let prev = state
                .read(cx)
                .pr(&updated.id)
                .and_then(|p| p.comments.clone())
                .unwrap_or_default();
            let store = ReviewStore::global_mut(cx);
            let reactions = store.reaction_holds();
            let merged = merge_resolved_update(&prev, updated, &store.thread_holds, &reactions);
            state.update(cx, |s, cx| {
                let uid = merged.id.clone();
                s.update_pr(&uid, |p| *p = merged, cx)
            });
            toast(ToastKind::Success, thread_resolved_toast(resolved), cx);
            true
        })
    })
}
