//! IANA time zones from the system database (`$TZDIR`, else `/usr/share/zoneinfo`) as a
//! chrono [`TimeZone`]. A zone file (TZif, RFC 8536) lists its transitions; times after the
//! last one follow its POSIX TZ footer rule (`EST5EDT,M3.2.0,M11.1.0`). Zones load once and
//! stay cached for the life of the process.

use chrono::{Datelike, FixedOffset, MappedLocalTime, NaiveDate, NaiveDateTime, Offset, TimeZone};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy)]
pub struct Tz(&'static Zone);

struct Zone {
    name: &'static str,
    /// UTC seconds at which `types[idx[i]]` starts.
    transitions: &'static [i64],
    idx: &'static [u8],
    types: &'static [LocalType],
    rule: Option<Rule>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LocalType {
    /// Seconds east of UTC.
    off: i32,
    abbr: &'static str,
}

/// A POSIX TZ rule: standard time, and DST between two yearly dates when present.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rule {
    std: LocalType,
    dst: Option<(LocalType, RuleDate, RuleDate)>,
}

/// A yearly date and the local time of day (seconds, may be negative or past 24h) it
/// applies at.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RuleDate {
    day: Day,
    time: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Day {
    /// `Jn`: 1..=365, February 29 never counted.
    Julian1(u16),
    /// `n`: 0..=365, February 29 counted.
    Julian0(u16),
    /// `Mm.w.d`: weekday `d` (0 Sunday) of week `w` (5 the last) of month `m`.
    Month(u8, u8, u8),
}

static UTC_ZONE: Zone = Zone {
    name: "UTC",
    transitions: &[],
    idx: &[],
    types: &[LocalType {
        off: 0,
        abbr: "UTC",
    }],
    rule: None,
};

impl Tz {
    pub const UTC: Tz = Tz(&UTC_ZONE);

    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// The local type in effect at `t` (UTC seconds).
    fn at(self, t: i64) -> LocalType {
        let z = self.0;
        match z.transitions.last() {
            Some(&last) if t < last || z.rule.is_none() => {
                let i = z.transitions.partition_point(|&x| x <= t);
                match i {
                    0 => z.types[0],
                    i => z.types[usize::from(z.idx[i - 1])],
                }
            }
            _ => z.rule.map_or(z.types[0], |r| r.at(t)),
        }
    }

    /// Every distinct offset the zone uses.
    fn offsets(self) -> Vec<LocalType> {
        let z = self.0;
        let mut out: Vec<LocalType> = z.types.to_vec();
        if let Some(r) = z.rule {
            out.push(r.std);
            out.extend(r.dst.map(|d| d.0));
        }
        out.sort_by_key(|t| t.off);
        out.dedup_by_key(|t| t.off);
        out
    }

    fn offset(self, t: LocalType) -> TzOffset {
        TzOffset { tz: self, ty: t }
    }
}

impl PartialEq for Tz {
    fn eq(&self, other: &Tz) -> bool {
        self.0.name == other.0.name
    }
}

impl Eq for Tz {}

impl fmt::Debug for Tz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.name)
    }
}

impl fmt::Display for Tz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownZone(pub String);

impl fmt::Display for UnknownZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown time zone {}", self.0)
    }
}

impl std::error::Error for UnknownZone {}

impl FromStr for Tz {
    type Err = UnknownZone;

    /// An IANA zone name (`America/New_York`, `UTC`, links like `US/Eastern`).
    fn from_str(name: &str) -> Result<Tz, UnknownZone> {
        static CACHE: OnceLock<Mutex<HashMap<String, Option<Tz>>>> = OnceLock::new();
        let unknown = || UnknownZone(name.to_owned());
        // Names come from users: only plain zone paths reach the file system.
        let plain = !name.is_empty()
            && !name.starts_with('/')
            && name
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != "..")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
            && !name.starts_with("posix/")
            && !name.starts_with("right/");
        if !plain {
            return Err(unknown());
        }
        let cache = CACHE.get_or_init(Default::default);
        if let Some(hit) = cache.lock().ok().and_then(|c| c.get(name).copied()) {
            return hit.ok_or_else(unknown);
        }
        let dir = std::env::var_os("TZDIR").unwrap_or_else(|| "/usr/share/zoneinfo".into());
        let loaded = std::fs::read(std::path::Path::new(&dir).join(name))
            .ok()
            .and_then(|b| parse_tzif(&b))
            .map(|mut z| {
                z.name = Box::leak(name.into());
                Tz(Box::leak(Box::new(z)))
            })
            .or((name == "UTC").then_some(Tz::UTC));
        if let Ok(mut c) = cache.lock() {
            c.insert(name.to_owned(), loaded);
        }
        loaded.ok_or_else(unknown)
    }
}

#[derive(Clone, Copy)]
pub struct TzOffset {
    tz: Tz,
    ty: LocalType,
}

impl TzOffset {
    pub fn abbreviation(&self) -> &'static str {
        self.ty.abbr
    }
}

impl fmt::Debug for TzOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.ty.abbr)
    }
}

impl fmt::Display for TzOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.ty.abbr)
    }
}

impl Offset for TzOffset {
    fn fix(&self) -> FixedOffset {
        FixedOffset::east_opt(self.ty.off).unwrap_or(FixedOffset::east_opt(0).expect("zero"))
    }
}

impl TimeZone for Tz {
    type Offset = TzOffset;

    fn from_offset(offset: &TzOffset) -> Tz {
        offset.tz
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<TzOffset> {
        self.offset_from_local_datetime(&local.and_time(chrono::NaiveTime::MIN))
    }

    /// Every offset `o` with `local - o` back at `o`: none in a gap, two in an overlap.
    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> MappedLocalTime<TzOffset> {
        let l = local.and_utc().timestamp();
        let mut hits: Vec<(i64, LocalType)> = self
            .offsets()
            .into_iter()
            .filter_map(|o| {
                let u = l - i64::from(o.off);
                let at = self.at(u);
                (at.off == o.off).then_some((u, at))
            })
            .collect();
        hits.sort_by_key(|h| h.0);
        match (hits.first(), hits.last()) {
            (None, _) | (_, None) => MappedLocalTime::None,
            (Some(a), Some(b)) if a.0 == b.0 => MappedLocalTime::Single(self.offset(a.1)),
            (Some(a), Some(b)) => MappedLocalTime::Ambiguous(self.offset(a.1), self.offset(b.1)),
        }
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> TzOffset {
        self.offset_from_utc_datetime(&utc.and_time(chrono::NaiveTime::MIN))
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> TzOffset {
        self.offset(self.at(utc.and_utc().timestamp()))
    }
}

impl Rule {
    fn at(&self, t: i64) -> LocalType {
        let Some((dst, start, end)) = self.dst else {
            return self.std;
        };
        let local = t + i64::from(self.std.off);
        let Some(year) = chrono::DateTime::from_timestamp(local, 0).map(|d| d.year()) else {
            return self.std;
        };
        // `start` is in standard time, `end` in daylight time.
        let (Some(s), Some(e)) = (start.utc(year, self.std.off), end.utc(year, dst.off)) else {
            return self.std;
        };
        let in_dst = if s < e {
            s <= t && t < e
        } else {
            !(e <= t && t < s)
        };
        if in_dst { dst } else { self.std }
    }
}

impl RuleDate {
    /// The UTC second this date falls on in `year`, given the offset in effect before it.
    fn utc(self, year: i32, off: i32) -> Option<i64> {
        let leap = NaiveDate::from_ymd_opt(year, 2, 29).is_some();
        let date = match self.day {
            Day::Julian1(n) => {
                let n = u32::from(n);
                let ord = if leap && n >= 60 { n + 1 } else { n };
                NaiveDate::from_yo_opt(year, ord)?
            }
            Day::Julian0(n) => NaiveDate::from_yo_opt(year, u32::from(n) + 1)?,
            Day::Month(m, w, d) => {
                let first = NaiveDate::from_ymd_opt(year, u32::from(m), 1)?;
                let shift = (7 + u32::from(d) - first.weekday().num_days_from_sunday()) % 7;
                let mut day = 1 + shift + 7 * (u32::from(w) - 1);
                let days = first
                    .checked_add_months(chrono::Months::new(1))?
                    .pred_opt()?
                    .day();
                while day > days {
                    day -= 7;
                }
                first.with_day(day)?
            }
        };
        let midnight = date.and_time(chrono::NaiveTime::MIN).and_utc().timestamp();
        Some(midnight + i64::from(self.time) - i64::from(off))
    }
}

/// A TZif file: the version 2+ (64-bit) data when present, and its footer rule.
fn parse_tzif(b: &[u8]) -> Option<Zone> {
    let header = |b: &[u8]| -> Option<[usize; 6]> {
        if b.get(..4)? != b"TZif" {
            return None;
        }
        let mut n = [0usize; 6];
        for (i, v) in n.iter_mut().enumerate() {
            let at = 20 + 4 * i;
            *v = u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize;
        }
        Some(n)
    };
    let block_len = |n: [usize; 6], t: usize| {
        let [isut, isstd, leap, time, typ, chars] = n;
        time * t + time + typ * 6 + chars + leap * (t + 4) + isstd + isut
    };
    let n1 = header(b)?;
    let v2 = b[4] >= b'2';
    let (n, data, t) = if v2 {
        let rest = b.get(44 + block_len(n1, 4)..)?;
        (header(rest)?, rest.get(44..)?, 8)
    } else {
        (n1, b.get(44..)?, 4)
    };
    let [_, _, _, time, typ, chars] = n;
    if typ == 0 {
        return None;
    }
    let mut p = 0;
    let mut take = |len: usize| {
        let s = data.get(p..p + len);
        p += len;
        s
    };
    let times = take(time * t)?;
    let idx = take(time)?;
    let raw_types = take(typ * 6)?;
    let abbrs = take(chars)?;
    let transitions: Vec<i64> = times
        .chunks(t)
        .map(|c| match t {
            8 => i64::from_be_bytes(c.try_into().unwrap_or_default()),
            _ => i64::from(i32::from_be_bytes(c.try_into().unwrap_or_default())),
        })
        .collect();
    if idx.iter().any(|&i| usize::from(i) >= typ) {
        return None;
    }
    let types: Vec<LocalType> = raw_types
        .chunks(6)
        .map(|c| {
            let abbr = abbrs.get(usize::from(c[5])..).unwrap_or_default();
            let abbr = &abbr[..abbr.iter().position(|&b| b == 0).unwrap_or(abbr.len())];
            LocalType {
                off: i32::from_be_bytes([c[0], c[1], c[2], c[3]]),
                abbr: Box::leak(String::from_utf8_lossy(abbr).into()),
            }
        })
        .collect();
    let rule = if v2 {
        let footer = data.get(block_len(n, t)..)?;
        let footer = footer.strip_prefix(b"\n")?;
        let end = footer.iter().position(|&b| b == b'\n')?;
        match &footer[..end] {
            [] => None,
            s => Some(parse_rule(std::str::from_utf8(s).ok()?)?),
        }
    } else {
        None
    };
    Some(Zone {
        name: "",
        transitions: Box::leak(transitions.into()),
        idx: Box::leak(idx.into()),
        types: Box::leak(types.into()),
        rule,
    })
}

/// `std offset [dst [offset] [,start[/time],end[/time]]]`.
fn parse_rule(s: &str) -> Option<Rule> {
    let mut p = Cursor(s.as_bytes());
    let std_name = p.name()?;
    let std_off = -p.time()?;
    let std = LocalType {
        off: std_off,
        abbr: Box::leak(std_name.into()),
    };
    if p.0.is_empty() {
        return Some(Rule { std, dst: None });
    }
    let dst_name = p.name()?;
    let dst_off = if p.0.first().is_some_and(|&b| b != b',') {
        -p.time()?
    } else {
        std_off + 3600
    };
    let dst = LocalType {
        off: dst_off,
        abbr: Box::leak(dst_name.into()),
    };
    let (start, end) = if p.0.is_empty() {
        // POSIX's default when a zone names DST without dates: the US rule.
        (
            RuleDate {
                day: Day::Month(3, 2, 0),
                time: 7200,
            },
            RuleDate {
                day: Day::Month(11, 1, 0),
                time: 7200,
            },
        )
    } else {
        p.eat(b',')?;
        let start = p.date()?;
        p.eat(b',')?;
        (start, p.date()?)
    };
    p.0.is_empty().then_some(Rule {
        std,
        dst: Some((dst, start, end)),
    })
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn eat(&mut self, c: u8) -> Option<()> {
        let (&first, rest) = self.0.split_first()?;
        (first == c).then(|| self.0 = rest)
    }

    /// `EST` or `<-03>`.
    fn name(&mut self) -> Option<String> {
        let (name, rest) = if self.eat(b'<').is_some() {
            let end = self.0.iter().position(|&b| b == b'>')?;
            (&self.0[..end], &self.0[end + 1..])
        } else {
            let end = self
                .0
                .iter()
                .position(|b| !b.is_ascii_alphabetic())
                .unwrap_or(self.0.len());
            (&self.0[..end], &self.0[end..])
        };
        if name.len() < 3 {
            return None;
        }
        self.0 = rest;
        Some(String::from_utf8_lossy(name).into_owned())
    }

    fn num(&mut self) -> Option<i32> {
        let end = self
            .0
            .iter()
            .position(|b| !b.is_ascii_digit())
            .unwrap_or(self.0.len());
        let n = std::str::from_utf8(&self.0[..end]).ok()?.parse().ok()?;
        self.0 = &self.0[end..];
        Some(n)
    }

    /// `[+-]hh[:mm[:ss]]` in seconds; hours up to 167.
    fn time(&mut self) -> Option<i32> {
        let sign = if self.eat(b'-').is_some() {
            -1
        } else {
            let _ = self.eat(b'+');
            1
        };
        let h = self.num()?;
        let m = if self.eat(b':').is_some() {
            self.num()?
        } else {
            0
        };
        let s = if self.eat(b':').is_some() {
            self.num()?
        } else {
            0
        };
        (h <= 167 && m < 60 && s < 60).then_some(sign * (h * 3600 + m * 60 + s))
    }

    /// `Jn`, `n` or `Mm.w.d`, then `/time` (default 02:00).
    fn date(&mut self) -> Option<RuleDate> {
        let day = if self.eat(b'J').is_some() {
            let n = self.num()?;
            (1..=365).contains(&n).then_some(Day::Julian1(n as u16))?
        } else if self.eat(b'M').is_some() {
            let m = self.num()?;
            self.eat(b'.')?;
            let w = self.num()?;
            self.eat(b'.')?;
            let d = self.num()?;
            ((1..=12).contains(&m) && (1..=5).contains(&w) && (0..=6).contains(&d))
                .then_some(Day::Month(m as u8, w as u8, d as u8))?
        } else {
            let n = self.num()?;
            (0..=365).contains(&n).then_some(Day::Julian0(n as u16))?
        };
        let time = if self.eat(b'/').is_some() {
            self.time()?
        } else {
            7200
        };
        Some(RuleDate { day, time })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn tz(name: &str) -> Tz {
        name.parse().unwrap()
    }

    /// UTC offset in hours at an RFC 3339 UTC instant.
    fn off(zone: &str, utc: &str) -> f64 {
        let t = chrono::DateTime::parse_from_rfc3339(utc)
            .unwrap()
            .with_timezone(&Utc);
        f64::from(t.with_timezone(&tz(zone)).offset().fix().local_minus_utc()) / 3600.
    }

    fn local(zone: &str, s: &str) -> MappedLocalTime<String> {
        let wall = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        tz(zone)
            .from_local_datetime(&wall)
            .map(|d| d.with_timezone(&Utc).format("%H:%MZ").to_string())
    }

    #[test]
    fn offsets_in_table_and_rule_years() {
        for (zone, utc, want) in [
            ("America/New_York", "2026-01-15T12:00:00Z", -5.),
            ("America/New_York", "2026-07-15T12:00:00Z", -4.),
            // Past the table: the footer rule.
            ("America/New_York", "2099-07-15T12:00:00Z", -4.),
            ("America/New_York", "2099-12-15T12:00:00Z", -5.),
            ("America/New_York", "1950-07-15T12:00:00Z", -4.),
            ("Europe/London", "2026-03-29T00:59:59Z", 0.),
            ("Europe/London", "2026-03-29T01:00:00Z", 1.),
            ("Europe/London", "2088-10-31T00:59:59Z", 1.),
            ("Europe/London", "2088-10-31T01:00:00Z", 0.),
            // Southern hemisphere: DST spans the new year.
            ("Australia/Sydney", "2026-01-15T00:00:00Z", 11.),
            ("Australia/Sydney", "2026-07-15T00:00:00Z", 10.),
            ("Australia/Sydney", "2091-01-01T00:00:00Z", 11.),
            ("Australia/Sydney", "2091-06-01T00:00:00Z", 10.),
            ("Australia/Lord_Howe", "2026-01-15T00:00:00Z", 11.),
            ("Australia/Lord_Howe", "2026-07-15T00:00:00Z", 10.5),
            ("Pacific/Chatham", "2026-01-15T00:00:00Z", 13.75),
            ("Asia/Kolkata", "2026-01-15T00:00:00Z", 5.5),
            ("America/Sao_Paulo", "2026-01-15T00:00:00Z", -3.),
            ("Asia/Tokyo", "2090-01-15T00:00:00Z", 9.),
            ("UTC", "2026-01-15T00:00:00Z", 0.),
            ("US/Eastern", "2026-07-15T12:00:00Z", -4.),
        ] {
            assert_eq!(off(zone, utc), want, "{zone} at {utc}");
        }
    }

    #[test]
    fn local_times_in_gaps_and_overlaps() {
        use chrono::LocalResult::*;
        let s = |x: &str| x.to_string();
        // Spring forward: 02:00-03:00 does not exist; fall back: 01:00-02:00 happens twice.
        assert_eq!(
            local("America/New_York", "2026-03-08 01:59"),
            Single(s("06:59Z"))
        );
        assert_eq!(local("America/New_York", "2026-03-08 02:30"), None);
        assert_eq!(
            local("America/New_York", "2026-03-08 03:00"),
            Single(s("07:00Z"))
        );
        assert_eq!(
            local("America/New_York", "2026-11-01 01:30"),
            Ambiguous(s("05:30Z"), s("06:30Z"))
        );
        assert_eq!(local("America/New_York", "2097-03-10 02:30"), None);
        assert_eq!(
            local("America/New_York", "2097-11-03 01:30"),
            Ambiguous(s("05:30Z"), s("06:30Z"))
        );
        assert_eq!(local("Australia/Sydney", "2026-10-04 02:30"), None);
        assert_eq!(
            local("Australia/Sydney", "2026-04-05 02:30"),
            Ambiguous(s("15:30Z"), s("16:30Z"))
        );
        assert_eq!(local("Australia/Lord_Howe", "2026-10-04 02:15"), None);
        assert_eq!(local("Asia/Tokyo", "2026-03-08 02:30"), Single(s("17:30Z")));
    }

    #[test]
    fn names() {
        assert_eq!(tz("UTC"), Tz::UTC);
        assert_eq!(tz("Asia/Tokyo").name(), "Asia/Tokyo");
        assert_eq!(tz("Asia/Tokyo"), tz("Asia/Tokyo"));
        for bad in [
            "",
            "Nowhere/City",
            "../../etc/passwd",
            "/etc/localtime",
            "America/../America/New_York",
            "right/America/New_York",
            "zone.tab",
            "America",
        ] {
            assert!(bad.parse::<Tz>().is_err(), "{bad}");
        }
    }

    #[test]
    fn posix_rules() {
        let r = parse_rule("<-02>2<-01>,M3.5.0/-1,M10.5.0/0").unwrap();
        assert_eq!(r.std.off, -7200);
        let (dst, start, end) = r.dst.unwrap();
        assert_eq!((dst.abbr, dst.off), ("-01", -3600));
        assert_eq!(
            start,
            RuleDate {
                day: Day::Month(3, 5, 0),
                time: -3600
            }
        );
        assert_eq!(end.time, 0);
        // March 2026's last Sunday is the 29th, 23:00 local standard the day before.
        let s = start.utc(2026, r.std.off).unwrap();
        assert_eq!(
            chrono::DateTime::from_timestamp(s, 0).unwrap().to_rfc3339(),
            "2026-03-29T01:00:00+00:00"
        );
        let r = parse_rule("IST-2IDT,M3.4.4/26,M10.5.0").unwrap();
        assert_eq!(r.dst.unwrap().1.time, 26 * 3600);
        let r = parse_rule("XXX3YYY,J60/0,300/1:30").unwrap();
        let (_, s, e) = r.dst.unwrap();
        // J60 is March 1 even in a leap year; zero-based 300 counts February 29.
        let day = |d: RuleDate, y| {
            chrono::DateTime::from_timestamp(d.utc(y, 0).unwrap(), 0)
                .unwrap()
                .date_naive()
        };
        assert_eq!(day(s, 2028).to_string(), "2028-03-01");
        assert_eq!(day(e, 2028).to_string(), "2028-10-27");
        assert_eq!(day(e, 2027).to_string(), "2027-10-28");
        assert_eq!(parse_rule("JST-9").unwrap().dst, None);
        for bad in [
            "",
            "AB1",
            "EST5EDT,M13.1.0,M11.1.0",
            "EST5EDT,M3.2.0",
            "EST",
            "EST5EDT,J0,J1",
        ] {
            assert!(parse_rule(bad).is_none(), "{bad}");
        }
    }
}
