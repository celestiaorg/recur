//! The Settings → Bundles scenarios. The run-command palette and the
//! enabled willSave transform are the Observatory editor's.

use std::sync::Arc;

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_domain::FileContent;
use recur_services::fake::Overrides;

use super::harness::{plain, with};

#[gpui::test]
fn bundles_can_disable_a_will_save_command(cx: &mut TestAppContext) {
    let writes: Arc<Mutex<Vec<String>>> = Default::default();
    let w = writes.clone();
    let opts = Overrides::default()
        .read_file(|_, _| {
            Ok(FileContent {
                path: "src/main.go".into(),
                language: "go".into(),
                content: "package main\n\n\nfunc main() {}   \n".into(),
                size: 40,
                writable: true,
                ..Default::default()
            })
        })
        .write_file(move |_, (_, _, _, content)| {
            w.lock().push(content);
            Ok(())
        });
    let mut s = with(cx, |_| {}, opts);
    s.open_settings();
    s.section("Commands");
    assert!(s.until(|s| s.has("tog:Disable Gofmt on Save:true")));
    s.click("tog:Disable Gofmt on Save:true");
    assert!(s.has("tog:Enable Gofmt on Save:false"));
    assert!(s.saved().disabled_bundle_commands.contains(&"recur.go.gofmt-on-save".into()));

    s.state
        .update(s.cx, |st, cx| st.open_workspace("LOOP-2231", false, cx));
    s.settle();
    s.click("obs-node:file:src/main.go");
    s.click("obs-editor-pane:main");
    s.type_text(" ");
    s.keys("backspace secondary-s");
    assert!(s.until(|_| !writes.lock().is_empty()));
    let first = writes.lock()[0].clone();
    assert!(
        regex::Regex::new(r"[ \t]+\n").unwrap().is_match(&first),
        "trailing spaces stay: {first:?}"
    );
    assert!(first.contains("\n\n\n"), "stacked blanks stay: {first:?}");
}

#[gpui::test]
fn bundles_is_a_nav_group_with_a_page_per_contribution_kind(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    for label in [
        "Commands",
        "Prompt actions",
        "Grammars",
        "Themes",
        "Snippets",
    ] {
        assert!(s.has(&format!("set-nav:{label}")), "{label}");
    }
    for (section, tog) in [
        ("Prompt actions", "tog:Disable /commit:true"),
        ("Grammars", "tog:Disable go:true"),
        ("Themes", "tog:Disable Charcoal:true"),
        ("Snippets", "tog:Disable If Err:true"),
    ] {
        s.section(section);
        assert!(s.until(|s| s.has(tog)), "{section}: {tog}");
    }
}

#[gpui::test]
fn bundles_can_disable_a_prompt_action(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Prompt actions");
    assert!(s.until(|s| s.has("tog:Disable /commit:true")));
    s.click("tog:Disable /commit:true");
    assert!(s.has("tog:Enable /commit:false"));
    assert!(s.saved().disabled_bundle_prompts.contains(&"recur.chat.commit".into()));
}

#[gpui::test]
fn bundles_can_disable_a_grammar_and_a_theme(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Grammars");
    assert!(s.until(|s| s.has("tog:Disable go:true")));
    s.click("tog:Disable go:true");
    assert!(s.has("tog:Enable go:false"));
    assert!(s.saved().disabled_bundle_grammars.contains(&"recur.languages.go".into()));
    s.section("Themes");
    assert!(s.until(|s| s.has("tog:Disable Cool Glow:true")));
    s.click("tog:Disable Cool Glow:true");
    assert!(s.has("tog:Enable Cool Glow:false"));
    assert!(s.saved().disabled_bundle_themes.contains(&"cool-glow".into()));
}

#[gpui::test]
fn bundles_can_disable_a_snippet(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Snippets");
    assert!(s.until(|s| s.has("tog:Disable If Err:true")));
    s.click("tog:Disable If Err:true");
    assert!(s.has("tog:Enable If Err:false"));
    assert!(s.saved().disabled_bundle_snippets.contains(&"recur.go.ife".into()));
}
