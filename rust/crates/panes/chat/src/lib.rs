//! The agent chat (`AgentChat.svelte`) that the workspace view and Ask Recur embed, with its
//! cards, pickers, composer and the app-wide prompt overlays.
//!
//! # Embedding the chat
//!
//! [`chat::AgentChat`] is an entity. Build it with the shell's [`ViewContext`] and the Svelte
//! props as [`chat::ChatProps`], declare which callbacks you handle with [`chat::ChatHooks`],
//! and subscribe to its [`chat::ChatEvent`]s:
//!
//! ```ignore
//! let chat = cx.new(|cx| AgentChat::new(&vcx, ChatProps { reset_key: format!("{ws}:{code}"), ..Default::default() }, window, cx));
//! chat.update(cx, |c, cx| c.set_hooks(ChatHooks { send: true, stop: true, ..Default::default() }, cx));
//! cx.guarded_subscribe_in(&chat, window, |host, chat, ev: &ChatEvent, window, cx| match ev {
//!     ChatEvent::Send { alt } => { let text = chat.read(cx).input().to_string(); /* send, then */ chat.update(cx, |c, cx| c.set_draft(String::new(), Vec::new(), cx)); }
//!     ChatEvent::Stop => { /* cancel the turn */ }
//!     _ => {}
//! });
//! ```
//!
//! - Props: push new props with `set_props(props, window, cx)` whenever the transcript
//!   (`messages`), `sending`, usage or scope change. A new `reset_key` parks the old chat's
//!   scroll and expanded rows in the transcript UI cache and restores the new one's.
//! - The draft (`bind:input` / `bind:attachments`): read `input()` / `attachments()`, replace
//!   them with `set_draft`; `ChatEvent::Draft` fires on every edit.
//! - Snippets: `set_toolbar(Some(view))` fills the composer row (for example a
//!   [`pickers::ModelPickers`]), `set_empty(Some(view))` replaces the empty transcript, and
//!   `set_session_card(Some(renderer))` draws cockpit session logs.
//! - Imperative calls: `send`, `scroll_to_bottom`, `restore_parked_scroll`, `reveal_fact`,
//!   `focus_composer`, `insert_file_mention`.
//! - Live events: hosts reduce `recur_services` session events into `messages` with
//!   [`session_events::session_event`] and the `recur_engine::view::chat` reducers (see
//!   [`app_chat::AppChat`] for a complete host), or attach a [`session_feed::SessionFeed`].
//!
//! [`app_chat::AppChat`] is the Ask Recur panel (`AppChat.svelte`): build it with
//! `AppChat::new(&vcx, visible, window, cx)`, call `set_visible` as the host panel shows or
//! hides, and handle [`app_chat::AppChatEvent::Hide`] (the header's hide button).
//!
//! # Overlays
//!
//! [`register`] adds the app-wide overlays to the shell: the floating permission card and the
//! off-tab ask toasts ([`prompts`]), and the menus of the emoji and `@mention` typeaheads
//! (`recur_view_widgets::suggest`). Text fields opt into a typeahead with `attach_emoji` /
//! `attach_user_mentions`.

use gpui_kit::AppContext as _;
use recur_view_core::{OverlaySlot, ViewContext, ViewRegistry};
use recur_view_widgets::suggest;
pub mod app_chat;
pub mod blocks;
pub mod cards;
pub mod chat;
pub mod chips;
pub mod composer;
pub mod composer_doc;
pub mod composer_undo;
pub mod dock;
pub mod flow;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod glance;
pub mod heap;
pub mod hoist;
pub mod html_inline;
pub mod meter;
pub mod panes;
pub mod pickers;
pub mod prompt_actions;
pub mod prompts;
pub mod rows;
pub mod session_events;
pub mod session_feed;
pub mod shhl;
#[cfg(test)]
mod spec;
pub mod strip;
pub mod style;
#[cfg(any(test, feature = "test-support"))]
mod testing;
pub mod tools;
pub mod transcript;
pub mod transcript_ui;
pub mod view;
pub mod window;

/// Registers the chat's overlays with the shell.
pub fn register(r: &mut ViewRegistry) {
    r.overlay(OverlaySlot::EmojiSuggest, |_, _, cx| {
        cx.new(|cx| suggest::SuggestOverlay::new(suggest::SuggestKind::Emoji, cx))
            .into()
    });
    r.overlay(OverlaySlot::UserMentionSuggest, |_, _, cx| {
        cx.new(|cx| suggest::SuggestOverlay::new(suggest::SuggestKind::Mention, cx))
            .into()
    });
    r.overlay(
        OverlaySlot::PermissionPrompts,
        |vcx: &ViewContext, _, cx| {
            let state = vcx.state.clone();
            cx.new(|cx| prompts::PermissionPrompts::new(state, cx))
                .into()
        },
    );
    r.overlay(OverlaySlot::QuestionPrompts, |vcx: &ViewContext, _, cx| {
        let state = vcx.state.clone();
        cx.new(|cx| prompts::QuestionPrompts::new(state, cx)).into()
    });
}
