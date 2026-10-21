use std::fmt;
use std::process::ExitStatus;

use crate::worktree::DirtyBaseError;

/// A git operation failure. The message reads like the Go error chain
/// (`git add: fatal: …: exit status 128`); [`ErrorKind`] keeps what callers
/// branch on, the way Go code used `errors.Is` / `errors.As`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    message: String,
    kind: ErrorKind,
}

#[derive(Debug, Clone)]
pub enum ErrorKind {
    Other,
    /// The `git` binary is not on PATH.
    NotFound,
    /// git ran and exited unsuccessfully (non-zero or killed by a signal).
    /// `stderr` is filled by [`crate::Command::output`], like Go's `ExitError.Stderr`.
    Exit {
        status: ExitStatus,
        stderr: Vec<u8>,
    },
    /// Spawning or talking to git failed.
    Io(std::io::ErrorKind),
    /// `git version` ran but could not be parsed (Go `ErrUnparsed`).
    Unparsed,
    /// A local merge would overwrite uncommitted changes in the base checkout.
    DirtyBase(DirtyBaseError),
}

impl Error {
    pub(crate) fn msg(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ErrorKind::Other,
        }
    }

    pub(crate) fn with_kind(message: impl Into<String>, kind: ErrorKind) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    /// Go `*os.PathError` text (`op path: errno`) for an io failure.
    pub(crate) fn io_path(op: &str, path: &str, e: &std::io::Error) -> Self {
        Self {
            message: crate::text::path_error(op, path, e),
            kind: ErrorKind::Io(e.kind()),
        }
    }

    /// Prefixes the message (`prefix: message`), keeping the kind, like Go `fmt.Errorf("prefix: %w", err)`.
    pub(crate) fn wrap(self, prefix: impl fmt::Display) -> Self {
        Self {
            message: format!("{prefix}: {}", self.message),
            kind: self.kind,
        }
    }

    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn dirty_base(&self) -> Option<&DirtyBaseError> {
        match &self.kind {
            ErrorKind::DirtyBase(d) => Some(d),
            _ => None,
        }
    }

    pub fn is_unparsed(&self) -> bool {
        matches!(self.kind, ErrorKind::Unparsed)
    }

    /// git's captured stderr when this is an exit failure from [`crate::Command::output`].
    pub fn stderr(&self) -> &[u8] {
        match &self.kind {
            ErrorKind::Exit { stderr, .. } => stderr,
            _ => &[],
        }
    }
}

impl From<DirtyBaseError> for Error {
    fn from(d: DirtyBaseError) -> Self {
        Self {
            message: d.to_string(),
            kind: ErrorKind::DirtyBase(d),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self {
            message: e.to_string(),
            kind: ErrorKind::Io(e.kind()),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
