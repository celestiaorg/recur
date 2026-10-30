//! The composer's behavior: prompt chips with arg slots and the wire caret. Hydrate /
//! serialize and chip labels are `recur_engine::view::composer`'s tests.

use gpui::{
    AppContext as _, InteractiveElement as _, ParentElement as _, Styled as _, TestAppContext,
};
use recur_engine::view::composer::Segment;
use recur_services::fake::Overrides;

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
    let mut h = boot(cx, Overrides::default(), props, ws_hooks());
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
    let mut h = boot(cx, Overrides::default(), ws_props(), ws_hooks());
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

#[gpui::test]
async fn a_draft_past_six_lines_follows_the_caret_and_scrolls(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), ws_props(), ws_hooks());
    h.focus_composer();
    h.type_text(&"the quick brown fox jumps over the lazy dog ".repeat(30));
    let scroll = h.read(|c| c.composer_scroll.clone());
    let max = scroll.max_offset().y;
    assert!(max > gpui::px(0.), "the draft overflows the composer");
    assert!(
        scroll.offset().y < gpui::px(6.) - max,
        "the new last line is in view: {:?} of {max:?}",
        scroll.offset().y
    );
    let bottom = scroll.offset().y;
    let at = scroll.bounds().center();
    h.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    h.cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(200.))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    h.settle();
    assert_eq!(
        scroll.offset().y,
        bottom + gpui::px(200.),
        "the wheel scrolls up"
    );
}

/// The composer is as tall as its lines in the frame that adds one, not a frame later.
#[gpui::test]
async fn a_new_line_grows_the_composer_in_its_own_frame(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), ws_props(), ws_hooks());
    h.focus_composer();
    h.type_text("a");
    h.keys("shift-enter");
    let mut was = h.bounds("chat-composer").expect("the composer").size.height;
    for _ in 0..3 {
        h.cx.simulate_keystrokes("shift-enter");
        h.cx.run_until_parked();
        let now =
            h.cx.debug_bounds("chat-composer")
                .expect("the composer")
                .size
                .height;
        assert!(now > was, "{now:?} after {was:?}");
        h.settle();
        assert_eq!(h.cx.debug_bounds("chat-composer").unwrap().size.height, now);
        was = now;
    }
}

struct Grown {
    fit: std::rc::Rc<recur_ui::isolated::Fit>,
}

impl gpui::Render for Grown {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        self.fit.content(
            gpui::div()
                .h(gpui::px(80.))
                .debug_selector(|| "grown".into()),
        )
    }
}

struct StaleHost {
    dock: gpui::Entity<Grown>,
}

impl gpui::Render for StaleHost {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let mut stale = gpui::div().w_full().flex_none().h(gpui::px(50.));
        gpui::div()
            .size_full()
            .flex()
            .flex_col()
            .justify_end()
            .child(recur_ui::style::isolated_view(
                &self.dock,
                stale.style().clone(),
                cx,
            ))
    }
}

/// The frame a dock grows in, before its host lays it out at the new height: it grows at
/// its top, so its bottom row stays where it was.
#[gpui::test]
async fn a_grown_dock_overflows_its_stale_box_upward(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, cx| StaleHost {
        dock: cx.new(|_| Grown {
            fit: std::rc::Rc::default(),
        }),
    });
    cx.simulate_resize(gpui::size(gpui::px(200.), gpui::px(500.)));
    cx.run_until_parked();
    let b = cx.debug_bounds("grown").expect("the dock's content");
    assert_eq!(b.bottom(), gpui::px(500.), "{b:?}");
}

/// A new line past the composer's last scrolls it into view in the frame that lays it out:
/// a frame at the old offset would draw the caret low and then jump it up.
#[gpui::test]
async fn a_new_line_at_the_bottom_scrolls_in_its_own_frame(cx: &mut TestAppContext) {
    let mut h = boot(cx, Overrides::default(), ws_props(), ws_hooks());
    h.focus_composer();
    h.type_text(&"line\n".repeat(8));
    let scroll = h.read(|c| c.composer_scroll.clone());
    recur_ui::guard::count_renders(true);
    h.cx.simulate_keystrokes("shift-enter");
    h.cx.run_until_parked();
    let counts = recur_ui::guard::take_render_counts();
    recur_ui::guard::count_renders(false);
    let docks: u32 = counts
        .iter()
        .filter(|(k, _)| k.ends_with("::DockPane"))
        .map(|(_, n)| *n)
        .sum();
    assert_eq!(docks, 1, "{counts:?}");
    assert!(scroll.offset().y < gpui::px(6.) - scroll.max_offset().y);
}
