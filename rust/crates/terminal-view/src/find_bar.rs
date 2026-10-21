//! The terminal find bar (`.term-dock-find.cm-recur-search` in `TerminalDock.svelte`): query
//! field, match count, previous / next, match-case / regexp / whole-word toggles and close.
//!
//! The dock owns when it opens and which pane it searches; the bar owns its fields and keys
//! (Enter / Shift+Enter step, Escape closes, ⌥⌘C / ⌥⌘R / ⌥⌘W toggle while it has focus).

use std::rc::Rc;

use gpui_kit::base::input::{Input, InputEditorStyle, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyView, App, AppContext, Bounds, BoxShadow, Context, Element, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, InspectorElementId, InteractiveElement, IntoElement,
    KeyDownEvent, LayoutId, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Style, Styled, Subscription, TransformationMatrix, Window, div,
    point, px,
};

use crate::element::gpui_color;
use crate::tokens::{Tokens, is_light_theme, mix, tokens};
use crate::view::{FindOptions, find_count_label};

/// Builds the themed hover tip for a control's title (the app's `TitleTooltip`).
pub type TooltipBuilder = Rc<dyn Fn(SharedString, &mut Window, &mut App) -> AnyView>;

/// What the dock reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindBarEvent {
    /// Query or toggles changed: run an incremental `find_next`.
    Changed,
    Next,
    Previous,
    Close,
}

/// `search`, `chevron-left`, `chevron-right`, `case-sensitive`, `regex`, `whole-word` and `x`
/// from `lib/icons.ts`, as Recur's `Icon.svelte` renders them.
pub mod icons {
    pub const SEARCH: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="6"/><path d="M16 16l4 4"/></svg>"#;
    pub const CHEVRON_LEFT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 18l-6-6 6-6"/></svg>"#;
    pub const CHEVRON_RIGHT: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>"#;
    pub const CASE_SENSITIVE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m2 16 4.039-9.69a.5.5 0 0 1 .923 0L11 16"/><path d="M22 9v7"/><path d="M3.304 13h6.392"/><circle cx="18.5" cy="12.5" r="3.5"/></svg>"#;
    pub const REGEX: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3v10"/><path d="m12.67 5.5 8.66 5"/><path d="m12.67 10.5 8.66-5"/><path d="M9 17a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v2a2 2 0 0 0 2 2h2a2 2 0 0 0 2-2v-2z"/></svg>"#;
    pub const WHOLE_WORD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="7" cy="12" r="3"/><path d="M10 9v6"/><circle cx="17" cy="12" r="3"/><path d="M14 7v8"/><path d="M22 17v1c0 .5-.5 1-1 1H3c-.5 0-1-.5-1-1v-1"/></svg>"#;
    pub const X: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 6l12 12M18 6L6 18"/></svg>"#;
}

/// A 15×15 SVG icon drawn in the current text colour (`.obs-icon-btn svg`).
pub struct Icon {
    name: &'static str,
    svg: &'static str,
    size: Pixels,
}

impl Icon {
    pub fn new(name: &'static str, svg: &'static str) -> Self {
        Self {
            name,
            svg,
            size: px(15.),
        }
    }
}

impl IntoElement for Icon {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Icon {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = self.size.into();
        style.size.height = self.size.into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let color = window.text_style().color;
        let path = SharedString::from(format!("recur-terminal-icon/{}.svg", self.name));
        let _ = window.paint_svg(
            bounds,
            path,
            Some(self.svg.as_bytes()),
            TransformationMatrix::unit(),
            color,
            cx,
        );
    }
}

pub struct TerminalFindBar {
    input: Entity<InputState>,
    focus: FocusHandle,
    case_sensitive: bool,
    regexp: bool,
    whole_word: bool,
    index: usize,
    total: usize,
    capped: bool,
    valid: bool,
    app_theme: String,
    tooltip: Option<TooltipBuilder>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FindBarEvent> for TerminalFindBar {}

impl Focusable for TerminalFindBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalFindBar {
    pub fn new(app_theme: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let subs =
            vec![
                cx.subscribe_in(&input, window, |bar, _, ev: &InputEvent, _w, cx| match ev {
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
                }),
            ];
        let mut bar = Self {
            input,
            focus: cx.focus_handle(),
            case_sensitive: false,
            regexp: false,
            whole_word: false,
            index: 0,
            total: 0,
            capped: false,
            valid: true,
            app_theme: app_theme.to_string(),
            tooltip: None,
            _subscriptions: subs,
        };
        bar.apply_editor_style(cx);
        bar
    }

    fn apply_editor_style(&mut self, cx: &mut Context<Self>) {
        let t = tokens(&self.app_theme);
        self.input.update(cx, |state, _| {
            state.set_editor_style(InputEditorStyle {
                foreground: gpui_color(t.text),
                muted_foreground: gpui_color(t.faint),
                background: gpui_color(input_bg(&self.app_theme, &t)),
                border: gpui_color(t.line),
                selection: gpui_color(t.amber_d),
                caret: gpui_color(t.text),
                ..InputEditorStyle::default()
            })
        });
    }

    pub fn set_app_theme(&mut self, theme: &str, cx: &mut Context<Self>) {
        if self.app_theme != theme {
            self.app_theme = theme.to_string();
            self.apply_editor_style(cx);
            cx.notify();
        }
    }

    pub fn set_tooltip(&mut self, tooltip: Option<TooltipBuilder>) {
        self.tooltip = tooltip;
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

    fn icon_button(
        &self,
        id: &'static str,
        title: &'static str,
        icon: Icon,
        on: Option<bool>,
        t: &Tokens,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let active = on == Some(true);
        let (pane_2, text) = (gpui_color(t.pane_2), gpui_color(t.text));
        let mut b = div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(26.))
            .rounded(px(5.))
            .text_color(gpui_color(if active { t.amber } else { t.faint }))
            .when(active, |d| d.bg(gpui_color(t.amber_d)))
            .when(!active, move |d| {
                d.hover(move |s| s.text_color(text).bg(pane_2))
            })
            .child(icon);
        if let Some(tip) = self.tooltip.clone() {
            b = b.tooltip(move |window, cx| tip(title.into(), window, cx));
        }
        b
    }
}

/// `var(--mono)`: JetBrains Mono with the platform monospace fallbacks.
fn mono_font(
    mut d: gpui_kit::Div,
    family: &SharedString,
    fallbacks: &gpui_kit::FontFallbacks,
) -> gpui_kit::Div {
    d = d.font_family(family.clone());
    d.style().text.font_fallbacks = Some(fallbacks.clone());
    d
}

fn input_bg(app_theme: &str, t: &Tokens) -> recur_term::screen::Rgba {
    if is_light_theme(app_theme) {
        t.pane
    } else {
        t.bg
    }
}

impl Render for TerminalFindBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(&self.app_theme);
        let mono: SharedString = "JetBrains Mono".into();
        let fallbacks = gpui_kit::FontFallbacks::from_fonts(
            crate::font::SYSTEM_MONO_FALLBACKS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let label = self.count_label(cx);
        let none = !self.query(cx).is_empty() && self.valid && self.total == 0;
        let border = if focused {
            mix(t.amber, t.line, 0.55)
        } else {
            t.line
        };
        let field = div()
            .flex_1()
            .min_w(px(80.))
            .h(px(28.))
            .px(px(9.))
            .flex()
            .items_center()
            .bg(gpui_color(input_bg(&self.app_theme, &t)))
            .border_1()
            .border_color(gpui_color(border))
            .rounded(px(6.))
            .when(focused, |d| {
                d.shadow(vec![BoxShadow {
                    color: gpui_color(t.amber_d),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                    inset: false,
                }])
            })
            .map(|d| mono_font(d, &mono, &fallbacks))
            .text_size(px(12.))
            .text_color(gpui_color(t.text))
            .child(Input::new(&self.input));
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_h(px(36.))
            .pt(px(5.))
            .pb(px(5.))
            .pl(px(10.))
            .pr(px(8.))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(26.))
                    .text_color(gpui_color(t.faint))
                    .child(Icon::new("search", icons::SEARCH)),
            )
            .child(field);
        if !label.is_empty() {
            let count = div()
                .flex_none()
                .map(|d| mono_font(d, &mono, &fallbacks))
                .text_size(px(10.5))
                .whitespace_nowrap()
                .text_color(gpui_color(if none { t.coral } else { t.muted }))
                .when(!none, |d| {
                    d.min_w(px(10.5 * 4.5)).px(px(2.)).flex().justify_end()
                })
                .when(none, |d| d.px(px(2.)))
                .child(label);
            row = row.child(count);
        }
        let ent = cx.entity();
        let (e1, e2, e3, e4, e5, e6) = (
            ent.clone(),
            ent.clone(),
            ent.clone(),
            ent.clone(),
            ent.clone(),
            ent,
        );
        let row = row
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        self.icon_button(
                            "term-find-prev",
                            "Previous match (⇧⌘G)",
                            Icon::new("chevron-left", icons::CHEVRON_LEFT),
                            None,
                            &t,
                        )
                        .on_click(move |_, _, cx| {
                            e1.update(cx, |_, cx| cx.emit(FindBarEvent::Previous))
                        }),
                    )
                    .child(
                        self.icon_button(
                            "term-find-next",
                            "Next match (⌘G)",
                            Icon::new("chevron-right", icons::CHEVRON_RIGHT),
                            None,
                            &t,
                        )
                        .on_click(move |_, _, cx| {
                            e2.update(cx, |_, cx| cx.emit(FindBarEvent::Next))
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        self.icon_button(
                            "term-find-case",
                            "Match case (⌥⌘C)",
                            Icon::new("case-sensitive", icons::CASE_SENSITIVE),
                            Some(self.case_sensitive),
                            &t,
                        )
                        .on_click(move |_, _, cx| e3.update(cx, |b, cx| b.toggle_case(cx))),
                    )
                    .child(
                        self.icon_button(
                            "term-find-regex",
                            "Regular expression (⌥⌘R)",
                            Icon::new("regex", icons::REGEX),
                            Some(self.regexp),
                            &t,
                        )
                        .on_click(move |_, _, cx| e4.update(cx, |b, cx| b.toggle_regexp(cx))),
                    )
                    .child(
                        self.icon_button(
                            "term-find-word",
                            "Whole word (⌥⌘W)",
                            Icon::new("whole-word", icons::WHOLE_WORD),
                            Some(self.whole_word),
                            &t,
                        )
                        .on_click(move |_, _, cx| e5.update(cx, |b, cx| b.toggle_whole_word(cx))),
                    ),
            )
            .child(
                div().ml(px(2.)).child(
                    self.icon_button(
                        "term-find-close",
                        "Close",
                        Icon::new("x", icons::X),
                        None,
                        &t,
                    )
                    .on_click(move |_, _, cx| e6.update(cx, |_, cx| cx.emit(FindBarEvent::Close))),
                ),
            );
        div()
            .id("term-dock-find")
            .track_focus(&self.focus)
            .key_context("TerminalFind")
            .capture_key_down(cx.listener(Self::key_down))
            .flex_none()
            .border_b_1()
            .border_color(gpui_color(t.line))
            .bg(gpui_color(t.pane))
            .child(div().flex().flex_col().child(row))
    }
}
