//! `PdfPreview.svelte`: every page as a fit-to-width slot, pages rendered by pdf.js while
//! they intersect the viewport (and discarded when they leave it), a page field that follows
//! the scroll, and zoom by toolbar or ⌘+ / ⌘-.

use recur_ui::guard::Guarded;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext, Bounds, Context, Corners, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, canvas, div,
    point, px,
};
use recur_ui::fonts::Family;
use recur_ui::tooltip::titled;
use recur_ui::{IconName, TextStyled, Theme, icon};

use super::engine::{self, Document, Opened, PageImage};
use super::logic::{MAX_SCALE, MIN_SCALE, clamp_page, fit_width_scale, page_at_scroll, zoom_step};

const PAGE_GAP: f32 = 12.;
const VIEW_PAD: f32 = 12.;

/// A page's canvas: rendering, or painted, at a zoom and device scale.
struct Slot {
    scale: f32,
    dpr: f32,
    task: u64,
    painted: Option<PageImage>,
    _render: Option<Task<()>>,
}

pub struct PdfPreview {
    data: Option<Arc<[u8]>>,
    doc: Option<Rc<Document>>,
    load: Option<Task<()>>,
    error: String,
    pending: bool,
    page: usize,
    page_count: usize,
    scale: f32,
    page_width: f32,
    sizes: Vec<(f32, f32)>,
    /// Opened before the viewport had a size: fit once it has one.
    fit_on_measure: bool,
    slots: HashMap<usize, Slot>,
    next_task: u64,
    handle: ScrollHandle,
    viewport: Option<Bounds<Pixels>>,
    last_offset: f32,
    input: Entity<InputState>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl PdfPreview {
    /// `src`: the document's bytes (`None` shows nothing).
    pub fn new(data: Option<Arc<[u8]>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_text_align(gpui_kit::TextAlign::Center, cx);
            s
        });
        recur_ui::style::set_field_shape(&input, false, gpui_kit::TextAlign::Center, cx);
        let subs =
            vec![
                cx.guarded_subscribe_in(
                    &input,
                    window,
                    |p, _, ev: &InputEvent, window, cx| match ev {
                        InputEvent::PressEnter { .. } => p.commit_page(window, cx),
                        InputEvent::Blur => p.commit_page(window, cx),
                        InputEvent::Change | InputEvent::Focus => cx.notify(),
                    },
                ),
            ];
        let mut p = Self {
            data: None,
            doc: None,
            load: None,
            error: String::new(),
            pending: false,
            page: 1,
            page_count: 0,
            scale: 1.,
            page_width: 0.,
            sizes: Vec::new(),
            fit_on_measure: false,
            slots: HashMap::new(),
            next_task: 1,
            handle: ScrollHandle::new(),
            viewport: None,
            last_offset: 0.,
            input,
            focus: cx.focus_handle(),
            _subscriptions: subs,
        };
        p.set_data(data, window, cx);
        p
    }

    /// Replace the document (the `src` effect): everything resets and the new one loads.
    pub fn set_data(
        &mut self,
        data: Option<Arc<[u8]>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if match (&self.data, &data) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => self.doc.is_none() && !self.pending && self.load.is_none(),
            _ => false,
        } {
            return;
        }
        self.data = data.clone();
        self.load = None;
        self.cancel_slots();
        self.doc = None;
        self.page_count = 0;
        self.page_width = 0.;
        self.sizes.clear();
        self.error.clear();
        self.fit_on_measure = false;
        self.set_page_number(1, window, cx);
        let Some(bytes) = data else {
            self.pending = false;
            cx.notify();
            return;
        };
        self.pending = true;
        if crate::webhost::blocking(cx) {
            let (doc, r) = engine::open_blocking(bytes);
            self.opened(doc, r, cx);
        } else {
            self.load = Some(cx.spawn(async move |this, cx| {
                let (doc, r) = engine::open(bytes).await;
                let _ = this.update(cx, |p, cx| p.opened(doc, r, cx));
            }));
        }
        cx.notify();
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    fn opened(&mut self, doc: Document, r: Result<Opened, String>, cx: &mut Context<Self>) {
        self.load = None;
        self.pending = false;
        match r {
            Ok(o) => {
                self.page_width = o.width;
                // simplify: page 1's size is every slot's placeholder; a later page's first
                // render corrects its slot if it differs (as the web preview does).
                self.sizes = vec![(o.width, o.height); o.pages];
                self.page_count = o.pages;
                self.doc = Some(Rc::new(doc));
                match self.viewport {
                    Some(_) => self.fit_width(cx),
                    None => self.fit_on_measure = true,
                }
            }
            Err(e) => self.error = e,
        }
        cx.notify();
    }

    fn ready(&self) -> bool {
        self.page_count > 0 && self.error.is_empty()
    }

    fn fit_width(&mut self, cx: &mut Context<Self>) {
        let inner = (self.viewport.map_or(0., |v| f32::from(v.size.width)) - VIEW_PAD * 2.).max(0.);
        self.scale = fit_width_scale(self.page_width, inner);
        cx.notify();
    }

    fn zoom(&mut self, dir: i8, cx: &mut Context<Self>) {
        self.scale = zoom_step(self.scale, dir);
        cx.notify();
    }

    fn set_page_number(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.page = n;
        let text = n.to_string();
        self.input.update(cx, |s, cx| {
            if s.value() != text.as_str() {
                s.set_value(text, window, cx);
            }
        });
    }

    /// The scroll top that puts page `n`'s slot at the viewport top.
    fn slot_top(&self, n: usize) -> f32 {
        VIEW_PAD
            + self.sizes[..n.saturating_sub(1).min(self.sizes.len())]
                .iter()
                .map(|(_, h)| h * self.scale + PAGE_GAP)
                .sum::<f32>()
    }

    /// `setPage`: clamp, show it in the field and scroll its slot to the top.
    fn set_page(&mut self, n: f64, window: &mut Window, cx: &mut Context<Self>) {
        let next = clamp_page(n, self.page_count);
        self.set_page_number(next, window, cx);
        if self.page_count > 0 {
            let max = f32::from(self.handle.max_offset().y).max(0.);
            let top = self.slot_top(next).min(max);
            let x = self.handle.offset().x;
            self.handle.set_offset(point(x, px(-top)));
            self.last_offset = top;
        }
        cx.notify();
    }

    fn commit_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.input.read(cx).value().to_string();
        // `Number(pageDraft)`: blank is 0, anything unparsable NaN.
        let n = match draft.trim() {
            "" => 0.,
            s => s.parse::<f64>().unwrap_or(f64::NAN),
        };
        self.set_page(n, window, cx);
    }

    /// `syncPageFromScroll`.
    fn sync_page_from_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(vp) = self.viewport else { return };
        if self.sizes.is_empty() {
            return;
        }
        let top = -f32::from(self.handle.offset().y);
        let origin = VIEW_PAD;
        let heights: Vec<f32> = self.sizes.iter().map(|(_, h)| h * self.scale).collect();
        let content =
            VIEW_PAD * 2. + heights.iter().sum::<f32>() + PAGE_GAP * (heights.len() - 1) as f32;
        let max = content - f32::from(vp.size.height) - origin;
        let next = page_at_scroll(top - origin, &heights, PAGE_GAP, Some(max), Some(self.page));
        if next != self.page {
            self.set_page_number(next, window, cx);
        }
    }

    /// Pages whose slots intersect the viewport (`IntersectionObserver` with the viewport
    /// as root; touching edges count).
    fn visible_pages(&self) -> HashSet<usize> {
        let Some(vp) = self.viewport else {
            return HashSet::new();
        };
        let top = -f32::from(self.handle.offset().y);
        let bottom = top + f32::from(vp.size.height);
        let mut y = VIEW_PAD;
        let mut out = HashSet::new();
        for (i, (_, h)) in self.sizes.iter().enumerate() {
            let h = h * self.scale;
            if y <= bottom && y + h >= top {
                out.insert(i + 1);
            }
            y += h + PAGE_GAP;
        }
        out
    }

    fn cancel_slots(&mut self) {
        for s in self.slots.values() {
            if s.painted.is_none() {
                engine::cancel(s.task);
            }
        }
        self.slots.clear();
    }

    /// Render visible pages at the current zoom and device scale; discard the rest.
    fn sync_slots(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone().filter(|_| self.error.is_empty()) else {
            return;
        };
        let dpr = window.scale_factor();
        let visible = self.visible_pages();
        let scale = self.scale;
        self.slots.retain(|n, s| {
            let keep = visible.contains(n) && s.scale == scale && s.dpr == dpr;
            if !keep && s.painted.is_none() {
                engine::cancel(s.task);
            }
            keep
        });
        let mut pages: Vec<usize> = visible
            .into_iter()
            .filter(|n| !self.slots.contains_key(n))
            .collect();
        pages.sort_unstable();
        for n in pages {
            let task = self.next_task;
            self.next_task += 1;
            if crate::webhost::blocking(cx) {
                let r = engine::render_blocking(&doc, n, scale, dpr, task);
                self.slots.insert(
                    n,
                    Slot {
                        scale,
                        dpr,
                        task,
                        painted: None,
                        _render: None,
                    },
                );
                self.rendered(n, task, r);
                continue;
            }
            let doc = doc.clone();
            let render = cx.spawn(async move |this, cx| {
                let r = engine::render(&doc, n, scale, dpr, task).await;
                let _ = this.update(cx, |p, cx| {
                    p.rendered(n, task, r);
                    cx.notify();
                });
            });
            self.slots.insert(
                n,
                Slot {
                    scale,
                    dpr,
                    task,
                    painted: None,
                    _render: Some(render),
                },
            );
        }
    }

    fn rendered(&mut self, n: usize, task: u64, r: Result<Option<PageImage>, String>) {
        let Some(slot) = self.slots.get_mut(&n).filter(|s| s.task == task) else {
            return;
        };
        match r {
            Ok(Some(img)) => {
                if let Some(size) = self.sizes.get_mut(n - 1)
                    && *size != (img.width, img.height)
                {
                    *size = (img.width, img.height);
                }
                slot.painted = Some(img);
                slot._render = None;
            }
            // Cancelled: the slot left the viewport or changed zoom; its replacement renders.
            Ok(None) => {
                self.slots.remove(&n);
            }
            Err(e) => {
                self.slots.remove(&n);
                self.error = e;
            }
        }
    }

    fn key_down(&mut self, e: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.error.is_empty() || self.page_count < 1 {
            return;
        }
        let k = &e.keystroke;
        if !(k.modifiers.platform || k.modifiers.control) {
            return;
        }
        match k.key.as_str() {
            "+" | "=" => self.zoom(1, cx),
            "-" => self.zoom(-1, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// `.obs-icon-btn` (26×26, radius 5, `--faint`; hover `--text` on `--pane-2`; disabled at
    /// .35 opacity without hover).
    fn icon_button(
        &self,
        id: &'static str,
        title: &'static str,
        name: IconName,
        disabled: bool,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let b = div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(5.))
            .text_color(t.faint)
            .when(disabled, |d| d.opacity(0.35))
            .when(!disabled, |d| {
                d.hover(move |s| s.text_color(t.text).bg(t.pane_2))
                    .on_click(cx.guarded_listener(move |p, _, window, cx| on(p, window, cx)))
            })
            .child(icon(name).size(px(15.)));
        titled(id, title, b).into_any_element()
    }

    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let label = |s: String, color| {
            recur_ui::text(s)
                .family(Family::Mono)
                .size(11.)
                .tabular()
                .color(color)
                .nowrap()
        };
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        // `.obs-pdf-page`: a 3.2em field, value centered; `--pane-2` while focused.
        let field_text = recur_ui::style::text_field(
            &self.input,
            &SharedString::default(),
            t.text,
            Family::Mono,
            11.,
            window,
            cx,
        );
        let field = div()
            .id("obs-pdf-page")
            .flex_none()
            .h(px(26.))
            .w(px(11. * 3.2))
            .px(px(6.))
            .rounded(px(5.))
            .when(focused, |d| d.bg(t.pane_2))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .child(field_text);
        let of = div()
            .flex_none()
            .h(px(26.))
            .px(px(4.))
            .flex()
            .items_center()
            .child(label(format!("of {}", self.page_count), t.muted));
        let pct = format!("{}%", (self.scale * 100.).round() as i64);
        let pct_button = div()
            .id("obs-pdf-tb-pct")
            .flex_none()
            .h(px(26.))
            .px(px(6.))
            .rounded(px(5.))
            .flex()
            .items_center()
            .text_color(t.muted)
            .hover(move |s| s.text_color(t.text).bg(t.pane_2))
            .on_click(cx.guarded_listener(|p, _, _, cx| p.fit_width(cx)))
            .child(
                recur_ui::text(pct)
                    .family(Family::Mono)
                    .size(11.)
                    .tabular()
                    .nowrap(),
            );
        let group = |children: Vec<AnyElement>| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(2.))
                .children(children)
        };
        let (page, count) = (self.page, self.page_count);
        let paging = group(vec![
            self.icon_button(
                "obs-pdf-prev",
                "Previous page",
                IconName::ChevronLeft,
                page <= 1,
                cx,
                |p, w, cx| p.set_page(p.page as f64 - 1., w, cx),
            ),
            field.into_any_element(),
            of.into_any_element(),
            self.icon_button(
                "obs-pdf-next",
                "Next page",
                IconName::ChevronRight,
                page >= count,
                cx,
                |p, w, cx| p.set_page(p.page as f64 + 1., w, cx),
            ),
        ]);
        let zoom = group(vec![
            self.icon_button(
                "obs-pdf-out",
                "Zoom out",
                IconName::Minus,
                self.scale <= MIN_SCALE,
                cx,
                |p, _, cx| p.zoom(-1, cx),
            ),
            titled("obs-pdf-tb-pct", "Fit to width", pct_button).into_any_element(),
            self.icon_button(
                "obs-pdf-in",
                "Zoom in",
                IconName::Plus,
                self.scale >= MAX_SCALE,
                cx,
                |p, _, cx| p.zoom(1, cx),
            ),
            self.icon_button(
                "obs-pdf-fit",
                "Fit to width",
                IconName::Maximize,
                false,
                cx,
                |p, _, cx| p.fit_width(cx),
            ),
        ]);
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .h(px(40.))
            .px(px(8.))
            .min_w_0()
            .border_b_1()
            .border_color(t.line)
            .bg(t.pane)
            .child(paging)
            .child(zoom)
            .into_any_element()
    }

    /// `.obs-pdf-page-slot`: white, a 1px `--line` ring outside, the canvas stretched over it.
    fn slot(&self, n: usize, w: f32, h: f32, line: recur_ui::Color) -> AnyElement {
        let image = self
            .slots
            .get(&n)
            .and_then(|s| s.painted.as_ref())
            .map(|p| p.image.clone());
        div()
            .relative()
            .flex_none()
            .w(px(w))
            .h(px(h))
            .bg(recur_ui::hex(0xffffff))
            .child(
                div()
                    .absolute()
                    .top(px(-1.))
                    .left(px(-1.))
                    .right(px(-1.))
                    .bottom(px(-1.))
                    .border_1()
                    .border_color(line),
            )
            .children(image.map(|img| {
                canvas(
                    |_, _, _| {},
                    move |b: Bounds<Pixels>, _, window, _| {
                        let _ = window.paint_image(b, b, Corners::default(), img.clone(), 0, false);
                    },
                )
                .absolute()
                .inset_0()
            }))
            .into_any_element()
    }
}

impl PdfPreview {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        recur_ui::style::style_text_input(&self.input, cx);
        let t = Theme::of(cx).tokens;
        self.sync_slots(window, cx);
        let this = cx.entity().downgrade();
        // The viewport's size (`clientWidth`) and scroll (`onscroll`), read each frame.
        let measure = canvas(
            move |b, window, cx| {
                let _ = this.update(cx, |p, cx| {
                    let first = p.viewport.is_none();
                    let resized = p.viewport.is_some_and(|v| v.size != b.size);
                    p.viewport = Some(b);
                    if first && p.fit_on_measure {
                        p.fit_on_measure = false;
                        p.fit_width(cx);
                    }
                    let top = -f32::from(p.handle.offset().y);
                    if (top - p.last_offset).abs() >= 0.5 {
                        p.last_offset = top;
                        p.sync_page_from_scroll(window, cx);
                        cx.notify();
                    } else if first || resized {
                        cx.notify();
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let hidden = self.pending || !self.error.is_empty();
        let pages = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(PAGE_GAP))
            .min_w_full()
            .children(
                self.sizes
                    .iter()
                    .enumerate()
                    .map(|(i, (w, h))| self.slot(i + 1, w * self.scale, h * self.scale, t.line)),
            );
        let viewport = div()
            .id("obs-pdf-viewport")
            .track_focus(&self.focus)
            .track_scroll(&self.handle)
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_scroll()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.guarded_listener(|p, _, window, cx| p.focus.focus(window, cx)),
            )
            .child(div().p(px(VIEW_PAD)).flex().min_w_full().child(pages))
            .when(hidden, |d| d.invisible());
        let message = (self.pending || !self.error.is_empty()).then(|| {
            let (label, color) = if self.pending {
                ("Loading…".to_string(), t.faint)
            } else {
                (self.error.clone(), t.coral.mix(0.85, t.text))
            };
            div()
                .absolute()
                .inset_0()
                .bg(t.bg)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px(px(24.))
                        .py(px(48.))
                        .when(!self.pending, |d| d.max_w(px(12. * 40.)))
                        .child(
                            recur_ui::text(label)
                                .ui()
                                .size(12.)
                                .leading(1.45)
                                .color(color)
                                .pre_wrap()
                                .align(recur_ui::text::Align::Center),
                        ),
                )
        });
        div()
            .id("obs-pdf-preview")
            .key_context("PdfPreview")
            .on_key_down(cx.guarded_listener(Self::key_down))
            .relative()
            .flex_1()
            .min_h_0()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(self.ready(), |d| d.child(self.toolbar(window, cx)))
            .children(message)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(measure)
                    .child(viewport),
            )
    }
}

impl Render for PdfPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
