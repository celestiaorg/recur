//! A port of the robfig/cron v3 parser and schedules that Recur's Go app
//! uses for cron cadences: optional seconds field, five or six fields,
//! names (JAN, MON), `*`/`?`, ranges, steps, descriptors (@monthly,
//! @every 1h30m), a `TZ=`/`CRON_TZ=` prefix, and its exact `Next` walk
//! (including DST handling and the five-year search limit).

use chrono_tz::Tz;

use crate::gotime::{self, Civil, SECOND, add_date, civil, date};

/// Set when a field was written as `*` or `?` (dom/dow matching treats
/// "either restricted" as OR).
const STAR_BIT: u64 = 1 << 63;

struct Bounds {
    min: u64,
    max: u64,
    names: &'static [(&'static str, u64)],
}

const SECONDS: Bounds = Bounds {
    min: 0,
    max: 59,
    names: &[],
};
const MINUTES: Bounds = Bounds {
    min: 0,
    max: 59,
    names: &[],
};
const HOURS: Bounds = Bounds {
    min: 0,
    max: 23,
    names: &[],
};
const DOM: Bounds = Bounds {
    min: 1,
    max: 31,
    names: &[],
};
const MONTHS: Bounds = Bounds {
    min: 1,
    max: 12,
    names: &[
        ("jan", 1),
        ("feb", 2),
        ("mar", 3),
        ("apr", 4),
        ("may", 5),
        ("jun", 6),
        ("jul", 7),
        ("aug", 8),
        ("sep", 9),
        ("oct", 10),
        ("nov", 11),
        ("dec", 12),
    ],
};
const DOW: Bounds = Bounds {
    min: 0,
    max: 6,
    names: &[
        ("sun", 0),
        ("mon", 1),
        ("tue", 2),
        ("wed", 3),
        ("thu", 4),
        ("fri", 5),
        ("sat", 6),
    ],
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Schedule {
    Spec(Spec),
    /// Fires every `delay` nanoseconds (whole seconds), from the given time.
    Every {
        delay: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spec {
    second: u64,
    minute: u64,
    hour: u64,
    dom: u64,
    month: u64,
    dow: u64,
    /// `None` is Go's `time.Local`: the zone of the time passed to `next`.
    location: Option<Tz>,
}

/// Parses with robfig's `SecondOptional | Minute | Hour | Dom | Month | Dow
/// | Descriptor` options. Errors carry robfig's messages.
pub(crate) fn parse(spec: &str) -> Result<Schedule, String> {
    if spec.is_empty() {
        return Err("empty spec string".into());
    }
    let mut spec = spec;
    let mut location = None;
    if spec.starts_with("TZ=") || spec.starts_with("CRON_TZ=") {
        let eq = spec.find('=').unwrap_or_default();
        // Go slices to the first space and panics without one; refuse instead.
        let Some(i) = spec.find(' ') else {
            return Err(format!(
                "provided bad location {}: missing schedule",
                &spec[eq + 1..]
            ));
        };
        let name = &spec[eq + 1..i];
        location = gotime::load_location(name)
            .map_err(|e| format!("provided bad location {name}: {e}"))?;
        spec = spec[i..].trim();
    }

    if spec.starts_with('@') {
        return parse_descriptor(spec, location);
    }

    let mut fields: Vec<&str> = spec.split_whitespace().collect();
    if !(5..=6).contains(&fields.len()) {
        return Err(format!(
            "expected 5 to 6 fields, found {}: [{}]",
            fields.len(),
            fields.join(" ")
        ));
    }
    if fields.len() == 5 {
        fields.insert(0, "0");
    }
    Ok(Schedule::Spec(Spec {
        second: get_field(fields[0], &SECONDS)?,
        minute: get_field(fields[1], &MINUTES)?,
        hour: get_field(fields[2], &HOURS)?,
        dom: get_field(fields[3], &DOM)?,
        month: get_field(fields[4], &MONTHS)?,
        dow: get_field(fields[5], &DOW)?,
        location,
    }))
}

/// A comma-separated list of ranges (empty items are skipped).
fn get_field(field: &str, r: &Bounds) -> Result<u64, String> {
    let mut bits = 0;
    for expr in field.split(',').filter(|e| !e.is_empty()) {
        bits |= get_range(expr, r)?;
    }
    Ok(bits)
}

/// `number | number "-" number [ "/" number ]`, or `*`/`?` with a step.
fn get_range(expr: &str, r: &Bounds) -> Result<u64, String> {
    let range_and_step: Vec<&str> = expr.split('/').collect();
    let low_and_high: Vec<&str> = range_and_step[0].split('-').collect();
    let single = low_and_high.len() == 1;
    let (start, mut end, mut extra);
    if low_and_high[0] == "*" || low_and_high[0] == "?" {
        (start, end, extra) = (r.min, r.max, STAR_BIT);
    } else {
        start = parse_int_or_name(low_and_high[0], r.names)?;
        end = match low_and_high.len() {
            1 => start,
            2 => parse_int_or_name(low_and_high[1], r.names)?,
            _ => return Err(format!("too many hyphens: {expr}")),
        };
        extra = 0;
    }
    let step = match range_and_step.len() {
        1 => 1,
        2 => {
            let step = must_parse_int(range_and_step[1])?;
            // "N/step" means "N-max/step".
            if single {
                end = r.max;
            }
            if step > 1 {
                extra = 0;
            }
            step
        }
        _ => return Err(format!("too many slashes: {expr}")),
    };
    if start < r.min {
        return Err(format!(
            "beginning of range ({start}) below minimum ({}): {expr}",
            r.min
        ));
    }
    if end > r.max {
        return Err(format!(
            "end of range ({end}) above maximum ({}): {expr}",
            r.max
        ));
    }
    if start > end {
        return Err(format!(
            "beginning of range ({start}) beyond end of range ({end}): {expr}"
        ));
    }
    if step == 0 {
        return Err(format!("step of range should be a positive number: {expr}"));
    }
    Ok(get_bits(start, end, step) | extra)
}

fn parse_int_or_name(expr: &str, names: &[(&str, u64)]) -> Result<u64, String> {
    let lower = expr.to_lowercase();
    match names.iter().find(|(n, _)| *n == lower) {
        Some(&(_, v)) => Ok(v),
        None => must_parse_int(expr),
    }
}

fn must_parse_int(expr: &str) -> Result<u64, String> {
    let num = gotime::atoi(expr).map_err(|e| format!("failed to parse int from {expr}: {e}"))?;
    if num < 0 {
        return Err(format!("negative number ({num}) not allowed: {expr}"));
    }
    Ok(num as u64)
}

/// Bits in [min, max] at the given step. Callers validate max <= 63.
fn get_bits(min: u64, max: u64, step: u64) -> u64 {
    (min..=max)
        .step_by(step as usize)
        .fold(0, |bits, i| bits | (1 << i))
}

fn all(r: &Bounds) -> u64 {
    get_bits(r.min, r.max, 1) | STAR_BIT
}

fn parse_descriptor(descriptor: &str, location: Option<Tz>) -> Result<Schedule, String> {
    let first = |r: &Bounds| 1u64 << r.min;
    let spec = |hour: u64, dom: u64, month: u64, dow: u64| {
        Ok(Schedule::Spec(Spec {
            second: first(&SECONDS),
            minute: first(&MINUTES),
            hour,
            dom,
            month,
            dow,
            location,
        }))
    };
    match descriptor {
        "@yearly" | "@annually" => spec(first(&HOURS), first(&DOM), first(&MONTHS), all(&DOW)),
        "@monthly" => spec(first(&HOURS), first(&DOM), all(&MONTHS), all(&DOW)),
        "@weekly" => spec(first(&HOURS), all(&DOM), all(&MONTHS), first(&DOW)),
        "@daily" | "@midnight" => spec(first(&HOURS), all(&DOM), all(&MONTHS), all(&DOW)),
        "@hourly" => spec(all(&HOURS), all(&DOM), all(&MONTHS), all(&DOW)),
        _ => match descriptor.strip_prefix("@every ") {
            Some(d) => {
                let duration = gotime::parse_duration(d)
                    .map_err(|e| format!("failed to parse duration {descriptor}: {e}"))?;
                // Under a second rounds up to one; sub-second parts are truncated.
                let duration = duration.max(SECOND);
                Ok(Schedule::Every {
                    delay: duration - duration % SECOND,
                })
            }
            None => Err(format!("unrecognized descriptor: {descriptor}")),
        },
    }
}

impl Schedule {
    /// The next activation strictly after `unix` (whole seconds; the
    /// sub-second part of the original time is already dropped) for a time
    /// whose zone is `tz`. `None` when nothing matches within five years.
    pub(crate) fn next(&self, tz: Tz, unix: i64) -> Option<i64> {
        match self {
            Schedule::Every { delay } => Some(unix + delay / SECOND),
            Schedule::Spec(s) => s.next(tz, unix),
        }
    }
}

impl Spec {
    fn day_matches(&self, c: &Civil) -> bool {
        let dom_match = (1u64 << c.day) & self.dom > 0;
        let dow_match = (1u64 << c.weekday) & self.dow > 0;
        if self.dom & STAR_BIT > 0 || self.dow & STAR_BIT > 0 {
            dom_match && dow_match
        } else {
            dom_match || dow_match
        }
    }

    fn next(&self, tz: Tz, unix: i64) -> Option<i64> {
        let loc = self.location.unwrap_or(tz);
        // Start at the earliest possible time (the upcoming second).
        let mut t = unix + 1;
        let mut added = false;
        // If no time is found within five years, give up.
        let year_limit = civil(loc, t).year + 5;

        'wrap: loop {
            let c = civil(loc, t);
            if c.year > year_limit {
                return None;
            }

            while (1u64 << civil(loc, t).month) & self.month == 0 {
                if !added {
                    added = true;
                    let c = civil(loc, t);
                    t = date(loc, c.year, c.month, 1, 0, 0, 0);
                }
                t = add_date(loc, t, 0, 1, 0);
                if civil(loc, t).month == 1 {
                    continue 'wrap;
                }
            }

            // Days whose midnight a DST shift skips land at 23:00 or 01:00;
            // nudge back to midnight.
            while !self.day_matches(&civil(loc, t)) {
                if !added {
                    added = true;
                    let c = civil(loc, t);
                    t = date(loc, c.year, c.month, c.day, 0, 0, 0);
                }
                t = add_date(loc, t, 0, 0, 1);
                let hour = civil(loc, t).hour;
                if hour != 0 {
                    t += if hour > 12 {
                        (24 - hour) * 3600
                    } else {
                        -hour * 3600
                    };
                }
                if civil(loc, t).day == 1 {
                    continue 'wrap;
                }
            }

            while (1u64 << civil(loc, t).hour) & self.hour == 0 {
                if !added {
                    added = true;
                    let c = civil(loc, t);
                    t = date(loc, c.year, c.month, c.day, c.hour, 0, 0);
                }
                t += 3600;
                if civil(loc, t).hour == 0 {
                    continue 'wrap;
                }
            }

            while (1u64 << civil(loc, t).minute) & self.minute == 0 {
                if !added {
                    added = true;
                    t = t.div_euclid(60) * 60;
                }
                t += 60;
                if civil(loc, t).minute == 0 {
                    continue 'wrap;
                }
            }

            while (1u64 << civil(loc, t).second) & self.second == 0 {
                added = true;
                t += 1;
                if civil(loc, t).second == 0 {
                    continue 'wrap;
                }
            }

            return Some(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(s: &str) -> Spec {
        match parse(s).unwrap() {
            Schedule::Spec(spec) => spec,
            other => panic!("{s}: {other:?}"),
        }
    }

    #[test]
    fn fields_and_ranges() {
        let s = spec("*/15 9-17 * * MON-FRI");
        assert_eq!(s.second, 1);
        assert_eq!(s.minute, (1 << 0) | (1 << 15) | (1 << 30) | (1 << 45));
        assert_eq!(s.hour, get_bits(9, 17, 1));
        assert_eq!(s.dom, all(&DOM));
        assert_eq!(s.dow, get_bits(1, 5, 1));
        let six = spec("*/10 * * * * *");
        assert_eq!(six.second, get_bits(0, 59, 10));
        assert_eq!(
            spec("5/20 * * * *").minute,
            (1 << 5) | (1 << 25) | (1 << 45)
        );
        assert_eq!(
            spec("*/1 * * * *").minute,
            all(&MINUTES),
            "step 1 keeps the star bit"
        );
        assert_eq!(spec("1,,3 * * * *").minute, (1 << 1) | (1 << 3));
        assert_eq!(spec("0 0 ? jan,Mar *").month, (1 << 1) | (1 << 3));
        assert_eq!(spec("0 0 1 * *").location, None);
        assert_eq!(
            spec("CRON_TZ=Asia/Tokyo 0 0 1 * *").location,
            Some(Tz::Asia__Tokyo)
        );
        assert_eq!(spec("TZ=Local 0 0 1 * *").location, None);
        assert_eq!(spec("TZ= 0 0 1 * *").location, Some(Tz::UTC));
    }

    #[test]
    fn parse_errors() {
        for (s, want) in [
            ("", "empty spec string"),
            ("bad cron", "expected 5 to 6 fields, found 2: [bad cron]"),
            ("60 * * * *", "end of range (60) above maximum (59): 60"),
            (
                "*/0 * * * *",
                "step of range should be a positive number: */0",
            ),
            ("1-2-3 * * * *", "too many hyphens: 1-2-3"),
            ("1/2/3 * * * *", "too many slashes: 1/2/3"),
            (
                "5-1 * * * *",
                "beginning of range (5) beyond end of range (1): 5-1",
            ),
            ("0 0 0 * *", "beginning of range (0) below minimum (1): 0"),
            (
                "x * * * *",
                "failed to parse int from x: strconv.Atoi: parsing \"x\": invalid syntax",
            ),
            (
                "@every nonsense",
                "failed to parse duration @every nonsense: time: invalid duration \"nonsense\"",
            ),
            ("@fortnightly", "unrecognized descriptor: @fortnightly"),
            (
                "CRON_TZ=Nowhere/Land 0 * * * *",
                "provided bad location Nowhere/Land: unknown time zone Nowhere/Land",
            ),
            ("CRON_TZ=UTC", "provided bad location UTC: missing schedule"),
        ] {
            assert_eq!(parse(s).unwrap_err(), want, "{s:?}");
        }
    }

    #[test]
    fn descriptors() {
        assert_eq!(spec("@yearly"), spec("@annually"));
        assert_eq!(spec("@daily"), spec("@midnight"));
        assert_eq!(spec("@weekly").dow, 1);
        assert_eq!(spec("@hourly").hour, all(&HOURS));
        assert_eq!(
            parse("@every 2h30m"),
            Ok(Schedule::Every {
                delay: 9_000 * SECOND
            })
        );
        assert_eq!(
            parse("@every 1500ms"),
            Ok(Schedule::Every { delay: SECOND })
        );
        assert_eq!(parse("@every 10ms"), Ok(Schedule::Every { delay: SECOND }));
        assert_eq!(parse("@every -5m"), Ok(Schedule::Every { delay: SECOND }));
    }
}
