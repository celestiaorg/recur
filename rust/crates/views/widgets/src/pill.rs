//! `_chat.scss`'s bordered inline pills (`.tc-chip`, `.tc-file`, `.tc-sev`, …) and the tints
//! they wash with, shared by the chat's rows and the Observatory's findings.

use gpui_kit::{Div, ParentElement, Styled, px};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Color, Tokens, rgba};

pub const fn amber_a(a: f32) -> Color {
    rgba(242, 179, 61, a)
}

pub const fn cyan_a(a: f32) -> Color {
    rgba(76, 198, 214, a)
}

pub const fn coral_a(a: f32) -> Color {
    rgba(224, 101, 95, a)
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
