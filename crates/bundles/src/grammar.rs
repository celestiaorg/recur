//! `[[grammar]]`, `[[theme]]` and `[[terminalTheme]]` entries.

use std::collections::{BTreeMap, HashSet};

use recur_domain::path;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manifest::{BundleMeta, valid_bundle_id};
use crate::{Error, err};

/// UI theme kinds for `[[theme]].uiTheme`.
pub const UI_THEME_DARK: &str = "dark";
pub const UI_THEME_LIGHT: &str = "light";

/// One `[[grammar]]` entry: a TextMate language grammar contributed by a
/// bundle (JSON `.tmLanguage.json` preferred; plist `.tmLanguage` accepted).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grammar {
    pub id: String,
    #[serde(rename = "scopeName")]
    pub scope_name: String,
    pub path: String,
    pub language: String,
    #[serde(rename = "fileTypes")]
    pub file_types: Vec<String>,
    #[serde(rename = "firstLineMatch")]
    pub first_line_match: String,
    #[serde(rename = "injectTo")]
    pub inject_to: Vec<String>,
    #[serde(
        rename = "embeddedLanguages",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub embedded_languages: BTreeMap<String, String>,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    /// "user" | "default"
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
    #[serde(skip)]
    pub abs_path: String,
}

/// Optional Settings picker swatches for a `[[theme]]`: bg/fg plus sample
/// `fn` / `main` / `"hi"` colours.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemePreview {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bg: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub fg: String,
    #[serde(rename = "fn", skip_serializing_if = "String::is_empty")]
    pub func: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub string: String,
}

/// One `[[theme]]` entry: a TextMate / VS Code colour theme JSON. `id` is
/// the Settings preference value (e.g. "cool-glow", "recur").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub path: String,
    /// light | dark
    #[serde(rename = "uiTheme")]
    pub ui_theme: String,
    /// Optional Settings display order (lower first). Zero sorts after
    /// explicit orders.
    #[serde(skip_serializing_if = "crate::manifest::is_zero")]
    pub order: i64,
    pub preview: ThemePreview,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
    #[serde(skip)]
    pub abs_path: String,
}

/// An xterm.js `ITheme` JSON object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TerminalPalette {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub background: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub foreground: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cursor: String,
    #[serde(rename = "cursorAccent", skip_serializing_if = "String::is_empty")]
    pub cursor_accent: String,
    #[serde(
        rename = "selectionBackground",
        skip_serializing_if = "String::is_empty"
    )]
    pub selection_background: String,
    #[serde(
        rename = "selectionForeground",
        skip_serializing_if = "String::is_empty"
    )]
    pub selection_foreground: String,
    #[serde(
        rename = "selectionInactiveBackground",
        skip_serializing_if = "String::is_empty"
    )]
    pub selection_inactive_background: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub black: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub red: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub green: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub yellow: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub blue: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub magenta: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cyan: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub white: String,
    #[serde(rename = "brightBlack", skip_serializing_if = "String::is_empty")]
    pub bright_black: String,
    #[serde(rename = "brightRed", skip_serializing_if = "String::is_empty")]
    pub bright_red: String,
    #[serde(rename = "brightGreen", skip_serializing_if = "String::is_empty")]
    pub bright_green: String,
    #[serde(rename = "brightYellow", skip_serializing_if = "String::is_empty")]
    pub bright_yellow: String,
    #[serde(rename = "brightBlue", skip_serializing_if = "String::is_empty")]
    pub bright_blue: String,
    #[serde(rename = "brightMagenta", skip_serializing_if = "String::is_empty")]
    pub bright_magenta: String,
    #[serde(rename = "brightCyan", skip_serializing_if = "String::is_empty")]
    pub bright_cyan: String,
    #[serde(rename = "brightWhite", skip_serializing_if = "String::is_empty")]
    pub bright_white: String,
}

impl TerminalPalette {
    fn color_fields(&mut self) -> [(&'static str, &mut String); 23] {
        [
            ("background", &mut self.background),
            ("foreground", &mut self.foreground),
            ("cursor", &mut self.cursor),
            ("cursorAccent", &mut self.cursor_accent),
            ("selectionBackground", &mut self.selection_background),
            ("selectionForeground", &mut self.selection_foreground),
            (
                "selectionInactiveBackground",
                &mut self.selection_inactive_background,
            ),
            ("black", &mut self.black),
            ("red", &mut self.red),
            ("green", &mut self.green),
            ("yellow", &mut self.yellow),
            ("blue", &mut self.blue),
            ("magenta", &mut self.magenta),
            ("cyan", &mut self.cyan),
            ("white", &mut self.white),
            ("brightBlack", &mut self.bright_black),
            ("brightRed", &mut self.bright_red),
            ("brightGreen", &mut self.bright_green),
            ("brightYellow", &mut self.bright_yellow),
            ("brightBlue", &mut self.bright_blue),
            ("brightMagenta", &mut self.bright_magenta),
            ("brightCyan", &mut self.bright_cyan),
            ("brightWhite", &mut self.bright_white),
        ]
    }

    /// Decodes the palette struct: unknown keys are ignored, field names match
    /// case-insensitively, `null` leaves a field empty, and any other
    /// non-string value is an error.
    fn from_json(body: &[u8]) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        let Value::Object(obj) = value else {
            return Err(format!(
                "json: cannot unmarshal {} into Go value of type bundles.TerminalPalette",
                go_json_kind(&value)
            ));
        };
        let mut pal = TerminalPalette::default();
        for (name, dst) in pal.color_fields() {
            match go_json_field(&obj, name) {
                None | Some(Value::Null) => {}
                Some(Value::String(s)) => *dst = s.clone(),
                Some(other) => {
                    return Err(format!(
                        "json: cannot unmarshal {} into Go struct field TerminalPalette.{name} of type string",
                        go_json_kind(other)
                    ));
                }
            }
        }
        Ok(pal)
    }
}

/// JSON field lookup: exact key first, then case-insensitive.
pub(crate) fn go_json_field<'a>(
    obj: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Option<&'a Value> {
    obj.get(name).or_else(|| {
        let lower = name.to_lowercase();
        obj.iter()
            .find(|(k, _)| k.to_lowercase() == lower)
            .map(|(_, v)| v)
    })
}

/// The value kind names decode errors use.
pub(crate) fn go_json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// One `[[terminalTheme]]` entry: an xterm.js `ITheme` JSON file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalTheme {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(rename = "uiTheme")]
    pub ui_theme: String,
    #[serde(skip_serializing_if = "crate::manifest::is_zero")]
    pub order: i64,

    #[serde(skip_deserializing)]
    pub palette: TerminalPalette,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
    #[serde(skip)]
    pub abs_path: String,
}

/// Joins `rel` to `bundle_dir`, rejecting escapes and absolute paths.
pub(crate) fn resolve_bundle_rel_path(bundle_dir: &str, rel: &str) -> Result<String, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("path is required".to_string());
    }
    if path::is_abs(rel) || rel.contains("..") {
        return Err(format!(
            "path {:?} must be relative and must not contain ..",
            rel
        ));
    }
    let abs = path::clean(&path::join(&[bundle_dir, rel]));
    let base = path::clean(bundle_dir);
    match path::rel(&base, &abs) {
        Some(out) if out != ".." && !out.starts_with("../") => Ok(abs),
        _ => Err(format!("path {:?} escapes bundle directory", rel)),
    }
}

pub(crate) fn parse_grammars(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<Grammar>,
) -> Result<Vec<Grammar>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    let mut seen_scope = HashSet::new();
    for (i, mut g) in raw.into_iter().enumerate() {
        let scope = g.scope_name.trim().to_string();
        if scope.is_empty() {
            return Err(err!("grammar[{i}]: scopeName is required"));
        }
        let qs = format!("{scope:?}");
        if !seen_scope.insert(scope.clone()) {
            return Err(err!("grammar: duplicate scopeName {qs} in bundle"));
        }
        g.scope_name = scope;

        let id = g.id.trim().to_string();
        if id.is_empty() {
            return Err(err!("grammar {qs}: id is required"));
        }
        if !valid_bundle_id(&id) {
            return Err(err!("grammar {qs}: id is not a safe id"));
        }
        g.id = id;

        let abs =
            resolve_bundle_rel_path(bundle_dir, &g.path).map_err(|e| err!("grammar {qs}: {e}"))?;
        std::fs::metadata(&abs).map_err(|e| err!("grammar {qs}: read path {:?}: {e}", g.path))?;
        g.abs_path = abs;
        g.path = g.path.trim().to_string();

        for ft in &mut g.file_types {
            *ft = ft.trim().to_string();
        }
        for inj in &mut g.inject_to {
            *inj = inj.trim().to_string();
        }
        g.language = g.language.trim().to_string();
        g.first_line_match = g.first_line_match.trim().to_string();
        g.bundle_name = meta.name.clone();
        g.bundle_id = meta.id.clone();
        g.bundle_dir = bundle_dir.to_string();
        out.push(g);
    }
    Ok(out)
}

pub(crate) fn parse_themes(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<Theme>,
) -> Result<Vec<Theme>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    let mut seen_id = HashSet::new();
    for (i, mut t) in raw.into_iter().enumerate() {
        let id = t.id.trim().to_string();
        if id.is_empty() {
            return Err(err!("theme[{i}]: id is required"));
        }
        if !valid_bundle_id(&id) {
            return Err(err!("theme[{i}]: id is not a safe id"));
        }
        let qid = format!("{id:?}");
        if !seen_id.insert(id.clone()) {
            return Err(err!("theme: duplicate id {qid} in bundle"));
        }
        t.id = id;

        let name = t.name.trim().to_string();
        if name.is_empty() {
            return Err(err!("theme {qid}: name is required"));
        }
        t.name = name;

        let mut ui = t.ui_theme.trim().to_string();
        if ui.is_empty() {
            ui = UI_THEME_DARK.to_string();
        }
        if ui != UI_THEME_DARK && ui != UI_THEME_LIGHT {
            return Err(err!("theme {qid}: uiTheme must be light or dark"));
        }
        t.ui_theme = ui;

        let p = &mut t.preview;
        for (name, dst) in [
            ("preview.bg", &mut p.bg),
            ("preview.fg", &mut p.fg),
            ("preview.fn", &mut p.func),
            ("preview.name", &mut p.name),
            ("preview.string", &mut p.string),
        ] {
            *dst = normalize_preview_color(dst).map_err(|e| err!("theme {qid}: {name}: {e}"))?;
        }

        let abs =
            resolve_bundle_rel_path(bundle_dir, &t.path).map_err(|e| err!("theme {qid}: {e}"))?;
        std::fs::metadata(&abs).map_err(|e| err!("theme {qid}: read path {:?}: {e}", t.path))?;
        t.abs_path = abs;
        t.path = t.path.trim().to_string();
        t.bundle_name = meta.name.clone();
        t.bundle_id = meta.id.clone();
        t.bundle_dir = bundle_dir.to_string();
        out.push(t);
    }
    Ok(out)
}

pub(crate) fn parse_terminal_themes(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<TerminalTheme>,
) -> Result<Vec<TerminalTheme>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    let mut seen_id = HashSet::new();
    for (i, mut t) in raw.into_iter().enumerate() {
        let id = t.id.trim().to_string();
        if id.is_empty() {
            return Err(err!("terminalTheme[{i}]: id is required"));
        }
        if !valid_bundle_id(&id) {
            return Err(err!("terminalTheme[{i}]: id is not a safe id"));
        }
        let qid = format!("{id:?}");
        if !seen_id.insert(id.clone()) {
            return Err(err!("terminalTheme: duplicate id {qid} in bundle"));
        }
        t.id = id;

        let name = t.name.trim().to_string();
        if name.is_empty() {
            return Err(err!("terminalTheme {qid}: name is required"));
        }
        t.name = name;

        let mut ui = t.ui_theme.trim().to_string();
        if ui.is_empty() {
            ui = UI_THEME_DARK.to_string();
        }
        if ui != UI_THEME_DARK && ui != UI_THEME_LIGHT {
            return Err(err!("terminalTheme {qid}: uiTheme must be light or dark"));
        }
        t.ui_theme = ui;

        let abs = resolve_bundle_rel_path(bundle_dir, &t.path)
            .map_err(|e| err!("terminalTheme {qid}: {e}"))?;
        let body = std::fs::read(&abs)
            .map_err(|e| err!("terminalTheme {qid}: read path {:?}: {e}", t.path))?;
        let mut pal = TerminalPalette::from_json(&body)
            .map_err(|e| err!("terminalTheme {qid}: parse {:?}: {e}", t.path))?;
        for (name, dst) in pal.color_fields() {
            *dst =
                normalize_theme_color(dst).map_err(|e| err!("terminalTheme {qid}: {name}: {e}"))?;
        }
        if pal.background.is_empty() || pal.foreground.is_empty() {
            return Err(err!(
                "terminalTheme {qid}: background and foreground are required"
            ));
        }
        t.palette = pal;
        t.abs_path = abs;
        t.path = t.path.trim().to_string();
        t.bundle_name = meta.name.clone();
        t.bundle_id = meta.id.clone();
        t.bundle_dir = bundle_dir.to_string();
        out.push(t);
    }
    Ok(out)
}

/// Accepts empty or `#rgb` / `#rrggbb` / `#rrggbbaa`.
fn normalize_preview_color(s: &str) -> Result<String, &'static str> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(String::new());
    }
    let Some(hex) = s.strip_prefix('#') else {
        return Err("must be a #hex colour");
    };
    if !matches!(hex.len(), 3 | 6 | 8) {
        return Err("must be #rgb, #rrggbb, or #rrggbbaa");
    }
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("invalid hex digit");
    }
    Ok(s.to_string())
}

/// Accepts empty, `#hex`, or rgb/hsl CSS colours.
fn normalize_theme_color(s: &str) -> Result<String, &'static str> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(String::new());
    }
    if s.starts_with('#') {
        return normalize_preview_color(s);
    }
    let lower = s.to_lowercase();
    if ["rgb(", "rgba(", "hsl(", "hsla("]
        .iter()
        .any(|p| lower.starts_with(p))
    {
        return Ok(s.to_string());
    }
    Err("must be a #hex or rgb/hsl colour")
}

/// The grammar with the given scopeName.
pub fn find_grammar_by_scope<'a>(grammars: &'a [Grammar], scope: &str) -> Option<&'a Grammar> {
    let scope = scope.trim();
    grammars.iter().find(|g| g.scope_name == scope)
}

/// The grammar with the given id.
pub fn find_grammar_by_id<'a>(grammars: &'a [Grammar], id: &str) -> Option<&'a Grammar> {
    let id = id.trim();
    grammars.iter().find(|g| g.id == id)
}

/// The theme with the given id.
pub fn find_theme_by_id<'a>(themes: &'a [Theme], id: &str) -> Option<&'a Theme> {
    let id = id.trim();
    themes.iter().find(|t| t.id == id)
}

/// The terminal theme with the given id.
pub fn find_terminal_theme_by_id<'a>(
    themes: &'a [TerminalTheme],
    id: &str,
) -> Option<&'a TerminalTheme> {
    let id = id.trim();
    themes.iter().find(|t| t.id == id)
}

/// Whether `path` looks like a TextMate grammar file.
pub fn grammar_convention_ok(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".tmlanguage.json")
        || lower.ends_with(".tmlanguage")
        || lower.ends_with(".json-tmlanguage")
}

/// Whether `path` looks like a TextMate theme JSON file.
pub fn theme_convention_ok(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".tmtheme.json") || lower.ends_with(".tmtheme")
}

/// Whether `path` looks like an xterm `ITheme` JSON file.
pub fn terminal_theme_convention_ok(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".json") && !lower.ends_with(".tmtheme.json")
}
