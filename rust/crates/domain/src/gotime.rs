//! Go `time` semantics the schedule math depends on: wall-clock fields in a
//! zone, `time.Date` normalization and DST resolution, `ParseDuration`, and
//! `strconv.Atoi`. Instants are whole Unix seconds.

use chrono::{DateTime, Offset, TimeZone};
use chrono_tz::Tz;

pub(crate) const SECOND: i64 = 1_000_000_000;

/// Wall-clock fields of an instant in a zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Civil {
    pub year: i64,
    /// 1..=12.
    pub month: i64,
    /// 1..=31.
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
    /// 0 = Sunday.
    pub weekday: i64,
}

fn offset(tz: Tz, unix: i64) -> i64 {
    DateTime::from_timestamp(unix, 0)
        .map(|t| {
            i64::from(
                tz.offset_from_utc_datetime(&t.naive_utc())
                    .fix()
                    .local_minus_utc(),
            )
        })
        .unwrap_or(0)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (month 1..=12).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

pub(crate) fn civil(tz: Tz, unix: i64) -> Civil {
    let local = unix + offset(tz, unix);
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Civil {
        year,
        month,
        day,
        hour: secs / 3600,
        minute: secs % 3600 / 60,
        second: secs % 60,
        weekday: (days + 4).rem_euclid(7),
    }
}

/// Go `time.Date`: out-of-range fields normalize (month 13 is next January,
/// day 0 is the previous month's last day), and a wall time that a zone
/// transition skips or repeats resolves exactly as Go does.
pub(crate) fn date(tz: Tz, year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> i64 {
    let m0 = month - 1;
    let year = year + m0.div_euclid(12);
    let month = m0.rem_euclid(12) + 1;
    let unix = (days_from_civil(year, month, 1) + day - 1) * 86_400 + hour * 3600 + min * 60 + sec;
    // Go looks up the offset at the wall time read as UTC, then re-checks it
    // at the adjusted instant.
    let guess = offset(tz, unix);
    unix - offset(tz, unix - guess)
}

/// Go `Time.AddDate` on the wall clock in tz.
pub(crate) fn add_date(tz: Tz, unix: i64, years: i64, months: i64, days: i64) -> i64 {
    let c = civil(tz, unix);
    date(
        tz,
        c.year + years,
        c.month + months,
        c.day + days,
        c.hour,
        c.minute,
        c.second,
    )
}

/// Go `time.LoadLocation` for an IANA name. "" and "UTC" are UTC; "Local"
/// is `Ok(None)` (the caller decides what local means).
pub(crate) fn load_location(name: &str) -> Result<Option<Tz>, String> {
    match name {
        "" | "UTC" => Ok(Some(Tz::UTC)),
        "Local" => Ok(None),
        _ => name
            .parse::<Tz>()
            .map(Some)
            .map_err(|_| format!("unknown time zone {name}")),
    }
}

/// Go `strconv.Atoi`.
pub(crate) fn atoi(s: &str) -> Result<i64, String> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("strconv.Atoi: parsing {s:?}: invalid syntax"));
    }
    s.parse::<i64>()
        .map_err(|_| format!("strconv.Atoi: parsing {s:?}: value out of range"))
}

/// Go `time.ParseDuration`, in nanoseconds:
/// `[-+]?([0-9]*(\.[0-9]*)?[a-z]+)+`.
pub(crate) fn parse_duration(orig: &str) -> Result<i64, String> {
    const LIMIT: u64 = 1 << 63;
    let invalid = || format!("time: invalid duration {orig:?}");
    let mut s = orig.as_bytes();
    let mut neg = false;
    if let Some(&c @ (b'-' | b'+')) = s.first() {
        neg = c == b'-';
        s = &s[1..];
    }
    if s == b"0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(invalid());
    }
    let mut d: u64 = 0;
    while !s.is_empty() {
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return Err(invalid());
        }
        // [0-9]*
        let mut v: u64 = 0;
        let mut i = 0;
        while i < s.len() && s[i].is_ascii_digit() {
            if v > LIMIT / 10 {
                return Err(invalid());
            }
            v = v * 10 + u64::from(s[i] - b'0');
            if v > LIMIT {
                return Err(invalid());
            }
            i += 1;
        }
        let pre = i > 0;
        s = &s[i..];
        // (\.[0-9]*)?
        let (mut f, mut scale, mut post) = (0u64, 1f64, false);
        if s.first() == Some(&b'.') {
            s = &s[1..];
            let mut i = 0;
            let mut overflow = false;
            while i < s.len() && s[i].is_ascii_digit() {
                if !overflow {
                    if f > (LIMIT - 1) / 10 {
                        overflow = true;
                    } else {
                        let y = f * 10 + u64::from(s[i] - b'0');
                        if y > LIMIT {
                            overflow = true;
                        } else {
                            f = y;
                            scale *= 10.0;
                        }
                    }
                }
                i += 1;
            }
            post = i > 0;
            s = &s[i..];
        }
        if !pre && !post {
            return Err(invalid());
        }
        let i = s
            .iter()
            .position(|&c| c == b'.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        if i == 0 {
            return Err(format!("time: missing unit in duration {orig:?}"));
        }
        let unit_name = std::str::from_utf8(&s[..i]).unwrap_or_default();
        s = &s[i..];
        let unit: u64 = match unit_name {
            "ns" => 1,
            "us" | "\u{b5}s" | "\u{3bc}s" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60_000_000_000,
            "h" => 3_600_000_000_000,
            _ => {
                return Err(format!(
                    "time: unknown unit {unit_name:?} in duration {orig:?}"
                ));
            }
        };
        if v > LIMIT / unit {
            return Err(invalid());
        }
        v *= unit;
        if f > 0 {
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > LIMIT {
                return Err(invalid());
            }
        }
        d += v;
        if d > LIMIT {
            return Err(invalid());
        }
    }
    if neg {
        return Ok((d as i64).wrapping_neg());
    }
    if d > LIMIT - 1 {
        return Err(invalid());
    }
    Ok(d as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for days in [-719_468, -1, 0, 1, 11_016, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        let c = civil(Tz::UTC, 1_788_872_400); // 2026-09-08T13:00:00Z
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.weekday),
            (2026, 9, 8, 13, 2)
        );
    }

    #[test]
    fn date_normalizes_and_resolves_dst() {
        let utc = Tz::UTC;
        assert_eq!(
            date(utc, 2026, 13, 1, 0, 0, 0),
            date(utc, 2027, 1, 1, 0, 0, 0)
        );
        assert_eq!(
            date(utc, 2026, 3, 0, 0, 0, 0),
            date(utc, 2026, 2, 28, 0, 0, 0)
        );
        assert_eq!(
            date(utc, 2026, 1, 1, 25, 0, 0),
            date(utc, 2026, 1, 2, 1, 0, 0)
        );
        let ny: Tz = "America/New_York".parse().unwrap();
        // Expected values computed with Go's time.Date.
        // Spring gap: 02:30 does not exist; Go lands on 06:30Z (01:30 EST).
        assert_eq!(date(ny, 2026, 3, 8, 2, 30, 0), 1_772_951_400);
        // Fall back: 01:30 happens twice; Go picks the first (EDT, 05:30Z).
        assert_eq!(date(ny, 2026, 11, 1, 1, 30, 0), 1_793_511_000);
        let lord_howe: Tz = "Australia/Lord_Howe".parse().unwrap();
        assert_eq!(date(lord_howe, 2026, 4, 5, 1, 45, 0), 1_775_315_700);
        let havana: Tz = "America/Havana".parse().unwrap();
        assert_eq!(date(havana, 2026, 3, 8, 0, 0, 0), 1_772_942_400);
    }

    #[test]
    fn durations() {
        for (s, want) in [
            ("0", 0),
            ("2h30m", 9_000 * SECOND),
            ("1.5h", 5_400 * SECOND),
            ("-1m", -60 * SECOND),
            ("+5s", 5 * SECOND),
            ("300ms", 300_000_000),
            ("1\u{b5}s", 1_000),
            ("1\u{3bc}s", 1_000),
            (".5s", 500_000_000),
            ("1.s", SECOND),
        ] {
            assert_eq!(parse_duration(s), Ok(want), "{s}");
        }
        for s in [
            "",
            "nonsense",
            "1",
            ".s",
            "-",
            "1x",
            "9999999999999999999h",
            "1.2.3s",
        ] {
            assert!(parse_duration(s).is_err(), "{s}");
        }
    }

    #[test]
    fn atoi_cases() {
        assert_eq!(atoi("+5"), Ok(5));
        assert_eq!(atoi("-3"), Ok(-3));
        assert!(
            atoi("").is_err() && atoi("+").is_err() && atoi("1a").is_err() && atoi(" 1").is_err()
        );
        assert!(
            atoi("99999999999999999999")
                .unwrap_err()
                .contains("out of range")
        );
    }

    #[test]
    fn locations() {
        assert_eq!(load_location(""), Ok(Some(Tz::UTC)));
        assert_eq!(load_location("Local"), Ok(None));
        assert!(load_location("invalid/zone").is_err());
        assert_eq!(load_location("Asia/Tokyo"), Ok(Some(Tz::Asia__Tokyo)));
    }
}
