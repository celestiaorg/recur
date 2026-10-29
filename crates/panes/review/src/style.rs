//! The review stylesheet (`_prs.scss`, `_observatory.scss`, `_settings.scss` comment rules):
//! the literal colors it hard-codes and the controls the review components share.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Div, ElementId, Hsla, InteractiveElement, ParentElement, SharedString, Stateful, Styled,
    Window, div, px,
};
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, Text, TextStyled, text};
use recur_ui::{Color, IconName, Theme, Tokens, hex, icon, rgba};

use crate::html::Token;

pub fn hsla(c: Color) -> Hsla {
    c.into()
}

/// The image pane's checkerboard (`--pane-2` 8px squares over `--bg`, `--bg` at the origin).
pub fn checkerboard(cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    let sq = t.pane_2;
    div().relative().bg(hsla(t.bg)).child(
        gpui_kit::canvas(
            |_, _, _| {},
            move |b, _, window, _| {
                let s = px(8.);
                let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                let (cols, rows) = ((w / 8.).ceil() as i32, (h / 8.).ceil() as i32);
                for r in 0..rows {
                    for c in 0..cols {
                        if (r + c) % 2 == 0 {
                            continue;
                        }
                        let o = b.origin + gpui_kit::point(s * c as f32, s * r as f32);
                        let q = gpui_kit::Bounds::new(o, gpui_kit::size(s, s)).intersect(&b);
                        window.paint_quad(gpui_kit::fill(q, hsla(sq)));
                    }
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

/// `.hline .cd .kw` and the other token colors (the same on every theme).
pub fn token_color(token: Token, t: &Tokens) -> Color {
    match token {
        Token::Kw => hex(0xc4a0f5),
        Token::Str => hex(0x9fe3b5),
        Token::Fn => t.amber,
        Token::Cm => t.faint,
        Token::Ty => t.cyan,
        Token::Nm => hex(0xe0b88a),
    }
}

/// `.hline.add` / `.hline.split .side.add`.
pub const ADD_BG: Color = rgba(93, 190, 126, 0.08);
/// `.hline.del` / `.hline.split .side.del`.
pub const DEL_BG: Color = rgba(235, 122, 92, 0.08);
/// `.hline.add .cd` on dark themes.
pub const ADD_CD: Color = hex(0xcfeed9);
/// `.hline.del .cd` on dark themes.
pub const DEL_CD: Color = hex(0xf0c3b5);
/// `.hline.hunk` and `.flag`.
pub const HUNK_BG: Color = rgba(148, 134, 244, 0.06);
/// `.hline.hunk:hover`.
pub const HUNK_HOVER_BG: Color = rgba(148, 134, 244, 0.1);
/// `.hline.split .side.blank`.
pub const BLANK_BG: Color = rgba(255, 255, 255, 0.02);
/// `.pending-pill`.
pub const PENDING_BG: Color = rgba(242, 179, 61, 0.12);
/// `.cmt .react`.
pub const REACT_BG: Color = rgba(76, 198, 214, 0.07);
pub const REACT_BORDER: Color = rgba(76, 198, 214, 0.2);
/// `.commit .cav` ink.
pub const CAV_INK: Color = hex(0x0a0c10);

/// Paper and Catppuccin Latte keep plain diff text readable (`--text` instead of the pastels).
pub fn light_diff_text(cx: &App) -> bool {
    matches!(Theme::of(cx).id.key(), "paper" | "catppuccin-latte")
}

/// Text in `family` at `size` with an explicit line height, on one line.
pub fn label(s: impl Into<SharedString>, family: Family, size: f32, lh: LineHeight) -> Text {
    text(s).family(family).size(size).line_height(lh).nowrap()
}

/// `.obs-icon-btn`: 26×26 (or `size`), radius 5, faint until hover, icon `icon_px`.
pub fn icon_btn(
    id: impl Into<ElementId>,
    name: IconName,
    size: f32,
    icon_px: f32,
    on: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    let fg = on.unwrap_or(t.faint);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(size))
        .h(px(size))
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(hsla(fg))
        .hover(|s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
        .child(icon(name).size(px(icon_px)))
}

/// `.obs-icon-btn:disabled`: faded to .35 over its backdrop, no hover.
pub fn icon_btn_disabled(
    id: impl Into<ElementId>,
    name: IconName,
    size: f32,
    icon_px: f32,
    backdrop: Color,
    cx: &App,
) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    let fg = recur_ui::style::faded(t.faint, recur_ui::color::TRANSPARENT, 0.35, backdrop);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(size))
        .h(px(size))
        .rounded(px(5.))
        .text_color(hsla(fg))
        .child(icon(name).size(px(icon_px)))
}

/// `.finding-note-btn`: 24px, padding 0 8, 1px border, radius 6, 11px; `.primary` keeps the
/// text color on `--pane` with the bright border. Disabled fades to .4 over `backdrop`.
pub fn note_btn(
    id: impl Into<ElementId>,
    label_text: impl Into<SharedString>,
    primary: bool,
    disabled: bool,
    backdrop: Color,
    lh: f32,
    cx: &App,
) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    let (fg, bg, border) = if primary {
        (t.text, t.pane, t.line_bright)
    } else {
        (t.muted, t.pane_2, t.line)
    };
    let (fg, bg, border) = if disabled {
        let f = |c: Color| recur_ui::style::faded(c, bg, 0.4, backdrop);
        (f(fg), f(bg), f(border))
    } else {
        (fg, bg, border)
    };
    let hover_border = if primary { t.muted } else { t.line_bright };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(24.))
        .px(px(8.))
        .rounded(px(6.))
        .border_1()
        .border_color(hsla(border))
        .bg(hsla(bg))
        .text_color(hsla(fg))
        .child(
            text(label_text.into())
                .size(11.)
                .line_height(LineHeight::Px(lh))
                .nowrap(),
        )
        .when(!disabled, |d| {
            d.cursor_pointer().hover(move |s| {
                if primary {
                    s.border_color(hsla(hover_border))
                } else {
                    s.text_color(hsla(t.text)).border_color(hsla(hover_border))
                }
            })
        })
}

/// `.seg` with `.commit-diff-layout` buttons: 26×22 icon buttons (13px icons), `.on` filled.
pub fn seg(children: impl IntoIterator<Item = gpui_kit::AnyElement>, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    recur_ui::style::card(t.pane, t.line, 6.)
        .flex()
        .flex_none()
        .p(px(2.))
        .children(children)
}

/// One `.seg` button.
pub fn seg_btn(
    id: impl Into<ElementId>,
    name: IconName,
    on: bool,
    dim_backdrop: Option<Color>,
    cx: &App,
) -> Stateful<Div> {
    let t = Theme::of(cx).tokens;
    let fg = if on { t.text } else { t.muted };
    let bg = if on {
        t.pane_2
    } else {
        recur_ui::color::TRANSPARENT
    };
    let (fg, bg) = match dim_backdrop {
        Some(b) => {
            let f = |c: Color| recur_ui::style::faded(c, bg, 0.4, b);
            (f(fg), f(bg))
        }
        None => (fg, bg),
    };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(26.))
        .h(px(22.))
        .rounded(px(4.))
        .bg(hsla(bg))
        .text_color(hsla(fg))
        .when(dim_backdrop.is_none(), |d| d.cursor_pointer())
        .child(icon(name).size(px(13.)))
}

/// The `+N −M` pair (`.a` emerald, `.r` coral) with `gap`.
pub fn add_del(
    add: i64,
    del: i64,
    family: Family,
    size: f32,
    gap: f32,
    lh: LineHeight,
    cx: &App,
) -> Div {
    let t = Theme::of(cx).tokens;
    div()
        .flex()
        .flex_none()
        .gap(px(gap))
        .child(label(format!("+{add}"), family.clone(), size, lh).color(t.emerald))
        .child(label(format!("−{del}"), family, size, lh).color(t.coral))
}

/// A window-level width in CSS px.
pub fn viewport_width(window: &Window) -> f32 {
    f32::from(window.viewport_size().width)
}

/// Colors a review textarea (`.finding-note-input`): text color, the faint `::placeholder`,
/// no fill or border of its own. Call when the theme may have changed.
pub fn style_textarea(area: &gpui_kit::Entity<gpui_kit::base::input::TextareaState>, cx: &mut App) {
    let t = Theme::of(cx).tokens;
    let caret: Hsla = if recur_ui::motion::is_frozen(cx) {
        t.text.with_alpha(0.001).into()
    } else {
        t.text.into()
    };
    area.update(cx, |s, _| {
        s.set_editor_style(gpui_kit::base::input::InputEditorStyle {
            foreground: t.text.into(),
            muted_foreground: t.faint.into(),
            background: Hsla::transparent_black(),
            border: Hsla::transparent_black(),
            selection: t.amber.fade(0.3).into(),
            caret,
            ..Default::default()
        })
    });
}
