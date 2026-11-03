//! Forge-neutral PR rules shared by every forge and the engine's PR code:
//! state and attention, check rollups, participants and reviewers, times.

mod checks;
mod people;
mod state;
mod text;
mod time;

pub use checks::*;
pub use people::*;
pub use state::*;
pub(crate) use text::first_non_empty;
pub use text::{
    comment_avatar_css, commit_subjects, format_commit_bullet_list, hard_truncate_chars,
    html_escape, pr_description, single_line,
};
pub use time::*;
