//! The part of the CodeMirror 5 API (`cm_adapter`'s `CodeMirror` class)
//! vim.js still needs, over an [`EditorView`]. Editor commands run as
//! [`Cmd`](crate::bindings::Cmd)s through [`Cm::run`]. Columns are
//! character indices within a line; changes go through the view like
//! `dispatchChange`, tagged `input.type.compose` so one vim operation
//! undoes as one step.

use std::collections::BTreeMap;

use gpui_kit::{Context, Window};

use super::types::*;
use crate::change::{Change, ChangeDesc, MapMode};
use crate::selection::{EditorSelection, SelectionRange};
use crate::state::{Effect, ScrollStrategy, ScrollTarget, TransactionSpec};
use crate::view::EditorView;
use crate::view_queries::ViewQueries;

/// A bookmark (`Marker`): an offset mapped through every change.
#[derive(Clone, Copy, Debug)]
pub struct Marker {
    pub offset: Option<usize>,
    pub assoc: i32,
}

/// The operation in progress (`curOp`).
#[derive(Clone, Debug, Default)]
pub struct CurOp {
    pub depth: i32,
    pub is_vim_op: bool,
    pub change_start: Option<usize>,
    /// Change texts (`change` / `lastChange.next` chain).
    pub changes: Vec<String>,
    pub cursor_activity: bool,
    /// Whether `change` handlers were registered when the first change
    /// arrived (`changeHandlers` snapshot).
    pub change_handlers: Option<bool>,
}

/// What the adapter keeps between calls (`cm.state` and friends).
#[derive(Debug, Default)]
pub struct CmState {
    pub cur_op: Option<CurOp>,
    pub marks: BTreeMap<u64, Marker>,
    pub last_change_end: usize,
    pub line_handle_changes: Option<Vec<ChangeDesc>>,
    pub overwrite: bool,
    pub textwidth: Option<i64>,
    /// The `change` listener (`onChange`) is registered (insert mode).
    pub listening_changes: bool,
    /// Ranges for `forEachSelection` (`virtualSelection`).
    pub virtual_selection: Option<EditorSelection>,
    /// The query `addOverlay` last installed (`cm6Query`).
    pub cm6_query: Option<std::rc::Rc<crate::search::SearchQuery>>,
}

thread_local! {
    static NEXT_MARKER: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

/// The adapter for one call into vim.
pub struct Cm<'a, 'b> {
    pub view: &'a mut EditorView,
    pub window: &'a mut Window,
    pub cx: &'a mut Context<'b, EditorView>,
}

/// `getSearchCursor`'s cursor.
pub struct SearchCursor {
    source: String,
    ignore_case: bool,
    first_offset: usize,
    last: Option<(usize, usize, Vec<Option<String>>)>,
    after_empty: bool,
    pub from: Option<Pos>,
    pub to: Option<Pos>,
    pub matched: Option<Vec<Option<String>>>,
}

/// `getScrollInfo`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollInfo {
    pub left: f32,
    pub top: f32,
    pub client_height: f32,
}

/// `charCoords`: content-relative box.
#[derive(Clone, Copy, Debug, Default)]
pub struct Coords {
    pub left: f32,
    pub top: f32,
    pub bottom: f32,
}

impl<'a, 'b> Cm<'a, 'b> {
    pub fn new(
        view: &'a mut EditorView,
        window: &'a mut Window,
        cx: &'a mut Context<'b, EditorView>,
    ) -> Self {
        Cm { view, window, cx }
    }

    pub fn st(&mut self) -> &mut CmState {
        &mut self.view.vim.as_mut().expect("vim").cm
    }

    pub fn vim(&mut self) -> &mut super::state::VimState {
        &mut self.view.vim.as_mut().expect("vim").state
    }

    pub fn vim_ref(&self) -> &super::state::VimState {
        &self.view.vim.as_ref().expect("vim").state
    }

    fn doc(&self) -> &crate::text::Text {
        &self.view.state.doc
    }

    // ---- positions ----

    pub fn last_line(&self) -> i64 {
        self.doc().lines() as i64 - 1
    }

    pub fn line_count(&self) -> i64 {
        self.doc().lines() as i64
    }

    pub fn get_line(&self, row: i64) -> String {
        let doc = self.doc();
        if row < 0 || row >= doc.lines() as i64 {
            return String::new();
        }
        doc.line_text(row as usize + 1)
    }

    pub fn line_chars(&self, row: i64) -> Vec<char> {
        self.get_line(row).chars().collect()
    }

    /// `lineLength`: characters in a line.
    pub fn line_len(&self, row: i64) -> i64 {
        self.get_line(row).chars().count() as i64
    }

    /// `indexFromPos`.
    pub fn index_from_pos(&self, pos: Pos) -> usize {
        let doc = self.doc();
        let mut ch = pos.ch;
        let mut n = pos.line + 1;
        if n < 1 {
            n = 1;
            ch = 0;
        }
        if n > doc.lines() as i64 {
            n = doc.lines() as i64;
            ch = INF;
        }
        let line = doc.line(n as usize);
        let text = doc.slice(line.from, line.to);
        let ch = ch.max(0) as usize;
        let byte = text.char_indices().nth(ch).map_or(text.len(), |(b, _)| b);
        (line.from + byte).min(line.to)
    }

    /// `posFromIndex`.
    pub fn pos_from_index(&self, offset: usize) -> Pos {
        let doc = self.doc();
        let offset = doc.clamp_to_char(offset.min(doc.len()));
        let line = doc.line_at(offset);
        let ch = doc.slice(line.from, offset).chars().count();
        Pos::new(line.number as i64 - 1, ch as i64)
    }

    /// `clipPos`.
    pub fn clip_pos(&self, p: Pos) -> Pos {
        let doc = self.doc();
        let mut ch = p.ch;
        let mut n = p.line + 1;
        if n < 1 {
            n = 1;
            ch = 0;
        }
        if n > doc.lines() as i64 {
            n = doc.lines() as i64;
            ch = INF;
        }
        let len = self.line_len(n - 1);
        Pos::new(n - 1, ch.clamp(0, len))
    }

    // ---- selection ----

    pub fn get_cursor(&self) -> Pos {
        self.pos_from_index(self.view.state.selection.main().head())
    }

    pub fn get_anchor(&self) -> Pos {
        self.pos_from_index(self.view.state.selection.main().anchor())
    }

    pub fn get_start(&self) -> Pos {
        self.pos_from_index(self.view.state.selection.main().from)
    }

    pub fn get_end(&self) -> Pos {
        self.pos_from_index(self.view.state.selection.main().to)
    }

    pub fn list_selections(&self) -> Vec<Range> {
        self.view
            .state
            .selection
            .ranges
            .iter()
            .map(|r| Range {
                anchor: self.pos_from_index(r.anchor()),
                head: self.pos_from_index(r.head()),
            })
            .collect()
    }

    fn dispatch(&mut self, spec: TransactionSpec) {
        self.view.dispatch(spec, self.window, self.cx);
    }

    /// `setCursor`.
    pub fn set_cursor(&mut self, p: Pos) {
        let offset = self.index_from_pos(p);
        let scroll = self.st().cur_op.is_none();
        let mut spec = TransactionSpec::default().selection(EditorSelection::cursor(offset));
        if scroll {
            spec = spec.scroll();
        }
        self.dispatch(spec);
        if self.st().cur_op.as_ref().is_some_and(|o| !o.is_vim_op) {
            self.on_before_end_operation();
        }
    }

    /// `setSelections`.
    pub fn set_selections(&mut self, ranges: &[Range], primary: usize) {
        let rs: Vec<SelectionRange> = ranges
            .iter()
            .map(|r| {
                let head = self.index_from_pos(r.head);
                let anchor = self.index_from_pos(r.anchor);
                if head == anchor {
                    SelectionRange::cursor_assoc(head, 1, None)
                } else {
                    SelectionRange::range(anchor, head)
                }
            })
            .collect();
        let primary = primary.min(rs.len().saturating_sub(1));
        self.dispatch(TransactionSpec::default().selection(EditorSelection::create(rs, primary)));
    }

    pub fn set_selection(&mut self, anchor: Pos, head: Pos) {
        self.set_selections(&[Range { anchor, head }], 0);
    }

    pub fn get_range(&self, s: Pos, e: Pos) -> String {
        let (a, b) = (self.index_from_pos(s), self.index_from_pos(e));
        self.view.state.slice_doc(a.min(b), a.max(b))
    }

    /// `dispatchChange`: edits are `input.type.compose`, the first in an
    /// operation `.start`.
    fn dispatch_change(&mut self, mut spec: TransactionSpec) {
        if self.view.state.read_only() {
            return;
        }
        let start = self
            .st()
            .cur_op
            .as_ref()
            .is_some_and(|o| o.changes.is_empty());
        let event = if start {
            "input.type.compose.start"
        } else {
            "input.type.compose"
        };
        let replace = spec
            .annotations
            .user_event
            .as_deref()
            .is_none_or(|e| e == "input");
        if replace {
            spec = spec.user_event(event);
        }
        self.dispatch(spec);
    }

    pub fn replace_range(&mut self, text: &str, s: Pos, e: Option<Pos>) {
        let e = e.unwrap_or(s);
        let (from, to) = (self.index_from_pos(s), self.index_from_pos(e));
        self.dispatch_change(TransactionSpec::default().changes(Change::new(
            from.min(to),
            from.max(to),
            text,
        )));
    }

    pub fn replace_selection(&mut self, text: &str) {
        let spec = self.view.state.replace_selection(text).spec();
        self.dispatch_change(spec);
    }

    pub fn replace_selections(&mut self, texts: &[String]) {
        let changes: Vec<Change> = self
            .view
            .state
            .selection
            .ranges
            .iter()
            .enumerate()
            .map(|(i, r)| Change::new(r.from, r.to, texts.get(i).cloned().unwrap_or_default()))
            .collect();
        self.dispatch_change(TransactionSpec::default().changes(changes));
    }

    pub fn get_selections(&self) -> Vec<String> {
        let s = &self.view.state;
        s.selection
            .ranges
            .iter()
            .map(|r| s.slice_doc(r.from, r.to))
            .collect()
    }

    pub fn get_selection(&self) -> String {
        self.get_selections().join("\n")
    }

    pub fn something_selected(&self) -> bool {
        self.view
            .state
            .selection
            .ranges
            .iter()
            .any(|r| !r.is_empty())
    }

    pub fn focus(&mut self) {
        self.view.focus(self.window, self.cx);
    }

    // ---- geometry ----

    pub fn default_text_height(&self) -> f32 {
        self.view.metrics.line_height()
    }

    fn with_lv<R>(&self, f: impl FnOnce(&crate::display::LayoutView) -> R) -> R {
        self.view.with_layout_view(self.window, f)
    }

    /// `charCoords(pos, 'local' | 'div')`: content-relative box of the
    /// character at `pos`.
    pub fn char_coords(&self, p: Pos) -> Coords {
        let offset = self.index_from_pos(p).min(self.doc().len());
        let tb = self.view.text_box(self.window);
        let pad = self.view.metrics.line_padding_left;
        self.with_lv(|lv| {
            let (top, _) = lv.row_box(offset, 1);
            let x = lv.x_at(offset, 1);
            Coords {
                left: pad + x,
                top: top + tb.top(),
                bottom: top + tb.top() + tb.height(),
            }
        })
    }

    /// `coordsChar`.
    pub fn coords_char(&self, left: f32, top: f32) -> Pos {
        let pad = self.view.metrics.line_padding_left;
        let offset = self.with_lv(|lv| lv.pos_at(left - pad, top));
        self.pos_from_index(offset)
    }

    pub fn get_scroll_info(&self) -> ScrollInfo {
        let f = self.view.frame.clone().unwrap_or_default();
        ScrollInfo {
            left: self.view.scroll_left,
            top: self.view.scroll_top,
            client_height: f.scroller_height,
        }
    }

    pub fn scroll_to(&mut self, x: Option<f32>, y: Option<f32>) {
        let f = self.view.frame.clone().unwrap_or_default();
        if let Some(x) = x {
            self.view.scroll_left = x.clamp(0.0, f.max_scroll_left.max(0.0));
        }
        if let Some(y) = y {
            self.view.scroll_top = y.clamp(0.0, f.max_scroll_top.max(0.0));
        }
        self.cx.notify();
    }

    /// `scrollIntoView`.
    pub fn scroll_into_view(&mut self, pos: Option<Pos>) {
        match pos {
            Some(p) => {
                let offset = self.index_from_pos(p);
                self.dispatch(TransactionSpec::default().effect(Effect::ScrollIntoView(
                    ScrollTarget::pos(offset, ScrollStrategy::Nearest, 5.0),
                )));
            }
            None => self.dispatch(TransactionSpec::default().scroll().user_event("scroll")),
        }
    }

    /// `findPosV`: `amount` lines or pages; `hit_side` when clipped.
    pub fn find_pos_v(
        &mut self,
        start: Pos,
        amount: i64,
        page: bool,
        goal: Option<f32>,
    ) -> (Pos, bool) {
        let doc_len = self.doc().len();
        let start_offset = self.index_from_pos(start);
        let pad = self.view.metrics.line_padding_left;
        let pixels = if page {
            let f = self.view.frame.clone().unwrap_or_default();
            f32::from(f.bounds.size.height)
        } else {
            0.0
        };
        let mut range = SelectionRange::cursor_assoc(start_offset, 1, goal.map(|g| g - pad));
        let count = amount.abs();
        for _ in 0..count {
            range =
                self.with_lv(|lv| lv.move_vertically(range, amount > 0, page.then_some(pixels)));
        }
        let pos = self.pos_from_index(range.head());
        let goal_ch = goal.map(|g| g as i64);
        let hit = (amount < 0
            && range.head() == 0
            && goal != Some(0.0)
            && start.line == 0
            && start.ch != 0)
            || (amount > 0
                && range.head() == doc_len
                && Some(pos.ch) != goal_ch
                && start.line == pos.line);
        (pos, hit)
    }

    // ---- brackets / indent / commands ----

    /// `findMatchingBracket`.
    pub fn find_matching_bracket(&self, pos: Pos) -> Option<Pos> {
        let state = &self.view.state;
        let offset = self.index_from_pos(pos);
        if let Some(m) = crate::brackets::match_brackets(state, offset + 1, -1)
            && let Some(end) = m.end
        {
            return Some(self.pos_from_index(end.0));
        }
        if let Some(m) = crate::brackets::match_brackets(state, offset, 1)
            && let Some(end) = m.end
        {
            return Some(self.pos_from_index(end.0));
        }
        None
    }

    /// `scanForBracket`: `Err(())` is the package's `false` (hit the
    /// document edge), `Ok(None)` its `null`.
    pub fn scan_for_bracket(
        &self,
        where_: Pos,
        dir: i64,
        brackets: &str,
    ) -> Result<Option<(Pos, char)>, ()> {
        let max_scan_len = 10000;
        let max_lines = 1000;
        let mut stack: Vec<char> = Vec::new();
        let line_end = if dir > 0 {
            (where_.line + max_lines).min(self.last_line() + 1)
        } else {
            (-1).max(where_.line - max_lines)
        };
        let matching = |c: char| -> Option<&'static str> {
            Some(match c {
                '(' => ")>",
                ')' => "(<",
                '[' => "]>",
                ']' => "[<",
                '{' => "}>",
                '}' => "{<",
                '<' => ">>",
                '>' => "<<",
                _ => return None,
            })
        };
        let mut line_no = where_.line;
        while line_no != line_end {
            let line = self.line_chars(line_no);
            if !line.is_empty() && line.len() <= max_scan_len {
                let len = line.len() as i64;
                let (mut pos, end) = if dir > 0 { (0, len) } else { (len - 1, -1) };
                if line_no == where_.line {
                    pos = where_.ch - if dir < 0 { 1 } else { 0 };
                }
                while pos != end {
                    if pos < 0 || pos >= len {
                        break;
                    }
                    let ch = line[pos as usize];
                    if brackets.contains(ch) {
                        let m = matching(ch);
                        if m.is_some_and(|m| (m.as_bytes()[1] == b'>') == (dir > 0)) {
                            stack.push(ch);
                        } else if stack.is_empty() {
                            return Ok(Some((Pos::new(line_no, pos), ch)));
                        } else {
                            stack.pop();
                        }
                    }
                    pos += dir;
                }
            }
            line_no += dir;
        }
        if line_no - dir == if dir > 0 { self.last_line() } else { 0 } {
            Err(())
        } else {
            Ok(None)
        }
    }

    pub fn run(&mut self, cmd: crate::bindings::Cmd) {
        self.view.run_cmd(cmd, self.window, self.cx);
    }

    /// `CodeMirror.commands.newlineAndIndent`.
    pub fn newline_and_indent(&mut self) {
        let spec = self.view.with_layout_view(self.window, |lv| {
            crate::commands::insert_newline_and_indent(lv)
        });
        if let Some(spec) = spec {
            self.dispatch_change(spec);
        }
    }

    /// `runHistoryCommand`: undo / redo, then put the cursor at the start
    /// of the changed text.
    pub fn run_history(&mut self, undo: bool) {
        if let Some(op) = self.st().cur_op.as_mut() {
            op.change_start = None;
        }
        let state = self.view.state.clone();
        let tr = if undo {
            crate::history::undo(&state)
        } else {
            crate::history::redo(&state)
        };
        if let Some(tr) = tr {
            self.view.apply(std::rc::Rc::new(tr), self.window, self.cx);
        }
        if let Some(start) = self.st().cur_op.as_ref().and_then(|o| o.change_start) {
            self.dispatch(TransactionSpec::default().selection(EditorSelection::cursor(start)));
        }
    }

    // ---- bookmarks ----

    /// `setBookmark`; `insert_left` keeps the mark before text inserted at
    /// it.
    pub fn set_bookmark(&mut self, pos: Pos, insert_left: bool) -> u64 {
        let offset = self.index_from_pos(pos);
        let id = NEXT_MARKER.with(|n| {
            let id = n.get();
            n.set(id + 1);
            id
        });
        self.st().marks.insert(
            id,
            Marker {
                offset: Some(offset),
                assoc: if insert_left { 1 } else { -1 },
            },
        );
        id
    }

    pub fn mark_find(&self, id: u64) -> Option<Pos> {
        let m = self.view.vim.as_ref()?.cm.marks.get(&id)?;
        Some(self.pos_from_index(m.offset?))
    }

    pub fn mark_clear(&mut self, id: u64) {
        self.st().marks.remove(&id);
    }

    // ---- overlays ----

    /// `addOverlay`: highlight a vim query through the search query.
    pub fn add_overlay(&mut self, source: &str, flags: &str) {
        let mut q = crate::search::SearchQuery::new(crate::search::SearchQueryConfig {
            regexp: true,
            search: source.to_string(),
            case_sensitive: !flags.contains('i'),
            ..Default::default()
        });
        if q.valid {
            q.for_vim = true;
            let q = std::rc::Rc::new(q);
            self.st().cm6_query = Some(q.clone());
            self.dispatch(
                TransactionSpec::default()
                    .effect(Effect::Search(crate::search::SearchEffect::SetQuery(q))),
            );
        }
    }

    pub fn remove_overlay(&mut self) {
        let Some(q) = self.st().cm6_query.clone() else {
            return;
        };
        let mut q = (*q).clone();
        q.for_vim = false;
        let q = std::rc::Rc::new(q);
        self.st().cm6_query = Some(q.clone());
        self.dispatch(
            TransactionSpec::default()
                .effect(Effect::Search(crate::search::SearchEffect::SetQuery(q))),
        );
    }

    // ---- search cursor ----

    /// `getSearchCursor`.
    pub fn get_search_cursor(&self, query: &super::search::VimRegex, pos: Pos) -> SearchCursor {
        let first_offset = self.index_from_pos(pos);
        SearchCursor {
            source: super::search::cm6_source(&query.source),
            ignore_case: query.ignore_case(),
            first_offset,
            last: None,
            after_empty: false,
            from: None,
            to: None,
            matched: None,
        }
    }

    pub fn cursor_find(&self, c: &mut SearchCursor, back: bool) -> bool {
        let doc = self.doc();
        let next = |from: usize| -> Option<(usize, usize, Vec<Option<String>>)> {
            if from > doc.len() {
                return None;
            }
            let mut cur =
                crate::search::RegExpCursor::new(doc, &c.source, c.ignore_case, from, doc.len());
            cur.next_match()
                .map(|m| (m.from, m.to, m.groups.unwrap_or_default()))
        };
        let prev_in_range =
            |from: usize, to: usize| -> Option<(usize, usize, Vec<Option<String>>)> {
                let chunk = 10000;
                let mut size = 1;
                loop {
                    let start = from.max(to.saturating_sub(size * chunk));
                    let mut cur =
                        crate::search::RegExpCursor::new(doc, &c.source, c.ignore_case, start, to);
                    let mut range = None;
                    while let Some(m) = cur.next_match() {
                        range = Some((m.from, m.to, m.groups.unwrap_or_default()));
                    }
                    if let Some(r) = range.as_ref()
                        && (start == from || r.0 > start + 10)
                    {
                        return range;
                    }
                    if start == from {
                        return None;
                    }
                    size += 1;
                }
            };
        if back {
            let end_at = match c.last.as_ref() {
                Some(l) => {
                    if c.after_empty {
                        l.1.saturating_sub(1)
                    } else {
                        l.0
                    }
                }
                None => c.first_offset,
            };
            c.last = prev_in_range(0, end_at);
        } else {
            let start_from = match c.last.as_ref() {
                Some(l) => {
                    if c.after_empty {
                        l.1 + 1
                    } else {
                        l.1
                    }
                }
                None => c.first_offset,
            };
            c.last = next(start_from);
        }
        match c.last.clone() {
            Some((f, t, groups)) => {
                c.from = Some(self.pos_from_index(f));
                c.to = Some(self.pos_from_index(t));
                c.matched = Some(if groups.is_empty() {
                    vec![Some(doc.slice(f, t))]
                } else {
                    groups
                });
                c.after_empty = f == t;
                true
            }
            None => {
                c.from = None;
                c.to = None;
                c.matched = None;
                c.after_empty = false;
                false
            }
        }
    }

    /// The cursor's match was empty (`!found[0]`).
    pub fn cursor_empty_match(c: &SearchCursor) -> bool {
        c.matched
            .as_ref()
            .and_then(|m| m.first().cloned().flatten())
            .is_none_or(|s| s.is_empty())
    }

    pub fn cursor_replace(&mut self, c: &mut SearchCursor, text: &str) {
        if let Some((f, t, _)) = c.last.clone() {
            self.dispatch_change(TransactionSpec::default().changes(Change::new(f, t, text)));
            let new_to = f + text.len();
            if let Some(l) = c.last.as_mut() {
                l.1 = new_to;
            }
            c.to = Some(self.pos_from_index(new_to));
        }
    }

    // ---- operations ----

    pub fn last_edit_end(&self) -> Pos {
        let off = self.view.vim.as_ref().map_or(0, |v| v.cm.last_change_end);
        self.pos_from_index(off)
    }

    /// `operation(fn)`: batch change / cursor events until the outermost
    /// operation ends.
    pub fn op_begin(&mut self) {
        let st = self.st();
        let op = st.cur_op.get_or_insert_with(CurOp::default);
        op.depth += 1;
    }

    pub fn op_end(&mut self) {
        let done = match self.st().cur_op.as_mut() {
            Some(op) => {
                op.depth -= 1;
                op.depth <= 0
            }
            None => false,
        };
        if done {
            self.on_before_end_operation();
        }
    }

    pub fn set_vim_op(&mut self) {
        if let Some(op) = self.st().cur_op.as_mut() {
            op.is_vim_op = true;
        }
    }

    /// `onBeforeEndOperation`: deliver change and cursor activity.
    pub fn on_before_end_operation(&mut self) {
        let Some(op) = self.st().cur_op.take() else {
            return;
        };
        let mut scroll = false;
        if !op.changes.is_empty() && op.change_handlers == Some(true) {
            super::insert::on_change(self, &op.changes);
        }
        if op.cursor_activity {
            super::insert::on_cursor_activity(self, op.is_vim_op);
            if op.is_vim_op {
                scroll = true;
            }
        }
        if scroll {
            self.scroll_into_view(None);
        }
    }

    // ---- options ----

    pub fn tab_size(&self) -> i64 {
        self.view.state.tab_size() as i64
    }

    pub fn read_only(&self) -> bool {
        self.view.state.read_only()
    }

    pub fn indent_with_tabs(&self) -> bool {
        self.view.state.indent_unit() == "\t"
    }

    pub fn toggle_overwrite(&mut self, on: bool) {
        self.st().overwrite = on;
    }

    pub fn overwrite(&self) -> bool {
        self.view.vim.as_ref().is_some_and(|v| v.cm.overwrite)
    }

    /// `overWriteSelection`.
    pub fn overwrite_selection(&mut self, text: &str) {
        let doc_len = self.doc().len();
        let sel = self.view.state.selection.clone();
        let ranges: Vec<SelectionRange> = sel
            .ranges
            .iter()
            .map(|x| {
                if x.is_empty() && x.to < doc_len {
                    let next = self
                        .doc()
                        .clamp_to_char(x.to + 1)
                        .max(x.to + 1)
                        .min(doc_len);
                    let ch = self.doc().slice(x.to, next);
                    if !ch.is_empty() && !ch.contains('\n') {
                        return SelectionRange::range(x.from, next);
                    }
                }
                *x
            })
            .collect();
        self.dispatch(
            TransactionSpec::default().selection(EditorSelection::create(ranges, sel.main_index)),
        );
        self.replace_selection(text);
    }

    pub fn in_multi_select(&self) -> bool {
        self.view.state.selection.ranges.len() > 1
    }

    // ---- line handles ----

    pub fn line_handle(&mut self, row: i64) -> usize {
        if self.st().line_handle_changes.is_none() {
            self.st().line_handle_changes = Some(Vec::new());
        }
        self.index_from_pos(Pos::new(row, 0))
    }

    pub fn line_number(&self, handle: usize) -> Option<i64> {
        let updates = self.view.vim.as_ref()?.cm.line_handle_changes.as_ref()?;
        let mut offset = handle;
        for u in updates {
            offset = u.map_pos_mode(offset, 1, MapMode::TrackAfter)?;
        }
        let p = self.pos_from_index(offset);
        (p.ch == 0).then_some(p.line)
    }

    pub fn release_line_handles(&mut self) {
        self.st().line_handle_changes = None;
    }

    // ---- hard wrap ----

    /// `hardWrap({from, to})`; returns the row after the last wrapped one.
    pub fn hard_wrap(&mut self, from: i64, to: i64) -> i64 {
        let max = self.st().textwidth.unwrap_or(80);
        let mut row = from.min(to);
        let mut end_row = from.max(to);
        while row <= end_row {
            let line = self.line_chars(row);
            if line.len() as i64 > max {
                if let Some((start, end)) = find_space(&line, max as usize, 5) {
                    let indentation: String = line
                        .iter()
                        .take_while(|c| crate::state::is_js_space(**c))
                        .collect();
                    self.replace_range(
                        &format!("\n{indentation}"),
                        Pos::new(row, start as i64),
                        Some(Pos::new(row, end as i64)),
                    );
                }
                end_row += 1;
            } else if line.iter().any(|c| !crate::state::is_js_space(*c)) && row != end_row {
                let next = self.line_chars(row + 1);
                if !next.is_empty() && next.iter().any(|c| !crate::state::is_js_space(*c)) {
                    let trimmed_len = line.len()
                        - line
                            .iter()
                            .rev()
                            .take_while(|c| crate::state::is_js_space(**c))
                            .count();
                    let next_lead = next
                        .iter()
                        .take_while(|c| crate::state::is_js_space(**c))
                        .count();
                    let mut merged: Vec<char> = line[..trimmed_len].to_vec();
                    merged.push(' ');
                    merged.extend_from_slice(&next[next_lead..]);
                    let space = find_space(&merged, max as usize, 5);
                    if space.is_some_and(|s| s.0 > trimmed_len) || (merged.len() as i64) < max {
                        self.replace_range(
                            " ",
                            Pos::new(row, trimmed_len as i64),
                            Some(Pos::new(row + 1, next_lead as i64)),
                        );
                        row -= 1;
                        end_row -= 1;
                    } else if trimmed_len < line.len() {
                        self.replace_range(
                            "",
                            Pos::new(row, trimmed_len as i64),
                            Some(Pos::new(row, line.len() as i64)),
                        );
                    }
                }
            }
            row += 1;
        }
        row
    }
}

/// `hardWrap`'s `findSpace`.
fn find_space(line: &[char], max: usize, min: usize) -> Option<(usize, usize)> {
    if line.len() < max {
        return None;
    }
    let ws = |c: char| crate::state::is_js_space(c);
    let before = &line[..max];
    let after = &line[max..];
    // spaceAfter: /^(?:(\s+)|(\S+)(\s+))/
    let lead_ws = after.iter().take_while(|c| ws(**c)).count();
    let space_after = if lead_ws > 0 {
        Some(Space::Ws(lead_ws))
    } else {
        let word = after.iter().take_while(|c| !ws(**c)).count();
        let sp = after[word..].iter().take_while(|c| ws(**c)).count();
        (word > 0 && sp > 0).then_some(Space::Word { index: word, sp })
    };
    // spaceBefore: /(?:(\s+)|(\s+)(\S+))$/
    let trail_ws = before.iter().rev().take_while(|c| ws(**c)).count();
    let space_before = if trail_ws > 0 {
        Some(Space::Ws(trail_ws))
    } else {
        let word = before.iter().rev().take_while(|c| !ws(**c)).count();
        let sp = before[..before.len() - word]
            .iter()
            .rev()
            .take_while(|c| ws(**c))
            .count();
        (word > 0 && sp > 0).then_some(Space::Word {
            index: before.len() - word - sp,
            sp,
        })
    };
    let mut start = 0;
    let mut end = 0;
    if let Some(Space::Ws(n)) = space_before {
        start = max - n;
        end = max;
    }
    if let Some(Space::Ws(n)) = space_after {
        if start == 0 {
            start = max;
        }
        end = max + n;
    }
    if start != 0 {
        return Some((start, end));
    }
    if let Some(Space::Word { index, sp }) = space_before
        && index > min
    {
        return Some((index, index + sp));
    }
    if let Some(Space::Word { index, sp }) = space_after {
        let start = max + index;
        return Some((start, start + sp));
    }
    None
}

/// `findSpace`'s match: a whitespace run, or a word and the spaces
/// beside it (`index` is where the spaces start, or the word length after).
enum Space {
    Ws(usize),
    Word { index: usize, sp: usize },
}

/// The plugin's `update(ViewUpdate)` half that feeds the adapter: changes
/// map marks and record into `curOp`; selection changes flag cursor
/// activity; outside a vim operation the events deliver right away.
pub(crate) fn view_updated(
    view: &mut EditorView,
    tr: &crate::state::Transaction,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    let Some(vim) = view.vim.as_mut() else { return };
    let st = &mut vim.cm;
    if tr.doc_changed() {
        let desc = tr.changes.desc();
        if let Some(v) = st.line_handle_changes.as_mut() {
            v.push(desc.clone());
        }
        for m in st.marks.values_mut() {
            if let Some(o) = m.offset {
                m.offset = desc.map_pos_mode(o, m.assoc, MapMode::TrackDel);
            }
        }
        if let Some(vs) = st.virtual_selection.as_mut() {
            *vs = vs.map(&desc, -1);
        }
        let listening = st.listening_changes;
        let op = st.cur_op.get_or_insert_with(CurOp::default);
        let mut last_end = None;
        tr.changes.iter_changes(true, |_fa, _ta, fb, tb, text| {
            if op.change_start.is_none_or(|s| s > fb) {
                op.change_start = Some(fb);
            }
            last_end = Some(tb);
            op.changes.push(text.to_string());
        });
        if op.change_handlers.is_none() {
            op.change_handlers = Some(listening);
        }
        if let Some(e) = last_end {
            st.last_change_end = e;
        }
    }
    if tr.selection.is_some() {
        let op = st.cur_op.get_or_insert_with(CurOp::default);
        op.cursor_activity = true;
    }
    for e in &tr.effects {
        if let Effect::Search(crate::search::SearchEffect::SetQuery(q)) = e {
            vim.highlight = q.for_vim.then(|| q.clone());
        }
    }
    let deliver = vim.cm.cur_op.as_ref().is_some_and(|o| !o.is_vim_op);
    if deliver {
        let mut cm = Cm::new(view, window, cx);
        cm.on_before_end_operation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_wrap_finds_spaces() {
        let s: Vec<char> = "aaaa bbbb cccc".chars().collect();
        assert_eq!(find_space(&s, 7, 2), Some((4, 5)));
        let s: Vec<char> = "aaaa  bbbb".chars().collect();
        assert_eq!(find_space(&s, 5, 2), Some((4, 6)));
    }
}
