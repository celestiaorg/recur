//! Bundle commands and snippets against the focused editor (`runEditorCommand`,
//! `applyCommandResult`, `insertBundleSnippet`, the keyEquivalent chord index), and the
//! language server attachment of the open file (`refreshLsp`, outline, references, quick
//! fixes, the host's file access for workspace edits).

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Duration;

use futures::FutureExt as _;
use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use gpui_kit::{App, Context, Entity, Task, WeakEntity, Window};
use recur_editor::lsp::{LspClient as _, LspJump};
use recur_editor::snippet::RegisteredSnippet;
use recur_editor::view::EditorView;
use recur_ui::widgets::ToastKind;
use recur_view_core::commands::{EditorContext, RegisteredCommand};
use recur_view_core::toast;

use crate::lsp::client::{
    LspAttach, LspFileIo, LspHub, apply_lsp_quick_fix, attach_lsp, lsp_outline, lsp_references,
    merged_diagnostics, uri_to_rel_path,
};
use crate::lsp::lang::lsp_language_id_for_path;
use crate::lsp::problems::LspProblem;
use crate::model::{Pane, TabBuffer, file_with_body, is_scratch_path};
use crate::obs::{Observatory, OpenRequest};

/// A palette command or snippet bound to a keyEquivalent (`ChordHit`).
#[derive(Clone)]
pub enum ChordHit {
    Command(RegisteredCommand),
    Snippet(RegisteredSnippet),
}

/// `chordIndex`: canonical chord → hits, for the focused file.
#[derive(Default)]
pub struct ChordIndex {
    pub path: String,
    pub hits: HashMap<String, Vec<ChordHit>>,
}

impl ChordHit {
    /// `chordItem`: the keybinding menu row.
    pub fn item(&self) -> recur_view_widgets::keybinding_menu::KeybindingMenuItem {
        match self {
            ChordHit::Command(c) => recur_view_widgets::keybinding_menu::KeybindingMenuItem {
                id: format!("c:{}", c.id),
                label: c.label.clone(),
                category: c.category.clone(),
                order: c.order,
            },
            ChordHit::Snippet(s) => recur_view_widgets::keybinding_menu::KeybindingMenuItem {
                id: format!("s:{}", s.id),
                label: s.label.clone(),
                category: s
                    .bundle_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Snippets".into()),
                order: s.order,
            },
        }
    }
}

/// The Observatory's buffers and disk for language server workspace edits (`setLspFileIO`).
struct ObsFileIo(WeakEntity<Observatory>);

impl LspFileIo for ObsFileIo {
    fn unsaved(&self, rel: &str, cx: &mut App) -> Option<String> {
        let o = self.0.upgrade()?;
        o.update(cx, |o, cx| {
            if rel == o.selected_path {
                return Some(
                    o.main_editor
                        .clone()
                        .map_or_else(|| o.draft.clone(), |e| e.update(cx, |e, cx| e.get_doc(cx))),
                );
            }
            if rel == o.split_path {
                return Some(o.split_editor.clone().map_or_else(
                    || o.split_draft.clone(),
                    |e| e.update(cx, |e, cx| e.get_doc(cx)),
                ));
            }
            o.tab_buffers.get(rel).map(|b| b.draft.clone())
        })
    }

    fn read(&self, rel: &str, cx: &mut App) -> LocalBoxFuture<'static, Option<String>> {
        let Some(o) = self.0.upgrade() else {
            return async { None }.boxed_local();
        };
        let t = o.update(cx, |o, cx| o.read_editor_file(rel, cx));
        async move { t.await.ok().map(|f| f.content) }.boxed_local()
    }

    fn write(&self, rel: &str, text: String, cx: &mut App) -> LocalBoxFuture<'static, ()> {
        let Some(o) = self.0.upgrade() else {
            return async {}.boxed_local();
        };
        let rel = rel.to_string();
        let write = o.update(cx, |o, cx| {
            if let Some(buf) = o.tab_buffers.get(&rel).cloned() {
                let dirty = !recur_editor::view_state::editor_text_eq(&text, &buf.saved);
                o.put_tab_buffer(
                    &rel,
                    TabBuffer {
                        draft: text.clone(),
                        saved: buf.saved.clone(),
                        file: file_with_body(&buf.file, &text),
                    },
                );
                o.mark_tab_dirty(&rel, dirty);
                cx.notify();
                return None;
            }
            let svc = o.services.clone();
            let (pid, wid) = (o.props.project_id.clone(), o.props.workspace_id.clone());
            Some(cx.spawn(async move |_, cx| {
                recur_view_core::call(
                    cx,
                    svc.code.write_file(&pid, &wid, &rel, &text),
                    "Apply edit failed",
                )
                .await;
            }))
        });
        async move {
            if let Some(t) = write {
                t.await;
            }
        }
        .boxed_local()
    }

    fn apply_if_open(&self, rel: &str, text: &str, cx: &mut App) -> bool {
        let Some(o) = self.0.upgrade() else {
            return false;
        };
        let Some(window) = o.read(cx).window else {
            return false;
        };
        window
            .update(cx, |_, window, cx| {
                o.update(cx, |o, cx| {
                    let target = if rel == o.selected_path {
                        o.main_editor.clone().map(|e| (e, Pane::Main))
                    } else if rel == o.split_path {
                        o.split_editor.clone().map(|e| (e, Pane::Split))
                    } else {
                        None
                    };
                    let Some((e, pane)) = target else {
                        return false;
                    };
                    e.update(cx, |e, cx| {
                        e.apply_output("replaceDocument", text, None, window, cx)
                    });
                    match pane {
                        Pane::Main => o.draft = text.to_string(),
                        Pane::Split => o.split_draft = text.to_string(),
                    }
                    cx.notify();
                    true
                })
            })
            .unwrap_or(false)
    }
}

impl Observatory {
    // ---- bundle commands ----

    /// `collectPrompts`: the command's prompt dialog, `None` when cancelled.
    fn collect_prompts(
        &mut self,
        cmd: &RegisteredCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<BTreeMap<String, String>>> {
        if cmd.prompts.is_empty() {
            return Task::ready(Some(BTreeMap::new()));
        }
        let (tx, rx) = oneshot::channel();
        if let Some(prev) = self.prompts_reply.replace(tx) {
            let _ = prev.send(None);
        }
        let (title, prompts) = (cmd.label.clone(), cmd.prompts.clone());
        self.prompts
            .update(cx, |p, cx| p.open(&title, prompts, window, cx));
        cx.spawn(async move |_, _| rx.await.unwrap_or(None))
    }

    /// The prompt dialog answered.
    pub(crate) fn resolve_prompts(
        &mut self,
        values: Option<BTreeMap<String, String>>,
        cx: &mut Context<Self>,
    ) {
        self.prompts.update(cx, |p, cx| p.close(cx));
        if let Some(tx) = self.prompts_reply.take() {
            let _ = tx.send(values);
        }
    }

    /// `runEditorCommand`: run a bundle command against the focused editor.
    pub(crate) fn run_editor_command(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.sync_editors(window, cx);
        let split =
            self.focused_pane == Pane::Split && self.split_open && self.split_editor.is_some();
        let Some(focused) = (if split {
            self.split_editor.clone()
        } else {
            self.main_editor.clone()
        }) else {
            toast(ToastKind::Error, "Open a file in the editor first", cx);
            return Task::ready(());
        };
        let path =
            if self.focused_pane == Pane::Split && self.split_open && !self.split_path.is_empty() {
                self.split_path.clone()
            } else {
                self.selected_path.clone()
            };
        let scope = self.scope_for(&path, cx);
        let list = self
            .state
            .update(cx, |s, cx| s.list_commands(&scope, &path, cx));
        let id = id.to_string();
        let root = self.workspace_root(cx);
        cx.spawn_in(window, async move |this, cx| {
            let cmds = list.await;
            let Some(cmd) = cmds.into_iter().find(|c| c.id == id) else {
                cx.update(|_, cx| toast(ToastKind::Error, "Unknown command", cx))
                    .ok();
                return;
            };
            let Ok(prompts) =
                this.update_in(cx, |o, window, cx| o.collect_prompts(&cmd, window, cx))
            else {
                return;
            };
            let Some(inputs) = prompts.await else { return };
            let Ok((ctx, run)) = this.update(cx, |o, cx| {
                let ectx = focused.read(cx).context();
                let clipboard = cx
                    .read_from_clipboard()
                    .and_then(|c| c.text())
                    .unwrap_or_default();
                let directory = match ectx.path.rfind('/') {
                    Some(i) => ectx.path[..i].to_string(),
                    None => String::new(),
                };
                let ctx = EditorContext {
                    selected_text: ectx.selected_text,
                    document: ectx.document,
                    current_line: ectx.current_line,
                    line_number: ectx.line_number as i64,
                    column: ectx.column as i64,
                    current_word: ectx.current_word,
                    scope: ectx.scope,
                    tab_size: ectx.tab_size as i64,
                    soft_tabs: ectx.soft_tabs,
                    path: ectx.path,
                    directory,
                    project_directory: root.clone(),
                    work_dir: root.clone(),
                    clipboard,
                    timeout_ms: 0,
                };
                let tab = recur_view_core::editor_env::editor_prefs(cx).tab_size as i64;
                let run = o
                    .state
                    .update(cx, |s, cx| s.run_command(&id, ctx.clone(), inputs, tab, cx));
                (ctx, run)
            }) else {
                return;
            };
            let _ = ctx;
            let res = run.await;
            let _ = this.update_in(cx, |o, window, cx| match res {
                Ok(res) => {
                    if res.exit_code != 0 {
                        let msg = if res.stderr.trim().is_empty() {
                            format!("Command exited {}", res.exit_code)
                        } else {
                            res.stderr.trim().to_string()
                        };
                        toast(ToastKind::Error, msg, cx);
                    }
                    let keep_focus = res.output != "newDocument" || !res.open_in_preview;
                    o.apply_command_result(&focused, &res, &cmd.label, window, cx);
                    if keep_focus {
                        focused.update(cx, |e, cx| e.focus(window, cx));
                    }
                }
                Err(e) => {
                    let msg = if e.is_empty() {
                        "Command failed".into()
                    } else {
                        e
                    };
                    toast(ToastKind::Error, msg, cx);
                    focused.update(cx, |e, cx| e.focus(window, cx));
                }
            });
        })
    }

    /// `applyCommandResult`: route a command's stdout by its `output`.
    fn apply_command_result(
        &mut self,
        ed: &Entity<EditorView>,
        res: &recur_view_core::commands::RunCommandResult,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ok = res.exit_code == 0;
        match res.output.as_str() {
            "replaceSelection" | "replaceDocument" | "replaceInput" | "insertText"
            | "insertSnippet" => {
                if !ok {
                    return;
                }
                let mode = (!res.input_mode.is_empty()).then_some(res.input_mode.as_str());
                let next = ed.update(cx, |e, cx| {
                    e.apply_output(&res.output, &res.stdout, mode, window, cx)
                });
                if let Some(next) = next {
                    self.apply_editor_text(next, cx);
                }
            }
            "newDocument" => {
                if !ok {
                    return;
                }
                let title = if label.is_empty() {
                    "Command Output"
                } else {
                    label
                };
                let lang = if res.language.is_empty() {
                    "txt"
                } else {
                    &res.language
                };
                self.open_scratch_tab(title, &res.stdout, lang, res.open_in_preview, window, cx);
            }
            "copyToClipboard" => {
                if !ok && res.stdout.is_empty() {
                    return;
                }
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(res.stdout.clone()));
                toast(ToastKind::Success, "Copied to clipboard", cx);
            }
            "showAsToast" | "showAsTooltip" => {
                let out = res.stdout.trim();
                if !out.is_empty() {
                    toast(ToastKind::Info, out.to_string(), cx);
                } else if ok {
                    toast(ToastKind::Info, label.to_string(), cx);
                }
            }
            "discard" => {
                if ok && !res.stderr.trim().is_empty() {
                    toast(ToastKind::Error, res.stderr.trim().to_string(), cx);
                }
            }
            other => {
                if ok {
                    let msg = if res.stderr.trim().is_empty() {
                        format!(
                            "Unknown output routing {}",
                            serde_json::to_string(other).unwrap_or_default()
                        )
                    } else {
                        res.stderr.trim().to_string()
                    };
                    toast(ToastKind::Error, msg, cx);
                }
            }
        }
    }

    /// New text in the focused editor from a command or snippet: mirror it into the drafts.
    fn apply_editor_text(&mut self, next: String, cx: &mut Context<Self>) {
        if self.focused_pane == Pane::Split && self.split_open {
            self.split_draft = next.clone();
            self.split_editor_sync.last_emitted = Some(next.clone());
            if self.split_path == self.selected_path {
                self.draft = next;
            }
        } else {
            self.main_editor_sync.last_emitted = Some(next.clone());
            self.draft = next;
        }
        self.sync_scratch_docs();
        self.persist_editor_session(cx);
        cx.notify();
    }

    /// `insertBundleSnippet`: at the focused editor's caret or over its selection.
    pub(crate) fn insert_bundle_snippet(
        &mut self,
        sn: &RegisteredSnippet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_editors(window, cx);
        let Some(ed) = self.focused_editor() else {
            toast(ToastKind::Error, "Open a file in the editor first", cx);
            return;
        };
        let next = ed.update(cx, |e, cx| e.apply_bundle_snippet(sn, window, cx));
        self.apply_editor_text(next, cx);
        ed.update(cx, |e, cx| e.focus(window, cx));
    }

    /// `applyChordHit`.
    pub(crate) fn apply_chord_hit(
        &mut self,
        hit: ChordHit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match hit {
            ChordHit::Command(c) => {
                recur_view_core::command_usage::record_command_usage(&self.state, &c.id, cx);
                self.run_editor_command(&c.id, window, cx).detach();
            }
            ChordHit::Snippet(s) => self.insert_bundle_snippet(&s, window, cx),
        }
    }

    /// Rebuild the keyEquivalent index for the focused file (commands with the palette
    /// trigger, snippets with a key equivalent).
    pub(crate) fn refresh_chord_index(&mut self, cx: &mut Context<Self>) {
        let path =
            if self.focused_pane == Pane::Split && self.split_open && !self.split_path.is_empty() {
                self.split_path.clone()
            } else {
                self.selected_path.clone()
            };
        let epoch = self.state.read(cx).bundles_epoch;
        let key = format!("{epoch}\0{path}");
        if self.chord_key == key {
            return;
        }
        self.chord_key = key;
        if path.is_empty() {
            self.chord_index = Rc::default();
            self.chord_task = None;
            return;
        }
        let scope = self.scope_for(&path, cx);
        let cmds = self
            .state
            .update(cx, |s, cx| s.list_commands(&scope, &path, cx));
        let snips = recur_view_core::editor_env::editor_env_peek(cx)
            .map(|e| e.read(cx).snippets.list(&scope, &path));
        self.chord_task = Some(cx.spawn(async move |this, cx| {
            let cmds = cmds.await;
            let snips = match snips {
                Some(s) => s.await,
                None => Vec::new(),
            };
            let mut hits: HashMap<String, Vec<ChordHit>> = HashMap::new();
            for c in cmds {
                if !c.triggers.iter().any(|t| t == "palette") || c.key_equivalent.is_empty() {
                    continue;
                }
                hits.entry(c.key_equivalent.clone())
                    .or_default()
                    .push(ChordHit::Command(c));
            }
            for s in snips {
                let Some(k) = s.key_equivalent.clone().filter(|k| !k.is_empty()) else {
                    continue;
                };
                hits.entry(k).or_default().push(ChordHit::Snippet(s));
            }
            let _ = this.update(cx, |o, _| {
                o.chord_index = Rc::new(ChordIndex { path, hits });
            });
        }));
    }

    // ---- language servers ----

    /// `detachLsp`.
    pub(crate) fn detach_lsp(&mut self, cx: &mut Context<Self>) {
        self.lsp_attach_gen += 1;
        self.close_lsp_attach(cx);
    }

    fn close_lsp_attach(&mut self, cx: &mut Context<Self>) {
        if let Some(a) = self.lsp_attach.take() {
            for c in a.clients.iter().skip(1) {
                c.sync();
                c.close_file(&a.file_uri);
            }
        }
        self.lsp_sync_task = None;
        if let Some(e) = &self.main_editor {
            e.update(cx, |e, cx| e.set_lsp(None, "", "", cx));
        }
        self.main_editor_sync.lsp_uri.clear();
    }

    /// The main editor unmounted: its attachment closed with it.
    pub(crate) fn lsp_editor_detached(&mut self, _cx: &mut Context<Self>) {
        self.main_editor_sync.lsp_uri.clear();
    }

    /// `refreshLsp`: attach the open file's language servers (soft-degrades to none).
    pub(crate) fn refresh_lsp(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.lsp_attach_gen += 1;
        let token = self.lsp_attach_gen;
        self.close_lsp_attach(cx);
        let (ws, proj) = (
            self.props.workspace_id.clone(),
            self.props.project_id.clone(),
        );
        if path.is_empty() || is_scratch_path(path) || ws.is_empty() || !self.writable() {
            return;
        }
        let hub = LspHub::global(cx);
        hub.wire(&self.services, cx);
        let language = {
            let env = recur_view_core::editor_env::editor_env_peek(cx);
            match env {
                Some(env) => {
                    let env = env.read(cx);
                    let tm = env.tm.borrow();
                    lsp_language_id_for_path(path, Some(&tm.catalog))
                }
                None => lsp_language_id_for_path(path, None),
            }
        };
        let weak = self.weak.clone();
        let (p, w) = (path.to_string(), ws.clone());
        let wanted: Rc<dyn Fn(&App) -> bool> = Rc::new(move |cx: &App| {
            weak.upgrade().is_some_and(|o| {
                let o = o.read(cx);
                token == o.lsp_attach_gen
                    && o.props.workspace_id == w
                    && o.selected_path == p
                    && o.writable()
            })
        });
        let path = path.to_string();
        let app = cx.to_async();
        cx.spawn_in(window, async move |this, cx| {
            let attach =
                attach_lsp(hub, proj, ws.clone(), path.clone(), language, wanted, app).await;
            let outline = this
                .update_in(cx, |o, window, cx| {
                    if token != o.lsp_attach_gen
                        || path != o.selected_path
                        || o.props.workspace_id != ws
                        || !o.writable()
                    {
                        return None;
                    }
                    let a = attach?;
                    o.install_lsp(a.clone(), window, cx);
                    Some(a)
                })
                .ok()
                .flatten();
            let Some(a) = outline else { return };
            let _ = a.client.initializing().await;
            let syms = lsp_outline(&a.client, &a.file_uri).await;
            let _ = this.update(cx, |o, cx| {
                if path == o.selected_path
                    && token == o.lsp_attach_gen
                    && let Some(s) = syms.filter(|s| !s.is_empty())
                {
                    o.outline = s;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn install_lsp(&mut self, a: LspAttach, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_editors(window, cx);
        if let Some(e) = self.main_editor.clone() {
            let client: Rc<dyn recur_editor::lsp::LspClient> = a.client.clone();
            e.update(cx, |e, cx| {
                e.set_lsp(Some(client), &a.file_uri, &a.language_id, cx)
            });
            let doc = e.read(cx).state().doc.clone();
            for c in a.clients.iter().skip(1) {
                c.open_file(&a.file_uri, &a.language_id, &doc);
            }
            self.main_editor_sync.lsp_uri = a.file_uri.clone();
        }
        self.lsp_attach = Some(a);
        self.paint_lsp_diagnostics(window, cx);
        cx.notify();
    }

    /// `secondaryDocSync` + `autoSyncClients`: the other servers see the main editor's text
    /// and sync 500ms after the last edit.
    pub(crate) fn notify_secondary_lsp(&mut self, pane: Pane, text: &str, cx: &mut Context<Self>) {
        if pane != Pane::Main {
            return;
        }
        let Some(a) = self.lsp_attach.clone() else {
            return;
        };
        if a.clients.len() < 2 {
            return;
        }
        for c in a.clients.iter().skip(1) {
            c.text_changed(&a.file_uri, text);
        }
        self.lsp_sync_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(crate::lsp::client::AUTO_SYNC_MS))
                .await;
            for c in a.clients.iter().skip(1) {
                c.sync();
            }
        }));
    }

    /// `scheduleLspOutline`: refetch `documentSymbol` 300ms after edits.
    pub(crate) fn schedule_lsp_outline(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.lsp_attach.clone() else {
            return;
        };
        if !a.client.has_document_symbol() {
            return;
        }
        let path = self.selected_path.clone();
        let token = self.lsp_attach_gen;
        self.lsp_outline_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let still = this
                .read_with(cx, |o, _| {
                    o.lsp_attach_gen == token && o.selected_path == path
                })
                .unwrap_or(false);
            if !still {
                return;
            }
            let syms = lsp_outline(&a.client, &a.file_uri).await;
            let _ = this.update(cx, |o, cx| {
                if o.lsp_attach_gen == token
                    && o.selected_path == path
                    && let Some(s) = syms
                {
                    o.outline = s;
                    cx.notify();
                }
            });
        }));
    }

    /// `paintMergedDiagnostics` for the main editor's attached file.
    pub(crate) fn paint_lsp_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.lsp_attach.clone() else {
            return;
        };
        let Some(e) = self.main_editor.clone() else {
            return;
        };
        let doc = e.read(cx).state().doc.clone();
        let diags = merged_diagnostics(&a.file_uri, &doc, &self.state, cx);
        e.update(cx, |e, cx| e.set_diagnostics(diags, window, cx));
    }

    /// The host side of `setLspFileIO` / `setLspDisplayFile` while this shell is live.
    pub(crate) fn register_lsp_host(&mut self, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        if id.is_empty() {
            return;
        }
        let hub = LspHub::global(cx);
        hub.set_file_io(&id, Some(Rc::new(ObsFileIo(self.weak.clone()))));
        let weak = self.weak.clone();
        hub.set_display_file(
            &id,
            Some(Rc::new(move |rel: String, jump: LspJump, cx: &mut App| {
                let Some(o) = weak.upgrade() else {
                    return async { false }.boxed_local();
                };
                let Some(window) = o.read(cx).window else {
                    return async { false }.boxed_local();
                };
                let opened = window
                    .update(cx, |_, window, cx| {
                        o.update(cx, |o, cx| {
                            o.open_path(
                                OpenRequest::new(rel.clone())
                                    .line(jump.line as usize + 1)
                                    .cols(jump.character as usize + 1, jump.character as usize + 1),
                                window,
                                cx,
                            )
                        })
                    })
                    .ok();
                async move {
                    match opened {
                        Some(t) => {
                            t.await;
                            true
                        }
                        None => false,
                    }
                }
                .boxed_local()
            })),
        );
    }

    pub(crate) fn unregister_lsp_host(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        let hub = LspHub::global(cx);
        hub.set_file_io(workspace_id, None);
        hub.set_display_file(workspace_id, None);
    }

    /// `findRefs`: LSP references at the symbol's line when attached, else the index.
    pub(crate) fn find_refs(
        &mut self,
        name: &str,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_code_panel(recur_view_core::prefs::CodePanelKey::Refs, window, cx);
        let pane_path =
            if self.focused_pane == Pane::Split && self.split_open && !self.split_path.is_empty() {
                self.split_path.clone()
            } else {
                self.selected_path.clone()
            };
        let lsp = self
            .lsp_attach
            .clone()
            .filter(|a| line > 0 && uri_to_rel_path(&a.root_uri, &a.file_uri) == pane_path);
        let character = lsp.as_ref().map_or(0, |_| {
            let ed = if self.focused_pane == Pane::Split {
                self.split_editor.as_ref()
            } else {
                self.main_editor.as_ref()
            };
            ed.map_or(0, |e| {
                let doc = &e.read(cx).state().doc;
                if line > doc.lines() {
                    return 0;
                }
                let l = doc.line(line);
                let text = doc.slice(l.from, l.to);
                text.find(name)
                    .map_or(0, |i| text[..i].encode_utf16().count())
            })
        });
        let svc = self.services.clone();
        let (pid, wid, name) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
            name.to_string(),
        );
        cx.spawn(async move |this, cx| {
            if let Some(a) = lsp
                && let Some(refs) = lsp_references(
                    &a.client,
                    &a.file_uri,
                    &a.root_uri,
                    line as i64 - 1,
                    character as i64,
                )
                .await
                && !refs.is_empty()
            {
                let _ = this.update(cx, |o, cx| {
                    o.refs = refs;
                    cx.notify();
                });
                return;
            }
            if let Some(r) = recur_view_core::call(
                cx,
                svc.code.find_refs(&pid, &wid, &name),
                "Find refs failed",
            )
            .await
            {
                let _ = this.update(cx, |o, cx| {
                    o.refs = r;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// `gotoDef`: the definition index when the editor's LSP jump did not apply.
    pub(crate) fn goto_def(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let n = name.trim().to_string();
        if n.is_empty() {
            return;
        }
        let svc = self.services.clone();
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let Some(defs) = recur_view_core::call(
                cx,
                svc.code.find_defs(&pid, &wid, &n),
                "Find definition failed",
            )
            .await
            else {
                return;
            };
            let _ = this.update_in(cx, |o, window, cx| {
                if defs.is_empty() {
                    toast(ToastKind::Info, format!("No definition for “{n}”"), cx);
                    return;
                }
                if defs.len() == 1 {
                    o.open_path(
                        OpenRequest::new(defs[0].path.clone()).line(defs[0].line.max(0) as usize),
                        window,
                        cx,
                    )
                    .detach();
                    return;
                }
                o.select_code_panel(recur_view_core::prefs::CodePanelKey::Refs, window, cx);
                o.refs = defs
                    .iter()
                    .map(|d| recur_domain::SymbolRef {
                        path: d.path.clone(),
                        line: d.line,
                        text: format!("{} {}", d.kind, d.name),
                        def: true,
                    })
                    .collect();
                toast(
                    ToastKind::Info,
                    format!("{} definitions for “{n}”", defs.len()),
                    cx,
                );
                cx.notify();
            });
        })
        .detach();
    }

    /// `applyProblemFix`: the server's quick fix for a Problems row.
    pub(crate) fn apply_problem_fix(&mut self, p: LspProblem, cx: &mut Context<Self>) {
        if p.workspace_id
            .as_deref()
            .is_some_and(|w| w != self.props.workspace_id)
        {
            return;
        }
        self.problem_fix_busy = format!("{}:{}:{}", p.uri, p.line, p.message);
        cx.notify();
        let hub = LspHub::global(cx);
        let fallback = self.lsp_attach.as_ref().map(|a| a.client.clone());
        cx.spawn(async move |this, cx| {
            let ok = apply_lsp_quick_fix(&hub, &p, fallback, cx).await;
            let _ = this.update(cx, |o, cx| {
                if ok
                    && !o.selected_path.is_empty()
                    && let Some(e) = o.main_editor.clone()
                {
                    o.draft = e.update(cx, |e, cx| e.get_doc(cx));
                }
                o.problem_fix_busy.clear();
                cx.notify();
            });
        })
        .detach();
    }

    /// A finding's gutter icon: open it in the Findings panel.
    pub(crate) fn open_finding_from_editor(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if id.is_empty() {
            return;
        }
        self.select_code_panel(recur_view_core::prefs::CodePanelKey::Findings, window, cx);
        self.findings_filter = crate::obs::FindingsFilter::Open;
        let id = id.to_string();
        self.findings_panel
            .update(cx, |p, cx| p.expand(vec![id], None, cx));
        cx.notify();
    }

    /// ⌘L `addEditorMentionToChat`: the active file or selection as an `@` chip.
    pub(crate) fn add_editor_mention_to_chat(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.props.can_add_editor_mention {
            return;
        }
        if !self
            .panels_on(recur_view_core::prefs::PanelSide::Left, cx)
            .contains(&recur_view_core::prefs::CodePanelKey::Maestro)
        {
            return;
        }
        self.sync_editors(window, cx);
        let use_split = self.focused_pane == Pane::Split && self.split_editor.is_some();
        let path = if use_split && !self.split_path.is_empty() {
            self.split_path.clone()
        } else {
            self.selected_path.clone()
        };
        if path.is_empty() || is_scratch_path(&path) {
            return;
        }
        let focused = if self.show_diff() {
            self.merge_editor.as_ref().map(|(_, e)| e.clone())
        } else if use_split {
            self.split_editor.clone()
        } else {
            self.main_editor.clone()
        };
        let sel = focused.and_then(|e| e.read(cx).mention_selection());
        let focus = self.focus_agent_panel(window, cx);
        cx.spawn(async move |this, cx| {
            focus.await;
            let _ = this.update(cx, |_, cx| {
                let ev = match sel {
                    Some(s) => {
                        let (sc, ec) = s.cols.unwrap_or((0, 0));
                        crate::obs::ObservatoryEvent::AddEditorMention {
                            path,
                            start_line: s.start_line,
                            end_line: s.end_line,
                            start_col: sc,
                            end_col: ec,
                        }
                    }
                    None => crate::obs::ObservatoryEvent::AddEditorMention {
                        path,
                        start_line: 0,
                        end_line: 0,
                        start_col: 0,
                        end_col: 0,
                    },
                };
                cx.emit(ev);
            });
        })
        .detach();
    }
}
