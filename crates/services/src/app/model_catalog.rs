//! The model catalog merge: one grouped picker list over every enabled,
//! configured provider (each provider lists its own models), and the Settings
//! remap that repoints slots naming a turned-off provider.

use recur_agent::Provider as _;
use recur_agent::catalog::{ModelChoice, ModelGroup, default_flags_from_dims};
use recur_domain::{
    Flags, PROVIDER_CHATGPT, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX, PROVIDER_CURSOR, Settings,
    provider_label,
};

use super::AppServices;

/// The grouped catalog: Claude Code, Cursor, Codex, each compatible
/// endpoint, then ChatGPT. Disabled providers are omitted; Cursor and Codex
/// appear only with models, ChatGPT once an account is connected.
pub fn build_model_catalog(
    set: &Settings,
    cursor_models: Vec<ModelChoice>,
    codex_models: Vec<ModelChoice>,
) -> Vec<ModelGroup> {
    let mut groups = Vec::new();
    if set.provider_enabled(PROVIDER_CLAUDE_CODE) {
        groups.push(ModelGroup {
            provider: PROVIDER_CLAUDE_CODE.into(),
            label: provider_label(PROVIDER_CLAUDE_CODE).into(),
            models: recur_provider_claude::flags::catalog(),
        });
    }
    if set.provider_enabled(PROVIDER_CURSOR) && !cursor_models.is_empty() {
        groups.push(ModelGroup {
            provider: PROVIDER_CURSOR.into(),
            label: provider_label(PROVIDER_CURSOR).into(),
            models: cursor_models,
        });
    }
    if set.provider_enabled(PROVIDER_CODEX) && !codex_models.is_empty() {
        groups.push(ModelGroup {
            provider: PROVIDER_CODEX.into(),
            label: provider_label(PROVIDER_CODEX).into(),
            models: codex_models,
        });
    }
    for p in &set.compatible_providers {
        if !set.provider_enabled(&p.id) || !p.configured() {
            continue;
        }
        groups.push(ModelGroup {
            provider: p.id.clone(),
            label: set.display_label(&p.id).to_owned(),
            models: recur_provider_compatible::compatible_catalog_models(p),
        });
    }
    if set.provider_offers_models(PROVIDER_CHATGPT) {
        let p = set.chatgpt_provider();
        groups.push(ModelGroup {
            provider: p.id.clone(),
            label: p.name.clone(),
            models: recur_provider_compatible::compatible_catalog_models(&p),
        });
    }
    groups
}

/// The first model in the catalog and its default flags.
pub fn first_catalog_choice(groups: &[ModelGroup]) -> (String, Flags) {
    groups
        .iter()
        .flat_map(|g| &g.models)
        .find(|m| !m.model_ref.trim().is_empty())
        .map(|m| (m.model_ref.clone(), default_flags_from_dims(&m.flags)))
        .unwrap_or_default()
}

fn strip_disabled_provider(list: &[String], p: &str) -> Vec<String> {
    list.iter().filter(|x| *x != p).cloned().collect()
}

/// Repoints every slot that still names a turned-off provider to the first
/// catalog model with its default flags. With nothing to pick, Claude Code is
/// turned back on so a turn can still run.
pub fn apply_catalog_fallbacks(mut set: Settings, groups: &[ModelGroup]) -> Settings {
    if !set.needs_catalog_fallback() {
        return set;
    }
    let (id, flags) = first_catalog_choice(groups);
    if id.is_empty() {
        set.disabled_providers =
            strip_disabled_provider(&set.disabled_providers, PROVIDER_CLAUDE_CODE);
        return set.normalize_providers();
    }
    let probe = set.clone();
    let apply = |model: &mut String, fl: &mut Flags, empty_means_claude: bool| {
        if probe.model_ref_unavailable(model, empty_means_claude) {
            model.clone_from(&id);
            fl.clone_from(&flags);
        }
    };
    apply(&mut set.model_ask, &mut set.flags_ask, true);
    apply(&mut set.model_plan, &mut set.flags_plan, true);
    apply(&mut set.model_agent, &mut set.flags_agent, true);
    apply(&mut set.model_loop, &mut set.flags_loop, true);
    if !set.model_review.trim().is_empty() {
        apply(&mut set.model_review, &mut set.flags_review, false);
    }
    apply(
        &mut set.model_workspace_title,
        &mut set.flags_workspace_title,
        true,
    );
    set
}

/// Whether a save turns a provider on or off, (un)configures Cursor, Codex or
/// ChatGPT, or adds, removes or (un)configures a compatible endpoint.
pub fn provider_enablement_changed(a: &Settings, b: &Settings) -> bool {
    if [
        PROVIDER_CLAUDE_CODE,
        PROVIDER_CURSOR,
        PROVIDER_CODEX,
        PROVIDER_CHATGPT,
    ]
    .iter()
    .any(|p| a.provider_enabled(p) != b.provider_enabled(p))
    {
        return true;
    }
    if a.cursor_configured() != b.cursor_configured()
        || a.codex_configured() != b.codex_configured()
        || a.chatgpt_configured() != b.chatgpt_configured()
    {
        return true;
    }
    if a.compatible_providers.len() != b.compatible_providers.len() {
        return true;
    }
    for p in &b.compatible_providers {
        if a.provider_enabled(&p.id) != b.provider_enabled(&p.id) {
            return true;
        }
        match a.compatible_by_id(&p.id) {
            Some(prev) if prev.configured() == p.configured() => {}
            _ => return true,
        }
    }
    a.compatible_providers
        .iter()
        .any(|p| b.compatible_by_id(&p.id).is_none())
}

impl AppServices {
    /// Cursor and Codex list live (a throwaway bridge or app-server); each
    /// provider remembers the result for remaps. Claude Code's pinned list
    /// gains the models the local CLI reports for the account;
    /// `probe_claude = false` reuses the last good probe, spawning claude only
    /// until one succeeds.
    pub(crate) async fn model_catalog(&self, probe_claude: bool) -> Vec<ModelGroup> {
        let set = self.store().settings();
        let claude = async {
            if !set.provider_enabled(PROVIDER_CLAUDE_CODE) {
                Vec::new()
            } else if probe_claude {
                self.providers.claude.models(&set).await.unwrap_or_default()
            } else {
                self.providers.claude.known_models(&set).await
            }
        };
        let rest = async {
            let cursor = self.providers.cursor.models(&set).await.unwrap_or_default();
            let codex = self.providers.codex.models(&set).await.unwrap_or_default();
            build_model_catalog(&set, cursor, codex)
        };
        let (claude, mut groups) = tokio::join!(claude, rest);
        if let Some(g) = groups
            .first_mut()
            .filter(|g| g.provider == PROVIDER_CLAUDE_CODE)
        {
            g.models = claude;
        }
        groups
    }

    /// The catalog a Settings save remaps slots with: the remembered lists
    /// first, so a toggle does not start a throwaway bridge; a live probe
    /// only when the cache has nothing the incoming settings can use, one
    /// provider at a time so a hung bridge does not also burn the Codex
    /// budget.
    pub(crate) async fn catalog_for_fallback(&self, set: &Settings) -> Vec<ModelGroup> {
        let codex_cached = self.providers.codex.cached_models(set);
        let groups = build_model_catalog(
            set,
            self.providers.cursor.cached_models(set),
            codex_cached.clone(),
        );
        if !first_catalog_choice(&groups).0.is_empty() {
            return groups;
        }
        let cursor = self.providers.cursor.models(set).await.unwrap_or_default();
        let groups = build_model_catalog(set, cursor.clone(), codex_cached);
        if !first_catalog_choice(&groups).0.is_empty() {
            return groups;
        }
        let codex = self.providers.codex.models(set).await.unwrap_or_default();
        build_model_catalog(set, cursor, codex)
    }
}
