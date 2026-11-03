//! User-defined OpenAI-compatible endpoints Recur drives via
//! POST {baseURL}/v1/responses. Each has its own slug (the ModelRef provider
//! prefix), models, and flag dimensions.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::Settings;
use crate::model::{
    PROVIDER_ANTHROPIC, PROVIDER_CHATGPT, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX, PROVIDER_CURSOR,
    PROVIDER_GEMINI, PROVIDER_OLLAMA, provider_label,
};
use crate::serde_util::{is_false, is_zero};
use crate::text::kebab_slug;

/// Slugs that collide with Recur's built-in harnesses or reserved session
/// identifiers. "openai" is allowed: users add it as a compatible endpoint.
const RESERVED_PROVIDER_SLUGS: [&str; 7] = [
    PROVIDER_CLAUDE_CODE,
    PROVIDER_CURSOR,
    PROVIDER_CODEX,
    PROVIDER_CHATGPT,
    PROVIDER_ANTHROPIC,
    PROVIDER_GEMINI,
    PROVIDER_OLLAMA,
];

/// Allowlisted JSON paths a CompatibleFlagDim.wire may set on the Responses
/// body.
const FLAG_WIRES: [&str; 5] = [
    "reasoning.effort",
    "reasoning.summary",
    "text.verbosity",
    "temperature",
    "max_output_tokens",
];

/// One user-defined OpenAI-compatible LLM endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompatibleProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<CompatibleModel>,
}

/// One model the user declared this endpoint supports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompatibleModel {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub context_window: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<CompatibleFlagDim>,
}

/// One picker axis on a compatible model, mapped onto a Responses request
/// field via Wire (e.g. "reasoning.effort").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatibleFlagDim {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub wire: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<CompatibleFlagOption>,
}

/// One selectable value inside a CompatibleFlagDim.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatibleFlagOption {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "is_false")]
    pub default: bool,
}

/// Why a compatible provider was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompatibleError {
    #[error("compatible provider id {0:?} must be kebab-case (a-z, digits, hyphen)")]
    InvalidId(String),
    #[error("compatible provider id {0:?} is reserved")]
    ReservedId(String),
    #[error("duplicate compatible provider {0:?}")]
    DuplicateProvider(String),
    #[error("base URL is required")]
    MissingBaseUrl,
    #[error("invalid base URL {0:?}")]
    InvalidBaseUrl(String),
    #[error("base URL must be http or https")]
    BaseUrlScheme,
    #[error("compatible provider {provider:?}: duplicate model id {model:?}")]
    DuplicateModel { provider: String, model: String },
    #[error("compatible provider {provider:?}: duplicate flag id {flag:?}")]
    DuplicateFlag { provider: String, flag: String },
    #[error("compatible provider {provider:?}: flag {flag:?}: duplicate flag option {option:?}")]
    DuplicateFlagOption {
        provider: String,
        flag: String,
        option: String,
    },
    #[error("compatible provider {0:?} has no models")]
    NoModels(String),
}

/// Reports whether path is an allowlisted Responses JSON path (empty is
/// allowed: no wire).
pub fn valid_compatible_wire(path: &str) -> bool {
    let path = path.trim();
    path.is_empty() || FLAG_WIRES.contains(&path)
}

/// Stock flag dimensions for the Settings UI. Unknown names yield no flags.
pub fn compatible_flag_preset(name: &str) -> Vec<CompatibleFlagDim> {
    let opt = |value: &str, label: &str, default: bool| CompatibleFlagOption {
        value: value.to_owned(),
        label: label.to_owned(),
        default,
    };
    let effort = CompatibleFlagDim {
        id: "effort".into(),
        label: "Effort".into(),
        wire: "reasoning.effort".into(),
        options: vec![
            opt("none", "None", false),
            opt("low", "Low", false),
            opt("medium", "Medium", true),
            opt("high", "High", false),
            opt("xhigh", "Extra High", false),
        ],
    };
    let verbosity = CompatibleFlagDim {
        id: "verbosity".into(),
        label: "Verbosity".into(),
        wire: "text.verbosity".into(),
        options: vec![
            opt("low", "Low", false),
            opt("medium", "Medium", true),
            opt("high", "High", false),
        ],
    };
    match name.trim().to_lowercase().as_str() {
        "openai" | "openai-reasoning" | "reasoning" => vec![effort, verbosity],
        "effort" => vec![effort],
        _ => Vec::new(),
    }
}

impl CompatibleProvider {
    /// Whether the provider has a base URL and at least one model. An API
    /// key is optional (local servers).
    pub fn configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.models.is_empty()
    }

    /// The declared model with this id.
    pub fn model_by_id(&self, id: &str) -> Option<&CompatibleModel> {
        let id = id.trim();
        self.models.iter().find(|m| m.id == id)
    }
}

impl Settings {
    /// The named compatible provider.
    pub fn compatible_by_id(&self, id: &str) -> Option<&CompatibleProvider> {
        let id = id.trim();
        if id.is_empty() {
            return None;
        }
        self.compatible_providers.iter().find(|p| p.id == id)
    }

    /// Whether slug names a compatible provider in Settings (configured or
    /// not). Built-in harness slugs are never compatible.
    pub fn is_compatible_slug(&self, slug: &str) -> bool {
        self.compatible_by_id(slug).is_some()
    }

    /// Providers whose tools and history Recur owns.
    pub fn uses_native_harness(&self, slug: &str) -> bool {
        slug == PROVIDER_CHATGPT || self.is_compatible_slug(slug)
    }

    /// Whether a ChatGPT account is connected with at least one model.
    pub fn chatgpt_configured(&self) -> bool {
        !self.chat_gpt_account_id.is_empty() && !self.chat_gpt_models.is_empty()
    }

    /// The ChatGPT subscription endpoint as a Responses provider.
    pub fn chatgpt_provider(&self) -> CompatibleProvider {
        CompatibleProvider {
            id: PROVIDER_CHATGPT.into(),
            name: "ChatGPT".into(),
            base_url: "https://chatgpt.com/backend-api/codex".into(),
            models: self.chat_gpt_models.clone(),
            ..Default::default()
        }
    }

    /// The human name for a provider slug, preferring a compatible
    /// provider's Name when present.
    pub fn display_label<'a>(&'a self, provider: &'a str) -> &'a str {
        match self.compatible_by_id(provider).map(|p| p.name.trim()) {
            Some(name) if !name.is_empty() => name,
            _ => provider_label(provider),
        }
    }

    /// Canonicalizes valid providers and omits invalid ones, so a corrupt
    /// stored row cannot break startup.
    pub fn normalize_compatible_providers(mut self) -> Self {
        if self.compatible_providers.is_empty() {
            return self;
        }
        let mut seen = HashSet::new();
        let mut out = Vec::with_capacity(self.compatible_providers.len());
        for p in std::mem::take(&mut self.compatible_providers) {
            if let Ok(np) = normalize_compatible_provider(p, &seen) {
                seen.insert(np.id.clone());
                out.push(np);
            }
        }
        self.compatible_providers = out;
        self
    }
}

/// The first validation error, without omitting any provider. Settings saves
/// use this so a bad edit cannot delete a saved card.
pub fn validate_compatible_providers(
    providers: &[CompatibleProvider],
) -> Result<(), CompatibleError> {
    let mut seen = HashSet::new();
    for p in providers {
        let np = normalize_compatible_provider(p.clone(), &seen)?;
        seen.insert(np.id);
    }
    Ok(())
}

/// Canonicalizes one provider: lower-case trimmed id, Name defaulting to the
/// id, base URL without a trailing slash, invalid extra-header keys dropped,
/// models/flags/options trimmed and deduped. `seen` holds ids already
/// accepted in this slice (duplicates are rejected).
pub fn normalize_compatible_provider(
    mut p: CompatibleProvider,
    seen: &HashSet<String>,
) -> Result<CompatibleProvider, CompatibleError> {
    p.id = p.id.trim().to_lowercase();
    if !kebab_slug(&p.id, 47) {
        return Err(CompatibleError::InvalidId(p.id));
    }
    if RESERVED_PROVIDER_SLUGS.contains(&p.id.as_str()) {
        return Err(CompatibleError::ReservedId(p.id));
    }
    if seen.contains(&p.id) {
        return Err(CompatibleError::DuplicateProvider(p.id));
    }
    p.name = p.name.trim().to_owned();
    if p.name.is_empty() {
        p.name = p.id.clone();
    }
    p.base_url = normalize_base_url(&p.base_url)?;
    p.api_key = p.api_key.trim().to_owned();
    p.extra_headers = clean_extra_headers(&p.extra_headers);
    unique_model_ids(&p.id, &p.models)?;
    unique_flags(&p.id, &p.models)?;
    p.models = normalize_models(std::mem::take(&mut p.models));
    if p.models.is_empty() {
        return Err(CompatibleError::NoModels(p.id));
    }
    Ok(p)
}

fn normalize_base_url(raw: &str) -> Result<String, CompatibleError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(CompatibleError::MissingBaseUrl);
    }
    let raw = raw.trim_end_matches('/');
    let url = url::Url::parse(raw)
        .ok()
        .filter(|u| u.host_str().is_some_and(|h| !h.is_empty()))
        .ok_or_else(|| CompatibleError::InvalidBaseUrl(raw.to_owned()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(CompatibleError::BaseUrlScheme);
    }
    Ok(raw.to_owned())
}

fn clean_extra_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            let k = k.trim();
            if k.is_empty() || k.contains([' ', '\t', '\r', '\n', ':']) {
                return None;
            }
            Some((k.to_owned(), v.trim().to_owned()))
        })
        .collect()
}

fn unique_model_ids(provider: &str, models: &[CompatibleModel]) -> Result<(), CompatibleError> {
    let mut seen = HashSet::new();
    for m in models {
        let id = m.id.trim();
        if !id.is_empty() && !seen.insert(id) {
            return Err(CompatibleError::DuplicateModel {
                provider: provider.to_owned(),
                model: id.to_owned(),
            });
        }
    }
    Ok(())
}

fn unique_flags(provider: &str, models: &[CompatibleModel]) -> Result<(), CompatibleError> {
    for m in models {
        let mut seen = HashSet::new();
        for d in &m.flags {
            let id = d.id.trim();
            if id.is_empty() {
                continue;
            }
            if !seen.insert(id) {
                return Err(CompatibleError::DuplicateFlag {
                    provider: provider.to_owned(),
                    flag: id.to_owned(),
                });
            }
            let mut values = HashSet::new();
            for o in &d.options {
                let v = o.value.trim();
                if !v.is_empty() && !values.insert(v) {
                    return Err(CompatibleError::DuplicateFlagOption {
                        provider: provider.to_owned(),
                        flag: id.to_owned(),
                        option: v.to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn normalize_models(models: Vec<CompatibleModel>) -> Vec<CompatibleModel> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(models.len());
    for mut m in models {
        m.id = m.id.trim().to_owned();
        if m.id.is_empty() || !seen.insert(m.id.clone()) {
            continue;
        }
        m.label = m.label.trim().to_owned();
        if m.label.is_empty() {
            m.label = m.id.clone();
        }
        m.context_window = m.context_window.max(0);
        m.flags = normalize_flags(std::mem::take(&mut m.flags));
        out.push(m);
    }
    out
}

fn normalize_flags(flags: Vec<CompatibleFlagDim>) -> Vec<CompatibleFlagDim> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(flags.len());
    for mut d in flags {
        d.id = d.id.trim().to_owned();
        if d.id.is_empty() || !seen.insert(d.id.clone()) {
            continue;
        }
        d.label = d.label.trim().to_owned();
        if d.label.is_empty() {
            d.label = d.id.clone();
        }
        d.wire = d.wire.trim().to_owned();
        if !valid_compatible_wire(&d.wire) {
            d.wire.clear();
        }
        d.options = normalize_flag_options(std::mem::take(&mut d.options));
        out.push(d);
    }
    out
}

fn normalize_flag_options(options: Vec<CompatibleFlagOption>) -> Vec<CompatibleFlagOption> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(options.len());
    for mut o in options {
        o.value = o.value.trim().to_owned();
        if o.value.is_empty() || !seen.insert(o.value.clone()) {
            continue;
        }
        o.label = o.label.trim().to_owned();
        if o.label.is_empty() {
            o.label = o.value.clone();
        }
        out.push(o);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> CompatibleModel {
        CompatibleModel {
            id: id.into(),
            ..Default::default()
        }
    }

    fn provider(id: &str, base: &str, models: Vec<CompatibleModel>) -> CompatibleProvider {
        CompatibleProvider {
            id: id.into(),
            base_url: base.into(),
            models,
            ..Default::default()
        }
    }

    fn opt(value: &str, label: &str, default: bool) -> CompatibleFlagOption {
        CompatibleFlagOption {
            value: value.into(),
            label: label.into(),
            default,
        }
    }

    fn dim(
        id: &str,
        label: &str,
        wire: &str,
        options: Vec<CompatibleFlagOption>,
    ) -> CompatibleFlagDim {
        CompatibleFlagDim {
            id: id.into(),
            label: label.into(),
            wire: wire.into(),
            options,
        }
    }

    #[test]
    fn normalize_provider() {
        let p = normalize_compatible_provider(
            CompatibleProvider {
                models: vec![CompatibleModel {
                    label: "GPT-5".into(),
                    ..model("gpt-5")
                }],
                ..provider(" OpenAI ", "https://api.openai.com/", vec![])
            },
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(p.id, "openai");
        assert_eq!(p.name, "openai");
        assert_eq!(p.base_url, "https://api.openai.com");
    }

    #[test]
    fn normalize_provider_cleans_everything() {
        let p = normalize_compatible_provider(
            CompatibleProvider {
                name: "  Local  ".into(),
                api_key: " k ".into(),
                extra_headers: [
                    (" X-A ".to_string(), " 1 ".to_string()),
                    ("Bad Key".to_string(), "x".to_string()),
                    ("a:b".to_string(), "x".to_string()),
                    ("  ".to_string(), "x".to_string()),
                ]
                .into_iter()
                .collect(),
                ..provider(
                    "local",
                    "http://localhost:11434//",
                    vec![
                        CompatibleModel {
                            context_window: -5,
                            flags: vec![
                                dim(
                                    " e ",
                                    "",
                                    "tools.foo",
                                    vec![opt(" low ", "", false), opt("", "x", false)],
                                ),
                                dim("", "skip", "", vec![]),
                            ],
                            ..model(" m1 ")
                        },
                        model(""),
                    ],
                )
            },
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(p.name, "Local");
        assert_eq!(p.api_key, "k");
        assert_eq!(p.base_url, "http://localhost:11434");
        assert_eq!(
            p.extra_headers,
            [("X-A".to_string(), "1".to_string())]
                .into_iter()
                .collect::<BTreeMap<_, _>>()
        );
        assert_eq!(p.models.len(), 1);
        let m = &p.models[0];
        assert_eq!(
            (m.id.as_str(), m.label.as_str(), m.context_window),
            ("m1", "m1", 0)
        );
        assert_eq!(
            m.flags,
            vec![dim("e", "e", "", vec![opt("low", "low", false)])]
        );
    }

    #[test]
    fn normalize_provider_rejections() {
        let none = HashSet::new();
        let models = || vec![model("x")];
        assert_eq!(
            normalize_compatible_provider(
                provider("claude-code", "https://example.com", models()),
                &none
            ),
            Err(CompatibleError::ReservedId("claude-code".into()))
        );
        let seen: HashSet<String> = ["openai".to_string()].into_iter().collect();
        assert_eq!(
            normalize_compatible_provider(
                provider("openai", "https://api.openai.com", models()),
                &seen
            ),
            Err(CompatibleError::DuplicateProvider("openai".into()))
        );
        assert_eq!(
            normalize_compatible_provider(provider("Bad_Id", "https://x", models()), &none),
            Err(CompatibleError::InvalidId("bad_id".into()))
        );
        assert_eq!(
            normalize_compatible_provider(provider(&"a".repeat(49), "https://x", models()), &none),
            Err(CompatibleError::InvalidId("a".repeat(49)))
        );
        assert!(
            normalize_compatible_provider(provider(&"a".repeat(48), "https://x", models()), &none)
                .is_ok()
        );
        assert_eq!(
            normalize_compatible_provider(provider("p", "  ", models()), &none),
            Err(CompatibleError::MissingBaseUrl)
        );
        assert_eq!(
            normalize_compatible_provider(provider("p", "localhost:8080", models()), &none),
            Err(CompatibleError::InvalidBaseUrl("localhost:8080".into()))
        );
        assert_eq!(
            normalize_compatible_provider(provider("p", "ftp://x/", models()), &none),
            Err(CompatibleError::BaseUrlScheme)
        );
        assert_eq!(
            normalize_compatible_provider(provider("p", "https://", models()), &none),
            Err(CompatibleError::InvalidBaseUrl("https:".into()))
        );
        assert!(
            normalize_compatible_provider(provider("p", "http://[::1]:8080/", models()), &none)
                .is_ok()
        );
        assert_eq!(
            normalize_compatible_provider(provider("p", "https://x", vec![model(" ")]), &none),
            Err(CompatibleError::NoModels("p".into()))
        );
    }

    #[test]
    fn offers_models_and_lookup() {
        let mut s = Settings {
            compatible_providers: vec![CompatibleProvider {
                name: " Open AI ".into(),
                ..provider("openai", "https://api.openai.com", vec![model("gpt-5")])
            }],
            ..Default::default()
        };
        assert!(s.provider_offers_models("openai"));
        assert!(s.is_compatible_slug(" openai "));
        assert!(!s.is_compatible_slug(""));
        assert_eq!(s.display_label("openai"), "Open AI");
        assert_eq!(s.display_label("cursor"), "Cursor");
        let p = s.compatible_by_id("openai").unwrap();
        assert!(p.configured());
        assert_eq!(p.model_by_id(" gpt-5 ").unwrap().id, "gpt-5");
        assert!(p.model_by_id("nope").is_none());
        s.disabled_providers = vec!["openai".into()];
        assert!(!s.provider_offers_models("openai"));
    }

    #[test]
    fn normalize_settings_omits_invalid() {
        let s = Settings {
            compatible_providers: vec![
                provider("a", "https://a", vec![model("m")]),
                provider("a", "https://b", vec![model("m")]),
                provider("cursor", "https://c", vec![model("m")]),
            ],
            ..Default::default()
        }
        .normalize_compatible_providers();
        assert_eq!(s.compatible_providers.len(), 1);
        assert_eq!(s.compatible_providers[0].base_url, "https://a");
    }

    #[test]
    fn flag_preset() {
        let dims = compatible_flag_preset("openai");
        assert_eq!(dims.len(), 2);
        assert_eq!(
            (dims[0].wire.as_str(), dims[1].wire.as_str()),
            ("reasoning.effort", "text.verbosity")
        );
        assert_eq!(compatible_flag_preset(" Effort ").len(), 1);
        assert!(compatible_flag_preset("none").is_empty());
    }

    #[test]
    fn wire_allowlist() {
        assert!(valid_compatible_wire("reasoning.effort") && valid_compatible_wire(""));
        assert!(valid_compatible_wire(" max_output_tokens "));
        assert!(!valid_compatible_wire("tools.foo"));
    }

    #[test]
    fn validate_rejects_empty_url_and_duplicates() {
        assert!(
            validate_compatible_providers(&[provider("openai", "", vec![model("gpt-5")])]).is_err()
        );
        let dup_models = provider(
            "openai",
            "https://api.openai.com",
            vec![
                CompatibleModel {
                    label: "GPT-5".into(),
                    context_window: 128000,
                    ..model("gpt-5")
                },
                CompatibleModel {
                    label: "Also GPT-5".into(),
                    context_window: 64000,
                    ..model(" gpt-5 ")
                },
            ],
        );
        assert_eq!(
            validate_compatible_providers(&[dup_models]),
            Err(CompatibleError::DuplicateModel {
                provider: "openai".into(),
                model: "gpt-5".into()
            })
        );
        let dup_flags = provider(
            "openai",
            "https://api.openai.com",
            vec![CompatibleModel {
                flags: vec![
                    dim(
                        "effort",
                        "Effort",
                        "reasoning.effort",
                        vec![opt("low", "", false)],
                    ),
                    dim(
                        "effort",
                        "Clobber",
                        "text.verbosity",
                        vec![opt("high", "", true)],
                    ),
                ],
                ..model("gpt-5")
            }],
        );
        assert!(matches!(
            validate_compatible_providers(&[dup_flags]),
            Err(CompatibleError::DuplicateFlag { .. })
        ));
        let dup_options = provider(
            "openai",
            "https://api.openai.com",
            vec![CompatibleModel {
                flags: vec![dim(
                    "effort",
                    "Effort",
                    "reasoning.effort",
                    vec![
                        opt("low", "Low", false),
                        opt("medium", "Medium", true),
                        opt("low", "Also low", false),
                    ],
                )],
                ..model("gpt-5")
            }],
        );
        let err = validate_compatible_providers(&[dup_options]).unwrap_err();
        assert_eq!(
            err.to_string(),
            r#"compatible provider "openai": flag "effort": duplicate flag option "low""#
        );
        let dup_providers = [
            provider("a", "https://a", vec![model("m")]),
            provider(" A ", "https://a", vec![model("m")]),
        ];
        assert_eq!(
            validate_compatible_providers(&dup_providers),
            Err(CompatibleError::DuplicateProvider("a".into()))
        );
    }
}
