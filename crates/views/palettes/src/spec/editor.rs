//! The palettes over the Observatory's editor: the ⌘⇧P Sort Lines run, syntax changes
//! through the language picker's rows, and Run Command's Recent
//! section in frecency order.

use std::rc::Rc;

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply};
use recur_view_core::commands::{MacroContext, MacroResult};
use serde_json::{Value, json};

use super::harness::*;

const WS: &str = "LOOP-2231";
const GO: &str = "package main\n\nzed\nalpha\nbeta\n";

/// The open file and recorded writes (`ReadFile` / `WriteFile` resolvers).
fn file_opts(calls: &Calls) -> MockOptions {
    let opts = MockOptions::default().resolver("ReadFile", |_, _, _| {
        Reply::value(json!({
            "path": "src/main.go", "language": "go", "content": GO, "size": GO.len(),
            "writable": true,
        }))
    });
    calls.record(opts, "WriteFile", Value::Null)
}

/// A macro runner that sorts its input's lines, standing in for the TypeScript worker.
fn sort_lines_runner(s: &mut S) {
    s.state.update(s.cx, |st, _| {
        st.set_macro_runner(Some(Rc::new(|_module, ctx: MacroContext, _| {
            let mut lines: Vec<&str> = ctx.input.lines().collect();
            lines.sort();
            let mut text = lines.join("\n");
            if ctx.input.ends_with('\n') {
                text.push('\n');
            }
            Box::pin(async move {
                Ok(MacroResult {
                    text,
                    ..Default::default()
                })
            })
        })));
    });
}

fn open_main_go(s: &mut S) {
    s.open_workspace(WS);
    assert!(s.until(|s| s.has("obs-node:file:src/main.go")));
    s.click("obs-node:file:src/main.go");
    s.wait(100);
    if s.has("view-mode:Code") {
        s.click("view-mode:Code");
    }
}

#[gpui::test]
fn run_command_palette_runs_sort_lines_ascending_on_the_open_buffer(cx: &mut TestAppContext) {
    let calls = Calls::default();
    let mut s = boot(cx, |_| {}, file_opts(&calls));
    sort_lines_runner(&mut s);
    open_main_go(&mut s);
    s.keys("secondary-shift-p");
    assert!(s.has("cmd-palette"));
    assert!(s.input_focused());
    s.type_text("Sort Lines Ascending");
    assert!(s.has("rc-row:Sort Lines Ascending:sel"));
    s.keys("enter");
    assert!(!s.has("cmd-palette"));
    s.wait(300);
    // The replacement stays selected and the editor has focus again: Save works without a
    // click.
    s.keys("secondary-s");
    assert!(s.until(|_| !calls.named("WriteFile").is_empty()));
    let written = calls
        .named("WriteFile")
        .last()
        .and_then(|a| a.get(3).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    let order: Vec<&str> = written
        .lines()
        .filter(|l| matches!(*l, "alpha" | "beta" | "zed"))
        .collect();
    assert_eq!(order, ["alpha", "beta", "zed"]);
    // The run counts for Recent.
    let usage = s.storage("recur.commandUsage").unwrap_or_default();
    assert!(usage.contains("recur.text.sort-lines-asc"), "{usage}");
    s.keys("secondary-shift-p");
    assert!(s.has("rc-group:Recent"));
    assert_eq!(
        s.rows("rc-row:", &["Sort Lines Ascending", "Uppercase"])
            .first()
            .map(String::as_str),
        Some("Sort Lines Ascending:sel")
    );
}

fn language(s: &mut S) -> String {
    s.state.read_with(s.cx, |_, cx| {
        recur_view_core::editor_env::editor_env_peek(cx)
            .map(|e| e.read(cx).language_label("src/main.go"))
            .unwrap_or_default()
    })
}

#[gpui::test]
fn the_status_bar_language_picker_remembers_the_pick_for_this_file(cx: &mut TestAppContext) {
    let calls = Calls::default();
    let mut s = boot(cx, |_| {}, file_opts(&calls));
    open_main_go(&mut s);
    assert!(language(&mut s).to_lowercase().contains("go"));
    s.click("status:lang");
    assert!(s.has("syntax-palette"));
    assert!(s.input_focused());
    s.type_text("plain");
    s.click_row("syn-row:Plain Text");
    assert!(!s.has("syntax-palette"));
    assert_eq!(language(&mut s), "");
    s.click("status:lang");
    s.type_text("auto detect");
    s.click("syn-row:Auto Detect:sel");
    assert!(language(&mut s).to_lowercase().contains("go"));
}

#[gpui::test]
fn always_open_as_sets_a_file_type_association_and_escape_steps_back(cx: &mut TestAppContext) {
    let calls = Calls::default();
    let mut s = boot(cx, |_| {}, file_opts(&calls));
    open_main_go(&mut s);
    s.click("status:lang");
    assert!(s.has("syn-row:Auto Detect:sel"));
    s.click("syn-row:Always open *.go as…");
    // Association mode lists Plain Text and the languages under the file type.
    assert!(s.has("syn-group:*.go"));
    assert!(!s.has("syn-row:Auto Detect") && !s.has("syn-row:Auto Detect:sel"));
    s.keys("escape");
    assert!(s.has("syn-row:Auto Detect:sel"));
    s.click("syn-row:Always open *.go as…");
    s.keys("enter");
    assert!(!s.has("syntax-palette"));
    // *.go now opens as Plain Text, and the picker offers to clear it.
    assert_eq!(language(&mut s), "");
    s.click("status:lang");
    assert!(s.has("syn-row:Clear *.go association"));
    s.click("syn-row:Clear *.go association");
    assert!(language(&mut s).to_lowercase().contains("go"));
}

#[gpui::test]
fn run_command_palette_inserts_a_snippet_with_its_tab_trigger_hint(cx: &mut TestAppContext) {
    let calls = Calls::default();
    let mut s = boot(cx, |_| {}, file_opts(&calls));
    open_main_go(&mut s);
    s.keys("secondary-shift-p");
    assert!(s.until(|s| s.has("rc-group:Go")));
    s.type_text("ife");
    assert!(s.has("rc-row:If Err:sel"));
    s.keys("enter");
    s.wait(300);
    s.keys("secondary-s");
    assert!(s.until(|_| !calls.named("WriteFile").is_empty()));
    let written = calls
        .named("WriteFile")
        .last()
        .and_then(|a| a.get(3).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    assert!(written.contains("if err != nil {"), "{written}");
    assert!(
        s.storage("recur.commandUsage")
            .unwrap_or_default()
            .contains("recur.go.ife")
    );
}
