//! Go-computed answers for the rules that port Go stdlib behavior by hand:
//! schedule math (robfig/cron, time.Date DST resolution, time.Parse) and
//! compatible base URL validation (net/url.Parse). See
//! `fixtures/gen_go_rules.go.txt` to regenerate.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use recur_domain::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rules {
    schedules: Vec<[String; 7]>,
    #[serde(rename = "baseURLs")]
    base_urls: Vec<[String; 2]>,
}

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
        if got != *want {
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

#[test]
fn compatible_base_urls_match_go() {
    for [input, want] in rules().base_urls {
        let p = CompatibleProvider {
            id: "p".into(),
            base_url: input.clone(),
            models: vec![CompatibleModel {
                id: "m".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let got = match normalize_compatible_provider(p, &HashSet::new()) {
            Ok(p) => p.base_url,
            Err(e) => format!("!{e}"),
        };
        assert_eq!(got, want, "{input:?}");
    }
}
