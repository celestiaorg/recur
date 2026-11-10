//! The model catalog the pickers read. The list is server-owned
//! (`SettingsService::list_model_catalog`); [`AppState`](crate::AppState) keeps the shared
//! cache and its revision, these are the pure rules over it.

pub use recur_engine::view::modes::CatalogModel;
use recur_services::api::{FlagDimension, ModelGroup};

pub type FlagMap = recur_domain::Flags;

const OPENAI: &str = "OpenAI";

/// `flattenModelCatalog`. ChatGPT and Codex share one OpenAI section at the first one's place
/// (with an `openai` endpoint group); refs still name the runtime, and labels say which when
/// more than one is listed.
pub fn flatten_model_catalog(groups: &[ModelGroup]) -> Vec<CatalogModel> {
    let models: Vec<CatalogModel> = groups
        .iter()
        .flat_map(|g| {
            let group = if g.provider == "chatgpt" || g.provider == "codex" {
                OPENAI.to_string()
            } else {
                g.label.clone()
            };
            g.models.iter().map(move |m| CatalogModel {
                r#ref: m.model_ref.clone(),
                label: m.label.clone(),
                group: group.clone(),
                provider: g.provider.clone(),
                flags: m.flags.clone(),
                harnesses: g.harnesses.clone(),
            })
        })
        .collect();
    let Some(first) = models.iter().position(|m| m.group == OPENAI) else {
        return models;
    };
    let (openai, rest): (Vec<CatalogModel>, Vec<CatalogModel>) =
        models.into_iter().partition(|m| m.group == OPENAI);
    let multiple = openai.iter().any(|m| m.provider != openai[0].provider);
    let openai = openai.into_iter().map(|mut m| {
        if multiple {
            let runtime = match m.provider.as_str() {
                "chatgpt" => "ChatGPT",
                "codex" => "Codex",
                _ => "API",
            };
            m.label = format!("{} ({runtime})", m.label);
        }
        m
    });
    let mut out: Vec<CatalogModel> = rest[..first].to_vec();
    out.extend(openai);
    out.extend(rest[first..].iter().cloned());
    out
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
    pub harnesses: Vec<recur_agent::catalog::HarnessChoice>,
}

pub fn catalog_select_options(models: &[CatalogModel]) -> Vec<ModelSelectOption> {
    models
        .iter()
        .map(|m| ModelSelectOption {
            value: m.r#ref.clone(),
            label: m.label.clone(),
            group: m.group.clone(),
            provider: m.provider.clone(),
            harnesses: m.harnesses.clone(),
        })
        .collect()
}

/// `shortModelLabel`: a qualified ref's label when the catalog row is not available yet.
pub fn short_model_label(model_ref: &str) -> String {
    let model_ref = recur_domain::base_model_ref(model_ref);
    if model_ref.is_empty() {
        return String::new();
    }
    let (provider, id) = match model_ref.find('/') {
        Some(i) => (&model_ref[..i], &model_ref[i + 1..]),
        None => ("", model_ref),
    };
    if provider == "codex" || provider == "openai" || provider == "chatgpt" {
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
    let base = recur_domain::base_model_ref(current);
    if base.is_empty() || opts.iter().any(|o| o.value == base) {
        return opts;
    }
    let provider = base.find('/').map_or("", |i| &base[..i]);
    let mut out = vec![ModelSelectOption {
        value: base.to_string(),
        label: short_model_label(base),
        group: "Current".into(),
        provider: provider.to_string(),
        harnesses: Vec::new(),
    }];
    out.extend(opts);
    out
}

/// `flagsForModel`.
pub fn flags_for_model(models: &[CatalogModel], model_ref: &str) -> Vec<FlagDimension> {
    let base = recur_domain::base_model_ref(model_ref);
    models
        .iter()
        .find(|m| m.r#ref == base)
        .map(|m| m.flags.clone())
        .unwrap_or_default()
}

/// `modelProvider`: the provider slug of a ref, Claude Code when unqualified.
pub fn model_provider(model_ref: &str) -> &str {
    let model_ref = recur_domain::base_model_ref(model_ref);
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

    fn group(provider: &str, label: &str, refs: &[(&str, &str)]) -> ModelGroup {
        ModelGroup {
            provider: provider.into(),
            label: label.into(),
            models: refs
                .iter()
                .map(|(r, l)| ModelChoice {
                    model_ref: (*r).into(),
                    label: (*l).into(),
                    ..Default::default()
                })
                .collect(),
            harnesses: Vec::new(),
        }
    }

    #[test]
    fn offers_one_openai_section_while_preserving_the_selected_model_runtime() {
        let models = flatten_model_catalog(&[
            group("claude-code", "Claude Code", &[("claude-code/a", "A")]),
            group("codex", "Codex", &[("codex/shared", "Shared")]),
            group("local", "Local", &[("local/q", "Q")]),
            group("chatgpt", "ChatGPT", &[("chatgpt/shared", "Shared")]),
            group("openai", "OpenAI", &[("openai/api", "API model")]),
        ]);
        let groups: Vec<&str> = models.iter().map(|m| m.group.as_str()).collect();
        assert_eq!(
            groups,
            ["Claude Code", "OpenAI", "OpenAI", "OpenAI", "Local"]
        );
        let rows: Vec<(&str, &str, &str)> = models[1..4]
            .iter()
            .map(|m| (m.r#ref.as_str(), m.provider.as_str(), m.label.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                ("codex/shared", "codex", "Shared (Codex)"),
                ("chatgpt/shared", "chatgpt", "Shared (ChatGPT)"),
                ("openai/api", "openai", "API model (API)"),
            ]
        );
    }

    #[test]
    fn keeps_model_labels_short_when_only_one_openai_runtime_is_available() {
        let m =
            &flatten_model_catalog(&[group("chatgpt", "ChatGPT", &[("chatgpt/gpt", "GPT")])])[0];
        assert_eq!(
            (
                m.r#ref.as_str(),
                m.group.as_str(),
                m.label.as_str(),
                m.provider.as_str()
            ),
            ("chatgpt/gpt", "OpenAI", "GPT", "chatgpt")
        );
    }

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
            harnesses: Vec::new(),
        }];
        let flat = flatten_model_catalog(&groups);
        assert_eq!(flat[0].r#ref, "claude-code/a");
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
    fn harness_refs_use_the_base_model() {
        let model = CatalogModel {
            r#ref: "openrouter/gpt-5".into(),
            label: "GPT-5".into(),
            group: "OpenRouter".into(),
            provider: "openrouter".into(),
            flags: vec![FlagDimension {
                id: "effort".into(),
                label: "Effort".into(),
                options: vec![FlagOption {
                    value: "high".into(),
                    label: "High".into(),
                    default: true,
                }],
            }],
            harnesses: Vec::new(),
        };
        let models = vec![model];
        assert_eq!(
            flags_for_model(&models, "opencode/openrouter/gpt-5").len(),
            1
        );
        let opts = model_options_for(&models, "opencode/openrouter/gpt-5");
        assert_eq!(opts[0].value, "openrouter/gpt-5");
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

    #[test]
    fn provider_rules() {
        assert!(provider_enabled(&[], "claude-code"));
        assert!(!provider_enabled(&["cursor".into()], "cursor"));
        assert!(provider_enabled(&["cursor".into()], "claude-code"));
        assert_eq!(model_provider("cursor/composer-2.5"), "cursor");
        assert_eq!(model_provider("sonnet"), "claude-code");
        assert_eq!(model_provider(""), "claude-code");
        assert_eq!(model_provider("opencode/openrouter/gpt-5"), "openrouter");
        assert!(chat_steer_follow_up(true, "agent", false, "PA"));
        assert!(chat_steer_follow_up(true, "ask", false, "PA"));
        assert!(chat_steer_follow_up(true, "plan", false, "PA2"));
        assert!(!chat_steer_follow_up(true, "agent", true, "PA"));
        assert!(!chat_steer_follow_up(false, "agent", false, "PA"));
        assert!(!chat_steer_follow_up(true, "loop", false, "MA"));
        assert!(!chat_steer_follow_up(true, "loop", false, "PA"));
    }
}
