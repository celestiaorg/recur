//! The PR dashboard sweep (Go `github_prs.go` sync orchestration): open
//! and recently merged PRs of every git project, including ones Recur did
//! not open, applied onto stored rows; rows that fell off both lists are
//! confirmed with a state probe and settled. Single-flight with a TTL; the
//! running app re-sweeps every few minutes.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use recur_domain::{DiffStat, FILES_PROJECT_ID, Pr, Project, Workspace};
use recur_forge::model::{
    compact_ago, listed_attention_reasons, listed_behind, listed_merge_field,
    listed_review_requested, listed_state_group, merge_participants, normalize_time, parse_time,
    reconcile_listed_checks,
};
use recur_forge::{Forge, PrListRow, PrListState};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::bus::{EngineEvent, PrsSynced};
use crate::prs::merge::stamp_reconcile_updated_at;
use crate::prs::{GROUP_LOOP, GROUP_MERGED, PR_SYNC_PROJECT_CONCURRENCY, none_if_empty, normalize_pr_url, pr_settled};
use crate::workspace::create::normalize_pr_branch;

/// Bounds the forge reads of one sweep.
const SWEEP_TIMEOUT: Duration = Duration::from_secs(90);
/// Bounds the forge-base mapping's git reads.
const BASE_MAP_TIMEOUT: Duration = Duration::from_secs(5);

/// Clears the in-flight sweep and wakes waiters, even if the sweep is
/// cancelled or panics.
struct SweepDone<'a> {
    eng: &'a Engine,
    done: CancellationToken,
}

impl Drop for SweepDone<'_> {
    fn drop(&mut self) {
        let mut s = self.eng.prs.sync.lock();
        s.at = Some(std::time::Instant::now());
        s.wait = None;
        self.done.cancel();
    }
}

/// Stored forge PRs keyed by project + URL and project + number.
type StoredIndex = HashMap<String, Pr>;

fn stored_key(project_id: &str, kind: &str, v: &str) -> String {
    format!("{project_id}\x00{kind}\x00{v}")
}

fn lookup_indexed(idx: &StoredIndex, project_id: &str, row: &PrListRow) -> Option<Pr> {
    let u = normalize_pr_url(&row.url);
    if !u.is_empty()
        && let Some(p) = idx.get(&stored_key(project_id, "u", &u))
    {
        return Some(p.clone());
    }
    if row.number > 0 {
        return idx
            .get(&stored_key(project_id, "n", &row.number.to_string()))
            .cloned();
    }
    None
}

impl Engine {
    /// Imports open (and recently merged) forge PRs for every git project
    /// and updates Recur-opened ones in place. Callers within the TTL get
    /// the stored list; concurrent callers wait for the in-flight sweep.
    /// A missing forge tool or a per-repo list error skips that project.
    pub async fn sync_prs(&self) -> Vec<Pr> {
        enum Flight {
            Fresh,
            Wait(CancellationToken),
            Run(CancellationToken),
        }
        let min = self.prs.timing.lock().sync_min_interval;
        let flight = {
            let mut s = self.prs.sync.lock();
            if !min.is_zero() && s.at.is_some_and(|at| at.elapsed() < min) {
                Flight::Fresh
            } else if let Some(wait) = s.wait.clone() {
                Flight::Wait(wait)
            } else {
                let done = CancellationToken::new();
                s.wait = Some(done.clone());
                Flight::Run(done)
            }
        };
        let done = match flight {
            Flight::Fresh => return self.store.prs(),
            Flight::Wait(wait) => {
                wait.cancelled().await;
                return self.store.prs();
            }
            Flight::Run(done) => done,
        };
        let _done = SweepDone { eng: self, done };

        let deadline = Instant::now() + SWEEP_TIMEOUT;
        let now = Utc::now();
        let sem = Arc::new(tokio::sync::Semaphore::new(PR_SYNC_PROJECT_CONCURRENCY));
        let mut viewers: HashMap<String, String> = HashMap::new();
        let mut tasks = tokio::task::JoinSet::new();
        for proj in self.store.projects().unwrap_or_default() {
            if proj.id == FILES_PROJECT_ID || proj.repo_path.trim().is_empty() {
                continue;
            }
            if !recur_git::is_repo(&proj.repo_path).await {
                continue;
            }
            let Some(forge) = self.project_forge(&proj.repo_path).await else {
                continue;
            };
            if !forge.available().await {
                continue;
            }
            let viewer = match viewers.get(forge.id()) {
                Some(v) => v.clone(),
                None => {
                    let v = tokio::time::timeout_at(deadline, forge.login())
                        .await
                        .unwrap_or_default();
                    viewers.insert(forge.id().to_owned(), v.clone());
                    v
                }
            };
            let Ok(permit) = Arc::clone(&sem).acquire_owned().await else {
                break;
            };
            let e = self.arc();
            tasks.spawn(async move {
                e.sync_project_prs(forge.as_ref(), &proj, &viewer, now, deadline)
                    .await;
                drop(permit);
            });
        }
        while tasks.join_next().await.is_some() {}
        // Even without the forge tool, a workspace created (or renamed) onto
        // a PR head after the first import still attaches.
        self.link_unlinked_prs().await;
        self.store.prs()
    }

    async fn sync_project_prs(
        &self,
        forge: &dyn Forge,
        proj: &Project,
        viewer: &str,
        now: DateTime<Utc>,
        deadline: Instant,
    ) {
        let repo = Path::new(&proj.repo_path);
        let list = |state: PrListState| async move {
            tokio::time::timeout_at(deadline, forge.list_prs(repo, state, state.sweep_limit()))
                .await
                .unwrap_or_else(|_| {
                    Err(recur_forge::ForgeError::failed("context deadline exceeded"))
                })
        };
        let (open, merged) = tokio::join!(list(PrListState::Open), list(PrListState::Merged));
        let Ok(open_rows) = open else {
            return;
        };
        let idx = self.index_workspaces_by_pr_head(&proj.id, deadline).await;
        let stored = self.index_stored_prs(forge);
        let mut seen = HashSet::new();
        let merged_ok = merged.is_ok();
        let merged_rows = merged.unwrap_or_default();
        for row in open_rows.iter().chain(&merged_rows) {
            self.apply_listed_pr_indexed(forge, &proj.id, row, viewer, now, Some(&idx), Some(&stored))
                .await;
            if row.number > 0 {
                seen.insert(row.number);
            }
        }
        let skip = !merged_ok
            || open_rows.len() >= PrListState::Open.sweep_limit()
            || merged_rows.len() >= PrListState::Merged.sweep_limit();
        self.reconcile_missing_prs(forge, &proj.id, &proj.repo_path, &seen, skip, deadline)
            .await;
        self.publish_project_prs_synced(&proj.id);
    }

    /// Streams one project's rows to the dashboard before the whole sweep
    /// finishes.
    fn publish_project_prs_synced(&self, project_id: &str) {
        let rows: Vec<Pr> = self
            .store
            .prs()
            .into_iter()
            .filter(|p| self.pr_project_id(p) == project_id)
            .collect();
        if rows.is_empty() {
            return;
        }
        self.bus.publish(EngineEvent::PrsSynced(PrsSynced {
            project_id: project_id.to_owned(),
            prs: Some(rows),
        }));
    }

    /// Re-lists forge PRs every sweep interval until shutdown so a PR
    /// opened outside Recur appears (and links) without a click. Run once
    /// the bus is wired.
    pub async fn run_pr_sweep(&self) {
        let period = self.prs.timing.lock().sweep_interval;
        let mut t = tokio::time::interval_at(Instant::now() + period, period);
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            t.tick().await;
            if self.is_shutting_down() {
                return;
            }
            let mut available = false;
            for f in self.forges.all() {
                if f.available().await {
                    available = true;
                    break;
                }
            }
            if available {
                self.sweep_prs().await;
            }
        }
    }

    /// [`Engine::sync_prs`] plus `pr.updated` for every row it added or
    /// changed (no caller waits on the list). A change to `when` alone does
    /// not count: the relative age drifts every sweep.
    pub async fn sweep_prs(&self) {
        let before: HashMap<String, Pr> = self
            .store
            .prs()
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        for p in self.sync_prs().await {
            if let Some(mut old) = before.get(&p.id).cloned() {
                old.when = p.when.clone();
                if old == p {
                    continue;
                }
            }
            self.publish_pr_updated(&p.id);
        }
    }

    /// Whether `refresh_pr_behind` can count from a live worktree; the
    /// list's 0/1 merge state must not overwrite that.
    fn pr_has_local_behind(&self, workspace_id: &str) -> bool {
        let id = self.live_pr_workspace_id(workspace_id);
        !id.is_empty()
            && self
                .store
                .workspace(&id)
                .is_some_and(|w| !w.worktree_path.trim().is_empty())
    }

    /// A stored PR of this forge for the project, by URL or number.
    pub(crate) fn find_forge_pr(
        &self,
        forge: &dyn Forge,
        project_id: &str,
        number: i64,
        url: &str,
    ) -> Option<Pr> {
        let url = normalize_pr_url(url);
        let source = forge.pr_source();
        self.store.prs().into_iter().find(|p| {
            if p.source != source {
                return false;
            }
            if !url.is_empty() && normalize_pr_url(&p.url) == url && self.pr_project_id(p) == project_id {
                return true;
            }
            number > 0 && p.number == number && self.pr_project_id(p) == project_id
        })
    }

    fn index_stored_prs(&self, forge: &dyn Forge) -> StoredIndex {
        let source = forge.pr_source();
        let mut out = HashMap::new();
        for p in self.store.prs() {
            if p.source != source {
                continue;
            }
            let pid = self.pr_project_id(&p);
            if pid.is_empty() {
                continue;
            }
            let u = normalize_pr_url(&p.url);
            if !u.is_empty() {
                out.insert(stored_key(&pid, "u", &u), p.clone());
            }
            if p.number > 0 {
                out.insert(stored_key(&pid, "n", &p.number.to_string()), p);
            }
        }
        out
    }

    /// Applies one list row without the sweep's indexes.
    pub async fn apply_listed_pr(
        &self,
        forge: &dyn Forge,
        project_id: &str,
        row: &PrListRow,
        viewer: &str,
        now: DateTime<Utc>,
    ) -> (String, bool) {
        self.apply_listed_pr_indexed(forge, project_id, row, viewer, now, None, None)
            .await
    }

    /// Imports a list row as a new PR or updates the stored one; returns
    /// (id, created). A loop's own PR keeps its group until it settles.
    #[allow(clippy::too_many_arguments)]
    async fn apply_listed_pr_indexed(
        &self,
        forge: &dyn Forge,
        project_id: &str,
        row: &PrListRow,
        viewer: &str,
        now: DateTime<Utc>,
        idx: Option<&HashMap<String, Workspace>>,
        stored: Option<&StoredIndex>,
    ) -> (String, bool) {
        if row.number <= 0 || project_id.trim().is_empty() {
            return (String::new(), false);
        }
        let (state, group) = listed_state_group(row, viewer);
        let existing = stored
            .and_then(|s| lookup_indexed(s, project_id, row))
            .or_else(|| self.find_forge_pr(forge, project_id, row.number, &row.url));
        let mut ws_id = existing
            .as_ref()
            .map(|e| self.live_pr_workspace_id(&e.workspace_id))
            .unwrap_or_default();
        if ws_id.is_empty() && state != "Merged" && state != "Closed" {
            let head = normalize_pr_branch(&row.head_ref);
            let found = match idx {
                None => self.workspace_matching_pr_branch(project_id, &head).await,
                Some(idx) => idx.get(&head).cloned(),
            };
            if let Some(w) = found {
                ws_id = w.id;
            }
        }
        let when = compact_ago(parse_time(&row.updated_at), now);
        let author = row.author_login.trim().to_owned();
        let reasons = listed_attention_reasons(row, viewer);
        let review_req = listed_review_requested(row, viewer);
        let reviewers = row.reviewers.clone();
        let Some(existing) = existing else {
            let pr = Pr {
                id: forge.imported_pr_id(project_id, row.number),
                workspace_id: ws_id,
                project_id: project_id.to_owned(),
                group: group.into(),
                number: row.number,
                source: forge.pr_source(),
                url: row.url.clone(),
                draft: row.is_draft,
                branch: row.head_ref.clone(),
                base: row.base_ref.clone(),
                title: row.title.clone(),
                state: state.into(),
                diff: DiffStat {
                    add: row.additions,
                    del: row.deletions,
                },
                file_count: row.changed_files,
                checks: none_if_empty(row.checks.clone()),
                checks_ok: row.checks_ok,
                has_checks: row.has_checks,
                when: if when.is_empty() { "now".into() } else { when },
                created_at: normalize_time(&row.created_at),
                updated_at: normalize_time(&row.updated_at),
                author_login: author,
                viewer_login: viewer.to_owned(),
                review_decision: row.review_decision.clone(),
                review_requested: review_req,
                attention_reasons: reasons,
                participants: row.participants.clone(),
                reviewers,
                mergeable: listed_merge_field("", &row.mergeable),
                merge_state_status: listed_merge_field("", &row.merge_state_status),
                is_cross_repository: row.is_cross_repository,
                behind_base: listed_behind(row),
                ..Pr::default()
            };
            let id = pr.id.clone();
            return match self.store.add_pr(pr) {
                Ok(_) => (id, true),
                Err(_) => (String::new(), false),
            };
        };

        let people = merge_participants(&existing.participants, &row.participants);
        let keep_group = existing.group == GROUP_LOOP;
        let pick = |want: &str, have: &str| {
            if want.is_empty() { have.to_owned() } else { want.to_owned() }
        };
        let want_ws = pick(&ws_id, &existing.workspace_id);
        let want_title = pick(&row.title, &existing.title);
        let want_url = pick(&row.url, &existing.url);
        let want_branch = pick(&row.head_ref, &existing.branch);
        let want_base = pick(&row.base_ref, &existing.base);
        let want_files = if row.changed_files > 0 {
            row.changed_files
        } else {
            existing.file_count
        };
        let want_when = pick(&when, &existing.when);
        let want_created = pick(&normalize_time(&row.created_at), &existing.created_at);
        let want_updated = pick(&normalize_time(&row.updated_at), &existing.updated_at);
        let (mut want_state, mut want_group) = (existing.state.clone(), existing.group.clone());
        if !keep_group {
            (want_state, want_group) = (state.to_owned(), group.to_owned());
        } else if state == "Merged" || state == "Closed" {
            want_state = state.to_owned();
            want_group = GROUP_MERGED.to_owned();
        }
        let want_behind = if existing.updating_base.is_empty() && !self.pr_has_local_behind(&want_ws)
        {
            listed_behind(row)
        } else {
            existing.behind_base
        };
        let want_mergeable = listed_merge_field(&existing.mergeable, &row.mergeable);
        let want_merge_state = listed_merge_field(&existing.merge_state_status, &row.merge_state_status);
        let want_viewer = pick(viewer.trim(), &existing.viewer_login);
        let stored_checks = existing.checks.clone().unwrap_or_default();
        let (want_checks, refresh_checks) = if row.has_checks {
            let (c, r) = reconcile_listed_checks(&stored_checks, &row.checks);
            (Some(c), r)
        } else {
            (existing.checks.clone(), false)
        };
        let want_checks_slice = want_checks.clone().unwrap_or_default();
        let want_has_checks = row.has_checks || !want_checks_slice.is_empty();
        // The list has no merge-queue fields; the per-PR snapshot does.
        if existing.merge_queued || (existing.merge_queue_enabled && want_updated != existing.updated_at)
        {
            self.schedule_refresh_pr_conversation(&existing.id);
        }
        let unchanged = !refresh_checks
            && existing.project_id == project_id
            && existing.workspace_id == want_ws
            && existing.title == want_title
            && existing.url == want_url
            && existing.branch == want_branch
            && existing.base == want_base
            && existing.draft == row.is_draft
            && existing.author_login == author
            && existing.viewer_login == want_viewer
            && existing.review_decision == row.review_decision
            && existing.mergeable == want_mergeable
            && existing.merge_state_status == want_merge_state
            && existing.is_cross_repository == row.is_cross_repository
            && existing.diff.add == row.additions
            && existing.diff.del == row.deletions
            && existing.file_count == want_files
            && existing.checks_ok == row.checks_ok
            && existing.has_checks == want_has_checks
            && stored_checks == want_checks_slice
            && existing.behind_base == want_behind
            && existing.when == want_when
            && existing.created_at == want_created
            && existing.updated_at == want_updated
            && existing.state == want_state
            && existing.group == want_group
            && existing.review_requested == review_req
            && existing.attention_reasons == reasons
            && existing.participants == people
            && existing.reviewers == reviewers;
        if unchanged {
            return (existing.id, false);
        }
        self.store.update_pr(&existing.id, |p| {
            p.project_id = project_id.to_owned();
            p.workspace_id = want_ws;
            p.title = want_title;
            p.url = want_url;
            p.branch = want_branch;
            p.base = want_base;
            p.draft = row.is_draft;
            p.author_login = author;
            if !viewer.trim().is_empty() {
                p.viewer_login = viewer.to_owned();
            }
            p.review_decision = row.review_decision.clone();
            p.review_requested = review_req;
            p.attention_reasons = reasons;
            p.participants = people;
            p.reviewers = reviewers;
            p.mergeable = want_mergeable;
            p.merge_state_status = want_merge_state;
            p.is_cross_repository = row.is_cross_repository;
            p.diff = DiffStat {
                add: row.additions,
                del: row.deletions,
            };
            p.file_count = want_files;
            p.checks = want_checks;
            p.checks_ok = row.checks_ok;
            p.has_checks = want_has_checks;
            p.behind_base = want_behind;
            p.when = want_when;
            p.created_at = want_created;
            p.updated_at = want_updated;
            p.state = want_state;
            p.group = want_group;
        });
        if refresh_checks {
            self.refresh_pr_checks(&existing.id).await;
        }
        (existing.id, false)
    }

    /// Settles forge PRs missing from both lists. `skip` when either list
    /// was truncated or the merged list failed (missing cannot be told from
    /// unlisted). Unseen rows are confirmed with a state probe first; rows
    /// are never deleted.
    pub(crate) async fn reconcile_missing_prs(
        &self,
        forge: &dyn Forge,
        project_id: &str,
        repo_path: &str,
        seen: &HashSet<i64>,
        skip: bool,
        deadline: Instant,
    ) {
        if skip || project_id.is_empty() {
            return;
        }
        let source = forge.pr_source();
        for p in self.store.prs() {
            if p.source != source || self.pr_project_id(&p) != project_id {
                continue;
            }
            if p.number <= 0 || seen.contains(&p.number) || pr_settled(&p) {
                continue;
            }
            let probe = tokio::time::timeout_at(deadline, forge.pr_state(Path::new(repo_path), p.number))
                .await;
            let Ok(Ok(view)) = probe else {
                continue;
            };
            if view.state.is_empty() || view.state == "OPEN" {
                continue;
            }
            if view.state == "MERGED" && !self.live_pr_workspace_id(&p.workspace_id).is_empty() {
                self.mark_merged(&p.id, &view.updated_at).await;
                continue;
            }
            let state = if view.state == "MERGED" {
                "Merged"
            } else {
                "Closed"
            };
            self.store.update_pr(&p.id, |pr| {
                pr.state = state.into();
                pr.group = GROUP_MERGED.into();
                stamp_reconcile_updated_at(pr, &view.updated_at);
            });
            self.publish_pr_updated(&p.id);
            self.archive_pr_chat_hosts(&p.id, "").await;
        }
    }

    /// Maps a forge base name to the local compare ref: the project's
    /// remote even when the tracking ref is not fetched yet, else the
    /// default remote's ref when it exists, else the name as is.
    pub(crate) async fn local_base_from_forge(&self, project_id: &str, forge_base: &str) -> String {
        let base = forge_base.trim();
        let base = base.strip_prefix("refs/heads/").unwrap_or(base);
        if base.is_empty() {
            return String::new();
        }
        let Some(proj) = self
            .store
            .project(project_id)
            .filter(|p| !p.repo_path.trim().is_empty())
        else {
            return base.to_owned();
        };
        let mgr = recur_git::Manager::new(&proj.repo_path, &self.worktree_root);
        let project_base = proj.base_branch.trim().to_owned();
        let mapped = tokio::time::timeout(BASE_MAP_TIMEOUT, async {
            if let Some((r, short)) = mgr.parse_remote_tracking(&project_base).await {
                if short == base {
                    return project_base.clone();
                }
                // Pin to the project's remote: origin could hold a
                // same-named fork branch.
                return format!("{r}/{base}");
            }
            let remote = crate::prs::default_remote(&mgr).await;
            if !remote.is_empty() {
                let r = format!("{remote}/{base}");
                if mgr.commit_exists(&r).await {
                    return r;
                }
            }
            base.to_owned()
        })
        .await;
        mapped.unwrap_or_else(|_| base.to_owned())
    }
}
