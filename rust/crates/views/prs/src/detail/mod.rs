//! `PRDetail.svelte`: one pull request. The header (state, number, title editing, people,
//! attention, branch), the agent dock beside the tabs, and the Conversation, Commits, Files
//! changed and Checks panes with every flow the Svelte view runs: GetPR hydration and polling,
//! live comments, replies, edits, deletes, resolves, reactions, findings posted as review
//! comments and settled once submitted, commit diffs, and fix-checks.

mod checks;
mod commits;
mod conversation;
pub(crate) mod field;
use conversation::Compose;
use recur_ui::guard::Guarded;
mod files;
mod html;
mod render;
mod skeleton;
mod timeline;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use chrono::Utc;
use gpui_kit::base::input::InputState;
use gpui_kit::{
    App, AppContext as _, Bounds, ClipboardItem, Context, Entity, FocusHandle, Pixels,
    ScrollHandle, Subscription, Task, Window,
};
use recur_content::{MarkdownClass, MarkdownEvent, MarkdownView};
use recur_domain::{Commit, FileDiff, Finding, PendingLineComment, Pr, PrComment, PrSource};
use recur_engine::view::comment_thread::{
    PENDING_REPLY_PREFIX, apply_thread_resolved, can_edit_pr_comment, comment_source,
    is_dropped_comment, is_line_note, is_pending_comment, keep_pending_replies, line_thread_key,
    same_thread_reply, settle_posted_reply,
};
use recur_engine::view::pr_detail_pending::{
    pr_checks_pending, pr_commits_pending, pr_conv_pending, pr_files_pending,
};
use recur_engine::view::pr_fix_checks::{
    FixChecksStep, created_workspace_ready, flush_pending_fix_checks, request_fix_checks,
};
use recur_engine::view::pr_workspace::pr_agent_workspace_id;
use recur_forge::view::PendingClock;
use recur_services::AppEvent;
use recur_ui::widgets::ToastKind;
use recur_view_core::pr::keep_merged_pr;
use recur_view_core::pr_commits::keep_commit_files;
use recur_view_core::{
    AppState, AppliedEvent, ObsJump, ShellEvent, ViewContext, ViewName, call, toast,
};
use recur_view_review::commits::CopiedShas;
use recur_view_review::file_list::ReviewFileList;
use recur_view_review::line_note::{note_sheet, now_ms, spread};
use recur_view_review::reactions::CommentReactions;
use recur_view_review::review_diff::{
    DiffLayout, FileListMode, read_diff_layout, read_file_list_mode, read_review_sidebar,
    write_diff_layout, write_file_list_mode, write_review_sidebar,
};
use recur_view_review::stack::FileStack;
use recur_view_review::store::{ReviewStore, apply_review_holds, patch_pr_pending_comments};

use crate::findings::{
    finding_comment_body, finding_has_pending_comment, finding_ready_to_settle,
    finding_review_anchor,
};
use crate::ui::{PrTab, PrUi, ui};
use field::Field;

/// `GH_CHECKS_POLL_MS`.
const POLL: Duration = Duration::from_secs(15);
/// The thread skeleton gives up waiting for a hydrated GetPR.
const HYDRATE_GIVE_UP: Duration = Duration::from_secs(25);

/// The ⋯ menu on a conversation row (`moreMenuId`, its trigger and the menu box).
#[derive(Default)]
pub(crate) struct More {
    pub id: String,
    pub trigger: Option<Bounds<Pixels>>,
    pub menu: Option<Bounds<Pixels>>,
    pub triggers: HashMap<String, Bounds<Pixels>>,
}

pub struct PrDetail {
    pub(crate) vcx: ViewContext,
    pub(crate) ui: Entity<PrUi>,
    /// `commitCachePR`: the PR the files filters, findings and commit cache belong to.
    cache_pr: String,
    /// The PR the per-PR resets last ran for (the `$selectedPRId` effect).
    reset_pr: String,
    on_route: bool,
    pub(crate) refreshing: bool,
    pub(crate) hydrate_give_up: bool,
    give_up: Option<Task<()>>,
    poll: Option<Task<()>>,
    poll_busy: Rc<Cell<bool>>,
    findings_gen: u64,
    pub(crate) findings: Vec<Finding>,
    /// Thread entries `pr.comment` appended that GetPR has not echoed yet.
    pub(crate) live_comments: Vec<PrComment>,
    pub(crate) posting: HashSet<String>,
    pub(crate) settling: HashSet<String>,
    pub(crate) settle_failed: HashSet<String>,
    settle_for: String,
    // Editing.
    pub(crate) editing_title: bool,
    pub(crate) title_input: Entity<InputState>,
    pub(crate) editing_desc: bool,
    pub(crate) desc: Field,
    pub(crate) editing_comment: String,
    pub(crate) comment: Field,
    pub(crate) saving: String,
    pub(crate) reply_for: String,
    pub(crate) reply: Field,
    pub(crate) composer: Field,
    composer_rev: u64,
    pub(crate) more: More,
    pub(crate) collapsed_ctx: HashSet<String>,
    pub(crate) open_resolved: HashSet<String>,
    // Checks.
    pub(crate) collapsed_check_groups: HashMap<String, bool>,
    pub(crate) clocks: RefCell<HashMap<String, PendingClock>>,
    check_tick: Option<Task<()>>,
    // Commits.
    pub(crate) commit_idx: Option<usize>,
    pub(crate) commit_files: Option<Vec<FileDiff>>,
    commit_cache: HashMap<String, Vec<FileDiff>>,
    pub(crate) commit_loading: bool,
    pub(crate) commit_error: bool,
    commit_gen: u64,
    pub(crate) commit_stack: FileStack,
    pub(crate) copied: CopiedShas,
    // Files changed.
    pub(crate) diff_layout: DiffLayout,
    pub(crate) file_list_mode: FileListMode,
    pub(crate) sidebar_open: bool,
    pub(crate) stack: FileStack,
    stack_pane: Option<Entity<files::StackPane>>,
    stack_filtered_empty: bool,
    pub(crate) list: Entity<ReviewFileList>,
    pub(crate) filter: Entity<InputState>,
    pub(crate) findings_only: bool,
    pub(crate) comments_only: bool,
    pub(crate) pane_width: f32,
    pub(crate) highlight: i64,
    // Panes.
    pub(crate) conv_scroll: ScrollHandle,
    pub(crate) timeline: timeline::TimelineWindow,
    /// The conversation's items as of its last render, for the timeline's item panes.
    pub(crate) conv_items: Rc<Vec<PrComment>>,
    render_pr: Option<Option<Rc<Pr>>>,
    pub(crate) commits_scroll: ScrollHandle,
    pub(crate) checks_scroll: ScrollHandle,
    pub(crate) checks_box_scroll: ScrollHandle,
    pub(crate) mds: HashMap<String, Entity<MarkdownView>>,
    pub(crate) rxns: HashMap<String, Entity<CommentReactions>>,
    pub(crate) col_focus: FocusHandle,
    pub(crate) dock_focus: FocusHandle,
    pub(crate) toggle_focus: FocusHandle,
    pub(crate) hovered_people: bool,
    pub(crate) people_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub(crate) head_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub(crate) watched: Option<gpui_kit::EntityId>,
    /// The tab the last frame showed (Files focuses its stack when entered).
    pub(crate) shown_tab: Option<PrTab>,
    /// The dock's open state, and where focus sat, as of the last frame (`setPRAgentDockOpen`
    /// moves focus between the two toggles when the focused one goes away).
    pub(crate) dock_was_open: bool,
    pub(crate) focus_in_dock: bool,
    pub(crate) focus_on_toggle: bool,
    pub(crate) watch: Option<Subscription>,
    _subs: Vec<Subscription>,
}

impl PrDetail {
    pub fn new(vcx: &ViewContext, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ui = ui(cx);
        let state = vcx.state.clone();
        let (layout, mode, sidebar) = {
            let s = state.read(cx).storage();
            (
                read_diff_layout(s),
                read_file_list_mode(s),
                read_review_sidebar(s),
            )
        };
        let title_input = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));
        recur_ui::style::style_text_input(&title_input, cx);
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        recur_ui::style::style_text_input(&filter, cx);
        let desc = Field::new(
            &state,
            "Description",
            (2, 2),
            |_: &mut Self, _| {},
            window,
            cx,
        )
        .on_submit(
            |d: &mut Self, w, cx| d.save_compose(&Compose::Desc, w, cx),
            cx,
        );
        let comment = Field::new(
            &state,
            "Edit comment",
            (2, 2),
            |_: &mut Self, _| {},
            window,
            cx,
        )
        .on_submit(
            |d: &mut Self, w, cx| {
                let id = d.editing_comment.clone();
                d.save_compose(&Compose::Comment(id), w, cx)
            },
            cx,
        );
        let reply = Field::new(&state, "Reply…", (1, 6), |_: &mut Self, _| {}, window, cx)
            .on_submit(
                |d: &mut Self, w, cx| {
                    let id = d.reply_for.clone();
                    d.save_compose(&Compose::Reply(id), w, cx)
                },
                cx,
            );
        let composer = Field::new(
            &state,
            "Leave a comment…",
            (3, 8),
            |me: &mut Self, cx| {
                let v = me.composer.value(cx);
                me.ui.update(cx, |u, _| u.composer_draft = v);
            },
            window,
            cx,
        )
        .on_submit(|d: &mut Self, w, cx| d.submit_composer(w, cx), cx);
        let me = cx.entity().downgrade();
        let list = cx.new(|_| {
            ReviewFileList::new(
                Default::default(),
                Some(Rc::new(move |path: String, hl, _, cx: &mut App| {
                    let me = me.clone();
                    cx.defer(move |cx| {
                        let _ = me.update(cx, |d, cx| d.on_list_select(&path, hl, cx));
                    });
                })),
            )
        });
        let subs = vec![
            cx.guarded_observe_in(&state, window, |d, _, window, cx| {
                d.state_changed(window, cx)
            }),
            cx.guarded_observe_in(&ui, window, |d, _, window, cx| d.ui_changed(window, cx)),
            cx.guarded_subscribe_in(&state, window, |d, _, ev: &AppliedEvent, window, cx| {
                d.on_event(&ev.0, window, cx)
            }),
            cx.guarded_subscribe(&state, |d, _, ev: &ShellEvent, cx| {
                if let ShellEvent::PrOpened(_) = ev {
                    d.ui.update(cx, |u, cx| u.reset_for_open(cx));
                }
            }),
            cx.on_release(|d: &mut Self, cx| {
                ReviewStore::global_mut(cx).posted_finding_ids.clear();
                let _ = d;
            }),
        ];
        let mut d = PrDetail {
            vcx: vcx.clone(),
            ui,
            cache_pr: String::new(),
            reset_pr: String::new(),
            on_route: false,
            refreshing: false,
            hydrate_give_up: false,
            give_up: None,
            poll: None,
            poll_busy: Rc::default(),
            findings_gen: 0,
            findings: Vec::new(),
            live_comments: Vec::new(),
            posting: HashSet::new(),
            settling: HashSet::new(),
            settle_failed: HashSet::new(),
            settle_for: String::new(),
            editing_title: false,
            title_input,
            editing_desc: false,
            desc,
            editing_comment: String::new(),
            comment,
            saving: String::new(),
            reply_for: String::new(),
            reply,
            composer,
            composer_rev: 0,
            more: More::default(),
            collapsed_ctx: HashSet::new(),
            open_resolved: HashSet::new(),
            collapsed_check_groups: HashMap::new(),
            clocks: RefCell::default(),
            check_tick: None,
            commit_idx: None,
            commit_files: None,
            commit_cache: HashMap::new(),
            commit_loading: false,
            commit_error: false,
            commit_gen: 0,
            commit_stack: FileStack::new(state.clone(), cx),
            copied: CopiedShas::default(),
            diff_layout: layout,
            file_list_mode: mode,
            sidebar_open: sidebar,
            stack: FileStack::new(state.clone(), cx),
            stack_pane: None,
            stack_filtered_empty: false,
            list,
            filter,
            findings_only: false,
            comments_only: false,
            pane_width: 0.,
            highlight: 0,
            conv_scroll: ScrollHandle::new(),
            timeline: Default::default(),
            conv_items: Rc::default(),
            render_pr: None,
            commits_scroll: ScrollHandle::new(),
            checks_scroll: ScrollHandle::new(),
            checks_box_scroll: ScrollHandle::new(),
            mds: HashMap::new(),
            rxns: HashMap::new(),
            col_focus: cx.focus_handle(),
            dock_focus: cx.focus_handle(),
            toggle_focus: cx.focus_handle().tab_stop(true),
            hovered_people: false,
            people_bounds: Rc::default(),
            head_bounds: Rc::default(),
            watched: None,
            shown_tab: None,
            dock_was_open: false,
            focus_in_dock: false,
            focus_on_toggle: false,
            watch: None,
            _subs: subs,
        };
        d.stack.set_spy_suppress(Duration::from_millis(1400));
        d.commit_stack.set_stub_selects_in_place(true);
        d.commit_stack
            .set_chrome(recur_view_review::stack::StackChrome {
                pad: 18.,
                max_width: Some(1560.),
            });
        d.state_changed(window, cx);
        #[cfg(test)]
        {
            let me = cx.entity().downgrade();
            cx.set_global(spec_handle::Live(me));
        }
        d
    }

    pub(crate) fn state(&self) -> &Entity<AppState> {
        &self.vcx.state
    }

    pub(crate) fn pr_id(&self, cx: &App) -> String {
        self.state().read(cx).selected_pr_id.clone()
    }

    /// `pr`: the selected PR from the store.
    pub(crate) fn pr(&self, cx: &App) -> Option<Pr> {
        self.pr_ref(cx).cloned()
    }

    /// The selected PR for a render, shared between renders until the store next notifies.
    pub(crate) fn render_pr(&mut self, cx: &App) -> Option<Rc<Pr>> {
        if self.render_pr.is_none() {
            self.render_pr = Some(self.pr(cx).map(Rc::new));
        }
        self.render_pr.clone().flatten()
    }

    /// The selected PR in the store, borrowed: a PR with its patches can be tens of MB.
    pub(crate) fn pr_ref<'a>(&self, cx: &'a App) -> Option<&'a Pr> {
        let s = self.state().read(cx);
        s.pr(&s.selected_pr_id)
    }

    pub(crate) fn tab(&self, cx: &App) -> PrTab {
        self.ui.read(cx).tab
    }

    /// `prAgentWsId`.
    pub(crate) fn agent_ws(&self, cx: &App) -> String {
        let s = self.state().read(cx);
        pr_agent_workspace_id(s.pr(&s.selected_pr_id), &s.workspaces)
    }

    /// `threadItems`: the PR's comments, then the live ones.
    pub(crate) fn thread_items(&self, cx: &App) -> Vec<PrComment> {
        let mut out = self
            .pr_ref(cx)
            .and_then(|p| p.comments.clone())
            .unwrap_or_default();
        out.extend(self.live_comments.iter().cloned());
        out
    }

    pub(crate) fn conv_pending(&self, cx: &App) -> bool {
        pr_conv_pending(self.pr_ref(cx), self.hydrate_give_up)
    }

    pub(crate) fn files_pending(&self, cx: &App) -> bool {
        pr_files_pending(self.pr_ref(cx), self.refreshing, self.hydrate_give_up)
    }

    pub(crate) fn commits_pending(&self, cx: &App) -> bool {
        pr_commits_pending(self.pr_ref(cx), self.refreshing)
    }

    pub(crate) fn checks_pending(&self, cx: &App) -> bool {
        pr_checks_pending(self.pr_ref(cx), self.refreshing)
    }

    // ---- effects ----

    fn state_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.render_pr = None;
        let (view, id) = {
            let s = self.state().read(cx);
            (s.view, s.selected_pr_id.clone())
        };
        // `commitCachePR`: a new PR drops the commit cache, the files filters and findings.
        if id != self.cache_pr {
            self.cache_pr = id.clone();
            self.commit_cache.clear();
            self.filter.update(cx, |f, cx| f.set_value("", window, cx));
            self.findings_only = false;
            self.comments_only = false;
            self.findings_gen += 1;
            self.findings.clear();
            self.posting.clear();
            self.settling.clear();
            self.settle_failed.clear();
            ReviewStore::global_mut(cx).posted_finding_ids.clear();
        }
        // The `$selectedPRId` resets.
        let switched = id != self.reset_pr;
        if switched {
            self.reset_pr = id.clone();
            self.collapsed_check_groups.clear();
            self.editing_title = false;
            self.editing_desc = false;
            self.editing_comment.clear();
            self.saving.clear();
            self.reply_for.clear();
            self.reply.set("", window, cx);
            let store = ReviewStore::global_mut(cx);
            store.reset_holds();
            self.close_more(cx);
            self.commit_idx = None;
            self.discard_commit_diff();
            self.ui.update(cx, |u, cx| {
                u.set_tab(PrTab::Conv, cx);
                u.set_composer_draft("", cx);
            });
        }
        // `markPostedFinding` for every pending draft that carries a finding.
        let ids: Vec<String> = self
            .pr_ref(cx)
            .map(|pr| {
                pr.pending_comments
                    .iter()
                    .map(|pc| pc.finding_id.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        ReviewStore::global_mut(cx).posted_finding_ids.extend(ids);
        let on = view == ViewName::PrDetail;
        if on && (!self.on_route || switched) {
            self.route_opened(&id, window, cx);
        }
        if !on && self.on_route {
            self.poll = None;
            self.give_up = None;
            self.check_tick = None;
        }
        self.on_route = on;
        if self.state().read(cx).app_overlay_open() {
            self.close_more(cx);
        }
        // `prDiffJump`: a jump into Files changed (the review modal's pending list).
        let jump = self.state().read(cx).pr_diff_jump.clone();
        if let Some(j) = jump {
            self.state().update(cx, |s, _| s.pr_diff_jump = None);
            self.jump_to_impact(&j.file, i64::from(j.line), cx);
        }
        self.sync_check_tick(cx);
        self.maybe_settle(cx);
        self.flush_fix_checks(cx);
        cx.notify();
    }

    /// Entering the route (or switching PRs on it): GetPR, the 15s check poll, and the 25s
    /// hydration give-up.
    fn route_opened(&mut self, id: &str, _window: &mut Window, cx: &mut Context<Self>) {
        if id.is_empty() {
            self.state()
                .update(cx, |s, cx| s.go(ViewName::Pr, None, cx));
            return;
        }
        self.refresh(id, false, false, cx).detach();
        self.hydrate_give_up = false;
        let wait = HYDRATE_GIVE_UP;
        self.give_up = Some(cx.spawn(async move |d, cx| {
            cx.background_executor().timer(wait).await;
            let _ = d.update(cx, |d, cx| {
                d.hydrate_give_up = true;
                cx.notify();
            });
        }));
        let busy = self.poll_busy.clone();
        let pid = id.to_string();
        self.poll = Some(cx.spawn(async move |d, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if busy.get() {
                    continue;
                }
                let go = d
                    .update(cx, |d, cx| {
                        let s = d.state().read(cx);
                        s.pr(&pid).is_some_and(|p| {
                            p.source == PrSource::GitHub
                                && p.state != "Merged"
                                && p.group != "merged"
                        })
                    })
                    .unwrap_or(false);
                if !go {
                    continue;
                }
                busy.set(true);
                let task = d.update(cx, |d, cx| d.refresh(&pid, true, false, cx));
                if let Ok(t) = task {
                    t.await;
                }
                busy.set(false);
            }
        }));
    }

    fn ui_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (rev, draft) = {
            let u = self.ui.read(cx);
            (u.composer_rev, u.composer_draft.clone())
        };
        if rev != self.composer_rev {
            self.composer_rev = rev;
            if self.composer.value(cx) != draft {
                self.composer.set(draft, window, cx);
            }
        }
        cx.notify();
    }

    fn on_event(&mut self, ev: &AppEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.pr_id(cx);
        match ev {
            // `onPRComment`: append a live thread entry for this PR.
            AppEvent::PrComment(d) => {
                if d.pr_id != selected {
                    return;
                }
                let row = d.comment.clone();
                if row.id.is_empty() {
                    return;
                }
                let comments = self
                    .pr_ref(cx)
                    .and_then(|p| p.comments.clone())
                    .unwrap_or_default();
                if comments.iter().any(|c| c.id == row.id)
                    || self.live_comments.iter().any(|c| c.id == row.id)
                {
                    return;
                }
                let pending = comments
                    .iter()
                    .find(|c| is_pending_comment(Some(c)) && same_thread_reply(c, &row))
                    .map(|c| c.id.clone());
                if let Some(pid) = pending {
                    let prid = selected.clone();
                    self.patch_comments(
                        &prid,
                        |list| settle_posted_reply(list, &pid, Some(row)),
                        cx,
                    );
                    return;
                }
                self.live_comments.push(row);
                cx.notify();
            }
            // `onPRRefresh`.
            AppEvent::PrUpdated(d) if d.pr_id == selected && !d.pending_only => {
                self.refresh(&selected, true, true, cx).detach();
            }
            // `onFindingsUpdated`: review-mode findings land on the dock workspace.
            AppEvent::FindingsUpdated(d) => {
                if self.state().read(cx).view != ViewName::PrDetail
                    || d.workspace_id.is_empty()
                    || d.workspace_id != self.agent_ws(cx)
                    || selected.is_empty()
                {
                    return;
                }
                self.refresh(&selected, true, true, cx).detach();
            }
            _ => {}
        }
    }

    /// `refresh(id, { silent, findings })`: GetPR merged into the store (optimistic replies,
    /// resolve and reaction holds kept), the live rows GetPR now has dropped, and the
    /// reviewer findings re-listed.
    pub(crate) fn refresh(
        &mut self,
        id: &str,
        silent: bool,
        with_findings: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if id.is_empty() {
            return Task::ready(());
        }
        if !silent {
            self.refreshing = true;
            cx.notify();
        }
        let services = self.vcx.services.clone();
        let id = id.to_string();
        cx.spawn(async move |d, cx| {
            let full = services.prs.get_pr(&id).await.ok();
            let _ = d.update(cx, |d, cx| {
                if !silent {
                    d.refreshing = false;
                }
                let Some(full) = full.filter(|p| !p.id.is_empty()) else {
                    // Unknown / deleted PR: back to the list. Poll failures stay put.
                    let s = d.state().read(cx);
                    if !silent && s.view == ViewName::PrDetail && s.selected_pr_id == id {
                        d.state().update(cx, |s, cx| s.go(ViewName::Pr, None, cx));
                    }
                    cx.notify();
                    return;
                };
                d.upsert_full(full.clone(), cx);
                let persisted: HashSet<String> = full
                    .comments
                    .iter()
                    .flatten()
                    .map(|c| c.id.clone())
                    .filter(|s| !s.is_empty())
                    .collect();
                if silent {
                    d.live_comments.retain(|c| !persisted.contains(&c.id));
                } else {
                    d.live_comments.clear();
                }
                if !silent || with_findings {
                    d.load_findings(&id, cx);
                }
                cx.notify();
            });
        })
    }

    fn upsert_full(&mut self, full: Pr, cx: &mut Context<Self>) {
        let state = self.state().clone();
        let prev = state.read(cx).pr(&full.id).cloned();
        let mut next = full;
        let prev_comments = prev
            .as_ref()
            .and_then(|p| p.comments.clone())
            .unwrap_or_default();
        let mut comments =
            keep_pending_replies(&prev_comments, next.comments.take().unwrap_or_default());
        apply_review_holds(&mut comments, cx);
        next.comments = Some(comments);
        next.commits = keep_commit_files(
            prev.as_ref().and_then(|p| p.commits.as_ref()),
            Some(next.commits.take().unwrap_or_default()),
        );
        let merged = keep_merged_pr(prev.as_ref(), next);
        state.update(cx, |s, cx| {
            match s.prs.iter_mut().find(|p| p.id == merged.id) {
                Some(p) => *p = merged,
                None => s.prs.push(merged),
            }
            cx.notify();
        });
    }

    fn load_findings(&mut self, id: &str, cx: &mut Context<Self>) {
        self.findings_gen += 1;
        let token = self.findings_gen;
        let services = self.vcx.services.clone();
        let id = id.to_string();
        cx.spawn(async move |d, cx| {
            let res = services.prs.list_pr_findings(&id).await;
            let _ = d.update(cx, |d, cx| {
                if token != d.findings_gen {
                    return;
                }
                d.findings = res.unwrap_or_default();
                d.maybe_settle(cx);
                cx.notify();
            });
        })
        .detach();
    }

    // ---- tabs ----

    /// `setTab`: Commits starts from the list and refreshes; Checks refreshes GitHub PRs.
    pub fn set_tab(&mut self, t: PrTab, cx: &mut Context<Self>) {
        self.ui.update(cx, |u, cx| u.set_tab(t, cx));
        let id = self.pr_id(cx);
        if t == PrTab::Commits {
            self.commit_idx = None;
            self.discard_commit_diff();
            self.refresh(&id, true, false, cx).detach();
        }
        if t == PrTab::Checks
            && self
                .pr_ref(cx)
                .is_some_and(|p| p.source == PrSource::GitHub)
        {
            self.refresh(&id, true, false, cx).detach();
        }
        cx.notify();
    }

    // ---- findings ----

    /// `openFindings`.
    pub(crate) fn open_findings(&self) -> Vec<Finding> {
        self.findings
            .iter()
            .filter(|f| f.status == "open" || f.status.is_empty())
            .cloned()
            .collect()
    }

    pub(crate) fn review_files<'a>(&self, cx: &'a App) -> &'a [FileDiff] {
        self.pr_ref(cx)
            .and_then(|p| p.files.as_deref())
            .unwrap_or_default()
    }

    /// `listedFindings`: open, without a pending draft, not settling.
    pub(crate) fn listed_findings(&self, cx: &App) -> Vec<Finding> {
        let pending = self
            .pr_ref(cx)
            .map(|p| p.pending_comments.clone())
            .unwrap_or_default();
        let files = self.review_files(cx);
        self.open_findings()
            .into_iter()
            .filter(|f| {
                !finding_has_pending_comment(f, &pending, files) && !self.settling.contains(&f.id)
            })
            .collect()
    }

    pub(crate) fn posted_ids(cx: &App) -> HashSet<String> {
        ReviewStore::get(cx)
            .map(|s| s.posted_finding_ids.clone())
            .unwrap_or_default()
    }

    /// `postableFindings`.
    pub(crate) fn postable_findings(&self, cx: &App) -> Vec<Finding> {
        let files = self.review_files(cx);
        let posted = Self::posted_ids(cx);
        self.listed_findings(cx)
            .into_iter()
            .filter(|f| finding_review_anchor(f, files).is_some() && !posted.contains(&f.id))
            .collect()
    }

    pub(crate) fn conversation_locked(&self, cx: &App) -> bool {
        recur_view_review::line_note::pr_conversation_locked(self.pr_ref(cx))
    }

    pub(crate) fn can_post_findings(&self, cx: &App) -> bool {
        self.pr_ref(cx)
            .is_some_and(|p| p.source == PrSource::GitHub)
            && !self.conversation_locked(cx)
    }

    /// `postFindings`: queue each as a pending review comment at its GitHub anchor.
    pub(crate) fn post_findings(&mut self, list: Vec<Finding>, cx: &mut Context<Self>) {
        let Some(pr) = self.pr(cx) else { return };
        if pr.id.is_empty() || !self.can_post_findings(cx) {
            return;
        }
        let files = self.review_files(cx);
        let posted = Self::posted_ids(cx);
        let jobs: Vec<(Finding, crate::findings::ReviewAnchor, String)> = list
            .into_iter()
            .filter(|f| {
                !self.posting.contains(&f.id)
                    && !posted.contains(&f.id)
                    && !finding_has_pending_comment(f, &pr.pending_comments, files)
            })
            .filter_map(|f| {
                let a = finding_review_anchor(&f, files)?;
                let body = finding_comment_body(&f.message).to_string();
                (!body.is_empty()).then_some((f, a, body))
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        for (f, _, _) in &jobs {
            self.posting.insert(f.id.clone());
        }
        cx.notify();
        let state = self.state().clone();
        let services = self.vcx.services.clone();
        let pr_id = pr.id.clone();
        cx.spawn(async move |d, cx| {
            let mut n = 0;
            for (f, a, body) in &jobs {
                let pc = PendingLineComment {
                    id: format!("pend-local-{}-{}", now_ms(), f.id),
                    file: a.file.clone(),
                    line: a.line,
                    side: "RIGHT".into(),
                    start_line: a.start_line,
                    start_side: if a.start_line > 0 {
                        "RIGHT".into()
                    } else {
                        String::new()
                    },
                    body: body.clone(),
                    finding_id: f.id.clone(),
                };
                let pcid = pc.id.clone();
                cx.update(|cx| {
                    patch_pr_pending_comments(
                        &state,
                        &pr_id,
                        |prev| {
                            let mut v = prev.to_vec();
                            v.push(pc);
                            v
                        },
                        cx,
                    )
                });
                let ok = call(
                    cx,
                    services
                        .prs
                        .add_pr_comment(&pr_id, &a.file, a.line, body, a.start_line, &f.id),
                    "Could not queue the comment",
                )
                .await;
                if ok.is_none() {
                    cx.update(|cx| {
                        patch_pr_pending_comments(
                            &state,
                            &pr_id,
                            |prev| prev.iter().filter(|x| x.id != pcid).cloned().collect(),
                            cx,
                        )
                    });
                    continue;
                }
                let fid = f.id.clone();
                cx.update(|cx| {
                    ReviewStore::global_mut(cx).posted_finding_ids.insert(fid);
                });
                n += 1;
            }
            cx.update(|cx| {
                if n == 1 {
                    toast(ToastKind::Success, "Saved — submit your review to post", cx);
                } else if n > 1 {
                    toast(
                        ToastKind::Success,
                        format!("{n} comments pending — submit a review to post them"),
                        cx,
                    );
                }
            });
            let _ = d.update(cx, |d, cx| {
                for (f, _, _) in &jobs {
                    d.posting.remove(&f.id);
                }
                d.maybe_settle(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The auto-settle effect: a finding this session queued whose submitted comment now
    /// occupies its anchor gets resolved.
    fn maybe_settle(&mut self, cx: &mut Context<Self>) {
        let Some(pr) = self.pr(cx) else { return };
        if pr.id.is_empty() {
            return;
        }
        let comments = self.thread_items(cx);
        let files = self.review_files(cx);
        let posted = Self::posted_ids(cx);
        let due = self.open_findings().iter().any(|f| {
            !self.settle_failed.contains(&f.id)
                && finding_ready_to_settle(f, &pr.pending_comments, &comments, files, &posted)
        });
        if due {
            self.settle_posted_findings(&pr.id, None, cx);
        }
    }

    /// `settlePostedFindings(prId, retryIds?)`.
    pub(crate) fn settle_posted_findings(
        &mut self,
        pr_id: &str,
        retry: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        if self.settle_for == pr_id {
            return;
        }
        self.settle_for = pr_id.to_string();
        let pending = self
            .pr_ref(cx)
            .map(|p| p.pending_comments.clone())
            .unwrap_or_default();
        let comments = self.thread_items(cx);
        let files = self.review_files(cx);
        let posted = Self::posted_ids(cx);
        let retry: Option<HashSet<String>> = retry.map(|v| v.into_iter().collect());
        let due: Vec<Finding> = self
            .open_findings()
            .into_iter()
            .filter(|f| {
                let skip = match &retry {
                    Some(r) => !r.contains(&f.id),
                    None => self.settle_failed.contains(&f.id),
                };
                !skip && finding_ready_to_settle(f, &pending, &comments, files, &posted)
            })
            .collect();
        if due.is_empty() {
            self.settle_for.clear();
            return;
        }
        for f in &due {
            self.settling.insert(f.id.clone());
        }
        cx.notify();
        let services = self.vcx.services.clone();
        let pr_id = pr_id.to_string();
        let agent_ws = self.agent_ws(cx);
        cx.spawn(async move |d, cx| {
            for f in &due {
                let still = d.update(cx, |d, cx| d.pr_id(cx) == pr_id).unwrap_or(false);
                if !still {
                    break;
                }
                let ws = if f.workspace_id.trim().is_empty() {
                    agent_ws.trim().to_string()
                } else {
                    f.workspace_id.trim().to_string()
                };
                if ws.is_empty() {
                    continue;
                }
                let ok = call(
                    cx,
                    services
                        .observatory
                        .set_finding_status(&ws, &f.id, "resolved"),
                    "Could not resolve the finding",
                )
                .await;
                let fid = f.id.clone();
                let _ = d.update(cx, |d, cx| {
                    if d.pr_id(cx) != pr_id {
                        return;
                    }
                    if ok.is_none() {
                        d.settle_failed.insert(fid);
                        return;
                    }
                    d.settle_failed.remove(&fid);
                    ReviewStore::global_mut(cx).posted_finding_ids.remove(&fid);
                    for x in d.findings.iter_mut().filter(|x| x.id == fid) {
                        x.status = "resolved".into();
                    }
                    cx.notify();
                });
            }
            let _ = d.update(cx, |d, cx| {
                if d.settle_for == pr_id {
                    d.settle_for.clear();
                    for f in &due {
                        d.settling.remove(&f.id);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- fix checks ----

    /// `requestFixChecks(prId, args)`.
    pub(crate) fn request_fix_checks(
        state: &Entity<AppState>,
        pr_id: &str,
        args: &str,
        cx: &mut App,
    ) {
        let id = pr_id.trim().to_string();
        if id.is_empty() {
            return;
        }
        let ui = ui(cx);
        ui.update(cx, |u, cx| u.set_tab(PrTab::Checks, cx));
        state.update(cx, |s, cx| {
            s.pr_agent_dock_open = true;
            cx.notify();
        });
        let step = {
            let s = state.read(cx);
            request_fix_checks(&id, args, &s.prs, &s.workspaces)
        };
        match step {
            None => {}
            Some(FixChecksStep::Run(ws)) => {
                let a = args.to_string();
                state.update(cx, |s, cx| {
                    s.request_prompt_action(&ws, "fix-checks", &a, cx)
                });
            }
            Some(FixChecksStep::CreateWorkspace(p)) => {
                ui.update(cx, |u, _| u.pending_fix_checks = Some(p));
                let services = state.read(cx).services().clone();
                let state = state.clone();
                cx.spawn(async move |cx| {
                    let ws = call(
                        cx,
                        services.prs.create_workspace_from_pr(&id),
                        "Could not create the workspace",
                    )
                    .await;
                    cx.update(|cx| {
                        let Some(ws) = ws else {
                            ui.update(cx, |u, _| u.pending_fix_checks = None);
                            return;
                        };
                        let ready = created_workspace_ready(&ws);
                        state.update(cx, |s, cx| {
                            let pid = ws.project_id.clone();
                            let wid = ws.id.clone();
                            s.upsert_workspace(ws, cx);
                            if ready {
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
                        flush(&state, &ui, cx);
                    });
                })
                .detach();
            }
        }
    }

    fn flush_fix_checks(&mut self, cx: &mut Context<Self>) {
        let state = self.state().clone();
        let ui = self.ui.clone();
        flush(&state, &ui, cx);
    }

    // ---- comments ----

    pub(crate) fn patch_comments(
        &self,
        pr_id: &str,
        f: impl FnOnce(Vec<PrComment>) -> Vec<PrComment>,
        cx: &mut App,
    ) {
        self.state().update(cx, |s, cx| {
            s.update_pr(
                pr_id,
                |p| {
                    let list = p.comments.take().unwrap_or_default();
                    p.comments = Some(f(list));
                },
                cx,
            )
        });
    }

    /// `setThreadResolved`.
    pub(crate) fn set_thread_resolved(
        &mut self,
        c: &PrComment,
        resolved: bool,
        cx: &mut Context<Self>,
    ) {
        let key = line_thread_key(Some(c));
        if resolved {
            self.open_resolved.remove(&c.id);
        }
        apply_thread_resolved(&mut self.live_comments, &key, resolved);
        let task = recur_view_review::store::set_pr_thread_resolved(self.state(), c, resolved, cx);
        cx.spawn(async move |d, cx| {
            let ok = task.await;
            let _ = d.update(cx, |d, cx| {
                let held = ReviewStore::get(cx).and_then(|s| s.thread_holds.holds(&key));
                if !ok && held != Some(resolved) {
                    apply_thread_resolved(&mut d.live_comments, &key, !resolved);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// `openReply`.
    pub(crate) fn open_reply(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.conversation_locked(cx) {
            return;
        }
        self.reply_for = id.to_string();
        self.reply.set("", window, cx);
        self.reply.focus(window, cx);
        cx.notify();
    }

    /// `optimisticReply`.
    fn optimistic_reply(&self, root_id: &str, body: &str, cx: &App) -> PrComment {
        let items = self.thread_items(cx);
        let pr = self.pr(cx);
        let root = items.iter().find(|c| c.id == root_id).cloned();
        let mine = items
            .iter()
            .find(|c| c.kind != "description" && can_edit_pr_comment(pr.as_ref(), Some(c)))
            .cloned();
        let login = pr
            .as_ref()
            .map(|p| p.viewer_login.clone())
            .filter(|l| !l.is_empty())
            .or_else(|| mine.as_ref().map(|m| m.author_login.clone()))
            .unwrap_or_default();
        let author = mine
            .as_ref()
            .map(|m| m.author.clone())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| {
                if login.is_empty() {
                    "You".into()
                } else {
                    login.clone()
                }
            });
        let root = root.unwrap_or_default();
        PrComment {
            id: format!("{PENDING_REPLY_PREFIX}{}", now_ms()),
            code: mine
                .as_ref()
                .map(|m| m.code.clone())
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| author.chars().take(2).collect::<String>().to_uppercase()),
            author,
            author_login: login,
            avatar_css: mine
                .as_ref()
                .map(|m| m.avatar_css.clone())
                .unwrap_or_default(),
            avatar_url: mine
                .as_ref()
                .map(|m| m.avatar_url.clone())
                .unwrap_or_default(),
            time: "now".into(),
            body_md: body.to_string(),
            verb: "commented".into(),
            kind: "comment".into(),
            in_reply_to_id: root_id.to_string(),
            parent_id: root.parent_id.clone(),
            path: root.path.clone(),
            line: root.line,
            side: root.side.clone(),
            thread_id: if root.thread_id.is_empty() {
                root_id.to_string()
            } else {
                root.thread_id.clone()
            },
            resolved: root.resolved,
            ..Default::default()
        }
    }

    /// `submitReply`: the composer closes before the GitHub write returns.
    pub(crate) fn submit_reply(
        &mut self,
        root_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let body = self.reply.value(cx).trim().to_string();
        if body.is_empty() || !self.saving.is_empty() || self.conversation_locked(cx) {
            return;
        }
        let pr_id = self.pr_id(cx);
        let pending = self.optimistic_reply(root_id, &body, cx);
        let pending_id = pending.id.clone();
        self.patch_comments(
            &pr_id,
            |mut v| {
                v.push(pending);
                v
            },
            cx,
        );
        self.reply_for.clear();
        self.reply.set("", window, cx);
        cx.notify();
        let services = self.vcx.services.clone();
        let root_id = root_id.to_string();
        cx.spawn(async move |d, cx| {
            let posted = call(
                cx,
                services.prs.reply_pr_comment(&pr_id, &root_id, &body),
                "Could not reply",
            )
            .await;
            let _ = d.update(cx, |d, cx| match posted.filter(|p| !p.id.is_empty()) {
                Some(posted) => {
                    let pid = posted.id.clone();
                    d.patch_comments(
                        &pr_id,
                        |v| settle_posted_reply(v, &pending_id, Some(posted)),
                        cx,
                    );
                    d.live_comments.retain(|c| c.id != pid);
                    toast(ToastKind::Success, "Reply posted", cx);
                    cx.notify();
                }
                None => {
                    d.patch_comments(&pr_id, |v| settle_posted_reply(v, &pending_id, None), cx);
                }
            });
        })
        .detach();
    }

    /// `deleteComment`.
    pub(crate) fn delete_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.saving.is_empty() || id.is_empty() {
            return;
        }
        let pr_id = self.pr_id(cx);
        let from_pr: Vec<PrComment> = self
            .pr(cx)
            .and_then(|p| p.comments)
            .unwrap_or_default()
            .into_iter()
            .filter(|c| is_dropped_comment(c, id))
            .collect();
        let from_live: Vec<PrComment> = self
            .live_comments
            .iter()
            .filter(|c| is_dropped_comment(c, id))
            .cloned()
            .collect();
        if from_pr.is_empty() && from_live.is_empty() {
            return;
        }
        self.saving = format!("del-{id}");
        let idc = id.to_string();
        self.patch_comments(
            &pr_id,
            |mut v| {
                v.retain(|c| !is_dropped_comment(c, &idc));
                v
            },
            cx,
        );
        self.live_comments.retain(|c| !is_dropped_comment(c, &idc));
        if self.editing_comment == id {
            self.editing_comment.clear();
        }
        if self.reply_for == id {
            self.reply_for.clear();
        }
        cx.notify();
        let services = self.vcx.services.clone();
        cx.spawn(async move |d, cx| {
            let updated = call(
                cx,
                services.prs.delete_pr_comment(&pr_id, &idc),
                "Could not delete comment",
            )
            .await;
            let _ = d.update(cx, |d, cx| {
                d.saving.clear();
                let Some(updated) = updated.filter(|p| !p.id.is_empty()) else {
                    d.patch_comments(
                        &pr_id,
                        |mut v| {
                            v.extend(from_pr);
                            v
                        },
                        cx,
                    );
                    d.live_comments.extend(from_live);
                    cx.notify();
                    return;
                };
                d.state().update(cx, |s, cx| {
                    let uid = updated.id.clone();
                    s.update_pr(&uid, |p| *p = updated, cx)
                });
                toast(ToastKind::Success, "Comment deleted", cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// `quoteReply`: a line note quotes into its thread's reply box, anything else into the
    /// conversation composer.
    pub(crate) fn quote_reply(
        &mut self,
        c: &PrComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.conversation_locked(cx) {
            return;
        }
        let src = comment_source(Some(c));
        if src.is_empty() {
            return;
        }
        let quoted = src
            .split('\n')
            .map(|l| format!("> {l}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n\n";
        let root = if is_line_note(Some(c)) {
            let r = c.in_reply_to_id.trim();
            if r.is_empty() {
                c.id.clone()
            } else {
                r.to_string()
            }
        } else {
            String::new()
        };
        if !root.is_empty() {
            let keep = if self.reply_for == root {
                self.reply.value(cx)
            } else {
                String::new()
            };
            self.reply_for = root;
            self.reply.set(quoted + &keep, window, cx);
            self.set_tab(PrTab::Conv, cx);
            self.reply.focus(window, cx);
            cx.notify();
            return;
        }
        let cur = self.composer.value(cx);
        self.ui
            .update(cx, |u, cx| u.set_composer_draft(quoted + &cur, cx));
        self.set_tab(PrTab::Conv, cx);
        self.composer.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_more(&mut self, cx: &mut Context<Self>) {
        if !self.more.id.is_empty() {
            self.more.id.clear();
            self.more.trigger = None;
            self.more.menu = None;
            cx.notify();
        }
    }

    pub(crate) fn toggle_more(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.more.id == id {
            self.close_more(cx);
            return;
        }
        self.more.id = id.to_string();
        self.more.trigger = self.more.triggers.get(id).copied();
        cx.notify();
    }

    /// `moreTarget`.
    pub(crate) fn more_target(&self, cx: &App) -> Option<PrComment> {
        if self.more.id.is_empty() {
            return None;
        }
        let items = self.thread_items(cx);
        let desc = items.iter().find(|c| c.kind == "description").cloned();
        if let Some(d) = &desc
            && (d.id == self.more.id || self.more.id == "gh-description")
        {
            return Some(d.clone());
        }
        if self.more.id == "gh-description" {
            return desc;
        }
        items.into_iter().find(|c| c.id == self.more.id)
    }

    pub(crate) fn more_permalink(&self, cx: &App) -> String {
        let url = self.pr_ref(cx).map(|p| p.url.clone()).unwrap_or_default();
        let target = self.more_target(cx).unwrap_or(PrComment {
            id: "gh-description".into(),
            kind: "description".into(),
            ..Default::default()
        });
        recur_forge::github::comment_permalink(&url, &target)
    }

    pub(crate) fn more_action(
        &mut self,
        a: MoreAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.more_target(cx);
        let link = self.more_permalink(cx);
        self.close_more(cx);
        match a {
            MoreAction::OpenGitHub => {
                if !link.is_empty() {
                    recur_content::links::open_external_url(&link, cx);
                }
            }
            MoreAction::CopyLink => {
                if !link.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(link));
                    toast(ToastKind::Info, "Copied link", cx);
                }
            }
            MoreAction::Quote => {
                if let Some(c) = target {
                    self.quote_reply(&c, window, cx);
                }
            }
            MoreAction::Edit => {
                if let Some(c) = target {
                    if c.kind == "description" {
                        self.start_edit_desc(window, cx);
                    } else {
                        self.start_edit_comment(&c, window, cx);
                    }
                }
            }
            MoreAction::Delete => {
                if let Some(c) = target {
                    self.delete_comment(&c.id, cx);
                }
            }
        }
    }

    // ---- title, description and comment edits ----

    pub(crate) fn can_edit_pr(&self, cx: &App) -> bool {
        self.pr_ref(cx).is_some_and(|p| p.can_edit_details())
    }

    pub(crate) fn description(&self, cx: &App) -> Option<PrComment> {
        self.thread_items(cx)
            .into_iter()
            .find(|c| c.kind == "description")
    }

    pub(crate) fn start_edit_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_edit_pr(cx) {
            return;
        }
        let title = self.pr_ref(cx).map(|p| p.title.clone()).unwrap_or_default();
        self.title_input.update(cx, |i, cx| {
            i.set_value(title, window, cx);
            i.focus(window, cx);
        });
        self.editing_title = true;
        self.editing_desc = false;
        self.editing_comment.clear();
        cx.notify();
    }

    pub(crate) fn start_edit_desc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_edit_pr(cx) {
            return;
        }
        let src = comment_source(self.description(cx).as_ref());
        self.desc.set(src, window, cx);
        self.desc.focus(window, cx);
        self.editing_desc = true;
        self.editing_title = false;
        self.editing_comment.clear();
        cx.notify();
    }

    pub(crate) fn start_edit_comment(
        &mut self,
        c: &PrComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing_comment = c.id.clone();
        self.comment.set(comment_source(Some(c)), window, cx);
        self.comment.focus(window, cx);
        self.editing_title = false;
        self.editing_desc = false;
        cx.notify();
    }

    /// `savePRField`.
    fn save_pr_field(
        &mut self,
        field: &'static str,
        title: String,
        body: String,
        ok_msg: &'static str,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit_pr(cx) || !self.saving.is_empty() {
            return;
        }
        self.saving = field.into();
        cx.notify();
        let services = self.vcx.services.clone();
        let id = self.pr_id(cx);
        cx.spawn(async move |d, cx| {
            let updated = call(
                cx,
                services.prs.update_pr_details(&id, &title, &body),
                "Could not update pull request",
            )
            .await;
            let _ = d.update(cx, |d, cx| {
                d.saving.clear();
                cx.notify();
                let Some(updated) = updated.filter(|p| !p.id.is_empty()) else {
                    return;
                };
                d.state().update(cx, |s, cx| {
                    let uid = updated.id.clone();
                    s.update_pr(&uid, |p| *p = updated, cx)
                });
                match field {
                    "title" => d.editing_title = false,
                    _ => d.editing_desc = false,
                }
                toast(ToastKind::Success, ok_msg, cx);
            });
        })
        .detach();
    }

    pub(crate) fn save_title(&mut self, cx: &mut Context<Self>) {
        let title = self.title_input.read(cx).value().trim().to_string();
        if title.is_empty() || !self.saving.is_empty() {
            return;
        }
        let body = comment_source(self.description(cx).as_ref());
        self.save_pr_field("title", title, body, "Title updated", cx);
    }

    pub(crate) fn save_desc(&mut self, cx: &mut Context<Self>) {
        if !self.saving.is_empty() {
            return;
        }
        let title = self.pr_ref(cx).map(|p| p.title.clone()).unwrap_or_default();
        let body = self.desc.value(cx);
        self.save_pr_field("desc", title, body, "Description updated", cx);
    }

    /// `saveComment`.
    pub(crate) fn save_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        let body = self.comment.value(cx).trim().to_string();
        if body.is_empty() || !self.saving.is_empty() {
            return;
        }
        self.saving = id.to_string();
        cx.notify();
        let services = self.vcx.services.clone();
        let pr_id = self.pr_id(cx);
        let id = id.to_string();
        cx.spawn(async move |d, cx| {
            let updated = call(
                cx,
                services.prs.update_pr_comment(&pr_id, &id, &body),
                "Could not update comment",
            )
            .await;
            let _ = d.update(cx, |d, cx| {
                d.saving.clear();
                cx.notify();
                let Some(updated) = updated else { return };
                let fix = |c: &mut PrComment| {
                    let mut next = spread(c, &updated);
                    next.body_md = body.clone();
                    *c = next;
                };
                d.patch_comments(
                    &pr_id,
                    |mut v| {
                        v.iter_mut().filter(|c| c.id == id).for_each(fix);
                        v
                    },
                    cx,
                );
                d.live_comments
                    .iter_mut()
                    .filter(|c| c.id == id)
                    .for_each(fix);
                d.editing_comment.clear();
                toast(ToastKind::Success, "Comment updated", cx);
            });
        })
        .detach();
    }

    /// `submitComposer`: a conversation comment, posted at once (not a review).
    pub(crate) fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.composer.value(cx).trim().to_string();
        if draft.is_empty() || self.conversation_locked(cx) || !self.saving.is_empty() {
            return;
        }
        self.saving = "composer".into();
        cx.notify();
        let services = self.vcx.services.clone();
        let pr_id = self.pr_id(cx);
        let _ = window;
        cx.spawn(async move |d, cx| {
            let posted = call(
                cx,
                services.prs.add_pr_comment(&pr_id, "", 0, &draft, 0, ""),
                "Comment failed",
            )
            .await;
            let _ = d.update(cx, |d, cx| {
                d.saving.clear();
                cx.notify();
                let Some(posted) = posted else { return };
                d.patch_comments(
                    &pr_id,
                    |mut v| {
                        v.push(posted);
                        v
                    },
                    cx,
                );
                d.ui.update(cx, |u, cx| u.set_composer_draft("", cx));
                toast(ToastKind::Success, "Comment posted", cx);
            });
        })
        .detach();
    }

    // ---- commits ----

    /// `prCommits`: newest first, as the store keeps them.
    pub(crate) fn commits<'a>(&self, cx: &'a App) -> &'a [Commit] {
        self.pr_ref(cx)
            .and_then(|p| p.commits.as_deref())
            .unwrap_or_default()
    }

    /// `detailFiles`.
    pub(crate) fn detail_files(&self, cx: &App) -> Vec<FileDiff> {
        let raw = match &self.commit_files {
            Some(f) if !f.is_empty() => f.clone(),
            _ => self
                .commit_idx
                .and_then(|i| self.commits(cx).get(i).cloned())
                .and_then(|c| c.files)
                .unwrap_or_default(),
        };
        recur_code::view::changed_tree::sort_files_by_changed_tree(&raw, |f| f.path.as_str())
    }

    pub(crate) fn discard_commit_diff(&mut self) {
        self.commit_gen += 1;
        self.commit_files = None;
        self.commit_stack.set_files(Vec::new());
        self.commit_loading = false;
        self.commit_error = false;
    }

    pub(crate) fn open_commit(&mut self, i: usize, cx: &mut Context<Self>) {
        self.commit_idx = Some(i);
        let Some(c) = self.commits(cx).get(i).cloned() else {
            cx.notify();
            return;
        };
        self.load_commit_diff(&c, cx);
        cx.notify();
    }

    /// `openLocalCommit`.
    pub(crate) fn open_local_commit(&mut self, i: usize, cx: &mut Context<Self>) {
        self.ui.update(cx, |u, cx| u.set_tab(PrTab::Commits, cx));
        self.open_commit(i, cx);
    }

    /// `loadCommitDiff`: the files the commit carries, the cache, or GetCommitDiff.
    fn load_commit_diff(&mut self, c: &Commit, cx: &mut Context<Self>) {
        let hash = if c.hash.trim().is_empty() {
            c.sha.trim().to_string()
        } else {
            c.hash.trim().to_string()
        };
        if let Some(files) = c.files.as_ref().filter(|f| !f.is_empty()) {
            self.commit_gen += 1;
            self.commit_files = Some(files.clone());
            self.select_first_commit_file(files);
            self.commit_loading = false;
            self.commit_error = false;
            if !hash.is_empty() {
                self.commit_cache.insert(hash, files.clone());
            }
            return;
        }
        if let Some(cached) = self
            .commit_cache
            .get(&hash)
            .filter(|f| !f.is_empty())
            .cloned()
        {
            self.commit_gen += 1;
            self.select_first_commit_file(&cached);
            self.commit_files = Some(cached);
            self.commit_loading = false;
            self.commit_error = false;
            return;
        }
        let id = self.pr_id(cx);
        if id.is_empty() || hash.is_empty() {
            return;
        }
        self.commit_gen += 1;
        let token = self.commit_gen;
        self.commit_loading = true;
        self.commit_error = false;
        self.commit_files = None;
        let services = self.vcx.services.clone();
        cx.spawn(async move |d, cx| {
            let res = services.prs.get_commit_diff(&id, &hash).await;
            let _ = d.update(cx, |d, cx| {
                if token != d.commit_gen {
                    return;
                }
                match res {
                    Ok(full) => {
                        let files = full.files.unwrap_or_default();
                        d.select_first_commit_file(&files);
                        if !hash.is_empty() && !files.is_empty() {
                            d.commit_cache.insert(hash, files.clone());
                        }
                        d.commit_files = Some(files);
                    }
                    Err(_) => {
                        d.commit_files = Some(Vec::new());
                        d.commit_error = true;
                    }
                }
                d.commit_loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn select_first_commit_file(&mut self, files: &[FileDiff]) {
        let first = files.first().map(|f| f.path.clone()).unwrap_or_default();
        self.commit_stack.set_selected(&first);
    }

    /// `localCommitIndex`: the commit a timeline SHA names (prefix match either way).
    pub(crate) fn local_commit_index(&self, sha: &str, cx: &App) -> Option<usize> {
        let s = sha.to_lowercase();
        if s.is_empty() {
            return None;
        }
        self.commits(cx).iter().position(|c| {
            let h = c.hash.to_lowercase();
            let full = c.sha.to_lowercase();
            (!h.is_empty() && (h == s || h.starts_with(&s) || s.starts_with(&h)))
                || (!full.is_empty() && (full == s || full.starts_with(&s) || s.starts_with(&full)))
        })
    }

    // ---- files changed ----

    pub(crate) fn set_diff_layout(&mut self, next: DiffLayout, cx: &mut Context<Self>) {
        self.diff_layout = next;
        self.state()
            .update(cx, |s, _| write_diff_layout(s.storage_mut(), next));
        cx.notify();
    }

    pub(crate) fn set_file_list_mode(&mut self, next: FileListMode, cx: &mut Context<Self>) {
        self.file_list_mode = next;
        self.state()
            .update(cx, |s, _| write_file_list_mode(s.storage_mut(), next));
        cx.notify();
    }

    pub(crate) fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.sidebar_open = open;
        self.state()
            .update(cx, |s, _| write_review_sidebar(s.storage_mut(), open));
        cx.notify();
    }

    /// The file list's `onSelect`: with comments-only on, jump to the first note.
    fn on_list_select(&mut self, path: &str, highlight: Option<i64>, cx: &mut Context<Self>) {
        let hl = match highlight {
            Some(h) => h,
            None if self.comments_only => self
                .comments_by_path(cx)
                .iter()
                .find(|(p, _)| p == path)
                .and_then(|(_, v)| v.first())
                .map_or(0, |c| c.line),
            None => 0,
        };
        self.select_path(path, hl, cx);
    }

    /// `selectPath(path, { highlight })`.
    pub(crate) fn select_path(&mut self, path: &str, highlight: i64, cx: &mut Context<Self>) {
        self.highlight = highlight;
        self.stack.select_path(path);
        cx.notify();
    }

    /// `jumpToImpact`: Files changed when the PR touches `path`, else the file on GitHub at
    /// the PR head.
    pub(crate) fn jump_to_impact(&mut self, path: &str, line: i64, cx: &mut Context<Self>) {
        if path.is_empty() || self.files_pending(cx) {
            return;
        }
        let paths: Vec<String> = self
            .review_files(cx)
            .iter()
            .map(|f| f.path.clone())
            .collect();
        let resolved =
            recur_code::view::finding_paths::match_finding_path(path, &paths).to_string();
        if !resolved.is_empty() {
            self.set_tab(PrTab::Files, cx);
            self.select_path(&resolved, line, cx);
            return;
        }
        let root = {
            let s = self.state().read(cx);
            let ws = self.agent_ws(cx);
            s.workspace(&ws)
                .map(|w| w.worktree_path.trim_end_matches('/').to_string())
                .unwrap_or_default()
        };
        let rel = if !root.is_empty() && path.starts_with(&format!("{root}/")) {
            path[root.len() + 1..].to_string()
        } else {
            path.to_string()
        };
        if rel.starts_with('/') {
            return;
        }
        let pr = self.pr(cx).unwrap_or_default();
        let href = recur_forge::github::blob_url(&pr.url, &pr.head_sha, &rel, line);
        if !href.is_empty() {
            recur_content::links::open_external_url(&href, cx);
        }
    }

    /// `openInWorkspace(path, line?)`.
    pub(crate) fn open_in_workspace(
        state: &Entity<AppState>,
        path: &str,
        line: Option<i64>,
        cx: &mut App,
    ) {
        let ws = {
            let s = state.read(cx);
            s.pr(&s.selected_pr_id)
                .map(|p| p.workspace_id.clone())
                .unwrap_or_default()
        };
        if ws.is_empty() {
            return;
        }
        state.update(cx, |s, cx| {
            s.observatory_jump = Some(ObsJump {
                path: path.to_string(),
                line: line.unwrap_or(0).max(0) as u32,
                workspace_id: ws.clone(),
                prefer_diff: true,
                ..Default::default()
            });
            s.open_workspace(&ws, false, cx);
        });
    }

    /// `openFinding`: the Findings panel in the PR's workspace, else the conversation.
    pub(crate) fn open_finding(&mut self, f: &Finding, cx: &mut Context<Self>) {
        let ws = self
            .pr_ref(cx)
            .map(|p| p.workspace_id.clone())
            .unwrap_or_default();
        if !ws.is_empty() && !f.id.is_empty() {
            self.state().update(cx, |s, cx| {
                s.observatory_jump = Some(ObsJump {
                    workspace_id: ws.clone(),
                    panel: "findings".into(),
                    finding_id: f.id.clone(),
                    path: f.file.clone(),
                    line: if f.line > 0 { f.line as u32 } else { 0 },
                    ..Default::default()
                });
                s.open_workspace(&ws, false, cx);
            });
            return;
        }
        self.set_tab(PrTab::Conv, cx);
    }

    /// `commentsByPath`.
    pub(crate) fn comments_by_path(&self, cx: &App) -> Vec<(String, Vec<PrComment>)> {
        let items = self.thread_items(cx);
        let paths: Vec<String> = self
            .review_files(cx)
            .iter()
            .map(|f| f.path.clone())
            .collect();
        recur_engine::view::comment_thread::unresolved_line_notes_by_path(&items, &paths)
            .into_iter()
            .map(|(p, v)| (p, v.into_iter().cloned().collect()))
            .collect()
    }

    // ---- checks ----

    /// The 1s tick while a check is pending (`checkNow`).
    fn sync_check_tick(&mut self, cx: &mut Context<Self>) {
        let checks = self
            .pr_ref(cx)
            .and_then(|p| p.checks.as_deref())
            .unwrap_or_default();
        let pending = self.on_route
            && checks
                .iter()
                .any(|c| recur_forge::view::is_pending_check(&c.status));
        if self.pr_ref(cx).is_some() {
            recur_forge::view::prune_pending_clocks(&mut self.clocks.borrow_mut(), checks);
        }
        if !pending {
            self.check_tick = None;
            return;
        }
        if self.check_tick.is_some() {
            return;
        }
        self.check_tick = Some(cx.spawn(async move |d, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if d.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
    }

    /// `checkNow`: wall time, advanced by the executor's clock so a test's advanced clock
    /// ticks durations (`page.clock`).
    pub(crate) fn check_now(cx: &mut App) -> chrono::DateTime<Utc> {
        let at = cx.background_executor().now();
        let base = cx
            .try_global::<ClockBase>()
            .map(|b| (b.utc, b.at))
            .unwrap_or_else(|| {
                let b = (Utc::now(), at);
                cx.set_global(ClockBase { utc: b.0, at: b.1 });
                b
            });
        let d = at.saturating_duration_since(base.1);
        base.0 + chrono::TimeDelta::from_std(d).unwrap_or_default()
    }

    /// Pins the wall clock `check_now` counts from (tests).
    pub fn set_clock(utc: chrono::DateTime<Utc>, cx: &mut App) {
        let at = cx.background_executor().now();
        cx.set_global(ClockBase { utc, at });
    }

    /// The markdown body for a conversation row (`.cm-md`), kept per comment id.
    pub(crate) fn md(
        &mut self,
        key: &str,
        body: &str,
        cx: &mut Context<Self>,
    ) -> Entity<MarkdownView> {
        if let Some(v) = self.mds.get(key) {
            v.update(cx, |v, cx| {
                if v.text().as_ref() != body {
                    v.set_text(body.to_string(), cx)
                }
            });
            return v.clone();
        }
        let v = cx.new(|cx| {
            let mut v = MarkdownView::new(body.to_string(), MarkdownClass::ChatCanvas, false, cx);
            v.set_sheet(note_sheet(), cx);
            v
        });
        let state = self.state().clone();
        let sub = cx.guarded_subscribe(&v, move |_, _, ev: &MarkdownEvent, cx| match ev {
            MarkdownEvent::LightboxOpen(open) => {
                let open = *open;
                state.update(cx, |s, cx| {
                    s.set_chat_occluder("attachment-lightbox", open, cx)
                });
            }
            MarkdownEvent::Toast { error, message } => {
                let kind = if *error {
                    ToastKind::Error
                } else {
                    ToastKind::Info
                };
                toast(kind, message.clone(), cx);
            }
            MarkdownEvent::TextChanged => {}
        });
        self._subs.push(sub);
        self.mds.insert(key.to_string(), v.clone());
        v
    }

    /// The reaction row for a conversation item.
    pub(crate) fn reactions(
        &mut self,
        c: &PrComment,
        cx: &mut Context<Self>,
    ) -> Entity<CommentReactions> {
        let can =
            recur_engine::view::comment_thread::can_react_pr_comment(self.pr_ref(cx), Some(c));
        if let Some(r) = self.rxns.get(&c.id) {
            r.update(cx, |r, cx| r.set(c.clone(), can, cx));
            return r.clone();
        }
        let state = self.state().clone();
        let r = cx.new(|_| CommentReactions::new(state, c.clone(), can));
        self.rxns.insert(c.id.clone(), r.clone());
        r
    }
}

/// `flushPendingFixChecks`: fire a queued `/fix-checks` once the PR has an Agent worktree.
fn flush(state: &Entity<AppState>, ui: &Entity<PrUi>, cx: &mut App) {
    let run = {
        let s = state.read(cx);
        let mut pending = ui.read(cx).pending_fix_checks.clone();
        let out = flush_pending_fix_checks(&mut pending, &s.prs, &s.workspaces);
        out.map(|o| (o, pending))
    };
    if let Some(((ws, args), rest)) = run {
        ui.update(cx, |u, _| u.pending_fix_checks = rest);
        state.update(cx, |s, cx| {
            s.request_prompt_action(&ws, "fix-checks", &args, cx)
        });
    }
}

/// Where `check_now` starts counting.
struct ClockBase {
    utc: chrono::DateTime<Utc>,
    at: std::time::Instant,
}

impl gpui_kit::Global for ClockBase {}

/// A ⋯ menu item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoreAction {
    OpenGitHub,
    CopyLink,
    Quote,
    Edit,
    Delete,
}

impl PrDetail {
    /// Focus the conversation's comment box; whether it took focus (a locked conversation
    /// has none).
    pub fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let f = gpui_kit::Focusable::focus_handle(self.composer.area.read(cx), cx);
        window.focus(&f, cx);
        f.is_focused(window)
    }
}

/// The PR detail the specs drive, for reading its fields.
#[cfg(test)]
pub(crate) mod spec_handle {
    pub struct Live(pub gpui_kit::WeakEntity<super::PrDetail>);
    impl gpui_kit::Global for Live {}

    pub fn composer_focused(window: &gpui_kit::Window, cx: &gpui_kit::App) -> bool {
        cx.global::<Live>().0.upgrade().is_some_and(|d| {
            gpui_kit::Focusable::focus_handle(d.read(cx).composer.area.read(cx), cx)
                .is_focused(window)
        })
    }

    /// The dock's Hide toggle when `open`, else PR detail's Show toggle.
    pub fn toggle_focused(open: bool, window: &gpui_kit::Window, cx: &gpui_kit::App) -> bool {
        if open {
            return super::render::dock_toggle_focus(cx).is_some_and(|f| f.is_focused(window));
        }
        cx.global::<Live>()
            .0
            .upgrade()
            .is_some_and(|d| d.read(cx).toggle_focus.is_focused(window))
    }

    pub fn reply_text(cx: &gpui_kit::App) -> String {
        cx.global::<Live>()
            .0
            .upgrade()
            .map(|d| d.read(cx).reply.value(cx))
            .unwrap_or_default()
    }
}
