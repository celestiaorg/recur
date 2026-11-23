//! The Insights report over `[start, end)`, read on its own read-only
//! connection so it never holds the store's connection while the app writes.
//!
//! Agent time is turn intervals (`completed_at - duration_ms` to
//! `completed_at` on the message that ends a turn) plus time an ask waited on
//! the human. A turn parked by ask_user or switch_mode records no end, so it
//! runs from its prompt (or the session's previous park) to the parking call. A task is a prompt (a user message with no provider and no
//! tool_result: provider carriers carry one, and startup recovery's carrier
//! closes nothing), ended by the next turn end in its session.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Datelike as _, Duration, Local, NaiveDate, Offset as _, TimeZone as _};
use chrono::{Timelike as _, Utc};
use recur_domain::insights::*;
use recur_domain::{STREAM_STREAMING, WorkspaceState};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};

use crate::error::{Result, SqlContext as _};
use crate::store::Store;
use crate::tool_calls::{backfill_progress, is_ask};

const H: i64 = 3_600_000;
const HEATMAP_DAYS: i64 = 53 * 7;
/// Fewer tasks than this make a typical time noise.
const MIN_SAMPLE: usize = 10;
/// Turn-end rows are found by created_at, which precedes completed_at by at
/// most the last message's streaming time.
const END_SLACK: i64 = 24 * H;
/// An ask whose call stayed open longer than this blocked inside the turn;
/// otherwise the answer arrives as the session's next prompt.
const BLOCKING_ASK_MS: i64 = 1000;
/// A wait longer than this was not answered: the next prompt came on another
/// day, about something else.
const MAX_WAIT_MS: i64 = 16 * H;

/// What the report needs from the in-memory working set.
pub(crate) struct WsInfo {
    project_id: String,
    parent_id: String,
    schedule_id: String,
    state: WorkspaceState,
    add: i64,
    del: i64,
}

pub(crate) struct PrInfo {
    add: i64,
    del: i64,
    files: i64,
    open: bool,
}

impl Store {
    /// The Insights report for `[start_ms, end_ms)`, clipped at now, compared with as long a
    /// stretch from `prev_start_ms` (the previous period's start), with the 53 weeks ending at
    /// the end. A store with no database reports nothing.
    pub fn insights(
        &self,
        prev_start_ms: i64,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<InsightsReport> {
        let Some(db) = &self.db else {
            return Ok(InsightsReport::default());
        };
        let c = Connection::open_with_flags(
            db.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ctx(|| "open insights connection".into())?;
        c.busy_timeout(std::time::Duration::from_secs(5))
            .ctx(|| "set busy_timeout".into())?;
        if let Some(p) = backfill_progress(&c)? {
            return Ok(InsightsReport {
                preparing: Some(p),
                ..Default::default()
            });
        }
        let (ws, prs) = {
            let s = self.state.read();
            let ws = s
                .workspaces
                .iter()
                .map(|w| {
                    let info = WsInfo {
                        project_id: w.project_id.clone(),
                        parent_id: w.parent_id.clone(),
                        schedule_id: w.schedule_id.clone(),
                        state: w.state.clone(),
                        add: w.diff.add,
                        del: w.diff.del,
                    };
                    (w.id.clone(), info)
                })
                .collect();
            let prs = s
                .prs
                .iter()
                .map(|p| {
                    let info = PrInfo {
                        add: p.diff.add,
                        del: p.diff.del,
                        files: p.file_count,
                        open: p.state != "Merged" && p.state != "Closed",
                    };
                    (p.id.clone(), info)
                })
                .collect();
            (ws, prs)
        };
        report(
            &c,
            &ws,
            &prs,
            prev_start_ms,
            start_ms,
            end_ms.min(Utc::now().timestamp_millis()),
        )
    }
}

#[derive(Default)]
struct Sess {
    workspace_id: String,
    kind: String,
    /// A specialist (coder, reviewer) a chat or loop drives.
    owned: bool,
    streaming: bool,
}

/// A message that ends a turn: stamped with a duration, or an error.
struct End {
    id: String,
    session: String,
    model: String,
    provider: String,
    error: bool,
    created: i64,
    completed: i64,
    duration: i64,
    input: i64,
    cached: i64,
    created_cache: i64,
    window: i64,
    /// Tokens billed for the turn; `None` on rows from before migration 0020
    /// and from providers that report no turn total.
    total: Option<TokenStats>,
}

impl End {
    fn interval(&self) -> Option<(i64, i64)> {
        (self.duration > 0 && self.completed > 0)
            .then(|| (self.completed - self.duration, self.completed))
    }

    /// Startup recovery marks a turn cut off by a quit as an error at relaunch,
    /// with no duration.
    fn recovered(&self) -> bool {
        self.error && self.duration == 0 && self.completed > 0
    }

    fn context(&self) -> i64 {
        self.input + self.cached + self.created_cache
    }
}

/// Agent time attributed to a session and model; `end` is `None` for a parked turn.
struct Turn<'a> {
    session: &'a str,
    model: &'a str,
    provider: &'a str,
    end: Option<&'a End>,
    a: i64,
    b: i64,
}

struct Call {
    session: String,
    label: String,
    started: i64,
    ended: Option<i64>,
    is_error: bool,
    no_match: bool,
    build_lock: bool,
    model: String,
    provider: String,
}

impl Call {
    fn ms(&self) -> Option<i64> {
        self.ended.map(|e| (e - self.started).max(0))
    }
}

fn overlap(a: i64, b: i64, lo: i64, hi: i64) -> i64 {
    (b.min(hi) - a.max(lo)).max(0)
}

/// Splits `[a, b)` at local hour boundaries.
fn each_hour(a: i64, b: i64, mut f: impl FnMut(DateTime<Local>, i64)) {
    let mut t = a;
    while t < b {
        let Some(lt) = Local.timestamp_millis_opt(t).single() else {
            return;
        };
        let off = i64::from(lt.offset().fix().local_minus_utc()) * 1000;
        let next = (((t + off).div_euclid(H) + 1) * H - off).min(b);
        f(lt, next - t);
        t = next;
    }
}

fn local_date(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map_or_else(|| Utc::now().date_naive(), |t| t.date_naive())
}

fn local_midnight(d: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&d.and_time(chrono::NaiveTime::MIN))
        .earliest()
        .map_or(0, |t| t.timestamp_millis())
}

fn median(mut v: Vec<i64>) -> Option<i64> {
    v.sort_unstable();
    v.get(v.len() / 2).copied()
}

/// `claude-opus-5-5` → `opus 5.5`, `gpt-6.1-sol` → `gpt 6.1-sol`.
fn model_name(id: &str) -> String {
    let s = id
        .strip_prefix("claude-")
        .unwrap_or(id)
        .replacen('-', " ", 1);
    let b = s.as_bytes();
    s.char_indices()
        .map(|(i, ch)| {
            let digits =
                i > 0 && b[i - 1].is_ascii_digit() && b.get(i + 1).is_some_and(u8::is_ascii_digit);
            if ch == '-' && digits { '.' } else { ch }
        })
        .collect()
}

/// `None` for a stop or an interruption, else the failure reason.
fn failure(text: &str) -> Option<&'static str> {
    if text.starts_with("Stopped by user") || text.starts_with("Interrupted") {
        return None;
    }
    let l = text.to_lowercase();
    Some(if l.contains("time limit") {
        "time limit"
    } else if [
        "spend limit",
        "usage limit",
        "rate limit",
        "plan limit",
        "session limit",
    ]
    .iter()
    .any(|k| l.contains(k))
    {
        "plan limit"
    } else {
        "provider error"
    })
}

fn rows<T>(
    c: &Connection,
    sql: &str,
    p: impl rusqlite::Params,
    f: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = c.prepare(sql).ctx(|| format!("prepare {sql}"))?;
    let out = stmt
        .query_map(p, f)
        .ctx(|| format!("query {sql}"))?
        .collect::<rusqlite::Result<Vec<T>>>()
        .ctx(|| format!("scan {sql}"))?;
    Ok(out)
}

fn report(
    c: &Connection,
    ws: &HashMap<String, WsInfo>,
    prs: &HashMap<String, PrInfo>,
    prev_start: i64,
    start: i64,
    end: i64,
) -> Result<InsightsReport> {
    let mut r = InsightsReport::default();
    if end <= start {
        return Ok(r);
    }
    let prev_end = (prev_start + (end - start)).min(start);
    let last_day = local_date(end - 1);
    let days: Vec<NaiveDate> = (0..HEATMAP_DAYS)
        .rev()
        .map(|back| last_day - Duration::days(back))
        .collect();
    let heat_start = local_midnight(days[0]);
    let from = prev_start.min(heat_start);

    let sessions: HashMap<String, Sess> = rows(
        c,
        "SELECT id, COALESCE(workspace_id, ''), kind,
			COALESCE(json_extract(config_json, '$.ownerCode'), '') != '', status = ?
		FROM session",
        [STREAM_STREAMING],
        |row| {
            Ok((
                row.get(0)?,
                Sess {
                    workspace_id: row.get(1)?,
                    kind: row.get(2)?,
                    owned: row.get(3)?,
                    streaming: row.get(4)?,
                },
            ))
        },
    )?
    .into_iter()
    .collect();
    let no_sess = Sess::default();
    let sess = |id: &str| sessions.get(id).unwrap_or(&no_sess);

    let ends = rows(
        c,
        "SELECT id, origin_session_id, COALESCE(model_id, ''), COALESCE(provider, ''),
			stream_status = 'error', created_at, COALESCE(completed_at, 0),
			COALESCE(duration_ms, 0), COALESCE(input_tokens, 0), COALESCE(cache_read_tokens, 0),
			COALESCE(cache_creation_tokens, 0),
			COALESCE(context_window, 0),
			COALESCE(total_input_tokens, total_output_tokens, total_cache_creation_tokens,
				total_cache_read_tokens) IS NOT NULL,
			COALESCE(total_input_tokens, 0) + COALESCE(total_cache_read_tokens, 0)
				+ COALESCE(total_cache_creation_tokens, 0),
			COALESCE(total_cache_read_tokens, 0), COALESCE(total_output_tokens, 0)
		FROM message
		WHERE (duration_ms IS NOT NULL OR stream_status = 'error') AND created_at >= ?",
        [from - END_SLACK],
        |row| {
            Ok(End {
                id: row.get(0)?,
                session: row.get(1)?,
                model: row.get(2)?,
                provider: row.get(3)?,
                error: row.get(4)?,
                created: row.get(5)?,
                completed: row.get(6)?,
                duration: row.get(7)?,
                input: row.get(8)?,
                cached: row.get(9)?,
                created_cache: row.get(10)?,
                window: row.get(11)?,
                total: if row.get(12)? {
                    Some(TokenStats {
                        input: row.get(13)?,
                        cached: row.get(14)?,
                        output: row.get(15)?,
                    })
                } else {
                    None
                },
            })
        },
    )?;
    // Every prompt ever: a few per turn, so small, and the first per session
    // decides whether a schedule or a parent sent it.
    let prompts: Vec<(String, String, i64)> = rows(
        c,
        "SELECT id, origin_session_id, created_at FROM message
		WHERE role = 'user' AND provider IS NULL AND created_at >= 0
			AND NOT EXISTS (SELECT 1 FROM message_content
				WHERE message_id = message.id AND type = 'tool_result')
		ORDER BY created_at",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let mut prompts_by_session: HashMap<&str, Vec<i64>> = HashMap::new();
    for (_, s, t) in &prompts {
        prompts_by_session.entry(s).or_default().push(*t);
    }
    let next_prompt = |session: &str, after: i64| {
        let v = prompts_by_session.get(session)?;
        v.get(v.partition_point(|&t| t <= after)).copied()
    };

    let call_sql = "SELECT t.session_id, t.label, t.started_at, t.ended_at, t.is_error,
			t.no_match, t.build_lock, COALESCE(m.model_id, ''), COALESCE(m.provider, '')
		FROM tool_call t JOIN message m ON m.id = t.message_id
		WHERE t.started_at >= ? AND t.started_at < ?";
    let scan_call = |row: &rusqlite::Row<'_>| {
        Ok(Call {
            session: row.get(0)?,
            label: row.get(1)?,
            started: row.get(2)?,
            ended: row.get(3)?,
            is_error: row.get(4)?,
            no_match: row.get(5)?,
            build_lock: row.get(6)?,
            model: row.get(7)?,
            provider: row.get(8)?,
        })
    };
    let calls = rows(c, call_sql, params![start, end], scan_call)?;
    let mut parks = rows(
        c,
        &format!(
            "{call_sql} AND (t.label = 'AskUserQuestion' OR t.label LIKE '%\\_\\_ask\\_user' ESCAPE '\\'
				OR t.label LIKE '%\\_\\_switch\\_mode' ESCAPE '\\')"
        ),
        params![from, end],
        scan_call,
    )?;
    parks.sort_by_key(|k| k.started);
    let asks: Vec<&Call> = parks.iter().filter(|k| is_ask(&k.label)).collect();
    // (ask, wait) with the wait unknown while it is still open or unanswered.
    let waits: Vec<(&Call, Option<i64>)> = asks
        .iter()
        .map(|&a| {
            let wait = match a.ms() {
                Some(ms) if ms > BLOCKING_ASK_MS => Some(ms),
                _ => next_prompt(&a.session, a.started).map(|t| t - a.started),
            };
            (a, wait.filter(|&w| w <= MAX_WAIT_MS))
        })
        .collect();

    let mut ends_by_session: HashMap<&str, Vec<(i64, &End)>> = HashMap::new();
    for e in &ends {
        ends_by_session
            .entry(&e.session)
            .or_default()
            .push((e.created, e));
    }
    for v in ends_by_session.values_mut() {
        v.sort_unstable_by_key(|(t, _)| *t);
    }

    // Agent time: turns plus answered waits.
    let mut turns: Vec<Turn<'_>> = ends
        .iter()
        .filter_map(|e| {
            let (a, b) = e.interval()?;
            Some(Turn {
                session: &e.session,
                model: &e.model,
                provider: &e.provider,
                end: Some(e),
                a,
                b,
            })
        })
        .collect();
    // A parked turn: no end since its prompt or the previous park, and no
    // end's interval covers the call (a blocking ask's turn went on).
    let mut last_park: HashMap<&str, i64> = HashMap::new();
    for k in &parks {
        let t = k.started;
        let prior = last_park.insert(&k.session, t);
        let Some(p) = prompts_by_session.get(k.session.as_str()).and_then(|v| {
            let i = v.partition_point(|&pt| pt <= t);
            i.checked_sub(1).map(|i| v[i])
        }) else {
            continue;
        };
        let a = prior.map_or(p, |q| q.max(p));
        let session_ends = ends_by_session
            .get(k.session.as_str())
            .map_or(&[][..], Vec::as_slice);
        let ended = session_ends.iter().any(|&(created, e)| {
            (created > a && created <= t) || e.interval().is_some_and(|(ia, ib)| ia <= t && t < ib)
        });
        if !ended && a < t {
            turns.push(Turn {
                session: &k.session,
                model: &k.model,
                provider: &k.provider,
                end: None,
                a,
                b: t,
            });
        }
    }
    let intervals = turns.iter().map(|u| (u.a, u.b)).chain(
        waits
            .iter()
            .filter_map(|(a, w)| w.map(|w| (a.started, a.started + w))),
    );
    let mut by_day: HashMap<NaiveDate, i64> = HashMap::new();
    let mut by_weekday = [0i64; 7];
    for (a, b) in intervals {
        each_hour(a.max(start), b.min(end), |lt, ms| {
            r.agent_ms += ms;
            r.by_hour[lt.hour() as usize] += ms;
            by_weekday[lt.weekday().num_days_from_monday() as usize] += ms;
        });
        r.prev_agent_ms += overlap(a, b, prev_start, prev_end);
        each_hour(a.max(heat_start), b.min(end), |lt, ms| {
            *by_day.entry(lt.date_naive()).or_default() += ms;
        });
    }
    r.days = days
        .iter()
        .map(|d| DayHours {
            date: d.format("%Y-%m-%d").to_string(),
            agent_ms: by_day.get(d).copied().unwrap_or_default(),
        })
        .collect();
    let mut weekdays = [0i64; 7];
    let mut d = local_date(start);
    while d <= last_day {
        weekdays[d.weekday().num_days_from_monday() as usize] += 1;
        d += Duration::days(1);
    }
    for i in 0..7 {
        if weekdays[i] > 0 {
            r.by_weekday[i] = by_weekday[i] / weekdays[i];
        }
    }

    // Peak: the most turns overlapping, ends before starts at a tie.
    let mut edges: Vec<(i64, i64)> = turns
        .iter()
        .filter(|u| overlap(u.a, u.b, start, end) > 0)
        .flat_map(|u| [(u.a.max(start), 1), (u.b.min(end), -1)])
        .collect();
    edges.sort_unstable();
    let mut running = 0;
    for (t, d) in edges {
        running += d;
        if running > r.peak_agents {
            (r.peak_agents, r.peak_at_ms) = (running, t);
        }
    }

    // Split: commands are the union of each session's tool intervals.
    let turn_ms: i64 = turns.iter().map(|u| overlap(u.a, u.b, start, end)).sum();
    let mut spans: BTreeMap<&str, Vec<(i64, i64)>> = BTreeMap::new();
    for k in calls.iter().filter(|k| !is_ask(&k.label)) {
        if let Some(e) = k.ended {
            spans
                .entry(&k.session)
                .or_default()
                .push((k.started, e.min(end)));
        }
    }
    let mut commands_ms = 0;
    for v in spans.values_mut() {
        v.sort_unstable();
        let mut cur: Option<(i64, i64)> = None;
        for &(a, b) in v.iter() {
            cur = match cur {
                Some((ca, cb)) if a <= cb => Some((ca, cb.max(b))),
                Some((ca, cb)) => {
                    commands_ms += cb - ca;
                    Some((a, b))
                }
                None => Some((a, b)),
            };
        }
        if let Some((ca, cb)) = cur {
            commands_ms += (cb - ca).max(0);
        }
    }
    let waiting_ms: i64 = waits
        .iter()
        .filter_map(|(a, w)| w.map(|w| overlap(a.started, a.started + w, start, end)))
        .sum();
    r.split = TimeSplit {
        commands_ms: commands_ms.min(turn_ms),
        model_ms: turn_ms - commands_ms.min(turn_ms),
        waiting_ms,
    };

    // Asks.
    let range_waits: Vec<(&Call, Option<i64>)> = waits
        .iter()
        .filter(|(a, _)| a.started >= start)
        .copied()
        .collect();
    let answered: Vec<i64> = range_waits.iter().filter_map(|(_, w)| *w).collect();
    r.asks = AskStats {
        count: range_waits.len() as i64,
        typical_wait_ms: median(answered.clone()),
        longest_wait_ms: answered.iter().max().copied(),
    };

    // Tasks.
    let mut first_prompt: HashMap<&str, i64> = HashMap::new();
    for (_, s, t) in &prompts {
        first_prompt.entry(s).or_insert(*t);
    }
    let mut prompt_text = c
        .prepare(
            "SELECT COALESCE(json_extract(content_json, '$.text'), ''),
				COALESCE(json_extract(content_json, '$.display_text'), '')
			FROM message_content WHERE message_id = ? AND type = 'text' ORDER BY idx LIMIT 1",
        )
        .ctx(|| "prepare prompt text".into())?;
    let mut failed_by: BTreeMap<&str, i64> = BTreeMap::new();
    let mut projects = BTreeSet::new();
    let mut failed_turns: Vec<(i64, i64)> = Vec::new();
    let mut review_ws = BTreeSet::new();
    for (id, s, t) in prompts.iter().filter(|(_, _, t)| *t >= start && *t < end) {
        let se = sess(s);
        let w = ws.get(&se.workspace_id);
        r.tasks.total += 1;
        if let Some(w) = w.filter(|w| !w.project_id.is_empty()) {
            projects.insert(w.project_id.as_str());
        }
        let (text, display): (String, String) = prompt_text
            .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
            .ctx(|| format!("prompt text {id}"))?
            .unwrap_or_default();
        let sent_by_agent = display.starts_with('\u{2064}')
            || display.starts_with("From parent ")
            || text.starts_with("Subagent ");
        let first = first_prompt.get(s.as_str()) == Some(t);
        if se.owned || se.kind == "reviewer" || se.kind == "maestro" || sent_by_agent {
            r.tasks.from_agent += 1;
        } else if first && w.is_some_and(|w| !w.schedule_id.is_empty()) {
            r.tasks.from_schedule += 1;
        } else if first && w.is_some_and(|w| !w.parent_id.is_empty()) {
            r.tasks.from_agent += 1;
        } else {
            r.tasks.from_you += 1;
        }
        if se.kind == "reviewer" {
            r.handoffs.to_reviewer += 1;
            review_ws.insert(se.workspace_id.as_str());
        }
        // The turn ends at the session's next end, unless another prompt
        // came first (a queued or steering follow-up shares that end).
        let end_row = ends_by_session.get(s.as_str()).and_then(|v| {
            let e = v.get(v.partition_point(|(et, _)| et <= t))?.1;
            match next_prompt(s, *t) {
                Some(np) if np < e.created => None,
                _ => Some(e),
            }
        });
        match end_row {
            Some(e) if e.recovered() => r.tasks.stopped += 1,
            Some(e) if e.error => {
                let text: String = c
                    .query_row(
                        "SELECT COALESCE(json_extract(content_json, '$.text'), '')
						FROM message_content WHERE message_id = ? AND type = 'text'
						ORDER BY idx LIMIT 1",
                        [&e.id],
                        |row| row.get(0),
                    )
                    .optional()
                    .ctx(|| format!("error text {}", e.id))?
                    .unwrap_or_default();
                match failure(&text) {
                    None => r.tasks.stopped += 1,
                    Some(reason) => {
                        r.tasks.failed += 1;
                        *failed_by.entry(reason).or_default() += 1;
                        if reason == "provider error" {
                            failed_turns.push((*t, e.completed.max(e.created)));
                        }
                    }
                }
            }
            None if se.streaming && next_prompt(s, *t).is_none() => r.tasks.running += 1,
            _ => r.tasks.finished += 1,
        }
    }
    r.tasks.projects = projects.len() as i64;
    r.tasks.failed_by = failed_by.into_iter().map(|(k, v)| (k.into(), v)).collect();
    r.tasks.failed_by.sort_by_key(|f| Reverse(f.1));

    // Commands.
    let mut by_label: BTreeMap<&str, Vec<&Call>> = BTreeMap::new();
    for k in calls.iter().filter(|k| !is_ask(&k.label)) {
        by_label.entry(&k.label).or_default().push(k);
    }
    r.commands = by_label
        .into_iter()
        .map(|(name, v)| {
            let ms: Vec<i64> = v.iter().filter_map(|k| k.ms()).collect();
            CommandStats {
                name: name.into(),
                total_ms: ms.iter().sum(),
                calls: v.len() as i64,
                p50_ms: median(ms.clone()).unwrap_or_default(),
                max_ms: ms.iter().max().copied().unwrap_or_default(),
                failed: v.iter().filter(|k| k.is_error).count() as i64,
                no_match: v.iter().filter(|k| k.no_match).count() as i64,
            }
        })
        .collect();
    r.commands.sort_by_key(|k| Reverse(k.total_ms));

    // Time lost.
    let mut lost = Vec::new();
    let lock: Vec<i64> = calls
        .iter()
        .filter(|k| k.build_lock)
        .map(|k| k.ms().unwrap_or_default())
        .collect();
    lost.push((LostKind::BuildLock, lock.iter().sum(), lock.len() as i64));
    // A label failing 3+ times in a row in a session: the re-runs are lost.
    let mut streaks: HashMap<(&str, &str), Vec<&Call>> = HashMap::new();
    let (mut rerun_ms, mut reruns) = (0, 0);
    let mut ordered: Vec<&Call> = calls.iter().filter(|k| !is_ask(&k.label)).collect();
    ordered.sort_by_key(|k| k.started);
    let mut close = |v: &mut Vec<&Call>| {
        if v.len() >= 3 {
            reruns += 1;
            rerun_ms += v[1..]
                .iter()
                .map(|k| k.ms().unwrap_or_default())
                .sum::<i64>();
        }
        v.clear();
    };
    for k in ordered {
        let v = streaks.entry((&k.session, &k.label)).or_default();
        if k.is_error {
            v.push(k);
        } else {
            close(v);
        }
    }
    for v in streaks.values_mut() {
        close(v);
    }
    lost.push((LostKind::RepeatedFailure, rerun_ms, reruns));
    let overnight: Vec<i64> = range_waits
        .iter()
        .filter_map(|(a, w)| {
            let h = Local.timestamp_millis_opt(a.started).single()?.hour();
            w.filter(|&w| w > H && !(6..23).contains(&h))
        })
        .collect();
    lost.push((
        LostKind::OvernightAsk,
        overnight.iter().sum(),
        overnight.len() as i64,
    ));
    lost.push((
        LostKind::ProviderError,
        failed_turns.iter().map(|&(a, b)| (b - a).max(0)).sum(),
        failed_turns.len() as i64,
    ));
    let unsettled: BTreeSet<String> = rows(
        c,
        "SELECT workspace_id FROM events
		WHERE type = 'escalation.raised' AND ts >= ? AND ts < ?
			AND json_extract(payload_json, '$.escalation.question') LIKE '%without converging%'",
        params![start, end],
        |row| row.get(0),
    )?
    .into_iter()
    .collect();
    let unsettled_ms = turns
        .iter()
        .filter(|u| {
            let se = sess(u.session);
            (se.owned || se.kind == "reviewer") && unsettled.contains(&se.workspace_id)
        })
        .map(|u| overlap(u.a, u.b, start, end))
        .sum();
    lost.push((
        LostKind::ReviewNotSettling,
        unsettled_ms,
        unsettled.len() as i64,
    ));
    r.lost = lost
        .into_iter()
        .filter(|&(_, _, count)| count > 0)
        .map(|(kind, ms, count)| TimeLost { kind, ms, count })
        .collect();
    r.lost.sort_by_key(|l| Reverse(l.ms));

    // Models, context and tokens: turns ending in range.
    let in_range: Vec<&End> = ends
        .iter()
        .filter(|e| e.completed >= start && e.completed < end)
        .collect();
    #[derive(Default)]
    struct M {
        ms: i64,
        durations: Vec<i64>,
        asks: i64,
        commands: i64,
        failed: i64,
        tokens: Option<i64>,
    }
    let mut models: BTreeMap<(&str, &str), M> = BTreeMap::new();
    for u in &turns {
        let m = models.entry((u.model, u.provider)).or_default();
        m.ms += overlap(u.a, u.b, start, end);
        if let Some(e) = u.end.filter(|e| e.completed >= start && e.completed < end) {
            m.durations.push(e.duration);
            if let Some(t) = &e.total {
                *m.tokens.get_or_insert(0) += t.input;
            }
        }
    }
    for (a, _) in &range_waits {
        if let Some(m) = models.get_mut(&(a.model.as_str(), a.provider.as_str())) {
            m.asks += 1;
        }
    }
    for k in calls.iter().filter(|k| !is_ask(&k.label)) {
        if let Some(m) = models.get_mut(&(k.model.as_str(), k.provider.as_str())) {
            m.commands += 1;
            m.failed += i64::from(k.is_error);
        }
    }
    r.models = models
        .into_iter()
        .filter(|((model, _), m)| !model.is_empty() && (m.ms > 0 || !m.durations.is_empty()))
        .map(|((model, provider), m)| {
            let enough = m.durations.len() >= MIN_SAMPLE;
            ModelStats {
                model: model_name(model),
                provider: provider.into(),
                agent_ms: m.ms,
                runs: m.durations.len() as i64,
                typical_task_ms: if enough { median(m.durations) } else { None },
                ms_per_ask: (enough && m.asks > 0).then(|| m.ms / m.asks),
                commands: m.commands,
                failed_commands: m.failed,
                tokens_in: m.tokens,
            }
        })
        .collect();
    r.models.sort_by_key(|m| Reverse(m.agent_ms));

    for e in &in_range {
        let used = e.context();
        r.context.largest = r.context.largest.max(used);
        if e.window > 0 && used * 10 > e.window * 9 {
            r.context.turns_over_90 += 1;
        }
        if let Some(t) = &e.total {
            r.tokens.input += t.input;
            r.tokens.cached += t.cached;
            r.tokens.output += t.output;
        }
    }
    r.context.compactions = c
        .query_row(
            "SELECT COUNT(*) FROM message_content
			WHERE type = 'compact' AND created_at >= ? AND created_at < ?",
            params![start, end],
            |row| row.get(0),
        )
        .ctx(|| "count compactions".into())?;

    // Handoffs: children whose first session started in range.
    let mut child_start: HashMap<&str, i64> = HashMap::new();
    let created: Vec<(String, i64)> = rows(
        c,
        "SELECT workspace_id, MIN(created_at) FROM session
		WHERE COALESCE(workspace_id, '') != '' GROUP BY workspace_id",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    for (id, t) in &created {
        if ws.get(id).is_some_and(|w| !w.parent_id.is_empty()) {
            child_start.insert(id, *t);
        }
    }
    for (id, t) in child_start {
        if t >= start && t < end {
            r.handoffs.to_child += 1;
            if ws.get(id).is_some_and(|w| {
                matches!(w.state, WorkspaceState::Failed | WorkspaceState::Stopped)
            }) {
                r.handoffs.unfinished += 1;
            }
        }
    }
    r.handoffs.review_loops = review_ws.len() as i64;
    r.handoffs.review_settled = review_ws
        .iter()
        .filter(|w| !unsettled.contains(**w))
        .count() as i64;

    // Merges and the changes they landed.
    let merges: Vec<(String, String, String)> = rows(
        c,
        "SELECT workspace_id, via, COALESCE(pr_id, '') FROM workspace_merge
		WHERE merged_at >= ? AND merged_at < ?",
        params![start, end],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    for (w, via, pr) in &merges {
        if via == "pr" {
            r.merged.by_pr += 1;
            if let Some(p) = prs.get(pr) {
                r.changes.added += p.add;
                r.changes.removed += p.del;
                r.changes.files += p.files;
            }
        } else {
            r.merged.direct += 1;
            if let Some(w) = ws.get(w) {
                r.changes.added += w.add;
                r.changes.removed += w.del;
            }
        }
    }
    r.merged.total = r.merged.by_pr + r.merged.direct;
    r.merged.open_prs = prs.values().filter(|p| p.open).count() as i64;
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_reasons() {
        assert_eq!(model_name("claude-opus-5-5"), "opus 5.5");
        assert_eq!(model_name("gpt-6.1-sol"), "gpt 6.1-sol");
        assert_eq!(model_name("grok-4.6"), "grok 4.6");
        assert_eq!(model_name("opus"), "opus");
        assert_eq!(failure("Stopped by user"), None);
        assert_eq!(
            failure("Agent error: You've hit your individual spend limit"),
            Some("plan limit")
        );
        assert_eq!(
            failure("Agent error: this turn hit Recur's time limit"),
            Some("time limit")
        );
        assert_eq!(
            failure("Agent error: error_during_execution"),
            Some("provider error")
        );
    }
}
