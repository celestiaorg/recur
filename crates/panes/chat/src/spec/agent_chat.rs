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

/// Draws one frame with the dock `w` px wide and returns where it put block `key` against the
/// scroller's top and bottom, before anything the frame deferred runs.
fn frame_at(h: &mut H, w: f32, key: &str) -> (f32, f32) {
    let (host, chat) = (h.host.clone(), h.chat());
    h.cx.update(|window, cx| {
        host.update(cx, |h, cx| {
            h.width = Some(w);
            cx.notify();
        });
        window.draw(cx).clear(cx);
        let c = chat.read(cx);
        let (y, height) = c.block_ranges.borrow()[key];
        let b = c.scroll.bounds();
        (y - f32::from(b.top()), f32::from(b.bottom()) - y - height)
    })
}

fn long_transcript(h: &mut H) {
    let long = "the quick brown fox jumps over the lazy dog ".repeat(12);
    h.set_messages(
        (0..12)
            .map(|i| AgentMsg {
                body: format!("{i} {long}"),
                ts: Some(i),
                ..Default::default()
            })
            .collect(),
    );
}

const DRAG: [f32; 7] = [520., 470., 420., 380., 430., 500., 560.];

/// Dragging the dock's width keeps a transcript pinned to its last block in every frame, not
/// a frame after each rewrap.
#[gpui::test]
async fn a_width_drag_keeps_the_bottom_pin_in_each_frame(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
    long_transcript(&mut h);
    let last = h.read(|c| {
        let r = c.block_ranges.borrow();
        r.iter()
            .max_by(|a, b| a.1.0.total_cmp(&b.1.0))
            .unwrap()
            .0
            .clone()
    });
    let settled = frame_at(&mut h, 560., &last).1;
    for w in DRAG {
        let now = frame_at(&mut h, w, &last).1;
        assert!((now - settled).abs() < 0.5, "{now} vs {settled} at {w}px");
    }
    h.settle();
    assert!(h.read(|c| c.near_bottom), "still pinned");
}

/// Scrolled up, a width drag keeps the block at the top of the scroller in place in every
/// frame, as the rewrapped blocks above it change height.
#[gpui::test]
async fn a_width_drag_holds_the_anchor_block_in_each_frame(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
    long_transcript(&mut h);
    let scroll = h.read(|c| c.scroll.clone());
    let at = scroll.bounds().center();
    h.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    h.cx.simulate_event(gpui::ScrollEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(900.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
        ..Default::default()
    });
    h.settle();
    assert!(!h.read(|c| c.near_bottom), "scrolled up");
    let key = h.read(|c| c.anchor.clone()).expect("an anchor block").0;
    let settled = frame_at(&mut h, 560., &key).0;
    for w in DRAG {
        let now = frame_at(&mut h, w, &key).0;
        assert!((now - settled).abs() < 0.5, "{now} vs {settled} at {w}px");
    }
}

#[gpui::test]
async fn find_counts_message_text_and_steps_the_current_match(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
    h.set_messages(vec![
        AgentMsg {
            me: true,
            body: "Where is the cache?".into(),
            ts: Some(1),
            ..Default::default()
        },
        tool("Read", "{}", "cache cache", 2),
        AgentMsg {
            body: "The cache lives in the store. Clear the cache with care.".into(),
            ts: Some(3),
            ..Default::default()
        },
    ]);
    let chat = h.chat();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| c.find(0, window, cx)));
    h.settle();
    h.type_text("cache");
    let label = |h: &mut H| {
        let chat = h.chat();
        h.cx.update(|_, cx| {
            let bar = chat.read(cx).find.as_ref().expect("find").bar.clone();
            bar.read(cx).count_label(cx)
        })
    };
    let current = |h: &mut H| {
        let chat = h.chat();
        h.cx.update(|_, cx| {
            let c = chat.read(cx);
            let v = &c.markdown[&format!("{}:m2-md", c.props().reset_key)];
            let mut hl: Vec<_> = v
                .read(cx)
                .highlights()
                .values()
                .flatten()
                .cloned()
                .collect();
            hl.sort_by_key(|(r, _)| r.start);
            hl.iter().map(|(_, cur)| *cur).collect::<Vec<_>>()
        })
    };
    assert_eq!(label(&mut h), "1 / 3");
    assert_eq!(current(&mut h), vec![false, false]);
    let bubble = |h: &mut H| {
        let chat = h.chat();
        h.cx.update(|_, cx| {
            let c = chat.read(cx);
            let (marks, n) = c.bubble_find_marks("Where is the cache?", 0, 0, cx);
            (
                n,
                marks
                    .iter()
                    .map(|(r, fill)| (r.clone(), fill.a))
                    .collect::<Vec<_>>(),
            )
        })
    };
    let (n, marks) = bubble(&mut h);
    assert_eq!((n, marks[0].0.clone()), (1, 13..18));
    let current_fill = marks[0].1;
    h.keys("enter");
    assert!(
        bubble(&mut h).1[0].1 < current_fill,
        "the bubble's mark stayed current"
    );
    assert_eq!(label(&mut h), "2 / 3");
    assert_eq!(current(&mut h), vec![true, false]);
    h.keys("enter");
    assert_eq!(current(&mut h), vec![false, true]);
    h.keys("shift-enter");
    assert_eq!(label(&mut h), "2 / 3");
    assert_eq!(current(&mut h), vec![true, false]);
    h.keys("escape");
    assert!(h.read(|c| !c.find.as_ref().expect("find").open));
    assert_eq!(current(&mut h), Vec::<bool>::new());
}

#[gpui::test]
async fn find_scrolls_a_far_match_in_without_bouncing(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), ws_props(), ws_hooks());
    let msgs = (0..200)
        .map(|i| AgentMsg {
            me: i % 2 == 0,
            body: if i == 1 || i == 199 {
                "Answer 10.\n\nThe zanzibar flag lives here.".into()
            } else {
                format!(
                    "Message {i}.\n\n{}",
                    "- reads `a`, then writes `b` to the store\n".repeat(60)
                )
            },
            ts: Some(i),
            ..Default::default()
        })
        .collect();
    h.set_messages(msgs);
    h.settle();
    recur_content::markdown::set_markdown_render_inline(false);
    assert!(h.read(|c| c.test_window_chat()));
    let chat = h.chat();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| c.find(0, window, cx)));
    h.settle();
    let top = |h: &mut H| h.read(|c| -f32::from(c.test_scroll().offset().y));
    let at_bottom = top(&mut h);
    h.type_text("zanzibar");
    let label = |h: &mut H| {
        let chat = h.chat();
        h.cx.update(|_, cx| {
            let bar = chat.read(cx).find.as_ref().expect("find").bar.clone();
            bar.read(cx).count_label(cx)
        })
    };
    assert_eq!(label(&mut h), "2 / 2", "typing picks the match in view");
    assert_eq!(top(&mut h), at_bottom, "typing scrolled to a match in view");
    h.keys("enter");
    assert_eq!(label(&mut h), "1 / 2");
    recur_ui::guard::count_renders(true);
    for _ in 0..150 {
        h.cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        h.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    let renders: u32 = counts
        .iter()
        .filter(|(k, _)| k.ends_with("::TranscriptPane"))
        .map(|(_, n)| *n)
        .sum();
    assert!(
        renders <= 10,
        "the scroll kept moving: {renders} transcript renders"
    );
    h.settle();
    let chat = h.chat();
    let (needle, view) = h.cx.update(|window, cx| {
        let c = chat.read(cx);
        let v = c.markdown[&format!("{}:m1-md", c.props().reset_key)].read(cx);
        let needle = v.text_blocks().into_iter().find_map(|(id, text)| {
            let start = text.find("zanzibar")?;
            v.range_bounds(id, start..start + 8, window)
        });
        (needle, c.test_scroll().bounds())
    });
    let needle = needle.expect("the match painted");
    assert!(
        needle.top() >= view.top() && needle.bottom() <= view.bottom(),
        "{needle:?} outside {view:?}"
    );
}

#[gpui::test]
async fn find_scrolls_into_a_tall_user_bubble_without_bouncing(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), ws_props(), ws_hooks());
    let msgs = (0..200)
        .map(|i| AgentMsg {
            me: i % 2 == 0,
            body: match i {
                0 => format!(
                    "{}Add a Lithe mock beacon.\n\n{}",
                    "- a plan line with enough words in it to wrap\n".repeat(60),
                    "- more plan lines after it\n".repeat(60)
                ),
                199 => "Done with lithe.".into(),
                _ => format!("Message {i}.\n\n{}", "- reads `a`, writes `b`\n".repeat(20)),
            },
            ts: Some(i),
            ..Default::default()
        })
        .collect();
    h.set_messages(msgs);
    h.settle();
    let chat = h.chat();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| c.find(0, window, cx)));
    h.settle();
    h.type_text("lithe");
    recur_ui::guard::count_renders(true);
    h.keys("enter");
    for _ in 0..150 {
        h.cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        h.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    let renders: u32 = counts
        .iter()
        .filter(|(k, _)| k.ends_with("::TranscriptPane"))
        .map(|(_, n)| *n)
        .sum();
    assert!(
        renders <= 10,
        "the scroll kept moving: {renders} transcript renders"
    );
    let top = h.read(|c| -f32::from(c.test_scroll().offset().y));
    assert!(
        top > 300.,
        "stopped at the bubble's top ({top}), not its match"
    );
}

#[gpui::test]
async fn find_highlights_a_reply_scrolled_back_into_a_windowed_chat(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), ws_props(), ws_hooks());
    let msgs = (0..200)
        .map(|i| AgentMsg {
            me: i % 2 == 0,
            body: if i == 1 || i == 199 {
                "The zanzibar flag lives here.".into()
            } else {
                format!("Message {i}.\n\n{}", "- reads `a`, writes `b`\n".repeat(20))
            },
            ts: Some(i),
            ..Default::default()
        })
        .collect();
    h.set_messages(msgs);
    h.settle();
    assert!(h.read(|c| c.test_window_chat()));
    let chat = h.chat();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| c.find(0, window, cx)));
    h.settle();
    h.type_text("zanzibar");
    h.settle();
    let key = h.read(|c| format!("{}:m1-md", c.props().reset_key));
    assert!(
        h.read(|c| !c.markdown.contains_key(&key)),
        "the far reply hydrated before scrolling"
    );
    let handle = h.read(|c| c.test_scroll().clone());
    h.cx.update(|window, cx| {
        handle.set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
        chat.update(cx, |c, cx| c.notify_transcript(cx));
        window.draw(cx).clear(cx);
    });
    h.settle();
    let marks = h.cx.update(|_, cx| {
        let c = chat.read(cx);
        c.markdown[&key]
            .read(cx)
            .highlights()
            .values()
            .flatten()
            .count()
    });
    assert_eq!(marks, 1, "the scrolled-in reply lost its highlight");
}

#[gpui::test]
async fn find_skips_matches_inside_bubble_chips(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), readonly(), ChatHooks::default());
    h.set_messages(vec![AgentMsg {
        me: true,
        body: "fix @src/cache.rs so the cache clears".into(),
        ts: Some(1),
        ..Default::default()
    }]);
    let chat = h.chat();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| c.find(0, window, cx)));
    h.settle();
    h.type_text("cache");
    let label = h.cx.update(|_, cx| {
        let bar = chat.read(cx).find.as_ref().expect("find").bar.clone();
        bar.read(cx).count_label(cx)
    });
    assert_eq!(label, "1 / 1");
}
