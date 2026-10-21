//! regexp2's parser (.NET regex syntax) and code writer.
//!
//! Chroma always compiles with no top-level options, and `ECMAScript` and
//! `RE2` can only be set at the top level, so their syntax variants are
//! unreachable and not ported. regexp2's tree reductions are optimizations
//! that preserve match results and are skipped.

use std::collections::HashMap;

use super::charclass::{CharSet, any_participate_in_case_conversion};
use super::unicode;

pub(crate) const IGNORE_CASE: u32 = 0x0001;
pub(crate) const MULTILINE: u32 = 0x0002;
pub(crate) const EXPLICIT_CAPTURE: u32 = 0x0004;
pub(crate) const SINGLELINE: u32 = 0x0010;
pub(crate) const IGNORE_PATTERN_WHITESPACE: u32 = 0x0020;
pub(crate) const RIGHT_TO_LEFT: u32 = 0x0040;
pub(crate) const ECMASCRIPT: u32 = 0x0100;
pub(crate) const RE2: u32 = 0x0200;
pub(crate) const UNICODE: u32 = 0x0400;

pub(crate) const MAX_INT32: i32 = i32::MAX;

// Node types (regexp2 `NodeType`).
pub(crate) const NT_ONELOOP: i32 = 3;
pub(crate) const NT_NOTONELOOP: i32 = 4;
pub(crate) const NT_SETLOOP: i32 = 5;
pub(crate) const NT_ONELAZY: i32 = 6;
pub(crate) const NT_NOTONELAZY: i32 = 7;
pub(crate) const NT_SETLAZY: i32 = 8;
pub(crate) const NT_ONE: i32 = 9;
pub(crate) const NT_NOTONE: i32 = 10;
pub(crate) const NT_SET: i32 = 11;
pub(crate) const NT_MULTI: i32 = 12;
pub(crate) const NT_REF: i32 = 13;
pub(crate) const NT_BOL: i32 = 14;
pub(crate) const NT_EOL: i32 = 15;
pub(crate) const NT_BOUNDARY: i32 = 16;
pub(crate) const NT_NONBOUNDARY: i32 = 17;
pub(crate) const NT_BEGINNING: i32 = 18;
pub(crate) const NT_START: i32 = 19;
pub(crate) const NT_ENDZ: i32 = 20;
pub(crate) const NT_END: i32 = 21;
pub(crate) const NT_NOTHING: i32 = 22;
pub(crate) const NT_EMPTY: i32 = 23;
pub(crate) const NT_ALTERNATE: i32 = 24;
pub(crate) const NT_CONCATENATE: i32 = 25;
pub(crate) const NT_LOOP: i32 = 26;
pub(crate) const NT_LAZYLOOP: i32 = 27;
pub(crate) const NT_CAPTURE: i32 = 28;
pub(crate) const NT_GROUP: i32 = 29;
pub(crate) const NT_POSLOOK: i32 = 30;
pub(crate) const NT_NEGLOOK: i32 = 31;
pub(crate) const NT_ATOMIC: i32 = 32;
pub(crate) const NT_BACKREFCOND: i32 = 33;
pub(crate) const NT_EXPRCOND: i32 = 34;

// Instruction opcodes (regexp2 `InstOp`).
pub(crate) const ONEREP: i32 = 0;
pub(crate) const NOTONEREP: i32 = 1;
pub(crate) const SETREP: i32 = 2;
pub(crate) const ONELOOP: i32 = 3;
pub(crate) const NOTONELOOP: i32 = 4;
pub(crate) const SETLOOP: i32 = 5;
pub(crate) const ONELAZY: i32 = 6;
pub(crate) const NOTONELAZY: i32 = 7;
pub(crate) const SETLAZY: i32 = 8;
pub(crate) const ONE: i32 = 9;
pub(crate) const NOTONE: i32 = 10;
pub(crate) const SET: i32 = 11;
pub(crate) const MULTI: i32 = 12;
pub(crate) const REF: i32 = 13;
pub(crate) const BOL: i32 = 14;
pub(crate) const EOL: i32 = 15;
pub(crate) const BOUNDARY: i32 = 16;
pub(crate) const NONBOUNDARY: i32 = 17;
pub(crate) const BEGINNING: i32 = 18;
pub(crate) const START: i32 = 19;
pub(crate) const ENDZ: i32 = 20;
pub(crate) const END: i32 = 21;
pub(crate) const NOTHING: i32 = 22;
pub(crate) const LAZYBRANCH: i32 = 23;
pub(crate) const BRANCHMARK: i32 = 24;
pub(crate) const LAZYBRANCHMARK: i32 = 25;
pub(crate) const NULLCOUNT: i32 = 26;
pub(crate) const SETCOUNT: i32 = 27;
pub(crate) const BRANCHCOUNT: i32 = 28;
pub(crate) const LAZYBRANCHCOUNT: i32 = 29;
pub(crate) const NULLMARK: i32 = 30;
pub(crate) const SETMARK: i32 = 31;
pub(crate) const CAPTUREMARK: i32 = 32;
pub(crate) const GETMARK: i32 = 33;
pub(crate) const SETJUMP: i32 = 34;
pub(crate) const BACKJUMP: i32 = 35;
pub(crate) const FOREJUMP: i32 = 36;
pub(crate) const TESTREF: i32 = 37;
pub(crate) const GOTO: i32 = 38;
pub(crate) const STOP: i32 = 40;
pub(crate) const RTL: i32 = 64;
pub(crate) const BACK: i32 = 128;
pub(crate) const BACK2: i32 = 256;
pub(crate) const CI: i32 = 512;

const BEFORE_CHILD: i32 = 64;
const AFTER_CHILD: i32 = 128;

/// regexp2's limit for turning `a{n}` into a literal string.
const MULTI_VS_REPEATER_LIMIT: i32 = 64;

/// A pattern error: the regexp2 message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError(pub String);

#[derive(Debug, Clone)]
struct Node {
    t: i32,
    children: Vec<usize>,
    str: Vec<u32>,
    set: Option<CharSet>,
    ch: u32,
    m: i32,
    n: i32,
    options: u32,
    parent: Option<usize>,
}

pub(crate) struct Tree {
    nodes: Vec<Node>,
    root: usize,
    caps: HashMap<i32, i32>,
    capnumlist: Option<Vec<i32>>,
    captop: i32,
    pub(crate) caplist: Option<Vec<String>>,
}

fn opt_from_code(ch: char) -> u32 {
    match ch {
        'i' | 'I' => IGNORE_CASE,
        'r' | 'R' => RIGHT_TO_LEFT,
        'm' | 'M' => MULTILINE,
        'n' | 'N' => EXPLICIT_CAPTURE,
        's' | 'S' => SINGLELINE,
        'x' | 'X' => IGNORE_PATTERN_WHITESPACE,
        'e' | 'E' => ECMASCRIPT,
        'u' | 'U' => UNICODE,
        _ => 0,
    }
}

// Character categories for the scanner.
const Q: u8 = 5; // quantifier
const S: u8 = 4; // ordinary stopper
const Z: u8 = 3; // scan_blank stopper
const X: u8 = 2; // whitespace
const CATEGORY: [u8; 128] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, X, X, X, X, X, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, //
    X, 0, 0, Z, S, 0, 0, 0, S, S, Q, Q, 0, 0, S, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    Q, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, S, S, 0, S,
    0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, Q, S, 0, 0, 0,
];

fn cat(ch: char) -> u8 {
    CATEGORY.get(ch as usize).copied().unwrap_or(0)
}

fn is_space(ch: char) -> bool {
    ch <= ' ' && cat(ch) == X
}

fn is_special(ch: char) -> bool {
    ch <= '|' && cat(ch) >= S
}

fn is_stopper_x(ch: char) -> bool {
    ch <= '|' && cat(ch) >= X
}

fn is_quantifier(ch: char) -> bool {
    ch <= '{' && cat(ch) >= Q
}

struct Parser {
    nodes: Vec<Node>,
    stack: Option<usize>,
    group: usize,
    alternation: usize,
    concatenation: usize,
    unit: Option<usize>,
    pattern: Vec<char>,
    pos: usize,
    autocap: i32,
    capcount: i32,
    captop: i32,
    caps: HashMap<i32, i32>,
    capnames: Option<HashMap<String, i32>>,
    capnumlist: Option<Vec<i32>>,
    capnamelist: Vec<String>,
    options: u32,
    options_stack: Vec<u32>,
    ignore_next_paren: bool,
}

type PResult<T> = Result<T, ParseError>;

/// Parses `pattern` into a tree (regexp2 `syntax.Parse`).
pub(crate) fn parse(pattern: &str) -> PResult<Tree> {
    let mut p = Parser {
        nodes: Vec::new(),
        stack: None,
        group: 0,
        alternation: 0,
        concatenation: 0,
        unit: None,
        pattern: pattern.chars().collect(),
        pos: 0,
        autocap: 0,
        capcount: 0,
        captop: 0,
        caps: HashMap::new(),
        capnames: None,
        capnumlist: None,
        capnamelist: Vec::new(),
        options: 0,
        options_stack: Vec::new(),
        ignore_next_paren: false,
    };
    p.count_captures()?;
    p.reset(0);
    let root = p.scan_regex()?;
    Ok(Tree {
        nodes: p.nodes,
        root,
        caps: p.caps,
        capnumlist: p.capnumlist,
        captop: p.captop,
        caplist: if p.capnamelist.is_empty() {
            None
        } else {
            Some(p.capnamelist)
        },
    })
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError(format!(
            "error parsing regexp: {} in `{}`",
            msg.into(),
            self.pattern.iter().collect::<String>()
        ))
    }

    fn new_node(&mut self, t: i32, options: u32) -> usize {
        self.nodes.push(Node {
            t,
            children: Vec::new(),
            str: Vec::new(),
            set: None,
            ch: 0,
            m: 0,
            n: 0,
            options,
            parent: None,
        });
        self.nodes.len() - 1
    }

    fn new_node_mn(&mut self, t: i32, options: u32, m: i32, n: i32) -> usize {
        let id = self.new_node(t, options);
        self.nodes[id].m = m;
        self.nodes[id].n = n;
        id
    }

    fn new_node_ch(&mut self, t: i32, options: u32, ch: u32) -> usize {
        let id = self.new_node(t, options);
        self.nodes[id].ch = ch;
        self.with_case_conversion(id)
    }

    fn new_node_set(&mut self, t: i32, options: u32, set: CharSet) -> usize {
        let id = self.new_node(t, options);
        self.nodes[id].set = Some(set);
        self.with_case_conversion(id)
    }

    /// regexp2 `nodeWithCaseConversion`: case-insensitive chars and sets
    /// become sets holding their case equivalents.
    fn with_case_conversion(&mut self, id: usize) -> usize {
        let node = &mut self.nodes[id];
        if node.options & IGNORE_CASE == 0 {
            return id;
        }
        if node.ch > 0 {
            let ch = node.ch;
            if char::from_u32(ch).is_some_and(|c| unicode::is_lower(c) || unicode::is_upper(c)) {
                let mut set = CharSet::default();
                set.add_char(ch);
                set.add_case_equivalences();
                let t = match node.t {
                    NT_ONELOOP | NT_NOTONELOOP => NT_SETLOOP,
                    NT_ONELAZY | NT_NOTONELAZY => NT_SETLAZY,
                    _ => NT_SET,
                };
                set.negate = matches!(node.t, NT_NOTONE | NT_NOTONELOOP | NT_NOTONELAZY);
                node.t = t;
                node.options &= !IGNORE_CASE;
                node.ch = 0;
                node.set = Some(set);
            }
        } else if let Some(set) = &mut node.set {
            set.add_case_equivalences();
            node.options &= !IGNORE_CASE;
        }
        id
    }

    fn add_child(&mut self, parent: usize, child: usize) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    fn reverse_left(&mut self, id: usize) -> usize {
        let n = &mut self.nodes[id];
        if n.options & RIGHT_TO_LEFT != 0 && n.t == NT_CONCATENATE {
            n.children.reverse();
        }
        id
    }

    fn make_quantifier(&mut self, id: usize, lazy: bool, min: i32, max: i32) -> usize {
        if min == 0 && max == 0 {
            let options = self.nodes[id].options;
            return self.new_node(NT_EMPTY, options);
        }
        if min == 1 && max == 1 {
            return id;
        }
        let node = &mut self.nodes[id];
        if min == max && max <= MULTI_VS_REPEATER_LIMIT && node.t == NT_ONE {
            node.t = NT_MULTI;
            node.str = std::iter::repeat_n(node.ch, max as usize).collect();
            node.ch = 0;
            return id;
        }
        match node.t {
            NT_ONE | NT_NOTONE | NT_SET => {
                let base = if lazy { NT_ONELAZY } else { NT_ONELOOP };
                node.t += base - NT_ONE;
                node.m = min;
                node.n = max;
                id
            }
            _ => {
                let options = node.options;
                let t = if lazy { NT_LAZYLOOP } else { NT_LOOP };
                let result = self.new_node_mn(t, options, min, max);
                self.add_child(result, id);
                result
            }
        }
    }

    // --- cursor helpers -------------------------------------------------

    fn chars_right(&self) -> usize {
        self.pattern.len() - self.pos
    }

    fn right_char(&self, i: usize) -> char {
        self.pattern[self.pos + i]
    }

    fn move_right_get_char(&mut self) -> char {
        let ch = self.pattern[self.pos];
        self.pos += 1;
        ch
    }

    fn move_right(&mut self, i: usize) {
        self.pos += i;
    }

    fn move_left(&mut self) {
        self.pos -= 1;
    }

    fn use_n(&self) -> bool {
        self.options & EXPLICIT_CAPTURE != 0
    }

    fn use_i(&self) -> bool {
        self.options & IGNORE_CASE != 0
    }

    fn use_m(&self) -> bool {
        self.options & MULTILINE != 0
    }

    fn use_s(&self) -> bool {
        self.options & SINGLELINE != 0
    }

    fn use_x(&self) -> bool {
        self.options & IGNORE_PATTERN_WHITESPACE != 0
    }

    // --- capture bookkeeping ------------------------------------------

    fn note_capture_slot(&mut self, i: i32, pos: i32) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.caps.entry(i) {
            e.insert(pos);
            self.capcount += 1;
            if self.captop <= i {
                self.captop = if i == MAX_INT32 { i } else { i + 1 };
            }
        }
    }

    fn note_capture_name(&mut self, name: &str, pos: i32) {
        let names = self.capnames.get_or_insert_with(HashMap::new);
        if !names.contains_key(name) {
            names.insert(name.to_string(), pos);
            self.capnamelist.push(name.to_string());
        }
    }

    fn assign_name_slots(&mut self) {
        if let Some(names) = &self.capnames {
            let list = self.capnamelist.clone();
            let mut names = names.clone();
            for name in list {
                while self.is_capture_slot(self.autocap) {
                    self.autocap += 1;
                }
                let pos = names[&name];
                names.insert(name, self.autocap);
                self.note_capture_slot(self.autocap, pos);
                self.autocap += 1;
            }
            self.capnames = Some(names);
        }

        if self.capcount < self.captop {
            let mut list: Vec<i32> = self.caps.keys().copied().collect();
            list.sort_unstable();
            self.capnumlist = Some(list);
        }

        if self.capnames.is_some() || self.capnumlist.is_some() {
            let (old, mut next): (Vec<String>, i32) = match &self.capnames {
                None => {
                    self.capnames = Some(HashMap::new());
                    (Vec::new(), -1)
                }
                Some(names) => {
                    let old = self.capnamelist.clone();
                    let next = names[&old[0]];
                    (old, next)
                }
            };
            self.capnamelist = Vec::new();
            let mut k = 0;
            for i in 0..self.capcount {
                let j = self.capnumlist.as_ref().map_or(i, |l| l[i as usize]);
                if next == j {
                    self.capnamelist.push(old[k].clone());
                    k += 1;
                    next = if k == old.len() {
                        -1
                    } else {
                        self.capnames.as_ref().map_or(-1, |n| n[&old[k]])
                    };
                } else {
                    let s = j.to_string();
                    self.capnamelist.push(s.clone());
                    if let Some(n) = self.capnames.as_mut() {
                        n.insert(s, j);
                    }
                }
            }
        }
    }

    fn consume_autocap(&mut self) -> i32 {
        let r = self.autocap;
        self.autocap += 1;
        r
    }

    fn is_capture_slot(&self, i: i32) -> bool {
        self.caps.contains_key(&i)
    }

    fn is_capture_name(&self, name: &str) -> bool {
        self.capnames.as_ref().is_some_and(|n| n.contains_key(name))
    }

    fn capture_slot_from_name(&self, name: &str) -> i32 {
        self.capnames
            .as_ref()
            .and_then(|n| n.get(name))
            .copied()
            .unwrap_or(0)
    }

    /// Prescans the pattern to find the capture slots.
    fn count_captures(&mut self) -> PResult<()> {
        self.note_capture_slot(0, 0);
        self.autocap = 1;
        while self.chars_right() > 0 {
            let pos = self.pos as i32;
            let ch = self.move_right_get_char();
            match ch {
                '\\' if self.chars_right() > 0 => {
                    let _ = self.scan_backslash(true);
                }
                '#' if self.use_x() => {
                    self.move_left();
                    let _ = self.scan_blank();
                }
                '[' => {
                    let _ = self.scan_char_set(false, true);
                }
                ')' if !self.options_stack.is_empty() => {
                    self.pop_options();
                }
                '(' => {
                    if self.chars_right() >= 2
                        && self.right_char(1) == '#'
                        && self.right_char(0) == '?'
                    {
                        self.move_left();
                        let _ = self.scan_blank();
                    } else {
                        self.push_options();
                        if self.chars_right() > 0 && self.right_char(0) == '?' {
                            self.move_right(1);
                            if self.chars_right() > 1
                                && (self.right_char(0) == '<' || self.right_char(0) == '\'')
                            {
                                self.move_right(1);
                                let ch = self.right_char(0);
                                if ch != '0' && unicode::is_word_char(ch) {
                                    if ('1'..='9').contains(&ch) {
                                        let dec = self.scan_decimal()?;
                                        self.note_capture_slot(dec, pos);
                                    } else {
                                        let name = self.scan_word();
                                        self.note_capture_name(&name, pos);
                                    }
                                }
                            } else {
                                self.scan_options();
                                if self.chars_right() > 0 {
                                    if self.right_char(0) == ')' {
                                        self.move_right(1);
                                        self.pop_keep_options();
                                    } else if self.right_char(0) == '(' {
                                        // (?(cond)yes|no): do not capture the condition.
                                        self.ignore_next_paren = true;
                                        continue;
                                    }
                                }
                            }
                        } else if !self.use_n() && !self.ignore_next_paren {
                            let slot = self.consume_autocap();
                            self.note_capture_slot(slot, pos);
                        }
                    }
                    self.ignore_next_paren = false;
                }
                _ => {}
            }
        }
        self.assign_name_slots();
        Ok(())
    }

    fn reset(&mut self, topopts: u32) {
        self.pos = 0;
        self.autocap = 1;
        self.ignore_next_paren = false;
        self.options_stack.clear();
        self.options = topopts;
        self.stack = None;
    }

    fn scan_regex(&mut self) -> PResult<usize> {
        let mut is_quant = false;
        let root = self.new_node_mn(NT_CAPTURE, self.options, 0, -1);
        self.start_group(root);

        'outer: while self.chars_right() > 0 {
            let mut was_prev_quantifier = is_quant;
            is_quant = false;
            self.scan_blank()?;
            let startpos = self.pos;

            if self.use_x() {
                while self.chars_right() > 0 {
                    let ch = self.right_char(0);
                    if is_stopper_x(ch) && (ch != '{' || self.is_true_quantifier()) {
                        break;
                    }
                    self.move_right(1);
                }
            } else {
                while self.chars_right() > 0 {
                    let ch = self.right_char(0);
                    if is_special(ch) && (ch != '{' || self.is_true_quantifier()) {
                        break;
                    }
                    self.move_right(1);
                }
            }
            let endpos = self.pos;
            self.scan_blank()?;

            let ch = if self.chars_right() == 0 {
                '!' // at end
            } else {
                let c = self.right_char(0);
                if is_special(c) {
                    is_quant = is_quantifier(c);
                    self.move_right(1);
                    c
                } else {
                    ' ' // at an ordinary char
                }
            };

            if startpos < endpos {
                let mut cch_unquantified = endpos - startpos;
                if is_quant {
                    cch_unquantified -= 1;
                }
                was_prev_quantifier = false;
                if cch_unquantified > 0 {
                    self.add_to_concatenate(startpos, cch_unquantified);
                }
                if is_quant {
                    let c = self.pattern[endpos - 1] as u32;
                    self.unit = Some(self.new_node_ch(NT_ONE, self.options, c));
                }
            }

            match ch {
                '!' => break 'outer,
                ' ' => continue 'outer,
                '[' => {
                    let set = self.scan_char_set(self.use_i(), false)?.unwrap_or_default();
                    self.unit = Some(self.new_node_set(NT_SET, self.options, set));
                }
                '(' => {
                    self.push_options();
                    match self.scan_group_open()? {
                        None => self.pop_keep_options(),
                        Some(grouper) => {
                            self.push_group();
                            self.start_group(grouper);
                        }
                    }
                    continue 'outer;
                }
                '|' => {
                    self.add_alternate();
                    continue 'outer;
                }
                ')' => {
                    if self.stack.is_none() {
                        return Err(self.err("unexpected )"));
                    }
                    self.add_group()?;
                    self.pop_group()?;
                    self.pop_options();
                    if self.unit.is_none() {
                        continue 'outer;
                    }
                }
                '\\' => {
                    let n = self.scan_backslash(false)?;
                    self.unit = n;
                }
                '^' => {
                    let t = if self.use_m() { NT_BOL } else { NT_BEGINNING };
                    self.unit = Some(self.new_node(t, self.options));
                }
                '$' => {
                    let t = if self.use_m() { NT_EOL } else { NT_ENDZ };
                    self.unit = Some(self.new_node(t, self.options));
                }
                '.' => {
                    self.unit = Some(if self.use_s() {
                        self.new_node_set(NT_SET, self.options, CharSet::any())
                    } else {
                        self.new_node_ch(NT_NOTONE, self.options, u32::from('\n'))
                    });
                }
                '{' | '*' | '+' | '?' => {
                    if self.unit.is_none() {
                        return Err(self.err(if was_prev_quantifier {
                            "invalid nested repetition operator"
                        } else {
                            "missing argument to repetition operator"
                        }));
                    }
                    self.move_left();
                }
                _ => return Err(self.err("regexp/syntax: internal error")),
            }

            self.scan_blank()?;
            if self.chars_right() > 0 {
                is_quant = self.is_true_quantifier();
            }
            if self.chars_right() == 0 || !is_quant {
                self.add_concatenate();
                continue 'outer;
            }

            let ch = self.move_right_get_char();
            while self.unit.is_some() {
                let (min, mut max);
                match ch {
                    '*' => {
                        min = 0;
                        max = MAX_INT32;
                    }
                    '?' => {
                        min = 0;
                        max = 1;
                    }
                    '+' => {
                        min = 1;
                        max = MAX_INT32;
                    }
                    '{' => {
                        let startpos = self.pos;
                        min = self.scan_decimal()?;
                        max = min;
                        if startpos < self.pos
                            && self.chars_right() > 0
                            && self.right_char(0) == ','
                        {
                            self.move_right(1);
                            if self.chars_right() == 0 || self.right_char(0) == '}' {
                                max = MAX_INT32;
                            } else {
                                max = self.scan_decimal()?;
                            }
                        }
                        if startpos == self.pos
                            || self.chars_right() == 0
                            || self.move_right_get_char() != '}'
                        {
                            self.add_concatenate();
                            self.pos = startpos - 1;
                            continue 'outer;
                        }
                    }
                    _ => return Err(self.err("regexp/syntax: internal error")),
                }
                self.scan_blank()?;
                let lazy = if self.chars_right() == 0 || self.right_char(0) != '?' {
                    false
                } else {
                    self.move_right(1);
                    true
                };
                if min > max {
                    return Err(self.err("invalid repeat count"));
                }
                self.add_concatenate3(lazy, min, max);
            }
        }

        if self.stack.is_some() {
            return Err(self.err("missing closing )"));
        }
        self.add_group()?;
        self.unit
            .ok_or_else(|| self.err("regexp/syntax: internal error"))
    }

    fn scan_group_open(&mut self) -> PResult<Option<usize>> {
        let start = self.pos;
        let mut close = '>';

        if self.chars_right() == 0
            || self.right_char(0) != '?'
            || (self.right_char(0) == '?' && self.chars_right() > 1 && self.right_char(1) == ')')
        {
            if self.use_n() || self.ignore_next_paren {
                self.ignore_next_paren = false;
                return Ok(Some(self.new_node(NT_GROUP, self.options)));
            }
            let cap = self.consume_autocap();
            return Ok(Some(self.new_node_mn(NT_CAPTURE, self.options, cap, -1)));
        }

        self.move_right(1);
        if self.chars_right() > 0 {
            let nt;
            let mut ch = self.move_right_get_char();
            match ch {
                ':' => nt = NT_GROUP,
                '=' => {
                    self.options &= !RIGHT_TO_LEFT;
                    nt = NT_POSLOOK;
                }
                '!' => {
                    self.options &= !RIGHT_TO_LEFT;
                    nt = NT_NEGLOOK;
                }
                '>' => nt = NT_ATOMIC,
                '\'' | '<' => {
                    if ch == '\'' {
                        close = '\'';
                    }
                    if self.chars_right() == 0 {
                        return Err(self.unrecognized_grouping(start));
                    }
                    ch = self.move_right_get_char();
                    match ch {
                        '=' if close != '\'' => {
                            self.options |= RIGHT_TO_LEFT;
                            nt = NT_POSLOOK;
                        }
                        '!' if close != '\'' => {
                            self.options |= RIGHT_TO_LEFT;
                            nt = NT_NEGLOOK;
                        }
                        '=' | '!' => return Err(self.unrecognized_grouping(start)),
                        _ => {
                            self.move_left();
                            let mut capnum = -1;
                            let mut uncapnum = -1;
                            let mut proceed = false;
                            if ch.is_ascii_digit() {
                                capnum = self.scan_decimal()?;
                                if !self.is_capture_slot(capnum) {
                                    capnum = -1;
                                }
                                if self.chars_right() > 0
                                    && self.right_char(0) != close
                                    && self.right_char(0) != '-'
                                {
                                    return Err(self.invalid_group_name());
                                }
                                if capnum == 0 {
                                    return Err(self.err("capture number cannot be zero"));
                                }
                            } else if unicode::is_word_char(ch) {
                                let name = self.scan_word();
                                if self.is_capture_name(&name) {
                                    capnum = self.capture_slot_from_name(&name);
                                }
                                if self.chars_right() > 0
                                    && self.right_char(0) != close
                                    && self.right_char(0) != '-'
                                {
                                    return Err(self.invalid_group_name());
                                }
                            } else if ch == '-' {
                                proceed = true;
                            } else {
                                return Err(self.invalid_group_name());
                            }

                            if (capnum != -1 || proceed)
                                && self.chars_right() > 0
                                && self.right_char(0) == '-'
                            {
                                self.move_right(1);
                                if self.chars_right() == 0 {
                                    return Err(self.invalid_group_name());
                                }
                                let ch = self.right_char(0);
                                if ch.is_ascii_digit() {
                                    uncapnum = self.scan_decimal()?;
                                    if !self.is_capture_slot(uncapnum) {
                                        return Err(self.err(format!(
                                            "reference to undefined group number {uncapnum}"
                                        )));
                                    }
                                    if self.chars_right() > 0 && self.right_char(0) != close {
                                        return Err(self.invalid_group_name());
                                    }
                                } else if unicode::is_word_char(ch) {
                                    let name = self.scan_word();
                                    if !self.is_capture_name(&name) {
                                        return Err(self.err(format!(
                                            "reference to undefined group name {name}"
                                        )));
                                    }
                                    uncapnum = self.capture_slot_from_name(&name);
                                    if self.chars_right() > 0 && self.right_char(0) != close {
                                        return Err(self.invalid_group_name());
                                    }
                                } else {
                                    return Err(self.invalid_group_name());
                                }
                            }

                            if (capnum != -1 || uncapnum != -1)
                                && self.chars_right() > 0
                                && self.move_right_get_char() == close
                            {
                                return Ok(Some(self.new_node_mn(
                                    NT_CAPTURE,
                                    self.options,
                                    capnum,
                                    uncapnum,
                                )));
                            }
                            return Err(self.unrecognized_grouping(start));
                        }
                    }
                }
                '(' => {
                    let paren_pos = self.pos;
                    if self.chars_right() > 0 {
                        let ch = self.right_char(0);
                        if ch.is_ascii_digit() {
                            let capnum = self.scan_decimal()?;
                            if self.chars_right() > 0 && self.move_right_get_char() == ')' {
                                if self.is_capture_slot(capnum) {
                                    return Ok(Some(self.new_node_mn(
                                        NT_BACKREFCOND,
                                        self.options,
                                        capnum,
                                        0,
                                    )));
                                }
                                return Err(self
                                    .err(format!("(?({capnum}) ) reference to undefined group")));
                            }
                            return Err(self.err(format!("(?({capnum}) ) malformed")));
                        } else if unicode::is_word_char(ch) {
                            let name = self.scan_word();
                            if self.is_capture_name(&name)
                                && self.chars_right() > 0
                                && self.move_right_get_char() == ')'
                            {
                                let slot = self.capture_slot_from_name(&name);
                                return Ok(Some(self.new_node_mn(
                                    NT_BACKREFCOND,
                                    self.options,
                                    slot,
                                    0,
                                )));
                            }
                        }
                    }
                    nt = NT_EXPRCOND;
                    self.pos = paren_pos - 1;
                    self.ignore_next_paren = true;
                    let chars_right = self.chars_right();
                    if chars_right >= 3 && self.right_char(1) == '?' {
                        let rc2 = self.right_char(2);
                        if rc2 == '#' {
                            return Err(self.err("alternation conditions cannot be comments"));
                        }
                        if rc2 == '\'' {
                            return Err(self
                                .err("alternation conditions do not capture and cannot be named"));
                        }
                        if chars_right >= 4
                            && rc2 == '<'
                            && self.right_char(3) != '!'
                            && self.right_char(3) != '='
                        {
                            return Err(self
                                .err("alternation conditions do not capture and cannot be named"));
                        }
                    }
                }
                _ => {
                    self.move_left();
                    nt = NT_GROUP;
                    if self.nodes[self.group].t != NT_EXPRCOND {
                        self.scan_options();
                    }
                    if self.chars_right() == 0 {
                        return Err(self.unrecognized_grouping(start));
                    }
                    let ch = self.move_right_get_char();
                    if ch == ')' {
                        return Ok(None);
                    }
                    if ch != ':' {
                        return Err(self.unrecognized_grouping(start));
                    }
                }
            }
            return Ok(Some(self.new_node(nt, self.options)));
        }
        Err(self.unrecognized_grouping(start))
    }

    fn invalid_group_name(&self) -> ParseError {
        self.err("invalid group name: group names must begin with a word character and have a matching terminator")
    }

    fn unrecognized_grouping(&self, start: usize) -> ParseError {
        let s: String = self.pattern[start..self.pos].iter().collect();
        self.err(format!("unrecognized grouping construct: ({s}"))
    }

    fn scan_backslash(&mut self, scan_only: bool) -> PResult<Option<usize>> {
        if self.chars_right() == 0 {
            return Err(self.err("illegal \\ at end of pattern"));
        }
        let ch = self.right_char(0);
        let set = match ch {
            'b' | 'B' | 'A' | 'G' | 'Z' | 'z' => {
                self.move_right(1);
                let t = match ch {
                    'b' => NT_BOUNDARY,
                    'B' => NT_NONBOUNDARY,
                    'A' => NT_BEGINNING,
                    'G' => NT_START,
                    'Z' => NT_ENDZ,
                    _ => NT_END,
                };
                return Ok(Some(self.new_node(t, self.options)));
            }
            'w' => CharSet::word(false),
            'W' => CharSet::word(true),
            's' => CharSet::space(false),
            'S' => CharSet::space(true),
            'd' => CharSet::digit(false),
            'D' => CharSet::digit(true),
            'p' | 'P' => {
                self.move_right(1);
                let prop = self.parse_property()?;
                let mut cc = CharSet::default();
                cc.add_category(&prop, ch != 'p', self.use_i());
                if self.use_i() {
                    cc.add_lowercase();
                }
                return Ok(Some(self.new_node_set(NT_SET, self.options, cc)));
            }
            _ => return self.scan_basic_backslash(scan_only),
        };
        self.move_right(1);
        Ok(Some(self.new_node_set(NT_SET, self.options, set)))
    }

    fn scan_basic_backslash(&mut self, scan_only: bool) -> PResult<Option<usize>> {
        if self.chars_right() == 0 {
            return Err(self.err("illegal \\ at end of pattern"));
        }
        let mut angled = false;
        let mut k = false;
        let mut close = '\0';
        let backpos = self.pos;
        let mut ch = self.right_char(0);

        if ch == 'k' {
            if self.chars_right() >= 2 {
                self.move_right(1);
                ch = self.move_right_get_char();
                if ch == '<' || ch == '\'' {
                    angled = true;
                    close = if ch == '\'' { '\'' } else { '>' };
                }
            }
            if !angled || self.chars_right() == 0 {
                return Err(self.err("malformed \\k<...> named back reference"));
            }
            ch = self.right_char(0);
            k = true;
        } else if (ch == '<' || ch == '\'') && self.chars_right() > 1 {
            angled = true;
            close = if ch == '\'' { '\'' } else { '>' };
            self.move_right(1);
            ch = self.right_char(0);
        }

        if angled && ch.is_ascii_digit() {
            let capnum = self.scan_decimal()?;
            if self.chars_right() > 0 && self.move_right_get_char() == close {
                if self.is_capture_slot(capnum) {
                    return Ok(Some(self.new_node_mn(NT_REF, self.options, capnum, 0)));
                }
                return Err(self.err(format!("reference to undefined group number {capnum}")));
            }
        } else if !angled && ('1'..='9').contains(&ch) {
            let capnum = self.scan_decimal()?;
            if scan_only {
                return Ok(None);
            }
            if self.is_capture_slot(capnum) {
                return Ok(Some(self.new_node_mn(NT_REF, self.options, capnum, 0)));
            }
            if capnum <= 9 {
                return Err(self.err(format!("reference to undefined group number {capnum}")));
            }
        } else if angled {
            let name = self.scan_word();
            if !name.is_empty() && self.chars_right() > 0 && self.move_right_get_char() == close {
                if scan_only {
                    return Ok(None);
                }
                if self.is_capture_name(&name) {
                    let slot = self.capture_slot_from_name(&name);
                    return Ok(Some(self.new_node_mn(NT_REF, self.options, slot, 0)));
                }
                return Err(self.err(format!("reference to undefined group name {name}")));
            } else if k {
                return Err(self.err("malformed \\k<...> named back reference"));
            }
        }

        // Not a back reference: a character escape.
        self.pos = backpos;
        let mut ch = self.scan_char_escape()?;
        if scan_only {
            return Ok(None);
        }
        if self.use_i() {
            ch = to_lower_u32(ch);
        }
        Ok(Some(self.new_node_ch(NT_ONE, self.options, ch)))
    }

    /// The `X` of `\p{X}` or `\pX`, canonicalized.
    fn parse_property(&mut self) -> PResult<String> {
        if self.chars_right() >= 1 && self.right_char(0) != '{' {
            let name = self.move_right_get_char().to_string();
            return canonical_category(&name).ok_or_else(|| {
                self.err(format!(
                    "unknown unicode category, script, or property '{name}'"
                ))
            });
        }
        if self.chars_right() < 3 {
            return Err(self.err("incomplete \\p{X} character escape"));
        }
        if self.move_right_get_char() != '{' {
            return Err(self.err("malformed \\p{X} character escape"));
        }
        let startpos = self.pos;
        while self.chars_right() > 0 {
            let ch = self.move_right_get_char();
            if !unicode::is_word_char(ch) && ch != '-' && ch != '=' {
                self.move_left();
                break;
            }
        }
        let name: String = self.pattern[startpos..self.pos].iter().collect();
        if self.chars_right() == 0 || self.move_right_get_char() != '}' {
            return Err(self.err("incomplete \\p{X} character escape"));
        }
        canonical_category(&name).ok_or_else(|| {
            self.err(format!(
                "unknown unicode category, script, or property '{name}'"
            ))
        })
    }

    fn scan_blank(&mut self) -> PResult<()> {
        if self.use_x() {
            loop {
                while self.chars_right() > 0 && is_space(self.right_char(0)) {
                    self.move_right(1);
                }
                if self.chars_right() == 0 {
                    break;
                }
                if self.right_char(0) == '#' {
                    while self.chars_right() > 0 && self.right_char(0) != '\n' {
                        self.move_right(1);
                    }
                } else if self.chars_right() >= 3
                    && self.right_char(2) == '#'
                    && self.right_char(1) == '?'
                    && self.right_char(0) == '('
                {
                    while self.chars_right() > 0 && self.right_char(0) != ')' {
                        self.move_right(1);
                    }
                    if self.chars_right() == 0 {
                        return Err(self.err("unterminated comment"));
                    }
                    self.move_right(1);
                } else {
                    break;
                }
            }
        } else {
            loop {
                if self.chars_right() < 3
                    || self.right_char(2) != '#'
                    || self.right_char(1) != '?'
                    || self.right_char(0) != '('
                {
                    return Ok(());
                }
                while self.chars_right() > 0 && self.right_char(0) != ')' {
                    self.move_right(1);
                }
                if self.chars_right() == 0 {
                    return Err(self.err("unterminated comment"));
                }
                self.move_right(1);
            }
        }
        Ok(())
    }

    fn scan_word(&mut self) -> String {
        let startpos = self.pos;
        while self.chars_right() > 0 {
            if !unicode::is_word_char(self.move_right_get_char()) {
                self.move_left();
                break;
            }
        }
        self.pattern[startpos..self.pos].iter().collect()
    }

    /// Scans the inside of `[...]` into a set (`None` when only scanning).
    fn scan_char_set(
        &mut self,
        case_insensitive: bool,
        scan_only: bool,
    ) -> PResult<Option<CharSet>> {
        let c = |ch: char| ch as u32;
        let mut ch_prev: u32 = 0;
        let mut in_range = false;
        let mut first_char = true;
        let mut closed = false;
        let mut cc = CharSet::default();

        if self.chars_right() > 0 && self.right_char(0) == '^' {
            self.move_right(1);
            cc.negate = true;
        }

        while self.chars_right() > 0 {
            let mut translated = false;
            let raw = self.move_right_get_char();
            let mut ch = c(raw);
            if raw == ']' {
                if !first_char {
                    closed = true;
                    break;
                }
            } else if raw == '\\' && self.chars_right() > 0 {
                let esc = self.move_right_get_char();
                ch = c(esc);
                match esc {
                    'D' | 'd' | 'S' | 's' | 'W' | 'w' => {
                        if !scan_only {
                            if in_range {
                                return Err(self.err(format!(
                                    "cannot include class \\{esc} in character range"
                                )));
                            }
                            match esc {
                                'd' | 'D' => cc.add_digit(esc == 'D'),
                                's' | 'S' => cc.add_space(esc == 'S'),
                                _ => cc.add_word(esc == 'W'),
                            }
                        }
                        first_char = false;
                        continue;
                    }
                    'p' | 'P' => {
                        let prop = self.parse_property()?;
                        if !scan_only {
                            if in_range {
                                return Err(self.err(format!(
                                    "cannot create range with shorthand escape sequence \\{esc}"
                                )));
                            }
                            cc.add_category(&prop, esc != 'p', case_insensitive);
                        }
                        first_char = false;
                        continue;
                    }
                    '-' => {
                        if !scan_only {
                            cc.add_range(ch, ch);
                        }
                        first_char = false;
                        continue;
                    }
                    _ => {
                        self.move_left();
                        ch = self.scan_char_escape()?;
                        translated = true;
                    }
                }
            } else if raw == '[' {
                // POSIX-style [:name:] is skipped (regexp2 only honors it in RE2 mode).
                if self.chars_right() > 0 && self.right_char(0) == ':' && !in_range {
                    let save = self.pos;
                    self.move_right(1);
                    if self.chars_right() > 1 && self.right_char(0) == '^' {
                        self.move_right(1);
                    }
                    let _ = self.scan_word();
                    if self.chars_right() < 2
                        || self.move_right_get_char() != ':'
                        || self.move_right_get_char() != ']'
                    {
                        self.pos = save;
                    }
                }
            }

            if in_range {
                in_range = false;
                if !scan_only {
                    if ch == c('[') && !translated && !first_char {
                        // A subtraction: [a-z-[aeiou]].
                        cc.add_char(ch_prev);
                        let sub = self
                            .scan_char_set(case_insensitive, false)?
                            .unwrap_or_default();
                        cc.add_subtraction(sub);
                        if self.chars_right() > 0 && self.right_char(0) != ']' {
                            return Err(self.err(
                                "a subtraction must be the last element in a character class",
                            ));
                        }
                    } else {
                        if ch_prev > ch {
                            return Err(
                                self.err(format!("[{ch_prev:x}-{ch:x}] range in reverse order"))
                            );
                        }
                        cc.add_range(ch_prev, ch);
                    }
                }
            } else if self.chars_right() >= 2
                && self.right_char(0) == '-'
                && self.right_char(1) != ']'
            {
                ch_prev = ch;
                in_range = true;
                self.move_right(1);
            } else if self.chars_right() >= 1
                && ch == c('-')
                && !translated
                && self.right_char(0) == '['
                && !first_char
            {
                self.move_right(1);
                let sub = self.scan_char_set(case_insensitive, scan_only)?;
                if !scan_only {
                    cc.add_subtraction(sub.unwrap_or_default());
                    if self.chars_right() > 0 && self.right_char(0) != ']' {
                        return Err(
                            self.err("a subtraction must be the last element in a character class")
                        );
                    }
                }
            } else if !scan_only {
                cc.add_range(ch, ch);
            }
            first_char = false;
        }

        if !closed {
            return Err(self.err("unterminated [] set"));
        }
        if scan_only {
            return Ok(None);
        }
        if case_insensitive {
            cc.add_lowercase();
        }
        Ok(Some(cc))
    }

    /// Decimal digits, failing past 2^31-1.
    fn scan_decimal(&mut self) -> PResult<i32> {
        let mut i: i32 = 0;
        while self.chars_right() > 0 {
            let Some(d) = self
                .right_char(0)
                .to_digit(10)
                .filter(|_| self.right_char(0).is_ascii_digit())
            else {
                break;
            };
            let d = d as i32;
            self.move_right(1);
            if i > MAX_INT32 / 10 || (i == MAX_INT32 / 10 && d > MAX_INT32 % 10) {
                return Err(self.err("capture group number out of range"));
            }
            i = i * 10 + d;
        }
        Ok(i)
    }

    /// Scans `imnsxu-imnsxu` options, stopping at the first other character.
    fn scan_options(&mut self) {
        let mut off = false;
        while self.chars_right() > 0 {
            let ch = self.right_char(0);
            match ch {
                '-' => off = true,
                '+' => off = false,
                _ => {
                    let option = opt_from_code(ch);
                    if option == 0
                        || option == RIGHT_TO_LEFT
                        || option == ECMASCRIPT
                        || option == RE2
                    {
                        return;
                    }
                    if off {
                        self.options &= !option;
                    } else {
                        self.options |= option;
                    }
                }
            }
            self.move_right(1);
        }
    }

    /// A character escape (after the backslash) as a code point. Hex escapes
    /// may name surrogates, which no Rust char (and so no text) can equal.
    fn scan_char_escape(&mut self) -> PResult<u32> {
        let ch = self.move_right_get_char();
        if ('0'..='7').contains(&ch) {
            self.move_left();
            return Ok(self.scan_octal());
        }
        match ch {
            'x' => {
                if self.chars_right() > 0 && self.right_char(0) == '{' {
                    self.move_right(1);
                    self.scan_hex_until_brace()
                } else {
                    self.scan_hex(2)
                }
            }
            'u' => self.scan_hex(4),
            'a' => Ok(0x07),
            'b' => Ok(0x08),
            'e' => Ok(0x1B),
            'f' => Ok(0x0C),
            'n' => Ok(0x0A),
            'r' => Ok(0x0D),
            't' => Ok(0x09),
            'v' => Ok(0x0B),
            'c' => self.scan_control(),
            _ => {
                if unicode::is_word_char(ch) {
                    return Err(self.err(format!("unrecognized escape sequence \\{ch}")));
                }
                Ok(ch as u32)
            }
        }
    }

    fn scan_control(&mut self) -> PResult<u32> {
        if self.chars_right() == 0 {
            return Err(self.err("missing control character"));
        }
        let mut ch = i64::from(self.move_right_get_char() as u32);
        if (i64::from(b'a')..=i64::from(b'z')).contains(&ch) {
            ch -= i64::from(b'a' - b'A');
        }
        ch -= i64::from(b'@');
        if (0..32).contains(&ch) {
            return Ok(ch as u32);
        }
        Err(self.err("unrecognized control character"))
    }

    fn scan_hex_until_brace(&mut self) -> PResult<u32> {
        let mut i: u32 = 0;
        let mut has_content = false;
        while self.chars_right() > 0 {
            let ch = self.move_right_get_char();
            if ch == '}' {
                if !has_content {
                    return Err(self.err("insufficient hexadecimal digits"));
                }
                return Ok(i);
            }
            has_content = true;
            let Some(d) = ch.to_digit(16) else {
                return Err(self.err("missing closing }"));
            };
            i = i * 16 + d;
            if i > 0x10FFFF {
                return Err(self.err("hex values may not be larger than 0x10FFFF"));
            }
        }
        Err(self.err("missing closing }"))
    }

    fn scan_hex(&mut self, mut c: usize) -> PResult<u32> {
        let mut i: u32 = 0;
        if self.chars_right() >= c {
            while c > 0 {
                let Some(d) = self.move_right_get_char().to_digit(16) else {
                    break;
                };
                i = i * 16 + d;
                c -= 1;
            }
        }
        if c > 0 {
            return Err(self.err("insufficient hexadecimal digits"));
        }
        Ok(i)
    }

    /// Up to three octal digits, truncated to a byte like Perl.
    fn scan_octal(&mut self) -> u32 {
        let mut c = 3.min(self.chars_right());
        let mut i: u32 = 0;
        let mut d = i64::from(self.right_char(0) as u32) - i64::from(b'0');
        while c > 0 && (0..=7).contains(&d) {
            i = i * 8 + d as u32;
            c -= 1;
            self.move_right(1);
            if self.chars_right() > 0 {
                d = i64::from(self.right_char(0) as u32) - i64::from(b'0');
            }
        }
        i & 0xFF
    }

    fn is_true_quantifier(&self) -> bool {
        let mut n_chars = self.chars_right();
        if n_chars == 0 {
            return false;
        }
        let startpos = self.pos;
        let mut ch = self.pattern[startpos];
        if ch != '{' {
            return is_quantifier(ch);
        }
        let mut pos = startpos;
        loop {
            n_chars -= 1;
            if n_chars == 0 {
                break;
            }
            pos += 1;
            ch = self.pattern[pos];
            if !ch.is_ascii_digit() {
                break;
            }
        }
        if n_chars == 0 || pos - startpos == 1 {
            return false;
        }
        if ch == '}' {
            return true;
        }
        if ch != ',' {
            return false;
        }
        loop {
            n_chars -= 1;
            if n_chars == 0 {
                break;
            }
            pos += 1;
            ch = self.pattern[pos];
            if !ch.is_ascii_digit() {
                break;
            }
        }
        n_chars > 0 && ch == '}'
    }

    // --- tree building ----------------------------------------------

    fn add_concatenate(&mut self) {
        if let Some(unit) = self.unit.take() {
            self.add_child(self.concatenation, unit);
        }
    }

    fn add_concatenate3(&mut self, lazy: bool, min: i32, max: i32) {
        if let Some(unit) = self.unit.take() {
            let q = self.make_quantifier(unit, lazy, min, max);
            self.add_child(self.concatenation, q);
        }
    }

    fn add_to_concatenate(&mut self, pos: usize, cch: usize) {
        if cch == 1 {
            let node = self.new_node_ch(NT_ONE, self.options, self.pattern[pos] as u32);
            self.add_child(self.concatenation, node);
            return;
        }
        let chars: Vec<u32> = self.pattern[pos..pos + cch]
            .iter()
            .map(|&c| c as u32)
            .collect();
        if !self.use_i() || !any_participate_in_case_conversion(&chars) {
            let node = self.new_node(NT_MULTI, self.options & !IGNORE_CASE);
            self.nodes[node].str = chars;
            self.add_child(self.concatenation, node);
            return;
        }
        for c in chars {
            let node = self.new_node_ch(NT_ONE, self.options, c);
            self.add_child(self.concatenation, node);
        }
    }

    fn add_group(&mut self) -> PResult<()> {
        let gt = self.nodes[self.group].t;
        if gt == NT_EXPRCOND || gt == NT_BACKREFCOND {
            let c = self.reverse_left(self.concatenation);
            self.add_child(self.group, c);
            let n = self.nodes[self.group].children.len();
            if (gt == NT_BACKREFCOND && n > 2) || n > 3 {
                return Err(self.err("too many | in (?()|)"));
            }
        } else {
            let c = self.reverse_left(self.concatenation);
            self.add_child(self.alternation, c);
            self.add_child(self.group, self.alternation);
        }
        self.unit = Some(self.group);
        Ok(())
    }

    fn pop_keep_options(&mut self) {
        self.options_stack.pop();
    }

    fn pop_options(&mut self) {
        if let Some(o) = self.options_stack.pop() {
            self.options = o;
        }
    }

    fn push_options(&mut self) {
        self.options_stack.push(self.options);
    }

    fn push_group(&mut self) {
        let (g, a, c) = (self.group, self.alternation, self.concatenation);
        self.nodes[g].parent = self.stack;
        self.nodes[a].parent = Some(g);
        self.nodes[c].parent = Some(a);
        self.stack = Some(c);
    }

    fn pop_group(&mut self) -> PResult<()> {
        let Some(c) = self.stack else {
            return Err(self.err("unexpected )"));
        };
        self.concatenation = c;
        self.alternation = self.nodes[c].parent.unwrap_or(c);
        self.group = self.nodes[self.alternation]
            .parent
            .unwrap_or(self.alternation);
        self.stack = self.nodes[self.group].parent;
        if self.nodes[self.group].t == NT_EXPRCOND && self.nodes[self.group].children.is_empty() {
            let Some(unit) = self.unit.take() else {
                return Err(self.err("illegal conditional (?(...)) expression"));
            };
            self.add_child(self.group, unit);
        }
        Ok(())
    }

    fn start_group(&mut self, open_group: usize) {
        self.group = open_group;
        self.alternation = self.new_node(NT_ALTERNATE, self.options);
        self.concatenation = self.new_node(NT_CONCATENATE, self.options);
    }

    fn add_alternate(&mut self) {
        let gt = self.nodes[self.group].t;
        let c = self.reverse_left(self.concatenation);
        if gt == NT_EXPRCOND || gt == NT_BACKREFCOND {
            self.add_child(self.group, c);
        } else {
            self.add_child(self.alternation, c);
        }
        self.concatenation = self.new_node(NT_CONCATENATE, self.options);
    }
}

/// Canonical name for a `\p{…}` category (general categories and their Go
/// aliases).
fn to_lower_u32(c: u32) -> u32 {
    char::from_u32(c).map_or(c, |ch| unicode::to_lower(ch) as u32)
}

fn canonical_category(name: &str) -> Option<String> {
    unicode::category(name).map(|_| name.to_string())
}

/// Compiled program (regexp2 `Code`).
pub(crate) struct Code {
    pub codes: Vec<i32>,
    pub strings: Vec<Vec<u32>>,
    pub sets: Vec<CharSet>,
    pub track_count: usize,
    pub caps: Option<HashMap<i32, i32>>,
    pub capsize: usize,
}

struct Writer<'t> {
    tree: &'t Tree,
    emitted: Vec<i32>,
    int_stack: Vec<usize>,
    strings: Vec<Vec<u32>>,
    sets: Vec<CharSet>,
    track_count: usize,
    caps: Option<HashMap<i32, i32>>,
}

/// Generates code from a parse tree (regexp2 `syntax.Write`).
pub(crate) fn write(tree: &Tree) -> Result<Code, ParseError> {
    let (capsize, caps) = match &tree.capnumlist {
        Some(list) if tree.captop as usize != list.len() => {
            let mut caps = tree.caps.clone();
            for (i, &k) in list.iter().enumerate() {
                caps.insert(k, i as i32);
            }
            (list.len(), Some(caps))
        }
        _ => (tree.captop as usize, None),
    };
    let mut w = Writer {
        tree,
        emitted: Vec::new(),
        int_stack: Vec::new(),
        strings: Vec::new(),
        sets: Vec::new(),
        track_count: 0,
        caps,
    };
    let mut cur = tree.root;
    let mut cur_child = 0usize;
    w.emit1(LAZYBRANCH, 0);
    loop {
        let node = &tree.nodes[cur];
        if node.children.is_empty() {
            w.emit_fragment(node.t, cur, 0)?;
        } else if cur_child < node.children.len() {
            w.emit_fragment(node.t | BEFORE_CHILD, cur, cur_child)?;
            cur = node.children[cur_child];
            w.int_stack.push(cur_child);
            cur_child = 0;
            continue;
        }
        let Some(child) = w.int_stack.pop() else {
            break;
        };
        cur_child = child;
        cur = tree.nodes[cur].parent.unwrap_or(tree.root);
        w.emit_fragment(tree.nodes[cur].t | AFTER_CHILD, cur, cur_child)?;
        cur_child += 1;
    }
    let end = w.cur_pos();
    w.patch_jump(0, end);
    w.emit(STOP);
    Ok(Code {
        codes: w.emitted,
        strings: w.strings,
        sets: w.sets,
        track_count: w.track_count,
        caps: w.caps,
        capsize,
    })
}

fn opcode_backtracks(op: i32) -> bool {
    matches!(
        op & 63,
        ONELOOP
            | NOTONELOOP
            | SETLOOP
            | ONELAZY
            | NOTONELAZY
            | SETLAZY
            | LAZYBRANCH
            | BRANCHMARK
            | LAZYBRANCHMARK
            | NULLCOUNT
            | SETCOUNT
            | BRANCHCOUNT
            | LAZYBRANCHCOUNT
            | SETMARK
            | CAPTUREMARK
            | GETMARK
            | SETJUMP
            | BACKJUMP
            | FOREJUMP
            | GOTO
    )
}

impl Writer<'_> {
    fn cur_pos(&self) -> i32 {
        self.emitted.len() as i32
    }

    fn patch_jump(&mut self, offset: i32, dest: i32) {
        self.emitted[offset as usize + 1] = dest;
    }

    fn emit(&mut self, op: i32) {
        if opcode_backtracks(op) {
            self.track_count += 1;
        }
        self.emitted.push(op);
    }

    fn emit1(&mut self, op: i32, a: i32) {
        self.emit(op);
        self.emitted.push(a);
    }

    fn emit2(&mut self, op: i32, a: i32, b: i32) {
        self.emit(op);
        self.emitted.push(a);
        self.emitted.push(b);
    }

    fn map_capnum(&self, capnum: i32) -> i32 {
        if capnum == -1 {
            return -1;
        }
        match &self.caps {
            Some(caps) => caps.get(&capnum).copied().unwrap_or(0),
            None => capnum,
        }
    }

    fn set_code(&mut self, set: &CharSet) -> i32 {
        if let Some(i) = self.sets.iter().position(|s| s == set) {
            return i as i32;
        }
        self.sets.push(set.clone());
        (self.sets.len() - 1) as i32
    }

    fn string_code(&mut self, s: &[u32]) -> i32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as i32;
        }
        self.strings.push(s.to_vec());
        (self.strings.len() - 1) as i32
    }

    fn pop_int(&mut self) -> i32 {
        self.int_stack.pop().map_or(0, |v| v as i32)
    }

    fn push_int(&mut self, v: i32) {
        self.int_stack.push(v as usize);
    }

    fn emit_fragment(
        &mut self,
        nodetype: i32,
        id: usize,
        cur_index: usize,
    ) -> Result<(), ParseError> {
        let node = &self.tree.nodes[id];
        let mut bits = 0;
        if node.options & RIGHT_TO_LEFT != 0 {
            bits |= RTL;
        }
        if node.options & IGNORE_CASE != 0 {
            bits |= CI;
        }
        let last_child = cur_index + 1 < node.children.len();
        match nodetype {
            // A childless concatenation is what regexp2's reduction turns into Empty.
            x if x == NT_CONCATENATE | BEFORE_CHILD
                || x == NT_CONCATENATE | AFTER_CHILD
                || x == NT_EMPTY
                || x == NT_CONCATENATE => {}
            x if x == NT_ALTERNATE | BEFORE_CHILD => {
                if last_child {
                    let p = self.cur_pos();
                    self.push_int(p);
                    self.emit1(LAZYBRANCH, 0);
                }
            }
            x if x == NT_ALTERNATE | AFTER_CHILD => {
                if last_child {
                    let lb = self.pop_int();
                    let p = self.cur_pos();
                    self.push_int(p);
                    self.emit1(GOTO, 0);
                    let here = self.cur_pos();
                    self.patch_jump(lb, here);
                } else {
                    for _ in 0..cur_index {
                        let j = self.pop_int();
                        let here = self.cur_pos();
                        self.patch_jump(j, here);
                    }
                }
            }
            x if x == NT_BACKREFCOND | BEFORE_CHILD => {
                if cur_index == 0 {
                    self.emit(SETJUMP);
                    let p = self.cur_pos();
                    self.push_int(p);
                    self.emit1(LAZYBRANCH, 0);
                    let c = self.map_capnum(node.m);
                    self.emit1(TESTREF, c);
                    self.emit(FOREJUMP);
                }
            }
            x if x == NT_BACKREFCOND | AFTER_CHILD => match cur_index {
                0 => {
                    let branchpos = self.pop_int();
                    let p = self.cur_pos();
                    self.push_int(p);
                    self.emit1(GOTO, 0);
                    let here = self.cur_pos();
                    self.patch_jump(branchpos, here);
                    self.emit(FOREJUMP);
                    if node.children.len() <= 1 {
                        let j = self.pop_int();
                        let here = self.cur_pos();
                        self.patch_jump(j, here);
                    }
                }
                1 => {
                    let j = self.pop_int();
                    let here = self.cur_pos();
                    self.patch_jump(j, here);
                }
                _ => {}
            },
            x if x == NT_EXPRCOND | BEFORE_CHILD => {
                if cur_index == 0 {
                    self.emit(SETJUMP);
                    self.emit(SETMARK);
                    let p = self.cur_pos();
                    self.push_int(p);
                    self.emit1(LAZYBRANCH, 0);
                }
            }
            x if x == NT_EXPRCOND | AFTER_CHILD => match cur_index {
                0 => {
                    self.emit(GETMARK);
                    self.emit(FOREJUMP);
                }
                1 => {
                    let branchpos = self.pop_int();
                    let p = self.cur_pos();
                    self.push_int(p);
                    self.emit1(GOTO, 0);
                    let here = self.cur_pos();
                    self.patch_jump(branchpos, here);
                    self.emit(GETMARK);
                    self.emit(FOREJUMP);
                    if node.children.len() <= 2 {
                        let j = self.pop_int();
                        let here = self.cur_pos();
                        self.patch_jump(j, here);
                    }
                }
                2 => {
                    let j = self.pop_int();
                    let here = self.cur_pos();
                    self.patch_jump(j, here);
                }
                _ => {}
            },
            x if x == NT_LOOP | BEFORE_CHILD || x == NT_LAZYLOOP | BEFORE_CHILD => {
                if node.n < MAX_INT32 || node.m > 1 {
                    if node.m == 0 {
                        self.emit1(NULLCOUNT, 0);
                    } else {
                        self.emit1(SETCOUNT, 1 - node.m);
                    }
                } else if node.m == 0 {
                    self.emit(NULLMARK);
                } else {
                    self.emit(SETMARK);
                }
                if node.m == 0 {
                    let p = self.cur_pos();
                    self.push_int(p);
                    self.emit1(GOTO, 0);
                }
                let p = self.cur_pos();
                self.push_int(p);
            }
            x if x == NT_LOOP | AFTER_CHILD || x == NT_LAZYLOOP | AFTER_CHILD => {
                let start_jump_pos = self.cur_pos();
                let lazy = nodetype - (NT_LOOP | AFTER_CHILD);
                if node.n < MAX_INT32 || node.m > 1 {
                    let target = self.pop_int();
                    let limit = if node.n == MAX_INT32 {
                        MAX_INT32
                    } else {
                        node.n - node.m
                    };
                    self.emit2(BRANCHCOUNT + lazy, target, limit);
                } else {
                    let target = self.pop_int();
                    self.emit1(BRANCHMARK + lazy, target);
                }
                if node.m == 0 {
                    let j = self.pop_int();
                    self.patch_jump(j, start_jump_pos);
                }
            }
            x if x == NT_GROUP | BEFORE_CHILD || x == NT_GROUP | AFTER_CHILD => {}
            x if x == NT_CAPTURE | BEFORE_CHILD => self.emit(SETMARK),
            x if x == NT_CAPTURE | AFTER_CHILD => {
                let (a, b) = (self.map_capnum(node.m), self.map_capnum(node.n));
                self.emit2(CAPTUREMARK, a, b);
            }
            x if x == NT_POSLOOK | BEFORE_CHILD => {
                self.emit(SETJUMP);
                self.emit(SETMARK);
            }
            x if x == NT_POSLOOK | AFTER_CHILD => {
                self.emit(GETMARK);
                self.emit(FOREJUMP);
            }
            x if x == NT_NEGLOOK | BEFORE_CHILD => {
                self.emit(SETJUMP);
                let p = self.cur_pos();
                self.push_int(p);
                self.emit1(LAZYBRANCH, 0);
            }
            x if x == NT_NEGLOOK | AFTER_CHILD => {
                self.emit(BACKJUMP);
                let j = self.pop_int();
                let here = self.cur_pos();
                self.patch_jump(j, here);
                self.emit(FOREJUMP);
            }
            x if x == NT_ATOMIC | BEFORE_CHILD => self.emit(SETJUMP),
            x if x == NT_ATOMIC | AFTER_CHILD => self.emit(FOREJUMP),
            NT_ONE | NT_NOTONE => self.emit1(node.t | bits, node.ch as i32),
            NT_NOTONELOOP | NT_NOTONELAZY | NT_ONELOOP | NT_ONELAZY => {
                if node.m > 0 {
                    let rep = if node.t == NT_ONELOOP || node.t == NT_ONELAZY {
                        ONEREP
                    } else {
                        NOTONEREP
                    };
                    self.emit2(rep | bits, node.ch as i32, node.m);
                }
                if node.n > node.m {
                    let count = if node.n == MAX_INT32 {
                        MAX_INT32
                    } else {
                        node.n - node.m
                    };
                    self.emit2(node.t | bits, node.ch as i32, count);
                }
            }
            NT_SETLOOP | NT_SETLAZY => {
                let set = node.set.clone().unwrap_or_default();
                let code = self.set_code(&set);
                if node.m > 0 {
                    self.emit2(SETREP | bits, code, node.m);
                }
                if node.n > node.m {
                    let count = if node.n == MAX_INT32 {
                        MAX_INT32
                    } else {
                        node.n - node.m
                    };
                    self.emit2(node.t | bits, code, count);
                }
            }
            NT_MULTI => {
                let s = node.str.clone();
                let code = self.string_code(&s);
                self.emit1(node.t | bits, code);
            }
            NT_SET => {
                let set = node.set.clone().unwrap_or_default();
                let code = self.set_code(&set);
                self.emit1(node.t | bits, code);
            }
            NT_REF => {
                let c = self.map_capnum(node.m);
                self.emit1(node.t | bits, c);
            }
            NT_NOTHING | NT_BOL | NT_EOL | NT_BOUNDARY | NT_NONBOUNDARY | NT_BEGINNING
            | NT_START | NT_ENDZ | NT_END => self.emit(node.t),
            other => {
                return Err(ParseError(format!(
                    "unexpected opcode in regular expression generation: {other}"
                )));
            }
        }
        Ok(())
    }
}
