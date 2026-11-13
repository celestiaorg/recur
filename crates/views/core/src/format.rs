//! Compact labels for UI chrome.

/// `formatCompactCount`: 460, 1.2k, 184k, 1.6M.
pub fn format_compact_count(n: f64) -> String {
    let v = n.round().max(0.);
    if v < 1000. {
        return format!("{v}");
    }
    let one_decimal = |x: f64| {
        let s = format!("{x:.1}");
        s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
    };
    if v < 10_000. {
        return format!("{}k", one_decimal(v / 1000.));
    }
    if v < 1_000_000. {
        return format!("{}k", (v / 1000.).round());
    }
    format!("{}M", one_decimal(v / 1_000_000.))
}

/// The live turn clock: `0:00` … `59:59`, then `h:mm:ss` (`formatElapsed`).
pub fn format_elapsed(sec: i64) -> String {
    let sec = sec.max(0);
    let s = sec % 60;
    if sec >= 3600 {
        return format!("{}:{:02}:{s:02}", sec / 3600, (sec % 3600) / 60);
    }
    format!("{}:{s:02}", sec / 60)
}

/// `new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short', timeZone })`
/// in en-US ("Oct 21, 2026 at 3:04 PM"), or "" when `zone` is not an IANA zone (Intl throws).
pub fn format_medium_short_in(ms: i64, zone: &str) -> String {
    use chrono::TimeZone as _;
    let Ok(tz) = zone.parse::<recur_base::tz::Tz>() else {
        return String::new();
    };
    match tz.timestamp_millis_opt(ms).single() {
        Some(d) => d.format("%b %-d, %Y at %-I:%M %p").to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::format_compact_count as f;
    use super::{format_elapsed, format_medium_short_in};

    #[test]
    fn medium_short_in_a_zone() {
        // 2026-10-21T15:04:00Z
        let ms = 1_792_595_040_000;
        assert_eq!(format_medium_short_in(ms, "UTC"), "Oct 21, 2026 at 3:04 PM");
        assert_eq!(
            format_medium_short_in(ms, "America/New_York"),
            "Oct 21, 2026 at 11:04 AM"
        );
        assert_eq!(format_medium_short_in(ms, "Local"), "");
        assert_eq!(format_medium_short_in(ms, ""), "");
    }

    #[test]
    fn elapsed_clock() {
        assert_eq!(format_elapsed(0), "0:00");
        assert_eq!(format_elapsed(5), "0:05");
        assert_eq!(format_elapsed(65), "1:05");
        assert_eq!(format_elapsed(59 * 60 + 59), "59:59");
        assert_eq!(format_elapsed(3600), "1:00:00");
        assert_eq!(format_elapsed(3600 + 16), "1:00:16");
        assert_eq!(format_elapsed(117 * 60 + 16), "1:57:16");
        assert_eq!(format_elapsed(25 * 3600 + 3 * 60), "25:03:00");
    }

    #[test]
    fn compact_counts() {
        assert_eq!(f(0.), "0");
        assert_eq!(f(999.), "999");
        assert_eq!(f(1000.), "1k");
        assert_eq!(f(1200.), "1.2k");
        assert_eq!(f(9999.), "10k");
        assert_eq!(f(40_000.), "40k");
        assert_eq!(f(184_200.), "184k");
        assert_eq!(f(1_000_000.), "1M");
        assert_eq!(f(1_600_000.), "1.6M");
        assert_eq!(f(-5.), "0");
    }
}
