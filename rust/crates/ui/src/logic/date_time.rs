//! The segmented editor behind WebKit's `<input type=date>` / `<input type=time>`
//! (`DateTimeEditElement` and its field elements): a locale pattern split into numeric and
//! AM/PM fields with literals between them, each field stepped, typed or cleared on its own,
//! and the input's value composed from all of them.

use std::time::{Duration, Instant};

/// `typeAheadTimeout`: digits typed further apart start a new number.
const TYPE_AHEAD_TIMEOUT: Duration = Duration::from_secs(1);
/// `DateComponents::minimumYear` / `maximumYear`.
const MIN_YEAR: i32 = 1;
const MAX_YEAR: i32 = 275_760;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Date,
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Year,
    Month,
    Day,
    /// The hour in `min..=max` (`h` 1-12, `K` 0-11, `H` 0-23, `k` 1-24).
    Hour {
        min: i32,
        max: i32,
    },
    Minute,
    Meridiem,
}

impl FieldKind {
    fn range(self) -> (i32, i32) {
        match self {
            Self::Year => (MIN_YEAR, MAX_YEAR),
            Self::Month => (1, 12),
            Self::Day => (1, 31),
            Self::Hour { min, max } => (min, max),
            Self::Minute => (0, 59),
            Self::Meridiem => (0, 1),
        }
    }

    /// `formatValue`: four digits past 999, three past 99, else two.
    fn digits(self) -> usize {
        match self.range().1 {
            m if m > 999 => 4,
            m if m > 99 => 3,
            _ => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Field(FieldKind),
    Literal(String),
}

/// `DateTimeFormat::parse` over an ICU / `NSDateFormatter` pattern: letter runs are fields,
/// quoted runs and everything else literals. Fields the input does not edit (era, weekday,
/// seconds) are dropped.
pub fn parse_pattern(pattern: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let flush = |literal: &mut String, parts: &mut Vec<Part>| {
        if !literal.is_empty() {
            parts.push(Part::Literal(std::mem::take(literal)));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                literal.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        literal.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                literal.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() {
            while i < chars.len() && chars[i] == c {
                i += 1;
            }
            let field = match c {
                'y' | 'Y' | 'u' => Some(FieldKind::Year),
                'M' | 'L' => Some(FieldKind::Month),
                'd' => Some(FieldKind::Day),
                'h' => Some(FieldKind::Hour { min: 1, max: 12 }),
                'K' => Some(FieldKind::Hour { min: 0, max: 11 }),
                'H' => Some(FieldKind::Hour { min: 0, max: 23 }),
                'k' => Some(FieldKind::Hour { min: 1, max: 24 }),
                'm' => Some(FieldKind::Minute),
                'a' | 'b' | 'B' => Some(FieldKind::Meridiem),
                _ => None,
            };
            if let Some(f) = field {
                flush(&mut literal, &mut parts);
                parts.push(Part::Field(f));
            }
            continue;
        }
        literal.push(c);
        i += 1;
    }
    flush(&mut literal, &mut parts);
    // A dropped field leaves its separator behind (`h:mm:ss a` without seconds).
    while let Some(Part::Literal(_)) = parts.last() {
        if parts.len() > 1 && matches!(parts[parts.len() - 2], Part::Literal(_)) {
            parts.pop();
        } else {
            break;
        }
    }
    parts
}

/// One editable field.
#[derive(Clone, Debug)]
pub struct Field {
    pub kind: FieldKind,
    pub value: Option<i32>,
    placeholder: i32,
    buffer: String,
    last_digit: Option<Instant>,
}

impl Field {
    fn new(kind: FieldKind, today: (i32, u32, u32)) -> Self {
        let placeholder = match kind {
            FieldKind::Year => today.0,
            FieldKind::Month => today.1 as i32,
            FieldKind::Day => today.2 as i32,
            FieldKind::Hour { max, .. } => {
                if max >= 12 {
                    12
                } else {
                    11
                }
            }
            FieldKind::Minute => 30,
            FieldKind::Meridiem => 1,
        };
        Self {
            kind,
            value: None,
            placeholder,
            buffer: String::new(),
            last_digit: None,
        }
    }

    /// The field's text: its value, or its placeholder (drawn in the placeholder color).
    pub fn text(&self, meridiem: &[String; 2]) -> String {
        let v = self.value.unwrap_or(self.placeholder);
        match self.kind {
            FieldKind::Meridiem => meridiem[v.clamp(0, 1) as usize].clone(),
            k => format!("{:0width$}", v, width = k.digits()),
        }
    }

    fn set(&mut self, v: i32) {
        let (min, max) = self.kind.range();
        self.value = Some(v.clamp(min, max));
    }

    /// `stepUp`: the next value, wrapping; an empty field starts at its minimum (a year at the
    /// current year).
    pub fn step_up(&mut self, today: (i32, u32, u32)) {
        self.buffer.clear();
        let (min, max) = self.kind.range();
        let next = match self.value {
            None if self.kind == FieldKind::Year => today.0,
            None => min,
            Some(v) if v + 1 > max => min,
            Some(v) => v + 1,
        };
        self.set(next);
    }

    /// `stepDown`: the previous value, wrapping; an empty field starts at its maximum (a year
    /// at the current year).
    pub fn step_down(&mut self, today: (i32, u32, u32)) {
        self.buffer.clear();
        let (min, max) = self.kind.range();
        let next = match self.value {
            None if self.kind == FieldKind::Year => today.0,
            None => max,
            Some(v) if v - 1 < min => max,
            Some(v) => v - 1,
        };
        self.set(next);
    }

    /// Backspace or Delete.
    pub fn clear(&mut self) {
        self.value = None;
        self.buffer.clear();
    }

    /// The field lost focus: the next digit starts a new number.
    pub fn blur(&mut self) {
        self.buffer.clear();
    }

    /// A typed character. Numeric fields append digits to a type-ahead buffer (restarted after
    /// a second, or once it holds the maximum's digit count) and take its value, clamped; the
    /// AM/PM field picks the label the character starts. True when it was consumed.
    pub fn type_char(&mut self, c: char, now: Instant, meridiem: &[String; 2]) -> bool {
        if self.kind == FieldKind::Meridiem {
            let lower = c.to_lowercase().next().unwrap_or(c);
            if let Some(i) = meridiem
                .iter()
                .position(|m| m.to_lowercase().starts_with(lower))
            {
                self.value = Some(i as i32);
            }
            return true;
        }
        let Some(d) = c.to_digit(10) else {
            return false;
        };
        let stale = self
            .last_digit
            .is_none_or(|t| now.saturating_duration_since(t) > TYPE_AHEAD_TIMEOUT);
        self.last_digit = Some(now);
        let max_len = self
            .kind
            .range()
            .1
            .to_string()
            .len()
            .max(self.kind.digits());
        if stale || self.buffer.len() == max_len {
            self.buffer.clear();
        }
        self.buffer.push(char::from_digit(d, 10).unwrap_or('0'));
        let v = self.buffer.parse::<i64>().unwrap_or(0);
        self.set(v.min(i64::from(i32::MAX)) as i32);
        true
    }
}

/// The fields of one input and the literals between them.
#[derive(Clone, Debug)]
pub struct Editor {
    pub kind: Kind,
    pub parts: Vec<Part>,
    pub fields: Vec<Field>,
    pub meridiem: [String; 2],
    today: (i32, u32, u32),
}

impl Editor {
    /// `pattern` is the locale's short date or time format; `today` gives the date
    /// placeholders and an empty year's first step.
    pub fn new(kind: Kind, pattern: &str, meridiem: [String; 2], today: (i32, u32, u32)) -> Self {
        let parts = parse_pattern(pattern);
        let fields = parts
            .iter()
            .filter_map(|p| match p {
                Part::Field(k) => Some(Field::new(*k, today)),
                Part::Literal(_) => None,
            })
            .collect();
        Self {
            kind,
            parts,
            fields,
            meridiem,
            today,
        }
    }

    pub fn step(&mut self, field: usize, up: bool) {
        let today = self.today;
        if let Some(f) = self.fields.get_mut(field) {
            if up {
                f.step_up(today);
            } else {
                f.step_down(today);
            }
        }
    }

    pub fn type_char(&mut self, field: usize, c: char, now: Instant) -> bool {
        let meridiem = self.meridiem.clone();
        self.fields
            .get_mut(field)
            .is_some_and(|f| f.type_char(c, now, &meridiem))
    }

    fn get(&self, kind: impl Fn(FieldKind) -> bool) -> Option<&Field> {
        self.fields.iter().find(|f| kind(f.kind))
    }

    /// The input's value: `YYYY-MM-DD` or `HH:MM`, or empty while any field is empty or the
    /// date does not exist (the value sanitizer's rule).
    pub fn value(&self) -> String {
        match self.kind {
            Kind::Date => {
                let year = self.get(|k| k == FieldKind::Year).and_then(|f| f.value);
                let month = self.get(|k| k == FieldKind::Month).and_then(|f| f.value);
                let day = self.get(|k| k == FieldKind::Day).and_then(|f| f.value);
                match (year, month, day) {
                    (Some(y), Some(m), Some(d)) if d <= days_in_month(y, m as u32) as i32 => {
                        format!("{y:04}-{m:02}-{d:02}")
                    }
                    _ => String::new(),
                }
            }
            Kind::Time => {
                let Some(hour) = self.get(|k| matches!(k, FieldKind::Hour { .. })) else {
                    return String::new();
                };
                let minute = self.get(|k| k == FieldKind::Minute).and_then(|f| f.value);
                let meridiem = self.get(|k| k == FieldKind::Meridiem);
                let (Some(h), Some(m)) = (hour.value, minute) else {
                    return String::new();
                };
                let h24 = match (hour.kind, meridiem) {
                    (
                        FieldKind::Hour { max: 12, .. } | FieldKind::Hour { max: 11, .. },
                        Some(mf),
                    ) => {
                        let Some(pm) = mf.value else {
                            return String::new();
                        };
                        h % 12 + 12 * pm
                    }
                    (FieldKind::Hour { max: 24, .. }, _) => h % 24,
                    _ => h,
                };
                format!("{h24:02}:{m:02}")
            }
        }
    }

    /// `setValue` from the page: fills every field from a valid value, empties them otherwise.
    pub fn set_value(&mut self, value: &str) {
        for f in &mut self.fields {
            f.clear();
        }
        match self.kind {
            Kind::Date => {
                let Some((y, m, d)) = parse_date(value) else {
                    return;
                };
                for f in &mut self.fields {
                    f.value = match f.kind {
                        FieldKind::Year => Some(y),
                        FieldKind::Month => Some(m as i32),
                        FieldKind::Day => Some(d as i32),
                        _ => f.value,
                    };
                }
            }
            Kind::Time => {
                let Some((h, m)) = parse_time(value) else {
                    return;
                };
                for f in &mut self.fields {
                    f.value = match f.kind {
                        FieldKind::Hour { max: 12, .. } => {
                            Some(if h % 12 == 0 { 12 } else { h % 12 })
                        }
                        FieldKind::Hour { max: 11, .. } => Some(h % 12),
                        FieldKind::Hour { max: 24, .. } => Some(if h == 0 { 24 } else { h }),
                        FieldKind::Hour { .. } => Some(h),
                        FieldKind::Minute => Some(m),
                        FieldKind::Meridiem => Some(i32::from(h >= 12)),
                        _ => f.value,
                    };
                }
            }
        }
    }
}

/// `YYYY-MM-DD` (four or more year digits) as WebKit's date parser takes it.
pub fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let mut it = s.split('-');
    let (y, m, d) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() || y.len() < 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let (y, m, d): (i32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    ((MIN_YEAR..=MAX_YEAR).contains(&y)
        && (1..=12).contains(&m)
        && d >= 1
        && d <= days_in_month(y, m))
    .then_some((y, m, d))
}

/// `HH:MM` (seconds ignored).
pub fn parse_time(s: &str) -> Option<(i32, i32)> {
    let mut it = s.split(':');
    let (h, m) = (it.next()?, it.next()?);
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some((h, m))
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: (i32, u32, u32) = (2026, 10, 27);

    fn ampm() -> [String; 2] {
        ["AM".into(), "PM".into()]
    }

    fn date() -> Editor {
        Editor::new(Kind::Date, "M/d/yy", ampm(), TODAY)
    }

    fn time() -> Editor {
        Editor::new(Kind::Time, "h:mm\u{202f}a", ampm(), TODAY)
    }

    #[test]
    fn patterns_split_into_fields_and_literals() {
        assert_eq!(
            parse_pattern("M/d/yy"),
            vec![
                Part::Field(FieldKind::Month),
                Part::Literal("/".into()),
                Part::Field(FieldKind::Day),
                Part::Literal("/".into()),
                Part::Field(FieldKind::Year),
            ]
        );
        assert_eq!(
            parse_pattern("dd.MM.y 'г.'"),
            vec![
                Part::Field(FieldKind::Day),
                Part::Literal(".".into()),
                Part::Field(FieldKind::Month),
                Part::Literal(".".into()),
                Part::Field(FieldKind::Year),
                Part::Literal(" г.".into()),
            ]
        );
        assert_eq!(parse_pattern("HH:mm").len(), 3);
    }

    #[test]
    fn empty_fields_show_placeholders() {
        let d = date();
        let texts: Vec<String> = d.fields.iter().map(|f| f.text(&d.meridiem)).collect();
        assert_eq!(texts, ["10", "27", "2026"]);
        let t = time();
        let texts: Vec<String> = t.fields.iter().map(|f| f.text(&t.meridiem)).collect();
        assert_eq!(texts, ["12", "30", "PM"]);
        assert_eq!(d.value(), "");
    }

    /// The probe against Playwright's WebKit: `10/27/2026`, month ↑ ↑ ↑ ↓, day ↑ ×5, year ↑.
    #[test]
    fn arrows_step_and_wrap_like_webkit() {
        let mut d = date();
        d.set_value("2026-10-27");
        d.step(0, true);
        assert_eq!(d.value(), "2026-11-27");
        d.step(0, true);
        d.step(0, true);
        assert_eq!(d.value(), "2026-01-27");
        d.step(0, false);
        assert_eq!(d.value(), "2026-12-27");
        for _ in 0..5 {
            d.step(1, true);
        }
        assert_eq!(d.value(), "2026-12-01");
        d.step(2, true);
        assert_eq!(d.value(), "2027-12-01");

        let mut t = time();
        t.set_value("09:05");
        t.step(0, true);
        assert_eq!(t.value(), "10:05");
        t.step(0, false);
        t.step(0, false);
        assert_eq!(t.value(), "08:05");
        for _ in 0..9 {
            t.step(0, false);
        }
        assert_eq!(t.value(), "11:05", "12 wraps to 1 and back to 11, AM kept");
        t.step(1, false);
        t.step(2, true);
        assert_eq!(t.value(), "23:04");
    }

    #[test]
    fn empty_fields_step_from_their_ends() {
        let mut d = date();
        d.step(0, true);
        assert_eq!(d.fields[0].value, Some(1));
        d.step(1, false);
        assert_eq!(d.fields[1].value, Some(31));
        assert_eq!(d.value(), "", "the year is still empty");
        d.step(2, false);
        assert_eq!(
            d.fields[2].value,
            Some(2026),
            "a year starts at the current year"
        );
        assert_eq!(d.value(), "2026-01-31");
    }

    /// The same probe's typing: month `1`, `2`, `3`, `1` (no auto-advance; the buffer restarts
    /// when full and values clamp), year `1` then `2` after 1.2s.
    #[test]
    fn typing_fills_a_type_ahead_buffer() {
        let mut d = date();
        d.set_value("2027-12-01");
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        d.type_char(0, '1', ms(0));
        assert_eq!(d.value(), "2027-01-01");
        d.type_char(0, '2', ms(10));
        assert_eq!(d.value(), "2027-12-01");
        d.type_char(0, '3', ms(20));
        assert_eq!(d.value(), "2027-03-01");
        d.type_char(0, '1', ms(30));
        assert_eq!(d.value(), "2027-12-01", "31 clamps to 12");
        d.type_char(2, '1', ms(40));
        assert_eq!(d.value(), "0001-12-01");
        d.type_char(2, '2', ms(1240));
        assert_eq!(
            d.value(),
            "0002-12-01",
            "after the timeout a digit starts over"
        );
        assert!(!d.type_char(0, 'x', ms(1300)));

        let mut t = time();
        t.set_value("11:04");
        t.type_char(2, 'p', ms(0));
        assert_eq!(t.value(), "23:04");
        t.type_char(2, 'A', ms(10));
        assert_eq!(t.value(), "11:04");
        t.type_char(0, '7', ms(20));
        t.type_char(0, '4', ms(30));
        t.type_char(0, '5', ms(40));
        assert_eq!(
            t.value(),
            "05:04",
            "74 clamps to 12, then 125 restarts at 5"
        );
    }

    #[test]
    fn a_cleared_field_or_an_impossible_date_empties_the_value() {
        let mut d = date();
        d.set_value("2026-02-28");
        d.fields[1].clear();
        assert_eq!(d.value(), "");
        assert_eq!(d.fields[1].text(&d.meridiem), "27", "the placeholder shows");
        d.type_char(1, '3', Instant::now());
        d.type_char(1, '0', Instant::now());
        assert_eq!(d.value(), "", "February 30");
        d.set_value("not a date");
        assert!(d.fields.iter().all(|f| f.value.is_none()));
    }
}
