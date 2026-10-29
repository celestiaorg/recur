//! Forge timestamps: RFC3339 parsing, normalization and relative ages.

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};

/// Parses an RFC3339 timestamp (fractional seconds allowed). `None` when
/// empty or malformed.
pub fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// RFC3339 in UTC with whole seconds.
pub fn format_time(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A forge timestamp as RFC3339 UTC, or empty when it does not parse.
pub fn normalize_time(s: &str) -> String {
    parse_time(s).map(format_time).unwrap_or_default()
}

/// Relative age for PR rows: now, 5m, 3h, 2d, 4mo, 1y. Empty for no time.
pub fn compact_ago(t: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(t) = t else {
        return String::new();
    };
    let d = (now - t).max(TimeDelta::zero());
    if d < TimeDelta::minutes(1) {
        return "now".to_owned();
    }
    if d < TimeDelta::hours(1) {
        return format!("{}m", d.num_minutes());
    }
    if d < TimeDelta::hours(24) {
        return format!("{}h", d.num_hours());
    }
    let days = d.num_hours() / 24;
    if days < 30 {
        return format!("{days}d");
    }
    let months = days / 30;
    if months < 12 {
        return format!("{months}mo");
    }
    format!("{}y", months / 12)
}

/// Relative age for conversation rows: now, 5m, 3h, 12d. Unparseable is "now".
pub fn format_when(raw: &str, now: DateTime<Utc>) -> String {
    let Some(t) = parse_time(raw) else {
        return "now".to_owned();
    };
    let d = (now - t).max(TimeDelta::zero());
    if d < TimeDelta::minutes(1) {
        return "now".to_owned();
    }
    if d < TimeDelta::hours(1) {
        return format!("{}m", d.num_minutes());
    }
    if d < TimeDelta::hours(24) {
        return format!("{}h", d.num_hours());
    }
    format!("{}d", d.num_hours() / 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        parse_time(s).unwrap()
    }

    #[test]
    fn normalizes_offsets_and_fractions() {
        assert_eq!(
            normalize_time("2026-08-21T12:00:00.123+02:00"),
            "2026-08-21T10:00:00Z"
        );
        assert_eq!(
            normalize_time(" 2026-08-21T12:00:00Z "),
            "2026-08-21T12:00:00Z"
        );
        assert_eq!(normalize_time("yesterday"), "");
        assert_eq!(normalize_time(""), "");
    }

    #[test]
    fn compact_ago_buckets() {
        let now = at("2026-08-21T12:00:00Z");
        for (t, want) in [
            ("2026-08-21T12:00:30Z", "now"),
            ("2026-08-21T11:59:30Z", "now"),
            ("2026-08-21T11:55:00Z", "5m"),
            ("2026-08-21T09:00:00Z", "3h"),
            ("2026-08-19T12:00:00Z", "2d"),
            ("2026-06-01T12:00:00Z", "2mo"),
            ("2024-06-01T12:00:00Z", "2y"),
        ] {
            assert_eq!(compact_ago(Some(at(t)), now), want, "{t}");
        }
        assert_eq!(compact_ago(None, now), "");
    }

    #[test]
    fn format_when_caps_at_days() {
        let now = at("2026-08-21T12:00:00Z");
        assert_eq!(format_when("2026-06-01T12:00:00Z", now), "81d");
        assert_eq!(format_when("2026-08-21T11:00:00Z", now), "1h");
        assert_eq!(format_when("", now), "now");
    }
}
