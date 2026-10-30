//! Live workspace session + fact feed. One per workspace view;
//! the Plan panel and the Loop cockpit both read it.

use std::collections::{BTreeSet, HashMap};

use gpui_kit::{Context, Task};
use recur_domain::{FeedItem, SessionView};
use recur_engine::view::chat::AgentMsg;
use recur_engine::view::chat::{
    StreamKind, add_tool, add_user, append_stream, apply_compact, apply_fallback, apply_notice,
    apply_stream, attach_tool_result, end_turn, map_chat_items, merge_persisted_with_live,
    note_thinking_tokens, patch_tool_input,
};
use recur_engine::view::live::SessionLiveFlags;
use recur_engine::view::loop_cockpit::{
    FeedSess, is_pair_chat_code, is_specialist_session, needs_live_turn_break, specialist_name,
};
use recur_services::{AppEvent, Services};
use recur_view_core::is_agent_running_status;

use crate::session_events::{SessionSignal, StreamEvt, session_event};

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn is_loop_fact(f: &FeedItem) -> bool {
    f.is_deviation
        || f.kind == "outcome"
        || f.kind == "deviation"
        || (f.kind.is_empty() && !f.text.is_empty())
}

fn code_from_agent_id<'a>(workspace_id: &str, agent_id: &'a str) -> &'a str {
    agent_id
        .strip_prefix(workspace_id)
        .and_then(|r| r.strip_prefix('-'))
        .unwrap_or("")
}

fn track(code: &str) -> bool {
    !code.is_empty() && is_specialist_session(code)
}

fn feed_sess(s: SessionView) -> FeedSess {
    FeedSess {
        messages: map_chat_items(s.items.as_deref().unwrap_or_default()),
        id: s.id,
        code: s.code,
        name: s.name,
        kind: s.kind,
        ts: s.created_at,
        live_ts: None,
    }
}

fn keep_live(fresh: FeedSess, live: &FeedSess) -> FeedSess {
    FeedSess {
        id: live.id.clone(),
        ts: live.ts,
        live_ts: live.live_ts,
        name: if fresh.name.is_empty() {
            live.name.clone()
        } else {
            fresh.name.clone()
        },
        messages: merge_persisted_with_live(&fresh.messages, &live.messages),
        ..fresh
    }
}

#[derive(Default)]
pub struct SessionFeed {
    pub sessions: Vec<FeedSess>,
    pub facts: Vec<FeedItem>,
    pub live_codes: BTreeSet<String>,
    workspace_id: String,
    chat_code: String,
    generation: u64,
    owner_of: HashMap<String, String>,
    stream_seq_by_key: HashMap<String, i64>,
    reload_gen_by_code: HashMap<String, u64>,
    live: SessionLiveFlags,
    services: Option<Services>,
    listener: Option<Task<()>>,
}

impl SessionFeed {
    pub fn new(services: Services) -> Self {
        SessionFeed {
            services: Some(services),
            ..Default::default()
        }
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn attach(&mut self, workspace_id: &str, chat_code: &str, cx: &mut Context<Self>) {
        if workspace_id == self.workspace_id && chat_code == self.chat_code {
            return;
        }
        let same_ws = workspace_id == self.workspace_id;
        if !same_ws {
            self.detach();
            self.workspace_id = workspace_id.to_string();
        }
        self.chat_code = chat_code.to_string();
        self.generation += 1;
        self.sessions.clear();
        self.facts.clear();
        self.owner_of.clear();
        self.live.reset();
        self.live_codes.clear();
        self.stream_seq_by_key.clear();
        self.reload_gen_by_code.clear();
        cx.notify();
        if workspace_id.is_empty() {
            return;
        }
        self.load(cx);
        if !same_ws {
            self.listen(cx);
        }
    }

    pub fn detach(&mut self) {
        self.generation += 1;
        self.listener = None;
        self.workspace_id.clear();
        self.chat_code.clear();
        self.live.reset();
    }

    /// Lights `live_codes` so the cockpit can inject a tail card. Does not stamp a session
    /// yet: the agent upsert races persist and a fresh stamp would sort the card before the
    /// prompt, then jump.
    pub fn mark_running<'a>(
        &mut self,
        codes: impl IntoIterator<Item = &'a str>,
        cx: &mut Context<Self>,
    ) {
        for code in codes {
            if track(code) {
                self.set_running(code, cx);
            }
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(services) = self.services.clone() else {
            return;
        };
        let (load_gen, id, owner) = (
            self.generation,
            self.workspace_id.clone(),
            self.chat_code.clone(),
        );
        cx.spawn(async move |this, cx| {
            let (sv, fx) = futures::join!(
                services.workspaces.workspace_sessions(&id, &owner, ""),
                services.workspaces.workspace_facts(&id),
            );
            let _ = this.update(cx, |s, cx| {
                if load_gen != s.generation {
                    return;
                }
                let incoming = sv
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|v| !is_pair_chat_code(&v.code) && s.owns(&v.owner_code))
                    .map(feed_sess)
                    .collect();
                s.sessions = s.merge_loaded(incoming);
                let facts: Vec<FeedItem> = fx
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|f| s.owns(&f.chat_code))
                    .collect();
                s.facts = s.merge_facts(facts);
                cx.notify();
            });
        })
        .detach();
    }

    fn merge_facts(&self, mut incoming: Vec<FeedItem>) -> Vec<FeedItem> {
        let extras: Vec<FeedItem> = self
            .facts
            .iter()
            .filter(|f| {
                if !f.id.is_empty() && incoming.iter().any(|x| x.id == f.id) {
                    return false;
                }
                // Live stamps now when ts is missing; persist uses the event ts. Same
                // milestone, two clocks: keep one.
                !incoming.iter().any(|x| {
                    x.text == f.text && x.agent_code == f.agent_code && (x.ts - f.ts).abs() < 5000
                })
            })
            .cloned()
            .collect();
        incoming.extend(extras);
        incoming
    }

    fn merge_loaded(&self, incoming: Vec<FeedSess>) -> Vec<FeedSess> {
        let mut out: Vec<FeedSess> = incoming
            .into_iter()
            .map(|s| match self.sessions.iter().find(|l| l.code == s.code) {
                Some(live) if self.live_codes.contains(&s.code) => keep_live(s, live),
                _ => s,
            })
            .collect();
        for live in &self.sessions {
            if self.live_codes.contains(&live.code) && !out.iter().any(|s| s.code == live.code) {
                out.push(live.clone());
            }
        }
        out
    }

    fn owns(&self, owner: &str) -> bool {
        let tab = self.chat_code.as_str();
        if tab.is_empty() {
            return owner.is_empty();
        }
        if owner.is_empty() {
            return tab == "PA" || tab == "MA";
        }
        owner == tab
    }

    fn for_me(&mut self, id: &str, code: &str, owner: &str) -> bool {
        if id != self.workspace_id || !track(code) {
            return false;
        }
        if !owner.is_empty() {
            self.owner_of.insert(code.to_string(), owner.to_string());
        }
        match self.owner_of.get(code) {
            None => true,
            Some(known) => self.owns(&known.clone()),
        }
    }

    fn publish_live(&mut self, cx: &mut Context<Self>) {
        self.live_codes = self.live.snapshot();
        cx.notify();
    }

    fn set_running(&mut self, code: &str, cx: &mut Context<Self>) {
        if self.live.mark_running(code) {
            self.publish_live(cx);
            self.touch_live_ts(code);
        }
    }

    fn touch_stream(&mut self, code: &str, cx: &mut Context<Self>) {
        if self.live.touch_from_stream(code) {
            self.publish_live(cx);
            self.touch_live_ts(code);
        }
    }

    fn touch_live_ts(&mut self, code: &str) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.code == code) {
            s.live_ts = Some(now_ms());
        }
    }

    fn reload_code(&mut self, code: &str, cx: &mut Context<Self>) {
        let Some(services) = self.services.clone() else {
            return;
        };
        if self.workspace_id.is_empty() {
            return;
        }
        let (generation, id, owner) = (
            self.generation,
            self.workspace_id.clone(),
            self.chat_code.clone(),
        );
        let load_gen = self.reload_gen_by_code.get(code).copied().unwrap_or(0) + 1;
        self.reload_gen_by_code.insert(code.to_string(), load_gen);
        let code = code.to_string();
        cx.spawn(async move |this, cx| {
            let sv = services
                .workspaces
                .workspace_sessions(&id, &owner, &code)
                .await;
            let _ = this.update(cx, |s, cx| {
                if generation != s.generation || s.reload_gen_by_code.get(&code) != Some(&load_gen)
                {
                    return;
                }
                let Some(fresh) = sv
                    .unwrap_or_default()
                    .into_iter()
                    .find(|v| v.code == code && s.owns(&v.owner_code))
                else {
                    return;
                };
                let mapped = feed_sess(fresh);
                let i = s.sessions.iter().position(|x| x.code == code);
                let next = match i {
                    Some(i) if s.live_codes.contains(&code) => keep_live(mapped, &s.sessions[i]),
                    _ => mapped,
                };
                match i {
                    Some(i) => s.sessions[i] = next,
                    None => s.sessions.push(next),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn patch(
        &mut self,
        code: &str,
        cx: &mut Context<Self>,
        f: impl FnOnce(&[AgentMsg]) -> Vec<AgentMsg>,
    ) {
        if !track(code) {
            return;
        }
        let now = now_ms();
        let live = self.live_codes.contains(code);
        match self.sessions.iter_mut().find(|s| s.code == code) {
            None => self.sessions.push(FeedSess {
                id: format!("live-{code}"),
                code: code.to_string(),
                name: specialist_name(code),
                kind: String::new(),
                ts: now,
                live_ts: Some(now),
                messages: f(&[]),
            }),
            Some(sess) => {
                if !live && needs_live_turn_break(&sess.messages) {
                    sess.messages.push(AgentMsg {
                        turn_end: true,
                        ts: Some(now),
                        ..AgentMsg::default()
                    });
                }
                sess.live_ts = match sess.live_ts {
                    Some(t) if live && !sess.messages.is_empty() => Some(t),
                    _ => Some(now),
                };
                sess.messages = f(&sess.messages);
            }
        }
        cx.notify();
        self.touch_stream(code, cx);
    }

    fn apply_live(&mut self, code: &str, kind: StreamKind, e: &StreamEvt, cx: &mut Context<Self>) {
        let tag = if kind == StreamKind::Text {
            "text"
        } else {
            "thinking"
        };
        let mut key = format!("{}:{code}:{tag}", self.workspace_id);
        if !e.parent_id.is_empty() {
            key = format!("{key}:{}", e.parent_id);
        }
        if e.seq > 0 {
            if e.seq <= self.stream_seq_by_key.get(&key).copied().unwrap_or(0) {
                return;
            }
            self.stream_seq_by_key.insert(key, e.seq);
        }
        if e.replace || e.done || !e.parent_id.is_empty() {
            self.patch(code, cx, |m| apply_stream(m, kind, &e.text, &e.opts()));
        } else if !e.text.is_empty() {
            self.patch(code, cx, |m| append_stream(m, kind, &e.text));
        }
    }

    fn listen(&mut self, cx: &mut Context<Self>) {
        let Some(services) = self.services.clone() else {
            return;
        };
        let mut events = services.subscribe();
        self.listener = Some(cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                if this.update(cx, |s, cx| s.apply_event(ev, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub fn apply_event(&mut self, ev: AppEvent, cx: &mut Context<Self>) {
        match ev {
            AppEvent::AgentUpdated(a) => {
                if a.workspace_id != self.workspace_id {
                    return;
                }
                let code = code_from_agent_id(&a.workspace_id, &a.agent_id).to_string();
                if !track(&code) || !self.for_me(&a.workspace_id, &code, "") {
                    return;
                }
                if is_agent_running_status(&a.status) {
                    self.set_running(&code, cx);
                    return;
                }
                if self.live.mark_settled(&code) {
                    self.publish_live(cx);
                }
                self.reload_code(&code, cx);
            }
            AppEvent::WorkspaceEvent(w) => {
                if w.workspace_id != self.workspace_id {
                    return;
                }
                let mut f = w.event;
                if !is_loop_fact(&f) || !self.owns(&f.chat_code) {
                    return;
                }
                if f.ts == 0 {
                    f.ts = now_ms();
                }
                match self
                    .facts
                    .iter_mut()
                    .find(|x| !x.id.is_empty() && x.id == f.id)
                {
                    // A re-emitted card (progress) replaces the payload in its slot.
                    Some(x) => {
                        let ts = if x.ts != 0 { x.ts } else { f.ts };
                        *x = FeedItem { ts, ..f };
                    }
                    None => self.facts.push(f),
                }
                cx.notify();
            }
            ev => self.apply_session_event(&ev, cx),
        }
    }

    fn apply_session_event(&mut self, ev: &AppEvent, cx: &mut Context<Self>) {
        let Some(e) = session_event(ev) else {
            return;
        };
        let (id, code) = (e.workspace_id.as_str(), e.code.as_str());
        let owner = match &e.signal {
            SessionSignal::Delta(d) | SessionSignal::Thinking(d) => d.owner_code.as_str(),
            SessionSignal::Step(s) => s.owner_code.as_str(),
            _ => "",
        };
        if !self.for_me(id, code, owner) {
            return;
        }
        match &e.signal {
            SessionSignal::Delta(d) => self.apply_live(code, StreamKind::Text, d, cx),
            SessionSignal::Thinking(d) => {
                if !d.text.is_empty() || d.replace || d.done {
                    self.apply_live(code, StreamKind::Thinking, d, cx);
                } else {
                    self.patch(code, cx, |m| {
                        note_thinking_tokens(m, d.tokens, &d.uuid, &d.parent_id)
                    });
                }
            }
            SessionSignal::Step(step) => self.patch(code, cx, |m| add_tool(m, step)),
            SessionSignal::ToolResult {
                id,
                result,
                is_error,
                extras,
            } => {
                let extras = extras.clone().unwrap_or_default();
                self.patch(code, cx, |m| {
                    attach_tool_result(m, id, result, *is_error, &extras)
                });
            }
            SessionSignal::ToolInput { id, extras } => {
                self.patch(code, cx, |m| patch_tool_input(m, id, extras));
            }
            SessionSignal::TurnEnd(t) => {
                self.patch(code, cx, |m| end_turn(m, t.duration_ms, now_ms()));
                // Per provider run, not per working window: drop live without latching
                // settled so the next nudge's deltas can re-light; idle is the latch.
                if self.live.clear_live(code) {
                    self.publish_live(cx);
                }
            }
            SessionSignal::Compact(c) => self.patch(code, cx, |m| apply_compact(m, c)),
            SessionSignal::Notice(text) => self.patch(code, cx, |m| apply_notice(m, text)),
            SessionSignal::User(text) => self.patch(code, cx, |m| add_user(m, text, now_ms())),
            SessionSignal::Fallback(f) => self.patch(code, cx, |m| apply_fallback(m, f)),
            SessionSignal::ContextUsage(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{AppContext as _, TestAppContext};
    use parking_lot::Mutex;
    use recur_domain::{ChatItem, FeedItem, SessionView};
    use recur_services::events::{
        AgentUpdated, SessionDelta, SessionTag, SessionTurnEnd, WorkspaceEvent,
    };
    use recur_services::fake::{Fake, Overrides, Release, hold};
    use recur_services::{AppEvent, Result};

    use super::*;

    fn session(code: &str, text: &str, owner: &str) -> SessionView {
        SessionView {
            id: code.into(),
            code: code.into(),
            owner_code: owner.into(),
            created_at: 1,
            items: Some(vec![ChatItem {
                kind: "text".into(),
                role: "assistant".into(),
                text: text.into(),
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    fn tag(code: &str) -> SessionTag {
        SessionTag {
            workspace_id: "ws".into(),
            code: code.into(),
            ..Default::default()
        }
    }

    fn body(feed: &SessionFeed, i: usize) -> String {
        feed.sessions[i].messages[0].body.clone()
    }

    #[gpui::test]
    async fn loads_only_the_selected_chat_and_refreshes_only_the_completed_specialist(
        cx: &mut TestAppContext,
    ) {
        let calls: Arc<Mutex<Vec<(String, String, String)>>> = Default::default();
        let replies = Arc::new(Mutex::new(vec![
            vec![session("CO", "finished", "PA2")],
            vec![
                session("CO", "coding", "PA2"),
                session("RV", "reviewing", "PA2"),
            ],
        ]));
        let (c, r) = (calls.clone(), replies.clone());
        let fake = Fake::fresh(
            Overrides::default()
                .workspace_sessions(move |_, args| {
                    c.lock().push(args);
                    Ok(r.lock().pop().unwrap_or_default())
                })
                .workspace_facts(|_, _| Ok(Vec::new())),
        );
        let feed = cx.new(|_| SessionFeed::new(fake.services()));
        feed.update(cx, |f, cx| f.attach("ws", "PA2", cx));
        cx.run_until_parked();
        assert_eq!(feed.read_with(cx, |f, _| f.sessions.len()), 2);
        let call = |a: &str, b: &str, c: &str| (a.to_owned(), b.to_owned(), c.to_owned());
        assert_eq!(*calls.lock(), vec![call("ws", "PA2", "")]);

        fake.dispatch(AppEvent::AgentUpdated(AgentUpdated {
            workspace_id: "ws".into(),
            agent_id: "ws-CO".into(),
            status: "idle".into(),
            ..Default::default()
        }));
        cx.run_until_parked();
        feed.read_with(cx, |f, _| {
            assert_eq!(body(f, 0), "finished");
            assert_eq!(body(f, 1), "reviewing");
        });
        assert_eq!(calls.lock().last().unwrap(), &call("ws", "PA2", "CO"));
    }

    #[gpui::test]
    async fn late_scoped_loads_cannot_overwrite_a_newer_visit_to_the_same_chat(
        cx: &mut TestAppContext,
    ) {
        type Pending = Vec<Release<Result<Vec<SessionView>>>>;
        let pending: Arc<Mutex<Pending>> = Default::default();
        let p = pending.clone();
        let fake = Fake::fresh(
            Overrides::default()
                .workspace_sessions(move |_, _| {
                    let (answer, release) = hold();
                    p.lock().push(release);
                    answer
                })
                .workspace_facts(|_, _| Ok(Vec::new())),
        );
        let feed = cx.new(|_| SessionFeed::new(fake.services()));
        feed.update(cx, |f, cx| {
            f.attach("ws", "PA2", cx);
            f.attach("ws", "PA3", cx);
            f.attach("ws", "PA2", cx);
        });
        cx.run_until_parked();
        let mut rel: Pending = std::mem::take(&mut *pending.lock());
        assert_eq!(rel.len(), 3);
        rel.pop()
            .unwrap()
            .send(Ok(vec![session("CO", "latest", "PA2")]));
        cx.run_until_parked();
        assert_eq!(feed.read_with(cx, |f, _| body(f, 0)), "latest");
        rel.pop()
            .unwrap()
            .send(Ok(vec![session("CO", "other chat", "PA3")]));
        rel.pop()
            .unwrap()
            .send(Ok(vec![session("CO", "stale", "PA2")]));
        cx.run_until_parked();
        assert_eq!(feed.read_with(cx, |f, _| body(f, 0)), "latest");
    }

    #[gpui::test]
    async fn live_deltas_light_and_turn_end_clears(cx: &mut TestAppContext) {
        let fake = Fake::fresh(
            Overrides::default()
                .workspace_sessions(|_, _| Ok(Vec::new()))
                .workspace_facts(|_, _| Ok(Vec::new())),
        );
        let feed = cx.new(|_| SessionFeed::new(fake.services()));
        feed.update(cx, |f, cx| f.attach("ws", "", cx));
        cx.run_until_parked();
        let d = |text: &str, seq: i64| {
            AppEvent::SessionDelta(SessionDelta {
                tag: tag("CO"),
                text: text.into(),
                seq: Some(seq),
                replace: true,
                ..Default::default()
            })
        };
        fake.dispatch(d("hel", 1));
        fake.dispatch(d("hello", 2));
        fake.dispatch(d("stale", 1));
        fake.dispatch(AppEvent::SessionDelta(SessionDelta {
            tag: tag("PA"),
            text: "x".into(),
            ..Default::default()
        }));
        cx.run_until_parked();
        feed.read_with(cx, |f, _| {
            assert_eq!(f.sessions.len(), 1);
            assert_eq!(f.sessions[0].name, "Coder");
            assert_eq!(body(f, 0), "hello");
            assert!(f.live_codes.contains("CO"));
        });
        fake.dispatch(AppEvent::SessionTurnEnd(SessionTurnEnd {
            tag: tag("CO"),
            duration_ms: 5,
            ..Default::default()
        }));
        fake.dispatch(AppEvent::WorkspaceEvent(WorkspaceEvent {
            workspace_id: "ws".into(),
            event: FeedItem {
                id: "f1".into(),
                kind: "outcome".into(),
                text: "done".into(),
                ..Default::default()
            },
        }));
        cx.run_until_parked();
        feed.read_with(cx, |f, _| {
            assert!(f.live_codes.is_empty());
            assert!(f.sessions[0].messages.last().unwrap().turn_end);
            assert_eq!(f.facts.len(), 1);
            assert!(f.facts[0].ts > 0);
        });
    }
}
