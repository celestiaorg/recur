//! `MarkdownHtml.svelte`: settled markdown as a native view. Rendered synchronously through
//! the shared cache (tab-swap scroll restore needs real heights on the first frame), with
//! copy controls on fences, Mermaid placeholders hydrated into diagrams, and images that open
//! the lightbox.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Task, WeakEntity,
    Window, canvas, div,
};
use recur_ui::Theme;

use crate::images::{self, Decoded, ImageCache};
use crate::lightbox::AttachmentLightbox;
use crate::markdown::{ParseOptions, render_markdown_cached};
use crate::render::block::{Build, Counters, Env, Handlers, ImageSlot, MermaidSlot, stack};
use crate::render::doc::{Doc, El, Node};
use crate::render::style::{MarkdownClass, Sheet};

/// What a markdown view reports to its host.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkdownEvent {
    /// The lightbox opened or closed (`setChatOccluder('attachment-lightbox', …)`).
    LightboxOpen(bool),
    /// A toast the host should show (`pushToast`).
    Toast { error: bool, message: String },
}

pub struct MarkdownView {
    text: SharedString,
    class: MarkdownClass,
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
    width: f32,
    lightbox: Option<Entity<AttachmentLightbox>>,
    focus: FocusHandle,
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

impl MarkdownView {
    pub fn new(
        text: impl Into<SharedString>,
        class: MarkdownClass,
        demote_images: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut v = MarkdownView {
            text: SharedString::default(),
            class,
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
            width: 0.,
            lightbox: None,
            focus: cx.focus_handle(),
        };
        v.set_text(text, cx);
        cx.observe_global::<Theme>(|v, cx| {
            // Mermaid theme variables track the app theme on the next hydrate.
            v.hydrate_mermaid(cx);
            cx.notify();
        })
        .detach();
        v
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
        let html = if self.text.is_empty() {
            Arc::from("")
        } else {
            render_markdown_cached(
                &self.text,
                ParseOptions {
                    demote_images: self.demote_images,
                },
            )
        };
        self.set_html(&html, cx);
    }

    pub(crate) fn set_html(&mut self, html: &str, cx: &mut Context<Self>) {
        self.doc = Arc::new(Doc::parse(html));
        self.fences = fence_texts(&self.doc);
        self.hover_fence = None;
        self.copied.clear();
        self.copy_timers.clear();
        self.load_images(cx);
        self.hydrate_mermaid(cx);
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
        let sources = mermaid_sources(&self.doc);
        self.mermaid = vec![MermaidSlot::Pending; sources.len()];
        self.mermaid_task = None;
        if sources.is_empty() {
            return;
        }
        self.mermaid_task = Some(crate::mermaid::hydrate(sources, cx));
    }

    pub(crate) fn set_mermaid(&mut self, i: usize, slot: MermaidSlot, cx: &mut Context<Self>) {
        if let Some(s) = self.mermaid.get_mut(i) {
            *s = slot;
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
        cx.subscribe(&lb, |_, _, ev: &crate::lightbox::LightboxEvent, cx| {
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

impl Render for MarkdownView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let sheet = Sheet::for_class(self.class);
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
            t: &theme.tokens,
            width,
            image: &image,
            mermaid: &self.mermaid,
            hover_fence: self.hover_fence,
            copied: &self.copied,
            hover_link: None,
            focused_image: None,
            handlers: &handlers,
        };
        let flow = crate::render::block::Flow {
            size: sheet.size,
            leading: sheet.leading,
            ink: crate::render::inline::Ink::Text,
            weight: 400.,
            lists: 0,
        };
        let doc = self.doc.clone();
        let mut b = Build {
            env: &env,
            window,
            n: Counters::default(),
        };
        let blocks = b.blocks(&doc.nodes, flow, "root");
        let (body, first, last) = stack(blocks, b.window);
        let gap_top = crate::render::exact::vgap(first, b.window);
        let gap_bottom = crate::render::exact::vgap(last, b.window);
        let this = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let w = f32::from(bounds.size.width);
                let _ = this.update(cx, |v, cx| {
                    if (v.width - w).abs() > 0.01 {
                        v.width = w;
                        cx.notify();
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
            .text_color(theme.tokens.text)
            .child(measure)
            .child(gap_top)
            .child(body)
            .child(gap_bottom);
        let root = div()
            .id("markdown")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .min_w_0();
        let root = match sheet.max_width {
            Some(m) => root
                .items_center()
                .child(div().w_full().max_w(gpui_kit::px(m)).child(content)),
            None => root.child(content),
        };
        root.children(self.lightbox.clone())
    }
}
