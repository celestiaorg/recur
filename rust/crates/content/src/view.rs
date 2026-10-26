//! `MarkdownHtml.svelte`: settled markdown as a native view. Rendered synchronously through
//! the shared cache (tab-swap scroll restore needs real heights on the first frame), with
//! copy controls on fences, Mermaid placeholders hydrated into diagrams, and images that open
//! the lightbox.

use recur_ui::guard::Guarded;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Task, WeakEntity, Window, canvas, div, px,
};
use recur_ui::Theme;

use crate::images::{self, Decoded, ImageCache};
use crate::lightbox::AttachmentLightbox;
use crate::markdown::{ParseOptions, markdown_doc_cached, markdown_doc_is_cached};
use crate::render::block::{
    Build, Counters, Env, Handlers, ImageSlot, Item, Laid, MermaidSlot, TextBlock, items, stack,
};
use crate::render::doc::{Doc, El, Node};
use crate::render::mirror::Mirror;
use crate::render::style::{MarkdownClass, Sheet};

/// What a markdown view reports to its host.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkdownEvent {
    /// The lightbox opened or closed (`setChatOccluder('attachment-lightbox', …)`).
    LightboxOpen(bool),
    /// A toast the host should show (`pushToast`).
    Toast { error: bool, message: String },
    /// A render changed the text blocks (what a `MutationObserver` on the rendered tree sees).
    TextChanged,
}

pub struct MarkdownView {
    text: SharedString,
    /// `MarkdownHtml` enhancements (copy controls, Mermaid, the lightbox); off for streaming.
    enhance: bool,
    class: MarkdownClass,
    sheet: Option<Sheet>,
    demote_images: bool,
    pub(crate) doc: Arc<Doc>,
    fences: Vec<String>,
    hover_fence: Option<usize>,
    copied: HashSet<usize>,
    copy_timers: HashMap<usize, Task<()>>,
    images: HashMap<String, ImageSlot>,
    loading: HashMap<String, Task<()>>,
    pub(crate) mermaid: Vec<MermaidSlot>,
    mermaid_task: Option<Task<()>>,
    /// A large document rendering off the UI thread.
    render_task: Option<Task<()>>,
    width: f32,
    /// The window's device scale, from the last frame.
    scale: Option<f32>,
    hover_link: Option<(usize, usize)>,
    highlights: HashMap<usize, Vec<(std::ops::Range<usize>, bool)>>,
    pub(crate) texts: Rc<RefCell<Vec<TextBlock>>>,
    lightbox: Option<Entity<AttachmentLightbox>>,
    focus: FocusHandle,
    /// The scroller a long document windows itself to (a file preview's).
    scroller: Option<ScrollHandle>,
    windowed: Option<Windowed>,
    /// The document's top in its scroller's content, from the last frame.
    content_y: Rc<Cell<Option<f32>>>,
    /// The span of the document the last render laid out, in body coordinates.
    built: Rc<Cell<(f32, f32)>>,
}

/// Top-level blocks before a document windows itself to its scroller.
const WINDOW_MIN_BLOCKS: usize = 48;

/// A long document's index: what laying out a top-level block needs without building the
/// ones before it, and their heights.
struct Windowed {
    segs: Vec<Seg>,
    /// Every text block of the document (find reads them all).
    texts: Vec<TextBlock>,
    /// Laid-out heights by segment, at `width`.
    heights: Rc<RefCell<Vec<Option<f32>>>>,
    width: f32,
}

/// A top-level block that lays out.
struct Seg {
    item: usize,
    n: Counters,
    mt: f32,
    mb: f32,
    chars: usize,
    lines: usize,
}

impl Seg {
    /// A height for a block not laid out yet; measured once it is.
    fn estimate(&self, size: f32, leading: f32, width: f32) -> f32 {
        let per_line = (width / (size * 0.5)).max(1.);
        let lines = self.lines.max((self.chars as f32 / per_line).ceil() as usize);
        lines.max(1) as f32 * size * leading
    }
}

/// Text length and hard line count of a block (for its estimate).
fn extent(it: &Item) -> (usize, usize) {
    fn el(e: &El, pre: bool, out: &mut (usize, usize)) {
        let pre = pre || e.is("pre");
        if e.is("li") || e.is("tr") || e.is("p") || e.is("br") {
            out.1 += 1;
        }
        walk(&e.children, pre, out);
    }
    fn walk(nodes: &[Node], pre: bool, out: &mut (usize, usize)) {
        for n in nodes {
            match n {
                Node::Text(t) => {
                    out.0 += t.len();
                    if pre {
                        out.1 += t.matches('\n').count();
                    }
                }
                Node::El(e) => el(e, pre, out),
            }
        }
    }
    let mut out = (0, 0);
    match it {
        Item::Block(e, _) => el(e, false, &mut out),
        Item::Inline(run) => walk(run, false, &mut out),
    }
    out
}

impl EventEmitter<MarkdownEvent> for MarkdownView {}

impl Focusable for MarkdownView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `codeText`: a fence's text without its trailing newline.
fn code_text(pre: &El) -> String {
    let code = pre.children.iter().find_map(|n| match n {
        Node::El(c) if c.is("code") => Some(c.text_content()),
        _ => None,
    });
    let t = code.unwrap_or_else(|| pre.text_content());
    t.strip_suffix('\n').map(str::to_string).unwrap_or(t)
}

/// The copy texts of fences in render order (`enhanceCodeCopy`).
fn fence_texts(doc: &Doc) -> Vec<String> {
    let mut out = Vec::new();
    fn go(nodes: &[Node], out: &mut Vec<String>) {
        for n in nodes {
            let Node::El(e) = n else { continue };
            if e.has_class("md-mermaid") {
                continue;
            }
            if e.has_class("mdref") {
                let pre = e.children.iter().find_map(|n| match n {
                    Node::El(c) if c.is("pre") => Some(c),
                    _ => None,
                });
                out.push(pre.map(code_text).unwrap_or_default());
                continue;
            }
            if e.is("pre") {
                out.push(code_text(e));
                continue;
            }
            go(&e.children, out);
        }
    }
    go(&doc.nodes, &mut out);
    out
}

/// Mermaid sources in document order.
pub(crate) fn mermaid_sources(doc: &Doc) -> Vec<String> {
    let mut out = Vec::new();
    doc.walk(&mut |e| {
        if e.has_class("md-mermaid") {
            out.push(e.text_content());
        }
    });
    out
}

/// Sources past this render on a background thread instead of inside `set_text`.
const ASYNC_RENDER_MIN_BYTES: usize = 16 * 1024;

impl MarkdownView {
    pub fn new(
        text: impl Into<SharedString>,
        class: MarkdownClass,
        demote_images: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut v = MarkdownView {
            text: SharedString::default(),
            enhance: true,
            class,
            sheet: None,
            demote_images,
            doc: Arc::default(),
            fences: Vec::new(),
            hover_fence: None,
            copied: HashSet::new(),
            copy_timers: HashMap::new(),
            images: HashMap::new(),
            loading: HashMap::new(),
            mermaid: Vec::new(),
            mermaid_task: None,
            render_task: None,
            width: 0.,
            scale: None,
            hover_link: None,
            highlights: HashMap::new(),
            texts: Rc::default(),
            lightbox: None,
            focus: cx.focus_handle(),
            scroller: None,
            windowed: None,
            content_y: Rc::default(),
            built: Rc::default(),
        };
        v.set_text(text, cx);
        cx.observe_global::<Theme>(|v, cx| {
            // Mermaid theme variables track the app theme on the next hydrate.
            if v.enhance {
                v.hydrate_mermaid(cx);
            }
            cx.notify();
        })
        .detach();
        v
    }

    /// A bare `.chat-canvas` for HTML rendered elsewhere (`StreamingMarkdown`): no copy
    /// controls, Mermaid stays source, images do not open the lightbox.
    pub fn plain(class: MarkdownClass, cx: &mut Context<Self>) -> Self {
        let mut v = MarkdownView::new("", class, false, cx);
        v.enhance = false;
        v
    }

    /// Replaces the class's sheet (a host's cascade on top of `.chat-canvas`).
    pub fn set_sheet(&mut self, sheet: Sheet, cx: &mut Context<Self>) {
        self.sheet = Some(sheet);
        self.windowed = None;
        cx.notify();
    }

    /// Lays a long document out only near `scroller`'s viewport, which scrolls this view.
    pub fn set_scroller(&mut self, scroller: ScrollHandle) {
        self.scroller = Some(scroller);
    }

    /// Where text block `id` sits in the scroller's content, laid out or not.
    pub fn text_top(&self, id: usize) -> Option<f32> {
        let w = self.windowed.as_ref()?;
        let seg = w.segs.iter().rposition(|s| s.n.text < id)?;
        let tops = self.tops(w, self.sheet_or_default().size, self.sheet_or_default().leading);
        Some(tops[seg] + self.content_y.get().unwrap_or(0.))
    }

    fn sheet_or_default(&self) -> Sheet {
        self.sheet
            .clone()
            .unwrap_or_else(|| Sheet::for_class(self.class))
    }

    /// Each segment's top in body coordinates, and the end of the last.
    fn tops(&self, w: &Windowed, size: f32, leading: f32) -> Vec<f32> {
        let heights = w.heights.borrow();
        let mut tops = Vec::with_capacity(w.segs.len() + 1);
        let mut y = 0.;
        for (i, s) in w.segs.iter().enumerate() {
            tops.push(y);
            y += heights[i].unwrap_or_else(|| s.estimate(size, leading, w.width));
            if let Some(next) = w.segs.get(i + 1) {
                y += s.mb.max(next.mt);
            }
        }
        tops.push(y);
        tops
    }

    /// The scroller's viewport in body coordinates.
    fn visible(scroller: &ScrollHandle, content_y: f32, window: &Window) -> (f32, f32) {
        let vp = scroller.bounds();
        let h = if vp.size.height > px(0.) {
            vp.size.height
        } else {
            window.viewport_size().height
        };
        let top = -f32::from(scroller.offset().y) - content_y;
        (top, top + f32::from(h))
    }

    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// Replace the markdown (`text` prop).
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        if text == self.text && !self.doc.nodes.is_empty() {
            return;
        }
        self.text = text;
        self.render_task = None;
        let opts = ParseOptions {
            demote_images: self.demote_images,
        };
        if self.text.len() > ASYNC_RENDER_MIN_BYTES && !markdown_doc_is_cached(&self.text, opts) {
            // A large file preview parses for long enough to freeze the window; the previous
            // document stays up until it lands.
            let src = self.text.clone();
            let render = cx
                .background_executor()
                .spawn(async move { markdown_doc_cached(&src, opts) });
            self.render_task = Some(cx.spawn(async move |this, cx| {
                let doc = render.await;
                let _ = this.update(cx, |v, cx| {
                    v.render_task = None;
                    v.set_doc(doc, cx);
                });
            }));
            return;
        }
        let doc = markdown_doc_cached(&self.text, opts);
        self.set_doc(doc, cx);
    }

    pub(crate) fn set_html(&mut self, html: &str, cx: &mut Context<Self>) {
        self.set_doc(Arc::new(Doc::parse(html)), cx);
    }

    fn set_doc(&mut self, doc: Arc<Doc>, cx: &mut Context<Self>) {
        self.doc = doc;
        self.windowed = None;
        self.fences = fence_texts(&self.doc);
        self.hover_fence = None;
        self.copied.clear();
        self.copy_timers.clear();
        self.load_images(cx);
        if self.enhance {
            self.hydrate_mermaid(cx);
        } else {
            self.mermaid.clear();
        }
        cx.notify();
    }

    fn load_images(&mut self, cx: &mut Context<Self>) {
        let mut srcs = Vec::new();
        self.doc.walk(&mut |e| {
            if e.is("img")
                && let Some(s) = e.attr("src")
            {
                srcs.push(s.to_string());
            }
        });
        for src in srcs {
            if self.images.contains_key(&src) {
                continue;
            }
            if let Some(hit) = cx.try_global::<ImageCache>().and_then(|c| c.0.get(&src)) {
                self.images.insert(src, ready(hit));
                continue;
            }
            let Some(fut) = images::fetch(&src, cx) else {
                self.images.insert(src, ImageSlot::Failed);
                continue;
            };
            self.images.insert(src.clone(), ImageSlot::Loading);
            let key = src.clone();
            let task = cx.spawn(async move |this, cx| {
                let res = cx.background_spawn(fut).await;
                let _ = this.update(cx, |v, cx| {
                    v.loading.remove(&key);
                    match res {
                        Ok(d) => {
                            cx.default_global::<ImageCache>()
                                .0
                                .insert(key.clone(), d.clone());
                            v.images.insert(key, ready(&d));
                        }
                        Err(_) => {
                            v.images.insert(key, ImageSlot::Failed);
                        }
                    }
                    cx.notify();
                });
            });
            self.loading.insert(src, task);
        }
    }

    /// `hydrateMermaid`: render every placeholder; the previous pass is cancelled.
    fn hydrate_mermaid(&mut self, cx: &mut Context<Self>) {
        self.windowed = None;
        let sources = mermaid_sources(&self.doc);
        self.mermaid = vec![MermaidSlot::Pending; sources.len()];
        self.mermaid_task = None;
        // Mermaid lays out per device scale, known from the first frame on.
        let Some(scale) = self.scale.filter(|_| !sources.is_empty()) else {
            return;
        };
        match crate::mermaid::hydrate(sources, scale, cx) {
            crate::mermaid::Hydration::Done(slots) => self.mermaid = slots,
            crate::mermaid::Hydration::Running(task) => self.mermaid_task = Some(task),
        }
    }

    pub(crate) fn set_mermaid(&mut self, i: usize, slot: MermaidSlot, cx: &mut Context<Self>) {
        if let Some(s) = self.mermaid.get_mut(i) {
            *s = slot;
            self.windowed = None;
            cx.notify();
        }
    }

    fn copy_fence(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(text) = self.fences.get(i).filter(|t| !t.is_empty()).cloned() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied.insert(i);
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |v, cx| {
                v.copied.remove(&i);
                v.copy_timers.remove(&i);
                cx.notify();
            });
        });
        self.copy_timers.insert(i, task);
        cx.notify();
    }

    fn open_image(
        &mut self,
        src: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let decoded = cx
            .try_global::<ImageCache>()
            .and_then(|c| c.0.get(&src))
            .cloned();
        let this = cx.entity().downgrade();
        let lb = cx.new(|cx| {
            AttachmentLightbox::new(
                src,
                name,
                decoded,
                Rc::new(move |_, cx: &mut App| {
                    let _ = this.update(cx, |v, cx| {
                        v.lightbox = None;
                        cx.emit(MarkdownEvent::LightboxOpen(false));
                        cx.notify();
                    });
                }),
                window,
                cx,
            )
        });
        cx.guarded_subscribe(&lb, |_, _, ev: &crate::lightbox::LightboxEvent, cx| {
            let crate::lightbox::LightboxEvent::Toast { error, message } = ev;
            cx.emit(MarkdownEvent::Toast {
                error: *error,
                message: message.clone(),
            });
        })
        .detach();
        self.lightbox = Some(lb);
        cx.emit(MarkdownEvent::LightboxOpen(true));
        cx.notify();
    }
}

impl MarkdownView {
    /// The link under a window position: (text block, link index, href).
    fn link_at(
        &self,
        p: gpui_kit::Point<gpui_kit::Pixels>,
        window: &Window,
    ) -> Option<(usize, usize, String)> {
        let texts = self.texts.borrow();
        for tb in texts.iter() {
            if tb.links.is_empty() {
                continue;
            }
            let Some(b) = tb.bounds.get() else { continue };
            if !b.contains(&p) {
                continue;
            }
            let m = Mirror::layout(&tb.spec, f32::from(b.size.width), window);
            let byte = m.hit(f32::from(p.x - b.origin.x), f32::from(p.y - b.origin.y))?;
            let (_, l) = tb.links.iter().find(|(r, _)| r.contains(&byte))?;
            return Some((tb.id, *l, tb.hrefs.get(*l)?.clone()));
        }
        None
    }

    /// Find highlights per text block (`::highlight(preview-find)`), the current one flagged.
    pub fn set_highlights(
        &mut self,
        highlights: HashMap<usize, Vec<(std::ops::Range<usize>, bool)>>,
        cx: &mut Context<Self>,
    ) {
        if highlights != self.highlights {
            self.highlights = highlights;
            cx.notify();
        }
    }

    /// Every rendered text block with its id and content, in document order.
    /// Find highlights by text block: `(range, current)`.
    pub fn highlights(&self) -> &HashMap<usize, Vec<(std::ops::Range<usize>, bool)>> {
        &self.highlights
    }

    pub fn text_blocks(&self) -> Vec<(usize, String)> {
        self.texts
            .borrow()
            .iter()
            .map(|t| (t.id, t.spec.runs.iter().map(|r| r.text.as_str()).collect()))
            .collect()
    }

    /// Where a range of a text block painted last frame (window coordinates).
    pub fn range_bounds(
        &self,
        id: usize,
        range: std::ops::Range<usize>,
        window: &Window,
    ) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        let texts = self.texts.borrow();
        let tb = texts.iter().find(|t| t.id == id)?;
        let b = tb.bounds.get()?;
        let m = Mirror::layout(&tb.spec, f32::from(b.size.width), window);
        let rects = m.rects(range);
        let (x, y, w, h) = *rects.first()?;
        Some(gpui_kit::Bounds {
            origin: gpui_kit::point(b.origin.x + gpui_kit::px(x), b.origin.y + gpui_kit::px(y)),
            size: gpui_kit::size(gpui_kit::px(w), gpui_kit::px(h)),
        })
    }
}

fn ready(d: &Decoded) -> ImageSlot {
    ImageSlot::Ready {
        image: d.image.clone(),
        width: d.width,
        height: d.height,
    }
}

fn handlers(this: WeakEntity<MarkdownView>) -> Handlers {
    let h = this.clone();
    let c = this.clone();
    let o = this.clone();
    Handlers {
        fence_hover: Rc::new(move |(i, on), _, cx| {
            let _ = h.update(cx, |v, cx| {
                let next = if on {
                    Some(i)
                } else if v.hover_fence == Some(i) {
                    None
                } else {
                    v.hover_fence
                };
                if next != v.hover_fence {
                    v.hover_fence = next;
                    cx.notify();
                }
            });
        }),
        copy_fence: Rc::new(move |i, _, cx| {
            let _ = c.update(cx, |v, cx| v.copy_fence(i, cx));
        }),
        link_hover: Rc::new(|_, _, _| {}),
        open_link: Rc::new(|href, _, cx| crate::links::open_external_url(&href, cx)),
        open_image: Rc::new(move |(src, name), window, cx| {
            let _ = o.update(cx, |v, cx| v.open_image(src, name, window, cx));
        }),
    }
}

impl MarkdownView {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        if self.scale != Some(window.scale_factor()) {
            self.scale = Some(window.scale_factor());
            if self.enhance {
                self.hydrate_mermaid(cx);
            }
        }
        let sheet = self
            .sheet
            .clone()
            .unwrap_or_else(|| Sheet::for_class(self.class));
        let mut tokens = theme.tokens;
        if sheet.muted {
            tokens.text = tokens.muted;
        }
        let images = &self.images;
        let image = |src: &str| images.get(src).cloned().unwrap_or(ImageSlot::Failed);
        let handlers = handlers(cx.entity().downgrade());
        let width = if self.width > 0. {
            self.width
        } else {
            f32::from(window.viewport_size().width)
        };
        let width = sheet.max_width.map_or(width, |m| width.min(m));
        let env = Env {
            sheet: &sheet,
            t: &tokens,
            width,
            image: &image,
            mermaid: &self.mermaid,
            enhance: self.enhance,
            hover_fence: self.hover_fence,
            copied: &self.copied,
            hover_link: self.hover_link,
            highlights: &self.highlights,
            focused_image: None,
            handlers: &handlers,
        };
        let flow = crate::render::block::Flow {
            size: sheet.size,
            leading: sheet.leading,
            ink: crate::render::inline::Ink::Text,
            weight: sheet.weight,
            lists: 0,
        };
        let doc = self.doc.clone();
        let mut b = Build {
            env: &env,
            window,
            n: Counters::default(),
            texts: Vec::new(),
            dry: false,
        };
        let items = items(&doc.nodes, "root");
        let scroller = self
            .scroller
            .clone()
            .filter(|_| items.len() >= WINDOW_MIN_BLOCKS);
        let (blocks, texts) = match scroller {
            None => {
                self.windowed = None;
                let blocks = items.iter().filter_map(|it| b.item(it, flow)).collect();
                (blocks, std::mem::take(&mut b.texts))
            }
            Some(scroller) => {
                if self.windowed.is_none() {
                    self.windowed = Some(index(&mut b, &items, flow, width));
                }
                let content_y = self.content_y.get().unwrap_or(0.);
                let visible = Self::visible(&scroller, content_y, b.window);
                let w = self.windowed.as_mut().expect("indexed");
                if (w.width - width).abs() > 0.01 {
                    w.width = width;
                    w.heights.borrow_mut().iter_mut().for_each(|h| *h = None);
                }
                let w = self.windowed.as_ref().expect("indexed");
                let tops = self.tops(w, flow.size, flow.leading);
                let (blocks, built) = windowed_blocks(&mut b, w, &items, &tops, visible, flow);
                self.built.set(built);
                let mut texts = w.texts.clone();
                for t in std::mem::take(&mut b.texts) {
                    if let Ok(i) = texts.binary_search_by_key(&t.id, |x| x.id) {
                        texts[i] = t;
                    }
                }
                (blocks, texts)
            }
        };
        let (body, first, last) = stack(blocks, b.window);
        let changed = {
            let old = self.texts.borrow();
            let content = |t: &TextBlock| t.spec.runs.iter().map(|r| r.text.as_str()).collect::<String>();
            old.len() != texts.len()
                || old
                    .iter()
                    .zip(&texts)
                    .any(|(a, b)| a.id != b.id || content(a) != content(b))
        };
        *self.texts.borrow_mut() = texts;
        if changed {
            cx.emit(MarkdownEvent::TextChanged);
        }
        let (first, last) = if sheet.collapse_edges {
            (0., 0.)
        } else {
            (first, last)
        };
        let gap_top = crate::render::exact::vgap(first, b.window);
        let gap_bottom = crate::render::exact::vgap(last, b.window);
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, window, cx| {
                let w = f32::from(bounds.size.width);
                let _ = this.update(cx, |v, cx| {
                    if (v.width - w).abs() > 0.01 {
                        v.width = w;
                        cx.notify();
                    }
                    if v.windowed.is_some()
                        && let Some(s) = v.scroller.as_ref()
                    {
                        // Lay out again when the viewport left what this frame built (a
                        // restored or revealed offset, a jump).
                        let y = f32::from(bounds.origin.y - s.bounds().origin.y - s.offset().y);
                        let moved = v.content_y.get().is_none_or(|old| (old - y).abs() > 1.);
                        v.content_y.set(Some(y));
                        let (top, bottom) = Self::visible(s, y, window);
                        let (a, z) = v.built.get();
                        if moved || top < a || bottom > z {
                            cx.notify();
                        }
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        let content = div()
            .relative()
            .flex()
            .flex_col()
            .min_w_0()
            .text_color(tokens.text)
            .child(measure)
            .child(gap_top)
            .child(body)
            .child(gap_bottom);
        let root = div()
            .id("markdown")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .min_w_0()
            .when(self.hover_link.is_some(), |d| d.cursor_pointer())
            .on_mouse_move(
                cx.guarded_listener(|v, e: &gpui_kit::MouseMoveEvent, window, cx| {
                    let hit = v.link_at(e.position, window).map(|(id, l, _)| (id, l));
                    if hit != v.hover_link {
                        v.hover_link = hit;
                        cx.notify();
                    }
                }),
            )
            .on_hover(cx.guarded_listener(|v, hovered: &bool, _, cx| {
                if !hovered && v.hover_link.is_some() {
                    v.hover_link = None;
                    cx.notify();
                }
            }))
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.guarded_listener(|v, e: &gpui_kit::MouseUpEvent, window, cx| {
                    if e.click_count == 1
                        && let Some((_, _, href)) = v.link_at(e.position, window)
                    {
                        crate::links::open_external_url(&href, cx);
                    }
                }),
            )
            .on_mouse_up(
                gpui_kit::MouseButton::Middle,
                cx.guarded_listener(|v, e: &gpui_kit::MouseUpEvent, window, cx| {
                    if let Some((_, _, href)) = v.link_at(e.position, window) {
                        crate::links::open_external_url(&href, cx);
                    }
                }),
            );
        let root = match sheet.max_width {
            Some(m) => root
                .items_center()
                .child(div().w_full().max_w(gpui_kit::px(m)).child(content)),
            None => root.child(content),
        };
        root.children(self.lightbox.clone())
    }
}

impl Render for MarkdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AppContext, Entity, IntoElement, ParentElement, Render, Styled, div, px};

    use super::*;
    use crate::render::mirror::Mirror;

    struct Column(Entity<MarkdownView>);

    impl Column {
        fn render_view(
            &mut self,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> impl IntoElement + use<> {
            div().w(px(320.)).child(self.0.clone())
        }
    }

    impl Render for Column {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            recur_ui::guard::view(self, window, cx, Self::render_view)
        }
    }

    /// Hit-testing and find rects come from the mirror, so its lines must be the ones the
    /// text element painted: same breaks, same line boxes.
    #[gpui::test]
    fn mirror_lines_match_the_painted_text(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            recur_ui::init(cx);
        });
        let md = include_str!("../../../tools/ui-parity/content/chat.md");
        let (column, cx) = cx.add_window_view(|_, cx| {
            Column(cx.new(|cx| MarkdownView::new(md, MarkdownClass::ChatCanvas, false, cx)))
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let view = column.read(cx).0.read(cx);
            let texts = view.texts.borrow();
            assert!(texts.len() > 10);
            let mut wrapped = 0;
            for t in texts.iter() {
                let b = t.bounds.get().expect("painted");
                let m = Mirror::layout(&t.spec, f32::from(b.size.width), window);
                let last = m.lines.last().expect("a line");
                let content: String = t.spec.runs.iter().map(|r| r.text.as_str()).collect();
                // Painted bounds are snapped to device pixels; a line is ~20px, so any break
                // that differs shows.
                assert!(
                    (last.top + last.height - f32::from(b.size.height)).abs()
                        <= 1. / window.scale_factor(),
                    "{content:?}: mirror {} vs painted {}",
                    last.top + last.height,
                    f32::from(b.size.height)
                );
                wrapped += usize::from(m.lines.len() > 1);
            }
            assert!(wrapped > 0, "the column is narrow enough to wrap");
        });
    }

    struct Scrolled(Entity<MarkdownView>, gpui_kit::ScrollHandle);

    impl Scrolled {
        fn render_view(
            &mut self,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> impl IntoElement + use<> {
            div()
                .id("scroller")
                .track_scroll(&self.1)
                .overflow_scroll()
                .w(px(320.))
                .h(px(400.))
                .flex()
                .flex_col()
                .child(self.0.clone())
        }
    }

    impl Render for Scrolled {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            recur_ui::guard::view(self, window, cx, Self::render_view)
        }
    }

    /// A long document in a scroller lays out the blocks near the viewport, keeps every text
    /// block for find, and lays out the end once scrolled there.
    #[gpui::test]
    fn a_long_document_lays_out_only_near_its_viewport(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            recur_ui::init(cx);
        });
        let md: String = (0..300)
            .map(|i| format!("Paragraph {i} has a few words in it.\n\n"))
            .collect();
        let handle = gpui_kit::ScrollHandle::new();
        let h = handle.clone();
        let (root, cx) = cx.add_window_view(|_, cx| {
            let v = cx.new(|cx| {
                let mut v = MarkdownView::new(md, MarkdownClass::MdDoc, false, cx);
                v.set_scroller(h);
                v
            });
            Scrolled(v, handle.clone())
        });
        cx.run_until_parked();
        let painted = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                let v = root.read(cx).0.read(cx);
                let texts = v.texts.borrow();
                assert_eq!(texts.len(), 300, "find sees every block");
                texts
                    .iter()
                    .filter(|t| t.bounds.get().is_some())
                    .map(|t| t.id)
                    .collect::<Vec<_>>()
            })
        };
        let first = painted(cx);
        assert!(first.contains(&1), "{first:?}");
        assert!(first.len() < 100, "{} blocks laid out", first.len());
        let max = handle.max_offset().y;
        assert!(max > px(4000.), "spacers keep the full height: {max:?}");
        handle.set_offset(gpui_kit::point(px(0.), -max));
        let md = cx.update(|_, cx| root.read(cx).0.clone());
        cx.update(|_, cx| md.update(cx, |_, cx| cx.notify()));
        cx.run_until_parked();
        let last = painted(cx);
        assert!(last.contains(&300), "{last:?}");
        assert!(!last.contains(&1));
    }
}

/// The document's index: every top-level block built once, without layout, for its counters,
/// margins and text blocks.
fn index(b: &mut Build, items: &[Item], flow: crate::render::block::Flow, width: f32) -> Windowed {
    b.dry = true;
    let mut segs = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let n = b.n.clone();
        if let Some(l) = b.item(it, flow) {
            let (chars, lines) = extent(it);
            segs.push(Seg {
                item: i,
                n,
                mt: l.mt,
                mb: l.mb,
                chars,
                lines,
            });
        }
    }
    b.dry = false;
    b.n = Counters::default();
    let heights = Rc::new(RefCell::new(vec![None; segs.len()]));
    Windowed {
        segs,
        texts: std::mem::take(&mut b.texts),
        heights,
        width,
    }
}

/// The blocks near `visible` (a viewport of overscan either side), the rest as spacers at
/// their measured or estimated heights. Returns them and the span they cover.
fn windowed_blocks(
    b: &mut Build,
    w: &Windowed,
    items: &[Item],
    tops: &[f32],
    visible: (f32, f32),
    flow: crate::render::block::Flow,
) -> (Vec<Laid>, (f32, f32)) {
    let n = w.segs.len();
    if n == 0 {
        return (Vec::new(), (f32::NEG_INFINITY, f32::INFINITY));
    }
    let pad = visible.1 - visible.0;
    let (lo, hi) = (visible.0 - pad, visible.1 + pad);
    let a = (0..n).find(|&i| tops[i + 1] >= lo).unwrap_or(n.saturating_sub(1));
    let z = (a..n).take_while(|&i| tops[i] <= hi).last().unwrap_or(a);
    let gap = |i: usize| w.segs.get(i + 1).map_or(0., |next| w.segs[i].mb.max(next.mt));
    let spacer = |from: usize, to: usize| Laid {
        el: div()
            .flex_none()
            .h(px(tops[to + 1] - tops[from] - gap(to)))
            .into_any_element(),
        mt: w.segs[from].mt,
        mb: w.segs[to].mb,
    };
    let mut out = Vec::new();
    if a > 0 {
        out.push(spacer(0, a - 1));
    }
    for i in a..=z {
        let seg = &w.segs[i];
        b.n = seg.n.clone();
        let Some(l) = b.item(&items[seg.item], flow) else {
            continue;
        };
        let heights = w.heights.clone();
        let probe = canvas(
            move |bounds, _, _| {
                if let Some(h) = heights.borrow_mut().get_mut(i) {
                    *h = Some(f32::from(bounds.size.height));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        out.push(Laid {
            el: div()
                .relative()
                .flex()
                .flex_col()
                .w_full()
                .child(l.el)
                .child(probe)
                .into_any_element(),
            mt: l.mt,
            mb: l.mb,
        });
    }
    if z + 1 < n {
        out.push(spacer(z + 1, n - 1));
    }
    let top = if a == 0 { f32::NEG_INFINITY } else { tops[a] };
    let bottom = if z + 1 >= n { f32::INFINITY } else { tops[z + 1] };
    (out, (top, bottom))
}
