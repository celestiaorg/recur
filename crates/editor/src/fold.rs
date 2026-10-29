//! Code folding (`@codemirror/language` foldState, `foldable`, fold
//! commands, fold gutter markers) with Recur's TextMate fold service:
//! grammar fold markers, else indentation.

use crate::state::{EditorState, Effect, Transaction, TransactionSpec};
use crate::view_queries::{LineBlock, ViewQueries};

/// Folded ranges, sorted by start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoldState {
    ranges: Vec<(usize, usize)>,
}

impl FoldState {
    pub fn ranges(&self) -> &[(usize, usize)] {
        &self.ranges
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// `clearTouchedFolds`: drop folds overlapping `from..to` (or strictly
    /// containing `from` when the range is empty).
    fn clear_touched(&mut self, from: usize, to: usize) {
        self.ranges.retain(|&(a, b)| !(a < to && b > from));
    }

    fn map(&mut self, tr: &Transaction) {
        if tr.changes.is_empty() {
            return;
        }
        let mut out = Vec::with_capacity(self.ranges.len());
        for &(from, to) in &self.ranges {
            let f = tr.changes.map_pos(from, 1);
            let t = tr.changes.map_pos(to, -1);
            if f < t {
                out.push((f, t));
            }
        }
        self.ranges = out;
    }

    fn exists(&self, from: usize, to: usize) -> bool {
        self.ranges.iter().any(|&(a, b)| a == from && b == to)
    }

    pub(crate) fn update(&self, tr: &Transaction) -> FoldState {
        let mut next = self.clone();
        if tr.is_user_event("delete") {
            tr.changes
                .iter_changed_ranges(false, |from_a, to_a, _, _| next.clear_touched(from_a, to_a));
        }
        next.map(tr);
        let mut add = Vec::new();
        for e in &tr.effects {
            match e {
                Effect::Fold { from, to } if !next.exists(*from, *to) => add.push((*from, *to)),
                Effect::Unfold { from, to } => {
                    next.ranges.retain(|&(a, b)| !(a == *from && b == *to))
                }
                _ => {}
            }
        }
        next.ranges.extend(add);
        next.ranges.sort();
        if let Some(sel) = &tr.selection {
            let head = sel.main().head();
            next.clear_touched(head, head);
        }
        next
    }
}

/// Folded range starting within `from..=to` with the lowest start.
pub fn find_fold(state: &EditorState, from: usize, to: usize) -> Option<(usize, usize)> {
    state
        .folds
        .ranges()
        .iter()
        .filter(|&&(a, b)| a <= to && b >= from)
        .min_by_key(|(a, _)| *a)
        .copied()
}

/// `foldable`: the range a fold starting on this line would cover.
pub fn foldable(state: &EditorState, line_start: usize, line_end: usize) -> Option<(usize, usize)> {
    let lang = state.language()?;
    let doc = &state.doc;
    let line = doc.line_at(line_start);
    let _ = line_end;
    let r = recur_textmate::textmate_fold(
        lang.fold_markers.as_ref(),
        |n| doc.line_text(n),
        doc.lines(),
        line.number,
        state.tab_size(),
    )?;
    Some((doc.line(r.start_line).to, doc.line(r.end_line).to))
}

fn selected_lines(view: &dyn ViewQueries) -> Vec<LineBlock> {
    let mut lines: Vec<LineBlock> = Vec::new();
    for r in &view.state().selection.ranges {
        let head = r.head();
        if lines.iter().any(|l| l.from <= head && l.to >= head) {
            continue;
        }
        lines.push(view.line_block_at(head));
    }
    lines
}

fn fold_spec(effects: Vec<Effect>) -> TransactionSpec {
    TransactionSpec::default().effects(effects)
}

/// `foldCode`.
pub fn fold_code(view: &dyn ViewQueries) -> Option<TransactionSpec> {
    for line in selected_lines(view) {
        if let Some((from, to)) = foldable(view.state(), line.from, line.to) {
            return Some(fold_spec(vec![Effect::Fold { from, to }]));
        }
    }
    None
}

/// `unfoldCode`.
pub fn unfold_code(view: &dyn ViewQueries) -> Option<TransactionSpec> {
    let mut effects = Vec::new();
    for line in selected_lines(view) {
        if let Some((from, to)) = find_fold(view.state(), line.from, line.to) {
            effects.push(Effect::Unfold { from, to });
        }
    }
    (!effects.is_empty()).then(|| fold_spec(effects))
}

/// `foldAll`.
pub fn fold_all(view: &dyn ViewQueries) -> Option<TransactionSpec> {
    let state = view.state();
    let mut effects = Vec::new();
    let mut pos = 0;
    while pos < state.doc.len() {
        let line = view.line_block_at(pos);
        let range = foldable(state, line.from, line.to);
        if let Some((from, to)) = range {
            effects.push(Effect::Fold { from, to });
        }
        pos = match range {
            Some((_, to)) => view.line_block_at(to).to + 1,
            None => line.to + 1,
        };
    }
    (!effects.is_empty()).then(|| fold_spec(effects))
}

/// `unfoldAll`.
pub fn unfold_all(state: &EditorState) -> Option<TransactionSpec> {
    if state.folds.is_empty() {
        return None;
    }
    Some(fold_spec(
        state
            .folds
            .ranges()
            .iter()
            .map(|&(from, to)| Effect::Unfold { from, to })
            .collect(),
    ))
}

fn foldable_container(view: &dyn ViewQueries, block: LineBlock) -> Option<(usize, usize)> {
    let mut line = block;
    loop {
        if let Some(region) = foldable(view.state(), line.from, line.to)
            && region.1 > block.from
        {
            return Some(region);
        }
        if line.from == 0 {
            return None;
        }
        line = view.line_block_at(line.from - 1);
    }
}

/// `toggleFold`.
pub fn toggle_fold(view: &dyn ViewQueries) -> Option<TransactionSpec> {
    let mut effects = Vec::new();
    for line in selected_lines(view) {
        if let Some((from, to)) = find_fold(view.state(), line.from, line.to) {
            effects.push(Effect::Unfold { from, to });
        } else if let Some((from, to)) = foldable_container(view, line) {
            effects.push(Effect::Fold { from, to });
        }
    }
    (!effects.is_empty()).then(|| fold_spec(effects))
}

/// Fold gutter marker for a line block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldMarker {
    /// "⌄" — can fold.
    Open,
    /// "›" — folded.
    Closed,
}

impl FoldMarker {
    pub fn text(self) -> &'static str {
        match self {
            FoldMarker::Open => "⌄",
            FoldMarker::Closed => "›",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            FoldMarker::Open => "Fold line",
            FoldMarker::Closed => "Unfold line",
        }
    }
}

pub fn fold_marker(state: &EditorState, block: LineBlock) -> Option<FoldMarker> {
    if find_fold(state, block.from, block.to).is_some() {
        Some(FoldMarker::Closed)
    } else if foldable(state, block.from, block.to).is_some() {
        Some(FoldMarker::Open)
    } else {
        None
    }
}

/// Fold gutter click: unfold a fold on the line, else fold it.
pub fn fold_gutter_click(state: &EditorState, block: LineBlock) -> Option<TransactionSpec> {
    if let Some((from, to)) = find_fold(state, block.from, block.to) {
        return Some(fold_spec(vec![Effect::Unfold { from, to }]));
    }
    let (from, to) = foldable(state, block.from, block.to)?;
    Some(fold_spec(vec![Effect::Fold { from, to }]))
}

/// Clicking a fold placeholder unfolds it.
pub fn unfold_at_block(state: &EditorState, block: LineBlock) -> Option<TransactionSpec> {
    let (from, to) = find_fold(state, block.from, block.to)?;
    Some(fold_spec(vec![Effect::Unfold { from, to }]))
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::change::Change;
    use crate::state::{LanguageConfig, StateConfig};
    use crate::view_queries::MonoView;

    fn st(doc: &str) -> EditorState {
        EditorState::create(
            doc,
            StateConfig {
                language: Some(Rc::new(LanguageConfig {
                    scope: "source.go".into(),
                    ..LanguageConfig::default()
                })),
                ..StateConfig::default()
            },
        )
    }

    const GO: &str = "func main() {\n\tx := 1\n\ty := 2\n}\n";

    #[test]
    fn foldable_uses_indentation_without_markers() {
        let s = st(GO);
        assert_eq!(foldable(&s, 0, 13), Some((13, 29)));
        assert_eq!(foldable(&s, 14, 20), None);
        let plain = EditorState::create(GO, StateConfig::default());
        assert_eq!(foldable(&plain, 0, 13), None);
    }

    #[test]
    fn fold_and_unfold_commands() {
        let s = st(GO);
        let v = MonoView::new(s.clone());
        let spec = fold_code(&v).unwrap();
        let s = s.update(spec).state().clone();
        assert_eq!(s.folds.ranges(), &[(13, 29)]);
        let v = MonoView::new(s.clone());
        assert_eq!(v.line_block_at(20), v.line_block_at(0));
        let s = s.update(unfold_code(&v).unwrap()).state().clone();
        assert!(s.folds.is_empty());
    }

    #[test]
    fn folds_map_and_clear_on_selection_and_delete() {
        let s = st(GO);
        let s = s
            .update(TransactionSpec::default().effect(Effect::Fold { from: 13, to: 27 }))
            .state()
            .clone();
        let s2 = s
            .update(TransactionSpec::default().changes(Change::insert(0, "// c\n")))
            .state()
            .clone();
        assert_eq!(s2.folds.ranges(), &[(18, 32)]);
        let s3 = s
            .update(TransactionSpec::default().cursor(20))
            .state()
            .clone();
        assert!(s3.folds.is_empty());
        let s4 = s
            .update(
                TransactionSpec::default()
                    .changes(Change::delete(15, 16))
                    .user_event("delete"),
            )
            .state()
            .clone();
        assert!(s4.folds.is_empty());
    }

    #[test]
    fn gutter_markers_and_fold_all() {
        let s = st("a {\n  b\n}\nc {\n  d\n}");
        let v = MonoView::new(s.clone());
        assert_eq!(fold_marker(&s, v.line_block_at(0)), Some(FoldMarker::Open));
        let s = s.update(fold_all(&v).unwrap()).state().clone();
        assert_eq!(s.folds.ranges().len(), 2);
        let v = MonoView::new(s.clone());
        assert_eq!(
            fold_marker(&s, v.line_block_at(0)),
            Some(FoldMarker::Closed)
        );
        assert!(unfold_all(&s).is_some());
    }
}
