//! The model catalog the pickers read (`lib/models.ts`). The list is server-owned
//! (`SettingsService::list_model_catalog`); [`AppState`](crate::AppState) keeps the shared
//! cache and its revision, these are the pure rules over it.

use std::collections::BTreeMap;

use recur_services::api::{FlagDimension, ModelGroup};

/// One selectable model, flattened for the pickers (`CatalogModel`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogModel {
    pub model_ref: String,
    pub label: String,
    pub group: String,
    pub provider: String,
    pub flags: Vec<FlagDimension>,
}

pub type FlagMap = BTreeMap<String, String>;

/// `flattenModelCatalog`.
pub fn flatten_model_catalog(groups: &[ModelGroup]) -> Vec<CatalogModel> {
    groups
        .iter()
        .flat_map(|g| {
            g.models.iter().map(|m| CatalogModel {
                model_ref: m.model_ref.clone(),
                label: m.label.clone(),
                group: g.label.clone(),
                provider: g.provider.clone(),
                flags: m.flags.clone(),
            })
        })
        .collect()
}

/// `normalizeFlags`: fill missing dimensions from the catalog defaults; drop dimensions the
/// model no longer has.
pub fn normalize_flags(dims: &[FlagDimension], cur: Option<&FlagMap>) -> FlagMap {
    let mut out = FlagMap::new();
    for d in dims {
        if let Some(v) = cur.and_then(|c| c.get(&d.id))
            && !v.is_empty()
            && d.options.iter().any(|o| &o.value == v)
        {
            out.insert(d.id.clone(), v.clone());
            continue;
        }
        if let Some(def) = d.options.iter().find(|o| o.default).or(d.options.first()) {
            out.insert(d.id.clone(), def.value.clone());
        }
    }
    out
}

pub fn flags_equal(a: &FlagMap, b: &FlagMap) -> bool {
    a == b
}

/// A picker row (`ModelSelectOption`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelectOption {
    pub value: String,
    pub label: String,
    pub group: String,
    pub provider: String,
}

pub fn catalog_select_options(models: &[CatalogModel]) -> Vec<ModelSelectOption> {
    models
        .iter()
        .map(|m| ModelSelectOption {
            value: m.model_ref.clone(),
            label: m.label.clone(),
            group: m.group.clone(),
            provider: m.provider.clone(),
        })
        .collect()
}

/// `shortModelLabel`: a qualified ref's label when the catalog row is not available yet.
pub fn short_model_label(model_ref: &str) -> String {
    if model_ref.is_empty() {
        return String::new();
    }
    let (provider, id) = match model_ref.find('/') {
        Some(i) => (&model_ref[..i], &model_ref[i + 1..]),
        None => ("", model_ref),
    };
    if provider == "codex" || provider == "openai" {
        return friendly_codex_slug(id);
    }
    id.to_string()
}

/// Server-side `friendlyCodexModelLabel` for slug-only fallbacks.
fn friendly_codex_slug(id: &str) -> String {
    let mut s = id.trim().to_string();
    if s.is_empty() {
        return s;
    }
    if s.len() >= 4 && s[..4].eq_ignore_ascii_case("gpt-") {
        s = format!("GPT-{}", &s[4..]);
    }
    if let Some(dash) = s.rfind('-')
        && dash > 0
    {
        let tail = &s[dash + 1..];
        if !tail.is_empty() && !tail.chars().any(|c| c.is_ascii_digit() || c == '.') {
            let mut chars = tail.chars();
            let first = chars.next().map(|c| c.to_uppercase().to_string());
            return format!(
                "{} {}{}",
                &s[..dash],
                first.unwrap_or_default(),
                chars.as_str()
            );
        }
    }
    s
}

/// `modelOptionsFor`: keep the current value selectable before the catalog loads (or once it no
/// longer offers that id).
pub fn model_options_for(models: &[CatalogModel], current: &str) -> Vec<ModelSelectOption> {
    let opts = catalog_select_options(models);
    if current.is_empty() || opts.iter().any(|o| o.value == current) {
        return opts;
    }
    let provider = current.find('/').map_or("", |i| &current[..i]);
    let mut out = vec![ModelSelectOption {
        value: current.to_string(),
        label: short_model_label(current),
        group: "Current".into(),
        provider: provider.to_string(),
    }];
    out.extend(opts);
    out
}

/// `flagsForModel`.
pub fn flags_for_model(models: &[CatalogModel], model_ref: &str) -> Vec<FlagDimension> {
    models
        .iter()
        .find(|m| m.model_ref == model_ref)
        .map(|m| m.flags.clone())
        .unwrap_or_default()
}

/// `modelProvider`: the provider slug of a ref, Claude Code when unqualified.
pub fn model_provider(model_ref: &str) -> &str {
    match model_ref.find('/') {
        Some(i) if i > 0 => &model_ref[..i],
        _ => "claude-code",
    }
}

/// `providerEnabled`: on unless listed in Settings → Providers.
pub fn provider_enabled(disabled: &[String], slug: &str) -> bool {
    !disabled.iter().any(|d| d == slug)
}

/// `chatSteerFollowUp`: text follow-ups inject into the live turn instead of queueing. Images
/// still queue; MA and Loop-mode conductor chats keep the queue.
pub fn chat_steer_follow_up(busy: bool, mode: &str, has_attachments: bool, code: &str) -> bool {
    busy && !has_attachments && code != "MA" && mode != "loop"
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_services::api::{FlagOption, ModelChoice};

    #[test]
    fn flatten_keeps_group_and_provider() {
        let groups = vec![ModelGroup {
            provider: "claude-code".into(),
            label: "Claude Code".into(),
            models: vec![ModelChoice {
                model_ref: "claude-code/a".into(),
                label: "A".into(),
                ..Default::default()
            }],
        }];
        let flat = flatten_model_catalog(&groups);
        assert_eq!(flat[0].model_ref, "claude-code/a");
        assert_eq!(flat[0].group, "Claude Code");
        assert_eq!(flat[0].provider, "claude-code");
    }

    #[test]
    fn normalize_fills_and_drops() {
        let dims = vec![FlagDimension {
            id: "effort".into(),
            label: "Effort".into(),
            options: vec![
                FlagOption {
                    value: "low".into(),
                    label: "Low".into(),
                    default: false,
                },
                FlagOption {
                    value: "high".into(),
                    label: "High".into(),
                    default: true,
                },
            ],
        }];
        let mut cur = FlagMap::new();
        cur.insert("stale".into(), "x".into());
        let got = normalize_flags(&dims, Some(&cur));
        assert_eq!(got.get("effort").map(String::as_str), Some("high"));
        assert!(!got.contains_key("stale"));
        cur.insert("effort".into(), "low".into());
        assert_eq!(
            normalize_flags(&dims, Some(&cur))
                .get("effort")
                .map(String::as_str),
            Some("low")
        );
        cur.insert("effort".into(), "bogus".into());
        assert_eq!(
            normalize_flags(&dims, Some(&cur))
                .get("effort")
                .map(String::as_str),
            Some("high")
        );
    }

    #[test]
    fn options_keep_an_unlisted_current_ref() {
        let opts = model_options_for(&[], "codex/gpt-5-codex");
        assert_eq!(opts[0].label, "GPT-5 Codex");
        assert_eq!(opts[0].group, "Current");
        assert_eq!(opts[0].provider, "codex");
        assert_eq!(short_model_label("claude-code/opus"), "opus");
        assert_eq!(short_model_label("openai/gpt-4.1"), "GPT-4.1");
        assert!(model_options_for(&[], "").is_empty());
    }

    // providers.test.ts
    #[test]
    fn provider_rules() {
        assert!(provider_enabled(&[], "claude-code"));
        assert!(!provider_enabled(&["cursor".into()], "cursor"));
        assert!(provider_enabled(&["cursor".into()], "claude-code"));
        assert_eq!(model_provider("cursor/composer-2.5"), "cursor");
        assert_eq!(model_provider("sonnet"), "claude-code");
        assert_eq!(model_provider(""), "claude-code");
        assert!(chat_steer_follow_up(true, "agent", false, "PA"));
        assert!(chat_steer_follow_up(true, "ask", false, "PA"));
        assert!(chat_steer_follow_up(true, "plan", false, "PA2"));
        assert!(!chat_steer_follow_up(true, "agent", true, "PA"));
        assert!(!chat_steer_follow_up(false, "agent", false, "PA"));
        assert!(!chat_steer_follow_up(true, "loop", false, "MA"));
        assert!(!chat_steer_follow_up(true, "loop", false, "PA"));
    }
}
