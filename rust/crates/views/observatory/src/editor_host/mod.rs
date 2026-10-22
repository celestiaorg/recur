//! The editor host side of `Observatory.svelte` and `CodeEditor.svelte` / `MergeEditor.svelte`:
//! the main, split and merge `EditorView`s driven from the Observatory's buffers the way the
//! Svelte props drove CodeMirror (mount on path change, external doc drift kept out of undo,
//! read-only, findings and SCM gutter, saved baseline), per-workspace tab parking across
//! root switches, the file preview and PDF pane, and the editor's events.

pub(crate) mod commands;
mod open;
mod save;

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{App, AppContext as _, Context, Entity, Window};
use recur_content::preview::{PreviewKind, file_preview_find_active, preview_kind_for_path};
use recur_content::{FilePreview, FilePreviewEvent};
use recur_editor::gutters::FindingMark;
use recur_editor::session::StashedBuffer;
use recur_editor::view::{EditorEvent, EditorKind, EditorOptions, EditorView, TabSnapshot};
use recur_editor::view_state::{
    EDITOR_BODY_CAP, EditorViewMode, FileViewState, editor_text_eq, editor_view_modes,
    is_binary_image_path, is_media_path, is_pdf_path, merge_view_current, merge_view_ready,
    scm_gutter_base,
};
use recur_view_core::file_drag::is_abs_filesystem_path;

use crate::model::{MAX_EDIT_BYTES, Pane, is_scratch_path};
use crate::obs::Observatory;
use crate::workspace_file::is_binary_preview_mime;

pub(crate) use crate::textmate::editor_prefs;

/// What a code pane last applied (`CodeEditor`'s `mounted*` state and parked session).
#[derive(Clone, Debug, Default)]
pub struct EditorSync {
    /// `mountedSession`: the workspace whose tabs the view holds.
    pub session: String,
    /// `parkedOutgoing`: the host already parked that session.
    pub parked_outgoing: String,
    /// `mountedPath`.
    pub path: String,
    /// `mountedDoc`: the `doc` prop last applied.
    pub doc: String,
    pub read_only: bool,
    /// The debounced text the editor last emitted (a `doc` equal to it is a local echo).
    pub last_emitted: Option<String>,
    /// `pendingViewState`: applied once the document has text.
    pub pending_view: Option<FileViewState>,
    /// `restoredLive`: the mount restored a parked tab, so a restore is a no-op.
    pub restored_live: bool,
    pub findings: Vec<FindingMark>,
    pub base: Option<Option<String>>,
    pub saved: Option<String>,
    pub lit: Option<bool>,
    pub env_epoch: u64,
    pub lsp_uri: String,
    /// The theme last handed to the view (its `Rc` address; 0 for none).
    pub theme_ptr: Option<usize>,
}

/// The merge pane's key (`{#key selectedPath + '\0' + baseDocPath}`) and its mounted props.
#[derive(Clone, Debug, Default)]
pub struct MergeSync {
    pub key: (String, String),
    pub orig_key: String,
    pub doc: String,
    pub read_only: bool,
    pub last_emitted: Option<String>,
    pub env_epoch: u64,
}

/// `origKey`: cheap identity of the base text.
fn orig_key(orig: &str) -> String {
    let n = orig.len();
    let head: String = orig.chars().take(64).collect();
    let tail: String = {
        let c: Vec<char> = orig.chars().collect();
        c[c.len().saturating_sub(64)..].iter().collect()
    };
    format!("{n}\0{head}\0{tail}")
}

/// `forgetEditorHistoryTree`.
pub(crate) fn forget_history_tree(scope: &str, path: &str, cx: &mut App) {
    cx.default_global::<recur_editor::history_cache::EditorHistory>()
        .0
        .forget_tree(scope, path);
}

/// `remapEditorHistory`.
pub(crate) fn remap_history(scope: &str, from: &str, to: &str, cx: &mut App) {
    cx.default_global::<recur_editor::history_cache::EditorHistory>()
        .0
        .remap(scope, from, to);
}

/// `parkableTabs`: huge buffers skip the process-wide park.
fn parkable(tabs: Vec<(String, TabSnapshot)>) -> Vec<(String, TabSnapshot)> {
    tabs.into_iter()
        .filter(|(_, s)| s.state.doc.byte_to_utf16(s.state.doc.len()) <= EDITOR_BODY_CAP)
        .collect()
}

impl Observatory {
    // ---- derived (the editor `$derived`s) ----

    pub(crate) fn file_is_binary_preview(&self) -> bool {
        self.file
            .as_ref()
            .is_some_and(|f| is_binary_preview_mime(&f.mime))
    }

    /// `dirty`.
    pub(crate) fn dirty(&self) -> bool {
        self.file
            .as_ref()
            .is_some_and(|f| !is_binary_preview_mime(&f.mime))
            && (self.main_editor_dirty || !editor_text_eq(&self.draft, &self.saved_content))
    }

    /// `splitDirty`.
    pub(crate) fn split_dirty(&self) -> bool {
        !self.split_path.is_empty()
            && (self.split_editor_dirty || !editor_text_eq(&self.split_draft, &self.split_saved))
    }

    pub(crate) fn active_dirty(&self) -> bool {
        if self.focused_pane == Pane::Split && self.split_open {
            self.split_dirty()
        } else {
            self.dirty()
        }
    }

    /// `writable`: the worktree edit bit, editable mime and under the edit cap.
    pub(crate) fn writable(&self) -> bool {
        self.file.as_ref().is_some_and(|f| {
            f.writable && !is_binary_preview_mime(&f.mime) && f.size <= MAX_EDIT_BYTES
        })
    }

    pub(crate) fn can_show_diff(&self) -> bool {
        !self.selected_git_status().is_empty()
            && !is_binary_image_path(&self.selected_path)
            && !is_media_path(&self.selected_path)
    }

    pub(crate) fn can_show_image_diff(&self) -> bool {
        let st = self.selected_git_status();
        is_binary_image_path(&self.selected_path) && (st == "m" || st == "d")
    }

    fn merge_missing_current(&self) -> bool {
        self.selected_git_status() == "d"
            || self.selected_is_dir
            || self.hydrate_failed_path == self.selected_path
    }

    pub(crate) fn merge_current(&self) -> String {
        merge_view_current(
            self.merge_missing_current(),
            &self.editor_path,
            &self.selected_path,
            &self.draft,
        )
    }

    pub(crate) fn merge_ready(&self) -> bool {
        merge_view_ready(
            &self.selected_path,
            &self.base_doc_path,
            self.can_show_diff(),
            self.merge_missing_current(),
            &self.editor_path,
        )
    }

    pub(crate) fn show_diff(&self) -> bool {
        self.view_mode == EditorViewMode::Diff && self.merge_ready()
    }

    /// Diff chosen but base or current still loading.
    pub(crate) fn diff_pending(&self) -> bool {
        self.view_mode == EditorViewMode::Diff
            && !self.selected_path.is_empty()
            && !self.merge_ready()
            && !is_binary_image_path(&self.selected_path)
            && !is_media_path(&self.selected_path)
    }

    pub(crate) fn is_image(&self) -> bool {
        self.file
            .as_ref()
            .is_some_and(|f| f.mime.starts_with("image/"))
    }

    pub(crate) fn is_pdf(&self) -> bool {
        self.file
            .as_ref()
            .is_some_and(|f| f.mime == "application/pdf")
            || is_pdf_path(&self.selected_path)
    }

    pub(crate) fn is_video(&self) -> bool {
        self.file
            .as_ref()
            .is_some_and(|f| f.mime.starts_with("video/"))
    }

    fn file_is_selected(&self) -> bool {
        self.file
            .as_ref()
            .is_some_and(|f| f.path == self.selected_path)
    }

    pub(crate) fn show_image_pane(&self) -> bool {
        let st = self.selected_git_status();
        (self.is_image() && self.file_is_selected() && st != "d")
            || (is_binary_image_path(&self.selected_path) && st == "d")
    }

    pub(crate) fn show_pdf_pane(&self) -> bool {
        self.is_pdf() && self.file_is_selected() && self.selected_git_status() != "d"
    }

    pub(crate) fn show_video_pane(&self) -> bool {
        self.is_video() && self.file_is_selected() && self.selected_git_status() != "d"
    }

    pub(crate) fn show_split_layout(&self) -> bool {
        self.split_open
            && !self.show_diff()
            && !self.show_image_pane()
            && !self.show_pdf_pane()
            && !self.show_video_pane()
    }

    pub(crate) fn preview_kind(&self) -> Option<PreviewKind> {
        let f = self.file.as_ref()?;
        if is_binary_preview_mime(&f.mime) {
            return None;
        }
        preview_kind_for_path(&f.path)
    }

    pub(crate) fn show_preview(&self) -> bool {
        self.preview_kind().is_some() && self.view_mode == EditorViewMode::Preview
    }

    /// `availableViewModes` (⌘⇧V cycles them).
    pub(crate) fn available_view_modes(&self) -> Vec<EditorViewMode> {
        let p = &self.selected_path;
        let st = self.selected_git_status();
        let raster = is_binary_image_path(p) || is_media_path(p);
        editor_view_modes(
            self.file.is_some() || (raster && st != "d"),
            st == "d",
            self.can_show_diff() || self.can_show_image_diff(),
            if raster {
                st != "d"
            } else {
                self.preview_kind().is_some()
            },
            self.is_image()
                || is_binary_image_path(p)
                || self.is_pdf()
                || self.is_video()
                || is_media_path(p),
        )
    }

    /// `unsavedPaths`.
    pub(crate) fn unsaved_paths(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.dirty() && !self.selected_path.is_empty() {
            out.push(self.selected_path.clone());
        }
        if self.split_dirty() && self.split_open && !self.split_path.is_empty() {
            out.push(self.split_path.clone());
        }
        out
    }

    pub(crate) fn can_toggle_find(&self) -> bool {
        if self.show_image_pane() || self.show_pdf_pane() || self.show_video_pane() {
            return false;
        }
        if self.show_diff() {
            return !self.selected_path.is_empty();
        }
        if self.focused_pane == Pane::Split && self.split_open {
            return !self.split_path.is_empty();
        }
        if self.show_preview() && self.preview_kind() == Some(PreviewKind::Svg) {
            return false;
        }
        !self.selected_path.is_empty()
    }

    /// `findingMarksForPath`: open findings as gutter marks.
    pub(crate) fn finding_marks_for_path(&self, path: &str, cx: &App) -> Vec<FindingMark> {
        if path.is_empty() {
            return Vec::new();
        }
        self.findings
            .iter()
            .filter(|f| {
                (f.status.is_empty() || f.status == "open")
                    && !f.id.is_empty()
                    && !f.file.is_empty()
                    && f.line >= 1
                    && self.finding_rel_path(&f.file, cx) == path
            })
            .map(|f| FindingMark {
                id: f.id.clone(),
                line: f.line as usize,
                end_line: (f.end_line > f.line).then_some(f.end_line as usize),
                severity: if f.severity.is_empty() {
                    "info".into()
                } else {
                    f.severity.to_string()
                },
                title: recur_view_chat::findings_ui::finding_title(&f.message),
            })
            .collect()
    }

    /// `gutterBaseFor`: the cached base text for the SCM bars, if loaded.
    pub(crate) fn gutter_base_for(&self, path: &str, cx: &App) -> Option<String> {
        if path.is_empty()
            || !self.is_git(cx)
            || is_scratch_path(path)
            || is_abs_filesystem_path(path)
        {
            return None;
        }
        let cached = self
            .base_doc_cache
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.as_str())?;
        scm_gutter_base(
            Some(cached),
            self.git_status.get(path).map_or("", String::as_str),
        )
    }

    /// The editor that takes commands, snippets and ⌘L (`focusedEditor`).
    pub(crate) fn focused_editor(&self) -> Option<Entity<EditorView>> {
        if self.focused_pane == Pane::Split && self.split_open
            && let Some(e) = &self.split_editor {
                return Some(e.clone());
            }
        self.main_editor.clone()
    }

    fn editor_for(&self, pane: Pane) -> Option<&Entity<EditorView>> {
        match pane {
            Pane::Main => self.main_editor.as_ref(),
            Pane::Split => self.split_editor.as_ref(),
        }
    }

    fn sync_for(&mut self, pane: Pane) -> &mut EditorSync {
        match pane {
            Pane::Main => &mut self.main_editor_sync,
            Pane::Split => &mut self.split_editor_sync,
        }
    }

    // ---- the code panes ----

    /// The props `<CodeEditor>` gets for a pane, or `None` when it is not mounted.
    #[allow(clippy::type_complexity)]
    fn code_pane_props(
        &self,
        pane: Pane,
        cx: &App,
    ) -> Option<(
        String,
        String,
        Option<String>,
        bool,
        Option<FileViewState>,
        Vec<FindingMark>,
        Option<String>,
        bool,
    )> {
        match pane {
            Pane::Main => {
                if self.show_image_pane()
                    || (self.show_pdf_pane() && self.file.is_some())
                    || (self.show_video_pane() && self.file.is_some())
                    || self.editor_path.is_empty()
                {
                    return None;
                }
                let path = self.editor_path.clone();
                let (doc, saved) = if path == self.selected_path {
                    (self.draft.clone(), self.saved_content.clone())
                } else {
                    let b = self.tab_buffers.get(&path);
                    (
                        b.map_or_else(|| self.draft.clone(), |b| b.draft.clone()),
                        b.map_or_else(|| self.saved_content.clone(), |b| b.saved.clone()),
                    )
                };
                let ro = !self.writable()
                    || self.show_preview()
                    || self.show_diff()
                    || self.view_mode == EditorViewMode::Diff;
                let vs = if self.skip_view_restore {
                    None
                } else {
                    self.file_views.get(&path).cloned()
                };
                let marks = {
                    let p = if self.editor_path.is_empty() {
                        &self.selected_path
                    } else {
                        &self.editor_path
                    };
                    self.finding_marks_for_path(p, cx)
                };
                let base = self.gutter_base_for(&path, cx);
                let lit = self.focus_zone == crate::model::FocusZone::Editor
                    && self.focused_pane == Pane::Main;
                Some((path, doc, Some(saved), ro, vs, marks, base, lit))
            }
            Pane::Split => {
                if !self.show_split_layout() || self.split_path.is_empty() {
                    return None;
                }
                let path = self.split_path.clone();
                Some((
                    path.clone(),
                    self.split_draft.clone(),
                    Some(self.split_saved.clone()),
                    !self.split_writable,
                    self.file_views.get(&path).cloned(),
                    self.finding_marks_for_path(&path, cx),
                    self.gutter_base_for(&path, cx),
                    self.focus_zone == crate::model::FocusZone::Editor
                        && self.focused_pane == Pane::Split,
                ))
            }
        }
    }

    fn build_editor_options(&self, pane: &str, path: &str, doc: &str, cx: &App) -> EditorOptions {
        let env = crate::textmate::editor_env_peek(cx);
        let mut o = EditorOptions::new(path, doc);
        o.pane = pane.to_string();
        o.history_scope = self.editor_session_key.clone();
        if let Some(env) = env {
            let env = env.read(cx);
            o.prefs = env.prefs.clone();
            o.theme = env.theme.clone();
            o.dark_fallback = env.dark_fallback;
            o.textmate = Some(env.tm.clone());
            o.snippets =
                Some(env.snippets.clone() as Rc<dyn recur_editor::snippet::SnippetProvider>);
        }
        o
    }

    #[allow(clippy::too_many_arguments)]
    fn create_code_editor(
        &mut self,
        pane: Pane,
        path: &str,
        doc: &str,
        saved: Option<&str>,
        ro: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<EditorView> {
        let mut o = self.build_editor_options(pane.key(), path, doc, cx);
        o.read_only = ro;
        o.saved_doc = saved.map(str::to_string);
        let ed = cx.new(|cx| EditorView::new(o, window, cx));
        let sub = cx.subscribe_in(&ed, window, move |obs, _, ev: &EditorEvent, window, cx| {
            obs.on_editor_event(pane, ev, window, cx)
        });
        self.editor_subs.push(sub);
        ed
    }

    /// Mirror the editor props onto every editor and preview (`CodeEditor` / `MergeEditor` /
    /// `FilePreview` `$effect`s). Runs before each paint and after state changes.
    pub(crate) fn sync_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let epoch = crate::textmate::editor_env_peek(cx).map_or(0, |e| e.read(cx).epoch);
        for pane in [Pane::Main, Pane::Split] {
            self.sync_code_pane(pane, epoch, window, cx);
        }
        self.sync_merge_editor(epoch, window, cx);
        self.sync_file_preview(window, cx);
        self.sync_media(window, cx);
    }

    fn unmount_code_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let ed = match pane {
            Pane::Main => self.main_editor.take(),
            Pane::Split => self.split_editor.take(),
        };
        let Some(ed) = ed else { return };
        let sync = std::mem::take(self.sync_for(pane));
        // onDestroy: park the session unless the host already did, capture history.
        let tabs = ed.update(cx, |e, cx| {
            e.capture_history(cx);
            e.take_tabs()
        });
        if !sync.session.is_empty() && sync.parked_outgoing != sync.session {
            crate::session_tabs::parks(cx)
                .pane(pane.key())
                .park(&sync.session, &parkable(tabs));
        }
        if pane == Pane::Main {
            self.lsp_editor_detached(cx);
        }
    }

    fn sync_code_pane(
        &mut self,
        pane: Pane,
        epoch: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((path, doc, saved, ro, vs, marks, base, lit)) = self.code_pane_props(pane, cx)
        else {
            self.unmount_code_pane(pane, cx);
            return;
        };
        let session = self.editor_session_key.clone();
        let ed = match self.editor_for(pane).cloned() {
            Some(ed) => ed,
            None => {
                // First mount: adopt this session's parked tabs, then mount the path.
                let parked = crate::session_tabs::parks(cx)
                    .pane(pane.key())
                    .take(&session)
                    .unwrap_or_default();
                let cached = parked.iter().any(|(p, _)| *p == path);
                let ed = self.create_code_editor(pane, "", "", None, ro, window, cx);
                ed.update(cx, |e, cx| {
                    e.adopt_tabs(parked);
                    e.mount_path(&path, &doc, saved.as_deref(), window, cx);
                });
                match pane {
                    Pane::Main => self.main_editor = Some(ed.clone()),
                    Pane::Split => self.split_editor = Some(ed.clone()),
                }
                let s = self.sync_for(pane);
                *s = EditorSync {
                    session: session.clone(),
                    path: path.clone(),
                    doc: doc.clone(),
                    read_only: ro,
                    restored_live: cached,
                    env_epoch: epoch,
                    ..Default::default()
                };
                if !cached {
                    s.pending_view = vs.clone();
                }
                self.after_mount(pane, &ed, &doc, cached, window, cx);
                self.apply_gutters(pane, &ed, marks, base, saved, lit, window, cx);
                return;
            }
        };
        // `adoptSession`: a root switch swaps the tab map under the same view.
        if self.sync_for(pane).session != session {
            let prev = self.sync_for(pane).clone();
            let outgoing = ed.update(cx, |e, cx| {
                e.capture_history(cx);
                e.take_tabs()
            });
            if prev.parked_outgoing != prev.session && !prev.session.is_empty() {
                crate::session_tabs::parks(cx)
                    .pane(pane.key())
                    .park(&prev.session, &parkable(outgoing));
            }
            let restored = crate::session_tabs::parks(cx)
                .pane(pane.key())
                .take(&session)
                .unwrap_or_default();
            ed.update(cx, |e, _| {
                e.set_history_scope(&session);
                e.adopt_tabs(restored);
            });
            let s = self.sync_for(pane);
            s.session = session.clone();
            s.parked_outgoing.clear();
            s.path.clear();
        }
        if self.sync_for(pane).env_epoch != epoch {
            self.sync_for(pane).env_epoch = epoch;
            self.apply_env(&ed, pane, window, cx);
        }
        if self.sync_for(pane).path != path {
            let cached = ed.read(cx).has_tab(&path);
            ed.update(cx, |e, cx| {
                e.set_read_only(ro, window, cx);
                e.mount_path(&path, &doc, saved.as_deref(), window, cx);
            });
            let s = self.sync_for(pane);
            s.path = path.clone();
            s.doc = doc.clone();
            s.read_only = ro;
            s.last_emitted = None;
            s.restored_live = cached;
            s.pending_view = if cached { None } else { vs.clone() };
            s.findings.clear();
            s.base = None;
            s.saved = None;
            self.after_mount(pane, &ed, &doc, cached, window, cx);
            self.apply_gutters(pane, &ed, marks, base, saved, lit, window, cx);
            return;
        }
        // Same path: external drift (not a local echo), then read-only.
        let s = self.sync_for(pane).clone();
        if doc != s.doc && s.last_emitted.as_deref() != Some(doc.as_str()) {
            let live_empty = ed.read(cx).state().doc.is_empty();
            ed.update(cx, |e, cx| e.external_doc_change(&doc, window, cx));
            if live_empty
                && !doc.is_empty()
                && let Some(vs) = self.sync_for(pane).pending_view.take()
            {
                ed.update(cx, |e, cx| e.apply_view_state(&vs, window, cx));
            }
            self.sync_for(pane).last_emitted = Some(doc.clone());
        }
        self.sync_for(pane).doc = doc;
        if ro != s.read_only {
            self.sync_for(pane).read_only = ro;
            ed.update(cx, |e, cx| e.set_read_only(ro, window, cx));
        }
        self.apply_gutters(pane, &ed, marks, base, saved, lit, window, cx);
    }

    /// After a mount: the pending view state, else a remeasure.
    fn after_mount(
        &mut self,
        pane: Pane,
        ed: &Entity<EditorView>,
        doc: &str,
        cached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cached {
            return;
        }
        if doc.is_empty() {
            return;
        }
        if let Some(vs) = self.sync_for(pane).pending_view.take() {
            ed.update(cx, |e, cx| e.apply_view_state(&vs, window, cx));
            // Applied: Observatory's restore after the open must not scroll again.
            self.sync_for(pane).restored_live = true;
        } else if let Some((top, left)) = ed.read(cx).restored_scroll() {
            ed.update(cx, |e, cx| e.set_scroll(top, left, cx));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_gutters(
        &mut self,
        pane: Pane,
        ed: &Entity<EditorView>,
        marks: Vec<FindingMark>,
        base: Option<String>,
        saved: Option<String>,
        lit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sync_for(pane).findings != marks {
            self.sync_for(pane).findings = marks.clone();
            ed.update(cx, |e, cx| e.set_findings(marks, window, cx));
        }
        if self.sync_for(pane).base.as_ref() != Some(&base) {
            self.sync_for(pane).base = Some(base.clone());
            ed.update(cx, |e, cx| e.set_diff_base(base.as_deref(), window, cx));
        }
        if let Some(saved) = saved
            && self.sync_for(pane).saved.as_ref() != Some(&saved)
        {
            self.sync_for(pane).saved = Some(saved.clone());
            ed.update(cx, |e, cx| e.apply_saved_baseline(&saved, cx));
        }
        if self.sync_for(pane).lit != Some(lit) {
            self.sync_for(pane).lit = Some(lit);
            ed.update(cx, |e, cx| e.set_lit(lit, cx));
        }
    }

    /// Theme, prefs, grammars or syntax overrides moved: re-apply, and drop clean parked tabs
    /// built under the previous palette.
    fn apply_env(
        &mut self,
        ed: &Entity<EditorView>,
        pane: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(env) = crate::textmate::editor_env_peek(cx) else {
            return;
        };
        let (theme, dark, prefs) = {
            let env = env.read(cx);
            (env.theme.clone(), env.dark_fallback, env.prefs.clone())
        };
        let ptr = theme.as_ref().map_or(0, |t| Rc::as_ptr(t) as usize);
        let theme_changed = self.sync_for(pane).theme_ptr != Some(ptr);
        self.sync_for(pane).theme_ptr = Some(ptr);
        ed.update(cx, |e, cx| {
            if theme_changed {
                e.set_theme(theme.clone(), dark, window, cx);
                let keep: Vec<String> = e
                    .cached_buffers()
                    .into_iter()
                    .filter(|(_, _, dirty)| *dirty)
                    .map(|(p, _, _)| p)
                    .collect();
                e.retain_paths(&keep);
            }
            e.set_prefs(prefs, window, cx);
            e.ensure_language(window, cx);
        });
        if theme_changed {
            let parks = crate::session_tabs::parks(cx);
            parks.prune_all_tabs(|s| s.dirty);
        }
    }

    /// An image, PDF or video pane replaces the editor surfaces.
    fn media_pane_shown(&self) -> bool {
        self.show_image_pane()
            || (self.file.is_some() && (self.show_pdf_pane() || self.show_video_pane()))
    }

    // ---- the merge pane ----

    fn sync_merge_editor(&mut self, epoch: u64, window: &mut Window, cx: &mut Context<Self>) {
        let shown = !self.media_pane_shown() && self.show_diff();
        if !shown {
            if let Some((_, ed)) = self.merge_editor.take() {
                ed.update(cx, |e, cx| e.capture_history(cx));
            }
            self.merge_sync = MergeSync::default();
            return;
        }
        let key = (self.selected_path.clone(), self.base_doc_path.clone());
        let doc = self.merge_current();
        let orig = self.base_doc.clone();
        let ro = !self.writable() || self.selected_git_status() == "d";
        let okey = orig_key(&orig);
        let existing = self.merge_editor.as_ref().map(|(_, e)| e.clone());
        let remount = match &existing {
            None => true,
            Some(_) => self.merge_sync.key != key || self.merge_sync.orig_key != okey,
        };
        let drift = !remount
            && doc != self.merge_sync.doc
            && self.merge_sync.last_emitted.as_deref() != Some(doc.as_str());
        if remount || drift {
            // Same path rebuilt (base refreshed, or the doc moved under it): the live
            // position outranks the saved view state.
            let mut vs = if self.skip_view_restore {
                None
            } else {
                self.file_views.get(&self.selected_path).cloned()
            };
            let mut refocus = false;
            if let Some(old) = &existing {
                old.update(cx, |e, cx| e.capture_history(cx));
                if self.merge_sync.key.0 == key.0 {
                    if let Some(live) = old.read(cx).get_view_state(window) {
                        vs = Some(live);
                    }
                    refocus = old.read(cx).has_focus(window);
                }
            }
            let mut o = self.build_editor_options("main", &key.0, &doc, cx);
            o.kind = EditorKind::Merge {
                original: orig.clone(),
            };
            o.read_only = ro;
            o.view_state = vs;
            let ed = cx.new(|cx| EditorView::new(o, window, cx));
            let sub = cx.subscribe_in(&ed, window, |obs, _, ev: &EditorEvent, window, cx| {
                obs.on_merge_event(ev, window, cx)
            });
            self.editor_subs.push(sub);
            if refocus {
                ed.update(cx, |e, cx| e.focus(window, cx));
            }
            self.merge_editor = Some((key.0.clone(), ed));
            self.merge_sync = MergeSync {
                key,
                orig_key: okey,
                doc: doc.clone(),
                read_only: ro,
                last_emitted: Some(doc),
                env_epoch: epoch,
            };
            return;
        }
        let Some(ed) = existing else { return };
        if self.merge_sync.read_only != ro {
            self.merge_sync.read_only = ro;
            ed.update(cx, |e, cx| e.set_read_only(ro, window, cx));
        }
        self.merge_sync.doc = doc;
        if self.merge_sync.env_epoch != epoch {
            self.merge_sync.env_epoch = epoch;
            if let Some(env) = crate::textmate::editor_env_peek(cx) {
                let (theme, dark, prefs) = {
                    let env = env.read(cx);
                    (env.theme.clone(), env.dark_fallback, env.prefs.clone())
                };
                ed.update(cx, |e, cx| {
                    e.set_theme(theme, dark, window, cx);
                    e.set_prefs(prefs, window, cx);
                    e.ensure_language(window, cx);
                });
            }
        }
    }

    // ---- previews ----

    fn sync_file_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown_branch = !self.media_pane_shown();
        let kind = self.preview_kind().filter(|_| shown_branch);
        let Some(kind) = kind else {
            self.file_preview = None;
            return;
        };
        let loaded = self
            .file
            .as_ref()
            .map(|f| f.path.clone())
            .unwrap_or_default();
        let active = file_preview_find_active(
            self.show_preview(),
            self.props.shell_active,
            self.main_surface(cx).key(),
            self.term_overlaying(),
            self.editor_hidden(cx),
        );
        let ws = self.props.workspace_id.clone();
        let (path, draft) = (self.selected_path.clone(), self.draft.clone());
        let fp = match &self.file_preview {
            Some((k, fp)) if *k == kind => fp.clone(),
            _ => {
                let scroll = self.preview_scroll.clone();
                let fp = cx.new(|cx| {
                    FilePreview::new(kind, draft.clone(), path.clone(), scroll, window, cx)
                });
                let sub = cx.subscribe_in(&fp, window, |obs, _, ev: &FilePreviewEvent, _, cx| {
                    let FilePreviewEvent::SearchOpenChanged(_) = ev;
                    obs.sync_find_panel_open(cx);
                });
                self.editor_subs.push(sub);
                self.file_preview = Some((kind, fp.clone()));
                fp
            }
        };
        fp.update(cx, |p, cx| {
            p.set_path(&path, &loaded, &ws, cx);
            p.set_content(draft, window, cx);
            p.set_parked(!self.props.shell_active);
            p.set_active(active, window, cx);
        });
    }

    // ---- editor events ----

    fn on_editor_event(
        &mut self,
        pane: Pane,
        ev: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            EditorEvent::DocChanged(v) => {
                self.sync_for(pane).last_emitted = Some(v.clone());
                self.sync_for(pane).doc = v.clone();
                match pane {
                    Pane::Main => self.on_main_doc_change(v.clone(), cx),
                    Pane::Split => self.on_split_doc_change(v.clone(), cx),
                }
                self.notify_secondary_lsp(pane, v, cx);
            }
            EditorEvent::DirtyChanged(d) => {
                let d = *d;
                match pane {
                    Pane::Main => {
                        self.main_editor_dirty = d;
                        let p = self.editor_path.clone();
                        self.mark_tab_dirty(&p, d);
                        if !d && !p.is_empty() {
                            self.drop_stash(&p);
                        }
                    }
                    Pane::Split => {
                        self.split_editor_dirty = d;
                        let p = self.split_path.clone();
                        self.mark_tab_dirty(&p, d);
                        if !d && !p.is_empty() {
                            self.drop_stash(&p);
                        }
                    }
                }
                self.persist_editor_session(cx);
                cx.notify();
            }
            EditorEvent::Save => self.save_edit(window, cx),
            EditorEvent::CopyLine(line) => self.copy_line(*line, cx),
            EditorEvent::Cursor { line, col } => {
                if self.focused_pane == pane {
                    self.cursor_line = *line;
                    self.cursor_col = *col;
                    cx.notify();
                }
            }
            EditorEvent::GotoDefinition(word) => self.goto_def(word, window, cx),
            EditorEvent::SearchOpenChanged(_) => self.sync_find_panel_open(cx),
            EditorEvent::OpenFinding(id) => self.open_finding_from_editor(id, window, cx),
            EditorEvent::PickSnippet { snippets, from, to } => {
                let ed = self.editor_for(pane).cloned();
                self.pick_snippet(ed, snippets.clone(), *from, *to, window, cx);
            }
            EditorEvent::Announce(_) => {}
            EditorEvent::Toast { error, text } => recur_view_core::toast(
                if *error {
                    recur_ui::widgets::ToastKind::Error
                } else {
                    recur_ui::widgets::ToastKind::Info
                },
                text.clone(),
                cx,
            ),
        }
    }

    fn on_merge_event(&mut self, ev: &EditorEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            EditorEvent::DocChanged(v) => {
                self.merge_sync.last_emitted = Some(v.clone());
                self.merge_sync.doc = v.clone();
                self.on_main_doc_change(v.clone(), cx);
            }
            EditorEvent::Save => self.save_edit(window, cx),
            EditorEvent::CopyLine(line) => self.copy_line(*line, cx),
            EditorEvent::SearchOpenChanged(_) => self.sync_find_panel_open(cx),
            EditorEvent::PickSnippet { snippets, from, to } => {
                let ed = self.merge_editor.as_ref().map(|(_, e)| e.clone());
                self.pick_snippet(ed, snippets.clone(), *from, *to, window, cx);
            }
            EditorEvent::Toast { error, text } => recur_view_core::toast(
                if *error {
                    recur_ui::widgets::ToastKind::Error
                } else {
                    recur_ui::widgets::ToastKind::Info
                },
                text.clone(),
                cx,
            ),
            _ => {}
        }
    }

    /// Several snippets share a tab trigger: the keybinding menu picks one.
    fn pick_snippet(
        &mut self,
        ed: Option<Entity<EditorView>>,
        snippets: Vec<recur_editor::snippet::RegisteredSnippet>,
        from: usize,
        to: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ed) = ed else { return };
        let items: Vec<crate::keybinding_menu::KeybindingMenuItem> = snippets
            .iter()
            .map(|s| crate::keybinding_menu::KeybindingMenuItem {
                id: s.id.clone(),
                label: s.label.clone(),
                category: s
                    .bundle_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Snippets".into()),
                order: s.order,
            })
            .collect();
        crate::keybinding_menu::open_keybinding_menu(
            items,
            Rc::new(move |item, window, cx| {
                let Some(sn) = snippets.iter().find(|s| s.id == item.id) else {
                    return;
                };
                ed.update(cx, |e, cx| e.apply_snippet_at(sn, from, to, window, cx));
            }),
            cx,
        );
    }

    /// `onMainDocChange`.
    pub(crate) fn on_main_doc_change(&mut self, v: String, cx: &mut Context<Self>) {
        self.draft = v.clone();
        let buf_path = if self.editor_path.is_empty() {
            self.selected_path.clone()
        } else {
            self.editor_path.clone()
        };
        if !buf_path.is_empty()
            && let Some(file) = self.file.clone()
        {
            let saved = self.saved_content.clone();
            self.put_tab_buffer(
                &buf_path,
                crate::model::TabBuffer {
                    draft: v.clone(),
                    saved,
                    file,
                },
            );
        }
        if self.split_open && self.split_path == self.selected_path {
            self.split_draft = v;
        }
        self.sync_scratch_docs();
        self.schedule_lsp_outline(cx);
        self.persist_editor_session(cx);
        cx.notify();
    }

    /// `onSplitDocChange`.
    pub(crate) fn on_split_doc_change(&mut self, v: String, cx: &mut Context<Self>) {
        self.split_draft = v.clone();
        if self.split_path == self.selected_path {
            self.draft = v.clone();
        }
        let buf_file = if self.split_path == self.selected_path {
            self.file.clone()
        } else {
            self.tab_buffers
                .get(&self.split_path)
                .map(|b| b.file.clone())
        };
        if !self.split_path.is_empty()
            && let Some(file) = buf_file
        {
            let saved = if self.split_path == self.selected_path {
                self.saved_content.clone()
            } else {
                self.tab_buffers
                    .get(&self.split_path)
                    .map_or_else(|| self.split_saved.clone(), |b| b.saved.clone())
            };
            let p = self.split_path.clone();
            self.put_tab_buffer(
                &p,
                crate::model::TabBuffer {
                    draft: v,
                    saved,
                    file,
                },
            );
        }
        self.sync_scratch_docs();
        self.persist_editor_session(cx);
        cx.notify();
    }

    /// Keep scratch docs in step with the live buffers (session persist reads them).
    pub(crate) fn sync_scratch_docs(&mut self) {
        let fallback = || recur_editor::session::ScratchDoc {
            title: "Scratch".into(),
            content: String::new(),
            lang: "txt".into(),
            mime: None,
        };
        if is_scratch_path(&self.selected_path) {
            let d = self
                .scratch_docs
                .entry(self.selected_path.clone())
                .or_insert_with(fallback);
            if d.content != self.draft {
                d.content = self.draft.clone();
            }
        }
        if self.split_open && is_scratch_path(&self.split_path) {
            let d = self
                .scratch_docs
                .entry(self.split_path.clone())
                .or_insert_with(fallback);
            if d.content != self.split_draft {
                d.content = self.split_draft.clone();
            }
        }
    }

    /// `captureHistory` on every editor (before a rename or root switch).
    pub(crate) fn capture_editor_history(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        for ed in [&self.main_editor, &self.split_editor]
            .into_iter()
            .flatten()
        {
            ed.update(cx, |e, cx| e.capture_history(cx));
        }
        if let Some((_, ed)) = &self.merge_editor {
            ed.update(cx, |e, cx| e.capture_history(cx));
        }
    }

    /// `editor.parkSession(root)`: freeze both panes' tabs before the root swaps.
    pub(crate) fn park_editor_sessions(&mut self, root: &str, cx: &mut Context<Self>) {
        for pane in [Pane::Main, Pane::Split] {
            let Some(ed) = self.editor_for(pane).cloned() else {
                continue;
            };
            let tabs = ed.update(cx, |e, cx| {
                e.capture_history(cx);
                e.take_tabs()
            });
            crate::session_tabs::parks(cx)
                .pane(pane.key())
                .park(root, &parkable(tabs));
            let s = self.sync_for(pane);
            s.parked_outgoing = root.to_string();
            s.path.clear();
        }
    }

    /// `flushEditorCaches`: inactive tabs' text back into the tab buffers.
    pub(crate) fn flush_editor_caches(&mut self, pane: Pane, cx: &App) {
        let Some(ed) = self.editor_for(pane).cloned() else {
            return;
        };
        for (path, doc, dirty) in ed.read(cx).cached_buffers() {
            let Some(existing) = self.tab_buffers.get(&path).cloned() else {
                continue;
            };
            if !dirty && !editor_text_eq(&existing.draft, &existing.saved) {
                continue;
            }
            self.put_tab_buffer(
                &path,
                crate::model::TabBuffer {
                    draft: doc,
                    saved: existing.saved,
                    file: existing.file,
                },
            );
            if dirty {
                self.mark_tab_dirty(&path, true);
            }
        }
    }

    /// `copyLine`: `path:line` to the clipboard.
    pub(crate) fn copy_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(f) = &self.file else { return };
        let r = format!("{}:{line}", f.path);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(r.clone()));
        recur_view_core::toast(
            recur_ui::widgets::ToastKind::Info,
            format!("Copied {r}"),
            cx,
        );
    }

    /// `getFocusedEditorContext`: the focused editor for bundle prompt actions.
    pub fn focused_editor_context(&self, cx: &App) -> Option<recur_editor::view::EditorContext> {
        let ed =
            if self.focused_pane == Pane::Split && self.split_open && self.split_editor.is_some() {
                self.split_editor.as_ref()
            } else {
                self.main_editor.as_ref()
            }?;
        Some(ed.read(cx).context())
    }

    /// `stashedBuffers` with the live panes folded in (`sessionStashedBuffers`).
    pub(crate) fn session_stashed_buffers(&self, cx: &App) -> HashMap<String, StashedBuffer> {
        let mut buffers: HashMap<String, StashedBuffer> = self
            .stashed_buffers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut put = |path: &str, body: String, saved: &str| {
            if path.is_empty() || is_scratch_path(path) {
                return;
            }
            if editor_text_eq(&body, saved) {
                buffers.remove(path);
                return;
            }
            buffers.insert(
                path.to_string(),
                StashedBuffer {
                    draft: body,
                    saved: Some(saved.to_string()),
                    base_hash: None,
                },
            );
        };
        if !self.selected_path.is_empty() && self.file.is_some() && !self.file_is_binary_preview() {
            put(
                &self.selected_path,
                self.read_main_doc(cx),
                &self.saved_content,
            );
        }
        if self.split_open && !self.split_path.is_empty() {
            let body = self
                .split_editor
                .as_ref()
                .map_or_else(|| self.split_draft.clone(), |e| e.read(cx).peek_doc());
            put(&self.split_path, body, &self.split_saved);
        }
        buffers
    }
}
