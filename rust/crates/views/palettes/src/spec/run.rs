//! `terminal-dock.spec.ts`'s ⌘R / ⌘⇧R palette scenarios: the run picker opens focused on the
//! default, filters as you type, runs with Enter or 1–9, and the Stop palette follows once
//! several commands run.

use gpui::TestAppContext;
use recur_services::mock::MockOptions;

use super::harness::*;

const WS: &str = "LOOP-2231";

fn mount<'a>(cx: &'a mut TestAppContext, t: &Calls, runs: &[(&str, &str)]) -> S<'a> {
    let mut s = boot(cx, with_runs(runs), t.terminals(MockOptions::default()));
    s.open_workspace(WS);
    s
}

fn run_open(s: &mut S) -> bool {
    s.read(|s| s.overlays.run_menu)
}

fn running(s: &mut S) -> Vec<String> {
    s.read(|s| s.run.running_runs(WS).into_iter().map(|(n, _)| n).collect())
}

const FIVE: [(&str, &str); 5] = [
    ("dev", "wails3 task dev"),
    ("test:go", "wails3 task test:go"),
    ("test:frontend", "wails3 task test:frontend"),
    ("check", "wails3 task check"),
    ("tidy", "wails3 task common:go:mod:tidy"),
];
const NAMES: [&str; 5] = ["dev", "test:go", "test:frontend", "check", "tidy"];

#[gpui::test]
fn cmd_r_opens_the_run_palette_and_enter_runs_the_selected_command(cx: &mut TestAppContext) {
    let t = Calls::default();
    let mut s = mount(cx, &t, &[("dev", "pnpm dev --port $RECUR_PORT")]);
    s.keys("cmd-r");
    assert!(s.has("run-palette"));
    assert!(s.input_focused());
    assert_eq!(s.rows("run-row:", &["dev"]), ["dev:sel"]);
    s.keys("enter");
    assert!(!run_open(&mut s));
    assert!(!s.has("run-palette"));
    assert!(s.until(|s| running(s) == ["dev"]));
    assert!(s.until(|s| s.has("term-dock")));
    assert!(s.until(|_| !t.named("WriteTerminal").is_empty()));
}

#[gpui::test]
fn the_run_palette_filters_to_matching_commands_as_you_type(cx: &mut TestAppContext) {
    let t = Calls::default();
    let mut s = mount(cx, &t, &FIVE);
    s.keys("cmd-r");
    assert!(s.input_focused());
    assert_eq!(s.rows("run-row:", &NAMES).len(), 5);
    s.type_text("tid");
    assert_eq!(s.rows("run-row:", &NAMES), ["tidy:sel"]);
    s.keys("escape");
    s.keys("cmd-r");
    assert!(s.input_focused());
    assert_eq!(s.rows("run-row:", &NAMES).len(), 5);
    s.keys("down down down");
    assert_eq!(s.rows("run-row:", &NAMES)[3], "check:sel");
    // A new query starts over at its first match.
    s.type_text("test");
    assert_eq!(s.rows("run-row:", &NAMES), ["test:go:sel", "test:frontend"]);
}

#[gpui::test]
fn the_run_palette_takes_digits_and_escape_like_open_in(cx: &mut TestAppContext) {
    let t = Calls::default();
    let mut s = mount(cx, &t, &[("dev", "pnpm dev --port $RECUR_PORT")]);
    s.keys("cmd-r");
    assert!(s.input_focused());
    s.keys("escape");
    assert!(!s.has("run-palette"));
    s.keys("cmd-r");
    assert!(s.input_focused());
    s.keys("1");
    assert!(!s.has("run-palette"));
    assert!(s.until(|s| running(s) == ["dev"]));
}

#[gpui::test]
fn cmd_shift_r_opens_the_stop_palette_when_several_commands_are_running(cx: &mut TestAppContext) {
    let t = Calls::default();
    let mut s = mount(
        cx,
        &t,
        &[
            ("dev", "pnpm dev --port $RECUR_PORT"),
            ("test", "go test ./..."),
        ],
    );
    s.keys("cmd-r");
    assert!(s.has("run-palette"));
    assert!(s.input_focused());
    s.keys("1");
    assert!(s.until(|s| running(s) == ["dev"]));
    s.keys("cmd-r");
    assert!(s.has("run-palette"));
    assert!(s.input_focused());
    s.keys("2");
    assert!(s.until(|s| running(s).len() == 2));
    s.keys("cmd-shift-r");
    assert!(s.has("stop-dev"));
    assert!(s.input_focused());
    assert!(s.has("stop-dev") && s.has("stop-test") && s.has("stop-__all__"));
    s.keys("escape");
    assert!(!s.has("stop-dev"));
    s.keys("cmd-shift-r");
    assert!(s.has("stop-dev"));
    assert!(s.input_focused());
    s.keys("1");
    assert!(!s.has("stop-dev"));
    assert!(s.until(|_| !t.named("InterruptTerminal").is_empty()));
}

#[gpui::test]
fn cmd_enter_and_the_pin_make_a_run_the_project_default(cx: &mut TestAppContext) {
    let t = Calls::default();
    let mut s = mount(cx, &t, &FIVE);
    s.keys("cmd-r");
    s.keys("down down");
    s.keys("cmd-enter");
    assert!(s.until(
        |s| s.rows("run-row:", &NAMES).first().map(String::as_str) == Some("test:frontend:sel")
    ));
    assert!(s.has("run-pin:test:frontend"));
    let pinned = s.read(|s| {
        s.projects
            .iter()
            .find(|p| p.id == "storefront")
            .map(|p| p.pinned_run.clone())
    });
    assert_eq!(pinned.as_deref(), Some("test:frontend"));
    s.hover("run-row:tidy");
    s.click("run-pin:tidy");
    assert!(
        s.until(|s| s.rows("run-row:", &NAMES).first().map(String::as_str) == Some("tidy:sel"))
    );
}
