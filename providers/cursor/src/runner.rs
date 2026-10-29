//! One Cursor turn over the bridge (Go `engine/cursor.go` `CursorRunner`).
//!
//! The mapping: worktree → `local.cwd`; model → `model.id` (+ flags as
//! params); the MCP URL → `mcpServers["recur"]` inline; Playwright →
//! `mcpServers["playwright"]` over stdio; tool_call frames → Step/ToolResult
//! with MCP tools relabelled at the display boundary.
//!
//! Two requests cannot be honoured, by Cursor's backend: no system prompt
//! (role instructions are prepended to the first user message of an agent),
//! and no tool allowlist (read-only roles run in Cursor's plan mode).
//!
//! Steering (D49) prefers SteerRun: `complete_delivered` keeps the current
//! Send. Otherwise CancelRun plus one follow-up Send of the joined steers on
//! the same agent, still inside this turn, so the engine never sees a failed
//! specialist turn.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::attach::{images_in_marker_order, marker, ordinal_word};
use recur_agent::turn::follow_up_text;
use recur_agent::{
    AgentEvent, EventSink, McpAttach, McpServerStatus, SteerInbox, TurnRequest, TurnResult,
    standing_in_user_prompt,
};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::bridge::Bridge;
use crate::client::{
    AgentOptions, Image, LocalOptions, McpServer, ModelParameterValue, ModelSelection,
    SETTING_SOURCES_MINIMAL, STEER_COMPLETE_DELIVERED, SandboxOptions, run_created_on_this_bridge,
};
use crate::connect::{Client, Error};
use crate::steer::{SteerBuf, ack_all, blank, follow_up_users};
use crate::turn::{Turn, TurnSpec};

/// Bounds the best-effort CloseAgent on the way out.
const CLOSE_AGENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Bounds the best-effort CancelRun after an interrupt.
const CANCEL_RUN_TIMEOUT: Duration = Duration::from_secs(10);

/// How long ObserveRun may go without progress frames (a non-empty offset)
/// before recovery falls back to cancel + Send. Keepalives do not count. A
/// live tool call often sends "running" and then nothing until it completes,
/// so this must exceed realistic tool wall time.
const OBSERVE_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub(crate) const MCP_CREATE_WORKSPACE: &str = "mcp__recur__create_workspace";

/// Blocks Cursor's native subagents (it has no per-run tool allowlist).
/// Parent Agent chats are pointed at create_workspace.
pub(crate) const CURSOR_SUBAGENT_PROMPT: &str = "Cursor's built-in subagents are disabled by policy in Recur. Never spawn a Cursor subagent or background agent. For anything beyond a very simple task, call mcp__recur__create_workspace: that is the Recur subagent; you are re-engaged with its report when it finishes. Redirect a running child, or resume a finished one, with mcp__recur__message_child.";

/// Nested workspaces execute here.
pub(crate) const CURSOR_CHILD_SUBAGENT_PROMPT: &str = "Cursor's built-in subagents are disabled by policy in Recur. Never spawn a Cursor subagent or background agent. Execute the assigned task here.";

pub(crate) type Hook =
    Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

/// A failed turn: the error and whatever result it produced.
#[derive(Debug)]
pub(crate) struct RunFailure {
    pub err: Error,
    pub partial: TurnResult,
}

impl RunFailure {
    fn new(err: Error) -> Self {
        Self {
            err,
            partial: TurnResult::default(),
        }
    }
}

/// The runner for one pooled bridge. Shared by every agent on its worktree.
pub(crate) struct CursorRunner {
    pub(crate) bridge: Arc<Bridge>,
    /// Sent per call so a settings change applies without a bridge restart.
    api_key: String,
    /// Suppresses ambient Cursor config (~/.cursor and the repo's own
    /// .cursor/rules + AGENTS.md). Off by default: a repo's rules are
    /// project context, as Claude lets CLAUDE.md load.
    pub(crate) strict_config: AtomicBool,
    /// The bridge lacks `run.observe`: recovery goes straight to cancel+Send.
    pub(crate) skip_observe: bool,
    /// SteerRun is unimplemented: later steers go straight to cancel+Send.
    steer_unavailable: AtomicBool,
    /// Outstanding pool handouts.
    pub(crate) in_use: AtomicI32,
    pub(crate) observe_idle_timeout: Duration,
    /// Runs after the follow-up Send is armed, before its take (tests).
    before_follow_up_take: Option<Hook>,
    /// Runs once the listener owns a batch for SteerRun (tests).
    after_steer_reserve: Option<Hook>,
}

impl CursorRunner {
    pub(crate) fn new(bridge: Arc<Bridge>, api_key: &str) -> Self {
        Self {
            bridge,
            api_key: api_key.to_owned(),
            strict_config: AtomicBool::new(false),
            skip_observe: false,
            steer_unavailable: AtomicBool::new(false),
            in_use: AtomicI32::new(0),
            observe_idle_timeout: OBSERVE_IDLE_TIMEOUT,
            before_follow_up_take: None,
            after_steer_reserve: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_hooks(
        mut self,
        before_follow_up_take: Option<Hook>,
        after_steer_reserve: Option<Hook>,
    ) -> Self {
        self.before_follow_up_take = before_follow_up_take;
        self.after_steer_reserve = after_steer_reserve;
        self
    }

    /// One agent turn. Events go to `sink` in order; `steer` is taken only
    /// once the agent exists, so a pre-stream failure leaves it for a retry.
    pub(crate) async fn run(
        self: &Arc<Self>,
        req: &TurnRequest,
        cancel: &CancellationToken,
        sink: Arc<dyn EventSink>,
        steer: &mut Option<SteerInbox>,
    ) -> Result<TurnResult, RunFailure> {
        let Some(cl) = self.bridge.client().cloned() else {
            return Err(RunFailure::new(Error::msg(
                "cursor runner: bridge not started",
            )));
        };
        let opts = self.agent_options(req);

        // Continue the session when asked; a resumed agent can be gone
        // (bridge restarted, state pruned), so fall back to a fresh one whose
        // first message must carry the standing notes again.
        let (created, resumed) = if req.resume_session.is_empty() {
            (cl.create_agent(cancel, &opts).await, false)
        } else {
            match cl.resume_agent(cancel, &req.resume_session, &opts).await {
                Ok(id) => (Ok(id), true),
                Err(_) => (cl.create_agent(cancel, &opts).await, false),
            }
        };
        let agent_id =
            created.map_err(|e| RunFailure::new(self.enrich_err(e).wrap("cursor create agent")))?;

        // CancelRun must run before CloseAgent so the provider really stops;
        // closing is best-effort and survives the turn future being dropped.
        let mut closer = CloseAgent {
            cl: Some(cl.clone()),
            agent_id: agent_id.clone(),
        };

        // Cursor has no init event: synthesize one from what was configured.
        sink.emit(AgentEvent::Init {
            session_id: agent_id.clone(),
            model: req.model.clone(),
            mcp_servers: mcp_status_for(req),
        });

        let st = Arc::new(Turn::new(
            TurnSpec {
                model: req.model.clone(),
                flags: req.flags.clone(),
                include_partials: req.include_partials,
            },
            Some(sink),
            &agent_id,
            true,
        ));

        let result = self
            .drive(req, cancel, &cl, &agent_id, &st, resumed, steer.take())
            .await;
        closer.close().await;
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn drive(
        self: &Arc<Self>,
        req: &TurnRequest,
        cancel: &CancellationToken,
        cl: &Arc<Client>,
        agent_id: &str,
        st: &Arc<Turn>,
        resumed: bool,
        inbox: Option<SteerInbox>,
    ) -> Result<TurnResult, RunFailure> {
        // Aborting the stream only disconnects Recur; CancelRun stops the
        // provider-side agent and its Task sub-agents. Steer cancels must not
        // share this once: a later Stop still has to cancel the follow-up.
        let stop = Arc::new(StopCancel {
            once: OnceCell::new(),
            cl: cl.clone(),
            st: st.clone(),
        });
        let stop_watch = CancellationToken::new();
        let _stop_watch_guard = stop_watch.clone().drop_guard();
        let watcher = {
            let (stop, stop_watch, cancel) = (stop.clone(), stop_watch.clone(), cancel.clone());
            tokio::spawn(async move {
                tokio::select! {
                    _ = cancel.cancelled() => stop.run().await,
                    _ = stop_watch.cancelled() => {}
                }
            })
        };

        let send = Arc::new(SendCtl::default());
        let steers = Arc::new(SteerBuf::default());
        let steer_ctx = cancel.child_token();
        let _steer_guard = steer_ctx.clone().drop_guard();
        let listener = inbox.map(|inbox| {
            let this = self.clone();
            let (st, steers, send, steer_ctx, cl) = (
                st.clone(),
                steers.clone(),
                send.clone(),
                steer_ctx.clone(),
                cl.clone(),
            );
            tokio::spawn(async move { this.listen(inbox, st, steers, send, steer_ctx, cl).await })
        });

        let result = self
            .main_loop(
                req, cancel, cl, agent_id, st, resumed, &stop, &send, &steers,
            )
            .await;

        // Go's defers, in order: stop the listener, nack what is left, stop
        // the cancel watcher.
        steer_ctx.cancel();
        if let Some(l) = listener {
            let _ = l.await;
        }
        ack_all(steers.take(), false);
        stop_watch.cancel();
        let _ = watcher.await;
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn main_loop(
        &self,
        req: &TurnRequest,
        cancel: &CancellationToken,
        cl: &Arc<Client>,
        agent_id: &str,
        st: &Arc<Turn>,
        resumed: bool,
        stop: &StopCancel,
        send: &SendCtl,
        steers: &SteerBuf,
    ) -> Result<TurnResult, RunFailure> {
        let mut prompt = self.wire_prompt(req, standing_in_user_prompt(req, resumed));
        let mut images = cursor_images(req);
        let mut follow_up = false;
        loop {
            let send_tok = cancel.child_token();
            send.arm(send_tok.clone());
            // Steers that landed during the last follow-up preparation belong
            // on this Send. The take acks the batch so the listener skips its
            // CancelRun of this new run; a later steer still cancels.
            if follow_up {
                if let Some(h) = &self.before_follow_up_take {
                    h().await;
                }
                let pending = steers.take();
                let extra = follow_up_text(&pending);
                if extra.is_empty() {
                    ack_all(pending, false);
                } else {
                    let users = follow_up_users(&pending, &extra, false);
                    ack_all(pending, true);
                    st.fire_users(users, false);
                    prompt = format!("{prompt}\n\n{extra}");
                }
            }

            let send_err = self
                .send_once(&send_tok, cl, agent_id, &prompt, &images, st)
                .await
                .err();

            if cancel.is_cancelled() {
                send_tok.cancel();
                stop.run().await;
                st.finish();
                return Err(RunFailure {
                    err: Error::canceled(),
                    partial: st.result(),
                });
            }

            if st.have_result() {
                let _ = tokio::time::timeout(Duration::from_secs(2), steers.wait_reserved(cancel))
                    .await;
                steers.abandon();
            } else {
                steers.wait_reserved(cancel).await;
            }
            let pending = steers.take();
            let joined = follow_up_text(&pending);
            if !joined.is_empty() && !st.have_result() {
                let users = follow_up_users(&pending, &joined, false);
                ack_all(pending, true);
                send_tok.cancel();
                st.prepare_steer_follow_up();
                st.fire_users(users, false);
                prompt = joined;
                images.clear();
                follow_up = true;
                continue;
            }
            ack_all(pending, false);
            // A late steer after FINISHED cancels the stream tail, but the
            // turn already completed: not a failed run.
            let late_steer_cancel = st.have_result() && send_tok.is_cancelled();
            send_tok.cancel();
            st.finish();

            if let Some(e) = send_err
                && !late_steer_cancel
            {
                let unauth = e.connect().is_some_and(|c| c.unauthenticated());
                let e = self.enrich_err(e);
                let err = if unauth {
                    e.wrap("cursor auth failed (reconnect Cursor in Settings)")
                } else {
                    e.wrap("cursor run")
                };
                return Err(RunFailure {
                    err,
                    partial: st.result(),
                });
            }
            if !st.have_result() {
                let tail = self.bridge.stderr_tail();
                if !tail.is_empty() {
                    tracing::warn!("recur: cursor run ended without a result\n{tail}");
                }
                return Err(RunFailure {
                    err: Error::msg(cursor_no_result_message(&tail, &self.bridge_log_hint())),
                    partial: st.result(),
                });
            }
            return Ok(st.result());
        }
    }

    /// The steer listener: SteerRun first; otherwise suppress the CANCELLED
    /// result, CancelRun, and interrupt the Send so the main loop follows up.
    async fn listen(
        self: Arc<Self>,
        mut inbox: SteerInbox,
        st: Arc<Turn>,
        steers: Arc<SteerBuf>,
        send: Arc<SendCtl>,
        steer_ctx: CancellationToken,
        cl: Arc<Client>,
    ) {
        loop {
            let msg = tokio::select! {
                _ = steer_ctx.cancelled() => return,
                m = inbox.recv() => m,
            };
            let Some(s) = msg else {
                return;
            };
            if blank(&s) {
                s.ack(false);
                continue;
            }
            steers.add(s);
            drain_inject(&mut inbox, &steers);
            loop {
                drain_inject(&mut inbox, &steers);
                let Some(reserved) = steers.reserve() else {
                    break;
                };
                if let Some(h) = &self.after_steer_reserve {
                    h().await;
                }
                if !reserved.joined.is_empty()
                    && self.try_steer(&steer_ctx, &st, &reserved.joined).await
                {
                    // Users before commit, so wait_reserved keeps the main
                    // loop off finish until the live-steer rows land.
                    st.fire_users(reserved.users, true);
                    steers.commit();
                    continue;
                }
                steers.release();
                let batch = steers.batch_seq();
                // Suppress a CANCELLED result so turn_end does not fire; a
                // FINISHED one still wins (late steer).
                st.set_suppress_result(true);
                let generation = send.generation();
                if !steers.consumed(batch) && st.all_run_ids().is_empty() {
                    st.wait_run_id(&steer_ctx, Duration::from_secs(2)).await;
                }
                // The main loop folded this batch into a follow-up prompt:
                // cancelling now would kill a Send that already carries it.
                if steers.consumed(batch) {
                    continue;
                }
                // Same RPC as the Stop path, not its once: a later Stop must
                // still cancel a follow-up run.
                cancel_runs(&cl, Some(&steer_ctx), st.all_run_ids()).await;
                drain_inject(&mut inbox, &steers);
                send.interrupt(generation);
                break;
            }
        }
    }

    async fn try_steer(&self, ctx: &CancellationToken, st: &Turn, text: &str) -> bool {
        if text.is_empty() || self.steer_unavailable.load(Ordering::SeqCst) {
            return false;
        }
        let Some(cl) = self.bridge.client() else {
            return false;
        };
        let mut id = st.parent_run();
        if id.is_empty() {
            id = st.wait_run_id(ctx, Duration::from_secs(2)).await;
        }
        if id.is_empty() {
            return false;
        }
        match cl.steer_run(ctx, &id, text).await {
            Ok(outcome) => outcome == STEER_COMPLETE_DELIVERED,
            Err(e) => {
                if e.connect().is_some_and(|c| c.unimplemented()) {
                    self.steer_unavailable.store(true, Ordering::SeqCst);
                }
                false
            }
        }
    }

    /// One Send. A leftover activeRunId (dropped stream) is drained or
    /// cancelled, then this prompt is retried.
    async fn send_once(
        &self,
        tok: &CancellationToken,
        cl: &Client,
        agent_id: &str,
        prompt: &str,
        images: &[Image],
        st: &Arc<Turn>,
    ) -> Result<(), Error> {
        let res = cl
            .send(tok, agent_id, prompt, images, |ev| st.on_event(ev))
            .await;
        match res {
            Err(e) if e.active_run_conflict() && !tok.is_cancelled() => {
                self.recover_active_run(tok, cl, agent_id, prompt, images, st)
                    .await
            }
            other => other,
        }
    }

    /// Drains or clears a leftover run, then Sends this turn's prompt.
    /// Observed frames are not folded into the turn: that would persist the
    /// previous run as this turn's answer.
    async fn recover_active_run(
        &self,
        ctx: &CancellationToken,
        cl: &Client,
        agent_id: &str,
        prompt: &str,
        images: &[Image],
        st: &Arc<Turn>,
    ) -> Result<(), Error> {
        let detached = CancellationToken::new();
        let listed = tokio::time::timeout(
            CANCEL_RUN_TIMEOUT,
            cl.first_non_terminal_run(&detached, agent_id),
        )
        .await
        .unwrap_or_else(|_| Err(Error::msg("context deadline exceeded")));
        match listed {
            Err(e) => {
                tracing::warn!(
                    "recur: cursor list runs on {agent_id} after active-run conflict: {e}"
                );
            }
            Ok(run)
                if !run.run_id.is_empty()
                    && !self.skip_observe
                    && run_created_on_this_bridge(&run, self.bridge.started_at()) =>
            {
                tracing::info!(
                    "recur: cursor draining active run {} on {agent_id} before Send",
                    run.run_id
                );
                st.note_run_id(&run.run_id);
                st.fire(AgentEvent::Notice {
                    text: format!("Waiting for previous run {} to finish.", run.run_id),
                });
                match self.observe_run_to_idle(ctx, cl, st, &run.run_id).await {
                    Ok(()) => {
                        st.clear_steer_target();
                        let res = cl
                            .send(ctx, agent_id, prompt, images, |ev| st.on_event(ev))
                            .await;
                        // Observe EOF is not proof the store cleared activeRunId.
                        match res {
                            Err(e) if e.active_run_conflict() && !ctx.is_cancelled() => {
                                tracing::warn!(
                                    "recur: cursor send after observe still conflicted on {agent_id}; cancelling"
                                );
                            }
                            other => return other,
                        }
                    }
                    Err(e) if ctx.is_cancelled() => return Err(e),
                    Err(e) => {
                        tracing::warn!(
                            "recur: cursor observe run {} on {agent_id} failed ({e}); cancelling and retrying Send",
                            run.run_id
                        );
                    }
                }
                st.fire(AgentEvent::Notice {
                    text: "Previous run did not finish; cancelling and retrying.".into(),
                });
            }
            Ok(run) if !run.run_id.is_empty() => {
                if self.skip_observe {
                    tracing::info!(
                        "recur: cursor skipping observe of run {} on {agent_id} (bridge lacks run.observe)",
                        run.run_id
                    );
                } else {
                    tracing::info!(
                        "recur: cursor skipping observe of run {} on {agent_id} (not created on this bridge)",
                        run.run_id
                    );
                }
            }
            Ok(_) => {}
        }

        let detached = CancellationToken::new();
        match tokio::time::timeout(
            CANCEL_RUN_TIMEOUT,
            cl.cancel_non_terminal_runs(&detached, agent_id),
        )
        .await
        {
            Ok(Err(e)) => tracing::warn!("recur: cursor clear stale runs on {agent_id}: {e}"),
            Err(_) => tracing::warn!(
                "recur: cursor clear stale runs on {agent_id}: context deadline exceeded"
            ),
            Ok(Ok(n)) if n > 0 => {
                tracing::info!(
                    "recur: cursor cleared {n} stale run(s) on {agent_id} after active-run conflict"
                );
            }
            Ok(Ok(_)) => {}
        }
        st.clear_steer_target();
        cl.send(ctx, agent_id, prompt, images, |ev| st.on_event(ev))
            .await
    }

    /// Waits for an existing run to finish without emitting into this turn.
    /// The idle deadline resets only on frames with an offset, so keepalives
    /// cannot disarm it.
    async fn observe_run_to_idle(
        &self,
        ctx: &CancellationToken,
        cl: &Client,
        st: &Turn,
        run_id: &str,
    ) -> Result<(), Error> {
        let obs = ctx.child_token();
        let _obs_guard = obs.clone().drop_guard();
        let last_progress = Arc::new(Mutex::new(tokio::time::Instant::now()));
        let idle = self.observe_idle_timeout;
        let timer = {
            let (obs, last) = (obs.clone(), last_progress.clone());
            tokio::spawn(async move {
                let mut wait = idle;
                loop {
                    tokio::select! {
                        _ = obs.cancelled() => return,
                        _ = tokio::time::sleep(wait) => {
                            let since = last.lock().elapsed();
                            if since >= idle {
                                obs.cancel();
                                return;
                            }
                            wait = idle - since;
                        }
                    }
                }
            })
        };
        let res = cl
            .observe_run(&obs, run_id, "", |ev| {
                if !ev.offset.is_empty() {
                    *last_progress.lock() = tokio::time::Instant::now();
                }
                if let Some(r) = &ev.result {
                    st.note_run_id(&r.run_id);
                    if let Some(rr) = &r.result {
                        st.note_run_id(&rr.run_id);
                    }
                }
                if let Some(m) = &ev.sdk_message {
                    st.note_run_id(&m.run_id);
                }
            })
            .await;
        let idled = obs.is_cancelled() && !ctx.is_cancelled();
        obs.cancel();
        let _ = timer.await;
        match res {
            Err(e) if idled => Err(e.wrap("observe idle deadline exceeded")),
            other => other,
        }
    }

    /// The sdk.v1 AgentOptions for a turn.
    pub(crate) fn agent_options(&self, req: &TurnRequest) -> AgentOptions {
        let mut local = LocalOptions {
            sandbox: Some(SandboxOptions { enabled: false }),
            ..Default::default()
        };
        if !req.worktree.as_os_str().is_empty() {
            local.cwd = vec![req.worktree.to_string_lossy().into_owned()];
        }
        if self.strict_config.load(Ordering::SeqCst) {
            // Empty would mean "SDK defaults": naming the narrowest layer is
            // the only way to suppress project and user config.
            local.setting_sources = SETTING_SOURCES_MINIMAL
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
        }
        // Native skills are stubbed by the install patch, so setting sources
        // can still load AGENTS.md and .cursor/rules.
        let mut servers = std::collections::BTreeMap::new();
        if !req.mcp_url.is_empty() {
            servers.insert("recur".to_owned(), McpServer::http(&req.mcp_url));
        }
        if req.playwright {
            servers.insert(
                "playwright".to_owned(),
                McpServer::stdio("npx", &["-y", "@playwright/mcp@latest"]),
            );
        }
        for u in &req.user_mcp {
            let name = u.name.trim();
            if !name.is_empty() {
                servers.insert(name.to_owned(), cursor_user_mcp(u));
            }
        }
        AgentOptions {
            model: (!req.model.is_empty()).then(|| ModelSelection {
                id: req.model.clone(),
                params: params_from_flags(&req.flags),
            }),
            api_key: self.api_key.clone(),
            name: req.agent_id.clone(),
            local: Some(local),
            mcp_servers: servers,
            // The closest Cursor has to a per-role tool allowlist.
            mode: if is_read_only_role(req) {
                "plan".into()
            } else {
                String::new()
            },
        }
    }

    /// The prompt as callers that do not know whether ResumeAgent succeeds
    /// see it: a resume handle reads as "will resume".
    #[cfg(test)]
    pub(crate) fn prompt(&self, req: &TurnRequest) -> String {
        let resumed = !req.resume_session.is_empty() && !req.refresh_standing;
        self.wire_prompt(req, standing_in_user_prompt(req, resumed))
    }

    /// The user message. Cursor has no system prompt, so role instructions
    /// ride in-band on a fresh (or remounted) session, fenced as standing
    /// instructions; an ordinary follow-up already has them in history.
    pub(crate) fn wire_prompt(&self, req: &TurnRequest, include_standing: bool) -> String {
        let body = wrap_attached_images_for_cursor(&req.prompt, &req.images);
        let body = join_append_prompt(&body, &req.turn_append_prompt);
        if !include_standing {
            return body;
        }
        let mut role = req.system_prompt.trim().to_owned();
        for add in [
            req.append_system_prompt.trim(),
            cursor_subagent_overlay(req),
        ] {
            if add.is_empty() {
                continue;
            }
            if !role.is_empty() {
                role.push_str("\n\n");
            }
            role.push_str(add);
        }
        if role.is_empty() {
            return body;
        }
        format!("<role-instructions>\n{role}\n</role-instructions>\n\n{body}")
    }

    fn enrich_err(&self, err: Error) -> Error {
        if let Some(c) = err.connect()
            && c.transient()
        {
            let tail = self.bridge.stderr_tail();
            if !tail.is_empty() {
                tracing::warn!("recur: cursor bridge {err}\n{tail}");
            } else if !c.raw.is_empty() {
                tracing::warn!("recur: cursor bridge {err}\n{}", c.raw);
            }
        }
        match self.bridge.log_path() {
            Some(p) => err.suffix(format!(" (bridge log: {})", p.display())),
            None => err,
        }
    }

    fn bridge_log_hint(&self) -> String {
        self.bridge
            .log_path()
            .map(|p| format!(" (bridge log: {})", p.display()))
            .unwrap_or_default()
    }
}

/// The Stop path's CancelRun, once per turn.
struct StopCancel {
    once: OnceCell<()>,
    cl: Arc<Client>,
    st: Arc<Turn>,
}

impl StopCancel {
    async fn run(&self) {
        self.once
            .get_or_init(|| async {
                if self.st.all_run_ids().is_empty() {
                    self.st
                        .wait_run_id(&CancellationToken::new(), Duration::from_secs(2))
                        .await;
                }
                cancel_runs(&self.cl, None, self.st.all_run_ids()).await;
            })
            .await;
    }
}

/// The in-flight Send's cancel token and generation.
#[derive(Default)]
struct SendCtl {
    inner: Mutex<(u64, Option<CancellationToken>)>,
}

impl SendCtl {
    fn arm(&self, tok: CancellationToken) {
        let mut g = self.inner.lock();
        g.0 += 1;
        g.1 = Some(tok);
    }

    fn generation(&self) -> u64 {
        self.inner.lock().0
    }

    /// Cancels only the Send that was in flight at `generation`: CancelRun
    /// can end that Send before this runs, and the follow-up must survive.
    fn interrupt(&self, generation: u64) {
        let g = self.inner.lock();
        if g.0 == generation
            && let Some(t) = &g.1
        {
            t.cancel();
        }
    }
}

/// Best-effort CancelRun of every id, bounded together.
async fn cancel_runs(cl: &Client, parent: Option<&CancellationToken>, ids: Vec<String>) {
    if ids.is_empty() {
        return;
    }
    let tok = parent.map_or_else(CancellationToken::new, CancellationToken::child_token);
    let _ = tokio::time::timeout(CANCEL_RUN_TIMEOUT, async {
        for id in ids {
            let _ = cl.cancel_run(&tok, &id).await;
        }
    })
    .await;
}

/// CloseAgent on the way out, even when the turn future is dropped.
struct CloseAgent {
    cl: Option<Arc<Client>>,
    agent_id: String,
}

impl CloseAgent {
    async fn close(&mut self) {
        if let Some(cl) = self.cl.take() {
            close_agent(cl, std::mem::take(&mut self.agent_id)).await;
        }
    }
}

impl Drop for CloseAgent {
    fn drop(&mut self) {
        if let Some(cl) = self.cl.take()
            && let Ok(h) = tokio::runtime::Handle::try_current()
        {
            h.spawn(close_agent(cl, std::mem::take(&mut self.agent_id)));
        }
    }
}

async fn close_agent(cl: Arc<Client>, agent_id: String) {
    let tok = CancellationToken::new();
    let _ = tokio::time::timeout(CLOSE_AGENT_TIMEOUT, cl.close_agent(&tok, &agent_id)).await;
}

fn drain_inject(inbox: &mut SteerInbox, steers: &SteerBuf) {
    while let Some(s) = inbox.try_next() {
        if blank(&s) {
            s.ack(false);
        } else {
            steers.add(s);
        }
    }
}

/// The run-ended-without-a-result error: one line plus the log path; the
/// stderr ring goes to Recur's log at the call site.
pub(crate) fn cursor_no_result_message(tail: &str, log_hint: &str) -> String {
    let mut msg = "cursor run ended without a result".to_owned();
    if bridge_tail_looks_like_shell_spawn_crash(tail) {
        msg.push_str(": bridge crashed on Shell spawn ENOENT (often a missing working_directory such as ~/.cursor/projects/…/terminals); retry the turn");
    } else if tail.contains("SIGTERM") {
        msg.push_str(": the Cursor bridge stopped mid-turn; retry");
    }
    msg + log_hint
}

/// Node failing to spawn the shell (usually a missing working directory).
/// Both tokens must share a line, so an older non-fatal `spawn npx ENOENT`
/// cannot misattribute a later result-less turn.
pub(crate) fn bridge_tail_looks_like_shell_spawn_crash(tail: &str) -> bool {
    tail.split('\n')
        .any(|l| l.contains("spawn /bin/") && l.contains("ENOENT"))
}

fn cursor_subagent_overlay(req: &TurnRequest) -> &'static str {
    if req.chat_child {
        return CURSOR_CHILD_SUBAGENT_PROMPT;
    }
    if req
        .allowed_tools
        .as_deref()
        .is_some_and(|t| t.iter().any(|t| t == MCP_CREATE_WORKSPACE))
    {
        return CURSOR_SUBAGENT_PROMPT;
    }
    ""
}

/// Images in marker order, as SDKUserMessage images.
pub(crate) fn cursor_images(req: &TurnRequest) -> Vec<Image> {
    images_in_marker_order(&req.prompt, &req.images)
        .into_iter()
        .map(|i| Image {
            data: i.data,
            mime_type: i.mime,
        })
        .collect()
}

/// Settings flags as ModelSelection params, sorted by id (BTreeMap order);
/// empty keys dropped.
fn params_from_flags(flags: &recur_agent::Flags) -> Vec<ModelParameterValue> {
    flags
        .iter()
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, v)| ModelParameterValue {
            id: k.clone(),
            value: v.clone(),
        })
        .collect()
}

/// A role whose allowlist has no file-edit tool runs in plan mode. Bare Bash
/// (lint/test) does not count: the Reviewer must stay in plan mode. No
/// allowlist means unscoped, not read-only.
pub(crate) fn is_read_only_role(req: &TurnRequest) -> bool {
    match req.allowed_tools.as_deref() {
        None | Some([]) => false,
        Some(tools) => !tools
            .iter()
            .any(|t| matches!(t.as_str(), "Edit" | "Write" | "NotebookEdit")),
    }
}

/// The MCP servers wired for a turn, reported as configured: the bridge has
/// no per-server status, and a broken MCP shows up as a failing call.
pub(crate) fn mcp_status_for(req: &TurnRequest) -> Vec<McpServerStatus> {
    let connected = |name: &str| McpServerStatus {
        name: name.to_owned(),
        status: "connected".into(),
    };
    let mut out = Vec::new();
    if !req.mcp_url.is_empty() {
        out.push(connected("recur"));
    }
    if req.playwright {
        out.push(connected("playwright"));
    }
    for u in &req.user_mcp {
        let name = u.name.trim();
        if !name.is_empty() {
            out.push(connected(name));
        }
    }
    out
}

fn cursor_user_mcp(u: &McpAttach) -> McpServer {
    if !u.url.is_empty() {
        let mut s = McpServer::http(&u.url);
        if let Some(h) = s.http.as_mut() {
            h.headers = u.headers.clone();
        }
        return s;
    }
    let args: Vec<&str> = u.args.iter().map(String::as_str).collect();
    let mut s = McpServer::stdio(&u.command, &args);
    if let Some(st) = s.stdio.as_mut() {
        st.env = u.env.clone();
    }
    s
}

/// A system instruction mapping each `⟦attachment:id⟧` to its ordinal in
/// SDKUserMessage.images (marker order). Markers stay in the body for
/// position; the instruction says they are not files. No-op without images.
pub fn wrap_attached_images_for_cursor(text: &str, imgs: &[recur_agent::UserImage]) -> String {
    if imgs.is_empty() {
        return text.to_owned();
    }
    let ordered = images_in_marker_order(text, imgs);
    if ordered.is_empty() {
        return text.to_owned();
    }
    let mut b = String::from("<system_instruction>\n");
    b.push_str("The user attached image(s) as a multimodal message (SDKUserMessage.images).\n");
    b.push_str("Markers like ⟦attachment:id⟧ in the text mark where each image sits; they are NOT files to open or search for.\n");
    b.push_str("The images payload is ordered to match those markers:\n");
    for (i, img) in ordered.iter().enumerate() {
        let n = i + 1;
        b.push_str(&format!(
            "- {} is image {n} ({} attached image)\n",
            marker(&img.id),
            ordinal_word(n)
        ));
    }
    b.push_str("</system_instruction>\n\n");
    b.push_str(text);
    b
}

/// Go `joinAppendPrompt`: trimmed parts joined by a blank line.
fn join_append_prompt(base: &str, extra: &str) -> String {
    let (base, extra) = (base.trim(), extra.trim());
    match (base.is_empty(), extra.is_empty()) {
        (true, _) => extra.to_owned(),
        (_, true) => base.to_owned(),
        _ => format!("{base}\n\n{extra}"),
    }
}

#[cfg(test)]
mod tests;
