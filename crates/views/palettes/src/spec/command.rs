//! Go to's rows running their targets: routes, actions, workspace
//! actions on the detail route, and bundle `[[search]]` hits after the 200ms debounce.

use gpui::TestAppContext;
use recur_services::api::{
    PaletteSearchDto, PaletteSearchGroupDto, PromptCompleteHitDto, PromptResultDto,
};
use recur_services::fake::Overrides;
use recur_view_core::ViewName;

use super::harness::*;

const WS: &str = "LOOP-2231";

fn pick(s: &mut S, label: &str) {
    s.keys("secondary-k");
    s.type_text(label);
    let sel = format!("pal-row:{label}:sel");
    assert!(s.has(&sel), "{sel}");
    s.keys("enter");
    assert!(!s.read(|s| s.overlays.palette));
}

#[gpui::test]
fn routes_and_actions_run_from_the_palette(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.keys("secondary-k");
    s.type_text("Pull requests");
    // The route row leads its group.
    s.keys("enter");
    assert_eq!(s.view(), ViewName::Pr);
    pick(&mut s, "Schedules");
    assert_eq!(s.view(), ViewName::Schedules);
    pick(&mut s, "Get started");
    assert_eq!(s.view(), ViewName::Welcome);
    pick(&mut s, "Settings · policy");
    assert_eq!(s.view(), ViewName::Settings);
    pick(&mut s, "New schedule");
    assert!(s.has("sc-modal:New schedule"));
    s.keys("escape");
    pick(&mut s, "New project");
    assert!(s.read(|s| s.overlays.new_project));
    s.state.update(s.cx, |st, cx| st.close_new_project(cx));
    pick(&mut s, "Keyboard shortcuts");
    assert!(s.read(|s| s.overlays.shortcuts));
    s.keys("escape");
    pick(&mut s, "Run command / snippets…");
    assert!(s.read(|s| s.overlays.commands && !s.overlays.shortcuts));
    s.keys("escape");
    pick(&mut s, "New workspace");
    assert!(s.read(|s| s.overlays.new_task));
}

#[gpui::test]
fn workspace_project_and_pr_rows_open_them(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    let (goal, pr_title, pr_id) = s.read(|st| {
        let w = st.workspaces.iter().find(|w| w.id == WS).expect("seed");
        let p = st.prs.iter().find(|p| p.id == "479").expect("seed pr");
        (w.goal.clone(), p.title.clone(), p.id.clone())
    });
    pick(&mut s, &goal);
    assert_eq!(s.view(), ViewName::Detail);
    assert_eq!(s.read(|s| s.selected_workspace_id.clone()), WS);
    pick(&mut s, &pr_title);
    assert_eq!(s.view(), ViewName::PrDetail);
    assert_eq!(s.read(|s| s.selected_pr_id.clone()), pr_id);
    pick(&mut s, "marketing-site");
    assert_eq!(s.view(), ViewName::Project);
}

#[gpui::test]
// Linux: the run's terminal takes focus and keeps Ctrl+K, as xterm.js does.
#[cfg_attr(not(target_os = "macos"), ignore)]
fn the_detail_route_adds_open_in_copy_path_and_the_run_actions(cx: &mut TestAppContext) {
    let t = Calls::default();
    let mut s = boot(
        cx,
        with_runs(&[("dev", "pnpm dev"), ("test", "go test ./...")]),
        t.terminals(Overrides::default()),
    );
    s.keys("secondary-k");
    assert!(!s.has("pal-row:Open in…") && !s.has("pal-row:Run…"));
    s.keys("escape");
    s.open_workspace(WS);
    s.keys("secondary-k");
    assert!(s.has("pal-row:Open in…") && s.has("pal-row:Copy path"));
    assert!(s.has("pal-row:Run…") && !s.has("pal-row:Stop run…"));
    s.keys("escape");
    pick(&mut s, "Open in…");
    assert!(s.read(|s| s.overlays.open_in));
    s.keys("escape");
    pick(&mut s, "Run…");
    assert!(s.read(|s| s.overlays.run_menu));
    s.keys("1");
    assert!(s.until(|s| s.read(|s| s.run.is_any_run_running(WS))));
    s.keys("secondary-k");
    assert!(s.has("pal-row:Stop run…"));
    s.keys("escape");
    pick(&mut s, "Copy path");
    assert!(s.until(|s| s.toasted(recur_ui::widgets::ToastKind::Info, "Copied path")));
}

fn search_opts(calls: &Calls) -> Overrides {
    let (search, expand) = (calls.clone(), calls.clone());
    let hit = |insert: &str, label: &str, psub: &str, action: &str| PromptCompleteHitDto {
        insert: insert.into(),
        label: label.into(),
        psub: psub.into(),
        action: action.into(),
        ..Default::default()
    };
    let group = |name: &str, hits| PaletteSearchGroupDto {
        name: name.into(),
        hits: Some(hits),
        ..Default::default()
    };
    Overrides::default()
        .search_palette(move |_, q| {
            search.push("search_palette", &[&q]);
            Ok(PaletteSearchDto {
                groups: Some(vec![
                    group(
                        "Linear",
                        vec![
                            hit("ENG-412", "ENG-412 Rate-limit the public API", "In Progress", ""),
                            hit("", "Linear settings", "", "settings:integrations"),
                        ],
                    ),
                    group("Empty", Vec::new()),
                ]),
            })
        })
        .expand_goal(move |_, text| {
            expand.push("expand_goal", &[&text]);
            Ok(PromptResultDto {
                prompt: format!("Work on {text}"),
                display: "Linear ENG-412".into(),
                ..Default::default()
            })
        })
}

#[gpui::test]
fn bundle_search_hits_appear_after_the_debounce_and_expand_into_a_new_workspace(
    cx: &mut TestAppContext,
) {
    let calls = Calls::default();
    let mut s = boot(cx, |_| {}, search_opts(&calls));
    s.keys("secondary-k");
    s.type_text("e");
    s.wait(300);
    // One character never searches.
    assert!(calls.named("search_palette").is_empty());
    s.type_text("ng");
    s.wait(100);
    assert!(calls.named("search_palette").is_empty());
    s.wait(150);
    assert_eq!(calls.named("search_palette"), [vec!["eng"]]);
    assert!(s.has("pal-group:Linear") && !s.has("pal-group:Empty"));
    let hit = "pal-row:ENG-412 Rate-limit the public API";
    assert!(s.has(hit) || s.has(&format!("{hit}:sel")));
    s.click_row(hit);
    assert!(s.until(|s| s.read(|s| s.overlays.new_task)));
    assert_eq!(calls.named("expand_goal"), [vec!["ENG-412"]]);
    // The expanded prompt is the modal's task.
    assert!(s.until(|s| s.has("nt-go:on")));
}

#[gpui::test]
fn a_settings_search_action_opens_that_settings_pane(cx: &mut TestAppContext) {
    let calls = Calls::default();
    let mut s = boot(cx, |_| {}, search_opts(&calls));
    s.keys("secondary-k");
    s.type_text("linear set");
    s.wait(300);
    let row = "pal-row:Linear settings";
    assert!(s.has(row) || s.has(&format!("{row}:sel")));
    s.click_row(row);
    assert_eq!(s.view(), ViewName::Settings);
    assert_eq!(s.read(|s| s.settings_section.clone()), "integrations");
}
