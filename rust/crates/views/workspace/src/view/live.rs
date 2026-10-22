//! Backend events into the per-tab transcripts (`onSession*`, `onAgent`), the tab strip
//! (`onWorkspaceChatTab`, `onAttentionTab`, `onChatsChanged`), escalations and findings; the
//! transcript fetch and resync; and the child entities' events.

use gpui_kit::{App, Context, Entity, Window};
use recur_engine::view::chat::{
    AgentMsg, StreamKind, StreamOpts, add_tool, add_user, append_stream, apply_compact,
    apply_fallback, apply_notice, apply_stream, attach_tool_result, current_turn_has_ended,
    drop_live_after_footer, drop_open_compact, end_turn, format_ask_user_display_text,
    map_chat_items, merge_persisted_with_live, note_thinking_tokens, patch_tool_input,
    retain_optimistic_users, should_refresh_turn_stamp, should_skip_idle_transcript_reset,
};
use recur_engine::view::loop_cockpit::is_pair_chat_code;
use recur_services::AppEvent;
use recur_view_chat::chat::{AgentChat, ChatEvent};
use recur_view_chat::hoist::{
    CodeRemap, HoistState, absorb_hoist_sending, concat_hoist_list, concat_hoist_text,
    dest_chat_key, hoist_cache_bust_codes, hoist_send_complete_applies,
};
use recur_view_chat::session_events::{SessionSignal, StreamEvt, session_event};
use recur_view_core::{AppState, AppliedEvent, ShellEvent, is_agent_running_status};
use recur_view_observatory::{Observatory, ObservatoryEvent};

use super::{WorkspaceView, now_ms};
use crate::plan_panel::{PlanPanel, PlanPanelEvent};

impl WorkspaceView {
    /// `acceptsChatCode`: events for this workspace's MA and pair tabs.
    fn accepts(&self, id: &str, code: &str, cx: &App) -> bool {
        id == self.ws_id
            && self.state.read(cx).workspace(id).is_some()
            && (code == "MA" || is_pair_chat_code(code))
    }

    fn live_turn_closed(&self, key: &str, cx: &App) -> bool {
        drop_live_after_footer(
            self.chat_by_key.get(key).map(Vec::as_slice).unwrap_or_default(),
            self.turn_started_at_by_key.get(key).copied().unwrap_or(0),
            self.chat_key_busy(key, cx),
        )
    }

    fn rows(&self, key: &str) -> Vec<AgentMsg> {
        self.chat_by_key.get(key).cloned().unwrap_or_default()
    }

    fn apply_live_stream(&mut self, key: &str, kind: StreamKind, evt: &StreamEvt, cx: &App) {
        if self.live_turn_closed(key, cx) {
            return;
        }
        let tag = if kind == StreamKind::Text {
            "text"
        } else {
            "thinking"
        };
        let seq_key = if evt.parent_id.is_empty() {
            format!("{key}:{tag}")
        } else {
            format!("{key}:{tag}:{}", evt.parent_id)
        };
        if evt.seq > 0 {
            let prev = self.stream_seq_by_key.get(&seq_key).copied().unwrap_or(0);
            if evt.seq <= prev {
                return;
            }
            self.stream_seq_by_key.insert(seq_key, evt.seq);
        }
        if kind == StreamKind::Text && !evt.text.is_empty() && evt.parent_id.is_empty() {
            self.turn_got_text_by_key.insert(key.to_string(), true);
        }
        let rows = self.rows(key);
        let next = if !evt.parent_id.is_empty() || evt.replace || evt.done {
            apply_stream(
                &rows,
                kind,
                &evt.text,
                &StreamOpts {
                    done: evt.done,
                    parent_id: evt.parent_id.clone(),
                    uuid: evt.uuid.clone(),
                },
            )
        } else if !evt.text.is_empty() {
            append_stream(&rows, kind, &evt.text)
        } else {
            return;
        };
        self.chat_by_key.insert(key.to_string(), next);
    }

    /// `applyTranscript`: the store's transcript, merged with the live tail only while the
    /// engine runs; queued bubbles stay at the end.
    pub(crate) fn apply_transcript(
        &mut self,
        key: &str,
        items: &[recur_domain::ChatItem],
        cx: &mut Context<Self>,
    ) {
        let mapped = map_chat_items(items);
        let live = self.rows(key);
        let merged = if self.chat_key_busy(key, cx) {
            merge_persisted_with_live(&mapped, &live)
        } else {
            retain_optimistic_users(&mapped, &live)
        };
        let pending = self.pending_bubbles(key);
        let list = if pending.is_empty() {
            merged
        } else {
            let mut l: Vec<AgentMsg> = merged.into_iter().filter(|m| !m.queued).collect();
            l.extend(pending);
            l
        };
        self.chat_by_key.insert(key.to_string(), list);
        if let Some(u) = recur_agent::context_usage::from_chat_items(items) {
            self.usage_by_key.insert(key.to_string(), u);
        }
        self.touch(cx);
    }

    /// `WorkspaceChatItems` into a tab, unless a permanent delete bumped its generation.
    pub(crate) fn resync_chat(&mut self, id: &str, code: &str, cx: &mut Context<Self>) {
        let key = recur_view_chat::hoist::chat_tab_key(id, code);
        let gen_ = self.transcript_gen.get(&key).copied().unwrap_or(0);
        let (svc, id, code) = (self.services.clone(), id.to_string(), code.to_string());
        cx.spawn(async move |this, cx| {
            let Ok(items) = svc.workspaces.workspace_chat_items(&id, &code).await else {
                return;
            };
            let _ = this.update(cx, |v, cx| {
                if v.transcript_gen.get(&key).copied().unwrap_or(0) != gen_ {
                    return;
                }
                v.apply_transcript(&key, &items, cx);
            });
        })
        .detach();
    }

    // ---- the backend ----

    pub(crate) fn on_applied_event(
        &mut self,
        _: Entity<AppState>,
        ev: &AppliedEvent,
        cx: &mut Context<Self>,
    ) {
        let ev = &ev.0;
        if let Some(se) = session_event(ev) {
            self.on_session(se.workspace_id, se.code, se.signal, cx);
            return;
        }
        match ev {
            AppEvent::AgentUpdated(a) => self.on_agent(&a.workspace_id, &a.agent_id, &a.status, cx),
            AppEvent::WorkspaceChatTab(t) => {
                if t.workspace_id != self.ws_id || t.code.is_empty() || t.title.is_empty() {
                    return;
                }
                if let Some(tabs) = self.tabs.as_mut()
                    && !tabs.is_empty()
                {
                    for tab in tabs.iter_mut().filter(|x| x.code == t.code) {
                        tab.title = t.title.clone();
                    }
                    self.put_strip(cx);
                    self.touch(cx);
                }
            }
            AppEvent::AttentionUpdated(a) => {
                self.on_attention_tab(&a.workspace_id, &a.code, a.tab_unseen, a.clear_all_unseen, cx)
            }
            AppEvent::ChatModeChanged(m) => {
                if m.workspace_id.is_empty()
                    || m.agent_code.is_empty()
                    || m.mode.is_empty()
                    || m.workspace_id != self.ws_id
                {
                    return;
                }
                let key = self.key(&m.agent_code);
                let mode = recur_view_core::modes::ChatMode::from_key(&m.mode);
                self.pickers_by_key
                    .entry(key)
                    .and_modify(|p| p.mode = mode)
                    .or_insert(recur_view_chat::heap::ChatPickerSnap {
                        model: String::new(),
                        flags: Default::default(),
                        mode,
                    });
                self.touch(cx);
            }
            AppEvent::EscalationRaised(e) => {
                if e.workspace_id != self.ws_id {
                    return;
                }
                self.esc_load_gen += 1;
                if let Some(open) = take_open_escalation(Some(e.escalation.clone())) {
                    self.live_esc = Some(open);
                }
                let owner = self.conductor_chat_code(cx);
                if !owner.is_empty() {
                    self.select_chat_tab(&owner, cx);
                }
                self.touch(cx);
            }
            AppEvent::EscalationResolved(e) => {
                if e.workspace_id != self.ws_id {
                    return;
                }
                self.load_escalation(cx);
            }
            AppEvent::FindingsUpdated(w) => {
                if w.workspace_id == self.ws_id {
                    self.load_findings(cx);
                }
            }
            AppEvent::WorkspaceUpdated(u) if u.chats_changed && !u.workspace_id.is_empty() => {
                let from = u.from_workspace_id.clone().filter(|f| !f.is_empty());
                let remap: Option<CodeRemap> = from.as_ref().map(|_| u.code_remap.clone().unwrap_or_default());
                let moved = u.moved_codes.clone().unwrap_or_default();
                self.on_chats_changed(&u.workspace_id, from.as_deref(), remap, moved, cx);
            }
            _ => {}
        }
    }

    fn on_session(&mut self, id: String, code: String, signal: SessionSignal, cx: &mut Context<Self>) {
        if !self.accepts(&id, &code, cx) {
            return;
        }
        let key = self.key(&code);
        match signal {
            SessionSignal::Delta(evt) => self.apply_live_stream(&key, StreamKind::Text, &evt, cx),
            SessionSignal::Thinking(evt) => {
                if self.live_turn_closed(&key, cx) {
                    return;
                }
                if !evt.text.is_empty() || evt.replace || evt.done {
                    self.apply_live_stream(&key, StreamKind::Thinking, &evt, cx);
                } else {
                    let rows = self.rows(&key);
                    self.chat_by_key.insert(
                        key,
                        note_thinking_tokens(&rows, evt.tokens, &evt.uuid, &evt.parent_id),
                    );
                }
            }
            SessionSignal::Step(step) => {
                if self.live_turn_closed(&key, cx) {
                    return;
                }
                let rows = self.rows(&key);
                self.chat_by_key.insert(key, add_tool(&rows, &step));
            }
            SessionSignal::ToolResult {
                id: tool_id,
                result,
                is_error,
                extras,
            } => {
                let rows = self.rows(&key);
                self.chat_by_key.insert(
                    key,
                    attach_tool_result(&rows, &tool_id, &result, is_error, &extras.unwrap_or_default()),
                );
            }
            SessionSignal::ToolInput {
                id: tool_id,
                extras,
            } => {
                let rows = self.rows(&key);
                self.chat_by_key
                    .insert(key, patch_tool_input(&rows, &tool_id, &extras));
            }
            SessionSignal::ContextUsage(u) => {
                let compacted = self.usage_by_key.get(&key).is_some_and(|p| p.compacted);
                self.usage_by_key.insert(
                    key,
                    recur_agent::context_usage::ContextUsage {
                        compacting: false,
                        compacted,
                        ..u
                    },
                );
            }
            SessionSignal::Compact(evt) => {
                if self.live_turn_closed(&key, cx) {
                    return;
                }
                let rows = self.rows(&key);
                self.chat_by_key.insert(key.clone(), apply_compact(&rows, &evt));
                if let Some(prev) = self.usage_by_key.get(&key).cloned() {
                    let next = if evt.status == "started" {
                        recur_agent::context_usage::ContextUsage {
                            compacting: true,
                            ..prev
                        }
                    } else {
                        prev.after_compact()
                    };
                    self.usage_by_key.insert(key, next);
                }
            }
            SessionSignal::Notice(text) => {
                if self.live_turn_closed(&key, cx) {
                    return;
                }
                let rows = self.rows(&key);
                self.chat_by_key.insert(key, apply_notice(&rows, &text));
            }
            SessionSignal::Fallback(evt) => {
                if self.live_turn_closed(&key, cx) {
                    return;
                }
                let rows = self.rows(&key);
                self.chat_by_key.insert(key, apply_fallback(&rows, &evt));
            }
            SessionSignal::User(text) => {
                let rows = self.rows(&key);
                self.chat_by_key.insert(key, add_user(&rows, &text, now_ms()));
            }
            SessionSignal::TurnEnd(turn) => {
                let rows = drop_open_compact(&self.rows(&key));
                self.chat_by_key
                    .insert(key.clone(), end_turn(&rows, turn.duration_ms, now_ms()));
                let prev = self.usage_by_key.get(&key).cloned();
                if let Some(u) = turn.usage {
                    self.usage_by_key.insert(
                        key,
                        recur_agent::context_usage::ContextUsage {
                            compacting: false,
                            compacted: prev.is_some_and(|p| p.compacted),
                            ..u
                        },
                    );
                } else if let Some(p) = prev.filter(|p| p.compacting) {
                    self.usage_by_key.insert(
                        key,
                        recur_agent::context_usage::ContextUsage {
                            compacting: false,
                            ..p
                        },
                    );
                }
            }
        }
        self.touch(cx);
    }

    /// `onAgent`: the engine's status is the durable busy signal. A turn start stamps the
    /// clock; idle drops the sticky send, resyncs and drains the queue.
    fn on_agent(&mut self, ws: &str, agent_id: &str, status: &str, cx: &mut Context<Self>) {
        let Some(code) = agent_id
            .strip_prefix(ws)
            .and_then(|r| r.strip_prefix('-'))
            .map(str::to_string)
        else {
            return;
        };
        if !self.accepts(ws, &code, cx) {
            return;
        }
        let key = self.key(&code);
        if is_agent_running_status(status) {
            let list = self.rows(&key);
            let started = self.turn_started_at_by_key.get(&key).copied().unwrap_or(0);
            if should_refresh_turn_stamp(&list, started) {
                if started > 0 && current_turn_has_ended(&list, started) {
                    self.stale_idle_armed_by_key.insert(key.clone(), true);
                }
                self.turn_started_at_by_key.insert(key, now_ms());
            }
            self.touch(cx);
            return;
        }
        if status != "idle" {
            return;
        }
        // A directing/working for the next turn can reorder past this idle: on the first idle
        // after a stamp refresh over a footer skip only the stamp clear and resync.
        let list = self.rows(&key);
        let started = self.turn_started_at_by_key.get(&key).copied().unwrap_or(0);
        let armed = self.stale_idle_armed_by_key.remove(&key).unwrap_or(false);
        let skip = should_skip_idle_transcript_reset(&list, started, armed);
        self.sending_by_key.remove(&key);
        if !skip {
            self.turn_started_at_by_key.remove(&key);
            self.live_model_by_key.remove(&key);
            if let Some(u) = self.usage_by_key.get_mut(&key)
                && u.compacting
            {
                u.compacting = false;
            }
            let rows = drop_open_compact(&self.rows(&key));
            self.chat_by_key.insert(key.clone(), rows);
            self.resync_chat(ws, &code, cx);
        }
        if code != "MA" {
            self.refresh_tab_titles(cx);
        }
        self.try_flush_pending(&key, cx);
        self.touch(cx);
    }

    /// `onAttentionTab`: a background turn finished (or Mark all read cleared every chip).
    fn on_attention_tab(
        &mut self,
        ws: &str,
        code: &str,
        tab_unseen: bool,
        clear_all: bool,
        cx: &mut Context<Self>,
    ) {
        let active = self.active_code();
        let Some(tabs) = self.tabs.as_mut() else {
            return;
        };
        if clear_all {
            if tabs.iter().any(|t| t.unseen) {
                for t in tabs.iter_mut() {
                    t.unseen = false;
                }
                self.put_strip(cx);
                self.touch(cx);
            }
            return;
        }
        if ws.is_empty() || code.is_empty() || ws != self.ws_id || tabs.is_empty() {
            return;
        }
        // The focused tab is already seen server-side; keep its chip clear.
        let unseen = tab_unseen && !(active == code);
        for t in tabs.iter_mut().filter(|t| t.code == code) {
            t.unseen = unseen;
        }
        self.put_strip(cx);
        self.touch(cx);
    }

    /// `onChatsChanged`: a hoist moved chats here (refetch, bust caches) or away (remap this
    /// view's in-flight drafts, queues and sends onto the destination).
    fn on_chats_changed(
        &mut self,
        ws: &str,
        from: Option<&str>,
        remap: Option<CodeRemap>,
        moved: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if from == Some(self.ws_id.as_str()) {
            self.apply_source_hoist(ws, remap.as_ref(), cx);
        }
        if ws != self.ws_id {
            return;
        }
        self.absorb_hoist_drafts(cx);
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        let hoisted = from.is_some();
        cx.spawn(async move |this, cx| {
            let Ok(tabs) = svc.workspaces.list_workspace_chats(&id).await else {
                return;
            };
            if tabs.is_empty() {
                return;
            }
            let _ = this.update(cx, |v, cx| {
                let codes: Vec<String> = tabs.iter().map(|t| t.code.clone()).collect();
                v.set_tabs(tabs, cx);
                if hoisted {
                    v.resync_activated_chats(&codes, Some(remap.unwrap_or_default()), &moved, cx);
                } else {
                    v.resync_activated_chats(&codes, None, &[], cx);
                }
            });
        })
        .detach();
        self.refresh_closed_tabs(cx);
    }

    /// `bustHoistedChatCaches`: hoist can reuse destination codes; their parked caches are the
    /// placeholder's, not the moved session's.
    fn bust_hoisted_chat_caches(&mut self, codes: &[String]) {
        for code in codes.iter().filter(|c| !c.is_empty()) {
            let key = self.key(code);
            if self.loaded_for == key {
                self.loaded_for.clear();
            }
            *self.transcript_gen.entry(key.clone()).or_default() += 1;
            self.pickers_by_key.remove(&key);
            self.simplify_by_key.remove(&key);
            self.pickers_fetched.remove(&key);
            self.simplify_fetched.remove(&key);
            *self.picker_gen.entry(key.clone()).or_default() += 1;
            *self.simplify_gen.entry(key.clone()).or_default() += 1;
            self.pickers_loading.remove(&key);
        }
    }

    /// `resyncActivatedChats`.
    pub(crate) fn resync_activated_chats(
        &mut self,
        codes: &[String],
        remap: Option<CodeRemap>,
        moved: &[String],
        cx: &mut Context<Self>,
    ) {
        let bust = hoist_cache_bust_codes(codes, remap.as_ref(), moved);
        self.bust_hoisted_chat_caches(&bust);
        HoistState::global(cx).take_dest_bust(&self.ws_id);
        let code = self.active_code();
        if code.is_empty() {
            return;
        }
        let id = self.ws_id.clone();
        self.load_chat_pickers(&id, &code, cx).detach();
        if code == "MA" {
            self.load_simplify(&id, &code, cx);
        }
        self.touch(cx);
    }

    /// `applySourceHoist`: this view's host chats moved to `to_id`.
    fn apply_source_hoist(&mut self, to_id: &str, remap: Option<&CodeRemap>, cx: &mut Context<Self>) {
        let from_id = self.ws_id.clone();
        if from_id.is_empty() || to_id.is_empty() || from_id == to_id {
            return;
        }
        let prefix = format!("{from_id}\t");
        let bound = self.input_bound_key.clone();
        let code_of = |k: &str| k.strip_prefix(&prefix).map(str::to_string);
        if let Some(code) = code_of(&bound) {
            let (input, atts) = (self.chat_input(cx), self.chat_attachments(cx));
            HoistState::global(cx).stash_draft(&dest_chat_key(&code, to_id, remap), &input, &atts);
        }
        for (k, input) in self.input_by_key.clone() {
            if k == bound {
                continue;
            }
            if let Some(code) = code_of(&k) {
                let atts = self.attachments_by_key.get(&k).cloned().unwrap_or_default();
                HoistState::global(cx).stash_draft(&dest_chat_key(&code, to_id, remap), &input, &atts);
            }
        }
        for (k, atts) in self.attachments_by_key.clone() {
            if k == bound || self.input_by_key.contains_key(&k) {
                continue;
            }
            if let Some(code) = code_of(&k) {
                HoistState::global(cx).stash_draft(&dest_chat_key(&code, to_id, remap), "", &atts);
            }
        }
        let mut dropped = false;
        for (k, items) in self.pending_by_key.clone() {
            let Some(code) = code_of(&k) else { continue };
            let dest = dest_chat_key(&code, to_id, remap);
            let hold = self.hold_pending_by_key.get(&k).copied().unwrap_or(false);
            let h = HoistState::global(cx);
            h.note_alias(&k, &dest);
            h.stash_pending(&dest, &items, hold);
            self.pending_by_key.remove(&k);
            self.hold_pending_by_key.remove(&k);
            dropped = true;
        }
        for (k, _) in self.hold_pending_by_key.clone() {
            let Some(code) = code_of(&k) else { continue };
            let dest = dest_chat_key(&code, to_id, remap);
            let h = HoistState::global(cx);
            h.note_alias(&k, &dest);
            h.stash_pending(&dest, &[], true);
            self.hold_pending_by_key.remove(&k);
            dropped = true;
        }
        let _ = dropped;
        for (k, on) in self.sending_by_key.clone() {
            if !on {
                continue;
            }
            let Some(code) = code_of(&k) else { continue };
            let dest = dest_chat_key(&code, to_id, remap);
            let gen_ = self.send_gen_by_key.get(&k).copied().unwrap_or(1);
            HoistState::global(cx).stash_sending(&dest, gen_);
            // A same-instance hoist remaps sending next; record identity before dest looks busy.
            if !self.sending_by_key.get(&dest).copied().unwrap_or(false) {
                self.hoist_send_gen_by_key.insert(dest, gen_);
            }
        }
        let h = HoistState::global(cx);
        self.input_by_key = h.remap_prefixed(&self.input_by_key, &from_id, to_id, remap, |d, s| {
            concat_hoist_text(d.map(String::as_str).unwrap_or_default(), s)
        });
        self.attachments_by_key =
            h.remap_prefixed(&self.attachments_by_key, &from_id, to_id, remap, |d, s| {
                concat_hoist_list(d.map(Vec::as_slice).unwrap_or_default(), s)
            });
        self.sending_by_key = h.remap_prefixed(&self.sending_by_key, &from_id, to_id, remap, |d, s| {
            d.copied().unwrap_or(false) || *s
        });
        self.send_gen_by_key = h.remap_prefixed(&self.send_gen_by_key, &from_id, to_id, remap, |d, s| {
            d.copied().unwrap_or(0).max(*s)
        });
        if let Some(code) = code_of(&self.prompt_target_key.clone()) {
            self.prompt_target_key = dest_chat_key(&code, to_id, remap);
        }
        if let Some(code) = code_of(&bound) {
            self.input_bound_key = dest_chat_key(&code, to_id, remap);
        }
        self.touch(cx);
    }

    /// `absorbHoistDrafts`: drafts, queues and in-flight sends stashed for this workspace.
    pub(crate) fn absorb_hoist_drafts(&mut self, cx: &mut Context<Self>) {
        let bound = self.input_bound_key.clone();
        if !bound.is_empty() {
            self.persist_draft_slot(cx);
        }
        let got = HoistState::global(cx).take_drafts(&self.ws_id);
        for (k, d) in &got {
            if !d.input.is_empty() {
                let cur = self.input_by_key.get(k).cloned().unwrap_or_default();
                self.input_by_key.insert(k.clone(), concat_hoist_text(&cur, &d.input));
            }
            if !d.attachments.is_empty() {
                let cur = self.attachments_by_key.get(k).cloned().unwrap_or_default();
                self.attachments_by_key
                    .insert(k.clone(), concat_hoist_list(&cur, &d.attachments));
            }
        }
        if !bound.is_empty() && got.contains_key(&bound) {
            let input = self.input_by_key.get(&bound).cloned().unwrap_or_default();
            let atts = self.attachments_by_key.get(&bound).cloned().unwrap_or_default();
            self.set_chat_draft(input, atts, cx);
        }
        let inflight = HoistState::global(cx).take_sending(&self.ws_id);
        if !inflight.is_empty() {
            let books = absorb_hoist_sending(
                recur_view_chat::hoist::SendBooks {
                    sending: self.sending_by_key.clone(),
                    send_gen: self.send_gen_by_key.clone(),
                    hoist_send_gen: self.hoist_send_gen_by_key.clone(),
                },
                &inflight,
            );
            self.sending_by_key = books.sending;
            self.send_gen_by_key = books.send_gen;
            self.hoist_send_gen_by_key = books.hoist_send_gen;
        }
        let qs = HoistState::global(cx).take_pending(&self.ws_id);
        for (k, q) in &qs {
            if !q.items.is_empty() {
                let cur = self.pending_by_key.get(k).cloned().unwrap_or_default();
                self.pending_by_key
                    .insert(k.clone(), concat_hoist_list(&cur, &q.items));
            }
            if q.hold {
                self.hold_pending_by_key.insert(k.clone(), true);
            }
        }
        for k in qs.keys() {
            self.try_flush_pending(k, cx);
        }
    }

    pub(crate) fn on_hoist_send_complete(&mut self, key: &str, gen_: u64, cx: &mut Context<Self>) {
        if !key.starts_with(&format!("{}\t", self.ws_id)) {
            return;
        }
        let dest = self.send_gen_by_key.get(key).copied().unwrap_or(0);
        if !hoist_send_complete_applies(gen_, dest, self.hoist_send_gen_by_key.get(key).copied()) {
            return;
        }
        self.hoist_send_gen_by_key.remove(key);
        if self.sending_by_key.get(key).copied().unwrap_or(false) {
            self.sending_by_key.insert(key.to_string(), false);
        }
        self.try_flush_pending(key, cx);
        self.touch(cx);
    }

    // ---- escalations, findings ----

    pub(crate) fn load_escalation(&mut self, cx: &mut Context<Self>) {
        self.esc_load_gen += 1;
        let gen_ = self.esc_load_gen;
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let e = svc.escalation.get_escalation(&id).await.ok();
            let _ = this.update(cx, |v, cx| {
                if v.esc_load_gen == gen_ {
                    v.live_esc = take_open_escalation(e);
                    v.touch(cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn load_findings(&mut self, cx: &mut Context<Self>) {
        self.findings_gen += 1;
        let gen_ = self.findings_gen;
        let (svc, id) = (self.services.clone(), self.ws_id.clone());
        cx.spawn(async move |this, cx| {
            let fs = svc.observatory.list_findings(&id).await.unwrap_or_default();
            let _ = this.update(cx, |v, cx| {
                if v.findings_gen == gen_ {
                    v.chat_findings = fs;
                    v.touch(cx);
                }
            });
        })
        .detach();
    }

    // ---- the shell ----

    pub(crate) fn on_shell_event(
        &mut self,
        _: Entity<AppState>,
        ev: &ShellEvent,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ShellEvent::QuestionAnswered(qa) => {
                let (q, ans) = &**qa;
                if q.workspace_id != self.ws_id || self.ws(cx).is_none() {
                    return;
                }
                let key = self.key(&q.agent_code);
                let body = format_ask_user_display_text(&ans.choice, &ans.free_text);
                let last_same = self
                    .chat_by_key
                    .get(&key)
                    .and_then(|l| l.last())
                    .is_some_and(|m| m.me && m.body == body);
                if last_same && ans.attachments.is_empty() {
                    return;
                }
                let attachments = ans
                    .attachments
                    .iter()
                    .map(|a| recur_engine::view::chat::ChatAttachmentView {
                        id: a.id.clone(),
                        name: a.name.clone(),
                        mime: a.mime.clone(),
                        data_url: if a.data.starts_with("data:") {
                            a.data.clone()
                        } else {
                            let mime = if a.mime.is_empty() {
                                "application/octet-stream"
                            } else {
                                &a.mime
                            };
                            format!("data:{mime};base64,{}", a.data)
                        },
                        ..Default::default()
                    })
                    .collect();
                self.push_msg(
                    &key,
                    AgentMsg {
                        me: true,
                        body,
                        optimistic: true,
                        ts: Some(now_ms()),
                        attachments,
                        ..Default::default()
                    },
                );
                self.touch(cx);
            }
            ShellEvent::QuestionAnswerFailed(qa) => {
                let (q, ans) = &**qa;
                if q.workspace_id != self.ws_id || self.ws(cx).is_none() {
                    return;
                }
                let key = self.key(&q.agent_code);
                let body = format_ask_user_display_text(&ans.choice, &ans.free_text);
                if let Some(list) = self.chat_by_key.get_mut(&key) {
                    list.retain(|m| !(m.optimistic && m.me && m.body == body));
                }
                self.touch(cx);
            }
            _ => {}
        }
    }

    // ---- children ----

    pub(crate) fn on_chat_event(
        &mut self,
        _: &Entity<AgentChat>,
        ev: &ChatEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ChatEvent::Send { alt } => self.send("", *alt, cx),
            ChatEvent::Stop => self.stop_chat(cx),
            ChatEvent::Retry => self.retry_chat(cx),
            ChatEvent::RemoveQueued(id) => self.remove_queued(id, cx),
            ChatEvent::CycleMode => self.cycle_chat_mode_from_composer(cx),
            ChatEvent::OpenFile {
                path,
                line,
                end_line,
                start_col,
                end_col,
            } => self.open_agent_file(path, *line, *end_line, *start_col, *end_col, cx),
            ChatEvent::OpenArtifact(a) => {
                let a = a.clone();
                self.observatory
                    .update(cx, |o, cx| o.open_artifact(&a.id, &a.title, &a.kind, &a.path, window, cx))
                    .detach();
            }
            ChatEvent::AnswerEscalation { choice, text } => {
                self.answer_loop_escalation(choice, text, cx).detach();
            }
            ChatEvent::Fork(mid) => self.fork_chat_tab(mid, cx),
            ChatEvent::ForkToWorkspace(mid) => self.fork_chat_to_workspace(mid, cx),
            ChatEvent::TurnDiff(mid) => self.show_turn_diff(mid, window, cx),
            ChatEvent::Reset {
                message_id,
                restores_files,
            } => {
                if self.active_code() != "MA" && !self.loop_pickers_pinned(cx) {
                    self.reset_chat_to(message_id, *restores_files, window, cx);
                }
            }
            ChatEvent::PromptAction { slash, args, alt } => {
                self.invoke_prompt_action(slash, args, *alt, window, cx)
            }
            ChatEvent::Draft => {}
        }
    }

    /// Open a file the agent touched: tool inputs carry absolute worktree paths, the editor
    /// wants repo-relative ones. A PR host jumps the PR's diff instead.
    pub(crate) fn open_agent_file(
        &mut self,
        path: &str,
        line: Option<i64>,
        end_line: Option<i64>,
        start_col: Option<i64>,
        end_col: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.ws(cx).cloned() else {
            return;
        };
        let mut rel = path.to_string();
        if !ws.worktree_path.is_empty() && rel.starts_with(&ws.worktree_path) {
            rel = rel[ws.worktree_path.len()..].trim_start_matches('/').to_string();
        }
        if rel.is_empty() {
            return;
        }
        let n = |v: Option<i64>| v.unwrap_or(0).max(0) as u32;
        let pr_host = !ws.host_pr_id.trim().is_empty();
        self.state.update(cx, |s, cx| {
            if pr_host {
                s.pr_diff_jump = Some(recur_view_core::state::PrDiffJump {
                    file: rel,
                    line: n(line),
                });
            } else {
                s.observatory_jump = Some(recur_view_core::state::ObsJump {
                    path: rel,
                    line: n(line),
                    end_line: n(end_line),
                    start_col: n(start_col),
                    end_col: n(end_col),
                    workspace_id: ws.id.clone(),
                    ..Default::default()
                });
            }
            cx.notify();
        });
    }

    pub(crate) fn on_observatory_event(
        &mut self,
        _: &Entity<Observatory>,
        ev: &ObservatoryEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            ObservatoryEvent::ToggleLeft(open) => {
                self.chat_open = *open;
                self.touch(cx);
            }
            ObservatoryEvent::AgentNewTab => {
                self.new_chat_tab(cx).detach();
            }
            ObservatoryEvent::AgentCloseTab => self.close_active_chat_tab(cx),
            ObservatoryEvent::AgentRotateTab(dir) => self.rotate_chat_tab(*dir, cx),
            ObservatoryEvent::FixFindings => self.fix_findings(window, cx),
            ObservatoryEvent::AddEditorMention {
                path,
                start_line,
                end_line,
                start_col,
                end_col,
            } => {
                let o = |v: usize| (v > 0).then_some(v as i64);
                let (p, sl, el, sc, ec) = (path.clone(), o(*start_line), o(*end_line), o(*start_col), o(*end_col));
                self.chat.update(cx, |c, cx| {
                    c.insert_file_mention(&p, sl, el, sc, ec, window, cx)
                });
            }
            ObservatoryEvent::FocusAgentComposer => {
                self.chat.update(cx, |c, cx| c.focus_composer(window, cx));
            }
            ObservatoryEvent::MaestroVisible(_) => self.touch(cx),
            ObservatoryEvent::AgentLit(lit) => {
                let lit = *lit;
                self.head.update(cx, |h, cx| h.set_lit(lit, cx));
            }
            ObservatoryEvent::SelectedArtifact(id) => {
                let id = id.clone();
                self.plan_panel
                    .update(cx, |p, cx| p.set_selected_artifact(&id, cx));
            }
            ObservatoryEvent::PlanTab(tab) => {
                let tab = tab.clone();
                self.plan_panel.update(cx, |p, cx| p.set_tab_key(&tab, cx));
            }
            ObservatoryEvent::PrDetailFocus(_) => {}
        }
    }

    pub(crate) fn on_plan_event(
        &mut self,
        _: &Entity<PlanPanel>,
        ev: &PlanPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            PlanPanelEvent::OpenArtifact(a) => {
                let a = a.clone();
                self.observatory
                    .update(cx, |o, cx| o.open_artifact(&a.id, &a.title, &a.kind, &a.path, window, cx))
                    .detach();
            }
            PlanPanelEvent::Changed => self.touch(cx),
        }
    }
}

/// `takeOpenEscalation`.
pub(crate) fn take_open_escalation(e: Option<recur_domain::Escalation>) -> Option<recur_domain::Escalation> {
    e.filter(|e| e.status == "open" && !e.question.is_empty())
}
