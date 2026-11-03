//! Terminal font settings (`TerminalPrefs`, `resolveTerminalFont`, `monoFontStack`) and the
//! character-cell geometry xterm.js derives from them.

use serde::{Deserialize, Serialize};

/// `''` / `0` in the terminal prefs mean "match the editor".
pub const TERMINAL_MATCH_EDITOR: &str = "";
/// Terminals read better tighter than code; 1.55 would cost ~4 rows in the dock.
pub const TERMINAL_LINE_HEIGHTS: [f32; 3] = [1.0, 1.15, 1.35];
/// The editor's sizes, shared by the terminal size picker.
pub const EDITOR_FONT_SIZES: [u32; 6] = [10, 11, 12, 13, 14, 16];
/// `DEFAULT_EDITOR_PREFS.fontFace` / `fontSize`.
pub const DEFAULT_EDITOR_FONT_FACE: &str = "JetBrains Mono";
pub const DEFAULT_EDITOR_FONT_SIZE: u32 = 12;

/// The stack behind the "System Mono" option and every family's fallback
/// (`ui-monospace, 'SF Mono', Menlo, Monaco, Consolas, monospace`).
pub const SYSTEM_MONO_FALLBACKS: [&str; 4] = ["SF Mono", "Menlo", "Monaco", "Consolas"];

/// The terminal's own font preferences, as persisted by the app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalPrefs {
    /// Family name, or `""` to follow the editor's font.
    pub font_face: String,
    /// Point size, or 0 to follow the editor's size.
    pub font_size: u32,
    pub line_height: f32,
    /// Bundle `[[terminalTheme]]` id, or `""` to follow the app chrome.
    pub theme: String,
}

impl Default for TerminalPrefs {
    fn default() -> Self {
        Self {
            font_face: TERMINAL_MATCH_EDITOR.into(),
            font_size: 0,
            line_height: 1.15,
            theme: TERMINAL_MATCH_EDITOR.into(),
        }
    }
}

impl TerminalPrefs {
    /// `normalizeTerminalPrefs`: out-of-range values fall back to the defaults.
    pub fn normalized(mut self) -> Self {
        let d = Self::default();
        if !TERMINAL_LINE_HEIGHTS.contains(&self.line_height) {
            self.line_height = d.line_height;
        }
        if self.font_size != 0 && !EDITOR_FONT_SIZES.contains(&self.font_size) {
            self.font_size = d.font_size;
        }
        self
    }
}

/// The effective terminal font.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalFont {
    pub face: String,
    pub size: f32,
    pub line_height: f32,
}

impl Default for TerminalFont {
    fn default() -> Self {
        resolve_terminal_font(
            &TerminalPrefs::default(),
            DEFAULT_EDITOR_FONT_FACE,
            DEFAULT_EDITOR_FONT_SIZE,
        )
    }
}

/// `resolveTerminalFont`: the editor's face and size fill in "match editor".
pub fn resolve_terminal_font(
    t: &TerminalPrefs,
    editor_face: &str,
    editor_size: u32,
) -> TerminalFont {
    TerminalFont {
        face: if t.font_face.is_empty() {
            editor_face.to_string()
        } else {
            t.font_face.clone()
        },
        size: if t.font_size == 0 {
            editor_size
        } else {
            t.font_size
        } as f32,
        line_height: t.line_height,
    }
}

/// `monoFontStack`: the primary family and its fallbacks. Legacy pref keys map to families and
/// "System Mono" is the platform monospace stack alone.
pub fn mono_font_stack(face: &str) -> (String, Vec<String>) {
    let resolved = match face {
        "jetbrains" => "JetBrains Mono",
        "plex" => "IBM Plex Mono",
        "source" => "Source Code Pro",
        "system" => "System Mono",
        f => f,
    };
    let fallbacks: Vec<String> = SYSTEM_MONO_FALLBACKS
        .iter()
        .map(|s| s.to_string())
        .collect();
    if resolved == "System Mono" {
        return (fallbacks[0].clone(), fallbacks[1..].to_vec());
    }
    (resolved.to_string(), fallbacks)
}

/// xterm.js's WebGL render dimensions, in device pixels, for one font at one scale factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    pub dpr: f32,
    pub font_size: f32,
    pub char_width: f32,
    pub char_height: f32,
    pub cell_width: f32,
    pub cell_height: f32,
    /// Offset of the character box inside the cell (vertical centring for line heights > 1).
    pub char_top: f32,
    /// Descent in device pixels; glyphs sit with their descent line on the char box bottom
    /// (canvas `textBaseline = 'ideographic'`).
    pub descent: f32,
}

impl CellMetrics {
    /// From the font's `W` advance and ascent/descent in CSS pixels (`TextMetrics` width and
    /// `fontBoundingBoxAscent + fontBoundingBoxDescent`).
    pub fn compute(
        advance_w: f32,
        ascent: f32,
        descent: f32,
        font_size: f32,
        line_height: f32,
        dpr: f32,
    ) -> Self {
        let char_width = (advance_w * dpr).floor();
        let char_height = ((ascent + descent) * dpr).ceil();
        let cell_height = (char_height * line_height).floor();
        let char_top = if line_height == 1.0 {
            0.0
        } else {
            ((cell_height - char_height) / 2.0).round()
        };
        Self {
            dpr,
            font_size,
            char_width,
            char_height,
            cell_width: char_width,
            cell_height,
            char_top,
            descent: descent * dpr,
        }
    }

    pub fn css_cell_width(&self) -> f32 {
        self.cell_width / self.dpr
    }

    pub fn css_cell_height(&self) -> f32 {
        self.cell_height / self.dpr
    }

    /// Alphabetic baseline below the cell top, device pixels.
    pub fn baseline(&self) -> f32 {
        self.char_top + self.char_height - self.descent
    }

    /// Underline / overline stroke width (`max(1, floor(fontSize * dpr / 15))`).
    pub fn underline_width(&self) -> f32 {
        (self.font_size * self.dpr / 15.0).floor().max(1.0)
    }

    /// Strikethrough stroke width (`max(1, floor(fontSize * dpr / 10))`).
    pub fn strike_width(&self) -> f32 {
        (self.font_size * self.dpr / 10.0).floor().max(1.0)
    }
}

pub const TERM_FIT_MIN_COLS: usize = 2;
pub const TERM_FIT_MIN_ROWS: usize = 1;

/// `termFitSize`: rows/cols that fit a host's content box.
pub fn term_fit_size(
    inner_width: f32,
    inner_height: f32,
    cell_width: f32,
    cell_height: f32,
    scrollbar_width: f32,
) -> (usize, usize) {
    if cell_width <= 0.0
        || cell_height <= 0.0
        || !cell_width.is_finite()
        || !cell_height.is_finite()
    {
        return (TERM_FIT_MIN_COLS, TERM_FIT_MIN_ROWS);
    }
    let cols = ((inner_width - scrollbar_width) / cell_width)
        .floor()
        .max(TERM_FIT_MIN_COLS as f32) as usize;
    let rows = (inner_height / cell_height)
        .floor()
        .max(TERM_FIT_MIN_ROWS as f32) as usize;
    (cols, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_editor_fallbacks() {
        let f = resolve_terminal_font(&TerminalPrefs::default(), "IBM Plex Mono", 13);
        assert_eq!(
            f,
            TerminalFont {
                face: "IBM Plex Mono".into(),
                size: 13.0,
                line_height: 1.15
            }
        );
        let own = TerminalPrefs {
            font_face: "Hack".into(),
            font_size: 14,
            line_height: 1.35,
            ..Default::default()
        };
        assert_eq!(
            resolve_terminal_font(&own, "IBM Plex Mono", 13),
            TerminalFont {
                face: "Hack".into(),
                size: 14.0,
                line_height: 1.35
            }
        );
    }

    #[test]
    fn normalizes_out_of_range_prefs() {
        let p = TerminalPrefs {
            font_size: 15,
            line_height: 1.55,
            ..Default::default()
        }
        .normalized();
        assert_eq!((p.font_size, p.line_height), (0, 1.15));
    }

    #[test]
    fn font_stacks() {
        assert_eq!(mono_font_stack("jetbrains").0, "JetBrains Mono");
        assert_eq!(
            mono_font_stack("Fira Code"),
            (
                "Fira Code".into(),
                SYSTEM_MONO_FALLBACKS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            )
        );
        assert_eq!(mono_font_stack("System Mono").0, "SF Mono");
    }

    #[test]
    fn xterm_cell_math_for_jetbrains_mono_12px_at_2x() {
        // JetBrains Mono: advance 600/1000 em, ascent 1020, descent 300.
        let m = CellMetrics::compute(7.2, 12.24, 3.6, 12.0, 1.15, 2.0);
        assert_eq!(
            (m.char_width, m.char_height, m.cell_height, m.char_top),
            (14.0, 32.0, 36.0, 2.0)
        );
        assert_eq!((m.css_cell_width(), m.css_cell_height()), (7.0, 18.0));
        assert_eq!(m.underline_width(), 1.0);
        assert_eq!(m.strike_width(), 2.0);
        let compact = CellMetrics::compute(7.2, 12.24, 3.6, 12.0, 1.0, 2.0);
        assert_eq!((compact.cell_height, compact.char_top), (32.0, 0.0));
    }

    #[test]
    fn fit_uses_the_content_box() {
        // Border-box 100px with 6px top padding → 94px content. 20px cells.
        assert_eq!(term_fit_size(200.0, 94.0, 10.0, 20.0, 0.0), (20, 4));
        assert_eq!(term_fit_size(80.0, 50.0, 8.0, 17.0, 0.0), (10, 2));
        assert_eq!(term_fit_size(94.0, 80.0, 8.0, 16.0, 11.0), (10, 5));
        assert_eq!(term_fit_size(0.0, 0.0, 8.0, 16.0, 0.0), (2, 1));
        assert_eq!(term_fit_size(80.0, 80.0, 0.0, 0.0, 0.0), (2, 1));
    }
}
