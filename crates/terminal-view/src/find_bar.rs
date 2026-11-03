//! The terminal find bar: query
//! field, match count, previous / next, match-case / regexp / whole-word toggles and close.
//!
//! The dock owns when it opens and which pane it searches; the bar owns its fields and keys
//! (Enter / Shift+Enter step, Escape closes, ⌥⌘C / ⌥⌘R / ⌥⌘W toggle while it has focus).

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::guard::Guarded;
use recur_ui::style::{self, card, focus_ring, is_focus_visible, outline, text_field};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, TextStyled, Theme, icon, text};

use crate::view::{FindOptions, find_count_label};

/// What the dock reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindBarEvent {
    /// Query or toggles changed: run an incremental `find_next`.
    Changed,
    Next,
    Previous,
    Close,
}

/// The buttons, in tab order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Button {
    Previous,
    Next,
    Case,
    Regexp,
    Word,
    Close,
}

impl Button {
    const ALL: [Button; 6] = [
        Button::Previous,
        Button::Next,
        Button::Case,
        Button::Regexp,
        Button::Word,
        Button::Close,
    ];

    fn id(self) -> &'static str {
        match self {
            Button::Previous => "term-find-prev",
            Button::Next => "term-find-next",
            Button::Case => "term-find-case",
            Button::Regexp => "term-find-regex",
            Button::Word => "term-find-word",
            Button::Close => "term-find-close",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Button::Previous => "Previous match (⇧⌘G)",
            Button::Next => "Next match (⌘G)",
            Button::Case => "Match case (⌥⌘C)",
            Button::Regexp => "Regular expression (⌥⌘R)",
            Button::Word => "Whole word (⌥⌘W)",
            Button::Close => "Close",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Button::Previous => IconName::ChevronLeft,
            Button::Next => IconName::ChevronRight,
            Button::Case => IconName::CaseSensitive,
            Button::Regexp => IconName::Regex,
            Button::Word => IconName::WholeWord,
            Button::Close => IconName::X,
        }
    }
}

pub struct TerminalFindBar {
    input: Entity<InputState>,
    focus: FocusHandle,
    buttons: [FocusHandle; 6],
    case_sensitive: bool,
    regexp: bool,
    whole_word: bool,
    index: usize,
    total: usize,
    capped: bool,
    valid: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FindBarEvent> for TerminalFindBar {}

impl Focusable for TerminalFindBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalFindBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = style::text_input(&"Find".into(), window, cx);
        let subs = vec![cx.guarded_subscribe_in(
            &input,
            window,
            |bar, _, ev: &InputEvent, _w, cx| match ev {
                InputEvent::Change => {
                    bar.reset_counts();
                    cx.emit(FindBarEvent::Changed);
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => cx.emit(if *shift {
                    FindBarEvent::Previous
                } else {
                    FindBarEvent::Next
                }),
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
            },
        )];
        Self {
            input,
            focus: cx.focus_handle(),
            buttons: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            case_sensitive: false,
            regexp: false,
            whole_word: false,
            index: 0,
            total: 0,
            capped: false,
            valid: true,
            _subscriptions: subs,
        }
    }

    pub fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(query.to_string(), window, cx));
    }

    pub fn options(&self) -> FindOptions {
        FindOptions {
            case_sensitive: self.case_sensitive,
            regexp: self.regexp,
            whole_word: self.whole_word,
            incremental: false,
        }
    }

    /// `onFindResults`: the active match (1-based), the highlighted total, query validity.
    pub fn set_results(&mut self, index: usize, total: usize, valid: bool, cx: &mut Context<Self>) {
        self.valid = valid;
        self.index = index;
        self.total = total;
        self.capped = total >= recur_term::screen::HIGHLIGHT_LIMIT;
        cx.notify();
    }

    /// Clears the count (empty query, or the bar closing).
    pub fn reset_counts(&mut self) {
        self.total = 0;
        self.index = 0;
        self.capped = false;
        self.valid = true;
    }

    /// The label next to the field; empty when hidden.
    pub fn count_label(&self, cx: &App) -> String {
        if self.query(cx).is_empty() || !self.valid {
            return String::new();
        }
        find_count_label(self.index, self.total, self.capped)
    }

    /// Focus the field and select its text (`openFind`).
    pub fn focus_and_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
            || self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn toggle_case(&mut self, cx: &mut Context<Self>) {
        self.case_sensitive = !self.case_sensitive;
        cx.emit(FindBarEvent::Changed);
        cx.notify();
    }

    pub fn toggle_regexp(&mut self, cx: &mut Context<Self>) {
        self.regexp = !self.regexp;
        cx.emit(FindBarEvent::Changed);
        cx.notify();
    }

    pub fn toggle_whole_word(&mut self, cx: &mut Context<Self>) {
        self.whole_word = !self.whole_word;
        cx.emit(FindBarEvent::Changed);
        cx.notify();
    }

    fn press(&mut self, b: Button, cx: &mut Context<Self>) {
        match b {
            Button::Previous => cx.emit(FindBarEvent::Previous),
            Button::Next => cx.emit(FindBarEvent::Next),
            Button::Case => self.toggle_case(cx),
            Button::Regexp => self.toggle_regexp(cx),
            Button::Word => self.toggle_whole_word(cx),
            Button::Close => cx.emit(FindBarEvent::Close),
        }
    }

    fn key_down(&mut self, e: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.key == "escape" {
            cx.stop_propagation();
            cx.emit(FindBarEvent::Close);
            return;
        }
        let m = k.modifiers;
        let cmd_or_ctrl = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.control
        };
        if cmd_or_ctrl && m.alt && !m.shift {
            match k.key.as_str() {
                "c" => self.toggle_case(cx),
                "r" => self.toggle_regexp(cx),
                "w" => self.toggle_whole_word(cx),
                _ => return,
            }
            cx.stop_propagation();
        }
    }

    /// `.obs-icon-btn` (26×26, radius 5, `--faint`, hover `--text` on `--pane-2`), with the
    /// toggles' `.on` state (`--amber` on `--amber-d`).
    fn button(&self, b: Button, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let on = match b {
            Button::Case => self.case_sensitive,
            Button::Regexp => self.regexp,
            Button::Word => self.whole_word,
            _ => false,
        };
        let focus = &self.buttons[Button::ALL.iter().position(|x| *x == b).unwrap_or(0)];
        let button = div()
            .id(b.id())
            .track_focus(focus)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(5.))
            .map(|d| {
                if on {
                    d.text_color(t.amber).bg(t.amber_d)
                } else {
                    d.text_color(t.faint)
                        .hover(move |s| s.text_color(t.text).bg(t.pane_2))
                }
            })
            .on_click(cx.guarded_listener(move |bar, _, _, cx| bar.press(b, cx)))
            .on_key_down(cx.guarded_listener(move |bar, e: &KeyDownEvent, _, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    bar.press(b, cx);
                }
            }))
            .child(icon(b.icon()).size(px(15.)))
            .when(is_focus_visible(focus, window), |d| {
                d.child(focus_ring(5., 0., cx))
            });
        titled(b.id(), b.title(), button).focus(focus)
    }
}

impl TerminalFindBar {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        style::style_text_input_unsmoothed(&self.input, cx);
        let theme = Theme::of(cx).clone();
        let t = theme.tokens;
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let label = self.count_label(cx);
        let none = !self.query(cx).is_empty() && self.valid && self.total == 0;
        // `.cm-recur-search-input`: `--pane` on light chrome, `--bg` otherwise; focus mixes the
        // border toward `--amber` and adds a 2px `--amber-d` ring.
        let field_bg = if theme.is_light() { t.pane } else { t.bg };
        let border = if focused {
            t.amber.mix(0.55, t.line)
        } else {
            t.line
        };
        let placeholder: SharedString = "Find".into();
        let field = card(field_bg, border, 6.)
            .flex_1()
            .min_w(px(80.))
            .h(px(26.))
            .px(px(9.))
            .flex()
            .items_center()
            .when(focused, |d| d.child(outline(2., 0., 6., 1., t.amber_d)))
            .child(text_field(
                &self.input,
                &placeholder,
                t.faint,
                Family::Mono,
                12.,
                window,
                cx,
            ));
        let count = (!label.is_empty()).then(|| {
            div()
                .flex_none()
                .flex()
                .justify_end()
                .px(px(2.))
                .when(!none, |d| d.min_w(px(10.5 * 4.5)))
                .child(
                    text(label)
                        .family(Family::Mono)
                        .size(10.5)
                        .color(if none { t.coral } else { t.muted })
                        .nowrap(),
                )
        });
        let group = |buttons: Vec<gpui_kit::AnyElement>| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(2.))
                .children(buttons)
        };
        let nav = group(vec![
            self.button(Button::Previous, window, cx).into_any_element(),
            self.button(Button::Next, window, cx).into_any_element(),
        ]);
        let toggles = group(vec![
            self.button(Button::Case, window, cx).into_any_element(),
            self.button(Button::Regexp, window, cx).into_any_element(),
            self.button(Button::Word, window, cx).into_any_element(),
        ]);
        let close = div()
            .ml(px(2.))
            .child(self.button(Button::Close, window, cx));
        let row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_h(px(34.))
            .pt(px(4.))
            .pb(px(4.))
            .pl(px(10.))
            .pr(px(8.))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(26.))
                    .text_color(t.faint)
                    .child(icon(IconName::Search).size(px(15.))),
            )
            .child(field)
            .children(count)
            .child(nav)
            .child(toggles)
            .child(close);
        div()
            .id("term-dock-find")
            .track_focus(&self.focus)
            .key_context("TerminalFind")
            .capture_key_down(cx.guarded_listener(Self::key_down))
            .flex_none()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(t.line)
            .bg(t.pane)
            .child(row)
    }
}

impl Render for TerminalFindBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
