//! Document changes -> line splices for the tokenizer document
//! (`cm-tm-delta.ts`) and the cheap document identity (`cm-tm-doc.ts`).

use std::collections::HashMap;

/// A 1-based line splice (same index in the old and new documents).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineReplace {
    pub from_line: usize,
    pub old_line_count: usize,
    pub new_lines: Vec<String>,
}

/// Line access for a document snapshot. Offsets are byte offsets; lines are
/// 1-based and exclude their newline.
pub trait LineDoc {
    fn len(&self) -> usize;
    fn line_count(&self) -> usize;
    fn line_number_at(&self, offset: usize) -> usize;
    fn line_text(&self, n: usize) -> String;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One changed range: `[from_a, to_a)` in the old document became
/// `[from_b, to_b)` in the new one.
#[derive(Clone, Copy, Debug)]
pub struct ChangedRange {
    pub from_a: usize,
    pub to_a: usize,
    pub from_b: usize,
    pub to_b: usize,
}

/// Narrowed line replace covering the lines whose text differs, or `None`.
pub fn line_replace_from_changes(
    old: &dyn LineDoc,
    new: &dyn LineDoc,
    changes: &[ChangedRange],
) -> Option<LineReplace> {
    if changes.is_empty() {
        return None;
    }
    let from_a = changes.iter().map(|c| c.from_a).min()?;
    let to_a = changes.iter().map(|c| c.to_a).max()?;
    let from_b = changes.iter().map(|c| c.from_b).min()?;
    let to_b = changes.iter().map(|c| c.to_b).max()?;

    let mut old_from = old.line_number_at(from_a.min(old.len()));
    let mut old_to = old.line_number_at(to_a.max(from_a).min(old.len()));
    let mut new_from = new.line_number_at(from_b.min(new.len()));
    let mut new_to = new.line_number_at(to_b.max(from_b).min(new.len()));

    while old_from <= old_to
        && new_from <= new_to
        && old.line_text(old_from) == new.line_text(new_from)
    {
        old_from += 1;
        new_from += 1;
    }
    while old_to >= old_from && new_to >= new_from && old.line_text(old_to) == new.line_text(new_to)
    {
        old_to -= 1;
        new_to -= 1;
    }
    if old_to < old_from && new_to < new_from {
        return None;
    }
    Some(LineReplace {
        from_line: old_from,
        old_line_count: (old_to + 1).saturating_sub(old_from),
        new_lines: (new_from..=new_to).map(|n| new.line_text(n)).collect(),
    })
}

/// Shift a line -> value map through a line splice (1-based keys).
pub fn splice_line_map<T>(
    map: HashMap<usize, T>,
    from_line: usize,
    old_count: usize,
    new_count: usize,
) -> HashMap<usize, T> {
    let old_end = from_line + old_count;
    map.into_iter()
        .filter_map(|(n, v)| {
            if n < from_line {
                Some((n, v))
            } else if n >= old_end {
                Some((n + new_count - old_count, v))
            } else {
                None
            }
        })
        .collect()
}

/// Line count, total length (lines joined with `\n`) and a sampled hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TmDocFingerprint {
    pub lines: usize,
    pub length: usize,
    pub sample: u32,
}

const FP_SAMPLES: usize = 32;

/// FNV-1a over up to 32 evenly spaced lines plus the last line.
pub fn tm_doc_fingerprint(
    line_at: impl Fn(usize) -> String,
    line_count: usize,
    length: usize,
) -> TmDocFingerprint {
    let mut h: u32 = 0x811c_9dc5;
    let mut mix = |s: &str| {
        for unit in s.encode_utf16() {
            h ^= u32::from(unit);
            h = h.wrapping_mul(0x0100_0193);
        }
        h ^= 0x0a;
        h = h.wrapping_mul(0x0100_0193);
    };
    let step = line_count.div_ceil(FP_SAMPLES).max(1);
    let mut n = 1;
    while n <= line_count {
        mix(&line_at(n));
        n += step;
    }
    if line_count > 1 && !(line_count - 1).is_multiple_of(step) {
        mix(&line_at(line_count));
    }
    TmDocFingerprint {
        lines: line_count,
        length,
        sample: h,
    }
}

/// Engine document id for a pane + path; panes never share one.
pub fn tm_doc_id_for(pane: &str, path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("{pane} {path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Doc(String);

    impl LineDoc for Doc {
        fn len(&self) -> usize {
            self.0.len()
        }
        fn line_count(&self) -> usize {
            self.0.split('\n').count()
        }
        fn line_number_at(&self, offset: usize) -> usize {
            self.0[..offset].matches('\n').count() + 1
        }
        fn line_text(&self, n: usize) -> String {
            self.0.split('\n').nth(n - 1).unwrap_or("").to_string()
        }
    }

    fn change(old: &str, from: usize, to: usize, insert: &str) -> (Doc, Doc, Vec<ChangedRange>) {
        let new = format!("{}{}{}", &old[..from], insert, &old[to..]);
        let r = ChangedRange {
            from_a: from,
            to_a: to,
            from_b: from,
            to_b: from + insert.len(),
        };
        (Doc(old.into()), Doc(new), vec![r])
    }

    fn lr(from_line: usize, old_line_count: usize, new_lines: &[&str]) -> Option<LineReplace> {
        Some(LineReplace {
            from_line,
            old_line_count,
            new_lines: new_lines.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn replaces_a_single_edited_line() {
        let (a, b, c) = change("a\nb\nc", 2, 3, "X");
        assert_eq!(line_replace_from_changes(&a, &b, &c), lr(2, 1, &["X"]));
    }

    #[test]
    fn splits_a_line_when_a_newline_is_inserted() {
        let (a, b, c) = change("hello", 2, 2, "\n");
        assert_eq!(
            line_replace_from_changes(&a, &b, &c),
            lr(1, 1, &["he", "llo"])
        );
    }

    #[test]
    fn joins_lines_when_a_newline_is_deleted() {
        let (a, b, c) = change("ab\ncd", 2, 3, "");
        assert_eq!(line_replace_from_changes(&a, &b, &c), lr(1, 2, &["abcd"]));
    }

    #[test]
    fn narrows_a_whole_document_replace() {
        let (a, b, c) = change("a\nb\nc\nd\ne", 0, 9, "a\nB\nc\nx\nd\ne");
        assert_eq!(
            line_replace_from_changes(&a, &b, &c),
            lr(2, 2, &["B", "c", "x"])
        );
    }

    #[test]
    fn identical_replace_and_no_change_give_none() {
        let (a, b, c) = change("a\nb", 0, 3, "a\nb");
        assert_eq!(line_replace_from_changes(&a, &b, &c), None);
        assert_eq!(
            line_replace_from_changes(&Doc("hello".into()), &Doc("hello".into()), &[]),
            None
        );
    }

    #[test]
    fn splice_line_map_drops_replaced_keys_and_shifts_the_tail() {
        let map = HashMap::from([(1, 'a'), (2, 'b'), (3, 'c'), (4, 'd')]);
        let mut got: Vec<_> = splice_line_map(map, 2, 1, 2).into_iter().collect();
        got.sort();
        assert_eq!(got, vec![(1, 'a'), (4, 'c'), (5, 'd')]);
    }

    fn fp(text: &str) -> TmDocFingerprint {
        let lines: Vec<&str> = text.split('\n').collect();
        tm_doc_fingerprint(|n| lines[n - 1].to_string(), lines.len(), text.len())
    }

    #[test]
    fn fingerprint_is_stable_and_moves_on_sampled_changes() {
        let text = "const a = 1\nconst b = 2\nconst c = 3";
        assert_eq!(fp(text), fp(text));
        assert_ne!(fp(text).sample, fp(&text.replacen('a', "x", 1)).sample);
        assert_ne!(fp("ab\n").sample, fp("a\nb").sample);
    }

    #[test]
    fn panes_get_separate_doc_ids() {
        assert_ne!(
            tm_doc_id_for("main", "src/a.ts"),
            tm_doc_id_for("split", "src/a.ts")
        );
        assert_eq!(tm_doc_id_for("main", ""), "");
    }
}
