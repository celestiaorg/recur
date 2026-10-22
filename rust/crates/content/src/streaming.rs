//! `StreamingMarkdown.svelte` and `lib/stream-reveal.ts`: live assistant text revealed a unit
//! at a time. A block with no newline paints whole; after the first newline the unfinished tail
//! waits for the next newline or `complete`. Nothing renders while hidden, and returning shows
//! the latest text at once. Parsing runs off the UI thread with streaming prefixes coalesced
//! per view; Mermaid stays source until the host swaps in a settled [`MarkdownView`].

use std::rc::Rc;
use std::sync::Arc;

use futures::channel::oneshot;
use gpui_kit::{AppContext, Context, Entity, IntoElement, Render, SharedString, Task, Window};

use crate::markdown::{RenderError, escape_text, render_markdown_async};
use crate::render::style::MarkdownClass;
use crate::view::MarkdownView;

/// `completeStreamPrefix`: paint through the last newline; no newline or `complete` paints the
/// whole block. Offsets are bytes.
pub fn complete_stream_prefix(text: &str, revealed: usize, complete: bool) -> usize {
    if revealed >= text.len() || complete {
        return text.len();
    }
    let Some(nl) = text.rfind('\n') else {
        return text.len();
    };
    if nl < revealed {
        return revealed;
    }
    nl + 1
}

type Renderer = Rc<dyn Fn(&str, &str) -> oneshot::Receiver<Result<Arc<str>, RenderError>>>;

pub struct StreamingMarkdown {
    text: SharedString,
    active: bool,
    complete: bool,
    visible: bool,
    revealed: usize,
    was_showing: bool,
    html: Arc<str>,
    shown: String,
    /// The source of `html` (what is on screen).
    painted: String,
    key: String,
    render: Renderer,
    view: Entity<MarkdownView>,
    pending: Option<Task<()>>,
}

impl StreamingMarkdown {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::with_renderer(
            Rc::new(|src: &str, key: &str| render_markdown_async(src, Some(key))),
            cx,
        )
    }

    /// With a custom async renderer (tests).
    pub fn with_renderer(render: Renderer, cx: &mut Context<Self>) -> Self {
        static KEY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        StreamingMarkdown {
            text: SharedString::default(),
            active: true,
            complete: false,
            visible: true,
            revealed: 0,
            was_showing: true,
            html: Arc::from(""),
            shown: String::new(),
            painted: String::new(),
            key: format!("sm:{n}"),
            render,
            view: cx.new(|cx| MarkdownView::plain(MarkdownClass::ChatCanvas, cx)),
            pending: None,
        }
    }

    /// The props: the complete text received so far (append-only), whether the chat is on
    /// screen, and whether this text block has ended.
    pub fn set(
        &mut self,
        text: impl Into<SharedString>,
        active: bool,
        complete: bool,
        cx: &mut Context<Self>,
    ) {
        self.text = text.into();
        self.active = active;
        self.complete = complete;
        self.update(cx);
    }

    /// The inner canvas's sheet (`MarkdownView::set_sheet`).
    pub fn set_sheet(&mut self, sheet: crate::Sheet, cx: &mut Context<Self>) {
        self.view.update(cx, |v, cx| v.set_sheet(sheet, cx));
    }

    /// `document.hidden` (the app window hidden or minimized).
    pub fn set_document_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        self.update(cx);
    }

    fn showing(&self) -> bool {
        self.active && self.visible
    }

    /// The revealed prefix.
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// The HTML last painted.
    pub fn html(&self) -> &str {
        &self.html
    }

    fn update(&mut self, cx: &mut Context<Self>) {
        if !self.showing() {
            self.was_showing = false;
            // `cancelled`: a render in flight must not paint after the stream is hidden.
            self.pending = None;
            return;
        }
        let len = self.text.len();
        if !self.was_showing || len < self.revealed {
            self.revealed = len;
            self.was_showing = true;
        } else {
            self.was_showing = true;
            self.revealed = complete_stream_prefix(&self.text, self.revealed, self.complete);
        }
        let shown = self.text[..self.revealed].to_string();
        if shown == self.shown && (self.pending.is_some() || shown == self.painted) {
            return;
        }
        self.shown = shown.clone();
        if shown.is_empty() {
            self.paint(Arc::from(""), String::new(), cx);
            return;
        }
        let rx = (self.render)(&shown, &self.key);
        self.pending = Some(cx.spawn(async move |this, cx| {
            let res = rx.await.unwrap_or(Err(RenderError::Aborted));
            let _ = this.update(cx, |s, cx| {
                s.pending = None;
                match res {
                    Ok(html) => s.paint(html, shown, cx),
                    Err(RenderError::Aborted) => {}
                    Err(RenderError::TimedOut) if !s.html.is_empty() => {}
                    Err(_) => {
                        let fallback = escape_text(&shown)
                            .split('\n')
                            .collect::<Vec<_>>()
                            .join("<br>");
                        s.paint(fallback.into(), shown, cx);
                    }
                }
            });
        }));
    }

    fn paint(&mut self, html: Arc<str>, src: String, cx: &mut Context<Self>) {
        self.html = html.clone();
        self.painted = src;
        self.view.update(cx, |v, cx| v.set_html(&html, cx));
        cx.notify();
    }
}

impl Render for StreamingMarkdown {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.view.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveals_through_the_last_newline() {
        assert_eq!(complete_stream_prefix("abc", 0, false), 3);
        assert_eq!(complete_stream_prefix("a\nb", 0, false), 2);
        assert_eq!(complete_stream_prefix("a\nb\nc", 2, false), 4);
        assert_eq!(complete_stream_prefix("a\nbc", 3, false), 3);
        assert_eq!(complete_stream_prefix("a\nb", 0, true), 3);
    }

    /// `stream-reveal.test.ts`.
    #[test]
    fn stream_reveal_cases() {
        assert_eq!(complete_stream_prefix("Hello world", 0, false), 11, "no newline yet");
        assert_eq!(complete_stream_prefix("one\ntwo\npartial", 0, false), 8, "holds the partial");
        assert_eq!(complete_stream_prefix("one\npartial", 0, true), 11, "flushes at block end");
        assert_eq!(complete_stream_prefix("one\ntwo", 6, false), 6, "never rewinds");
        assert_eq!(complete_stream_prefix("hi", 10, false), 2, "clamps when it shrinks");
    }
}
