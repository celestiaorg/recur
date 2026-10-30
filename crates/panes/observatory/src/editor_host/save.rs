//! Saving (with `willSave` bundle transforms), reload from disk, the split pane, and the find
//! toggle.

use std::time::{Duration, Instant};

use gpui_kit::{Context, Task, Window};
use recur_editor::prefs::{EditorIndent, language_id_for_path};
use recur_editor::session::ScratchDoc;
use recur_ui::widgets::ToastKind;
use recur_view_core::commands::{EditorContext, scope_for_path};
use recur_view_core::toast;

use crate::model::{Pane, TabBuffer, file_with_body, is_scratch_path};
use crate::obs::{ExplorerReveal, Observatory, OpenRequest};

/// `applyWillSaveTransforms` budget.
const WILL_SAVE_DEADLINE: Duration = Duration::from_millis(15_000);

impl Observatory {
    /// The grammar scope the bundle commands match (`scopeForPath`).
    pub(crate) fn scope_for(&self, path: &str, cx: &gpui_kit::App) -> String {
        let grammar = recur_view_core::editor_env::editor_env_peek(cx).and_then(|env| {
            env.read(cx)
                .tm
                .borrow()
                .catalog
                .resolve_scope_for_path(path, None)
        });
        scope_for_path(path, grammar.as_deref())
    }

    /// `applyWillSaveTransforms`: every `willSave` command over `body`; any failure keeps
    /// the original.
    pub(crate) fn apply_will_save_transforms(
        &mut self,
        path: &str,
        body: String,
        cx: &mut Context<Self>,
    ) -> Task<String> {
        if is_scratch_path(path) {
            return Task::ready(body);
        }
        let scope = self.scope_for(path, cx);
        let path = path.to_string();
        let cmds = self.state.update(cx, |s, cx| {
            s.commands_for_trigger("willSave", &scope, &path, cx)
        });
        let prefs = recur_view_core::editor_env::editor_prefs(cx);
        let indent = prefs.resolve_indent(language_id_for_path(&path));
        let soft_tabs = indent != EditorIndent::Tabs;
        let tab_size = match indent {
            EditorIndent::Spaces2 => 2,
            EditorIndent::Spaces4 => 4,
            EditorIndent::Tabs => {
                if prefs.tab_size > 0 {
                    prefs.tab_size as i64
                } else {
                    4
                }
            }
        };
        let root = self.workspace_root(cx);
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let cmds = cmds.await;
            if cmds.is_empty() {
                return body;
            }
            let deadline = Instant::now() + WILL_SAVE_DEADLINE;
            let mut next = body.clone();
            for cmd in cmds {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    cx.update(|cx| {
                        toast(
                            ToastKind::Error,
                            "Save transforms timed out — writing original",
                            cx,
                        )
                    });
                    return body;
                }
                let ctx = EditorContext {
                    document: next.clone(),
                    line_number: 1,
                    column: 1,
                    scope: scope.clone(),
                    tab_size,
                    soft_tabs,
                    path: path.clone(),
                    project_directory: root.clone(),
                    work_dir: root.clone(),
                    timeout_ms: remaining.as_millis().min(12_000) as i64,
                    ..Default::default()
                };
                let run = cx.update(|cx| {
                    state.update(cx, |s, cx| {
                        s.run_command(&cmd.id, ctx, Default::default(), tab_size, cx)
                    })
                });
                match run.await {
                    Ok(res) => {
                        if res.exit_code != 0 {
                            let msg = if res.stderr.trim().is_empty() {
                                format!("{} failed", cmd.label)
                            } else {
                                res.stderr.trim().to_string()
                            };
                            cx.update(|cx| toast(ToastKind::Error, msg, cx));
                            return body;
                        }
                        if res.output == "replaceDocument" || res.output == "replaceInput" {
                            let out = res.stdout;
                            if out.trim().is_empty() {
                                cx.update(|cx| {
                                    toast(
                                        ToastKind::Error,
                                        format!("{} returned empty — keeping original", cmd.label),
                                        cx,
                                    )
                                });
                                return body;
                            }
                            let (n_out, n_next) =
                                (out.encode_utf16().count(), next.encode_utf16().count());
                            if n_next > 100 && (n_out as f64) < n_next as f64 * 0.1 {
                                cx.update(|cx| {
                                    toast(
                                        ToastKind::Error,
                                        format!(
                                            "{} shrank output too much — keeping original",
                                            cmd.label
                                        ),
                                        cx,
                                    )
                                });
                                return body;
                            }
                            next = out;
                        }
                    }
                    Err(e) => {
                        let msg = if e.is_empty() {
                            format!("{} failed", cmd.label)
                        } else {
                            e
                        };
                        cx.update(|cx| toast(ToastKind::Error, msg, cx));
                        return body;
                    }
                }
            }
            next
        })
    }

    fn scratch_saved(&mut self, path: &str, body: &str) {
        let d = self
            .scratch_docs
            .entry(path.to_string())
            .or_insert_with(|| ScratchDoc {
                title: "Scratch".into(),
                content: String::new(),
                lang: "txt".into(),
                mime: None,
            });
        d.content = body.to_string();
    }

    /// `saveEdit`: the focused pane, scratch buffers in memory, files through `WriteFile`.
    pub(crate) fn save_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_editors(window, cx);
        if self.focused_pane == Pane::Split && self.split_open && !self.split_path.is_empty() {
            if !self.split_writable {
                return;
            }
            let path = self.split_path.clone();
            let body = self.split_editor.clone().map_or_else(
                || self.split_draft.clone(),
                |e| e.update(cx, |e, cx| e.get_doc(cx)),
            );
            if is_scratch_path(&path) {
                self.scratch_saved(&path, &body);
                self.split_saved_now(&path, &body, cx);
                toast(ToastKind::Info, "Scratch buffer updated (not on disk)", cx);
                return;
            }
            let transform = self.apply_will_save_transforms(&path, body, cx);
            let svc = self.services.clone();
            let (pid, wid) = (
                self.props.project_id.clone(),
                self.props.workspace_id.clone(),
            );
            cx.spawn(async move |this, cx| {
                let body = transform.await;
                if let Err(err) = svc.code.write_file(&pid, &wid, &path, &body).await {
                    cx.update(|cx| {
                        toast(
                            ToastKind::Error,
                            format!("Save failed: {}", err.message),
                            cx,
                        )
                    });
                    return;
                }
                let refresh = this
                    .update(cx, |o, cx| {
                        o.split_saved_now(&path, &body, cx);
                        o.drop_stash(&path);
                        crate::lsp::client::LspHub::global(cx).notify_did_save(&wid, &path, &body);
                        o.refresh_git_status(cx)
                    })
                    .ok();
                if let Some(t) = refresh {
                    t.await;
                }
            })
            .detach();
            return;
        }
        let Some(file) = self.file.clone() else {
            return;
        };
        if !file.writable
            || crate::workspace_file::is_binary_preview_mime(&file.mime)
            || file.size > crate::model::MAX_EDIT_BYTES
        {
            return;
        }
        let body = if self.show_diff() {
            self.merge_editor
                .as_ref()
                .map(|(_, e)| e.clone())
                .map_or_else(
                    || self.draft.clone(),
                    |e| e.update(cx, |e, cx| e.get_doc(cx)),
                )
        } else {
            self.main_editor.clone().map_or_else(
                || self.draft.clone(),
                |e| e.update(cx, |e, cx| e.get_doc(cx)),
            )
        };
        if is_scratch_path(&file.path) {
            self.scratch_saved(&file.path, &body);
            self.draft = body.clone();
            self.saved_content = body.clone();
            if let Some(e) = &self.main_editor {
                e.update(cx, |e, cx| e.mark_saved(cx));
            }
            self.mark_tab_dirty(&file.path, false);
            self.put_tab_buffer(
                &file.path,
                TabBuffer {
                    draft: body.clone(),
                    saved: body.clone(),
                    file: file_with_body(&file, &body),
                },
            );
            toast(ToastKind::Info, "Scratch buffer updated (not on disk)", cx);
            cx.notify();
            return;
        }
        let transform = self.apply_will_save_transforms(&file.path, body, cx);
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn(async move |this, cx| {
            let body = transform.await;
            if let Err(err) = svc.code.write_file(&pid, &wid, &file.path, &body).await {
                cx.update(|cx| {
                    toast(
                        ToastKind::Error,
                        format!("Save failed: {}", err.message),
                        cx,
                    )
                });
                return;
            }
            let refresh = this
                .update(cx, |o, cx| {
                    o.draft = body.clone();
                    o.saved_content = body.clone();
                    if let Some(e) = &o.main_editor {
                        e.update(cx, |e, cx| e.mark_saved(cx));
                    }
                    o.mark_tab_dirty(&file.path, false);
                    let cur = o.file.clone().unwrap_or(file.clone());
                    o.put_tab_buffer(
                        &file.path,
                        TabBuffer {
                            draft: body.clone(),
                            saved: body.clone(),
                            file: file_with_body(&cur, &body),
                        },
                    );
                    if o.split_open && o.split_path == file.path {
                        o.split_draft = body.clone();
                        o.split_saved = body.clone();
                        if let Some(e) = &o.split_editor {
                            e.update(cx, |e, cx| e.mark_saved(cx));
                        }
                    }
                    o.drop_stash(&file.path);
                    crate::lsp::client::LspHub::global(cx).notify_did_save(&wid, &file.path, &body);
                    o.persist_editor_session(cx);
                    cx.notify();
                    o.refresh_git_status(cx)
                })
                .ok();
            if let Some(t) = refresh {
                t.await;
            }
        })
        .detach();
    }

    /// The split pane's save landed: mirror to the main pane when it shows the same file.
    fn split_saved_now(&mut self, path: &str, body: &str, cx: &mut Context<Self>) {
        self.split_draft = body.to_string();
        self.split_saved = body.to_string();
        if let Some(e) = &self.split_editor {
            e.update(cx, |e, cx| e.mark_saved(cx));
        }
        self.mark_tab_dirty(path, false);
        if path == self.selected_path {
            self.draft = body.to_string();
            self.saved_content = body.to_string();
            if let Some(e) = &self.main_editor {
                e.update(cx, |e, cx| e.mark_saved(cx));
            }
            if let Some(f) = self.file.clone() {
                self.put_tab_buffer(
                    path,
                    TabBuffer {
                        draft: body.to_string(),
                        saved: body.to_string(),
                        file: file_with_body(&f, body),
                    },
                );
            }
        } else if let Some(buf) = self.tab_buffers.get(path).cloned() {
            self.put_tab_buffer(
                path,
                TabBuffer {
                    draft: body.to_string(),
                    saved: body.to_string(),
                    file: file_with_body(&buf.file, body),
                },
            );
        }
        self.persist_editor_session(cx);
        cx.notify();
    }

    /// `dropEditorBuffer`: forget a path's buffer, parked tab and undo.
    pub(crate) fn drop_editor_buffer(&mut self, path: &str, cx: &mut Context<Self>) {
        if path.is_empty() {
            return;
        }
        self.tab_buffers.remove(path);
        for e in [&self.main_editor, &self.split_editor]
            .into_iter()
            .flatten()
        {
            e.update(cx, |e, _| e.forget_path(path));
        }
        super::forget_history_tree(&self.editor_session_key, path, cx);
        self.mark_tab_dirty(path, false);
        let p = path.to_string();
        for s in [&mut self.main_editor_sync, &mut self.split_editor_sync] {
            if s.path == p {
                s.path.clear();
            }
        }
    }

    /// `reloadFromDisk` (F5).
    pub(crate) fn reload_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused_pane == Pane::Split && self.split_open && !self.split_path.is_empty() {
            let confirm = if self.split_dirty() {
                self.ask_confirm(
                    "Reload",
                    "Reload split pane from disk and discard unsaved edits?",
                    "Reload",
                    window,
                    cx,
                )
            } else {
                Task::ready(true)
            };
            let path = self.split_path.clone();
            cx.spawn_in(window, async move |this, cx| {
                if !confirm.await {
                    return;
                }
                let t = this
                    .update_in(cx, |o, window, cx| {
                        o.drop_editor_buffer(&path, cx);
                        let keep = o.focused_pane;
                        o.focused_pane = Pane::Split;
                        let line = o.cursor_line;
                        let t = o.open_in_split(
                            &path,
                            line,
                            0,
                            ExplorerReveal::Reveal,
                            true,
                            0,
                            0,
                            window,
                            cx,
                        );
                        (t, keep)
                    })
                    .ok();
                if let Some((t, keep)) = t {
                    t.await;
                    let _ = this.update(cx, |o, cx| {
                        o.focused_pane = keep;
                        toast(ToastKind::Info, "Reloaded from disk", cx);
                        cx.notify();
                    });
                }
            })
            .detach();
            return;
        }
        if self.selected_path.is_empty() || self.selected_git_status() == "d" {
            return;
        }
        let confirm = if self.dirty() {
            self.ask_confirm(
                "Reload",
                "Reload from disk and discard unsaved editor buffer?",
                "Reload",
                window,
                cx,
            )
        } else {
            Task::ready(true)
        };
        let path = self.selected_path.clone();
        let artifact = recur_engine::view::artifacts::artifact_id_from_scratch_path(&path);
        let svc = self.services.clone();
        let wid = self.props.workspace_id.clone();
        cx.spawn_in(window, async move |this, cx| {
            if !confirm.await {
                return;
            }
            if !artifact.is_empty() {
                let Some(list) = recur_view_core::call(
                    cx,
                    svc.artifacts.list_artifacts(&wid),
                    "Could not list artifacts",
                )
                .await
                else {
                    return;
                };
                let Some(a) = list.into_iter().find(|x| x.id == artifact) else {
                    cx.update(|_, cx| toast(ToastKind::Error, "Artifact not found", cx))
                        .ok();
                    return;
                };
                let t = this
                    .update_in(cx, |o, window, cx| {
                        o.drop_editor_buffer(&path, cx);
                        o.open_artifact(&a.id, &a.title, &a.kind.to_string(), &a.path, window, cx)
                    })
                    .ok();
                if let Some(t) = t
                    && t.await
                {
                    cx.update(|_, cx| toast(ToastKind::Info, "Reloaded from disk", cx))
                        .ok();
                }
                return;
            }
            let t = this
                .update_in(cx, |o, window, cx| {
                    o.drop_editor_buffer(&path, cx);
                    let line = o.cursor_line;
                    o.open_path(
                        OpenRequest::new(path.clone()).line(line).from_disk(true),
                        window,
                        cx,
                    )
                })
                .ok();
            if let Some(t) = t {
                t.await;
                cx.update(|_, cx| toast(ToastKind::Info, "Reloaded from disk", cx))
                    .ok();
            }
        })
        .detach();
    }

    /// `closeSplit`.
    pub(crate) fn close_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.split_open = false;
        self.split_path.clear();
        self.split_draft.clear();
        self.split_saved.clear();
        self.split_writable = false;
        self.focused_pane = Pane::Main;
        self.sync_editors(window, cx);
        if let Some(e) = self.main_editor.clone() {
            e.update(cx, |e, cx| e.focus(window, cx));
        }
        cx.notify();
    }

    /// `toggleSplit` (⌘\): clone the current file into the split pane, focus stays main.
    pub(crate) fn toggle_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_open {
            self.stash_split_buffer(cx);
            self.close_split(window, cx);
            return;
        }
        if self.selected_path.is_empty()
            || self.file.is_none()
            || self.file_is_binary_preview()
            || self.show_diff()
        {
            toast(ToastKind::Info, "Open a file in Code view to split", cx);
            return;
        }
        self.split_open = true;
        self.split_path = self.selected_path.clone();
        self.split_draft = self.draft.clone();
        self.split_saved = self.saved_content.clone();
        self.split_writable = self.writable();
        self.focused_pane = Pane::Main;
        self.persist_editor_session(cx);
        cx.notify();
    }

    /// `clampSplitPct`.
    pub(crate) fn clamp_split_pct(n: f64) -> i64 {
        n.round().clamp(20., 80.) as i64
    }

    /// Move the split; `persist` writes `recur.editorSplitPct` (drag end, keyboard).
    pub(crate) fn set_split_pct(&mut self, n: f64, persist: bool, cx: &mut Context<Self>) {
        self.split_pct = Self::clamp_split_pct(n);
        if persist {
            let v = self.split_pct.to_string();
            self.state
                .update(cx, |s, _| s.storage_mut().set("recur.editorSplitPct", v));
        }
        cx.notify();
    }

    // ---- find ----

    /// The text editor the menubar's Find and Go to Line items act on: the focused editor
    /// surface, not a preview, image or PDF (those keep ⌘F to themselves).
    pub(crate) fn menu_text_editor(
        &self,
        cx: &gpui_kit::App,
    ) -> Option<gpui_kit::Entity<recur_editor::view::EditorView>> {
        if self.focus_zone != crate::model::FocusZone::Editor
            || self.main_surface(cx) != crate::model::MainSurface::Editor
            || self.show_pdf_pane()
            || self.show_image_pane()
            || self.show_video_pane()
        {
            return None;
        }
        if self.show_diff() {
            return self.merge_editor.as_ref().map(|(_, e)| e.clone());
        }
        if self.focused_pane == Pane::Split && self.split_open {
            return self.split_editor.clone();
        }
        if self.show_preview() {
            return None;
        }
        self.main_editor.clone()
    }

    /// `syncFindPanelOpen`: the crumb bar's find toggle follows the focused surface.
    pub(crate) fn sync_find_panel_open(&mut self, cx: &mut Context<Self>) {
        let open = if self.show_pdf_pane() {
            self.pdf_preview
                .as_ref()
                .is_some_and(|p| p.read(cx).is_search_open())
        } else if self.show_diff() {
            self.merge_editor
                .as_ref()
                .is_some_and(|(_, e)| e.read(cx).is_search_open())
        } else if self.focused_pane == Pane::Split && self.split_open && self.split_editor.is_some()
        {
            self.split_editor
                .as_ref()
                .is_some_and(|e| e.read(cx).is_search_open())
        } else if self.show_preview() {
            self.file_preview
                .as_ref()
                .is_some_and(|(_, p)| p.read(cx).is_search_open())
        } else {
            self.main_editor
                .as_ref()
                .is_some_and(|e| e.read(cx).is_search_open())
        };
        if open != self.find_panel_open {
            self.find_panel_open = open;
            cx.notify();
        }
    }

    /// `toggleFindPanel` (the crumb bar's search button).
    pub(crate) fn toggle_find_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_editors(window, cx);
        if self.show_pdf_pane() {
            if let Some(p) = self.pdf_preview.clone() {
                p.update(cx, |p, cx| p.toggle_search(window, cx));
            }
        } else if self.show_diff() {
            if let Some((_, e)) = self.merge_editor.clone() {
                e.update(cx, |e, cx| e.toggle_search(window, cx));
            }
        } else if self.focused_pane == Pane::Split && self.split_open && self.split_editor.is_some()
        {
            if let Some(e) = self.split_editor.clone() {
                e.update(cx, |e, cx| e.toggle_search(window, cx));
            }
        } else if self.show_preview() {
            if let Some((_, p)) = self.file_preview.clone() {
                p.update(cx, |p, cx| p.toggle_search(window, cx));
            }
        } else if let Some(e) = self.main_editor.clone() {
            e.update(cx, |e, cx| e.toggle_search(window, cx));
        }
        self.sync_find_panel_open(cx);
    }
}
