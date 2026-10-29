//! Input for [`EditorView`]: keys through the CodeMirror keymap, commands,
//! text input and IME composition, mouse selection (`pointerMouseSelection`,
//! `rectangularSelection`, drag and drop), gutter clicks, wheel and
//! scrollbar scrolling, the clipboard and hover tooltips.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{
    Bounds, ClipboardItem, Context, EntityInputHandler, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, UTF16Selection, Window, point,
    px, size,
};

use crate::bindings::Cmd;
use crate::change::Change;
use crate::clock::now_ms;
use crate::commands::{self as c, Command};
use crate::element::GutterKind;
use crate::keymap::{KeyEvent, Platform};
use crate::search::{self, SearchFlag};
use crate::selection::{EditorSelection, SelectionRange};
use crate::state::{Effect, RangeChange, Transaction, TransactionSpec};
use crate::view::{EditorEvent, EditorKind, EditorView};
use crate::view_queries::ViewQueries;

/// `POINTER_SLOP_PX`: moves this small keep the mousedown position.
const POINTER_SLOP_PX: f32 = 4.0;
/// Pointer travel (`dist`) before a press inside the selection drags it.
const DRAG_START_PX: f32 = 10.0;
/// `dragScrollMargin`.
const DRAG_SCROLL_MARGIN: f32 = 6.0;
/// `rectangleFor`'s `MaxOff`.
const MAX_OFF: usize = 2000;
/// Chromium pages a scrollbar track click by this fraction of the view.
const TRACK_PAGE_FRACTION: f32 = 0.875;

thread_local! {
    /// `lastLinewiseCopy`: shared by every editor, like CodeMirror's module
    /// global.
    static LAST_LINEWISE_COPY: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn drag_scroll_speed(dist: f32) -> f32 {
    dist.max(0.0) * 0.7 + 8.0
}

/// A rectangular-selection corner (`getPos`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectPos {
    pub line: usize,
    pub col: isize,
    pub off: usize,
}

/// `rectangleFor`.
pub fn rectangle_for(
    state: &crate::state::EditorState,
    a: RectPos,
    b: RectPos,
) -> Vec<SelectionRange> {
    let (start_line, end_line) = (a.line.min(b.line), a.line.max(b.line));
    let mut ranges = Vec::new();
    if a.off > MAX_OFF || b.off > MAX_OFF || a.col < 0 || b.col < 0 {
        let (start_off, end_off) = (a.off.min(b.off), a.off.max(b.off));
        for i in start_line..=end_line {
            let line = state.doc.line(i);
            if line.len() <= end_off {
                ranges.push(SelectionRange::range(
                    line.from + start_off,
                    line.to + end_off,
                ));
            }
        }
    } else {
        let (start_col, end_col) = (a.col.min(b.col) as usize, a.col.max(b.col) as usize);
        for i in start_line..=end_line {
            let line = state.doc.line(i);
            let text = state.doc.slice(line.from, line.to);
            match crate::text::find_column(&text, start_col, state.tab_size(), true) {
                None => ranges.push(SelectionRange::cursor(line.to)),
                Some(start) => {
                    let end = crate::text::find_column(&text, end_col, state.tab_size(), false)
                        .unwrap_or(text.len());
                    ranges.push(SelectionRange::range(line.from + start, line.from + end));
                }
            }
        }
    }
    ranges
}

/// `selectionForClick`.
pub fn selection_for_click(
    state: &crate::state::EditorState,
    pos: usize,
    assoc: i32,
    kind: usize,
) -> SelectionRange {
    if kind <= 1 {
        return SelectionRange::cursor_assoc(pos, assoc, None);
    }
    if kind == 2 {
        return state
            .word_at(pos)
            .map_or(SelectionRange::cursor_assoc(pos, assoc, None), |w| {
                SelectionRange::range(w.from, w.to)
            });
    }
    let line = state.doc.line_at(pos);
    let mut to = line.to;
    if to < state.doc.len() {
        to += 1;
    }
    SelectionRange::range(line.from, to)
}

/// `copiedRange`: the selected text, or whole lines when nothing is
/// selected.
pub fn copied_range(state: &crate::state::EditorState) -> (String, Vec<(usize, usize)>, bool) {
    let mut content = Vec::new();
    let mut ranges = Vec::new();
    for r in &state.selection.ranges {
        if !r.is_empty() {
            content.push(state.slice_doc(r.from, r.to));
            ranges.push((r.from, r.to));
        }
    }
    let mut linewise = false;
    if content.is_empty() {
        let mut upto = 0;
        for r in &state.selection.ranges {
            let line = state.doc.line_at(r.from);
            if line.number > upto {
                content.push(state.doc.slice(line.from, line.to));
                ranges.push((line.from, (line.to + 1).min(state.doc.len())));
            }
            upto = line.number;
        }
        linewise = true;
    }
    (content.join(state.line_break()), ranges, linewise)
}

/// `doPaste`.
pub fn paste_spec(state: &crate::state::EditorState, input: &str) -> TransactionSpec {
    let input = crate::text::normalize_newlines(input).into_owned();
    let text = crate::text::Text::of(&input);
    let ranges = &state.selection.ranges;
    let by_line = text.lines() == ranges.len();
    let linewise = ranges.iter().all(SelectionRange::is_empty)
        && LAST_LINEWISE_COPY.with(|c| c.borrow().as_deref() == Some(input.as_str()));
    let mut i = 1;
    let spec = if linewise {
        let mut last_line = usize::MAX;
        state
            .change_by_range(|range| {
                let line = state.doc.line_at(range.from);
                if line.from == last_line {
                    return RangeChange::range(range);
                }
                last_line = line.from;
                let insert = format!(
                    "{}{}",
                    if by_line {
                        text.line_text(i)
                    } else {
                        input.clone()
                    },
                    state.line_break()
                );
                i += 1;
                let len = insert.len();
                RangeChange::with(
                    vec![Change::insert(line.from, insert)],
                    SelectionRange::cursor(range.from + len),
                )
            })
            .spec()
    } else if by_line {
        state
            .change_by_range(|range| {
                let line = text.line_text(i);
                i += 1;
                let len = line.len();
                RangeChange::with(
                    vec![Change::new(range.from, range.to, line)],
                    SelectionRange::cursor(range.from + len),
                )
            })
            .spec()
    } else {
        state.replace_selection(&input).spec()
    };
    spec.user_event("input.paste").scroll()
}

/// What the mouse is doing between press and release.
#[derive(Clone, Debug)]
pub(crate) enum MouseDrag {
    /// `pointerMouseSelection` (or the drag-to-move pending state).
    Select {
        start_pos: usize,
        start_assoc: i32,
        start_point: Point<Pixels>,
        last_point: Point<Pixels>,
        kind: usize,
        start_sel: EditorSelection,
        extend: bool,
        multiple: bool,
        /// `None` while a press inside the primary selection may still
        /// become a text drag; `Some(true)` dragging text; `Some(false)`
        /// selecting.
        dragging: Option<bool>,
        /// Mod-click without movement runs go-to-definition on release.
        mod_click: bool,
    },
    /// `rectangleSelectionStyle`.
    Rect {
        start: RectPos,
        start_sel: EditorSelection,
        multiple: bool,
        last_point: Point<Pixels>,
    },
    /// A gutter press; the action runs on release over the same cell.
    Gutter { kind: GutterKind, line: usize },
    /// Dragging a scrollbar thumb: grab offset along the track.
    Thumb { vertical: bool, grab: f32 },
    /// Press on the revert chip.
    Chip,
}

/// An overlay scroller's box and knob, in window coordinates along its axis.
pub(crate) struct OverlayBar {
    pub area: Bounds<Pixels>,
    pub start: f32,
    pub track: f32,
    pub total: f32,
    pub visible: f32,
    pub knob_len: f32,
    /// From `start`.
    pub knob_at: f32,
}

impl EditorView {
    // ---- commands ----

    fn view_cmd(&mut self, f: Command, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let spec = self.with_layout_view(window, |lv| f(lv));
        self.run_spec(spec, window, cx)
    }

    pub(crate) fn run_spec(
        &mut self,
        spec: Option<TransactionSpec>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match spec {
            Some(s) => {
                self.dispatch(s, window, cx);
                true
            }
            None => false,
        }
    }

    fn run_tr(
        &mut self,
        tr: Option<Transaction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match tr {
            Some(tr) => {
                self.apply(Rc::new(tr), window, cx);
                true
            }
            None => false,
        }
    }

    /// Run one keymap command (`Command`); true when it applied.
    pub(crate) fn run_cmd(
        &mut self,
        cmd: Cmd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use Cmd::*;
        let state = self.state.clone();
        match cmd {
            CursorCharLeft => self.view_cmd(c::cursor_char_left, window, cx),
            CursorCharRight => self.view_cmd(c::cursor_char_right, window, cx),
            SelectCharLeft => self.view_cmd(c::select_char_left, window, cx),
            SelectCharRight => self.view_cmd(c::select_char_right, window, cx),
            CursorGroupLeft => self.view_cmd(c::cursor_group_left, window, cx),
            CursorGroupRight => self.view_cmd(c::cursor_group_right, window, cx),
            SelectGroupLeft => self.view_cmd(c::select_group_left, window, cx),
            SelectGroupRight => self.view_cmd(c::select_group_right, window, cx),
            CursorLineBoundaryLeft => self.view_cmd(c::cursor_line_boundary_left, window, cx),
            CursorLineBoundaryRight => self.view_cmd(c::cursor_line_boundary_right, window, cx),
            SelectLineBoundaryLeft => self.view_cmd(c::select_line_boundary_left, window, cx),
            SelectLineBoundaryRight => self.view_cmd(c::select_line_boundary_right, window, cx),
            CursorLineBoundaryBackward => {
                self.view_cmd(c::cursor_line_boundary_backward, window, cx)
            }
            CursorLineBoundaryForward => self.view_cmd(c::cursor_line_boundary_forward, window, cx),
            SelectLineBoundaryBackward => {
                self.view_cmd(c::select_line_boundary_backward, window, cx)
            }
            SelectLineBoundaryForward => self.view_cmd(c::select_line_boundary_forward, window, cx),
            CursorLineUp => self.view_cmd(c::cursor_line_up, window, cx),
            CursorLineDown => self.view_cmd(c::cursor_line_down, window, cx),
            SelectLineUp => self.view_cmd(c::select_line_up, window, cx),
            SelectLineDown => self.view_cmd(c::select_line_down, window, cx),
            CursorDocStart => self.view_cmd(c::cursor_doc_start, window, cx),
            CursorDocEnd => self.view_cmd(c::cursor_doc_end, window, cx),
            SelectDocStart => self.view_cmd(c::select_doc_start, window, cx),
            SelectDocEnd => self.view_cmd(c::select_doc_end, window, cx),
            CursorPageUp => self.view_cmd(c::cursor_page_up, window, cx),
            CursorPageDown => self.view_cmd(c::cursor_page_down, window, cx),
            SelectPageUp => self.view_cmd(c::select_page_up, window, cx),
            SelectPageDown => self.view_cmd(c::select_page_down, window, cx),
            CursorLineStart => self.view_cmd(c::cursor_line_start, window, cx),
            CursorLineEnd => self.view_cmd(c::cursor_line_end, window, cx),
            SelectLineStart => self.view_cmd(c::select_line_start, window, cx),
            SelectLineEnd => self.view_cmd(c::select_line_end, window, cx),
            CursorSyntaxLeft => self.view_cmd(c::cursor_syntax_left, window, cx),
            CursorSyntaxRight => self.view_cmd(c::cursor_syntax_right, window, cx),
            SelectSyntaxLeft => self.view_cmd(c::select_syntax_left, window, cx),
            SelectSyntaxRight => self.view_cmd(c::select_syntax_right, window, cx),
            MoveLineUp => self.view_cmd(c::move_line_up, window, cx),
            MoveLineDown => self.view_cmd(c::move_line_down, window, cx),
            CopyLineUp => self.view_cmd(c::copy_line_up, window, cx),
            CopyLineDown => self.view_cmd(c::copy_line_down, window, cx),
            AddCursorAbove => self.view_cmd(c::add_cursor_above, window, cx),
            AddCursorBelow => self.view_cmd(c::add_cursor_below, window, cx),
            SimplifySelection => self.view_cmd(c::simplify_selection, window, cx),
            InsertBlankLine => self.view_cmd(c::insert_blank_line, window, cx),
            SelectLine => self.view_cmd(c::select_line, window, cx),
            SelectParentSyntax => self.view_cmd(c::select_parent_syntax, window, cx),
            IndentLess => self.view_cmd(c::indent_less, window, cx),
            IndentMore => self.view_cmd(c::indent_more, window, cx),
            IndentSelection => self.view_cmd(c::indent_selection, window, cx),
            DeleteLine => self.view_cmd(c::delete_line, window, cx),
            CursorMatchingBracket => self.view_cmd(c::cursor_matching_bracket, window, cx),
            ToggleComment => self.view_cmd(c::toggle_comment, window, cx),
            ToggleBlockComment => self.view_cmd(c::toggle_block_comment, window, cx),
            ToggleTabFocusMode => {
                self.tab_focus = if self.tab_focus < 0 { 0 } else { -1 };
                true
            }
            InsertNewlineAndIndent => self.view_cmd(c::insert_newline_and_indent, window, cx),
            SelectAll => self.view_cmd(c::select_all, window, cx),
            DeleteCharBackward => self.view_cmd(c::delete_char_backward, window, cx),
            DeleteCharForward => self.view_cmd(c::delete_char_forward, window, cx),
            DeleteGroupBackward => self.view_cmd(c::delete_group_backward, window, cx),
            DeleteGroupForward => self.view_cmd(c::delete_group_forward, window, cx),
            DeleteLineBoundaryBackward => {
                self.view_cmd(c::delete_line_boundary_backward, window, cx)
            }
            DeleteLineBoundaryForward => self.view_cmd(c::delete_line_boundary_forward, window, cx),
            DeleteToLineEnd => self.view_cmd(c::delete_to_line_end, window, cx),
            SplitLine => self.view_cmd(c::split_line, window, cx),
            TransposeChars => self.view_cmd(c::transpose_chars, window, cx),
            Undo => self.run_tr(crate::history::undo(&state), window, cx),
            Redo => self.run_tr(crate::history::redo(&state), window, cx),
            UndoSelection => self.run_tr(crate::history::undo_selection(&state), window, cx),
            RedoSelection => self.run_tr(crate::history::redo_selection(&state), window, cx),
            FoldCode => self.view_cmd(crate::fold::fold_code, window, cx),
            UnfoldCode => self.view_cmd(crate::fold::unfold_code, window, cx),
            FoldAll => self.view_cmd(crate::fold::fold_all, window, cx),
            UnfoldAll => self.run_spec(crate::fold::unfold_all(&state), window, cx),
            OpenSearchPanel => self.open_search_panel(window, cx),
            FindNext => self.run_search_run(search::find_next(&state), window, cx),
            FindPrevious => self.run_search_run(search::find_previous(&state), window, cx),
            CloseSearchPanel => {
                let ran = self.run_spec(search::close_search_panel(&state), window, cx);
                if ran {
                    self.focus(window, cx);
                }
                ran
            }
            SelectSelectionMatches => {
                self.run_spec(search::select_selection_matches(&state), window, cx)
            }
            SelectNextOccurrence => {
                self.run_spec(search::select_next_occurrence(&state), window, cx)
            }
            GotoLine => {
                self.open_goto(window, cx);
                true
            }
            ToggleCase => self.run_spec(
                search::toggle_search_flag(&state, SearchFlag::CaseSensitive),
                window,
                cx,
            ),
            ToggleRegexp => self.run_spec(
                search::toggle_search_flag(&state, SearchFlag::Regexp),
                window,
                cx,
            ),
            ToggleWholeWord => self.run_spec(
                search::toggle_search_flag(&state, SearchFlag::WholeWord),
                window,
                cx,
            ),
            DeleteBracketPair => self.run_spec(
                crate::close_brackets::delete_bracket_pair(&state),
                window,
                cx,
            ),
            StartCompletion => {
                self.run_spec(crate::autocomplete::start_completion(&state), window, cx)
            }
            CloseCompletion => {
                self.run_spec(crate::autocomplete::close_completion(&state), window, cx)
            }
            CompletionDown => self.run_spec(
                crate::autocomplete::move_completion_selection(&state, true, None),
                window,
                cx,
            ),
            CompletionUp => self.run_spec(
                crate::autocomplete::move_completion_selection(&state, false, None),
                window,
                cx,
            ),
            CompletionPageDown => {
                let page = self.completion_page();
                self.run_spec(
                    crate::autocomplete::move_completion_selection(&state, true, Some(page)),
                    window,
                    cx,
                )
            }
            CompletionPageUp => {
                let page = self.completion_page();
                self.run_spec(
                    crate::autocomplete::move_completion_selection(&state, false, Some(page)),
                    window,
                    cx,
                )
            }
            AcceptCompletion => {
                let mut ok = false;
                self.with_dispatcher(window, cx, |d| {
                    ok = crate::autocomplete::accept_completion(d)
                });
                ok
            }
            NextSnippetField => {
                self.run_spec(crate::snippet::next_snippet_field(&state), window, cx)
            }
            PrevSnippetField => {
                self.run_spec(crate::snippet::prev_snippet_field(&state), window, cx)
            }
            ClearSnippet => self.run_spec(crate::snippet::clear_snippet(&state), window, cx),
            InsertNewlineBetweenPairs => self.insert_newline_between_pairs(window, cx),
            ExpandTabSnippet => self.expand_tab_snippet(window, cx),
            Save => {
                cx.emit(EditorEvent::Save);
                true
            }
            GotoDefinition => self.goto_definition(window, cx),
            RevertHunk => self.revert_hunk(window, cx),
            FormatDocument => self.format_document(window, cx),
            RenameSymbol => self.rename_symbol(window, cx),
            ShowSignatureHelp => self.show_signature_help(window, cx),
            PrevSignature => self.step_signature(false, cx),
            NextSignature => self.step_signature(true, cx),
        }
    }

    /// `openSearchPanel`: open, or refocus and refill the find field.
    fn open_search_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.state.search.panel {
            let focused = self
                .search
                .as_ref()
                .is_some_and(|s| s.find_focused(window, cx));
            if focused {
                return true;
            }
            let spec = search::open_search_panel(&self.state, false);
            self.run_spec(spec, window, cx);
            self.ensure_search_ui(window, cx);
            if let Some(s) = self.search.as_ref() {
                s.focus_find(window, cx);
            }
            return true;
        }
        let spec = search::open_search_panel(&self.state, false);
        self.run_spec(spec, window, cx);
        true
    }

    /// Recur's `insertNewlineBetweenPairs` (Return between `{}` / tags).
    fn insert_newline_between_pairs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let state = &self.state;
        let sel = state.selection.main();
        if !sel.is_empty() || state.read_only() {
            return false;
        }
        let line = state.doc.line_at(sel.head());
        let text = state.doc.slice(line.from, line.to);
        let Some(edit) = recur_textmate::newline_between_pairs(
            &text,
            sel.head() - line.from,
            state.indent_unit(),
            state.tab_size(),
        ) else {
            return false;
        };
        let spec = TransactionSpec::default()
            .changes(Change::new(
                line.from + edit.from,
                line.from + edit.to,
                edit.insert,
            ))
            .cursor(line.from + edit.cursor)
            .user_event("input");
        self.dispatch(spec, window, cx);
        true
    }

    /// `expandTabSnippet`.
    fn expand_tab_snippet(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(provider) = self.snippets.clone() else {
            return false;
        };
        let snippets = provider.cached(&self.scope, &self.path);
        let mut pick: Option<(Vec<crate::snippet::RegisteredSnippet>, usize, usize)> = None;
        let mut ok = false;
        self.with_dispatcher(window, cx, |d| {
            ok = crate::snippet::try_expand_snippet(d, &snippets, |m, f, t| pick = Some((m, f, t)));
        });
        if let Some((snippets, from, to)) = pick {
            cx.emit(EditorEvent::PickSnippet { snippets, from, to });
        }
        if ok {
            self.last_emitted = Some(self.state.doc.to_string());
        }
        ok
    }

    /// F12: LSP jump to definition, else the word to the host.
    fn goto_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.run_lsp_goto_def(window, cx) {
            return true;
        }
        let head = self.state.selection.main().head();
        let word = self
            .state
            .word_at(head)
            .map(|w| self.state.slice_doc(w.from, w.to))
            .unwrap_or_default();
        if word.is_empty() {
            return false;
        }
        cx.emit(EditorEvent::GotoDefinition(word));
        true
    }

    /// `revertHunkCommand`.
    fn revert_hunk(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.state.read_only() || self.state.merge.is_none() {
            return false;
        }
        let state = self.state.clone();
        let spec = crate::merge::reject_chunk(&state, self.revert.from_b);
        self.run_spec(spec, window, cx)
    }

    /// Rows a completion page moves (`offsetHeight / li height - 1`).
    fn completion_page(&self) -> usize {
        self.tooltips.completion_page()
    }

    // ---- keys ----

    /// Key down in the content. Returns whether the key was consumed.
    pub(crate) fn key_down(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = KeyEvent::from_keystroke(&ev.keystroke, self.platform);
        let handled = self.key_down_handlers(&key, ev, window, cx);
        // Vim's insert-mode `keydown` listener on the content, after
        // CodeMirror's own handlers.
        if self.vim.as_ref().is_some_and(|v| v.cm.listening_changes) {
            crate::vim::insert::on_insert_key_down(&key.name);
        }
        handled
    }

    fn key_down_handlers(
        &mut self,
        key: &KeyEvent,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = key.clone();
        let now = now_ms() as i64;
        if key.name == "Tab"
            && self.tab_focus > -1
            && (self.tab_focus == 0 || now <= self.tab_focus)
        {
            return false;
        }
        if self.tab_focus > 0 && key.name != "Escape" && !is_modifier(&key.name) {
            self.tab_focus = -1;
        }
        if key.name == "Escape" && self.tab_focus != 0 {
            self.tab_focus = now + 2000;
        }
        if self.kind == EditorKind::Code {
            self.commit_characters(&key, ev, window, cx);
        }
        if self.run_keymap(&key, "editor", window, cx) {
            return true;
        }
        if self.emacs.is_some() && crate::emacs::key_down(self, &key, ev, window, cx) {
            return true;
        }
        if self.vim.is_some() && crate::vim::key_down(self, &key, ev, window, cx) {
            return true;
        }
        self.clipboard_key(&key, window, cx)
    }

    /// `commitCharacters` (a highest-precedence keydown handler): a typed
    /// commit character accepts the selected option before it is inserted.
    fn commit_characters(
        &mut self,
        key: &KeyEvent,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typed = ev
            .keystroke
            .key_char
            .clone()
            .unwrap_or_else(|| key.name.clone());
        let windows_alt = self.platform == Platform::Win && key.alt;
        if typed.chars().count() != 1 || (key.ctrl && !windows_alt) || key.meta {
            return;
        }
        self.with_dispatcher(window, cx, |d| {
            crate::autocomplete::commit_character(d, &typed)
        });
    }

    /// Run the keymap for a scope (`runScopeHandlers`).
    pub(crate) fn run_keymap(
        &mut self,
        key: &KeyEvent,
        scope: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let platform = self.platform;
        let keymap = std::mem::replace(
            &mut self.keymap,
            crate::keymap::Keymap::build(&[], platform),
        );
        let handled = keymap.run(key, scope, platform, |c| self.run_cmd(*c, window, cx));
        self.keymap = keymap;
        handled
    }

    /// Native copy / cut / paste (the browser's, not keymap bindings).
    fn clipboard_key(
        &mut self,
        key: &KeyEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modk = if self.platform == Platform::Mac {
            key.meta && !key.ctrl
        } else {
            key.ctrl && !key.meta
        };
        if !modk || key.alt || key.shift {
            return false;
        }
        match key.name.to_ascii_lowercase().as_str() {
            "c" => {
                self.copy(false, window, cx);
                true
            }
            "x" => {
                self.copy(true, window, cx);
                true
            }
            "v" => {
                if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
                    self.paste(&text, window, cx);
                }
                true
            }
            _ => false,
        }
    }

    /// Copy (or cut) the selection, line-wise when it is empty.
    pub fn copy(&mut self, cut: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (text, ranges, linewise) = copied_range(&self.state);
        if text.is_empty() && !linewise {
            return;
        }
        LAST_LINEWISE_COPY.with(|c| *c.borrow_mut() = linewise.then(|| text.clone()));
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if !cut {
            crate::vim::after_copy(self, window, cx);
        }
        if cut && !self.state.read_only() {
            let changes: Vec<Change> = ranges
                .into_iter()
                .map(|(f, t)| Change::delete(f, t))
                .collect();
            self.dispatch(
                TransactionSpec::default()
                    .changes(changes)
                    .scroll()
                    .user_event("delete.cut"),
                window,
                cx,
            );
        }
    }

    pub fn paste(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.read_only() {
            return;
        }
        let spec = paste_spec(&self.state, text);
        self.dispatch(spec, window, cx);
        crate::vim::after_paste(self, window, cx);
    }

    // ---- text input ----

    /// Typed text replacing `from..to` (`applyDOMChange` for an input
    /// event): input handlers first, then the default insert.
    pub(crate) fn input_text(
        &mut self,
        from: usize,
        to: usize,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let text = crate::text::normalize_newlines(text).into_owned();
        if crate::vim::input(self, from, to, &text, window, cx) {
            return;
        }
        let state = self.state.clone();
        if !self.composing {
            if let Some(spec) =
                crate::close_brackets::skip_paired_quote_input(&state, from, to, &text)
            {
                self.dispatch(spec, window, cx);
                return;
            }
            if self.kind == EditorKind::Code
                && let Some(spec) = crate::close_brackets::close_brackets_input(
                    &state,
                    from.min(state.doc.len()),
                    to.min(state.doc.len()),
                    &text,
                    false,
                )
            {
                self.dispatch(spec, window, cx);
                return;
            }
        }
        self.default_insert(from, to, &text, window, cx);
    }

    /// `applyDefaultInsert`.
    fn default_insert(
        &mut self,
        from: usize,
        to: usize,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let doc_len = state.doc.len();
        let (from, to) = (from.min(doc_len), to.min(doc_len));
        let sel = state.selection.main();
        let mut spec = if from >= sel.from
            && to <= sel.to
            && (to - from) * 3 >= sel.to - sel.from
            && !self.composing
        {
            let before = if sel.from < from {
                state.slice_doc(sel.from, from)
            } else {
                String::new()
            };
            let after = if sel.to > to {
                state.slice_doc(to, sel.to)
            } else {
                String::new()
            };
            state
                .replace_selection(&format!("{before}{text}{after}"))
                .spec()
        } else if state.selection.ranges.len() > 1
            && self.composing
            && to + 10 >= sel.to
            && to <= sel.to + 10
        {
            // Replicate a composition edit around the main cursor to every
            // range, like CodeMirror's multi-cursor IME path.
            let (rel_from, rel_to) = (
                sel.to as isize - from as isize,
                to as isize - sel.to as isize,
            );
            let main = sel;
            state
                .change_by_range(|range| {
                    if range.from == main.from && range.to == main.to {
                        return RangeChange::with(
                            vec![Change::new(from, to, text)],
                            SelectionRange::cursor(from + text.len()),
                        );
                    }
                    let f = (range.to as isize - rel_from).max(0) as usize;
                    let t = ((range.to as isize + rel_to).max(0) as usize).min(doc_len);
                    if f > range.from
                        || state.slice_doc(f, range.to) != state.slice_doc(from, main.to)
                    {
                        return RangeChange::range(range);
                    }
                    RangeChange::with(
                        vec![Change::new(f, t, text)],
                        SelectionRange::cursor(f + text.len()),
                    )
                })
                .spec()
        } else {
            TransactionSpec::default()
                .changes(Change::new(from, to, text))
                .cursor(from + text.len())
        };
        let mut event = "input.type".to_string();
        if self.composing {
            event.push_str(".compose");
            if self.composition_first {
                event.push_str(".start");
                self.composition_first = false;
            }
        }
        spec = spec.user_event(&event).scroll();
        self.dispatch(spec, window, cx);
    }

    // ---- pointer geometry ----

    /// Pointer in content coordinates: x from the line's text start, y
    /// from the content top.
    pub(crate) fn content_xy(&self, p: Point<Pixels>) -> Option<(f32, f32)> {
        let f = self.frame.as_ref()?;
        let x = f32::from(p.x) - (f32::from(f.bounds.origin.x) + f.gutter_width) + self.scroll_left
            - self.metrics.line_padding_left;
        let y = f32::from(p.y) - f.scroller_top + self.scroll_top;
        Some((x, y))
    }

    /// `posAtPointer`: position and side under the pointer.
    pub(crate) fn pos_at_pointer(&self, p: Point<Pixels>, window: &Window) -> (usize, i32) {
        let Some((x, y)) = self.content_xy(p) else {
            return (self.state.selection.main().head(), 1);
        };
        self.with_layout_view(window, |lv| {
            let pos = lv.pos_at(x, y);
            let caret = lv.x_at(pos, 1);
            (pos, if x < caret { 1 } else { -1 })
        })
    }

    /// `getPos` for rectangular selection.
    fn rect_pos(&self, p: Point<Pixels>, window: &Window) -> RectPos {
        let (offset, _) = self.pos_at_pointer(p, window);
        let line = self.state.doc.line_at(offset);
        let off = offset - line.from;
        let col = if off > MAX_OFF {
            -1
        } else if off == line.len() {
            let x = self.content_xy(p).map_or(0.0, |(x, _)| x);
            let cw = self.layout.borrow().config.char_width;
            if cw > 0.0 {
                (x.abs() / cw).round() as isize
            } else {
                -1
            }
        } else {
            let text = self.state.doc.slice(line.from, line.to);
            crate::text::count_column(&text, self.state.tab_size(), off) as isize
        };
        RectPos {
            line: line.number,
            col,
            off,
        }
    }

    /// The selection the drag implies at `p` (`MouseSelectionStyle.get`).
    fn drag_selection(
        &self,
        drag: &MouseDrag,
        p: Point<Pixels>,
        window: &Window,
    ) -> Option<EditorSelection> {
        match drag {
            MouseDrag::Select {
                start_pos,
                start_assoc,
                start_point,
                kind,
                start_sel,
                extend,
                multiple,
                ..
            } => {
                let moved = (f32::from(p.x) - f32::from(start_point.x))
                    .abs()
                    .max((f32::from(p.y) - f32::from(start_point.y)).abs())
                    > POINTER_SLOP_PX;
                let (cur_pos, cur_assoc) = if moved {
                    self.pos_at_pointer(p, window)
                } else {
                    (*start_pos, *start_assoc)
                };
                let state = &self.state;
                let mut range = selection_for_click(state, cur_pos, cur_assoc, *kind);
                if *start_pos != cur_pos && !*extend {
                    let start_range = selection_for_click(state, *start_pos, *start_assoc, *kind);
                    let from = start_range.from.min(range.from);
                    let to = start_range.to.max(range.to);
                    range = if from < range.from {
                        SelectionRange::range_with(from, to, None, range.assoc())
                    } else {
                        SelectionRange::range_with(to, from, None, range.assoc())
                    };
                }
                Some(if *extend {
                    let main = start_sel.main().extend(range.from, range.to, range.assoc());
                    start_sel.replace_range(main, start_sel.main_index)
                } else if *multiple {
                    start_sel.add_range(range, true)
                } else {
                    EditorSelection::create(vec![range], 0)
                })
            }
            MouseDrag::Rect {
                start,
                start_sel,
                multiple,
                ..
            } => {
                let cur = self.rect_pos(p, window);
                let ranges = rectangle_for(&self.state, *start, cur);
                if ranges.is_empty() {
                    return Some(start_sel.clone());
                }
                Some(if *multiple {
                    let mut all = ranges;
                    all.extend(start_sel.ranges.iter().copied());
                    EditorSelection::create(all, 0)
                } else {
                    EditorSelection::create(ranges, 0)
                })
            }
            _ => None,
        }
    }

    /// `MouseSelection.select`.
    fn drag_select(
        &mut self,
        p: Point<Pixels>,
        must: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.clone() else {
            return;
        };
        let Some(selection) = self.drag_selection(&drag, p, window) else {
            return;
        };
        let dragging_false = matches!(
            drag,
            MouseDrag::Select {
                dragging: Some(false),
                ..
            } | MouseDrag::Rect { .. }
        );
        if must || !selection.eq_sel(&self.state.selection, dragging_false) {
            self.dispatch(
                TransactionSpec::default()
                    .selection(selection)
                    .user_event("select.pointer"),
                window,
                cx,
            );
        }
    }

    fn is_in_primary_selection(&self, p: Point<Pixels>, window: &Window) -> bool {
        let main = self.state.selection.main();
        if main.is_empty() {
            return false;
        }
        let (pos, _) = self.pos_at_pointer(p, window);
        pos > main.from && pos < main.to || {
            let (x, _) = self.content_xy(p).unwrap_or((0.0, 0.0));
            let (fx, tx) =
                self.with_layout_view(window, |lv| (lv.x_at(main.from, 1), lv.x_at(main.to, -1)));
            (pos == main.from && x >= fx) || (pos == main.to && x <= tx)
        }
    }

    /// An overlay scroller's box and knob from the last frame, while its
    /// content overflows.
    pub(crate) fn overlay_bar(&self, vertical: bool) -> Option<OverlayBar> {
        use recur_ui::widgets::scrollbar::{OVERLAY_BOX, overlay_knob};
        let f = self.frame.as_ref()?;
        let (bx, by) = (f32::from(f.bounds.origin.x), f32::from(f.bounds.origin.y));
        let (bw, bh) = (
            f32::from(f.bounds.size.width),
            f32::from(f.bounds.size.height),
        );
        let bar = |start, track, total: f32, visible, pos, area| {
            let (knob_len, knob_at) = overlay_knob(pos, total, visible, track);
            OverlayBar {
                area,
                start,
                track,
                total,
                visible,
                knob_len,
                knob_at,
            }
        };
        if vertical {
            let view = f.scroller_height;
            (f.max_scroll_top > 0.0).then(|| {
                let area = gpui_kit::bounds(
                    point(px(bx + bw - OVERLAY_BOX), px(by)),
                    gpui_kit::size(px(OVERLAY_BOX), px(view)),
                );
                bar(
                    by,
                    view,
                    f.max_scroll_top + view,
                    view,
                    self.scroll_top,
                    area,
                )
            })
        } else {
            // The vertical scroller keeps the corner.
            let corner = if f.max_scroll_top > 0.0 {
                OVERLAY_BOX
            } else {
                0.0
            };
            let track = f.scroller_width - corner;
            (f.max_scroll_left > 0.0).then(|| {
                let area = gpui_kit::bounds(
                    point(px(bx), px(by + bh - OVERLAY_BOX)),
                    gpui_kit::size(px(track), px(OVERLAY_BOX)),
                );
                let total = f.gutter_width + f.content_width;
                bar(bx, track, total, f.scroller_width, self.scroll_left, area)
            })
        }
    }

    /// The scrollbar under `p`, as (vertical, on thumb, thumb start, thumb
    /// length, track start, track length). Overlay scrollers take the pointer
    /// only while they show.
    fn scrollbar_hit(
        &self,
        p: Point<Pixels>,
        now: std::time::Instant,
    ) -> Option<(bool, bool, f32, f32, f32, f32)> {
        let f = self.frame.as_ref()?;
        if !f.legacy_bars {
            return self
                .bars
                .iter()
                .zip([true, false])
                .find_map(|(o, vertical)| {
                    let bar = self.overlay_bar(vertical)?;
                    if !o.visible(now) || !bar.area.contains(&p) {
                        return None;
                    }
                    let along = f32::from(if vertical { p.y } else { p.x });
                    let at = bar.start + bar.knob_at;
                    let on = along >= at && along < at + bar.knob_len;
                    Some((vertical, on, at, bar.knob_len, bar.start, bar.track))
                });
        }
        let (bx, by) = (f32::from(f.bounds.origin.x), f32::from(f.bounds.origin.y));
        let (bw, bh) = (
            f32::from(f.bounds.size.width),
            f32::from(f.bounds.size.height),
        );
        let (x, y) = (f32::from(p.x), f32::from(p.y));
        let view_h = f.scroller_height;
        let content_h = f.max_scroll_top + view_h;
        if f.max_scroll_top > 0.0
            && x >= bx + bw - crate::element::scrollbar_size()
            && x < bx + bw
            && y >= by
            && y < by + view_h
        {
            let (thumb, at) = recur_ui::widgets::scrollbar::classic_thumb(
                self.scroll_top,
                content_h.round(),
                view_h,
                view_h,
            );
            let ty = by + at;
            return Some((true, y >= ty && y < ty + thumb, ty, thumb, by, view_h));
        }
        if f.max_scroll_left > 0.0
            && y >= by + bh - crate::element::scrollbar_size()
            && y < by + bh
            && x >= bx
            && x < bx + f.scroller_width
        {
            let track = f.scroller_width;
            let total = (f.gutter_width + f.content_width).round();
            let (thumb, at) = recur_ui::widgets::scrollbar::classic_thumb(
                self.scroll_left,
                total,
                track.round(),
                track.round(),
            );
            let tx = bx + at;
            return Some((false, x >= tx && x < tx + thumb, tx, thumb, bx, track));
        }
        None
    }

    /// The collapsed merge region under content `y`.
    fn collapsed_at(&self, y: f32) -> Option<usize> {
        let merge = self.state.merge.as_ref()?;
        let l = self.layout.borrow();
        l.widgets().iter().find_map(|w| {
            w.replace_to?;
            let top = l.block_top(w.line) as f32;
            (y >= top && y < top + w.height)
                .then(|| w.id - 1_000_000)
                .filter(|i| *i < merge.collapsed.len())
        })
    }

    /// The fold placeholder under content `(x, y)`: its line.
    fn placeholder_at(&self, x: f32, y: f32) -> Option<usize> {
        let l = self.layout.borrow();
        let n = l.line_at_y(y as f64);
        l.fold_end(n)?;
        let m = l.measured(n)?;
        let (ph, _) = m.display.placeholder()?;
        let row = (((y as f64 - l.line_top(n)) / l.config.line_height as f64)
            .floor()
            .max(0.0) as usize)
            .min(m.rows.len() - 1);
        let px0 = m.row_x(ph, row);
        let w = m.x_of(m.display.text.len()) - m.x_of(ph) + 6.0;
        (m.row_of(ph, 1) == row && x >= px0 && x <= px0 + w).then_some(n)
    }

    /// Whether `p` is on the revert chip.
    fn on_chip(&self, p: Point<Pixels>) -> bool {
        self.chip_bounds.is_some_and(|b| b.contains(&p))
    }

    // ---- mouse ----

    pub(crate) fn mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tooltips.hide_hover();
        if self.on_chip(e.position) && e.button == MouseButton::Left {
            self.drag = Some(MouseDrag::Chip);
            cx.stop_propagation();
            return;
        }
        let now = recur_ui::motion::now(cx);
        if let Some((vertical, on_thumb, start, len, track, track_len)) =
            self.scrollbar_hit(e.position, now)
        {
            if e.button != MouseButton::Left {
                return;
            }
            self.bars[usize::from(!vertical)].press(true, now);
            let along = if vertical {
                f32::from(e.position.y)
            } else {
                f32::from(e.position.x)
            };
            if on_thumb {
                self.drag = Some(MouseDrag::Thumb {
                    vertical,
                    grab: along - start,
                });
            } else {
                let f = self.frame.clone().unwrap_or_default();
                let page = if vertical {
                    f.scroller_height
                } else {
                    f.scroller_width - f.gutter_width
                } * TRACK_PAGE_FRACTION;
                let dir = if along < start { -1.0 } else { 1.0 };
                if vertical {
                    self.scroll_top = (self.scroll_top + dir * page).clamp(0.0, f.max_scroll_top);
                } else {
                    self.scroll_left =
                        (self.scroll_left + dir * page).clamp(0.0, f.max_scroll_left);
                }
                let _ = (len, track, track_len);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if let Some((kind, line)) = self.hit_gutter(e.position, window) {
            if e.button == MouseButton::Left {
                if kind == GutterKind::Findings {
                    let from = self.state.doc.line(line).from;
                    if let Some(id) = self.state.gutters.finding_at(from).map(str::to_string) {
                        cx.emit(EditorEvent::OpenFinding(id));
                        cx.stop_propagation();
                        return;
                    }
                }
                self.drag = Some(MouseDrag::Gutter { kind, line });
            }
            cx.stop_propagation();
            return;
        }
        let Some((x, y)) = self.content_xy(e.position) else {
            return;
        };
        if let Some(em) = self.emacs.as_mut() {
            em.mouse_down();
        }
        self.focus(window, cx);
        cx.stop_propagation();
        if e.button != MouseButton::Left {
            return;
        }
        if let Some(idx) = self.collapsed_at(y) {
            let from = self
                .state
                .merge
                .as_ref()
                .map_or(0, |m| m.collapsed[idx].from);
            self.dispatch(
                TransactionSpec::default()
                    .effect(Effect::Merge(crate::merge::MergeEffect::Uncollapse(from))),
                window,
                cx,
            );
            return;
        }
        if let Some(n) = self.placeholder_at(x, y) {
            let state = self.state.clone();
            let block =
                self.with_layout_view(window, |lv| lv.line_block_at(state.doc.line(n).from));
            let spec = crate::fold::unfold_at_block(&state, block);
            if self.run_spec(spec, window, cx) {
                return;
            }
        }
        let (pos, assoc) = self.pos_at_pointer(e.position, window);
        if self.state.snippet.is_some() {
            let state = self.state.clone();
            if self.run_spec(crate::snippet::snippet_mousedown(&state, pos), window, cx) {
                return;
            }
        }
        let modk = if self.platform == Platform::Mac {
            e.modifiers.platform
        } else {
            e.modifiers.control
        };
        if e.modifiers.alt {
            if !self.prefs.rectangular_select {
                return;
            }
            let start = self.rect_pos(e.position, window);
            self.drag = Some(MouseDrag::Rect {
                start,
                start_sel: self.state.selection.clone(),
                multiple: modk,
                last_point: e.position,
            });
            self.drag_select(e.position, false, window, cx);
            return;
        }
        let kind = e.click_count.max(1);
        let in_sel = kind == 1 && self.is_in_primary_selection(e.position, window);
        self.drag = Some(MouseDrag::Select {
            start_pos: pos,
            start_assoc: assoc,
            start_point: e.position,
            last_point: e.position,
            kind,
            start_sel: self.state.selection.clone(),
            extend: e.modifiers.shift,
            multiple: modk,
            dragging: if in_sel { None } else { Some(false) },
            mod_click: modk && kind == 1,
        });
        if !in_sel {
            self.drag_select(e.position, false, window, cx);
        }
    }

    pub(crate) fn mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if bounds.contains(&e.position) {
            let now = recur_ui::motion::now(cx);
            for o in &mut self.bars {
                o.pointer_moved(now);
            }
        }
        let pressed = e.pressed_button == Some(MouseButton::Left);
        match self.drag.clone() {
            Some(_) if !pressed => {
                self.end_drag(window, cx);
            }
            Some(MouseDrag::Thumb { vertical, grab }) => {
                if let Some(bar) = self
                    .overlay_bar(vertical)
                    .filter(|_| self.frame.as_ref().is_some_and(|f| !f.legacy_bars))
                {
                    let along = f32::from(if vertical { e.position.y } else { e.position.x });
                    let pos = recur_ui::widgets::scrollbar::overlay_scroll(
                        along - grab - bar.start,
                        bar.total,
                        bar.visible,
                        bar.track,
                    );
                    if vertical {
                        self.scroll_top = pos;
                    } else {
                        self.scroll_left = pos;
                    }
                    cx.notify();
                } else if let Some(f) = self.frame.clone() {
                    let (track, track_len, max) = if vertical {
                        (
                            f32::from(f.bounds.origin.y),
                            f.scroller_height,
                            f.max_scroll_top,
                        )
                    } else {
                        (
                            f32::from(f.bounds.origin.x),
                            f.scroller_width,
                            f.max_scroll_left,
                        )
                    };
                    let content = if vertical {
                        f.max_scroll_top + f.scroller_height
                    } else {
                        f.gutter_width + f.content_width
                    };
                    let (thumb, _) = recur_ui::widgets::scrollbar::classic_thumb(
                        0.0,
                        content.round(),
                        track_len,
                        track_len,
                    );
                    let along = if vertical {
                        f32::from(e.position.y)
                    } else {
                        f32::from(e.position.x)
                    };
                    let frac =
                        ((along - grab - track) / (track_len - thumb).max(1.0)).clamp(0.0, 1.0);
                    if vertical {
                        self.scroll_top = frac * max;
                    } else {
                        self.scroll_left = frac * max;
                    }
                    cx.notify();
                }
                return;
            }
            Some(MouseDrag::Select {
                start_point,
                dragging,
                ..
            }) => {
                let dist = (f32::from(e.position.x) - f32::from(start_point.x))
                    .abs()
                    .max((f32::from(e.position.y) - f32::from(start_point.y)).abs());
                if dragging == Some(true) || (dragging.is_none() && dist >= DRAG_START_PX) {
                    if let Some(MouseDrag::Select { dragging, .. }) = self.drag.as_mut() {
                        *dragging = Some(true);
                    }
                    if self.prefs.drop_cursor {
                        let (pos, _) = self.pos_at_pointer(e.position, window);
                        self.drop_cursor = Some(pos);
                        cx.notify();
                    }
                    return;
                }
                if dragging.is_none() {
                    return;
                }
                if dist > 0.0
                    && let Some(MouseDrag::Select { mod_click, .. }) = self.drag.as_mut()
                {
                    *mod_click = false;
                }
                self.set_last_point(e.position);
                self.drag_select(e.position, false, window, cx);
                self.update_drag_scroll(e.position, bounds, window, cx);
                return;
            }
            Some(MouseDrag::Rect { .. }) => {
                self.set_last_point(e.position);
                self.drag_select(e.position, false, window, cx);
                self.update_drag_scroll(e.position, bounds, window, cx);
                return;
            }
            Some(_) => return,
            None => {}
        }
        if !bounds.contains(&e.position) {
            if self.chip_hover {
                self.chip_hover = false;
                cx.notify();
            }
            if self.revert_pointer_inside {
                self.revert_pointer_inside = false;
                self.revert.leave(now_ms());
                let a = crate::merge::ChipAction::Keep;
                self.apply_chip(a, cx);
            }
            self.mouse_left(window, cx);
            return;
        }
        self.pointer_hover(e.position, window, cx);
    }

    fn set_last_point(&mut self, p: Point<Pixels>) {
        match self.drag.as_mut() {
            Some(MouseDrag::Select { last_point, .. })
            | Some(MouseDrag::Rect { last_point, .. }) => *last_point = p,
            _ => {}
        }
    }

    /// Autoscroll while selecting outside the scroller (`setScrollSpeed`).
    fn update_drag_scroll(
        &mut self,
        p: Point<Pixels>,
        _bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.frame.clone() else { return };
        let (left, right) = (
            f32::from(f.bounds.origin.x),
            f32::from(f.bounds.origin.x) + f.scroller_width,
        );
        let (top, bottom) = (f.scroller_top, f.scroller_top + f.scroller_height);
        let (x, y) = (f32::from(p.x), f32::from(p.y));
        let mut sx = 0.0;
        let mut sy = 0.0;
        if x <= left + DRAG_SCROLL_MARGIN {
            sx = -drag_scroll_speed(left - x);
        } else if x >= right - DRAG_SCROLL_MARGIN {
            sx = drag_scroll_speed(x - right);
        }
        if y <= top + DRAG_SCROLL_MARGIN {
            sy = -drag_scroll_speed(top - y);
        } else if y >= bottom - DRAG_SCROLL_MARGIN {
            sy = drag_scroll_speed(y - bottom);
        }
        self.drag_scroll_speed = (sx, sy);
        if (sx != 0.0 || sy != 0.0) && self.drag_scroll_task.is_none() {
            self.drag_scroll_task = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                    let go = this
                        .update_in(cx, |v, window, cx| {
                            let (sx, sy) = v.drag_scroll_speed;
                            if (sx == 0.0 && sy == 0.0) || v.drag.is_none() {
                                v.drag_scroll_task = None;
                                return false;
                            }
                            if let Some(f) = v.frame.as_ref() {
                                v.scroll_left = (v.scroll_left + sx).clamp(0.0, f.max_scroll_left);
                                v.scroll_top = (v.scroll_top + sy).clamp(0.0, f.max_scroll_top);
                            }
                            let last = match v.drag.as_ref() {
                                Some(MouseDrag::Select {
                                    last_point,
                                    dragging: Some(false),
                                    ..
                                }) => Some(*last_point),
                                Some(MouseDrag::Rect { last_point, .. }) => Some(*last_point),
                                _ => None,
                            };
                            if let Some(p) = last {
                                v.drag_select(p, false, window, cx);
                            }
                            cx.notify();
                            true
                        })
                        .unwrap_or(false);
                    if !go {
                        break;
                    }
                }
            }));
        } else if sx == 0.0 && sy == 0.0 {
            self.drag_scroll_task = None;
        }
    }

    pub(crate) fn mouse_up(
        &mut self,
        e: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = recur_ui::motion::now(cx);
        for o in &mut self.bars {
            o.press(false, now);
        }
        let Some(drag) = self.drag.clone() else {
            return;
        };
        match drag {
            MouseDrag::Chip => {
                self.drag = None;
                if self.on_chip(e.position) && !self.state.read_only() {
                    let state = self.state.clone();
                    let spec = self.revert.revert(&state);
                    self.run_spec(spec, window, cx);
                    let a = self.revert.force_hide();
                    self.apply_chip(a, cx);
                }
            }
            MouseDrag::Gutter { kind, line } => {
                self.drag = None;
                if self.hit_gutter(e.position, window) != Some((kind, line)) {
                    return;
                }
                match kind {
                    GutterKind::LineNumbers => cx.emit(EditorEvent::CopyLine(line)),
                    GutterKind::Fold => {
                        let state = self.state.clone();
                        let block = self.with_layout_view(window, |lv| {
                            lv.line_block_at(state.doc.line(line).from)
                        });
                        let spec = crate::fold::fold_gutter_click(&state, block);
                        self.run_spec(spec, window, cx);
                    }
                    _ => {}
                }
            }
            MouseDrag::Select {
                dragging: Some(true),
                ..
            } => {
                self.drop_selection(e.position, window, cx);
            }
            MouseDrag::Select {
                dragging: None,
                last_point,
                ..
            } => {
                if let Some(MouseDrag::Select { dragging, .. }) = self.drag.as_mut() {
                    *dragging = Some(false);
                }
                self.drag_select(last_point, false, window, cx);
                self.end_drag(window, cx);
            }
            MouseDrag::Select {
                mod_click: true, ..
            } => {
                self.end_drag(window, cx);
                self.mod_click_goto(e.position, window, cx);
            }
            _ => self.end_drag(window, cx),
        }
    }

    fn end_drag(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
        self.drag_scroll_task = None;
        self.drag_scroll_speed = (0.0, 0.0);
        if self.drop_cursor.take().is_some() {
            cx.notify();
        }
    }

    /// `dropText` for a selection dragged inside the editor.
    fn drop_selection(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let alt = window.modifiers().alt;
        let ctrl = window.modifiers().control;
        self.end_drag(window, cx);
        if self.state.read_only() {
            return;
        }
        let main = self.state.selection.main();
        let text = self.state.slice_doc(main.from, main.to);
        if text.is_empty() {
            return;
        }
        let (drop_pos, _) = self.pos_at_pointer(p, window);
        let moves = if self.platform == Platform::Mac {
            !alt
        } else {
            !ctrl
        };
        let mut changes = Vec::new();
        if moves {
            changes.push(Change::delete(main.from, main.to));
        }
        changes.push(Change::insert(drop_pos, text));
        let set = self.state.changes(&changes);
        let anchor = set.map_pos(drop_pos, -1);
        let head = set.map_pos(drop_pos, 1);
        self.focus(window, cx);
        self.dispatch(
            TransactionSpec::default()
                .changes(set)
                .selection(EditorSelection::single(anchor, head))
                .user_event(if moves { "move.drop" } else { "input.drop" }),
            window,
            cx,
        );
    }

    /// Mod-click: LSP definition, else the word to the host.
    fn mod_click_goto(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let (pos, _) = self.pos_at_pointer(p, window);
        if self.run_lsp_goto_def(window, cx) {
            return;
        }
        if let Some(w) = self.state.word_at(pos) {
            let word = self.state.slice_doc(w.from, w.to);
            cx.emit(EditorEvent::GotoDefinition(word));
        }
    }

    pub(crate) fn scroll_wheel(
        &mut self,
        e: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lh = px(self.metrics.line_height());
        let d = e.delta.pixel_delta(lh);
        let Some(f) = self.frame.clone() else { return };
        let (mut dx, dy) = (f32::from(d.x), f32::from(d.y));
        if e.modifiers.shift && dx == 0.0 {
            dx = dy;
            let left = (self.scroll_left - dx).clamp(0.0, f.max_scroll_left);
            if left != self.scroll_left {
                self.scroll_left = left;
                self.bars[1].scrolled(recur_ui::motion::now(cx));
                cx.notify();
            }
            return;
        }
        let top = (self.scroll_top - dy).clamp(0.0, f.max_scroll_top);
        let left = (self.scroll_left - dx).clamp(0.0, f.max_scroll_left);
        let now = recur_ui::motion::now(cx);
        if top != self.scroll_top {
            self.bars[0].scrolled(now);
        }
        if left != self.scroll_left {
            self.bars[1].scrolled(now);
        }
        if top != self.scroll_top || left != self.scroll_left {
            self.scroll_top = top;
            self.scroll_left = left;
            self.tooltips.hide_hover();
            cx.notify();
            cx.stop_propagation();
        }
        let _ = window;
    }

    // ---- hover ----

    /// Pointer moved over the editor with no button held: revert chip
    /// tracking, hover tooltips, gutter titles.
    fn pointer_hover(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let now = recur_ui::motion::now(cx);
        let bar = self.scrollbar_hit(p, now);
        let mut changed = false;
        for (o, vertical) in self.bars.iter_mut().zip([true, false]) {
            changed |= o.hover(bar.is_some_and(|b| b.0 == vertical), now);
        }
        let knob = bar.filter(|b| b.1).map(|b| b.0);
        if changed || knob != self.knob_hover {
            self.knob_hover = knob;
            cx.notify();
        }
        let chip = self.on_chip(p);
        if chip != self.chip_hover {
            self.chip_hover = chip;
            if chip {
                self.revert.enter_chip();
            }
            cx.notify();
        }
        if self.state.merge.is_some() {
            self.revert_pointer_inside = true;
            let held = self.revert.from_b;
            let chunk = if chip {
                held.and_then(|h| self.state.merge.as_ref()?.chunk_at(h).map(|c| c.from_b))
            } else {
                let (pos, _) = self.pos_at_pointer(p, window);
                self.state
                    .merge
                    .as_ref()
                    .and_then(|m| m.chunk_at(pos).map(|c| c.from_b))
            };
            let state = self.state.clone();
            let a = self
                .revert
                .pointer(&state, f32::from(p.x), f32::from(p.y), chunk, now_ms());
            self.apply_chip(a, cx);
        }
        let gutter = self.hit_gutter(p, window);
        if gutter != self.gutter_hover {
            self.gutter_hover = gutter;
            cx.notify();
        }
        let title = self.title_at(p, window);
        if title != self.title_seen {
            self.title_seen = title;
            cx.notify();
        }
        if gutter.is_some() || bar.is_some() {
            return;
        }
        self.hover_moved(p, window, cx);
    }

    fn mouse_left(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.hover_task = None;
        let now = recur_ui::motion::now(cx);
        for o in &mut self.bars {
            if o.hover(false, now) {
                cx.notify();
            }
        }
        self.knob_hover = None;
        if self.title_seen.take().is_some() {
            cx.notify();
        }
        if self.gutter_hover.take().is_some() {
            cx.notify();
        }
        if self.tooltips.has_hover() && !self.tooltips.pointer_in_hover {
            self.tooltips.hide_hover();
            cx.notify();
        }
    }

    /// `HoverPlugin.mousemove`: hide a tooltip the pointer left, then
    /// (re)arm the hover timer.
    fn hover_moved(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.last_hover_point = Some(p);
        if !self.tooltips.pointer_in_hover {
            let (pos, _) = self.pos_at_pointer(p, window);
            let left = |v: &Self, h: &Option<crate::tooltip_ui::HoverSection>| {
                h.as_ref().is_some_and(|h| {
                    if h.pos == h.end {
                        pos != h.pos
                    } else {
                        !v.is_over_range(h.pos, h.end, p, window)
                    }
                })
            };
            let (drop_lsp, drop_lint) = (
                left(self, &self.tooltips.lsp_hover),
                left(self, &self.tooltips.lint_hover),
            );
            if drop_lsp {
                self.tooltips.lsp_hover = None;
            }
            if drop_lint {
                self.tooltips.lint_hover = None;
            }
            if drop_lsp || drop_lint {
                cx.notify();
            }
        }
        let lsp_active = self.lsp.is_none() || self.tooltips.lsp_hover.is_some();
        if lsp_active && self.tooltips.lint_hover.is_some() {
            return;
        }
        self.hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(crate::lsp::HOVER_TIME_MS))
                .await;
            this.update_in(cx, |v, window, cx| v.start_hover(p, window, cx))
                .ok();
        }));
    }

    /// `isOverRange` with CodeMirror's 6px margin.
    fn is_over_range(&self, from: usize, to: usize, p: Point<Pixels>, window: &Window) -> bool {
        let Some((x, y)) = self.content_xy(p) else {
            return false;
        };
        let margin = 6.0;
        self.with_layout_view(window, |lv| {
            let (ft, _) = lv.row_box(from, 1);
            let (_, tb) = lv.row_box(to, -1);
            if y < ft - margin || y > tb + margin {
                return false;
            }
            let pos = lv.pos_at(x, y);
            pos >= from && pos <= to
        })
    }

    /// `startHover`: query lint and LSP at the resting pointer.
    fn start_hover(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_some()
            || self.last_hover_point != Some(p)
            || !self.has_focus_or_hover(window)
        {
            return;
        }
        let Some((x, y)) = self.content_xy(p) else {
            return;
        };
        let (pos, side) = self.pos_at_pointer(p, window);
        let (top, bottom, cx_left) = self.with_layout_view(window, |lv| {
            let (t, b) = lv.row_box(pos, side);
            (t, b, lv.x_at(pos, side))
        });
        let cw = self.layout.borrow().config.char_width;
        if x < cx_left - cw || x > cx_left + cw || y < top || y > bottom {
            return;
        }
        if self.tooltips.lint_hover.is_none() {
            let lint = self.state.lint.tooltip_at(pos, side);
            let state = self.state.clone();
            self.tooltips.set_lint_hover(pos, lint, &state);
        }
        if self.tooltips.lsp_hover.is_none()
            && let Some(lsp) = self.lsp.as_mut()
        {
            let state = self.state.clone();
            if let Some(req) = lsp.hover_request(&state, pos) {
                let id = req.id;
                self.tooltips.lsp_hover_pending = Some((id, pos));
                let fut = req.response;
                cx.spawn_in(window, async move |this, cx| {
                    let result = fut.await;
                    this.update_in(cx, |v, _window, cx| {
                        if v.tooltips.lsp_hover_pending != Some((id, pos)) {
                            return;
                        }
                        v.tooltips.lsp_hover_pending = None;
                        if let Ok(value) = result
                            && let Some(info) =
                                crate::lsp::LspAttachment::hover_info(&v.state.doc, pos, &value)
                        {
                            v.tooltips.set_lsp_hover(info);
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn has_focus_or_hover(&self, _window: &Window) -> bool {
        true
    }

    /// The `title` a painted target shows (`title=` on gutter markers and
    /// the revert chip), with its screen bounds, for the app's title
    /// tooltip.
    pub fn title_at(&self, p: Point<Pixels>, window: &Window) -> Option<(String, Bounds<Pixels>)> {
        if self.on_chip(p) {
            return self
                .chip_bounds
                .map(|b| (crate::merge::revert_hunk_title(), b));
        }
        let (kind, line) = self.hit_gutter(p, window)?;
        let f = self.frame.as_ref()?;
        let col = f.gutters.iter().find(|g| g.kind == kind)?;
        let from = self.state.doc.line(line).from;
        let l = self.layout.borrow();
        let top = f.scroller_top - self.scroll_top + l.line_top(line) as f32;
        let rows = l.measured(line).map_or(1, |m| m.rows.len());
        let bounds = Bounds::new(
            point(f.bounds.origin.x + px(col.x), px(top)),
            size(px(col.width), px(rows as f32 * l.config.line_height)),
        );
        let title = match kind {
            GutterKind::Findings => self
                .state
                .gutters
                .finding_hits
                .get(&from)
                .map(|h| h.title.clone()),
            GutterKind::Lint => self.state.lint.gutter.get(&from).map(|h| h.title.clone()),
            GutterKind::Diff => self
                .state
                .gutters
                .diff_marks
                .iter()
                .find(|(pos, _)| {
                    self.state
                        .doc
                        .line_at((*pos).min(self.state.doc.len()))
                        .from
                        == from
                })
                .map(|(_, k)| k.title().to_string()),
            GutterKind::Fold => {
                let block = crate::view_queries::LineBlock {
                    from,
                    to: self.state.doc.line(l.block_end_line(line)).to,
                    top: 0.0,
                    height: 0.0,
                };
                crate::fold::fold_marker(&self.state, block).map(|m| m.title().to_string())
            }
            _ => None,
        }?;
        Some((title, bounds))
    }

    // ---- LSP ----

    /// Attach (or detach) a language server session.
    pub fn set_lsp(
        &mut self,
        client: Option<Rc<dyn crate::lsp::LspClient>>,
        uri: &str,
        language_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.lsp =
            client.map(|c| crate::lsp::LspAttachment::new(c, uri, language_id, &self.state.doc));
        self.lsp_sync_task = None;
        self.tooltips.signature_changed();
        self.rebuild_keymap();
        cx.notify();
    }

    /// Diagnostics from the server, in its synced document's coordinates
    /// (`serverDiagnostics`).
    pub fn set_lsp_diagnostics(
        &mut self,
        diagnostics: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(lsp) = self.lsp.as_ref() else { return };
        let Some(items) = diagnostics.as_array() else {
            return;
        };
        let doc_len = self.state.doc.len();
        let diags: Vec<crate::lint::Diagnostic> = items
            .iter()
            .filter_map(|d| {
                let range = d.get("range")?;
                let from = lsp.map_synced(range.get("start")?)?.min(doc_len);
                let to = lsp.map_synced(range.get("end")?)?.min(doc_len);
                let severity = match d.get("severity").and_then(serde_json::Value::as_u64) {
                    Some(2) => crate::lint::Severity::Warning,
                    Some(3) => crate::lint::Severity::Info,
                    Some(4) => crate::lint::Severity::Hint,
                    _ => crate::lint::Severity::Error,
                };
                Some(crate::lint::Diagnostic {
                    from,
                    to,
                    severity,
                    message: d.get("message")?.as_str()?.to_string(),
                    source: d
                        .get("source")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    mark_class: None,
                    actions: Vec::new(),
                })
            })
            .collect();
        self.set_diagnostics(diags, window, cx);
    }

    /// LSP bookkeeping after each transaction: auto-sync and signature help
    /// triggers.
    pub(crate) fn after_lsp_transaction(
        &mut self,
        tr: &Transaction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(lsp) = self.lsp.as_ref() else { return };
        if tr.doc_changed() {
            self.lsp_sync_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(crate::lsp::AUTO_SYNC_MS))
                    .await;
                this.update(cx, |v, _| {
                    let doc = v.state.doc.clone();
                    if let Some(l) = v.lsp.as_mut() {
                        l.sync(&doc);
                    }
                    v.lsp_sync_task = None;
                })
                .ok();
            }));
        }
        match lsp.signature_trigger(tr) {
            Some(crate::lsp::SignatureTrigger::Character(ch)) => {
                let is_retrigger = lsp.signature.is_some();
                let context = serde_json::json!({
                    "triggerKind": 2,
                    "isRetrigger": is_retrigger,
                    "triggerCharacter": ch,
                    "activeSignatureHelp": lsp.signature.as_ref().map(|s| s.data.clone()),
                });
                self.signature_task = None;
                self.request_signature(context, false, window, cx);
            }
            Some(crate::lsp::SignatureTrigger::ContentChange) => {
                self.signature_task = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(crate::lsp::SIGNATURE_RETRIGGER_MS))
                        .await;
                    this.update_in(cx, |v, window, cx| {
                        let Some(l) = v.lsp.as_ref() else { return };
                        let context = serde_json::json!({
                            "triggerKind": 3,
                            "isRetrigger": true,
                            "activeSignatureHelp": l.signature.as_ref().map(|s| s.data.clone()),
                        });
                        v.request_signature(context, true, window, cx);
                    })
                    .ok();
                }));
            }
            None => {}
        }
    }

    fn request_signature(
        &mut self,
        context: serde_json::Value,
        content_change: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let Some(lsp) = self.lsp.as_mut() else { return };
        let Some(req) = lsp.signature_request(&state, context) else {
            return;
        };
        let id = req.id;
        let fut = req.response;
        cx.spawn_in(window, async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| {
                let Some(lsp) = v.lsp.as_mut() else { return };
                let value = match result {
                    Ok(v) => Some(v),
                    Err(e) if e.is_cancelled() => return,
                    Err(_) => None,
                };
                if lsp.signature_response(id, value, content_change) {
                    v.tooltips.signature_changed();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// `showSignatureHelp` (Mod-Shift-Space).
    fn show_signature_help(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(lsp) = self.lsp.as_ref() else {
            return false;
        };
        if lsp.client.has_capability("signatureHelpProvider") == Some(false) {
            return false;
        }
        let context = match lsp.signature.as_ref() {
            Some(s) => {
                serde_json::json!({ "triggerKind": 1, "isRetrigger": true, "activeSignatureHelp": s.data })
            }
            None => serde_json::json!({ "triggerKind": 1, "isRetrigger": false }),
        };
        self.request_signature(context, false, window, cx);
        true
    }

    fn step_signature(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(lsp) = self.lsp.as_mut() else {
            return false;
        };
        match lsp.step_signature(forward) {
            Some(_) => {
                self.tooltips.signature_changed();
                cx.notify();
                true
            }
            None => false,
        }
    }

    /// `formatDocument` (Shift-Alt-f).
    fn format_document(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let state = self.state.clone();
        let Some(lsp) = self.lsp.as_mut() else {
            return false;
        };
        let req = lsp.format_request(&state);
        let start_doc = state.doc.clone();
        let since_start = self.state.doc.len();
        let fut = req.response;
        self.format_since = Some(crate::change::ChangeSet::empty(since_start));
        cx.spawn_in(window, async move |this, cx| {
            let result = fut.await;
            this.update_in(cx, |v, window, cx| {
                let since = v
                    .format_since
                    .take()
                    .unwrap_or_else(|| crate::change::ChangeSet::empty(start_doc.len()));
                match result {
                    Ok(response) if !response.is_null() => {
                        if let Some(changes) = crate::lsp::LspAttachment::format_changes(
                            &start_doc,
                            &since.desc(),
                            &response,
                        ) {
                            v.dispatch(
                                TransactionSpec::default()
                                    .changes(changes)
                                    .user_event("format"),
                                window,
                                cx,
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        v.report_lsp_error("Formatting request failed", &e.message, window, cx)
                    }
                }
            })
            .ok();
        })
        .detach();
        true
    }

    /// Recur's F2 (`renameWithWorkspaceLoad`): prompt for a new name for
    /// the word at the cursor. Always consumes the key.
    fn rename_symbol(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(lsp) = self.lsp.as_ref() else {
            return true;
        };
        let head = self.state.selection.main().head();
        let Some(word) = self.state.word_at(head) else {
            return true;
        };
        if lsp.client.has_capability("renameProvider") != Some(true) {
            return true;
        }
        let text = self.state.slice_doc(word.from, word.to);
        self.open_rename(&text, window, cx);
        true
    }

    /// Send the rename and apply its `WorkspaceEdit` across files.
    pub(crate) fn do_rename(
        &mut self,
        new_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let Some(lsp) = self.lsp.as_mut() else { return };
        let Some(req) = lsp.rename_request(&state, new_name) else {
            return;
        };
        let client = lsp.client.clone();
        let fut = req.response;
        cx.spawn_in(window, async move |this, cx| match fut.await {
            Ok(edit) if !edit.is_null() => {
                let ok = client.apply_workspace_edit(edit).await;
                this.update_in(cx, |v, window, cx| match ok {
                    Ok(true) => {}
                    Ok(false) => cx.emit(EditorEvent::Toast {
                        error: false,
                        text: "Rename skipped: could not load every affected file".into(),
                    }),
                    Err(e) => v.report_lsp_error("Rename request failed", &e.message, window, cx),
                })
                .ok();
            }
            Ok(_) => {}
            Err(e) => {
                this.update_in(cx, |v, window, cx| {
                    v.report_lsp_error("Rename request failed", &e.message, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// Apply a `WorkspaceEdit`'s changes for this editor's file (the
    /// workspace's `updateFile` for an open view).
    pub fn apply_lsp_text_edits(
        &mut self,
        edits: &serde_json::Value,
        user_event: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(lsp) = self.lsp.as_ref() else { return };
        let Some(items) = edits.as_array() else {
            return;
        };
        let mut changes = Vec::new();
        for e in items {
            let Some(range) = e.get("range") else {
                continue;
            };
            let (Some(from), Some(to)) = (
                range.get("start").and_then(|p| lsp.map_synced(p)),
                range.get("end").and_then(|p| lsp.map_synced(p)),
            ) else {
                continue;
            };
            let text = e
                .get("newText")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            changes.push(Change::new(from, to, text));
        }
        if !changes.is_empty() {
            self.dispatch(
                TransactionSpec::default()
                    .changes(changes)
                    .user_event(user_event),
                window,
                cx,
            );
        }
    }

    /// `jumpToDefinition`; false without a capable server.
    fn run_lsp_goto_def(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let state = self.state.clone();
        let Some(lsp) = self.lsp.as_mut() else {
            return false;
        };
        let Some(req) = lsp.definition_request(&state) else {
            return false;
        };
        let uri = lsp.uri.clone();
        let client = lsp.client.clone();
        let fut = req.response;
        cx.spawn_in(window, async move |this, cx| {
            let result = fut.await;
            match result {
                Ok(resp) => {
                    let Some((target, start)) = crate::lsp::LspAttachment::first_location(&resp)
                    else {
                        return;
                    };
                    if target == uri {
                        this.update_in(cx, |v, window, cx| {
                            let Some(l) = v.lsp.as_ref() else { return };
                            let Some(pos) = l.map_synced(&start) else {
                                return;
                            };
                            let pos = pos.min(v.state.doc.len());
                            v.dispatch(
                                TransactionSpec::default()
                                    .cursor(pos)
                                    .scroll()
                                    .user_event("select.definition"),
                                window,
                                cx,
                            );
                        })
                        .ok();
                    } else {
                        let jump = crate::lsp::LspJump {
                            line: start
                                .get("line")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0) as u32,
                            character: start
                                .get("character")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0) as u32,
                        };
                        let _ = client.display_file(&target, jump).await;
                    }
                }
                Err(e) => {
                    this.update_in(cx, |v, window, cx| {
                        v.report_lsp_error("Find definition failed", &e.message, window, cx)
                    })
                    .ok();
                }
            }
        })
        .detach();
        true
    }
}

fn is_modifier(name: &str) -> bool {
    matches!(
        name,
        "Shift" | "Control" | "Alt" | "Meta" | "CapsLock" | "AltGraph"
    )
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        recur_ui::guard::catch("editor text_for_range", || {
            let doc = &self.state.doc;
            let from = doc.utf16_to_byte(range.start);
            let to = doc.utf16_to_byte(range.end).max(from);
            *adjusted = Some(doc.byte_to_utf16(from)..doc.byte_to_utf16(to));
            Some(doc.slice(from, to))
        })
        .flatten()
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        recur_ui::guard::catch("editor selected_text_range", || {
            let main = self.state.selection.main();
            let doc = &self.state.doc;
            Some(UTF16Selection {
                range: doc.byte_to_utf16(main.from)..doc.byte_to_utf16(main.to),
                reversed: main.head() < main.anchor(),
            })
        })
        .flatten()
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        recur_ui::guard::catch("editor marked_text_range", || {
            let (from, to) = self.marked?;
            let doc = &self.state.doc;
            Some(doc.byte_to_utf16(from)..doc.byte_to_utf16(to))
        })
        .flatten()
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        recur_ui::guard::catch("editor unmark_text", || {
            self.end_composition(window, cx);
        });
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        recur_ui::guard::catch("editor replace_text_in_range", || {
            let (from, to) = match (range, self.marked) {
                (Some(r), _) => (
                    self.state.doc.utf16_to_byte(r.start),
                    self.state.doc.utf16_to_byte(r.end),
                ),
                (None, Some(m)) => m,
                (None, None) => {
                    let m = self.state.selection.main();
                    (m.from, m.to)
                }
            };
            self.input_text(from, to, text, window, cx);
            self.end_composition(window, cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        recur_ui::guard::catch("editor replace_and_mark_text_in_range", || {
            if !self.composing {
                self.composing = true;
                self.composition_first = true;
                self.completion.composition_start();
                if let Some(v) = self.vim.as_mut() {
                    v.use_next_text_input = true;
                }
            }
            let (from, to) = match (range, self.marked) {
                (Some(r), _) => (
                    self.state.doc.utf16_to_byte(r.start),
                    self.state.doc.utf16_to_byte(r.end),
                ),
                (None, Some(m)) => m,
                (None, None) => {
                    let m = self.state.selection.main();
                    (m.from, m.to)
                }
            };
            self.input_text(from, to, new_text, window, cx);
            if !self.composing {
                return;
            }
            let end = from + new_text.len();
            self.marked = (!new_text.is_empty()).then_some((from, end));
            if let Some(sel) = new_selected {
                let base16 = self.state.doc.byte_to_utf16(from);
                let a = self.state.doc.utf16_to_byte(base16 + sel.start);
                let b = self.state.doc.utf16_to_byte(base16 + sel.end);
                if self.state.selection.main().from != a || self.state.selection.main().to != b {
                    self.dispatch(
                        TransactionSpec::default().selection(EditorSelection::single(a, b)),
                        window,
                        cx,
                    );
                }
            }
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        recur_ui::guard::catch("editor bounds_for_range", || {
            let f = self.frame.as_ref()?;
            let pos = self.state.doc.utf16_to_byte(range_utf16.start);
            let (top, bottom) = self.with_layout_view(window, |lv| lv.row_box(pos, 1));
            let x = self.with_layout_view(window, |lv| lv.x_at(pos, 1));
            let left = f32::from(f.bounds.origin.x) + f.gutter_width - self.scroll_left
                + self.metrics.line_padding_left
                + x;
            Some(Bounds::new(
                point(px(left), px(f.scroller_top - self.scroll_top + top)),
                size(px(2.0), px(bottom - top)),
            ))
        })
        .flatten()
    }

    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        recur_ui::guard::catch("editor character_index_for_point", || {
            let (pos, _) = self.pos_at_pointer(p, window);
            Some(self.state.doc.byte_to_utf16(pos))
        })
        .flatten()
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        !self.read_only
    }
}

impl EditorView {
    pub(crate) fn end_composition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.composing {
            return;
        }
        self.composing = false;
        self.marked = None;
        if let Some(spec) = self.completion.composition_end() {
            self.completion_restart_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                this.update_in(cx, |v, window, cx| v.dispatch(spec, window, cx))
                    .ok();
            }));
        }
    }

    /// The editor lost focus: close completion unless it moved into the
    /// list (dispatched 10ms later, like CodeMirror).
    pub(crate) fn blurred(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.has_focus = false;
        self.blink_task = None;
        self.end_composition(window, cx);
        if let Some(spec) = self.completion.blur(&self.state, false) {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;
                this.update_in(cx, |v, window, cx| {
                    if !v.focus.is_focused(window) {
                        v.dispatch(spec, window, cx);
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// `MouseSelection.update`: typing ends a drag; other edits map its
    /// anchor.
    pub(crate) fn map_drag(&mut self, tr: &Transaction) {
        if !tr.doc_changed() {
            return;
        }
        if tr.is_user_event("input.type") {
            self.drag = None;
            return;
        }
        let changes = tr.changes.desc();
        match self.drag.as_mut() {
            Some(MouseDrag::Select {
                start_pos,
                start_sel,
                ..
            }) => {
                *start_pos = changes.map_pos(*start_pos, 1);
                *start_sel = start_sel.map(&changes, -1);
            }
            Some(MouseDrag::Rect {
                start, start_sel, ..
            }) => {
                let from = tr.start_state.doc.line(start.line).from;
                let mapped = changes.map_pos(from, 1);
                let line = tr.state().doc.line_at(mapped);
                start.line = line.number;
                start.off = start.off.min(line.len());
                *start_sel = start_sel.map(&changes, -1);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{EditorState, StateConfig};

    #[test]
    fn click_kinds_and_rectangles() {
        let s = EditorState::create("foo bar\n\tbaz\nx", StateConfig::default());
        assert_eq!(
            selection_for_click(&s, 5, 1, 2),
            SelectionRange::range(4, 7)
        );
        assert_eq!(
            selection_for_click(&s, 5, 1, 3),
            SelectionRange::range(0, 8)
        );
        let r = rectangle_for(
            &s,
            RectPos {
                line: 1,
                col: 1,
                off: 1,
            },
            RectPos {
                line: 3,
                col: 5,
                off: 1,
            },
        );
        assert_eq!(r[0], SelectionRange::range(1, 5));
        assert_eq!(r[1], SelectionRange::range(9, 10));
        assert_eq!(r[2], SelectionRange::cursor(14));
    }

    #[test]
    fn copy_and_paste_linewise() {
        let s = EditorState::create("one\ntwo", StateConfig::default());
        let (text, ranges, linewise) = copied_range(&s);
        assert_eq!(
            (text.as_str(), ranges, linewise),
            ("one", vec![(0, 4)], true)
        );
        LAST_LINEWISE_COPY.with(|c| *c.borrow_mut() = Some("one".into()));
        let s2 = EditorState::create_with(
            crate::text::Text::of("one\ntwo"),
            EditorSelection::cursor(5),
            StateConfig::default(),
        );
        let tr = s2.update(paste_spec(&s2, "one"));
        assert_eq!(tr.state().doc.to_string(), "one\none\ntwo");
        LAST_LINEWISE_COPY.with(|c| *c.borrow_mut() = None);
        let tr = s2.update(paste_spec(&s2, "X"));
        assert_eq!(tr.state().doc.to_string(), "one\ntXwo");
    }
}
