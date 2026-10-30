//! Per-project configuration Recur reads from a checkout and the processes it
//! runs there: `.recur/project.toml` (read, overlay, comment-preserving
//! write), the workspace environment and dotenv files, and run-config
//! (setup/teardown commands and dev-server boot).
//!
//! [`home`] holds the `~` expansion and home / config dir rules they and
//! the store build on.

pub mod home;
pub mod projfile;
pub mod runconfig;
pub mod wsenv;

#[cfg(test)]
mod testenv;
