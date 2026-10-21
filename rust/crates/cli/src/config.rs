//! Viper-equivalent config for the CLI (`internal/cli/config.go`).
//!
//! `bundles-dir` precedence: the `--bundles-dir` flag when set, then a
//! non-empty `$RECUR_BUNDLES_DIR`, then `bundles-dir` in the first config
//! file Viper finds in `~/.recur` (parsed as YAML), then
//! `~/.recur/bundles`.

use recur_bundles::go;
use yaml_rust2::{Yaml, YamlLoader};

use crate::paths;

/// Viper's `SupportedExts`, in its search order.
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

/// `initConfig`: fails only when the home dir is unknown.
pub fn init() -> Result<(), String> {
    paths::user_home_dir().map(|_| ())
}

/// `viper.GetString("bundles-dir")`. `flag` is the flag value when changed.
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
    if let Some(v) = config_file_value(&go::join(&[&home, ".recur"]), "bundles-dir") {
        return Ok(v);
    }
    Ok(go::join(&[&home, ".recur", "bundles"]))
}

/// `bundlesDir()`: the setting (created), or the store default when blank.
pub fn bundles_dir(flag: Option<&str>) -> Result<String, String> {
    let d = bundles_dir_setting(flag)?;
    let d = d.trim();
    if !d.is_empty() {
        std::fs::create_dir_all(d).map_err(|e| go::path_err("mkdir", d, &e))?;
        return Ok(d.to_string());
    }
    paths::user_bundles_dir()
}

/// Viper's config search in `dir` for `config.<ext>` then `config`.
fn find_config_file(dir: &str) -> Option<String> {
    SUPPORTED_EXTS
        .iter()
        .map(|ext| go::join(&[dir, &format!("config.{ext}")]))
        .chain(std::iter::once(go::join(&[dir, "config"])))
        .find(|p| std::fs::metadata(p).is_ok_and(|m| !m.is_dir()))
}

fn config_file_value(dir: &str, key: &str) -> Option<String> {
    let file = find_config_file(dir)?;
    let text = std::fs::read_to_string(&file).ok()?;
    let docs = YamlLoader::load_from_str(&text).ok()?;
    let Some(Yaml::Hash(map)) = docs.into_iter().next() else {
        return None;
    };
    // Viper lowercases config keys; the last case-variant wins.
    let mut found = None;
    for (k, v) in map {
        if let Yaml::String(k) = k
            && k.to_lowercase() == key
        {
            found = Some(v);
        }
    }
    yaml_to_string(&found?)
}

/// `cast.ToString` for YAML scalars; other kinds read as unset.
fn yaml_to_string(v: &Yaml) -> Option<String> {
    Some(match v {
        Yaml::String(s) => s.clone(),
        Yaml::Integer(i) => i.to_string(),
        Yaml::Boolean(b) => b.to_string(),
        Yaml::Real(r) => match r.parse::<f64>() {
            Ok(f) if f.is_nan() => "NaN".to_string(),
            Ok(f) if f.is_infinite() => if f > 0.0 { "+Inf" } else { "-Inf" }.to_string(),
            Ok(f) => f.to_string(),
            Err(_) => r.clone(),
        },
        Yaml::Null => String::new(),
        _ => return None,
    })
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
        // config.json is found first and parsed as YAML.
        std::fs::write(format!("{d}/config.json"), "{\"bundles-dir\": 42}").unwrap();
        assert_eq!(config_file_value(&d, "bundles-dir").as_deref(), Some("42"));
        std::fs::write(format!("{d}/config.json"), "{not yaml").unwrap();
        assert_eq!(config_file_value(&d, "bundles-dir"), None);
    }
}
