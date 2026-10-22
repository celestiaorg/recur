//! The PR textareas (`.finding-note-input`): a gpui-base textarea with the emoji and GitHub
//! `@mention` typeaheads attached (`emojiShortcodes`, `userMentions`).

use gpui_kit::base::input::{InputEvent, TextareaMode, TextareaState};
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Subscription, Window};
use recur_view_chat::suggest::{SuggestAttach, attach_emoji, attach_user_mentions};
use recur_view_core::AppState;

pub struct Field {
    pub area: Entity<TextareaState>,
    _attach: (
        Entity<SuggestAttach<TextareaMode>>,
        Entity<SuggestAttach<TextareaMode>>,
    ),
    _sub: Subscription,
}

impl Field {
    /// A textarea with `placeholder`; `on_change` runs on every edit.
    pub fn new<H: 'static>(
        state: &Entity<AppState>,
        placeholder: &str,
        rows: (usize, usize),
        on_change: fn(&mut H, &mut Context<H>),
        window: &mut Window,
        cx: &mut Context<H>,
    ) -> Field {
        let placeholder = SharedString::from(placeholder.to_string());
        let area = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(rows.0, rows.1)
                .placeholder(placeholder)
        });
        recur_view_review::style::style_textarea(&area, cx);
        let emoji = attach_emoji(&area, None, window, cx);
        let mentions = attach_user_mentions(&area, state.clone(), None, window, cx);
        let sub = cx.subscribe(&area, move |h, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                on_change(h, cx);
                cx.notify();
            }
        });
        Field {
            area,
            _attach: (emoji, mentions),
            _sub: sub,
        }
    }

    pub fn value(&self, cx: &App) -> String {
        self.area.read(cx).value().to_string()
    }

    pub fn set(&self, v: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        let v = v.into();
        self.area.update(cx, |a, cx| a.set_value(v, window, cx));
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.area.update(cx, |a, cx| a.focus(window, cx));
    }
}
