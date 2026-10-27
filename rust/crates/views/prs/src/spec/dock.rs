//! `pr-review.spec.ts`'s PR agent chat dock: the linked or host workspace's chat beside PR
//! detail, its tabs and hide toggle, attention and unread counts; plus `activity.spec.ts`'s
//! PR-opened fact.

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply};
use recur_view_core::ViewName;
use serde_json::{Value, json};

use super::harness::*;

const LINKED: &str = "LOOP-2231";

fn no_agents() -> MockOptions {
    MockOptions::default().resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
}

/// A transcript `SendWorkspaceMessage` appends to and `WorkspaceChatItems` replays.
fn echo() -> MockOptions {
    let log = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<Value>::new()));
    let l2 = log.clone();
    no_agents()
        .resolver("SendWorkspaceMessage", move |a, _, _| {
            let text = a.get(2).and_then(Value::as_str).unwrap_or("").to_string();
            let mut l = log.lock();
            l.push(json!({"role": "user", "kind": "text", "text": text}));
            l.push(
                json!({"role": "assistant", "kind": "text", "text": format!("reply to: {text}")}),
            );
            Reply::value(json!(format!("reply to: {text}")))
        })
        .resolver("WorkspaceChatItems", move |_, _, _| {
            Reply::value(Value::Array(l2.lock().clone()))
        })
}

fn open_ws(s: &mut S, id: &str) {
    let id = id.to_string();
    s.state
        .update(s.cx, |st, cx| st.open_workspace(&id, false, cx));
    s.settle();
}

fn hide_dock(s: &mut S) {
    s.click("button:Hide left panel");
    assert!(!s.has("obs-left"));
}

#[gpui::test]
fn shows_the_linked_workspace_chat_with_the_same_tabs(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, no_agents());
    open_ws(&mut s, LINKED);
    assert!(s.has("chat-composer"));
    let ws_tabs = s.dock_codes();
    assert!(!ws_tabs.is_empty());

    s.open_pr("482");
    assert!(s.has("prd-agent"));
    assert!(s.has("chat-composer"));
    assert_eq!(s.dock_ws(), LINKED);
    assert_eq!(s.dock_codes(), ws_tabs);
    assert!(s.has("prtab:Conversation:on:lit") || s.has("prtab:Conversation:on"));

    let head = s.bounds("prh").unwrap();
    let split = s.bounds("prd-split").unwrap();
    assert!((f32::from(head.size.width) - f32::from(split.size.width)).abs() < 2.);
    let agent = s.bounds("button:Hide left panel").unwrap();
    let tabs = s.bounds("prtabs").unwrap();
    assert!(
        tabs.contains(&gpui::point(tabs.center().x, agent.center().y)),
        "the dock head sits on the tabs row"
    );
    assert!(agent.right() <= tabs.origin.x + gpui::px(2.));
}

#[gpui::test]
fn selected_agent_chat_tab_stays_in_view_when_opening_a_pr(cx: &mut TestAppContext) {
    let mut s = boot(
        cx,
        |sd| {
            let host = add_host_pr(sd, "gh-tab-scroll", 810, "Tab scroll host");
            let extra: Vec<Value> = (0..8)
                .map(|i| json!({ "code": format!("PA{}", i + 2), "title": format!("Chat {}", i + 2), "updatedAt": 0 }))
                .collect();
            sd.chats_by_loop.insert(host, extra);
        },
        no_agents(),
    );
    s.open_pr("gh-tab-scroll");
    let dock = s.bounds("prd-agent").expect("dock");
    let visible = |s: &mut S, code: &str| {
        let chip = s.chip(code).expect(code);
        let b = s.bounds(&chip).unwrap();
        let overlap =
            f32::from(b.right().min(dock.right())) - f32::from(b.origin.x.max(dock.origin.x));
        overlap > 24.
    };
    // After a round trip through another route, the strip shows its active chip whole.
    let inside = |s: &mut S, code: &str| {
        let chip = s.chip(code).expect(code);
        let b = s.bounds(&chip).unwrap();
        b.left() >= dock.left() && b.right() <= dock.right()
    };
    s.click_chip("PA9");
    assert_eq!(s.dock_active(), "PA9");
    assert!(s.until(|s| visible(s, "PA9")));

    s.click_chip("PA");
    assert_eq!(s.dock_active(), "PA");
    assert!(s.until(|s| visible(s, "PA")));
    hide_dock(&mut s);
    s.click("button:Show left panel");
    assert_eq!(s.dock_active(), "PA");
    assert!(s.until(|s| visible(s, "PA")));

    s.click_chip("PA9");
    assert_eq!(s.dock_active(), "PA9");
    s.click("nav-inbox");
    assert_eq!(s.view(), ViewName::Briefing);
    s.open_pr("gh-tab-scroll");
    assert_eq!(s.dock_active(), "PA9");
    assert!(s.until(|s| inside(s, "PA9")));

    hide_dock(&mut s);
    s.click("nav-inbox");
    s.open_pr("gh-tab-scroll");
    s.click("button:Show left panel");
    assert_eq!(s.dock_active(), "PA9");
    assert!(s.until(|s| inside(s, "PA9")));
}

#[gpui::test]
fn a_message_sent_on_the_pr_is_visible_after_view_workspace(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, echo());
    open_ws(&mut s, LINKED);
    let chat = s_chip_new(&mut s);
    s.click(&chat);
    s.dock_send("from the workspace");
    assert!(s.until(|s| s.me("from the workspace")));

    s.open_pr("482");
    assert!(s.until(|s| s.me("from the workspace")));
    s.dock_send("from the PR");
    assert!(s.until(|s| s.me("from the PR")));

    s.click("tb-view-workspace");
    assert_eq!(s.view(), ViewName::Detail);
    assert!(s.me("from the workspace"));
    assert!(s.me("from the PR"));
}

/// The workspace strip's "New chat" tab.
fn s_chip_new(s: &mut S) -> String {
    let codes = s.dock_codes();
    let fresh = codes
        .iter()
        .find(|c| c.starts_with("PA"))
        .cloned()
        .unwrap_or_else(|| "PA".into());
    s.chip(&fresh).expect("a chat tab")
}

#[gpui::test]
fn nav_and_list_show_when_a_host_agent_is_running_with_no_workspace(cx: &mut TestAppContext) {
    let host_id = std::sync::Arc::new(parking_lot::Mutex::new(String::new()));
    let h2 = host_id.clone();
    let mut s = boot(
        cx,
        move |sd| {
            let host = add_host_pr(sd, "gh-host-run", 808, "Imported checkout review");
            *h2.lock() = host.clone();
            for id in sd.loop_ids().to_vec() {
                let l = sd.obj_mut(id);
                if l["id"] == host.as_str() {
                    continue;
                }
                l["state"] = json!("paused");
                l["phase"] = json!("idle");
                if l["group"] == "needs" {
                    l["group"] = json!("ready");
                }
            }
        },
        {
            let host = host_id.clone();
            MockOptions::default().resolver("ListLoopAgents", move |a, _, _| {
                let id = a.first().and_then(Value::as_str).unwrap_or("");
                if id != host.lock().as_str() {
                    return Reply::value(json!([]));
                }
                Reply::value(json!([{ "id": format!("{id}-PA"), "workspaceId": id, "code": "PA", "status": "working" }]))
            })
        },
    );
    let host = host_id.lock().clone();
    assert!(s.until(|s| s.pr_nav().0 == 1));
    assert!(s.has("nav-pr-sd-dot") || s.pr_nav().0 == 1);
    assert!(
        !s.has(&format!("hit:{host}")),
        "a host stays out of the tree"
    );
    assert!(s.has("foot-prs"));

    s.prs_nav();
    s.seg("All");
    assert!(s.has("prrow-sd:gh-host-run"));
    s.click("prrow:gh-host-run");
    assert!(s.has("obs-left"));
    hide_dock(&mut s);
    assert!(s.has("button:Show left panel. Agent running:running"));

    s.wire(
        "agent.updated",
        json!({ "workspaceId": host, "agentId": format!("{host}-PA"), "status": "idle" }),
    );
    assert!(s.until(|s| s.pr_nav().0 == 0));
    assert!(s.has("button:Show left panel"));
    s.prs_nav();
    assert!(!s.has("prrow-sd:gh-host-run"));
}

#[gpui::test]
fn shows_the_agent_dock_when_the_pr_has_no_workspace(cx: &mut TestAppContext) {
    let mut s = boot(
        cx,
        |sd| patch_pr(sd, "482", json!({ "workspaceId": "" })),
        no_agents(),
    );
    s.open_pr("482");
    assert!(s.has("obs-left"));
    assert!(s.has("chat-composer"));
    assert!(!s.has("tb-view-workspace"));
    assert!(s.has("tb-create-workspace"));
    s.click("tb-create-workspace");
    assert!(s.until(|s| s.has("tb-view-workspace")));
    assert!(s.has("chat-composer"));
}

#[gpui::test]
fn pr_chat_survives_creating_a_workspace(cx: &mut TestAppContext) {
    let mut s = boot(
        cx,
        |sd| patch_pr(sd, "482", json!({ "workspaceId": "" })),
        echo(),
    );
    s.open_pr("482");
    s.dock_send("from the imported PR");
    assert!(s.until(|s| s.me("from the imported PR")));
    s.click("tb-create-workspace");
    assert!(s.until(|s| s.has("tb-view-workspace")));
    assert!(s.until(|s| s.me("from the imported PR")));
    s.click("tb-view-workspace");
    assert_eq!(s.view(), ViewName::Detail);
    assert!(s.until(|s| s.me("from the imported PR")));
}

#[gpui::test]
fn open_filter_includes_other_open_prs_and_needs_you_group(cx: &mut TestAppContext) {
    let mut s = plain_with(cx, |sd| {
        sd.insert_pr(pr_row(json!({
            "id": "gh-storefront-99", "number": 99, "title": "Docs typo", "branch": "docs/typo",
            "state": "Open", "when": "3h",
        })));
    });
    s.prs_nav();
    assert!(s.has("briefing-h:you"));
    assert!(s.has("prrow:gh-storefront-99"));
    assert!(s.has("briefing-h:open"));
}

#[gpui::test]
// Linux: Ctrl+Alt+B from the dock toggle reaches the shell's Ctrl+B (rail), not the Observatory (PARITY follow-up).
#[cfg_attr(not(target_os = "macos"), ignore)]
fn the_chat_panel_can_be_hidden_and_shown_like_the_observatory(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, no_agents());
    open_ws(&mut s, LINKED);
    assert!(s.has("obs-left"));
    s.open_pr("482");
    let far_left = |s: &mut S, sel: &str| {
        let split = s.bounds("prd-split").unwrap();
        let b = s.bounds(sel).unwrap_or_else(|| panic!("{sel}"));
        f32::from(b.origin.x) - f32::from(split.origin.x) < 16.
    };
    assert!(s.has("obs-left"));
    assert!(far_left(&mut s, "button:Hide left panel"));

    s.click("button:Hide left panel");
    assert!(!s.has("obs-left"));
    assert!(s.has("prtab:Conversation:on:lit"));
    assert!(s.has("button:Show left panel"));
    assert!(dock_toggle_focused(&mut s, false));
    assert!(far_left(&mut s, "button:Show left panel"));

    // Hiding the PR dock leaves the workspace's own chat open.
    s.click("tb-view-workspace");
    assert_eq!(s.view(), ViewName::Detail);
    assert!(s.has("obs-left"));
    s.click("tb-view-pr");
    assert_eq!(s.view(), ViewName::PrDetail);
    assert!(!s.has("obs-left"));

    s.click("button:Show left panel");
    assert!(s.has("obs-left"));
    assert!(dock_toggle_focused(&mut s, true));
    assert!(far_left(&mut s, "button:Hide left panel"));

    s.keys("secondary-alt-b");
    assert!(!s.has("obs-left"));
    assert!(dock_toggle_focused(&mut s, false));
    s.keys("secondary-alt-b");
    assert!(s.has("obs-left"));
    assert!(dock_toggle_focused(&mut s, true));

    s.click("composer-textarea");
    let composer_focused = |s: &mut S| {
        s.cx.update(|w, cx| crate::detail::spec_handle::composer_focused(w, cx))
    };
    assert!(composer_focused(&mut s));
    s.keys("secondary-alt-b");
    assert!(!s.has("obs-left"));
    assert!(composer_focused(&mut s));
    s.keys("secondary-alt-b");
    assert!(s.has("obs-left"));
    assert!(composer_focused(&mut s));
}

/// Whether the dock toggle on screen (`Hide` in the dock when `open`, else PR detail's `Show`)
/// holds focus.
fn dock_toggle_focused(s: &mut S, open: bool) -> bool {
    s.cx.update(|w, cx| crate::detail::spec_handle::toggle_focused(open, w, cx))
}

#[gpui::test]
fn a_finish_while_the_pr_dock_is_hidden_raises_unseen(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, no_agents());
    open_ws(&mut s, LINKED);
    assert!(s.has("chat-composer"));
    assert!(s.until(|s| s.attention().focus_ws == LINKED));
    let code = s.attention().focus_code;

    s.open_pr("482");
    assert!(s.has("obs-left"));
    assert!(s.until(|s| !s.attention().occluded));
    hide_dock(&mut s);
    assert!(s.until(|s| s.attention().occluded));

    s.bridge.simulate_chat_turn_idle(LINKED, &code);
    s.settle();
    let unseen = |s: &mut S| {
        s.state.read_with(s.cx, |st, _| {
            st.attention.by_workspace.get(LINKED).copied().unwrap_or(0)
        })
    };
    assert!(s.until(|s| unseen(s) == 1));
    s.prs_nav();
    s.seg("All");
    assert!(s.has("prrow-ct:482=1"));
}

#[gpui::test]
fn a_finish_on_a_host_pr_with_no_workspace_lands_in_inbox(cx: &mut TestAppContext) {
    let host = std::sync::Arc::new(parking_lot::Mutex::new(String::new()));
    let h2 = host.clone();
    let mut s = boot(
        cx,
        move |sd| *h2.lock() = add_host_pr(sd, "gh-host-inbox", 808, "Imported checkout review"),
        no_agents(),
    );
    let host = host.lock().clone();
    s.open_pr("gh-host-inbox");
    assert!(s.has("chat-composer"));
    assert!(s.until(|s| s.attention().focus_ws == host));
    let code = s.attention().focus_code;

    s.click("nav-inbox");
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(s.until(|s| s.attention().occluded));
    s.bridge.simulate_chat_turn_idle(&host, &code);
    s.settle();
    assert!(s.until(|s| s.pr_nav().1 == 1));
    assert!(!s.has(&format!("hit:{host}")));

    s.prs_nav();
    s.seg("All");
    assert_eq!(s.pr_nav().1, 1);
    assert!(s.has("prrow-ct:gh-host-inbox=1"));

    // The Briefing card opens a host's PR (the Briefing view is not ported yet).
    s.state
        .update(s.cx, |st, cx| st.open_pr("gh-host-inbox", cx));
    s.settle();
    assert_eq!(s.view(), ViewName::PrDetail);
    assert!(s.has("obs-left"));
    assert!(s.until(|s| s.attention().focus_ws == host));
    assert!(s.until(|s| s.pr_nav().1 == 0));
}

#[gpui::test]
fn pr_nav_unread_is_the_sum_of_hidden_host_chats_and_counts_down_as_they_are_read(
    cx: &mut TestAppContext,
) {
    let hosts = std::sync::Arc::new(parking_lot::Mutex::new((String::new(), String::new())));
    let h2 = hosts.clone();
    let mut s = boot(
        cx,
        move |sd| {
            let a = add_host_pr(sd, "gh-host-a", 808, "First imported");
            let b = add_host_pr(sd, "gh-host-b", 809, "Second imported");
            *h2.lock() = (a, b);
        },
        no_agents(),
    );
    let (a, b) = hosts.lock().clone();
    s.open_pr("gh-host-a");
    assert!(s.has("chat-composer"));
    assert!(s.until(|s| s.attention().focus_ws == a));
    let code_a = s.attention().focus_code;

    s.click("nav-inbox");
    assert!(s.until(|s| s.attention().occluded));
    s.bridge.simulate_chat_turn_idle(&a, &code_a);
    s.bridge.simulate_chat_turn_idle(&b, "PA");
    s.settle();
    assert!(s.until(|s| s.pr_nav().1 == 2));

    s.prs_nav();
    s.seg("All");
    assert_eq!(s.pr_nav().1, 2);
    assert!(s.has("prrow-ct:gh-host-a=1"));
    assert!(s.has("prrow-ct:gh-host-b=1"));

    s.click("prrow:gh-host-a");
    assert!(s.has("obs-left"));
    assert!(s.until(|s| !s.attention().occluded));
    assert!(s.until(|s| s.pr_nav().1 == 1));

    s.prs_nav();
    assert!(!s.has("prrow-ct:gh-host-a=1"));
    assert!(s.has("prrow-ct:gh-host-b=1"));
    s.click("prrow:gh-host-b");
    assert!(s.until(|s| s.attention().focus_ws == b));
    assert!(s.until(|s| s.pr_nav().1 == 0));
}

#[gpui::test]
fn focusing_the_agent_dock_lights_chat_tabs_and_unlits_the_pr_tab(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, no_agents());
    open_ws(&mut s, LINKED);
    s.open_pr("482");
    let agent_lit = |s: &mut S| s.cx.update(|_, cx| crate::ui::ui(cx).read(cx).focus_agent);
    assert!(s.has("chat-composer"));
    assert!(s.has("prtab:Conversation:on:lit"));
    assert!(!agent_lit(&mut s));

    s.click("chat-composer");
    assert!(agent_lit(&mut s));
    assert!(s.has("prtab:Conversation:on"));

    s.click("prtab:Conversation:on");
    assert!(s.has("prtab:Conversation:on:lit"));
    assert!(!agent_lit(&mut s));

    let code = s.dock_active();
    let chip = s.chip(&code).expect("active chip");
    s.click(&chip);
    assert!(agent_lit(&mut s));
    assert!(s.has("prtab:Conversation:on"));

    hide_dock(&mut s);
    assert!(s.has("prtab:Conversation:on:lit"));
}

#[gpui::test]
fn pr_opened_fact_shows_completed_time_and_opens_the_pr(cx: &mut TestAppContext) {
    let html = "Completed in 12m 04s · <a class=\"loop-pr-link\" data-pr=\"482\">Local PR #482</a><div class=\"loop-wrap\"><b>Done</b> Retry the payment token once</div>";
    let facts = json!([{"id": "f-pr", "agentCode": "MA", "name": "Maestro", "ts": 5_000, "isDeviation": false, "text": html}]);
    let sessions = json!([{
        "id": "s-co", "code": "CO", "name": "Coder", "kind": "coder", "status": "idle",
        "createdAt": 1_000, "updatedAt": 2_000,
        "items": [{"role": "assistant", "kind": "text", "text": "Implemented the rotation fix."}],
    }]);
    let mut s = boot(
        cx,
        |_| {},
        MockOptions::default()
            .resolver("WorkspaceSessions", move |_, _, _| {
                Reply::value(sessions.clone())
            })
            .resolver("WorkspaceFacts", move |_, _, _| Reply::value(facts.clone())),
    );
    let ws = s
        .state
        .read_with(s.cx, |st, _| st.selected_workspace_id.clone());
    let ws = if ws.is_empty() {
        LINKED.to_string()
    } else {
        ws
    };
    open_ws(&mut s, &ws);
    let ma = s.chip("MA").expect("the Loop tab");
    s.click(&ma);
    s.wait(300);
    assert!(s.has("loop-fact:f-pr"));
    s.click("loop-pr-link:482");
    assert_eq!(s.view(), ViewName::PrDetail);
    assert!(s.has("prh-title=Add OAuth provider config + secrets handling"));
}
