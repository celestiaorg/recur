//! The model pickers' Run in popup: it fits the window like the model Select's menu.

use gpui::{Entity, Modifiers, TestAppContext, px};
use gpui_kit::{AppContext as _, Context, IntoElement, ParentElement, Render, Styled, Window, div};
use recur_agent::catalog::HarnessChoice;
use recur_view_core::models::{CatalogModel, FlagMap};

use super::harness::init;
use crate::pickers::ModelPickers;

/// The pickers in the bottom-right corner of the window.
struct Corner {
    pickers: Entity<ModelPickers>,
}

impl Render for Corner {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .justify_end()
            .items_end()
            .p(px(4.))
            .child(self.pickers.clone())
    }
}

fn harness(id: &str, label: &str) -> HarnessChoice {
    HarnessChoice {
        id: id.into(),
        label: label.into(),
        ..Default::default()
    }
}

fn catalog() -> Vec<CatalogModel> {
    vec![
        CatalogModel {
            r#ref: "claude-code/sonnet".into(),
            label: "Sonnet".into(),
            group: "Claude Code".into(),
            provider: "claude-code".into(),
            ..Default::default()
        },
        CatalogModel {
            r#ref: "deepseek/deepseek-flash".into(),
            label: "DeepSeek Flash".into(),
            group: "DeepSeek".into(),
            provider: "deepseek".into(),
            flags: Vec::new(),
            harnesses: vec![harness("", "Recur"), harness("opencode", "OpenCode")],
        },
    ]
}

#[gpui::test]
fn run_in_popup_stays_inside_the_window_near_the_bottom_right(cx: &mut TestAppContext) {
    init(cx);
    // `up: false`, as in Settings → Models: below would cross the bottom edge.
    let (_, vcx) = cx.add_window_view(|window, cx| {
        let pickers = cx.new(|cx| {
            let mut p = ModelPickers::new(true, false, window, cx);
            p.set(catalog(), "claude-code/sonnet", &FlagMap::new(), None, cx);
            p
        });
        Corner { pickers }
    });
    vcx.update(|window, _| window.activate_window());
    vcx.simulate_resize(gpui::size(px(420.), px(320.)));
    let settle = |vcx: &mut gpui::VisualTestContext| {
        for _ in 0..3 {
            vcx.run_until_parked();
            vcx.update(|window, _| window.refresh());
        }
        vcx.run_until_parked();
    };
    let click = |vcx: &mut gpui::VisualTestContext, sel: &'static str| {
        settle(vcx);
        let p = vcx
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        vcx.simulate_mouse_move(p, None, Modifiers::none());
        vcx.simulate_click(p, Modifiers::none());
        settle(vcx);
    };
    click(vcx, "sel:Model=Sonnet");
    click(vcx, "sel-opt:DeepSeek Flash");

    // The Select shows the picked model while the popup asks where to run it.
    let trigger = vcx
        .debug_bounds("sel:Model=DeepSeek Flash")
        .expect("trigger");
    let menu = vcx.debug_bounds("harness-menu").expect("Run in popup");
    let vp = vcx.update(|w, _| w.viewport_size());
    let (vw, vh) = (f32::from(vp.width), f32::from(vp.height));
    // The trigger hugs the bottom-right corner, so the right edge would overflow too.
    assert!(f32::from(trigger.left()) + 248. > vw, "{trigger:?}");
    assert!(f32::from(menu.left()) >= 0., "{menu:?}");
    assert!(f32::from(menu.top()) >= 0., "{menu:?}");
    assert!(f32::from(menu.right()) <= vw, "{menu:?} in {vw}");
    assert!(f32::from(menu.bottom()) <= vh, "{menu:?} in {vh}");
    // It opens upward, above the trigger.
    assert!(menu.bottom() <= trigger.top(), "{menu:?} over {trigger:?}");
}
