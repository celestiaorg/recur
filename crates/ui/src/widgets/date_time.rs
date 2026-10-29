//! `<input type=date>` / `<input type=time>` as WebKit draws them on macOS: the locale's short
//! format as focusable fields (tabular digits, 1px padding, the focused one white on the
//! control accent), literals between them, placeholders for empty fields (today's date,
//! `12:30 PM`) in `datePlaceholderTextColor`, and the field keys of
//! [`crate::logic::date_time`]. A date input opens its picker on click or Space: the native
//! calendar on macOS, WebKitGTK's calendar popover elsewhere ([`super::calendar`]); time
//! inputs have no picker.

use std::rc::Rc;
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Bounds, Context, EventEmitter, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, canvas, div, px,
};

use super::calendar::{self, Calendar, CalendarAction};
use crate::color::{Color, hex};
use crate::guard::Guarded as _;
use crate::logic::date_time::{Editor, FieldKind, Kind, Part};
use crate::text::{TextStyled, text};

/// `-apple-system-control-accent` as WebKit resolves it for a focused field.
const ACCENT: Color = hex(0x007aff);
/// The `outline: auto` ring of a keyboard-focused field: 3px of the focus ring color.
const RING: Color = crate::color::rgba(26, 169, 255, 0.5);
const RING_WIDTH: f32 = 3.;
/// `datePlaceholderColorLightnessAdjustmentFactor`.
const PLACEHOLDER_LIGHTNESS: f32 = 0.66;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DateTimeEvent {
    /// The value after an edit: `YYYY-MM-DD` / `HH:MM`, or empty while incomplete.
    Change(String),
}

/// The input's look: text color and size (the box around it is the host's).
#[derive(Clone, Copy, PartialEq)]
pub struct DateTimeStyle {
    pub color: Color,
    pub size: f32,
    /// The UA's `font-variant-numeric: tabular-nums`, unless the page's `font` shorthand reset
    /// it (`.sc-in`'s `font: inherit` does).
    pub tabular: bool,
    /// Horizontal padding and height of the input's box, which anchors the picker.
    pub pad_x: f32,
    pub height: f32,
    /// `color-scheme: dark` for the picker.
    pub dark: bool,
}

pub struct DateTimeField {
    id: SharedString,
    editor: Editor,
    handles: Vec<FocusHandle>,
    focused: Option<usize>,
    /// `:focus-visible`, decided when a field took focus (by keyboard).
    ring: bool,
    emitted: String,
    style: DateTimeStyle,
    bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    #[cfg(target_os = "macos")]
    picker: Option<super::date_picker_mac::Picker>,
    _poll: Option<Task<()>>,
    /// The calendar popover's month and selection while it is open.
    calendar: Option<Calendar>,
    calendar_focus: FocusHandle,
    /// The platform's native picker (macOS) rather than the popover.
    native: bool,
}

impl EventEmitter<DateTimeEvent> for DateTimeField {}

/// The locale's short date or time pattern and AM/PM labels, as WebKit's `LocaleCocoa` reads
/// them from `NSDateFormatter`.
#[cfg(target_os = "macos")]
fn locale_format(kind: Kind) -> (String, [String; 2]) {
    use objc2_foundation::{NSDateFormatter, NSDateFormatterStyle};
    let f = NSDateFormatter::new();
    match kind {
        Kind::Date => {
            f.setDateStyle(NSDateFormatterStyle::ShortStyle);
            f.setTimeStyle(NSDateFormatterStyle::NoStyle);
        }
        Kind::Time => {
            f.setDateStyle(NSDateFormatterStyle::NoStyle);
            f.setTimeStyle(NSDateFormatterStyle::ShortStyle);
        }
    }
    let pattern = f.dateFormat().to_string();
    (
        pattern,
        [f.AMSymbol().to_string(), f.PMSymbol().to_string()],
    )
}

#[cfg(not(target_os = "macos"))]
fn locale_format(kind: Kind) -> (String, [String; 2]) {
    // simplify: en-US patterns. WebKitGTK reads the ICU locale's short formats; upgrade by
    // reading them from ICU4X for `LC_TIME`.
    let pattern = match kind {
        Kind::Date => "M/d/y",
        Kind::Time => "h:mm a",
    };
    (pattern.into(), ["AM".into(), "PM".into()])
}

fn today() -> (i32, u32, u32) {
    use chrono::Datelike as _;
    let d = chrono::Local::now().date_naive();
    (d.year(), d.month(), d.day())
}

/// `RenderTheme::datePlaceholderTextColor` over a transparent background (luminance 0):
/// the text color's HSL lightness times 0.66.
pub fn placeholder_color(text: Color) -> Color {
    let mut h = Hsla::from(text);
    h.l *= PLACEHOLDER_LIGHTNESS;
    Color::from_hsla(h)
}

impl DateTimeField {
    pub fn new(
        id: impl Into<SharedString>,
        kind: Kind,
        style: DateTimeStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let (pattern, meridiem) = locale_format(kind);
        Self::with_pattern(id, kind, &pattern, meridiem, style, cx)
    }

    /// A field over an explicit pattern (tests and gallery scenes pin the locale).
    pub fn with_pattern(
        id: impl Into<SharedString>,
        kind: Kind,
        pattern: &str,
        meridiem: [String; 2],
        style: DateTimeStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = Editor::new(kind, pattern, meridiem, today());
        let handles = editor.fields.iter().map(|_| cx.focus_handle()).collect();
        Self {
            id: id.into(),
            editor,
            handles,
            focused: None,
            ring: false,
            emitted: String::new(),
            style,
            bounds: Rc::default(),
            #[cfg(target_os = "macos")]
            picker: None,
            _poll: None,
            calendar: None,
            calendar_focus: cx.focus_handle(),
            native: cfg!(target_os = "macos"),
        }
    }

    /// WebKitGTK's calendar popover in place of the native picker (specs drive it on macOS).
    pub fn with_popover(mut self) -> Self {
        self.native = false;
        self
    }

    pub fn value(&self) -> String {
        self.editor.value()
    }

    /// The page setting the value (no change event).
    pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        if self.editor.value() == value && !value.is_empty() {
            return;
        }
        self.editor.set_value(value);
        self.emitted = self.editor.value();
        cx.notify();
    }

    pub fn set_style(&mut self, style: DateTimeStyle, cx: &mut Context<Self>) {
        if self.style != style {
            self.style = style;
            cx.notify();
        }
    }

    /// One focus stop per field, in order, for the host's Tab cycle.
    pub fn focus_handles(&self) -> Vec<FocusHandle> {
        self.handles.clone()
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.handles.iter().any(|h| h.is_focused(window))
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let v = self.editor.value();
        if v != self.emitted {
            self.emitted = v.clone();
            cx.emit(DateTimeEvent::Change(v));
        }
        cx.notify();
    }

    fn focus_field(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(h) = self.handles.get(i) {
            window.focus(h, cx);
        }
    }

    fn key(&mut self, i: usize, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &e.keystroke.modifiers;
        if m.platform || m.control || m.function {
            return;
        }
        let key = e.keystroke.key.as_str();
        let last = self.handles.len().saturating_sub(1);
        match key {
            "left" => {
                if i > 0 {
                    self.focus_field(i - 1, window, cx);
                    cx.stop_propagation();
                }
                return;
            }
            "right" | "," | "-" | "." | "/" | ";" => {
                if i < last {
                    self.focus_field(i + 1, window, cx);
                    cx.stop_propagation();
                }
                return;
            }
            "up" | "down" => {
                self.editor.step(i, key == "up");
                self.changed(cx);
                cx.stop_propagation();
                return;
            }
            "backspace" | "delete" => {
                if let Some(f) = self.editor.fields.get_mut(i) {
                    f.clear();
                }
                self.changed(cx);
                cx.stop_propagation();
                return;
            }
            "space" if self.editor.kind == Kind::Date => {
                self.open_picker(true, window, cx);
                cx.stop_propagation();
                return;
            }
            "escape" if self.picker_open() => {
                self.close_picker(cx);
                cx.stop_propagation();
                return;
            }
            _ => {}
        }
        let typed = e.keystroke.key_char.as_deref().or_else(|| {
            let k = e.keystroke.key.as_str();
            (k.chars().count() == 1).then_some(k)
        });
        if let Some(c) = typed.and_then(|s| s.chars().next())
            && !c.is_control()
            && self.editor.type_char(i, c, Instant::now())
        {
            self.changed(cx);
            cx.stop_propagation();
        }
    }

    /// The calendar picker is open (it takes Escape before the page does).
    pub fn picker_open(&self) -> bool {
        #[cfg(target_os = "macos")]
        if self.picker.is_some() {
            return true;
        }
        self.calendar.is_some()
    }

    fn close_picker(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            self.picker = None;
        }
        self._poll = None;
        self.calendar = None;
        cx.notify();
    }

    /// `showPicker`: the calendar under the input, at its value (today when empty).
    fn open_picker(&mut self, keyboard: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.kind != Kind::Date {
            return;
        }
        if !self.native {
            if let Some(i) = self.handles.iter().position(|h| h.is_focused(window)) {
                self.focused = Some(i);
            }
            // A GtkPopover takes the keyboard however it opened.
            let (y, m, d) =
                crate::logic::date_time::parse_date(&self.editor.value()).unwrap_or_else(today);
            self.calendar.get_or_insert(Calendar::new(y, m, d));
            window.focus(&self.calendar_focus, cx);
            cx.notify();
            return;
        }
        self.open_native(keyboard, window, cx);
    }

    #[cfg(not(target_os = "macos"))]
    fn open_native(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) {}

    #[cfg(target_os = "macos")]
    fn open_native(&mut self, keyboard: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(anchor) = self.bounds.get() else {
            return;
        };
        if let Some(p) = &self.picker {
            p.set_value(&self.editor.value());
            return;
        }
        let Some(p) = super::date_picker_mac::Picker::open(
            anchor,
            &self.editor.value(),
            self.style.dark,
            keyboard,
            window,
        ) else {
            return;
        };
        let mut last = p.value();
        self.picker = Some(p);
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(60))
                    .await;
                let live = this.update(cx, |this, cx| {
                    let Some(p) = &this.picker else {
                        return false;
                    };
                    if p.escaped() {
                        this.close_picker(cx);
                        return false;
                    }
                    let v = p.value();
                    if v != last {
                        // `didChooseValue`: the page sets the value with input and change.
                        last = v.clone();
                        this.editor.set_value(&v);
                        this.changed(cx);
                    }
                    true
                });
                if !matches!(live, Ok(true)) {
                    return;
                }
            }
        }));
    }

    /// A day chosen in the popover sets the value (`didChooseValue`); closing it hands the
    /// keyboard back to the field it came from.
    fn calendar_action(
        &mut self,
        c: Calendar,
        action: CalendarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            CalendarAction::Changed { select } => {
                self.calendar = Some(c);
                if select {
                    self.editor.set_value(&c.value());
                    self.changed(cx);
                }
                cx.notify();
            }
            CalendarAction::Close => {
                self.calendar = None;
                let back = self.focused.unwrap_or(0);
                self.focus_field(back, window, cx);
                cx.notify();
            }
        }
    }

    fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // A field that lost focus drops its type-ahead; the control losing focus ends the
        // picker (`didBlurFromControl`).
        let now = self.handles.iter().position(|h| h.is_focused(window));
        let in_popover = self.calendar_focus.is_focused(window);
        if now != self.focused && !in_popover {
            if let Some(f) = self.focused.and_then(|i| self.editor.fields.get_mut(i)) {
                f.blur();
            }
            if now.is_none() && self.picker_open() {
                #[cfg(target_os = "macos")]
                {
                    self.picker = None;
                }
                self._poll = None;
                self.calendar = None;
            }
            self.focused = now;
            self.ring = window.last_input_was_keyboard();
        }
        let now = if in_popover { None } else { now };
        let s = self.style;
        let placeholder = placeholder_color(s.color);
        let label = |t: String| {
            let t = text(t).size(s.size);
            if s.tabular { t.tabular() } else { t }
        };
        let widest = |samples: &[String]| {
            samples
                .iter()
                .map(|t| label(t.clone()).measure(None, window).0)
                .fold(0f32, f32::max)
        };
        let ring = self.ring;
        let digits: Vec<String> = (0..10).map(|d| d.to_string()).collect();
        // `::-webkit-datetime-edit`: 1px padding, `overflow: hidden` (it clips the focus ring).
        let mut row = div()
            .flex()
            .items_center()
            .p(px(1.))
            .overflow_hidden()
            .whitespace_nowrap();
        let mut field = 0;
        for part in &self.editor.parts {
            match part {
                Part::Literal(l) => {
                    row = row.child(label(l.clone()).color(s.color));
                }
                Part::Field(kind) => {
                    let i = field;
                    field += 1;
                    let f = &self.editor.fields[i];
                    let min_w = match kind {
                        FieldKind::Meridiem => widest(&self.editor.meridiem),
                        k => {
                            let n = if matches!(k, FieldKind::Year) { 4 } else { 2 };
                            widest(&digits.iter().map(|d| d.repeat(n)).collect::<Vec<_>>())
                        }
                    };
                    let focused = now == Some(i);
                    let fg = if f.value.is_none() {
                        placeholder
                    } else if focused {
                        hex(0xffffff)
                    } else {
                        s.color
                    };
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("{}-f{i}", self.id)))
                            .debug_selector({
                                let id = self.id.clone();
                                move || format!("{id}:field{i}")
                            })
                            .track_focus(&self.handles[i])
                            .key_context("TextEntry")
                            .relative()
                            .flex()
                            .justify_center()
                            .p(px(1.))
                            .min_w(px(min_w + 2.))
                            .rounded(px(3.))
                            .when(focused, |d| d.bg(Hsla::from(ACCENT)))
                            .child(label(f.text(&self.editor.meridiem)).color(fg))
                            .when(focused && ring, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(-RING_WIDTH))
                                        .left(px(-RING_WIDTH))
                                        .right(px(-RING_WIDTH))
                                        .bottom(px(-RING_WIDTH))
                                        .border(px(RING_WIDTH))
                                        .border_color(Hsla::from(RING))
                                        .rounded(px(3. + RING_WIDTH)),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.guarded_listener(move |this, _, window, cx| {
                                    this.focus_field(i, window, cx);
                                }),
                            )
                            .on_key_down(cx.guarded_listener(
                                move |this, e: &KeyDownEvent, window, cx| {
                                    this.key(i, e, window, cx);
                                },
                            )),
                    );
                }
            }
        }
        let bounds = self.bounds.clone();
        let popover = self.calendar.map(|c| {
            let this = cx.entity().downgrade();
            div()
                .absolute()
                .top(px(s.height))
                .left_0()
                .child(gpui_kit::deferred(calendar::render(
                    &format!("{}-calendar", self.id),
                    c,
                    today(),
                    s.dark,
                    &self.calendar_focus,
                    move |c, action, window, cx| {
                        let _ = this.update(cx, |f, cx| f.calendar_action(c, action, window, cx));
                    },
                )))
        });
        div()
            .id(self.id.clone())
            .relative()
            .flex()
            .items_center()
            .h(px(s.height))
            .px(px(s.pad_x))
            .child(
                canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .child(row)
            .children(popover)
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|this, _, window, cx| {
                    if !this.is_focused(window) {
                        this.focus_field(0, window, cx);
                    }
                }),
            )
            // `DOMActivate`: a click opens the picker.
            .on_click(cx.guarded_listener(|this, _, window, cx| {
                this.open_picker(false, window, cx);
            }))
            .into_any_element()
    }
}

impl Render for DateTimeField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_darken_the_text_color() {
        let p = placeholder_color(hex(0xaaaaaa));
        let r8 = (p.r * 255.).round() as u8;
        assert_eq!(r8, 112, "WebKit draws #aaa's placeholder at 112");
    }
}
