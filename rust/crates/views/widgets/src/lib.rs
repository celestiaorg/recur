//! View pieces more than one view crate shows, below every view crate but `recur-view-core`:
//! the emoji and `@mention` typeaheads ([`suggest`]), finding rows and severity icons
//! ([`findings_ui`]), the chat's bordered pills ([`pill`]), a bundle command's prompts form
//! ([`command_prompts`]) and the byte-budgeted LRU the chat and Observatory park state in
//! ([`byte_lru`]).

pub mod avatar;
pub mod byte_lru;
pub mod chrome;
pub mod command_prompts;
pub mod file_index;
pub mod findings_ui;
pub mod keybinding_menu;
pub mod list_chrome;
pub mod people;
pub mod pill;
pub mod schedule_expanded;
pub mod schedules;
pub mod suggest;
