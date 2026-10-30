//! The New workspace modal and add-project scenarios.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::TestAppContext;
use recur_domain::{BranchChoice, Project, Workspace};
use recur_services::Result;
use recur_services::api::{FlagDimension, FlagOption, ModelChoice, ModelGroup};
use recur_services::fake::{Overrides, Release, hold};
use recur_view_core::ViewName;

use super::harness::{S, plain, with};
use crate::new_task::NewTaskModal;

fn dim(id: &str, label: &str, options: &[(&str, &str, bool)]) -> FlagDimension {
    FlagDimension {
        id: id.into(),
        label: label.into(),
        options: options
            .iter()
            .map(|(value, label, default)| FlagOption {
                value: (*value).into(),
                label: (*label).into(),
                default: *default,
            })
            .collect(),
    }
}

impl S<'_> {
    /// The non-main, live rows the rail shows (`.rail .tloop:not(.tmain)`).
    fn rail_rows(&mut self) -> usize {
        self.state.read_with(self.cx, |s, _| {
            s.workspaces
                .iter()
                .filter(|w| !w.archived && !w.is_main && w.host_pr_id.trim().is_empty())
                .count()
        })
    }

    fn open_new_task(&mut self) {
        self.keys("secondary-n");
        assert!(self.has("v-newtask"), "New workspace open");
    }

    fn modal(&mut self) -> gpui::Entity<NewTaskModal> {
        self.cx
            .update(|_, cx| NewTaskModal::current(cx))
            .expect("modal")
    }

    /// The model picker's `sel:Model=…` handle.
    fn model_handle(&mut self) -> String {
        let m = self.modal();
        let (model, models) = self.cx.update(|_, cx| {
            let m = m.read(cx);
            (m.model().0.to_string(), cx.global_or_none_models())
        });
        let label = models
            .iter()
            .find(|c| c.r#ref == model)
            .map(|c| c.label.clone())
            .unwrap_or(model);
        format!("sel:Model={label}")
    }

    fn flags_handle(&mut self) -> String {
        let m = self.modal();
        let (model, flags) = self.cx.update(|_, cx| {
            let m = m.read(cx);
            (m.model().0.to_string(), m.model().1.clone())
        });
        let models = self.cx.update(|_, cx| cx.global_or_none_models());
        let dims = recur_view_core::models::flags_for_model(&models, &model);
        format!(
            "sel:Model flags={}",
            recur_view_chat::pickers::flags_summary(&dims, &flags)
        )
    }
}

trait Catalog {
    fn global_or_none_models(&self) -> Vec<recur_view_core::modes::CatalogModel>;
}

impl Catalog for gpui::App {
    fn global_or_none_models(&self) -> Vec<recur_view_core::modes::CatalogModel> {
        recur_view_core::AppState::try_global(self)
            .map(|s| s.read(self).peek_model_catalog().to_vec())
            .unwrap_or_default()
    }
}

#[gpui::test]
fn new_loop_modal_has_repository_and_base_branch_pickers(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_new_task();
    s.click("sel:Mode=Agent");
    s.click("sel-opt:Loop");
    assert!(s.has("sel:Mode=Loop"));
    s.click("sel:Repository=storefront");
    for p in ["storefront", "payments-api", "marketing-site"] {
        assert!(s.has(&format!("sel-opt:{p}")), "{p}");
    }
    s.keys("escape");
    assert!(s.has("v-newtask"), "Escape closes the menu first");
    s.click("sel:Base branch=main");
    s.type_text("auth");
    assert!(s.has("sel-opt:feature/auth"));
    assert!(!s.has("sel-opt:main"));
    s.click("sel-opt:feature/auth");
    assert!(s.has("sel:Base branch=feature/auth"));
    assert!(s.has("sel:Loop=recur.loops.pr") || s.has("sel:Loop=PR"));
    s.click("sel:Policy=Balanced");
    for p in ["Cautious", "Balanced", "Aggressive"] {
        assert!(s.has(&format!("sel-opt:{p}")), "{p}");
    }
    s.keys("escape");
    assert!(s.has("nt-go:off"), "a loop needs a task");
    assert!(s.has("nt-simplify:on"));
    s.keys("escape");
    assert!(!s.has("v-newtask"));
}

#[gpui::test]
fn escape_abandons_eager_create_so_a_second_cmd_n_is_one_row(cx: &mut TestAppContext) {
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    let opts = Overrides::default().create_workspace(move |_, (project_id, _, base, ..)| {
        let i = c.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("WS-EAGER-{i}");
        Ok(Workspace {
            branch: format!("recur/{id}"),
            id,
            project_id,
            r#type: "solo".into(),
            status_label: "Creating".into(),
            phase: "creating".into(),
            goal: "New Workspace".into(),
            short: "New Workspace".into(),
            base_branch: base,
            ..Default::default()
        })
    });
    let mut s = with(cx, |_| {}, opts);
    let before = s.rail_rows();
    for _ in 0..2 {
        s.open_new_task();
        assert!(s.until(|s| s.rail_rows() == before + 1));
        s.keys("escape");
        assert!(!s.has("v-newtask"));
        assert!(s.until(|s| s.rail_rows() == before));
    }
    assert_eq!(n.load(Ordering::SeqCst), 2);
}

#[gpui::test]
fn create_workspace_model_menu_stays_inside_the_window(cx: &mut TestAppContext) {
    let effort = dim("effort", "Effort", &[("low", "Low", false), ("high", "High", true)]);
    let models: Vec<ModelChoice> = (0..24)
        .map(|i| ModelChoice {
            model_ref: format!("claude-code/model-{i}"),
            label: format!("Model {:02}", i + 1),
            flags: vec![effort.clone()],
            context_window: 200_000,
            ..Default::default()
        })
        .collect();
    let opts = Overrides::default().list_model_catalog(move |_, ()| {
        Ok(vec![ModelGroup {
            provider: "claude-code".into(),
            label: "Claude Code".into(),
            models: models.clone(),
            ..Default::default()
        }])
    });
    let mut s = with(cx, |_| {}, opts);
    s.open_new_task();
    let h = s.model_handle();
    s.click(&h);
    let menu = s.bounds("sel-menu").expect("menu");
    let vh = f32::from(s.cx.update(|w, _| w.viewport_size().height));
    assert!(f32::from(menu.origin.y) >= 0., "{menu:?}");
    assert!(f32::from(menu.bottom()) <= vh, "{menu:?}");
    assert!(s.has("sel-opt:Model 01"));
    let first = s.bounds("sel-opt:Model 01").expect("first row");
    assert!(first.origin.y >= menu.origin.y - gpui::px(1.));
}

#[gpui::test]
fn model_and_flags_stay_on_one_row_in_create_workspace_and_chat_composer(cx: &mut TestAppContext) {
    let flags = vec![
        dim(
            "effort",
            "Effort",
            &[("low", "Low", false), ("medium", "Medium", true), ("high", "High", false)],
        ),
        dim("context", "Context", &[("272k", "272K", true), ("1m", "1M", false)]),
        dim("thinking", "On", &[("true", "Yes", true), ("false", "No", false)]),
    ];
    let opts = Overrides::default().list_model_catalog(move |_, ()| {
        Ok(vec![ModelGroup {
            provider: "openai".into(),
            label: "OpenAI".into(),
            models: vec![ModelChoice {
                model_ref: "openai/gpt-6-astra".into(),
                label: "GPT-6 Astra".into(),
                flags: flags.clone(),
                context_window: 272_000,
                ..Default::default()
            }],
            ..Default::default()
        }])
    });
    let mut s = with(cx, |_| {}, opts);
    s.open_new_task();
    let (mh, fh) = (s.model_handle(), s.flags_handle());
    let model = s.bounds(&mh).expect("model picker");
    let fl = s.bounds(&fh).expect("flags picker");
    assert!((f32::from(model.origin.y) - f32::from(fl.origin.y)).abs() < 4.);
    s.keys("escape");
    assert!(!s.has("v-newtask"));
}

#[gpui::test]
fn creating_a_workspace_keeps_chat_visible_while_the_editor_is_blocked(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click("foot-help");
    assert_eq!(s.view(), ViewName::Welcome);
    s.open_new_task();
    s.type_text("review the auth flow");
    assert!(s.has("nt-go:on"));
    s.click("nt-go:on");
    assert!(!s.has("v-newtask"));
    assert!(s.until(|s| s.view() == ViewName::Detail));
    assert_eq!(
        s.state
            .read_with(s.cx, |st, _| st.selected_workspace_id.clone()),
        "WS-NEW"
    );
    // A send held for setup is a queued `.chat-row.me`.
    assert!(s.until(|s| s.has("me:review the auth flow") || s.has("queued:review the auth flow")));
    assert!(s.has("wsv-creating"));
}

#[gpui::test]
fn inbox_create_shows_the_opening_prompt_after_opening_the_rail_row(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    assert_eq!(s.view(), ViewName::Briefing);
    s.open_new_task();
    s.type_text("review the auth flow");
    s.click("nt-go:on");
    assert!(!s.has("v-newtask"));
    assert_eq!(s.view(), ViewName::Briefing);
    let seeded = s
        .state
        .read_with(s.cx, |st, _| st.workspace_seeds.get("WS-NEW").cloned());
    assert_eq!(seeded.map(|x| x.text), Some("review the auth flow".into()));
    s.state
        .update(s.cx, |st, cx| st.open_workspace("WS-NEW", false, cx));
    s.settle();
    assert_eq!(s.view(), ViewName::Detail);
    assert!(s.until(|s| s.has("me:review the auth flow") || s.has("queued:review the auth flow")));
}

#[gpui::test]
fn add_project_is_a_create_modal(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.state.update(s.cx, |st, cx| st.open_project("", cx));
    s.settle();
    assert!(s.has("v-newproject"));
    assert_ne!(s.view(), ViewName::Project);
    s.fill("inp:Repository path", "/tmp/quick-app");
    let name = s.cx.update(|_, cx| {
        crate::new_project::NewProjectModal::current(cx)
            .map(|m| m.read(cx).name(cx))
            .unwrap_or_default()
    });
    assert_eq!(name, "quick-app");
    s.click("np-go:on");
    assert!(s.until(|s| !s.has("v-newproject")));
    let added = s.state.read_with(s.cx, |st, _| {
        st.projects.iter().any(|p| p.name == "quick-app")
    });
    assert!(added);
}

#[gpui::test]
fn adding_a_compatible_provider_shows_its_models_in_an_already_loaded_picker(
    cx: &mut TestAppContext,
) {
    let choice = |model_ref: String, label: &str| ModelChoice {
        model_ref,
        label: label.into(),
        ..Default::default()
    };
    let opts = Overrides::default().list_model_catalog(move |sd, ()| {
        let mut out = vec![ModelGroup {
            provider: "claude-code".into(),
            label: "Claude Code".into(),
            models: vec![choice("claude-code/claude-opus-5-5".into(), "Opus 5.5")],
            ..Default::default()
        }];
        for p in &sd.settings.compatible_providers {
            out.push(ModelGroup {
                provider: p.id.clone(),
                label: p.name.clone(),
                models: p
                    .models
                    .iter()
                    .map(|m| choice(format!("{}/{}", p.id, m.id), &m.label))
                    .collect(),
                ..Default::default()
            });
        }
        Ok(out)
    });
    let mut s = with(cx, |_| {}, opts);
    s.open_new_task();
    let h = s.model_handle();
    s.click(&h);
    assert!(s.has("sel-opt:Opus 5.5"));
    assert!(!s.has("sel-opt:GPT-4o"));
    s.keys("escape");
    s.keys("escape");
    assert!(!s.has("v-newtask"));

    s.open_settings();
    s.section("Providers");
    s.click("btn:Add provider");
    s.click("preset:OpenRouter");
    s.click("nt-go:on");
    assert!(!s.has("v-compat-provider"));

    s.open_new_task();
    let h = s.model_handle();
    s.click(&h);
    assert!(s.until(|s| s.has("sel-opt:GPT-4o")));
}

#[gpui::test]
fn a_late_project_refresh_does_not_reload_branches_or_disable_create(cx: &mut TestAppContext) {
    let gate = Arc::new(AtomicBool::new(false));
    let list_branches = Arc::new(AtomicUsize::new(0));
    type Late = (Release<Result<Vec<Project>>>, Vec<Project>);
    let release: Arc<Mutex<Option<Late>>> = Arc::default();
    let (g, r, lb) = (gate.clone(), release.clone(), list_branches.clone());
    let opts = Overrides::default()
        .list_projects(move |sd, ()| {
            if !g.load(Ordering::SeqCst) {
                return Ok(sd.projects.clone()).into();
            }
            let late: Vec<Project> = sd
                .projects
                .iter()
                .map(|p| Project {
                    name: format!("{}-late", p.name),
                    ..p.clone()
                })
                .collect();
            let (answer, rel) = hold();
            *r.lock().unwrap() = Some((rel, late));
            answer
        })
        .list_branches(move |_, _| {
            lb.fetch_add(1, Ordering::SeqCst);
            let local = |name: &str| BranchChoice {
                name: name.into(),
                group: "Local".into(),
                ..Default::default()
            };
            Ok(vec![local("main"), local("develop")])
        });
    let mut s = with(cx, |_| {}, opts);
    gate.store(true, Ordering::SeqCst);
    s.open_new_task();
    assert!(s.until(|s| s.has("nt-go:on")));
    assert!(s.has("sel:Repository=storefront"));
    let (rel, late) = release.lock().unwrap().take().expect("refresh in flight");
    rel.send(Ok(late));
    assert!(s.until(|s| s.has("sel:Repository=storefront-late")));
    assert!(s.has("nt-go:on"));
    assert_eq!(list_branches.load(Ordering::SeqCst), 1);
}

#[gpui::test]
fn slash_menu_offers_prompts_and_skills_and_at_searches_the_project(cx: &mut TestAppContext) {
    let opts = Overrides::default().list_files(|_, _| {
        Ok(["README.md", "src/main.go", "PROGRESS.md"]
            .map(String::from)
            .to_vec())
    });
    let mut s = with(cx, |_| {}, opts);
    s.open_new_task();
    let chat_focused = s.cx.update(|w, cx| {
        let m = NewTaskModal::current(cx).expect("modal");
        let c = m.read(cx).chat().clone();
        gpui_kit::Focusable::focus_handle(c.read(cx), cx).is_focused(w)
    });
    assert!(chat_focused, "the composer has focus");
    s.type_text("/");
    let menu = s.bounds("chat-mention").expect("slash menu");
    assert!(s.has("mention:/commit"));
    let composer = s.bounds("chat-composer").expect("composer");
    let vh = f32::from(s.cx.update(|w, _| w.viewport_size().height));
    assert!(f32::from(menu.origin.y) >= 0.);
    assert!(f32::from(menu.bottom()) <= vh);
    assert!(menu.origin.y > composer.origin.y);
    assert!(menu.origin.y < composer.origin.y + gpui::px(72.));
    s.type_text("design");
    assert!(s.until(|s| s.has("mention:/design-audit") || s.has("mention:design-audit")));
    s.keys("escape");
    assert!(!s.has("chat-mention"));
    assert!(s.has("v-newtask"));
    s.keys("secondary-a backspace");
    s.type_text("@PROG");
    assert!(s.until(|s| s.has("chat-mention")));
    assert!(s.until(|s| s.has("mention:PROGRESS.md")));
    s.keys("enter");
    let draft = s.cx.update(|_, cx| {
        NewTaskModal::current(cx)
            .map(|m| m.read(cx).task().to_string())
            .unwrap_or_default()
    });
    assert!(draft.contains("PROGRESS.md"), "{draft}");
}

#[gpui::test]
fn picking_a_skill_chips_it_and_create_sends_it_as_the_opening_message(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.click("foot-help");
    s.open_new_task();
    s.type_text("/design");
    assert!(s.until(|s| s.has("mention:design-audit") || s.has("mention:/design-audit")));
    if s.has("mention:design-audit") {
        s.click("mention:design-audit");
    } else {
        s.click("mention:/design-audit");
    }
    let draft = s.cx.update(|_, cx| {
        NewTaskModal::current(cx)
            .map(|m| m.read(cx).task().to_string())
            .unwrap_or_default()
    });
    assert!(draft.contains("design-audit"), "{draft}");
    s.click("nt-go:on");
    assert!(!s.has("v-newtask"));
    assert!(s.until(|s| s.view() == ViewName::Detail));
    assert!(s.until(|s| s.has("me:/design-audit") || s.has("queued:/design-audit")));
}

#[gpui::test]
fn picking_review_runs_the_prompt_and_opens_the_workspace(cx: &mut TestAppContext) {
    let opts = Overrides::default().create_workspace(|_, (project_id, _, base, ..)| {
        Ok(Workspace {
            id: "WS-NEW".into(),
            project_id,
            r#type: "solo".into(),
            status_label: "Creating".into(),
            phase: "creating".into(),
            branch: "recur/new".into(),
            base_branch: base,
            worktree_path: "/tmp/recur-e2e/WS-NEW".into(),
            ..Default::default()
        })
    });
    let mut s = with(cx, |_| {}, opts);
    s.click("foot-help");
    s.open_new_task();
    s.type_text("/review");
    assert!(s.until(|s| s.has("mention:/review")));
    s.click("mention:/review");
    assert!(s.until(|s| !s.has("v-newtask")));
    assert!(s.until(|s| s.view() == ViewName::Detail));
    assert!(s.until(|s| {
        let t = "Please review the changes in this worktree and file any real";
        s.has(&format!("me:{t}")) || s.has(&format!("queued:{t}"))
    }));
}

#[gpui::test]
fn plain_enter_inserts_a_newline(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_new_task();
    s.type_text("first line");
    s.keys("enter");
    assert!(s.has("v-newtask"));
    s.type_text("second line");
    let draft = s.cx.update(|_, cx| {
        NewTaskModal::current(cx)
            .map(|m| m.read(cx).task().to_string())
            .unwrap_or_default()
    });
    assert_eq!(draft, "first line\nsecond line");
}

#[gpui::test]
fn empty_cmd_enter_from_the_focused_composer_creates_a_workspace(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_new_task();
    assert!(s.until(|s| s.has("nt-go:on")));
    s.keys("secondary-enter");
    assert!(s.until(|s| !s.has("v-newtask")));
    assert_eq!(s.view(), ViewName::Briefing);
}

impl S<'_> {
    fn focused(&mut self) -> Option<&'static str> {
        let m = self.modal();
        self.cx
            .update(|window, cx| m.read(cx).focused_stop(window, cx))
    }
}

/// Tab reaches every field and Enter/arrows change them: the dialog's focus cycle (wrapping
/// both ways, the prompt first), pickers opened and changed from the keyboard keep focus on
/// their trigger.
#[gpui::test]
fn tab_reaches_every_field_and_enter_and_arrows_change_them(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_new_task();
    assert_eq!(s.focused(), Some("Prompt"));
    let model = s.model_handle();
    assert!(s.has(&model));
    for want in ["Mode", "Model", "Model flags", "Simplify"] {
        s.keys("tab");
        assert_eq!(s.focused(), Some(want));
    }
    assert!(s.has("nt-simplify:on"));
    for want in ["Create", "Repository", "Base branch", "Prompt"] {
        s.keys("tab");
        assert_eq!(s.focused(), Some(want), "after {want}");
    }
    s.keys("shift-tab");
    assert_eq!(s.focused(), Some("Base branch"));
    s.keys("enter");
    assert!(s.has("sel-menu"));
    s.type_text("auth");
    assert!(s.has("sel-opt:feature/auth"));
    s.keys("enter");
    assert!(!s.has("sel-menu"));
    assert!(s.has("sel:Base branch=feature/auth"));
    assert_eq!(s.focused(), Some("Base branch"));
    // Mode from the keyboard: Enter opens, ↓↓ reaches Loop, Enter picks it.
    s.keys("tab tab");
    assert_eq!(s.focused(), Some("Mode"));
    s.keys("enter");
    assert!(s.has("sel-opt:Loop"));
    s.keys("down down enter");
    assert!(s.has("sel:Mode=Loop"));
    assert_eq!(s.focused(), Some("Mode"));
    s.keys("tab");
    assert_eq!(s.focused(), Some("Loop"));
    s.keys("tab");
    assert_eq!(s.focused(), Some("Policy"));
    s.keys("enter");
    assert!(s.has("sel-menu"));
    s.keys("down enter");
    assert!(!s.has("sel-menu"));
    assert_eq!(s.focused(), Some("Policy"));
}

/// `composerOwnsDialogTab`: while the slash menu is open Tab belongs to the composer (it picks
/// the highlighted prompt), so the dialog cycle leaves focus in the prompt.
#[gpui::test]
fn tab_belongs_to_the_slash_menu_while_it_is_open(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_new_task();
    assert_eq!(s.focused(), Some("Prompt"));
    s.type_text("/");
    let m = s.modal();
    let menu = |s: &mut S| s.cx.update(|_, cx| m.read(cx).chat_menu_open(cx));
    assert!(menu(&mut s));
    s.keys("tab");
    assert_eq!(s.focused(), Some("Prompt"));
    assert!(!menu(&mut s), "the menu took the Tab");
}

/// Select menus ignore a stationary cursor during keyboard nav: a menu
/// reopened with the keyboard under a parked pointer keeps the keyboard's row active.
#[gpui::test]
fn select_menus_ignore_a_stationary_cursor_during_keyboard_nav(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_new_task();
    s.click("sel:Repository=storefront");
    let last = s
        .bounds("sel-opt:marketing-site")
        .expect("last option")
        .center();
    s.keys("escape");
    assert!(!s.has("sel-opt:marketing-site"));
    // Park the pointer where a different option will appear, then reopen with the keyboard.
    s.cx.simulate_mouse_move(last, None, gpui::Modifiers::none());
    s.keys("enter");
    assert!(s.has("sel-opt:marketing-site"), "Enter reopens the menu");
    assert!(s.has("sel-opt-active:storefront"));
    assert!(!s.has("sel-opt-active:marketing-site"));
}
