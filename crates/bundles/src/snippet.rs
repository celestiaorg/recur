//! `[[snippet]]`: tab-trigger (and/or keyEquivalent) snippet bodies in VS
//! Code placeholder syntax (`$0` / `$1` / `${1:default}`).

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::manifest::{BundleMeta, valid_bundle_id};
use crate::scope::matches_scope_glob;
use crate::{Error, err};

static TAB_TRIGGER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{L}_][\p{L}\p{Nd}_]*$").expect("tab trigger"));

/// Letter/underscore then word chars (ife, forof, py3, ifmain), ≤ 40 bytes.
fn valid_tab_trigger(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && TAB_TRIGGER_RE.is_match(s)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snippet {
    pub id: String,
    pub name: String,
    #[serde(rename = "tabTrigger")]
    pub tab_trigger: String,
    pub scope: String,
    #[serde(rename = "fileGlob")]
    pub file_glob: String,
    pub body: String,
    #[serde(rename = "keyEquivalent")]
    pub key_equivalent: String,
    pub order: i64,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
}

pub(crate) fn parse_snippets(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<Snippet>,
) -> Result<Vec<Snippet>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    let mut seen_id = HashSet::new();
    for (i, mut s) in raw.into_iter().enumerate() {
        let id = s.id.trim().to_string();
        if id.is_empty() {
            return Err(err!("snippet[{i}]: id is required"));
        }
        if !valid_bundle_id(&id) {
            return Err(err!("snippet[{i}]: id is not a safe id"));
        }
        let qid = format!("{id:?}");
        if !seen_id.insert(id.clone()) {
            return Err(err!("snippet: duplicate id {qid} in bundle"));
        }
        s.id = id;

        let name = s.name.trim().to_string();
        if name.is_empty() {
            return Err(err!("snippet {qid}: name is required"));
        }
        s.name = name;

        s.tab_trigger = s.tab_trigger.trim().to_string();
        s.key_equivalent = s.key_equivalent.trim().to_string();
        if s.tab_trigger.is_empty() && s.key_equivalent.is_empty() {
            return Err(err!("snippet {qid}: set tabTrigger and/or keyEquivalent"));
        }
        if !s.tab_trigger.is_empty() && !valid_tab_trigger(&s.tab_trigger) {
            return Err(err!(
                "snippet {qid}: tabTrigger {:?} must be a word (letter, then letters/digits/_)",
                s.tab_trigger
            ));
        }

        s.scope = s.scope.trim().to_string();
        s.file_glob = s.file_glob.trim().to_string();
        s.body = s.body.trim().to_string();
        if s.body.is_empty() {
            return Err(err!("snippet {qid}: body is required"));
        }

        s.bundle_name = meta.name.clone();
        s.bundle_id = meta.id.clone();
        s.bundle_dir = bundle_dir.to_string();
        out.push(s);
    }
    Ok(out)
}

/// The snippet with the given id.
pub fn find_snippet_by_id<'a>(all: &'a [Snippet], id: &str) -> Option<&'a Snippet> {
    let id = id.trim();
    all.iter().find(|s| s.id == id)
}

/// Whether the snippet applies to the editor scope/path.
pub fn snippet_matches(s: &Snippet, file_scope: &str, file_path: &str) -> bool {
    matches_scope_glob(&s.scope, &s.file_glob, file_scope, file_path)
}
