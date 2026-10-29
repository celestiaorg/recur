//! App chrome themes: every token and palette, the picker list, aliases and "Sync with
//! system" rules.
//!
//! Themes are color only; identity and layout stay constant. A palette that does not set a
//! token inherits Charcoal's `:root` value, exactly like the CSS cascade.

use crate::color::{Color, hex, rgba};
use gpui_kit::{App, Global, Hsla, Pixels, px};
use serde::{Deserialize, Serialize};

/// The chrome themes, in picker order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ThemeId {
    #[default]
    Charcoal,
    Paper,
    Slate,
    CoolGlow,
    CatppuccinMocha,
    CatppuccinLatte,
    Nord,
}

impl ThemeId {
    pub const ALL: [ThemeId; 7] = [
        ThemeId::Charcoal,
        ThemeId::Paper,
        ThemeId::Slate,
        ThemeId::CoolGlow,
        ThemeId::CatppuccinMocha,
        ThemeId::CatppuccinLatte,
        ThemeId::Nord,
    ];

    /// Persisted key. Charcoal is `default`: no `data-theme` attribute.
    pub fn key(self) -> &'static str {
        match self {
            ThemeId::Charcoal => "default",
            ThemeId::Paper => "paper",
            ThemeId::Slate => "slate",
            ThemeId::CoolGlow => "coolglow",
            ThemeId::CatppuccinMocha => "catppuccin-mocha",
            ThemeId::CatppuccinLatte => "catppuccin-latte",
            ThemeId::Nord => "nord",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeId::Charcoal => "Charcoal",
            ThemeId::Paper => "Paper",
            ThemeId::Slate => "Slate",
            ThemeId::CoolGlow => "Cool Glow",
            ThemeId::CatppuccinMocha => "Catppuccin Mocha",
            ThemeId::CatppuccinLatte => "Catppuccin Latte",
            ThemeId::Nord => "Nord",
        }
    }

    /// The short descriptor shown on the Settings theme card (`d`).
    pub fn descriptor(self) -> &'static str {
        match self {
            ThemeId::Charcoal => "default",
            ThemeId::Paper => "light",
            ThemeId::Slate => "cool",
            ThemeId::CoolGlow => "navy",
            ThemeId::CatppuccinMocha => "mocha",
            ThemeId::CatppuccinLatte => "latte",
            ThemeId::Nord => "frost",
        }
    }

    /// `isLightTheme`: light chrome changes mermaid, file-icon colors and terminal selection.
    pub fn is_light(self) -> bool {
        matches!(self, ThemeId::Paper | ThemeId::CatppuccinLatte)
    }

    /// `applyTheme`: legacy keys map through `THEME_ALIASES`; unknown keys fall back to
    /// Charcoal.
    pub fn from_key(key: &str) -> ThemeId {
        let resolved = match key {
            "gruvbox" | "conductor" | "charcoal" => "default",
            "tokyonight" => "coolglow",
            "catppuccin" => "catppuccin-mocha",
            other => other,
        };
        ThemeId::ALL
            .into_iter()
            .find(|t| t.key() == resolved)
            .unwrap_or_default()
    }

    /// Settings picker swatches (`bg`, `pane`, `text`, `a`, `ok`, `warn`).
    pub fn swatches(self) -> Swatches {
        let (bg, pane, text, a, ok, warn) = match self {
            ThemeId::Charcoal => (0x141414, 0x1c1c1c, 0xe6e4e1, 0xe07a5f, 0x4ade80, 0xf87171),
            ThemeId::Paper => (0xebecef, 0xf7f8fa, 0x1a1c20, 0xd4634a, 0x2d9a5a, 0xd14b4b),
            ThemeId::Slate => (0x0a0c10, 0x161a21, 0xdbe2ea, 0xf2b33d, 0x5dbe7e, 0xeb7a5c),
            ThemeId::CoolGlow => (0x0a0c22, 0x0e1130, 0xd8dce8, 0x45c4b8, 0x6ecf8a, 0xe87890),
            ThemeId::CatppuccinMocha => {
                (0x0e0e16, 0x181825, 0xe0e6f8, 0xcba6f7, 0xa6e3a1, 0xf38ba8)
            }
            ThemeId::CatppuccinLatte => {
                (0xe6e9ef, 0xeff1f5, 0x4c4f69, 0x8839ef, 0x40a02b, 0xd20f39)
            }
            ThemeId::Nord => (0x181c24, 0x222730, 0xeceff4, 0x88c0d0, 0xa3be8c, 0xbf616a),
        };
        Swatches {
            bg: hex(bg),
            pane: hex(pane),
            text: hex(text),
            accent: hex(a),
            ok: hex(ok),
            warn: hex(warn),
        }
    }

    pub fn tokens(self) -> Tokens {
        let mut t = Tokens::charcoal();
        match self {
            ThemeId::Charcoal => {}
            ThemeId::Slate => {
                t.bg = hex(0x0a0c10);
                t.pane = hex(0x101319);
                t.pane_2 = hex(0x161a21);
                t.rail = hex(0x0c0e13);
                t.titlebar = hex(0x11141a);
                t.statusbar = hex(0x0e1116);
                t.line = hex(0x212732);
                t.line_soft = hex(0x191e26);
                t.line_bright = hex(0x2c343f);
                t.text = hex(0xdbe2ea);
                t.muted = hex(0x7c8896);
                t.faint = hex(0x4e5864);
                t.amber = hex(0xf2b33d);
                t.amber_d = rgba(242, 179, 61, 0.12);
                t.cyan = hex(0x4cc6d6);
                t.emerald = hex(0x5dbe7e);
                t.coral = hex(0xeb7a5c);
                t.coral_d = rgba(235, 122, 92, 0.15);
                t.indigo = hex(0x9486f4);
                t.scrollbar_thumb = hex(0x2c343f);
                t.scrollbar_thumb_hover = hex(0x3a4452);
            }
            ThemeId::CoolGlow => {
                t.bg = hex(0x0a0c22);
                t.pane = hex(0x0e1130);
                t.pane_2 = hex(0x161a3c);
                t.rail = hex(0x0c0e28);
                t.titlebar = hex(0x0c0e28);
                t.statusbar = hex(0x080a1c);
                t.line = hex(0x1c2348);
                t.line_soft = hex(0x141836);
                t.line_bright = hex(0x2a3460);
                t.text = hex(0xd8dce8);
                t.muted = hex(0x6e7d9a);
                t.faint = hex(0x4e5670);
                t.amber = hex(0x45c4b8);
                t.amber_d = rgba(69, 196, 184, 0.12);
                t.cyan = hex(0x6eb8d4);
                t.emerald = hex(0x6ecf8a);
                t.coral = hex(0xe87890);
                t.coral_d = rgba(232, 120, 144, 0.18);
                t.indigo = hex(0xa78bc4);
                t.warn = hex(0xd4c98a);
                t.scrollbar_thumb = hex(0x2a3460);
                t.scrollbar_thumb_hover = hex(0x3a4578);
            }
            ThemeId::CatppuccinMocha => {
                t.bg = hex(0x0e0e16);
                t.pane = hex(0x181825);
                t.pane_2 = hex(0x262637);
                t.rail = hex(0x13131d);
                t.titlebar = hex(0x13131d);
                t.statusbar = hex(0x0c0c14);
                t.line = hex(0x313244);
                t.line_soft = hex(0x1e1e2e);
                t.line_bright = hex(0x45475a);
                t.text = hex(0xe0e6f8);
                t.muted = hex(0x7f849c);
                t.faint = hex(0x585b70);
                t.amber = hex(0xcba6f7);
                t.amber_d = rgba(203, 166, 247, 0.14);
                t.cyan = hex(0x89dceb);
                t.emerald = hex(0xa6e3a1);
                t.coral = hex(0xf38ba8);
                t.coral_d = rgba(243, 139, 168, 0.16);
                t.indigo = hex(0xb4befe);
                t.warn = hex(0xf9e2af);
                t.scrollbar_thumb = hex(0x45475a);
                t.scrollbar_thumb_hover = hex(0x585b70);
            }
            ThemeId::CatppuccinLatte => {
                t.bg = hex(0xe6e9ef);
                t.pane = hex(0xeff1f5);
                t.pane_2 = hex(0xdce0e8);
                t.rail = hex(0xe6e9ef);
                t.titlebar = hex(0xe6e9ef);
                t.statusbar = hex(0xdce0e8);
                t.line = hex(0xccd0da);
                t.line_soft = hex(0xdce0e8);
                t.line_bright = hex(0xbcc0cc);
                t.text = hex(0x4c4f69);
                t.muted = hex(0x6c6f85);
                t.faint = hex(0x8c8fa1);
                t.amber = hex(0x8839ef);
                t.amber_d = rgba(136, 57, 239, 0.12);
                t.on_amber = hex(0xeff1f5);
                t.cyan = hex(0x209fb5);
                t.emerald = hex(0x40a02b);
                t.coral = hex(0xd20f39);
                t.coral_d = rgba(210, 15, 57, 0.11);
                t.indigo = hex(0x7287fd);
                t.warn = hex(0xa86a10);
                t.git_m = hex(0xa86a10);
                t.git_a = hex(0x40a02b);
                t.git_d = hex(0xd20f39);
                t.git_u = hex(0x40a02b);
                t.scrollbar_thumb = hex(0xbcc0cc);
                t.scrollbar_thumb_hover = hex(0xacb0be);
            }
            ThemeId::Paper => {
                t.bg = hex(0xebecef);
                t.pane = hex(0xf7f8fa);
                t.pane_2 = hex(0xe4e6eb);
                t.rail = hex(0xf0f1f4);
                t.titlebar = hex(0xf0f1f4);
                t.statusbar = hex(0xe8e9ed);
                t.line = hex(0xcfd3db);
                t.line_soft = hex(0xdde0e6);
                t.line_bright = hex(0xb4bac6);
                t.text = hex(0x1a1c20);
                t.muted = hex(0x5a616c);
                t.faint = hex(0x8b929e);
                t.amber = hex(0xd4634a);
                t.amber_d = rgba(212, 99, 74, 0.12);
                t.cyan = hex(0x2a7eb5);
                t.emerald = hex(0x2d9a5a);
                t.coral = hex(0xd14b4b);
                t.coral_d = rgba(209, 75, 75, 0.11);
                t.indigo = hex(0x6b5bd4);
                t.warn = hex(0xb8860b);
                t.git_m = hex(0xa68512);
                t.git_a = hex(0x2d9a5a);
                t.git_d = hex(0xd14b4b);
                t.git_u = hex(0x2d9a5a);
                t.scrollbar_thumb = hex(0xc5cad3);
                t.scrollbar_thumb_hover = hex(0xaeb5c0);
            }
            ThemeId::Nord => {
                t.bg = hex(0x181c24);
                t.pane = hex(0x222730);
                t.pane_2 = hex(0x2e3440);
                t.rail = hex(0x1c2129);
                t.titlebar = hex(0x1c2129);
                t.statusbar = hex(0x14181f);
                t.line = hex(0x3b4252);
                t.line_soft = hex(0x2a303a);
                t.line_bright = hex(0x4c566a);
                t.text = hex(0xeceff4);
                t.muted = hex(0x7b88a1);
                t.faint = hex(0x4c566a);
                t.amber = hex(0x88c0d0);
                t.amber_d = rgba(136, 192, 208, 0.14);
                t.cyan = hex(0x8fbcbb);
                t.emerald = hex(0xa3be8c);
                t.coral = hex(0xbf616a);
                t.coral_d = rgba(191, 97, 106, 0.18);
                t.indigo = hex(0xb48ead);
                t.warn = hex(0xebcb8b);
                t.scrollbar_thumb = hex(0x4c566a);
                t.scrollbar_thumb_hover = hex(0x5e6a80);
            }
        }
        // --coral-line is declared once on :root as a color-mix of var()s, so every palette
        // recomputes it from its own --coral and --line.
        t.coral_line = t.coral.mix(0.42, t.line);
        t
    }
}

/// Preview swatches for the Settings theme picker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swatches {
    pub bg: Color,
    pub pane: Color,
    pub text: Color,
    pub accent: Color,
    pub ok: Color,
    pub warn: Color,
}

/// Every theme token. Names follow the CSS custom properties (`--pane-2` → `pane_2`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub bg: Color,
    pub pane: Color,
    pub pane_2: Color,
    pub rail: Color,
    pub titlebar: Color,
    pub statusbar: Color,
    pub line: Color,
    pub line_soft: Color,
    pub line_bright: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    /// The accent. Called amber in CSS even where a palette makes it teal or mauve.
    pub amber: Color,
    pub amber_d: Color,
    pub cyan: Color,
    pub emerald: Color,
    pub coral: Color,
    pub coral_d: Color,
    pub indigo: Color,
    pub warn: Color,
    pub coral_line: Color,
    pub git_m: Color,
    pub git_a: Color,
    pub git_d: Color,
    pub git_u: Color,
    pub scrollbar_thumb: Color,
    pub scrollbar_thumb_hover: Color,
    pub on_amber: Color,
}

impl Tokens {
    fn charcoal() -> Tokens {
        let coral = hex(0xf87171);
        let line = hex(0x2e2e2e);
        Tokens {
            bg: hex(0x141414),
            pane: hex(0x1c1c1c),
            pane_2: hex(0x262626),
            rail: hex(0x181818),
            titlebar: hex(0x181818),
            statusbar: hex(0x161616),
            line,
            line_soft: hex(0x222222),
            line_bright: hex(0x3d3d3d),
            text: hex(0xe6e4e1),
            muted: hex(0x969490),
            faint: hex(0x686662),
            amber: hex(0xe07a5f),
            amber_d: rgba(224, 122, 95, 0.14),
            cyan: hex(0x6eb6ff),
            emerald: hex(0x4ade80),
            coral,
            coral_d: rgba(248, 113, 113, 0.14),
            indigo: hex(0xa78bfa),
            warn: hex(0xfbbf24),
            coral_line: coral.mix(0.42, line),
            git_m: hex(0xe6c84a),
            git_a: hex(0x5dbe7e),
            git_d: hex(0xf07178),
            git_u: hex(0x5dbe7e),
            scrollbar_thumb: hex(0x3a3a3a),
            scrollbar_thumb_hover: hex(0x4d4d4d),
            on_amber: hex(0x0a0c10),
        }
    }

    /// Look up a token by its CSS custom property name (`--pane-2` or `pane-2`).
    pub fn by_name(&self, name: &str) -> Option<Color> {
        let n = name.trim().trim_start_matches("--");
        Some(match n {
            "bg" => self.bg,
            "pane" => self.pane,
            "pane-2" => self.pane_2,
            "rail" => self.rail,
            "titlebar" => self.titlebar,
            "statusbar" => self.statusbar,
            "line" => self.line,
            "line-soft" => self.line_soft,
            "line-bright" => self.line_bright,
            "text" => self.text,
            "muted" => self.muted,
            "faint" => self.faint,
            "amber" => self.amber,
            "amber-d" => self.amber_d,
            "cyan" => self.cyan,
            "emerald" => self.emerald,
            "coral" => self.coral,
            "coral-d" => self.coral_d,
            "indigo" => self.indigo,
            "warn" => self.warn,
            "coral-line" => self.coral_line,
            "git-m" => self.git_m,
            "git-a" => self.git_a,
            "git-d" => self.git_d,
            "git-u" => self.git_u,
            "scrollbar-thumb" => self.scrollbar_thumb,
            "scrollbar-thumb-hover" => self.scrollbar_thumb_hover,
            "on-amber" => self.on_amber,
            _ => return None,
        })
    }

    /// Every color token with its CSS name, in declaration order.
    pub fn named(&self) -> [(&'static str, Color); 28] {
        [
            ("bg", self.bg),
            ("pane", self.pane),
            ("pane-2", self.pane_2),
            ("rail", self.rail),
            ("titlebar", self.titlebar),
            ("statusbar", self.statusbar),
            ("line", self.line),
            ("line-soft", self.line_soft),
            ("line-bright", self.line_bright),
            ("text", self.text),
            ("muted", self.muted),
            ("faint", self.faint),
            ("amber", self.amber),
            ("amber-d", self.amber_d),
            ("cyan", self.cyan),
            ("emerald", self.emerald),
            ("coral", self.coral),
            ("coral-d", self.coral_d),
            ("indigo", self.indigo),
            ("warn", self.warn),
            ("coral-line", self.coral_line),
            ("git-m", self.git_m),
            ("git-a", self.git_a),
            ("git-d", self.git_d),
            ("git-u", self.git_u),
            ("scrollbar-thumb", self.scrollbar_thumb),
            ("scrollbar-thumb-hover", self.scrollbar_thumb_hover),
            ("on-amber", self.on_amber),
        ]
    }
}

/// `--scrollbar-size`.
pub const SCROLLBAR_SIZE: Pixels = px(11.);

/// The active chrome theme. A GPUI global: read with [`Theme::of`], switch with
/// [`Theme::set`].
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub id: ThemeId,
    pub tokens: Tokens,
}

impl Global for Theme {}

impl Theme {
    pub fn new(id: ThemeId) -> Theme {
        Theme {
            id,
            tokens: id.tokens(),
        }
    }

    pub fn of(cx: &App) -> &Theme {
        cx.global::<Theme>()
    }

    pub fn is_light(&self) -> bool {
        self.id.is_light()
    }

    /// Switch the chrome theme at runtime and repaint every window.
    pub fn set(id: ThemeId, cx: &mut App) {
        if cx.has_global::<Theme>() && Theme::of(cx).id == id {
            return;
        }
        cx.set_global(Theme::new(id));
        crate::kit::sync_component_theme(cx);
        cx.refresh_windows();
    }

    /// Resolve an inline CSS color: `var(--token)`, hex, `rgb()`.
    pub fn resolve_css_color(&self, css: &str) -> Option<Color> {
        let t = css.trim();
        if let Some(inner) = t.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
            let (name, fallback) = match inner.split_once(',') {
                Some((n, f)) => (n, Some(f)),
                None => (inner, None),
            };
            return self
                .tokens
                .by_name(name)
                .or_else(|| fallback.and_then(|f| self.resolve_css_color(f)));
        }
        Color::parse(t)
    }

    /// GPUI color for a token, for call sites that take `Hsla` directly.
    pub fn hsla(&self, c: Color) -> Hsla {
        c.into()
    }
}

/// `prefers-color-scheme` answer plus the light/dark picks of one kind of theme. The field
/// names are the persisted `recur.<name>Auto|Light|Dark` suffixes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSync {
    pub auto: bool,
    pub light: String,
    pub dark: String,
}

impl SystemSync {
    /// App chrome defaults: Paper by day, Charcoal by night.
    pub fn app_default() -> SystemSync {
        SystemSync {
            auto: false,
            light: ThemeId::Paper.key().into(),
            dark: ThemeId::Charcoal.key().into(),
        }
    }

    /// The pick to apply for the current system appearance, or `None` while sync is off.
    pub fn pick(&self, system_dark: bool) -> Option<&str> {
        let pick = if system_dark { &self.dark } else { &self.light };
        self.auto.then_some(pick.as_str())
    }
}

/// Chrome theme selection: an explicit theme key, and a
/// system sync that overrides it while on. Persist this as one JSON file under the data dir.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    /// `recur.theme`: the last applied (alias-resolved) theme key.
    pub theme: String,
    pub sync: SystemSync,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: ThemeId::Charcoal.key().into(),
            sync: SystemSync::app_default(),
        }
    }
}

impl Appearance {
    /// The theme to paint for this system appearance.
    pub fn resolve(&self, system_dark: bool) -> ThemeId {
        ThemeId::from_key(self.sync.pick(system_dark).unwrap_or(&self.theme))
    }

    /// `setTheme`: an explicit pick applies and turns system-follow off.
    pub fn set_theme(&mut self, key: &str) -> ThemeId {
        let id = ThemeId::from_key(key);
        self.theme = id.key().into();
        self.sync.auto = false;
        id
    }

    /// `setThemeAuto`. Applying the sync pick also records it as the current theme, like
    /// `applyTheme` does.
    pub fn set_auto(&mut self, on: bool, system_dark: bool) -> ThemeId {
        self.sync.auto = on;
        self.reapply(system_dark)
    }

    /// `setSystemTheme(mode, key)`.
    pub fn set_pick(&mut self, dark: bool, key: &str, system_dark: bool) -> ThemeId {
        if dark {
            self.sync.dark = key.into();
        } else {
            self.sync.light = key.into();
        }
        self.reapply(system_dark)
    }

    /// Re-run the sync after the system appearance changed (`system.theme`).
    pub fn reapply(&mut self, system_dark: bool) -> ThemeId {
        let id = self.resolve(system_dark);
        if self.sync.auto {
            self.theme = id.key().into();
        }
        id
    }

    /// Apply to the running app.
    pub fn apply(&self, system_dark: bool, cx: &mut App) {
        Theme::set(self.resolve(system_dark), cx);
    }
}

pub(crate) fn init(cx: &mut App) {
    if !cx.has_global::<Theme>() {
        cx.set_global(Theme::new(ThemeId::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `recur-terminal-view`'s local token copy asserts, so it can swap to these.
    #[test]
    fn terminal_token_values() {
        let t = |k: &str| ThemeId::from_key(k).tokens();
        assert_eq!(t("anything"), t("default"));
        assert_eq!(t("default").bg.to_rgba8(), [0x14, 0x14, 0x14, 255]);
        assert_eq!(t("default").amber_d.to_rgba8(), [224, 122, 95, 36]);
        assert_eq!(t("slate").warn.to_rgba8(), [0xfb, 0xbf, 0x24, 255]);
        assert!(ThemeId::from_key("paper").is_light());
        assert!(ThemeId::from_key("catppuccin-latte").is_light());
        assert!(!ThemeId::from_key("nord").is_light());
        let d = t("default");
        assert_eq!(d.amber.mix(0.55, d.line).to_rgba8(), [144, 88, 73, 255]);
    }

    #[test]
    fn keys_and_aliases_round_trip() {
        for id in ThemeId::ALL {
            assert_eq!(ThemeId::from_key(id.key()), id);
        }
        assert_eq!(ThemeId::from_key("gruvbox"), ThemeId::Charcoal);
        assert_eq!(ThemeId::from_key("charcoal"), ThemeId::Charcoal);
        assert_eq!(ThemeId::from_key("tokyonight"), ThemeId::CoolGlow);
        assert_eq!(ThemeId::from_key("catppuccin"), ThemeId::CatppuccinMocha);
        assert_eq!(ThemeId::from_key("everforest"), ThemeId::Charcoal);
        assert!(ThemeId::Paper.is_light() && ThemeId::CatppuccinLatte.is_light());
        assert!(!ThemeId::Nord.is_light());
    }

    #[test]
    fn palettes_inherit_unset_tokens_from_charcoal() {
        let charcoal = ThemeId::Charcoal.tokens();
        let slate = ThemeId::Slate.tokens();
        assert_eq!(slate.warn, charcoal.warn);
        assert_eq!(slate.on_amber, charcoal.on_amber);
        assert_eq!(slate.git_m, charcoal.git_m);
        assert_eq!(ThemeId::CatppuccinLatte.tokens().on_amber, hex(0xeff1f5));
        assert_eq!(ThemeId::Paper.tokens().on_amber, charcoal.on_amber);
        assert_eq!(slate.coral_line, slate.coral.mix(0.42, slate.line));
        assert_ne!(slate.coral_line, charcoal.coral_line);
    }

    #[test]
    fn swatches_match_tokens() {
        for id in ThemeId::ALL {
            let (s, t) = (id.swatches(), id.tokens());
            assert_eq!(s.bg, t.bg, "{id:?} bg");
            assert_eq!(s.text, t.text, "{id:?} text");
            assert_eq!(s.accent, t.amber, "{id:?} accent");
            assert_eq!(s.ok, t.emerald, "{id:?} ok");
            assert_eq!(s.warn, t.coral, "{id:?} warn");
        }
        // Slate's card shows --pane-2 as its pane swatch; the others show --pane.
        assert_eq!(
            ThemeId::Slate.swatches().pane,
            ThemeId::Slate.tokens().pane_2
        );
        assert_eq!(ThemeId::Nord.swatches().pane, ThemeId::Nord.tokens().pane);
    }

    #[test]
    fn system_sync_follows_stores_ts() {
        let mut a = Appearance::default();
        assert_eq!(a.resolve(true), ThemeId::Charcoal);
        assert_eq!(a.set_auto(true, false), ThemeId::Paper);
        assert_eq!(a.theme, "paper");
        assert_eq!(a.reapply(true), ThemeId::Charcoal);
        assert_eq!(a.set_pick(true, "nord", true), ThemeId::Nord);
        assert_eq!(a.set_theme("tokyonight"), ThemeId::CoolGlow);
        assert!(!a.sync.auto);
        assert_eq!(a.resolve(false), ThemeId::CoolGlow);
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Appearance>(&json).unwrap(), a);
    }

    #[test]
    fn resolves_inline_css_colors() {
        let t = Theme::new(ThemeId::Nord);
        assert_eq!(t.resolve_css_color("var(--cyan)"), Some(hex(0x8fbcbb)));
        assert_eq!(
            t.resolve_css_color("var(--nope, #fff)"),
            Some(hex(0xffffff))
        );
        assert_eq!(t.resolve_css_color("#b08bd8"), Some(hex(0xb08bd8)));
    }
}
