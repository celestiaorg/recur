//! Vim's state: per editor (`cm.state.vim`) and shared by every editor
//! (`vimGlobalState`: registers, history, jump list, macros, options).

use std::cell::RefCell;
use std::collections::HashMap;

use super::keymap::VimKey;
use super::types::*;

/// A special key pressed in insert mode (`InsertModeKey`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertModeKey {
    pub key_name: String,
}

/// One recorded insert-mode change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertChange {
    Text(String),
    /// `[text]`: typed over (replace mode).
    Overwrite(String),
    /// `[text, offset]`: inserted with the cursor `offset` before its end.
    Offset(String, i64),
    Key(InsertModeKey),
}

/// `InsertModeChanges`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InsertModeChanges {
    pub changes: Vec<InsertChange>,
    pub expect_cursor_activity_for_change: bool,
    pub visual_block: i64,
    pub ignore_count: i64,
    pub maybe_reset: bool,
}

/// `ChangeQueue`: text an insert-mode mapping prefix typed and replaced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeQueue {
    pub removed: Vec<String>,
    pub inserted: String,
}

/// `InputState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputState {
    pub prefix_repeat: Vec<String>,
    pub motion_repeat: Vec<String>,
    pub operator: Option<Operator>,
    pub operator_args: Option<OperatorArgs>,
    pub motion: Option<Motion>,
    pub motion_args: Option<MotionArgs>,
    pub key_buffer: Vec<String>,
    pub register_name: Option<String>,
    pub change_queue: Option<ChangeQueue>,
    pub change_queue_list: Vec<Option<ChangeQueue>>,
    pub operator_shortcut: Option<String>,
    pub selected_character: Option<String>,
    pub repeat_override: Option<i64>,
}

impl InputState {
    pub fn push_repeat_digit(&mut self, n: &str) {
        if self.operator.is_none() {
            self.prefix_repeat.push(n.to_string());
        } else {
            self.motion_repeat.push(n.to_string());
        }
    }

    pub fn get_repeat(&self) -> i64 {
        let mut repeat = 0;
        if !self.prefix_repeat.is_empty() || !self.motion_repeat.is_empty() {
            repeat = 1;
            if !self.prefix_repeat.is_empty() {
                repeat *= self.prefix_repeat.join("").parse::<i64>().unwrap_or(0);
            }
            if !self.motion_repeat.is_empty() {
                repeat *= self.motion_repeat.join("").parse::<i64>().unwrap_or(0);
            }
        }
        repeat
    }
}

/// `lastSelection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastSelection {
    pub anchor_mark: u64,
    pub head_mark: u64,
    pub anchor: Pos,
    pub head: Pos,
    pub visual_mode: bool,
    pub visual_line: bool,
    pub visual_block: bool,
}

/// `vim.marks`: bookmark ids by name, iterated in JavaScript object key
/// order (array-index names first, ascending; then insertion order).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks(Vec<(String, u64)>);

impl Marks {
    pub fn get(&self, name: &str) -> Option<u64> {
        self.0.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }

    /// Assign; an existing name keeps its place.
    pub fn insert(&mut self, name: String, id: u64) {
        match self.0.iter_mut().find(|(k, _)| *k == name) {
            Some(slot) => slot.1 = id,
            None => self.0.push((name, id)),
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(k, _)| k != name);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &u64)> {
        super::ex::js_key_order(self.0.iter().map(|(k, v)| (k, v))).into_iter()
    }
}

/// The search state (`SearchState`): the vim overlay installed.
#[derive(Clone, Debug, Default)]
pub struct SearchState {
    pub overlay: Option<super::search::VimRegex>,
    pub highlight_pending: bool,
}

/// A buffer-local option value.
#[derive(Clone, Debug, PartialEq)]
pub enum OptValue {
    Bool(bool),
    Num(i64),
    Str(String),
}

/// `cm.state.vim`.
#[derive(Debug)]
pub struct VimState {
    pub input_state: InputState,
    pub last_edit_input_state: Option<InputState>,
    pub last_edit_action_command: Option<VimKey>,
    /// `lastHPos` (`INF` is `Infinity`).
    pub last_h_pos: i64,
    pub last_hs_pos: f32,
    pub last_motion: Option<Motion>,
    pub marks: Marks,
    pub insert_mode: bool,
    pub insert_mode_return: bool,
    pub insert_mode_repeat: Option<i64>,
    pub visual_mode: bool,
    pub visual_line: bool,
    pub visual_block: bool,
    pub last_selection: Option<LastSelection>,
    pub last_pasted_text: Option<String>,
    pub sel: Range,
    pub options: HashMap<String, OptValue>,
    pub expect_literal_next: bool,
    pub status: String,
    pub mode: String,
    pub insert_end: Option<u64>,
    pub search_state: SearchState,
    pub ex_mode: bool,
    pub was_in_visual_block: bool,
    /// `oneNormalCommand`'s pending `vim-command-done` handler.
    pub one_normal_handler: bool,
}

impl Default for VimState {
    fn default() -> Self {
        VimState {
            input_state: InputState::default(),
            last_edit_input_state: None,
            last_edit_action_command: None,
            last_h_pos: -1,
            last_hs_pos: -1.0,
            last_motion: None,
            marks: Marks::default(),
            insert_mode: false,
            insert_mode_return: false,
            insert_mode_repeat: None,
            visual_mode: false,
            visual_line: false,
            visual_block: false,
            last_selection: None,
            last_pasted_text: None,
            sel: Range::default(),
            options: HashMap::new(),
            expect_literal_next: false,
            status: String::new(),
            mode: "normal".into(),
            insert_end: None,
            search_state: SearchState::default(),
            ex_mode: false,
            was_in_visual_block: false,
            one_normal_handler: false,
        }
    }
}

/// A register (`Register`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Register {
    pub key_buffer: Vec<String>,
    pub insert_mode_changes: Vec<InsertModeChanges>,
    pub search_queries: Vec<String>,
    pub linewise: bool,
    pub blockwise: bool,
}

impl Register {
    pub fn new(text: &str, linewise: bool, blockwise: bool) -> Register {
        Register {
            key_buffer: vec![text.to_string()],
            insert_mode_changes: Vec::new(),
            search_queries: Vec::new(),
            linewise,
            blockwise,
        }
    }

    pub fn set_text(&mut self, text: &str, linewise: bool, blockwise: bool) {
        self.key_buffer = vec![text.to_string()];
        self.linewise = linewise;
        self.blockwise = blockwise;
    }

    pub fn push_text(&mut self, text: &str, linewise: bool) {
        if linewise {
            if !self.linewise {
                self.key_buffer.push("\n".into());
            }
            self.linewise = true;
        }
        self.key_buffer.push(text.to_string());
    }

    pub fn clear(&mut self) {
        self.key_buffer.clear();
        self.insert_mode_changes.clear();
        self.search_queries.clear();
        self.linewise = false;
    }

    pub fn text(&self) -> String {
        self.key_buffer.join("")
    }
}

pub const VALID_MARKS: [&str; 2] = ["<", ">"];

fn is_latin(name: &str) -> bool {
    let mut c = name.chars();
    matches!((c.next(), c.next()), (Some(ch), None) if ch.is_ascii_alphanumeric() || ch == '_')
}

/// `isUpperCase` (`\p{Lu}`).
pub fn is_upper(k: &str) -> bool {
    let mut c = k.chars();
    matches!((c.next(), c.next()), (Some(ch), None) if ch.is_uppercase())
}

/// `RegisterController`: registers in creation order (`'"'` is the unnamed
/// register).
#[derive(Clone, Debug)]
pub struct RegisterController {
    pub registers: Vec<(String, Register)>,
    pub valid: Vec<String>,
}

impl Default for RegisterController {
    fn default() -> Self {
        let mut r = RegisterController {
            registers: Vec::new(),
            valid: ["-", "\"", ".", ":", "_", "/", "+"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        for n in ["\"", ".", ":", "/", "+"] {
            r.registers.push((n.into(), Register::default()));
        }
        r
    }
}

impl RegisterController {
    pub fn is_valid(&self, name: Option<&str>) -> bool {
        name.is_some_and(|n| !n.is_empty() && (self.valid.iter().any(|v| v == n) || is_latin(n)))
    }

    fn slot(&mut self, name: &str) -> &mut Register {
        if let Some(i) = self.registers.iter().position(|(n, _)| n == name) {
            return &mut self.registers[i].1;
        }
        self.registers.push((name.to_string(), Register::default()));
        let last = self.registers.len() - 1;
        &mut self.registers[last].1
    }

    fn put(&mut self, name: &str, reg: Register) {
        *self.slot(name) = reg;
    }

    /// `getRegister`: the unnamed register for invalid names.
    pub fn get(&mut self, name: Option<&str>) -> &mut Register {
        if !self.is_valid(name) {
            return self.slot("\"");
        }
        let name = name.unwrap_or("\"").to_lowercase();
        self.slot(&name)
    }

    #[cfg(test)]
    pub fn get_text(&mut self, name: Option<&str>) -> String {
        self.get(name).text()
    }

    pub fn unnamed(&mut self) -> &mut Register {
        self.slot("\"")
    }

    /// `pushText`; returns text for the system clipboard (`"+`).
    pub fn push_text(
        &mut self,
        name: Option<&str>,
        operator: &str,
        text: &str,
        linewise: bool,
        blockwise: bool,
    ) -> Option<String> {
        if name == Some("_") {
            return None;
        }
        let mut text = text.to_string();
        if linewise && !text.ends_with('\n') {
            text.push('\n');
        }
        let valid = self.is_valid(name);
        if !valid || name.is_none() {
            match operator {
                "yank" => self.put("0", Register::new(&text, linewise, blockwise)),
                "delete" | "change" => {
                    if !text.contains('\n') {
                        self.put("-", Register::new(&text, linewise, false));
                    } else {
                        for i in (2..=9).rev() {
                            let prev = self.get(Some(&(i - 1).to_string())).clone();
                            self.put(&i.to_string(), prev);
                        }
                        self.put("1", Register::new(&text, linewise, false));
                    }
                }
                _ => {}
            }
            self.unnamed().set_text(&text, linewise, blockwise);
            return None;
        }
        let name = name.unwrap_or_default();
        let append = is_upper(name);
        {
            let reg = self.get(Some(name));
            if append {
                reg.push_text(&text, linewise);
            } else {
                reg.set_text(&text, linewise, blockwise);
            }
        }
        let content = self.get(Some(name)).text();
        self.unnamed().set_text(&content, linewise, false);
        (name == "+").then_some(text)
    }
}

/// `HistoryController`.
#[derive(Clone, Debug, Default)]
pub struct HistoryController {
    pub buffer: Vec<String>,
    pub iterator: usize,
    pub initial_prefix: Option<String>,
}

impl HistoryController {
    /// `nextMatch`.
    pub fn next_match(&mut self, input: &str, up: bool) -> Option<String> {
        if self.initial_prefix.is_none() {
            self.initial_prefix = Some(input.to_string());
        }
        let prefix = self.initial_prefix.clone().unwrap_or_default();
        let mut i = self.iterator as i64 + if up { -1 } else { 1 };
        while if up {
            i >= 0
        } else {
            (i as usize) < self.buffer.len()
        } {
            let element = &self.buffer[i as usize];
            let chars: Vec<char> = element.chars().collect();
            for j in 0..=chars.len() {
                if prefix == chars[..j].iter().collect::<String>() {
                    self.iterator = i as usize;
                    return Some(element.clone());
                }
            }
            i += if up { -1 } else { 1 };
        }
        if i >= self.buffer.len() as i64 {
            self.iterator = self.buffer.len();
            return Some(prefix);
        }
        if i < 0 {
            return Some(input.to_string());
        }
        None
    }

    pub fn push_input(&mut self, input: &str) {
        if let Some(i) = self.buffer.iter().position(|b| b == input) {
            self.buffer.remove(i);
        }
        if !input.is_empty() {
            self.buffer.push(input.to_string());
        }
    }

    pub fn reset(&mut self) {
        self.initial_prefix = None;
        self.iterator = self.buffer.len();
    }
}

/// `createCircularJumpList`: bookmark ids in a ring of 100.
#[derive(Clone, Debug)]
pub struct JumpList {
    pub pointer: i64,
    pub head: i64,
    pub tail: i64,
    pub buffer: Vec<Option<u64>>,
    pub cached_cursor: Option<Pos>,
}

pub const JUMP_SIZE: i64 = 100;

impl Default for JumpList {
    fn default() -> Self {
        JumpList {
            pointer: -1,
            head: 0,
            tail: 0,
            buffer: vec![None; JUMP_SIZE as usize],
            cached_cursor: None,
        }
    }
}

/// `MacroModeState`.
#[derive(Clone, Debug, Default)]
pub struct MacroModeState {
    pub latest_register: Option<String>,
    pub is_playing: bool,
    pub is_recording: bool,
    pub replay_search_queries: Vec<String>,
    /// The "recording @x" notification is up.
    pub recording_dialog: bool,
    pub last_insert_mode_changes: InsertModeChanges,
}

/// A vim option (`defineOption`).
#[derive(Clone, Debug)]
pub struct VimOption {
    pub kind: &'static str,
    pub value: Option<OptValue>,
}

/// `lastCharacterSearch`.
#[derive(Clone, Debug, Default)]
pub struct LastCharacterSearch {
    pub increment: i64,
    pub forward: bool,
    pub selected_character: String,
}

/// An ex command definition (`commandMap_` entry).
#[derive(Clone, Debug, PartialEq)]
pub struct ExCommandDef {
    pub name: String,
    pub short_name: Option<String>,
    pub kind: ExKind,
    pub possibly_async: bool,
    pub exclude_from_history: bool,
    pub user: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExKind {
    Builtin,
    ExToKey(String),
    ExToEx(String),
}

/// The shared state (`vimGlobalState` plus the module-level tables).
pub struct VimGlobal {
    pub query: Option<super::search::VimRegex>,
    pub is_reversed: bool,
    pub last_substitute_replace_part: Option<String>,
    pub jump_list: JumpList,
    pub macro_mode_state: MacroModeState,
    pub last_character_search: LastCharacterSearch,
    pub registers: RegisterController,
    pub search_history: HistoryController,
    pub ex_history: HistoryController,
    pub keymap: Vec<VimKey>,
    pub default_keymap_len: usize,
    pub used_keys: HashMap<String, i64>,
    pub ex_commands: Vec<(String, ExCommandDef)>,
    pub options: HashMap<String, VimOption>,
    pub option_aliases: HashMap<String, String>,
    pub langmap: super::Langmap,
    pub key_to_key_stack: Vec<String>,
    pub noremap: bool,
    pub virtual_prompt: Option<super::prompt::PromptOptions>,
}

impl VimGlobal {
    fn new() -> VimGlobal {
        let keymap = super::keymap::default_keymap();
        let default_keymap_len = keymap.len();
        let mut g = VimGlobal {
            query: None,
            is_reversed: false,
            last_substitute_replace_part: None,
            jump_list: JumpList::default(),
            macro_mode_state: MacroModeState::default(),
            last_character_search: LastCharacterSearch {
                increment: 0,
                forward: true,
                selected_character: String::new(),
            },
            registers: RegisterController::default(),
            search_history: HistoryController::default(),
            ex_history: HistoryController::default(),
            keymap,
            default_keymap_len,
            used_keys: HashMap::new(),
            ex_commands: Vec::new(),
            options: HashMap::new(),
            option_aliases: HashMap::new(),
            langmap: super::Langmap::default(),
            key_to_key_stack: Vec::new(),
            noremap: false,
            virtual_prompt: None,
        };
        g.define_option("filetype", None, "string", &["ft"], true);
        g.define_option(
            "textwidth",
            Some(OptValue::Num(80)),
            "number",
            &["tw"],
            true,
        );
        g.define_option("langmap", None, "string", &["lmap"], true);
        g.define_option("pcre", Some(OptValue::Bool(true)), "boolean", &[], false);
        g.define_option(
            "insertModeEscKeysTimeout",
            Some(OptValue::Num(200)),
            "number",
            &[],
            false,
        );
        g.ex_commands = super::ex::default_ex_command_map();
        g
    }

    fn define_option(
        &mut self,
        name: &str,
        default: Option<OptValue>,
        kind: &'static str,
        aliases: &[&str],
        callback: bool,
    ) {
        self.options.insert(
            name.into(),
            VimOption {
                kind,
                value: if callback { None } else { default },
            },
        );
        for a in aliases {
            self.option_aliases.insert((*a).into(), name.into());
        }
    }

    /// The option record for a name or alias.
    pub fn option_name(&self, name: &str) -> Option<String> {
        if self.options.contains_key(name) {
            return Some(name.into());
        }
        self.option_aliases.get(name).cloned()
    }

    pub fn option_value(&self, name: &str) -> Option<OptValue> {
        self.options.get(name).and_then(|o| o.value.clone())
    }

    pub fn pcre(&self) -> bool {
        self.option_value("pcre") != Some(OptValue::Bool(false))
    }

    /// `setTimeout`'s delay from the option (`:set` stores strings).
    pub fn esc_keys_timeout(&self) -> u64 {
        match self.option_value("insertModeEscKeysTimeout") {
            Some(OptValue::Num(n)) => n.max(0) as u64,
            Some(OptValue::Str(s)) => s.trim().parse::<f64>().map_or(0, |n| n.max(0.0) as u64),
            _ => 0,
        }
    }

    /// `_mapCommand`.
    pub fn map_command(&mut self, command: VimKey) {
        for part in super::keymap::key_parts(&command.keys) {
            *self.used_keys.entry(part).or_insert(0) += 1;
        }
        self.keymap.insert(0, command);
    }

    pub fn remove_used_keys(&mut self, keys: &str) {
        for part in super::keymap::key_parts(keys) {
            if let Some(n) = self.used_keys.get_mut(&part)
                && *n > 0
            {
                *n -= 1;
            }
        }
    }

    pub fn key_used(&self, key: &str) -> bool {
        self.used_keys.get(key).is_some_and(|n| *n > 0)
    }
}

thread_local! {
    static GLOBAL: RefCell<Option<VimGlobal>> = const { RefCell::new(None) };
}

/// Run `f` with the shared vim state.
pub fn with_global<R>(f: impl FnOnce(&mut VimGlobal) -> R) -> R {
    GLOBAL.with(|g| {
        let mut g = g.borrow_mut();
        let g = g.get_or_insert_with(VimGlobal::new);
        f(g)
    })
}

/// `resetVimGlobalState` (tests).
#[cfg(test)]
pub fn reset_global() {
    GLOBAL.with(|g| *g.borrow_mut() = Some(VimGlobal::new()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_shift_and_append() {
        let mut r = RegisterController::default();
        r.push_text(None, "delete", "one\n", true, false);
        r.push_text(None, "delete", "two\n", true, false);
        assert_eq!(r.get_text(Some("1")), "two\n");
        assert_eq!(r.get_text(Some("2")), "one\n");
        r.push_text(None, "delete", "x", false, false);
        assert_eq!(r.get_text(Some("-")), "x");
        r.push_text(Some("a"), "yank", "aa", false, false);
        r.push_text(Some("A"), "yank", "bb", false, false);
        assert_eq!(r.get_text(Some("a")), "aabb");
        assert_eq!(r.get_text(None), "aabb");
        assert_eq!(r.push_text(Some("_"), "yank", "zz", false, false), None);
    }

    #[test]
    fn history_prefix_matching() {
        let mut h = HistoryController::default();
        h.push_input("foo");
        h.push_input("bar");
        h.push_input("fob");
        h.reset();
        assert_eq!(h.next_match("fo", true).as_deref(), Some("fob"));
        assert_eq!(h.next_match("fob", true).as_deref(), Some("foo"));
        assert_eq!(h.next_match("foo", false).as_deref(), Some("fob"));
        assert_eq!(h.next_match("fob", false).as_deref(), Some("fo"));
    }

    #[test]
    fn repeat_digits() {
        let mut s = InputState::default();
        s.push_repeat_digit("2");
        s.operator = Some(Operator::Delete);
        s.push_repeat_digit("3");
        assert_eq!(s.get_repeat(), 6);
    }
}
