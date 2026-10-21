//! Vim's command table (`defaultKeymap`), key matching
//! (`commandMatches` / `commandMatch` / `lastChar`) and the key names it
//! reads from keyboard events (`vimKeyFromEvent`).

use super::types::*;

/// Which modes a binding applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctx {
    Normal,
    Insert,
    Visual,
    OperatorPending,
}

/// A command's kind and payload (`vimKey.type` and its fields).
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    KeyToKey {
        to_keys: String,
    },
    Motion {
        motion: Motion,
        args: MotionArgs,
    },
    Operator {
        operator: Operator,
        args: OperatorArgs,
    },
    OperatorMotion {
        operator: Operator,
        motion: Motion,
        motion_args: MotionArgs,
        operator_args: OperatorArgs,
        visual_line: Option<bool>,
    },
    Action {
        action: Action,
        args: Option<ActionArgs>,
        motion: Option<Motion>,
        motion_args: MotionArgs,
    },
    Search {
        args: SearchArgs,
    },
    Ex,
    Idle,
}

/// One keymap entry (`vimKey`).
#[derive(Clone, Debug, PartialEq)]
pub struct VimKey {
    pub keys: String,
    pub kind: Kind,
    pub context: Option<Ctx>,
    pub is_edit: bool,
    pub interlace_insert_repeat: bool,
    pub exit_visual_block: bool,
    pub repeat_override: Option<i64>,
    pub noremap: Option<bool>,
}

impl VimKey {
    pub fn new(keys: &str, kind: Kind) -> VimKey {
        VimKey {
            keys: keys.into(),
            kind,
            context: None,
            is_edit: false,
            interlace_insert_repeat: false,
            exit_visual_block: false,
            repeat_override: None,
            noremap: None,
        }
    }

    fn ctx(mut self, c: Ctx) -> VimKey {
        self.context = Some(c);
        self
    }

    fn edit(mut self) -> VimKey {
        self.is_edit = true;
        self
    }

    pub fn is_action(&self) -> bool {
        matches!(self.kind, Kind::Action { .. })
    }
}

fn k2k(keys: &str, to: &str) -> VimKey {
    VimKey::new(keys, Kind::KeyToKey { to_keys: to.into() })
}

fn motion(keys: &str, m: Motion, args: MotionArgs) -> VimKey {
    VimKey::new(keys, Kind::Motion { motion: m, args })
}

fn op(keys: &str, o: Operator, args: OperatorArgs) -> VimKey {
    VimKey::new(keys, Kind::Operator { operator: o, args })
}

fn opm(
    keys: &str,
    o: Operator,
    m: Motion,
    motion_args: MotionArgs,
    operator_args: OperatorArgs,
    visual_line: Option<bool>,
) -> VimKey {
    VimKey::new(
        keys,
        Kind::OperatorMotion {
            operator: o,
            motion: m,
            motion_args,
            operator_args,
            visual_line,
        },
    )
}

fn act(keys: &str, a: Action, args: Option<ActionArgs>) -> VimKey {
    VimKey::new(
        keys,
        Kind::Action {
            action: a,
            args,
            motion: None,
            motion_args: MotionArgs::default(),
        },
    )
}

fn act_m(keys: &str, a: Action, args: ActionArgs, m: Motion) -> VimKey {
    VimKey::new(
        keys,
        Kind::Action {
            action: a,
            args: Some(args),
            motion: Some(m),
            motion_args: MotionArgs::default(),
        },
    )
}

fn search(keys: &str, args: SearchArgs) -> VimKey {
    VimKey::new(keys, Kind::Search { args })
}

fn ma() -> MotionArgs {
    MotionArgs::default()
}

fn fw(forward: bool) -> MotionArgs {
    MotionArgs {
        forward: Some(forward),
        ..ma()
    }
}

fn aa() -> ActionArgs {
    ActionArgs::default()
}

fn oa() -> OperatorArgs {
    OperatorArgs::default()
}

/// `defaultKeymap`.
pub fn default_keymap() -> Vec<VimKey> {
    use Action as A;
    use Ctx::*;
    use Motion as M;
    use Operator as O;
    let ins = |at: InsertAt| {
        Some(ActionArgs {
            insert_at: Some(at),
            ..aa()
        })
    };
    vec![
        k2k("<Left>", "h"),
        k2k("<Right>", "l"),
        k2k("<Up>", "k"),
        k2k("<Down>", "j"),
        k2k("g<Up>", "gk"),
        k2k("g<Down>", "gj"),
        k2k("<Space>", "l"),
        k2k("<BS>", "h"),
        k2k("<Del>", "x"),
        k2k("<C-Space>", "W"),
        k2k("<C-BS>", "B"),
        k2k("<S-Space>", "w"),
        k2k("<S-BS>", "b"),
        k2k("<C-n>", "j"),
        k2k("<C-p>", "k"),
        k2k("<C-[>", "<Esc>"),
        k2k("<C-c>", "<Esc>"),
        k2k("<C-[>", "<Esc>").ctx(Insert),
        k2k("<C-c>", "<Esc>").ctx(Insert),
        k2k("<C-Esc>", "<Esc>"),
        k2k("<C-Esc>", "<Esc>").ctx(Insert),
        k2k("s", "cl").ctx(Normal),
        k2k("s", "c").ctx(Visual),
        k2k("S", "cc").ctx(Normal),
        k2k("S", "VdO").ctx(Visual),
        k2k("<Home>", "0"),
        k2k("<End>", "$"),
        k2k("<PageUp>", "<C-b>"),
        k2k("<PageDown>", "<C-f>"),
        k2k("<CR>", "j^").ctx(Normal),
        k2k("<Ins>", "i").ctx(Normal),
        act("<Ins>", A::ToggleOverwrite, None).ctx(Insert),
        // Motions
        motion(
            "H",
            M::MoveToTopLine,
            MotionArgs {
                linewise: true,
                to_jumplist: true,
                ..ma()
            },
        ),
        motion(
            "M",
            M::MoveToMiddleLine,
            MotionArgs {
                linewise: true,
                to_jumplist: true,
                ..ma()
            },
        ),
        motion(
            "L",
            M::MoveToBottomLine,
            MotionArgs {
                linewise: true,
                to_jumplist: true,
                ..ma()
            },
        ),
        motion("h", M::MoveByCharacters, fw(false)),
        motion("l", M::MoveByCharacters, fw(true)),
        motion(
            "j",
            M::MoveByLines,
            MotionArgs {
                linewise: true,
                ..fw(true)
            },
        ),
        motion(
            "k",
            M::MoveByLines,
            MotionArgs {
                linewise: true,
                ..fw(false)
            },
        ),
        motion("gj", M::MoveByDisplayLines, fw(true)),
        motion("gk", M::MoveByDisplayLines, fw(false)),
        motion(
            "w",
            M::MoveByWords,
            MotionArgs {
                word_end: false,
                ..fw(true)
            },
        ),
        motion(
            "W",
            M::MoveByWords,
            MotionArgs {
                word_end: false,
                big_word: true,
                ..fw(true)
            },
        ),
        motion(
            "e",
            M::MoveByWords,
            MotionArgs {
                word_end: true,
                inclusive: true,
                ..fw(true)
            },
        ),
        motion(
            "E",
            M::MoveByWords,
            MotionArgs {
                word_end: true,
                big_word: true,
                inclusive: true,
                ..fw(true)
            },
        ),
        motion(
            "b",
            M::MoveByWords,
            MotionArgs {
                word_end: false,
                ..fw(false)
            },
        ),
        motion(
            "B",
            M::MoveByWords,
            MotionArgs {
                word_end: false,
                big_word: true,
                ..fw(false)
            },
        ),
        motion(
            "ge",
            M::MoveByWords,
            MotionArgs {
                word_end: true,
                inclusive: true,
                ..fw(false)
            },
        ),
        motion(
            "gE",
            M::MoveByWords,
            MotionArgs {
                word_end: true,
                big_word: true,
                inclusive: true,
                ..fw(false)
            },
        ),
        motion(
            "{",
            M::MoveByParagraph,
            MotionArgs {
                to_jumplist: true,
                ..fw(false)
            },
        ),
        motion(
            "}",
            M::MoveByParagraph,
            MotionArgs {
                to_jumplist: true,
                ..fw(true)
            },
        ),
        motion("(", M::MoveBySentence, fw(false)),
        motion(")", M::MoveBySentence, fw(true)),
        motion("<C-f>", M::MoveByPage, fw(true)),
        motion("<C-b>", M::MoveByPage, fw(false)),
        motion(
            "<C-d>",
            M::MoveByScroll,
            MotionArgs {
                explicit_repeat: true,
                ..fw(true)
            },
        ),
        motion(
            "<C-u>",
            M::MoveByScroll,
            MotionArgs {
                explicit_repeat: true,
                ..fw(false)
            },
        ),
        motion(
            "gg",
            M::MoveToLineOrEdgeOfDocument,
            MotionArgs {
                explicit_repeat: true,
                linewise: true,
                to_jumplist: true,
                ..fw(false)
            },
        ),
        motion(
            "G",
            M::MoveToLineOrEdgeOfDocument,
            MotionArgs {
                explicit_repeat: true,
                linewise: true,
                to_jumplist: true,
                ..fw(true)
            },
        ),
        motion("g$", M::MoveToEndOfDisplayLine, ma()),
        motion("g^", M::MoveToStartOfDisplayLine, ma()),
        motion("g0", M::MoveToStartOfDisplayLine, ma()),
        motion("0", M::MoveToStartOfLine, ma()),
        motion("^", M::MoveToFirstNonWhiteSpaceCharacter, ma()),
        motion(
            "+",
            M::MoveByLines,
            MotionArgs {
                to_first_char: true,
                ..fw(true)
            },
        ),
        motion(
            "-",
            M::MoveByLines,
            MotionArgs {
                to_first_char: true,
                ..fw(false)
            },
        ),
        motion(
            "_",
            M::MoveByLines,
            MotionArgs {
                to_first_char: true,
                repeat_offset: -1,
                ..fw(true)
            },
        ),
        motion(
            "$",
            M::MoveToEol,
            MotionArgs {
                inclusive: true,
                ..ma()
            },
        ),
        motion(
            "%",
            M::MoveToMatchedSymbol,
            MotionArgs {
                inclusive: true,
                to_jumplist: true,
                ..ma()
            },
        ),
        motion(
            "f<character>",
            M::MoveToCharacter,
            MotionArgs {
                inclusive: true,
                ..fw(true)
            },
        ),
        motion("F<character>", M::MoveToCharacter, fw(false)),
        motion(
            "t<character>",
            M::MoveTillCharacter,
            MotionArgs {
                inclusive: true,
                ..fw(true)
            },
        ),
        motion("T<character>", M::MoveTillCharacter, fw(false)),
        motion(";", M::RepeatLastCharacterSearch, fw(true)),
        motion(",", M::RepeatLastCharacterSearch, fw(false)),
        motion(
            "'<register>",
            M::GoToMark,
            MotionArgs {
                to_jumplist: true,
                linewise: true,
                ..ma()
            },
        ),
        motion(
            "`<register>",
            M::GoToMark,
            MotionArgs {
                to_jumplist: true,
                ..ma()
            },
        ),
        motion("]`", M::JumpToMark, fw(true)),
        motion("[`", M::JumpToMark, fw(false)),
        motion(
            "]'",
            M::JumpToMark,
            MotionArgs {
                linewise: true,
                ..fw(true)
            },
        ),
        motion(
            "['",
            M::JumpToMark,
            MotionArgs {
                linewise: true,
                ..fw(false)
            },
        ),
        act(
            "]p",
            A::Paste,
            Some(ActionArgs {
                after: true,
                is_edit: true,
                match_indent: true,
                ..aa()
            }),
        )
        .edit(),
        act(
            "[p",
            A::Paste,
            Some(ActionArgs {
                after: false,
                is_edit: true,
                match_indent: true,
                ..aa()
            }),
        )
        .edit(),
        motion(
            "]<character>",
            M::MoveToSymbol,
            MotionArgs {
                to_jumplist: true,
                ..fw(true)
            },
        ),
        motion(
            "[<character>",
            M::MoveToSymbol,
            MotionArgs {
                to_jumplist: true,
                ..fw(false)
            },
        ),
        motion("|", M::MoveToColumn, ma()),
        motion("o", M::MoveToOtherHighlightedEnd, ma()).ctx(Visual),
        motion(
            "O",
            M::MoveToOtherHighlightedEnd,
            MotionArgs {
                same_line: true,
                ..ma()
            },
        )
        .ctx(Visual),
        // Operators
        op("d", O::Delete, oa()),
        op("y", O::Yank, oa()),
        op("c", O::Change, oa()),
        op("=", O::IndentAuto, oa()),
        op(
            ">",
            O::Indent,
            OperatorArgs {
                indent_right: Some(true),
                ..oa()
            },
        ),
        op(
            "<",
            O::Indent,
            OperatorArgs {
                indent_right: Some(false),
                ..oa()
            },
        ),
        op("g~", O::ChangeCase, oa()),
        op(
            "gu",
            O::ChangeCase,
            OperatorArgs {
                to_lower: Some(true),
                ..oa()
            },
        )
        .edit(),
        op(
            "gU",
            O::ChangeCase,
            OperatorArgs {
                to_lower: Some(false),
                ..oa()
            },
        )
        .edit(),
        motion(
            "n",
            M::FindNext,
            MotionArgs {
                to_jumplist: true,
                ..fw(true)
            },
        ),
        motion(
            "N",
            M::FindNext,
            MotionArgs {
                to_jumplist: true,
                ..fw(false)
            },
        ),
        motion("gn", M::FindAndSelectNextInclusive, fw(true)),
        motion("gN", M::FindAndSelectNextInclusive, fw(false)),
        op("gq", O::HardWrap, oa()),
        op(
            "gw",
            O::HardWrap,
            OperatorArgs {
                keep_cursor: true,
                ..oa()
            },
        ),
        op("g?", O::Rot13, oa()),
        // Operator-Motion dual commands
        opm(
            "x",
            O::Delete,
            M::MoveByCharacters,
            fw(true),
            oa(),
            Some(false),
        ),
        opm(
            "X",
            O::Delete,
            M::MoveByCharacters,
            fw(false),
            oa(),
            Some(true),
        ),
        opm(
            "D",
            O::Delete,
            M::MoveToEol,
            MotionArgs {
                inclusive: true,
                ..ma()
            },
            oa(),
            None,
        )
        .ctx(Normal),
        op(
            "D",
            O::Delete,
            OperatorArgs {
                linewise: true,
                ..oa()
            },
        )
        .ctx(Visual),
        opm(
            "Y",
            O::Yank,
            M::ExpandToLine,
            MotionArgs {
                linewise: true,
                ..ma()
            },
            oa(),
            None,
        )
        .ctx(Normal),
        op(
            "Y",
            O::Yank,
            OperatorArgs {
                linewise: true,
                ..oa()
            },
        )
        .ctx(Visual),
        opm(
            "C",
            O::Change,
            M::MoveToEol,
            MotionArgs {
                inclusive: true,
                ..ma()
            },
            oa(),
            None,
        )
        .ctx(Normal),
        op(
            "C",
            O::Change,
            OperatorArgs {
                linewise: true,
                ..oa()
            },
        )
        .ctx(Visual),
        opm(
            "~",
            O::ChangeCase,
            M::MoveByCharacters,
            fw(true),
            OperatorArgs {
                should_move_cursor: true,
                ..oa()
            },
            None,
        )
        .ctx(Normal),
        op("~", O::ChangeCase, oa()).ctx(Visual),
        opm("<C-u>", O::Delete, M::MoveToStartOfLine, ma(), oa(), None).ctx(Insert),
        opm(
            "<C-w>",
            O::Delete,
            M::MoveByWords,
            MotionArgs {
                word_end: false,
                ..fw(false)
            },
            oa(),
            None,
        )
        .ctx(Insert),
        VimKey::new("<C-w>", Kind::Idle).ctx(Normal),
        // Actions
        act(
            "<C-i>",
            A::JumpListWalk,
            Some(ActionArgs {
                forward: true,
                ..aa()
            }),
        ),
        act(
            "<C-o>",
            A::JumpListWalk,
            Some(ActionArgs {
                forward: false,
                ..aa()
            }),
        ),
        act(
            "<C-e>",
            A::Scroll,
            Some(ActionArgs {
                forward: true,
                linewise: true,
                ..aa()
            }),
        ),
        act(
            "<C-y>",
            A::Scroll,
            Some(ActionArgs {
                forward: false,
                linewise: true,
                ..aa()
            }),
        ),
        act("a", A::EnterInsertMode, ins(InsertAt::CharAfter))
            .edit()
            .ctx(Normal),
        act("A", A::EnterInsertMode, ins(InsertAt::Eol))
            .edit()
            .ctx(Normal),
        act("A", A::EnterInsertMode, ins(InsertAt::EndOfSelectedArea))
            .edit()
            .ctx(Visual),
        act("i", A::EnterInsertMode, ins(InsertAt::Inplace))
            .edit()
            .ctx(Normal),
        act("gi", A::EnterInsertMode, ins(InsertAt::LastEdit))
            .edit()
            .ctx(Normal),
        act("I", A::EnterInsertMode, ins(InsertAt::FirstNonBlank))
            .edit()
            .ctx(Normal),
        act("gI", A::EnterInsertMode, ins(InsertAt::Bol))
            .edit()
            .ctx(Normal),
        act("I", A::EnterInsertMode, ins(InsertAt::StartOfSelectedArea))
            .edit()
            .ctx(Visual),
        VimKey {
            interlace_insert_repeat: true,
            ..act(
                "o",
                A::NewLineAndEnterInsertMode,
                Some(ActionArgs {
                    after: true,
                    ..aa()
                }),
            )
            .edit()
            .ctx(Normal)
        },
        VimKey {
            interlace_insert_repeat: true,
            ..act(
                "O",
                A::NewLineAndEnterInsertMode,
                Some(ActionArgs {
                    after: false,
                    ..aa()
                }),
            )
            .edit()
            .ctx(Normal)
        },
        act("v", A::ToggleVisualMode, None),
        act(
            "V",
            A::ToggleVisualMode,
            Some(ActionArgs {
                linewise: true,
                ..aa()
            }),
        ),
        act(
            "<C-v>",
            A::ToggleVisualMode,
            Some(ActionArgs {
                blockwise: true,
                ..aa()
            }),
        ),
        act(
            "<C-q>",
            A::ToggleVisualMode,
            Some(ActionArgs {
                blockwise: true,
                ..aa()
            }),
        ),
        act("gv", A::ReselectLastSelection, None),
        act("J", A::JoinLines, None).edit(),
        act(
            "gJ",
            A::JoinLines,
            Some(ActionArgs {
                keep_spaces: true,
                ..aa()
            }),
        )
        .edit(),
        act(
            "p",
            A::Paste,
            Some(ActionArgs {
                after: true,
                is_edit: true,
                ..aa()
            }),
        )
        .edit(),
        act(
            "P",
            A::Paste,
            Some(ActionArgs {
                after: false,
                is_edit: true,
                ..aa()
            }),
        )
        .edit(),
        act("r<character>", A::Replace, None).edit(),
        act("@<register>", A::ReplayMacro, None),
        act("q<register>", A::EnterMacroRecordMode, None),
        act(
            "R",
            A::EnterInsertMode,
            Some(ActionArgs {
                replace: true,
                ..aa()
            }),
        )
        .edit()
        .ctx(Normal),
        VimKey {
            exit_visual_block: true,
            ..op(
                "R",
                O::Change,
                OperatorArgs {
                    linewise: true,
                    full_line: true,
                    ..oa()
                },
            )
            .ctx(Visual)
        },
        act("u", A::Undo, None).ctx(Normal),
        op(
            "u",
            O::ChangeCase,
            OperatorArgs {
                to_lower: Some(true),
                ..oa()
            },
        )
        .ctx(Visual)
        .edit(),
        op(
            "U",
            O::ChangeCase,
            OperatorArgs {
                to_lower: Some(false),
                ..oa()
            },
        )
        .ctx(Visual)
        .edit(),
        act("<C-r>", A::Redo, None),
        act("m<register>", A::SetMark, None),
        act("\"<register>", A::SetRegister, None),
        act("<C-r><register>", A::InsertRegister, None)
            .ctx(Insert)
            .edit(),
        act("<C-o>", A::OneNormalCommand, None).ctx(Insert),
        act(
            "zz",
            A::ScrollToCursor,
            Some(ActionArgs {
                position: Some(ScrollPos::Center),
                ..aa()
            }),
        ),
        act_m(
            "z.",
            A::ScrollToCursor,
            ActionArgs {
                position: Some(ScrollPos::Center),
                ..aa()
            },
            M::MoveToFirstNonWhiteSpaceCharacter,
        ),
        act(
            "zt",
            A::ScrollToCursor,
            Some(ActionArgs {
                position: Some(ScrollPos::Top),
                ..aa()
            }),
        ),
        act_m(
            "z<CR>",
            A::ScrollToCursor,
            ActionArgs {
                position: Some(ScrollPos::Top),
                ..aa()
            },
            M::MoveToFirstNonWhiteSpaceCharacter,
        ),
        act(
            "zb",
            A::ScrollToCursor,
            Some(ActionArgs {
                position: Some(ScrollPos::Bottom),
                ..aa()
            }),
        ),
        act_m(
            "z-",
            A::ScrollToCursor,
            ActionArgs {
                position: Some(ScrollPos::Bottom),
                ..aa()
            },
            M::MoveToFirstNonWhiteSpaceCharacter,
        ),
        act(".", A::RepeatLastEdit, None),
        act(
            "<C-a>",
            A::IncrementNumberToken,
            Some(ActionArgs {
                increase: true,
                backtrack: false,
                ..aa()
            }),
        )
        .edit(),
        act(
            "<C-x>",
            A::IncrementNumberToken,
            Some(ActionArgs {
                increase: false,
                backtrack: false,
                ..aa()
            }),
        )
        .edit(),
        act(
            "<C-t>",
            A::Indent,
            Some(ActionArgs {
                indent_right: true,
                ..aa()
            }),
        )
        .ctx(Insert),
        act(
            "<C-d>",
            A::Indent,
            Some(ActionArgs {
                indent_right: false,
                ..aa()
            }),
        )
        .ctx(Insert),
        // Text object motions
        motion("a<register>", M::TextObjectManipulation, ma()),
        motion(
            "i<register>",
            M::TextObjectManipulation,
            MotionArgs {
                text_object_inner: true,
                ..ma()
            },
        ),
        // Search
        search(
            "/",
            SearchArgs {
                forward: true,
                query_src: QuerySrc::Prompt,
                to_jumplist: true,
                whole_word_only: false,
            },
        ),
        search(
            "?",
            SearchArgs {
                forward: false,
                query_src: QuerySrc::Prompt,
                to_jumplist: true,
                whole_word_only: false,
            },
        ),
        search(
            "*",
            SearchArgs {
                forward: true,
                query_src: QuerySrc::WordUnderCursor,
                whole_word_only: true,
                to_jumplist: true,
            },
        ),
        search(
            "#",
            SearchArgs {
                forward: false,
                query_src: QuerySrc::WordUnderCursor,
                whole_word_only: true,
                to_jumplist: true,
            },
        ),
        search(
            "g*",
            SearchArgs {
                forward: true,
                query_src: QuerySrc::WordUnderCursor,
                to_jumplist: true,
                whole_word_only: false,
            },
        ),
        search(
            "g#",
            SearchArgs {
                forward: false,
                query_src: QuerySrc::WordUnderCursor,
                to_jumplist: true,
                whole_word_only: false,
            },
        ),
        // Ex command
        VimKey::new(":", Kind::Ex),
    ]
}

/// The result of matching the key buffer.
#[derive(Clone, Debug, PartialEq)]
pub enum Match {
    None,
    Partial { expect_literal_next: bool },
    Clear,
    Full(Box<VimKey>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum M1 {
    Full,
    Partial,
}

/// `commandMatch`.
fn command_match(pressed: &str, mapped: &str) -> Option<M1> {
    let is_char = mapped.ends_with("<character>");
    let is_reg = mapped.ends_with("<register>");
    if is_char || is_reg {
        let prefix_len = mapped.len() - if is_char { 11 } else { 10 };
        let pressed_prefix = &pressed[..floor_boundary(pressed, prefix_len.min(pressed.len()))];
        let mapped_prefix = &mapped[..prefix_len];
        if pressed_prefix == mapped_prefix && pressed.len() > prefix_len {
            Some(M1::Full)
        } else if mapped_prefix.starts_with(pressed_prefix) {
            Some(M1::Partial)
        } else {
            None
        }
    } else if pressed == mapped {
        Some(M1::Full)
    } else if mapped.starts_with(pressed) {
        Some(M1::Partial)
    } else {
        None
    }
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// `matchCommand`: `keys` against the keymap in `context`. `user_len` is
/// how many user mappings precede the defaults; a `noremap` expansion only
/// looks at the defaults.
pub fn match_command(
    keys: &str,
    keymap: &[VimKey],
    context: Ctx,
    operator_pending: bool,
    noremap: bool,
    default_len: usize,
) -> (Match, Option<String>) {
    let context = if operator_pending {
        Ctx::OperatorPending
    } else {
        context
    };
    let start = if noremap {
        keymap.len().saturating_sub(default_len)
    } else {
        0
    };
    let mut partial: Vec<&VimKey> = Vec::new();
    let mut full: Vec<&VimKey> = Vec::new();
    for command in &keymap[start..] {
        if context == Ctx::Insert && command.context != Some(Ctx::Insert) {
            continue;
        }
        if command.context.is_some_and(|c| c != context) {
            continue;
        }
        if operator_pending && command.is_action() {
            continue;
        }
        match command_match(keys, &command.keys) {
            Some(M1::Partial) => partial.push(command),
            Some(M1::Full) => full.push(command),
            None => {}
        }
    }
    let Some(best) = full.first() else {
        if !partial.is_empty() {
            return (
                Match::Partial {
                    expect_literal_next: partial.len() == 1
                        && partial[0].keys.ends_with("<character>"),
                },
                None,
            );
        }
        return (Match::None, None);
    };
    if best.keys.ends_with("<character>") || best.keys.ends_with("<register>") {
        let character = last_char(keys);
        if character.is_empty() || character.chars().count() > 1 {
            return (Match::Clear, None);
        }
        return (Match::Full(Box::new((*best).clone())), Some(character));
    }
    (Match::Full(Box::new((*best).clone())), None)
}

/// `lastChar`.
pub fn last_char(keys: &str) -> String {
    let selected = if keys.ends_with('>')
        && let Some(i) = keys.rfind('<')
        && !keys[i + 1..keys.len() - 1].contains('>')
        && keys.len() - i > 2
    {
        keys[i..].to_string()
    } else {
        keys.chars()
            .last()
            .map(|c| c.to_string())
            .unwrap_or_default()
    };
    if selected.chars().count() > 1 {
        return match selected.as_str() {
            "<CR>" | "<S-CR>" => "\n".into(),
            "<Space>" | "<S-Space>" => " ".into(),
            _ => String::new(),
        };
    }
    selected
}

/// Split a key sequence into keys (`/<(?:[CSMA]-)*\w+>|./gi`).
pub fn split_keys(keys: &str) -> Vec<String> {
    let chars: Vec<char> = keys.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<'
            && let Some(len) = special_key_len(&chars[i..])
        {
            out.push(chars[i..i + len].iter().collect());
            i += len;
            continue;
        }
        out.push(chars[i].to_string());
        i += 1;
    }
    out
}

/// Length of a `<(?:[CSMA]-)*\w+>` at the start of `s`.
fn special_key_len(s: &[char]) -> Option<usize> {
    let mut i = 1;
    while i + 1 < s.len()
        && matches!(s[i].to_ascii_uppercase(), 'C' | 'S' | 'M' | 'A')
        && s[i + 1] == '-'
    {
        i += 2;
    }
    let start = i;
    while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == '_') {
        i += 1;
    }
    (i > start && i < s.len() && s[i] == '>').then_some(i + 1)
}

/// Keys used by mappings (`addUsedKeys` splits).
pub fn key_parts(keys: &str) -> Vec<String> {
    split_keys(keys)
}

/// `vimKeyFromEvent` for a keystroke: `name` (`event.key`), the US base
/// key and modifiers. `literal` mirrors `vim.expectLiteralNext`; `used`
/// tells whether a non-ASCII key is mapped.
#[allow(clippy::too_many_arguments)]
pub fn vim_key_from_event(
    key: &str,
    base: Option<&str>,
    ctrl: bool,
    alt: bool,
    meta: bool,
    shift: bool,
    is_mac: bool,
    literal: bool,
    used: impl Fn(&str) -> bool,
    langmap: &super::Langmap,
) -> Option<String> {
    if matches!(
        key,
        "Shift" | "Alt" | "Command" | "Control" | "CapsLock" | "AltGraph" | "Dead" | "Unidentified"
    ) {
        return None;
    }
    let mut key = key.to_string();
    if key.len() > 1 && key.starts_with('n') {
        key = key.replace("Numpad", "");
    }
    key = match key.as_str() {
        "Return" | "Enter" => "CR".into(),
        "Backspace" => "BS".into(),
        "Delete" => "Del".into(),
        "Escape" => "Esc".into(),
        "Insert" => "Ins".into(),
        "ArrowLeft" => "Left".into(),
        "ArrowRight" => "Right".into(),
        "ArrowUp" => "Up".into(),
        "ArrowDown" => "Down".into(),
        " " => "Space".into(),
        _ => key,
    };
    let mut name = String::new();
    if ctrl {
        name.push_str("C-");
    }
    if alt {
        name.push_str("A-");
    }
    if meta {
        name.push_str("M-");
    }
    let one = key.chars().count() == 1;
    if is_mac && name == "A-" && one {
        name.clear();
    }
    if (!name.is_empty() || !one) && shift {
        name.push_str("S-");
    }
    if !literal && one {
        if let Some(mapped) = langmap.keymap.get(&key)
            && (langmap.remap_ctrl != Some(false) || name.is_empty())
        {
            key = mapped.clone();
        } else if key.chars().next().is_some_and(|c| c as u32 > 128) && !used(&key) {
            let code = base
                .and_then(|b| b.chars().last())
                .map(|c| c.to_string())
                .unwrap_or_default();
            let code = if shift {
                code.to_uppercase()
            } else {
                code.to_lowercase()
            };
            if !code.is_empty() {
                key = code;
                if name.is_empty() && alt {
                    name = "A-".into();
                }
            }
        }
    }
    name.push_str(&key);
    if name.chars().count() > 1 {
        name = format!("<{name}>");
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_and_characters() {
        let km = default_keymap();
        let n = km.len();
        assert!(matches!(
            match_command("d", &km, Ctx::Normal, false, false, n).0,
            Match::Full(_)
        ));
        assert!(matches!(
            match_command("g", &km, Ctx::Normal, false, false, n).0,
            Match::Partial { .. }
        ));
        let (m, c) = match_command("fx", &km, Ctx::Normal, false, false, n);
        assert!(matches!(m, Match::Full(_)));
        assert_eq!(c.as_deref(), Some("x"));
        assert!(matches!(
            match_command("f<Left>", &km, Ctx::Normal, false, false, n).0,
            Match::Clear
        ));
        assert_eq!(last_char("f<Space>"), " ");
        assert_eq!(
            split_keys("d<C-a>x<lt"),
            vec!["d", "<C-a>", "x", "<", "l", "t"]
        );
        assert!(matches!(
            match_command("u", &km, Ctx::Normal, true, false, n).0,
            Match::None
        ));
    }

    #[test]
    fn key_names() {
        let lm = crate::vim::Langmap::default();
        let k = |key: &str, ctrl, alt, shift| {
            vim_key_from_event(
                key,
                Some(&key.to_lowercase()),
                ctrl,
                alt,
                false,
                shift,
                true,
                false,
                |_| false,
                &lm,
            )
        };
        assert_eq!(k("a", false, false, false).as_deref(), Some("a"));
        assert_eq!(k("A", false, false, true).as_deref(), Some("A"));
        assert_eq!(k("a", true, false, false).as_deref(), Some("<C-a>"));
        assert_eq!(k("Escape", false, false, false).as_deref(), Some("<Esc>"));
        assert_eq!(k("Enter", false, false, true).as_deref(), Some("<S-CR>"));
        assert_eq!(
            k("ArrowLeft", false, false, false).as_deref(),
            Some("<Left>")
        );
        assert_eq!(
            vim_key_from_event(
                "ƒ",
                Some("f"),
                false,
                true,
                false,
                false,
                true,
                false,
                |_| false,
                &lm
            )
            .as_deref(),
            Some("<A-f>")
        );
    }
}
