//! `<input type=color>` off macOS, as WebKitGTK shows it (`WebColorPickerGtk`): GTK's
//! `GtkColorChooserDialog`, "Select Color", in its own window. The GNOME palette (nine hues
//! of five shades), a Custom row whose `+` opens the editor (saturation / value plane, hue
//! slider, hex entry); every change reaches the page at once (`notify::rgba`), Cancel restores
//! the initial color and Select keeps the current one.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Corners, Entity, Hsla, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render,
    RenderImage, SharedString, StatefulInteractiveElement, Styled, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowOptions, canvas, div, px, size,
};

use crate::color::hex;
use crate::guard::Guarded as _;
use crate::text::{TextStyled, text};

/// `default_colors` in `gtkcolorchooserwidget.c`: columns of five shades.
pub const PALETTE: [[u32; 5]; 9] = [
    [0x99c1f1, 0x62a0ea, 0x3584e4, 0x1c71d8, 0x1a5fb4],
    [0x8ff0a4, 0x57e389, 0x33d17a, 0x2ec27e, 0x26a269],
    [0xf9f06b, 0xf8e45c, 0xf6d32d, 0xf5c211, 0xe5a50a],
    [0xffbe6f, 0xffa348, 0xff7800, 0xe66100, 0xc64600],
    [0xf66151, 0xed333b, 0xe01b24, 0xc01c28, 0xa51d2d],
    [0xdc8add, 0xc061cb, 0x9141ac, 0x813d9c, 0x613583],
    [0xcdab8f, 0xb5835a, 0x986a44, 0x865e3c, 0x63452c],
    [0xffffff, 0xf6f5f4, 0xdeddda, 0xc0bfbc, 0x9a9996],
    [0x77767b, 0x5e5c64, 0x3d3846, 0x241f31, 0x000000],
];

const SWATCH: f32 = 32.;
const PLANE: (f32, f32) = (220., 160.);
const HUE_W: f32 = 24.;
const ACCENT: u32 = 0x3584e4;

/// `#rrggbb` → 0xrrggbb.
pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().strip_prefix('#')?;
    match h.len() {
        6 => u32::from_str_radix(h, 16).ok(),
        3 => {
            let v = u32::from_str_radix(h, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some(((r * 17) << 16) | ((g * 17) << 8) | (b * 17))
        }
        _ => None,
    }
}

pub fn to_hex(rgb: u32) -> String {
    format!("#{rgb:06x}")
}

/// HSV (h in 0..1) → 0xrrggbb.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> u32 {
    let h6 = (h.rem_euclid(1.)) * 6.;
    let i = h6.floor() as i32 % 6;
    let f = h6 - h6.floor();
    let (p, q, t) = (v * (1. - s), v * (1. - s * f), v * (1. - s * (1. - f)));
    let (r, g, b) = match i {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    let c = |x: f32| (x.clamp(0., 1.) * 255.).round() as u32;
    (c(r) << 16) | (c(g) << 8) | c(b)
}

/// 0xrrggbb → HSV, h in 0..1.
pub fn rgb_to_hsv(rgb: u32) -> (f32, f32, f32) {
    let (r, g, b) = (
        ((rgb >> 16) & 0xff) as f32 / 255.,
        ((rgb >> 8) & 0xff) as f32 / 255.,
        (rgb & 0xff) as f32 / 255.,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0. {
        0.
    } else if max == r {
        ((g - b) / d).rem_euclid(6.) / 6.
    } else if max == g {
        ((b - r) / d + 2.) / 6.
    } else {
        ((r - g) / d + 4.) / 6.
    };
    let s = if max == 0. { 0. } else { d / max };
    (h, s, max)
}

pub type OnColor = Rc<dyn Fn(String, &mut App)>;

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Plane,
    Hue,
}

pub struct ColorChooser {
    initial: u32,
    current: u32,
    hsv: (f32, f32, f32),
    editing: bool,
    custom: Vec<u32>,
    dark: bool,
    entry: Entity<InputState>,
    on_color: OnColor,
    plane: Option<Bounds<Pixels>>,
    hue: Option<Bounds<Pixels>>,
    drag: Option<Drag>,
    done: bool,
    _subs: Vec<Subscription>,
}

/// Opens the chooser at `color` (`#rrggbb`) in its own window; `on_color` gets every change.
pub fn open(color: &str, dark: bool, on_color: OnColor, cx: &mut App) {
    let initial = parse_hex(color).unwrap_or(0);
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Select Color".into()),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(372.), px(342.)), cx)),
        is_resizable: false,
        ..Default::default()
    };
    let _ = cx.open_window(options, move |window, cx| {
        let chooser = cx.new(|cx| ColorChooser::new(initial, dark, on_color, window, cx));
        cx.new(|cx| gpui_kit::base::Root::new(chooser, window, cx))
    });
}

impl ColorChooser {
    pub fn new(
        initial: u32,
        dark: bool,
        on_color: OnColor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entry = cx.new(|cx| InputState::new(window, cx));
        entry.update(cx, |e, cx| e.set_value(to_hex(initial), window, cx));
        let subs =
            vec![
                cx.guarded_subscribe_in(&entry, window, |this, e, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur)
                        && let Some(c) = parse_hex(&e.read(cx).value())
                    {
                        this.set(c, true, cx);
                    }
                }),
            ];
        // Closing the window is Cancel (`GTK_RESPONSE_DELETE_EVENT`).
        window.on_window_should_close(cx, {
            let this = cx.entity().downgrade();
            move |_, cx| {
                let _ = this.update(cx, |c, cx| c.finish(false, cx));
                true
            }
        });
        Self {
            initial,
            current: initial,
            hsv: rgb_to_hsv(initial),
            editing: false,
            custom: Vec::new(),
            dark,
            entry,
            on_color,
            plane: None,
            hue: None,
            drag: None,
            done: false,
            _subs: subs,
        }
    }

    pub fn current(&self) -> String {
        to_hex(self.current)
    }

    /// A new color: shown, and sent to the page.
    fn set(&mut self, rgb: u32, from_hsv: bool, cx: &mut Context<Self>) {
        if !from_hsv {
            self.hsv = rgb_to_hsv(rgb);
        }
        if rgb != self.current {
            self.current = rgb;
            (self.on_color)(to_hex(rgb), cx);
        }
        cx.notify();
    }

    fn set_hsv(&mut self, h: f32, s: f32, v: f32, cx: &mut Context<Self>) {
        self.hsv = (h, s, v);
        let rgb = hsv_to_rgb(h, s, v);
        self.set(rgb, true, cx);
    }

    /// The dialog's response: Select keeps the color, Cancel puts the initial one back.
    fn finish(&mut self, select: bool, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        self.done = true;
        if !select && self.current != self.initial {
            (self.on_color)(to_hex(self.initial), cx);
        }
    }

    fn drag_to(&mut self, p: gpui_kit::Point<Pixels>, cx: &mut Context<Self>) {
        let frac = |b: Bounds<Pixels>| {
            (
                ((f32::from(p.x) - f32::from(b.left())) / f32::from(b.size.width)).clamp(0., 1.),
                ((f32::from(p.y) - f32::from(b.top())) / f32::from(b.size.height)).clamp(0., 1.),
            )
        };
        let (h, s, v) = self.hsv;
        match self.drag {
            Some(Drag::Plane) => {
                if let Some(b) = self.plane {
                    let (x, y) = frac(b);
                    self.set_hsv(h, x, 1. - y, cx);
                }
            }
            Some(Drag::Hue) => {
                if let Some(b) = self.hue {
                    let (_, y) = frac(b);
                    self.set_hsv(y.min(0.999), s, v, cx);
                }
            }
            None => {}
        }
    }

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let (bg, fg, pane) = if self.dark {
            (hex(0x242424), hex(0xffffff), hex(0x303030))
        } else {
            (hex(0xfafafa), hex(0x000000).with_alpha(0.8), hex(0xebebeb))
        };
        let current = self.current;
        let swatch = |id: String, rgb: u32| {
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .size(px(SWATCH))
                .flex()
                .items_center()
                .justify_center()
                .bg(Hsla::from(hex(rgb)))
                .cursor_pointer()
                .when(rgb == current, |d| {
                    let (_, _, v) = rgb_to_hsv(rgb);
                    let mark = if v > 0.7 {
                        hex(0x000000)
                    } else {
                        hex(0xffffff)
                    };
                    d.child(text("✓").size(14.).color(mark))
                })
        };
        let body = if self.editing {
            self.editor(fg, pane, window, cx)
        } else {
            let mut grid = div().flex().gap(px(6.));
            for (c, col) in PALETTE.iter().enumerate() {
                let mut column = div().flex().flex_col().rounded(px(6.)).overflow_hidden();
                for (r, &rgb) in col.iter().enumerate() {
                    column = column.child(swatch(format!("swatch-{c}-{r}"), rgb).on_click(
                        cx.guarded_listener(move |this, _, _, cx| this.set(rgb, false, cx)),
                    ));
                }
                grid = grid.child(column);
            }
            let mut custom = div().flex().gap(px(6.)).child(
                div()
                    .id("custom-add")
                    .debug_selector(|| "custom-add".into())
                    .size(px(SWATCH))
                    .rounded(px(6.))
                    .bg(Hsla::from(pane))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(text("+").size(16.).color(fg))
                    .on_click(cx.guarded_listener(|this, _, window, cx| {
                        this.editing = true;
                        if !this.custom.contains(&this.current) {
                            this.custom.insert(0, this.current);
                            this.custom.truncate(8);
                        }
                        let hexv = to_hex(this.current);
                        this.entry.update(cx, |e, cx| e.set_value(hexv, window, cx));
                        cx.notify();
                    })),
            );
            for (i, &rgb) in self.custom.iter().enumerate() {
                custom =
                    custom.child(swatch(format!("custom-{i}"), rgb).rounded(px(6.)).on_click(
                        cx.guarded_listener(move |this, _, _, cx| this.set(rgb, false, cx)),
                    ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(grid)
                .child(text("Custom").size(13.).color(fg))
                .child(custom)
                .into_any_element()
        };
        let button = |id: &'static str, label: &'static str, suggested: bool| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .px(px(14.))
                .py(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
                .bg(Hsla::from(if suggested { hex(ACCENT) } else { pane }))
                .child(
                    text(label)
                        .size(13.)
                        .color(if suggested { hex(0xffffff) } else { fg }),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(Hsla::from(bg))
            .p(px(18.))
            .gap(px(14.))
            .on_mouse_move(cx.guarded_listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() && e.pressed_button == Some(MouseButton::Left) {
                    this.drag_to(e.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.guarded_listener(|this, _: &MouseUpEvent, _, _| this.drag = None),
            )
            .child(div().flex_1().child(body))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        button("cc-cancel", "Cancel", false).on_click(cx.guarded_listener(
                            |this, _, window, cx| {
                                this.finish(false, cx);
                                window.remove_window();
                            },
                        )),
                    )
                    .child(
                        button("cc-select", "Select", true).on_click(cx.guarded_listener(
                            |this, _, window, cx| {
                                this.finish(true, cx);
                                window.remove_window();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn editor(
        &mut self,
        fg: crate::color::Color,
        pane: crate::color::Color,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let (h, s, v) = self.hsv;
        let scale = window.scale_factor();
        let (pw, ph) = (
            (PLANE.0 * scale).round() as u32,
            (PLANE.1 * scale).round() as u32,
        );
        let plane_img = image(pw, ph, |x, y| {
            hsv_to_rgb(h, x as f32 / pw as f32, 1. - y as f32 / ph as f32)
        });
        let hue_img = image((HUE_W * scale) as u32, ph, |_, y| {
            hsv_to_rgb(y as f32 / ph as f32, 1., 1.)
        });
        let this = cx.entity().downgrade();
        let this2 = this.clone();
        let marker = |x: f32, y: f32| {
            div()
                .absolute()
                .left(px(x - 6.))
                .top(px(y - 6.))
                .size(px(12.))
                .rounded_full()
                .border(px(2.))
                .border_color(Hsla::from(hex(0xffffff)))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(
                        div()
                            .id("cc-hue")
                            .debug_selector(|| "cc-hue".into())
                            .relative()
                            .w(px(HUE_W))
                            .h(px(PLANE.1))
                            .child(paint_image(hue_img))
                            .child(
                                canvas(
                                    move |b, _, cx| {
                                        let _ = this.update(cx, |c, _| c.hue = Some(b));
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(-2.))
                                    .top(px(h * PLANE.1 - 2.))
                                    .w(px(HUE_W + 4.))
                                    .h(px(4.))
                                    .border(px(1.))
                                    .border_color(Hsla::from(fg)),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.guarded_listener(|this, e: &MouseDownEvent, _, cx| {
                                    this.drag = Some(Drag::Hue);
                                    this.drag_to(e.position, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("cc-plane")
                            .debug_selector(|| "cc-plane".into())
                            .relative()
                            .w(px(PLANE.0))
                            .h(px(PLANE.1))
                            .child(paint_image(plane_img))
                            .child(
                                canvas(
                                    move |b, _, cx| {
                                        let _ = this2.update(cx, |c, _| c.plane = Some(b));
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(marker(s * PLANE.0, (1. - v) * PLANE.1))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.guarded_listener(|this, e: &MouseDownEvent, _, cx| {
                                    this.drag = Some(Drag::Plane);
                                    this.drag_to(e.position, cx);
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .size(px(SWATCH))
                            .rounded(px(6.))
                            .bg(Hsla::from(hex(self.current))),
                    )
                    .child(
                        div()
                            .debug_selector(|| "cc-entry".into())
                            .w(px(120.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .bg(Hsla::from(pane))
                            .child(gpui_kit::base::input::Input::new(&self.entry)),
                    ),
            )
            .into_any_element()
    }
}

/// Device-pixel RGBA for `f(x, y)` as a GPUI image (BGRA frames).
fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> u32) -> Arc<RenderImage> {
    let mut img = image::RgbaImage::new(w.max(1), h.max(1));
    for y in 0..h {
        for x in 0..w {
            let c = f(x, y);
            img.put_pixel(
                x,
                y,
                image::Rgba([
                    (c & 0xff) as u8,
                    ((c >> 8) & 0xff) as u8,
                    (c >> 16) as u8,
                    255,
                ]),
            );
        }
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(img)]))
}

fn paint_image(img: Arc<RenderImage>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let _ = window.paint_image(b, b, Corners::all(px(4.)), img.clone(), 0, false);
        },
    )
    .absolute()
    .size_full()
    .rounded(px(4.))
}

impl Render for ColorChooser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_hsv_round_trip() {
        assert_eq!(parse_hex("#3584e4"), Some(0x3584e4));
        assert_eq!(parse_hex("#abc"), Some(0xaabbcc));
        assert_eq!(parse_hex("3584e4"), None);
        for c in PALETTE.iter().flatten() {
            let (h, s, v) = rgb_to_hsv(*c);
            assert_eq!(hsv_to_rgb(h, s, v), *c, "{}", to_hex(*c));
        }
        assert_eq!(hsv_to_rgb(0., 1., 1.), 0xff0000);
        assert_eq!(hsv_to_rgb(1. / 3., 1., 1.), 0x00ff00);
    }
}
