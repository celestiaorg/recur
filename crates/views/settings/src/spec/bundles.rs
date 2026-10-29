//! The Settings → Bundles scenarios. The run-command palette and the
//! enabled willSave transform are the Observatory editor's.

use std::sync::Arc;

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Reply};
use serde_json::{Value, json};

use super::harness::{S, plain, with};

fn disabled(s: &mut S, key: &str) -> Vec<String> {
    s.saved()[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

#[gpui::test]
fn bundles_can_disable_a_will_save_command(cx: &mut TestAppContext) {
    let writes: Arc<Mutex<Vec<String>>> = Default::default();
    let w = writes.clone();
    let opts = MockOptions::default()
        .resolver("ReadFile", |_, _, _| {
            Reply::value(json!({
                "path": "src/main.go", "language": "go",
                "content": "package main\n\n\nfunc main() {}   \n", "size": 40, "writable": true,
            }))
        })
        .resolver("WriteFile", move |a, _, _| {
            w.lock().push(
                a.get(3)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
            Reply::null()
        });
    let mut s = with(cx, |_| {}, opts);
    s.open_settings();
    s.section("Commands");
    assert!(s.until(|s| s.has("tog:Disable Gofmt on Save:true")));
    s.click("tog:Disable Gofmt on Save:true");
    assert!(s.has("tog:Enable Gofmt on Save:false"));
    assert!(disabled(&mut s, "disabledBundleCommands").contains(&"recur.go.gofmt-on-save".into()));

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
    assert!(disabled(&mut s, "disabledBundlePrompts").contains(&"recur.chat.commit".into()));
}

#[gpui::test]
fn bundles_can_disable_a_grammar_and_a_theme(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Grammars");
    assert!(s.until(|s| s.has("tog:Disable go:true")));
    s.click("tog:Disable go:true");
    assert!(s.has("tog:Enable go:false"));
    assert!(disabled(&mut s, "disabledBundleGrammars").contains(&"recur.languages.go".into()));
    s.section("Themes");
    assert!(s.until(|s| s.has("tog:Disable Cool Glow:true")));
    s.click("tog:Disable Cool Glow:true");
    assert!(s.has("tog:Enable Cool Glow:false"));
    assert!(disabled(&mut s, "disabledBundleThemes").contains(&"cool-glow".into()));
}

#[gpui::test]
fn bundles_can_disable_a_snippet(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Snippets");
    assert!(s.until(|s| s.has("tog:Disable If Err:true")));
    s.click("tog:Disable If Err:true");
    assert!(s.has("tog:Enable If Err:false"));
    assert!(disabled(&mut s, "disabledBundleSnippets").contains(&"recur.go.ife".into()));
}
