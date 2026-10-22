//! Pull requests (slice E5): Local PRs the engine owns end to end, and forge
//! PRs it mirrors through the registered [`recur_forge::Forge`] whose
//! `pr_source` matches `Pr.source`. Go `pr.go` (minus the walk's open/wrap
//! up), `rework.go`, `workspace_base.go`, `workspace_pr.go` and the engine
//! halves of `github*.go` and `workspace_remote.go`.
//!
//! The engine never names a forge (R8): user-facing sentences take the
//! forge's `label()`, refspecs come from `head_ref`, and comment ids, merge
//! plans and API calls stay inside the forge.

pub mod forge_glue;
pub mod from_pr;
pub mod holds;
pub mod linking;
pub mod merge;
pub mod ops;
pub mod refresh;
pub mod sync;
pub mod workspace;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use recur_domain::{Pr, PrComment, PrSource};
use recur_forge::Forge;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::bus::{EngineEvent, PrCommentEvent};
use crate::text::unix_nanos;

pub use recur_forge::model::{GROUP_LOOP, GROUP_MERGED, GROUP_OPEN, GROUP_YOU, pr_settled};

/// How often the PR list sweep may run again (Go `githubPRSyncMinInterval`).
pub const PR_SYNC_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// The background re-list cadence (Go `githubPRSweepInterval`).
pub const PR_SWEEP_INTERVAL: Duration = Duration::from_secs(180);
/// Projects listed at once by one sweep (two list calls each).
pub const PR_SYNC_PROJECT_CONCURRENCY: usize = 8;
/// How often a conversation snapshot may start per PR.
pub const PR_CONVERSATION_MIN_INTERVAL: Duration = Duration::from_secs(20);
/// How often a diff hydration may start per PR.
pub const PR_DIFF_MIN_INTERVAL: Duration = Duration::from_secs(60);
/// How long a stored branch-rules probe is fresh.
pub const BRANCH_RULES_TTL: Duration = Duration::from_secs(600);
/// How long local holds (dropped comments, resolves, reactions) win over a
/// stale snapshot.
pub const HOLD_TTL: Duration = Duration::from_secs(120);

/// The intervals tests shorten (Go's package-level vars).
#[derive(Debug, Clone, Copy)]
pub(crate) struct PrTiming {
    pub sync_min_interval: Duration,
    pub sweep_interval: Duration,
    pub branch_rules_ttl: Duration,
}

impl Default for PrTiming {
    fn default() -> Self {
        Self {
            sync_min_interval: PR_SYNC_MIN_INTERVAL,
            sweep_interval: PR_SWEEP_INTERVAL,
            branch_rules_ttl: BRANCH_RULES_TTL,
        }
    }
}

/// The single-flight PR list sweep: when the last one finished, and the
/// in-flight one callers wait on.
#[derive(Default)]
pub(crate) struct SyncFlight {
    pub at: Option<Instant>,
    pub wait: Option<CancellationToken>,
}

/// E5's engine state (Go's PR fields on `Engine`).
#[derive(Default)]
pub(crate) struct PrState {
    /// Leftover PR-host tab key → the live worktree tab it was hoisted onto.
    pub chat_hoist_alias: Mutex<HashMap<String, String>>,
    pub sync: Mutex<SyncFlight>,
    pub conv_last: Mutex<HashMap<String, Instant>>,
    pub conv_busy: Mutex<HashSet<String>>,
    pub diff_last: Mutex<HashMap<String, Instant>>,
    pub diff_busy: Mutex<HashSet<String>>,
    pub dropped_comments: Mutex<HashMap<String, Instant>>,
    pub thread_resolved: Mutex<HashMap<String, (bool, Instant)>>,
    pub comment_reactions: Mutex<HashMap<String, (Vec<recur_domain::PrReaction>, Instant)>>,
    pub branch_rules_inflight: Mutex<HashSet<String>>,
    pub timing: Mutex<PrTiming>,
}

/// Records last-activity time for a genuine mutation (review, comment,
/// rework, merge). Cache fills must not call this: they would float the
/// row to the top of "Recently updated".
pub fn touch_pr_updated(p: &mut Pr) {
    p.updated_at = recur_forge::model::format_time(chrono::Utc::now());
}

/// A thread row with a stable id and the avatar fill used by the seeded
/// thread; `code` is the two-letter avatar.
pub fn now_comment(code: &str, author: &str, body_html: &str) -> PrComment {
    PrComment {
        id: format!("c-{}", unix_nanos()),
        code: code.to_owned(),
        author: author.to_owned(),
        avatar_css: recur_forge::model::comment_avatar_css(code).to_owned(),
        time: "now".into(),
        verb: "commented".into(),
        body_html: body_html.to_owned(),
        ..PrComment::default()
    }
}

/// Go `normalizePRURL`.
pub fn normalize_pr_url(u: &str) -> String {
    u.trim().to_lowercase().trim_end_matches('/').to_owned()
}

/// The comments of a PR (Go's nil slice is empty).
pub(crate) fn comments(p: &Pr) -> &[PrComment] {
    p.comments.as_deref().unwrap_or_default()
}

pub(crate) fn none_if_empty<T>(v: Vec<T>) -> Option<Vec<T>> {
    (!v.is_empty()).then_some(v)
}

impl Engine {
    /// The forge that owns this PR row; `None` for a Local PR (or a source
    /// no registered forge claims).
    pub fn pr_forge(&self, pr: &Pr) -> Option<Arc<dyn Forge>> {
        if pr.source == PrSource::Local {
            return None;
        }
        self.forges.for_pr_source(&pr.source)
    }

    /// The forge a project's PRs open on: the one that recognizes the
    /// project's default remote, else the first registered forge (the only
    /// one today).
    pub async fn project_forge(&self, repo_path: &str) -> Option<Arc<dyn Forge>> {
        let all = self.forges.all();
        if all.len() <= 1 {
            return all.into_iter().next();
        }
        if !repo_path.trim().is_empty() {
            let mgr = recur_git::Manager::new(repo_path, &self.worktree_root);
            let remote = default_remote(&mgr).await;
            if !remote.is_empty()
                && let Ok(out) = recur_git::command(["remote", "get-url", remote.as_str()])
                    .current_dir(repo_path)
                    .timeout(Duration::from_secs(5))
                    .output()
                    .await
                && let Some((f, _)) = self.forges.detect(String::from_utf8_lossy(&out).trim())
            {
                return Some(f);
            }
        }
        all.into_iter().next()
    }

    /// The user-facing name of the forge a project opens PRs on.
    pub(crate) async fn project_forge_label(&self, repo_path: &str) -> String {
        match self.project_forge(repo_path).await {
            Some(f) => f.label().to_owned(),
            None => String::new(),
        }
    }

    /// The Recur project this PR belongs to. Imported PRs stamp it;
    /// Recur-opened PRs inherit it from the workspace.
    pub fn pr_project_id(&self, pr: &Pr) -> String {
        let id = pr.project_id.trim();
        if !id.is_empty() {
            return id.to_owned();
        }
        self.loop_project_id(&pr.workspace_id)
    }

    /// The project id of a workspace ("" when gone).
    pub(crate) fn loop_project_id(&self, workspace_id: &str) -> String {
        self.store
            .workspace(workspace_id)
            .map(|w| w.project_id)
            .unwrap_or_default()
    }

    /// The project repo a PR's forge CLI runs in (and its git root).
    pub fn pr_repo_path(&self, pr: &Pr) -> String {
        let id = self.pr_project_id(pr);
        if id.is_empty() {
            return String::new();
        }
        self.store
            .project(&id)
            .map(|p| p.repo_path)
            .unwrap_or_default()
    }

    /// `id` when that workspace still exists, is live and is not a PR chat
    /// host; otherwise empty so import can attach a different match.
    pub fn live_pr_workspace_id(&self, id: &str) -> String {
        let id = id.trim();
        if id.is_empty() {
            return String::new();
        }
        match self.store.workspace(id) {
            Some(w) if !w.archived && !w.is_pr_chat_host() => w.id,
            _ => String::new(),
        }
    }

    /// `pr.updated` for a local pending-comment change: the UI merges
    /// `pendingComments` and skips refetching.
    pub fn publish_pr_pending_updated(&self, pr_id: &str) {
        self.emit_pr_updated(pr_id, true);
    }

    /// `pr.updated` after a local thread write (reply, edit, delete): the
    /// call already returned the new row.
    pub fn publish_pr_thread_updated(&self, pr_id: &str) {
        self.emit_pr_updated(pr_id, true);
    }

    /// `pr.comment` for a new (already persisted) thread entry.
    pub fn publish_pr_comment(&self, pr_id: &str, c: &PrComment) {
        self.bus.publish(EngineEvent::PrComment(PrCommentEvent {
            comment: c.clone(),
            pr_id: pr_id.to_owned(),
        }));
    }
}

/// Go `githubDefaultRemote`: origin, else the first configured remote.
pub(crate) async fn default_remote(mgr: &recur_git::Manager) -> String {
    let Ok(remotes) = mgr.remotes().await else {
        return String::new();
    };
    if remotes.iter().any(|r| r == "origin") {
        return "origin".into();
    }
    remotes.into_iter().next().unwrap_or_default()
}

/// Test seams (Go's `githubPRSyncMinInterval`, `githubPRSweepInterval`,
/// `githubBranchPRTTL` and the hold timestamps).
#[cfg(any(test, feature = "testing"))]
impl Engine {
    pub fn set_pr_sync_min_interval(&self, d: Duration) {
        self.prs.timing.lock().sync_min_interval = d;
    }

    pub fn set_pr_sweep_interval(&self, d: Duration) {
        self.prs.timing.lock().sweep_interval = d;
    }

    pub fn set_branch_rules_ttl(&self, d: Duration) {
        self.prs.timing.lock().branch_rules_ttl = d;
    }

    /// Backdates every PR hold and throttle stamp by `by` (Go tests write
    /// `time.Now().Add(-…)` into the maps).
    pub fn age_pr_holds(&self, by: Duration) {
        let back = |t: &mut Instant| {
            if let Some(b) = t.checked_sub(by) {
                *t = b;
            }
        };
        self.prs.dropped_comments.lock().values_mut().for_each(back);
        self.prs
            .thread_resolved
            .lock()
            .values_mut()
            .for_each(|(_, t)| back(t));
        self.prs
            .comment_reactions
            .lock()
            .values_mut()
            .for_each(|(_, t)| back(t));
        self.prs.conv_last.lock().values_mut().for_each(back);
        self.prs.diff_last.lock().values_mut().for_each(back);
    }
}
