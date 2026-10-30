//! Whole-UI zoom (`lib/zoom.ts`): ⌘+ / ⌘- / ⌘0, ⌘ + wheel and the View menu step the
//! window's zoom (`Window::set_ui_zoom`, Recur's gpui-pre patch) through fixed levels, toast
//! the level and save it in `ui-view.json`.

use std::time::{Duration, Instant};

use gpui_kit::{AppContext, Context, Keystroke, ScrollDelta, ScrollWheelEvent, Window};
use recur_ui::widgets::{ToastKind, ToastOptions, push_toast};

use crate::shell::Shell;

pub const ZOOM_STEPS: [f32; 10] = [0.5, 0.67, 0.8, 0.9, 1., 1.1, 1.25, 1.5, 1.75, 2.];
const EPS: f32 = 1e-6;
/// One trackpad gesture is a stream of wheel events: step once per this window.
const WHEEL_THROTTLE: Duration = Duration::from_millis(120);

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
    if !k.modifiers.secondary() || k.modifiers.alt {
        return None;
    }
    match k.key.as_str() {
        "=" | "+" => Some(Zoom::In),
        "-" => Some(Zoom::Out),
        "0" => Some(Zoom::Reset),
        _ => None,
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
        ev: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(l) => l.y,
        };
        if !ev.modifiers.secondary() || dy == 0. {
            return;
        }
        cx.stop_propagation();
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
    fn chords() {
        let k = |s: &str| Keystroke::parse(s).unwrap();
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
