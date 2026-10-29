//! `settings-newloop.spec.ts`: the project-settings scenarios.

use gpui::{Modifiers, TestAppContext, point, px};
use recur_ui::text::{TextStyled, text};
use recur_ui::widgets::ToastKind;
use recur_view_core::ViewName;

use super::harness::{S, plain};

/// The sidebar row's settings button (`.tproj-h` hover, then `.pset`).
pub fn open_project(s: &mut S, id: &str) {
    s.hover(&format!("ph:{id}"));
    s.click(&format!("ph-set:{id}"));
    assert_eq!(s.view(), ViewName::Project);
    assert!(s.has("v-project:on"));
}

/// Clicks the link span of a `.set-lead` (`lead-link:{section}`) that starts after `before`.
fn click_lead_link(s: &mut S, section: &str, before: &str) {
    let b = s.bounds(&format!("lead-link:{section}")).expect("lead");
    let w = s.cx.update(|window, _| {
        let (w, _, _) = text(before.to_string())
            .mono()
            .size(12.)
            .measure(None, window);
        w
    });
    let p = point(b.origin.x + px(w + 24.), b.origin.y + px(10.));
    s.click_at(p, Modifiers::none());
}

#[gpui::test]
fn project_skills_links_to_settings_skills(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_project(&mut s, "storefront");
    s.click("set-nav:Skills");
    click_lead_link(
        &mut s,
        "skills",
        "Repository skills for this project. Recur and personal skills follow ",
    );
    assert_eq!(s.view(), ViewName::Settings);
    assert!(s.has("set-head=Skills"));
}

#[gpui::test]
fn project_mcp_links_to_settings_mcp(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_project(&mut s, "storefront");
    s.click("set-nav:MCP");
    click_lead_link(&mut s, "mcp", "MCP tools for this project follow ");
    assert_eq!(s.view(), ViewName::Settings);
    assert!(s.has("set-head=MCP"));
}

#[gpui::test]
fn project_settings_files_to_copy_previews_matching_paths(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_project(&mut s, "storefront");
    s.click("set-nav:Run");
    assert!(s.has("set-head=Run"));
    assert!(s.has("inp:Files to copy"));
    s.wait(300);
    assert!(s.has("copy-files-meta=Nothing will be copied"));
    s.click("inp:Files to copy");
    s.type_text(".env*");
    s.wait(300);
    assert!(s.until(|s| {
        s.has("copy-files-meta=3 files will be copied from /Users/josh/code/storefront")
    }));
    assert!(s.has("copy-file:apps/web/.env"));
    assert!(s.has("copy-file:deploy/.env.mainnet"));
    s.wait(500);
    assert!(s.until(|s| s.toasted(ToastKind::Success, "Project saved")));
}

#[gpui::test]
fn project_settings_highlights_the_project_row(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_project(&mut s, "storefront");
    assert!(s.has("on:ph:storefront"));
    let loops: Vec<String> = s.state.read_with(s.cx, |st, _| {
        st.workspaces.iter().map(|w| w.id.clone()).collect()
    });
    for id in loops {
        assert!(!s.has(&format!("on:{id}")), "{id} highlighted");
    }
}

#[gpui::test]
fn project_settings_toggle_hides_the_project_from_the_sidebar(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_project(&mut s, "storefront");
    s.click("tog:toggle hide from sidebar:false");
    assert!(s.has("tog:toggle hide from sidebar:true"));
    assert!(!s.has("ph:storefront"), "hidden from the sidebar");
    s.click("tog:toggle hide from sidebar:true");
    assert!(s.has("ph:storefront"));
}

/// The script editors take the editor's own keys (Enter, Backspace), not only typed text.
#[gpui::test]
fn project_setup_script_editor_takes_enter_and_backspace(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    open_project(&mut s, "storefront");
    s.click("set-nav:Run");
    let shell = s.root.read_with(s.cx, |r, _| r.shell.clone());
    let project =
        s.cx.update(|_, cx| shell.read(cx).route_view(ViewName::Project))
            .and_then(|v| v.downcast::<crate::project::ProjectView>().ok())
            .expect("the project view");
    let ed = project
        .read_with(s.cx, |p, _| p.setup.clone())
        .expect("the setup editor");
    s.cx.update(|window, cx| ed.update(cx, |e, cx| e.focus(window, cx)));
    s.settle();
    let before = ed.read_with(s.cx, |e, cx| e.value(cx));
    s.type_text("ab");
    s.keys("backspace enter");
    let value = ed.read_with(s.cx, |e, cx| e.value(cx));
    assert_eq!(value, format!("a\n{before}"));
}
