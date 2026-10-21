//! Recur's code editor: a CodeMirror 6 model (state, transactions,
//! history, commands, search, folding, completion, snippets, merge) driving
//! a GPUI editor element with TextMate highlighting.

pub mod autocomplete;
pub mod brackets;
pub mod change;
pub mod clock;
pub mod close_brackets;
pub mod commands;
pub mod diff;
pub mod fold;
pub mod gutters;
pub mod history;
pub mod history_cache;
pub mod indent;
pub mod keyequiv;
pub mod keymap;
pub mod lint;
pub mod merge;
pub mod prefs;
pub mod search;
pub mod selection;
pub mod session;
pub mod snippet;
pub mod state;
pub mod text;
pub mod view_queries;
pub mod view_state;
