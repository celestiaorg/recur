//! Emacs keybindings (`@replit/codemirror-emacs` 6.1.0): the key-chain and
//! universal-argument handler, mark ring, kill ring and its commands, and
//! the block cursor. Recur loads it after the editor keymap, so it sees
//! the keys the default keymap leaves unhandled.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::{ClipboardItem, Context, KeyDownEvent, Window};

use crate::bindings::Cmd;
use crate::change::{Change, ChangeDesc, MapMode};
use crate::keymap::KeyEvent;
use crate::selection::{EditorSelection, SelectionRange};
use crate::state::{RangeChange, TransactionSpec};
use crate::view::EditorView;

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Cmd(Cmd),
    /// `goOrSelect`: the second command while a mark is active.
    GoOrSelect(Cmd, Cmd),
    /// `insertstring`: not a registered command, so lookups of it fail.
    InsertString,
    KillWord {
        left: bool,
    },
    ChangeCase {
        up: bool,
        region: bool,
    },
    Named(&'static str),
    /// A command name with no implementation (`gotoline`, `findnext`, …).
    Missing,
    /// A key-chain prefix.
    Prefix,
    UniversalArgument,
}

use Action::*;

/// `emacsKeys`.
fn emacs_keys() -> Vec<(&'static str, Action)> {
    vec![
        ("Up|C-p", GoOrSelect(Cmd::CursorLineUp, Cmd::SelectLineUp)),
        (
            "Down|C-n",
            GoOrSelect(Cmd::CursorLineDown, Cmd::SelectLineDown),
        ),
        (
            "Left|C-b",
            GoOrSelect(Cmd::CursorCharLeft, Cmd::SelectCharLeft),
        ),
        (
            "Right|C-f",
            GoOrSelect(Cmd::CursorCharRight, Cmd::SelectCharRight),
        ),
        (
            "C-Left|M-b",
            GoOrSelect(Cmd::CursorGroupLeft, Cmd::SelectGroupLeft),
        ),
        (
            "C-Right|M-f",
            GoOrSelect(Cmd::CursorGroupRight, Cmd::SelectGroupRight),
        ),
        (
            "Home|C-a",
            GoOrSelect(Cmd::CursorLineStart, Cmd::SelectLineStart),
        ),
        (
            "End|C-e",
            GoOrSelect(Cmd::CursorLineEnd, Cmd::SelectLineEnd),
        ),
        (
            "C-Home|S-M-,",
            GoOrSelect(Cmd::CursorDocStart, Cmd::SelectDocStart),
        ),
        (
            "C-End|S-M-.",
            GoOrSelect(Cmd::CursorDocEnd, Cmd::SelectDocEnd),
        ),
        ("S-Up|S-C-p", Cmd(Cmd::SelectLineUp)),
        ("S-Down|S-C-n", Cmd(Cmd::SelectLineDown)),
        ("S-Left|S-C-b", Cmd(Cmd::SelectCharLeft)),
        ("S-Right|S-C-f", Cmd(Cmd::SelectCharRight)),
        ("S-C-Left|S-M-b", Cmd(Cmd::SelectGroupLeft)),
        ("S-C-Right|S-M-f", Cmd(Cmd::SelectGroupRight)),
        ("S-Home|S-C-a", Cmd(Cmd::SelectLineStart)),
        ("S-End|S-C-e", Cmd(Cmd::SelectLineEnd)),
        ("S-C-Home", Cmd(Cmd::SelectDocStart)),
        ("S-C-End", Cmd(Cmd::SelectDocEnd)),
        ("C-l", Named("recenterTopBottom")),
        ("M-s", Named("centerSelection")),
        ("M-g", Missing),
        ("C-x C-p|C-x h", Cmd(Cmd::SelectAll)),
        (
            "PageDown|C-v|C-Down",
            GoOrSelect(Cmd::CursorPageDown, Cmd::SelectPageDown),
        ),
        // The package pairs PageUp with selectPageDown.
        (
            "PageUp|M-v|C-Up",
            GoOrSelect(Cmd::CursorPageUp, Cmd::SelectPageDown),
        ),
        ("S-C-Down", Cmd(Cmd::SelectPageDown)),
        ("S-C-Up", Cmd(Cmd::SelectPageUp)),
        ("C-s", Cmd(Cmd::OpenSearchPanel)),
        ("C-r", Cmd(Cmd::OpenSearchPanel)),
        ("M-C-s", Missing),
        ("M-C-r", Missing),
        ("S-M-5", Missing),
        ("Backspace", Cmd(Cmd::DeleteCharBackward)),
        ("Delete|C-d", Cmd(Cmd::DeleteCharForward)),
        ("Return|C-m", InsertString),
        ("C-o", Cmd(Cmd::SplitLine)),
        ("M-d|C-Delete", KillWord { left: false }),
        ("C-Backspace|M-Backspace|M-Delete", KillWord { left: true }),
        ("C-k", Named("killLine")),
        ("M-h", Named("selectParagraph")),
        ("M-@|M-S-2", Named("markWord")),
        ("C-y|S-Delete", Named("yank")),
        ("M-y", Named("yankRotate")),
        ("C-g", Named("keyboardQuit")),
        ("C-w|C-S-w", Named("killRegion")),
        ("M-w", Named("killRingSave")),
        ("C-Space", Named("setMark")),
        ("C-x C-x", Named("exchangePointAndMark")),
        ("C-t", Cmd(Cmd::TransposeChars)),
        (
            "M-u",
            ChangeCase {
                up: true,
                region: false,
            },
        ),
        (
            "M-l",
            ChangeCase {
                up: false,
                region: false,
            },
        ),
        (
            "C-x C-u",
            ChangeCase {
                up: true,
                region: true,
            },
        ),
        (
            "C-x C-l",
            ChangeCase {
                up: true,
                region: true,
            },
        ),
        ("M-/", Cmd(Cmd::StartCompletion)),
        ("C-u", UniversalArgument),
        ("M-;", Cmd(Cmd::ToggleComment)),
        ("C-/|C-x u|S-C--|C-z", Cmd(Cmd::Undo)),
        ("S-C-/|S-C-x u|C--|S-C-z", Cmd(Cmd::Redo)),
        ("C-x r", Named("selectRectangularRegion")),
        ("M-x", Named("focusCommandLine")),
        ("Esc", Named("unsetTransientMark")),
    ]
}

/// `EmacsHandler.bindKey` over every binding.
fn command_key_binding() -> HashMap<String, Action> {
    let mut map = HashMap::new();
    for (group, action) in emacs_keys() {
        for binding in group.split('|') {
            let mut chain = String::new();
            let parts: Vec<&str> = binding.split_whitespace().collect();
            for (index, part) in parts.iter().enumerate() {
                let mut pieces = split_modifiers(part);
                let key = pieces.pop().unwrap_or_default();
                if !pieces.is_empty() {
                    pieces.sort();
                    chain.push_str(&pieces.join("-"));
                    chain.push('-');
                }
                chain.push_str(&key);
                if index == parts.len() - 1 {
                    map.insert(chain.clone(), action.clone());
                } else {
                    map.insert(chain.clone(), Prefix);
                    chain.push(' ');
                }
            }
        }
    }
    map
}

/// `keyGroup.split(/-(?=.)/)`.
fn split_modifiers(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '-' && i + 1 < chars.len() {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// `EmacsHandler.getKey`: the DOM `code` (`specialKey`-mapped), the
/// modifier prefix and `event.key`.
fn get_key(k: &KeyEvent) -> Option<(String, String, String)> {
    let base = k.base.clone().unwrap_or_else(|| k.name.clone());
    let code = match base.as_str() {
        "ArrowLeft" => "Left".to_string(),
        "ArrowRight" => "Right".to_string(),
        "ArrowUp" => "Up".to_string(),
        "ArrowDown" => "Down".to_string(),
        "Enter" => "Return".to_string(),
        "Escape" => "Esc".to_string(),
        "Insert" => "Ins".to_string(),
        " " => "Space".to_string(),
        "-" => "-".to_string(),
        "=" => "=".to_string(),
        "/" => "/".to_string(),
        "," => "Comma".to_string(),
        "." => "Period".to_string(),
        ";" => "Semicolon".to_string(),
        "'" => "Quote".to_string(),
        "`" => "Backquote".to_string(),
        "[" => "BracketLeft".to_string(),
        "]" => "BracketRight".to_string(),
        "\\" => "Backslash".to_string(),
        b if b.len() == 1 && b.as_bytes()[0].is_ascii_digit() => format!("Digit{b}"),
        b if b.chars().count() == 1 => b.to_lowercase(),
        b => b.to_string(),
    };
    if code.is_empty() {
        return None;
    }
    let mut modifier = String::new();
    if k.ctrl {
        modifier.push_str("C-");
    }
    if k.meta {
        modifier.push_str("CMD-");
    }
    if k.alt {
        modifier.push_str("M-");
    }
    if k.shift {
        modifier.push_str("S-");
    }
    Some((code, modifier, k.name.clone()))
}

thread_local! {
    static BINDINGS: HashMap<String, Action> = command_key_binding();
    /// `killRing`: shared by every editor.
    static KILL_RING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn kill_add(s: &str) {
    KILL_RING.with(|r| {
        let mut r = r.borrow_mut();
        if !s.is_empty() {
            r.push(s.to_string());
        }
        if r.len() > 30 {
            r.remove(0);
        }
    });
}

fn kill_append(s: &str) {
    KILL_RING.with(|r| {
        let mut r = r.borrow_mut();
        let mut text = r.last().cloned().unwrap_or_default();
        text.push_str(s);
        if !text.is_empty() {
            match r.last_mut() {
                Some(last) => *last = text,
                None => r.push(text),
            }
        }
    });
}

fn kill_get() -> String {
    KILL_RING.with(|r| r.borrow().last().cloned().unwrap_or_default())
}

fn kill_rotate() -> String {
    KILL_RING.with(|r| {
        let mut r = r.borrow_mut();
        if let Some(last) = r.pop() {
            r.insert(0, last);
        }
    });
    kill_get()
}

/// The handler's state (`EmacsHandler`).
#[derive(Debug, Default)]
pub struct Emacs {
    count: i64,
    key_chain: String,
    last_command: Option<&'static str>,
    mark_ring: Vec<Vec<usize>>,
    mark: Option<Vec<usize>>,
}

struct Found {
    action: Action,
    count: i64,
}

impl Emacs {
    pub fn new() -> Emacs {
        Emacs::default()
    }

    /// A count or key chain is pending (the block cursor halves).
    pub fn pending(&self) -> bool {
        self.count != 0 || !self.key_chain.is_empty()
    }

    fn push_mark(&mut self, p: Option<Vec<usize>>, activate: bool) {
        if let Some(prev) = self.mark.clone() {
            push_unique(&mut self.mark_ring, prev);
        }
        match p {
            Some(p) if !activate => push_unique(&mut self.mark_ring, p),
            p => self.mark = p,
        }
    }

    fn pop_mark(&mut self) -> Option<Vec<usize>> {
        if let Some(m) = self.mark.take() {
            return Some(m);
        }
        self.mark_ring.pop()
    }

    /// `updateMarksOnChange` (and the plugin clears the active mark).
    pub fn doc_changed(&mut self, changes: &ChangeDesc) {
        self.mark = None;
        let map = |m: &Vec<usize>| -> Option<Vec<usize>> {
            let v: Vec<usize> = m
                .iter()
                .filter_map(|&p| changes.map_pos_mode(p, 1, MapMode::TrackDel))
                .collect();
            (!v.is_empty()).then_some(v)
        };
        self.mark_ring = self.mark_ring.iter().filter_map(map).collect();
    }

    pub fn mouse_down(&mut self) {
        self.mark = None;
    }

    /// `findCommand`; `Err(())` stands for a command that throws.
    fn find_command(&mut self, key: &str, modifier: &str, text: &str) -> Option<Result<Found, ()>> {
        let mut key = key.to_string();
        if modifier.is_empty() && key.chars().count() == 1 {
            self.push_mark(None, false);
            if self.count != 0 {
                self.count = 0;
                let _ = text;
                return Some(Err(()));
            }
        }
        if (modifier == "C-" || self.count != 0)
            && let Some(d) = key.chars().last().and_then(|c| c.to_digit(10))
        {
            self.count = 10 * self.count.max(0) + d as i64;
            return Some(Ok(Found {
                action: Prefix,
                count: 1,
            }));
        }
        if !modifier.is_empty() {
            key = format!("{modifier}{key}");
        }
        if !self.key_chain.is_empty() {
            self.key_chain = format!("{} {}", self.key_chain, key);
            key = self.key_chain.clone();
        }
        let command = BINDINGS.with(|b| b.get(&key).cloned());
        self.key_chain = if command == Some(Prefix) {
            key
        } else {
            String::new()
        };
        let command = command?;
        match command {
            Prefix => {
                return Some(Ok(Found {
                    action: Prefix,
                    count: 1,
                }));
            }
            UniversalArgument => {
                self.count = -4;
                return Some(Ok(Found {
                    action: Prefix,
                    count: 1,
                }));
            }
            InsertString | Cmd(Cmd::SplitLine) | Cmd(Cmd::ToggleComment) => {
                self.push_mark(None, false)
            }
            _ => {}
        }
        if matches!(command, InsertString | Missing) {
            return None;
        }
        let read_only = matches!(
            command,
            Named("setMark") | Named("exchangePointAndMark") | Named("killRingSave")
        );
        let keep_last = matches!(
            command,
            Named("killLine") | Named("yank") | Named("yankRotate")
        );
        if !read_only && !keep_last {
            self.last_command = None;
        }
        let count = if self.count != 0 { self.count } else { 1 };
        self.count = 0;
        Some(Ok(Found {
            action: command,
            count,
        }))
    }
}

fn push_unique(ring: &mut Vec<Vec<usize>>, item: Vec<usize>) {
    if ring.last() == Some(&item) {
        return;
    }
    ring.push(item);
}

fn handles_count(a: &Action) -> bool {
    matches!(a, Named("setMark") | Named("exchangePointAndMark"))
}

/// `handleKeyboard` for a key the editor keymap left unhandled.
pub(crate) fn key_down(
    view: &mut EditorView,
    key: &KeyEvent,
    _ev: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> bool {
    let Some((code, modifier, text)) = get_key(key) else {
        return false;
    };
    let Some(em) = view.emacs.as_mut() else {
        return false;
    };
    let found = em.find_command(&code, &modifier, &text);
    if (code.contains("Up") || code.contains("Down")) && view.state.completion.status().is_some() {
        return false;
    }
    let Some(found) = found else { return false };
    let Ok(found) = found else { return false };
    let mut count = found.count.abs();
    let mut arg_count = None;
    if count > 1 && handles_count(&found.action) {
        arg_count = Some(count);
        count = 1;
    }
    let mut result = true;
    for _ in 0..count {
        result = exec(view, &found.action, arg_count, window, cx);
    }
    view.restart_blink(cx);
    cx.notify();
    result
}

fn has_mark(view: &EditorView) -> bool {
    view.emacs.as_ref().is_some_and(|e| e.mark.is_some())
}

fn em(view: &mut EditorView) -> &mut Emacs {
    view.emacs.get_or_insert_with(Emacs::new)
}

/// `clearSelection`.
fn clear_selection(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> bool {
    let sel = view.state.selection.clone();
    if sel.ranges.iter().all(SelectionRange::is_empty) {
        return false;
    }
    let ranges = sel
        .ranges
        .iter()
        .map(|r| SelectionRange::range(r.head(), r.head()))
        .collect();
    view.dispatch(
        TransactionSpec::default().selection(EditorSelection::create(ranges, sel.main_index)),
        window,
        cx,
    );
    true
}

fn copy_text(view: &EditorView) -> String {
    let s = &view.state;
    s.selection
        .ranges
        .iter()
        .map(|r| s.slice_doc(r.from, r.to))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `onPaste`.
fn paste(view: &mut EditorView, text: &str, window: &mut Window, cx: &mut Context<EditorView>) {
    let state = view.state.clone();
    let lines: Option<Vec<&str>> = (state.selection.ranges.len() > 1)
        .then(|| text.split('\n').collect::<Vec<_>>())
        .filter(|l| l.len() == state.selection.ranges.len());
    let mut i = 0;
    let spec = state
        .change_by_range(|range| {
            let insert = lines.as_ref().map_or(text, |l| l[i]).to_string();
            i += 1;
            let len = insert.len();
            RangeChange::with(
                vec![Change::new(range.from, range.to, insert)],
                SelectionRange::cursor(range.from + len),
            )
        })
        .spec();
    view.dispatch(spec, window, cx);
}

fn exec(
    view: &mut EditorView,
    action: &Action,
    arg_count: Option<i64>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> bool {
    match action {
        Cmd(c) => {
            view.run_cmd(*c, window, cx);
        }
        GoOrSelect(go, select) => {
            let c = if has_mark(view) { *select } else { *go };
            view.run_cmd(c, window, cx);
        }
        KillWord { left } => {
            let sel = view.state.selection.clone();
            let ranges = sel
                .ranges
                .iter()
                .map(|r| SelectionRange::range(r.head(), r.head()))
                .collect();
            view.dispatch(
                TransactionSpec::default()
                    .selection(EditorSelection::create(ranges, sel.main_index)),
                window,
                cx,
            );
            view.run_cmd(
                if *left {
                    Cmd::SelectGroupLeft
                } else {
                    Cmd::SelectGroupRight
                },
                window,
                cx,
            );
            let state = view.state.clone();
            for r in &state.selection.ranges {
                kill_add(&state.slice_doc(r.from, r.to));
            }
            view.dispatch(state.replace_selection("").spec(), window, cx);
        }
        ChangeCase { up, region } => {
            if !region {
                clear_selection(view, window, cx);
                view.run_cmd(Cmd::SelectGroupRight, window, cx);
            }
            let state = view.state.clone();
            let spec = state
                .change_by_range(|range| {
                    let text = state.slice_doc(range.from, range.to);
                    let text = if *up {
                        text.to_uppercase()
                    } else {
                        text.to_lowercase()
                    };
                    let len = text.len();
                    RangeChange::with(
                        vec![Change::new(range.from, range.to, text)],
                        SelectionRange::cursor(range.from + len),
                    )
                })
                .spec();
            view.dispatch(spec, window, cx);
        }
        Named(name) => return exec_named(view, name, arg_count, window, cx),
        InsertString | Missing | Prefix | UniversalArgument => {}
    }
    true
}

fn exec_named(
    view: &mut EditorView,
    name: &str,
    arg_count: Option<i64>,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> bool {
    match name {
        "unsetTransientMark" => {
            em(view).mark = None;
            return false;
        }
        "markWord" | "focusCommandLine" => {}
        "selectParagraph" => {
            let doc = view.state.doc.clone();
            let head = view.state.selection.ranges[0].head();
            let has_text = |n: usize| {
                doc.line_text(n)
                    .chars()
                    .any(|c| !crate::state::is_js_space(c))
            };
            let start_line = doc.line_at(head);
            let (mut start, mut end): (Option<usize>, Option<usize>) = (None, None);
            let mut line = start_line;
            while has_text(line.number) && line.from > 0 {
                start = Some(line.from);
                line = doc.line_at(line.from - 1);
            }
            if start.is_none() {
                while !has_text(line.number) && line.to < doc.len() {
                    start = Some(line.from);
                    line = doc.line_at(line.to + 1);
                }
            } else {
                line = start_line;
            }
            while has_text(line.number) && line.to < doc.len() {
                end = Some(line.to);
                line = doc.line_at(line.to + 1);
            }
            // `EditorSelection.range(-1, …)` would throw for an unset start.
            let Some(start) = start else { return true };
            let end = end.unwrap_or(start_line.to);
            view.dispatch(
                TransactionSpec::default().selection(EditorSelection::create(
                    vec![SelectionRange::range(start, end)],
                    0,
                )),
                window,
                cx,
            );
        }
        "centerSelection" => {
            view.dispatch(TransactionSpec::default().scroll(), window, cx);
        }
        "recenterTopBottom" => {
            let before = view.scroll_top;
            view.dispatch(TransactionSpec::default().scroll(), window, cx);
            view.resolve_pending_scroll(window);
            if before != view.scroll_top {
                return true;
            }
            let head = view.state.selection.main().head();
            let Some(b) = view.coords_at_pos(head, 1, window) else {
                return true;
            };
            let Some(f) = view.frame.clone() else {
                return true;
            };
            let line_height = f32::from(b.size.height);
            let screen = f.scroller_height;
            let cursor_top = f32::from(b.origin.y) - f.scroller_top;
            let mut top = before;
            if cursor_top.abs() < line_height / 4.0 {
                top += cursor_top + line_height - screen + 2.0;
            } else if (cursor_top - screen * 0.5).abs() < line_height / 4.0 {
                top += cursor_top - 2.0;
            } else {
                top += cursor_top - screen * 0.5;
            }
            view.scroll_top = top.clamp(0.0, f.max_scroll_top);
        }
        "selectRectangularRegion" => {
            let state = view.state.clone();
            let ranges = &state.selection.ranges;
            let mut out = Vec::new();
            if ranges.len() > 1 {
                out.push(SelectionRange::range(
                    ranges[0].from,
                    ranges[ranges.len() - 1].to,
                ));
            } else {
                let doc = &state.doc;
                let mut start_line = doc.line_at(ranges[0].from);
                let end_line = doc.line_at(ranges[0].to);
                let start_col = ranges[0].from - start_line.from;
                let end_col = ranges[0].to - end_line.from;
                while start_line.from < end_line.to {
                    let a = (start_line.from + start_col).min(doc.len());
                    let b = (start_line.from + end_col).min(doc.len());
                    out.push(SelectionRange::range(a, b));
                    if start_line.to + 1 >= doc.len() {
                        break;
                    }
                    start_line = doc.line_at(start_line.to + 1);
                }
            }
            if !out.is_empty() {
                view.dispatch(
                    TransactionSpec::default().selection(EditorSelection::create(out, 0)),
                    window,
                    cx,
                );
            }
        }
        "setMark" => {
            let ranges = view.state.selection.ranges.clone();
            if arg_count.is_some() {
                let new_mark: Vec<usize> = ranges.iter().map(|r| r.head()).collect();
                if let Some(mark) = em(view).pop_mark() {
                    let rs = mark.iter().map(|&p| SelectionRange::cursor(p)).collect();
                    view.dispatch(
                        TransactionSpec::default().selection(EditorSelection::create(rs, 0)),
                        window,
                        cx,
                    );
                    em(view).mark_ring.insert(0, new_mark);
                }
                return true;
            }
            let mark = em(view).mark.clone();
            let positions: Vec<usize> = ranges.iter().map(|r| r.head()).collect();
            let no_selection = ranges.iter().all(SelectionRange::is_empty);
            if mark.is_some() || !no_selection {
                clear_selection(view, window, cx);
                if mark.is_some() {
                    em(view).push_mark(None, false);
                }
                return true;
            }
            let e = em(view);
            e.push_mark(Some(positions.clone()), false);
            e.mark = Some(positions);
        }
        "exchangePointAndMark" => {
            let sel = view.state.selection.clone();
            let empty = sel.ranges.iter().all(SelectionRange::is_empty);
            if arg_count.is_none() && !empty {
                let rs = sel
                    .ranges
                    .iter()
                    .map(|r| SelectionRange::range(r.head(), r.anchor()))
                    .collect();
                view.dispatch(
                    TransactionSpec::default()
                        .selection(EditorSelection::create(rs, sel.main_index)),
                    window,
                    cx,
                );
                return true;
            }
            let Some(last) = em(view).mark_ring.last().cloned() else {
                return true;
            };
            if arg_count.is_some() {
                let current: Vec<usize> = sel.ranges.iter().map(|r| r.head()).collect();
                if let Some(slot) = em(view).mark_ring.last_mut() {
                    *slot = current;
                }
                clear_selection(view, window, cx);
                let rs = last.iter().map(|&p| SelectionRange::range(p, p)).collect();
                view.dispatch(
                    TransactionSpec::default()
                        .selection(EditorSelection::create(rs, sel.main_index)),
                    window,
                    cx,
                );
            }
            // Without a count the package builds a selection to the last
            // mark but never dispatches it.
        }
        "killLine" => {
            em(view).push_mark(None, false);
            clear_selection(view, window, cx);
            let state = view.state.clone();
            let mut text = Vec::new();
            let changes: Vec<Change> = state
                .selection
                .ranges
                .iter()
                .map(|range| {
                    let from = range.head();
                    let line = state.doc.line_at(from);
                    let mut to = line.to;
                    let rest = state.slice_doc(from, to);
                    if rest.chars().all(crate::state::is_js_space) && to + 1 < state.doc.len() {
                        to += 1;
                        text.push(format!("{rest}\n"));
                    } else {
                        text.push(rest);
                    }
                    Change::delete(from, to)
                })
                .collect();
            if em(view).last_command == Some("killLine") {
                kill_append(&text.join("\n"));
            } else {
                kill_add(&text.join("\n"));
            }
            em(view).last_command = Some("killLine");
            view.dispatch(TransactionSpec::default().changes(changes), window, cx);
        }
        "yank" => {
            paste(view, &kill_get(), window, cx);
            em(view).last_command = Some("yank");
        }
        "yankRotate" => {
            if em(view).last_command != Some("yank") {
                return true;
            }
            view.run_cmd(Cmd::Undo, window, cx);
            em(view).mark_ring.pop();
            paste(view, &kill_rotate(), window, cx);
            em(view).last_command = Some("yank");
        }
        "killRegion" => {
            kill_add(&copy_text(view));
            let state = view.state.clone();
            view.dispatch(state.replace_selection("").spec(), window, cx);
            em(view).mark = None;
        }
        "killRingSave" => {
            let text = copy_text(view);
            kill_add(&text);
            clear_selection(view, window, cx);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        "keyboardQuit" => {
            let sel = view.state.selection.clone();
            let empty = sel.ranges.iter().all(SelectionRange::is_empty);
            if sel.ranges.len() > 1 && !empty {
                let rs = sel
                    .ranges
                    .iter()
                    .map(|r| SelectionRange::range(r.head(), r.head()))
                    .collect();
                view.dispatch(
                    TransactionSpec::default()
                        .selection(EditorSelection::create(rs, sel.main_index)),
                    window,
                    cx,
                );
            } else {
                view.run_cmd(Cmd::SimplifySelection, window, cx);
            }
            let e = em(view);
            e.mark = None;
            e.count = 0;
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bindings_sort_modifiers_and_chain() {
        let b = command_key_binding();
        assert_eq!(b.get("C-x"), Some(&Prefix));
        assert_eq!(b.get("C-x h"), Some(&Cmd(Cmd::SelectAll)));
        assert_eq!(b.get("C-S-p"), Some(&Cmd(Cmd::SelectLineUp)));
        assert_eq!(b.get("C-S--"), Some(&Cmd(Cmd::Undo)));
        assert_eq!(
            b.get("M-S-,"),
            Some(&GoOrSelect(Cmd::CursorDocStart, Cmd::SelectDocStart))
        );
        assert_eq!(split_modifiers("S-C--"), vec!["S", "C", "-"]);
    }

    #[test]
    fn counts_and_chains() {
        let mut e = Emacs::new();
        assert!(matches!(
            e.find_command("u", "C-", "u"),
            Some(Ok(Found { action: Prefix, .. }))
        ));
        assert_eq!(e.count, -4);
        assert!(matches!(
            e.find_command("Digit3", "", "3"),
            Some(Ok(Found { action: Prefix, .. }))
        ));
        assert_eq!(e.count, 3);
        let f = e.find_command("f", "C-", "f").unwrap().unwrap();
        assert_eq!(
            (f.action, f.count),
            (GoOrSelect(Cmd::CursorCharRight, Cmd::SelectCharRight), 3)
        );
        assert!(matches!(
            e.find_command("x", "C-", "x"),
            Some(Ok(Found { action: Prefix, .. }))
        ));
        assert!(e.pending());
        let f = e.find_command("h", "", "h").unwrap().unwrap();
        assert_eq!(f.action, Cmd(Cmd::SelectAll));
        assert!(e.find_command("Return", "", "Enter").is_none());
    }

    #[test]
    fn kill_ring_appends_and_rotates() {
        KILL_RING.with(|r| r.borrow_mut().clear());
        kill_add("a");
        kill_append("b");
        kill_add("c");
        assert_eq!(kill_get(), "c");
        assert_eq!(kill_rotate(), "ab");
    }
}
