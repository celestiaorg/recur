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

/// The views GPUI must render for a key typed into the composer: the composer's dock and its
/// ancestors (a notify marks every ancestor view dirty, and an uncached view renders when its
/// parent does). Everything else on screen is a cached boundary and is reused as it was.
const COMPOSER_PATH: [&str; 6] = [
    "::Shell",
    "::WorkspaceDetail",
    "::WorkspaceView",
    "::Observatory",
    "::AgentChat",
    "::DockPane",
];

#[gpui::test]
async fn a_keystroke_in_a_long_chat_renders_and_lays_out_only_the_composer(
    cx: &mut TestAppContext,
) {
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
    recur_ui::guard::count_layouts(true);
    for ch in ["b", "c", "d"] {
        s.cx.simulate_input(ch);
        s.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    let layouts = recur_ui::guard::take_layout_counts();
    recur_ui::guard::count_renders(false);
    recur_ui::guard::count_layouts(false);
    assert_eq!(chat.read_with(s.cx, |c, _| c.input().to_string()), "abcd");
    for (view, n) in &counts {
        assert!(
            COMPOSER_PATH.iter().any(|p| view.ends_with(p)),
            "{view} rendered on a keystroke: {counts:?}"
        );
        // One frame per key: nothing a render does may schedule another.
        assert_eq!(*n, 3, "{view}: {counts:?}");
    }
    for p in COMPOSER_PATH {
        assert_eq!(renders(p, &counts), 3, "{p}: {counts:?}");
    }
    assert!(
        layouts.is_empty(),
        "text laid out outside the composer: {layouts:?}"
    );
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
    // One frame per key, and only the composer's path in it: a parked shell (and the review it
    // shows) is not laid out again however many workspaces came before, and the bound review
    // is cached.
    for (view, n) in [
        ("::Shell", 3),
        ("::WorkspaceView", 3),
        ("::Observatory", 3),
        ("::ReviewSurface", 0),
        ("::FilesChangedView", 0),
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

#[gpui::test]
async fn a_key_in_a_line_note_redraws_its_text_box_not_the_review(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    s.open(ACTIVE);
    let obs = bound(&mut s).read_with(s.cx, |v, _| v.observatory().clone());
    s.cx.update(|_, cx| obs.update(cx, |o, cx| o.click_review_toggle(cx)));
    s.settle();
    let cards: Vec<_> = obs.read_with(s.cx, |o, cx| {
        o.review_surface()
            .map(|r| {
                r.read(cx)
                    .files_view()
                    .read(cx)
                    .stack()
                    .diff_files()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    });
    let opened = cards.iter().any(|card| {
        s.cx.update(|window, cx| card.update(cx, |c, cx| c.open_first_note(window, cx)))
    });
    assert!(opened, "no card took a line note");
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
    assert!(renders("::DraftView", &counts) >= 3, "{counts:?}");
    for quiet in [
        "::DiffFile",
        "::StackPane",
        "::ReviewSurface",
        "::FilesChangedView",
        "::ReviewFileList",
        "::TranscriptPane",
    ] {
        assert_eq!(renders(quiet, &counts), 0, "{quiet} rendered: {counts:?}");
    }
    let typed = cards
        .iter()
        .find_map(|c| c.read_with(s.cx, |c, cx| c.composer_open().then(|| c.draft_text(cx))));
    assert_eq!(typed.as_deref(), Some("abcd"));
}
