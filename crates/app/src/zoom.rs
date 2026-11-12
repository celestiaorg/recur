//! Whole-UI zoom (`lib/zoom.ts`): ⌘+ / ⌘- / ⌘0, ⌘ + wheel and the View menu step the
//! window's zoom (`Window::set_ui_zoom`, Recur's gpui-pre patch) through fixed levels, toast
//! the level and save it in `ui-view.json`.

use std::time::{Duration, Instant};

use gpui_kit::{AppContext, Context, Keystroke, ScrollDelta, ScrollEvent, TouchPhase, Window};
use recur_ui::widgets::{ToastKind, ToastOptions, push_toast};

use crate::shell::Shell;

pub const ZOOM_STEPS: [f32; 10] = [0.5, 0.67, 0.8, 0.9, 1., 1.1, 1.25, 1.5, 1.75, 2.];
const EPS: f32 = 1e-6;
/// One trackpad gesture is a stream of wheel events: step once per this window.
const WHEEL_THROTTLE: Duration = Duration::from_millis(120);
/// Pixels a ⌘ gesture travels before a step, so a finger brushing a Magic Mouse while ⌘T or
/// ⌘C is held does not zoom.
const WHEEL_STEP_PX: f32 = 24.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

/// The level `step` moves `cur` to: the next step up or down (from an off-step value too),
/// clamped at both ends.
pub fn next_zoom(cur: f32, step: Zoom) -> f32 {
    match step {
        Zoom::In => ZOOM_STEPS
            .into_iter()
            .find(|s| *s > cur + EPS)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1]),
        Zoom::Out => ZOOM_STEPS
            .into_iter()
            .rev()
            .find(|s| *s < cur - EPS)
            .unwrap_or(ZOOM_STEPS[0]),
        Zoom::Reset => 1.,
    }
}

/// The zoom a keystroke asks for: ⌘ on macOS, Ctrl elsewhere, on `=` / `+` / `-` / `0` (GPUI
/// reports the numpad keys and ⇧= as the same characters).
pub fn zoom_chord(k: &Keystroke) -> Option<Zoom> {
    let secondary = if cfg!(target_os = "macos") {
        k.modifiers.platform
    } else {
        k.modifiers.control
    };
    if !secondary || k.modifiers.alt {
        return None;
    }
    match k.key.as_str() {
        "=" | "+" => Some(Zoom::In),
        "-" => Some(Zoom::Out),
        "0" => Some(Zoom::Reset),
        _ => None,
    }
}

/// Whether a wheel event zooms. A trackpad gesture zooms only if ⌘ was down when it began,
/// and its momentum never does, so ⌘T pressed while a flick coasts does not zoom. `gesture`
/// holds the current gesture's answer; Linux and line-based wheels report no phases and
/// fall back to the live modifiers.
fn wheel_zooms(gesture: &mut Option<bool>, ev: &ScrollEvent) -> bool {
    let cmd = if cfg!(target_os = "macos") {
        ev.modifiers.platform
    } else {
        ev.modifiers.control
    };
    match (ev.delta, ev.touch_phase) {
        (ScrollDelta::Lines(_), _) => cmd,
        (_, TouchPhase::Started) => *gesture.insert(cmd),
        (_, TouchPhase::Ended) => gesture.replace(false).unwrap_or(cmd),
        _ => gesture.unwrap_or(cmd),
    }
}

/// Zooms the window. The native traffic lights keep their size, so they re-center in the
/// zoomed titlebar.
fn set_zoom(zoom: f32, window: &mut Window, cx: &mut Context<Shell>) {
    window.set_ui_zoom(zoom, cx);
    #[cfg(target_os = "macos")]
    {
        let (x, y) = crate::TRAFFIC_LIGHT_POSITION;
        let y = y + crate::TITLEBAR_HEIGHT * (zoom - 1.) / 2.;
        window.set_traffic_light_position(gpui_kit::point(gpui_kit::px(x), gpui_kit::px(y)));
    }
}

impl Shell {
    pub(crate) fn zoom(&mut self, step: Zoom, window: &mut Window, cx: &mut Context<Self>) {
        let next = next_zoom(window.ui_zoom(), step);
        if (next - window.ui_zoom()).abs() < EPS {
            return;
        }
        set_zoom(next, window, cx);
        push_toast(
            ToastKind::Info,
            format!("Zoom {}%", (next * 100.).round()),
            ToastOptions {
                replace: Some("ui-zoom".into()),
                ..Default::default()
            },
            cx,
        );
        let services = self.state.read(cx).services().clone();
        cx.background_spawn(async move {
            let _ = services.shell.set_ui_zoom(next).await;
        })
        .detach();
    }

    /// ⌘ + wheel, seen in the capture phase so no scroller under the pointer takes it.
    pub(crate) fn zoom_wheel(
        &mut self,
        ev: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y,
        };
        let off = (self.state.read(cx).settings.as_ref()).is_some_and(|s| s.disable_scroll_zoom);
        if off || !wheel_zooms(&mut self.wheel_gesture, ev) || dy == 0. {
            return;
        }
        cx.stop_propagation();
        if let ScrollDelta::Pixels(_) = ev.delta {
            if ev.touch_phase == TouchPhase::Started {
                self.wheel_travel = 0.;
            }
            self.wheel_travel += dy;
            if self.wheel_travel.abs() < WHEEL_STEP_PX {
                return;
            }
            self.wheel_travel = 0.;
        }
        let now = Instant::now();
        if self
            .last_wheel_zoom
            .is_some_and(|t| now.duration_since(t) < WHEEL_THROTTLE)
        {
            return;
        }
        self.last_wheel_zoom = Some(now);
        // GPUI's wheel delta is positive scrolling up, the DOM's negative.
        let step = if dy > 0. { Zoom::In } else { Zoom::Out };
        self.zoom(step, window, cx);
    }

    /// `initZoom`: the saved level, without a toast or a save.
    pub(crate) fn restore_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let services = self.state.read(cx).services().clone();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(zoom) = services.shell.get_ui_zoom().await {
                let _ = this.update_in(cx, |_, window, cx| set_zoom(zoom, window, cx));
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_one_level_at_a_time_and_clamps() {
        assert_eq!(next_zoom(1., Zoom::In), 1.1);
        assert_eq!(next_zoom(1.1, Zoom::In), 1.25);
        assert_eq!(next_zoom(1.25, Zoom::Out), 1.1);
        assert_eq!(next_zoom(2., Zoom::In), 2.);
        assert_eq!(next_zoom(0.5, Zoom::Out), 0.5);
        assert_eq!(next_zoom(1.75, Zoom::Reset), 1.);
        assert_eq!(next_zoom(1.05, Zoom::In), 1.1);
        assert_eq!(next_zoom(1.05, Zoom::Out), 1.);
        let mut z = 1.;
        for _ in 0..20 {
            z = next_zoom(z, Zoom::In);
        }
        assert_eq!(z, 2.);
    }

    #[test]
    fn momentum_and_mid_gesture_cmd_do_not_zoom() {
        let ev = |phase, cmd: bool| ScrollEvent {
            delta: ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(4.))),
            touch_phase: phase,
            modifiers: gpui_kit::Modifiers {
                platform: cfg!(target_os = "macos") && cmd,
                control: !cfg!(target_os = "macos") && cmd,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut g = None;
        assert!(wheel_zooms(&mut g, &ev(TouchPhase::Moved, true)));
        assert!(!wheel_zooms(&mut g, &ev(TouchPhase::Started, false)));
        assert!(!wheel_zooms(&mut g, &ev(TouchPhase::Moved, true)));
        assert!(!wheel_zooms(&mut g, &ev(TouchPhase::Ended, true)));
        assert!(wheel_zooms(&mut g, &ev(TouchPhase::Started, true)));
        assert!(wheel_zooms(&mut g, &ev(TouchPhase::Moved, true)));
        assert!(wheel_zooms(&mut g, &ev(TouchPhase::Ended, true)));
        assert!(!wheel_zooms(&mut g, &ev(TouchPhase::Moved, true)));
    }

    #[test]
    fn chords() {
        let k = |s: &str| Keystroke::parse_with_os(s, gpui_kit::OperatingSystem::Mac).unwrap();
        let cmd = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        assert_eq!(zoom_chord(&k(&format!("{cmd}-="))), Some(Zoom::In));
        assert_eq!(zoom_chord(&k(&format!("{cmd}-+"))), Some(Zoom::In));
        assert_eq!(zoom_chord(&k(&format!("{cmd}-shift-="))), Some(Zoom::In));
        assert_eq!(zoom_chord(&k(&format!("{cmd}--"))), Some(Zoom::Out));
        assert_eq!(zoom_chord(&k(&format!("{cmd}-0"))), Some(Zoom::Reset));
        assert_eq!(zoom_chord(&k(&format!("{cmd}-alt--"))), None);
        assert_eq!(zoom_chord(&k("-")), None);
        if cfg!(target_os = "macos") {
            // ⌃- stays the terminal's readline chord.
            assert_eq!(zoom_chord(&k("ctrl--")), None);
        }
    }
}
