//! `FilePreview.svelte`: the Preview surface of dual-nature files (markdown, SVG, HTML,
//! Mermaid) with find-in-page and per-path scroll that survives remounts and restores.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Render, RenderImage, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div, point,
    px,
};
use recur_ui::{Theme, TextStyled};

use super::PreviewKind;
use super::find::{FindOptions, PreviewFindMatch, collect_preview_find_ranges, reveal_offset};
use super::find_bar::{FindBarEvent, PreviewFindBar};
use super::scroll::{Attachment, PreviewScrollStore, Scroller};
use crate::html::{HtmlPreview, HtmlPreviewEvent};
use crate::mermaid::MermaidPreview;
use crate::render::style::MarkdownClass;
use crate::view::MarkdownView;

/// The scroll store a preview owner shares with its surfaces (`scroll` prop).
pub type SharedScroll = Rc<RefCell<PreviewScrollStore>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePreviewEvent {
    /// `onSearchOpenChange`.
    SearchOpenChanged(bool),
}

enum Surface {
    Markdown(Entity<MarkdownView>),
    Svg(Option<(Arc<resvg::usvg::Tree>, f32, f32)>),
    Html(Entity<HtmlPreview>),
    Mermaid(Entity<MermaidPreview>),
    Empty(&'static str),
}

struct HandleScroller<'a>(&'a ScrollHandle);

impl Scroller for HandleScroller<'_> {
    fn top(&self) -> f64 {
        -f64::from(f32::from(self.0.offset().y))
    }
    fn left(&self) -> f64 {
        -f64::from(f32::from(self.0.offset().x))
    }
    fn set(&mut self, top: f64, left: f64) {
        let max = self.0.max_offset();
        let t = top.clamp(0., f64::from(f32::from(max.y)).max(0.)) as f32;
        let l = left.clamp(0., f64::from(f32::from(max.x)).max(0.)) as f32;
        self.0.set_offset(point(px(-l), px(-t)));
    }
}

pub struct FilePreview {
    kind: PreviewKind,
    content: SharedString,
    path: SharedString,
    loaded: SharedString,
    workspace: SharedString,
    scroll: SharedScroll,
    active: bool,
    /// The surface sits in a parked (reparented) shell: its zeroed overflow must not save.
    parked: bool,
    surface: Surface,
    handle: ScrollHandle,
    attachment: Option<Attachment>,
    last_offset: (f64, f64),
    find_open: bool,
    bar: Entity<PreviewFindBar>,
    matches: PreviewFindMatch,
    index: usize,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<FilePreviewEvent> for FilePreview {}

impl Focusable for FilePreview {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FilePreview {
    pub fn new(
        kind: PreviewKind,
        content: impl Into<SharedString>,
        path: impl Into<SharedString>,
        scroll: SharedScroll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = path.into();
        let bar = cx.new(|cx| PreviewFindBar::new(window, cx));
        let sub = cx.subscribe_in(&bar, window, |p, _, ev: &FindBarEvent, window, cx| match ev {
            FindBarEvent::Changed => p.refresh(window, cx),
            FindBarEvent::Next => p.step(1, window, cx),
            FindBarEvent::Previous => p.step(-1, window, cx),
            FindBarEvent::Close => p.close_search(window, cx),
        });
        let mut fp = FilePreview {
            kind,
            content: SharedString::default(),
            loaded: path.clone(),
            path,
            workspace: SharedString::default(),
            scroll,
            active: true,
            parked: false,
            surface: Surface::Empty(""),
            handle: ScrollHandle::new(),
            attachment: None,
            last_offset: (0., 0.),
            find_open: false,
            bar,
            matches: PreviewFindMatch {
                valid: true,
                ..Default::default()
            },
            index: 0,
            focus: cx.focus_handle(),
            _subs: vec![sub],
        };
        fp.set_content(content, window, cx);
        fp
    }

    fn findable(&self) -> bool {
        self.kind != PreviewKind::Svg
    }

    /// `content` changed (the editor buffer).
    pub fn set_content(&mut self, content: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let content = content.into();
        if content == self.content && !matches!(self.surface, Surface::Empty("")) {
            return;
        }
        self.content = content.clone();
        let blank = content.trim().is_empty();
        self.surface = match (self.kind, &self.surface) {
            (PreviewKind::Markdown, _) if blank => Surface::Empty("Empty markdown"),
            (PreviewKind::Markdown, Surface::Markdown(v)) => {
                v.update(cx, |v, cx| v.set_text(content, cx));
                Surface::Markdown(v.clone())
            }
            (PreviewKind::Markdown, _) => {
                let v = cx.new(|cx| MarkdownView::new(content, MarkdownClass::MdDoc, false, cx));
                // `MutationObserver`: re-collect matches when the rendered tree changes.
                self._subs.push(cx.observe_in(&v, window, |p, _, window, cx| {
                    if p.find_open && p.active {
                        p.refresh_later(window, cx);
                    }
                }));
                Surface::Markdown(v)
            }
            (PreviewKind::Svg, _) if blank => Surface::Empty("Empty SVG"),
            (PreviewKind::Svg, _) => Surface::Svg(crate::svg::parse(&content).ok().map(|t| {
                let s = t.size();
                (Arc::new(t), s.width(), s.height())
            })),
            (PreviewKind::Html, Surface::Html(h)) => {
                h.update(cx, |h, cx| h.set_html(&content, window, cx));
                Surface::Html(h.clone())
            }
            (PreviewKind::Html, _) => {
                let path = self.path.clone();
                let h = cx.new(|cx| HtmlPreview::new(&content, &path, self.scroll.clone(), window, cx));
                self._subs.push(cx.subscribe_in(&h, window, |p, _, ev: &HtmlPreviewEvent, window, cx| {
                    p.on_page(ev, window, cx)
                }));
                Surface::Html(h)
            }
            (PreviewKind::Mermaid, Surface::Mermaid(m)) => {
                m.update(cx, |m, cx| m.set_content(&content, window, cx));
                Surface::Mermaid(m.clone())
            }
            (PreviewKind::Mermaid, _) => {
                let path = self.path.clone();
                let scroll = self.scroll.clone();
                let m = cx.new(|cx| MermaidPreview::new(&content, &path, Some(scroll), window, cx));
                self._subs.push(cx.observe_in(&m, window, |p, _, window, cx| {
                    if p.find_open && p.active {
                        p.refresh_later(window, cx);
                    }
                }));
                Surface::Mermaid(m)
            }
        };
        if self.find_open {
            self.refresh_later(window, cx);
        }
        cx.notify();
    }

    /// The path the editor shows and the document that has loaded for it (`loaded`), and
    /// the workspace that owns the scroll map.
    pub fn set_path(&mut self, path: &str, loaded: &str, workspace: &str, cx: &mut Context<Self>) {
        if self.path.as_ref() == path && self.loaded.as_ref() == loaded && self.workspace.as_ref() == workspace {
            return;
        }
        self.detach();
        self.path = path.to_string().into();
        self.loaded = loaded.to_string().into();
        self.workspace = workspace.to_string().into();
        cx.notify();
    }

    /// `active`: the surface is the one on screen. Highlights paint only for it.
    pub fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active == self.active {
            return;
        }
        self.active = active;
        if !self.findable() {
            return;
        }
        if active && self.find_open {
            self.refresh(window, cx);
        } else {
            self.paint_highlights(false, cx);
        }
        cx.notify();
    }

    /// The surface moved under a parked shell (or back).
    pub fn set_parked(&mut self, parked: bool) {
        self.parked = parked;
    }

    pub fn is_search_open(&self) -> bool {
        self.find_open
    }

    /// `toggleSearch`: false when this kind has no find (SVG).
    pub fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.findable() {
            return false;
        }
        if self.find_open {
            self.close_search(window, cx);
        } else {
            self.open_search(window, cx);
        }
        true
    }

    /// `openSearch`: open (or refocus) the bar and select its query.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.findable() {
            return;
        }
        self.set_find_open(true, cx);
        self.bar.update(cx, |b, cx| b.focus_and_select(window, cx));
        self.refresh(window, cx);
    }

    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_find_open(false, cx);
        window.focus(&self.focus, cx);
    }

    fn set_find_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.find_open == open {
            return;
        }
        self.find_open = open;
        cx.emit(FilePreviewEvent::SearchOpenChanged(open));
        if !open {
            self.matches = PreviewFindMatch {
                valid: true,
                ..Default::default()
            };
            self.index = 0;
            self.bar.update(cx, |b, cx| {
                b.reset_counts();
                cx.notify();
            });
            if self.active {
                self.paint_highlights(false, cx);
            }
            if let Surface::Html(h) = &self.surface {
                h.update(cx, |h, cx| h.find_clear(cx));
            }
        }
        cx.notify();
    }

    fn refresh_later(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.on_next_frame(window, |p, window, cx| p.refresh(window, cx));
    }

    /// The rendered text blocks of the surface.
    fn blocks(&self, cx: &gpui_kit::App) -> Vec<(usize, String)> {
        match &self.surface {
            Surface::Markdown(v) => v.read(cx).text_blocks(),
            Surface::Mermaid(m) => m.read(cx).text_blocks(),
            _ => Vec::new(),
        }
    }

    /// `refreshInPage` (or the page's own find for HTML).
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open || !self.active || !self.findable() {
            return;
        }
        let query = self.bar.read(cx).query(cx);
        let opts: FindOptions = self.bar.read(cx).options();
        if let Surface::Html(h) = &self.surface {
            h.update(cx, |h, cx| h.find_query(&query, opts, cx));
            return;
        }
        if query.is_empty() {
            self.matches = PreviewFindMatch {
                valid: true,
                ..Default::default()
            };
            self.index = 0;
            self.paint_highlights(false, cx);
            return;
        }
        self.matches = collect_preview_find_ranges(&self.blocks(cx), &query, opts);
        let n = self.matches.ranges.len();
        self.index = if n == 0 {
            0
        } else if (1..=n).contains(&self.index) {
            self.index
        } else {
            1
        };
        let (index, total, capped, valid) = (self.index, n, self.matches.capped, self.matches.valid);
        self.bar.update(cx, |b, cx| b.set_results(index, total, capped, valid, cx));
        self.paint_highlights(true, cx);
        self.reveal(window, cx);
    }

    fn step(&mut self, dir: i32, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        if let Surface::Html(h) = &self.surface {
            h.update(cx, |h, cx| h.find_step(dir, cx));
            return;
        }
        let n = self.matches.ranges.len();
        if n == 0 {
            return;
        }
        let mut i = self.index as i64 + dir as i64;
        if i < 1 {
            i = n as i64;
        }
        if i > n as i64 {
            i = 1;
        }
        self.index = i as usize;
        let (index, capped, valid) = (self.index, self.matches.capped, self.matches.valid);
        self.bar.update(cx, |b, cx| b.set_results(index, n, capped, valid, cx));
        if !self.active {
            return;
        }
        self.paint_highlights(true, cx);
        self.reveal(window, cx);
    }

    /// `applyPreviewFindHighlights` / `clearPreviewFindHighlights`.
    fn paint_highlights(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut map: HashMap<usize, Vec<(std::ops::Range<usize>, bool)>> = HashMap::new();
        if on {
            for (i, (id, r)) in self.matches.ranges.iter().enumerate() {
                map.entry(*id).or_default().push((r.clone(), i + 1 == self.index));
            }
        }
        match &self.surface {
            Surface::Markdown(v) => v.update(cx, |v, cx| v.set_highlights(map, cx)),
            Surface::Mermaid(m) => m.update(cx, |m, cx| m.set_highlights(map, cx)),
            _ => {}
        }
    }

    /// `revealPreviewFindRange`: centre the current match in the scroller (Mermaid pans).
    fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, r)) = self.index.checked_sub(1).and_then(|i| self.matches.ranges.get(i)).cloned()
        else {
            return;
        };
        match &self.surface {
            Surface::Markdown(v) => {
                let Some(b) = v.read(cx).range_bounds(id, r, window) else {
                    return;
                };
                let view = self.handle.bounds();
                let mut s = HandleScroller(&self.handle);
                let max = self.handle.max_offset();
                let mid = |a: Pixels, b: Pixels| f32::from(a + b) / 2.;
                let top = reveal_offset(
                    s.top() as f32,
                    f32::from(max.y),
                    mid(b.top(), b.bottom()),
                    mid(view.top(), view.bottom()),
                );
                let left = reveal_offset(
                    s.left() as f32,
                    f32::from(max.x),
                    mid(b.left(), b.right()),
                    mid(view.left(), view.right()),
                );
                s.set(top as f64, left as f64);
                cx.notify();
            }
            Surface::Mermaid(m) => m.update(cx, |m, cx| m.reveal(id, r, window, cx)),
            _ => {}
        }
    }

    fn on_page(&mut self, ev: &HtmlPreviewEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            HtmlPreviewEvent::FindResult { total, index, valid } => {
                let capped = *total >= super::find::PREVIEW_FIND_CAP;
                let (t, i, v) = (*total, *index, *valid);
                self.bar.update(cx, |b, cx| b.set_results(i, t, capped, v, cx));
            }
            HtmlPreviewEvent::FindKey(k) => {
                if !self.active {
                    return;
                }
                match k.as_str() {
                    "f" => self.open_search(window, cx),
                    "g" if self.find_open => self.step(1, window, cx),
                    "G" if self.find_open => self.step(-1, window, cx),
                    "Escape" if self.find_open => self.close_search(window, cx),
                    _ => {}
                }
            }
            HtmlPreviewEvent::Loaded => {
                if self.find_open && self.active {
                    self.refresh(window, cx);
                }
            }
        }
    }

    /// `onWinKey`: ⌘F opens, Escape closes, ⌘G / F3 step while open. Keys typed into another
    /// field are not ours.
    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.active || !self.findable() {
            return;
        }
        let in_find = self.bar.read(cx).contains_focus(window, cx);
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
        if !self.find_open {
            return;
        }
        if (cmd && !m.alt && k.key.eq_ignore_ascii_case("g")) || k.key == "f3" {
            cx.stop_propagation();
            self.step(if m.shift { -1 } else { 1 }, window, cx);
            return;
        }
        let _ = in_find;
    }

    fn detach(&mut self) {
        if let Some(a) = self.attachment.take() {
            let el = HandleScroller(&self.handle);
            a.detach(&mut self.scroll.borrow_mut(), &el, self.parked);
        }
    }

    /// Restore on attach, retry while content grows, save the user's scrolling.
    fn track_scroll(&mut self) {
        let el = HandleScroller(&self.handle);
        let now = (el.top(), el.left());
        match &mut self.attachment {
            None => {
                let mut el = HandleScroller(&self.handle);
                self.attachment = Attachment::attach(
                    &self.scroll.borrow(),
                    &self.path,
                    &self.loaded,
                    &self.workspace,
                    &mut el,
                );
                self.last_offset = (el.top(), el.left());
            }
            Some(a) => {
                if (now.0 - self.last_offset.0).abs() >= 0.5 || (now.1 - self.last_offset.1).abs() >= 0.5 {
                    a.save(&mut self.scroll.borrow_mut(), &el, self.parked);
                }
                let mut el = HandleScroller(&self.handle);
                a.try_restore(&self.scroll.borrow(), &mut el);
                self.last_offset = (el.top(), el.left());
            }
        }
    }
}

impl Drop for FilePreview {
    fn drop(&mut self) {
        self.detach();
    }
}

impl Render for FilePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let this = cx.entity().downgrade();
        let track = canvas(
            move |_, _, cx| {
                let _ = this.update(cx, |p, _| p.track_scroll());
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let empty = |label: &'static str| {
            div()
                .flex()
                .justify_center()
                .pt(px(48.))
                .pb(px(48.))
                .px(px(24.))
                .child(
                    recur_ui::text(label)
                        .ui()
                        .size(12.)
                        .color(t.faint)
                        .leading(1.45)
                        .align(recur_ui::text::Align::Center),
                )
                .into_any_element()
        };
        let surface = match &self.surface {
            Surface::Markdown(v) => div()
                .id("file-preview-md")
                .track_scroll(&self.handle)
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .bg(t.bg)
                .flex()
                .flex_col()
                .child(track)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .pt(px(28.))
                        .pb(px(48.))
                        .px(px(32.))
                        .child(v.clone()),
                )
                .into_any_element(),
            Surface::Svg(tree) => {
                let img = tree.as_ref().map(|(tree, w, h)| svg_img(tree.clone(), *w, *h, t.line));
                div()
                    .id("file-preview-svg")
                    .track_scroll(&self.handle)
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .bg(t.bg)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .p(px(24.))
                    .child(track)
                    .children(img)
                    .into_any_element()
            }
            Surface::Html(h) => div().flex_1().min_h_0().flex().flex_col().child(h.clone()).into_any_element(),
            Surface::Mermaid(m) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(m.clone())
                .into_any_element(),
            Surface::Empty(label) => div()
                .flex_1()
                .min_h_0()
                .bg(t.bg)
                .flex()
                .flex_col()
                .child(empty(label))
                .into_any_element(),
        };
        div()
            .id("file-preview-host")
            .track_focus(&self.focus)
            .key_context("FilePreview")
            .capture_key_down(cx.listener(Self::key_down))
            .flex_1()
            .min_h_0()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(self.find_open, |d| d.child(self.bar.clone()))
            .child(surface)
    }
}

/// `<img src={svgDataUrl}>`: natural size (`max-width/height: 100%`), a 1px `--line` ring.
fn svg_img(tree: Arc<resvg::usvg::Tree>, w: f32, h: f32, line: recur_ui::Color) -> gpui_kit::AnyElement {
    let cache: Rc<RefCell<Option<(u32, u32, Arc<RenderImage>)>>> = Rc::default();
    div()
        .flex_none()
        .w(px(w))
        .h(px(h))
        .max_w_full()
        .max_h_full()
        .shadow(vec![gpui_kit::BoxShadow {
            color: line.into(),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
            inset: false,
        }])
        .child(
            canvas(
                |_, _, _| {},
                move |b: Bounds<Pixels>, _, window, _| {
                    let s = window.scale_factor();
                    let (dw, dh) = (
                        (f32::from(b.size.width) * s).round() as u32,
                        (f32::from(b.size.height) * s).round() as u32,
                    );
                    let mut c = cache.borrow_mut();
                    if c.as_ref().is_none_or(|(cw, ch, _)| (*cw, *ch) != (dw, dh))
                        && let Some(img) =
                            crate::svg::image_at(&tree, f32::from(b.size.width), f32::from(b.size.height), s)
                    {
                        *c = Some((dw, dh, img));
                    }
                    if let Some((_, _, img)) = c.as_ref() {
                        let _ = window.paint_image(b, b, Default::default(), img.clone(), 0, false);
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}
