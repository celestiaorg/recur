//! The right panel floating: the same panel as when docked, drawn as a flyout at the right edge
//! of the body, at its own remembered width. It slides out when it opens and back in when it
//! closes, and a press on the center column closes it.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseDownEvent, ParentElement, Styled,
    Window, deferred, div, px,
};
use recur_ui::guard::Guarded;
use recur_ui::motion::{self, Easing};
use recur_ui::rgba;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_view_core::prefs::PanelKey;

use crate::obs::Observatory;

/// The chat keeps at least this much of the width beside the center column.
const CHAT_MIN_W: f32 = 380.;
/// How much of what the flyout opens over stays visible beside it.
const FLYOUT_KEEP: f32 = 120.;
/// How far the flyout's resize handle reaches past its edge.
const HANDLE_REACH: f32 = 4.;
const FLYOUT_SLIDE: std::time::Duration = std::time::Duration::from_millis(180);

/// How much of the flyout is out (0 to 1) at this point of its slide.
fn slide_shown((out, from, at): (bool, f32, std::time::Instant), cx: &gpui_kit::App) -> f32 {
    let to = if out { 1. } else { 0. };
    from + (to - from) * motion::eased(at, FLYOUT_SLIDE, Easing::SETTLE, cx)
}

impl Observatory {
    /// The center column's width while the right panel floats: its saved size, leaving the
    /// chat [`CHAT_MIN_W`].
    pub(crate) fn center_width(&self, cx: &gpui_kit::App) -> f32 {
        let obs_w = f32::from(self.bounds.obs.get().size.width);
        let width = self.state.read(cx).panels.size(PanelKey::ObsCenter);
        if obs_w > 0. {
            width.min((obs_w - CHAT_MIN_W).max(PanelKey::ObsCenter.bounds().0))
        } else {
            width
        }
    }

    /// The right panel's width is remembered apart for docked and for floating.
    pub(crate) fn side_width_key(&self, cx: &gpui_kit::App) -> PanelKey {
        if self.floating(cx) {
            PanelKey::ObsSideFloat
        } else {
            PanelKey::ObsSide
        }
    }

    /// The right panel's width, capped while it floats so some of what it opens over stays
    /// visible.
    pub(crate) fn obs_side_width(&self, cx: &gpui_kit::App) -> f32 {
        let width = self.state.read(cx).panels.size(self.side_width_key(cx));
        let obs_w = f32::from(self.bounds.obs.get().size.width);
        if self.floating(cx) && obs_w > 0. {
            width.min((obs_w - FLYOUT_KEEP).max(FLYOUT_KEEP))
        } else {
            width
        }
    }

    /// How much of the floating panel is out, stepping its slide: out when it opens, back in
    /// when it closes (reversing from where it is). `None` once it is fully in.
    pub(super) fn step_side_slide(
        &mut self,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> Option<f32> {
        match (self.show_side, self.side_slide) {
            (true, Some((true, ..))) => {}
            (true, was) => {
                let from = was.map_or(0., |s| slide_shown(s, cx));
                self.side_slide = Some((true, from, motion::now(cx)));
            }
            (false, Some(s @ (true, ..))) => {
                self.side_slide = Some((false, slide_shown(s, cx), motion::now(cx)));
            }
            (false, _) => {}
        }
        let slide = self.side_slide?;
        let done = motion::progress(slide.2, FLYOUT_SLIDE, cx) >= 1.;
        if done && !slide.0 {
            self.side_slide = None;
            return None;
        }
        if !done {
            window.request_animation_frame();
        }
        Some(slide_shown(slide, cx))
    }

    /// How much of the floating panel is out right now (0 closed, 1 open), without stepping
    /// its slide.
    pub(crate) fn side_shown(&self, cx: &gpui_kit::App) -> f32 {
        match self.side_slide {
            Some(s) => slide_shown(s, cx),
            None if self.show_side => 1.,
            None => 0.,
        }
    }

    /// The flyout around the right panel, `shown` of it out. It stays while you work in it or
    /// in the chat; a press on the center column, clear of its resize handle, closes it and
    /// still reaches what was pressed.
    pub(super) fn render_side_flyout(
        &mut self,
        side: AnyElement,
        shown: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = self.obs_side_width(cx);
        let panel = div()
            .debug_selector(|| "obs-side-float".into())
            .flex()
            .size_full()
            .when(self.show_side, |d| {
                d.occlude().on_mouse_down_out(cx.guarded_listener(
                    move |o, e: &MouseDownEvent, _, cx| {
                        let center = o.bounds.work.get();
                        let edge = center.right() - px(width + HANDLE_REACH);
                        if center.contains(&e.position) && e.position.x < edge {
                            o.close_side_float(cx);
                        }
                    },
                ))
            })
            .child(side);
        let shadow = css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45));
        // Under the blocked overlay (10).
        deferred(
            with_shadow(panel, &[shadow], 0.)
                .absolute()
                .top_0()
                .bottom_0()
                .right(px((shown - 1.) * width))
                .w(px(width)),
        )
        .with_priority(9)
        .into_any_element()
    }
}
