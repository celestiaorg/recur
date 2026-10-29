//! `PRTitlebarActions.svelte`: PR detail's review and merge actions leading `.tb-right`
//! (Open in GitHub, Send back, Update with base, Pull / Push / Force push, the pending chip,
//! Approve / Request changes, draft state, Fix checks, and the merge split).

use recur_ui::guard::Guarded;
use recur_view_core::{ObserveState as _, Topics};
use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::{
    AnyElement, App, Bounds, Context, Entity, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Task, Window, deferred, div, px,
};
use recur_domain::{Pr, PrSource, WorkspaceLandState};
use recur_engine::view::pr_merge_action::{
    MERGE_METHODS, MergeAction, MergeActionKind, UPDATE_BASE_METHODS, allowed_merge_methods,
    can_approve_pr_review, can_set_pr_draft, pick_allowed_merge_method, pr_merge_action,
    resolve_update_base_method, short_base_name, show_pr_update, update_with_base_label,
};
use recur_services::AppEvent;
use recur_ui::fonts::weight;
use recur_ui::shadow::{css_box_shadow, with_shadow};
use recur_ui::style::{self, Rbtn, Tbtn, TbtnSize, rbtn, tbtn};
use recur_ui::text::{TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::ToastKind;
use recur_ui::{IconName, Theme, color::rgba, icon};
use recur_view_core::land::{Land, LandCache};
use recur_view_core::modes::ws_has_run;
use recur_view_core::pr::{PrLifecycle, keep_merged_pr, pr_lifecycle};
use recur_view_core::{AppState, AppliedEvent, PrReviewKind, ViewContext, ViewName, call, toast};

use crate::detail::PrDetail;
use crate::style::spin_deg;
use crate::ui::{PrTab, PrUi, ui};

const LAND_FS_DEBOUNCE: Duration = Duration::from_millis(50);

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

/// `githubPRHrefFromCheck`: the PR page from a check run URL on the same repo.
fn github_pr_href_from_check(url: &str, n: i64) -> String {
    let Ok(u) = url::Url::parse(url) else {
        return String::new();
    };
    let parts: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return String::new();
    }
    format!(
        "{}/{}/{}/pull/{n}",
        u.origin().ascii_serialization(),
        parts[0],
        parts[1]
    )
}

/// `githubHref`: the stored URL, else one derived from a check run.
pub fn github_href(pr: &Pr) -> String {
    if pr.source != PrSource::GitHub || pr.number == 0 {
        return String::new();
    }
    let stored = pr.url.trim();
    if !stored.is_empty() {
        return stored.to_string();
    }
    pr.checks
        .iter()
        .flatten()
        .map(|c| github_pr_href_from_check(&c.url, pr.number))
        .find(|h| !h.is_empty())
        .unwrap_or_default()
}

fn is_merged(pr: &Pr) -> bool {
    pr.state == "Merged" || pr.group == "merged"
}

fn github_open(pr: &Pr) -> bool {
    pr.source == PrSource::GitHub
        && !matches!(
            pr_lifecycle(Some(pr)),
            Some(PrLifecycle::Merged | PrLifecycle::Closed)
        )
}

pub struct PrTitlebarActions {
    state: Entity<AppState>,
    ui: Entity<PrUi>,
    confirming_merge: bool,
    merge_menu_open: bool,
    review_menu_open: bool,
    review_kind: PrReviewKind,
    awaiting_update: bool,
    draft_busy: bool,
    merge_busy: bool,
    merge_override: Option<String>,
    update_override: Option<String>,
    update_menu_open: bool,
    land: Land,
    land_seq: u64,
    land_tracked: String,
    land_known: bool,
    refresh_key: Option<(String, bool, (i64, i64))>,
    fetch_key: String,
    fetch_gen: u64,
    fs_timer: Option<Task<()>>,
    pulling: bool,
    pushing: bool,
    sending_back: bool,
    review_pr_id: String,
    mounted: bool,
    hits: HashMap<&'static str, Bounds<Pixels>>,
    _subs: Vec<Subscription>,
}

/// What the bar reads off the PR, without its comments, commits and patches.
struct PrBrief {
    id: String,
    number: i64,
    draft: bool,
    behind_base: i64,
    pending_comments: usize,
    merge_queue_enabled: bool,
    state: String,
    group: String,
}

impl PrBrief {
    fn of(pr: &Pr) -> Self {
        PrBrief {
            id: pr.id.clone(),
            number: pr.number,
            draft: pr.draft,
            behind_base: pr.behind_base,
            pending_comments: pr.pending_comments.len(),
            merge_queue_enabled: pr.merge_queue_enabled,
            state: pr.state.clone(),
            group: pr.group.clone(),
        }
    }
}

/// What the bar derives from the store for the PR on screen.
struct Derived {
    pr: PrBrief,
    merged: bool,
    github_href: String,
    can_send_back: bool,
    land_ws: String,
    remote_busy: bool,
    busy: bool,
    land_pending_title: &'static str,
    can_pull: bool,
    can_push: bool,
    can_force_push: bool,
    action: MergeAction,
    can_draft: bool,
    is_draft: bool,
    can_approve: bool,
    show_update: bool,
    merge_keys: Vec<&'static str>,
    show_merge_menu: bool,
    merge_method: &'static str,
    merge_label: String,
    update_method: &'static str,
    update_label: String,
    update_base_name: String,
}

impl PrTitlebarActions {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = vcx.state.clone();
        let ui = ui(cx);
        let subs = vec![
            cx.observe_state_in(&state, Topics::ALL, window, |a, _, window, cx| {
                a.state_changed(window, cx)
            }),
            cx.guarded_observe(&ui, |_, _, cx| cx.notify()),
            cx.guarded_subscribe(&state, |a, _, ev: &AppliedEvent, cx| a.on_event(&ev.0, cx)),
        ];
        let review_pr_id = state.read(cx).selected_pr_id.clone();
        let mut a = PrTitlebarActions {
            state,
            ui,
            confirming_merge: false,
            merge_menu_open: false,
            review_menu_open: false,
            review_kind: PrReviewKind::Approve,
            awaiting_update: false,
            draft_busy: false,
            merge_busy: false,
            merge_override: None,
            update_override: None,
            update_menu_open: false,
            land: Land::default(),
            land_seq: 0,
            land_tracked: String::new(),
            land_known: false,
            refresh_key: None,
            fetch_key: String::new(),
            fetch_gen: 0,
            fs_timer: None,
            pulling: false,
            pushing: false,
            sending_back: false,
            review_pr_id,
            mounted: false,
            hits: HashMap::new(),
            _subs: subs,
        };
        a.state_changed(window, cx);
        a
    }

    fn pr(&self, cx: &App) -> Option<Pr> {
        let s = self.state.read(cx);
        s.pr(&s.selected_pr_id).cloned()
    }

    fn derive(&self, cx: &App) -> Option<Derived> {
        let s = self.state.read(cx);
        let pr = s.pr(&s.selected_pr_id)?;
        let merged = is_merged(pr);
        let pr_ws = s.workspaces.iter().find(|w| w.id == pr.workspace_id);
        let can_send_back = !merged && pr_ws.is_some_and(ws_has_run);
        let land_ws = pr.workspace_id.trim().to_string();
        let github_open = github_open(pr);
        let agent_busy = !land_ws.is_empty() && s.workspaces_running.contains(&land_ws);
        let land_pending = github_open && !land_ws.is_empty() && !self.land_known;
        let busy =
            self.awaiting_update || !pr.updating_base.is_empty() || self.pulling || self.pushing;
        let remote_busy = busy || agent_busy || land_pending;
        let l = &self.land;
        let clean = github_open && !land_ws.is_empty() && !l.dirty && !l.unmerged;
        let diverged = github_open && l.ahead_remote > 0 && l.behind_remote > 0;
        let action = pr_merge_action(Some(pr), self.awaiting_update);
        let can_draft = can_set_pr_draft(Some(pr));
        let is_draft = pr.draft || action.kind == MergeActionKind::Ready;
        let can_approve = !is_draft && can_approve_pr_review(Some(pr));
        let show_update = show_pr_update(Some(pr), &action);
        let merge_keys = allowed_merge_methods(Some(pr));
        let show_merge_menu = merge_keys.len() > 1 && !pr.merge_queue_enabled;
        let project = pr_ws
            .and_then(|w| s.project(&w.project_id))
            .or_else(|| s.project(&pr.project_id));
        let preferred = project
            .map(|p| p.merge_method.as_str().to_string())
            .filter(|m| merge_method_label(m).is_some())
            .or_else(|| {
                s.settings
                    .as_ref()
                    .map(|st| st.default_merge_method.as_str().to_string())
                    .filter(|m| merge_method_label(m).is_some())
            })
            .unwrap_or_else(|| "merge".into());
        let resolved = pick_allowed_merge_method(&preferred, &merge_keys);
        let merge_method = pick_allowed_merge_method(
            self.merge_override.as_deref().unwrap_or(resolved),
            &merge_keys,
        );
        let merge_label = if pr.merge_queue_enabled {
            "Add to merge queue".to_string()
        } else {
            merge_method_label(merge_method)
                .unwrap_or("Merge")
                .to_string()
        };
        let resolved_update = resolve_update_base_method(
            project.map(|p| p.update_base_method.as_str()),
            s.settings
                .as_ref()
                .map(|st| st.default_update_base_method.as_str()),
            None,
        );
        let update_method = resolve_update_base_method(
            Some(resolved_update),
            None,
            self.update_override.as_deref(),
        );
        Some(Derived {
            merged,
            github_href: github_href(pr),
            can_send_back,
            remote_busy,
            busy,
            land_pending_title: if land_pending {
                "Checking git status…"
            } else {
                ""
            },
            can_pull: clean && l.behind_remote > 0,
            can_push: clean && l.ahead_remote > 0 && l.behind_remote == 0,
            can_force_push: clean && diverged,
            action,
            can_draft,
            is_draft,
            can_approve,
            show_update,
            merge_keys,
            show_merge_menu,
            merge_method,
            merge_label,
            update_method,
            update_label: update_with_base_label(update_method, &pr.base),
            update_base_name: short_base_name(&pr.base),
            land_ws,
            pr: PrBrief::of(pr),
        })
    }

    // ---- land state ----

    fn state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (selected, pr, on) = {
            let s = self.state.read(cx);
            (
                s.selected_pr_id.clone(),
                s.pr(&s.selected_pr_id).cloned(),
                s.view == ViewName::PrDetail,
            )
        };
        // The bar mounts with PR detail on the web: entering the route starts it fresh.
        if on && !self.mounted {
            self.mount();
        }
        self.mounted = on;
        if !on {
            return;
        }
        if selected != self.review_pr_id {
            self.review_pr_id = selected;
            self.reset_for_pr(cx);
        }
        let land_ws = pr
            .as_ref()
            .map(|p| p.workspace_id.trim().to_string())
            .unwrap_or_default();
        let open = pr.as_ref().is_some_and(github_open);
        // `$effect.pre`: another workspace recalls its last snapshot until git answers.
        if land_ws != self.land_tracked {
            self.land_tracked = land_ws.clone();
            self.land_seq += 1;
            self.land = cx
                .default_global::<LandCache>()
                .get(&land_ws)
                .unwrap_or_default();
            self.land_known = false;
        }
        let diff = self
            .state
            .read(cx)
            .workspace(&land_ws)
            .map(|w| (w.diff.add, w.diff.del))
            .unwrap_or_default();
        let key = (land_ws.clone(), open, diff);
        if self.refresh_key.as_ref() != Some(&key) {
            self.refresh_key = Some(key);
            self.refresh_land(cx);
        }
        // Fetch the upstream two frames after the bar paints.
        let fetch_key = if open { land_ws } else { String::new() };
        if fetch_key != self.fetch_key {
            self.fetch_key = fetch_key.clone();
            self.fetch_gen += 1;
            if !fetch_key.is_empty() {
                let gen_ = self.fetch_gen;
                let me = cx.entity().downgrade();
                window.on_next_frame(move |window, _| {
                    window.on_next_frame(move |_, cx| {
                        let Some(me) = me.upgrade() else { return };
                        if me.read(cx).fetch_gen != gen_ {
                            return;
                        }
                        let services = me.read(cx).state.read(cx).services().clone();
                        let id = fetch_key.clone();
                        cx.spawn(async move |cx| {
                            if let Ok(st) =
                                services.workspaces.fetch_working_copy_upstream(&id).await
                            {
                                me.update(cx, |a, cx| {
                                    if a.fetch_gen == gen_ {
                                        a.land_seq += 1;
                                        a.apply_land(&st, cx);
                                    }
                                });
                            }
                        })
                        .detach();
                    });
                });
            }
        }
        cx.notify();
    }

    fn apply_land(&mut self, st: &WorkspaceLandState, cx: &mut Context<Self>) {
        self.land = Land::from(st);
        self.land_known = true;
        if !self.land_tracked.is_empty() {
            let (id, l) = (self.land_tracked.clone(), self.land.clone());
            cx.default_global::<LandCache>().put(&id, &l);
        }
        cx.notify();
    }

    /// `refreshLandState`.
    fn refresh_land(&mut self, cx: &mut Context<Self>) {
        let Some(pr) = self.pr(cx) else { return };
        let id = pr.workspace_id.trim().to_string();
        if id.is_empty() || !github_open(&pr) {
            return;
        }
        self.land_seq += 1;
        let seq = self.land_seq;
        let services = self.state.read(cx).services().clone();
        cx.spawn(async move |me, cx| {
            let res = services.workspaces.workspace_land_state(&id).await;
            let _ = me.update(cx, |a, cx| {
                if a.land_seq != seq {
                    return;
                }
                match res {
                    Ok(st) => a.apply_land(&st, cx),
                    Err(_) => {
                        a.land = Land::default();
                        a.land_known = true;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn on_event(&mut self, ev: &AppEvent, cx: &mut Context<Self>) {
        let watching =
            self.pr(cx).is_some_and(|p| github_open(&p)) && !self.land_tracked.is_empty();
        if !watching {
            return;
        }
        match ev {
            AppEvent::WorkspaceFsChanged(d) if d.workspace_id == self.land_tracked => {
                if self.fs_timer.is_some() {
                    return;
                }
                self.fs_timer = Some(cx.spawn(async move |me, cx| {
                    cx.background_executor().timer(LAND_FS_DEBOUNCE).await;
                    let _ = me.update(cx, |a, cx| {
                        a.fs_timer = None;
                        a.refresh_land(cx);
                    });
                }));
            }
            AppEvent::AppFocused(_) => self.refresh_land(cx),
            _ => {}
        }
    }

    /// A fresh component's state: land recalled from the cache and unconfirmed until git
    /// answers, nothing in flight.
    fn mount(&mut self) {
        self.confirming_merge = false;
        self.merge_menu_open = false;
        self.review_menu_open = false;
        self.update_menu_open = false;
        self.review_kind = PrReviewKind::Approve;
        self.merge_override = None;
        self.update_override = None;
        self.awaiting_update = false;
        self.draft_busy = false;
        self.merge_busy = false;
        self.pulling = false;
        self.pushing = false;
        self.sending_back = false;
        self.land_tracked.clear();
        self.refresh_key = None;
        self.fetch_key.clear();
        self.fetch_gen += 1;
        self.fs_timer = None;
    }

    /// Selecting another PR resets the menus, overrides and the review draft.
    fn reset_for_pr(&mut self, cx: &mut Context<Self>) {
        self.confirming_merge = false;
        self.merge_menu_open = false;
        self.review_menu_open = false;
        self.update_menu_open = false;
        self.review_kind = PrReviewKind::Approve;
        self.merge_override = None;
        self.update_override = None;
        self.awaiting_update = false;
        self.draft_busy = false;
        self.merge_busy = false;
        self.ui
            .update(cx, |u, cx| u.set_review_draft(String::new(), cx));
        self.state.update(cx, |s, cx| s.close_pr_review(cx));
    }

    // ---- actions ----

    fn pull(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.derive(cx) else { return };
        if d.remote_busy || !d.can_pull || d.land_ws.is_empty() {
            return;
        }
        let (id, n) = (d.land_ws, d.pr.number);
        self.pulling = true;
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        cx.spawn(async move |me, cx| {
            let res = call(
                cx,
                services.workspaces.pull_workspace_pr(&id),
                "Could not pull from the PR",
            )
            .await;
            if let Some(res) = res {
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
            let _ = me.update(cx, |a, cx| {
                a.pulling = false;
                if a.land_tracked == id {
                    a.refresh_land(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn push(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(d) = self.derive(cx) else { return };
        let allowed = if force { d.can_force_push } else { d.can_push };
        if d.remote_busy || !allowed || d.land_ws.is_empty() {
            return;
        }
        let (id, n) = (d.land_ws, d.pr.number);
        self.pushing = true;
        cx.notify();
        let services = self.state.read(cx).services().clone();
        cx.spawn(async move |me, cx| {
            let fail = if force {
                "Could not force-push to the PR"
            } else {
                "Could not push to the PR"
            };
            let pr_id = call(cx, services.workspaces.open_workspace_pr(&id, force), fail).await;
            if pr_id.is_some_and(|p| !p.is_empty()) {
                let msg = match (force, n != 0) {
                    (true, true) => format!("Force-pushed to PR #{n}"),
                    (true, false) => "Force-pushed updates to the PR".into(),
                    (false, true) => format!("Pushed to PR #{n}"),
                    (false, false) => "Pushed updates to the PR".into(),
                };
                cx.update(|cx| toast(ToastKind::Success, msg, cx));
            }
            let _ = me.update(cx, |a, cx| {
                a.pushing = false;
                if a.land_tracked == id {
                    a.refresh_land(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn send_back(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.derive(cx) else { return };
        if d.pr.id.is_empty() || self.sending_back || d.busy {
            return;
        }
        self.sending_back = true;
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let id = d.pr.id;
        cx.spawn(async move |me, cx| {
            let res = call(
                cx,
                services.prs.send_back_to_loop(&id, &[]),
                "Could not send the review to the conductor",
            )
            .await;
            if res.is_some() {
                cx.update(|cx| toast(ToastKind::Success, "Sent the review to the conductor", cx));
            }
            let _ = me.update(cx, |a, cx| {
                a.sending_back = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn update_with_base(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.derive(cx) else { return };
        if d.busy || !d.show_update {
            return;
        }
        self.awaiting_update = true;
        self.update_menu_open = false;
        self.ui.update(cx, |u, cx| u.set_tab(PrTab::Conv, cx));
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        let (id, method, base) = (d.pr.id, d.update_method, d.update_base_name);
        cx.spawn(async move |me, cx| {
            let res = call(
                cx,
                services.prs.update_with_base(&id, method),
                "Update with base failed",
            )
            .await;
            let _ = me.update(cx, |a, cx| {
                a.awaiting_update = false;
                cx.notify();
            });
            let Some(res) = res else { return };
            if res.status == "conflict" && !res.workspace.id.is_empty() {
                let ws = res.workspace.id.clone();
                state.update(cx, |s, cx| {
                    s.pr_agent_dock_open = true;
                    s.request_prompt_action(&ws, "update", "", cx);
                    cx.notify();
                });
            } else if res.status == "updated" {
                cx.update(|cx| {
                    toast(
                        ToastKind::Success,
                        format!("Updated with latest {base}"),
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    fn request_merge(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.derive(cx) else { return };
        if d.busy || self.merge_busy || d.action.kind != MergeActionKind::Merge {
            return;
        }
        if !self.confirming_merge {
            self.confirming_merge = true;
            cx.notify();
            return;
        }
        self.confirming_merge = false;
        self.merge_menu_open = false;
        let pr = d.pr;
        let (id, prev_state, prev_group) = (pr.id.clone(), pr.state.clone(), pr.group.clone());
        self.merge_busy = true;
        if !pr.merge_queue_enabled {
            self.state.update(cx, |s, cx| {
                s.update_pr(
                    &id,
                    |p| {
                        p.state = "Merged".into();
                        p.group = "merged".into();
                    },
                    cx,
                )
            });
        }
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        let method = d.merge_method;
        cx.spawn(async move |me, cx| {
            let merged = call(cx, services.prs.merge_pr(&id, method), "Merge failed").await;
            let _ = me.update(cx, |a, cx| {
                a.merge_busy = false;
                cx.notify();
            });
            if let Some(m) = merged {
                let queued = pr_lifecycle(Some(&m)) == Some(PrLifecycle::Queued);
                state.update(cx, |s, cx| {
                    s.update_pr(
                        &m.id.clone(),
                        |p| {
                            *p = if queued {
                                m
                            } else {
                                keep_merged_pr(Some(p), m)
                            };
                        },
                        cx,
                    )
                });
                let msg = if queued {
                    "Added to merge queue"
                } else {
                    "PR merged"
                };
                cx.update(|cx| toast(ToastKind::Success, msg, cx));
                return;
            }
            let full = services.prs.get_pr(&id).await.ok();
            state.update(cx, |s, cx| {
                s.update_pr(
                    &id,
                    |p| {
                        if full.as_ref().is_some_and(is_merged) {
                            p.state = "Merged".into();
                            p.group = "merged".into();
                        } else {
                            p.state = prev_state;
                            p.group = prev_group;
                        }
                    },
                    cx,
                )
            });
        })
        .detach();
    }

    fn set_draft(&mut self, draft: bool, cx: &mut Context<Self>) {
        let Some(d) = self.derive(cx) else { return };
        if d.busy || !d.can_draft || self.draft_busy {
            return;
        }
        let (id, prev) = (d.pr.id, d.pr.draft);
        self.draft_busy = true;
        self.state
            .update(cx, |s, cx| s.update_pr(&id, |p| p.draft = draft, cx));
        cx.notify();
        let services = self.state.read(cx).services().clone();
        let state = self.state.clone();
        cx.spawn(async move |me, cx| {
            let fail = if draft {
                "Convert to draft failed"
            } else {
                "Mark ready failed"
            };
            let updated = call(cx, services.prs.set_pr_draft(&id, draft), fail).await;
            let _ = me.update(cx, |a, cx| {
                a.draft_busy = false;
                cx.notify();
            });
            match updated {
                Some(mut u) => {
                    u.draft = draft;
                    state.update(cx, |s, cx| s.update_pr(&u.id.clone(), |p| *p = u, cx));
                    let msg = if draft {
                        "Converted to draft"
                    } else {
                        "Ready for review"
                    };
                    cx.update(|cx| toast(ToastKind::Success, msg, cx));
                }
                None => state.update(cx, |s, cx| s.update_pr(&id, |p| p.draft = prev, cx)),
            }
        })
        .detach();
    }

    fn open_review(&mut self, kind: PrReviewKind, cx: &mut Context<Self>) {
        self.review_kind = kind;
        self.review_menu_open = false;
        self.state.update(cx, |s, cx| s.open_pr_review(kind, cx));
        cx.notify();
    }

    fn fix_checks(&mut self, cx: &mut Context<Self>) {
        let Some(pr) = self.pr(cx) else { return };
        let state = self.state.clone();
        PrDetail::request_fix_checks(&state, &pr.id, "", cx);
    }
}

/// `.merge-menu`: under its split, right-aligned, above the body.
fn split_menu(items: Vec<AnyElement>, probe: Option<AnyElement>, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    deferred(
        div()
            .absolute()
            .top(px(26. + 6.))
            .right_0()
            .occlude()
            .child(with_shadow(
                style::card(t.rail, t.line, 8.)
                    .min_w(px(180.))
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .children(items)
                    .children(probe),
                &[css_box_shadow(0., 6., 18., 0., rgba(0, 0, 0, 0.3))],
                8.,
            )),
    )
    .with_priority(90)
    .into_any_element()
}

/// `.merge-menu-item` with its `.mm-check` column.
fn menu_item(
    id: impl Into<SharedString>,
    on: bool,
    label: &'static str,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    let id: SharedString = id.into();
    let sel = id.to_string();
    div()
        .id(id)
        .debug_selector(move || sel)
        .flex()
        .items_center()
        .gap(px(8.))
        .w_full()
        .py(px(7.))
        .px(px(10.))
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(if on { t.emerald } else { t.text })
        .hover(move |s| s.bg(t.pane_2))
        .child(
            div().flex_none().w(px(14.)).flex().justify_center().child(
                text(if on { "✓" } else { "" })
                    .size(13.)
                    .line_height(recur_ui::LineHeight::Normal)
                    .nowrap(),
            ),
        )
        .child(style::button_label(label, 12.5, weight::MEDIUM))
}

/// Spec handle on a titlebar action: `pr-act:{id}={label}`, `:off` while disabled.
fn tag(
    b: gpui_kit::Stateful<gpui_kit::Div>,
    id: &str,
    label: &str,
    off: bool,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let sel = format!("pr-act:{id}={label}{}", if off { ":off" } else { "" });
    b.debug_selector(move || sel)
}

/// Clicks inside the bar must not drag the window.
fn no_drag<E: InteractiveElement>(el: E) -> E {
    el.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

impl PrTitlebarActions {
    #[allow(clippy::too_many_arguments)]
    fn button(
        &self,
        id: &'static str,
        kind: Tbtn,
        off: bool,
        ico: IconName,
        spin: f32,
        label: impl Into<SharedString>,
        tip: Option<String>,
        on_click: Option<fn(&mut Self, &mut Context<Self>)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let backdrop = Theme::of(cx).tokens.titlebar;
        let label: SharedString = label.into();
        let mut b = tag(
            tbtn(id, kind, TbtnSize::TITLEBAR, off.then_some(backdrop), cx),
            id,
            &label,
            off,
        )
        .child(icon(ico).size(px(13.)).rotate(spin))
        .child(style::button_label(label, 11.5, weight::MEDIUM));
        if let Some(f) = on_click.filter(|_| !off) {
            b = b.on_click(cx.guarded_listener(move |a, _, _, cx| f(a, cx)));
        }
        let b = no_drag(b);
        match tip {
            Some(tip) => titled(SharedString::from(format!("{id}-tip")), tip, b).into_any_element(),
            None => b.into_any_element(),
        }
    }

    fn update_split(&mut self, d: &Derived, cx: &mut Context<Self>) -> AnyElement {
        let backdrop = Theme::of(cx).tokens.titlebar;
        let over = d.busy.then_some(backdrop);
        let tip = match d.action.kind {
            MergeActionKind::UpdateBase | MergeActionKind::Resolve => d.action.title.clone(),
            _ => {
                let n = d.pr.behind_base;
                format!(
                    "{n} commit{} on {} not in this branch",
                    if n == 1 { "" } else { "s" },
                    d.update_base_name
                )
            }
        };
        let label = if d.action.kind == MergeActionKind::Resolve {
            d.action.label.clone()
        } else {
            d.update_label.clone()
        };
        let mut main = tag(
            tbtn("pr-update", Tbtn::Solid, TbtnSize::TITLEBAR, over, cx),
            "pr-update",
            &label,
            d.busy,
        )
        .rounded_r(px(0.))
        .relative()
        .child(style::split_divider());
        if !d.busy {
            main = main.on_click(cx.guarded_listener(|a, _, _, cx| a.update_with_base(cx)));
        }
        let chev = tbtn("pr-update-chev", Tbtn::Solid, TbtnSize::TITLEBAR, None, cx)
            .debug_selector(|| "pr-update-chev".into())
            .rounded_l(px(0.))
            .px(px(5.))
            .on_click(cx.guarded_listener(|a, _, _, cx| {
                a.update_menu_open = !a.update_menu_open;
                a.merge_menu_open = false;
                a.review_menu_open = false;
                a.confirming_merge = false;
                cx.notify();
            }));
        let mut split = div()
            .id("pr-update-split")
            .relative()
            .child(probe("update", cx))
            .flex()
            .items_center()
            .child(titled(
                "pr-update-tip",
                tip,
                no_drag(
                    main.child(icon(IconName::ArrowUp).size(px(13.)))
                        .child(style::button_label(label, 11.5, weight::MEDIUM)),
                ),
            ))
            .child(no_drag(
                chev.child(icon(IconName::ChevronDown).size(px(13.))),
            ));
        if self.update_menu_open {
            let items = UPDATE_BASE_METHODS
                .iter()
                .map(|&m| {
                    menu_item(
                        format!("pr-update-{m}"),
                        d.update_method == m,
                        update_method_menu_label(m),
                        cx,
                    )
                    .on_click(cx.guarded_listener(move |a, _, _, cx| {
                        a.update_override = Some(m.to_string());
                        a.update_menu_open = false;
                        cx.notify();
                    }))
                    .into_any_element()
                })
                .collect();
            split = split
                .child(split_menu(items, Some(probe("update-menu", cx)), cx))
                .child(outside_closer("update", cx));
        }
        split.into_any_element()
    }

    fn review_split(&mut self, d: &Derived, cx: &mut Context<Self>) -> AnyElement {
        let backdrop = Theme::of(cx).tokens.titlebar;
        let over = d.busy.then_some(backdrop);
        let approve = self.review_kind == PrReviewKind::Approve;
        let (label, tip, ico) = if approve {
            ("Approve", "Approve this pull request", IconName::Check)
        } else {
            (
                "Request changes",
                "Request changes on this pull request",
                IconName::OctagonX,
            )
        };
        let mut main = tag(
            tbtn("pr-review", Tbtn::Default, TbtnSize::TITLEBAR, over, cx),
            "pr-review",
            label,
            d.busy,
        )
        .rounded_r(px(0.));
        // `.review-split .tbtn.review-chev` drops the left border; `.tb-right` pads it 5px.
        let mut chev = tbtn(
            "pr-review-chev",
            Tbtn::Default,
            TbtnSize::TITLEBAR,
            over,
            cx,
        )
        .debug_selector(|| "pr-review-chev".into())
        .rounded_l(px(0.))
        .border_l(px(0.))
        .px(px(5.));
        if !d.busy {
            let kind = self.review_kind;
            main = main.on_click(cx.guarded_listener(move |a, _, _, cx| a.open_review(kind, cx)));
            chev = chev.on_click(cx.guarded_listener(|a, _, _, cx| {
                a.review_menu_open = !a.review_menu_open;
                a.merge_menu_open = false;
                a.update_menu_open = false;
                cx.notify();
            }));
        }
        let mut split = div()
            .id("pr-review-split")
            .relative()
            .child(probe("review", cx))
            .flex()
            .items_center()
            .child(titled(
                "pr-review-tip",
                tip,
                no_drag(
                    main.child(icon(ico).size(px(13.)))
                        .child(style::button_label(label, 11.5, weight::MEDIUM)),
                ),
            ))
            .child(no_drag(
                chev.child(icon(IconName::ChevronDown).size(px(13.))),
            ));
        if self.review_menu_open {
            let items = [
                (PrReviewKind::Approve, "Approve"),
                (PrReviewKind::Request, "Request changes"),
            ]
            .into_iter()
            .map(|(k, l)| {
                let id = if k == PrReviewKind::Approve {
                    "pr-review-approve"
                } else {
                    "pr-review-request"
                };
                menu_item(id, self.review_kind == k, l, cx)
                    .on_click(cx.guarded_listener(move |a, _, _, cx| a.open_review(k, cx)))
                    .into_any_element()
            })
            .collect();
            split = split
                .child(split_menu(items, Some(probe("review-menu", cx)), cx))
                .child(outside_closer("review", cx));
        }
        split.into_any_element()
    }

    fn merge_split(&mut self, d: &Derived, cx: &mut Context<Self>) -> AnyElement {
        let backdrop = Theme::of(cx).tokens.titlebar;
        let over = d.busy.then_some(backdrop);
        let label = if self.confirming_merge {
            format!("Confirm {}", d.merge_label.to_lowercase())
        } else {
            d.merge_label.clone()
        };
        let solo = !d.show_merge_menu;
        let mut main = tag(
            rbtn("pr-merge", Rbtn::Merge, over, cx),
            "pr-merge",
            &label,
            d.busy,
        )
        .h(px(26.))
        .px(px(9.))
        .text_size(px(11.5));
        main = if solo {
            // `.merge-split.solo .rbtn.merge` cascades after the titlebar's 6px.
            main.rounded(px(7.))
        } else {
            main.rounded(px(6.))
                .rounded_r(px(0.))
                .relative()
                .child(style::split_divider())
        };
        if !d.busy {
            main = main.on_click(cx.guarded_listener(|a, _, _, cx| a.request_merge(cx)));
        }
        let mut split = div()
            .id("pr-merge-split")
            .relative()
            .flex()
            .items_center()
            .child(titled(
                "pr-merge-tip",
                d.action.title.clone(),
                no_drag(
                    main.child(icon(IconName::GitPullRequest).size(px(13.)))
                        .child(style::button_label(label, 11.5, weight::MEDIUM_PLUS)),
                ),
            ));
        if d.show_merge_menu {
            let mut chev = rbtn("pr-merge-chev", Rbtn::Merge, over, cx)
                .debug_selector(|| "pr-merge-chev".into())
                .h(px(26.))
                .rounded(px(6.))
                .rounded_l(px(0.))
                .px(px(5.));
            if !d.busy {
                chev = chev.on_click(cx.guarded_listener(|a, _, _, cx| {
                    a.merge_menu_open = !a.merge_menu_open;
                    a.confirming_merge = false;
                    a.update_menu_open = false;
                    a.review_menu_open = false;
                    cx.notify();
                }));
            }
            split = split.child(no_drag(
                chev.child(icon(IconName::ChevronDown).size(px(13.))),
            ));
            if self.merge_menu_open {
                let items = d
                    .merge_keys
                    .iter()
                    .filter(|m| MERGE_METHODS.contains(m))
                    .map(|&m| {
                        menu_item(
                            format!("pr-merge-{m}"),
                            d.merge_method == m,
                            merge_method_label(m).unwrap_or("Merge"),
                            cx,
                        )
                        .on_click(cx.guarded_listener(move |a, _, _, cx| {
                            a.merge_override = Some(m.to_string());
                            a.merge_menu_open = false;
                            a.confirming_merge = false;
                            cx.notify();
                        }))
                        .into_any_element()
                    })
                    .collect();
                split = split.child(split_menu(items, None, cx));
            }
        }
        split.into_any_element()
    }

    fn pending_chip(&mut self, n: usize, d: &Derived, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let border = t.amber.mix(0.35, t.line);
        let hover = t.amber.mix(0.55, t.line);
        let open = d.can_approve && !d.busy;
        let label = format!("{n} pending");
        let chip = no_drag(
            div()
                .id("pr-pending-chip")
                .debug_selector({
                    let l = label.clone();
                    move || format!("pr-act:pending={l}")
                })
                .flex_none()
                .flex()
                .rounded(px(999.))
                .border(px(1.))
                .border_color(Hsla::from(border))
                .bg(rgba(242, 179, 61, 0.12))
                .py(px(2.))
                .px(px(8.))
                .cursor_pointer()
                .hover(move |s| s.border_color(Hsla::from(hover)))
                .on_click(cx.guarded_listener(move |a, _, _, cx| {
                    if open {
                        let k = a.review_kind;
                        a.open_review(k, cx);
                    }
                }))
                .child(
                    text(label)
                        .mono()
                        .size(10.5)
                        .line_height(recur_ui::LineHeight::Normal)
                        .color(t.amber)
                        .nowrap(),
                ),
        );
        titled(
            "pr-pending-tip",
            "Line comments post when you submit the review",
            chip,
        )
        .into_any_element()
    }
}

/// Records `key`'s bounds each frame, for the outside-press test.
fn probe(key: &'static str, cx: &mut Context<PrTitlebarActions>) -> AnyElement {
    let me = cx.entity().downgrade();
    gpui_kit::canvas(
        move |b, _, cx| {
            let _ = me.update(cx, |a, _| {
                a.hits.insert(key, b);
            });
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// The capture-phase `pointerdown` listener that closes the update or review menu when the
/// press lands outside its split (`closest('.update-split')`); the merge menu has none.
fn outside_closer(which: &'static str, cx: &mut Context<PrTitlebarActions>) -> AnyElement {
    let me = cx.entity().downgrade();
    gpui_kit::canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let me = me.clone();
            window.on_mouse_event(move |ev: &gpui_kit::MouseDownEvent, phase, _, cx| {
                if phase != gpui_kit::DispatchPhase::Capture {
                    return;
                }
                let _ = me.update(cx, |a, cx| {
                    let menu = if which == "update" {
                        "update-menu"
                    } else {
                        "review-menu"
                    };
                    let inside = [which, menu]
                        .iter()
                        .any(|k| a.hits.get(k).is_some_and(|b| b.contains(&ev.position)));
                    if inside {
                        return;
                    }
                    match which {
                        "update" => a.update_menu_open = false,
                        _ => a.review_menu_open = false,
                    }
                    cx.notify();
                });
            });
        },
    )
    .absolute()
    .size_0()
    .into_any_element()
}

impl PrTitlebarActions {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let root = div()
            .id("pr-review-actions")
            .flex()
            .items_center()
            .gap(px(6.));
        let Some(d) = self.derive(cx) else {
            return root;
        };
        if d.github_href.is_empty() && d.merged {
            return root;
        }
        let mut out: Vec<AnyElement> = Vec::new();
        if !d.github_href.is_empty() {
            let href = d.github_href.clone();
            let b = no_drag(
                tag(
                    tbtn("pr-gh", Tbtn::Default, TbtnSize::TITLEBAR, None, cx),
                    "pr-gh",
                    "Open in GitHub",
                    false,
                )
                .on_click(move |_, _, cx| recur_content::links::open_external_url(&href, cx))
                .child(icon(IconName::Github).size(px(13.)))
                .child(style::button_label("Open in GitHub", 11.5, weight::MEDIUM)),
            );
            out.push(titled("pr-gh-tip", "Open in GitHub", b).into_any_element());
        }
        if d.can_send_back {
            let label = if self.sending_back {
                "Sending…"
            } else {
                "Send back"
            };
            out.push(self.button(
                "pr-send-back",
                Tbtn::Default,
                d.busy || self.sending_back,
                IconName::Undo,
                0.,
                label,
                Some("Talk to the loop conductor with this PR's comments and line notes".into()),
                Some(|a, cx| a.send_back(cx)),
                cx,
            ));
        }
        if !d.merged {
            if d.action.kind == MergeActionKind::Updating {
                let deg = spin_deg(window, cx);
                out.push(self.button(
                    "pr-updating",
                    Tbtn::Solid,
                    true,
                    IconName::RefreshCw,
                    deg,
                    "Updating…",
                    None,
                    None,
                    cx,
                ));
            } else if d.show_update {
                out.push(self.update_split(&d, cx));
            }
            let tip = |t: &str| {
                Some(if d.land_pending_title.is_empty() {
                    t.to_string()
                } else {
                    d.land_pending_title.to_string()
                })
            };
            if d.can_pull {
                let label = if self.pulling {
                    "Pulling…"
                } else {
                    "Pull updates"
                };
                out.push(self.button(
                    "pr-pull",
                    Tbtn::Solid,
                    d.remote_busy,
                    IconName::ArrowUp,
                    0.,
                    label,
                    tip("Bring GitHub commits into this workspace"),
                    Some(|a, cx| a.pull(cx)),
                    cx,
                ));
            }
            if d.can_push {
                let label = if self.pushing {
                    "Pushing…"
                } else {
                    "Push updates"
                };
                out.push(self.button(
                    "pr-push",
                    Tbtn::Solid,
                    d.remote_busy,
                    IconName::Upload,
                    0.,
                    label,
                    tip("Push new commits to the open pull request"),
                    Some(|a, cx| a.push(false, cx)),
                    cx,
                ));
            }
            if d.can_force_push {
                let label = if self.pushing {
                    "Force-pushing…"
                } else {
                    "Force push"
                };
                out.push(self.button(
                    "pr-force-push",
                    Tbtn::Solid,
                    d.remote_busy,
                    IconName::Upload,
                    0.,
                    label,
                    tip("Overwrite the GitHub head with this workspace (e.g. after rebase onto main). Remote-only commits will be dropped."),
                    Some(|a, cx| a.push(true, cx)),
                    cx,
                ));
            }
            let pending = d.pr.pending_comments;
            if pending > 0 {
                out.push(self.pending_chip(pending, &d, cx));
            }
            if d.can_approve {
                out.push(self.review_split(&d, cx));
            }
            if d.can_draft && !d.is_draft {
                out.push(self.button(
                    "pr-to-draft",
                    Tbtn::Default,
                    d.busy || self.draft_busy,
                    IconName::GitPullRequestDraft,
                    0.,
                    "Convert to draft",
                    Some("Convert this pull request to a draft on GitHub".into()),
                    Some(|a, cx| a.set_draft(true, cx)),
                    cx,
                ));
            }
            let title = Some(d.action.title.clone());
            match d.action.kind {
                MergeActionKind::Ready => out.push(self.button(
                    "pr-ready",
                    Tbtn::Solid,
                    d.busy || self.draft_busy,
                    IconName::GitPullRequest,
                    0.,
                    "Ready for review",
                    title,
                    Some(|a, cx| a.set_draft(false, cx)),
                    cx,
                )),
                MergeActionKind::Checks => out.push(self.button(
                    "pr-fix-checks",
                    Tbtn::Solid,
                    false,
                    IconName::Wrench,
                    0.,
                    "Fix checks",
                    title,
                    Some(|a, cx| a.fix_checks(cx)),
                    cx,
                )),
                MergeActionKind::Queued => out.push(self.button(
                    "pr-queued",
                    Tbtn::Default,
                    true,
                    IconName::GitMergeQueued,
                    0.,
                    d.action.label.clone(),
                    title,
                    None,
                    cx,
                )),
                MergeActionKind::Conflicts | MergeActionKind::Blocked => out.push(self.button(
                    "pr-blocked",
                    Tbtn::Default,
                    true,
                    IconName::CircleAlert,
                    0.,
                    d.action.label.clone(),
                    title,
                    None,
                    cx,
                )),
                MergeActionKind::Merge => out.push(self.merge_split(&d, cx)),
                _ => {}
            }
        }
        root.children(out)
    }
}

impl Render for PrTitlebarActions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_href_from_check_runs() {
        assert_eq!(
            github_pr_href_from_check("https://github.com/acme/web/actions/runs/1/job/2", 42),
            "https://github.com/acme/web/pull/42"
        );
        assert_eq!(github_pr_href_from_check("https://github.com/acme", 42), "");
        assert_eq!(github_pr_href_from_check("not a url", 42), "");
        let mut pr = Pr {
            source: PrSource::GitHub,
            number: 7,
            ..Default::default()
        };
        assert_eq!(github_href(&pr), "");
        pr.checks = Some(vec![recur_domain::Check {
            url: "https://github.com/acme/web/runs/9".into(),
            ..Default::default()
        }]);
        assert_eq!(github_href(&pr), "https://github.com/acme/web/pull/7");
        pr.url = "https://github.com/acme/web/pull/7".into();
        assert_eq!(github_href(&pr), "https://github.com/acme/web/pull/7");
    }
}
