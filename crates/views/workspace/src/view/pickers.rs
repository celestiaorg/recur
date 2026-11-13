//! The dock's model, flags, mode and Simplify per chat tab (`loadChatPickers`,
//! `onChatModeChange`, `toggleSimplify`), the loop / policy pickers (`persistLoopPolicy`) and
//! the catalogs they read.

use futures::FutureExt as _;
use gpui_kit::{App, Context, Task};
use recur_engine::view::loops::{LoopInfo, PolicyInfo, fallback_loops, fallback_policies};
use recur_ui::widgets::ToastKind;
use recur_view_chat::heap::ChatPickerSnap;
use recur_view_core::models::{
    FlagMap, flags_equal, flags_for_model, model_provider, normalize_flags, provider_enabled,
};
use recur_view_core::modes::{
    ChatMode, chat_has_started, picker_after_mode_change, resolved_default_for_mode, ws_run_active,
};
use recur_view_core::toast;

use super::WorkspaceView;

/// The loop and policy lists the last shell loaded: a shell built later shows them in its
/// first frame instead of the fallbacks.
pub(crate) struct LoopCatalog(pub Vec<LoopInfo>, pub Vec<PolicyInfo>);

impl gpui_kit::Global for LoopCatalog {}

impl LoopCatalog {
    pub fn get(cx: &App) -> (Vec<LoopInfo>, Vec<PolicyInfo>) {
        cx.try_global::<LoopCatalog>().map_or_else(
            || (fallback_loops(), fallback_policies()),
            |c| (c.0.clone(), c.1.clone()),
        )
    }
}

impl WorkspaceView {
    fn simplify_setting(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .settings
            .as_ref()
            .is_some_and(|s| s.simplify_enabled())
    }

    /// `simplifyOn`.
    pub(crate) fn simplify_on(&self, cx: &App) -> bool {
        self.simplify_by_key
            .get(&self.active_key())
            .copied()
            .unwrap_or_else(|| self.simplify_setting(cx))
    }

    fn apply_simplify_sel(&mut self, key: &str, sel: Option<bool>, gen_: u64) {
        if self.simplify_gen.get(key).copied().unwrap_or(0) != gen_ {
            return;
        }
        self.simplify_fetched.insert(key.to_string());
        if let Some(on) = sel {
            self.simplify_by_key.insert(key.to_string(), on);
        }
    }

    pub(crate) fn load_simplify(&mut self, id: &str, code: &str, cx: &mut Context<Self>) {
        if id.is_empty() || code.is_empty() || self.state.read(cx).settings.is_none() {
            return;
        }
        let key = self.key(code);
        if self.simplify_fetched.contains(&key) {
            return;
        }
        let gen_ = self.simplify_gen.get(&key).copied().unwrap_or(0);
        let (svc, id, code) = (self.services.clone(), id.to_string(), code.to_string());
        cx.spawn(async move |this, cx| {
            // A failure stays unfetched so a later tab switch can retry.
            let Ok(sel) = svc.workspaces.workspace_chat_selection(&id, &code).await else {
                return;
            };
            let _ = this.update(cx, |v, cx| {
                v.apply_simplify_sel(&key, sel.simplify, gen_);
                v.touch(cx);
            });
        })
        .detach();
    }

    /// The Simplify value a send uses, frozen into the tab.
    pub(crate) fn freeze_simplify(&mut self, key: &str, cx: &App) -> bool {
        let on = self
            .simplify_by_key
            .get(key)
            .copied()
            .unwrap_or_else(|| self.simplify_setting(cx));
        self.simplify_by_key.insert(key.to_string(), on);
        self.simplify_fetched.insert(key.to_string());
        on
    }

    pub(crate) fn toggle_simplify(&mut self, cx: &mut Context<Self>) {
        let code = self.active_code();
        if code.is_empty() {
            return;
        }
        let key = self.active_key();
        let next = !self.simplify_on(cx);
        *self.simplify_gen.entry(key.clone()).or_default() += 1;
        self.simplify_by_key.insert(key.clone(), next);
        self.simplify_fetched.insert(key);
        let id = self.ws_id.clone();
        self.persist_chat_dock(&id, &code, Some(next), None, cx)
            .detach();
        self.touch(cx);
    }

    fn bump_picker_gen(&mut self, key: &str) {
        *self.picker_gen.entry(key.to_string()).or_default() += 1;
    }

    fn apply_chat_pickers(&mut self, key: &str, next: ChatPickerSnap) {
        if let Some(cur) = self.pickers_by_key.get(key)
            && cur.model == next.model
            && cur.mode == next.mode
            && flags_equal(&cur.flags, &next.flags)
        {
            return;
        }
        self.pickers_by_key.insert(key.to_string(), next);
    }

    /// `loadChatPickers`: the tab's session selection, else its mode's Settings default.
    /// Shares in-flight work per tab; a cache hit answers at once and still refreshes.
    pub(crate) fn load_chat_pickers(
        &mut self,
        id: &str,
        code: &str,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        if id.is_empty()
            || code.is_empty()
            || code == "MA"
            || self.state.read(cx).settings.is_none()
        {
            return Task::ready(false);
        }
        let key = self.key(code);
        let have = self
            .pickers_by_key
            .get(&key)
            .is_some_and(|p| !p.model.is_empty());
        if have && self.pickers_fetched.contains(&key) {
            return Task::ready(true);
        }
        if let Some(inflight) = self.pickers_loading.get(&key) {
            if have {
                return Task::ready(true);
            }
            let f = inflight.clone();
            return cx.foreground_executor().spawn(f);
        }
        let gen_ = self.picker_gen.get(&key).copied().unwrap_or(0);
        let (svc, id, code, k) = (
            self.services.clone(),
            id.to_string(),
            code.to_string(),
            key.clone(),
        );
        let run = cx
            .spawn(async move |this, cx| {
                // Wait for the catalog so flag dims are not dropped as `{}`.
                let need_catalog = this
                    .update(cx, |v, _| v.catalog_models.is_empty())
                    .unwrap_or(true);
                if need_catalog {
                    let load = this
                        .update(cx, |v, cx| {
                            v.state.update(cx, |s, cx| s.load_model_catalog(cx))
                        })
                        .ok();
                    if let Some(load) = load {
                        let models = load.await;
                        let _ = this.update(cx, |v, _| {
                            if !models.is_empty() {
                                v.catalog_models = models;
                            }
                        });
                    }
                }
                let ready = this
                    .update(cx, |v, cx| {
                        !v.catalog_models.is_empty() && v.state.read(cx).settings.is_some()
                    })
                    .unwrap_or(false);
                if !ready {
                    return this
                        .update(cx, |v, _| v.finish_picker_load(&k, gen_))
                        .unwrap_or(false);
                }
                let sel = svc.workspaces.workspace_chat_selection(&id, &code).await;
                this.update(cx, |v, cx| {
                    let tab_mode = ChatMode::from_key(
                        &v.tabs()
                            .iter()
                            .find(|t| t.code == code)
                            .map(|t| t.mode.clone())
                            .unwrap_or_default(),
                    );
                    let settings = v.state.read(cx).settings.clone();
                    let write = |v: &mut Self, next: ChatPickerSnap| {
                        if v.picker_gen.get(&k).copied().unwrap_or(0) == gen_ {
                            v.apply_chat_pickers(&k, next);
                        }
                    };
                    match sel {
                        Ok(sel) => {
                            if !v.simplify_fetched.contains(&k) {
                                let sg = v.simplify_gen.get(&k).copied().unwrap_or(0);
                                v.apply_simplify_sel(&k, sel.simplify, sg);
                            }
                            // A live mode from chat.mode_changed wins over a lagging selection.
                            let mode =
                                v.pickers_by_key.get(&k).map(|p| p.mode).unwrap_or_else(|| {
                                    if sel.mode.is_empty() {
                                        tab_mode
                                    } else {
                                        ChatMode::from_key(&sel.mode)
                                    }
                                });
                            let d = resolved_default_for_mode(
                                mode.key(),
                                settings.as_ref(),
                                &v.catalog_models,
                            );
                            let disabled = settings
                                .as_ref()
                                .map(|s| s.disabled_providers.clone())
                                .unwrap_or_default();
                            let stored_ok = !sel.model.is_empty()
                                && provider_enabled(&disabled, model_provider(&sel.model));
                            let model = if stored_ok {
                                sel.model.clone()
                            } else {
                                d.model
                            };
                            let cur: FlagMap = if stored_ok {
                                sel.flags.clone()
                            } else {
                                d.flags
                            };
                            let flags = normalize_flags(
                                &flags_for_model(&v.catalog_models, &model),
                                Some(&cur),
                            );
                            write(v, ChatPickerSnap { model, flags, mode });
                        }
                        Err(_) => {
                            if v.pickers_by_key.get(&k).is_none_or(|p| p.model.is_empty()) {
                                let mode =
                                    v.pickers_by_key.get(&k).map(|p| p.mode).unwrap_or(tab_mode);
                                let d = resolved_default_for_mode(
                                    mode.key(),
                                    settings.as_ref(),
                                    &v.catalog_models,
                                );
                                write(
                                    v,
                                    ChatPickerSnap {
                                        model: d.model,
                                        flags: d.flags,
                                        mode,
                                    },
                                );
                            }
                        }
                    }
                    v.touch(cx);
                    v.finish_picker_load(&k, gen_)
                })
                .unwrap_or(false)
            })
            .shared();
        self.pickers_loading.insert(key, run.clone());
        if have {
            cx.foreground_executor().spawn(run).detach();
            return Task::ready(true);
        }
        cx.foreground_executor().spawn(run)
    }

    fn finish_picker_load(&mut self, key: &str, gen_: u64) -> bool {
        let have = self
            .pickers_by_key
            .get(key)
            .is_some_and(|p| !p.model.is_empty());
        if self.picker_gen.get(key).copied().unwrap_or(0) == gen_ && have {
            self.pickers_fetched.insert(key.to_string());
        }
        self.pickers_loading.remove(key);
        have
    }

    pub(crate) fn on_chat_model_change(
        &mut self,
        model: String,
        flags: FlagMap,
        cx: &mut Context<Self>,
    ) {
        let key = self.active_key();
        self.bump_picker_gen(&key);
        let mode = self.chat_mode(cx);
        self.pickers_by_key
            .insert(key, ChatPickerSnap { model, flags, mode });
        self.touch(cx);
    }

    pub(crate) fn on_chat_flags_change(&mut self, flags: FlagMap, cx: &mut Context<Self>) {
        let key = self.active_key();
        self.bump_picker_gen(&key);
        let (model, _) = self.dock_model(cx);
        let mode = self.chat_mode(cx);
        self.pickers_by_key
            .insert(key, ChatPickerSnap { model, flags, mode });
        self.touch(cx);
    }

    /// `onChatModeChange`: guards (main, PR host, a running conductor), the picker rule after a
    /// switch, the strip icon, and the persisted selection.
    pub(crate) fn on_chat_mode_change(
        &mut self,
        next: ChatMode,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if self.is_main(cx) && next == ChatMode::Loop {
            return Task::ready(());
        }
        if self.is_pr_host(cx) && matches!(next, ChatMode::Loop | ChatMode::Agent) {
            return Task::ready(());
        }
        let code = self.active_code();
        let owner = self.conductor_chat_code(cx);
        if next != ChatMode::Loop && code == owner && self.has_run(cx) && ws_run_active(self.ws(cx))
        {
            toast(
                ToastKind::Error,
                "This tab is running the workflow: pause or stop it before leaving Autopilot mode.",
                cx,
            );
            return Task::ready(());
        }
        let key = self.active_key();
        let hydrated = self
            .pickers_by_key
            .get(&key)
            .is_some_and(|p| !p.model.is_empty());
        let title = self.active_tab().map(|t| t.title.clone());
        let queued: Option<Vec<bool>> = self
            .chat_by_key
            .get(&key)
            .map(|l| l.iter().map(|m| m.queued).collect());
        let started = chat_has_started(title.as_deref(), queued.as_deref());
        let (model, flags) = self.dock_model(cx);
        let cur = self.chat_mode(cx);
        let settings = self.state.read(cx).settings.clone();
        let pick = picker_after_mode_change(
            cur.key(),
            next.key(),
            &model,
            &flags,
            settings.as_ref(),
            &self.catalog_models,
            hydrated,
            started,
        );
        self.bump_picker_gen(&key);
        self.pickers_by_key.insert(
            key,
            ChatPickerSnap {
                model: pick.model.clone(),
                flags: pick.flags.clone(),
                mode: next,
            },
        );
        if let Some(tabs) = self.tabs.as_mut() {
            for t in tabs.iter_mut().filter(|t| t.code == code) {
                t.mode = next.key().to_string();
            }
        }
        self.put_strip(cx);
        self.touch(cx);
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.background_executor().spawn(async move {
            let _ = svc
                .workspaces
                .remember_workspace_chat_selection(
                    &id,
                    &code,
                    &pick.model,
                    &pick.flags,
                    next.key(),
                    None,
                )
                .await;
        })
    }

    /// ⌘. in the composer: advance the mode (not on MA, not while busy or restoring).
    pub(crate) fn cycle_chat_mode_from_composer(&mut self, cx: &mut Context<Self>) {
        if self.composer_busy(cx) || self.active_code() == "MA" || !self.pickers_ready() {
            return;
        }
        let (main, pr) = (self.is_main(cx), self.is_pr_host(cx));
        let next = self.chat_mode(cx).cycle(main || pr, pr);
        self.on_chat_mode_change(next, cx).detach();
    }

    /// `persistLoopPolicy`: the tab's loop + policy, and the workspace's before a run.
    pub(crate) fn persist_loop_policy(
        &mut self,
        next_loop: &str,
        next_policy: &str,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let code = self.active_code();
        if self.ws(cx).is_none() || code.is_empty() || self.loop_pickers_pinned(cx) {
            return Task::ready(false);
        }
        if let Some(tabs) = self.tabs.as_mut() {
            for t in tabs.iter_mut().filter(|t| t.code == code) {
                t.loop_id = next_loop.to_string();
                t.policy_id = next_policy.to_string();
            }
        }
        self.put_strip(cx);
        self.touch(cx);
        let has_run = self.has_run(cx);
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        let (l, p) = (next_loop.to_string(), next_policy.to_string());
        cx.spawn(async move |this, cx| {
            let res = async {
                svc.workspaces
                    .remember_chat_loop_policy(&id, &code, &l, &p)
                    .await?;
                if !has_run {
                    let ws = svc
                        .workspaces
                        .set_workspace_loop_policy(&id, &l, &p)
                        .await?;
                    return Ok(Some(ws));
                }
                Ok::<_, recur_services::Error>(None)
            }
            .await;
            this.update(cx, |v, cx| match res {
                Ok(ws) => {
                    if let Some(ws) = ws {
                        v.state.update(cx, |s, cx| s.upsert_workspace(ws, cx));
                    }
                    true
                }
                Err(err) => {
                    toast(ToastKind::Error, err.to_string(), cx);
                    false
                }
            })
            .unwrap_or(false)
        })
    }

    pub(crate) fn load_loops(&mut self, cx: &mut Context<Self>) {
        let svc = self.services.clone();
        cx.spawn(async move |this, cx| {
            let loops = svc.bundle.list_loops().await.unwrap_or_default();
            let policies = svc.bundle.list_policies().await.unwrap_or_default();
            let _ = this.update(cx, |v, cx| {
                if !loops.is_empty() {
                    v.catalog_loops = loops
                        .into_iter()
                        .map(|l| LoopInfo {
                            id: l.id,
                            name: l.name,
                            hint: l.hint,
                        })
                        .collect();
                }
                if !policies.is_empty() {
                    v.catalog_policies = policies
                        .into_iter()
                        .map(|p| PolicyInfo {
                            id: p.id,
                            name: p.name,
                            hint: p.hint,
                        })
                        .collect();
                }
                cx.set_global(LoopCatalog(
                    v.catalog_loops.clone(),
                    v.catalog_policies.clone(),
                ));
                v.touch(cx);
            });
        })
        .detach();
    }

    /// The model catalog, reloaded when `modelCatalogRev` bumps.
    pub(crate) fn sync_catalog(&mut self, cx: &mut Context<Self>) {
        let rev = self.state.read(cx).model_catalog_rev();
        if self.catalog_rev == Some(rev) {
            return;
        }
        self.catalog_rev = Some(rev);
        let load = self.state.update(cx, |s, cx| s.load_model_catalog(cx));
        cx.spawn(async move |this, cx| {
            let models = load.await;
            let _ = this.update(cx, |v, cx| {
                if v.catalog_rev == Some(rev) {
                    v.catalog_models = models;
                    v.touch(cx);
                }
            });
        })
        .detach();
    }

    /// Whether `promptModelsReady`'s preconditions hold, loading the catalog when it is empty.
    pub(crate) fn prompt_models_ready(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        if self.state.read(cx).settings.is_none() {
            toast(
                ToastKind::Error,
                "Models are still loading: try again in a moment",
                cx,
            );
            return Task::ready(false);
        }
        if !self.catalog_models.is_empty() {
            return Task::ready(true);
        }
        let load = self.state.update(cx, |s, cx| s.load_model_catalog(cx));
        cx.spawn(async move |this, cx| {
            let models = load.await;
            this.update(cx, |v, cx| {
                if !models.is_empty() {
                    v.catalog_models = models;
                }
                if v.catalog_models.is_empty() {
                    toast(
                        ToastKind::Error,
                        "Models are still loading: try again in a moment",
                        cx,
                    );
                    return false;
                }
                true
            })
            .unwrap_or(false)
        })
    }
}
