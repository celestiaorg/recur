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
