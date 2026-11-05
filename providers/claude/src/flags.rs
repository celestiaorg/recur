//! Claude Code model flags and the picker list (Claude Code has no catalogue
//! API).

use recur_agent::{FlagDimension, FlagOption, Flags, ModelChoice};
use serde::Deserialize;

fn flag<'a>(flags: &'a Flags, key: &str) -> &'a str {
    flags.get(key).map_or("", String::as_str)
}

/// Applies the context flag to a bare model id. Models with a 200K↔1M toggle
/// get `[1m]` only when `context=1m`. Pinned always-1M ids always get it: a
/// Claude Code build that predates the id assumes a 200K window and
/// auto-compacts at ~167K (claude-fable-5-1 on 2.1.252); `[1m]` makes it use
/// 1M and is a no-op on recognized models. Aliases stay bare.
pub fn resolve_model(base: &str, flags: &Flags) -> String {
    if base.is_empty() {
        return String::new();
    }
    if flag(flags, "context") == "1m" && selectable_context(base) {
        return format!("{base}[1m]");
    }
    if always_1m(base) && bare_model_id(base).starts_with("claude-") && !base.ends_with(']') {
        return format!("{base}[1m]");
    }
    base.to_owned()
}

/// The active context limit for the meter. `modelUsage.contextWindow` is the
/// model ceiling and does not reflect the 4.6 200K↔1M dock selection.
/// Always-1M models ignore stale `300k` flags; selectable 4.6 models use 200K
/// unless `context=1m`; Haiku and unknown models use the standard 200K.
pub fn context_window(model: &str, flags: &Flags) -> i64 {
    if flag(flags, "context") == "1m" || always_1m(model) {
        return 1_000_000;
    }
    200_000
}

/// Whether a dock context dimension was set (including legacy `300k`). The
/// engine resolves a stored message's window from the flags only when a
/// model or this flag is known.
pub fn has_context_flag(flags: &Flags) -> bool {
    flags.contains_key("context")
}

/// Models that always run with a 1M window on the Anthropic API.
fn always_1m(model: &str) -> bool {
    let id = bare_model_id(model);
    matches!(
        id,
        "sonnet"
            | "fable"
            | "opus"
            | "claude-sonnet-5"
            | "claude-fable-5-1"
            | "claude-fable-5"
            | "claude-opus-5"
            | "claude-opus-4-8"
            | "claude-opus-4-7"
    ) || [
        "claude-sonnet-5",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-haiku-5",
        "claude-opus-5",
    ]
    .iter()
    .any(|p| id.starts_with(p))
}

/// Models that still expose a 200K↔1M toggle through the `[1m]` suffix.
pub fn selectable_context(model: &str) -> bool {
    matches!(
        bare_model_id(model),
        "claude-sonnet-4-6" | "claude-opus-4-6"
    )
}

fn bare_model_id(model: &str) -> &str {
    let id = model.trim();
    let id = id.split_once('/').map_or(id, |(_, rest)| rest);
    id.split_once('[').map_or(id, |(head, _)| head)
}

/// A short picker-style name: claude-fable-5-1 → Fable 5.1, claude-opus-4-8 →
/// Opus 4.8, fable → Fable.
pub fn model_label(model: &str) -> String {
    let id = bare_model_id(model);
    let id = id.strip_prefix("claude-").unwrap_or(id).trim();
    if id.is_empty() {
        return String::new();
    }
    let mut parts = id.split('-');
    let family = parts.next().unwrap_or_default();
    if family.is_empty() {
        return id.to_owned();
    }
    let mut label = String::with_capacity(id.len());
    let mut chars = family.chars();
    if let Some(c) = chars.next() {
        label.push(c.to_ascii_uppercase());
    }
    label.push_str(chars.as_str());
    let rest: Vec<&str> = parts.collect();
    if !rest.is_empty() {
        label.push(' ');
        label.push_str(&rest.join("."));
    }
    label
}

/// The `--effort` value; empty when unset so the flag is omitted.
pub fn effort(flags: &Flags) -> &str {
    flag(flags, "effort")
}

fn opt(value: &str, label: &str, default: bool) -> FlagOption {
    FlagOption {
        value: value.into(),
        label: label.into(),
        default,
    }
}

fn effort_dim() -> FlagDimension {
    FlagDimension {
        id: "effort".into(),
        label: "Effort".into(),
        options: vec![
            opt("low", "Low", false),
            opt("medium", "Medium", false),
            opt("high", "High", true),
            opt("xhigh", "Extra High", false),
            opt("max", "Max", false),
        ],
    }
}

/// Only for models that still expose a real 200K↔1M toggle (Sonnet/Opus 4.6).
fn context_dim() -> FlagDimension {
    FlagDimension {
        id: "context".into(),
        label: "Context".into(),
        options: vec![opt("200k", "200K", true), opt("1m", "1M", false)],
    }
}

/// The Claude Code picker list. Every row is a pinned id, never an alias, so
/// a saved choice keeps meaning the same model. Extended context is a flag
/// only where Claude Code still distinguishes it ([`resolve_model`] applies
/// `[1m]` at run time); Sonnet 5, Fable 5.1, Opus 4.7+ and Haiku 5.5 always run at 1M.
pub fn catalog() -> Vec<ModelChoice> {
    vec![
        row("claude-fable-5-1", "Fable 5.1", false, 1_000_000),
        row("claude-fable-5", "Fable 5", false, 1_000_000),
        row("claude-opus-5-5", "Opus 5.5", false, 1_000_000),
        row("claude-opus-5", "Opus 5", false, 1_000_000),
        row("claude-opus-4-8", "Opus 4.8", false, 1_000_000),
        row("claude-opus-4-7", "Opus 4.7", false, 1_000_000),
        row("claude-opus-4-6", "Opus 4.6", true, 0),
        row("claude-sonnet-5-5", "Sonnet 5.5", false, 1_000_000),
        row("claude-sonnet-5", "Sonnet 5", false, 1_000_000),
        row("claude-sonnet-4-6", "Sonnet 4.6", true, 0),
        row("claude-haiku-5-5", "Haiku 5.5", false, 1_000_000),
        row("claude-haiku-4-5-20251001", "Haiku 4.5", false, 200_000),
    ]
}

fn row(id: &str, label: &str, with_context: bool, context_window: i64) -> ModelChoice {
    let mut flags = vec![effort_dim()];
    if with_context {
        flags.push(context_dim());
    }
    ModelChoice {
        model_ref: format!("claude-code/{id}"),
        label: label.into(),
        flags,
        context_window,
    }
}

/// One model the CLI's stream-json initialize handshake reports.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProbedModel {
    pub value: String,
    pub resolved_model: String,
    pub display_name: String,
}

/// [`catalog`] plus the models Claude Code reports for this account that
/// the pinned list lacks (e.g. Mythos 5.1 on accounts with access), listed
/// first. Aliases resolve to pinned ids, so they dedupe against existing rows.
pub fn with_probed(probed: &[ProbedModel]) -> Vec<ModelChoice> {
    let mut out = catalog();
    let mut extra = 0;
    for p in probed {
        let id = match p.resolved_model.trim() {
            "" => p.value.trim(),
            id => id,
        };
        let id = id.split_once('[').map_or(id, |(head, _)| head);
        let model_ref = format!("claude-code/{id}");
        if !id.starts_with("claude-") || out.iter().any(|m| m.model_ref == model_ref) {
            continue;
        }
        let label = match p.display_name.trim() {
            "" => id,
            l => l,
        };
        out.insert(
            extra,
            row(id, label, false, context_window(id, &Flags::new())),
        );
        extra += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use recur_domain::{PROVIDER_CLAUDE_CODE, parse_model_ref};

    use super::*;

    fn flags(pairs: &[(&str, &str)]) -> Flags {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn resolve_context_window() {
        let none = Flags::new();
        for (model, f, want) in [
            ("sonnet", flags(&[]), 1_000_000),
            ("sonnet", flags(&[("context", "300k")]), 1_000_000),
            ("claude-code/opus", none.clone(), 1_000_000),
            ("claude-code/claude-opus-5", none.clone(), 1_000_000),
            ("claude-fable-5-1", none.clone(), 1_000_000),
            ("claude-mythos-5-1", none.clone(), 1_000_000),
            ("claude-haiku-5-5", none.clone(), 1_000_000),
            ("claude-sonnet-4-6", flags(&[("context", "200k")]), 200_000),
            ("claude-opus-4-6", none.clone(), 200_000),
            ("claude-sonnet-4-6", flags(&[("context", "1m")]), 1_000_000),
            ("haiku", none.clone(), 200_000),
        ] {
            assert_eq!(context_window(model, &f), want, "{model} {f:?}");
        }
    }

    /// Models Claude Code reports beyond the pinned list (Mythos on accounts
    /// with access) join the top of the picker; aliases, pinned ids and `[1m]`
    /// suffixes do not duplicate rows.
    #[test]
    fn probed_models_add_account_only_models() {
        let p = |value: &str, resolved: &str, label: &str| ProbedModel {
            value: value.into(),
            resolved_model: resolved.into(),
            display_name: label.into(),
        };
        let pinned = catalog().len();
        let models = with_probed(&[
            p("default", "claude-opus-5-5", "Default (recommended)"),
            p("claude-fable-5", "claude-fable-5", "Fable 5"),
            p("opus[1m]", "claude-opus-5-5[1m]", "Opus"),
            p("mythos[1m]", "claude-mythos-5-1[1m]", "Mythos 5.1"),
            p("gpt-5", "", ""),
        ]);
        assert_eq!(models.len(), pinned + 1);
        let first = &models[0];
        assert_eq!(first.model_ref, "claude-code/claude-mythos-5-1");
        assert_eq!(first.label, "Mythos 5.1");
        assert_eq!(first.context_window, 1_000_000);
        assert!(!first.flags.is_empty());
        assert_eq!(with_probed(&[]).len(), pinned);
    }

    #[test]
    fn resolve_claude_model() {
        for (model, f, want) in [
            ("sonnet", flags(&[("context", "1m")]), "sonnet"),
            (
                "claude-sonnet-4-6",
                flags(&[("context", "1m")]),
                "claude-sonnet-4-6[1m]",
            ),
            (
                "claude-sonnet-4-6",
                flags(&[("context", "200k")]),
                "claude-sonnet-4-6",
            ),
            ("sonnet", flags(&[("context", "300k")]), "sonnet"),
            ("sonnet", flags(&[]), "sonnet"),
            (
                "claude-fable-5-1",
                flags(&[("effort", "high")]),
                "claude-fable-5-1[1m]",
            ),
            ("claude-opus-5-5", flags(&[]), "claude-opus-5-5[1m]"),
            ("claude-sonnet-5-5", flags(&[]), "claude-sonnet-5-5[1m]"),
            ("claude-opus-4-6", flags(&[]), "claude-opus-4-6"),
            ("claude-fable-5-1[1m]", flags(&[]), "claude-fable-5-1[1m]"),
            ("", flags(&[("context", "1m")]), ""),
        ] {
            assert_eq!(resolve_model(model, &f), want, "{model} {f:?}");
        }
    }

    #[test]
    fn labels() {
        for (input, want) in [
            ("claude-fable-5-1", "Fable 5.1"),
            ("claude-fable-5", "Fable 5"),
            ("claude-opus-4-8", "Opus 4.8"),
            ("claude-code/fable", "Fable"),
            ("opus", "Opus"),
            ("", ""),
        ] {
            assert_eq!(model_label(input), want, "{input}");
        }
    }

    #[test]
    fn effort_and_context_flag() {
        assert_eq!(effort(&flags(&[("effort", "high")])), "high");
        assert_eq!(effort(&Flags::new()), "");
        assert!(has_context_flag(&flags(&[("context", "300k")])));
        assert!(!has_context_flag(&Flags::new()));
    }

    #[test]
    fn catalog_refs_are_qualified() {
        let models = catalog();
        assert!(!models.is_empty());
        for m in &models {
            assert!(m.model_ref.starts_with("claude-code/"), "{}", m.model_ref);
            let r = parse_model_ref(&m.model_ref);
            assert_eq!(r.provider, PROVIDER_CLAUDE_CODE);
            assert!(!r.id.is_empty());
            assert!(!m.label.is_empty());
            assert!(!m.model_ref.contains("[1m]"), "{}", m.model_ref);
        }
        assert!(
            models
                .iter()
                .any(|m| m.model_ref == "claude-code/claude-sonnet-5-5")
        );
    }

    #[test]
    fn catalog_flags() {
        for m in catalog() {
            assert_eq!(m.flags[0].id, "effort", "{}", m.model_ref);
            let ctx = m.flags.iter().find(|f| f.id == "context");
            let want = matches!(
                m.model_ref.as_str(),
                "claude-code/claude-opus-4-6" | "claude-code/claude-sonnet-4-6"
            );
            assert_eq!(ctx.is_some(), want, "{}", m.model_ref);
            if let Some(ctx) = ctx {
                let values: Vec<_> = ctx.options.iter().map(|o| o.value.as_str()).collect();
                assert_eq!(values, ["200k", "1m"]);
            } else {
                assert!(m.context_window > 0, "{}", m.model_ref);
            }
        }
    }

    #[test]
    fn catalog_tracks_fable_51() {
        let rows = |pred: &dyn Fn(&str) -> bool| {
            catalog()
                .into_iter()
                .filter(|m| pred(&m.model_ref))
                .map(|m| format!("{} {}", m.model_ref, m.label))
                .collect::<Vec<_>>()
                .join(",")
        };
        assert_eq!(
            rows(&|r| r.contains("fable")),
            "claude-code/claude-fable-5-1 Fable 5.1,claude-code/claude-fable-5 Fable 5"
        );
        assert_eq!(
            rows(&|r| r.contains("opus") && !r.contains("opus-4")),
            "claude-code/claude-opus-5-5 Opus 5.5,claude-code/claude-opus-5 Opus 5"
        );
    }
}
