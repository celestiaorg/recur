//! `composerDom.test.ts` cases that are about the composer's behavior rather than its DOM:
//! prompt chips with arg slots and the wire caret. The rest of that file
//! (hydrate / serialize, chip labels) is `recur_engine::view::composer`'s tests.

use gpui::TestAppContext;
use recur_engine::view::composer::Segment;
use recur_services::mock::MockOptions;

use super::harness::*;

fn prompt_chips(h: &mut H) -> usize {
    h.read(|c| {
        c.doc
            .chips
            .iter()
            .filter(|s| matches!(s, Segment::Prompt { .. }))
            .count()
    })
}

fn skill_chips(h: &mut H) -> usize {
    h.read(|c| {
        c.doc
            .chips
            .iter()
            .filter(|s| matches!(s, Segment::Skill(_)))
            .count()
    })
}

/// A `/fix-checks <check>` draft once the prompt index has loaded.
fn with_prompt<'a>(cx: &'a mut TestAppContext, draft: &str) -> H<'a> {
    let mut props = ws_props();
    props.agent_code = "PA".into();
    let mut h = boot(cx, MockOptions::default(), props, ws_hooks());
    h.focus_composer();
    h.type_text("/");
    h.wait(50);
    h.fill(draft);
    h
}

fn slots(h: &mut H) -> Vec<String> {
    let chat = h.chat();
    h.cx.update(|_, cx| {
        chat.read(cx)
            .prompt_slots
            .iter()
            .map(|s| s.read(cx).value().to_string())
            .collect()
    })
}

#[gpui::test]
async fn hydrate_does_not_skill_chip_a_takes_args_prompt(cx: &mut TestAppContext) {
    let mut h = with_prompt(cx, "/fix-checks lint");
    assert_eq!(prompt_chips(&mut h), 1);
    assert_eq!(skill_chips(&mut h), 0);
    assert_eq!(h.input(), "/fix-checks lint");
    assert_eq!(slots(&mut h), ["lint"]);
}

#[gpui::test]
async fn empty_args_serialize_with_a_trailing_space(cx: &mut TestAppContext) {
    let mut h = with_prompt(cx, "/fix-checks ");
    assert_eq!(prompt_chips(&mut h), 1);
    assert_eq!(h.input(), "/fix-checks ");
}

#[gpui::test]
async fn typing_in_a_prompt_arg_keeps_its_focus_and_updates_the_wire(cx: &mut TestAppContext) {
    let mut h = with_prompt(cx, "/fix-checks ");
    let chat = h.chat();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| c.focus_prompt_slot(0, window, cx)));
    h.settle();
    h.type_text("E");
    let focused =
        h.cx.update(|window, cx| chat.read(cx).focused_slot(window, cx));
    assert_eq!(focused, Some(0));
    assert_eq!(h.input(), "/fix-checks E");
}

#[gpui::test]
async fn backspace_in_a_prompt_arg_input_does_not_delete_the_chip(cx: &mut TestAppContext) {
    let mut h = with_prompt(cx, "/fix-checks ENG");
    let chat = h.chat();
    h.cx.update(|window, cx| {
        chat.update(cx, |c, cx| {
            c.focus_prompt_slot(0, window, cx);
            let slot = c.prompt_slots[0].clone();
            slot.update(cx, |s, cx| {
                s.set_cursor_position(gpui_kit::base::input::Position::new(0, 3), window, cx)
            });
        })
    });
    h.settle();
    h.keys("backspace");
    assert_eq!(prompt_chips(&mut h), 1);
    assert_eq!(h.input(), "/fix-checks EN");
    h.keys("delete");
    assert_eq!(prompt_chips(&mut h), 1);
}

#[gpui::test]
async fn backspace_at_an_empty_first_slot_removes_the_chip(cx: &mut TestAppContext) {
    let mut h = with_prompt(cx, "/fix-checks ");
    let chat = h.chat();
    h.cx.update(|window, cx| chat.update(cx, |c, cx| c.focus_prompt_slot(0, window, cx)));
    h.settle();
    h.keys("backspace");
    assert_eq!(prompt_chips(&mut h), 0);
    let focused =
        h.cx.update(|window, cx| chat.read(cx).focus.is_focused(window));
    assert!(focused, "focus returns to the composer");
}

#[gpui::test]
async fn an_artifact_chip_counts_in_the_caret_wire_length(cx: &mut TestAppContext) {
    let mut h = boot(cx, MockOptions::default(), ws_props(), ws_hooks());
    h.fill("see @artifact:abc-def x");
    let chat = h.chat();
    h.cx.update(|_, cx| {
        chat.update(cx, |c, _| {
            let end = c.doc.text.len();
            c.doc.move_to(end, false);
        })
    });
    assert_eq!(
        h.read(|c| c.doc.wire_caret()),
        "see @artifact:abc-def x".len()
    );
}
