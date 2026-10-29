//! Chat style primitives the transcript and dock reuse: the literal colors the styles
//! hard-codes, the bordered pills (`.tc-chip`, `.tc-file`, `.tc-url`, status chips, …), the
//! agent tints and small text builders.

use gpui_kit::{Div, Hsla, IntoElement, ParentElement, Styled, Window, div, px};
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, Text, TextStyled, text};
use recur_ui::{Color, IconName, Tokens, hex, icon, rgba};
pub use recur_view_widgets::pill::{Pill, amber_a, coral_a, cyan_a, sev_pill};

/// `#3fb950`: copied checks, `+` diff counts, add gutters.
pub const GREEN: Color = hex(0x3fb950);
/// `#e0655f`: error text, `−` counts, delete gutters.
pub const ERR: Color = hex(0xe0655f);
/// `#0A0C10`: text on coral / emerald fills.
pub const INK: Color = hex(0x0a0c10);
/// `#0b0e12`: the check on a done todo box.
pub const INK_2: Color = hex(0x0b0e12);

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
pub fn line_box(
    child: impl IntoElement,
    strut_size: f32,
    strut_leading: f32,
    baseline: f32,
    height: f32,
    window: &Window,
) -> Div {
    let (_, _, s) = text("x")
        .size(strut_size)
        .line_height(LineHeight::Scale(strut_leading))
        .measure(None, window);
    let above = s.baseline.max(baseline);
    let below = (s.height - s.baseline).max(height - baseline);
    // Exact gaps above and below the box: GPUI would round authored fractional lengths.
    div()
        .flex()
        .flex_col()
        .child(recur_ui::exact::vgap(above - baseline, window))
        .child(div().child(child))
        .child(recur_ui::exact::vgap(below - (height - baseline), window))
}

/// The baseline of one line of text from its line box's top.
pub fn text_baseline(t: Text, window: &Window) -> (f32, f32) {
    let (_, _, l) = t.measure(None, window);
    (l.baseline, l.height)
}

pub fn label(s: impl Into<gpui_kit::SharedString>, family: Family, size: f32, weight: f32) -> Text {
    text(s).family(family).size(size).weight(weight).nowrap()
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
    ico(
        recur_view_widgets::findings_ui::finding_sev_icon(sev),
        size,
        color,
    )
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
        "pending" => Pill {
            fg: t.muted,
            ..base
        },
        _ => base,
    }
}
