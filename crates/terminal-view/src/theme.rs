//! Terminal palettes: the app-chrome mapping and bundle `[[terminalTheme]]` palettes, and the
//! find highlight colours.

use recur_term::screen::{Decorations, Rgba, Theme};
use recur_ui::{ThemeId, Tokens};
use serde::{Deserialize, Serialize};

/// A bundle-contributed terminal palette (`TerminalThemeInfo` from `ListTerminalThemes`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalThemeInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    /// `light` or `dark`.
    pub ui_theme: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub order: i64,
    pub palette: Theme,
    pub bundle_name: String,
    pub source_root: String,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

/// The terminal theme preference (`""` follows the app chrome) and the bundle catalog. The app
/// theme comes from `recur_ui::Theme`, so palettes follow runtime theme switches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeContext {
    pub terminal_theme: String,
    pub catalog: Vec<TerminalThemeInfo>,
}

impl ThemeContext {
    /// `paletteFor`: the catalog palette for `id` with empty values dropped, if it has a
    /// background and foreground.
    fn palette_for(&self, id: &str) -> Option<Theme> {
        if id.is_empty() {
            return None;
        }
        let mut t = self.catalog.iter().find(|x| x.id == id)?.palette.clone();
        for v in theme_fields(&mut t) {
            if v.as_deref() == Some("") {
                *v = None;
            }
        }
        (t.background.is_some() && t.foreground.is_some()).then_some(t)
    }

    /// `resolveTerminalTheme(id, { transparentBg })` under the app theme `app`.
    pub fn resolve(&self, app: ThemeId, transparent_bg: bool) -> Theme {
        terminal_theme(
            &app.tokens(),
            app.is_light(),
            self.palette_for(&self.terminal_theme).as_ref(),
            transparent_bg,
        )
    }

    /// `terminalSurfaceBg`: the opaque cell fill for chrome around the grid.
    pub fn surface_bg(&self, app: ThemeId) -> Rgba {
        self.resolve(app, false)
            .background
            .as_deref()
            .and_then(Rgba::parse)
            .unwrap_or_else(|| rgba(app.tokens().bg))
    }

    /// `terminalFrosts`: maximize frosts (transparent cells) only on dark palettes.
    pub fn frosts(&self, app: ThemeId) -> bool {
        if self.terminal_theme.is_empty() {
            return !app.is_light();
        }
        self.catalog
            .iter()
            .find(|t| t.id == self.terminal_theme)
            .is_none_or(|t| t.ui_theme != "light")
    }
}

fn theme_fields(t: &mut Theme) -> [&mut Option<String>; 26] {
    [
        &mut t.background,
        &mut t.foreground,
        &mut t.cursor,
        &mut t.cursor_accent,
        &mut t.selection_background,
        &mut t.selection_foreground,
        &mut t.selection_inactive_background,
        &mut t.scrollbar_slider_background,
        &mut t.scrollbar_slider_hover_background,
        &mut t.scrollbar_slider_active_background,
        &mut t.black,
        &mut t.red,
        &mut t.green,
        &mut t.yellow,
        &mut t.blue,
        &mut t.magenta,
        &mut t.cyan,
        &mut t.white,
        &mut t.bright_black,
        &mut t.bright_red,
        &mut t.bright_green,
        &mut t.bright_yellow,
        &mut t.bright_blue,
        &mut t.bright_magenta,
        &mut t.bright_cyan,
        &mut t.bright_white,
    ]
}

/// `terminalTheme`: a bundle palette when given, otherwise the app chrome's CSS variables mapped
/// onto xterm's palette so the terminal recolours with the app.
pub fn terminal_theme(
    t: &Tokens,
    light: bool,
    palette: Option<&Theme>,
    transparent_bg: bool,
) -> Theme {
    let h = |c: recur_ui::Color| Some(c.to_hex());
    if let Some(p) = palette {
        let opaque = p.background.clone().or_else(|| h(t.bg));
        let mut out = p.clone();
        out.background = if transparent_bg {
            Some("#00000000".into())
        } else {
            p.background.clone().or_else(|| opaque.clone())
        };
        out.cursor_accent = p.cursor_accent.clone().or(opaque);
        return out;
    }
    // --bg, not --pane: the dock body is the editor-level surface, and the grid paints its own
    // cell background, so a mismatch shows as a seam.
    let bg = if transparent_bg {
        Some("#00000000".into())
    } else {
        h(t.bg)
    };
    Theme {
        background: bg,
        foreground: h(t.text),
        cursor: h(t.amber),
        cursor_accent: h(t.bg),
        // A tinted block rather than a solid fill, so selected text stays readable.
        selection_background: Some(
            if light {
                "rgba(0,0,0,.12)"
            } else {
                "rgba(255,255,255,.18)"
            }
            .into(),
        ),
        black: h(t.line_bright),
        red: h(t.coral),
        green: h(t.emerald),
        yellow: h(t.warn),
        blue: h(t.cyan),
        magenta: h(t.indigo),
        cyan: h(t.cyan),
        white: h(t.muted),
        bright_black: h(t.faint),
        bright_red: h(t.coral),
        bright_green: h(t.emerald),
        bright_yellow: h(t.warn),
        bright_blue: h(t.cyan),
        bright_magenta: h(t.indigo),
        bright_cyan: h(t.cyan),
        bright_white: h(t.text),
        ..Theme::default()
    }
}

/// A token as the terminal's colour type (`#rrggbb`, as `getComputedStyle` reports it).
fn rgba(c: recur_ui::Color) -> Rgba {
    let [r, g, b, a] = c.to_rgba8();
    Rgba::new(r, g, b, a)
}

/// The find highlight colours (`searchDecorations`): matches at `mixHex(--amber, --bg, .4)`,
/// the active one solid `--amber`.
pub fn search_decorations(t: &Tokens) -> Decorations {
    let mix = |fg: u8, bg: u8| (fg as f64 * 0.4 + bg as f64 * 0.6).round() as u8;
    let (a, b) = (rgba(t.amber), rgba(t.bg));
    Decorations {
        match_background: Rgba::rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b())),
        active_match_background: a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: ThemeId = ThemeId::Charcoal;

    fn bundled(id: &str, palette: Theme, ui: &str) -> TerminalThemeInfo {
        TerminalThemeInfo {
            id: id.into(),
            name: id.into(),
            path: format!("themes/{id}.json"),
            ui_theme: ui.into(),
            palette,
            bundle_name: "Terminal".into(),
            source_root: "default".into(),
            ..Default::default()
        }
    }

    fn theme(bg: &str, fg: &str) -> Theme {
        Theme {
            background: Some(bg.into()),
            foreground: Some(fg.into()),
            ..Default::default()
        }
    }

    fn ctx(id: &str, catalog: Vec<TerminalThemeInfo>) -> ThemeContext {
        ThemeContext {
            terminal_theme: id.into(),
            catalog,
        }
    }

    #[test]
    fn empty_id_uses_app_chrome_fallbacks() {
        let t = ctx("", vec![]).resolve(APP, false);
        assert_eq!(t.background.as_deref(), Some("#141414"));
        assert_eq!(t.foreground.as_deref(), Some("#e6e4e1"));
        assert_eq!(t.cursor.as_deref(), Some("#e07a5f"));
        assert_eq!(t.black.as_deref(), Some("#3d3d3d"));
        assert_eq!(t.yellow.as_deref(), Some("#fbbf24"));
    }

    #[test]
    fn app_chrome_follows_the_app_theme() {
        let t = ctx("", vec![]).resolve(ThemeId::Nord, false);
        assert_eq!(t.background.as_deref(), Some("#181c24"));
        assert_eq!(t.foreground.as_deref(), Some("#eceff4"));
    }

    #[test]
    fn bundle_id_applies_the_palette() {
        let mut p = theme("#282a36", "#f8f8f2");
        p.red = Some("#ff5555".into());
        let t = ctx("dracula", vec![bundled("dracula", p, "dark")]).resolve(APP, false);
        assert_eq!(
            (
                t.background.as_deref(),
                t.foreground.as_deref(),
                t.red.as_deref()
            ),
            (Some("#282a36"), Some("#f8f8f2"), Some("#ff5555"))
        );
        assert_eq!(t.cursor_accent.as_deref(), Some("#282a36"));
    }

    #[test]
    fn unknown_id_falls_back_to_app_chrome() {
        assert_eq!(
            ctx("missing", vec![])
                .resolve(APP, false)
                .background
                .as_deref(),
            Some("#141414")
        );
    }

    #[test]
    fn surface_bg_stays_opaque_for_pane_chrome() {
        let c = ctx(
            "homebrew",
            vec![bundled("homebrew", theme("#000000", "#00ff00"), "dark")],
        );
        assert_eq!(c.surface_bg(APP), Rgba::rgb(0, 0, 0));
        assert_eq!(
            c.resolve(APP, true).background.as_deref(),
            Some("#00000000")
        );
        assert_eq!(
            ctx("", vec![]).surface_bg(ThemeId::Paper),
            Rgba::rgb(0xeb, 0xec, 0xef)
        );
    }

    #[test]
    fn only_dark_palettes_frost() {
        let catalog = vec![
            bundled("solarized-light", theme("#fdf6e3", "#657b83"), "light"),
            bundled("homebrew", theme("#000000", "#00ff00"), "dark"),
        ];
        assert!(!ctx("solarized-light", catalog.clone()).frosts(APP));
        assert!(ctx("homebrew", catalog.clone()).frosts(APP));
        assert!(ctx("", catalog.clone()).frosts(APP));
        assert!(!ctx("", catalog).frosts(ThemeId::Paper));
    }

    #[test]
    fn light_chrome_uses_a_dark_tint_selection() {
        assert_eq!(
            ctx("", vec![])
                .resolve(ThemeId::Paper, false)
                .selection_background
                .as_deref(),
            Some("rgba(0,0,0,.12)")
        );
    }

    #[test]
    fn palette_without_foreground_is_ignored() {
        let p = Theme {
            background: Some("#000000".into()),
            foreground: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            ctx("x", vec![bundled("x", p, "dark")])
                .resolve(APP, false)
                .background
                .as_deref(),
            Some("#141414")
        );
    }

    #[test]
    fn search_colours_mix_amber_into_the_background() {
        let d = search_decorations(&APP.tokens());
        assert_eq!(d.active_match_background, Rgba::rgb(0xe0, 0x7a, 0x5f));
        assert_eq!(d.match_background, Rgba::rgb(102, 61, 50));
    }

    #[test]
    fn catalog_json_matches_go() {
        let json = r##"{"id":"vga","name":"VGA","path":"themes/vga.json","uiTheme":"dark","order":10,"palette":{"background":"#000000","foreground":"#aaaaaa"},"bundleName":"Terminal","sourceRoot":"default"}"##;
        let t: TerminalThemeInfo = serde_json::from_str(json).unwrap();
        assert_eq!(
            (t.id.as_str(), t.order, t.palette.foreground.as_deref()),
            ("vga", 10, Some("#aaaaaa"))
        );
    }
}
