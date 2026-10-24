//! `WorkspaceDetail.svelte`: the detail route. It binds one [`WorkspaceView`] to the selected
//! workspace (or, in PR detail, to the PR's agent workspace, so a turn survives the switch),
//! keeps the previous one mounted as a parked hot shell, starts a PR's chat host, and leaves
//! a stale selection for the Briefing.

use recur_ui::guard::Guarded;
use std::collections::{HashMap, HashSet};

use gpui_kit::{
    AppContext as _, BorrowAppContext as _, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Subscription, Window, div,
};
use recur_services::AppEvent;
use recur_view_chat::heap::ChatHeapCache;
use recur_view_chat::hoist::{HoistState, hoist_cache_bust_codes};
use recur_view_core::pr::pr_agent_workspace_id;
use recur_view_core::{AppState, AppliedEvent, ViewContext, ViewName, call};
use recur_view_observatory::hot_shells::{live_workspace_ids, touch_hot_shells};

use crate::view::{WorkspaceProps, WorkspaceView};

/// The detail route the shell built, for PR detail's agent dock.
struct CurrentDetail(gpui_kit::WeakEntity<WorkspaceDetail>);

impl gpui_kit::Global for CurrentDetail {}

pub struct WorkspaceDetail {
    vcx: ViewContext,
    keep_id: String,
    hot: Vec<String>,
    views: HashMap<String, Entity<WorkspaceView>>,
    host_attempt: String,
    _subs: Vec<Subscription>,
}

impl WorkspaceDetail {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Subscribed before any WorkspaceView, so the hoist bookkeeping api.ts did ahead of its
        // `onChatsChanged` listeners runs first.
        let subs = vec![
            cx.guarded_subscribe(&vcx.state, |_, _, ev: &AppliedEvent, cx| {
                on_hoist(&ev.0, cx)
            }),
            cx.guarded_observe_in(&vcx.state, window, |d, _, window, cx| d.sync(window, cx)),
        ];
        let mut d = WorkspaceDetail {
            vcx: vcx.clone(),
            keep_id: String::new(),
            hot: Vec::new(),
            views: HashMap::new(),
            host_attempt: String::new(),
            _subs: subs,
        };
        cx.set_global(CurrentDetail(cx.entity().downgrade()));
        d.sync(window, cx);
        d
    }

    /// The most recently built detail route (the shell builds one per window).
    pub fn current(cx: &gpui_kit::App) -> Option<Entity<WorkspaceDetail>> {
        cx.try_global::<CurrentDetail>()?.0.upgrade()
    }

    fn state(&self) -> &Entity<AppState> {
        &self.vcx.state
    }

    /// The workspace view on screen, if any (PR detail renders its agent dock).
    pub fn bound_view(&self, cx: &gpui_kit::App) -> Option<Entity<WorkspaceView>> {
        let id = self.bound_id(cx);
        self.views.get(&id).cloned()
    }

    fn pr_workspace_id(&self, cx: &gpui_kit::App) -> String {
        let s = self.state().read(cx);
        if s.view != ViewName::PrDetail {
            return String::new();
        }
        s.prs
            .iter()
            .find(|p| p.id == s.selected_pr_id)
            .map(|pr| pr_agent_workspace_id(&pr.id, &pr.workspace_id, &s.workspaces))
            .unwrap_or_default()
    }

    /// `boundWsId`: PR detail binds only to the PR's linked worktree or chat host; otherwise
    /// the selection, else the last one kept.
    fn bound_id(&self, cx: &gpui_kit::App) -> String {
        let s = self.state().read(cx);
        let live = live_workspace_ids(&s.workspaces);
        let id = if s.view == ViewName::PrDetail {
            self.pr_workspace_id(cx)
        } else if !s.selected_workspace_id.is_empty() {
            s.selected_workspace_id.clone()
        } else {
            self.keep_id.clone()
        };
        if live.contains(&id) {
            id
        } else {
            String::new()
        }
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (view, selected) = {
            let s = self.state().read(cx);
            (s.view, s.selected_workspace_id.clone())
        };
        let live: HashSet<String> = live_workspace_ids(&self.state().read(cx).workspaces);
        let target = if view == ViewName::PrDetail {
            self.pr_workspace_id(cx)
        } else {
            selected.clone()
        };
        if !target.is_empty() && live.contains(&target) {
            self.keep_id = target;
        }
        let bound = self.bound_id(cx);
        let next = touch_hot_shells(&self.hot, &bound, None, Some(&live));
        if next != self.hot {
            self.hot = next;
        }
        self.views.retain(|id, _| self.hot.contains(id));
        // Park the outgoing shell before the incoming one claims the menu and watcher.
        let mut order = self.hot.clone();
        order.sort_by_key(|id| *id == bound);
        for id in order {
            let active = id == bound;
            match self.views.get(&id) {
                Some(v) => v.update(cx, |v, cx| v.set_shell_active(active, window, cx)),
                None => {
                    let vcx = self.vcx.clone();
                    let props = WorkspaceProps {
                        workspace_id: id.clone(),
                        shell_active: active,
                    };
                    let v = cx.new(|cx| WorkspaceView::new(&vcx, props, window, cx));
                    self.views.insert(id, v);
                }
            }
        }
        self.ensure_pr_chat_host(cx);
        // A stale or removed selection bails out instead of rendering blank.
        if view == ViewName::Detail && (selected.is_empty() || !live.contains(&selected)) {
            self.state()
                .update(cx, |s, cx| s.go(ViewName::Briefing, None, cx));
        }
        cx.notify();
    }

    /// PR detail without a live chat workspace (or with a leftover host beside a linked one)
    /// starts the PR's chat host once.
    fn ensure_pr_chat_host(&mut self, cx: &mut Context<Self>) {
        let s = self.state().read(cx);
        if s.view != ViewName::PrDetail {
            self.host_attempt.clear();
            return;
        }
        let Some(pr) = s.prs.iter().find(|p| p.id == s.selected_pr_id).cloned() else {
            return;
        };
        if pr.id.is_empty()
            || pr.state == "Merged"
            || pr.state == "Closed"
            || pr.group.as_str() == "merged"
        {
            return;
        }
        let leftover_host = s
            .workspaces
            .iter()
            .any(|w| !w.archived && w.host_pr_id.trim() == pr.id);
        let linked_live = s.workspaces.iter().any(|w| {
            !w.archived && w.id == pr.workspace_id.trim() && w.host_pr_id.trim().is_empty()
        });
        if !(self.pr_workspace_id(cx).is_empty() || leftover_host && linked_live) {
            return;
        }
        if self.host_attempt == pr.id {
            return;
        }
        self.host_attempt = pr.id.clone();
        let (svc, state) = (self.vcx.services.clone(), self.vcx.state.clone());
        cx.spawn(async move |_, cx| {
            let ws = call(
                cx,
                svc.prs.ensure_pr_chat_host(&pr.id),
                "Could not start pull request chat",
            )
            .await;
            if let Some(ws) = ws.filter(|w| !w.id.is_empty()) {
                state.update(cx, |s, cx| s.upsert_workspace(ws, cx));
            }
        })
        .detach();
    }
}

/// `noteHoistDestBust` + `migrateEvictedHoistDrafts` for a hoist (`workspace.updated` with
/// `chatsChanged` from another workspace).
fn on_hoist(ev: &AppEvent, cx: &mut gpui_kit::App) {
    let AppEvent::WorkspaceUpdated(u) = ev else {
        return;
    };
    if !u.chats_changed || u.workspace_id.is_empty() {
        return;
    }
    let Some(from) = u.from_workspace_id.as_deref().filter(|f| !f.is_empty()) else {
        return;
    };
    let remap = u.code_remap.clone().unwrap_or_default();
    let moved = u.moved_codes.clone().unwrap_or_default();
    let bust = hoist_cache_bust_codes(&[], Some(&remap), &moved);
    HoistState::global(cx).note_dest_bust(&u.workspace_id, &bust);
    let to = u.workspace_id.clone();
    let from = from.to_string();
    cx.update_global::<HoistState, _>(|h, cx| {
        h.migrate_evicted_drafts(ChatHeapCache::global(cx), &from, &to, Some(&remap))
    });
}

impl WorkspaceDetail {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let bound = self.bound_id(cx);
        let mut root = div()
            .id("v-detail")
            .debug_selector(|| "v-detail".into())
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0();
        // Parked shells stay laid out at the live slot's size, hidden (`#wsv-park`). They paint
        // first: GPUI still hit-tests an invisible subtree, and `visibility: hidden` takes no
        // pointer events, so the live shell must be on top.
        let mut order: Vec<&String> = self.hot.iter().filter(|id| **id != bound).collect();
        order.extend(self.hot.iter().filter(|id| **id == bound));
        for id in order {
            let Some(v) = self.views.get(id).cloned() else {
                continue;
            };
            if *id == bound {
                root = root.child(div().flex_1().min_h_0().flex().flex_col().child(v));
            } else {
                // A parked shell keeps running its effects (queues, seeds, live events); its layout
                // is cached until it notifies.
                v.update(cx, |v, cx| v.sync(window, cx));
                root = root.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .invisible()
                        .child(v.cached(gpui_kit::StyleRefinement::default().size_full())),
                );
            }
        }
        root
    }
}

impl Render for WorkspaceDetail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
