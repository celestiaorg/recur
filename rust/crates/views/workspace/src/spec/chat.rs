//! `frontend/e2e/chat.spec.ts` on the real workspace view: the chat dock in the Observatory's
//! left panel, its tabs, send / queue / steer / Stop / Retry routing, prompt actions, live
//! events, the transcript UI cache and the tab strip's persistence.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Modifiers, TestAppContext, point, px};
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Release, Reply, Seed};
use serde_json::{Value, json};

use recur_view_core::prefs::CodePanelKey;

use super::harness::*;

type Pending = Arc<Mutex<Vec<Release>>>;

/// `WorkspaceChatItems` over a per-code store.
fn items_opts(store: Store) -> MockOptions {
    MockOptions::default()
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
        .resolver("WorkspaceChatItems", move |a, _, _| {
            if a.first().and_then(Value::as_str) != Some(WS) {
                return Reply::value(json!([]));
            }
            let code = a
                .get(1)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("PA");
            Reply::value(Value::Array(
                store.lock().get(code).cloned().unwrap_or_default(),
            ))
        })
}

/// Sends record the user turn under its display text; `reply` answers (`None` hangs until
/// released through `hold`).
fn bridge_opts(
    store: Store,
    sent: Calls,
    hold: Pending,
    reply: impl Fn(usize, &str) -> Option<String> + Send + Sync + 'static,
) -> MockOptions {
    let s2 = store.clone();
    items_opts(store).resolver("SendWorkspaceMessage", move |a, _, _| {
        let n = {
            let mut s = sent.lock();
            s.push(a.to_vec());
            s.len()
        };
        let code = a[1].as_str().filter(|s| !s.is_empty()).unwrap_or("PA").to_string();
        let text = a[2].as_str().unwrap_or_default().to_string();
        let display = a[7].as_str().unwrap_or_default().to_string();
        if display != "\u{2060}" {
            s2.lock().entry(code.clone()).or_default().push(
                json!({"role": "user", "kind": "text", "text": if display.is_empty() { &text } else { &display }}),
            );
        }
        match reply(n, &text) {
            Some(r) => {
                s2.lock()
                    .entry(code)
                    .or_default()
                    .push(json!({"role": "assistant", "kind": "text", "text": r}));
                Reply::value(json!(r))
            }
            None => {
                let (r, release) = Reply::deferred();
                hold.lock().push(release);
                r
            }
        }
    })
}

fn texts(sent: &Calls) -> Vec<String> {
    sent.lock()
        .iter()
        .map(|a| a[2].as_str().unwrap_or_default().to_string())
        .collect()
}

fn arg(sent: &Calls, i: usize, at: usize) -> Value {
    sent.lock()[i][at].clone()
}

fn release(hold: &Pending, store: &Store, code: &str, reply: &str) {
    store
        .lock()
        .entry(code.into())
        .or_default()
        .push(json!({"role": "assistant", "kind": "text", "text": reply}));
    if let Some(r) = hold.lock().pop() {
        r.send(Reply::value(json!(reply)));
    }
}

/// Put a transcript in the store and pull it into the open tab (`WorkspaceChatItems`).
fn load(h: &mut H, store: &Store, code: &str, items: Value) {
    store.lock().insert(
        code.to_string(),
        items.as_array().cloned().unwrap_or_default(),
    );
    let code = code.to_string();
    h.with(|v, cx| {
        let id = v.workspace_id().to_string();
        v.resync_chat(&id, &code, cx)
    });
}

fn wire(h: &mut H, name: &str, data: Value) {
    h.dispatch(name, data);
}

fn working(h: &mut H, code: &str) {
    wire(
        h,
        "agent.updated",
        json!({"workspaceId": WS, "agentId": format!("{WS}-{code}"), "status": "working"}),
    );
}

fn delta(h: &mut H, code: &str, text: &str, seq: i64) {
    wire(
        h,
        "session.delta",
        json!({"workspaceId": WS, "code": code, "text": text, "seq": seq, "replace": true}),
    );
}

fn turn_end(h: &mut H, code: &str, ms: i64) {
    wire(
        h,
        "session.turn_end",
        json!({"workspaceId": WS, "code": code, "durationMs": ms}),
    );
}

fn idle(h: &mut H, code: &str) {
    h.bridge.simulate_chat_turn_idle(WS, code);
    h.settle();
}

fn send_click(h: &mut H) {
    h.click("chat-send");
}

// ---- Maestro chat auto-scroll ----

fn dist_from_bottom(h: &mut H) -> f32 {
    h.settle();
    let chat = h.chat();
    chat.read_with(h.cx, |c, _| {
        let s = c.test_scroll();
        f32::from(s.max_offset().y) + f32::from(s.offset().y)
    })
}

fn scroll_top(h: &mut H) -> f32 {
    let chat = h.chat();
    chat.read_with(h.cx, |c, _| -f32::from(c.test_scroll().offset().y))
}

fn max_scroll(h: &mut H) -> f32 {
    let chat = h.chat();
    chat.read_with(h.cx, |c, _| f32::from(c.test_scroll().max_offset().y))
}

fn scroll_to(h: &mut H, top: f32) {
    let chat = h.chat();
    h.cx.update(|_, cx| {
        chat.update(cx, |c, cx| {
            c.test_scroll().set_offset(point(px(0.), px(-top)));
            cx.notify();
        })
    });
    h.settle();
}

#[gpui::test]
async fn sending_a_message_scrolls_to_the_bottom(cx: &mut TestAppContext) {
    let opts = bridge_opts(Store::default(), calls(), Pending::default(), |_, t| {
        Some(format!("reply to: {t}"))
    });
    let mut h = boot(cx, opts);
    assert!(h.has("chat-composer"));
    for i in 0..8 {
        h.fill(&format!("message {i}"));
        send_click(&mut h);
        assert!(h.me(&format!("message {i}")), "message {i}");
    }
    assert!(max_scroll(&mut h) > 100.);
    assert!(dist_from_bottom(&mut h) < 4.);
}

#[gpui::test]
async fn scrolling_away_from_the_bottom_keeps_position_when_a_reply_streams_in(
    cx: &mut TestAppContext,
) {
    let (store, hold) = (Store::default(), Pending::default());
    let opts = bridge_opts(store.clone(), calls(), hold.clone(), |_, _| None);
    let mut h = boot(cx, opts);
    let mut prior = Vec::new();
    for i in 0..10 {
        prior.push(json!({"role": "user", "kind": "text", "text": format!("question {i}")}));
        prior.push(json!({"role": "assistant", "kind": "text", "text": format!("answer {i}\n\n{}", "paragraph ".repeat(30))}));
    }
    load(&mut h, &store, "MA", Value::Array(prior));
    h.fill("kick off a slow reply");
    send_click(&mut h);
    scroll_to(&mut h, 0.);
    let before = scroll_top(&mut h);
    release(&hold, &store, "MA", "a slow reply");
    h.settle();
    assert!(h.assistant("a slow reply"));
    assert_eq!(scroll_top(&mut h), before);
}

/// A tab switch draws the incoming transcript where it was left, with the blocks there laid
/// out, in the switch's first frame. A test update draws until the window is clean, so the
/// switch and its first frame run inside one update.
#[gpui::test]
async fn a_tab_switch_draws_the_transcript_where_it_was_left_in_its_first_frame(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    let mut saved = HashMap::new();
    for (code, turns, at) in [("MA", 150, 0.4), ("PA", 200, 0.7)] {
        h.select_tab(code);
        let mut items = Vec::new();
        for i in 0..turns {
            items.push(
                json!({"role": "user", "kind": "text", "text": format!("{code} question {i}")}),
            );
            items.push(json!({"role": "assistant", "kind": "text", "text": format!("answer {i}\n\n{}", "paragraph ".repeat(i % 40))}));
        }
        load(&mut h, &store, code, Value::Array(items));
        let top = (max_scroll(&mut h) * at).round();
        scroll_to(&mut h, top);
        saved.insert(code, scroll_top(&mut h));
    }
    let (view, chat) = (h.view(), h.chat());
    for code in ["MA", "PA", "MA"] {
        let (top, stubs) = h.cx.update(|window, cx| {
            view.update(cx, |v, cx| v.select_chat_tab(code, cx));
            window.draw(cx).clear(cx);
            let c = chat.read(cx);
            (
                -f32::from(c.test_scroll().offset().y),
                c.test_stubs_in_view(),
            )
        });
        assert_eq!(top, saved[code], "{code}: first frame");
        assert!(stubs.is_empty(), "{code}: stubs in view {stubs:?}");
        // Not `settle`: its whole-window refresh redraws what a mouse move would.
        h.cx.run_until_parked();
        assert_eq!(scroll_top(&mut h), saved[code], "{code}: settled");
        // What the settled frame drew is where a full redraw draws it: a restore that moved
        // the scroller after the switch redrew the transcript too.
        let keys = chat.read_with(h.cx, |c, _| c.test_block_keys());
        let drawn = |h: &mut H| -> Vec<_> {
            keys.iter()
                .map(|k| {
                    h.cx.debug_bounds(Box::leak(format!("cblk:{k}").into_boxed_str()))
                })
                .collect()
        };
        let settled = drawn(&mut h);
        h.cx.update(|window, _| window.refresh());
        h.cx.run_until_parked();
        assert_eq!(scroll_top(&mut h), saved[code], "{code}: after a redraw");
        assert!(
            settled == drawn(&mut h),
            "{code}: the settled frame is stale"
        );
    }
}

// ---- Chat follow-up queue ----

#[gpui::test]
async fn all_follow_ups_queued_while_a_turn_is_running_fold_into_the_next_user_turn(
    cx: &mut TestAppContext,
) {
    let (store, sent, hold) = (Store::default(), calls(), Pending::default());
    let opts = bridge_opts(store.clone(), sent.clone(), hold.clone(), |n, t| {
        (n > 1).then(|| format!("reply to: {t}"))
    });
    let mut h = boot(cx, opts);
    h.fill("first turn");
    send_click(&mut h);
    assert!(h.has("chat-stop"));
    h.fill("fix the tests");
    send_click(&mut h);
    h.fill("and update the docs");
    send_click(&mut h);
    assert!(h.has("queued:fix the tests"));
    assert!(h.has("queued:and update the docs"));
    assert_eq!(texts(&sent).len(), 1);

    h.click("queued-x:fix the tests");
    assert!(!h.has("queued:fix the tests"));
    assert!(h.has("queued:and update the docs"));

    release(&hold, &store, "MA", "done with first");
    h.settle();
    let t = texts(&sent);
    assert_eq!(t.len(), 2);
    assert!(t[1].ends_with("and update the docs"), "{t:?}");
    assert!(!h.has("queued:and update the docs"));
    assert!(h.me("and update the docs"));
    assert!(!h.me("fix the tests"));
}

async fn busy_follow_ups(cx: &mut TestAppContext, queue_follow_ups: bool) {
    let (store, sent) = (Store::default(), calls());
    let opts = bridge_opts(store, sent.clone(), Pending::default(), |n, _| {
        (n > 1).then(|| "\u{2060}inject".to_string())
    });
    let mut h = boot_with(
        cx,
        |s: &mut Seed| s.settings["queueFollowUps"] = json!(queue_follow_ups),
        opts,
        |_| {},
        WS,
    );
    h.select_tab("PA");
    h.fill("first turn");
    h.keys("enter");
    assert!(h.has("chat-stop"));
    let (steer, queue) = if queue_follow_ups {
        ("secondary-enter", "enter")
    } else {
        ("enter", "secondary-enter")
    };
    h.fill("steer now");
    h.keys(steer);
    assert_eq!(texts(&sent).len(), 2);
    assert!(texts(&sent)[1].ends_with("steer now"));
    assert!(!h.has("queued:steer now"));

    h.fill("after the turn");
    h.keys(queue);
    assert!(h.has("queued:after the turn"));
    assert_eq!(texts(&sent).len(), 2);

    h.fill("steer past the queue");
    h.keys(steer);
    assert_eq!(texts(&sent).len(), 3);
    assert!(texts(&sent)[2].ends_with("steer past the queue"));
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
    let opts = bridge_opts(store.clone(), sent.clone(), hold.clone(), |n, t| {
        (n > 1).then(|| format!("reply to: {t}"))
    })
    .resolver("CancelWorkspaceMessage", move |_, _, _| {
        release(&h2, &s2, "MA", "Stopped by user");
        Reply::null()
    });
    let mut h = boot(cx, opts);
    h.fill("first turn");
    send_click(&mut h);
    assert!(h.has("chat-stop"));
    h.fill("fix the tests");
    send_click(&mut h);
    h.fill("and update the docs");
    send_click(&mut h);
    assert!(h.has("queued:fix the tests") && h.has("queued:and update the docs"));

    h.click("chat-stop");
    assert!(h.has("chat-stopped"));
    assert!(!h.has("queued:fix the tests") && !h.has("queued:and update the docs"));
    let chat = h.chat();
    let lines: Vec<String> = chat
        .read_with(h.cx, |c, _| c.test_lines())
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert_eq!(lines, ["fix the tests", "and update the docs"]);
    assert_eq!(texts(&sent).len(), 1);
}

// ---- Composer undo ----

#[gpui::test]
async fn cmd_z_undoes_one_typing_burst_at_a_time_including_shift_enter(cx: &mut TestAppContext) {
    let mut h = boot(cx, items_opts(Store::default()));
    h.focus_composer();
    let lines = |h: &mut H| {
        let chat = h.chat();
        chat.read_with(h.cx, |c, _| c.test_lines())
    };
    h.type_text("hello");
    h.wait(1000);
    h.type_text(" world");
    h.keys("shift-enter");
    h.type_text("next");

    h.keys("secondary-z");
    assert_eq!(lines(&mut h), ["hello world", ""]);
    h.keys("secondary-z");
    assert_eq!(lines(&mut h), ["hello world"]);
    h.keys("secondary-z");
    assert_eq!(lines(&mut h), ["hello"]);
    h.keys("secondary-shift-z");
    assert_eq!(lines(&mut h), ["hello world"]);
}

// ---- Fix findings and prompt routing ----

fn open_finding() -> Value {
    json!({"id": "f-1", "iteration": 1, "file": "src/main.go", "line": 10, "severity": "medium",
        "message": "handle the error", "status": "open"})
}

fn on_it_opts(store: Store, sent: Calls) -> MockOptions {
    bridge_opts(store, sent, Pending::default(), |n, _| {
        Some(format!("On it. ({n})"))
    })
}

#[gpui::test]
async fn slash_fix_findings_shows_a_short_label_while_sending_the_full_checklist(
    cx: &mut TestAppContext,
) {
    let (store, sent) = (Store::default(), calls());
    let opts = on_it_opts(store, sent.clone()).resolver("ListFindings", |a, _, _| {
        let mut f = open_finding();
        f["workspaceId"] = a[0].clone();
        Reply::value(json!([f]))
    });
    let mut h = boot(cx, opts);
    h.select_tab("PA");
    h.focus_composer();
    h.type_text("/fix-findings");
    assert!(h.has("mention:/fix-findings"));
    h.keys("enter");
    assert!(h.me("Fix open reviewer findings"));
    assert!(
        !h.bodies()
            .iter()
            .any(|b| b.contains("mcp__recur__list_findings"))
    );
    let t = texts(&sent);
    assert_eq!(t.len(), 1);
    assert!(t[0].contains("mcp__recur__list_findings"));
    assert!(t[0].contains("mcp__recur__update_finding"));
    let chat = h.chat();
    let focused =
        h.cx.update(|window, cx| chat.read(cx).test_composer_focused(window));
    assert!(focused);
}

#[gpui::test]
async fn slash_review_and_fix_opens_a_loop_tab_instead_of_converting_the_agent_chat(
    cx: &mut TestAppContext,
) {
    let (store, sent) = (Store::default(), calls());
    let opts = bridge_opts(store, sent.clone(), Pending::default(), |_, _| {
        Some("Loop engaged.".into())
    });
    let mut h = boot(cx, opts);
    h.select_tab("PA");
    h.focus_composer();
    h.type_text("/review-and-fix");
    assert!(h.has("mention:/review-and-fix"));
    assert!(h.has("badge:Loop"));
    h.keys("enter");
    assert!(h.me("Review & fix"));
    assert_eq!(texts(&sent).len(), 1);
    assert_eq!(arg(&sent, 0, 6), json!("loop"));
    assert_ne!(arg(&sent, 0, 1), json!("PA"));
    assert_eq!(arg(&sent, 0, 7), json!("Review & fix"));
    assert!(texts(&sent)[0].contains("fix and re-review until the policy bar passes"));
    assert_eq!(
        h.read(|v, cx| v.chat_mode(cx)),
        recur_view_core::modes::ChatMode::Loop
    );
    assert_eq!(h.read(|v, cx| v.dock_loop_id(cx)), "recur.loops.converge");
}

/// A started PA tab, then `/slash` picked from the menu with a click.
fn started_pa(h: &mut H) {
    h.select_tab("PA");
    h.fill("hello from the agent tab");
    send_click(h);
    assert!(h.me("hello from the agent tab"));
}

fn pick(h: &mut H, slash: &str) {
    h.focus_composer();
    h.type_text(&format!("/{slash}"));
    h.click(&format!("mention:/{slash}"));
}

#[gpui::test]
async fn slash_review_on_a_started_agent_tab_opens_a_new_chat(cx: &mut TestAppContext) {
    let (store, sent) = (Store::default(), calls());
    let mut h = boot(cx, on_it_opts(store, sent.clone()));
    started_pa(&mut h);
    pick(&mut h, "review");
    assert!(h.me("Please review the changes in this worktree and file any real issues"));
    let review = sent
        .lock()
        .iter()
        .find(|a| a[2].as_str().unwrap_or_default().contains("file findings"))
        .cloned()
        .expect("review sent");
    assert_ne!(review[1], json!("PA"));
    h.select_tab("PA");
    assert!(h.me("hello from the agent tab"));
    assert!(!h.me("Please review the changes in this worktree and file any real issues"));
}

#[gpui::test]
async fn slash_commit_on_a_started_agent_tab_opens_a_grouped_tab_and_isolates(
    cx: &mut TestAppContext,
) {
    let (store, sent) = (Store::default(), calls());
    let mut h = boot(cx, on_it_opts(store, sent.clone()));
    started_pa(&mut h);
    pick(&mut h, "commit");
    assert!(h.me("Commit all changes"));
    assert!(!h.me("hello from the agent tab"));
    assert_eq!(texts(&sent).len(), 2);
    assert_ne!(arg(&sent, 1, 1), arg(&sent, 0, 1));
    assert_eq!(arg(&sent, 1, 8), json!(true));

    h.select_tab("PA");
    assert!(h.me("hello from the agent tab"));
    pick(&mut h, "commit-push");
    assert!(h.me("Commit all changes and push"));
    assert!(!h.me("hello from the agent tab"));
    assert_eq!(texts(&sent).len(), 3);
    assert_eq!(arg(&sent, 2, 1), arg(&sent, 1, 1));
    assert_eq!(arg(&sent, 2, 8), json!(true));
}

#[gpui::test]
async fn slash_commit_does_not_claim_a_new_chat_with_an_unsent_draft(cx: &mut TestAppContext) {
    let (store, sent) = (Store::default(), calls());
    let mut h = boot(cx, on_it_opts(store, sent.clone()));
    h.select_tab("PA");
    h.fill("unsent draft stays here");
    h.select_tab("MA");
    pick(&mut h, "commit");
    assert!(h.me("Commit all changes"));
    assert_eq!(texts(&sent).len(), 1);
    assert_ne!(arg(&sent, 0, 1), json!("PA"));
    h.select_tab("PA");
    assert_eq!(h.input(), "unsent draft stays here");
    assert!(!h.me("Commit all changes"));
}

#[gpui::test]
async fn failed_deferred_commit_retry_sends_a_fresh_grouped_action_without_eating_the_draft(
    cx: &mut TestAppContext,
) {
    let (store, sent) = (Store::default(), calls());
    let creates = Arc::new(Mutex::new(0usize));
    let opts = on_it_opts(store, sent.clone()).resolver("CreateWorkspaceChat", move |a, s, _| {
        let n = {
            let mut c = creates.lock();
            *c += 1;
            *c
        };
        if n == 1 {
            return Reply::null();
        }
        let ws = a[0].as_str().unwrap_or_default().to_string();
        let list = s.chats_by_loop.entry(ws).or_default();
        let tab =
            json!({"code": format!("PA{}", list.len() + 2), "title": "New chat", "updatedAt": 0});
        list.push(tab.clone());
        Reply::value(tab)
    });
    let mut h = boot(cx, opts);
    h.select_tab("PA");
    h.fill("unsent draft stays here");
    h.select_tab("MA");
    pick(&mut h, "commit");
    assert!(
        h.toasts()
            .iter()
            .any(|t| t.contains("is ready — open a chat tab and retry"))
    );
    h.select_tab("PA");
    assert_eq!(h.input(), "unsent draft stays here");
    assert!(texts(&sent).is_empty());

    h.toast_action("Retry");
    assert!(h.me("Commit all changes"));
    assert_eq!(texts(&sent).len(), 1);
    assert_ne!(arg(&sent, 0, 1), json!("PA"));
    assert_eq!(arg(&sent, 0, 8), json!(true));
    h.select_tab("PA");
    assert_eq!(h.input(), "unsent draft stays here");
    assert!(!h.me("Commit all changes"));
}

#[gpui::test]
async fn slash_commit_push_does_not_reuse_an_ops_tab_with_an_unsent_draft(cx: &mut TestAppContext) {
    let (store, sent) = (Store::default(), calls());
    let mut h = boot(cx, on_it_opts(store, sent.clone()));
    started_pa(&mut h);
    pick(&mut h, "commit");
    assert!(h.me("Commit all changes"));
    let ops = arg(&sent, 1, 1);
    assert_ne!(ops, json!("PA"));
    h.fill("follow-up stays on the ops tab");
    h.select_tab("PA");
    assert!(h.me("hello from the agent tab"));
    pick(&mut h, "commit-push");
    assert!(h.me("Commit all changes and push"));
    assert_eq!(texts(&sent).len(), 3);
    assert_ne!(arg(&sent, 2, 1), ops);
    assert_ne!(arg(&sent, 2, 1), json!("PA"));
    h.select_tab(ops.as_str().unwrap_or_default());
    assert_eq!(h.input(), "follow-up stays on the ops tab");
    assert!(!h.me("Commit all changes and push"));
}

#[gpui::test]
async fn slash_review_send_stays_on_the_routed_tab_if_the_operator_switches_away(
    cx: &mut TestAppContext,
) {
    let (store, sent) = (Store::default(), calls());
    let gate: Arc<Mutex<Option<Release>>> = Default::default();
    let g2 = gate.clone();
    let opts = on_it_opts(store, sent.clone()).resolver("RunPromptAction", move |a, _, _| {
        if a[0] == json!("recur.chat.review") {
            let (r, release) = Reply::deferred();
            *g2.lock() = Some(release);
            return r;
        }
        Reply::value(json!({"prompt": "x", "display": "x"}))
    });
    let mut h = boot(cx, opts);
    started_pa(&mut h);
    pick(&mut h, "review");
    assert_eq!(h.codes().len(), 3);
    h.select_tab("PA");
    assert!(h.me("hello from the agent tab"));
    if let Some(r) = gate.lock().take() {
        r.send(Reply::value(json!({
            "prompt": "Review the changes in this worktree and file findings. Git context is already gathered — do NOT re-run git status/diff/log.",
            "display": "Please review the changes in this worktree and file any real issues",
            "freshSession": true,
        })));
    }
    h.settle();
    let reviews: Vec<Value> = sent
        .lock()
        .iter()
        .filter(|a| a[2].as_str().unwrap_or_default().contains("file findings"))
        .map(|a| a[1].clone())
        .collect();
    assert_eq!(reviews.len(), 1);
    assert_ne!(reviews[0], json!("PA"));
    assert!(!h.me("Please review the changes in this worktree and file any real issues"));
}

#[gpui::test]
async fn findings_panel_fix_open_findings_switches_to_the_agent_dock_and_tab(
    cx: &mut TestAppContext,
) {
    let (store, sent) = (Store::default(), calls());
    let opts = on_it_opts(store, sent.clone()).resolver("ListFindings", |a, _, _| {
        let mut f = open_finding();
        f["workspaceId"] = a[0].clone();
        Reply::value(json!([f]))
    });
    let mut h = boot(cx, opts);
    h.select_tab("PA");
    assert!(h.has("chat-composer"));
    h.click("panel-toggle:findings");
    assert!(!h.has("chat-composer"));
    h.click("fix-findings");
    assert!(h.has("chat-composer"));
    assert!(h.me("Fix open reviewer findings"));
    let t = texts(&sent);
    assert_eq!(t.len(), 1);
    assert!(t[0].contains("mcp__recur__list_findings"));
}

// ---- Chat stop ----

/// Sends hang per code; Cancel records `ws:code` and answers that code's turn.
fn cancel_opts(store: Store, cancelled: Calls) -> MockOptions {
    let holds: Arc<Mutex<HashMap<String, Release>>> = Default::default();
    let (h1, h2, s2, s3) = (holds.clone(), holds, store.clone(), store.clone());
    items_opts(store)
        .resolver("SendWorkspaceMessage", move |a, _, _| {
            let code = a[1].as_str().unwrap_or("MA").to_string();
            s2.lock()
                .entry(code.clone())
                .or_default()
                .push(json!({"role": "user", "kind": "text", "text": a[2]}));
            let (r, release) = Reply::deferred();
            h1.lock().insert(code, release);
            r
        })
        .resolver("CancelWorkspaceMessage", move |a, _, _| {
            let (ws, code) = (
                a[0].as_str().unwrap_or_default(),
                a[1].as_str().unwrap_or_default().to_string(),
            );
            cancelled.lock().push(vec![json!(format!("{ws}:{code}"))]);
            s3.lock()
                .entry(code.clone())
                .or_default()
                .push(json!({"role": "assistant", "kind": "text", "text": "Stopped by user"}));
            if let Some(r) = h2.lock().remove(&code) {
                r.send(Reply::value(json!("Stopped by user")));
            }
            Reply::null()
        })
}

fn flat(c: &Calls) -> Vec<String> {
    c.lock()
        .iter()
        .map(|a| a[0].as_str().unwrap_or_default().to_string())
        .collect()
}

#[gpui::test]
async fn stop_interrupts_the_active_tabs_in_flight_turn(cx: &mut TestAppContext) {
    let cancelled = calls();
    let mut h = boot(cx, cancel_opts(Store::default(), cancelled.clone()));
    h.fill("please run forever");
    send_click(&mut h);
    assert!(h.has("chat-stop"));
    h.click("chat-stop");
    assert!(h.has("chat-stopped"));
    assert_eq!(flat(&cancelled), [format!("{WS}:MA")]);
    assert!(!h.has("chat-stop"));
}

#[gpui::test]
async fn cmd_shift_backspace_stops_an_in_flight_turn_when_the_agent_panel_is_focused(
    cx: &mut TestAppContext,
) {
    let cancelled = calls();
    let mut h = boot(cx, cancel_opts(Store::default(), cancelled.clone()));
    h.fill("please run forever");
    send_click(&mut h);
    assert!(h.has("chat-stop"));
    h.focus_composer();
    h.keys("secondary-shift-backspace");
    assert!(h.has("chat-stopped"));
    assert_eq!(flat(&cancelled), [format!("{WS}:MA")]);
}

#[gpui::test]
async fn stop_cancels_only_the_active_tab_a_background_tabs_turn_keeps_running(
    cx: &mut TestAppContext,
) {
    let cancelled = calls();
    let mut h = boot(cx, cancel_opts(Store::default(), cancelled.clone()));
    h.select_tab("PA");
    h.fill("long PA task");
    send_click(&mut h);
    h.click("wsv-chat-new");
    assert_eq!(h.codes().len(), 3);
    assert!(!h.has("chat-stop"));
    h.fill("long PA2 task");
    send_click(&mut h);
    assert!(h.has("chat-stop"));
    h.click("chat-stop");
    assert!(h.has("chat-stopped"));
    assert_eq!(flat(&cancelled), [format!("{WS}:PA2")]);
    h.select_tab("PA");
    assert!(h.me("long PA task"));
    assert!(!h.has("chat-stopped"));
}

// ---- Chat live turn restore ----

fn hang_opts(store: Store, sent: Calls) -> MockOptions {
    bridge_opts(store, sent, Pending::default(), |_, _| None)
}

#[gpui::test]
async fn leaving_and_returning_mid_turn_keeps_earlier_messages_and_the_elapsed_clock(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, hang_opts(store.clone(), calls()));
    load(
        &mut h,
        &store,
        "MA",
        json!([
            {"role": "user", "kind": "text", "text": "earlier question"},
            {"role": "assistant", "kind": "text", "text": "earlier answer"},
        ]),
    );
    assert!(h.me("earlier question"));
    h.fill("keep going");
    send_click(&mut h);
    assert!(h.has("chat-turn-elapsed"));
    h.with(|v, cx| {
        let k = v.active_key();
        *v.turn_started_at_by_key.entry(k).or_default() -= 5_000;
        v.touch(cx);
    });
    let chat = h.chat();
    let before = chat.read_with(h.cx, |c, _| c.test_turn_elapsed_sec());
    assert!(before >= 5);

    h.open("LOOP-2229");
    h.open(WS);
    assert!(h.me("earlier question"));
    assert!(h.me("keep going"));
    assert!(h.assistant("earlier answer"));
    assert!(h.has("chat-turn-elapsed"));
    let chat = h.chat();
    assert!(chat.read_with(h.cx, |c, _| c.test_turn_elapsed_sec()) >= before);
}

#[gpui::test]
async fn elapsed_clock_hides_when_the_engine_goes_idle_while_send_is_still_pending(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()));
    h.fill("keep going");
    send_click(&mut h);
    assert!(h.has("chat-turn-elapsed"));
    idle(&mut h, "MA");
    assert!(!h.has("chat-turn-elapsed"));
    let chat = h.chat();
    assert!(!chat.read_with(h.cx, |c, _| c.props().sending));
}

#[gpui::test]
async fn elapsed_clock_hides_when_turn_end_lands_while_send_is_still_pending(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()));
    h.fill("keep going");
    send_click(&mut h);
    assert!(h.has("chat-turn-elapsed"));
    working(&mut h, "MA");
    delta(&mut h, "MA", "here is the reply", 1);
    turn_end(&mut h, "MA", 103_000);
    assert!(h.has("turn-end"));
    assert!(h.assistant("here is the reply"));
    assert!(!h.has("chat-turn-elapsed"));
}

#[gpui::test]
async fn leftover_stream_after_turn_end_does_not_leak_the_elapsed_clock_onto_other_chats(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()));
    h.fill("keep going");
    send_click(&mut h);
    assert!(h.has("chat-turn-elapsed"));
    working(&mut h, "MA");
    delta(&mut h, "MA", "here is the reply", 1);
    turn_end(&mut h, "MA", 103_000);
    delta(&mut h, "MA", "late leftover chunk", 2);
    assert!(h.has("turn-end"));
    assert!(h.assistant("here is the reply"));
    assert!(!h.bodies().iter().any(|b| b.contains("late leftover chunk")));
    assert!(!h.has("chat-turn-elapsed"));
    assert!(h.has("chat-stop"));

    h.select_tab("PA");
    assert!(!h.has("chat-turn-elapsed"));
    h.select_tab("MA");
    assert!(h.has("turn-end"));
    assert!(!h.has("chat-turn-elapsed"));
    assert!(!h.bodies().iter().any(|b| b.contains("late leftover chunk")));
}

#[gpui::test]
async fn a_new_engine_turn_after_lost_idle_still_streams_past_the_previous_footer(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()));
    h.fill("keep going");
    send_click(&mut h);
    working(&mut h, "MA");
    delta(&mut h, "MA", "here is the reply", 1);
    turn_end(&mut h, "MA", 103_000);
    delta(&mut h, "MA", "late leftover chunk", 2);
    assert!(h.has("turn-end"));
    working(&mut h, "MA");
    delta(&mut h, "MA", "second turn after lost idle", 3);
    assert!(
        h.bodies()
            .iter()
            .any(|b| b == "second turn after lost idle")
    );
    assert!(!h.bodies().iter().any(|b| b.contains("late leftover chunk")));
}

#[gpui::test]
async fn idle_reordered_past_the_next_turn_still_lets_that_turn_stream(cx: &mut TestAppContext) {
    let mut h = boot(cx, hang_opts(Store::default(), calls()));
    h.fill("keep going");
    send_click(&mut h);
    working(&mut h, "MA");
    delta(&mut h, "MA", "here is the reply", 1);
    turn_end(&mut h, "MA", 103_000);
    working(&mut h, "MA");
    idle(&mut h, "MA");
    delta(&mut h, "MA", "second turn after reordered idle", 2);
    assert!(
        h.bodies()
            .iter()
            .any(|b| b == "second turn after reordered idle")
    );
    assert!(h.has("turn-end"));
}

#[gpui::test]
async fn a_reordered_idle_still_drops_sticky_send_so_a_queued_follow_up_can_flush(
    cx: &mut TestAppContext,
) {
    let sent = calls();
    let opts = bridge_opts(
        Store::default(),
        sent.clone(),
        Pending::default(),
        |n, _| (n > 1).then(|| "ok".to_string()),
    );
    let mut h = boot(cx, opts);
    h.fill("keep going");
    send_click(&mut h);
    assert!(h.has("chat-stop"));
    h.fill("follow up next");
    send_click(&mut h);
    assert!(h.has("queued:follow up next"));
    working(&mut h, "MA");
    delta(&mut h, "MA", "here is the reply", 1);
    turn_end(&mut h, "MA", 103_000);
    working(&mut h, "MA");
    idle(&mut h, "MA");
    let t = texts(&sent);
    assert_eq!(t.len(), 2);
    assert!(t[1].ends_with("follow up next"));
    assert!(!h.has("queued:follow up next"));
}

#[gpui::test]
async fn a_reply_landing_after_idle_flushed_the_queue_does_not_repaint_under_the_follow_up(
    cx: &mut TestAppContext,
) {
    let (store, sent) = (Store::default(), calls());
    let first: Pending = Default::default();
    let f2 = first.clone();
    let opts = hang_opts(store.clone(), sent.clone()).resolver("SendWorkspaceMessage", {
        let (sent, store) = (sent.clone(), store.clone());
        move |a, _, _| {
            let n = {
                let mut s = sent.lock();
                s.push(a.to_vec());
                s.len()
            };
            store
                .lock()
                .entry("MA".into())
                .or_default()
                .push(json!({"role": "user", "kind": "text", "text": a[2]}));
            let (r, release) = Reply::deferred();
            if n == 1 {
                f2.lock().push(release);
            } else {
                std::mem::forget(release);
            }
            r
        }
    });
    let mut h = boot(cx, opts);
    h.fill("first turn");
    send_click(&mut h);
    h.fill("queued follow-up");
    send_click(&mut h);
    assert!(h.has("queued:queued follow-up"));
    store
        .lock()
        .entry("MA".into())
        .or_default()
        .push(json!({"role": "assistant", "kind": "text", "text": "first reply text"}));
    working(&mut h, "MA");
    delta(&mut h, "MA", "first reply text", 1);
    let count = |h: &mut H| {
        h.bodies()
            .iter()
            .filter(|b| b.as_str() == "first reply text")
            .count()
    };
    assert_eq!(count(&mut h), 1);
    idle(&mut h, "MA");
    assert_eq!(texts(&sent).len(), 2);
    if let Some(r) = first.lock().pop() {
        r.send(Reply::value(json!("first reply text")));
    }
    h.wait(300);
    assert_eq!(count(&mut h), 1);
}

#[gpui::test]
async fn an_autonomous_turn_after_idle_still_streams_past_the_previous_footer(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, hang_opts(store.clone(), calls()));
    h.fill("kick off");
    send_click(&mut h);
    assert!(h.has("chat-turn-elapsed"));
    delta(&mut h, "MA", "first reply", 1);
    turn_end(&mut h, "MA", 1200);
    store.lock().entry("MA".into()).or_default().extend([
        json!({"role": "assistant", "kind": "text", "text": "first reply"}),
        json!({"role": "assistant", "kind": "turn", "durationMs": 1200}),
    ]);
    idle(&mut h, "MA");
    assert!(h.has("turn-end"));
    assert!(!h.has("chat-turn-elapsed"));
    working(&mut h, "MA");
    delta(&mut h, "MA", "planning the next slice", 2);
    assert!(h.bodies().iter().any(|b| b == "planning the next slice"));
    assert!(h.has("chat-turn-elapsed"));
}

#[gpui::test]
async fn a_second_idle_resyncs_a_stream_that_never_got_turn_end_after_a_footer(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, hang_opts(store.clone(), calls()));
    h.fill("kick off");
    send_click(&mut h);
    working(&mut h, "MA");
    delta(&mut h, "MA", "first reply", 1);
    turn_end(&mut h, "MA", 1200);
    working(&mut h, "MA");
    delta(&mut h, "MA", "second turn still streaming", 2);
    store.lock().entry("MA".into()).or_default().extend([
        json!({"role": "assistant", "kind": "text", "text": "first reply"}),
        json!({"role": "assistant", "kind": "turn", "durationMs": 1200}),
        json!({"role": "assistant", "kind": "text", "text": "second turn still streaming"}),
    ]);
    let streaming = |h: &mut H| {
        h.messages()
            .iter()
            .any(|m| m.streaming && m.body == "second turn still streaming")
    };
    assert!(streaming(&mut h));
    idle(&mut h, "MA");
    assert!(streaming(&mut h));
    idle(&mut h, "MA");
    assert!(!streaming(&mut h));
    assert!(h.assistant("second turn still streaming"));
}

// ---- Chat code fence copy ----

#[gpui::test]
async fn fenced_code_in_a_settled_assistant_reply_has_a_copy_control(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(
        &mut h,
        &store,
        "MA",
        json!([{"role": "assistant", "kind": "text", "text": "here is a snippet:\n\n```ts\nconst n = 1\n```\n"}]),
    );
    assert!(h.has("md-fence"));
    assert!(!h.has("md-copy"));
    h.hover("md-fence");
    assert!(h.has("md-copy"));
    h.click("md-copy");
    assert_eq!(
        h.cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("const n = 1")
    );
}

#[gpui::test]
async fn user_messages_have_a_copy_control(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(
        &mut h,
        &store,
        "MA",
        json!([
            {"role": "user", "kind": "text", "text": "please copy this prompt"},
            {"role": "assistant", "kind": "text", "text": "ok"},
        ]),
    );
    assert!(h.me("please copy this prompt"));
    h.hover("me:please copy this prompt");
    h.click("chat-bubble-copy");
    assert_eq!(
        h.cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("please copy this prompt")
    );
    let chat = h.chat();
    assert!(chat.read_with(h.cx, |c, _| c.test_copied_bubble()));
}

// ---- Chat agent error retry ----

const SPEND: &str = "Agent error: You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit";

#[gpui::test]
async fn spend_limit_error_renders_a_card_with_retry_that_re_sends_the_last_prompt(
    cx: &mut TestAppContext,
) {
    let sent = calls();
    let opts = bridge_opts(
        Store::default(),
        sent.clone(),
        Pending::default(),
        |n, _| {
            Some(if n == 1 {
                SPEND.into()
            } else {
                "ok, continuing".into()
            })
        },
    );
    let mut h = boot(cx, opts);
    h.fill("ship the dark mode polish");
    send_click(&mut h);
    assert!(h.has("chat-error:Usage limit"));
    let me_rows = |h: &mut H| {
        h.messages()
            .iter()
            .filter(|m| m.me && m.body == "ship the dark mode polish")
            .count()
    };
    assert_eq!(me_rows(&mut h), 1);
    h.click("chat-error-retry");
    assert!(h.assistant("ok, continuing"));
    let displays: Vec<String> = sent
        .lock()
        .iter()
        .map(|a| a[7].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(displays, ["", "\u{2060}"]);
    assert_eq!(texts(&sent)[1], "\u{2060}");
    assert_eq!(me_rows(&mut h), 1);
}

#[gpui::test]
async fn retry_shows_the_elapsed_clock_before_the_first_stream_row(cx: &mut TestAppContext) {
    let opts = bridge_opts(Store::default(), calls(), Pending::default(), |n, _| {
        (n == 1).then(|| SPEND.to_string())
    });
    let mut h = boot(cx, opts);
    h.fill("ship the dark mode polish");
    send_click(&mut h);
    assert!(h.has("chat-error-retry"));
    turn_end(&mut h, "MA", 1200);
    assert!(h.has("turn-end"));
    h.click("chat-error-retry");
    assert!(h.has("chat-turn-elapsed"));
}

// ---- @ file mentions ----

fn files_opts(files: &'static [&'static str], sent: Calls) -> MockOptions {
    bridge_opts(Store::default(), sent, Pending::default(), |_, _| {
        Some("ok".into())
    })
    .resolver("ListFiles", move |_, _, _| Reply::value(json!(files)))
}

fn chips(h: &mut H) -> usize {
    let chat = h.chat();
    chat.read_with(h.cx, |c, _| c.test_chip_count())
}

fn lines(h: &mut H) -> Vec<String> {
    let chat = h.chat();
    chat.read_with(h.cx, |c, _| c.test_lines())
}

fn type_wait(h: &mut H, s: &str) {
    h.type_text(s);
    h.wait(250);
}

#[gpui::test]
async fn typing_at_inserts_deletable_inline_chips_in_the_composer_sentence(
    cx: &mut TestAppContext,
) {
    let sent = calls();
    let skills = json!([
        {"name": "design-audit", "description": "Audit UI polish", "scope": "workspace", "userInvocable": true},
        {"name": "land-it", "description": "Ship the PR", "scope": "workspace", "userInvocable": true},
    ]);
    let (s1, s2) = (skills.clone(), skills);
    let opts = files_opts(
        &[
            "PROGRESS.md",
            "DECISIONS.md",
            ".gitignore",
            "frontend/src/lib/agentchat.ts",
        ],
        sent.clone(),
    )
    .resolver("ListSkills", move |_, _, _| Reply::value(s1.clone()))
    .resolver("ListAllSkills", move |_, _, _| Reply::value(s2.clone()));
    let mut h = boot(cx, opts);
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
    assert!(lines(&mut h)[0].starts_with("how does"));

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
    send_click(&mut h);
    let t = texts(&sent);
    assert!(
        t[0].ends_with("how does @PROGRESS.md compare to @DECISIONS.md"),
        "{t:?}"
    );
    assert!(h.has("file-inline:PROGRESS.md"));
    assert!(h.has("file-inline:DECISIONS.md"));
}

#[gpui::test]
async fn mention_menu_keeps_lower_ranked_matches_past_the_old_top_12_cap(cx: &mut TestAppContext) {
    let mut files: Vec<String> = (0..15).map(|i| format!("a{i:02}/match.md")).collect();
    files.push("frontend/src/lib/deeply/nested/path/match-util.ts".into());
    let opts = items_opts(Store::default())
        .resolver("ListFiles", move |_, _, _| Reply::value(json!(files)));
    let mut h = boot(cx, opts);
    h.focus_composer();
    type_wait(&mut h, "@match");
    assert!(h.has("chat-mention"));
    let chat = h.chat();
    assert_eq!(chat.read_with(h.cx, |c, _| c.test_mention_hits()), 16);
    assert!(h.has("menu-row:15"));
    assert!(h.has("mention:match-util.ts"));
}

#[gpui::test]
async fn directory_hits_appear_in_at_search_with_a_folder_icon(cx: &mut TestAppContext) {
    let opts = items_opts(Store::default()).resolver("ListFiles", |_, _, _| {
        Reply::value(json!(["src/lib/a.ts", "src/app.ts", "README.md"]))
    });
    let mut h = boot(cx, opts);
    h.focus_composer();
    type_wait(&mut h, "@src");
    assert!(h.has("chat-mention"));
    assert!(h.has("mention-folder"));
    h.click("mention:src");
    assert!(h.has("chip-folder"));
    assert!(h.has("chip:src"));
    h.click("chip:src");
    assert!(h.toasts().is_empty());
    assert!(h.has("chip:src"));
}

#[gpui::test]
async fn at_hyp_lists_both_hyperlane_validator_scripts(cx: &mut TestAppContext) {
    let opts = items_opts(Store::default()).resolver("ListFiles", |_, _, _| {
        Reply::value(json!([
            "validators/decode-hyperlane-message.sh",
            "validators/hyperlane_validator2.sh",
            "hypothesis.txt"
        ]))
    });
    let mut h = boot(cx, opts);
    h.focus_composer();
    type_wait(&mut h, "@hyp");
    assert!(h.has("chat-mention"));
    assert!(h.has("mention:decode-hyperlane-message.sh"));
    assert!(h.has("mention:hyperlane_validator2.sh"));
}

#[gpui::test]
async fn escape_dismisses_the_mention_menu_without_reopening_on_keyup(cx: &mut TestAppContext) {
    let opts = items_opts(Store::default()).resolver("ListFiles", |_, _, _| {
        Reply::value(json!(["PROGRESS.md", "DECISIONS.md"]))
    });
    let mut h = boot(cx, opts);
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

// ---- Transcript file chips route into the Observatory ----

fn chip_opts(mention: &'static str) -> MockOptions {
    MockOptions::default()
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
        .resolver("WorkspaceChatItems", move |_, _, _| {
            Reply::value(json!([{"role": "user", "kind": "text", "text": format!("please look at @{mention}")}]))
        })
}

fn right_panel(h: &mut H) -> Option<CodePanelKey> {
    let v = h.view();
    h.cx.update(|_, cx| {
        let o = v.read(cx).observatory().clone();
        o.read(cx).right_panel(cx)
    })
}

fn obs_read<R>(
    h: &mut H,
    f: impl FnOnce(&recur_view_observatory::Observatory, &gpui_kit::App) -> R,
) -> R {
    let v = h.view();
    h.cx.update(|_, cx| {
        let o = v.read(cx).observatory().clone();
        f(o.read(cx), cx)
    })
}

#[gpui::test]
async fn transcript_file_chip_opens_the_editor_without_stealing_the_files_panel(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, chip_opts("src/main.go"));
    assert!(h.has("obs-node:file:src/main.go"));
    assert_eq!(right_panel(&mut h), Some(CodePanelKey::Files));
    h.click("file-inline:main.go");
    assert_eq!(
        obs_read(&mut h, |o, _| o.selected_path().to_string()),
        "src/main.go"
    );
    assert_eq!(right_panel(&mut h), Some(CodePanelKey::Files));
    assert_eq!(
        obs_read(&mut h, |o, _| o.tree_path().to_string()),
        "src/main.go"
    );
}

#[gpui::test]
async fn transcript_file_chip_keeps_source_control_open_instead_of_flipping_to_outline(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, chip_opts("src/main.go"));
    h.click("panel-toggle:scm");
    assert_eq!(right_panel(&mut h), Some(CodePanelKey::Scm));
    h.click("file-inline:main.go");
    assert_eq!(
        obs_read(&mut h, |o, _| o.selected_path().to_string()),
        "src/main.go"
    );
    assert_eq!(right_panel(&mut h), Some(CodePanelKey::Scm));
}

#[gpui::test]
async fn transcript_file_chip_does_not_select_a_file_that_is_filtered_out_of_the_explorer(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, chip_opts("PROGRESS.md"));
    assert!(h.has("obs-node:file:src/main.go"));
    h.click("obs-node:file:src/main.go");
    assert_eq!(
        obs_read(&mut h, |o, _| o.tree_path().to_string()),
        "src/main.go"
    );
    h.click("file-inline:PROGRESS.md");
    assert_eq!(
        obs_read(&mut h, |o, _| o.selected_path().to_string()),
        "PROGRESS.md"
    );
    assert_eq!(right_panel(&mut h), Some(CodePanelKey::Files));
    assert_eq!(
        obs_read(&mut h, |o, _| o.tree_path().to_string()),
        "src/main.go"
    );
    assert!(!h.has("obs-node:file:PROGRESS.md"));
}

// ---- Drops ----

fn drop_explorer(h: &mut H, paths: &[&str]) {
    let chat = h.chat();
    let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| c.test_drop_explorer(&paths, window, cx)));
    h.settle();
}

#[gpui::test]
async fn dragging_a_file_from_the_explorer_inserts_an_at_file_chip(cx: &mut TestAppContext) {
    let sent = calls();
    let mut h = boot(cx, files_opts(&[], sent.clone()));
    h.focus_composer();
    h.type_text("please review ");
    drop_explorer(&mut h, &["src/main.go"]);
    let chat = h.chat();
    assert!(!chat.read_with(h.cx, |c, _| c.test_file_drop_over()));
    assert!(h.has("chip:main.go"));
    assert!(lines(&mut h)[0].starts_with("please review"));
    send_click(&mut h);
    assert!(texts(&sent)[0].ends_with("please review @src/main.go"));
    assert!(h.has("file-inline:main.go"));
}

#[gpui::test]
async fn dragging_a_folder_from_the_explorer_inserts_a_folder_chip(cx: &mut TestAppContext) {
    let mut h = boot(cx, items_opts(Store::default()));
    h.focus_composer();
    drop_explorer(&mut h, &["src/"]);
    assert!(h.has("chip-folder"));
    assert!(h.has("chip:src"));
}

fn os_drop(h: &mut H, path: &str) {
    let at = h.bounds("chat").expect("chat").center();
    let paths = gpui::ExternalPaths(smallvec::smallvec![std::path::PathBuf::from(path)]);
    h.cx.simulate_event(gpui::FileDropEvent::Entered {
        position: at,
        paths,
    });
    h.cx.simulate_event(gpui::FileDropEvent::Submit { position: at });
    h.settle();
}

#[gpui::test]
async fn dropping_a_file_uri_onto_chat_inserts_an_at_file_chip(cx: &mut TestAppContext) {
    let sent = calls();
    let mut h = boot(cx, files_opts(&[], sent.clone()));
    h.focus_composer();
    os_drop(&mut h, &format!("/mock/worktrees/{WS}/src/main.go"));
    assert!(h.has("chip:main.go"));
    send_click(&mut h);
    assert!(texts(&sent)[0].ends_with("@src/main.go"));
}

#[gpui::test]
async fn dropping_an_external_image_attaches_it_like_paste(cx: &mut TestAppContext) {
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
        .unwrap();
    let dir = std::env::temp_dir().join(format!("recur-ws-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let shot = dir.join("shot.png");
    std::fs::write(&shot, png).unwrap();
    let mut h = boot(cx, items_opts(Store::default()));
    h.focus_composer();
    let id = recur_view_core::file_drag::encode_chat_drop_id(&format!("{WS}\tMA"));
    h.dispatch(
        "files.dropped",
        json!({"files": [shot.to_string_lossy()], "attributes": {"data-chat-drop": id}}),
    );
    assert!(h.has("chip-att:0"));
    let chat = h.chat();
    assert_eq!(chat.read_with(h.cx, |c, _| c.attachments().len()), 1);
    assert!(!chat.read_with(h.cx, |c, _| c.test_has_file_chip()));
    let _ = std::fs::remove_dir_all(dir);
}

#[gpui::test]
async fn clicking_an_external_file_chip_opens_it_in_the_editor(cx: &mut TestAppContext) {
    let mut h = boot(cx, items_opts(Store::default()));
    h.focus_composer();
    os_drop(&mut h, "/Users/josh/Downloads/export-111.csv");
    assert!(h.has("chip:export-111.csv"));
    h.click("chip:export-111.csv");
    let jump = h
        .state
        .read_with(h.cx, |s, _| s.observatory_jump.clone().map(|j| j.path));
    let opened = obs_read(&mut h, |o, _| o.selected_path().to_string());
    assert!(
        jump.as_deref() == Some("/Users/josh/Downloads/export-111.csv")
            || opened == "/Users/josh/Downloads/export-111.csv",
        "{jump:?} {opened}"
    );
}

// ---- @terminal / @artifact mentions ----

#[gpui::test]
async fn typing_at_lists_terminals_and_inserts_a_chip(cx: &mut TestAppContext) {
    let sent = calls();
    let opts = files_opts(&["README.md"], sent.clone()).resolver("ListTerminals", |_, _, _| {
        Reply::value(json!([{"id": "term-dev-1", "workspaceId": WS, "title": "dev", "cwd": "/mock", "shell": "/bin/zsh", "createdAt": "2026-08-24T12:00:00Z"}]))
    });
    let mut h = boot(cx, opts);
    h.focus_composer();
    type_wait(&mut h, "errors in @term");
    assert!(h.has("mention:dev"));
    h.keys("enter");
    assert!(h.has("chip:dev"));
    send_click(&mut h);
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
    let mut h = boot(cx, opts);
    h.focus_composer();
    type_wait(&mut h, "review @Notes");
    assert!(h.has("mention:Notes"));
    h.keys("enter");
    assert!(h.has("chip:Notes"));
    send_click(&mut h);
    assert!(texts(&sent)[0].contains("@artifact:art-notes-1"));
}

// ---- Chat tab persistence ----

fn saved_tab(h: &mut H) -> Option<String> {
    use recur_engine::view::chat_tab_session::{ACTIVE_CHAT_TAB_KEY, ActiveChatTabs};
    h.state.read_with(h.cx, |s, _| {
        ActiveChatTabs::parse(s.storage().get(ACTIVE_CHAT_TAB_KEY))
            .load(WS)
            .map(str::to_string)
    })
}

#[gpui::test]
async fn reopening_a_workspace_after_reload_restores_the_last_active_chat_tab(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, items_opts(Store::default()));
    assert_eq!(h.active(), "MA");
    h.select_tab("PA");
    assert_eq!(h.active(), "PA");
    assert_eq!(saved_tab(&mut h).as_deref(), Some("PA"));
    h.reload(WS);
    assert_eq!(h.active(), "PA");
}

#[gpui::test]
async fn a_newly_opened_chat_tab_stays_selected_after_reload(cx: &mut TestAppContext) {
    let mut h = boot(cx, items_opts(Store::default()));
    h.click("wsv-chat-new");
    assert_eq!(h.codes().len(), 3);
    assert_eq!(h.active(), "PA2");
    assert_eq!(saved_tab(&mut h).as_deref(), Some("PA2"));
    h.reload(WS);
    assert_eq!(h.codes().len(), 3);
    assert_eq!(h.active(), "PA2");
}

#[gpui::test]
async fn dragging_a_chat_tab_reorders_the_strip_and_survives_reload(cx: &mut TestAppContext) {
    let saved = calls();
    let s2 = saved.clone();
    let opts = items_opts(Store::default()).resolver("ReorderWorkspaceChats", move |a, s, _| {
        s2.lock().push(a.to_vec());
        s.chat_order_by_loop.insert(
            a[0].as_str().unwrap().into(),
            serde_json::from_value(a[1].clone()).unwrap(),
        );
        Reply::null()
    });
    let mut h = boot(cx, opts);
    h.click("wsv-chat-new");
    assert_eq!(h.codes(), ["MA", "PA", "PA2"]);
    let (pa, ma) = (h.chip("PA").expect("PA"), h.chip("MA").expect("MA"));
    let from = h.bounds(&pa).expect("PA").center();
    let to = h.bounds(&ma).expect("MA");
    h.cx.simulate_mouse_move(from, None, Modifiers::none());
    h.cx.simulate_mouse_down(from, gpui::MouseButton::Left, Modifiers::none());
    for i in 1..=12 {
        let x = from.x + (to.origin.x + px(8.) - from.x) * (i as f32 / 12.);
        h.cx.simulate_mouse_move(
            point(x, from.y),
            Some(gpui::MouseButton::Left),
            Modifiers::none(),
        );
        h.cx.run_until_parked();
    }
    let end = point(to.origin.x + px(8.), from.y);
    h.cx.simulate_mouse_up(end, gpui::MouseButton::Left, Modifiers::none());
    h.wait(400);
    assert_eq!(h.codes()[..2], ["PA", "MA"]);
    assert_eq!(saved.lock()[0][1][0], json!("PA"));
    h.reload(WS);
    assert_eq!(h.codes()[..2], ["PA", "MA"]);
}

#[gpui::test]
async fn switching_workspaces_restores_agent_tab_strip_scroll(cx: &mut TestAppContext) {
    let mut h = boot(cx, items_opts(Store::default()));
    for _ in 0..8 {
        h.click("wsv-chat-new");
    }
    assert_eq!(h.codes().len(), 10);
    // The Loop chip has scrolled out of view; Playwright scrolls it back before clicking.
    h.with(|v, cx| v.select_chat_tab("MA", cx));
    h.wait(400);
    assert_eq!(h.active(), "MA");
    let handle = h.read(|v, _| v.strip_scroll.clone());
    let max = f32::from(handle.max_offset().x);
    assert!(max > 60., "max {max}");
    let left = 120f32.min(max.max(40.));
    handle.set_offset(point(px(-left), px(0.)));
    h.settle();
    h.open("LOOP-2229");
    assert_ne!(h.codes().len(), 10);
    h.open(WS);
    assert_eq!(h.codes().len(), 10);
    let handle = h.read(|v, _| v.strip_scroll.clone());
    assert_eq!(-f32::from(handle.offset().x), left);
    let last = h.codes().last().cloned().unwrap();
    // Playwright scrolls the clipped chip into view before clicking; select it directly.
    h.with(|v, cx| v.select_chat_tab(&last, cx));
    h.wait(400);
    let strip = h.read(|v, _| v.strip_scroll.bounds());
    let sel = h.chip(&last).expect("chip");
    let chip = h.bounds(&sel).expect("chip");
    let visible = f32::from(chip.right().min(strip.right()) - chip.left().max(strip.left()));
    assert!(visible > 24., "visible {visible}");
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
    tall(
        (recur_view_chat::window::CHAT_HYDRATE_ALL_BELOW + 4).div_ceil(2),
        &"line ".repeat(6),
        &"paragraph ".repeat(18),
    )
}

fn tool_head() -> String {
    format!("tc-head:tc-{WS}\tMA-1")
}

fn tool_open(h: &mut H) -> bool {
    let chat = h.chat();
    chat.read_with(h.cx, |c, _| {
        c.test_open_rows().get("1").copied().unwrap_or(false)
    })
}

fn window_chat(h: &mut H) -> bool {
    let chat = h.chat();
    chat.read_with(h.cx, |c, _| c.test_window_chat())
}

#[gpui::test]
async fn expanded_tool_calls_stay_expanded_after_switching_chat_tabs(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", tool_items());
    let head = tool_head();
    assert!(h.has(&head));
    assert!(!tool_open(&mut h));
    h.click(&head);
    assert!(tool_open(&mut h));
    h.select_tab("PA");
    assert!(!h.has(&head));
    h.select_tab("MA");
    assert!(h.has(&head));
    assert!(tool_open(&mut h));
}

#[gpui::test]
async fn expanded_tool_calls_stay_expanded_after_switching_workspaces(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", tool_items());
    let head = tool_head();
    h.click(&head);
    assert!(tool_open(&mut h));
    h.open("LOOP-2229");
    assert!(!h.has(&head));
    h.open(WS);
    assert!(h.has(&head));
    assert!(tool_open(&mut h));
}

fn leave_mid_scroll(h: &mut H, top: f32) -> f32 {
    assert!(max_scroll(h) > top + 80.);
    scroll_to(h, top);
    h.settle();
    let chat = h.chat();
    assert!(!chat.read_with(h.cx, |c, _| c.test_near_bottom()));
    assert!(dist_from_bottom(h) > 80.);
    scroll_top(h)
}

fn assert_restored_mid(h: &mut H, saved: f32) {
    let top = scroll_top(h);
    assert!((top - saved).abs() < 80., "restored {top} vs saved {saved}");
    assert!(dist_from_bottom(h) > 80.);
}

#[gpui::test]
async fn scroll_offset_is_restored_after_switching_chat_tabs(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", tall_transcript());
    assert!(h.me("question 0"));
    let saved = leave_mid_scroll(&mut h, 360.);
    h.select_tab("PA");
    assert!(!h.me("question 0"));
    h.select_tab("MA");
    assert!(h.me("question 0"));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn scroll_offset_is_restored_after_switching_tabs_on_a_windowed_transcript(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", windowed_transcript());
    assert!(window_chat(&mut h));
    let saved = leave_mid_scroll(&mut h, 360.);
    h.select_tab("PA");
    assert!(!window_chat(&mut h));
    h.select_tab("MA");
    assert!(window_chat(&mut h));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn scroll_offset_is_restored_after_hiding_the_agent_panel_on_a_windowed_transcript(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", windowed_transcript());
    let saved = leave_mid_scroll(&mut h, 360.);
    h.click("obs-left-toggle");
    assert!(!h.has("obs-left"));
    h.click("obs-left-toggle");
    assert!(h.has("obs-left"));
    assert!(window_chat(&mut h));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn a_small_scroll_up_from_the_tail_is_not_pulled_back_on_a_windowed_transcript(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(
        &mut h,
        &store,
        "MA",
        tall(
            (recur_view_chat::window::CHAT_HYDRATE_ALL_BELOW + 4).div_ceil(2),
            &"line\n".repeat(8),
            &"paragraph\n".repeat(16),
        ),
    );
    assert!(window_chat(&mut h));
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
    let top = scroll_top(&mut h);
    scroll_to(&mut h, (top - 40.).max(0.));
    h.wait(50);
    assert!(dist_from_bottom(&mut h) > 20.);
}

#[gpui::test]
async fn scroll_offset_is_restored_after_switching_workspaces(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", tall_transcript());
    let saved = leave_mid_scroll(&mut h, 360.);
    h.open("LOOP-2229");
    assert!(!h.bodies().iter().any(|b| b.starts_with("question 0")));
    h.open(WS);
    assert!(h.me("question 0"));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn parked_workspace_chat_stays_live_and_only_its_owner_reloads_on_idle(
    cx: &mut TestAppContext,
) {
    let reloads = calls();
    let r2 = reloads.clone();
    let text = Arc::new(Mutex::new("initial owned transcript".to_string()));
    let t2 = text.clone();
    let opts = MockOptions::default()
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
        .resolver("WorkspaceChatItems", move |a, _, _| {
            r2.lock().push(a.to_vec());
            Reply::value(json!([{"role": "assistant", "kind": "text", "text": t2.lock().clone()}]))
        });
    let mut h = boot(cx, opts);
    assert!(h.assistant("initial owned transcript"));
    // Parked: another workspace on screen; this shell stays mounted and fed.
    h.open("LOOP-2229");
    working(&mut h, "MA");
    delta(&mut h, "MA", "parked live reply", 1);
    *text.lock() = "settled owned transcript".into();
    reloads.lock().clear();
    idle(&mut h, "MA");
    let got: Vec<Vec<Value>> = reloads.lock().clone();
    assert!(got.contains(&vec![json!(WS), json!("MA")]), "{got:?}");
    h.open(WS);
    assert!(h.assistant("settled owned transcript"));
}

#[gpui::test]
async fn a_bottom_pinned_in_flight_chat_stays_on_the_tail_after_growth_while_parked(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, hang_opts(store.clone(), calls()));
    load(&mut h, &store, "MA", tall_transcript());
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
    h.fill("keep going");
    send_click(&mut h);
    assert!(h.has("chat-stop"));
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
    h.open("LOOP-2229");
    let grown = format!("grown-tail {}", "paragraph ".repeat(40));
    working(&mut h, "MA");
    delta(&mut h, "MA", &grown, 1);
    h.open(WS);
    assert!(h.bodies().iter().any(|b| b.starts_with("grown-tail")));
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
}

#[gpui::test]
async fn a_bottom_pinned_transcript_stays_pinned_after_switching_chat_tabs(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", tall_transcript());
    assert!(max_scroll(&mut h) > 400.);
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
    h.select_tab("PA");
    h.select_tab("MA");
    assert!(h.me("question 0"));
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
}

#[gpui::test]
async fn a_mid_scrolled_shorter_tab_is_not_pinned_when_entered_from_a_longer_tab(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    let long = tall_transcript();
    let short = Value::Array(long.as_array().unwrap()[..24].to_vec());
    load(&mut h, &store, "MA", long);
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
    store
        .lock()
        .insert("PA".into(), short.as_array().cloned().unwrap());
    h.select_tab("PA");
    assert!(h.me("question 0"));
    let saved = leave_mid_scroll(&mut h, 320.);
    h.select_tab("MA");
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
    h.select_tab("PA");
    assert!(h.me("question 0"));
    assert_restored_mid(&mut h, saved);
}

#[gpui::test]
async fn transcript_fades_an_edge_only_while_more_content_sits_past_it(cx: &mut TestAppContext) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", tall_transcript());
    assert!(max_scroll(&mut h) > 400.);
    assert!(dist_from_bottom(&mut h) < recur_view_chat::transcript_ui::NEAR_BOTTOM_PX);
    assert!(h.has("fade-top") && !h.has("fade-bot"));
    scroll_to(&mut h, 0.);
    assert!(!h.has("fade-top") && h.has("fade-bot"));
    leave_mid_scroll(&mut h, 360.);
    assert!(h.has("fade-top") && h.has("fade-bot"));
}

// ---- Turn-end findings digest ----

fn finding_transcript(footer: bool) -> Value {
    let input = json!({"file": "store.go", "line": 1276, "end_line": 1286, "severity": "high",
        "message": "activity hook can miss a turn", "summary": "Hook misses turn"})
    .to_string();
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
async fn renders_the_findings_summary_widget_after_a_codex_style_report_finding_turn(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", finding_transcript(true));
    assert!(h.has("turn-end"));
    assert!(h.has("turn-end-link:1 finding"));
    assert!(h.has("findings-filed"));
    let msgs = h.messages();
    let chat = h.chat();
    let digest = chat.read_with(h.cx, |c, _| c.test_turn_digest(&msgs, msgs.len() - 1));
    assert_eq!(digest[0], "activity hook can miss a turn");
}

#[gpui::test]
async fn does_not_render_the_findings_widget_when_the_turn_footer_is_missing(
    cx: &mut TestAppContext,
) {
    let store = Store::default();
    let mut h = boot(cx, items_opts(store.clone()));
    load(&mut h, &store, "MA", finding_transcript(false));
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
    let forked = calls();
    let f2 = forked.clone();
    let opts = MockOptions::default()
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
        .resolver("WorkspaceChatItems", |a, _, _| {
            Reply::value(if a[1] == json!("PA2") {
                json!([])
            } else {
                fork_transcript()
            })
        })
        .resolver("ForkWorkspaceChat", move |a, s, _| {
            f2.lock().push(a.to_vec());
            let ws = a[0].as_str().unwrap_or_default().to_string();
            let tab = json!({"code": "PA2", "title": "ship it", "updatedAt": 1});
            let list = s.chats_by_loop.entry(ws).or_default();
            list.retain(|t| t["code"] != json!("PA2"));
            list.push(tab.clone());
            Reply::value(tab)
        });
    let mut h = boot(cx, opts);
    assert!(h.has("turn-end"));
    h.click("turn-end-fork");
    h.click("fork-item:Fork chat");
    assert_eq!(forked.lock()[0][2], json!("msg-turn-1"));
    assert!(h.chip("PA2").is_some(), "codes {:?}", h.codes());
    assert_eq!(h.active(), "PA2");
    assert!(!h.has("fork-item:Fork chat"));
}

#[gpui::test]
async fn fork_to_new_workspace_opens_the_created_workspace(cx: &mut TestAppContext) {
    let opts = MockOptions::default()
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
        .resolver("WorkspaceChatItems", |a, _, _| {
            Reply::value(if a[0] == json!("WS-FORK") {
                json!([])
            } else {
                fork_transcript()
            })
        });
    let mut h = boot(cx, opts);
    assert!(h.has("turn-end"));
    h.click("turn-end-fork");
    h.click("fork-item:Fork to new workspace");
    let (selected, listed) = h.state.read_with(h.cx, |s, _| {
        (
            s.selected_workspace_id.clone(),
            s.workspaces.iter().any(|w| w.id == "WS-FORK"),
        )
    });
    assert!(listed);
    assert_eq!(selected, "WS-FORK");
}

// ---- External markdown links ----

#[gpui::test]
async fn http_links_in_agent_output_open_in_the_os_browser(cx: &mut TestAppContext) {
    let opts = bridge_opts(Store::default(), calls(), Pending::default(), |_, _| {
        Some("See [the docs](https://example.com/guide) for details.".into())
    });
    let mut h = boot(cx, opts);
    h.fill("link please");
    send_click(&mut h);
    assert!(h.assistant("See [the docs](https://example.com/guide) for details."));
    let chat = h.chat();
    let at =
        h.cx.update(|window, cx| chat.read(cx).test_text_bounds("the docs", window, cx))
            .expect("link bounds")
            .center();
    h.cx.simulate_click(at, Modifiers::none());
    h.settle();
    assert_eq!(
        h.cx.opened_url().as_deref(),
        Some("https://example.com/guide")
    );
}

// ---- maestro-dock.spec.ts ----

#[gpui::test]
async fn the_dock_opens_with_the_loop_tab_sends_a_message_and_toggles_closed(
    cx: &mut TestAppContext,
) {
    let store: Store = Default::default();
    let opts = bridge_opts(store, calls(), Default::default(), |_, _| {
        Some("On it — steering the loop (mock reply).".into())
    });
    let mut h = boot(cx, opts);
    assert!(h.has("obs-left"));
    assert!(
        h.chip("MA").is_some(),
        "the run's chat leads with the pinned Loop tab"
    );
    assert!(h.has("chat-composer"));

    h.click("obs-node:file:src/main.go");
    let editor = h.bounds("obs-editor-pane:main").expect("the editor");
    let dock = h.bounds("obs-left").expect("the dock");
    assert!(
        dock.origin.x < editor.origin.x,
        "the dock sits left of the editor"
    );

    h.fill("steer from dock");
    send_click(&mut h);
    assert!(h.me("steer from dock"));

    h.click("obs-left-toggle");
    assert!(!h.has("obs-left"));
    h.click("obs-left-toggle");
    assert!(h.has("obs-left"));
    assert!(
        h.me("steer from dock"),
        "the transcript survives the toggle"
    );
}

#[gpui::test]
async fn the_chat_panel_can_open_on_the_right_when_configured(cx: &mut TestAppContext) {
    let mut h = boot_with(
        cx,
        |_| {},
        MockOptions::default(),
        |st| {
            st.set(
                "recur.codePanelSides",
                r#"{"maestro":"right","files":"left","outline":"right","search":"right","refs":"right"}"#,
            )
        },
        WS,
    );
    let center = h.bounds("obs-tb-center").expect("the toolbar center");
    let toggle = h.bounds("panel-toggle:maestro").expect("the Agent toggle");
    assert!(
        toggle.origin.x > center.origin.x,
        "the chat toggle lives in the right toolbar"
    );

    h.click("panel-toggle:maestro");
    let side = h.bounds("obs-side").expect("the right panel");
    let composer = h.bounds("chat-composer").expect("the composer");
    assert!(side.contains(&composer.center()));

    h.click("panel-toggle:files");
    assert!(h.has("obs-left"));
    h.click("obs-node:file:src/main.go");
    let editor = h.bounds("obs-editor-pane:main").expect("the editor");
    let side = h.bounds("obs-side").expect("the right panel");
    assert!(
        side.origin.x > editor.origin.x,
        "the chat pane sits right of the editor"
    );
}
