//! Selections, a port of CodeMirror's `SelectionRange` / `EditorSelection`.

use crate::change::{ChangeDesc, ChangeSet};

const INVERTED: u8 = 32;
const ASSOC_BEFORE: u8 = 8;
const ASSOC_AFTER: u8 = 16;
const UNDIRECTIONAL: u8 = 64;
const BIDI_NONE: u8 = 7;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionRange {
    pub from: usize,
    pub to: usize,
    flags: u8,
    /// Stored vertical-motion x offset (pixels), like CodeMirror's goalColumn.
    pub goal_column: Option<f32>,
}

impl SelectionRange {
    pub fn cursor(pos: usize) -> SelectionRange {
        SelectionRange::cursor_assoc(pos, 0, None)
    }

    pub fn cursor_assoc(pos: usize, assoc: i32, goal_column: Option<f32>) -> SelectionRange {
        let a = match assoc.cmp(&0) {
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Less => ASSOC_BEFORE,
            std::cmp::Ordering::Greater => ASSOC_AFTER,
        };
        SelectionRange {
            from: pos,
            to: pos,
            flags: a | BIDI_NONE,
            goal_column,
        }
    }

    pub fn range(anchor: usize, head: usize) -> SelectionRange {
        SelectionRange::range_with(anchor, head, None, 0)
    }

    pub fn range_with(
        anchor: usize,
        head: usize,
        goal_column: Option<f32>,
        assoc: i32,
    ) -> SelectionRange {
        let mut flags = BIDI_NONE;
        let mut assoc = assoc;
        if assoc == 0 && anchor != head {
            assoc = if head < anchor { 1 } else { -1 };
        }
        if assoc != 0 {
            flags |= if assoc < 0 { ASSOC_BEFORE } else { ASSOC_AFTER };
        }
        if head < anchor {
            SelectionRange {
                from: head,
                to: anchor,
                flags: flags | INVERTED,
                goal_column,
            }
        } else {
            SelectionRange {
                from: anchor,
                to: head,
                flags,
                goal_column,
            }
        }
    }

    pub fn undirectional(from: usize, to: usize) -> SelectionRange {
        SelectionRange {
            from,
            to,
            flags: UNDIRECTIONAL,
            goal_column: None,
        }
    }

    pub fn anchor(&self) -> usize {
        if self.flags & INVERTED != 0 {
            self.to
        } else {
            self.from
        }
    }

    pub fn head(&self) -> usize {
        if self.flags & INVERTED != 0 {
            self.from
        } else {
            self.to
        }
    }

    pub fn is_empty(&self) -> bool {
        self.from == self.to
    }

    pub fn assoc(&self) -> i32 {
        if self.flags & ASSOC_BEFORE != 0 {
            -1
        } else if self.flags & ASSOC_AFTER != 0 {
            1
        } else {
            0
        }
    }

    pub fn is_undirectional(&self) -> bool {
        self.flags & UNDIRECTIONAL != 0
    }

    pub fn map(&self, change: &ChangeDesc, assoc: i32) -> SelectionRange {
        let (from, to) = if self.is_empty() {
            let p = change.map_pos(self.from, assoc);
            (p, p)
        } else {
            (change.map_pos(self.from, 1), change.map_pos(self.to, -1))
        };
        if from == self.from && to == self.to {
            return *self;
        }
        SelectionRange {
            from,
            to,
            flags: self.flags,
            goal_column: self.goal_column,
        }
    }

    pub fn map_set(&self, change: &ChangeSet, assoc: i32) -> SelectionRange {
        self.map(&change.desc(), assoc)
    }

    /// Extend to cover at least `from..to`.
    pub fn extend(&self, from: usize, to: usize, assoc: i32) -> SelectionRange {
        let anchor = self.anchor();
        if from <= anchor && to >= anchor {
            return SelectionRange::range_with(from, to, None, assoc);
        }
        let head = if from.abs_diff(anchor) > to.abs_diff(anchor) {
            from
        } else {
            to
        };
        SelectionRange::range_with(anchor, head, None, assoc)
    }

    pub fn eq_range(&self, other: &SelectionRange, include_assoc: bool) -> bool {
        self.anchor() == other.anchor()
            && self.head() == other.head()
            && self.goal_column == other.goal_column
            && (!include_assoc || !self.is_empty() || self.assoc() == other.assoc())
    }

    pub fn with_goal_column(mut self, goal: Option<f32>) -> SelectionRange {
        self.goal_column = goal;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditorSelection {
    pub ranges: Vec<SelectionRange>,
    pub main_index: usize,
}

impl EditorSelection {
    pub fn single(anchor: usize, head: usize) -> EditorSelection {
        EditorSelection {
            ranges: vec![SelectionRange::range(anchor, head)],
            main_index: 0,
        }
    }

    pub fn cursor(pos: usize) -> EditorSelection {
        EditorSelection {
            ranges: vec![SelectionRange::cursor(pos)],
            main_index: 0,
        }
    }

    pub fn of_range(range: SelectionRange) -> EditorSelection {
        EditorSelection {
            ranges: vec![range],
            main_index: 0,
        }
    }

    /// Sort and merge ranges into a valid selection.
    pub fn create(ranges: Vec<SelectionRange>, main_index: usize) -> EditorSelection {
        let mut pos = 0;
        for (i, range) in ranges.iter().enumerate() {
            let bad = if range.is_empty() {
                range.from <= pos && i > 0
            } else {
                range.from < pos
            };
            if bad {
                return EditorSelection::normalized(ranges, main_index);
            }
            pos = range.to;
        }
        EditorSelection { ranges, main_index }
    }

    fn normalized(mut ranges: Vec<SelectionRange>, main_index: usize) -> EditorSelection {
        let mut tagged: Vec<(usize, SelectionRange)> = ranges.drain(..).enumerate().collect();
        tagged.sort_by_key(|(_, r)| r.from);
        let mut main_index = tagged
            .iter()
            .position(|(i, _)| *i == main_index)
            .unwrap_or(0);
        let mut out: Vec<SelectionRange> = tagged.into_iter().map(|(_, r)| r).collect();
        let mut i = 1;
        while i < out.len() {
            let range = out[i];
            let prev = out[i - 1];
            let overlaps = if range.is_empty() {
                range.from <= prev.to
            } else {
                range.from < prev.to
            };
            if overlaps {
                let from = prev.from;
                let to = range.to.max(prev.to);
                if i <= main_index {
                    main_index = main_index.saturating_sub(1);
                }
                let merged = if range.anchor() > range.head() {
                    SelectionRange::range(to, from)
                } else {
                    SelectionRange::range(from, to)
                };
                out.splice(i - 1..=i, [merged]);
                continue;
            }
            i += 1;
        }
        EditorSelection {
            ranges: out,
            main_index,
        }
    }

    pub fn main(&self) -> SelectionRange {
        self.ranges[self.main_index]
    }

    pub fn as_single(&self) -> EditorSelection {
        if self.ranges.len() == 1 {
            return self.clone();
        }
        EditorSelection {
            ranges: vec![self.main()],
            main_index: 0,
        }
    }

    pub fn add_range(&self, range: SelectionRange, main: bool) -> EditorSelection {
        let mut ranges = vec![range];
        ranges.extend(self.ranges.iter().copied());
        EditorSelection::create(ranges, if main { 0 } else { self.main_index + 1 })
    }

    pub fn replace_range(&self, range: SelectionRange, which: usize) -> EditorSelection {
        let mut ranges = self.ranges.clone();
        ranges[which] = range;
        EditorSelection::create(ranges, self.main_index)
    }

    pub fn map(&self, change: &ChangeDesc, assoc: i32) -> EditorSelection {
        if change.is_empty() {
            return self.clone();
        }
        EditorSelection::create(
            self.ranges.iter().map(|r| r.map(change, assoc)).collect(),
            self.main_index,
        )
    }

    pub fn eq_sel(&self, other: &EditorSelection, include_assoc: bool) -> bool {
        self.ranges.len() == other.ranges.len()
            && self.main_index == other.main_index
            && self
                .ranges
                .iter()
                .zip(&other.ranges)
                .all(|(a, b)| a.eq_range(b, include_assoc))
    }

    /// `{ranges: [{anchor, head}], main}`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ranges": self.ranges.iter().map(|r| serde_json::json!({"anchor": r.anchor(), "head": r.head()})).collect::<Vec<_>>(),
            "main": self.main_index,
        })
    }

    pub fn from_json(json: &serde_json::Value) -> Option<EditorSelection> {
        let ranges: Option<Vec<SelectionRange>> = json
            .get("ranges")?
            .as_array()?
            .iter()
            .map(|r| {
                Some(SelectionRange::range(
                    r.get("anchor")?.as_u64()? as usize,
                    r.get("head")?.as_u64()? as usize,
                ))
            })
            .collect();
        let ranges = ranges?;
        let main = json.get("main")?.as_u64()? as usize;
        (main < ranges.len()).then_some(EditorSelection {
            ranges,
            main_index: main,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;

    #[test]
    fn ranges_track_direction() {
        let r = SelectionRange::range(5, 2);
        assert_eq!((r.from, r.to, r.anchor(), r.head()), (2, 5, 5, 2));
        assert_eq!(r.assoc(), 1);
        let e = r.extend(7, 7, 0);
        assert_eq!((e.anchor(), e.head()), (5, 7));
    }

    #[test]
    fn create_merges_overlaps_and_tracks_main() {
        let sel = EditorSelection::create(
            vec![
                SelectionRange::range(4, 8),
                SelectionRange::range(0, 2),
                SelectionRange::range(6, 10),
            ],
            2,
        );
        assert_eq!(sel.ranges.len(), 2);
        assert_eq!((sel.ranges[0].from, sel.ranges[0].to), (0, 2));
        assert_eq!((sel.ranges[1].from, sel.ranges[1].to), (4, 10));
        assert_eq!(sel.main_index, 1);
        let cursors = EditorSelection::create(
            vec![SelectionRange::cursor(3), SelectionRange::cursor(3)],
            1,
        );
        assert_eq!(cursors.ranges.len(), 1);
    }

    #[test]
    fn maps_through_changes() {
        let set = ChangeSet::of(&[Change::insert(0, "xx")], 10);
        let sel = EditorSelection::create(
            vec![SelectionRange::range(1, 4), SelectionRange::cursor(6)],
            0,
        );
        let m = sel.map(&set.desc(), -1);
        assert_eq!((m.ranges[0].from, m.ranges[0].to), (3, 6));
        assert_eq!(m.ranges[1].from, 8);
    }

    #[test]
    fn json_round_trip() {
        let sel = EditorSelection::create(
            vec![SelectionRange::range(4, 1), SelectionRange::cursor(9)],
            1,
        );
        let back = EditorSelection::from_json(&sel.to_json()).unwrap();
        assert!(back.eq_sel(&sel, false));
    }
}
