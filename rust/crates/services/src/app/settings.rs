//! `SettingsService` (Go `services.go` settings half, `claude_status.go`,
//! `codex_service.go`, `cursor_service.go`, `compatible_service.go`,
//! `provider_usage.go`): settings and their save side effects, provider
//! status cards, the model catalog, usage and assigned issues.

use async_trait::async_trait;
use recur_agent::{CredentialDraft, Provider as _, ProviderStatus};
use recur_domain::{
    BranchPrefixMode, CompatibleFlagDim, CompatibleProvider, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX,
    PROVIDER_CURSOR, Resource, Settings, effective_agent_turn_timeout_minutes,
    effective_max_agents, effective_max_loops, normalize_mcp_servers,
    validate_compatible_providers, validate_mcp_servers,
};
use tokio_util::sync::CancellationToken;

use super::model_catalog::{apply_catalog_fallbacks, provider_enablement_changed};
use super::rt::{blocking, rt};
use super::{AppServices, err};
use crate::api::{
    AssignedIssue, ClaudeStatus, CodexStatus, CompatibleTestResult, CursorStatus,
    CursorUsageConnect, ModelGroup, SettingsService, Snapshot,
};
use crate::events::{AppEvent, CursorInstall};
use crate::{Error, Result};

/// Go `persistBranchNamePrefix`: the stored prefix is the normalized
/// effective one; empty is "none", mirrored into the legacy mode so a dropped
/// empty value cannot silently revert to `recur`.
pub(crate) fn persist_branch_name_prefix(mut set: Settings) -> Settings {
    let raw = match &set.branch_name_prefix {
        Some(p) => p.clone(),
        None => set.effective_branch_name_prefix(),
    };
    let n = recur_git::normalize_branch_prefix(&raw);
    set.branch_prefix_mode = if n.is_empty() {
        BranchPrefixMode::None
    } else {
        BranchPrefixMode::default()
    };
    set.branch_name_prefix = Some(n);
    set.branch_prefix_custom.clear();
    set
}

fn claude_status(st: ProviderStatus) -> ClaudeStatus {
    ClaudeStatus {
        authenticated: st.authenticated,
        path: st.path,
        version: st.version,
        provider: st.provider,
        login_method: st.login_method,
        organization: st.organization,
        email: st.email,
        error: st.error,
    }
}

fn codex_status(st: ProviderStatus) -> CodexStatus {
    CodexStatus {
        bin_installed: st.installed,
        bin_path: st.path,
        authenticated: st.authenticated,
        login_method: st.login_method,
        email: st.email,
        plan: st.plan,
        error: st.error,
    }
}

impl AppServices {
    pub(crate) fn settings_view(&self) -> Settings {
        let mut set = self.store().settings().migrate_bundle_values();
        set.branch_name_prefix = Some(recur_git::normalize_branch_prefix(
            &set.effective_branch_name_prefix(),
        ));
        set
    }

    /// Whether `provider` could run a turn now: on, configured, and (Cursor,
    /// Codex) its bridge or binary installed.
    fn provider_runnable(&self, set: &Settings, provider: &str) -> bool {
        if !set.provider_enabled(provider) {
            return false;
        }
        match provider {
            PROVIDER_CLAUDE_CODE => true,
            PROVIDER_CURSOR => {
                set.cursor_configured() && recur_provider_cursor::locate(self.data_dir()).is_ok()
            }
            PROVIDER_CODEX => {
                set.codex_configured()
                    && recur_provider_codex::locate::locate(&set.codex_bin_path).is_ok()
            }
            _ => set
                .compatible_by_id(provider)
                .is_some_and(|p| p.configured()),
        }
    }

    /// Turns Claude Code back on when every other enabled provider is missing
    /// its binary or bridge, so the pickers cannot go fully empty.
    fn ensure_runnable_catalogue(&self, mut set: Settings) -> Settings {
        let fixed = [PROVIDER_CLAUDE_CODE, PROVIDER_CURSOR, PROVIDER_CODEX];
        if fixed.iter().any(|p| self.provider_runnable(&set, p))
            || set
                .compatible_providers
                .iter()
                .any(|p| self.provider_runnable(&set, &p.id))
        {
            return set;
        }
        set.disabled_providers.retain(|p| p != PROVIDER_CLAUDE_CODE);
        set.normalize_providers()
    }

    /// Go `SetSettings`: validate, normalize, remap slots off turned-off
    /// providers, persist, then apply the side effects: Cursor disconnect,
    /// provider registration and settings, chat remaps, language servers,
    /// keep-awake, governor caps and the scheduled-workspace prune.
    pub(crate) async fn save_settings(&self, set: Settings) -> Result<Settings> {
        validate_mcp_servers(&set.mcp_servers).map_err(err)?;
        validate_compatible_providers(&set.compatible_providers).map_err(err)?;
        let mut set = set.migrate_bundle_values();
        // A concrete minutes value round-trips (0/unset → default; clamped).
        set.agent_turn_timeout_minutes =
            effective_agent_turn_timeout_minutes(set.agent_turn_timeout_minutes);
        set.max_loops = effective_max_loops(set.max_loops);
        set.max_agents = effective_max_agents(set.max_agents);
        let mut set = persist_branch_name_prefix(set);
        set.mcp_servers = normalize_mcp_servers(&set.mcp_servers);
        let prev = self.store().settings();
        let mut set = set.normalize_providers();
        let enablement_changed = provider_enablement_changed(&prev, &set);
        if enablement_changed {
            set = self.ensure_runnable_catalogue(set);
            if set.needs_catalog_fallback() {
                let groups = self.catalog_for_fallback(&set).await;
                set = apply_catalog_fallbacks(set, &groups);
            }
        }
        let turning_cursor_off =
            prev.provider_enabled(PROVIDER_CURSOR) && !set.provider_enabled(PROVIDER_CURSOR);
        if turning_cursor_off {
            // Recur owns the Connect session: off means Disconnect.
            set.cursor_api_key.clear();
        }
        self.store().set_settings(set.clone());
        if turning_cursor_off {
            // The key is already cleared, so this only revokes the session.
            if let Err(e) = self.usage.disconnect_cursor_usage().await {
                tracing::warn!("recur: warning: disconnect cursor usage: {e}");
            }
        }
        if self.providers.sync_compatible(&set, self.data_dir()) {
            self.engine.providers().replace(self.providers.all());
        }
        if enablement_changed {
            self.engine.remap_chats_off_disabled_providers(&set);
        }
        // Providers keep a settings snapshot; Cursor also rewrites the
        // attribution file warm bridges read, closes its pool when turned
        // off, and cancels a deferred close when on.
        for p in self.engine.providers().lifecycle() {
            p.settings_changed(&prev, &set).await;
        }
        self.lsp
            .registry()
            .set_disabled_servers(&set.disabled_language_servers);
        self.lsp.close_servers(&set.disabled_language_servers);
        // Prevent-sleep applies at once, for in-flight turns too.
        self.engine.sync_keep_awake();
        self.engine
            .apply_governor_caps(set.max_loops, set.max_agents)
            .await;
        if set.scheduled_workspace_keep != prev.scheduled_workspace_keep {
            let eng = self.engine.clone();
            tokio::spawn(async move { eng.prune_scheduled_workspaces("").await });
        }
        Ok(set)
    }

    async fn cursor_status_at(&self) -> CursorStatus {
        // A leftover pasted key goes when Recur has no Connect session.
        let set = self
            .usage
            .drop_orphan_cursor_key(self.store().settings(), self.data_dir())
            .await;
        self.providers.cursor.cursor_status(&set).await
    }

    async fn install_cursor_bridge_at(&self) -> Result<CursorStatus> {
        let hub = self.hub.clone();
        let progress = move |p: recur_provider_cursor::InstallProgress| {
            if let Ok(ev) = AppEvent::from_wire("cursor.install", p.event_payload()) {
                hub.publish(ev);
            }
        };
        let bin = match self
            .providers
            .cursor
            .install(&CancellationToken::new(), &progress)
            .await
        {
            Ok(bin) => bin,
            Err(e) => {
                self.hub.publish(AppEvent::CursorInstall(CursorInstall {
                    phase: "error".into(),
                    message: e.clone(),
                    percent: -1,
                    ..Default::default()
                }));
                return Err(Error::new(e));
            }
        };
        // Display only: locate always uses <data>/cursor-bridge.
        let mut set = self.store().settings();
        set.cursor_bridge_path = bin.to_string_lossy().into_owned();
        self.store().set_settings(set);
        Ok(self.cursor_status_at().await)
    }

    fn logs_path_at(&self) -> String {
        recur_store::applog::app_log_path(self.data_dir())
            .to_string_lossy()
            .into_owned()
    }

    async fn github_login_at(&self) -> String {
        match self.engine.forges().get("github") {
            Some(f) => f.login().await,
            None => String::new(),
        }
    }
}

#[async_trait]
impl SettingsService for AppServices {
    async fn cancel_cursor_usage_connect(&self) -> Result<()> {
        rt!(self, => |s| {
            s.usage.cancel_cursor_usage_connect();
            Ok(())
        })
    }

    async fn clear_all_data(&self) -> Result<()> {
        rt!(self, => |s| s.engine.clear_all().map_err(err))
    }

    async fn compatible_flag_preset(&self, name: &str) -> Result<Vec<CompatibleFlagDim>> {
        Ok(recur_domain::compatible_flag_preset(name.trim()))
    }

    async fn connect_cursor_usage(&self) -> Result<CursorUsageConnect> {
        rt!(self, => |s| Ok(s.usage.connect_cursor_usage()))
    }

    async fn detect_agent(&self) -> Result<String> {
        rt!(self, => |s| {
            let st = s.providers.claude.status(&s.store().settings()).await;
            Ok(recur_provider_claude::status::detect_agent(&st))
        })
    }

    async fn disconnect_cursor_usage(&self) -> Result<()> {
        rt!(self, => |s| s.usage.disconnect_cursor_usage().await.map_err(Error::new))
    }

    async fn get_claude_status(&self) -> Result<ClaudeStatus> {
        rt!(self, => |s| {
            Ok(claude_status(s.providers.claude.status(&s.store().settings()).await))
        })
    }

    async fn get_codex_status(&self) -> Result<CodexStatus> {
        rt!(self, => |s| {
            Ok(codex_status(s.providers.codex.status(&s.store().settings()).await))
        })
    }

    async fn get_cursor_status(&self) -> Result<CursorStatus> {
        rt!(self, => |s| Ok(s.cursor_status_at().await))
    }

    async fn get_instance_label(&self) -> Result<String> {
        Ok(recur_platform::identity::instance_label())
    }

    async fn get_provider_usage(&self) -> Result<Snapshot> {
        rt!(self, => |s| Ok(s.usage.get_provider_usage().await))
    }

    async fn get_resource(&self) -> Result<Resource> {
        rt!(self, => |s| {
            let mut r = s.store().resource();
            r.git_warning = s.engine.git_warning().to_owned();
            Ok(r)
        })
    }

    async fn get_settings(&self) -> Result<Settings> {
        rt!(self, => |s| Ok(s.settings_view()))
    }

    async fn github_login(&self) -> Result<String> {
        rt!(self, => |s| Ok(s.github_login_at().await))
    }

    async fn install_cursor_bridge(&self) -> Result<CursorStatus> {
        rt!(self, => |s| s.install_cursor_bridge_at().await)
    }

    async fn list_assigned_issues(&self) -> Result<Vec<AssignedIssue>> {
        rt!(self, => |s| Ok(s.list_assigned_issues_at().await))
    }

    async fn list_model_catalog(&self) -> Result<Vec<ModelGroup>> {
        rt!(self, => |s| Ok(s.model_catalog().await))
    }

    async fn list_monospace_fonts(&self) -> Result<Vec<String>> {
        rt!(self, => |_s| blocking(|| Ok(recur_platform::osfonts::monospace_families())).await)
    }

    async fn logs_path(&self) -> Result<String> {
        Ok(self.logs_path_at())
    }

    async fn open_linear_in_app(&self, raw: &str) -> Result<bool> {
        rt!(self, raw => |s| Ok(s.open_linear_in_app_at(&raw).await))
    }

    async fn refresh_provider_usage(&self) -> Result<Snapshot> {
        rt!(self, => |s| Ok(s.usage.refresh_provider_usage().await))
    }

    async fn reveal_logs(&self) -> Result<()> {
        rt!(self, => |s| {
            recur_store::paths::logs_dir(s.data_dir()).map_err(err)?;
            recur_code::reveal_in_finder(&s.logs_path_at()).map_err(err)
        })
    }

    async fn set_settings(&self, set: &Settings) -> Result<Settings> {
        rt!(self, set => |s| s.save_settings(set).await)
    }

    async fn test_codex_key(&self, api_key: &str, chat_gpt_auth: bool) -> Result<CodexStatus> {
        rt!(self, api_key, chat_gpt_auth => |s| {
            let draft = CredentialDraft {
                api_key,
                chatgpt_auth: chat_gpt_auth,
                compatible: None,
            };
            let st = s
                .providers
                .codex
                .test_credentials(&s.store().settings(), &draft)
                .await;
            Ok(codex_status(st))
        })
    }

    async fn test_compatible_provider(
        &self,
        p: &CompatibleProvider,
    ) -> Result<CompatibleTestResult> {
        rt!(self, p => |_s| Ok(recur_provider_compatible::test_compatible_provider(p).await))
    }

    async fn test_cursor_key(&self, api_key: &str) -> Result<CursorStatus> {
        rt!(self, api_key => |s| {
            Ok(s.providers.cursor.test_cursor_key(&s.store().settings(), &api_key).await)
        })
    }
}
