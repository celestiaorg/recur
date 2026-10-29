//! The mock's TextMate and terminal theme catalogs. Metadata comes from the
//! shipped `bundle.toml` files (so ids and paths cannot drift); file bodies
//! are read only by `ReadGrammar` / `ReadTheme`, so a missing file fails that
//! call. Read once per process.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Map, Value};

use super::js;

const MOCK_GRAMMAR_SCOPES: &[&str] = &["source.go"];
const MOCK_THEME_IDS: &[&str] = &["recur", "paper", "cool-glow"];

/// The repo's shipped `bundles/` directory.
pub(crate) fn bundles_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bundles")
}

fn read_shipped(parts: &[&str]) -> std::io::Result<String> {
    let mut p = bundles_root();
    for part in parts {
        p.push(part);
    }
    std::fs::read_to_string(p)
}

fn parse_inline_table(raw: &str) -> Value {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"([A-Za-z0-9_]+)\s*=\s*"([^"]*)""#).expect("inline table regex")
    });
    let mut out = Map::new();
    for c in re.captures_iter(raw) {
        out.insert(c[1].to_owned(), Value::String(c[2].to_owned()));
    }
    Value::Object(out)
}

/// The mock bridge's minimal TOML reader: every `[[name]]` table as a flat
/// record of string, inline-table, string-array, bool and integer values.
pub fn parse_toml_array_tables(src: &str, name: &str) -> Vec<Value> {
    let sep =
        Regex::new(&format!(r"(?:^|\n)\[\[{}\]\]\s*", regex::escape(name))).expect("table regex");
    let quoted = Regex::new(r#""([^"]*)""#).expect("string regex");
    let int = Regex::new(r"^-?[0-9]+$").expect("int regex");
    let mut tables = Vec::new();
    for part in sep.split(src).skip(1) {
        let body = part.split("\n[[").next().unwrap_or("");
        let mut rec = Map::new();
        for line in body.split('\n') {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let Some(eq) = trimmed.find('=') else {
                continue;
            };
            let key = trimmed[..eq].trim().to_owned();
            let raw = trimmed[eq + 1..].trim();
            let v = if raw.starts_with('{') {
                parse_inline_table(raw)
            } else if raw.starts_with('[') {
                Value::Array(
                    quoted
                        .captures_iter(raw)
                        .map(|c| Value::String(c[1].to_owned()))
                        .collect(),
                )
            } else if raw.starts_with('"') && raw.ends_with('"') {
                match serde_json::from_str::<Value>(raw) {
                    Ok(v) => v,
                    Err(_) => Value::String(raw.get(1..raw.len() - 1).unwrap_or("").to_owned()),
                }
            } else if raw == "true" || raw == "false" {
                Value::Bool(raw == "true")
            } else if int.is_match(raw) {
                js::num(js::number(Some(&Value::String(raw.to_owned()))))
            } else {
                Value::String(raw.to_owned())
            };
            rec.insert(key, v);
        }
        tables.push(Value::Object(rec));
    }
    tables
}

pub(crate) struct TmCatalog {
    /// `(public info, absolute path)`.
    pub grammars: Vec<(Value, PathBuf)>,
    pub themes: Vec<(Value, PathBuf)>,
}

fn s(v: &Value, key: &str) -> String {
    js::string(js::nullish(js::get(v, key), &Value::String(String::new())))
}

pub(crate) fn tm_catalog() -> &'static TmCatalog {
    static CAT: OnceLock<TmCatalog> = OnceLock::new();
    CAT.get_or_init(load_tm_catalog)
}

fn load_tm_catalog() -> TmCatalog {
    let mut grammars: Vec<(Value, PathBuf)> = Vec::new();
    if let Ok(src) = read_shipped(&["Languages", "bundle.toml"]) {
        for g in parse_toml_array_tables(&src, "grammar") {
            let scope_name = s(&g, "scopeName");
            if !MOCK_GRAMMAR_SCOPES.contains(&scope_name.as_str()) {
                continue;
            }
            let mut info = Map::new();
            info.insert("scopeName".into(), Value::String(scope_name));
            info.insert("id".into(), Value::String(s(&g, "id")));
            info.insert("path".into(), Value::String(s(&g, "path")));
            if js::truthy(js::get(&g, "language")) {
                info.insert("language".into(), Value::String(s(&g, "language")));
            }
            if let Some(Value::Array(types)) = js::get(&g, "fileTypes") {
                let types = types.iter().map(|t| Value::String(js::string(t))).collect();
                info.insert("fileTypes".into(), Value::Array(types));
            }
            info.insert("bundleName".into(), "Languages".into());
            info.insert("sourceRoot".into(), "default".into());
            let abs = bundles_root().join("Languages").join(s(&g, "path"));
            grammars.push((Value::Object(info), abs));
        }
        if let Ok(src) = read_shipped(&["LanguageData", "bundle.toml"]) {
            for d in parse_toml_array_tables(&src, "languageData") {
                let scope_name = s(&d, "scopeName");
                let Some((g, _)) = grammars
                    .iter_mut()
                    .find(|(g, _)| js::get_str(g, "scopeName") == scope_name)
                else {
                    continue;
                };
                for key in [
                    "lineComment",
                    "blockCommentStart",
                    "blockCommentEnd",
                    "increaseIndentPattern",
                    "decreaseIndentPattern",
                    "indentNextLinePattern",
                    "zeroIndentPattern",
                ] {
                    if let Some(Value::String(v)) = js::get(&d, key) {
                        js::set(g, key, Value::String(v.clone()));
                    }
                }
                if js::get(&d, "disableIndentCorrections") == Some(&Value::Bool(true)) {
                    js::set(g, "disableIndentCorrections", Value::Bool(true));
                }
                if let Some(Value::Array(b)) = js::get(&d, "closeBrackets") {
                    let b = b.iter().map(|x| Value::String(js::string(x))).collect();
                    js::set(g, "closeBrackets", Value::Array(b));
                }
            }
        }
    }
    let mut themes = Vec::new();
    for (dir, bundle_name) in [("Recur", "Recur"), ("Themes", "Themes")] {
        let Ok(src) = read_shipped(&[dir, "bundle.toml"]) else {
            continue;
        };
        for t in parse_toml_array_tables(&src, "theme") {
            let id = s(&t, "id");
            if !MOCK_THEME_IDS.contains(&id.as_str()) {
                continue;
            }
            let mut info = Map::new();
            info.insert("id".into(), Value::String(id));
            info.insert("name".into(), Value::String(s(&t, "name")));
            info.insert("path".into(), Value::String(s(&t, "path")));
            info.insert("uiTheme".into(), Value::String(s(&t, "uiTheme")));
            if let Some(order @ Value::Number(_)) = js::get(&t, "order") {
                info.insert("order".into(), order.clone());
            }
            let preview = match js::get(&t, "preview") {
                Some(p @ Value::Object(_)) => p.clone(),
                _ => Value::Object(Map::new()),
            };
            info.insert("preview".into(), preview);
            info.insert("bundleName".into(), bundle_name.into());
            info.insert("sourceRoot".into(), "default".into());
            let abs = bundles_root().join(dir).join(s(&t, "path"));
            themes.push((Value::Object(info), abs));
        }
    }
    TmCatalog { grammars, themes }
}

pub(crate) fn terminal_themes() -> &'static [Value] {
    static CAT: OnceLock<Vec<Value>> = OnceLock::new();
    CAT.get_or_init(load_terminal_themes)
}

fn load_terminal_themes() -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(src) = read_shipped(&["Terminal", "bundle.toml"]) else {
        return out;
    };
    for t in parse_toml_array_tables(&src, "terminalTheme") {
        let path = s(&t, "path");
        let Some(palette) = read_shipped(&["Terminal", &path])
            .ok()
            .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        else {
            continue;
        };
        let mut info = Map::new();
        info.insert("id".into(), Value::String(s(&t, "id")));
        info.insert("name".into(), Value::String(s(&t, "name")));
        info.insert("path".into(), Value::String(path));
        let ui = js::nullish(js::get(&t, "uiTheme"), &Value::String("dark".into())).clone();
        info.insert("uiTheme".into(), Value::String(js::string(&ui)));
        if let Some(order @ Value::Number(_)) = js::get(&t, "order") {
            info.insert("order".into(), order.clone());
        }
        info.insert("palette".into(), palette);
        info.insert("bundleName".into(), "Terminal".into());
        info.insert("sourceRoot".into(), "default".into());
        out.push(Value::Object(info));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn toml_tables() {
        let src = "[bundle]\nname = \"X\"\n\n[[theme]]\nid = \"a\" # x\norder = 10\npreview = { bg = \"#000\", fn = \"#fff\" }\n# c\ntags = [\"t1\", \"t2\"]\nflag = true\n[[other]]\nid = \"z\"\n[[theme]]\nid = \"b\"\nesc = \"a\\\"b\"\n";
        let t = parse_toml_array_tables(src, "theme");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0]["order"], json!(10));
        assert_eq!(t[0]["preview"], json!({"bg": "#000", "fn": "#fff"}));
        assert_eq!(t[0]["tags"], json!(["t1", "t2"]));
        assert_eq!(t[0]["flag"], json!(true));
        assert_eq!(t[0]["id"], json!("\"a\" # x"));
        assert!(t[0].get("other").is_none());
        assert_eq!(t[1]["esc"], json!("a\"b"));
    }

    #[test]
    fn shipped_catalogs() {
        let cat = tm_catalog();
        assert_eq!(cat.grammars.len(), 1);
        let (go, abs) = &cat.grammars[0];
        assert_eq!(go["scopeName"], json!("source.go"));
        assert_eq!(go["language"], json!("go"));
        assert_eq!(go["lineComment"], json!("//"));
        assert!(abs.is_file());
        let ids: Vec<&str> = cat
            .themes
            .iter()
            .map(|(t, _)| js::get_str(t, "id"))
            .collect();
        assert_eq!(ids, ["recur", "paper", "cool-glow"]);
        assert!(!terminal_themes().is_empty());
    }
}
