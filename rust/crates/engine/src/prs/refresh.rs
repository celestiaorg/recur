//! Keeping a stored PR current (Go `pr.go` refreshers, `github_thread.go`
//! `RefreshPRConversation`, `github_prs.go` diff hydration): the forge's
//! conversation and review state, commits from the worktree, one commit's
//! diff on demand, the behind-base count and the check rollup. Every
//! refresh is best effort: a failed read leaves the stored row alone.

use std::path::Path;
use std::time::{Duration, Instant};

use recur_domain::{Commit, DiffStat, Pr};
use recur_forge::model::{
    apply_commit_avatars, comments_equal, commits_equal, conversation_attention,
    conversation_state, enrich_commits, merge_participants, normalize_time, snapshot_participants,
    snapshot_reviewers,
};

use crate::Engine;
use crate::core::{EngineError, Result};
use crate::prs::{PR_CONVERSATION_MIN_INTERVAL, PR_DIFF_MIN_INTERVAL, comments, none_if_empty};

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const DIFF_TIMEOUT: Duration = Duration::from_secs(20);
const COMMITS_TIMEOUT: Duration = Duration::from_secs(15);
const BEHIND_TIMEOUT: Duration = Duration::from_secs(20);
const CHECKS_TIMEOUT: Duration = Duration::from_secs(15);
const HEAD_FETCH_TIMEOUT: Duration = Duration::from_secs(4);

/// Go `Manager.BehindBase`'s three results: the ref to merge and the count;
/// an error answers (base, 0) unless the inputs were missing.
pub(crate) async fn behind_base(
    mgr: &recur_git::Manager,
    worktree: &str,
    base: &str,
    fetch: bool,
) -> (String, i64, bool) {
    if worktree.is_empty() || base.trim().is_empty() {
        return (String::new(), 0, false);
    }
    match mgr.behind_base(worktree, base, fetch).await {
        Ok((r, n)) => (r, n, true),
        Err(_) => (base.trim().to_owned(), 0, false),
    }
}

/// The stored commit matching `r` by short or full hash (prefixes of 7+).
pub fn find_pr_commit(commits: &[Commit], r: &str) -> Option<(usize, Commit)> {
    commits
        .iter()
        .position(|c| commit_ref_matches(c, r))
        .map(|i| (i, commits[i].clone()))
}

/// Whether `r` names this commit (exact, or a 7+ char prefix either way).
pub fn commit_ref_matches(c: &Commit, r: &str) -> bool {
    let r = r.trim().to_lowercase();
    if r.is_empty() {
        return false;
    }
    let h = c.hash.trim().to_lowercase();
    let s = c.sha.trim().to_lowercase();
    if h == r || s == r {
        return true;
    }
    if r.len() < 7 {
        return false;
    }
    (!h.is_empty() && (h.starts_with(&r) || r.starts_with(&h)))
        || (!s.is_empty() && (s.starts_with(&r) || r.starts_with(&s)))
}

/// Overlays a fresh `git log` onto the stored list: files and the
/// forge-only fields stay on the matching stored row, so `git show` does
/// not rerun and verified/check state is kept.
pub fn merge_commit_log(existing: &[Commit], fresh: Vec<Commit>) -> Vec<Commit> {
    let mut by_short: Vec<(String, &Commit)> = Vec::new();
    let mut by_full: std::collections::HashMap<String, &Commit> = std::collections::HashMap::new();
    for c in existing {
        let h = c.hash.trim();
        if !h.is_empty() {
            match by_short.iter_mut().find(|(k, _)| k == h) {
                Some(e) => e.1 = c,
                None => by_short.push((h.to_owned(), c)),
            }
        }
        let s = c.sha.trim();
        if !s.is_empty() {
            by_full.insert(s.to_owned(), c);
        }
    }
    fresh
        .into_iter()
        .map(|mut n| {
            let mut old = by_full.get(n.sha.trim()).copied();
            if old.is_none() {
                old = by_short
                    .iter()
                    .find(|(k, _)| k == n.hash.trim())
                    .map(|(_, c)| *c);
            }
            if old.is_none() && n.hash.len() >= 7 {
                old = by_short
                    .iter()
                    .find(|(h, c)| {
                        c.sha.starts_with(&n.hash)
                            || n.sha.starts_with(h.as_str())
                            || h.starts_with(&n.hash)
                    })
                    .map(|(_, c)| *c);
            }
            if let Some(o) = old {
                n.files = o.files.clone();
                n.avatar_url = o.avatar_url.clone();
                n.verified = o.verified;
                n.check_status = o.check_status.clone();
            }
            n
        })
        .collect()
}

impl Engine {
    /// The worktree and project a PR's local refreshers read, with the
    /// base to compare against (PR, then workspace, then project).
    fn pr_worktree_base(&self, pr: &Pr) -> Option<(String, String, String)> {
        let lp = self
            .store
            .workspace(&self.live_pr_workspace_id(&pr.workspace_id))?;
        if lp.worktree_path.is_empty() {
            return None;
        }
        let proj = self.store.project(&lp.project_id)?;
        if proj.repo_path.is_empty() {
            return None;
        }
        let base = [&pr.base, &lp.base_branch, &proj.base_branch]
            .into_iter()
            .find(|b| !b.is_empty())?
            .clone();
        Some((proj.repo_path, lp.worktree_path, base))
    }

    /// Reloads commit metadata from the worktree, keeping per-commit diffs
    /// and forge fields for hashes that still match.
    pub async fn refresh_pr_commits(&self, pr_id: &str) {
        let Some(pr) = self.store.pr(pr_id) else {
            return;
        };
        if !pr.updating_base.is_empty() {
            return;
        }
        let Some((repo, wt, base)) = self.pr_worktree_base(&pr) else {
            return;
        };
        let mgr = recur_git::Manager::new(&repo, &self.worktree_root);
        let fresh = match tokio::time::timeout(COMMITS_TIMEOUT, mgr.log_commits(&wt, &base)).await {
            Ok(Ok(f)) if !f.is_empty() => f,
            _ => return,
        };
        let stored = pr.commits.clone().unwrap_or_default();
        let merged = merge_commit_log(&stored, fresh);
        if commits_equal(&stored, &merged) {
            return;
        }
        self.store.update_pr(pr_id, |p| p.commits = Some(merged));
    }

    /// Fills one commit's file diff (`git show --first-parent`) when the
    /// stored row has none. An empty (not missing) list is a cache hit.
    /// Missing worktrees fall back to the project repo so archived PRs
    /// still open; a forge PR head is fetched when the commit is unknown.
    pub async fn ensure_commit_diff(&self, pr_id: &str, hash: &str) -> Result<Commit> {
        let pr = self
            .store
            .pr(pr_id)
            .ok_or_else(|| EngineError(format!("PR {pr_id:?} not found")))?;
        let (_, mut c) = find_pr_commit(pr.commits.as_deref().unwrap_or_default(), hash)
            .ok_or_else(|| EngineError(format!("commit {hash:?} not found on PR {pr_id:?}")))?;
        if let Some(files) = c.files.as_mut().filter(|f| !f.is_empty()) {
            if recur_git::highlight_files(files) {
                let files = files.clone();
                if let Some(updated) = self.store.update_pr(pr_id, |p| {
                    if let Some(cs) = p.commits.as_mut()
                        && let Some((i, _)) = find_pr_commit(cs, hash)
                    {
                        cs[i].files = Some(files);
                    }
                }) && let Some((_, filled)) =
                    find_pr_commit(updated.commits.as_deref().unwrap_or_default(), hash)
                {
                    return Ok(filled);
                }
            }
            return Ok(c);
        }
        if c.files.is_some() {
            // Already shown: a merge or empty commit.
            return Ok(c);
        }
        let lp = self
            .store
            .workspace(&self.live_pr_workspace_id(&pr.workspace_id));
        let proj = self
            .store
            .project(&self.pr_project_id(&pr))
            .filter(|p| !p.repo_path.trim().is_empty())
            .ok_or_else(|| EngineError(format!("commit {hash:?}: no repo for PR {pr_id:?}")))?;
        let r = [c.sha.trim(), c.hash.trim(), hash.trim()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default()
            .to_owned();
        let repo = proj.repo_path.trim().to_owned();
        let mut dir = lp
            .map(|w| w.worktree_path.trim().to_owned())
            .unwrap_or_default();
        if dir.is_empty() {
            dir = repo.clone();
        }
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let work = async {
            if let Some(forge) = self.pr_forge(&pr)
                && pr.number > 0
                && dir == repo
                && !mgr.commit_exists(&r).await
            {
                let _ = tokio::time::timeout(
                    HEAD_FETCH_TIMEOUT,
                    crate::prs::forge_glue::fetch_forge_pr_head(
                        &mgr,
                        &repo,
                        &forge.head_ref(pr.number),
                        pr.number,
                    ),
                )
                .await;
            }
            let mut res = mgr.commit_diff_first_parent(&dir, &r).await;
            if res.is_err() && dir != repo {
                res = mgr.commit_diff_first_parent(&repo, &r).await;
            }
            res.map_err(|e| e.to_string())
        };
        let mut files = tokio::time::timeout(COMMITS_TIMEOUT, work)
            .await
            .unwrap_or_else(|_| Err("context deadline exceeded".into()))
            .map_err(|e| EngineError(format!("commit diff {r}: {e}")))?;
        recur_git::highlight_files(&mut files);
        let filled_files = files.clone();
        let updated = self.store.update_pr(pr_id, |p| {
            if let Some(cs) = p.commits.as_mut()
                && let Some((i, _)) = find_pr_commit(cs, hash)
            {
                cs[i].files = Some(filled_files);
            }
        });
        if let Some(updated) = updated
            && let Some((_, filled)) =
                find_pr_commit(updated.commits.as_deref().unwrap_or_default(), hash)
        {
            return Ok(filled);
        }
        c.files = Some(files);
        Ok(c)
    }

    /// Recomputes how far the merge-base tip is ahead of the PR branch,
    /// fetching the remote-tracking counterpart of the stored base first.
    pub async fn refresh_pr_behind(&self, pr_id: &str) {
        let Some(pr) = self.store.pr(pr_id) else {
            return;
        };
        if pr.state == "Merged" || pr.group == "merged" || !pr.updating_base.is_empty() {
            return;
        }
        let Some((repo, wt, base)) = self.pr_worktree_base(&pr) else {
            return;
        };
        let mgr = recur_git::Manager::new(&repo, &self.worktree_root);
        let Ok((_, behind, ok)) =
            tokio::time::timeout(BEHIND_TIMEOUT, behind_base(&mgr, &wt, &base, true)).await
        else {
            return;
        };
        if !ok || pr.behind_base == behind {
            return;
        }
        self.store.update_pr(pr_id, |p| p.behind_base = behind);
    }

    /// Replaces a forge PR's checks with the forge's live rollup. Local
    /// PRs, merged PRs and fetch errors leave the stored checks alone.
    pub async fn refresh_pr_checks(&self, pr_id: &str) {
        let Some(pr) = self.store.pr(pr_id) else {
            return;
        };
        if pr.state == "Merged" || pr.group == "merged" || pr.number <= 0 {
            return;
        }
        let Some(forge) = self.pr_forge(&pr) else {
            return;
        };
        let repo = self.pr_repo_path(&pr);
        if repo.is_empty() {
            return;
        }
        let Ok(Ok(report)) = tokio::time::timeout(
            CHECKS_TIMEOUT,
            forge.checks(Path::new(&repo), pr.number, &pr.url),
        )
        .await
        else {
            return;
        };
        let stored = pr.checks.clone().unwrap_or_default();
        let has = !report.checks.is_empty();
        if pr.checks_ok == report.ok && stored == report.checks && pr.has_checks == has {
            return;
        }
        self.store.update_pr(pr_id, |p| {
            p.checks = Some(report.checks);
            p.checks_ok = report.ok;
            p.has_checks = has;
        });
    }

    /// Hydrates a forge PR's files in the background, at most once per
    /// [`PR_DIFF_MIN_INTERVAL`] (a failure also counts) and single-flight.
    pub fn schedule_refresh_pr_diff(&self, pr_id: &str) {
        let Some(pr) = self.store.pr(pr_id) else {
            return;
        };
        if self.pr_forge(&pr).is_none() || pr.number <= 0 {
            return;
        }
        if pr.files.as_ref().is_some_and(|f| !f.is_empty()) {
            return;
        }
        if self
            .prs
            .diff_last
            .lock()
            .get(pr_id)
            .is_some_and(|t| t.elapsed() < PR_DIFF_MIN_INTERVAL)
        {
            return;
        }
        if !self.prs.diff_busy.lock().insert(pr_id.to_owned()) {
            return;
        }
        let e = self.arc();
        let id = pr_id.to_owned();
        tokio::spawn(async move {
            e.refresh_pr_diff(&id).await;
            e.prs.diff_last.lock().insert(id.clone(), Instant::now());
            e.prs.diff_busy.lock().remove(&id);
        });
    }

    /// Fills files from the forge's diff when a forge PR has none stored
    /// (imported PRs, no worktree yet).
    pub async fn refresh_pr_diff(&self, pr_id: &str) {
        let Some(pr) = self.store.pr(pr_id) else {
            return;
        };
        if pr.number <= 0 || pr.files.as_ref().is_some_and(|f| !f.is_empty()) {
            return;
        }
        let Some(forge) = self.pr_forge(&pr) else {
            return;
        };
        let repo = self.pr_repo_path(&pr);
        if repo.is_empty() {
            return;
        }
        let Ok(Ok(out)) =
            tokio::time::timeout(DIFF_TIMEOUT, forge.diff(Path::new(&repo), pr.number)).await
        else {
            return;
        };
        let files = recur_git::parse_unified_diff_plain(&out);
        if files.is_empty() {
            return;
        }
        let (add, del) = files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.diff.add, d + f.diff.del));
        self.store.update_pr(pr_id, |p| {
            p.file_count = files.len() as i64;
            p.files = Some(files);
            p.diff = DiffStat { add, del };
        });
        self.publish_pr_updated(pr_id);
    }

    /// Runs [`Engine::refresh_pr_conversation`] in the background at most
    /// once per [`PR_CONVERSATION_MIN_INTERVAL`] per PR (single-flight), and
    /// publishes `pr.updated` when it changed the row.
    pub fn schedule_refresh_pr_conversation(&self, pr_id: &str) {
        let Some(pr) = self.store.pr(pr_id) else {
            return;
        };
        if self.pr_forge(&pr).is_none() || pr.number <= 0 {
            return;
        }
        if self
            .prs
            .conv_last
            .lock()
            .get(pr_id)
            .is_some_and(|t| t.elapsed() < PR_CONVERSATION_MIN_INTERVAL)
        {
            return;
        }
        if !self.prs.conv_busy.lock().insert(pr_id.to_owned()) {
            return;
        }
        let e = self.arc();
        let id = pr_id.to_owned();
        tokio::spawn(async move {
            let changed = e.refresh_pr_conversation(&id).await;
            e.prs.conv_last.lock().insert(id.clone(), Instant::now());
            e.prs.conv_busy.lock().remove(&id);
            if changed {
                e.publish_pr_updated(&id);
            }
        });
    }

    /// Replaces a forge PR's thread and review state with the live
    /// conversation. Recur-only rows are kept and local holds win over a
    /// stale snapshot. Fetch errors leave the stored thread alone. Reports
    /// whether the stored PR changed.
    pub async fn refresh_pr_conversation(&self, pr_id: &str) -> bool {
        let Some(pr) = self.store.pr(pr_id) else {
            return false;
        };
        if pr.number <= 0 {
            return false;
        }
        let Some(forge) = self.pr_forge(&pr) else {
            return false;
        };
        let repo = self.pr_repo_path(&pr);
        if repo.is_empty() {
            return false;
        }
        let Ok(Ok(snap)) = tokio::time::timeout(
            SNAPSHOT_TIMEOUT,
            forge.snapshot(Path::new(&repo), pr.number),
        )
        .await
        else {
            return false;
        };
        let merged = forge.merge_comments(comments(&pr), &snap, chrono::Utc::now());
        let cs = self.apply_remembered_comment_reactions(
            pr_id,
            self.apply_remembered_thread_resolved(
                pr_id,
                self.drop_remembered_comments(pr_id, merged),
            ),
        );
        let stored_commits = pr.commits.clone().unwrap_or_default();
        let mut commits = enrich_commits(&stored_commits, &snap);
        if commits.is_empty() {
            commits = enrich_commits(&snap.commits, &snap);
        }
        apply_commit_avatars(&mut commits, &snap.author_avatars);
        let (state, group) = conversation_state(&pr, &snap);
        let (reasons, review_req, group) = conversation_attention(&pr, &snap, &state, &group);
        let avatar = |l: &str, p: &str| forge.avatar_url(l, p, 56);
        let people = merge_participants(
            &pr.participants,
            &snapshot_participants(&snap, &cs, &avatar),
        );
        let reviewers = snapshot_reviewers(&snap, &cs, &avatar);
        let title = if snap.title.is_empty() {
            pr.title.clone()
        } else {
            snap.title.clone()
        };
        let created_at = Some(normalize_time(&snap.created_at))
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| pr.created_at.clone());
        let updated_at = Some(normalize_time(&snap.updated_at))
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| pr.updated_at.clone());
        let url = snap.url.trim().to_owned();
        let author_login = snap.author_login.trim().to_owned();
        let viewer_login = snap.viewer_login.trim().to_owned();
        let head_sha = snap.head_sha.trim().to_owned();
        let unchanged = comments_equal(comments(&pr), &cs)
            && commits_equal(&stored_commits, &commits)
            && pr.state == state
            && pr.group == group
            && pr.title == title
            && (url.is_empty() || pr.url == url)
            && (head_sha.is_empty() || pr.head_sha == head_sha)
            && pr.author_login == author_login
            && pr.viewer_login == viewer_login
            && pr.viewer_can_write == snap.viewer_can_write
            && (snap.node_id.is_empty() || pr.node_id == snap.node_id)
            && pr.created_at == created_at
            && pr.updated_at == updated_at
            && pr.mergeable == snap.mergeable
            && pr.merge_state_status == snap.merge_state_status
            && pr.review_decision == snap.review_decision
            && pr.draft == snap.is_draft
            && pr.review_requested == review_req
            && pr.attention_reasons == reasons
            && pr.participants == people
            && pr.reviewers == reviewers
            && pr.allowed_merge_methods == snap.allowed_merge_methods
            && pr.merge_queue_enabled == snap.merge_queue_enabled
            && pr.merge_queued == snap.merge_queued;
        if unchanged {
            return false;
        }
        let became_merged = state == "Merged" && pr.state != "Merged";
        self.store.update_pr(pr_id, |p| {
            p.comments = Some(cs);
            p.commits = none_if_empty(commits);
            p.title = title;
            if !url.is_empty() {
                p.url = url;
            }
            if !author_login.is_empty() {
                p.author_login = author_login;
            }
            if !viewer_login.is_empty() {
                p.viewer_login = viewer_login;
            }
            if !head_sha.is_empty() {
                p.head_sha = head_sha;
            }
            p.viewer_can_write = snap.viewer_can_write;
            if !snap.node_id.is_empty() {
                p.node_id = snap.node_id.clone();
            }
            p.mergeable = snap.mergeable.clone();
            p.merge_state_status = snap.merge_state_status.clone();
            p.review_decision = snap.review_decision.clone();
            p.review_requested = review_req;
            p.attention_reasons = reasons;
            p.participants = people;
            p.reviewers = reviewers;
            p.draft = snap.is_draft;
            p.allowed_merge_methods = snap.allowed_merge_methods.clone();
            p.merge_queue_enabled = snap.merge_queue_enabled;
            p.merge_queued = snap.merge_queued;
            if !created_at.is_empty() {
                p.created_at = created_at;
            }
            if !updated_at.is_empty() {
                p.updated_at = updated_at.clone();
            }
            // Recompute from the live row: a merge can land while the
            // snapshot was in flight, and the state above would un-merge it.
            let (live_state, live_group) = conversation_state(p, &snap);
            let (_, _, live_group) = conversation_attention(p, &snap, &live_state, &live_group);
            p.state = live_state;
            p.group = live_group;
        });
        if became_merged {
            self.mark_merged(pr_id, &updated_at).await;
        }
        true
    }
}
