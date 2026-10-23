//! The workspace titlebar's git chrome (App.svelte): Commit / Update / Merge for a
//! hand-driven workspace, Commit / Update / Push for the main working copy, the GitHub PR
//! actions (Open PR, Pull / Push / Force push, View PR), and on PR detail View / Create
//! workspace. Land state is `WorkspaceLandState`, recalled from a per-workspace snapshot so
//! the bar keeps its width while git answers.

use std::time::Duration;

use gpui_kit::{
    AnyElement, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, deferred, div,
    prelude::FluentBuilder, px,
};
use recur_domain::{Pr, PrSource, Workspace, WorkspaceLandState};
use recur_services::AppEvent;
use recur_ui::fonts::weight;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::{self, Rbtn, Tbtn, TbtnSize, rbtn, tbtn};
use recur_ui::text::{TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Select, SelectEvent, SelectOption, ToastKind, TriggerStyle};
use recur_ui::{IconName, Theme, color::rgba, icon};
pub use recur_view_core::land::{Land, LandCache};
use recur_view_core::modes::{ws_main_blocked, ws_run_active};
use recur_view_core::pr::{PrLifecycle, pr_for_workspace, pr_icon_name, pr_lifecycle};
use recur_view_core::{AppState, ViewName, call, toast};

use crate::chrome::no_drag;
use crate::shell::Shell;

/// A `github.branch_rules` answer that may land while git is still being asked.
#[derive(Clone, Debug)]
struct RulesEvt {
    project_id: String,
    remote: String,
    branch: String,
    require_pr: bool,
    at: u64,
}

const LAND_FS_DEBOUNCE: Duration = Duration::from_millis(50);

/// The chrome's state on the shell.
#[derive(Default)]
pub struct LandChrome {
    pub land: Land,
    seq: u64,
    clock: u64,
    workspace_id: String,
    /// False until WorkspaceLandState settles for the on-screen workspace.
    known: bool,
    updating_base: bool,
    pulling_pr: bool,
    pushing_remote: bool,
    rules_evt: Option<RulesEvt>,
    fs_timer: Option<Task<()>>,
    fetch: Option<Task<()>>,
    fetch_key: String,
    refresh_key: Option<RefreshKey>,
    merge_method_override: Option<String>,
    update_method_override: Option<String>,
    confirming_merge: bool,
    merge_menu_open: bool,
    update_menu_open: bool,
    open_pr_menu_open: bool,
    menus_for: String,
    pending_pr_create: Option<(String, String)>,
    base_select: Option<(Entity<Select>, Subscription)>,
    base_branches: Vec<recur_domain::BranchChoice>,
    base_loaded_for: Option<String>,
    base_loading_key: String,
    base_saving: bool,
    /// The issue picker and the `(split, label)` it was built for.
    issue_select: Option<(Entity<Select>, Subscription, (bool, &'static str))>,
    tracker_issues: Vec<recur_services::api::AssignedIssue>,
    tracker_fetched_at: Option<u64>,
}

/// What the land refresh effect depends on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RefreshKey {
    id: String,
    workspace_mode: bool,
    main_mode: bool,
    diff: (i64, i64),
    behind_base: i64,
    state: String,
    blocked: bool,
}

const MERGE_METHOD_KEYS: [&str; 3] = ["merge", "squash", "rebase"];
const UPDATE_METHOD_KEYS: [&str; 2] = ["merge", "rebase"];

fn merge_method_label(m: &str) -> Option<&'static str> {
    match m {
        "merge" => Some("Merge"),
        "squash" => Some("Squash and merge"),
        "rebase" => Some("Rebase and merge"),
        _ => None,
    }
}

fn update_method_menu_label(m: &str) -> &'static str {
    if m == "rebase" { "Rebase" } else { "Merge" }
}

/// Everything App.svelte derived for the detail titlebar.
pub struct LandView {
    pub workspace_mode: bool,
    pub main_mode: bool,
    pub land_on: bool,
    pub show_commit: bool,
    pub show_update: bool,
    pub show_push: bool,
    pub show_merge: bool,
    pub agent_busy: bool,
    pub land_busy: bool,
    pub land_busy_title: String,
    pub github_prs: bool,
    pub github_pr_open: bool,
    pub show_open_github_pr: bool,
    pub can_pull_pr: bool,
    pub can_push_pr: bool,
    pub can_force_push_pr: bool,
    pub merge_base_label: String,
    pub update_label: String,
    pub update_method: String,
    pub merge_method: String,
    pub merge_label: String,
    pub merge_hint: String,
    pub view_pr_id: String,
    pub view_pr_number: i64,
    pub view_pr_draft: bool,
    pub show_view_pr: bool,
    pub store_pr: Option<Pr>,
}

fn is_workspace_mode(s: &AppState, ws: Option<&Workspace>) -> bool {
    s.view == ViewName::Detail
        && ws.is_some_and(|w| {
            !w.is_main
                && w.host_pr_id.trim().is_empty()
                && w.state.as_str() != "merged"
                && w.state.as_str() != "pr_open"
                && !ws_run_active(Some(w))
        })
}

fn is_main_mode(s: &AppState, ws: Option<&Workspace>) -> bool {
    s.view == ViewName::Detail && ws.is_some_and(|w| w.is_main)
}

impl LandChrome {
    /// App.svelte's derived values for the selected workspace.
    pub fn view(&self, s: &AppState) -> LandView {
        let ws = s.selected_workspace();
        let id = s.selected_workspace_id.as_str();
        let land = &self.land;
        let workspace_mode = is_workspace_mode(s, ws);
        let main_mode = is_main_mode(s, ws);
        let blocked = ws_main_blocked(ws);
        let land_on = (workspace_mode || main_mode) && !blocked;
        let show_commit = land_on && land.dirty && !land.unmerged;
        let show_update = if main_mode {
            !land.remote.is_empty() && (land.behind_base > 0 || land.unmerged)
        } else {
            workspace_mode && !show_commit && (land.behind_base > 0 || land.unmerged)
        };
        let show_push = main_mode
            && !land.remote.is_empty()
            && land.ahead_base > 0
            && land.behind_base == 0
            && !land.unmerged;
        let workspace_github_pr = s.prs.iter().any(|p| {
            p.workspace_id == id
                && p.source == PrSource::GitHub
                && p.state != "Merged"
                && p.state != "Closed"
                && p.group != "merged"
        });
        let project = ws.and_then(|w| s.project(&w.project_id));
        let github_prs = project.is_some_and(|p| p.pr_on_github);
        let show_merge = workspace_mode
            && !show_commit
            && !show_update
            && land.ahead_base > 0
            && !workspace_github_pr
            && !(github_prs && !land.remote.is_empty() && land.require_pr != Some(false));
        let agent_busy = s.workspaces_running.contains(id);
        let land_locked =
            agent_busy || self.updating_base || self.pulling_pr || self.pushing_remote;
        let land_pending = land_on && !self.known;
        let land_busy = land_locked || land_pending;
        let land_locked_title = if agent_busy {
            "Wait until the agent finishes"
        } else {
            ""
        };
        let land_busy_title = if land_pending {
            "Checking git status…".to_string()
        } else {
            land_locked_title.to_string()
        };
        let store_pr = if id.is_empty() {
            None
        } else {
            pr_for_workspace(&s.prs, id).cloned()
        };
        let store_github_pr = store_pr.as_ref().filter(|p| p.source == PrSource::GitHub);
        let gh_life = pr_lifecycle(store_github_pr);
        let settled =
            |l: Option<PrLifecycle>| matches!(l, Some(PrLifecycle::Merged | PrLifecycle::Closed));
        let store_open_pr = store_github_pr.filter(|_| !settled(gh_life));
        let view_pr_id = store_pr
            .as_ref()
            .map(|p| p.id.clone())
            .filter(|x| !x.is_empty())
            .unwrap_or_else(|| land.pr_id.clone());
        let view_pr_number = store_pr
            .as_ref()
            .map(|p| p.number)
            .filter(|n| *n != 0)
            .unwrap_or(land.pr_number);
        let view_pr_draft = land.pr_draft || store_open_pr.is_some_and(|p| p.draft);
        let show_view_pr =
            s.view == ViewName::Detail && ws.is_some_and(|w| !w.is_main) && !view_pr_id.is_empty();
        let open_pr_source = if land.pr_source.is_empty() {
            store_open_pr
                .map(|p| p.source.as_str().to_string())
                .unwrap_or_default()
        } else {
            land.pr_source.clone()
        };
        let github_pr_open = open_pr_source == "gh"
            && (store_open_pr.is_some() || (!land.pr_id.is_empty() && !settled(gh_life)));
        let show_open_github_pr = github_prs
            && !github_pr_open
            && gh_life != Some(PrLifecycle::Merged)
            && s.view == ViewName::Detail
            && ws.is_some_and(|w| {
                !w.is_main && !blocked && w.state.as_str() != "merged" && !ws_run_active(Some(w))
            })
            && (workspace_mode || show_view_pr);
        let clean = !land.dirty && !land.unmerged;
        let pr_diverged = github_pr_open && land.ahead_remote > 0 && land.behind_remote > 0;
        let can_pull_pr = github_pr_open && clean && land.behind_remote > 0;
        let can_push_pr =
            github_pr_open && clean && land.ahead_remote > 0 && land.behind_remote == 0;
        let can_force_push_pr = github_pr_open && clean && pr_diverged;
        let merge_base_label = {
            let b = ws
                .map(|w| w.base_branch.trim())
                .filter(|b| !b.is_empty())
                .unwrap_or("main");
            let stripped = ["origin/", "upstream/"]
                .iter()
                .find_map(|p| b.strip_prefix(p).filter(|r| !r.is_empty()));
            stripped.unwrap_or(b).to_string()
        };
        let resolved_merge = project
            .map(|p| p.merge_method.as_str().to_string())
            .filter(|m| merge_method_label(m).is_some())
            .or_else(|| {
                s.settings
                    .as_ref()
                    .map(|st| st.default_merge_method.as_str().to_string())
                    .filter(|m| merge_method_label(m).is_some())
            })
            .unwrap_or_else(|| "merge".into());
        let resolved_update = project
            .map(|p| p.update_base_method.as_str().to_string())
            .filter(|m| m == "rebase" || m == "merge")
            .or_else(|| {
                s.settings
                    .as_ref()
                    .map(|st| st.default_update_base_method.as_str().to_string())
                    .filter(|m| m == "rebase" || m == "merge")
            })
            .unwrap_or_else(|| "merge".into());
        let merge_method = self.merge_method_override.clone().unwrap_or(resolved_merge);
        let update_method = self
            .update_method_override
            .clone()
            .unwrap_or(resolved_update);
        let merge_label = merge_method_label(&merge_method)
            .unwrap_or("Merge")
            .to_string();
        let update_label = {
            let base = if main_mode {
                if land.base.is_empty() {
                    "remote".to_string()
                } else {
                    land.base.clone()
                }
            } else {
                merge_base_label.clone()
            };
            if update_method == "rebase" {
                format!("Rebase onto {base}")
            } else if main_mode {
                format!("Update from {base}")
            } else {
                format!("Update with {base}")
            }
        };
        let merge_hint = if !land.remote.is_empty() && !land.base.is_empty() {
            format!("{merge_label} to {} (push)", land.base)
        } else {
            format!("{merge_label} into {merge_base_label} on disk — you are the only gate")
        };
        LandView {
            workspace_mode,
            main_mode,
            land_on,
            show_commit,
            show_update,
            show_push,
            show_merge,
            agent_busy,
            land_busy,
            land_busy_title,
            github_prs,
            github_pr_open,
            show_open_github_pr,
            can_pull_pr,
            can_push_pr,
            can_force_push_pr,
            merge_base_label,
            update_label,
            update_method,
            merge_method,
            merge_label,
            merge_hint,
            view_pr_id,
            view_pr_number,
            view_pr_draft,
            show_view_pr,
            store_pr,
        }
    }

    fn land_short_branch(&self) -> String {
        if self.land.remote.is_empty() {
            return self.land.base.clone();
        }
        let prefix = format!("{}/", self.land.remote);
        self.land
            .base
            .strip_prefix(&prefix)
            .unwrap_or(&self.land.base)
            .to_string()
    }

    fn land_hits_rules(&self, branch: &str, remote: Option<&str>) -> bool {
        if self.land.remote.is_empty() {
            return false;
        }
        if remote.is_some_and(|r| r != self.land.remote) {
            return false;
        }
        self.land_short_branch() == branch
    }
}

fn split_menu(items: Vec<AnyElement>, cx: &gpui_kit::App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    // `.merge-menu`: under its split, right-aligned, above the body.
    deferred(
        div()
            .absolute()
            .top(px(26. + 6.))
            .right_0()
            .child(with_shadow(
                style::card(t.rail, t.line, 8.)
                    .min_w(px(180.))
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .children(items),
                &[css_box_shadow(0., 6., 18., 0., rgba(0, 0, 0, 0.3))],
                8.,
            )),
    )
    .with_priority(1)
    .into_any_element()
}

fn menu_item(
    id: impl Into<gpui_kit::ElementId>,
    on: bool,
    check: Option<bool>,
    label: impl Into<SharedString>,
    cx: &gpui_kit::App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    let color = if on { t.emerald } else { t.text };
    let id = id.into();
    let sel = id.to_string();
    let mut row = div()
        .id(id)
        .debug_selector(move || sel)
        .role(gpui_kit::Role::MenuItem)
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .py(px(7.))
        .px(px(10.))
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(color)
        .hover(move |s| s.bg(t.pane_2));
    if let Some(checked) = check {
        row = row.child(
            div().flex_none().w(px(14.)).flex().justify_center().child(
                text(if checked { "✓" } else { "" })
                    .size(13.)
                    .line_height(recur_ui::LineHeight::Normal)
                    .nowrap(),
            ),
        );
    }
    row.child(style::button_label(label, 12.5, weight::MEDIUM))
}

/// Spec handles on a titlebar button: `{id}={label}`, or `{id}={label}:off:{tip}` while
/// disabled.
fn spec_tag(
    b: gpui_kit::Stateful<gpui_kit::Div>,
    id: &str,
    label: &str,
    off: Option<&str>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    b.debug_selector(|| match off {
        None => format!("{id}={label}"),
        Some(tip) => format!("{id}={label}:off:{tip}"),
    })
}

/// A spec handle `{id}` around a titlebar control, whatever its label or state.
fn spec_wrap(id: &'static str, el: impl IntoElement) -> gpui_kit::Div {
    div().flex().debug_selector(move || id.into()).child(el)
}

impl Shell {
    /// Refresh the land chrome off store changes (App.svelte's `$effect`s).
    pub(crate) fn land_state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (id, key, scan_pending) = {
            let s = self.state.read(cx);
            let ws = s.selected_workspace();
            let key = RefreshKey {
                id: s.selected_workspace_id.clone(),
                workspace_mode: is_workspace_mode(s, ws),
                main_mode: is_main_mode(s, ws),
                diff: ws.map(|w| (w.diff.add, w.diff.del)).unwrap_or_default(),
                behind_base: ws.map(|w| w.behind_base).unwrap_or_default(),
                state: ws.map(|w| w.state.as_str().to_string()).unwrap_or_default(),
                blocked: ws_main_blocked(ws),
            };
            (s.selected_workspace_id.clone(), key, s.scan_pending)
        };
        // `$effect.pre`: a new workspace recalls its last snapshot, disabled until git
        // confirms.
        if id != self.land.workspace_id {
            self.land.workspace_id = id.clone();
            self.land.seq += 1;
            self.land.land = cx
                .default_global::<LandCache>()
                .get(&id)
                .unwrap_or_default();
            self.land.known = false;
            self.land.rules_evt = None;
        }
        // Selecting another workspace resets the menus and method overrides.
        if self.land.menus_for != id {
            self.land.menus_for = id.clone();
            let l = &mut self.land;
            l.confirming_merge = false;
            l.merge_menu_open = false;
            l.merge_method_override = None;
            l.update_menu_open = false;
            l.open_pr_menu_open = false;
            l.update_method_override = None;
            self.state.update(cx, |s, cx| {
                if s.overlays.run_menu || s.overlays.stop_menu {
                    s.overlays.run_menu = false;
                    s.overlays.stop_menu = false;
                    cx.notify();
                }
            });
        }
        if self.land.refresh_key.as_ref() != Some(&key) || scan_pending {
            self.land.refresh_key = Some(key.clone());
            if scan_pending {
                self.land.seq += 1;
            } else {
                self.refresh_land_state(cx);
            }
        }
        // Fetch remotes two frames after the workbench paints, so a click is not coupled to
        // git fetch.
        let fetch_key = if !scan_pending && (key.main_mode || key.workspace_mode) && !id.is_empty()
        {
            id.clone()
        } else {
            String::new()
        };
        if fetch_key != self.land.fetch_key {
            self.land.fetch_key = fetch_key.clone();
            self.land.fetch = None;
            if !fetch_key.is_empty() {
                self.land.clock += 1;
                let started = self.land.clock;
                let services = self.state.read(cx).services().clone();
                let id = fetch_key.clone();
                let entity = cx.entity();
                window.on_next_frame(move |window, cx| {
                    window.on_next_frame(move |_, cx| {
                        let task = cx.spawn(async move |cx| {
                            if let Ok(st) =
                                services.workspaces.fetch_working_copy_upstream(&id).await
                            {
                                entity.update(cx, |shell, cx| {
                                    if shell.land.fetch_key == id {
                                        shell.land.seq += 1;
                                        shell.apply_land(Some(&st), started, cx);
                                    }
                                });
                            }
                        });
                        task.detach();
                    });
                    let _ = cx;
                });
            }
        }
        let view = self.land.view(self.state.read(cx));
        if view.land_busy {
            let l = &mut self.land;
            l.confirming_merge = false;
            l.merge_menu_open = false;
            l.update_menu_open = false;
            l.open_pr_menu_open = false;
            self.state.update(cx, |s, cx| {
                if s.overlays.run_menu || s.overlays.stop_menu {
                    s.overlays.run_menu = false;
                    s.overlays.stop_menu = false;
                    cx.notify();
                }
            });
        }
        self.pending_pr_create_changed(cx);
        self.base_select_changed(window, cx);
        self.issue_select_changed(window, cx);
    }

    /// `applyLand`: a fresh snapshot, keeping a branch-rules answer that arrived meanwhile.
    fn apply_land(
        &mut self,
        st: Option<&WorkspaceLandState>,
        started: u64,
        cx: &mut Context<Self>,
    ) {
        let mut land = st.map(Land::from).unwrap_or_default();
        if let Some(evt) = self.land.rules_evt.clone()
            && started > 0
            && evt.at >= started
        {
            let s = self.state.read(cx);
            let hits = s
                .selected_workspace()
                .is_some_and(|w| w.project_id == evt.project_id);
            self.land.land = land.clone();
            let remote = (!evt.remote.is_empty()).then_some(evt.remote.as_str());
            if hits && self.land.land_hits_rules(&evt.branch, remote) {
                land.require_pr = Some(evt.require_pr);
            }
        }
        self.land.land = land;
        self.land.known = true;
        let id = self.state.read(cx).selected_workspace_id.clone();
        if !id.is_empty() {
            let l = self.land.land.clone();
            cx.default_global::<LandCache>().put(&id, &l);
        }
        cx.notify();
    }

    /// `refreshLandState`.
    pub(crate) fn refresh_land_state(&mut self, cx: &mut Context<Self>) {
        let (id, on) = {
            let s = self.state.read(cx);
            let ws = s.selected_workspace();
            let on = (is_workspace_mode(s, ws) || is_main_mode(s, ws)) && !ws_main_blocked(ws);
            (s.selected_workspace_id.clone(), on)
        };
        if id.is_empty() || !on {
            return;
        }
        self.land.seq += 1;
        self.land.clock += 1;
        let (seq, started) = (self.land.seq, self.land.clock);
        let services = self.state.read(cx).services().clone();
        cx.spawn(async move |this, cx| {
            let res = services.workspaces.workspace_land_state(&id).await;
            let _ = this.update(cx, |shell, cx| {
                if shell.land.seq != seq {
                    return;
                }
                match res {
                    Ok(st) => shell.apply_land(Some(&st), started, cx),
                    Err(_) => {
                        // Never leave a recalled snapshot stuck on "Checking…".
                        shell.land.land = Land::default();
                        shell.land.known = true;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// `scheduleLandRefreshFromDisk`: a clean `git pull` moves HEAD without touching the row,
    /// so `workspace.fs_changed` and `app.focused` refresh the chrome (50ms debounce).
    pub(crate) fn schedule_land_refresh_from_disk(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).scan_pending || self.land.fs_timer.is_some() {
            return;
        }
        self.land.fs_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LAND_FS_DEBOUNCE).await;
            let _ = this.update(cx, |shell, cx| {
                shell.land.fs_timer = None;
                if !shell.state.read(cx).scan_pending {
                    shell.refresh_land_state(cx);
                }
            });
        }));
    }

    /// `applyRequirePR` (`github.branch_rules`).
    pub(crate) fn apply_require_pr(
        &mut self,
        project_id: &str,
        branch: &str,
        require_pr: bool,
        remote: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let s = self.state.read(cx);
        let id = s.selected_workspace_id.clone();
        if id.is_empty() || s.workspace(&id).is_none_or(|w| w.project_id != project_id) {
            return;
        }
        self.land.rules_evt = Some(RulesEvt {
            project_id: project_id.into(),
            remote: remote
                .map(str::to_string)
                .unwrap_or_else(|| self.land.land.remote.clone()),
            branch: branch.into(),
            require_pr,
            at: self.land.clock,
        });
        if !self.land.land_hits_rules(branch, remote)
            || self.land.land.require_pr == Some(require_pr)
        {
            return;
        }
        self.land.land.require_pr = Some(require_pr);
        let l = self.land.land.clone();
        cx.default_global::<LandCache>().put(&id, &l);
        cx.notify();
    }

    pub(crate) fn land_event(&mut self, ev: &AppEvent, cx: &mut Context<Self>) {
        match ev {
            AppEvent::WorkspaceFsChanged(d)
                if d.workspace_id == self.state.read(cx).selected_workspace_id =>
            {
                self.schedule_land_refresh_from_disk(cx);
            }
            AppEvent::AppFocused(_) if !self.state.read(cx).selected_workspace_id.is_empty() => {
                self.schedule_land_refresh_from_disk(cx);
            }
            AppEvent::GithubBranchRules(d) => {
                let remote = (!d.remote.is_empty()).then_some(d.remote.as_str());
                self.apply_require_pr(&d.project_id, &d.branch, d.require_pr, remote, cx);
            }
            AppEvent::WorkspaceDeleted(d) => {
                cx.default_global::<LandCache>().delete(&d.workspace_id)
            }
            AppEvent::WorkspaceRemoved(d) => {
                cx.default_global::<LandCache>().delete(&d.workspace_id)
            }
            _ => {}
        }
    }

    fn request_prompt_action(&mut self, slash: &str, cx: &mut Context<Self>) {
        let slash = slash.to_string();
        self.state.update(cx, |s, cx| {
            let id = s.selected_workspace_id.clone();
            s.request_prompt_action(&id, &slash, "", cx);
        });
    }

    async fn after_update(this: &gpui_kit::WeakEntity<Shell>, cx: &mut gpui_kit::AsyncApp) {
        let _ = this.update(cx, |shell, cx| shell.refresh_land_state(cx));
    }

    fn merge_workspace(&mut self, cx: &mut Context<Self>) {
        let v = self.land.view(self.state.read(cx));
        if v.land_busy {
            return;
        }
        if !self.land.confirming_merge {
            self.land.confirming_merge = true;
            self.land.merge_menu_open = false;
            cx.notify();
            return;
        }
        self.land.confirming_merge = false;
        self.land.merge_menu_open = false;
        let (id, services) = {
            let s = self.state.read(cx);
            (s.selected_workspace_id.clone(), s.services().clone())
        };
        let (method, label, base_label) = (v.merge_method, v.merge_label, v.merge_base_label);
        let (remote, base) = (self.land.land.remote.clone(), self.land.land.base.clone());
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let Some(merged) = call(
                cx,
                services.workspaces.merge_workspace(&id, &method),
                "Could not merge",
            )
            .await
            else {
                return;
            };
            state.update(cx, |s, cx| s.replace_workspace(merged, cx));
            let msg = if !remote.is_empty() && !base.is_empty() {
                format!("{label} to {base}")
            } else {
                format!("{label} into {base_label}")
            };
            cx.update(|cx| toast(ToastKind::Success, msg, cx));
        })
        .detach();
        cx.notify();
    }

    fn update_workspace(&mut self, cx: &mut Context<Self>) {
        let v = self.land.view(self.state.read(cx));
        let id = self.state.read(cx).selected_workspace_id.clone();
        if v.land_busy || id.is_empty() {
            return;
        }
        self.land.updating_base = true;
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        let (main, method, base_label) = (v.main_mode, v.update_method, v.merge_base_label);
        cx.spawn(async move |this, cx| {
            let fut = async {
                if main {
                    services.workspaces.update_working_copy_from_remote(&id, &method).await
                } else {
                    services.workspaces.update_workspace_with_base(&id, &method).await
                }
            };
            if let Some(res) = call(cx, fut, "Could not update").await {
                if !res.workspace.id.is_empty() {
                    state.update(cx, |s, cx| s.patch_workspace_after_update(res.workspace.clone(), cx));
                }
                let base = match res.base.trim() {
                    "" => base_label.trim().to_string(),
                    b => b.to_string(),
                };
                let base = if base.is_empty() { "main".to_string() } else { base };
                match res.status.as_str() {
                    "conflict" => {
                        state.update(cx, |s, cx| s.request_prompt_action(&id, "update", "", cx));
                    }
                    "updated" => {
                        let msg = if res.pushed_pr != 0 {
                            let verb = if res.force_pushed { "force-pushed" } else { "pushed" };
                            format!("Updated with latest {base} and {verb} to PR #{}", res.pushed_pr)
                        } else {
                            format!("Updated with latest {base}")
                        };
                        cx.update(|cx| toast(ToastKind::Success, msg, cx));
                    }
                    "autostash" => cx.update(|cx| {
                        toast(
                            ToastKind::Error,
                            format!("Updated with latest {base}, but uncommitted edits could not be reapplied — they are in git stash"),
                            cx,
                        )
                    }),
                    "noop" => cx.update(|cx| {
                        toast(ToastKind::Info, format!("Already up to date with {base}"), cx)
                    }),
                    _ => {}
                }
            }
            let _ = this.update(cx, |shell, _| shell.land.updating_base = false);
            Shell::after_update(&this, cx).await;
        })
        .detach();
    }

    fn push_working_copy(&mut self, cx: &mut Context<Self>) {
        let v = self.land.view(self.state.read(cx));
        let id = self.state.read(cx).selected_workspace_id.clone();
        if v.land_busy || id.is_empty() {
            return;
        }
        self.land.pushing_remote = true;
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        let remote = self.land.land.remote.clone();
        cx.spawn(async move |this, cx| {
            if let Some(pushed) = call(
                cx,
                services.workspaces.push_working_copy(&id),
                "Could not push",
            )
            .await
            {
                if !pushed.id.is_empty() {
                    state.update(cx, |s, cx| s.replace_workspace(pushed, cx));
                }
                let to = if remote.is_empty() {
                    "remote".to_string()
                } else {
                    remote
                };
                cx.update(|cx| toast(ToastKind::Success, format!("Pushed to {to}"), cx));
            }
            let _ = this.update(cx, |shell, _| shell.land.pushing_remote = false);
            Shell::after_update(&this, cx).await;
        })
        .detach();
    }

    fn pull_workspace_pr(&mut self, cx: &mut Context<Self>) {
        let v = self.land.view(self.state.read(cx));
        let id = self.state.read(cx).selected_workspace_id.clone();
        if v.land_busy || !v.can_pull_pr || id.is_empty() {
            return;
        }
        self.land.pulling_pr = true;
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        let n = v.view_pr_number;
        cx.spawn(async move |this, cx| {
            if let Some(res) = call(
                cx,
                services.workspaces.pull_workspace_pr(&id),
                "Could not pull from the PR",
            )
            .await
            {
                if !res.workspace.id.is_empty() {
                    state.update(cx, |s, cx| {
                        s.patch_workspace_after_update(res.workspace.clone(), cx)
                    });
                }
                match res.status.as_str() {
                    "conflict" => {
                        state.update(cx, |s, cx| s.request_prompt_action(&id, "update", "", cx))
                    }
                    "updated" => cx.update(|cx| {
                        let msg = if n != 0 {
                            format!("Pulled updates from PR #{n}")
                        } else {
                            "Pulled updates from the PR".into()
                        };
                        toast(ToastKind::Success, msg, cx)
                    }),
                    "noop" => cx.update(|cx| {
                        let msg = if n != 0 {
                            format!("Already up to date with PR #{n}")
                        } else {
                            "Already up to date with the PR".into()
                        };
                        toast(ToastKind::Info, msg, cx)
                    }),
                    _ => {}
                }
            }
            let _ = this.update(cx, |shell, _| shell.land.pulling_pr = false);
            Shell::after_update(&this, cx).await;
        })
        .detach();
    }

    fn push_workspace_pr(&mut self, force: bool, cx: &mut Context<Self>) {
        let v = self.land.view(self.state.read(cx));
        let id = self.state.read(cx).selected_workspace_id.clone();
        let allowed = if force {
            v.can_force_push_pr
        } else {
            v.can_push_pr
        };
        if v.land_busy || !allowed || id.is_empty() {
            return;
        }
        self.land.pushing_remote = true;
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let n = v.view_pr_number;
        cx.spawn(async move |this, cx| {
            let fail = if force {
                "Could not force-push to the PR"
            } else {
                "Could not push to the PR"
            };
            if let Some(pr_id) =
                call(cx, services.workspaces.open_workspace_pr(&id, force), fail).await
                && !pr_id.is_empty()
            {
                let msg = match (force, n != 0) {
                    (true, true) => format!("Force-pushed to PR #{n}"),
                    (true, false) => "Force-pushed updates to the PR".into(),
                    (false, true) => format!("Pushed to PR #{n}"),
                    (false, false) => "Pushed updates to the PR".into(),
                };
                cx.update(|cx| toast(ToastKind::Success, msg, cx));
            }
            let _ = this.update(cx, |shell, _| shell.land.pushing_remote = false);
            Shell::after_update(&this, cx).await;
        })
        .detach();
    }

    fn request_open_pr(&mut self, draft: bool, cx: &mut Context<Self>) {
        let v = self.land.view(self.state.read(cx));
        if v.land_busy || v.agent_busy {
            return;
        }
        self.land.open_pr_menu_open = false;
        self.request_prompt_action(if draft { "open-draft-pr" } else { "open-pr" }, cx);
        cx.notify();
    }

    fn close_split_menus(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.overlays.run_menu = false;
            s.overlays.stop_menu = false;
            cx.notify();
        });
    }

    /// `createPRWorkspace`: a Recur workspace on the PR on screen.
    fn create_pr_workspace(&mut self, cx: &mut Context<Self>) {
        let id = self.state.read(cx).selected_pr_id.clone();
        if id.is_empty()
            || self
                .land
                .pending_pr_create
                .as_ref()
                .is_some_and(|p| p.0 == id)
        {
            return;
        }
        self.land.pending_pr_create = Some((id.clone(), String::new()));
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let Some(ws) = call(
                cx,
                services.prs.create_workspace_from_pr(&id),
                "Could not create the workspace",
            )
            .await
            else {
                let _ = this.update(cx, |shell, cx| {
                    if shell
                        .land
                        .pending_pr_create
                        .as_ref()
                        .is_some_and(|p| p.0 == id)
                    {
                        shell.land.pending_pr_create = None;
                        cx.notify();
                    }
                });
                return;
            };
            let ready = !ws.worktree_path.trim().is_empty() && ws.host_pr_id.trim().is_empty();
            state.update(cx, |s, cx| {
                s.upsert_workspace(ws.clone(), cx);
                s.replace_workspace(ws.clone(), cx);
                if ready {
                    let wid = ws.id.clone();
                    let pid = ws.project_id.clone();
                    s.update_pr(
                        &id,
                        |p| {
                            p.workspace_id = wid;
                            if p.project_id.is_empty() {
                                p.project_id = pid;
                            }
                        },
                        cx,
                    );
                }
            });
            let _ = this.update(cx, |shell, cx| {
                if ready {
                    toast(ToastKind::Success, "Workspace created", cx);
                    if shell
                        .land
                        .pending_pr_create
                        .as_ref()
                        .is_some_and(|p| p.0 == id)
                    {
                        shell.land.pending_pr_create = None;
                    }
                } else {
                    shell.land.pending_pr_create = Some((id.clone(), ws.id.clone()));
                    toast(ToastKind::Info, "Creating workspace…", cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The `pendingPRCreate` effect: done once the PR links its workspace; dropped if the
    /// workspace failed or vanished.
    fn pending_pr_create_changed(&mut self, cx: &mut Context<Self>) {
        let Some((pr_id, ws_id)) = self.land.pending_pr_create.clone() else {
            return;
        };
        let s = self.state.read(cx);
        if s.pr(&pr_id)
            .is_some_and(|p| !p.workspace_id.trim().is_empty())
        {
            self.land.pending_pr_create = None;
            return;
        }
        if ws_id.is_empty() {
            return;
        }
        let gone = s
            .workspace(&ws_id)
            .is_none_or(|w| !w.last_error.is_empty() || w.phase == "failed");
        if gone {
            self.land.pending_pr_create = None;
        }
    }

    /// The land buttons, Open PR, the issue control and View PR (detail), or View / Create
    /// workspace (PR detail), in App.svelte's order.
    pub(crate) fn land_chrome(
        &mut self,
        view: ViewName,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let t = Theme::of(cx).tokens;
        let backdrop = t.titlebar;
        if view == ViewName::PrDetail {
            let s = self.state.read(cx);
            let Some(pr) = s.pr(&s.selected_pr_id).cloned() else {
                return out;
            };
            if !pr.workspace_id.is_empty() {
                let state = self.state.clone();
                let wid = pr.workspace_id.clone();
                out.push(
                    spec_wrap(
                        "tb-view-workspace",
                        titled(
                            "tb-view-workspace-tip",
                            "View this pull request's workspace",
                            no_drag(
                                spec_tag(
                                    tbtn(
                                        "tb-view-workspace",
                                        Tbtn::Default,
                                        TbtnSize::TITLEBAR,
                                        None,
                                        cx,
                                    ),
                                    "tb-view-workspace",
                                    "View workspace",
                                    None,
                                )
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |s, cx| s.open_workspace(&wid, false, cx))
                                })
                                .child(icon(IconName::Box).size(px(13.)))
                                .child(style::button_label(
                                    "View workspace",
                                    11.5,
                                    weight::MEDIUM,
                                )),
                            ),
                        ),
                    )
                    .into_any_element(),
                );
            } else if !matches!(
                pr_lifecycle(Some(&pr)),
                Some(PrLifecycle::Merged | PrLifecycle::Closed)
            ) {
                let creating = self
                    .land
                    .pending_pr_create
                    .as_ref()
                    .is_some_and(|p| p.0 == pr.id);
                let label = if creating {
                    "Creating…"
                } else {
                    "Create workspace"
                };
                let mut b = spec_tag(
                    tbtn(
                        "tb-create-workspace",
                        Tbtn::Default,
                        TbtnSize::TITLEBAR,
                        creating.then_some(backdrop),
                        cx,
                    ),
                    "tb-create-workspace",
                    label,
                    creating.then_some("Create a Recur workspace on this pull request"),
                );
                if !creating {
                    b = b.on_click(cx.listener(|this, _, _, cx| this.create_pr_workspace(cx)));
                }
                out.push(
                    spec_wrap(
                        "tb-create-workspace",
                        titled(
                            "tb-create-workspace-tip",
                            "Create a Recur workspace on this pull request",
                            no_drag(
                                b.child(icon(IconName::Plus).size(px(13.)))
                                    .child(style::button_label(label, 11.5, weight::MEDIUM)),
                            ),
                        ),
                    )
                    .into_any_element(),
                );
            }
            return out;
        }
        if view != ViewName::Detail {
            return out;
        }
        let v = self.land.view(self.state.read(cx));
        let busy_over = v.land_busy.then_some(backdrop);
        let title = |fallback: &str| -> String {
            if v.land_busy_title.is_empty() {
                fallback.to_string()
            } else {
                v.land_busy_title.clone()
            }
        };
        if (v.workspace_mode || v.main_mode) && v.land_on {
            if v.show_commit {
                let tip = title("Commit uncommitted changes");
                let mut b = spec_tag(
                    tbtn("tb-commit", Tbtn::Solid, TbtnSize::TITLEBAR, busy_over, cx),
                    "tb-commit",
                    "Commit",
                    v.land_busy.then_some(tip.as_str()),
                );
                if !v.land_busy {
                    b = b.on_click(cx.listener(|this, _, _, cx| {
                        let v = this.land.view(this.state.read(cx));
                        if !v.land_busy {
                            this.request_prompt_action("commit", cx);
                        }
                    }));
                }
                out.push(
                    spec_wrap(
                        "tb-commit",
                        titled(
                            "tb-commit-tip",
                            tip,
                            no_drag(
                                b.child(icon(IconName::GitCommit).size(px(13.)))
                                    .child(style::button_label("Commit", 11.5, weight::MEDIUM)),
                            ),
                        ),
                    )
                    .into_any_element(),
                );
            }
            if v.show_update {
                out.push(self.update_split(&v, cx));
            }
            if v.show_push {
                let remote = self.land.land.remote.clone();
                let label = if self.land.pushing_remote {
                    "Pushing…".to_string()
                } else {
                    format!("Push to {remote}")
                };
                let tip = title(&format!("Push to {remote}"));
                let mut b = spec_tag(
                    rbtn("tb-push", Rbtn::Merge, busy_over, cx),
                    "tb-push",
                    &label,
                    v.land_busy.then_some(tip.as_str()),
                )
                .h(px(26.))
                .px(px(9.))
                .rounded(px(6.))
                .text_size(px(11.5));
                if !v.land_busy {
                    b = b.on_click(cx.listener(|this, _, _, cx| this.push_working_copy(cx)));
                }
                out.push(
                    spec_wrap(
                        "tb-push",
                        titled(
                            "tb-push-tip",
                            tip,
                            no_drag(
                                b.child(icon(IconName::Upload).size(px(13.)))
                                    .child(style::button_label(label, 11.5, weight::MEDIUM_PLUS)),
                            ),
                        ),
                    )
                    .into_any_element(),
                );
            }
            if v.show_merge {
                out.push(self.merge_split(&v, cx));
            }
            if v.workspace_mode && v.github_prs && v.github_pr_open {
                let pr_btn =
                    |id: &'static str,
                     label: String,
                     tip: &str,
                     f: fn(&mut Shell, &mut Context<Shell>),
                     cx: &mut Context<Shell>| {
                        let tip = title(tip);
                        let mut b = spec_tag(
                            tbtn(id, Tbtn::Solid, TbtnSize::TITLEBAR, busy_over, cx),
                            id,
                            &label,
                            v.land_busy.then_some(tip.as_str()),
                        );
                        if !v.land_busy {
                            b = b.on_click(cx.listener(move |this, _, _, cx| f(this, cx)));
                        }
                        spec_wrap(
                            id,
                            titled(
                                SharedString::from(format!("{id}-tip")),
                                tip,
                                no_drag(
                                    b.child(
                                        icon(if id == "tb-pull-pr" {
                                            IconName::ArrowUp
                                        } else {
                                            IconName::Upload
                                        })
                                        .size(px(13.)),
                                    )
                                    .child(style::button_label(label, 11.5, weight::MEDIUM)),
                                ),
                            ),
                        )
                        .into_any_element()
                    };
                if v.can_pull_pr {
                    let label = if self.land.pulling_pr {
                        "Pulling…"
                    } else {
                        "Pull updates"
                    };
                    out.push(pr_btn(
                        "tb-pull-pr",
                        label.into(),
                        "Bring GitHub commits into this workspace",
                        |this, cx| this.pull_workspace_pr(cx),
                        cx,
                    ));
                }
                if v.can_push_pr {
                    let label = if self.land.pushing_remote {
                        "Pushing…"
                    } else {
                        "Push updates"
                    };
                    out.push(pr_btn(
                        "tb-push-pr",
                        label.into(),
                        "Push new commits to the open pull request",
                        |this, cx| this.push_workspace_pr(false, cx),
                        cx,
                    ));
                }
                if v.can_force_push_pr {
                    let label = if self.land.pushing_remote {
                        "Force-pushing…"
                    } else {
                        "Force push"
                    };
                    out.push(pr_btn(
                        "tb-force-push-pr",
                        label.into(),
                        "Overwrite the GitHub head with this workspace (e.g. after rebase onto main). Remote-only commits will be dropped.",
                        |this, cx| this.push_workspace_pr(true, cx),
                        cx,
                    ));
                }
            }
        }
        if v.show_open_github_pr {
            out.push(self.open_pr_split(&v, cx));
        }
        let s = self.state.read(cx);
        if s.selected_workspace().is_some_and(|w| !w.is_main)
            && let Some(issue) = self.issue_button(cx)
        {
            out.push(issue);
        }
        if v.show_view_pr {
            let label = match (v.view_pr_number != 0, v.view_pr_draft) {
                (true, true) => format!("View draft #{}", v.view_pr_number),
                (true, false) => format!("View PR #{}", v.view_pr_number),
                (false, true) => "View draft".into(),
                (false, false) => "View PR".into(),
            };
            let mut shown = v.store_pr.clone().unwrap_or_default();
            shown.draft = v.view_pr_draft;
            let ico = pr_icon_name(Some(&shown));
            let state = self.state.clone();
            let pr_id = v.view_pr_id.clone();
            out.push(
                spec_wrap(
                    "tb-view-pr",
                    titled(
                        "tb-view-pr-tip",
                        "View this workspace's pull request",
                        no_drag(
                            spec_tag(
                                tbtn("tb-view-pr", Tbtn::Default, TbtnSize::TITLEBAR, None, cx),
                                "tb-view-pr",
                                &label,
                                None,
                            )
                            .on_click(move |_, _, cx| {
                                state.update(cx, |s, cx| s.open_pr(&pr_id, cx))
                            })
                            .child(icon(ico).size(px(13.)))
                            .child(style::button_label(
                                label,
                                11.5,
                                weight::MEDIUM,
                            )),
                        ),
                    ),
                )
                .into_any_element(),
            );
        }
        out
    }

    fn update_split(&mut self, v: &LandView, cx: &mut Context<Self>) -> AnyElement {
        let backdrop = Theme::of(cx).tokens.titlebar;
        let busy_over = v.land_busy.then_some(backdrop);
        let remote_base = if v.main_mode {
            if self.land.land.base.is_empty() {
                "remote".to_string()
            } else {
                self.land.land.base.clone()
            }
        } else {
            v.merge_base_label.clone()
        };
        let tip = if !v.land_busy_title.is_empty() {
            v.land_busy_title.clone()
        } else if self.land.land.unmerged {
            "A merge or rebase is already in progress — finish or abort it before updating".into()
        } else if v.update_method == "rebase" {
            format!("Rebase onto latest {remote_base}")
        } else {
            format!("Update with latest {remote_base}")
        };
        let label = if self.land.updating_base {
            "Updating…".to_string()
        } else {
            v.update_label.clone()
        };
        let mut main = spec_tag(
            tbtn("tb-update", Tbtn::Solid, TbtnSize::TITLEBAR, busy_over, cx),
            "tb-update",
            &label,
            v.land_busy.then_some(tip.as_str()),
        )
        .rounded_r(px(0.))
        .relative()
        .child(style::split_divider());
        let mut chev = tbtn(
            "tb-update-chev",
            Tbtn::Solid,
            TbtnSize::TITLEBAR,
            busy_over,
            cx,
        )
        .debug_selector(|| "tb-update-chev".into())
        .rounded_l(px(0.))
        .px(px(5.));
        if !v.land_busy {
            main = main.on_click(cx.listener(|this, _, _, cx| this.update_workspace(cx)));
            chev = chev.on_click(cx.listener(|this, _, _, cx| {
                this.land.update_menu_open = !this.land.update_menu_open;
                this.land.merge_menu_open = false;
                this.land.open_pr_menu_open = false;
                this.close_split_menus(cx);
                cx.notify();
            }));
        }
        let spin = self.land.updating_base;
        let mut split = div()
            .id("update-split")
            .debug_selector(|| "tb-update".into())
            .relative()
            .flex()
            .items_center()
            .child(titled(
                "tb-update-tip",
                tip,
                no_drag(
                    main.child(
                        icon(IconName::ArrowUp)
                            .size(px(13.))
                            .when(spin, |i| i.rotate(0.)),
                    )
                    .child(style::button_label(label, 11.5, weight::MEDIUM)),
                ),
            ))
            .child(no_drag(
                chev.child(icon(IconName::ChevronDown).size(px(13.))),
            ));
        if self.land.update_menu_open {
            let items = UPDATE_METHOD_KEYS
                .iter()
                .map(|m| {
                    let on = v.update_method == *m;
                    let m = m.to_string();
                    menu_item(
                        SharedString::from(format!("update-{m}")),
                        on,
                        Some(on),
                        update_method_menu_label(&m),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.land.update_method_override = Some(m.clone());
                        this.land.update_menu_open = false;
                        cx.notify();
                    }))
                    .into_any_element()
                })
                .collect();
            split = split.child(split_menu(items, cx));
        }
        split.into_any_element()
    }

    fn merge_split(&mut self, v: &LandView, cx: &mut Context<Self>) -> AnyElement {
        let backdrop = Theme::of(cx).tokens.titlebar;
        let busy_over = v.land_busy.then_some(backdrop);
        let tip = if v.land_busy_title.is_empty() {
            v.merge_hint.clone()
        } else {
            v.land_busy_title.clone()
        };
        let label = if self.land.confirming_merge {
            format!("Confirm {}", v.merge_label.to_lowercase())
        } else {
            v.merge_label.clone()
        };
        let mut main = spec_tag(
            rbtn("tb-merge", Rbtn::Merge, busy_over, cx),
            "tb-merge",
            &label,
            v.land_busy.then_some(tip.as_str()),
        )
        .h(px(26.))
        .px(px(9.))
        .rounded(px(6.))
        .rounded_r(px(0.))
        .relative()
        .child(style::split_divider())
        .text_size(px(11.5));
        let mut chev = rbtn("tb-merge-chev", Rbtn::Merge, busy_over, cx)
            .debug_selector(|| "tb-merge-chev".into())
            .h(px(26.))
            .rounded(px(6.))
            .rounded_l(px(0.))
            .px(px(5.));
        if !v.land_busy {
            main = main.on_click(cx.listener(|this, _, _, cx| this.merge_workspace(cx)));
            chev = chev.on_click(cx.listener(|this, _, _, cx| {
                this.land.merge_menu_open = !this.land.merge_menu_open;
                this.land.open_pr_menu_open = false;
                this.land.confirming_merge = false;
                this.close_split_menus(cx);
                cx.notify();
            }));
        }
        let mut split = div()
            .id("merge-split")
            .debug_selector(|| "tb-merge".into())
            .relative()
            .flex()
            .items_center()
            .child(titled(
                "tb-merge-tip",
                tip,
                no_drag(
                    main.child(icon(IconName::GitPullRequest).size(px(13.)))
                        .child(style::button_label(label, 11.5, weight::MEDIUM_PLUS)),
                ),
            ))
            .child(no_drag(
                chev.child(icon(IconName::ChevronDown).size(px(13.))),
            ));
        if self.land.merge_menu_open {
            let items = MERGE_METHOD_KEYS
                .iter()
                .map(|m| {
                    let on = v.merge_method == *m;
                    let m = m.to_string();
                    let label = merge_method_label(&m).unwrap_or("Merge");
                    menu_item(
                        SharedString::from(format!("merge-{m}")),
                        on,
                        Some(on),
                        label,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.land.merge_method_override = Some(m.clone());
                        this.land.merge_menu_open = false;
                        this.land.confirming_merge = false;
                        cx.notify();
                    }))
                    .into_any_element()
                })
                .collect();
            split = split.child(split_menu(items, cx));
        }
        split.into_any_element()
    }

    fn open_pr_split(&mut self, v: &LandView, cx: &mut Context<Self>) -> AnyElement {
        let backdrop = Theme::of(cx).tokens.titlebar;
        let off = v.land_busy || v.agent_busy;
        let over = off.then_some(backdrop);
        let tip = if !v.land_busy_title.is_empty() {
            v.land_busy_title.clone()
        } else if v.agent_busy {
            "Wait until the agent finishes".into()
        } else {
            "Open a GitHub pull request".into()
        };
        let mut main = spec_tag(
            tbtn("tb-open-pr", Tbtn::Default, TbtnSize::TITLEBAR, over, cx),
            "tb-open-pr",
            "Open PR",
            off.then_some(tip.as_str()),
        )
        .rounded_r(px(0.));
        let mut chev = tbtn(
            "tb-open-pr-chev",
            Tbtn::Default,
            TbtnSize::TITLEBAR,
            over,
            cx,
        )
        .debug_selector(|| "tb-open-pr-chev".into())
        .rounded_l(px(0.))
        .px(px(5.));
        if !off {
            main = main.on_click(cx.listener(|this, _, _, cx| this.request_open_pr(false, cx)));
            chev = chev.on_click(cx.listener(|this, _, _, cx| {
                this.land.open_pr_menu_open = !this.land.open_pr_menu_open;
                this.land.merge_menu_open = false;
                this.land.update_menu_open = false;
                this.close_split_menus(cx);
                cx.notify();
            }));
        }
        let mut split = div()
            .id("open-pr-split")
            .debug_selector(|| "tb-open-pr".into())
            .relative()
            .flex()
            .items_center()
            .child(titled(
                "tb-open-pr-tip",
                tip,
                no_drag(main.child(style::button_label("Open PR", 11.5, weight::MEDIUM))),
            ))
            .child(no_drag(
                chev.child(icon(IconName::ChevronDown).size(px(13.))),
            ));
        if self.land.open_pr_menu_open {
            let items = vec![
                menu_item("open-pr-item", true, None, "Open PR", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.request_open_pr(false, cx)))
                    .into_any_element(),
                menu_item("open-draft-pr-item", false, None, "Open draft PR", cx)
                    .on_click(cx.listener(|this, _, _, cx| this.request_open_pr(true, cx)))
                    .into_any_element(),
            ];
            split = split.child(split_menu(items, cx));
        }
        split.into_any_element()
    }

    // ---- WorkspaceBaseSelect ----

    /// `WorkspaceBaseSelect`: the target-branch picker after the crumb. Changing it retargets
    /// diffs and open PRs; it does not rebase the worktree.
    pub(crate) fn base_select(&mut self) -> AnyElement {
        match &self.land.base_select {
            Some((sel, _)) => no_drag(
                div()
                    .id("crumb-base")
                    .debug_selector(|| "crumb-base".into())
                    .flex_none()
                    .child(sel.clone()),
            )
            .into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn base_select_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ws, project_base) = {
            let s = self.state.read(cx);
            let ws = (s.view == ViewName::Detail)
                .then(|| s.selected_workspace().cloned())
                .flatten()
                .filter(|w| !w.is_main);
            let base = ws
                .as_ref()
                .and_then(|w| s.project(&w.project_id))
                .map(|p| p.base_branch.clone())
                .unwrap_or_default();
            (ws, base)
        };
        let Some(ws) = ws else { return };
        if self.land.base_select.is_none() {
            let t = Theme::of(cx).tokens;
            let sel = cx.new(|cx| {
                Select::new(Vec::new(), window, cx)
                    .label("Target branch")
                    .searchable()
                    .menu_variant(recur_ui::logic::menu_space::MenuVariant::Wide)
                    .trigger_icon(IconName::GitBranch)
                    .trigger_style(TriggerStyle {
                        height: None,
                        pad_y: Some(1.),
                        pad_x: Some(6.),
                        radius: Some(5.),
                        gap: Some(5.),
                        font_size: Some(10.5),
                        bg: Some(t.pane.fade(0.)),
                        border: Some(t.pane.fade(0.)),
                        color: Some(t.muted),
                        hover_bg: Some(t.pane_2),
                        hover_border: Some(t.pane_2.fade(0.)),
                        hover_color: Some(t.text),
                        open_bg: Some(t.pane_2),
                        open_border: Some(t.pane_2.fade(0.)),
                        open_color: Some(t.text),
                        no_ring: true,
                        icon_color: Some(t.muted),
                        icon_size: Some(12.),
                        chevron_size: Some(11.),
                        open_icon_color: Some(t.muted),
                        sel_open_wins: true,
                        value_auto: true,
                        auto_width: true,
                        ..Default::default()
                    })
            });
            let sub = cx.subscribe(&sel, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(next) = ev {
                    this.set_base_branch(next.to_string(), cx);
                }
            });
            self.land.base_select = Some((sel, sub));
        }
        // Reload branches when the workspace or project changes.
        let load_key = format!("{}\0{}", ws.project_id, ws.id);
        if self.land.base_loading_key != load_key {
            self.land.base_loading_key = load_key.clone();
            let services = self.state.read(cx).services().clone();
            let project = ws.project_id.clone();
            cx.spawn(async move |this, cx| {
                let list = services
                    .projects
                    .list_branches(&project)
                    .await
                    .unwrap_or_default();
                let _ = this.update(cx, |shell, cx| {
                    if shell.land.base_loading_key == load_key {
                        shell.land.base_branches = list;
                        shell.land.base_loaded_for = Some(project);
                        shell.sync_base_options(cx);
                    }
                });
            })
            .detach();
        }
        let _ = project_base;
        self.sync_base_options(cx);
    }

    fn sync_base_options(&mut self, cx: &mut Context<Self>) {
        let Some(sel) = self.land.base_select.as_ref().map(|(s, _)| s.clone()) else {
            return;
        };
        let s = self.state.read(cx);
        let Some(ws) = s.selected_workspace().cloned() else {
            return;
        };
        let project_base = s
            .project(&ws.project_id)
            .map(|p| p.base_branch.clone())
            .unwrap_or_default();
        let current = [ws.base_branch.trim(), project_base.trim(), "main"]
            .into_iter()
            .find(|b| !b.is_empty())
            .unwrap_or("main")
            .to_string();
        let own = ws.branch.clone();
        let mut seen = std::collections::HashSet::new();
        let mut opts: Vec<SelectOption> = Vec::new();
        for b in &self.land.base_branches {
            if b.name.is_empty() || b.name == own || !seen.insert(b.name.clone()) {
                continue;
            }
            let mut o = SelectOption::new(b.name.clone(), b.name.clone());
            if !b.group.is_empty() {
                o = o.group(b.group.clone());
            }
            opts.push(o);
        }
        if !current.is_empty() && current != own && !seen.contains(&current) {
            opts.insert(
                0,
                SelectOption::new(current.clone(), current.clone()).group("Local"),
            );
        }
        let disabled = self.land.base_saving
            || self.land.base_loaded_for.as_deref() != Some(ws.project_id.as_str())
            || opts.is_empty();
        let backdrop = Theme::of(cx).tokens.titlebar;
        sel.update(cx, |sel, cx| {
            sel.set_options(opts, cx);
            sel.set_value(current, cx);
            sel.set_disabled(disabled.then_some(backdrop), cx);
        });
    }

    fn set_base_branch(&mut self, next: String, cx: &mut Context<Self>) {
        let (ws, services) = {
            let s = self.state.read(cx);
            (s.selected_workspace().cloned(), s.services().clone())
        };
        let Some(ws) = ws else {
            return;
        };
        let current = ws.base_branch.trim().to_string();
        if next.is_empty() || next == current || self.land.base_saving {
            return;
        }
        self.land.base_saving = true;
        self.sync_base_options(cx);
        let state = self.state.clone();
        cx.spawn(async move |this, cx| {
            let updated = call(
                cx,
                services.workspaces.set_workspace_base_branch(&ws.id, &next),
                "Could not change the target branch",
            )
            .await;
            let _ = this.update(cx, |shell, cx| {
                shell.land.base_saving = false;
                if let Some(updated) = updated {
                    let base = if updated.base_branch.is_empty() {
                        next.clone()
                    } else {
                        updated.base_branch.clone()
                    };
                    state.update(cx, |s, cx| s.replace_workspace(updated, cx));
                    toast(
                        ToastKind::Success,
                        format!("Target branch is now {base}"),
                        cx,
                    );
                }
                shell.sync_base_options(cx);
            });
        })
        .detach();
    }

    // ---- WorkspaceIssueButton ----

    fn issue_select_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stale = self.state.read(cx).issues_stale;
        if self.land.tracker_fetched_at != Some(stale) {
            self.land.tracker_fetched_at = Some(stale);
            let services = self.state.read(cx).services().clone();
            cx.spawn(async move |this, cx| {
                if let Ok(rows) = services.settings.list_assigned_issues().await {
                    let _ = this.update(cx, |shell, cx| {
                        shell.land.tracker_issues = rows;
                        shell.sync_issue_options(cx);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        let Some(ws) = self.state.read(cx).selected_workspace().cloned() else {
            return;
        };
        let url = ws
            .issue
            .as_ref()
            .map(|i| issue_url(i, &self.land.tracker_issues))
            .unwrap_or_default();
        let split = ws.issue.is_some() && !url.is_empty();
        let label = if ws.issue.is_some() {
            "Change linked issue"
        } else {
            "Link issue"
        };
        if self.land.issue_select.as_ref().map(|(_, _, m)| *m) != Some((split, label)) {
            let t = Theme::of(cx).tokens;
            // `.tb-right .issue-pick .sel-btn`; in the split only the chevron shows (its icon
            // and value are `display: none`, so they take no width and no gap).
            let style = TriggerStyle {
                height: Some(26.),
                pad_x: Some(if split { 5. } else { 9. }),
                radius: Some(6.),
                square_left: split,
                gap: Some(if split { 0. } else { 6. }),
                font_size: Some(if split { 0. } else { 11.5 }),
                ui_font: true,
                weight: Some(weight::MEDIUM),
                hover_border: Some(t.line_bright),
                hover_bg: Some(t.pane),
                open_border: Some(t.line_bright),
                open_bg: Some(t.pane),
                no_ring: true,
                icon_size: Some(if split { 0. } else { 13. }),
                chevron_size: Some(13.),
                auto_width: true,
                ..Default::default()
            };
            let sel = cx.new(|cx| {
                Select::new(Vec::new(), window, cx)
                    .label(label)
                    .searchable()
                    .placeholder("Link issue")
                    .menu_variant(recur_ui::logic::menu_space::MenuVariant::Issue)
                    .trigger_icon(IconName::CircleDot)
                    .trigger_style(style)
            });
            let sub = cx.subscribe(&sel, |this, _, ev: &SelectEvent, cx| {
                if let SelectEvent::Change(next) = ev {
                    this.change_issue(next.to_string(), cx);
                }
            });
            self.land.issue_select = Some((sel, sub, (split, label)));
        }
        self.sync_issue_options(cx);
    }

    fn issue_options(&self, ws: &Workspace) -> Vec<SelectOption> {
        let key = |source: &str, id: &str| format!("{source}:{id}");
        let current = ws
            .issue
            .as_ref()
            .map(|i| key(&i.source, &i.id))
            .unwrap_or_default();
        let mut out: Vec<SelectOption> = self
            .land
            .tracker_issues
            .iter()
            .filter(|i| i.project_id == ws.project_id)
            .map(|i| SelectOption::new(key(&i.source, &i.id), format!("{} {}", i.r#ref, i.title)))
            .collect();
        if let Some(issue) = &ws.issue
            && !out.iter().any(|o| o.value.as_ref() == current)
        {
            let label = if issue.key.is_empty() {
                issue.id.clone()
            } else {
                issue.key.clone()
            };
            out.insert(0, SelectOption::new(current.clone(), label));
        }
        if ws.issue.is_some() {
            out.insert(0, SelectOption::new("", "Unlink issue"));
        }
        out
    }

    fn sync_issue_options(&mut self, cx: &mut Context<Self>) {
        let Some(sel) = self.land.issue_select.as_ref().map(|(s, _, _)| s.clone()) else {
            return;
        };
        let Some(ws) = self.state.read(cx).selected_workspace().cloned() else {
            return;
        };
        let opts = self.issue_options(&ws);
        let current = ws
            .issue
            .as_ref()
            .map(|i| format!("{}:{}", i.source, i.id))
            .unwrap_or_default();
        sel.update(cx, |sel, cx| {
            sel.set_options(opts, cx);
            sel.set_value(current, cx);
        });
    }

    fn change_issue(&mut self, next: String, cx: &mut Context<Self>) {
        let s = self.state.read(cx);
        let Some(ws) = s.selected_workspace().cloned() else {
            return;
        };
        let current = ws
            .issue
            .as_ref()
            .map(|i| format!("{}:{}", i.source, i.id))
            .unwrap_or_default();
        if next == current {
            return;
        }
        if next.is_empty() {
            self.state
                .update(cx, |s, cx| s.unlink_workspace_issue(&ws.id, cx));
            return;
        }
        let hit = self
            .land
            .tracker_issues
            .iter()
            .find(|i| i.project_id == ws.project_id && format!("{}:{}", i.source, i.id) == next)
            .cloned();
        if let Some(i) = hit {
            let issue = recur_domain::IssueRef {
                source: i.source.clone(),
                id: i.id.clone(),
                key: i.r#ref.clone(),
            };
            self.state
                .update(cx, |s, cx| s.link_workspace_issue(&ws.id, issue, cx));
        }
    }

    /// `WorkspaceIssueButton`: View the linked issue plus the picker, or the picker alone;
    /// hidden with no link and nothing to link.
    fn issue_button(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sel = self.land.issue_select.as_ref().map(|(s, _, _)| s.clone())?;
        let ws = self.state.read(cx).selected_workspace().cloned()?;
        let opts = self.issue_options(&ws);
        let issue = ws.issue.clone();
        let url = issue
            .as_ref()
            .map(|i| issue_url(i, &self.land.tracker_issues))
            .unwrap_or_default();
        if let Some(issue) = issue.filter(|_| !url.is_empty()) {
            let github = issue.source == "github";
            let label = if github {
                format!("#{}", issue.key.rsplit('#').next().unwrap_or(""))
            } else if issue.key.is_empty() {
                "issue".into()
            } else {
                issue.key.clone()
            };
            let tip = format!(
                "Open {} in {}",
                if issue.key.is_empty() {
                    "this issue"
                } else {
                    &issue.key
                },
                if github { "GitHub" } else { "Linear" }
            );
            let services = self.state.read(cx).services().clone();
            let source = issue.source.clone();
            let view = tbtn("tb-issue-view", Tbtn::Default, TbtnSize::TITLEBAR, None, cx)
                .rounded_r(px(0.))
                .on_click(move |_, _, cx| open_issue_url(&services, &source, &url, cx))
                .child(
                    icon(if github {
                        IconName::Github
                    } else {
                        IconName::Linear
                    })
                    .size(px(13.)),
                )
                .child(style::button_label(
                    format!("View {label}"),
                    11.5,
                    weight::MEDIUM,
                ));
            return Some(
                div()
                    .id("issue-split")
                    .debug_selector(|| "issue-split".into())
                    .relative()
                    .flex()
                    .items_center()
                    .child(titled(
                        "tb-issue-view-tip",
                        tip,
                        no_drag(spec_tag(
                            view,
                            "tb-issue-view",
                            &format!("View {label}"),
                            None,
                        )),
                    ))
                    .child(no_drag(
                        div()
                            .id("issue-pick")
                            .debug_selector(|| "issue-pick".into())
                            .child(sel),
                    ))
                    .into_any_element(),
            );
        }
        if opts.is_empty() {
            return None;
        }
        Some(
            no_drag(
                div()
                    .id("issue-pick")
                    .debug_selector(|| "issue-pick".into())
                    .flex_none()
                    .child(sel),
            )
            .into_any_element(),
        )
    }
}

/// `issueUrl`: the tracker URL from the Issues list, or a GitHub `owner/repo#N` key.
pub fn issue_url(
    r: &recur_domain::IssueRef,
    issues: &[recur_services::api::AssignedIssue],
) -> String {
    if let Some(hit) = issues
        .iter()
        .find(|i| i.source == r.source && i.id == r.id)
        .map(|i| i.url.clone())
        .filter(|u| !u.is_empty())
    {
        return hit;
    }
    if r.source != "github" {
        return String::new();
    }
    let Some((repo, num)) = r.key.rsplit_once('#') else {
        return String::new();
    };
    let valid_repo = repo.split_once('/').is_some_and(|(o, n)| {
        !o.is_empty()
            && !n.is_empty()
            && !o.contains(char::is_whitespace)
            && !n.contains(['#'])
            && !n.contains(char::is_whitespace)
    });
    if !valid_repo || num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    format!("https://github.com/{repo}/issues/{num}")
}

/// `openIssueUrl`: Linear issues open in the Linear app when installed.
fn open_issue_url(
    services: &recur_services::Services,
    source: &str,
    url: &str,
    cx: &mut gpui_kit::App,
) {
    let (services, source, url) = (services.clone(), source.to_string(), url.to_string());
    cx.spawn(async move |cx| {
        if source == "linear"
            && services
                .settings
                .open_linear_in_app(&url)
                .await
                .unwrap_or(false)
        {
            return;
        }
        cx.update(|cx| cx.open_url(&url));
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_urls() {
        let r = |source: &str, id: &str, key: &str| recur_domain::IssueRef {
            source: source.into(),
            id: id.into(),
            key: key.into(),
        };
        assert_eq!(
            issue_url(&r("github", "x", "acme/web#12"), &[]),
            "https://github.com/acme/web/issues/12"
        );
        assert_eq!(issue_url(&r("linear", "x", "ENG-1"), &[]), "");
        assert_eq!(issue_url(&r("github", "x", "bad key#1"), &[]), "");
    }

    #[test]
    fn merge_labels() {
        assert_eq!(merge_method_label("squash"), Some("Squash and merge"));
        assert_eq!(merge_method_label("x"), None);
        assert_eq!(update_method_menu_label("rebase"), "Rebase");
    }
}
