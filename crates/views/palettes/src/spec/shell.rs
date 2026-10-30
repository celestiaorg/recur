//! The shell's palette scenarios: Go to (⌘K), the shortcuts list (⌘⇧/) and Open in (⌘O,
//! the titlebar button).

use gpui::TestAppContext;
use recur_ui::widgets::ToastKind;
use recur_view_core::ViewName;

use super::harness::*;

const ACTIVE_ID: &str = "LOOP-2231";

fn row(s: &mut S, prefix: &str, label: &str) -> bool {
    s.has(&format!("{prefix}{label}")) || s.has(&format!("{prefix}{label}:sel"))
}

fn palette_open(s: &mut S) -> bool {
    s.read(|s| s.overlays.palette)
}

#[gpui::test]
fn go_to_palette_opens_on_cmd_k_and_searches(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    // The titlebar has no search field: ⌘K and Navigate → Go to… open the palette.
    assert!(!s.has("tb-search"));
    s.keys("secondary-k");
    assert!(s.has("palette"));
    assert!(s.input_focused());
    s.type_text("pull");
    assert!(row(&mut s, "pal-row:", "Pull requests"));
    s.keys("escape");
    assert!(!palette_open(&mut s));
    assert!(!s.has("palette"));
}

#[gpui::test]
fn go_to_palette_scrolls_selection_into_view_with_arrow_keys(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.keys("secondary-k");
    let labels = s.read(|st| {
        let mut l: Vec<String> = [
            "New workspace",
            "New project",
            "New schedule",
            "New blank workspace",
            "Open file or folder",
            "Run command / snippets…",
            "Keyboard shortcuts",
            "Inbox",
            "Issues",
            "Pull requests",
            "Schedules",
            "Settings · policy",
            "Get started",
        ]
        .map(String::from)
        .to_vec();
        l.extend(
            st.workspaces
                .iter()
                .filter(|w| !w.archived && w.host_pr_id.is_empty())
                .map(|w| w.goal.clone()),
        );
        l.extend(st.prs.iter().map(|p| p.title.clone()));
        l.extend(st.projects.iter().map(|p| p.name.clone()));
        l
    });
    let count = labels.len();
    assert!(count > 8, "{count}");
    let list = s.bounds("pal-list").expect("list");
    let last = labels.last().cloned().unwrap_or_default();
    let last_row = s.bounds(&format!("pal-row:{last}")).expect("last row");
    assert!(
        last_row.bottom() > list.bottom() + gpui::px(1.),
        "the list overflows"
    );
    for _ in 0..count - 1 {
        s.keys("down");
    }
    let sel = s
        .bounds(&format!("pal-row:{last}:sel"))
        .expect("selected row");
    let list = s.bounds("pal-list").expect("list");
    assert!(sel.top() >= list.top() - gpui::px(1.) && sel.bottom() <= list.bottom() + gpui::px(1.));
    let first = s.bounds("pal-row:New workspace").expect("first row");
    assert!(first.top() < list.top(), "scrolled down");
}

#[gpui::test]
fn keyboard_shortcuts_opens_on_cmd_shift_slash_and_filters(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.keys("secondary-shift-/");
    assert!(s.has("shortcuts-panel"));
    assert!(s.has("sc-row:Go to (workspaces / PRs)"));
    assert!(s.has("sc-row:Run command / snippets"));
    // Bound bundle commands appear under their category.
    assert!(s.until(|s| s.has("sc-row:Uppercase")));
    s.type_text("terminal");
    assert!(s.has("sc-row:Toggle terminal"));
    assert!(!s.has("sc-row:Uppercase"));
    s.keys("escape");
    assert!(!s.has("shortcuts-panel"));
    assert!(!s.read(|s| s.overlays.shortcuts));
}

#[gpui::test]
fn go_to_palette_matches_a_loop_by_project_id_and_a_pr_by_number(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    let (loop_goal, project, pr_title, pr_number) = s.read(|st| {
        let l = &st
            .workspaces
            .iter()
            .find(|w| w.id == "LOOP-2218")
            .expect("seed loop");
        let p = st.prs.iter().find(|p| p.id == "482").expect("seed pr");
        (
            l.goal.clone(),
            l.project_id.clone(),
            p.title.clone(),
            p.number,
        )
    });
    s.keys("secondary-k");
    s.type_text(&project);
    assert!(row(&mut s, "pal-row:", &loop_goal));
    s.fill(&format!("#{pr_number}"));
    assert!(row(&mut s, "pal-row:", &pr_title));
}

#[gpui::test]
fn open_in_palette_opens_from_the_titlebar_and_cmd_o_on_a_workspace(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    // Off a workspace ⌘O does nothing.
    s.keys("secondary-o");
    assert!(!s.has("open-in-palette"));
    s.open_workspace(ACTIVE_ID);
    assert_eq!(s.view(), ViewName::Detail);
    assert!(s.has("tb-open-in"));
    s.click("tb-open-in");
    assert!(s.has("open-in-palette"));
    assert!(s.until(|s| row(s, "oi-row:", "finder")));
    assert!(row(&mut s, "oi-row:", "cursor"));
    assert!(row(&mut s, "oi-row:", "copy-path"));
    s.keys("escape");
    assert!(!s.has("open-in-palette"));
    s.keys("secondary-o");
    assert!(s.has("open-in-palette"));
    assert!(s.until(|s| row(s, "oi-row:", "finder")));
    s.click_row("oi-row:finder");
    assert!(!s.has("open-in-palette"));
    s.click("tb-copy-path");
    assert!(s.until(|s| s.toasted(ToastKind::Info, "Copied path")));
}

#[gpui::test]
fn open_in_palette_pins_a_default_app_to_the_top_of_the_list(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_workspace(ACTIVE_ID);
    s.click("tb-open-in");
    let ids = ["finder", "cursor", "terminal", "copy-path"];
    assert!(s.until(|s| s.rows("oi-row:", &ids).len() == 4));
    assert_eq!(s.rows("oi-row:", &ids)[0], "finder:sel");
    s.hover("oi-row:cursor");
    s.click("oi-pin:cursor");
    assert_eq!(
        s.rows("oi-row:", &ids),
        ["cursor:sel", "finder", "terminal", "copy-path"]
    );
    assert_eq!(s.storage("recur.openInDefault").as_deref(), Some("cursor"));
    s.keys("escape");
    s.keys("secondary-o");
    assert!(s.until(|s| s.rows("oi-row:", &ids).len() == 4));
    assert_eq!(s.rows("oi-row:", &ids)[0], "cursor:sel");
}

#[gpui::test]
fn open_in_keys_pick_digits_and_cmd_enter_makes_the_default(cx: &mut TestAppContext) {
    let calls = Calls::default();
    let c = calls.clone();
    let opts = recur_services::fake::Overrides::default().open_workspace_in(move |_, (ws, target)| {
        c.push("open_workspace_in", &[&ws, &target]);
        Ok(())
    });
    let mut s = boot(cx, |_| {}, opts);
    s.open_workspace(ACTIVE_ID);
    s.keys("secondary-o");
    let ids = ["finder", "cursor", "terminal", "copy-path"];
    assert!(s.until(|s| s.rows("oi-row:", &ids).len() == 4));
    s.keys("down down");
    s.keys("secondary-enter");
    assert_eq!(
        s.storage("recur.openInDefault").as_deref(),
        Some("terminal")
    );
    assert_eq!(s.rows("oi-row:", &ids)[0], "terminal:sel");
    // 2 opens the second row.
    s.keys("2");
    assert!(!s.has("open-in-palette"));
    let opened = calls.named("open_workspace_in");
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0][0], ACTIVE_ID);
    assert_eq!(opened[0][1], "finder");
}
