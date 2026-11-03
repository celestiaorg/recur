//! The GTK color chooser port in a window: palette clicks, the editor's plane and hex entry
//! report each color at once; Cancel restores the initial color.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use crate::widgets::color_chooser::{ColorChooser, PALETTE, to_hex};

type Picks = Rc<RefCell<Vec<String>>>;

fn open(cx: &mut TestAppContext) -> (Entity<ColorChooser>, Picks, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let picks: Picks = Rc::default();
    let mut chooser = None;
    let sink = picks.clone();
    let (_, vcx) = cx.add_window_view(|window, cx| {
        let c = cx.new(|cx| {
            ColorChooser::new(
                0x3584e4,
                true,
                Rc::new(move |hex, _| sink.borrow_mut().push(hex)),
                window,
                cx,
            )
        });
        chooser = Some(c.clone());
        gpui_kit::base::Root::new(c, window, cx)
    });
    vcx.run_until_parked();
    (chooser.expect("chooser"), picks, vcx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
        .center();
    cx.simulate_click(at, gpui_kit::Modifiers::none());
    cx.run_until_parked();
}

#[gpui_kit::test]
fn picks_reach_the_page_at_once_and_cancel_restores(cx: &mut TestAppContext) {
    let (chooser, picks, cx) = open(cx);
    click(cx, "swatch-4-1");
    assert_eq!(picks.borrow().last().cloned(), Some(to_hex(PALETTE[4][1])));

    click(cx, "custom-add");
    assert!(cx.debug_bounds("cc-plane").is_some(), "+ opens the editor");
    let plane = cx.debug_bounds("cc-plane").expect("plane");
    // The plane's top-right corner: full saturation and value at the current hue.
    let corner = gpui_kit::point(
        plane.right() - gpui_kit::px(1.),
        plane.top() + gpui_kit::px(1.),
    );
    cx.simulate_click(corner, gpui_kit::Modifiers::none());
    cx.run_until_parked();
    let after_plane = chooser.read_with(cx, |c, _| c.current());
    assert_ne!(after_plane, to_hex(PALETTE[4][1]));
    assert_eq!(picks.borrow().last().cloned(), Some(after_plane));

    click(cx, "cc-entry");
    cx.simulate_keystrokes("secondary-a");
    cx.simulate_input("#00ff00");
    cx.simulate_keystrokes("enter");
    assert_eq!(chooser.read_with(cx, |c, _| c.current()), "#00ff00");

    click(cx, "cc-cancel");
    assert_eq!(picks.borrow().last().cloned(), Some("#3584e4".into()));
}
