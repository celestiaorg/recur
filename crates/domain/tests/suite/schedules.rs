//! Next run times for stored schedules (cron cadences and once/daily/weekdays) in
//! `fixtures/schedules.json`: `[cadence, cron, date, time, zone, after, next]`, `next` empty
//! for none and `!` for an error. Zones come from the system time zone database, so a rule
//! change there (a country dropping or moving DST) changes `next` for that zone; update the
//! fixture by hand when that or the behavior changes on purpose. Keep only rows that agree
//! across the tzdata versions CI sees: Havana rows that land after Cuba's 2026 rule change
//! are left out because macOS (2026f) and Ubuntu's tzdata disagree there.
//!
//! At a DST transition a wall time a gap skips runs when the gap ends, a repeated wall time
//! runs once, and Lord Howe's half-hour fall-back skips no day or week.

use chrono::{DateTime, Utc};
use recur_domain::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Rows {
    schedules: Vec<[String; 7]>,
}

#[test]
fn next_run_times() {
    let rows: Rows = serde_json::from_str(include_str!("../fixtures/schedules.json")).unwrap();
    assert!(rows.schedules.len() > 2000);
    let mut mismatches = Vec::new();
    for [cadence, cron, date, time, zone, after, want] in &rows.schedules {
        let p = ScheduledPrompt {
            cadence: cadence.clone(),
            cron: cron.clone(),
            date: date.clone(),
            time: time.clone(),
            time_zone: zone.clone(),
            ..Default::default()
        };
        let after_t: DateTime<Utc> = DateTime::parse_from_rfc3339(after).unwrap().into();
        let got = match next_scheduled_time(&p, after_t) {
            Err(_) => "!".to_owned(),
            Ok(None) => String::new(),
            Ok(Some(t)) => t
                .with_timezone(&Utc)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
        };
        if &got != want {
            mismatches.push(format!(
                "{cadence} {cron:?} {date} {time} {zone} after {after}: got {got:?}, want {want:?}"
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
