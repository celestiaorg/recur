//! Change sets, a port of CodeMirror's `ChangeDesc` / `ChangeSet`.
//!
//! Sections are `(len, ins)` pairs: `len` bytes of the old document, then
//! `ins == -1` for "unchanged" or the byte length of the replacement.

use crate::text::{Text, normalize_newlines};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapMode {
    Simple,
    TrackDel,
    TrackBefore,
    TrackAfter,
}

/// One requested edit in the coordinates of the document it applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub from: usize,
    pub to: usize,
    pub insert: String,
}

impl Change {
    pub fn new(from: usize, to: usize, insert: impl Into<String>) -> Change {
        Change {
            from,
            to,
            insert: insert.into(),
        }
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Change {
        Change::new(at, at, text)
    }

    pub fn delete(from: usize, to: usize) -> Change {
        Change::new(from, to, "")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeDesc {
    pub(crate) sections: Vec<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    pub(crate) sections: Vec<i64>,
    pub(crate) inserted: Vec<String>,
}

fn add_section(sections: &mut Vec<i64>, len: i64, ins: i64, force_join: bool) {
    if len == 0 && ins <= 0 {
        return;
    }
    let n = sections.len();
    if n >= 2 && ins <= 0 && ins == sections[n - 1] {
        sections[n - 2] += len;
    } else if n >= 2 && len == 0 && sections[n - 2] == 0 {
        sections[n - 1] += ins;
    } else if force_join && n >= 2 {
        sections[n - 2] += len;
        sections[n - 1] += ins;
    } else {
        sections.push(len);
        sections.push(ins);
    }
}

fn add_insert(values: &mut Vec<String>, sections: &[i64], value: &str) {
    if value.is_empty() {
        return;
    }
    let index = (sections.len() - 2) >> 1;
    if index < values.len() {
        if let Some(last) = values.last_mut() {
            last.push_str(value);
        }
    } else {
        while values.len() < index {
            values.push(String::new());
        }
        values.push(value.to_string());
    }
}

trait Sections {
    fn sections(&self) -> &[i64];
    fn inserted(&self) -> Option<&[String]>;
}

impl Sections for ChangeDesc {
    fn sections(&self) -> &[i64] {
        &self.sections
    }
    fn inserted(&self) -> Option<&[String]> {
        None
    }
}

impl Sections for ChangeSet {
    fn sections(&self) -> &[i64] {
        &self.sections
    }
    fn inserted(&self) -> Option<&[String]> {
        Some(&self.inserted)
    }
}

fn length_of(sections: &[i64]) -> usize {
    sections.chunks(2).map(|c| c[0] as usize).sum()
}

fn new_length_of(sections: &[i64]) -> usize {
    sections
        .chunks(2)
        .map(|c| {
            if c[1] < 0 {
                c[0] as usize
            } else {
                c[1] as usize
            }
        })
        .sum()
}

fn is_empty(sections: &[i64]) -> bool {
    sections.is_empty() || (sections.len() == 2 && sections[1] < 0)
}

fn map_pos(sections: &[i64], pos: usize, assoc: i32, mode: MapMode) -> Option<usize> {
    let pos = pos as i64;
    let (mut pos_a, mut pos_b) = (0i64, 0i64);
    let mut i = 0;
    while i < sections.len() {
        let len = sections[i];
        let ins = sections[i + 1];
        i += 2;
        let end_a = pos_a + len;
        if ins < 0 {
            if end_a > pos {
                return Some((pos_b + (pos - pos_a)) as usize);
            }
            pos_b += len;
        } else {
            if mode != MapMode::Simple
                && end_a >= pos
                && ((mode == MapMode::TrackDel && pos_a < pos && end_a > pos)
                    || (mode == MapMode::TrackBefore && pos_a < pos)
                    || (mode == MapMode::TrackAfter && end_a > pos))
            {
                return None;
            }
            if end_a > pos || (end_a == pos && assoc < 0 && len == 0) {
                return Some(if pos == pos_a || assoc < 0 {
                    pos_b
                } else {
                    pos_b + ins
                } as usize);
            }
            pos_b += ins;
        }
        pos_a = end_a;
    }
    // Positions past the end clamp to the end (CodeMirror throws here).
    Some(pos_b as usize)
}

fn touches_range(sections: &[i64], from: usize, to: usize) -> Touch {
    let (from, to) = (from as i64, to as i64);
    let mut pos = 0i64;
    let mut i = 0;
    while i < sections.len() && pos <= to {
        let len = sections[i];
        let ins = sections[i + 1];
        i += 2;
        let end = pos + len;
        if ins >= 0 && pos <= to && end >= from {
            return if pos < from && end > to {
                Touch::Cover
            } else {
                Touch::Yes
            };
        }
        pos = end;
    }
    Touch::No
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Touch {
    No,
    Yes,
    Cover,
}

/// `(from_a, to_a, from_b, to_b, inserted text)` per changed range.
fn iter_changes(
    set: &dyn Sections,
    individual: bool,
    mut f: impl FnMut(usize, usize, usize, usize, &str),
) {
    let sections = set.sections();
    let inserted = set.inserted();
    let (mut pos_a, mut pos_b) = (0usize, 0usize);
    let mut i = 0;
    while i < sections.len() {
        let mut len = sections[i];
        let mut ins = sections[i + 1];
        i += 2;
        if ins < 0 {
            pos_a += len as usize;
            pos_b += len as usize;
            continue;
        }
        let (mut end_a, mut end_b) = (pos_a, pos_b);
        let mut text = String::new();
        loop {
            end_a += len as usize;
            end_b += ins as usize;
            if ins != 0
                && let Some(ins_texts) = inserted
                && let Some(t) = ins_texts.get((i - 2) >> 1)
            {
                text.push_str(t);
            }
            if individual || i == sections.len() || sections[i + 1] < 0 {
                break;
            }
            len = sections[i];
            ins = sections[i + 1];
            i += 2;
        }
        f(pos_a, end_a, pos_b, end_b, &text);
        pos_a = end_a;
        pos_b = end_b;
    }
}

struct SectionIter<'a> {
    sections: &'a [i64],
    inserted: Option<&'a [String]>,
    i: usize,
    len: i64,
    ins: i64,
    off: i64,
}

impl<'a> SectionIter<'a> {
    fn new(set: &'a dyn Sections) -> Self {
        let mut s = SectionIter {
            sections: set.sections(),
            inserted: set.inserted(),
            i: 0,
            len: 0,
            ins: 0,
            off: 0,
        };
        s.next();
        s
    }

    fn next(&mut self) {
        if self.i < self.sections.len() {
            self.len = self.sections[self.i];
            self.ins = self.sections[self.i + 1];
            self.i += 2;
        } else {
            self.len = 0;
            self.ins = -2;
        }
        self.off = 0;
    }

    fn done(&self) -> bool {
        self.ins == -2
    }

    fn len2(&self) -> i64 {
        if self.ins < 0 { self.len } else { self.ins }
    }

    fn text(&self) -> &'a str {
        let index = (self.i - 2) >> 1;
        self.inserted
            .and_then(|v| v.get(index))
            .map_or("", String::as_str)
    }

    fn text_bit(&self, len: Option<i64>) -> &'a str {
        let t = self.text();
        let start = self.off as usize;
        match len {
            Some(l) => t.get(start..start + l as usize).unwrap_or(""),
            None => t.get(start..).unwrap_or(""),
        }
    }

    fn forward(&mut self, len: i64) {
        if len == self.len {
            self.next();
        } else {
            self.len -= len;
            self.off += len;
        }
    }

    fn forward2(&mut self, len: i64) {
        if self.ins == -1 {
            self.forward(len);
        } else if len == self.ins {
            self.next();
        } else {
            self.ins -= len;
            self.off += len;
        }
    }
}

fn map_set(
    set_a: &dyn Sections,
    set_b: &dyn Sections,
    before: bool,
    mk_set: bool,
) -> (Vec<i64>, Vec<String>) {
    let mut sections = Vec::new();
    let mut insert: Vec<String> = Vec::new();
    let mut a = SectionIter::new(set_a);
    let mut b = SectionIter::new(set_b);
    let mut inserted: i64 = -1;
    loop {
        let a_i = a.i as i64;
        if (a.done() && b.len != 0) || (b.done() && a.len != 0) {
            // Mismatched lengths: stop with what was produced.
            return (sections, insert);
        } else if a.ins == -1 && b.ins == -1 {
            let len = a.len.min(b.len);
            add_section(&mut sections, len, -1, false);
            a.forward(len);
            b.forward(len);
        } else if b.ins >= 0
            && (a.ins < 0
                || inserted == a_i
                || (a.off == 0 && (b.len < a.len || (b.len == a.len && !before))))
        {
            let mut len = b.len;
            add_section(&mut sections, b.ins, -1, false);
            while len != 0 {
                let piece = a.len.min(len);
                if a.ins >= 0 && inserted < a.i as i64 && a.len <= piece {
                    add_section(&mut sections, 0, a.ins, false);
                    if mk_set {
                        let t = a.text().to_string();
                        add_insert(&mut insert, &sections, &t);
                    }
                    inserted = a.i as i64;
                }
                a.forward(piece);
                len -= piece;
            }
            b.next();
        } else if a.ins >= 0 {
            let mut len = 0;
            let mut left = a.len;
            while left != 0 {
                if b.ins == -1 {
                    let piece = left.min(b.len);
                    len += piece;
                    left -= piece;
                    b.forward(piece);
                } else if b.ins == 0 && b.len < left {
                    left -= b.len;
                    b.next();
                } else {
                    break;
                }
            }
            let fresh = inserted < a.i as i64;
            add_section(&mut sections, len, if fresh { a.ins } else { 0 }, false);
            if mk_set && fresh {
                let t = a.text().to_string();
                add_insert(&mut insert, &sections, &t);
            }
            inserted = a.i as i64;
            a.forward(a.len - left);
        } else {
            return (sections, insert);
        }
    }
}

fn compose_sets(
    set_a: &dyn Sections,
    set_b: &dyn Sections,
    mk_set: bool,
) -> (Vec<i64>, Vec<String>) {
    let mut sections = Vec::new();
    let mut insert: Vec<String> = Vec::new();
    let mut a = SectionIter::new(set_a);
    let mut b = SectionIter::new(set_b);
    let mut open = false;
    loop {
        if a.done() && b.done() {
            return (sections, insert);
        } else if a.ins == 0 {
            add_section(&mut sections, a.len, 0, open);
            a.next();
        } else if b.len == 0 && !b.done() {
            add_section(&mut sections, 0, b.ins, open);
            if mk_set {
                let t = b.text().to_string();
                add_insert(&mut insert, &sections, &t);
            }
            b.next();
        } else if a.done() || b.done() {
            return (sections, insert);
        } else {
            let len = a.len2().min(b.len);
            let section_len = sections.len();
            if a.ins == -1 {
                let ins_b = if b.ins == -1 {
                    -1
                } else if b.off != 0 {
                    0
                } else {
                    b.ins
                };
                add_section(&mut sections, len, ins_b, open);
                if mk_set && ins_b != 0 && ins_b != -1 {
                    let t = b.text().to_string();
                    add_insert(&mut insert, &sections, &t);
                }
            } else if b.ins == -1 {
                add_section(&mut sections, if a.off != 0 { 0 } else { a.len }, len, open);
                if mk_set {
                    let t = a.text_bit(Some(len)).to_string();
                    add_insert(&mut insert, &sections, &t);
                }
            } else {
                add_section(
                    &mut sections,
                    if a.off != 0 { 0 } else { a.len },
                    if b.off != 0 { 0 } else { b.ins },
                    open,
                );
                if mk_set && b.off == 0 {
                    let t = b.text().to_string();
                    add_insert(&mut insert, &sections, &t);
                }
            }
            open = (a.ins > len || (b.ins >= 0 && b.len > len))
                && (open || sections.len() > section_len);
            a.forward2(len);
            b.forward(len);
        }
    }
}

impl ChangeDesc {
    pub fn len(&self) -> usize {
        length_of(&self.sections)
    }

    pub fn new_len(&self) -> usize {
        new_length_of(&self.sections)
    }

    pub fn is_empty(&self) -> bool {
        is_empty(&self.sections)
    }

    pub fn map_pos(&self, pos: usize, assoc: i32) -> usize {
        map_pos(&self.sections, pos, assoc, MapMode::Simple).unwrap_or(pos)
    }

    pub fn map_pos_mode(&self, pos: usize, assoc: i32, mode: MapMode) -> Option<usize> {
        map_pos(&self.sections, pos, assoc, mode)
    }

    pub fn touches_range(&self, from: usize, to: usize) -> Touch {
        touches_range(&self.sections, from, to)
    }

    pub fn iter_changed_ranges(
        &self,
        individual: bool,
        mut f: impl FnMut(usize, usize, usize, usize),
    ) {
        iter_changes(self, individual, |a, b, c, d, _| f(a, b, c, d));
    }

    pub fn iter_gaps(&self, mut f: impl FnMut(usize, usize, usize)) {
        let (mut pos_a, mut pos_b) = (0usize, 0usize);
        for c in self.sections.chunks(2) {
            let (len, ins) = (c[0] as usize, c[1]);
            if ins < 0 {
                f(pos_a, pos_b, len);
                pos_b += len;
            } else {
                pos_b += ins as usize;
            }
            pos_a += len;
        }
    }

    pub fn inverted_desc(&self) -> ChangeDesc {
        let mut sections = Vec::with_capacity(self.sections.len());
        for c in self.sections.chunks(2) {
            if c[1] < 0 {
                sections.extend_from_slice(&[c[0], c[1]]);
            } else {
                sections.extend_from_slice(&[c[1], c[0]]);
            }
        }
        ChangeDesc { sections }
    }

    pub fn compose_desc(&self, other: &ChangeDesc) -> ChangeDesc {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        ChangeDesc {
            sections: compose_sets(self, other, false).0,
        }
    }

    pub fn map_desc(&self, other: &ChangeDesc, before: bool) -> ChangeDesc {
        if other.is_empty() {
            return self.clone();
        }
        ChangeDesc {
            sections: map_set(self, other, before, false).0,
        }
    }

    pub fn to_json(&self) -> Vec<i64> {
        self.sections.clone()
    }

    pub fn from_json(sections: Vec<i64>) -> Option<ChangeDesc> {
        sections
            .len()
            .is_multiple_of(2)
            .then_some(ChangeDesc { sections })
    }
}

impl ChangeSet {
    /// No change to a document of `len` bytes.
    pub fn empty(len: usize) -> ChangeSet {
        ChangeSet {
            sections: if len > 0 {
                vec![len as i64, -1]
            } else {
                Vec::new()
            },
            inserted: Vec::new(),
        }
    }

    /// `ChangeSet.of` for a document of `len` bytes. Changes may be given in
    /// any order; each is in the coordinates of the original document.
    pub fn of(changes: &[Change], len: usize) -> ChangeSet {
        let mut sections: Vec<i64> = Vec::new();
        let mut inserted: Vec<String> = Vec::new();
        let mut pos = 0usize;
        let mut total: Option<ChangeSet> = None;
        let flush = |sections: &mut Vec<i64>,
                     inserted: &mut Vec<String>,
                     pos: &mut usize,
                     total: &mut Option<ChangeSet>,
                     force: bool| {
            if !force && sections.is_empty() {
                return;
            }
            if *pos < len {
                add_section(sections, (len - *pos) as i64, -1, false);
            }
            let set = ChangeSet {
                sections: std::mem::take(sections),
                inserted: std::mem::take(inserted),
            };
            *total = Some(match total.take() {
                Some(t) => {
                    let mapped = set.map(&t, false);
                    t.compose(&mapped)
                }
                None => set,
            });
            *pos = 0;
        };
        for spec in changes {
            let from = spec.from.min(len);
            let to = spec.to.clamp(from, len);
            let text = normalize_newlines(&spec.insert);
            if from == to && text.is_empty() {
                continue;
            }
            if from < pos {
                flush(&mut sections, &mut inserted, &mut pos, &mut total, false);
            }
            if from > pos {
                add_section(&mut sections, (from - pos) as i64, -1, false);
            }
            add_section(&mut sections, (to - from) as i64, text.len() as i64, false);
            add_insert(&mut inserted, &sections, &text);
            pos = to;
        }
        let force = total.is_none();
        flush(&mut sections, &mut inserted, &mut pos, &mut total, force);
        total.unwrap_or_else(|| ChangeSet::empty(len))
    }

    pub fn len(&self) -> usize {
        length_of(&self.sections)
    }

    pub fn new_len(&self) -> usize {
        new_length_of(&self.sections)
    }

    pub fn is_empty(&self) -> bool {
        is_empty(&self.sections)
    }

    pub fn desc(&self) -> ChangeDesc {
        ChangeDesc {
            sections: self.sections.clone(),
        }
    }

    pub fn apply(&self, doc: &Text) -> Text {
        let mut doc = doc.clone();
        iter_changes(self, false, |from_a, to_a, from_b, _to_b, text| {
            doc = doc.replace(from_b, from_b + (to_a - from_a), text);
        });
        doc
    }

    pub fn invert(&self, doc: &Text) -> ChangeSet {
        let mut sections = self.sections.clone();
        let mut inserted: Vec<String> = Vec::new();
        let mut pos = 0usize;
        let mut i = 0;
        while i < sections.len() {
            let len = sections[i];
            let ins = sections[i + 1];
            if ins >= 0 {
                sections[i] = ins;
                sections[i + 1] = len;
                let index = i >> 1;
                while inserted.len() < index {
                    inserted.push(String::new());
                }
                inserted.push(if len > 0 {
                    doc.slice(pos, pos + len as usize)
                } else {
                    String::new()
                });
            }
            pos += len as usize;
            i += 2;
        }
        ChangeSet { sections, inserted }
    }

    pub fn compose(&self, other: &ChangeSet) -> ChangeSet {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }
        let (sections, inserted) = compose_sets(self, other, true);
        ChangeSet { sections, inserted }
    }

    /// Map this set over `other` (both starting in the same document).
    pub fn map(&self, other: &ChangeSet, before: bool) -> ChangeSet {
        if other.is_empty() {
            return self.clone();
        }
        let (sections, inserted) = map_set(self, other, before, true);
        ChangeSet { sections, inserted }
    }

    pub fn map_desc(&self, other: &ChangeDesc, before: bool) -> ChangeDesc {
        if other.is_empty() {
            return self.desc();
        }
        ChangeDesc {
            sections: map_set(self, other, before, false).0,
        }
    }

    pub fn map_pos(&self, pos: usize, assoc: i32) -> usize {
        map_pos(&self.sections, pos, assoc, MapMode::Simple).unwrap_or(pos)
    }

    pub fn map_pos_mode(&self, pos: usize, assoc: i32, mode: MapMode) -> Option<usize> {
        map_pos(&self.sections, pos, assoc, mode)
    }

    pub fn touches_range(&self, from: usize, to: usize) -> Touch {
        touches_range(&self.sections, from, to)
    }

    /// `(from_a, to_a, from_b, to_b, inserted)` per change.
    pub fn iter_changes(&self, individual: bool, f: impl FnMut(usize, usize, usize, usize, &str)) {
        iter_changes(self, individual, f);
    }

    pub fn iter_changed_ranges(
        &self,
        individual: bool,
        mut f: impl FnMut(usize, usize, usize, usize),
    ) {
        iter_changes(self, individual, |a, b, c, d, _| f(a, b, c, d));
    }

    pub fn iter_gaps(&self, f: impl FnMut(usize, usize, usize)) {
        self.desc().iter_gaps(f);
    }

    pub fn inverted_desc(&self) -> ChangeDesc {
        self.desc().inverted_desc()
    }

    /// Changes as `(from, to, insert)` in the original document.
    pub fn changes(&self) -> Vec<Change> {
        let mut out = Vec::new();
        iter_changes(self, false, |from_a, to_a, _, _, text| {
            out.push(Change::new(from_a, to_a, text));
        });
        out
    }

    /// CodeMirror's JSON form: kept lengths as numbers, replacements as
    /// `[len, ...lines]`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut parts = Vec::new();
        for (i, c) in self.sections.chunks(2).enumerate() {
            let (len, ins) = (c[0], c[1]);
            if ins < 0 {
                parts.push(serde_json::json!(len));
            } else if ins == 0 {
                parts.push(serde_json::json!([len]));
            } else {
                let mut part = vec![serde_json::json!(len)];
                let text = self.inserted.get(i).map_or("", String::as_str);
                part.extend(text.split('\n').map(|l| serde_json::json!(l)));
                parts.push(serde_json::Value::Array(part));
            }
        }
        serde_json::Value::Array(parts)
    }

    pub fn from_json(json: &serde_json::Value) -> Option<ChangeSet> {
        let parts = json.as_array()?;
        let mut sections = Vec::new();
        let mut inserted: Vec<String> = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            if let Some(n) = part.as_i64() {
                sections.extend_from_slice(&[n, -1]);
                continue;
            }
            let arr = part.as_array()?;
            let len = arr.first()?.as_i64()?;
            if arr.len() == 1 {
                sections.extend_from_slice(&[len, 0]);
                continue;
            }
            let lines: Option<Vec<&str>> = arr[1..].iter().map(|v| v.as_str()).collect();
            let text = lines?.join("\n");
            while inserted.len() < i {
                inserted.push(String::new());
            }
            sections.extend_from_slice(&[len, text.len() as i64]);
            inserted.push(text);
        }
        Some(ChangeSet { sections, inserted })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(doc: &str, set: &ChangeSet) -> String {
        set.apply(&Text::of(doc)).to_string()
    }

    #[test]
    fn builds_and_applies_unsorted_changes() {
        let set = ChangeSet::of(&[Change::new(6, 11, "there"), Change::insert(0, ">")], 11);
        assert_eq!(apply("hello world", &set), ">hello there");
        assert_eq!(set.len(), 11);
        assert_eq!(set.new_len(), 12);
    }

    #[test]
    fn maps_positions_with_assoc() {
        let set = ChangeSet::of(&[Change::insert(2, "xx")], 5);
        assert_eq!(set.map_pos(2, -1), 2);
        assert_eq!(set.map_pos(2, 1), 4);
        assert_eq!(set.map_pos(4, -1), 6);
        let del = ChangeSet::of(&[Change::delete(1, 3)], 5);
        assert_eq!(del.map_pos(2, -1), 1);
        assert_eq!(del.map_pos_mode(2, -1, MapMode::TrackDel), None);
        assert_eq!(del.map_pos_mode(1, -1, MapMode::TrackDel), Some(1));
    }

    #[test]
    fn inverts() {
        let doc = Text::of("abcdef");
        let set = ChangeSet::of(&[Change::new(1, 3, "XYZ"), Change::delete(4, 5)], 6);
        let after = set.apply(&doc);
        assert_eq!(after.to_string(), "aXYZdf");
        assert_eq!(set.invert(&doc).apply(&after).to_string(), "abcdef");
    }

    #[test]
    fn composes() {
        let a = ChangeSet::of(&[Change::insert(0, "ab")], 3);
        let b = ChangeSet::of(&[Change::new(1, 3, "Q")], 5);
        let c = a.compose(&b);
        assert_eq!(apply("xyz", &c), "aQyz");
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn maps_concurrent_changes() {
        let a = ChangeSet::of(&[Change::insert(1, "A")], 4);
        let b = ChangeSet::of(&[Change::insert(3, "B")], 4);
        let ab = a.compose(&b.map(&a, false));
        let ba = b.compose(&a.map(&b, true));
        assert_eq!(apply("wxyz", &ab), "wAxyBz");
        assert_eq!(apply("wxyz", &ab), apply("wxyz", &ba));
    }

    #[test]
    fn overlapping_specs_compose_in_original_coordinates() {
        let set = ChangeSet::of(&[Change::insert(3, "b"), Change::insert(1, "a")], 4);
        assert_eq!(apply("wxyz", &set), "waxybz");
    }

    #[test]
    fn json_round_trip() {
        let set = ChangeSet::of(&[Change::new(1, 2, "a\nb"), Change::delete(3, 4)], 5);
        let back = ChangeSet::from_json(&set.to_json()).unwrap();
        assert_eq!(back, set);
        assert_eq!(apply("01234", &back), "0a\nb24");
    }

    #[test]
    fn iterates_changed_ranges() {
        let set = ChangeSet::of(&[Change::new(1, 2, "XY"), Change::insert(4, "Z")], 5);
        let mut got = Vec::new();
        set.iter_changes(false, |a, b, c, d, t| got.push((a, b, c, d, t.to_string())));
        assert_eq!(
            got,
            vec![(1, 2, 1, 3, "XY".into()), (4, 4, 5, 6, "Z".into())]
        );
        assert_eq!(set.touches_range(0, 1), Touch::Yes);
        assert_eq!(set.touches_range(2, 3), Touch::Yes);
        assert_eq!(set.touches_range(3, 3), Touch::No);
    }
}
