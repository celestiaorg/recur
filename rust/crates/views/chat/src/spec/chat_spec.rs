//! `frontend/e2e/chat.spec.ts`. Tab routing, the follow-up queue and Stop are the
//! workspace view's; these drive the chat through the [`Host`] double and assert what the chat
//! shows and emits.

use std::sync::Arc;

use gpui::TestAppContext;
use gpui_kit::AppContext as _;
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
    let chat = h.chat();
    let focused = h.cx.update(|window, cx| chat.read(cx).focus.is_focused(window));
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

// ---- @ file mentions ----

fn files_opts(files: &'static [&'static str], sent: Calls) -> MockOptions {
    let store = Store::default();
    bridge_opts(store, sent, Pending::default(), |_, _| Some("ok".into()))
        .resolver("ListFiles", move |_, _, _| Reply::value(json!(files)))
}

fn chips(h: &mut H) -> usize {
    h.read(|c| c.doc.chips.len())
}

/// Type and let the 200ms search debounce run.
fn type_wait(h: &mut H, s: &str) {
    h.type_text(s);
    h.wait(250);
}

#[gpui::test]
async fn typing_at_inserts_deletable_inline_chips_in_the_composer_sentence(cx: &mut TestAppContext) {
    let sent = calls();
    let skills = json!([
        {"name": "design-audit", "description": "Audit UI polish", "scope": "workspace", "userInvocable": true},
        {"name": "land-it", "description": "Ship the PR", "scope": "workspace", "userInvocable": true},
    ]);
    let (s1, s2) = (skills.clone(), skills);
    let opts = files_opts(&["PROGRESS.md", "DECISIONS.md", ".gitignore", "frontend/src/lib/agentchat.ts"], sent.clone())
        .resolver("ListSkills", move |_, _, _| Reply::value(s1.clone()))
        .resolver("ListAllSkills", move |_, _, _| Reply::value(s2.clone()));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.focus_composer();
    type_wait(&mut h, "how does @");
    assert!(h.has("chat-mention"));
    assert!(h.has("menu-row-on:0"));
    h.keys("down");
    assert!(h.has("menu-row-on:1"));
    assert!(h.has("menu-row:0"));

    type_wait(&mut h, "PROG");
    assert_eq!(chips(&mut h), 0);
    assert!(h.has("mention:PROGRESS.md"));
    h.keys("enter");
    assert!(h.has("chip:PROGRESS.md"));
    assert!(h.lines()[0].starts_with("how does"));

    type_wait(&mut h, "compare to @DEC");
    assert!(h.has("mention:DECISIONS.md"));
    h.keys("enter");
    assert!(h.has("chip:DECISIONS.md"));
    assert_eq!(chips(&mut h), 2);

    h.click("chip-x:0");
    assert!(!h.has("chip:PROGRESS.md"));
    assert!(h.has("chip:DECISIONS.md"));

    h.fill("");
    type_wait(&mut h, "check @.git");
    assert!(h.has("mention:.gitignore"));
    h.keys("enter");
    assert!(h.has("chip:.gitignore"));

    h.fill("");
    type_wait(&mut h, "how does @PROG");
    h.keys("enter");
    type_wait(&mut h, "compare to @DEC");
    h.keys("enter");
    h.click("chat-send");
    assert_eq!(texts(&sent), ["how does @PROGRESS.md compare to @DECISIONS.md"]);
    assert!(h.has("file-inline:PROGRESS.md"));
    assert!(h.has("file-inline:DECISIONS.md"));
}

#[gpui::test]
async fn mention_menu_keeps_lower_ranked_matches_past_the_old_top_12_cap(cx: &mut TestAppContext) {
    let mut files: Vec<String> = (0..15).map(|i| format!("a{i:02}/match.md")).collect();
    files.push("frontend/src/lib/deeply/nested/path/match-util.ts".into());
    let opts = MockOptions::default().resolver("ListFiles", move |_, _, _| Reply::value(json!(files)));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.focus_composer();
    type_wait(&mut h, "@match");
    assert!(h.has("chat-mention"));
    assert_eq!(h.read(|c| c.mention_hits.len()), 16);
    assert!(h.has("menu-row:15"));
    assert!(h.has("mention:match-util.ts"));
}

#[gpui::test]
async fn directory_hits_appear_in_at_search_with_a_folder_icon(cx: &mut TestAppContext) {
    let opts = MockOptions::default().resolver("ListFiles", |_, _, _| Reply::value(json!(["src/lib/a.ts", "src/app.ts", "README.md"])));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.focus_composer();
    type_wait(&mut h, "@src");
    assert!(h.has("chat-mention"));
    assert!(h.has("mention-folder"));
    h.click("mention:src");
    assert!(h.has("chip-folder"));
    assert!(h.has("chip:src"));
    h.click("chip:src");
    let toasts = h.cx.update(|_, cx| recur_ui::widgets::ToastStore::global(cx).read(cx).toasts().len());
    assert_eq!(toasts, 0);
    assert!(h.has("chip:src"));
}

#[gpui::test]
async fn at_hyp_lists_both_hyperlane_validator_scripts(cx: &mut TestAppContext) {
    let opts = MockOptions::default().resolver("ListFiles", |_, _, _| {
        Reply::value(json!(["validators/decode-hyperlane-message.sh", "validators/hyperlane_validator2.sh", "hypothesis.txt"]))
    });
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.focus_composer();
    type_wait(&mut h, "@hyp");
    assert!(h.has("chat-mention"));
    assert!(h.has("mention:decode-hyperlane-message.sh"));
    assert!(h.has("mention:hyperlane_validator2.sh"));
}

#[gpui::test]
async fn escape_dismisses_the_mention_menu_without_reopening_on_keyup(cx: &mut TestAppContext) {
    let opts = MockOptions::default().resolver("ListFiles", |_, _, _| Reply::value(json!(["PROGRESS.md", "DECISIONS.md"])));
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.focus_composer();
    type_wait(&mut h, "@PRO");
    assert!(h.has("chat-mention"));
    h.keys("escape");
    assert!(!h.has("chat-mention"));
    h.wait(50);
    assert!(!h.has("chat-mention"));
    type_wait(&mut h, "G");
    assert!(h.has("chat-mention"));
}

fn opened(h: &H) -> Vec<String> {
    h.events()
        .into_iter()
        .filter_map(|e| match e {
            ChatEvent::OpenFile { path, .. } => Some(path),
            _ => None,
        })
        .collect()
}

async fn transcript_chip_opens(cx: &mut TestAppContext, mention: &str, name: &str, path: &str) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(json!([{"role": "user", "kind": "text", "text": format!("please look at @{mention}")}])));
    h.click(&format!("file-inline:{name}"));
    assert_eq!(opened(&h), [path]);
}

#[gpui::test]
async fn transcript_file_chip_opens_the_editor_without_stealing_the_files_panel(cx: &mut TestAppContext) {
    transcript_chip_opens(cx, "src/main.go", "main.go", "src/main.go").await;
}

#[gpui::test]
async fn transcript_file_chip_keeps_source_control_open_instead_of_flipping_to_outline(cx: &mut TestAppContext) {
    transcript_chip_opens(cx, "src/main.go", "main.go", "src/main.go").await;
}

#[gpui::test]
async fn transcript_file_chip_does_not_select_a_file_that_is_filtered_out_of_the_explorer(cx: &mut TestAppContext) {
    transcript_chip_opens(cx, "PROGRESS.md", "PROGRESS.md", "PROGRESS.md").await;
}

fn drop_explorer(h: &mut H, paths: &[&str]) {
    let chat = h.chat();
    let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| {
        c.file_drop_over = true;
        c.drop_explorer_paths(&paths, window, cx)
    }));
    h.settle();
}

#[gpui::test]
async fn dragging_a_file_from_the_explorer_inserts_an_at_file_chip(cx: &mut TestAppContext) {
    let sent = calls();
    let mut h = boot(cx, files_opts(&[], sent.clone()), ws_props(), ws_hooks(), true);
    h.focus_composer();
    h.type_text("please review ");
    drop_explorer(&mut h, &["src/main.go"]);
    assert!(!h.read(|c| c.file_drop_over));
    assert!(h.has("chip:main.go"));
    assert!(h.lines()[0].starts_with("please review"));
    h.click("chat-send");
    assert_eq!(texts(&sent), ["please review @src/main.go"]);
    assert!(h.has("file-inline:main.go"));
}

#[gpui::test]
async fn dragging_a_folder_from_the_explorer_inserts_a_folder_chip(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.focus_composer();
    drop_explorer(&mut h, &["src/"]);
    assert!(h.has("chip-folder"));
    assert!(h.has("chip:src"));
}

fn os_drop(h: &mut H, path: &str) {
    let at = h.bounds("chat").expect("chat").center();
    let paths = gpui::ExternalPaths(smallvec::smallvec![std::path::PathBuf::from(path)]);
    h.cx.simulate_event(gpui::FileDropEvent::Entered { position: at, paths });
    h.cx.simulate_event(gpui::FileDropEvent::Submit { position: at });
    h.settle();
}

#[gpui::test]
async fn dropping_a_file_uri_onto_chat_inserts_an_at_file_chip(cx: &mut TestAppContext) {
    let sent = calls();
    let mut h = boot(cx, files_opts(&[], sent.clone()), ws_props(), ws_hooks(), true);
    h.focus_composer();
    os_drop(&mut h, &format!("{}/src/main.go", root_path()));
    assert!(h.has("chip:main.go"));
    h.click("chat-send");
    assert_eq!(texts(&sent), ["@src/main.go"]);
}

#[gpui::test]
async fn dropping_an_external_image_attaches_it_like_paste(cx: &mut TestAppContext) {
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
        .unwrap();
    let dir = std::env::temp_dir().join(format!("recur-chat-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let shot = dir.join("shot.png");
    std::fs::write(&shot, png).unwrap();
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.focus_composer();
    let id = recur_view_core::file_drag::encode_chat_drop_id(&format!("{WS}:MA"));
    h.bridge
        .dispatch_wire("files.dropped", json!({"files": [shot.to_string_lossy()], "attributes": {"data-chat-drop": id}}))
        .unwrap();
    h.settle();
    assert!(h.has("chip-att:0"));
    assert_eq!(h.read(|c| c.attachments.len()), 1);
    assert!(!h.read(|c| c.doc.chips.iter().any(|s| matches!(s, recur_engine::view::composer::Segment::File { .. }))));
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
async fn clicking_an_external_file_chip_opens_it_in_the_editor(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.focus_composer();
    os_drop(&mut h, "/Users/josh/Downloads/export-111.csv");
    assert!(h.has("chip:export-111.csv"));
    h.click("chip:export-111.csv");
    assert_eq!(opened(&h), ["/Users/josh/Downloads/export-111.csv"]);
}

// ---- @terminal / @artifact mentions ----

#[gpui::test]
async fn typing_at_lists_terminals_and_inserts_a_chip(cx: &mut TestAppContext) {
    let sent = calls();
    let opts = files_opts(&["README.md"], sent.clone()).resolver("ListTerminals", |_, _, _| {
        Reply::value(json!([{"id": "term-dev-1", "workspaceId": WS, "title": "dev", "cwd": "/mock", "shell": "/bin/zsh", "createdAt": "2026-08-24T12:00:00Z"}]))
    });
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.focus_composer();
    type_wait(&mut h, "errors in @term");
    assert!(h.has("mention:dev"));
    h.keys("enter");
    assert!(h.has("chip:dev"));
    h.click("chat-send");
    assert!(texts(&sent)[0].contains("@terminal:term-dev-1"));
}

#[gpui::test]
async fn typing_at_lists_artifacts_and_inserts_a_chip(cx: &mut TestAppContext) {
    let sent = calls();
    let opts = files_opts(&["README.md"], sent.clone()).resolver("ListArtifacts", |_, _, _| {
        Reply::value(json!([{
            "id": "art-notes-1", "title": "Notes", "kind": "md",
            "path": "workspaces/w/artifacts/art-notes-1.md", "createdAt": "2026-08-24T12:00:00Z",
        }]))
    });
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.focus_composer();
    type_wait(&mut h, "review @Notes");
    assert!(h.has("mention:Notes"));
    h.keys("enter");
    assert!(h.has("chip:Notes"));
    h.click("chat-send");
    assert!(texts(&sent)[0].contains("@artifact:art-notes-1"));
}

// ---- Chat tab persistence ----
// The strip and its saved state are the workspace view's; these port the rules it applies.

use recur_engine::view::chat_tab_session::{ACTIVE_CHAT_TAB_KEY, ActiveChatTabs};
use recur_view_core::LocalStorage;

fn reload(storage: &LocalStorage) -> ActiveChatTabs {
    ActiveChatTabs::parse(storage.get(ACTIVE_CHAT_TAB_KEY))
}

#[test]
fn reopening_a_workspace_after_reload_restores_the_last_active_chat_tab() {
    let mut storage = LocalStorage::in_memory();
    let mut tabs = reload(&storage);
    assert_eq!(tabs.restored_chat_code(WS, &["MA", "PA"]), "MA");
    tabs.save(WS, "PA");
    storage.set(ACTIVE_CHAT_TAB_KEY, tabs.encode().unwrap());
    assert_eq!(reload(&storage).restored_chat_code(WS, &["MA", "PA"]), "PA");
}

#[test]
fn a_newly_opened_chat_tab_stays_selected_after_reload() {
    let mut storage = LocalStorage::in_memory();
    let mut tabs = reload(&storage);
    tabs.save(WS, "PA2");
    storage.set(ACTIVE_CHAT_TAB_KEY, tabs.encode().unwrap());
    assert_eq!(reload(&storage).restored_chat_code(WS, &["MA", "PA", "PA2"]), "PA2");
}

#[gpui::test]
async fn dragging_a_chat_tab_reorders_the_strip_and_survives_reload(cx: &mut TestAppContext) {
    use recur_ui::widgets::tab_strip::{reorder_by_insert, restore_strip_order};
    let saved = calls();
    let s2 = saved.clone();
    let opts = MockOptions::default().resolver("ReorderWorkspaceChats", move |a, s, _| {
        s2.lock().push(a.to_vec());
        s.chat_order_by_loop.insert(a[0].as_str().unwrap().into(), serde_json::from_value(a[1].clone()).unwrap());
        Reply::null()
    });
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), false);
    let strip = ["MA", "PA", "PA2"];
    let next = reorder_by_insert(&strip, 1, 0);
    assert_eq!(next, ["PA", "MA", "PA2"]);
    let codes: Vec<String> = next.iter().map(|s| s.to_string()).collect();
    let services = h.bridge.services();
    h.cx.update(|_, cx| cx.background_spawn(async move { services.workspaces.reorder_workspace_chats(WS, &codes).await }))
        .detach();
    h.settle();
    assert_eq!(saved.lock()[0][1], json!(["PA", "MA", "PA2"]));
    // After reload the server order wins over a stale live order.
    assert_eq!(restore_strip_order(&strip, &next, |s| *s), ["PA", "MA", "PA2"]);
    let order = h.bridge.with_seed(|s| s.chat_order_by_loop.get(WS).cloned());
    assert_eq!(order.unwrap(), ["PA", "MA", "PA2"]);
}

#[test]
fn switching_workspaces_restores_agent_tab_strip_scroll() {
    use recur_ui::widgets::tab_strip::next_strip_scroll_left;
    // A restored offset stays put when the active chip is in view...
    assert_eq!(next_strip_scroll_left(120., 300., 1000., 200., 100.), 120.);
    // ...and selecting the last chip scrolls it into view.
    let left = next_strip_scroll_left(120., 300., 1000., 900., 100.);
    assert!(900. >= left && 1000. <= left + 300.);
}

// ---- Chat transcript UI state ----

fn tool_items() -> Value {
    json!([
        {"role": "user", "kind": "text", "text": "run the check"},
        {"role": "assistant", "kind": "tool", "tool": "Bash", "toolInput": "python3 -m py_compile tetris.py",
         "description": "Compile check", "input": "{\"command\":\"python3 -m py_compile tetris.py && echo OK\"}", "result": "OK"},
    ])
}

fn tall(n: usize, line: &str, para: &str) -> Value {
    let mut v = Vec::new();
    for i in 0..n {
        v.push(json!({"role": "user", "kind": "text", "text": format!("question {i}\n{line}")}));
        v.push(json!({"role": "assistant", "kind": "text", "text": format!("answer {i}\n{para}")}));
    }
    Value::Array(v)
}

fn tall_transcript() -> Value {
    tall(20, &"line ".repeat(6), &"paragraph ".repeat(18))
}

fn windowed_transcript() -> Value {
    tall((crate::window::CHAT_HYDRATE_ALL_BELOW + 4).div_ceil(2), &"line ".repeat(6), &"paragraph ".repeat(18))
}

fn tool_head(key: &str) -> String {
    format!("tc-head:tc-{key}-1")
}

fn tool_open(h: &mut H) -> bool {
    h.read(|c| c.open_rows.get("1").copied().unwrap_or(false))
}

#[gpui::test]
async fn expanded_tool_calls_stay_expanded_after_switching_chat_tabs(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(tool_items()));
    let head = tool_head(&format!("{WS}:MA"));
    assert!(h.has(&head));
    assert!(!tool_open(&mut h));
    h.click(&head);
    assert!(tool_open(&mut h));
    h.host(|h, cx| h.switch("PA", cx));
    assert!(!h.has(&head));
    h.host(|h, cx| h.switch("MA", cx));
    assert!(h.has(&head));
    assert!(tool_open(&mut h));
}

#[gpui::test]
async fn expanded_tool_calls_stay_expanded_after_switching_workspaces(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(tool_items()));
    let head = tool_head(&format!("{WS}:MA"));
    h.click(&head);
    assert!(tool_open(&mut h));
    h.host(|h, cx| h.switch_ws("LOOP-2229", cx));
    assert!(!h.has(&head));
    h.host(|h, cx| h.switch_ws(WS, cx));
    assert!(h.has(&head));
    assert!(tool_open(&mut h));
}

/// `leaveMidScroll`: an unpinned offset that survives a frame.
fn leave_mid_scroll(h: &mut H, top: f32) -> f32 {
    assert!(h.max_scroll() > top + 80.);
    h.scroll_to(top);
    h.settle();
    assert!(!h.read(|c| c.near_bottom));
    assert!(h.dist_from_bottom() > 80.);
    h.scroll_top()
}

fn assert_restored_mid(h: &mut H, saved: f32) {
    let top = h.scroll_top();
    assert!((top - saved).abs() < 80., "restored {top} vs saved {saved}");
    assert!(h.dist_from_bottom() > 80.);
}

#[gpui::test]
async fn scroll_offset_is_restored_after_switching_chat_tabs(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(tall_transcript()));
    assert!(h.me("question 0"));
    let saved = leave_mid_scroll(&mut h, 360.);
    h.host(|h, cx| h.switch("PA", cx));
    assert!(!h.me("question 0"));
    h.host(|h, cx| h.switch("MA", cx));
    assert!(h.me("question 0"));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn scroll_offset_is_restored_after_switching_tabs_on_a_windowed_transcript(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(windowed_transcript()));
    assert!(h.read(|c| c.window_chat()));
    let saved = leave_mid_scroll(&mut h, 360.);
    h.host(|h, cx| h.switch("PA", cx));
    assert!(!h.read(|c| c.window_chat()));
    h.host(|h, cx| h.switch("MA", cx));
    assert!(h.read(|c| c.window_chat()));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn scroll_offset_is_restored_after_hiding_the_agent_panel_on_a_windowed_transcript(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(windowed_transcript()));
    let saved = leave_mid_scroll(&mut h, 360.);
    h.remount();
    assert!(h.read(|c| c.window_chat()));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn a_small_scroll_up_from_the_tail_is_not_pulled_back_on_a_windowed_transcript(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(tall(
        (crate::window::CHAT_HYDRATE_ALL_BELOW + 4).div_ceil(2),
        &"line\n".repeat(8),
        &"paragraph\n".repeat(16),
    )));
    assert!(h.read(|c| c.window_chat()));
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
    let top = h.scroll_top();
    h.scroll_to((top - 40.).max(0.));
    h.wait(50);
    assert!(h.dist_from_bottom() > 20.);
}

#[gpui::test]
async fn scroll_offset_is_restored_after_switching_workspaces(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(tall_transcript()));
    let saved = leave_mid_scroll(&mut h, 360.);
    h.host(|h, cx| h.switch_ws("LOOP-2229", cx));
    assert!(!h.me("question 0"));
    h.host(|h, cx| h.switch_ws(WS, cx));
    assert!(h.me("question 0"));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn parked_workspace_chat_stays_live_and_only_its_owner_reloads_on_idle(cx: &mut TestAppContext) {
    let reloads = calls();
    let r2 = reloads.clone();
    let text = Arc::new(Mutex::new("initial owned transcript".to_string()));
    let t2 = text.clone();
    let opts = MockOptions::default().resolver("WorkspaceChatItems", move |a, _, _| {
        r2.lock().push(a.to_vec());
        Reply::value(json!([{"role": "assistant", "kind": "text", "text": t2.lock().clone()}]))
    });
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.set_messages(items(json!([{"role": "assistant", "kind": "text", "text": "initial owned transcript"}])));
    assert!(h.assistant("initial owned transcript"));
    // Parked: hidden but still mounted and fed.
    h.host(|h, cx| {
        h.base.presentation_active = false;
        h.working(cx);
        h.live_text("parked live reply", cx);
    });
    assert!(bodies(&mut h).iter().any(|b| b == "parked live reply"));
    *text.lock() = "settled owned transcript".into();
    reloads.lock().clear();
    h.host(|h, cx| h.idle(cx));
    h.host(|h, cx| {
        h.base.presentation_active = true;
        h.push(cx);
    });
    assert!(h.assistant("settled owned transcript"));
    let got: Vec<Vec<Value>> = reloads.lock().clone();
    assert_eq!(got, [vec![json!(WS), json!("MA")]]);
}

#[gpui::test]
async fn a_bottom_pinned_in_flight_chat_stays_on_the_tail_after_growth_while_parked(cx: &mut TestAppContext) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()), ws_props(), ws_hooks(), true);
    h.set_messages(items(tall_transcript()));
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
    h.fill("keep going");
    h.click("chat-send");
    assert!(h.has("chat-stop"));
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
    h.host(|h, cx| h.switch_ws("LOOP-2229", cx));
    // The parked tab grows while another workspace shows.
    h.host(|h, cx| {
        let key = format!("{WS}:MA");
        let grown = format!("grown-tail {}", "paragraph ".repeat(40));
        let tab = h.tabs.get_mut(&key).unwrap();
        tab.msgs = recur_engine::view::chat::apply_stream(
            &tab.msgs,
            recur_engine::view::chat::StreamKind::Text,
            &grown,
            &Default::default(),
        );
        h.push(cx);
    });
    h.host(|h, cx| h.switch_ws(WS, cx));
    assert!(h.has("streaming:grown-tail paragraph paragraph paragraph paragraph paragraph"));
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
}

#[gpui::test]
async fn a_bottom_pinned_transcript_stays_pinned_after_switching_chat_tabs(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(tall_transcript()));
    assert!(h.max_scroll() > 400.);
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
    h.host(|h, cx| h.switch("PA", cx));
    h.host(|h, cx| h.switch("MA", cx));
    assert!(h.me("question 0"));
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
}

#[gpui::test]
async fn a_mid_scrolled_shorter_tab_is_not_pinned_when_entered_from_a_longer_tab(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    let long = items(tall_transcript());
    let short: Vec<_> = long[..24].to_vec();
    h.set_messages(long);
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
    h.host(|h, cx| h.switch("PA", cx));
    h.set_messages(short);
    assert!(h.me("question 0"));
    let saved = leave_mid_scroll(&mut h, 320.);
    h.host(|h, cx| h.switch("MA", cx));
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
    h.host(|h, cx| h.switch("PA", cx));
    assert!(h.me("question 0"));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn transcript_fades_an_edge_only_while_more_content_sits_past_it(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(tall_transcript()));
    assert!(h.max_scroll() > 400.);
    assert!(h.dist_from_bottom() < crate::transcript_ui::NEAR_BOTTOM_PX);
    assert!(h.has("fade-top") && !h.has("fade-bot"));
    h.scroll_to(0.);
    assert!(!h.has("fade-top") && h.has("fade-bot"));
    leave_mid_scroll(&mut h, 360.);
    assert!(h.has("fade-top") && h.has("fade-bot"));
}

// ---- Turn-end findings digest ----

fn finding_transcript(footer: bool) -> Value {
    let input = json!({"file": "store.go", "line": 1276, "end_line": 1286, "severity": "high",
        "message": "activity hook can miss a turn", "summary": "Hook misses turn"}).to_string();
    let result = json!({"id": "f-hook", "file": "store.go", "line": 1276, "endLine": 1286, "severity": "high",
        "message": "activity hook can miss a turn", "summary": "Hook misses turn", "status": "open"}).to_string();
    let mut v = vec![
        json!({"role": "user", "kind": "text", "text": "review this"}),
        json!({"role": "assistant", "kind": "tool", "tool": "mcp__recur__report_finding", "input": input, "result": result}),
        json!({"role": "assistant", "kind": "text", "text": "Filed 1 finding."}),
    ];
    if footer {
        v.push(json!({"role": "assistant", "kind": "turn", "durationMs": 8400}));
    }
    Value::Array(v)
}

#[gpui::test]
async fn renders_the_findings_summary_widget_after_a_codex_style_report_finding_turn(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(finding_transcript(true)));
    assert!(h.has("turn-end"));
    assert!(h.has("turn-end-link:1 finding"));
    assert!(h.has("findings-filed"));
    let digest = h.read(|c| c.turn_digest_visible(&c.props.messages, c.props.messages.len() - 1));
    assert_eq!(digest[0].message, "activity hook can miss a turn");
}

#[gpui::test]
async fn does_not_render_the_findings_widget_when_the_turn_footer_is_missing(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(finding_transcript(false)));
    assert!(h.assistant("Filed 1 finding."));
    assert!(!h.has("findings-filed"));
}

// ---- Chat fork ----

fn fork_transcript() -> Value {
    json!([
        {"role": "user", "kind": "text", "text": "ship it"},
        {"role": "assistant", "kind": "text", "text": "done"},
        {"role": "assistant", "kind": "turn", "durationMs": 1200, "messageId": "msg-turn-1"},
    ])
}

#[gpui::test]
async fn fork_control_on_a_finished_turn_opens_a_new_chat_tab(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(fork_transcript()));
    assert!(h.has("turn-end"));
    h.click("turn-end-fork");
    h.click("fork-item:Fork chat");
    assert!(h.events().contains(&ChatEvent::Fork("msg-turn-1".into())));
    assert!(!h.has("fork-item:Fork chat"));
}

#[gpui::test]
async fn fork_to_new_workspace_opens_the_created_workspace(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks(), true);
    h.set_messages(items(fork_transcript()));
    h.click("turn-end-fork");
    h.click("fork-item:Fork to new workspace");
    assert!(h.events().contains(&ChatEvent::ForkToWorkspace("msg-turn-1".into())));
}

// ---- External markdown links ----

#[gpui::test]
async fn http_links_in_agent_output_open_in_the_os_browser(cx: &mut TestAppContext) {
    let opts = bridge_opts(Store::default(), calls(), Pending::default(), |_, _| {
        Some("See [the docs](https://example.com/guide) for details.".into())
    });
    let mut h = boot(cx, opts, ws_props(), ws_hooks(), true);
    h.fill("link please");
    h.click("chat-send");
    assert!(h.assistant("See [the docs](https://example.com/guide) for details."));
    let chat = h.chat();
    let at = h.cx.update(|window, cx| {
        let chat = chat.read(cx);
        chat.markdown.values().find_map(|v| {
            let v = v.read(cx);
            v.text_blocks().into_iter().find_map(|(id, text)| {
                let start = text.find("the docs")?;
                v.range_bounds(id, start..start + "the docs".len(), window)
            })
        })
    });
    let at = at.expect("link bounds").center();
    h.cx.simulate_click(at, gpui::Modifiers::none());
    h.settle();
    assert_eq!(h.cx.opened_url().as_deref(), Some("https://example.com/guide"));
}
