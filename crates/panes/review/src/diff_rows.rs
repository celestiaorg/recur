//! `DiffFile.svelte`'s row rules: line numbers per side, which rows carry notes, pending
//! comments and the draft range, hunk folding and the windowed slots.

use std::collections::HashSet;

use recur_domain::{DiffLine, PendingLineComment, PrComment};
use recur_engine::view::comment_thread::{DiffSide, line_notes_covering_row};

use crate::comment_hunk::comment_hunk_line_label;
use crate::html::js_int;
use crate::split_diff::SplitRow;

/// `rowSide`: deletions note on the left, everything else on the right.
pub fn row_side(line: &DiffLine) -> DiffSide {
    if line.r#type == "del" {
        DiffSide::Left
    } else {
        DiffSide::Right
    }
}

/// `rowNum`: the line number on `side`, 0 when the row has none.
pub fn row_num(line: &DiffLine, side: DiffSide) -> i64 {
    js_int(match side {
        DiffSide::Left => &line.old,
        DiffSide::Right => &line.new,
    })
}

/// `lineNums`: `(oldN, newN)`.
pub fn line_nums(line: &DiffLine) -> (i64, i64) {
    (js_int(&line.old), js_int(&line.new))
}

/// `lineNo`: the new number, else the old.
pub fn line_no(line: &DiffLine) -> i64 {
    js_int(if line.new.is_empty() {
        &line.old
    } else {
        &line.new
    })
}

/// `openLineNo`: the working-tree line for ⌘-click; deleted rows have none.
pub fn open_line_no(line: &DiffLine) -> i64 {
    js_int(&line.new)
}

/// `apiLine`: the signed line `AddPRComment` takes (negative on the left).
pub fn api_line(line: &DiffLine, side: DiffSide) -> i64 {
    let n = line_no(line);
    if side == DiffSide::Left { -n } else { n }
}

/// The composer's anchor row (`openLine`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenLine {
    pub old_n: i64,
    pub new_n: i64,
}

/// An in-progress drag over the `+` controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selecting {
    pub side: DiffSide,
    pub anchor: i64,
    pub tip: i64,
}

/// The highlighted draft range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraftSel {
    pub side: DiffSide,
    pub start: i64,
    pub end: i64,
}

/// `draftSel`: the drag in progress, else the open composer's range.
pub fn draft_sel(
    selecting: Option<Selecting>,
    open: Option<OpenLine>,
    open_side: DiffSide,
    open_start: i64,
) -> Option<DraftSel> {
    if let Some(s) = selecting {
        return Some(DraftSel {
            side: s.side,
            start: s.anchor.min(s.tip),
            end: s.anchor.max(s.tip),
        });
    }
    let open = open?;
    let end = if open_side == DiffSide::Left {
        open.old_n
    } else {
        open.new_n
    };
    if end == 0 {
        return None;
    }
    let start = if open_start > 0 && open_start < end {
        open_start
    } else {
        end
    };
    Some(DraftSel {
        side: open_side,
        start,
        end,
    })
}

pub fn draft_covers(sel: Option<DraftSel>, line: &DiffLine, side: Option<DiffSide>) -> bool {
    let s = side.unwrap_or_else(|| row_side(line));
    match sel {
        Some(r) if r.side == s => {
            let n = row_num(line, s);
            n >= r.start && n <= r.end
        }
        _ => false,
    }
}

pub fn draft_ends(sel: Option<DraftSel>, line: &DiffLine, side: Option<DiffSide>) -> bool {
    let s = side.unwrap_or_else(|| row_side(line));
    matches!(sel, Some(r) if r.side == s && row_num(line, s) == r.end)
}

/// The drag's anchor or tip row.
pub fn draft_tip(selecting: Option<Selecting>, line: &DiffLine, side: Option<DiffSide>) -> bool {
    let Some(sel) = selecting else { return false };
    let s = side.unwrap_or_else(|| row_side(line));
    if sel.side != s {
        return false;
    }
    let n = row_num(line, s);
    n == sel.tip || n == sel.anchor
}

/// `findLine`: the row with number `n` on `side`.
pub fn find_line(lines: &[DiffLine], n: i64, side: DiffSide) -> Option<usize> {
    lines.iter().position(|l| {
        l.r#type != "hunk"
            && row_num(l, side) == n
            && match side {
                DiffSide::Left => l.r#type != "add",
                DiffSide::Right => l.r#type != "del",
            }
    })
}

/// `composerOn`: whether the open composer sits on this row (and side).
pub fn composer_on(
    open: Option<OpenLine>,
    open_side: DiffSide,
    line: Option<&DiffLine>,
    side: Option<DiffSide>,
) -> bool {
    let Some(open) = open else { return false };
    if side.is_some_and(|s| s != open_side) {
        return false;
    }
    let (old_n, new_n) = line.map_or((0, 0), line_nums);
    if open.new_n != 0 && new_n != 0 {
        return open.new_n == new_n;
    }
    if open.old_n != 0 && old_n != 0 {
        return open.old_n == old_n;
    }
    false
}

/// `composePlaceholder`.
pub fn compose_placeholder(line: &DiffLine, open_side: DiffSide, open_start: i64) -> String {
    let end = row_num(line, open_side);
    let loc = comment_hunk_line_label(end, open_start);
    if loc.is_empty() {
        return "Comment…".into();
    }
    if loc.contains('-') {
        format!("Comment on lines {}…", loc.replace('-', "–"))
    } else {
        format!("Comment on line {loc}…")
    }
}

fn pending_side(pc: &PendingLineComment) -> &str {
    if pc.side.is_empty() {
        "RIGHT"
    } else {
        &pc.side
    }
}

/// `pendingOn`: pending comments drawn under this row (or split side).
pub fn pending_on<'a>(
    pending: &'a [PendingLineComment],
    line: &DiffLine,
    side: Option<DiffSide>,
) -> Vec<&'a PendingLineComment> {
    let (old_n, new_n) = line_nums(line);
    if let Some(side) = side {
        let n = if side == DiffSide::Left { old_n } else { new_n };
        if n == 0 {
            return Vec::new();
        }
        let want = if side == DiffSide::Left {
            "LEFT"
        } else {
            "RIGHT"
        };
        return pending
            .iter()
            .filter(|pc| pc.line == n && pending_side(pc) == want)
            .collect();
    }
    pending
        .iter()
        .filter(|pc| {
            let want = pending_side(pc);
            match line.r#type.as_str() {
                "del" => want == "LEFT" && old_n == pc.line,
                "add" => want == "RIGHT" && new_n == pc.line,
                _ if want == "LEFT" => old_n == pc.line,
                _ => new_n == pc.line,
            }
        })
        .collect()
}

/// `pendingCovering`: how many pending ranges include this row.
pub fn pending_covering(
    pending: &[PendingLineComment],
    path: &str,
    line: &DiffLine,
    side: Option<DiffSide>,
) -> usize {
    let as_notes: Vec<PrComment> = pending
        .iter()
        .map(|pc| PrComment {
            id: pc.id.clone(),
            path: pc.file.clone(),
            line: pc.line,
            start_line: pc.start_line,
            side: pc.side.clone(),
            ..Default::default()
        })
        .collect();
    line_notes_covering_row(&as_notes, path, line, side).len()
}

/// `hunkOf`: the hunk header index each row belongs to (-1 on headers and before the first).
pub fn hunk_of(lines: &[DiffLine]) -> Vec<isize> {
    let mut cur = -1isize;
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.r#type == "hunk" {
                cur = i as isize;
                -1
            } else {
                cur
            }
        })
        .collect()
}

fn visible(hunk_of: &[isize], collapsed: &HashSet<usize>, i: usize) -> bool {
    let h = hunk_of.get(i).copied().unwrap_or(-1);
    h == -1 || !collapsed.contains(&(h as usize))
}

/// `unifiedSlots`: line indexes shown (headers always, rows of open hunks).
pub fn unified_slots(lines: &[DiffLine], collapsed: &HashSet<usize>) -> Vec<usize> {
    let of = hunk_of(lines);
    (0..lines.len())
        .filter(|&i| lines[i].r#type == "hunk" || visible(&of, collapsed, i))
        .collect()
}

/// `splitSlots`.
pub fn split_slots(
    lines: &[DiffLine],
    rows: &[SplitRow],
    collapsed: &HashSet<usize>,
) -> Vec<SplitRow> {
    let of = hunk_of(lines);
    rows.iter()
        .filter(|r| matches!(r, SplitRow::Hunk { .. }) || visible(&of, collapsed, r.i()))
        .copied()
        .collect()
}

/// `slotOfLine` in the unified layout.
pub fn unified_slot_of_line(lines: &[DiffLine], slots: &[usize], n: i64) -> isize {
    if n == 0 {
        return -1;
    }
    slots
        .iter()
        .position(|&i| {
            lines
                .get(i)
                .is_some_and(|l| l.r#type != "hunk" && line_no(l) == n)
        })
        .map_or(-1, |s| s as isize)
}

/// `slotOfLine` in the split layout.
pub fn split_slot_of_line(lines: &[DiffLine], slots: &[SplitRow], n: i64) -> isize {
    if n == 0 {
        return -1;
    }
    slots
        .iter()
        .position(|row| match row {
            SplitRow::Hunk { .. } => false,
            SplitRow::Pair { left, right, .. } => {
                left.is_some_and(|c| row_num(&lines[c.i], DiffSide::Left) == n)
                    || right.is_some_and(|c| row_num(&lines[c.i], DiffSide::Right) == n)
            }
        })
        .map_or(-1, |s| s as isize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(t: &str, old: &str, new: &str) -> DiffLine {
        DiffLine {
            r#type: t.into(),
            old: old.into(),
            new: new.into(),
            code_html: "x".into(),
            ..Default::default()
        }
    }

    #[test]
    fn row_numbers_by_side() {
        let del = l("del", "12", "");
        assert_eq!(row_side(&del), DiffSide::Left);
        assert_eq!(row_num(&del, DiffSide::Left), 12);
        assert_eq!(row_num(&del, DiffSide::Right), 0);
        assert_eq!(line_no(&del), 12);
        assert_eq!(open_line_no(&del), 0);
        assert_eq!(api_line(&del, DiffSide::Left), -12);
        let ctx = l("ctx", "14", "16");
        assert_eq!(line_nums(&ctx), (14, 16));
        assert_eq!(line_no(&ctx), 16);
        assert_eq!(api_line(&ctx, DiffSide::Right), 16);
    }

    #[test]
    fn draft_range_follows_the_drag_then_the_open_composer() {
        let drag = Selecting {
            side: DiffSide::Right,
            anchor: 15,
            tip: 13,
        };
        let sel = draft_sel(Some(drag), None, DiffSide::Right, 0);
        assert_eq!(
            sel,
            Some(DraftSel {
                side: DiffSide::Right,
                start: 13,
                end: 15
            })
        );
        assert!(draft_covers(sel, &l("add", "", "14"), None));
        assert!(!draft_covers(sel, &l("del", "14", ""), None));
        assert!(draft_ends(sel, &l("add", "", "15"), None));
        assert!(draft_tip(Some(drag), &l("add", "", "13"), None));
        assert!(!draft_tip(Some(drag), &l("add", "", "14"), None));
        let open = OpenLine {
            old_n: 0,
            new_n: 15,
        };
        assert_eq!(
            draft_sel(None, Some(open), DiffSide::Right, 13),
            Some(DraftSel {
                side: DiffSide::Right,
                start: 13,
                end: 15
            })
        );
        assert_eq!(
            draft_sel(None, Some(open), DiffSide::Right, 0).map(|s| s.start),
            Some(15)
        );
        assert_eq!(draft_sel(None, Some(open), DiffSide::Left, 0), None);
    }

    #[test]
    fn composer_placeholders_name_the_line_or_range() {
        let row = l("add", "", "15");
        assert_eq!(
            compose_placeholder(&row, DiffSide::Right, 0),
            "Comment on line 15…"
        );
        assert_eq!(
            compose_placeholder(&row, DiffSide::Right, 13),
            "Comment on lines 13–15…"
        );
        assert_eq!(compose_placeholder(&row, DiffSide::Left, 0), "Comment…");
    }

    #[test]
    fn composer_matches_the_new_number_then_the_old() {
        let open = Some(OpenLine {
            old_n: 14,
            new_n: 16,
        });
        assert!(composer_on(
            open,
            DiffSide::Right,
            Some(&l("ctx", "14", "16")),
            None
        ));
        assert!(!composer_on(
            open,
            DiffSide::Right,
            Some(&l("ctx", "14", "16")),
            Some(DiffSide::Left)
        ));
        assert!(composer_on(
            open,
            DiffSide::Right,
            Some(&l("del", "14", "")),
            None
        ));
        assert!(!composer_on(
            None,
            DiffSide::Right,
            Some(&l("ctx", "1", "1")),
            None
        ));
        assert!(!composer_on(open, DiffSide::Right, None, None));
    }

    fn pc(line: i64, side: &str) -> PendingLineComment {
        PendingLineComment {
            id: format!("p{line}{side}"),
            file: "a.ts".into(),
            line,
            side: side.into(),
            body: "b".into(),
            ..Default::default()
        }
    }

    #[test]
    fn pending_cards_sit_on_their_side() {
        let list = vec![pc(12, "LEFT"), pc(12, ""), pc(13, "RIGHT")];
        assert_eq!(pending_on(&list, &l("del", "12", ""), None).len(), 1);
        assert_eq!(pending_on(&list, &l("add", "", "12"), None).len(), 1);
        assert_eq!(pending_on(&list, &l("ctx", "12", "12"), None).len(), 2);
        assert_eq!(
            pending_on(&list, &l("ctx", "12", "12"), Some(DiffSide::Left)).len(),
            1
        );
        assert_eq!(
            pending_on(&list, &l("ctx", "12", "13"), Some(DiffSide::Right)).len(),
            1
        );
        let mut ranged = pc(15, "RIGHT");
        ranged.start_line = 13;
        assert_eq!(
            pending_covering(&[ranged], "a.ts", &l("add", "", "14"), None),
            1
        );
    }

    #[test]
    fn find_line_skips_the_other_side() {
        let lines = vec![l("hunk", "", ""), l("del", "4", ""), l("add", "", "4")];
        assert_eq!(find_line(&lines, 4, DiffSide::Left), Some(1));
        assert_eq!(find_line(&lines, 4, DiffSide::Right), Some(2));
        assert_eq!(find_line(&lines, 9, DiffSide::Right), None);
    }

    #[test]
    fn folded_hunks_drop_their_rows_but_keep_the_header() {
        let lines = vec![
            l("hunk", "", ""),
            l("ctx", "1", "1"),
            l("hunk", "", ""),
            l("add", "", "9"),
        ];
        assert_eq!(hunk_of(&lines), vec![-1, 0, -1, 2]);
        let none = HashSet::new();
        assert_eq!(unified_slots(&lines, &none), vec![0, 1, 2, 3]);
        let folded: HashSet<usize> = [0].into();
        assert_eq!(unified_slots(&lines, &folded), vec![0, 2, 3]);
        assert_eq!(unified_slot_of_line(&lines, &[0, 2, 3], 9), 2);
        assert_eq!(unified_slot_of_line(&lines, &[0, 2, 3], 1), -1);
        let rows = crate::split_diff::split_diff_rows(&lines);
        let slots = split_slots(&lines, &rows, &folded);
        assert_eq!(slots.len(), 3);
        assert_eq!(split_slot_of_line(&lines, &slots, 9), 2);
    }
}
