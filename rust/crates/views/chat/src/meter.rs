//! `ContextUsageMeter.svelte`: the last turn's context fill as a 14px ring in the composer,
//! with a popover breaking it down. Disabled until a turn reports usage.

use recur_ui::guard::Guarded;
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Subscription, Window, div, px, relative,
};
use recur_agent::context_usage::{ContextUsage, format_tokens};
use recur_ui::fonts::Family;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::text::TextStyled;
use recur_ui::{Color, Theme, Tokens, motion, rgba, svg_markup};

use crate::style::{hsla, label};

const SIZE: f32 = 14.;
const SW: f32 = 2.25;

/// The meter's numbers (`ready`, `used`, `pct`, …).
#[derive(Clone, Debug, PartialEq)]
pub struct MeterValues {
    pub ready: bool,
    pub used: i64,
    pub limit: i64,
    pub pct: f32,
    pub pct_label: i64,
    pub free_label: i64,
    pub cache_read: i64,
    pub fresh: i64,
    pub show_split: bool,
    pub compacting: bool,
    pub compacted: bool,
}

pub fn meter_values(u: Option<&ContextUsage>) -> MeterValues {
    let ready = u.is_some_and(ContextUsage::ready);
    let used = u.map_or(0, ContextUsage::used_tokens);
    let limit = u.map_or(0, |u| u.context_window);
    let pct = if ready && limit > 0 {
        (used as f32 / limit as f32).min(1.)
    } else {
        0.
    };
    let free = (limit - used).max(0);
    let creation = u.map_or(0, |u| u.cache_creation_tokens);
    let cache_read = u.map_or(0, |u| u.cache_read_tokens);
    MeterValues {
        ready,
        used,
        limit,
        pct,
        pct_label: (pct * 100.).round() as i64,
        free_label: if limit > 0 {
            ((free as f32 / limit as f32) * 100.).round() as i64
        } else {
            0
        },
        cache_read,
        fresh: u.map_or(0, |u| u.input_tokens) + creation,
        show_split: ready && (cache_read > 0 || creation > 0),
        compacting: u.is_some_and(|u| u.compacting),
        compacted: u.is_some_and(|u| u.compacted),
    }
}

/// `ringColor`.
pub fn ring_color(v: &MeterValues, t: &Tokens) -> Color {
    if !v.ready {
        t.line
    } else if v.compacting {
        t.amber
    } else if v.pct >= 0.9 {
        t.coral
    } else if v.pct >= 0.7 {
        t.amber
    } else {
        t.muted
    }
}

fn ring_svg(v: &MeterValues, color: Color, fill_opacity: f32, t: &Tokens) -> String {
    let r = SIZE / 2. - SW;
    let c = SIZE / 2.;
    let circ = 2. * std::f32::consts::PI * r;
    let off = circ * (1. - v.pct);
    let track = t.line.to_css();
    let stroke = color.to_css();
    let op = if v.ready { fill_opacity } else { 0. };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{SIZE}" height="{SIZE}" viewBox="0 0 {SIZE} {SIZE}"><circle cx="{c}" cy="{c}" r="{r}" fill="none" stroke="{track}" stroke-width="{SW}"/><circle cx="{c}" cy="{c}" r="{r}" fill="none" stroke="{stroke}" stroke-width="{SW}" stroke-linecap="round" stroke-dasharray="{circ}" stroke-dashoffset="{off}" transform="rotate(-90 {c} {c})" opacity="{op}"/></svg>"#
    )
}

pub struct ContextMeter {
    usage: Option<ContextUsage>,
    open: bool,
    epoch: Instant,
    _sub: Option<Subscription>,
}

impl ContextMeter {
    pub fn new() -> Self {
        ContextMeter {
            usage: None,
            open: false,
            epoch: Instant::now(),
            _sub: None,
        }
    }

    pub fn set_usage(&mut self, usage: Option<ContextUsage>, cx: &mut Context<Self>) {
        if usage != self.usage {
            self.usage = usage;
            if !meter_values(self.usage.as_ref()).ready {
                self.open = false;
            }
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }
}

impl Default for ContextMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextMeter {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let v = meter_values(self.usage.as_ref());
        let color = ring_color(&v, &t);
        // `ctx-pulse 1.1s ease-in-out infinite` on the fill while compacting.
        let fill_opacity = if v.compacting && !motion::reduced(cx) {
            let p = (motion::elapsed(self.epoch, cx).as_secs_f32() % 1.1) / 1.1;
            let half = if p < 0.5 { p * 2. } else { (1. - p) * 2. };
            if !motion::is_frozen(cx) {
                window.request_animation_frame();
            }
            1. - 0.55 * motion::Easing::EASE_IN_OUT.apply(half)
        } else {
            1.
        };
        let svg = ring_svg(&v, color, fill_opacity, &t);
        let open = self.open;
        let ready = v.ready;
        let tip = if ready {
            "Context usage"
        } else {
            "Context usage updates after a turn finishes"
        };
        let btn = div()
            .id("ctx-btn")
            .size(px(24.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .when(open, |d| d.bg(hsla(t.pane_2)))
            .when(!ready, |d| d.opacity(0.45))
            .when(ready, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(hsla(t.pane_2)))
                    .on_click(cx.guarded_listener(|m, _, _, cx| {
                        cx.stop_propagation();
                        m.open = !m.open;
                        cx.notify();
                    }))
            })
            .child(svg_markup(svg).size(px(SIZE)));
        let mut root = div()
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .child(recur_ui::tooltip::titled("ctx-btn-tip", tip, btn));
        if open && ready {
            let row = |a: &str, b: String, sub: bool| {
                div()
                    .flex()
                    .justify_between()
                    .gap(px(12.))
                    .child(label(a.to_string(), Family::Ui, 12., 400.).color(if sub {
                        t.faint
                    } else {
                        t.muted
                    }))
                    .child(label(b, Family::Mono, 11., 400.).color(if sub {
                        t.muted
                    } else {
                        t.text
                    }))
            };
            let mut rows = div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(row("Used", format!("{}%", v.pct_label), false))
                .child(row("Free space", format!("{}%", v.free_label), false));
            if v.compacting {
                rows = rows.child(row("Compacting", "in progress".into(), false));
            } else if v.compacted {
                rows = rows.child(row("Last compact", "this session".into(), false));
            }
            if v.show_split {
                rows = rows
                    .child(row("Cache read", format_tokens(v.cache_read), true))
                    .child(row("Fresh input", format_tokens(v.fresh), true));
            }
            let pop = recur_ui::style::card(t.pane_2, t.line_bright, 9.)
                .w(px(220.))
                .px(px(12.))
                .py(px(11.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .gap(px(10.))
                        .mb(px(8.))
                        .child(
                            label(
                                if v.compacting {
                                    "Compacting"
                                } else {
                                    "Context"
                                },
                                Family::Ui,
                                12.,
                                600.,
                            )
                            .color(t.text),
                        )
                        .child(
                            label(
                                format!("{}/{}", format_tokens(v.used), format_tokens(v.limit)),
                                Family::Mono,
                                11.,
                                400.,
                            )
                            .color(t.muted),
                        ),
                )
                .child(
                    div()
                        .h(px(4.))
                        .rounded(px(2.))
                        .bg(hsla(t.line))
                        .overflow_hidden()
                        .mb(px(8.))
                        .child(
                            div()
                                .h_full()
                                .rounded(px(2.))
                                .w(relative(v.pct_label as f32 / 100.))
                                .bg(hsla(color)),
                        ),
                )
                .child(rows);
            root = root.child(
                div()
                    .absolute()
                    .right_0()
                    .bottom(px(24. + 8.))
                    .occlude()
                    .child(with_shadow(
                        pop,
                        &[css_box_shadow(0., 12., 32., 0., rgba(0, 0, 0, 0.45))],
                        9.,
                    )),
            );
        }
        root
    }
}

impl Render for ContextMeter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_colors() {
        let t = recur_ui::ThemeId::default().tokens();
        let none = meter_values(None);
        assert!(!none.ready);
        assert_eq!(ring_color(&none, &t), t.line);
        let u = ContextUsage {
            input_tokens: 90_000,
            cache_creation_tokens: 0,
            cache_read_tokens: 10_000,
            context_window: 100_000,
            compacting: false,
            compacted: false,
        };
        let v = meter_values(Some(&u));
        assert!(v.ready && v.pct_label == 100 && v.show_split);
        assert_eq!(ring_color(&v, &t), t.coral);
    }
}
