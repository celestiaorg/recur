//! A whole-file Mermaid diagram with fit, zoom and pan. Zoom resizes the
//! drawing (vectors and labels re-rasterize at the new size, as the SVG's width/height do),
//! the camera persists per path, and find pans to a match.

use recur_ui::guard::Guarded;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Bounds, Context, CursorStyle, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, Task, Window, canvas, div, point, px,
};
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, TextStyled, Theme, icon, rgba};

use super::Diagram;
use crate::pdf::logic::{MAX_SCALE, MIN_SCALE, ZOOM_STEP};
use crate::preview::scroll::{PreviewScroll, preview_camera};
use crate::preview::view::SharedScroll;

const PAD: f32 = 40.;

fn clamp_scale(s: f32) -> f32 {
    s.clamp(MIN_SCALE, MAX_SCALE)
}

/// The diagram (and with it the text find searches) was replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagramChanged;

pub struct MermaidPreview {
    content: SharedString,
    path: SharedString,
    scroll: Option<SharedScroll>,
    diagram: Option<Rc<Diagram>>,
    error: String,
    pending: bool,
    scale: f32,
    tx: f32,
    ty: f32,
    user_adjusted: bool,
    drag: Option<(gpui_kit::Point<Pixels>, f32, f32)>,
    viewport: Option<Bounds<Pixels>>,
    highlights: HashMap<usize, Vec<(Range<usize>, bool)>>,
    task: Option<Task<()>>,
    /// A camera to restore once the diagram is drawn (`peek(path)`).
    restore: Option<super::super::preview::scroll::Camera>,
}

impl MermaidPreview {
    pub fn new(
        content: &str,
        path: &str,
        scroll: Option<SharedScroll>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut p = MermaidPreview {
            content: SharedString::default(),
            path: path.to_string().into(),
            scroll,
            diagram: None,
            error: String::new(),
            pending: false,
            scale: 1.,
            tx: 0.,
            ty: 0.,
            user_adjusted: false,
            drag: None,
            viewport: None,
            highlights: HashMap::new(),
            task: None,
            restore: None,
        };
        p.set_content(content, cx);
        p
    }

    /// The source changed: reset the camera and render again (errors clear first, so an edit
    /// can recover).
    pub fn set_content(&mut self, content: &str, cx: &mut Context<Self>) {
        if content == self.content.as_ref()
            && (self.diagram.is_some() || self.pending || !self.error.is_empty())
        {
            return;
        }
        self.content = content.to_string().into();
        self.error.clear();
        self.user_adjusted = false;
        self.scale = 1.;
        self.tx = 0.;
        self.ty = 0.;
        if self.diagram.take().is_some() {
            cx.emit(DiagramChanged);
        }
        self.restore = self
            .scroll
            .as_ref()
            .filter(|_| !self.path.is_empty())
            .and_then(|s| preview_camera(s.borrow().peek(&self.path)));
        if content.trim().is_empty() {
            self.pending = false;
            self.task = None;
            cx.notify();
            return;
        }
        self.pending = true;
        let theme = Theme::of(cx).clone();
        let src = content.to_string();
        self.task = Some(cx.spawn(async move |this, cx| {
            let res = super::render(src, &theme, cx).await;
            let _ = this.update(cx, |p, cx| p.finish(res, cx));
        }));
        cx.notify();
    }

    fn finish(&mut self, res: Result<Rc<Diagram>, String>, cx: &mut Context<Self>) {
        self.pending = false;
        match res {
            Ok(d) => {
                self.diagram = Some(d);
                if let Some(cam) = self.restore.take() {
                    self.scale = clamp_scale(cam.zoom as f32);
                    self.tx = cam.left as f32;
                    self.ty = cam.top as f32;
                    self.user_adjusted = true;
                } else {
                    self.fit(true);
                }
            }
            Err(e) => {
                self.error = if e.is_empty() {
                    "Mermaid render failed".into()
                } else {
                    e
                }
            }
        }
        cx.emit(DiagramChanged);
        cx.notify();
    }

    fn natural(&self) -> Option<(f32, f32)> {
        let d = self.diagram.as_ref()?;
        Some((d.view_box.2, d.view_box.3)).filter(|(w, h)| *w >= 1. && *h >= 1.)
    }

    /// `fit`: the whole diagram centred with 20px of air (unless the user moved the camera).
    fn fit(&mut self, force: bool) {
        if self.user_adjusted && !force {
            return;
        }
        let (Some((nw, nh)), Some(vp)) = (self.natural(), self.viewport) else {
            return;
        };
        let (vw, vh) = (f32::from(vp.size.width), f32::from(vp.size.height));
        if vw < 8. || vh < 8. {
            return;
        }
        let next = clamp_scale(((vw - PAD) / nw).min((vh - PAD) / nh));
        self.scale = next;
        self.tx = (vw - nw * next) / 2.;
        self.ty = (vh - nh * next) / 2.;
        self.user_adjusted = false;
    }

    fn zoom_at(&mut self, factor: f32, cx_: f32, cy_: f32, cx: &mut Context<Self>) {
        let next = clamp_scale(self.scale * factor);
        if next == self.scale {
            return;
        }
        self.tx = cx_ - ((cx_ - self.tx) / self.scale) * next;
        self.ty = cy_ - ((cy_ - self.ty) / self.scale) * next;
        self.scale = next;
        self.adjusted(cx);
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let Some(vp) = self.viewport else { return };
        let (w, h) = (f32::from(vp.size.width), f32::from(vp.size.height));
        self.zoom_at(factor, w / 2., h / 2., cx);
    }

    /// The camera moved: persist it (`remember(path, { top: ty, left: tx, zoom })`).
    fn adjusted(&mut self, cx: &mut Context<Self>) {
        self.user_adjusted = true;
        if let Some(s) = &self.scroll
            && !self.path.is_empty()
        {
            s.borrow_mut().remember(
                &self.path,
                PreviewScroll {
                    top: self.ty as f64,
                    left: self.tx as f64,
                    zoom: Some(self.scale as f64),
                },
            );
        }
        cx.notify();
    }

    fn reset_view(&mut self, cx: &mut Context<Self>) {
        self.user_adjusted = false;
        if let Some(s) = &self.scroll
            && !self.path.is_empty()
        {
            s.borrow_mut()
                .remember(&self.path, PreviewScroll::at(0., 0.));
        }
        self.fit(true);
        cx.notify();
    }

    fn interactive(&self) -> bool {
        !self.pending && self.error.is_empty() && !self.content.trim().is_empty()
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn camera(&self) -> (f32, f32, f32) {
        (self.tx, self.ty, self.scale)
    }

    /// Find: the diagram's labels as one block.
    pub fn text_blocks(&self) -> Vec<(usize, String)> {
        self.diagram
            .as_ref()
            .map(|d| vec![(0, d.text())])
            .unwrap_or_default()
    }

    pub fn highlights(&self) -> &HashMap<usize, Vec<(Range<usize>, bool)>> {
        &self.highlights
    }

    pub fn set_highlights(
        &mut self,
        h: HashMap<usize, Vec<(Range<usize>, bool)>>,
        cx: &mut Context<Self>,
    ) {
        if h != self.highlights {
            self.highlights = h;
            cx.notify();
        }
    }

    /// `panMermaidCamera`: centre a match in the viewport.
    pub fn reveal(
        &mut self,
        _block: usize,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(d), Some(vp)) = (self.diagram.clone(), self.viewport) else {
            return;
        };
        let Some((x, y, w, h)) = d.range_rects(range, self.scale, window).into_iter().next() else {
            return;
        };
        let (bx, by) = (self.tx + x + w / 2., self.ty + y + h / 2.);
        let (vx, vy) = (
            f32::from(vp.size.width) / 2.,
            f32::from(vp.size.height) / 2.,
        );
        let (dx, dy) = (vx - bx, vy - by);
        if dx.abs() < 1. && dy.abs() < 1. {
            return;
        }
        self.tx += dx;
        self.ty += dy;
        self.user_adjusted = true;
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn tb_button(
        &self,
        id: &'static str,
        title: &'static str,
        glyph: AnyElement,
        disabled: bool,
        last: bool,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let b = div()
            .id(id)
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .px(px(9.))
            .text_color(t.text)
            .when(!last, |d| d.border_r_1().border_color(t.line))
            .when(!disabled, |d| {
                d.hover(move |s| s.bg(t.pane_2).border_color(t.line_bright))
                    .on_click(cx.guarded_listener(move |p, _, _, cx| on(p, cx)))
            })
            .when(disabled, |d| d.opacity(0.35))
            .child(glyph);
        titled(id, title, b).into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let pct = format!("{}%", (self.scale * 100.).round() as i64);
        let ic = |name: IconName| icon(name).size(px(14.)).into_any_element();
        let pct_button = div()
            .id("mmd-pct")
            .h(px(28.))
            .min_w(px(52.))
            .flex()
            .items_center()
            .justify_center()
            .px(px(9.))
            .border_r_1()
            .border_color(t.line)
            .hover(move |s| s.bg(t.pane_2).border_color(t.line_bright))
            .on_click(cx.guarded_listener(|p, _, _, cx| p.reset_view(cx)))
            .child(
                recur_ui::text(pct)
                    .mono()
                    .size(11.)
                    .tabular()
                    .weight(550.)
                    .color(t.muted)
                    .nowrap(),
            );
        let bar = recur_ui::style::card(t.pane, t.line, 7.)
            .overflow_hidden()
            .flex()
            .items_center()
            .child(self.tb_button(
                "mmd-out",
                "Zoom out",
                ic(IconName::Minus),
                self.scale <= MIN_SCALE,
                false,
                cx,
                |p, cx| p.zoom_by(1. / ZOOM_STEP, cx),
            ))
            .child(titled("mmd-pct", "Fit to view", pct_button))
            .child(self.tb_button(
                "mmd-in",
                "Zoom in",
                ic(IconName::Plus),
                self.scale >= MAX_SCALE,
                false,
                cx,
                |p, cx| p.zoom_by(ZOOM_STEP, cx),
            ))
            .child(self.tb_button(
                "mmd-fit",
                "Fit to view",
                ic(IconName::Maximize),
                false,
                true,
                cx,
                |p, cx| p.reset_view(cx),
            ));
        div()
            .absolute()
            .top(px(10.))
            .right(px(10.))
            .child(with_shadow(
                bar,
                &[css_box_shadow(0., 6., 18., 0., rgba(0, 0, 0, 0.28))],
                7.,
            ))
            .into_any_element()
    }
}

impl gpui_kit::EventEmitter<DiagramChanged> for MermaidPreview {}

impl MermaidPreview {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let message = |label: String, error: bool| {
            let color = if error {
                t.coral.mix(0.85, t.text)
            } else {
                t.faint
            };
            div()
                .absolute()
                .inset_0()
                .bg(t.bg)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div().px(px(24.)).py(px(48.)).max_w(px(12. * 40.)).child(
                        recur_ui::text(label)
                            .ui()
                            .size(12.)
                            .leading(1.45)
                            .color(color)
                            .pre_wrap()
                            .align(recur_ui::text::Align::Center),
                    ),
                )
        };
        let root = div()
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(t.bg);
        if self.content.trim().is_empty() {
            return root.child(
                div().flex().justify_center().px(px(24.)).py(px(48.)).child(
                    recur_ui::text("Empty Mermaid")
                        .ui()
                        .size(12.)
                        .leading(1.45)
                        .color(t.faint),
                ),
            );
        }
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |b, _, cx| {
                let _ = this.update(cx, |p, cx| {
                    let changed = p.viewport.is_none_or(|v| v.size != b.size);
                    p.viewport = Some(b);
                    if changed {
                        // `ResizeObserver(() => fit(false))`.
                        p.fit(false);
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let stage = self.diagram.clone().map(|d| {
            let (nw, nh) = (d.view_box.2, d.view_box.3);
            let colors = (t.amber.fade(0.4).into(), t.amber.into());
            // WebKit paints the translated stage unsnapped.
            let dev = window.scale_factor();
            let nudge = (
                self.tx - (self.tx * dev).round() / dev,
                self.ty - (self.ty * dev).round() / dev,
            );
            div()
                .absolute()
                .left(px(self.tx))
                .top(px(self.ty))
                .child(d.element(
                    nw * self.scale,
                    nh * self.scale,
                    &self.highlights,
                    colors,
                    nudge,
                    window,
                ))
        });
        let hidden = self.pending || !self.error.is_empty();
        let viewport = div()
            .id("mmd-viewport")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .cursor(if self.drag.is_some() {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .child(measure)
            .when(!hidden, |d| d.children(stage))
            .on_scroll_wheel(cx.guarded_listener(|p, e: &ScrollWheelEvent, window, cx| {
                if !p.interactive() {
                    return;
                }
                let Some(vp) = p.viewport else { return };
                let dy = f32::from(e.delta.pixel_delta(window.line_height()).y);
                let factor = if dy > 0. { ZOOM_STEP } else { 1. / ZOOM_STEP };
                let (x, y) = (
                    f32::from(e.position.x - vp.origin.x),
                    f32::from(e.position.y - vp.origin.y),
                );
                p.zoom_at(factor, x, y, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|p, e: &MouseDownEvent, _, cx| {
                    if !p.interactive() {
                        return;
                    }
                    if e.click_count == 2 {
                        p.reset_view(cx);
                        return;
                    }
                    p.drag = Some((e.position, p.tx, p.ty));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.guarded_listener(|p, e: &MouseMoveEvent, _, cx| {
                if let Some((o, tx, ty)) = p.drag {
                    p.tx = tx + f32::from(e.position.x - o.x);
                    p.ty = ty + f32::from(e.position.y - o.y);
                    p.adjusted(cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.guarded_listener(|p, _: &MouseUpEvent, _, cx| {
                    if p.drag.take().is_some() {
                        cx.notify();
                    }
                }),
            );
        let mut root = root.child(viewport);
        if self.error.is_empty() {
            root = root.child(self.toolbar(cx));
        }
        if self.pending {
            root = root.child(message("Rendering…".into(), false));
        } else if !self.error.is_empty() {
            root = root.child(message(self.error.clone(), true));
        }
        let _ = point(px(0.), px(0.));
        root
    }
}

impl Render for MermaidPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
