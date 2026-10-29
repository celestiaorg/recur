//! Pair a unified diff into side-by-side rows (GitHub-style: deletions
//! zip with the following additions).

use recur_domain::{DiffLine, PrComment};

use crate::html::js_int;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitCell {
    pub i: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitRow {
    Hunk {
        i: usize,
    },
    Pair {
        i: usize,
        left: Option<SplitCell>,
        right: Option<SplitCell>,
    },
}

impl SplitRow {
    /// The first line index of the row (its key).
    pub fn i(&self) -> usize {
        match *self {
            SplitRow::Hunk { i } | SplitRow::Pair { i, .. } => i,
        }
    }
}

pub fn split_diff_rows(lines: &[DiffLine]) -> Vec<SplitRow> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].r#type.as_str();
        if t == "hunk" {
            out.push(SplitRow::Hunk { i });
            i += 1;
            continue;
        }
        if t != "del" && t != "add" {
            let c = Some(SplitCell { i });
            out.push(SplitRow::Pair {
                i,
                left: c,
                right: c,
            });
            i += 1;
            continue;
        }
        let mut dels = Vec::new();
        let mut adds = Vec::new();
        while i < lines.len() && lines[i].r#type == "del" {
            dels.push(SplitCell { i });
            i += 1;
        }
        while i < lines.len() && lines[i].r#type == "add" {
            adds.push(SplitCell { i });
            i += 1;
        }
        for k in 0..dels.len().max(adds.len()) {
            let left = dels.get(k).copied();
            let right = adds.get(k).copied();
            let first = left.or(right).map_or(0, |c| c.i);
            out.push(SplitRow::Pair {
                i: first,
                left,
                right,
            });
        }
    }
    out
}

/// The row's `data-line`: the right (new) number, else the left.
pub fn split_row_data_line(lines: &[DiffLine], row: &SplitRow) -> i64 {
    let SplitRow::Pair { left, right, .. } = row else {
        return 0;
    };
    let Some(cell) = right.or(*left) else {
        return 0;
    };
    let l = &lines[cell.i];
    js_int(if l.new.is_empty() { &l.old } else { &l.new })
}

fn thread_on(threads: &[&PrComment], n: i64) -> Option<bool> {
    threads
        .iter()
        .find(|t| t.line == n)
        .map(|t| side_or_right(&t.side).eq_ignore_ascii_case("LEFT"))
}

fn side_or_right(s: &str) -> &str {
    if s.is_empty() { "RIGHT" } else { s }
}

/// A posted thread on this file jumps to its unified row, not the first matching number.
pub fn slot_of_unified_highlight(
    lines: &[DiffLine],
    slots: &[usize],
    n: i64,
    threads: &[&PrComment],
) -> isize {
    if n == 0 {
        return -1;
    }
    if let Some(left) = thread_on(threads, n) {
        for (s, &li) in slots.iter().enumerate() {
            let Some(l) = lines.get(li) else { continue };
            if l.r#type == "hunk" {
                continue;
            }
            if js_int(if left { &l.old } else { &l.new }) == n {
                return s as isize;
            }
        }
    }
    for (s, &li) in slots.iter().enumerate() {
        if let Some(l) = lines.get(li)
            && l.r#type != "hunk"
            && js_int(if l.new.is_empty() { &l.old } else { &l.new }) == n
        {
            return s as isize;
        }
    }
    -1
}

/// A posted thread on this file jumps to its split cell, not the row's `data-line`.
pub fn slot_of_split_highlight(
    lines: &[DiffLine],
    slots: &[SplitRow],
    n: i64,
    threads: &[&PrComment],
) -> isize {
    if n == 0 {
        return -1;
    }
    if let Some(left) = thread_on(threads, n) {
        for (s, row) in slots.iter().enumerate() {
            let SplitRow::Pair {
                left: l, right: r, ..
            } = row
            else {
                continue;
            };
            let raw = match (left, l, r) {
                (true, Some(c), _) => lines[c.i].old.as_str(),
                (false, _, Some(c)) => lines[c.i].new.as_str(),
                _ => "",
            };
            if js_int(raw) == n {
                return s as isize;
            }
        }
    }
    slots
        .iter()
        .position(|row| split_row_data_line(lines, row) == n)
        .map_or(-1, |s| s as isize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_window::{DIFF_LINE_OVERSCAN, window_around};

    fn l(t: &str, old: &str, new: &str, html: &str) -> DiffLine {
        DiffLine {
            r#type: t.into(),
            old: old.into(),
            new: new.into(),
            code_html: html.into(),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_hunk_headers_and_context_on_both_sides() {
        let rows = split_diff_rows(&[l("hunk", "", "", "@@"), l("ctx", "1", "1", "same")]);
        let c = Some(SplitCell { i: 1 });
        assert_eq!(
            rows,
            vec![
                SplitRow::Hunk { i: 0 },
                SplitRow::Pair {
                    i: 1,
                    left: c,
                    right: c
                }
            ]
        );
    }

    #[test]
    fn zips_a_delete_with_the_following_add() {
        let rows = split_diff_rows(&[l("del", "4", "", "old"), l("add", "", "4", "new")]);
        assert_eq!(
            rows,
            vec![SplitRow::Pair {
                i: 0,
                left: Some(SplitCell { i: 0 }),
                right: Some(SplitCell { i: 1 })
            }]
        );
    }

    #[test]
    fn pads_the_shorter_side_when_dels_and_adds_differ_in_count() {
        let rows = split_diff_rows(&[
            l("del", "1", "", "a"),
            l("del", "2", "", "b"),
            l("add", "", "1", "c"),
        ]);
        assert_eq!(
            rows,
            vec![
                SplitRow::Pair {
                    i: 0,
                    left: Some(SplitCell { i: 0 }),
                    right: Some(SplitCell { i: 2 })
                },
                SplitRow::Pair {
                    i: 1,
                    left: Some(SplitCell { i: 1 }),
                    right: None
                },
            ]
        );
    }

    fn replacement(n: usize) -> Vec<DiffLine> {
        let mut lines = vec![l("hunk", "", "", &format!("@@ -701,{n} +501,{n} @@"))];
        for i in 0..n {
            lines.push(l("del", &(701 + i).to_string(), "", "old"));
            lines.push(l("add", "", &(501 + i).to_string(), "new"));
        }
        lines
    }

    fn thread(line: i64, side: &str) -> PrComment {
        PrComment {
            line,
            side: side.into(),
            ..Default::default()
        }
    }

    #[test]
    fn split_mounts_the_data_line_row_when_old_and_new_numbers_diverge() {
        let lines = replacement(400);
        let slots = split_diff_rows(&lines);
        assert_eq!(split_row_data_line(&lines, &slots[100]), 600);
        assert_eq!(split_row_data_line(&lines, &slots[300]), 800);
        let slot = slot_of_split_highlight(&lines, &slots, 800, &[]);
        assert_eq!(slot, 300);
        let win = window_around(slot, slots.len(), DIFF_LINE_OVERSCAN);
        assert!(slot as usize >= win.start && (slot as usize) < win.end);
        assert!(100 < win.start);
    }

    #[test]
    fn split_mounts_a_left_thread_when_old_and_new_numbers_diverge() {
        let lines = replacement(400);
        let slots = split_diff_rows(&lines);
        assert_eq!(split_row_data_line(&lines, &slots[150]), 650);
        assert_eq!(slot_of_split_highlight(&lines, &slots, 850, &[]), 350);
        let t = thread(850, "LEFT");
        let slot = slot_of_split_highlight(&lines, &slots, 850, &[&t]);
        assert_eq!(slot, 150);
        let win = window_around(slot, slots.len(), DIFF_LINE_OVERSCAN);
        assert!(slot as usize >= win.start && (slot as usize) < win.end);
        assert!(350 >= win.end);
    }

    fn replacement_unified(n: usize) -> Vec<DiffLine> {
        let mut lines = vec![l("hunk", "", "", &format!("@@ -1,{n} +1,{n} @@"))];
        for i in 1..=n {
            lines.push(l("del", &i.to_string(), "", "old"));
        }
        for i in 1..=n {
            lines.push(l("add", "", &i.to_string(), "new"));
        }
        lines
    }

    #[test]
    fn unified_mounts_a_right_thread_when_old_and_new_numbers_overlap() {
        let lines = replacement_unified(200);
        let slots: Vec<usize> = (0..lines.len()).collect();
        assert_eq!(slot_of_unified_highlight(&lines, &slots, 150, &[]), 150);
        let t = thread(150, "RIGHT");
        let slot = slot_of_unified_highlight(&lines, &slots, 150, &[&t]);
        assert_eq!(slot, 350);
        let win = window_around(slot, slots.len(), DIFF_LINE_OVERSCAN);
        assert!(slot as usize >= win.start && (slot as usize) < win.end);
        assert!(150 < win.start);
    }

    #[test]
    fn unified_mounts_a_left_thread_when_old_and_new_numbers_overlap() {
        let lines = replacement_unified(200);
        let slots: Vec<usize> = (0..lines.len()).collect();
        let t = thread(150, "LEFT");
        let slot = slot_of_unified_highlight(&lines, &slots, 150, &[&t]);
        assert_eq!(slot, 150);
        let win = window_around(slot, slots.len(), DIFF_LINE_OVERSCAN);
        assert!(slot as usize >= win.start && (slot as usize) < win.end);
        assert!(350 >= win.end);
    }
}
