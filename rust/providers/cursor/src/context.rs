//! Cursor model context limits (Go `cursor/context.go`) and the context
//! window a turn resolves to (Go `engine/cursor_flags.go`).

use recur_agent::Flags;

/// A model's working window: `base` by default, `max` when the dock selects
/// a larger context or Max Mode (0 = does not expand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextLimits {
    pub base: i64,
    pub max: i64,
}

const fn lim(base: i64, max: i64) -> ContextLimits {
    ContextLimits { base, max }
}

/// Normalized model-id prefixes (lowercase, '.' → '-'); the longest prefix
/// wins.
///
/// Sources (Cursor docs + SpaceXAI model card, Aug 2026): composer-2.5 fixed
/// (treated as 200k); grok-4.7 / 4.6 500k; grok-4.5 256k; claude-sonnet-5 up
/// to 1M (200k threshold); claude-opus-5 up to 1M (300k threshold);
/// claude-fable-5 / 5-1 200k / 1M; gemini-3.1-pro and 3.6-flash 1M;
/// gpt-5.6 / 5.5 / 5.4 272k ↔ 1M from ListModels; mini/nano and older GPT
/// have no toggle. Older or unlisted families keep conservative prefixes.
const CONTEXT_WINDOWS: &[(&str, ContextLimits)] = &[
    ("composer", lim(200_000, 0)),
    ("grok-4-7", lim(500_000, 0)),
    ("grok-4-6", lim(500_000, 0)),
    ("grok", lim(256_000, 0)),
    ("claude-sonnet-5", lim(200_000, 1_000_000)),
    ("claude-opus-5", lim(300_000, 1_000_000)),
    ("claude-fable-5", lim(200_000, 1_000_000)),
    ("claude-sonnet", lim(200_000, 1_000_000)),
    ("claude-opus", lim(200_000, 1_000_000)),
    ("claude-fable", lim(200_000, 1_000_000)),
    ("claude-haiku", lim(200_000, 0)),
    ("gpt-5-6", lim(272_000, 1_000_000)),
    ("gpt-5-5", lim(272_000, 1_000_000)),
    ("gpt-5-4-mini", lim(272_000, 0)),
    ("gpt-5-4-nano", lim(272_000, 0)),
    ("gpt-5-4", lim(272_000, 1_000_000)),
    ("gpt-5-3", lim(272_000, 0)),
    ("gpt-5-2", lim(272_000, 0)),
    ("gpt-5-1", lim(272_000, 0)),
    ("gpt-5", lim(272_000, 0)),
    ("gemini-3-6", lim(1_000_000, 0)),
    ("gemini-3-1", lim(1_000_000, 0)),
    ("gemini-3-5", lim(1_000_000, 0)),
    ("gemini-3", lim(1_000_000, 0)),
    ("gemini-2-5", lim(1_000_000, 0)),
    ("gemini", lim(1_000_000, 0)),
    ("kimi-k3", lim(200_000, 1_000_000)),
    ("kimi", lim(200_000, 1_000_000)),
    ("glm", lim(200_000, 0)),
    ("auto", lim(200_000, 0)),
];

/// When no prefix matches.
const DEFAULT_CONTEXT_WINDOW: i64 = 200_000;

fn normalize_model_id(id: &str) -> String {
    crate::gojson::trim(id).to_lowercase().replace('.', "-")
}

/// The context limits for a bare Cursor model id; unknown ids get 200k.
pub fn limits_for(model_id: &str) -> ContextLimits {
    let m = normalize_model_id(model_id);
    if m.is_empty() {
        return lim(DEFAULT_CONTEXT_WINDOW, 0);
    }
    CONTEXT_WINDOWS
        .iter()
        .filter(|(p, _)| m.starts_with(p))
        .max_by_key(|(p, _)| p.len())
        .map_or(lim(DEFAULT_CONTEXT_WINDOW, 0), |(_, l)| *l)
}

/// The default (non-expanded) window.
pub fn base_context_window(model_id: &str) -> i64 {
    let l = limits_for(model_id);
    if l.base > 0 {
        l.base
    } else {
        DEFAULT_CONTEXT_WINDOW
    }
}

/// Picker order inside the Cursor group: house models (Composer / Auto /
/// Grok) first, then the lab that trained the rest.
pub fn vendor(model_id: &str) -> &'static str {
    let n = normalize_model_id(model_id);
    let has = |p: &str| n.starts_with(p);
    if has("composer") || has("auto") || has("grok") {
        "Cursor"
    } else if has("claude") {
        "Anthropic"
    } else if has("gpt") || has("o1") || has("o3") || has("o4") {
        "OpenAI"
    } else if has("gemini") {
        "Google"
    } else if has("kimi") {
        "Moonshot"
    } else if has("glm") {
        "Zhipu"
    } else {
        "Other"
    }
}

pub const VENDOR_ORDER: &[&str] = &[
    "Cursor",
    "Anthropic",
    "OpenAI",
    "Google",
    "Moonshot",
    "Zhipu",
    "Other",
];

/// The context limit of a Cursor turn: an explicit `context` flag ("1m",
/// "200k", authoritative because the options come from ListModels), else Max
/// Mode → the model's max, else the model's base, else 200k.
pub fn resolve_context_window(model: &str, flags: &Flags) -> i64 {
    let n = parse_context_flag(flags.get("context").map_or("", String::as_str));
    if n > 0 {
        return n;
    }
    let l = limits_for(model);
    if max_mode_enabled(flags) && l.max > 0 {
        return l.max;
    }
    if l.base > 0 {
        return l.base;
    }
    200_000
}

fn max_mode_enabled(flags: &Flags) -> bool {
    let get = |k: &str| flags.get(k).map_or("", |v| crate::gojson::trim(v));
    if matches!(
        get("maxMode").to_lowercase().as_str(),
        "true" | "on" | "1" | "max"
    ) {
        return true;
    }
    get("max").eq_ignore_ascii_case("true")
}

/// "1m", "200k", "300000", …; 0 when unparseable.
pub fn parse_context_flag(s: &str) -> i64 {
    let s = crate::gojson::trim(s).to_lowercase();
    if s.is_empty() {
        return 0;
    }
    let (num, mult) = if let Some(n) = s.strip_suffix('m') {
        (n, 1_000_000.0)
    } else if let Some(n) = s.strip_suffix('k') {
        (n, 1_000.0)
    } else {
        (s.as_str(), 1.0)
    };
    let num = crate::gojson::trim(num);
    if num.is_empty() {
        return 0;
    }
    match num.parse::<f64>() {
        Ok(f) if f > 0.0 => (f * mult) as i64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go TestLimitsForDocumentedModels.
    #[test]
    fn limits_for_documented_models() {
        for (id, base, max) in [
            ("composer-2.5", 200_000, 0),
            ("grok-4.7", 500_000, 0),
            ("grok-4.6", 500_000, 0),
            ("grok-4.5", 256_000, 0),
            ("claude-sonnet-5", 200_000, 1_000_000),
            ("claude-opus-5", 300_000, 1_000_000),
            ("claude-opus-5-fast", 300_000, 1_000_000),
            ("claude-fable-5", 200_000, 1_000_000),
            ("claude-fable-5-1", 200_000, 1_000_000),
            ("gemini-3.1-pro", 1_000_000, 0),
            ("gemini-3-1-pro", 1_000_000, 0),
            ("gemini-3.6-flash", 1_000_000, 0),
            ("gpt-5.6-sol", 272_000, 1_000_000),
            ("gpt-5.6-sol-fast", 272_000, 1_000_000),
            ("gpt-5.6-terra", 272_000, 1_000_000),
            ("gpt-5.6-luna", 272_000, 1_000_000),
            ("gpt-5.5", 272_000, 1_000_000),
            ("gpt-5.4", 272_000, 1_000_000),
            ("gpt-5.4-mini", 272_000, 0),
            ("gpt-5.2", 272_000, 0),
            ("claude-opus-4-8", 200_000, 1_000_000),
        ] {
            assert_eq!(limits_for(id), lim(base, max), "{id}");
        }
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_model_id("GPT-5.6-Sol"), "gpt-5-6-sol");
    }

    #[test]
    fn vendors() {
        for (id, want) in [
            ("composer-2.5", "Cursor"),
            ("auto-smart", "Cursor"),
            ("grok-4.6", "Cursor"),
            ("claude-opus-5", "Anthropic"),
            ("gpt-5.6-sol", "OpenAI"),
            ("o3", "OpenAI"),
            ("gemini-3.1-pro", "Google"),
            ("kimi-k2", "Moonshot"),
            ("glm-4.6", "Zhipu"),
            ("mystery-9", "Other"),
        ] {
            assert_eq!(vendor(id), want, "{id}");
        }
    }

    fn flags(kv: &[(&str, &str)]) -> Flags {
        kv.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }

    // Go TestResolveCursorContextWindow.
    #[test]
    fn resolve_window() {
        for (model, f, want) in [
            ("composer-2.5", flags(&[]), 200_000),
            ("grok-4.7", flags(&[]), 500_000),
            ("grok-4.6", flags(&[]), 500_000),
            ("grok-4.5", flags(&[]), 256_000),
            ("claude-sonnet-5", flags(&[]), 200_000),
            ("claude-sonnet-5", flags(&[("context", "1m")]), 1_000_000),
            ("claude-opus-5", flags(&[]), 300_000),
            ("claude-opus-5", flags(&[("maxMode", "true")]), 1_000_000),
            ("gpt-5.6-sol", flags(&[]), 272_000),
            ("gpt-5.6-sol", flags(&[("context", "1m")]), 1_000_000),
            ("gpt-5.6-terra", flags(&[]), 272_000),
            ("gpt-5.6-terra", flags(&[("maxMode", "true")]), 1_000_000),
            ("gpt-5.6-luna", flags(&[("maxMode", "true")]), 1_000_000),
            ("gpt-5.4-mini", flags(&[("context", "1m")]), 1_000_000),
            ("gemini-3.1-pro", flags(&[]), 1_000_000),
            ("gemini-3.6-flash", flags(&[]), 1_000_000),
            ("composer-2.5", flags(&[("maxMode", "true")]), 200_000),
            ("grok-4.7", flags(&[("maxMode", "true")]), 500_000),
            ("grok-4.6", flags(&[("maxMode", "true")]), 500_000),
            ("grok-4.5", flags(&[("maxMode", "true")]), 256_000),
            ("brand-new-model-9", flags(&[]), 200_000),
            ("brand-new-model-9", flags(&[("context", "1m")]), 1_000_000),
        ] {
            assert_eq!(resolve_context_window(model, &f), want, "{model} {f:?}");
        }
    }

    // Go TestParseContextFlag.
    #[test]
    fn parse_flag() {
        for (input, want) in [
            ("", 0),
            ("272k", 272_000),
            ("1m", 1_000_000),
            ("1.0M", 1_000_000),
            ("200k", 200_000),
            ("300K", 300_000),
            ("200000", 200_000),
            ("garbage", 0),
        ] {
            assert_eq!(parse_context_flag(input), want, "{input}");
        }
    }
}
