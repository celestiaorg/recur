//! The right panel floating (the app-wide preference): it takes no column and opens as a flyout
//! at the right edge that slides out, closes on a pick, on Esc and on a press outside, while
//! the chat takes the width the center column leaves. Docked, the layout is unchanged.

use gpui::{TestAppContext, px, size};
use recur_services::fake::Overrides;
use recur_view_core::prefs::{CodePanelKey, PanelKey, PanelLayout};

use super::harness::*;
use crate::model::MainSurface;

const FLOAT_KEY: &str = "recur.sidePanelFloat";

fn mount_float(cx: &mut TestAppContext) -> H<'_> {
    mount(
        cx,
        Overrides::default(),
        |st| st.set(FLOAT_KEY, "true"),
        |_| {},
    )
}

fn open(h: &mut H) -> bool {
    h.read(|o, _| o.show_side)
}

/// Shows `panel` in the right panel: the toolbar's toggle brings it out, and its icons (in the
/// flyout while it floats) switch it.
fn show(h: &mut H, panel: &str) {
    if !open(h) {
        h.click("obs-side-toggle");
    }
    h.click(&format!("panel-toggle:{panel}"));
}

fn width(h: &mut H, sel: &str) -> f32 {
    f32::from(h.bounds(sel).unwrap().size.width)
}

#[gpui::test]
async fn a_panel_icon_opens_the_flyout_at_the_right_edge_and_resizes_nothing(
    cx: &mut TestAppContext,
) {
    let mut h = mount_float(cx);
    assert!(h.read(|o, cx| o.floating(cx)));
    assert!(!open(&mut h), "it rests closed");
    assert!(!h.has("obs-side"));
    let (chat, main) = (width(&mut h, "obs-left"), width(&mut h, "obs-main"));

    show(&mut h, "files");
    assert!(open(&mut h));
    assert!(h.has("obs-side-float"));
    assert!(
        h.has("obs-node:file:src/main.go"),
        "the docked panel's body"
    );
    assert_eq!(width(&mut h, "obs-left"), chat);
    assert_eq!(width(&mut h, "obs-main"), main);
    let (fly, body, bar) = (
        h.bounds("obs-side").unwrap(),
        h.bounds("obs-body").unwrap(),
        h.bounds("obs-toolbar").unwrap(),
    );
    assert_eq!(fly.right(), body.right());
    assert_eq!(fly.top(), bar.bottom(), "under the toolbar");
    assert_eq!(fly.bottom(), body.bottom());
    assert_eq!(
        f32::from(fly.size.width),
        h.state
            .read_with(h.cx, |s, _| s.panels.size(PanelKey::ObsSideFloat)),
        "at its own width"
    );
}

#[gpui::test]
async fn docked_the_panel_icons_come_before_the_rule_and_the_controls(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    let (files, rule, pin, toggle) = (
        h.bounds("panel-toggle:files").unwrap(),
        h.bounds("obs-side-rule").unwrap(),
        h.bounds("obs-float-toggle").unwrap(),
        h.bounds("obs-side-toggle").unwrap(),
    );
    assert!(files.right() <= rule.left());
    assert!(rule.right() <= pin.left() && pin.right() <= toggle.left());
}

#[gpui::test]
async fn docking_shows_the_right_panel(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    assert!(!open(&mut h), "floating, it rests closed");
    h.click("obs-float-toggle");
    assert!(!h.read(|o, cx| o.floating(cx)));
    assert!(open(&mut h) && h.has("obs-side"), "docked, it shows");

    // The same from the shortcut or Settings, which flip the preference itself.
    h.state_update(|s, cx| s.set_side_float(true, cx));
    h.keys("escape");
    assert!(!open(&mut h));
    h.state_update(|s, cx| s.set_side_float(false, cx));
    assert!(open(&mut h) && h.has("obs-side"));
}

#[gpui::test]
async fn undocking_closes_the_right_panel_in_parked_workspaces(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    h.update(|o, window, cx| {
        let mut p = o.props().clone();
        p.shell_active = false;
        o.set_props(p, window, cx);
    });
    h.state_update(|s, cx| s.set_side_float(false, cx));
    assert!(open(&mut h), "docked, every workspace shows it");
    h.state_update(|s, cx| s.set_side_float(true, cx));
    assert!(!open(&mut h), "it is not left out as a flyout");
}

#[gpui::test]
async fn a_flyout_closed_while_parked_does_not_slide_shut_on_return(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    h.update(|_, _, cx| recur_ui::motion::thaw(cx));
    h.click("obs-side-toggle");
    h.wait(400);
    assert!(open(&mut h) && h.has("obs-side"));

    // One update, so nothing draws while parked: hot shells are not rendered.
    h.update(|o, window, cx| {
        let shell_active = |o: &mut crate::Observatory, on, window: &mut _, cx: &mut _| {
            let mut p = o.props().clone();
            p.shell_active = on;
            o.set_props(p, window, cx);
        };
        shell_active(o, false, window, cx);
        for float in [false, true] {
            o.state
                .clone()
                .update(cx, |s, cx| s.set_side_float(float, cx));
            o.after_state_change(window, cx);
        }
        shell_active(o, true, window, cx);
    });
    assert!(!open(&mut h));
    assert!(!h.has("obs-side"), "it is gone at once");
}

#[gpui::test]
async fn the_flyout_dismisses(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    assert!(
        !h.has("panel-toggle:files"),
        "floating, the toolbar has no panel icons"
    );
    let (rule, pin, toggle) = (
        h.bounds("obs-side-rule").unwrap(),
        h.bounds("obs-float-toggle").unwrap(),
        h.bounds("obs-side-toggle").unwrap(),
    );
    assert!(
        rule.right() <= pin.left() && pin.right() <= toggle.left(),
        "the pin and the hide toggle sit together after the rule"
    );
    h.click("obs-side-toggle");
    assert!(open(&mut h));
    let (icon, fly, bar, review) = (
        h.bounds("panel-toggle:search").unwrap(),
        h.bounds("obs-side-float").unwrap(),
        h.bounds("obs-toolbar").unwrap(),
        h.bounds("obs-review-toggle").unwrap(),
    );
    assert!(
        icon.left() >= fly.left() && icon.bottom() <= bar.bottom(),
        "its icons are in the toolbar over it, as when docked"
    );
    assert!(
        review.right() <= fly.left(),
        "the center's toggles move clear of it"
    );
    h.click("panel-toggle:search");
    assert_eq!(h.read(|o, _| o.right_kind), CodePanelKey::Search);
    h.click("panel-toggle:search");
    assert!(open(&mut h), "the icon of the panel showing leaves it");
    let (pin, toggle) = (
        h.bounds("obs-float-toggle").unwrap(),
        h.bounds("obs-side-toggle").unwrap(),
    );
    assert!(pin.right() <= toggle.left(), "the pin is beside the toggle");

    let mirrored = |h: &mut H| h.read(|o, cx| o.state.read(cx).side_float_open);
    assert!(mirrored(&mut h), "the shell knows Esc is the flyout's");
    h.keys("escape");
    assert!(!open(&mut h), "Esc closes it from its own input");
    assert!(!mirrored(&mut h));

    h.click("obs-side-toggle");
    assert!(open(&mut h));
    h.click("obs-side-toggle");
    assert!(!open(&mut h));
    h.keys("secondary-alt-shift-b");
    assert!(open(&mut h));

    h.click("agent-chat");
    assert!(open(&mut h), "a press on the chat leaves it");
    show(&mut h, "files");
    h.click("obs-node:dir:src");
    assert!(open(&mut h), "so does a press in it");
    h.click("obs-review-toggle");
    assert!(open(&mut h), "and one on the toolbar");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);

    let (fly, main) = (
        h.bounds("obs-side-float").unwrap(),
        h.bounds("obs-main").unwrap(),
    );
    let none = gpui::Modifiers::none();
    h.click_at(
        gpui::point(fly.left() - gpui::px(2.), main.center().y),
        none,
    );
    assert!(open(&mut h), "its resize handle is part of it");
    h.click("obs-editor-toggle");
    h.click_at(
        gpui::point(main.left() + gpui::px(40.), main.center().y),
        none,
    );
    assert!(!open(&mut h), "a press on the center column closes it");
    assert_eq!(
        h.read(|o, cx| o.main_surface(cx)),
        MainSurface::Editor,
        "and still reaches what was pressed"
    );

    show(&mut h, "files");
    h.click("obs-float-toggle");
    assert!(open(&mut h), "docking keeps the panel that was showing");
    assert!(h.has("obs-side") && !h.has("obs-side-float"));
}

#[gpui::test]
async fn a_pick_opens_the_document_and_the_flyout_stays(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    h.update(|o, _, cx| o.toggle_review(cx));
    assert!(h.has("obs-review-host"));
    assert!(
        h.has("button:Hide file list") || h.has("button:Show file list"),
        "the review keeps its own file list"
    );

    show(&mut h, "files");
    assert!(h.has("obs-review-host"), "it opens over the review");
    h.click("obs-node:file:src/main.go");
    assert!(open(&mut h));
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Editor);
    assert!(h.active_tab("src/main.go"));

    h.click("obs-pr-toggle");
    assert!(open(&mut h), "the pull request leaves it too");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Pr);
}

#[gpui::test]
async fn the_flyout_has_its_own_width(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    let docked = h
        .state
        .read_with(h.cx, |s, _| s.panels.size(PanelKey::ObsSide));
    show(&mut h, "files");
    h.state_update(|s, cx| s.set_panel(PanelKey::ObsSideFloat, 400., cx));
    assert_eq!(width(&mut h, "obs-side-float"), 400.);
    h.click("obs-float-toggle");
    assert_eq!(width(&mut h, "obs-side"), docked, "docked keeps its own");
}

#[gpui::test]
async fn the_flyout_keeps_its_full_height_over_a_full_width_terminal(cx: &mut TestAppContext) {
    let mut h = mount(
        cx,
        Overrides::default(),
        |st| {
            st.set(FLOAT_KEY, "true");
            st.set("recur.terminalPlacement", "full");
        },
        |_| {},
    );
    h.state_update(|s, cx| s.set_terminal_dock_open(WS, true, cx));
    h.wait(100);
    show(&mut h, "files");
    let (fly, dock, obs) = (
        h.bounds("obs-side-float").unwrap(),
        h.bounds("term-dock").unwrap(),
        h.bounds("obs").unwrap(),
    );
    assert_eq!(fly.bottom(), obs.bottom(), "the right panel is full height");
    assert!(fly.top() < dock.top() && fly.left() < dock.right());
    assert_eq!((dock.left(), dock.right()), (obs.left(), obs.right()));
    assert_eq!(dock.bottom(), obs.bottom());
}

#[gpui::test]
async fn the_flyout_keeps_the_wheel_and_the_pointer_from_what_is_under_it(cx: &mut TestAppContext) {
    let long = (0..200)
        .map(|i| format!("// line {i}\n"))
        .collect::<String>();
    let mut h = mount(
        cx,
        super::observatory::many_files_opts(vec!["src/main.go".into()], long),
        |st| st.set(FLOAT_KEY, "true"),
        |_| {},
    );
    show(&mut h, "files");
    h.click("obs-node:file:src/main.go");
    h.keys("escape");
    if h.has("view-mode:Code") {
        h.click("view-mode:Code");
    }
    show(&mut h, "files");
    let (fly, main) = (
        h.bounds("obs-side-float").unwrap(),
        h.bounds("obs-editors").unwrap(),
    );
    assert!(fly.left() > main.left() && fly.right() <= main.right());
    let top = |h: &mut H| h.read(|o, cx| o.main_editor.as_ref().unwrap().read(cx).scroll_top());
    let wheel = |h: &mut H, position| {
        h.cx.simulate_event(gpui::ScrollEvent {
            position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-40.))),
            ..Default::default()
        });
        h.settle();
    };
    let over_flyout = gpui::point(fly.right() - gpui::px(20.), main.center().y);
    let beside_it = gpui::point(main.left() + gpui::px(60.), main.center().y);
    let before = top(&mut h);
    wheel(&mut h, over_flyout);
    assert_eq!(
        top(&mut h),
        before,
        "the editor under the flyout does not scroll"
    );
    h.click_at(over_flyout, gpui::Modifiers::none());
    assert!(
        open(&mut h),
        "a press on the flyout is not a press outside it"
    );
    let editor_focused = h.cx.update(|window, cx| {
        let e = h.obs.read(cx).main_editor.clone().unwrap();
        gpui_kit::Focusable::focus_handle(e.read(cx), cx).is_focused(window)
    });
    assert!(!editor_focused, "nor a press on the editor under it");
    wheel(&mut h, beside_it);
    assert!(top(&mut h) > before, "beside it the editor scrolls");
}

#[gpui::test]
async fn selecting_a_panel_opens_it_as_the_flyout(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    for k in [
        CodePanelKey::Refs,
        CodePanelKey::Problems,
        CodePanelKey::Search,
    ] {
        h.update(|o, window, cx| o.select_code_panel(k, window, cx));
        assert!(open(&mut h));
        assert!(h.has("obs-side-float"));
        assert_eq!(h.read(|o, cx| o.right_meta(cx)), Some(k));
    }
    h.type_text("retry");
    assert_eq!(
        h.read(|o, cx| o.inputs.query(cx)),
        "retry",
        "the search box takes the keys"
    );
}

#[gpui::test]
async fn the_flyout_slides_out_from_the_right_edge_and_back(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    h.update(|_, _, cx| recur_ui::motion::thaw(cx));
    let edge = h.bounds("obs").unwrap().right();
    let review = h.bounds("obs-review-toggle").unwrap().left();
    h.click("obs-side-toggle");
    assert_eq!(
        h.bounds("obs-side").unwrap().left(),
        edge,
        "starts past the edge"
    );
    assert!(!h.has("panel-toggle:files"), "its icons come out with it");
    h.wait(90);
    let mid = h.bounds("obs-review-toggle").unwrap().left();
    h.wait(400);
    let fly = h.bounds("obs-side").unwrap();
    assert_eq!(fly.right(), edge);
    let moved = h.bounds("obs-review-toggle").unwrap();
    assert!(
        moved.left() < mid && mid < review,
        "the center's toggles slide over with it"
    );
    assert!(moved.right() <= fly.left());

    show(&mut h, "search");
    assert_eq!(
        h.bounds("obs-side").unwrap().right(),
        edge,
        "no slide between two panels"
    );

    h.keys("escape");
    assert!(!open(&mut h));
    assert!(h.has("obs-side"), "slides back before it goes");
    h.wait(400);
    assert!(!h.has("obs-side"));
}

#[gpui::test]
async fn the_chat_flexes_and_the_center_keeps_its_width(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    let center = PanelKey::ObsCenter.default_size();
    assert_eq!(width(&mut h, "obs-work"), center);
    assert_eq!(
        width(&mut h, "obs-left") + center,
        width(&mut h, "obs"),
        "the chat takes the rest"
    );
    assert_eq!(
        h.bounds("obs-tb-center").unwrap().left(),
        h.bounds("obs-work").unwrap().left(),
        "the toolbar lines up with the columns"
    );
    assert_eq!(
        h.bounds("obs-side-toggle").unwrap().right() + px(8.),
        h.bounds("obs").unwrap().right(),
        "the right panel toggle stays the last icon"
    );

    h.cx.simulate_resize(size(px(1500.), px(800.)));
    assert_eq!(width(&mut h, "obs-work"), center);
    assert_eq!(width(&mut h, "obs-left") + center, width(&mut h, "obs"));

    h.state_update(|s, cx| s.set_panel(PanelKey::ObsCenter, 3000., cx));
    assert_eq!(
        width(&mut h, "obs-left"),
        380.,
        "the chat keeps its minimum"
    );

    h.keys("secondary-alt-b");
    assert!(!h.has("obs-left"));
    assert_eq!(
        width(&mut h, "obs-work"),
        width(&mut h, "obs"),
        "with no left dock the center has it all"
    );
}

#[gpui::test]
async fn a_narrow_window_stacks_as_when_docked(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    h.cx.simulate_resize(size(px(900.), px(800.)));
    h.settle();
    assert!(!h.read(|o, cx| o.floating(cx)));
    h.click("obs-side-toggle");
    assert!(h.has("obs-side"));
    assert!(!h.has("obs-side-float"));
    assert_eq!(width(&mut h, "obs-side"), width(&mut h, "obs"));
}

#[gpui::test]
async fn the_preference_persists_and_applies_to_every_workspace(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    assert!(!h.read(|o, cx| o.floating(cx)), "specs start docked");
    h.click("obs-float-toggle");
    assert!(h.read(|o, cx| o.floating(cx)));
    h.switch_to(PROJECT, "LOOP-2229");
    assert!(h.read(|o, cx| o.floating(cx)), "in another workspace too");
    assert!(!open(&mut h));
    assert!(!h.has("obs-side"));
    let saved = h
        .state
        .read_with(h.cx, |s, _| PanelLayout::load(s.storage()).side_float);
    assert_eq!(saved, Some(true));

    h.click("obs-float-toggle");
    h.switch_to(PROJECT, WS);
    assert!(!h.read(|o, cx| o.floating(cx)));
}

#[gpui::test]
async fn docked_the_right_panel_is_a_column_that_stays(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    assert!(open(&mut h));
    assert!(h.has("obs-side"));
    assert!(!h.has("obs-side-float"));
    assert_eq!(
        h.bounds("obs-main").unwrap().right(),
        h.bounds("obs-side").unwrap().left(),
        "beside the center, not over it"
    );
    assert_eq!(
        width(&mut h, "obs-left"),
        h.state
            .read_with(h.cx, |s, _| s.panels.size(PanelKey::MaestroDock)),
        "the chat keeps its own width"
    );

    show(&mut h, "files");
    assert!(open(&mut h), "its icon does not close it");
    h.keys("escape");
    h.click("agent-chat");
    assert!(open(&mut h));
    h.click("obs-node:file:src/main.go");
    assert!(open(&mut h), "opening a file leaves it");
    h.click("obs-review-toggle");
    assert!(open(&mut h));
    assert!(h.has("button:Hide file list"), "the review lists its files");
}

#[gpui::test]
async fn esc_clears_the_file_filter_before_it_closes_the_flyout(cx: &mut TestAppContext) {
    let mut h = mount_float(cx);
    show(&mut h, "files");
    h.click("obs-files-filter");
    h.type_text("main");
    let text = |h: &mut H| h.read(|o, cx| o.inputs.files_filter.read(cx).value().to_string());
    assert_eq!(text(&mut h), "main");

    h.keys("escape");
    assert_eq!(text(&mut h), "");
    assert!(open(&mut h), "the first Esc only clears the text");
    assert!(h.has("obs-node:file:src/main.go"));

    h.keys("escape");
    assert!(!open(&mut h), "with the box empty Esc closes it, as before");
}

#[gpui::test]
async fn a_press_in_a_previewed_page_closes_it(cx: &mut TestAppContext) {
    let opts = Overrides::default()
        .working_status(|_, _| Ok(statuses(&[("README.md", "m")])))
        .read_file(|_, (_, _, path)| Ok(file(&path, "markdown", "# Hello\n", true)));
    let mut h = mount(cx, opts, |st| st.set(FLOAT_KEY, "true"), |_| {});
    h.update(|o, window, cx| {
        o.open_path(crate::OpenRequest::new("README.md"), window, cx)
            .detach()
    });
    h.settle();
    h.update(|o, window, cx| {
        o.set_view_mode(
            recur_editor::view_state::EditorViewMode::Preview,
            window,
            cx,
        )
        .detach()
    });
    h.settle();
    assert!(h.read(|o, _| o.file_preview.is_some()));
    show(&mut h, "files");
    assert!(open(&mut h));
    // macOS hands a press on an HTML page to the page; the preview reports it.
    h.update(|o, _, cx| {
        let (_, fp) = o.file_preview.clone().expect("the preview");
        fp.update(cx, |_, cx| {
            cx.emit(recur_content::FilePreviewEvent::Pressed)
        });
    });
    h.settle();
    assert!(!open(&mut h));
}
