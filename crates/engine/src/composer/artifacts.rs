//! `@artifact[:id-or-title]` and loose `@Title` mentions (Go
//! `artifact_mentions.go`): mentioned artifacts are inlined (text kinds,
//! capped) into a system instruction ahead of the wire prompt.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use recur_domain::Artifact;
use regex::Regex;

use super::files::FILE_AT_RE;
use super::terminals::{at_token_bounded, fence_snapshot, scrub_instruction_text};
use crate::artifacts::{artifact_kind_is_binary, list_artifacts, read_artifact_text};

/// Keep in sync with the frontend `ARTIFACT_AT_RE`.
static ARTIFACT_AT_RE: LazyLock<Regex> = LazyLock::new(|| {
    recur_bundles::go::compile_regexp(r"@artifact(?::([^\s@]+))?").expect("artifact pattern")
});

static AT_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| recur_bundles::go::compile_regexp(r"@([^\s@]+)").expect("at-name pattern"));

const MAX_ARTIFACT_SNAPSHOTS: usize = 4;

/// One artifact chip.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactMention {
    /// The token after `@`.
    pub chip: String,
    /// Empty for bare `@artifact`.
    pub qual: String,
    pub start: usize,
    /// `@Title` (the panel name) rather than `@artifact…`. Unmatched loose
    /// mentions are prose, not "no matching artifact".
    pub loose: bool,
}

fn equal_fold(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// `@artifact…` and `@terminal…` are keywords, not titles.
fn reserved_at_name(token: &str) -> bool {
    let t = token.to_lowercase();
    t == "artifact" || t.starts_with("artifact:") || t == "terminal" || t.starts_with("terminal:")
}

fn at_name_is_file_mention(token: &str) -> bool {
    let s = format!("@{token}");
    FILE_AT_RE.find(&s).is_some_and(|m| m.as_str() == s)
}

/// Unique `@artifact` chips in first-seen order.
pub fn collect_artifact_mentions(text: &str) -> Vec<ArtifactMention> {
    if text.is_empty() || !text.contains("@artifact") {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in ARTIFACT_AT_RE.captures_iter(text) {
        let Some(m) = c.get(0) else { continue };
        if !at_token_bounded(text, m.start(), m.end()) {
            continue;
        }
        let qual = c.get(1).map_or("", |q| q.as_str()).to_owned();
        let chip = if qual.is_empty() {
            "artifact".to_owned()
        } else {
            format!("artifact:{qual}")
        };
        if !seen.insert(chip.clone()) {
            continue;
        }
        out.push(ArtifactMention {
            chip,
            qual,
            start: m.start(),
            loose: false,
        });
    }
    out
}

/// `@Title` tokens that uniquely match a single-word panel title.
pub fn collect_loose_artifact_title_mentions(
    text: &str,
    list: &[Artifact],
) -> Vec<ArtifactMention> {
    if text.is_empty() || !text.contains('@') || list.is_empty() {
        return Vec::new();
    }
    let unique = unique_single_token_titles(list);
    if unique.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in AT_NAME_RE.captures_iter(text) {
        let (Some(m), Some(tok)) = (c.get(0), c.get(1)) else {
            continue;
        };
        if !at_token_bounded(text, m.start(), m.end()) {
            continue;
        }
        let token = tok.as_str();
        if reserved_at_name(token) || at_name_is_file_mention(token) {
            continue;
        }
        let key = token.to_lowercase();
        if unique.iter().filter(|t| **t == key).count() != 1 {
            continue;
        }
        if !seen.insert(key) {
            continue;
        }
        out.push(ArtifactMention {
            chip: token.to_owned(),
            qual: token.to_owned(),
            start: m.start(),
            loose: true,
        });
    }
    out
}

/// Lowercased single-word titles, one entry per artifact (duplicates kept
/// so a shared title is ambiguous).
fn unique_single_token_titles(list: &[Artifact]) -> Vec<String> {
    list.iter()
        .filter_map(|a| {
            let title = a.title.trim();
            if title.is_empty()
                || title.contains([' ', '\t', '\n', '\r'])
                || reserved_at_name(title)
            {
                return None;
            }
            Some(title.to_lowercase())
        })
        .collect()
}

/// Inlines the text's own artifact mentions. Unchanged without a workspace.
pub fn wrap_attached_artifact_mentions(text: &str, data_dir: &Path, workspace_id: &str) -> String {
    if workspace_id.is_empty() {
        return text.to_owned();
    }
    let mut mentions = collect_artifact_mentions(text);
    let mut list = Vec::new();
    if text.contains('@') {
        list = list_artifacts(data_dir, workspace_id).unwrap_or_default();
        mentions.extend(collect_loose_artifact_title_mentions(text, &list));
    }
    attach_artifact_mentions(text, &mentions, &list, data_dir)
}

/// Prepends the mentioned artifacts (at most four; binary kinds noted, text
/// kinds fenced). Unchanged with no mentions or no artifacts.
pub fn attach_artifact_mentions(
    text: &str,
    mentions: &[ArtifactMention],
    list: &[Artifact],
    data_dir: &Path,
) -> String {
    if mentions.is_empty() || list.is_empty() {
        return text.to_owned();
    }
    let mut b = String::from("<system_instruction>\n");
    b.push_str("The user attached Observatory artifacts. These live under the managed data dir ");
    b.push_str("(workspaces/<id>/artifacts/) — not the git worktree. Do not search the repo for these paths. ");
    b.push_str("For another artifact, call mcp__recur__list_artifacts / mcp__recur__read_artifact / mcp__recur__present_artifact.\n");
    let mut wrote = 0;
    let mut seen_id = HashSet::new();
    for m in mentions {
        if wrote >= MAX_ARTIFACT_SNAPSHOTS {
            break;
        }
        let Some(hit) = resolve_artifact_mention(m, list) else {
            if m.loose {
                continue;
            }
            b.push_str(&format!(
                "\n### @{}\n(no matching artifact)\n",
                scrub_instruction_text(&m.chip)
            ));
            wrote += 1;
            continue;
        };
        if !seen_id.insert(hit.id.clone()) {
            continue;
        }
        let title = match hit.title.trim() {
            "" => hit.id.as_str(),
            t => t,
        };
        b.push_str(&format!(
            "\n### @{} ({}, {}, id {})\n",
            scrub_instruction_text(&m.chip),
            scrub_instruction_text(title),
            scrub_instruction_text(hit.kind.as_str()),
            scrub_instruction_text(&hit.id),
        ));
        wrote += 1;
        if artifact_kind_is_binary(&hit.kind) {
            b.push_str(&format!(
                "(binary {} image — open in the Artifacts panel; bytes are not inlined)\n",
                hit.kind.as_str()
            ));
            continue;
        }
        match read_artifact_text(data_dir, hit) {
            Err(e) => b.push_str(&format!("(could not read: {e})\n")),
            Ok(body) if body.trim().is_empty() => b.push_str("(empty)\n"),
            Ok(body) => b.push_str(&fence_snapshot(&body)),
        }
    }
    if wrote == 0 {
        return text.to_owned();
    }
    b.push_str("</system_instruction>\n\n");
    b.push_str(text);
    b
}

/// The artifact a mention names: bare → the newest; loose → the one
/// title match; a qualifier → exact id, a unique id prefix, then a unique
/// title.
pub fn resolve_artifact_mention<'a>(
    m: &ArtifactMention,
    list: &'a [Artifact],
) -> Option<&'a Artifact> {
    let first = list.first()?;
    if m.qual.is_empty() {
        return Some(first);
    }
    let qual = m.qual.as_str();
    if m.loose {
        let mut hits = list.iter().filter(|a| equal_fold(a.title.trim(), qual));
        let found = hits.next()?;
        return hits.next().is_none().then_some(found);
    }
    let mut prefix = Vec::new();
    let mut title = Vec::new();
    for a in list {
        if a.id == qual {
            return Some(a);
        }
        if a.id.starts_with(qual) {
            prefix.push(a);
        }
        if equal_fold(a.title.trim(), qual) {
            title.push(a);
        }
    }
    if prefix.len() == 1 {
        return Some(prefix[0]);
    }
    if title.len() == 1 {
        return Some(title[0]);
    }
    None
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;
