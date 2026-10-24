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
    let view = s
        .cx
        .update(|_, cx| WorkspaceDetail::current(cx)?.read(cx).bound_view(cx))
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
    recur_ui::guard::count_renders(true);
    for ch in ["a", "b", "c"] {
        s.cx.simulate_input(ch);
        s.cx.run_until_parked();
    }
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    assert_eq!(chat.read_with(s.cx, |c, _| c.input().to_string()), "abc");
    assert!(renders("::DockPane", &counts) >= 3, "{counts:?}");
    for quiet in ["::TranscriptPane", "::Sidebar", "::MarkdownView"] {
        assert_eq!(renders(quiet, &counts), 0, "{quiet} re-rendered: {counts:?}");
    }
}
