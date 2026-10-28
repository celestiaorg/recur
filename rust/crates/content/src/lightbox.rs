//! `AttachmentLightbox.svelte`: a full-window image viewer. The image fits the window (minus
//! 32px padding) and a click toggles actual size when it was shrunk; Escape closes, ⌘C copies,
//! right-click opens Copy Image. Hosts raise unseen attention while it is open
//! (`MarkdownEvent::LightboxOpen`, `setChatOccluder`).

use recur_ui::guard::Guarded;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, AppContext, Context, Corners, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, ParentElement,
    Render, StatefulInteractiveElement, Styled, Task, Window, canvas, deferred, div, px,
};
use recur_ui::shadow::{box_shadow, css_box_shadow};
use recur_ui::style::{anchored_at, backdrop_blur};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, Theme, icon, rgba};

use crate::image_menu::{ImageContextMenu, ImageMenuEvent, copy_png};
use crate::images::{self, Decoded};
use crate::render::exact::{hgap, vgap};

#[derive(Clone, Debug, PartialEq)]
pub enum LightboxEvent {
    Toast { error: bool, message: String },
}

type OnClose = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct AttachmentLightbox {
    src: String,
    name: String,
    image: Option<Decoded>,
    /// `video`: the clip plays here instead of an image.
    video: Option<Entity<crate::video::VideoPlayer>>,
    actual: bool,
    copied: bool,
    copy_timer: Option<Task<()>>,
    menu: Option<Entity<ImageContextMenu>>,
    on_close: OnClose,
    focus: FocusHandle,
    _load: Option<Task<()>>,
}

impl EventEmitter<LightboxEvent> for AttachmentLightbox {}

impl Focusable for AttachmentLightbox {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The fitted display size (`max-width/height: 100%`, `object-fit: contain`, never larger
/// than natural) and whether the image was shrunk (`scaled`).
pub fn fit(natural: (f32, f32), avail: (f32, f32)) -> ((f32, f32), bool) {
    let (nw, nh) = natural;
    if nw <= 0. || nh <= 0. {
        return ((0., 0.), false);
    }
    let s = (avail.0 / nw).min(avail.1 / nh).min(1.);
    ((nw * s, nh * s), nw > avail.0 || nh > avail.1)
}

impl AttachmentLightbox {
    pub fn new(
        src: String,
        name: String,
        image: Option<Decoded>,
        on_close: OnClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let load = image
            .is_none()
            .then(|| images::fetch(&src, cx))
            .flatten()
            .map(|fut| {
                cx.spawn(async move |this, cx| {
                    let res = cx.background_spawn(fut).await;
                    let _ = this.update(cx, |lb, cx| {
                        if let Ok(d) = res {
                            lb.image = Some(d);
                        }
                        cx.notify();
                    });
                })
            });
        AttachmentLightbox {
            src,
            name,
            image,
            video: None,
            actual: false,
            copied: false,
            copy_timer: None,
            menu: None,
            on_close,
            focus,
            _load: load,
        }
    }

    /// The lightbox on a video file (`video`, `autoplay`, `currentTime = startTime` once
    /// its metadata loads), as the Observatory's zoom opens it.
    pub fn video(
        path: std::path::PathBuf,
        name: String,
        start: f64,
        on_close: OnClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let player = cx.new(|cx| {
            crate::video::VideoPlayer::new(
                path.clone(),
                crate::video::VideoOptions {
                    autoplay: true,
                    start,
                    radius: 8.,
                    ring: false,
                    shadow: true,
                },
                cx,
            )
        });
        let mut lb = Self::new(
            path.to_string_lossy().into_owned(),
            name,
            None,
            on_close,
            window,
            cx,
        );
        lb._load = None;
        lb.video = Some(player);
        lb
    }

    /// The video's `currentTime`, for the host to resume from.
    pub fn video_position(&self, cx: &App) -> Option<f64> {
        self.video.as_ref().map(|v| v.read(cx).position())
    }

    pub fn src(&self) -> &str {
        &self.src
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_close)(window, cx);
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        let png = self.image.as_ref().map(|d| d.png.clone());
        match copy_png(png.as_ref(), cx) {
            Ok(()) => {
                self.copied = true;
                self.copy_timer = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(1400))
                        .await;
                    let _ = this.update(cx, |lb, cx| {
                        lb.copied = false;
                        cx.notify();
                    });
                }));
            }
            Err(_) => self.copied = false,
        }
        cx.notify();
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The context menu owns Escape while it is open.
        if self.menu.is_some() {
            return;
        }
        let k = &e.keystroke;
        if k.key == "escape" {
            cx.stop_propagation();
            self.close(window, cx);
            return;
        }
        let m = k.modifiers;
        let cmd = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.control
        };
        if cmd && !m.shift && !m.alt && k.key.eq_ignore_ascii_case("c") {
            cx.stop_propagation();
            self.copy(cx);
        }
    }

    fn open_menu(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let png = self.image.as_ref().map(|d| d.png.clone());
        let at = (f32::from(e.position.x), f32::from(e.position.y));
        let menu = cx.new(|cx| ImageContextMenu::new(at, png, window, cx));
        cx.guarded_subscribe_in(
            &menu,
            window,
            |lb, _, ev: &ImageMenuEvent, window, cx| match ev {
                ImageMenuEvent::Close => {
                    lb.menu = None;
                    window.focus(&lb.focus, cx);
                    cx.notify();
                }
                ImageMenuEvent::Toast { error, message } => cx.emit(LightboxEvent::Toast {
                    error: *error,
                    message: message.clone(),
                }),
            },
        )
        .detach();
        self.menu = Some(menu);
        cx.notify();
    }

    fn button(
        &self,
        id: &'static str,
        title: &'static str,
        glyph: gpui_kit::AnyElement,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let copied = id == "lb-copy" && self.copied;
        let b = div()
            .id(id)
            .size(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .border_1()
            .border_color(t.line_bright)
            .bg(t.pane)
            .text_color(if copied {
                crate::render::style::copied_green()
            } else {
                t.text
            })
            .cursor_pointer()
            .hover(move |s| s.bg(t.pane_2))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.guarded_listener(move |lb, _, window, cx| {
                cx.stop_propagation();
                on(lb, window, cx)
            }))
            .child(glyph);
        titled(id, title, b)
    }
}

impl AttachmentLightbox {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let natural = self
            .image
            .as_ref()
            .map_or((0., 0.), |d| (d.width, d.height));
        let ((fw, fh), scaled) = fit(natural, (vw - 64., vh - 64.));
        let (w, h) = if self.actual { natural } else { (fw, fh) };
        let img = self.image.as_ref().map(|d| d.image.clone());
        let picture = div()
            .id("lb-img")
            .relative()
            .flex_none()
            // The fitted side is whole; the other reaches layout unrounded through the ratio
            // (GPUI rounds authored lengths), so its edges snap where WebKit's do.
            .map(|d| {
                if h <= 0. {
                    d.w(px(w)).h(px(h))
                } else if (w * window.scale_factor()).fract() == 0. {
                    d.w(px(w)).aspect_ratio(w / h)
                } else {
                    d.h(px(h)).aspect_ratio(w / h)
                }
            })
            .child(box_shadow(
                &[css_box_shadow(0., 24., 80., -20., rgba(0, 0, 0, 0.85))],
                8.,
                0.,
            ))
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, _| {
                        window.paint_quad(
                            gpui_kit::fill(b, gpui_kit::Hsla::from(t.pane_2)).corner_radii(px(8.)),
                        );
                        if let Some(img) = img {
                            let _ = window.paint_image(b, b, Corners::all(px(8.)), img, 0, false);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.guarded_listener(move |lb, _, _, cx| {
                cx.stop_propagation();
                if scaled {
                    lb.actual = !lb.actual;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.guarded_listener(|lb, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    lb.open_menu(e, window, cx);
                }),
            );
        let scroll = div()
            .id("lb-scroll")
            .absolute()
            .inset_0()
            .flex()
            .p(px(32.))
            .overflow_scroll()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|lb, _, window, cx| lb.close(window, cx)),
            )
            // `margin: auto`, at WebKit's fractional offsets (it snaps the box's edges, not its
            // margins).
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .child(vgap(((vh - 64.) - h).max(0.) / 2., window))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .child(hgap(((vw - 64.) - w).max(0.) / 2., window))
                            .child(picture),
                    ),
            );
        let scroll = match &self.video {
            // `.chat-att-lb-img` on a `<video>`: fitted inside the 32px padding.
            Some(v) => div()
                .id("lb-scroll")
                .absolute()
                .inset_0()
                .p(px(32.))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.guarded_listener(|lb, _, window, cx| lb.close(window, cx)),
                )
                .child(v.clone()),
            None => scroll,
        };
        let copy_glyph = if self.copied {
            icon(IconName::Check)
                .stroke_width(2.4)
                .size(px(16.))
                .into_any_element()
        } else {
            icon(IconName::Copy).size(px(16.)).into_any_element()
        };
        let copy_title = if self.copied { "Copied" } else { "Copy image" };
        let actions = div()
            .absolute()
            .top(px(16.))
            .right(px(16.))
            .flex()
            .gap(px(8.))
            .when(self.video.is_none(), |d| {
                d.child(
                    self.button("lb-copy", copy_title, copy_glyph, cx, |lb, _, cx| {
                        lb.copy(cx)
                    }),
                )
            })
            .child(
                self.button(
                    "lb-close",
                    "Close",
                    icon(IconName::X)
                        .stroke_width(2.2)
                        .size(px(16.))
                        .into_any_element(),
                    cx,
                    |lb, window, cx| lb.close(window, cx),
                ),
            );
        let overlay = div()
            .id("chat-att-lb")
            .debug_selector(|| "chat-att-lb".into())
            .track_focus(&self.focus)
            .key_context("Lightbox")
            .capture_key_down(cx.guarded_listener(Self::key_down))
            .w(px(vw))
            .h(px(vh))
            .relative()
            .child(backdrop_blur(8., rgba(5, 7, 10, 0.72)))
            .child(scroll)
            .child(actions)
            .children(self.menu.clone());
        deferred(anchored_at(0., 0., overlay)).with_priority(220)
    }
}

impl Render for AttachmentLightbox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_without_upscaling() {
        let ((w, h), scaled) = fit((1200., 800.), (736., 496.));
        assert!(scaled && w == 736. && (h - 490.666).abs() < 0.01);
        assert_eq!(fit((100., 50.), (736., 496.)), ((100., 50.), false));
        assert_eq!(fit((0., 0.), (10., 10.)), ((0., 0.), false));
    }
}
