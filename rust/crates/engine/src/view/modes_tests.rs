//! Port of the chat-mode cases of `lib/modes.test.ts` (the workspace
//! predicate cases are `recur_view_core::modes`').

use recur_agent::catalog::{FlagDimension, FlagOption};
use recur_domain::{Flags, Settings, Workspace};

use super::*;

fn flags(pairs: &[(&str, &str)]) -> Flags {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn set() -> Settings {
    Settings {
        model_ask: "claude-code/haiku".into(),
        model_plan: "claude-code/opus".into(),
        model_agent: "claude-code/sonnet".into(),
        model_review: "claude-code/fable".into(),
        model_loop: "claude-code/opus".into(),
        flags_ask: flags(&[("effort", "low")]),
        flags_plan: flags(&[("effort", "high")]),
        flags_agent: flags(&[("effort", "medium")]),
        flags_review: flags(&[("effort", "high")]),
        flags_loop: flags(&[("effort", "high")]),
        ..Settings::default()
    }
}

fn opt(value: &str, label: &str, default: bool) -> FlagOption {
    FlagOption {
        value: value.into(),
        label: label.into(),
        default,
    }
}

fn effort(default: &str) -> Vec<FlagDimension> {
    vec![FlagDimension {
        id: "effort".into(),
        label: "Effort".into(),
        options: ["low", "medium", "high"]
            .iter()
            .map(|v| {
                let label = match *v {
                    "low" => "Low",
                    "medium" => "Medium",
                    _ => "High",
                };
                opt(v, label, *v == default)
            })
            .collect(),
    }]
}

fn claude(r#ref: &str, label: &str, default: &str) -> CatalogModel {
    CatalogModel {
        r#ref: r#ref.into(),
        label: label.into(),
        group: "Claude".into(),
        provider: "claude-code".into(),
        flags: effort(default),
    }
}

fn catalog() -> Vec<CatalogModel> {
    vec![
        claude("claude-code/opus", "Opus", "high"),
        claude("claude-code/sonnet", "Sonnet", "medium"),
        claude("claude-code/fable", "Fable", "high"),
        claude("claude-code/haiku", "Haiku", "low"),
    ]
}

fn cursor_catalog() -> Vec<CatalogModel> {
    vec![CatalogModel {
        r#ref: "cursor/composer-2.5".into(),
        label: "Composer 2.5".into(),
        group: "Cursor".into(),
        provider: "cursor".into(),
        flags: vec![FlagDimension {
            id: "fast".into(),
            label: "Fast".into(),
            options: vec![opt("true", "On", true), opt("false", "Off", false)],
        }],
    }]
}

#[test]
fn default_model_maps_each_mode_to_its_slot() {
    let s = set();
    assert_eq!(
        default_model_for_mode("ask", Some(&s)).model,
        "claude-code/haiku"
    );
    assert_eq!(
        default_model_for_mode("plan", Some(&s)).model,
        "claude-code/opus"
    );
    assert_eq!(
        default_model_for_mode("agent", Some(&s)).model,
        "claude-code/sonnet"
    );
    assert_eq!(
        default_model_for_mode("review", Some(&s)).model,
        "claude-code/fable"
    );
    assert_eq!(
        default_model_for_mode("loop", Some(&s)).model,
        "claude-code/opus"
    );
}

#[test]
fn resolved_default_skips_a_turned_off_provider() {
    let mut mixed = catalog();
    mixed.extend(cursor_catalog());
    let off = Settings {
        disabled_providers: vec!["claude-code".into()],
        ..set()
    };
    let got = resolved_default_for_mode("agent", Some(&off), &mixed);
    assert_eq!(got.model, "cursor/composer-2.5");
    assert_eq!(got.flags, flags(&[("fast", "true")]));
}

#[test]
fn resolved_default_uses_a_remapped_slot_with_claude_off() {
    let off = Settings {
        model_agent: "cursor/composer-2.5".into(),
        flags_agent: flags(&[("fast", "false")]),
        disabled_providers: vec!["claude-code".into()],
        ..set()
    };
    let got = resolved_default_for_mode("agent", Some(&off), &cursor_catalog());
    assert_eq!(got.model, "cursor/composer-2.5");
    assert_eq!(got.flags, flags(&[("fast", "false")]));
}

#[test]
fn unhydrated_pickers_take_the_new_mode_default() {
    let (s, c) = (set(), catalog());
    let next = picker_after_mode_change(
        "agent",
        "review",
        &s.model_agent,
        &Flags::new(),
        Some(&s),
        &c,
        false,
        false,
    );
    assert_eq!(next, resolved_default_for_mode("review", Some(&s), &c));
}

#[test]
fn hydrated_default_follows_the_new_default_on_an_unused_chat() {
    let (s, c) = (set(), catalog());
    let from = resolved_default_for_mode("agent", Some(&s), &c);
    assert!(is_mode_default_selection(
        "agent",
        &from.model,
        &from.flags,
        Some(&s),
        &c
    ));
    let next = picker_after_mode_change(
        "agent",
        "review",
        &from.model,
        &from.flags,
        Some(&s),
        &c,
        true,
        false,
    );
    assert_eq!(next, resolved_default_for_mode("review", Some(&s), &c));
}

#[test]
fn hydrated_default_is_kept_once_the_session_started() {
    let (s, c) = (set(), catalog());
    let from = resolved_default_for_mode("agent", Some(&s), &c);
    let next = picker_after_mode_change(
        "agent",
        "ask",
        &from.model,
        &from.flags,
        Some(&s),
        &c,
        true,
        true,
    );
    assert_eq!(next, from);
}

#[test]
fn explicit_picks_are_kept_across_modes() {
    let (s, c) = (set(), catalog());
    let want = ModelPick {
        model: "claude-code/opus".into(),
        flags: flags(&[("effort", "low")]),
    };
    let next = picker_after_mode_change(
        "agent",
        "review",
        &want.model,
        &want.flags,
        Some(&s),
        &c,
        true,
        false,
    );
    assert_eq!(next, want);
    let started = picker_after_mode_change(
        "agent",
        "plan",
        &want.model,
        &want.flags,
        Some(&s),
        &c,
        true,
        true,
    );
    assert_eq!(started, want);
}

#[test]
fn plan_and_ask_targets_get_mode_defaults_when_unhydrated() {
    let (s, c) = (set(), catalog());
    for to in ["plan", "ask"] {
        assert_eq!(
            picker_after_mode_change(
                "agent",
                to,
                &s.model_agent,
                &Flags::new(),
                Some(&s),
                &c,
                false,
                false
            ),
            resolved_default_for_mode(to, Some(&s), &c)
        );
    }
}

#[test]
fn chat_has_started_rules() {
    assert!(!chat_has_started(None, None));
    assert!(!chat_has_started(Some(""), Some(&[])));
    assert!(!chat_has_started(Some("New chat"), Some(&[])));
    assert!(!chat_has_started(Some("New chat"), Some(&[true])));
    assert!(chat_has_started(Some("New chat"), Some(&[false])));
    assert!(chat_has_started(Some("Fix the dock"), Some(&[])));
    assert!(chat_has_started(Some("Image"), None));
}

fn ws(f: impl FnOnce(&mut Workspace)) -> Workspace {
    let mut w = Workspace {
        r#type: recur_domain::WORKSPACE_SOLO.into(),
        ..Workspace::default()
    };
    f(&mut w);
    w
}

#[test]
fn setup_status_shows_setup_and_slot_waits() {
    let s = |f: fn(&mut Workspace)| ws_setup_status(Some(&ws(f)));
    assert_eq!(
        s(|w| w.phase = "creating".into()),
        "Setting up the worktree…"
    );
    assert_eq!(
        s(|w| {
            w.phase = "creating".into();
            w.worktree_path = "/tmp/wt".into();
        }),
        "Setting up the worktree…"
    );
    assert_eq!(
        s(|w| w.phase = "waiting for a slot".into()),
        "Waiting for a loop slot…"
    );
    assert_eq!(
        s(|w| {
            w.phase = "waiting for a slot".into();
            w.worktree_path = "/tmp/wt".into();
        }),
        "Waiting for a loop slot…"
    );
    assert_eq!(
        s(|w| {
            w.phase = "starting".into();
            w.worktree_path = "/tmp/wt".into();
        }),
        ""
    );
}

#[test]
fn chat_held_until_setup_finishes() {
    let h = |f: fn(&mut Workspace)| ws_chat_held(Some(&ws(f)));
    assert!(h(|w| w.phase = "creating".into()));
    assert!(h(|w| {
        w.phase = "creating".into();
        w.worktree_path = "/tmp/wt".into();
    }));
    assert!(h(|w| w.phase = "editing".into()));
    assert!(!h(|w| {
        w.phase = "editing".into();
        w.worktree_path = "/tmp/wt".into();
    }));
    assert!(!h(|w| {
        w.is_main = true;
        w.phase = "creating".into();
    }));
    assert!(!h(|w| {
        w.phase = "failed".into();
        w.last_error = "git failed".into();
        w.status_label = "Couldn't create".into();
    }));
}

#[test]
fn create_failed_is_not_a_failed_loop() {
    assert!(ws_create_failed(Some(&ws(|w| {
        w.phase = "failed".into();
        w.last_error = "git failed".into();
        w.status_label = "Couldn't create".into();
    }))));
    assert!(ws_create_failed(Some(&ws(|w| {
        w.phase = "failed".into();
        w.last_error = "setup command failed".into();
        w.worktree_path = "/tmp/wt".into();
        w.status_label = "Couldn't create".into();
    }))));
    for (label, err) in [
        ("Failed", "agent died"),
        ("Couldn't create", "dispatch failed"),
        ("Couldn't create", "allocate worktree: bad base"),
    ] {
        let w = Workspace {
            phase: "failed".into(),
            last_error: err.into(),
            status_label: label.into(),
            run_started: true,
            ..Workspace::default()
        };
        assert!(!ws_create_failed(Some(&w)), "{err}");
    }
    assert!(!ws_create_failed(None));
}

#[test]
fn cycle_and_options_follow_selector_order() {
    assert_eq!(cycle_chat_mode("ask", false, false), "plan");
    assert_eq!(cycle_chat_mode("review", false, false), "loop");
    assert_eq!(cycle_chat_mode("loop", false, false), "ask");
    assert_eq!(cycle_chat_mode("review", true, false), "ask");
    assert_eq!(cycle_chat_mode("plan", false, true), "review");
    assert_eq!(cycle_chat_mode("bogus", false, false), "review");
    let values: Vec<_> = chat_mode_options(true, true)
        .iter()
        .map(|o| o.value)
        .collect();
    assert_eq!(values, ["ask", "plan", "review"]);
    assert_eq!(chat_mode_meta("").label, "Agent");
    assert_eq!(chat_mode_meta("plan").tint, "#facc15");
}
