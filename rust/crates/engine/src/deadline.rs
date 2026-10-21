//! Go `context.WithTimeout` for sync walks that take a cancellation token:
//! a token that fires after `d`.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// A token cancelled after `d`. Outside a tokio runtime it never fires (the
/// walk runs to completion, as with Go's `context.Background()`).
pub fn deadline_token(d: Duration) -> CancellationToken {
    let tok = CancellationToken::new();
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let t = tok.clone();
        rt.spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(d) => t.cancel(),
                _ = t.cancelled() => {}
            }
        });
    }
    tok
}
