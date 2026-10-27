//! `<video controls>`: the Observatory's video pane and the zoom lightbox's player. Frames are
//! decoded by the platform's media framework ([`decoder`]) and painted as a GPUI image, so the
//! player sits in the window's paint order (menus, palettes and toasts draw over it) and takes
//! the box's corners and shadow like any element. The controls follow WebKit's inline media
//! controls: play / pause, elapsed time, a scrubber, remaining time, mute and full screen,
//! shown while paused or for a moment after the pointer moves. Space toggles playback,
//! Left / Right seek five seconds, Escape leaves full screen.

pub mod decoder;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Corners, FocusHandle, Focusable, Hsla, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Render, RenderImage, SharedString, StatefulInteractiveElement, Styled,
    Task, Window, canvas, deferred, div, px,
};
use recur_ui::guard::Guarded as _;
use recur_ui::icon::lucide_icon;
use recur_ui::text::{TextStyled as _, text};
use recur_ui::{IconName, Theme, icon, rgba};

use decoder::{Decoder, Status};

/// How long the controls stay after the pointer stops while playing.
const CONTROLS_LINGER: Duration = Duration::from_secs(2);
/// Left / Right.
const SEEK_STEP: f64 = 5.;
const TICK: Duration = Duration::from_millis(16);

/// How a host shows the player (`.obs-image video`, `.chat-att-lb-img`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoOptions {
    /// `autoplay`.
    pub autoplay: bool,
    /// Seconds to seek to once metadata loads (`onloadedmetadata`).
    pub start: f64,
    /// Corner radius of the video box.
    pub radius: f32,
    /// `box-shadow: 0 0 0 1px var(--line)`.
    pub ring: bool,
    /// The lightbox's `0 24px 80px -20px` shadow.
    pub shadow: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        VideoOptions {
            autoplay: false,
            start: 0.,
            radius: 0.,
            ring: true,
            shadow: false,
        }
    }
}

/// `m:ss`, or `h:mm:ss` past an hour, as WebKit's media controls show time.
pub fn format_time(seconds: f64) -> String {
    let s = if seconds.is_finite() {
        seconds.max(0.).floor() as u64
    } else {
        0
    };
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// The video's box: its natural size scaled down to fit `avail` (`max-width/height: 100%`,
/// never upscaled), centered.
pub fn fit_box(natural: (u32, u32), avail: Bounds<Pixels>) -> Bounds<Pixels> {
    let (nw, nh) = (natural.0 as f32, natural.1 as f32);
    let (aw, ah) = (f32::from(avail.size.width), f32::from(avail.size.height));
    if nw <= 0. || nh <= 0. || aw <= 0. || ah <= 0. {
        return Bounds::new(avail.origin, gpui_kit::size(px(0.), px(0.)));
    }
    let s = (aw / nw).min(ah / nh).min(1.);
    let (w, h) = (nw * s, nh * s);
    Bounds::new(
        gpui_kit::point(
            avail.origin.x + px((aw - w) / 2.),
            avail.origin.y + px((ah - h) / 2.),
        ),
        gpui_kit::size(px(w), px(h)),
    )
}

pub struct VideoPlayer {
    path: PathBuf,
    opts: VideoOptions,
    decoder: Option<Decoder>,
    status: Status,
    started: bool,
    muted: bool,
    image: Option<Arc<RenderImage>>,
    stale: Vec<Arc<RenderImage>>,
    focus: FocusHandle,
    moved_at: Option<Instant>,
    hovered: bool,
    scrubbing: bool,
    fullscreen: bool,
    area: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    track: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    _tick: Option<Task<()>>,
}

impl Focusable for VideoPlayer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl VideoPlayer {
    pub fn new(path: impl Into<PathBuf>, opts: VideoOptions, cx: &mut Context<Self>) -> Self {
        let path = path.into();
        let decoder = Decoder::open(&path);
        let failed = decoder.is_none();
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |p, cx| p.tick(cx)).is_err() {
                    return;
                }
            }
        });
        VideoPlayer {
            path,
            opts,
            decoder,
            status: Status {
                failed,
                ..Status::default()
            },
            started: false,
            muted: false,
            image: None,
            stale: Vec::new(),
            focus: cx.focus_handle(),
            moved_at: None,
            hovered: false,
            scrubbing: false,
            fullscreen: false,
            area: Rc::default(),
            track: Rc::default(),
            _tick: Some(tick),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// `currentTime`.
    pub fn position(&self) -> f64 {
        self.status.position
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn playing(&self) -> bool {
        self.status.playing
    }

    pub fn play(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = &mut self.decoder {
            d.play();
        }
        self.refresh(cx);
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = &mut self.decoder {
            d.pause();
        }
        self.refresh(cx);
    }

    /// `currentTime = t`.
    pub fn seek(&mut self, t: f64, cx: &mut Context<Self>) {
        let t = if self.status.duration > 0. {
            t.clamp(0., self.status.duration)
        } else {
            t.max(0.)
        };
        if let Some(d) = &mut self.decoder {
            d.seek(t);
        }
        self.status.position = t;
        self.status.ended = false;
        cx.notify();
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.status.playing {
            self.pause(cx);
        } else {
            self.play(cx);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = &self.decoder {
            self.status = d.status();
        }
        cx.notify();
    }

    /// One poll: metadata, the start seek and autoplay once ready, and the newest frame.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.decoder else { return };
        let mut changed = false;
        let status = d.status();
        if status.ready && !self.started {
            self.started = true;
            if self.opts.start > 0. {
                d.seek(self.opts.start);
            }
            if self.opts.autoplay {
                d.play();
            }
            changed = true;
        }
        if let Some(f) = d.frame()
            && let Some(buf) = image::RgbaImage::from_raw(f.width, f.height, f.bgra)
        {
            let img = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(
                buf
            )]));
            if let Some(old) = self.image.replace(img) {
                self.stale.push(old);
            }
            changed = true;
        }
        let linger = self.moved_at.is_some_and(|t| {
            recur_ui::motion::now(cx).saturating_duration_since(t) < CONTROLS_LINGER + TICK
        });
        if status != self.status || changed || linger {
            self.status = status;
            cx.notify();
        }
    }

    fn controls_shown(&self, cx: &App) -> bool {
        !self.status.playing
            || self.scrubbing
            || self.moved_at.is_some_and(|t| {
                self.hovered
                    && recur_ui::motion::now(cx).saturating_duration_since(t) < CONTROLS_LINGER
            })
    }

    fn scrub_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(track) = self.track.get() else {
            return;
        };
        let w = f32::from(track.size.width);
        if w <= 0. || self.status.duration <= 0. {
            return;
        }
        let frac = (f32::from(x - track.left()) / w).clamp(0., 1.) as f64;
        self.seek(frac * self.status.duration, cx);
    }

    fn key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = &e.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return;
        }
        match e.keystroke.key.as_str() {
            "space" => self.toggle(cx),
            "left" => self.seek(self.status.position - SEEK_STEP, cx),
            "right" => self.seek(self.status.position + SEEK_STEP, cx),
            "escape" if self.fullscreen => {
                self.fullscreen = false;
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn button(
        &self,
        id: &'static str,
        glyph: &'static str,
        label: &'static str,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let button = div()
            .id(id)
            .debug_selector(move || format!("video-{id}"))
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(Hsla::from(rgba(255, 255, 255, 0.12))))
            .child(
                lucide_icon(glyph)
                    .size(px(14.))
                    .color(Hsla::from(rgba(255, 255, 255, 0.92))),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.guarded_listener(move |p, _, _, cx| {
                cx.stop_propagation();
                f(p, cx);
            }));
        recur_ui::tooltip::titled(SharedString::from(format!("video-t-{id}")), label, button)
    }

    fn controls(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let s = self.status;
        let frac = if s.duration > 0. {
            (s.position / s.duration).clamp(0., 1.) as f32
        } else {
            0.
        };
        let white = |a: f32| Hsla::from(rgba(255, 255, 255, a));
        let time = |t: String| {
            text(t)
                .size(11.)
                .tabular()
                .color(rgba(255, 255, 255, 0.92))
                .nowrap()
        };
        let track_cell = self.track.clone();
        let scrubber = div()
            .id("video-scrubber")
            .debug_selector(|| "video-scrubber".into())
            .flex_1()
            .min_w(px(24.))
            .h(px(28.))
            .flex()
            .items_center()
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(white(0.28))
                    .child(
                        canvas(move |b, _, _| track_cell.set(Some(b)), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(gpui_kit::relative(frac))
                            .rounded(px(2.))
                            .bg(white(0.9)),
                    ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|p, e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    p.scrubbing = true;
                    p.scrub_to(e.position.x, cx);
                }),
            );
        let play = if s.playing {
            self.button("pause", "pause", "Pause", cx, |p, cx| p.pause(cx))
                .into_any_element()
        } else {
            self.button("play", "play", "Play", cx, |p, cx| p.play(cx))
                .into_any_element()
        };
        let mute = if self.muted {
            self.button("unmute", "volume-x", "Unmute", cx, |p, cx| {
                p.set_muted(false, cx)
            })
            .into_any_element()
        } else {
            self.button("mute", "volume-2", "Mute", cx, |p, cx| {
                p.set_muted(true, cx)
            })
            .into_any_element()
        };
        let full = if self.fullscreen {
            self.button("exit-full", "minimize", "Exit Full Screen", cx, |p, cx| {
                p.fullscreen = false;
                cx.notify();
            })
            .into_any_element()
        } else {
            self.button("full", "maximize", "Enter Full Screen", cx, |p, cx| {
                p.fullscreen = true;
                cx.notify();
            })
            .into_any_element()
        };
        div()
            .absolute()
            .left(px(6.))
            .right(px(6.))
            .bottom(px(6.))
            .h(px(32.))
            .px(px(4.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(8.))
            .bg(Hsla::from(rgba(28, 28, 30, 0.72)))
            .debug_selector(|| "video-controls".into())
            .child(play)
            .child(time(format_time(s.position)))
            .child(scrubber)
            .child(time(format!(
                "-{}",
                format_time((s.duration - s.position).max(0.))
            )))
            .child(mute)
            .child(full)
            .into_any_element()
    }

    pub fn set_muted(&mut self, muted: bool, cx: &mut Context<Self>) {
        self.muted = muted;
        if let Some(d) = &self.decoder {
            d.set_muted(muted);
        }
        cx.notify();
    }

    /// The loading state: the pane's poster (a black 16:9 box with a play glyph), or an error.
    fn poster(&self, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        div()
            .debug_selector(|| "video-poster".into())
            .border_1()
            .border_color(Hsla::from(t.line))
            .bg(Hsla::from(recur_ui::hex(0x000000)))
            .w(px(320.))
            .h(px(180.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                icon(IconName::Play)
                    .size(px(28.))
                    .color(Hsla::from(t.muted)),
            )
            .into_any_element()
    }

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        for img in self.stale.drain(..) {
            let _ = window.drop_image(img);
        }
        let t = Theme::of(cx).tokens;
        let area = self.area.get();
        let frame = (self.status.ready && self.image.is_some())
            .then(|| area.map(|a| fit_box(self.status.size, a)))
            .flatten()
            .filter(|b| b.size.width > px(0.));
        let area_cell = self.area.clone();
        let entity = cx.entity().downgrade();
        let measure = canvas(
            move |b, _, cx| {
                if area_cell.get() != Some(b) {
                    area_cell.set(Some(b));
                    if let Some(e) = entity.upgrade() {
                        cx.notify(e.entity_id());
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let mut root = div()
            .id(SharedString::from(format!("video:{}", self.path.display())))
            .debug_selector(|| "video".into())
            .track_focus(&self.focus)
            .key_context("Video")
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(measure)
            .on_key_down(cx.guarded_listener(|p, e: &KeyDownEvent, _, cx| p.key(e, cx)))
            .on_hover(cx.guarded_listener(|p, on: &bool, _, cx| {
                p.hovered = *on;
                cx.notify();
            }))
            .on_mouse_move(cx.guarded_listener(|p, e: &MouseMoveEvent, _, cx| {
                p.moved_at = Some(recur_ui::motion::now(cx));
                if p.scrubbing && e.pressed_button == Some(MouseButton::Left) {
                    p.scrub_to(e.position.x, cx);
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.guarded_listener(|p, _: &MouseUpEvent, _, cx| {
                    if p.scrubbing {
                        p.scrubbing = false;
                        cx.notify();
                    }
                }),
            );
        let Some(b) = frame else {
            return root.child(self.poster(cx)).into_any_element();
        };
        let (radius, ring) = (self.opts.radius, self.opts.ring);
        let img = self.image.clone();
        let line = Hsla::from(t.line);
        let video = canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                if ring {
                    window.paint_quad(
                        gpui_kit::outline(b.dilate(px(1.)), line, gpui_kit::BorderStyle::Solid)
                            .corner_radii(px(radius + 1.)),
                    );
                }
                window.paint_quad(
                    gpui_kit::fill(b, Hsla::from(recur_ui::hex(0x000000))).corner_radii(px(radius)),
                );
                if let Some(img) = img {
                    let _ = window.paint_image(b, b, Corners::all(px(radius)), img, 0, false);
                }
            },
        )
        .absolute()
        .size_full();
        let shown = self.controls_shown(cx);
        let mut boxed = div()
            .relative()
            .flex_none()
            .w(b.size.width)
            .h(b.size.height)
            .when(self.opts.shadow, |d| {
                d.child(recur_ui::shadow::box_shadow(
                    &[recur_ui::shadow::css_box_shadow(
                        0.,
                        24.,
                        80.,
                        -20.,
                        rgba(0, 0, 0, 0.85),
                    )],
                    radius,
                    0.,
                ))
            })
            .child(video)
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|p, _, window, cx| {
                    // The video takes the press (a lightbox closes on presses around it).
                    cx.stop_propagation();
                    window.focus(&p.focus, cx);
                }),
            );
        if shown {
            boxed = boxed.child(self.controls(cx));
        }
        let boxed = div()
            .id("video-box")
            .child(boxed)
            .on_click(cx.guarded_listener(|p, _, _, cx| p.toggle(cx)));
        if self.fullscreen {
            let vp = window.viewport_size();
            // Element full screen: the video over the whole window.
            root = root.child(deferred(
                gpui_kit::anchored()
                    .position_mode(gpui_kit::AnchoredPositionMode::Window)
                    .position(gpui_kit::point(px(0.), px(0.)))
                    .child(
                        div()
                            .id("video-full")
                            .debug_selector(|| "video-fullscreen".into())
                            .w(vp.width)
                            .h(vp.height)
                            .bg(Hsla::from(recur_ui::hex(0x000000)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .occlude()
                            .child(boxed),
                    ),
            ));
            return root.into_any_element();
        }
        root.child(boxed).into_any_element()
    }
}

impl Render for VideoPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_like_webkits_controls() {
        assert_eq!(format_time(0.), "0:00");
        assert_eq!(format_time(5.9), "0:05");
        assert_eq!(format_time(75.), "1:15");
        assert_eq!(format_time(3725.), "1:02:05");
        assert_eq!(format_time(f64::NAN), "0:00");
    }

    #[test]
    fn the_box_fits_without_upscaling() {
        let area = Bounds::new(
            gpui_kit::point(px(0.), px(0.)),
            gpui_kit::size(px(400.), px(300.)),
        );
        let b = fit_box((160, 90), area);
        assert_eq!(
            (f32::from(b.size.width), f32::from(b.size.height)),
            (160., 90.)
        );
        assert_eq!(f32::from(b.origin.x), 120.);
        let b = fit_box((1920, 1080), area);
        assert_eq!(f32::from(b.size.width), 400.);
        assert_eq!(f32::from(b.size.height), 225.);
    }
}
