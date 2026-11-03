//! Classified phase failures: a failed agent turn keeps its classification and
//! the provider's message up to the walk, so a transient failure (D56) parks
//! the loop as retryable instead of failing it for good.

use std::error::Error;

use recur_agent::{TurnResult, error::classify, error::has_transient_marker};

use crate::turn::ERR_MCP_UNAVAILABLE;

/// The error a phase returns to the walker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopError {
    msg: String,
    turn: Option<TurnFailure>,
}

/// A classified agent-turn failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFailure {
    pub role: String,
    pub reason: String,
    pub transient: bool,
}

impl LoopError {
    /// A plain (unclassified) phase error.
    pub fn msg(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            turn: None,
        }
    }

    /// Classifies a failed agent turn.
    pub fn turn(role: &str, err: Option<&(dyn Error + 'static)>, res: &TurnResult) -> Self {
        let (reason, transient) = classify(err, res);
        let msg = if reason.is_empty() {
            format!("{role} turn failed")
        } else {
            format!("{role} turn failed: {reason}")
        };
        Self {
            msg,
            turn: Some(TurnFailure {
                role: role.to_owned(),
                reason,
                transient,
            }),
        }
    }

    pub fn turn_failure(&self) -> Option<&TurnFailure> {
        self.turn.as_ref()
    }
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl Error for LoopError {}

impl From<crate::EngineError> for LoopError {
    fn from(e: crate::EngineError) -> Self {
        Self::msg(e.0)
    }
}

/// Whether a phase error is a transient agent failure, and the reason to
/// record: a classified turn keeps its verdict, a failed MCP bind is transient
/// (the bind is retried), and any other error is scanned for the transient
/// markers.
pub fn transient_failure(err: &LoopError) -> (String, bool) {
    if let Some(t) = &err.turn {
        return (t.reason.clone(), t.transient);
    }
    if err.msg.contains(ERR_MCP_UNAVAILABLE) {
        return (err.msg.clone(), true);
    }
    if has_transient_marker(&err.msg) {
        return (err.msg.clone(), true);
    }
    (String::new(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_agent::ProviderError;
    use recur_agent::error::ErrorKind;

    #[test]
    fn turn_error_text_and_classification() {
        let err = ProviderError::new(ErrorKind::Failed, "provider overloaded");
        let res = TurnResult {
            is_error: true,
            ..TurnResult::default()
        };
        let e = LoopError::turn("Coder", Some(&err), &res);
        assert!(e.to_string().starts_with("Coder turn failed: "));
        let (reason, transient) = transient_failure(&e);
        assert!(transient);
        assert!(reason.contains("overloaded"));
    }

    #[test]
    fn plain_errors_scan_markers_and_mcp() {
        assert_eq!(
            transient_failure(&LoopError::msg("engine commit: boom")),
            (String::new(), false)
        );
        let mcp = LoopError::msg(format!("{ERR_MCP_UNAVAILABLE}: bind"));
        assert!(transient_failure(&mcp).1);
        assert!(transient_failure(&LoopError::msg("git: 503 Service Unavailable")).1);
    }
}
