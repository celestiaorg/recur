//! `frontend/e2e/chat.spec.ts`. Tab routing, the follow-up queue and Stop are the
//! workspace view's; these drive the chat through the [`Host`] double and assert what the chat
//! shows and emits.

use std::sync::Arc;

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Release, Reply};
use serde_json::{Value, json};

use super::harness::*;
use crate::chat::{ChatEvent, now_ms};

type Pending = Arc<Mutex<Vec<Release>>>;

/// `SendWorkspaceMessage` / `WorkspaceChatItems` over one transcript store: the user turn is
/// recorded, then `reply` decides the answer (`None` hangs until released through `hold`).
fn bridge_opts(
    store: Store,
    sent: Calls,
    hold: Pending,
    reply: impl Fn(usize, &str) -> Option<String> + Send + Sync + 'static,
) -> MockOptions {
    let s2 = store.clone();
    MockOptions::default()
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
        .resolver("SendWorkspaceMessage", move |a, _, _| {
            let n = {
                let mut s = sent.lock();
                s.push(a.to_vec());
                s.len()
            };
            let code = a[1].as_str().filter(|s| !s.is_empty()).unwrap_or("MA").to_string();
            let text = a[2].as_str().unwrap_or_default().to_string();
            let display = a[7].as_str().unwrap_or_default().to_string();
            if display != "\u{2060}" {
                store
                    .lock()
                    .entry(code.clone())
                    .or_default()
                    .push(json!({"role": "user", "kind": "text", "text": if display.is_empty() { &text } else { &display }}));
            }
            match reply(n, &text) {
                Some(r) => {
                    store.lock().entry(code).or_default().push(json!({"role": "assistant", "kind": "text", "text": r}));
                    Reply::value(json!(r))
                }
                None => {
                    let (r, release) = Reply::deferred();
                    hold.lock().push(release);
                    r
                }
            }
        })
        .resolver("WorkspaceChatItems", move |a, _, _| {
            let code = a.get(1).and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("MA");
            Reply::value(Value::Array(s2.lock().get(code).cloned().unwrap_or_default()))
        })
}

fn texts(sent: &Calls) -> Vec<String> {
    sent.lock().iter().map(|a| a[2].as_str().unwrap_or_default().to_string()).collect()
}

fn release(hold: &Pending, store: &Store, code: &str, reply: &str) {
    store.lock().entry(code.into()).or_default().push(json!({"role": "assistant", "kind": "text", "text": reply}));
    if let Some(r) = hold.lock().pop() {
        r.send(Reply::value(json!(reply)));
    }
}

// ---- Maestro chat auto-scroll ----

#[gpui::test]
async fn sending_a_message_scrolls_to_the_bottom(cx: &mut TestAppContext) {
    let opts = bridge_opts(Store::default(), calls(), Pending::default(), |_, t| Some(format!("reply to: {t}")));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    for i in 0..8 {
        h.fill(&format!("message {i}"));
        h.click("chat-send");
        assert!(h.me(&format!("message {i}")), "message {i}");
    }
    assert!(h.max_scroll() > 100., "the transcript overflows");
    assert!(h.dist_from_bottom() < 4.);
}

#[gpui::test]
async fn scrolling_away_from_the_bottom_keeps_position_when_a_reply_streams_in(cx: &mut TestAppContext) {
    let (store, hold) = (Store::default(), Pending::default());
    let opts = bridge_opts(store.clone(), calls(), hold.clone(), |n, t| (n > 1).then(|| format!("reply to: {t}")));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    let mut prior = Vec::new();
    for i in 0..10 {
        prior.push(json!({"role": "user", "kind": "text", "text": format!("question {i}")}));
        prior.push(json!({"role": "assistant", "kind": "text", "text": format!("answer {i}\n\n{}", "paragraph ".repeat(30))}));
    }
    store.lock().insert("MA".into(), prior.clone());
    h.set_messages(items(Value::Array(prior)));
    h.fill("kick off a slow reply");
    h.click("chat-send");
    h.scroll_to(0.);
    let before = h.scroll_top();
    release(&hold, &store, "MA", "a slow reply");
    h.settle();
    assert!(h.assistant("a slow reply"));
    assert_eq!(h.scroll_top(), before);
}

// ---- Chat follow-up queue ----

#[gpui::test]
async fn all_follow_ups_queued_while_a_turn_is_running_fold_into_the_next_user_turn(cx: &mut TestAppContext) {
    let (store, sent, hold) = (Store::default(), calls(), Pending::default());
    let opts = bridge_opts(store.clone(), sent.clone(), hold.clone(), |n, t| (n > 1).then(|| format!("reply to: {t}")));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.fill("first turn");
    h.click("chat-send");
    assert!(h.has("chat-stop"));
    h.fill("fix the tests");
    h.click("chat-send");
    h.fill("and update the docs");
    h.click("chat-send");
    assert!(h.has("queued:fix the tests"));
    assert!(h.has("queued:and update the docs"));
    assert_eq!(texts(&sent), ["first turn"]);

    h.click("queued-x:fix the tests");
    assert!(!h.has("queued:fix the tests"));
    assert!(h.has("queued:and update the docs"));

    release(&hold, &store, "MA", "done with first");
    h.settle();
    assert_eq!(texts(&sent), ["first turn", "and update the docs"]);
    assert!(!h.has("queued:and update the docs"));
    assert!(h.me("and update the docs"));
    assert!(!h.me("fix the tests"));
}

async fn busy_follow_ups(cx: &mut TestAppContext, queue_follow_ups: bool) {
    let (store, sent) = (Store::default(), calls());
    let opts = bridge_opts(store, sent.clone(), Pending::default(), |n, _| (n > 1).then(|| "\u{2060}inject".to_string()));
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, opts, props, ws_hooks(), true);
    h.host(|h, cx| {
        h.steer_by_default = !queue_follow_ups;
        h.push(cx);
    });
    h.fill("first turn");
    h.keys("enter");
    assert!(h.has("chat-stop"));
    let (steer, queue) = if queue_follow_ups { ("cmd-enter", "enter") } else { ("enter", "cmd-enter") };

    h.fill("steer now");
    h.keys(steer);
    assert_eq!(texts(&sent), ["first turn", "steer now"]);
    assert!(!h.has("queued:steer now"));

    h.fill("after the turn");
    h.keys(queue);
    assert!(h.has("queued:after the turn"));
    assert_eq!(texts(&sent), ["first turn", "steer now"]);

    h.fill("steer past the queue");
    h.keys(steer);
    assert_eq!(texts(&sent), ["first turn", "steer now", "steer past the queue"]);
    assert!(h.has("queued:after the turn"));
}

#[gpui::test]
async fn busy_follow_ups_enter_steers_cmd_enter_queues(cx: &mut TestAppContext) {
    busy_follow_ups(cx, false).await;
}

#[gpui::test]
async fn busy_follow_ups_enter_queues_cmd_enter_steers(cx: &mut TestAppContext) {
    busy_follow_ups(cx, true).await;
}

#[gpui::test]
async fn stop_restores_queued_follow_ups_into_the_composer(cx: &mut TestAppContext) {
    let (store, sent, hold) = (Store::default(), calls(), Pending::default());
    let (s2, h2) = (store.clone(), hold.clone());
    let opts = bridge_opts(store.clone(), sent.clone(), hold.clone(), |n, t| (n > 1).then(|| format!("reply to: {t}")))
        .resolver("CancelWorkspaceMessage", move |_, _, _| {
            release(&h2, &s2, "MA", "Stopped by user");
            Reply::null()
        });
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.fill("first turn");
    h.click("chat-send");
    assert!(h.has("chat-stop"));
    h.fill("fix the tests");
    h.click("chat-send");
    h.fill("and update the docs");
    h.click("chat-send");
    assert!(h.has("queued:fix the tests") && h.has("queued:and update the docs"));

    h.click("chat-stop");
    assert!(h.has("chat-stopped"));
    assert!(!h.has("queued:fix the tests") && !h.has("queued:and update the docs"));
    let lines: Vec<String> = h.lines().into_iter().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines, ["fix the tests", "and update the docs"]);
    assert_eq!(texts(&sent), ["first turn"]);
}

// ---- Composer undo ----

#[gpui::test]
async fn cmd_z_undoes_one_typing_burst_at_a_time_including_shift_enter(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.focus_composer();
    let lines = |h: &mut H| h.lines();
    h.type_text("hello");
    h.wait(1000);
    h.type_text(" world");
    h.keys("shift-enter");
    h.type_text("next");

    h.keys("cmd-z");
    assert_eq!(lines(&mut h), ["hello world", ""]);
    h.keys("cmd-z");
    assert_eq!(lines(&mut h), ["hello world"]);
    h.keys("cmd-z");
    assert_eq!(lines(&mut h), ["hello"]);
    h.keys("cmd-shift-z");
    assert_eq!(lines(&mut h), ["hello world"]);
}

// ---- Fix findings ----

fn prompt_actions(h: &H) -> Vec<String> {
    h.events()
        .into_iter()
        .filter_map(|e| match e {
            ChatEvent::PromptAction { slash, .. } => Some(slash),
            _ => None,
        })
        .collect()
}

#[gpui::test]
async fn slash_fix_findings_shows_a_short_label_while_sending_the_full_checklist(cx: &mut TestAppContext) {
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, MockOptions::default(), props, ws_hooks(), true);
    h.focus_composer();
    h.type_text("/fix-findings");
    assert!(h.has("mention:/fix-findings"));
    h.keys("enter");
    assert_eq!(prompt_actions(&h), ["fix-findings"]);
    // Enter keeps focus on the composer while the /query clears.
    assert!(h.input().is_empty());
    let focused = h.cx.update(|window, cx| h.chat.read(cx).focus.is_focused(window));
    assert!(focused);
    assert_eq!(h.read(|c| c.doc.caret), 0);
}

#[gpui::test]
async fn slash_review_and_fix_opens_a_loop_tab_instead_of_converting_the_agent_chat(cx: &mut TestAppContext) {
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, MockOptions::default(), props, ws_hooks(), true);
    h.focus_composer();
    h.type_text("/review-and-fix");
    assert!(h.has("mention:/review-and-fix"));
    assert!(h.has("badge:Loop"));
    h.keys("enter");
    assert_eq!(prompt_actions(&h), ["review-and-fix"]);
}

/// A started agent tab, then `/slash` picked from the menu with a click.
async fn started_tab_then_pick(cx: &mut TestAppContext, slash: &str) -> (Vec<String>, Vec<String>) {
    let (store, sent) = (Store::default(), calls());
    let opts = bridge_opts(store, sent.clone(), Pending::default(), |_, _| Some("On it.".into()));
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, opts, props, ws_hooks(), true);
    h.fill("hello from the agent tab");
    h.click("chat-send");
    assert!(h.me("hello from the agent tab"));
    h.focus_composer();
    h.type_text(&format!("/{slash}"));
    h.click(&format!("mention:/{slash}"));
    (prompt_actions(&h), texts(&sent))
}

#[gpui::test]
async fn slash_review_on_a_started_agent_tab_opens_a_new_chat(cx: &mut TestAppContext) {
    let (actions, sent) = started_tab_then_pick(cx, "review").await;
    assert_eq!(actions, ["review"]);
    // The routed send is the host's: nothing went to the started tab.
    assert_eq!(sent, ["hello from the agent tab"]);
}

#[gpui::test]
async fn slash_commit_on_a_started_agent_tab_opens_a_grouped_tab_and_isolates(cx: &mut TestAppContext) {
    let (actions, sent) = started_tab_then_pick(cx, "commit").await;
    assert_eq!(actions, ["commit"]);
    assert_eq!(sent, ["hello from the agent tab"]);
}

/// The draft of the tab the slash was not typed in survives the pick.
async fn draft_survives_slash(cx: &mut TestAppContext, slash: &str) {
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, MockOptions::default(), props, ws_hooks(), true);
    h.fill("unsent draft stays here");
    h.host(|h, cx| h.switch("MA", cx));
    assert!(h.input().is_empty());
    h.focus_composer();
    h.type_text(&format!("/{slash}"));
    h.click(&format!("mention:/{slash}"));
    assert_eq!(prompt_actions(&h), [slash]);
    h.host(|h, cx| h.switch("PA", cx));
    assert_eq!(h.input(), "unsent draft stays here");
}

#[gpui::test]
async fn slash_commit_does_not_claim_a_new_chat_with_an_unsent_draft(cx: &mut TestAppContext) {
    draft_survives_slash(cx, "commit").await;
}

#[gpui::test]
async fn failed_deferred_commit_retry_sends_a_fresh_grouped_action_without_eating_the_draft(cx: &mut TestAppContext) {
    draft_survives_slash(cx, "commit").await;
}

#[gpui::test]
async fn slash_commit_push_does_not_reuse_an_ops_tab_with_an_unsent_draft(cx: &mut TestAppContext) {
    draft_survives_slash(cx, "commit-push").await;
}

#[gpui::test]
async fn slash_review_send_stays_on_the_routed_tab_if_the_operator_switches_away(cx: &mut TestAppContext) {
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, MockOptions::default(), props, ws_hooks(), true);
    h.focus_composer();
    h.type_text("/review");
    h.click("mention:/review");
    assert_eq!(prompt_actions(&h), ["review"]);
    // Switching away while the action runs leaves this tab's transcript alone.
    h.host(|h, cx| h.switch("MA", cx));
    assert!(!h.me("Please review the changes in this worktree and file any real issues"));
}

#[gpui::test]
async fn findings_panel_fix_open_findings_switches_to_the_agent_dock_and_tab(cx: &mut TestAppContext) {
    // The panel's button sends `/fix-findings` through the same prompt action path.
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, MockOptions::default(), props, ws_hooks(), true);
    h.focus_composer();
    h.type_text("/fix-findings");
    h.click("mention:/fix-findings");
    assert_eq!(prompt_actions(&h), ["fix-findings"]);
}

// ---- Chat stop ----

/// Sends hang per code; `CancelWorkspaceMessage` records `ws:code` and answers that code's turn.
fn cancel_opts(store: Store, cancelled: Calls) -> MockOptions {
    let holds: Arc<Mutex<std::collections::HashMap<String, Release>>> = Default::default();
    let (h1, h2, s1, s2, s3) = (holds.clone(), holds, store.clone(), store.clone(), store);
    MockOptions::default()
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
        .resolver("WorkspaceChatItems", move |a, _, _| {
            let code = a.get(1).and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("MA");
            Reply::value(Value::Array(s1.lock().get(code).cloned().unwrap_or_default()))
        })
        .resolver("SendWorkspaceMessage", move |a, _, _| {
            let code = a[1].as_str().unwrap_or("MA").to_string();
            s2.lock().entry(code.clone()).or_default().push(json!({"role": "user", "kind": "text", "text": a[2]}));
            let (r, release) = Reply::deferred();
            h1.lock().insert(code, release);
            r
        })
        .resolver("CancelWorkspaceMessage", move |a, _, _| {
            let (ws, code) = (a[0].as_str().unwrap_or_default(), a[1].as_str().unwrap_or_default().to_string());
            cancelled.lock().push(vec![json!(format!("{ws}:{code}"))]);
            s3.lock().entry(code.clone()).or_default().push(json!({"role": "assistant", "kind": "text", "text": "Stopped by user"}));
            if let Some(r) = h2.lock().remove(&code) {
                r.send(Reply::value(json!("Stopped by user")));
            }
            Reply::null()
        })
}

fn flat(c: &Calls) -> Vec<String> {
    c.lock().iter().map(|a| a[0].as_str().unwrap_or_default().to_string()).collect()
}

#[gpui::test]
async fn stop_interrupts_the_active_tabs_in_flight_turn(cx: &mut TestAppContext) {
    let cancelled = calls();
    let mut h = boot(cx, cancel_opts(Store::default(), cancelled.clone()), ws_props(), ws_hooks(), true);
    h.fill("please run forever");
    h.click("chat-send");
    assert!(h.has("chat-stop"));
    h.click("chat-stop");
    assert!(h.has("chat-stopped"));
    assert_eq!(flat(&cancelled), [format!("{WS}:MA")]);
    assert!(!h.has("chat-stop"));
    assert!(h.has("chat-send-disabled"));
}

#[gpui::test]
async fn cmd_shift_backspace_stops_an_in_flight_turn_when_the_agent_panel_is_focused(cx: &mut TestAppContext) {
    let cancelled = calls();
    let mut h = boot(cx, cancel_opts(Store::default(), cancelled.clone()), ws_props(), ws_hooks(), true);
    h.fill("please run forever");
    h.click("chat-send");
    assert!(h.has("chat-stop"));
    h.focus_composer();
    h.keys("cmd-shift-backspace");
    assert!(h.has("chat-stopped"));
    assert_eq!(flat(&cancelled), [format!("{WS}:MA")]);
}

#[gpui::test]
async fn stop_cancels_only_the_active_tab_a_background_tabs_turn_keeps_running(cx: &mut TestAppContext) {
    let cancelled = calls();
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, cancel_opts(Store::default(), cancelled.clone()), props, ws_hooks(), true);
    h.fill("long PA task");
    h.click("chat-send");
    h.host(|h, cx| h.switch("PA2", cx));
    assert!(!h.has("chat-stop"));
    h.fill("long PA2 task");
    h.click("chat-send");
    assert!(h.has("chat-stop"));
    h.click("chat-stop");
    assert!(h.has("chat-stopped"));
    assert_eq!(flat(&cancelled), [format!("{WS}:PA2")]);
    h.host(|h, cx| h.switch("PA", cx));
    assert!(h.me("long PA task"));
    assert!(!h.has("chat-stopped"));
}

// ---- Chat live turn restore ----

fn hang_opts(store: Store, sent: Calls) -> MockOptions {
    bridge_opts(store, sent, Pending::default(), |_, _| None)
}

#[gpui::test]
async fn leaving_and_returning_mid_turn_keeps_earlier_messages_and_the_elapsed_clock(cx: &mut TestAppContext) {
    let store = Store::default();
    let earlier = json!([
        {"role": "user", "kind": "text", "text": "earlier question"},
        {"role": "assistant", "kind": "text", "text": "earlier answer"},
    ]);
    store.lock().insert("MA".into(), earlier.as_array().unwrap().clone());
    let mut h = boot(cx, hang_opts(store, calls()), ws_props(), ws_hooks(), true);
    h.set_messages(items(earlier));
    assert!(h.me("earlier question"));
    h.fill("keep going");
    h.click("chat-send");
    assert!(h.has("chat-turn-elapsed"));
    h.host(|h, cx| {
        h.tab().turn_started_at -= 5_000;
        h.push(cx);
    });
    let before = h.read(|c| c.turn_elapsed_sec);
    assert!(before >= 5);

    h.remount();
    assert!(h.me("earlier question"));
    assert!(h.me("keep going"));
    assert!(h.assistant("earlier answer"));
    assert!(h.has("chat-turn-elapsed"));
    assert!(h.read(|c| c.turn_elapsed_sec) >= before);
}

#[gpui::test]
async fn elapsed_clock_hides_when_the_engine_goes_idle_while_send_is_still_pending(cx: &mut TestAppContext) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()), ws_props(), ws_hooks(), true);
    h.fill("keep going");
    h.click("chat-send");
    assert!(h.has("chat-turn-elapsed"));
    h.host(|h, cx| h.idle(cx));
    assert!(!h.has("chat-turn-elapsed"));
    // Send leaves queue mode.
    let tip = h.read(|c| c.props.sending);
    assert!(!tip);
}

#[gpui::test]
async fn elapsed_clock_hides_when_turn_end_lands_while_send_is_still_pending(cx: &mut TestAppContext) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()), ws_props(), ws_hooks(), true);
    h.fill("keep going");
    h.click("chat-send");
    assert!(h.has("chat-turn-elapsed"));
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("here is the reply", cx);
        h.turn_end(103_000, cx);
    });
    assert!(h.has("turn-end"));
    assert!(h.assistant("here is the reply"));
    assert!(!h.has("chat-turn-elapsed"));
}

fn bodies(h: &mut H) -> Vec<String> {
    h.read(|c| c.props.messages.iter().map(|m| m.body.clone()).collect())
}

#[gpui::test]
async fn leftover_stream_after_turn_end_does_not_leak_the_elapsed_clock_onto_other_chats(cx: &mut TestAppContext) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()), ws_props(), ws_hooks(), true);
    h.fill("keep going");
    h.click("chat-send");
    assert!(h.has("chat-turn-elapsed"));
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("here is the reply", cx);
        h.turn_end(103_000, cx);
        h.live_text("late leftover chunk", cx);
    });
    assert!(h.has("turn-end"));
    assert!(h.assistant("here is the reply"));
    assert!(!bodies(&mut h).iter().any(|b| b.contains("late leftover chunk")));
    assert!(!h.has("chat-turn-elapsed"));
    assert!(h.has("chat-stop"));

    h.host(|h, cx| h.switch("PA", cx));
    assert!(!h.has("chat-turn-elapsed"));
    h.host(|h, cx| h.switch("MA", cx));
    assert!(h.has("turn-end"));
    assert!(!h.has("chat-turn-elapsed"));
    assert!(!bodies(&mut h).iter().any(|b| b.contains("late leftover chunk")));
}

#[gpui::test]
async fn a_new_engine_turn_after_lost_idle_still_streams_past_the_previous_footer(cx: &mut TestAppContext) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()), ws_props(), ws_hooks(), true);
    h.fill("keep going");
    h.click("chat-send");
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("here is the reply", cx);
        h.turn_end(103_000, cx);
        h.live_text("late leftover chunk", cx);
    });
    assert!(h.has("turn-end"));
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("second turn after lost idle", cx);
    });
    assert!(h.has("streaming:second turn after lost idle"));
    assert!(!bodies(&mut h).iter().any(|b| b.contains("late leftover chunk")));
}

#[gpui::test]
async fn idle_reordered_past_the_next_turn_still_lets_that_turn_stream(cx: &mut TestAppContext) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()), ws_props(), ws_hooks(), true);
    h.fill("keep going");
    h.click("chat-send");
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("here is the reply", cx);
        h.turn_end(103_000, cx);
        h.working(cx);
    });
    h.host(|h, cx| h.idle(cx));
    h.host(|h, cx| h.live_text("second turn after reordered idle", cx));
    assert!(h.has("streaming:second turn after reordered idle"));
    assert!(h.has("turn-end"));
}

#[gpui::test]
async fn a_reordered_idle_still_drops_sticky_send_so_a_queued_follow_up_can_flush(cx: &mut TestAppContext) {
    let sent = calls();
    let opts = bridge_opts(Store::default(), sent.clone(), Pending::default(), |n, _| (n > 1).then(|| "ok".to_string()));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.fill("keep going");
    h.click("chat-send");
    assert!(h.has("chat-stop"));
    h.fill("follow up next");
    h.click("chat-send");
    assert!(h.has("queued:follow up next"));
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("here is the reply", cx);
        h.turn_end(103_000, cx);
        h.working(cx);
    });
    h.host(|h, cx| h.idle(cx));
    assert_eq!(texts(&sent), ["keep going", "follow up next"]);
    assert!(!h.has("queued:follow up next"));
}

#[gpui::test]
async fn a_reply_landing_after_idle_flushed_the_queue_does_not_repaint_under_the_follow_up(cx: &mut TestAppContext) {
    let (store, sent, hold) = (Store::default(), calls(), Pending::default());
    let hold2: Pending = Default::default();
    let (hs, h2) = (hold.clone(), hold2.clone());
    let opts = bridge_opts(store.clone(), sent.clone(), Pending::default(), |_, _| None).resolver("SendWorkspaceMessage", {
        let (sent, store) = (sent.clone(), store.clone());
        move |a, _, _| {
            let n = {
                let mut s = sent.lock();
                s.push(a.to_vec());
                s.len()
            };
            store.lock().entry("MA".into()).or_default().push(json!({"role": "user", "kind": "text", "text": a[2]}));
            let (r, release) = Reply::deferred();
            if n == 1 { hs.lock().push(release) } else { h2.lock().push(release) }
            r
        }
    });
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.fill("first turn");
    h.click("chat-send");
    h.fill("queued follow-up");
    h.click("chat-send");
    assert!(h.has("queued:queued follow-up"));
    store.lock().entry("MA".into()).or_default().push(json!({"role": "assistant", "kind": "text", "text": "first reply text"}));
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("first reply text", cx);
    });
    let count = |h: &mut H| bodies(h).iter().filter(|b| b.as_str() == "first reply text").count();
    assert_eq!(count(&mut h), 1);
    h.host(|h, cx| h.idle(cx));
    assert_eq!(texts(&sent), ["first turn", "queued follow-up"]);
    if let Some(r) = hold.lock().pop() {
        r.send(Reply::value(json!("first reply text")));
    }
    h.wait(300);
    assert_eq!(count(&mut h), 1);
}

#[gpui::test]
async fn an_autonomous_turn_after_idle_still_streams_past_the_previous_footer(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, hang_opts(store.clone(), calls()), ws_props(), ws_hooks(), true);
    h.fill("kick off");
    h.click("chat-send");
    assert!(h.has("chat-turn-elapsed"));
    h.host(|h, cx| {
        h.live_text("first reply", cx);
        h.turn_end(1200, cx);
    });
    store.lock().entry("MA".into()).or_default().extend([
        json!({"role": "assistant", "kind": "text", "text": "first reply"}),
        json!({"role": "assistant", "kind": "turn", "durationMs": 1200, "ts": now_ms()}),
    ]);
    h.host(|h, cx| h.idle(cx));
    assert!(h.has("turn-end"));
    assert!(!h.has("chat-turn-elapsed"));
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("planning the next slice", cx);
    });
    assert!(h.has("streaming:planning the next slice"));
    assert!(h.has("chat-turn-elapsed"));
}

#[gpui::test]
async fn a_second_idle_resyncs_a_stream_that_never_got_turn_end_after_a_footer(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, hang_opts(store.clone(), calls()), ws_props(), ws_hooks(), true);
    h.fill("kick off");
    h.click("chat-send");
    h.host(|h, cx| {
        h.working(cx);
        h.live_text("first reply", cx);
        h.turn_end(1200, cx);
        h.working(cx);
        h.live_text("second turn still streaming", cx);
    });
    store.lock().entry("MA".into()).or_default().extend([
        json!({"role": "assistant", "kind": "text", "text": "first reply"}),
        json!({"role": "assistant", "kind": "turn", "durationMs": 1200, "ts": now_ms()}),
        json!({"role": "assistant", "kind": "text", "text": "second turn still streaming"}),
    ]);
    assert!(h.has("streaming:second turn still streaming"));
    h.host(|h, cx| h.idle(cx));
    assert!(h.has("streaming:second turn still streaming"));
    h.host(|h, cx| h.idle(cx));
    assert!(!h.has("streaming:second turn still streaming"));
    assert!(h.assistant("second turn still streaming"));
}

// ---- Chat code fence copy ----

#[gpui::test]
async fn fenced_code_in_a_settled_assistant_reply_has_a_copy_control(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(json!([{"role": "assistant", "kind": "text", "text": "here is a snippet:\n\n```ts\nconst n = 1\n```\n"}])));
    assert!(h.has("md-fence"));
    assert!(!h.has("md-copy"));
    h.hover("md-fence");
    assert!(h.has("md-copy"));
    h.click("md-copy");
    assert_eq!(h.cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("const n = 1"));
}

#[gpui::test]
async fn user_messages_have_a_copy_control(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(json!([
        {"role": "user", "kind": "text", "text": "please copy this prompt"},
        {"role": "assistant", "kind": "text", "text": "ok"},
    ])));
    assert!(h.me("please copy this prompt"));
    h.hover("me:please copy this prompt");
    h.click("chat-bubble-copy");
    assert_eq!(h.cx.read_from_clipboard().and_then(|c| c.text()).as_deref(), Some("please copy this prompt"));
    assert!(h.read(|c| c.copied_bubble.is_some()));
}

// ---- Chat agent error retry ----

const SPEND: &str = "Agent error: You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit";

#[gpui::test]
async fn spend_limit_error_renders_a_card_with_retry_that_re_sends_the_last_prompt(cx: &mut TestAppContext) {
    let sent = calls();
    let opts = bridge_opts(Store::default(), sent.clone(), Pending::default(), |n, _| Some(if n == 1 { SPEND.into() } else { "ok, continuing".into() }));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.fill("ship the dark mode polish");
    h.click("chat-send");
    assert!(h.has("chat-error:Usage limit"));
    let me_rows = |h: &mut H| h.read(|c| c.props.messages.iter().filter(|m| m.me && m.body == "ship the dark mode polish").count());
    assert_eq!(me_rows(&mut h), 1);
    h.click("chat-error-retry");
    assert!(h.assistant("ok, continuing"));
    let displays: Vec<String> = sent.lock().iter().map(|a| a[7].as_str().unwrap_or_default().to_string()).collect();
    assert_eq!(displays, ["", "\u{2060}"]);
    assert_eq!(texts(&sent), ["ship the dark mode polish", "\u{2060}"]);
    assert_eq!(me_rows(&mut h), 1);
}

#[gpui::test]
async fn retry_shows_the_elapsed_clock_before_the_first_stream_row(cx: &mut TestAppContext) {
    let opts = bridge_opts(Store::default(), calls(), Pending::default(), |n, _| (n == 1).then(|| SPEND.to_string()));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.fill("ship the dark mode polish");
    h.click("chat-send");
    assert!(h.has("chat-error-retry"));
    h.host(|h, cx| h.turn_end(1200, cx));
    assert!(h.has("turn-end"));
    h.click("chat-error-retry");
    assert!(h.has("chat-turn-elapsed"));
}
