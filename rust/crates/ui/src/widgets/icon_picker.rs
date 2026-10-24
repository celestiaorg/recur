//! `IconPicker.svelte`: pick a Lucide icon for a project. A small select-style trigger and a
//! searchable 8-column grid (`.ico-menu`, `.ico-grid`, `.ico-cell` in `_settings.scss`).

use crate::assets::{LucideEntry, lucide_icons, lucide_suggested};
use crate::fonts::UI;
use crate::guard::Guarded;
use crate::icon::{icon, lucide_icon};
use crate::icon_name::IconName;
use crate::logic::fuzzy::{fuzzy_filter_prepared, prepare_fuzzy};
use crate::logic::hover_gate::HoverGate;
use crate::logic::menu_space::{clamp_menu_left, menu_fixed_box, menu_prefer_up};
use crate::style::{faded, is_focus_visible};
use crate::text::{LineHeight, TextStyled, text};
use crate::theme::Theme;
use crate::widgets::select::{search_header, search_input, style_search_input};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, Bounds, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable,
    Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseMoveEvent, ParentElement, Pixels,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    canvas, deferred, div, prelude::FluentBuilder, px,
};

const SEARCH_PLACEHOLDER: &str = "Search icons…";

pub const COLS: usize = 8;
pub const SEARCH_LIMIT: usize = 80;

/// The grid: suggestions (with the current icon first when it is not one of them), or the
/// best fuzzy matches over names and aliases.
pub fn shown_icons(value: &str, query: &str) -> Vec<LucideEntry> {
    let q = query.trim();
    if q.is_empty() {
        let suggested: Vec<LucideEntry> = lucide_suggested().cloned().collect();
        let cur = lucide_icons()
            .iter()
            .find(|e| e.name == value || e.aliases.iter().any(|a| a == value));
        return match cur {
            Some(c) if !suggested.contains(c) => {
                std::iter::once(c.clone()).chain(suggested).collect()
            }
            _ => suggested,
        };
    }
    let prepared = prepare_fuzzy(lucide_icons(), |e| {
        format!("{} {}", e.name, e.aliases.join(" "))
    });
    fuzzy_filter_prepared(&prepared, q, SEARCH_LIMIT)
        .into_iter()
        .map(|h| h.item)
        .collect()
}

/// Emitted with the picked icon name.
pub struct IconPicked(pub SharedString);

pub struct IconPicker {
    focus: FocusHandle,
    value: SharedString,
    disabled_over: Option<crate::color::Color>,
    open: bool,
    active: usize,
    gate: HoverGate,
    search: Entity<InputState>,
    query: SharedString,
    trigger_bounds: Option<Bounds<Pixels>>,
    grid_scroll: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<IconPicked> for IconPicker {}

impl Focusable for IconPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl IconPicker {
    pub fn new(
        value: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = search_input(window, cx, SEARCH_PLACEHOLDER);
        let sub = cx.guarded_subscribe_in(
            &search,
            window,
            |this: &mut IconPicker, input, e: &InputEvent, _, cx| {
                if matches!(e, InputEvent::Change) {
                    this.query = input.read(cx).value();
                    let n = shown_icons(&this.value, &this.query).len();
                    if this.active >= n {
                        this.active = 0;
                    }
                    this.gate.reset();
                    cx.notify();
                }
            },
        );
        IconPicker {
            // A `<button>`: in the Tab order.
            focus: cx.focus_handle().tab_stop(true),
            value: value.into(),
            disabled_over: None,
            open: false,
            active: 0,
            gate: HoverGate::default(),
            search,
            query: SharedString::default(),
            trigger_bounds: None,
            grid_scroll: ScrollHandle::new(),
            _subs: vec![sub],
        }
    }

    pub fn set_value(&mut self, v: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.value = v.into();
        cx.notify();
    }

    pub fn set_disabled(&mut self, backdrop: Option<crate::color::Color>, cx: &mut Context<Self>) {
        self.disabled_over = backdrop;
        cx.notify();
    }

    fn shown_value(&self) -> SharedString {
        if self.value.is_empty() {
            "box".into()
        } else {
            self.value.clone()
        }
    }

    fn open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.query = SharedString::default();
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        let shown = shown_icons(&self.value, "");
        self.active = shown
            .iter()
            .position(|e| e.name == self.value || e.aliases.iter().any(|a| *a == self.value))
            .unwrap_or(0);
        self.gate.reset();
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn close(&mut self, restore_focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.active = 0;
        self.query = SharedString::default();
        if restore_focus {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn pick(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.value = name.clone();
        cx.emit(IconPicked(name));
        self.close(true, window, cx);
    }

    fn jump(&mut self, i: isize, len: usize) {
        if len == 0 {
            self.active = 0;
            return;
        }
        self.gate.reset();
        self.active = i.clamp(0, len as isize - 1) as usize;
        self.grid_scroll.scroll_to_item(self.active);
    }

    /// `onSearchKey`: arrows move by one cell or one row of 8.
    fn on_search_key(
        &mut self,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let shown = shown_icons(&self.value, &self.query);
        let len = shown.len();
        let a = self.active as isize;
        match e.keystroke.key.as_str() {
            "escape" => self.close(true, window, cx),
            "tab" => {
                self.close(true, window, cx);
                return false;
            }
            "right" => self.jump(a + 1, len),
            "left" => self.jump(a - 1, len),
            "down" => self.jump(a + COLS as isize, len),
            "up" => self.jump(a - COLS as isize, len),
            "enter" => {
                if let Some(hit) = shown.get(self.active) {
                    self.pick(hit.name.clone().into(), window, cx);
                }
            }
            "home" => self.jump(0, len),
            "end" => self.jump(len as isize - 1, len),
            _ => return false,
        }
        cx.notify();
        true
    }
}

impl IconPicker {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        style_search_input(&self.search, cx);
        let t = Theme::of(cx).tokens;
        let disabled = self.disabled_over.is_some();
        let fv = is_focus_visible(&self.focus, window);
        let lit = self.open || fv;
        // `.ico-pick.sel.sm` trigger.
        let (mut bg, mut border, mut fg, mut ico, mut chev) =
            (t.pane, t.line, t.text, t.muted, t.faint);
        if lit && !disabled {
            border = crate::color::rgba(242, 179, 61, 0.55);
            ico = t.amber;
            chev = t.amber;
        }
        if let Some(back) = self.disabled_over {
            let fill = bg;
            bg = faded(bg, bg, 0.55, back);
            border = faded(border, fill, 0.55, back);
            fg = faded(fg, fill, 0.55, back);
            ico = faded(ico, fill, 0.55, back);
            chev = faded(chev, fill, 0.55, back);
        }
        let entity = cx.entity();
        let hover = t.pane_2;
        let hover_border = t.line_bright;
        // As `.sel` triggers: `:focus-visible` rounds to 4px and outlines; hover fills, and an
        // open trigger keeps its amber border.
        let radius = if fv { 4. } else { 6. };
        let keep_border = self.open;
        let mut button = div()
            .id("ico-btn")
            .track_focus(&self.focus)
            .relative()
            .flex()
            .items_center()
            .gap(px(7.))
            .w_full()
            .h(px(30.))
            .px(px(10.))
            .rounded(px(radius))
            .bg(bg)
            .border(px(1.))
            .border_color(Hsla::from(border))
            .text_color(fg)
            .font_family(UI)
            .text_size(px(12.))
            .aria_label("Lucide icon")
            .when(lit && !disabled, |d| {
                d.relative().child(crate::shadow::box_shadow(
                    &[crate::shadow::css_box_shadow(0., 0., 0., 3., t.amber_d)],
                    radius,
                    1.,
                ))
            })
            .when(fv && !disabled, |d| {
                d.child(crate::style::focus_ring(radius, 1., cx))
            })
            .when(!disabled, |d| d.cursor_pointer())
            .when(!disabled, |d| {
                d.hover(move |s| {
                    let s = s.bg(hover);
                    if keep_border {
                        s
                    } else {
                        s.border_color(Hsla::from(hover_border))
                    }
                })
            })
            .child(
                div()
                    .flex_none()
                    .text_color(ico)
                    .child(lucide_icon(&self.shown_value()).size(px(14.))),
            )
            .child(
                div().flex_1().min_w_0().child(
                    text(self.shown_value())
                        .truncate()
                        .line_height(LineHeight::Normal),
                ),
            )
            // `chevron-down` at the small select's 12px; `.sel.sm.open .sel-chev` does not rotate.
            .child(
                icon(IconName::ChevronDown)
                    .stroke_width(2.2)
                    .size(px(12.))
                    .color(chev),
            )
            .child(
                canvas(
                    // The menu places itself from the trigger's border box.
                    move |b, _, cx| {
                        let b = b.dilate(px(1.));
                        entity.update(cx, |this, _| this.trigger_bounds = Some(b))
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        if !disabled {
            button = button.on_click(cx.guarded_listener(|this, _: &ClickEvent, window, cx| {
                if this.open {
                    this.close(false, window, cx);
                } else {
                    this.open_menu(window, cx);
                }
            }));
        }
        let mut root = div().relative().min_w(px(132.)).child(button);
        if self.open
            && let Some(m) = self.menu(window, cx)
        {
            root = root.child(m);
        }
        root
    }
}

impl Render for IconPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::guard::view(self, window, cx, Self::render_view)
    }
}

impl IconPicker {
    fn menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        let tr = self.trigger_bounds?;
        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
        let rect = (
            f32::from(tr.top()),
            f32::from(tr.bottom()),
            f32::from(tr.left()),
            f32::from(tr.size.width),
        );
        let up = menu_prefer_up(rect.0, rect.1, vh);
        let mbox = menu_fixed_box(rect, up, 300., (vw, vh));
        let width = mbox.width.max(300.);
        let left = clamp_menu_left(mbox.left, width, vw);
        // `.ico-menu { max-height: min(60vh, 380px, var(--sel-space)) }`.
        let max_h = (vh * 0.6).min(380.).min(mbox.space);
        let shown = shown_icons(&self.value, &self.query);
        let mut cells: Vec<AnyElement> = Vec::new();
        for (i, e) in shown.iter().enumerate() {
            let on =
                e.name == self.value.as_ref() || e.aliases.iter().any(|a| a == self.value.as_ref());
            let active = i == self.active;
            let name: SharedString = e.name.clone().into();
            let hover_bg = t.pane;
            let mut c = div()
                .id(("ico-cell", i))
                .flex()
                .items_center()
                .justify_center()
                // `.ico-cell { aspect-ratio: 1 }` in a `repeat(8, 1fr)` track.
                .w_full()
                .aspect_ratio(1.)
                .rounded(px(6.))
                .text_color(t.text)
                .cursor_pointer()
                .aria_label(name.clone())
                .hover(move |s| s.bg(hover_bg))
                .child(lucide_icon(&e.name).size(px(16.)));
            if active {
                c = c.bg(t.pane);
            }
            if on {
                c = c
                    .bg(crate::color::rgba(242, 179, 61, 0.15))
                    .text_color(t.amber);
            }
            let pick = name.clone();
            c =
                c.on_mouse_move(
                    cx.guarded_listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if this.gate.allows(ev.position.x.into(), ev.position.y.into())
                            && this.active != i
                        {
                            this.active = i;
                            cx.notify();
                        }
                    }),
                )
                .on_click(cx.guarded_listener(
                    move |this, _: &ClickEvent, window, cx| this.pick(pick.clone(), window, cx),
                ));
            cells.push(crate::tooltip::titled(("ico-tip", i), name, c).into_any_element());
        }
        let grid = if cells.is_empty() {
            div()
                .p(px(4.))
                .child(
                    div().py(px(14.)).px(px(10.)).child(
                        text("No matches")
                            .size(12.)
                            .color(t.faint)
                            .align(crate::text::Align::Center),
                    ),
                )
                .into_any_element()
        } else {
            div()
                .flex_auto()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    crate::widgets::scroll_area("ico-grid")
                        .track(&self.grid_scroll)
                        .child(
                            div()
                                .id("ico-cells")
                                .p(px(4.))
                                .grid()
                                .grid_cols(COLS as u16)
                                .gap(px(2.))
                                .role(gpui_kit::accesskit::Role::ListBox)
                                .aria_label("Lucide icons")
                                .children(cells),
                        ),
                )
                .into_any_element()
        };
        // The search header sits flush; the menu's padding moves onto the grid (`search_header`).
        let menu = crate::style::menu_panel(cx)
            .p_0()
            .id("ico-menu")
            .w(px(width))
            .max_h(px(max_h))
            .font_family(UI)
            .occlude()
            .on_mouse_down_out(
                cx.guarded_listener(|this, _, window, cx| this.close(false, window, cx)),
            )
            .on_key_down(cx.guarded_listener(|this, e: &KeyDownEvent, window, cx| {
                if this.on_search_key(e, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(search_header(
                &self.search,
                &SEARCH_PLACEHOLDER.into(),
                window,
                cx,
            ))
            .child(
                div()
                    .flex_auto()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .px(px(4.))
                    .pb(px(4.))
                    .child(grid),
            );
        let (pos, anchor) = match (mbox.top, mbox.bottom) {
            (Some(top), _) => (top, gpui_kit::Anchor::TopLeft),
            (None, Some(b)) => (vh - b, gpui_kit::Anchor::BottomLeft),
            _ => (0., gpui_kit::Anchor::TopLeft),
        };
        Some(
            deferred(
                crate::style::anchored_at(
                    left,
                    pos,
                    crate::shadow::with_shadow(menu, &crate::style::MENU_SHADOW, 8.),
                )
                .anchor(anchor),
            )
            .with_priority(210)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_lead_with_a_custom_current_icon() {
        let plain = shown_icons("box", "");
        assert_eq!(plain.first().map(|e| e.name.as_str()), Some("box"));
        let custom = shown_icons("zap-off", "");
        assert_eq!(custom.first().map(|e| e.name.as_str()), Some("zap-off"));
        assert_eq!(custom.len(), plain.len() + 1);
        let hits = shown_icons("box", "rock");
        assert!(hits.iter().any(|e| e.name == "rocket"));
        assert!(hits.len() <= SEARCH_LIMIT);
    }
}
