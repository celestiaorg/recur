//! `briefing.spec.ts`: the Inbox (Briefing with its Ask Recur dock) on the whole shell.

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply};
use recur_view_briefing::briefing::BriefingView;
use recur_view_core::{LocalStorage, ViewName};
use serde_json::json;

use super::harness::{S, boot, plain, with};

const RATE: &str = "Rate-limit the public API";
const SIGN_IN: &str = "Add Apple & Google sign-in";
const PG16: &str = "Migrate analytics to PG16";

fn row(t: &str) -> String {
    format!("wbrief-row:{t}")
}

fn seg(label: &str, on: bool) -> String {
    format!("seg:{label}:{on}")
}

fn unread(s: &mut S, title: &str) -> Option<String> {
    for n in 1..10 {
        for tip in pill_tips(s, title) {
            let sel = format!("wbrief-unread:{title}={n}:{tip}");
            if s.has(&sel) {
                return Some(format!("{n}:{tip}"));
            }
        }
    }
    None
}

/// The pill's tooltips a workspace can have: its unread notification titles joined, or the
/// count line.
fn pill_tips(s: &mut S, title: &str) -> Vec<String> {
    let (inbox, briefs) = s.state.read_with(s.cx, |st, _| {
        (
            st.inbox.clone(),
            st.briefing.workspaces.clone().unwrap_or_default(),
        )
    });
    let Some(b) = briefs.iter().find(|b| b.title == title) else {
        return Vec::new();
    };
    let notes: Vec<String> = inbox
        .iter()
        .filter(|i| i.unread && i.workspace_id == b.workspace_id)
        .map(|i| {
            let mut out = String::new();
            let mut tag = false;
            for c in i.title.chars() {
                match c {
                    '<' => tag = true,
                    '>' => tag = false,
                    c if !tag => out.push(c),
                    _ => {}
                }
            }
            out
        })
        .collect();
    let n = notes.len();
    vec![
        notes.join(" · "),
        format!("{n} unread notification{}", if n == 1 { "" } else { "s" }),
    ]
}

fn briefing(s: &mut S) -> gpui::Entity<BriefingView> {
    let shell = s.root.read_with(s.cx, |r, _| r.shell.clone());
    s.cx.update(|_, cx| {
        shell
            .read(cx)
            .route_view(ViewName::Briefing)
            .and_then(|v| v.downcast::<BriefingView>().ok())
            .expect("briefing view")
    })
}

#[gpui::test]
fn boots_into_the_unified_list_with_now_next_cards_and_ask_recur(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert!(s.has("v-briefing:on"));
    assert!(s.has("nav-inbox"));
    assert!(s.has(&seg("All", true)));
    let sub = s
        .state
        .read_with(s.cx, |st, _| st.inbox.iter().filter(|i| i.unread).count());
    assert!(sub > 0 && s.has(&format!("tb-sub=1 needs you · {sub} unread")));
    assert!(s.has("briefing-needs"));
    assert!(s.has(&row(RATE)));
    assert!(s.has("briefing-ask"));
    for chip in [
        "What needs me?",
        "Where did I leave off?",
        "Which workspace should I open next?",
    ] {
        assert!(s.has(&format!("ask-chip:{chip}")), "{chip}");
    }
}

#[gpui::test]
fn unread_pill_recalls_its_notification_titles_on_hover(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    let got = unread(&mut s, SIGN_IN).expect("pill");
    assert!(got.starts_with("2:"), "{got}");
    assert!(got.contains("#482 is ready for review"), "{got}");
}

#[gpui::test]
fn notifications_with_no_card_land_in_elsewhere(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert!(s.has("briefing-elsewhere"));
    assert!(s.has("ibrow:<b>#471</b> merged to main:read"));
}

#[gpui::test]
fn cards_carry_unread_badges_that_clear_on_open(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert!(unread(&mut s, RATE).is_some_and(|u| u.starts_with("1:")));
    assert!(unread(&mut s, SIGN_IN).is_some_and(|u| u.starts_with("2:")));
    s.click(&row(RATE));
    assert_eq!(s.view(), ViewName::Detail);
    s.click("nav-inbox");
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(unread(&mut s, RATE).is_none());
}

#[gpui::test]
fn mark_all_read_clears_every_badge(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click("tb-mark-read");
    assert!(unread(&mut s, RATE).is_none());
    assert!(unread(&mut s, SIGN_IN).is_none());
    assert!(!s.has("tb-mark-read"));
}

#[gpui::test]
fn bucket_chips_filter_the_overview(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert!(s.has("briefing-working"));
    s.click(&seg("Needs you", false));
    assert!(!s.has("briefing-working"));
    assert!(!s.has("briefing-pickup"));
    assert!(s.has("briefing-needs"));
    s.click(&seg("All", false));
    assert!(s.has("briefing-working"));
}

#[gpui::test]
fn unread_filter_narrows_to_workspaces_with_unread_notifications(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert!(s.has(&row(PG16)));
    s.click(&seg("Unread", false));
    assert!(!s.has(&row(PG16)));
    assert!(s.has(&row(SIGN_IN)));
    assert!(s.has(&row(RATE)));
}

#[gpui::test]
fn project_filter_scopes_the_overview(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert!(s.has("sel:Filter by project=All projects"));
    s.click("sel:Filter by project=All projects");
    s.click("sel-opt:payments");
    assert!(s.has("sel-menu"), "the multi-select menu stays open");
    assert!(s.has(&row(RATE)));
    assert!(!s.has(&row(SIGN_IN)));
    assert!(!s.has("briefing-working"));
    s.click("sel-opt:storefront");
    assert!(s.has(&row(SIGN_IN)));
    s.keys("escape");
    assert!(s.has("sel:Filter by project=payments, storefront"));
}

#[gpui::test]
fn ask_dock_toggles_from_the_pane_headers_like_the_pr_dock(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert!(s.has("briefing-ask"));
    let ask = s.bounds("briefing-ask").expect("dock");
    let list = s.bounds("briefing-filters").expect("list");
    assert!(
        ask.origin.x < list.origin.x,
        "the dock sits left of the list"
    );
    s.click("ask-hide");
    assert!(!s.has("briefing-ask"));
    s.click("ask-reopen");
    assert!(s.has("briefing-ask"));
    assert!(!s.has("ask-reopen"));
}

#[gpui::test]
fn ask_recur_selected_tab_lights_when_the_chat_has_focus(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    let b = briefing(&mut s);
    let app = s.cx.update(|_, cx| b.read(cx).chat().clone());
    let lit = |s: &mut S| {
        let app = app.clone();
        s.cx.update(|window, cx| app.read(cx).lit(window, cx))
    };
    assert!(s.has("tab-chip:ask-tab-APP:on"));
    assert!(!lit(&mut s));
    s.click("chat-composer");
    assert!(lit(&mut s));
    s.click(&seg("All", true));
    assert!(s.has("tab-chip:ask-tab-APP:on"));
    assert!(!lit(&mut s));
    s.click("ask-new");
    assert!(!s.has("tab-chip:ask-tab-APP:on"));
    assert!(lit(&mut s), "the new tab is lit");
}

#[gpui::test]
fn ask_recur_tabs_show_running_status_like_workspace_chats(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    let b = briefing(&mut s);
    let app = s.cx.update(|_, cx| b.read(cx).chat().clone());
    let busy = |s: &mut S, code: &str| {
        let app = app.clone();
        let code = code.to_string();
        s.cx.update(|_, cx| app.read(cx).tab_busy(&code, cx))
    };
    s.click("chat-composer");
    s.type_text("what needs me");
    s.keys("enter");
    assert!(s.until(|s| s.has("me:what needs me")));
    assert!(
        !busy(&mut s, "APP"),
        "a reply that returns at once drops busy"
    );
    s.wire(
        "agent.updated",
        json!({ "workspaceId": "", "agentId": "app-APP", "status": "working", "surface": "app", "code": "APP" }),
    );
    assert!(busy(&mut s, "APP"));
    s.click("ask-new");
    assert!(busy(&mut s, "APP"), "the primary keeps its running mark");
    s.wire(
        "agent.updated",
        json!({ "workspaceId": "", "agentId": "app-APP", "status": "idle", "surface": "app", "code": "APP" }),
    );
    assert!(!busy(&mut s, "APP"));
}

#[gpui::test]
fn ask_dock_supports_multiple_chat_tabs_like_the_pr_dock(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert!(s.has("tab-chip:ask-tab-APP:on"));
    s.click("ask-new");
    assert!(s.has("tab-chip:ask-tab-APP"));
    assert!(s.has("tab-chip:ask-tab-AP2:on"));
    s.hover("tab-chip:ask-tab-AP2:on");
    s.click("tab-close:ask-tab-AP2");
    assert!(!s.has("tab-chip:ask-tab-AP2"));
    assert!(s.has("tab-chip:ask-tab-APP:on"));
    s.hover("tab-chip:ask-tab-APP:on");
    s.click("tab-close:ask-tab-APP");
    assert!(
        s.has("tab-chip:ask-tab-APP:on"),
        "the primary resets to a fresh Ask Recur"
    );
}

#[gpui::test]
fn unread_and_project_filters_survive_a_reload_bucket_does_not(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click(&seg("Unread", false));
    s.click(&seg("Needs you", false));
    s.reload();
    assert!(s.has(&seg("Unread", true)));
    assert!(s.has(&seg("All", true)));
    assert!(!s.has(&row(PG16)));
}

#[gpui::test]
fn restores_briefing_from_shell_state(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click("nav-pr");
    assert_eq!(s.view(), ViewName::Pr);
    s.click("nav-inbox");
    s.reload();
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(s.has("briefing-needs"));
}

#[gpui::test]
fn footer_needs_you_opens_the_list_focused_clearing_bucket_filters(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click(&seg("Working", false));
    assert!(!s.has("briefing-needs"));
    s.click("foot-needs");
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(s.has("briefing-needs"));
}

// `go-to palette reaches Briefing` waits for `CommandPalette.svelte`'s port (the ⌘K palette).

#[gpui::test]
fn opening_a_brief_row_lands_on_the_workspace(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click(&row(RATE));
    assert_eq!(s.view(), ViewName::Detail);
}

#[gpui::test]
fn ask_recur_briefing_tools_skip_json_only_present_workspace_is_a_card(cx: &mut TestAppContext) {
    let brief = json!({
        "workspaceId": "LOOP-2218",
        "projectName": "payments",
        "title": RATE,
        "branch": "recur/rate-limit",
        "bucket": "needs",
        "reasons": ["escalation", "plan_approval"],
        "now": "Escalated: ambiguous burst-limit spec",
        "next": "Answer the question",
        "primary": { "label": "Answer", "target": "workspace:LOOP-2218" },
    });
    let transcript = json!([
        { "role": "user", "kind": "text", "text": "What needs me?" },
        {
            "role": "assistant", "kind": "tool",
            "tool": "mcp__recur__list_workspace_briefs", "input": "{}",
            "result": json!({ "counts": { "needs": 1, "working": 0, "pickup": 0 }, "workspaces": [brief] }).to_string(),
        },
        {
            "role": "assistant", "kind": "tool",
            "tool": "mcp__recur__get_workspace_context", "input": "{\"workspace_id\":\"LOOP-2218\"}",
            "result": json!({ "brief": brief, "planSteps": [{ "id": "S1", "title": "Specify burst vs sustained", "status": "pending" }] }).to_string(),
        },
        {
            "role": "assistant", "kind": "tool",
            "tool": "mcp__recur__present_workspace", "input": "{\"target\":\"workspace:LOOP-2218\"}",
            "result": json!({ "ok": true, "target": "workspace:LOOP-2218", "kind": "workspace", "brief": brief }).to_string(),
        },
        { "role": "assistant", "kind": "text", "text": "Rate-limit is waiting on you." },
        { "role": "assistant", "kind": "turn", "durationMs": 1400 },
    ]);
    let mut s = with(
        cx,
        |_| {},
        MockOptions::default().resolver("AppChatItems", move |_, _, _| {
            Reply::value(transcript.clone())
        }),
    );
    assert!(s.has("v-briefing:on"));
    // The tool rows' labels ("Listed 1 workspace", "Looked up …", "Presented …") are the
    // chat's (`tools.rs`); here the turn renders and its pinned workspace links out.
    assert!(s.until(|s| s.has("me:What needs me?")));
    assert!(s.has(&format!("turn-end-link:Open {RATE}")));
    s.click(&format!("turn-end-link:Open {RATE}"));
    assert_eq!(s.view(), ViewName::Detail);
}

#[gpui::test]
fn the_ask_dock_is_a_drawer_below_1100px_and_escape_closes_it(cx: &mut TestAppContext) {
    let mut s = boot(
        cx,
        |_| {},
        MockOptions::default(),
        LocalStorage::in_memory(),
    );
    s.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(800.)));
    s.settle();
    let ask = s.bounds("briefing-ask").expect("drawer");
    let list = s.bounds("briefing-filters").expect("list");
    assert_eq!(ask.origin.x, list.origin.x, "the drawer covers the list");
    s.keys("escape");
    assert!(!s.has("briefing-ask"));
    assert_eq!(s.storage("recur.askRecurOpen").as_deref(), Some("0"));
}
