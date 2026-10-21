//! Millisecond clock for transaction timestamps (CodeMirror's `Date.now()`
//! on `Transaction.time`), monotonic for the life of the process.

use std::sync::OnceLock;
use std::time::Instant;

fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Milliseconds since the first call in this process. Starts above zero so
/// "no previous time" (0) never looks recent.
pub fn now_ms() -> u64 {
    start().elapsed().as_millis() as u64 + 1_000_000
}
