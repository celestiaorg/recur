//! Scheduled prompts: one row per schedule, one per model flag, one per
//! occurrence. Occurrences are claimed by (schedule_id, due_at), so a
//! repeated tick or a restart never launches the same due time twice.
//!
//! Time columns are ISO 8601 UTC text with milliseconds; the domain carries
//! unix ms. `''` means no time (a one-time schedule that already ran).

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone as _, Utc};
use recur_domain::{ScheduledPrompt, ScheduledPromptRun};
use rusqlite::{Connection, Row, ToSql, params};

use crate::error::{Error, Result, SqlContext as _};
use crate::store::Store;

pub(crate) fn iso_from_millis(ms: i64) -> String {
    if ms == 0 {
        return String::new();
    }
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_default()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

pub(crate) fn millis_from_iso(s: &str) -> Result<i64> {
    if s.is_empty() {
        return Ok(0);
    }
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.timestamp_millis())
        .map_err(|e| Error::msg(format!("bad timestamp {s:?}: {e}")))
}

const SCHEDULE_COLUMNS: &str =
    "id, project_id, name, prompt, base_branch, mode, model, simplify, loop_id, policy_id,
	cadence, cron, run_date, run_time, time_zone, paused, next_run_at";

/// The query behind [`Store::active_scheduled_prompt_runs`].
pub const RUN_SELECT: &str = "SELECT id, schedule_id, due_at, COALESCE(workspace_id, ''), status, detail FROM scheduled_prompt_run";

/// running, waiting and interrupted: interrupted stays in the overlap set so
/// a restart does not launch over unanswered work.
pub const ACTIVE_RUN_FILTER: &str = "status IN ('running', 'waiting', 'interrupted')";

fn scan_schedule(r: &Row<'_>) -> rusqlite::Result<(ScheduledPrompt, String)> {
    let p = ScheduledPrompt {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        prompt: r.get(3)?,
        base_branch: r.get(4)?,
        mode: r.get(5)?,
        model: r.get(6)?,
        simplify: Some(r.get::<_, bool>(7)?),
        loop_id: r.get(8)?,
        policy_id: r.get(9)?,
        cadence: r.get(10)?,
        cron: r.get(11)?,
        date: r.get(12)?,
        time: r.get(13)?,
        time_zone: r.get(14)?,
        paused: r.get(15)?,
        ..Default::default()
    };
    Ok((p, r.get(16)?))
}

fn scan_run(r: &Row<'_>) -> rusqlite::Result<(ScheduledPromptRun, String)> {
    Ok((
        ScheduledPromptRun {
            id: r.get(0)?,
            schedule_id: r.get(1)?,
            workspace_id: r.get(3)?,
            status: r.get(4)?,
            detail: r.get(5)?,
            ..Default::default()
        },
        r.get(2)?,
    ))
}

fn query_runs(c: &Connection, q: &str) -> Result<Vec<ScheduledPromptRun>> {
    let mut stmt = c.prepare(q).bare()?;
    let rows = stmt.query_map([], scan_run).bare()?;
    let mut out = Vec::new();
    for row in rows {
        let (mut r, due) = row.bare()?;
        r.due_at = millis_from_iso(&due)?;
        out.push(r);
    }
    Ok(out)
}

fn schedule_args(p: &ScheduledPrompt) -> Vec<Box<dyn ToSql>> {
    let simplify = p.simplify.unwrap_or(true);
    vec![
        Box::new(p.id.clone()),
        Box::new(p.project_id.clone()),
        Box::new(p.name.clone()),
        Box::new(p.prompt.clone()),
        Box::new(p.base_branch.clone()),
        Box::new(p.mode.clone()),
        Box::new(p.model.clone()),
        Box::new(simplify),
        Box::new(p.loop_id.clone()),
        Box::new(p.policy_id.clone()),
        Box::new(p.cadence.clone()),
        Box::new(p.cron.clone()),
        Box::new(p.date.clone()),
        Box::new(p.time.clone()),
        Box::new(p.time_zone.clone()),
        Box::new(p.paused),
        Box::new(iso_from_millis(p.next_run_at)),
    ]
}

/// Writes the schedule row and replaces its flags. A schedule's project is
/// fixed: an update naming another project changes nothing and errors.
fn upsert_schedule(c: &Connection, p: &ScheduledPrompt) -> Result<()> {
    let n = c
        .execute(
            &format!(
                "INSERT INTO scheduled_prompt({SCHEDULE_COLUMNS})
		VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
		ON CONFLICT(id) DO UPDATE SET
			name=excluded.name, prompt=excluded.prompt, base_branch=excluded.base_branch, mode=excluded.mode,
			model=excluded.model, simplify=excluded.simplify, loop_id=excluded.loop_id, policy_id=excluded.policy_id,
			cadence=excluded.cadence, cron=excluded.cron, run_date=excluded.run_date, run_time=excluded.run_time,
			time_zone=excluded.time_zone, paused=excluded.paused, next_run_at=excluded.next_run_at,
			updated_at=strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
		WHERE project_id = excluded.project_id"
            ),
            rusqlite::params_from_iter(schedule_args(p).iter()),
        )
        .bare()?;
    if n == 0 {
        return Err(Error::msg(format!(
            "schedule {} belongs to another project",
            p.id
        )));
    }
    c.execute(
        "DELETE FROM scheduled_prompt_flag WHERE schedule_id=?",
        [&p.id],
    )
    .bare()?;
    for (k, v) in &p.flags {
        c.execute(
            "INSERT INTO scheduled_prompt_flag(schedule_id, key, value) VALUES(?,?,?)",
            params![p.id, k, v],
        )
        .bare()?;
    }
    Ok(())
}

fn requires_db() -> Error {
    Error::msg("schedules require a database")
}

impl Store {
    fn query_schedules(&self, q: &str, args: &[&dyn ToSql]) -> Result<Vec<ScheduledPrompt>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        db.with(|c| {
            let mut out = Vec::new();
            {
                let mut stmt = c.prepare(q).bare()?;
                let rows = stmt.query_map(args, scan_schedule).bare()?;
                for row in rows {
                    let (mut p, next) = row.bare()?;
                    p.flags = BTreeMap::new();
                    p.next_run_at = millis_from_iso(&next)?;
                    out.push(p);
                }
            }
            if out.is_empty() {
                return Ok(out);
            }
            let by_id: BTreeMap<String, usize> = out
                .iter()
                .enumerate()
                .map(|(i, p)| (p.id.clone(), i))
                .collect();
            let mut stmt = c
                .prepare("SELECT schedule_id, key, value FROM scheduled_prompt_flag")
                .bare()?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .bare()?;
            for row in rows {
                let (id, k, v) = row.bare()?;
                if let Some(&i) = by_id.get(&id) {
                    out[i].flags.insert(k, v);
                }
            }
            Ok(out)
        })
    }

    /// Every schedule with its flags and latest occurrence.
    pub fn scheduled_prompts(&self) -> Result<Vec<ScheduledPrompt>> {
        let mut out = self.query_schedules(
            &format!("SELECT {SCHEDULE_COLUMNS} FROM scheduled_prompt ORDER BY created_at, rowid"),
            &[],
        )?;
        let Some(db) = &self.db else {
            return Ok(out);
        };
        if out.is_empty() {
            return Ok(out);
        }
        let by_id: BTreeMap<String, usize> = out
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();
        // Latest occurrence per schedule (most recently created row).
        let last = db.with(|c| {
            query_runs(
                c,
                "SELECT r.id, r.schedule_id, r.due_at, COALESCE(r.workspace_id, ''), r.status, r.detail
		FROM scheduled_prompt_run r
		WHERE r.rowid = (SELECT rowid FROM scheduled_prompt_run WHERE schedule_id = r.schedule_id ORDER BY created_at DESC, rowid DESC LIMIT 1)",
            )
        })?;
        for r in last {
            if let Some(&i) = by_id.get(&r.schedule_id) {
                out[i].last_run = Some(r);
            }
        }
        Ok(out)
    }

    /// The scheduler tick set: unpaused schedules that are due, plus
    /// `@reboot` cron schedules on startup. Skips last-run history.
    pub fn due_scheduled_prompts(
        &self,
        now: DateTime<Utc>,
        startup: bool,
    ) -> Result<Vec<ScheduledPrompt>> {
        let now = iso_from_millis(now.timestamp_millis());
        let start = i64::from(startup);
        self.query_schedules(
            &format!(
                "SELECT {SCHEDULE_COLUMNS} FROM scheduled_prompt
		WHERE paused = 0 AND ((next_run_at <> '' AND next_run_at <= ?) OR (? = 1 AND cadence = 'cron' AND cron = '@reboot'))
		ORDER BY created_at, rowid"
            ),
            &[&now, &start],
        )
    }

    /// Persists configuration without touching run history.
    pub fn save_scheduled_prompt(&self, p: &ScheduledPrompt) -> Result<()> {
        let db = self.db.as_ref().ok_or_else(requires_db)?;
        db.tx(|tx| upsert_schedule(tx, p))
    }

    /// Deletes the schedule and its history, keeping workspaces.
    pub fn delete_scheduled_prompt(&self, id: &str) -> Result<()> {
        let db = self.db.as_ref().ok_or_else(requires_db)?;
        db.with(|c| {
            c.execute("DELETE FROM scheduled_prompt WHERE id=?", [id])
                .bare()?;
            Ok(())
        })
    }

    /// Occurrences oldest first, including active ones.
    pub fn scheduled_prompt_runs(&self) -> Result<Vec<ScheduledPromptRun>> {
        match &self.db {
            None => Ok(Vec::new()),
            Some(db) => {
                db.with(|c| query_runs(c, &format!("{RUN_SELECT} ORDER BY created_at, rowid")))
            }
        }
    }

    /// Occurrences that still block the next launch.
    pub fn active_scheduled_prompt_runs(&self) -> Result<Vec<ScheduledPromptRun>> {
        match &self.db {
            None => Ok(Vec::new()),
            Some(db) => db.with(|c| {
                query_runs(
                    c,
                    &format!("{RUN_SELECT} WHERE {ACTIVE_RUN_FILTER} ORDER BY created_at, rowid"),
                )
            }),
        }
    }

    /// Atomically records the occurrence and advances the schedule. A
    /// duplicate due time is a no-op returning false, even after a restart.
    pub fn claim_scheduled_prompt(
        &self,
        p: &ScheduledPrompt,
        r: &ScheduledPromptRun,
    ) -> Result<bool> {
        let db = self.db.as_ref().ok_or_else(requires_db)?;
        db.with(|c| {
            let tx = c.transaction().bare()?;
            let n = tx
                .execute(
                    "INSERT INTO scheduled_prompt_run(id, schedule_id, due_at, status, detail) VALUES(?,?,?,?,?)
		ON CONFLICT(schedule_id, due_at) DO NOTHING",
                    params![r.id, p.id, iso_from_millis(r.due_at), r.status, r.detail],
                )
                .bare()?;
            if n == 0 {
                return Ok(false);
            }
            upsert_schedule(&tx, p)?;
            tx.commit().bare()?;
            Ok(true)
        })
    }

    /// Associates an occurrence with its workspace.
    pub fn link_scheduled_prompt_run(&self, id: &str, workspace_id: &str) -> Result<()> {
        let db = self.db.as_ref().ok_or_else(requires_db)?;
        db.with(|c| {
            c.execute(
                "UPDATE scheduled_prompt_run SET workspace_id=? WHERE id=?",
                [workspace_id, id],
            )
            .bare()?;
            Ok(())
        })
    }

    /// Records completion or failure without rewriting the workspace link,
    /// so a deleted workspace's SET NULL is preserved.
    pub fn update_scheduled_prompt_run(&self, r: &ScheduledPromptRun) -> Result<()> {
        let db = self.db.as_ref().ok_or_else(requires_db)?;
        db.with(|c| {
            c.execute(
                "UPDATE scheduled_prompt_run SET status=?, detail=? WHERE id=?",
                [&r.status, &r.detail, &r.id],
            )
            .bare()?;
            Ok(())
        })
    }
}
