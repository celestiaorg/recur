//! The editor's resolved look: CodeMirror's base theme, the TextMate theme
//! chrome (`tmChromeExtension`), Recur's layout chrome
//! (`editorChromeExtension`) and the app tokens the editor CSS reads.

use gpui_kit::Rgba;
use recur_textmate::{ParsedTmTheme, theme_chrome};

use recur_ui::{ThemeId, Tokens};

use crate::color::{TRANSPARENT, css, mix, over, parse_css_color, rgba_f, wash};
use crate::prefs::EditorPrefs;

/// A chrome font stack at weight 400 (`recur-ui`'s faces).
pub fn chrome_font(family: recur_ui::fonts::Family) -> gpui_kit::Font {
    family.font(400., false, Default::default())
}

/// `editor.background` when a theme has none.
pub const EDITOR_BACKGROUND_FALLBACK: &str = "#1e1e1e";

/// `.cm-fat-cursor` (vim and emacs block cursor).
pub fn fat_cursor() -> Rgba {
    css("#ff9696", TRANSPARENT)
}

/// `editorChromeExtension`'s diff-line washes in `.diff` / patch views.
pub fn diff_line_wash(kind: crate::prefs::DiffLineKind) -> Rgba {
    match kind {
        crate::prefs::DiffLineKind::Ins => rgba_f(63.0 / 255.0, 185.0 / 255.0, 80.0 / 255.0, 0.14),
        crate::prefs::DiffLineKind::Del => rgba_f(224.0 / 255.0, 101.0 / 255.0, 95.0 / 255.0, 0.14),
        crate::prefs::DiffLineKind::Meta => rgba_f(210.0 / 255.0, 168.0 / 255.0, 1.0, 0.10),
    }
}

/// `codehl`'s `sh-*` token colors in LSP documentation.
pub fn doc_code_token(kind: recur_textmate::ShKind) -> Rgba {
    let c = match kind {
        recur_textmate::ShKind::Number => "#d19a66",
        recur_textmate::ShKind::Keyword => "#6cb6ff",
        recur_textmate::ShKind::String => "#7ee08a",
        recur_textmate::ShKind::Comment => "#7fa9ad",
    };
    css(c, TRANSPARENT)
}

/// The base theme's `.cm-diagnosticAction` fill and ink.
pub fn diagnostic_action() -> (Rgba, Rgba) {
    (css("#444", TRANSPARENT), css("white", TRANSPARENT))
}

/// The base theme's light `.cm-tooltip-section:not(:first-child)` rule.
pub fn tooltip_section_border() -> Rgba {
    css("#bbb", TRANSPARENT)
}

/// The vim panel's prompt description (`#888`) and message (`red`) ink,
/// and the input's `FieldText` (form controls stay light-scheme black: the
/// page sets no `color-scheme`).
pub fn vim_panel_colors() -> (Rgba, Rgba, Rgba) {
    (
        css("#888", TRANSPARENT),
        css("red", TRANSPARENT),
        css("black", TRANSPARENT),
    )
}

/// Every color the editor paints with.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorTheme {
    pub dark: bool,
    pub background: Rgba,
    pub foreground: Rgba,
    /// `.cm-scroller` fill: the TextMate `source` / `text` root background.
    pub source_background: Rgba,
    pub text_background: Rgba,
    pub caret: Rgba,
    pub selection: Rgba,
    pub active_line: Rgba,
    /// `.cm-activeLineGutter`: the TextMate line highlight, else the base
    /// theme's #222227 / #e2f2ff.
    pub active_line_gutter: Rgba,
    pub gutter_background: Rgba,
    pub gutter_foreground: Rgba,
    pub gutter_border: Rgba,
    pub gutter_added: Rgba,
    pub gutter_deleted: Rgba,
    pub gutter_modified: Rgba,
    pub selection_match: Rgba,
    pub search_match: Rgba,
    pub search_match_selected: Rgba,
    pub matching_bracket: Rgba,
    pub nonmatching_bracket: Rgba,
    pub special_char: Rgba,
    pub trailing_space: Rgba,
    pub tab_arrow: Rgba,
    pub fold_placeholder_bg: Rgba,
    pub fold_placeholder_border: Rgba,
    pub fold_placeholder_fg: Rgba,
    pub snippet_field: Rgba,
    pub snippet_field_position: Rgba,
    pub ruler: Rgba,
    /// The app chrome tokens (`recur-ui`).
    pub tokens: Tokens,
    /// The app chrome theme is light (`isLightTheme`).
    pub light: bool,
}

/// The mini script editor's layout chrome: an
/// 18px line in the app's mono face (the code editor's font for `md`), its own content and line
/// padding, a 10.5px line-number gutter, and the faint placeholder.
#[derive(Clone, Debug, PartialEq)]
pub struct MiniChrome {
    /// `md` prose: the code editor's font, size and line height with 6px content padding; else
    /// mono 12px with 8px.
    pub prose: bool,
    /// `fill`: 4px content padding (the host stretches the editor).
    pub fill: bool,
    pub placeholder: String,
}

impl MiniChrome {
    /// `lineH`.
    pub const LINE_PX: f32 = 18.0;

    pub fn font_size(&self) -> f32 {
        if self.prose { 12.5 } else { 12.0 }
    }

    /// `.cm-scroller { lineHeight: lineH / fontSize }` (a unitless factor).
    pub fn line_factor(&self) -> f32 {
        Self::LINE_PX / self.font_size()
    }

    pub fn content_padding_y(&self) -> f32 {
        if self.fill {
            4.0
        } else if self.prose {
            6.0
        } else {
            8.0
        }
    }

    pub fn family(&self) -> recur_ui::fonts::Family {
        if self.prose {
            recur_ui::fonts::Family::Ui
        } else {
            recur_ui::fonts::Family::Mono
        }
    }

    /// `.cm-lineNumbers .cm-gutterElement { fontSize: 10.5px }`.
    pub const GUTTER_FONT: f32 = 10.5;
    /// `.cm-lineNumbers .cm-gutterElement { padding: 0 8px 0 6px }`.
    pub const GUTTER_PAD: (f32, f32) = (6.0, 8.0);
}

/// Layout metrics from prefs (`editorChromeExtension`, `.cm-line` padding,
/// gutter element padding).
#[derive(Clone, Debug, PartialEq)]
pub struct EditorMetrics {
    pub font_families: Vec<String>,
    pub font_size: f32,
    /// Unitless CSS line-height factor.
    pub line_height_factor: f32,
    /// `.cm-content` vertical padding.
    pub content_padding_y: f32,
    /// `.cm-line` left/right padding.
    pub line_padding_left: f32,
    pub line_padding_right: f32,
    pub caret_width: f32,
    pub caret_nudge: f32,
    pub cursor_blink_ms: u64,
}

impl EditorMetrics {
    /// `MiniScriptEditor`'s metrics: `.cm-line { padding: 0 10px 0 4px }`.
    pub fn mini(prefs: &EditorPrefs, mini: &MiniChrome) -> EditorMetrics {
        let mut m = EditorMetrics::from_prefs(prefs);
        if !mini.prose {
            m.font_families = crate::prefs::font_stack(recur_ui::fonts::MONO);
            m.font_size = mini.font_size();
            m.line_height_factor = mini.line_factor();
        }
        m.content_padding_y = mini.content_padding_y();
        m.line_padding_left = 4.0;
        m.line_padding_right = 10.0;
        m
    }

    pub fn from_prefs(prefs: &EditorPrefs) -> EditorMetrics {
        let (caret_width, caret_nudge) = prefs.caret_metrics();
        EditorMetrics {
            font_families: crate::prefs::font_stack(&prefs.font_face),
            font_size: prefs.font_size as f32,
            line_height_factor: prefs.line_height,
            content_padding_y: 8.0,
            line_padding_left: 6.0,
            line_padding_right: 2.0,
            caret_width,
            caret_nudge,
            cursor_blink_ms: prefs.cursor_blink_rate_ms(),
        }
    }

    /// The line box height in pixels (`font-size × line-height`).
    /// A numeric `line-height` as WebKit lays it out: floored to 1/64 px.
    pub fn line_height(&self) -> f32 {
        (self.font_size * self.line_height_factor * 64.0).floor() / 64.0
    }
}

fn opt(c: Option<&String>) -> Option<Rgba> {
    c.and_then(|s| parse_css_color(s))
}

impl EditorTheme {
    /// `MiniScriptEditor`'s `fallbackChrome()` until (or when no) paired theme lands:
    /// transparent editor and gutters, `--faint` numbers, a `--cyan` caret and a cyan (dark)
    /// or black (light) selection wash.
    pub fn mini_fallback(app: ThemeId) -> EditorTheme {
        let light = app.is_light();
        let mut t = EditorTheme::resolve(None, !light, app);
        let tokens = app.tokens();
        t.background = TRANSPARENT;
        t.source_background = TRANSPARENT;
        t.text_background = TRANSPARENT;
        t.foreground = tokens.text.into();
        t.gutter_background = TRANSPARENT;
        t.gutter_border = TRANSPARENT;
        t.gutter_foreground = tokens.faint.into();
        t.active_line_gutter = TRANSPARENT;
        t.caret = tokens.cyan.into();
        t.selection = if light {
            rgba_f(0.0, 0.0, 0.0, 0.18)
        } else {
            rgba_f(109.0 / 255.0, 211.0 / 255.0, 232.0 / 255.0, 0.18)
        };
        t.active_line = if light {
            rgba_f(0.0, 0.0, 0.0, 0.06)
        } else {
            rgba_f(109.0 / 255.0, 211.0 / 255.0, 232.0 / 255.0, 0.06)
        };
        t
    }

    /// Resolve a TextMate theme's chrome; without one, CodeMirror's bare
    /// dark/light base theme (`EditorView.darkTheme`).
    pub fn resolve(
        parsed: Option<&ParsedTmTheme>,
        dark_fallback: bool,
        app: ThemeId,
    ) -> EditorTheme {
        let tokens = app.tokens();
        let (git_a, git_d): (Rgba, Rgba) = (tokens.git_a.into(), tokens.git_d.into());
        let base_dark = parsed.map_or(dark_fallback, |p| {
            p.ui_theme == recur_textmate::UiTheme::Dark
        });
        let (
            background,
            foreground,
            source_bg,
            text_bg,
            caret,
            selection,
            active_line,
            gutter_bg,
            gutter_fg,
            gutter_border,
            added,
            deleted,
        ) = match parsed {
            Some(p) => {
                let ch = theme_chrome(p);
                let bg = css(&ch.background, rgba_f(0.118, 0.118, 0.118, 1.0));
                (
                    bg,
                    css(&ch.foreground, rgba_f(0.8, 0.8, 0.8, 1.0)),
                    css(&ch.source_background, bg),
                    css(&ch.text_background, bg),
                    css(&ch.caret, rgba_f(0.8, 0.8, 0.8, 1.0)),
                    css(&ch.selection, TRANSPARENT),
                    css(&ch.active_line, TRANSPARENT),
                    css(&ch.gutter_background, bg),
                    css(&ch.line_number, rgba_f(0.5, 0.5, 0.5, 1.0)),
                    css(&ch.gutter_border, TRANSPARENT),
                    opt(ch.gutter_added.as_ref()).unwrap_or(git_a),
                    opt(ch.gutter_deleted.as_ref()).unwrap_or(git_d),
                )
            }
            None if base_dark => (
                TRANSPARENT,
                tokens.text.into(),
                TRANSPARENT,
                TRANSPARENT,
                css("#ddd", TRANSPARENT),
                css("#233", TRANSPARENT),
                css("#99eeff33", TRANSPARENT),
                css("#333338", TRANSPARENT),
                css("#ccc", TRANSPARENT),
                TRANSPARENT,
                git_a,
                git_d,
            ),
            None => (
                TRANSPARENT,
                tokens.text.into(),
                TRANSPARENT,
                TRANSPARENT,
                css("black", TRANSPARENT),
                css("#d7d4f0", TRANSPARENT),
                css("#cceeff44", TRANSPARENT),
                css("#f5f5f5", TRANSPARENT),
                css("#6c6c6c", TRANSPARENT),
                css("#ddd", TRANSPARENT),
                git_a,
                git_d,
            ),
        };
        let dark = base_dark;
        EditorTheme {
            dark,
            background,
            foreground,
            source_background: source_bg,
            text_background: text_bg,
            caret,
            selection,
            active_line,
            active_line_gutter: if parsed.is_some() {
                active_line
            } else if dark {
                css("#222227", TRANSPARENT)
            } else {
                css("#e2f2ff", TRANSPARENT)
            },
            gutter_background: gutter_bg,
            gutter_foreground: gutter_fg,
            gutter_border,
            gutter_added: added,
            gutter_deleted: deleted,
            gutter_modified: tokens.git_m.into(),
            selection_match: css("#99ff7780", TRANSPARENT),
            search_match: css(if dark { "#00ffff8a" } else { "#ffff0054" }, TRANSPARENT),
            search_match_selected: css(if dark { "#ff00ff8a" } else { "#ff6a0054" }, TRANSPARENT),
            matching_bracket: css("#328c8252", TRANSPARENT),
            nonmatching_bracket: css("#bb555544", TRANSPARENT),
            special_char: css(if dark { "#f78" } else { "red" }, TRANSPARENT),
            trailing_space: wash(css("#e07a5f", TRANSPARENT), 0.16),
            tab_arrow: css("#88888838", TRANSPARENT),
            fold_placeholder_bg: css("#eee", TRANSPARENT),
            fold_placeholder_border: css("#ddd", TRANSPARENT),
            fold_placeholder_fg: css("#888", TRANSPARENT),
            snippet_field: css(if dark { "#ffffff22" } else { "#00000022" }, TRANSPARENT),
            snippet_field_position: css("#888", TRANSPARENT),
            ruler: css("rgba(150, 148, 144, 0.35)", TRANSPARENT),
            tokens,
            light: app.is_light(),
        }
    }

    /// The pane fill code sits on for a grammar scope.
    pub fn content_background(&self, scope: &str) -> Rgba {
        if recur_textmate::is_text_mate_text_scope(scope) {
            self.text_background
        } else {
            self.source_background
        }
    }

    /// Selection wash: dimmed on an unlit Observatory pane
    /// (`.obs-editor-pane:not(.lit)`).
    pub fn selection_for(&self, lit: bool) -> Rgba {
        if lit {
            self.selection
        } else if self.light {
            rgba_f(0.0, 0.0, 0.0, 0.1)
        } else {
            rgba_f(1.0, 1.0, 1.0, 0.12)
        }
    }

    /// The active-line gutter inset bar: amber while the pane is lit,
    /// `--line` otherwise.
    pub fn active_gutter_bar(&self, lit: bool) -> Rgba {
        if lit {
            self.tokens.amber.into()
        } else {
            self.tokens.line.into()
        }
    }

    /// `.cm-button`'s gradient stops and border.
    pub fn stock_button(&self) -> (Rgba, Rgba, Rgba) {
        let (top, bottom) = if self.dark {
            ("#393939", "#111")
        } else {
            ("#eff1f5", "#d9d9df")
        };
        (
            css(top, TRANSPARENT),
            css(bottom, TRANSPARENT),
            css("#888", TRANSPARENT),
        )
    }

    /// Whitespace dot color: `color-mix(currentColor 16%, transparent)`.
    pub fn whitespace_dot(&self, current: Rgba) -> Rgba {
        wash(current, 0.16)
    }

    /// Merge view washes.
    pub fn merge(&self) -> MergeColors {
        let (git_a, git_d): (Rgba, Rgba) = (self.tokens.git_a.into(), self.tokens.git_d.into());
        MergeColors {
            changed_line: wash(git_a, 0.10),
            changed_line_active: mix(git_a, 0.10, self.active_line),
            deleted_chunk: wash(git_d, 0.10),
            changed_text: wash(git_a, 0.12),
            deleted_text: wash(git_d, 0.14),
            gutter_changed: git_a,
            gutter_deleted: git_d,
            collapsed_fg: if self.dark {
                rgba_f(230.0 / 255.0, 228.0 / 255.0, 225.0 / 255.0, 0.62)
            } else {
                rgba_f(26.0 / 255.0, 28.0 / 255.0, 32.0 / 255.0, 0.5)
            },
            collapsed_fg_hover: if self.dark {
                rgba_f(230.0 / 255.0, 228.0 / 255.0, 225.0 / 255.0, 0.88)
            } else {
                rgba_f(26.0 / 255.0, 28.0 / 255.0, 32.0 / 255.0, 0.78)
            },
            collapsed_bg: if self.dark {
                rgba_f(1.0, 1.0, 1.0, 0.075)
            } else {
                rgba_f(0.0, 0.0, 0.0, 0.045)
            },
            collapsed_bg_hover: if self.dark {
                rgba_f(1.0, 1.0, 1.0, 0.11)
            } else {
                rgba_f(0.0, 0.0, 0.0, 0.07)
            },
            collapsed_border: if self.dark {
                rgba_f(1.0, 1.0, 1.0, 0.1)
            } else {
                rgba_f(0.0, 0.0, 0.0, 0.08)
            },
            revert_fg: if self.dark {
                rgba_f(230.0 / 255.0, 228.0 / 255.0, 225.0 / 255.0, 0.92)
            } else {
                rgba_f(26.0 / 255.0, 28.0 / 255.0, 32.0 / 255.0, 0.86)
            },
            revert_fg_hover: if self.dark {
                rgba_f(230.0 / 255.0, 228.0 / 255.0, 225.0 / 255.0, 0.98)
            } else {
                rgba_f(26.0 / 255.0, 28.0 / 255.0, 32.0 / 255.0, 0.94)
            },
            revert_bg: if self.dark {
                mix(self.source_background, 0.78, rgba_f(1.0, 1.0, 1.0, 1.0))
            } else {
                mix(self.source_background, 0.86, rgba_f(0.0, 0.0, 0.0, 1.0))
            },
            revert_bg_hover: if self.dark {
                mix(self.source_background, 0.68, rgba_f(1.0, 1.0, 1.0, 1.0))
            } else {
                mix(self.source_background, 0.78, rgba_f(0.0, 0.0, 0.0, 1.0))
            },
            revert_border: if self.dark {
                rgba_f(1.0, 1.0, 1.0, 0.2)
            } else {
                rgba_f(0.0, 0.0, 0.0, 0.16)
            },
        }
    }

    /// Lint underline / point colors (`@codemirror/lint` base theme).
    pub fn lint_color(&self, severity: crate::lint::Severity) -> Rgba {
        use crate::lint::Severity::*;
        css(
            match severity {
                Error => "#f11",
                Warning => "orange",
                Info => "#999",
                Hint => "#66d",
            },
            TRANSPARENT,
        )
    }

    pub fn lint_point_color(&self, severity: crate::lint::Severity) -> Rgba {
        use crate::lint::Severity::*;
        css(
            match severity {
                Error => "#d11",
                Warning => "orange",
                Info => "#999",
                Hint => "#66d",
            },
            TRANSPARENT,
        )
    }

    /// `.cm-diagnostic-<sev>` left border colors.
    pub fn diagnostic_border(&self, severity: crate::lint::Severity) -> Rgba {
        use crate::lint::Severity::*;
        css(
            match severity {
                Error => "#d11",
                Warning => "orange",
                Info => "#999",
                Hint => "#66d",
            },
            TRANSPARENT,
        )
    }

    /// Recur's lint gutter icon colors.
    pub fn lint_gutter_color(&self, severity: crate::lint::Severity) -> Rgba {
        use crate::lint::Severity::*;
        match severity {
            Error => self.tokens.coral.into(),
            Warning => self.tokens.amber.into(),
            Info => self.tokens.cyan.into(),
            Hint => self.tokens.muted.into(),
        }
    }

    /// Findings gutter icon colors.
    pub fn finding_color(&self, severity: &str) -> Rgba {
        match crate::gutters::norm_finding_severity(severity) {
            "critical" | "high" => self.tokens.coral.into(),
            "medium" => self.tokens.amber.into(),
            _ => self.tokens.muted.into(),
        }
    }

    /// The dimmer of two fills when painting one over another.
    pub fn composite(top: Rgba, bottom: Rgba) -> Rgba {
        over(top, bottom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MergeColors {
    pub changed_line: Rgba,
    pub changed_line_active: Rgba,
    pub deleted_chunk: Rgba,
    pub changed_text: Rgba,
    pub deleted_text: Rgba,
    pub gutter_changed: Rgba,
    pub gutter_deleted: Rgba,
    pub collapsed_fg: Rgba,
    pub collapsed_fg_hover: Rgba,
    pub collapsed_bg: Rgba,
    pub collapsed_bg_hover: Rgba,
    pub collapsed_border: Rgba,
    pub revert_fg: Rgba,
    pub revert_fg_hover: Rgba,
    pub revert_bg: Rgba,
    pub revert_bg_hover: Rgba,
    pub revert_border: Rgba,
}

/// Theme cycle order and picker helpers.
pub mod catalog {
    use std::cmp::Ordering;

    use crate::text::locale_compare;

    pub const DEFAULT_THEME_ID: &str = "recur";
    /// First-party themes pinned at the top of the picker.
    pub const PINNED_EDITOR_THEME_KEYS: [&str; 2] = ["recur", "paper"];

    /// App chrome key -> bundle editor theme (`APP_CHROME_EDITOR_THEME`).
    pub fn app_chrome_editor_theme(app_theme: &str) -> &'static str {
        match app_theme {
            "paper" => "paper",
            "slate" => "github-dark-default",
            "coolglow" => "cool-glow",
            "catppuccin-mocha" => "catppuccin-mocha",
            "catppuccin-latte" => "catppuccin-latte",
            "nord" => "nord",
            _ => "recur",
        }
    }

    /// `editorThemeIdForAppChrome`: the preferred theme if usable, else
    /// Paper/Charcoal by chrome.
    pub fn editor_theme_id_for_app_chrome(
        app_theme: &str,
        usable: impl Fn(&str) -> bool,
    ) -> String {
        let preferred = app_chrome_editor_theme(app_theme);
        if usable(preferred) {
            return preferred.into();
        }
        let fallback = if app_theme == "paper" || app_theme == "catppuccin-latte" {
            "paper"
        } else {
            "recur"
        };
        if fallback != preferred && usable(fallback) {
            return fallback.into();
        }
        preferred.into()
    }

    /// `isEditorThemeKey` before the catalog loads.
    pub fn looks_like_theme_key(key: &str) -> bool {
        let b = key.as_bytes();
        !b.is_empty()
            && b[0].is_ascii_alphanumeric()
            && b.iter()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    }

    /// A theme row for the picker (`EditorThemeCard`).
    #[derive(Clone, Debug, PartialEq)]
    pub struct EditorThemeCard {
        pub key: String,
        pub name: String,
        pub dark: bool,
        pub bg: String,
        pub fg: String,
        pub a: String,
        pub b: String,
        pub c: String,
        pub pinned: bool,
    }

    fn pinned_rank(key: &str) -> Option<usize> {
        PINNED_EDITOR_THEME_KEYS.iter().position(|k| *k == key)
    }

    /// `allEditorThemesForPicker` ordering: pinned first, then by name.
    pub fn sort_for_picker(cards: &mut [EditorThemeCard]) {
        cards.sort_by(|a, b| match (pinned_rank(&a.key), pinned_rank(&b.key)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => locale_compare(&a.name, &b.name),
        });
    }

    /// `filterEditorThemesForList`.
    pub fn filter_for_list(
        cards: &[EditorThemeCard],
        query: &str,
        kind: Option<bool>,
    ) -> Vec<EditorThemeCard> {
        let q = query.trim().to_lowercase();
        let matches = |t: &EditorThemeCard| {
            kind.is_none_or(|dark| t.dark == dark)
                && (q.is_empty()
                    || t.name.to_lowercase().contains(&q)
                    || t.key.to_lowercase().contains(&q))
        };
        let mut out: Vec<EditorThemeCard> = PINNED_EDITOR_THEME_KEYS
            .iter()
            .filter_map(|k| cards.iter().find(|t| t.key == *k))
            .filter(|t| matches(t))
            .cloned()
            .collect();
        let mut rest: Vec<EditorThemeCard> = cards
            .iter()
            .filter(|t| !t.pinned && matches(t))
            .cloned()
            .collect();
        rest.sort_by(|a, b| locale_compare(&a.name, &b.name));
        out.extend(rest);
        out
    }

    /// `compareEditorThemesForCycle`: dark A–Z then light A–Z, pinned first.
    pub fn compare_for_cycle(a: &EditorThemeCard, b: &EditorThemeCard) -> Ordering {
        if a.dark != b.dark {
            return if a.dark {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        match (pinned_rank(&a.key), pinned_rank(&b.key)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => locale_compare(&a.name, &b.name),
        }
    }

    /// `cycleEditorThemeKey`: wraps; unknown keys start at the first.
    pub fn cycle_key(current: &str, delta: i64, keys: &[String]) -> String {
        if keys.is_empty() {
            return if current.is_empty() {
                DEFAULT_THEME_ID.into()
            } else {
                current.into()
            };
        }
        let cur = if current.is_empty() {
            DEFAULT_THEME_ID
        } else {
            current
        };
        let i = keys.iter().position(|k| k == cur).unwrap_or(0) as i64;
        let n = keys.len() as i64;
        keys[(((i + delta) % n + n) % n) as usize].clone()
    }

    /// `fallbackEnabledEditorThemeId`.
    pub fn fallback_enabled(
        enabled_ids: &[String],
        current: Option<&str>,
        disabled: impl Fn(&str) -> bool,
    ) -> String {
        if let Some(c) = current
            && !disabled(c)
            && looks_like_theme_key(c)
        {
            return c.into();
        }
        for id in PINNED_EDITOR_THEME_KEYS {
            if enabled_ids.iter().any(|e| e == id) {
                return id.into();
            }
        }
        enabled_ids
            .first()
            .cloned()
            .unwrap_or_else(|| current.unwrap_or(DEFAULT_THEME_ID).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::catalog::*;
    use super::*;

    #[test]
    fn resolves_tm_chrome() {
        let json = serde_json::json!({
            "name": "T", "type": "dark",
            "colors": {"editor.background": "#101010", "editor.foreground": "#eeeeee", "editorCursor.foreground": "#ff0000"},
            "tokenColors": []
        });
        let parsed = recur_textmate::parse_theme_value(&json, "t");
        let t = EditorTheme::resolve(Some(&parsed), true, ThemeId::Charcoal);
        assert!(t.dark);
        assert!((t.caret.r - 1.0).abs() < 0.01);
        assert!((t.background.r - 16.0 / 255.0).abs() < 0.01);
        assert_eq!(t.selection_for(false), rgba_f(1.0, 1.0, 1.0, 0.12));
    }

    #[test]
    fn theme_catalog_helpers() {
        assert_eq!(app_chrome_editor_theme("slate"), "github-dark-default");
        assert_eq!(
            editor_theme_id_for_app_chrome("paper", |id| id == "paper"),
            "paper"
        );
        assert_eq!(
            editor_theme_id_for_app_chrome("nord", |id| id == "recur"),
            "recur"
        );
        let keys: Vec<String> = ["recur", "a", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(cycle_key("b", 1, &keys), "recur");
        assert_eq!(cycle_key("zzz", -1, &keys), "b");
        assert!(looks_like_theme_key("github-dark-default"));
        assert!(!looks_like_theme_key("-x"));
    }
}
