//! Snapshot next run times for stored schedules (cron cadences and
//! once/daily/weekdays) in `fixtures/go_rules.json`. Edit the fixture by hand
//! when the behavior changes on purpose.

use chrono::{DateTime, Utc};
use recur_domain::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rules {
    schedules: Vec<[String; 7]>,
}

/// Cron rows where Rust deliberately differs from robfig/cron at a DST
/// transition: a wall time a gap skips runs when the gap ends (robfig skips
/// it), a repeated wall time runs once (robfig runs it twice), and Lord
/// Howe's half-hour fall-back no longer skips a day or week. `[cron, zone
/// (empty for any), after, Rust answer]`.
const DST_DIVERGENCES: &[[&str; 4]] = &[
    [
        "TZ=Europe/London 30 1 * * *",
        "",
        "2026-03-29T00:30:00Z",
        "2026-03-29T01:00:00Z",
    ],
    [
        "TZ=Europe/London 30 1 * * *",
        "",
        "2026-10-25T00:59:59Z",
        "2026-10-26T01:30:00Z",
    ],
    [
        "*/10 * * * * *",
        "America/Havana",
        "2026-11-01T05:59:00Z",
        "2026-11-01T06:00:00Z",
    ],
    [
        "30 */20 * * * *",
        "America/New_York",
        "2026-03-08T06:59:30Z",
        "2026-03-08T07:00:00Z",
    ],
    [
        "30 2 * * *",
        "America/New_York",
        "2026-03-08T06:59:30Z",
        "2026-03-08T07:00:00Z",
    ],
    [
        "* * * * *",
        "America/New_York",
        "2026-11-01T05:59:00Z",
        "2026-11-01T07:00:00Z",
    ],
    [
        "*/7 * * * *",
        "America/New_York",
        "2026-11-01T05:59:00Z",
        "2026-11-01T07:00:00Z",
    ],
    [
        "30 */20 * * * *",
        "America/New_York",
        "2026-11-01T05:59:00Z",
        "2026-11-01T07:00:30Z",
    ],
    [
        "30 1 * * *",
        "America/New_York",
        "2026-11-01T05:59:00Z",
        "2026-11-02T06:30:00Z",
    ],
    [
        "@hourly",
        "America/New_York",
        "2026-11-01T05:59:00Z",
        "2026-11-01T07:00:00Z",
    ],
    [
        "* * * * *",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-04T15:30:00Z",
    ],
    [
        "*/10 * * * * *",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-04T15:30:00Z",
    ],
    [
        "*/7 * * * *",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-04T15:30:00Z",
    ],
    [
        "0 0 * * *",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-05T13:30:00Z",
    ],
    [
        "0 0 ? * 0-6",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-05T13:30:00Z",
    ],
    [
        "0 12 * * SUN",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-05T01:30:00Z",
    ],
    [
        "30 */20 * * * *",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-04T15:30:30Z",
    ],
    [
        "5-10/2 3 * * *",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-04T16:35:00Z",
    ],
    [
        "@daily",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-05T13:30:00Z",
    ],
    [
        "@weekly",
        "Australia/Lord_Howe",
        "2026-04-04T15:00:00Z",
        "2026-04-11T13:30:00Z",
    ],
];

fn rules() -> Rules {
    serde_json::from_str(include_str!("fixtures/go_rules.json")).unwrap()
}

#[test]
fn schedules_match_go() {
    let rows = rules().schedules;
    assert!(rows.len() > 2000);
    let mut mismatches = Vec::new();
    for [cadence, cron, date, time, zone, after, want] in &rows {
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
        let want = DST_DIVERGENCES
            .iter()
            .find(|[c, z, a, _]| c == cron && (z.is_empty() || z == zone) && a == after)
            .map_or(want.as_str(), |d| d[3]);
        if got != want {
            mismatches.push(format!(
                "{cadence} {cron:?} {date} {time} {zone} after {after}: rust {got:?}, go {want:?}"
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
