//! Vim actions (`actions`): mode changes, paste, undo, macros, scrolling,
//! replace, increment and the rest of the non-motion commands.

use super::cm::Cm;
use super::dispatch::signal_visual;
use super::helpers::*;
use super::state::with_global;
use super::types::*;
use super::{insert, mode_change};
use crate::bindings::Cmd;

pub(crate) fn run(cm: &mut Cm, action: Action, args: &ActionArgs) {
    use Action::*;
    match action {
        JumpListWalk => {
            if cm.vim().visual_mode {
                return;
            }
            let repeat = if args.repeat == 0 { 1 } else { args.repeat };
            let mark = jump_move(cm, if args.forward { repeat } else { -repeat });
            let pos = mark
                .and_then(|m| cm.mark_find(m))
                .unwrap_or(cm.get_cursor());
            cm.set_cursor(pos);
        }
        Scroll => scroll(cm, args),
        ScrollToCursor => {
            let line = cm.get_cursor().line;
            let coords = cm.char_coords(Pos::new(line, 0));
            let height = cm.get_scroll_info().client_height;
            let mut y = coords.top;
            match args.position {
                Some(ScrollPos::Center) => y = coords.bottom - height / 2.0,
                Some(ScrollPos::Bottom) => {
                    let last = Pos::new(line, cm.line_len(line) - 1);
                    let lc = cm.char_coords(last);
                    let lh = lc.bottom - y;
                    y = y - height + lh;
                }
                _ => {}
            }
            cm.scroll_to(None, Some(y));
        }
        ReplayMacro => {
            let mut name = args.selected_character.clone().unwrap_or_default();
            let mut repeat = if args.repeat == 0 { 1 } else { args.repeat };
            if name == "@" {
                name =
                    with_global(|g| g.macro_mode_state.latest_register.clone()).unwrap_or_default();
            } else {
                with_global(|g| g.macro_mode_state.latest_register = Some(name.clone()));
            }
            while repeat > 0 {
                repeat -= 1;
                insert::execute_macro_register(cm, &name);
            }
        }
        EnterMacroRecordMode => {
            let name = args.selected_character.clone();
            if with_global(|g| g.registers.is_valid(name.as_deref())) {
                insert::enter_macro_record_mode(cm, name.as_deref().unwrap_or_default());
            }
        }
        ToggleOverwrite => {
            if !cm.overwrite() {
                cm.toggle_overwrite(true);
                mode_change(cm, "replace", None);
            } else {
                cm.toggle_overwrite(false);
                mode_change(cm, "insert", None);
            }
        }
        EnterInsertMode => enter_insert_mode(cm, args),
        ToggleVisualMode => toggle_visual_mode(cm, args),
        ReselectLastSelection => {
            let last = cm.vim().last_selection;
            if cm.vim().visual_mode {
                update_last_selection(cm);
            }
            if let Some(last) = last {
                let (Some(anchor), Some(head)) =
                    (cm.mark_find(last.anchor_mark), cm.mark_find(last.head_mark))
                else {
                    return;
                };
                let vim = cm.vim();
                vim.sel = Range { anchor, head };
                vim.visual_mode = true;
                vim.visual_line = last.visual_line;
                vim.visual_block = last.visual_block;
                update_cm_selection(cm, None, None);
                update_mark(cm, "<", pmin(anchor, head));
                update_mark(cm, ">", pmax(anchor, head));
                signal_visual(cm);
            }
        }
        JoinLines => join_lines(cm, args.repeat, args.keep_spaces),
        NewLineAndEnterInsertMode => {
            cm.vim().insert_mode = true;
            let mut at = cm.get_cursor();
            if at.line == 0 && !args.after {
                cm.replace_range("\n", Pos::new(0, 0), None);
                cm.set_cursor(Pos::new(0, 0));
            } else {
                at.line = if args.after { at.line } else { at.line - 1 };
                at.ch = cm.line_len(at.line);
                cm.set_cursor(at);
                cm.newline_and_indent();
            }
            enter_insert_mode(
                cm,
                &ActionArgs {
                    repeat: args.repeat,
                    ..Default::default()
                },
            );
        }
        Paste => {
            let register = with_global(|g| g.registers.get(args.register_name.as_deref()).clone());
            if args.register_name.as_deref() == Some("+") {
                let text = cm
                    .cx
                    .read_from_clipboard()
                    .and_then(|i| i.text())
                    .unwrap_or_default();
                continue_paste(cm, args, &text, &register);
            } else {
                let text = register.text();
                continue_paste(cm, args, &text, &register);
            }
        }
        Undo => {
            cm.op_begin();
            for _ in 0..args.repeat {
                cm.run_history(true);
            }
            let start = cm.get_start();
            let p = clip_cursor_to_content(cm, start, None);
            cm.set_cursor(p);
            cm.op_end();
        }
        Redo => {
            for _ in 0..args.repeat {
                cm.run_history(false);
            }
        }
        SetRegister => cm.vim().input_state.register_name = args.selected_character.clone(),
        InsertRegister => {
            let text = with_global(|g| g.registers.get(args.selected_character.as_deref()).text());
            if !text.is_empty() {
                cm.replace_selection(&text);
            }
        }
        OneNormalCommand => {
            insert::exit_insert_mode(cm, true);
            let vim = cm.vim();
            vim.insert_mode_return = true;
            vim.one_normal_handler = true;
        }
        SetMark => {
            if let Some(name) = args.selected_character.clone() {
                let c = cm.get_cursor();
                update_mark(cm, &name, c);
            }
        }
        Replace => replace(cm, args),
        IncrementNumberToken => increment_number(cm, args),
        RepeatLastEdit => {
            let Some(mut last) = cm.vim().last_edit_input_state.clone() else {
                return;
            };
            let mut repeat = args.repeat;
            if repeat != 0 && args.repeat_is_explicit {
                last.repeat_override = Some(repeat);
                cm.vim().last_edit_input_state = Some(last);
            } else {
                repeat = last.repeat_override.unwrap_or(repeat);
            }
            insert::repeat_last_edit(cm, repeat, false);
        }
        Indent => cm.run(if args.indent_right {
            Cmd::IndentMore
        } else {
            Cmd::IndentLess
        }),
    }
}

/// `scroll` (Ctrl-E / Ctrl-Y).
fn scroll(cm: &mut Cm, args: &ActionArgs) {
    if cm.vim().visual_mode {
        return;
    }
    let repeat = if args.repeat == 0 { 1 } else { args.repeat };
    let lh = cm.default_text_height();
    let top = cm.get_scroll_info().top;
    let delta = lh * repeat as f32;
    let new_pos = if args.forward {
        top + delta
    } else {
        top - delta
    };
    let mut cursor = cm.get_cursor();
    let mut coords = cm.char_coords(cursor);
    if args.forward {
        if new_pos > coords.top {
            cursor.line = (cursor.line as f32 + (new_pos - coords.top) / lh).ceil() as i64;
            cm.set_cursor(cursor);
            coords = cm.char_coords(cursor);
            cm.scroll_to(None, Some(coords.top));
        } else {
            cm.scroll_to(None, Some(new_pos));
        }
    } else {
        let new_bottom = new_pos + cm.get_scroll_info().client_height;
        if new_bottom < coords.bottom {
            cursor.line = (cursor.line as f32 - (coords.bottom - new_bottom) / lh).floor() as i64;
            cm.set_cursor(cursor);
            coords = cm.char_coords(cursor);
            let ch = cm.get_scroll_info().client_height;
            cm.scroll_to(None, Some(coords.bottom - ch));
        } else {
            cm.scroll_to(None, Some(new_pos));
        }
    }
}

/// `enterInsertMode`.
pub(crate) fn enter_insert_mode(cm: &mut Cm, args: &ActionArgs) {
    if cm.read_only() {
        return;
    }
    cm.vim().insert_mode = true;
    cm.vim().insert_mode_repeat = Some(if args.repeat == 0 { 1 } else { args.repeat });
    let sel = cm.vim().sel;
    let mut head = args.head.unwrap_or(cm.get_cursor());
    let mut height = cm.list_selections().len() as i64;
    match args.insert_at {
        Some(InsertAt::Eol) => head = Pos::new(head.line, cm.line_len(head.line)),
        Some(InsertAt::Bol) => head = Pos::new(head.line, 0),
        Some(InsertAt::CharAfter) => head = offset(head, 0, 1),
        Some(InsertAt::FirstNonBlank) => {
            head = Pos::new(head.line, first_non_ws(&cm.get_line(head.line)))
        }
        Some(InsertAt::StartOfSelectedArea) => {
            if !cm.vim().visual_mode {
                return;
            }
            if !cm.vim().visual_block {
                head = if sel.head.line < sel.anchor.line {
                    sel.head
                } else {
                    Pos::new(sel.anchor.line, 0)
                };
            } else {
                head = Pos::new(
                    sel.head.line.min(sel.anchor.line),
                    sel.head.ch.min(sel.anchor.ch),
                );
                height = (sel.head.line - sel.anchor.line).abs() + 1;
            }
        }
        Some(InsertAt::EndOfSelectedArea) => {
            if !cm.vim().visual_mode {
                return;
            }
            if !cm.vim().visual_block {
                head = if sel.head.line >= sel.anchor.line {
                    offset(sel.head, 0, 1)
                } else {
                    Pos::new(sel.anchor.line, 0)
                };
            } else {
                head = Pos::new(
                    sel.head.line.min(sel.anchor.line),
                    sel.head.ch.max(sel.anchor.ch) + 1,
                );
                height = (sel.head.line - sel.anchor.line).abs() + 1;
            }
        }
        Some(InsertAt::Inplace) if cm.vim().visual_mode => return,
        Some(InsertAt::Inplace) => {}
        Some(InsertAt::LastEdit) => head = cm.last_edit_end(),
        None => {}
    }
    if args.replace {
        cm.toggle_overwrite(true);
        mode_change(cm, "replace", None);
    } else {
        cm.toggle_overwrite(false);
        mode_change(cm, "insert", None);
    }
    if !with_global(|g| g.macro_mode_state.is_playing) {
        cm.st().listening_changes = true;
        if let Some(old) = cm.vim().insert_end.take() {
            cm.mark_clear(old);
        }
        let id = cm.set_bookmark(head, true);
        cm.vim().insert_end = Some(id);
    }
    if cm.vim().visual_mode {
        exit_visual_mode(cm, true);
    }
    select_for_insert(cm, head, height);
}

/// `toggleVisualMode`.
fn toggle_visual_mode(cm: &mut Cm, args: &ActionArgs) {
    let repeat = args.repeat;
    let anchor = cm.get_cursor();
    if !cm.vim().visual_mode {
        let vim = cm.vim();
        vim.visual_mode = true;
        vim.visual_line = args.linewise;
        vim.visual_block = args.blockwise;
        let head = clip_cursor_to_content(cm, Pos::new(anchor.line, anchor.ch + repeat - 1), None);
        cm.vim().sel = Range { anchor, head };
        signal_visual(cm);
        update_cm_selection(cm, None, None);
        update_mark(cm, "<", pmin(anchor, head));
        update_mark(cm, ">", pmax(anchor, head));
    } else if cm.vim().visual_line != args.linewise || cm.vim().visual_block != args.blockwise {
        let vim = cm.vim();
        vim.visual_line = args.linewise;
        vim.visual_block = args.blockwise;
        signal_visual(cm);
        update_cm_selection(cm, None, None);
    } else {
        exit_visual_mode(cm, true);
    }
}

/// `joinLines`.
pub(crate) fn join_lines(cm: &mut Cm, repeat: i64, keep_spaces: bool) {
    let (cur_start, cur_end);
    if cm.vim().visual_mode {
        let mut s = cm.get_anchor();
        let mut e = cm.get_cursor();
        if before(e, s) {
            std::mem::swap(&mut s, &mut e);
        }
        e.ch = cm.line_len(e.line) - 1;
        cur_start = s;
        cur_end = e;
    } else {
        let repeat = repeat.max(2);
        cur_start = cm.get_cursor();
        cur_end = clip_cursor_to_content(cm, Pos::new(cur_start.line + repeat - 1, INF), None);
    }
    let mut final_ch = 0;
    for _ in cur_start.line..cur_end.line {
        final_ch = cm.line_len(cur_start.line);
        let mut text = "";
        let mut next_start = 0;
        if !keep_spaces {
            let next = cm.line_chars(cur_start.line + 1);
            match next.iter().position(|c| !crate::state::is_js_space(*c)) {
                Some(i) => {
                    next_start = i as i64;
                    text = " ";
                }
                None => next_start = next.len() as i64,
            }
        }
        cm.replace_range(
            text,
            Pos::new(cur_start.line, final_ch),
            Some(Pos::new(cur_start.line + 1, next_start)),
        );
    }
    let fin = clip_cursor_to_content(cm, Pos::new(cur_start.line, final_ch), None);
    if cm.vim().visual_mode {
        exit_visual_mode(cm, false);
    }
    cm.set_cursor(fin);
}

/// `continuePaste`.
fn continue_paste(cm: &mut Cm, args: &ActionArgs, text: &str, register: &super::state::Register) {
    let mut cur = cm.get_cursor();
    if text.is_empty() {
        return;
    }
    let mut text = text.to_string();
    if args.match_indent {
        let tab = cm.tab_size();
        let ws_len = |s: &str| -> i64 {
            s.matches('\t').count() as i64 * tab + s.matches(' ').count() as i64
        };
        let current = cm.get_line(cm.get_cursor().line);
        let indent = ws_len(&leading_ws(&current));
        let chomped = text.strip_suffix('\n').unwrap_or(&text).to_string();
        let was_chomped = chomped != text;
        let first_indent = ws_len(&leading_ws(&text));
        let with_tabs = cm.indent_with_tabs();
        let mut out = String::new();
        for (i, line) in chomped.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let lead = leading_ws(line);
            let new_indent = indent + (ws_len(&lead) - first_indent);
            let replacement = if new_indent < 0 {
                String::new()
            } else if with_tabs {
                "\t".repeat((new_indent / tab) as usize)
            } else {
                " ".repeat(new_indent as usize)
            };
            out.push_str(&replacement);
            out.push_str(&line[lead.len()..]);
        }
        text = out + if was_chomped { "\n" } else { "" };
    }
    if args.repeat > 1 {
        text = text.repeat(args.repeat as usize);
    }
    let linewise = register.linewise;
    let blockwise = register.blockwise;
    let mut text_lines: Option<Vec<String>> =
        blockwise.then(|| text.split('\n').map(str::to_string).collect());
    if let Some(lines) = text_lines.as_mut() {
        if linewise {
            lines.pop();
        }
        for l in lines.iter_mut() {
            if l.is_empty() {
                *l = " ".into();
            }
        }
        cur.ch += if args.after { 1 } else { 0 };
        cur.ch = cm.line_len(cur.line).min(cur.ch);
    } else if linewise {
        if cm.vim().visual_mode {
            let chars: Vec<char> = text.chars().collect();
            text = if cm.vim().visual_line {
                chars[..chars.len().saturating_sub(1)].iter().collect()
            } else {
                format!(
                    "\n{}\n",
                    chars[..chars.len().saturating_sub(1)]
                        .iter()
                        .collect::<String>()
                )
            };
        } else if args.after {
            let chars: Vec<char> = text.chars().collect();
            text = format!(
                "\n{}",
                chars[..chars.len().saturating_sub(1)]
                    .iter()
                    .collect::<String>()
            );
            cur.ch = cm.line_len(cur.line);
        } else {
            cur.ch = 0;
        }
    } else {
        cur.ch += if args.after { 1 } else { 0 };
    }
    let mut cur_pos_final;
    if cm.vim().visual_mode {
        cm.vim().last_pasted_text = Some(text.clone());
        let (selection_start, mut selection_end) = get_selected_area_range(cm);
        let selected_text = cm.get_selection();
        let count = cm.list_selections().len();
        let empty = vec![String::new(); count];
        let last_sel_end = cm
            .vim()
            .last_selection
            .and_then(|l| cm.mark_find(l.head_mark));
        with_global(|g| g.registers.unnamed().set_text(&selected_text, false, false));
        if blockwise {
            cm.replace_selections(&empty);
            selection_end = Pos::new(
                selection_start.line + text.chars().count() as i64 - 1,
                selection_start.ch,
            );
            cm.set_cursor(selection_start);
            select_block(cm, &mut selection_end);
            let chars: Vec<String> = text.chars().map(|c| c.to_string()).collect();
            cm.replace_selections(&chars);
            cur_pos_final = selection_start;
        } else if cm.vim().visual_block {
            cm.replace_selections(&empty);
            cm.set_cursor(selection_start);
            cm.replace_range(&text, selection_start, Some(selection_start));
            cur_pos_final = selection_start;
        } else {
            cm.replace_range(&text, selection_start, Some(selection_end));
            let idx = cm.index_from_pos(selection_start) + text.len();
            cur_pos_final = cm.pos_from_index(
                idx.saturating_sub(text.chars().last().map_or(1, |c| c.len_utf8())),
            );
        }
        if let Some(e) = last_sel_end
            && let Some(mut l) = cm.vim().last_selection
        {
            l.head_mark = cm.set_bookmark(e, false);
            cm.vim().last_selection = Some(l);
        }
        if linewise {
            cur_pos_final.ch = 0;
        }
    } else if let (true, Some(lines)) = (blockwise, text_lines.as_ref()) {
        cm.set_cursor(cur);
        for i in 0..lines.len() as i64 {
            let line = cur.line + i;
            if line > cm.last_line() {
                cm.replace_range("\n", Pos::new(line, 0), None);
            }
            let last_ch = cm.line_len(line);
            if last_ch < cur.ch {
                extend_line_to_column(cm, line, cur.ch);
            }
        }
        cm.set_cursor(cur);
        let mut end = Pos::new(cur.line + lines.len() as i64 - 1, cur.ch);
        select_block(cm, &mut end);
        cm.replace_selections(lines);
        cur_pos_final = cur;
    } else {
        cm.replace_range(&text, cur, None);
        if linewise {
            let line = if args.after { cur.line + 1 } else { cur.line };
            cur_pos_final = Pos::new(line, first_non_ws(&cm.get_line(line)));
        } else {
            cur_pos_final = cur;
            if !text.contains('\n') {
                cur_pos_final.ch += text.chars().count() as i64 - if args.after { 1 } else { 0 };
            }
        }
    }
    if cm.vim().visual_mode {
        exit_visual_mode(cm, false);
    }
    cm.set_cursor(cur_pos_final);
}

fn leading_ws(s: &str) -> String {
    s.chars()
        .take_while(|c| crate::state::is_js_space(*c))
        .collect()
}

/// `replace` (`r<character>`).
fn replace(cm: &mut Cm, args: &ActionArgs) {
    let with = args.selected_character.clone().unwrap_or_default();
    let mut cur_start = cm.get_cursor();
    let cur_end;
    let selections = cm.list_selections();
    if cm.vim().visual_mode {
        cur_start = cm.get_start();
        cur_end = cm.get_end();
    } else {
        let len = cm.line_len(cur_start.line);
        let to = (cur_start.ch + args.repeat).min(len);
        cur_end = Pos::new(cur_start.line, to);
    }
    if with == "\n" {
        if !cm.vim().visual_mode {
            cm.replace_range("", cur_start, Some(cur_end));
        }
        cm.newline_and_indent();
        return;
    }
    let orig = cm.get_range(cur_start, cur_end);
    let replaced: String = orig
        .chars()
        .map(|c| {
            if c == '\n' {
                "\n".to_string()
            } else {
                with.clone()
            }
        })
        .collect();
    if cm.vim().visual_block {
        let spaces = " ".repeat(cm.tab_size() as usize);
        let sel = cm.get_selection();
        let strings: Vec<String> = sel
            .replace('\t', &spaces)
            .chars()
            .map(|c| {
                if c == '\n' {
                    "\n".to_string()
                } else {
                    with.clone()
                }
            })
            .collect::<String>()
            .split('\n')
            .map(str::to_string)
            .collect();
        cm.replace_selections(&strings);
    } else {
        cm.replace_range(&replaced, cur_start, Some(cur_end));
    }
    if cm.vim().visual_mode {
        let s = if before(selections[0].anchor, selections[0].head) {
            selections[0].anchor
        } else {
            selections[0].head
        };
        cm.set_cursor(s);
        exit_visual_mode(cm, false);
    } else {
        cm.set_cursor(offset(cur_end, 0, -1));
    }
}

/// `incrementNumberToken` (`<C-a>` / `<C-x>`).
fn increment_number(cm: &mut Cm, args: &ActionArgs) {
    let cur = cm.get_cursor();
    let line: Vec<char> = cm.line_chars(cur.line);
    // /(-?)(?:(0x)([\da-f]+)|(0b|0|)(\d+))/gi
    let mut found: Option<(usize, usize, bool, String, String)> = None;
    let mut i = 0;
    while i < line.len() {
        if let Some(m) = number_at(&line, i) {
            let (start, end) = (i, m.0);
            found = Some((start, end, m.1, m.2.clone(), m.3.clone()));
            if (cur.ch as usize) < end {
                break;
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    let Some((start, end, neg, base_str, digits)) = found else {
        return;
    };
    if !args.backtrack && end as i64 <= cur.ch {
        return;
    }
    let base: u32 = match base_str.to_lowercase().as_str() {
        "0b" => 2,
        "0" => 8,
        "0x" => 16,
        _ => 10,
    };
    let increment: i64 = if args.increase { 1 } else { -1 };
    let parsed = i64::from_str_radix(
        &format!(
            "{}{}",
            if neg { "-" } else { "" },
            parse_prefix(&digits, base)
        ),
        base,
    )
    .unwrap_or(0);
    let number = parsed + increment * args.repeat;
    let mut number_str = to_radix(number, base);
    let zero_padding = if !base_str.is_empty() {
        "0".repeat((digits.len() as i64 - number_str.len() as i64 + i64::from(neg)).max(0) as usize)
    } else {
        String::new()
    };
    number_str = if let Some(rest) = number_str.strip_prefix('-') {
        format!("-{base_str}{zero_padding}{rest}")
    } else {
        format!("{base_str}{zero_padding}{number_str}")
    };
    cm.replace_range(
        &number_str,
        Pos::new(cur.line, start as i64),
        Some(Pos::new(cur.line, end as i64)),
    );
    cm.set_cursor(Pos::new(
        cur.line,
        start as i64 + number_str.chars().count() as i64 - 1,
    ));
}

/// `parseInt(digits, base)`: the longest valid prefix.
fn parse_prefix(digits: &str, base: u32) -> String {
    digits
        .chars()
        .take_while(|c| c.is_digit(base))
        .collect::<String>()
}

fn to_radix(n: i64, base: u32) -> String {
    if base == 10 {
        return n.to_string();
    }
    let neg = n < 0;
    let mut v = n.unsigned_abs();
    if v == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while v > 0 {
        out.push(std::char::from_digit((v % base as u64) as u32, base).unwrap_or('0'));
        v /= base as u64;
    }
    let s: String = out.into_iter().rev().collect();
    if neg { format!("-{s}") } else { s }
}

/// One match of the number regex at `i`: (end, negative, base prefix,
/// digits).
fn number_at(line: &[char], i: usize) -> Option<(usize, bool, String, String)> {
    let mut j = i;
    let neg = line.get(j) == Some(&'-');
    if neg {
        j += 1;
    }
    let lower = |k: usize| line.get(k).map(|c| c.to_ascii_lowercase());
    if line.get(j) == Some(&'0') && lower(j + 1) == Some('x') {
        let hex: String = line[j + 2..]
            .iter()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if !hex.is_empty() {
            let end = j + 2 + hex.len();
            return Some((end, neg, line[j..j + 2].iter().collect(), hex));
        }
    }
    for prefix in ["0b", "0", ""] {
        let plen = prefix.len();
        if line.len() < j + plen {
            continue;
        }
        let candidate: String = line[j..j + plen].iter().collect::<String>().to_lowercase();
        if candidate != prefix {
            continue;
        }
        let digits: String = line[j + plen..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return Some((
                j + plen + digits.len(),
                neg,
                line[j..j + plen].iter().collect(),
                digits,
            ));
        }
    }
    if neg {
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_tokens() {
        let l: Vec<char> = "x = -0x1F;".chars().collect();
        let m = number_at(&l, 4).unwrap();
        assert_eq!(m, (9, true, "0x".into(), "1F".into()));
        let l: Vec<char> = "a 0b101 b".chars().collect();
        assert_eq!(
            number_at(&l, 2).unwrap(),
            (7, false, "0b".into(), "101".into())
        );
        let l: Vec<char> = "007".chars().collect();
        assert_eq!(
            number_at(&l, 0).unwrap(),
            (3, false, "0".into(), "07".into())
        );
        assert_eq!(to_radix(-31, 16), "-1f");
    }
}
