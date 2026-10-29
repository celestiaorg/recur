//! The compatible branch of the model catalogue and its flag
//! dimensions.

use recur_agent::{FlagDimension, FlagOption, ModelChoice};
use recur_domain::{CompatibleFlagDim, CompatibleProvider, ModelRef};

/// One picker choice per declared model with an id, labelled by its label or
/// id, refs qualified with the provider slug.
pub fn compatible_catalog_models(p: &CompatibleProvider) -> Vec<ModelChoice> {
    p.models
        .iter()
        .filter(|m| !m.id.is_empty())
        .map(|m| ModelChoice {
            model_ref: ModelRef::new(&p.id, &m.id).to_string(),
            label: if m.label.is_empty() {
                m.id.clone()
            } else {
                m.label.clone()
            },
            flags: compatible_flag_dims(&m.flags),
            context_window: m.context_window,
        })
        .collect()
}

/// Picker dimensions without their wire paths.
pub fn compatible_flag_dims(dims: &[CompatibleFlagDim]) -> Vec<FlagDimension> {
    dims.iter()
        .map(|d| FlagDimension {
            id: d.id.clone(),
            label: d.label.clone(),
            options: d
                .options
                .iter()
                .map(|o| FlagOption {
                    value: o.value.clone(),
                    label: o.label.clone(),
                    default: o.default,
                })
                .collect(),
        })
        .collect()
}
