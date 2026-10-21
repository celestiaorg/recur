//! GPUI terminal view rendering `recur-term` sessions: the port of `TerminalPane.svelte` and its
//! xterm.js 6 configuration (fit, search, web-links and WebGL addons).

mod backend;
mod blink;
mod font;
mod park;
mod scrollbar;
mod theme;
mod tokens;

pub use backend::TerminalBackend;
pub use font::*;
pub use park::{Park, TERM_PARK_ENTRY_BYTES, TERM_PARK_MAX_BYTES};
pub use theme::{TerminalThemeInfo, ThemeContext, search_decorations, terminal_theme};
pub use tokens::{Tokens, is_light_theme, tokens};
