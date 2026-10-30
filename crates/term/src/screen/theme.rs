//! Terminal colours, resolved the way xterm.js's `ThemeService` resolves an `ITheme`.

use serde::{Deserialize, Serialize};

/// A colour packed as `0xRRGGBBAA`, the representation xterm.js computes with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgba(pub u32);

/// JavaScript's `Math.round`: halves round towards positive infinity.
pub(crate) fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba(0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 0xff)
    }

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba((r as u32) << 24 | (g as u32) << 16 | (b as u32) << 8 | a as u32)
    }

    pub fn r(self) -> u8 {
        (self.0 >> 24) as u8
    }

    pub fn g(self) -> u8 {
        (self.0 >> 16) as u8
    }

    pub fn b(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub fn a(self) -> u8 {
        self.0 as u8
    }

    pub fn is_opaque(self) -> bool {
        self.a() == 0xff
    }

    pub fn opaque(self) -> Self {
        Rgba(self.0 | 0xff)
    }

    /// `color.opacity`: replaces alpha with `opacity` (0..1).
    pub fn with_opacity(self, opacity: f64) -> Self {
        Self::new(
            self.r(),
            self.g(),
            self.b(),
            js_round(opacity * 255.0) as u8,
        )
    }

    /// `color.multiplyOpacity`.
    pub fn multiply_opacity(self, factor: f64) -> Self {
        self.with_opacity(self.a() as f64 * factor / 255.0)
    }

    /// `color.blend(bg, fg)`: `fg` composited over `bg`, opaque result.
    pub fn blend(bg: Rgba, fg: Rgba) -> Rgba {
        let a = fg.a() as f64 / 255.0;
        if fg.a() == 0xff {
            return fg;
        }
        let mix = |b: u8, f: u8| (b as f64 + js_round((f as f64 - b as f64) * a)) as u8;
        Rgba::rgb(
            mix(bg.r(), fg.r()),
            mix(bg.g(), fg.g()),
            mix(bg.b(), fg.b()),
        )
    }

    /// WCAG relative luminance of the RGB channels.
    pub fn relative_luminance(self) -> f64 {
        let ch = |c: u8| {
            let s = c as f64 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        ch(self.r()) * 0.2126 + ch(self.g()) * 0.7152 + ch(self.b()) * 0.0722
    }

    /// WCAG contrast ratio of the RGB channels.
    pub fn contrast(self, other: Rgba) -> f64 {
        let (a, b) = (self.relative_luminance(), other.relative_luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// xterm.js's `ensureContrastRatio`: steps the colour 10% at a time away from `bg`'s
    /// luminance until it reaches `ratio`, trying the other direction when the first runs out.
    pub fn ensure_contrast(self, bg: Rgba, ratio: f64) -> Rgba {
        if self.contrast(bg) >= ratio {
            return self;
        }
        let walk = |f: fn(u8) -> u8| {
            let mut c = self;
            loop {
                let next = Rgba::new(f(c.r()), f(c.g()), f(c.b()), c.a());
                if c.contrast(bg) >= ratio || next == c {
                    return c;
                }
                c = next;
            }
        };
        let darker = || walk(|c| c - (c as f64 * 0.1).ceil() as u8);
        let lighter = || walk(|c| c + ((255 - c) as f64 * 0.1).ceil() as u8);
        let dark_first = self.relative_luminance() < bg.relative_luminance();
        let first = if dark_first { darker() } else { lighter() };
        if first.contrast(bg) >= ratio {
            return first;
        }
        let second = if dark_first { lighter() } else { darker() };
        if second.contrast(bg) > first.contrast(bg) {
            second
        } else {
            first
        }
    }

    /// `#rrggbb` or `#rrggbbaa` when translucent.
    pub fn to_hex(self) -> String {
        if self.is_opaque() {
            format!("#{:02x}{:02x}{:02x}", self.r(), self.g(), self.b())
        } else {
            format!("#{:08x}", self.0)
        }
    }

    /// xterm's `css.toColor` for the formats themes use: `#rgb[a]`, `#rrggbb[aa]`, `rgb()`,
    /// `rgba()` and `transparent`.
    pub fn parse(css: &str) -> Option<Rgba> {
        if let Some(hex) = css.strip_prefix('#')
            && hex.chars().all(|c| c.is_ascii_hexdigit())
        {
            let nib = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v * 17);
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
            return match hex.len() {
                3 => Some(Rgba::rgb(nib(0)?, nib(1)?, nib(2)?)),
                4 => Some(Rgba::new(nib(0)?, nib(1)?, nib(2)?, nib(3)?)),
                6 => Some(Rgba::rgb(byte(0)?, byte(2)?, byte(4)?)),
                8 => Some(Rgba::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
                _ => None,
            };
        }
        if css == "transparent" {
            return Some(Rgba::TRANSPARENT);
        }
        let inner = css
            .strip_prefix("rgba(")
            .or_else(|| css.strip_prefix("rgb("))?
            .strip_suffix(')')?;
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        let chan = |s: &str| {
            s.parse::<u16>()
                .ok()
                .filter(|v| *v <= 999 && s.len() <= 3)
                .map(|v| v.min(255) as u8)
        };
        let (r, g, b) = (
            chan(parts.first()?)?,
            chan(parts.get(1)?)?,
            chan(parts.get(2)?)?,
        );
        let a = match parts.get(3) {
            None => 1.0,
            Some(a) => a.parse::<f64>().ok().filter(|v| (0.0..=1.0).contains(v))?,
        };
        if parts.len() > 4 {
            return None;
        }
        Some(Rgba::new(r, g, b, js_round(a * 255.0) as u8))
    }
}

/// An xterm.js `ITheme`. Bundle `[[terminalTheme]]` palettes and the app-chrome mapping both
/// produce one; unset keys fall back to xterm's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Theme {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_foreground: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_inactive_background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollbar_slider_background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollbar_slider_hover_background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollbar_slider_active_background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub black: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub red: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub green: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yellow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magenta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cyan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_black: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_red: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_green: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_yellow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_blue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_magenta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_cyan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_white: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extended_ansi: Option<Vec<String>>,
}

impl Theme {
    fn ansi16(&self) -> [&Option<String>; 16] {
        [
            &self.black,
            &self.red,
            &self.green,
            &self.yellow,
            &self.blue,
            &self.magenta,
            &self.cyan,
            &self.white,
            &self.bright_black,
            &self.bright_red,
            &self.bright_green,
            &self.bright_yellow,
            &self.bright_blue,
            &self.bright_magenta,
            &self.bright_cyan,
            &self.bright_white,
        ]
    }
}

const DEFAULT_FOREGROUND: Rgba = Rgba::rgb(0xff, 0xff, 0xff);
const DEFAULT_BACKGROUND: Rgba = Rgba::rgb(0, 0, 0);
const DEFAULT_SELECTION: Rgba = Rgba(0xFFFF_FF4D);

/// xterm.js's default 256-colour palette.
pub fn default_ansi() -> [Rgba; 256] {
    const BASE: [u32; 16] = [
        0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, 0x555753,
        0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
    ];
    let mut out = [Rgba::TRANSPARENT; 256];
    for (i, c) in BASE.iter().enumerate() {
        out[i] = Rgba(c << 8 | 0xff);
    }
    const V: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    for i in 0..216 {
        out[16 + i] = Rgba::rgb(V[(i / 36) % 6], V[(i / 6) % 6], V[i % 6]);
    }
    for i in 0..24 {
        let c = (8 + i * 10) as u8;
        out[232 + i] = Rgba::rgb(c, c, c);
    }
    out
}

/// Resolved colours, as `ThemeService._setTheme` computes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub foreground: Rgba,
    pub background: Rgba,
    pub cursor: Rgba,
    pub cursor_accent: Rgba,
    pub selection_background_transparent: Rgba,
    pub selection_background_opaque: Rgba,
    pub selection_inactive_background_transparent: Rgba,
    pub selection_inactive_background_opaque: Rgba,
    pub selection_foreground: Option<Rgba>,
    pub scrollbar_slider_background: Rgba,
    pub scrollbar_slider_hover_background: Rgba,
    pub scrollbar_slider_active_background: Rgba,
    pub ansi: [Rgba; 256],
}

impl Default for Palette {
    fn default() -> Self {
        Self::from_theme(&Theme::default())
    }
}

impl Palette {
    pub fn from_theme(theme: &Theme) -> Self {
        let parse = |s: &Option<String>, fallback: Rgba| {
            s.as_deref().and_then(Rgba::parse).unwrap_or(fallback)
        };
        let foreground = parse(&theme.foreground, DEFAULT_FOREGROUND);
        let background = parse(&theme.background, DEFAULT_BACKGROUND);
        let mut sel = parse(&theme.selection_background, DEFAULT_SELECTION);
        let mut sel_inactive = parse(&theme.selection_inactive_background, sel);
        let sel_opaque = Rgba::blend(background, sel);
        let sel_inactive_opaque = Rgba::blend(background, sel_inactive);
        // An opaque selection would hide the text under it; xterm forces 30% opacity.
        if sel.is_opaque() {
            sel = sel.with_opacity(0.3);
        }
        if sel_inactive.is_opaque() {
            sel_inactive = sel_inactive.with_opacity(0.3);
        }
        let mut ansi = default_ansi();
        for (i, c) in theme.ansi16().into_iter().enumerate() {
            ansi[i] = parse(c, ansi[i]);
        }
        if let Some(ext) = &theme.extended_ansi {
            for (i, c) in ext.iter().take(240).enumerate() {
                ansi[16 + i] = Rgba::parse(c).unwrap_or(ansi[16 + i]);
            }
        }
        Self {
            foreground,
            background,
            cursor: Rgba::blend(background, parse(&theme.cursor, DEFAULT_FOREGROUND)),
            cursor_accent: Rgba::blend(background, parse(&theme.cursor_accent, DEFAULT_BACKGROUND)),
            selection_background_transparent: sel,
            selection_background_opaque: sel_opaque,
            selection_inactive_background_transparent: sel_inactive,
            selection_inactive_background_opaque: sel_inactive_opaque,
            selection_foreground: theme
                .selection_foreground
                .as_deref()
                .and_then(Rgba::parse)
                .filter(|c| c.0 != 0),
            scrollbar_slider_background: parse(
                &theme.scrollbar_slider_background,
                foreground.with_opacity(0.2),
            ),
            scrollbar_slider_hover_background: parse(
                &theme.scrollbar_slider_hover_background,
                foreground.with_opacity(0.4),
            ),
            scrollbar_slider_active_background: parse(
                &theme.scrollbar_slider_active_background,
                foreground.with_opacity(0.5),
            ),
            ansi,
        }
    }

    /// Dark when the background is darker than the foreground (`CSI ? 997 ; 1 n`).
    pub fn is_dark(&self) -> bool {
        self.background.relative_luminance() < self.foreground.relative_luminance()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_formats() {
        assert_eq!(Rgba::parse("#141414"), Some(Rgba::rgb(0x14, 0x14, 0x14)));
        assert_eq!(Rgba::parse("#fff"), Some(Rgba::rgb(255, 255, 255)));
        assert_eq!(Rgba::parse("#00000000"), Some(Rgba::TRANSPARENT));
        assert_eq!(
            Rgba::parse("rgba(255,255,255,.18)"),
            Some(Rgba::new(255, 255, 255, 46))
        );
        assert_eq!(Rgba::parse("rgba(0,0,0,.12)"), Some(Rgba::new(0, 0, 0, 31)));
        assert_eq!(Rgba::parse("rgb(1, 2, 3)"), Some(Rgba::rgb(1, 2, 3)));
        assert_eq!(Rgba::parse("bogus"), None);
    }

    #[test]
    fn blend_rounds_like_javascript() {
        // 0x14 + round((0xff - 0x14) * 0.18039) = 20 + round(42.39) = 62
        let got = Rgba::blend(Rgba::rgb(0x14, 0x14, 0x14), Rgba::new(255, 255, 255, 46));
        assert_eq!(got, Rgba::rgb(62, 62, 62));
        // Halves round towards positive infinity, as Math.round does.
        assert_eq!(
            (js_round(-2.5), js_round(2.5), js_round(-2.6)),
            (-2.0, 3.0, -3.0)
        );
    }

    #[test]
    fn theme_service_defaults() {
        let p = Palette::default();
        assert_eq!(p.foreground, Rgba::rgb(255, 255, 255));
        assert_eq!(p.background, Rgba::rgb(0, 0, 0));
        assert_eq!(p.selection_background_transparent, DEFAULT_SELECTION);
        assert_eq!(p.ansi[1], Rgba::rgb(0xcc, 0, 0));
        assert_eq!(p.ansi[16], Rgba::rgb(0, 0, 0));
        assert_eq!(p.ansi[231], Rgba::rgb(255, 255, 255));
        assert_eq!(p.ansi[255], Rgba::rgb(238, 238, 238));
        assert_eq!(p.scrollbar_slider_background, Rgba::new(255, 255, 255, 51));
    }

    #[test]
    fn opaque_selection_is_made_translucent() {
        let t = Theme {
            background: Some("#40318d".into()),
            selection_background: Some("#7869c4".into()),
            ..Default::default()
        };
        let p = Palette::from_theme(&t);
        assert_eq!(p.selection_background_opaque, Rgba::rgb(0x78, 0x69, 0xc4));
        assert_eq!(
            p.selection_background_transparent,
            Rgba::new(0x78, 0x69, 0xc4, 77)
        );
        assert_eq!(
            p.selection_inactive_background_transparent,
            Rgba::new(0x78, 0x69, 0xc4, 77)
        );
    }

    #[test]
    fn cursor_is_blended_over_background() {
        let t = Theme {
            background: Some("#000000".into()),
            cursor: Some("#ffffff80".into()),
            ..Default::default()
        };
        assert_eq!(Palette::from_theme(&t).cursor, Rgba::rgb(128, 128, 128));
    }

    #[test]
    fn dark_detection() {
        assert!(Palette::default().is_dark());
        let light = Theme {
            background: Some("#fdf6e3".into()),
            foreground: Some("#657b83".into()),
            ..Default::default()
        };
        assert!(!Palette::from_theme(&light).is_dark());
    }

    #[test]
    fn bundle_palette_json_deserialises() {
        let t: Theme = serde_json::from_str(
            r##"{"background":"#282a36","foreground":"#f8f8f2","brightBlack":"#6272a4"}"##,
        )
        .unwrap();
        assert_eq!(t.bright_black.as_deref(), Some("#6272a4"));
        assert_eq!(Palette::from_theme(&t).ansi[8], Rgba::rgb(0x62, 0x72, 0xa4));
    }
}
