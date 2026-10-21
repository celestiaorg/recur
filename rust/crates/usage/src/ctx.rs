//! Go `context.Context` for the fetchers: a cancellation token plus an
//! optional deadline that child steps narrow.

use std::fmt;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Why a context ended (Go `context.Canceled` / `DeadlineExceeded`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtxError {
    Canceled,
    DeadlineExceeded,
}

impl fmt::Display for CtxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CtxError::Canceled => "context canceled",
            CtxError::DeadlineExceeded => "context deadline exceeded",
        })
    }
}

impl std::error::Error for CtxError {}

#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub cancel: CancellationToken,
    pub deadline: Option<Instant>,
}

impl Ctx {
    /// No deadline, never canceled unless the token is.
    pub fn background() -> Self {
        Self::default()
    }

    /// A child that also ends `d` from now (or at the parent's deadline,
    /// whichever comes first).
    pub fn with_timeout(&self, d: Duration) -> Self {
        let at = Instant::now() + d;
        Self {
            cancel: self.cancel.child_token(),
            deadline: Some(self.deadline.map_or(at, |p| p.min(at))),
        }
    }

    /// A child that ends when `cancel` fires.
    pub fn with_cancel(&self) -> Self {
        Self {
            cancel: self.cancel.child_token(),
            deadline: self.deadline,
        }
    }

    /// Time left before the deadline; `None` without one.
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// Why the context ended, if it has.
    pub fn err(&self) -> Option<CtxError> {
        if self.cancel.is_cancelled() {
            return Some(CtxError::Canceled);
        }
        match self.deadline {
            Some(d) if Instant::now() >= d => Some(CtxError::DeadlineExceeded),
            _ => None,
        }
    }

    /// Resolves when the context ends, with the reason.
    pub async fn done(&self) -> CtxError {
        match self.deadline {
            Some(d) => tokio::select! {
                _ = self.cancel.cancelled() => CtxError::Canceled,
                _ = tokio::time::sleep_until(d) => CtxError::DeadlineExceeded,
            },
            None => {
                self.cancel.cancelled().await;
                CtxError::Canceled
            }
        }
    }

    /// Runs `fut` until it finishes or the context ends.
    pub async fn run<T>(&self, fut: impl Future<Output = T>) -> Result<T, CtxError> {
        if let Some(e) = self.err() {
            return Err(e);
        }
        tokio::select! {
            biased;
            e = self.done() => Err(e),
            v = fut => Ok(v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn child_deadline_never_outlives_parent() {
        let parent = Ctx::background().with_timeout(Duration::from_secs(1));
        let child = parent.with_timeout(Duration::from_secs(10));
        assert_eq!(child.deadline, parent.deadline);
        let short = parent.with_timeout(Duration::from_millis(10));
        assert!(short.deadline < parent.deadline);
        assert_eq!(
            short.run(std::future::pending::<()>()).await,
            Err(CtxError::DeadlineExceeded)
        );
        parent.cancel.cancel();
        assert_eq!(child.err(), Some(CtxError::Canceled));
    }
}
