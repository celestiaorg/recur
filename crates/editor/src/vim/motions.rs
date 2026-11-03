//! Vim motions (`motions`) and the scanners they share: words, paragraphs,
//! sentences, symbols and text objects.

use super::cm::Cm;
use super::helpers::*;
use super::state::{InputState, with_global};
use super::types::*;
use crate::bindings::Cmd;

fn ch_at(line: &[char], i: i64) -> Option<char> {
    (i >= 0).then(|| line.get(i as usize).copied()).flatten()
}

/// `wordCharTest[0]`.
fn word_char(c: Option<char>) -> bool {
    c.is_some_and(is_word_char)
}

/// `wordCharTest[1]`: punctuation.
fn punct_char(c: Option<char>) -> bool {
    c.is_some_and(|c| !is_word_char(c) && !crate::state::is_js_space(c))
}

/// `bigWordCharTest[0]`.
fn big_word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| !crate::state::is_js_space(c))
}

fn is_space(c: Option<char>) -> bool {
    c.is_some_and(crate::state::is_js_space)
}

/// `isLine`.
fn is_line(cm: &Cm, n: i64) -> bool {
    (0..=cm.last_line()).contains(&n)
}

/// Run a motion (`motions[name]`).
pub(crate) fn run(
    cm: &mut Cm,
    motion: Motion,
    head: Pos,
    args: &mut MotionArgs,
    prev: &InputState,
) -> Option<MotionResult> {
    use Motion::*;
    let h = MotionResult::Head;
    match motion {
        MoveToTopLine => {
            let line = user_visible_lines(cm).0 + args.repeat - 1;
            Some(h(Pos::new(line, first_non_ws(&cm.get_line(line)))))
        }
        MoveToMiddleLine => {
            let (top, bottom) = user_visible_lines(cm);
            let line = ((top + bottom) as f64 * 0.5).floor() as i64;
            Some(h(Pos::new(line, first_non_ws(&cm.get_line(line)))))
        }
        MoveToBottomLine => {
            let line = user_visible_lines(cm).1 - args.repeat + 1;
            Some(h(Pos::new(line, first_non_ws(&cm.get_line(line)))))
        }
        ExpandToLine => Some(h(Pos::new(head.line + args.repeat - 1, INF))),
        FindNext => {
            let query = with_global(|g| g.query.clone())?;
            let mut prev_dir = !args.fwd();
            if with_global(|g| g.is_reversed) {
                prev_dir = !prev_dir;
            }
            super::search::highlight_search_matches(cm, &query);
            let result = super::search::find_next(cm, prev_dir, &query, args.repeat);
            if result.is_none() {
                let pcre = with_global(|g| g.pcre());
                show_confirm(
                    cm,
                    &format!(
                        "No match found {}{}",
                        query.display(),
                        if pcre {
                            " (set nopcre to use Vim regexps)"
                        } else {
                            ""
                        }
                    ),
                );
            }
            result.map(h)
        }
        FindAndSelectNextInclusive => {
            let query = with_global(|g| g.query.clone())?;
            let reversed = with_global(|g| g.is_reversed);
            let mut prev_dir = !args.fwd();
            if reversed {
                prev_dir = !prev_dir;
            }
            let (from, to_) =
                super::search::find_next_from_and_to_inclusive(cm, prev_dir, &query, args.repeat)?;
            if prev.operator.is_some() {
                return Some(MotionResult::Range(from, to_));
            }
            let to = Pos::new(to_.line, to_.ch - 1);
            if cm.vim().visual_mode {
                if cm.vim().visual_line || cm.vim().visual_block {
                    cm.vim().visual_line = false;
                    cm.vim().visual_block = false;
                    super::mode_change(cm, "visual", Some(""));
                }
                let anchor = cm.vim().sel.anchor;
                return Some(if reversed {
                    if args.fwd() {
                        MotionResult::Range(anchor, from)
                    } else {
                        MotionResult::Range(anchor, to)
                    }
                } else if args.fwd() {
                    MotionResult::Range(anchor, to)
                } else {
                    MotionResult::Range(anchor, from)
                });
            }
            let vim = cm.vim();
            vim.visual_mode = true;
            vim.visual_line = false;
            vim.visual_block = false;
            super::mode_change(cm, "visual", Some(""));
            Some(if prev_dir {
                MotionResult::Range(to, from)
            } else {
                MotionResult::Range(from, to)
            })
        }
        GoToMark => {
            let pos = get_mark_pos(cm, args.selected_character.as_deref().unwrap_or(""))?;
            Some(h(if args.linewise {
                Pos::new(pos.line, first_non_ws(&cm.get_line(pos.line)))
            } else {
                pos
            }))
        }
        MoveToOtherHighlightedEnd => {
            let sel = cm.vim().sel;
            if cm.vim().visual_block && args.same_line {
                let a = clip_cursor_to_content(cm, Pos::new(sel.anchor.line, sel.head.ch), None);
                let b = clip_cursor_to_content(cm, Pos::new(sel.head.line, sel.anchor.ch), None);
                Some(MotionResult::Range(a, b))
            } else {
                Some(MotionResult::Range(sel.head, sel.anchor))
            }
        }
        JumpToMark => {
            let mut best = head;
            let marks: Vec<(String, u64)> = cm
                .vim()
                .marks
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            for _ in 0..args.repeat {
                let cursor = best;
                for (key, id) in &marks {
                    if !(key.len() == 1 && key.chars().all(|c| c.is_ascii_lowercase())) {
                        continue;
                    }
                    let Some(mark) = cm.mark_find(*id) else {
                        continue;
                    };
                    let wrong = if args.fwd() {
                        before(mark, cursor)
                    } else {
                        before(cursor, mark)
                    };
                    if wrong {
                        continue;
                    }
                    if args.linewise && mark.line == cursor.line {
                        continue;
                    }
                    let equal = cursor == best;
                    let btw = if args.fwd() {
                        between(cursor, mark, best)
                    } else {
                        between(best, mark, cursor)
                    };
                    if equal || btw {
                        best = mark;
                    }
                }
            }
            if args.linewise {
                best = Pos::new(best.line, first_non_ws(&cm.get_line(best.line)));
            }
            Some(h(best))
        }
        MoveByCharacters => {
            let ch = if args.fwd() {
                head.ch + args.repeat
            } else {
                head.ch - args.repeat
            };
            Some(h(Pos::new(head.line, ch)))
        }
        MoveByLines => Some(h(move_by_lines(cm, head, args))),
        MoveByDisplayLines => Some(h(move_by_display_lines(cm, head, args))),
        MoveByPage => {
            let (p, _) = cm.find_pos_v(
                head,
                if args.fwd() {
                    args.repeat
                } else {
                    -args.repeat
                },
                true,
                None,
            );
            Some(h(p))
        }
        MoveByParagraph => {
            let dir = if args.fwd() { 1 } else { -1 };
            Some(h(find_paragraph(cm, head, args.repeat, dir, false).0))
        }
        MoveBySentence => {
            let dir = if args.fwd() { 1 } else { -1 };
            Some(h(find_sentence(cm, head, args.repeat, dir)))
        }
        MoveByScroll => {
            let info = cm.get_scroll_info();
            let mut repeat = args.repeat;
            if repeat == 0 {
                repeat = (info.client_height / (2.0 * cm.default_text_height())).round() as i64;
            }
            let orig = cm.char_coords(head);
            args.repeat = repeat;
            let cur_end = move_by_display_lines(cm, head, args);
            let dest = cm.char_coords(cur_end);
            cm.scroll_to(None, Some(info.top + dest.top - orig.top));
            Some(h(cur_end))
        }
        MoveByWords => move_to_word(
            cm,
            head,
            args.repeat,
            args.fwd(),
            args.word_end,
            args.big_word,
        )
        .map(h),
        MoveTillCharacter => {
            let cur_end = move_to_character(
                cm,
                args.repeat,
                args.fwd(),
                args.selected_character.as_deref(),
                Some(head),
            );
            let increment = if args.fwd() { -1 } else { 1 };
            record_last_character_search(increment, args);
            let mut e = cur_end?;
            e.ch += increment;
            Some(h(e))
        }
        MoveToCharacter => {
            record_last_character_search(0, args);
            Some(h(move_to_character(
                cm,
                args.repeat,
                args.fwd(),
                args.selected_character.as_deref(),
                Some(head),
            )
            .unwrap_or(head)))
        }
        MoveToSymbol => {
            let r = args
                .selected_character
                .clone()
                .and_then(|c| find_symbol(cm, args.repeat, args.fwd(), &c));
            Some(h(r.unwrap_or(head)))
        }
        MoveToColumn => {
            cm.vim().last_h_pos = args.repeat - 1;
            cm.vim().last_hs_pos = cm.char_coords(head).left;
            let line = cm.get_cursor().line;
            Some(h(clip_cursor_to_content(
                cm,
                Pos::new(line, args.repeat - 1),
                None,
            )))
        }
        MoveToEol => Some(h(move_to_eol(cm, head, args, false))),
        MoveToFirstNonWhiteSpaceCharacter => Some(h(Pos::new(
            head.line,
            first_non_ws(&cm.get_line(head.line)),
        ))),
        MoveToMatchedSymbol => {
            let line = cm.line_chars(head.line);
            let mut ch = head.ch.max(0) as usize;
            while ch < line.len() {
                let symbol = line[ch];
                if "()[]{}".contains(symbol) {
                    break;
                }
                ch += 1;
            }
            if ch < line.len() {
                cm.find_matching_bracket(Pos::new(head.line, ch as i64))
                    .map(h)
            } else {
                Some(h(head))
            }
        }
        MoveToStartOfLine => Some(h(Pos::new(head.line, 0))),
        MoveToLineOrEdgeOfDocument => {
            let mut line = if args.fwd() { cm.last_line() } else { 0 };
            if args.repeat_is_explicit {
                line = args.repeat - 1;
            }
            Some(h(Pos::new(line, first_non_ws(&cm.get_line(line)))))
        }
        MoveToStartOfDisplayLine => {
            cm.run(Cmd::CursorLineBoundaryBackward);
            Some(h(cm.get_cursor()))
        }
        MoveToEndOfDisplayLine => {
            // `goLineRight`: at a wrap the boundary is the next row's
            // start, so step back onto this row. Positions carry no
            // `sticky`, so the package's own step back never applies.
            cm.run(Cmd::CursorLineBoundaryForward);
            let state = &cm.view.state;
            let cur = state.selection.main().head();
            if cur < state.doc.len()
                && state.slice_doc(cur, state.doc.clamp_to_char(cur + 1).max(cur + 1)) != "\n"
            {
                cm.run(Cmd::CursorCharLeft);
            }
            Some(h(cm.get_cursor()))
        }
        TextObjectManipulation => text_object(cm, head, args),
        RepeatLastCharacterSearch => {
            let last = with_global(|g| g.last_character_search.clone());
            let forward = args.fwd() == last.forward;
            let increment =
                (if last.increment != 0 { 1 } else { 0 }) * if forward { -1 } else { 1 };
            cm.set_cursor(offset(cm.get_cursor(), 0, -increment));
            args.inclusive = forward;
            let Some(mut e) = move_to_character(
                cm,
                args.repeat,
                forward,
                Some(&last.selected_character),
                None,
            ) else {
                cm.set_cursor(offset(cm.get_cursor(), 0, increment));
                return Some(h(head));
            };
            e.ch += increment;
            Some(h(e))
        }
    }
}

/// `moveByLines`.
fn move_by_lines(cm: &mut Cm, cur: Pos, args: &MotionArgs) -> Pos {
    let mut end_ch = cur.ch;
    use Motion::*;
    match cm.vim().last_motion {
        Some(MoveByLines)
        | Some(MoveByDisplayLines)
        | Some(MoveByScroll)
        | Some(MoveToColumn)
        | Some(MoveToEol) => {
            end_ch = cm.vim().last_h_pos;
        }
        _ => cm.vim().last_h_pos = end_ch,
    }
    let repeat = args.repeat + args.repeat_offset;
    let mut line = if args.fwd() {
        cur.line + repeat
    } else {
        cur.line - repeat
    };
    let first = 0;
    let last = cm.last_line();
    let goal = cm.vim().last_hs_pos;
    let (pos_v, _) = cm.find_pos_v(
        cur,
        if args.fwd() { repeat } else { -repeat },
        false,
        Some(goal),
    );
    let has_marked = if args.fwd() {
        pos_v.line > line
    } else {
        pos_v.line < line
    };
    if has_marked {
        line = pos_v.line;
        end_ch = pos_v.ch;
    }
    if line < first && cur.line == first {
        return Pos::new(cur.line, 0);
    } else if line > last && cur.line == last {
        return move_to_eol(cm, cur, args, true);
    }
    if args.to_first_char {
        end_ch = first_non_ws(&cm.get_line(line));
        cm.vim().last_h_pos = end_ch;
    }
    cm.vim().last_hs_pos = cm.char_coords(Pos::new(line, end_ch)).left;
    Pos::new(line, end_ch)
}

/// `moveByDisplayLines`.
fn move_by_display_lines(cm: &mut Cm, cur: Pos, args: &MotionArgs) -> Pos {
    use Motion::*;
    match cm.vim().last_motion {
        Some(MoveByDisplayLines)
        | Some(MoveByScroll)
        | Some(MoveByLines)
        | Some(MoveToColumn)
        | Some(MoveToEol) => {}
        _ => cm.vim().last_hs_pos = cm.char_coords(cur).left,
    }
    let repeat = args.repeat;
    let goal = cm.vim().last_hs_pos;
    let (mut res, hit) = cm.find_pos_v(
        cur,
        if args.fwd() { repeat } else { -repeat },
        false,
        Some(goal),
    );
    if hit {
        if args.fwd() {
            let last = cm.char_coords(res);
            res = cm.coords_char(goal, last.top + 8.0);
        } else {
            let first = cm.char_coords(Pos::new(0, 0));
            res = cm.coords_char(goal, first.top);
        }
    }
    cm.vim().last_h_pos = res.ch;
    res
}

/// `moveToEol`.
fn move_to_eol(cm: &mut Cm, cur: Pos, args: &MotionArgs, keep_h_pos: bool) -> Pos {
    let retval = Pos::new(cur.line + args.repeat - 1, INF);
    let mut end = cm.clip_pos(retval);
    end.ch -= 1;
    if !keep_h_pos {
        cm.vim().last_h_pos = INF;
        cm.vim().last_hs_pos = cm.char_coords(end).left;
    }
    retval
}

/// `getUserVisibleLines`.
pub fn user_visible_lines(cm: &Cm) -> (i64, i64) {
    let info = cm.get_scroll_info();
    let from = cm.coords_char(0.0, 6.0 + info.top);
    let bottom = info.client_height - 10.0 + info.top;
    let to = cm.coords_char(0.0, bottom);
    (from.line, to.line)
}

fn record_last_character_search(increment: i64, args: &MotionArgs) {
    with_global(|g| {
        let l = &mut g.last_character_search;
        l.increment = increment;
        l.forward = args.fwd();
        l.selected_character = args.selected_character.clone().unwrap_or_default();
    });
}

/// `moveToCharacter`.
fn move_to_character(
    cm: &Cm,
    repeat: i64,
    forward: bool,
    character: Option<&str>,
    head: Option<Pos>,
) -> Option<Pos> {
    let character = character.filter(|c| !c.is_empty())?;
    let cur = head.unwrap_or(cm.get_cursor());
    let mut start = cur.ch;
    let mut idx = None;
    for _ in 0..repeat {
        let line = cm.line_chars(cur.line);
        let found = char_idx_in_line(start, &line, character, forward)?;
        idx = Some(found);
        start = found;
    }
    idx.map(|i| Pos::new(cm.get_cursor().line, i))
}

/// `charIdxInLine` (`indexOf` / `lastIndexOf` of a one-char string).
fn char_idx_in_line(start: i64, line: &[char], character: &str, forward: bool) -> Option<i64> {
    let target: Vec<char> = character.chars().collect();
    let n = line.len() as i64;
    let matches_at = |i: i64| -> bool {
        i >= 0
            && (i as usize) + target.len() <= line.len()
            && line[i as usize..i as usize + target.len()] == target[..]
    };
    if forward {
        let mut i = (start + 1).max(0);
        while i < n {
            if matches_at(i) {
                return Some(i);
            }
            i += 1;
        }
        None
    } else {
        let from = start - 1;
        if from < 0 {
            return if matches_at(0) { Some(0) } else { None };
        }
        let mut i = from.min(n - 1);
        while i >= 0 {
            if matches_at(i) {
                return Some(i);
            }
            i -= 1;
        }
        None
    }
}

/// A word's span (`findWord`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Word {
    pub from: i64,
    pub to: i64,
    pub line: i64,
}

/// `findWord`.
pub fn find_word(
    cm: &Cm,
    cur: Pos,
    forward: bool,
    big_word: bool,
    empty_line_is_word: bool,
) -> Option<Word> {
    let mut line_num = cur.line;
    let mut pos = cur.ch;
    let mut line = cm.line_chars(line_num);
    let dir: i64 = if forward { 1 } else { -1 };
    let tests: Vec<fn(Option<char>) -> bool> = if big_word {
        vec![big_word_char]
    } else {
        vec![word_char, punct_char]
    };
    if empty_line_is_word && line.is_empty() {
        line_num += dir;
        line = cm.line_chars(line_num);
        if !is_line(cm, line_num) {
            return None;
        }
        pos = if forward { 0 } else { line.len() as i64 };
    }
    loop {
        if empty_line_is_word && line.is_empty() {
            return Some(Word {
                from: 0,
                to: 0,
                line: line_num,
            });
        }
        let stop = if dir > 0 { line.len() as i64 } else { -1 };
        while pos != stop {
            let mut found = false;
            let mut i = 0;
            while i < tests.len() && !found {
                if tests[i](ch_at(&line, pos)) {
                    let word_start = pos;
                    while pos != stop && tests[i](ch_at(&line, pos)) {
                        pos += dir;
                    }
                    let word_end = pos;
                    found = word_start != word_end;
                    if word_start == cur.ch && line_num == cur.line && word_end == word_start + dir
                    {
                        i += 1;
                        continue;
                    }
                    return Some(Word {
                        from: word_start.min(word_end + 1),
                        to: word_start.max(word_end),
                        line: line_num,
                    });
                }
                i += 1;
            }
            if !found {
                pos += dir;
            }
        }
        line_num += dir;
        if !is_line(cm, line_num) {
            return None;
        }
        line = cm.line_chars(line_num);
        pos = if dir > 0 { 0 } else { line.len() as i64 };
    }
}

/// `moveToWord`.
fn move_to_word(
    cm: &Cm,
    cur: Pos,
    mut repeat: i64,
    forward: bool,
    word_end: bool,
    big_word: bool,
) -> Option<Pos> {
    let cur_start = cur;
    let mut cur = cur;
    let mut words: Vec<Word> = Vec::new();
    if (forward && !word_end) || (!forward && word_end) {
        repeat += 1;
    }
    let empty_line_is_word = !(forward && word_end);
    for _ in 0..repeat {
        match find_word(cm, cur, forward, big_word, empty_line_is_word) {
            Some(w) => {
                words.push(w);
                cur = Pos::new(w.line, if forward { w.to - 1 } else { w.from });
            }
            None => {
                let eod = cm.line_len(cm.last_line());
                words.push(if forward {
                    Word {
                        line: cm.last_line(),
                        from: eod,
                        to: eod,
                    }
                } else {
                    Word {
                        line: 0,
                        from: 0,
                        to: 0,
                    }
                });
                break;
            }
        }
    }
    let short_circuit = words.len() as i64 != repeat;
    let first = words[0];
    let mut last = words.pop();
    if forward && !word_end {
        if !short_circuit && (first.from != cur_start.ch || first.line != cur_start.line) {
            last = words.pop();
        }
        last.map(|w| Pos::new(w.line, w.from))
    } else if forward && word_end {
        last.map(|w| Pos::new(w.line, w.to - 1))
    } else if !forward && word_end {
        if !short_circuit && (first.to != cur_start.ch || first.line != cur_start.line) {
            last = words.pop();
        }
        last.map(|w| Pos::new(w.line, w.to))
    } else {
        last.map(|w| Pos::new(w.line, w.from))
    }
}

/// `findParagraph`: (start, end).
pub fn find_paragraph(
    cm: &Cm,
    head: Pos,
    mut repeat: i64,
    dir: i64,
    mut inclusive: bool,
) -> (Pos, Pos) {
    let mut line = head.line;
    let min = 0;
    let max = cm.last_line();
    let is_empty = |i: i64| cm.get_line(i).is_empty();
    let is_boundary = |i: i64, dir: i64, any: bool| -> bool {
        if any {
            return is_empty(i) != is_empty(i + dir);
        }
        !is_empty(i) && is_empty(i + dir)
    };
    let mut i = line;
    if dir != 0 {
        while min <= i && i <= max && repeat > 0 {
            if is_boundary(i, dir, false) {
                repeat -= 1;
            }
            i += dir;
        }
        return (Pos::new(i, 0), head);
    }
    let vim = cm.vim_ref();
    if vim.visual_line && is_boundary(line, 1, true) {
        let anchor = vim.sel.anchor;
        if is_boundary(anchor.line, -1, true) && (!inclusive || anchor.line != line) {
            line += 1;
        }
    }
    let mut start_state = is_empty(line);
    i = line;
    while i <= max && repeat != 0 {
        if is_boundary(i, 1, true) && (!inclusive || is_empty(i) != start_state) {
            repeat -= 1;
        }
        i += 1;
    }
    let end = Pos::new(i, 0);
    if i > max && !start_state {
        start_state = true;
    } else {
        inclusive = false;
    }
    i = line;
    while i > min {
        if (!inclusive || is_empty(i) == start_state || i == line) && is_boundary(i, -1, true) {
            break;
        }
        i -= 1;
    }
    (Pos::new(i, 0), end)
}

fn is_eos(c: Option<char>) -> bool {
    c.is_some_and(|c| ".?!".contains(c))
}

/// `getSentence`.
fn get_sentence(cm: &Cm, cur: Pos, mut repeat: i64, dir: i64, inclusive: bool) -> Pos {
    struct Curr {
        line: Option<Vec<char>>,
        ln: i64,
        pos: i64,
        dir: i64,
    }
    fn next_char(c: &mut Curr) {
        let Some(line) = c.line.as_ref() else { return };
        if c.pos + c.dir < 0 || c.pos + c.dir >= line.len() as i64 {
            c.line = None;
        } else {
            c.pos += c.dir;
        }
    }
    let at =
        |l: &Option<Vec<char>>, i: i64| -> Option<char> { l.as_ref().and_then(|l| ch_at(l, i)) };
    let forward = |ln: i64, pos: i64, dir: i64| -> (i64, i64) {
        let line = cm.line_chars(ln);
        let mut c = Curr {
            line: Some(line.clone()),
            ln,
            pos,
            dir,
        };
        if line.is_empty() {
            return (c.ln, c.pos);
        }
        let mut last = c.pos;
        next_char(&mut c);
        while c.line.is_some() {
            last = c.pos;
            if is_eos(at(&c.line, c.pos)) {
                if !inclusive {
                    return (c.ln, c.pos + 1);
                }
                next_char(&mut c);
                while c.line.is_some() {
                    if is_ws(at(&c.line, c.pos)) {
                        last = c.pos;
                        next_char(&mut c);
                    } else {
                        break;
                    }
                }
                return (c.ln, last + 1);
            }
            next_char(&mut c);
        }
        (c.ln, last + 1)
    };
    let reverse = |ln: i64, pos: i64, dir: i64| -> (i64, i64) {
        let line = cm.line_chars(ln);
        let mut c = Curr {
            line: Some(line.clone()),
            ln,
            pos,
            dir,
        };
        if line.is_empty() {
            return (c.ln, c.pos);
        }
        let mut last = c.pos;
        next_char(&mut c);
        while c.line.is_some() {
            let ch = at(&c.line, c.pos);
            if !is_ws(ch) && !is_eos(ch) {
                last = c.pos;
            } else if is_eos(ch) {
                if !inclusive {
                    return (c.ln, last);
                }
                if is_ws(at(&c.line, c.pos + 1)) {
                    return (c.ln, c.pos + 1);
                }
                return (c.ln, last);
            }
            next_char(&mut c);
        }
        c.line = Some(line);
        if inclusive && is_ws(at(&c.line, c.pos)) {
            (c.ln, c.pos)
        } else {
            (c.ln, last)
        }
    };
    let mut idx = (cur.line, cur.ch);
    while repeat > 0 {
        idx = if dir < 0 {
            reverse(idx.0, idx.1, dir)
        } else {
            forward(idx.0, idx.1, dir)
        };
        repeat -= 1;
    }
    Pos::new(idx.0, idx.1)
}

/// `findSentence`.
fn find_sentence(cm: &Cm, cur: Pos, mut repeat: i64, dir: i64) -> Pos {
    struct Idx {
        line: Option<Vec<char>>,
        ln: i64,
        pos: i64,
        dir: i64,
    }
    let next_char = |idx: &mut Idx| {
        let Some(line) = idx.line.as_ref() else {
            return;
        };
        if idx.pos + idx.dir < 0 || idx.pos + idx.dir >= line.len() as i64 {
            idx.ln += idx.dir;
            if !is_line(cm, idx.ln) {
                idx.line = None;
                return;
            }
            let l = cm.line_chars(idx.ln);
            idx.pos = if idx.dir > 0 { 0 } else { l.len() as i64 - 1 };
            idx.line = Some(l);
        } else {
            idx.pos += idx.dir;
        }
    };
    let at =
        |l: &Option<Vec<char>>, i: i64| -> Option<char> { l.as_ref().and_then(|l| ch_at(l, i)) };
    let forward = |ln: i64, pos: i64, dir: i64| -> (i64, i64) {
        let line = cm.line_chars(ln);
        let mut stop = line.is_empty();
        let mut c = Idx {
            line: Some(line.clone()),
            ln,
            pos,
            dir,
        };
        let mut last = (c.ln, c.pos);
        let skip_empty = line.is_empty();
        next_char(&mut c);
        while let Some(l) = c.line.clone() {
            last = (c.ln, c.pos);
            if (l.is_empty() && !skip_empty)
                || (stop && !l.is_empty() && !is_ws(at(&c.line, c.pos)))
            {
                return (c.ln, c.pos);
            } else if is_eos(at(&c.line, c.pos))
                && !stop
                && (c.pos == l.len() as i64 - 1 || is_ws(at(&c.line, c.pos + 1)))
            {
                stop = true;
            }
            next_char(&mut c);
        }
        let line = cm.line_chars(last.0);
        last.1 = 0;
        for i in (0..line.len()).rev() {
            if !is_ws(Some(line[i])) {
                last.1 = i as i64;
                break;
            }
        }
        last
    };
    let reverse = |ln: i64, pos: i64, dir: i64| -> (i64, i64) {
        let line = cm.line_chars(ln);
        let mut c = Idx {
            line: Some(line.clone()),
            ln,
            pos,
            dir,
        };
        let mut last_ln = c.ln;
        let mut last_pos: Option<i64> = None;
        let mut skip_empty = line.is_empty();
        next_char(&mut c);
        while let Some(l) = c.line.clone() {
            if l.is_empty() && !skip_empty {
                return match last_pos {
                    Some(p) => (last_ln, p),
                    None => (c.ln, c.pos),
                };
            } else if is_eos(at(&c.line, c.pos))
                && last_pos.is_some()
                && !(c.ln == last_ln && Some(c.pos + 1) == last_pos)
            {
                return (last_ln, last_pos.unwrap_or(0));
            } else if !l.is_empty() && !is_ws(at(&c.line, c.pos)) {
                skip_empty = false;
                last_ln = c.ln;
                last_pos = Some(c.pos);
            }
            next_char(&mut c);
        }
        let line = cm.line_chars(last_ln);
        let mut p = 0;
        for (i, ch) in line.iter().enumerate() {
            if !is_ws(Some(*ch)) {
                p = i as i64;
                break;
            }
        }
        (last_ln, p)
    };
    let mut idx = (cur.line, cur.ch);
    while repeat > 0 {
        idx = if dir < 0 {
            reverse(idx.0, idx.1, dir)
        } else {
            forward(idx.0, idx.1, dir)
        };
        repeat -= 1;
    }
    Pos::new(idx.0, idx.1)
}

/// `findSymbol`.
fn find_symbol(cm: &Cm, mut repeat: i64, forward: bool, symb: &str) -> Option<Pos> {
    let mut cur = cm.get_cursor();
    let increment: i64 = if forward { 1 } else { -1 };
    let end_line = if forward { cm.line_count() } else { -1 };
    let mode = match symb {
        "(" | ")" | "{" | "}" => "bracket",
        "[" | "]" => "section",
        "*" | "/" => "comment",
        "m" | "M" => "method",
        "#" => "preprocess",
        _ => return Some(cur),
    };
    let mut line = cur.line;
    let mut line_text = cm.line_chars(line);
    let mut index = cur.ch;
    let mut next_ch = ch_at(&line_text, index);
    let mut last_ch: Option<char> = None;
    let mut symb_c = symb.chars().next();
    let mut reverse = if forward {
        match symb {
            ")" => Some('('),
            "}" => Some('{'),
            _ => None,
        }
    } else {
        match symb {
            "(" => Some(')'),
            "{" => Some('}'),
            _ => None,
        }
    };
    let mut depth: i64 = 0;
    let mut move_through = false;
    match mode {
        "section" => {
            move_through = true;
            symb_c = Some(
                if (if forward { ']' } else { '[' }) == symb_c.unwrap_or(' ') {
                    '{'
                } else {
                    '}'
                },
            );
        }
        "method" => {
            symb_c = Some(if symb == "m" { '{' } else { '}' });
            reverse = Some(if symb_c == Some('{') { '}' } else { '{' });
        }
        "preprocess" => index = 0,
        _ => {}
    }
    while line != end_line && repeat != 0 {
        index += increment;
        next_ch = ch_at(&line_text, index);
        if next_ch.is_none() {
            line += increment;
            line_text = cm.line_chars(line);
            index = if increment > 0 {
                0
            } else {
                let len = line_text.len() as i64;
                if len > 0 { len - 1 } else { 0 }
            };
            next_ch = ch_at(&line_text, index);
        }
        let complete = match mode {
            "bracket" => {
                if next_ch == symb_c {
                    depth += 1;
                    depth >= 1
                } else {
                    if next_ch == reverse {
                        depth -= 1;
                    }
                    false
                }
            }
            "section" => index == 0 && next_ch == symb_c,
            "comment" => {
                let found = last_ch == Some('*') && next_ch == Some('/');
                last_ch = next_ch;
                found
            }
            "method" => next_ch == symb_c,
            _ => {
                if next_ch == Some('#') {
                    let token: String = line_text
                        .iter()
                        .skip(1)
                        .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                        .collect();
                    let token = if line_text.first() == Some(&'#') {
                        token
                    } else {
                        String::new()
                    };
                    let mut done = false;
                    if token == "endif" {
                        if forward && depth == 0 {
                            done = true;
                        } else {
                            depth += 1;
                        }
                    } else if token == "if" {
                        if !forward && depth == 0 {
                            done = true;
                        } else {
                            depth -= 1;
                        }
                    }
                    done || (token == "else" && depth == 0)
                } else {
                    false
                }
            }
        };
        if complete {
            cur = Pos::new(line, index);
            repeat -= 1;
        }
    }
    if next_ch.is_some() || move_through {
        return Some(Pos::new(line, index));
    }
    Some(cur)
}

/// A text object's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: Pos,
    pub end: Pos,
}

/// `expandWordUnderCursor`.
#[allow(clippy::too_many_arguments)]
pub fn expand_word_under_cursor(
    cm: &Cm,
    inclusive: bool,
    inner_word: bool,
    big_word: bool,
    no_symbol: bool,
    multiline: bool,
    cursor: Option<Pos>,
) -> Option<Span> {
    let cur = cursor.unwrap_or_else(|| get_head(cm));
    let line = cm.line_chars(cur.line);
    let mut end_line = line.clone();
    let start_line_number = cur.line;
    let mut end_line_number = start_line_number;
    let mut idx = cur.ch;
    let mut word_on_next_line = None;
    let mut test: fn(Option<char>) -> bool = if no_symbol { word_char } else { big_word_char };
    if inner_word && is_space(ch_at(&line, idx)) {
        test = is_space;
    } else {
        while !test(ch_at(&line, idx)) {
            idx += 1;
            if idx >= line.len() as i64 {
                if !multiline {
                    return None;
                }
                idx -= 1;
                word_on_next_line = find_word(cm, cur, true, big_word, true);
                break;
            }
        }
        if big_word {
            test = big_word_char;
        } else {
            test = word_char;
            if !test(ch_at(&line, idx)) {
                test = punct_char;
            }
        }
    }
    let mut end = idx;
    let mut start = idx;
    while test(ch_at(&line, start)) && start >= 0 {
        start -= 1;
    }
    start += 1;
    if let Some(w) = word_on_next_line {
        end = w.to;
        end_line_number = w.line;
        end_line = cm.line_chars(end_line_number);
        if end_line.is_empty() && end == 0 {
            end += 1;
        }
    } else {
        while test(ch_at(&line, end)) && end < line.len() as i64 {
            end += 1;
        }
    }
    if inclusive {
        let word_end = end;
        let starts_with_space = cur.ch <= start && is_space(ch_at(&line, cur.ch));
        if !starts_with_space {
            while is_space(ch_at(&end_line, end)) && end < end_line.len() as i64 {
                end += 1;
            }
        }
        if word_end == end || starts_with_space {
            let word_start = start;
            while is_space(ch_at(&line, start - 1)) && start > 0 {
                start -= 1;
            }
            if start == 0 && !starts_with_space {
                start = word_start;
            }
        }
    }
    Some(Span {
        start: Pos::new(start_line_number, start),
        end: Pos::new(end_line_number, end),
    })
}

/// `selectCompanionObject`.
fn select_companion_object(cm: &Cm, head: Pos, symb: char, inclusive: bool) -> Option<Span> {
    let (brackets, open) = match symb {
        '(' | ')' => ("()", '('),
        '[' | ']' => ("[]", '['),
        '{' | '}' => ("{}", '{'),
        _ => ("<>", '<'),
    };
    let cur_char = ch_at(&cm.line_chars(head.line), head.ch);
    let off = if cur_char == Some(open) { 1 } else { 0 };
    let at = Pos::new(head.line, head.ch + off);
    let start = cm.scan_for_bracket(at, -1, brackets).ok().flatten()?;
    let end = cm.scan_for_bracket(at, 1, brackets).ok().flatten()?;
    let (mut s, mut e) = (start.0, end.0);
    if (s.line == e.line && s.ch > e.ch) || s.line > e.line {
        std::mem::swap(&mut s, &mut e);
    }
    if inclusive {
        e.ch += 1;
    } else {
        s.ch += 1;
    }
    Some(Span { start: s, end: e })
}

/// `findBeginningAndEnd`.
fn find_beginning_and_end(cm: &Cm, head: Pos, symb: char, inclusive: bool) -> Span {
    let mut cur = head;
    let chars = cm.line_chars(cur.line);
    let first_index = chars
        .iter()
        .position(|c| *c == symb)
        .map_or(-1, |i| i as i64);
    let mut start: Option<i64> = None;
    let mut end: Option<i64> = None;
    if cur.ch < first_index {
        cur.ch = first_index;
    } else if first_index < cur.ch && ch_at(&chars, cur.ch) == Some(symb) {
        end = Some(cur.ch);
        cur.ch -= 1;
    }
    if ch_at(&chars, cur.ch) == Some(symb) && end.is_none_or(|e| e == 0) {
        start = Some(cur.ch + 1);
    } else {
        let mut i = cur.ch;
        while i > -1 && start.is_none_or(|s| s == 0) {
            if ch_at(&chars, i) == Some(symb) {
                start = Some(i + 1);
            }
            i -= 1;
        }
    }
    if start.is_some_and(|s| s != 0) && end.is_none_or(|e| e == 0) {
        let mut i = start.unwrap_or(0);
        while i < chars.len() as i64 && end.is_none_or(|e| e == 0) {
            if ch_at(&chars, i) == Some(symb) {
                end = Some(i);
            }
            i += 1;
        }
    }
    let (Some(mut s), Some(mut e)) = (start.filter(|s| *s != 0), end.filter(|e| *e != 0)) else {
        return Span {
            start: cur,
            end: cur,
        };
    };
    if inclusive {
        s -= 1;
        e += 1;
    }
    Span {
        start: Pos::new(cur.line, s),
        end: Pos::new(cur.line, e),
    }
}

/// `textObjectManipulation`.
fn text_object(cm: &mut Cm, mut head: Pos, args: &mut MotionArgs) -> Option<MotionResult> {
    let mut character = args.selected_character.clone().unwrap_or_default();
    if character == "b" {
        character = "(".into();
    } else if character == "B" {
        character = "{".into();
    }
    let inclusive = !args.text_object_inner;
    let c = character.chars().next();
    let mut tmp: Option<Span> = None;
    let mut mv = false;
    match c {
        Some(ch) if "(){}[]<>".contains(ch) => {
            mv = true;
            tmp = select_companion_object(cm, head, ch, inclusive);
            if tmp.is_none() {
                let re = super::search::VimRegex::new(&format!("\\{ch}"), "g");
                if let Some(re) = re {
                    let mut sc = cm.get_search_cursor(&re, head);
                    if cm.cursor_find(&mut sc, false)
                        && let Some(f) = sc.from
                    {
                        tmp = select_companion_object(cm, f, ch, inclusive);
                    }
                }
            }
        }
        Some(ch) if "'\"`".contains(ch) => {
            mv = true;
            tmp = Some(find_beginning_and_end(cm, head, ch, inclusive));
        }
        Some('W') | Some('w') => {
            let mut repeat = if args.repeat == 0 { 1 } else { args.repeat };
            let big = c == Some('W');
            while repeat > 0 {
                repeat -= 1;
                let repeated = expand_word_under_cursor(
                    cm,
                    inclusive,
                    !inclusive,
                    big,
                    big,
                    true,
                    tmp.map(|t| t.end),
                );
                if let Some(r) = repeated {
                    let t = tmp.get_or_insert(r);
                    t.end = r.end;
                }
            }
        }
        Some('p') => {
            let (s, mut e) = find_paragraph(cm, head, args.repeat, 0, inclusive);
            args.linewise = true;
            if cm.vim().visual_mode {
                if !cm.vim().visual_line {
                    cm.vim().visual_line = true;
                }
            } else {
                if let Some(oa) = cm.vim().input_state.operator_args.as_mut() {
                    oa.linewise = true;
                }
                e.line -= 1;
            }
            tmp = Some(Span { start: s, end: e });
        }
        Some('t') => {
            tmp = Some(Span {
                start: head,
                end: head,
            });
        }
        Some('s') => {
            let content = cm.line_chars(head.line);
            if head.ch > 0 && is_eos(ch_at(&content, head.ch)) {
                head.ch -= 1;
            }
            let end = get_sentence(cm, head, args.repeat, 1, inclusive);
            let mut start = get_sentence(cm, head, args.repeat, -1, inclusive);
            if is_ws_opt(ch_at(&cm.line_chars(start.line), start.ch))
                && is_ws_opt(ch_at(&cm.line_chars(end.line), end.ch - 1))
            {
                start = Pos::new(start.line, start.ch + 1);
            }
            tmp = Some(Span { start, end });
        }
        _ => {}
    }
    let tmp = tmp?;
    if !cm.vim().visual_mode {
        return Some(MotionResult::Range(tmp.start, tmp.end));
    }
    let (a, h) = expand_selection(cm, tmp.start, tmp.end, mv);
    Some(MotionResult::Range(a, h))
}

/// `isWhiteSpaceString(str[i])`: `undefined` reads as `"undefined"`.
fn is_ws_opt(c: Option<char>) -> bool {
    c.is_some_and(crate::state::is_js_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_index_search() {
        let line: Vec<char> = "a,b,c".chars().collect();
        assert_eq!(char_idx_in_line(0, &line, ",", true), Some(1));
        assert_eq!(char_idx_in_line(1, &line, ",", true), Some(3));
        assert_eq!(char_idx_in_line(4, &line, ",", false), Some(3));
        assert_eq!(char_idx_in_line(1, &line, ",", false), None);
    }
}
