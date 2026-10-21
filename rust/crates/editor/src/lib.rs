//! Recur's code editor: a CodeMirror 6 model (state, transactions,
//! history, commands, search, folding, completion, snippets, merge) driving
//! a GPUI editor element with TextMate highlighting.

pub mod autocomplete;
pub mod bindings;
pub mod brackets;
pub mod change;
pub mod clock;
pub mod close_brackets;
pub mod color;
pub mod commands;
pub mod diff;
pub mod display;
pub mod element;
pub mod emacs;
pub mod fold;
pub mod gutters;
pub mod highlight;
pub mod history;
pub mod history_cache;
pub mod icons;
pub mod indent;
pub mod keyequiv;
pub mod keymap;
pub mod lint;
pub mod lsp;
pub mod markdown;
pub mod merge;
pub mod panels;
pub mod prefs;
pub mod search;
pub mod selection;
pub mod session;
pub mod snippet;
pub mod state;
pub mod text;
pub mod theme;
pub mod tokens;
pub mod tooltip_ui;
pub mod view;
pub mod view_input;
pub mod view_queries;
pub mod view_state;
pub mod vim;
