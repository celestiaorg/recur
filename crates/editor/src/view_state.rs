//! Per-file view state and buffer bookkeeping: caret/scroll/view mode,
//! dirty tracking, the Code/Diff/Preview chip and SCM base rules and the
//! `@file` mention span.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::selection::{EditorSelection, SelectionRange};
use crate::state::{EditorState, ScrollStrategy, ScrollTarget};
use crate::text::{Text, normalize_newlines};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorViewMode {
    #[default]
    Code,
    Preview,
    Diff,
}

/// Caret, scroll and view mode for one file (`FileViewState`). Lines and
/// columns are 1-based; columns count UTF-16 units, as stored in sessions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileViewState {
    pub view_mode: EditorViewMode,
    pub line: usize,
    pub col: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_col: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_line: Option<usize>,
    pub scroll_top: f64,
    pub scroll_left: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_scroll_top: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_scroll_left: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_zoom: Option<f64>,
}

pub const MAX_STORED_VIEWS: usize = 64;
/// Clean buffer bodies larger than this are not cached in memory.
pub const EDITOR_BODY_CAP: usize = 1024 * 1024;

/// `posAtLineCol`: clamp a 1-based line/column into the document.
pub fn pos_at_line_col(doc: &Text, line: usize, col: usize) -> usize {
    let ln = doc.line(line.clamp(1, doc.lines()));
    let offset = col.max(1) - 1;
    let text = doc.slice(ln.from, ln.to);
    let units: usize = text.chars().map(char::len_utf16).sum();
    doc.utf16_to_byte(doc.byte_to_utf16(ln.from) + offset.min(units))
}

/// 1-based column (UTF-16) of `pos` on its line.
pub fn col_of(doc: &Text, pos: usize) -> usize {
    let line = doc.line_at(pos);
    doc.byte_to_utf16(pos) - doc.byte_to_utf16(line.from) + 1
}

pub fn selection_from_view_state(doc: &Text, vs: Option<&FileViewState>) -> EditorSelection {
    let Some(vs) = vs else {
        return EditorSelection::cursor(0);
    };
    let head = pos_at_line_col(doc, vs.line, vs.col);
    let anchor = match vs.anchor_line {
        Some(l) => pos_at_line_col(doc, l, vs.anchor_col.unwrap_or(1)),
        None => head,
    };
    EditorSelection::single(anchor, head)
}

/// Scroll the saved top line to the top of the viewport.
pub fn scroll_target_from_view_state(
    doc: &Text,
    vs: Option<&FileViewState>,
) -> Option<ScrollTarget> {
    let vs = vs?;
    let top = pos_at_line_col(doc, vs.top_line.unwrap_or(vs.line), 1);
    Some(ScrollTarget::pos(top, ScrollStrategy::Start, 0.0))
}

/// `captureFileViewState`: `top_pos` is the document position at the top
/// of the viewport; `None` when the view has no geometry (hidden).
pub fn capture_file_view_state(
    state: &EditorState,
    view_mode: EditorViewMode,
    top_pos: Option<usize>,
    scroll: (f64, f64),
    has_geometry: bool,
) -> Option<FileViewState> {
    if !has_geometry {
        return None;
    }
    let sel = state.selection.main();
    let head_line = state.doc.line_at(sel.head());
    let mut out = FileViewState {
        view_mode,
        line: head_line.number,
        col: col_of(&state.doc, sel.head()),
        top_line: Some(top_pos.map_or(1, |p| state.doc.line_at(p).number)),
        scroll_top: scroll.0,
        scroll_left: scroll.1,
        ..Default::default()
    };
    if !sel.is_empty() {
        out.anchor_line = Some(state.doc.line_at(sel.anchor()).number);
        out.anchor_col = Some(col_of(&state.doc, sel.anchor()));
    }
    Some(out)
}

fn finite(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|n| n.is_finite())
}

/// `parseFileViewState`.
pub fn parse_file_view_state(raw: &Value) -> Option<FileViewState> {
    let o = raw.as_object()?;
    let view_mode = match o.get("viewMode").and_then(Value::as_str) {
        Some("preview") => EditorViewMode::Preview,
        Some("diff") => EditorViewMode::Diff,
        _ => EditorViewMode::Code,
    };
    let line = finite(o.get("line")).filter(|l| *l >= 1.0)?;
    let col = finite(o.get("col")).filter(|c| *c >= 1.0)?;
    let scroll_top = match o.get("scrollTop") {
        None | Some(Value::Null) => 0.0,
        v => finite(v).filter(|s| *s >= 0.0)?,
    };
    let scroll_left = finite(o.get("scrollLeft"))
        .filter(|s| *s > 0.0)
        .map_or(0.0, |s| s.min(1e9));
    let mut state = FileViewState {
        view_mode,
        line: line.floor().min(1e9) as usize,
        col: col.floor().min(1e6) as usize,
        scroll_top: scroll_top.min(1e9),
        scroll_left,
        ..Default::default()
    };
    if let Some(t) = finite(o.get("topLine")).filter(|t| *t >= 1.0) {
        state.top_line = Some(t.floor().min(1e9) as usize);
    }
    let anchor_line = finite(o.get("anchorLine")).filter(|l| *l >= 1.0);
    let anchor_col = finite(o.get("anchorCol")).filter(|c| *c >= 1.0);
    if let (Some(l), Some(c)) = (anchor_line, anchor_col) {
        state.anchor_line = Some(l.floor() as usize);
        state.anchor_col = Some(c.floor() as usize);
    }
    if let Some(v) = finite(o.get("previewScrollTop")).filter(|v| *v != 0.0) {
        state.preview_scroll_top = Some(v.floor().clamp(-1e9, 1e9));
    }
    if let Some(v) = finite(o.get("previewScrollLeft")).filter(|v| *v != 0.0) {
        state.preview_scroll_left = Some(v.floor().clamp(-1e9, 1e9));
    }
    if let Some(z) = finite(o.get("previewZoom")).filter(|z| *z > 0.0) {
        state.preview_zoom = Some(z.min(8.0));
    }
    Some(state)
}

/// `parseFileViews`.
pub fn parse_file_views(raw: &Value) -> BTreeMap<String, FileViewState> {
    let mut out = BTreeMap::new();
    if let Some(o) = raw.as_object() {
        for (path, v) in o {
            if path.is_empty() {
                continue;
            }
            if let Some(p) = parse_file_view_state(v) {
                out.insert(path.clone(), p);
            }
        }
    }
    out
}

/// `pruneFileViews`: the views of `keep_paths`, in that order, at most
/// `max`.
pub fn prune_file_views<'a>(
    views: &BTreeMap<String, FileViewState>,
    keep_paths: impl IntoIterator<Item = &'a str>,
    max: usize,
) -> BTreeMap<String, FileViewState> {
    let mut out = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for path in keep_paths {
        if !seen.insert(path) {
            continue;
        }
        if let Some(v) = views.get(path) {
            out.insert(path.to_string(), v.clone());
        }
        if out.len() >= max {
            return out;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RestoreModeOpts {
    pub want_diff: bool,
    pub deleted: bool,
    pub can_diff: bool,
    pub can_preview: bool,
    pub is_image: bool,
}

/// `resolveRestoredViewMode`.
pub fn resolve_restored_view_mode(
    saved: Option<EditorViewMode>,
    o: RestoreModeOpts,
) -> EditorViewMode {
    use EditorViewMode::*;
    if o.is_image {
        if o.deleted {
            return Diff;
        }
        if o.want_diff || saved == Some(Diff) {
            return if o.can_diff { Diff } else { Preview };
        }
        return Preview;
    }
    if o.deleted {
        return Diff;
    }
    if o.want_diff || saved == Some(Diff) {
        return if o.can_diff { Diff } else { Code };
    }
    if saved == Some(Preview) {
        return if o.can_preview { Preview } else { Code };
    }
    Code
}

/// `editorTextEq`: same document ignoring CR/LF differences.
pub fn editor_text_eq(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if !a.contains('\r') && !b.contains('\r') {
        return false;
    }
    normalize_newlines(a) == normalize_newlines(b)
}

/// `diskBaselineMoved`: the saved text differs from the clean baseline.
pub fn disk_baseline_moved(clean: Option<&Text>, saved: &str) -> bool {
    let Some(clean) = clean else { return true };
    let saved = normalize_newlines(saved);
    clean.len() != saved.len() || clean.to_string() != saved
}

/// `bufferMountState`: dirty with the clean baseline, or clean.
pub fn buffer_mount_state(initial: &str, saved: Option<&str>) -> Option<Text> {
    match saved {
        Some(s) if !editor_text_eq(s, initial) => Some(Text::of(s)),
        _ => None,
    }
}

/// `isBufferDirty`.
pub fn is_buffer_dirty(doc: &Text, clean: Option<&Text>) -> bool {
    match clean {
        None => false,
        Some(c) => doc != c,
    }
}

/// `scmGutterBase`: the base text for diff bars, or none. An empty base is
/// a new file only for untracked/added status.
pub fn scm_gutter_base(cached: Option<&str>, git_status: &str) -> Option<String> {
    let c = cached?;
    if c.is_empty() && git_status != "u" && git_status != "a" {
        return None;
    }
    Some(c.to_string())
}

fn file_name(path: &str) -> String {
    let base = path.trim().rsplit(['/', '\\']).next().unwrap_or("");
    let name = base.rfind('?').map_or(base, |q| &base[..q]);
    name.to_lowercase()
}

/// Raster images (SVG stays text).
pub fn is_binary_image_path(path: &str) -> bool {
    let n = file_name(path);
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".bmp", ".avif", ".tif", ".tiff",
    ]
    .iter()
    .any(|e| n.ends_with(e))
}

pub fn is_pdf_path(path: &str) -> bool {
    file_name(path).ends_with(".pdf")
}

pub fn is_video_path(path: &str) -> bool {
    let n = file_name(path);
    [".mp4", ".m4v", ".mov", ".webm", ".ogv"]
        .iter()
        .any(|e| n.ends_with(e))
}

pub fn is_media_path(path: &str) -> bool {
    is_pdf_path(path) || is_video_path(path)
}

/// `editorViewModes`: the chip's options in order.
pub fn editor_view_modes(
    has_file: bool,
    deleted: bool,
    can_show_diff: bool,
    has_preview: bool,
    is_image: bool,
) -> Vec<EditorViewMode> {
    let mut modes = Vec::new();
    if is_image {
        if has_file && !deleted {
            modes.push(EditorViewMode::Preview);
        }
        if can_show_diff {
            modes.push(EditorViewMode::Diff);
        }
        return modes;
    }
    if has_file && !deleted {
        modes.push(EditorViewMode::Code);
    }
    if can_show_diff {
        modes.push(EditorViewMode::Diff);
    }
    if has_preview && has_file && !deleted {
        modes.push(EditorViewMode::Preview);
    }
    modes
}

/// `nextEditorViewMode`.
pub fn next_editor_view_mode(
    current: EditorViewMode,
    modes: &[EditorViewMode],
) -> Option<EditorViewMode> {
    if modes.len() < 2 {
        return None;
    }
    let i = modes.iter().position(|m| *m == current).unwrap_or(0);
    Some(
        modes[if modes.contains(&current) {
            (i + 1) % modes.len()
        } else {
            0
        }],
    )
}

/// `mergeViewReady`.
pub fn merge_view_ready(
    selected: &str,
    base_doc_path: &str,
    can_show_diff: bool,
    missing_current: bool,
    editor_path: &str,
) -> bool {
    if selected.is_empty() || base_doc_path != selected || !can_show_diff {
        return false;
    }
    missing_current || editor_path == selected
}

/// `mergeViewCurrent`.
pub fn merge_view_current(
    missing_current: bool,
    editor_path: &str,
    selected: &str,
    draft: &str,
) -> String {
    if missing_current || editor_path != selected {
        String::new()
    } else {
        draft.to_string()
    }
}

/// `shouldHydrateRestoredCurrent`.
#[allow(clippy::too_many_arguments)]
pub fn should_hydrate_restored_current(
    selected: &str,
    editor_path: &str,
    selected_is_dir: bool,
    file_loading: bool,
    hydrate_failed_path: &str,
    prev_status: &str,
    next_status: &str,
) -> bool {
    if selected.is_empty() || editor_path == selected || selected_is_dir || file_loading {
        return false;
    }
    if hydrate_failed_path == selected {
        return false;
    }
    prev_status == "d" && !next_status.is_empty() && next_status != "d"
}

/// A 1-based inclusive span for an `@file` chip (`MentionSelection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MentionSelection {
    pub start_line: usize,
    pub end_line: usize,
    /// Columns (1-based, UTF-16) when the selection is not whole lines.
    pub cols: Option<(usize, usize)>,
}

/// `mentionSelectionFromState`.
pub fn mention_selection_from_state(state: &EditorState) -> Option<MentionSelection> {
    let sel = state.selection.main();
    if sel.is_empty() {
        return None;
    }
    let (from, to) = (sel.from, sel.to);
    let doc = &state.doc;
    let start = doc.line_at(from);
    let mut end = doc.line_at(to);
    if to > from && end.from == to {
        end = doc.line(start.number.max(end.number - 1));
    }
    if from == start.from && to >= end.to {
        return Some(MentionSelection {
            start_line: start.number,
            end_line: end.number,
            cols: None,
        });
    }
    let last_pos = from
        .max(to.saturating_sub(1))
        .min(end.from.max(end.to.saturating_sub(1)));
    let last_pos = doc.clamp_to_char(last_pos);
    Some(MentionSelection {
        start_line: start.number,
        end_line: end.number,
        cols: Some((
            col_of(doc, from),
            (doc.byte_to_utf16(last_pos) - doc.byte_to_utf16(end.from) + 1).max(1),
        )),
    })
}

/// `mentionRangeFromLoc`: document range for a mention location.
pub fn mention_range_from_loc(
    doc: &Text,
    start_line: usize,
    end_line: Option<usize>,
    start_col: usize,
    end_col: usize,
) -> Option<(usize, usize)> {
    if start_line < 1 || doc.lines() < 1 {
        return None;
    }
    let start_ln = start_line.min(doc.lines());
    let end_ln = match end_line {
        Some(e) if e > start_ln => e.min(doc.lines()),
        _ => start_ln,
    };
    if start_col > 0 && end_col > 0 {
        let from = pos_at_line_col(doc, start_ln, start_col);
        let line = doc.line(end_ln);
        let mut to = pos_at_line_col(doc, end_ln, end_col);
        if to < line.to {
            to += doc.char_at(to).map_or(1, char::len_utf8);
        }
        return Some((from, to.max(from)));
    }
    Some((doc.line(start_ln).from, doc.line(end_ln).to))
}

/// Selection for a mention click: the range, scrolled to the center.
pub fn mention_selection_spec(range: (usize, usize)) -> (EditorSelection, ScrollTarget) {
    (
        EditorSelection::of_range(SelectionRange::range(range.0, range.1)),
        ScrollTarget::pos(range.0, ScrollStrategy::Center, 5.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{StateConfig, TransactionSpec};

    #[test]
    fn view_state_round_trip() {
        let doc = Text::of("one\ntwo\nthree");
        assert_eq!(pos_at_line_col(&doc, 2, 3), 6);
        assert_eq!(pos_at_line_col(&doc, 9, 99), 13);
        let vs = parse_file_view_state(&serde_json::json!({"line": 2, "col": 2, "anchorLine": 1, "anchorCol": 1, "scrollTop": 10, "viewMode": "diff"})).unwrap();
        assert_eq!(vs.view_mode, EditorViewMode::Diff);
        let sel = selection_from_view_state(&doc, Some(&vs));
        assert_eq!((sel.main().anchor(), sel.main().head()), (0, 5));
        assert!(parse_file_view_state(&serde_json::json!({"line": 0, "col": 1})).is_none());
        let s = EditorState::create("one\ntwo", StateConfig::default());
        let s = s
            .update(TransactionSpec::default().selection(EditorSelection::single(1, 6)))
            .state()
            .clone();
        let cap =
            capture_file_view_state(&s, EditorViewMode::Code, Some(4), (0.0, 0.0), true).unwrap();
        assert_eq!(
            (
                cap.line,
                cap.col,
                cap.anchor_line,
                cap.anchor_col,
                cap.top_line
            ),
            (2, 3, Some(1), Some(2), Some(2))
        );
    }

    #[test]
    fn restore_modes_and_dirty() {
        let o = RestoreModeOpts {
            can_diff: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_restored_view_mode(Some(EditorViewMode::Diff), o),
            EditorViewMode::Diff
        );
        assert_eq!(
            resolve_restored_view_mode(Some(EditorViewMode::Preview), o),
            EditorViewMode::Code
        );
        assert!(editor_text_eq("a\r\nb", "a\nb"));
        assert!(!disk_baseline_moved(Some(&Text::of("a\nb")), "a\r\nb"));
        assert!(buffer_mount_state("x", Some("y")).is_some());
        assert!(is_buffer_dirty(&Text::of("x"), Some(&Text::of("y"))));
        assert_eq!(scm_gutter_base(Some(""), "m"), None);
        assert_eq!(scm_gutter_base(Some(""), "u"), Some(String::new()));
        assert_eq!(
            next_editor_view_mode(
                EditorViewMode::Diff,
                &[EditorViewMode::Code, EditorViewMode::Diff]
            ),
            Some(EditorViewMode::Code)
        );
        assert!(is_binary_image_path("a/b.PNG"));
        assert!(!is_binary_image_path("a/b.svg"));
    }

    #[test]
    fn mention_spans() {
        let s = EditorState::create("abc\ndef\nghi", StateConfig::default());
        let sel = |a, h| {
            s.update(TransactionSpec::default().selection(EditorSelection::single(a, h)))
                .state()
                .clone()
        };
        assert_eq!(
            mention_selection_from_state(&sel(0, 8)),
            Some(MentionSelection {
                start_line: 1,
                end_line: 2,
                cols: None
            })
        );
        assert_eq!(
            mention_selection_from_state(&sel(1, 6)),
            Some(MentionSelection {
                start_line: 1,
                end_line: 2,
                cols: Some((2, 2))
            })
        );
        assert_eq!(
            mention_range_from_loc(&s.doc, 1, Some(2), 2, 2),
            Some((1, 6))
        );
        assert_eq!(mention_range_from_loc(&s.doc, 2, None, 0, 0), Some((4, 7)));
    }
}
