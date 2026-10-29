//! `[[symbol]]`: TextMate `showInSymbolList` preferences.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::manifest::{BundleMeta, valid_bundle_id};
use crate::{Error, err};

/// On-disk `[[symbol]]` shape. `show` is optional so an omitted value means
/// include (TextMate `showInSymbolList` default 1).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SymbolToml {
    id: String,
    scope: String,
    transform: String,
    kind: String,
    show: Option<bool>,
}

/// One `[[symbol]]` entry. `scope` is a token-scope selector (not a file
/// grammar). `show = false` excludes a more-specific scope from a broader
/// include.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Symbol {
    pub id: String,
    pub scope: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub transform: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    pub show: bool,

    #[serde(rename = "bundleName")]
    pub bundle_name: String,
    #[serde(rename = "bundleID")]
    pub bundle_id: String,
    #[serde(rename = "sourceRoot")]
    pub source_root: String,
    #[serde(rename = "bundleDir")]
    pub bundle_dir: String,
}

pub(crate) fn parse_symbols(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<SymbolToml>,
) -> Result<Vec<Symbol>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    let mut seen_id = HashSet::new();
    for (i, r) in raw.into_iter().enumerate() {
        let id = r.id.trim().to_string();
        if id.is_empty() {
            return Err(err!("symbol[{i}]: id is required"));
        }
        if !valid_bundle_id(&id) {
            return Err(err!("symbol[{i}]: id is not a safe id"));
        }
        let qid = format!("{id:?}");
        if !seen_id.insert(id.clone()) {
            return Err(err!("symbol: duplicate id {qid} in bundle"));
        }
        let scope = r.scope.trim().to_string();
        if scope.is_empty() {
            return Err(err!("symbol {qid}: scope is required"));
        }
        out.push(Symbol {
            id,
            scope,
            transform: r.transform.trim().to_string(),
            kind: r.kind.trim().to_string(),
            show: r.show.unwrap_or(true),
            bundle_name: meta.name.clone(),
            bundle_id: meta.id.clone(),
            source_root: String::new(),
            bundle_dir: bundle_dir.to_string(),
        });
    }
    Ok(out)
}

/// The symbol rule with the given id.
pub fn find_symbol_by_id<'a>(all: &'a [Symbol], id: &str) -> Option<&'a Symbol> {
    let id = id.trim();
    all.iter().find(|s| s.id == id)
}
