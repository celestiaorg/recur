//! `[[setting]]`: persisted values (often secrets) shown in Settings →
//! Integrations and injected into that bundle's shebang env as
//! `RECUR_SETTING_<NAME>` plus `RECUR_BUNDLE_SETTINGS` JSON. TypeScript
//! Workers never see these values.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::manifest::{BundleMeta, PROMPT_ARG_NAME_PATTERN, PROMPT_ARG_NAME_RE, valid_bundle_id};
use crate::{Error, err};

/// Per-bundle ceiling for `[[setting]]` entries.
pub const SETTING_MAX: usize = 16;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Setting {
    pub id: String,
    /// Machine name → `RECUR_SETTING_*`.
    pub name: String,
    pub label: String,
    pub description: String,
    pub secret: bool,
    pub placeholder: String,
    pub url: String,
    #[serde(rename = "urlLabel")]
    pub url_label: String,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
}

/// Shebang env name for a setting (`api_key` → `RECUR_SETTING_API_KEY`).
pub fn setting_env_name(name: &str) -> String {
    format!("RECUR_SETTING_{}", name.trim().to_uppercase())
}

pub(crate) fn parse_settings(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<Setting>,
) -> Result<Vec<Setting>, Error> {
    if raw.len() > SETTING_MAX {
        return Err(err!("setting: at most {SETTING_MAX} entries per bundle"));
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut seen_id = HashSet::new();
    let mut seen_name = HashSet::new();
    for (i, mut s) in raw.into_iter().enumerate() {
        let id = s.id.trim().to_string();
        if id.is_empty() {
            return Err(err!("setting[{i}]: id is required"));
        }
        if !valid_bundle_id(&id) {
            return Err(err!("setting[{i}]: id is not a safe id"));
        }
        let qid = format!("{id:?}");
        if !seen_id.insert(id.clone()) {
            return Err(err!("setting: duplicate id {qid} in bundle"));
        }
        s.id = id;

        let name = s.name.trim().to_string();
        if name.is_empty() {
            return Err(err!("setting {qid}: name is required"));
        }
        if !PROMPT_ARG_NAME_RE.is_match(&name) {
            return Err(err!(
                "setting {qid}: name {:?} must match {PROMPT_ARG_NAME_PATTERN}",
                name
            ));
        }
        if !seen_name.insert(name.clone()) {
            return Err(err!("setting: duplicate name {:?} in bundle", name));
        }
        s.name = name;

        s.label = s.label.trim().to_string();
        if s.label.is_empty() {
            s.label = s.name.clone();
        }
        s.description = s.description.trim().to_string();
        s.placeholder = s.placeholder.trim().to_string();
        s.url = s.url.trim().to_string();
        s.url_label = s.url_label.trim().to_string();
        s.bundle_name = meta.name.clone();
        s.bundle_id = meta.id.clone();
        s.bundle_dir = bundle_dir.to_string();
        out.push(s);
    }
    Ok(out)
}

/// The setting with the given id.
pub fn find_setting_by_id<'a>(all: &'a [Setting], id: &str) -> Option<&'a Setting> {
    all.iter().find(|s| s.id == id)
}

/// `[[setting]]` entries belonging to `bundle_id`.
pub fn settings_for_bundle(all: &[Setting], bundle_id: &str) -> Vec<Setting> {
    all.iter()
        .filter(|s| s.bundle_id == bundle_id)
        .cloned()
        .collect()
}
