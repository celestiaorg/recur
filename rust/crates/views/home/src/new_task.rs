//! `NewTask.svelte`: the New workspace modal (`newTaskOpen`). One workspace noun; the mode
//! decides what starts inside it: Loop engages the autopilot with the task as its goal, every
//! other mode opens the workspace with its first chat tab in that mode (the opening prompt is
//! optional). A non-loop workspace is created eagerly as soon as the repository and base are
//! known, so Create closes the modal at once.

use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, px,
};
use recur_bundles::PromptAction;
use recur_domain::{BranchChoice, IssueRef, Settings, Workspace};
use recur_engine::view::chat::ChatAttachmentDraft;
use recur_engine::view::loops::{
    LoopInfo, PolicyInfo, autonomy_from_policy_id, default_loop_id, default_policy_id,
    enabled_loops, enabled_policies, fallback_loops, fallback_policies,
};
use recur_engine::view::mentions::strip_attachment_markers;
use recur_engine::view::new_workspace::{
    EagerCancel, eager_create_input, end_eager_create, opening_chat_seed, opening_seed_text,
    start_eager_create,
};
use recur_engine::view::prompts::{
    PromptWorkspace, build_prompt_context, find_prompt_by_slash, params_from_args,
    prompt_action_error_message, prompt_enabled,
};
use recur_ui::fonts::weight;
use recur_ui::shadow::with_shadow;
use recur_ui::style::{MODAL_SHADOW, button_label, modal_panel, modal_scrim, nt_esc_hint, outline};
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{
    PicBox, Select, SelectEvent, SelectOption, ToastKind, TriggerStyle, project_pic,
};
use recur_ui::{IconName, Theme, icon, motion};
use recur_view_chat::chat::{AgentChat, ChatEvent, ChatHooks, ChatProps};
use recur_view_chat::pickers::{LoopPolicyEvent, LoopPolicyPickers, ModelPickers, PickerEvent};
use recur_view_chat::prompt_actions::{PromptCatalog, prompt_action, run_prompt_action};
use recur_view_core::models::{FlagMap, flags_for_model, normalize_flags};
use recur_view_core::modes::{
    CatalogModel, ChatMode, is_mode_default_selection, resolved_default_for_mode,
};
use recur_view_core::{ViewContext, call, toast};

use crate::dialog::{cycle, window_keys};
use crate::new_workspace::{
    Abort, OpeningSeed, link_linear_project, reveal_created_workspace, wait_for_worktree,
    wire_attachments,
};

/// The remembered mode (`recur.newWorkspaceMode`).
pub const MODE_KEY: &str = "recur.newWorkspaceMode";

const LOOP_PLACEHOLDER: &str = "What do you want to work on?";
const AGENT_PLACEHOLDER: &str =
    "What should the agent do? (optional) — e.g. review PR 123, or add a dark-mode toggle";

type Pending = Shared<Task<Option<Workspace>>>;

pub struct NewTaskModal {
    vcx: ViewContext,
    focus: FocusHandle,
    open: bool,
    mode: ChatMode,
    task: String,
    attachments: Vec<ChatAttachmentDraft>,
    seed_wire: String,
    seed_display: String,
    starting: bool,
    linear_key: String,
    linked_project_id: String,
    link_linear: bool,
    issue_ref: Option<IssueRef>,
    repo_id: String,
    base_branch: String,
    branches: Vec<BranchChoice>,
    loading_branches: bool,
    branch_key: Option<(String, String)>,
    branch_task: Option<Task<()>>,
    pending_id: String,
    pending: Option<Pending>,
    eager: Option<EagerCancel>,
    eager_key: Option<(String, String)>,
    claimed: bool,
    loop_id: String,
    policy_id: String,
    catalog_loops: Vec<LoopInfo>,
    catalog_policies: Vec<PolicyInfo>,
    catalog_models: Vec<CatalogModel>,
    model: String,
    flags: FlagMap,
    simplify_on: bool,
    simplify_seeded: bool,
    seen_settings: Option<(Option<Settings>, u64)>,
    /// Bumped whenever `catalog_models` changes (the open effect tracks it).
    catalog_gen: u64,
    seen_catalog_rev: Option<u64>,
    prompt_busy: bool,
    prompt_abort: Option<Abort>,
    chat: Entity<AgentChat>,
    repo_sel: Entity<Select>,
    branch_sel: Entity<Select>,
    mode_sel: Entity<Select>,
    loops: Entity<LoopPolicyPickers>,
    models: Entity<ModelPickers>,
    link_focus: FocusHandle,
    empty_focus: FocusHandle,
    simplify_focus: FocusHandle,
    go_focus: FocusHandle,
    hover_x: bool,
    chat_key: Option<(bool, bool, String, String, String, String)>,
    _subs: Vec<Subscription>,
}

impl Focusable for NewTaskModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `.nt-pickers .sel-btn`: borderless 30px triggers in the UI font, `--pane-2` on hover.
pub(crate) fn picker_trigger(t: &recur_ui::Tokens, vw: f32) -> TriggerStyle {
    TriggerStyle {
        height: Some(30.),
        pad_x: Some(8.),
        radius: Some(7.),
        gap: Some(7.),
        font_size: Some(13.),
        ui_font: true,
        bg: Some(recur_ui::color::TRANSPARENT),
        border: Some(recur_ui::color::TRANSPARENT),
        color: Some(t.text),
        hover_bg: Some(t.pane_2),
        hover_border: Some(recur_ui::color::TRANSPARENT),
        open_bg: Some(t.pane_2),
        open_border: Some(recur_ui::color::TRANSPARENT),
        open_color: Some(t.text),
        no_ring: true,
        icon_color: Some(t.muted),
        open_icon_color: Some(t.muted),
        open_chevron_color: Some(t.muted),
        auto_width: true,
        max_width: Some(240f32.min(0.46 * vw)),
        value_shrink: true,
        no_outline: true,
        kb_ring: true,
        ..Default::default()
    }
}

impl NewTaskModal {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mode = ChatMode::from_key(vcx.state.read(cx).storage().get(MODE_KEY).unwrap_or(""));
        let chat = cx.new(|cx| {
            AgentChat::new(
                vcx,
                ChatProps {
                    composer_only: true,
                    placeholder: AGENT_PLACEHOLDER.into(),
                    ..Default::default()
                },
                window,
                cx,
            )
        });
        chat.update(cx, |c, cx| {
            c.set_hooks(
                ChatHooks {
                    send: true,
                    prompt_action: true,
                    ..Default::default()
                },
                cx,
            )
        });
        let mk =
            |label: &'static str, searchable: bool, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    // `.nt-pickers` menus are `.sel-menu-wide` (`portalMenuMods`).
                    let s = Select::new(Vec::new(), window, cx)
                        .label(label)
                        .menu_variant(recur_ui::logic::menu_space::MenuVariant::Wide);
                    if searchable { s.searchable() } else { s }
                })
            };
        let repo_sel = mk("Repository", false, window, cx);
        let branch_sel = mk("Base branch", true, window, cx);
        branch_sel.update(cx, |s, cx| s.set_icon(Some(IconName::GitBranch), cx));
        let mode_sel = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Mode")
                .ghost()
                .up()
                .trigger_style(TriggerStyle {
                    no_outline: true,
                    kb_ring: true,
                    ..Default::default()
                })
        });
        let loops = cx.new(|cx| LoopPolicyPickers::new(true, true, true, window, cx));
        loops.update(cx, |l, cx| l.set_kb_ring(true, cx));
        let models = cx.new(|cx| ModelPickers::new(true, true, window, cx));
        models.update(cx, |m, cx| {
            m.set_trigger_style(
                TriggerStyle {
                    no_outline: true,
                    kb_ring: true,
                    ..Default::default()
                },
                cx,
            )
        });
        let subs = vec![
            window_keys(cx, Self::on_key),
            cx.observe_in(&vcx.state, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            cx.subscribe_in(
                &chat,
                window,
                |this, chat, ev: &ChatEvent, _, cx| match ev {
                    ChatEvent::Draft => {
                        let c = chat.read(cx);
                        this.task = c.input().to_string();
                        this.attachments = c.attachments().to_vec();
                        this.sync_chat(cx);
                        cx.notify();
                    }
                    ChatEvent::Send { .. } => this.start(cx),
                    ChatEvent::PromptAction { slash, args, .. } => {
                        this.invoke_prompt_action(slash.clone(), args.clone(), cx)
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &repo_sel,
                window,
                |this, _, ev: &SelectEvent, window, cx| {
                    if let SelectEvent::Change(v) = ev {
                        this.repo_id = v.to_string();
                        this.effects(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &branch_sel,
                window,
                |this, _, ev: &SelectEvent, window, cx| {
                    if let SelectEvent::Change(v) = ev {
                        this.base_branch = v.to_string();
                        this.effects(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &mode_sel,
                window,
                |this, _, ev: &SelectEvent, window, cx| {
                    if let SelectEvent::Change(v) = ev {
                        this.pick_mode(ChatMode::from_key(v), cx);
                        this.effects(window, cx);
                    }
                },
            ),
            cx.subscribe(&loops, |this, _, ev: &LoopPolicyEvent, cx| {
                match ev {
                    LoopPolicyEvent::Loop(id) => this.loop_id = id.clone(),
                    LoopPolicyEvent::Policy(id) => this.policy_id = id.clone(),
                }
                cx.notify();
            }),
            cx.subscribe(&models, |this, _, ev: &PickerEvent, cx| {
                match ev {
                    PickerEvent::Model(r, f) => {
                        this.model = r.clone();
                        this.flags = f.clone();
                    }
                    PickerEvent::Flags(f) => this.flags = f.clone(),
                }
                cx.notify();
            }),
        ];
        let catalog_models = vcx.state.read(cx).peek_model_catalog().to_vec();
        NewTaskModal {
            vcx: vcx.clone(),
            focus: cx.focus_handle(),
            open: false,
            mode,
            task: String::new(),
            attachments: Vec::new(),
            seed_wire: String::new(),
            seed_display: String::new(),
            starting: false,
            linear_key: String::new(),
            linked_project_id: String::new(),
            link_linear: true,
            issue_ref: None,
            repo_id: String::new(),
            base_branch: String::new(),
            branches: Vec::new(),
            loading_branches: false,
            branch_key: None,
            branch_task: None,
            pending_id: String::new(),
            pending: None,
            eager: None,
            eager_key: None,
            claimed: false,
            loop_id: default_loop_id(None),
            policy_id: default_policy_id(None),
            catalog_loops: fallback_loops(),
            catalog_policies: fallback_policies(),
            catalog_models,
            model: String::new(),
            flags: FlagMap::new(),
            simplify_on: true,
            simplify_seeded: false,
            seen_settings: None,
            catalog_gen: 0,
            seen_catalog_rev: None,
            prompt_busy: false,
            prompt_abort: None,
            chat,
            repo_sel,
            branch_sel,
            mode_sel,
            loops,
            models,
            link_focus: cx.focus_handle().tab_stop(true),
            empty_focus: cx.focus_handle().tab_stop(true),
            simplify_focus: cx.focus_handle().tab_stop(true),
            go_focus: cx.focus_handle().tab_stop(true),
            hover_x: false,
            chat_key: None,
            _subs: subs,
        }
    }

    // ---- derived ----

    fn git_projects(&self, cx: &App) -> Vec<recur_domain::Project> {
        self.vcx
            .state
            .read(cx)
            .projects
            .iter()
            .filter(|p| p.is_git)
            .cloned()
            .collect()
    }

    /// `repo`: the picked git project, else the first.
    fn repo(&self, cx: &App) -> Option<recur_domain::Project> {
        let git = self.git_projects(cx);
        git.iter()
            .find(|p| p.id == self.repo_id)
            .or(git.first())
            .cloned()
    }

    pub fn mode(&self) -> ChatMode {
        self.mode
    }

    pub fn model(&self) -> (&str, &FlagMap) {
        (&self.model, &self.flags)
    }

    pub fn pending_id(&self) -> &str {
        &self.pending_id
    }

    fn offer_linear_link(&self, cx: &App) -> bool {
        !self.linear_key.is_empty()
            && self
                .repo(cx)
                .is_some_and(|r| r.id != self.linked_project_id)
    }

    /// `canStart`: a repository and base, branches loaded, and for Loop a task.
    pub fn can_start(&self, cx: &App) -> bool {
        self.repo(cx).is_some_and(|r| !r.id.is_empty())
            && !self.base_branch.is_empty()
            && !self.loading_branches
            && !self.starting
            && (self.mode != ChatMode::Loop || !self.task.trim().is_empty())
    }

    fn pickers_ready(&self) -> bool {
        !self.catalog_models.is_empty() && !self.model.is_empty()
    }

    // ---- effects ----

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.vcx.state.read(cx).overlays.new_task;
        if open && !self.open {
            self.open = true;
            self.on_open(window, cx);
        } else if !open && self.open {
            // Focus inside the closed dialog falls back to the document (`body`).
            let inside = self.stops(cx).iter().any(|h| h.is_focused(window))
                || self.chat.focus_handle(cx).contains_focused(window, cx);
            self.open = false;
            self.on_close(cx);
            if inside {
                window.blur(cx);
            }
        }
        self.effects(window, cx);
    }

    fn on_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // `$effect.pre`: a fresh open has not claimed its eager row.
        self.claimed = false;
        // Re-probe isGit so a mid-session `git init` shows up.
        self.vcx
            .state
            .update(cx, |s, cx| s.refresh_projects(cx))
            .detach();
        self.seen_settings = None;
        self.seen_catalog_rev = None;
        let svc = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let loops = svc.bundle.list_loops().await.unwrap_or_default();
            let policies = svc.bundle.list_policies().await.unwrap_or_default();
            let _ = this.update(cx, |m, cx| {
                if !m.open {
                    return;
                }
                if !loops.is_empty() {
                    m.catalog_loops = loops
                        .into_iter()
                        .map(|l| LoopInfo {
                            id: l.id,
                            name: l.name,
                            hint: l.hint,
                        })
                        .collect();
                }
                if !policies.is_empty() {
                    m.catalog_policies = policies
                        .into_iter()
                        .map(|p| PolicyInfo {
                            id: p.id,
                            name: p.name,
                            hint: p.hint,
                        })
                        .collect();
                }
                cx.notify();
            });
        })
        .detach();
        self.apply_draft(window, cx);
        self.focus_composer(window, cx);
    }

    fn on_close(&mut self, cx: &mut Context<Self>) {
        if let Some(a) = self.prompt_abort.take() {
            a.abort();
        }
        self.linear_key.clear();
        self.issue_ref = None;
        self.simplify_seeded = false;
        cx.notify();
    }

    fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chat = self.chat.clone();
        window.defer(cx, move |window, cx| {
            chat.update(cx, |c, cx| c.focus_composer(window, cx));
        });
    }

    /// The draft effect: a prefill from the Issues list, Welcome or a palette.
    fn apply_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.vcx.state.update(cx, |s, _| s.new_task_draft.take()) else {
            return;
        };
        if !d.mode.is_empty() {
            self.pick_mode(ChatMode::from_key(&d.mode), cx);
        }
        self.seed_display = d.display.trim().to_string();
        self.seed_wire = if self.seed_display.is_empty() {
            String::new()
        } else {
            d.text.clone()
        };
        self.task = if self.seed_display.is_empty() {
            d.text.clone()
        } else {
            self.seed_display.clone()
        };
        self.attachments.clear();
        let task = self.task.clone();
        self.chat
            .update(cx, |c, cx| c.set_draft(task, Vec::new(), cx));
        self.linear_key = d.linear_key;
        self.linked_project_id = d.linked_project_id;
        self.link_linear = true;
        self.focus_composer(window, cx);
    }

    /// Every `$effect` that follows store and picker state.
    fn effects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.repo_preference(cx);
            self.defaults_effect(window, cx);
            self.simplify_effect(cx);
            self.catalog_effect(window, cx);
        }
        self.branches_effect(cx);
        self.eager_effect(cx);
        self.sync_chat(cx);
        cx.notify();
    }

    /// Prefer `openNewTask`'s preselect, else the first git project; a non-git preference is
    /// dropped so the modal never lands on an unusable host.
    fn repo_preference(&mut self, cx: &mut Context<Self>) {
        let git = self.git_projects(cx);
        let pref = self.vcx.state.read(cx).new_task_repo_id.clone();
        if !pref.is_empty() {
            self.vcx.state.update(cx, |s, _| s.new_task_repo_id.clear());
            if git.iter().any(|p| p.id == pref) {
                self.repo_id = pref;
                return;
            }
        }
        if !self.repo_id.is_empty() && !git.iter().any(|p| p.id == self.repo_id) {
            self.repo_id.clear();
        }
        if self.repo_id.is_empty()
            && let Some(p) = git.first()
        {
            self.repo_id = p.id.clone();
        }
    }

    /// On open (and when Settings or the catalog change): the default loop, policy and the
    /// mode's model; the mode keeps the remembered choice.
    fn defaults_effect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.vcx.state.read(cx).settings.clone();
        let key = Some((settings.clone(), self.catalog_gen));
        if self.seen_settings == key {
            return;
        }
        self.seen_settings = key;
        self.loop_id = default_loop_id(settings.as_ref());
        self.policy_id = default_policy_id(settings.as_ref());
        let d = resolved_default_for_mode(self.mode.key(), settings.as_ref(), &self.catalog_models);
        self.model = d.model;
        self.flags = d.flags;
        self.focus_composer(window, cx);
    }

    /// Simplify starts from Settings once they have loaded, then is the user's.
    fn simplify_effect(&mut self, cx: &mut Context<Self>) {
        if self.simplify_seeded {
            return;
        }
        if let Some(s) = self.vcx.state.read(cx).settings.as_ref() {
            self.simplify_on = s.simplify_enabled();
            self.simplify_seeded = true;
        }
    }

    /// Load the model catalog on open and whenever it is invalidated.
    fn catalog_effect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rev = self.vcx.state.read(cx).model_catalog_rev();
        if self.seen_catalog_rev == Some(rev) {
            return;
        }
        self.seen_catalog_rev = Some(rev);
        let load = self.vcx.state.update(cx, |s, cx| s.load_model_catalog(cx));
        cx.spawn_in(window, async move |this, cx| {
            let models = load.await;
            let _ = this.update_in(cx, |m, window, cx| {
                if !m.open {
                    return;
                }
                let settings = m.vcx.state.read(cx).settings.clone();
                let d = resolved_default_for_mode(m.mode.key(), settings.as_ref(), &models);
                // Only follow the catalogue while still on the mode default.
                if m.model.is_empty() || m.model == d.model {
                    m.model = d.model;
                    m.flags = d.flags;
                } else {
                    m.flags = normalize_flags(&flags_for_model(&models, &m.model), Some(&m.flags));
                }
                m.catalog_models = models;
                m.catalog_gen += 1;
                m.effects(window, cx);
            });
        })
        .detach();
    }

    /// Branches of the picked repository, defaulting to its configured base. Keyed on the
    /// project id and fallback, not the project object, so a refresh does not reload them.
    fn branches_effect(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo(cx);
        let id = repo.as_ref().map(|r| r.id.clone()).unwrap_or_default();
        let fallback = repo
            .as_ref()
            .map(|r| r.base_branch.clone())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "main".into());
        let key = (id.clone(), fallback.clone());
        if self.branch_key.as_ref() == Some(&key) {
            return;
        }
        self.branch_key = Some(key);
        if id.is_empty() {
            self.branches.clear();
            self.base_branch.clear();
            self.branch_task = None;
            self.loading_branches = false;
            return;
        }
        self.loading_branches = true;
        self.base_branch = fallback.clone();
        let services = self.vcx.services.clone();
        self.branch_task = Some(cx.spawn(async move |this, cx| {
            let r = services.projects.list_branches(&id).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(list) => {
                        let next = if list.is_empty() {
                            vec![BranchChoice {
                                name: fallback.clone(),
                                group: "Local".into(),
                            }]
                        } else {
                            list
                        };
                        m.base_branch = if next.iter().any(|b| b.name == fallback) {
                            fallback
                        } else {
                            next.first().map(|b| b.name.clone()).unwrap_or(fallback)
                        };
                        m.branches = next;
                    }
                    Err(_) => {
                        m.branches = vec![BranchChoice {
                            name: fallback.clone(),
                            group: "Local".into(),
                        }];
                        m.base_branch = fallback;
                    }
                }
                m.loading_branches = false;
                m.branch_task = None;
                m.eager_effect(cx);
                m.sync_chat(cx);
                cx.notify();
            });
        }));
    }

    /// Start the worktree as soon as the modal has a repository and base (not in Loop, which
    /// needs the goal). A changed repository or base, Loop, or closing abandons the unclaimed
    /// row.
    fn eager_effect(&mut self, cx: &mut Context<Self>) {
        let input = eager_create_input(
            self.open,
            self.mode == ChatMode::Loop,
            &self.repo_id,
            &self.base_branch,
            self.claimed,
        );
        if input == self.eager_key {
            return;
        }
        if let Some(cancel) = self.eager.take() {
            end_eager_create(self.claimed, &cancel);
            self.pending_id.clear();
            self.pending = None;
        }
        self.eager_key = input.clone();
        let Some((project, branch)) = input else {
            return;
        };
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        let me = cx.entity().downgrade();
        // Abandon runs off the UI thread; the drop lands back on it.
        let (atx, mut arx) = futures::channel::mpsc::unbounded::<String>();
        let (rtx, mut rrx) = futures::channel::mpsc::unbounded::<Workspace>();
        let svc = services.clone();
        let create = async move {
            svc.workspaces
                .create_workspace(&project, "", &branch, "agent", "", "", None)
                .await
                .ok()
        };
        let (cancel, fut) = start_eager_create(
            create,
            move |ws| {
                let _ = rtx.unbounded_send(ws.clone());
            },
            move |id| {
                let _ = atx.unbounded_send(id.to_string());
            },
        );
        let st = state.clone();
        let svc = services.clone();
        cx.spawn(async move |_, cx| {
            use futures::StreamExt as _;
            while let Some(id) = arx.next().await {
                let s = svc.clone();
                let del = id.clone();
                cx.background_spawn(async move {
                    let _ = s.workspaces.delete_workspace(&del).await;
                })
                .detach();
                st.update(cx, |s, cx| s.drop_workspace(&id, cx));
            }
        })
        .detach();
        cx.spawn(async move |_, cx| {
            use futures::StreamExt as _;
            if let Some(ws) = rrx.next().await {
                let id = ws.id.clone();
                state.update(cx, |s, cx| s.upsert_workspace(ws, cx));
                let _ = me.update(cx, |m, cx| {
                    if m.eager.is_some() {
                        m.pending_id = id;
                        m.sync_chat(cx);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        let task: Task<Option<Workspace>> = cx.background_spawn(fut);
        self.pending = Some(task.shared());
        self.eager = Some(cancel);
    }

    /// `pickMode`: follow the new mode's default model only while on the old mode's default.
    fn pick_mode(&mut self, m: ChatMode, cx: &mut Context<Self>) {
        let settings = self.vcx.state.read(cx).settings.clone();
        if is_mode_default_selection(
            self.mode.key(),
            &self.model,
            &self.flags,
            settings.as_ref(),
            &self.catalog_models,
        ) {
            let d = resolved_default_for_mode(m.key(), settings.as_ref(), &self.catalog_models);
            self.model = d.model;
            self.flags = d.flags;
        }
        self.mode = m;
        self.vcx.state.update(cx, |s, _| {
            s.storage_mut().set(MODE_KEY, m.key().to_string())
        });
        cx.notify();
    }

    /// The AgentChat props.
    fn sync_chat(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo(cx);
        let props = ChatProps {
            composer_only: true,
            sending: self.starting,
            prompt_busy: self.prompt_busy || self.starting,
            project_id: repo.as_ref().map(|r| r.id.clone()).unwrap_or_default(),
            workspace_id: self.pending_id.clone(),
            root_path: repo.map(|r| r.repo_path).unwrap_or_default(),
            placeholder: if self.mode == ChatMode::Loop {
                LOOP_PLACEHOLDER.into()
            } else {
                AGENT_PLACEHOLDER.into()
            },
            ..Default::default()
        };
        let key = (
            props.sending,
            props.prompt_busy,
            props.project_id.clone(),
            props.workspace_id.clone(),
            props.root_path.clone(),
            props.placeholder.clone(),
        );
        if self.chat_key.as_ref() != Some(&key) {
            self.chat_key = Some(key);
            self.chat.update(cx, |c, cx| c.set_props(props, cx));
        }
    }

    // ---- actions ----

    fn close(&mut self, cx: &mut Context<Self>) {
        self.vcx.state.update(cx, |s, cx| s.close_new_task(cx));
    }

    fn go_add_project(&mut self, cx: &mut Context<Self>) {
        self.vcx
            .state
            .update(cx, |s, cx| s.open_new_project(true, cx));
    }

    /// `submit`: a typed task goes through the composer (so a committed `/prompt` runs);
    /// an empty one creates straight away.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt_busy || self.starting {
            return;
        }
        if !self.task.trim().is_empty() || !self.attachments.is_empty() {
            self.chat.update(cx, |c, cx| c.send(false, window, cx));
            return;
        }
        self.start(cx);
    }

    /// `start`: Loop engages the autopilot with the task as its goal; any other mode takes
    /// the eager row (or creates one) and seeds the prompt to its first chat tab.
    pub fn start(&mut self, cx: &mut Context<Self>) {
        if !self.can_start(cx) {
            return;
        }
        self.claimed = true;
        self.starting = true;
        self.sync_chat(cx);
        cx.notify();
        let marked = self.task.trim().to_string();
        let drafts = self.attachments.clone();
        let mode = self.mode;
        let mut prompt = strip_attachment_markers(&marked);
        if prompt.is_empty() && mode == ChatMode::Loop && !drafts.is_empty() {
            prompt = "image attachment".into();
        }
        let (seed_wire, seed_display) = (self.seed_wire.clone(), self.seed_display.clone());
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        let repo = self.repo(cx);
        let (pending_id, pending) = (self.pending_id.clone(), self.pending.clone());
        let (base, loop_id, policy_id) = (
            self.base_branch.clone(),
            self.loop_id.clone(),
            self.policy_id.clone(),
        );
        let (model, flags, simplify) = (self.model.clone(), self.flags.clone(), self.simplify_on);
        let link =
            (self.offer_linear_link(cx) && self.link_linear).then(|| self.linear_key.clone());
        cx.spawn(async move |this, cx| {
            let Some(repo) = repo else { return };
            let mut display = String::new();
            let mut prompt = prompt;
            if !prompt.is_empty() {
                let expanded = if !seed_wire.is_empty() && prompt == seed_display {
                    Ok(recur_services::api::PromptResultDto {
                        prompt: seed_wire.clone(),
                        display: seed_display.clone(),
                        ..Default::default()
                    })
                } else {
                    services.bundle.expand_goal(&prompt).await
                };
                match expanded {
                    Ok(e) => {
                        if let Some(issue) = e.issue.clone() {
                            let _ = this.update(cx, |m, _| m.issue_ref = Some(issue));
                        }
                        let (text, d) = opening_chat_seed(
                            &prompt,
                            &seed_wire,
                            &seed_display,
                            Some((&e.prompt, &e.display)),
                        );
                        prompt = text;
                        display = d;
                    }
                    Err(err) => {
                        let msg = recur_view_core::bridge_error_message(&err);
                        cx.update(|cx| {
                            toast(
                                ToastKind::Error,
                                if msg.is_empty() {
                                    "Could not fetch Linear issue".into()
                                } else {
                                    msg
                                },
                                cx,
                            )
                        });
                        let _ = this.update(cx, |m, cx| {
                            m.claimed = false;
                            m.starting = false;
                            m.sync_chat(cx);
                            cx.notify();
                        });
                        return;
                    }
                }
            }
            let mut ws: Option<Workspace> = None;
            if mode != ChatMode::Loop {
                let rows = state.read_with(cx, |s, _| s.workspaces.clone());
                ws = recur_engine::view::new_workspace::take_eager_workspace(
                    &pending_id,
                    pending,
                    &rows,
                )
                .await;
                if let Some(w) = &ws {
                    let live = state.read_with(cx, |s, _| s.workspace(&w.id).cloned());
                    if let Some(live) = live {
                        ws = Some(live);
                    }
                }
                if let Some(w) = ws.clone()
                    && (!w.last_error.is_empty() || w.phase == "failed")
                {
                    ws = None;
                    let _ = this.update(cx, |m, _| {
                        m.pending_id.clear();
                        m.pending = None;
                        m.eager = None;
                    });
                    let s = services.clone();
                    let id = w.id.clone();
                    cx.background_spawn(async move {
                        let _ = s.workspaces.delete_workspace(&id).await;
                    })
                    .detach();
                    state.update(cx, |s, cx| s.drop_workspace(&w.id, cx));
                }
            }
            if ws.is_none() {
                let auto = autonomy_from_policy_id(&policy_id);
                ws = if mode == ChatMode::Loop {
                    call(
                        cx,
                        services.workspaces.create_workspace_with(
                            &repo.id,
                            &display,
                            &base,
                            mode.key(),
                            &prompt,
                            auto,
                            &loop_id,
                            &policy_id,
                            Some(simplify),
                            &wire_attachments(&drafts),
                        ),
                        "Could not start the loop",
                    )
                    .await
                } else {
                    call(
                        cx,
                        services.workspaces.create_workspace(
                            &repo.id,
                            "",
                            &base,
                            mode.key(),
                            &prompt,
                            auto,
                            None,
                        ),
                        "Could not create the workspace",
                    )
                    .await
                };
            }
            let Some(ws) = ws else {
                let _ = this.update(cx, |m, cx| {
                    m.claimed = false;
                    m.starting = false;
                    m.sync_chat(cx);
                    cx.notify();
                });
                return;
            };
            let id = ws.id.clone();
            state.update(cx, |s, cx| s.upsert_workspace(ws, cx));
            if let Some(key) = link {
                let (st, pid) = (state.clone(), repo.id.clone());
                cx.spawn(async move |cx| link_linear_project(&st, &pid, &key, cx).await)
                    .detach();
            }
            let issue = this.update(cx, |m, _| m.issue_ref.clone()).ok().flatten();
            if let Some(issue) = issue {
                let id = id.clone();
                state.update(cx, |s, cx| s.link_workspace_issue(&id, issue, cx));
            }
            let _ = this.update(cx, |m, cx| {
                m.task.clear();
                m.attachments.clear();
                m.chat
                    .update(cx, |c, cx| c.set_draft(String::new(), Vec::new(), cx));
            });
            state.update(cx, |s, cx| s.close_new_task(cx));
            cx.update(|cx| {
                toast(
                    ToastKind::Success,
                    if mode == ChatMode::Loop {
                        "Loop started"
                    } else {
                        "Workspace created"
                    },
                    cx,
                )
            });
            let _ = this.update(cx, |m, cx| {
                m.starting = false;
                m.sync_chat(cx);
                cx.notify();
            });
            let ids: Vec<&str> = drafts.iter().map(|a| a.id.as_str()).collect();
            let seed = OpeningSeed {
                id,
                text: opening_seed_text(&marked, &prompt, ids),
                display,
                mode: mode.key().into(),
                model,
                flags,
                simplify: Some(simplify),
                attachments: drafts,
            };
            reveal_created_workspace(&state, seed, mode == ChatMode::Loop, cx).await;
        })
        .detach();
    }

    /// `invokePromptAction`: a committed `/prompt` in the composer runs before the workspace
    /// is created (in its pending worktree); a prompt that returns text starts the workspace
    /// with it.
    fn invoke_prompt_action(&mut self, slash: String, args: String, cx: &mut Context<Self>) {
        if self.prompt_busy || self.starting {
            return;
        }
        let Some(repo) = self.repo(cx).filter(|r| !r.id.is_empty()) else {
            return;
        };
        if let Some(a) = self.prompt_abort.take() {
            a.abort();
        }
        let abort = Abort::default();
        self.prompt_abort = Some(abort.clone());
        self.prompt_busy = true;
        self.sync_chat(cx);
        cx.notify();
        let slash_name = slash.trim_start_matches('/').trim().to_string();
        let project_id = repo.id.clone();
        let services = self.vcx.services.clone();
        let state = self.vcx.state.clone();
        cx.spawn(async move |this, cx| {
            let epoch = state.read_with(cx, |s, _| s.bundles_epoch);
            let load = cx.update(|cx| PromptCatalog::load(&services, epoch, cx));
            let list = load.await;
            let res: Result<(), String> = async {
                if abort.aborted() {
                    return Ok(());
                }
                let actions: Vec<PromptAction> = list.iter().map(prompt_action).collect();
                let Some(found) = find_prompt_by_slash(&slash_name, &actions).cloned() else {
                    cx.update(|cx| {
                        toast(
                            ToastKind::Error,
                            format!("Unknown prompt /{slash_name}"),
                            cx,
                        )
                    });
                    return Ok(());
                };
                let Some(info) = list.iter().find(|i| i.id == found.id).cloned() else {
                    return Ok(());
                };
                let disabled = state.read_with(cx, |s, _| {
                    s.settings
                        .as_ref()
                        .is_some_and(|set| !prompt_enabled(set, &info.id))
                });
                if disabled {
                    cx.update(|cx| {
                        toast(
                            ToastKind::Error,
                            format!("{} is disabled in Settings → Bundles", info.name),
                            cx,
                        )
                    });
                    return Ok(());
                }
                let _ = this.update(cx, |m, cx| {
                    if !info.mode.is_empty() {
                        m.pick_mode(ChatMode::from_key(&info.mode), cx);
                    }
                    if !info.r#loop.trim().is_empty() {
                        m.loop_id = info.r#loop.trim().to_string();
                    }
                });
                let still = |cx: &mut gpui_kit::AsyncApp| {
                    let open = state.read_with(cx, |s, _| s.overlays.new_task);
                    let same = this
                        .read_with(cx, |m, cx| m.repo(cx).is_some_and(|r| r.id == project_id))
                        .unwrap_or(false);
                    !abort.aborted() && open && same
                };
                if !still(cx) {
                    return Ok(());
                }
                let (pending_id, pending) = this
                    .read_with(cx, |m, _| (m.pending_id.clone(), m.pending.clone()))
                    .unwrap_or_default();
                let rows = state.read_with(cx, |s, _| s.workspaces.clone());
                let mut ws = recur_engine::view::new_workspace::take_eager_workspace(
                    &pending_id,
                    pending,
                    &rows,
                )
                .await;
                if !still(cx) {
                    return Ok(());
                }
                if let Some(w) = ws.clone()
                    && !w.id.is_empty()
                    && w.worktree_path.trim().is_empty()
                {
                    if !w.last_error.is_empty() || w.phase == "failed" {
                        cx.update(|cx| {
                            toast(ToastKind::Error, "Could not create the workspace", cx)
                        });
                        return Ok(());
                    }
                    let ready = wait_for_worktree(&state, &w.id, &abort, cx).await;
                    let same_pending = this
                        .read_with(cx, |m, _| m.pending_id == w.id)
                        .unwrap_or(false);
                    if !still(cx) || !same_pending {
                        return Ok(());
                    }
                    let Some(ready) = ready else {
                        cx.update(|cx| {
                            toast(ToastKind::Error, "Could not create the workspace", cx)
                        });
                        return Ok(());
                    };
                    ws = Some(ready);
                }
                // simplify: Loop mode has no pending worktree, so a shebang runs in the
                // project checkout. Upgrade: allocate a worktree before prompts that need one.
                let invoked = ws
                    .as_ref()
                    .map(|w| w.id.clone())
                    .unwrap_or_else(|| pending_id.clone());
                let worktree = ws
                    .as_ref()
                    .map(|w| w.worktree_path.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| repo.repo_path.clone());
                let ctx = build_prompt_context(
                    &PromptWorkspace {
                        id: invoked.clone(),
                        project_id: project_id.clone(),
                        worktree_path: worktree,
                        port: ws.as_ref().map(|w| w.port).unwrap_or_default(),
                    },
                    None,
                    "workspace",
                    &args,
                    params_from_args(Some(&found), &args),
                );
                let run = cx.update(|cx| run_prompt_action(&info, ctx, "", &services, cx));
                let mut signal = abort.signal();
                let result = futures::select_biased! {
                    _ = signal => return Err("prompt cancelled".to_string()),
                    r = run.fuse() => r,
                };
                let same_pending = this
                    .read_with(cx, |m, _| m.pending_id == invoked)
                    .unwrap_or(false);
                if !still(cx) || !same_pending {
                    return Ok(());
                }
                let result = result.map_err(|e| prompt_action_error_message(&e))?;
                let display = if result.display.is_empty() {
                    info.name.clone()
                } else {
                    result.display.clone()
                };
                if let Some(issue) = result.issue.clone() {
                    let _ = this.update(cx, |m, _| m.issue_ref = Some(issue));
                }
                if result.prompt.trim().is_empty() {
                    cx.update(|cx| toast(ToastKind::Success, display, cx));
                    return Ok(());
                }
                let _ = this.update(cx, |m, cx| {
                    m.seed_wire = result.prompt.clone();
                    m.seed_display = display.clone();
                    m.task = display.clone();
                    m.chat
                        .update(cx, |c, cx| c.set_draft(display.clone(), Vec::new(), cx));
                    m.prompt_busy = false;
                    m.start(cx);
                });
                Ok(())
            }
            .await;
            if let Err(msg) = res
                && !abort.aborted()
                && msg != "prompt cancelled"
            {
                cx.update(|cx| toast(ToastKind::Error, msg, cx));
            }
            let _ = this.update(cx, |m, cx| {
                if m.prompt_abort.as_ref().is_some_and(|a| a.same(&abort)) {
                    m.prompt_abort = None;
                }
                m.prompt_busy = false;
                m.sync_chat(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

impl NewTaskModal {
    /// The dialog's Tab stops in DOM order (disabled controls and `tabindex=-1` skipped).
    fn stops(&self, cx: &App) -> Vec<FocusHandle> {
        let mut v = Vec::new();
        if self.git_projects(cx).is_empty() {
            v.push(self.empty_focus.clone());
        } else {
            v.push(self.repo_sel.focus_handle(cx));
            if !self.loading_branches && !self.branches.is_empty() {
                v.push(self.branch_sel.focus_handle(cx));
            }
            if self.offer_linear_link(cx) {
                v.push(self.link_focus.clone());
            }
        }
        v.push(self.chat.focus_handle(cx));
        v.push(self.mode_sel.focus_handle(cx));
        if self.mode == ChatMode::Loop {
            v.extend(self.loops.read(cx).focus_handles(cx));
        } else if self.pickers_ready() {
            v.extend(self.models.read(cx).focus_handles(cx));
        }
        v.push(self.simplify_focus.clone());
        if self.can_start(cx) && !self.prompt_busy {
            v.push(self.go_focus.clone());
        }
        v
    }

    fn any_select_open(&self, cx: &App) -> bool {
        [&self.repo_sel, &self.branch_sel, &self.mode_sel]
            .iter()
            .any(|s| s.read(cx).is_open())
            || self.loops.read(cx).is_open(cx)
            || self.models.read(cx).is_open(cx)
    }

    /// The window keydown listener (capture): ⌘. cycles the mode, Tab cycles the dialog
    /// (unless the composer's menu owns it), ⌘↵ submits, Escape closes unless a menu, the
    /// composer's menu or the lightbox is open.
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let m = &ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        let cmd = m.platform || m.control;
        if cmd && !m.shift && !m.alt && key == "." {
            cx.stop_propagation();
            self.pick_mode(self.mode.cycle(false, false), cx);
            self.effects(window, cx);
            return;
        }
        let chat = self.chat.read(cx);
        if key == "tab" && !m.platform && !m.control && !m.alt {
            if chat.image_overlay_open() || chat.menu_open() {
                return;
            }
            cx.stop_propagation();
            let stops = self.stops(cx);
            let at = stops.iter().position(|h| h.is_focused(window));
            if let Some(i) = cycle(stops.len(), at, m.shift) {
                window.focus(&stops[i], cx);
                cx.notify();
            }
            return;
        }
        if key == "enter" && cmd && !m.shift && !m.alt {
            cx.stop_propagation();
            self.submit(window, cx);
            return;
        }
        if key == "escape" {
            if self.any_select_open(cx) {
                return;
            }
            let chat = self.chat.read(cx);
            if chat.menu_open() || chat.image_overlay_open() {
                return;
            }
            cx.stop_propagation();
            self.close(cx);
        }
    }

    fn sync_selects(&mut self, window: &Window, cx: &mut Context<Self>) {
        let t = Theme::of(cx).tokens;
        let vw = f32::from(window.viewport_size().width);
        let git = self.git_projects(cx);
        let repo = self.repo(cx);
        let repo_opts: Vec<SelectOption> = git
            .iter()
            .map(|p| SelectOption::new(p.id.clone(), p.name.clone()))
            .collect();
        let repo_val = self.repo_id.clone();
        let slot: recur_ui::widgets::select::IconSlot = match repo {
            Some(r) => std::rc::Rc::new(move |_, _| {
                project_pic(r.pic.clone(), r.color.clone(), r.name.clone(), PicBox::NT)
                    .into_any_element()
            }),
            None => {
                std::rc::Rc::new(|_, _| icon(IconName::Folder).size(px(14.)).into_any_element())
            }
        };
        let trig = picker_trigger(&t, vw);
        self.repo_sel.update(cx, |s, cx| {
            s.set_options(repo_opts, cx);
            if s.value() != &repo_val {
                s.set_value(repo_val, cx);
            }
            s.set_icon_slot(Some(slot), cx);
            if s.trigger() != &trig {
                s.set_trigger_style(trig.clone(), cx);
            }
        });
        let branch_opts: Vec<SelectOption> = self
            .branches
            .iter()
            .map(|b| SelectOption::new(b.name.clone(), b.name.clone()).group(b.group.clone()))
            .collect();
        let disabled = self.loading_branches || branch_opts.is_empty();
        let base = self.base_branch.clone();
        self.branch_sel.update(cx, |s, cx| {
            s.set_options(branch_opts, cx);
            if s.value() != &base {
                s.set_value(base, cx);
            }
            s.set_disabled(disabled.then_some(t.pane), cx);
            if s.trigger() != &trig {
                s.set_trigger_style(trig, cx);
            }
        });
        let mode_opts: Vec<SelectOption> = ChatMode::options(false, false)
            .iter()
            .map(|m| {
                SelectOption::new(m.key(), m.label())
                    .icon(m.icon())
                    .tint(m.tint(&t))
                    .hint(m.hint())
            })
            .collect();
        let mode = self.mode.key();
        self.mode_sel.update(cx, |s, cx| {
            s.set_options(mode_opts, cx);
            if s.value() != mode {
                s.set_value(mode, cx);
            }
        });
        if self.mode == ChatMode::Loop {
            let settings = self.vcx.state.read(cx).settings.clone();
            let loops = enabled_loops(&self.catalog_loops, settings.as_ref());
            let policies = enabled_policies(&self.catalog_policies, settings.as_ref());
            let (l, p) = (self.loop_id.clone(), self.policy_id.clone());
            self.loops.update(cx, |lp, cx| {
                lp.set(&l, &p, &loops, &policies, false, t.pane, cx)
            });
        } else if self.pickers_ready() {
            let (models, model, flags) = (
                self.catalog_models.clone(),
                self.model.clone(),
                self.flags.clone(),
            );
            self.models
                .update(cx, |mp, cx| mp.set(models, &model, &flags, None, cx));
        }
    }

    fn simplify_chip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let on = self.simplify_on;
        let toggle = cx.listener(|m: &mut Self, _: &gpui_kit::ClickEvent, _, cx| {
            m.simplify_on = !m.simplify_on;
            m.simplify_seeded = true;
            cx.notify();
        });
        simplify_chip("nt-simplify", on, &self.simplify_focus, toggle, window, cx)
    }
}

/// `.wsv-simplify` in `.nt-foot`: the feather alone, 4px sides (`id` names the spec handle,
/// `{id}:on|off`).
pub(crate) fn simplify_chip(
    id: &'static str,
    on: bool,
    focus: &FocusHandle,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let h = motion::hover(
        format!("{id}-hover"),
        std::time::Duration::from_millis(150),
        motion::Easing::EASE,
        window,
        cx,
    );
    let lit = if on { 1. } else { h.k() };
    let fg = t.text.mix(lit, t.muted);
    let bg = t.pane_2.fade(lit);
    let ring = recur_ui::style::is_focus_visible(focus, window);
    let btn = div()
        .id(id)
        .debug_selector(move || format!("{id}:{}", if on { "on" } else { "off" }))
        .track_focus(focus)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .h(px(24.))
        .px(px(4.))
        .rounded(px(6.))
        .bg(Hsla::from(bg))
        .text_color(Hsla::from(fg))
        .cursor_pointer()
        .on_hover(h.listener())
        .on_click(on_click)
        .child(icon(IconName::Feather).size(px(12.)))
        .when(ring, |d| d.child(outline(2., 2., 6., 0., t.amber)));
    titled(
        format!("{id}-t"),
        if on { "Simplify on" } else { "Simplify off" },
        div().id(format!("{id}-wrap")).flex_none().child(btn),
    )
    .into_any_element()
}

impl Render for NewTaskModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let t = Theme::of(cx).tokens;
        self.sync_selects(window, cx);
        let git = self.git_projects(cx);
        let mut head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .pt(px(12.))
            .px(px(14.))
            .min_h(px(44.));
        if git.is_empty() {
            let ring = recur_ui::style::is_focus_visible(&self.empty_focus, window);
            let hovered = window.use_keyed_state("nt-empty-lnk-h", cx, |_, _| false);
            let over = *hovered.read(cx);
            let hh = hovered.clone();
            head = head.child(
                div()
                    .flex()
                    .items_baseline()
                    .px(px(2.))
                    .child(
                        text("No git projects yet — ")
                            .size(12.5)
                            .color(t.muted)
                            .pre_wrap(),
                    )
                    .child(
                        div()
                            .id("nt-empty-lnk")
                            .debug_selector(|| "nt-empty:add a repository first".into())
                            .relative()
                            .track_focus(&self.empty_focus)
                            .cursor_pointer()
                            .on_hover(move |o, _, cx| {
                                hh.update(cx, |v, cx| {
                                    *v = *o;
                                    cx.notify();
                                })
                            })
                            .on_click(cx.listener(|m, _, _, cx| m.go_add_project(cx)))
                            .child(
                                recur_ui::style::lnk("add a repository first", cx).underline(over),
                            )
                            .when(ring, |d| d.child(recur_ui::style::focus_ring(4., 0., cx))),
                    ),
            );
        } else {
            let mut pickers = div()
                .flex()
                .items_center()
                .gap(px(4.))
                .min_w_0()
                .flex_1()
                .child(div().flex_none().child(self.repo_sel.clone()))
                .child(div().flex_none().child(self.branch_sel.clone()));
            if self.offer_linear_link(cx)
                && let Some(repo) = self.repo(cx)
            {
                let on = self.link_linear;
                let key = self.linear_key.clone();
                let tip = format!(
                    "{} Linear “{}” issues to {} from now on",
                    if on { "Will send" } else { "Send" },
                    key,
                    if repo.name.is_empty() {
                        &repo.id
                    } else {
                        &repo.name
                    }
                );
                let ring = recur_ui::style::is_focus_visible(&self.link_focus, window);
                pickers = pickers.child(titled(
                    "nt-link-t",
                    tip,
                    recur_ui::style::link_button("nt-link", on, cx)
                        .debug_selector(move || {
                            format!("nt-link:{}", if on { "on" } else { "off" })
                        })
                        .relative()
                        .track_focus(&self.link_focus)
                        .on_click(cx.listener(|m, _, _, cx| {
                            m.link_linear = !m.link_linear;
                            cx.notify();
                        }))
                        .child(icon(IconName::Link).size(px(12.)))
                        .child(button_label(
                            format!("Remember for {key}"),
                            12.,
                            weight::REGULAR,
                        ))
                        .when(ring, |d| d.child(outline(2., 2., 6., 0., t.amber))),
                ));
            }
            head = head.child(pickers);
        }
        head = head.child(
            div()
                .id("nt-x")
                .debug_selector(|| "nt-x".into())
                .ml_auto()
                .flex_none()
                .cursor_pointer()
                .on_hover(cx.listener(|m, h: &bool, _, cx| {
                    m.hover_x = *h;
                    cx.notify();
                }))
                .on_click(cx.listener(|m, _, _, cx| m.close(cx)))
                .child(nt_esc_hint(self.hover_x, window, cx)),
        );

        let body = div()
            .flex()
            .flex_col()
            .min_w_0()
            .relative()
            .debug_selector(|| "nt-body".into())
            .child(self.chat.clone());

        let mut foot = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .pt(px(10.))
            .px(px(14.))
            .pb(px(14.))
            .min_w_0()
            .child(div().flex_none().child(self.mode_sel.clone()));
        if self.mode == ChatMode::Loop {
            foot = foot.child(self.loops.clone());
        } else if self.pickers_ready() {
            foot = foot.child(self.models.clone());
        }
        foot = foot.child(self.simplify_chip(window, cx));
        let can = self.can_start(cx) && !self.prompt_busy;
        let loop_mode = self.mode == ChatMode::Loop;
        let mut go = recur_ui::style::primary_button("nt-go", (!can).then_some(t.pane), cx)
            .debug_selector(move || format!("nt-go:{}", if can { "on" } else { "off" }))
            .relative()
            .track_focus(&self.go_focus);
        go = if self.starting {
            go.child(button_label(
                if loop_mode {
                    "Starting…"
                } else {
                    "Creating…"
                },
                13.,
                weight::SEMIBOLD,
            ))
        } else {
            go.child(button_label(
                if loop_mode {
                    "Start"
                } else {
                    "Create workspace"
                },
                13.,
                weight::SEMIBOLD,
            ))
            .child(
                recur_ui::kbd::kbd("⌘↵")
                    .size(12.)
                    .line_height(LineHeight::Normal)
                    .opacity(0.55),
            )
        };
        if can {
            go = go.on_click(cx.listener(|m, _, window, cx| m.submit(window, cx)));
        }
        if recur_ui::style::is_focus_visible(&self.go_focus, window) {
            go = go.child(outline(2., 2., 8., 0., t.amber));
        }
        foot = foot.child(go);

        let panel = modal_panel(cx)
            .w_full()
            .debug_selector(|| "nt-modal".into())
            .child(head)
            .child(body)
            .child(foot);
        let width = 640f32.min(f32::from(window.viewport_size().width) - 32.);
        let panel = with_shadow(panel, &MODAL_SHADOW, 16.)
            .id("nt-modal")
            .w(px(width))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        modal_scrim(window, panel)
            .id("v-newtask")
            .debug_selector(|| "v-newtask".into())
            .track_focus(&self.focus)
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|m, _, _, cx| m.close(cx)))
            .into_any_element()
    }
}

/// The window's New workspace modal, for specs and parity scenes.
pub struct CurrentNewTask(pub gpui_kit::WeakEntity<NewTaskModal>);

impl gpui_kit::Global for CurrentNewTask {}

impl NewTaskModal {
    pub fn current(cx: &App) -> Option<Entity<NewTaskModal>> {
        cx.try_global::<CurrentNewTask>()
            .and_then(|c| c.0.upgrade())
    }

    /// The composer's draft (`bind:input`).
    pub fn task(&self) -> &str {
        &self.task
    }

    pub fn chat(&self) -> &Entity<AgentChat> {
        &self.chat
    }
}

/// The overlay (`OverlaySlot::NewTask`).
pub fn build(vcx: &ViewContext, window: &mut Window, cx: &mut App) -> Entity<NewTaskModal> {
    let e = cx.new(|cx| NewTaskModal::new(vcx, window, cx));
    cx.set_global(CurrentNewTask(e.downgrade()));
    e
}
