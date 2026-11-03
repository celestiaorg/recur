//! The one error every service method can return.

/// A failed service call. Views show [`Error::message`]; some match on its stable wording, e.g.
/// "worktree is not a directory" for a missing worktree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Wraps any error by its display text.
    pub fn from_display(err: impl std::fmt::Display) -> Self {
        Self::new(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
