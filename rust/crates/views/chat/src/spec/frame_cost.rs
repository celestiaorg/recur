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
