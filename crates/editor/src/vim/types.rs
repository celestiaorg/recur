//! Vim's data types: CodeMirror 5 positions (line, char column) and the
//! argument records motions, operators and actions receive.

/// `Infinity` / `Number.MAX_VALUE` columns ("end of line").
pub const INF: i64 = i64::MAX / 4;

/// A CodeMirror 5 position: zero-based line and character column. Vim
/// arithmetic may push either out of range before clipping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pos {
    pub line: i64,
    pub ch: i64,
}

impl Pos {
    pub const fn new(line: i64, ch: i64) -> Pos {
        Pos { line, ch }
    }
}

/// `cursorIsBefore`.
pub fn before(a: Pos, b: Pos) -> bool {
    a.line < b.line || (a.line == b.line && a.ch < b.ch)
}

pub fn pmin(a: Pos, b: Pos) -> Pos {
    if before(a, b) { a } else { b }
}

pub fn pmax(a: Pos, b: Pos) -> Pos {
    if before(a, b) { b } else { a }
}

/// `cursorIsBetween`: `b` strictly between `a` and `c`.
pub fn between(a: Pos, b: Pos, c: Pos) -> bool {
    before(a, b) && before(b, c)
}

/// `offsetCursor`.
pub fn offset(p: Pos, line: i64, ch: i64) -> Pos {
    Pos::new(p.line + line, p.ch + ch)
}

/// An anchor/head pair (`CM5RangeInterface`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub anchor: Pos,
    pub head: Pos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    MoveToTopLine,
    MoveToMiddleLine,
    MoveToBottomLine,
    ExpandToLine,
    FindNext,
    FindAndSelectNextInclusive,
    GoToMark,
    MoveToOtherHighlightedEnd,
    JumpToMark,
    MoveByCharacters,
    MoveByLines,
    MoveByDisplayLines,
    MoveByPage,
    MoveByParagraph,
    MoveBySentence,
    MoveByScroll,
    MoveByWords,
    MoveTillCharacter,
    MoveToCharacter,
    MoveToSymbol,
    MoveToColumn,
    MoveToEol,
    MoveToFirstNonWhiteSpaceCharacter,
    MoveToMatchedSymbol,
    MoveToStartOfLine,
    MoveToLineOrEdgeOfDocument,
    MoveToStartOfDisplayLine,
    MoveToEndOfDisplayLine,
    TextObjectManipulation,
    RepeatLastCharacterSearch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Change,
    Delete,
    Indent,
    IndentAuto,
    HardWrap,
    ChangeCase,
    Yank,
    Rot13,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    JumpListWalk,
    Scroll,
    ScrollToCursor,
    ReplayMacro,
    EnterMacroRecordMode,
    ToggleOverwrite,
    EnterInsertMode,
    ToggleVisualMode,
    ReselectLastSelection,
    JoinLines,
    NewLineAndEnterInsertMode,
    Paste,
    Undo,
    Redo,
    SetRegister,
    InsertRegister,
    OneNormalCommand,
    SetMark,
    Replace,
    IncrementNumberToken,
    RepeatLastEdit,
    Indent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAt {
    Eol,
    Bol,
    CharAfter,
    FirstNonBlank,
    StartOfSelectedArea,
    EndOfSelectedArea,
    Inplace,
    LastEdit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollPos {
    Center,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuerySrc {
    Prompt,
    WordUnderCursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchArgs {
    pub forward: bool,
    pub query_src: QuerySrc,
    pub to_jumplist: bool,
    pub whole_word_only: bool,
}

/// `MotionArgs`. `forward` is optional: some motions read it as `undefined`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MotionArgs {
    pub forward: Option<bool>,
    pub linewise: bool,
    pub to_jumplist: bool,
    pub word_end: bool,
    pub big_word: bool,
    pub inclusive: bool,
    pub explicit_repeat: bool,
    pub repeat_offset: i64,
    pub to_first_char: bool,
    pub same_line: bool,
    pub text_object_inner: bool,
    pub no_repeat: bool,
    pub repeat: i64,
    pub repeat_is_explicit: bool,
    pub selected_character: Option<String>,
}

impl MotionArgs {
    pub fn fwd(&self) -> bool {
        self.forward.unwrap_or(false)
    }
}

/// The visual selection a visual operator ran on (`lastSel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastSel {
    pub anchor: Pos,
    pub head: Pos,
    pub visual_block: bool,
    pub visual_line: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OperatorArgs {
    pub indent_right: Option<bool>,
    pub to_lower: Option<bool>,
    pub keep_cursor: bool,
    pub linewise: bool,
    pub full_line: bool,
    pub should_move_cursor: bool,
    pub last_sel: Option<LastSel>,
    pub repeat: i64,
    pub register_name: Option<String>,
    pub selected_character: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActionArgs {
    pub insert_at: Option<InsertAt>,
    pub after: bool,
    pub is_edit: bool,
    pub match_indent: bool,
    pub linewise: bool,
    pub blockwise: bool,
    pub replace: bool,
    pub forward: bool,
    pub position: Option<ScrollPos>,
    pub increase: bool,
    pub backtrack: bool,
    pub keep_spaces: bool,
    pub indent_right: bool,
    pub repeat: i64,
    pub repeat_is_explicit: bool,
    pub register_name: Option<String>,
    pub selected_character: Option<String>,
    /// `enterInsertMode`'s `{head}` from `change`.
    pub head: Option<Pos>,
}

/// A motion's result: a head, or an anchor and head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionResult {
    Head(Pos),
    Range(Pos, Pos),
}
