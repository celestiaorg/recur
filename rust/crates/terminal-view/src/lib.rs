//! GPUI terminal view rendering `recur-term` sessions: the port of `TerminalPane.svelte` and its
//! xterm.js 6 configuration (fit, search, web-links and WebGL addons).

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod atlas;
mod backend;
mod blink;
#[rustfmt::skip]
mod custom_glyph_data;
mod custom_glyphs;
mod element;
mod find_bar;
mod font;
#[cfg(feature = "gallery")]
pub mod gallery;
mod park;
mod scrollbar;
mod theme;
#[cfg(target_os = "macos")]
mod tiles;
mod view;

pub use backend::TerminalBackend;
pub use element::gpui_color;
pub use find_bar::{FindBarEvent, TerminalFindBar};
pub use font::*;
pub use park::{Park, TERM_PARK_ENTRY_BYTES, TERM_PARK_MAX_BYTES};
pub use theme::{TerminalThemeInfo, ThemeContext, search_decorations, terminal_theme};
pub use view::{
    ACK_THRESHOLD, Copy, FindOptions, KEY_CONTEXT, Paste, Router, TerminalEvent, TerminalOptions,
    TerminalPark, TerminalView, WorkspaceFileDrag, discard_parked, find_count_label, init,
    is_external_http_url, park, prune_parked, quote_for_shell, resolve_explorer_drop_path,
    route_event, should_dispose, take_parked, terminal_drop_text,
};
