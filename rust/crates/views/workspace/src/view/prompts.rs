//! Prompt actions (`/commit`, `/review`, the Findings panel's Fix button): route to the right
//! tab (grouped, fresh or current), run the script, then send its prompt there; a prepared
//! send whose routing fails is held behind a Retry toast.

use futures::FutureExt as _;
use gpui_kit::{App, AsyncApp, Context, Task, WeakEntity, Window};
use recur_bundles::PromptAction;
use recur_engine::view::chat::AgentMsg;
use recur_engine::view::prompts::{
    PromptEditor, PromptTab, PromptTabPick, PromptTabQuery, PromptWorkspace, build_prompt_context,
    find_prompt_by_slash, params_from_args, pick_prompt_tab, prompt_action_error_message,
    prompt_enabled,
};
use recur_services::api::PromptActionInfo;
use recur_ui::widgets::{ToastKind, ToastOptions, push_toast, toast_action};
use recur_view_chat::heap::PendingChatSend;
use recur_view_chat::prompt_actions::{PromptCatalog, prompt_action, run_prompt_action};
use recur_view_core::models::chat_steer_follow_up;
use recur_view_core::modes::{ChatMode, chat_has_started, ws_run_active};
use recur_view_core::toast;

use super::send::Routed;
use super::{Abort, HeldPromptSend, WorkspaceView};
use crate::uuid::uuidv7;

const OPEN_PR_IDS: [&str; 2] = ["recur.chat.open-pr", "recur.chat.open-draft-pr"];
const LOOP_OPEN_PR: &str =
    "A loop is running — wait until it pauses, then open a PR from the titlebar or /open-pr.";
const MODELS_LOADING: &str = "Models are still loading — try again in a moment";

struct TabRef<'a>(&'a recur_domain::WorkspaceChatTab);

impl PromptTab for TabRef<'_> {
    fn code(&self) -> &str {
        &self.0.code
    }
    fn prompt_group(&self) -> &str {
        &self.0.prompt_group
    }
}

fn editor_ctx(e: recur_editor::view::EditorContext, clipboard: Option<String>) -> PromptEditor {
    PromptEditor {
        path: Some(e.path),
        selected_text: Some(e.selected_text),
        document: Some(e.document),
        current_line: Some(e.current_line),
        line_number: Some(e.line_number as i64),
        column: Some(e.column as i64),
        current_word: Some(e.current_word),
        scope: Some(e.scope),
        clipboard,
        tab_size: Some(e.tab_size as i64),
        soft_tabs: Some(e.soft_tabs),
    }
}

/// `this.update` that reads `false` when the view is gone.
fn upd<R: Default>(
    this: &WeakEntity<WorkspaceView>,
    cx: &mut AsyncApp,
    f: impl FnOnce(&mut WorkspaceView, &mut Context<WorkspaceView>) -> R,
) -> R {
    this.update(cx, f).unwrap_or_default()
}

impl WorkspaceView {
    fn tab_has_unsent_draft(&self, code: &str, cx: &App) -> bool {
        let key = self.key(code);
        let live = self.active_code() == code;
        let text = if live {
            self.chat_input(cx)
        } else {
            self.input_by_key.get(&key).cloned().unwrap_or_default()
        };
        if !text.trim().is_empty() {
            return true;
        }
        if live {
            !self.chat_attachments(cx).is_empty()
        } else {
            self.attachments_by_key.get(&key).is_some_and(|a| !a.is_empty())
        }
    }

    /// `pickIdleTab`.
    pub(crate) fn pick_idle_tab(&self, mode: ChatMode, group: &str, cx: &App) -> PromptTabPick {
        if self.ws(cx).is_none() {
            return PromptTabPick::Create;
        }
        let tabs: Vec<TabRef<'_>> = self.tabs().iter().map(TabRef).collect();
        let active = self.active_code();
        let busy = |code: &str| self.chat_key_busy(&self.key(code), cx);
        let tab_mode = |t: &TabRef<'_>| self.tab_mode_for(t.0).key().to_string();
        let started = |t: &TabRef<'_>| {
            let rows: Option<Vec<bool>> = self
                .chat_by_key
                .get(&self.key(&t.0.code))
                .map(|l| l.iter().map(|m| m.queued).collect());
            chat_has_started(Some(&t.0.title), rows.as_deref())
        };
        let drafted = |t: &TabRef<'_>| self.tab_has_unsent_draft(&t.0.code, cx);
        let locked = |code: &str| self.conductor_tab_locked(code, cx);
        pick_prompt_tab(&PromptTabQuery {
            tabs: &tabs,
            active_code: &active,
            mode: mode.key(),
            group,
            is_busy: &busy,
            tab_mode: &tab_mode,
            started: Some(&started),
            drafted: Some(&drafted),
            conductor_locked: Some(&locked),
        })
    }

    /// `ensureIdleTabForMode`: the current idle tab in the mode, else the newest idle one,
    /// else a new tab; hydrated before the mode flips. Grouped prompts stamp their group.
    pub(crate) fn ensure_idle_tab_for_mode(
        &mut self,
        mode: ChatMode,
        group: String,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        if self.ws(cx).is_none() {
            return Task::ready(false);
        }
        if mode == ChatMode::Loop && self.is_main(cx) {
            toast(ToastKind::Error, "The main working copy cannot run a loop", cx);
            return Task::ready(false);
        }
        if mode == ChatMode::Loop && self.is_pr_host(cx) {
            toast(ToastKind::Error, "PR chat cannot run a loop", cx);
            return Task::ready(false);
        }
        let id = self.ws_id.clone();
        self.chat_open = true;
        self.touch(cx);
        // Preflight before creating a tab so a hydrate failure leaves no stray empty chat.
        let ready = self.prompt_models_ready(cx);
        cx.spawn(async move |this, cx| {
            if !ready.await {
                return false;
            }
            let pick = upd(&this, cx, |v, cx| Some(v.pick_idle_tab(mode, &group, cx)));
            let Some(pick) = pick else { return false };
            let mut created = String::new();
            match pick {
                PromptTabPick::Select(code) => upd(&this, cx, |v, cx| v.select_chat_tab(&code, cx)),
                PromptTabPick::Create => {
                    let Ok(task) = this.update(cx, |v, cx| v.new_chat_tab(cx)) else {
                        return false;
                    };
                    match task.await {
                        Some(code) => created = code,
                        None => {
                            upd(&this, cx, |_, cx| {
                                toast(
                                    ToastKind::Error,
                                    format!("Could not open a chat tab for {} mode", mode.key()),
                                    cx,
                                )
                            });
                            return false;
                        }
                    }
                }
                PromptTabPick::Current => {}
            }
            let code = if created.is_empty() {
                upd(&this, cx, |v, _| v.active_code())
            } else {
                created.clone()
            };
            if code == "MA" {
                upd(&this, cx, |_, cx| {
                    toast(
                        ToastKind::Error,
                        format!("Open a chat tab for {} mode", mode.key()),
                        cx,
                    )
                });
                return false;
            }
            // Hydrate before flipping the mode: the coder fallback would otherwise stick.
            let Ok(hydrate) = this.update(cx, |v, cx| v.load_chat_pickers(&id, &code, cx)) else {
                return false;
            };
            let hydrated = hydrate.await;
            this.update(cx, |v, cx| {
                // Close only a tab this call opened, never the user's own.
                let abort_created = |v: &mut Self, cx: &mut Context<Self>| {
                    if !created.is_empty() {
                        v.close_chat_tab(&created, cx);
                    }
                };
                if v.active_code() != code {
                    abort_created(v, cx);
                    toast(ToastKind::Error, "Chat tab changed — try the command again", cx);
                    return false;
                }
                if !hydrated {
                    abort_created(v, cx);
                    toast(ToastKind::Error, MODELS_LOADING, cx);
                    return false;
                }
                let dock = v.on_chat_mode_change(mode, cx);
                if !group.is_empty() {
                    v.stamp_prompt_group(&code, &group, dock, cx);
                }
                v.active_code() == code
            })
            .unwrap_or(false)
        })
    }

    fn stamp_prompt_group(&mut self, code: &str, group: &str, after: Task<()>, cx: &mut Context<Self>) {
        if let Some(tabs) = self.tabs.as_mut() {
            for t in tabs.iter_mut().filter(|t| t.code == code) {
                t.prompt_group = group.to_string();
            }
        }
        self.put_strip(cx);
        self.touch(cx);
        let (svc, id, code, group) = (
            self.services.clone(),
            self.ws_id.clone(),
            code.to_string(),
            group.to_string(),
        );
        cx.background_executor().spawn(async move {
            after.await;
            let _ = svc
                .workspaces
                .remember_workspace_chat_prompt_group(&id, &code, &group)
                .await;
        })
        .detach();
    }

    fn ensure_current_coding_tab(&mut self, cx: &mut Context<Self>) -> bool {
        if self.ws(cx).is_none() {
            return false;
        }
        self.chat_open = true;
        self.touch(cx);
        if self.active_code() == "MA" {
            toast(ToastKind::Error, "Open a coding chat tab first", cx);
            return false;
        }
        true
    }

    fn discard_held_prompt(&mut self, hold_id: &str) {
        self.held_prompts.retain(|h| h.hold_id != hold_id);
    }

    fn hold_prepared_prompt(&mut self, held: HeldPromptSend, cx: &mut Context<Self>) {
        if !self.held_prompts.iter().any(|h| h.hold_id == held.hold_id) {
            self.held_prompts.push(held.clone());
        }
        let me = cx.entity().downgrade();
        let me2 = me.clone();
        let (retry_id, dismiss_id) = (held.hold_id.clone(), held.hold_id.clone());
        push_toast(
            ToastKind::Error,
            format!("{} is ready — open a chat tab and retry", held.display),
            ToastOptions {
                action: Some(toast_action("Retry", move |window, cx| {
                    let id = retry_id.clone();
                    let _ = me.update(cx, |v, cx| v.flush_held_prompt_send(&id, window, cx));
                })),
                on_dismiss: Some(std::rc::Rc::new(move |_, cx| {
                    let _ = me2.update(cx, |v, _| v.discard_held_prompt(&dismiss_id));
                })),
                replace: Some(format!("held-prompt-send:{}", held.hold_id).into()),
                ttl: Some(std::time::Duration::ZERO),
            },
            cx,
        );
    }

    /// `dispatchPreparedPrompt`: the routed tab takes the prompt (steered, queued or sent).
    fn dispatch_prepared_prompt(
        &mut self,
        routed: Routed,
        prompt: &str,
        display: &str,
        fresh_session: bool,
        alt: bool,
        cx: &mut Context<Self>,
    ) {
        let Routed {
            id,
            code,
            key,
            mode,
            model,
            flags,
        } = routed;
        self.prompt_target_key = key.clone();
        if self.ws_id == id && self.active_code() == code {
            let atts = self.chat_attachments(cx);
            self.set_chat_draft(String::new(), atts, cx);
        }
        self.input_by_key.insert(key.clone(), String::new());
        let pending = PendingChatSend {
            id: uuidv7(),
            marked: prompt.to_string(),
            plain: display.to_string(),
            mode,
            model,
            flags,
            display_text: display.to_string(),
            prompt_action: true,
            fresh_session,
            ..Default::default()
        };
        if self.active_key() == key {
            self.chat.update(cx, |c, cx| c.scroll_to_bottom(cx));
        }
        let setup_hold = self.setup_hold(cx);
        if self.chat_key_busy(&key, cx) || setup_hold {
            let tab_mode = self.pickers_by_key.get(&key).map(|p| p.mode).unwrap_or(mode);
            if !setup_hold
                && !fresh_session
                && self.steer_by_default(cx) != alt
                && chat_steer_follow_up(true, tab_mode.key(), false, &code)
            {
                self.push_msg(
                    &key,
                    AgentMsg {
                        me: true,
                        body: display.to_string(),
                        optimistic: true,
                        pending_id: pending.id.clone(),
                        ..Default::default()
                    },
                );
                self.dispatch_steer(&id, &code, &key, pending, cx);
                return;
            }
            self.push_msg(
                &key,
                AgentMsg {
                    me: true,
                    body: display.to_string(),
                    queued: true,
                    pending_id: pending.id.clone(),
                    ..Default::default()
                },
            );
            self.pending_by_key.entry(key).or_default().push(pending);
            self.touch(cx);
            return;
        }
        self.push_msg(
            &key,
            AgentMsg {
                me: true,
                body: display.to_string(),
                optimistic: true,
                ..Default::default()
            },
        );
        self.dispatch_send(&id, &code, &key, pending, cx);
    }

    fn snapshot_routed(&self, mode: Option<ChatMode>, cx: &App) -> Option<Routed> {
        let code = self.active_code();
        if code.is_empty() || code == "MA" {
            return None;
        }
        let key = self.key(&code);
        let (model, flags) = self.dock_model(cx);
        let p = self.pickers_by_key.get(&key);
        Some(Routed {
            id: self.ws_id.clone(),
            code,
            mode: mode.unwrap_or_else(|| self.chat_mode(cx)),
            model: p
                .map(|p| p.model.clone())
                .filter(|m| !m.is_empty())
                .unwrap_or(model),
            flags: p.map(|p| p.flags.clone()).unwrap_or(flags),
            key,
        })
    }

    /// Start a prompt run: lock the target tab before any await (the `/` query clears at once
    /// and the idle placeholder must not flash) and abort the previous one.
    fn begin_prompt(&mut self, cx: &mut Context<Self>) -> Abort {
        self.prompt_running = true;
        self.prompt_target_key = self.active_key();
        if let Some(a) = self.prompt_abort.take() {
            a.abort();
        }
        let abort = Abort::default();
        self.prompt_abort = Some(abort.clone());
        self.touch(cx);
        abort
    }

    /// The `finally`: unlock, then flush follow-ups that queued only behind the prompt.
    fn end_prompt(&mut self, cx: &mut Context<Self>) {
        let key = std::mem::take(&mut self.prompt_target_key);
        self.prompt_running = false;
        self.prompt_abort = None;
        if !key.is_empty() {
            self.try_flush_pending(&key, cx);
        }
        self.touch(cx);
    }

    /// Route to a mode tab or the current coding tab, then pin a loop prompt's loop.
    async fn route(
        this: &WeakEntity<Self>,
        mode: Option<ChatMode>,
        group: String,
        loop_id: String,
        abort: &Abort,
        cx: &mut AsyncApp,
    ) -> bool {
        match mode {
            Some(m) => {
                let Ok(t) = this.update(cx, |v, cx| v.ensure_idle_tab_for_mode(m, group, cx)) else {
                    return false;
                };
                if !t.await {
                    return false;
                }
            }
            None => {
                if !upd(this, cx, |v, cx| v.ensure_current_coding_tab(cx)) {
                    return false;
                }
            }
        }
        if abort.aborted() {
            return false;
        }
        let ok = upd(this, cx, |v, _| {
            v.prompt_target_key = v.active_key();
            v.active_code() == "MA" || v.pickers_ready()
        });
        if !ok {
            return false;
        }
        if mode == Some(ChatMode::Loop) && !loop_id.is_empty() {
            let pin = this.update(cx, |v, cx| {
                if v.loop_pickers_pinned(cx) {
                    toast(ToastKind::Error, "This tab is already running a loop", cx);
                    return None;
                }
                let policy = v.dock_policy_id(cx);
                Some(v.persist_loop_policy(&loop_id, &policy, cx))
            });
            let Ok(Some(pin)) = pin else { return false };
            if !pin.await || abort.aborted() {
                return false;
            }
        }
        true
    }

    /// `flushHeldPromptSend`: the held toast's Retry.
    pub(crate) fn flush_held_prompt_send(&mut self, hold_id: &str, _: &mut Window, cx: &mut Context<Self>) {
        let Some(held) = self.held_prompts.iter().find(|h| h.hold_id == hold_id).cloned() else {
            return;
        };
        if self.ws(cx).is_none() || held.workspace_id != self.ws_id || self.prompt_running {
            self.hold_prepared_prompt(held, cx);
            return;
        }
        let settings = self.state.read(cx).settings.clone();
        if settings
            .as_ref()
            .is_some_and(|s| !prompt_enabled(s, &held.action_id))
        {
            toast(
                ToastKind::Error,
                format!("{} is disabled in Settings → Bundles", held.display),
                cx,
            );
            self.hold_prepared_prompt(held, cx);
            return;
        }
        if ws_run_active(self.ws(cx)) && OPEN_PR_IDS.contains(&held.action_id.as_str()) {
            toast(ToastKind::Error, LOOP_OPEN_PR, cx);
            self.hold_prepared_prompt(held, cx);
            return;
        }
        let abort = self.begin_prompt(cx);
        cx.spawn(async move |this, cx| {
            let routed = Self::route(&this, held.mode, held.tab_group.clone(), held.loop_id.clone(), &abort, cx).await;
            upd(&this, cx, |v, cx| {
                let routed = routed
                    .then(|| v.snapshot_routed(held.mode, cx))
                    .flatten()
                    .filter(|_| !abort.aborted());
                match routed {
                    Some(r) => {
                        v.discard_held_prompt(&held.hold_id);
                        v.dispatch_prepared_prompt(r, &held.prompt, &held.display, held.fresh_session, false, cx);
                    }
                    None => v.hold_prepared_prompt(held, cx),
                }
                v.end_prompt(cx);
            });
        })
        .detach();
    }

    /// `invokePromptAction(slash, args, alt)`.
    pub(crate) fn invoke_prompt_action(
        &mut self,
        slash: &str,
        args: &str,
        alt: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ws(cx).is_none() || self.prompt_running {
            return;
        }
        let abort = self.begin_prompt(cx);
        let epoch = self.state.read(cx).bundles_epoch;
        let load = PromptCatalog::load(&self.services.clone(), epoch, cx);
        let (slash, args) = (slash.to_string(), args.to_string());
        cx.spawn(async move |this, cx| {
            let list = load.await;
            if !abort.aborted() {
                if let Err(msg) = Self::run_invoke(&this, &list, &slash, &args, alt, &abort, cx).await
                    && msg != "prompt cancelled"
                {
                    upd(&this, cx, |_, cx| toast(ToastKind::Error, msg, cx));
                }
            }
            upd(&this, cx, |v, cx| v.end_prompt(cx));
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_invoke(
        this: &WeakEntity<Self>,
        list: &[PromptActionInfo],
        slash: &str,
        args: &str,
        alt: bool,
        abort: &Abort,
        cx: &mut AsyncApp,
    ) -> Result<(), String> {
        let actions: Vec<PromptAction> = list.iter().map(prompt_action).collect();
        let Some(found) = find_prompt_by_slash(slash, &actions).cloned() else {
            upd(this, cx, |_, cx| {
                toast(
                    ToastKind::Error,
                    format!("Unknown prompt /{}", slash.trim_start_matches('/')),
                    cx,
                )
            });
            return Ok(());
        };
        let Some(info) = list.iter().find(|i| i.id == found.id).cloned() else {
            return Ok(());
        };
        let blocked = upd(this, cx, |v, cx| {
            if ws_run_active(v.ws(cx)) && OPEN_PR_IDS.contains(&info.id.as_str()) {
                toast(ToastKind::Error, LOOP_OPEN_PR, cx);
                return true;
            }
            if v
                .state
                .read(cx)
                .settings
                .as_ref()
                .is_some_and(|s| !prompt_enabled(s, &info.id))
            {
                toast(
                    ToastKind::Error,
                    format!("{} is disabled in Settings → Bundles", info.name),
                    cx,
                );
                return true;
            }
            false
        });
        if blocked {
            return Ok(());
        }
        let mode = (!info.mode.is_empty()).then(|| ChatMode::from_key(&info.mode));
        let group = info.tab_group.trim().to_string();
        let loop_id = info.r#loop.trim().to_string();
        // Review prompts stamp and read the target tab; other fresh-session prompts route only
        // once the script will send (an empty /open-pr must not spawn a tab).
        let route_first = !info.fresh_session || mode == Some(ChatMode::Review);
        let mut routed: Option<Routed> = None;
        if route_first {
            if !Self::route(this, mode, group.clone(), loop_id.clone(), abort, cx).await {
                return Ok(());
            }
            routed = upd(this, cx, |v, cx| Some(v.snapshot_routed(mode, cx))).flatten();
            if routed.is_none() {
                return Ok(());
            }
        } else {
            let pre = upd(this, cx, |v, cx| {
                if mode == Some(ChatMode::Loop) && v.is_main(cx) {
                    toast(ToastKind::Error, "The main working copy cannot run a loop", cx);
                    return None;
                }
                if mode == Some(ChatMode::Loop) && v.is_pr_host(cx) {
                    toast(ToastKind::Error, "PR chat cannot run a loop", cx);
                    return None;
                }
                Some(v.prompt_models_ready(cx))
            });
            let Some(ready) = pre else { return Ok(()) };
            if !ready.await {
                return Ok(());
            }
            match mode {
                None => {
                    if !upd(this, cx, |v, cx| v.ensure_current_coding_tab(cx)) {
                        return Ok(());
                    }
                }
                Some(m) => {
                    let hydrate = upd(this, cx, |v, cx| {
                        let code = match v.pick_idle_tab(m, &group, cx) {
                            PromptTabPick::Select(c) => c,
                            PromptTabPick::Current => v.active_code(),
                            PromptTabPick::Create => String::new(),
                        };
                        (!code.is_empty() && code != "MA").then(|| {
                            let id = v.ws_id.clone();
                            v.load_chat_pickers(&id, &code, cx)
                        })
                    });
                    if let Some(h) = hydrate
                        && !h.await
                    {
                        upd(this, cx, |_, cx| toast(ToastKind::Error, MODELS_LOADING, cx));
                        return Ok(());
                    }
                }
            }
        }

        let ctx = upd(this, cx, |v, cx| {
            let ws = v.ws(cx).cloned()?;
            let clipboard = cx.read_from_clipboard().and_then(|c| c.text());
            let editor = v
                .observatory
                .read(cx)
                .focused_editor_context(cx)
                .map(|e| editor_ctx(e, clipboard));
            let surface = v.chat_surface(cx).surface;
            let ctx = build_prompt_context(
                &PromptWorkspace {
                    id: ws.id.clone(),
                    project_id: ws.project_id.clone(),
                    worktree_path: ws.worktree_path.clone(),
                    port: ws.port,
                },
                editor.as_ref(),
                &surface,
                args,
                params_from_args(Some(&found), args),
            );
            let chat_code = routed.as_ref().map(|r| r.code.clone()).unwrap_or_else(|| v.active_code());
            Some((ctx, chat_code))
        });
        let Some((ctx, chat_code)) = ctx else { return Ok(()) };
        if abort.aborted() {
            return Ok(());
        }
        let run = upd(this, cx, |v, cx| {
            Some(run_prompt_action(&info, ctx, &chat_code, &v.services.clone(), cx))
        });
        let Some(run) = run else { return Ok(()) };
        let mut signal = abort.signal();
        let result: Result<recur_services::api::PromptResultDto, String> = futures::select_biased! {
            _ = signal => return Err("prompt cancelled".into()),
            r = run.fuse() => r,
        };
        if abort.aborted() {
            return Ok(());
        }
        let result = result.map_err(|e| prompt_action_error_message(&e))?;
        if let Some(issue) = result.issue.clone() {
            upd(this, cx, |v, cx| {
                let id = v.ws_id.clone();
                v.state
                    .update(cx, |s, cx| s.link_workspace_issue(&id, issue, cx));
            });
        }
        let display = if result.display.is_empty() {
            info.name.clone()
        } else {
            result.display.clone()
        };
        if result.prompt.trim().is_empty() {
            upd(this, cx, |_, cx| toast(ToastKind::Success, display, cx));
            return Ok(());
        }
        let prepared = HeldPromptSend {
            hold_id: uuidv7(),
            workspace_id: upd(this, cx, |v, _| v.ws_id.clone()),
            action_id: info.id.clone(),
            prompt: result.prompt.clone(),
            display,
            mode,
            tab_group: group.clone(),
            loop_id: loop_id.clone(),
            fresh_session: result.fresh_session.unwrap_or(info.fresh_session),
        };
        if !route_first {
            if !Self::route(this, mode, group, loop_id, abort, cx).await {
                upd(this, cx, |v, cx| v.hold_prepared_prompt(prepared, cx));
                return Ok(());
            }
            routed = upd(this, cx, |v, cx| Some(v.snapshot_routed(mode, cx))).flatten();
        }
        upd(this, cx, |v, cx| match routed {
            Some(r) => v.dispatch_prepared_prompt(
                r,
                &prepared.prompt,
                &prepared.display,
                prepared.fresh_session,
                alt,
                cx,
            ),
            None => v.hold_prepared_prompt(prepared, cx),
        });
        Ok(())
    }

    /// The Findings panel's Fix button: `/fix-findings` through the prompt path.
    pub(crate) fn fix_findings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending(cx) || self.prompt_running || self.ws(cx).is_none() {
            return;
        }
        self.invoke_prompt_action("fix-findings", "", false, window, cx);
    }

    /// `promptActionRequest`: a slash requested from elsewhere (the titlebar's land chrome).
    pub(crate) fn take_prompt_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt_running {
            return;
        }
        let id = self.ws_id.clone();
        let pending = self
            .state
            .read(cx)
            .prompt_action
            .as_ref()
            .is_some_and(|r| r.workspace_id == id);
        if !pending {
            return;
        }
        let taken = self.state.update(cx, |s, _| s.take_prompt_action(&id));
        if let Some((slash, args)) = taken {
            self.invoke_prompt_action(&slash, &args, false, window, cx);
        }
    }
}
