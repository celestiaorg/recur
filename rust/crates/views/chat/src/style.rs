//! `_chat.scss` primitives the transcript and dock reuse: the literal colors the stylesheet
//! hard-codes, the bordered pills (`.tc-chip`, `.tc-file`, `.tc-url`, status chips, …), the
//! agent tints and small text builders.

use gpui_kit::{Div, Hsla, IntoElement, ParentElement, Styled, Window, div, px};
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, Text, TextStyled, text};
use recur_ui::{Color, IconName, Tokens, hex, icon, rgba};

/// `#3fb950`: copied checks, `+` diff counts, add gutters.
pub const GREEN: Color = hex(0x3fb950);
/// `#e0655f`: error text, `−` counts, delete gutters.
pub const ERR: Color = hex(0xe0655f);
/// `#0A0C10`: text on coral / emerald fills.
pub const INK: Color = hex(0x0a0c10);
/// `#0b0e12`: the check on a done todo box.
pub const INK_2: Color = hex(0x0b0e12);

pub const fn amber_a(a: f32) -> Color {
    rgba(242, 179, 61, a)
}

pub const fn cyan_a(a: f32) -> Color {
    rgba(76, 198, 214, a)
}

pub const fn coral_a(a: f32) -> Color {
    rgba(224, 101, 95, a)
}

pub const fn green_a(a: f32) -> Color {
    rgba(63, 185, 80, a)
}

/// `agColor`: an agent code's tint.
pub fn ag_color(code: &str, t: &Tokens) -> Option<Color> {
    Some(match code {
        "MA" => t.amber,
        "CO" => t.cyan,
        "QA" => t.coral,
        "RV" => t.indigo,
        "TE" => t.emerald,
        "PR" => hex(0xb08bd8),
        _ => return None,
    })
}

pub fn hsla(c: Color) -> Hsla {
    c.into()
}

/// A nowrap text in `family` at `size` and `weight`.
/// An `inline-flex` box alone on a block's line: the line is at least the block's strut
/// (`strut_size` / `strut_leading`, UI font), and the box's baseline (`baseline` from its top,
/// `height` tall) sits on the strut's baseline.
pub fn line_box(child: impl IntoElement, strut_size: f32, strut_leading: f32, baseline: f32, height: f32, window: &Window) -> Div {
    let (_, _, s) = text("x").size(strut_size).line_height(LineHeight::Scale(strut_leading)).measure(None, window);
    let above = s.baseline.max(baseline);
    let below = (s.height - s.baseline).max(height - baseline);
    div().flex().h(px(above + below)).child(div().mt(px(above - baseline)).child(child))
}

/// The baseline of one line of text from its line box's top.
pub fn text_baseline(t: Text, window: &Window) -> (f32, f32) {
    let (_, _, l) = t.measure(None, window);
    (l.baseline, l.height)
}

pub fn label(s: impl Into<gpui_kit::SharedString>, family: Family, size: f32, weight: f32) -> Text {
    text(s).family(family).size(size).weight(weight).nowrap()
}

/// A bordered inline pill: `font-size`, mono or UI, `line-height` px, `padding: 0 px`,
/// 1px border and radius.
#[derive(Clone, Copy)]
pub struct Pill {
    pub family_mono: bool,
    pub size: f32,
    pub weight: f32,
    pub tracking: f32,
    pub uppercase: bool,
    pub line: f32,
    pub pad_x: f32,
    pub radius: f32,
    pub fg: Color,
    pub bg: Color,
    pub border: Color,
    pub tabular: bool,
}

impl Pill {
    /// `.tc-chip`: mono 10.5, muted on `--pane-2`, `--line` border, radius 5, 16px line.
    pub fn chip(t: &Tokens) -> Pill {
        Pill {
            family_mono: true,
            size: 10.5,
            weight: 400.,
            tracking: 0.,
            uppercase: false,
            line: 16.,
            pad_x: 6.,
            radius: 5.,
            fg: t.muted,
            bg: t.pane_2,
            border: t.line,
            tabular: false,
        }
    }

    /// `.tc-file` / `.tc-task-id`: cyan on its 8% wash.
    pub fn cyan(t: &Tokens) -> Pill {
        Pill {
            fg: t.cyan,
            bg: cyan_a(0.08),
            border: cyan_a(0.22),
            ..Pill::chip(t)
        }
    }

    /// `.tc-sev` / `.tc-finding-status` / `.tc-art-kind`: mono 10 semibold uppercase.
    pub fn status(t: &Tokens, tracking: f32) -> Pill {
        Pill {
            size: 10.,
            weight: 600.,
            tracking,
            uppercase: true,
            ..Pill::chip(t)
        }
    }

    pub fn render(self, s: impl Into<gpui_kit::SharedString>) -> Div {
        let mut tx = text(s)
            .size(self.size)
            .weight(self.weight)
            .color(self.fg)
            .line_height(LineHeight::Px(self.line))
            .nowrap();
        tx = if self.family_mono { tx.mono() } else { tx.ui() };
        if self.tracking != 0. {
            tx = tx.tracking(self.tracking);
        }
        if self.uppercase {
            tx = tx.uppercase();
        }
        if self.tabular {
            tx = tx.tabular();
        }
        recur_ui::style::card(self.bg, self.border, self.radius)
            .flex_none()
            .px(px(self.pad_x))
            .child(tx)
    }

    /// A pill whose text truncates inside `max_w`.
    pub fn render_truncated(self, s: impl Into<gpui_kit::SharedString>) -> Div {
        let mut tx = text(s)
            .size(self.size)
            .weight(self.weight)
            .color(self.fg)
            .line_height(LineHeight::Px(self.line))
            .truncate();
        tx = if self.family_mono { tx.mono() } else { tx.ui() };
        recur_ui::style::card(self.bg, self.border, self.radius)
            .min_w_0()
            .overflow_hidden()
            .px(px(self.pad_x))
            .child(tx)
    }
}

/// A 14px icon slot (`.skill-ico`, `.tc-art-ico`, …) in `color`.
pub fn ico(name: IconName, size: f32, color: Color) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size))
        .child(icon(name).size(px(size)).color(hsla(color)))
}

/// `.finding-sev`: the severity icon in its color.
pub fn finding_sev(sev: &str, size: f32, t: &Tokens) -> impl IntoElement {
    let color = match sev {
        "medium" => t.amber,
        "high" | "critical" => t.coral,
        _ => t.muted,
    };
    ico(crate::findings_ui::finding_sev_icon(sev), size, color)
}

/// `.tc-finding-status.<status>` colors.
pub fn status_pill(status: &str, t: &Tokens) -> Pill {
    let base = Pill {
        fg: t.faint,
        ..Pill::status(t, 0.03)
    };
    match status {
        "resolved" | "done" => Pill {
            fg: t.emerald,
            border: green_a(0.35),
            bg: green_a(0.08),
            ..base
        },
        "invalid" => Pill {
            fg: t.muted,
            border: t.line,
            bg: t.pane_2,
            ..base
        },
        "ignored" | "skipped" => base,
        "doing" => Pill {
            fg: t.amber,
            border: amber_a(0.35),
            bg: amber_a(0.08),
            ..base
        },
        "pending" => Pill { fg: t.muted, ..base },
        _ => base,
    }
}

/// `.tc-sev.<class>` (lint severities).
pub fn sev_pill(class: &str, t: &Tokens) -> Pill {
    let base = Pill::status(t, 0.03);
    match class {
        "critical" | "high" => Pill {
            fg: t.coral,
            border: coral_a(0.35),
            bg: coral_a(0.08),
            ..base
        },
        "medium" => Pill {
            fg: t.amber,
            border: amber_a(0.35),
            bg: amber_a(0.08),
            ..base
        },
        _ => base,
    }
}
