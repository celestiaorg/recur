//! Layout probe for parity work: `PROBE_SCENE=chat/transcript PROBE_SELS='a|b' cargo test -p
//! recur-view-chat --lib spec::probe -- --ignored --nocapture` prints the selectors' bounds.

use gpui::{Modifiers, TestAppContext, point, px, size};

#[gpui::test]
#[ignore]
async fn probe(cx: &mut TestAppContext) {
    super::harness::init(cx);
    let scene = std::env::var("PROBE_SCENE").unwrap_or_else(|_| "chat/transcript".into());
    let spec = crate::gallery::specs().into_iter().find(|s| s.name == scene).expect("scene");
    let name = scene.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = crate::gallery::build(&name, window, cx).expect("build");
        Root(view)
    });
    cx.simulate_resize(size(px(spec.width), px(spec.height)));
    cx.run_until_parked();
    for step in &spec.steps {
        if let recur_ui::gallery::Step::Click([x, y]) = step {
            cx.simulate_click(point(px(*x), px(*y)), Modifiers::none());
            cx.run_until_parked();
        }
    }
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    for sel in std::env::var("PROBE_SELS").unwrap_or_default().split('|').filter(|s| !s.is_empty()) {
        let b = cx.debug_bounds(Box::leak(sel.to_string().into_boxed_str()));
        println!("{sel} => {:?}", b.map(|b| (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))));
    }
}

struct Root(gpui_kit::AnyView);

impl gpui_kit::Render for Root {
    fn render(&mut self, _: &mut gpui_kit::Window, _: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement, Styled};
        gpui_kit::div().size_full().child(self.0.clone())
    }
}
