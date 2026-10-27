//! `Select.svelte`: the app-styled select (`.sel*` in `_forms.scss`). A trigger button and a
//! window-anchored menu with keyboard navigation, a hover gate, optional fuzzy search,
//! provider-grouped options, inline hints, and multi-select.

use crate::color::Color;
use crate::fonts::{MONO, UI};
use crate::guard::Guarded;
use crate::icon::{icon, provider_icon};
use crate::icon_name::IconName;
use crate::logic::fuzzy::{fuzzy_filter_prepared, prepare_fuzzy};
use crate::logic::hover_gate::HoverGate;
use crate::logic::menu_space::{
    MENU_GAP_PX, MenuVariant, clamp_menu_left, menu_fixed_box, menu_prefer_up,
};
use crate::motion::{self, Easing};
use crate::style::{faded, is_focus_visible};
use crate::text::{LineHeight, TextStyled, text};
use crate::theme::Theme;
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, Bounds, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseMoveEvent, ParentElement, Pixels,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    canvas, deferred, div, prelude::FluentBuilder, px,
};
use std::time::{Duration, Instant};

const SEARCH_PLACEHOLDER: &str = "Search…";

/// `SelectOption`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectOption {
    pub value: SharedString,
    pub label: SharedString,
    /// Consecutive options sharing a group render under one heading.
    pub group: Option<SharedString>,
    /// Provider slug: brand mark on the trigger and the group heading.
    pub provider: Option<SharedString>,
    /// App icon on the row and the closed trigger (chat modes).
    pub icon: Option<IconName>,
    /// Accent for icon and label (chat modes).
    pub tint: Option<Color>,
    /// One-line description after the label.
    pub hint: Option<SharedString>,
    /// `optionStyle`: the label's own font stack and size at line-height 1.45 (font pickers).
    pub font: Option<(crate::fonts::Family, f32)>,
}

impl SelectOption {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        SelectOption {
            value: value.into(),
            label: label.into(),
            ..Default::default()
        }
    }
    pub fn group(mut self, g: impl Into<SharedString>) -> Self {
        self.group = Some(g.into());
        self
    }
    pub fn provider(mut self, p: impl Into<SharedString>) -> Self {
        self.provider = Some(p.into());
        self
    }
    pub fn icon(mut self, i: IconName) -> Self {
        self.icon = Some(i);
        self
    }
    pub fn tint(mut self, c: Color) -> Self {
        self.tint = Some(c);
        self
    }
    pub fn hint(mut self, h: impl Into<SharedString>) -> Self {
        self.hint = Some(h.into());
        self
    }
    /// Draw the label in `family` at `size` px, line-height 1.45 (`optionStyle`).
    pub fn font(mut self, family: crate::fonts::Family, size: f32) -> Self {
        self.font = Some((family, size));
        self
    }
}

/// `size`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectSize {
    #[default]
    Md,
    Sm,
}

/// Change notifications.
#[derive(Clone, Debug, PartialEq)]
pub enum SelectEvent {
    /// `onchange` (single select).
    Change(SharedString),
    /// `onvalues` (multiple).
    Values(Vec<SharedString>),
}

/// Context restyling of the trigger button (`.crumb-base .sel-btn`, `.nt-pickers .sel-btn`,
/// `.issue-pick .sel-btn`, …): the view that places the select supplies the overrides its CSS
/// context applies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriggerStyle {
    pub height: Option<f32>,
    /// Square left corners (the right half of a split button).
    pub square_left: bool,
    /// `height: auto` with this vertical padding (the trigger hugs its content).
    pub pad_y: Option<f32>,
    pub pad_x: Option<f32>,
    pub radius: Option<f32>,
    pub gap: Option<f32>,
    pub font_size: Option<f32>,
    /// `font-family: inherit` (UI) instead of mono.
    pub ui_font: bool,
    /// Mono even where the size sets the UI font (`.compat-flag-card .sel-btn`).
    pub mono_font: bool,
    pub weight: Option<f32>,
    /// Background, border and text at rest (None keeps the variant's).
    pub bg: Option<Color>,
    pub border: Option<Color>,
    pub color: Option<Color>,
    pub hover_bg: Option<Color>,
    pub hover_border: Option<Color>,
    pub hover_color: Option<Color>,
    /// Open / keyboard-focused look; `None` keeps the amber ring.
    pub open_bg: Option<Color>,
    pub open_border: Option<Color>,
    pub open_color: Option<Color>,
    pub no_ring: bool,
    pub icon_color: Option<Color>,
    pub icon_size: Option<f32>,
    pub chevron_size: Option<f32>,
    /// Icon and chevron color while open / focused.
    pub open_icon_color: Option<Color>,
    /// The chevron's, where it differs from the icon's.
    pub open_chevron_color: Option<Color>,
    /// While open, `.sel.open`'s ring and amber icons win over the host's (a later sheet, same
    /// specificity); `no_ring` and `open_icon_color` then apply to `:focus-visible` only.
    pub sel_open_wins: bool,
    /// The value keeps its natural width (`flex: none`).
    pub value_auto: bool,
    pub auto_width: bool,
    /// No value text (`.sel-val { display: none }`): the icon and chevron only.
    pub hide_value: bool,
    /// A cap on the select (`max-width` on `.sel`; the trigger fills it at most).
    pub max_width: Option<f32>,
    /// The value shrinks under that cap and ellipsizes (`.sel-val { flex: 0 1 auto;
    /// min-width: 0 }`).
    pub value_shrink: bool,
    /// No `:focus-visible` outline (`.nt-modal .sel-btn:focus-visible { outline: none }`); the
    /// host draws its own keyboard ring.
    pub no_outline: bool,
    /// `min-height` with `pad_y` (`.editor-prefs .font-sel .sel-btn`).
    pub min_height: Option<f32>,
    /// Inside a modal dialog (`.nt-modal .sel-btn.kb-focus`): reached by Tab, the trigger
    /// shows a 2px `--amber` ring 2px out instead of the global `:focus-visible` ring.
    pub kb_ring: bool,
}

/// A trigger icon drawn by the owner (the `icon` snippet: `.nt-pic` project pictures).
pub type IconSlot = std::rc::Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// The select. Create with [`Select::new`] in an owning view, keep the entity, and render it.
pub struct Select {
    focus: FocusHandle,
    options: Vec<SelectOption>,
    value: SharedString,
    values: Vec<SharedString>,
    disabled_over: Option<Color>,
    label: SharedString,
    size: SelectSize,
    ghost: bool,
    up: bool,
    searchable: bool,
    multiple: bool,
    placeholder: SharedString,
    variant: MenuVariant,
    trigger: TriggerStyle,
    custom_icon: Option<IconName>,
    icon_slot: Option<IconSlot>,
    open: bool,
    opened_at: Option<Instant>,
    closed_at: Option<Instant>,
    active: Option<usize>,
    gate: HoverGate,
    search: Entity<InputState>,
    query: SharedString,
    trigger_bounds: Option<Bounds<Pixels>>,
    list_scroll: ScrollHandle,
    display_override: Option<SharedString>,
    value_font: Option<(crate::fonts::Family, f32)>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SelectEvent> for Select {}

impl Focusable for Select {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `.sel-search`: the search header row with its magnifier and borderless input.
pub(crate) fn search_input(
    window: &mut Window,
    cx: &mut App,
    placeholder: &str,
) -> Entity<InputState> {
    crate::style::text_input(&placeholder.to_string().into(), window, cx)
}

pub(crate) fn style_search_input(input: &Entity<InputState>, cx: &mut App) {
    crate::style::style_text_input(input, cx);
}

/// `.sel-search`, flush with the menu's border: its `margin: -4px -4px 0` cancels the menu
/// padding, which a menu with a search header moves onto its list instead (Taffy sizes a flex
/// column wrongly around negative margins).
pub(crate) fn search_header(
    input: &Entity<InputState>,
    placeholder: &SharedString,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .py(px(8.))
        .px(px(10.))
        // GPUI clips rectangles: round the corners under the menu's 8px radius and 1px border.
        .rounded_t(px(7.))
        .bg(t.pane_2)
        .border_b(px(1.))
        .border_color(Hsla::from(t.line))
        .child(icon(IconName::Search).size(px(13.)).color(t.faint))
        .child(crate::style::text_field(
            input,
            placeholder,
            t.faint,
            crate::fonts::Family::Ui,
            12.5,
            window,
            cx,
        ))
        .into_any_element()
}

struct Row {
    ix: usize,
    heading: Option<SharedString>,
    heading_provider: Option<SharedString>,
}

impl Select {
    pub fn new(options: Vec<SelectOption>, window: &mut Window, cx: &mut Context<Self>) -> Select {
        let search = search_input(window, cx, SEARCH_PLACEHOLDER);
        let sub = cx.guarded_subscribe_in(
            &search,
            window,
            |this: &mut Select, input, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.query = input.read(cx).value();
                    // The filter changed: clamp the active row and re-gate hover.
                    let n = this.filtered().len();
                    this.active = if n == 0 {
                        None
                    } else {
                        Some(this.active.unwrap_or(0).min(n - 1))
                    };
                    if this.active.is_none() && n > 0 {
                        this.active = Some(0);
                    }
                    this.gate.reset();
                    cx.notify();
                }
            },
        );
        Select {
            // A `<button>`: in the Tab order.
            focus: cx.focus_handle().tab_stop(true),
            options,
            value: SharedString::default(),
            values: Vec::new(),
            disabled_over: None,
            label: SharedString::default(),
            size: SelectSize::Md,
            ghost: false,
            up: false,
            searchable: false,
            multiple: false,
            placeholder: SharedString::default(),
            variant: MenuVariant::Default,
            trigger: TriggerStyle::default(),
            custom_icon: None,
            icon_slot: None,
            open: false,
            opened_at: None,
            closed_at: None,
            active: None,
            gate: HoverGate::default(),
            search,
            query: SharedString::default(),
            trigger_bounds: None,
            list_scroll: ScrollHandle::new(),
            display_override: None,
            value_font: None,
            _subs: vec![sub],
        }
    }

    // Owners push these from their own render: an unchanged value must not notify, or the
    // select (and every view above it) re-renders on the next frame for nothing.
    pub fn set_options(&mut self, options: Vec<SelectOption>, cx: &mut Context<Self>) {
        if self.options != options {
            self.options = options;
            cx.notify();
        }
    }
    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        let value = value.into();
        if self.value != value {
            self.value = value;
            cx.notify();
        }
    }
    pub fn set_values(&mut self, values: Vec<SharedString>, cx: &mut Context<Self>) {
        if self.values != values {
            self.values = values;
            cx.notify();
        }
    }
    /// The closed trigger's text, when the owner summarizes the selection itself
    /// (`ModelFlagsSelect`'s `Fast · High`).
    pub fn set_display(&mut self, display: Option<SharedString>, cx: &mut Context<Self>) {
        if self.display_override == display {
            return;
        }
        self.display_override = display;
        cx.notify();
    }
    /// `valueStyle`: the closed trigger's value in `family` at `size` px.
    pub fn set_value_font(
        &mut self,
        font: Option<(crate::fonts::Family, f32)>,
        cx: &mut Context<Self>,
    ) {
        if self.value_font != font {
            self.value_font = font;
            cx.notify();
        }
    }
    pub fn value(&self) -> &SharedString {
        &self.value
    }
    pub fn values(&self) -> &[SharedString] {
        &self.values
    }
    /// Disabled, faded over the color it sits on.
    /// Restyle the trigger as its context changes (a container width crossing a breakpoint).
    pub fn set_trigger_style(&mut self, t: TriggerStyle, cx: &mut Context<Self>) {
        if self.trigger != t {
            self.trigger = t;
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn trigger(&self) -> &TriggerStyle {
        &self.trigger
    }

    /// Swap the trigger's custom icon (a sort control showing the current order's icon).
    /// `portalMenuMods`' menu class for the context the select sits in.
    pub fn set_menu_variant(&mut self, v: MenuVariant, cx: &mut Context<Self>) {
        self.variant = v;
        cx.notify();
    }
    /// The `icon` snippet as an element (wins over [`Select::set_icon`]).
    pub fn set_icon_slot(&mut self, slot: Option<IconSlot>, cx: &mut Context<Self>) {
        self.icon_slot = slot;
        cx.notify();
    }
    pub fn set_icon(&mut self, i: Option<IconName>, cx: &mut Context<Self>) {
        if self.custom_icon != i {
            self.custom_icon = i;
            cx.notify();
        }
    }

    pub fn set_disabled(&mut self, backdrop: Option<Color>, cx: &mut Context<Self>) {
        if self.disabled_over == backdrop && (backdrop.is_none() || !self.open) {
            return;
        }
        self.disabled_over = backdrop;
        if backdrop.is_some() {
            self.open = false;
        }
        cx.notify();
    }
    /// Builder-style configuration.
    pub fn label(mut self, l: impl Into<SharedString>) -> Self {
        self.label = l.into();
        self
    }
    pub fn size(mut self, s: SelectSize) -> Self {
        self.size = s;
        if s == SelectSize::Sm && self.variant == MenuVariant::Default {
            self.variant = MenuVariant::Small;
        }
        self
    }
    /// Borderless inline trigger (composer footer controls).
    pub fn ghost(mut self) -> Self {
        self.ghost = true;
        if self.variant == MenuVariant::Default {
            self.variant = MenuVariant::Ghost;
        }
        self
    }
    /// Force the menu above the trigger.
    pub fn up(mut self) -> Self {
        self.up = true;
        self
    }
    pub fn searchable(mut self) -> Self {
        self.searchable = true;
        self
    }
    pub fn multiple(mut self) -> Self {
        self.multiple = true;
        self
    }
    pub fn placeholder(mut self, p: impl Into<SharedString>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// The menu class the trigger's context implies (`portalMenuMods`).
    pub fn menu_variant(mut self, v: MenuVariant) -> Self {
        self.variant = v;
        self
    }
    pub fn trigger_style(mut self, t: TriggerStyle) -> Self {
        self.trigger = t;
        self
    }
    /// Override the closed-button icon.
    pub fn trigger_icon(mut self, i: IconName) -> Self {
        self.custom_icon = Some(i);
        self
    }
    pub fn with_value(mut self, v: impl Into<SharedString>) -> Self {
        self.value = v.into();
        self
    }
    pub fn with_values(mut self, v: Vec<SharedString>) -> Self {
        self.values = v;
        self
    }

    fn selected(&self, v: &str) -> bool {
        if self.multiple {
            self.values.iter().any(|x| x == v)
        } else {
            self.value == v
        }
    }

    fn chosen(&self) -> Vec<&SelectOption> {
        self.options
            .iter()
            .filter(|o| self.selected(&o.value))
            .collect()
    }

    fn display(&self) -> SharedString {
        if let Some(d) = &self.display_override {
            return d.clone();
        }
        let chosen = self.chosen();
        let shown: SharedString = if self.multiple {
            chosen
                .iter()
                .map(|o| o.label.to_string())
                .collect::<Vec<_>>()
                .join(", ")
                .into()
        } else {
            chosen
                .first()
                .map_or(self.value.clone(), |o| o.label.clone())
        };
        if !shown.is_empty() {
            shown
        } else if !self.placeholder.is_empty() {
            self.placeholder.clone()
        } else {
            "—".into()
        }
    }

    fn filtered(&self) -> Vec<SelectOption> {
        if !self.searchable || self.query.trim().is_empty() {
            return self.options.clone();
        }
        let prepared = prepare_fuzzy(&self.options, |o| o.label.to_string());
        fuzzy_filter_prepared(&prepared, &self.query, self.options.len())
            .into_iter()
            .map(|h| h.item)
            .collect()
    }

    fn rows(filtered: &[SelectOption]) -> Vec<Row> {
        filtered
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let new_group = o.group.is_some()
                    && o.group != i.checked_sub(1).and_then(|p| filtered[p].group.clone());
                Row {
                    ix: i,
                    heading: if new_group { o.group.clone() } else { None },
                    heading_provider: if new_group { o.provider.clone() } else { None },
                }
            })
            .collect()
    }

    fn open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.opened_at = Some(Instant::now());
        self.query = SharedString::default();
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        let first = self.options.iter().position(|o| self.selected(&o.value));
        self.active = Some(first.unwrap_or(0));
        self.gate.reset();
        if self.searchable {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        } else if let Some(a) = self.active {
            self.list_scroll.scroll_to_item(a);
        }
        cx.notify();
    }

    /// `close(restoreFocus)`.
    pub fn close(&mut self, restore_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.closed_at = Some(Instant::now());
        self.active = None;
        self.query = SharedString::default();
        if restore_focus {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled_over.is_some() || self.options.is_empty() {
            return;
        }
        if self.open {
            self.close(false, window, cx);
        } else {
            self.open_menu(window, cx);
        }
    }

    fn pick(&mut self, v: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiple {
            if let Some(i) = self.values.iter().position(|x| *x == v) {
                self.values.remove(i);
            } else {
                self.values.push(v);
            }
            cx.emit(SelectEvent::Values(self.values.clone()));
            cx.notify();
            return;
        }
        self.value = v.clone();
        cx.emit(SelectEvent::Change(v));
        self.close(true, window, cx);
    }

    fn move_active(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.active = None;
            return;
        }
        self.gate.reset();
        let next = match self.active {
            None if delta > 0 => 0,
            None => len as isize - 1,
            Some(a) => a as isize + delta,
        };
        let a = next.clamp(0, len as isize - 1) as usize;
        self.active = Some(a);
        self.list_scroll.scroll_to_item(a);
    }

    fn jump(&mut self, i: usize) {
        self.gate.reset();
        self.active = Some(i);
        self.list_scroll.scroll_to_item(i);
    }

    /// `onKey` / `onSearchKey`.
    fn on_key(
        &mut self,
        e: &KeyDownEvent,
        from_search: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.disabled_over.is_some() {
            return false;
        }
        let key = e.keystroke.key.as_str();
        if !self.open {
            if !from_search && matches!(key, "down" | "up" | "enter" | "space") {
                self.open_menu(window, cx);
                return true;
            }
            return false;
        }
        if key == "escape" {
            self.close(true, window, cx);
            return true;
        }
        if key == "tab" {
            self.close(true, window, cx);
            return false;
        }
        // A searchable menu's trigger only handles Escape; the field handles the rest.
        if self.searchable && !from_search {
            return false;
        }
        let filtered = self.filtered();
        let len = filtered.len();
        let handled = match key {
            "down" => {
                self.move_active(1, len);
                true
            }
            "up" => {
                self.move_active(-1, len);
                true
            }
            "enter" => {
                if let Some(o) = self.active.and_then(|a| filtered.get(a)) {
                    self.pick(o.value.clone(), window, cx);
                }
                true
            }
            "space" if !from_search => {
                if let Some(o) = self.active.and_then(|a| filtered.get(a)) {
                    self.pick(o.value.clone(), window, cx);
                }
                true
            }
            "home" if !from_search || self.query.is_empty() => {
                if len > 0 {
                    self.jump(0);
                }
                true
            }
            "end" if !from_search || self.query.is_empty() => {
                if len > 0 {
                    self.jump(len - 1);
                }
                true
            }
            _ => false,
        };
        if handled {
            cx.notify();
        }
        handled
    }
}

impl Select {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        style_search_input(&self.search, cx);
        let t = Theme::of(cx).tokens;
        let disabled = self.disabled_over.is_some();
        let sm = self.size == SelectSize::Sm;
        let focus_visible = is_focus_visible(&self.focus, window);
        let lit = self.open || focus_visible;
        let chosen: Vec<SelectOption> = self.chosen().into_iter().cloned().collect();
        let first = chosen.first().cloned();
        let tr = self.trigger.clone();

        // Variant metrics.
        let (h, pad, radius, gap, size, ui) = if self.ghost {
            (24., 6., 6., 5., 12., true)
        } else if sm {
            (30., 10., 6., 7., 12., true)
        } else {
            (38., 12., 7., 9., 12., false)
        };
        // The global `:focus-visible { border-radius: 4px }` (later in the cascade, same
        // specificity) rounds a keyboard-focused trigger less, unless a context sets its own.
        let radius = if focus_visible { 4. } else { radius };
        let (h, pad, radius, gap, size) = (
            tr.height.unwrap_or(h),
            tr.pad_x.unwrap_or(pad),
            tr.radius.unwrap_or(radius),
            tr.gap.unwrap_or(gap),
            tr.font_size.unwrap_or(size),
        );
        let ui = (ui || tr.ui_font) && !tr.mono_font;
        let (mut bg, mut border, mut fg) = if self.ghost {
            (
                crate::color::TRANSPARENT,
                crate::color::TRANSPARENT,
                t.muted,
            )
        } else {
            (t.pane, t.line, t.text)
        };
        bg = tr.bg.unwrap_or(bg);
        border = tr.border.unwrap_or(border);
        fg = tr.color.unwrap_or(fg);
        let mut ring = false;
        if lit && !disabled {
            if self.ghost {
                if self.open {
                    bg = tr.open_bg.unwrap_or(t.pane_2);
                    fg = tr.open_color.unwrap_or(t.text);
                }
            } else {
                bg = tr.open_bg.unwrap_or(bg);
                fg = tr.open_color.unwrap_or(fg);
                border = tr
                    .open_border
                    .unwrap_or(crate::color::rgba(242, 179, 61, 0.55));
                ring = (tr.sel_open_wins && self.open) || !tr.no_ring;
            }
        }
        let mut ico_color = tr.icon_color.unwrap_or(t.muted);
        let mut chev_color = t.faint;
        // `.sel.open` / `:focus-visible` icons go amber; a ghost's open icons stay muted.
        if !disabled && (self.open || focus_visible) {
            let own = tr
                .open_icon_color
                .filter(|_| !(tr.sel_open_wins && self.open));
            let c = own.unwrap_or(if self.ghost && self.open {
                t.muted
            } else {
                t.amber
            });
            ico_color = c;
            chev_color = tr
                .open_chevron_color
                .filter(|_| !(tr.sel_open_wins && self.open))
                .unwrap_or(c);
        }
        let (hover_bg, hover_border, hover_fg) = if self.ghost {
            (
                tr.hover_bg.unwrap_or(t.pane_2),
                tr.hover_border.unwrap_or(crate::color::TRANSPARENT),
                tr.hover_color.unwrap_or(t.text),
            )
        } else {
            (
                tr.hover_bg.unwrap_or(t.pane_2),
                tr.hover_border.unwrap_or(t.line_bright),
                tr.hover_color.unwrap_or(fg),
            )
        };
        // `.sel.ghost .sel-btn { transition: color .15s ease, background .15s ease }`.
        let ghost_hover = (self.ghost && !disabled).then(|| {
            motion::hover(
                "sel-btn-hover",
                Duration::from_millis(150),
                Easing::EASE,
                window,
                cx,
            )
        });
        if let Some(h) = &ghost_hover {
            bg = hover_bg.mix(h.k(), bg);
            fg = hover_fg.mix(h.k(), fg);
        }
        // `.sel-btn:disabled { opacity: .55 }`.
        if let Some(back) = self.disabled_over {
            let fill = bg;
            bg = faded(bg, bg, 0.55, back);
            border = faded(border, fill, 0.55, back);
            fg = faded(fg, fill, 0.55, back);
            ico_color = faded(ico_color, fill, 0.55, back);
            chev_color = faded(chev_color, fill, 0.55, back);
        }
        let ico_size = tr.icon_size.unwrap_or(if self.ghost { 12. } else { 14. });

        let border_w = if !self.ghost || tr.border.is_some() {
            1.
        } else {
            0.
        };
        let mut button = div()
            .id("sel-btn")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .items_center()
            .gap(px(gap))
            .map(|b| match tr.pad_y {
                Some(py) => b.py(px(py)),
                None => b.h(px(h)),
            })
            .when_some(tr.min_height, |b, mh| b.min_h(px(mh)))
            .px(px(pad))
            .rounded(px(radius))
            .when(tr.square_left, |b| b.rounded_l(px(0.)))
            .bg(bg)
            .when(border_w > 0., |d| d.border(px(border_w)))
            .border_color(Hsla::from(border))
            .text_color(fg)
            .font_family(if ui { UI } else { MONO })
            .text_size(px(size))
            .when_some(tr.weight, |d, w| d.font_weight(gpui_kit::FontWeight(w)))
            .role(gpui_kit::accesskit::Role::ComboBox)
            .aria_label(self.label.clone());
        // Spec handle: `sel:{aria label}={shown value}`, `:off` while disabled.
        let (label, shown) = (self.label.clone(), self.display());
        button = button.debug_selector(move || {
            format!("sel:{label}={shown}{}", if disabled { ":off" } else { "" })
        });
        button = if self.ghost || tr.auto_width {
            button.flex_none()
        } else {
            button.w_full()
        };
        if ring {
            button = button.relative().child(crate::shadow::box_shadow(
                &[crate::shadow::css_box_shadow(0., 0., 0., 3., t.amber_d)],
                radius,
                border_w,
            ));
        }
        // `:focus-visible`'s outline outranks `.sel-btn { outline: none }` by coming later.
        if focus_visible && !disabled && tr.kb_ring {
            button = button
                .relative()
                .child(crate::style::outline(2., 2., radius, border_w, t.amber));
        } else if focus_visible && !disabled && !tr.no_outline {
            button = button
                .relative()
                .child(crate::style::focus_ring(radius, border_w, cx));
        }
        if !disabled {
            button = button.cursor_pointer();
            // `.sel-btn:hover:not(:disabled)` outranks `:focus-visible`; `.sel.open .sel-btn`
            // ties it and comes later, so an open trigger keeps its amber border under hover.
            let keep_border = self.open && !self.ghost;
            button = match &ghost_hover {
                Some(h) => button.on_hover(h.listener()),
                None => button.hover(move |s| {
                    let s = s.bg(hover_bg).text_color(Hsla::from(hover_fg));
                    if keep_border {
                        s
                    } else {
                        s.border_color(Hsla::from(hover_border))
                    }
                }),
            };
        }
        // Icon: custom, the selected option's app icon (tinted), or its provider mark.
        if let Some(slot) = self.icon_slot.clone() {
            button = button.child(
                div()
                    .flex_none()
                    .text_color(ico_color)
                    .child(slot(window, cx)),
            );
        } else if let Some(i) = self.custom_icon {
            button = button.child(
                div()
                    .flex_none()
                    .text_color(ico_color)
                    .child(icon(i).size(px(ico_size))),
            );
        } else if let Some(o) = first.as_ref().filter(|o| o.icon.is_some()) {
            let c = o.tint.unwrap_or(ico_color);
            button = button.child(
                div().flex_none().child(
                    icon(o.icon.unwrap_or(IconName::Plus))
                        .size(px(ico_size))
                        .color(c),
                ),
            );
        } else if let Some(p) = first.as_ref().and_then(|o| o.provider.clone()) {
            button = button.child(
                div()
                    .flex_none()
                    .text_color(ico_color)
                    .child(provider_icon(&p).size(px(ico_size))),
            );
        }
        let mut val = text(self.display())
            .truncate()
            .line_height(LineHeight::Normal);
        if let Some((family, size)) = &self.value_font {
            val = val.family(family.clone()).size(*size);
        }
        if let Some(c) = first.as_ref().and_then(|o| o.tint) {
            val = val.color(c);
        }
        if !tr.hide_value {
            button = button.child(if tr.value_shrink {
                div().flex_initial().min_w_0().child(val)
            } else if self.ghost || tr.value_auto {
                div().flex_none().child(val)
            } else {
                div().flex_1().min_w_0().child(val)
            });
        }
        // Chevron: `sm` uses chevrons-up-down and never rotates; otherwise chevron-down rotates
        // 180° over .12s when open. `.sel.ghost .sel-chev` (11px) comes after `.sel.sm`'s 12px.
        let chev_size = tr.chevron_size.unwrap_or(if self.ghost {
            11.
        } else if sm {
            12.
        } else {
            14.
        });
        let chev = if sm {
            icon(IconName::ChevronsUpDown)
                .size(px(chev_size))
                .color(chev_color)
        } else {
            let s = chev_size;
            let since = if self.open {
                self.opened_at
            } else {
                self.closed_at
            };
            let p = since.map_or(1., |at| {
                motion::eased(at, Duration::from_millis(120), Easing::EASE, cx)
            });
            if p < 1. {
                window.request_animation_frame();
            }
            let deg = if self.open { 180. * p } else { 180. * (1. - p) };
            icon(IconName::ChevronDown)
                .stroke_width(2.2)
                .size(px(s))
                .color(chev_color)
                .rotate(deg.round())
        };
        button = button.child(chev);
        let entity = cx.entity();
        button = button
            .on_click(
                cx.guarded_listener(|this, _: &ClickEvent, window, cx| this.toggle(window, cx)),
            )
            .on_key_down(cx.guarded_listener(|this, e: &KeyDownEvent, window, cx| {
                if this.on_key(e, false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                canvas(
                    // The menu places itself from the trigger's border box.
                    move |bounds, _, cx| {
                        let b = bounds.dilate(px(border_w));
                        entity.update(cx, |this, _| this.trigger_bounds = Some(b))
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );

        let mut root = div()
            .relative()
            .when(sm && !self.ghost, |d| d.min_w(px(132.)))
            .when_some(tr.max_width, |d, w| d.flex_none().min_w_0().max_w(px(w)))
            // `title={shown}`.
            .child(crate::tooltip::titled(
                ("sel-title", cx.entity_id().as_u64() as usize),
                self.display(),
                button,
            ));
        if self.open
            && let Some(menu) = self.menu(window, cx)
        {
            root = root.child(menu);
        }
        root
    }
}

impl Render for Select {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}

impl Select {
    fn menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        let trigger = self.trigger_bounds?;
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let rect = (
            f32::from(trigger.top()),
            f32::from(trigger.bottom()),
            f32::from(trigger.left()),
            f32::from(trigger.size.width),
        );
        let filtered = self.filtered();
        let hinted = self.options.iter().any(|o| o.hint.is_some());
        let up = self.up || menu_prefer_up(rect.0, rect.1, vh);
        let mbox = menu_fixed_box(rect, up, self.variant.min_width(hinted), (vw, vh));
        let width = mbox.width;
        let left = clamp_menu_left(mbox.left, width, vw);
        let max_h = self.variant.max_height(vh, mbox.space);

        let rows = Self::rows(&filtered);
        let mut list: Vec<AnyElement> = Vec::new();
        let mut first_heading = true;
        for row in &rows {
            let o = &filtered[row.ix];
            if let Some(h) = &row.heading {
                let mut g = div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .pt(px(8.))
                    .px(px(10.))
                    .pb(px(4.))
                    .text_color(t.muted.with_alpha(0.85))
                    .text_size(px(11.));
                if !first_heading || row.ix > 0 {
                    g = g
                        .mt(px(4.))
                        .border_t(px(1.))
                        .border_color(Hsla::from(t.line))
                        .pt(px(8.));
                }
                first_heading = false;
                if let Some(p) = &row.heading_provider {
                    g = g.child(
                        div().flex_none().size(px(12.)).child(
                            provider_icon(p)
                                .size(px(12.))
                                .color(t.muted.with_alpha(0.85 * 0.95)),
                        ),
                    );
                }
                list.push(
                    g.child(text(h.clone()).tracking(0.06).uppercase().nowrap())
                        .into_any_element(),
                );
            }
            let on = self.selected(&o.value);
            let active = self.active == Some(row.ix);
            let value = o.value.clone();
            let ix = row.ix;
            let label = o.label.clone();
            let mut opt = div()
                .id(("sel-opt", ix))
                .debug_selector(move || format!("sel-opt:{label}"))
                .flex()
                .items_center()
                .gap(px(8.))
                .w_full()
                .py(px(8.))
                .px(px(10.))
                .rounded(px(5.))
                .text_color(t.text)
                .text_size(px(12.5))
                .cursor_pointer()
                .when(active, |d| {
                    // `data-active` for specs.
                    let label = o.label.clone();
                    d.bg(t.pane).child(
                        div()
                            .absolute()
                            .debug_selector(move || format!("sel-opt-active:{label}")),
                    )
                })
                .child(if on {
                    icon(IconName::Check)
                        .stroke_width(2.4)
                        .size(px(14.))
                        .color(t.amber)
                        .into_any_element()
                } else {
                    div().flex_none().size(px(14.)).into_any_element()
                });
            if let Some(i) = o.icon {
                let c = o.tint.unwrap_or(t.text);
                opt = opt.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .child(icon(i).size(px(12.)).color(c)),
                );
            }
            let mut lbl = text(o.label.clone())
                .truncate()
                .line_height(LineHeight::Normal);
            if let Some((family, size)) = &o.font {
                lbl = lbl
                    .family(family.clone())
                    .size(*size)
                    .line_height(LineHeight::Scale(1.45));
            }
            if let Some(c) = o.tint {
                lbl = lbl.color(c);
            }
            opt = opt.child(if o.hint.is_some() {
                div().flex_none().child(lbl)
            } else {
                div().flex_1().min_w_0().child(lbl)
            });
            if let Some(hint) = &o.hint {
                opt = opt.child(
                    div()
                        .flex_initial()
                        .min_w_0()
                        .ml_auto()
                        .pl(px(14.))
                        .max_w(px(210.))
                        .child(
                            text(hint.clone())
                                .size(10.5)
                                .color(t.faint)
                                .truncate()
                                .line_height(LineHeight::Normal),
                        ),
                );
            }
            opt =
                opt.on_mouse_move(cx.guarded_listener(move |this, e: &MouseMoveEvent, _, cx| {
                    if this.gate.allows(e.position.x.into(), e.position.y.into())
                        && this.active != Some(ix)
                    {
                        this.active = Some(ix);
                        cx.notify();
                    }
                }))
                .on_click(cx.guarded_listener(
                    move |this, _: &ClickEvent, window, cx| this.pick(value.clone(), window, cx),
                ));
            list.push(opt.into_any_element());
        }
        if filtered.is_empty() {
            list.push(
                div()
                    .py(px(14.))
                    .px(px(10.))
                    .child(
                        text(if self.query.trim().is_empty() {
                            "No options"
                        } else {
                            "No matches"
                        })
                        .size(12.)
                        .color(t.faint)
                        .align(crate::text::Align::Center),
                    )
                    .into_any_element(),
            );
        }
        let mut menu = crate::style::menu_panel(cx)
            .id("sel-menu")
            .debug_selector(|| "sel-menu".into())
            .w(px(width))
            .max_h(px(max_h))
            .when_some(self.variant.max_width(vw), |d, w| d.max_w(px(w)))
            .font_family(UI)
            .occlude()
            .on_mouse_down_out(
                cx.guarded_listener(|this, _, window, cx| this.close(false, window, cx)),
            );
        if self.searchable {
            menu = menu
                .p_0()
                .child(search_header(
                    &self.search,
                    &SEARCH_PLACEHOLDER.into(),
                    window,
                    cx,
                ))
                .on_key_down(cx.guarded_listener(|this, e: &KeyDownEvent, window, cx| {
                    if this.on_key(e, true, window, cx) {
                        cx.stop_propagation();
                    }
                }));
        }
        // `.sel-list { overflow: auto }`: the themed scrollbar, padding scrolling with the rows.
        menu = menu.child(
            div()
                .flex_auto()
                .min_h_0()
                .flex()
                .flex_col()
                .when(self.searchable, |d| d.px(px(4.)).pb(px(4.)))
                .child(
                    crate::widgets::scroll_area("sel-list")
                        .track(&self.list_scroll)
                        .child(
                            div()
                                .id("sel-rows")
                                .when(self.searchable, |d| d.pt(px(4.)))
                                .role(gpui_kit::accesskit::Role::ListBox)
                                .aria_label(self.label.clone())
                                .children(list),
                        ),
                ),
        );
        // The menu hangs off the trigger in the layout tree (see `style::attached`).
        let below = mbox.top.is_some();
        Some(
            deferred(crate::style::attached(
                below,
                MENU_GAP_PX,
                left - rect.2,
                crate::shadow::with_shadow(menu, &crate::style::MENU_SHADOW, 8.),
            ))
            .with_priority(210)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_headings_start_runs() {
        let opts = vec![
            SelectOption::new("a", "A")
                .group("Claude")
                .provider("claude"),
            SelectOption::new("b", "B")
                .group("Claude")
                .provider("claude"),
            SelectOption::new("c", "C").group("Codex").provider("codex"),
            SelectOption::new("d", "D"),
        ];
        let rows = Select::rows(&opts);
        let heads: Vec<Option<&str>> = rows.iter().map(|r| r.heading.as_deref()).collect();
        assert_eq!(heads, vec![Some("Claude"), None, Some("Codex"), None]);
        assert_eq!(rows[2].heading_provider.as_deref(), Some("codex"));
    }
}
