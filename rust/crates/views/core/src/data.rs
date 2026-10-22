//! Loading the app's data and keeping it live (`lib/api.ts`: `loadInitial`, `wireEvents` and
//! the store helpers). Event handlers here update the shared stores only; a view that needs an
//! event's details (session deltas, PR comments, …) subscribes to `Services::subscribe` itself.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use gpui_kit::{AppContext, AsyncApp, Context, Task, WeakEntity};
use recur_domain::{
    AttentionCounts, Group, OpenPathResult, Pr, Project, Workspace, WorkspaceState,
};
use recur_services::AppEvent;
use recur_services::events::{AttentionUpdated, PrUpdated, WorkspaceUpdated};
use recur_ui::widgets::ToastKind;

use crate::pr::keep_merged_pr;
use crate::pr_commits::keep_commit_files;
use crate::state::{AppState, ShellEvent, call, toast};
use crate::workspace_nav::unique_by_id;

/// Matches the engine's `githubPRSyncMinInterval`, so a dashboard revisit skips the call.
pub const GITHUB_PR_SYNC_MIN: Duration = Duration::from_secs(30);
const BRIEFING_REFRESH_DELAY: Duration = Duration::from_millis(120);
/// Ask Recur has no workspace row: its running ids bucket here so workspace seeding and
/// pruning cannot wipe them, and they never join `workspaces_running`.
const APP_AGENTS_BUCKET: &str = "app";

/// An agent counts as running for the sidebar indicator while it is mid-turn.
pub fn is_agent_running_status(status: &str) -> bool {
    matches!(status, "working" | "directing" | "planning" | "replanning")
}

/// The synthetic `agents_running` id for an Ask Recur tab.
pub fn app_chat_agent_id(code: &str) -> String {
    format!("app-{}", if code.is_empty() { "APP" } else { code })
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn merge_synced_pr(prev: &Pr, mut p: Pr) -> Pr {
    if p.workspace_id.is_empty() {
        p.workspace_id = prev.workspace_id.clone();
    }
    if p.project_id.is_empty() {
        p.project_id = prev.project_id.clone();
    }
    if p.comments.is_none() {
        p.comments = prev.comments.clone();
    }
    if p.files.is_none() {
        p.files = prev.files.clone();
    }
    p.commits = match p.commits.take() {
        Some(c) => keep_commit_files(prev.commits.as_ref(), Some(c)),
        None => prev.commits.clone(),
    };
    p.pending_comments = prev.pending_comments.clone();
    p
}

fn clean_counts(m: Option<BTreeMap<String, i64>>) -> BTreeMap<String, i64> {
    m.unwrap_or_default()
        .into_iter()
        .filter(|(_, v)| *v > 0)
        .collect()
}

impl AppState {
    fn note_git_warning(&mut self, warning: &str, cx: &mut Context<Self>) {
        let w = warning.trim();
        if w.is_empty() || self.git_warned {
            return;
        }
        self.git_warned = true;
        toast(ToastKind::Error, w.to_string(), cx);
    }

    /// `applyAttention`.
    pub(crate) fn apply_attention(&mut self, raw: AttentionCounts) {
        self.attention = crate::state::Attention {
            unseen: raw.unseen,
            inbox_unread: raw.inbox_unread,
            dock: raw.dock,
            app_unseen: raw.app_unseen,
            by_workspace: clean_counts(raw.by_workspace),
            by_project: clean_counts(raw.by_project),
        };
    }

    // ---- workspaces ----

    /// `setWorkspacesFromList`: a list snapshot without wiping fresher live diff stats.
    pub fn set_workspaces_from_list(&mut self, list: Vec<Workspace>, cx: &mut Context<Self>) {
        let incoming = unique_by_id(&list, true)
            .into_iter()
            .filter(|w| !self.dropped_workspace_ids.contains(&w.id))
            .map(|mut w| {
                if let Some(live) = self.live_diff.get(&w.id) {
                    w.diff = *live;
                }
                w
            })
            .collect();
        self.workspaces = incoming;
        cx.notify();
    }

    /// `upsertWorkspace`: insert a workspace the UI just created. An existing row is kept:
    /// `workspace.updated` may already have patched it.
    pub fn upsert_workspace(&mut self, mut ws: Workspace, cx: &mut Context<Self>) {
        if ws.id.is_empty() || ws.archived {
            return;
        }
        self.dropped_workspace_ids.remove(&ws.id);
        if let Some(live) = self.live_diff.get(&ws.id) {
            ws.diff = *live;
        }
        if !self.workspaces.iter().any(|w| w.id == ws.id) {
            self.workspaces.insert(0, ws);
            cx.notify();
        }
    }

    /// Replace a row in place (a service call returned the fresh workspace).
    pub fn replace_workspace(&mut self, ws: Workspace, cx: &mut Context<Self>) {
        if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == ws.id) {
            *w = ws;
            cx.notify();
        }
    }

    /// Fold an update result's workspace into its row (`{ ...l, ...res.workspace }`).
    pub fn patch_workspace_after_update(&mut self, ws: Workspace, cx: &mut Context<Self>) {
        self.replace_workspace(ws, cx);
    }

    /// `linkWorkspaceIssue`: one workspace per issue, so the link moves; the Issues list
    /// refreshes once the backend assigned it.
    pub fn link_workspace_issue(
        &mut self,
        workspace_id: &str,
        issue: recur_domain::IssueRef,
        cx: &mut Context<Self>,
    ) {
        if workspace_id.is_empty() || issue.source.is_empty() || issue.id.is_empty() {
            return;
        }
        for w in self.workspaces.iter_mut() {
            if w.id == workspace_id {
                w.issue = Some(issue.clone());
            } else if w
                .issue
                .as_ref()
                .is_some_and(|i| i.source == issue.source && i.id == issue.id)
            {
                w.issue = None;
            }
        }
        cx.notify();
        let services = self.services.clone();
        let id = workspace_id.to_string();
        cx.spawn(async move |this, cx| {
            if services
                .workspaces
                .set_workspace_issue(&id, &issue)
                .await
                .is_ok()
            {
                let _ = this.update(cx, |s, cx| {
                    s.issues_stale += 1;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// `unlinkWorkspaceIssue`.
    pub fn unlink_workspace_issue(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        let services = self.services.clone();
        let id = workspace_id.to_string();
        cx.spawn(async move |this, cx| {
            if call(
                cx,
                services.workspaces.clear_workspace_issue(&id),
                "Could not unlink issue",
            )
            .await
            .is_some()
            {
                let _ = this.update(cx, |s, cx| {
                    if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == id) {
                        w.issue = None;
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Insert or replace a row (`OpenMainWorkspace`).
    pub fn put_workspace(&mut self, ws: Workspace, cx: &mut Context<Self>) {
        match self.workspaces.iter_mut().find(|w| w.id == ws.id) {
            Some(w) => *w = ws,
            None => self.workspaces.push(ws),
        }
        cx.notify();
    }

    /// `dropWorkspace`: gone from the live list (delete / discard / modal cancel). A stale
    /// list or event must not resurrect it.
    pub fn drop_workspace(&mut self, id: &str, cx: &mut Context<Self>) {
        if !id.is_empty() {
            self.dropped_workspace_ids.insert(id.to_string());
        }
        self.live_diff.remove(id);
        self.workspaces.retain(|w| w.id != id);
        cx.emit(ShellEvent::WorkspaceGone(id.to_string()));
        cx.notify();
    }

    /// `pruneWorkspaceClientState`: release per-workspace UI caches after Remove (the worktree
    /// is gone; the archived row may remain).
    pub fn prune_workspace_client_state(&mut self, id: &str, cx: &mut Context<Self>) {
        self.live_diff.remove(id);
        cx.emit(ShellEvent::WorkspaceGone(id.to_string()));
    }

    fn fetch_unknown_workspace(&mut self, id: String, cx: &mut Context<Self>) {
        if id.is_empty() || self.dropped_workspace_ids.contains(&id) {
            return;
        }
        self.fetching_workspace.insert(id.clone());
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            loop {
                let got = services.workspaces.get_workspace(&id).await;
                let again = this
                    .update(cx, |s, cx| {
                        if s.refetch_workspace.remove(&id) {
                            return true;
                        }
                        if let Ok(ws) = got
                            && !s.dropped_workspace_ids.contains(&id)
                            && !ws.archived
                        {
                            s.upsert_workspace(ws, cx);
                        }
                        false
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
            }
            let _ = this.update(cx, |s, _| s.fetching_workspace.remove(&id));
        })
        .detach();
    }

    /// `removeWorkspace` (`lib/remove-workspace.ts`): hide the row at once (leaving the detail
    /// view if it is on screen), archive it, and restore the list on failure.
    pub fn remove_workspace(&mut self, id: &str, cx: &mut Context<Self>) -> Task<bool> {
        if id.is_empty() {
            return Task::ready(false);
        }
        let viewing = self.view == crate::ViewName::Detail && self.selected_workspace_id == id;
        let original = self.workspace(id).cloned();
        if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == id) {
            *w = mark_removed(w);
        }
        if viewing {
            self.go(crate::ViewName::Briefing, None, cx);
        }
        cx.notify();
        let services = self.services.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            if let Some(removed) = call(
                cx,
                services.workspaces.remove_workspace(&id),
                "Remove failed",
            )
            .await
            {
                let _ = this.update(cx, |s, cx| {
                    s.replace_workspace(removed, cx);
                    s.prune_workspace_client_state(&id, cx);
                    toast(ToastKind::Success, "Workspace removed", cx);
                });
                return true;
            }
            match services.workspaces.list_workspaces().await {
                Ok(list) => {
                    let _ = this.update(cx, |s, cx| s.set_workspaces_from_list(list, cx));
                }
                Err(_) => {
                    if let Some(original) = original {
                        let _ = this.update(cx, |s, cx| s.upsert_workspace(original, cx));
                    }
                }
            }
            false
        })
    }

    // ---- projects ----

    /// `upsertProject`: fold one project into the live list (autosave / OpenPath).
    pub fn upsert_project(&mut self, p: Project, cx: &mut Context<Self>) {
        if p.id.is_empty() {
            return;
        }
        match self.projects.iter_mut().find(|x| x.id == p.id) {
            Some(x) => *x = p,
            None => self.projects.push(p),
        }
        cx.notify();
    }

    /// `refreshProjects`: re-stamp `isGit` (and the rest) without a full reload.
    pub fn refresh_projects(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(list) = services.projects.list_projects().await {
                let _ = this.update(cx, |s, cx| {
                    s.projects = unique_by_id(&list, true);
                    cx.notify();
                });
            }
        })
    }

    /// `setProjectHidden`: persist a project's sidebar hidden flag.
    pub fn set_project_hidden(
        &mut self,
        project_id: &str,
        hidden: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(mut p) = self.project(project_id).cloned() else {
            return Task::ready(());
        };
        p.hidden = hidden;
        let services = self.services.clone();
        let id = project_id.to_string();
        cx.spawn(async move |this, cx| {
            if let Some(saved) = call(
                cx,
                services.projects.update_project_run_config(&p),
                "Could not save project view",
            )
            .await
            {
                let _ = this.update(cx, |s, cx| {
                    if let Some(x) = s.projects.iter_mut().find(|x| x.id == id) {
                        x.hidden = saved.hidden;
                    }
                    cx.notify();
                });
            }
        })
    }

    /// Patch a project row (a sidebar toggle returned the saved project).
    pub fn update_project(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut Project),
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
            f(p);
            cx.notify();
        }
    }

    /// Replace the project order (sidebar drag reorder).
    pub fn set_projects(&mut self, list: Vec<Project>, cx: &mut Context<Self>) {
        self.projects = list;
        cx.notify();
    }

    pub fn set_project_groups(
        &mut self,
        list: Vec<recur_domain::ProjectGroup>,
        cx: &mut Context<Self>,
    ) {
        self.project_groups = list;
        cx.notify();
    }

    pub fn set_settings(&mut self, settings: recur_domain::Settings, cx: &mut Context<Self>) {
        self.settings = Some(settings);
        self.on_settings_changed(cx);
        cx.notify();
    }

    // ---- PRs ----

    /// `githubPRSyncDue`.
    pub fn github_pr_sync_due(&self) -> bool {
        self.last_github_pr_sync
            .is_none_or(|at| at.elapsed() >= GITHUB_PR_SYNC_MIN)
    }

    /// `setPRsFromSync`: rows come without commits/files (GetPR loads those), so loaded ones
    /// stay; pending comments are client-only.
    pub fn set_prs_from_sync(&mut self, list: Vec<Pr>, cx: &mut Context<Self>) {
        self.last_github_pr_sync = Some(Instant::now());
        let prev: BTreeMap<String, Pr> = std::mem::take(&mut self.prs)
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        self.prs = list
            .into_iter()
            .map(|p| match prev.get(&p.id) {
                Some(old) => merge_synced_pr(old, p),
                None => p,
            })
            .collect();
        cx.notify();
    }

    /// `mergeProjectPRsSync`: one project's rows from a sweep still in flight.
    pub fn merge_project_prs_sync(&mut self, list: Vec<Pr>, cx: &mut Context<Self>) {
        if list.is_empty() {
            return;
        }
        let mut incoming: Vec<Option<Pr>> = list.into_iter().map(Some).collect();
        for prev in self.prs.iter_mut() {
            if let Some(slot) = incoming
                .iter_mut()
                .find(|p| p.as_ref().is_some_and(|p| p.id == prev.id))
                && let Some(p) = slot.take()
            {
                *prev = merge_synced_pr(prev, p);
            }
        }
        self.prs.extend(incoming.into_iter().flatten());
        cx.notify();
    }

    /// `upsertPR`: merge a full PR, inserting unknown ones.
    pub fn upsert_pr(&mut self, mut pr: Pr, cx: &mut Context<Self>) {
        match self.prs.iter_mut().find(|p| p.id == pr.id) {
            None => self.prs.push(pr),
            Some(prev) => {
                pr.commits = keep_commit_files(
                    prev.commits.as_ref(),
                    pr.commits.take().or_else(|| prev.commits.clone()),
                );
                *prev = keep_merged_pr(Some(prev), pr);
            }
        }
        cx.notify();
    }

    pub fn update_pr(&mut self, id: &str, f: impl FnOnce(&mut Pr), cx: &mut Context<Self>) {
        if let Some(p) = self.prs.iter_mut().find(|p| p.id == id) {
            f(p);
            cx.notify();
        }
    }

    fn fetch_and_upsert_pr(&mut self, pr_id: String, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(full) = services.prs.get_pr(&pr_id).await
                && !full.id.is_empty()
            {
                let _ = this.update(cx, |s, cx| s.upsert_pr(full, cx));
            }
        })
        .detach();
    }

    fn apply_pr_updated(&mut self, d: PrUpdated, cx: &mut Context<Self>) {
        if d.pr_id.is_empty() {
            return;
        }
        let Some(prev) = self.prs.iter_mut().find(|p| p.id == d.pr_id) else {
            // Engine-created PRs are not in the store yet.
            self.fetch_and_upsert_pr(d.pr_id, cx);
            self.schedule_briefing_refresh(cx);
            return;
        };
        let mut next = prev.clone();
        macro_rules! take {
            ($($f:ident),*) => { $( if let Some(v) = d.$f { next.$f = v; } )* };
        }
        take!(
            state,
            group,
            checks_ok,
            has_checks,
            file_count,
            title,
            number,
            when,
            diff,
            behind_base,
            updating_base,
            branch,
            base,
            workspace_id,
            project_id,
            mergeable,
            merge_state_status,
            review_decision
        );
        if let Some(v) = d.review_requested {
            next.review_requested = v;
        }
        if let Some(v) = d.attention_reasons {
            next.attention_reasons = v;
        }
        if let Some(v) = d.participants {
            next.participants = v;
        }
        if let Some(v) = d.reviewers {
            next.reviewers = v;
        }
        if let Some(v) = d.allowed_merge_methods {
            next.allowed_merge_methods = v;
        }
        if let Some(v) = d.pending_comments {
            next.pending_comments = v;
        }
        if let Some(v) = d.draft {
            next.draft = v;
        }
        *prev = keep_merged_pr(Some(prev), next);
        self.schedule_briefing_refresh(cx);
        cx.notify();
    }

    // ---- agents running ----

    fn publish_agents_running(&mut self) {
        self.workspaces_running = self
            .running_agent_ids
            .iter()
            .filter(|(ws, agents)| !agents.is_empty() && *ws != APP_AGENTS_BUCKET)
            .map(|(ws, _)| ws.clone())
            .collect();
        self.agents_running = self
            .running_agent_ids
            .values()
            .flat_map(|a| a.iter().cloned())
            .collect();
    }

    fn set_agent_running(&mut self, workspace_id: &str, agent_id: &str, running: bool) {
        if workspace_id.is_empty() || agent_id.is_empty() {
            return;
        }
        self.agent_event_seq += 1;
        self.workspace_agent_event_seq
            .insert(workspace_id.to_string(), self.agent_event_seq);
        let set = match self.running_agent_ids.get_mut(workspace_id) {
            Some(set) => set,
            None if running => self
                .running_agent_ids
                .entry(workspace_id.to_string())
                .or_default(),
            None => return,
        };
        if running {
            set.insert(agent_id.to_string());
        } else {
            set.remove(agent_id);
        }
        if set.is_empty() {
            self.running_agent_ids.remove(workspace_id);
        }
        self.publish_agents_running();
    }

    fn clear_workspace_agents_running(&mut self, workspace_id: &str) {
        if self.running_agent_ids.remove(workspace_id).is_none() {
            return;
        }
        self.workspace_agent_event_seq.remove(workspace_id);
        self.publish_agents_running();
    }

    /// `seedAgentsRunning`: hydrate the running dots from persisted agent status so a reload
    /// mid-turn shows them before the next event.
    fn seed_agents_running(&mut self, list: &[Workspace], cx: &mut Context<Self>) {
        self.agents_running_seed_gen += 1;
        let gen_ = self.agents_running_seed_gen;
        let started_at = self.agent_event_seq;
        let ids: Vec<String> = list
            .iter()
            .filter(|w| !w.archived)
            .map(|w| w.id.clone())
            .collect();
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let mut snaps = Vec::with_capacity(ids.len());
            for id in ids {
                let agents = services
                    .agents
                    .list_loop_agents(&id)
                    .await
                    .unwrap_or_default();
                snaps.push((id, agents));
            }
            let _ = this.update(cx, |s, cx| {
                if gen_ != s.agents_running_seed_gen {
                    return;
                }
                for (id, agents) in snaps {
                    if s.workspace_agent_event_seq.get(&id).copied().unwrap_or(0) > started_at {
                        continue;
                    }
                    let set: BTreeSet<String> = agents
                        .iter()
                        .filter(|a| is_agent_running_status(&a.status) && !a.id.is_empty())
                        .map(|a| a.id.clone())
                        .collect();
                    if set.is_empty() {
                        s.running_agent_ids.remove(&id);
                    } else {
                        s.running_agent_ids.insert(id, set);
                    }
                }
                let current: BTreeSet<&str> = s
                    .workspaces
                    .iter()
                    .filter(|w| !w.archived)
                    .map(|w| w.id.as_str())
                    .collect();
                let gone: Vec<String> = s
                    .running_agent_ids
                    .keys()
                    .filter(|id| *id != APP_AGENTS_BUCKET && !current.contains(id.as_str()))
                    .cloned()
                    .collect();
                for id in gone {
                    s.running_agent_ids.remove(&id);
                    s.workspace_agent_event_seq.remove(&id);
                }
                s.publish_agents_running();
                cx.notify();
            });
        })
        .detach();
    }

    // ---- briefing, inbox, usage ----

    fn schedule_briefing_refresh(&mut self, cx: &mut Context<Self>) {
        if self.briefing_refresh.is_some() {
            return;
        }
        let services = self.services.clone();
        self.briefing_refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BRIEFING_REFRESH_DELAY).await;
            let _ = this.update(cx, |s, _| s.briefing_refresh = None);
            if let Ok(b) = services.briefing.get_briefing().await {
                let _ = this.update(cx, |s, cx| {
                    s.briefing = b;
                    cx.notify();
                });
            }
        }));
    }

    fn reload_inbox(&mut self, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(ib) = services.inbox.get_inbox().await {
                let _ = this.update(cx, |s, cx| {
                    s.inbox = ib;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Replace the inbox (a view marked items read).
    pub fn set_inbox(&mut self, inbox: Vec<recur_domain::InboxItem>, cx: &mut Context<Self>) {
        self.inbox = inbox;
        cx.notify();
    }

    /// `prefetchProviderUsage`: fill the sidebar meter after first paint (collect can take
    /// seconds and may prompt the keychain).
    pub fn prefetch_provider_usage(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(s) = services.settings.get_provider_usage().await {
                let _ = this.update(cx, |st, cx| {
                    st.provider_usage = Some(s);
                    cx.notify();
                });
            }
        })
    }

    /// `refreshProviderUsage`: force a collect.
    pub fn refresh_provider_usage(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(s) = services.settings.refresh_provider_usage().await {
                let _ = this.update(cx, |st, cx| {
                    st.provider_usage = Some(s);
                    cx.notify();
                });
            }
        })
    }

    // ---- boot ----

    /// `loadInitial`: pull the dataset into the stores. Resolves once the first paint has what
    /// it needs; the slower extras (usage, GitHub sync, running dots, briefing, prompts) follow.
    pub fn load_initial(&mut self, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            let loaded = async {
                let pj = services.projects.list_projects().await?;
                let pg = services.projects.list_project_groups().await?;
                let lp = services.workspaces.list_workspaces().await?;
                let pr = services.prs.list_prs().await?;
                let ib = services.inbox.get_inbox().await?;
                let pol = services.policy.get_policy().await?;
                let st = services.settings.get_settings().await?;
                let res = services.settings.get_resource().await?;
                let att = services.workspaces.attention_counts().await.ok();
                let dock = services
                    .terminal
                    .list_terminal_dock_chrome()
                    .await
                    .unwrap_or_default();
                let label = services
                    .settings
                    .get_instance_label()
                    .await
                    .unwrap_or_default();
                Ok::<_, recur_services::Error>((pj, pg, lp, pr, ib, pol, st, res, att, dock, label))
            }
            .await;
            let (pj, pg, lp, pr, ib, pol, st, res, att, dock, label) = match loaded {
                Ok(v) => v,
                Err(err) => {
                    let msg = format!("Failed to load Recur: {}", err.message);
                    let _ = this.update(cx, |s, cx| {
                        toast(ToastKind::Error, msg.clone(), cx);
                        s.booted = true;
                        cx.notify();
                    });
                    return Err(err.message);
                }
            };
            let _ = this.update(cx, |s, cx| {
                s.projects = unique_by_id(&pj, true);
                s.project_groups = unique_by_id(&pg, true);
                s.set_workspaces_from_list(lp.clone(), cx);
                s.prs = pr;
                s.inbox = ib;
                if let Some(att) = att {
                    s.apply_attention(att);
                }
                s.policy = Some(pol);
                s.settings = Some(st);
                s.on_settings_changed(cx);
                s.instance_label = label.trim().to_string();
                s.note_git_warning(&res.git_warning, cx);
                s.resource = Some(res);
                s.dock.apply(&dock);
                s.prefetch_provider_usage(cx).detach();
                if s.github_pr_sync_due() {
                    s.sync_github_prs(cx);
                }
                s.seed_agents_running(&lp, cx);
                s.schedule_briefing_load(cx);
                s.reload_pending_prompts(cx);
                s.booted = true;
                cx.notify();
            });
            Ok(())
        })
    }

    fn sync_github_prs(&mut self, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(list) = services.prs.sync_github_prs().await {
                let _ = this.update(cx, |s, cx| s.set_prs_from_sync(list, cx));
            }
        })
        .detach();
    }

    fn schedule_briefing_load(&mut self, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(b) = services.briefing.get_briefing().await {
                let _ = this.update(cx, |s, cx| {
                    s.briefing = b;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Permission and ask_user prompts only arrive as events, so a reload mid-turn rebuilds
    /// the queues.
    fn reload_pending_prompts(&mut self, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this, cx| {
            if let Ok(q) = services.permissions.list_pending_permissions().await {
                let _ = this.update(cx, |s, cx| {
                    s.permission_requests = q;
                    cx.notify();
                });
            }
            if let Ok(q) = services.questions.list_pending_questions().await {
                let _ = this.update(cx, |s, cx| {
                    s.user_questions = q;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// `wireEvents`: subscribe to the backend and keep the shared stores live. Idempotent.
    pub fn wire_events(&mut self, cx: &mut Context<Self>) {
        if self.event_bridge.is_some() {
            return;
        }
        let mut events = self.services.subscribe();
        self.event_bridge = Some(cx.spawn(
            async move |this: WeakEntity<AppState>, cx: &mut AsyncApp| {
                while let Some(ev) = events.recv().await {
                    if this.update(cx, |s, cx| s.apply_event(ev, cx)).is_err() {
                        break;
                    }
                }
            },
        ));
    }

    /// One backend event's effect on the shared stores.
    pub fn apply_event(&mut self, ev: AppEvent, cx: &mut Context<Self>) {
        match ev {
            AppEvent::WorkspaceEvent(e) => {
                if e.workspace_id.is_empty() {
                    return;
                }
                if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == e.workspace_id) {
                    w.last = strip_tags(&e.event.text);
                    cx.notify();
                }
            }
            AppEvent::WorkspaceUpdated(d) => self.apply_workspace_updated(*d, cx),
            AppEvent::WorkspaceDeleted(d) => {
                if d.workspace_id.is_empty() {
                    return;
                }
                self.drop_workspace(&d.workspace_id, cx);
                self.clear_workspace_agents_running(&d.workspace_id);
                self.schedule_briefing_refresh(cx);
            }
            AppEvent::WorkspaceRemoved(d) => {
                if d.workspace_id.is_empty() {
                    return;
                }
                self.prune_workspace_client_state(&d.workspace_id, cx);
                self.clear_workspace_agents_running(&d.workspace_id);
                self.schedule_briefing_refresh(cx);
                cx.notify();
            }
            AppEvent::AgentUpdated(d) => {
                let app = d.surface == "app"
                    || (d.workspace_id.is_empty() && d.agent_id.starts_with("app-"));
                if d.workspace_id.is_empty() && !app {
                    return;
                }
                let bucket = if app {
                    APP_AGENTS_BUCKET
                } else {
                    &d.workspace_id
                };
                self.set_agent_running(bucket, &d.agent_id, is_agent_running_status(&d.status));
                if !app {
                    self.schedule_briefing_refresh(cx);
                }
                cx.notify();
            }
            AppEvent::ResourceUpdated(r) => {
                self.note_git_warning(&r.git_warning, cx);
                self.resource = Some(r);
                cx.notify();
            }
            AppEvent::NotificationOpen(target) => self.open_notification(&target, cx),
            AppEvent::InboxUpdated(_) => {
                self.reload_inbox(cx);
                let services = self.services.clone();
                cx.spawn(async move |this, cx| {
                    if let Ok(att) = services.workspaces.attention_counts().await {
                        let _ = this.update(cx, |s, cx| {
                            s.apply_attention(att);
                            cx.notify();
                        });
                    }
                })
                .detach();
                self.schedule_briefing_refresh(cx);
            }
            AppEvent::AttentionUpdated(d) => {
                let AttentionUpdated { counts, .. } = d;
                self.apply_attention(AttentionCounts {
                    by_workspace: Some(counts.by_workspace.unwrap_or_default()),
                    by_project: Some(counts.by_project.unwrap_or_default()),
                    ..counts
                });
                self.schedule_briefing_refresh(cx);
                cx.notify();
            }
            AppEvent::PrUpdated(d) => self.apply_pr_updated(*d, cx),
            AppEvent::PrsSynced(d) => self.merge_project_prs_sync(d.prs.unwrap_or_default(), cx),
            AppEvent::PrRemoved(d) => {
                if d.pr_id.is_empty() {
                    return;
                }
                self.prs.retain(|p| p.id != d.pr_id);
                cx.notify();
            }
            AppEvent::PermissionRequested(d) => {
                if d.id.is_empty() {
                    return;
                }
                let loop_surface = d.surface == "loop";
                if !self.permission_requests.iter().any(|r| r.id == d.id) {
                    self.permission_requests.push(d);
                }
                if loop_surface {
                    self.reload_inbox(cx);
                }
                self.schedule_briefing_refresh(cx);
                cx.notify();
            }
            AppEvent::PermissionResolved(d) => {
                if d.id.is_empty() {
                    return;
                }
                self.permission_requests.retain(|r| r.id != d.id);
                self.reload_inbox(cx);
                self.schedule_briefing_refresh(cx);
                cx.notify();
            }
            AppEvent::QuestionAsked(d) => {
                if d.question.id.is_empty() {
                    return;
                }
                if !self.user_questions.iter().any(|q| q.id == d.question.id) {
                    self.user_questions.push(d.question);
                }
                self.reload_inbox(cx);
                self.schedule_briefing_refresh(cx);
                cx.notify();
            }
            AppEvent::QuestionAnswered(d) => {
                if d.id.is_empty() {
                    return;
                }
                self.user_questions.retain(|q| q.id != d.id);
                self.reload_inbox(cx);
                self.schedule_briefing_refresh(cx);
                cx.notify();
            }
            AppEvent::EscalationRaised(d) => {
                if d.workspace_id.is_empty() {
                    return;
                }
                if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == d.workspace_id) {
                    w.group = Group::Needs;
                    w.state = WorkspaceState::Escalated;
                    w.status_label = "Needs you".into();
                    w.phase = "asking a question".into();
                }
                self.reload_inbox(cx);
                self.schedule_briefing_refresh(cx);
                cx.notify();
            }
            AppEvent::EscalationResolved(d) => {
                if d.workspace_id.is_empty() {
                    return;
                }
                if d.open != Some(true)
                    && let Some(w) = self.workspaces.iter_mut().find(|w| w.id == d.workspace_id)
                {
                    let escalated = w.state.as_str() == "escalated";
                    w.group = Group::Active;
                    if escalated {
                        w.state = WorkspaceState::Coding;
                        w.status_label = "Working".into();
                        w.phase = "resuming".into();
                    }
                }
                self.reload_inbox(cx);
                self.schedule_briefing_refresh(cx);
                cx.notify();
            }
            AppEvent::AppFocused(d) => cx.emit(ShellEvent::AppFocused(d.workspace_id)),
            AppEvent::ProviderUsage(snapshot) => {
                let connected_cursor = snapshot.providers.as_ref().is_some_and(|ps| {
                    ps.iter()
                        .any(|p| p.id == "cursor" && p.source_id == "cursor_connect")
                });
                self.provider_usage = Some(snapshot);
                let has_key = self
                    .settings
                    .as_ref()
                    .is_some_and(|s| !s.cursor_api_key.is_empty());
                if connected_cursor && !has_key {
                    let services = self.services.clone();
                    cx.spawn(async move |this, cx| {
                        if let Ok(s) = services.settings.get_settings().await {
                            let _ = this.update(cx, |st, cx| st.set_settings(s, cx));
                        }
                    })
                    .detach();
                }
                cx.notify();
            }
            AppEvent::FindingsUpdated(_) | AppEvent::BriefingUpdated(_) => {
                self.schedule_briefing_refresh(cx)
            }
            AppEvent::SystemTheme(dark) => self.set_native_dark(dark, cx),
            AppEvent::AppOpen(res) => self.apply_open_path_result(res, cx),
            AppEvent::MenuAction(action) => self.dispatch_menu_action(action, cx),
            AppEvent::Term(e) => self.on_term_event(&e, cx),
            _ => {}
        }
    }

    fn apply_workspace_updated(&mut self, d: WorkspaceUpdated, cx: &mut Context<Self>) {
        if !d.workspace_id.is_empty()
            && let Some(diff) = d.diff
        {
            self.live_diff.insert(d.workspace_id.clone(), diff);
        }
        // An engine-created row (fan-out child, scheduled prompt) has no client create path:
        // fetch it once so it appears without a reload.
        if !d.workspace_id.is_empty() && self.workspace(&d.workspace_id).is_none() {
            if self.fetching_workspace.contains(&d.workspace_id) {
                self.refetch_workspace.insert(d.workspace_id.clone());
            } else {
                self.fetch_unknown_workspace(d.workspace_id.clone(), cx);
            }
        }
        if let Some(w) = self.workspaces.iter_mut().find(|w| w.id == d.workspace_id) {
            macro_rules! take {
                ($($f:ident),*) => { $( if let Some(v) = d.$f.clone() { w.$f = v; } )* };
            }
            take!(
                state,
                phase,
                iteration,
                archived,
                group,
                status_label,
                progress,
                goal,
                short,
                definition_of_done,
                branch,
                base_branch,
                diff,
                behind_base,
                worktree_path,
                port,
                last_error,
                host_pr_id,
                loop_chat_code,
                schedule_id
            );
            if let Some(c) = d.convergence.clone() {
                w.convergence = Some(c);
            }
            if let Some(p) = d.plan.clone() {
                w.plan = Some(p);
            }
        }
        if !d.workspace_id.is_empty()
            && let Some(code) = d.focus_chat_code.as_ref().filter(|c| !c.is_empty())
        {
            self.pending_chat_focus = Some(crate::state::ChatRef {
                workspace_id: d.workspace_id.clone(),
                code: code.clone(),
            });
        }
        self.schedule_briefing_refresh(cx);
        cx.notify();
    }

    /// `notification.open`: mark the target's inbox rows read and route to it.
    pub fn open_notification(&mut self, target: &str, cx: &mut Context<Self>) {
        let unread: Vec<String> = self
            .inbox
            .iter()
            .filter(|i| i.target == target && i.unread)
            .map(|i| i.id.clone())
            .collect();
        for id in unread {
            let services = self.services.clone();
            cx.background_spawn(async move {
                let _ = services.inbox.mark_inbox_read(&id).await;
            })
            .detach();
        }
        for i in self.inbox.iter_mut().filter(|i| i.target == target) {
            i.unread = false;
        }
        self.route_target(target, cx);
        cx.notify();
    }

    /// `applyOpenPathResult`: fold an `OpenPath` result in and open its workspace.
    pub fn apply_open_path_result(&mut self, res: OpenPathResult, cx: &mut Context<Self>) {
        if let Some(p) = res.project.as_deref() {
            self.upsert_project(p.clone(), cx);
        }
        if let Some(w) = res.workspace.as_deref() {
            self.upsert_workspace(w.clone(), cx);
        }
        self.refresh_projects(cx).detach();
        let ws = res.workspace_id.clone();
        cx.emit(ShellEvent::PathOpened(Box::new(res.clone())));
        if ws.is_empty() {
            return;
        }
        let jump = if res.file.trim().is_empty() {
            res.reveal.trim()
        } else {
            res.file.trim()
        };
        if !jump.is_empty() {
            self.observatory_jump = Some(crate::state::ObsJump {
                path: jump.to_string(),
                workspace_id: ws.clone(),
                ..Default::default()
            });
        }
        self.open_workspace(&ws, false, cx);
    }

    /// A Finder drop landed on the window: allowlist it and publish `files.dropped`.
    pub fn files_dropped(&mut self, files: Vec<String>, cx: &mut Context<Self>) {
        if files.is_empty() {
            return;
        }
        if let Some(hook) = &self.drop_hook {
            hook(&files);
        }
        self.services.emit(AppEvent::FilesDropped(
            recur_services::events::FilesDropped {
                files,
                ..Default::default()
            },
        ));
        cx.notify();
    }
}

/// `markRemoved`: archived, no worktree, no port.
fn mark_removed(w: &Workspace) -> Workspace {
    Workspace {
        archived: true,
        worktree_path: String::new(),
        port: 0,
        ..w.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags() {
        assert_eq!(strip_tags("<b>Coder</b> wrote <i>x</i>"), "Coder wrote x");
    }

    #[test]
    fn agent_ids() {
        assert!(is_agent_running_status("directing"));
        assert!(!is_agent_running_status("idle"));
        assert_eq!(app_chat_agent_id(""), "app-APP");
        assert_eq!(app_chat_agent_id("AP2"), "app-AP2");
    }

    #[test]
    fn synced_prs_keep_loaded_detail() {
        let prev = Pr {
            id: "1".into(),
            workspace_id: "ws".into(),
            comments: Some(vec![]),
            ..Pr::default()
        };
        let p = merge_synced_pr(
            &prev,
            Pr {
                id: "1".into(),
                title: "fresh".into(),
                ..Pr::default()
            },
        );
        assert_eq!(p.workspace_id, "ws");
        assert_eq!(p.title, "fresh");
        assert!(p.comments.is_some());
    }
}
