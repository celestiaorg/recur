//! TextMate / VS Code theme loading and editor chrome colours.

use std::collections::HashMap;

use serde_json::Value;

use crate::grammar::meta;
use crate::theme::{RawTheme, RawThemeSetting, ThemeScope};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiTheme {
    Dark,
    Light,
}

impl UiTheme {
    pub fn parse(s: &str) -> Option<UiTheme> {
        match s {
            "dark" => Some(UiTheme::Dark),
            "light" => Some(UiTheme::Light),
            _ => None,
        }
    }
}

/// `source` / `text` root backgrounds from classic TextMate rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RootScopeBackgrounds {
    pub source: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedTmTheme {
    pub name: String,
    pub ui_theme: UiTheme,
    pub raw_theme: RawTheme,
    pub colors: HashMap<String, String>,
    pub root_backgrounds: RootScopeBackgrounds,
}

/// A theme rule as copied out of the JSON (`MutableTmRule`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TmRule {
    pub name: Option<String>,
    pub scope: Option<Value>,
    pub settings: HashMap<String, String>,
}

fn scope_is_falsy(scope: &Option<Value>) -> bool {
    match scope {
        None | Some(Value::Null) | Some(Value::Bool(false)) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        _ => false,
    }
}

fn string_settings(v: Option<&Value>) -> HashMap<String, String> {
    v.and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn copy_rules(json: &Value) -> Vec<TmRule> {
    let entries = match json.get("settings").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a,
        _ => match json.get("tokenColors").and_then(Value::as_array) {
            Some(a) => a,
            None => return Vec::new(),
        },
    };
    entries
        .iter()
        .map(|e| TmRule {
            name: e.get("name").and_then(Value::as_str).map(str::to_string),
            scope: e.get("scope").cloned(),
            settings: string_settings(e.get("settings")),
        })
        .collect()
}

/// Fill the global rule's colours so vscode-textmate never defaults the
/// token background to `#ffffff`.
pub fn ensure_global_theme_chrome(settings: &mut Vec<TmRule>, colors: &HashMap<String, String>) {
    let fg = colors
        .get("editor.foreground")
        .cloned()
        .unwrap_or_else(|| "#cccccc".into());
    let bg = colors
        .get("editor.background")
        .cloned()
        .unwrap_or_else(|| "#1e1e1e".into());
    match settings.iter_mut().find(|s| scope_is_falsy(&s.scope)) {
        None => settings.insert(
            0,
            TmRule {
                settings: HashMap::from([("foreground".into(), fg), ("background".into(), bg)]),
                ..TmRule::default()
            },
        ),
        Some(global) => {
            let s = &mut global.settings;
            if s.get("foreground").is_none_or(String::is_empty) {
                s.insert("foreground".into(), fg);
            }
            if s.get("background").is_none_or(String::is_empty) {
                s.insert("background".into(), bg);
            }
        }
    }
}

/// Comma-separated or array scopes; a space-separated selector stays whole.
pub fn theme_scope_selectors(scope: Option<&Value>) -> Vec<String> {
    let parts: Vec<&Value> = match scope {
        None | Some(Value::Null) => return Vec::new(),
        Some(Value::String(s)) if s.is_empty() => return Vec::new(),
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v) => vec![v],
    };
    parts
        .into_iter()
        .filter_map(Value::as_str)
        .flat_map(|p| p.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Language-root `source` / `text` only.
pub fn is_root_language_scope(scope: Option<&Value>) -> bool {
    let sels = theme_scope_selectors(scope);
    !sels.is_empty() && sels.iter().all(|s| s == "source" || s == "text")
}

pub fn strip_root_scope_token_backgrounds(settings: &mut [TmRule]) {
    for entry in settings {
        if is_root_language_scope(entry.scope.as_ref())
            && entry
                .settings
                .get("background")
                .is_some_and(|b| !b.is_empty())
        {
            entry.settings.remove("background");
        }
    }
}

pub fn extract_root_scope_backgrounds(settings: &[TmRule]) -> RootScopeBackgrounds {
    let mut out = RootScopeBackgrounds::default();
    for entry in settings {
        if !is_root_language_scope(entry.scope.as_ref()) {
            continue;
        }
        let Some(bg) = entry.settings.get("background") else {
            continue;
        };
        if is_css_transparent(Some(bg)) {
            continue;
        }
        for sel in theme_scope_selectors(entry.scope.as_ref()) {
            match sel.as_str() {
                "source" => out.source = Some(bg.clone()),
                "text" => out.text = Some(bg.clone()),
                _ => {}
            }
        }
    }
    out
}

fn to_raw_setting(rule: &TmRule) -> RawThemeSetting {
    let scope = match &rule.scope {
        Some(Value::String(s)) => ThemeScope::One(s.clone()),
        Some(Value::Array(a)) => ThemeScope::Many(
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        ),
        _ => ThemeScope::None,
    };
    RawThemeSetting {
        name: rule.name.clone(),
        scope,
        font_style: rule.settings.get("fontStyle").cloned(),
        foreground: rule.settings.get("foreground").cloned(),
        background: rule.settings.get("background").cloned(),
    }
}

fn theme_colors(json: &Value) -> HashMap<String, String> {
    string_settings(json.get("colors"))
}

/// VS Code theme JSON -> raw TextMate theme for the tokenizer.
pub fn to_raw_theme(json: &Value, fallback_name: &str) -> RawTheme {
    let mut settings = copy_rules(json);
    ensure_global_theme_chrome(&mut settings, &theme_colors(json));
    strip_root_scope_token_backgrounds(&mut settings);
    RawTheme {
        name: Some(
            json.get("name")
                .and_then(Value::as_str)
                .unwrap_or(fallback_name)
                .to_string(),
        ),
        settings: settings.iter().map(to_raw_setting).collect(),
    }
}

fn is_hex(s: &str, digits: usize) -> bool {
    s.len() == digits + 1 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Normalize #RGB / #RRGGBB / #RRGGBBAA for equality (alpha ignored).
pub fn css_color_key(c: &str) -> String {
    let s = c.trim().to_lowercase();
    if is_hex(&s, 3) {
        let b: Vec<char> = s.chars().collect();
        return format!("#{0}{0}{1}{1}{2}{2}", b[1], b[2], b[3]);
    }
    if is_hex(&s, 8) {
        return s[..7].to_string();
    }
    s
}

/// Missing, `transparent`, or alpha-0 hex.
pub fn is_css_transparent(c: Option<&String>) -> bool {
    let Some(c) = c.map(|c| c.trim()).filter(|c| !c.is_empty()) else {
        return true;
    };
    let s = c.to_lowercase();
    if s == "transparent" {
        return true;
    }
    if is_hex(&s, 4) {
        return &s[4..] == "0";
    }
    if is_hex(&s, 8) {
        return &s[7..] == "00";
    }
    false
}

/// 0-1 opacity from hex; omitted alpha and non-hex are opaque.
pub fn css_color_alpha(c: &str) -> f64 {
    let s = c.trim().to_lowercase();
    if is_hex(&s, 4) {
        return f64::from(u8::from_str_radix(&s[4..5], 16).unwrap_or(15)) / 15.0;
    }
    if is_hex(&s, 8) {
        return f64::from(u8::from_str_radix(&s[7..9], 16).unwrap_or(255)) / 255.0;
    }
    1.0
}

/// Near-transparent washes read as patchy boxes on glyph-sized spans.
pub const TOKEN_BG_MIN_ALPHA: f64 = 32.0 / 255.0;

pub fn should_paint_token_background(color: Option<&str>, document_bgs: &[String]) -> bool {
    let Some(color) = color else { return false };
    if is_css_transparent(Some(&color.to_string())) {
        return false;
    }
    if css_color_alpha(color) < TOKEN_BG_MIN_ALPHA {
        return false;
    }
    let key = css_color_key(color);
    !document_bgs
        .iter()
        .any(|bg| !bg.is_empty() && css_color_key(bg) == key)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub fn parse_hex_rgb(c: &str) -> Option<Rgb> {
    let s = css_color_key(c);
    if !is_hex(&s, 6) {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some(Rgb {
        r: p(1)?,
        g: p(3)?,
        b: p(5)?,
    })
}

fn hex_rgb(r: f64, g: f64, b: f64) -> String {
    let h = |n: f64| format!("{:02x}", n.clamp(0.0, 255.0) as u8);
    format!("#{}{}{}", h(r), h(g), h(b))
}

/// JS `Math.round` (halves toward +infinity).
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// Blend `fg` over `bg` by `fg`'s alpha.
pub fn composite_css_color(fg: &str, bg: &str) -> String {
    let (Some(top), Some(bot)) = (parse_hex_rgb(fg), parse_hex_rgb(bg)) else {
        return fg.to_string();
    };
    let a = css_color_alpha(fg);
    if a >= 1.0 {
        return css_color_key(fg);
    }
    let mix = |t: u8, b: u8| js_round(a * f64::from(t) + (1.0 - a) * f64::from(b));
    hex_rgb(mix(top.r, bot.r), mix(top.g, bot.g), mix(top.b, bot.b))
}

fn srgb_luma(c: Rgb) -> f64 {
    let lin = |n: u8| {
        let x = f64::from(n) / 255.0;
        if x <= 0.04045 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG contrast of `fg` (alpha-composited onto `bg`) against `bg`.
pub fn contrast_ratio(fg: &str, bg: &str) -> Option<f64> {
    let a = parse_hex_rgb(&composite_css_color(fg, bg))?;
    let b = parse_hex_rgb(bg)?;
    let (l1, l2) = (srgb_luma(a), srgb_luma(b));
    Some((l1.max(l2) + 0.05) / (l1.min(l2) + 0.05))
}

pub const TOKEN_FG_MIN_CONTRAST: f64 = 1.6;

pub fn token_foreground_readable(fg: Option<&str>, bg: Option<&str>) -> bool {
    let (Some(fg), Some(bg)) = (fg.filter(|s| !s.is_empty()), bg.filter(|s| !s.is_empty())) else {
        return true;
    };
    contrast_ratio(fg, bg).is_none_or(|c| c >= TOKEN_FG_MIN_CONTRAST)
}

/// Rec. 601 luma (TextMate `color_is_dark`).
pub fn is_css_color_dark(c: &str) -> Option<bool> {
    let rgb = parse_hex_rgb(c)?;
    Some(
        0.30 * (f64::from(rgb.r) / 255.0)
            + 0.59 * (f64::from(rgb.g) / 255.0)
            + 0.11 * (f64::from(rgb.b) / 255.0)
            < 0.5,
    )
}

/// TextMate `OakColorCreateCopySoften`.
pub fn soften_css_color(c: &str, factor: f64) -> Option<String> {
    let rgb = parse_hex_rgb(c)?;
    let t = factor.clamp(0.0, 1.0);
    let dark = is_css_color_dark(c) == Some(true);
    let ch = |n: u8| {
        let x = f64::from(n) / 255.0;
        let y = if dark { 1.0 - t * (1.0 - x) } else { x * t };
        js_round(y * 255.0)
    };
    Some(hex_rgb(ch(rgb.r), ch(rgb.g), ch(rgb.b)))
}

fn opaque<'a>(colors: &'a HashMap<String, String>, key: &str) -> Option<&'a String> {
    colors.get(key).filter(|c| !is_css_transparent(Some(c)))
}

pub fn resolve_gutter_background(colors: &HashMap<String, String>, editor_bg: &str) -> String {
    if let Some(c) = opaque(colors, "editorGutter.background") {
        return c.clone();
    }
    soften_css_color(editor_bg, 0.87).unwrap_or_else(|| editor_bg.to_string())
}

pub fn resolve_gutter_border(
    colors: &HashMap<String, String>,
    editor_fg: &str,
    ui: UiTheme,
) -> String {
    let explicit = colors
        .get("gutter.divider")
        .filter(|c| !c.is_empty())
        .or_else(|| colors.get("editorGroup.border"));
    if let Some(c) = explicit.filter(|c| !is_css_transparent(Some(c))) {
        return c.clone();
    }
    let fallback = match ui {
        UiTheme::Dark => "#282828",
        UiTheme::Light => "#cfd3db",
    };
    if opaque(colors, "editorGutter.background").is_none() {
        return soften_css_color(editor_fg, 0.4).unwrap_or_else(|| fallback.to_string());
    }
    fallback.to_string()
}

/// SCM gutter hint colour; `None` means "use the app's git token".
pub fn resolve_gutter_hint_color(colors: &HashMap<String, String>, key: &str) -> Option<String> {
    opaque(colors, key).cloned()
}

pub fn resolve_line_number_foreground(colors: &HashMap<String, String>, editor_fg: &str) -> String {
    if let Some(c) = opaque(colors, "editorLineNumber.foreground") {
        return c.clone();
    }
    soften_css_color(editor_fg, 0.5).unwrap_or_else(|| "#6b7280".into())
}

pub fn with_css_alpha(c: &str, alpha_byte: f64) -> Option<String> {
    let key = css_color_key(c);
    if !is_hex(&key, 6) {
        return None;
    }
    Some(format!(
        "{key}{:02x}",
        js_round(alpha_byte).clamp(0.0, 255.0) as u8
    ))
}

pub fn resolve_selection_background(
    colors: &HashMap<String, String>,
    editor_fg: &str,
    ui: UiTheme,
) -> String {
    if let Some(c) = opaque(colors, "editor.selectionBackground") {
        return c.clone();
    }
    with_css_alpha(editor_fg, f64::from(0x4d)).unwrap_or_else(|| match ui {
        UiTheme::Dark => "rgba(255,255,255,0.25)".into(),
        UiTheme::Light => "rgba(0,0,0,0.18)".into(),
    })
}

pub fn resolve_line_highlight_background(
    colors: &HashMap<String, String>,
    editor_bg: &str,
    ui: UiTheme,
) -> String {
    if let Some(c) = opaque(colors, "editor.lineHighlightBackground") {
        return c.clone();
    }
    soften_css_color(editor_bg, 0.95).unwrap_or_else(|| match ui {
        UiTheme::Dark => "rgba(255,255,255,0.06)".into(),
        UiTheme::Light => "rgba(0,0,0,0.06)".into(),
    })
}

fn merge_gutter_settings(json: &Value) -> HashMap<String, String> {
    let mut colors = theme_colors(json);
    let gs = string_settings(json.get("gutterSettings"));
    for (from, key) in [
        ("background", "editorGutter.background"),
        ("foreground", "editorLineNumber.foreground"),
        ("divider", "gutter.divider"),
    ] {
        if let Some(v) = gs.get(from).filter(|v| !v.is_empty())
            && colors.get(key).is_none_or(String::is_empty)
        {
            colors.insert(key.into(), v.clone());
        }
    }
    colors
}

fn lift_global_chrome_settings(
    json: &Value,
    mut colors: HashMap<String, String>,
) -> HashMap<String, String> {
    let rules = copy_rules(json);
    let Some(global) = rules.iter().find(|r| scope_is_falsy(&r.scope)) else {
        return colors;
    };
    for (from, key) in [
        ("background", "editor.background"),
        ("foreground", "editor.foreground"),
        ("caret", "editorCursor.foreground"),
        ("selection", "editor.selectionBackground"),
        ("lineHighlight", "editor.lineHighlightBackground"),
        ("invisibles", "editorWhitespace.foreground"),
    ] {
        if let Some(v) = global.settings.get(from).filter(|v| !v.is_empty())
            && colors.get(key).is_none_or(String::is_empty)
        {
            colors.insert(key.into(), v.clone());
        }
    }
    colors
}

#[derive(Debug, thiserror::Error)]
#[error("invalid theme JSON: {0}")]
pub struct ThemeParseError(#[from] serde_json::Error);

pub fn parse_theme_json(text: &str, fallback_name: &str) -> Result<ParsedTmTheme, ThemeParseError> {
    let json: Value = serde_json::from_str(text)?;
    Ok(parse_theme_value(&json, fallback_name))
}

pub fn parse_theme_value(json: &Value, fallback_name: &str) -> ParsedTmTheme {
    let ui_theme = if json.get("type").and_then(Value::as_str) == Some("light") {
        UiTheme::Light
    } else {
        UiTheme::Dark
    };
    let rules = copy_rules(json);
    ParsedTmTheme {
        name: json
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(fallback_name)
            .to_string(),
        ui_theme,
        raw_theme: to_raw_theme(json, fallback_name),
        colors: lift_global_chrome_settings(json, merge_gutter_settings(json)),
        root_backgrounds: extract_root_scope_backgrounds(&rules),
    }
}

/// Editor chrome derived from a theme (`tmChromeExtension`).
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeChrome {
    pub dark: bool,
    pub background: String,
    pub foreground: String,
    pub caret: String,
    pub selection: String,
    pub active_line: String,
    pub gutter_background: String,
    pub gutter_border: String,
    pub line_number: String,
    pub source_background: String,
    pub text_background: String,
    /// `None`: fall back to the app's `--git-a` / `--git-d`.
    pub gutter_added: Option<String>,
    pub gutter_deleted: Option<String>,
}

pub fn theme_chrome(parsed: &ParsedTmTheme) -> ThemeChrome {
    let c = &parsed.colors;
    let bg = c
        .get("editor.background")
        .cloned()
        .unwrap_or_else(|| "#1e1e1e".into());
    let fg = c
        .get("editor.foreground")
        .cloned()
        .unwrap_or_else(|| "#cccccc".into());
    ThemeChrome {
        dark: parsed.ui_theme == UiTheme::Dark,
        caret: c
            .get("editorCursor.foreground")
            .cloned()
            .unwrap_or_else(|| fg.clone()),
        selection: resolve_selection_background(c, &fg, parsed.ui_theme),
        active_line: resolve_line_highlight_background(c, &bg, parsed.ui_theme),
        gutter_background: resolve_gutter_background(c, &bg),
        gutter_border: resolve_gutter_border(c, &fg, parsed.ui_theme),
        line_number: resolve_line_number_foreground(c, &fg),
        source_background: parsed
            .root_backgrounds
            .source
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| bg.clone()),
        text_background: parsed
            .root_backgrounds
            .text
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| bg.clone()),
        gutter_added: resolve_gutter_hint_color(c, "editorGutter.addedBackground"),
        gutter_deleted: resolve_gutter_hint_color(c, "editorGutter.deletedBackground"),
        background: bg,
        foreground: fg,
    }
}

pub fn is_text_mate_text_scope(scope: &str) -> bool {
    scope == "text" || scope.starts_with("text.")
}

pub mod tm_font {
    pub const ITALIC: u32 = 1;
    pub const BOLD: u32 = 2;
    pub const UNDERLINE: u32 = 4;
    pub const STRIKETHROUGH: u32 = 8;
}

pub fn decode_foreground(m: u32) -> u32 {
    (m & meta::FOREGROUND_MASK) >> meta::FOREGROUND_OFFSET
}

/// Colour-map index of the token background; 0 = none.
pub fn decode_background(m: u32) -> u32 {
    (m & meta::BACKGROUND_MASK) >> meta::BACKGROUND_OFFSET
}

pub fn decode_font_style(m: u32) -> u32 {
    (m & meta::FONT_STYLE_MASK) >> meta::FONT_STYLE_OFFSET
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn colors(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn converts_token_colors_to_raw_settings() {
        let raw = to_raw_theme(
            &json!({"name":"Demo","tokenColors":[
                {"settings":{"foreground":"#fff","background":"#000"}},
                {"scope":"keyword","settings":{"foreground":"#f0f"}}]}),
            "theme",
        );
        assert_eq!(raw.name.as_deref(), Some("Demo"));
        assert_eq!(raw.settings.len(), 2);
        assert_eq!(raw.settings[1].scope, ThemeScope::One("keyword".into()));
    }

    #[test]
    fn parses_cool_glow_shaped_json() {
        let p = parse_theme_json(
            &json!({"name":"Cool Glow","type":"dark","colors":{"editor.background":"#06071D"},
                "tokenColors":[{"settings":{"foreground":"#E0E0E0","background":"#06071D"}},
                    {"scope":["keyword"],"settings":{"foreground":"#2BF1DC"}}]})
            .to_string(),
            "theme",
        )
        .unwrap();
        assert_eq!(p.ui_theme, UiTheme::Dark);
        assert_eq!(p.colors["editor.background"], "#06071D");
        assert!(!p.raw_theme.settings.is_empty());
    }

    #[test]
    fn decodes_metadata_bits() {
        let m = (9 << 15) | (4 << 24);
        assert_eq!(decode_foreground(m), 9);
        assert_eq!(decode_background(m), 4);
        assert_eq!(decode_background(9 << 15), 0);
    }

    #[test]
    fn fills_missing_global_background() {
        let raw = to_raw_theme(
            &json!({"name":"Gruv","colors":{"editor.background":"#1d2021","editor.foreground":"#ebdbb2"},
                "tokenColors":[{"settings":{"foreground":"#ebdbb2"}},{"scope":"keyword","settings":{"foreground":"#fb4934"}}]}),
            "theme",
        );
        let global = raw
            .settings
            .iter()
            .find(|s| s.scope == ThemeScope::None)
            .unwrap();
        assert_eq!(global.background.as_deref(), Some("#1d2021"));
        assert_eq!(global.foreground.as_deref(), Some("#ebdbb2"));
    }

    #[test]
    fn color_keys_and_transparency() {
        assert_eq!(css_color_key("#1D2021"), "#1d2021");
        assert_eq!(css_color_key("#102622FA"), "#102622");
        assert_eq!(css_color_key("#abc"), "#aabbcc");
        let mut s = vec![TmRule {
            settings: colors(&[("foreground", "#fff"), ("background", "#090916")]),
            ..TmRule::default()
        }];
        ensure_global_theme_chrome(&mut s, &colors(&[("editor.background", "#1e1e1e")]));
        assert_eq!(s[0].settings["background"], "#090916");
        let t = |c: Option<&str>| is_css_transparent(c.map(str::to_string).as_ref());
        assert!(t(None));
        assert!(t(Some("transparent")));
        assert!(t(Some("#2d353b00")));
        assert!(t(Some("#0000")));
        assert!(!t(Some("#161616")));
    }

    #[test]
    fn gutter_background_and_border() {
        assert_eq!(
            resolve_gutter_background(
                &colors(&[("editorGutter.background", "#161616")]),
                "#0e0e0e"
            ),
            "#161616"
        );
        assert_eq!(
            resolve_gutter_background(
                &colors(&[("editorGutter.background", "#2d353b00")]),
                "#372725"
            ),
            soften_css_color("#372725", 0.87).unwrap()
        );
        assert_eq!(
            resolve_gutter_background(&colors(&[]), "#372725"),
            "#514341"
        );
        assert_eq!(
            resolve_gutter_border(
                &colors(&[("editorGroup.border", "#3c3836")]),
                "#E6E1C4",
                UiTheme::Dark
            ),
            "#3c3836"
        );
        assert_eq!(
            resolve_gutter_border(
                &colors(&[("gutter.divider", "#5c5a4e")]),
                "#E6E1C4",
                UiTheme::Dark
            ),
            "#5c5a4e"
        );
        assert_eq!(
            resolve_gutter_border(&colors(&[]), "#E6E1C4", UiTheme::Dark),
            "#5c5a4e"
        );
        assert_eq!(
            resolve_gutter_border(
                &colors(&[("editorGutter.background", "#161616")]),
                "#e6e4e1",
                UiTheme::Dark
            ),
            "#282828"
        );
        assert_eq!(
            resolve_gutter_border(
                &colors(&[("editorGutter.background", "#ebecef")]),
                "#1a1c20",
                UiTheme::Light
            ),
            "#cfd3db"
        );
    }

    #[test]
    fn softens_like_textmate() {
        assert_eq!(is_css_color_dark("#372725"), Some(true));
        assert_eq!(is_css_color_dark("#E6E1C4"), Some(false));
        assert_eq!(
            soften_css_color("#372725", 0.87).as_deref(),
            Some("#514341")
        );
        assert_eq!(soften_css_color("#E6E1C4", 0.4).as_deref(), Some("#5c5a4e"));
        assert_eq!(soften_css_color("#E6E1C4", 0.5).as_deref(), Some("#737162"));
        assert_eq!(
            resolve_line_number_foreground(&colors(&[]), "#E6E1C4"),
            "#737162"
        );
        assert_eq!(
            resolve_line_number_foreground(
                &colors(&[("editorLineNumber.foreground", "#888")]),
                "#E6E1C4"
            ),
            "#888"
        );
    }

    #[test]
    fn gutter_hint_prefers_opaque_theme_keys() {
        let key = "editorGutter.addedBackground";
        assert_eq!(
            resolve_gutter_hint_color(&colors(&[(key, "#1e754f")]), key).as_deref(),
            Some("#1e754f")
        );
        assert_eq!(
            resolve_gutter_hint_color(&colors(&[(key, "#2d353b00")]), key),
            None
        );
    }

    #[test]
    fn maps_classic_gutter_settings() {
        let p = parse_theme_value(
            &json!({"name":"Guttered","type":"dark","colors":{"editor.background":"#000000","editor.foreground":"#ffffff"},
                "gutterSettings":{"background":"#111111","foreground":"#aaaaaa","divider":"#333333"},
                "tokenColors":[{"settings":{"foreground":"#ffffff","background":"#000000"}}]}),
            "t",
        );
        assert_eq!(p.colors["editorGutter.background"], "#111111");
        assert_eq!(p.colors["editorLineNumber.foreground"], "#aaaaaa");
        assert_eq!(p.colors["gutter.divider"], "#333333");
    }

    #[test]
    fn lifts_classic_global_chrome() {
        let p = parse_theme_value(
            &json!({"name":"Birds","type":"dark","tokenColors":[{"settings":{
                "foreground":"#E6E1C4","background":"#372725","caret":"#E6E1C4",
                "selection":"#16120E","lineHighlight":"#1F1611","invisibles":"#42302D"}}]}),
            "t",
        );
        assert_eq!(p.colors["editor.selectionBackground"], "#16120E");
        assert_eq!(p.colors["editor.lineHighlightBackground"], "#1F1611");
        assert_eq!(p.colors["editorCursor.foreground"], "#E6E1C4");
        assert_eq!(p.colors["editorWhitespace.foreground"], "#42302D");
    }

    #[test]
    fn selection_and_line_highlight() {
        assert_eq!(
            with_css_alpha("#E6E1C4", 77.0).as_deref(),
            Some("#e6e1c44d")
        );
        assert_eq!(
            resolve_selection_background(
                &colors(&[("editor.selectionBackground", "#e07a5f47")]),
                "#e6e4e1",
                UiTheme::Dark
            ),
            "#e07a5f47"
        );
        assert_eq!(
            resolve_selection_background(&colors(&[]), "#E6E1C4", UiTheme::Dark),
            "#e6e1c44d"
        );
        assert_eq!(
            resolve_line_highlight_background(
                &colors(&[("editor.lineHighlightBackground", "#ffffff0f")]),
                "#0e0e0e",
                UiTheme::Dark
            ),
            "#ffffff0f"
        );
        assert_eq!(
            resolve_line_highlight_background(&colors(&[]), "#372725", UiTheme::Dark),
            soften_css_color("#372725", 0.95).unwrap()
        );
    }

    #[test]
    fn scope_selectors_and_roots() {
        assert_eq!(theme_scope_selectors(Some(&json!("source"))), ["source"]);
        assert_eq!(
            theme_scope_selectors(Some(&json!("source, text"))),
            ["source", "text"]
        );
        assert_eq!(
            theme_scope_selectors(Some(&json!(["text source", "meta.embedded"]))),
            ["text source", "meta.embedded"]
        );
        assert!(is_root_language_scope(Some(&json!("source"))));
        assert!(is_root_language_scope(Some(&json!("text"))));
        assert!(is_root_language_scope(Some(&json!("source, text"))));
        assert!(!is_root_language_scope(Some(&json!("source.ts"))));
        assert!(!is_root_language_scope(Some(&json!("text source"))));
        assert!(!is_root_language_scope(Some(&json!("meta.embedded"))));
        assert!(is_text_mate_text_scope("text.html"));
        assert!(!is_text_mate_text_scope("source.svelte"));
    }

    #[test]
    fn strips_root_backgrounds_into_pane_fills() {
        let p = parse_theme_value(
            &json!({"name":"Birds of Paradise","type":"dark",
                "colors":{"editor.background":"#372725","editor.foreground":"#E6E1C4"},
                "tokenColors":[
                    {"settings":{"foreground":"#E6E1C4","background":"#372725"}},
                    {"name":"Source","scope":"source","settings":{"foreground":"#E6E1C4","background":"#322323"}},
                    {"name":"Keyword","scope":"keyword","settings":{"foreground":"#EF5D32"}},
                    {"name":"Invalid","scope":"invalid","settings":{"foreground":"#FFFFFF","background":"#990000"}}]}),
            "t",
        );
        assert_eq!(p.root_backgrounds.source.as_deref(), Some("#322323"));
        assert_eq!(p.root_backgrounds.text, None);
        let src = p
            .raw_theme
            .settings
            .iter()
            .find(|s| s.scope == ThemeScope::One("source".into()))
            .unwrap();
        assert_eq!(src.background, None);
        assert_eq!(src.foreground.as_deref(), Some("#E6E1C4"));
        let inv = p
            .raw_theme
            .settings
            .iter()
            .find(|s| s.scope == ThemeScope::One("invalid".into()))
            .unwrap();
        assert_eq!(inv.background.as_deref(), Some("#990000"));
    }

    #[test]
    fn keeps_compound_selector_backgrounds() {
        let mut s = vec![TmRule {
            scope: Some(json!(["text source", "meta.embedded"])),
            settings: colors(&[("background", "#B1B3BA08")]),
            ..TmRule::default()
        }];
        assert_eq!(extract_root_scope_backgrounds(&s).source, None);
        strip_root_scope_token_backgrounds(&mut s);
        assert_eq!(s[0].settings["background"], "#B1B3BA08");
    }

    #[test]
    fn token_background_policy() {
        assert!((css_color_alpha("#B1B3BA08") - 8.0 / 255.0).abs() < 1e-9);
        assert!((css_color_alpha("#102622FA") - 250.0 / 255.0).abs() < 1e-9);
        assert_eq!(css_color_alpha("#990000"), 1.0);
        let doc = vec!["#372725".to_string(), "#322323".to_string()];
        assert!(!should_paint_token_background(Some("#322323"), &doc));
        assert!(!should_paint_token_background(Some("#372725"), &doc));
        assert!(!should_paint_token_background(
            Some("#B1B3BA08"),
            &["#090916".into()]
        ));
        assert!(should_paint_token_background(
            Some("#102622FA"),
            &["#090916".into()]
        ));
        assert!(should_paint_token_background(Some("#990000"), &doc));
        assert!(should_paint_token_background(
            Some("#803D0033"),
            &["#0D0D0DFA".into()]
        ));
        assert!(!should_paint_token_background(None, &doc));
    }

    #[test]
    fn unreadable_foregrounds_are_dropped() {
        assert!(!token_foreground_readable(
            Some("#00000080"),
            Some("#0D0D0D")
        ));
        assert!(token_foreground_readable(Some("#EEEEEE"), Some("#0D0D0D")));
        assert!(token_foreground_readable(Some("#FFFC80"), Some("#0D0D0D")));
        let c = composite_css_color("#803D0033", "#0D0D0D");
        assert!(c.len() == 7 && c.starts_with('#'));
    }

    #[test]
    fn paints_theme_hex_as_is() {
        let leftover = parse_theme_value(
            &json!({"name":"Monokai","type":"dark","colorSpaceName":"Generic RGB",
                "colors":{"editor.background":"#272822","editor.foreground":"#F8F8F2"},
                "tokenColors":[{"settings":{"foreground":"#F8F8F2","background":"#272822"}},{"scope":"keyword","settings":{"foreground":"#F92672"}}]}),
            "t",
        );
        assert_eq!(leftover.colors["editor.background"], "#272822");
        assert_eq!(leftover.colors["editor.foreground"], "#F8F8F2");
    }
}
