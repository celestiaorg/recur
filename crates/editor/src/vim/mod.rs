//! Vim keybindings: a port of `@replit/codemirror-vim` 6.3.0 (vim.js on its
//! CodeMirror 5 adapter, plus the CodeMirror 6 plugin: key handling, block
//! cursor, search highlighting and the bottom `.cm-vim-panel`). Recur
//! loads it after the editor keymap, so keys the keymap handles never
//! reach vim.

pub(crate) mod actions;
pub(crate) mod cm;
pub(crate) mod dispatch;
pub(crate) mod ex;
pub(crate) mod helpers;
pub(crate) mod insert;
pub(crate) mod keymap;
pub(crate) mod motions;
pub(crate) mod operators;
pub(crate) mod prompt;
pub(crate) mod search;
pub(crate) mod state;
pub(crate) mod types;

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{Context, KeyDownEvent, Task, Window};

use cm::{Cm, CmState};
use keymap::{Kind, Match, VimKey};
use state::{VimState, with_global};
use types::*;

use crate::keymap::KeyEvent;
use crate::view::EditorView;

/// `langmap`.
#[derive(Clone, Debug, Default)]
pub struct Langmap {
    pub keymap: HashMap<String, String>,
    pub string: String,
    pub remap_ctrl: Option<bool>,
}

/// `parseLangmap`.
pub fn parse_langmap(s: &str) -> Langmap {
    let mut keymap = HashMap::new();
    if s.is_empty() {
        return Langmap::default();
    }
    let escaped = |list: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut it = list.chars();
        while let Some(c) = it.next() {
            if c == '\\' {
                if let Some(n) = it.next() {
                    out.push(n.to_string());
                }
            } else {
                out.push(c.to_string());
            }
        }
        out
    };
    for part in split_unescaped(s, ',') {
        if part.is_empty() {
            continue;
        }
        let semi = split_unescaped(&part, ';');
        if semi.len() == 2 {
            let (from, to) = (escaped(&semi[0]), escaped(&semi[1]));
            if from.len() != to.len() {
                continue;
            }
            for (f, t) in from.into_iter().zip(to) {
                keymap.insert(f, t);
            }
        } else if semi.len() == 1 {
            let pairs = escaped(&part);
            if pairs.len() % 2 != 0 {
                continue;
            }
            for p in pairs.chunks(2) {
                keymap.insert(p[0].clone(), p[1].clone());
            }
        }
    }
    Langmap {
        keymap,
        string: s.to_string(),
        remap_ctrl: None,
    }
}

fn split_unescaped(s: &str, sep: char) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            out.last_mut().unwrap().push(c);
            if let Some(n) = it.next() {
                out.last_mut().unwrap().push(n);
            }
        } else if c == sep {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

/// One editor's vim mode.
pub(crate) struct Vim {
    pub state: VimState,
    pub cm: CmState,
    /// The vim match highlight (`vimPlugin.query`).
    pub highlight: Option<Rc<crate::search::SearchQuery>>,
    /// The bottom panel (`cm.state.dialog`).
    pub dialog: Option<prompt::Dialog>,
    pub notification_timer: Option<Task<()>>,
    pub highlight_timer: Option<Task<()>>,
    pub insert_key_timer: Option<Task<()>>,
    /// The editor's clipboard copy is pending (`waitForCopy`).
    pub wait_for_copy: bool,
    /// The next one-character text input is a key (`useNextTextInput`):
    /// set by composition and IME keydowns.
    pub use_next_text_input: bool,
    /// Text a literal-next composition inserted (`compositionText`).
    pub composition_text: String,
}

impl Vim {
    pub fn new() -> Vim {
        Vim {
            state: VimState::default(),
            cm: CmState::default(),
            highlight: None,
            dialog: None,
            notification_timer: None,
            highlight_timer: None,
            insert_key_timer: None,
            wait_for_copy: false,
            use_next_text_input: false,
            composition_text: String::new(),
        }
    }

    /// Whether the block cursor replaces the caret (`measureCursor`'s
    /// `fatCursor`), and its height fraction.
    pub fn fat_cursor(&self) -> Option<f32> {
        if self.state.insert_mode && !self.cm.overwrite {
            return None;
        }
        Some(if self.cm.overwrite {
            0.2
        } else if !self.state.status.is_empty() {
            0.5
        } else {
            1.0
        })
    }
}

/// `vim-command-done`: the plugin clears the status; `oneNormalCommand`
/// returns to insert mode.
pub(crate) fn command_done(cm: &mut Cm) {
    cm.vim().status.clear();
    let vim = cm.vim();
    if vim.one_normal_handler && !vim.visual_mode {
        vim.one_normal_handler = false;
        if vim.insert_mode_return {
            vim.insert_mode_return = false;
            if !cm.vim().insert_mode {
                actions::enter_insert_mode(cm, &ActionArgs::default());
            }
        }
    }
    cm.cx.notify();
}

/// `vim-mode-change`.
pub(crate) fn mode_change(cm: &mut Cm, mode: &str, sub_mode: Option<&str>) {
    let vim = cm.vim();
    vim.mode = mode.to_string();
    if sub_mode.is_some_and(|s| !s.is_empty()) {
        vim.mode.push_str(" block");
    }
    vim.status.clear();
    cm.cx.notify();
}

/// `clearInputState`.
pub(crate) fn clear_input_state(cm: &mut Cm) {
    let vim = cm.vim();
    vim.input_state = state::InputState::default();
    vim.expect_literal_next = false;
    command_done(cm);
}

/// The plugin's keydown (`handleKey`) for a key the editor keymap left.
pub(crate) fn key_down(
    view: &mut EditorView,
    key: &KeyEvent,
    ev: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> bool {
    let composing = view.composing;
    let Some(v) = view.vim.as_mut() else {
        return false;
    };
    // An IME keydown (`Process`) leaves the key to the text input.
    v.use_next_text_input = composing;
    if composing {
        return false;
    }
    let typed = ev.keystroke.key_char.as_deref();
    let Some(name) = event_key_name(view, key, typed, true) else {
        return false;
    };
    let dom_key = dom_key(key, typed);
    let mut cm = Cm::new(view, window, cx);
    handle_plugin_key(&mut cm, &name, &dom_key)
}

/// `event.key`: the typed character for printable keys.
fn dom_key(key: &KeyEvent, typed: Option<&str>) -> String {
    if key.name.chars().count() == 1 {
        return key.name.clone();
    }
    typed
        .filter(|t| t.chars().count() == 1 && !key.ctrl && !key.meta)
        .map_or_else(|| key.name.clone(), String::from)
}

/// `vimKeyFromEvent(e, vim)`; `with_vim` false is the prompts' call
/// without a vim state (no langmap, no literal-next handling).
pub(crate) fn event_key_name(
    view: &EditorView,
    key: &KeyEvent,
    typed: Option<&str>,
    with_vim: bool,
) -> Option<String> {
    let is_mac = view.platform == crate::keymap::Platform::Mac;
    let literal = !with_vim
        || view
            .vim
            .as_ref()
            .is_some_and(|v| v.state.expect_literal_next);
    let dom_key = dom_key(key, typed);
    with_global(|g| {
        keymap::vim_key_from_event(
            &dom_key,
            key.base.as_deref(),
            key.ctrl,
            key.alt,
            key.meta,
            key.shift,
            is_mac,
            literal,
            |k| g.key_used(k),
            &g.langmap,
        )
    })
}

/// `vimPlugin.handleKey` from `vimKeyFromEvent`'s name.
pub(crate) fn handle_plugin_key(cm: &mut Cm, key: &str, dom_key: &str) -> bool {
    let has_query = cm.view.vim.as_ref().is_some_and(|v| v.highlight.is_some());
    if key == "<Esc>" && !cm.vim().insert_mode && !cm.vim().visual_mode && has_query {
        cm.remove_overlay();
        cm.vim().search_state.overlay = None;
    }
    // `<C-c>` with a selection lets the native copy run, then leaves the
    // mode in `after_copy`.
    let is_copy = key == "<C-c>" && cm.view.platform != crate::keymap::Platform::Mac;
    if is_copy && cm.something_selected() {
        if let Some(v) = cm.view.vim.as_mut() {
            v.wait_for_copy = true;
        }
        return false;
    }
    cm.vim().status.push_str(key);
    let mut result = multi_select_handle_key(cm, key, "user");
    if !result && cm.vim().insert_mode && cm.overwrite() {
        if dom_key.chars().count() == 1 && dom_key != "\n" {
            result = true;
            cm.overwrite_selection(dom_key);
        } else if dom_key == "Backspace" {
            result = true;
            cm.run(crate::bindings::Cmd::CursorCharLeft);
        }
    }
    cm.view.restart_blink(cm.cx);
    cm.cx.notify();
    result
}

/// Text input while vim is active (the plugin's `inputHandler`): in
/// normal mode a one-character composition result is a key. Normal typing
/// never gets here because the keydown swallowed it.
pub(crate) fn input(
    view: &mut EditorView,
    _from: usize,
    _to: usize,
    text: &str,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) -> bool {
    let composing = view.composing;
    let Some(v) = view.vim.as_mut() else {
        return false;
    };
    if v.state.insert_mode || v.cm.cur_op.as_ref().is_some_and(|o| o.is_vim_op) {
        return false;
    }
    if text == "\0\0" {
        return true;
    }
    if text.chars().count() != 1 || !v.use_next_text_input {
        return false;
    }
    if v.state.expect_literal_next && composing {
        v.composition_text = text.to_string();
        return false;
    }
    let to_remove = std::mem::take(&mut v.composition_text);
    let mut cm = Cm::new(view, window, cx);
    if !to_remove.is_empty() {
        let head = cm.view.state.selection.main().head();
        let start = head.saturating_sub(to_remove.len());
        if cm.view.state.slice_doc(start, head) == to_remove {
            let pos = cm.get_cursor();
            let from = cm.pos_from_index(start);
            cm.replace_range("", from, Some(pos));
        }
    }
    handle_plugin_key(&mut cm, text, text);
    view.end_composition(window, cx);
    true
}

/// The content's `paste` listener (`getOnPasteFn`): pasting in normal
/// mode continues in insert mode after the cursor.
pub(crate) fn after_paste(
    view: &mut EditorView,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    if view.vim.as_ref().is_none_or(|v| v.state.insert_mode) {
        return;
    }
    let mut cm = Cm::new(view, window, cx);
    let c = cm.get_cursor();
    cm.set_cursor(offset(c, 0, 1));
    actions::enter_insert_mode(&mut cm, &ActionArgs::default());
}

/// The editor's copy (cut) ran: `<C-c>` on Windows/Linux leaves visual or
/// insert mode after the browser copied (`copy` handler).
pub(crate) fn after_copy(view: &mut EditorView, window: &mut Window, cx: &mut Context<EditorView>) {
    let Some(v) = view.vim.as_mut() else { return };
    if !v.wait_for_copy {
        return;
    }
    v.wait_for_copy = false;
    let mut cm = Cm::new(view, window, cx);
    if cm.vim().insert_mode {
        let c = cm.get_cursor();
        cm.set_selection(c, c);
    } else {
        cm.op_begin();
        cm.set_vim_op();
        handle_key(&mut cm, "<Esc>", "user");
        cm.op_end();
    }
}

/// `multiSelectHandleKey`.
pub(crate) fn multi_select_handle_key(cm: &mut Cm, key: &str, origin: &str) -> bool {
    let mut is_handled = false;
    let visual_block = cm.vim().visual_block || cm.vim().was_in_visual_block;
    if cm.view.vim.as_ref().is_some_and(|v| {
        matches!(
            v.dialog,
            Some(prompt::Dialog::Notification { long: true, .. })
        )
    }) {
        prompt::close_notification(cm);
        if key == "<CR>" {
            clear_input_state(cm);
            return true;
        }
    }
    let was_multi = cm.in_multi_select();
    if cm.vim().was_in_visual_block && !was_multi {
        cm.vim().was_in_visual_block = false;
    } else if was_multi && cm.vim().visual_block {
        cm.vim().was_in_visual_block = true;
    }
    let status_is_esc = cm.vim().status == "<Esc>";
    if key == "<Esc>"
        && !cm.vim().insert_mode
        && !cm.vim().visual_mode
        && was_multi
        && status_is_esc
    {
        clear_input_state(cm);
    } else if visual_block || !was_multi || cm.st().virtual_selection.is_some() {
        is_handled = handle_key(cm, key, origin);
    } else {
        let old = clone_vim_state(cm.vim_ref());
        let mut queues = cm.vim().input_state.change_queue_list.clone();
        cm.op_begin();
        cm.set_vim_op();
        let sel = cm.view.state.selection.clone();
        cm.st().virtual_selection = Some(sel.clone());
        let count = sel.ranges.len();
        let mut first: Option<VimState> = None;
        for i in 0..count {
            let Some(range) = cm
                .st()
                .virtual_selection
                .as_ref()
                .and_then(|v| v.ranges.get(i).copied())
            else {
                continue;
            };
            cm.view.dispatch(
                crate::state::TransactionSpec::default()
                    .selection(crate::selection::EditorSelection::create(vec![range], 0)),
                cm.window,
                cm.cx,
            );
            if queues.len() <= i {
                queues.resize(i + 1, None);
            }
            cm.vim().input_state.change_queue = queues[i].clone();
            let mut head = cm.get_cursor();
            let mut anchor = cm.get_anchor();
            let head_off = if !before(head, anchor) { -1 } else { 0 };
            let anchor_off = if before(head, anchor) { -1 } else { 0 };
            head = offset(head, 0, head_off);
            anchor = offset(anchor, 0, anchor_off);
            cm.vim().sel = Range { anchor, head };
            is_handled = handle_key(cm, key, origin);
            if cm.st().virtual_selection.is_some() {
                queues[i] = cm.vim().input_state.change_queue.clone();
                if i == 0 {
                    first = Some(clone_vim_state(cm.vim_ref()));
                }
                let restored = clone_vim_state(&old);
                *cm.vim() = restored;
            }
            let main = cm.view.state.selection.main();
            if let Some(vs) = cm.st().virtual_selection.as_mut()
                && i < vs.ranges.len()
            {
                vs.ranges[i] = main;
            }
        }
        if let Some(vs) = cm.st().virtual_selection.take() {
            cm.view.dispatch(
                crate::state::TransactionSpec::default().selection(vs),
                cm.window,
                cm.cx,
            );
        }
        if !is_handled && let Some(op) = cm.st().cur_op.as_mut() {
            op.cursor_activity = false;
        }
        if let Some(f) = first {
            *cm.vim() = f;
        }
        cm.vim().input_state.change_queue_list = queues;
        cm.vim().input_state.change_queue = None;
        cm.op_end();
    }
    if is_handled
        && !cm.vim().visual_mode
        && !cm.vim().insert_mode
        && cm.vim().visual_mode != cm.something_selected()
    {
        insert::handle_external_selection(cm);
    }
    is_handled
}

/// `cloneVimState` (without `insertEnd`).
fn clone_vim_state(s: &VimState) -> VimState {
    VimState {
        input_state: s.input_state.clone(),
        last_edit_input_state: s.last_edit_input_state.clone(),
        last_edit_action_command: s.last_edit_action_command.clone(),
        last_h_pos: s.last_h_pos,
        last_hs_pos: s.last_hs_pos,
        last_motion: s.last_motion,
        marks: s.marks.clone(),
        insert_mode: s.insert_mode,
        insert_mode_return: s.insert_mode_return,
        insert_mode_repeat: s.insert_mode_repeat,
        visual_mode: s.visual_mode,
        visual_line: s.visual_line,
        visual_block: s.visual_block,
        last_selection: s.last_selection,
        last_pasted_text: s.last_pasted_text.clone(),
        sel: s.sel,
        options: s.options.clone(),
        expect_literal_next: s.expect_literal_next,
        status: s.status.clone(),
        mode: s.mode.clone(),
        insert_end: None,
        search_state: s.search_state.clone(),
        ex_mode: s.ex_mode,
        was_in_visual_block: s.was_in_visual_block,
        one_normal_handler: s.one_normal_handler,
    }
}

/// What `findKey` found.
enum Found {
    /// Not handled.
    None,
    /// Handled without running anything (buffered key, swallowed char).
    Noop,
    Command(Box<VimKey>),
}

/// `handleKey`.
pub(crate) fn handle_key(cm: &mut Cm, key: &str, origin: &str) -> bool {
    match find_key(cm, key, origin) {
        Found::None => false,
        Found::Noop => true,
        Found::Command(command) => {
            cm.op_begin();
            cm.set_vim_op();
            match &command.kind {
                Kind::KeyToKey { to_keys } => {
                    let to = to_keys.clone();
                    do_key_to_key(cm, &to, Some(&*command));
                }
                _ => dispatch::process_command(cm, &command),
            }
            cm.op_end();
            true
        }
    }
}

/// `findKey`.
fn find_key(cm: &mut Cm, key: &str, origin: &str) -> Found {
    let command = if cm.vim().insert_mode {
        handle_key_insert_mode(cm, key)
    } else {
        handle_key_non_insert_mode(cm, key, origin)
    };
    match command {
        KeyResult::False => {
            let single = key.chars().count() == 1;
            let mac_alt = cm.view.platform == crate::keymap::Platform::Mac
                && key.starts_with("<A-")
                && key.chars().count() == 5;
            if !cm.vim().insert_mode && (single || mac_alt) {
                Found::Noop
            } else {
                Found::None
            }
        }
        KeyResult::True => Found::Noop,
        KeyResult::Command(c) => Found::Command(c),
        KeyResult::Undefined => Found::None,
    }
}

enum KeyResult {
    False,
    True,
    Undefined,
    Command(Box<VimKey>),
}

fn handle_esc(cm: &mut Cm, key: &str) -> bool {
    if key != "<Esc>" {
        return false;
    }
    if cm.vim().visual_mode {
        helpers::exit_visual_mode(cm, true);
    } else if cm.vim().insert_mode {
        insert::exit_insert_mode(cm, false);
    } else {
        return false;
    }
    clear_input_state(cm);
    true
}

fn handle_key_insert_mode(cm: &mut Cm, key: &str) -> KeyResult {
    if handle_esc(cm, key) {
        return KeyResult::True;
    }
    cm.vim().input_state.key_buffer.push(key.to_string());
    let keys = cm.vim().input_state.key_buffer.join("");
    let keys_are_chars = key.chars().count() == 1;
    let operator_pending = cm.vim().input_state.operator.is_some();
    let (m, character) = with_global(|g| {
        keymap::match_command(
            &keys,
            &g.keymap,
            keymap::Ctx::Insert,
            operator_pending,
            g.noremap,
            g.default_keymap_len,
        )
    });
    if let Some(c) = character {
        cm.vim().input_state.selected_character = Some(c);
    }
    let mut change_queue = cm.vim().input_state.change_queue.clone();
    match m {
        Match::None => {
            clear_input_state(cm);
            return KeyResult::False;
        }
        Match::Partial {
            expect_literal_next,
        } => {
            if expect_literal_next {
                cm.vim().expect_literal_next = true;
            }
            insert::arm_insert_key_timer(cm, keys_are_chars);
            if keys_are_chars {
                let selections = cm.list_selections();
                if change_queue
                    .as_ref()
                    .is_none_or(|q| q.removed.len() != selections.len())
                {
                    change_queue = Some(state::ChangeQueue::default());
                }
                let mut q = change_queue.unwrap_or_default();
                q.inserted.push_str(key);
                for (i, s) in selections.iter().enumerate() {
                    let from = pmin(s.anchor, s.head);
                    let to = pmax(s.anchor, s.head);
                    let to = if cm.overwrite() { offset(to, 0, 1) } else { to };
                    let text = cm.get_range(from, to);
                    if q.removed.len() <= i {
                        q.removed.resize(i + 1, String::new());
                    }
                    q.removed[i].push_str(&text);
                }
                cm.vim().input_state.change_queue = Some(q);
            }
            return if keys_are_chars {
                KeyResult::False
            } else {
                KeyResult::True
            };
        }
        Match::Full(_) => {
            cm.vim().input_state.key_buffer.clear();
        }
        Match::Clear => {}
    }
    cm.vim().expect_literal_next = false;
    if let Some(v) = cm.view.vim.as_mut() {
        v.insert_key_timer = None;
    }
    let command = match m {
        Match::Full(c) => Some(c),
        _ => None,
    };
    if let (Some(_), Some(q)) = (command.as_ref(), change_queue.as_ref()) {
        let selections = cm.list_selections();
        let inserted = q.inserted.chars().count() as i64;
        for (i, s) in selections.iter().enumerate() {
            let here = s.head;
            let removed = q.removed.get(i).cloned().unwrap_or_default();
            cm.replace_range(&removed, offset(here, 0, -inserted), Some(here));
        }
        with_global(|g| {
            g.macro_mode_state.last_insert_mode_changes.changes.pop();
        });
    }
    match command {
        Some(c) => KeyResult::Command(c),
        None => {
            clear_input_state(cm);
            KeyResult::Undefined
        }
    }
}

fn handle_key_non_insert_mode(cm: &mut Cm, key: &str, origin: &str) -> KeyResult {
    if insert::handle_macro_recording(cm, key, origin) || handle_esc(cm, key) {
        return KeyResult::True;
    }
    cm.vim().input_state.key_buffer.push(key.to_string());
    let keys = cm.vim().input_state.key_buffer.join("");
    let digits = keys.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == keys.chars().count() && keys.starts_with(|c: char| ('1'..='9').contains(&c)) {
        return KeyResult::True;
    }
    let (count, rest) = keys.split_at(
        keys.char_indices()
            .nth(digits)
            .map_or(keys.len(), |(i, _)| i),
    );
    let context = if cm.vim().visual_mode {
        keymap::Ctx::Visual
    } else {
        keymap::Ctx::Normal
    };
    let mut main_key = if rest.is_empty() {
        count.to_string()
    } else {
        rest.to_string()
    };
    if let Some(short) = cm.vim().input_state.operator_shortcut.clone()
        && short.chars().last().map(|c| c.to_string()) == Some(main_key.clone())
    {
        main_key = short;
    }
    let operator_pending = cm.vim().input_state.operator.is_some();
    let (m, character) = with_global(|g| {
        keymap::match_command(
            &main_key,
            &g.keymap,
            context,
            operator_pending,
            g.noremap,
            g.default_keymap_len,
        )
    });
    if let Some(c) = character {
        cm.vim().input_state.selected_character = Some(c);
    }
    match m {
        Match::None => {
            clear_input_state(cm);
            KeyResult::False
        }
        Match::Partial {
            expect_literal_next,
        } => {
            if expect_literal_next {
                cm.vim().expect_literal_next = true;
            }
            KeyResult::True
        }
        Match::Clear => {
            clear_input_state(cm);
            KeyResult::True
        }
        Match::Full(command) => {
            cm.vim().expect_literal_next = false;
            cm.vim().input_state.key_buffer.clear();
            if !count.is_empty() && count != "0" {
                cm.vim().input_state.push_repeat_digit(count);
            }
            KeyResult::Command(command)
        }
    }
}

/// The key at char index `i` of `keys` (`/<(?:[CSMA]-)*\w+>|./gi`) and
/// its length in chars.
fn key_at(keys: &[char], i: usize) -> (String, usize) {
    let parts = keymap::split_keys(&keys[i..].iter().collect::<String>());
    let first = parts.into_iter().next().unwrap_or_default();
    let len = first.chars().count().max(1);
    (first, len)
}

/// `doKeyToKey`: run a key sequence as if typed (mappings, `:normal`).
pub(crate) fn do_key_to_key(cm: &mut Cm, keys: &str, from_key: Option<&VimKey>) {
    let noremap_before = with_global(|g| g.noremap);
    if let Some(k) = from_key {
        let id = format!("{}\u{0}{:?}", k.keys, k.kind);
        // An ad-hoc `{noremap}` (`:normal`) is a fresh object every call.
        let adhoc = k.keys.is_empty() && k.kind == Kind::Idle;
        if !adhoc && with_global(|g| g.key_to_key_stack.contains(&id)) {
            return;
        }
        with_global(|g| {
            g.key_to_key_stack.push(id);
            g.noremap = k.noremap != Some(false);
        });
    }
    let chars: Vec<char> = keys.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        let (mut key, len) = key_at(&chars, i);
        i += len;
        let was_insert = cm.vim().insert_mode;
        if with_global(|g| g.virtual_prompt.is_some()) {
            prompt::send_key_to_prompt(cm, &key);
            continue;
        }
        let result = handle_key(cm, &key, "mapping");
        if !result && was_insert && cm.vim().insert_mode {
            if key.starts_with('<') {
                let lower = key.to_lowercase();
                let inner = if lower.len() >= 2 {
                    &lower[1..lower.len() - 1]
                } else {
                    ""
                };
                let last = inner.rsplit('-').next().unwrap_or("").to_string();
                if last == "lt" {
                    key = "<".into();
                } else if last == "space" {
                    key = " ".into();
                } else if last == "cr" {
                    key = "\n".into();
                } else if let Some(cmkey) = vim_to_cm_key(&last) {
                    insert::send_cm_key(cm, cmkey);
                    continue;
                } else {
                    key = "<".into();
                    i = start + 1;
                }
            }
            cm.replace_selection(&key);
        }
    }
    if from_key.is_some() {
        with_global(|g| {
            g.key_to_key_stack.pop();
        });
    }
    let pending = with_global(|g| {
        g.noremap = if g.key_to_key_stack.is_empty() {
            false
        } else {
            noremap_before
        };
        if g.key_to_key_stack.is_empty() {
            g.virtual_prompt.take()
        } else {
            None
        }
    });
    if let Some(p) = pending {
        prompt::show_prompt(cm, p);
    }
}

/// `vimToCmKeyMap`.
pub(crate) fn vim_to_cm_key(lower: &str) -> Option<&'static str> {
    Some(match lower {
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        "end" => "End",
        "home" => "Home",
        "cr" | "return" => "Return",
        "bs" | "backspace" => "Backspace",
        "del" | "delete" => "Delete",
        "esc" | "escape" => "Escape",
        "ins" | "insert" => "Insert",
        "arrowleft" => "ArrowLeft",
        "arrowright" => "ArrowRight",
        "arrowup" => "ArrowUp",
        "arrowdown" => "ArrowDown",
        "enter" => "Enter",
        "space" | " " => " ",
        _ => return None,
    })
}

/// Install vim on a view (`enterVimMode`).
pub(crate) fn enter(view: &mut EditorView) {
    if view.vim.is_some() {
        return;
    }
    view.vim = Some(Vim::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn langmaps() {
        let l = parse_langmap("aA,bc;BC");
        assert_eq!(l.keymap.get("a").map(String::as_str), Some("A"));
        assert_eq!(l.keymap.get("b").map(String::as_str), Some("B"));
        assert_eq!(l.keymap.get("c").map(String::as_str), Some("C"));
    }
}
