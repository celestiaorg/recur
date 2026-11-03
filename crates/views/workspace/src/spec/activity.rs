//! The Loop chat cockpit (specialist session cards, fan-out
//! packing, facts, tool cards inside a session), the plan panel's checklist and agent todos,
//! and the closed-chat recents menu.

use gpui::{Entity, TestAppContext};
use recur_domain::{FeedItem, SessionView};
use recur_engine::view::chat::{AgentMsg, ChatLog, ChatLogSession};
use recur_services::AppEvent;
use recur_services::events::{AgentUpdated, SessionDelta, SessionTag, SessionTurnEnd};
use recur_services::fake::Overrides;
use recur_view_chat::cards::session::{SessionCard, last_meta};
use recur_view_chat::chat::AgentChat;
use recur_view_chat::tools::{ToolCx, ToolView};
use recur_view_core::ViewName;
use serde_json::{Value, json};

use super::harness::*;

fn sessions() -> Vec<SessionView> {
    de(json!([
        {
            "id": "s-rv", "code": "RV", "name": "Reviewer", "kind": "reviewer", "status": "idle",
            "createdAt": 1_000, "updatedAt": 2_000,
            "items": [
                {"role": "assistant", "kind": "text", "text": "Reviewing the diff against the base branch."},
                {
                    "role": "assistant", "kind": "tool", "tool": "Bash",
                    "toolInput": "python3 -m py_compile tetris.py",
                    "description": "Compile check",
                    "input": "{\"command\":\"python3 -m py_compile tetris.py && echo OK\"}",
                    "result": "OK",
                },
                {"role": "assistant", "kind": "text", "text": "The smoke test including a piece with rotation is green."},
            ],
        },
        {
            "id": "s-co", "code": "CO", "name": "Coder", "kind": "coder", "status": "idle",
            "createdAt": 3_000, "updatedAt": 4_000,
            "items": [{"role": "assistant", "kind": "text", "text": "Implemented the rotation fix."}],
        },
    ]))
}

fn facts() -> Vec<FeedItem> {
    de(
        json!([{"id": "f1", "agentCode": "MA", "name": "Maestro", "text": "Converged: 0 findings.", "time": "now", "ts": 2_500, "isDeviation": false}]),
    )
}

fn cockpit_opts(sessions: Vec<SessionView>, facts: Vec<FeedItem>) -> Overrides {
    Overrides::default()
        .workspace_sessions(move |_, _| Ok(sessions.clone()))
        .workspace_facts(move |_, _| Ok(facts.clone()))
}

fn one_coder(items: Value) -> Vec<SessionView> {
    de(json!([{
        "id": "s-co", "code": "CO", "name": "Coder", "kind": "coder", "status": "idle",
        "createdAt": 1_000, "updatedAt": 2_000, "items": items,
    }]))
}

/// `openLoopCockpit`: the active workspace on its Loop tab.
fn open_cockpit(cx: &mut TestAppContext, opts: Overrides) -> H<'_> {
    let mut h = boot(cx, opts);
    assert!(h.chip("MA").is_some(), "the Loop tab");
    h.select_tab("MA");
    h.wait(300);
    h
}

/// The cockpit's session cards in order, each with its packed rows.
fn cards(h: &mut H) -> Vec<Vec<ChatLogSession>> {
    h.messages()
        .into_iter()
        .filter_map(|m| match m.log.as_deref() {
            Some(ChatLog::Session(s)) => Some(vec![s.clone()]),
            Some(ChatLog::Sessions { sessions, .. }) => Some(sessions.clone()),
            _ => None,
        })
        .collect()
}

fn names(card: &[ChatLogSession]) -> Vec<&str> {
    card.iter().map(|s| s.name.as_str()).collect()
}

fn card_entity(h: &mut H, log_id: &str) -> Entity<SessionCard> {
    let id = log_id.to_string();
    h.read(|v, _| {
        v.session_cards
            .borrow()
            .get(&id)
            .map(|(c, _)| c.clone())
            .unwrap_or_else(|| panic!("no session card {id}"))
    })
}

/// The open card's read-only transcript.
fn card_chat(h: &mut H, log_id: &str) -> Entity<AgentChat> {
    let card = card_entity(h, log_id);
    card.read_with(h.cx, |c, _| c.test_chat())
        .expect("an opened session card")
}

fn card_msgs(h: &mut H, log_id: &str) -> Vec<AgentMsg> {
    let chat = card_chat(h, log_id);
    chat.read_with(h.cx, |c, _| c.props().messages.clone())
}

fn tool_ix(msgs: &[AgentMsg], tool: &str) -> usize {
    msgs.iter()
        .position(|m| m.tool == tool)
        .unwrap_or_else(|| panic!("no {tool} row"))
}

fn view_of<R>(m: &AgentMsg, f: impl FnOnce(&ToolView) -> R) -> R {
    let v = ToolView::new(
        m,
        ToolCx {
            has_open_file: true,
            ..Default::default()
        },
    );
    f(&v)
}

/// Click a tool row's head inside a session card (`.tc-head`).
fn open_tool(h: &mut H, log_id: &str, i: usize) {
    // On the label, past the icon (`.tc-label`), clear of any link chip.
    let b = h
        .bounds(&format!("tc-head:tc--{i}"))
        .expect("the tool head");
    h.click_at(
        gpui::point(b.origin.x + gpui::px(30.), b.center().y),
        gpui::Modifiers::none(),
    );
    let chat = card_chat(h, log_id);
    let open = chat.read_with(h.cx, |c, _| c.test_open_rows().get(&i.to_string()).copied());
    assert_eq!(open, Some(true), "the tool row opened");
}

fn text_shown(h: &mut H, log_id: &str, needle: &str) -> bool {
    let chat = card_chat(h, log_id);
    h.cx.update(|window, cx| chat.read(cx).test_text_bounds(needle, window, cx).is_some())
}

// ---- loop chat cockpit ----

#[gpui::test]
async fn session_cards_render_chronologically_and_expand_to_the_full_transcript(
    cx: &mut TestAppContext,
) {
    let mut h = open_cockpit(cx, cockpit_opts(sessions(), facts()));
    let cs = cards(&mut h);
    assert_eq!(cs.len(), 2);
    assert_eq!(names(&cs[0]), ["Reviewer"]);
    assert_eq!(
        names(&cs[1]),
        ["Coder", "QA"],
        "Coder and live QA pack in one card"
    );
    assert!(h.has("sess-live:QA"), "QA is working");
    assert!(cs[1][1].live);

    // Collapsed: a fanout row with the final message as meta.
    let (meta, _) = last_meta(&cs[0][0].messages).expect("meta");
    assert!(meta.contains("rotation is green"));
    assert!(h.has("sess-meta:RV"));
    h.click("sess-head:RV:closed");
    assert!(h.has("sess-head:RV:open"));
    let body = h.bounds("fanout-transcript").expect("the body");
    assert!(f32::from(body.size.height) <= 421., "max-height 420");
    let rv = cs[0][0].id.clone();
    let msgs = card_msgs(&mut h, &rv);
    assert!(msgs[0].body.contains("Reviewing the diff"));
    assert!(text_shown(&mut h, &rv, "Reviewing the diff"));

    // The card's transcript selects with the chat's: select all copies its messages too.
    let at = gpui::point(body.left() + gpui::px(40.), body.top() + gpui::px(12.));
    h.click_at(at, gpui::Modifiers::none());
    h.keys("secondary-a");
    h.keys("secondary-c");
    let copied =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .unwrap_or_default();
    assert!(
        copied.contains("Reviewing the diff against the base branch."),
        "{copied:?}"
    );

    // The tool call renders as a card; expanding shows the full command and result.
    let i = tool_ix(&msgs, "Bash");
    assert_eq!(msgs[i].description, "Compile check");
    open_tool(&mut h, &rv, i);
    view_of(&msgs[i], |v| {
        assert!(v.bash_command().contains("python3 -m py_compile tetris.py"))
    });
    assert_eq!(msgs[i].result, "OK");
}

#[gpui::test]
async fn sibling_specialists_share_one_fanout_card(cx: &mut TestAppContext) {
    let opts = cockpit_opts(
        de(json!([
            {
                "id": "s-co", "code": "CO", "name": "Coder", "kind": "coder", "status": "idle",
                "createdAt": 1_000, "updatedAt": 2_000,
                "items": [{"role": "assistant", "kind": "text", "text": "Implemented the rotation fix."}],
            },
            {
                "id": "s-rv", "code": "RV", "name": "Reviewer", "kind": "reviewer", "status": "idle",
                "createdAt": 3_000, "updatedAt": 4_000,
                "items": [{"role": "assistant", "kind": "text", "text": "No new findings."}],
            },
        ])),
        de(json!([{"id": "f1", "agentCode": "MA", "name": "Maestro", "text": "Converged: 0 findings.", "time": "now", "ts": 5_000, "isDeviation": false}])),
    )
    .list_loop_agents(|_, _| Ok(Vec::new()));
    let mut h = open_cockpit(cx, opts);
    let cs = cards(&mut h);
    assert_eq!(cs.len(), 1);
    assert_eq!(names(&cs[0]), ["Coder", "Reviewer"]);
    assert!(h.has("sess-head:CO:closed"));
    assert!(h.has("sess-head:RV:closed"));
    assert!(h.has(&format!("sesscard:{}", cs[0][0].id)));
}

#[gpui::test]
async fn commit_and_handoff_land_in_the_specialist_timeline_collapsed_shows_the_last_line(
    cx: &mut TestAppContext,
) {
    let opts = cockpit_opts(
        de(json!([{
            "id": "s-co", "code": "CO", "name": "Coder", "kind": "coder", "status": "idle",
            "createdAt": 1_000, "updatedAt": 4_000,
            "items": [{"role": "assistant", "kind": "text", "text": "Implemented the rotation fix.", "ts": 1_100}],
        }])),
        de(json!([
            {"id": "f-c", "agentCode": "CO", "name": "Coder", "text": "Committed <code>abc</code> (+1 −0) to <code>main</code>.", "time": "now", "ts": 2_000, "isDeviation": false},
            {"id": "f-h", "agentCode": "CO", "name": "Coder", "text": "Handoff: raised the breakpoint.", "time": "now", "ts": 2_100, "isDeviation": false},
        ])),
    );
    let mut h = open_cockpit(cx, opts);
    let cs = cards(&mut h);
    let coder = cs
        .iter()
        .flatten()
        .find(|s| s.code == "CO")
        .cloned()
        .expect("a Coder row");
    let (meta, _) = last_meta(&coder.messages).expect("meta");
    assert!(meta.contains("Handoff: raised the breakpoint."));
    assert!(!meta.contains("Committed"));
    let head = h.bounds("sess-head:CO:closed").expect("the row");
    let m = h.bounds("sess-meta:CO").expect("the meta");
    assert!(
        head.contains(&m.center()),
        "name and meta share the row's line"
    );
    h.click("sess-head:CO:closed");
    let msgs = card_msgs(&mut h, &coder.id);
    let all: String = msgs
        .iter()
        .map(|m| m.body.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("Implemented the rotation fix."));
    assert!(all.contains("abc"));
    assert!(all.contains("Handoff: raised the breakpoint."));
}

fn coder_tool(cx: &mut TestAppContext, item: Value) -> (H<'_>, String, Vec<AgentMsg>) {
    let mut h = open_cockpit(cx, cockpit_opts(one_coder(json!([item])), Vec::new()));
    let cs = cards(&mut h);
    let id = cs[0][0].id.clone();
    h.click("sess-head:CO:closed");
    let msgs = card_msgs(&mut h, &id);
    (h, id, msgs)
}

#[gpui::test]
async fn read_lints_tool_card_shows_a_clean_summary_instead_of_raw_json(cx: &mut TestAppContext) {
    let item = json!({
        "role": "assistant", "kind": "tool", "tool": "readLints",
        "toolInput": "{\"paths\":[\"/tmp/ws/frontend/src/a.ts\",\"/tmp/ws/frontend/src/b.ts\"]}",
        "input": json!({"paths": [
            "/tmp/ws/frontend/src/lib/agentchat.ts",
            "/tmp/ws/frontend/src/components/AgentChat.svelte",
            "/tmp/ws/frontend/src/styles/app.css",
        ]}).to_string(),
        "result": json!({
            "fileDiagnostics": [
                {"path": "/tmp/ws/frontend/src/lib/agentchat.ts", "diagnostics": [], "diagnosticsCount": 0},
                {"path": "/tmp/ws/frontend/src/components/AgentChat.svelte", "diagnostics": [], "diagnosticsCount": 0},
                {"path": "/tmp/ws/frontend/src/styles/app.css", "diagnostics": [], "diagnosticsCount": 0},
            ],
            "totalFiles": 3,
            "totalDiagnostics": 0,
        }).to_string(),
    });
    let (mut h, id, msgs) = coder_tool(cx, item);
    let i = tool_ix(&msgs, "readLints");
    view_of(&msgs[i], |v| {
        assert_eq!(v.label(), "Read lints");
        assert!(v.meta().contains("clean"), "{}", v.meta());
        assert_eq!(v.read_lints_paths().len(), 3);
        let r = v.read_lints_result().expect("a parsed result");
        assert_eq!(r.total_diagnostics, 0);
    });
    open_tool(&mut h, &id, i);
}

#[gpui::test]
async fn delete_tool_card_names_the_file_instead_of_dumping_raw_json_args(cx: &mut TestAppContext) {
    let item = json!({
        "role": "assistant", "kind": "tool", "tool": "Delete",
        "toolInput": "{\"file_path\":\"/tmp/ws/frontend/src/views/Escalation.svelte\"}",
        "input": "{\"file_path\":\"/tmp/ws/frontend/src/views/Escalation.svelte\"}",
        "result": "",
    });
    let (mut h, _, msgs) = coder_tool(cx, item);
    let i = tool_ix(&msgs, "Delete");
    view_of(&msgs[i], |v| {
        assert_eq!(v.label(), "Deleted Escalation.svelte");
        assert!(!v.shows_meta(false), "no raw JSON meta");
        assert!(!v.expandable(), "a static head");
        assert!(!v.label().contains("{\"file_path\""));
        assert_eq!(v.icon(), recur_view_chat::tools::StepIcon::Trash);
    });
    assert!(h.has(&format!("tc-head:tc--{i}")));
}

#[gpui::test]
async fn web_fetch_tool_card_shows_a_url_chip_instead_of_dumping_raw_json_args(
    cx: &mut TestAppContext,
) {
    let url =
        "https://docs.cdp.coinbase.com/api-reference/rest-api/onramp-offramp/create-session-token";
    let input = format!(
        "{{\"url\":\"{url}\",\"prompt\":\"List the request body fields. Is clientIp required?\"}}"
    );
    let item = json!({
        "role": "assistant", "kind": "tool", "tool": "WebFetch",
        "toolInput": input, "input": input,
        "result": "[![CI](https://img.shields.io/badge.svg)](https://example.com)\n\n# Onramp Session Token Request Body Fields\n\n**addresses** (Required)\n\n![](https://tracker.example/pixel.png)",
    });
    let (mut h, id, msgs) = coder_tool(cx, item);
    let i = tool_ix(&msgs, "WebFetch");
    view_of(&msgs[i], |v| {
        assert_eq!(v.label(), "Fetched a page");
        assert_eq!(v.fetch_url(), url);
        assert_eq!(
            recur_agent::display::web::compact_url(&v.fetch_url()),
            "docs.cdp.coinbase.com/…/create-session-token"
        );
        assert!(!v.shows_meta(false));
        assert!(v.fetch_prompt().contains("Is clientIp required?"));
    });
    open_tool(&mut h, &id, i);
    assert!(text_shown(
        &mut h,
        &id,
        "Onramp Session Token Request Body Fields"
    ));
    assert!(text_shown(&mut h, &id, "addresses"));
    assert!(
        text_shown(&mut h, &id, "CI"),
        "the badge image becomes its alt link"
    );
    assert!(!text_shown(&mut h, &id, "{\"url\""));
}

#[gpui::test]
async fn failed_web_search_with_an_empty_result_still_shows_a_failed_placeholder(
    cx: &mut TestAppContext,
) {
    let item = json!({
        "role": "assistant", "kind": "tool", "tool": "WebSearch",
        "toolInput": "onramp session token",
        "input": "{\"query\":\"onramp session token\"}",
        "result": "",
        "resultError": true,
    });
    let (mut h, id, msgs) = coder_tool(cx, item);
    let i = tool_ix(&msgs, "WebSearch");
    assert!(msgs[i].result_error && msgs[i].result.is_empty());
    view_of(&msgs[i], |v| {
        assert_eq!(v.label(), "Searched the web");
        assert!(v.expandable(), "the failed placeholder opens");
        assert!(!v.label().contains("{\"query\""));
    });
    open_tool(&mut h, &id, i);
}

#[gpui::test]
async fn loop_facts_interleave_between_session_cards_by_timestamp(cx: &mut TestAppContext) {
    let mut h = open_cockpit(cx, cockpit_opts(sessions(), facts()));
    let fact = h.bounds("loop-fact:f1").expect("the fact");
    let cs = cards(&mut h);
    let first = h
        .bounds(&format!("sesscard:{}", cs[0][0].id))
        .expect("first card");
    let last = h
        .bounds(&format!("sesscard:{}", cs[1][0].id))
        .expect("last card");
    assert!(first.origin.y < fact.origin.y && fact.origin.y < last.origin.y);
    let fm = h
        .messages()
        .into_iter()
        .find_map(|m| match m.log.as_deref() {
            Some(ChatLog::Fact { text, name, .. }) => Some((text.clone(), name.clone())),
            _ => None,
        })
        .expect("a fact row");
    assert_eq!(
        fm,
        ("Converged: 0 findings.".to_string(), "Maestro".to_string())
    );
}

#[gpui::test]
async fn long_loop_fact_errors_wrap_instead_of_ellipsizing(cx: &mut TestAppContext) {
    let text = "Loop failed: Coder turn failed: cursor run ended without a result (bridge log: /Users/josh/Library/Application Support/Recur/logs/cursor-bridges/7d6963206f17cc84/cursor-bridge.log)";
    let mut h = open_cockpit(
        cx,
        cockpit_opts(
            sessions(),
            de(
                json!([{"id": "f-err", "agentCode": "MA", "name": "Maestro", "text": text, "time": "now", "ts": 2_500, "isDeviation": false}]),
            ),
        ),
    );
    let fact = h.bounds("loop-fact:f-err").expect("the fact");
    assert!(
        f32::from(fact.size.height) > 30.,
        "the long error wraps onto more lines: {:?}",
        fact.size
    );
}

#[gpui::test]
async fn pr_opened_fact_shows_completed_time_and_opens_the_pr(cx: &mut TestAppContext) {
    let html = "Completed in 12m 04s · <a class=\"loop-pr-link\" data-pr=\"482\">Local PR #482</a><div class=\"loop-wrap\"><b>Done</b> Retry the payment token once</div>";
    let mut h = open_cockpit(
        cx,
        cockpit_opts(
            sessions(),
            de(
                json!([{"id": "f-pr", "agentCode": "MA", "name": "Maestro", "ts": 5_000, "isDeviation": false, "text": html}]),
            ),
        ),
    );
    assert!(h.has("loop-fact:f-pr"));
    let done = recur_engine::view::loop_cockpit::loop_pr_completion(html, None).expect("a PR fact");
    assert_eq!(done.duration, "12m 04s");
    assert_eq!(done.pr_label, "Local PR #482");
    let wrap = recur_engine::view::loop_cockpit::loop_completion_wrap(html);
    assert!(wrap.contains("Retry the payment token once"));
    assert!(!wrap.contains("ready for your review"));
    h.click("loop-pr-link:482");
    assert_eq!(h.state.read_with(h.cx, |s, _| s.view), ViewName::PrDetail);
}

#[gpui::test]
async fn one_specialist_session_with_two_turns_renders_two_cards_at_those_times(
    cx: &mut TestAppContext,
) {
    let opts = cockpit_opts(
        de(json!([{
            "id": "s-co", "code": "CO", "name": "Coder", "kind": "coder", "status": "idle",
            "createdAt": 1_000, "updatedAt": 4_000,
            "items": [
                {"role": "user", "kind": "text", "text": "fix rotation", "ts": 1_000},
                {"role": "assistant", "kind": "text", "text": "first pass", "ts": 1_100},
                {"role": "assistant", "kind": "turn", "durationMs": 1, "ts": 1_200},
                {"role": "user", "kind": "text", "text": "fix again", "ts": 3_000},
                {"role": "assistant", "kind": "text", "text": "second pass", "ts": 3_100},
            ],
        }])),
        Vec::new(),
    );
    let mut h = open_cockpit(cx, opts);
    let cs = cards(&mut h);
    let with_coder: Vec<_> = cs
        .iter()
        .filter(|c| c.iter().any(|s| s.code == "CO"))
        .collect();
    assert_eq!(with_coder.len(), 1, "one card");
    let coders: Vec<&ChatLogSession> = with_coder[0].iter().filter(|s| s.code == "CO").collect();
    assert_eq!(coders.len(), 2, "two Coder rows");
    assert!(coders.iter().all(|s| s.name == "Coder"));
    assert!(
        last_meta(&coders[0].messages)
            .unwrap()
            .0
            .contains("first pass")
    );
    assert!(
        last_meta(&coders[1].messages)
            .unwrap()
            .0
            .contains("second pass")
    );
    assert!(h.has("sess-live:QA"));
}

#[gpui::test]
async fn coder_fix_turn_prompt_shows_the_short_label_not_the_mcp_dispatch_wire(
    cx: &mut TestAppContext,
) {
    let wire = [
        "You are the Coder agent. A reviewer found issues with the current diff. ",
        "Fix every finding below with minimal, correct edits. For each finding, call ",
        "mcp__recur__update_finding with its id: status resolved if you fixed it, or ",
        "invalid with a clear reason if it is wrong/out of scope. Then call mcp__recur__done.",
        "",
        "Original task: create a cli game of pong",
        "",
        "Reviewer findings to fix:",
        "1. id=F-test [medium] pong/game.py:135: paddle centering mismatch",
    ]
    .join("\n");
    let opts = cockpit_opts(
        one_coder(json!([
            {"role": "user", "kind": "text", "text": wire, "ts": 1_000},
            {"role": "assistant", "kind": "text", "text": "Centered the paddle on integer coords.", "ts": 1_100},
        ])),
        Vec::new(),
    );
    let mut h = open_cockpit(cx, opts);
    let cs = cards(&mut h);
    let id = cs[0][0].id.clone();
    h.click("sess-head:CO:closed");
    let msgs = card_msgs(&mut h, &id);
    let me = msgs.iter().find(|m| m.me).expect("the prompt row");
    assert!(
        me.body.contains("Fix open reviewer findings"),
        "{}",
        me.body
    );
    let all: String = msgs
        .iter()
        .map(|m| m.body.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!all.contains("mcp__recur__update_finding"));
    assert!(!all.contains("You are the Coder agent"));
    assert!(h.me(&me.body));
}

fn coder_tag() -> SessionTag {
    SessionTag {
        workspace_id: WS.into(),
        code: "CO".into(),
        ..Default::default()
    }
}

fn coder_status(status: &str) -> AppEvent {
    AppEvent::AgentUpdated(AgentUpdated {
        workspace_id: WS.into(),
        agent_id: format!("{WS}-CO"),
        status: status.into(),
        ..Default::default()
    })
}

fn coder_delta(text: &str, seq: i64) -> AppEvent {
    AppEvent::SessionDelta(SessionDelta {
        tag: coder_tag(),
        text: text.into(),
        seq: Some(seq),
        replace: true,
        ..Default::default()
    })
}

#[gpui::test]
async fn late_turn_end_and_stream_after_idle_do_not_keep_the_running_timer(
    cx: &mut TestAppContext,
) {
    let opts = cockpit_opts(Vec::new(), Vec::new()).list_loop_agents(|_, _| Ok(Vec::new()));
    let mut h = open_cockpit(cx, opts);
    assert!(cards(&mut h).is_empty());

    h.dispatch(coder_status("working"));
    h.dispatch(coder_delta("editing the rotation", 1));
    assert!(h.has("sess-live:CO"));
    assert!(h.has("chat-turn-elapsed"));

    h.dispatch(coder_status("idle"));
    assert!(!h.has("sess-live:CO"));
    assert!(!h.has("chat-turn-elapsed"));

    h.dispatch(AppEvent::SessionTurnEnd(SessionTurnEnd {
        tag: coder_tag(),
        duration_ms: 4200,
        ..Default::default()
    }));
    h.dispatch(coder_delta("late leftover chunk", 2));
    // turn_end splits the session; open the settled slice so its footer renders.
    let ended = cards(&mut h)
        .into_iter()
        .flatten()
        .find(|s| s.code == "CO")
        .expect("the ended slice");
    let card = card_entity(&mut h, &ended.id);
    if !card.read_with(h.cx, |c, _| c.test_expanded()) {
        h.cx.update(|_, cx| card.update(cx, |c, cx| c.test_toggle(cx)));
        h.settle();
    }
    assert!(ended.messages.iter().any(|m| m.turn_end), "completed in …");
    assert!(h.has("turn-end"));
    assert!(!h.has("sess-live:CO"));
    assert!(!h.has("chat-turn-elapsed"));
}

// ---- plan panel ----

#[gpui::test]
async fn plan_panel_shows_the_workspace_checklist_and_agent_todos(cx: &mut TestAppContext) {
    let plan = json!({
        "summary": "Ship the Loop cockpit",
        "body": "Merge specialist cards so the cockpit is one surface. Keep agent todos distinct from plan steps.",
        "source": "human",
        "approved": false,
        "steps": [
            {"id": "s1", "title": "Merge specialist cards into Loop chat", "status": "doing"},
            {"id": "s2", "title": "Retarget Activity e2e", "status": "pending"},
        ],
    });
    let todos = json!({"todos": [
        {"content": "Wire Plan panel", "status": "in_progress"},
        {"content": "Retarget e2e", "status": "pending"},
    ]})
    .to_string();
    let opts = Overrides::default().workspace_sessions(move |_, _| {
        Ok(one_coder(json!([
            {"role": "assistant", "kind": "tool", "tool": "TodoWrite", "input": todos},
        ])))
    });
    let mut h = boot_with(
        cx,
        |s| {
            if let Some(w) = s.workspace_mut(WS) {
                w.plan = Some(de(plan.clone()));
            }
        },
        opts,
        |_| {},
        WS,
    );
    h.click("panel-toggle:plan");
    assert!(h.has("plan-pane"));
    let summary = h.state.read_with(h.cx, |s, _| {
        s.workspace(WS)
            .and_then(|w| w.plan.as_ref())
            .map(|p| p.summary.clone())
    });
    assert_eq!(summary.as_deref(), Some("Ship the Loop cockpit"));
    let panel = h.read(|v, _| v.plan_panel.clone());
    let body = h.cx.update(|_, cx| panel.read(cx).test_body_text(cx));
    assert!(body.contains("Keep agent todos distinct"), "{body}");
    // The rendered body selects and copies, as the page does.
    let scroll = h.bounds("plan-body-scroll").expect("the body");
    h.click_at(
        gpui::point(scroll.left() + gpui::px(40.), scroll.top() + gpui::px(20.)),
        gpui::Modifiers::none(),
    );
    h.keys("secondary-a");
    h.keys("secondary-c");
    let copied =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .unwrap_or_default();
    assert!(copied.contains("Keep agent todos distinct"), "{copied:?}");

    h.click("tab-chip:plan-tab-todos");
    let title =
        h.cx.update(|_, cx| panel.read(cx).test_step_value("s1", cx));
    assert_eq!(
        title.as_deref(),
        Some("Merge specialist cards into Loop chat")
    );
    assert!(h.has("plan-todo-name:Coder"));
    let todos = h.read(|v, cx| {
        v.feed
            .read(cx)
            .sessions
            .iter()
            .find(|s| s.code == "CO")
            .map(|s| crate::plan_panel::latest_todos_from_messages(&s.messages))
            .unwrap_or_default()
    });
    assert_eq!(
        todos.first().map(|t| t.content.as_str()),
        Some("Wire Plan panel")
    );

    // Editing mounts the shared `MiniScriptEditor` (`lang="md"`, `fill`) on the plan's body,
    // and what is typed there is the draft a save submits.
    h.cx.update(|window, cx| panel.update(cx, |p, cx| p.start_edit(window, cx)));
    h.settle();
    let (editor, _) = h.cx.update(|_, cx| panel.read(cx).test_body_editor());
    let editor = editor.expect("the body editor");
    let seeded = h.cx.update(|_, cx| editor.read(cx).value(cx));
    assert!(seeded.starts_with("Merge specialist cards"), "{seeded}");
    h.cx.update(|window, cx| editor.update(cx, |e, cx| e.focus(window, cx)));
    h.settle();
    h.cx.simulate_input("!");
    // The editor publishes its text on a debounce.
    h.cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    h.settle();
    let (_, draft) = h.cx.update(|_, cx| panel.read(cx).test_body_editor());
    assert!(draft.starts_with("!Merge specialist cards"), "{draft}");
}

// ---- closed chat recents ----

fn new_then_close(h: &mut H) -> String {
    h.click("wsv-chat-new");
    assert_eq!(h.codes().len(), 3);
    let code = h.codes()[2].clone();
    let close = format!("tab-close:chat-tab-{code}");
    let chip = h.chip(&code).expect("the new tab");
    h.hover(&chip);
    h.click(&close);
    assert_eq!(h.codes().len(), 2);
    code
}

fn closed_title(h: &mut H, code: &str) -> String {
    let code = code.to_string();
    h.read(|v, _| {
        v.closed_tabs
            .iter()
            .find(|t| t.code == code)
            .map(|t| {
                if t.title.is_empty() {
                    recur_view_core::modes::NEW_CHAT_TITLE.to_string()
                } else {
                    t.title.clone()
                }
            })
            .unwrap_or_default()
    })
}

#[gpui::test]
async fn reopens_a_closed_pair_tab_from_the_recents_menu(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default());
    let code = new_then_close(&mut h);
    h.click("wsv-recents");
    assert!(h.has(&format!("wsv-recents-item:{code}")));
    let title = closed_title(&mut h, &code);
    assert_eq!(title, "New chat");
    assert!(!title.contains("PA2"));
    let updated = h.read(|v, _| {
        v.closed_tabs
            .iter()
            .find(|t| t.code == code)
            .map(|t| t.updated_at)
    });
    assert_eq!(
        recur_engine::view::artifacts::format_relative_age(
            updated.unwrap_or_default(),
            crate::view::now_ms()
        )
        .map(|a| a.rel)
        .as_deref(),
        Some("just now")
    );
    h.click(&format!("wsv-recents-item:{code}"));
    assert_eq!(h.codes().len(), 3);
    assert!(!h.has("wsv-recents"), "nothing left to reopen");
}

#[gpui::test]
async fn permanently_deletes_a_closed_pair_tab_from_the_recents_menu(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default());
    let code = new_then_close(&mut h);
    h.click("wsv-recents");
    let row = format!("wsv-recents-item:{code}");
    assert_eq!(closed_title(&mut h, &code), "New chat");
    h.hover(&row);
    h.click(&format!("wsv-recents-del:{code}"));
    assert!(h.has("wsv-recents"), "the first click only arms");
    h.click(&format!("wsv-recents-del:{code}"));
    assert!(!h.has("wsv-recents"));
    assert_eq!(h.codes().len(), 2);
}

#[gpui::test]
async fn closes_the_loop_tab_like_other_chats(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default());
    let loop_chip = h.chip("MA").expect("the Loop tab");
    h.hover(&loop_chip);
    assert!(h.has("tab-close:chat-tab-MA"));
    h.click("tab-close:chat-tab-MA");
    assert!(h.chip("MA").is_none());
    let titles = h.read(|v, _| v.tabs().iter().map(|t| t.title.clone()).collect::<Vec<_>>());
    assert!(titles.iter().any(|t| t == "New chat"), "{titles:?}");

    h.click("wsv-recents");
    assert!(h.has("wsv-recents-item:MA"));
    assert_eq!(closed_title(&mut h, "MA"), "Loop");
    h.click("wsv-recents-item:MA");
    assert!(h.chip("MA").is_some());
}
