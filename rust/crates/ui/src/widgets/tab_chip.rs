//! `TabChip.svelte`: one tab in a [`super::tab_strip::TabStrip`]: icon, label (with the
//! rename decode), badge / dirty / unseen markers, the busy wash, and the close button.

use crate::file_icons::file_icon;
use crate::fonts::{UI, weight};
use crate::icon::icon;
use crate::icon_name::IconName;
use crate::motion;
use crate::shadow::{css_box_shadow, with_shadow};
use crate::text::{AlphaMask, LineHeight, TextStyled, text};
use crate::theme::Theme;
use crate::widgets::decode_text::decode_text;
use gpui_kit::{
    App, Bounds, ClickEvent, CursorStyle, ElementId, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, ParentElement, Pixels, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder, px,
};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The busy wash cycle (`tab-busy-wash 1.8s linear infinite`).
pub const BUSY_WASH: Duration = Duration::from_millis(1800);

/// Alpha of the busy wash at `x` within a body `width` px wide, `t` into the cycle (0..1).
/// The mask is `linear-gradient(90deg, .45 0 35%, 1 50%, .45 65% 100%)` sized 200% × 100%,
/// its position sweeping from 100% to −100%, and repeating.
pub fn busy_wash_alpha(x: f32, width: f32, t: f32) -> f32 {
    if width <= 0. {
        return 1.;
    }
    let tile = 2. * width;
    // mask-position p% places the tile at (W − 2W) · p; p runs 1 → −1.
    let offset = -width * (1. - 2. * t);
    let u = (x - offset).rem_euclid(tile) / tile;
    match u {
        u if u <= 0.35 || u >= 0.65 => 0.45,
        u if u <= 0.5 => 0.45 + 0.55 * (u - 0.35) / 0.15,
        u => 1. - 0.55 * (u - 0.5) / 0.15,
    }
}

type Click = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type KeyDown = Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App) -> bool>;
type Press = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;
type Guard = Rc<dyn Fn(&mut App) -> bool>;

/// What draws the chip's leading icon.
#[derive(Clone)]
pub enum ChipIcon {
    /// An app icon, optionally tinted (chat mode identity).
    App(IconName, Option<Hsla>),
    /// The file-type icon for a file name.
    File(SharedString),
}

/// `<TabChip …>`.
#[derive(IntoElement)]
pub struct TabChip {
    id: ElementId,
    label: SharedString,
    title: Option<SharedString>,
    active: bool,
    lit: bool,
    dirty: bool,
    unseen: bool,
    badge: u32,
    busy: bool,
    muted: bool,
    animate_label: bool,
    label_key: SharedString,
    icon: Option<ChipIcon>,
    close_label: Option<SharedString>,
    reorderable: bool,
    floating: bool,
    on_click: Option<Click>,
    on_close: Option<Click>,
    on_key_down: Option<KeyDown>,
    on_press: Option<Press>,
    on_context_menu: Option<Press>,
    click_guard: Option<Guard>,
    lift: f32,
}

pub fn tab_chip(id: impl Into<ElementId>, label: impl Into<SharedString>) -> TabChip {
    TabChip {
        id: id.into(),
        label: label.into(),
        title: None,
        active: false,
        lit: false,
        dirty: false,
        unseen: false,
        badge: 0,
        busy: false,
        muted: false,
        animate_label: false,
        label_key: SharedString::default(),
        icon: None,
        close_label: None,
        reorderable: false,
        floating: false,
        on_click: None,
        on_close: None,
        on_key_down: None,
        on_press: None,
        on_context_menu: None,
        click_guard: None,
        lift: 1.,
    }
}

impl TabChip {
    /// Hover title (defaults to the label).
    pub fn title(mut self, t: impl Into<SharedString>) -> Self {
        self.title = Some(t.into());
        self
    }
    pub fn active(mut self, on: bool) -> Self {
        self.active = on;
        self
    }
    /// Amber underline: the panel holding this tab has keyboard focus.
    pub fn lit(mut self, on: bool) -> Self {
        self.lit = on;
        self
    }
    pub fn dirty(mut self, on: bool) -> Self {
        self.dirty = on;
        self
    }
    /// Coral dot: a finished background turn not yet viewed.
    pub fn unseen(mut self, on: bool) -> Self {
        self.unseen = on;
        self
    }
    /// Count pill; hidden at 0.
    pub fn badge(mut self, n: u32) -> Self {
        self.badge = n;
        self
    }
    /// Busy wash on icon and label while not selected.
    pub fn busy(mut self, on: bool) -> Self {
        self.busy = on;
        self
    }
    /// Dimmed (an exited terminal).
    pub fn muted(mut self, on: bool) -> Self {
        self.muted = on;
        self
    }
    /// Decode the label when it changes; `key` keeps identity across switches.
    pub fn animate_label(mut self, on: bool, key: impl Into<SharedString>) -> Self {
        self.animate_label = on;
        self.label_key = key.into();
        self
    }
    pub fn icon(mut self, icon: ChipIcon) -> Self {
        self.icon = Some(icon);
        self
    }
    /// Close button's accessible name (default `Close <label>`).
    pub fn close_label(mut self, l: impl Into<SharedString>) -> Self {
        self.close_label = Some(l.into());
        self
    }
    /// Grab cursor for drag-reorderable strips.
    pub fn reorderable(mut self, on: bool) -> Self {
        self.reorderable = on;
        self
    }
    /// The drag ghost: the chip, lifted.
    pub fn floating(mut self, on: bool) -> Self {
        self.floating = on;
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
    /// Shows the close button.
    pub fn on_close(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(f));
        self
    }
    /// Key handler run first; return true when handled (the chip then skips Enter/Space).
    pub fn on_key_down(
        mut self,
        f: impl Fn(&KeyDownEvent, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        self.on_key_down = Some(Rc::new(f));
        self
    }
    /// Primary-button press (drag start).
    pub fn on_press(
        mut self,
        f: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_press = Some(Rc::new(f));
        self
    }
    /// Runs before the click handler; returning true swallows the click.
    pub fn click_guard(mut self, f: impl Fn(&mut App) -> bool + 'static) -> Self {
        self.click_guard = Some(Rc::new(f));
        self
    }
    /// The floating chip's lift while it settles: 1 is the full drag shadow, 0 none.
    pub fn lift(mut self, lift: f32) -> Self {
        self.lift = lift;
        self
    }
    pub fn on_context_menu(
        mut self,
        f: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_context_menu = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for TabChip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let wash = self.busy && !self.active && !motion::reduced(cx);
        let group = SharedString::from(format!("tab-chip-{}", self.id));
        let fg = if self.active {
            t.text
        } else if self.muted {
            t.faint
        } else {
            t.muted
        };
        // The busy body is drawn in --text under the moving mask.
        let body_fg = if wash { t.text } else { fg };
        let body_bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
        let mask: Option<AlphaMask> = wash.then(|| {
            let start =
                window.use_keyed_state((self.id.clone(), "wash"), cx, |_, _| Instant::now());
            let started = *start.read(cx);
            let elapsed = motion::elapsed(started, cx);
            if !motion::is_frozen(cx) {
                window.request_animation_frame();
            }
            let phase = (elapsed.as_secs_f32() / BUSY_WASH.as_secs_f32()).fract();
            let b = body_bounds.clone();
            Rc::new(move |x: f32| {
                let Some(b) = b.get() else { return 1. };
                busy_wash_alpha(x - f32::from(b.left()), f32::from(b.size.width), phase)
            }) as AlphaMask
        });
        // `prefers-reduced-motion`: the busy body just dims to .7.
        let reduced_busy = self.busy && !self.active && motion::reduced(cx);

        let mut body = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .max_w_full()
            .text_color(body_fg)
            .child({
                let cell = body_bounds.clone();
                canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
            });
        if let Some(i) = &self.icon {
            body = body.child(match i {
                ChipIcon::App(name, tint) => {
                    let mut ic = icon(*name).size(px(14.));
                    if let Some(c) = tint {
                        ic = ic.color(*c);
                    }
                    if let Some(m) = &mask {
                        ic = ic.mask(m.clone());
                    }
                    ic.into_any_element()
                }
                ChipIcon::File(name) => file_icon(name.clone()).into_any_element(),
            });
        }
        let label_opacity = if reduced_busy { 0.7 } else { 1. };
        let label_mask = mask.clone();
        body = body.child(
            decode_text((self.id.clone(), "name"), self.label.clone())
                .animate(self.animate_label)
                .source_key(self.label_key.clone())
                .style(move |t| {
                    let t = t.truncate().opacity(label_opacity);
                    match label_mask {
                        Some(m) => t.mask(m),
                        None => t,
                    }
                }),
        );
        if self.badge > 0 {
            body = body.child(
                div()
                    .flex_none()
                    .bg(t.pane_2)
                    .rounded(px(10.))
                    .px(px(5.))
                    .child(
                        text(self.badge.to_string())
                            .mono()
                            .size(9.5)
                            .color(if self.active { t.muted } else { t.faint })
                            .nowrap(),
                    ),
            );
        }
        if self.unseen && !self.busy && !self.active {
            body = body.child(div().flex_none().size(px(6.)).rounded_full().bg(t.coral));
        } else if self.dirty && !self.busy {
            body = body.child(div().flex_none().size(px(5.)).rounded_full().bg(t.amber));
        }

        let border = if self.active {
            if self.lit { t.amber } else { t.line }
        } else {
            crate::color::TRANSPARENT
        };
        let hover_text = t.text;
        let sel = format!(
            "tab-chip:{}{}",
            self.id,
            if self.active { ":on" } else { "" }
        );
        let mut chip = div()
            .id(self.id.clone())
            .debug_selector(move || sel)
            .group(group.clone())
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h_full()
            .px(px(10.))
            .border_b(px(2.))
            .border_color(Hsla::from(border))
            .text_color(fg)
            .font_family(UI)
            .text_size(px(12.))
            .font_weight(gpui_kit::FontWeight(weight::MEDIUM))
            .when(!self.floating, |d| d.max_w(px(160.)))
            .hover(move |s| s.text_color(Hsla::from(hover_text)))
            .cursor(if self.floating {
                CursorStyle::ClosedHand
            } else if self.reorderable {
                CursorStyle::OpenHand
            } else {
                CursorStyle::PointingHand
            })
            .child(body);
        // `0 8px 24px rgba(0,0,0,.35)` under the floating chip, eased to nothing as it settles.
        let lift = self.floating.then(|| {
            let l = self.lift;
            css_box_shadow(
                0.,
                8. * l,
                24. * l,
                0.,
                crate::color::rgba(0, 0, 0, 0.35 * l),
            )
        });
        if self.floating {
            chip = chip.bg(t.pane);
        }
        if self.on_click.is_some() || self.click_guard.is_some() {
            let f = self.on_click.clone();
            let guard = self.click_guard.clone();
            chip = chip.on_click(move |e, w, cx| {
                if guard.as_ref().is_some_and(|g| g(cx)) {
                    return;
                }
                if let Some(f) = &f {
                    f(e, w, cx);
                }
            });
        }
        if let Some(f) = self.on_press.clone() {
            chip = chip.on_mouse_down(MouseButton::Left, move |e, w, cx| f(e, w, cx));
        }
        if let Some(f) = self.on_context_menu.clone() {
            chip = chip.on_mouse_down(MouseButton::Right, move |e, w, cx| f(e, w, cx));
        }
        {
            let key = self.on_key_down.clone();
            let click = self.on_click.clone();
            chip = chip.on_key_down(move |e: &KeyDownEvent, w, cx| {
                if let Some(k) = &key
                    && k(e, w, cx)
                {
                    cx.stop_propagation();
                    return;
                }
                if matches!(e.keystroke.key.as_str(), "enter" | "space")
                    && let Some(c) = &click
                {
                    cx.stop_propagation();
                    c(&ClickEvent::default(), w, cx);
                }
            });
        }
        if let Some(close) = self.on_close.clone() {
            let label = self
                .close_label
                .clone()
                .unwrap_or_else(|| format!("Close {}", self.label).into());
            let visible = self.active;
            let faint = t.faint;
            let hover_bg = t.pane_2;
            let close_sel = format!("tab-close:{}", self.id);
            let x = div()
                .id((self.id.clone(), "close"))
                .debug_selector(move || close_sel)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded(px(4.))
                .text_color(faint)
                .cursor_pointer()
                .aria_label(label)
                .when(!visible, |d| {
                    d.invisible().group_hover(group.clone(), |s| s.visible())
                })
                .hover(move |s| s.bg(hover_bg).text_color(Hsla::from(hover_text)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |e, w, cx| {
                    cx.stop_propagation();
                    close(e, w, cx)
                })
                .child(
                    text("×")
                        .size(13.)
                        .line_height(LineHeight::Scale(1.))
                        .nowrap(),
                );
            chip = chip.child(crate::tooltip::titled(
                (self.id.clone(), "close-tip"),
                "Close",
                x,
            ));
        }
        let title = self.title.unwrap_or(self.label);
        let chip = crate::tooltip::titled((self.id, "tip"), title, chip);
        match lift {
            Some(shadow) => with_shadow(chip, &[shadow], 0.).h_full().into_any_element(),
            None => chip.into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_wash_sweeps_left_to_right() {
        // At t = 0 the bright band sits on the left edge.
        assert_eq!(busy_wash_alpha(0., 100., 0.), 1.);
        assert_eq!(busy_wash_alpha(50., 100., 0.), 0.45);
        // Halfway through, the band is at the right edge.
        assert!((busy_wash_alpha(100., 100., 0.5) - 1.).abs() < 1e-5);
        // Ramp: 15% of the 200% tile (30px of 100) from .45 up to 1.
        assert!((busy_wash_alpha(-15., 100., 0.) - 0.725).abs() < 1e-4);
    }
}
