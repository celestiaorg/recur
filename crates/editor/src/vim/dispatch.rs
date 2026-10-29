//! `commandDispatcher`: runs a matched command, building the input state
//! (operator, motion, repeat) and evaluating it (`evalInput`).

use super::cm::Cm;
use super::helpers::*;
use super::keymap::{Kind, VimKey};
use super::state::{InputState, with_global};
use super::types::*;
use super::{clear_input_state, mode_change};

/// `processCommand`.
pub(crate) fn process_command(cm: &mut Cm, command: &VimKey) {
    cm.vim().input_state.repeat_override = command.repeat_override;
    match &command.kind {
        Kind::Motion { motion, args } => process_motion(cm, *motion, args),
        Kind::Operator { .. } => process_operator(cm, command),
        Kind::OperatorMotion { .. } => process_operator_motion(cm, command),
        Kind::Action { .. } => process_action(cm, command),
        Kind::Search { args } => super::search::process_search(cm, *args),
        Kind::Ex => super::ex::process_ex(cm),
        Kind::KeyToKey { .. } | Kind::Idle => {}
    }
}

/// `processMotion`.
pub(crate) fn process_motion(cm: &mut Cm, motion: Motion, args: &MotionArgs) {
    let s = &mut cm.vim().input_state;
    s.motion = Some(motion);
    s.motion_args = Some(args.clone());
    eval_input(cm);
}

fn operator_of(command: &VimKey) -> Option<(Operator, OperatorArgs)> {
    match &command.kind {
        Kind::Operator { operator, args } => Some((*operator, args.clone())),
        Kind::OperatorMotion {
            operator,
            operator_args,
            ..
        } => Some((*operator, operator_args.clone())),
        _ => None,
    }
}

/// `processOperator`.
pub(crate) fn process_operator(cm: &mut Cm, command: &VimKey) {
    let Some((operator, args)) = operator_of(command) else {
        return;
    };
    if let Some(current) = cm.vim().input_state.operator {
        if current == operator {
            let s = &mut cm.vim().input_state;
            s.motion = Some(Motion::ExpandToLine);
            s.motion_args = Some(MotionArgs {
                linewise: true,
                repeat: 1,
                ..Default::default()
            });
            eval_input(cm);
            return;
        }
        clear_input_state(cm);
    }
    let s = &mut cm.vim().input_state;
    s.operator = Some(operator);
    s.operator_args = Some(args);
    if command.keys.chars().count() > 1 {
        s.operator_shortcut = Some(command.keys.clone());
    }
    if command.exit_visual_block {
        cm.vim().visual_block = false;
        update_cm_selection(cm, None, None);
    }
    if cm.vim().visual_mode {
        eval_input(cm);
    }
}

/// `processOperatorMotion`.
fn process_operator_motion(cm: &mut Cm, command: &VimKey) {
    let visual_mode = cm.vim().visual_mode;
    let Kind::OperatorMotion {
        motion,
        motion_args,
        visual_line,
        ..
    } = &command.kind
    else {
        return;
    };
    if visual_mode && *visual_line == Some(true) {
        cm.vim().visual_line = true;
    }
    process_operator(cm, command);
    if !visual_mode {
        process_motion(cm, *motion, motion_args);
    }
}

/// `processAction`.
pub(crate) fn process_action(cm: &mut Cm, command: &VimKey) {
    let Kind::Action {
        action,
        args,
        motion,
        motion_args,
    } = &command.kind
    else {
        return;
    };
    let input_state = cm.vim().input_state.clone();
    let repeat = input_state.get_repeat();
    let repeat_is_explicit = repeat != 0;
    let mut action_args = args.clone().unwrap_or(ActionArgs {
        repeat: 1,
        ..Default::default()
    });
    if let Some(c) = input_state.selected_character.clone() {
        action_args.selected_character = Some(c);
    }
    if let Some(m) = motion {
        process_motion(cm, *m, motion_args);
    }
    if motion.is_some() {
        eval_input(cm);
    }
    action_args.repeat = if repeat != 0 { repeat } else { 1 };
    action_args.repeat_is_explicit = repeat_is_explicit;
    action_args.register_name = input_state.register_name.clone();
    clear_input_state(cm);
    cm.vim().last_motion = None;
    if command.is_edit {
        record_last_edit(cm, input_state, Some(command.clone()));
    }
    super::actions::run(cm, *action, &action_args);
}

/// `evalInput`.
pub(crate) fn eval_input(cm: &mut Cm) {
    let input_state = cm.vim().input_state.clone();
    let motion = input_state.motion;
    let mut motion_args = input_state.motion_args.clone().unwrap_or(MotionArgs {
        repeat: 1,
        ..Default::default()
    });
    let operator = input_state.operator;
    let mut operator_args = input_state.operator_args.clone().unwrap_or_default();
    let register_name = input_state.register_name.clone();
    let visual = cm.vim().visual_mode;
    let sel = cm.vim().sel;
    let orig_head = if visual {
        clip_cursor_to_content(cm, sel.head, None)
    } else {
        cm.get_cursor()
    };
    let orig_anchor = if visual {
        clip_cursor_to_content(cm, sel.anchor, None)
    } else {
        cm.get_anchor()
    };
    let old_head = orig_head;
    let old_anchor = orig_anchor;
    let mut new_head: Option<Pos> = None;
    let mut new_anchor: Option<Pos> = None;
    if operator.is_some() {
        record_last_edit(cm, input_state.clone(), None);
    }
    let mut repeat = input_state
        .repeat_override
        .unwrap_or_else(|| input_state.get_repeat());
    if repeat > 0 && motion_args.explicit_repeat {
        motion_args.repeat_is_explicit = true;
    } else if motion_args.no_repeat || (!motion_args.explicit_repeat && repeat == 0) {
        repeat = 1;
        motion_args.repeat_is_explicit = false;
    }
    if let Some(c) = input_state.selected_character.clone() {
        motion_args.selected_character = Some(c.clone());
        operator_args.selected_character = Some(c);
    }
    motion_args.repeat = repeat;
    clear_input_state(cm);
    if let Some(motion) = motion {
        let result = super::motions::run(cm, motion, orig_head, &mut motion_args, &input_state);
        cm.vim().last_motion = Some(motion);
        let Some(result) = result else { return };
        if motion_args.to_jumplist {
            let cached = with_global(|g| g.jump_list.cached_cursor.take());
            let to = match result {
                MotionResult::Head(h) => h,
                MotionResult::Range(_, h) => h,
            };
            record_jump_position(cm, cached.unwrap_or(orig_head), to);
        }
        match result {
            MotionResult::Range(a, h) => {
                new_anchor = Some(a);
                new_head = Some(h);
            }
            MotionResult::Head(h) => new_head = Some(h),
        }
        let mut head = new_head.unwrap_or(orig_head);
        if cm.vim().visual_mode {
            if !(cm.vim().visual_block && head.ch >= INF) {
                head = clip_cursor_to_content(cm, head, Some(old_head));
            }
            let anchor = match new_anchor {
                Some(a) => clip_cursor_to_content(cm, a, None),
                None => old_anchor,
            };
            new_anchor = Some(anchor);
            cm.vim().sel = Range { anchor, head };
            update_cm_selection(cm, None, None);
            update_mark(cm, "<", if before(anchor, head) { anchor } else { head });
            update_mark(cm, ">", if before(anchor, head) { head } else { anchor });
        } else if operator.is_none() {
            head = clip_cursor_to_content(cm, head, Some(old_head));
            cm.set_cursor(head);
        }
        new_head = Some(head);
    }
    let Some(operator) = operator else { return };
    if let Some(last) = operator_args.last_sel {
        new_anchor = Some(old_anchor);
        let line_offset = (last.head.line - last.anchor.line).abs();
        let ch_offset = (last.head.ch - last.anchor.ch).abs();
        let head = if last.visual_line {
            Pos::new(old_anchor.line + line_offset, old_anchor.ch)
        } else if last.visual_block {
            Pos::new(old_anchor.line + line_offset, old_anchor.ch + ch_offset)
        } else if last.head.line == last.anchor.line {
            Pos::new(old_anchor.line, old_anchor.ch + ch_offset)
        } else {
            Pos::new(old_anchor.line + line_offset, old_anchor.ch)
        };
        new_head = Some(head);
        let vim = cm.vim();
        vim.visual_mode = true;
        vim.visual_line = last.visual_line;
        vim.visual_block = last.visual_block;
        vim.sel = Range {
            anchor: old_anchor,
            head,
        };
        update_cm_selection(cm, None, None);
    } else if cm.vim().visual_mode {
        let sel = cm.vim().sel;
        operator_args.last_sel = Some(LastSel {
            anchor: sel.anchor,
            head: sel.head,
            visual_block: cm.vim().visual_block,
            visual_line: cm.vim().visual_line,
        });
    }
    let linewise;
    let cm_sel;
    if cm.vim().visual_mode {
        let sel = cm.vim().sel;
        let cur_start = pmin(sel.head, sel.anchor);
        let cur_end = pmax(sel.head, sel.anchor);
        linewise = cm.vim().visual_line || operator_args.linewise;
        let mode = if cm.vim().visual_block {
            SelMode::Block
        } else if linewise {
            SelMode::Line
        } else {
            SelMode::Char
        };
        let mut s = make_cm_selection(
            cm,
            Range {
                anchor: cur_start,
                head: cur_end,
            },
            mode,
            false,
        );
        if linewise {
            if mode == SelMode::Block {
                for r in s.ranges.iter_mut() {
                    r.head.ch = cm.line_len(r.head.line);
                }
            } else if mode == SelMode::Line {
                s.ranges[0].head = Pos::new(s.ranges[0].head.line + 1, 0);
            }
        }
        cm_sel = s;
    } else {
        let mut cur_start = new_anchor.unwrap_or(old_anchor);
        let mut cur_end = new_head.unwrap_or(old_head);
        if before(cur_end, cur_start) {
            std::mem::swap(&mut cur_start, &mut cur_end);
        }
        linewise = motion_args.linewise || operator_args.linewise;
        if linewise {
            cur_start.ch = 0;
            cur_end.ch = 0;
            cur_end.line += 1;
        } else if motion_args.fwd() {
            clip_to_line(cm, cur_start, &mut cur_end);
        }
        let exclusive = !motion_args.inclusive || linewise;
        cm_sel = make_cm_selection(
            cm,
            Range {
                anchor: cur_start,
                head: cur_end,
            },
            SelMode::Char,
            exclusive,
        );
    }
    cm.set_selections(&cm_sel.ranges, cm_sel.primary);
    cm.vim().last_motion = None;
    operator_args.repeat = repeat;
    operator_args.register_name = register_name;
    operator_args.linewise = linewise;
    let move_to = super::operators::run(
        cm,
        operator,
        &operator_args,
        &cm_sel.ranges,
        old_anchor,
        new_head,
    );
    if cm.vim().visual_mode {
        exit_visual_mode(cm, move_to.is_some());
    }
    if let Some(p) = move_to {
        cm.set_cursor(p);
    }
}

/// `recordLastEdit`.
pub(crate) fn record_last_edit(cm: &mut Cm, input_state: InputState, action: Option<VimKey>) {
    if with_global(|g| g.macro_mode_state.is_playing) {
        return;
    }
    let vb = if cm.vim().visual_block {
        cm.vim().sel.head.line - cm.vim().sel.anchor.line
    } else {
        0
    };
    let vim = cm.vim();
    vim.last_edit_input_state = Some(input_state);
    vim.last_edit_action_command = action;
    with_global(|g| {
        let c = &mut g.macro_mode_state.last_insert_mode_changes;
        c.changes.clear();
        c.expect_cursor_activity_for_change = false;
        c.visual_block = vb;
    });
}

/// `CM.signal(cm, "vim-mode-change", {mode: "visual", subMode})`.
pub(crate) fn signal_visual(cm: &mut Cm) {
    let sub = if cm.vim().visual_line {
        "linewise"
    } else if cm.vim().visual_block {
        "blockwise"
    } else {
        ""
    };
    mode_change(cm, "visual", Some(sub));
}
