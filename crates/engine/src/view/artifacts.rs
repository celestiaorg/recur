//! Artifact rules of the Observatory and the chat list card.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use super::js;

/// The fields the Observatory needs to open or preview an artifact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactRef {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub path: String,
}

/// Normalizes an `artifact.created` / `artifact.presented` payload. The wire
/// carries `domain.Artifact`'s lowerCamel JSON tags; PascalCase names are
/// not accepted.
pub fn artifact_from_event(raw: &Value) -> Option<ArtifactRef> {
    let o = raw.as_object()?;
    let field = |k: &str| js::trim(js::str_field(o.get(k))).to_owned();
    let id = field("id");
    let path = field("path");
    if id.is_empty() || path.is_empty() {
        return None;
    }
    let title = field("title");
    Some(ArtifactRef {
        title: if title.is_empty() { id.clone() } else { title },
        kind: field("kind").to_lowercase(),
        id,
        path,
    })
}

/// Unseen artifact ids per workspace (badge = list length).
pub type UnseenArtifactIds = BTreeMap<String, Vec<String>>;

/// Records an unseen artifact once: create then present of one id stays one.
pub fn add_unseen_artifact_id(
    by_workspace: &UnseenArtifactIds,
    workspace_id: &str,
    artifact_id: &str,
) -> UnseenArtifactIds {
    let wid = js::trim(workspace_id);
    let id = js::trim(artifact_id);
    let mut out = by_workspace.clone();
    if wid.is_empty() || id.is_empty() {
        return out;
    }
    let ids = out.entry(wid.to_owned()).or_default();
    if !ids.iter().any(|x| x == id) {
        ids.push(id.to_owned());
    }
    out
}

pub fn clear_unseen_artifact_ids(
    by_workspace: &UnseenArtifactIds,
    workspace_id: &str,
) -> UnseenArtifactIds {
    let wid = js::trim(workspace_id);
    let mut out = by_workspace.clone();
    if let Some(ids) = out.get_mut(wid) {
        ids.clear();
    }
    out
}

/// The icon for an artifact kind.
pub fn artifact_icon(kind: &str) -> &'static str {
    match kind.to_lowercase().as_str() {
        "mermaid" => "sitemap",
        "html" => "file-type-html",
        "svg" => "file-type-svg",
        "png" | "jpeg" | "jpg" | "gif" | "webp" => "photo",
        "csv" => "file-type-csv",
        "json" => "braces",
        "md" | "markdown" => "markdown",
        _ => "file",
    }
}

/// One artifact as list_artifacts JSON (and the chat list).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListedArtifact {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub path: String,
    pub created_at: String,
    pub created_by: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListedArtifacts {
    pub count: i64,
    pub artifacts: Vec<ListedArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactKindTally {
    pub kind: String,
    pub count: usize,
}

const KIND_TALLY_ORDER: [&str; 10] = [
    "html", "svg", "mermaid", "png", "jpeg", "jpg", "csv", "json", "md", "markdown",
];

fn parse_json_object(text: &str) -> Option<Map<String, Value>> {
    let t = js::trim(text);
    if t.is_empty() {
        return None;
    }
    if let Some(o) = js::parse_object(t) {
        return Some(o);
    }
    match t.find('{') {
        Some(i) if i > 0 => js::parse_object(&t[i..]),
        _ => None,
    }
}

pub fn parse_listed_artifact(a: &Value) -> ListedArtifact {
    let empty = Map::new();
    let o = a.as_object().unwrap_or(&empty);
    let s = |k: &str| js::str_field(o.get(k)).to_owned();
    ListedArtifact {
        id: match o.get("id") {
            Some(Value::String(id)) => id.clone(),
            v => js::string_of(v),
        },
        title: s("title"),
        kind: js::trim(js::str_field(o.get("kind"))).to_lowercase(),
        path: s("path"),
        created_at: s("createdAt"),
        created_by: s("createdBy"),
    }
}

/// Parses a list_artifacts tool result. A leading prose prefix is accepted so a
/// wrapped MCP payload still yields rows; a null list (`"artifacts":null`) is
/// an empty list.
pub fn parse_list_artifacts_result(text: &str) -> Option<ListedArtifacts> {
    if text.is_empty() {
        return None;
    }
    let raw = parse_json_object(text)?;
    let count = raw.get("count").filter(|c| c.is_number());
    match raw.get("artifacts") {
        Some(Value::Array(list)) => Some(ListedArtifacts {
            count: count.map_or(list.len() as i64, |c| js::num_field(Some(c))),
            artifacts: list.iter().map(parse_listed_artifact).collect(),
        }),
        None | Some(Value::Null) if raw.contains_key("artifacts") || count.is_some() => {
            Some(ListedArtifacts {
                count: js::num_field(count),
                artifacts: Vec::new(),
            })
        }
        _ => None,
    }
}

static SCRATCH_ARTIFACT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^scratch://artifact-(.+)\.([^.]+)$").expect("scratch re"));

/// The artifact id of a scratch editor tab (`scratch://artifact-<id>.<lang>`).
/// The lang is the last dotted segment, so ids may contain dots.
pub fn artifact_id_from_scratch_path(path: &str) -> String {
    SCRATCH_ARTIFACT
        .captures(path)
        .map(|c| c[1].to_owned())
        .unwrap_or_default()
}

/// Whether the Observatory can open this row (an id and a `workspaces/`
/// path).
pub fn artifact_can_open(id: &str, path: &str) -> bool {
    let rel = path.strip_prefix('/').unwrap_or(path);
    !js::trim(id).is_empty() && rel.starts_with("workspaces/")
}

/// The chat card's list; unparseable or failed results are `None`.
pub fn resolve_list_artifacts(result: &str, result_error: bool) -> Option<ListedArtifacts> {
    if result_error {
        return None;
    }
    parse_list_artifacts_result(result)
}

/// Kind counts: the known kinds in display order, then the rest in first-seen
/// order.
pub fn artifact_kind_tally<'a>(kinds: impl IntoIterator<Item = &'a str>) -> Vec<ArtifactKindTally> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for k in kinds {
        let k = k.to_lowercase();
        if k.is_empty() {
            continue;
        }
        match counts.iter_mut().find(|(x, _)| *x == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k, 1)),
        }
    }
    let mut out: Vec<ArtifactKindTally> = KIND_TALLY_ORDER
        .iter()
        .filter_map(|k| counts.iter().find(|(x, _)| x == k))
        .map(|(kind, count)| ArtifactKindTally {
            kind: kind.clone(),
            count: *count,
        })
        .collect();
    out.extend(
        counts
            .into_iter()
            .filter(|(k, _)| !KIND_TALLY_ORDER.contains(&k.as_str()))
            .map(|(kind, count)| ArtifactKindTally { kind, count }),
    );
    out
}

/// A compact age (`just now`, `12m`, `3h`, `2d`, `3w`, `2mo`, `1y`) plus the
/// full stamp for a tooltip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativeAge {
    pub rel: String,
    pub full: String,
}

/// [`RelativeAge`] for unix milliseconds `ms` at `now`. The full stamp is the
/// en-US `toLocaleString` shape in local time ("Aug 20, 2026, 4:32 AM").
pub fn format_relative_age(ms: i64, now: i64) -> Option<RelativeAge> {
    if ms <= 0 {
        return None;
    }
    let full = chrono::DateTime::from_timestamp_millis(ms)?
        .with_timezone(&chrono::Local)
        .format("%b %-d, %Y, %-I:%M %p")
        .to_string();
    let sec = ((now - ms) as f64 / 1000.0).round().max(0.0) as i64;
    const MINUTE: i64 = 60;
    const HOUR: i64 = 3600;
    const DAY: i64 = 86400;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;
    let rel = if sec < MINUTE {
        "just now".to_owned()
    } else if sec < HOUR {
        format!("{}m", sec / MINUTE)
    } else if sec < DAY {
        format!("{}h", sec / HOUR)
    } else if sec < 14 * DAY {
        format!("{}d", sec / DAY)
    } else if sec < MONTH {
        format!("{}w", sec / WEEK)
    } else if sec < YEAR {
        format!("{}mo", sec / MONTH)
    } else {
        format!("{}y", sec / YEAR)
    };
    Some(RelativeAge { rel, full })
}

/// A list row's age; blank or unparseable `iso` is `None`.
pub fn artifact_when(iso: &str, now: i64) -> Option<RelativeAge> {
    if js::trim(iso).is_empty() {
        return None;
    }
    let ms = chrono::DateTime::parse_from_rfc3339(js::trim(iso))
        .ok()?
        .timestamp_millis();
    format_relative_age(ms, now)
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;
