//! Text fields with the DOM's event model: `input` on every edit, `change` when a value edited
//! since focus is committed (Enter or blur), `focus` / `blur`, and Svelte's one-way
//! `value={expr}` (the field takes the expression's value whenever the expression changes).

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{AppContext as _, Context, Entity, SharedString, Subscription, Window};

/// What a field reports to its view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEvent {
    Input,
    Change,
    Focus,
    Blur,
}

/// One field: the editor state, the bound value last applied, and the value at focus.
pub struct Field {
    pub input: Entity<InputState>,
    bound: Option<String>,
    pending: Option<String>,
    at_focus: String,
    masked: std::cell::Cell<bool>,
    _sub: Subscription,
}

/// Options for a new field.
#[derive(Clone, Copy, Debug, Default)]
pub struct FieldOpts {
    pub masked: bool,
    pub align_right: bool,
}

impl Field {
    /// A field whose events go to `on(view, key, event, window, cx)`.
    pub fn new<V: 'static>(
        key: String,
        opts: FieldOpts,
        on: fn(&mut V, &str, FieldEvent, &mut Window, &mut Context<V>),
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Field {
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).masked(opts.masked);
            if opts.align_right {
                s.set_text_align(gpui_kit::TextAlign::Right, cx);
            }
            s
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            move |view: &mut V, _, ev: &InputEvent, window, cx| {
                let evs: &[FieldEvent] = match ev {
                    InputEvent::Change => &[FieldEvent::Input],
                    InputEvent::Focus => &[FieldEvent::Focus],
                    InputEvent::Blur => &[FieldEvent::Change, FieldEvent::Blur],
                    InputEvent::PressEnter { .. } => &[FieldEvent::Change],
                };
                for e in evs {
                    on(view, &key, *e, window, cx);
                }
            },
        );
        Field {
            input,
            bound: None,
            pending: None,
            at_focus: String::new(),
            masked: std::cell::Cell::new(opts.masked),
            _sub: sub,
        }
    }

    pub fn value(&self, cx: &gpui_kit::App) -> String {
        self.input.read(cx).value().to_string()
    }

    /// The `change` gate: true (once) when the value differs from the value at focus or at
    /// the last commit.
    pub fn take_change(&mut self, cx: &gpui_kit::App) -> bool {
        let v = self.value(cx);
        if v == self.at_focus {
            return false;
        }
        self.at_focus = v;
        true
    }

    /// Record the value at focus (`focus`).
    pub fn focused(&mut self, cx: &gpui_kit::App) {
        self.at_focus = self.value(cx);
    }

    /// `value={expr}`: apply `v` when it differs from the last bound value.
    pub fn bind(&mut self, v: &str, window: &mut Window, cx: &mut gpui_kit::App) {
        if let Some(p) = self.pending.take() {
            self.set(&p, window, cx);
        }
        if self.bound.as_deref() == Some(v) {
            return;
        }
        self.bound = Some(v.to_string());
        self.set(v, window, cx);
    }

    /// `el.value = …` from an async callback: applied on the next render.
    pub fn rewrite(&mut self, v: impl Into<String>) {
        self.pending = Some(v.into());
    }

    /// Set the text now (no events), keeping the focus baseline in step.
    pub fn set(&mut self, v: &str, window: &mut Window, cx: &mut gpui_kit::App) {
        if self.value(cx) != v {
            let v = SharedString::from(v.to_string());
            self.input.update(cx, |s, cx| s.set_value(v, window, cx));
        }
        self.at_focus = v.to_string();
    }

    /// Apply a pending rewrite without a bound value (`bind:value` fields).
    pub fn flush(&mut self, window: &mut Window, cx: &mut gpui_kit::App) {
        if let Some(p) = self.pending.take() {
            self.set(&p, window, cx);
        }
    }

    pub fn set_masked(&self, masked: bool, window: &mut Window, cx: &mut gpui_kit::App) {
        if self.masked.get() != masked {
            self.masked.set(masked);
            self.input
                .update(cx, |s, cx| s.set_masked(masked, window, cx));
        }
    }

    pub fn is_focused(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        gpui_kit::Focusable::focus_handle(self.input.read(cx), cx).is_focused(window)
    }
}
