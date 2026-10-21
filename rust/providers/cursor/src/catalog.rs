//! Cursor's picker rows from live ListModels entries (the Cursor branch of
//! Go `app/model_catalog.go`).

use std::collections::BTreeMap;

use recur_agent::{FlagDimension, FlagOption, ModelChoice};
use recur_domain::{ModelRef, PROVIDER_CURSOR, parse_model_ref};

use crate::client::{Model, ModelParameterDefinition, ModelVariant};
use crate::context::{VENDOR_ORDER, base_context_window, vendor};

/// Catalog rows for ListModels entries. Drops id "default" (a second "Auto"
/// for the account default: Recur always picks an explicit model) and puts
/// house models (Composer, Auto, Grok) first, then the rest by lab.
pub fn cursor_catalog_models(models: &[Model]) -> Vec<ModelChoice> {
    let out: Vec<ModelChoice> = models
        .iter()
        .filter(|m| m.id != "default")
        .map(cursor_model_choice)
        .collect();
    order_cursor_models(out)
}

fn order_cursor_models(models: Vec<ModelChoice>) -> Vec<ModelChoice> {
    if models.len() < 2 {
        return models;
    }
    let mut buckets: BTreeMap<&str, Vec<ModelChoice>> = BTreeMap::new();
    for m in models {
        let v = vendor(&parse_model_ref(&m.model_ref).id);
        buckets.entry(v).or_default().push(m);
    }
    VENDOR_ORDER
        .iter()
        .flat_map(|v| buckets.remove(v).unwrap_or_default())
        .collect()
}

/// One ListModels entry as a row. Parameters become flag dimensions (the
/// default variant marks defaults); the context window comes from Recur's
/// table because ListModels does not advertise limits.
pub fn cursor_model_choice(m: &Model) -> ModelChoice {
    ModelChoice {
        model_ref: ModelRef::new(PROVIDER_CURSOR, &m.id).to_string(),
        label: if m.display_name.is_empty() {
            m.id.clone()
        } else {
            m.display_name.clone()
        },
        flags: parameters_to_flags(&m.parameters, &m.variants),
        context_window: base_context_window(&m.id),
    }
}

pub(crate) fn parameters_to_flags(
    params: &[ModelParameterDefinition],
    variants: &[ModelVariant],
) -> Vec<FlagDimension> {
    if params.is_empty() {
        return Vec::new();
    }
    let defaults: BTreeMap<&str, &str> = variants
        .iter()
        .find(|v| v.is_default)
        .map(|v| v.params.iter().map(|p| (p.id.as_str(), p.value.as_str())).collect())
        .unwrap_or_default();

    params
        .iter()
        .filter(|p| !p.values.is_empty())
        .map(|p| {
            let def = defaults.get(p.id.as_str()).copied();
            let opts = p
                .values
                .iter()
                .enumerate()
                .map(|(i, v)| FlagOption {
                    value: v.value.clone(),
                    label: if v.display_name.is_empty() {
                        v.value.clone()
                    } else {
                        v.display_name.clone()
                    },
                    // SDK: omitted params use the first allowed value.
                    default: def.map_or(i == 0, |d| v.value == d),
                })
                .collect();
            FlagDimension {
                id: p.id.clone(),
                label: if p.display_name.is_empty() {
                    p.id.clone()
                } else {
                    p.display_name.clone()
                },
                options: normalize_bool_flag_options(opts, def),
            }
        })
        .collect()
}

/// true/false params (fast, thinking, …) as On/Off with On first; wire
/// values stay "true"/"false".
fn normalize_bool_flag_options(opts: Vec<FlagOption>, def: Option<&str>) -> Vec<FlagOption> {
    if opts.len() != 2
        || !opts.iter().any(|o| o.value == "true")
        || !opts.iter().any(|o| o.value == "false")
    {
        return opts;
    }
    let (on_default, off_default) = match def {
        Some(d) => (d == "true", d == "false"),
        // Keep the SDK's first-value default on its value.
        None => (opts[0].value == "true", opts[0].value == "false"),
    };
    vec![
        FlagOption {
            value: "true".into(),
            label: "On".into(),
            default: on_default,
        },
        FlagOption {
            value: "false".into(),
            label: "Off".into(),
            default: off_default,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ModelParameterChoice, ModelParameterValue};

    fn model(id: &str, name: &str) -> Model {
        Model {
            id: id.into(),
            display_name: name.into(),
            ..Default::default()
        }
    }

    fn choice(v: &str, name: &str) -> ModelParameterChoice {
        ModelParameterChoice {
            value: v.into(),
            display_name: name.into(),
        }
    }

    // Go TestCursorCatalogHouseModelsFirst.
    #[test]
    fn house_models_first() {
        let out = cursor_catalog_models(&[
            model("gpt-5.6-sol", "GPT-5.6 Sol"),
            model("composer-2.5", "Composer 2.5"),
            model("claude-opus-5", "Opus 5"),
            model("default", "Auto"),
            model("grok-4.6", "Grok 4.6"),
            model("gemini-3.1-pro", "Gemini 3.1 Pro"),
            model("auto-smart", "Auto"),
        ]);
        let got: Vec<&str> = out.iter().map(|m| m.model_ref.as_str()).collect();
        assert_eq!(
            got,
            [
                "cursor/composer-2.5",
                "cursor/grok-4.6",
                "cursor/auto-smart",
                "cursor/claude-opus-5",
                "cursor/gpt-5.6-sol",
                "cursor/gemini-3.1-pro",
            ]
        );
    }

    // Go TestCursorCatalogOmitsDefault.
    #[test]
    fn omits_default() {
        let out = cursor_catalog_models(&[
            model("auto-smart", "Auto"),
            model("default", "Auto"),
            model("composer-2.5", "Composer 2.5"),
            model("grok-4.6", "Grok 4.6"),
        ]);
        assert_eq!(out.len(), 3);
        assert_eq!((out[0].model_ref.as_str(), out[0].label.as_str()), ("cursor/auto-smart", "Auto"));
        assert_eq!(out[1].model_ref, "cursor/composer-2.5");
        assert_eq!(out[1].context_window, 200_000);
        assert_eq!((out[2].model_ref.as_str(), out[2].label.as_str()), ("cursor/grok-4.6", "Grok 4.6"));
        assert_eq!(out[2].context_window, 500_000);
    }

    // Go TestCursorParametersToFlags.
    #[test]
    fn parameters_to_flag_dims() {
        let flags = parameters_to_flags(
            &[ModelParameterDefinition {
                id: "fast".into(),
                display_name: "Fast".into(),
                values: vec![choice("false", ""), choice("true", "Fast")],
            }],
            &[ModelVariant {
                display_name: "Fast".into(),
                is_default: true,
                params: vec![ModelParameterValue {
                    id: "fast".into(),
                    value: "true".into(),
                }],
                ..Default::default()
            }],
        );
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].id, "fast");
        let o = &flags[0].options;
        assert_eq!((o[0].value.as_str(), o[0].label.as_str(), o[0].default), ("true", "On", true));
        assert_eq!((o[1].value.as_str(), o[1].label.as_str(), o[1].default), ("false", "Off", false));
    }

    // Go TestCursorThinkingBoolLabels.
    #[test]
    fn thinking_bool_labels() {
        let flags = parameters_to_flags(
            &[ModelParameterDefinition {
                id: "thinking".into(),
                values: vec![choice("true", ""), choice("false", "")],
                ..Default::default()
            }],
            &[],
        );
        let o = &flags[0].options;
        assert_eq!((o[0].label.as_str(), o[0].value.as_str()), ("On", "true"));
        assert_eq!((o[1].label.as_str(), o[1].value.as_str()), ("Off", "false"));
        assert!(o[0].default);
    }

    // Go TestCursorParametersDefaultToFirstWhenNoVariant.
    #[test]
    fn default_to_first_without_variant() {
        let flags = parameters_to_flags(
            &[ModelParameterDefinition {
                id: "effort".into(),
                values: vec![choice("low", ""), choice("high", "High")],
                ..Default::default()
            }],
            &[],
        );
        assert!(flags[0].options[0].default && !flags[0].options[1].default);
        assert_eq!(flags[0].options[1].label, "High");
        assert_eq!(flags[0].label, "effort");
    }

    // Go TestCursorPassesThroughListModelsContext.
    #[test]
    fn passes_through_list_models_context() {
        let got = cursor_model_choice(&Model {
            id: "gpt-5.6-sol".into(),
            display_name: "GPT-5.6 Sol".into(),
            parameters: vec![ModelParameterDefinition {
                id: "context".into(),
                display_name: "Context".into(),
                values: vec![choice("272k", "272K"), choice("1m", "1M")],
            }],
            variants: vec![ModelVariant {
                display_name: "GPT-5.6 Sol".into(),
                is_default: true,
                params: vec![ModelParameterValue {
                    id: "context".into(),
                    value: "1m".into(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        let ctx = got.flags.iter().find(|f| f.id == "context").unwrap();
        let vals: Vec<&str> = ctx.options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(vals, ["272k", "1m"]);
        assert!(!ctx.options[0].default && ctx.options[1].default);
    }

    // Go TestCursorModelChoiceDoesNotInventGPTContext.
    #[test]
    fn does_not_invent_context() {
        let got = cursor_model_choice(&model("gpt-5.6-sol", "GPT-5.6 Sol"));
        assert!(got.flags.iter().all(|f| f.id != "context"));
        assert_eq!(got.context_window, 272_000);
    }
}
