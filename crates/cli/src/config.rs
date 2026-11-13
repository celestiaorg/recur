//! CLI config.
//!
//! `bundles-dir` precedence: the `--bundles-dir` flag when set, then a
//! non-empty `$RECUR_BUNDLES_DIR`, then `bundles-dir` in the first config
//! file found in `~/.recur` (a top-level YAML `key: value` line), then
//! `~/.recur/bundles`.

use recur_domain::path;

use crate::paths;

/// Config file extensions, in search order.
const SUPPORTED_EXTS: [&str; 12] = [
    "json",
    "toml",
    "yaml",
    "yml",
    "properties",
    "props",
    "prop",
    "hcl",
    "tfvars",
    "dotenv",
    "env",
    "ini",
];

/// Fails only when the home dir is unknown.
pub fn init() -> Result<(), String> {
    paths::user_home_dir().map(|_| ())
}

/// The `bundles-dir` setting. `flag` is the `--bundles-dir` value when set.
pub fn bundles_dir_setting(flag: Option<&str>) -> Result<String, String> {
    if let Some(v) = flag {
        return Ok(v.to_string());
    }
    if let Ok(v) = std::env::var("RECUR_BUNDLES_DIR")
        && !v.is_empty()
    {
        return Ok(v);
    }
    let home = paths::user_home_dir()?;
    if let Some(v) = config_file_value(&path::join(&[&home, ".recur"]), "bundles-dir") {
        return Ok(v);
    }
    Ok(path::join(&[&home, ".recur", "bundles"]))
}

/// The setting (created), or the store default when blank.
pub fn bundles_dir(flag: Option<&str>) -> Result<String, String> {
    let d = bundles_dir_setting(flag)?;
    let d = d.trim();
    if !d.is_empty() {
        std::fs::create_dir_all(d).map_err(|e| format!("{d}: {e}"))?;
        return Ok(d.to_string());
    }
    paths::user_bundles_dir()
}

/// The first `config.<ext>`, then `config`, in `dir`.
fn find_config_file(dir: &str) -> Option<String> {
    SUPPORTED_EXTS
        .iter()
        .map(|ext| path::join(&[dir, &format!("config.{ext}")]))
        .chain(std::iter::once(path::join(&[dir, "config"])))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| !m.is_dir()))
}

fn config_file_value(dir: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(find_config_file(dir)?).ok()?;
    // simplify: top-level `key: scalar` lines only; nested, flow and multi-line YAML read as
    // unset. Swap in a YAML parser if the config grows beyond flat keys.
    let mut found = None;
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#']) {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        // Keys match case-insensitively; the last case-variant wins.
        if unquote(k.trim()).to_lowercase() == key {
            found = Some(scalar(v.trim()));
        }
    }
    found
}

fn unquote(s: &str) -> &str {
    for q in ['"', '\''] {
        if let Some(rest) = s.strip_prefix(q) {
            return rest.split(q).next().unwrap_or(rest);
        }
    }
    s
}

/// A YAML scalar as a string: quotes and trailing comments stripped, null as empty.
fn scalar(v: &str) -> String {
    if v.starts_with(['"', '\'']) {
        return unquote(v).to_string();
    }
    let v = v.split(" #").next().unwrap_or(v).trim();
    match v {
        "~" | "null" | "Null" | "NULL" => String::new(),
        _ => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_search_and_values() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_string_lossy().into_owned();
        assert_eq!(config_file_value(&d, "bundles-dir"), None);
        std::fs::write(format!("{d}/config.yml"), "Bundles-Dir: /from/yml\n").unwrap();
        assert_eq!(
            config_file_value(&d, "bundles-dir").as_deref(),
            Some("/from/yml")
        );
        std::fs::write(
            format!("{d}/config.yml"),
            "# c\nbundles-dir: /a\nnested:\n  bundles-dir: /no\nBUNDLES-DIR: '/b c' # x\n",
        )
        .unwrap();
        assert_eq!(
            config_file_value(&d, "bundles-dir").as_deref(),
            Some("/b c")
        );
        std::fs::write(format!("{d}/config.yml"), "bundles-dir: 42 # n\n").unwrap();
        assert_eq!(config_file_value(&d, "bundles-dir").as_deref(), Some("42"));
        std::fs::write(format!("{d}/config.yml"), "bundles-dir: ~\n").unwrap();
        assert_eq!(config_file_value(&d, "bundles-dir").as_deref(), Some(""));
        // config.json is found first and read the same way.
        std::fs::write(format!("{d}/config.json"), "{not yaml").unwrap();
        assert_eq!(config_file_value(&d, "bundles-dir"), None);
    }
}
