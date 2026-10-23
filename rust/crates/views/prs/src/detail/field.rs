//! The PR textareas (`.finding-note-input`): a gpui-base textarea with the emoji and GitHub
//! `@mention` typeaheads attached (`emojiShortcodes`, `userMentions`).

use gpui_kit::base::input::{InputEvent, TextareaMode, TextareaState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, KeystrokeEvent, SharedString,
    Subscription, Window,
};
use std::rc::Rc;

use recur_view_chat::suggest::{SetFn, SuggestAttach, attach_emoji, attach_user_mentions};
use recur_view_core::AppState;

pub struct Field {
    pub area: Entity<TextareaState>,
    _attach: (
        Entity<SuggestAttach<TextareaMode>>,
        Entity<SuggestAttach<TextareaMode>>,
    ),
    _sub: Subscription,
    _submit: Option<Subscription>,
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
        // A typeahead pick rewrites the value without an input event.
        let me = cx.entity().downgrade();
        let on_set: SetFn = Rc::new(move |_, _, cx| {
            let _ = me.update(cx, |h, cx| {
                on_change(h, cx);
                cx.notify();
            });
        });
        let emoji = attach_emoji(&area, Some(on_set.clone()), window, cx);
        let mentions = attach_user_mentions(&area, state.clone(), Some(on_set), window, cx);
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
            _submit: None,
        }
    }

    /// ⌘↵ / Ctrl+↵ while focused runs `f` (`composeKey`). The keystroke is taken before the
    /// textarea's `secondary-enter` binding, which would insert a newline first.
    pub fn on_submit<H: 'static>(
        mut self,
        f: fn(&mut H, &mut Window, &mut Context<H>),
        cx: &mut Context<H>,
    ) -> Field {
        let (area, me) = (self.area.clone(), cx.entity().downgrade());
        self._submit = Some(
            cx.intercept_keystrokes(move |ev: &KeystrokeEvent, window, cx| {
                let k = &ev.keystroke;
                if k.key != "enter" || !(k.modifiers.platform || k.modifiers.control) {
                    return;
                }
                if !area.read(cx).focus_handle(cx).is_focused(window) {
                    return;
                }
                cx.stop_propagation();
                let _ = me.update(cx, |h, cx| f(h, window, cx));
            }),
        );
        self
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

/// A press anywhere on a field's box focuses the field and goes no further: the padding is
/// the textarea's own in the web build, and `onPRChromeMouseDown` skips text entries.
pub fn press_focuses<E: gpui_kit::InteractiveElement>(el: E, focus: gpui_kit::FocusHandle) -> E {
    el.on_mouse_down(gpui_kit::MouseButton::Left, move |_, window, cx| {
        window.focus(&focus, cx);
        cx.stop_propagation();
    })
}
