//! What a keystroke in a long chat re-renders in the whole shell: the dock and the views on
//! its path to the root, never the transcript, the sidebar or a parked workspace.

use std::collections::BTreeMap;

use gpui::TestAppContext;
use recur_engine::view::chat::AgentMsg;
use recur_services::mock::MockOptions;

use super::shell::boot;
use crate::detail::WorkspaceDetail;

const ACTIVE: &str = "LOOP-2231";

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
            body: format!("Answer {i}. It **parses** the input:\n\n- reads `a`\n- writes `b`"),
            ts: Some(i as i64 * 10 + 1),
            ..Default::default()
        });
    }
    out
}

fn renders(name: &str, counts: &BTreeMap<&'static str, u32>) -> u32 {
    counts
        .iter()
        .filter(|(k, _)| k.ends_with(name))
        .map(|(_, n)| *n)
        .sum()
}

#[gpui::test]
async fn a_keystroke_in_a_long_chat_redraws_only_the_dock_path(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    s.open(ACTIVE);
    let view =
        s.cx.update(|_, cx| WorkspaceDetail::current(cx)?.read(cx).bound_view(cx))
            .expect("bound view");
    s.cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            let key = v.active_key();
            v.chat_by_key.insert(key, long_chat(300));
            v.touch(cx);
        })
    });
    s.settle();
    let chat = view.read_with(s.cx, |v, _| v.chat().clone());
    s.cx.update(|window, cx| chat.update(cx, |c, cx| c.focus_composer(window, cx)));
    s.settle();
    // The first key after a click switches GPUI's input modality, which refreshes the whole
    // window once; steady-state typing is what is measured.
    s.cx.simulate_input("a");
    s.cx.run_until_parked();
    recur_ui::guard::count_renders(true);
    for ch in ["b", "c", "d"] {
        s.cx.simulate_input(ch);
        s.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    assert_eq!(chat.read_with(s.cx, |c, _| c.input().to_string()), "abcd");
    assert!(renders("::DockPane", &counts) >= 3, "{counts:?}");
    // One frame per key: nothing a render does may schedule another.
    assert_eq!(renders("::Shell", &counts), 3, "{counts:?}");
    for quiet in ["::TranscriptPane", "::Sidebar", "::MarkdownView"] {
        assert_eq!(
            renders(quiet, &counts),
            0,
            "{quiet} re-rendered: {counts:?}"
        );
    }
}

/// The bound workspace view after `open`.
fn bound(s: &mut super::shell::S) -> gpui::Entity<crate::view::WorkspaceView> {
    s.cx.update(|_, cx| WorkspaceDetail::current(cx)?.read(cx).bound_view(cx))
        .expect("bound view")
}

#[gpui::test]
async fn a_long_browse_leaves_parked_workspaces_cached(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    // Each workspace opens its review, so a parked shell holds the heaviest surface there is.
    let mut visited = Vec::new();
    for id in ["LOOP-2224", "LOOP-2229", "LOOP-2218", "LOOP-2235", ACTIVE] {
        s.open(id);
        let view = bound(&mut s);
        visited.push(view.downgrade());
        let obs = view.read_with(s.cx, |v, _| v.observatory().clone());
        s.cx.update(|_, cx| obs.update(cx, |o, cx| o.click_review_toggle(cx)));
        s.settle();
    }
    // Only the hot shells stay alive: a shell that left them (and its chat, Observatory and
    // review) is released, so nothing a visit built outlives it.
    let alive = visited.iter().filter(|v| v.upgrade().is_some()).count();
    assert_eq!(
        alive,
        recur_view_observatory::hot_shells::HOT_SHELL_MAX,
        "workspace views alive after a browse"
    );
    let chat = bound(&mut s).read_with(s.cx, |v, _| v.chat().clone());
    s.cx.update(|window, cx| chat.update(cx, |c, cx| c.focus_composer(window, cx)));
    s.settle();
    s.cx.simulate_input("a");
    s.cx.run_until_parked();
    recur_ui::guard::count_renders(true);
    for ch in ["b", "c", "d"] {
        s.cx.simulate_input(ch);
        s.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    // One frame per key, and only the bound workspace's path in it: a parked shell (and the
    // review it shows) is not laid out again however many workspaces came before. The bound
    // review's host redraws with its Observatory; its cards and file list are cached.
    for (view, n) in [
        ("::Shell", 3),
        ("::WorkspaceView", 3),
        ("::Observatory", 3),
        ("::ReviewSurface", 3),
        ("::StackPane", 0),
        ("::ReviewFileList", 0),
        ("::DiffFile", 0),
        ("::TranscriptPane", 0),
    ] {
        assert_eq!(renders(view, &counts), n, "{view}: {counts:?}");
    }
}

#[gpui::test]
async fn the_wheel_scrolls_the_live_chat_over_a_parked_one(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    for id in ["LOOP-2229", ACTIVE] {
        s.open(id);
        let view = bound(&mut s);
        s.cx.update(|_, cx| {
            view.update(cx, |v, cx| {
                let key = v.active_key();
                v.chat_by_key.insert(key, long_chat(60));
                v.touch(cx);
            })
        });
        s.settle();
    }
    let chat = bound(&mut s).read_with(s.cx, |v, _| v.chat().clone());
    let handle = chat.read_with(s.cx, |c, _| c.test_scroll().clone());
    let before = f32::from(handle.offset().y);
    let at = handle.bounds().center();
    s.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    s.cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(300.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    s.settle();
    let after = f32::from(handle.offset().y);
    assert!(
        after > before + 100.,
        "the live chat did not scroll: {before} -> {after}"
    );
}

#[gpui::test]
async fn a_parked_chat_comes_back_where_it_was_scrolled(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    s.open(ACTIVE);
    let view = bound(&mut s);
    s.cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            let key = v.active_key();
            v.chat_by_key.insert(key, long_chat(60));
            v.touch(cx);
        })
    });
    s.settle();
    let chat = view.read_with(s.cx, |v, _| v.chat().clone());
    let handle = chat.read_with(s.cx, |c, _| c.test_scroll().clone());
    let at = handle.bounds().center();
    s.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    s.cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(1e6))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    s.settle();
    s.cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-900.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    s.settle();
    let before = -f32::from(handle.offset().y);
    assert!(before > 100., "scrolled to {before}");
    s.open("LOOP-2229");
    s.open(ACTIVE);
    assert_eq!(-f32::from(handle.offset().y), before);
}
