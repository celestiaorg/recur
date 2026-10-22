//! `editor-syntax.spec.ts`: the status bar's language button opens the language picker, and
//! the per-file pick it applies relabels the file and survives a restart through the editor
//! session. The picker's own list is `SyntaxPalette` (the palettes' port); its pick goes
//! through [`crate::textmate::EditorEnv::apply_language_mode`] as here.

use gpui::TestAppContext;
use recur_services::mock::MockOptions;

use super::harness::*;

fn label(h: &mut H) -> String {
    h.read(|o, cx| o.status_language(cx))
}

fn pick(h: &mut H, scope: Option<&'static str>) {
    h.update(move |_, _, cx| {
        if let Some(env) = crate::textmate::editor_env_peek(cx) {
            env.update(cx, |e, cx| e.apply_language_mode("src/main.go", scope, cx));
        }
    });
}

fn open_main_go(h: &mut H) {
    h.click("obs-node:file:src/main.go");
    h.wait(50);
    h.click("view-mode:Code");
}

#[gpui::test]
async fn the_status_bar_language_is_a_picker_and_the_pick_is_remembered(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    open_main_go(&mut h);
    assert!(label(&mut h).to_lowercase().contains("go"));
    h.click("status:lang");
    assert!(h.state.read_with(h.cx, |s, _| s.overlays.syntax_mode));
    pick(&mut h, Some(recur_textmate::SYNTAX_PLAIN));
    assert_eq!(label(&mut h), "Plain Text");
    let scope = h.read(|o, cx| o.main_editor.as_ref().map(|e| e.read(cx).context().scope));
    assert_eq!(scope.as_deref(), Some(""));
    // Auto Detect.
    pick(&mut h, None);
    assert!(label(&mut h).to_lowercase().contains("go"));
}

#[gpui::test]
async fn the_language_pick_survives_a_restart_via_the_editor_session(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let store = SessionStore::default();
    let mut h = mount(cx, store.install(MockOptions::default()), |_| {}, |_| {});
    open_main_go(&mut h);
    pick(&mut h, Some(recur_textmate::SYNTAX_PLAIN));
    assert_eq!(label(&mut h), "Plain Text");
    h.wait(600);
    h.quit();
    let mut h = mount(
        restarted,
        store.install(MockOptions::default()),
        |_| {},
        |_| {},
    );
    h.wait(200);
    assert_eq!(label(&mut h), "Plain Text");
}
