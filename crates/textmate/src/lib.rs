//! TextMate grammar tokenizer and theme resolution used by the editor, diffs and chat code
//! blocks.
//!
//! The tokenizer is a port of vscode-textmate 9.3 over Oniguruma (the `onig`
//! crate, same engine and options as vscode-oniguruma). The rest ports
//! Recur's `lib/tm-*.ts`, `lib/codehl.ts`, the editor-independent parts of
//! `lib/cm-lang-data.ts` / `lib/cm-tm-*.ts` / `lib/syntax-mode.ts`, and
//! `internal/bundles/scope.go`. Grammars, themes, language data and symbol
//! rules are inputs; nothing here depends on the bundles crate.

mod catalog;
mod codehl;
mod delta;
mod engine;
mod fill;
mod fold;
mod grammar;
mod jsregex;
mod lang_data;
mod matcher;
mod node;
mod onig_lib;
mod paint;
mod plist;
mod raw;
mod registry;
mod rule;
mod scope;
mod scope_selector;
mod stacks;
mod symbols;
mod syntax_mode;
mod theme;
mod tm_theme;
mod utils;

pub use catalog::{
    GrammarInfo, GrammarLoader, SyntaxLanguage, ThemeInfo, ThemePreviewInfo, TmCatalog,
    extract_fold_marker_sources, language_display_name, pick_preferred_theme,
};
pub use codehl::{
    DiffKind, DiffRow, ShKind, diff_lines, highlight_code, highlight_html_code_blocks,
    highlight_line, highlight_line_spans, lang_from_code_class, lang_from_info, lang_from_path,
    looks_like_unified_diff, parse_unified_diff,
};
pub use delta::{
    ChangedRange, LineDoc, LineReplace, TmDocFingerprint, line_replace_from_changes,
    splice_line_map, tm_doc_fingerprint, tm_doc_id_for,
};
pub use engine::{
    FillChunk, FillJob, FillProgress, GrammarSourceLoader, GrammarSourceText, MAX_IDLE_DOCS,
    MAX_IDLE_LINES, ScopeToken, TmEngine, ViewportMode, ViewportResult,
};
pub use fill::{FillListener, FillSubscription, TmFill};
pub use fold::{
    CompiledFoldMarkers, FoldRange, TmFoldMarkers, compile_fold_markers, fold_by_indent,
    fold_by_markers, textmate_fold,
};
pub use grammar::{
    Grammar, StateStack, Token, TokenizeError, TokenizeLineResult, TokenizeLineResult2, meta,
};
pub use jsregex::JsRegex;
pub use lang_data::{
    BASE_BRACKETS, BlockComment, CloseBracketsConfig, CommentTokens, CompiledIndent, LanguageData,
    LanguageDataFields, PairNewline, SELF_PAIR_QUOTES, between_bracket_pair, between_tag_pair,
    close_brackets_of, comment_tokens_of, compile_indent, compile_regex, compute_indent,
    has_non_space, indent_string, indent_unit_width, inside_paired_quote, language_data,
    leading_indent, matches_pattern, newline_between_pairs,
};
pub use matcher::{Matcher, create_matchers};
pub use node::Node;
pub use onig_lib::{CaptureIndex, OnigError, OnigMatch, OnigScanner, OnigString};
pub use paint::{TokenPaint, paint_for_meta, pane_background};
pub use plist::{PlistError, parse_plist};
pub use raw::{GrammarParseError, RawGrammar, parse_raw_grammar};
pub use registry::{GrammarSource, LoadGrammarError, Registry};
pub use scope::{
    glob_match, match_file_glob, matches as scope_matches, path_match, scope_prefix_match,
};
pub use scope_selector::{CompiledSelector, compile_selector, selector_matches, selector_score};
pub use stacks::{LineTokens, RuleStack, TmStackCache, tokenize_line_range};
pub use symbols::{
    OutlineSymbol, SymbolHit, SymbolRule, apply_symbol_transform, best_symbol_rule,
    collect_from_engine, collect_symbols, filter_symbols, infer_symbol_kind, outline_open_file,
    qualify_go_receiver, symbol_at_line, symbol_lookup_name,
};
pub use syntax_mode::{
    FileTypeAssociation, MAX_SYNTAX_BY_FILE_TYPE, MAX_SYNTAX_BY_PATH, OrderedMap, SYNTAX_PLAIN,
    SyntaxMode, SyntaxModeMaps, file_type_association_for_path, normalize_syntax_path,
    parse_syntax_map, prune_syntax_maps,
};
pub use theme::{
    RawTheme, RawThemeSetting, ScopeStack, StyleAttributes, Theme, ThemeScope, font_style,
};
pub use tm_theme::{
    ParsedTmTheme, RootScopeBackgrounds, ThemeChrome, ThemeParseError, TmRule, UiTheme,
    composite_css_color, contrast_ratio, css_color_alpha, css_color_key, decode_background,
    decode_font_style, decode_foreground, is_css_color_dark, is_css_transparent,
    is_text_mate_text_scope, parse_theme_json, parse_theme_value, soften_css_color, theme_chrome,
    tm_font, with_css_alpha,
};
pub use utils::utf16_len;
