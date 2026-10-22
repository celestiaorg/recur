//! `components/AgentChat.test.ts` (with `AgentChat.harness.svelte`'s SessionCard snippet).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::TestAppContext;
use gpui_kit::{AppContext as _, Entity, IntoElement};
use recur_engine::view::chat::{AgentMsg, ChatLog, ChatLogSession};
use recur_services::mock::MockOptions;

use super::harness::*;
use crate::blocks::Block;
use crate::cards::session::{SessionCard, SessionCardProps, last_meta};
use crate::chat::{ChatHooks, ChatProps, SessionCardRenderer};

fn readonly() -> ChatProps {
    ChatProps {
        readonly: true,
        empty_text: String::new(),
        ..ws_props()
    }
}

fn note(label: &str, body: &str, html: bool, ts: i64) -> AgentMsg {
    AgentMsg {
        note: true,
        note_label: label.into(),
        note_html: html,
        body: body.into(),
        ts: Some(ts),
        ..Default::default()
    }
}

fn tool(name: &str, input: &str, result: &str, ts: i64) -> AgentMsg {
    AgentMsg {
        tool: name.into(),
        tool_input: input.into(),
        result: result.into(),
        ts: Some(ts),
        ..Default::default()
    }
}

fn blocks(h: &mut H) -> Vec<Block> {
    h.read(|c| c.blocks(&c.transcript()).0)
}

#[gpui::test]
async fn plain_notices_stay_escaped_engine_wrap_facts_render_html(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), readonly(), ChatHooks::default());
    h.set_messages(vec![note(
        "Subagent",
        "fix <img src=x onerror=alert(1)> (child-1)",
        false,
        1,
    )]);
    assert!(h.has("notice:plain:fix <img src=x onerror=alert(1)> (child-1)"));
    h.set_messages(vec![note(
        "Commit",
        "Committed <code>abc</code> to <code>main</code>.",
        true,
        2,
    )]);
    assert!(h.has("notice:html:Committed <code>abc</code> to <code>main</code>."));
    let spans = crate::html_inline::inline_spans(
        "Committed <code>abc</code> to <code>main</code>.",
        &crate::html_inline::InlineStyle::meta(
            11.,
            recur_ui::hex(0x888888),
            &recur_ui::ThemeId::default().tokens(),
        ),
    );
    let flat: String = spans.iter().map(|s| s.text.as_str()).collect();
    assert_eq!(flat, "Committed abc to main.");
}

#[gpui::test]
async fn subagent_notices_fold_into_the_following_steps_group(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), readonly(), ChatHooks::default());
    h.set_messages(vec![
        note("Subagent", "Goal finished (child-1)", false, 1),
        tool("Read", "a.ts", "ok", 2),
        tool("Grep", "TODO", "none", 3),
    ]);
    assert_eq!(
        blocks(&mut h),
        [Block::Steps {
            start: 0,
            rows: vec![0, 1, 2]
        }]
    );
    assert!(!h.has("notice:plain:Goal finished (child-1)"));
    let head = h.read(|c| {
        let msgs = c.transcript();
        crate::blocks::group_summary(&msgs, &[0, 1, 2], &[0, 1, 2], c.tool_data.cx())
    });
    assert!(head.contains("2 tool calls"), "{head}");
    h.click(&format!("steps-head:{head}"));
    assert!(h.has("notice:plain:Goal finished (child-1)"));

    h.set_messages(vec![
        note(
            "Commit",
            "Committed <code>abc</code> to <code>main</code>.",
            true,
            1,
        ),
        tool("Read", "a.ts", "ok", 2),
    ]);
    assert_eq!(
        blocks(&mut h),
        [
            Block::Msg(0),
            Block::Steps {
                start: 1,
                rows: vec![1]
            }
        ]
    );
    assert!(h.has("notice:html:Committed <code>abc</code> to <code>main</code>."));

    h.set_messages(vec![note("Subagent", "Goal finished (child-1)", false, 1)]);
    assert_eq!(blocks(&mut h), [Block::Msg(0)]);
    assert!(h.has("notice:plain:Goal finished (child-1)"));
}

#[test]
fn collapsed_session_card_meta_escapes_ordinary_notes() {
    let msgs = vec![note("Subagent", "fix <b>xss</b> (child-1)", false, 1)];
    assert_eq!(
        last_meta(&msgs),
        Some(("fix <b>xss</b> (child-1)".into(), false))
    );
}

fn coder() -> ChatLogSession {
    ChatLogSession {
        id: "s-co#0".into(),
        code: "CO".into(),
        name: "Coder".into(),
        live: false,
        messages: vec![AgentMsg {
            body: "done".into(),
            ts: Some(1100),
            ..Default::default()
        }],
    }
}

fn log(l: ChatLog) -> AgentMsg {
    AgentMsg {
        ts: Some(1000),
        log: Some(Box::new(l)),
        ..Default::default()
    }
}

#[gpui::test]
async fn session_card_stays_expanded_when_a_sibling_specialist_joins_the_group(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, MockOptions::default(), readonly(), ChatHooks::default());
    let vcx = h
        .host
        .read_with(h.cx, |host, cx| recur_view_core::ViewContext {
            state: host.chat.read(cx).state.clone(),
            services: host.services.clone(),
        });
    let cards: Rc<RefCell<HashMap<String, Entity<SessionCard>>>> = Default::default();
    let renderer: SessionCardRenderer = Rc::new(move |log, first, _, cx| {
        let props = SessionCardProps {
            code: log.code.clone(),
            name: log.name.clone(),
            messages: log.messages.clone(),
            live: log.live,
            presentation_active: true,
            root_path: String::new(),
            workspace_id: String::new(),
            findings: Vec::new(),
            open_file: false,
            first,
            font_size: 13.,
        };
        let card = cards
            .borrow_mut()
            .entry(log.id.clone())
            .or_insert_with(|| cx.new(|_| SessionCard::new(vcx.clone(), props.clone())))
            .clone();
        card.update(cx, |c, cx| c.set_props(props, cx));
        card.into_any_element()
    });
    let chat = h.chat();
    h.cx.update(|_, cx| chat.update(cx, |c, cx| c.set_session_card(Some(renderer), cx)));
    h.set_messages(vec![log(ChatLog::Session(coder()))]);
    assert!(h.has("sess-head:CO:closed"));
    h.click("sess-head:CO:closed");
    assert!(h.has("sess-head:CO:open"));
    assert!(h.has("fanout-transcript"));

    h.set_messages(vec![log(ChatLog::Sessions {
        id: "s-co#0".into(),
        sessions: vec![
            coder(),
            ChatLogSession {
                id: "s-rv#0".into(),
                code: "RV".into(),
                name: "Reviewer".into(),
                live: true,
                messages: Vec::new(),
            },
        ],
    })]);
    assert!(h.has("sess-head:CO:open"));
    assert!(h.has("sess-head:RV:open"));
    assert!(h.has("fanout-transcript"));
}

#[gpui::test]
async fn final_reply_stays_visible_when_compaction_follows_the_turn(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), readonly(), ChatHooks::default());
    let reply = "Check-in only renders when Apollo Sync returns one.";
    h.set_messages(vec![
        AgentMsg {
            me: true,
            body: "where is check-in?".into(),
            ts: Some(1),
            ..Default::default()
        },
        tool("Read", "checkin.go", "ok", 2),
        AgentMsg {
            thinking: true,
            body: "looking at mockup vs product".into(),
            ts: Some(3),
            ..Default::default()
        },
        AgentMsg {
            body: reply.into(),
            ts: Some(4),
            ..Default::default()
        },
        AgentMsg {
            compact: true,
            body: "folded prior turns".into(),
            ts: Some(5),
            ..Default::default()
        },
        AgentMsg {
            turn_end: true,
            duration_ms: 42000,
            ts: Some(6),
            ..Default::default()
        },
    ]);
    let steps = blocks(&mut h)
        .into_iter()
        .find_map(|b| match b {
            Block::Steps { rows, .. } => Some(rows),
            _ => None,
        })
        .expect("a steps group");
    assert!(
        !steps.contains(&3),
        "the reply is not folded into the steps"
    );
    let head = h.read(|c| {
        let msgs = c.transcript();
        crate::blocks::group_summary(&msgs, &steps, &steps, c.tool_data.cx())
    });
    assert!(head.contains("compaction"), "{head}");
    assert!(h.has(&format!("steps-head:{head}")));
    assert!(h.assistant(reply));
}
