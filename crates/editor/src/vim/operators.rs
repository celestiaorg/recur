//! Vim operators (`operators`): they act on the selection `evalInput` set
//! and return where the cursor goes.

use super::cm::Cm;
use super::helpers::*;
use super::state::{is_upper, with_global};
use super::types::*;

fn first_non_ws_of(cm: &Cm, p: Pos) -> Pos {
    Pos::new(p.line, first_non_ws(&cm.get_line(p.line)))
}

fn push_text(
    cm: &mut Cm,
    name: Option<&str>,
    operator: &str,
    text: &str,
    linewise: bool,
    blockwise: bool,
) {
    let clip = with_global(|g| {
        g.registers
            .push_text(name, operator, text, linewise, blockwise)
    });
    if let Some(t) = clip {
        cm.cx
            .write_to_clipboard(gpui_kit::ClipboardItem::new_string(t));
    }
}

/// Run an operator.
pub(crate) fn run(
    cm: &mut Cm,
    op: Operator,
    args: &OperatorArgs,
    ranges: &[Range],
    old_anchor: Pos,
    new_head: Option<Pos>,
) -> Option<Pos> {
    match op {
        Operator::Change => {
            change(cm, args, ranges);
            None
        }
        Operator::Delete => Some(delete(cm, args, ranges)),
        Operator::Indent => indent(cm, args, ranges),
        Operator::IndentAuto => {
            cm.exec_command("indentAuto");
            Some(first_non_ws_of(cm, ranges[0].anchor))
        }
        Operator::HardWrap => {
            let from = ranges[0].anchor.line;
            let mut to = ranges[0].head.line;
            if args.linewise {
                to -= 1;
            }
            let mut end_row = cm.hard_wrap(from, to);
            if end_row > from && args.linewise {
                end_row -= 1;
            }
            Some(if args.keep_cursor {
                old_anchor
            } else {
                Pos::new(end_row, 0)
            })
        }
        Operator::ChangeCase => {
            let swapped: Vec<String> = cm
                .get_selections()
                .iter()
                .map(|t| match args.to_lower {
                    Some(true) => t.to_lowercase(),
                    Some(false) => t.to_uppercase(),
                    None => t
                        .chars()
                        .map(|c| {
                            let s = c.to_string();
                            if is_upper(&s) {
                                s.to_lowercase()
                            } else {
                                s.to_uppercase()
                            }
                        })
                        .collect(),
                })
                .collect();
            cm.replace_selections(&swapped);
            case_result(cm, args, ranges, old_anchor, new_head)
        }
        Operator::Yank => {
            let text = cm.get_selection();
            let end = if cm.vim().visual_mode {
                let sel = cm.vim().sel;
                pmin(
                    sel.anchor,
                    pmin(sel.head, pmin(ranges[0].head, ranges[0].anchor)),
                )
            } else {
                old_anchor
            };
            let vb = cm.vim().visual_block;
            push_text(
                cm,
                args.register_name.as_deref(),
                "yank",
                &text,
                args.linewise,
                vb,
            );
            Some(end)
        }
        Operator::Rot13 => {
            let swapped: Vec<String> = cm
                .get_selections()
                .iter()
                .map(|t| {
                    t.chars()
                        .map(|c| match c {
                            'A'..='Z' => (b'A' + ((c as u8 - b'A' + 13) % 26)) as char,
                            'a'..='z' => (b'a' + ((c as u8 - b'a' + 13) % 26)) as char,
                            _ => c,
                        })
                        .collect()
                })
                .collect();
            cm.replace_selections(&swapped);
            case_result(cm, args, ranges, old_anchor, new_head)
        }
    }
}

fn case_result(
    cm: &Cm,
    args: &OperatorArgs,
    ranges: &[Range],
    old_anchor: Pos,
    new_head: Option<Pos>,
) -> Option<Pos> {
    if args.should_move_cursor {
        new_head
    } else if !cm.vim_ref().visual_mode
        && args.linewise
        && ranges[0].anchor.line + 1 == ranges[0].head.line
    {
        Some(first_non_ws_of(cm, old_anchor))
    } else if args.linewise {
        Some(old_anchor)
    } else {
        Some(pmin(ranges[0].anchor, ranges[0].head))
    }
}

/// `change`.
fn change(cm: &mut Cm, args: &OperatorArgs, ranges: &[Range]) {
    let final_head;
    let text;
    let mut anchor = ranges[0].anchor;
    let mut head = ranges[0].head;
    if !cm.vim().visual_mode {
        let mut t = cm.get_range(anchor, head);
        let last = cm.vim().last_edit_input_state.clone();
        if last
            .as_ref()
            .is_some_and(|l| l.motion == Some(Motion::MoveByWords))
            && !is_ws_str(&t)
        {
            let trailing = t
                .chars()
                .rev()
                .take_while(|c| crate::state::is_js_space(*c))
                .count();
            if trailing > 0
                && last
                    .as_ref()
                    .and_then(|l| l.motion_args.as_ref())
                    .is_some_and(|a| a.fwd())
            {
                head = offset(head, 0, -(trailing as i64));
                let keep = t.chars().count() - trailing;
                t = t.chars().take(keep).collect();
            }
        }
        if args.linewise {
            anchor = Pos::new(anchor.line, first_non_ws(&cm.get_line(anchor.line)));
            if head.line > anchor.line {
                head = Pos::new(head.line - 1, INF);
            }
        }
        cm.replace_range("", anchor, Some(head));
        text = t;
        final_head = anchor;
    } else if args.full_line {
        head.ch = INF;
        head.line -= 1;
        cm.set_selection(anchor, head);
        text = cm.get_selection();
        cm.replace_selection("");
        final_head = anchor;
    } else {
        text = cm.get_selection();
        let empty = vec![String::new(); ranges.len()];
        cm.replace_selections(&empty);
        final_head = pmin(ranges[0].head, ranges[0].anchor);
    }
    push_text(
        cm,
        args.register_name.as_deref(),
        "change",
        &text,
        args.linewise,
        ranges.len() > 1,
    );
    super::actions::enter_insert_mode(
        cm,
        &ActionArgs {
            head: Some(final_head),
            ..Default::default()
        },
    );
}

/// `delete`.
fn delete(cm: &mut Cm, args: &OperatorArgs, ranges: &[Range]) -> Pos {
    let mut final_head;
    let text;
    let vb = cm.vim().visual_block;
    if !vb {
        let mut anchor = ranges[0].anchor;
        let head = ranges[0].head;
        if args.linewise
            && head.line != cm.first_line()
            && anchor.line == cm.last_line()
            && anchor.line == head.line - 1
        {
            if anchor.line == cm.first_line() {
                anchor.ch = 0;
            } else {
                anchor = Pos::new(anchor.line - 1, cm.line_len(anchor.line - 1));
            }
        }
        text = cm.get_range(anchor, head);
        cm.replace_range("", anchor, Some(head));
        final_head = anchor;
        if args.linewise {
            final_head = first_non_ws_of(cm, anchor);
        }
    } else {
        text = cm.get_selection();
        let empty = vec![String::new(); ranges.len()];
        cm.replace_selections(&empty);
        final_head = pmin(ranges[0].head, ranges[0].anchor);
    }
    push_text(
        cm,
        args.register_name.as_deref(),
        "delete",
        &text,
        args.linewise,
        vb,
    );
    clip_cursor_to_content(cm, final_head, None)
}

/// `indent`.
fn indent(cm: &mut Cm, args: &OperatorArgs, ranges: &[Range]) -> Option<Pos> {
    let repeat = if cm.vim().visual_mode {
        args.repeat.max(1)
    } else {
        1
    };
    if cm.vim().visual_block {
        let tab = cm.tab_size();
        let unit = if cm.indent_with_tabs() {
            "\t".to_string()
        } else {
            " ".repeat(tab as usize)
        };
        let mut cursor = Pos::default();
        for r in ranges.iter().rev() {
            cursor = pmin(r.anchor, r.head);
            if args.indent_right == Some(true) {
                cm.replace_range(&unit.repeat(repeat as usize), cursor, Some(cursor));
            } else {
                let text = cm.line_chars(cursor.line);
                let mut end: i64 = 0;
                for _ in 0..repeat {
                    let at = |i: i64| text.get((cursor.ch + i).max(0) as usize).copied();
                    match at(end) {
                        Some('\t') => end += 1,
                        Some(' ') => {
                            end += 1;
                            for _ in 1..unit.chars().count() {
                                if at(end) != Some(' ') {
                                    break;
                                }
                                end += 1;
                            }
                        }
                        _ => break,
                    }
                }
                cm.replace_range("", cursor, Some(offset(cursor, 0, end)));
            }
        }
        return Some(cursor);
    }
    for _ in 0..repeat {
        if args.indent_right == Some(true) {
            cm.indent_more();
        } else {
            cm.indent_less();
        }
    }
    Some(first_non_ws_of(cm, ranges[0].anchor))
}
