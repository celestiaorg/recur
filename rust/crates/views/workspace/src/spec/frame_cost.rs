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

/// The views a key typed into the composer renders: the dock alone. It is an isolated view,
/// so the frame replays its ancestors (Shell, the workspace, the Observatory, the chat) and
/// everything beside it, and the toolbar and meter cached inside it.
const COMPOSER_PATH: [&str; 1] = ["::DockPane"];

#[gpui::test]
async fn a_keystroke_in_a_long_chat_renders_and_lays_out_only_the_composer(
    cx: &mut TestAppContext,
) {
    keystroke_renders_only_the_composer(cx, false);
}

/// The terminal dock beside the chat is a sibling of the composer's path, so a key reuses it.
#[gpui::test]
async fn a_keystroke_beside_an_open_terminal_does_not_redraw_it(cx: &mut TestAppContext) {
    keystroke_renders_only_the_composer(cx, true);
}

fn keystroke_renders_only_the_composer(cx: &mut TestAppContext, terminal: bool) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    s.open(ACTIVE);
    if terminal {
        let state = s.state.clone();
        s.cx.update(|_, cx| state.update(cx, |st, cx| st.set_terminal_dock_open(ACTIVE, true, cx)));
        s.settle();
    }
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
    // One frame per key, rendering the dock alone: a parked shell (and the review it shows) is
    // not laid out again however many workspaces came before, and the bound review is cached.
    for (view, n) in [
        ("::DockPane", 3),
        ("::Shell", 0),
        ("::WorkspaceView", 0),
        ("::Observatory", 0),
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

/// A shell whose workspace leaves the live list while something still holds it (an
/// archived workspace's turn in flight keeps its view alive) is parked as it is released: two
/// live Observatories pushed their menu states over each other in an endless store loop.
#[gpui::test]
async fn a_shell_released_while_held_is_parked(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    s.open("LOOP-2224");
    let held = bound(&mut s);
    let state = s.state.clone();
    s.cx.update(|_, cx| {
        state.update(cx, |st, cx| {
            let mut w = st.workspace("LOOP-2224").cloned().expect("workspace");
            w.archived = true;
            st.replace_workspace(w, cx);
        })
    });
    s.settle();
    s.open(ACTIVE);
    assert!(
        !held.read_with(s.cx, |v, _| v.shell_active),
        "a released shell still claims the menu"
    );
    let changes = std::rc::Rc::new(std::cell::Cell::new(0));
    let n = changes.clone();
    let _sub = s.cx.update(|_, cx| {
        recur_view_core::observe_state_app(
            &state,
            recur_view_core::Topics::ALL,
            move |_, _| n.set(n.get() + 1),
            cx,
        )
    });
    s.settle();
    assert!(
        changes.get() < 10,
        "the store kept changing: {}",
        changes.get()
    );
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

/// The transcript is an isolated view: a wheel step, and the pin or stub window it moves,
/// redraw it (and what it hosts) without Shell, the route, the workspace, the Observatory or
/// the chat.
#[gpui::test]
async fn a_wheel_step_renders_only_the_transcript(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    s.open(ACTIVE);
    let view = bound(&mut s);
    s.cx.update(|_, cx| {
        view.update(cx, |v, cx| {
            let key = v.active_key();
            v.chat_by_key.insert(key, long_chat(200));
            v.touch(cx);
        })
    });
    s.settle();
    let chat = view.read_with(s.cx, |v, _| v.chat().clone());
    let handle = chat.read_with(s.cx, |c, _| c.test_scroll().clone());
    let at = handle.bounds().center();
    s.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    s.settle();
    recur_ui::guard::count_renders(true);
    for _ in 0..3 {
        s.cx.simulate_event(gpui::ScrollWheelEvent {
            position: at,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(120.))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        s.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    assert!(renders("::TranscriptPane", &counts) >= 3, "{counts:?}");
    for path in [
        "::Shell",
        "::WorkspaceDetail",
        "::WorkspaceView",
        "::Observatory",
        "::DockPane",
    ] {
        assert_eq!(
            renders(path, &counts),
            0,
            "{path} rendered on a wheel step: {counts:?}"
        );
    }
    // Session cards in the transcript host their own (session-body) chats; the workspace's
    // chat itself does not render.
    assert!(
        renders("::AgentChat", &counts) <= renders("::SessionCard", &counts),
        "{counts:?}"
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
    // The findings beside the review, in the left dock.
    s.cx.update(|window, cx| {
        obs.update(cx, |o, cx| {
            o.select_code_panel(recur_view_core::prefs::CodePanelKey::Findings, window, cx)
        })
    });
    s.settle();
    assert!(s.has("obs-left"), "no left dock");
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
    let beside: Vec<_> = counts
        .keys()
        .filter(|k| !k.ends_with("::DraftView"))
        .collect();
    assert!(
        beside.is_empty(),
        "a key rendered beside the text box: {counts:?}"
    );
    for quiet in [
        "::DiffFile",
        "::StackPane",
        "::ReviewSurface",
        "::FilesChangedView",
        "::ReviewFileList",
        "::TranscriptPane",
        "::FindingsPanel",
    ] {
        assert_eq!(renders(quiet, &counts), 0, "{quiet} rendered: {counts:?}");
    }
    let typed = cards
        .iter()
        .find_map(|c| c.read_with(s.cx, |c, cx| c.composer_open().then(|| c.draft_text(cx))));
    assert_eq!(typed.as_deref(), Some("abcd"));
}

/// A key in the commit message renders the box alone, not the source control panel, the
/// Observatory or the chat beside it.
#[gpui::test]
async fn a_key_in_the_commit_message_renders_the_box_only(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    s.open(ACTIVE);
    let obs = bound(&mut s).read_with(s.cx, |v, _| v.observatory().clone());
    s.cx.update(|window, cx| {
        obs.update(cx, |o, cx| {
            o.select_code_panel(recur_view_core::prefs::CodePanelKey::Scm, window, cx)
        })
    });
    s.settle();
    let focused =
        s.cx.update(|window, cx| obs.update(cx, |o, cx| o.focus_commit_msg(window, cx)));
    assert!(focused, "no commit box");
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
    assert!(renders("::FieldView", &counts) >= 3, "{counts:?}");
    let beside: Vec<_> = counts
        .keys()
        .filter(|k| !k.ends_with("::FieldView"))
        .collect();
    assert!(
        beside.is_empty(),
        "a commit key rendered beside the box: {counts:?}"
    );
}

/// The keys that empty the commit message and type into it again redraw its foot (the Commit
/// button), never the Observatory's cached panes (the changes list).
#[gpui::test]
async fn a_key_that_empties_the_commit_message_leaves_the_panes_alone(cx: &mut TestAppContext) {
    let mut s = boot(cx, |_| {}, MockOptions::default(), |_| {});
    s.open(ACTIVE);
    let obs = bound(&mut s).read_with(s.cx, |v, _| v.observatory().clone());
    let focused =
        s.cx.update(|window, cx| obs.update(cx, |o, cx| o.focus_commit_msg(window, cx)));
    assert!(focused, "no commit box");
    s.settle();
    s.cx.simulate_input("a");
    s.cx.run_until_parked();
    recur_ui::guard::count_renders(true);
    for k in ["backspace", "b", "backspace", "c"] {
        s.cx.simulate_keystrokes(k);
        s.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    assert!(
        renders("::Part<recur_view_observatory::obs::Observatory>", &counts) >= 4,
        "{counts:?}"
    );
    assert_eq!(
        renders("::Pane<recur_view_observatory::obs::Observatory>", &counts),
        0,
        "{counts:?}"
    );
}
