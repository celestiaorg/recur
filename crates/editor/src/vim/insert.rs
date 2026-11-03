//! Insert-mode bookkeeping: recording changes for `.` and macros
//! (`onChange`, `onCursorActivity`, `onKeyEventTargetKeyDown`), leaving
//! insert mode, repeating edits and replaying macros.

use std::time::Duration;

use super::cm::Cm;
use super::helpers::*;
use super::state::{InsertChange, InsertModeKey, with_global};
use super::types::*;
use super::{clear_input_state, mode_change};

/// `onChange` for one operation's changes (only while recording).
pub(crate) fn on_change(cm: &mut Cm, changes: &[String]) {
    if with_global(|g| g.macro_mode_state.is_playing) {
        return;
    }
    let overwrite = cm.overwrite();
    let selection_count = cm.list_selections().len() as i64;
    let insert_end = cm.vim().insert_end.and_then(|id| cm.mark_find(id));
    let cursor = cm.get_cursor();
    with_global(|g| {
        let last = &mut g.macro_mode_state.last_insert_mode_changes;
        for text in changes {
            last.expect_cursor_activity_for_change = true;
            if last.ignore_count > 1 {
                last.ignore_count -= 1;
                continue;
            }
            if selection_count > 1 {
                last.ignore_count = selection_count;
            }
            if last.maybe_reset {
                last.changes.clear();
                last.maybe_reset = false;
            }
            if text.is_empty() {
                continue;
            }
            if overwrite && !text.contains('\n') {
                last.changes.push(InsertChange::Overwrite(text.clone()));
                continue;
            }
            let mut text = text.clone();
            if text.chars().count() > 1
                && let Some(end) = insert_end
                && end.line == cursor.line
            {
                let off = end.ch - cursor.ch;
                if off > 0 && off < text.chars().count() as i64 {
                    last.changes.push(InsertChange::Offset(text.clone(), off));
                    text.clear();
                }
            }
            if !text.is_empty() {
                last.changes.push(InsertChange::Text(text));
            }
        }
    });
}

/// `onCursorActivity`.
pub(crate) fn on_cursor_activity(cm: &mut Cm, is_vim_op: bool) {
    if cm.vim().insert_mode {
        if with_global(|g| g.macro_mode_state.is_playing) {
            return;
        }
        let expected = with_global(|g| {
            let last = &mut g.macro_mode_state.last_insert_mode_changes;
            if last.expect_cursor_activity_for_change {
                last.expect_cursor_activity_for_change = false;
                true
            } else {
                last.maybe_reset = true;
                false
            }
        });
        if !expected {
            if let Some(old) = cm.vim().insert_end.take() {
                cm.mark_clear(old);
            }
            let c = cm.get_cursor();
            let id = cm.set_bookmark(c, true);
            cm.vim().insert_end = Some(id);
        }
    } else if !is_vim_op {
        handle_external_selection(cm);
    }
}

/// `handleExternalSelection`: follow a selection made outside vim.
pub(crate) fn handle_external_selection(cm: &mut Cm) {
    let mut anchor = cm.get_anchor();
    let mut head = cm.get_cursor();
    if cm.vim().visual_mode && !cm.something_selected() {
        exit_visual_mode(cm, false);
    } else if !cm.vim().visual_mode && !cm.vim().insert_mode && cm.something_selected() {
        cm.vim().visual_mode = true;
        cm.vim().visual_line = false;
        mode_change(cm, "visual", None);
    }
    if cm.vim().visual_mode {
        let head_off = if !before(head, anchor) { -1 } else { 0 };
        let anchor_off = if before(head, anchor) { -1 } else { 0 };
        head = offset(head, 0, head_off);
        anchor = offset(anchor, 0, anchor_off);
        cm.vim().sel = Range { anchor, head };
        update_mark(cm, "<", pmin(head, anchor));
        update_mark(cm, ">", pmax(head, anchor));
    } else if !cm.vim().insert_mode {
        cm.vim().last_h_pos = cm.get_cursor().ch;
    }
}

/// `onKeyEventTargetKeyDown`: record Backspace / Delete in insert mode.
pub(crate) fn on_insert_key_down(key_name: &str) {
    if !(key_name.contains("Delete") || key_name.contains("Backspace")) {
        return;
    }
    with_global(|g| {
        let last = &mut g.macro_mode_state.last_insert_mode_changes;
        if last.maybe_reset {
            last.changes.clear();
            last.maybe_reset = false;
        }
        last.changes.push(InsertChange::Key(InsertModeKey {
            key_name: key_name.to_string(),
        }));
    });
}

/// `exitInsertMode`.
pub(crate) fn exit_insert_mode(cm: &mut Cm, keep_cursor: bool) {
    let is_playing = with_global(|g| g.macro_mode_state.is_playing);
    if !is_playing {
        cm.st().listening_changes = false;
        if let Some(old) = cm.vim().insert_end.take() {
            cm.mark_clear(old);
        }
    }
    let repeat = cm.vim().insert_mode_repeat;
    if !is_playing && repeat.is_some_and(|r| r > 1) {
        let r = repeat.unwrap_or(1);
        repeat_last_edit(cm, r - 1, true);
        if let Some(s) = cm.vim().last_edit_input_state.as_mut() {
            s.repeat_override = Some(r);
        }
    }
    cm.vim().insert_mode_repeat = None;
    cm.vim().insert_mode = false;
    if !keep_cursor {
        let c = cm.get_cursor();
        cm.set_cursor(Pos::new(c.line, c.ch - 1));
    }
    cm.toggle_overwrite(false);
    with_global(|g| {
        let text: String = g
            .macro_mode_state
            .last_insert_mode_changes
            .changes
            .iter()
            .map(|c| match c {
                InsertChange::Text(t) => t.clone(),
                InsertChange::Overwrite(t) => t.clone(),
                InsertChange::Offset(t, o) => format!("{t},{o}"),
                InsertChange::Key(_) => "[object Object]".to_string(),
            })
            .collect();
        g.registers.get(Some(".")).set_text(&text, false, false);
    });
    mode_change(cm, "normal", None);
    if with_global(|g| g.macro_mode_state.is_recording) {
        log_insert_mode_change();
    }
}

/// `repeatLastEdit`.
pub(crate) fn repeat_last_edit(cm: &mut Cm, repeat: i64, repeat_for_insert: bool) {
    with_global(|g| g.macro_mode_state.is_playing = true);
    let last_action = cm.vim().last_edit_action_command.clone();
    let cached = cm.vim().input_state.clone();
    let repeat_command = |cm: &mut Cm| {
        if let Some(a) = last_action.clone() {
            super::dispatch::process_action(cm, &a);
        } else {
            super::dispatch::eval_input(cm);
        }
    };
    let repeat_insert = |cm: &mut Cm, repeat: i64| {
        let changes = with_global(|g| g.macro_mode_state.last_insert_mode_changes.changes.clone());
        if !changes.is_empty() {
            let r = if cm.vim().last_edit_action_command.is_none() {
                1
            } else {
                repeat
            };
            repeat_insert_mode_changes(cm, &changes, r);
        }
    };
    if let Some(s) = cm.vim().last_edit_input_state.clone() {
        cm.vim().input_state = s;
    }
    if last_action
        .as_ref()
        .is_some_and(|a| a.interlace_insert_repeat)
    {
        for _ in 0..repeat {
            repeat_command(cm);
            repeat_insert(cm, 1);
        }
    } else {
        if !repeat_for_insert {
            repeat_command(cm);
        }
        repeat_insert(cm, repeat);
    }
    cm.vim().input_state = cached;
    if cm.vim().insert_mode && !repeat_for_insert {
        exit_insert_mode(cm, false);
    }
    with_global(|g| g.macro_mode_state.is_playing = false);
}

/// `sendCmKey`: run a key through the editor keymap.
pub(crate) fn send_cm_key(cm: &mut Cm, key: &str) {
    let name = match key {
        "Left" | "ArrowLeft" => "ArrowLeft",
        "Right" | "ArrowRight" => "ArrowRight",
        "Up" | "ArrowUp" => "ArrowUp",
        "Down" | "ArrowDown" => "ArrowDown",
        "Backspace" => "Backspace",
        "Delete" => "Delete",
        _ => return,
    };
    let ev = crate::keymap::KeyEvent {
        name: name.into(),
        base: Some(name.into()),
        shifted: Some(name.into()),
        alt: false,
        ctrl: false,
        meta: false,
        shift: false,
    };
    cm.view.run_keymap(&ev, "editor", cm.window, cm.cx);
}

/// `repeatInsertModeChanges`.
pub(crate) fn repeat_insert_mode_changes(cm: &mut Cm, changes: &[InsertChange], mut repeat: i64) {
    let head = cm.get_cursor();
    let visual_block = with_global(|g| g.macro_mode_state.last_insert_mode_changes.visual_block);
    if visual_block != 0 {
        select_for_insert(cm, head, visual_block + 1);
        repeat = cm.list_selections().len() as i64;
        cm.set_cursor(head);
    }
    for i in 0..repeat {
        if visual_block != 0 {
            cm.set_cursor(offset(head, i, 0));
        }
        for change in changes {
            match change {
                InsertChange::Key(k) => send_cm_key(cm, &k.key_name),
                InsertChange::Text(t) => cm.replace_selection(t),
                InsertChange::Overwrite(t) => {
                    let start = cm.get_cursor();
                    let end = offset(start, 0, t.chars().count() as i64);
                    cm.replace_range(t, start, Some(end));
                    cm.set_cursor(end);
                }
                InsertChange::Offset(t, o) => {
                    let start = cm.get_cursor();
                    let end = offset(start, 0, t.chars().count() as i64 - o);
                    cm.replace_range(t, start, Some(start));
                    cm.set_cursor(end);
                }
            }
        }
    }
    if visual_block != 0 {
        cm.set_cursor(offset(head, 0, 1));
    }
}

/// The macro-recording half of `findKey` (`handleMacroRecording`).
pub(crate) fn handle_macro_recording(cm: &mut Cm, key: &str, origin: &str) -> bool {
    if !with_global(|g| g.macro_mode_state.is_recording) {
        return false;
    }
    if key == "q" {
        exit_macro_record_mode(cm);
        clear_input_state(cm);
        return true;
    }
    if origin != "mapping" {
        log_key(key);
    }
    false
}

/// `MacroModeState.enterMacroRecordMode`.
pub(crate) fn enter_macro_record_mode(cm: &mut Cm, name: &str) {
    let ok = with_global(|g| {
        if !g.registers.is_valid(Some(name)) {
            return false;
        }
        g.registers.get(Some(name)).clear();
        g.macro_mode_state.latest_register = Some(name.to_string());
        g.macro_mode_state.is_recording = true;
        g.macro_mode_state.recording_dialog = true;
        true
    });
    if ok {
        super::prompt::open_message(cm, &format!("recording @{name}"));
    }
}

/// `MacroModeState.exitMacroRecordMode`.
pub(crate) fn exit_macro_record_mode(cm: &mut Cm) {
    let had = with_global(|g| {
        let had = g.macro_mode_state.recording_dialog;
        g.macro_mode_state.recording_dialog = false;
        g.macro_mode_state.is_recording = false;
        had
    });
    if had {
        super::prompt::close_message(cm);
    }
}

fn log_key(key: &str) {
    with_global(|g| {
        if g.macro_mode_state.is_playing {
            return;
        }
        let name = g.macro_mode_state.latest_register.clone();
        g.registers.get(name.as_deref()).push_text(key, false);
    });
}

fn log_insert_mode_change() {
    with_global(|g| {
        if g.macro_mode_state.is_playing {
            return;
        }
        let name = g.macro_mode_state.latest_register.clone();
        let changes = g.macro_mode_state.last_insert_mode_changes.clone();
        g.registers.get(name.as_deref()).insert_mode_changes.push(
            super::state::InsertModeChanges {
                changes: changes.changes,
                expect_cursor_activity_for_change: changes.expect_cursor_activity_for_change,
                ..Default::default()
            },
        );
    });
}

/// `logSearchQuery`.
pub(crate) fn log_search_query(query: &str) {
    with_global(|g| {
        if g.macro_mode_state.is_playing {
            return;
        }
        let name = g.macro_mode_state.latest_register.clone();
        g.registers
            .get(name.as_deref())
            .search_queries
            .push(query.to_string());
    });
}

/// `executeMacroRegister`.
pub(crate) fn execute_macro_register(cm: &mut Cm, name: &str) {
    let register = with_global(|g| g.registers.get(Some(name)).clone());
    if name == ":" {
        // A command that throws leaves `isPlaying` set, as upstream does.
        if let Some(cmd) = register
            .key_buffer
            .first()
            .cloned()
            .filter(|c| !c.is_empty())
            && super::ex::process_command(cm, &cmd).is_err()
        {
            return;
        }
        with_global(|g| g.macro_mode_state.is_playing = false);
        return;
    }
    let mut imc = 0;
    with_global(|g| {
        g.macro_mode_state.is_playing = true;
        g.macro_mode_state.replay_search_queries = register.search_queries.clone();
    });
    for text in &register.key_buffer {
        for key in super::keymap::split_keys(text) {
            super::handle_key(cm, &key, "macro");
            if cm.vim().insert_mode {
                let changes = register
                    .insert_mode_changes
                    .get(imc)
                    .map(|c| c.changes.clone())
                    .unwrap_or_default();
                imc += 1;
                with_global(|g| {
                    g.macro_mode_state.last_insert_mode_changes.changes = changes.clone()
                });
                repeat_insert_mode_changes(cm, &changes, 1);
                exit_insert_mode(cm, false);
            }
        }
    }
    with_global(|g| g.macro_mode_state.is_playing = false);
}

/// The two-key insert-mode mapping timeout (`lastInsertModeKeyTimer`).
pub(crate) fn arm_insert_key_timer(cm: &mut Cm, keys_are_chars: bool) {
    let timeout = with_global(|g| g.esc_keys_timeout());
    let Some(v) = cm.view.vim.as_mut() else {
        return;
    };
    v.insert_key_timer = None;
    if !keys_are_chars {
        return;
    }
    let task = cm.cx.spawn_in(cm.window, async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(timeout))
            .await;
        this.update_in(cx, |view, window, cx| {
            let pending = view
                .vim
                .as_ref()
                .is_some_and(|v| v.state.insert_mode && !v.state.input_state.key_buffer.is_empty());
            if pending {
                let mut cm = Cm::new(view, window, cx);
                clear_input_state(&mut cm);
            }
        })
        .ok();
    });
    if let Some(v) = cm.view.vim.as_mut() {
        v.insert_key_timer = Some(task);
    }
}
