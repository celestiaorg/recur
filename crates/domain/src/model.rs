//! Provider slugs and fully-qualified model refs (`provider/id`).
//!
//! The same slug prefixes a [`ModelRef`] and is stored on session/message
//! rows, so a session's provider and the model that produced it agree.

use std::fmt;

use serde::{Deserialize, Serialize};

// Provider identifiers stored on session/message rows. Neutral names, mapped
// to real API strings inside each provider implementation.
pub const PROVIDER_CLAUDE_CODE: &str = "claude-code";
pub const PROVIDER_CURSOR: &str = "cursor";
pub const PROVIDER_CODEX: &str = "codex";
pub const PROVIDER_CHATGPT: &str = "chatgpt";
pub const PROVIDER_ANTHROPIC: &str = "anthropic";
pub const PROVIDER_OPENAI: &str = "openai";
pub const PROVIDER_GEMINI: &str = "gemini";
pub const PROVIDER_OLLAMA: &str = "ollama";

/// The human name for a provider slug.
pub fn provider_label(provider: &str) -> &str {
    match provider {
        PROVIDER_CURSOR => "Cursor",
        PROVIDER_CODEX => "Codex",
        PROVIDER_CHATGPT => "ChatGPT",
        PROVIDER_CLAUDE_CODE => "Claude Code",
        _ => provider,
    }
}

/// Reports whether slug is a Recur-hosted CLI/SDK harness (Claude Code,
/// Cursor, Codex) rather than a user-defined compatible endpoint.
pub fn builtin_provider(provider: &str) -> bool {
    matches!(
        provider,
        PROVIDER_CLAUDE_CODE | PROVIDER_CURSOR | PROVIDER_CODEX | ""
    )
}

/// A fully-qualified model selection: which provider runs the turn, and which
/// of that provider's models it uses. Stored as `provider/id`, e.g.
/// `claude-code/opus` or `cursor/composer-2.5`. Picking a model IS picking a
/// provider.
///
/// Model ids MAY contain "/": [`parse_model_ref`] splits on the first slash,
/// so `openrouter/openai/gpt-4o` is provider `openrouter`, id `openai/gpt-4o`.
///
/// Go has no JSON tags on this type, so it encodes with the field names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelRef {
    #[serde(rename = "Provider")]
    pub provider: String,
    #[serde(rename = "ID")]
    pub id: String,
}

/// Splits a stored model ref. An unqualified value is Claude Code, the
/// default provider. An empty value yields an empty id, which means "let the
/// provider choose its own default model".
pub fn parse_model_ref(s: &str) -> ModelRef {
    let s = s.trim();
    if s.is_empty() {
        return ModelRef::new(PROVIDER_CLAUDE_CODE, "");
    }
    match s.split_once('/') {
        Some((provider, id)) => ModelRef::new_raw(provider, id),
        None => ModelRef::new(PROVIDER_CLAUDE_CODE, s),
    }
}

impl ModelRef {
    /// Builds a ref, defaulting an empty provider to Claude Code.
    pub fn new(provider: &str, id: &str) -> Self {
        let provider = if provider.is_empty() {
            PROVIDER_CLAUDE_CODE
        } else {
            provider
        };
        Self::new_raw(provider, id)
    }

    fn new_raw(provider: &str, id: &str) -> Self {
        Self {
            provider: provider.to_owned(),
            id: id.to_owned(),
        }
    }

    /// Reports whether no model was selected.
    pub fn is_zero(&self) -> bool {
        self.id.is_empty()
    }
}

/// Renders the ref in storage form; empty when no model is selected.
impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.id.is_empty() {
            return Ok(());
        }
        write!(f, "{}/{}", self.provider, self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_model_ref_cases() {
        assert_eq!(
            parse_model_ref("openrouter/openai/gpt-4o"),
            ModelRef::new("openrouter", "openai/gpt-4o")
        );
        assert_eq!(
            parse_model_ref("  opus "),
            ModelRef::new("claude-code", "opus")
        );
        let empty = parse_model_ref("");
        assert_eq!(empty, ModelRef::new("claude-code", ""));
        assert!(empty.is_zero());
        assert_eq!(empty.to_string(), "");
        assert_eq!(parse_model_ref("/x"), ModelRef::new_raw("", "x"));
        assert_eq!(
            parse_model_ref("cursor/composer-2.5").to_string(),
            "cursor/composer-2.5"
        );
        assert_eq!(
            parse_model_ref("claude-code/opus").to_string(),
            "claude-code/opus"
        );
        assert_eq!(ModelRef::new("", "x").provider, PROVIDER_CLAUDE_CODE);
    }

    #[test]
    fn labels() {
        assert_eq!(provider_label(PROVIDER_CURSOR), "Cursor");
        assert_eq!(provider_label(PROVIDER_CODEX), "Codex");
        assert_eq!(provider_label(PROVIDER_CLAUDE_CODE), "Claude Code");
        assert_eq!(provider_label("openai"), "openai");
        assert!(builtin_provider("") && builtin_provider(PROVIDER_CODEX));
        assert!(!builtin_provider(PROVIDER_OPENAI));
    }

    #[test]
    fn go_json_shape() {
        let r = ModelRef::new("cursor", "x");
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"Provider":"cursor","ID":"x"}"#
        );
    }
}
