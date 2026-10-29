//! Children of Agent chats (Go `chat_child.go`): Recur's subagents. A chat
//! child is a workspace with a parent and no parent steps (loop fan-out
//! children carry steps and merge through their parent's board). When a
//! chat child settles, the parent tab gets one turn carrying its report and
//! decides: merge_child, archive_child, or keep reading it.
//! `fan_out.outcome` on the child row is the lifecycle marker the card is
//! rebuilt from: "" running, settled, merged, closed; report-pending is an
//! internal delivery marker.

use std::sync::Arc;
use std::time::{Duration, Instant};

use recur_agent::attach::DISPLAY_HIDDEN_SENTINEL;
use recur_domain::{
    ChildFanOut, FEED_CARD_FAN_OUT, FeedItem, Group, Workspace, WorkspaceState, finding_loc,
};

use crate::Engine;
use crate::chatlife::ChatTurn;
use crate::codes::{PAIR_AGENT_CODE, chat_turn_key, is_chat_child, is_pair_chat_code};
use crate::loops::fanout::{FanOutChild, FanOutWave};
use crate::publish::status_label;
use crate::text::{html_escape, truncate};
use crate::turn::finalize::CHAT_STOPPED_REPLY;

pub const CHILD_OUTCOME_SETTLED: &str = "settled";
pub const CHILD_OUTCOME_MERGED: &str = "merged";
pub const CHILD_OUTCOME_CLOSED: &str = "closed";
pub const CHILD_OUTCOME_REPORT_PENDING: &str = "report-pending";
pub const CHILD_REPLY_MAX: usize = 4000;
pub const CHILD_NOTICE_REPLY_MAX: usize = 1200;
pub const CHILD_DIFF_MAX: usize = 12000;
pub const PARENT_STEER_PREFIX: &str = "Parent workspace sent this while you were working. Treat it as additional instruction and continue:\n\n";

/// The parent tab a chat child reports to.
pub fn child_chat(w: &Workspace) -> String {
    match &w.fan_out {
        Some(f) if !f.chat.trim().is_empty() => f.chat.clone(),
        _ => PAIR_AGENT_CODE.into(),
    }
}

/// The child's create failed (it may have no worktree).
pub fn child_create_failed(w: &Workspace) -> bool {
    w.state == WorkspaceState::Failed || w.phase == "failed" || !w.last_error.trim().is_empty()
}

/// The child's lifecycle is past reporting (settled, merged, closed, …).
pub fn chat_child_outcome_done(w: &Workspace) -> bool {
    match &w.fan_out {
        Some(f) => !f.outcome.is_empty() && f.outcome != CHILD_OUTCOME_REPORT_PENDING,
        None => false,
    }
}

/// The notice turn's display: the hidden marker (never steers, runs as its
/// own turn) and the label the transcript renders as a Subagent row.
pub fn child_notice_display(child: &Workspace) -> String {
    format!(
        "{DISPLAY_HIDDEN_SENTINEL}{} finished ({})",
        child.goal, child.id
    )
}

/// The child's state as the parent's report reads it.
pub fn describe_child_state(w: &Workspace) -> String {
    if matches!(w.state, WorkspaceState::Failed | WorkspaceState::Stopped) {
        return w.state.as_str().to_owned();
    }
    match w.fan_out.as_ref().map(|f| f.outcome.as_str()) {
        Some(CHILD_OUTCOME_MERGED) => "been merged".into(),
        Some(CHILD_OUTCOME_CLOSED) => "been closed".into(),
        _ => "finished".into(),
    }
}

/// A chat-child live agent status (matches the sidebar/composer busy set).
pub fn is_live_agent_status(status: &str) -> bool {
    matches!(status, "working" | "directing" | "planning")
}

async fn until<T>(deadline: Instant, f: impl std::future::Future<Output = T>) -> Option<T> {
    tokio::time::timeout_at(deadline.into(), f).await.ok()
}

/// Stdout of a git run in `dir` whatever its exit status (Go
/// `cmd.Output()` keeps stdout on an exit error; `git diff --no-index`
/// exits 1 when the files differ).
async fn git_stdout(dir: &str, args: &[&str], deadline: Instant) -> Vec<u8> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match until(deadline, cmd.output()).await {
        Some(Ok(out)) => out.stdout,
        _ => Vec::new(),
    }
}

/// The worktree's uncommitted changes: tracked edits (`git diff HEAD`) plus
/// each untracked file diffed against /dev/null, without touching the index
/// (merge_child stages them later).
pub async fn child_uncommitted_diff(worktree: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut b = String::new();
    if let Some(Ok(out)) = until(
        deadline,
        recur_git::command(["-C", worktree, "diff", "HEAD"]).output(),
    )
    .await
    {
        b.push_str(&String::from_utf8_lossy(&out));
    }
    let st = until(deadline, recur_git::status_map(worktree))
        .await
        .flatten()
        .unwrap_or_default();
    let mut untracked: Vec<&String> = st
        .iter()
        .filter(|(_, l)| l.as_str() == "u")
        .map(|(p, _)| p)
        .collect();
    untracked.sort();
    for p in untracked {
        let out = git_stdout(
            worktree,
            &["diff", "--no-index", "--", "/dev/null", p],
            deadline,
        )
        .await;
        b.push_str(&String::from_utf8_lossy(&out));
    }
    b.trim().to_owned()
}

impl Engine {
    /// A child of `parent_id`, archived rows included.
    pub fn chat_child(&self, parent_id: &str, child_id: &str) -> Option<Workspace> {
        self.store
            .workspace(child_id)
            .filter(|c| c.parent_id == parent_id && is_chat_child(c))
    }

    /// A chat child never leaves its creation state on plain turns, so
    /// settled means: no walk, no live or resuming turn, no live agent, no
    /// parked question, and the launch's first turn finished. A child that
    /// engaged a loop defers to the loop rule.
    pub fn child_settled_for_chat(&self, child_id: &str) -> bool {
        let Some(ws) = self.store.workspace(child_id) else {
            return true;
        };
        if self.running_loop_by_id(child_id).is_some()
            || self.workspace_has_live_chat(child_id)
            || self.chat_resuming(child_id)
            || ws.scheduled_prompt_pending
            || self.ask_user_workspace_busy(child_id)
        {
            return false;
        }
        if self
            .store
            .agents(child_id)
            .iter()
            .any(|a| is_live_agent_status(&a.status))
        {
            return false;
        }
        if !ws.loop_chat_code.trim().is_empty() {
            return self.child_settled(child_id);
        }
        !matches!(
            ws.state,
            WorkspaceState::Planning
                | WorkspaceState::Coding
                | WorkspaceState::Reviewing
                | WorkspaceState::Verifying
        )
    }

    pub(crate) fn chat_child_ready_to_report(&self, child: &Workspace) -> bool {
        if chat_child_outcome_done(child) || !self.child_settled_for_chat(&child.id) {
            return false;
        }
        !child.worktree_path.is_empty() || child_create_failed(child)
    }

    /// Reports the child to its parent tab once it settles; one watcher per
    /// child.
    pub(crate) fn spawn_chat_child_watcher(&self, parent_id: &str, child_id: &str) {
        let (eng, p, c) = (self.arc(), parent_id.to_owned(), child_id.to_owned());
        tokio::spawn(async move { eng.watch_chat_child(&p, &c).await });
    }

    pub(crate) async fn watch_chat_child(&self, parent_id: &str, child_id: &str) {
        if !self.chat_child_watchers.lock().insert(child_id.to_owned()) {
            return;
        }
        loop {
            if self.is_shutting_down() {
                break;
            }
            let Some(child) = self.chat_child(parent_id, child_id).filter(|c| !c.archived) else {
                break;
            };
            if self.chat_child_ready_to_report(&child) && self.report_child(parent_id, &child).await
            {
                // The report turn may have messaged the child into a new
                // turn: keep watching for its next report.
                match self.chat_child(parent_id, child_id) {
                    Some(c) if !chat_child_outcome_done(&c) => {}
                    _ => break,
                }
            }
            tokio::time::sleep(self.fan_out_poll()).await;
        }
        self.chat_child_watchers.lock().remove(child_id);
    }

    /// Restarts the watchers a restart dropped. A report that was pending
    /// when the app died but whose notice is already on the parent tab is
    /// marked settled, not resent (AutoResume replays the parent's turn).
    pub(crate) async fn rearm_chat_children(&self) {
        for w in self.store.workspaces() {
            if w.archived || !is_chat_child(&w) || chat_child_outcome_done(&w) {
                continue;
            }
            let pending = w
                .fan_out
                .as_ref()
                .is_some_and(|f| f.outcome == CHILD_OUTCOME_REPORT_PENDING);
            if pending
                && self
                    .notice_delivered(&w.parent_id, &child_chat(&w), &w)
                    .await
            {
                self.mark_child_reported(&w.parent_id, &w.id, None);
                continue;
            }
            if self
                .store
                .workspace(&w.parent_id)
                .is_some_and(|p| !p.archived)
            {
                self.spawn_chat_child_watcher(&w.parent_id, &w.id);
            }
        }
    }

    /// Re-watches a reported child that starts a new turn (a retry, a
    /// follow-up), so its parent gets another report. Called once the turn
    /// is live.
    pub(crate) fn reopen_chat_child(&self, child_id: &str) {
        let mut reopened = false;
        let updated = self.store.update_workspace(child_id, |w| {
            let reportable = !w.archived && is_chat_child(w);
            if let Some(f) = w.fan_out.as_mut().filter(|_| reportable)
                && (f.outcome == CHILD_OUTCOME_SETTLED
                    || f.outcome == CHILD_OUTCOME_REPORT_PENDING
                    || f.outcome.starts_with("conflict: "))
            {
                f.outcome.clear();
                f.note.clear();
                reopened = true;
            }
        });
        let Some(updated) = updated.filter(|_| reopened) else {
            return;
        };
        if self
            .store
            .workspace(&updated.parent_id)
            .is_none_or(|p| p.archived)
        {
            return;
        }
        self.emit_child_card(&updated.parent_id, &updated);
        self.spawn_chat_child_watcher(&updated.parent_id, child_id);
    }

    /// Stamps the lifecycle marker and refreshes the card.
    pub fn set_chat_child_outcome(
        &self,
        parent_id: &str,
        child_id: &str,
        outcome: &str,
        note: &str,
    ) {
        let updated = self.store.update_workspace(child_id, |w| {
            let f = w.fan_out.get_or_insert_with(ChildFanOut::default);
            f.outcome = outcome.to_owned();
            f.note = note.to_owned();
        });
        if let Some(u) = updated
            && outcome != CHILD_OUTCOME_REPORT_PENDING
        {
            self.emit_child_card(parent_id, &u);
        }
    }

    /// A removed chat child reports closed to its parent unless it already
    /// merged or closed.
    pub(crate) fn close_chat_child_on_remove(&self, l: &Workspace) {
        if !is_chat_child(l) {
            return;
        }
        let out = l.fan_out.as_ref().map(|f| f.outcome.as_str()).unwrap_or("");
        if out == CHILD_OUTCOME_MERGED || out == CHILD_OUTCOME_CLOSED {
            return;
        }
        self.set_chat_child_outcome(&l.parent_id, &l.id, CHILD_OUTCOME_CLOSED, "");
    }

    /// The subagent wave for a child the parent tab creates now: siblings
    /// created by one live parent turn share a wave (and a card); a new turn
    /// opens the next, numbered from the parent's `fan_out_waves`.
    pub(crate) fn chat_child_wave(&self, parent_id: &str, code: &str) -> i64 {
        let turn = self.live_chat_turn(parent_id, code);
        if let Some(t) = &turn
            && let Some(w) = self.child_waves.lock().get(&turn_key(t))
        {
            return *w;
        }
        let mut wave = 0;
        self.store.update_workspace(parent_id, |w| {
            w.fan_out_waves += 1;
            wave = w.fan_out_waves;
        });
        if let Some(t) = &turn {
            self.child_waves.lock().insert(turn_key(t), wave);
        }
        wave
    }

    /// Forgets the wave a finished parent turn was creating.
    pub(crate) fn forget_child_wave(&self, turn: &Arc<ChatTurn>) {
        self.child_waves.lock().remove(&turn_key(turn));
    }

    /// (Re)publishes the child's wave card on the parent feed under a stable
    /// id: one row per sibling of the same parent turn, no step chips;
    /// archived siblings stay on the card.
    pub fn emit_child_card(&self, parent_id: &str, child: &Workspace) {
        let wave = child.fan_out.as_ref().map_or(0, |f| f.wave);
        let chat = child_chat(child);
        let mut rows = Vec::new();
        let mut done = true;
        for c in self.store.workspaces() {
            let Some(f) = c.fan_out.as_ref() else {
                continue;
            };
            if c.parent_id != parent_id
                || !is_chat_child(&c)
                || f.wave != wave
                || child_chat(&c) != chat
            {
                continue;
            }
            if f.outcome != CHILD_OUTCOME_MERGED && f.outcome != CHILD_OUTCOME_CLOSED {
                done = false;
            }
            rows.push(FanOutChild {
                id: c.id.clone(),
                title: c.goal.clone(),
                steps: Vec::new(),
                mode: f.mode.clone(),
                state: f.outcome.clone(),
                note: f.note.clone(),
            });
        }
        if rows.is_empty() {
            return;
        }
        let title = if rows.len() > 1 {
            "Subagents"
        } else {
            "Subagent"
        };
        let mut text = format!("{title}: {} child workspace(s) — ", rows.len());
        for (i, r) in rows.iter().enumerate() {
            if i > 0 {
                text.push_str(" · ");
            }
            let state = if r.state.is_empty() {
                "running"
            } else {
                &r.state
            };
            text.push_str(&format!(
                "{} ({})",
                html_escape(&r.title),
                html_escape(state)
            ));
        }
        let Ok(data) = serde_json::to_string(&FanOutWave {
            wave,
            title: title.into(),
            after: Vec::new(),
            children: rows,
            done,
        }) else {
            return;
        };
        self.emit_workspace_card(
            parent_id,
            FeedItem {
                id: format!("fanout-{parent_id}-c{wave}-{chat}"),
                agent_code: "MA".into(),
                name: "Recur".into(),
                text,
                chat_code: chat,
                card: FEED_CARD_FAN_OUT.into(),
                data,
                ..FeedItem::default()
            },
        );
    }

    /// Sends one turn to the parent tab that created the child. The input
    /// is agent-to-agent (hidden bubble); the reply and the card are what
    /// the human sees. Delivery is the notice persisted on the parent tab,
    /// not the reply text (empty for a parked ask_user, "Stopped by user"
    /// on Stop). A send the engine refused persists nothing and is retried.
    pub(crate) async fn report_child(&self, parent_id: &str, child: &Workspace) -> bool {
        self.set_chat_child_outcome(parent_id, &child.id, CHILD_OUTCOME_REPORT_PENDING, "");
        let chat = child_chat(child);
        let before = self.notice_count(parent_id, &chat, child).await;
        let (model, flags, mode) = self.workspace_chat_selection(parent_id, &chat);
        let notice = self.child_notice(parent_id, child).await;
        let reply = self
            .send_workspace_message(
                parent_id,
                &chat,
                &notice,
                &model,
                Some(&flags),
                &[],
                &mode,
                &child_notice_display(child),
            )
            .await;
        if self.notice_count(parent_id, &chat, child).await > before {
            self.mark_child_reported(parent_id, &child.id, None);
            return true;
        }
        if reply == CHAT_STOPPED_REPLY {
            // Stop while the notice queued behind a live turn: honor it, no
            // automatic retry. The card row carries the explanation (plain
            // feed lines do not render on Agent tabs).
            self.mark_child_reported(
                parent_id,
                &child.id,
                Some(&format!(
                    "report cancelled by Stop — ask this chat to read_child {}",
                    child.id
                )),
            );
            return true;
        }
        false
    }

    pub(crate) async fn notice_delivered(
        &self,
        parent_id: &str,
        chat: &str,
        child: &Workspace,
    ) -> bool {
        self.notice_count(parent_id, chat, child).await > 0
    }

    /// The child's notices on the parent tab (a reopened child reports again
    /// under the same display text).
    pub(crate) async fn notice_count(
        &self,
        parent_id: &str,
        chat: &str,
        child: &Workspace,
    ) -> usize {
        let want = child_notice_display(child);
        let want = want.trim();
        self.workspace_chat_items(parent_id, chat)
            .await
            .iter()
            .filter(|it| it.role == "user" && it.kind == "text" && it.text.trim() == want)
            .count()
    }

    /// Moves report-pending to settled. The parent's report turn may
    /// already have merged, closed or conflicted the child; those stand.
    pub(crate) fn mark_child_reported(&self, parent_id: &str, child_id: &str, note: Option<&str>) {
        let updated = self.store.update_workspace(child_id, |w| {
            if let Some(f) = w.fan_out.as_mut()
                && f.outcome == CHILD_OUTCOME_REPORT_PENDING
            {
                f.outcome = CHILD_OUTCOME_SETTLED.into();
                if let Some(n) = note {
                    f.note = n.to_owned();
                }
            }
        });
        if let Some(u) = updated
            && u.fan_out
                .as_ref()
                .is_some_and(|f| f.outcome == CHILD_OUTCOME_SETTLED)
        {
            self.emit_child_card(parent_id, &u);
        }
    }

    /// The short re-engage turn: state, the capped reply, committed file
    /// count and the verbs; read_child has the rest.
    async fn child_notice(&self, parent_id: &str, child: &Workspace) -> String {
        let mut b = format!(
            "Subagent {} ({:?}) has {}.",
            child.id,
            child.goal,
            describe_child_state(child)
        );
        let err = child.last_error.trim();
        if !err.is_empty() {
            b.push_str(&format!(" Create failed: {err}."));
        }
        let (paths, _) = self.child_changed_diff(parent_id, child).await;
        if paths.is_empty() {
            b.push_str(" It committed no files.");
        } else {
            b.push_str(&format!(
                " It committed {} file(s) on branch {}.",
                paths.len(),
                child.branch
            ));
        }
        let dirty = self.child_dirty_paths(child).await.len();
        if dirty > 0 {
            b.push_str(&format!(
                " It has {dirty} uncommitted file(s) that merge_child will commit."
            ));
        }
        let reply = self.store.last_assistant_text(&child.id);
        let reply = reply.trim();
        if !reply.is_empty() {
            b.push_str(&format!(
                "\n\nIts reply:\n{}",
                truncate(reply, CHILD_NOTICE_REPLY_MAX)
            ));
        }
        let id = &child.id;
        b.push_str(&format!(
            "\n\nread_child {id} for the full report and diff; merge_child {id} to land its commits here; archive_child {id} to close it without merging. Each subagent reports in its own turn; do not assume another child is still running — its report arrives when it finishes."
        ));
        b
    }

    /// The child's committed changes against the parent branch; empty for
    /// archived children (the worktree is gone).
    async fn child_changed_diff(
        &self,
        parent_id: &str,
        child: &Workspace,
    ) -> (Vec<String>, String) {
        let Some(parent) = self.store.workspace(parent_id) else {
            return Default::default();
        };
        if child.worktree_path.is_empty() || child.archived {
            return Default::default();
        }
        let Some(proj) = self.store.project(&parent.project_id) else {
            return Default::default();
        };
        tokio::time::timeout(
            Duration::from_secs(30),
            self.git_manager(&proj.repo_path)
                .changed_diff(&child.worktree_path, &parent.branch),
        )
        .await
        .unwrap_or_default()
    }

    async fn child_dirty_paths(&self, child: &Workspace) -> Vec<String> {
        if child.worktree_path.is_empty() || child.archived {
            return Vec::new();
        }
        let st = tokio::time::timeout(
            Duration::from_secs(15),
            recur_git::status_map(&child.worktree_path),
        )
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
        let mut paths: Vec<String> = st.into_keys().collect();
        paths.sort();
        paths
    }

    /// The read_child body: state, final reply, handoff, plan, findings,
    /// changes against the parent branch (with the diff when `with_diff`)
    /// and artifacts, then the verbs that act on the child.
    pub(crate) async fn child_report(
        &self,
        parent_id: &str,
        child: &Workspace,
        with_diff: bool,
    ) -> String {
        let mut b = String::new();
        let running = !child.archived && !self.child_settled_for_chat(&child.id);
        if running {
            b.push_str(&format!(
                "Subagent {} ({:?}, branch {}) is STILL RUNNING. What follows is a partial snapshot; its commits and reply are not final. You will get a turn here with its report when it finishes — do not wait, poll, merge, or archive it now.\n\n",
                child.id,
                child.goal,
                child.branch
            ));
        } else {
            b.push_str(&format!(
                "Subagent {} ({:?}, branch {}) has {}",
                child.id,
                child.goal,
                child.branch,
                describe_child_state(child)
            ));
            if child.archived {
                b.push_str("; it is archived");
            }
            b.push_str(".\n\n");
        }
        let err = child.last_error.trim();
        if !err.is_empty() {
            b.push_str(&format!("Error: {err}\n\n"));
        }
        let reply = self.store.last_assistant_text(&child.id);
        let reply = reply.trim();
        if !reply.is_empty() {
            b.push_str(&format!(
                "Final reply:\n{}\n\n",
                truncate(reply, CHILD_REPLY_MAX)
            ));
        }
        let h = child.handoff.trim();
        if !h.is_empty() {
            b.push_str(&format!("Handoff notes: {h}\n\n"));
        }
        if let Some(plan) = &child.plan {
            let t = plan.text();
            let t = t.trim();
            if !t.is_empty() {
                b.push_str(&format!("Plan:\n{}\n\n", truncate(t, CHILD_REPLY_MAX)));
            }
        }
        let findings = self.store.findings(&child.id);
        if !findings.is_empty() {
            b.push_str("Findings:\n");
            for f in &findings {
                let mut loc = finding_loc(&f.file, f.line, f.end_line);
                if !loc.is_empty() {
                    loc.insert(0, ' ');
                }
                b.push_str(&format!(
                    "- [{}] {}{loc}: {}\n",
                    f.status,
                    f.severity.as_str(),
                    f.message
                ));
            }
            b.push('\n');
        }
        if let Some(parent) = self.store.workspace(parent_id)
            && !child.worktree_path.is_empty()
            && !child.archived
        {
            let (paths, body) = self.child_changed_diff(parent_id, child).await;
            if paths.is_empty() {
                b.push_str(&format!(
                    "Committed changes vs {}: none.\n\n",
                    parent.branch
                ));
            } else {
                b.push_str(&format!(
                    "Committed changes vs {} ({} file(s)): {}\n",
                    parent.branch,
                    paths.len(),
                    paths.join(", ")
                ));
                if with_diff && !body.trim().is_empty() {
                    b.push_str(&format!(
                        "```diff\n{}\n```\n",
                        truncate(&body, CHILD_DIFF_MAX)
                    ));
                }
                b.push('\n');
            }
            let dirty = self.child_dirty_paths(child).await;
            if !dirty.is_empty() {
                b.push_str(&format!(
                    "Uncommitted (staged/unstaged/untracked, {} file(s)): {}\n",
                    dirty.len(),
                    dirty.join(", ")
                ));
                if with_diff {
                    let out = child_uncommitted_diff(&child.worktree_path).await;
                    if !out.is_empty() {
                        b.push_str(&format!(
                            "```diff\n{}\n```\n",
                            truncate(&out, CHILD_DIFF_MAX)
                        ));
                    }
                }
                b.push('\n');
            }
        }
        if let Ok(arts) = self.list_artifacts(&child.id)
            && !arts.is_empty()
        {
            b.push_str("Artifacts:\n");
            for a in &arts {
                b.push_str(&format!("- {} — {} ({})\n", a.id, a.title, a.kind));
            }
            b.push('\n');
        }
        if !child.archived && !running {
            let id = &child.id;
            b.push_str(&format!(
                "You can: read_child {id} (full diff), read_artifact with workspace_id={id}, merge_child {id} to land its commits in this worktree, or archive_child {id} to close it without merging."
            ));
        }
        b
    }

    /// The read_child tool.
    pub async fn read_child_from_chat(&self, parent_id: &str, child_id: &str) -> String {
        match self.chat_child(parent_id, child_id) {
            Some(child) => self.child_report(parent_id, &child, true).await,
            None => format!("read_child: {child_id} is not a child of this workspace"),
        }
    }

    /// The message_child tool: steers a running child (chat child or loop
    /// fan-out) mid-turn, or starts a new turn on a settled chat child so it
    /// reports again.
    pub async fn message_child_from_chat(
        &self,
        parent_id: &str,
        child_id: &str,
        message: &str,
    ) -> String {
        let message = message.trim();
        if message.is_empty() {
            return "message_child: message is required".into();
        }
        let Some(child) = self.store.workspace(child_id).filter(|c| {
            !parent_id.is_empty()
                && !c.parent_id.is_empty()
                && c.parent_id == parent_id
                && !c.archived
        }) else {
            return format!("message_child: {child_id} is not a child of this workspace");
        };
        let prefix = chat_turn_key(child_id, "");
        let mut code = PAIR_AGENT_CODE.to_owned();
        for (key, _) in self.live_chat_turns() {
            let Some(tab) = key.strip_prefix(&prefix) else {
                continue;
            };
            if !is_pair_chat_code(tab) {
                continue;
            }
            if tab == PAIR_AGENT_CODE {
                code = tab.to_owned();
                break;
            }
            if code == PAIR_AGENT_CODE {
                code = tab.to_owned();
            }
        }
        let display = format!("From parent {parent_id}: {message}");
        if self
            .try_inject_chat(
                child_id,
                &code,
                &format!("{PARENT_STEER_PREFIX}{message}"),
                &display,
            )
            .await
        {
            return "Steered the child mid-turn; the child accepted the message.".into();
        }
        if let Some(lp) = self.running_loop_by_id(child_id) {
            lp.queue_steer(message);
            return "Steered the child's running loop; this call does not wait.".into();
        }
        if !is_chat_child(&child)
            || child.worktree_path.is_empty()
            || !self.child_settled_for_chat(child_id)
        {
            return format!(
                "message_child: {child_id} is still starting or already finished; wait, or read_child / merge_child / archive_child"
            );
        }
        let (model, flags, mode) = self.workspace_chat_selection(child_id, &code);
        let (eng, c, m) = (self.arc(), child_id.to_owned(), message.to_owned());
        tokio::spawn(async move {
            eng.send_workspace_message(&c, &code, &m, &model, Some(&flags), &[], &mode, &display)
                .await;
        });
        "The child had finished, so this started a new turn on it; you get a new turn with its report when it finishes again.".into()
    }

    /// read_artifact with a workspace_id scoped to own children.
    pub fn read_child_artifact_from_chat(
        &self,
        parent_id: &str,
        child_id: &str,
        artifact_id: &str,
    ) -> crate::Result<String> {
        if self.chat_child(parent_id, child_id).is_none() {
            return Err(crate::EngineError(format!(
                "read_artifact: {child_id} is not a child of this workspace"
            )));
        }
        self.read_artifact(child_id, artifact_id)
    }

    /// The archive_child tool: closes a settled child without merging (its
    /// worktree and branch go).
    pub async fn archive_child_from_chat(&self, parent_id: &str, child_id: &str) -> String {
        let Some(child) = self.chat_child(parent_id, child_id).filter(|c| !c.archived) else {
            return format!("archive_child: {child_id} is not a live child of this workspace");
        };
        if !self.child_settled_for_chat(child_id) {
            return format!(
                "archive_child: {child_id} is still running; stop it or wait for it to settle"
            );
        }
        let mut dropped = 0;
        if let Some(parent) = self.store.workspace(parent_id)
            && !child.worktree_path.is_empty()
            && let Some(proj) = self.store.project(&parent.project_id)
        {
            dropped = tokio::time::timeout(
                Duration::from_secs(30),
                self.git_manager(&proj.repo_path)
                    .commits_ahead(&child.worktree_path, &parent.branch),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(0);
        }
        self.set_chat_child_outcome(parent_id, child_id, CHILD_OUTCOME_CLOSED, "");
        if let Some(u) = self.store.update_workspace(child_id, |w| {
            w.state = WorkspaceState::Stopped;
            w.status_label = status_label(&WorkspaceState::Stopped);
            w.phase = "closed by parent".into();
            w.group = Group::Ready;
            w.checkpoint = None;
        }) {
            self.publish_loop_updated(&u.id, u.state.clone(), &u.phase, u.iteration);
        }
        if let Err(e) = self.remove_workspace(child_id).await {
            return format!("archive_child: {e}");
        }
        if dropped > 0 {
            return format!(
                "Archived subagent {} ({:?}) without merging: its worktree and branch {} are deleted, discarding {dropped} unmerged commit(s).",
                child.id, child.goal, child.branch
            );
        }
        format!(
            "Archived subagent {} ({:?}): its worktree and branch {} are deleted; it had no unmerged commits.",
            child.id, child.goal, child.branch
        )
    }

    fn merge_child_lock(&self, parent_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.merge_child_locks
                .lock()
                .entry(parent_id.to_owned())
                .or_default(),
        )
    }

    /// The merge_child tool: merges the child's branch `--no-ff` into the
    /// parent worktree now and archives it. The parent is mid-turn, so it
    /// commits first and resolves conflicts itself.
    pub async fn merge_child_from_chat(&self, parent_id: &str, child_id: &str) -> String {
        let not_live = || format!("merge_child: {child_id} is not a live child of this workspace");
        let running =
            || format!("merge_child: {child_id} is still running; try again when it settles");
        if self
            .chat_child(parent_id, child_id)
            .filter(|c| !c.archived)
            .is_none()
        {
            return not_live();
        }
        if !self.child_settled_for_chat(child_id) {
            return running();
        }
        let lock = self.merge_child_lock(parent_id);
        let _held = lock.lock().await;
        let Some(child) = self.chat_child(parent_id, child_id).filter(|c| !c.archived) else {
            return not_live();
        };
        if !self.child_settled_for_chat(child_id) {
            return running();
        }
        if child.worktree_path.is_empty() {
            return format!(
                "merge_child: {child_id} has no worktree (it never started); archive_child {child_id} instead"
            );
        }
        let Some(parent) = self.store.workspace(parent_id) else {
            return "merge_child: parent workspace not found".into();
        };
        let Some(proj) = self.store.project(&parent.project_id) else {
            return "merge_child: project not found".into();
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        let timed_out = || "context deadline exceeded".to_owned();
        let git = self.git_manager(&proj.repo_path);
        let label = format!(
            "subagent {} ({:?}, branch {})",
            child.id, child.goal, child.branch
        );
        if until(deadline, git.merge_head(&parent.worktree_path))
            .await
            .unwrap_or(false)
        {
            return format!(
                "merge_child: a merge is in progress in this worktree. Resolve it and git commit (or git merge --abort), then call merge_child {} again.",
                child.id
            );
        }
        let dirty = until(deadline, recur_git::status_map(&parent.worktree_path))
            .await
            .flatten()
            .map_or(0, |m| m.len());
        if dirty > 0 {
            return format!(
                "merge_child: this worktree has {dirty} uncommitted change(s). Commit them first, then call merge_child {} again.",
                child.id
            );
        }
        let (name, email) = match until(deadline, recur_git::identity(&parent.worktree_path)).await
        {
            Some(Ok(id)) => id,
            Some(Err(e)) => return format!("merge_child: {e}"),
            None => return format!("merge_child: {}", timed_out()),
        };
        match until(
            deadline,
            git.commit(
                &child.worktree_path,
                "WIP: uncommitted child work",
                &name,
                &email,
            ),
        )
        .await
        {
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                return format!("merge_child: saving the child's uncommitted work failed: {e}");
            }
            None => {
                return format!(
                    "merge_child: saving the child's uncommitted work failed: {}",
                    timed_out()
                );
            }
        }
        let conflicts = match until(
            deadline,
            git.merge_into(
                &parent.worktree_path,
                &child.branch,
                &format!("Merge subagent: {}", truncate(&child.goal, 60)),
                &name,
                &email,
            ),
        )
        .await
        {
            Some(Ok(c)) => c,
            Some(Err(e)) => {
                return format!("merge_child: merging {label} failed: {e}. Its branch is kept.");
            }
            None => {
                return format!(
                    "merge_child: merging {label} failed: {}. Its branch is kept.",
                    timed_out()
                );
            }
        };
        if !conflicts.is_empty() {
            self.set_chat_child_outcome(
                parent_id,
                child_id,
                &format!("conflict: {}", conflicts.join(", ")),
                "",
            );
            return format!(
                "Merged {label} with conflicts in: {}. Resolve the markers, stage the files, run `git commit --no-edit`, then call merge_child {} again to archive it.",
                conflicts.join(", "),
                child.id
            );
        }
        self.set_chat_child_outcome(parent_id, child_id, CHILD_OUTCOME_MERGED, "");
        self.finish_child(&child.id).await;
        format!("Merged {label} into this worktree and archived it.")
    }
}

fn turn_key(t: &Arc<ChatTurn>) -> usize {
    Arc::as_ptr(t) as usize
}
