//! Props, roots and lifecycle: the root switch (`$effect.pre` keyed by project + workspace)
//! with the keep-alive snapshot and the backend editor session, persisting that session,
//! backend events (worktree changes, findings, artifacts), deep links (`observatoryJump`), the
//! shell activation edges (hot shells), the worktree watcher and the native menu state.

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::{BTreeMap, BTreeSet, HashSet};

use gpui_kit::{App, AppContext as _, Context, Task, Window};
use recur_editor::session::{
    Session, StashedBuffer, max_scratch_seq, prune_scratch_docs, unique_open_files,
};
use recur_editor::view_state::{EDITOR_BODY_CAP, EditorViewMode, editor_text_eq, prune_file_views};
use recur_services::AppEvent;
use recur_textmate::OrderedMap;
use recur_view_core::ShellEvent;
use recur_view_core::prefs::CodePanelKey;

use crate::model::{FocusZone, MainSurface, Pane, TabBuffer, is_scratch_path};
use crate::obs::{ExplorerReveal, FindingsFilter, Observatory, ObservatoryProps, OpenRequest};
use crate::obs_cache::{ObservatorySnapshot, observatory_cache_key};
use crate::workspace_file::is_binary_preview_mime;

fn ordered(m: Option<&BTreeMap<String, String>>) -> OrderedMap {
    OrderedMap(
        m.map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
    )
}

fn btree(m: &OrderedMap) -> BTreeMap<String, String> {
    m.0.iter().cloned().collect()
}

fn split_root(root: &str) -> (String, String) {
    match root.find('\0') {
        Some(i) => (root[..i].to_string(), root[i + 1..].to_string()),
        None => (root.to_string(), String::new()),
    }
}

/// The store topics the Observatory reads; live agents, the inbox, briefing, usage, resource
/// and the menu state it pushes itself are not among them.
const OBS_TOPICS: Topics = Topics::ALL.without(
    Topics::AGENTS
        .union(Topics::INBOX)
        .union(Topics::BRIEFING)
        .union(Topics::USAGE)
        .union(Topics::RESOURCE)
        .union(Topics::MENU),
);

/// Topics that change with every agent event in any workspace: the Observatory follows only
/// what it shows of them ([`ObsStoreKey`]).
const KEYED_TOPICS: Topics = Topics::WORKSPACES
    .union(Topics::ATTENTION)
    .union(Topics::PRS);

/// What the Observatory reads of [`KEYED_TOPICS`]: its workspace, which workspaces exist, its
/// unseen chat badge and whether its agent docks beside the selected PR.
#[derive(PartialEq)]
pub(crate) struct ObsStoreKey {
    workspace: Option<recur_domain::Workspace>,
    ids: u64,
    unseen: usize,
    portal: bool,
}

impl Observatory {
    /// Push new props (`$props()`): a new project / workspace switches roots.
    pub fn set_props(
        &mut self,
        props: ObservatoryProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.window.is_none() {
            self.window = Some(window.window_handle());
        }
        let root = observatory_cache_key(&props.project_id, &props.workspace_id);
        if root != self.loaded_root {
            self.leave_root(window, cx);
        }
        let prev = std::mem::replace(&mut self.props, props);
        if root != self.loaded_root {
            self.on_root_change(root, window, cx);
        } else if prev.left_open != self.props.left_open {
            // The chat dock flag is pane chrome the session remembers.
            self.persist_editor_session(cx);
        }
        if !self.props.initial_path.is_empty() && self.props.initial_path != prev.initial_path {
            let p = self.props.initial_path.clone();
            self.open_path(OpenRequest::new(p), window, cx).detach();
        }
        if prev.shell_active != self.props.shell_active
            || prev.workspace_id != self.props.workspace_id
        {
            self.activation_changed(prev.shell_active, &prev.workspace_id, window, cx);
        }

        self.after_state_change(window, cx);
        cx.notify();
    }

    /// Observe the app state, the editor environment, problems and backend events.
    pub(crate) fn subscribe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        self._subscriptions.push(cx.observe_state_changes_in(
            &state,
            OBS_TOPICS,
            window,
            |o, topics, window, cx| {
                if topics.without(KEYED_TOPICS).is_empty() && !o.store_key_changed(cx) {
                    return;
                }
                o.after_state_change(window, cx)
            },
        ));
        self._subscriptions.push(cx.guarded_subscribe_in(
            &state,
            window,
            |o, _, ev: &ShellEvent, window, cx| match ev {
                ShellEvent::MenuAction(a) => o.on_menu_action(a, window, cx),
                ShellEvent::WorkspaceGone(id) => o.on_workspace_gone(id, cx),
                _ => {}
            },
        ));
        let env = recur_view_core::editor_env::editor_env(&state, cx);
        self._subscriptions
            .push(cx.guarded_observe_in(&env, window, |o, env, window, cx| {
                o.sync_editors(window, cx);
                // The session remembers language picks (`syntaxByPath` / `syntaxByFileType`).
                let syntax = env.read(cx).syntax_epoch();
                if o.seen_syntax_epoch != Some(syntax) {
                    let first = o.seen_syntax_epoch.is_none();
                    o.seen_syntax_epoch = Some(syntax);
                    if !first && o.syntax_maps_ready {
                        o.persist_editor_session(cx);
                    }
                }
                cx.notify();
            }));
        self._subscriptions
            .push(cx.observe_global_in::<crate::lsp::problems::LspProblems>(
                window,
                |o, window, cx| {
                    o.paint_lsp_diagnostics(window, cx);
                    cx.notify();
                },
            ));
        self._subscriptions.push(
            cx.observe_global_in::<recur_view_widgets::keybinding_menu::KeybindingMenuStore>(
                window,
                |_, _, cx| cx.notify(),
            ),
        );
        self._subscriptions.push(cx.guarded_subscribe_in(
            &self.prompts.clone(),
            window,
            |o, _, ev: &recur_view_widgets::command_prompts::CommandPromptsEvent, _, cx| match ev {
                recur_view_widgets::command_prompts::CommandPromptsEvent::Resolve(v) => {
                    o.resolve_prompts(Some(v.clone()), cx)
                }
                recur_view_widgets::command_prompts::CommandPromptsEvent::Cancel => {
                    o.resolve_prompts(None, cx)
                }
            },
        ));
        self._subscriptions.push(
            cx.on_focus_in(&self.focus.clone(), window, |o, window, cx| {
                o.on_obs_focus_in(window, cx)
            }),
        );
        for h in [
            self.main_focus.clone(),
            self.tabs_focus.clone(),
            self.files_focus.clone(),
            self.scm_focus.clone(),
            self.left_focus.clone(),
        ] {
            let subs = self.watch_zone(&h, window, cx);
            self._subscriptions.extend(subs);
        }
        self._subscriptions
            .push(cx.on_release(|o, cx| o.on_destroy(cx)));
        self._subscriptions.push(Self::observe_keys(cx));
        self._subscriptions
            .push(cx.observe_window_activation(window, |o, window, cx| {
                if !window.is_window_active() {
                    o.page_hide(window, cx);
                }
            }));
        self._subscriptions.push(cx.on_app_quit(|o, cx| {
            if let Some(w) = o.window {
                let _ = w.update(cx, |_, window, cx| o.page_hide(window, cx));
            }
            crate::session_host::flush_editor_session_now(cx);
            async {}
        }));
        self._subscriptions
            .push(cx.observe_window_bounds(window, |o, window, cx| {
                o.window_fullscreen_changed(window, cx)
            }));
        self._subscriptions.push(cx.guarded_subscribe_in(
            &self.findings_panel.clone(),
            window,
            |o, _, ev: &crate::findings_panel::FindingsPanelEvent, window, cx| {
                use crate::findings_panel::FindingsPanelEvent as E;
                match ev {
                    E::Open(f) => {
                        if !f.file.is_empty() {
                            let req = crate::OpenRequest::new(f.file.clone())
                                .line(f.line.max(0) as usize)
                                .end_line(f.end_line.max(0) as usize);
                            o.open_path(req, window, cx).detach();
                        }
                    }
                    E::SetStatus(f, status) => o.set_finding_status(f, status, cx),
                    E::AddNote(f, body) => o.add_finding_note(f, body, cx),
                    E::DeleteNote(f, id) => o.delete_finding_note(f, id, cx),
                }
            },
        ));
        self._subscriptions.push(cx.guarded_subscribe_in(
            &self.quick_open.clone(),
            window,
            |o, _, ev: &crate::quick_open::QuickOpenEvent, window, cx| {
                use crate::quick_open::QuickOpenEvent as E;
                match ev {
                    E::PickFile { path, line } => {
                        let req = crate::OpenRequest::new(path.clone())
                            .line(line.unwrap_or(0))
                            .explorer(crate::ExplorerReveal::Scroll);
                        o.open_path(req, window, cx).detach();
                    }
                    E::PickSymbol { line, .. } => o.scroll_to_line(*line, 0, 0, 0, window, cx),
                    E::Closed => {}
                }
            },
        ));
        crate::media::install_image_loader(&self.services, cx);
        let mut events = self.services.subscribe();
        self.event_task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Some(ev) = events.recv().await {
                if this
                    .update_in(cx, |o, window, cx| o.on_app_event(ev, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        crate::lsp::client::LspHub::global(cx).wire(&self.services, cx);
        crate::session_host::set_services(self.services.clone(), cx);
    }

    /// Things the Svelte derived from stores (`$workspaces`, `$codePanelSides`, dock maps…).
    /// Whether what this Observatory reads of the [`KEYED_TOPICS`] changed since it last looked.
    fn store_key_changed(&mut self, cx: &App) -> bool {
        let s = self.state.read(cx);
        let ids = {
            use std::hash::{Hash, Hasher};
            let mut h = std::hash::DefaultHasher::new();
            for w in &s.workspaces {
                (&w.id, w.archived).hash(&mut h);
            }
            h.finish()
        };
        let key = ObsStoreKey {
            workspace: s.workspace(&self.props.workspace_id).cloned(),
            ids,
            unseen: self.unseen_chat_count(cx),
            portal: self.portal_agent_to_pr(cx),
        };
        if self.store_key.as_ref() == Some(&key) {
            return false;
        }
        self.store_key = Some(key);
        true
    }

    pub(crate) fn after_state_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.track_base(cx);
        self.keep_panel_kinds_valid(cx);
        self.dock_state_changed(window, cx);
        self.handle_jump(window, cx);
        self.arm_watcher(cx);
        self.prune_caches(cx);
        self.retry_skipped_tree(cx);
        let disabled = self
            .state
            .read(cx)
            .settings
            .as_ref()
            .map(|s| {
                let mut v = s.disabled_language_servers.clone();
                v.sort();
                v.join("\0")
            })
            .unwrap_or_default();
        match &self.prev_disabled_ls {
            None => self.prev_disabled_ls = Some(disabled),
            Some(prev) if *prev != disabled => {
                self.prev_disabled_ls = Some(disabled);
                let p = self.selected_path.clone();
                if !p.is_empty() {
                    self.refresh_lsp(&p, window, cx);
                }
            }
            _ => {}
        }
        if self.max_open_files_seen != self.max_open_files(cx) {
            self.max_open_files_seen = self.max_open_files(cx);
            self.enforce_max_open_files(cx);
        }
        self.sync_side_effects(cx);
        cx.notify();
    }

    /// `$effect`s that mirror state outward: occluder, menu state, editor command owner,
    /// unseen artifacts, the chord index, emitted snippet values.
    pub(crate) fn sync_side_effects(&mut self, cx: &mut Context<Self>) {
        self.clamp_view_mode();
        if self.props.shell_active {
            let id = self.props.workspace_id.clone();
            let occlude = self.slots.left_panel.is_some() && !self.maestro_panel_visible(cx);
            cx.default_global::<crate::Owners>().occluder = id.clone();
            self.state
                .update(cx, |s, cx| s.set_chat_occluder("agent-pane", occlude, cx));
            self.push_menu_state(cx);
            if self.artifacts_panel_focused(cx) && !id.is_empty() {
                let cleared = recur_engine::view::artifacts::clear_unseen_artifact_ids(
                    &cx.default_global::<crate::UnseenArtifacts>().0,
                    &id,
                );
                if cleared != cx.default_global::<crate::UnseenArtifacts>().0 {
                    cx.default_global::<crate::UnseenArtifacts>().0 = cleared;
                    cx.notify();
                }
            }
            self.refresh_chord_index(cx);
        }
        if !self.term_open(cx) {
            self.term_tab_count = 0;
        }
        let visible = self.maestro_panel_visible(cx);
        if self.emitted_maestro_visible != Some(visible) {
            self.emitted_maestro_visible = Some(visible);
            cx.emit(crate::obs::ObservatoryEvent::MaestroVisible(visible));
        }
        let lit = self.focus_zone == FocusZone::Agent;
        if self.emitted_agent_lit != Some(lit) {
            self.emitted_agent_lit = Some(lit);
            cx.emit(crate::obs::ObservatoryEvent::AgentLit(lit));
        }
        let art = self.selected_artifact_id();
        if self.emitted_artifact.as_ref() != Some(&art) {
            self.emitted_artifact = Some(art.clone());
            cx.emit(crate::obs::ObservatoryEvent::SelectedArtifact(art));
        }
    }

    // ---- the root switch ----

    /// Snapshot and persist the outgoing root while its props (the host's chat dock flag)
    /// still describe it.
    fn leave_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let outgoing = std::mem::take(&mut self.loaded_root);
        if !outgoing.is_empty() && outgoing != "\0" {
            let snap = self.capture_snapshot(Some(window), cx);
            crate::obs_cache::cache(cx).put(&outgoing, snap);
            self.persist_session_for_root(&outgoing, Some(window), cx);
            self.park_editor_sessions(&outgoing, cx);
            let (_, prev_wid) = split_root(&outgoing);
            self.detach_lsp(cx);
            if !prev_wid.is_empty() {
                crate::lsp::client::LspHub::global(cx).clear_workspace(&prev_wid, cx);
                let svc = self.services.clone();
                cx.background_spawn(async move {
                    let _ = svc.lsp.close_workspace_lsp(&prev_wid).await;
                })
                .detach();
            }
        }
    }

    fn on_root_change(&mut self, root: String, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_root(window, cx);
        self.loaded_root = root.clone();
        self.root_token += 1;
        self.git_status_gen += 1;
        self.working_status_gen += 1;
        self.worktree_missing = false;
        self.base_doc_cache.clear();
        self.base_doc_inflight.clear();
        self.base_doc_epoch += 1;
        let cached = if root != "\0" {
            crate::obs_cache::cache(cx).get(&root)
        } else {
            None
        };
        if let Some(snap) = cached {
            self.apply_snapshot(snap, window, cx);
            self.editor_session_key = root.clone();
            if !self.session_load_settled {
                self.restore_session_from_backend(root.clone(), self.root_token, window, cx);
            }
            self.refresh_tree(true, cx).detach();
            if !self.selected_path.is_empty()
                && self.file.is_some()
                && !self.main_editor_dirty
                && editor_text_eq(&self.draft, &self.saved_content)
            {
                self.refresh_open_file_if_stale(cx).detach();
            }
            let sel = self.selected_path.clone();
            if !sel.is_empty() {
                self.reveal_in_tree(&sel, cx).detach();
                self.prefetch_base_doc(&sel, cx);
            }
            let split = self.split_path.clone();
            if !split.is_empty() {
                self.prefetch_base_doc(&split, cx);
            }
            if !self.props.workspace_id.is_empty() {
                self.load_findings(cx).detach();
            }
            self.skip_view_restore = false;
            let token = self.root_token;
            let zone = self.focus_zone;
            cx.on_next_frame(window, move |o, window, cx| {
                if o.root_token != token {
                    return;
                }
                o.sync_editors(window, cx);
                cx.on_next_frame(window, move |o, window, cx| {
                    if o.root_token == token {
                        o.restore_chrome_focus(zone, token, window, cx);
                    }
                });
            });
            return;
        }
        // Cold open: drop the previous worktree's data now so scopes never mix.
        self.reset_explorer_data(cx);
        let saved_scope = crate::session_host::load_explorer_scope(
            self.state.read(cx).storage(),
            &self.props.project_id,
            &self.props.workspace_id,
        );
        self.scope_chosen = saved_scope.is_some();
        self.browse_all = saved_scope.unwrap_or(!self.props.loop_scoped);
        self.changed_collapsed.clear();
        self.open_files.clear();
        self.scratch_docs.clear();
        self.scratch_seq = 0;
        self.stashed_buffers.clear();
        self.file = None;
        self.selected_is_dir = false;
        self.selected_path.clear();
        self.tree_path.clear();
        self.close_split(window, cx);
        self.session_restored = false;
        self.closed_tabs.clear();
        self.right_kind = CodePanelKey::Files;
        self.left_kind = CodePanelKey::Maestro;
        self.show_side = true;
        self.main_editor_dirty = false;
        self.split_editor_dirty = false;
        self.focus_zone = FocusZone::Editor;
        self.base_doc.clear();
        self.base_doc_path.clear();
        self.view_mode = EditorViewMode::Code;
        self.file_views.clear();
        let ws = self.props.workspace_id.clone();
        self.preview_scroll
            .borrow_mut()
            .hydrate(std::iter::empty(), &ws);
        self.hydrate_syntax(None, None, false, cx);
        self.restoring_session_views = false;
        self.restoring_session_views_token = 0;
        self.skip_view_restore = false;
        self.file_tab_scroll_restore = None;
        self.review_restore = None;
        self.main_surface_pref = MainSurface::Editor;
        self.draft.clear();
        self.saved_content.clear();
        self.editor_path.clear();
        self.tab_buffers.clear();
        self.dirty_tabs.clear();
        self.editor_session_key = root.clone();
        self.refresh_tree(false, cx).detach();
        if !ws.is_empty() {
            self.load_findings(cx).detach();
        }
        self.session_load_settled = false;
        self.syntax_maps_ready = false;
        self.restore_session_from_backend(root, self.root_token, window, cx);
        if self.editor_hidden(cx) && self.term_open(cx) && !ws.is_empty() {
            let token = self.root_token;
            cx.on_next_frame(window, move |o, window, cx| {
                cx.on_next_frame(window, move |o2, window, cx| {
                    let _ = o;
                    if o2.root_token == token {
                        o2.restore_chrome_focus(FocusZone::Terminal, token, window, cx);
                    }
                });
            });
        }
    }

    /// `hydrateSyntaxMode` on the shared catalog.
    pub(crate) fn hydrate_syntax(
        &mut self,
        by_path: Option<&BTreeMap<String, String>>,
        by_type: Option<&BTreeMap<String, String>>,
        merge_live: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(env) = recur_view_core::editor_env::editor_env_peek(cx) else {
            return;
        };
        let changed = {
            let env = env.read(cx);
            let mut tm = env.tm.borrow_mut();
            let before = tm.catalog.syntax_mode.epoch();
            tm.catalog
                .syntax_mode
                .hydrate(ordered(by_path), ordered(by_type), merge_live);
            tm.catalog.syntax_mode.epoch() != before
        };
        if changed {
            env.update(cx, |e, cx| e.syntax_changed(cx));
        }
    }

    fn syntax_snapshot(
        &self,
        cx: &gpui_kit::App,
    ) -> (BTreeMap<String, String>, BTreeMap<String, String>, bool) {
        match recur_view_core::editor_env::editor_env_peek(cx) {
            Some(env) => {
                let env = env.read(cx);
                let tm = env.tm.borrow();
                let s = tm.catalog.syntax_mode.snapshot();
                let non_empty = !s.syntax_by_path.is_empty() || !s.syntax_by_file_type.is_empty();
                (
                    btree(&s.syntax_by_path),
                    btree(&s.syntax_by_file_type),
                    non_empty,
                )
            }
            None => (BTreeMap::new(), BTreeMap::new(), false),
        }
    }

    /// `captureSnapshot`: everything the keep-alive cache restores synchronously.
    pub(crate) fn capture_snapshot(
        &mut self,
        window: Option<&Window>,
        cx: &mut gpui_kit::App,
    ) -> ObservatorySnapshot {
        let main_draft = match &self.main_editor {
            Some(e) if !self.selected_path.is_empty() && !self.file_is_binary_preview() => {
                e.update(cx, |e, cx| e.get_doc(cx))
            }
            _ => self.draft.clone(),
        };
        let side_draft = match &self.split_editor {
            Some(e) if self.split_open && !self.split_path.is_empty() => {
                e.update(cx, |e, cx| e.get_doc(cx))
            }
            _ => self.split_draft.clone(),
        };
        let mut docs = self.scratch_docs.clone();
        let blank = || recur_editor::session::ScratchDoc {
            title: "Scratch".into(),
            content: String::new(),
            lang: "txt".into(),
            mime: None,
        };
        if is_scratch_path(&self.selected_path) {
            docs.entry(self.selected_path.clone())
                .or_insert_with(blank)
                .content = main_draft.clone();
        }
        if self.split_open && is_scratch_path(&self.split_path) {
            docs.entry(self.split_path.clone())
                .or_insert_with(blank)
                .content = side_draft.clone();
        }
        let main_dirty = self.main_editor_dirty
            || (self.file.is_some()
                && !self.file_is_binary_preview()
                && !editor_text_eq(&main_draft, &self.saved_content));
        let split_dirty = self.split_editor_dirty
            || (!self.split_path.is_empty() && !editor_text_eq(&side_draft, &self.split_saved));
        let big = |s: &str| s.encode_utf16().count() > EDITOR_BODY_CAP;
        let slim_main = !main_dirty && !is_scratch_path(&self.selected_path) && big(&main_draft);
        let slim_split = !split_dirty && !is_scratch_path(&self.split_path) && big(&side_draft);
        let slim_file = self.file.clone().map(|mut f| {
            if slim_main {
                f.content.clear();
            }
            f
        });
        if !self.restoring_session_views
            && let Some(w) = window
        {
            self.remember_leaving_editors(w, cx);
        }
        if !self.selected_path.is_empty()
            && let Some(f) = self.file.clone()
            && !is_binary_preview_mime(&f.mime)
        {
            let (p, saved) = (self.selected_path.clone(), self.saved_content.clone());
            self.put_tab_buffer(
                &p,
                TabBuffer {
                    draft: main_draft.clone(),
                    saved,
                    file: f,
                },
            );
        }
        self.flush_editor_caches(Pane::Main, cx);
        self.flush_editor_caches(Pane::Split, cx);
        let dirty_bufs: BTreeMap<String, TabBuffer> = self
            .tab_buffers
            .iter()
            .filter(|(_, b)| !editor_text_eq(&b.draft, &b.saved))
            .map(|(p, b)| {
                let mut file = b.file.clone();
                file.content = b.saved.clone();
                (
                    p.clone(),
                    TabBuffer {
                        draft: b.draft.clone(),
                        saved: b.saved.clone(),
                        file,
                    },
                )
            })
            .collect();
        let mut buffers = self.stashed_buffers.clone();
        if !self.selected_path.is_empty() && !is_scratch_path(&self.selected_path) && main_dirty {
            buffers.insert(
                self.selected_path.clone(),
                StashedBuffer {
                    draft: main_draft.clone(),
                    saved: Some(self.saved_content.clone()),
                    base_hash: None,
                },
            );
        }
        if self.split_open
            && !self.split_path.is_empty()
            && !is_scratch_path(&self.split_path)
            && split_dirty
        {
            buffers.insert(
                self.split_path.clone(),
                StashedBuffer {
                    draft: side_draft.clone(),
                    saved: Some(self.split_saved.clone()),
                    base_hash: None,
                },
            );
        }
        let (syn_path, syn_type, _) = self.syntax_snapshot(cx);
        let review = self
            .review_surface
            .as_ref()
            .map(|r| r.read(cx).view_state(cx));
        let mut keep: Vec<&str> = self.open_files.iter().map(String::as_str).collect();
        if self.split_open && !self.split_path.is_empty() {
            keep.push(&self.split_path);
        }
        let views: std::collections::HashMap<_, _> = self
            .file_views
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let file_views = self
            .preview_scroll
            .borrow()
            .apply_to_file_views(&views, keep.iter().copied())
            .unwrap_or(views)
            .into_iter()
            .collect();
        ObservatorySnapshot {
            tree: self.tree.clone(),
            expanded: self.expanded.clone(),
            loaded_dirs: self.loaded_dirs.clone(),
            changed_collapsed: self.changed_collapsed.clone(),
            changed_paths: self.changed_paths.clone(),
            git_status: self.git_status.clone(),
            git_status_ready: self.git_status_ready,
            working_status: self.working_status.clone(),
            staged_files: self.staged_files.clone(),
            open_files: self.open_files.clone(),
            closed_tabs: self.closed_tabs.clone(),
            selected_path: self.selected_path.clone(),
            tree_path: self.tree_path.clone(),
            file: slim_file,
            draft: if slim_main { String::new() } else { main_draft },
            saved_content: if slim_main {
                String::new()
            } else {
                self.saved_content.clone()
            },
            outline: self.outline.clone(),
            view_mode: self.view_mode,
            base_doc: self.base_doc.clone(),
            base_doc_path: self.base_doc_path.clone(),
            browse_all: self.browse_all,
            scope_chosen: self.scope_chosen,
            left_kind: self.left_kind,
            right_kind: self.right_kind,
            left_open: Some(self.left_gutter_open()),
            show_side: Some(self.show_side),
            split_open: self.split_open,
            split_path: self.split_path.clone(),
            split_draft: if slim_split {
                String::new()
            } else {
                side_draft
            },
            split_saved: if slim_split {
                String::new()
            } else {
                self.split_saved.clone()
            },
            split_writable: self.split_writable,
            focused_pane: self.focused_pane,
            focus_zone: self.focus_zone,
            main_editor_dirty: main_dirty,
            split_editor_dirty: split_dirty,
            session_restored: self.session_restored,
            session_load_settled: self.session_load_settled,
            syntax_maps_ready: Some(self.syntax_maps_ready),
            scratch_docs: docs,
            scratch_seq: self.scratch_seq,
            file_views,
            review_tab: Some(self.review_tab),
            review_selected_path: review.as_ref().map(|r| r.selected_path.clone()),
            review_scroll_top: review.as_ref().map(|r| r.scroll_top),
            main_surface: self.main_surface_pref,
            syntax_by_path: syn_path.into_iter().collect(),
            syntax_by_file_type: syn_type.into_iter().collect(),
            tab_buffers: (!dirty_bufs.is_empty()).then_some(dirty_bufs),
            stashed_buffers: buffers,
            file_tab_scroll_left: Some(-f32::from(self.file_tab_scroll.offset().x)),
        }
    }

    /// `applySnapshot`.
    fn apply_snapshot(
        &mut self,
        snap: ObservatorySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end_tree_load();
        self.tree = snap.tree;
        self.expanded = snap.expanded;
        self.loaded_dirs = snap.loaded_dirs;
        self.loading_dirs.clear();
        self.dir_not_found_since.clear();
        self.maybe_clear_gone_confirm();
        self.changed_collapsed = snap.changed_collapsed;
        self.changed_paths = snap.changed_paths;
        self.git_status = snap.git_status;
        self.paths_gen += 1;
        self.git_status_ready = snap.git_status_ready;
        self.working_status = snap.working_status;
        self.staged_files = snap.staged_files;
        self.working_changed();
        self.open_files = unique_open_files(&snap.open_files);
        self.closed_tabs = snap.closed_tabs;
        self.selected_path = snap.selected_path.clone();
        self.tree_path = if snap.tree_path.is_empty() {
            snap.selected_path.clone()
        } else {
            snap.tree_path
        };
        self.file = snap.file.clone();
        self.draft = snap.draft.clone();
        self.saved_content = snap.saved_content.clone();
        self.outline = snap.outline;
        self.view_mode = snap.view_mode;
        self.base_doc = snap.base_doc.clone();
        self.base_doc_path = snap.base_doc_path.clone();
        if !snap.base_doc_path.is_empty() && !snap.base_doc.is_empty() {
            self.remember_base_doc(&snap.base_doc_path, &snap.base_doc);
        }
        self.browse_all = snap.browse_all;
        self.scope_chosen = snap.scope_chosen;
        self.left_kind = snap.left_kind;
        self.right_kind = snap.right_kind;
        if let Some(side) = snap.show_side {
            self.show_side = side;
        }
        self.split_open = snap.split_open;
        self.split_path = snap.split_path.clone();
        self.split_draft = snap.split_draft.clone();
        self.split_saved = snap.split_saved.clone();
        self.split_writable = snap.split_writable;
        self.focused_pane = snap.focused_pane;
        self.focus_zone = snap.focus_zone;
        self.main_editor_dirty = snap.main_editor_dirty;
        self.split_editor_dirty = snap.split_editor_dirty;
        self.session_restored = snap.session_restored;
        self.scratch_docs = snap.scratch_docs;
        self.scratch_seq = if snap.scratch_seq > 0 {
            snap.scratch_seq
        } else {
            max_scratch_seq(self.scratch_docs.keys().map(String::as_str))
        };
        self.file_views = snap.file_views;
        let ws = self.props.workspace_id.clone();
        self.preview_scroll
            .borrow_mut()
            .hydrate(self.file_views.iter(), &ws);
        let by_path: BTreeMap<String, String> = snap.syntax_by_path.into_iter().collect();
        let by_type: BTreeMap<String, String> = snap.syntax_by_file_type.into_iter().collect();
        self.hydrate_syntax(Some(&by_path), Some(&by_type), false, cx);
        self.restoring_session_views = false;
        self.restoring_session_views_token = 0;
        self.editor_path = if !snap.selected_path.is_empty() && snap.file.is_some() {
            snap.selected_path.clone()
        } else {
            String::new()
        };
        self.hydrate_failed_path.clear();
        let mut bufs: BTreeMap<String, TabBuffer> = BTreeMap::new();
        if let Some(f) = &snap.file
            && !snap.selected_path.is_empty()
            && (!snap.draft.is_empty() || !snap.saved_content.is_empty())
        {
            bufs.insert(
                snap.selected_path.clone(),
                TabBuffer {
                    draft: snap.draft.clone(),
                    saved: snap.saved_content.clone(),
                    file: f.clone(),
                },
            );
        }
        bufs.extend(snap.tab_buffers.unwrap_or_default());
        self.tab_buffers = bufs;
        let mut dirty: BTreeSet<String> = self
            .tab_buffers
            .iter()
            .filter(|(_, b)| !editor_text_eq(&b.draft, &b.saved))
            .map(|(p, _)| p.clone())
            .collect();
        if snap.main_editor_dirty && !snap.selected_path.is_empty() {
            dirty.insert(snap.selected_path.clone());
        }
        if snap.split_editor_dirty && !snap.split_path.is_empty() {
            dirty.insert(snap.split_path.clone());
        }
        self.dirty_tabs = dirty;
        self.stashed_buffers = snap.stashed_buffers;
        self.file_tab_scroll_restore_id += 1;
        self.file_tab_scroll_restore = snap
            .file_tab_scroll_left
            .map(|l| (l, self.file_tab_scroll_restore_id));
        if let Some(t) = snap.review_tab {
            self.set_review_tab(t, cx);
        }
        self.review_restore = snap
            .review_selected_path
            .map(|p| (p, snap.review_scroll_top.unwrap_or(0.)));
        self.main_surface_pref = snap.main_surface;
        self.session_load_settled = snap.session_load_settled || !snap.open_files.is_empty();
        self.syntax_maps_ready = snap.syntax_maps_ready.unwrap_or(snap.session_load_settled);
        // Last: the host's dock flag persists the session, which must see the whole snapshot.
        if let Some(open) = snap.left_open {
            self.set_left_gutter_open(open, cx);
        }
        self.hits.clear();
        self.refs.clear();
        self.search_skipped_large = 0;
        self.search_truncated = false;
        self.findings.clear();
        self.expanded_lists.clear();
        self.open_file_not_found_path.clear();
        self.open_file_not_found_since = None;
        self.maybe_clear_gone_confirm();
        self.set_file_loading(false, cx);
        let deleted_diff = snap.file.is_none()
            && snap.view_mode == EditorViewMode::Diff
            && snap.base_doc_path == snap.selected_path;
        let main_needs_body = !snap.selected_path.is_empty()
            && !deleted_diff
            && !is_scratch_path(&snap.selected_path)
            && !snap
                .file
                .as_ref()
                .is_some_and(|f| is_binary_preview_mime(&f.mime))
            && snap
                .file
                .as_ref()
                .is_none_or(|f| f.content.is_empty() && f.size > 0);
        if main_needs_body {
            self.hydrate_file(&snap.selected_path, 0, 0, 0, 0, window, cx)
                .detach();
        } else if !snap.selected_path.is_empty() && snap.file.is_some() && !deleted_diff {
            let p = snap.selected_path.clone();
            cx.defer_in(window, move |o, window, cx| o.refresh_lsp(&p, window, cx));
        }
        if snap.split_open
            && !snap.split_path.is_empty()
            && !is_scratch_path(&snap.split_path)
            && snap.split_draft.is_empty()
            && snap.split_saved.is_empty()
        {
            self.restore_split_buffer(snap.split_path, cx);
        }
    }

    fn restore_split_buffer(&mut self, path: String, cx: &mut Context<Self>) {
        let read = self.read_editor_file(&path, cx);
        cx.spawn(async move |this, cx| {
            let Ok(f) = read.await else { return };
            let _ = this.update(cx, |o, cx| {
                if o.split_path != path {
                    return;
                }
                o.split_draft = f.content.clone();
                o.split_saved = f.content.clone();
                o.split_editor_dirty = false;
                o.split_writable = f.writable
                    && !is_binary_preview_mime(&f.mime)
                    && f.size <= crate::model::MAX_EDIT_BYTES;
                o.overlay_split_stash(&path, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// `restoreSessionFromBackend`.
    fn restore_session_from_backend(
        &mut self,
        root: String,
        token: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let load = crate::session_host::load_editor_session(&self.props.workspace_id, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(loaded) = load.await else { return };
            let _ = this.update_in(cx, |o, window, cx| {
                if o.root_token != token || o.loaded_root != root {
                    return;
                }
                o.session_load_settled = true;
                o.syntax_maps_ready = loaded
                    .as_ref()
                    .is_none_or(|l| l.syntax_maps_known != Some(false));
                let sess = loaded.map(|l| l.session);
                o.hydrate_syntax(
                    sess.as_ref().and_then(|s| s.syntax_by_path.as_ref()),
                    sess.as_ref().and_then(|s| s.syntax_by_file_type.as_ref()),
                    true,
                    cx,
                );
                if o.main_surface_pref != MainSurface::Review
                    && o.open_files.is_empty()
                    && o.selected_path.is_empty()
                {
                    o.main_surface_pref = if sess.as_ref().is_some_and(|s| s.main_surface_review) {
                        MainSurface::Review
                    } else {
                        MainSurface::Editor
                    };
                }
                o.apply_session_pane_chrome(sess.as_ref(), cx);
                if !o.props.initial_path.is_empty() {
                    return;
                }
                let Some(sess) = sess.filter(|s| !s.open_files.is_empty()) else {
                    if !o.open_files.is_empty() || !o.selected_path.is_empty() {
                        o.write_editor_session(false, None, None, cx);
                    }
                    return;
                };
                if !o.open_files.is_empty() || !o.selected_path.is_empty() {
                    o.write_editor_session(false, None, None, cx);
                    return;
                }
                o.restoring_session_views = true;
                o.restoring_session_views_token = token;
                if let Some(views) = &sess.file_views {
                    o.file_views = views.clone();
                    let ws = o.props.workspace_id.clone();
                    o.preview_scroll
                        .borrow_mut()
                        .hydrate(o.file_views.iter(), &ws);
                }
                if let Some(docs) = &sess.scratch_docs {
                    o.scratch_docs = docs.clone();
                }
                let mut paths: Vec<&str> = sess.open_files.iter().map(String::as_str).collect();
                if let Some(sp) = &sess.split_path {
                    paths.push(sp);
                }
                let keys: Vec<String> = o.scratch_docs.keys().cloned().collect();
                paths.extend(keys.iter().map(String::as_str));
                o.scratch_seq = sess.scratch_seq.unwrap_or(0).max(max_scratch_seq(paths));
                o.open_files = unique_open_files(&sess.open_files);
                o.stashed_buffers = sess.stashed_buffers.clone().unwrap_or_default();
                o.session_restored = true;
                let pick = if !sess.selected_path.is_empty()
                    && sess.open_files.contains(&sess.selected_path)
                {
                    Some(sess.selected_path.clone())
                } else {
                    sess.open_files.first().cloned()
                };
                let finish = move |o: &mut Observatory,
                                   window: &mut Window,
                                   cx: &mut Context<Observatory>| {
                    cx.on_next_frame(window, move |_, window, cx| {
                        cx.on_next_frame(window, move |o, _, _| {
                            if o.root_token == token && o.restoring_session_views_token == token {
                                o.restoring_session_views = false;
                            }
                        });
                    });
                    let _ = o;
                };
                match pick {
                    Some(p) => {
                        let t = o.open_path(OpenRequest::new(p), window, cx);
                        cx.spawn_in(window, async move |this, cx| {
                            t.await;
                            let _ = this.update_in(cx, |o, window, cx| finish(o, window, cx));
                        })
                        .detach();
                    }
                    None => finish(o, window, cx),
                }
                if let Some(sp) = sess.split_path.clone() {
                    let t =
                        o.open_in_split(&sp, 0, 0, ExplorerReveal::Reveal, false, 0, 0, window, cx);
                    cx.spawn_in(window, async move |this, cx| {
                        t.await;
                        let _ = this.update_in(cx, |o, window, cx| {
                            o.focused_pane = Pane::Main;
                            o.sync_editors(window, cx);
                            if let Some(e) = o.main_editor.clone() {
                                e.update(cx, |e, cx| e.focus(window, cx));
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- persisting the editor session ----

    pub(crate) fn pane_chrome_non_default(&self) -> bool {
        self.left_kind != CodePanelKey::Maestro
            || self.right_kind != CodePanelKey::Files
            || !self.show_side
            || !self.left_gutter_open()
    }

    fn panes_at_defaults(&self) -> bool {
        !self.pane_chrome_non_default()
    }

    /// `applySessionPaneChrome`: cold restore of dock kinds unless the user moved them.
    fn apply_session_pane_chrome(&mut self, sess: Option<&Session>, cx: &mut Context<Self>) {
        let Some(s) = sess else { return };
        if !self.panes_at_defaults() {
            return;
        }
        if let Some(k) = s
            .left_kind
            .as_deref()
            .and_then(|k| CodePanelKey::ALL.into_iter().find(|c| c.key() == k))
        {
            self.left_kind = k;
        }
        if let Some(k) = s
            .right_kind
            .as_deref()
            .and_then(|k| CodePanelKey::ALL.into_iter().find(|c| c.key() == k))
        {
            self.right_kind = k;
        }
        if let Some(open) = s.right_open {
            self.show_side = open;
        }
        if let Some(open) = s.left_open {
            self.set_left_gutter_open(open, cx);
        }
    }

    /// `writeEditorSession`.
    pub(crate) fn write_editor_session(
        &mut self,
        flush: bool,
        window: Option<&Window>,
        root: Option<(&str, &str)>,
        cx: &mut gpui_kit::App,
    ) {
        let (pid, wid) = match root {
            Some((p, w)) => (p.to_string(), w.to_string()),
            None => (
                self.props.project_id.clone(),
                self.props.workspace_id.clone(),
            ),
        };
        if pid.is_empty() && wid.is_empty() {
            return;
        }
        if !wid.is_empty() && wid != "_" && self.state.read(cx).workspace(&wid).is_none() {
            return;
        }
        let (syn_path, syn_type, syn_non_empty) = self.syntax_snapshot(cx);
        if self.open_files.is_empty()
            && !syn_non_empty
            && self.main_surface_pref != MainSurface::Review
            && !self.pane_chrome_non_default()
        {
            return;
        }
        let mut keep: Vec<String> = self.open_files.clone();
        if self.split_open && !self.split_path.is_empty() {
            keep.push(self.split_path.clone());
        }
        if !self.restoring_session_views
            && flush
            && let Some(w) = window
        {
            self.remember_leaving_editors(w, cx);
        }
        let pruned = prune_file_views(
            &self.file_views,
            keep.iter().map(String::as_str),
            recur_editor::view_state::MAX_STORED_VIEWS,
        );
        let views: std::collections::HashMap<_, _> = pruned.into_iter().collect();
        let views = self
            .preview_scroll
            .borrow()
            .apply_to_file_views(&views, keep.iter().map(String::as_str))
            .unwrap_or(views);
        let stashed: BTreeMap<String, StashedBuffer> =
            self.session_stashed_buffers(cx).into_iter().collect();
        let session = Session {
            open_files: self.open_files.clone(),
            selected_path: self.selected_path.clone(),
            split_path: (self.split_open && !self.split_path.is_empty())
                .then(|| self.split_path.clone()),
            scratch_docs: Some(prune_scratch_docs(
                &self.scratch_docs,
                keep.iter().map(String::as_str),
            )),
            scratch_seq: Some(self.scratch_seq),
            stashed_buffers: Some(stashed),
            file_views: Some(views.into_iter().collect()),
            syntax_by_path: Some(syn_path),
            syntax_by_file_type: Some(syn_type),
            main_surface_review: self.main_surface_pref == MainSurface::Review,
            left_kind: Some(self.left_kind.key().to_string()),
            right_kind: Some(self.right_kind.key().to_string()),
            left_open: Some(self.left_gutter_open()),
            right_open: Some(self.show_side),
            syntax_maps_known: self.syntax_maps_ready,
        };
        crate::session_host::save_editor_session(&wid, session, cx);
        if flush {
            crate::session_host::flush_editor_session_now(cx);
        }
    }

    /// The persist `$effect`: write (debounced) whenever tabs, buffers or chrome change.
    pub(crate) fn persist_editor_session(&mut self, cx: &mut Context<Self>) {
        if self.props.project_id.is_empty() && self.props.workspace_id.is_empty() {
            return;
        }
        if self.open_files.is_empty()
            && !self.pane_chrome_non_default()
            && self.main_surface_pref != MainSurface::Review
        {
            self.persist_or_clear_editor_session(cx);
            return;
        }
        self.write_editor_session(false, None, None, cx);
    }

    /// `persistOrClearEditorSession`.
    pub(crate) fn persist_or_clear_editor_session(&mut self, cx: &mut Context<Self>) {
        let (pid, wid) = (
            self.props.project_id.clone(),
            self.props.workspace_id.clone(),
        );
        self.persist_or_clear_for(&pid, &wid, None, cx);
    }

    fn persist_or_clear_for(
        &mut self,
        pid: &str,
        wid: &str,
        window: Option<&Window>,
        cx: &mut gpui_kit::App,
    ) {
        let (_, _, syn_non_empty) = self.syntax_snapshot(cx);
        if self.open_files.is_empty()
            && !syn_non_empty
            && self.main_surface_pref != MainSurface::Review
            && !self.pane_chrome_non_default()
        {
            if self.session_load_settled && self.syntax_maps_ready {
                crate::session_host::clear_editor_session(wid, cx);
            }
            return;
        }
        self.write_editor_session(true, window, Some((pid, wid)), cx);
    }

    /// `persistSessionForRoot`.
    pub(crate) fn persist_session_for_root(
        &mut self,
        root: &str,
        window: Option<&Window>,
        cx: &mut gpui_kit::App,
    ) {
        if root.is_empty() || root == "\0" {
            return;
        }
        let (pid, wid) = split_root(root);
        self.persist_or_clear_for(&pid, &wid, window, cx);
    }

    /// `pagehide` / `visibilitychange` hidden: write the session with the live editors' caret
    /// and scroll.
    pub fn page_hide(&mut self, window: &Window, cx: &mut gpui_kit::App) {
        let root = self.loaded_root.clone();
        self.persist_session_for_root(&root, Some(window), cx);
    }

    // ---- backend events ----

    fn on_app_event(&mut self, ev: AppEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            AppEvent::WorkspaceFsChanged(d) => {
                if self.props.workspace_id.is_empty() || d.workspace_id != self.props.workspace_id {
                    return;
                }
                if !self.props.shell_active {
                    return;
                }
                self.schedule_live_refresh(cx);
            }
            AppEvent::FindingsUpdated(r)
                if !self.props.workspace_id.is_empty()
                    && r.workspace_id == self.props.workspace_id =>
            {
                self.load_findings(cx).detach();
            }
            AppEvent::ArtifactCreated(e) => {
                if e.workspace_id.is_empty() {
                    return;
                }
                if e.workspace_id == self.props.workspace_id && self.artifacts_panel_focused(cx) {
                    return;
                }
                let raw = serde_json::to_value(&e.artifact).unwrap_or_default();
                let Some(a) = recur_engine::view::artifacts::artifact_from_event(&raw) else {
                    return;
                };
                self.bump_unseen_artifact(&e.workspace_id, &a.id, cx);
            }
            AppEvent::ArtifactPresented(e) => {
                if e.workspace_id.is_empty() {
                    return;
                }
                let raw = serde_json::to_value(&e.artifact).unwrap_or_default();
                let Some(a) = recur_engine::view::artifacts::artifact_from_event(&raw) else {
                    return;
                };
                if e.workspace_id != self.props.workspace_id || !self.props.shell_active {
                    self.bump_unseen_artifact(&e.workspace_id, &a.id, cx);
                    return;
                }
                self.select_code_panel(CodePanelKey::Artifacts, window, cx);
                self.open_artifact(&a.id, &a.title, &a.kind, &a.path, window, cx)
                    .detach();
            }
            _ => {}
        }
    }

    fn bump_unseen_artifact(&mut self, wid: &str, id: &str, cx: &mut Context<Self>) {
        let next = recur_engine::view::artifacts::add_unseen_artifact_id(
            &cx.default_global::<crate::UnseenArtifacts>().0,
            wid,
            id,
        );
        cx.default_global::<crate::UnseenArtifacts>().0 = next;
        cx.notify();
    }

    fn on_workspace_gone(&mut self, id: &str, cx: &mut Context<Self>) {
        let _ = id;
        self.prune_caches(cx);
    }

    /// Drop keep-alive entries and parked tabs of workspaces that no longer exist.
    fn prune_caches(&mut self, cx: &mut Context<Self>) {
        // Runs on every store change: fingerprint the ids before collecting them.
        let ids = || self.state.read(cx).workspaces.iter().map(|w| w.id.as_str());
        let mark = {
            use std::hash::{Hash, Hasher};
            let mut h = std::hash::DefaultHasher::new();
            for id in ids() {
                id.hash(&mut h);
            }
            h.finish()
        };
        if self.pruned_live == Some(mark) {
            return;
        }
        self.pruned_live = Some(mark);
        let live: HashSet<String> = ids().map(str::to_string).collect();
        crate::obs_cache::cache(cx).prune(&live);
        crate::session_tabs::parks(cx).prune_workspaces(&live);
    }

    fn retry_skipped_tree(&mut self, cx: &mut Context<Self>) {
        if self.props.workspace_id.is_empty()
            || self.worktree_path(cx).is_empty()
            || !self.skipped_tree_for_create
        {
            return;
        }
        self.skipped_tree_for_create = false;
        self.refresh_tree(false, cx).detach();
    }

    // ---- deep links ----

    /// `observatoryJump`: open a file / finding / panel requested elsewhere.
    fn handle_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(jump) = self.state.read(cx).observatory_jump.clone() else {
            return;
        };
        let (ws, pid) = (&self.props.workspace_id, &self.props.project_id);
        if !jump.workspace_id.is_empty() && !ws.is_empty() && jump.workspace_id != *ws {
            return;
        }
        if !jump.project_id.is_empty() && !pid.is_empty() && jump.project_id != *pid {
            return;
        }
        if !self.props.shell_active && jump.workspace_id.is_empty() {
            return;
        }
        let clear = |o: &mut Observatory, cx: &mut Context<Observatory>| {
            o.state.update(cx, |s, cx| {
                s.observatory_jump = None;
                cx.notify();
            });
        };
        match jump.panel.as_str() {
            "plan" => {
                clear(self, cx);
                if !jump.plan_tab.is_empty() {
                    cx.emit(crate::ObservatoryEvent::PlanTab(jump.plan_tab.clone()));
                }
                self.select_code_panel(CodePanelKey::Plan, window, cx);
                return;
            }
            "review" => {
                if self.props.main_blocked || !self.is_git(cx) {
                    return;
                }
                clear(self, cx);
                self.show_review_surface(cx);
                self.set_review_tab(recur_view_review::review_diff::ReviewTab::Changes, cx);
                return;
            }
            _ => {}
        }
        if !jump.finding_id.is_empty() || jump.panel == "findings" {
            let id = jump.finding_id.clone();
            let focus_note = jump.focus_note;
            clear(self, cx);
            self.select_code_panel(CodePanelKey::Findings, window, cx);
            self.findings_filter = FindingsFilter::Open;
            let load = self.load_findings(cx);
            cx.spawn_in(window, async move |this, cx| {
                let list = load.await;
                if id.is_empty() {
                    return;
                }
                let _ = this.update_in(cx, |o, window, cx| {
                    let note = focus_note.then(|| id.clone());
                    o.findings_panel
                        .update(cx, |p, cx| p.expand(vec![id.clone()], note, cx));
                    if let Some(f) = list
                        .iter()
                        .find(|f| recur_view_widgets::findings_ui::finding_matches_ref(f, &id))
                        && !f.file.is_empty()
                    {
                        o.open_path(
                            OpenRequest::new(f.file.clone())
                                .line(f.line.max(0) as usize)
                                .end_line(f.end_line.max(0) as usize),
                            window,
                            cx,
                        )
                        .detach();
                    }
                });
            })
            .detach();
            return;
        }
        if jump.path.is_empty() {
            return;
        }
        self.open_path(
            OpenRequest::new(jump.path.clone())
                .line(jump.line as usize)
                .prefer_diff(jump.prefer_diff)
                .end_line(jump.end_line as usize)
                .explorer(ExplorerReveal::IfVisible)
                .cols(jump.start_col as usize, jump.end_col as usize),
            window,
            cx,
        )
        .detach();
        clear(self, cx);
    }

    // ---- the worktree watcher, menu, command owner ----

    /// Arm the engine's watcher for the workspace on screen (re-armed when its path lands).
    fn arm_watcher(&mut self, cx: &mut Context<Self>) {
        if !self.props.shell_active || self.state.read(cx).scan_pending {
            return;
        }
        let id = self.props.workspace_id.clone();
        let path = self.worktree_path(cx);
        let key = if id.is_empty() {
            String::new()
        } else {
            format!("{id}:{path}")
        };
        if key == self.watch_armed_for {
            return;
        }
        self.watch_armed_for = key.clone();
        cx.default_global::<crate::Owners>().watch = key;
        let svc = self.services.clone();
        cx.background_spawn(async move {
            let _ = svc.workspaces.watch_workspace(&id).await;
        })
        .detach();
    }

    fn disarm_watcher(&mut self, cx: &mut Context<Self>) {
        if self.watch_armed_for.is_empty() {
            return;
        }
        if cx.default_global::<crate::Owners>().watch == self.watch_armed_for {
            cx.default_global::<crate::Owners>().watch.clear();
            let svc = self.services.clone();
            cx.background_spawn(async move {
                let _ = svc.workspaces.watch_workspace("").await;
            })
            .detach();
        }
        self.watch_armed_for.clear();
    }

    /// Push the contextual menubar enables (`setMenuState`).
    fn push_menu_state(&mut self, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        cx.default_global::<crate::Owners>().menu = id.clone();
        let zone = self.focus_zone;
        let in_term = zone == FocusZone::Terminal;
        let in_editor = zone == FocusZone::Editor;
        let in_agent = zone == FocusZone::Agent;
        let in_review = in_editor && self.main_surface(cx) == MainSurface::Review;
        let tab_count = if in_term {
            self.term_tab_count
        } else if in_review {
            if self.review_has_commits(cx) { 2 } else { 1 }
        } else if in_editor {
            self.open_files.len()
        } else if in_agent {
            self.props.agent_tab_count
        } else {
            0
        };
        let can_close = if in_term {
            self.term_tab_count > 0
        } else if in_review {
            false
        } else if in_editor {
            !self.selected_path.is_empty()
        } else {
            in_agent && self.props.agent_tab_count > 0 && self.props.agent_tabs
        };
        let blocked = self.props.main_blocked;
        let has_ws = !id.is_empty();
        let s = self.state.read(cx);
        let dev = has_ws
            && !blocked
            && !recur_view_core::run::workspace_dev_cmd(&id, &s.workspaces, &s.projects).is_empty();
        let running = has_ws && !blocked && s.run.is_any_run_running(&id);
        let has_maestro_left = self
            .panels_on(recur_view_core::prefs::PanelSide::Left, cx)
            .contains(&CodePanelKey::Maestro);
        let portal = self.portal_agent_to_pr(cx);
        let patch = recur_services::events::MenuState {
            new_tab: Some((in_term && has_ws) || (in_agent && self.props.agent_tabs)),
            save: Some(self.dirty()),
            close_tab: Some(can_close),
            toggle_left_panel: Some(self.has_left(cx) || portal),
            toggle_right_panel: Some(self.has_right(cx) && !blocked),
            toggle_editor: Some(has_ws && !blocked && !self.term_overlaying()),
            toggle_terminal: Some(has_ws && !blocked),
            maximize_terminal: Some(has_ws && self.term_open(cx) && !blocked),
            focus_agent: Some(has_maestro_left || portal),
            focus_editor: Some(!blocked),
            focus_terminal: Some(has_ws && !blocked),
            prev_tab: Some(tab_count > 1),
            next_tab: Some(tab_count > 1),
            run_dev: Some(dev),
            stop_run: Some(running),
            ..Default::default()
        };
        self.state.update(cx, |s, cx| s.set_menu_state(patch, cx));
    }

    /// The editor command / snippet runners and focus provider while this shell is live.
    fn register_command_handlers(&mut self, cx: &mut Context<Self>) {
        let id = self.props.workspace_id.clone();
        cx.default_global::<crate::Owners>().command = id;
        let weak = self.weak.clone();
        let handler: recur_view_core::commands::EditorCommandHandler =
            std::rc::Rc::new(move |cmd: String, cx: &mut gpui_kit::App| {
                let Some(o) = weak.upgrade() else {
                    return Task::ready(Err(
                        "No editor is focused — open a file in the Observatory first".into(),
                    ));
                };
                let Some(window) = o.read(cx).window else {
                    return Task::ready(Ok(()));
                };
                let t = window
                    .update(cx, |_, window, cx| {
                        o.update(cx, |o, cx| o.run_editor_command(&cmd, window, cx))
                    })
                    .ok();
                match t {
                    Some(t) => cx.background_spawn(async move {
                        t.await;
                        Ok(())
                    }),
                    None => Task::ready(Ok(())),
                }
            });
        let weak = self.weak.clone();
        let focus: recur_view_core::commands::EditorFocusProvider =
            std::rc::Rc::new(move |cx: &gpui_kit::App| {
                let Some(o) = weak.upgrade() else {
                    return (String::new(), String::new());
                };
                let o = o.read(cx);
                let path =
                    if o.focused_pane == Pane::Split && o.split_open && !o.split_path.is_empty() {
                        o.split_path.clone()
                    } else {
                        o.selected_path.clone()
                    };
                (o.scope_for(&path, cx), path)
            });
        self.state.update(cx, |s, _| {
            s.set_editor_command_handler(Some(handler));
            s.set_editor_focus_provider(Some(focus));
            s.set_session_flusher(Some(std::rc::Rc::new(
                crate::session_host::flush_editor_session_now,
            )));
        });
        let weak = self.weak.clone();
        recur_view_core::snippets::set_editor_snippet_handler(
            Some(std::rc::Rc::new(move |sn, window, cx| {
                if let Some(o) = weak.upgrade() {
                    o.update(cx, |o, cx| o.insert_bundle_snippet(&sn, window, cx));
                }
            })),
            cx,
        );
    }

    fn release_command_handlers(&mut self, id: &str, cx: &mut Context<Self>) {
        if cx.default_global::<crate::Owners>().command != id {
            return;
        }
        cx.default_global::<crate::Owners>().command.clear();
        self.state.update(cx, |s, _| {
            s.set_editor_command_handler(None);
            s.set_editor_focus_provider(None);
        });
        recur_view_core::snippets::set_editor_snippet_handler(None, cx);
    }

    // ---- activation (hot shells) ----

    fn activation_changed(
        &mut self,
        was_active: bool,
        prev_ws: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let on = self.props.shell_active;
        if !prev_ws.is_empty() && prev_ws != self.props.workspace_id {
            self.unregister_lsp_host(prev_ws, cx);
            self.release_command_handlers(prev_ws, cx);
        }
        if on {
            self.register_command_handlers(cx);
            self.register_lsp_host(cx);
        } else {
            let id = self.props.workspace_id.clone();
            self.release_command_handlers(&id, cx);
            self.unregister_lsp_host(&id, cx);
            self.disarm_watcher(cx);
            if cx.default_global::<crate::Owners>().occluder == id {
                cx.default_global::<crate::Owners>().occluder.clear();
                self.state
                    .update(cx, |s, cx| s.set_chat_occluder("agent-pane", false, cx));
            }
            if cx.default_global::<crate::Owners>().menu == id {
                cx.default_global::<crate::Owners>().menu.clear();
                self.state.update(cx, |s, cx| {
                    s.set_menu_state(recur_view_core::menu::menu_state_idle(), cx)
                });
            }
            self.set_editor_fullscreen(false, window, cx);
        }
        if on && !was_active {
            let zone = self.focus_zone;
            let token = self.root_token;
            self.sync_editors(window, cx);
            self.schedule_live_refresh(cx);
            cx.on_next_frame(window, move |o, window, cx| {
                cx.on_next_frame(window, move |o2, window, cx| {
                    let _ = o;
                    if o2.root_token == token {
                        o2.restore_chrome_focus(zone, token, window, cx);
                    }
                });
            });
        }
        if let Some((_, p)) = &self.file_preview {
            p.update(cx, |p, _| p.set_parked(!on));
        }
        self.was_shell_active = on;
    }

    /// `onDestroy`.
    fn on_destroy(&mut self, cx: &mut gpui_kit::App) {
        self.stop_live_refresh();
        let id = self.props.workspace_id.clone();
        if !self.watch_armed_for.is_empty()
            && cx.default_global::<crate::Owners>().watch == self.watch_armed_for
        {
            cx.default_global::<crate::Owners>().watch.clear();
            let svc = self.services.clone();
            cx.background_spawn(async move {
                let _ = svc.workspaces.watch_workspace("").await;
            })
            .detach();
        }
        if !id.is_empty() {
            crate::lsp::client::LspHub::global(cx).clear_workspace(&id, cx);
            let svc = self.services.clone();
            let wid = id.clone();
            cx.background_spawn(async move {
                let _ = svc.lsp.close_workspace_lsp(&wid).await;
            })
            .detach();
        }
        if cx.default_global::<crate::Owners>().menu == id {
            cx.default_global::<crate::Owners>().menu.clear();
            let state = self.state.clone();
            state.update(cx, |s, cx| {
                s.set_menu_state(recur_view_core::menu::menu_state_idle(), cx)
            });
        }
        if cx.default_global::<crate::Owners>().command == id {
            cx.default_global::<crate::Owners>().command.clear();
            self.state.update(cx, |s, _| {
                s.set_editor_command_handler(None);
                s.set_editor_focus_provider(None);
            });
            recur_view_core::snippets::set_editor_snippet_handler(None, cx);
        }
        if cx.default_global::<crate::Owners>().occluder == id {
            cx.default_global::<crate::Owners>().occluder.clear();
            self.state
                .update(cx, |s, cx| s.set_chat_occluder("agent-pane", false, cx));
        }
        let root = self.loaded_root.clone();
        if root.is_empty() || root == "\0" {
            return;
        }
        if !id.is_empty()
            && !self
                .state
                .read(cx)
                .workspaces
                .iter()
                .any(|w| w.id == id && !w.archived)
        {
            return;
        }
        let mut snap = None;
        if let Some(w) = self.window {
            let _ = w.update(cx, |_, window, cx| {
                snap = Some(self.capture_snapshot(Some(window), cx))
            });
        }
        let snap = match snap {
            Some(s) => s,
            None => self.capture_snapshot(None, cx),
        };
        crate::obs_cache::cache(cx).put(&root, snap);
        self.persist_session_for_root(&root, None, cx);
    }
}
