//! The pure rules behind Settings: `lib/compatible-providers.ts`, the `lib/providers.ts`
//! catalogue rules, and the view's own derivations (branch prefix presets, governor caps, the
//! `onchange` number parsing of an `<input type="number">`).

use std::collections::{BTreeMap, HashSet};

use recur_domain::{CompatibleFlagDim, CompatibleModel, CompatibleProvider, Settings};
use recur_view_core::models::provider_enabled;

pub const PROVIDER_CLAUDE: &str = "claude-code";
pub const PROVIDER_CURSOR: &str = "cursor";
pub const PROVIDER_CODEX: &str = "codex";
pub const PROVIDER_CHATGPT: &str = "chatgpt";

const CATALOGUE_PROVIDERS: [&str; 3] = [PROVIDER_CLAUDE, PROVIDER_CURSOR, PROVIDER_CODEX];

/// One `COMPATIBLE_PRESETS` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompatiblePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub flags: &'static str,
    pub model_id: &'static str,
    pub model_label: &'static str,
}

pub const COMPATIBLE_PRESETS: [CompatiblePreset; 4] = [
    CompatiblePreset {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com",
        flags: "openai",
        model_id: "gpt-5",
        model_label: "GPT-5",
    },
    CompatiblePreset {
        id: "openrouter",
        name: "OpenRouter",
        base_url: "https://openrouter.ai/api",
        flags: "",
        model_id: "openai/gpt-4o",
        model_label: "GPT-4o",
    },
    CompatiblePreset {
        id: "groq",
        name: "Groq",
        base_url: "https://api.groq.com/openai",
        flags: "",
        model_id: "",
        model_label: "",
    },
    CompatiblePreset {
        id: "custom",
        name: "Custom",
        base_url: "",
        flags: "",
        model_id: "",
        model_label: "",
    },
];

/// `COMPATIBLE_WIRES` (value and label are the same).
pub const COMPATIBLE_WIRES: [&str; 5] = [
    "reasoning.effort",
    "reasoning.summary",
    "text.verbosity",
    "temperature",
    "max_output_tokens",
];

pub const RESERVED_PROVIDER_SLUGS: [&str; 7] = [
    "claude-code",
    "cursor",
    "codex",
    "chatgpt",
    "anthropic",
    "gemini",
    "ollama",
];

/// `COMPATIBLE_SLUG_RE`: `^[a-z][a-z0-9-]{0,47}$`.
fn slug_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 48
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `compatibleSlugError`.
pub fn compatible_slug_error(id: &str, taken: &[String]) -> String {
    let slug = id.trim().to_lowercase();
    if !slug_ok(&slug) {
        return "Slug must be kebab-case (a-z, digits, hyphen), starting with a letter.".into();
    }
    if RESERVED_PROVIDER_SLUGS.contains(&slug.as_str()) {
        return format!("Slug {slug} is reserved.");
    }
    if taken.contains(&slug) {
        return format!("A provider named {slug} already exists.");
    }
    String::new()
}

/// `compatibleConfigured`.
pub fn compatible_configured(p: &CompatibleProvider) -> bool {
    !p.base_url.trim().is_empty() && p.models.iter().any(|m| !m.id.trim().is_empty())
}

/// `persistableModels`: empty ids are dropped by the backend, so those rows stay local.
pub fn persistable_models(models: &[CompatibleModel]) -> Vec<CompatibleModel> {
    models
        .iter()
        .filter(|m| !m.id.trim().is_empty())
        .cloned()
        .collect()
}

/// `persistableHeaders`: rows with a key, keyed by the trimmed key (later rows win).
pub fn persistable_headers(rows: &[(String, String)]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in rows {
        let k = k.trim();
        if !k.is_empty() {
            out.insert(k.to_string(), v.clone());
        }
    }
    out
}

/// `unusedKey`: `prefix`, then `prefix2`, `prefix3`, … skipping taken keys.
pub fn unused_key<'a>(taken: impl IntoIterator<Item = &'a str>, prefix: &str) -> String {
    let have: HashSet<&str> = taken
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if !have.contains(prefix) {
        return prefix.to_string();
    }
    (2..)
        .map(|i| format!("{prefix}{i}"))
        .find(|k| !have.contains(k.as_str()))
        .unwrap_or_default()
}

/// `persistableFlags`: trimmed ids, labels defaulting to the id, empty wires dropped, options
/// without a value dropped, dimensions without an id dropped.
pub fn persistable_flags(flags: &[CompatibleFlagDim]) -> Vec<CompatibleFlagDim> {
    flags
        .iter()
        .map(|d| {
            let id = d.id.trim().to_string();
            let label = match d.label.trim() {
                "" => id.clone(),
                l => l.to_string(),
            };
            CompatibleFlagDim {
                label,
                wire: d.wire.trim().to_string(),
                options: d
                    .options
                    .iter()
                    .filter(|o| !o.value.trim().is_empty())
                    .cloned()
                    .collect(),
                id,
            }
        })
        .filter(|d| !d.id.is_empty())
        .collect()
}

/// `flagKind` in `CompatibleProviderCard.svelte`: which picker-controls preset a model's flags
/// look like.
pub fn flag_kind(m: &CompatibleModel) -> &'static str {
    if m.flags.is_empty() {
        return "";
    }
    let has = |id: &str| m.flags.iter().any(|f| f.id == id);
    if has("effort") && has("verbosity") && m.flags.len() == 2 {
        return "openai";
    }
    if m.flags.len() == 1 && m.flags[0].id == "effort" {
        return "effort";
    }
    "custom"
}

/// `providerOffersModels`: the provider is on and (for Cursor, Codex and compatible slugs)
/// actually ready.
pub fn provider_offers_models(
    slug: &str,
    disabled: &[String],
    cursor_ready: bool,
    codex_ready: bool,
    extra_ready: &[String],
) -> bool {
    if !provider_enabled(disabled, slug) {
        return false;
    }
    match slug {
        PROVIDER_CLAUDE => true,
        PROVIDER_CURSOR => cursor_ready,
        PROVIDER_CODEX => codex_ready,
        _ => extra_ready.iter().any(|s| s == slug),
    }
}

/// `canDisableProvider`: false when turning `slug` off would leave no models in the pickers.
pub fn can_disable_provider(
    slug: &str,
    disabled: &[String],
    cursor_ready: bool,
    codex_ready: bool,
    extra_ready: &[String],
) -> bool {
    if !provider_enabled(disabled, slug) {
        return true;
    }
    let next = recur_code::view::skills::toggle_disabled_name(disabled, slug, false);
    CATALOGUE_PROVIDERS
        .iter()
        .copied()
        .chain(extra_ready.iter().map(String::as_str))
        .any(|p| provider_offers_models(p, &next, cursor_ready, codex_ready, extra_ready))
}

/// `savedBranchPrefix`.
pub fn saved_branch_prefix(s: Option<&Settings>) -> String {
    match s {
        Some(s) => match &s.branch_name_prefix {
            Some(p) => p.clone(),
            None if s.branch_prefix_mode.as_str() == "none" => String::new(),
            None => "recur".into(),
        },
        None => "recur".into(),
    }
}

/// `branchPrefixExample`.
pub fn branch_prefix_example(value: &str) -> String {
    match value.trim() {
        "" => "crispy-quiche".into(),
        v => format!("{v}/crispy-quiche"),
    }
}

/// `prefixPreset`: which preset button is on.
pub fn prefix_preset(saved: &str, dirty: bool, github_login: &str) -> &'static str {
    if dirty {
        return "";
    }
    if saved.is_empty() {
        return "none";
    }
    if saved == "recur" {
        return "recur";
    }
    if !github_login.is_empty() && saved.to_lowercase() == github_login.to_lowercase() {
        return "github";
    }
    ""
}

/// The value of an `<input type="number">`: the sanitized value is `""` unless the text is a
/// valid floating-point number (HTML), and `Number("")` is 0.
pub fn number_input_value(raw: &str) -> f64 {
    valid_float(raw).unwrap_or(0.)
}

/// HTML's "valid floating-point number": `-`? then digits with an optional fraction (or a
/// fraction alone), then an optional exponent.
fn valid_float(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    if b.first() == Some(&b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return None;
        }
    } else if int_digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let exp_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// JavaScript's `Math.round`: halves round toward +∞.
pub fn js_round(n: f64) -> f64 {
    (n + 0.5).floor()
}

/// `parseGovernorCap`: blank or negative falls back; otherwise the rounded number.
pub fn parse_governor_cap(raw: &str, fallback: i64) -> i64 {
    let t = raw.trim();
    if t.is_empty() {
        return fallback;
    }
    let n = number_input_value(t);
    let v = js_round(n) as i64;
    if v < 0 { fallback } else { v }
}

/// The JavaScript `String(n)` of a whole number shown back in a number field.
pub fn int_text(n: i64) -> String {
    n.to_string()
}

/// `mcpAuthHeader`: a bare token becomes `Bearer <token>`.
pub fn mcp_auth_header(raw: &str) -> String {
    let auth = raw.trim();
    if auth.starts_with("Bearer ") {
        auth.to_string()
    } else {
        format!("Bearer {auth}")
    }
}

/// `str.trim().split(/\s+/).filter(Boolean)`.
pub fn split_args(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_domain::CompatibleFlagOption;

    fn model(id: &str, label: &str) -> CompatibleModel {
        CompatibleModel {
            id: id.into(),
            label: label.into(),
            ..Default::default()
        }
    }

    // compatible-providers.test.ts
    #[test]
    fn persistable_models_drops_empty_ids() {
        assert_eq!(
            persistable_models(&[
                model("qwen/qwen3-coder-next", "Qwen"),
                model("  ", "x"),
                model("", ""),
            ]),
            vec![model("qwen/qwen3-coder-next", "Qwen")]
        );
    }

    #[test]
    fn slug_errors() {
        assert!(compatible_slug_error("claude-code", &[]).contains("reserved"));
        assert!(compatible_slug_error("Foo_Bar", &[]).contains("kebab-case"));
        assert!(compatible_slug_error("openai", &["openai".into()]).contains("already exists"));
        assert_eq!(compatible_slug_error("openrouter", &[]), "");
        assert!(compatible_slug_error("", &[]).contains("kebab-case"));
        assert_eq!(compatible_slug_error(&"a".repeat(48), &[]), "");
        assert!(compatible_slug_error(&"a".repeat(49), &[]).contains("kebab-case"));
        assert_eq!(compatible_slug_error(" OpenAI ", &[]), "");
    }

    #[test]
    fn persistable_flags_drops_empty_ids_and_values() {
        let opt = |v: &str, l: &str| CompatibleFlagOption {
            value: v.into(),
            label: l.into(),
            default: false,
        };
        let got = persistable_flags(&[
            CompatibleFlagDim {
                id: "effort".into(),
                label: "Effort".into(),
                wire: "reasoning.effort".into(),
                options: vec![opt("high", "High"), opt("", "x")],
            },
            CompatibleFlagDim {
                id: String::new(),
                label: "nope".into(),
                ..Default::default()
            },
        ]);
        assert_eq!(
            got,
            vec![CompatibleFlagDim {
                id: "effort".into(),
                label: "Effort".into(),
                wire: "reasoning.effort".into(),
                options: vec![opt("high", "High")],
            }]
        );
        let labeled = persistable_flags(&[CompatibleFlagDim {
            id: " flag2 ".into(),
            label: "  ".into(),
            wire: "  ".into(),
            options: vec![],
        }]);
        assert_eq!(labeled[0].id, "flag2");
        assert_eq!(labeled[0].label, "flag2");
        assert_eq!(labeled[0].wire, "");
    }

    #[test]
    fn unused_key_repeats_and_skips_live_ids() {
        let mut values: Vec<String> = vec![unused_key(std::iter::empty(), "value")];
        values.push(unused_key(values.iter().map(String::as_str), "value"));
        assert_eq!(values, ["value", "value2"]);

        let mut ids = vec!["effort".to_string(), "flag2".into(), "flag3".into()];
        ids.retain(|id| id != "flag2");
        let next = unused_key(ids.iter().map(String::as_str), "flag");
        ids.push(next);
        assert_eq!(ids, ["effort", "flag3", "flag"]);
    }

    #[test]
    fn persistable_headers_drops_empty_keys() {
        let got = persistable_headers(&[
            ("X-Title".into(), "recur".into()),
            ("  ".into(), "x".into()),
            (String::new(), String::new()),
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got.get("X-Title").map(String::as_str), Some("recur"));
        assert!(persistable_headers(&[]).is_empty());
    }

    #[test]
    fn flag_kinds() {
        let dim = |id: &str| CompatibleFlagDim {
            id: id.into(),
            ..Default::default()
        };
        let mut m = model("a", "A");
        assert_eq!(flag_kind(&m), "");
        m.flags = vec![dim("effort")];
        assert_eq!(flag_kind(&m), "effort");
        m.flags = vec![dim("verbosity"), dim("effort")];
        assert_eq!(flag_kind(&m), "openai");
        m.flags.push(dim("x"));
        assert_eq!(flag_kind(&m), "custom");
    }

    // providers.test.ts (the catalogue halves; `providerEnabled` and friends are view-core's)
    #[test]
    fn provider_offers_models_rules() {
        let none: Vec<String> = vec![];
        let claude = vec![PROVIDER_CLAUDE.to_string()];
        let cursor = vec![PROVIDER_CURSOR.to_string()];
        let openai = vec!["openai".to_string()];
        assert!(provider_offers_models(
            PROVIDER_CLAUDE,
            &none,
            false,
            false,
            &none
        ));
        assert!(!provider_offers_models(
            PROVIDER_CLAUDE,
            &claude,
            false,
            false,
            &none
        ));
        assert!(!provider_offers_models(
            PROVIDER_CURSOR,
            &none,
            false,
            false,
            &none
        ));
        assert!(provider_offers_models(
            PROVIDER_CURSOR,
            &none,
            true,
            false,
            &none
        ));
        assert!(!provider_offers_models(
            PROVIDER_CURSOR,
            &cursor,
            true,
            false,
            &none
        ));
        assert!(provider_offers_models(
            PROVIDER_CODEX,
            &none,
            false,
            true,
            &none
        ));
        assert!(!provider_offers_models(
            "openai", &none, false, false, &none
        ));
        assert!(provider_offers_models(
            "openai", &none, false, false, &openai
        ));
        assert!(!provider_offers_models(
            "openai", &openai, false, false, &openai
        ));
    }

    #[test]
    fn can_disable_provider_rules() {
        let none: Vec<String> = vec![];
        let claude = vec![PROVIDER_CLAUDE.to_string()];
        let openai = vec!["openai".to_string()];
        assert!(!can_disable_provider(
            PROVIDER_CLAUDE,
            &none,
            false,
            false,
            &none
        ));
        assert!(can_disable_provider(
            PROVIDER_CURSOR,
            &none,
            false,
            false,
            &none
        ));
        assert!(can_disable_provider(
            PROVIDER_CLAUDE,
            &none,
            true,
            false,
            &none
        ));
        assert!(!can_disable_provider(
            PROVIDER_CURSOR,
            &claude,
            true,
            false,
            &none
        ));
        assert!(can_disable_provider(
            PROVIDER_CLAUDE,
            &none,
            false,
            false,
            &openai
        ));
        assert!(can_disable_provider("openai", &none, false, false, &openai));
        assert!(!can_disable_provider(
            "openai", &claude, false, false, &openai
        ));
    }

    #[test]
    fn branch_prefix_rules() {
        assert_eq!(saved_branch_prefix(None), "recur");
        let mut s = Settings::default();
        assert_eq!(saved_branch_prefix(Some(&s)), "recur");
        s.branch_prefix_mode = "none".into();
        assert_eq!(saved_branch_prefix(Some(&s)), "");
        s.branch_name_prefix = Some("josh".into());
        assert_eq!(saved_branch_prefix(Some(&s)), "josh");
        assert_eq!(branch_prefix_example(" josh "), "josh/crispy-quiche");
        assert_eq!(branch_prefix_example("  "), "crispy-quiche");
        assert_eq!(prefix_preset("", false, ""), "none");
        assert_eq!(prefix_preset("recur", false, ""), "recur");
        assert_eq!(prefix_preset("Josh", false, "josh"), "github");
        assert_eq!(prefix_preset("josh", false, ""), "");
        assert_eq!(prefix_preset("recur", true, ""), "");
    }

    #[test]
    fn number_fields_parse_like_the_page() {
        assert_eq!(number_input_value("12"), 12.);
        assert_eq!(number_input_value("-3.5"), -3.5);
        assert_eq!(number_input_value(".5"), 0.5);
        assert_eq!(number_input_value("1e2"), 100.);
        assert_eq!(number_input_value("1."), 0.);
        assert_eq!(number_input_value("+5"), 0.);
        assert_eq!(number_input_value("abc"), 0.);
        assert_eq!(number_input_value(""), 0.);
        assert_eq!(number_input_value("inf"), 0.);
        assert_eq!(js_round(2.5), 3.);
        assert_eq!(js_round(-2.5), -2.);
        assert_eq!(parse_governor_cap("", 4), 4);
        assert_eq!(parse_governor_cap(" 6 ", 4), 6);
        assert_eq!(parse_governor_cap("-1", 4), 4);
        assert_eq!(parse_governor_cap("2.6", 8), 3);
        assert_eq!(parse_governor_cap("x", 8), 0);
    }

    #[test]
    fn mcp_helpers() {
        assert_eq!(mcp_auth_header(" tok "), "Bearer tok");
        assert_eq!(mcp_auth_header("Bearer tok"), "Bearer tok");
        assert_eq!(split_args("  -y  @example/mcp "), ["-y", "@example/mcp"]);
        assert!(split_args("   ").is_empty());
    }
}
