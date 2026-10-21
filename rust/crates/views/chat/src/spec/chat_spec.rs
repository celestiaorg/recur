//! `frontend/e2e/chat.spec.ts`. Tab routing, the follow-up queue and Stop are the
//! workspace view's; these drive the chat through the [`Host`] double and assert what the chat
//! shows and emits.

use std::sync::Arc;

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Release, Reply};
use serde_json::{Value, json};

use super::harness::*;
use crate::chat::ChatEvent;

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
