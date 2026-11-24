//! The review's Uncommitted cards as editable diffs: each card near the viewport gets an
//! `EditorKind::Merge` editor of the file at HEAD against the working tree. A card edits the
//! file's one buffer: its text is the editor tabs' draft of that file (the open file, the
//! split, a background tab, opened when a card first edits a file without one), and a card
//! released or closed leaves it stashed like any unsaved buffer the user leaves. ⌘S saves it
//! through the editor's save path.

use recur_ui::guard::Guarded;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::{App, AppContext as _, Context, Entity, Subscription, Task, WeakEntity, Window};
use recur_domain::{FileContent, FileDiff};
use recur_editor::view::{EditorEvent, EditorKind, EditorView};
use recur_editor::view_state::{editor_text_eq, is_binary_image_path, is_media_path};
use recur_ui::widgets::ToastKind;
use recur_view_core::toast;
use recur_view_review::card_editor::WorkingEditors;

use crate::model::{MainSurface, TabBuffer, file_with_body};
use crate::obs::Observatory;

/// One card's editor and the texts it was built from.
pub(crate) struct CardEd {
    /// The diff the texts were read for; a new one re-reads them.
    diff: FileDiff,
    /// The file at HEAD and on disk; `None` when it is not editable.
    texts: Option<(String, String)>,
    /// Texts read since, applied by the next mount.
    fresh: Option<Option<(String, String)>>,
    editor: Option<(Entity<EditorView>, Subscription)>,
    _load: Option<Task<()>>,
}

impl CardEd {
    fn editor(&self) -> Option<&Entity<EditorView>> {
        self.editor.as_ref().map(|(e, _)| e)
    }

    fn saved(&self) -> Option<&str> {
        self.texts.as_ref().map(|(_, s)| s.as_str())
    }
}

/// What the review asks for its cards.
pub(crate) struct CardHost(pub WeakEntity<Observatory>);

impl WorkingEditors for CardHost {
    fn editor(
        &self,
        file: &FileDiff,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<EditorView>> {
        self.0
            .upgrade()?
            .update(cx, |o, cx| o.card_editor(file, window, cx))
    }

    fn retain(&self, paths: HashSet<String>, cx: &mut App) {
        let obs = self.0.clone();
        cx.defer(move |cx| {
            let _ = obs.update(cx, |o, cx| o.retain_cards(&paths, cx));
        });
    }
}

/// A file the card can edit: inlined UTF-8 text in a writable worktree.
// simplify: text past the 256 KiB inline read stays a read-only card (a content-height editor
// paints every line); stream it as the code editor does to lift that.
fn card_editable(f: &FileContent) -> bool {
    f.writable && f.mime.is_empty() && (f.size == 0 || !f.content.is_empty())
}

impl Observatory {
    pub(crate) fn card_host(&self) -> Rc<dyn WorkingEditors> {
        Rc::new(CardHost(self.weak.clone()))
    }

    /// The Uncommitted card editor mounted for `path`.
    pub fn card_editor_for(&self, path: &str) -> Option<&Entity<EditorView>> {
        self.cards.get(path)?.editor()
    }

    /// `file`'s card editor, reading its texts first.
    fn card_editor(
        &mut self,
        file: &FileDiff,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<EditorView>> {
        let path = file.path.clone();
        if file.status == "d" || is_binary_image_path(&path) || is_media_path(&path) {
            return None;
        }
        let stale = match self.cards.get(&path) {
            None => true,
            Some(c) => c.diff != *file && !c.editor().is_some_and(|e| e.read(cx).is_dirty()),
        };
        if stale {
            let load = self.load_card_texts(&path, cx);
            let c = self.cards.entry(path.clone()).or_insert_with(|| CardEd {
                diff: file.clone(),
                texts: None,
                fresh: None,
                editor: None,
                _load: None,
            });
            c.diff = file.clone();
            c._load = Some(load);
        }
        let c = self.cards.get_mut(&path)?;
        if let Some(fresh) = c.fresh.take() {
            let ed = c.editor().cloned();
            let dirty = ed.as_ref().is_some_and(|e| e.read(cx).is_dirty());
            let same_head =
                matches!((&fresh, &c.texts), (Some((h, _)), Some((was, _))) if h == was);
            match (fresh, ed) {
                // Unsaved text outranks the disk: only the baseline moves.
                (Some((_, disk)), Some(ed)) if dirty || same_head => {
                    if let Some((_, saved)) = &mut c.texts {
                        *saved = disk.clone();
                    }
                    ed.update(cx, |e, cx| {
                        if !dirty && e.peek_doc() != disk {
                            e.external_doc_change(&disk, window, cx);
                        }
                        e.apply_saved_baseline(&disk, cx);
                    });
                }
                (_, Some(_)) if dirty => {}
                (fresh, _) => {
                    c.texts = fresh;
                    c.editor = None;
                }
            }
        }
        let c = self.cards.get(&path)?;
        if let Some(ed) = c.editor() {
            return Some(ed.clone());
        }
        let (head, disk) = c.texts.clone()?;
        let doc = self.shared_draft(&path, cx).unwrap_or_else(|| disk.clone());
        let mut o = self.build_editor_options("review", &path, &doc, cx);
        o.kind = EditorKind::Merge { original: head };
        o.saved_doc = Some(disk);
        o.history_scope.clear();
        o.lit = false;
        o.prefs.scroll_past_end = false;
        let ed = cx.new(|cx| EditorView::new(o, window, cx));
        let p = path.clone();
        let sub =
            cx.guarded_subscribe_in(&ed, window, move |o, _, ev: &EditorEvent, window, cx| {
                o.on_card_event(&p, ev, window, cx)
            });
        self.cards.get_mut(&path)?.editor = Some((ed.clone(), sub));
        Some(ed)
    }

    /// Reads `path` at HEAD and on disk, then redraws the stack to mount it.
    fn load_card_texts(&mut self, path: &str, cx: &mut Context<Self>) -> Task<()> {
        let svc = self.services.clone();
        let (pid, wid, path) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            path.to_string(),
        );
        cx.spawn(async move |this, cx| {
            let disk = svc.code.read_file(&pid, &wid, &path).await;
            let head = svc.code.file_at_head(&pid, &wid, &path).await;
            let texts = match (head, disk) {
                (Ok(head), Ok(f)) if card_editable(&f) => Some((head, f.content)),
                _ => None,
            };
            let _ = this.update(cx, |o, cx| {
                let Some(c) = o.cards.get_mut(&path) else {
                    return;
                };
                c.fresh = Some(texts);
                if let Some(r) = &o.review_surface {
                    let files = r.read(cx).files_view().clone();
                    files.update(cx, |_, cx| cx.notify());
                }
            });
        })
    }

    /// The unsaved text of `path` in the editor tabs or their stash, if any.
    fn shared_draft(&self, path: &str, cx: &App) -> Option<String> {
        if self.main_buffer_path() == path && self.file.is_some() {
            return self.dirty().then(|| self.read_main_doc(cx));
        }
        if self.split_open && self.split_path == path {
            return self.split_dirty().then(|| {
                self.split_editor
                    .as_ref()
                    .map_or_else(|| self.split_draft.clone(), |e| e.read(cx).peek_doc())
            });
        }
        for ed in [&self.main_editor, &self.split_editor]
            .into_iter()
            .flatten()
        {
            if let Some((_, doc, true)) = ed
                .read(cx)
                .cached_buffers()
                .into_iter()
                .find(|(p, _, _)| p == path)
            {
                return Some(doc);
            }
        }
        if let Some(b) = self.tab_buffers.get(path)
            && !editor_text_eq(&b.draft, &b.saved)
        {
            return Some(b.draft.clone());
        }
        self.stashed_buffers.get(path).map(|s| s.draft.clone())
    }

    /// The path the main pane's buffer (`draft`) belongs to.
    pub(crate) fn main_buffer_path(&self) -> &str {
        if self.editor_path.is_empty() {
            &self.selected_path
        } else {
            &self.editor_path
        }
    }

    fn on_card_event(
        &mut self,
        path: &str,
        ev: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            EditorEvent::DocChanged(v) => self.card_doc_change(path, v.clone(), cx),
            EditorEvent::DirtyChanged(d) => {
                self.mark_tab_dirty(path, *d);
                cx.notify();
            }
            EditorEvent::Save => self.save_card(path, window, cx),
            EditorEvent::CopyLine(line) => {
                let r = format!("{path}:{line}");
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(r.clone()));
                toast(ToastKind::Info, format!("Copied {r}"), cx);
            }
            EditorEvent::PickSnippet { snippets, from, to } => {
                let ed = self.card_editor_for(path).cloned();
                self.pick_snippet(ed, snippets.clone(), *from, *to, window, cx);
            }
            EditorEvent::Toast { error, text } => toast(
                if *error {
                    ToastKind::Error
                } else {
                    ToastKind::Info
                },
                text.clone(),
                cx,
            ),
            _ => {}
        }
    }

    /// A card's text is the file's draft wherever the editor tabs keep it.
    fn card_doc_change(&mut self, path: &str, v: String, cx: &mut Context<Self>) {
        let Some(saved) = self
            .cards
            .get(path)
            .and_then(|c| c.saved())
            .map(str::to_string)
        else {
            return;
        };
        if self.file.is_some() && self.main_buffer_path() == path {
            self.on_main_doc_change(v, cx);
            return;
        }
        if self.split_open && self.split_path == path {
            self.on_split_doc_change(v, cx);
            return;
        }
        if let Some(b) = self.tab_buffers.get(path).cloned() {
            self.put_tab_buffer(
                path,
                TabBuffer {
                    draft: v.clone(),
                    ..b
                },
            );
        }
        // A background tab remounts from the draft, not its cached state.
        for ed in [&self.main_editor, &self.split_editor]
            .into_iter()
            .flatten()
        {
            if ed.read(cx).has_tab(path) {
                ed.update(cx, |e, _| e.forget_path(path));
            }
        }
        // An unsaved buffer is an open tab: it shows dirty, asks before it closes, and persists.
        if !editor_text_eq(&v, &saved) {
            self.ensure_open_tab(path, cx);
            self.main_surface_pref.get_or_insert(MainSurface::Review);
        }
        self.stash_path(path, &v, &saved);
        self.persist_editor_session(cx);
    }

    /// An editor tab typed into `path`: its card shows the same text.
    pub(crate) fn card_follow(
        &mut self,
        path: &str,
        v: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(c) = self.cards.get(path) else {
            return;
        };
        let (Some(ed), Some(saved)) = (c.editor().cloned(), c.saved().map(str::to_string)) else {
            return;
        };
        if ed.read(cx).peek_doc() == v {
            return;
        }
        ed.update(cx, |e, cx| {
            e.external_doc_change(v, window, cx);
            e.apply_saved_baseline(&saved, cx);
        });
    }

    /// The card whose editor has focus.
    pub(crate) fn focused_card(&self, window: &Window, cx: &App) -> Option<String> {
        self.cards
            .iter()
            .find(|(_, c)| c.editor().is_some_and(|e| e.read(cx).has_focus(window)))
            .map(|(p, _)| p.clone())
    }

    /// Release the cards not in `keep`, leaving their unsaved text with the editor tabs.
    pub(crate) fn retain_cards(&mut self, keep: &HashSet<String>, cx: &mut Context<Self>) {
        let gone: Vec<String> = self
            .cards
            .keys()
            .filter(|p| !keep.contains(*p))
            .cloned()
            .collect();
        for path in gone {
            if let Some(ed) = self.card_editor_for(&path).cloned() {
                let (text, dirty) = ed.update(cx, |e, cx| (e.get_doc(cx), e.is_dirty()));
                if dirty {
                    self.card_doc_change(&path, text, cx);
                }
            }
            self.cards.remove(&path);
        }
    }

    pub(crate) fn release_cards(&mut self, cx: &mut Context<Self>) {
        if !self.cards.is_empty() {
            self.retain_cards(&HashSet::new(), cx);
        }
    }

    /// ⌘S in a card: its text through the save transforms to disk.
    pub(crate) fn save_card(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.card_editor_for(path).cloned() else {
            return;
        };
        let body = ed.update(cx, |e, cx| e.get_doc(cx));
        let transform = self.apply_will_save_transforms(path, body, cx);
        let svc = self.services.clone();
        let (pid, wid, path) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            path.to_string(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let body = transform.await;
            if let Err(err) = svc.code.write_file(&pid, &wid, &path, &body).await {
                let msg = format!("Save failed: {}", err.message);
                let _ = cx.update(|_, cx| toast(ToastKind::Error, msg, cx));
                return;
            }
            let refresh = this
                .update_in(cx, |o, window, cx| {
                    if let Some(ed) = o.card_editor_for(&path).cloned()
                        && ed.read(cx).peek_doc() != body
                    {
                        ed.update(cx, |e, cx| e.external_doc_change(&body, window, cx));
                    }
                    o.path_saved(&path, &body, cx);
                    o.schedule_review_refresh(cx);
                    o.refresh_git_status(cx)
                })
                .ok();
            if let Some(t) = refresh {
                t.await;
            }
        })
        .detach();
    }

    /// `path` was written with `body`: every buffer of it is clean at that text.
    fn path_saved(&mut self, path: &str, body: &str, cx: &mut Context<Self>) {
        self.card_saved(path, body, cx);
        if let Some(f) = self.file.clone().filter(|f| f.path == path) {
            self.draft = body.to_string();
            self.saved_content = body.to_string();
            if let Some(e) = self.main_editor.clone()
                && e.read(cx).path() == path
            {
                e.update(cx, |e, cx| e.mark_saved(cx));
            }
            self.put_tab_buffer(
                path,
                TabBuffer {
                    draft: body.to_string(),
                    saved: body.to_string(),
                    file: file_with_body(&f, body),
                },
            );
        } else if let Some(b) = self.tab_buffers.get(path).cloned() {
            self.put_tab_buffer(
                path,
                TabBuffer {
                    draft: body.to_string(),
                    saved: body.to_string(),
                    file: file_with_body(&b.file, body),
                },
            );
        }
        if self.split_open && self.split_path == path {
            self.split_draft = body.to_string();
            self.split_saved = body.to_string();
            if let Some(e) = &self.split_editor {
                e.update(cx, |e, cx| e.mark_saved(cx));
            }
        }
        for ed in [&self.main_editor, &self.split_editor]
            .into_iter()
            .flatten()
        {
            if ed.read(cx).path() != path && ed.read(cx).has_tab(path) {
                ed.update(cx, |e, _| e.forget_path(path));
            }
        }
        self.mark_tab_dirty(path, false);
        self.drop_stash(path);
        let wid = self.props.workspace_id.clone();
        crate::lsp::client::LspHub::global(cx).notify_did_save(&wid, path, body);
        self.persist_editor_session(cx);
        cx.notify();
    }

    /// An editor tab saved `path`: its card is clean at `body` too.
    pub(crate) fn card_saved(&mut self, path: &str, body: &str, cx: &mut Context<Self>) {
        let Some(c) = self.cards.get_mut(path) else {
            return;
        };
        if let Some((_, saved)) = &mut c.texts {
            *saved = body.to_string();
        }
        if let Some(ed) = c.editor().cloned() {
            ed.update(cx, |e, cx| {
                if e.peek_doc() == body {
                    e.mark_saved(cx);
                } else {
                    e.apply_saved_baseline(body, cx);
                }
            });
        }
    }
}

/// The cards by path.
pub(crate) type Cards = HashMap<String, CardEd>;
