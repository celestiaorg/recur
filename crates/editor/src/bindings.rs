//! The editor's commands and the code and merge editors' keymaps, in
//! CodeMirror precedence order (highest first, then extension order).

use crate::keymap::KeyBinding;

/// Every command a key can reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cmd {
    // @codemirror/commands
    CursorCharLeft,
    CursorCharRight,
    SelectCharLeft,
    SelectCharRight,
    CursorGroupLeft,
    CursorGroupRight,
    SelectGroupLeft,
    SelectGroupRight,
    CursorLineBoundaryLeft,
    CursorLineBoundaryRight,
    SelectLineBoundaryLeft,
    SelectLineBoundaryRight,
    CursorLineBoundaryBackward,
    CursorLineBoundaryForward,
    SelectLineBoundaryBackward,
    SelectLineBoundaryForward,
    CursorLineUp,
    CursorLineDown,
    SelectLineUp,
    SelectLineDown,
    CursorDocStart,
    CursorDocEnd,
    SelectDocStart,
    SelectDocEnd,
    CursorPageUp,
    CursorPageDown,
    SelectPageUp,
    SelectPageDown,
    CursorLineStart,
    CursorLineEnd,
    SelectLineStart,
    SelectLineEnd,
    CursorSyntaxLeft,
    CursorSyntaxRight,
    SelectSyntaxLeft,
    SelectSyntaxRight,
    MoveLineUp,
    MoveLineDown,
    CopyLineUp,
    CopyLineDown,
    AddCursorAbove,
    AddCursorBelow,
    SimplifySelection,
    InsertBlankLine,
    SelectLine,
    SelectParentSyntax,
    IndentLess,
    IndentMore,
    IndentSelection,
    DeleteLine,
    CursorMatchingBracket,
    ToggleComment,
    ToggleBlockComment,
    ToggleTabFocusMode,
    InsertNewlineAndIndent,
    SelectAll,
    DeleteCharBackward,
    DeleteCharForward,
    DeleteGroupBackward,
    DeleteGroupForward,
    DeleteLineBoundaryBackward,
    DeleteLineBoundaryForward,
    DeleteToLineEnd,
    SplitLine,
    TransposeChars,
    Undo,
    Redo,
    UndoSelection,
    RedoSelection,
    // @codemirror/language
    FoldCode,
    UnfoldCode,
    FoldAll,
    UnfoldAll,
    // @codemirror/search + Recur panel
    OpenSearchPanel,
    FindNext,
    FindPrevious,
    CloseSearchPanel,
    SelectSelectionMatches,
    SelectNextOccurrence,
    GotoLine,
    ToggleCase,
    ToggleRegexp,
    ToggleWholeWord,
    // @codemirror/autocomplete
    DeleteBracketPair,
    StartCompletion,
    CloseCompletion,
    CompletionDown,
    CompletionUp,
    CompletionPageDown,
    CompletionPageUp,
    AcceptCompletion,
    NextSnippetField,
    PrevSnippetField,
    ClearSnippet,
    // Recur
    InsertNewlineBetweenPairs,
    ExpandTabSnippet,
    Save,
    GotoDefinition,
    RevertHunk,
    // @codemirror/lsp-client
    FormatDocument,
    RenameSymbol,
    ShowSignatureHelp,
    PrevSignature,
    NextSignature,
}

use Cmd::*;

fn k(key: &'static str, run: Cmd) -> KeyBinding<Cmd> {
    KeyBinding::new(key, run)
}

/// `emacsStyleKeymap` (macOS only inside `standardKeymap`).
fn emacs_style() -> Vec<KeyBinding<Cmd>> {
    vec![
        KeyBinding::mac_only("Ctrl-b", CursorCharLeft).shift(SelectCharLeft),
        KeyBinding::mac_only("Ctrl-f", CursorCharRight).shift(SelectCharRight),
        KeyBinding::mac_only("Ctrl-p", CursorLineUp).shift(SelectLineUp),
        KeyBinding::mac_only("Ctrl-n", CursorLineDown).shift(SelectLineDown),
        KeyBinding::mac_only("Ctrl-a", CursorLineStart).shift(SelectLineStart),
        KeyBinding::mac_only("Ctrl-e", CursorLineEnd).shift(SelectLineEnd),
        KeyBinding::mac_only("Ctrl-d", DeleteCharForward),
        KeyBinding::mac_only("Ctrl-h", DeleteCharBackward),
        KeyBinding::mac_only("Ctrl-k", DeleteToLineEnd),
        KeyBinding::mac_only("Ctrl-Alt-h", DeleteGroupBackward),
        KeyBinding::mac_only("Ctrl-o", SplitLine),
        KeyBinding::mac_only("Ctrl-t", TransposeChars),
        KeyBinding::mac_only("Ctrl-v", CursorPageDown),
    ]
}

/// `standardKeymap`.
fn standard() -> Vec<KeyBinding<Cmd>> {
    let mut v = vec![
        k("ArrowLeft", CursorCharLeft)
            .shift(SelectCharLeft)
            .prevent_default(),
        k("Mod-ArrowLeft", CursorGroupLeft)
            .mac("Alt-ArrowLeft")
            .shift(SelectGroupLeft)
            .prevent_default(),
        KeyBinding::mac_only("Cmd-ArrowLeft", CursorLineBoundaryLeft)
            .shift(SelectLineBoundaryLeft)
            .prevent_default(),
        k("ArrowRight", CursorCharRight)
            .shift(SelectCharRight)
            .prevent_default(),
        k("Mod-ArrowRight", CursorGroupRight)
            .mac("Alt-ArrowRight")
            .shift(SelectGroupRight)
            .prevent_default(),
        KeyBinding::mac_only("Cmd-ArrowRight", CursorLineBoundaryRight)
            .shift(SelectLineBoundaryRight)
            .prevent_default(),
        k("ArrowUp", CursorLineUp)
            .shift(SelectLineUp)
            .prevent_default(),
        KeyBinding::mac_only("Cmd-ArrowUp", CursorDocStart).shift(SelectDocStart),
        KeyBinding::mac_only("Ctrl-ArrowUp", CursorPageUp).shift(SelectPageUp),
        k("ArrowDown", CursorLineDown)
            .shift(SelectLineDown)
            .prevent_default(),
        KeyBinding::mac_only("Cmd-ArrowDown", CursorDocEnd).shift(SelectDocEnd),
        KeyBinding::mac_only("Ctrl-ArrowDown", CursorPageDown).shift(SelectPageDown),
        k("PageUp", CursorPageUp).shift(SelectPageUp),
        k("PageDown", CursorPageDown).shift(SelectPageDown),
        k("Home", CursorLineBoundaryBackward)
            .shift(SelectLineBoundaryBackward)
            .prevent_default(),
        k("Mod-Home", CursorDocStart).shift(SelectDocStart),
        k("End", CursorLineBoundaryForward)
            .shift(SelectLineBoundaryForward)
            .prevent_default(),
        k("Mod-End", CursorDocEnd).shift(SelectDocEnd),
        k("Enter", InsertNewlineAndIndent).shift(InsertNewlineAndIndent),
        k("Mod-a", SelectAll),
        k("Backspace", DeleteCharBackward)
            .shift(DeleteCharBackward)
            .prevent_default(),
        k("Delete", DeleteCharForward).prevent_default(),
        k("Mod-Backspace", DeleteGroupBackward)
            .mac("Alt-Backspace")
            .prevent_default(),
        k("Mod-Delete", DeleteGroupForward)
            .mac("Alt-Delete")
            .prevent_default(),
        KeyBinding::mac_only("Mod-Backspace", DeleteLineBoundaryBackward).prevent_default(),
        KeyBinding::mac_only("Mod-Delete", DeleteLineBoundaryForward).prevent_default(),
    ];
    v.extend(emacs_style());
    v
}

/// `recurDefaultKeymap`: ⌃⇧↑/↓ add cursors, then `defaultKeymap` without
/// Mod-Alt-ArrowUp/Down (workspace cycling).
fn recur_default() -> Vec<KeyBinding<Cmd>> {
    let mut v = vec![
        k("Ctrl-Shift-ArrowUp", AddCursorAbove),
        k("Ctrl-Shift-ArrowDown", AddCursorBelow),
    ];
    v.extend([
        k("Alt-ArrowLeft", CursorSyntaxLeft)
            .mac("Ctrl-ArrowLeft")
            .shift(SelectSyntaxLeft),
        k("Alt-ArrowRight", CursorSyntaxRight)
            .mac("Ctrl-ArrowRight")
            .shift(SelectSyntaxRight),
        k("Alt-ArrowUp", MoveLineUp),
        k("Shift-Alt-ArrowUp", CopyLineUp),
        k("Alt-ArrowDown", MoveLineDown),
        k("Shift-Alt-ArrowDown", CopyLineDown),
        k("Escape", SimplifySelection),
        k("Mod-Enter", InsertBlankLine),
        k("Alt-l", SelectLine).mac("Ctrl-l"),
        k("Mod-i", SelectParentSyntax).prevent_default(),
        k("Mod-[", IndentLess),
        k("Mod-]", IndentMore),
        k("Mod-Alt-\\", IndentSelection),
        k("Shift-Mod-k", DeleteLine),
        k("Shift-Mod-\\", CursorMatchingBracket),
        k("Mod-/", ToggleComment),
        k("Alt-A", ToggleBlockComment),
        k("Ctrl-m", ToggleTabFocusMode).mac("Shift-Alt-m"),
    ]);
    v.extend(standard());
    v
}

/// `recurSearchKeymap`.
fn recur_search() -> Vec<KeyBinding<Cmd>> {
    vec![
        k("Mod-f", OpenSearchPanel).scope("editor search-panel"),
        k("F3", FindNext)
            .shift(FindPrevious)
            .scope("editor search-panel")
            .prevent_default(),
        k("Mod-g", FindNext)
            .shift(FindPrevious)
            .scope("editor search-panel")
            .prevent_default(),
        k("Escape", CloseSearchPanel).scope("editor search-panel"),
        k("Mod-Shift-l", SelectSelectionMatches),
        k("Mod-d", SelectNextOccurrence).prevent_default(),
        k("Mod-Alt-g", GotoLine),
        k("Mod-Alt-c", ToggleCase).scope("search-panel"),
        k("Mod-Alt-r", ToggleRegexp).scope("search-panel"),
        k("Mod-Alt-w", ToggleWholeWord).scope("search-panel"),
    ]
}

fn history() -> Vec<KeyBinding<Cmd>> {
    vec![
        k("Mod-z", Undo).prevent_default(),
        k("Mod-y", Redo).mac("Mod-Shift-z").prevent_default(),
        KeyBinding {
            key: None,
            linux: Some("Ctrl-Shift-z"),
            ..k("", Redo)
        }
        .prevent_default(),
        k("Mod-u", UndoSelection).prevent_default(),
        k("Alt-u", RedoSelection)
            .mac("Mod-Shift-u")
            .prevent_default(),
    ]
}

fn fold() -> Vec<KeyBinding<Cmd>> {
    vec![
        k("Ctrl-Shift-[", FoldCode).mac("Cmd-Alt-["),
        k("Ctrl-Shift-]", UnfoldCode).mac("Cmd-Alt-]"),
        k("Ctrl-Alt-[", FoldAll),
        k("Ctrl-Alt-]", UnfoldAll),
    ]
}

fn completion() -> Vec<KeyBinding<Cmd>> {
    vec![
        k("Ctrl-Space", StartCompletion),
        KeyBinding::mac_only("Alt-`", StartCompletion),
        KeyBinding::mac_only("Alt-i", StartCompletion),
        k("Escape", CloseCompletion),
        k("ArrowDown", CompletionDown),
        k("ArrowUp", CompletionUp),
        k("PageDown", CompletionPageDown),
        k("PageUp", CompletionPageUp),
        k("Enter", AcceptCompletion),
    ]
}

/// The snippet keymap CodeMirror installs (highest precedence) once a
/// snippet is expanded.
fn snippet_highest() -> Vec<KeyBinding<Cmd>> {
    vec![
        k("Tab", NextSnippetField).shift(PrevSnippetField),
        k("Escape", ClearSnippet),
    ]
}

/// `formatKeymap` + Recur's F2 rename (LSP attached only).
fn lsp() -> Vec<KeyBinding<Cmd>> {
    vec![
        k("Shift-Alt-f", FormatDocument).prevent_default(),
        k("F2", RenameSymbol),
    ]
}

/// `signatureKeymap` (Prec.high, LSP attached only).
fn signature() -> Vec<KeyBinding<Cmd>> {
    vec![
        k("Mod-Shift-Space", ShowSignatureHelp),
        k("Mod-Shift-ArrowUp", PrevSignature),
        k("Mod-Shift-ArrowDown", NextSignature),
    ]
}

/// The code editor's bindings. `lsp`: an LSP client is attached.
pub fn code_editor_keymap(lsp_attached: bool) -> Vec<KeyBinding<Cmd>> {
    let mut v = snippet_highest();
    v.push(k("Enter", InsertNewlineBetweenPairs));
    if lsp_attached {
        v.extend(signature());
    }
    v.push(KeyBinding {
        key: Some("Backspace"),
        ..k("Backspace", DeleteBracketPair)
    });
    v.extend(completion());
    v.extend(recur_default());
    v.extend(recur_search());
    v.extend(history());
    v.extend(fold());
    v.push(k("Tab", NextSnippetField).shift(PrevSnippetField));
    v.push(k("Tab", ExpandTabSnippet));
    v.push(k("Tab", IndentMore).shift(IndentLess));
    v.push(k("Mod-/", ToggleComment));
    v.push(k("Mod-d", SelectNextOccurrence));
    v.push(k("Mod-s", Save).prevent_default());
    v.push(k("F12", GotoDefinition));
    v.push(k("Mod-f", OpenSearchPanel));
    v.push(k("Mod-Alt-g", GotoLine));
    v.push(k("Escape", CloseSearchPanel));
    if lsp_attached {
        v.extend(lsp());
    }
    v
}

/// The merge editor's bindings (no completion or closeBrackets).
pub fn merge_editor_keymap() -> Vec<KeyBinding<Cmd>> {
    let mut v = snippet_highest();
    v.push(k("Enter", InsertNewlineBetweenPairs));
    v.push(k("Tab", NextSnippetField).shift(PrevSnippetField));
    v.push(k("Tab", ExpandTabSnippet));
    v.push(k("Tab", IndentMore).shift(IndentLess));
    v.extend(recur_default());
    v.extend(recur_search());
    v.extend(history());
    v.extend(fold());
    v.push(k("Mod-/", ToggleComment));
    v.push(k("Mod-d", SelectNextOccurrence));
    v.push(k("Mod-s", Save));
    v.push(k("Mod-f", OpenSearchPanel));
    v.push(k("Mod-Alt-g", GotoLine));
    v.push(k("Mod-Alt-z", RevertHunk));
    v.push(k("Escape", CloseSearchPanel));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{KeyEvent, Keymap, Platform};
    use gpui_kit::Keystroke;

    fn first(km: &Keymap<Cmd>, key: &'static str, scope: &str) -> Vec<Cmd> {
        let mut ran = Vec::new();
        km.run(
            &KeyEvent::from_keystroke(&Keystroke::parse(key).unwrap(), Platform::Mac),
            scope,
            Platform::Mac,
            |c| {
                ran.push(*c);
                false
            },
        );
        ran
    }

    #[test]
    fn precedence_matches_codemirror() {
        let km = Keymap::build(&code_editor_keymap(false), Platform::Mac);
        assert_eq!(
            first(&km, "enter", "editor"),
            vec![
                InsertNewlineBetweenPairs,
                AcceptCompletion,
                InsertNewlineAndIndent
            ]
        );
        assert_eq!(
            first(&km, "tab", "editor"),
            vec![NextSnippetField, ExpandTabSnippet, IndentMore]
        );
        assert_eq!(
            first(&km, "escape", "editor"),
            vec![
                ClearSnippet,
                CloseCompletion,
                SimplifySelection,
                CloseSearchPanel
            ]
        );
        assert_eq!(first(&km, "cmd-shift-z", "editor"), vec![Redo]);
        assert_eq!(
            first(&km, "backspace", "editor"),
            vec![DeleteBracketPair, DeleteCharBackward]
        );
        assert_eq!(first(&km, "cmd-alt-c", "search-panel"), vec![ToggleCase]);
        assert_eq!(
            first(&km, "ctrl-shift-up", "editor"),
            vec![AddCursorAbove, SelectPageUp]
        );
        let merge = Keymap::build(&merge_editor_keymap(), Platform::Mac);
        assert_eq!(first(&merge, "cmd-alt-z", "editor"), vec![RevertHunk]);
    }
}
