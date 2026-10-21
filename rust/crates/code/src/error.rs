use std::io;

use crate::gopath::errno_text;

/// Errors from worktree file access. `Display` is the Go error string, which
/// the UI sniffs for `not found: <rel>` and `worktree is not a directory`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Go `ErrNotFound`: the path genuinely does not exist under the root
    /// (deleted, renamed, never existed), as opposed to a transient failure.
    #[error("not found: {0}")]
    NotFound(String),
    /// Go `ErrWorktreeMissing`: the recorded worktree is no longer a
    /// directory. Callers must not fall back to the project checkout.
    #[error("worktree is not a directory: {0}")]
    WorktreeMissing(String),
    /// Go `context.Canceled`.
    #[error("context canceled")]
    Canceled,
    /// Go `context.DeadlineExceeded`.
    #[error("context deadline exceeded")]
    DeadlineExceeded,
    /// Go `*os.PathError`: `op path: errno`.
    #[error("{op} {path}: {}", errno_text(.source))]
    Path {
        op: &'static str,
        path: String,
        source: io::Error,
    },
    #[error(transparent)]
    Git(#[from] recur_git::Error),
    #[error("{0}")]
    Msg(String),
}

impl Error {
    pub(crate) fn msg(s: impl Into<String>) -> Self {
        Self::Msg(s.into())
    }

    pub(crate) fn path(op: &'static str, path: &str, source: io::Error) -> Self {
        Self::Path {
            op,
            path: path.to_string(),
            source,
        }
    }

    /// Go `os.IsNotExist`.
    pub fn is_not_exist(&self) -> bool {
        matches!(self, Self::Path { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
