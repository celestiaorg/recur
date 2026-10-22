//! Scrollbars from `_base.scss`. Every scroller gets WebKit's custom `::-webkit-scrollbar`
//! (11px, transparent track, `--scrollbar-thumb` thumb with radius 8, `--scrollbar-thumb-hover`
//! under the pointer) that takes layout space while the content overflows. Scrollers that keep
//! sticky rows use the 6px overlay thumb of `lib/overlay-scrollbar.ts` (`.overlay-sb`) instead,
//! which floats above the rows and fades in while scrolling, hovered or dragged.

use crate::motion::{self, Easing};
use crate::theme::{SCROLLBAR_SIZE, Theme};
use crate::widgets::Axis;
use gpui_kit::{
    AnyElement, App, DispatchPhase, ElementId, InteractiveElement, IntoElement, MouseButton,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, RenderOnce, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, canvas, div, point, px,
};
use std::time::{Duration, Instant};

/// `overlayThumb`: thumb height and offset for a vertical scroller, and whether it overflows.
pub fn overlay_thumb(
    scroll_top: f32,
    scroll_height: f32,
    client_height: f32,
    min_thumb: f32,
) -> (f32, f32, bool) {
    let max_scroll = scroll_height - client_height;
    let track = client_height.max(1.);
    let ratio = (track / scroll_height.max(1.)).min(1.);
    let h = track.min(min_thumb.max(track * ratio));
    let top = if max_scroll > 0. {
        scroll_top / max_scroll * (track - h)
    } else {
        0.
    };
    (h, top, max_scroll > 1.)
}

/// `scrollFromTrack`: the scroll offset that puts the overlay thumb's top at `thumb_top`.
pub fn scroll_from_track(
    thumb_top: f32,
    scroll_height: f32,
    client_height: f32,
    min_thumb: f32,
) -> f32 {
    let (h, _, _) = overlay_thumb(0., scroll_height, client_height, min_thumb);
    scroll_for_thumb(
        thumb_top,
        h,
        client_height.max(1.),
        scroll_height - client_height,
    )
}

/// The shortest classic thumb WebKit paints for the app's `::-webkit-scrollbar` (measured: a
/// 680px track over 43,200px of content draws a 17px thumb, not 11px).
pub const MIN_THUMB: f32 = 17.;

fn scroll_for_thumb(thumb_pos: f32, thumb_len: f32, track: f32, max_scroll: f32) -> f32 {
    let travel = track - thumb_len;
    if travel <= 0. || max_scroll <= 0. {
        return 0.;
    }
    thumb_pos.clamp(0., travel) / travel * max_scroll
}

/// WebKit's custom scrollbar thumb (`ScrollbarThemeComposite::thumbLength`/`thumbPosition`):
/// whole-pixel length and offset along a track of `track` px. A thumb longer than the track
/// disappears. The contents size is the snapped (rounded) scroll height, as `scrollHeight`;
/// a long document's thumb stops at [`MIN_THUMB`].
pub fn classic_thumb(pos: f32, total: f32, visible: f32, track: f32) -> (f32, f32) {
    let (total, visible, track) = (total.round(), visible.floor(), track.floor());
    if total <= 0. {
        return (0., 0.);
    }
    let mut len = (visible / total * track).round().max(MIN_THUMB.min(track));
    if len > track {
        len = 0.;
    }
    let size = total - visible;
    if size <= 0. {
        return (len, 0.);
    }
    let at = pos.max(0.) * (track - len) / size;
    (len, if at > 0. && at < 1. { 1. } else { at.trunc() })
}

/// `Scrollbar::pageStep`: how far a press on the track pages.
pub fn page_step(visible: f32) -> f32 {
    (visible * 0.875).max(visible - 40.).max(1.)
}

/// Before the overlay hides after the last scroll or pointer leave.
const HIDE: Duration = Duration::from_millis(700);
const FADE_IN: Duration = Duration::from_millis(120);
const FADE_OUT: Duration = Duration::from_millis(450);
const PAGE_FIRST: Duration = Duration::from_millis(250);
const PAGE_REPEAT: Duration = Duration::from_millis(50);

#[derive(Default)]
struct State {
    handle: ScrollHandle,
    /// Pointer offset within the thumb while it is dragged.
    grab: Option<f32>,
    /// Track press paging: pointer position along the track and the next repeat.
    paging: Option<(f32, Instant)>,
    hover_track: bool,
    hover_thumb: bool,
    last_pos: Option<f32>,
    hide_at: Option<Instant>,
    /// `.overlay-sb.on` and its opacity transition (start, starting opacity).
    on: bool,
    fade: Option<(Instant, f32)>,
}

impl State {
    fn opacity(&self, cx: &App) -> f32 {
        let target = if self.on { 1. } else { 0. };
        let Some((start, from)) = self.fade else {
            return target;
        };
        let duration = if self.on { FADE_IN } else { FADE_OUT };
        let p = Easing::EASE.apply(motion::progress(start, duration, cx));
        from + (target - from) * p
    }

    /// `hideSoon`: no timer while the pointer holds or hovers the bar.
    fn hide_soon(&mut self) {
        self.hide_at = (self.grab.is_none() && !self.hover_track).then(|| Instant::now() + HIDE);
    }
}

/// (position, content size, viewport size) along `axis`.
fn metrics(h: &ScrollHandle, axis: Axis) -> (f32, f32, f32) {
    let (off, max, client) = match axis {
        Axis::X => (h.offset().x, h.max_offset().x, h.bounds().size.width),
        Axis::Y => (h.offset().y, h.max_offset().y, h.bounds().size.height),
    };
    let client = f32::from(client);
    (-f32::from(off), client + f32::from(max), client)
}

fn set_pos(h: &ScrollHandle, axis: Axis, pos: f32) {
    let o = h.offset();
    h.set_offset(match axis {
        Axis::X => point(px(-pos), o.y),
        Axis::Y => point(o.x, px(-pos)),
    });
}

fn along(p: Point<Pixels>, h: &ScrollHandle, axis: Axis) -> f32 {
    let b = h.bounds();
    f32::from(match axis {
        Axis::X => p.x - b.left(),
        Axis::Y => p.y - b.top(),
    })
}

/// A scroller with the web UI's scrollbar. Vertical with the classic bar by default.
#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    axis: Axis,
    overlay: bool,
    handle: Option<ScrollHandle>,
    children: Vec<AnyElement>,
}

pub fn scroll_area(id: impl Into<ElementId>) -> ScrollArea {
    ScrollArea {
        id: id.into(),
        axis: Axis::Y,
        overlay: false,
        handle: None,
        children: Vec::new(),
    }
}

impl ScrollArea {
    /// Scroll sideways; the 11px bar sits along the bottom.
    pub fn horizontal(mut self) -> Self {
        self.axis = Axis::X;
        self
    }

    /// The `.overlay-sb` thumb (vertical only).
    pub fn overlay(mut self) -> Self {
        self.axis = Axis::Y;
        self.overlay = true;
        self
    }

    /// Drive and observe the scroller with the caller's handle.
    pub fn track(mut self, handle: &ScrollHandle) -> Self {
        self.handle = Some(handle.clone());
        self
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let (axis, overlay) = (self.axis, self.overlay);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| State::default());
        let handle = match self.handle {
            Some(h) => h,
            None => state.read(cx).handle.clone(),
        };
        let (pos, total, client) = metrics(&handle, axis);
        let overflow = total - client > if overlay { 1. } else { 0.5 };
        let size = if overlay {
            6.
        } else {
            f32::from(SCROLLBAR_SIZE)
        };
        let (thumb_len, thumb_at) = if overlay {
            let (h, top, _) = overlay_thumb(pos, total, client, 24.);
            (h, top)
        } else {
            // WebKit lays native scrollbars out on whole CSS pixels (`snappedIntRect`), so a
            // scroller at a fractional offset draws its thumb from the rounded edge.
            let o = handle.bounds().origin;
            let edge = f32::from(match axis {
                Axis::X => o.x,
                Axis::Y => o.y,
            });
            let (len, at) = classic_thumb(pos, total, client, client);
            (len, at + edge.round() - edge)
        };

        let now = Instant::now();
        let opacity = state.update(cx, |s, cx| {
            if s.last_pos.is_some_and(|l| (l - pos).abs() > 0.01) {
                s.hide_soon();
            }
            s.last_pos = Some(pos);
            if let Some((at, next)) = s.paging
                && now >= next
            {
                page(&handle, axis, at);
                s.paging = Some((at, now + PAGE_REPEAT));
            }
            if !overlay {
                return 1.;
            }
            let on = overflow
                && (s.grab.is_some() || s.hover_track || s.hide_at.is_some_and(|h| now < h));
            if on != s.on {
                let from = s.opacity(cx);
                s.on = on;
                s.fade = Some((now, from));
            }
            s.opacity(cx)
        });
        {
            let s = state.read(cx);
            let fading = s.fade.is_some_and(|(start, _)| {
                motion::progress(start, if s.on { FADE_IN } else { FADE_OUT }, cx) < 1.
            });
            if s.paging.is_some() || (overlay && (fading || s.hide_at.is_some_and(|h| now < h))) {
                window.request_animation_frame();
            }
        }

        let s = state.read(cx);
        let thumb_color = if !overlay && (s.hover_thumb || s.grab.is_some()) {
            t.scrollbar_thumb_hover
        } else {
            t.scrollbar_thumb
        };
        let base = div().id(self.id.clone()).flex_1().min_w_0().min_h_0();
        let content = match axis {
            Axis::X => base.overflow_x_scroll(),
            Axis::Y => base.overflow_y_scroll(),
        }
        .track_scroll(&handle)
        .children(self.children);

        let mut root = div().relative().flex().size_full().min_w_0().min_h_0();
        root = match axis {
            Axis::X => root.flex_col(),
            Axis::Y => root.flex_row(),
        }
        .child(content);
        if !overflow {
            return root;
        }
        if !overlay {
            root = match axis {
                Axis::X => root.pb(px(size)),
                Axis::Y => root.pr(px(size)),
            };
        }

        let thumb = div()
            .id("thumb")
            .absolute()
            .rounded(px(if overlay { 99. } else { 8. }))
            .bg(thumb_color.fade(opacity));
        let thumb = match axis {
            Axis::X => thumb.top_0().bottom_0().left(px(thumb_at)).w(px(thumb_len)),
            Axis::Y => thumb.left_0().right_0().top(px(thumb_at)).h(px(thumb_len)),
        };
        let thumb = {
            let state = state.clone();
            thumb.on_hover(move |on, window, cx| {
                state.update(cx, |s, _| s.hover_thumb = *on);
                window.refresh();
            })
        };

        let track = div()
            .id((self.id.clone(), "scrollbar"))
            .absolute()
            .occlude()
            .child(thumb);
        let track = match axis {
            Axis::X => track.left_0().right(px(size)).bottom_0().h(px(size)),
            Axis::Y if overlay => track.top_0().bottom_0().right(px(1.)).w(px(size)),
            Axis::Y => track.top_0().bottom_0().right_0().w(px(size)),
        };
        let track = {
            let (enter, press) = (state.clone(), state.clone());
            let h = handle.clone();
            track
                .on_hover(move |on, window, cx| {
                    enter.update(cx, |s, _| {
                        s.hover_track = *on;
                        if !*on {
                            s.hide_soon();
                        }
                    });
                    window.refresh();
                })
                .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    let at = along(e.position, &h, axis);
                    let (pos, total, client) = metrics(&h, axis);
                    let (len, thumb_at) = if overlay {
                        let (len, top, _) = overlay_thumb(pos, total, client, 24.);
                        (len, top)
                    } else {
                        classic_thumb(pos, total, client, client)
                    };
                    let on_thumb = at >= thumb_at && at <= thumb_at + len;
                    press.update(cx, |s, _| {
                        if overlay {
                            // The overlay jumps so the pointer holds the thumb where it grabbed.
                            let grab = if on_thumb { at - thumb_at } else { len / 2. };
                            s.grab = Some(grab);
                            set_pos(&h, axis, scroll_from_track(at - grab, total, client, 24.));
                        } else if on_thumb {
                            s.grab = Some(at - thumb_at);
                        } else {
                            page(&h, axis, at);
                            s.paging = Some((at, Instant::now() + PAGE_FIRST));
                        }
                    });
                    window.refresh();
                })
        };

        // Pointer capture: drags and paging follow the pointer anywhere in the window.
        let capture = {
            let state = state.clone();
            let h = handle.clone();
            canvas(
                |_, _, _| {},
                move |_, _, window, cx| {
                    let s = state.read(cx);
                    if s.grab.is_none() && s.paging.is_none() {
                        return;
                    }
                    let (moved, up) = (state.clone(), state.clone());
                    let h = h.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let at = along(e.position, &h, axis);
                        moved.update(cx, |s, _| {
                            if let Some(grab) = s.grab {
                                let (_, total, client) = metrics(&h, axis);
                                let pos = if overlay {
                                    scroll_from_track(at - grab, total, client, 24.)
                                } else {
                                    let (len, _) = classic_thumb(0., total, client, client);
                                    scroll_for_thumb(at - grab, len, client, total - client)
                                };
                                set_pos(&h, axis, pos);
                            } else if let Some((_, next)) = s.paging {
                                s.paging = Some((at, next));
                            }
                        });
                        window.refresh();
                    });
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        up.update(cx, |s, _| {
                            s.grab = None;
                            s.paging = None;
                            s.hide_soon();
                        });
                        window.refresh();
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };

        root.child(capture).child(track)
    }
}

/// One `autoscrollPressedPart` step: page toward the pointer unless the thumb would land
/// under it.
fn page(h: &ScrollHandle, axis: Axis, at: f32) {
    let (pos, total, client) = metrics(h, axis);
    let (len, thumb_at) = classic_thumb(pos, total, client, client);
    let dir = if at < thumb_at { -1. } else { 1. };
    let next = (pos + dir * page_step(client)).clamp(0., total - client);
    let (_, next_at) = classic_thumb(next, total, client, client);
    if (at >= next_at && at < next_at + len) || (at >= thumb_at && at < thumb_at + len) {
        return;
    }
    set_pos(h, axis, next);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_geometry() {
        assert_eq!(overlay_thumb(0., 1000., 200., 24.), (40., 0., true));
        assert_eq!(overlay_thumb(800., 1000., 200., 24.), (40., 160., true));
        assert_eq!(overlay_thumb(0., 100_000., 200., 24.).0, 24.);
        assert_eq!(overlay_thumb(0., 200., 200., 24.), (200., 0., false));
        assert!(!overlay_thumb(0., 201., 200., 24.).2);
        assert_eq!(scroll_from_track(80., 1000., 200., 24.), 400.);
        assert_eq!(scroll_from_track(-5., 1000., 200., 24.), 0.);
        assert_eq!(scroll_from_track(500., 1000., 200., 24.), 800.);
        assert_eq!(scroll_from_track(10., 200., 200., 24.), 0.);
    }

    #[test]
    fn classic_geometry() {
        assert_eq!(classic_thumb(0., 1000., 200., 200.), (40., 0.));
        assert_eq!(classic_thumb(800., 1000., 200., 200.), (40., 160.));
        // Positions under one pixel still move the thumb off the top.
        assert_eq!(classic_thumb(1., 1000., 200., 200.), (40., 1.));
        assert_eq!(classic_thumb(13., 1000., 200., 200.), (40., 2.));
        assert_eq!(classic_thumb(0., 200., 200., 200.), (200., 0.));
        assert_eq!(classic_thumb(0., 1089.5, 680., 680.), (424., 0.));
        assert_eq!(classic_thumb(0., 43200., 680., 680.), (17., 0.));
        assert_eq!(page_step(200.), 175.);
        assert_eq!(page_step(400.), 360.);
    }
}
