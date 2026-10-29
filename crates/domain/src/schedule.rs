//! Scheduled prompts: saved local times that start an independent workspace.

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::cron;
use crate::settings::Flags;

/// Starts an independent workspace at a saved local time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScheduledPrompt {
    #[serde(skip_serializing_if = "Flags::is_empty")]
    pub flags: Flags,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simplify: Option<bool>,
    pub cron: String,
    pub loop_id: String,
    pub policy_id: String,
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub prompt: String,
    pub base_branch: String,
    pub mode: String,
    pub model: String,
    /// once | daily | weekdays | cron.
    pub cadence: String,
    /// YYYY-MM-DD for once.
    pub date: String,
    /// HH:MM for once/daily/weekdays.
    pub time: String,
    /// An IANA zone name.
    pub time_zone: String,
    pub paused: bool,
    pub next_run_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ScheduledPromptRun>,
}

/// A durable occurrence, claimed before any side effects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScheduledPromptRun {
    pub id: String,
    pub schedule_id: String,
    pub due_at: i64,
    pub workspace_id: String,
    pub status: String,
    pub detail: String,
}

/// Why a schedule has no computable next time.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("choose a valid IANA time zone")]
    TimeZone,
    #[error("invalid cron expression: {0}")]
    Cron(String),
    #[error("cron expression has no upcoming occurrence")]
    NoOccurrence,
    #[error("time must be HH:MM")]
    Time,
    #[error("choose a valid date")]
    Date,
    #[error("choose a one-time run or cron schedule")]
    Cadence,
}

impl ScheduledPrompt {
    /// Runs when Recur starts: the desktop equivalent of @reboot.
    pub fn on_startup(&self) -> bool {
        self.cadence == "cron" && self.cron.trim() == "@reboot"
    }
}

/// The first occurrence strictly after `after`, in the schedule's zone.
/// `None` for an exhausted one-time schedule, a one-time wall time that does
/// not exist (DST gap), or @reboot. Cron uses robfig/cron's calendar and DST
/// semantics; once/daily/weekdays fire at the first matching wall minute of
/// the day (never twice on a fall-back day).
pub fn next_scheduled_time(
    s: &ScheduledPrompt,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Tz>>, ScheduleError> {
    let loc: Tz = s.time_zone.parse().map_err(|_| ScheduleError::TimeZone)?;
    if s.on_startup() {
        return Ok(None);
    }
    if s.cadence == "cron" {
        let schedule = cron::parse(s.cron.trim()).map_err(ScheduleError::Cron)?;
        return schedule
            .next(loc, after.timestamp())
            .and_then(|unix| DateTime::from_timestamp(unix, 0))
            .map(|t| Some(t.with_timezone(&loc)))
            .ok_or(ScheduleError::NoOccurrence);
    }
    let (hour, minute) = parse_clock(&s.time).ok_or(ScheduleError::Time)?;
    let mut day = match s.cadence.as_str() {
        "once" => parse_date(&s.date).ok_or(ScheduleError::Date)?,
        "daily" | "weekdays" => after.with_timezone(&loc).date_naive(),
        _ => return Err(ScheduleError::Cadence),
    };
    for _ in 0..8 {
        let weekend = matches!(day.weekday(), Weekday::Sat | Weekday::Sun);
        if s.cadence != "weekdays" || !weekend {
            // The first occurrence only, so a fall-back day never runs twice.
            let first = day
                .and_hms_opt(hour, minute, 0)
                .and_then(|wall| loc.from_local_datetime(&wall).earliest());
            // Candidates are whole seconds, so "strictly after" ignores sub-seconds.
            if let Some(t) = first.filter(|t| t.timestamp() > after.timestamp()) {
                return Ok(Some(t));
            }
        }
        if s.cadence == "once" {
            break;
        }
        let Some(next) = day.succ_opt() else { break };
        day = next;
    }
    Ok(None)
}

/// A clock time: a one- or two-digit hour below 24, a colon,
/// and exactly two minute digits below 60.
fn parse_clock(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let digits = |x: &str, lens: std::ops::RangeInclusive<usize>| {
        lens.contains(&x.len()) && x.bytes().all(|b| b.is_ascii_digit())
    };
    if !digits(h, 1..=2) || !digits(m, 2..=2) {
        return None;
    }
    let (hour, minute): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// A `YYYY-MM-DD` date: a zero-padded date that exists.
fn parse_date(s: &str) -> Option<NaiveDate> {
    if s.len() != 10 {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rfc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn fmt(t: Option<DateTime<Tz>>) -> String {
        t.map(|t| {
            t.with_timezone(&Utc)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
        .unwrap_or_default()
    }

    fn prompt(cadence: &str, date: &str, time: &str, zone: &str) -> ScheduledPrompt {
        ScheduledPrompt {
            cadence: cadence.into(),
            date: date.into(),
            time: time.into(),
            time_zone: zone.into(),
            ..Default::default()
        }
    }

    #[test]
    fn next_scheduled_time_cases() {
        for (name, cadence, date, clock, zone, after, want) in [
            (
                "daily",
                "daily",
                "",
                "09:00",
                "America/New_York",
                "2026-09-08T12:00:00Z",
                "2026-09-08T13:00:00Z",
            ),
            (
                "strictly after",
                "daily",
                "",
                "09:00",
                "America/New_York",
                "2026-09-08T13:00:00Z",
                "2026-09-09T13:00:00Z",
            ),
            (
                "weekend",
                "weekdays",
                "",
                "09:00",
                "America/New_York",
                "2026-09-11T14:00:00Z",
                "2026-09-14T13:00:00Z",
            ),
            (
                "midnight gap",
                "daily",
                "",
                "00:00",
                "America/Havana",
                "2026-03-08T04:00:00Z",
                "2026-03-09T04:00:00Z",
            ),
            (
                "before midnight gap",
                "daily",
                "",
                "23:30",
                "America/Havana",
                "2026-03-08T04:00:00Z",
                "2026-03-08T04:30:00Z",
            ),
            (
                "spring gap",
                "daily",
                "",
                "02:30",
                "America/New_York",
                "2026-03-08T05:00:00Z",
                "2026-03-09T06:30:00Z",
            ),
            (
                "fall first",
                "daily",
                "",
                "01:30",
                "America/New_York",
                "2026-11-01T04:00:00Z",
                "2026-11-01T05:30:00Z",
            ),
            (
                "fall never twice",
                "daily",
                "",
                "01:30",
                "America/New_York",
                "2026-11-01T05:30:00Z",
                "2026-11-02T06:30:00Z",
            ),
            (
                "half hour transition",
                "daily",
                "",
                "01:45",
                "Australia/Lord_Howe",
                "2026-04-04T14:45:00Z",
                "2026-04-05T15:15:00Z",
            ),
            (
                "once",
                "once",
                "2026-09-09",
                "09:00",
                "UTC",
                "2026-09-08T13:00:00Z",
                "2026-09-09T09:00:00Z",
            ),
            (
                "once exhausted",
                "once",
                "2026-09-08",
                "09:00",
                "UTC",
                "2026-09-08T13:00:00Z",
                "",
            ),
            (
                "once nonexistent",
                "once",
                "2026-03-08",
                "02:30",
                "America/New_York",
                "2026-03-07T00:00:00Z",
                "",
            ),
            (
                "one-digit hour",
                "daily",
                "",
                "9:00",
                "UTC",
                "2026-09-08T08:00:00Z",
                "2026-09-08T09:00:00Z",
            ),
            (
                "sub-second after",
                "daily",
                "",
                "09:00",
                "UTC",
                "2026-09-08T08:59:59.5Z",
                "2026-09-08T09:00:00Z",
            ),
        ] {
            let got = next_scheduled_time(&prompt(cadence, date, clock, zone), rfc(after))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(fmt(got), want, "{name}");
        }
    }

    #[test]
    fn rejects_invalid_input() {
        let now = Utc::now();
        for (p, want) in [
            (
                prompt("daily", "", "09:00", "Local"),
                ScheduleError::TimeZone,
            ),
            (prompt("daily", "", "09:00", ""), ScheduleError::TimeZone),
            (
                prompt("daily", "", "09:00", "invalid/zone"),
                ScheduleError::TimeZone,
            ),
            (prompt("daily", "", "24:00", "UTC"), ScheduleError::Time),
            (prompt("daily", "", "09:0", "UTC"), ScheduleError::Time),
            (prompt("daily", "", "009:00", "UTC"), ScheduleError::Time),
            (prompt("daily", "", "09:60", "UTC"), ScheduleError::Time),
            (
                prompt("cron", "", "09:00", "UTC"),
                ScheduleError::Cron("empty spec string".into()),
            ),
            (
                prompt("once", "2026-02-30", "09:00", "UTC"),
                ScheduleError::Date,
            ),
            (
                prompt("once", "2026-2-03", "09:00", "UTC"),
                ScheduleError::Date,
            ),
            (prompt("hourly", "", "09:00", "UTC"), ScheduleError::Cadence),
            (prompt("hourly", "", "", "UTC"), ScheduleError::Time),
        ] {
            assert_eq!(next_scheduled_time(&p, now), Err(want), "{p:?}");
        }
        assert_eq!(
            parse_date("2028-02-29"),
            NaiveDate::from_ymd_opt(2028, 2, 29)
        );
        assert_eq!(parse_date("2100-02-29"), None);
    }

    #[test]
    fn cron_scheduled_time() {
        for (expression, zone, after, want) in [
            (
                "*/15 9-17 * * MON-FRI",
                "UTC",
                "2026-09-11T17:45:00Z",
                "2026-09-14T09:00:00Z",
            ),
            (
                "0 9,17 * * *",
                "UTC",
                "2026-09-08T09:00:00Z",
                "2026-09-08T17:00:00Z",
            ),
            (
                "0 9 1,15 * *",
                "UTC",
                "2026-09-08T09:00:00Z",
                "2026-09-15T09:00:00Z",
            ),
            (
                "0 9 * JAN,MAR MON",
                "UTC",
                "2026-09-08T09:00:00Z",
                "2027-01-04T09:00:00Z",
            ),
            (
                "0 9 15 * MON",
                "UTC",
                "2026-09-08T09:00:00Z",
                "2026-09-14T09:00:00Z",
            ),
            (
                "0 0 29 FEB *",
                "UTC",
                "2026-09-08T09:00:00Z",
                "2028-02-29T00:00:00Z",
            ),
            (
                "*/10 * * * * *",
                "UTC",
                "2026-09-08T09:00:01Z",
                "2026-09-08T09:00:10Z",
            ),
            (
                "@monthly",
                "UTC",
                "2026-09-08T09:00:00Z",
                "2026-10-01T00:00:00Z",
            ),
            (
                "@every 2h30m",
                "UTC",
                "2026-09-08T09:00:00Z",
                "2026-09-08T11:30:00Z",
            ),
            (
                "0 9 * * *",
                "America/New_York",
                "2026-09-08T09:00:00Z",
                "2026-09-08T13:00:00Z",
            ),
            (
                "CRON_TZ=Asia/Tokyo 0 9 * * *",
                "UTC",
                "2026-09-08T09:00:00Z",
                "2026-09-09T00:00:00Z",
            ),
            (
                "30 2 * * *",
                "America/New_York",
                "2026-03-08T05:00:00Z",
                "2026-03-08T07:00:00Z",
            ),
            (
                "30 1 * * *",
                "America/New_York",
                "2026-11-01T05:30:00Z",
                "2026-11-02T06:30:00Z",
            ),
            (
                "@every 1m",
                "UTC",
                "2026-09-08T09:00:00.7Z",
                "2026-09-08T09:01:00Z",
            ),
        ] {
            let p = ScheduledPrompt {
                cadence: "cron".into(),
                cron: expression.into(),
                time_zone: zone.into(),
                ..Default::default()
            };
            let got =
                next_scheduled_time(&p, rfc(after)).unwrap_or_else(|e| panic!("{expression}: {e}"));
            assert_eq!(fmt(got), want, "{expression} after {after}");
        }
        for expression in [
            "bad cron",
            "60 * * * *",
            "*/0 * * * *",
            "0 0 30 FEB *",
            "@every nonsense",
        ] {
            let p = ScheduledPrompt {
                cadence: "cron".into(),
                cron: expression.into(),
                time_zone: "UTC".into(),
                ..Default::default()
            };
            assert!(
                next_scheduled_time(&p, Utc::now()).is_err(),
                "accepted {expression:?}"
            );
        }
        let reboot = ScheduledPrompt {
            cadence: "cron".into(),
            cron: " @reboot ".into(),
            time_zone: "UTC".into(),
            ..Default::default()
        };
        assert!(reboot.on_startup());
        assert_eq!(next_scheduled_time(&reboot, Utc::now()), Ok(None));
    }

    #[test]
    fn result_is_in_schedule_zone() {
        let p = prompt("daily", "", "09:00", "America/New_York");
        let got = next_scheduled_time(&p, rfc("2026-09-08T12:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(got.timezone(), Tz::America__New_York);
        assert_eq!(got.to_rfc3339(), "2026-09-08T09:00:00-04:00");
    }
}
