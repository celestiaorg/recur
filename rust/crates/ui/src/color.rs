//! CSS color math: hex parsing, `rgba()`, `color-mix(in srgb, …)` and the luminance rule
//! `lib/project-pic.ts` uses. Results are kept as unrounded sRGB floats, exactly as a
//! browser holds a computed color before rasterizing it.

use gpui_kit::{Hsla, Rgba};

/// An sRGB color with straight (non-premultiplied) alpha, components in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub const TRANSPARENT: Color = Color {
    r: 0.,
    g: 0.,
    b: 0.,
    a: 0.,
};

/// `#rrggbb`.
pub const fn hex(rgb: u32) -> Color {
    Color {
        r: ((rgb >> 16) & 0xff) as f32 / 255.,
        g: ((rgb >> 8) & 0xff) as f32 / 255.,
        b: (rgb & 0xff) as f32 / 255.,
        a: 1.,
    }
}

/// `rgba(r, g, b, a)` with 0–255 channels.
pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a,
    }
}

impl Color {
    pub fn from_hsla(c: Hsla) -> Color {
        let r = Rgba::from(c);
        Color {
            r: r.r,
            g: r.g,
            b: r.b,
            a: r.a,
        }
    }

    pub const fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// `color-mix(in srgb, self p%, other)`. CSS Color 5 §3: interpolate premultiplied
    /// components, then un-premultiply. `p` is `0.0..=1.0`.
    pub fn mix(self, p: f32, other: Color) -> Color {
        let q = 1. - p;
        let a = self.a * p + other.a * q;
        if a == 0. {
            return TRANSPARENT;
        }
        let ch = |x: f32, y: f32| (x * self.a * p + y * other.a * q) / a;
        Color {
            r: ch(self.r, other.r),
            g: ch(self.g, other.g),
            b: ch(self.b, other.b),
            a,
        }
    }

    /// `color-mix(in srgb, self p%, transparent)`.
    pub fn fade(self, p: f32) -> Color {
        self.mix(p, TRANSPARENT)
    }

    /// Source-over composite of `self` onto an opaque `below` (what a translucent fill
    /// looks like once painted).
    pub fn over(self, below: Color) -> Color {
        let a = self.a + below.a * (1. - self.a);
        if a == 0. {
            return TRANSPARENT;
        }
        let ch = |x: f32, y: f32| (x * self.a + y * below.a * (1. - self.a)) / a;
        Color {
            r: ch(self.r, below.r),
            g: ch(self.g, below.g),
            b: ch(self.b, below.b),
            a,
        }
    }

    /// CSS `filter: brightness(k)`: a linear multiply of each sRGB channel, clamped.
    pub fn brightness(self, k: f32) -> Color {
        Color {
            r: (self.r * k).min(1.),
            g: (self.g * k).min(1.),
            b: (self.b * k).min(1.),
            a: self.a,
        }
    }

    /// WCAG relative luminance, with the 0.03928 knee `lib/project-pic.ts` uses.
    pub fn luminance(self) -> f32 {
        let lin = |c: f32| {
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// Lowercase `#rrggbb` of the 8-bit color a browser would paint.
    pub fn to_hex(self) -> String {
        let c = |x: f32| (x * 255.).round().clamp(0., 255.) as u8;
        format!("#{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b))
    }

    /// CSS serialization used when a color is written into generated SVG markup.
    pub fn to_css(self) -> String {
        if self.a >= 1. {
            return self.to_hex();
        }
        let c = |x: f32| (x * 255.).round().clamp(0., 255.) as u8;
        format!("rgba({},{},{},{})", c(self.r), c(self.g), c(self.b), self.a)
    }

    /// Parse `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb(…)` / `rgba(…)` and `transparent`.
    /// `var(--token)` needs a theme: see [`crate::theme::Theme::resolve_css_color`].
    pub fn parse(css: &str) -> Option<Color> {
        let t = css.trim();
        if t.eq_ignore_ascii_case("transparent") {
            return Some(TRANSPARENT);
        }
        if let Some(h) = t.strip_prefix('#') {
            let nib = |i: usize| u8::from_str_radix(h.get(i..i + 1)?, 16).ok();
            let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
            return match h.len() {
                3 => Some(rgba(nib(0)? * 17, nib(1)? * 17, nib(2)? * 17, 1.)),
                6 => Some(rgba(byte(0)?, byte(2)?, byte(4)?, 1.)),
                8 => Some(rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)? as f32 / 255.)),
                _ => None,
            };
        }
        let lower = t.to_ascii_lowercase();
        let inner = lower
            .strip_prefix("rgba(")
            .or_else(|| lower.strip_prefix("rgb("))?
            .strip_suffix(')')?;
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let chan = |s: &str| -> Option<f32> {
            match s.strip_suffix('%') {
                Some(p) => Some(p.parse::<f32>().ok()? / 100.),
                None => Some(s.parse::<f32>().ok()? / 255.),
            }
        };
        let alpha = match parts.get(3) {
            Some(s) => match s.strip_suffix('%') {
                Some(p) => p.parse::<f32>().ok()? / 100.,
                None => s.parse::<f32>().ok()?,
            },
            None => 1.,
        };
        Some(Color {
            r: chan(parts[0])?.clamp(0., 1.),
            g: chan(parts[1])?.clamp(0., 1.),
            b: chan(parts[2])?.clamp(0., 1.),
            a: alpha.clamp(0., 1.),
        })
    }
}

impl From<Color> for Rgba {
    fn from(c: Color) -> Rgba {
        Rgba {
            r: c.r,
            g: c.g,
            b: c.b,
            a: c.a,
        }
    }
}

impl From<Color> for Hsla {
    fn from(c: Color) -> Hsla {
        Rgba::from(c).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(c: Color) -> [u8; 4] {
        let q = |x: f32| (x * 255.).round() as u8;
        [q(c.r), q(c.g), q(c.b), q(c.a)]
    }

    #[test]
    fn parses_css_forms() {
        assert_eq!(Color::parse("#0A0C10"), Some(hex(0x0a0c10)));
        assert_eq!(Color::parse("#abc"), Some(hex(0xaabbcc)));
        assert_eq!(
            Color::parse("rgba(242, 179, 61, .55)"),
            Some(rgba(242, 179, 61, 0.55))
        );
        assert_eq!(Color::parse("rgb(1 2 3)"), Some(rgba(1, 2, 3, 1.)));
        assert_eq!(Color::parse("transparent"), Some(TRANSPARENT));
        assert_eq!(Color::parse("var(--cyan)"), None);
    }

    #[test]
    fn color_mix_matches_chromium() {
        // --coral-line in Charcoal: color-mix(in srgb, #f87171 42%, #2e2e2e).
        let c = hex(0xf87171).mix(0.42, hex(0x2e2e2e));
        assert!((c.r - (248. * 0.42 + 46. * 0.58) / 255.).abs() < 1e-6);
        assert_eq!(bytes(c), [131, 74, 74, 255]);
        // Translucent mixes stay premultiplied: amber 70% over transparent keeps the hue.
        let ring = hex(0xe07a5f).fade(0.7);
        assert_eq!(bytes(ring), [224, 122, 95, 179]);
    }

    #[test]
    fn luminance_knee_matches_project_pic() {
        assert!(hex(0x0a0c10).luminance() <= 0.12);
        assert!(hex(0x6eb6ff).luminance() > 0.12);
    }

    #[test]
    fn over_and_brightness() {
        let painted = rgba(255, 255, 255, 0.5).over(hex(0x000000));
        assert_eq!(bytes(painted), [128, 128, 128, 255]);
        assert_eq!(bytes(hex(0x808080).brightness(1.07)), [137, 137, 137, 255]);
    }
}
