//! Comment and commit avatars: `.ca` / `.cav` boxes painted from an inline `style` (`avatarCss`,
//! `background:{color}`), with the initials centered and the avatar image over them.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    App, Div, ObjectFit, ParentElement, SharedString, Styled, StyledImage, div, img, px,
};
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::{Color, Theme};

use recur_ui::set::hsla;

/// A `linear-gradient(<angle>deg, a, b)` or a solid fill.
#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Solid(Color),
    Gradient(f32, Color, Color),
}

/// The background and text color an inline style declares (`var(--token)` resolved).
pub fn parse_avatar_css(css: &str, cx: &App) -> (Option<Fill>, Option<Color>) {
    let theme = Theme::of(cx);
    let mut fill = None;
    let mut color = None;
    for decl in split_decls(css) {
        let Some((k, v)) = decl.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "background" | "background-color" | "background-image" => {
                if let Some(g) = parse_gradient(v, theme) {
                    fill = Some(g);
                } else if let Some(c) = theme.resolve_css_color(v) {
                    fill = Some(Fill::Solid(c));
                }
            }
            "color" => color = theme.resolve_css_color(v),
            _ => {}
        }
    }
    (fill, color)
}

/// Split declarations on `;` outside parentheses.
fn split_decls(css: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in css.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' if depth == 0 => {
                out.push(&css[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&css[start..]);
    out
}

fn parse_gradient(v: &str, theme: &Theme) -> Option<Fill> {
    let inner = v
        .trim()
        .strip_prefix("linear-gradient(")?
        .strip_suffix(')')?;
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(inner[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(inner[start..].trim());
    let (angle, colors) = match parts.first()?.strip_suffix("deg") {
        Some(a) => (a.trim().parse::<f32>().ok()?, &parts[1..]),
        None => (180., &parts[..]),
    };
    let [a, b] = colors else { return None };
    Some(Fill::Gradient(
        angle,
        theme.resolve_css_color(a)?,
        theme.resolve_css_color(b)?,
    ))
}

/// A `size`-px avatar box with `radius`, mono initials at `font` px weight 700 in the style's
/// color, and the avatar image covering it.
pub fn avatar(
    code: &str,
    css: &str,
    url: &str,
    size: f32,
    radius: f32,
    font: f32,
    cx: &App,
) -> Div {
    let (fill, color) = parse_avatar_css(css, cx);
    avatar_with(
        code,
        fill,
        color,
        url,
        size,
        radius,
        font,
        LineHeight::Scale(1.45),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn avatar_with(
    code: &str,
    fill: Option<Fill>,
    color: Option<Color>,
    url: &str,
    size: f32,
    radius: f32,
    font: f32,
    lh: LineHeight,
) -> Div {
    let mut d = div()
        .relative()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .rounded(px(radius))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center();
    match fill {
        Some(Fill::Solid(c)) => d = d.bg(hsla(c)),
        Some(Fill::Gradient(angle, a, b)) => {
            d = d.child(recur_ui::style::css_gradient(
                angle,
                (a, 0.),
                (b, 1.),
                radius,
                0.,
                recur_ui::color::TRANSPARENT,
            ))
        }
        None => {}
    }
    let mut label = text(code.to_string())
        .family(Family::Mono)
        .size(font)
        .weight(700.)
        .line_height(lh)
        .nowrap();
    if let Some(c) = color {
        label = label.color(c);
    }
    d.child(label).when(!url.is_empty(), |d| {
        d.child(
            img(SharedString::from(url.to_string()))
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
    })
}
