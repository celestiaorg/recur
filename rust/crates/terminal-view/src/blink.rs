//! xterm.js's WebGL `CursorBlinkStateManager`, driven by explicit timestamps.
//!
//! Blinks at 600ms while focused, restarts the phase when the cursor moves or the mouse is
//! pressed, pauses (cursor shown) on blur, and stops blinking after 5 minutes idle.

use std::time::{Duration, Instant};

pub const BLINK_INTERVAL: Duration = Duration::from_millis(600);
pub const CURSOR_BLINK_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Paused,
    /// `_blinkStartTimeout`: the first hide.
    Starting(Instant),
    /// `_blinkInterval`: toggles every 600ms.
    Blinking(Instant),
}

#[derive(Debug, Clone)]
pub struct CursorBlink {
    visible: bool,
    phase: Phase,
    idle_at: Option<Instant>,
    idle_paused: bool,
    restarted_at: Option<Instant>,
}

impl CursorBlink {
    pub fn new(now: Instant, focused: bool) -> Self {
        let mut b = Self { visible: true, phase: Phase::Paused, idle_at: None, idle_paused: false, restarted_at: None };
        if focused {
            b.restart_interval(now, BLINK_INTERVAL);
            b.reset_idle(now);
        }
        b
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn is_paused(&self) -> bool {
        self.phase == Phase::Paused
    }

    fn restart_interval(&mut self, now: Instant, delay: Duration) {
        self.phase = Phase::Starting(now + delay);
    }

    fn reset_idle(&mut self, now: Instant) {
        self.idle_paused = false;
        self.idle_at = Some(now + CURSOR_BLINK_IDLE_TIMEOUT);
    }

    /// `restartBlinkAnimation`: show the cursor and hold it for a full interval.
    pub fn restart(&mut self, now: Instant) {
        if self.idle_paused {
            self.reset_idle(now);
        }
        if self.is_paused() {
            return;
        }
        self.restarted_at = Some(now);
        self.visible = true;
    }

    /// Blur: cursor shown, no timers.
    pub fn pause(&mut self) {
        self.visible = true;
        self.idle_paused = false;
        self.phase = Phase::Paused;
        self.idle_at = None;
    }

    /// Focus.
    pub fn resume(&mut self, now: Instant) {
        self.pause();
        self.restarted_at = None;
        self.restart_interval(now, BLINK_INTERVAL);
        self.reset_idle(now);
        self.restart(now);
    }

    /// When the next timer fires, if any.
    pub fn next_deadline(&self) -> Option<Instant> {
        let timer = match self.phase {
            Phase::Paused => None,
            Phase::Starting(t) | Phase::Blinking(t) => Some(t),
        };
        match (timer, self.idle_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Runs every timer due by `now`. Returns whether visibility changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let before = self.visible;
        while let Some(due) = self.next_deadline().filter(|d| *d <= now) {
            if self.idle_at == Some(due) {
                self.visible = true;
                self.idle_paused = true;
                self.phase = Phase::Paused;
                self.idle_at = None;
                continue;
            }
            match self.phase {
                Phase::Starting(_) => {
                    if let Some(r) = self.restarted_at.take() {
                        let left = BLINK_INTERVAL.saturating_sub(due.saturating_duration_since(r));
                        if !left.is_zero() {
                            self.restart_interval(due, left);
                            continue;
                        }
                    }
                    self.visible = false;
                    self.phase = Phase::Blinking(due + BLINK_INTERVAL);
                }
                Phase::Blinking(_) => {
                    if let Some(r) = self.restarted_at.take() {
                        let left = BLINK_INTERVAL.saturating_sub(due.saturating_duration_since(r));
                        self.restart_interval(due, left);
                        continue;
                    }
                    self.visible = !self.visible;
                    self.phase = Phase::Blinking(due + BLINK_INTERVAL);
                }
                Phase::Paused => break,
            }
        }
        before != self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn blinks_every_600ms_while_focused() {
        let t0 = Instant::now();
        let mut b = CursorBlink::new(t0, true);
        assert!(b.is_visible());
        assert!(!b.tick(t0 + ms(599)));
        assert!(b.tick(t0 + ms(600)));
        assert!(!b.is_visible());
        b.tick(t0 + ms(1200));
        assert!(b.is_visible());
        b.tick(t0 + ms(1800));
        assert!(!b.is_visible());
    }

    #[test]
    fn unfocused_cursor_stays_shown() {
        let t0 = Instant::now();
        let mut b = CursorBlink::new(t0, false);
        assert!(b.is_paused() && b.next_deadline().is_none());
        b.tick(t0 + ms(5000));
        assert!(b.is_visible());
    }

    #[test]
    fn restart_holds_the_cursor_for_a_full_interval() {
        let t0 = Instant::now();
        let mut b = CursorBlink::new(t0, true);
        b.tick(t0 + ms(600));
        assert!(!b.is_visible());
        b.restart(t0 + ms(900));
        assert!(b.is_visible());
        // The interval fires at 1200 and defers to 900 + 600.
        b.tick(t0 + ms(1200));
        assert!(b.is_visible());
        b.tick(t0 + ms(1499));
        assert!(b.is_visible());
        b.tick(t0 + ms(1500));
        assert!(!b.is_visible());
    }

    #[test]
    fn blur_pauses_and_focus_resumes() {
        let t0 = Instant::now();
        let mut b = CursorBlink::new(t0, true);
        b.tick(t0 + ms(600));
        b.pause();
        assert!(b.is_visible() && b.is_paused());
        b.resume(t0 + ms(700));
        b.tick(t0 + ms(1299));
        assert!(b.is_visible());
        b.tick(t0 + ms(1300));
        assert!(!b.is_visible());
    }

    #[test]
    fn stops_blinking_when_idle() {
        let t0 = Instant::now();
        let mut b = CursorBlink::new(t0, true);
        b.tick(t0 + CURSOR_BLINK_IDLE_TIMEOUT + ms(1));
        assert!(b.is_visible() && b.is_paused());
        // As in xterm.js, a restart after the idle stop resets the idle timer but does not
        // resume blinking until the next focus.
        b.restart(t0 + CURSOR_BLINK_IDLE_TIMEOUT + ms(10));
        assert!(b.is_paused());
    }
}
