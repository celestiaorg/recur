//! The in-memory working set and its write-through mutations.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{DateTime, TimeZone as _, Utc};
use parking_lot::{Mutex, RwLock};
use recur_domain::{
    Agent, AnalysisRun, AttentionCounts, Escalation, FINDING_ACTOR_CODER, FINDING_ACTOR_ENGINE,
    FINDING_ACTOR_HUMAN, FINDING_ACTOR_REVIEWER, FeedItem, Finding, FindingNote, FindingStatus,
    Group, IdType, InboxItem, Policy, Pr, PrSource, Project, ProjectGroup, Resource,
    SESSION_KIND_APP, Settings, StoredEvent, Workspace, effective_runs, new_unique_id, shortid,
    valid_finding_status,
};
use recur_project::projfile;
use serde::Deserialize;

use crate::db::{Db, ESCALATION_KEY_SEP, GithubBranchRule, github_branch_rule_key};
use crate::error::{Error, Result};
use crate::paths::{data_dir, db_path};

/// The persisted working set. Guarded by `Store::state`.
#[derive(Debug, Default)]
pub(crate) struct State {
    pub project_groups: Vec<ProjectGroup>,
    pub projects: Vec<Project>,
    pub workspaces: Vec<Workspace>,
    /// workspace id → agents.
    pub agents: BTreeMap<String, Vec<Agent>>,
    pub feed: BTreeMap<String, Vec<FeedItem>>,
    /// workspace id → findings.
    pub findings: BTreeMap<String, Vec<Finding>>,
    pub prs: Vec<Pr>,
    /// The last issued Local PR number; never decreases.
    pub pr_number_seq: i64,
    /// escalation key (workspace, or workspace + agent) → escalation.
    pub escalations: BTreeMap<String, Escalation>,
    pub inbox: Vec<InboxItem>,
    /// Observatory runs, newest first.
    pub analysis: Vec<AnalysisRun>,
    pub policy: Policy,
    pub settings: Settings,
    pub resource: Resource,
    /// `project\0repo\0branch` → cached rule.
    pub github_branch_rules: BTreeMap<String, GithubBranchRule>,
}

/// Recur's persisted state: an in-memory working set the engine and
/// services read, written through to SQLite so it survives a restart.
/// Sessions and schedules live only in SQLite and are queried on demand.
///
/// Locking (see the crate docs): `state` is the store-wide read/write lock
/// every getter takes; `feed_mu` orders feed writes so their SQLite writes
/// land in the same order as their in-memory edits without holding `state`
/// across the write (lock order `feed_mu` → `state`); `session_mu` serializes
/// session idx/seq allocation (lock order `session_mu` → `state`). The
/// connection itself is a mutex-guarded single connection.
pub struct Store {
    pub(crate) state: RwLock<State>,
    feed_mu: Mutex<()>,
    pub(crate) session_mu: Mutex<()>,
    /// `None` only for [`Store::new_in_memory`].
    pub(crate) db: Option<Db>,
}

pub(crate) fn now_rfc3339() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn nil_empty<T>(v: &mut Option<Vec<T>>) {
    if v.as_ref().is_some_and(Vec::is_empty) {
        *v = None;
    }
}

fn nil_if_empty<T>(v: Vec<T>) -> Option<Vec<T>> {
    (!v.is_empty()).then_some(v)
}

/// The getters hand out fully owned copies. Go's deep copy re-slices every
/// nested slice with `append([]T(nil), …)`, which turns an empty slice into
/// nil; these mirror that so JSON keeps Go's `null`.
pub(crate) fn clone_workspace(w: &Workspace) -> Workspace {
    let mut w = w.clone();
    nil_empty(&mut w.agent_codes);
    nil_empty(&mut w.convergence);
    w
}

fn clone_agent(a: &Agent) -> Agent {
    let mut a = a.clone();
    nil_empty(&mut a.steps);
    a
}

fn clone_files(files: &mut Option<Vec<recur_domain::FileDiff>>) {
    if let Some(files) = files {
        for f in files {
            nil_empty(&mut f.lines);
        }
    }
}

fn clone_pr(p: &Pr) -> Pr {
    let mut p = p.clone();
    if let Some(commits) = &mut p.commits {
        for c in commits {
            clone_files(&mut c.files);
        }
    }
    clone_files(&mut p.files);
    nil_empty(&mut p.comments);
    nil_empty(&mut p.verify);
    nil_empty(&mut p.checks);
    p
}

/// Keeps a pin only when that command still exists.
fn with_valid_pinned_run(mut p: Project) -> Project {
    let pin = p.pinned_run.trim().to_owned();
    p.pinned_run = if !pin.is_empty() && effective_runs(&p).iter().any(|r| r.name == pin) {
        pin
    } else {
        String::new()
    };
    p
}

/// Fills CreatedAt when missing so the Findings age column works for legacy
/// rows and ids stamped as `F-…-<unixNano>`.
pub(crate) fn ensure_finding_created_at(f: &mut Finding) {
    if !f.created_at.is_empty() {
        return;
    }
    if !f.updated_at.is_empty() {
        f.created_at = f.updated_at.clone();
        return;
    }
    if let Some(i) = f.id.rfind('-')
        && let Ok(n) = f.id[i + 1..].parse::<i64>()
        && n > 1_000_000_000_000_000
    {
        f.created_at = Utc
            .timestamp_nanos(n)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
    }
}

/// Fills ShortID when missing or already taken in list.
fn assign_finding_short_id(f: &mut Finding, list: &[Finding]) {
    let mut taken = |id: &str| list.iter().any(|e| e.short_id == id);
    if !f.short_id.is_empty() && !taken(&f.short_id) {
        return;
    }
    if let Ok(sid) = new_unique_id(&IdType::Finding, Some(&mut taken)) {
        f.short_id = sid;
    }
}

fn fill_pr_times(p: &mut Pr, existing_created: &str) {
    let now = now_rfc3339();
    if p.created_at.trim().is_empty() {
        p.created_at = existing_created.to_owned();
    }
    if p.created_at.trim().is_empty() {
        p.created_at = now.clone();
    }
    if p.updated_at.trim().is_empty() {
        p.updated_at = now;
    }
}

/// Copies a known sibling stamp when only one of CreatedAt / UpdatedAt is
/// set. Both-empty rows stay empty: the frontend treats missing stamps as
/// unknown, which is safer than claiming the upgrade instant.
pub(crate) fn fill_missing_pr_times(p: &mut Pr) -> bool {
    let c = p.created_at.trim().is_empty();
    let u = p.updated_at.trim().is_empty();
    match (c, u) {
        (true, false) => {
            p.created_at = p.updated_at.clone();
            true
        }
        (false, true) => {
            p.updated_at = p.created_at.clone();
            true
        }
        _ => false,
    }
}

/// A PR without its commits and files, copied field by field so the
/// (possibly huge) diffs are never cloned. The exhaustive pattern fails to
/// compile when Pr gains a field.
fn pr_list_row(p: &Pr) -> Pr {
    let Pr {
        id,
        workspace_id,
        project_id,
        group,
        number,
        source,
        url,
        head_sha,
        draft,
        branch,
        base,
        title,
        summary,
        focus,
        state,
        diff,
        file_count,
        checks_ok,
        has_checks,
        when,
        created_at,
        updated_at,
        comments,
        pending_comments,
        verify,
        checks,
        behind_base,
        updating_base,
        impact,
        author_login,
        participants,
        reviewers,
        viewer_login,
        viewer_can_write,
        node_id,
        mergeable,
        merge_state_status,
        review_decision,
        review_requested,
        attention_reasons,
        is_cross_repository,
        allowed_merge_methods,
        merge_queue_enabled,
        merge_queued,
        commits: _,
        files: _,
    } = p;
    Pr {
        id: id.clone(),
        workspace_id: workspace_id.clone(),
        project_id: project_id.clone(),
        group: group.clone(),
        number: *number,
        source: source.clone(),
        url: url.clone(),
        head_sha: head_sha.clone(),
        draft: *draft,
        branch: branch.clone(),
        base: base.clone(),
        title: title.clone(),
        summary: summary.clone(),
        focus: focus.clone(),
        state: state.clone(),
        diff: *diff,
        file_count: *file_count,
        checks_ok: *checks_ok,
        has_checks: *has_checks,
        when: when.clone(),
        created_at: created_at.clone(),
        updated_at: updated_at.clone(),
        comments: comments.clone(),
        pending_comments: pending_comments.clone(),
        verify: verify.clone(),
        checks: checks.clone(),
        behind_base: *behind_base,
        updating_base: updating_base.clone(),
        impact: impact.clone(),
        author_login: author_login.clone(),
        participants: participants.clone(),
        reviewers: reviewers.clone(),
        viewer_login: viewer_login.clone(),
        viewer_can_write: *viewer_can_write,
        node_id: node_id.clone(),
        mergeable: mergeable.clone(),
        merge_state_status: merge_state_status.clone(),
        review_decision: review_decision.clone(),
        review_requested: *review_requested,
        attention_reasons: attention_reasons.clone(),
        is_cross_repository: *is_cross_repository,
        allowed_merge_methods: allowed_merge_methods.clone(),
        merge_queue_enabled: *merge_queue_enabled,
        merge_queued: *merge_queued,
        commits: None,
        files: None,
    }
}

fn pr_is_unmerged(p: &Pr) -> bool {
    p.state != "Merged" && p.group != "merged" && p.state != "Closed"
}

pub(crate) fn escalation_key(workspace_id: &str, agent_code: &str) -> String {
    let agent_code = agent_code.trim();
    if agent_code.is_empty() {
        workspace_id.to_owned()
    } else {
        format!("{workspace_id}{ESCALATION_KEY_SEP}{agent_code}")
    }
}

/// Whether target deep-links into this workspace (`loop:` / `workspace:` /
/// `chat:ws:code`). PR targets are handled separately.
pub(crate) fn inbox_targets_workspace(target: &str, workspace_id: &str) -> bool {
    let (kind, rest) = target.split_once(':').unwrap_or((target, ""));
    if (kind == "loop" || kind == "workspace") && rest == workspace_id {
        return true;
    }
    if kind == "chat" {
        return rest
            .split_once(':')
            .is_some_and(|(ws, _)| ws == workspace_id);
    }
    false
}

/// The hard-delete cascade predicate: workspace deep-links plus `pr:` rows
/// for PRs deleted with the workspace.
fn inbox_targets_deleted_workspace(
    target: &str,
    workspace_id: &str,
    pr_ids: &BTreeSet<String>,
) -> bool {
    if inbox_targets_workspace(target, workspace_id) {
        return true;
    }
    let (kind, rest) = target.split_once(':').unwrap_or((target, ""));
    kind == "pr" && pr_ids.contains(rest)
}

fn mergeable_agent(dst: &Agent, src: &Agent) -> Agent {
    let mut out = clone_agent(dst);
    if !src.session_id.is_empty() {
        out.session_id = src.session_id.clone();
        if !src.model.is_empty() {
            out.model = src.model.clone();
        }
        if !src.status.is_empty() {
            out.status = src.status.clone();
        }
        if !src.name.is_empty() {
            out.name = src.name.clone();
        }
    }
    out.unseen = dst.unseen || src.unseen;
    out
}

impl Store {
    fn with_state(state: State, db: Option<Db>) -> Self {
        Self {
            state: RwLock::new(state),
            feed_mu: Mutex::new(()),
            session_mu: Mutex::new(()),
            db,
        }
    }

    /// Opens the SQLite-backed store under the managed root
    /// ([`crate::paths::data_dir`]).
    pub fn new() -> Result<Self> {
        let root = data_dir().map_err(|source| Error::Io {
            context: "resolve data dir".into(),
            source,
        })?;
        Self::open(&db_path(&root))
    }

    /// Opens (creating and migrating if needed) the database at path. A
    /// brand-new database gets the default policy, settings and a zeroed
    /// resource (never the demo data); an existing one is loaded.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Db::open(path)?;
        let fresh = db.needs_seed()?;
        let mut state = State::default();
        if fresh {
            crate::seed::seed_defaults(&mut state);
            db.seed_all(&state)
                .map_err(|e| Error::msg(format!("seed db: {e}")))?;
        } else {
            db.load_all(&mut state)
                .map_err(|e| Error::msg(format!("load db: {e}")))?;
        }
        Ok(Self::with_state(state, Some(db)))
    }

    /// A seeded store (defaults plus the demo dataset) with no persistence,
    /// for tests and the e2e fixture dump.
    pub fn new_in_memory() -> Self {
        let mut state = State::default();
        crate::seed::seed(&mut state);
        Self::with_state(state, None)
    }

    /// Releases the database (checkpointing the WAL first). Safe to call
    /// twice and on an in-memory store; later writes fail or are skipped as
    /// in Go after `Close`.
    pub fn close(&self) -> Result<()> {
        match &self.db {
            Some(db) => db.close(),
            None => Ok(()),
        }
    }

    /// The whole state replaced by the demo seed and written through, as the
    /// Go tests' persistDemoSeed does. Test and fixture use only.
    #[doc(hidden)]
    pub fn persist_demo_seed(&self) -> Result<()> {
        let mut s = self.state.write();
        crate::seed::seed_demo(&mut s);
        if let Some(db) = &self.db {
            db.seed_all(&s)?;
        }
        Ok(())
    }

    // ---- Project groups ----

    /// Go returns nil (JSON `null`) when there are none.
    pub fn project_groups(&self) -> Option<Vec<ProjectGroup>> {
        nil_if_empty(self.state.read().project_groups.clone())
    }

    pub fn project_group(&self, id: &str) -> Option<ProjectGroup> {
        self.state
            .read()
            .project_groups
            .iter()
            .find(|g| g.id == id)
            .cloned()
    }

    pub fn has_project_group(&self, id: &str) -> bool {
        self.state.read().project_groups.iter().any(|g| g.id == id)
    }

    /// Persists a new group, or updates an existing id in place.
    pub fn add_project_group(&self, mut g: ProjectGroup) -> Result<ProjectGroup> {
        let mut s = self.state.write();
        let idx = s.project_groups.iter().position(|x| x.id == g.id);
        let write_idx = idx.unwrap_or(s.project_groups.len());
        let now = now_rfc3339();
        if g.created_at.is_empty() {
            g.created_at = match idx {
                Some(i) => s.project_groups[i].created_at.clone(),
                None => now.clone(),
            };
        }
        g.updated_at = now;
        if let Some(db) = &self.db {
            db.upsert_project_group(&g, write_idx)?;
        }
        match idx {
            Some(i) => s.project_groups[i] = g.clone(),
            None => s.project_groups.push(g.clone()),
        }
        Ok(g)
    }

    /// Persists edits to an existing group.
    pub fn update_project_group(&self, mut g: ProjectGroup) -> Result<ProjectGroup> {
        let mut s = self.state.write();
        let Some(i) = s.project_groups.iter().position(|x| x.id == g.id) else {
            return Err(Error::msg(format!("project group {:?} not found", g.id)));
        };
        g.created_at = s.project_groups[i].created_at.clone();
        g.updated_at = now_rfc3339();
        if let Some(db) = &self.db {
            db.upsert_project_group(&g, i)?;
        }
        s.project_groups[i] = g.clone();
        Ok(g)
    }

    /// Removes a group and clears group_id on its member projects. A missing
    /// group is a no-op.
    pub fn delete_project_group(&self, id: &str) -> Result<()> {
        let mut s = self.state.write();
        let Some(idx) = s.project_groups.iter().position(|g| g.id == id) else {
            return Ok(());
        };
        if let Some(db) = &self.db {
            db.delete_project_group_and_ungroup(id)?;
        }
        for p in &mut s.projects {
            if p.group_id == id {
                p.group_id.clear();
            }
        }
        s.project_groups.remove(idx);
        Ok(())
    }

    /// Persists a new sidebar group order; ids must be a permutation of the
    /// current group ids.
    pub fn reorder_project_groups(&self, ids: &[String]) -> Result<()> {
        let mut s = self.state.write();
        if ids.len() != s.project_groups.len() {
            return Err(Error::msg(format!(
                "reorder groups: got {} ids, have {} groups",
                ids.len(),
                s.project_groups.len()
            )));
        }
        let mut next = Vec::with_capacity(ids.len());
        let mut seen = BTreeSet::new();
        for id in ids {
            let Some(g) = s.project_groups.iter().find(|g| &g.id == id) else {
                return Err(Error::msg(format!("reorder groups: unknown group {id:?}")));
            };
            if !seen.insert(id) {
                return Err(Error::msg(format!(
                    "reorder groups: duplicate group {id:?}"
                )));
            }
            next.push(g.clone());
        }
        if let Some(db) = &self.db {
            db.set_project_group_sorts(ids)?;
        }
        s.project_groups = next;
        Ok(())
    }

    // ---- Projects ----

    /// Every project with run-config overlaid from its project.toml (read
    /// outside the store lock). Go returns nil when there are none.
    pub fn projects(&self) -> Option<Vec<Project>> {
        let list = self.state.read().projects.clone();
        nil_if_empty(list.into_iter().map(projfile::overlay).collect())
    }

    pub fn project(&self, id: &str) -> Option<Project> {
        let p = self
            .state
            .read()
            .projects
            .iter()
            .find(|p| p.id == id)
            .cloned()?;
        Some(projfile::overlay(p))
    }

    /// Persists a new project (or updates an existing id in place) and
    /// returns the saved copy overlaid from project.toml. With
    /// `write_project_file` it first creates or replaces that file from the
    /// submitted run-config.
    pub fn add_project(&self, mut p: Project) -> Result<Project> {
        let mut s = self.state.write();
        let write_file = p.write_project_file;
        p.write_project_file = false;
        if write_file {
            projfile::write(&p)?;
        }
        let p = with_valid_pinned_run(p);
        let idx = s.projects.iter().position(|x| x.id == p.id);
        let write_idx = idx.unwrap_or(s.projects.len());
        if let Some(db) = &self.db {
            db.upsert_project(&p, write_idx)?;
        }
        match idx {
            Some(i) => s.projects[i] = p.clone(),
            None => s.projects.push(p.clone()),
        }
        // Overlay on the way out so has_project_file / runs match the
        // getters; the stored copy stays the machine-local record.
        Ok(projfile::overlay(p))
    }

    /// Persists machine-local edits to an existing project. With
    /// `write_project_file` it creates or replaces project.toml from the
    /// submitted run-config. Returns the copy overlaid from that file.
    pub fn update_project_run_config(&self, mut p: Project) -> Result<Project> {
        let mut s = self.state.write();
        let Some(i) = s.projects.iter().position(|x| x.id == p.id) else {
            return Err(Error::msg(format!("project {:?} not found", p.id)));
        };
        let write_file = p.write_project_file;
        p.write_project_file = false;
        let p = with_valid_pinned_run(p);
        if write_file {
            projfile::write(&p)?;
        }
        if let Some(db) = &self.db {
            db.upsert_project(&p, i)?;
        }
        s.projects[i] = p.clone();
        Ok(projfile::overlay(p))
    }

    pub fn has_project(&self, id: &str) -> bool {
        self.state.read().projects.iter().any(|p| p.id == id)
    }

    /// Persists a new sidebar order; ids must be a permutation of the
    /// project ids. The DB write happens first, so a failure leaves the
    /// order unchanged.
    pub fn reorder_projects(&self, ids: &[String]) -> Result<()> {
        let mut s = self.state.write();
        if ids.len() != s.projects.len() {
            return Err(Error::msg(format!(
                "reorder: got {} ids, have {} projects",
                ids.len(),
                s.projects.len()
            )));
        }
        let mut next = Vec::with_capacity(ids.len());
        let mut seen = BTreeSet::new();
        for id in ids {
            let Some(p) = s.projects.iter().find(|p| &p.id == id) else {
                return Err(Error::msg(format!("reorder: unknown project {id:?}")));
            };
            if !seen.insert(id) {
                return Err(Error::msg(format!("reorder: duplicate project {id:?}")));
            }
            next.push(p.clone());
        }
        if let Some(db) = &self.db {
            db.set_project_sorts(ids)?;
        }
        s.projects = next;
        Ok(())
    }

    /// Removes a project and cascades: every workspace in it and all their
    /// data (agents, feed, findings, events, escalations, sessions, PRs, inbox
    /// rows). Returns the deleted workspace ids (`None` when there were none,
    /// Go's nil). A missing project is a no-op.
    pub fn delete_project(&self, id: &str) -> Result<Option<Vec<String>>> {
        let mut s = self.state.write();
        let ws_ids: Vec<String> = s
            .workspaces
            .iter()
            .filter(|w| w.project_id == id)
            .map(|w| w.id.clone())
            .collect();
        for wid in &ws_ids {
            self.delete_workspace_locked(&mut s, wid)?;
        }
        if let Some(db) = &self.db {
            db.delete_doc("projects", id)?;
        }
        s.projects.retain(|p| p.id != id);
        let prefix = format!("{id}\x00");
        s.github_branch_rules.retain(|k, _| !k.starts_with(&prefix));
        Ok(nil_if_empty(ws_ids))
    }

    /// Removes one workspace and everything scoped to it from the DB first
    /// (on error memory is unchanged), then from memory.
    fn delete_workspace_locked(&self, s: &mut State, id: &str) -> Result<()> {
        let pr_ids: BTreeSet<String> = s
            .prs
            .iter()
            .filter(|p| p.workspace_id == id)
            .map(|p| p.id.clone())
            .collect();
        let deleted_inbox: Vec<String> = s
            .inbox
            .iter()
            .filter(|it| inbox_targets_deleted_workspace(&it.target, id, &pr_ids))
            .map(|it| it.id.clone())
            .collect();

        if let Some(db) = &self.db {
            db.delete_doc("workspaces", id)?;
            // "session" cascades message/message_content/session_context via
            // ON DELETE CASCADE (foreign_keys is ON on the one connection).
            for t in ["agents", "feed", "findings", "events", "session"] {
                db.delete_by_workspace(t, id)?;
            }
            db.delete_escalations(id)?;
            for pid in &pr_ids {
                db.delete_doc("prs", pid)?;
            }
            for iid in &deleted_inbox {
                db.delete_doc("inbox", iid)?;
            }
        }

        s.workspaces.retain(|w| w.id != id);
        s.prs.retain(|p| p.workspace_id != id);
        s.inbox.retain(|it| !deleted_inbox.contains(&it.id));
        s.agents.remove(id);
        s.feed.remove(id);
        s.findings.remove(id);
        s.escalations.remove(id);
        let prefix = format!("{id}{ESCALATION_KEY_SEP}");
        s.escalations.retain(|k, _| !k.starts_with(&prefix));
        Ok(())
    }

    /// Removes one workspace and its cascade (PRs, agents, feed, findings,
    /// events, escalations, sessions, inbox rows). Errors for an unknown id.
    pub fn delete_workspace(&self, id: &str) -> Result<()> {
        let mut s = self.state.write();
        if !s.workspaces.iter().any(|w| w.id == id) {
            return Err(Error::msg(format!("loop {id:?} not found")));
        }
        self.delete_workspace_locked(&mut s, id)
    }

    /// Wipes all data and zeroes the resource but keeps policy, settings and
    /// the PR-number high-water mark. The demo data never comes back.
    pub fn clear_all(&self) -> Result<()> {
        let mut s = self.state.write();
        if let Some(db) = &self.db {
            db.clear_data()?;
        }
        s.projects.clear();
        s.workspaces.clear();
        s.prs.clear();
        s.inbox.clear();
        s.agents.clear();
        s.feed.clear();
        s.findings.clear();
        s.escalations.clear();
        s.github_branch_rules.clear();
        s.resource = Resource::default();
        if let Some(db) = &self.db {
            let _ = db.upsert_singleton("resource", &s.resource);
        }
        Ok(())
    }

    // ---- Workspaces ----

    /// Workspaces in insertion order (oldest first).
    pub fn workspaces(&self) -> Vec<Workspace> {
        self.state
            .read()
            .workspaces
            .iter()
            .map(clone_workspace)
            .collect()
    }

    pub fn workspace(&self, id: &str) -> Option<Workspace> {
        self.state
            .read()
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .map(clone_workspace)
    }

    /// Applies f under the store lock and writes the result through
    /// (best-effort: a persistence failure keeps the in-memory update).
    /// Returns the updated workspace, or `None` when it is unknown. f must
    /// not call back into the store.
    pub fn update_workspace(&self, id: &str, f: impl FnOnce(&mut Workspace)) -> Option<Workspace> {
        let mut s = self.state.write();
        let i = s.workspaces.iter().position(|w| w.id == id)?;
        f(&mut s.workspaces[i]);
        if let Some(db) = &self.db {
            let _ = db.upsert_doc("workspaces", id, &s.workspaces[i], i);
        }
        Some(s.workspaces[i].clone())
    }

    /// Registers a new workspace (appended; the engine drains the oldest
    /// queued first). An existing id is updated in place. DB first.
    pub fn add_workspace(&self, w: Workspace) -> Result<Workspace> {
        let mut s = self.state.write();
        let idx = s.workspaces.iter().position(|x| x.id == w.id);
        let sort = idx.unwrap_or(s.workspaces.len());
        if let Some(db) = &self.db {
            db.upsert_doc("workspaces", &w.id, &w, sort)?;
        }
        let out = clone_workspace(&w);
        match idx {
            Some(i) => s.workspaces[i] = w,
            None => s.workspaces.push(w),
        }
        Ok(out)
    }

    /// How many workspaces are in a list bucket.
    pub fn count_group(&self, g: &Group) -> usize {
        self.state
            .read()
            .workspaces
            .iter()
            .filter(|w| w.group == *g)
            .count()
    }

    // ---- Agents ----

    pub fn agents(&self, workspace_id: &str) -> Vec<Agent> {
        self.state
            .read()
            .agents
            .get(workspace_id)
            .map(|l| l.iter().map(clone_agent).collect())
            .unwrap_or_default()
    }

    /// The agent matched by code or id.
    pub fn agent(&self, workspace_id: &str, code: &str) -> Option<Agent> {
        self.state
            .read()
            .agents
            .get(workspace_id)?
            .iter()
            .find(|a| a.code == code || a.id == code)
            .map(clone_agent)
    }

    fn persist_agents(&self, s: &State, workspace_id: &str) {
        if let Some(db) = &self.db {
            let list = s
                .agents
                .get(workspace_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let _ = db.replace_list("agents", workspace_id, list);
        }
    }

    /// Inserts or replaces (by code) an agent and persists the list.
    pub fn upsert_agent(&self, workspace_id: &str, a: Agent) {
        let mut s = self.state.write();
        let list = s.agents.entry(workspace_id.to_owned()).or_default();
        match list.iter().position(|x| x.code == a.code) {
            Some(i) => list[i] = a,
            None => list.push(a),
        }
        self.persist_agents(&s, workspace_id);
    }

    /// Applies f to the agent with code and persists. No-op when missing.
    pub fn update_agent(&self, workspace_id: &str, code: &str, f: impl FnOnce(&mut Agent)) {
        let mut s = self.state.write();
        let Some(a) = s
            .agents
            .get_mut(workspace_id)
            .and_then(|l| l.iter_mut().find(|a| a.code == code))
        else {
            return;
        };
        f(a);
        self.persist_agents(&s, workspace_id);
    }

    /// Removes an agent (by code). Does not touch its session transcript.
    pub fn delete_agent(&self, workspace_id: &str, code: &str) {
        let mut s = self.state.write();
        let Some(list) = s.agents.get_mut(workspace_id) else {
            return;
        };
        let Some(i) = list.iter().position(|a| a.code == code) else {
            return;
        };
        list.remove(i);
        self.persist_agents(&s, workspace_id);
    }

    /// Moves chat-tab agent rows from one workspace onto another. remap
    /// rewrites source codes that collided with a dest transcript. Dest
    /// placeholder tabs (same code, no provider session) take the source's
    /// resumable fields; moved agents get dest identity.
    pub fn move_workspace_agents(
        &self,
        from_id: &str,
        to_id: &str,
        remap: &BTreeMap<String, String>,
    ) {
        let from_id = from_id.trim();
        let to_id = to_id.trim();
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return;
        }
        let mut s = self.state.write();
        let from = s.agents.get(from_id).cloned().unwrap_or_default();
        if from.is_empty() {
            return;
        }
        let mut dest = s.agents.get(to_id).cloned().unwrap_or_default();
        let mut have: BTreeMap<String, usize> = dest
            .iter()
            .enumerate()
            .map(|(i, a)| (a.code.clone(), i))
            .collect();
        for a in &from {
            let mut a = clone_agent(a);
            if let Some(next) = remap.get(&a.code).filter(|n| !n.is_empty()) {
                a.code = next.clone();
            }
            a.workspace_id = to_id.to_owned();
            a.id = format!("{to_id}-{}", a.code);
            if let Some(&i) = have.get(&a.code) {
                dest[i] = mergeable_agent(&dest[i], &a);
                continue;
            }
            have.insert(a.code.clone(), dest.len());
            dest.push(a);
        }
        s.agents.insert(to_id.to_owned(), dest);
        s.agents.remove(from_id);
        self.persist_agents(&s, to_id);
        self.persist_agents(&s, from_id);
    }

    /// Moves reviewer findings from one workspace onto another. Returns how
    /// many moved.
    pub fn reassign_workspace_findings(&self, from_id: &str, to_id: &str) -> usize {
        let from_id = from_id.trim();
        let to_id = to_id.trim();
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return 0;
        }
        let mut s = self.state.write();
        let Some(src) = s.findings.remove(from_id).filter(|l| !l.is_empty()) else {
            return 0;
        };
        let n = src.len();
        let dest = s.findings.entry(to_id.to_owned()).or_default();
        for (sort, mut f) in (dest.len()..).zip(src) {
            f.workspace_id = to_id.to_owned();
            if let Some(db) = &self.db {
                let _ = db.upsert_finding(&f, sort);
            }
            dest.push(f);
        }
        n
    }

    // ---- Activity feed ----

    /// Newest first; `None` when empty (Go's nil).
    pub fn feed(&self, workspace_id: &str) -> Option<Vec<FeedItem>> {
        nil_if_empty(
            self.state
                .read()
                .feed
                .get(workspace_id)
                .cloned()
                .unwrap_or_default(),
        )
    }

    /// Adds an item to the front (newest first). The DB write is one INSERT
    /// outside the store-wide lock.
    pub fn prepend_feed(&self, workspace_id: &str, item: FeedItem) {
        let _feed = self.feed_mu.lock();
        self.state
            .write()
            .feed
            .entry(workspace_id.to_owned())
            .or_default()
            .insert(0, item.clone());
        if let Some(db) = &self.db {
            let _ = db.prepend_feed_item(workspace_id, &item);
        }
    }

    /// Replaces the item with item.id in place, or prepends it when absent.
    /// Typed cards re-emit under one id as they progress.
    pub fn upsert_feed(&self, workspace_id: &str, item: FeedItem) {
        let _feed = self.feed_mu.lock();
        let snapshot = {
            let mut s = self.state.write();
            let items = s.feed.entry(workspace_id.to_owned()).or_default();
            match items.iter().position(|i| i.id == item.id) {
                Some(i) => {
                    items[i] = item.clone();
                    Some(items.clone())
                }
                None => {
                    items.insert(0, item.clone());
                    None
                }
            }
        };
        let Some(db) = &self.db else {
            return;
        };
        match snapshot {
            Some(all) => {
                let _ = db.replace_list("feed", workspace_id, &all);
            }
            None => {
                let _ = db.prepend_feed_item(workspace_id, &item);
            }
        }
    }

    /// Updates the item that recorded tool_use_id in place and returns a
    /// copy, or `None` when none matched. The workspace's rows are rewritten
    /// (renumbered 0..N-1, consistent with prepends' negative sorts) off the
    /// store-wide lock.
    pub fn patch_feed_by_tool_use_id(
        &self,
        workspace_id: &str,
        tool_use_id: &str,
        f: impl FnOnce(&mut FeedItem),
    ) -> Option<FeedItem> {
        if tool_use_id.is_empty() {
            return None;
        }
        let _feed = self.feed_mu.lock();
        let (patched, snapshot) = {
            let mut s = self.state.write();
            let items = s.feed.get_mut(workspace_id)?;
            let it = items.iter_mut().find(|i| i.tool_use_id == tool_use_id)?;
            f(it);
            (it.clone(), items.clone())
        };
        if let Some(db) = &self.db {
            let _ = db.replace_list("feed", workspace_id, &snapshot);
        }
        Some(patched)
    }

    // ---- Durable events ----

    /// Persists one event to the append-only log and returns its row id (0
    /// on an in-memory store). Takes no store lock.
    pub fn append_event(&self, e: &StoredEvent) -> Result<i64> {
        match &self.db {
            Some(db) => db.append_event(e),
            None => Ok(0),
        }
    }

    /// Every durable event for a workspace, oldest first.
    pub fn events_for_loop(&self, workspace_id: &str) -> Result<Option<Vec<StoredEvent>>> {
        match &self.db {
            Some(db) => db.events_for_workspace(workspace_id),
            None => Ok(None),
        }
    }

    // ---- Findings ----

    /// A workspace's findings in insertion order.
    pub fn findings(&self, workspace_id: &str) -> Vec<Finding> {
        let mut out = self
            .state
            .read()
            .findings
            .get(workspace_id)
            .cloned()
            .unwrap_or_default();
        for f in &mut out {
            ensure_finding_created_at(f);
        }
        out
    }

    /// Records a new reviewer finding. The store assigns a UUIDv7 id and a
    /// public `find_…` short id unique per workspace (retried on a UNIQUE
    /// collision). A persist failure leaves it uncached. Returns the stored
    /// copy.
    pub fn add_finding(&self, mut f: Finding) -> Finding {
        let mut s = self.state.write();
        if f.created_at.is_empty() {
            f.created_at = now_rfc3339();
        }
        if f.id.is_empty() {
            f.id = uuid::Uuid::now_v7().to_string();
        }
        let list = s.findings.get(&f.workspace_id).cloned().unwrap_or_default();
        let ord = list.len();
        assign_finding_short_id(&mut f, &list);
        if let Some(db) = &self.db {
            let mut res = Ok(());
            for _ in 0..shortid::DEFAULT_ATTEMPTS {
                res = db.upsert_finding(&f, ord);
                match &res {
                    Err(e) if e.is_unique_constraint() => {
                        f.short_id.clear();
                        assign_finding_short_id(&mut f, &list);
                    }
                    _ => break,
                }
            }
            if res.is_err() {
                return f;
            }
        }
        s.findings
            .entry(f.workspace_id.clone())
            .or_default()
            .push(f.clone());
        f
    }

    /// Marks every open finding resolved (engine bulk clear); ignored and
    /// invalid findings are left alone.
    pub fn resolve_findings(&self, workspace_id: &str) {
        let mut s = self.state.write();
        let now = now_rfc3339();
        let Some(list) = s.findings.get_mut(workspace_id) else {
            return;
        };
        for (i, f) in list.iter_mut().enumerate() {
            if f.status == FindingStatus::Open.as_str() {
                f.status = FindingStatus::Resolved.as_str().to_owned();
                f.updated_by = FINDING_ACTOR_ENGINE.to_owned();
                f.updated_at = now.clone();
                f.status_reason.clear();
                if let Some(db) = &self.db {
                    let _ = db.upsert_finding(f, i);
                }
            }
        }
    }

    /// Human Ignore / Mark invalid / Reopen.
    pub fn set_finding_status(
        &self,
        workspace_id: &str,
        finding_id: &str,
        status: &str,
    ) -> Result<Finding> {
        self.update_finding_status(workspace_id, finding_id, status, "", FINDING_ACTOR_HUMAN)
    }

    /// Appends an operator comment (non-empty after trim). author defaults
    /// to human.
    pub fn add_finding_note(
        &self,
        workspace_id: &str,
        finding_id: &str,
        body: &str,
        author: &str,
    ) -> Result<Finding> {
        let body = body.trim();
        if body.is_empty() {
            return Err(Error::msg("finding note body is required"));
        }
        let author = match author {
            FINDING_ACTOR_HUMAN | FINDING_ACTOR_CODER | FINDING_ACTOR_ENGINE => author,
            "" => FINDING_ACTOR_HUMAN,
            _ => {
                return Err(Error::msg(format!(
                    "invalid finding note author {author:?}"
                )));
            }
        };
        let mut s = self.state.write();
        let Some((i, f)) = s.findings.get_mut(workspace_id).and_then(|l| {
            l.iter_mut()
                .enumerate()
                .find(|(_, f)| f.matches_ref(finding_id))
        }) else {
            return Err(Error::msg(format!("finding {finding_id:?} not found")));
        };
        f.notes.push(FindingNote {
            id: uuid::Uuid::now_v7().to_string(),
            author: author.to_owned(),
            body: body.to_owned(),
            created_at: now_rfc3339(),
        });
        if let Some(db) = &self.db {
            let _ = db.upsert_finding(f, i);
        }
        Ok(f.clone())
    }

    /// Removes one operator note by id.
    pub fn delete_finding_note(
        &self,
        workspace_id: &str,
        finding_id: &str,
        note_id: &str,
    ) -> Result<Finding> {
        let note_id = note_id.trim();
        if note_id.is_empty() {
            return Err(Error::msg("finding note id is required"));
        }
        let mut s = self.state.write();
        let Some((i, f)) = s.findings.get_mut(workspace_id).and_then(|l| {
            l.iter_mut()
                .enumerate()
                .find(|(_, f)| f.matches_ref(finding_id))
        }) else {
            return Err(Error::msg(format!("finding {finding_id:?} not found")));
        };
        let before = f.notes.len();
        f.notes.retain(|n| n.id != note_id);
        if f.notes.len() == before {
            return Err(Error::msg(format!("finding note {note_id:?} not found")));
        }
        if let Some(db) = &self.db {
            let _ = db.upsert_finding(f, i);
        }
        Ok(f.clone())
    }

    /// Flips one finding's status with attribution. actor is
    /// human|coder|reviewer|engine. Coder and reviewer may only set
    /// resolved|invalid (invalid requires a reason) and cannot touch
    /// human-ignored findings. Engine may only resolve still-open findings.
    /// Same status (+ same/empty reason) returns the row unchanged.
    pub fn update_finding_status(
        &self,
        workspace_id: &str,
        finding_id: &str,
        status: &str,
        reason: &str,
        actor: &str,
    ) -> Result<Finding> {
        if !valid_finding_status(status) {
            return Err(Error::msg(format!("invalid finding status {status:?}")));
        }
        let reason = reason.trim();
        let actor = match actor {
            FINDING_ACTOR_HUMAN
            | FINDING_ACTOR_CODER
            | FINDING_ACTOR_REVIEWER
            | FINDING_ACTOR_ENGINE => actor,
            "" => FINDING_ACTOR_HUMAN,
            _ => return Err(Error::msg(format!("invalid finding actor {actor:?}"))),
        };
        let agent_actor = actor == FINDING_ACTOR_CODER || actor == FINDING_ACTOR_REVIEWER;
        if agent_actor {
            if status != FindingStatus::Resolved.as_str()
                && status != FindingStatus::Invalid.as_str()
            {
                return Err(Error::msg(format!(
                    "{actor} may only set status resolved or invalid, got {status:?}"
                )));
            }
            if status == FindingStatus::Invalid.as_str() && reason.is_empty() {
                return Err(Error::msg(
                    "reason is required when marking a finding invalid",
                ));
            }
        }
        if actor == FINDING_ACTOR_ENGINE && status != FindingStatus::Resolved.as_str() {
            return Err(Error::msg(format!(
                "engine may only set status resolved, got {status:?}"
            )));
        }

        let mut s = self.state.write();
        let Some((i, f)) = s.findings.get_mut(workspace_id).and_then(|l| {
            l.iter_mut()
                .enumerate()
                .find(|(_, f)| f.matches_ref(finding_id))
        }) else {
            return Err(Error::msg(format!("finding {finding_id:?} not found")));
        };
        if agent_actor && f.status == FindingStatus::Ignored.as_str() {
            return Err(Error::msg(format!(
                "finding {finding_id:?} is ignored by the human; {actor} cannot change it"
            )));
        }
        if actor == FINDING_ACTOR_ENGINE && f.status != FindingStatus::Open.as_str() {
            return Ok(f.clone());
        }
        if f.status == status {
            if !reason.is_empty() && reason != f.status_reason {
                f.status_reason = reason.to_owned();
                f.updated_by = actor.to_owned();
                f.updated_at = now_rfc3339();
                if let Some(db) = &self.db {
                    let _ = db.upsert_finding(f, i);
                }
            }
            return Ok(f.clone());
        }
        f.status = status.to_owned();
        f.updated_by = actor.to_owned();
        f.updated_at = now_rfc3339();
        if status == FindingStatus::Open.as_str() {
            f.status_reason.clear();
        } else if !reason.is_empty() {
            f.status_reason = reason.to_owned();
        } else if status != FindingStatus::Invalid.as_str() {
            f.status_reason.clear();
        }
        if let Some(db) = &self.db {
            let _ = db.upsert_finding(f, i);
        }
        Ok(f.clone())
    }

    /// Still-open findings (insertion order); `None` when none (Go's nil).
    pub fn open_findings(&self, workspace_id: &str) -> Option<Vec<Finding>> {
        self.findings_by_status(workspace_id, FindingStatus::Open.as_str())
    }

    /// Findings matching status, or all for "" / "all"; `None` when none.
    pub fn findings_by_status(&self, workspace_id: &str, status: &str) -> Option<Vec<Finding>> {
        let s = self.state.read();
        let all = s
            .findings
            .get(workspace_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let out: Vec<Finding> = if status.is_empty() || status == "all" {
            all.to_vec()
        } else {
            all.iter().filter(|f| f.status == status).cloned().collect()
        };
        nil_if_empty(out)
    }

    pub fn open_findings_count(&self, workspace_id: &str) -> usize {
        self.state.read().findings.get(workspace_id).map_or(0, |l| {
            l.iter()
                .filter(|f| f.status == FindingStatus::Open.as_str())
                .count()
        })
    }

    // ---- Analysis (observatory) ----

    /// Persists an observatory run; memory keeps the newest 50.
    pub fn add_analysis(&self, a: AnalysisRun) -> AnalysisRun {
        let mut s = self.state.write();
        s.analysis.insert(0, a.clone());
        s.analysis.truncate(50);
        if let Some(db) = &self.db {
            let _ = db.upsert_analysis(&a);
        }
        a
    }

    /// The newest run for a project (scoped to a workspace when given),
    /// falling back to a project-level run when no workspace run exists.
    pub fn latest_analysis(&self, project_id: &str, workspace_id: &str) -> Option<AnalysisRun> {
        let s = self.state.read();
        let hit = s.analysis.iter().find(|a| {
            a.project_id == project_id
                && (workspace_id.is_empty() || a.workspace_id == workspace_id)
        });
        if let Some(a) = hit {
            return Some(a.clone());
        }
        if !workspace_id.is_empty() {
            return s
                .analysis
                .iter()
                .find(|a| a.project_id == project_id && a.workspace_id.is_empty())
                .cloned();
        }
        None
    }

    // ---- PRs ----

    /// List rows: every field except commits and files, which can be tens
    /// of megabytes per PR. [`Store::pr`] returns the full record.
    pub fn prs(&self) -> Vec<Pr> {
        self.state
            .read()
            .prs
            .iter()
            .map(|p| clone_pr(&pr_list_row(p)))
            .collect()
    }

    pub fn pr(&self, id: &str) -> Option<Pr> {
        self.state
            .read()
            .prs
            .iter()
            .find(|p| p.id == id)
            .map(clone_pr)
    }

    /// The PR linked to a workspace. A Local row older than a GitHub row is
    /// a leftover fallback and never wins; otherwise the newest open match
    /// wins, else the newest match.
    pub fn pr_by_workspace(&self, workspace_id: &str) -> Option<Pr> {
        let s = self.state.read();
        let mut last = None;
        let mut seen_github = false;
        for (i, p) in s.prs.iter().enumerate().rev() {
            if p.workspace_id != workspace_id {
                continue;
            }
            if p.source == PrSource::GitHub {
                seen_github = true;
            } else if seen_github {
                continue;
            }
            if pr_is_unmerged(p) {
                return Some(clone_pr(p));
            }
            last.get_or_insert(i);
        }
        last.map(|i| clone_pr(&s.prs[i]))
    }

    /// An unmerged PR for the workspace, preferring GitHub over a leftover
    /// Local fallback.
    pub fn open_pr_by_workspace(&self, workspace_id: &str) -> Option<Pr> {
        let s = self.state.read();
        let mut local = None;
        for p in s.prs.iter().rev() {
            if p.workspace_id != workspace_id || !pr_is_unmerged(p) {
                continue;
            }
            if p.source == PrSource::GitHub {
                return Some(clone_pr(p));
            }
            local.get_or_insert(p);
        }
        local.map(clone_pr)
    }

    /// Removes a PR by id; false when missing or the delete failed.
    pub fn delete_pr(&self, id: &str) -> bool {
        let mut s = self.state.write();
        let Some(i) = s.prs.iter().position(|p| p.id == id) else {
            return false;
        };
        if let Some(db) = &self.db
            && db.delete_doc("prs", id).is_err()
        {
            return false;
        }
        s.prs.remove(i);
        true
    }

    /// Persists a PR (DB first); an existing id is updated in place keeping
    /// its CreatedAt. Returns the stored copy.
    pub fn add_pr(&self, mut p: Pr) -> Result<Pr> {
        let mut s = self.state.write();
        let idx = s.prs.iter().position(|x| x.id == p.id);
        match idx {
            Some(i) => {
                let existing = s.prs[i].created_at.clone();
                fill_pr_times(&mut p, &existing);
            }
            None => fill_pr_times(&mut p, ""),
        }
        let sort = idx.unwrap_or(s.prs.len());
        if let Some(db) = &self.db {
            db.upsert_doc("prs", &p.id, &p, sort)?;
        }
        let out = clone_pr(&p);
        let (source, number) = (p.source.clone(), p.number);
        match idx {
            Some(i) => s.prs[i] = p,
            None => s.prs.push(p),
        }
        self.note_local_pr_number_locked(&mut s, &source, number);
        Ok(out)
    }

    /// The next sequential Local PR number from the persisted high-water
    /// mark (1 when none was issued). Deletes never reclaim numbers.
    pub fn next_pr_number(&self) -> i64 {
        let mut s = self.state.write();
        let n = s.pr_number_seq + 1;
        self.note_pr_number_locked(&mut s, n);
        n
    }

    fn note_pr_number_locked(&self, s: &mut State, n: i64) {
        if n <= s.pr_number_seq {
            return;
        }
        s.pr_number_seq = n;
        if let Some(db) = &self.db {
            let _ = db.set_pr_number_seq(n);
        }
    }

    /// GitHub-assigned numbers live in their own namespace and never move
    /// the Local sequence.
    fn note_local_pr_number_locked(&self, s: &mut State, source: &PrSource, number: i64) {
        if *source != PrSource::GitHub {
            self.note_pr_number_locked(s, number);
        }
    }

    /// Applies f under the lock, writes through, and returns a deep copy.
    pub fn update_pr(&self, id: &str, f: impl FnOnce(&mut Pr)) -> Option<Pr> {
        let mut s = self.state.write();
        let i = s.prs.iter().position(|p| p.id == id)?;
        f(&mut s.prs[i]);
        if let Some(db) = &self.db {
            let _ = db.upsert_doc("prs", id, &s.prs[i], i);
        }
        let out = clone_pr(&s.prs[i]);
        self.note_local_pr_number_locked(&mut s, &out.source, out.number);
        Some(out)
    }

    // ---- Escalations ----

    pub fn escalation(&self, workspace_id: &str) -> Option<Escalation> {
        lookup_escalation(&self.state.read(), workspace_id, "").map(|(_, e)| e)
    }

    /// Whether any walk on this workspace still has an unanswered question.
    pub fn has_open_escalation(&self, workspace_id: &str) -> bool {
        let s = self.state.read();
        let prefix = format!("{workspace_id}{ESCALATION_KEY_SEP}");
        s.escalations.iter().any(|(k, e)| {
            e.status == "open"
                && (e.workspace_id == workspace_id || k == workspace_id || k.starts_with(&prefix))
        })
    }

    /// Records (or replaces) an escalation. Chat-owned walks key by
    /// workspace + agent so sibling walks each keep an open question.
    pub fn add_escalation(&self, e: Escalation) -> Escalation {
        let mut s = self.state.write();
        let key = escalation_key(&e.workspace_id, &e.agent_code);
        s.escalations.insert(key.clone(), e.clone());
        if let Some(db) = &self.db {
            let _ = db.upsert_escalation(&key, &e);
        }
        e
    }

    /// Resolves the workspace's open escalation. Idempotent: an already
    /// answered (or missing) escalation returns `(record, false)` so a
    /// settled loop is never re-dispatched.
    pub fn answer_escalation(
        &self,
        workspace_id: &str,
        choice: &str,
        free_text: &str,
    ) -> (Option<Escalation>, bool) {
        self.answer_escalation_for(workspace_id, "", choice, free_text)
    }

    /// Resolves the escalation owned by agent_code ("" is the
    /// workspace-scoped row). Returns the record (`None` when missing) and
    /// whether this call answered it.
    pub fn answer_escalation_for(
        &self,
        workspace_id: &str,
        agent_code: &str,
        choice: &str,
        free_text: &str,
    ) -> (Option<Escalation>, bool) {
        let mut s = self.state.write();
        let Some((key, mut e)) = lookup_escalation(&s, workspace_id, agent_code) else {
            return (None, false);
        };
        if e.status == "answered" {
            return (Some(e), false);
        }
        let choice = choice.trim();
        let free_text = free_text.trim();
        e.answer = if free_text.is_empty() {
            choice
        } else {
            free_text
        }
        .to_owned();
        e.choice = choice.to_owned();
        e.free_text = free_text.to_owned();
        e.status = "answered".to_owned();
        e.applied = false;
        s.escalations.insert(key.clone(), e.clone());
        if let Some(db) = &self.db {
            let _ = db.upsert_escalation(&key, &e);
        }
        (Some(e), true)
    }

    /// Records that a loop consumed the persisted answer, so a re-dispatch
    /// does not seed it again.
    pub fn mark_escalation_applied(&self, workspace_id: &str) {
        let mut s = self.state.write();
        let Some((key, mut e)) = lookup_escalation(&s, workspace_id, "") else {
            return;
        };
        if e.status != "answered" || e.applied {
            return;
        }
        e.applied = true;
        s.escalations.insert(key.clone(), e.clone());
        if let Some(db) = &self.db {
            let _ = db.upsert_escalation(&key, &e);
        }
    }

    // ---- Inbox ----

    /// Newest first; `None` when empty (Go's nil).
    pub fn inbox(&self) -> Option<Vec<InboxItem>> {
        nil_if_empty(self.state.read().inbox.clone())
    }

    pub fn unread_count(&self) -> usize {
        self.state.read().inbox.iter().filter(|i| i.unread).count()
    }

    /// How many chat-facing agents have Unseen set.
    pub fn unseen_chat_count(&self) -> usize {
        self.state
            .read()
            .agents
            .values()
            .flatten()
            .filter(|a| a.unseen)
            .count()
    }

    /// Unseen chat tabs per workspace / project plus inbox unread, for
    /// sidebar badges and the dock icon. Dock is unseen + non-chat inbox
    /// unread + Ask Recur unseen. Archived workspaces are skipped; hidden PR
    /// chat hosts count toward totals but not their project.
    pub fn attention_counts(&self) -> AttentionCounts {
        let s = self.state.read();
        let mut archived = BTreeSet::new();
        let mut host = BTreeSet::new();
        let mut proj_of = BTreeMap::new();
        for w in &s.workspaces {
            if w.archived {
                archived.insert(w.id.as_str());
            }
            if w.is_pr_chat_host() {
                host.insert(w.id.as_str());
            }
            proj_of.insert(w.id.as_str(), w.project_id.as_str());
        }
        let mut by_ws = BTreeMap::new();
        let mut by_proj: BTreeMap<String, i64> = BTreeMap::new();
        let mut unseen = 0;
        for (ws, list) in &s.agents {
            if archived.contains(ws.as_str()) {
                continue;
            }
            let n = list.iter().filter(|a| a.unseen).count() as i64;
            if n == 0 {
                continue;
            }
            by_ws.insert(ws.clone(), n);
            unseen += n;
            if host.contains(ws.as_str()) {
                continue;
            }
            if let Some(p) = proj_of.get(ws.as_str()).filter(|p| !p.is_empty()) {
                *by_proj.entry((*p).to_owned()).or_default() += n;
            }
        }
        let inbox_unread = s.inbox.iter().filter(|i| i.unread).count() as i64;
        let other_unread = s
            .inbox
            .iter()
            .filter(|i| i.unread && i.kind != "chat")
            .count() as i64;
        let mut app_unseen = 0;
        if let Some(db) = &self.db
            && let Ok(list) = crate::sessions::sessions_for_app_kind(db, SESSION_KIND_APP)
        {
            #[derive(Deserialize, Default)]
            #[serde(default)]
            struct Cfg {
                unseen: bool,
                closed: bool,
            }
            for sess in list {
                let cfg: Cfg = serde_json::from_str(&sess.config_json).unwrap_or_default();
                if cfg.unseen && !cfg.closed {
                    app_unseen += 1;
                }
            }
        }
        AttentionCounts {
            unseen,
            inbox_unread,
            dock: unseen + other_unread + app_unseen,
            by_workspace: Some(by_ws),
            by_project: Some(by_proj),
            app_unseen,
        }
    }

    pub fn mark_inbox_read(&self, id: &str) {
        let mut s = self.state.write();
        for (i, it) in s.inbox.iter_mut().enumerate() {
            if it.id == id {
                it.unread = false;
                if let Some(db) = &self.db {
                    let _ = db.upsert_doc("inbox", &it.id, it, i);
                }
            }
        }
    }

    pub fn mark_all_inbox_read(&self) {
        let mut s = self.state.write();
        for (i, it) in s.inbox.iter_mut().enumerate() {
            it.unread = false;
            if let Some(db) = &self.db {
                let _ = db.upsert_doc("inbox", &it.id, it, i);
            }
        }
    }

    /// Prepends an actionable row (newest first) and persists it; an
    /// existing id is replaced in place.
    pub fn add_inbox_item(&self, it: InboxItem) {
        let mut s = self.state.write();
        if let Some(i) = s.inbox.iter().position(|x| x.id == it.id) {
            if let Some(db) = &self.db {
                let _ = db.upsert_doc("inbox", &it.id, &it, i);
            }
            s.inbox[i] = it;
            return;
        }
        s.inbox.insert(0, it);
        if let Some(db) = &self.db {
            // Re-persist with refreshed sort indexes so the new head sorts first.
            for (i, it) in s.inbox.iter().enumerate() {
                let _ = db.upsert_doc("inbox", &it.id, it, i);
            }
        }
    }

    fn remove_inbox_where(&self, pred: impl Fn(&InboxItem) -> bool) -> usize {
        let mut s = self.state.write();
        let mut removed = 0;
        let db = self.db.as_ref();
        s.inbox.retain(|it| {
            if !pred(it) {
                return true;
            }
            removed += 1;
            if let Some(db) = db {
                let _ = db.delete_doc("inbox", &it.id);
            }
            false
        });
        removed
    }

    /// Drops every row whose target matches and persists the removal.
    pub fn remove_inbox_by_target(&self, target: &str) -> usize {
        self.remove_inbox_where(|it| it.target == target)
    }

    /// Drops rows that deep-link into this workspace so Remove does not
    /// leave badges for a hidden archived workspace. PR rows are kept.
    pub fn clear_inbox_for_workspace(&self, workspace_id: &str) -> usize {
        self.remove_inbox_where(|it| inbox_targets_workspace(&it.target, workspace_id))
    }

    /// Drops one row by id, reporting whether it was there.
    pub fn remove_inbox_item(&self, id: &str) -> bool {
        let mut s = self.state.write();
        let Some(i) = s.inbox.iter().position(|it| it.id == id) else {
            return false;
        };
        s.inbox.remove(i);
        if let Some(db) = &self.db {
            let _ = db.delete_doc("inbox", id);
        }
        true
    }

    /// Drops every chat-kind (or `ib-chat-*`) row.
    pub fn remove_chat_inbox_items(&self) -> usize {
        self.remove_inbox_where(|it| it.kind == "chat" || it.id.starts_with("ib-chat-"))
    }

    // ---- Policy / Settings / Resource ----

    pub fn policy(&self) -> Policy {
        self.state.read().policy.clone()
    }

    pub fn set_policy(&self, p: Policy) {
        let mut s = self.state.write();
        if let Some(db) = &self.db {
            let _ = db.upsert_singleton("policy", &p);
        }
        s.policy = p;
    }

    pub fn settings(&self) -> Settings {
        self.state.read().settings.clone()
    }

    pub fn set_settings(&self, set: Settings) {
        let mut s = self.state.write();
        if let Some(db) = &self.db {
            let _ = db.upsert_singleton("settings", &set);
        }
        s.settings = set;
    }

    pub fn resource(&self) -> Resource {
        self.state.read().resource.clone()
    }

    pub fn set_resource(&self, r: Resource) {
        let mut s = self.state.write();
        if let Some(db) = &self.db {
            let _ = db.upsert_singleton("resource", &r);
        }
        s.resource = r;
    }

    // ---- GitHub branch rules ----

    /// The cached "pull request required" rule and when it was checked.
    pub fn github_branch_require_pr(
        &self,
        project_id: &str,
        repo: &str,
        branch: &str,
    ) -> Option<(bool, DateTime<Utc>)> {
        self.state
            .read()
            .github_branch_rules
            .get(&github_branch_rule_key(project_id, repo, branch))
            .map(|e| (e.require, e.at))
    }

    /// Caches the rule (blank ids are ignored). The DB write happens after
    /// the store lock is released.
    pub fn set_github_branch_require_pr(
        &self,
        project_id: &str,
        repo: &str,
        branch: &str,
        require: bool,
    ) {
        let (project_id, repo, branch) = (project_id.trim(), repo.trim(), branch.trim());
        if project_id.is_empty() || repo.is_empty() || branch.is_empty() {
            return;
        }
        let e = GithubBranchRule {
            require,
            at: Utc::now(),
        };
        self.state
            .write()
            .github_branch_rules
            .insert(github_branch_rule_key(project_id, repo, branch), e);
        if let Some(db) = &self.db {
            let _ = db.upsert_github_branch_rule(project_id, repo, branch, e);
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Finds the escalation for (workspace, agent). With an agent code: its own
/// row, else a workspace row it owns or that has no owner. Without: the
/// lowest-keyed open row for the workspace, else the workspace row, else any
/// row for the workspace.
fn lookup_escalation(
    s: &State,
    workspace_id: &str,
    agent_code: &str,
) -> Option<(String, Escalation)> {
    let agent_code = agent_code.trim();
    if !agent_code.is_empty() {
        let key = escalation_key(workspace_id, agent_code);
        if let Some(e) = s.escalations.get(&key) {
            return Some((key, e.clone()));
        }
        if let Some(e) = s.escalations.get(workspace_id) {
            let ac = e.agent_code.trim();
            if ac.is_empty() || ac == agent_code {
                return Some((workspace_id.to_owned(), e.clone()));
            }
        }
        return None;
    }
    let prefix = format!("{workspace_id}{ESCALATION_KEY_SEP}");
    let mut any: Option<(&String, &Escalation)> = None;
    let mut open: Option<(&String, &Escalation)> = None;
    for (k, e) in &s.escalations {
        if e.workspace_id != workspace_id && k != workspace_id && !k.starts_with(&prefix) {
            continue;
        }
        if any.is_none() || k == workspace_id {
            any = Some((k, e));
        }
        if e.status == "open" && open.is_none_or(|(ok, _)| k < ok) {
            open = Some((k, e));
        }
    }
    if let Some((k, e)) = open {
        return Some((k.clone(), e.clone()));
    }
    if let Some(e) = s.escalations.get(workspace_id) {
        return Some((workspace_id.to_owned(), e.clone()));
    }
    any.map(|(k, e)| (k.clone(), e.clone()))
}
