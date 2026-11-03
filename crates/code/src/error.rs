use std::io;

/// Errors from worktree file access. `Display` is the error string, which
/// the UI sniffs for `not found: <rel>` and `worktree is not a directory`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The path genuinely does not exist under the root (deleted, renamed,
    /// never existed), as opposed to a transient failure.
    #[error("not found: {0}")]
    NotFound(String),
    /// The recorded worktree is no longer a directory. Callers must not fall
    /// back to the project checkout.
    #[error("worktree is not a directory: {0}")]
    WorktreeMissing(String),
    /// The caller cancelled.
    #[error("context canceled")]
    Canceled,
    /// The deadline passed.
    #[error("context deadline exceeded")]
    DeadlineExceeded,
    #[error("{path}: {source}")]
    Path { path: String, source: io::Error },
    #[error(transparent)]
    Git(#[from] recur_git::Error),
    #[error("{0}")]
    Msg(String),
}

impl Error {
    pub(crate) fn msg(s: impl Into<String>) -> Self {
        Self::Msg(s.into())
    }

    pub(crate) fn path(path: &str, source: io::Error) -> Self {
        Self::Path {
            path: path.to_string(),
            source,
        }
    }

    pub(crate) fn mkdir_all(path: &str) -> Result<()> {
        std::fs::create_dir_all(path).map_err(|e| Self::path(path, e))
    }

    /// Whether the error means the path does not exist.
    pub fn is_not_exist(&self) -> bool {
        matches!(self, Self::Path { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
