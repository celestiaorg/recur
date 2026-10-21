//! `[[languageData]]`: comment tokens and indent patterns for a TextMate
//! scope (CodeMirror languageData, not Settings prefs).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::go::quote;
use crate::manifest::{BundleMeta, is_false, valid_bundle_id};
use crate::{Error, err};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LanguageData {
    pub id: String,
    #[serde(rename = "scopeName")]
    pub scope_name: String,
    #[serde(rename = "lineComment", skip_serializing_if = "String::is_empty")]
    pub line_comment: String,
    #[serde(rename = "blockCommentStart", skip_serializing_if = "String::is_empty")]
    pub block_comment_start: String,
    #[serde(rename = "blockCommentEnd", skip_serializing_if = "String::is_empty")]
    pub block_comment_end: String,
    #[serde(
        rename = "increaseIndentPattern",
        skip_serializing_if = "String::is_empty"
    )]
    pub increase_indent_pattern: String,
    #[serde(
        rename = "decreaseIndentPattern",
        skip_serializing_if = "String::is_empty"
    )]
    pub decrease_indent_pattern: String,
    #[serde(
        rename = "indentNextLinePattern",
        skip_serializing_if = "String::is_empty"
    )]
    pub indent_next_line_pattern: String,
    #[serde(rename = "zeroIndentPattern", skip_serializing_if = "String::is_empty")]
    pub zero_indent_pattern: String,
    #[serde(rename = "disableIndentCorrections", skip_serializing_if = "is_false")]
    pub disable_indent_corrections: bool,
    /// Extra opener characters merged onto CodeMirror defaults (`()[]{} '" \``).
    /// `<` pairs with `>`; `_` / `*` pair with themselves.
    #[serde(rename = "closeBrackets", skip_serializing_if = "Vec::is_empty")]
    pub close_brackets: Vec<String>,

    #[serde(skip_deserializing, rename = "bundleName")]
    pub bundle_name: String,
    #[serde(skip_deserializing, rename = "bundleID")]
    pub bundle_id: String,
    /// "user" | "default"
    #[serde(skip_deserializing, rename = "sourceRoot")]
    pub source_root: String,
    #[serde(skip_deserializing, rename = "bundleDir")]
    pub bundle_dir: String,
}

impl LanguageData {
    /// True when the entry sets comment tokens, indent patterns, or brackets.
    pub fn has_payload(&self) -> bool {
        !self.line_comment.is_empty()
            || !self.block_comment_start.is_empty()
            || !self.increase_indent_pattern.is_empty()
            || !self.decrease_indent_pattern.is_empty()
            || !self.indent_next_line_pattern.is_empty()
            || !self.zero_indent_pattern.is_empty()
            || self.disable_indent_corrections
            || !self.close_brackets.is_empty()
    }
}

pub(crate) fn parse_language_data(
    bundle_dir: &str,
    meta: &BundleMeta,
    raw: Vec<LanguageData>,
) -> Result<Vec<LanguageData>, Error> {
    let mut out = Vec::with_capacity(raw.len());
    let mut seen_id = HashSet::new();
    let mut seen_scope = HashSet::new();
    for (i, mut d) in raw.into_iter().enumerate() {
        let id = d.id.trim().to_string();
        if id.is_empty() {
            return Err(err!("languageData[{i}]: id is required"));
        }
        if !valid_bundle_id(&id) {
            return Err(err!("languageData[{i}]: id is not a safe id"));
        }
        let qid = quote(&id);
        if !seen_id.insert(id.clone()) {
            return Err(err!("languageData: duplicate id {qid} in bundle"));
        }
        d.id = id;

        let scope = d.scope_name.trim().to_string();
        if scope.is_empty() {
            return Err(err!("languageData {qid}: scopeName is required"));
        }
        if !seen_scope.insert(scope.clone()) {
            return Err(err!(
                "languageData: duplicate scopeName {} in bundle",
                quote(&scope)
            ));
        }
        d.scope_name = scope;

        for s in [
            &mut d.line_comment,
            &mut d.block_comment_start,
            &mut d.block_comment_end,
            &mut d.increase_indent_pattern,
            &mut d.decrease_indent_pattern,
            &mut d.indent_next_line_pattern,
            &mut d.zero_indent_pattern,
        ] {
            *s = s.trim().to_string();
        }

        let mut extras = Vec::with_capacity(d.close_brackets.len());
        let mut seen_extra = HashSet::new();
        for raw in &d.close_brackets {
            let ch = raw.trim();
            if ch.is_empty() {
                continue;
            }
            if ch.chars().count() != 1 {
                return Err(err!(
                    "languageData {qid}: closeBrackets extra {} must be a single character",
                    quote(ch)
                ));
            }
            if seen_extra.insert(ch.to_string()) {
                extras.push(ch.to_string());
            }
        }
        d.close_brackets = extras;

        if d.block_comment_start.is_empty() != d.block_comment_end.is_empty() {
            return Err(err!(
                "languageData {qid}: blockCommentStart and blockCommentEnd must both be set"
            ));
        }
        if !d.has_payload() {
            return Err(err!(
                "languageData {qid}: set comment tokens, indent patterns, and/or closeBrackets"
            ));
        }

        d.bundle_name = meta.name.clone();
        d.bundle_id = meta.id.clone();
        d.bundle_dir = bundle_dir.to_string();
        out.push(d);
    }
    Ok(out)
}

/// The language-data entry with the given id.
pub fn find_language_data_by_id<'a>(all: &'a [LanguageData], id: &str) -> Option<&'a LanguageData> {
    let id = id.trim();
    all.iter().find(|d| d.id == id)
}

/// The language-data entry with the given scopeName.
pub fn find_language_data_by_scope<'a>(
    all: &'a [LanguageData],
    scope: &str,
) -> Option<&'a LanguageData> {
    let scope = scope.trim();
    all.iter().find(|d| d.scope_name == scope)
}

/// Walks dotted scope prefixes (`source.go` → `source`) and returns the most
/// specific catalog entry.
pub fn resolve_language_data<'a>(all: &'a [LanguageData], scope: &str) -> Option<&'a LanguageData> {
    let scope = scope.trim();
    if scope.is_empty() || all.is_empty() {
        return None;
    }
    let by_scope: HashMap<&str, &LanguageData> =
        all.iter().map(|d| (d.scope_name.as_str(), d)).collect();
    let mut s = scope;
    while !s.is_empty() {
        if let Some(d) = by_scope.get(s) {
            return Some(d);
        }
        match s.rfind('.') {
            Some(i) if i > 0 => s = &s[..i],
            _ => break,
        }
    }
    None
}
