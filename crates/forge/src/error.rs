use crate::cli::CmdError;

/// Coarse class of a forge failure. The message is the user-facing text,
/// so callers show it as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeErrorKind {
    /// The forge tool is missing or not authenticated.
    Unavailable,
    /// The request was rejected before anything ran (bad id, empty body).
    Invalid,
    /// An HTTP fetch answered with this status (image proxy).
    Http(u16),
    /// The command or API call failed.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeError {
    pub kind: ForgeErrorKind,
    pub message: String,
}

impl ForgeError {
    pub fn new(kind: ForgeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorKind::Failed, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorKind::Invalid, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorKind::Unavailable, message)
    }

    /// `<prefix>: <err>`.
    pub fn wrap(prefix: impl std::fmt::Display, err: impl std::fmt::Display) -> Self {
        Self::failed(format!("{prefix}: {err}"))
    }
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ForgeError {}

impl From<CmdError> for ForgeError {
    fn from(e: CmdError) -> Self {
        Self::failed(e.to_string())
    }
}
