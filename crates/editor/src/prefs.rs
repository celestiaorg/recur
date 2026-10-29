//! Editor preferences, language ids for per-language indentation and the
//! font stack.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EDITOR_FONT_SIZES: [u32; 6] = [10, 11, 12, 13, 14, 16];
pub const EDITOR_TAB_SIZES: [usize; 3] = [2, 4, 8];
pub const EDITOR_LINE_HEIGHTS: [f32; 3] = [1.4, 1.55, 1.7];
pub const EDITOR_RULER_COLUMNS: [u32; 4] = [0, 80, 100, 120];
/// Soft cap on `max_open_files`.
pub const MAX_OPEN_FILES_CAP: u32 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditorIndent {
    #[serde(rename = "spaces-2")]
    Spaces2,
    #[serde(rename = "spaces-4")]
    Spaces4,
    #[serde(rename = "tabs")]
    Tabs,
}

impl EditorIndent {
    pub fn parse(s: &str) -> Option<EditorIndent> {
        match s {
            "spaces-2" => Some(EditorIndent::Spaces2),
            "spaces-4" => Some(EditorIndent::Spaces4),
            "tabs" => Some(EditorIndent::Tabs),
            _ => None,
        }
    }

    /// The `indentUnit` string.
    pub fn unit(self) -> &'static str {
        match self {
            EditorIndent::Tabs => "\t",
            EditorIndent::Spaces4 => "    ",
            EditorIndent::Spaces2 => "  ",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EditorIndent::Spaces2 => "2 spaces",
            EditorIndent::Spaces4 => "4 spaces",
            EditorIndent::Tabs => "Tabs",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorWhitespace {
    Off,
    Trailing,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorKeymap {
    Default,
    Vim,
    Emacs,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorPrefs {
    pub font_size: u32,
    pub font_face: String,
    pub line_wrapping: bool,
    pub tab_size: usize,
    pub indent: EditorIndent,
    pub indent_by_lang: std::collections::BTreeMap<String, EditorIndent>,
    pub whitespace: EditorWhitespace,
    pub special_chars: bool,
    pub line_numbers: bool,
    pub active_line: bool,
    pub fold_gutter: bool,
    pub diff_gutter: bool,
    pub bracket_matching: bool,
    pub scroll_past_end: bool,
    pub line_height: f32,
    pub cursor_blink: bool,
    pub thick_caret: bool,
    pub rectangular_select: bool,
    pub drop_cursor: bool,
    pub ruler_column: u32,
    pub keymap: EditorKeymap,
    pub max_open_files: u32,
}

impl Default for EditorPrefs {
    fn default() -> Self {
        EditorPrefs {
            font_size: 12,
            font_face: "JetBrains Mono".into(),
            line_wrapping: true,
            tab_size: 4,
            indent: EditorIndent::Spaces4,
            indent_by_lang: Default::default(),
            whitespace: EditorWhitespace::Off,
            special_chars: true,
            line_numbers: true,
            active_line: true,
            fold_gutter: true,
            diff_gutter: true,
            bracket_matching: true,
            scroll_past_end: false,
            line_height: 1.55,
            cursor_blink: true,
            thick_caret: false,
            rectangular_select: true,
            drop_cursor: true,
            ruler_column: 0,
            keymap: EditorKeymap::Default,
            max_open_files: 32,
        }
    }
}

/// Legacy `fontFace` keys.
pub fn legacy_font_face(key: &str) -> Option<&'static str> {
    Some(match key {
        "jetbrains" => "JetBrains Mono",
        "plex" => "IBM Plex Mono",
        "source" => "Source Code Pro",
        "system" => "System Mono",
        _ => return None,
    })
}

fn bool_pref(v: Option<&Value>, fallback: bool) -> bool {
    v.and_then(Value::as_bool).unwrap_or(fallback)
}

impl EditorPrefs {
    /// `normalizeEditorPrefs`: every field validated, unknowns to defaults.
    pub fn normalize(raw: &Value) -> EditorPrefs {
        let d = EditorPrefs::default();
        let get = |k: &str| raw.get(k);
        let num = |k: &str| get(k).and_then(Value::as_f64);
        let font_size = num("fontSize")
            .filter(|n| n.fract() == 0.0)
            .map(|n| n as u32)
            .filter(|n| EDITOR_FONT_SIZES.contains(n))
            .unwrap_or(d.font_size);
        let raw_face = get("fontFace")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let font_face = if raw_face.is_empty() {
            d.font_face.clone()
        } else {
            legacy_font_face(&raw_face).map_or(raw_face, str::to_string)
        };
        let tab_size = num("tabSize")
            .filter(|n| n.fract() == 0.0)
            .map(|n| n as usize)
            .filter(|n| EDITOR_TAB_SIZES.contains(n))
            .unwrap_or(d.tab_size);
        let indent = get("indent")
            .and_then(Value::as_str)
            .and_then(EditorIndent::parse)
            .unwrap_or(d.indent);
        let mut indent_by_lang = std::collections::BTreeMap::new();
        if let Some(obj) = get("indentByLang").and_then(Value::as_object) {
            for (k, v) in obj {
                if let Some(i) = v.as_str().and_then(EditorIndent::parse) {
                    indent_by_lang.insert(k.clone(), i);
                }
            }
        }
        let whitespace = match get("whitespace").and_then(Value::as_str) {
            Some("off") => EditorWhitespace::Off,
            Some("trailing") => EditorWhitespace::Trailing,
            Some("all") => EditorWhitespace::All,
            _ => d.whitespace,
        };
        let line_height = num("lineHeight")
            .map(|n| n as f32)
            .filter(|n| EDITOR_LINE_HEIGHTS.contains(n))
            .unwrap_or(d.line_height);
        let ruler_column = num("rulerColumn")
            .filter(|n| n.fract() == 0.0)
            .map(|n| n as u32)
            .filter(|n| EDITOR_RULER_COLUMNS.contains(n))
            .unwrap_or(d.ruler_column);
        let keymap = match get("keymap").and_then(Value::as_str) {
            Some("default") => EditorKeymap::Default,
            Some("vim") => EditorKeymap::Vim,
            Some("emacs") => EditorKeymap::Emacs,
            _ => d.keymap,
        };
        let max_open_files = match num("maxOpenFiles").filter(|n| n.is_finite()) {
            None => d.max_open_files,
            Some(n) if n.floor() <= 0.0 => 0,
            Some(n) => (n.floor() as u32).min(MAX_OPEN_FILES_CAP),
        };
        EditorPrefs {
            font_size,
            font_face,
            line_wrapping: bool_pref(get("lineWrapping"), d.line_wrapping),
            tab_size,
            indent,
            indent_by_lang,
            whitespace,
            special_chars: bool_pref(get("specialChars"), d.special_chars),
            line_numbers: bool_pref(get("lineNumbers"), d.line_numbers),
            active_line: bool_pref(get("activeLine"), d.active_line),
            fold_gutter: bool_pref(get("foldGutter"), d.fold_gutter),
            diff_gutter: bool_pref(get("diffGutter"), d.diff_gutter),
            bracket_matching: bool_pref(get("bracketMatching"), d.bracket_matching),
            scroll_past_end: bool_pref(get("scrollPastEnd"), d.scroll_past_end),
            line_height,
            cursor_blink: bool_pref(get("cursorBlink"), d.cursor_blink),
            thick_caret: bool_pref(get("thickCaret"), d.thick_caret),
            rectangular_select: bool_pref(get("rectangularSelect"), d.rectangular_select),
            drop_cursor: bool_pref(get("dropCursor"), d.drop_cursor),
            ruler_column,
            keymap,
            max_open_files,
        }
    }

    /// `resolveEditorIndent`: override, then the language convention, then
    /// the global indent.
    pub fn resolve_indent(&self, lang_id: &str) -> EditorIndent {
        self.indent_by_lang
            .get(lang_id)
            .copied()
            .or_else(|| language_indent_default(lang_id))
            .unwrap_or(self.indent)
    }

    /// Caret width and its left nudge (2px / -1px, thick 3px / -1.5px).
    pub fn caret_metrics(&self) -> (f32, f32) {
        if self.thick_caret {
            (3.0, -1.5)
        } else {
            (2.0, -1.0)
        }
    }

    /// Cursor blink period (`drawSelection({ cursorBlinkRate })`); 0 = solid.
    pub fn cursor_blink_rate_ms(&self) -> u64 {
        if self.cursor_blink { 1200 } else { 0 }
    }
}

/// `SYSTEM_MONO_STACK` as font family names.
pub const SYSTEM_MONO_FAMILIES: [&str; 5] = ["SF Mono", "Menlo", "Monaco", "Consolas", "monospace"];

/// `monoFontStack`: the chosen family, then the system monospace stack.
/// "System Mono" is the system stack alone.
pub fn font_stack(face: &str) -> Vec<String> {
    let resolved = legacy_font_face(face).unwrap_or(face);
    let mut out = Vec::new();
    if resolved != "System Mono" {
        out.push(resolved.to_string());
    }
    out.extend(SYSTEM_MONO_FAMILIES.iter().map(|s| s.to_string()));
    out
}

/// Conventional indent per language (`LANGUAGE_INDENT_DEFAULTS`).
pub fn language_indent_default(lang_id: &str) -> Option<EditorIndent> {
    use EditorIndent::*;
    Some(match lang_id {
        "go" => Tabs,
        "python" | "rust" | "java" | "c" | "cpp" | "php" => Spaces4,
        "typescript" | "javascript" | "json" | "yaml" | "markdown" | "html" | "css" | "shell"
        | "svelte" | "vue" | "ruby" | "sql" | "toml" => Spaces2,
        _ => return None,
    })
}

/// Languages offered for per-language indent in Settings: id, label, hint.
pub const EDITOR_INDENT_LANGS: &[(&str, &str, Option<&str>)] = &[
    ("go", "Go", Some("gofmt")),
    ("python", "Python", Some("PEP 8")),
    ("typescript", "TypeScript", None),
    ("javascript", "JavaScript", None),
    ("json", "JSON", None),
    ("yaml", "YAML", None),
    ("markdown", "Markdown", None),
    ("rust", "Rust", Some("rustfmt")),
    ("html", "HTML", None),
    ("css", "CSS", None),
    ("shell", "Shell", None),
    ("svelte", "Svelte", None),
    ("vue", "Vue", None),
];

fn path_parts(path: &str) -> (String, String) {
    let base = path.rsplit('/').next().unwrap_or(path);
    let lower = base.to_lowercase();
    let ext = lower
        .rfind('.')
        .map_or(String::new(), |i| lower[i..].to_string());
    (lower, ext)
}

/// `languageIdForPath`: the prefs language id for a file.
pub fn language_id_for_path(path: &str) -> &'static str {
    let (lower, ext) = path_parts(path);
    match lower.as_str() {
        "dockerfile" | "makefile" | "gnumakefile" => return "shell",
        "go.mod" | "go.sum" => return "go",
        "cargo.lock" | "poetry.lock" | "uv.lock" => return "toml",
        "nginx.conf" => return "other",
        _ => {}
    }
    match ext.as_str() {
        ".ts" | ".mts" | ".cts" | ".tsx" => "typescript",
        ".js" | ".mjs" | ".cjs" | ".jsx" => "javascript",
        ".vue" => "vue",
        ".svelte" => "svelte",
        ".html" | ".htm" => "html",
        ".css" | ".scss" | ".sass" | ".less" => "css",
        ".go" => "go",
        ".py" | ".pyi" | ".pyw" => "python",
        ".rs" => "rust",
        ".java" => "java",
        ".c" | ".h" => "c",
        ".cc" | ".cpp" | ".cxx" | ".hpp" | ".hxx" | ".hh" => "cpp",
        ".rb" | ".rake" | ".gemspec" => "ruby",
        ".php" => "php",
        ".json" | ".jsonc" | ".json5" => "json",
        ".yaml" | ".yml" => "yaml",
        ".toml" => "toml",
        ".md" | ".mdx" | ".markdown" => "markdown",
        ".sql" => "sql",
        ".sh" | ".bash" | ".zsh" | ".ksh" | ".fish" | ".ps1" | ".psm1" | ".dockerfile" => "shell",
        _ => "other",
    }
}

/// `wantsDiffLineChrome`: `.diff` / `.patch` files get unified-diff line
/// washes.
pub fn wants_diff_line_chrome(path: &str) -> bool {
    let (lower, ext) = path_parts(path);
    ext == ".diff" || ext == ".patch" || lower.ends_with(".diff") || lower.ends_with(".patch")
}

/// The line wash for a unified-diff line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    /// `cm-diff-ins`: rgba(63, 185, 80, .14).
    Ins,
    /// `cm-diff-del`: rgba(224, 101, 95, .14).
    Del,
    /// `cm-diff-meta`: rgba(210, 168, 255, .10).
    Meta,
}

pub fn diff_line_kind(text: &str) -> Option<DiffLineKind> {
    if text.starts_with('+') {
        Some(DiffLineKind::Ins)
    } else if text.starts_with('-') {
        Some(DiffLineKind::Del)
    } else if text.starts_with("@@") {
        Some(DiffLineKind::Meta)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_resolves() {
        let p = EditorPrefs::normalize(&serde_json::json!({
            "fontSize": 15, "fontFace": "plex", "tabSize": 2, "indent": "tabs",
            "indentByLang": {"go": "spaces-2", "bad": "x"}, "lineHeight": 1.7,
            "keymap": "vim", "maxOpenFiles": 500, "whitespace": "all"
        }));
        assert_eq!(p.font_size, 12);
        assert_eq!(p.font_face, "IBM Plex Mono");
        assert_eq!(p.tab_size, 2);
        assert_eq!(p.max_open_files, 200);
        assert_eq!(p.line_height, 1.7);
        assert_eq!(p.keymap, EditorKeymap::Vim);
        assert_eq!(p.resolve_indent("go"), EditorIndent::Spaces2);
        assert_eq!(p.resolve_indent("python"), EditorIndent::Spaces4);
        assert_eq!(p.resolve_indent("other"), EditorIndent::Tabs);
        assert_eq!(
            EditorPrefs::normalize(&serde_json::json!({"maxOpenFiles": -3})).max_open_files,
            0
        );
        assert_eq!(font_stack("system")[0], "SF Mono");
        assert_eq!(font_stack("Fira Code")[0], "Fira Code");
    }

    #[test]
    fn language_ids() {
        assert_eq!(language_id_for_path("a/b/main.go"), "go");
        assert_eq!(language_id_for_path("Dockerfile"), "shell");
        assert_eq!(language_id_for_path("x/Cargo.lock"), "toml");
        assert_eq!(language_id_for_path("x.TSX"), "typescript");
        assert_eq!(language_id_for_path("README"), "other");
        assert!(wants_diff_line_chrome("fix.patch"));
        assert_eq!(diff_line_kind("@@ -1 +1 @@"), Some(DiffLineKind::Meta));
    }
}
