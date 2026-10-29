//! Undo history, a port of `@codemirror/commands` history (`historyField`,
//! `undo` / `redo` / `undoSelection` / `redoSelection`, depths, JSON).

use std::rc::Rc;

use crate::change::{ChangeDesc, ChangeSet};
use crate::selection::EditorSelection;
use crate::state::{Annotations, EditorState, Isolate, StateConfig, Transaction, TransactionSpec};

const MAX_SELECTIONS_PER_EVENT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Branch {
    Done,
    Undone,
}

#[derive(Clone, Debug)]
pub(crate) struct HistEvent {
    /// `None` for selection-only events (always the last of a branch).
    changes: Option<ChangeSet>,
    mapped: Option<ChangeDesc>,
    start_selection: Option<EditorSelection>,
    selections_after: Vec<EditorSelection>,
}

impl HistEvent {
    fn set_sel_after(&self, after: Vec<EditorSelection>) -> HistEvent {
        HistEvent {
            selections_after: after,
            ..self.clone()
        }
    }

    fn from_transaction(
        tr: &Transaction,
        selection: Option<&EditorSelection>,
    ) -> Option<HistEvent> {
        if tr.changes.is_empty() {
            return None;
        }
        Some(HistEvent {
            changes: Some(tr.changes.invert(&tr.start_state.doc)),
            mapped: None,
            start_selection: Some(
                selection
                    .cloned()
                    .unwrap_or_else(|| tr.start_state.selection.clone()),
            ),
            selections_after: Vec::new(),
        })
    }

    fn selection(selections: Vec<EditorSelection>) -> HistEvent {
        HistEvent {
            changes: None,
            mapped: None,
            start_selection: None,
            selections_after: selections,
        }
    }

    fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        if let Some(c) = &self.changes {
            o.insert("changes".into(), c.to_json());
        }
        if let Some(m) = &self.mapped {
            o.insert("mapped".into(), serde_json::json!(m.to_json()));
        }
        if let Some(s) = &self.start_selection {
            o.insert("startSelection".into(), s.to_json());
        }
        o.insert(
            "selectionsAfter".into(),
            serde_json::Value::Array(
                self.selections_after
                    .iter()
                    .map(EditorSelection::to_json)
                    .collect(),
            ),
        );
        serde_json::Value::Object(o)
    }

    fn from_json(json: &serde_json::Value) -> Option<HistEvent> {
        let changes = match json.get("changes") {
            Some(c) if !c.is_null() => Some(ChangeSet::from_json(c)?),
            _ => None,
        };
        let mapped = match json.get("mapped") {
            Some(m) if !m.is_null() => {
                let v: Option<Vec<i64>> = m.as_array()?.iter().map(|x| x.as_i64()).collect();
                Some(ChangeDesc::from_json(v?)?)
            }
            _ => None,
        };
        let start_selection = match json.get("startSelection") {
            Some(s) if !s.is_null() => Some(EditorSelection::from_json(s)?),
            _ => None,
        };
        let selections_after: Option<Vec<EditorSelection>> = json
            .get("selectionsAfter")?
            .as_array()?
            .iter()
            .map(EditorSelection::from_json)
            .collect();
        Some(HistEvent {
            changes,
            mapped,
            start_selection,
            selections_after: selections_after?,
        })
    }
}

fn update_branch(
    branch: &[HistEvent],
    to: usize,
    max_len: usize,
    new_event: HistEvent,
) -> Vec<HistEvent> {
    let start = if to + 1 > max_len + 20 {
        to - max_len - 1
    } else {
        0
    };
    let mut out = branch[start..to].to_vec();
    out.push(new_event);
    out
}

fn is_adjacent(a: &ChangeSet, b: &ChangeSet) -> bool {
    let mut ranges = Vec::new();
    a.iter_changed_ranges(false, |f, t, _, _| ranges.push((f, t)));
    let mut adjacent = false;
    b.iter_changed_ranges(false, |_, _, f, t| {
        if ranges.iter().any(|&(from, to)| t >= from && f <= to) {
            adjacent = true;
        }
    });
    adjacent
}

fn eq_selection_shape(a: &EditorSelection, b: &EditorSelection) -> bool {
    a.ranges.len() == b.ranges.len()
        && a.ranges
            .iter()
            .zip(&b.ranges)
            .all(|(x, y)| x.is_empty() == y.is_empty())
}

fn add_selection(branch: &[HistEvent], selection: &EditorSelection) -> Vec<HistEvent> {
    let Some(last) = branch.last() else {
        return vec![HistEvent::selection(vec![selection.clone()])];
    };
    let skip = last
        .selections_after
        .len()
        .saturating_sub(MAX_SELECTIONS_PER_EVENT);
    let mut sels = last.selections_after[skip..].to_vec();
    if sels.last().is_some_and(|s| s.eq_sel(selection, false)) {
        return branch.to_vec();
    }
    sels.push(selection.clone());
    update_branch(
        branch,
        branch.len() - 1,
        usize::MAX / 4,
        last.set_sel_after(sels),
    )
}

fn pop_selection(branch: &[HistEvent]) -> Vec<HistEvent> {
    let mut out = branch.to_vec();
    if let Some(last) = out.last_mut() {
        let n = last.selections_after.len();
        last.selections_after.truncate(n.saturating_sub(1));
    }
    out
}

fn map_event(event: &HistEvent, mapping: &ChangeDesc, extra: &[EditorSelection]) -> HistEvent {
    let mut selections: Vec<EditorSelection> = event
        .selections_after
        .iter()
        .map(|s| s.map(mapping, -1))
        .collect();
    selections.extend(extra.iter().cloned());
    let Some(changes) = &event.changes else {
        return HistEvent::selection(selections);
    };
    let mapped_changes = changes.map(&changeset_of_desc(mapping), false);
    let before = mapping.map_desc(&changes.desc(), true);
    let full_mapping = match &event.mapped {
        Some(m) => m.compose_desc(&before),
        None => before.clone(),
    };
    HistEvent {
        changes: Some(mapped_changes),
        mapped: Some(full_mapping),
        start_selection: event.start_selection.as_ref().map(|s| s.map(&before, -1)),
        selections_after: selections,
    }
}

/// A change set with no inserted text standing in for a description when
/// mapping (mapping never reads the other side's text).
fn changeset_of_desc(d: &ChangeDesc) -> ChangeSet {
    ChangeSet {
        sections: d.sections.clone(),
        inserted: Vec::new(),
    }
}

fn add_mapping_to_branch(branch: &[HistEvent], mapping: &ChangeDesc) -> Vec<HistEvent> {
    if branch.is_empty() {
        return Vec::new();
    }
    let mut length = branch.len();
    let mut selections: Vec<EditorSelection> = Vec::new();
    let mut mapping = mapping.clone();
    while length > 0 {
        let event = map_event(&branch[length - 1], &mapping, &selections);
        if event.changes.as_ref().is_some_and(|c| !c.is_empty()) {
            let mut result = branch[..length].to_vec();
            result[length - 1] = event;
            return result;
        }
        mapping = event.mapped.clone().unwrap_or_default();
        length -= 1;
        selections = event.selections_after;
    }
    if selections.is_empty() {
        Vec::new()
    } else {
        vec![HistEvent::selection(selections)]
    }
}

fn joinable_user_event(e: &str) -> bool {
    ["input.type", "delete"]
        .iter()
        .any(|p| e == *p || (e.starts_with(p) && e.as_bytes()[p.len()] == b'.'))
}

fn is_select_event(e: &str) -> bool {
    e == "select" || e.starts_with("select.")
}

/// What `pop` records so the history field can rebuild branches.
#[derive(Debug)]
pub(crate) struct FromHistory {
    side: Branch,
    rest: Vec<HistEvent>,
    selection: EditorSelection,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryState {
    done: Vec<HistEvent>,
    undone: Vec<HistEvent>,
    prev_time: u64,
    prev_user_event: Option<String>,
}

impl HistoryState {
    fn isolate(&self) -> HistoryState {
        if self.prev_time != 0 {
            HistoryState {
                done: self.done.clone(),
                undone: self.undone.clone(),
                prev_time: 0,
                prev_user_event: None,
            }
        } else {
            self.clone()
        }
    }

    fn add_changes(
        &self,
        event: HistEvent,
        time: u64,
        user_event: Option<&str>,
        config: &StateConfig,
        tr: &Transaction,
    ) -> HistoryState {
        let done = &self.done;
        let join = done.last().is_some_and(|last| {
            last.changes.as_ref().is_some_and(|c| !c.is_empty())
                && event.changes.is_some()
                && user_event.is_none_or(joinable_user_event)
                && ((last.selections_after.is_empty()
                    && time.saturating_sub(self.prev_time) < config.history_new_group_delay
                    && is_adjacent(
                        last.changes.as_ref().expect("checked"),
                        event.changes.as_ref().expect("checked"),
                    ))
                    || user_event == Some("input.type.compose"))
        });
        let _ = tr;
        let done = if join {
            let last = done.last().expect("join implies a last event");
            let merged = HistEvent {
                changes: Some(
                    event
                        .changes
                        .as_ref()
                        .expect("checked")
                        .compose(last.changes.as_ref().expect("checked")),
                ),
                mapped: last.mapped.clone(),
                start_selection: last.start_selection.clone(),
                selections_after: Vec::new(),
            };
            update_branch(done, done.len() - 1, config.history_min_depth, merged)
        } else {
            update_branch(done, done.len(), config.history_min_depth, event)
        };
        HistoryState {
            done,
            undone: Vec::new(),
            prev_time: time,
            prev_user_event: user_event.map(str::to_string),
        }
    }

    fn add_selection_to_state(
        &self,
        selection: &EditorSelection,
        time: u64,
        user_event: Option<&str>,
        delay: u64,
    ) -> HistoryState {
        let last = self.done.last().map(|e| &e.selections_after);
        if let Some(last) = last
            && let Some(last_sel) = last.last()
            && time.saturating_sub(self.prev_time) < delay
            && user_event == self.prev_user_event.as_deref()
            && user_event.is_some_and(is_select_event)
            && eq_selection_shape(last_sel, selection)
        {
            return self.clone();
        }
        HistoryState {
            done: add_selection(&self.done, selection),
            undone: self.undone.clone(),
            prev_time: time,
            prev_user_event: user_event.map(str::to_string),
        }
    }

    fn add_mapping(&self, mapping: &ChangeDesc) -> HistoryState {
        HistoryState {
            done: add_mapping_to_branch(&self.done, mapping),
            undone: add_mapping_to_branch(&self.undone, mapping),
            prev_time: self.prev_time,
            prev_user_event: self.prev_user_event.clone(),
        }
    }

    /// The history field's update.
    pub(crate) fn update(&self, tr: &Transaction, config: &StateConfig) -> HistoryState {
        if let Some(from) = &tr.annotations.from_history {
            let item = HistEvent::from_transaction(tr, Some(&from.selection));
            let other = if from.side == Branch::Done {
                &self.undone
            } else {
                &self.done
            };
            let other = match item {
                Some(item) => update_branch(other, other.len(), config.history_min_depth, item),
                None => add_selection(other, &tr.start_state.selection),
            };
            return if from.side == Branch::Done {
                HistoryState {
                    done: from.rest.clone(),
                    undone: other,
                    prev_time: 0,
                    prev_user_event: None,
                }
            } else {
                HistoryState {
                    done: other,
                    undone: from.rest.clone(),
                    prev_time: 0,
                    prev_user_event: None,
                }
            };
        }
        let isolate = tr.annotations.isolate_history;
        let mut state = if matches!(isolate, Some(Isolate::Full | Isolate::Before)) {
            self.isolate()
        } else {
            self.clone()
        };
        if tr.annotations.add_to_history == Some(false) {
            return if tr.changes.is_empty() {
                state
            } else {
                state.add_mapping(&tr.changes.desc())
            };
        }
        let time = tr.time();
        let user_event = tr.user_event();
        if let Some(event) = HistEvent::from_transaction(tr, None) {
            state = state.add_changes(event, time, user_event, config, tr);
        } else if tr.selection.is_some() {
            state = state.add_selection_to_state(
                &tr.start_state.selection,
                time,
                user_event,
                config.history_new_group_delay,
            );
        }
        if matches!(isolate, Some(Isolate::Full | Isolate::After)) {
            state = state.isolate();
        }
        state
    }

    fn depth(branch: &[HistEvent]) -> usize {
        branch.len() - usize::from(branch.first().is_some_and(|e| e.changes.is_none()))
    }

    pub fn undo_depth(&self) -> usize {
        Self::depth(&self.done)
    }

    pub fn redo_depth(&self) -> usize {
        Self::depth(&self.undone)
    }

    fn pop(
        &self,
        side: Branch,
        state: &EditorState,
        only_selection: bool,
    ) -> Option<TransactionSpec> {
        let branch = if side == Branch::Done {
            &self.done
        } else {
            &self.undone
        };
        let event = branch.last()?;
        let selection = event.selections_after.first().cloned().unwrap_or_else(|| {
            match (&event.start_selection, &event.changes) {
                (Some(s), Some(c)) => s.map(&c.inverted_desc(), 1),
                _ => state.selection.clone(),
            }
        });
        if only_selection && !event.selections_after.is_empty() {
            let target = event.selections_after.last().cloned().expect("non-empty");
            return Some(TransactionSpec {
                selection: Some(target),
                annotations: Annotations {
                    user_event: Some(
                        if side == Branch::Done {
                            "select.undo"
                        } else {
                            "select.redo"
                        }
                        .into(),
                    ),
                    from_history: Some(Rc::new(FromHistory {
                        side,
                        rest: pop_selection(branch),
                        selection,
                    })),
                    ..Annotations::default()
                },
                scroll_into_view: true,
                ..TransactionSpec::default()
            });
        }
        let changes = event.changes.as_ref()?;
        let mut rest: Vec<HistEvent> = branch[..branch.len() - 1].to_vec();
        if let Some(mapped) = &event.mapped {
            rest = add_mapping_to_branch(&rest, mapped);
        }
        Some(TransactionSpec {
            changes: crate::state::ChangesInput::Set(changes.clone()),
            selection: event.start_selection.clone(),
            filter: Some(false),
            annotations: Annotations {
                user_event: Some(if side == Branch::Done { "undo" } else { "redo" }.into()),
                from_history: Some(Rc::new(FromHistory {
                    side,
                    rest,
                    selection,
                })),
                ..Annotations::default()
            },
            scroll_into_view: true,
            ..TransactionSpec::default()
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "done": self.done.iter().map(HistEvent::to_json).collect::<Vec<_>>(),
            "undone": self.undone.iter().map(HistEvent::to_json).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(json: &serde_json::Value) -> Option<HistoryState> {
        let parse = |key: &str| -> Option<Vec<HistEvent>> {
            json.get(key)?
                .as_array()?
                .iter()
                .map(HistEvent::from_json)
                .collect()
        };
        Some(HistoryState {
            done: parse("done")?,
            undone: parse("undone")?,
            prev_time: 0,
            prev_user_event: None,
        })
    }
}

/// `undo` / `redo` / `undoSelection` / `redoSelection`: the transaction to
/// dispatch, if any.
pub fn history_command(
    state: &EditorState,
    side: Branch,
    selection_only: bool,
) -> Option<Transaction> {
    if !selection_only && state.read_only() {
        return None;
    }
    let spec = state.history.pop(side, state, selection_only)?;
    Some(state.update(spec))
}

pub fn undo(state: &EditorState) -> Option<Transaction> {
    history_command(state, Branch::Done, false)
}

pub fn redo(state: &EditorState) -> Option<Transaction> {
    history_command(state, Branch::Undone, false)
}

pub fn undo_selection(state: &EditorState) -> Option<Transaction> {
    history_command(state, Branch::Done, true)
}

pub fn redo_selection(state: &EditorState) -> Option<Transaction> {
    history_command(state, Branch::Undone, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::Change;
    use crate::state::StateConfig;

    fn st(doc: &str) -> EditorState {
        EditorState::create(doc, StateConfig::default())
    }

    fn type_at(s: &EditorState, pos: usize, text: &str, time: u64) -> EditorState {
        s.update(
            TransactionSpec::default()
                .changes(Change::insert(pos, text))
                .user_event("input.type")
                .time(time),
        )
        .state()
        .clone()
    }

    #[test]
    fn groups_adjacent_typing_within_the_delay() {
        let mut s = st("");
        s = type_at(&s, 0, "a", 10_000);
        s = type_at(&s, 1, "b", 10_100);
        s = type_at(&s, 2, "c", 10_200);
        assert_eq!(s.history.undo_depth(), 1);
        let back = undo(&s).unwrap().state().clone();
        assert_eq!(back.doc.to_string(), "");
        let fwd = redo(&back).unwrap().state().clone();
        assert_eq!(fwd.doc.to_string(), "abc");
    }

    #[test]
    fn splits_groups_after_the_delay() {
        let mut s = st("");
        s = type_at(&s, 0, "a", 10_000);
        s = type_at(&s, 1, "b", 11_000);
        assert_eq!(s.history.undo_depth(), 2);
        assert_eq!(undo(&s).unwrap().state().doc.to_string(), "a");
    }

    #[test]
    fn non_history_changes_map_the_stack() {
        let mut s = st("hello");
        s = type_at(&s, 5, "!", 10_000);
        s = s
            .update(
                TransactionSpec::default()
                    .changes(Change::insert(0, ">> "))
                    .add_to_history(false),
            )
            .state()
            .clone();
        assert_eq!(s.doc.to_string(), ">> hello!");
        let back = undo(&s).unwrap().state().clone();
        assert_eq!(back.doc.to_string(), ">> hello");
    }

    #[test]
    fn selection_undo_restores_previous_selections() {
        let mut s = st("abcdef");
        s = s
            .update(
                TransactionSpec::default()
                    .cursor(3)
                    .user_event("select")
                    .time(10_000),
            )
            .state()
            .clone();
        s = s
            .update(
                TransactionSpec::default()
                    .cursor(5)
                    .user_event("select.pointer")
                    .time(20_000),
            )
            .state()
            .clone();
        let back = undo_selection(&s).unwrap().state().clone();
        assert_eq!(back.selection.main().head(), 3);
    }

    #[test]
    fn serializes_and_restores_history() {
        let mut s = st("x");
        s = type_at(&s, 1, "y", 10_000);
        let json = s.to_json_with_history();
        let restored = EditorState::from_json_with_history(&json, StateConfig::default()).unwrap();
        assert_eq!(restored.history.undo_depth(), 1);
        assert_eq!(undo(&restored).unwrap().state().doc.to_string(), "x");
    }

    #[test]
    fn caps_depth_at_min_depth_plus_slack() {
        let mut s = EditorState::create(
            "",
            StateConfig {
                history_min_depth: 5,
                ..StateConfig::default()
            },
        );
        for i in 0..40 {
            s = type_at(&s, i, "a", 10_000 + i as u64 * 1000);
        }
        assert!(s.history.undo_depth() <= 26);
        assert!(s.history.undo_depth() >= 5);
    }
}
