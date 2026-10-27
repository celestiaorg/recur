//! Recur: panic containment (`rust/vendor/README.md`, "Contained panics").
//!
//! A panic must not unwind into a platform callback (AppKit, GCD and GLib frames cannot be
//! unwound through, so the process aborts). Element listeners and `canvas` closures run under
//! [`contain`]; task futures under [`Recorded`], whose panic the task system keeps and
//! re-raises in an awaiter. Entity and window updates restore their lease and window before an
//! unwind leaves them (`App::update_entity_erased`, `App::update_window_erased`).

use std::cell::RefCell;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

thread_local! {
    static CONTAINED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The panics contained on this thread since the last call, as `site: message`. GPUI tests fail
/// when a test leaves any; a test that panics on purpose takes them.
pub fn take_contained_panics() -> Vec<String> {
    CONTAINED.with_borrow_mut(std::mem::take)
}

fn record(site: &str, payload: &(dyn std::any::Any + Send)) {
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic".into());
    log::error!("gpui: contained a panic in {site}: {message}");
    CONTAINED.with_borrow_mut(|c| c.push(format!("{site}: {message}")));
}

/// Runs `f`; a panic is recorded and dropped, so the caller carries on as if `f` returned
/// nothing.
pub(crate) fn contain<R>(site: &str, f: impl FnOnce() -> R) -> Option<R> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => Some(r),
        Err(payload) => {
            record(site, &*payload);
            None
        }
    }
}

/// A task's future: a panic in a poll is recorded and resumed for the task to keep.
pub(crate) struct Recorded<F>(pub(crate) F);

impl<F: Future> Future for Recorded<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        // SAFETY: the inner future is structurally pinned and never moved out.
        let inner = unsafe { self.map_unchecked_mut(|r| &mut r.0) };
        match catch_unwind(AssertUnwindSafe(|| inner.poll(cx))) {
            Ok(p) => p,
            Err(payload) => {
                record("a task", &*payload);
                resume_unwind(payload)
            }
        }
    }
}
