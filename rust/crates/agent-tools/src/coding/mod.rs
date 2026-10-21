//! The native-harness coding tools (not served over MCP): Read, Edit, Write,
//! Glob, Grep, Bash, TodoWrite.

pub mod bash;
pub mod edit;
#[cfg(feature = "code")]
pub mod glob;
#[cfg(feature = "code")]
pub mod grep;
pub mod read;
pub mod todos;
pub mod write;

use std::time::Duration;

pub(crate) const MAX_READ_LINES: usize = 2000;
pub(crate) const MAX_LINE_CHARS: usize = 2000;
pub(crate) const MAX_BASH_LINES: usize = 500;
#[cfg(feature = "code")]
pub(crate) const MAX_GREP_HITS: usize = 200;
pub(crate) const DEFAULT_BASH_WAIT: Duration = Duration::from_secs(2 * 60);
pub(crate) const MAX_BASH_WAIT: Duration = Duration::from_secs(10 * 60);

/// A NUL byte in the first 512 bytes.
pub(crate) fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(512)].contains(&0)
}

/// Truncates a line longer than `MAX_LINE_CHARS` runes, marking it with `…`.
pub(crate) fn clip_line(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

/// The managed data root (where a managed ripgrep lives).
#[cfg(feature = "code")]
pub(crate) fn data_dir() -> std::path::PathBuf {
    recur_store::data_dir().unwrap_or_default()
}
