//! The Codex branch of Go `app/model_catalog.go`: live `model/list` entries
//! to picker rows with effort, long-context and Fast dimensions.

use recur_agent::{FlagDimension, FlagOption, ModelChoice};
use recur_domain::{ModelRef, PROVIDER_CODEX};

use crate::types::Model;

/// Visible rows for `model/list` entries (hidden and id-less ones dropped).
pub fn catalog_models(models: &[Model]) -> Vec<ModelChoice> {
    models
        .iter()
        .filter(|m| !m.id.trim().is_empty() && !m.hidden)
        .map(|m| ModelChoice {
            model_ref: ModelRef::new(PROVIDER_CODEX, &m.id).to_string(),
            label: friendly_model_label(&m.display_name, &m.id),
            flags: model_flags(m),
            context_window: 0,
        })
        .collect()
}

/// Effort, then the 272K/1M context for GPT-5.6 / GPT-6, then Fast.
pub fn model_flags(m: &Model) -> Vec<FlagDimension> {
    let mut out = effort_flags(m);
    if gpt56_family(&m.id) {
        out.push(long_context_dim());
    }
    out.extend(speed_flag(m));
    out
}

/// Opt-in long context for GPT-5.6 / GPT-6. The default stays 272K; larger
/// prompts are billed at 2× input / 1.5× output.
// simplify: Astra's API ceiling is 1.05M; Recur's 1M flag matches GPT-5.6.
fn long_context_dim() -> FlagDimension {
    FlagDimension {
        id: "context".into(),
        label: "Context".into(),
        options: vec![
            FlagOption {
                value: "272k".into(),
                label: "272K".into(),
                default: true,
            },
            FlagOption {
                value: "1m".into(),
                label: "1M".into(),
                default: false,
            },
        ],
    }
}

fn normalized(id: &str) -> String {
    id.trim().to_lowercase().replace('.', "-")
}

fn gpt6_family(id: &str) -> bool {
    normalized(id).starts_with("gpt-6")
}

fn gpt56_family(id: &str) -> bool {
    normalized(id).contains("gpt-5-6") || gpt6_family(id)
}

/// Picker labels in Claude's register: `GPT-5.6-Sol` / `gpt-5.6-sol` →
/// `GPT-5.6 Sol`.
pub fn friendly_model_label(display_name: &str, id: &str) -> String {
    let s = display_name.trim();
    if !s.is_empty() {
        return space_codename(s);
    }
    space_codename(&pretty_slug(id))
}

/// Swaps the final hyphen before a letter-only codename for a space;
/// version hyphens stay.
fn space_codename(s: &str) -> String {
    match s.rfind('-') {
        Some(i) if i > 0 => {
            let tail = &s[i + 1..];
            if tail.is_empty() || tail.contains(|c: char| c.is_ascii_digit() || c == '.') {
                return s.to_owned();
            }
            format!("{} {}", &s[..i], upper_first(tail))
        }
        _ => s.to_owned(),
    }
}

fn pretty_slug(id: &str) -> String {
    let id = id.trim();
    if id.is_empty() {
        return String::new();
    }
    match id.get(..4) {
        Some(head) if head.eq_ignore_ascii_case("gpt-") => format!("GPT-{}", &id[4..]),
        _ => upper_first(id),
    }
}

/// Go `unicode.ToUpper` on the first rune (a simple, one-rune mapping).
fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    let Some(first) = c.next() else {
        return String::new();
    };
    let mut up = first.to_uppercase();
    let first = match (up.next(), up.next()) {
        (Some(u), None) => u,
        _ => first,
    };
    let mut out = String::with_capacity(s.len());
    out.push(first);
    out.push_str(c.as_str());
    out
}

fn effort_flags(m: &Model) -> Vec<FlagDimension> {
    if m.supported_reasoning_effort.is_empty() {
        return Vec::new();
    }
    vec![FlagDimension {
        id: "effort".into(),
        label: "Effort".into(),
        options: m
            .supported_reasoning_effort
            .iter()
            .map(|e| FlagOption {
                value: e.reasoning_effort.clone(),
                label: friendly_effort_label(&e.reasoning_effort),
                default: e.reasoning_effort == m.default_reasoning_effort,
            })
            .collect(),
    }]
}

/// Mirrors Claude's effort chips.
pub fn friendly_effort_label(effort: &str) -> String {
    match effort.trim().to_lowercase().as_str() {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra High",
        "max" => "Max",
        "ultra" => "Ultra",
        "" => "",
        _ => return upper_first(effort),
    }
    .to_owned()
}

/// A Fast On/Off flag over the model's boost tier. The On value is the real
/// tier id so `turn/start` never invents an unsupported one.
fn speed_flag(m: &Model) -> Option<FlagDimension> {
    let boost = boost_tier_id(m);
    if boost.is_empty() {
        return None;
    }
    let def = m.default_service_tier.trim();
    let on = !def.is_empty() && def.to_lowercase() == boost.to_lowercase();
    Some(FlagDimension {
        id: "fast".into(),
        label: "Fast".into(),
        options: vec![
            FlagOption {
                value: boost.to_owned(),
                label: "On".into(),
                default: on,
            },
            FlagOption {
                value: "false".into(),
                label: "Off".into(),
                default: !on,
            },
        ],
    })
}

/// The Fast/priority-style tier id; empty when only standard/default (or
/// nothing) is advertised.
fn boost_tier_id(m: &Model) -> &str {
    let mut fallback = "";
    for t in &m.service_tiers {
        let id = t.id.trim();
        if id.is_empty() {
            continue;
        }
        match id.to_lowercase().as_str() {
            "standard" | "default" => {}
            "priority" | "fast" => return id,
            _ if fallback.is_empty() => fallback = id,
            _ => {}
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use crate::types::{ReasoningEffort, ServiceTier};

    use super::*;

    fn efforts(v: &[&str]) -> Vec<ReasoningEffort> {
        v.iter()
            .map(|e| ReasoningEffort {
                reasoning_effort: (*e).into(),
                description: String::new(),
            })
            .collect()
    }

    #[test]
    fn labels() {
        for (display, id, want) in [
            ("GPT-5.6-Sol", "gpt-5.6-sol", "GPT-5.6 Sol"),
            ("GPT-5.6-Luna", "", "GPT-5.6 Luna"),
            ("", "gpt-5.6-terra", "GPT-5.6 Terra"),
            ("", "gpt-6-astra", "GPT-6 Astra"),
            ("GPT-6-Astra", "", "GPT-6 Astra"),
            ("", "o3", "O3"),
        ] {
            assert_eq!(
                friendly_model_label(display, id),
                want,
                "{display:?} {id:?}"
            );
        }
        for (input, want) in [
            ("xhigh", "Extra High"),
            ("ultra", "Ultra"),
            ("max", "Max"),
            ("turbo", "Turbo"),
        ] {
            assert_eq!(friendly_effort_label(input), want);
        }
    }

    #[test]
    fn friendly_rows_with_all_flags() {
        let out = catalog_models(&[Model {
            id: "gpt-5.6-luna".into(),
            display_name: "GPT-5.6-Luna".into(),
            default_reasoning_effort: "medium".into(),
            supported_reasoning_effort: efforts(&["low", "medium", "xhigh"]),
            default_service_tier: "priority".into(),
            service_tiers: vec![ServiceTier {
                id: "priority".into(),
                name: "Fast".into(),
                description: "1.5x speed".into(),
            }],
            ..Default::default()
        }]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            (out[0].model_ref.as_str(), out[0].label.as_str()),
            ("codex/gpt-5.6-luna", "GPT-5.6 Luna")
        );
        let ids: Vec<_> = out[0].flags.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["effort", "context", "fast"]);
        let labels: Vec<_> = out[0].flags[0]
            .options
            .iter()
            .map(|o| o.label.as_str())
            .collect();
        assert_eq!(labels, ["Low", "Medium", "Extra High"]);
        assert!(out[0].flags[0].options[1].default);
        let ctx = &out[0].flags[1];
        assert_eq!(ctx.label, "Context");
        assert_eq!(
            ctx.options
                .iter()
                .map(|o| (o.value.as_str(), o.default))
                .collect::<Vec<_>>(),
            [("272k", true), ("1m", false)]
        );
        let fast = &out[0].flags[2];
        assert_eq!(fast.label, "Fast");
        assert_eq!(
            fast.options
                .iter()
                .map(|o| (o.value.as_str(), o.label.as_str(), o.default))
                .collect::<Vec<_>>(),
            [("priority", "On", true), ("false", "Off", false)]
        );
    }

    #[test]
    fn shows_gpt6_astra_and_hides_hidden() {
        let out = catalog_models(&[
            Model {
                id: "gpt-5.4-mini".into(),
                hidden: true,
                display_name: "GPT-5.4 Mini".into(),
                ..Default::default()
            },
            Model {
                id: "gpt-6-astra".into(),
                display_name: "GPT-6-Astra".into(),
                default_reasoning_effort: "high".into(),
                supported_reasoning_effort: efforts(&["high"]),
                ..Default::default()
            },
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            (out[0].model_ref.as_str(), out[0].label.as_str()),
            ("codex/gpt-6-astra", "GPT-6 Astra")
        );
        let ctx = out[0].flags.iter().find(|f| f.id == "context").unwrap();
        assert_eq!(ctx.options[0].value, "272k");
        assert!(ctx.options[0].default);
        assert_eq!(ctx.options[1].value, "1m");
    }

    #[test]
    fn speed_flag_omitted_without_tiers() {
        let out = catalog_models(&[Model {
            id: "gpt-5.4-mini".into(),
            display_name: "GPT-5.4 Mini".into(),
            default_reasoning_effort: "medium".into(),
            supported_reasoning_effort: efforts(&["low", "medium"]),
            ..Default::default()
        }]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].flags.len(), 1);
        assert_eq!(out[0].flags[0].id, "effort");
        let standard = Model {
            id: "x".into(),
            service_tiers: vec![
                ServiceTier {
                    id: "standard".into(),
                    ..Default::default()
                },
                ServiceTier {
                    id: "flex".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(boost_tier_id(&standard), "flex");
    }
}
