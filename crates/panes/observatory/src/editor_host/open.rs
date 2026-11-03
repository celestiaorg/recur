//! Opening files: `openPath`, `openInSplit`, `hydrateFile`, the per-file view state
//! (`rememberEditorView`, `viewModeForPath`, `restoreCodeView`), stashes of unsaved buffers,
//! the base-revision cache for Diff and the SCM gutter, the outline, and live refresh of the
//! open file.

use std::time::{Duration, Instant};

use gpui_kit::{App, AppContext as _, Context, Entity, Task, Window};
use recur_domain::{FileContent, OutlineSymbol};
use recur_editor::session::StashedBuffer;
use recur_editor::view::EditorView;
use recur_editor::view_state::{
    EDITOR_BODY_CAP, EditorViewMode, FileViewState, RestoreModeOpts, editor_text_eq,
    is_binary_image_path, is_media_path, resolve_restored_view_mode,
    should_hydrate_restored_current,
};
use recur_ui::widgets::ToastKind;
use recur_view_core::file_drag::is_abs_filesystem_path;
use recur_view_core::{call, toast};

use crate::explorer::{is_directory_error, is_not_found, is_worktree_missing};
use crate::model::{MAX_EDIT_BYTES, Pane, TabBuffer, is_scratch_path};
use crate::obs::{BASE_DOC_CACHE_MAX, ExplorerReveal, GONE_CONFIRM_MS, Observatory, OpenRequest};
use crate::workspace_file::{
    fetch_workspace_file_text, file_needs_http_body, is_binary_preview_mime,
};

/// The outline poll while the TextMate idle fill catches up (`outlineOpenFile` retry): each
/// poll fills `OUTLINE_FILL_SLICE` on the UI thread.
const OUTLINE_RETRY_MS: u64 = 24;

fn domain_symbols(v: Vec<recur_textmate::OutlineSymbol>) -> Vec<OutlineSymbol> {
    v.into_iter()
        .map(|s| OutlineSymbol {
            name: s.name,
            kind: s.kind,
            line: s.line as i64,
            end: s.end.unwrap_or(0) as i64,
        })
        .collect()
}

/// The TextMate outline of an open document: `None` without a grammar, engine document or
/// rules; `Some((symbols, complete))` otherwise. Without `partial` (a poll waiting for the
/// editor's background fill) it returns no symbols until that fill completes: outlining
/// tokenizes the filled lines again on the UI thread.
pub(crate) fn tm_outline(
    pane: Pane,
    path: &str,
    partial: bool,
    cx: &App,
) -> Option<(Vec<OutlineSymbol>, bool)> {
    let env = recur_view_core::editor_env::editor_env_peek(cx)?;
    let env = env.read(cx);
    if env.symbol_rules.is_empty() {
        return None;
    }
    let id = recur_textmate::tm_doc_id_for(pane.key(), path);
    let mut tm = env.tm.borrow_mut();
    if !partial && tm.fill.is_active(&id) {
        let e = tm.catalog.engine_mut();
        if !e.has_doc(&id) {
            return None;
        }
        if e.filled_through(&id) < e.line_count(&id) {
            return Some((Vec::new(), false));
        }
    }
    let (syms, complete) =
        recur_textmate::outline_open_file(tm.catalog.engine_mut(), &id, &env.symbol_rules)?;
    Some((domain_symbols(syms), complete))
}

impl Observatory {
    // ---- view state ----

    /// `rememberEditorView`: caret / scroll from `ed` (else the previous entry) and `mode`.
    pub(crate) fn remember_editor_view(
        &mut self,
        path: &str,
        ed: Option<Entity<EditorView>>,
        mode: EditorViewMode,
        window: &Window,
        cx: &App,
    ) {
        if path.is_empty() {
            return;
        }
        let prev = self.file_views.get(path).cloned();
        let live = ed.and_then(|e| e.read(cx).get_view_state(window));
        let (pt, pl, pz) = self.preview_scroll.borrow().fields(path, prev.as_ref());
        let pick = |l: Option<usize>, p: Option<usize>, d: usize| l.or(p).unwrap_or(d);
        let mut next = FileViewState {
            view_mode: mode,
            line: pick(
                live.as_ref().map(|v| v.line),
                prev.as_ref().map(|v| v.line),
                1,
            ),
            col: pick(
                live.as_ref().map(|v| v.col),
                prev.as_ref().map(|v| v.col),
                1,
            ),
            scroll_top: live
                .as_ref()
                .map(|v| v.scroll_top)
                .or(prev.as_ref().map(|v| v.scroll_top))
                .unwrap_or(0.),
            scroll_left: live
                .as_ref()
                .map(|v| v.scroll_left)
                .or(prev.as_ref().map(|v| v.scroll_left))
                .unwrap_or(0.),
            top_line: live
                .as_ref()
                .map_or_else(|| prev.as_ref().and_then(|v| v.top_line), |v| v.top_line),
            preview_scroll_top: pt,
            preview_scroll_left: pl,
            preview_zoom: pz,
            ..Default::default()
        };
        let anchor = match &live {
            Some(l) => l.anchor_line.map(|a| (a, l.anchor_col)),
            None => prev
                .as_ref()
                .and_then(|p| p.anchor_line.map(|a| (a, p.anchor_col))),
        };
        if let Some((line, col)) = anchor {
            next.anchor_line = Some(line);
            next.anchor_col = col;
        }
        self.file_views.insert(path.to_string(), next);
    }

    /// `rememberLeavingEditors`: the surface showing the selected file (the merge pane in
    /// Diff; the code pane only once it shows that file) and the split pane.
    pub(crate) fn remember_leaving_editors(&mut self, window: &Window, cx: &App) {
        if !self.selected_path.is_empty() {
            let src = if self.view_mode == EditorViewMode::Diff {
                self.merge_editor.as_ref().map(|(_, e)| e.clone())
            } else if self.editor_path == self.selected_path {
                self.main_editor.clone()
            } else {
                None
            };
            let (p, m) = (self.selected_path.clone(), self.view_mode);
            self.remember_editor_view(&p, src, m, window, cx);
        }
        if self.split_open && !self.split_path.is_empty() && self.split_path != self.selected_path {
            let p = self.split_path.clone();
            let m = self
                .file_views
                .get(&p)
                .map_or(EditorViewMode::Code, |v| v.view_mode);
            let ed = self.split_editor.clone();
            self.remember_editor_view(&p, ed, m, window, cx);
        }
    }

    /// `viewModeForPath`.
    pub(crate) fn view_mode_for_path(
        &self,
        path: &str,
        want_diff: bool,
        deleted: bool,
    ) -> EditorViewMode {
        let saved = self.file_views.get(path).map(|v| v.view_mode);
        let pending_diff = self.restoring_session_views
            && !self.git_status_ready
            && saved == Some(EditorViewMode::Diff);
        let image = is_binary_image_path(path);
        let media = is_media_path(path);
        let status = self.git_status.get(path).map_or("", String::as_str);
        resolve_restored_view_mode(
            saved,
            RestoreModeOpts {
                want_diff,
                deleted: if media { false } else { deleted },
                can_diff: if media {
                    false
                } else if image {
                    status == "m" || status == "d" || deleted || pending_diff
                } else {
                    !status.is_empty() || deleted || pending_diff
                },
                can_preview: if image || media {
                    status != "d" && !deleted
                } else {
                    recur_content::preview::preview_kind_for_path(path).is_some()
                },
                is_image: image || media,
            },
        )
    }

    /// `restoreCodeView`: a line jump, else the saved caret / scroll, else focus.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn restore_code_view(
        &mut self,
        path: &str,
        line: usize,
        end_line: usize,
        pane: Pane,
        start_col: usize,
        end_col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_editors(window, cx);
        let target = match pane {
            Pane::Main => self.main_editor.clone(),
            Pane::Split => self.split_editor.clone(),
        };
        if self.draft_edit.is_some() {
            return;
        }
        if line > 0 {
            if pane == Pane::Split {
                if let Some(t) = &target {
                    t.update(cx, |e, cx| {
                        e.scroll_to_line(line, end_line, start_col, end_col, window, cx)
                    });
                }
            } else {
                self.scroll_to_line(line, end_line, start_col, end_col, window, cx);
            }
            return;
        }
        let Some(t) = target else { return };
        let vs = if self.skip_view_restore {
            None
        } else {
            self.file_views.get(path).cloned()
        };
        let sync = match pane {
            Pane::Main => &mut self.main_editor_sync,
            Pane::Split => &mut self.split_editor_sync,
        };
        match vs {
            Some(vs) => {
                if sync.restored_live {
                    sync.restored_live = false;
                    t.update(cx, |e, cx| e.focus(window, cx));
                } else if t.read(cx).state().doc.is_empty() {
                    sync.pending_view = Some(vs);
                } else {
                    t.update(cx, |e, cx| {
                        e.apply_view_state(&vs, window, cx);
                        e.focus(window, cx);
                    });
                }
            }
            None => t.update(cx, |e, cx| e.focus(window, cx)),
        }
    }

    // ---- stashes ----

    /// `readMainDoc`: the merge pane in Diff, the code pane when it holds a text file.
    pub(crate) fn read_main_doc(&self, cx: &App) -> String {
        if self.show_diff() {
            return self
                .merge_editor
                .as_ref()
                .map_or_else(|| self.draft.clone(), |(_, e)| e.read(cx).peek_doc());
        }
        if let (Some(e), Some(f)) = (&self.main_editor, &self.file)
            && !is_binary_preview_mime(&f.mime)
            && e.read(cx).path() == self.selected_path
        {
            return e.read(cx).peek_doc();
        }
        self.draft.clone()
    }

    /// `stashCurrentBuffer`.
    pub(crate) fn stash_current_buffer(&mut self, cx: &mut Context<Self>) {
        let path = self.selected_path.clone();
        if path.is_empty() {
            return;
        }
        if is_scratch_path(&path) {
            let body = self.read_main_doc(cx);
            let d = self.scratch_docs.entry(path).or_insert_with(|| {
                recur_editor::session::ScratchDoc {
                    title: "Scratch".into(),
                    content: String::new(),
                    lang: "txt".into(),
                    mime: None,
                }
            });
            d.content = body;
            return;
        }
        if self.file.is_none() || self.file_is_binary_preview() {
            return;
        }
        let body = self.read_main_doc(cx);
        let saved = self.saved_content.clone();
        self.stash_path(&path, &body, &saved);
    }

    /// `stashSplitBuffer`.
    pub(crate) fn stash_split_buffer(&mut self, cx: &mut Context<Self>) {
        if self.split_path.is_empty() || is_scratch_path(&self.split_path) {
            return;
        }
        let body = self
            .split_editor
            .as_ref()
            .map_or_else(|| self.split_draft.clone(), |e| e.read(cx).peek_doc());
        let (p, s) = (self.split_path.clone(), self.split_saved.clone());
        self.stash_path(&p, &body, &s);
    }

    /// `overlayStash`: a stash outranks the disk text just read.
    pub(crate) fn overlay_stash(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(s) = self.stashed_buffers.get(path).cloned() else {
            return;
        };
        self.draft = s.draft.clone();
        self.main_editor_dirty = !editor_text_eq(&s.draft, &self.saved_content);
        if !self.main_editor_dirty {
            self.drop_stash(path);
        } else {
            let disk = self.saved_content.clone();
            self.check_stash_disk_moved(path, &s, &disk, cx);
        }
    }

    /// `overlaySplitStash`.
    pub(crate) fn overlay_split_stash(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(s) = self.stashed_buffers.get(path).cloned() else {
            return;
        };
        self.split_draft = s.draft.clone();
        self.split_editor_dirty = !editor_text_eq(&s.draft, &self.split_saved);
        if !self.split_editor_dirty {
            self.drop_stash(path);
        } else {
            let disk = self.split_saved.clone();
            self.check_stash_disk_moved(path, &s, &disk, cx);
        }
    }

    // ---- base revisions ----

    /// `rememberBaseDoc`: LRU of `BASE_DOC_CACHE_MAX`; bumps the epoch when it changes.
    pub(crate) fn remember_base_doc(&mut self, path: &str, content: &str) {
        if content.encode_utf16().count() > EDITOR_BODY_CAP {
            return;
        }
        let prev = self
            .base_doc_cache
            .iter()
            .position(|(p, _)| p == path)
            .map(|i| self.base_doc_cache.remove(i).1);
        self.base_doc_cache
            .push((path.to_string(), content.to_string()));
        let mut evicted = false;
        while self.base_doc_cache.len() > BASE_DOC_CACHE_MAX {
            self.base_doc_cache.remove(0);
            evicted = true;
        }
        if prev.as_deref() != Some(content) || evicted {
            self.base_doc_epoch += 1;
        }
    }

    fn cached_base(&self, path: &str) -> Option<String> {
        self.base_doc_cache
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, b)| b.clone())
    }

    /// `fetchBaseDoc`: cached, in flight, or a quiet `FileAtBase`.
    pub(crate) fn fetch_base_doc(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Option<String>> {
        if let Some(b) = self.cached_base(path) {
            return Task::ready(Some(b));
        }
        if !self.base_doc_inflight.insert(path.to_string()) {
            let path = path.to_string();
            return cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    let (done, cached) = this
                        .read_with(cx, |o, _| {
                            (!o.base_doc_inflight.contains(&path), o.cached_base(&path))
                        })
                        .unwrap_or((true, None));
                    if done {
                        return cached;
                    }
                }
            });
        }
        let token = self.root_token;
        let (pid, wid, path) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            path.to_string(),
        );
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let b = svc.code.file_at_base(&pid, &wid, &path).await.ok();
            this.update(cx, |o, cx| {
                o.base_doc_inflight.remove(&path);
                let b = b?;
                if o.root_token != token {
                    return None;
                }
                o.remember_base_doc(&path, &b);
                cx.notify();
                Some(b)
            })
            .ok()
            .flatten()
        })
    }

    /// `prefetchBaseDoc`: warm the gutter base without blocking or toasting.
    pub(crate) fn prefetch_base_doc(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.is_git(cx)
            || path.is_empty()
            || is_scratch_path(path)
            || is_abs_filesystem_path(path)
            || is_binary_image_path(path)
            || is_media_path(path)
        {
            return;
        }
        self.fetch_base_doc(path, cx).detach();
    }

    /// `loadBaseDoc`: the Diff base for the selected file (cached first, revalidated).
    pub(crate) fn load_base_doc(&mut self, path: &str, cx: &mut Context<Self>) -> Task<()> {
        if is_binary_image_path(path) || is_media_path(path) {
            return Task::ready(());
        }
        let (pid, wid, path) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            path.to_string(),
        );
        let svc = self.services.clone();
        let token = self.root_token;
        if let Some(cached) = self.cached_base(&path) {
            self.base_doc = cached;
            self.base_doc_path = path.clone();
            cx.notify();
            cx.spawn(async move |this, cx| {
                let Ok(b) = svc.code.file_at_base(&pid, &wid, &path).await else {
                    return;
                };
                let _ = this.update(cx, |o, cx| {
                    if o.root_token != token {
                        return;
                    }
                    o.remember_base_doc(&path, &b);
                    if path == o.selected_path && o.base_doc_path == path && b != o.base_doc {
                        o.base_doc = b;
                    }
                    cx.notify();
                });
            })
            .detach();
            return Task::ready(());
        }
        let pending = self
            .base_doc_inflight
            .contains(&path)
            .then(|| self.fetch_base_doc(&path, cx));
        cx.spawn(async move |this, cx| {
            if let Some(p) = pending {
                let b = p.await;
                let applied = this
                    .update(cx, |o, cx| {
                        if path != o.selected_path {
                            return true;
                        }
                        if let Some(b) = b {
                            o.base_doc = b;
                            o.base_doc_path = path.clone();
                            cx.notify();
                            return true;
                        }
                        false
                    })
                    .unwrap_or(true);
                if applied {
                    return;
                }
            }
            let b = call(
                cx,
                svc.code.file_at_base(&pid, &wid, &path),
                "Base revision failed",
            )
            .await;
            let _ = this.update(cx, |o, cx| {
                if o.root_token != token {
                    return;
                }
                if let Some(b) = &b {
                    o.remember_base_doc(&path, b);
                }
                if path != o.selected_path {
                    return;
                }
                o.base_doc = b.unwrap_or_default();
                o.base_doc_path = path.clone();
                cx.notify();
            });
        })
    }

    /// `invalidateBaseDocs`: the merge-base moved (retarget, catch up with base).
    pub(crate) fn invalidate_base_docs(&mut self, cx: &mut Context<Self>) {
        if self.base_doc_cache.is_empty()
            && self.base_doc_inflight.is_empty()
            && self.base_doc_path.is_empty()
        {
            return;
        }
        self.base_doc_cache.clear();
        self.base_doc_inflight.clear();
        self.base_doc_epoch += 1;
        self.base_doc.clear();
        self.base_doc_path.clear();
        let sel = self.selected_path.clone();
        if self.view_mode == EditorViewMode::Diff && !sel.is_empty() {
            self.load_base_doc(&sel, cx).detach();
        } else if !sel.is_empty() {
            self.prefetch_base_doc(&sel, cx);
        }
        let split = self.split_path.clone();
        if !split.is_empty() && split != sel {
            self.prefetch_base_doc(&split, cx);
        }
        cx.notify();
    }

    /// Watch the workspace's base branch and behind count (`trackedBase*`).
    pub(crate) fn track_base(&mut self, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        let (branch, behind) = self
            .state
            .read(cx)
            .workspace(&id)
            .map(|w| (w.base_branch.trim().to_string(), w.behind_base))
            .unwrap_or_default();
        if id != self.tracked_base.0 {
            self.tracked_base = (id, branch, behind);
            return;
        }
        let retargeted = branch != self.tracked_base.1;
        let caught_up = behind < self.tracked_base.2;
        self.tracked_base.1 = branch;
        self.tracked_base.2 = behind;
        if retargeted || caught_up {
            self.invalidate_base_docs(cx);
        }
    }

    // ---- outline ----

    /// `diskOutline`: the language-agnostic outline (quiet).
    pub(crate) fn disk_outline(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Option<Vec<OutlineSymbol>>> {
        if is_abs_filesystem_path(path) || is_scratch_path(path) {
            return Task::ready(None);
        }
        let (pid, wid, path) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            path.to_string(),
        );
        let svc = self.services.clone();
        cx.spawn(async move |_, cx| {
            call(cx, svc.code.outline(&pid, &wid, &path), "Outline failed").await
        })
    }

    fn quiet_outline(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Option<Vec<OutlineSymbol>>> {
        let (pid, wid, path) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            path.to_string(),
        );
        let svc = self.services.clone();
        cx.background_spawn(async move { svc.code.outline(&pid, &wid, &path).await.ok() })
    }

    /// The outline the TextMate update for `pane`/`path` should land on, if still current.
    fn apply_tm_outline(&mut self, pane: Pane, path: &str, syms: Vec<OutlineSymbol>) -> bool {
        match pane {
            Pane::Split => {
                if self.focused_pane == Pane::Split && self.split_path == path {
                    self.outline = syms;
                    return true;
                }
            }
            Pane::Main => {
                if self
                    .lsp_attach
                    .as_ref()
                    .is_some_and(|a| a.client.has_document_symbol())
                {
                    return false;
                }
                if self.selected_path == path && self.editor_path == path {
                    self.outline = syms;
                    return true;
                }
            }
        }
        false
    }

    /// `resolveFileOutline`: the TextMate outline of the open document (updating once its
    /// fill completes), else the disk outline.
    pub(crate) fn resolve_file_outline(
        &mut self,
        path: &str,
        pane: Pane,
        disk: Task<Option<Vec<OutlineSymbol>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Vec<OutlineSymbol>>> {
        self.sync_editors(window, cx);
        let path = path.to_string();
        match tm_outline(pane, &path, true, cx) {
            Some((syms, complete)) => {
                if !complete {
                    self.poll_tm_outline(pane, path, cx);
                }
                Task::ready(Some(syms))
            }
            None => {
                // The engine document may not exist until first paint: retry once.
                let retry_path = path.clone();
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(OUTLINE_RETRY_MS))
                        .await;
                    let _ = this.update(cx, |o, cx| {
                        if let Some((syms, complete)) = tm_outline(pane, &retry_path, true, cx) {
                            o.apply_tm_outline(pane, &retry_path, syms);
                            if !complete {
                                o.poll_tm_outline(pane, retry_path.clone(), cx);
                            }
                            cx.notify();
                        }
                    });
                })
                .detach();
                disk
            }
        }
    }

    fn poll_tm_outline(&mut self, pane: Pane, path: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(OUTLINE_RETRY_MS))
                    .await;
                let done = this
                    .update(cx, |o, cx| match tm_outline(pane, &path, false, cx) {
                        Some((syms, true)) => {
                            o.apply_tm_outline(pane, &path, syms);
                            cx.notify();
                            true
                        }
                        Some((_, false)) => false,
                        None => true,
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
            }
        })
        .detach();
    }

    // ---- opening ----

    /// `openPath`: open a file in the focused pane (or its tab), keeping the explorer in step.
    pub fn open_path(
        &mut self,
        req: OpenRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let OpenRequest {
            path,
            line,
            prefer_diff,
            end_line,
            explorer,
            from_disk,
            start_col,
            end_col,
        } = req;
        if path.is_empty() {
            return Task::ready(());
        }
        self.image.base_failed_src.clear();
        self.review_unhid_for.clear();
        if !self.restoring_session_views {
            self.set_main_surface(crate::model::MainSurface::Editor, cx);
        }
        if !self.props.workspace_id.is_empty() && self.editor_hidden(cx) {
            let id = self.props.workspace_id.clone();
            self.state
                .update(cx, |s, cx| s.set_editor_hidden(&id, false, cx));
        }
        let st = self.git_status.get(&path).cloned().unwrap_or_default();
        let image_path = is_binary_image_path(&path);
        let media_path = is_media_path(&path);
        let want_diff = prefer_diff || st == "d";
        self.skip_view_restore = line > 0;
        if line > 0 {
            self.editor_reveal_gen += 1;
        }
        self.remember_leaving_editors(window, cx);
        if self.split_open
            && self.focused_pane == Pane::Split
            && !want_diff
            && !image_path
            && !media_path
            && !is_scratch_path(&path)
        {
            return self.open_in_split(
                &path, line, end_line, explorer, from_disk, start_col, end_col, window, cx,
            );
        }
        if !from_disk
            && path == self.selected_path
            && (self.file.is_some() || self.selected_is_dir || self.file_loading)
        {
            let diff = if prefer_diff && (self.can_show_diff() || self.can_show_image_diff()) {
                Some(self.set_view_mode(EditorViewMode::Diff, window, cx))
            } else {
                None
            };
            return cx.spawn_in(window, async move |this, cx| {
                if let Some(d) = diff {
                    d.await;
                }
                let _ = this.update_in(cx, |o, window, cx| {
                    o.sync_editors(window, cx);
                    if line > 0 {
                        if o.view_mode == EditorViewMode::Code
                            || o.view_mode == EditorViewMode::Diff
                        {
                            o.scroll_to_line(line, end_line, start_col, end_col, window, cx);
                        }
                    } else if o.show_diff() {
                        if let Some((_, m)) = &o.merge_editor {
                            m.update(cx, |e, cx| e.focus(window, cx));
                        }
                    } else if let Some(e) = &o.main_editor {
                        e.update(cx, |e, cx| e.focus(window, cx));
                    }
                    o.file_tab_reveal_nonce += 1;
                    if o.should_select_in_explorer(&path, explorer, cx) {
                        o.tree_path = path.clone();
                    }
                    o.sync_explorer_to_path(&path, explorer, cx);
                    cx.notify();
                });
            });
        }
        if from_disk {
            self.drop_stash(&path);
        } else {
            self.stash_current_buffer(cx);
        }
        if is_scratch_path(&path) && path != self.selected_path {
            if self.should_select_in_explorer(&path, explorer, cx) {
                self.tree_path = path.clone();
            }
            self.ensure_open_tab(&path, cx);
            let act = self.activate_scratch_tab(&path, false, window, cx);
            return cx.spawn_in(window, async move |this, cx| {
                act.await;
                let _ = this.update_in(cx, |o, window, cx| {
                    if line > 0 {
                        o.scroll_to_line(line, end_line, start_col, end_col, window, cx);
                    }
                    if explorer == ExplorerReveal::IfVisible {
                        o.scroll_explorer_row(&path, cx);
                    }
                });
            });
        }
        self.nav_gen += 1;
        let token = self.nav_gen;
        self.focused_pane = Pane::Main;
        self.selected_path = path.clone();
        self.file_tab_reveal_nonce += 1;
        if self.should_select_in_explorer(&path, explorer, cx) {
            self.tree_path = path.clone();
        }
        self.highlight_line = line;
        self.open_file_not_found_path.clear();
        self.open_file_not_found_since = None;
        self.maybe_clear_gone_confirm();
        self.selected_is_dir = false;
        self.hydrate_failed_path.clear();
        self.base_doc.clear();
        self.base_doc_path.clear();
        cx.notify();
        let work: Task<()> = if st == "d" {
            self.file = None;
            self.draft.clear();
            self.saved_content.clear();
            self.editor_path.clear();
            self.main_editor_dirty = false;
            self.outline.clear();
            self.detach_lsp(cx);
            self.ensure_open_tab(&path, cx);
            self.view_mode = if media_path {
                EditorViewMode::Preview
            } else {
                EditorViewMode::Diff
            };
            if !image_path && !media_path {
                self.load_base_doc(&path, cx)
            } else {
                Task::ready(())
            }
        } else if want_diff {
            self.ensure_open_tab(&path, cx);
            if image_path {
                self.view_mode = if st == "m" || st == "d" {
                    EditorViewMode::Diff
                } else {
                    EditorViewMode::Preview
                };
                let h = self.hydrate_file(&path, line, end_line, start_col, end_col, window, cx);
                cx.spawn(async move |_, _| {
                    h.await;
                })
            } else if media_path {
                self.view_mode = EditorViewMode::Preview;
                let h = self.hydrate_file(&path, line, end_line, start_col, end_col, window, cx);
                cx.spawn(async move |_, _| {
                    h.await;
                })
            } else {
                self.view_mode = if st.is_empty() {
                    EditorViewMode::Code
                } else {
                    EditorViewMode::Diff
                };
                let b = self.load_base_doc(&path, cx);
                let h = self.hydrate_file(&path, line, end_line, start_col, end_col, window, cx);
                cx.spawn(async move |_, _| {
                    futures::join!(b, h);
                })
            }
        } else {
            if let Some(cached) = self.tab_buffers.get(&path).cloned() {
                self.file = Some(cached.file);
                self.draft = cached.draft;
                self.saved_content = cached.saved;
                self.editor_path = path.clone();
                self.selected_is_dir = false;
            }
            let target = self.view_mode_for_path(&path, false, false);
            self.view_mode = target;
            if target == EditorViewMode::Diff && !image_path {
                let b = self.load_base_doc(&path, cx);
                let h = self.hydrate_file(&path, line, end_line, start_col, end_col, window, cx);
                cx.spawn(async move |_, _| {
                    futures::join!(b, h);
                })
            } else {
                let h = self.hydrate_file(&path, line, end_line, start_col, end_col, window, cx);
                cx.spawn(async move |_, _| {
                    h.await;
                })
            }
        };
        let mirror_split = st != "d" && !want_diff;
        cx.spawn_in(window, async move |this, cx| {
            work.await;
            let _ = this.update_in(cx, |o, window, cx| {
                if token != o.nav_gen {
                    return;
                }
                if mirror_split && o.split_open && o.split_path == path {
                    o.split_draft = o.draft.clone();
                    o.split_saved = o.saved_content.clone();
                    o.split_writable = o.writable();
                    o.split_editor_dirty = o.main_editor_dirty;
                }
                o.sync_explorer_to_path(&path, explorer, cx);
                o.sync_editors(window, cx);
                if o.view_mode == EditorViewMode::Code
                    || (o.view_mode == EditorViewMode::Diff && line > 0)
                {
                    o.restore_code_view(
                        &path,
                        line,
                        end_line,
                        Pane::Main,
                        start_col,
                        end_col,
                        window,
                        cx,
                    );
                }
                cx.notify();
            });
        })
    }

    /// `openInSplit`: the split pane's own buffer (text files only).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_in_split(
        &mut self,
        path: &str,
        line: usize,
        end_line: usize,
        explorer: ExplorerReveal,
        from_disk: bool,
        start_col: usize,
        end_col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if path.is_empty() {
            return Task::ready(());
        }
        if !self.props.workspace_id.is_empty() && self.editor_hidden(cx) {
            let id = self.props.workspace_id.clone();
            self.state
                .update(cx, |s, cx| s.set_editor_hidden(&id, false, cx));
        }
        let path = path.to_string();
        if !from_disk && path == self.split_path {
            self.sync_editors(window, cx);
            if let Some(e) = self.split_editor.clone() {
                e.update(cx, |e, cx| {
                    if line > 0 {
                        e.scroll_to_line(line, end_line, start_col, end_col, window, cx);
                    } else {
                        e.focus(window, cx);
                    }
                });
            }
            if self.should_select_in_explorer(&path, explorer, cx) {
                self.tree_path = path.clone();
            }
            self.sync_explorer_to_path(&path, explorer, cx);
            cx.notify();
            return Task::ready(());
        }
        if from_disk {
            self.drop_stash(&path);
        } else if path != self.split_path {
            let sp = self.split_path.clone();
            let mode = self
                .file_views
                .get(&sp)
                .map_or(EditorViewMode::Code, |v| v.view_mode);
            let ed = self.split_editor.clone();
            self.remember_editor_view(&sp, ed, mode, window, cx);
            self.stash_split_buffer(cx);
        }
        let read = self.read_editor_file(&path, cx);
        cx.spawn_in(window, async move |this, cx| {
            let f = match read.await {
                Ok(f) => f,
                Err(err) => {
                    let _ = this.update(cx, |o, cx| {
                        if is_directory_error(&err.to_string()) {
                            toast(
                                ToastKind::Info,
                                "Directories can’t be opened in the split pane",
                                cx,
                            );
                        } else if is_worktree_missing(&err.to_string()) {
                            o.worktree_missing = true;
                            cx.notify();
                        } else {
                            toast(
                                ToastKind::Error,
                                format!("Read failed: {}", err.message),
                                cx,
                            );
                        }
                    });
                    return;
                }
            };
            let outline = this
                .update_in(cx, |o, window, cx| {
                    if !is_abs_filesystem_path(&path) {
                        o.worktree_missing = false;
                    }
                    if is_binary_preview_mime(&f.mime) {
                        toast(
                            ToastKind::Info,
                            "Cannot open that file in the split pane",
                            cx,
                        );
                        return None;
                    }
                    o.split_open = true;
                    o.split_path = path.clone();
                    let buf = o.tab_buffers.get(&path).cloned();
                    let keep_dirty = buf
                        .as_ref()
                        .is_some_and(|b| !editor_text_eq(&b.draft, &b.saved));
                    o.split_writable = f.writable && f.size <= MAX_EDIT_BYTES;
                    match buf.filter(|_| keep_dirty) {
                        Some(b) => {
                            o.split_draft = b.draft.clone();
                            o.split_saved = f.content.clone();
                            o.split_editor_dirty = true;
                            o.put_tab_buffer(
                                &path,
                                TabBuffer {
                                    draft: b.draft,
                                    saved: f.content.clone(),
                                    file: f.clone(),
                                },
                            );
                        }
                        None => {
                            o.split_draft = f.content.clone();
                            o.split_saved = f.content.clone();
                            o.split_editor_dirty = false;
                            o.put_tab_buffer(
                                &path,
                                TabBuffer {
                                    draft: f.content.clone(),
                                    saved: f.content.clone(),
                                    file: f.clone(),
                                },
                            );
                            o.overlay_split_stash(&path, cx);
                        }
                    }
                    o.focused_pane = Pane::Split;
                    if o.should_select_in_explorer(&path, explorer, cx) {
                        o.tree_path = path.clone();
                    }
                    o.ensure_open_tab(&path, cx);
                    o.sync_explorer_to_path(&path, explorer, cx);
                    o.sync_editors(window, cx);
                    let disk = o.disk_outline(&path, cx);
                    cx.notify();
                    Some(o.resolve_file_outline(&path, Pane::Split, disk, window, cx))
                })
                .ok()
                .flatten();
            let Some(outline) = outline else { return };
            let syms = outline.await;
            let _ = this.update_in(cx, |o, window, cx| {
                if o.focused_pane == Pane::Split && o.split_path == path {
                    o.outline = syms.unwrap_or_default();
                }
                o.prefetch_base_doc(&path, cx);
                o.sync_editors(window, cx);
                if line > 0 {
                    if let Some(e) = o.split_editor.clone() {
                        e.update(cx, |e, cx| {
                            e.scroll_to_line(line, end_line, start_col, end_col, window, cx)
                        });
                    }
                } else {
                    o.restore_code_view(&path, 0, 0, Pane::Split, 0, 0, window, cx);
                }
                cx.notify();
            });
        })
    }

    /// `focusPane`: focus a pane and follow its outline.
    pub(crate) fn focus_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.focused_pane = pane;
        self.sync_editors(window, cx);
        match pane {
            Pane::Split => {
                if let Some(e) = self.split_editor.clone() {
                    e.update(cx, |e, cx| e.focus(window, cx));
                }
                if !self.split_path.is_empty() {
                    let p = self.split_path.clone();
                    let disk = self.disk_outline(&p, cx);
                    let t = self.resolve_file_outline(&p, Pane::Split, disk, window, cx);
                    cx.spawn(async move |this, cx| {
                        let o = t.await;
                        let _ = this.update(cx, |obs, cx| {
                            if obs.focused_pane == Pane::Split
                                && let Some(o) = o
                            {
                                obs.outline = o;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
            }
            Pane::Main => {
                if let Some(e) = self.main_editor.clone() {
                    e.update(cx, |e, cx| e.focus(window, cx));
                }
                if !self.selected_path.is_empty()
                    && self.file.is_some()
                    && !self.file_is_binary_preview()
                {
                    let p = self.selected_path.clone();
                    let disk = self.disk_outline(&p, cx);
                    let t = self.resolve_file_outline(&p, Pane::Main, disk, window, cx);
                    cx.spawn(async move |this, cx| {
                        let o = t.await;
                        let _ = this.update(cx, |obs, cx| {
                            if obs.focused_pane == Pane::Main
                                && let Some(o) = o
                            {
                                obs.outline = o;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
            }
        }
        cx.notify();
    }

    /// `readEditorFile`: external paths read through the external file route; worktree files
    /// fetch their body over HTTP when `ReadFile` leaves it out.
    pub(crate) fn read_editor_file(
        &self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<FileContent, recur_services::Error>> {
        let svc = self.services.clone();
        let (pid, wid, path) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            path.to_string(),
        );
        cx.background_spawn(async move {
            if is_abs_filesystem_path(&path) {
                return crate::workspace_file::read_external_editor_file(&svc, &path)
                    .await
                    .map_err(recur_services::Error::new);
            }
            let mut f = svc.code.read_file(&pid, &wid, &path).await?;
            if file_needs_http_body(&f) {
                f.content = fetch_workspace_file_text(&svc, &pid, &wid, &path)
                    .await
                    .map_err(recur_services::Error::new)?;
            }
            Ok(f)
        })
    }

    /// `hydrateFile`: read the open path (and its outline) into the main pane.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn hydrate_file(
        &mut self,
        path: &str,
        line: usize,
        end_line: usize,
        start_col: usize,
        end_col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let path = path.to_string();
        if is_scratch_path(&path) {
            let scratch = self.scratch_docs.get(&path).cloned();
            let content = scratch
                .as_ref()
                .map(|s| s.content.clone())
                .unwrap_or_default();
            let mime = scratch
                .as_ref()
                .and_then(|s| s.mime.clone())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "text/plain".into());
            let binary = is_binary_preview_mime(&mime);
            let f = FileContent {
                path: path.clone(),
                content: content.clone(),
                size: content.encode_utf16().count() as i64,
                writable: !binary,
                language: scratch
                    .as_ref()
                    .map(|s| s.lang.clone())
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| "text".into()),
                mime,
                ..Default::default()
            };
            self.file = Some(f.clone());
            self.draft = content.clone();
            self.saved_content = content.clone();
            self.editor_path = path.clone();
            self.put_tab_buffer(
                &path,
                TabBuffer {
                    draft: content.clone(),
                    saved: content,
                    file: f,
                },
            );
            self.main_editor_dirty = false;
            self.outline.clear();
            self.selected_is_dir = false;
            self.ensure_open_tab(&path, cx);
            self.sync_editors(window, cx);
            if line > 0 && (self.view_mode == EditorViewMode::Code || self.show_diff()) && !binary {
                self.scroll_to_line(line, end_line, start_col, end_col, window, cx);
            }
            let p = path.clone();
            cx.defer_in(window, move |o, window, cx| o.refresh_lsp(&p, window, cx));
            cx.notify();
            return Task::ready(true);
        }
        self.file_load_gen += 1;
        let token = self.file_load_gen;
        let had_buffer = self.tab_buffers.contains_key(&path);
        self.set_file_loading(!had_buffer, cx);
        let likely_file = {
            let base = crate::model::base_name(&path);
            base.rfind('.').is_some_and(|i| i > 0 && i + 1 < base.len())
                || base.starts_with('.') && base.len() > 1 && base[1..].contains('.')
        };
        let outline_p =
            (likely_file && !is_abs_filesystem_path(&path)).then(|| self.quiet_outline(&path, cx));
        let read = self.read_editor_file(&path, cx);
        cx.spawn_in(window, async move |this, cx| {
            let res = read.await;
            let step = this.update_in(cx, |o, window, cx| {
                if token != o.file_load_gen || path != o.selected_path {
                    if token == o.file_load_gen {
                        o.set_file_loading(false, cx);
                    }
                    return None;
                }
                let f = match res {
                    Ok(f) => f,
                    Err(err) => {
                        o.outline.clear();
                        o.selected_is_dir = false;
                        o.file = None;
                        o.draft.clear();
                        o.saved_content.clear();
                        o.editor_path.clear();
                        o.detach_lsp(cx);
                        if is_directory_error(&err.to_string()) {
                            o.selected_is_dir = true;
                            o.hydrate_failed_path.clear();
                        } else if is_worktree_missing(&err.to_string()) {
                            o.worktree_missing = true;
                            o.hydrate_failed_path = path.clone();
                        } else {
                            o.hydrate_failed_path = path.clone();
                            toast(
                                ToastKind::Error,
                                format!("Read failed: {}", err.message),
                                cx,
                            );
                        }
                        o.set_file_loading(false, cx);
                        o.sync_editors(window, cx);
                        cx.notify();
                        return Some(None);
                    }
                };
                if !is_abs_filesystem_path(&path) {
                    o.worktree_missing = false;
                }
                o.selected_is_dir = false;
                let buf = o.tab_buffers.get(&path).cloned();
                let keep_dirty = buf
                    .as_ref()
                    .is_some_and(|b| !editor_text_eq(&b.draft, &b.saved));
                o.file = Some(f.clone());
                if let Some(b) = buf.clone().filter(|_| keep_dirty) {
                    o.check_stash_disk_moved(
                        &path,
                        &StashedBuffer {
                            draft: b.draft.clone(),
                            saved: Some(b.saved.clone()),
                            base_hash: None,
                        },
                        &f.content,
                        cx,
                    );
                    o.draft = b.draft.clone();
                    o.saved_content = f.content.clone();
                    o.main_editor_dirty = true;
                    o.put_tab_buffer(
                        &path,
                        TabBuffer {
                            draft: b.draft,
                            saved: f.content.clone(),
                            file: f.clone(),
                        },
                    );
                } else if !editor_text_eq(&o.draft, &f.content) {
                    o.draft = f.content.clone();
                    o.saved_content = f.content.clone();
                    o.main_editor_dirty = false;
                    o.put_tab_buffer(
                        &path,
                        TabBuffer {
                            draft: f.content.clone(),
                            saved: f.content.clone(),
                            file: f.clone(),
                        },
                    );
                } else {
                    o.saved_content = f.content.clone();
                    o.main_editor_dirty = false;
                    let d = o.draft.clone();
                    o.put_tab_buffer(
                        &path,
                        TabBuffer {
                            draft: d,
                            saved: f.content.clone(),
                            file: f.clone(),
                        },
                    );
                }
                o.editor_path = path.clone();
                o.hydrate_failed_path.clear();
                o.ensure_open_tab(&path, cx);
                if is_binary_preview_mime(&f.mime) {
                    o.outline.clear();
                    let p = path.clone();
                    cx.defer_in(window, move |o, window, cx| o.refresh_lsp(&p, window, cx));
                    o.set_file_loading(false, cx);
                    o.sync_editors(window, cx);
                    cx.notify();
                    return Some(Some(None));
                }
                o.prefetch_base_doc(&path, cx);
                if !keep_dirty {
                    o.overlay_stash(&path, cx);
                }
                o.sync_editors(window, cx);
                cx.notify();
                let disk = match outline_p {
                    Some(t) => t,
                    None if is_abs_filesystem_path(&path) => Task::ready(None),
                    None => o.quiet_outline(&path, cx),
                };
                Some(Some(Some(o.resolve_file_outline(
                    &path,
                    Pane::Main,
                    disk,
                    window,
                    cx,
                ))))
            });
            let outline = match step {
                Ok(Some(Some(Some(t)))) => t,
                Ok(Some(Some(None))) => return true,
                _ => return false,
            };
            let syms = outline.await;
            this.update_in(cx, |o, window, cx| {
                if token == o.file_load_gen {
                    o.set_file_loading(false, cx);
                }
                if token != o.file_load_gen || path != o.selected_path {
                    return false;
                }
                o.outline = syms.unwrap_or_default();
                if line > 0 && (o.view_mode == EditorViewMode::Code || o.show_diff()) {
                    o.scroll_to_line(line, end_line, start_col, end_col, window, cx);
                }
                let p = path.clone();
                cx.defer_in(window, move |o, window, cx| o.refresh_lsp(&p, window, cx));
                cx.notify();
                true
            })
            .unwrap_or(false)
        })
    }

    /// `scrollToLine`: the merge pane in Diff, else the focused code pane.
    pub(crate) fn scroll_to_line(
        &mut self,
        line: usize,
        end_line: usize,
        start_col: usize,
        end_col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if line == 0 {
            return;
        }
        self.highlight_line = line;
        self.sync_editors(window, cx);
        let target = if self.show_diff() {
            self.merge_editor.as_ref().map(|(_, e)| e.clone())
        } else if self.focused_pane == Pane::Split && self.split_open {
            self.split_editor.clone()
        } else {
            self.main_editor.clone()
        };
        if let Some(t) = target {
            t.update(cx, |e, cx| {
                e.scroll_to_line(line, end_line, start_col, end_col, window, cx)
            });
        }
    }

    /// `setViewMode`: Code / Diff / Preview for the selected file.
    pub(crate) fn set_view_mode(
        &mut self,
        mode: EditorViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let mut load: Option<Task<()>> = None;
        if mode == EditorViewMode::Diff {
            if is_binary_image_path(&self.selected_path) {
                if !self.can_show_image_diff() {
                    toast(ToastKind::Info, "No changes versus base for this file", cx);
                    return Task::ready(());
                }
            } else {
                if self.selected_path.is_empty() || !self.can_show_diff() {
                    toast(ToastKind::Info, "No changes versus base for this file", cx);
                    return Task::ready(());
                }
                if self.base_doc_path != self.selected_path {
                    let p = self.selected_path.clone();
                    load = Some(self.load_base_doc(&p, cx));
                }
            }
        }
        cx.spawn_in(window, async move |this, cx| {
            if let Some(l) = load {
                l.await;
            }
            let _ = this.update_in(cx, |o, window, cx| {
                if !o.selected_path.is_empty() {
                    let src = if o.view_mode == EditorViewMode::Diff {
                        o.merge_editor.as_ref().map(|(_, e)| e.clone())
                    } else {
                        o.main_editor.clone()
                    };
                    let p = o.selected_path.clone();
                    o.remember_editor_view(&p, src, mode, window, cx);
                }
                o.view_mode = mode;
                o.sync_editors(window, cx);
                if mode == EditorViewMode::Code
                    && let Some(e) = o.main_editor.clone()
                {
                    e.update(cx, |e, cx| e.focus(window, cx));
                }
                o.persist_editor_session(cx);
                cx.notify();
            });
        })
    }

    /// Diff can stop applying (the file reverted); rasters have no Code surface.
    pub(crate) fn clamp_view_mode(&mut self) {
        if (is_binary_image_path(&self.selected_path) || is_media_path(&self.selected_path))
            && self.view_mode == EditorViewMode::Code
        {
            self.view_mode = EditorViewMode::Preview;
        }
        if self.view_mode != EditorViewMode::Diff
            || self.selected_path.is_empty()
            || !self.git_status_ready
        {
            return;
        }
        if is_binary_image_path(&self.selected_path) || is_media_path(&self.selected_path) {
            if !self.can_show_image_diff() {
                self.view_mode = EditorViewMode::Preview;
            }
            return;
        }
        if !self.can_show_diff() && self.selected_git_status() != "d" {
            self.view_mode = EditorViewMode::Code;
        }
    }

    /// `maybeHydrateRestoredCurrent`: a file restored as deleted came back.
    pub(crate) fn maybe_hydrate_restored_current(
        &mut self,
        prev: &std::collections::BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let path = self.selected_path.clone();
        if path.is_empty() {
            return;
        }
        if !should_hydrate_restored_current(
            &path,
            &self.editor_path,
            self.selected_is_dir,
            self.file_loading,
            &self.hydrate_failed_path,
            prev.get(&path).map_or("", String::as_str),
            self.git_status.get(&path).map_or("", String::as_str),
        ) {
            return;
        }
        self.pending_rehydrate = Some(path);
        cx.notify();
    }

    /// `refreshOpenFileIfStale`: reload the clean open file when the disk moved; confirm a
    /// vanished one over the gone window before closing its tab.
    pub(crate) fn refresh_open_file_if_stale(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if !self.live_refresh_alive {
            return Task::ready(());
        }
        let path = self.selected_path.clone();
        if path.is_empty() || is_scratch_path(&path) || is_abs_filesystem_path(&path) {
            return Task::ready(());
        }
        if self.dirty() {
            if self.open_file_not_found_since.is_some() {
                self.open_file_not_found_path.clear();
                self.open_file_not_found_since = None;
                self.maybe_clear_gone_confirm();
            }
            return Task::ready(());
        }
        if self.editor_path != path {
            return Task::ready(());
        }
        if self.refresh_open_file_in_flight {
            self.refresh_open_file_pending = true;
            return Task::ready(());
        }
        self.refresh_open_file_in_flight = true;
        self.refresh_open_file_pending = false;
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn(async move |this, cx| {
            let res = svc.code.read_file(&pid, &wid, &path).await;
            let still =
                |o: &Observatory| !o.dirty() && o.selected_path == path && o.editor_path == path;
            let next = this.update(cx, |o, cx| -> Option<FileContent> {
                match res {
                    Err(err) => {
                        if !still(o) {
                            return None;
                        }
                        if is_worktree_missing(&err.to_string()) {
                            o.worktree_missing = true;
                        } else if is_not_found(&err.to_string()) {
                            let now = Instant::now();
                            if o.open_file_not_found_path != path {
                                o.open_file_not_found_path = path.clone();
                                o.open_file_not_found_since = Some(now);
                                o.arm_gone_confirm(cx);
                            } else if o.open_file_not_found_since.is_some_and(|t| {
                                now.duration_since(t) >= Duration::from_millis(GONE_CONFIRM_MS)
                            }) {
                                o.open_files.retain(|p| *p != path);
                                o.file = None;
                                o.selected_path.clear();
                                o.editor_path.clear();
                                o.draft.clear();
                                o.saved_content.clear();
                                o.open_file_not_found_path.clear();
                                o.open_file_not_found_since = None;
                                o.maybe_clear_gone_confirm();
                                if o.open_files.is_empty() {
                                    o.persist_or_clear_editor_session(cx);
                                }
                            } else {
                                o.arm_gone_confirm(cx);
                            }
                        } else if o.open_file_not_found_path == path {
                            o.open_file_not_found_path.clear();
                            o.open_file_not_found_since = None;
                            o.maybe_clear_gone_confirm();
                        }
                        cx.notify();
                        None
                    }
                    Ok(f) => {
                        if o.open_file_not_found_path == path {
                            o.open_file_not_found_path.clear();
                            o.open_file_not_found_since = None;
                            o.maybe_clear_gone_confirm();
                        }
                        if !still(o) {
                            return None;
                        }
                        o.worktree_missing = false;
                        let cur = o.file.as_ref();
                        if !f.content.is_empty() {
                            if Some(&f.content) == cur.map(|c| &c.content) {
                                return None;
                            }
                        } else if Some(f.size) == cur.map(|c| c.size)
                            && Some(f.mtime) == cur.map(|c| c.mtime)
                        {
                            return None;
                        }
                        Some(f)
                    }
                }
            });
            let mut f = match next {
                Ok(Some(f)) => f,
                _ => {
                    let _ = this.update(cx, |o, cx| o.finish_open_file_refresh(cx));
                    return;
                }
            };
            if file_needs_http_body(&f) {
                match fetch_workspace_file_text(&svc, &pid, &wid, &path).await {
                    Ok(t) => f.content = t,
                    Err(_) => {
                        let _ = this.update(cx, |o, cx| o.finish_open_file_refresh(cx));
                        return;
                    }
                }
            }
            let outline = this
                .update(cx, |o, cx| {
                    if !still(o) {
                        return None;
                    }
                    o.file = Some(f.clone());
                    o.draft = f.content.clone();
                    o.saved_content = f.content.clone();
                    cx.notify();
                    let quiet = o.quiet_outline(&path, cx);
                    Some(match tm_outline(Pane::Main, &path, true, cx) {
                        Some((syms, _)) => Task::ready(Some(syms)),
                        None => quiet,
                    })
                })
                .ok()
                .flatten();
            if let Some(t) = outline {
                let o = t.await;
                let _ = this.update(cx, |obs, cx| {
                    if still(obs)
                        && let Some(o) = o
                    {
                        obs.outline = o;
                        cx.notify();
                    }
                });
            }
            let _ = this.update(cx, |o, cx| o.finish_open_file_refresh(cx));
        })
    }

    fn finish_open_file_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_open_file_in_flight = false;
        if self.refresh_open_file_pending {
            self.schedule_live_refresh(cx);
        }
    }
}
