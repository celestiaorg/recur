//! Local git: worktrees, branches, status, diffs, checkpoints, and highlighting
//! of diff hunks. Port of Go `internal/git`.
//!
//! Recur shells out to the system `git` (never a git library) so behavior and
//! edge cases match what agents and users see in a terminal. Every function
//! that runs git is `async` on tokio; cancel one by dropping its future.

mod branches;
mod checkpoint;
mod command;
mod diff;
mod error;
mod gopath;
mod ignore;
mod scm;
mod status;
#[cfg(test)]
mod testutil;
mod text;
mod version;
mod working_diff;
mod worktree;

pub use branches::*;
pub use checkpoint::*;
pub use command::{CANCEL_GRACE, Command, command};
pub use diff::*;
pub use error::{Error, ErrorKind, Result};
pub use ignore::*;
pub use recur_chroma::{highlight_files, lexer_name};
pub use recur_project::home::expand_home;
pub use scm::*;
pub use status::{StatusLetters, parse_status_porcelain, status_map};
pub use version::*;
pub use working_diff::*;
pub use worktree::*;
