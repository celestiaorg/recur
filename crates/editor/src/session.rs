//! Editor session persistence logic: the session
//! shape, its caps and pruning, the wire format the backend stores, and the
//! change gate that turns a session into a minimal patch. Scheduling the
//! debounced writes and the RPC queue is the host's job.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::text::normalize_newlines;
use crate::view_state::{
    FileViewState, MAX_STORED_VIEWS, editor_text_eq, parse_file_views, prune_file_views,
};

/// Max length (UTF-16) stored per scratch buffer.
pub const MAX_SCRATCH_CHARS: usize = 400_000;
pub const MAX_SCRATCH_DOCS: usize = 12;
/// Unsaved buffers longer than this (UTF-16) are skipped, never truncated.
pub const MAX_UNSAVED_CHARS: usize = 4_000_000;
pub const MAX_UNSAVED_BUFFERS: usize = 100;
pub const MAX_OPEN_FILES: usize = 64;
/// Debounce for keystroke-driven session writes.
pub const SAVE_DEBOUNCE_MS: u64 = 400;
pub const MAX_SYNTAX_BY_PATH: usize = recur_textmate::MAX_SYNTAX_BY_PATH;
pub const MAX_SYNTAX_BY_FILE_TYPE: usize = recur_textmate::MAX_SYNTAX_BY_FILE_TYPE;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScratchDoc {
    pub title: String,
    pub content: String,
    pub lang: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

/// An unsaved buffer: the draft and either the disk text it diverged from
/// (live) or that text's hash (restored).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StashedBuffer {
    pub draft: String,
    pub saved: Option<String>,
    pub base_hash: Option<String>,
}

const PANEL_KEYS: [&str; 9] = [
    "maestro",
    "plan",
    "findings",
    "files",
    "scm",
    "outline",
    "search",
    "refs",
    "artifacts",
];

fn panel_key(v: Option<&str>) -> Option<String> {
    v.filter(|k| PANEL_KEYS.contains(k)).map(str::to_string)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub open_files: Vec<String>,
    pub selected_path: String,
    pub split_path: Option<String>,
    pub scratch_docs: Option<BTreeMap<String, ScratchDoc>>,
    pub scratch_seq: Option<u64>,
    pub stashed_buffers: Option<BTreeMap<String, StashedBuffer>>,
    pub file_views: Option<BTreeMap<String, FileViewState>>,
    pub syntax_by_path: Option<BTreeMap<String, String>>,
    pub syntax_by_file_type: Option<BTreeMap<String, String>>,
    /// `true` for the Review column; otherwise Editor.
    pub main_surface_review: bool,
    pub left_kind: Option<String>,
    pub right_kind: Option<String>,
    pub left_open: Option<bool>,
    pub right_open: Option<bool>,
    pub syntax_maps_known: bool,
}

/// `sessionHasPaneChrome`: dock state differs from the cold default.
pub fn session_has_pane_chrome(s: &Session) -> bool {
    s.left_kind.as_deref().is_some_and(|k| k != "maestro")
        || s.right_kind.as_deref().is_some_and(|k| k != "files")
        || s.left_open == Some(false)
        || s.right_open == Some(false)
}

/// `uniqueOpenFiles`: first wins, empty paths dropped.
pub fn unique_open_files(paths: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    paths
        .iter()
        .filter(|p| !p.is_empty() && seen.insert(p.as_str()))
        .cloned()
        .collect()
}

/// `remapOpenFiles`: follow a rename/move of `from` (and its children).
pub fn remap_open_files(paths: &[String], from: &str, to: &str) -> Vec<String> {
    if from.is_empty() || to.is_empty() || from == to {
        return unique_open_files(paths);
    }
    let was_from = |p: &str| p == from || p.starts_with(&format!("{from}/"));
    let rewrite = |p: &str| {
        if p == from {
            to.to_string()
        } else {
            format!("{to}{}", &p[from.len()..])
        }
    };
    let claimed: BTreeSet<String> = paths
        .iter()
        .filter(|p| was_from(p))
        .map(|p| rewrite(p))
        .collect();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for p in paths {
        if !was_from(p) && claimed.contains(p) {
            continue;
        }
        let np = if was_from(p) { rewrite(p) } else { p.clone() };
        if np.is_empty() || !seen.insert(np.clone()) {
            continue;
        }
        out.push(np);
    }
    out
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn utf16_prefix(s: &str, units: usize) -> &str {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        if n + c.len_utf16() > units {
            return &s[..i];
        }
        n += c.len_utf16();
    }
    s
}

/// `pruneScratchDocs`: scratch docs still open, truncated, at most 12.
pub fn prune_scratch_docs<'a>(
    docs: &BTreeMap<String, ScratchDoc>,
    keep: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, ScratchDoc> {
    let mut out = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for path in keep {
        if !seen.insert(path) || !path.starts_with("scratch://") {
            continue;
        }
        let Some(doc) = docs.get(path) else { continue };
        out.insert(
            path.to_string(),
            ScratchDoc {
                title: if doc.title.is_empty() {
                    "Scratch".into()
                } else {
                    doc.title.clone()
                },
                lang: if doc.lang.is_empty() {
                    "txt".into()
                } else {
                    doc.lang.clone()
                },
                content: utf16_prefix(&doc.content, MAX_SCRATCH_CHARS).to_string(),
                mime: None,
            },
        );
        if out.len() >= MAX_SCRATCH_DOCS {
            break;
        }
    }
    out
}

/// `maxScratchSeq`: the highest `scratch://N-…` sequence.
pub fn max_scratch_seq<'a>(paths: impl IntoIterator<Item = &'a str>) -> u64 {
    paths
        .into_iter()
        .filter_map(|p| {
            let rest = p.strip_prefix("scratch://")?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty() && rest[digits.len()..].starts_with('-'))
                .then(|| digits.parse::<u64>().unwrap_or(0))
        })
        .max()
        .unwrap_or(0)
}

/// `pruneStashedBuffers`: dirty disk buffers for open tabs; scratch, clean
/// and oversized ones dropped.
pub fn prune_stashed_buffers<'a>(
    buffers: Option<&BTreeMap<String, StashedBuffer>>,
    keep: impl IntoIterator<Item = &'a str>,
) -> Option<BTreeMap<String, StashedBuffer>> {
    let buffers = buffers?;
    let mut out = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for path in keep {
        if path.is_empty() || path.starts_with("scratch://") || !seen.insert(path) {
            continue;
        }
        let Some(buf) = buffers.get(path) else {
            continue;
        };
        if utf16_len(&buf.draft) > MAX_UNSAVED_CHARS {
            continue;
        }
        match (&buf.saved, &buf.base_hash) {
            (Some(saved), _) => {
                if utf16_len(saved) > MAX_UNSAVED_CHARS || editor_text_eq(&buf.draft, saved) {
                    continue;
                }
            }
            (None, Some(h)) if !h.is_empty() => {}
            _ => continue,
        }
        out.insert(path.to_string(), buf.clone());
        if out.len() >= MAX_UNSAVED_BUFFERS {
            break;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// sha256 hex of the text with LF newlines (`editorContentHash`).
pub fn editor_content_hash(text: &str) -> String {
    let digest = Sha256::digest(normalize_newlines(text).as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn keep_paths(s: &Session, open: &[String]) -> Vec<String> {
    let mut keep = Vec::new();
    if !s.selected_path.is_empty() {
        keep.push(s.selected_path.clone());
    }
    keep.extend(open.iter().cloned());
    if let Some(sp) = &s.split_path {
        keep.push(sp.clone());
    }
    keep
}

fn prune_syntax(
    map: Option<&BTreeMap<String, String>>,
    max: usize,
    drop_scratch: bool,
) -> Option<BTreeMap<String, String>> {
    let map = map?;
    let entries: Vec<(&String, &String)> = map
        .iter()
        .filter(|(k, _)| !(k.is_empty() || drop_scratch && k.starts_with("scratch://")))
        .collect();
    let skip = entries.len().saturating_sub(max);
    Some(
        entries[skip..]
            .iter()
            .map(|(k, v)| ((*k).clone(), (*v).clone()))
            .collect(),
    )
}

/// `normalizeSession`: caps and keep-set pruning before diffing.
pub fn normalize_session(s: &Session) -> Session {
    let open: Vec<String> = unique_open_files(&s.open_files)
        .into_iter()
        .take(MAX_OPEN_FILES)
        .collect();
    let keep = keep_paths(s, &open);
    let keep_refs = || keep.iter().map(String::as_str);
    let views = s
        .file_views
        .as_ref()
        .map(|v| prune_file_views(v, keep_refs(), MAX_STORED_VIEWS));
    Session {
        open_files: open.clone(),
        selected_path: s.selected_path.clone(),
        split_path: s.split_path.clone().filter(|p| !p.is_empty()),
        scratch_seq: s.scratch_seq.filter(|n| *n > 0),
        scratch_docs: s
            .scratch_docs
            .as_ref()
            .map(|d| prune_scratch_docs(d, keep_refs())),
        stashed_buffers: prune_stashed_buffers(s.stashed_buffers.as_ref(), keep_refs()),
        file_views: views.filter(|v| !v.is_empty()),
        syntax_by_path: Some(
            prune_syntax(s.syntax_by_path.as_ref(), MAX_SYNTAX_BY_PATH, true).unwrap_or_default(),
        ),
        syntax_by_file_type: Some(
            prune_syntax(
                s.syntax_by_file_type.as_ref(),
                MAX_SYNTAX_BY_FILE_TYPE,
                false,
            )
            .unwrap_or_default(),
        ),
        syntax_maps_known: s.syntax_maps_known,
        main_surface_review: s.main_surface_review,
        left_kind: panel_key(s.left_kind.as_deref()),
        right_kind: panel_key(s.right_kind.as_deref()),
        left_open: s.left_open,
        right_open: s.right_open,
    }
}

/// The session metadata on the wire (`EditorSessionMeta`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireMeta {
    pub open_files: Vec<String>,
    pub selected_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scratch_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_views: Option<BTreeMap<String, FileViewState>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syntax_by_path: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syntax_by_file_type: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_surface: Option<String>,
    pub left_kind: String,
    pub right_kind: String,
    pub left_open: bool,
    pub right_open: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireBuffer {
    pub draft: String,
    pub base_hash: String,
}

/// A session patch (`EditorSessionPatch`); absent parts are unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<WireMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffers: Option<BTreeMap<String, WireBuffer>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_buffers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scratch_docs: Option<BTreeMap<String, ScratchDoc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_scratch_docs: Option<Vec<String>>,
}

/// `metaFor`.
pub fn meta_for(s: &Session) -> WireMeta {
    WireMeta {
        open_files: s.open_files.iter().take(MAX_OPEN_FILES).cloned().collect(),
        selected_path: s.selected_path.clone(),
        split_path: s.split_path.clone().filter(|p| !p.is_empty()),
        scratch_seq: s.scratch_seq.filter(|n| *n > 0),
        file_views: s.file_views.clone().filter(|v| !v.is_empty()),
        syntax_by_path: s
            .syntax_maps_known
            .then(|| s.syntax_by_path.clone().unwrap_or_default()),
        syntax_by_file_type: s
            .syntax_maps_known
            .then(|| s.syntax_by_file_type.clone().unwrap_or_default()),
        main_surface: s.main_surface_review.then(|| "review".to_string()),
        left_kind: panel_key(s.left_kind.as_deref()).unwrap_or_else(|| "maestro".into()),
        right_kind: panel_key(s.right_kind.as_deref()).unwrap_or_else(|| "files".into()),
        left_open: s.left_open != Some(false),
        right_open: s.right_open != Some(false),
    }
}

/// What the backend holds after the last acknowledged write (the change
/// gate's memory).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SentState {
    pub meta_json: String,
    pub buffers: BTreeMap<String, (String, String)>,
    pub scratch: BTreeMap<String, String>,
}

impl SentState {
    pub fn is_empty(&self) -> bool {
        self.meta_json.is_empty() && self.buffers.is_empty() && self.scratch.is_empty()
    }

    /// Sent state for a session that just came from the backend.
    pub fn for_session(s: &Session) -> SentState {
        SentState {
            meta_json: serde_json::to_string(&meta_for(s)).unwrap_or_default(),
            buffers: s
                .stashed_buffers
                .iter()
                .flatten()
                .map(|(p, b)| {
                    let base = b
                        .saved
                        .clone()
                        .or_else(|| b.base_hash.clone())
                        .unwrap_or_default();
                    (p.clone(), (b.draft.clone(), base))
                })
                .collect(),
            scratch: s
                .scratch_docs
                .iter()
                .flatten()
                .map(|(p, d)| (p.clone(), d.content.clone()))
                .collect(),
        }
    }
}

/// `buildPatch`: diff a normalized session against the last acknowledged
/// state. `prev: None` means the backend state is unknown: ship
/// everything, drop nothing.
pub fn build_patch(s: &Session, prev: Option<&SentState>) -> (Option<WirePatch>, SentState) {
    let meta = meta_for(s);
    let meta_json = serde_json::to_string(&meta).unwrap_or_default();
    let mut next = SentState {
        meta_json: meta_json.clone(),
        ..Default::default()
    };
    let mut upsert_buffers = BTreeMap::new();
    for (path, buf) in s.stashed_buffers.iter().flatten() {
        let base = buf
            .saved
            .clone()
            .or_else(|| buf.base_hash.clone())
            .unwrap_or_default();
        next.buffers
            .insert(path.clone(), (buf.draft.clone(), base.clone()));
        if prev.and_then(|p| p.buffers.get(path)) == Some(&(buf.draft.clone(), base)) {
            continue;
        }
        let base_hash = buf.base_hash.clone().unwrap_or_else(|| {
            buf.saved
                .as_deref()
                .map(editor_content_hash)
                .unwrap_or_default()
        });
        upsert_buffers.insert(
            path.clone(),
            WireBuffer {
                draft: buf.draft.clone(),
                base_hash,
            },
        );
    }
    let drop_buffers: Vec<String> = prev
        .map(|p| {
            p.buffers
                .keys()
                .filter(|k| !next.buffers.contains_key(*k))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let mut upsert_scratch = BTreeMap::new();
    for (path, doc) in s.scratch_docs.iter().flatten() {
        next.scratch.insert(path.clone(), doc.content.clone());
        if prev.and_then(|p| p.scratch.get(path)) == Some(&doc.content) {
            continue;
        }
        upsert_scratch.insert(path.clone(), doc.clone());
    }
    let drop_scratch: Vec<String> = prev
        .map(|p| {
            p.scratch
                .keys()
                .filter(|k| !next.scratch.contains_key(*k))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let meta_changed = prev.is_none_or(|p| p.meta_json != meta_json);
    if !meta_changed
        && upsert_buffers.is_empty()
        && drop_buffers.is_empty()
        && upsert_scratch.is_empty()
        && drop_scratch.is_empty()
    {
        return (None, next);
    }
    let patch = WirePatch {
        meta: meta_changed.then_some(meta),
        buffers: (!upsert_buffers.is_empty()).then_some(upsert_buffers),
        drop_buffers: (!drop_buffers.is_empty()).then_some(drop_buffers),
        scratch_docs: (!upsert_scratch.is_empty()).then_some(upsert_scratch),
        drop_scratch_docs: (!drop_scratch.is_empty()).then_some(drop_scratch),
    };
    (Some(patch), next)
}

fn string_map(v: Option<&Value>) -> Option<BTreeMap<String, String>> {
    let m: BTreeMap<String, String> = v?
        .as_object()?
        .iter()
        .filter_map(|(k, v)| (!k.is_empty()).then_some((k.clone(), v.as_str()?.to_string())))
        .collect();
    (!m.is_empty()).then_some(m)
}

/// `sessionFromWire`: a session as the backend returned it.
pub fn session_from_wire(w: &Value) -> Option<Session> {
    let open = w.get("openFiles")?.as_array()?;
    let open: Vec<String> = open
        .iter()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect();
    let mut stashed = BTreeMap::new();
    if let Some(bufs) = w.get("buffers").and_then(Value::as_object) {
        for (path, b) in bufs {
            if let Some(draft) = b.get("draft").and_then(Value::as_str) {
                stashed.insert(
                    path.clone(),
                    StashedBuffer {
                        draft: draft.to_string(),
                        saved: None,
                        base_hash: Some(
                            b.get("baseHash")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        ),
                    },
                );
            }
        }
    }
    let mut scratch = BTreeMap::new();
    if let Some(docs) = w.get("scratchDocs").and_then(Value::as_object) {
        for (path, d) in docs {
            if d.get("content").and_then(Value::as_str).is_some()
                && let Ok(doc) = serde_json::from_value::<ScratchDoc>(d.clone())
            {
                scratch.insert(path.clone(), doc);
            }
        }
    }
    let views = w.get("fileViews").map(parse_file_views).unwrap_or_default();
    Some(Session {
        open_files: unique_open_files(&open),
        selected_path: w
            .get("selectedPath")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        split_path: w
            .get("splitPath")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        scratch_seq: w
            .get("scratchSeq")
            .and_then(Value::as_f64)
            .map(|n| n as u64),
        scratch_docs: (!scratch.is_empty()).then_some(scratch),
        stashed_buffers: (!stashed.is_empty()).then_some(stashed),
        file_views: (!views.is_empty()).then_some(views),
        syntax_by_path: string_map(w.get("syntaxByPath")),
        syntax_by_file_type: string_map(w.get("syntaxByFileType")),
        main_surface_review: w.get("mainSurface").and_then(Value::as_str) == Some("review"),
        left_kind: panel_key(w.get("leftKind").and_then(Value::as_str)),
        right_kind: panel_key(w.get("rightKind").and_then(Value::as_str)),
        left_open: w.get("leftOpen").and_then(Value::as_bool),
        right_open: w.get("rightOpen").and_then(Value::as_bool),
        syntax_maps_known: false,
    })
}

/// The restore half of `loadEditorSession`: dedupe tabs, keep only what
/// open tabs need, and return `None` when nothing is worth restoring.
pub fn restorable_session(mut s: Session) -> Option<Session> {
    s.open_files = unique_open_files(&s.open_files);
    let has_syntax = s.syntax_by_path.as_ref().is_some_and(|m| !m.is_empty())
        || s.syntax_by_file_type
            .as_ref()
            .is_some_and(|m| !m.is_empty());
    if s.open_files.is_empty() {
        if !has_syntax && !s.main_surface_review && !session_has_pane_chrome(&s) {
            return None;
        }
        return Some(Session {
            syntax_by_path: s.syntax_by_path,
            syntax_by_file_type: s.syntax_by_file_type,
            syntax_maps_known: s.syntax_maps_known,
            main_surface_review: s.main_surface_review,
            left_kind: s.left_kind,
            right_kind: s.right_kind,
            left_open: s.left_open,
            right_open: s.right_open,
            ..Default::default()
        });
    }
    let mut keep = s.open_files.clone();
    if !s.selected_path.is_empty() {
        keep.insert(0, s.selected_path.clone());
    }
    if let Some(sp) = &s.split_path {
        keep.push(sp.clone());
    }
    let refs = || keep.iter().map(String::as_str);
    s.stashed_buffers = prune_stashed_buffers(s.stashed_buffers.as_ref(), refs());
    let views = s
        .file_views
        .as_ref()
        .map(|v| prune_file_views(v, refs(), MAX_STORED_VIEWS));
    s.file_views = views.filter(|v| !v.is_empty());
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn open_file_helpers() {
        assert_eq!(
            unique_open_files(&paths(&["a", "", "b", "a"])),
            paths(&["a", "b"])
        );
        assert_eq!(
            remap_open_files(&paths(&["d/x", "e", "d"]), "d", "f"),
            paths(&["f/x", "e", "f"])
        );
        assert_eq!(
            remap_open_files(&paths(&["a", "b"]), "a", "b"),
            paths(&["b"])
        );
        assert_eq!(
            max_scratch_seq(["scratch://3-x", "scratch://12-y", "a"]),
            12
        );
        assert_eq!(editor_content_hash("a\r\nb"), editor_content_hash("a\nb"));
        assert_eq!(
            editor_content_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn patches_are_change_gated() {
        let mut s = Session {
            open_files: paths(&["a.rs"]),
            selected_path: "a.rs".into(),
            ..Default::default()
        };
        s.stashed_buffers = Some(BTreeMap::from([(
            "a.rs".into(),
            StashedBuffer {
                draft: "x".into(),
                saved: Some("y".into()),
                base_hash: None,
            },
        )]));
        let n = normalize_session(&s);
        let (patch, sent) = build_patch(&n, None);
        let patch = patch.unwrap();
        assert!(patch.meta.is_some());
        assert_eq!(
            patch.buffers.unwrap()["a.rs"].base_hash,
            editor_content_hash("y")
        );
        let (again, _) = build_patch(&n, Some(&sent));
        assert!(again.is_none());
        let mut closed = n.clone();
        closed.stashed_buffers = None;
        let (drop, _) = build_patch(&closed, Some(&sent));
        assert_eq!(drop.unwrap().drop_buffers, Some(paths(&["a.rs"])));
    }

    #[test]
    fn wire_round_trip_and_restore() {
        let w = serde_json::json!({
            "openFiles": ["a", "a", "b"], "selectedPath": "b",
            "buffers": {"b": {"draft": "d", "baseHash": "h"}},
            "fileViews": {"b": {"line": 1, "col": 1}, "zzz": {"line": 1, "col": 1}},
            "leftKind": "scm", "rightOpen": false
        });
        let s = restorable_session(session_from_wire(&w).unwrap()).unwrap();
        assert_eq!(s.open_files, paths(&["a", "b"]));
        assert!(s.stashed_buffers.unwrap().contains_key("b"));
        assert_eq!(s.file_views.unwrap().len(), 1);
        assert!(s.left_kind.is_some());
        let empty = serde_json::json!({"openFiles": []});
        assert!(restorable_session(session_from_wire(&empty).unwrap()).is_none());
    }
}
