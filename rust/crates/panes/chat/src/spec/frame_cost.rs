//! What a frame re-renders and lays out in a long chat: typing never touches the transcript,
//! and a long windowed transcript lays out as a handful of nodes plus its live blocks.

use gpui::TestAppContext;
use recur_engine::view::chat::AgentMsg;
use recur_services::mock::MockOptions;

use super::harness::*;

fn long_chat(turns: usize) -> Vec<AgentMsg> {
    let mut out = Vec::new();
    for i in 0..turns {
        out.push(AgentMsg {
            me: true,
            body: format!("question {i}: what does `step_{i}` do?"),
            ts: Some(i as i64 * 10),
            ..Default::default()
        });
        out.push(AgentMsg {
            body: format!(
                "Answer {i}. It **parses** the input, then:\n\n- reads `a`\n- writes `b`\n\nDone."
            ),
            ts: Some(i as i64 * 10 + 1),
            ..Default::default()
        });
    }
    out
}

fn renders(name: &str, counts: &std::collections::BTreeMap<&'static str, u32>) -> u32 {
    counts
        .iter()
        .filter(|(k, _)| k.ends_with(name))
        .map(|(_, n)| *n)
        .sum()
}

#[gpui::test]
async fn typing_redraws_the_dock_but_not_the_transcript(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks());
    h.set_messages(long_chat(300));
    h.focus_composer();
    // The first key after a click switches GPUI's input modality, which refreshes the whole
    // window once; steady-state typing is what is measured.
    h.cx.simulate_input("a");
    h.cx.run_until_parked();
    recur_ui::guard::count_renders(true);
    for ch in ["b", "c", "d"] {
        h.cx.simulate_input(ch);
        h.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    assert_eq!(h.input(), "abcd");
    assert!(renders("::DockPane", &counts) >= 3, "{counts:?}");
    assert_eq!(renders("::TranscriptPane", &counts), 0, "{counts:?}");
}

#[gpui::test]
async fn a_long_transcript_mounts_a_bounded_window(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks());
    h.set_messages(long_chat(400));
    h.settle();
    let live = h.read(|c| c.test_live_block_count());
    let total = h.read(|c| c.blocks(&c.transcript()).0.len());
    assert!(total >= 800, "{total}");
    assert!(
        live <= crate::window::CHAT_HYDRATE_MAX + crate::window::CHAT_TAIL_LIVE,
        "{live} of {total} blocks laid out"
    );
}

#[gpui::test]
async fn the_wheel_redraws_the_cached_transcript(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks());
    h.set_messages(long_chat(2));
    let handle = h.read(|c| c.test_scroll().clone());
    let at = handle.bounds().center();
    h.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    h.cx.run_until_parked();
    recur_ui::guard::count_renders(true);
    h.cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(1e6))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    h.cx.run_until_parked();
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    // The scroller laid out again and clamped the offset: the transcript fits.
    assert!(renders("::TranscriptPane", &counts) >= 1, "{counts:?}");
    assert_eq!(f32::from(handle.offset().y), 0.);
}

/// A cached child its parent builds while rendering.
struct Lazy {
    child: Option<gpui::Entity<Leaf>>,
    plain: bool,
}

struct Leaf;

impl gpui_kit::Render for Leaf {
    fn render(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        recur_ui::guard::view(self, window, cx, |_, _, _| gpui_kit::div())
    }
}

impl gpui_kit::Render for Lazy {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::{AppContext as _, IntoElement as _, ParentElement as _, Styled as _};
        let child = self.child.get_or_insert_with(|| cx.new(|_| Leaf)).clone();
        let style = gpui_kit::StyleRefinement::default().size_full();
        gpui_kit::div().size_full().child(if self.plain {
            child.cached(style).into_any_element()
        } else {
            recur_ui::style::cached_view(&child, style, cx)
        })
    }
}

/// How often the leaf renders after its first frames when it notifies, built `plain` (GPUI's
/// `cached`) or through `cached_view`.
fn leaf_renders_on_notify(cx: &mut TestAppContext, plain: bool) -> u32 {
    init(cx);
    let (host, cx) = cx.add_window_view(move |_, _| Lazy { child: None, plain });
    cx.run_until_parked();
    for _ in 0..3 {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    let leaf = host.read_with(cx, |h, _| h.child.clone()).expect("leaf");
    recur_ui::guard::count_renders(true);
    cx.update(|_, cx| leaf.update(cx, |_, cx| cx.notify()));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let n = renders("::Leaf", &recur_ui::guard::take_render_counts());
    recur_ui::guard::count_renders(false);
    n
}

#[gpui::test]
async fn a_cached_view_built_in_its_parents_render_redraws_on_notify(cx: &mut TestAppContext) {
    assert!(leaf_renders_on_notify(cx, false) >= 1);
}

/// Why `cached_view` reads the entity: GPUI drops the notify of a cached view built in its
/// parent's render. If this starts failing, GPUI fixed it and the read can go.
#[gpui::test]
async fn gpui_drops_notifies_of_a_lazily_built_cached_view(cx: &mut TestAppContext) {
    assert_eq!(leaf_renders_on_notify(cx, true), 0);
}

#[gpui::test]
async fn store_changes_redraw_the_transcript_only_when_they_concern_it(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks());
    h.set_messages(long_chat(40));
    let state = h.read(|c| c.state.clone());
    let pane_renders = |h: &mut H, f: &dyn Fn(&mut recur_view_core::AppState)| {
        recur_ui::guard::count_renders(true);
        h.cx.update(|_, cx| {
            state.update(cx, |s, cx| {
                f(s);
                cx.notify();
            })
        });
        h.cx.run_until_parked();
        let n = renders("::TranscriptPane", &recur_ui::guard::take_render_counts());
        recur_ui::guard::count_renders(false);
        n
    };
    // Another workspace's permission card: not this transcript's.
    let other: recur_domain::PermissionRequest = serde_json::from_value(serde_json::json!({
        "id": "perm-x", "workspaceId": "LOOP-9999", "agentCode": "PA", "surface": "chat",
        "tool": "Bash", "input": "{}", "createdAt": 1
    }))
    .unwrap();
    assert_eq!(
        pane_renders(&mut h, &|s| s.permission_requests.push(other.clone())),
        0
    );
    let mine: recur_domain::PermissionRequest = serde_json::from_value(serde_json::json!({
        "id": "perm-ma", "workspaceId": WS, "agentCode": "MA", "surface": "chat",
        "tool": "Bash", "input": "{}", "createdAt": 1
    }))
    .unwrap();
    assert!(pane_renders(&mut h, &|s| s.permission_requests.push(mine.clone())) >= 1);
}

#[gpui::test]
async fn a_key_in_a_long_draft_lays_out_its_paragraph_and_the_visible_lines(
    cx: &mut TestAppContext,
) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks());
    let para = "The quick brown fox jumps over the lazy dog and naps in the sun. ".repeat(3);
    let draft: String = (0..200).map(|i| format!("{i}: {para}\n")).collect();
    let chat = h.chat();
    h.cx.update(|_, cx| chat.update(cx, |c, cx| c.set_draft(draft.clone(), Vec::new(), cx)));
    h.settle();
    h.focus_composer();
    h.cx.simulate_input("x");
    h.cx.run_until_parked();
    let before = crate::flow::para_layouts();
    recur_ui::guard::count_layouts(true);
    for ch in ["y", "z", "w"] {
        h.cx.simulate_input(ch);
        h.cx.run_until_parked();
    }
    let layouts = recur_ui::guard::take_layout_counts();
    recur_ui::guard::count_layouts(false);
    // The paragraph typed in, at each width layout measures the composer at (its max-content
    // width and its box), whatever the draft's length.
    let paras = crate::flow::para_layouts() - before;
    assert!(paras <= 3 * 2, "{paras} paragraphs laid out for 3 keys");
    // The composer places the lines its scroller can show, not the draft's ~600.
    let placed = renders("::DockPane", &layouts);
    assert!(
        placed < 3 * 80,
        "{placed} text runs laid out for 3 keys: {layouts:?}"
    );
}

/// A chat mounted fresh (a workspace shell built on a switch) lays its dock out at the dock's
/// height and draws its draft's text and chips in its first frame: nothing in it asks for a
/// second one. A test update draws until the window is clean, so the counts are the check.
#[gpui::test]
async fn a_new_chat_draws_its_dock_in_the_first_frame(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks());
    let draft = "look at @src/main.rs please";
    h.fill(draft);
    let composer = h.bounds("chat-composer").expect("composer");
    let chip = h.bounds("chip:main.rs").expect("draft chip");
    let (host, old) = (h.host.clone(), h.chat());
    recur_ui::guard::count_renders(true);
    h.cx.update(|window, cx| {
        let c = old.read(cx);
        let vcx = recur_view_core::ViewContext {
            state: c.state.clone(),
            services: c.services.clone(),
        };
        let chat = gpui_kit::AppContext::new(cx, |cx| {
            crate::chat::AgentChat::new(&vcx, ws_props(), window, cx)
        });
        chat.update(cx, |c, cx| {
            c.set_hooks(ws_hooks(), cx);
            c.set_draft(draft.into(), Vec::new(), cx);
        });
        host.update(cx, |h, cx| {
            h.chat = chat;
            cx.notify();
        });
    });
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    assert_eq!(renders("::AgentChat", &counts), 1, "{counts:?}");
    assert_eq!(renders("::DockPane", &counts), 1, "{counts:?}");
    assert_eq!(h.cx.debug_bounds("chat-composer"), Some(composer));
    assert_eq!(h.cx.debug_bounds("chip:main.rs"), Some(chip));
}
