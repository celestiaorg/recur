//! Per-project configuration Recur reads from a checkout and the processes it
//! runs there: `.recur/project.toml` (read, overlay, comment-preserving
//! write), the workspace environment and dotenv files, and run-config
//! (setup/teardown commands and dev-server boot).
//!
//! Port of Go `internal/projfile`, `internal/wsenv` and `internal/runconfig`,
//! plus `git.ExpandHome` and the `os.UserHomeDir` / `os.UserConfigDir` rules
//! they and the store build on ([`home`]).

pub mod home;
pub mod projfile;
pub mod runconfig;
pub mod tomledit;
pub mod wsenv;

#[cfg(test)]
mod testenv;
