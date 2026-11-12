//! Scrollbars from `_base.scss`. Its `* { scrollbar-color: var(--scrollbar-thumb) transparent }`
//! wins over the `::-webkit-scrollbar` rules in WKWebView, so every scroller gets AppKit's
//! overlay scroller tinted with the thumb color ([`Overlay`]): no layout space, shown while the
//! user scrolls, wider with a track under the pointer. With "Show scroll bars: Always"
//! ([`legacy`]) WebKit draws the custom `::-webkit-scrollbar` instead (11px, transparent track,
//! `--scrollbar-thumb` thumb with radius 8, `--scrollbar-thumb-hover` under the pointer), which
//! takes layout space while the content overflows. Scrollers that keep sticky rows use the 6px
//! overlay thumb of `lib/overlay-scrollbar.ts` (`.overlay-sb`) instead, which floats above the
//! rows and fades in while scrolling, hovered or dragged.

use crate::motion::{self, Easing};
use crate::theme::{SCROLLBAR_SIZE, Theme};
use crate::widgets::Axis;
use gpui_kit::{
    AnyElement, App, DispatchPhase, ElementId, Global, InteractiveElement, IntoElement,
    MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, RenderOnce,
    ScrollHandle, StatefulInteractiveElement, Styled, Window, canvas, div, point, px,
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

/// AppKit's overlay `NSScroller` at the regular control size, measured from `NSScrollerImp`:
/// a 17px scroller whose knob keeps 3px from the outer edge and the track ends, 6px thick at
/// rest and 11px expanded, at least 26px long.
pub const OVERLAY_BOX: f32 = 17.;
const OVERLAY_INSET: f32 = 3.;
const OVERLAY_THIN: f32 = 6.;
const OVERLAY_WIDE: f32 = 11.;
const OVERLAY_MIN_KNOB: f32 = 26.;
/// The expanded track over the thumb color. AppKit paints a faint slot, and `scrollbar-color`'s
/// transparent track leaves no color of its own.
const OVERLAY_TRACK: f32 = 0.25;
/// A scroll shows the knob at once (`knobAlpha` 1 on the first sample); it holds this long
/// after the last scroll, pointer leave or release, then fades out ease-in-out.
const OVERLAY_HOLD: Duration = Duration::from_millis(500);
const OVERLAY_FADE: Duration = Duration::from_millis(225);
/// `mouseEnteredScroller` expands the knob after a rollover delay, then widens it.
const OVERLAY_ROLLOVER: Duration = Duration::from_millis(40);
const OVERLAY_EXPAND: Duration = Duration::from_millis(120);

/// The overlay knob along a track of `track` px: (length, offset from the track start).
pub fn overlay_knob(pos: f32, total: f32, visible: f32, track: f32) -> (f32, f32) {
    let slot = track - 2. * OVERLAY_INSET;
    let max = total - visible;
    if max <= 0. || slot <= 0. {
        return (0., 0.);
    }
    let len = (visible / total * slot).max(OVERLAY_MIN_KNOB).min(slot);
    (len, OVERLAY_INSET + pos.clamp(0., max) / max * (slot - len))
}

/// The scroll position that puts the overlay knob's start at `at` along the track.
pub fn overlay_scroll(at: f32, total: f32, visible: f32, track: f32) -> f32 {
    let (len, _) = overlay_knob(0., total, visible, track);
    scroll_for_thumb(
        at - OVERLAY_INSET,
        len,
        track - 2. * OVERLAY_INSET,
        total - visible,
    )
}

/// The show, fade and expand timing of an AppKit overlay scroller.
#[derive(Clone, Copy, Debug, Default)]
pub struct Overlay {
    /// The last scroll, pointer leave or release, which starts the hold before the fade.
    idle: Option<Instant>,
    hovered: bool,
    pressed: bool,
    /// When the knob starts to widen; it stays wide until the scroller hides.
    expand: Option<Instant>,
}

impl Overlay {
    /// A wheel or trackpad scroll. Programmatic scrolls leave the scroller hidden.
    pub fn scrolled(&mut self, now: Instant) {
        if !self.visible(now) {
            self.expand = None;
        }
        self.idle = Some(now);
    }

    /// The pointer is over the scroller's box or not; whether that changed anything. A hidden
    /// scroller ignores the pointer, as WebKit hit tests overlay scrollbars only while they show.
    pub fn hover(&mut self, on: bool, now: Instant) -> bool {
        let on = on && (self.hovered || self.visible(now));
        if on == self.hovered {
            return false;
        }
        self.hovered = on;
        if on {
            self.expand.get_or_insert(now + OVERLAY_ROLLOVER);
        } else {
            self.idle = Some(now);
        }
        true
    }

    /// The pointer moved inside the scroll container. A scroller still in its hold stays up,
    /// so the pointer can reach it; one already fading is left to fade.
    pub fn pointer_moved(&mut self, now: Instant) {
        if !self.held()
            && self
                .idle
                .is_some_and(|t| now.saturating_duration_since(t) < OVERLAY_HOLD)
        {
            self.idle = Some(now);
        }
    }

    /// A press on the scroller (track paging or a knob drag) starts or ends.
    pub fn press(&mut self, on: bool, now: Instant) {
        if self.pressed && !on {
            self.idle = Some(now);
        }
        self.pressed = on;
    }

    fn held(&self) -> bool {
        self.hovered || self.pressed
    }

    pub fn opacity(&self, now: Instant) -> f32 {
        if self.held() {
            return 1.;
        }
        let Some(t) = self.idle else { return 0. };
        let fading = now
            .saturating_duration_since(t)
            .saturating_sub(OVERLAY_HOLD);
        1. - Easing::EASE_IN_OUT.apply(fading.as_secs_f32() / OVERLAY_FADE.as_secs_f32())
    }

    /// Showing, and so taking the pointer.
    pub fn visible(&self, now: Instant) -> bool {
        self.opacity(now) > 0.
    }

    /// The knob's thickness across the track.
    pub fn thickness(&self, now: Instant) -> f32 {
        let p = match self.expand {
            Some(t) if self.visible(now) => {
                let d = now.saturating_duration_since(t).as_secs_f32();
                Easing::EASE_IN_OUT.apply(d / OVERLAY_EXPAND.as_secs_f32())
            }
            _ => 0.,
        };
        OVERLAY_THIN + (OVERLAY_WIDE - OVERLAY_THIN) * p
    }

    /// The expanded track's alpha over the thumb color.
    pub fn track_alpha(&self, now: Instant) -> f32 {
        if self.expand.is_some_and(|t| now >= t) {
            self.opacity(now) * OVERLAY_TRACK
        } else {
            0.
        }
    }

    /// The knob's distance from the scroller's outer edge.
    pub fn edge_inset(&self) -> f32 {
        OVERLAY_INSET
    }

    /// Whether a later frame looks different: holding, fading or widening.
    pub fn animating(&self, now: Instant) -> bool {
        if self.held() {
            return self.expand.is_some_and(|t| now < t + OVERLAY_EXPAND);
        }
        self.idle
            .is_some_and(|t| now < t + OVERLAY_HOLD + OVERLAY_FADE)
    }
}

/// "Show scroll bars: Always" (`NSScroller.preferredScrollerStyle` legacy): scrollers keep the
/// classic bar. Off until [`watch_system_style`] or [`set_legacy`] sets it, so Linux and tests
/// get the overlay scroller.
pub fn legacy(cx: &App) -> bool {
    cx.try_global::<Legacy>().is_some_and(|l| l.0)
}

/// The layout space a scrollbar takes while its content overflows.
pub fn layout_size(cx: &App) -> f32 {
    if legacy(cx) {
        f32::from(SCROLLBAR_SIZE)
    } else {
        0.
    }
}

/// Switch between the classic bar and overlay scrollers, repainting.
pub fn set_legacy(legacy: bool, cx: &mut App) {
    cx.set_global(Legacy(legacy));
    cx.refresh_windows();
}

struct Legacy(bool);

impl Global for Legacy {}

/// Follow the macOS scroller style preference, repainting when it changes.
pub fn watch_system_style(cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        use futures::StreamExt;
        use objc2_app_kit::{
            NSPreferredScrollerStyleDidChangeNotification, NSScroller, NSScrollerStyle,
        };
        use objc2_foundation::{MainThreadMarker, NSNotificationCenter};

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let legacy = move || NSScroller::preferredScrollerStyle(mtm) == NSScrollerStyle::Legacy;
        set_legacy(legacy(), cx);
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        // The center may call back while GPUI holds the app; hand off to the executor.
        let block = block2::RcBlock::new(move |_| {
            let _ = tx.unbounded_send(());
        });
        // SAFETY: the block only sends on a channel, and the observer lives as long as the app.
        #[allow(unsafe_code)]
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSPreferredScrollerStyleDidChangeNotification),
                None,
                None,
                &block,
            )
        };
        cx.spawn(async move |cx| {
            let _observer = observer;
            while rx.next().await.is_some() {
                cx.update(|cx| set_legacy(legacy(), cx));
            }
        })
        .detach();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
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
    /// A wheel event since the last render, so the next move shows the overlay scroller.
    wheeled: bool,
    mac: Overlay,
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
    fn hide_soon(&mut self, now: Instant) {
        self.hide_at = (self.grab.is_none() && !self.hover_track).then(|| now + HIDE);
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

/// A scroller with the web UI's scrollbar. Vertical with the overlay scroller by default.
#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    axis: Axis,
    overlay: bool,
    handle: Option<ScrollHandle>,
    scroller: Option<ScrollerStyle>,
    children: Vec<AnyElement>,
}

type ScrollerStyle =
    Box<dyn FnOnce(gpui_kit::Stateful<gpui_kit::Div>) -> gpui_kit::Stateful<gpui_kit::Div>>;

pub fn scroll_area(id: impl Into<ElementId>) -> ScrollArea {
    ScrollArea {
        id: id.into(),
        axis: Axis::Y,
        overlay: false,
        handle: None,
        scroller: None,
        children: Vec::new(),
    }
}

impl ScrollArea {
    /// Scroll sideways; the bar sits along the bottom.
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

    /// Style the scroll container itself, as when the CSS scroller is also the layout box
    /// (`display: flex; flex-direction: column; overflow: auto` with padding and gaps, whose
    /// items shrink to fit before it scrolls).
    pub fn scroller(
        mut self,
        f: impl FnOnce(gpui_kit::Stateful<gpui_kit::Div>) -> gpui_kit::Stateful<gpui_kit::Div> + 'static,
    ) -> Self {
        self.scroller = Some(Box::new(f));
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

impl ScrollArea {
    fn render_view(self, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let (axis, overlay) = (self.axis, self.overlay);
        let native = !overlay && !legacy(cx);
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| State::default());
        let handle = match self.handle {
            Some(h) => h,
            None => state.read(cx).handle.clone(),
        };
        let (pos, total, client) = metrics(&handle, axis);
        let overflow = total - client > if overlay { 1. } else { 0.5 };
        let size = if overlay {
            6.
        } else if native {
            OVERLAY_BOX
        } else {
            f32::from(SCROLLBAR_SIZE)
        };
        let (thumb_len, thumb_at) = if overlay {
            let (h, top, _) = overlay_thumb(pos, total, client, 24.);
            (h, top)
        } else if native {
            overlay_knob(pos, total, client, client)
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

        let now = crate::motion::now(cx);
        let opacity = state.update(cx, |s, cx| {
            let moved = s.last_pos.is_some_and(|l| (l - pos).abs() > 0.01);
            if moved {
                s.hide_soon(now);
            }
            if std::mem::take(&mut s.wheeled) && moved {
                s.mac.scrolled(now);
            }
            s.last_pos = Some(pos);
            if let Some((at, next)) = s.paging
                && now >= next
            {
                page(&handle, axis, at, native);
                s.paging = Some((at, now + PAGE_REPEAT));
            }
            if native {
                return s.mac.opacity(now);
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
            if s.paging.is_some()
                || (overlay && (fading || s.hide_at.is_some_and(|h| now < h)))
                || (native && overflow && s.mac.animating(now))
            {
                window.request_animation_frame();
            }
        }

        let s = state.read(cx);
        let thumb_color = if !overlay && (s.hover_thumb || s.grab.is_some()) {
            t.scrollbar_thumb_hover
        } else {
            t.scrollbar_thumb
        };
        let (thickness, track_alpha) = (s.mac.thickness(now), s.mac.track_alpha(now));
        let mut base = div().id(self.id.clone()).flex_1().min_w_0().min_h_0();
        if let Some(f) = self.scroller {
            base = f(base);
        }
        let mut content = match axis {
            Axis::X => base.overflow_x_scroll(),
            Axis::Y => base.overflow_y_scroll(),
        }
        .track_scroll(&handle)
        .children(self.children);
        if native {
            let state = state.clone();
            let moved = state.clone();
            content = content
                .on_scroll(move |_, _, cx| {
                    state.update(cx, |s, _| s.wheeled = true);
                })
                .on_mouse_move(move |_, _, cx| {
                    let now = crate::motion::now(cx);
                    moved.update(cx, |s, _| s.mac.pointer_moved(now));
                });
        }

        // A column for both axes: a row root sizes a vertical scroller's height by laying its
        // content out once more per pass.
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(content);
        // A hidden overlay scroller takes no pointer, so the content under it stays clickable.
        if !overflow || (native && opacity <= 0.) {
            return root;
        }
        if !overlay && !native {
            root = match axis {
                Axis::X => root.pb(px(size)),
                Axis::Y => root.pr(px(size)),
            };
        }

        let thumb = div()
            .id("thumb")
            .absolute()
            .rounded(px(if native {
                thickness / 2.
            } else if overlay {
                99.
            } else {
                8.
            }))
            .bg(thumb_color.fade(opacity));
        let edge = s.mac.edge_inset();
        let thumb = match axis {
            Axis::X if native => thumb.bottom(px(edge)).h(px(thickness)),
            Axis::Y if native => thumb.right(px(edge)).w(px(thickness)),
            Axis::X => thumb.top_0().bottom_0(),
            Axis::Y => thumb.left_0().right_0(),
        };
        let thumb = match axis {
            Axis::X => thumb.left(px(thumb_at)).w(px(thumb_len)),
            Axis::Y => thumb.top(px(thumb_at)).h(px(thumb_len)),
        };
        let thumb = {
            let state = state.clone();
            thumb.on_hover(move |on, window, cx| {
                state.update(cx, |s, _| s.hover_thumb = *on);
                window.refresh();
            })
        };

        let mut track = div()
            .id((self.id.clone(), "scrollbar"))
            .absolute()
            .occlude()
            .child(thumb);
        if native {
            track = track.bg(thumb_color.fade(track_alpha));
        }
        let track = match axis {
            Axis::X if native => track.left_0().right_0().bottom_0().h(px(size)),
            Axis::X => track.left_0().right(px(size)).bottom_0().h(px(size)),
            Axis::Y if overlay => track.top_0().bottom_0().right(px(1.)).w(px(size)),
            Axis::Y => track.top_0().bottom_0().right_0().w(px(size)),
        };
        let track = {
            let (enter, press) = (state.clone(), state.clone());
            let h = handle.clone();
            track
                .on_hover(move |on, window, cx| {
                    let now = crate::motion::now(cx);
                    enter.update(cx, |s, _| {
                        s.hover_track = *on;
                        s.mac.hover(*on, now);
                        if !*on {
                            s.hide_soon(now);
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
                        knob(native, pos, total, client)
                    };
                    let on_thumb = at >= thumb_at && at <= thumb_at + len;
                    let now = crate::motion::now(cx);
                    press.update(cx, |s, _| {
                        s.mac.press(true, now);
                        if overlay {
                            // The overlay jumps so the pointer holds the thumb where it grabbed.
                            let grab = if on_thumb { at - thumb_at } else { len / 2. };
                            s.grab = Some(grab);
                            set_pos(&h, axis, scroll_from_track(at - grab, total, client, 24.));
                        } else if on_thumb {
                            s.grab = Some(at - thumb_at);
                        } else {
                            page(&h, axis, at, native);
                            s.paging = Some((at, now + PAGE_FIRST));
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
                                } else if native {
                                    overlay_scroll(at - grab, total, client, client)
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
                        let now = crate::motion::now(cx);
                        up.update(cx, |s, _| {
                            s.grab = None;
                            s.paging = None;
                            s.mac.press(false, now);
                            s.hide_soon(now);
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

impl RenderOnce for ScrollArea {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        crate::guard::component(self, window, cx, Self::render_view)
    }
}

/// The overlay knob or the classic thumb: (length, offset along the track).
fn knob(native: bool, pos: f32, total: f32, client: f32) -> (f32, f32) {
    if native {
        overlay_knob(pos, total, client, client)
    } else {
        classic_thumb(pos, total, client, client)
    }
}

/// One `autoscrollPressedPart` step: page toward the pointer unless the thumb would land
/// under it.
fn page(h: &ScrollHandle, axis: Axis, at: f32, native: bool) {
    let (pos, total, client) = metrics(h, axis);
    let (len, thumb_at) = knob(native, pos, total, client);
    let dir = if at < thumb_at { -1. } else { 1. };
    let next = (pos + dir * page_step(client)).clamp(0., total - client);
    let (_, next_at) = knob(native, next, total, client);
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
    fn overlay_knob_geometry() {
        // 3px from the track ends, proportional, at least 26px.
        assert_eq!(overlay_knob(0., 1030., 206., 206.), (40., 3.));
        assert_eq!(overlay_knob(824., 1030., 206., 206.), (40., 163.));
        assert_eq!(overlay_knob(0., 100_000., 206., 206.).0, 26.);
        assert_eq!(overlay_knob(0., 206., 206., 206.), (0., 0.));
        assert_eq!(overlay_scroll(83., 1030., 206., 206.), 412.);
        assert_eq!(overlay_scroll(-10., 1030., 206., 206.), 0.);
        assert_eq!(overlay_scroll(500., 1030., 206., 206.), 824.);
    }

    /// Moving inside the container keeps a showing scroller up; stopping lets it fade, and
    /// moving again once it fades does not bring it back.
    #[test]
    fn pointer_moves_hold_the_overlay_until_it_fades() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut o = Overlay::default();
        o.pointer_moved(t0);
        assert_eq!(o.opacity(t0), 0., "moving alone does not show it");
        o.scrolled(t0);
        for ms in (100..=2000).step_by(100) {
            o.pointer_moved(at(ms));
            assert_eq!(o.opacity(at(ms)), 1., "held at {ms}ms");
        }
        assert_eq!(o.opacity(at(2400)), 1.);
        assert!(o.opacity(at(2600)) < 1., "fades once the pointer stops");
        o.pointer_moved(at(2600));
        assert!(o.opacity(at(2600)) < 1., "a fading scroller is not revived");
        assert_eq!(o.opacity(at(2800)), 0.);
    }

    #[test]
    fn overlay_shows_on_scroll_and_fades() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut o = Overlay::default();
        assert_eq!(o.opacity(t0), 0.);
        assert!(!o.animating(t0));
        o.scrolled(t0);
        assert_eq!(o.opacity(t0), 1.);
        assert_eq!(o.opacity(at(500)), 1.);
        assert!((o.opacity(at(612)) - 0.5).abs() < 0.05);
        assert_eq!(o.opacity(at(725)), 0.);
        assert!(o.animating(at(600)));
        assert!(!o.animating(at(725)));
        // Another scroll restarts the hold.
        o.scrolled(at(400));
        assert_eq!(o.opacity(at(900)), 1.);
        assert_eq!(o.thickness(at(400)), 6.);
        assert_eq!(o.track_alpha(at(400)), 0.);
    }

    #[test]
    fn overlay_hover_expands_and_holds() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut o = Overlay::default();
        // A hidden scroller does not take the pointer.
        assert!(!o.hover(true, t0));
        assert_eq!(o.opacity(at(10)), 0.);
        o.scrolled(at(100));
        assert!(o.hover(true, at(200)));
        assert_eq!(o.thickness(at(239)), 6.);
        assert_eq!(o.track_alpha(at(239)), 0.);
        assert!((o.thickness(at(300)) - 8.5).abs() < 0.5);
        assert_eq!(o.thickness(at(360)), 11.);
        assert_eq!(o.track_alpha(at(360)), 0.25);
        assert!(o.animating(at(300)));
        assert!(!o.animating(at(360)));
        // Hovered, it stays.
        assert_eq!(o.opacity(at(5000)), 1.);
        // Leaving holds, fades, and stays wide until hidden.
        assert!(o.hover(false, at(5000)));
        assert_eq!(o.thickness(at(5600)), 11.);
        assert_eq!(o.opacity(at(5725)), 0.);
        assert_eq!(o.thickness(at(5725)), 6.);
        // Shown again by a scroll, it starts thin.
        o.scrolled(at(6000));
        assert_eq!(o.thickness(at(6000)), 6.);
    }

    #[test]
    fn overlay_press_holds_until_release() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut o = Overlay::default();
        // A release without a press shows nothing.
        o.press(false, t0);
        assert_eq!(o.opacity(t0), 0.);
        o.scrolled(t0);
        o.hover(true, at(100));
        o.press(true, at(150));
        o.hover(false, at(200));
        assert_eq!(o.opacity(at(3000)), 1.);
        o.press(false, at(3000));
        assert_eq!(o.opacity(at(3500)), 1.);
        assert_eq!(o.opacity(at(3725)), 0.);
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
