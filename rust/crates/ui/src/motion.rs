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

/// Progress `0..=1` of a finite animation of `duration` started at `start`. While frozen it
/// is the frozen time over the duration (settled when frozen without a time), matching the
/// reference capture's `currentTime`.
pub fn progress(start: Instant, duration: Duration, cx: &App) -> f32 {
    if duration.is_zero() || reduced(cx) {
        return 1.;
    }
    let elapsed = match cx.try_global::<Motion>().and_then(|m| m.frozen) {
        Some(Some(at)) => at,
        Some(None) => return 1.,
        None => start.elapsed(),
    };
    (elapsed.as_secs_f32() / duration.as_secs_f32()).min(1.)
}

/// `prefers-reduced-motion: reduce`, read from the platform by gpui-base at init.
pub fn reduced(cx: &App) -> bool {
    cx.reduce_motion()
}

/// A CSS timing function.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    Linear,
    /// `cubic-bezier(x1, y1, x2, y2)`.
    Bezier(f32, f32, f32, f32),
}

impl Easing {
    pub const EASE: Easing = Easing::Bezier(0.25, 0.1, 0.25, 1.);
    pub const EASE_IN: Easing = Easing::Bezier(0.42, 0., 1., 1.);
    pub const EASE_OUT: Easing = Easing::Bezier(0., 0., 0.58, 1.);
    pub const EASE_IN_OUT: Easing = Easing::Bezier(0.42, 0., 0.58, 1.);
    /// `cubic-bezier(.2, .8, .2, 1)`, the app's settle curve (toasts, tab FLIP, permission card).
    pub const SETTLE: Easing = Easing::Bezier(0.2, 0.8, 0.2, 1.);

    /// Output progress for input progress `t` in `0..=1`.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0., 1.);
        match self {
            Easing::Linear => t,
            Easing::Bezier(x1, y1, x2, y2) => {
                let bez = |a: f32, b: f32, s: f32| {
                    let u = 1. - s;
                    3. * u * u * s * a + 3. * u * s * s * b + s * s * s
                };
                // Solve x(s) = t by Newton steps, falling back to bisection.
                let mut s = t;
                for _ in 0..8 {
                    let x = bez(x1, x2, s) - t;
                    let u = 1. - s;
                    let dx = 3. * u * u * x1 + 6. * u * s * (x2 - x1) + 3. * s * s * (1. - x2);
                    if x.abs() < 1e-6 || dx.abs() < 1e-6 {
                        break;
                    }
                    s = (s - x / dx).clamp(0., 1.);
                }
                let (mut lo, mut hi) = (0f32, 1f32);
                if (bez(x1, x2, s) - t).abs() > 1e-5 {
                    for _ in 0..40 {
                        s = (lo + hi) / 2.;
                        if bez(x1, x2, s) < t {
                            lo = s;
                        } else {
                            hi = s;
                        }
                    }
                }
                bez(y1, y2, s)
            }
        }
    }
}

/// Eased progress of a finite animation (`1` once done, frozen or under reduced motion).
pub fn eased(start: Instant, duration: Duration, easing: Easing, cx: &App) -> f32 {
    easing.apply(progress(start, duration, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_curves() {
        assert_eq!(Easing::Linear.apply(0.3), 0.3);
        for e in [Easing::EASE, Easing::EASE_OUT, Easing::SETTLE] {
            assert!(e.apply(0.).abs() < 1e-4 && (e.apply(1.) - 1.).abs() < 1e-4);
        }
        // ease-out is ahead of linear; ease-in behind.
        assert!(Easing::EASE_OUT.apply(0.5) > 0.5);
        assert!(Easing::EASE_IN.apply(0.5) < 0.5);
        // ease at 50% ≈ 0.8024 (the reference value of cubic-bezier(.25,.1,.25,1)).
        assert!((Easing::EASE.apply(0.5) - 0.8024).abs() < 1e-3);
    }
}
