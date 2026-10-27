//! The merge and remap cases of `model_catalog_test.go`. The per-provider
//! lists (Claude flags, Cursor ordering and params, Codex labels) are tested
//! in their provider crates.

use recur_agent::catalog::ModelGroup;
use recur_domain::{
    CompatibleModel, CompatibleProvider, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX, PROVIDER_CURSOR,
    Settings, compatible_flag_preset,
};

use super::{Fx, write};
use crate::app::model_catalog::{
    apply_catalog_fallbacks, build_model_catalog, first_catalog_choice, provider_enablement_changed,
};

fn cursor_models(ids: &[(&str, &str)]) -> Vec<recur_provider_cursor::Model> {
    ids.iter()
        .map(|(id, name)| recur_provider_cursor::Model {
            id: (*id).into(),
            display_name: (*name).into(),
            ..Default::default()
        })
        .collect()
}

fn cursor(ids: &[(&str, &str)]) -> Vec<recur_agent::ModelChoice> {
    recur_provider_cursor::cursor_catalog_models(&cursor_models(ids))
}

fn codex_models(id: &str, name: &str) -> Vec<recur_provider_codex::types::Model> {
    vec![recur_provider_codex::types::Model {
        id: id.into(),
        display_name: name.into(),
        ..Default::default()
    }]
}

fn providers(groups: &[ModelGroup]) -> Vec<&str> {
    groups.iter().map(|g| g.provider.as_str()).collect()
}

fn off(ps: &[&str]) -> Settings {
    Settings {
        disabled_providers: ps.iter().map(|p| (*p).into()).collect(),
        ..Default::default()
    }
}

#[test]
fn model_catalog_omits_disabled_providers() {
    assert!(build_model_catalog(&off(&[PROVIDER_CLAUDE_CODE]), vec![], vec![]).is_empty());
    let groups = build_model_catalog(
        &off(&[PROVIDER_CLAUDE_CODE]),
        cursor(&[("composer-2.5", "Composer 2.5")]),
        vec![],
    );
    assert_eq!(providers(&groups), [PROVIDER_CURSOR]);
    let groups = build_model_catalog(
        &off(&[PROVIDER_CURSOR, PROVIDER_CODEX]),
        cursor(&[("composer-2.5", "Composer 2.5")]),
        recur_provider_codex::catalog::catalog_models(&codex_models(
            "gpt-5.6-luna",
            "GPT-5.6-Luna",
        )),
    );
    assert_eq!(providers(&groups), [PROVIDER_CLAUDE_CODE]);
}

fn slots(s: &Settings) -> [&str; 4] {
    [&s.model_ask, &s.model_plan, &s.model_agent, &s.model_loop].map(String::as_str)
}

#[test]
fn apply_catalog_fallbacks_repoints_claude_when_off() {
    let set = Settings {
        cursor_api_key: "sk-x".into(),
        model_ask: "claude-code/sonnet".into(),
        model_plan: "claude-code/opus".into(),
        model_agent: "claude-code/sonnet".into(),
        model_loop: "claude-code/opus".into(),
        model_review: "claude-code/fable".into(),
        ..off(&[PROVIDER_CLAUDE_CODE])
    };
    let groups = build_model_catalog(
        &set,
        cursor(&[("composer-2.5", "Composer 2.5"), ("grok-4.6", "Grok 4.6")]),
        vec![],
    );
    let got = apply_catalog_fallbacks(set, &groups);
    let want = "cursor/composer-2.5";
    assert_eq!(slots(&got), [want; 4]);
    assert_eq!(got.model_review, want);
    assert_eq!(
        got.model_workspace_title, want,
        "empty title defaults to haiku"
    );
    assert!(got.provider_enabled(PROVIDER_CURSOR) && !got.provider_enabled(PROVIDER_CLAUDE_CODE));
}

#[test]
fn apply_catalog_fallbacks_keeps_enabled_cursor_slot() {
    let set = Settings {
        cursor_api_key: "sk-x".into(),
        model_agent: "cursor/grok-4.6".into(),
        model_ask: "claude-code/sonnet".into(),
        ..off(&[PROVIDER_CLAUDE_CODE])
    };
    let groups = build_model_catalog(
        &set,
        cursor(&[("composer-2.5", "Composer 2.5"), ("grok-4.6", "Grok 4.6")]),
        vec![],
    );
    let got = apply_catalog_fallbacks(set, &groups);
    assert_eq!(got.model_agent, "cursor/grok-4.6");
    assert_eq!(got.model_ask, "cursor/composer-2.5");
}

#[test]
fn apply_catalog_fallbacks_leaves_empty_review() {
    let set = Settings {
        cursor_api_key: "sk-x".into(),
        model_agent: "claude-code/sonnet".into(),
        model_loop: "claude-code/sonnet".into(),
        ..off(&[PROVIDER_CLAUDE_CODE])
    };
    let groups = build_model_catalog(&set, cursor(&[("composer-2.5", "Composer 2.5")]), vec![]);
    assert_eq!(
        apply_catalog_fallbacks(set, &groups).model_review,
        "",
        "inherits Loop"
    );
}

#[test]
fn apply_catalog_fallbacks_reenables_claude_when_catalog_empty() {
    let set = Settings {
        cursor_api_key: "sk-x".into(),
        model_agent: "claude-code/sonnet".into(),
        ..off(&[PROVIDER_CLAUDE_CODE])
    };
    let got = apply_catalog_fallbacks(set, &[]);
    assert!(got.provider_enabled(PROVIDER_CLAUDE_CODE));
    assert_eq!(got.model_agent, "claude-code/sonnet");
}

#[test]
fn apply_catalog_fallbacks_removed_compatible_when_claude_off() {
    let beta = CompatibleProvider {
        id: "beta".into(),
        name: "Beta".into(),
        base_url: "https://b.example".into(),
        models: vec![CompatibleModel {
            id: "m2".into(),
            label: "M2".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let set = Settings {
        compatible_providers: vec![beta],
        model_ask: "alpha/m1".into(),
        model_plan: "alpha/m1".into(),
        model_agent: "alpha/m1".into(),
        model_loop: "alpha/m1".into(),
        model_workspace_title: "alpha/m1".into(),
        ..off(&[PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR, PROVIDER_CODEX])
    };
    assert!(set.needs_catalog_fallback(), "alpha was removed");
    let groups = build_model_catalog(&set, vec![], vec![]);
    let got = apply_catalog_fallbacks(set, &groups);
    assert_eq!(slots(&got), ["beta/m2"; 4]);
    assert_eq!(got.model_workspace_title, "beta/m2");
}

#[test]
fn provider_enablement_changed_cases() {
    let a = Settings {
        cursor_api_key: "sk-x".into(),
        ..Default::default()
    };
    assert!(!provider_enablement_changed(&a, &a.clone()));
    let b = Settings {
        disabled_providers: vec![PROVIDER_CLAUDE_CODE.into()],
        ..a.clone()
    };
    assert!(provider_enablement_changed(&a, &b), "Claude off");
    let b = Settings {
        cursor_api_key: String::new(),
        ..a.clone()
    };
    assert!(provider_enablement_changed(&a, &b), "Cursor key cleared");
    let b = Settings {
        bypass_permissions: true,
        ..a.clone()
    };
    assert!(!provider_enablement_changed(&a, &b), "unrelated field");
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_uses_cached_catalog() {
    let fx = Fx::new().await;
    fx.svc
        .providers
        .cursor
        .remember_models(cursor_models(&[("composer-2.5", "Composer 2.5")]));
    let bridge = fx.root.join("cursor-bridge/bin/cursor-sdk-bridge");
    write(&bridge, "#!/bin/sh\n");
    // Executable, so Cursor is runnable here without a host Codex.
    std::fs::set_permissions(&bridge, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut set = fx.s.settings.get_settings().await.unwrap();
    set.cursor_api_key = "sk-x".into();
    set.disabled_providers = vec![PROVIDER_CLAUDE_CODE.into()];
    for m in [
        &mut set.model_ask,
        &mut set.model_plan,
        &mut set.model_agent,
        &mut set.model_loop,
    ] {
        *m = "claude-code/sonnet".into();
    }
    let got = fx.s.settings.set_settings(&set).await.unwrap();
    assert_eq!(
        got.model_agent, "cursor/composer-2.5",
        "from the cached catalogue"
    );
    assert!(!got.provider_enabled(PROVIDER_CLAUDE_CODE));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn catalog_for_fallback_uses_cache() {
    let fx = Fx::new().await;
    fx.svc
        .providers
        .cursor
        .remember_models(cursor_models(&[("composer-2.5", "Composer 2.5")]));
    let set = Settings {
        cursor_api_key: "sk-x".into(),
        ..off(&[PROVIDER_CLAUDE_CODE])
    };
    let (id, _) = first_catalog_choice(&fx.svc.catalog_for_fallback(&set).await);
    assert_eq!(id, "cursor/composer-2.5");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn catalog_for_fallback_prefers_cached_codex() {
    let fx = Fx::new().await;
    fx.svc
        .providers
        .codex
        .remember_models(codex_models("gpt-5.6-luna", "GPT-5.6 Luna"));
    let set = Settings {
        codex_api_key: "sk-x".into(),
        ..off(&[PROVIDER_CLAUDE_CODE])
    };
    let (id, _) = first_catalog_choice(&fx.svc.catalog_for_fallback(&set).await);
    assert_eq!(id, "codex/gpt-5.6-luna");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn catalog_for_fallback_live_when_cache_cold() {
    let fx = Fx::new().await;
    let set = Settings {
        cursor_api_key: "sk-x".into(),
        cursor_bridge_path: format!("{}/missing-bridge", fx.temp()),
        ..off(&[PROVIDER_CLAUDE_CODE])
    };
    let (id, _) = first_catalog_choice(&fx.svc.catalog_for_fallback(&set).await);
    assert_eq!(id, "", "cold cache and no bridge");
    fx.close().await;
}

#[test]
fn model_catalog_includes_compatible_providers() {
    let mut set = Settings {
        compatible_providers: vec![
            CompatibleProvider {
                id: "openai".into(),
                name: "OpenAI".into(),
                base_url: "https://api.openai.com".into(),
                models: vec![CompatibleModel {
                    id: "gpt-5".into(),
                    label: "GPT-5".into(),
                    flags: compatible_flag_preset("openai"),
                    ..Default::default()
                }],
                ..Default::default()
            },
            CompatibleProvider {
                id: "groq".into(),
                name: "Groq".into(),
                base_url: "https://api.groq.com/openai".into(),
                models: vec![CompatibleModel {
                    id: "llama-3.1-70b".into(),
                    label: "Llama".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        ],
        ..off(&[PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR, PROVIDER_CODEX])
    };
    let groups = build_model_catalog(&set, vec![], vec![]);
    assert_eq!(providers(&groups), ["openai", "groq"]);
    assert_eq!(groups[0].label, "OpenAI");
    let refs = |g: &ModelGroup| {
        g.models
            .iter()
            .map(|m| m.model_ref.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(refs(&groups[0]), ["openai/gpt-5"]);
    assert_eq!(
        groups[0].models[0].flags.len(),
        2,
        "effort + verbosity, no wire"
    );
    assert_eq!(refs(&groups[1]), ["groq/llama-3.1-70b"]);

    set.disabled_providers.push("openai".into());
    assert_eq!(
        providers(&build_model_catalog(&set, vec![], vec![])),
        ["groq"]
    );
}
