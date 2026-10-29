//! `PdfPreview.svelte`: every page as a fit-to-width slot, pages rendered by hayro while
//! they intersect the viewport (and discarded when they leave it), a page field that follows
//! the scroll, zoom by toolbar or ⌘+ / ⌘-, and find (⌘F) over the pages' text layer.

use recur_ui::guard::Guarded;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext, Bounds, Context, Corners, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Render, RenderImage,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window,
    canvas, div, point, px,
};
use recur_ui::fonts::Family;
use recur_ui::tooltip::titled;
use recur_ui::{IconName, TextStyled, Theme, icon};

use super::engine::{self, Document, FindText, NormRect, Opened};
use super::logic::{MAX_SCALE, MIN_SCALE, clamp_page, fit_width_scale, page_at_scroll, zoom_step};
use crate::preview::find::collect_preview_find_ranges;
use crate::preview::find_bar::{FindBarEvent, PreviewFindBar};
use crate::preview::view::FilePreviewEvent;

const PAGE_GAP: f32 = 12.;
const VIEW_PAD: f32 = 12.;

/// A page's canvas: rendering, or painted, at a zoom and device scale.
struct Slot {
    scale: f32,
    dpr: f32,
    painted: Option<Arc<RenderImage>>,
    _render: Option<Task<()>>,
}

pub struct PdfPreview {
    data: Option<Arc<[u8]>>,
    doc: Option<Arc<Document>>,
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
    handle: ScrollHandle,
    viewport: Option<Bounds<Pixels>>,
    last_offset: f32,
    input: Entity<InputState>,
    focus: FocusHandle,
    find_open: bool,
    bar: Entity<PreviewFindBar>,
    /// Extracted on the first find, then kept for the document.
    find_text: Option<FindText>,
    find_task: Option<Task<()>>,
    /// Every match: its page (1-based) and its rects on that page.
    matches: Vec<(usize, Vec<NormRect>)>,
    /// The current match, 1-based (0: none).
    index: usize,
    _subscriptions: Vec<Subscription>,
}

impl gpui_kit::EventEmitter<FilePreviewEvent> for PdfPreview {}

impl PdfPreview {
    /// `src`: the document's bytes (`None` shows nothing).
    pub fn new(data: Option<Arc<[u8]>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_text_align(gpui_kit::TextAlign::Center, cx);
            s
        });
        recur_ui::style::set_field_shape(&input, false, gpui_kit::TextAlign::Center, cx);
        let bar = cx.new(|cx| PreviewFindBar::new(window, cx));
        let subs = vec![
            cx.guarded_subscribe_in(
                &input,
                window,
                |p, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { .. } => p.commit_page(window, cx),
                    InputEvent::Blur => p.commit_page(window, cx),
                    InputEvent::Change | InputEvent::Focus => cx.notify(),
                },
            ),
            cx.guarded_subscribe_in(
                &bar,
                window,
                |p, _, ev: &FindBarEvent, window, cx| match ev {
                    FindBarEvent::Changed => p.refresh_find(cx),
                    FindBarEvent::Next => p.step_find(1, cx),
                    FindBarEvent::Previous => p.step_find(-1, cx),
                    FindBarEvent::Close => p.close_search(window, cx),
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
            handle: ScrollHandle::new(),
            viewport: None,
            last_offset: 0.,
            input,
            focus: cx.focus_handle(),
            find_open: false,
            bar,
            find_text: None,
            find_task: None,
            matches: Vec::new(),
            index: 0,
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
        self.slots.clear();
        self.find_text = None;
        self.find_task = None;
        self.matches.clear();
        self.index = 0;
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
            self.opened(engine::open(&bytes), cx);
        } else {
            let open = cx.background_spawn(async move { engine::open(&bytes) });
            self.load = Some(cx.spawn(async move |this, cx| {
                let r = open.await;
                let _ = this.update(cx, |p, cx| p.opened(r, cx));
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

    fn opened(&mut self, r: Result<Opened, String>, cx: &mut Context<Self>) {
        self.load = None;
        self.pending = false;
        match r {
            Ok(o) => {
                self.page_width = o.sizes[0].0;
                self.page_count = o.sizes.len();
                self.sizes = o.sizes;
                self.doc = Some(o.doc);
                match self.viewport {
                    Some(_) => self.fit_width(cx),
                    None => self.fit_on_measure = true,
                }
                self.refresh_find(cx);
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

    /// Render visible pages at the current zoom and device scale; discard the rest.
    fn sync_slots(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone().filter(|_| self.error.is_empty()) else {
            return;
        };
        let dpr = window.scale_factor();
        let visible = self.visible_pages();
        let scale = self.scale;
        // Dropping a slot drops its render task, which cancels a render not yet started.
        self.slots
            .retain(|n, s| visible.contains(n) && s.scale == scale && s.dpr == dpr);
        let mut pages: Vec<usize> = visible
            .into_iter()
            .filter(|n| !self.slots.contains_key(n))
            .collect();
        pages.sort_unstable();
        for n in pages {
            let blocking = crate::webhost::blocking(cx);
            let render = (!blocking).then(|| {
                let doc = doc.clone();
                let r = cx.background_spawn(async move { engine::render(&doc, n, scale, dpr) });
                cx.spawn(async move |this, cx| {
                    let r = r.await;
                    let _ = this.update(cx, |p, cx| {
                        p.rendered(n, r);
                        cx.notify();
                    });
                })
            });
            self.slots.insert(
                n,
                Slot {
                    scale,
                    dpr,
                    painted: None,
                    _render: render,
                },
            );
            if blocking {
                self.rendered(n, engine::render(&doc, n, scale, dpr));
            }
        }
    }

    fn rendered(&mut self, n: usize, r: Result<Arc<RenderImage>, String>) {
        let Some(slot) = self.slots.get_mut(&n) else {
            return;
        };
        match r {
            Ok(img) => {
                slot.painted = Some(img);
                slot._render = None;
            }
            Err(e) => {
                self.slots.remove(&n);
                self.error = e;
            }
        }
    }

    pub fn is_search_open(&self) -> bool {
        self.find_open
    }

    pub fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_open {
            self.close_search(window, cx);
        } else {
            self.open_search(window, cx);
        }
    }

    /// The find bar's count: empty while there is no query.
    pub fn find_count_label(&self, cx: &gpui_kit::App) -> String {
        self.bar.read(cx).count_label(cx)
    }

    /// Open (or refocus) the find bar and select its query.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready() {
            return;
        }
        if !self.find_open {
            self.find_open = true;
            cx.emit(FilePreviewEvent::SearchOpenChanged(true));
        }
        self.bar.update(cx, |b, cx| b.focus_and_select(window, cx));
        self.refresh_find(cx);
        cx.notify();
    }

    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        self.find_open = false;
        cx.emit(FilePreviewEvent::SearchOpenChanged(false));
        self.matches.clear();
        self.index = 0;
        self.bar.update(cx, |b, cx| {
            b.reset_counts();
            cx.notify();
        });
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Match the query against every page's text (extracting it off the UI thread first).
    fn refresh_find(&mut self, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        let Some(text) = &self.find_text else {
            let Some(doc) = self.doc.clone().filter(|_| self.find_task.is_none()) else {
                return;
            };
            if crate::webhost::blocking(cx) {
                self.find_text = Some(engine::find_text(&doc));
                return self.refresh_find(cx);
            }
            let text = cx.background_spawn(async move { engine::find_text(&doc) });
            self.find_task = Some(cx.spawn(async move |this, cx| {
                let text = text.await;
                let _ = this.update(cx, |p, cx| {
                    p.find_text = Some(text);
                    p.find_task = None;
                    p.refresh_find(cx);
                });
            }));
            return;
        };
        let query = self.bar.read(cx).query(cx);
        let found = collect_preview_find_ranges(&text.blocks, &query, self.bar.read(cx).options());
        self.matches = found
            .ranges
            .iter()
            .map(|(page, r)| (page + 1, text.rects(*page, r.clone())))
            .collect();
        let n = self.matches.len();
        if !(1..=n).contains(&self.index) {
            self.index = usize::from(n > 0);
        }
        let (index, capped, valid) = (self.index, found.capped, found.valid);
        self.bar.update(cx, |b, cx| {
            if query.is_empty() {
                b.reset_counts();
                cx.notify();
            } else {
                b.set_results(index, n, capped, valid, cx);
            }
        });
        self.reveal_match(cx);
    }

    fn step_find(&mut self, dir: i64, cx: &mut Context<Self>) {
        let n = self.matches.len() as i64;
        if !self.find_open || n == 0 {
            return;
        }
        self.index = ((self.index as i64 - 1 + dir).rem_euclid(n) + 1) as usize;
        self.refresh_find(cx);
    }

    /// Scroll the current match to the viewport's middle.
    // simplify: vertical only; a match outside a zoomed page's visible width stays off screen
    // horizontally (scroll x by the slot's centered left plus `r.x * w` to add it).
    fn reveal_match(&mut self, cx: &mut Context<Self>) {
        let Some((page, rects)) = self.index.checked_sub(1).and_then(|i| self.matches.get(i))
        else {
            cx.notify();
            return;
        };
        let (Some(r), Some(vp), Some((_, h))) =
            (rects.first(), self.viewport, self.sizes.get(page - 1))
        else {
            cx.notify();
            return;
        };
        let h = h * self.scale;
        let mid = self.slot_top(*page) + (r.y + r.h / 2.) * h;
        let max = f32::from(self.handle.max_offset().y).max(0.);
        let top = (mid - f32::from(vp.size.height) / 2.).clamp(0., max);
        let x = self.handle.offset().x;
        self.handle.set_offset(point(x, px(-top)));
        cx.notify();
    }

    /// ⌘F opens find, Escape closes it, ⌘G / ⇧⌘G / F3 step; ⌘+ / ⌘- zoom.
    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending || !self.ready() {
            return;
        }
        let k = &e.keystroke;
        let m = k.modifiers;
        let cmd = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.control
        };
        if k.key == "escape" && self.find_open {
            cx.stop_propagation();
            self.close_search(window, cx);
            return;
        }
        if cmd && !m.alt && !m.shift && k.key.eq_ignore_ascii_case("f") {
            cx.stop_propagation();
            self.open_search(window, cx);
            return;
        }
        if self.find_open && ((cmd && !m.alt && k.key.eq_ignore_ascii_case("g")) || k.key == "f3") {
            cx.stop_propagation();
            self.step_find(if m.shift { -1 } else { 1 }, cx);
            return;
        }
        if !(m.platform || m.control) {
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

    /// `.obs-pdf-page-slot`: white, a 1px `--line` ring outside, the canvas stretched over it,
    /// find matches over the canvas.
    fn slot(&self, n: usize, w: f32, h: f32, t: &recur_ui::Tokens) -> AnyElement {
        let marks = self
            .matches
            .iter()
            .enumerate()
            .filter(|(_, (page, _))| *page == n)
            .flat_map(|(i, (_, rects))| {
                let fill = t.amber.fade(if i + 1 == self.index { 0.6 } else { 0.3 });
                rects.iter().map(move |r| {
                    div()
                        .absolute()
                        .left(px(r.x * w))
                        .top(px(r.y * h))
                        .w(px(r.w * w))
                        .h(px(r.h * h))
                        .bg(fill)
                })
            });
        let image = self.slots.get(&n).and_then(|s| s.painted.clone());
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
                    .border_color(t.line),
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
            .children(marks)
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
                    .map(|(i, (w, h))| self.slot(i + 1, w * self.scale, h * self.scale, &t)),
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
            .when(self.find_open && self.ready(), |d| {
                d.child(self.bar.clone())
            })
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
