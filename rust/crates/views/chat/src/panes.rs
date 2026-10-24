//! The chat's two halves as their own views, so typing never re-renders the transcript.
//!
//! GPUI re-renders every view that is not behind a cached boundary on each frame. The
//! transcript is a cached [`TranscriptPane`]: it re-renders when the chat notifies (any
//! transcript, props or store change) or when its scroller moves, and is reused otherwise.
//! The composer is a [`DockPane`]: composer-only edits notify it through
//! [`AgentChat::notify_dock`], which redraws the chat's chrome and the dock but leaves the
//! transcript's cached layout in place.

use gpui_kit::{
    AnyElement, Context, Entity, IntoElement, Render, Subscription, WeakEntity, Window, div,
};

use crate::chat::AgentChat;

pub struct TranscriptPane {
    chat: WeakEntity<AgentChat>,
    _sub: Subscription,
}

impl TranscriptPane {
    pub(crate) fn new(chat: &Entity<AgentChat>, cx: &mut Context<Self>) -> Self {
        TranscriptPane {
            chat: chat.downgrade(),
            _sub: cx.observe(chat, |_, _, cx| cx.notify()),
        }
    }

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.chat.upgrade() {
            Some(chat) => chat.update(cx, |c, cx| c.transcript_main(window, cx)),
            None => div().into_any_element(),
        }
    }
}

impl Render for TranscriptPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

pub struct DockPane {
    chat: WeakEntity<AgentChat>,
}

impl DockPane {
    pub(crate) fn new(chat: &Entity<AgentChat>) -> Self {
        DockPane {
            chat: chat.downgrade(),
        }
    }

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.chat
            .upgrade()
            .and_then(|chat| chat.update(cx, |c, cx| c.dock(window, cx)))
            .unwrap_or_else(|| div().into_any_element())
    }
}

impl Render for DockPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
