//! `Ring.svelte`: progress ring with five iteration dots and a centered iteration count.

use crate::color::Color;
use crate::fonts::weight;
use crate::icon::svg_markup;
use crate::text::{TextStyled, text};
use crate::theme::Theme;
use gpui_kit::{App, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px};

#[derive(IntoElement)]
pub struct Ring {
    size: f32,
    prog: f32,
    iter: u32,
    color: Option<Color>,
}

/// `<Ring size prog iter color>`; color defaults to `--amber`.
pub fn ring(size: f32, prog: f32, iter: u32) -> Ring {
    Ring {
        size,
        prog,
        iter,
        color: None,
    }
}

impl Ring {
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
}

/// The SVG `Ring.svelte` builds, with `color` already resolved.
pub fn ring_svg(size: f32, prog: f32, color: Color) -> String {
    let big = size > 90.;
    let r = size / 2. - if big { 7. } else { 5. };
    let c = size / 2.;
    let circ = 2. * std::f32::consts::PI * r;
    let sw = if big { 7. } else { 4.5 };
    let off = circ * (1. - prog);
    let fill = color.to_css();
    let mut dots = String::new();
    for i in 0..5 {
        let a = ((-90. + i as f32 * 72.) * std::f32::consts::PI) / 180.;
        let f = if i as f32 / 5. <= prog {
            fill.clone()
        } else {
            "#2a3340".into()
        };
        dots.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{f}\"/>",
            c + r * a.cos(),
            c + r * a.sin(),
            if big { 3.2 } else { 2.3 }
        ));
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" viewBox=\"0 0 {size} {size}\"><circle cx=\"{c}\" cy=\"{c}\" r=\"{r}\" fill=\"none\" stroke=\"#1b212a\" stroke-width=\"{sw}\"/><circle cx=\"{c}\" cy=\"{c}\" r=\"{r}\" fill=\"none\" stroke=\"{fill}\" stroke-width=\"{sw}\" stroke-linecap=\"round\" stroke-dasharray=\"{circ}\" stroke-dashoffset=\"{off}\" transform=\"rotate(-90 {c} {c})\"/>{dots}</svg>"
    )
}

impl Ring {
    fn render_view(self, _: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let color = self.color.unwrap_or(t.amber);
        let big = self.size > 90.;
        let count_size = if big {
            24.
        } else if self.size < 42. {
            11.
        } else {
            13.
        };
        let mut label = div().flex().flex_col().items_center().child(
            text(self.iter.to_string())
                .mono()
                .weight(weight::SEMIBOLD)
                .leading(1.)
                .size(count_size)
                .color(color)
                .nowrap(),
        );
        if big {
            label = label.child(
                text("iter")
                    .mono()
                    .uppercase()
                    .tracking(0.1)
                    .size(8.)
                    .color(t.faint)
                    .nowrap(),
            );
        }
        div()
            .relative()
            .flex_none()
            .size(px(self.size))
            .child(svg_markup(ring_svg(self.size, self.prog, color)).size(px(self.size)))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(label),
            )
    }
}

impl RenderOnce for Ring {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        crate::guard::component(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::hex;

    #[test]
    fn dots_fill_up_to_progress() {
        let svg = ring_svg(30., 0.4, hex(0xe07a5f));
        // i/5 <= 0.4 → dots 0, 1, 2 filled.
        assert_eq!(svg.matches("fill=\"#e07a5f\"").count(), 3);
        assert_eq!(svg.matches("fill=\"#2a3340\"").count(), 2);
        assert!(ring_svg(100., 0.6, hex(0)).contains("stroke-width=\"7\""));
    }
}
