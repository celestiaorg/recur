//! xterm.js 6's overlay scrollbar (VS Code's `ScrollableElement`): slider geometry, wheel
//! scrolling in pixels, track clicks and slider drags, and the reveal / fade-out behaviour.

use std::time::{Duration, Instant};

const MINIMUM_SLIDER_SIZE: f32 = 20.0;
/// Pixels per normalized wheel unit (`SCROLL_WHEEL_SENSITIVITY`).
const SCROLL_WHEEL_SENSITIVITY: f32 = 50.0;
/// WebKit's `Scrollbar::pixelsPerLineStep`: line deltas become this many pixels.
pub const PIXELS_PER_LINE: f32 = 40.0;
const HIDE_TIMEOUT: Duration = Duration::from_millis(500);
const FADE_IN: Duration = Duration::from_millis(100);
const FADE_OUT: Duration = Duration::from_millis(800);

/// Slider placement in the track, CSS pixels from the top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    pub position: f32,
    pub size: f32,
}

#[derive(Debug, Clone)]
pub struct Scrollbar {
    /// Viewport height (`css.canvas.height`).
    visible_size: f32,
    /// Content height (`cell.height * lines`).
    scroll_size: f32,
    /// `scrollTop`, CSS pixels.
    scroll_top: f32,
    mouse_over: bool,
    drag: Option<(f32, f32)>,
    revealed: bool,
    hide_at: Option<Instant>,
    /// Opacity transition: from, to, start.
    fade: (f32, f32, Option<Instant>),
}

impl Default for Scrollbar {
    fn default() -> Self {
        Self {
            visible_size: 0.0,
            scroll_size: 0.0,
            scroll_top: 0.0,
            mouse_over: false,
            drag: None,
            revealed: false,
            hide_at: None,
            fade: (0.0, 0.0, None),
        }
    }
}

impl Scrollbar {
    /// `Viewport._sync`: dimensions from the grid and the scroll position from `ydisp`.
    pub fn sync(&mut self, visible_size: f32, scroll_size: f32, ydisp: usize, cell_height: f32) {
        self.set_dimensions(visible_size, scroll_size);
        self.scroll_top = (ydisp as f32 * cell_height).clamp(0.0, self.max_scroll());
    }

    /// Dimensions only; keeps a fractional `scrollTop` from precise wheel deltas.
    pub fn set_dimensions(&mut self, visible_size: f32, scroll_size: f32) {
        self.visible_size = visible_size;
        self.scroll_size = scroll_size;
        self.scroll_top = self.scroll_top.clamp(0.0, self.max_scroll());
    }

    fn max_scroll(&self) -> f32 {
        (self.scroll_size - self.visible_size).max(0.0)
    }

    pub fn is_needed(&self) -> bool {
        self.scroll_size > 0.0 && self.scroll_size > self.visible_size
    }

    fn ratio(&self, slider: f32) -> f32 {
        (self.visible_size - slider) / (self.scroll_size - self.visible_size)
    }

    fn slider_size(&self) -> f32 {
        (self.visible_size * self.visible_size / self.scroll_size)
            .floor()
            .max(MINIMUM_SLIDER_SIZE)
            .round()
    }

    pub fn slider(&self) -> Option<Slider> {
        if !self.is_needed() {
            return None;
        }
        let size = self.slider_size();
        Some(Slider {
            position: (self.scroll_top * self.ratio(size)).round(),
            size,
        })
    }

    /// A wheel event, DOM-normalized (positive `delta` scrolls towards newer output). Returns the
    /// new `scrollTop`, or None when it did not move. `pixels` marks precise (trackpad) deltas.
    pub fn wheel(&mut self, delta: f32, pixels: bool, alt: bool, now: Instant) -> Option<f32> {
        // StandardWheelEvent: WebKit reports wheelDeltaY = 120 ticks per 40px line.
        let mut units = if pixels {
            -delta / PIXELS_PER_LINE
        } else {
            -delta
        };
        if alt {
            units *= recur_term::input::FAST_SCROLL_SENSITIVITY as f32;
        }
        if units == 0.0 {
            return None;
        }
        let step = SCROLL_WHEEL_SENSITIVITY * units;
        let step = if step < 0.0 {
            step.floor()
        } else {
            step.ceil()
        };
        self.set_scroll_top(self.scroll_top - step, now)
    }

    fn set_scroll_top(&mut self, top: f32, now: Instant) -> Option<f32> {
        let top = top.clamp(0.0, self.max_scroll());
        if top == self.scroll_top {
            return None;
        }
        self.scroll_top = top;
        self.reveal(now);
        Some(top)
    }

    /// Pointer down in the track at `offset` from its top. Jumps unless on the slider, then
    /// starts a slider drag either way. Returns the new `scrollTop` if it moved.
    pub fn pointer_down(&mut self, offset: f32, now: Instant) -> Option<f32> {
        let slider = self.slider()?;
        let moved = if offset >= slider.position && offset <= slider.position + slider.size {
            None
        } else {
            let desired = ((offset - slider.size / 2.0) / self.ratio(slider.size)).round();
            self.set_scroll_top(desired, now)
        };
        let slider = self.slider()?;
        self.drag = Some((offset, slider.position));
        self.reveal(now);
        moved
    }

    pub fn pointer_move(&mut self, offset: f32, now: Instant) -> Option<f32> {
        let (start, slider_start) = self.drag?;
        let size = self.slider_size();
        let desired = ((slider_start + offset - start) / self.ratio(size)).round();
        self.set_scroll_top(desired, now)
    }

    pub fn pointer_up(&mut self, now: Instant) {
        if self.drag.take().is_some() {
            self.schedule_hide(now);
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Mouse over / out of the scrollable area (the whole grid).
    pub fn set_mouse_over(&mut self, over: bool, now: Instant) {
        if over == self.mouse_over {
            return;
        }
        self.mouse_over = over;
        if over {
            self.reveal(now);
        } else {
            self.hide(now);
        }
    }

    fn reveal(&mut self, now: Instant) {
        if !self.revealed {
            self.revealed = true;
            self.fade = (self.opacity(now), 1.0, Some(now));
        }
        self.schedule_hide(now);
    }

    fn schedule_hide(&mut self, now: Instant) {
        self.hide_at = (!self.mouse_over && self.drag.is_none()).then_some(now + HIDE_TIMEOUT);
    }

    fn hide(&mut self, now: Instant) {
        if self.mouse_over || self.drag.is_some() {
            return;
        }
        self.hide_at = None;
        if self.revealed {
            self.revealed = false;
            self.fade = (self.opacity(now), 0.0, Some(now));
        }
    }

    /// Runs the hide timer. Returns whether anything changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.hide_at.is_some_and(|t| t <= now) {
            self.hide(now);
            return true;
        }
        false
    }

    /// Opacity now (100ms fade in, 800ms fade out, linear).
    pub fn opacity(&self, now: Instant) -> f32 {
        self.opacity_with(|start, dur| {
            (now.saturating_duration_since(start).as_secs_f32() / dur.as_secs_f32()).min(1.0)
        })
    }

    /// Opacity with the fade's progress (`0..=1`) from `progress(start, duration)`.
    pub fn opacity_with(&self, progress: impl Fn(Instant, Duration) -> f32) -> f32 {
        let (from, to, start) = self.fade;
        let Some(start) = start else { return to };
        let dur = if to > from { FADE_IN } else { FADE_OUT };
        from + (to - from) * progress(start, dur)
    }

    /// Next time the scrollbar needs repainting (an active fade or the hide timer).
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let (from, to, start) = self.fade;
        let fading = start.filter(|s| {
            let dur = if to > from { FADE_IN } else { FADE_OUT };
            now < *s + dur
        });
        match (
            fading.map(|_| now + Duration::from_millis(16)),
            self.hide_at,
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(lines: usize) -> Scrollbar {
        let mut s = Scrollbar::default();
        s.sync(240.0, 16.0 * lines as f32, lines - 15, 16.0);
        s
    }

    #[test]
    fn slider_geometry() {
        let s = bar(150);
        // visible 240, content 2400: floor(240*240/2400) = 24, at the bottom.
        assert_eq!(
            s.slider(),
            Some(Slider {
                position: 216.0,
                size: 24.0
            })
        );
        let mut short = Scrollbar::default();
        short.sync(240.0, 160.0, 0, 16.0);
        assert!(short.slider().is_none(), "not needed without scrollback");
        let tall = bar(10_015);
        assert_eq!(tall.slider().map(|s| s.size), Some(MINIMUM_SLIDER_SIZE));
    }

    #[test]
    fn wheel_scrolls_in_fifty_pixel_units() {
        let now = Instant::now();
        let mut s = bar(150);
        // One mouse-wheel line up: 50px.
        assert_eq!(s.wheel(-1.0, false, false, now), Some(2160.0 - 50.0));
        // A 10px trackpad delta: 50 * 10/40 = 12.5 → ceil 13.
        assert_eq!(s.wheel(10.0, true, false, now), Some(2110.0 + 13.0));
        assert_eq!(
            s.wheel(1000.0, true, false, now),
            Some(2160.0),
            "clamped to the bottom"
        );
        assert_eq!(s.wheel(1.0, true, false, now), None);
        assert_eq!(
            s.wheel(-1.0, false, true, now),
            Some(2160.0 - 250.0),
            "alt scrolls fast"
        );
    }

    #[test]
    fn track_click_jumps_and_drags() {
        let now = Instant::now();
        let mut s = bar(150);
        let top = s.pointer_down(12.0, now).unwrap();
        assert_eq!(s.slider().unwrap().position, 0.0);
        assert_eq!(top, 0.0);
        assert!(s.is_dragging());
        let moved = s.pointer_move(120.0, now).unwrap();
        assert_eq!(s.slider().unwrap().position, 108.0);
        assert!(moved > 0.0);
        s.pointer_up(now);
        assert!(!s.is_dragging());
    }

    #[test]
    fn reveals_on_scroll_and_fades_after_500ms() {
        let t0 = Instant::now();
        let mut s = bar(150);
        assert_eq!(s.opacity(t0), 0.0);
        s.wheel(-1.0, false, false, t0);
        assert_eq!(s.opacity(t0 + Duration::from_millis(100)), 1.0);
        assert!(!s.tick(t0 + Duration::from_millis(499)));
        assert!(s.tick(t0 + Duration::from_millis(500)));
        let half = s.opacity(t0 + Duration::from_millis(900));
        assert!((half - 0.5).abs() < 0.01, "{half}");
        assert_eq!(s.opacity(t0 + Duration::from_millis(1300)), 0.0);
    }

    #[test]
    fn stays_visible_while_hovered() {
        let t0 = Instant::now();
        let mut s = bar(150);
        s.set_mouse_over(true, t0);
        assert!(!s.tick(t0 + Duration::from_secs(5)));
        assert_eq!(s.opacity(t0 + Duration::from_secs(5)), 1.0);
        s.set_mouse_over(false, t0 + Duration::from_secs(5));
        assert_eq!(s.opacity(t0 + Duration::from_secs(6)), 0.0);
    }
}
