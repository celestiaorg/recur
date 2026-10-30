//! Ex commands (`ExCommandDispatcher` and `exCommands`): `:` input
//! parsing, the command table, user ex mappings and every built-in command,
//! including `:substitute`'s confirm loop (`doReplace`).

use std::cell::RefCell;
use std::rc::Rc;

use super::cm::{Cm, SearchCursor};
use super::helpers::*;
use super::keymap::{Ctx, Kind, VimKey};
use super::prompt::{Prefix, PromptCtl, PromptKind, PromptOptions};
use super::search::{
    VimRegex, clear_search_highlight, split_by_separator, translate_regex_replace,
    unescape_regex_replace, update_search_query,
};
use super::state::{ExCommandDef, ExKind, OptValue, Register, with_global};
use super::types::*;
use super::{clear_input_state, do_key_to_key};
use crate::state::is_js_space;

/// `defaultExCommandMap` plus `version` (`defineEx`), keyed by short name.
pub fn default_ex_command_map() -> Vec<(String, ExCommandDef)> {
    let table: &[(&str, Option<&str>, bool, bool)] = &[
        ("colorscheme", Some("colo"), false, false),
        ("map", None, false, false),
        ("imap", Some("im"), false, false),
        ("nmap", Some("nm"), false, false),
        ("vmap", Some("vm"), false, false),
        ("omap", Some("om"), false, false),
        ("noremap", Some("no"), false, false),
        ("nnoremap", Some("nn"), false, false),
        ("vnoremap", Some("vn"), false, false),
        ("inoremap", Some("ino"), false, false),
        ("onoremap", Some("ono"), false, false),
        ("unmap", None, false, false),
        ("mapclear", Some("mapc"), false, false),
        ("nmapclear", Some("nmapc"), false, false),
        ("vmapclear", Some("vmapc"), false, false),
        ("imapclear", Some("imapc"), false, false),
        ("omapclear", Some("omapc"), false, false),
        ("write", Some("w"), false, false),
        ("undo", Some("u"), false, false),
        ("redo", Some("red"), false, false),
        ("set", Some("se"), false, false),
        ("setlocal", Some("setl"), false, false),
        ("setglobal", Some("setg"), false, false),
        ("sort", Some("sor"), false, false),
        ("substitute", Some("s"), true, false),
        ("startinsert", Some("start"), false, false),
        ("nohlsearch", Some("noh"), false, false),
        ("yank", Some("y"), false, false),
        ("delmarks", Some("delm"), false, false),
        ("marks", None, false, true),
        ("registers", Some("reg"), false, true),
        ("vglobal", Some("v"), false, false),
        ("delete", Some("d"), false, false),
        ("join", Some("j"), false, false),
        ("normal", Some("norm"), false, false),
        ("global", Some("g"), false, false),
        ("version", Some("ve"), false, false),
    ];
    let mut out: Vec<(String, ExCommandDef)> = Vec::new();
    for (name, short, possibly_async, exclude) in table {
        let def = ExCommandDef {
            name: (*name).into(),
            short_name: short.map(Into::into),
            kind: ExKind::Builtin,
            possibly_async: *possibly_async,
            exclude_from_history: *exclude,
            user: false,
        };
        set_command(&mut out, short.unwrap_or(name), def);
    }
    out
}

fn set_command(map: &mut Vec<(String, ExCommandDef)>, key: &str, def: ExCommandDef) {
    match map.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = def,
        None => map.push((key.to_string(), def)),
    }
}

/// What runs after a command finishes (`params.callback`).
#[derive(Clone)]
pub(crate) enum ExCallback {
    /// `:global`'s `nextCommand`.
    Global(Rc<RefCell<GlobalIter>>),
}

pub(crate) struct GlobalIter {
    handles: Vec<usize>,
    index: usize,
    cmd: String,
}

fn run_callback(cm: &mut Cm, cb: &ExCallback) -> Result<(), String> {
    match cb {
        ExCallback::Global(g) => global_next(cm, g),
    }
}

fn global_next(cm: &mut Cm, g: &Rc<RefCell<GlobalIter>>) -> Result<(), String> {
    let next = {
        let mut it = g.borrow_mut();
        if it.index < it.handles.len() {
            let h = it.handles[it.index];
            it.index += 1;
            Some((h, it.cmd.clone()))
        } else {
            None
        }
    };
    match next {
        Some((handle, cmd)) => match cm.line_number(handle) {
            None => global_next(cm, g),
            Some(line) => run_command(
                cm,
                &format!("{}{}", line + 1, cmd),
                Some(ExCallback::Global(g.clone())),
            ),
        },
        None => {
            cm.release_line_handles();
            Ok(())
        }
    }
}

/// Parsed ex input (`ExParams`).
#[derive(Clone, Default)]
pub(crate) struct ExParams {
    pub input: String,
    pub line: Option<i64>,
    pub line_end: Option<i64>,
    pub selection_line: Option<i64>,
    pub selection_line_end: Option<i64>,
    pub command_name: Option<String>,
    pub arg_string: Option<String>,
    pub args: Option<Vec<String>>,
    pub set_cfg: Option<Scope>,
    pub callback: Option<ExCallback>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Local,
    Global,
}

/// `a || b` for a line number (`0` is falsy).
fn or_line(a: Option<i64>, b: i64) -> i64 {
    match a {
        Some(x) if x != 0 => x,
        _ => b,
    }
}

/// `CodeMirror.StringStream` over the input, with JavaScript's handling of
/// a position backed up past the start.
struct Stream {
    s: Vec<char>,
    pos: i64,
}

impl Stream {
    fn new(s: &str) -> Stream {
        Stream {
            s: s.chars().collect(),
            pos: 0,
        }
    }

    fn len(&self) -> i64 {
        self.s.len() as i64
    }

    /// `string.charAt(pos)`.
    fn char_at(&self, pos: i64) -> Option<char> {
        if pos < 0 {
            None
        } else {
            self.s.get(pos as usize).copied()
        }
    }

    /// `string.slice(pos)`.
    fn rest(&self) -> &[char] {
        let start = if self.pos < 0 {
            (self.len() + self.pos).max(0)
        } else {
            self.pos.min(self.len())
        };
        &self.s[start as usize..]
    }

    fn eol(&self) -> bool {
        self.pos >= self.len()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.char_at(self.pos) == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_while(&mut self, c: char) {
        while self.eat(c) {}
    }

    fn eat_space(&mut self) -> bool {
        let start = self.pos;
        while self.char_at(self.pos).is_some_and(is_js_space) {
            self.pos += 1;
        }
        self.pos > start
    }

    /// `next()`: `""` at a negative position, `None` at the end.
    fn next(&mut self) -> Option<String> {
        if self.pos < self.len() {
            let c = self.char_at(self.pos);
            self.pos += 1;
            Some(c.map(String::from).unwrap_or_default())
        } else {
            None
        }
    }

    fn back_up(&mut self, n: i64) {
        self.pos -= n;
    }

    /// `match(/^(\d+)/)`.
    fn digits(&mut self) -> Option<String> {
        let n = self
            .rest()
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if n == 0 {
            return None;
        }
        let d: String = self.rest()[..n].iter().collect();
        self.pos += n as i64;
        Some(d)
    }

    /// `match(/.*/)`: the rest of the line.
    fn line(&mut self) -> String {
        let n = self
            .rest()
            .iter()
            .take_while(|c| !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
            .count();
        let out: String = self.rest()[..n].iter().collect();
        self.pos += n as i64;
        out
    }
}

/// JavaScript `parseInt(s, 10)` on ASCII digits.
fn parse_int(s: &str) -> i64 {
    s.parse::<i64>().unwrap_or(i64::MAX / 4)
}

fn parse_line_spec_offset(stream: &mut Stream, mut line: i64) -> i64 {
    let rest = stream.rest();
    let sign = rest.first().copied().filter(|c| *c == '+' || *c == '-');
    let skip = usize::from(sign.is_some());
    let n = rest[skip..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    if n > 0 {
        let digits: String = rest[skip..skip + n].iter().collect();
        let offset = parse_int(&digits);
        stream.pos += (skip + n) as i64;
        if sign == Some('-') {
            line -= offset;
        } else {
            line += offset;
        }
    }
    line
}

fn parse_line_spec(cm: &mut Cm, stream: &mut Stream) -> Result<Option<i64>, String> {
    if let Some(d) = stream.digits() {
        return Ok(Some(parse_int(&d) - 1));
    }
    match stream.next().as_deref() {
        Some(".") => {
            let l = cm.get_cursor().line;
            Ok(Some(parse_line_spec_offset(stream, l)))
        }
        Some("$") => {
            let l = cm.last_line();
            Ok(Some(parse_line_spec_offset(stream, l)))
        }
        Some("'") => {
            let name = stream.next().unwrap_or_default();
            let Some(pos) = get_mark_pos(cm, &name) else {
                return Err("Error: Mark not set".into());
            };
            Ok(Some(parse_line_spec_offset(stream, pos.line)))
        }
        Some("-") | Some("+") => {
            stream.back_up(1);
            let l = cm.get_cursor().line;
            Ok(Some(parse_line_spec_offset(stream, l)))
        }
        _ => {
            stream.back_up(1);
            Ok(None)
        }
    }
}

/// `parseInput_`.
fn parse_input(cm: &mut Cm, stream: &mut Stream, p: &mut ExParams) -> Result<(), String> {
    stream.eat_while(':');
    if stream.eat('%') {
        p.line = Some(0);
        p.line_end = Some(cm.last_line());
    } else {
        p.line = parse_line_spec(cm, stream)?;
        if p.line.is_some() && stream.eat(',') {
            p.line_end = parse_line_spec(cm, stream)?;
        }
    }
    if p.line.is_none() {
        if cm.vim().visual_mode {
            p.selection_line = get_mark_pos(cm, "<").map(|m| m.line);
            p.selection_line_end = get_mark_pos(cm, ">").map(|m| m.line);
        } else {
            p.selection_line = Some(cm.get_cursor().line);
        }
    } else {
        p.selection_line = p.line;
        p.selection_line_end = p.line_end;
    }
    // /^(\w+|!!|@@|[!#&*<=>@~])/
    let rest = stream.rest();
    let word = rest
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
        .count();
    let n = if word > 0 {
        word
    } else if rest.len() >= 2
        && ((rest[0] == '!' && rest[1] == '!') || (rest[0] == '@' && rest[1] == '@'))
    {
        2
    } else if rest.first().is_some_and(|c| "!#&*<=>@~".contains(*c)) {
        1
    } else {
        0
    };
    if n > 0 {
        p.command_name = Some(rest[..n].iter().collect());
        stream.pos += n as i64;
    } else {
        p.command_name = Some(stream.line());
    }
    Ok(())
}

/// `parseCommandArgs_`.
fn parse_command_args(stream: &mut Stream, p: &mut ExParams) {
    if stream.eol() {
        return;
    }
    let arg = stream.line();
    let args: Vec<String> = arg
        .trim_matches(is_js_space)
        .split(is_js_space)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    let first_ok = arg
        .trim_matches(is_js_space)
        .split(is_js_space)
        .next()
        .is_some_and(|s| !s.is_empty());
    p.arg_string = Some(arg.clone());
    if first_ok {
        // `trim(s).split(/\s+/)`: runs of spaces are one delimiter.
        p.args = Some(args);
    }
}

/// `matchCommand_`.
fn match_command(name: &str) -> Option<ExCommandDef> {
    let chars: Vec<char> = name.chars().collect();
    with_global(|g| {
        for i in (1..=chars.len()).rev() {
            let prefix: String = chars[..i].iter().collect();
            if let Some((_, def)) = g.ex_commands.iter().find(|(k, _)| *k == prefix)
                && def.name.starts_with(name)
            {
                return Some(def.clone());
            }
        }
        None
    })
}

/// `processEx`: `:` opens the prompt.
pub(crate) fn process_ex(cm: &mut Cm) {
    let value = cm.vim().visual_mode.then(|| "'<,'>".to_string());
    super::prompt::show_prompt(
        cm,
        PromptOptions {
            prefix: Prefix::Text(":".into()),
            desc: None,
            value,
            kind: PromptKind::Ex,
        },
    );
}

/// The `:` prompt's `onClose`.
pub(crate) fn ex_prompt_close(cm: &mut Cm, input: &str) {
    with_global(|g| {
        g.ex_history.push_input(input);
        g.ex_history.reset();
    });
    if run_command(cm, input, None).is_err() {
        return;
    }
    if cm.view.vim.is_some() {
        clear_input_state(cm);
    }
    clear_search_highlight(cm);
}

/// The `:` prompt's `onKeyUp`: preview `:s` patterns.
pub(crate) fn ex_prompt_key_up(cm: &mut Cm, query: &str) {
    let mut stream = Stream::new(query);
    let mut p = ExParams::default();
    if parse_input(cm, &mut stream, &mut p).is_err() {
        return;
    }
    if p.command_name.as_deref() != Some("s") {
        clear_search_highlight(cm);
        return;
    }
    if match_command("s").is_none() {
        return;
    }
    parse_command_args(&mut stream, &mut p);
    let Some(arg) = p.arg_string.filter(|a| !a.is_empty()) else {
        return;
    };
    let pattern: String = arg.chars().skip(1).collect();
    if let Ok(Some(re)) = super::search::parse_query(&pattern, true, true) {
        super::search::highlight_search_matches(cm, &re);
    }
}

/// `exCommandDispatcher.processCommand`; `Err` when the command threw.
pub(crate) fn process_command(cm: &mut Cm, input: &str) -> Result<(), ()> {
    run_command(cm, input, None).map_err(|_| ())
}

fn run_command(cm: &mut Cm, input: &str, callback: Option<ExCallback>) -> Result<(), String> {
    cm.op_begin();
    cm.set_vim_op();
    let r = process_command_inner(cm, input, callback);
    cm.op_end();
    r
}

/// `_processCommand`.
fn process_command_inner(
    cm: &mut Cm,
    input: &str,
    callback: Option<ExCallback>,
) -> Result<(), String> {
    let previous = with_global(|g| {
        let reg = g.registers.get(Some(":"));
        let prev = reg.text();
        reg.set_text(input, false, false);
        prev
    });
    let mut stream = Stream::new(input);
    let mut p = ExParams {
        input: input.to_string(),
        callback,
        ..Default::default()
    };
    if let Err(e) = parse_input(cm, &mut stream, &mut p) {
        show_confirm(cm, &e);
        return Err(e);
    }
    if cm.vim().visual_mode {
        exit_visual_mode(cm, true);
    }
    let mut command = None;
    let mut command_name = None;
    match p.command_name.clone().filter(|n| !n.is_empty()) {
        None => {
            if p.line.is_some() {
                command_name = Some("move".to_string());
            }
        }
        Some(name) => {
            if let Some(c) = match_command(&name) {
                command_name = Some(c.name.clone());
                if c.exclude_from_history {
                    with_global(|g| g.registers.get(Some(":")).set_text(&previous, false, false));
                }
                parse_command_args(&mut stream, &mut p);
                match &c.kind {
                    ExKind::ExToKey(keys) => {
                        let from = VimKey {
                            noremap: None,
                            ..VimKey::new(
                                &c.name,
                                Kind::KeyToKey {
                                    to_keys: keys.clone(),
                                },
                            )
                        };
                        do_key_to_key(cm, keys, Some(&from));
                        return Ok(());
                    }
                    ExKind::ExToEx(to) => {
                        return run_command(cm, to, None);
                    }
                    ExKind::Builtin => {}
                }
                command = Some(c);
            }
        }
    }
    let Some(name) = command_name else {
        show_confirm(cm, &format!("Not an editor command \":{input}\""));
        return Ok(());
    };
    let possibly_async = command.as_ref().is_some_and(|c| c.possibly_async);
    let mut r = run_ex(cm, &name, &mut p);
    if r.is_ok()
        && !possibly_async
        && let Some(cb) = p.callback.clone()
    {
        r = run_callback(cm, &cb);
    }
    if let Err(e) = &r {
        show_confirm(cm, e);
    }
    r
}

/// `exCommands[name](cm, params)`; `Err` is the thrown error's text.
fn run_ex(cm: &mut Cm, name: &str, p: &mut ExParams) -> Result<(), String> {
    match name {
        "colorscheme" => {
            if p.args.as_ref().is_none_or(|a| a.is_empty()) {
                show_confirm(cm, "");
            }
            Ok(())
        }
        "map" => map(cm, p, None, false),
        "imap" => map(cm, p, Some(Ctx::Insert), false),
        "nmap" => map(cm, p, Some(Ctx::Normal), false),
        "vmap" => map(cm, p, Some(Ctx::Visual), false),
        "omap" => map(cm, p, Some(Ctx::OperatorPending), false),
        "noremap" => map(cm, p, None, true),
        "inoremap" => map(cm, p, Some(Ctx::Insert), true),
        "nnoremap" => map(cm, p, Some(Ctx::Normal), true),
        "vnoremap" => map(cm, p, Some(Ctx::Visual), true),
        "onoremap" => map(cm, p, Some(Ctx::OperatorPending), true),
        "unmap" => {
            let ok = match p.args.as_ref().and_then(|a| a.first()) {
                Some(lhs) => unmap(lhs, None)?,
                None => false,
            };
            if !ok {
                show_confirm(cm, &format!("No such mapping: {}", p.input));
            }
            Ok(())
        }
        "mapclear" => {
            mapclear(None);
            Ok(())
        }
        "imapclear" => {
            mapclear(Some(Ctx::Insert));
            Ok(())
        }
        "nmapclear" => {
            mapclear(Some(Ctx::Normal));
            Ok(())
        }
        "vmapclear" => {
            mapclear(Some(Ctx::Visual));
            Ok(())
        }
        "omapclear" => {
            mapclear(Some(Ctx::OperatorPending));
            Ok(())
        }
        "move" => {
            let mut key = VimKey::new(
                "",
                Kind::Motion {
                    motion: Motion::MoveToLineOrEdgeOfDocument,
                    args: MotionArgs {
                        forward: Some(false),
                        explicit_repeat: true,
                        linewise: true,
                        ..Default::default()
                    },
                },
            );
            key.repeat_override = p.line.map(|l| l + 1);
            super::dispatch::process_command(cm, &key);
            Ok(())
        }
        "set" => set(cm, p),
        "setlocal" => {
            p.set_cfg = Some(Scope::Local);
            set(cm, p)
        }
        "setglobal" => {
            p.set_cfg = Some(Scope::Global);
            set(cm, p)
        }
        "registers" => {
            registers(cm, p);
            Ok(())
        }
        "marks" => {
            marks(cm, p);
            Ok(())
        }
        "sort" => sort(cm, p),
        "vglobal" | "global" => global(cm, p),
        "normal" => {
            normal(cm, p);
            Ok(())
        }
        "substitute" => substitute(cm, p),
        "startinsert" => {
            let keys = if p.arg_string.as_deref() == Some("!") {
                "A"
            } else {
                "i"
            };
            do_key_to_key(cm, keys, Some(&adhoc_key(true)));
            Ok(())
        }
        "redo" => {
            cm.run_history(false);
            Ok(())
        }
        "undo" => {
            cm.run_history(true);
            Ok(())
        }
        // Recur defines no `CodeMirror.commands.save` and the adapter has
        // no `cm.save`, so `:w` does nothing.
        "write" => Ok(()),
        "nohlsearch" => {
            clear_search_highlight(cm);
            Ok(())
        }
        "yank" => {
            let cur = cm.get_cursor();
            let text = cm.get_line(cur.line);
            let clip = with_global(|g| g.registers.push_text(Some("0"), "yank", &text, true, true));
            if let Some(t) = clip {
                cm.cx
                    .write_to_clipboard(gpui_kit::ClipboardItem::new_string(t));
            }
            Ok(())
        }
        "delete" => {
            let line = p.selection_line.unwrap_or_else(|| cm.get_cursor().line);
            let end = p.selection_line_end.unwrap_or(line);
            let range = Range {
                anchor: Pos::new(line, 0),
                head: Pos::new(end + 1, 0),
            };
            super::operators::run(
                cm,
                Operator::Delete,
                &OperatorArgs {
                    linewise: true,
                    ..Default::default()
                },
                &[range],
                range.anchor,
                None,
            );
            Ok(())
        }
        "join" => {
            let line = p.selection_line.unwrap_or_else(|| cm.get_cursor().line);
            let end = p.selection_line_end.unwrap_or(line);
            cm.set_cursor(Pos::new(line, 0));
            super::actions::join_lines(cm, end - line, false);
            Ok(())
        }
        "delmarks" => {
            delmarks(cm, p);
            Ok(())
        }
        "version" => {
            show_confirm(cm, "Codemirror-vim version: 6.3.0");
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The `{noremap}` object `:normal` and `:startinsert` pass to `doKeyToKey`.
fn adhoc_key(noremap: bool) -> VimKey {
    VimKey {
        noremap: Some(noremap),
        ..VimKey::new("", Kind::Idle)
    }
}

fn map(cm: &mut Cm, p: &ExParams, ctx: Option<Ctx>, noremap: bool) -> Result<(), String> {
    let Some(args) = p.args.as_ref().filter(|a| a.len() >= 2) else {
        show_confirm(cm, &format!("Invalid mapping: {}", p.input));
        return Ok(());
    };
    dispatcher_map(&args[0], &args[1], ctx, noremap)
}

/// `exCommandDispatcher.map`.
pub(crate) fn dispatcher_map(
    lhs: &str,
    rhs: &str,
    ctx: Option<Ctx>,
    noremap: bool,
) -> Result<(), String> {
    if lhs != ":" && lhs.starts_with(':') {
        if ctx.is_some() {
            return Err("Error: Mode not supported for ex mappings".into());
        }
        let name = lhs[1..].to_string();
        let kind = if rhs != ":" && rhs.starts_with(':') {
            ExKind::ExToEx(rhs[1..].to_string())
        } else {
            ExKind::ExToKey(rhs.to_string())
        };
        let def = ExCommandDef {
            name: name.clone(),
            short_name: None,
            kind,
            possibly_async: false,
            exclude_from_history: false,
            user: true,
        };
        with_global(|g| set_command(&mut g.ex_commands, &name, def));
    } else {
        let mut mapping = VimKey::new(
            lhs,
            Kind::KeyToKey {
                to_keys: rhs.into(),
            },
        );
        mapping.noremap = Some(noremap);
        mapping.context = ctx;
        with_global(|g| g.map_command(mapping));
    }
    Ok(())
}

/// `exCommandDispatcher.unmap`.
fn unmap(lhs: &str, ctx: Option<Ctx>) -> Result<bool, String> {
    if lhs != ":" && lhs.starts_with(':') {
        if ctx.is_some() {
            return Err("Error: Mode not supported for ex mappings".into());
        }
        let name = &lhs[1..];
        return Ok(with_global(|g| {
            if let Some(i) = g.ex_commands.iter().position(|(k, d)| k == name && d.user) {
                g.ex_commands.remove(i);
                true
            } else {
                false
            }
        }));
    }
    Ok(with_global(|g| {
        if let Some(i) = g
            .keymap
            .iter()
            .position(|k| k.keys == lhs && k.context == ctx)
        {
            g.keymap.remove(i);
            g.remove_used_keys(lhs);
            true
        } else {
            false
        }
    }))
}

/// `vimApi.mapclear`.
fn mapclear(ctx: Option<Ctx>) {
    with_global(|g| {
        let actual = g.keymap.len() as i64;
        let split = actual - g.default_keymap_len as i64;
        let idx = if split >= 0 {
            split
        } else {
            (actual + split).max(0)
        } as usize;
        let rest = g.keymap.split_off(idx);
        let user = std::mem::replace(&mut g.keymap, rest);
        if let Some(ctx) = ctx {
            for mapping in user.into_iter().rev() {
                if Some(ctx) == mapping.context {
                    continue;
                }
                if mapping.context.is_some() {
                    g.map_command(mapping);
                } else {
                    for c in [Ctx::Normal, Ctx::Insert, Ctx::Visual] {
                        if c != ctx {
                            let mut m = mapping.clone();
                            m.context = Some(c);
                            g.map_command(m);
                        }
                    }
                }
            }
        }
    });
}

// ---- options ----

/// `getOption`; `Ok(None)` is `undefined`.
fn get_option(cm: &mut Cm, name: &str, scope: Option<Scope>) -> Result<Option<OptValue>, String> {
    let Some(canonical) = with_global(|g| g.option_name(name)) else {
        return Err(format!("Unknown option: {name}"));
    };
    match canonical.as_str() {
        "filetype" => Ok(None),
        "textwidth" => {
            let local = cm.st().textwidth.map(OptValue::Num);
            if scope != Some(Scope::Global) && local.is_some() {
                return Ok(local);
            }
            Ok(None)
        }
        "langmap" => {
            let s = with_global(|g| g.langmap.string.clone());
            Ok(Some(OptValue::Str(s)))
        }
        _ => {
            if scope != Some(Scope::Global)
                && let Some(v) = cm.vim().options.get(name).cloned()
            {
                return Ok(Some(v));
            }
            if scope == Some(Scope::Local) {
                return Ok(None);
            }
            Ok(with_global(|g| g.option_value(&canonical)))
        }
    }
}

/// JavaScript `Number(s)` for an option string, rounded (`Math.round`).
fn js_round_number(v: &OptValue) -> Option<f64> {
    let n = match v {
        OptValue::Bool(b) => f64::from(u8::from(*b)),
        OptValue::Num(n) => *n as f64,
        OptValue::Str(s) => {
            let t = s.trim_matches(is_js_space);
            if t.is_empty() {
                0.0
            } else if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                i64::from_str_radix(h, 16)
                    .map(|n| n as f64)
                    .unwrap_or(f64::NAN)
            } else if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
                i64::from_str_radix(o, 8)
                    .map(|n| n as f64)
                    .unwrap_or(f64::NAN)
            } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
                i64::from_str_radix(b, 2)
                    .map(|n| n as f64)
                    .unwrap_or(f64::NAN)
            } else {
                match t {
                    "Infinity" | "+Infinity" => f64::INFINITY,
                    "-Infinity" => f64::NEG_INFINITY,
                    _ if t.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c)) => {
                        t.parse::<f64>().unwrap_or(f64::NAN)
                    }
                    _ => f64::NAN,
                }
            }
        }
    };
    if n.is_nan() {
        None
    } else {
        Some((n + 0.5).floor())
    }
}

/// `setOption`.
fn set_option(
    cm: &mut Cm,
    name: &str,
    value: Option<OptValue>,
    scope: Option<Scope>,
) -> Result<(), String> {
    let Some(canonical) = with_global(|g| g.option_name(name)) else {
        return Err(format!("Unknown option: {name}"));
    };
    let kind = with_global(|g| g.options.get(&canonical).map(|o| o.kind)).unwrap_or("string");
    let mut value = value;
    if kind == "boolean" {
        let truthy_non_bool = match &value {
            Some(OptValue::Str(s)) => !s.is_empty(),
            Some(OptValue::Num(n)) => *n != 0,
            _ => false,
        };
        if truthy_non_bool {
            let shown = match &value {
                Some(OptValue::Str(s)) => s.clone(),
                Some(OptValue::Num(n)) => n.to_string(),
                _ => String::new(),
            };
            return Err(format!("Invalid argument: {name}={shown}"));
        }
        if value != Some(OptValue::Bool(false)) {
            value = Some(OptValue::Bool(true));
        }
    }
    match canonical.as_str() {
        "filetype" => {}
        "textwidth" => {
            if scope != Some(Scope::Global)
                && let Some(n) = value.as_ref().and_then(js_round_number)
                && n > 1.0
            {
                cm.st().textwidth = Some(if n.is_infinite() { INF } else { n as i64 });
            }
        }
        "langmap" => {
            let s = match &value {
                Some(OptValue::Str(s)) => s.clone(),
                _ => String::new(),
            };
            with_global(|g| {
                if g.langmap.string != s {
                    g.langmap = super::parse_langmap(&s);
                }
                g.langmap.remap_ctrl = None;
            });
        }
        _ => {
            if scope != Some(Scope::Local) {
                let v = value.clone();
                with_global(|g| {
                    if let Some(o) = g.options.get_mut(&canonical) {
                        o.value = v;
                    }
                });
            }
            if scope != Some(Scope::Global)
                && let Some(v) = value
            {
                cm.vim().options.insert(name.to_string(), v);
            }
        }
    }
    Ok(())
}

fn js_string(v: &Option<OptValue>) -> String {
    match v {
        None => "undefined".into(),
        Some(OptValue::Bool(b)) => b.to_string(),
        Some(OptValue::Num(n)) => n.to_string(),
        Some(OptValue::Str(s)) => s.clone(),
    }
}

/// `:set`.
fn set(cm: &mut Cm, p: &ExParams) -> Result<(), String> {
    let scope = p.set_cfg;
    let Some(arg) = p.args.as_ref().and_then(|a| a.first()) else {
        show_confirm(cm, &format!("Invalid mapping: {}", p.input));
        return Ok(());
    };
    let mut parts = arg.split('=');
    let mut option_name = parts.next().unwrap_or_default().to_string();
    let rest: Vec<&str> = parts.collect();
    let mut value: Option<OptValue> = (!rest.is_empty()).then(|| OptValue::Str(rest.join("=")));
    let mut force_get = false;
    let mut force_toggle = false;
    if option_name.ends_with('?') {
        if value
            .as_ref()
            .is_some_and(|v| !matches!(v, OptValue::Str(s) if s.is_empty()))
        {
            return Err(format!(
                "Error: Trailing characters: {}",
                p.arg_string.clone().unwrap_or_default()
            ));
        }
        option_name.pop();
        force_get = true;
    } else if option_name.ends_with('!') {
        option_name.pop();
        force_toggle = true;
    }
    if value.is_none() && option_name.starts_with("no") {
        option_name = option_name[2..].to_string();
        value = Some(OptValue::Bool(false));
    }
    let canonical = with_global(|g| g.option_name(&option_name));
    let is_boolean = canonical
        .as_ref()
        .is_some_and(|c| with_global(|g| g.options.get(c).is_some_and(|o| o.kind == "boolean")));
    if is_boolean {
        if force_toggle {
            let cur = get_option(cm, &option_name, scope);
            let truthy = matches!(cur, Ok(Some(OptValue::Bool(true))))
                || matches!(&cur, Ok(Some(OptValue::Str(s))) if !s.is_empty())
                || matches!(cur, Ok(Some(OptValue::Num(n))) if n != 0);
            value = Some(OptValue::Bool(!truthy));
        } else if value.is_none() {
            value = Some(OptValue::Bool(true));
        }
    }
    if (!is_boolean && value.is_none()) || force_get {
        match get_option(cm, &option_name, scope) {
            Err(e) => show_confirm(cm, &e),
            Ok(Some(OptValue::Bool(b))) => show_confirm(
                cm,
                &format!(" {}{}", if b { "" } else { "no" }, option_name),
            ),
            Ok(v) => show_confirm(cm, &format!("  {}={}", option_name, js_string(&v))),
        }
    } else if let Err(e) = set_option(cm, &option_name, value, scope) {
        show_confirm(cm, &e);
    }
    Ok(())
}

// ---- listings ----

/// A JavaScript object's own-key order: array-index keys ascending, then
/// the rest in insertion order.
pub(crate) fn js_key_order<'a, V>(
    entries: impl Iterator<Item = (&'a String, V)>,
) -> Vec<(&'a String, V)> {
    let is_index = |k: &str| {
        !k.is_empty()
            && k.chars().all(|c| c.is_ascii_digit())
            && (k == "0" || !k.starts_with('0'))
            && k.parse::<u64>().is_ok_and(|n| n < u32::MAX as u64)
    };
    let (mut idx, rest): (Vec<_>, Vec<_>) = entries.partition(|(k, _)| is_index(k));
    idx.sort_by_key(|(k, _)| k.parse::<u64>().unwrap_or(0));
    idx.extend(rest);
    idx
}

/// `:registers`.
fn registers(cm: &mut Cm, p: &ExParams) {
    let mut info = String::from("----------Registers----------\n\n");
    with_global(|g| match p.args.as_ref() {
        None => {
            for (name, reg) in js_key_order(g.registers.registers.iter().map(|(k, v)| (k, v))) {
                let text = reg.text();
                if !text.is_empty() {
                    info.push_str(&format!("\"{name}    {text}\n"));
                }
            }
        }
        Some(args) => {
            for c in args.join("").chars() {
                let name = c.to_string();
                if !g.registers.is_valid(Some(&name)) {
                    continue;
                }
                let text = g
                    .registers
                    .registers
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, r)| r.text())
                    .unwrap_or_else(|| Register::default().text());
                info.push_str(&format!("\"{name}    {text}\n"));
            }
        }
    });
    show_confirm_long(cm, &info);
}

fn show_confirm_long(cm: &mut Cm, text: &str) {
    super::prompt::show_confirm(cm, text, true);
}

/// `:marks`.
fn marks(cm: &mut Cm, p: &ExParams) {
    let mut info = String::from("-----------Marks-----------\nmark\tline\tcol\n\n");
    let all: Vec<(String, u64)> = cm
        .vim()
        .marks
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let names: Vec<String> = match p.args.as_ref() {
        None => all.iter().map(|(k, _)| k.clone()).collect(),
        Some(args) => args.join("").chars().map(String::from).collect(),
    };
    for name in names {
        let Some(id) = all.iter().find(|(k, _)| *k == name).map(|(_, v)| *v) else {
            continue;
        };
        if let Some(m) = cm.mark_find(id) {
            info.push_str(&format!("{}\t{}\t{}\n", name, m.line, m.ch));
        }
    }
    show_confirm_long(cm, &info);
}

// ---- :sort ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortNumber {
    Decimal,
    Hex,
    Octal,
}

/// `numberRegex.exec(s)`: the sign and digits.
fn number_match(kind: SortNumber, s: &str) -> Option<(String, String)> {
    let chars: Vec<char> = s.chars().collect();
    for start in 0..chars.len() {
        let mut i = start;
        let mut sign = String::new();
        if kind != SortNumber::Octal && chars[i] == '-' {
            sign.push('-');
            i += 1;
        }
        if kind == SortNumber::Hex
            && i + 1 < chars.len()
            && chars[i] == '0'
            && (chars[i + 1] == 'x' || chars[i + 1] == 'X')
        {
            let j = i + 2;
            let n = chars[j..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            if n > 0 {
                return Some((sign, chars[j..j + n].iter().collect()));
            }
        }
        let ok = |c: &char| match kind {
            SortNumber::Decimal => c.is_ascii_digit(),
            SortNumber::Hex => c.is_ascii_hexdigit(),
            SortNumber::Octal => ('0'..='7').contains(c),
        };
        let n = chars[i..].iter().take_while(|c| ok(c)).count();
        if n > 0 {
            let digits: String = chars[i..i + n].iter().collect();
            // Octal has no sign group: `amatch[1] + amatch[2]` appends
            // "undefined", which `parseInt` stops at.
            return Some((sign, digits));
        }
    }
    None
}

fn sort_value(kind: SortNumber, sign: &str, digits: &str) -> f64 {
    let radix = match kind {
        SortNumber::Decimal => 10,
        SortNumber::Hex => 16,
        SortNumber::Octal => 8,
    };
    let v = i128::from_str_radix(&digits.to_lowercase(), radix)
        .map(|n| n as f64)
        .unwrap_or(f64::INFINITY);
    if sign == "-" { -v } else { v }
}

/// JavaScript `<` on strings (UTF-16 code units).
fn js_less(a: &str, b: &str) -> bool {
    a.encode_utf16().lt(b.encode_utf16())
}

enum SortItem {
    Match { key: String, input: String },
    Line(String),
}

fn sort(cm: &mut Cm, p: &ExParams) -> Result<(), String> {
    let mut reverse = false;
    let mut ignore_case = false;
    let mut unique = false;
    let mut number: Option<SortNumber> = None;
    let mut pattern: Option<VimRegex> = None;
    let mut err: Option<&str> = None;
    if let Some(arg) = p.arg_string.as_deref().filter(|a| !a.is_empty()) {
        let mut s = Stream::new(arg);
        if s.eat('!') {
            reverse = true;
        }
        if !s.eol() {
            if !s.eat_space() {
                err = Some("Invalid arguments");
            } else {
                // /([dinuox]+)?\s*(\/.+\/)?\s*/
                let rest: Vec<char> = s.rest().to_vec();
                let fl = rest.iter().take_while(|c| "dinuox".contains(**c)).count();
                let flags: String = rest[..fl].iter().collect();
                let mut i = fl;
                while i < rest.len() && is_js_space(rest[i]) {
                    i += 1;
                }
                let mut pat: Option<String> = None;
                if i < rest.len() && rest[i] == '/' {
                    let tail = &rest[i..];
                    let ends: Vec<usize> = tail
                        .iter()
                        .enumerate()
                        .filter(|(j, c)| **c == '/' && *j >= 2)
                        .map(|(j, _)| j)
                        .collect();
                    let pick = ends
                        .iter()
                        .rev()
                        .find(|&&j| tail[j + 1..].iter().all(|c| is_js_space(*c)))
                        .or(ends.last());
                    if let Some(&j) = pick {
                        pat = Some(tail[1..j].iter().collect());
                        i += j + 1;
                    }
                }
                while i < rest.len() && is_js_space(rest[i]) {
                    i += 1;
                }
                if i < rest.len() {
                    err = Some("Invalid arguments");
                } else {
                    if !flags.is_empty() {
                        ignore_case = flags.contains('i');
                        unique = flags.contains('u');
                        let decimal = flags.contains('d') || flags.contains('n');
                        let hex = flags.contains('x');
                        let octal = flags.contains('o');
                        if u8::from(decimal) + u8::from(hex) + u8::from(octal) > 1 {
                            err = Some("Invalid arguments");
                        }
                        number = if decimal {
                            Some(SortNumber::Decimal)
                        } else if hex {
                            Some(SortNumber::Hex)
                        } else if octal {
                            Some(SortNumber::Octal)
                        } else {
                            None
                        };
                    }
                    if err.is_none()
                        && let Some(src) = pat
                    {
                        let flags = if ignore_case { "i" } else { "" };
                        match VimRegex::new(&src, flags) {
                            Some(re) => pattern = Some(re),
                            None => {
                                return Err(format!(
                                    "SyntaxError: Invalid regular expression: /{src}/{flags}"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    if let Some(e) = err {
        show_confirm(
            cm,
            &format!("{e}: {}", p.arg_string.clone().unwrap_or_default()),
        );
        return Ok(());
    }
    let line_start = or_line(p.line, 0);
    let line_end = or_line(p.line_end, or_line(p.line, cm.last_line()));
    if line_start == line_end {
        return Ok(());
    }
    let cur_start = Pos::new(line_start, 0);
    let cur_end = Pos::new(line_end, cm.line_len(line_end));
    let text: Vec<String> = cm
        .get_range(cur_start, cur_end)
        .split('\n')
        .map(String::from)
        .collect();
    let mut num_part: Vec<SortItem> = Vec::new();
    let mut text_part: Vec<String> = Vec::new();
    if number.is_some() || pattern.is_some() {
        for line in text {
            let m = pattern
                .as_ref()
                .and_then(|re| re.re.exec_at(&line, 0))
                .and_then(|m| m.first().copied().flatten())
                .map(|(f, t)| line[f..t].to_string());
            if let Some(key) = m.filter(|k| !k.is_empty()) {
                num_part.push(SortItem::Match { key, input: line });
            } else if number.is_some_and(|k| number_match(k, &line).is_some()) {
                num_part.push(SortItem::Line(line));
            } else {
                text_part.push(line);
            }
        }
    } else {
        text_part = text;
    }
    let compare = |a: &str, b: &str| -> std::cmp::Ordering {
        let (a, b) = if reverse { (b, a) } else { (a, b) };
        let (a, b) = if ignore_case {
            (a.to_lowercase(), b.to_lowercase())
        } else {
            (a.to_string(), b.to_string())
        };
        let am = number.and_then(|k| number_match(k, &a));
        let bm = number.and_then(|k| number_match(k, &b));
        match (am, bm, number) {
            (Some((sa, da)), Some((sb, db)), Some(k)) => {
                let d = sort_value(k, &sa, &da) - sort_value(k, &sb, &db);
                d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            }
            _ => {
                if a == b {
                    std::cmp::Ordering::Equal
                } else if js_less(&a, &b) {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
        }
    };
    let item_key = |i: &SortItem| -> String {
        match i {
            SortItem::Match { key, .. } => key.clone(),
            SortItem::Line(l) => l.chars().next().map(String::from).unwrap_or_default(),
        }
    };
    if pattern.is_some() {
        num_part.sort_by(|a, b| {
            let (a, b) = if reverse { (b, a) } else { (a, b) };
            let (ka, kb) = (item_key(a), item_key(b));
            let (ka, kb) = if ignore_case {
                (ka.to_lowercase(), kb.to_lowercase())
            } else {
                (ka, kb)
            };
            if ka == kb {
                std::cmp::Ordering::Equal
            } else if js_less(&ka, &kb) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });
    } else {
        num_part.sort_by(|a, b| {
            let line = |i: &SortItem| match i {
                SortItem::Match { input, .. } => input.clone(),
                SortItem::Line(l) => l.clone(),
            };
            compare(&line(a), &line(b))
        });
    }
    let nums: Vec<String> = num_part
        .into_iter()
        .map(|i| match i {
            SortItem::Match { input, .. } => input,
            SortItem::Line(l) => {
                if pattern.is_some() {
                    String::new()
                } else {
                    l
                }
            }
        })
        .collect();
    if pattern.is_none() && number.is_none() {
        text_part.sort_by(|a, b| compare(a, b));
    }
    let mut out: Vec<String> = if !reverse {
        text_part.into_iter().chain(nums).collect()
    } else {
        nums.into_iter().chain(text_part).collect()
    };
    if unique {
        let old = std::mem::take(&mut out);
        let mut last: Option<String> = None;
        for l in old {
            if last.as_ref() != Some(&l) {
                out.push(l.clone());
            }
            last = Some(l);
        }
    }
    cm.replace_range(&out.join("\n"), cur_start, Some(cur_end));
    Ok(())
}

// ---- :global / :normal ----

fn global(cm: &mut Cm, p: &ExParams) -> Result<(), String> {
    let Some(mut arg) = p.arg_string.clone().filter(|a| !a.is_empty()) else {
        show_confirm(cm, "Regular Expression missing from global");
        return Ok(());
    };
    let command_name = p.command_name.clone().unwrap_or_default();
    let mut inverted = command_name.starts_with('v');
    if arg.starts_with('!') && command_name.starts_with('g') {
        inverted = true;
        arg = arg[1..].to_string();
    }
    let line_start = p.line.unwrap_or(0);
    let line_end = or_line(p.line_end, or_line(p.line, cm.last_line()));
    let tokens = split_by_separator(&arg, '/');
    let (regex_part, cmd) = match tokens {
        Some(t) if !t.is_empty() => (t[0].clone(), t[1..].join("/")),
        _ => (arg.clone(), String::new()),
    };
    if !regex_part.is_empty() && update_search_query(cm, &regex_part, true, true).is_err() {
        show_confirm(cm, &format!("Invalid regex: {regex_part}"));
        return Ok(());
    }
    let Some(query) = with_global(|g| g.query.clone()) else {
        return Err("TypeError: undefined is not an object (evaluating 'query.test')".into());
    };
    let mut lines: Vec<String> = Vec::new();
    let mut handles: Vec<usize> = Vec::new();
    for i in line_start..=line_end {
        let line = cm.get_line(i);
        if query.test(&line) != inverted {
            if cmd.is_empty() {
                lines.push(line);
            } else {
                handles.push(cm.line_handle(i));
            }
        }
    }
    if cmd.is_empty() {
        show_confirm(cm, &lines.join("\n"));
        return Ok(());
    }
    let it = Rc::new(RefCell::new(GlobalIter {
        handles,
        index: 0,
        cmd,
    }));
    global_next(cm, &it)
}

fn normal(cm: &mut Cm, p: &ExParams) {
    let mut noremap = false;
    let mut arg = p.arg_string.clone().unwrap_or_default();
    if arg.starts_with('!') {
        arg = arg[1..].to_string();
        noremap = true;
    }
    let arg = arg.trim_start_matches(is_js_space).to_string();
    if arg.is_empty() {
        show_confirm(cm, "Argument is required.");
        return;
    }
    // `params.argString.trimStart()`: the keys keep a leading `!`.
    let keys = p
        .arg_string
        .clone()
        .unwrap_or_default()
        .trim_start_matches(is_js_space)
        .to_string();
    match p.line {
        Some(line) => {
            let end = p.line_end.unwrap_or(line);
            let mut i = line;
            while i <= end {
                cm.set_cursor(Pos::new(i, 0));
                do_key_to_key(cm, &keys, Some(&adhoc_key(noremap)));
                if cm.vim().insert_mode {
                    super::insert::exit_insert_mode(cm, true);
                }
                i += 1;
            }
        }
        None => {
            do_key_to_key(cm, &keys, Some(&adhoc_key(noremap)));
            if cm.vim().insert_mode {
                super::insert::exit_insert_mode(cm, true);
            }
        }
    }
}

fn delmarks(cm: &mut Cm, p: &ExParams) {
    let arg = p.arg_string.clone().unwrap_or_default();
    let trimmed = arg.trim_matches(is_js_space).to_string();
    if trimmed.is_empty() {
        show_confirm(cm, "Argument required");
        return;
    }
    let tail = |count: i64| -> String { arg.chars().skip(count.max(0) as usize).collect() };
    let mut s = Stream::new(&trimmed);
    while !s.eol() {
        s.eat_space();
        let count = s.pos;
        if !s.char_at(s.pos).is_some_and(|c| c.is_ascii_alphabetic()) {
            show_confirm(cm, &format!("Invalid argument: {}", tail(count)));
            return;
        }
        let sym = s.next().unwrap_or_default();
        if s.eat('-') {
            if !s.char_at(s.pos).is_some_and(|c| c.is_ascii_alphabetic()) {
                show_confirm(cm, &format!("Invalid argument: {}", tail(count)));
                return;
            }
            let finish = s.next().unwrap_or_default();
            let is_lower = |m: &str| m.len() == 1 && m.chars().all(|c| c.is_ascii_lowercase());
            if !sym.is_empty() && !finish.is_empty() && is_lower(&sym) == is_lower(&finish) {
                let (start, end) = (
                    sym.chars().next().unwrap() as u32,
                    finish.chars().next().unwrap() as u32,
                );
                if start >= end {
                    show_confirm(cm, &format!("Invalid argument: {}", tail(count)));
                    return;
                }
                for c in start..=end {
                    if let Some(ch) = char::from_u32(c) {
                        cm.vim().marks.remove(&ch.to_string());
                    }
                }
            } else {
                show_confirm(cm, &format!("Invalid argument: {sym}-"));
                return;
            }
        } else if !sym.is_empty() {
            cm.vim().marks.remove(&sym);
        }
    }
}

// ---- :substitute ----

fn escape_unescaped_amp(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if i + 1 < chars.len() && chars[i] != '\\' && chars[i + 1] == '&' {
            out.push(chars[i]);
            out.push_str("$&");
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn substitute(cm: &mut Cm, p: &ExParams) -> Result<(), String> {
    let arg = p.arg_string.clone().unwrap_or_default();
    let tokens = if arg.is_empty() {
        Some(Vec::new())
    } else {
        split_by_separator(&arg, arg.chars().next().unwrap())
    };
    let mut regex_part = String::new();
    let mut replace_part: Option<String> = None;
    let mut trailing: Option<Vec<String>> = None;
    let mut confirm = false;
    let mut global = false;
    let pcre = with_global(|g| g.pcre());
    match tokens.as_ref() {
        Some(t) if !t.is_empty() => {
            regex_part = t[0].clone();
            if pcre && !regex_part.is_empty() {
                match VimRegex::new(&regex_part, "") {
                    Some(re) => regex_part = re.source.clone(),
                    None => {
                        return Err(format!(
                            "SyntaxError: Invalid regular expression: /{regex_part}/"
                        ));
                    }
                }
            }
            if let Some(r) = t.get(1) {
                let r = if pcre {
                    unescape_regex_replace(&escape_unescaped_amp(r))
                } else {
                    translate_regex_replace(r)
                };
                with_global(|g| g.last_substitute_replace_part = Some(r.clone()));
                replace_part = Some(r);
            }
            trailing = Some(
                t.get(2)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.split(' ').map(String::from).collect())
                    .unwrap_or_default(),
            );
        }
        _ => {
            if !arg.is_empty() {
                show_confirm(
                    cm,
                    "Substitutions should be of the form :s/pattern/replace/",
                );
                return Ok(());
            }
        }
    }
    let mut count: i64 = 0;
    if let Some(trailing) = trailing.as_ref() {
        let flags = trailing.first().cloned().unwrap_or_default();
        count = trailing.get(1).map(|s| js_parse_int(s)).unwrap_or(0);
        if !flags.is_empty() {
            confirm = flags.contains('c');
            global = flags.contains('g');
            regex_part = if pcre {
                format!("{regex_part}/{flags}")
            } else {
                format!("{}/{}", regex_part.replace('/', "\\/"), flags)
            };
        }
    }
    if !regex_part.is_empty() && update_search_query(cm, &regex_part, true, true).is_err() {
        show_confirm(cm, &format!("Invalid regex: {regex_part}"));
        return Ok(());
    }
    let replace_part = match replace_part.filter(|r| !r.is_empty()) {
        Some(r) => Some(r),
        None => with_global(|g| g.last_substitute_replace_part.clone()),
    };
    let Some(replace_with) = replace_part else {
        show_confirm(cm, "No previous substitute regular expression");
        return Ok(());
    };
    let Some(query) = with_global(|g| g.query.clone()) else {
        return Err("TypeError: undefined is not an object (evaluating 'query.source')".into());
    };
    let mut line_start = p.line.unwrap_or_else(|| cm.get_cursor().line);
    let mut line_end = or_line(p.line_end, line_start);
    if line_start == 0 && line_end == cm.last_line() {
        line_end = INF;
    }
    if count != 0 {
        line_start = line_end;
        line_end = line_start + count - 1;
    }
    let start = clip_cursor_to_content(cm, Pos::new(line_start, 0), None);
    let cursor = cm.get_search_cursor(&query, start);
    do_replace(
        cm,
        Replace {
            confirm,
            global,
            line_start,
            line_end,
            cursor,
            query,
            replace_with,
            callback: p.callback.clone(),
            done: false,
            matches: 0,
            last_pos: None,
            modified_line_number: None,
            joined: false,
        },
    )
}

/// `parseInt(s)`: leading sign and decimal digits; `0` for `NaN`.
fn js_parse_int(s: &str) -> i64 {
    let t = s.trim_start_matches(is_js_space);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let d: String = digits.chars().take_while(|c| c.is_ascii_digit()).collect();
    let n = d.parse::<i64>().unwrap_or(0);
    if neg { -n } else { n }
}

/// `doReplace`'s state, kept across the confirm prompt's keys.
pub(crate) struct Replace {
    confirm: bool,
    global: bool,
    line_start: i64,
    line_end: i64,
    cursor: SearchCursor,
    query: VimRegex,
    replace_with: String,
    callback: Option<ExCallback>,
    done: bool,
    matches: usize,
    last_pos: Option<Pos>,
    modified_line_number: Option<i64>,
    joined: bool,
}

/// `replaceWith.replace(/\$(\d{1,3}|[$&])/g, ...)` against a match.
fn expand_replacement(template: &str, m: &[Option<String>]) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            let n = chars[i + 1..]
                .iter()
                .take(3)
                .take_while(|c| c.is_ascii_digit())
                .count();
            if n > 0 {
                let x: String = chars[i + 1..i + 1 + n].iter().collect();
                let mut x1 = x.clone();
                while !x1.is_empty() && x1.parse::<usize>().unwrap_or(usize::MAX) >= m.len() {
                    x1.pop();
                }
                if x1.is_empty() {
                    out.push('$');
                    out.push_str(&x);
                } else {
                    let canonical = x1 == "0" || !x1.starts_with('0');
                    let group = if canonical {
                        x1.parse::<usize>()
                            .ok()
                            .and_then(|k| m.get(k).cloned().flatten())
                    } else {
                        None
                    };
                    out.push_str(&group.unwrap_or_else(|| "undefined".into()));
                    out.push_str(&x[x1.len()..]);
                }
                i += 1 + n;
                continue;
            }
            match chars[i + 1] {
                '$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                '&' => {
                    out.push_str(m.first().cloned().flatten().as_deref().unwrap_or(""));
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

impl Replace {
    fn replace(&mut self, cm: &mut Cm) {
        let m = self.cursor.matched.clone().unwrap_or_default();
        let new_text = expand_replacement(&self.replace_with, &m);
        let unmodified = self.cursor.to.map_or(0, |p| p.line);
        cm.cursor_replace(&mut self.cursor, &new_text);
        let modified = self.cursor.to.map_or(0, |p| p.line);
        self.modified_line_number = Some(modified);
        self.line_end = self.line_end.saturating_add(modified - unmodified);
        self.joined = modified < unmodified;
    }

    fn find_next_valid_match(&mut self, cm: &mut Cm) -> bool {
        let last_match_to = self.last_pos.and(self.cursor.to);
        let mut found = cm.cursor_find(&mut self.cursor, false);
        if found
            && Cm::cursor_empty_match(&self.cursor)
            && last_match_to.is_some()
            && self.cursor.from == last_match_to
        {
            found = cm.cursor_find(&mut self.cursor, false);
        }
        if found {
            self.matches += 1;
        }
        found
    }

    fn next(&mut self, cm: &mut Cm) {
        while self.find_next_valid_match(cm) {
            let Some(from) = self.cursor.from else { break };
            if !(from.line >= self.line_start && from.line <= self.line_end) {
                break;
            }
            if !self.global && Some(from.line) == self.modified_line_number && !self.joined {
                continue;
            }
            let to = self.cursor.to.unwrap_or(from);
            cm.scroll_into_view(Some(from));
            cm.set_selection(from, to);
            self.last_pos = Some(from);
            self.done = false;
            return;
        }
        self.done = true;
    }

    fn replace_all(&mut self, cm: &mut Cm) -> Result<(), String> {
        cm.op_begin();
        while !self.done {
            self.replace(cm);
            self.next(cm);
        }
        let r = self.stop(cm, None);
        cm.op_end();
        r
    }

    /// `stop(close)`.
    fn stop(&mut self, cm: &mut Cm, close: Option<&mut PromptCtl>) -> Result<(), String> {
        if let Some(ctl) = close {
            ctl.close(cm);
        }
        cm.focus();
        if let Some(lp) = self.last_pos {
            cm.set_cursor(lp);
            let vim = cm.vim();
            vim.ex_mode = false;
            vim.last_h_pos = lp.ch;
            vim.last_hs_pos = lp.ch as f32;
        }
        if let Some(cb) = self.callback.clone() {
            run_callback(cm, &cb)?;
        } else if self.done {
            let pcre = with_global(|g| g.pcre());
            let text = format!(
                "{} for pattern: {}{}",
                if self.matches > 0 {
                    format!("Found {} matches", self.matches)
                } else {
                    "No matches found".into()
                },
                self.query.display(),
                if pcre {
                    " (set nopcre to use Vim regexps)"
                } else {
                    ""
                }
            );
            show_confirm(cm, &text);
        }
        Ok(())
    }

    /// The confirm prompt's `onKeyDown` (it swallows every key).
    pub(crate) fn key(&mut self, cm: &mut Cm, key: Option<&str>, ctl: &mut PromptCtl) {
        match key {
            Some("y") => {
                self.replace(cm);
                self.next(cm);
            }
            Some("n") => self.next(cm),
            Some("a") => {
                let saved = self.callback.take();
                cm.op_begin();
                let _ = self.replace_all(cm);
                cm.op_end();
                self.callback = saved;
            }
            Some("l") => {
                self.replace(cm);
                let _ = self.stop(cm, Some(ctl));
            }
            Some("q") | Some("<Esc>") | Some("<C-c>") | Some("<C-[>") => {
                let _ = self.stop(cm, Some(ctl));
            }
            _ => {}
        }
        if self.done {
            let _ = self.stop(cm, Some(ctl));
        }
    }
}

fn do_replace(cm: &mut Cm, mut r: Replace) -> Result<(), String> {
    cm.vim().ex_mode = true;
    r.next(cm);
    if r.done {
        let pcre = with_global(|g| g.pcre());
        show_confirm(
            cm,
            &format!(
                "No matches for {}{}",
                r.query.display(),
                if pcre {
                    " (set nopcre to use vim regexps)"
                } else {
                    ""
                }
            ),
        );
        return Ok(());
    }
    if !r.confirm {
        r.replace_all(cm)?;
        if let Some(cb) = r.callback.clone() {
            run_callback(cm, &cb)?;
        }
        return Ok(());
    }
    let with = r.replace_with.clone();
    super::prompt::show_prompt(
        cm,
        PromptOptions {
            prefix: Prefix::Replace(with),
            desc: None,
            value: None,
            kind: PromptKind::Replace(Box::new(r)),
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_table_matches_prefixes() {
        crate::vim::state::reset_global();
        assert_eq!(
            match_command("s").map(|c| c.name),
            Some("substitute".into())
        );
        assert_eq!(
            match_command("sub").map(|c| c.name),
            Some("substitute".into())
        );
        assert_eq!(match_command("se").map(|c| c.name), Some("set".into()));
        assert_eq!(
            match_command("setl").map(|c| c.name),
            Some("setlocal".into())
        );
        assert_eq!(match_command("norm").map(|c| c.name), Some("normal".into()));
        assert_eq!(match_command("ve").map(|c| c.name), Some("version".into()));
        assert_eq!(match_command("foo").map(|c| c.name), None);
        assert_eq!(match_command("subx").map(|c| c.name), None);
    }

    #[test]
    fn replacement_templates() {
        let m = vec![Some("ab".to_string()), Some("a".to_string()), None];
        assert_eq!(expand_replacement("[$1]", &m), "[a]");
        assert_eq!(expand_replacement("$&-$$", &m), "ab-$");
        assert_eq!(expand_replacement("$2", &m), "undefined");
        assert_eq!(expand_replacement("$12", &m), "a2");
        assert_eq!(expand_replacement("$9", &m), "$9");
        assert_eq!(escape_unescaped_amp("a&b\\&&"), "a$&b\\&$&");
    }

    #[test]
    fn js_key_order_puts_indices_first() {
        let entries = [
            ("\"".to_string(), 1),
            ("a".to_string(), 2),
            ("1".to_string(), 3),
            ("0".to_string(), 4),
        ];
        let order: Vec<&str> = js_key_order(entries.iter().map(|(k, v)| (k, *v)))
            .into_iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(order, vec!["0", "1", "\"", "a"]);
    }

    #[test]
    fn sort_number_matching() {
        assert_eq!(
            number_match(SortNumber::Decimal, "x -12y"),
            Some(("-".into(), "12".into()))
        );
        assert_eq!(
            number_match(SortNumber::Hex, "0x1F"),
            Some((String::new(), "1F".into()))
        );
        assert_eq!(sort_value(SortNumber::Hex, "-", "ff"), -255.0);
        assert_eq!(js_parse_int("3x"), 3);
        assert_eq!(or_line(Some(0), 7), 7);
    }
}
