//! `ScheduleEditor.svelte`: the New / Edit schedule modal, opened from anywhere through
//! `AppState::schedule_editor`. It shares New workspace's chrome; a save bumps
//! `schedulesVersion` so the lists refresh.

use recur_ui::guard::Guarded;
use std::time::Duration;

use gpui_kit::base::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, px,
    relative,
};
use recur_domain::{BranchChoice, ScheduledPrompt};
use recur_engine::view::loops::{
    LoopInfo, PolicyInfo, default_loop_id, default_policy_id, enabled_loops, enabled_policies,
    fallback_loops, fallback_policies,
};
use recur_ui::fonts::weight;
use recur_ui::shadow::with_shadow;
use recur_ui::style::{MODAL_SHADOW, button_label, modal_panel, modal_scrim, nt_esc_hint, outline};
use recur_ui::text::{LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::widgets::{PicBox, Select, SelectEvent, SelectOption, TriggerStyle, project_pic};
use recur_ui::{IconName, Theme, icon};
use recur_view_chat::pickers::{LoopPolicyEvent, LoopPolicyPickers, ModelPickers, PickerEvent};
use recur_view_core::format::format_medium_short_in;
use recur_view_core::models::{FlagMap, flags_for_model, normalize_flags};
use recur_view_core::modes::{
    CatalogModel, ChatMode, is_mode_default_selection, resolved_default_for_mode,
};
use recur_view_core::{ScheduleEditorRequest, ViewContext};

use crate::dialog::{cycle, window_keys};
use crate::new_task::{picker_trigger, simplify_chip};

const LOOP_PLACEHOLDER: &str = "What should the loop work on each run?";
const AGENT_PLACEHOLDER: &str =
    "What should the agent do each run? — e.g. review yesterday's commits for missing tests";
/// The preview's debounce.
const PREVIEW_DELAY: Duration = Duration::from_millis(250);

/// `scheduleName`: a new schedule is titled from the prompt's first non-blank line.
pub fn schedule_name(prompt: &str) -> String {
    prompt
        .split('\n')
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(60)
        .collect()
}

/// The runtime's IANA zone (`Intl.DateTimeFormat().resolvedOptions().timeZone`).
pub fn local_time_zone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

/// `timeZoneOptions`: the IANA zones plus UTC, the local zone and the draft's (so an edit
/// round-trips), sorted, labeled without underscores and grouped by region.
pub fn time_zone_options(draft_zone: &str, local: &str) -> Vec<(String, String, String)> {
    // simplify: `Intl.supportedValuesOf('timeZone')` lists canonical zones only; chrono-tz has
    // no canonical flag, so the region-prefixed names stand in (backward links such as
    // `US/Eastern` and `Etc/*` drop out). Upgrade: a canonical-zone table in recur_domain.
    const REGIONS: [&str; 10] = [
        "Africa",
        "America",
        "Antarctica",
        "Arctic",
        "Asia",
        "Atlantic",
        "Australia",
        "Europe",
        "Indian",
        "Pacific",
    ];
    let mut zones: std::collections::BTreeSet<String> = chrono_tz::TZ_VARIANTS
        .iter()
        .map(|z| z.name().to_string())
        .filter(|n| n.split_once('/').is_some_and(|(r, _)| REGIONS.contains(&r)))
        .collect();
    zones.insert("UTC".into());
    if !local.is_empty() {
        zones.insert(local.to_string());
    }
    if !draft_zone.is_empty() {
        zones.insert(draft_zone.to_string());
    }
    zones
        .into_iter()
        .map(|z| {
            let label = z.replace('_', " ");
            let group = match z.split_once('/') {
                Some((r, _)) => r.to_string(),
                None => "Other".into(),
            };
            (z, label, group)
        })
        .collect()
}

/// `nextLabel`.
pub fn next_label(d: &ScheduledPrompt, preview: &[i64], busy: bool, error: &str) -> String {
    if !error.is_empty() {
        return error
            .strip_prefix("Error:")
            .map(str::trim_start)
            .unwrap_or(error)
            .to_string();
    }
    if busy {
        return "Checking…".into();
    }
    if d.cadence == "cron" && d.cron.trim() == "@reboot" {
        return "Runs when Recur starts".into();
    }
    match preview.first() {
        Some(at) => format!("Next {}", format_medium_short_in(*at, &d.time_zone)),
        None => String::new(),
    }
}

/// `open(req)`: the draft for a request (a new schedule's defaults, or a copy of the edited
/// one with legacy daily / weekday cadences folded into cron).
pub fn draft_for(
    req: &ScheduleEditorRequest,
    projects: &[recur_domain::Project],
    settings: Option<&recur_domain::Settings>,
    models: &[CatalogModel],
    local_zone: &str,
) -> ScheduledPrompt {
    let mut d = match &req.schedule {
        Some(p) => p.clone(),
        None => {
            let target = req
                .project_id
                .as_ref()
                .and_then(|id| projects.iter().find(|p| &p.id == id))
                .or(projects.first());
            let defaults = resolved_default_for_mode("ask", settings, models);
            ScheduledPrompt {
                project_id: target.map(|p| p.id.clone()).unwrap_or_default(),
                base_branch: target.map(|p| p.base_branch.clone()).unwrap_or_default(),
                mode: "ask".into(),
                model: defaults.model,
                flags: defaults.flags,
                simplify: Some(settings.is_none_or(|s| s.simplify_enabled())),
                cadence: "cron".into(),
                cron: "0 9 * * MON-FRI".into(),
                loop_id: default_loop_id(settings),
                policy_id: default_policy_id(settings),
                time: "09:00".into(),
                time_zone: local_zone.to_string(),
                ..Default::default()
            }
        }
    };
    let flags: FlagMap = d.flags.clone();
    d.flags = normalize_flags(&flags_for_model(models, &d.model), Some(&flags));
    if d.simplify.is_none() {
        d.simplify = Some(settings.is_none_or(|s| s.simplify_enabled()));
    }
    if d.loop_id.is_empty() {
        d.loop_id = default_loop_id(settings);
    }
    if d.policy_id.is_empty() {
        d.policy_id = default_policy_id(settings);
    }
    if d.cadence == "daily" || d.cadence == "weekdays" {
        let mut hm = d.time.split(':').map(|x| x.parse::<i64>().unwrap_or(0));
        let (hour, minute) = (hm.next().unwrap_or(0), hm.next().unwrap_or(0));
        d.cron = format!(
            "{minute} {hour} * * {}",
            if d.cadence == "weekdays" {
                "MON-FRI"
            } else {
                "*"
            }
        );
        d.cadence = "cron".into();
    }
    d
}

pub struct ScheduleEditor {
    vcx: ViewContext,
    focus: FocusHandle,
    draft: Option<ScheduledPrompt>,
    busy: bool,
    error: String,
    cron_help: bool,
    preview: Vec<i64>,
    preview_error: String,
    preview_busy: bool,
    preview_key: Option<(String, String, String, String, String)>,
    preview_task: Option<Task<()>>,
    branches: Vec<BranchChoice>,
    branches_loading: bool,
    branch_key: Option<String>,
    branch_task: Option<Task<()>>,
    models: Vec<CatalogModel>,
    loops: Vec<LoopInfo>,
    policies: Vec<PolicyInfo>,
    seen_settings: Option<Option<recur_domain::Settings>>,
    local_zone: String,
    return_focus: Option<FocusHandle>,
    prompt: Entity<TextareaState>,
    cron: Entity<InputState>,
    date: Entity<InputState>,
    time: Entity<InputState>,
    repo_sel: Entity<Select>,
    branch_sel: Entity<Select>,
    repeat_sel: Entity<Select>,
    zone_sel: Entity<Select>,
    mode_sel: Entity<Select>,
    loop_pickers: Entity<LoopPolicyPickers>,
    model_pickers: Entity<ModelPickers>,
    help_focus: FocusHandle,
    simplify_focus: FocusHandle,
    go_focus: FocusHandle,
    hover_x: bool,
    hover_field: Option<&'static str>,
    prompt_ph: &'static str,
    _subs: Vec<Subscription>,
}

impl Focusable for ScheduleEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// `.sc-in`: a 24px borderless field, `--muted` until hover or focus fill it `--pane-2`.
fn sc_in(
    input: &Entity<InputState>,
    placeholder: &str,
    mono: bool,
    width: Option<f32>,
    hovered: bool,
    window: &Window,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    let lit = focused || hovered;
    let family = if mono {
        recur_ui::fonts::Family::Mono
    } else {
        recur_ui::fonts::Family::Ui
    };
    let field = div().flex().items_center().h(px(24.)).px(px(6.)).child(
        recur_ui::style::text_field(
            input,
            &SharedString::from(placeholder.to_string()),
            t.faint,
            family,
            12.,
            window,
            cx,
        )
        .text_color(Hsla::from(if lit { t.text } else { t.muted })),
    );
    let el = div()
        .id(SharedString::from(format!("sc-in-{placeholder}")))
        .relative()
        .flex()
        .flex_none()
        .h(px(24.))
        .rounded(px(6.))
        .when(lit, |d| d.bg(Hsla::from(t.pane_2)));
    match width {
        // A fractional width (`24ch`) stays exact: a leaf that long, the field over it.
        Some(w) => el
            .child(recur_ui::exact::hgap(w, window))
            .child(div().absolute().inset_0().child(field)),
        None => el.child(field),
    }
}

impl ScheduleEditor {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| TextareaState::new(window, cx));
        let field =
            |window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx));
        let cron = field(window, cx);
        let date = field(window, cx);
        let time = field(window, cx);
        let mk =
            |label: &'static str, searchable: bool, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| {
                    let s = Select::new(Vec::new(), window, cx)
                        .label(label)
                        .menu_variant(recur_ui::logic::menu_space::MenuVariant::Wide);
                    if searchable { s.searchable() } else { s }
                })
            };
        let repo_sel = mk("Repository", false, window, cx);
        let branch_sel = mk("Base branch", true, window, cx);
        branch_sel.update(cx, |s, cx| s.set_icon(Some(IconName::GitBranch), cx));
        let ghost = TriggerStyle {
            no_outline: true,
            kb_ring: true,
            ..Default::default()
        };
        let repeat_sel = cx.new(|cx| {
            Select::new(
                vec![
                    SelectOption::new("cron", "Cron")
                        .icon(IconName::RotateCwFadingClock)
                        .hint("Repeat on a cron expression"),
                    SelectOption::new("once", "Once")
                        .icon(IconName::ClockFading)
                        .hint("Run once at a date and time"),
                ],
                window,
                cx,
            )
            .label("Repeat")
            .ghost()
            .trigger_style(ghost.clone())
        });
        let zone_sel = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Time zone")
                .ghost()
                .searchable()
                .trigger_style(ghost.clone())
        });
        let mode_sel = cx.new(|cx| {
            Select::new(Vec::new(), window, cx)
                .label("Mode")
                .ghost()
                .up()
                .trigger_style(ghost.clone())
        });
        let loop_pickers = cx.new(|cx| LoopPolicyPickers::new(true, true, true, window, cx));
        loop_pickers.update(cx, |l, cx| l.set_kb_ring(true, cx));
        let model_pickers = cx.new(|cx| ModelPickers::new(true, true, window, cx));
        model_pickers.update(cx, |m, cx| m.set_trigger_style(ghost.clone(), cx));
        let subs = vec![
            window_keys(cx, Self::on_key),
            cx.observe_in(&vcx.state, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            cx.subscribe(&prompt, |this, area, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change)
                    && let Some(d) = this.draft.as_mut()
                {
                    d.prompt = area.read(cx).value().to_string();
                    cx.notify();
                }
            }),
            cx.subscribe(&cron, |this, f, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change)
                    && let Some(d) = this.draft.as_mut()
                {
                    d.cron = f.read(cx).value().to_string();
                    this.preview_effect(cx);
                }
            }),
            cx.subscribe(&date, |this, f, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change)
                    && let Some(d) = this.draft.as_mut()
                {
                    d.date = f.read(cx).value().to_string();
                    this.preview_effect(cx);
                }
            }),
            cx.subscribe(&time, |this, f, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change)
                    && let Some(d) = this.draft.as_mut()
                {
                    d.time = f.read(cx).value().to_string();
                    this.preview_effect(cx);
                }
            }),
            cx.subscribe(&repo_sel, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    this.choose_project(v, cx);
                }
            }),
            cx.subscribe(&branch_sel, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev
                    && let Some(d) = this.draft.as_mut()
                {
                    d.base_branch = v.to_string();
                    cx.notify();
                }
            }),
            cx.subscribe(&repeat_sel, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev
                    && let Some(d) = this.draft.as_mut()
                {
                    d.cadence = v.to_string();
                    this.preview_effect(cx);
                }
            }),
            cx.subscribe(&zone_sel, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev
                    && let Some(d) = this.draft.as_mut()
                {
                    d.time_zone = v.to_string();
                    this.preview_effect(cx);
                }
            }),
            cx.subscribe(&mode_sel, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(v) = ev {
                    this.pick_mode(ChatMode::from_key(v), cx);
                }
            }),
            cx.subscribe(&loop_pickers, |this, _, ev: &LoopPolicyEvent, cx| {
                if let Some(d) = this.draft.as_mut() {
                    match ev {
                        LoopPolicyEvent::Loop(id) => d.loop_id = id.clone(),
                        LoopPolicyEvent::Policy(id) => d.policy_id = id.clone(),
                    }
                }
                cx.notify();
            }),
            cx.subscribe(&model_pickers, |this, _, ev: &PickerEvent, cx| {
                if let Some(d) = this.draft.as_mut() {
                    match ev {
                        PickerEvent::Model(r, f) => {
                            d.model = r.clone();
                            d.flags = f.clone();
                        }
                        PickerEvent::Flags(f) => d.flags = f.clone(),
                    }
                }
                cx.notify();
            }),
        ];
        let models = vcx.state.read(cx).peek_model_catalog().to_vec();
        let mut me = ScheduleEditor {
            vcx: vcx.clone(),
            focus: cx.focus_handle(),
            draft: None,
            busy: false,
            error: String::new(),
            cron_help: false,
            preview: Vec::new(),
            preview_error: String::new(),
            preview_busy: false,
            preview_key: None,
            preview_task: None,
            branches: Vec::new(),
            branches_loading: false,
            branch_key: None,
            branch_task: None,
            models,
            loops: fallback_loops(),
            policies: fallback_policies(),
            seen_settings: None,
            local_zone: local_time_zone(),
            return_focus: None,
            prompt,
            cron,
            date,
            time,
            repo_sel,
            branch_sel,
            repeat_sel,
            zone_sel,
            mode_sel,
            loop_pickers,
            model_pickers,
            help_focus: cx.focus_handle().tab_stop(true),
            simplify_focus: cx.focus_handle().tab_stop(true),
            go_focus: cx.focus_handle().tab_stop(true),
            hover_x: false,
            hover_field: None,
            prompt_ph: "",
            _subs: subs,
        };
        me.sync(window, cx);
        me
    }

    fn hover(&mut self, on: bool, name: &'static str, cx: &mut Context<Self>) {
        if on {
            self.hover_field = Some(name);
        } else if self.hover_field == Some(name) {
            self.hover_field = None;
        }
        cx.notify();
    }

    pub fn draft(&self) -> Option<&ScheduledPrompt> {
        self.draft.as_ref()
    }

    pub fn is_open(&self) -> bool {
        self.draft.is_some()
    }

    // ---- effects ----

    /// The request effect (open on `scheduleEditor`), the settings effect (refresh catalogs).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let req = self.vcx.state.read(cx).schedule_editor.clone();
        if let Some(req) = req {
            self.vcx
                .state
                .update(cx, |s, cx| s.set_schedule_editor(None, cx));
            self.open(req, window, cx);
            return;
        }
        if self.draft.is_some() {
            let settings = self.vcx.state.read(cx).settings.clone();
            if self.seen_settings.as_ref() != Some(&settings) {
                self.seen_settings = Some(settings);
                self.refresh_catalogs(cx);
            }
        }
    }

    /// `refreshCatalogs`: loops and policies, then the model catalog (normalizing the draft's
    /// flags against it).
    fn refresh_catalogs(&mut self, cx: &mut Context<Self>) {
        let svc = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let loops = svc.bundle.list_loops().await.unwrap_or_default();
            let policies = svc.bundle.list_policies().await.unwrap_or_default();
            let _ = this.update(cx, |m, cx| {
                if !loops.is_empty() {
                    m.loops = loops
                        .into_iter()
                        .map(|l| LoopInfo {
                            id: l.id,
                            name: l.name,
                            hint: l.hint,
                        })
                        .collect();
                }
                if !policies.is_empty() {
                    m.policies = policies
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
        let load = self.vcx.state.update(cx, |s, cx| s.load_model_catalog(cx));
        cx.spawn(async move |this, cx| {
            let models = load.await;
            let _ = this.update(cx, |m, cx| {
                if let Some(d) = m.draft.as_mut() {
                    let flags: FlagMap = d.flags.clone();
                    d.flags = normalize_flags(&flags_for_model(&models, &d.model), Some(&flags));
                }
                m.models = models;
                cx.notify();
            });
        })
        .detach();
    }

    fn branches_effect(&mut self, cx: &mut Context<Self>) {
        let id = self
            .draft
            .as_ref()
            .map(|d| d.project_id.clone())
            .unwrap_or_default();
        if id.is_empty() || self.branch_key.as_deref() == Some(id.as_str()) {
            return;
        }
        self.branch_key = Some(id.clone());
        self.branches.clear();
        self.branches_loading = true;
        let services = self.vcx.services.clone();
        self.branch_task = Some(cx.spawn(async move |this, cx| {
            let r = services.projects.list_branches(&id).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(rows) => m.branches = rows,
                    Err(e) => m.error = format!("Error: {}", e.message),
                }
                m.branches_loading = false;
                cx.notify();
            });
        }));
    }

    /// The preview effect: the next runs, 250ms after the timing settles.
    fn preview_effect(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.draft.as_ref() else {
            return;
        };
        let key = (
            d.cadence.clone(),
            d.cron.clone(),
            d.date.clone(),
            d.time.clone(),
            d.time_zone.clone(),
        );
        if self.preview_key.as_ref() == Some(&key) {
            return;
        }
        self.preview_key = Some(key);
        let timing = ScheduledPrompt {
            cadence: d.cadence.clone(),
            cron: d.cron.clone(),
            date: d.date.clone(),
            time: d.time.clone(),
            time_zone: d.time_zone.clone(),
            ..Default::default()
        };
        self.preview.clear();
        self.preview_error.clear();
        self.preview_busy = true;
        cx.notify();
        let services = self.vcx.services.clone();
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DELAY).await;
            let r = services.projects.preview_scheduled_prompt(&timing).await;
            let _ = this.update(cx, |m, cx| {
                match r {
                    Ok(times) => m.preview = times,
                    Err(e) => m.preview_error = format!("Error: {}", e.message),
                }
                m.preview_busy = false;
                cx.notify();
            });
        }));
    }

    // ---- actions ----

    fn open(&mut self, req: ScheduleEditorRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.return_focus = window.focused(cx);
        self.error.clear();
        self.cron_help = false;
        let (projects, settings) = {
            let s = self.vcx.state.read(cx);
            (s.projects.clone(), s.settings.clone())
        };
        let d = draft_for(
            &req,
            &projects,
            settings.as_ref(),
            &self.models,
            &self.local_zone,
        );
        let (prompt, cron, date, time) = (
            d.prompt.clone(),
            d.cron.clone(),
            d.date.clone(),
            d.time.clone(),
        );
        self.draft = Some(d);
        self.prompt
            .update(cx, |s, cx| s.set_value(prompt, window, cx));
        self.cron.update(cx, |s, cx| s.set_value(cron, window, cx));
        self.date.update(cx, |s, cx| s.set_value(date, window, cx));
        self.time.update(cx, |s, cx| s.set_value(time, window, cx));
        self.branch_key = None;
        self.preview_key = None;
        self.seen_settings = Some(settings);
        self.refresh_catalogs(cx);
        self.branches_effect(cx);
        self.preview_effect(cx);
        let h = self.prompt.read(cx).focus_handle(cx);
        window.defer(cx, move |window, cx| window.focus(&h, cx));
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.finish(window, cx);
    }

    /// The draft is gone: focus goes back where it was before the modal opened.
    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = None;
        self.preview_task = None;
        self.branch_task = None;
        match self.return_focus.take() {
            Some(h) => window.focus(&h, cx),
            None => window.blur(cx),
        }
        cx.notify();
    }

    fn choose_project(&mut self, id: &str, cx: &mut Context<Self>) {
        let base = self
            .vcx
            .state
            .read(cx)
            .project(id)
            .map(|p| p.base_branch.clone())
            .unwrap_or_default();
        let Some(d) = self.draft.as_mut() else {
            return;
        };
        if !d.id.is_empty() {
            return;
        }
        d.project_id = id.to_string();
        d.base_branch = base;
        self.branches_effect(cx);
        cx.notify();
    }

    /// `pickMode`: follow the new mode's default model only while on the old mode's default.
    fn pick_mode(&mut self, m: ChatMode, cx: &mut Context<Self>) {
        let settings = self.vcx.state.read(cx).settings.clone();
        let models = self.models.clone();
        let Some(d) = self.draft.as_mut() else {
            return;
        };
        let flags: FlagMap = d.flags.clone();
        if is_mode_default_selection(
            ChatMode::from_key(&d.mode).key(),
            &d.model,
            &flags,
            settings.as_ref(),
            &models,
        ) {
            let def = resolved_default_for_mode(m.key(), settings.as_ref(), &models);
            d.model = def.model;
            d.flags = def.flags;
        }
        d.mode = m.key().to_string();
        cx.notify();
    }

    pub fn can_save(&self) -> bool {
        let Some(d) = &self.draft else {
            return false;
        };
        !d.prompt.trim().is_empty()
            && !d.model.is_empty()
            && !d.base_branch.is_empty()
            && !self.branches_loading
            && !self.preview_busy
            && self.preview_error.is_empty()
            && (d.mode != "loop" || (!d.loop_id.is_empty() && !d.policy_id.is_empty()))
    }

    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_save() || self.busy {
            return;
        }
        let Some(d) = &self.draft else {
            return;
        };
        let mut p = d.clone();
        if p.name.trim().is_empty() {
            p.name = schedule_name(&p.prompt);
        } else {
            p.name = p.name.trim().to_string();
        }
        self.busy = true;
        self.error.clear();
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = services.projects.save_scheduled_prompt(&p).await;
            let _ = this.update_in(cx, |m, window, cx| {
                m.busy = false;
                match r {
                    Ok(_) => {
                        m.vcx.state.update(cx, |s, cx| s.bump_schedules_version(cx));
                        m.finish(window, cx);
                    }
                    Err(e) => m.error = format!("Error: {}", e.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn prompt(&self) -> &Entity<TextareaState> {
        &self.prompt
    }

    /// The dialog's Tab stops in DOM order (disabled controls and `tabindex=-1` skipped).
    pub fn stops(&self, cx: &App) -> Vec<FocusHandle> {
        let Some(d) = &self.draft else {
            return Vec::new();
        };
        let mut v = Vec::new();
        if d.id.is_empty() {
            v.push(self.repo_sel.focus_handle(cx));
        }
        if !self.branches_loading && !self.branches.is_empty() {
            v.push(self.branch_sel.focus_handle(cx));
        }
        v.push(self.prompt.read(cx).focus_handle(cx));
        v.push(self.repeat_sel.focus_handle(cx));
        if d.cadence == "once" {
            v.push(self.date.read(cx).focus_handle(cx));
            v.push(self.time.read(cx).focus_handle(cx));
        } else {
            v.push(self.cron.read(cx).focus_handle(cx));
            v.push(self.help_focus.clone());
        }
        v.push(self.zone_sel.focus_handle(cx));
        v.push(self.mode_sel.focus_handle(cx));
        if d.mode == "loop" {
            v.extend(self.loop_pickers.read(cx).focus_handles(cx));
        } else {
            v.extend(self.model_pickers.read(cx).focus_handles(cx));
        }
        v.push(self.simplify_focus.clone());
        if self.can_save() && !self.busy {
            v.push(self.go_focus.clone());
        }
        v
    }

    fn any_select_open(&self, cx: &App) -> bool {
        [
            &self.repo_sel,
            &self.branch_sel,
            &self.repeat_sel,
            &self.zone_sel,
            &self.mode_sel,
        ]
        .iter()
        .any(|s| s.read(cx).is_open())
            || self.loop_pickers.read(cx).is_open(cx)
            || self.model_pickers.read(cx).is_open(cx)
    }

    /// The window keydown listener (capture): ⌘. cycles the mode, Escape closes an open menu,
    /// then the cron help, then the modal; Tab cycles the dialog; ⌘↵ in the prompt saves.
    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.draft else {
            return;
        };
        let m = &ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        let cmd = m.platform || m.control;
        if cmd && !m.shift && !m.alt && key == "." {
            cx.stop_propagation();
            let next = ChatMode::from_key(&d.mode).cycle(false, false);
            self.pick_mode(next, cx);
            return;
        }
        if key == "escape" {
            if self.any_select_open(cx) {
                return;
            }
            cx.stop_propagation();
            if self.cron_help {
                self.cron_help = false;
                cx.notify();
            } else {
                self.close(window, cx);
            }
            return;
        }
        if key == "tab" && !m.platform && !m.control && !m.alt {
            if self.any_select_open(cx) {
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
        if key == "enter" && cmd && self.prompt.read(cx).focus_handle(cx).is_focused(window) {
            cx.stop_propagation();
            self.submit(window, cx);
        }
    }

    fn sync_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.draft.clone() else {
            return;
        };
        let t = Theme::of(cx).tokens;
        let vw = f32::from(window.viewport_size().width);
        let (projects, settings) = {
            let s = self.vcx.state.read(cx);
            (s.projects.clone(), s.settings.clone())
        };
        let repo_opts: Vec<SelectOption> = projects
            .iter()
            .map(|p| SelectOption::new(p.id.clone(), p.name.clone()))
            .collect();
        let project = projects.iter().find(|p| p.id == d.project_id).cloned();
        let slot: recur_ui::widgets::select::IconSlot = match project {
            Some(r) => std::rc::Rc::new(move |_, _| {
                project_pic(r.pic.clone(), r.color.clone(), r.name.clone(), PicBox::NT)
                    .into_any_element()
            }),
            None => {
                std::rc::Rc::new(|_, _| icon(IconName::Folder).size(px(14.)).into_any_element())
            }
        };
        let trig = picker_trigger(&t, vw);
        let editing = !d.id.is_empty();
        let pid = d.project_id.clone();
        self.repo_sel.update(cx, |s, cx| {
            s.set_options(repo_opts, cx);
            if s.value().as_ref() != pid {
                s.set_value(pid, cx);
            }
            s.set_icon_slot(Some(slot), cx);
            s.set_disabled(editing.then_some(t.pane), cx);
            if s.trigger() != &trig {
                s.set_trigger_style(trig.clone(), cx);
            }
        });
        let branch_opts: Vec<SelectOption> = self
            .branches
            .iter()
            .map(|b| SelectOption::new(b.name.clone(), b.name.clone()).group(b.group.clone()))
            .collect();
        let disabled = self.branches_loading || branch_opts.is_empty();
        let base = d.base_branch.clone();
        self.branch_sel.update(cx, |s, cx| {
            s.set_options(branch_opts, cx);
            if s.value().as_ref() != base {
                s.set_value(base, cx);
            }
            s.set_disabled(disabled.then_some(t.pane), cx);
            if s.trigger() != &trig {
                s.set_trigger_style(trig, cx);
            }
        });
        let cadence = d.cadence.clone();
        self.repeat_sel.update(cx, |s, cx| {
            if s.value().as_ref() != cadence {
                s.set_value(cadence, cx);
            }
        });
        let zones: Vec<SelectOption> = time_zone_options(&d.time_zone, &self.local_zone)
            .into_iter()
            .map(|(v, l, g)| SelectOption::new(v, l).group(g))
            .collect();
        let zone = d.time_zone.clone();
        self.zone_sel.update(cx, |s, cx| {
            s.set_options(zones, cx);
            if s.value().as_ref() != zone {
                s.set_value(zone, cx);
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
        let mode = ChatMode::from_key(&d.mode).key();
        self.mode_sel.update(cx, |s, cx| {
            s.set_options(mode_opts, cx);
            if s.value() != mode {
                s.set_value(mode, cx);
            }
        });
        if d.mode == "loop" {
            let loops = enabled_loops(&self.loops, settings.as_ref());
            let policies = enabled_policies(&self.policies, settings.as_ref());
            let (l, p) = (d.loop_id.clone(), d.policy_id.clone());
            self.loop_pickers.update(cx, |lp, cx| {
                lp.set(&l, &p, &loops, &policies, false, t.pane, cx)
            });
        } else {
            let models = self.models.clone();
            let flags: FlagMap = d.flags.clone();
            let model = d.model.clone();
            self.model_pickers
                .update(cx, |mp, cx| mp.set(models, &model, &flags, None, cx));
        }
        let ph = if d.mode == "loop" {
            LOOP_PLACEHOLDER
        } else {
            AGENT_PLACEHOLDER
        };
        let prompt_ph = std::mem::replace(&mut self.prompt_ph, ph);
        self.prompt.update(cx, |s, cx| {
            if prompt_ph != ph {
                s.set_placeholder(ph, window, cx);
            }
        });
        recur_ui::style::style_textarea(&self.prompt, t.faint, cx);
        // `.sc-in` is `--muted` until hovered or focused.
        for (name, f) in [
            ("cron", &self.cron),
            ("date", &self.date),
            ("time", &self.time),
        ] {
            let lit =
                self.hover_field == Some(name) || f.read(cx).focus_handle(cx).is_focused(window);
            let fg = if lit { t.text } else { t.muted };
            recur_ui::style::style_text_input_fg(f, fg, cx);
        }
    }

    /// `.sc-tip`: the cron format help, 8px under the modal.
    fn cron_tip(&self, window: &Window, cx: &App) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let c = |s: &str| -> Span { span(s.to_string()).mono().size(11.).color(t.text) };
        let para = Text::from_spans(vec![
            span("Cron").weight(600.).color(t.text),
            span(" is "),
            c("minute hour day month weekday"),
            span("; add a leading field for seconds. "),
            c("*"),
            span(" any, "),
            c(","),
            span(" list, "),
            c("-"),
            span(" range, "),
            c("/"),
            span(" step, names like "),
            c("MON"),
            span(" or "),
            c("JAN"),
            span("."),
        ])
        .id("sc-tip-p")
        .size(12.)
        .leading(1.45)
        .color(t.muted);
        let items: Vec<Vec<Span>> = vec![
            vec![
                c("*/15 9-17 * * MON-FRI"),
                span(" every 15 min, weekday hours"),
            ],
            vec![c("0 9,17 * * *"), span(" daily at 9am and 5pm")],
            vec![c("0 9 1,15 * *"), span(" 1st and 15th at 9am")],
            vec![
                c("@hourly"),
                span(" "),
                c("@daily"),
                span(" "),
                c("@weekly"),
                span(" "),
                c("@monthly"),
            ],
            vec![c("@every 2h30m"), span(" fixed interval")],
            vec![c("@reboot"), span(" when Recur starts")],
        ];
        let mut list = div().mt(px(8.)).pl(px(14.)).flex().flex_col();
        for (i, spans) in items.into_iter().enumerate() {
            list = list.child(
                div()
                    .mt(px(3.))
                    .relative()
                    .child(recur_content::render::block::disc_marker(
                        12.,
                        12. * 1.45,
                        t.muted,
                        window,
                    ))
                    .child(
                        Text::from_spans(spans)
                            .id(SharedString::from(format!("sc-tip-li{i}")))
                            .size(12.)
                            .leading(1.45)
                            .color(t.muted),
                    ),
            );
        }
        let card = recur_ui::style::card(t.pane_2, t.line_bright, 9.)
            .w(px(340.))
            .py(px(11.))
            .px(px(12.))
            .child(para)
            .child(list);
        div()
            .debug_selector(|| "sc-tip".into())
            .absolute()
            .top(relative(1.))
            .mt(px(8.))
            .left(px(14.))
            .child(with_shadow(card, &recur_ui::style::POPOVER_SHADOW, 9.))
            .into_any_element()
    }
}

impl ScheduleEditor {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let Some(d) = self.draft.clone() else {
            return div().into_any_element();
        };
        let t = Theme::of(cx).tokens;
        self.sync_selects(window, cx);
        let head = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .pt(px(12.))
            .px(px(14.))
            .min_h(px(44.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .flex_1()
                    .child(div().flex_none().child(self.repo_sel.clone()))
                    .child(div().flex_none().child(self.branch_sel.clone())),
            )
            .child(
                div()
                    .id("sc-x")
                    .debug_selector(|| "sc-x".into())
                    .ml_auto()
                    .flex_none()
                    .cursor_pointer()
                    .on_hover(cx.guarded_listener(|m, h: &bool, _, cx| {
                        m.hover_x = *h;
                        cx.notify();
                    }))
                    .on_click(cx.guarded_listener(|m, _, window, cx| m.close(window, cx)))
                    .child(nt_esc_hint(self.hover_x, window, cx)),
            );

        // `.nt-in`: 14px / 1.55 in 18px 18px 12px padding, 148px tall.
        let body = div().relative().flex().flex_col().min_w_0().child(
            div()
                .debug_selector(|| "sc-prompt".into())
                .h(px(148.))
                .pt(px(18.))
                .px(px(18.))
                .pb(px(12.))
                .text_size(px(14.))
                .line_height(relative(1.55))
                .text_color(Hsla::from(t.text))
                .child(Textarea::new(&self.prompt)),
        );

        let mut when = div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .pt(px(10.))
            .px(px(14.))
            .min_w_0()
            .child(div().flex_none().child(self.repeat_sel.clone()));
        if d.cadence == "once" {
            // simplify: WebKit's `type=date` / `type=time` segmented pickers are plain
            // `YYYY-MM-DD` / `HH:MM` fields. Upgrade: a date / time field in recur_ui.
            when = when
                .child(
                    sc_in(
                        &self.date,
                        "YYYY-MM-DD",
                        false,
                        None,
                        self.hover_field == Some("date"),
                        window,
                        cx,
                    )
                    .debug_selector(|| "inp:Date".into())
                    .on_hover(cx.guarded_listener(|m, h: &bool, _, cx| m.hover(*h, "date", cx))),
                )
                .child(
                    sc_in(
                        &self.time,
                        "HH:MM",
                        false,
                        None,
                        self.hover_field == Some("time"),
                        window,
                        cx,
                    )
                    .debug_selector(|| "inp:Time".into())
                    .on_hover(cx.guarded_listener(|m, h: &bool, _, cx| m.hover(*h, "time", cx))),
                );
        } else {
            // `.sc-cron { width: 24ch }` in the mono face, padding included (`border-box`).
            let ch = recur_ui::font_metrics::zero_advance(&recur_ui::fonts::Family::Mono, 12.);
            when = when.child(
                sc_in(
                    &self.cron,
                    "0 9 * * MON-FRI",
                    true,
                    Some(24. * ch),
                    self.hover_field == Some("cron"),
                    window,
                    cx,
                )
                .debug_selector(|| "inp:Cron expression".into())
                .on_hover(cx.guarded_listener(|m, h: &bool, _, cx| m.hover(*h, "cron", cx))),
            );
            let on = self.cron_help;
            let ring = recur_ui::style::is_focus_visible(&self.help_focus, window);
            let lit = on || ring;
            when = when.child(
                div()
                    .id("sc-help")
                    .debug_selector(move || format!("sc-help:{on}"))
                    .track_focus(&self.help_focus)
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_color(Hsla::from(if lit { t.muted } else { t.faint }))
                    .when(lit, |d| d.bg(Hsla::from(t.pane_2)))
                    .hover(move |s| s.bg(Hsla::from(t.pane_2)).text_color(Hsla::from(t.muted)))
                    .on_click(cx.guarded_listener(|m, _, _, cx| {
                        m.cron_help = !m.cron_help;
                        cx.notify();
                    }))
                    .child(icon(IconName::Info).size(px(14.))),
            );
        }
        when = when.child(div().flex_none().child(self.zone_sel.clone()));
        let next = next_label(&d, &self.preview, self.preview_busy, &self.preview_error);
        let err = !self.preview_error.is_empty();
        when = when.child(
            div()
                .debug_selector({
                    let n = next.clone();
                    move || format!("sc-preview:{}={n}", if err { "alert" } else { "status" })
                })
                .ml_auto()
                .min_w_0()
                .child(
                    text(next)
                        .id("sc-preview")
                        .mono()
                        .size(10.)
                        .leading(1.45)
                        .tabular()
                        .color(if err { t.coral } else { t.faint })
                        .align(recur_ui::text::Align::Right)
                        .nowrap(),
                ),
        );

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
        if d.mode == "loop" {
            foot = foot.child(self.loop_pickers.clone());
        } else {
            foot = foot.child(self.model_pickers.clone());
        }
        let on = d.simplify.unwrap_or(true);
        let toggle = cx.guarded_listener(|m: &mut Self, _: &gpui_kit::ClickEvent, _, cx| {
            if let Some(d) = m.draft.as_mut() {
                d.simplify = Some(!d.simplify.unwrap_or(true));
            }
            cx.notify();
        });
        foot = foot.child(simplify_chip(
            "sc-simplify",
            on,
            &self.simplify_focus,
            toggle,
            window,
            cx,
        ));
        if !self.error.is_empty() {
            let e = self.error.clone();
            foot = foot.child(
                div()
                    .debug_selector({
                        let e = e.clone();
                        move || format!("sc-preview:alert={e}")
                    })
                    .ml_auto()
                    .min_w_0()
                    .child(
                        text(e)
                            .id("sc-error")
                            .mono()
                            .size(10.)
                            .leading(1.45)
                            .color(t.coral)
                            .nowrap(),
                    ),
            );
        }
        let can = self.can_save() && !self.busy;
        let editing = !d.id.is_empty();
        let mut go = recur_ui::style::primary_button("sc-go", (!can).then_some(t.pane), cx)
            .debug_selector(move || format!("sc-go:{}", if can { "on" } else { "off" }))
            .relative()
            .track_focus(&self.go_focus);
        go = if self.busy {
            go.child(button_label("Saving…", 13., weight::SEMIBOLD))
        } else {
            go.child(button_label(
                if editing { "Save schedule" } else { "Schedule" },
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
            go = go.on_click(cx.guarded_listener(|m, _, window, cx| m.submit(window, cx)));
        }
        if recur_ui::style::is_focus_visible(&self.go_focus, window) {
            go = go.child(outline(2., 2., 8., 0., t.amber));
        }
        foot = foot.child(go);

        let mut panel = modal_panel(cx)
            .relative()
            .w_full()
            .debug_selector(move || {
                format!(
                    "sc-modal:{}",
                    if editing {
                        "Edit schedule"
                    } else {
                        "New schedule"
                    }
                )
            })
            .child(head)
            .child(body)
            .child(when)
            .child(foot);
        if self.cron_help {
            panel = panel.child(self.cron_tip(window, cx));
        }
        let width = 640f32.min(f32::from(window.viewport_size().width) - 32.);
        let panel = with_shadow(panel, &MODAL_SHADOW, 16.)
            .id("sc-modal")
            .w(px(width))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        modal_scrim(window, panel)
            .id("v-schedule-editor")
            .debug_selector(|| "v-schedule-editor".into())
            .track_focus(&self.focus)
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.guarded_listener(|m, _, window, cx| m.close(window, cx)),
            )
            .into_any_element()
    }
}

impl Render for ScheduleEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The window's schedule editor, for specs and parity scenes.
pub struct CurrentScheduleEditor(pub gpui_kit::WeakEntity<ScheduleEditor>);

impl gpui_kit::Global for CurrentScheduleEditor {}

impl ScheduleEditor {
    pub fn current(cx: &App) -> Option<Entity<ScheduleEditor>> {
        cx.try_global::<CurrentScheduleEditor>()
            .and_then(|c| c.0.upgrade())
    }
}

/// The overlay (`OverlaySlot::ScheduleEditor`).
pub fn build(vcx: &ViewContext, window: &mut Window, cx: &mut App) -> Entity<ScheduleEditor> {
    let e = cx.new(|cx| ScheduleEditor::new(vcx, window, cx));
    cx.set_global(CurrentScheduleEditor(e.downgrade()));
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_first_line() {
        assert_eq!(
            schedule_name("\n  New global schedule \nCheck"),
            "New global schedule"
        );
        assert_eq!(schedule_name(""), "");
        assert_eq!(schedule_name(&"x".repeat(80)).len(), 60);
    }

    #[test]
    fn zones_are_grouped_and_keep_the_draft() {
        let z = time_zone_options("Mars/Base", "America/New_York");
        assert!(
            z.iter().any(|(v, l, g)| v == "America/New_York"
                && l == "America/New York"
                && g == "America")
        );
        assert!(z.iter().any(|(v, _, g)| v == "UTC" && g == "Other"));
        assert!(z.iter().any(|(v, _, _)| v == "Mars/Base"));
        assert!(!z.iter().any(|(v, _, _)| v.starts_with("US/")));
        let mut sorted = z.clone();
        sorted.sort();
        assert_eq!(z, sorted);
    }

    #[test]
    fn next_label_reports_errors_and_reboot() {
        let mut d = ScheduledPrompt {
            cadence: "cron".into(),
            cron: "@reboot".into(),
            time_zone: "UTC".into(),
            ..Default::default()
        };
        assert_eq!(
            next_label(&d, &[], false, "Error: Invalid cron expression"),
            "Invalid cron expression"
        );
        assert_eq!(next_label(&d, &[], true, ""), "Checking…");
        assert_eq!(next_label(&d, &[], false, ""), "Runs when Recur starts");
        d.cron = "0 9 * * *".into();
        assert_eq!(
            next_label(&d, &[1_792_595_040_000], false, ""),
            "Next Oct 21, 2026 at 3:04 PM"
        );
    }

    #[test]
    fn legacy_cadences_fold_into_cron() {
        let req = ScheduleEditorRequest {
            project_id: None,
            schedule: Some(ScheduledPrompt {
                id: "s".into(),
                cadence: "weekdays".into(),
                time: "07:30".into(),
                ..Default::default()
            }),
        };
        let d = draft_for(&req, &[], None, &[], "UTC");
        assert_eq!(
            (d.cadence.as_str(), d.cron.as_str()),
            ("cron", "30 7 * * MON-FRI")
        );
        let fresh = draft_for(
            &ScheduleEditorRequest::default(),
            &[],
            None,
            &[],
            "Europe/Paris",
        );
        assert_eq!(fresh.cron, "0 9 * * MON-FRI");
        assert_eq!(fresh.time_zone, "Europe/Paris");
        assert_eq!(fresh.mode, "ask");
    }
}
