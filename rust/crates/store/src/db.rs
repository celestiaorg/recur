//! The SQLite layer behind the in-memory [`crate::Store`].
//!
//! Simple, queried-by-id tables (projects, groups, findings, policy,
//! settings, inbox, escalations) are normalized into columns or singleton
//! docs; rich nested aggregates (a PR's commits/files, an agent's steps, a
//! workspace's feed and convergence) are JSON documents in one column.
//!
//! Go caps `database/sql` at one connection, so every statement and
//! transaction is serialized on it. Here that connection sits behind a
//! mutex: a transaction holds it from begin to commit, exactly like Go's
//! pooled single connection.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use chrono::{DateTime, TimeZone as _, Utc};
use parking_lot::Mutex;
use recur_domain::json::to_string as marshal;
use recur_domain::{
    AnalysisRun, Escalation, Finding, FindingNote, IdType, MergeCommitMessage,
    MergeMethod, Project, ProjectGroup, Severity, SquashCommitMessage, StoredEvent,
    UpdateBaseMethod, new_unique_id,
};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Error, Result, SqlContext as _, json_err};
use crate::migrations::run_migrations;
use crate::store::{State, ensure_finding_created_at, escalation_key, fill_missing_pr_times};

pub(crate) const ESCALATION_KEY_SEP: &str = "\x1e";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GithubBranchRule {
    pub require: bool,
    pub at: DateTime<Utc>,
}

pub(crate) fn github_branch_rule_key(project_id: &str, repo: &str, branch: &str) -> String {
    format!("{project_id}\x00{repo}\x00{branch}")
}

pub(crate) struct Db {
    conn: Mutex<Option<Connection>>,
    /// Strictly decreasing sort values for prepended feed rows, seeded at or
    /// below the smallest sort on disk (and never above zero), so a prepend
    /// never collides with an existing row and always sorts ahead of the
    /// 0..N-1 numbering a full rewrite produces.
    feed_sort: AtomicI64,
}

impl Db {
    /// Opens (creating if needed) the database at path, applies migrations,
    /// backfills finding identities, primes the feed counter and flushes the
    /// WAL.
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)
            .ctx(|| format!("open sqlite {:?}", path.to_string_lossy()))?;
        // A transient lock (a just-closed connection's WAL checkpoint, a quick
        // close→reopen) should wait briefly rather than fail with SQLITE_BUSY,
        // and must cover the migration DDL too.
        conn.busy_timeout(Duration::from_millis(5000))
            .ctx(|| "set busy_timeout".into())?;
        run_migrations(&mut conn)?;
        backfill_finding_identities(&mut conn)?;
        let min: i64 = conn
            .query_row("SELECT COALESCE(MIN(sort), 0) FROM feed", [], |r| r.get(0))
            .ctx(|| "seed feed sort".into())?;
        let db = Self {
            conn: Mutex::new(Some(conn)),
            feed_sort: AtomicI64::new(min.min(0)),
        };
        // Replay can leave hours of committed rows in the WAL; flush them so
        // a crash or a file copy is not a last-checkpoint view.
        db.checkpoint();
        Ok(db)
    }

    /// Runs f on the connection, holding it for the duration.
    pub fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock();
        match guard.as_mut() {
            Some(c) => f(c),
            None => Err(Error::Closed),
        }
    }

    /// Runs f in a transaction, committing when it returns Ok and rolling
    /// back otherwise.
    pub fn tx<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        self.with(|c| {
            let tx = c.transaction().bare()?;
            let out = f(&tx)?;
            tx.commit().bare()?;
            Ok(out)
        })
    }

    /// Checkpoints and drops the connection; later calls fail with
    /// [`Error::Closed`]. Closing twice is a no-op.
    pub fn close(&self) -> Result<()> {
        self.checkpoint();
        let conn = self.conn.lock().take();
        match conn {
            Some(c) => c.close().map_err(|(_, e)| e).bare(),
            None => Ok(()),
        }
    }

    /// Moves committed WAL frames into recur.db (TRUNCATE) so a byte copy of
    /// the main file, or a sqlite3 CLI that cannot see this process's WAL,
    /// still has every committed row. Best-effort.
    pub fn checkpoint(&self) {
        let _ = self.with(|c| {
            let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
            Ok(())
        });
    }

    pub fn next_feed_sort(&self) -> i64 {
        self.feed_sort.fetch_sub(1, Ordering::SeqCst) - 1
    }

    /// Whether the database was never initialized, keyed on the settings
    /// singleton (not project count): a user who deletes every project, or
    /// clears all data, must not be re-seeded.
    pub fn needs_seed(&self) -> Result<bool> {
        self.with(|c| {
            let n: i64 = c
                .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
                .ctx(|| "count settings".into())?;
            Ok(n == 0)
        })
    }

    /// Removes every row a workspace owns from a workspace-keyed table.
    pub fn delete_by_workspace(&self, table: &str, workspace_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                &format!("DELETE FROM {table} WHERE workspace_id = ?"),
                [workspace_id],
            )
            .ctx(|| format!("delete {table} by loop {workspace_id:?}"))?;
            Ok(())
        })
    }

    /// Deletes all user data but keeps the policy/settings/resource
    /// singletons and the PR number sequence: the DB stays initialized and
    /// PR numbers keep climbing.
    pub fn clear_data(&self) -> Result<()> {
        self.with(|c| {
            // "session" cascades message/message_content/session_context.
            for t in [
                "github_branch_rules",
                "projects",
                "project_groups",
                "workspaces",
                "agents",
                "feed",
                "prs",
                "escalations",
                "inbox",
                "finding_notes",
                "findings",
                "events",
                "analysis",
                "session",
            ] {
                c.execute(&format!("DELETE FROM {t}"), [])
                    .ctx(|| format!("clear {t}"))?;
            }
            Ok(())
        })
    }

    /// Writes the whole in-memory state into an empty database in one
    /// transaction.
    pub fn seed_all(&self, s: &State) -> Result<()> {
        self.tx(|tx| {
            for (i, g) in s.project_groups.iter().enumerate() {
                upsert_project_group_tx(tx, g, i)?;
            }
            for (i, p) in s.projects.iter().enumerate() {
                upsert_project_tx(tx, p, i)?;
            }
            for (i, w) in s.workspaces.iter().enumerate() {
                upsert_doc_tx(tx, "workspaces", &w.id, w, i)?;
            }
            for (ws, agents) in &s.agents {
                replace_list_tx(tx, "agents", ws, agents)?;
            }
            for (ws, items) in &s.feed {
                replace_list_tx(tx, "feed", ws, items)?;
            }
            for (i, p) in s.prs.iter().enumerate() {
                upsert_doc_tx(tx, "prs", &p.id, p, i)?;
            }
            for (key, e) in &s.escalations {
                upsert_escalation_tx(tx, key, e)?;
            }
            for (i, it) in s.inbox.iter().enumerate() {
                upsert_doc_tx(tx, "inbox", &it.id, it, i)?;
            }
            for (ws, fs) in &s.findings {
                for (i, f) in fs.iter().enumerate() {
                    upsert_finding_tx(tx, f, ws, i)?;
                }
            }
            for a in &s.analysis {
                upsert_analysis_tx(tx, a)?;
            }
            upsert_singleton_tx(tx, "policy", &s.policy)?;
            upsert_singleton_tx(tx, "settings", &s.settings)?;
            upsert_singleton_tx(tx, "resource", &s.resource)?;
            set_pr_number_seq_tx(tx, s.pr_number_seq)
        })
    }

    /// Reads the persisted state back into s.
    pub fn load_all(&self, s: &mut State) -> Result<()> {
        self.with(|c| {
            s.project_groups = load_project_groups(c)?;
            s.projects = load_projects(c)?;
            s.workspaces = load_doc_list(c, "workspaces")?;
            s.prs = load_doc_list(c, "prs")?;
            Ok(())
        })?;
        self.backfill_pr_times(s)?;
        self.with(|c| {
            s.inbox = load_doc_list(c, "inbox")?;
            s.agents = load_doc_by_workspace(c, "agents")?;
            s.feed = load_doc_by_workspace(c, "feed")?;
            s.escalations = load_escalations(c)?;
            s.findings = load_findings(c)?;
            s.analysis = load_analysis(c)?;
            if let Some(p) = load_singleton(c, "policy")? {
                s.policy = p;
            }
            if let Some(v) = load_singleton(c, "settings")? {
                s.settings = v;
            }
            if let Some(r) = load_singleton(c, "resource")? {
                s.resource = r;
            }
            s.pr_number_seq = load_pr_number_seq(c)?;
            s.github_branch_rules = load_github_branch_rules(c)?;
            Ok(())
        })
    }

    /// Copies a known sibling stamp onto PRs that have only one of
    /// CreatedAt / UpdatedAt and persists them.
    pub fn backfill_pr_times(&self, s: &mut State) -> Result<()> {
        let dirty: Vec<usize> = s
            .prs
            .iter_mut()
            .enumerate()
            .filter_map(|(i, p)| fill_missing_pr_times(p).then_some(i))
            .collect();
        if dirty.is_empty() {
            return Ok(());
        }
        self.with(|c| {
            let tx = c
                .transaction()
                .ctx(|| "backfill PR times".into())?;
            for &i in &dirty {
                upsert_doc_tx(&tx, "prs", &s.prs[i].id, &s.prs[i], i)?;
            }
            tx.commit().ctx(|| "backfill PR times".into())
        })
    }

    pub fn upsert_project(&self, p: &Project, sort: usize) -> Result<()> {
        self.with(|c| upsert_project_tx(c, p, sort))
    }

    pub fn upsert_project_group(&self, g: &ProjectGroup, sort: usize) -> Result<()> {
        self.with(|c| upsert_project_group_tx(c, g, sort))
    }

    /// Clears members' group_id and deletes the group in one transaction.
    pub fn delete_project_group_and_ungroup(&self, id: &str) -> Result<()> {
        self.tx(|tx| {
            tx.execute("UPDATE projects SET group_id = '' WHERE group_id = ?", [id])
                .ctx(|| format!("clear project group_id {id:?}"))?;
            tx.execute("DELETE FROM project_groups WHERE id = ?", [id])
                .ctx(|| format!("delete project_group {id:?}"))?;
            Ok(())
        })
    }

    /// Writes `projects.sort = 0..N-1` for ids (a complete permutation).
    pub fn set_project_sorts(&self, ids: &[String]) -> Result<()> {
        self.set_sorts("projects", "project", "project", ids)
    }

    pub fn set_project_group_sorts(&self, ids: &[String]) -> Result<()> {
        self.set_sorts("project_groups", "project_group", "group", ids)
    }

    fn set_sorts(&self, table: &str, label: &str, noun: &str, ids: &[String]) -> Result<()> {
        self.tx(|tx| {
            for (i, id) in ids.iter().enumerate() {
                let n = tx
                    .execute(&format!("UPDATE {table} SET sort = ? WHERE id = ?"), params![i as i64, id])
                    .ctx(|| format!("set {label} sort {id:?}"))?;
                if n == 0 {
                    return Err(Error::msg(format!("set {label} sort: unknown {noun} {id:?}")));
                }
            }
            Ok(())
        })
    }

    pub fn upsert_doc<T: Serialize>(&self, table: &str, id: &str, v: &T, sort: usize) -> Result<()> {
        self.with(|c| upsert_doc_tx(c, table, id, v, sort))
    }

    /// Removes one doc-table row by id; a missing id is not an error.
    pub fn delete_doc(&self, table: &str, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(&format!("DELETE FROM {table} WHERE id = ?"), [id])
                .ctx(|| format!("delete {table} {id:?}"))?;
            Ok(())
        })
    }

    pub fn upsert_escalation(&self, key: &str, e: &Escalation) -> Result<()> {
        self.with(|c| upsert_escalation_tx(c, key, e))
    }

    pub fn delete_escalations(&self, workspace_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM escalations WHERE workspace_id = ? OR workspace_id LIKE ?",
                params![workspace_id, format!("{workspace_id}{ESCALATION_KEY_SEP}%")],
            )
            .ctx(|| format!("delete escalations for {workspace_id:?}"))?;
            Ok(())
        })
    }

    pub fn upsert_singleton<T: Serialize>(&self, table: &str, v: &T) -> Result<()> {
        self.with(|c| upsert_singleton_tx(c, table, v))
    }

    pub fn set_pr_number_seq(&self, n: i64) -> Result<()> {
        self.with(|c| set_pr_number_seq_tx(c, n))
    }

    /// Swaps all rows for a workspace in a per-workspace list table.
    pub fn replace_list<T: Serialize>(&self, table: &str, workspace_id: &str, items: &[T]) -> Result<()> {
        self.tx(|tx| replace_list_tx(tx, table, workspace_id, items))
    }

    /// Inserts one new newest-first feed row with a sort below every
    /// existing one (rows load back ORDER BY sort), instead of rewriting the
    /// workspace's rows.
    pub fn prepend_feed_item<T: Serialize>(&self, workspace_id: &str, item: &T) -> Result<()> {
        let doc = marshal(item).map_err(json_err(""))?;
        let sort = self.next_feed_sort();
        self.with(|c| {
            c.execute(
                "INSERT INTO feed (workspace_id, doc_json, sort) VALUES (?,?,?)",
                params![workspace_id, doc, sort],
            )
            .ctx(|| format!("prepend feed for {workspace_id:?}"))?;
            Ok(())
        })
    }

    /// Inserts one durable event; returns its autoincrement id.
    pub fn append_event(&self, e: &StoredEvent) -> Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO events (workspace_id, agent_id, type, payload_json, ts) VALUES (?,?,?,?,?)",
                params![e.workspace_id, e.agent_id, e.r#type, e.payload_json, e.ts],
            )
            .ctx(|| "append event".into())?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Inserts or replaces one finding and its notes in one transaction.
    pub fn upsert_finding(&self, f: &Finding, sort: usize) -> Result<()> {
        self.tx(|tx| upsert_finding_tx(tx, f, &f.workspace_id, sort))
    }

    pub fn upsert_analysis(&self, a: &AnalysisRun) -> Result<()> {
        self.with(|c| upsert_analysis_tx(c, a))
    }

    /// Every durable event for a workspace in insertion order.
    pub fn events_for_workspace(&self, workspace_id: &str) -> Result<Option<Vec<StoredEvent>>> {
        self.with(|c| {
            let ctx = || format!("query events for {workspace_id:?}");
            let mut stmt = c
                .prepare(
                    "SELECT id, workspace_id, agent_id, type, payload_json, ts FROM events WHERE workspace_id = ? ORDER BY id",
                )
                .ctx(ctx)?;
            let rows = stmt
                .query_map([workspace_id], |r| {
                    Ok(StoredEvent {
                        id: r.get(0)?,
                        workspace_id: r.get(1)?,
                        agent_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        r#type: r.get(3)?,
                        payload_json: r.get(4)?,
                        ts: r.get(5)?,
                    })
                })
                .ctx(ctx)?;
            let mut out = Vec::new();
            for e in rows {
                out.push(e.ctx(|| "scan event".into())?);
            }
            Ok((!out.is_empty()).then_some(out))
        })
    }

    pub fn upsert_github_branch_rule(
        &self,
        project_id: &str,
        repo: &str,
        branch: &str,
        e: GithubBranchRule,
    ) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO github_branch_rules (project_id, repo, branch, require_pr, checked_at)
		VALUES (?,?,?,?,?)
		ON CONFLICT(project_id, repo, branch) DO UPDATE SET require_pr=excluded.require_pr, checked_at=excluded.checked_at",
                params![project_id, repo, branch, i64::from(e.require), e.at.timestamp_millis()],
            )
            .ctx(|| format!("upsert github_branch_rules {project_id} {repo} {branch}"))?;
            Ok(())
        })
    }
}

/// Go's `json.Unmarshal` leaves a field at its zero value for JSON `null`.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> std::result::Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// The JSON shape of `projects.extra_json` (machine-local fields).
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ProjectExtra {
    #[serde(deserialize_with = "nullable")]
    pic: String,
    #[serde(deserialize_with = "nullable")]
    color: String,
    #[serde(deserialize_with = "nullable")]
    open: bool,
    #[serde(deserialize_with = "nullable")]
    idle: bool,
    #[serde(deserialize_with = "nullable")]
    hidden: bool,
    #[serde(rename = "prOnGithub", deserialize_with = "nullable")]
    pr_on_github: bool,
    #[serde(deserialize_with = "nullable")]
    delete_branch_on_remove: bool,
    #[serde(deserialize_with = "nullable")]
    linear_projects: String,
    #[serde(deserialize_with = "nullable")]
    merge_method: MergeMethod,
    #[serde(deserialize_with = "nullable")]
    update_base_method: UpdateBaseMethod,
    #[serde(deserialize_with = "nullable")]
    squash_commit_message: SquashCommitMessage,
    #[serde(deserialize_with = "nullable")]
    merge_commit_message: MergeCommitMessage,
    #[serde(deserialize_with = "nullable")]
    pinned_run: String,
    #[serde(deserialize_with = "nullable")]
    disabled_skills: Vec<String>,
    #[serde(deserialize_with = "nullable")]
    enabled_skills: Vec<String>,
    #[serde(rename = "disabledMCP", deserialize_with = "nullable")]
    disabled_mcp: Vec<String>,
    #[serde(rename = "enabledMCP", deserialize_with = "nullable")]
    enabled_mcp: Vec<String>,
}

fn load_project_groups(c: &Connection) -> Result<Vec<ProjectGroup>> {
    let mut stmt = c
        .prepare("SELECT id, name, open, created_at, updated_at FROM project_groups ORDER BY sort")
        .ctx(|| "query project_groups".into())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ProjectGroup {
                id: r.get(0)?,
                name: r.get(1)?,
                open: r.get::<_, i64>(2)? != 0,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
            })
        })
        .ctx(|| "query project_groups".into())?;
    let mut out = Vec::new();
    for g in rows {
        out.push(g.ctx(|| "scan project_group".into())?);
    }
    Ok(out)
}

fn load_projects(c: &Connection) -> Result<Vec<Project>> {
    let mut stmt = c
        .prepare("SELECT id, name, repo_path, base_branch, group_id, extra_json FROM projects ORDER BY sort")
        .ctx(|| "query projects".into())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                Project {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    repo_path: r.get(2)?,
                    base_branch: r.get(3)?,
                    group_id: r.get(4)?,
                    ..Default::default()
                },
                r.get::<_, String>(5)?,
            ))
        })
        .ctx(|| "query projects".into())?;
    let mut out = Vec::new();
    for row in rows {
        let (mut p, extra) = row.ctx(|| "scan project".into())?;
        let e: ProjectExtra =
            serde_json::from_str(&extra).map_err(json_err("unmarshal project extra"))?;
        p.pic = e.pic;
        p.color = e.color;
        p.open = e.open;
        p.idle = e.idle;
        p.hidden = e.hidden;
        p.pr_on_github = e.pr_on_github;
        p.delete_branch_on_remove = e.delete_branch_on_remove;
        p.linear_projects = e.linear_projects;
        p.merge_method = e.merge_method;
        p.update_base_method = e.update_base_method;
        p.squash_commit_message = e.squash_commit_message;
        p.merge_commit_message = e.merge_commit_message;
        p.pinned_run = e.pinned_run;
        p.disabled_skills = e.disabled_skills;
        p.enabled_skills = e.enabled_skills;
        p.disabled_mcp = e.disabled_mcp;
        p.enabled_mcp = e.enabled_mcp;
        out.push(p);
    }
    Ok(out)
}

fn load_escalations(c: &Connection) -> Result<BTreeMap<String, Escalation>> {
    let mut out = BTreeMap::new();
    for doc in query_strings(c, "SELECT doc_json FROM escalations", "escalations")? {
        let e: Escalation = serde_json::from_str(&doc).map_err(json_err("unmarshal escalation"))?;
        out.insert(escalation_key(&e.workspace_id, &e.agent_code), e);
    }
    Ok(out)
}

fn query_strings(c: &Connection, q: &str, table: &str) -> Result<Vec<String>> {
    let mut stmt = c.prepare(q).ctx(|| format!("query {table}"))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .ctx(|| format!("query {table}"))?;
    let mut out = Vec::new();
    for d in rows {
        out.push(d.ctx(|| format!("scan {table}"))?);
    }
    Ok(out)
}

fn load_doc_list<T: DeserializeOwned>(c: &Connection, table: &str) -> Result<Vec<T>> {
    query_strings(c, &format!("SELECT doc_json FROM {table} ORDER BY sort"), table)?
        .iter()
        .map(|doc| serde_json::from_str(doc).map_err(json_err(format!("unmarshal {table}"))))
        .collect()
}

fn load_doc_by_workspace<T: DeserializeOwned>(c: &Connection, table: &str) -> Result<BTreeMap<String, Vec<T>>> {
    let mut stmt = c
        .prepare(&format!("SELECT workspace_id, doc_json FROM {table} ORDER BY sort"))
        .ctx(|| format!("query {table}"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .ctx(|| format!("query {table}"))?;
    let mut out: BTreeMap<String, Vec<T>> = BTreeMap::new();
    for row in rows {
        let (ws, doc) = row.ctx(|| format!("scan {table}"))?;
        let v = serde_json::from_str(&doc).map_err(json_err(format!("unmarshal {table}")))?;
        out.entry(ws).or_default().push(v);
    }
    Ok(out)
}

const FINDING_SELECT_COLS: &str = "id, short_id, workspace_id, iteration, file, line, end_line,
	severity, message, summary, status, status_reason, created_at, updated_by, updated_at";

fn scan_finding(r: &rusqlite::Row<'_>) -> rusqlite::Result<Finding> {
    let mut f = Finding {
        id: r.get(0)?,
        short_id: r.get(1)?,
        workspace_id: r.get(2)?,
        iteration: r.get(3)?,
        file: r.get(4)?,
        line: r.get(5)?,
        end_line: r.get(6)?,
        severity: Severity::from(r.get::<_, String>(7)?),
        message: r.get(8)?,
        summary: r.get(9)?,
        status: r.get(10)?,
        status_reason: r.get(11)?,
        created_at: r.get(12)?,
        updated_by: r.get(13)?,
        updated_at: r.get(14)?,
        ..Default::default()
    };
    ensure_finding_created_at(&mut f);
    Ok(f)
}

/// Rebuilds the per-workspace findings map, notes attached.
fn load_findings(c: &Connection) -> Result<BTreeMap<String, Vec<Finding>>> {
    let mut stmt = c
        .prepare(&format!(
            "SELECT {FINDING_SELECT_COLS} FROM findings ORDER BY workspace_id, sort, id"
        ))
        .ctx(|| "query findings".into())?;
    let rows = stmt.query_map([], scan_finding).ctx(|| "query findings".into())?;
    let mut out: BTreeMap<String, Vec<Finding>> = BTreeMap::new();
    let mut index: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for f in rows {
        let f = f.ctx(|| "scan finding".into())?;
        let list = out.entry(f.workspace_id.clone()).or_default();
        index.insert(f.id.clone(), (f.workspace_id.clone(), list.len()));
        list.push(f);
    }
    for (finding_id, notes) in load_finding_notes(c)? {
        if let Some((ws, i)) = index.get(&finding_id)
            && let Some(list) = out.get_mut(ws)
        {
            list[*i].notes = notes;
        }
    }
    Ok(out)
}

fn load_finding_notes(c: &Connection) -> Result<BTreeMap<String, Vec<FindingNote>>> {
    let mut stmt = c
        .prepare("SELECT id, finding_id, author, body, created_at FROM finding_notes ORDER BY finding_id, sort, created_at")
        .ctx(|| "query finding notes".into())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                FindingNote {
                    id: r.get(0)?,
                    author: r.get(2)?,
                    body: r.get(3)?,
                    created_at: r.get(4)?,
                },
            ))
        })
        .ctx(|| "query finding notes".into())?;
    let mut out: BTreeMap<String, Vec<FindingNote>> = BTreeMap::new();
    for row in rows {
        let (fid, n) = row.ctx(|| "scan finding note".into())?;
        out.entry(fid).or_default().push(n);
    }
    Ok(out)
}

/// Fills short_id (and CreatedAt) on legacy finding rows. Existing ids stay
/// put so transcript refs keep resolving. Safe on every open.
fn backfill_finding_identities(conn: &mut Connection) -> Result<()> {
    struct Row {
        id: String,
        workspace_id: String,
        short_id: String,
        created_at: String,
        updated_at: String,
    }
    let list: Vec<Row> = {
        let mut stmt = conn
            .prepare("SELECT id, workspace_id, short_id, created_at, updated_at FROM findings")
            .ctx(|| "query findings for identity backfill".into())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    workspace_id: r.get(1)?,
                    short_id: r.get(2)?,
                    created_at: r.get(3)?,
                    updated_at: r.get(4)?,
                })
            })
            .ctx(|| "query findings for identity backfill".into())?;
        let mut list = Vec::new();
        for r in rows {
            list.push(r.ctx(|| "scan finding for identity backfill".into())?);
        }
        list
    };
    if list.is_empty() {
        return Ok(());
    }
    let mut used: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in &list {
        if !r.short_id.is_empty() {
            used.entry(r.workspace_id.clone())
                .or_default()
                .insert(r.short_id.clone());
        }
    }

    let tx = conn
        .transaction()
        .ctx(|| "begin finding identity backfill".into())?;
    for r in &list {
        let mut f = Finding {
            id: r.id.clone(),
            short_id: r.short_id.clone(),
            workspace_id: r.workspace_id.clone(),
            created_at: r.created_at.clone(),
            updated_at: r.updated_at.clone(),
            ..Default::default()
        };
        ensure_finding_created_at(&mut f);
        let ws_used = used.entry(r.workspace_id.clone()).or_default();
        if f.short_id.is_empty() {
            let mut taken = |id: &str| ws_used.contains(id);
            f.short_id = new_unique_id(&IdType::Finding, Some(&mut taken))
                .map_err(|e| Error::msg(format!("mint finding short id: {e}")))?;
        }
        ws_used.insert(f.short_id.clone());
        if f.short_id == r.short_id && f.created_at == r.created_at {
            continue;
        }
        tx.execute(
            "UPDATE findings SET short_id=?, created_at=? WHERE id=?",
            params![f.short_id, f.created_at, r.id],
        )
        .ctx(|| format!("backfill finding {:?}", r.id))?;
    }
    tx.commit().ctx(|| "commit finding identity backfill".into())
}

fn load_singleton<T: DeserializeOwned>(c: &Connection, table: &str) -> Result<Option<T>> {
    let doc: Option<String> = c
        .query_row(&format!("SELECT doc_json FROM {table} WHERE id = 1"), [], |r| r.get(0))
        .optional()
        .ctx(|| format!("query {table}"))?;
    match doc {
        None => Ok(None),
        Some(doc) => serde_json::from_str(&doc)
            .map(Some)
            .map_err(json_err(format!("unmarshal {table}"))),
    }
}

fn load_pr_number_seq(c: &Connection) -> Result<i64> {
    Ok(c
        .query_row("SELECT last_number FROM pr_seq WHERE id = 1", [], |r| r.get(0))
        .optional()
        .ctx(|| "query pr_seq".into())?
        .unwrap_or(0))
}

fn load_analysis(c: &Connection) -> Result<Vec<AnalysisRun>> {
    query_strings(c, "SELECT doc_json FROM analysis ORDER BY ts DESC", "analysis")?
        .iter()
        .map(|doc| serde_json::from_str(doc).map_err(json_err("unmarshal analysis")))
        .collect()
}

fn load_github_branch_rules(c: &Connection) -> Result<BTreeMap<String, GithubBranchRule>> {
    let ctx = || "query github_branch_rules".to_owned();
    let mut stmt = c
        .prepare("SELECT project_id, repo, branch, require_pr, checked_at FROM github_branch_rules")
        .ctx(ctx)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .ctx(ctx)?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (p, repo, branch, require, checked) =
            row.ctx(|| "scan github_branch_rules".into())?;
        out.insert(
            github_branch_rule_key(&p, &repo, &branch),
            GithubBranchRule {
                require: require != 0,
                at: Utc.timestamp_millis_opt(checked).single().unwrap_or_default(),
            },
        );
    }
    Ok(out)
}

pub(crate) fn upsert_project_group_tx(c: &Connection, g: &ProjectGroup, sort: usize) -> Result<()> {
    c.execute(
        "INSERT INTO project_groups (id, name, open, sort, created_at, updated_at)
		VALUES (?,?,?,?,?,?)
		ON CONFLICT(id) DO UPDATE SET
			name=excluded.name, open=excluded.open, updated_at=excluded.updated_at",
        params![g.id, g.name, i64::from(g.open), sort as i64, g.created_at, g.updated_at],
    )
    .ctx(|| format!("upsert project_group {:?}", g.id))?;
    Ok(())
}

pub(crate) fn upsert_project_tx(c: &Connection, p: &Project, sort: usize) -> Result<()> {
    // extra_json carries machine-local fields without their own columns.
    // Run-config lives only in project.toml.
    let extra = marshal(&Project {
        pic: p.pic.clone(),
        color: p.color.clone(),
        open: p.open,
        idle: p.idle,
        hidden: p.hidden,
        pr_on_github: p.pr_on_github,
        delete_branch_on_remove: p.delete_branch_on_remove,
        linear_projects: p.linear_projects.clone(),
        merge_method: p.merge_method.clone(),
        update_base_method: p.update_base_method.clone(),
        squash_commit_message: p.squash_commit_message.clone(),
        merge_commit_message: p.merge_commit_message.clone(),
        pinned_run: p.pinned_run.clone(),
        disabled_skills: p.disabled_skills.clone(),
        enabled_skills: p.enabled_skills.clone(),
        disabled_mcp: p.disabled_mcp.clone(),
        enabled_mcp: p.enabled_mcp.clone(),
        ..Default::default()
    })
    .map_err(json_err(""))?;
    c.execute(
        "INSERT INTO projects (id, name, repo_path, base_branch, group_id, extra_json, sort)
		VALUES (?,?,?,?,?,?,?)
		ON CONFLICT(id) DO UPDATE SET
			name=excluded.name, repo_path=excluded.repo_path, base_branch=excluded.base_branch,
			group_id=excluded.group_id, extra_json=excluded.extra_json",
        params![p.id, p.name, p.repo_path, p.base_branch, p.group_id, extra, sort as i64],
    )
    .ctx(|| format!("upsert project {:?}", p.id))?;
    Ok(())
}

pub(crate) fn upsert_doc_tx<T: Serialize>(c: &Connection, table: &str, id: &str, v: &T, sort: usize) -> Result<()> {
    let doc = marshal(v).map_err(json_err(""))?;
    c.execute(
        &format!(
            "INSERT INTO {table} (id, doc_json, sort) VALUES (?,?,?)
		ON CONFLICT(id) DO UPDATE SET doc_json=excluded.doc_json"
        ),
        params![id, doc, sort as i64],
    )
    .ctx(|| format!("upsert {table} {id:?}"))?;
    Ok(())
}

fn upsert_escalation_tx(c: &Connection, key: &str, e: &Escalation) -> Result<()> {
    let doc = marshal(e).map_err(json_err(""))?;
    c.execute(
        "INSERT INTO escalations (workspace_id, doc_json) VALUES (?,?)
		ON CONFLICT(workspace_id) DO UPDATE SET doc_json=excluded.doc_json",
        params![key, doc],
    )
    .ctx(|| format!("upsert escalation {key:?}"))?;
    Ok(())
}

fn upsert_singleton_tx<T: Serialize>(c: &Connection, table: &str, v: &T) -> Result<()> {
    let doc = marshal(v).map_err(json_err(""))?;
    c.execute(
        &format!(
            "INSERT INTO {table} (id, doc_json) VALUES (1, ?)
		ON CONFLICT(id) DO UPDATE SET doc_json=excluded.doc_json"
        ),
        [doc],
    )
    .ctx(|| format!("upsert {table}"))?;
    Ok(())
}

fn set_pr_number_seq_tx(c: &Connection, n: i64) -> Result<()> {
    c.execute(
        "INSERT INTO pr_seq (id, last_number) VALUES (1, ?)
		ON CONFLICT(id) DO UPDATE SET last_number=excluded.last_number",
        [n],
    )
    .ctx(|| "upsert pr_seq".into())?;
    Ok(())
}

fn replace_list_tx<T: Serialize>(c: &Connection, table: &str, workspace_id: &str, items: &[T]) -> Result<()> {
    c.execute(&format!("DELETE FROM {table} WHERE workspace_id = ?"), [workspace_id])
        .ctx(|| format!("clear {table} for {workspace_id:?}"))?;
    for (i, it) in items.iter().enumerate() {
        let doc = marshal(it).map_err(json_err(""))?;
        c.execute(
            &format!("INSERT INTO {table} (workspace_id, doc_json, sort) VALUES (?,?,?)"),
            params![workspace_id, doc, i as i64],
        )
        .ctx(|| format!("insert {table} for {workspace_id:?}"))?;
    }
    Ok(())
}

fn upsert_analysis_tx(c: &Connection, a: &AnalysisRun) -> Result<()> {
    let doc = marshal(a).map_err(json_err(""))?;
    c.execute(
        "INSERT INTO analysis (id, project_id, workspace_id, doc_json, ts) VALUES (?,?,?,?,?)
			ON CONFLICT(id) DO UPDATE SET doc_json=excluded.doc_json, project_id=excluded.project_id, workspace_id=excluded.workspace_id, ts=excluded.ts",
        params![a.id, a.project_id, a.workspace_id, doc, a.ts],
    )
    .ctx(|| format!("upsert analysis {:?}", a.id))?;
    Ok(())
}

/// Writes one finding and replaces its notes.
pub(crate) fn upsert_finding_tx(c: &Connection, f: &Finding, workspace_id: &str, sort: usize) -> Result<()> {
    c.execute(
        "INSERT INTO findings (
			id, short_id, workspace_id, iteration, file, line, end_line,
			severity, message, summary, status, status_reason,
			created_at, updated_by, updated_at, sort
		) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
			ON CONFLICT(id) DO UPDATE SET
				short_id=excluded.short_id,
				workspace_id=excluded.workspace_id,
				iteration=excluded.iteration,
				file=excluded.file,
				line=excluded.line,
				end_line=excluded.end_line,
				severity=excluded.severity,
				message=excluded.message,
				summary=excluded.summary,
				status=excluded.status,
				status_reason=excluded.status_reason,
				created_at=excluded.created_at,
				updated_by=excluded.updated_by,
				updated_at=excluded.updated_at",
        params![
            f.id,
            f.short_id,
            workspace_id,
            f.iteration,
            f.file,
            f.line,
            f.end_line,
            f.severity.as_str(),
            f.message,
            f.summary,
            f.status,
            f.status_reason,
            f.created_at,
            f.updated_by,
            f.updated_at,
            sort as i64
        ],
    )
    .ctx(|| format!("upsert finding {:?}", f.id))?;
    c.execute("DELETE FROM finding_notes WHERE finding_id = ?", [&f.id])
        .ctx(|| format!("clear finding notes {:?}", f.id))?;
    for (i, n) in f.notes.iter().enumerate() {
        if n.id.is_empty() {
            continue;
        }
        c.execute(
            "INSERT INTO finding_notes (id, finding_id, author, body, created_at, sort) VALUES (?,?,?,?,?,?)",
            params![n.id, f.id, n.author, n.body, n.created_at, i as i64],
        )
        .ctx(|| format!("insert finding note {:?}", n.id))?;
    }
    Ok(())
}
