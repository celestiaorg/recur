//! The Inbox (Briefing with its Ask Recur dock) on the whole shell.

use gpui::TestAppContext;
use recur_domain::ChatItem;
use recur_services::AppEvent;
use recur_services::events::AgentUpdated;
use recur_services::fake::Overrides;
use recur_view_briefing::briefing::BriefingView;
use recur_view_core::{LocalStorage, ViewName};
use serde_json::json;

use super::harness::{S, boot, plain, with};

fn app_agent(status: &str) -> AppEvent {
    AppEvent::AgentUpdated(AgentUpdated {
        workspace_id: String::new(),
        agent_id: "app-APP".into(),
        status: status.into(),
        surface: "app".into(),
        code: "APP".into(),
    })
}

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

/// Drags the list wide enough for its full rows and bucket chips beside the overview.
fn full_list(s: &mut S) {
    s.state.update(s.cx, |st, cx| {
        st.set_panel(recur_view_core::prefs::PanelKey::InboxList, 700., cx)
    });
    s.settle();
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
    assert!(
        s.has(&format!("ov-title:{RATE}")),
        "the first row opens in the overview"
    );
    assert!(
        !s.has(&seg("All", true)),
        "a narrow list drops the bucket chips"
    );
    let diffed: Vec<String> = s.state.read_with(s.cx, |st, _| {
        let briefs = st.briefing.workspaces.clone().unwrap_or_default();
        briefs
            .into_iter()
            .filter(|b| b.diff_add + b.diff_del > 0)
            .map(|b| b.title)
            .collect()
    });
    assert!(
        diffed.iter().any(|t| s.has(&format!("wbrief-diff:{t}"))),
        "a narrow row keeps its diff"
    );
    full_list(&mut s);
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
    assert!(unread(&mut s, RATE).is_none(), "the open row is read");
    assert!(unread(&mut s, SIGN_IN).is_some_and(|u| u.starts_with("2:")));
    s.click(&row(SIGN_IN));
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(s.has(&format!("ov-title:{SIGN_IN}")));
    assert!(unread(&mut s, SIGN_IN).is_none());
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
    full_list(&mut s);
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
    s.click(&row(SIGN_IN));
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
    s.dispatch(app_agent("working"));
    assert!(busy(&mut s, "APP"));
    s.click("ask-new");
    assert!(busy(&mut s, "APP"), "the primary keeps its running mark");
    s.dispatch(app_agent("idle"));
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
    full_list(&mut s);
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
    full_list(&mut s);
    s.click(&seg("Working", false));
    assert!(!s.has("briefing-needs"));
    s.click("foot-needs");
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(s.has("briefing-needs"));
}

#[gpui::test]
fn go_to_palette_reaches_briefing(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click("nav-pr");
    assert_eq!(s.view(), ViewName::Pr);
    s.keys("secondary-k");
    assert!(s.has("palette"));
    s.type_text("Inbox");
    s.click("pal-row:Inbox:sel");
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(s.has("briefing-needs"));
}

#[gpui::test]
fn a_brief_row_click_shows_its_overview_and_open_lands_on_the_workspace(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click(&row(RATE));
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(s.has(&format!("ov-title:{RATE}")));
    s.keys("down");
    assert!(s.has("briefing-overview") && !s.has(&format!("ov-title:{RATE}")));
    s.keys("up");
    assert!(s.has(&format!("ov-title:{RATE}")));
    s.keys("escape");
    assert!(
        s.has("briefing-overview"),
        "the wide layout keeps an overview open"
    );
    s.click("ov-open");
    assert_eq!(s.view(), ViewName::Detail);
    s.click("nav-inbox");
    assert!(s.has(&format!("ov-title:{RATE}")));
    s.click(&row(RATE));
    s.keys("enter");
    assert_eq!(s.view(), ViewName::Detail);
}

#[gpui::test]
fn a_question_shows_as_its_card_without_its_notice(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click(&row(SIGN_IN));
    let wid = briefing(&mut s)
        .read_with(s.cx, |v, cx| v.chat().read(cx).viewing().map(str::to_owned))
        .expect("viewing");
    s.click(&row(RATE));
    let note = |id: &str, target: &str| recur_domain::InboxItem {
        id: id.into(),
        group: "needs".into(),
        unread: true,
        kind: "pause".into(),
        title: id.into(),
        target: target.into(),
        workspace_id: wid.clone(),
        ..Default::default()
    };
    let items = [
        note("ib-ask-q1", "question:q1"),
        note("ib-esc-x", &format!("chat:{wid}:MA")),
    ];
    let q: recur_domain::UserQuestion = serde_json::from_value(json!({
        "id": "q1", "workspaceId": wid, "agentCode": "PA", "question": "Ship it?", "createdAt": 1,
    }))
    .unwrap();
    s.state.update(s.cx, |st, cx| {
        st.inbox.extend(items);
        st.user_questions.push(q);
        cx.notify();
    });
    s.click(&row(SIGN_IN));
    assert!(s.has("ov-note:ib-esc-x"), "other paused notices stay");
    assert!(!s.has("ov-note:ib-ask-q1"), "the card below says it");
    s.state.update(s.cx, |st, cx| {
        st.user_questions.clear();
        cx.notify();
    });
    assert!(!s.has("ov-note:ib-ask-q1"), "an answered question is stale");
}

#[gpui::test]
fn ask_recur_knows_the_workspace_open_in_the_overview(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    let viewing = |s: &mut S| {
        briefing(s).read_with(s.cx, |v, cx| v.chat().read(cx).viewing().map(str::to_owned))
    };
    let first = viewing(&mut s).expect("viewing the first row");
    s.click(&row(SIGN_IN));
    let sign_in = viewing(&mut s).expect("viewing the selected workspace");
    assert_ne!(first, sign_in);
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
    let text = |role: &str, text: &str| ChatItem {
        role: role.into(),
        kind: "text".into(),
        text: text.into(),
        ..Default::default()
    };
    let tool = |tool: &str, input: &str, result: serde_json::Value| ChatItem {
        role: "assistant".into(),
        kind: "tool".into(),
        tool: tool.into(),
        input: input.into(),
        result: result.to_string(),
        ..Default::default()
    };
    let transcript = vec![
        text("user", "What needs me?"),
        tool(
            "mcp__recur__list_workspace_briefs",
            "{}",
            json!({ "counts": { "needs": 1, "working": 0, "pickup": 0 }, "workspaces": [brief] }),
        ),
        tool(
            "mcp__recur__get_workspace_context",
            "{\"workspace_id\":\"LOOP-2218\"}",
            json!({ "brief": brief, "planSteps": [{ "id": "S1", "title": "Specify burst vs sustained", "status": "pending" }] }),
        ),
        tool(
            "mcp__recur__present_workspace",
            "{\"target\":\"workspace:LOOP-2218\"}",
            json!({ "ok": true, "target": "workspace:LOOP-2218", "kind": "workspace", "brief": brief }),
        ),
        text("assistant", "Rate-limit is waiting on you."),
        ChatItem {
            role: "assistant".into(),
            kind: "turn".into(),
            duration_ms: 1400,
            ..Default::default()
        },
    ];
    let mut s = with(
        cx,
        |_| {},
        Overrides::default().app_chat_items(move |_, _| Ok(transcript.clone())),
    );
    assert!(s.has("v-briefing:on"));
    // The tool rows' labels ("Listed 1 workspace", "Looked up …", "Presented …") are the
    // chat's (`tools.rs`); here the turn renders and its pinned workspace links out.
    assert!(s.until(|s| s.has("me:What needs me?")));
    assert!(s.has(&format!("turn-end-link:Open {RATE}")));
    s.click(&format!("turn-end-link:Open {RATE}"));
    assert_eq!(s.view(), ViewName::Detail);
    // `WorkspaceBriefCard`: the pinned card opens its primary target too.
    s.go(ViewName::Briefing);
    assert!(s.until(|s| s.has(&format!("wbrief:{RATE}"))));
    s.click(&format!("wbrief:{RATE}"));
    assert_eq!(s.view(), ViewName::Detail);
    assert_eq!(
        s.state
            .read_with(s.cx, |s, _| s.selected_workspace_id.clone()),
        "LOOP-2218"
    );
}

#[gpui::test]
fn the_ask_dock_is_a_drawer_below_1100px_and_escape_closes_it(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, Overrides::default(), LocalStorage::in_memory());
    s.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(800.)));
    s.settle();
    let ask = s.bounds("briefing-ask").expect("drawer");
    let list = s.bounds("briefing-filters").expect("list");
    assert_eq!(ask.origin.x, list.origin.x, "the drawer covers the list");
    s.keys("escape");
    assert!(!s.has("briefing-ask"));
    assert_eq!(s.storage("recur.askRecurOpen").as_deref(), Some("0"));
}

#[gpui::test]
fn the_overview_lists_open_findings_and_opens_the_findings_panel(cx: &mut TestAppContext) {
    let finding = |id: &str, sev: &str, status: &str, msg: &str| recur_domain::Finding {
        id: id.into(),
        severity: sev.into(),
        status: status.into(),
        file: "src/ratelimit.go".into(),
        line: 37,
        message: msg.into(),
        ..Default::default()
    };
    let opts = Overrides::default().list_findings(move |_, _| {
        Ok(vec![
            finding("f1", "low", "open", "attempts keeps an entry for every key"),
            finding("f2", "high", "open", "clientKey trusts X-Forwarded-For"),
            finding("f3", "medium", "resolved", "already fixed"),
        ])
    });
    let mut s = with(cx, |_| {}, opts);
    s.click(&row(RATE));
    assert!(s.has("ov-finding:clientKey trusts X-Forwarded-For"));
    assert!(s.has("ov-finding:attempts keeps an entry for every key"));
    assert!(
        !s.has("ov-finding:already fixed"),
        "resolved findings stay out"
    );
    s.click("ov-finding:clientKey trusts X-Forwarded-For");
    assert_eq!(s.view(), ViewName::Detail);
}

#[gpui::test]
fn ask_recur_mentions_workspaces_and_projects(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click("chat-composer");
    s.type_text("@");
    assert!(s.has("chat-mention"), "the @ menu opens in Ask Recur");
    assert!(s.has("menu-row-on:0"));
    s.keys("enter");
    let chat = briefing(&mut s).read_with(s.cx, |v, cx| v.chat().read(cx).chat().clone());
    let wire = chat.read_with(s.cx, |c, _| c.input().to_owned());
    assert!(
        wire.starts_with("@workspace:") || wire.starts_with("@project:"),
        "{wire}"
    );
    assert!(!s.has("chat-mention"), "picking closes the menu");
}

#[gpui::test]
fn arrow_keys_scroll_the_selected_row_into_view(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.cx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(360.)));
    s.settle();
    let titles: Vec<String> = s.state.read_with(s.cx, |st, _| {
        let briefs = st.briefing.workspaces.clone().unwrap_or_default();
        briefs.into_iter().map(|b| b.title).collect()
    });
    let first = titles
        .iter()
        .find(|t| s.has(&row(t)))
        .cloned()
        .expect("a row");
    s.click(&row(&first));
    let mut scrolled = false;
    for _ in 0..titles.len() {
        s.keys("down");
        let view = s.bounds("briefing-scroll").expect("list");
        let sel = titles
            .iter()
            .find(|t| s.has(&format!("ov-title:{t}")))
            .cloned()
            .expect("selected");
        let r = s.bounds(&row(&sel)).expect("row");
        assert!(
            r.top() >= view.top() && r.bottom() <= view.bottom(),
            "{sel} is out of view"
        );
        scrolled |= s.bounds(&row(&first)).is_none_or(|f| f.top() < view.top());
    }
    assert!(scrolled, "the list never had to scroll");
    for _ in 0..titles.len() {
        s.keys("up");
        let view = s.bounds("briefing-scroll").expect("list");
        let sel = titles
            .iter()
            .find(|t| s.has(&format!("ov-title:{t}")))
            .cloned()
            .expect("selected");
        let r = s.bounds(&row(&sel)).expect("row");
        assert!(
            r.top() >= view.top() + gpui::px(26.),
            "{sel} is under the pinned header"
        );
    }
}

#[gpui::test]
fn the_list_beside_the_overview_resizes_and_keeps_its_width(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click(&row(RATE));
    let before = s.bounds("briefing-overview").expect("overview").left();
    let p = gpui::point(before - gpui::px(2.), gpui::px(300.));
    let to = gpui::point(before + gpui::px(78.), gpui::px(300.));
    s.cx.simulate_mouse_move(p, None, gpui::Modifiers::none());
    s.cx.simulate_mouse_down(p, gpui::MouseButton::Left, gpui::Modifiers::none());
    s.cx.simulate_mouse_move(to, gpui::MouseButton::Left, gpui::Modifiers::none());
    s.cx.simulate_mouse_up(to, gpui::MouseButton::Left, gpui::Modifiers::none());
    s.settle();
    let after = s.bounds("briefing-overview").expect("overview").left();
    assert_eq!(after, before + gpui::px(80.));
    s.reload();
    s.click(&row(RATE));
    assert_eq!(
        s.bounds("briefing-overview").expect("overview").left(),
        after
    );
}

#[gpui::test]
fn bucket_headers_stay_pinned_while_the_list_scrolls(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.cx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(360.)));
    s.settle();
    assert!(!s.has("briefing-sticky:needs"), "nothing pins at rest");
    let view = s.bounds("briefing-scroll").expect("list");
    s.cx.simulate_event(gpui::ScrollEvent {
        position: view.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-40.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    });
    let head = s.bounds("briefing-sticky:needs").expect("pinned header");
    assert_eq!(head.top(), view.top());
}

#[gpui::test]
fn footer_needs_you_scrolls_a_scrolled_list_back_to_the_bucket(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.cx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(360.)));
    s.settle();
    let view = s.bounds("briefing-scroll").expect("list");
    s.cx.simulate_event(gpui::ScrollEvent {
        position: view.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-60.))),
        ..Default::default()
    });
    assert!(s.bounds("briefing-needs").expect("needs").top() < view.top());
    s.click("foot-needs");
    assert_eq!(s.bounds("briefing-needs").expect("needs").top(), view.top());
}

#[gpui::test]
fn the_overview_marks_its_workspace_unread_and_read_again(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click(&row(SIGN_IN));
    s.click("ov-read:read");
    assert!(s.has("ov-read:unread"));
    s.click(&row(RATE));
    s.click(&seg("Unread", false));
    assert!(
        s.has(&row(SIGN_IN)),
        "marked unread, it is in the Unread list"
    );
    assert!(s.has(&row(RATE)), "the open row stays in the Unread list");
    s.click(&row(SIGN_IN));
    assert!(s.has("ov-read:read"), "opening it reads it");
    s.keys("secondary-shift-u");
    assert!(s.has("ov-read:unread"));
    s.keys("secondary-shift-u");
    assert!(s.has("ov-read:read"));
}

#[gpui::test]
fn a_narrow_window_opens_the_overview_only_on_a_click(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, Overrides::default(), LocalStorage::in_memory());
    s.cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(800.)));
    s.settle();
    s.click("ask-hide");
    assert!(!s.has("briefing-overview"));
    s.click(&row(RATE));
    assert!(s.has("briefing-overview"));
    s.keys("escape");
    assert!(!s.has("briefing-overview"));
}
