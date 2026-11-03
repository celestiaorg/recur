//! Model picker types. Each provider lists its own models through
//! [`crate::Provider::models`]; the services layer merges them into one grouped
//! list.

use serde::{Deserialize, Serialize};

use crate::Flags;

/// One selectable value inside a [`FlagDimension`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlagOption {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "is_false")]
    pub default: bool,
}

/// One configurable axis on a model (Effort, Context, Fast, …).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlagDimension {
    pub id: String,
    pub label: String,
    pub options: Vec<FlagOption>,
}

/// One selectable model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelChoice {
    /// The fully qualified value to store, e.g. `cursor/composer-2.5`.
    #[serde(rename = "ref")]
    pub model_ref: String,
    pub label: String,
    /// The per-model dimensions the second picker shows; empty hides it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<FlagDimension>,
    /// The default context limit in tokens; 0 = use the context flag.
    #[serde(skip_serializing_if = "is_zero")]
    pub context_window: i64,
}

/// One provider's section of the picker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelGroup {
    /// The provider slug, e.g. `cursor`.
    pub provider: String,
    pub label: String,
    /// In display order.
    pub models: Vec<ModelChoice>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl FlagDimension {
    /// The option marked default, else the first option; empty when there
    /// are no options.
    pub fn default_value(&self) -> &str {
        self.options
            .iter()
            .find(|o| o.default)
            .or(self.options.first())
            .map_or("", |o| o.value.as_str())
    }
}

/// Each dimension's default value; dimensions without one are omitted.
pub fn default_flags_from_dims(dims: &[FlagDimension]) -> Flags {
    dims.iter()
        .filter(|d| !d.default_value().is_empty())
        .map(|d| (d.id.clone(), d.default_value().to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(v: &str, default: bool) -> FlagOption {
        FlagOption {
            value: v.into(),
            label: v.to_uppercase(),
            default,
        }
    }

    #[test]
    fn json_matches_go() {
        let g = ModelGroup {
            provider: "claude-code".into(),
            label: "Claude Code".into(),
            models: vec![ModelChoice {
                model_ref: "claude-code/claude-opus-4-6".into(),
                label: "Opus 4.6".into(),
                flags: vec![FlagDimension {
                    id: "context".into(),
                    label: "Context".into(),
                    options: vec![opt("200k", true), opt("1m", false)],
                }],
                context_window: 0,
            }],
        };
        assert_eq!(
            serde_json::to_string(&g).unwrap(),
            r#"{"provider":"claude-code","label":"Claude Code","models":[{"ref":"claude-code/claude-opus-4-6","label":"Opus 4.6","flags":[{"id":"context","label":"Context","options":[{"value":"200k","label":"200K","default":true},{"value":"1m","label":"1M"}]}]}]}"#
        );
    }

    #[test]
    fn default_flags() {
        let dims = [
            FlagDimension {
                id: "effort".into(),
                options: vec![opt("low", false), opt("high", true)],
                ..Default::default()
            },
            FlagDimension {
                id: "fast".into(),
                options: vec![opt("true", false), opt("false", false)],
                ..Default::default()
            },
            FlagDimension {
                id: "empty".into(),
                ..Default::default()
            },
        ];
        let got = default_flags_from_dims(&dims);
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got["effort"].as_str(), got["fast"].as_str()),
            ("high", "true")
        );
    }
}
