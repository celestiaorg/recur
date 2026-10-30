//! The agent chat view, with session cards rendered inside it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::TestAppContext;
use gpui_kit::{AppContext as _, Entity, IntoElement};
use recur_engine::view::chat::{AgentMsg, ChatLog, ChatLogSession};
use recur_services::fake::Overrides;

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
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
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
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
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
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
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
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
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

#[gpui::test]
async fn text_selects_across_messages(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
    h.set_messages(vec![
        AgentMsg {
            me: true,
            body: "Fix the bug".into(),
            ts: Some(1),
            ..Default::default()
        },
        AgentMsg {
            body: "Done.\n\nIt works.".into(),
            ts: Some(2),
            ..Default::default()
        },
    ]);
    let me = h.bounds("me:Fix the bug").expect("bubble");
    let reply = h.bounds("assistant:Done.").expect("reply");
    let from = gpui::point(me.left() + gpui::px(12.5), me.center().y);
    let to = gpui::point(reply.right() - gpui::px(1.), reply.bottom() - gpui::px(2.));
    let m = gpui::Modifiers::none();
    h.cx.simulate_mouse_move(from, None, m);
    h.cx.simulate_mouse_down(from, gpui::MouseButton::Left, m);
    for i in 1..=4 {
        let p = from + (to - from) * (i as f32 / 4.);
        h.cx.simulate_mouse_move(p, Some(gpui::MouseButton::Left), m);
        h.cx.run_until_parked();
    }
    h.cx.simulate_mouse_up(to, gpui::MouseButton::Left, m);
    h.cx.run_until_parked();
    h.keys("secondary-c");
    let copied =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(copied.as_deref(), Some("Fix the bug\nDone.\n\nIt works."));
}

#[gpui::test]
async fn text_selects_across_a_file_chip_and_copies_its_label(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
    h.set_messages(vec![AgentMsg {
        me: true,
        body: "see @docs/notes.md please".into(),
        ts: Some(1),
        ..Default::default()
    }]);
    let me = h.bounds("me:see @docs/notes.md please").expect("bubble");
    let from = gpui::point(me.left() + gpui::px(12.5), me.center().y);
    let to = gpui::point(me.right() - gpui::px(12.5), me.center().y);
    let m = gpui::Modifiers::none();
    h.cx.simulate_mouse_move(from, None, m);
    h.cx.simulate_mouse_down(from, gpui::MouseButton::Left, m);
    for i in 1..=4 {
        let p = from + (to - from) * (i as f32 / 4.);
        h.cx.simulate_mouse_move(p, Some(gpui::MouseButton::Left), m);
        h.cx.run_until_parked();
    }
    h.cx.simulate_mouse_up(to, gpui::MouseButton::Left, m);
    h.cx.run_until_parked();
    h.keys("secondary-c");
    let copied =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(copied.as_deref(), Some("see notes.md please"));
}

#[gpui::test]
async fn select_all_copies_tool_rows(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
    h.set_messages(vec![
        AgentMsg {
            me: true,
            body: "Look".into(),
            ts: Some(1),
            ..Default::default()
        },
        tool("Read", "a.ts", "ok", 2),
        AgentMsg {
            body: "Done.".into(),
            ts: Some(3),
            ..Default::default()
        },
    ]);
    let me = h.bounds("me:Look").expect("bubble");
    let at = gpui::point(me.left() + gpui::px(12.5), me.center().y);
    h.cx.simulate_click(at, gpui::Modifiers::none());
    h.keys("secondary-a");
    h.keys("secondary-c");
    let copied =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .unwrap_or_default();
    let label = h.read(|c| {
        let msgs = c.transcript();
        crate::tools::ToolView::new(&msgs[1], c.tool_data.cx()).label()
    });
    assert!(copied.starts_with("Look\n"), "{copied:?}");
    assert!(copied.contains(&label), "{label:?} in {copied:?}");
    assert!(copied.ends_with("Done."), "{copied:?}");
}

#[gpui::test]
async fn a_bubble_with_an_image_grows_with_its_wrapped_text(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
    let text = "and runs in the nearest loop tab. but if there are no loop tabs then you have no idea what is going on. I'd think that resolve conflicts would just run in the nearest ops tab like other git-operations that need AI";
    let body = format!("if PR has conflicts it shows this: ⟦attachment:i1⟧ {text}");
    h.set_messages(vec![AgentMsg {
        me: true,
        body: body.clone(),
        ts: Some(1),
        attachments: vec![recur_engine::view::chat::ChatAttachmentView {
            id: "i1".into(),
            data_url: "data:image/png;base64,AA".into(),
            name: "shot.png".into(),
            ..Default::default()
        }],
        ..Default::default()
    }]);
    let plain = recur_engine::view::mentions::strip_attachment_markers(&body);
    let me = h
        .bounds(&format!("me:{}", crate::chat::debug_key(&plain)))
        .expect("bubble");
    let inner = f32::from(me.size.width) - 24.;
    let text_h = h.cx.update(|window, _| {
        let style = crate::flow::FlowStyle {
            family: recur_ui::fonts::Family::Ui,
            size: 12.5,
            weight: 400.,
            leading: 1.5,
            color: gpui::black(),
        };
        crate::flow::FlowLayout::compute(&format!(" {text}"), &[], &style, inner, window).height
    });
    assert!(
        f32::from(me.size.height) >= 18. + 18. + text_h,
        "bubble {me:?} vs text {text_h}"
    );
}
