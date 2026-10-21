//! The app theme tokens the terminal reads, from `frontend/src/styles/_tokens.scss`.
//!
//! simplify: a local copy of the handful of tokens the terminal needs, keyed by the app theme id.
//! Replace with `recur_ui::theme` once it lands (same keys and values).

use recur_term::screen::Rgba;

/// The `--*` tokens used by the terminal pane, dock and find bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    pub bg: Rgba,
    pub pane: Rgba,
    pub pane_2: Rgba,
    pub line: Rgba,
    pub line_bright: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub amber: Rgba,
    pub amber_d: Rgba,
    pub cyan: Rgba,
    pub emerald: Rgba,
    pub coral: Rgba,
    pub indigo: Rgba,
    pub warn: Rgba,
}

const fn hex(v: u32) -> Rgba {
    Rgba(v << 8 | 0xff)
}

const fn rgba(r: u8, g: u8, b: u8, a: f64) -> Rgba {
    // CSS alpha to 8 bits, rounded.
    Rgba::new(r, g, b, (a * 255.0 + 0.5) as u8)
}

const CHARCOAL: Tokens = Tokens {
    bg: hex(0x141414),
    pane: hex(0x1c1c1c),
    pane_2: hex(0x262626),
    line: hex(0x2e2e2e),
    line_bright: hex(0x3d3d3d),
    text: hex(0xe6e4e1),
    muted: hex(0x969490),
    faint: hex(0x686662),
    amber: hex(0xe07a5f),
    amber_d: rgba(224, 122, 95, 0.14),
    cyan: hex(0x6eb6ff),
    emerald: hex(0x4ade80),
    coral: hex(0xf87171),
    indigo: hex(0xa78bfa),
    warn: hex(0xfbbf24),
};

/// Tokens for an app theme key (`default`, `paper`, `slate`, `coolglow`, `catppuccin-mocha`,
/// `catppuccin-latte`, `nord`). Unknown keys are Charcoal, as `applyTheme` does.
pub fn tokens(theme: &str) -> Tokens {
    match theme {
        "slate" => Tokens {
            bg: hex(0x0A0C10),
            pane: hex(0x101319),
            pane_2: hex(0x161A21),
            line: hex(0x212732),
            line_bright: hex(0x2C343F),
            text: hex(0xDBE2EA),
            muted: hex(0x7C8896),
            faint: hex(0x4E5864),
            amber: hex(0xF2B33D),
            amber_d: rgba(242, 179, 61, 0.12),
            cyan: hex(0x4CC6D6),
            emerald: hex(0x5DBE7E),
            coral: hex(0xEB7A5C),
            indigo: hex(0x9486F4),
            warn: CHARCOAL.warn,
        },
        "coolglow" => Tokens {
            bg: hex(0x0a0c22),
            pane: hex(0x0e1130),
            pane_2: hex(0x161a3c),
            line: hex(0x1c2348),
            line_bright: hex(0x2a3460),
            text: hex(0xd8dce8),
            muted: hex(0x6e7d9a),
            faint: hex(0x4e5670),
            amber: hex(0x45c4b8),
            amber_d: rgba(69, 196, 184, 0.12),
            cyan: hex(0x6eb8d4),
            emerald: hex(0x6ecf8a),
            coral: hex(0xe87890),
            indigo: hex(0xa78bc4),
            warn: hex(0xd4c98a),
        },
        "catppuccin-mocha" => Tokens {
            bg: hex(0x0e0e16),
            pane: hex(0x181825),
            pane_2: hex(0x262637),
            line: hex(0x313244),
            line_bright: hex(0x45475a),
            text: hex(0xe0e6f8),
            muted: hex(0x7f849c),
            faint: hex(0x585b70),
            amber: hex(0xcba6f7),
            amber_d: rgba(203, 166, 247, 0.14),
            cyan: hex(0x89dceb),
            emerald: hex(0xa6e3a1),
            coral: hex(0xf38ba8),
            indigo: hex(0xb4befe),
            warn: hex(0xf9e2af),
        },
        "catppuccin-latte" => Tokens {
            bg: hex(0xe6e9ef),
            pane: hex(0xeff1f5),
            pane_2: hex(0xdce0e8),
            line: hex(0xccd0da),
            line_bright: hex(0xbcc0cc),
            text: hex(0x4c4f69),
            muted: hex(0x6c6f85),
            faint: hex(0x8c8fa1),
            amber: hex(0x8839ef),
            amber_d: rgba(136, 57, 239, 0.12),
            cyan: hex(0x209fb5),
            emerald: hex(0x40a02b),
            coral: hex(0xd20f39),
            indigo: hex(0x7287fd),
            warn: hex(0xa86a10),
        },
        "paper" => Tokens {
            bg: hex(0xebecef),
            pane: hex(0xf7f8fa),
            pane_2: hex(0xe4e6eb),
            line: hex(0xcfd3db),
            line_bright: hex(0xb4bac6),
            text: hex(0x1a1c20),
            muted: hex(0x5a616c),
            faint: hex(0x8b929e),
            amber: hex(0xd4634a),
            amber_d: rgba(212, 99, 74, 0.12),
            cyan: hex(0x2a7eb5),
            emerald: hex(0x2d9a5a),
            coral: hex(0xd14b4b),
            indigo: hex(0x6b5bd4),
            warn: hex(0xb8860b),
        },
        "nord" => Tokens {
            bg: hex(0x181c24),
            pane: hex(0x222730),
            pane_2: hex(0x2e3440),
            line: hex(0x3b4252),
            line_bright: hex(0x4c566a),
            text: hex(0xeceff4),
            muted: hex(0x7b88a1),
            faint: hex(0x4c566a),
            amber: hex(0x88c0d0),
            amber_d: rgba(136, 192, 208, 0.14),
            cyan: hex(0x8fbcbb),
            emerald: hex(0xa3be8c),
            coral: hex(0xbf616a),
            indigo: hex(0xb48ead),
            warn: hex(0xebcb8b),
        },
        _ => CHARCOAL,
    }
}

/// Light app-chrome palettes (`isLightTheme`).
pub fn is_light_theme(theme: &str) -> bool {
    theme == "paper" || theme == "catppuccin-latte"
}

/// `color-mix(in srgb, a p%, b)` for opaque colours.
pub fn mix(a: Rgba, b: Rgba, p: f64) -> Rgba {
    let ch = |x: u8, y: u8| (x as f64 * p + y as f64 * (1.0 - p) + 0.5).floor() as u8;
    let alpha = (a.a() as f64 * p + b.a() as f64 * (1.0 - p) + 0.5).floor() as u8;
    Rgba::new(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()), alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charcoal_is_the_default() {
        assert_eq!(tokens("default"), tokens("anything"));
        assert_eq!(tokens("default").bg, Rgba::rgb(0x14, 0x14, 0x14));
        assert_eq!(tokens("default").amber_d, Rgba::new(224, 122, 95, 36));
    }

    #[test]
    fn slate_inherits_warn() {
        assert_eq!(tokens("slate").warn, Rgba::rgb(0xfb, 0xbf, 0x24));
    }

    #[test]
    fn light_themes() {
        assert!(is_light_theme("paper") && is_light_theme("catppuccin-latte"));
        assert!(!is_light_theme("default") && !is_light_theme("nord"));
    }

    #[test]
    fn color_mix() {
        let t = tokens("default");
        assert_eq!(mix(t.amber, t.line, 0.55), Rgba::rgb(144, 88, 73));
    }
}
