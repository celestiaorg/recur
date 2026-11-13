//! The Settings scenarios. The New workspace modal, the project
//! settings view and the add-project modal halves belong to the views that host them.

use gpui::TestAppContext;
use recur_services::api::{CursorStatus, ModelChoice, ModelGroup, ProviderQuota, Snapshot};
use recur_services::fake::Overrides;
use recur_services::{AppEvent, Error};
use recur_ui::widgets::ToastKind;
use recur_view_core::LocalStorage;
use serde_json::{Value, json};

use super::harness::{S, boot, plain, with};

fn models_and_loops(s: &mut S) {
    s.open_settings();
    assert!(s.has("set-head=General"));
    s.section("Models");
    for label in ["Ask", "Plan", "Agent / Coder", "Review", "Loop"] {
        assert!(
            s.has(&format!("sel:{label} model=Sonnet 5.5")),
            "{label} model"
        );
    }
    assert!(s.has("sel:Review flags=High"));
    s.section("Loops");
    assert!(s.has("sel:Loop=recur.loops.pr"));
    assert!(s.has("sel:Policy=Balanced"));
}

#[gpui::test]
fn settings_exposes_editable_models_and_the_fixed_rail_set(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    models_and_loops(&mut s);
}

#[gpui::test]
fn cursor_without_the_sdk_warns_and_installs(cx: &mut TestAppContext) {
    let installed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let status = |on: bool| CursorStatus {
        bridge_installed: on,
        authenticated: on,
        email: if on { "dev@example.com" } else { "" }.into(),
        error: if on {
            ""
        } else {
            "cursor-sdk-bridge not found"
        }
        .into(),
        ..Default::default()
    };
    let usage = Snapshot {
        fetched_at: 1,
        stale: false,
        providers: Some(vec![ProviderQuota {
            id: "cursor".into(),
            name: "Cursor".into(),
            email: "dev@example.com".into(),
            source: "Cursor Connect".into(),
            source_id: "cursor_connect".into(),
            ..Default::default()
        }]),
    };
    let (a, b) = (installed.clone(), installed.clone());
    let (u1, u2) = (usage.clone(), usage);
    let opts = Overrides::default()
        .get_cursor_status(move |_, ()| Ok(status(a.load(std::sync::atomic::Ordering::SeqCst))))
        .install_cursor_bridge(move |_, ()| {
            b.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(status(true))
        })
        .get_provider_usage(move |_, ()| Ok(u1.clone()))
        .refresh_provider_usage(move |_, ()| Ok(u2.clone()));
    let mut s = with(
        cx,
        |sd| sd.settings.cursor_api_key = "crsr_test".into(),
        opts,
    );
    s.open_settings();
    s.section("Providers");
    assert!(s.has("status-head:Warn:Connected, SDK not installed"));
    assert!(s.has("acct-sdk-note"));
    s.click("btn:Install SDK");
    assert!(s.until(|s| !s.has("acct-sdk-note")));
    assert!(!s.has("status-head:Warn:Connected, SDK not installed"));
}

#[gpui::test]
fn adding_a_compatible_provider_reaches_the_model_catalog(cx: &mut TestAppContext) {
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
            });
        }
        Ok(out)
    });
    let mut s = with(cx, |_| {}, opts);
    s.open_settings();
    s.section("Models");
    s.click("sel:Agent / Coder model=claude-sonnet-5-5");
    assert!(s.has("sel-opt:Opus 5.5"));
    assert!(!s.has("sel-opt:GPT-4o"));
    s.keys("escape");

    s.section("Providers");
    s.click("btn:Add provider");
    assert!(s.has("v-compat-provider"));
    s.click("preset:OpenRouter");
    s.click("nt-go:on");
    assert!(!s.has("v-compat-provider"));
    assert_eq!(s.saved().compatible_providers[0].id, "openrouter");

    s.section("Models");
    s.click("sel:Agent / Coder model=claude-sonnet-5-5");
    assert!(s.until(|s| s.has("sel-opt:GPT-4o")));
}

#[gpui::test]
fn changing_the_agent_model_is_accepted(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Models");
    s.pick("Agent / Coder model", "Sonnet 5.5", "Haiku 4.5");
    assert!(s.has("sel:Agent / Coder model=Haiku 4.5"));
    assert_eq!(
        s.saved().model_agent,
        "claude-code/claude-haiku-4-5-20251001"
    );
}

#[gpui::test]
fn changing_review_flags_is_accepted(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Models");
    s.pick("Review flags", "High", "Extra High");
    assert!(s.has("sel:Review flags=Extra High"));
    assert_eq!(s.saved().flags_review["effort"], "xhigh");
}

#[gpui::test]
fn appearance_picker_switches_and_persists_the_theme(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Appearance");
    for id in recur_ui::theme::ThemeId::ALL {
        assert!(
            s.has(&format!("thcard:app-{}", id.key()))
                || s.has(&format!("thcard:app-{}:on", id.key())),
            "{id:?}"
        );
    }
    assert!(s.has("thcard:app-default:on"));
    assert_eq!(s.theme(), "default");

    for key in ["nord", "paper", "catppuccin-latte"] {
        s.click(&format!("thcard:app-{key}"));
        assert_eq!(s.theme(), key);
        assert!(s.has(&format!("thcard:app-{key}:on")));
        assert_eq!(s.storage("recur.theme").as_deref(), Some(key));
    }
    s.click("thcard:app-default");
    assert_eq!(s.theme(), "default");
}

#[gpui::test]
fn legacy_catppuccin_theme_key_migrates_to_mocha(cx: &mut TestAppContext) {
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", "catppuccin".to_string());
    let mut s = boot(cx, |_| {}, Overrides::default(), storage);
    assert_eq!(s.theme(), "catppuccin-mocha");
    assert_eq!(
        s.storage("recur.theme").as_deref(),
        Some("catppuccin-mocha")
    );
}

#[gpui::test]
fn match_system_appearance_uses_the_chosen_light_and_dark_themes(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.system_dark(false);
    s.open_settings();
    s.section("Appearance");

    s.click("seg:App theme mode:Sync with system");
    assert_eq!(s.theme(), "paper");
    let light = recur_ui::theme::ThemeId::ALL
        .iter()
        .filter(|id| id.is_light())
        .count();
    assert_eq!(light, 2);
    assert!(s.has("thcard:app-light-paper:on"));
    assert!(s.has("thcard:app-light-catppuccin-latte"));
    assert!(s.has("thcard:app-dark-default:on"));
    s.click("thcard:app-dark-nord");
    s.system_dark(true);
    assert_eq!(s.theme(), "nord");

    s.click("thcard:app-dark-default");
    assert_eq!(s.theme(), "default");

    s.click("seg:App theme mode:Single theme");
    s.click("thcard:app-slate");
    s.system_dark(false);
    assert_eq!(s.theme(), "slate");
}

fn terminal_theme(s: &mut S) -> String {
    let raw = s.storage("recur.terminalPrefs").unwrap_or_default();
    serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null)["theme"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[gpui::test]
fn editor_and_terminal_themes_can_sync_with_the_system(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.system_dark(false);
    s.open_settings();
    s.section("Appearance");

    s.click("seg:Code editor theme mode:Sync with system");
    assert!(s.has("edth-list:edth-light"));
    assert!(s.has("edth-list:edth-dark"));
    assert!(s.until(|s| s.storage("recur.editorTheme").as_deref() == Some("paper")));
    s.system_dark(true);
    assert_eq!(s.storage("recur.editorTheme").as_deref(), Some("recur"));

    s.click("seg:Terminal theme mode:Sync with system");
    assert!(s.until(|s| s.has("thcard:term-dark-homebrew")));
    s.click("thcard:term-dark-homebrew");
    assert_eq!(terminal_theme(&mut s), "homebrew");
    s.system_dark(false);
    assert_eq!(terminal_theme(&mut s), "");
}

#[gpui::test]
fn terminal_theme_picker_is_independent_and_persists(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Appearance");
    assert!(s.has("thcard:term-match:on"));
    assert!(s.until(|s| s.has("thcard:term-homebrew")));
    s.click("thcard:term-homebrew");
    assert!(s.has("thcard:term-homebrew:on"));
    assert_eq!(terminal_theme(&mut s), "homebrew");
}

#[gpui::test]
fn code_editor_theme_is_independent_and_persists(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Appearance");
    assert!(s.until(|s| s.has("edth:Charcoal:on")));
    assert!(s.has("edth:Paper"));
    assert!(s.until(|s| s.has("edth:Cool Glow")));

    s.click("edth:Paper");
    assert!(s.has("edth:Paper:on"));
    assert_eq!(s.storage("recur.editorTheme").as_deref(), Some("paper"));

    s.click("edth:Cool Glow");
    assert!(s.has("edth:Cool Glow:on"));
    assert_eq!(s.storage("recur.editorTheme").as_deref(), Some("cool-glow"));
    assert_eq!(s.theme(), "default");
}

#[gpui::test]
fn code_editor_layout_prefs_persist(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Code editor");

    s.click("sel:Editor font=JetBrains Mono");
    s.click("sel-opt:IBM Plex Mono");
    assert!(s.has("sel:Editor font=IBM Plex Mono"));

    for (group, label) in [
        ("Editor font size", "11"),
        ("Line wrapping", "Scroll"),
        ("Tab size", "2"),
        ("Default indent style", "2 spaces"),
    ] {
        s.click(&format!("seg:{group}:{label}"));
    }
    s.click("seg:Go indent:4");
    assert!(s.has("indent-desc:go=Override · 4 spaces"));
    assert!(!s.has("indent-desc:go=Auto · Tabs(gofmt)"));
    assert!(s.has("indent-desc:typescript=Auto · 2 spaces"));

    for (group, label) in [
        ("Render whitespace", "Trailing"),
        ("Special characters", "Off"),
        ("Line numbers", "Off"),
        ("Scroll past end", "On"),
        ("Line height", "Relaxed"),
        ("Thick caret", "On"),
        ("Print margin", "80"),
        ("Editor keymap", "Vim"),
    ] {
        s.click(&format!("seg:{group}:{label}"));
    }

    let saved: Value =
        serde_json::from_str(&s.storage("recur.editorPrefs").expect("editor prefs")).unwrap();
    let want = json!({
        "fontFace": "IBM Plex Mono", "fontSize": 11, "lineWrapping": false, "tabSize": 2,
        "indent": "spaces-2", "indentByLang": { "go": "spaces-4" }, "whitespace": "trailing",
        "specialChars": false, "lineNumbers": false, "scrollPastEnd": true, "lineHeight": 1.7,
        "thickCaret": true, "rulerColumn": 80, "keymap": "vim",
    });
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(&saved[k], v, "{k}");
    }
}

#[gpui::test]
fn loops_section_shows_loop_and_policy_pickers(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Loops");
    assert!(s.has("sel:Loop=recur.loops.pr"));
    s.pick("Policy", "Balanced", "Aggressive");
    assert!(s.has("sel:Policy=Aggressive"));
}

#[gpui::test]
fn a_failed_policy_save_shows_an_error_toast(cx: &mut TestAppContext) {
    let opts = Overrides::default().set_settings(|_, _| Err(Error::new("settings save failed")));
    let mut s = with(cx, |_| {}, opts);
    s.open_settings();
    s.section("Loops");
    s.pick("Policy", "Balanced", "Aggressive");
    assert!(s.until(|s| s.toasted(ToastKind::Error, "")));
}

#[gpui::test]
fn general_settings_include_a_branch_name_prefix_field(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("General");
    assert!(s.has("inp:Branch name prefix"));
    assert!(s.has("seg:Prefix presets:Recur:on"));
    assert!(s.has("branch-example=recur/crispy-quiche"));
    s.fill("inp:Branch name prefix", "typed-prefix");
    assert!(s.has("branch-example=typed-prefix/crispy-quiche"));
    s.click("seg:Prefix presets:None");
    assert!(s.has("branch-example=crispy-quiche"));
    assert_eq!(s.saved().branch_name_prefix.as_deref(), Some(""));
    assert!(s.until(|s| s.has("seg:Prefix presets:GitHub")));
    s.click("seg:Prefix presets:GitHub");
    assert!(s.has("branch-example=mycodecrafting/crispy-quiche"));
    assert_eq!(
        s.saved().branch_name_prefix.as_deref(),
        Some("mycodecrafting")
    );
    s.click("seg:Prefix presets:Recur");
    assert!(s.has("branch-example=recur/crispy-quiche"));
    assert!(s.has("seg:Prefix presets:Recur:on"));
}

#[gpui::test]
fn general_settings_include_live_concurrency_caps(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("General");
    assert!(s.has("inp:Max concurrent loops"));
    assert!(s.has("inp:Max concurrent agents"));
    let v =
        s.cx.update(|_, cx| crate::view::SettingsView::current(cx))
            .expect("settings view");
    let (loops, agents) = s.cx.update(|_, cx| {
        let v = v.read(cx);
        (
            v.field_value("max-loops", cx),
            v.field_value("max-agents", cx),
        )
    });
    assert_eq!((loops.as_str(), agents.as_str()), ("4", "8"));
}

#[gpui::test]
fn pr_settings_radio_dots_share_a_column(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Pull requests");
    let mut xs = Vec::new();
    for c in &crate::sections::prs::CHOICES {
        let (title, first) = (c.title, c.opts[0].1);
        let sel = [true, false]
            .into_iter()
            .map(|on| format!("radio:{title}:{first}:{on}"))
            .find(|sel| s.has(sel))
            .unwrap_or_else(|| panic!("{title}"));
        xs.push(f32::from(s.bounds(&sel).unwrap().origin.x));
    }
    assert_eq!(xs.len(), 4);
    for x in &xs {
        assert!((x - xs[0]).abs() < 1., "{xs:?}");
    }
}

#[gpui::test]
fn pr_automatic_fixes_are_off_until_turned_on(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Pull requests");
    for title in [
        "Fix merge conflicts automatically",
        "Fix failing checks automatically",
        "Address review comments automatically",
    ] {
        assert!(s.has(&format!("tog:{title}:false")), "{title}");
    }
    s.click("tog:Fix failing checks automatically:false");
    assert!(s.has("tog:Fix failing checks automatically:true"));
    let saved = s.saved();
    assert!(saved.auto_fix_checks);
    assert!(!saved.auto_update_conflicts && !saved.auto_address_review);
}

/// The menu item (`menu.action` `settings`) and ⌘, open
/// Settings on General.
#[gpui::test]
fn the_menu_action_and_cmd_comma_open_settings(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.dispatch(AppEvent::MenuAction("settings".into()));
    assert_eq!(s.view(), recur_view_core::ViewName::Settings);
    assert!(s.has("set-head=General"));
    s.section("Data");
    s.click("nav-pr");
    assert_ne!(s.view(), recur_view_core::ViewName::Settings);
    s.keys("secondary-,");
    assert_eq!(s.view(), recur_view_core::ViewName::Settings);
    assert!(s.has("v-settings:on"));
}

#[gpui::test]
fn licenses_section_shows_the_third_party_licenses(cx: &mut TestAppContext) {
    let mut s = plain(cx);
    s.open_settings();
    s.section("Licenses");
    assert!(s.has("set-head=Licenses"));
}

/// Off by default; turning it on adds the sidebar row and the ⌥⌘5 route, a dock without the
/// workspace's chrome; turning it off leaves the page.
#[gpui::test]
fn terminals_page_toggle_shows_the_sidebar_row_and_route(cx: &mut TestAppContext) {
    use recur_view_core::ViewName;
    let mut s = plain(cx);
    assert!(!s.has("nav-terminals"));
    s.dispatch(AppEvent::MenuAction("view:terminals".into()));
    assert_ne!(s.view(), ViewName::Terminals);

    s.open_settings();
    s.section("General");
    s.click("tog:toggle terminals page:false");
    assert!(
        s.storage("recur.terminalPrefs")
            .unwrap()
            .contains("\"page\":true")
    );
    s.click("nav-terminals");
    assert_eq!(s.view(), ViewName::Terminals);
    assert!(s.has("term-dock"));
    assert!(s.has("term-act:new"));
    assert!(!s.has("term-act:maximize"));
    assert!(!s.has("term-act:hide"));
    let new_tab = |s: &S| s.state.read_with(&*s.cx, |st, _| st.menu_state.new_tab);
    assert_eq!(new_tab(&s), Some(true), "⌘T reaches the page's dock");

    s.open_settings();
    assert_eq!(new_tab(&s), Some(false), "leaving the page idles the menu");
    s.dispatch(AppEvent::MenuAction("view:terminals".into()));
    assert_eq!(s.view(), ViewName::Terminals);
    s.state
        .update(s.cx, |st, cx| st.set_terminal_prefs(|p| p.page = false, cx));
    assert_eq!(s.view(), ViewName::Briefing);
    assert!(!s.has("nav-terminals"));
}
