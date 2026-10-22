//! The open-file strip and per-tab buffers: opening, trimming to the max, closing (one discard
//! prompt for several dirty tabs), reopening closed tabs, scratch documents from command
//! output, and artifact tabs.

use gpui_kit::{Context, Task, Window};
use recur_editor::session::{ScratchDoc, StashedBuffer, unique_open_files};
use recur_editor::view_state::{EDITOR_BODY_CAP, EditorViewMode, editor_text_eq};
use recur_ui::widgets::ToastKind;
use recur_view_core::toast;

use crate::model::{TabBuffer, is_scratch_path};
use crate::obs::{Observatory, OpenRequest};

/// `artifactScratchLang`: the scratch extension / preview language for an artifact kind.
pub fn artifact_scratch_lang(kind: &str, path: &str) -> String {
    match kind.to_lowercase().as_str() {
        "mermaid" => return "mmd".into(),
        "html" => return "html".into(),
        "svg" => return "svg".into(),
        "png" => return "png".into(),
        "jpeg" | "jpg" => return "jpg".into(),
        "gif" => return "gif".into(),
        "webp" => return "webp".into(),
        "csv" => return "csv".into(),
        "json" => return "json".into(),
        "md" | "markdown" => return "md".into(),
        _ => {}
    }
    let base = path.rsplit(['/', '\\']).next().unwrap_or("");
    match base.rfind('.') {
        Some(i) => recur_view_core::commands::scratch_language(&base[i + 1..]),
        None => "txt".into(),
    }
}

/// `artifactImageMIME`: raster artifacts open as image tabs; SVG stays text.
pub fn artifact_image_mime(kind: &str) -> &'static str {
    match kind.to_lowercase().as_str() {
        "png" => "image/png",
        "jpeg" | "jpg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "",
    }
}

/// The scratch path's slug (`title` with runs of other characters as `_`).
pub fn scratch_slug(title: &str) -> String {
    let mut out = String::new();
    let mut run = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
            out.push(c);
            run = false;
        } else if !run {
            out.push('_');
            run = true;
        }
    }
    let t = out.trim_matches('_').to_string();
    if t.is_empty() { "Scratch".into() } else { t }
}

impl Observatory {
    pub(crate) fn mark_tab_dirty(&mut self, path: &str, dirty: bool) {
        if path.is_empty() {
            return;
        }
        if dirty {
            self.dirty_tabs.insert(path.to_string());
        } else {
            self.dirty_tabs.remove(path);
        }
    }

    /// `pathIsDirty`: live dirty for mounted panes, else a stash.
    pub(crate) fn path_is_dirty(&self, path: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        if path == self.selected_path && self.file.is_some() {
            return self.dirty();
        }
        if self.split_open && path == self.split_path {
            return self.split_dirty();
        }
        self.stashed_buffers.contains_key(path)
    }

    /// `tabIsDirty`: the file-tab dirty dot.
    pub(crate) fn tab_is_dirty(&self, path: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        if self.dirty_tabs.contains(path) {
            return true;
        }
        if let Some(b) = self.tab_buffers.get(path)
            && !editor_text_eq(&b.draft, &b.saved)
        {
            return true;
        }
        self.path_is_dirty(path)
    }

    /// `putTabBuffer`: clean oversized bodies are not kept.
    pub(crate) fn put_tab_buffer(&mut self, path: &str, buf: TabBuffer) {
        let clean = editor_text_eq(&buf.draft, &buf.saved);
        if clean {
            self.mark_tab_dirty(path, false);
        }
        if clean && buf.draft.encode_utf16().count() > EDITOR_BODY_CAP {
            self.tab_buffers.remove(path);
            return;
        }
        self.tab_buffers.insert(path.to_string(), buf);
    }

    /// `trimOpenFiles`: unique, capped at `max` (0 = unlimited), never dropping `keep`.
    pub(crate) fn trim_open_files(
        &mut self,
        paths: Vec<String>,
        keep: &str,
        max: usize,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let mut next = unique_open_files(&paths);
        if max == 0 {
            return next;
        }
        while next.len() > max {
            let Some(drop) = next.iter().position(|p| p != keep) else {
                break;
            };
            let dropped = next.remove(drop);
            crate::editor_host::forget_history_tree(&self.editor_session_key, &dropped, cx);
        }
        next
    }

    pub(crate) fn max_open_files(&self, cx: &gpui_kit::App) -> usize {
        crate::editor_host::editor_prefs(cx).max_open_files as usize
    }

    /// `ensureOpenTab`: add after the selected tab; never reorder existing tabs.
    pub(crate) fn ensure_open_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.open_files.iter().any(|p| p == path) {
            return;
        }
        let mut next = self.open_files.clone();
        match next.iter().position(|p| *p == self.selected_path) {
            Some(i) => next.insert(i + 1, path.to_string()),
            None => next.push(path.to_string()),
        }
        let max = self.max_open_files(cx);
        self.open_files = self.trim_open_files(next, path, max, cx);
        cx.notify();
    }

    /// The max-open-files pref went down: trim, keeping the active tab.
    pub(crate) fn enforce_max_open_files(&mut self, cx: &mut Context<Self>) {
        let max = self.max_open_files(cx);
        if max > 0 && self.open_files.len() > max {
            let keep = self.selected_path.clone();
            let files = self.open_files.clone();
            self.open_files = self.trim_open_files(files, &keep, max, cx);
        }
    }

    /// The new order after a drag (`fileTabDrag.setItems`).
    pub(crate) fn reorder_open_files(&mut self, order: Vec<String>, cx: &mut Context<Self>) {
        self.open_files = unique_open_files(&order);
        self.persist_editor_session(cx);
        cx.notify();
    }

    /// `closeMRU`.
    pub(crate) fn close_mru(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tabs(vec![path.to_string()], window, cx);
    }

    /// `closeTabs`: one discard prompt, at most one neighbor load.
    pub(crate) fn close_tabs(
        &mut self,
        paths: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gone: Vec<String> = paths
            .into_iter()
            .filter(|p| self.open_files.contains(p) || (!p.is_empty() && *p == self.selected_path))
            .collect();
        if gone.is_empty() {
            return;
        }
        let dirty = gone.iter().filter(|p| self.tab_is_dirty(p)).count();
        let confirm = if dirty > 0 {
            let msg = if dirty == 1 {
                "Discard unsaved edits?".to_string()
            } else {
                format!("Discard unsaved edits in {dirty} tabs?")
            };
            self.ask_confirm("Unsaved edits", &msg, "Discard", window, cx)
        } else {
            Task::ready(true)
        };
        cx.spawn_in(window, async move |this, cx| {
            if !confirm.await {
                return;
            }
            let _ = this.update_in(cx, |o, window, cx| o.close_tabs_now(gone, window, cx));
        })
        .detach();
    }

    fn close_tabs_now(&mut self, gone: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let idx = self
            .open_files
            .iter()
            .position(|p| *p == self.selected_path);
        let was_selected = gone.contains(&self.selected_path);
        let remaining: Vec<String> = self
            .open_files
            .iter()
            .filter(|p| !gone.contains(p))
            .cloned()
            .collect();
        let mut closed = gone.clone();
        closed.extend(
            self.closed_tabs
                .iter()
                .filter(|p| !gone.contains(p))
                .cloned(),
        );
        closed.truncate(20);
        self.closed_tabs = closed;
        self.open_files = remaining.clone();
        for path in &gone {
            self.stashed_buffers.remove(path);
            self.scratch_docs.remove(path);
            self.tab_buffers.remove(path);
            self.mark_tab_dirty(path, false);
            if self.editor_path == *path {
                self.editor_path.clear();
            }
            if self.tree_path == *path && was_selected {
                self.tree_path.clear();
            }
        }
        if !self.split_path.is_empty() && gone.contains(&self.split_path) {
            self.close_split(window, cx);
        }
        if was_selected {
            self.file = None;
            self.selected_is_dir = false;
            self.base_doc.clear();
            self.base_doc_path.clear();
            self.selected_path.clear();
            self.draft.clear();
            self.saved_content.clear();
            let next = if remaining.is_empty() {
                None
            } else {
                let i = idx.unwrap_or(0).min(remaining.len() - 1);
                remaining.get(i).cloned()
            };
            match next {
                Some(next) => self.open_path(OpenRequest::new(next), window, cx).detach(),
                None => self.detach_lsp(cx),
            }
        }
        let scope = self.editor_session_key.clone();
        cx.defer(move |cx| {
            for path in &gone {
                crate::editor_host::forget_history_tree(&scope, path, cx);
            }
        });
        if self.open_files.is_empty() {
            self.persist_or_clear_editor_session(cx);
        }
        self.sync_editors(window, cx);
        cx.notify();
    }

    /// ⌘⇧T.
    pub(crate) fn reopen_closed_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.closed_tabs.is_empty() {
            return false;
        }
        let path = self.closed_tabs.remove(0);
        self.open_path(OpenRequest::new(path), window, cx).detach();
        true
    }

    /// `rotate`: the next or previous tab, wrapping.
    pub(crate) fn rotate_file_tab(
        &mut self,
        dir: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let n = self.open_files.len();
        if n < 2 {
            return;
        }
        let i = self
            .open_files
            .iter()
            .position(|p| *p == self.selected_path)
            .unwrap_or(0) as i32;
        let next = self.open_files[((i + dir).rem_euclid(n as i32)) as usize].clone();
        self.open_path(OpenRequest::new(next), window, cx).detach();
    }

    // ---- scratch documents ----

    /// The tab label; same-titled scratches get `(n)`.
    pub(crate) fn scratch_tab_label(&self, path: &str) -> String {
        let title = self
            .scratch_docs
            .get(path)
            .map(|d| d.title.trim().to_string())
            .unwrap_or_default();
        let base = if !title.is_empty() {
            title.clone()
        } else {
            let rest = path
                .strip_prefix("scratch://")
                .and_then(|r| {
                    let (n, tail) = r.split_once('-')?;
                    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then_some(tail)
                })
                .unwrap_or(path);
            let stem = match rest.rfind('.') {
                Some(i) if i + 1 < rest.len() => &rest[..i],
                _ => rest,
            };
            if stem.is_empty() {
                "Scratch".to_string()
            } else {
                stem.to_string()
            }
        };
        let peers: Vec<&String> = self
            .open_files
            .iter()
            .filter(|p| {
                is_scratch_path(p)
                    && self
                        .scratch_docs
                        .get(*p)
                        .map(|d| d.title.trim().to_string())
                        .unwrap_or_default()
                        == title
            })
            .collect();
        if peers.len() <= 1 {
            return base;
        }
        let n = peers.iter().position(|p| *p == path).map_or(0, |i| i + 1);
        if n > 1 { format!("{base} ({n})") } else { base }
    }

    /// `openScratchTab`: always a new scratch buffer and tab (command output).
    pub(crate) fn open_scratch_tab(
        &mut self,
        title: &str,
        contents: &str,
        lang: &str,
        open_in_preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scratch_seq += 1;
        let ext = recur_view_core::commands::scratch_language(lang);
        let path = format!(
            "scratch://{}-{}.{ext}",
            self.scratch_seq,
            scratch_slug(title)
        );
        self.scratch_docs.insert(
            path.clone(),
            ScratchDoc {
                title: title.to_string(),
                content: contents.to_string(),
                lang: ext,
                mime: None,
            },
        );
        self.ensure_open_tab(&path, cx);
        self.stash_current_buffer(cx);
        self.activate_scratch_tab(&path, open_in_preview, window, cx)
            .detach();
    }

    /// `activateScratchTab`.
    pub(crate) fn activate_scratch_tab(
        &mut self,
        path: &str,
        open_in_preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.nav_gen += 1;
        let token = self.nav_gen;
        self.focused_pane = crate::model::Pane::Main;
        if path != self.selected_path {
            self.remember_leaving_editors(window, cx);
        }
        self.selected_path = path.to_string();
        self.file_tab_reveal_nonce += 1;
        self.tree_path = path.to_string();
        self.highlight_line = 0;
        self.open_file_not_found_path.clear();
        self.open_file_not_found_since = None;
        self.maybe_clear_gone_confirm();
        self.selected_is_dir = false;
        self.base_doc.clear();
        self.base_doc_path.clear();
        self.skip_view_restore = false;
        let first_open = open_in_preview && !self.file_views.contains_key(path);
        self.view_mode = if first_open {
            if recur_content::preview::preview_kind_for_path(path).is_some() {
                EditorViewMode::Preview
            } else {
                EditorViewMode::Code
            }
        } else {
            self.view_mode_for_path(path, false, false)
        };
        let hydrate = self.hydrate_file(path, 0, 0, 0, 0, window, cx);
        let path = path.to_string();
        cx.spawn_in(window, async move |this, cx| {
            hydrate.await;
            let _ = this.update_in(cx, |o, window, cx| {
                if token != o.nav_gen {
                    return;
                }
                if o.view_mode == EditorViewMode::Code {
                    o.restore_code_view(&path, 0, 0, crate::model::Pane::Main, 0, 0, window, cx);
                }
            });
        })
    }

    // ---- artifacts ----

    /// `openArtifact`: artifacts live under the data dir, not the worktree; they open as a
    /// stable scratch tab per artifact id.
    pub fn open_artifact(
        &mut self,
        id: &str,
        title: &str,
        kind: &str,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let id = id.trim().to_string();
        let rel = path.trim_start_matches('/').to_string();
        if id.is_empty() || !rel.starts_with("workspaces/") {
            toast(ToastKind::Error, "Invalid artifact", cx);
            return Task::ready(false);
        }
        self.show_editor_surface(cx);
        let url = format!("/{rel}");
        let kind = kind.to_lowercase();
        let lang = artifact_scratch_lang(&kind, &rel);
        let scratch = format!("scratch://artifact-{id}.{lang}");
        let title = if title.trim().is_empty() {
            id.clone()
        } else {
            title.trim().to_string()
        };
        let svc = self.services.clone();
        cx.spawn_in(window, async move |this, cx| {
            let res = crate::workspace_file::fetch_bytes(&svc, &url).await;
            let (_, body) = match res {
                Ok(r) => r,
                Err(e) => {
                    cx.update(|_, cx| {
                        toast(
                            ToastKind::Error,
                            format!("Could not open artifact: {e}"),
                            cx,
                        )
                    })
                    .ok();
                    return false;
                }
            };
            let mime = {
                let m = artifact_image_mime(&kind);
                if m.is_empty() {
                    artifact_image_mime(&lang)
                } else {
                    m
                }
            };
            let content = if mime.is_empty() {
                String::from_utf8_lossy(&body).into_owned()
            } else {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(&body)
            };
            this.update_in(cx, |o, window, cx| {
                o.scratch_docs.insert(
                    scratch.clone(),
                    ScratchDoc {
                        title,
                        content,
                        lang: lang.clone(),
                        mime: (!mime.is_empty()).then(|| mime.to_string()),
                    },
                );
                o.ensure_open_tab(&scratch, cx);
                let preview = mime.is_empty()
                    && recur_content::preview::preview_kind_for_path(&scratch).is_some();
                o.activate_scratch_tab(&scratch, preview, window, cx)
                    .detach();
                true
            })
            .unwrap_or(false)
        })
    }

    pub(crate) fn artifact_tab_path(&self, id: &str) -> String {
        self.open_files
            .iter()
            .find(|p| recur_engine::view::artifacts::artifact_id_from_scratch_path(p) == id)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn rename_artifact_tab(&mut self, id: &str, title: &str, cx: &mut Context<Self>) {
        let path = self.artifact_tab_path(id);
        if let Some(doc) = self.scratch_docs.get_mut(&path) {
            doc.title = if title.is_empty() {
                id.to_string()
            } else {
                title.to_string()
            };
            cx.notify();
        }
    }

    pub(crate) fn drop_artifact_tab(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = self.artifact_tab_path(id);
        if path.is_empty() {
            return;
        }
        self.drop_open_path(&path, window, cx);
        self.scratch_docs.remove(&path);
    }

    // ---- stash ----

    pub(crate) fn drop_stash(&mut self, path: &str) {
        self.stashed_buffers.remove(path);
    }

    /// `stashPath`: keep a dirty disk buffer the user leaves; clean ones drop.
    pub(crate) fn stash_path(&mut self, path: &str, body: &str, saved: &str) {
        if path.is_empty() || is_scratch_path(path) {
            return;
        }
        if editor_text_eq(body, saved) {
            self.drop_stash(path);
            return;
        }
        self.stashed_buffers.insert(
            path.to_string(),
            StashedBuffer {
                draft: body.to_string(),
                saved: Some(saved.to_string()),
                base_hash: None,
            },
        );
    }

    /// `warnStashDiskMoved`.
    pub(crate) fn warn_stash_disk_moved(&self, path: &str, cx: &mut Context<Self>) {
        let name = crate::model::base_name(path);
        let name = if name.is_empty() { path } else { name };
        recur_ui::widgets::push_toast(
            ToastKind::Info,
            format!(
                "{name} changed on disk while you had unsaved edits. Showing your draft; save will overwrite the disk version."
            ),
            recur_ui::widgets::ToastOptions {
                ttl: Some(std::time::Duration::from_millis(8000)),
                ..Default::default()
            },
            cx,
        );
    }

    /// `checkStashDiskMoved`: a live stash compares text, a restored one compares hashes.
    pub(crate) fn check_stash_disk_moved(
        &self,
        path: &str,
        s: &StashedBuffer,
        disk: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(saved) = &s.saved {
            if !editor_text_eq(saved, disk) {
                self.warn_stash_disk_moved(path, cx);
            }
            return;
        }
        let Some(base) = s.base_hash.clone().filter(|h| !h.is_empty()) else {
            return;
        };
        if recur_editor::session::editor_content_hash(disk) != base {
            self.warn_stash_disk_moved(path, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_langs_and_mimes() {
        assert_eq!(artifact_scratch_lang("Mermaid", "x/y.txt"), "mmd");
        assert_eq!(artifact_scratch_lang("", "workspaces/w/a/report.md"), "md");
        assert_eq!(artifact_scratch_lang("", "workspaces/w/a/noext"), "txt");
        assert_eq!(artifact_image_mime("JPG"), "image/jpeg");
        assert_eq!(artifact_image_mime("svg"), "");
    }

    #[test]
    fn scratch_slugs() {
        assert_eq!(scratch_slug("Command Output"), "Command_Output");
        assert_eq!(scratch_slug("  ??  "), "Scratch");
        assert_eq!(scratch_slug("a.b-c_d"), "a.b-c_d");
    }
}
