//! Inline chips: the user bubble's `.chat-file-inline` family (file, folder, skill, terminal,
//! artifact, image) and the composer's `.chat-composer-chip` family (the same kinds with a ×,
//! the image thumbnail and the prompt chip with its slot inputs). Each comes with its
//! margin-box size for [`crate::flow`].

use recur_ui::guard::Guarded;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Bounds, ContentMask, Context, Corners, InteractiveElement,
    IntoElement, ParentElement, Pixels, RenderImage, SharedString, StatefulInteractiveElement,
    Styled, Window, canvas, div, point, px, size,
};
use recur_content::images::{self, Decoded, ImageCache};
use recur_engine::view::composer::Segment;
use recur_engine::view::mentions::{
    artifact_mention_label, file_mention_label, parse_file_mention_chip, terminal_mention_label,
};
use recur_ui::file_icons::file_icon;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Color, IconName, Theme, icon, rgba};

use crate::chat::AgentChat;
use crate::flow::Atom;
use crate::style::{amber_a, hsla};

/// How an image fills its box.
#[derive(Clone, Copy, PartialEq)]
pub enum Fit {
    /// The box already has the image's aspect ratio.
    Fill,
    /// `object-fit: cover`: scale to cover, center, clip.
    Cover,
}

/// Paints `image` (natural `nw`×`nh`) into the element's box with rounded corners.
pub fn image_box(
    image: Arc<RenderImage>,
    nw: f32,
    nh: f32,
    fit: Fit,
    radius: f32,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b: Bounds<Pixels>, _, window, _| {
            let target = match fit {
                Fit::Fill => b,
                Fit::Cover => {
                    let (bw, bh) = (f32::from(b.size.width), f32::from(b.size.height));
                    let k = (bw / nw.max(1.)).max(bh / nh.max(1.));
                    let (w, h) = (nw * k, nh * k);
                    Bounds::new(
                        b.origin + point(px((bw - w) / 2.), px((bh - h) / 2.)),
                        size(px(w), px(h)),
                    )
                }
            };
            window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
                let _ = window.paint_image(
                    target,
                    b,
                    Corners::all(px(radius)),
                    image.clone(),
                    0,
                    false,
                );
            });
        },
    )
    .size_full()
}

/// CSS `max-width` / `max-height` on an auto-sized replaced element: the natural size scaled
/// down (aspect kept) to fit both.
pub fn fit_max(nw: f32, nh: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    if nw <= 0. || nh <= 0. {
        return (0., 0.);
    }
    let k = (max_w / nw).min(max_h / nh).min(1.);
    (nw * k, nh * k)
}

fn label_w(s: &str, window: &Window) -> f32 {
    text(s.to_string())
        .size(11.5)
        .line_height(LineHeight::Scale(1.))
        .nowrap()
        .measure(None, window)
        .0
}

fn chip_name(s: impl Into<SharedString>, color: Color) -> AnyElement {
    div()
        .min_w_0()
        .overflow_hidden()
        .child(
            text(s)
                .size(11.5)
                .line_height(LineHeight::Scale(1.))
                .color(color)
                .truncate(),
        )
        .into_any_element()
}

fn ico14(name: IconName, color: Color) -> AnyElement {
    div()
        .flex_none()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(name).size(px(14.)).color(hsla(color)))
        .into_any_element()
}

fn fico(path: &str) -> AnyElement {
    let base = path.rsplit('/').next().unwrap_or(path).to_string();
    div()
        .flex_none()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .child(file_icon(base))
        .into_any_element()
}

impl AgentChat {
    /// A decoded image for `src`, loading it once (the webview's image cache).
    pub(crate) fn image_for(&mut self, src: &str, cx: &mut Context<Self>) -> Option<Decoded> {
        if let Some(d) = cx.try_global::<ImageCache>().and_then(|c| c.0.get(src)) {
            return Some(d.clone());
        }
        if self.image_loading.contains_key(src) {
            return None;
        }
        let fut = images::fetch(src, cx)?;
        let key = src.to_string();
        let task = cx.spawn(async move |this, cx| {
            let res = cx.background_spawn(fut).await;
            let _ = this.update(cx, |c, cx| {
                c.image_loading.remove(&key);
                if let Ok(d) = res {
                    cx.default_global::<ImageCache>().0.insert(key.clone(), d);
                }
                cx.notify();
            });
        });
        self.image_loading.insert(src.to_string(), task);
        None
    }

    // ---- bubble chips (`.chat-file-inline`) ----

    /// The shell of `.chat-file-inline` / `.chat-skill-inline`: 22px, `margin: 2px 2px 2px 0`,
    /// `padding: 0 8px 0 6px`, `gap: 5px`, a 1px border.
    #[allow(clippy::too_many_arguments)]
    fn bubble_chip(
        &self,
        id: SharedString,
        lead: AnyElement,
        name: String,
        border: Color,
        bg: Color,
        hover: Option<(Color, Color)>,
        max_w: Option<f32>,
        faded: Option<Color>,
        tip: String,
        window: &Window,
        cx: &App,
    ) -> (gpui_kit::Stateful<gpui_kit::Div>, f32) {
        let t = Theme::of(cx).tokens;
        let natural = 1. + 6. + 14. + 5. + label_w(&name, window) + 8. + 1.;
        let w = max_w.map_or(natural, |m| natural.min(m));
        let (border, bg, fg) = match faded {
            Some(backdrop) => (
                recur_ui::style::faded(border, bg, 0.9, backdrop),
                recur_ui::style::faded(bg, recur_ui::color::TRANSPARENT, 0.9, backdrop),
                recur_ui::style::faded(t.text, bg, 0.9, backdrop),
            ),
            None => (border, bg, t.text),
        };
        let chip = div()
            .id(id.clone())
            .flex()
            .items_center()
            .gap(px(5.))
            .h(px(22.))
            // The parts add up to the natural width exactly; an authored width would round.
            .when(w < natural, |d| d.w(px(w)))
            .pl(px(6.))
            .pr(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(border))
            .bg(hsla(bg))
            .when_some(hover, |d, (hb, hbg)| {
                d.hover(move |s| s.border_color(hsla(hb)).bg(hsla(hbg)))
            })
            .child(lead)
            .child(chip_name(name, fg));
        let chip = if tip.is_empty() {
            chip
        } else {
            div()
                .id(SharedString::from(format!("{id}-w")))
                .child(recur_ui::tooltip::titled(
                    SharedString::from(format!("{id}-tip")),
                    tip,
                    chip,
                ))
        };
        (chip, w + 2.)
    }

    /// One bubble part as a flow atom (`bubbleParts` → markup).
    pub(crate) fn bubble_atom(
        &mut self,
        part: &crate::blocks::BubblePart,
        id: SharedString,
        backdrop: Color,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Atom> {
        use crate::blocks::BubblePart;
        let t = Theme::of(cx).tokens;
        let amber = (amber_a(0.28), amber_a(0.08));
        let amber_hover = Some((amber_a(0.45), amber_a(0.14)));
        let (el, w, h, copy): (AnyElement, f32, f32, String) = match part {
            BubblePart::Text(_) => return None,
            BubblePart::File { path, dir: true } => {
                let (chip, w) = self.bubble_chip(
                    id,
                    ico14(IconName::Folder, t.muted),
                    file_mention_label(path),
                    amber.0,
                    amber.1,
                    None,
                    None,
                    None,
                    path.clone(),
                    window,
                    cx,
                );
                (
                    chip.debug_selector(|| format!("file-inline:{}", file_mention_label(path)))
                        .into_any_element(),
                    w,
                    26.,
                    file_mention_label(path),
                )
            }
            BubblePart::File { path, .. } => {
                let enabled = self.hooks.open_file;
                let stem = parse_file_mention_chip(path);
                let icon_for = if stem.path.is_empty() {
                    path.clone()
                } else {
                    stem.path
                };
                let (chip, w) = self.bubble_chip(
                    id,
                    fico(&icon_for),
                    file_mention_label(path),
                    amber.0,
                    amber.1,
                    enabled.then_some(amber_hover).flatten(),
                    None,
                    (!enabled).then_some(backdrop),
                    path.clone(),
                    window,
                    cx,
                );
                let p = path.clone();
                let chip =
                    chip.debug_selector(|| format!("file-inline:{}", file_mention_label(path)));
                let el = if enabled {
                    chip.cursor_pointer()
                        .on_click(
                            cx.guarded_listener(move |this, _, _, cx| this.open_file_chip(&p, cx)),
                        )
                        .into_any_element()
                } else {
                    chip.into_any_element()
                };
                (el, w, 26., file_mention_label(path))
            }
            BubblePart::Skill(name) => {
                let (chip, w) = self.bubble_chip(
                    id,
                    ico14(IconName::Slash, t.amber),
                    name.clone(),
                    t.line,
                    amber_a(0.06),
                    None,
                    None,
                    None,
                    format!("/{name}"),
                    window,
                    cx,
                );
                (chip.into_any_element(), w, 26., name.clone())
            }
            BubblePart::Terminal(tid) => {
                let title = self.terminal_title_for(tid, cx);
                let chip_wire = if tid.is_empty() {
                    "terminal".to_string()
                } else {
                    format!("terminal:{tid}")
                };
                let tip = if title.is_empty() {
                    "Terminal".to_string()
                } else {
                    title.clone()
                };
                let label = terminal_mention_label(&chip_wire, &title);
                let (chip, w) = self.bubble_chip(
                    id,
                    ico14(IconName::Terminal, t.muted),
                    label.clone(),
                    amber.0,
                    amber.1,
                    amber_hover,
                    Some(220.),
                    None,
                    tip,
                    window,
                    cx,
                );
                let tid = tid.clone();
                (
                    chip.cursor_pointer()
                        .on_click(cx.guarded_listener(move |this, _, _, cx| {
                            this.open_terminal_chip(&tid, cx)
                        }))
                        .into_any_element(),
                    w,
                    26.,
                    label,
                )
            }
            BubblePart::Artifact(aid) => {
                let title = self.artifact_title_for(aid, cx);
                let kind = self
                    .live_artifacts(cx)
                    .into_iter()
                    .find(|a| &a.id == aid)
                    .map(|a| a.kind)
                    .unwrap_or_default();
                let icon_name =
                    IconName::from_key(recur_engine::view::artifacts::artifact_icon(&kind))
                        .unwrap_or(IconName::Grid);
                let chip_wire = if aid.is_empty() {
                    "artifact".to_string()
                } else {
                    format!("artifact:{aid}")
                };
                let tip = if title.is_empty() {
                    "Artifact".to_string()
                } else {
                    title.clone()
                };
                let label = artifact_mention_label(&chip_wire, &title);
                let (chip, w) = self.bubble_chip(
                    id,
                    ico14(icon_name, t.muted),
                    label.clone(),
                    amber.0,
                    amber.1,
                    amber_hover,
                    Some(220.),
                    None,
                    tip,
                    window,
                    cx,
                );
                let aid = aid.clone();
                (
                    chip.cursor_pointer()
                        .on_click(cx.guarded_listener(move |this, _, _, cx| {
                            this.open_artifact_chip(&aid, cx)
                        }))
                        .into_any_element(),
                    w,
                    26.,
                    label,
                )
            }
            BubblePart::Image { src, name, .. } => {
                let d = self.image_for(src, cx);
                let (w, h) = d
                    .as_ref()
                    .map_or((0., 0.), |d| fit_max(d.width, d.height, 280., 200.));
                let (bw, bh) = if d.is_some() {
                    (w + 2., h + 2.)
                } else {
                    (0., 0.)
                };
                let (s, n) = (src.clone(), name.clone());
                let s2 = src.clone();
                let el = div()
                    .id(id)
                    .w(px(bw))
                    .h(px(bh))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(hsla(t.line))
                    .bg(hsla(t.pane_2))
                    .cursor(gpui_kit::CursorStyle::PointingHand)
                    .when_some(d, |el, d| {
                        el.child(image_box(d.image.clone(), d.width, d.height, Fit::Fill, 5.))
                    })
                    .on_click(cx.guarded_listener(move |this, _, window, cx| {
                        this.open_lightbox(&s, &n, window, cx)
                    }))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Right,
                        cx.guarded_listener(
                            move |this, ev: &gpui_kit::MouseDownEvent, window, cx| {
                                this.open_image_menu(&s2, ev.position, window, cx);
                            },
                        ),
                    )
                    .into_any_element();
                (el, bw, bh, String::new())
            }
        };
        Some(Atom {
            el,
            width: w,
            height: h,
            copy,
        })
    }

    // ---- composer chips (`.chat-composer-chip`) ----

    /// The × (`.chat-composer-chip-x`).
    fn chip_x(&self, id: SharedString, i: usize, att: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let base = div()
            .id(id)
            .debug_selector(|| format!("chip-x:{i}"))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation()
            })
            .on_click(cx.guarded_listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.remove_chip(i, window, cx);
            }));
        if att {
            base.absolute()
                .top(px(-5.))
                .right(px(-5.))
                .size(px(16.))
                .rounded(px(6.))
                .bg(hsla(rgba(0, 0, 0, 0.65)))
                .hover(|s| s.bg(hsla(rgba(0, 0, 0, 0.85))))
                .child(
                    text("×")
                        .size(12.)
                        .line_height(LineHeight::Scale(1.))
                        .color(recur_ui::hex(0xffffff))
                        .nowrap(),
                )
                .into_any_element()
        } else {
            base.ml(px(1.))
                .size(px(14.))
                .rounded(px(4.))
                .text_color(hsla(t.muted))
                .hover(|s| {
                    s.bg(hsla(rgba(255, 255, 255, 0.08)))
                        .text_color(hsla(t.text))
                })
                .child(
                    text("×")
                        .size(12.)
                        .line_height(LineHeight::Scale(1.))
                        .nowrap(),
                )
                .into_any_element()
        }
    }

    /// Chip `i` of the composer doc as a flow atom.
    pub(crate) fn composer_atom(
        &mut self,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Atom> {
        let t = Theme::of(cx).tokens;
        let chip = self.doc.chips.get(i)?.clone();
        let id = SharedString::from(format!("cchip-{}-{i}", self.props.reset_key));
        let x = self.chip_x(
            SharedString::from(format!("{id}-x")),
            i,
            matches!(chip, Segment::Attachment(_)),
            cx,
        );
        // `.chat-composer-chip`: 22px, margin 0 2px, padding 0 6px 0 5px, gap 5px, max 220px.
        let shell = |lead: AnyElement,
                     name: String,
                     border: Color,
                     bg: Color,
                     hover: bool,
                     tip: String,
                     window: &Window| {
            let natural = 1. + 5. + 14. + 5. + label_w(&name, window) + 5. + 1. + 14. + 6. + 1.;
            let w = natural.min(220.);
            let el = div()
                .id(id.clone())
                .debug_selector(|| format!("chip:{name}"))
                .relative()
                .flex()
                .items_center()
                .gap(px(5.))
                .h(px(22.))
                .when(w < natural, |d| d.w(px(w)))
                .pl(px(5.))
                .pr(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(hsla(border))
                .bg(hsla(bg))
                .when(hover, |d| {
                    d.hover(|s| s.border_color(hsla(t.line_bright)).bg(hsla(t.pane)))
                })
                .child(lead)
                .child(div().flex_1().min_w_0().child(chip_name(name, t.text)));
            (el, w, tip)
        };
        let (el, w, h): (AnyElement, f32, f32) = match &chip {
            Segment::File { path, dir } => {
                let parsed = parse_file_mention_chip(path);
                let lead = if *dir {
                    div()
                        .flex_none()
                        .flex()
                        .debug_selector(|| "chip-folder".into())
                        .child(ico14(IconName::Folder, t.muted))
                        .into_any_element()
                } else {
                    fico(if parsed.path.is_empty() {
                        path
                    } else {
                        &parsed.path
                    })
                };
                let tip = if *dir {
                    parsed.path.clone()
                } else {
                    format!("Open {path}")
                };
                let (el, w, tip) = shell(
                    lead,
                    file_mention_label(path),
                    t.line,
                    t.pane_2,
                    true,
                    tip,
                    window,
                );
                let el = el.child(x).cursor(if *dir {
                    gpui_kit::CursorStyle::Arrow
                } else {
                    gpui_kit::CursorStyle::PointingHand
                });
                let el =
                    el.on_click(cx.guarded_listener(move |this, _, window, cx| {
                        this.click_chip(i, window, cx)
                    }));
                (
                    recur_ui::tooltip::titled(SharedString::from(format!("{id}-tip")), tip, el)
                        .into_any_element(),
                    w + 4.,
                    22.,
                )
            }
            Segment::Skill(name) => {
                let (el, w, tip) = shell(
                    ico14(IconName::Slash, t.amber),
                    name.clone(),
                    t.line,
                    amber_a(0.06),
                    false,
                    format!("/{name}"),
                    window,
                );
                (
                    recur_ui::tooltip::titled(
                        SharedString::from(format!("{id}-tip")),
                        tip,
                        el.child(x),
                    )
                    .into_any_element(),
                    w + 4.,
                    22.,
                )
            }
            Segment::Terminal { id: tid, title } => {
                let chip_wire = if tid.is_empty() {
                    "terminal".to_string()
                } else {
                    format!("terminal:{tid}")
                };
                let tip = if title.is_empty() {
                    "Terminal".into()
                } else {
                    format!("{title} terminal")
                };
                let (el, w, tip) = shell(
                    ico14(IconName::Terminal, t.muted),
                    terminal_mention_label(&chip_wire, title),
                    t.line,
                    t.pane_2,
                    true,
                    tip,
                    window,
                );
                let el = el.child(x).cursor_pointer().on_click(
                    cx.guarded_listener(move |this, _, window, cx| this.click_chip(i, window, cx)),
                );
                (
                    recur_ui::tooltip::titled(SharedString::from(format!("{id}-tip")), tip, el)
                        .into_any_element(),
                    w + 4.,
                    22.,
                )
            }
            Segment::Artifact { id: aid, title } => {
                let kind = self
                    .live_artifacts(cx)
                    .into_iter()
                    .find(|a| &a.id == aid)
                    .map(|a| a.kind)
                    .unwrap_or_default();
                let icon_name =
                    IconName::from_key(recur_engine::view::artifacts::artifact_icon(&kind))
                        .unwrap_or(IconName::Grid);
                let chip_wire = if aid.is_empty() {
                    "artifact".to_string()
                } else {
                    format!("artifact:{aid}")
                };
                let tip = if title.is_empty() {
                    "Artifact".into()
                } else {
                    format!("{title} artifact")
                };
                let (el, w, tip) = shell(
                    ico14(icon_name, t.muted),
                    artifact_mention_label(&chip_wire, title),
                    t.line,
                    t.pane_2,
                    true,
                    tip,
                    window,
                );
                let el = el.child(x).cursor_pointer().on_click(
                    cx.guarded_listener(move |this, _, window, cx| this.click_chip(i, window, cx)),
                );
                (
                    recur_ui::tooltip::titled(SharedString::from(format!("{id}-tip")), tip, el)
                        .into_any_element(),
                    w + 4.,
                    22.,
                )
            }
            Segment::Attachment(a) => {
                let d = self.image_for(&a.data_url, cx);
                let label = if a.name.is_empty() {
                    "image".to_string()
                } else {
                    a.name.clone()
                };
                // `.chat-composer-att`: 32px tall, 2px padding, a 28px cover thumbnail.
                let el = div()
                    .id(id.clone())
                    .debug_selector(|| format!("chip-att:{i}"))
                    .relative()
                    .flex()
                    .items_center()
                    .h(px(32.))
                    .w(px(34.))
                    .p(px(2.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(hsla(t.line))
                    .bg(hsla(t.pane_2))
                    .hover(|s| s.border_color(hsla(t.line_bright)).bg(hsla(t.pane)))
                    .cursor(gpui_kit::CursorStyle::PointingHand)
                    .child(div().size(px(28.)).rounded(px(4.)).when_some(d, |el, d| {
                        el.child(image_box(
                            d.image.clone(),
                            d.width,
                            d.height,
                            Fit::Cover,
                            4.,
                        ))
                    }))
                    .child(x)
                    .on_click(cx.guarded_listener(move |this, _, window, cx| {
                        this.click_chip(i, window, cx)
                    }));
                (
                    recur_ui::tooltip::titled(
                        SharedString::from(format!("{id}-tip")),
                        format!("View {label}"),
                        el,
                    )
                    .into_any_element(),
                    34. + 4.,
                    32.,
                )
            }
            Segment::Prompt { spec, .. } => {
                let (el, w) = self.prompt_chip(i, &spec.slash, &spec.args, x, window, cx);
                (el, w + 4., 22.)
            }
            Segment::Text(_) => return None,
        };
        // `margin: 0 2px`: the atom box is the margin box.
        Some(Atom {
            el: div().flex().px(px(2.)).child(el).into_any_element(),
            width: w,
            height: h,
            copy: String::new(),
        })
    }

    /// `.chat-composer-prompt`: the cyan `/slash` and one labeled input per arg.
    fn prompt_chip(
        &mut self,
        i: usize,
        slash: &str,
        args: &[recur_bundles::PromptArgSpec],
        x: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, f32) {
        let t = Theme::of(cx).tokens;
        let tint = |p: f32, other: Color| t.cyan.mix(p, other);
        let focused = self.focused_slot(window, cx);
        let border = if focused.is_some() {
            tint(0.5, t.line)
        } else {
            tint(0.22, t.line)
        };
        let names: Vec<String> = if args.is_empty() {
            vec!["args".into()]
        } else {
            args.iter().map(|a| a.name.clone()).collect()
        };
        let cmd = format!("/{slash}");
        let cmd_w = text(cmd.clone())
            .size(11.5)
            .weight(650.)
            .tracking(0.01)
            .line_height(LineHeight::Scale(1.))
            .nowrap()
            .measure(None, window)
            .0;
        let ch_ui = text("0").size(11.5).measure(None, window).0;
        let mut width = 1. + 8. + cmd_w + 1.;
        let mut slots = Vec::new();
        for (si, name) in names.iter().enumerate() {
            let Some(input) = self.prompt_slots.get(si).cloned() else {
                continue;
            };
            let on = focused == Some(si);
            let value = input.read(cx).value().to_string();
            let placeholder = args
                .get(si)
                .map(|a| a.placeholder.clone())
                .unwrap_or_default();
            let n =
                (value.chars().count().max(placeholder.chars().count()) + 1).clamp(8, 42) as f32;
            let iw = n * ch_ui;
            let lab_w = text(name.clone())
                .size(10.)
                .weight(650.)
                .tracking(0.02)
                .line_height(LineHeight::Scale(1.))
                .nowrap()
                .measure(None, window)
                .0;
            width += 6. + lab_w + 6. + 1. + 6. + iw;
            let ph = SharedString::from(placeholder);
            slots.push(
                div()
                    .id(SharedString::from(format!("pslot-{i}-{si}")))
                    .flex()
                    .items_center()
                    .min_h(px(16.))
                    .cursor(gpui_kit::CursorStyle::IBeam)
                    .child(
                        div()
                            .flex_none()
                            .mr(px(6.))
                            .pr(px(6.))
                            .border_r_1()
                            .border_color(hsla(tint(0.22, t.line)))
                            .child(
                                text(name.clone())
                                    .size(10.)
                                    .weight(650.)
                                    .tracking(0.02)
                                    .line_height(LineHeight::Scale(1.))
                                    .color(if on { t.cyan } else { t.muted })
                                    .nowrap(),
                            ),
                    )
                    .child(
                        div()
                            .w(px(iw))
                            .h(px(16.))
                            .child(recur_ui::style::text_field(
                                &input,
                                &ph,
                                t.faint,
                                Family::Ui,
                                11.5,
                                window,
                                cx,
                            )),
                    )
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.guarded_listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.focus_prompt_slot(si, window, cx);
                        }),
                    ),
            );
        }
        width += 6. + 14. + 1. + 4.;
        let el = div()
            .id(SharedString::from(format!("pchip-{i}")))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .min_h(px(22.))
            .pl(px(8.))
            .pr(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(border))
            .bg(hsla(tint(0.08, t.pane_2)))
            .child(
                text(cmd)
                    .size(11.5)
                    .weight(650.)
                    .tracking(0.01)
                    .line_height(LineHeight::Scale(1.))
                    .color(t.cyan)
                    .nowrap(),
            )
            .children(slots)
            .child(x);
        let el = recur_ui::tooltip::titled(
            SharedString::from(format!("pchip-{i}-tip")),
            format!("/{slash}"),
            el,
        );
        (el.into_any_element(), width)
    }
}
