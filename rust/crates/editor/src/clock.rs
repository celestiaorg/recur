//! Millisecond clock for transaction timestamps (CodeMirror's `Date.now()`
//! on `Transaction.time`), monotonic for the life of the thread. Views
//! install their executor's clock so deadlines agree with GPUI timers
//! (including the test platform's simulated time).

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

type Source = Box<dyn Fn() -> Instant>;

thread_local! {
    static SOURCE: RefCell<Option<Source>> = const { RefCell::new(None) };
    /// The source's reading when installed, and the clock value then.
    static START: RefCell<Option<(Instant, u64)>> = const { RefCell::new(None) };
}

fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Read time from `source` (an executor's `now`) on this thread, once;
/// the clock continues from its current value.
pub fn install_source(source: impl Fn() -> Instant + 'static) {
    if SOURCE.with(|s| s.borrow().is_some()) {
        return;
    }
    let base = now_ms();
    START.with(|s| *s.borrow_mut() = Some((source(), base)));
    SOURCE.with(|s| *s.borrow_mut() = Some(Box::new(source)));
}

/// Milliseconds since the clock started. Starts above zero so "no previous
/// time" (0) never looks recent.
pub fn now_ms() -> u64 {
    let installed = SOURCE.with(|s| s.borrow().as_ref().map(|f| f()));
    match (installed, START.with(|s| *s.borrow())) {
        (Some(now), Some((start, base))) => {
            base + now.saturating_duration_since(start).as_millis() as u64
        }
        _ => process_start().elapsed().as_millis() as u64 + 1_000_000,
    }
}
