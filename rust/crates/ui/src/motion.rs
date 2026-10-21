//! Animation clock shared by recur-ui widgets, plus the reduced-motion preference.
//!
//! Widgets read elapsed time through [`elapsed`] instead of `Instant::now()` directly, so the
//! parity renderer can freeze every animation at a chosen time, the way the reference capture
//! pauses `document.getAnimations()`.

use gpui_kit::{App, Global};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Motion {
    frozen: Option<Option<Duration>>,
}

impl Global for Motion {}

/// Freeze animations: `Some(t)` pins infinite animations at `t`, `None` pins them at 0. Finite
/// animations always render settled while frozen. Pass through [`thaw`] to resume.
pub fn freeze(at: Option<Duration>, cx: &mut App) {
    cx.default_global::<Motion>().frozen = Some(at);
}

pub fn thaw(cx: &mut App) {
    cx.default_global::<Motion>().frozen = None;
}

/// Whether animations are frozen for capture.
pub fn is_frozen(cx: &App) -> bool {
    cx.try_global::<Motion>()
        .is_some_and(|m| m.frozen.is_some())
}

/// Time since `start` for an infinite animation, or the frozen time.
pub fn elapsed(start: Instant, cx: &App) -> Duration {
    match cx.try_global::<Motion>().and_then(|m| m.frozen) {
        Some(at) => at.unwrap_or_default(),
        None => start.elapsed(),
    }
}

/// Progress `0..=1` of a finite animation of `duration` started at `start`; 1 while frozen.
pub fn progress(start: Instant, duration: Duration, cx: &App) -> f32 {
    if is_frozen(cx) || reduced(cx) || duration.is_zero() {
        return 1.;
    }
    (start.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.)
}

/// `prefers-reduced-motion: reduce`, read from the platform by gpui-base at init.
pub fn reduced(cx: &App) -> bool {
    cx.reduce_motion()
}
