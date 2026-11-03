//! Animation clock shared by recur-ui widgets, plus the reduced-motion preference.
//!
//! Widgets read elapsed time through [`elapsed`] instead of `Instant::now()` directly, so spec
//! harnesses can freeze every animation for deterministic frames. Starts and elapsed time come
//! from the executor's clock
//! ([`now`]): `Instant::now()` in the app, and in tests the clock specs advance with their
//! timers, so an animation is where the spec's time says it is however loaded the machine.

use gpui_kit::{App, ElementId, Entity, Global, Window};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Motion {
    frozen: bool,
}

impl Global for Motion {}

/// Freeze animations: infinite animations pin at 0 and finite ones render settled. Pass
/// through [`thaw`] to resume.
pub fn freeze(cx: &mut App) {
    cx.default_global::<Motion>().frozen = true;
}

pub fn thaw(cx: &mut App) {
    cx.default_global::<Motion>().frozen = false;
}

/// Whether animations are frozen.
pub fn is_frozen(cx: &App) -> bool {
    cx.try_global::<Motion>().is_some_and(|m| m.frozen)
}

/// The animation clock: start animations at this instant.
pub fn now(cx: &App) -> Instant {
    cx.background_executor().now()
}

fn since(start: Instant, cx: &App) -> Duration {
    now(cx).saturating_duration_since(start)
}

/// Time since `start` for an infinite animation, or zero while frozen.
pub fn elapsed(start: Instant, cx: &App) -> Duration {
    if is_frozen(cx) {
        Duration::ZERO
    } else {
        since(start, cx)
    }
}

/// Progress `0..=1` of a finite animation of `duration` started at `start`. Settled while
/// frozen.
pub fn progress(start: Instant, duration: Duration, cx: &App) -> f32 {
    if duration.is_zero() || reduced(cx) || is_frozen(cx) {
        return 1.;
    }
    (since(start, cx).as_secs_f32() / duration.as_secs_f32()).min(1.)
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

/// A CSS `transition` between an element's rest and hover styles (`transition: background
/// .12s ease`): [`Hover::k`] eases from 0 to 1 after the pointer enters and back after it
/// leaves, reversing from where it is when interrupted. Mix the rest and hover values by it
/// (`Color::mix`) instead of a `.hover(..)` style, which snaps, and attach
/// [`Hover::listener`] with `on_hover`.
pub struct Hover {
    k: f32,
    state: Entity<HoverState>,
}

#[derive(Default)]
struct HoverState {
    on: bool,
    /// Progress when the pointer last crossed, and when.
    from: f32,
    at: Option<Instant>,
}

impl HoverState {
    fn k(&self, duration: Duration, easing: Easing, cx: &App) -> (f32, bool) {
        let to = if self.on { 1. } else { 0. };
        let Some(at) = self.at else {
            return (to, false);
        };
        let p = progress(at, duration, cx);
        (self.from + (to - self.from) * easing.apply(p), p < 1.)
    }
}

/// The hover transition of the element keyed `id` in the current view (see [`Hover`]).
pub fn hover(
    id: impl Into<ElementId>,
    duration: Duration,
    easing: Easing,
    window: &mut Window,
    cx: &mut App,
) -> Hover {
    let state = window.use_keyed_state(id, cx, |_, _| HoverState::default());
    let (k, running) = state.read(cx).k(duration, easing, cx);
    if running && !is_frozen(cx) {
        window.request_animation_frame();
    }
    Hover { k, state }
}

impl Hover {
    /// The transition's progress, `0` at rest and `1` hovered.
    pub fn k(&self) -> f32 {
        self.k
    }

    /// Whether the pointer is over the element.
    pub fn on(&self, cx: &App) -> bool {
        self.state.read(cx).on
    }

    /// For the element's `on_hover`.
    pub fn listener(&self) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let (state, k) = (self.state.clone(), self.k);
        move |on, _, cx| {
            state.update(cx, |s, cx| {
                if s.on != *on {
                    *s = HoverState {
                        on: *on,
                        from: k,
                        at: Some(now(cx)),
                    };
                    cx.notify();
                }
            })
        }
    }
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

    #[gpui_kit::test]
    fn hover_eases_in_and_reverses_from_where_it_is(cx: &mut gpui_kit::TestAppContext) {
        let ms = Duration::from_millis;
        cx.update(|cx| {
            let k = |s: &HoverState| s.k(ms(120), Easing::Linear, cx).0;
            let rest = HoverState::default();
            assert_eq!(k(&rest), 0.);
            let entering = HoverState {
                on: true,
                from: 0.,
                at: Some(now(cx) - ms(60)),
            };
            assert!((k(&entering) - 0.5).abs() < 0.2);
            // Leaving halfway in: back down from 0.5, done in 120ms.
            let leaving = HoverState {
                on: false,
                from: 0.5,
                at: Some(now(cx) - ms(60)),
            };
            assert!((k(&leaving) - 0.25).abs() < 0.2);
            let settled = HoverState {
                on: true,
                from: 0.,
                at: Some(now(cx) - ms(500)),
            };
            assert_eq!(k(&settled), 1.);
        });
    }
}
