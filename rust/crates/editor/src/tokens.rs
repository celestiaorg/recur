//! App chrome design tokens the editor draws with (`styles/_tokens.scss`):
//! panes, lines, text tiers, accents and git colors per app theme.
//!
//! simplify: `recur-ui` does not carry these yet, so they live here behind
//! [`AppTokens`]; swap the constructor for `recur-ui`'s once it owns the
//! palette.

use gpui_kit::Rgba;

use crate::color::{mix_srgb, parse_css_color, rgba_f};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppTokens {
    pub light: bool,
    pub bg: Rgba,
    pub pane: Rgba,
    pub pane_2: Rgba,
    pub line: Rgba,
    pub line_soft: Rgba,
    pub line_bright: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub amber: Rgba,
    pub amber_d: Rgba,
    pub cyan: Rgba,
    pub emerald: Rgba,
    pub coral: Rgba,
    pub coral_d: Rgba,
    pub indigo: Rgba,
    pub warn: Rgba,
    pub git_m: Rgba,
    pub git_a: Rgba,
    pub git_d: Rgba,
    pub git_u: Rgba,
    pub scrollbar_thumb: Rgba,
    pub scrollbar_thumb_hover: Rgba,
}

/// `--scrollbar-size`.
pub const SCROLLBAR_SIZE: f32 = 11.0;

fn c(s: &str) -> Rgba {
    parse_css_color(s).unwrap_or(rgba_f(1.0, 0.0, 1.0, 1.0))
}

/// The Charcoal defaults every theme starts from (`:root`).
fn charcoal() -> AppTokens {
    AppTokens {
        light: false,
        bg: c("#141414"),
        pane: c("#1c1c1c"),
        pane_2: c("#262626"),
        line: c("#2e2e2e"),
        line_soft: c("#222222"),
        line_bright: c("#3d3d3d"),
        text: c("#e6e4e1"),
        muted: c("#969490"),
        faint: c("#686662"),
        amber: c("#e07a5f"),
        amber_d: c("rgba(224, 122, 95, .14)"),
        cyan: c("#6eb6ff"),
        emerald: c("#4ade80"),
        coral: c("#f87171"),
        coral_d: c("rgba(248, 113, 113, .14)"),
        indigo: c("#a78bfa"),
        warn: c("#fbbf24"),
        git_m: c("#e6c84a"),
        git_a: c("#5dbe7e"),
        git_d: c("#f07178"),
        git_u: c("#5dbe7e"),
        scrollbar_thumb: c("#3a3a3a"),
        scrollbar_thumb_hover: c("#4d4d4d"),
    }
}

impl AppTokens {
    /// Tokens for an app chrome theme key (`stores` `theme`); unknown keys
    /// are Charcoal.
    pub fn for_theme(key: &str) -> AppTokens {
        let base = charcoal();
        match key {
            "slate" => AppTokens {
                scrollbar_thumb: c("#2C343F"),
                scrollbar_thumb_hover: c("#3A4452"),
                bg: c("#0A0C10"),
                pane: c("#101319"),
                pane_2: c("#161A21"),
                line: c("#212732"),
                line_soft: c("#191E26"),
                line_bright: c("#2C343F"),
                text: c("#DBE2EA"),
                muted: c("#7C8896"),
                faint: c("#4E5864"),
                amber: c("#F2B33D"),
                amber_d: c("rgba(242, 179, 61, .12)"),
                cyan: c("#4CC6D6"),
                emerald: c("#5DBE7E"),
                coral: c("#EB7A5C"),
                coral_d: c("rgba(235, 122, 92, .15)"),
                indigo: c("#9486F4"),
                ..base
            },
            "coolglow" => AppTokens {
                scrollbar_thumb: c("#2a3460"),
                scrollbar_thumb_hover: c("#3a4578"),
                bg: c("#0a0c22"),
                pane: c("#0e1130"),
                pane_2: c("#161a3c"),
                line: c("#1c2348"),
                line_soft: c("#141836"),
                line_bright: c("#2a3460"),
                text: c("#d8dce8"),
                muted: c("#6e7d9a"),
                faint: c("#4e5670"),
                amber: c("#45c4b8"),
                amber_d: c("rgba(69, 196, 184, .12)"),
                cyan: c("#6eb8d4"),
                emerald: c("#6ecf8a"),
                coral: c("#e87890"),
                coral_d: c("rgba(232, 120, 144, .18)"),
                indigo: c("#a78bc4"),
                warn: c("#d4c98a"),
                ..base
            },
            "catppuccin-mocha" => AppTokens {
                scrollbar_thumb: c("#45475a"),
                scrollbar_thumb_hover: c("#585b70"),
                bg: c("#0e0e16"),
                pane: c("#181825"),
                pane_2: c("#262637"),
                line: c("#313244"),
                line_soft: c("#1e1e2e"),
                line_bright: c("#45475a"),
                text: c("#e0e6f8"),
                muted: c("#7f849c"),
                faint: c("#585b70"),
                amber: c("#cba6f7"),
                amber_d: c("rgba(203, 166, 247, .14)"),
                cyan: c("#89dceb"),
                emerald: c("#a6e3a1"),
                coral: c("#f38ba8"),
                coral_d: c("rgba(243, 139, 168, .16)"),
                indigo: c("#b4befe"),
                warn: c("#f9e2af"),
                ..base
            },
            "catppuccin-latte" => AppTokens {
                scrollbar_thumb: c("#bcc0cc"),
                scrollbar_thumb_hover: c("#acb0be"),
                light: true,
                bg: c("#e6e9ef"),
                pane: c("#eff1f5"),
                pane_2: c("#dce0e8"),
                line: c("#ccd0da"),
                line_soft: c("#dce0e8"),
                line_bright: c("#bcc0cc"),
                text: c("#4c4f69"),
                muted: c("#6c6f85"),
                faint: c("#8c8fa1"),
                amber: c("#8839ef"),
                amber_d: c("rgba(136, 57, 239, .12)"),
                cyan: c("#209fb5"),
                emerald: c("#40a02b"),
                coral: c("#d20f39"),
                coral_d: c("rgba(210, 15, 57, .11)"),
                indigo: c("#7287fd"),
                warn: c("#a86a10"),
                git_m: c("#a86a10"),
                git_a: c("#40a02b"),
                git_d: c("#d20f39"),
                git_u: c("#40a02b"),
            },
            "paper" => AppTokens {
                scrollbar_thumb: c("#c5cad3"),
                scrollbar_thumb_hover: c("#aeb5c0"),
                light: true,
                bg: c("#ebecef"),
                pane: c("#f7f8fa"),
                pane_2: c("#e4e6eb"),
                line: c("#cfd3db"),
                line_soft: c("#dde0e6"),
                line_bright: c("#b4bac6"),
                text: c("#1a1c20"),
                muted: c("#5a616c"),
                faint: c("#8b929e"),
                amber: c("#d4634a"),
                amber_d: c("rgba(212, 99, 74, .12)"),
                cyan: c("#2a7eb5"),
                emerald: c("#2d9a5a"),
                coral: c("#d14b4b"),
                coral_d: c("rgba(209, 75, 75, .11)"),
                indigo: c("#6b5bd4"),
                warn: c("#b8860b"),
                git_m: c("#a68512"),
                git_a: c("#2d9a5a"),
                git_d: c("#d14b4b"),
                git_u: c("#2d9a5a"),
            },
            "nord" => AppTokens {
                scrollbar_thumb: c("#4c566a"),
                scrollbar_thumb_hover: c("#5e6a80"),
                bg: c("#181c24"),
                pane: c("#222730"),
                pane_2: c("#2e3440"),
                line: c("#3b4252"),
                line_soft: c("#2a303a"),
                line_bright: c("#4c566a"),
                text: c("#eceff4"),
                muted: c("#7b88a1"),
                faint: c("#4c566a"),
                amber: c("#88c0d0"),
                amber_d: c("rgba(136, 192, 208, .14)"),
                cyan: c("#8fbcbb"),
                emerald: c("#a3be8c"),
                coral: c("#bf616a"),
                coral_d: c("rgba(191, 97, 106, .18)"),
                indigo: c("#b48ead"),
                warn: c("#ebcb8b"),
                ..base
            },
            _ => base,
        }
    }

    /// `color-mix(in srgb, <c> p%, transparent)`.
    pub fn wash(c: Rgba, pct: f32) -> Rgba {
        mix_srgb(c, pct, crate::color::TRANSPARENT, 1.0 - pct)
    }

    /// `color-mix(in srgb, a p%, b)`.
    pub fn mix(a: Rgba, pct: f32, b: Rgba) -> Rgba {
        mix_srgb(a, pct, b, 1.0 - pct)
    }
}

/// `--mono`: 'JetBrains Mono', ui-monospace, 'SF Mono', Menlo, monospace.
pub const MONO_FAMILIES: [&str; 4] = ["JetBrains Mono", "SF Mono", "Menlo", "monospace"];
/// `--ui`: 'Inter', system-ui, sans-serif.
pub const UI_FAMILIES: [&str; 2] = ["Inter", ".SystemUIFont"];

fn font_of(families: &[&str]) -> gpui_kit::Font {
    let mut f = gpui_kit::font(families[0].to_string());
    f.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(
        families[1..].iter().map(|s| s.to_string()).collect(),
    ));
    f
}

/// CSS `monospace` as WebKit resolves it (the default fixed font).
pub const GENERIC_MONO_FAMILIES: [&str; 2] = if cfg!(target_os = "macos") {
    ["Courier", "monospace"]
} else {
    ["DejaVu Sans Mono", "monospace"]
};

/// The generic `monospace` font.
pub fn generic_mono_font() -> gpui_kit::Font {
    font_of(&GENERIC_MONO_FAMILIES)
}

/// The `--mono` font.
pub fn mono_font() -> gpui_kit::Font {
    font_of(&MONO_FAMILIES)
}

/// The `--ui` font.
pub fn ui_font() -> gpui_kit::Font {
    font_of(&UI_FAMILIES)
}

/// Set an element's text font family and fallbacks.
pub fn with_font<E: gpui_kit::Styled>(mut e: E, font: &gpui_kit::Font) -> E {
    let text = &mut e.style().text;
    text.font_family = Some(font.family.clone());
    text.font_fallbacks = font.fallbacks.clone();
    e
}
