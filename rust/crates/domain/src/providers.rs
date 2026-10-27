//! Provider on/off switches and remapping model slots off turned-off
//! providers (Settings → Providers).

use std::collections::BTreeMap;

use crate::Settings;
use crate::model::{PROVIDER_CLAUDE_CODE, PROVIDER_CODEX, PROVIDER_CURSOR, parse_model_ref};
use crate::settings::Flags;
use crate::skills::name_listed;

// Fallbacks for a mode that still points at a provider the user turned off.
// Claude ids are hardcoded in-repo, so they are safe to substitute;
// Cursor/Codex catalogues are live per account, so those ids are never
// invented here.
pub const FALLBACK_CLAUDE_MODEL: &str = "claude-code/claude-sonnet-5-5";
pub(crate) const FALLBACK_CLAUDE_TITLE_MODEL: &str = "claude-code/claude-haiku-4-5-20251001";

impl Settings {
    /// Whether the provider is on. Empty slug is Claude Code; an empty
    /// disabled list means every provider is on.
    pub fn provider_enabled(&self, provider: &str) -> bool {
        !name_listed(&self.disabled_providers, Self::default_provider(provider))
    }

    /// Whether this provider would appear in the model catalogue: it is on,
    /// and (for Cursor/Codex/compatible) configured. Claude Code needs no
    /// Recur-stored key.
    pub fn provider_offers_models(&self, provider: &str) -> bool {
        if !self.provider_enabled(provider) {
            return false;
        }
        match provider {
            PROVIDER_CLAUDE_CODE | "" => true,
            PROVIDER_CURSOR => self.cursor_configured(),
            PROVIDER_CODEX => self.codex_configured(),
            _ => self
                .compatible_by_id(provider)
                .is_some_and(|p| p.configured()),
        }
    }

    /// Keeps at least one catalogue provider on and remaps mode models that
    /// point at a turned-off provider. Unconfigured-but-on providers are left
    /// alone so clearing a key does not rewrite model picks.
    pub fn normalize_providers(self) -> Self {
        self.normalize_compatible_providers()
            .ensure_some_provider_offers_models()
            .remap_disabled_models()
    }

    fn offers_any_models(&self) -> bool {
        [PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR, PROVIDER_CODEX]
            .into_iter()
            .any(|p| self.provider_offers_models(p))
            || self
                .compatible_providers
                .iter()
                .any(|p| self.provider_offers_models(&p.id))
    }

    fn ensure_some_provider_offers_models(mut self) -> Self {
        if self.offers_any_models() {
            return self;
        }
        // Prefer a configured compatible endpoint over forcing Claude back on.
        let compatible: Vec<String> = self
            .compatible_providers
            .iter()
            .map(|p| p.id.clone())
            .collect();
        let builtin = [PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR, PROVIDER_CODEX].map(String::from);
        for p in compatible.iter().chain(&builtin) {
            drop_string(&mut self.disabled_providers, p);
            if self.offers_any_models() {
                break;
            }
        }
        self
    }

    fn remap_disabled_models(mut self) -> Self {
        (self.model_ask, self.flags_ask) =
            self.remap_disabled_model(&self.model_ask, &self.flags_ask, false);
        (self.model_plan, self.flags_plan) =
            self.remap_disabled_model(&self.model_plan, &self.flags_plan, false);
        (self.model_agent, self.flags_agent) =
            self.remap_disabled_model(&self.model_agent, &self.flags_agent, false);
        (self.model_loop, self.flags_loop) =
            self.remap_disabled_model(&self.model_loop, &self.flags_loop, false);
        // Empty Review inherits Loop: do not fill it in just because Claude is off.
        if !self.model_review.trim().is_empty() {
            (self.model_review, self.flags_review) =
                self.remap_disabled_model(&self.model_review, &self.flags_review, false);
        }
        (self.model_workspace_title, self.flags_workspace_title) = self.remap_disabled_model(
            &self.model_workspace_title,
            &self.flags_workspace_title,
            true,
        );
        self
    }

    /// Substitutes a Claude default when ref's provider is turned off and
    /// Claude is still on. Otherwise the ref is unchanged, including when the
    /// provider is merely unconfigured (credential edits cannot destroy
    /// picks) and when Claude itself is off (callers remap from the live
    /// catalogue then).
    pub fn remap_disabled_model(
        &self,
        model_ref: &str,
        flags: &Flags,
        title: bool,
    ) -> (String, Flags) {
        if !self.model_slot_disabled(model_ref, title)
            || !self.provider_enabled(PROVIDER_CLAUDE_CODE)
        {
            return (model_ref.to_owned(), flags.clone());
        }
        let (model, effort) = if title {
            (FALLBACK_CLAUDE_TITLE_MODEL, "low")
        } else {
            (FALLBACK_CLAUDE_MODEL, "high")
        };
        (
            model.to_owned(),
            BTreeMap::from([("effort".to_owned(), effort.to_owned())]),
        )
    }

    /// True when a slot still names a turned-off provider. Empty
    /// Ask/Plan/Agent/Loop and the workspace-title slot default to Claude, so
    /// they count when Claude is off. Empty Review inherits Loop.
    pub fn needs_catalog_fallback(&self) -> bool {
        [
            &self.model_ask,
            &self.model_plan,
            &self.model_agent,
            &self.model_loop,
        ]
        .into_iter()
        .any(|m| self.model_slot_disabled(m, true))
            || (!self.model_review.trim().is_empty()
                && self.model_slot_disabled(&self.model_review, false))
            || self.model_slot_disabled(&self.model_workspace_title, true)
    }

    /// Whether ref resolves to a turned-off provider or a compatible slug no
    /// longer in Settings. An empty slot inherits another slot unless
    /// `empty_means_claude`.
    pub fn model_ref_unavailable(&self, model_ref: &str, empty_means_claude: bool) -> bool {
        self.model_slot_disabled(model_ref, empty_means_claude)
    }

    fn model_slot_disabled(&self, model_ref: &str, empty_means_claude: bool) -> bool {
        let trimmed = model_ref.trim();
        if trimmed.is_empty() && !empty_means_claude {
            return false;
        }
        let p = parse_model_ref(trimmed).provider;
        if !self.provider_enabled(&p) {
            return true;
        }
        match p.as_str() {
            PROVIDER_CLAUDE_CODE | PROVIDER_CURSOR | PROVIDER_CODEX | "" => false,
            // Removed compatible slugs are unavailable even if not listed as disabled.
            _ => self.compatible_by_id(&p).is_none(),
        }
    }
}

fn drop_string(list: &mut Vec<String>, name: &str) {
    list.retain(|d| d != name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompatibleModel, CompatibleProvider};

    fn flags(pairs: &[(&str, &str)]) -> Flags {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn disabled(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn compatible(id: &str, base: &str, model: &str) -> CompatibleProvider {
        CompatibleProvider {
            id: id.into(),
            base_url: base.into(),
            models: vec![CompatibleModel {
                id: model.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn enabled_default_on() {
        let s = Settings::default();
        for p in [PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR, PROVIDER_CODEX, ""] {
            assert!(s.provider_enabled(p), "{p:?}");
        }
    }

    #[test]
    fn enabled_disabled_list() {
        let s = Settings {
            disabled_providers: disabled(&[PROVIDER_CURSOR, PROVIDER_CODEX]),
            ..Default::default()
        };
        assert!(s.provider_enabled(PROVIDER_CLAUDE_CODE));
        assert!(!s.provider_enabled(PROVIDER_CURSOR) && !s.provider_enabled(PROVIDER_CODEX));
        let claude_off = Settings {
            disabled_providers: disabled(&[PROVIDER_CLAUDE_CODE]),
            ..Default::default()
        };
        assert!(
            !claude_off.provider_enabled(""),
            "empty slug is Claude Code"
        );
    }

    #[test]
    fn offers_models() {
        let mut s = Settings {
            cursor_api_key: "sk-x".into(),
            codex_chat_gpt_auth: true,
            ..Default::default()
        };
        assert!(s.provider_offers_models(PROVIDER_CLAUDE_CODE));
        assert!(
            s.provider_offers_models(PROVIDER_CURSOR) && s.provider_offers_models(PROVIDER_CODEX)
        );
        s.disabled_providers = disabled(&[PROVIDER_CURSOR]);
        assert!(!s.provider_offers_models(PROVIDER_CURSOR));
        assert!(!Settings::default().provider_offers_models(PROVIDER_CURSOR));
        assert!(!Settings::default().provider_offers_models(PROVIDER_CODEX));
        assert!(!Settings::default().provider_offers_models("unknown"));
    }

    #[test]
    fn normalize_reenables_last_catalogue() {
        let s = Settings {
            disabled_providers: disabled(&[PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR, PROVIDER_CODEX]),
            model_agent: "cursor/composer-2.5".into(),
            ..Default::default()
        };
        let got = s.normalize_providers();
        assert!(got.provider_enabled(PROVIDER_CLAUDE_CODE));
        assert_eq!(got.model_agent, FALLBACK_CLAUDE_MODEL);
    }

    #[test]
    fn normalize_reenables_compatible() {
        let s = Settings {
            disabled_providers: disabled(&[
                PROVIDER_CLAUDE_CODE,
                PROVIDER_CURSOR,
                PROVIDER_CODEX,
                "openai",
            ]),
            compatible_providers: vec![compatible("openai", "https://api.openai.com", "gpt-5")],
            ..Default::default()
        };
        let got = s.normalize_providers();
        assert!(got.provider_enabled("openai"));
        assert!(
            !got.provider_enabled(PROVIDER_CLAUDE_CODE),
            "compatible preferred over Claude"
        );
    }

    #[test]
    fn normalize_remaps_disabled_provider() {
        let s = Settings {
            disabled_providers: disabled(&[PROVIDER_CURSOR]),
            cursor_api_key: "sk-x".into(),
            model_ask: "cursor/composer-2.5".into(),
            model_plan: "cursor/grok-4.6".into(),
            model_agent: "cursor/composer-2.5".into(),
            model_review: "cursor/composer-2.5".into(),
            model_loop: "claude-code/sonnet".into(),
            model_workspace_title: "cursor/composer-2.5".into(),
            flags_ask: flags(&[("fast", "true")]),
            flags_loop: flags(&[("effort", "high")]),
            ..Default::default()
        };
        let got = s.normalize_providers();
        assert_eq!(got.model_ask, FALLBACK_CLAUDE_MODEL);
        assert_eq!(got.model_plan, FALLBACK_CLAUDE_MODEL);
        assert_eq!(got.model_review, FALLBACK_CLAUDE_MODEL);
        assert_eq!(got.model_loop, "claude-code/sonnet");
        assert_eq!(got.flags_ask, flags(&[("effort", "high")]));
        assert_eq!(got.flags_loop, flags(&[("effort", "high")]));
        assert_eq!(got.model_workspace_title, FALLBACK_CLAUDE_TITLE_MODEL);
        assert_eq!(got.flags_workspace_title, flags(&[("effort", "low")]));
        assert_eq!(
            got.cursor_api_key, "sk-x",
            "turning Cursor off keeps the API key"
        );
    }

    #[test]
    fn normalize_leaves_unconfigured_pick() {
        let s = Settings {
            model_agent: "codex/gpt-5.6-luna".into(),
            flags_agent: flags(&[("effort", "high")]),
            model_ask: "cursor/composer-2.5".into(),
            ..Default::default()
        };
        let got = s.normalize_providers();
        assert_eq!(got.model_agent, "codex/gpt-5.6-luna");
        assert_eq!(got.model_ask, "cursor/composer-2.5");
    }

    #[test]
    fn normalize_leaves_empty_review() {
        let s = Settings {
            disabled_providers: disabled(&[PROVIDER_CURSOR]),
            ..Default::default()
        };
        let got = s.normalize_providers();
        assert_eq!(got.model_review, "");
        assert_eq!(
            got.model_ask, "",
            "empty slots on an enabled Claude stay empty"
        );
    }

    #[test]
    fn normalize_does_not_invent_cursor_fallback() {
        let s = Settings {
            disabled_providers: disabled(&[PROVIDER_CLAUDE_CODE]),
            cursor_api_key: "sk-x".into(),
            model_agent: "claude-code/sonnet".into(),
            ..Default::default()
        };
        let got = s.normalize_providers();
        assert_eq!(got.model_agent, "claude-code/sonnet");
        assert!(got.needs_catalog_fallback());
    }

    #[test]
    fn needs_catalog_fallback_empty_title_when_claude_off() {
        let mut s = Settings {
            disabled_providers: disabled(&[PROVIDER_CLAUDE_CODE]),
            cursor_api_key: "sk-x".into(),
            ..Default::default()
        };
        assert!(s.needs_catalog_fallback(), "empty title defaults to haiku");
        for slot in [
            &mut s.model_ask,
            &mut s.model_plan,
            &mut s.model_agent,
            &mut s.model_loop,
        ] {
            *slot = "cursor/composer-2".into();
        }
        s.model_workspace_title = "cursor/composer-2".into();
        assert!(!s.needs_catalog_fallback());
    }

    #[test]
    fn model_ref_unavailable_removed_compatible() {
        let s = Settings {
            disabled_providers: disabled(&[PROVIDER_CLAUDE_CODE]),
            compatible_providers: vec![compatible("beta", "https://b.example", "m2")],
            model_ask: "alpha/m1".into(),
            model_plan: "alpha/m1".into(),
            model_agent: "alpha/m1".into(),
            model_loop: "alpha/m1".into(),
            model_workspace_title: "alpha/m1".into(),
            ..Default::default()
        };
        assert!(s.model_ref_unavailable("alpha/m1", false));
        assert!(!s.model_ref_unavailable("beta/m2", false));
        assert!(!s.model_ref_unavailable("", false));
        assert!(s.model_ref_unavailable("", true));
        assert!(s.needs_catalog_fallback());
    }

    #[test]
    fn normalize_remaps_removed_compatible() {
        let s = Settings {
            model_agent: "openai/gpt-5".into(),
            compatible_providers: vec![compatible("other", "https://example.com", "m")],
            ..Default::default()
        };
        assert_eq!(s.normalize_providers().model_agent, FALLBACK_CLAUDE_MODEL);
    }
}
